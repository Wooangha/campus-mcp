//! The Moodle side of PLMS, once SSO has established a session. This is where
//! [`PlmsClient`] becomes an [`Lms`].
//!
//! One file per thing Moodle is asked for:
//!
//! - [`service`] — how an AJAX service is called, and the envelope it answers in.
//! - [`calendar`] — the dashboard timeline: deadlines.
//! - [`courses`] — the enrolled courses, and which belong to the current term.
//! - [`board`] — course announcements and their postings.
//!
//! Deadlines and courses go through Moodle's own AJAX services rather than its
//! rendered pages: the services answer with JSON, so they survive theme changes
//! that would break scraping. Announcements have no such service and are read
//! off the pages.

mod board;
mod board_list;
mod calendar;
mod content;
mod courses;
mod service;
mod submission;

use async_trait::async_trait;

use crate::services::lms::{
    Announcement, Course, Deadline, Lms, LmsError, Posting, Scope, plms::sso::PlmsClient,
};

#[async_trait]
impl Lms for PlmsClient {
    fn name(&self) -> &str {
        "PLMS"
    }

    async fn upcoming(&self, limit: u32) -> Result<Vec<Deadline>, LmsError> {
        Ok(self.timeline(limit).await?)
    }

    async fn courses(&self, scope: Scope) -> Result<Vec<Course>, LmsError> {
        Ok(self.enrolled(scope).await?)
    }

    /// One request per course, so [`Scope::All`] costs one for every course
    /// ever taken.
    async fn announcements_for(&self, courses: &[Course]) -> Result<Vec<Announcement>, LmsError> {
        Ok(self.announcements_of(courses).await?)
    }

    async fn collect_announcements(
        &self,
        scope: Scope,
    ) -> Result<(Vec<Announcement>, Vec<String>), LmsError> {
        let courses = self.courses(scope).await?;
        Ok(self.announcements_with_warnings(&courses, 10, 100).await?)
    }

    async fn read_content(&self, url: &str) -> Result<crate::services::content::Content, LmsError> {
        Ok(PlmsClient::read_content(self, url).await?)
    }
    async fn download_attachment(&self, url: &str, max: usize) -> Result<Vec<u8>, LmsError> {
        Ok(PlmsClient::download_attachment(self, url, max).await?)
    }

    async fn read_posting(&self, url: &str) -> Result<Posting, LmsError> {
        Ok(self.posting(url).await?)
    }
}
