//! Which LMS to read.

use super::plms::PlmsConfig;
use crate::services::config::{ConfigError, Env, Vars, optional};

/// The LMS to read (`LMS_SERVICE`), with that backend's own settings.
#[derive(Debug, Clone)]
pub enum LmsConfig {
    Plms(PlmsConfig),
}

impl LmsConfig {
    pub fn from_env() -> Result<Self, ConfigError> {
        Self::from_vars(&Env)
    }

    pub fn from_vars(vars: &impl Vars) -> Result<Self, ConfigError> {
        let name = optional(vars, "LMS_SERVICE").unwrap_or_else(|| "plms".to_owned());
        match name.to_lowercase().as_str() {
            "plms" => Ok(Self::Plms(PlmsConfig::from_vars(vars)?)),
            other => Err(ConfigError::Invalid {
                var: "LMS_SERVICE",
                reason: format!("알 수 없는 LMS {other} (plms)"),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::config::testing::full;

    #[test]
    fn the_lms_defaults_to_plms() {
        let LmsConfig::Plms(plms) = LmsConfig::from_vars(&full()).unwrap();
        assert_eq!(plms.username, "student");
    }

    #[test]
    fn an_unknown_lms_is_named_in_the_error() {
        let mut map = full();
        map.0.insert("LMS_SERVICE", "canvas");
        assert!(matches!(
            LmsConfig::from_vars(&map).unwrap_err(),
            ConfigError::Invalid { var: "LMS_SERVICE", ref reason } if reason.contains("canvas")
        ));
    }

    /// The backend's own settings are checked as part of choosing it.
    #[test]
    fn the_backends_own_gaps_surface() {
        let mut map = full();
        map.0.remove("PLMS_PASSWORD");
        assert_eq!(
            LmsConfig::from_vars(&map).unwrap_err(),
            ConfigError::Missing("PLMS_PASSWORD")
        );
    }
}
