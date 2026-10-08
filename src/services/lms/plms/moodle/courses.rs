//! The enrolled courses, and which of them belong to the current term.

use std::sync::LazyLock;

use regex::Regex;

use super::service::{service_data, string};
use crate::services::lms::{
    Course, Scope, Term,
    plms::{PlmsError, http::PLMS, sso::PlmsClient},
};
use crate::services::time::now_unix;

/// The service behind the dashboard's course overview block.
const ENROLLED_COURSES: &str = "core_course_get_enrolled_courses_by_timeline_classification";
/// How many courses to ask for; well above a normal term's load.
const COURSE_LIMIT: u32 = 50;

/// PLMS stamps the term into a course's short name: `운영체제 (2026-092, ...)`.
static TERM: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\((\d{4})-(\d{3})[,)]").unwrap());

impl PlmsClient {
    /// The enrolled courses within `scope`.
    ///
    /// Moodle's own "in progress" list cannot be trusted for the current term:
    /// a course PLMS left without an end date stays "in progress" forever,
    /// which is how last year's courses kept turning up. The term stamped into
    /// each short name is what actually decides.
    pub(super) async fn enrolled(&self, scope: Scope) -> Result<Vec<Course>, PlmsError> {
        let classification = match scope {
            Scope::CurrentTerm => "inprogress",
            Scope::All => "all",
        };
        let response = self
            .service(
                ENROLLED_COURSES,
                serde_json::json!({
                    "offset": 0,
                    "limit": COURSE_LIMIT,
                    "classification": classification,
                    "sort": "fullname",
                }),
            )
            .await?;
        let mut courses = courses(&response)?;
        if scope == Scope::CurrentTerm {
            retain_current_term(&mut courses, now_unix());
        }
        Ok(courses)
    }
}

/// Reads the course overview service's response.
fn courses(json: &str) -> Result<Vec<Course>, PlmsError> {
    let data = service_data(json)?;
    let listed = data["courses"]
        .as_array()
        .ok_or(PlmsError::InvalidMoodleResponse)?;
    Ok(listed
        .iter()
        .map(|course| {
            let id = course["id"].as_i64().unwrap_or_default();
            Course {
                id,
                name: string(&course["fullname"]),
                url: format!("{PLMS}/course/view.php?id={id}"),
                term: term_of(&string(&course["shortname"])),
                // Moodle reports a missing end date as 0, not as null.
                ends: course["enddate"].as_i64().filter(|end| *end > 0),
            }
        })
        .collect())
}

/// The term a course's short name is stamped with, if it carries one.
fn term_of(shortname: &str) -> Option<Term> {
    let found = TERM.captures(shortname)?;
    Some(Term {
        year: found[1].parse().ok()?,
        code: found[2].parse().ok()?,
    })
}

/// Narrows a list to the newest term present.
///
/// With no term stamps to go on it falls back to the end dates, keeping the
/// courses that have not closed yet — which still drops the undated strays,
/// since those are exactly the ones with no end date at all.
fn retain_current_term(courses: &mut Vec<Course>, now: i64) {
    match courses.iter().filter_map(|course| course.term).max() {
        Some(current) => courses.retain(|course| course.term == Some(current)),
        None => courses.retain(|course| course.ends.is_some_and(|end| end > now)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn courses_are_read_with_a_page_url_each() {
        let json = r#"[{"error":false,"data":{"courses":[
            {"id":15725,"fullname":"데이터베이스시스템","shortname":"DB (2026-092)"},
            {"id":15766,"fullname":"운영체제","shortname":"OS (2026-092)"}
        ],"nextoffset":2}}]"#;
        let found = courses(json).unwrap();
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].id, 15725);
        assert_eq!(found[0].name, "데이터베이스시스템");
        assert_eq!(
            found[0].url,
            "https://plms.postech.ac.kr/course/view.php?id=15725"
        );
    }

    fn course(id: i64, shortname: &str, ends: Option<i64>) -> Course {
        Course {
            id,
            name: format!("course {id}"),
            url: format!("{PLMS}/course/view.php?id={id}"),
            term: term_of(shortname),
            ends,
        }
    }

    #[test]
    fn a_term_is_read_from_the_short_name() {
        assert_eq!(
            term_of("운영체제 (2026-092, 01400738_01661235)"),
            Some(Term {
                year: 2026,
                code: 92
            })
        );
        assert_eq!(
            term_of("기계학습 (2025-092, 01403209_01664031)-HAPBAN-CSMS-01403209"),
            Some(Term {
                year: 2025,
                code: 92
            })
        );
        assert_eq!(term_of("online course with no stamp"), None);
    }

    /// The exact shape that leaked last year's courses into the listing: two
    /// courses Moodle still calls "in progress" only because they never end.
    #[test]
    fn an_undated_course_from_an_older_term_is_dropped() {
        let mut courses = vec![
            course(14109, "기계학습 (2025-092, x)-HAPBAN", None),
            course(
                15725,
                "데이터베이스시스템 (2026-092, x)",
                Some(1_797_000_000),
            ),
            course(11203, "프로그래밍과문제해결 (2024-092, x)-HAPBAN", None),
            course(15766, "운영체제 (2026-092, x)", Some(1_797_000_000)),
        ];
        retain_current_term(&mut courses, 1_789_000_000);
        let kept: Vec<i64> = courses.iter().map(|course| course.id).collect();
        assert_eq!(kept, vec![15725, 15766]);
    }

    #[test]
    fn without_term_stamps_the_end_dates_decide() {
        let mut courses = vec![
            course(1, "no stamp, still running", Some(1_797_000_000)),
            course(2, "no stamp, long finished", Some(1_700_000_000)),
            course(3, "no stamp, never ends", None),
        ];
        retain_current_term(&mut courses, 1_789_000_000);
        let kept: Vec<i64> = courses.iter().map(|course| course.id).collect();
        assert_eq!(kept, vec![1]);
    }

    #[test]
    fn a_missing_end_date_is_read_as_absent_not_as_1970() {
        let json = r#"[{"error":false,"data":{"courses":[
            {"id":1,"fullname":"기계학습","shortname":"기계학습 (2025-092, x)","enddate":0},
            {"id":2,"fullname":"운영체제","shortname":"운영체제 (2026-092, x)","enddate":1797000000}
        ]}}]"#;
        let found = courses(json).unwrap();
        assert_eq!(found[0].ends, None);
        assert_eq!(found[1].ends, Some(1_797_000_000));
    }

    #[test]
    fn a_service_error_reaches_the_caller() {
        let json = r#"[{"error":true,"exception":{"message":"Invalid session key"}}]"#;
        assert!(matches!(
            courses(json).unwrap_err(),
            PlmsError::MoodleService(_)
        ));
    }
}
