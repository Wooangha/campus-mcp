//! Read-only assignment discovery and submission facts from PLMS HTML.

#[path = "deadline_parse.rs"]
mod deadline_parse;

use crate::services::lms::plms::{PlmsClient, http::PLMS, page::text_of};
use crate::services::{
    lms::{Course, Scope},
    submissions::{Assignment, Snapshot, Status, SubmissionError, SubmittedFile},
};
use regex::Regex;
use scraper::{ElementRef, Html, Selector};
use std::sync::LazyLock;

static LINKS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?is)<a\b[^>]*href=["']([^"']+)["'][^>]*>(.*?)</a>"#).unwrap());
static ROWS: LazyLock<Selector> = LazyLock::new(|| Selector::parse("tr").unwrap());

impl PlmsClient {
    pub async fn submission_assignments(&self) -> Result<Vec<Assignment>, SubmissionError> {
        let courses = self.enrolled(Scope::CurrentTerm).await?;
        let mut assignments = Vec::new();
        for course in courses {
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            let html = self.session()?.submission_page(&course.url).await?;
            assignments.extend(assignment_links(&html, &course)?);
        }
        assignments.sort_by(|a, b| a.url.cmp(&b.url));
        assignments.dedup_by(|a, b| a.url == b.url);
        Ok(assignments)
    }

    pub async fn submission_status(
        &self,
        assignment: &Assignment,
    ) -> Result<Snapshot, SubmissionError> {
        let url = url::Url::parse(&assignment.url)
            .map_err(|_| SubmissionError::Unreadable("잘못된 과제 주소".into()))?;
        if url.origin().ascii_serialization() != PLMS
            || url.path() != "/mod/assign/view.php"
            || url.query_pairs().any(|(key, _)| key != "id")
        {
            return Err(SubmissionError::Unreadable(
                "허용되지 않은 과제 주소".into(),
            ));
        }
        let html = self.session()?.submission_page(url.as_str()).await?;
        parse_submission(&html)
    }
}

fn assignment_links(html: &str, course: &Course) -> Result<Vec<Assignment>, SubmissionError> {
    let base = url::Url::parse(&course.url)
        .map_err(|_| SubmissionError::Unreadable("잘못된 과목 주소".into()))?;
    let mut result = Vec::new();
    for link in LINKS.captures_iter(html) {
        let Ok(mut url) = base.join(&link[1].replace("&amp;", "&")) else {
            continue;
        };
        if url.origin().ascii_serialization() != PLMS || url.path() != "/mod/assign/view.php" {
            continue;
        }
        let Some(id) = url
            .query_pairs()
            .find(|(key, _)| key == "id")
            .and_then(|(_, id)| id.parse::<u64>().ok())
        else {
            continue;
        };
        url.set_query(Some(&format!("id={id}")));
        url.set_fragment(None);
        let title = text_of(&link[2]);
        if title.is_empty() {
            continue;
        }
        result.push(Assignment {
            url: url.into(),
            title,
            course: course.name.clone(),
        });
    }
    Ok(result)
}

fn parse_submission(html: &str) -> Result<Snapshot, SubmissionError> {
    let mut status = None;
    let mut modified = None;
    let mut due = None;
    let mut extension_due = None;
    let mut online_text = false;
    let document = Html::parse_document(html);
    for row in document.select(&ROWS) {
        // Submitted online text can itself contain tables. Those rows are
        // student content, not submission metadata, and must not override it.
        if row
            .ancestors()
            .filter_map(ElementRef::wrap)
            .any(|ancestor| ancestor.value().name() == "tr")
        {
            continue;
        }
        let raw_cells: Vec<_> = row
            .children()
            .filter_map(ElementRef::wrap)
            .filter(|cell| matches!(cell.value().name(), "th" | "td"))
            .map(|cell| cell.inner_html())
            .collect();
        let cells: Vec<_> = raw_cells.iter().map(|cell| text_of(cell)).collect();
        if cells.len() != 2 {
            continue;
        }
        let label = cells[0].trim_end_matches(':').trim().to_ascii_lowercase();
        if matches!(
            label.as_str(),
            "extension due date"
                | "연장 종료 일시"
                | "연장 종료일시"
                | "연장 마감일"
                | "연장 마감 일시"
                | "연장 마감일시"
        ) && let Some(parsed) = deadline_parse::parse(&raw_cells[1])
        {
            extension_due = Some(parsed);
        }
        if matches!(
            label.as_str(),
            "due date"
                | "종료일시"
                | "종료 일시"
                | "마감일"
                | "마감 일시"
                | "마감일시"
                | "제출 마감일"
                | "제출 마감일시"
        ) && let Some(parsed) = deadline_parse::parse(&raw_cells[1])
        {
            due = Some(parsed);
        }
        match cells[0].as_str() {
            "제출 여부" | "제출 상태" | "Submission status" => {
                status = Some(match cells[1].as_str() {
                    "제출 완료" | "Submitted for grading" => Status::Submitted,
                    "과제에서 온라인 제출물을 요구하지 않습니다."
                    | "This assignment does not require you to submit anything online"
                    | "This assignment does not require you to submit anything online." => {
                        Status::NoOnlineSubmission
                    }
                    "제출 안 함"
                    | "미제출"
                    | "No submissions have been made yet"
                    | "Nothing has been submitted for this assignment" => Status::NotSubmitted,
                    text if text.contains("초안") || text.starts_with("Draft") => Status::Draft,
                    _ => Status::Unknown,
                });
            }
            "최종 수정 일시" | "Last modified" if !cells[1].is_empty() && cells[1] != "-" => {
                modified = Some(cells[1].clone());
            }
            "온라인 텍스트" | "Online text" if !cells[1].is_empty() && cells[1] != "-" => {
                online_text = true;
            }
            _ => {}
        }
    }
    let status = status
        .ok_or_else(|| SubmissionError::Unreadable("제출 상태 표를 찾을 수 없습니다".into()))?;
    if status == Status::Unknown {
        return Err(SubmissionError::Unreadable(
            "알 수 없는 제출 상태입니다".into(),
        ));
    }
    let mut files = Vec::new();
    for link in LINKS.captures_iter(html) {
        let raw = link[1].replace("&amp;", "&");
        let Ok(mut url) = url::Url::parse(PLMS).unwrap().join(&raw) else {
            continue;
        };
        if url.origin().ascii_serialization() != PLMS
            || !url.path().contains("/assignsubmission_file/")
        {
            continue;
        }
        // Download flags are presentation details, not a new submission version.
        url.set_query(None);
        url.set_fragment(None);
        files.push(SubmittedFile {
            name: text_of(&link[2]),
            url: url.into(),
        });
    }
    files.sort_by(|a, b| a.url.cmp(&b.url));
    files.dedup_by(|a, b| a.url == b.url);
    Ok(Snapshot {
        status,
        files,
        modified,
        online_text,
        due: extension_due.or(due),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    const SUBMITTED: &str = r#"<table><tr><th>제출 여부</th><td class="submissionstatussubmitted">제출 완료</td></tr>
<tr><th>최종 수정 일시</th><td>2026-09-24 21:24</td></tr></table>
<a href="https://plms.postech.ac.kr/pluginfile.php/1/assignsubmission_file/submission_files/2/report.zip?forcedownload=1">report.zip</a>
<a href="https://plms.postech.ac.kr/pluginfile.php/1/mod_assign/introattachment/0/instructions.pdf">instructions.pdf</a>"#;
    #[test]
    fn nested_online_text_tables_cannot_replace_submission_metadata() {
        let html = format!(
            r#"{SUBMITTED}<table>
<tr><th>온라인 텍스트</th><td><table>
<tr><td>Submission status</td><td>Nothing has been submitted for this assignment</td></tr>
<tr><td>Last modified</td><td>fake modification</td></tr>
<tr><td>Due date</td><td>2030-01-01 00:00</td></tr>
</table></td></tr></table>"#
        );
        let snapshot = parse_submission(&html).unwrap();
        assert!(snapshot.online_text);
        assert_eq!(snapshot.status, Status::Submitted);
        assert_eq!(snapshot.modified.as_deref(), Some("2026-09-24 21:24"));
        assert!(snapshot.due.is_none());
    }

    #[test]
    fn extension_deadline_overrides_original_regardless_of_row_order() {
        let original = "<tr><th>Due date</th><td>2026-10-20 23:59</td></tr>";
        let expected = chrono::DateTime::parse_from_rfc3339("2026-10-22T23:59:00+09:00")
            .unwrap()
            .timestamp();
        for label in ["Extension due date", "연장 종료 일시"] {
            let extension =
                format!("<tr><th>{label}</th><td>2026년 10월 22일(목요일), 23:59</td></tr>");
            for rows in [
                format!("{original}{extension}"),
                format!("{extension}{original}"),
            ] {
                let html = format!("{SUBMITTED}<table>{rows}</table>");
                assert_eq!(parse_submission(&html).unwrap().due, Some(expected));
            }
            let only_extension = format!("{SUBMITTED}<table>{extension}</table>");
            assert_eq!(
                parse_submission(&only_extension).unwrap().due,
                Some(expected)
            );
        }
    }

    #[test]
    fn only_due_labels_contribute_deadlines_and_missing_dates_preserve_status() {
        let expected = chrono::DateTime::parse_from_rfc3339("2026-10-20T23:59:00+09:00")
            .unwrap()
            .timestamp();
        for label in ["종료일시", "마감일", "Due date"] {
            let html = format!(
                "{SUBMITTED}<table><tr><th>{label}</th><td>2026년 10월 20일(화요일), 23:59</td></tr></table>"
            );
            let snapshot = parse_submission(&html).unwrap();
            assert_eq!(snapshot.due, Some(expected));
            assert_eq!(snapshot.status, Status::Submitted);
        }
        for label in [
            "최종 수정 일시",
            "Last modified",
            "Cut-off date",
            "제출 시작일",
            "남은 시간",
        ] {
            let html = format!(
                "{SUBMITTED}<table><tr><th>{label}</th><td>2026-10-20 23:59</td></tr></table>"
            );
            assert!(parse_submission(&html).unwrap().due.is_none());
        }
        let html =
            format!("{SUBMITTED}<table><tr><th>Due date</th><td>2026-10-20</td></tr></table>");
        let snapshot = parse_submission(&html).unwrap();
        assert!(snapshot.due.is_none());
        assert_eq!(snapshot.status, Status::Submitted);
    }
    #[test]
    fn due_row_datetime_attribute_is_retained_for_parsing() {
        let html = format!(
            r#"{SUBMITTED}<table><tr><th>Due date</th><td><time datetime="2026-10-20T14:59:00Z">unrecognized local text</time></td></tr></table>"#
        );
        assert_eq!(
            parse_submission(&html).unwrap().due,
            Some(
                chrono::DateTime::parse_from_rfc3339("2026-10-20T23:59:00+09:00")
                    .unwrap()
                    .timestamp()
            )
        );
    }

    #[test]
    fn parses_submission_facts_without_mistaking_instructions_for_uploads() {
        let s = parse_submission(SUBMITTED).unwrap();
        assert_eq!(s.status, Status::Submitted);
        assert_eq!(s.modified.as_deref(), Some("2026-09-24 21:24"));
        assert_eq!(s.files.len(), 1);
        assert_eq!(s.files[0].name, "report.zip");
    }
    #[test]
    fn files_do_not_imply_final_submission_and_unknown_is_not_unsubmitted() {
        assert_eq!(
            parse_submission(&SUBMITTED.replace("제출 완료", "초안 (미제출)"))
                .unwrap()
                .status,
            Status::Draft
        );
        assert_eq!(
            parse_submission(&SUBMITTED.replace("제출 완료", "제출 안 함"))
                .unwrap()
                .status,
            Status::NotSubmitted
        );
        assert!(parse_submission(&SUBMITTED.replace("제출 완료", "new status")).is_err());
        assert!(parse_submission("<html>로그인</html>").is_err());
    }
    #[test]
    fn offline_assignments_are_neither_unsubmitted_nor_completed() {
        let html = SUBMITTED.replace("제출 완료", "과제에서 온라인 제출물을 요구하지 않습니다.");
        assert_eq!(
            parse_submission(&html).unwrap().status,
            Status::NoOnlineSubmission
        );
        assert!(matches!(
            parse_submission(&SUBMITTED.replace("제출 완료", "unrecognized")),
            Err(SubmissionError::Unreadable(_))
        ));
    }

    #[test]
    fn discovery_canonicalizes_links_and_rejects_external_hosts() {
        let c = Course {
            id: 1,
            name: "course".into(),
            url: format!("{PLMS}/course/view.php?id=1"),
            term: None,
            ends: None,
        };
        let html = r#"<a href="/mod/assign/view.php?id=42&amp;forceview=1"><span>Project</span></a><a href="https://evil.test/mod/assign/view.php?id=1">bad</a>"#;
        let a = assignment_links(html, &c).unwrap();
        assert_eq!(a.len(), 1);
        assert_eq!(a[0].url, format!("{PLMS}/mod/assign/view.php?id=42"));
    }
}
