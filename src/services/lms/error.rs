//! What can go wrong reading an LMS, in backend-neutral terms.

use thiserror::Error;

use super::plms::PlmsError;

/// A failure reading an LMS, in terms a caller can act on without knowing
/// which backend is behind it.
///
/// The two sign-in outcomes a caller handles differently get their own
/// variants; everything else keeps its backend's own error as the source.
#[derive(Debug, Error)]
pub enum LmsError {
    #[error("로그인이 거부되었습니다 ({0})")]
    AuthenticationFailed(String),
    #[error("2차 인증이 필요한 계정입니다 ({0}); 우회하지 않습니다")]
    SecondFactorRequired(String),
    #[error(transparent)]
    Backend(Box<dyn std::error::Error + Send + Sync>),
}

impl From<PlmsError> for LmsError {
    fn from(error: PlmsError) -> Self {
        match error {
            PlmsError::AuthenticationFailed(code) => Self::AuthenticationFailed(code),
            PlmsError::SecondFactorRequired(code) => Self::SecondFactorRequired(code),
            other => Self::Backend(Box::new(other)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sign_in_outcomes_keep_their_meaning_across_the_boundary() {
        assert!(matches!(
            LmsError::from(PlmsError::SecondFactorRequired("EOTP01".into())),
            LmsError::SecondFactorRequired(code) if code == "EOTP01"
        ));
        assert!(matches!(
            LmsError::from(PlmsError::AuthenticationFailed("SS0002".into())),
            LmsError::AuthenticationFailed(code) if code == "SS0002"
        ));
    }

    #[test]
    fn other_failures_carry_the_backends_own_message() {
        let error = LmsError::from(PlmsError::MissingSesskey);
        assert!(matches!(error, LmsError::Backend(_)));
        assert_eq!(error.to_string(), PlmsError::MissingSesskey.to_string());
    }
}
