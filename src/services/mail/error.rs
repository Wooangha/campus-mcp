//! What can go wrong reading the mailbox.

use thiserror::Error;

/// Failures of the Outlook mail reader. Kept apart from the LMS reader's
/// error: the two speak to different servers with different auth.
#[derive(Debug, Error)]
pub enum MailError {
    #[error(transparent)]
    Content(#[from] crate::services::content::ContentError),
    #[error("Outlook 인증이 필요합니다: {0}")]
    Auth(String),
    #[error("Microsoft Graph rejected the request ({0})")]
    Service(String),
    #[error("invalid response from Microsoft Graph")]
    InvalidResponse,
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Http(#[from] reqwest::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}
