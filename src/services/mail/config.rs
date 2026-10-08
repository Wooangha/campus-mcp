//! What the mail reader needs, and where it comes from.

use super::DEFAULT_TENANT;
use crate::services::config::{Env, Vars, optional};

/// Which Microsoft 365 tenant the mailbox lives in.
#[derive(Debug, Clone)]
pub struct MailConfig {
    pub tenant: String,
}

impl MailConfig {
    pub fn from_env() -> Self {
        Self::from_vars(&Env)
    }

    pub fn from_vars(vars: &impl Vars) -> Self {
        Self {
            tenant: optional(vars, "MAIL_TENANT").unwrap_or_else(|| DEFAULT_TENANT.to_owned()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::config::testing::vars;

    #[test]
    fn mail_defaults_to_the_postech_tenant() {
        assert_eq!(MailConfig::from_vars(&vars(&[])).tenant, DEFAULT_TENANT);
        assert_eq!(
            MailConfig::from_vars(&vars(&[("MAIL_TENANT", "other.ac.kr")])).tenant,
            "other.ac.kr"
        );
    }
}
