//! Read-only Outlook access. Login is an explicit CLI operation, never an MCP tool.
//! The existing `$HOME/.config/lms-helper/outlook-token.json` refresh-token
//! cache is intentionally shared with lms-helper; no second login store is made.
use crate::services::{
    content::{Content, ContentCache, ContentError},
    mail::{MailConfig, MailError, Message, OutlookClient},
};
use serde_json::{Value, json};

#[derive(Default)]
pub struct MailBackend {
    client: Option<OutlookClient>,
}
impl MailBackend {
    pub fn new() -> Self {
        Self::default()
    }

    fn client(&mut self) -> Result<&OutlookClient, String> {
        if self.client.is_none() {
            // Mail calls require no LMS configuration and never initiate login.
            let config = MailConfig::from_env();
            self.client = Some(OutlookClient::for_tenant(config.tenant).map_err(safe_error)?);
        }
        Ok(self.client.as_ref().expect("client was initialized"))
    }

    pub async fn list(&mut self, args: &Value) -> Result<Value, String> {
        let (limit, unread_only) = list_args(args)?;
        let messages = self.client()?.recent(limit).await.map_err(safe_error)?;
        Ok(list_result(messages, limit, unread_only))
    }

    pub async fn read_content(
        &mut self,
        id: &str,
        cache: &mut ContentCache,
    ) -> Result<Content, String> {
        validate_id(id)?;
        let mut content = self
            .client()?
            .read_content(id, cache)
            .await
            .map_err(safe_error)?;
        sanitize_partial(&mut content);
        Ok(content)
    }
}

fn sanitize_partial(content: &mut Content) {
    // Per-attachment errors are successful partial results, but upstream
    // diagnostics can contain request URLs. Keep only fixed public wording.
    for warning in &mut content.warnings {
        *warning =
            "메일 첨부 목록을 완전히 확인하지 못했습니다. 일부 자료가 누락될 수 있습니다.".into();
    }
    content.warnings.dedup();
    for attachment in &mut content.attachments {
        if attachment.error.is_some() {
            attachment.error = Some(
                "첨부 파일을 다운로드하지 못했습니다. 지원 형식·용량·접근 권한을 확인하세요."
                    .into(),
            );
        }
    }
}

/// CLI only: device-flow instructions may write to stdout. Do not expose this as
/// an MCP method or run it while stdio is being used for the MCP transport.
pub async fn login() -> Result<(), String> {
    let config = MailConfig::from_env();
    OutlookClient::for_tenant(config.tenant)
        .map_err(safe_error)?
        .login()
        .await
        .map_err(safe_error)
}

fn list_args(args: &Value) -> Result<(u32, bool), String> {
    let args = args
        .as_object()
        .ok_or("메일 목록 인자는 객체여야 합니다.")?;
    let limit = match args.get("limit") {
        None => 20,
        Some(value) => value
            .as_u64()
            .filter(|n| (1..=100).contains(n))
            .ok_or("limit은 1~100 사이의 정수여야 합니다.")? as u32,
    };
    let unread_only = match args.get("unread_only") {
        None => false,
        Some(value) => value
            .as_bool()
            .ok_or("unread_only는 true 또는 false여야 합니다.")?,
    };
    Ok((limit, unread_only))
}
fn validate_id(id: &str) -> Result<(), String> {
    if matches!(id, "." | "..")
        || id.is_empty()
        || id.len() > 2048
        || id.chars().any(char::is_whitespace)
        || id.chars().any(char::is_control)
    {
        return Err("메일 ID는 공백·제어문자 없이 1~2048바이트여야 합니다.".into());
    }
    Ok(())
}
fn list_result(messages: Vec<Message>, limit: u32, unread_only: bool) -> Value {
    let fetched_count = messages.len();
    let messages: Vec<_> = messages
        .into_iter()
        .filter(|m| !unread_only || !m.read)
        .map(|m| {
            json!({
                "id":m.id,"subject":m.subject,"from_name":m.from_name,"from_address":m.from_address,
                "received":m.received,"is_read":m.read,"preview":m.preview,"web_link":m.web_link,
            })
        })
        .collect();
    json!({
        "messages":messages,"requested_limit":limit,"fetched_count":fetched_count,
        "returned_count":messages.len(),"unread_only":unread_only,
        "scope":"recent_mailbox_window","pagination":"single_page","partial":true,
        "note":"메일함의 최근 메일 첫 페이지만 조회했습니다. 받은편지함 전용 조회가 아니며 보낸편지·임시보관·삭제 항목이 포함될 수 있습니다. unread_only는 이 최근 조회 범위 안에서만 적용되므로 전체 안 읽은 메일 검색 결과가 아닙니다.",
    })
}
fn safe_error(error: MailError) -> String {
    match error {
        MailError::Auth(_) | MailError::Content(ContentError::Authentication)
        | MailError::Content(ContentError::HttpStatus(401)) => "메일 인증이 필요합니다. 터미널에서 `campus-mcp mail-login`을 실행한 뒤 다시 시도하세요.".into(),
        MailError::Content(ContentError::TooLarge) => "메일 자료가 허용된 다운로드 크기를 초과했습니다.".into(),
        MailError::Content(ContentError::HttpStatus(429)) => "메일 서비스 요청 제한에 도달했습니다. 잠시 후 다시 시도하세요.".into(),
        MailError::InvalidResponse | MailError::Json(_) => "메일 서비스 응답을 해석하지 못했습니다. 잠시 후 다시 시도하세요.".into(),
        MailError::Io(_) => "메일 인증 또는 캐시 파일을 읽거나 저장하지 못했습니다. 로컬 파일 접근 권한을 확인하세요.".into(),
        _ => "메일 서비스 요청에 실패했습니다. 연결 상태와 메일 접근 권한을 확인하세요.".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn message(id: &str, read: bool) -> Message {
        Message {
            id: id.into(),
            subject: "Fixture subject".into(),
            from_name: "Fixture sender".into(),
            from_address: "sender@example.test".into(),
            received: "2026-10-03T01:00:00Z".into(),
            read,
            preview: "Fixture preview".into(),
            web_link: "https://outlook.example.test/message".into(),
        }
    }
    #[test]
    fn args_have_bounded_defaults_and_do_not_coerce_invalid_types() {
        assert_eq!(list_args(&json!({})).unwrap(), (20, false));
        assert_eq!(
            list_args(&json!({"limit":100,"unread_only":true})).unwrap(),
            (100, true)
        );
        for value in [
            json!(0),
            json!(101),
            json!(-1),
            json!(1.5),
            json!("20"),
            Value::Null,
        ] {
            assert!(list_args(&json!({"limit":value})).is_err());
        }
        assert!(list_args(&json!({"unread_only":"true"})).is_err());
        assert!(list_args(&Value::Null).is_err());
    }
    #[test]
    fn ids_are_validated_before_client_or_auth_initialization() {
        assert!(validate_id("AAMk/a+b==").is_ok());
        assert!(validate_id(&"a".repeat(2048)).is_ok());
        for id in ["", ".", "..", " ", "a\nb", "a\0b"] {
            assert!(validate_id(id).is_err());
        }
        assert!(validate_id(&"a".repeat(2049)).is_err());
    }
    #[tokio::test]
    async fn invalid_list_does_not_initialize_client() {
        let mut backend = MailBackend::new();
        assert!(backend.list(&json!({"limit":0})).await.is_err());
        assert!(backend.client.is_none());
    }
    #[test]
    fn unread_filter_is_explicitly_only_a_recent_window() {
        let result = list_result(
            vec![message("read", true), message("unread", false)],
            20,
            true,
        );
        assert_eq!(result["fetched_count"], 2);
        assert_eq!(result["returned_count"], 1);
        assert_eq!(result["messages"][0]["id"], "unread");
        assert_eq!(result["messages"][0]["is_read"], false);
        assert_eq!(result["partial"], true);
        assert_eq!(result["scope"], "recent_mailbox_window");
        assert!(
            result["note"]
                .as_str()
                .unwrap()
                .contains("전체 안 읽은 메일 검색 결과가 아닙니다")
        );
        assert_eq!(
            list_result(vec![message("read", true)], 20, false)["returned_count"],
            1
        );
    }
    #[test]
    fn partial_results_keep_source_data_without_raw_diagnostics() {
        let secret = "https://example.test/?access_token=fixture-secret";
        let mut file = crate::services::content::Attachment::remote(
            "fixture.pdf".into(),
            "https://graph.microsoft.com/file".into(),
            false,
        );
        file.error = Some(secret.into());
        let mut content = Content {
            title: "Source title".into(),
            body: "Source body".into(),
            attachments: vec![file],
            warnings: vec![secret.into(), secret.into()],
        };
        sanitize_partial(&mut content);
        assert_eq!(content.body, "Source body");
        assert_eq!(content.warnings.len(), 1);
        assert!(!content.warnings[0].contains("fixture-secret"));
        assert!(
            !content.attachments[0]
                .error
                .as_ref()
                .unwrap()
                .contains("fixture-secret")
        );
    }

    #[test]
    fn upstream_diagnostics_never_escape_error_boundary() {
        let secret = "https://example.test/?access_token=fixture-secret";
        for error in [
            MailError::Auth(secret.into()),
            MailError::Service(secret.into()),
            MailError::Io(std::io::Error::other(secret)),
        ] {
            let public = safe_error(error);
            assert!(!public.contains("fixture-secret"));
            assert!(!public.contains("https://"));
        }
        assert!(safe_error(MailError::Auth("hidden".into())).contains("campus-mcp mail-login"));
    }
}
