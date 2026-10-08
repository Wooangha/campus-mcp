//! Everything that can go wrong between the first SSO request and a Moodle
//! answer.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum PlmsError {
    #[error(transparent)]
    Content(#[from] crate::services::content::ContentError),
    #[error("missing required SSO input: {0}")]
    MissingInput(&'static str),
    #[error("unexpected redirect origin: {0}")]
    UnexpectedRedirect(String),
    #[error("SSO rejected the login ({0})")]
    AuthenticationFailed(String),
    #[error("POSTECH SSO requires second-factor authentication ({0}); it is not bypassed")]
    SecondFactorRequired(String),
    #[error("invalid response from POSTECH SSO")]
    InvalidSsoResponse,
    #[error("PLMS rejected the SSO handoff ({0})")]
    HandoffRejected(String),
    #[error("PLMS did not establish a Moodle session after SSO")]
    MissingMoodleSession,
    #[error("PLMS did not expose a Moodle session key")]
    MissingSesskey,
    #[error("Moodle rejected the request ({0})")]
    MoodleService(String),
    #[error("invalid response from Moodle")]
    InvalidMoodleResponse,
    #[error("SSO navigation exceeded its expected limit")]
    NavigationLimit,
    #[error(transparent)]
    Http(#[from] reqwest::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Crypto(#[from] openssl::error::ErrorStack),
    #[error(transparent)]
    Url(#[from] url::ParseError),
    #[error(transparent)]
    Hex(#[from] hex::FromHexError),
}
