//! Environment settings shared by the PLMS and Outlook readers.

use std::env;

use thiserror::Error;

/// A variable that is missing or does not parse.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum ConfigError {
    #[error("{0}가 필요합니다 (.env에 설정)")]
    Missing(&'static str),
    #[error("{var}: {reason}")]
    Invalid { var: &'static str, reason: String },
}

/// The environment, abstracted so sections can be tested against a map.
pub trait Vars {
    fn get(&self, name: &str) -> Option<String>;
}

/// The process environment.
pub struct Env;

impl Vars for Env {
    fn get(&self, name: &str) -> Option<String> {
        env::var(name).ok()
    }
}

/// A required variable, or [`ConfigError::Missing`]. Blank counts as unset: an
/// empty `KEY=` line in `.env` is a mistake, not a value.
pub fn required(vars: &impl Vars, name: &'static str) -> Result<String, ConfigError> {
    optional(vars, name).ok_or(ConfigError::Missing(name))
}

/// An optional variable; blank counts as unset.
pub fn optional(vars: &impl Vars, name: &str) -> Option<String> {
    vars.get(name).filter(|value| !value.trim().is_empty())
}

/// A map standing in for the environment, for the tests of every section.
#[cfg(test)]
pub mod testing {
    use std::collections::HashMap;

    use super::Vars;

    pub struct Map(pub HashMap<&'static str, &'static str>);

    impl Vars for Map {
        fn get(&self, name: &str) -> Option<String> {
            self.0.get(name).map(|value| value.to_string())
        }
    }

    pub fn vars(pairs: &[(&'static str, &'static str)]) -> Map {
        Map(pairs.iter().copied().collect())
    }

    /// Minimal LMS configuration fixture.
    pub fn full() -> Map {
        vars(&[
            ("HOME", "/tmp/campus-mcp-tests"),
            ("PLMS_USERNAME", "student"),
            ("PLMS_PASSWORD", "secret"),
        ])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn errors_read_as_advice() {
        assert_eq!(
            ConfigError::Missing("PLMS_USERNAME").to_string(),
            "PLMS_USERNAME가 필요합니다 (.env에 설정)"
        );
    }
}
