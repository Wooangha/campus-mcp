//! The clock, in the one form the crate uses it.

use std::time::{SystemTime, UNIX_EPOCH};

/// The current time in Unix seconds.
pub fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_secs() as i64)
        .unwrap_or_default()
}
