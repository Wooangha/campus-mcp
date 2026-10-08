//! What signing in to PLMS needs, and where it comes from.

use super::PlmsClient;
use crate::services::config::{ConfigError, Vars, required};
use crate::services::lms::LmsError;

/// The POSTECH SSO account PLMS is read with.
#[derive(Debug, Clone)]
pub struct PlmsConfig {
    pub username: String,
    pub password: String,
}

impl PlmsConfig {
    pub fn from_vars(vars: &impl Vars) -> Result<Self, ConfigError> {
        Ok(Self {
            username: required(vars, "PLMS_USERNAME")?,
            password: required(vars, "PLMS_PASSWORD")?,
        })
    }
}

impl PlmsClient {
    /// Signs in through POSTECH SSO. The credentials are used and dropped; only
    /// the session stays, in memory.
    pub async fn connect(config: &PlmsConfig) -> Result<Self, LmsError> {
        let mut client = Self::new()?;
        client.login(&config.username, &config.password).await?;
        Ok(client)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::config::testing::vars;

    #[test]
    fn plms_needs_both_credentials() {
        assert_eq!(
            PlmsConfig::from_vars(&vars(&[("PLMS_USERNAME", "student")])).unwrap_err(),
            ConfigError::Missing("PLMS_PASSWORD")
        );
        // Blank is as good as unset: an empty password line in .env is a mistake.
        assert_eq!(
            PlmsConfig::from_vars(&vars(&[
                ("PLMS_USERNAME", "student"),
                ("PLMS_PASSWORD", " ")
            ]))
            .unwrap_err(),
            ConfigError::Missing("PLMS_PASSWORD")
        );
    }
}
