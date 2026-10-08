//! Course announcements. Moodle has no JSON service for PLMS's `ubboard`, so
//! these are read off the rendered pages.

use crate::services::lms::{
    Announcement, Attachment, Course, Posting,
    plms::{
        PlmsError,
        page::{AnnouncementRow, ArticlePage, announcement_board, article, course_announcements},
        sso::PlmsClient,
    },
};

impl PlmsClient {
    /// Announcements from `courses`, newest first.
    ///
    /// Each course page carries a preview of its board holding the latest few
    /// postings, so this reads one page per course rather than every board.
    /// [`PlmsClient::announcement_board`] gives the board for the full list.
    pub(super) async fn announcements_of(
        &self,
        courses: &[Course],
    ) -> Result<Vec<Announcement>, PlmsError> {
        let session = self.session()?;
        let mut announcements = Vec::new();
        for course in courses {
            let page = session.get(&course.url, None).await?;
            let page = session.follow(page).await?;
            announcements.extend(
                course_announcements(&page.body)
                    .into_iter()
                    .map(|row| announcement(course, row)),
            );
        }
        newest_first(&mut announcements);
        Ok(announcements)
    }

    /// Full boards, with explicit warnings when a course fails or a bound is reached.
    pub async fn announcements_with_warnings(
        &self,
        courses: &[Course],
        max_pages: usize,
        max_items: usize,
    ) -> Result<(Vec<Announcement>, Vec<String>), PlmsError> {
        let session = self.session()?;
        let mut found = Vec::new();
        let mut warnings = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for course in courses {
            let result: Result<(), PlmsError> = async {
                let html = session.board_page(&course.url).await?;
                let base = url::Url::parse(&course.url)?;
                let Some(href) = announcement_board(&html) else {
                    warnings.push(format!("{}: 공지 게시판을 찾지 못했습니다", course.name));
                    return Ok(());
                };
                let mut next = Some(
                    super::board_list::board_url(&base, &href)
                        .ok_or(PlmsError::InvalidMoodleResponse)?,
                );
                let mut count = 0;
                for _ in 0..max_pages.clamp(1, 10) {
                    let Some(url) = next.take() else {
                        break;
                    };
                    tokio::time::sleep(std::time::Duration::from_millis(250)).await;
                    let html = session.board_page(url.as_str()).await?;
                    let page = super::board_list::parse(&html, &url)?;
                    next = page.next;
                    for row in page.rows {
                        if !seen.insert(row.url.clone()) {
                            continue;
                        }
                        if count == max_items.clamp(1, 100) {
                            warnings.push(format!(
                                "{}: 공지 수집 상한에 도달해 일부 항목이 생략됐습니다",
                                course.name
                            ));
                            return Ok(());
                        }
                        found.push(announcement(course, row));
                        count += 1;
                    }
                }
                if next.is_some() {
                    warnings.push(format!(
                        "{}: 공지 페이지 상한에 도달해 이전 글이 생략됐습니다",
                        course.name
                    ));
                }
                Ok(())
            }
            .await;
            if result.is_err() {
                warnings.push(format!(
                    "{}: 공지 목록 수집에 실패했습니다. 수집된 항목만 표시합니다",
                    course.name
                ));
            }
        }
        newest_first(&mut found);
        Ok((found, warnings))
    }

    /// The full text of any `ubboard` posting, given its URL.
    pub(super) async fn posting(&self, url: &str) -> Result<Posting, PlmsError> {
        let session = self.session()?;
        let page = session.get(url, None).await?;
        let page = session.follow(page).await?;
        posting(article(&page.body), page.url.to_string())
    }

    /// The board behind a course's announcement preview, for the postings the
    /// preview leaves out.
    pub async fn announcement_board(&self, course: &Course) -> Result<Option<String>, PlmsError> {
        let session = self.session()?;
        let page = session.get(&course.url, None).await?;
        let page = session.follow(page).await?;
        Ok(announcement_board(&page.body))
    }
}

fn announcement(course: &Course, row: AnnouncementRow) -> Announcement {
    Announcement {
        course: course.name.clone(),
        title: row.title,
        date: row.date,
        url: row.url,
    }
}

/// Dates are `YYYY-MM-DD`, so ordering them as text orders them in time.
fn newest_first(announcements: &mut [Announcement]) {
    announcements.sort_by(|a, b| b.date.cmp(&a.date));
}

/// A scraped article as a [`Posting`]. A page with neither title nor body is
/// not an article at all — a login page, or an error — and is rejected.
fn posting(found: ArticlePage, url: String) -> Result<Posting, PlmsError> {
    if found.title.is_empty() && found.body.is_empty() {
        return Err(PlmsError::InvalidMoodleResponse);
    }
    Ok(Posting {
        title: found.title,
        author: found.author,
        posted: found.posted,
        body: found.body,
        attachments: found
            .attachments
            .into_iter()
            .map(|(name, url)| Attachment { name, url })
            .collect(),
        url,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn course(name: &str) -> Course {
        Course {
            id: 1,
            name: name.into(),
            url: "https://plms.example/course/1".into(),
            term: None,
            ends: None,
        }
    }

    fn row(title: &str, date: &str) -> AnnouncementRow {
        AnnouncementRow {
            title: title.into(),
            date: date.into(),
            url: format!("https://plms.example/{title}"),
        }
    }

    #[test]
    fn a_row_is_tied_to_its_course() {
        let found = announcement(&course("운영체제"), row("중간고사", "2026-10-01"));
        assert_eq!(found.course, "운영체제");
        assert_eq!(found.title, "중간고사");
        assert_eq!(found.date, "2026-10-01");
    }

    #[test]
    fn announcements_from_several_courses_interleave_by_date() {
        let mut found = vec![
            announcement(&course("a"), row("old", "2026-09-01")),
            announcement(&course("b"), row("new", "2026-09-17")),
            announcement(&course("a"), row("mid", "2026-09-09")),
        ];
        newest_first(&mut found);
        let titles: Vec<&str> = found.iter().map(|a| a.title.as_str()).collect();
        assert_eq!(titles, vec!["new", "mid", "old"]);
    }

    #[test]
    fn an_article_becomes_a_posting_with_named_attachments() {
        let found = ArticlePage {
            title: "유고결석 관련 규정".into(),
            author: "김광선".into(),
            posted: "2026-09-09 16:53:20".into(),
            body: "참고하시기 바랍니다.".into(),
            attachments: vec![("유고결석.png".into(), "https://plms.example/f".into())],
        };
        let posting = posting(found, "https://plms.example/article".into()).unwrap();
        assert_eq!(posting.title, "유고결석 관련 규정");
        assert_eq!(posting.url, "https://plms.example/article");
        assert_eq!(
            posting.attachments,
            vec![Attachment {
                name: "유고결석.png".into(),
                url: "https://plms.example/f".into()
            }]
        );
    }

    #[test]
    fn a_page_that_is_not_an_article_is_rejected() {
        assert!(matches!(
            posting(ArticlePage::default(), "https://plms.example/x".into()).unwrap_err(),
            PlmsError::InvalidMoodleResponse
        ));
    }
}
