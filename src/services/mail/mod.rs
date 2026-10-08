//! Reading the Outlook mailbox through Microsoft Graph.
//!
//! POSTECH mail is in Microsoft 365, so IMAP is disabled and password auth is
//! gone. This signs in once through the browser (`auth`) and then reads mail
//! over Graph's REST API.

mod auth;
mod config;
mod content;
mod error;

use serde_json::Value;

pub use config::MailConfig;
pub use error::MailError;

/// The Graph endpoint for the signed-in user's messages, newest first.
const MESSAGES: &str = "https://graph.microsoft.com/v1.0/me/messages";
/// The message fields worth pulling; Graph sends far more by default.
const FIELDS: &str = "id,subject,receivedDateTime,from,isRead,bodyPreview,webLink";
/// The Microsoft 365 tenant [`OutlookClient::new`] signs in against.
pub const DEFAULT_TENANT: &str = "postech.ac.kr";

/// One message, trimmed to what a listing needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    pub id: String,
    pub subject: String,
    pub from_name: String,
    pub from_address: String,
    /// ISO-8601 UTC, as Graph returns it.
    pub received: String,
    pub read: bool,
    /// Graph's short plain-text preview of the body.
    pub preview: String,
    /// Opens the message in Outlook on the web.
    pub web_link: String,
}

impl Message {
    /// Who it is from: the display name, or the address when there is none.
    pub fn sender(&self) -> &str {
        if self.from_name.is_empty() {
            &self.from_address
        } else {
            &self.from_name
        }
    }

    /// The preview's first line, cut to `width` characters with an ellipsis.
    pub fn preview_line(&self, width: usize) -> String {
        let line = self.preview.lines().next().unwrap_or("").trim();
        if line.chars().count() > width {
            let cut: String = line.chars().take(width).collect();
            format!("{cut}…")
        } else {
            line.to_owned()
        }
    }
}

/// A signed-in Outlook mailbox. The token, not a password, lives on disk.
pub struct OutlookClient {
    pub(super) http: reqwest::Client,
    tenant: String,
}

impl OutlookClient {
    /// A client for the POSTECH tenant.
    pub fn new() -> Result<Self, MailError> {
        Self::for_tenant(DEFAULT_TENANT)
    }

    /// A client for another institution's Microsoft 365 tenant.
    pub fn for_tenant(tenant: impl Into<String>) -> Result<Self, MailError> {
        Ok(Self {
            http: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(std::time::Duration::from_secs(30))
                .build()?,
            tenant: tenant.into(),
        })
    }

    /// Whether a stored sign-in exists, without making a network call.
    pub fn is_signed_in(&self) -> bool {
        auth::has_stored_login()
    }

    /// The most recent messages, newest first.
    ///
    /// This never opens a browser: it needs a sign-in already on disk, and asks
    /// the caller to run [`OutlookClient::login`] first when there is none.
    pub async fn recent(&self, limit: u32) -> Result<Vec<Message>, MailError> {
        let token = self.access_token().await?;
        let url =
            format!("{MESSAGES}?$top={limit}&$select={FIELDS}&$orderby=receivedDateTime%20desc");
        let response = self
            .http
            .get(&url)
            .header("Prefer", "IdType=\"ImmutableId\"")
            .bearer_auth(token)
            .send()
            .await?;
        messages(&response.text().await?)
    }

    pub(super) fn token_url(&self) -> String {
        format!(
            "https://login.microsoftonline.com/{}/oauth2/v2.0/token",
            self.tenant
        )
    }

    pub(super) fn device_code_url(&self) -> String {
        format!(
            "https://login.microsoftonline.com/{}/oauth2/v2.0/devicecode",
            self.tenant
        )
    }
}

/// Reads a Graph messages response into [`Message`]s.
fn messages(json: &str) -> Result<Vec<Message>, MailError> {
    let response: Value = serde_json::from_str(json)?;
    if let Some(error) = response["error"].as_object() {
        let message = error
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("unknown error");
        return Err(MailError::Service(message.to_owned()));
    }
    let items = response["value"]
        .as_array()
        .ok_or(MailError::InvalidResponse)?;
    Ok(items.iter().map(message).collect())
}

fn message(item: &Value) -> Message {
    let sender = &item["from"]["emailAddress"];
    Message {
        id: string(&item["id"]),
        subject: string(&item["subject"]),
        from_name: string(&sender["name"]),
        from_address: string(&sender["address"]),
        received: string(&item["receivedDateTime"]),
        read: item["isRead"].as_bool().unwrap_or(true),
        preview: string(&item["bodyPreview"]),
        web_link: string(&item["webLink"]),
    }
}

fn string(value: &Value) -> String {
    value.as_str().unwrap_or_default().to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn previewing(preview: &str) -> Message {
        Message {
            id: "test-message".into(),
            subject: String::new(),
            from_name: String::new(),
            from_address: "reg@postech.ac.kr".into(),
            received: String::new(),
            read: false,
            preview: preview.into(),
            web_link: String::new(),
        }
    }

    #[test]
    fn a_preview_line_is_the_first_line_cut_by_characters() {
        assert_eq!(previewing("  첫 줄  \n둘째 줄").preview_line(70), "첫 줄");
        // Counted in characters, not bytes, so Korean is not cut mid-letter.
        assert_eq!(previewing("가나다라마").preview_line(3), "가나다…");
        assert_eq!(previewing("").preview_line(10), "");
    }

    #[test]
    fn the_sender_falls_back_to_the_address() {
        let mut message = previewing("");
        assert_eq!(message.sender(), "reg@postech.ac.kr");
        message.from_name = "학사팀".into();
        assert_eq!(message.sender(), "학사팀");
    }

    /// Shaped like a real Graph `/me/messages` response.
    const RESPONSE: &str = r#"{
        "@odata.context": "https://graph.microsoft.com/v1.0/$metadata#users('me')/messages",
        "value": [
            {
                "subject": "[학사] 2학기 수강신청 안내",
                "receivedDateTime": "2026-09-18T01:02:03Z",
                "isRead": false,
                "bodyPreview": "수강신청 기간은...",
                "webLink": "https://outlook.office365.com/owa/?ItemID=AAA",
                "from": {"emailAddress": {"name": "학사팀", "address": "registrar@postech.ac.kr"}}
            },
            {
                "subject": "Re: project meeting",
                "receivedDateTime": "2026-09-17T09:30:00Z",
                "isRead": true,
                "bodyPreview": "Sounds good.",
                "webLink": "https://outlook.office365.com/owa/?ItemID=BBB",
                "from": {"emailAddress": {"name": "Sunho Cha", "address": "cha@postech.ac.kr"}}
            }
        ]
    }"#;

    #[test]
    fn messages_are_read_with_sender_and_read_state() {
        let found = messages(RESPONSE).unwrap();
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].subject, "[학사] 2학기 수강신청 안내");
        assert_eq!(found[0].from_name, "학사팀");
        assert_eq!(found[0].from_address, "registrar@postech.ac.kr");
        assert!(!found[0].read);
        assert!(found[1].read);
    }

    #[test]
    fn a_graph_error_is_reported_with_its_message() {
        let json = r#"{"error":{"code":"InvalidAuthenticationToken","message":"Access token has expired."}}"#;
        let error = messages(json).unwrap_err();
        assert!(matches!(error, MailError::Service(m) if m == "Access token has expired."));
    }

    #[test]
    fn a_response_without_a_value_array_is_rejected() {
        let error = messages(r#"{"unexpected":true}"#).unwrap_err();
        assert!(matches!(error, MailError::InvalidResponse));
    }

    #[test]
    fn a_message_missing_optional_fields_still_reads() {
        let json = r#"{"value":[{"subject":"no sender block"}]}"#;
        let found = messages(json).unwrap();
        assert_eq!(found[0].from_address, "");
        // Missing isRead defaults to read, so it never nags about phantom unread mail.
        assert!(found[0].read);
    }
}
