//! Facts observed on assignment pages; no LLM judgment or file validation.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Assignment {
    pub url: String,
    pub title: String,
    pub course: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum Status {
    NotSubmitted,
    NoOnlineSubmission,
    Draft,
    Submitted,
    Unknown,
}

impl Status {
    pub fn label(&self) -> &'static str {
        match self {
            Self::NotSubmitted => "미제출",
            Self::NoOnlineSubmission => "온라인 제출 대상 아님",
            Self::Draft => "초안 (최종 제출 전)",
            Self::Submitted => "제출 완료",
            Self::Unknown => "상태 확인 불가",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SubmittedFile {
    pub name: String,
    pub url: String,
}

/// Only stable submission facts belong here: countdowns change on every poll.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Snapshot {
    pub status: Status,
    pub files: Vec<SubmittedFile>,
    pub online_text: bool,
    pub modified: Option<String>,
    /// Assignment due time from PLMS, when an explicit date and time are available.
    #[serde(default)]
    pub due: Option<i64>,
}

#[cfg(test)]
mod tests {
    #[test]
    fn older_snapshots_without_deadlines_still_load() {
        let old = r#"{"status":"Submitted","files":[],"online_text":false,"modified":null}"#;
        let snapshot: super::Snapshot = serde_json::from_str(old).unwrap();
        assert!(snapshot.due.is_none());
        assert_eq!(snapshot.status, super::Status::Submitted);
    }
}
