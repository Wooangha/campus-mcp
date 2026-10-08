//! Moodle's AJAX services: how one is called, and the envelope it answers in.

use serde_json::Value;

use crate::services::lms::plms::{PlmsError, http::PLMS, page::moodle_sesskey, sso::PlmsClient};

/// Moodle's AJAX entry point; every service call goes through it.
const SERVICE: &str = "/lib/ajax/service.php";
/// The dashboard, which carries the sesskey every service call must quote.
const DASHBOARD: &str = "/my/";

impl PlmsClient {
    /// One AJAX service call, answered as raw JSON for [`service_data`].
    pub(super) async fn service(&self, method: &str, args: Value) -> Result<String, PlmsError> {
        let session = self.session()?;
        let dashboard = session.get(&format!("{PLMS}{DASHBOARD}"), None).await?;
        let dashboard = session.follow(dashboard).await?;
        let sesskey = moodle_sesskey(&dashboard.body).ok_or(PlmsError::MissingSesskey)?;
        let call = serde_json::json!([{ "index": 0, "methodname": method, "args": args }]);
        Ok(session
            .post_json(
                &format!("{PLMS}{SERVICE}?sesskey={sesskey}&info={method}"),
                call.to_string(),
                Some(dashboard.url.as_str()),
            )
            .await?
            .body)
    }
}

/// Unwraps the envelope every AJAX service answers with.
pub(super) fn service_data(json: &str) -> Result<Value, PlmsError> {
    let response: Value = serde_json::from_str(json)?;
    let call = response
        .as_array()
        .and_then(|calls| calls.first())
        .ok_or(PlmsError::InvalidMoodleResponse)?;
    if call["error"] != false {
        let message = call["exception"]["message"]
            .as_str()
            .unwrap_or("unknown error");
        return Err(PlmsError::MoodleService(message.to_owned()));
    }
    Ok(call["data"].clone())
}

/// A JSON string field, or empty when it is absent or not a string.
pub(super) fn string(value: &Value) -> String {
    value.as_str().unwrap_or_default().to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_data_is_unwrapped_from_the_envelope() {
        let data = service_data(r#"[{"error":false,"data":{"answer":42}}]"#).unwrap();
        assert_eq!(data["answer"], 42);
    }

    #[test]
    fn a_service_error_is_reported_with_its_message() {
        let json = r#"[{"error":true,"exception":{"message":"Invalid session key"}}]"#;
        assert!(matches!(
            service_data(json).unwrap_err(),
            PlmsError::MoodleService(message) if message == "Invalid session key"
        ));
    }

    #[test]
    fn anything_but_a_list_of_calls_is_rejected() {
        assert!(matches!(
            service_data("{}").unwrap_err(),
            PlmsError::InvalidMoodleResponse
        ));
        assert!(matches!(
            service_data("[]").unwrap_err(),
            PlmsError::InvalidMoodleResponse
        ));
    }

    #[test]
    fn a_missing_string_reads_as_empty() {
        let value = serde_json::json!({ "name": "x", "count": 3 });
        assert_eq!(string(&value["name"]), "x");
        assert_eq!(string(&value["count"]), "");
        assert_eq!(string(&value["absent"]), "");
    }
}
