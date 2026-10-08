//! Read-only assignment submission facts and errors.
mod model;
pub use model::{Assignment, Snapshot, Status, SubmittedFile};

#[derive(Debug, thiserror::Error)]
pub enum SubmissionError {
    #[error("이 과제의 제출 상태를 읽을 수 없습니다: {0}")]
    Unreadable(String),
    #[error("제출 상태 조회 실패: {0}")]
    Source(String),
    #[error("PLMS 재시도 대기 ({0}초)")]
    RetryAfter(u64),
    #[error("PLMS 세션이 만료되었습니다")]
    SessionExpired,
    #[error("제출 이력 오류: {0}")]
    State(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Lms(#[from] crate::services::lms::LmsError),
    #[error(transparent)]
    Plms(#[from] crate::services::lms::plms::PlmsError),
}
