//! Reading a learning management system, whichever one it is.
//!
//! [`Lms`] is what a signed-in LMS can be asked: what is due, which courses,
//! what was announced, what does this posting say. Backends implement it —
//! today [`plms`], POSTECH's Moodle — and callers hold a `dyn Lms`, so a second
//! LMS is a new submodule rather than a change to everything that reads one.
//!
//! Signing in is deliberately *not* part of the trait. How an LMS authenticates
//! is the most backend-specific thing about it (SSO handoff, API token, OAuth),
//! so each backend signs in its own way and the trait begins once that is done.
//! [`connect`] is the one place that knows every backend: it takes an
//! [`LmsConfig`] and hands back a signed-in `dyn Lms`.
//!
//! Supporting another LMS is a sibling of [`plms`], a variant of [`LmsConfig`],
//! and an arm in [`connect`].

mod config;
mod error;
mod model;
pub mod plms;

use async_trait::async_trait;

pub use config::LmsConfig;
pub use error::LmsError;
pub use model::{
    Activity, Announcement, Attachment, Course, Deadline, Posting, Term, term_of_courses,
};

/// Which courses a query covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Scope {
    /// Only the term in progress — what a student means by "my courses".
    #[default]
    CurrentTerm,
    /// Every course ever enrolled in.
    All,
}

/// A signed-in LMS.
#[async_trait]
pub trait Lms: Send + Sync {
    /// What to call this LMS in front of the user, such as "PLMS".
    fn name(&self) -> &str;

    /// Deadlines still ahead, soonest first.
    async fn upcoming(&self, limit: u32) -> Result<Vec<Deadline>, LmsError>;

    /// The courses within `scope`.
    async fn courses(&self, scope: Scope) -> Result<Vec<Course>, LmsError>;

    /// Announcements from the courses within `scope`, newest first.
    async fn announcements(&self, scope: Scope) -> Result<Vec<Announcement>, LmsError> {
        let courses = self.courses(scope).await?;
        self.announcements_for(&courses).await
    }

    /// Full bounded board collection with partial-failure warnings.
    async fn collect_announcements(
        &self,
        scope: Scope,
    ) -> Result<(Vec<Announcement>, Vec<String>), LmsError> {
        Ok((self.announcements(scope).await?, Vec::new()))
    }

    /// Announcements from an already fetched course list, newest first.
    async fn announcements_for(&self, courses: &[Course]) -> Result<Vec<Announcement>, LmsError>;

    /// The full text of a posting, given its URL.
    async fn read_posting(&self, url: &str) -> Result<Posting, LmsError>;

    /// Full source text and attachment metadata for an announcement or assignment.
    async fn read_content(&self, url: &str) -> Result<crate::services::content::Content, LmsError> {
        let posting = self.read_posting(url).await?;
        Ok(crate::services::content::Content {
            title: posting.title,
            body: posting.body,
            attachments: posting
                .attachments
                .into_iter()
                .map(|a| crate::services::content::Attachment::remote(a.name, a.url, false))
                .collect(),
            warnings: Vec::new(),
        })
    }

    /// Download through this backend's authenticated session, with a byte limit.
    async fn download_attachment(&self, _url: &str, _max: usize) -> Result<Vec<u8>, LmsError> {
        Err(LmsError::Backend(Box::new(
            crate::services::content::ContentError::Unsupported,
        )))
    }

    /// The assignments among [`Lms::upcoming`].
    async fn upcoming_assignments(&self, limit: u32) -> Result<Vec<Deadline>, LmsError> {
        Ok(self
            .upcoming(limit)
            .await?
            .into_iter()
            .filter(Deadline::is_assignment)
            .collect())
    }

    /// The full text of an announcement.
    async fn read(&self, announcement: &Announcement) -> Result<Posting, LmsError> {
        self.read_posting(&announcement.url).await
    }
}

/// Signs in to the configured LMS.
pub async fn connect(config: &LmsConfig) -> Result<Box<dyn Lms>, LmsError> {
    match config {
        LmsConfig::Plms(plms) => Ok(Box::new(plms::PlmsClient::connect(plms).await?)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A backend that knows two deadlines, to exercise the provided methods.
    struct Fake;

    fn deadline(name: &str, kind: Activity) -> Deadline {
        Deadline {
            name: name.into(),
            course: "course".into(),
            kind,
            due: 0,
            due_text: String::new(),
            url: String::new(),
            action: None,
        }
    }

    #[async_trait]
    impl Lms for Fake {
        fn name(&self) -> &str {
            "Fake"
        }

        async fn upcoming(&self, _limit: u32) -> Result<Vec<Deadline>, LmsError> {
            Ok(vec![
                deadline("lecture", Activity::Video),
                deadline("hw", Activity::Assignment),
            ])
        }

        async fn courses(&self, _scope: Scope) -> Result<Vec<Course>, LmsError> {
            Ok(Vec::new())
        }

        async fn announcements_for(
            &self,
            _courses: &[Course],
        ) -> Result<Vec<Announcement>, LmsError> {
            Ok(Vec::new())
        }

        async fn read_posting(&self, url: &str) -> Result<Posting, LmsError> {
            Ok(Posting {
                title: "title".into(),
                author: String::new(),
                posted: String::new(),
                body: String::new(),
                attachments: Vec::new(),
                url: url.into(),
            })
        }
    }

    #[tokio::test]
    async fn assignments_are_filtered_from_whatever_the_backend_lists() {
        let lms: Box<dyn Lms> = Box::new(Fake);
        let found = lms.upcoming_assignments(10).await.unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].name, "hw");
    }

    #[tokio::test]
    async fn an_announcement_is_read_through_its_url() {
        let lms: Box<dyn Lms> = Box::new(Fake);
        let announcement = Announcement {
            course: "course".into(),
            title: "title".into(),
            date: "2026-09-20".into(),
            url: "https://lms.example/post/1".into(),
        };
        let posting = lms.read(&announcement).await.unwrap();
        assert_eq!(posting.url, "https://lms.example/post/1");
    }
}
