//! What any LMS hands back, whichever backend read it.

use std::fmt;

/// Fallback for timestamps outside chrono's supported range.
const DAY: i64 = 86_400;

/// What a deadline is for. Backends translate their own activity names into
/// these, so callers never match on a backend's vocabulary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Activity {
    Assignment,
    Quiz,
    /// A recorded lecture to watch.
    Video,
    Attendance,
    /// A board to post on or read.
    Board,
    /// Anything else, under the backend's own name for it.
    Other(String),
}

impl Activity {
    /// The activity in the words students use for it.
    pub fn label(&self) -> &str {
        match self {
            Self::Assignment => "과제",
            Self::Quiz => "퀴즈",
            Self::Video => "강의 영상",
            Self::Attendance => "출석",
            Self::Board => "게시판",
            Self::Other(name) => name,
        }
    }
}

/// A dated item the LMS wants the user to act on: an assignment to submit, a
/// lecture to watch, a quiz to take.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Deadline {
    /// The LMS's own wording, such as "Project 1 is due".
    pub name: String,
    pub course: String,
    pub kind: Activity,
    /// When it comes due, in Unix seconds.
    pub due: i64,
    /// The LMS's own localized rendering of that time.
    pub due_text: String,
    pub url: String,
    /// What the LMS offers to do about it, such as "과제 제출하기" (submit).
    pub action: Option<String>,
}

impl Deadline {
    /// Whether this is an assignment submission rather than another activity.
    pub fn is_assignment(&self) -> bool {
        self.kind == Activity::Assignment
    }

    /// Calendar days until the deadline in the process local timezone.
    pub fn days_left(&self, now: i64) -> i64 {
        match (
            chrono::DateTime::from_timestamp(self.due, 0),
            chrono::DateTime::from_timestamp(now, 0),
        ) {
            (Some(due), Some(now)) => (due.with_timezone(&chrono::Local).date_naive()
                - now.with_timezone(&chrono::Local).date_naive())
            .num_days(),
            _ => self.due.saturating_sub(now).div_euclid(DAY),
        }
    }

    /// The D-day label: `D-3`, `D-0` when due today, and `지남` ("past") afterwards.
    pub fn dday(&self, now: i64) -> String {
        if self.due < now {
            return "지남".to_owned();
        }
        match self.days_left(now) {
            days if days < 0 => "지남".to_owned(),
            days => format!("D-{days}"),
        }
    }
}

/// An academic term.
///
/// Ordering is chronological, so the newest term is the greatest.
///
/// This is the one model still shaped like its only backend: `code` holds
/// POSTECH's own term codes, and [`Term::label`] reads them back in POSTECH's
/// words. It is left that way on purpose. What a neutral term looks like — a
/// season enum, a free-form label, a sort key — depends on how the second LMS
/// names its terms, and guessing now would mean redoing it then. When that
/// backend arrives: have each backend translate into whatever replaces `code`,
/// as `Activity` already does for deadline kinds, and keep the ordering, since
/// picking the current term relies on `max()`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Term {
    pub year: u16,
    /// POSTECH's term code, ordered in time within the year: 090 spring,
    /// 091 summer, 092 autumn, 093 winter.
    pub code: u16,
}

impl Term {
    /// The term in the words students use for it.
    pub fn label(&self) -> String {
        let season = match self.code {
            90 => "1학기",
            91 => "여름학기",
            92 => "2학기",
            93 => "겨울학기",
            _ => return self.to_string(),
        };
        format!("{}학년도 {season}", self.year)
    }
}

impl fmt::Display for Term {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}-{:03}", self.year, self.code)
    }
}

/// A course the user is enrolled in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Course {
    pub id: i64,
    pub name: String,
    pub url: String,
    /// The term it belongs to, when the LMS says.
    pub term: Option<Term>,
    /// When it closes, when the LMS says.
    pub ends: Option<i64>,
}

/// A posting on a course's announcement board.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Announcement {
    pub course: String,
    pub title: String,
    /// `YYYY-MM-DD`, so that ordering as text orders in time.
    pub date: String,
    pub url: String,
}

/// A file attached to a posting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attachment {
    pub name: String,
    pub url: String,
}

/// The full text of an announcement, read from its own page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Posting {
    pub title: String,
    pub author: String,
    /// `YYYY-MM-DD HH:MM:SS`.
    pub posted: String,
    /// The body as plain text, with its paragraphs and list items preserved.
    pub body: String,
    pub attachments: Vec<Attachment>,
    pub url: String,
}

/// The term the given courses belong to, when they agree on one.
pub fn term_of_courses(courses: &[Course]) -> Option<Term> {
    let mut terms = courses.iter().filter_map(|course| course.term);
    let first = terms.next()?;
    terms.all(|term| term == first).then_some(first)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn course(id: i64, term: Option<Term>) -> Course {
        Course {
            id,
            name: format!("course {id}"),
            url: format!("https://lms.example/course/{id}"),
            term,
            ends: None,
        }
    }

    const AUTUMN: Term = Term {
        year: 2026,
        code: 92,
    };
    const SPRING: Term = Term {
        year: 2026,
        code: 90,
    };

    #[test]
    fn terms_order_chronologically_and_read_back_in_student_words() {
        assert!(
            Term {
                year: 2025,
                code: 92
            } < SPRING
        );
        assert!(SPRING < AUTUMN);
        assert_eq!(AUTUMN.to_string(), "2026-092");
        assert_eq!(AUTUMN.label(), "2026학년도 2학기");
        assert_eq!(SPRING.label(), "2026학년도 1학기");
    }

    #[test]
    fn a_shared_term_is_reported_and_a_mixed_one_is_not() {
        let same = vec![course(1, Some(AUTUMN)), course(2, Some(AUTUMN))];
        assert_eq!(term_of_courses(&same), Some(AUTUMN));
        let mixed = vec![course(1, Some(AUTUMN)), course(2, Some(SPRING))];
        assert_eq!(term_of_courses(&mixed), None);
    }

    fn deadline(kind: Activity, due: i64) -> Deadline {
        Deadline {
            name: "x".into(),
            course: "y".into(),
            kind,
            due,
            due_text: String::new(),
            url: String::new(),
            action: None,
        }
    }

    #[test]
    fn only_assignments_are_assignments() {
        assert!(deadline(Activity::Assignment, 0).is_assignment());
        assert!(!deadline(Activity::Video, 0).is_assignment());
    }

    #[test]
    fn dday_uses_calendar_dates_and_checks_expiry_separately() {
        use chrono::{Local, TimeZone};
        let at = |day, hour, minute| {
            Local
                .with_ymd_and_hms(2026, 9, day, hour, minute, 0)
                .single()
                .unwrap()
                .timestamp()
        };
        let now = at(26, 23, 30);
        assert_eq!(
            deadline(Activity::Assignment, at(27, 0, 30)).dday(now),
            "D-1"
        );
        assert_eq!(
            deadline(Activity::Assignment, at(26, 23, 59)).dday(now),
            "D-0"
        );
        assert_eq!(
            deadline(Activity::Assignment, at(26, 23, 0)).dday(now),
            "지남"
        );
        assert_eq!(
            deadline(Activity::Assignment, at(29, 0, 0)).days_left(now),
            3
        );
    }

    #[test]
    fn an_unknown_activity_keeps_the_backends_name() {
        assert_eq!(Activity::Assignment.label(), "과제");
        assert_eq!(Activity::Other("workshop".into()).label(), "workshop");
    }
}
