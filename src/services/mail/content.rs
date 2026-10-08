//! Message bodies and Graph attachments using one token per collection.
use super::{MESSAGES, MailError, Message, OutlookClient, messages, string};
use crate::services::content::{
    Attachment, Content, ContentCache, ContentError, html_text, read_limited,
};
use chrono::{DateTime, Duration, SecondsFormat, Utc};
use serde_json::Value;
use std::{collections::HashSet, future::Future};
use url::Url;

const INBOX_MESSAGES: &str = "https://graph.microsoft.com/v1.0/me/mailFolders/inbox/messages";
const INBOX_PATH: &str = "/v1.0/me/mailFolders/inbox/messages";

impl OutlookClient {
    pub async fn read_content(
        &self,
        id: &str,
        cache: &mut ContentCache,
    ) -> Result<Content, MailError> {
        let token = self.access_token().await?;
        self.content_with_token(id, &token, cache).await
    }

    pub async fn recent_unread_content(
        &self,
        limit: u32,
        cache: &mut ContentCache,
    ) -> Result<Vec<(Message, Result<Content, MailError>)>, MailError> {
        let token = self.access_token().await?;
        let mut url = Url::parse(MESSAGES).unwrap();
        url.query_pairs_mut()
            .append_pair("$top", &limit.to_string())
            .append_pair("$select", super::FIELDS)
            .append_pair("$orderby", "receivedDateTime desc");
        let json = self.graph_json(&url, &token).await?;
        let mut out = Vec::new();
        for message in messages(&json.to_string())?.into_iter().filter(|m| !m.read) {
            let content = self.content_with_token(&message.id, &token, cache).await;
            out.push((message, content));
        }
        Ok(out)
    }

    /// Collect recent inbox messages regardless of their read state. Listing failures after
    /// a successful page preserve that page and return a visible warning.
    pub async fn recent_content_since(
        &self,
        days: u32,
        max_items: u32,
        cache: &mut ContentCache,
    ) -> Result<(Vec<(Message, Result<Content, MailError>)>, Vec<String>), MailError> {
        let url = recent_url(days, max_items, Utc::now())?;
        let token = self.access_token().await?;
        let (messages, warnings) = collect_pages(url, max_items as usize, |url| {
            let token = &token;
            async move { self.graph_json(&url, token).await }
        })
        .await?;
        let mut out = Vec::with_capacity(messages.len());
        for message in messages {
            let content = self.content_with_token(&message.id, &token, cache).await;
            out.push((message, content));
        }
        Ok((out, warnings))
    }

    async fn content_with_token(
        &self,
        id: &str,
        token: &str,
        cache: &mut ContentCache,
    ) -> Result<Content, MailError> {
        let mut url = message_url(id)?;
        url.query_pairs_mut().append_pair("$select", "subject,body");
        let value = self.graph_json(&url, token).await?;
        let mut content = parse_body(&value)?;
        let mut url = message_url(id)?;
        url.path_segments_mut().unwrap().push("attachments");
        url.query_pairs_mut()
            .append_pair("$select", "id,name,isInline,size");
        let attachment_path = url.path().to_owned();
        let mut next = Some(url);
        let mut visited = HashSet::new();
        let mut attachment_ids = HashSet::new();
        for _ in 0..10 {
            let Some(url) = next.take() else { break };
            if !begin_attachment_page(&url, &mut visited, &mut content.warnings) {
                break;
            }
            let page = match self.graph_json(&url, token).await {
                Ok(page) => page,
                Err(error) => {
                    content
                        .warnings
                        .push(format!("첨부 목록 조회 실패: {error}"));
                    break;
                }
            };
            let Some(parsed) = attachment_page(&page, &attachment_path, &mut attachment_ids) else {
                content.warnings.push("첨부 목록 응답 형식 오류".into());
                break;
            };
            content.warnings.extend(parsed.warnings);
            next = parsed.next;
            for item in parsed.items {
                // attachment_page only retains entries with a nonempty ID.
                let attachment_id = item["id"].as_str().unwrap();
                let mut url = message_url(id)?;
                url.path_segments_mut()
                    .unwrap()
                    .push("attachments")
                    .push(attachment_id)
                    .push("$value");
                let mut file = Attachment::remote(
                    string(&item["name"]),
                    url.to_string(),
                    item["isInline"].as_bool().unwrap_or(false),
                );
                if item["@odata.type"].as_str() != Some("#microsoft.graph.fileAttachment") {
                    file.error = Some("일반 파일 첨부가 아니어서 다운로드하지 않았습니다".into());
                } else if item["size"]
                    .as_u64()
                    .is_some_and(|n| n > cache.limit() as u64)
                    || cache.limit() == 0
                {
                    file.error = Some(ContentError::TooLarge.to_string());
                } else {
                    match self.graph_download(&url, token, cache.limit()).await {
                        Ok(bytes) => match cache.save(&bytes, &file.name) {
                            Ok(path) => file.local_path = Some(path),
                            Err(e) => file.error = Some(e.to_string()),
                        },
                        Err(e) => file.error = Some(e.to_string()),
                    }
                }
                content.attachments.push(file);
            }
        }
        if next.is_some() {
            content
                .warnings
                .push("첨부 목록은 최대 10페이지까지 조회했습니다".into());
        }
        Ok(content)
    }

    async fn graph_json(&self, url: &Url, token: &str) -> Result<Value, MailError> {
        let bytes = self.graph_download(url, token, 8 * 1024 * 1024).await?;
        let value: Value = serde_json::from_slice(&bytes)?;
        if let Some(message) = value["error"]["message"].as_str() {
            return Err(MailError::Service(message.to_owned()));
        }
        Ok(value)
    }
    async fn graph_download(
        &self,
        url: &Url,
        token: &str,
        max: usize,
    ) -> Result<Vec<u8>, MailError> {
        validate_graph_url(url)?;
        let response = self
            .http
            .get(url.clone())
            .bearer_auth(token)
            .header("Prefer", "IdType=\"ImmutableId\"")
            .send()
            .await?;
        Ok(read_limited(response, max).await?)
    }
}
struct AttachmentPage<'a> {
    items: Vec<&'a Value>,
    next: Option<Url>,
    warnings: Vec<String>,
}

fn begin_attachment_page(
    url: &Url,
    visited: &mut HashSet<String>,
    warnings: &mut Vec<String>,
) -> bool {
    if visited.insert(url.as_str().to_owned()) {
        true
    } else {
        warnings.push("첨부 목록 페이지가 반복되어 조회를 중단했습니다: 목록 확인 불완전".into());
        false
    }
}

fn attachment_page<'a>(
    page: &'a Value,
    expected_path: &str,
    ids: &mut HashSet<String>,
) -> Option<AttachmentPage<'a>> {
    let mut warnings = Vec::new();
    let items = page["value"]
        .as_array()?
        .iter()
        .filter(|item| {
            let valid = item["id"].as_str().is_some_and(|id| !id.trim().is_empty());
            if !valid {
                warnings.push("식별자가 없는 첨부를 제외했습니다: 목록 확인 불완전".into());
            }
            if valid && !ids.insert(item["id"].as_str().unwrap().to_owned()) {
                warnings.push("중복 첨부 식별자를 제외했습니다: 목록 확인 불완전".into());
                return false;
            }
            valid
        })
        .collect();
    let next = match page.get("@odata.nextLink") {
        None | Some(Value::Null) => None,
        Some(value) => match value.as_str().and_then(|s| Url::parse(s).ok()) {
            Some(url) if validate_graph_url(&url).is_ok() && url.path() == expected_path => {
                Some(url)
            }
            _ => {
                warnings.push("첨부 목록 다음 페이지 주소 오류: 목록 확인 불완전".into());
                None
            }
        },
    };
    warnings.sort();
    warnings.dedup();
    Some(AttachmentPage {
        items,
        next,
        warnings,
    })
}

fn recent_url(days: u32, max_items: u32, now: DateTime<Utc>) -> Result<Url, MailError> {
    if days == 0 || max_items == 0 {
        return Err(MailError::InvalidResponse);
    }
    let since = now
        .checked_sub_signed(Duration::days(i64::from(days)))
        .ok_or(MailError::InvalidResponse)?
        .to_rfc3339_opts(SecondsFormat::Secs, true);
    let mut url = Url::parse(INBOX_MESSAGES).unwrap();
    url.query_pairs_mut()
        .append_pair("$top", &max_items.min(100).to_string())
        .append_pair("$select", super::FIELDS)
        .append_pair("$filter", &format!("receivedDateTime ge {since}"))
        .append_pair("$orderby", "receivedDateTime desc");
    Ok(url)
}

async fn collect_pages<F, Fut>(
    first: Url,
    max_items: usize,
    mut fetch: F,
) -> Result<(Vec<Message>, Vec<String>), MailError>
where
    F: FnMut(Url) -> Fut,
    Fut: Future<Output = Result<Value, MailError>>,
{
    let mut next = Some(first);
    let mut visited = HashSet::new();
    let mut ids = HashSet::new();
    let mut out = Vec::new();
    let mut warnings = Vec::new();
    for page_number in 0..100 {
        let Some(url) = next.take() else { break };
        if !visited.insert(url.to_string()) {
            warnings.push("메일 목록 다음 페이지가 반복되어 조회를 중단했습니다".into());
            break;
        }
        let page = fetch(url).await.and_then(|page| {
            let messages = messages(&page.to_string())?;
            Ok((page, messages))
        });
        let (page, messages) = match page {
            Ok(page) => page,
            Err(error) if page_number == 0 => return Err(error),
            Err(_) => {
                warnings.push(
                    "메일 목록 일부 페이지 조회에 실패했습니다. 조회된 메일만 표시합니다".into(),
                );
                break;
            }
        };
        let mut omitted = false;
        for message in messages {
            if message.id.trim().is_empty() {
                warnings.push("식별자가 없는 메일을 제외했습니다".into());
                continue;
            }
            if !ids.insert(message.id.clone()) {
                continue;
            }
            if out.len() == max_items {
                omitted = true;
                break;
            }
            out.push(message);
        }
        next = match page.get("@odata.nextLink") {
            None | Some(Value::Null) => None,
            Some(value) => match value.as_str().and_then(|link| Url::parse(link).ok()) {
                Some(url)
                    if validate_graph_url(&url).is_ok()
                        && url.path().eq_ignore_ascii_case(INBOX_PATH) =>
                {
                    Some(url)
                }
                _ => {
                    warnings.push(
                        "메일 목록 다음 페이지 주소가 유효하지 않아 조회를 중단했습니다".into(),
                    );
                    None
                }
            },
        };
        if omitted || (out.len() == max_items && next.is_some()) {
            warnings.push(format!(
                "메일 수집 상한 {max_items}개에 도달했습니다. 더 오래된 메일은 생략될 수 있습니다"
            ));
            next = None;
            break;
        }
    }
    if next.is_some() {
        warnings.push("메일 목록 조회는 최대 100페이지까지 지원합니다".into());
    }
    warnings.sort();
    warnings.dedup();
    Ok((out, warnings))
}

fn parse_body(value: &Value) -> Result<Content, MailError> {
    let body = value["body"]["content"]
        .as_str()
        .ok_or(MailError::InvalidResponse)?;
    let text = if value["body"]["contentType"]
        .as_str()
        .is_some_and(|v| v.eq_ignore_ascii_case("html"))
    {
        html_text(body)
    } else {
        body.to_owned()
    };
    Ok(Content {
        title: string(&value["subject"]),
        body: text,
        ..Content::default()
    })
}

fn message_url(id: &str) -> Result<Url, MailError> {
    if id.is_empty() {
        return Err(MailError::InvalidResponse);
    }
    let mut url = Url::parse(MESSAGES).unwrap();
    url.path_segments_mut().unwrap().push(id);
    Ok(url)
}
fn validate_graph_url(url: &Url) -> Result<(), ContentError> {
    if url.origin().ascii_serialization() != "https://graph.microsoft.com"
        || !(url.path() == "/v1.0/me/messages"
            || url.path().starts_with("/v1.0/me/messages/")
            || url.path().eq_ignore_ascii_case(INBOX_PATH))
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(ContentError::InvalidUrl);
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    async fn pages(
        responses: Vec<Result<Value, MailError>>,
        max: usize,
    ) -> (Vec<Message>, Vec<String>) {
        let mut responses = responses.into_iter();
        collect_pages(Url::parse(INBOX_MESSAGES).unwrap(), max, |_| {
            std::future::ready(responses.next().expect("unexpected page fetch"))
        })
        .await
        .unwrap()
    }

    #[test]
    fn malformed_attachment_pages_keep_good_entries_but_never_claim_completeness() {
        let path = "/v1.0/me/messages/a/attachments";
        for next in [
            serde_json::json!(12),
            serde_json::json!(true),
            serde_json::json!("bad-url"),
            serde_json::json!("https://graph.microsoft.com/v1.0/me/messages/b/attachments"),
            serde_json::json!("https://other.example/v1.0/me/messages/a/attachments"),
        ] {
            let page = serde_json::json!({"value":[{"id":"known","name":"file.pdf"}],"@odata.nextLink":next});
            let parsed = attachment_page(&page, path, &mut HashSet::new()).unwrap();
            assert_eq!(parsed.items.len(), 1);
            assert!(parsed.next.is_none());
            assert!(
                !parsed.warnings.is_empty(),
                "invalid continuation cannot prove the old attachments were removed"
            );
        }
        let page = serde_json::json!({"value":[{"id":"known"},{"name":"missing.pdf"},{"id":""},{"id":"  "}],"@odata.nextLink":null});
        let parsed = attachment_page(&page, path, &mut HashSet::new()).unwrap();
        assert_eq!(parsed.items.len(), 1);
        assert_eq!(parsed.warnings.len(), 1);
        assert!(parsed.next.is_none());
    }

    #[test]
    fn repeated_attachment_pages_and_ids_do_not_repeat_download_candidates() {
        let path = "/v1.0/me/messages/a/attachments";
        let url = Url::parse(&format!(
            "https://graph.microsoft.com{path}?$skiptoken=same"
        ))
        .unwrap();
        let mut visited = HashSet::new();
        let mut warnings = Vec::new();
        assert!(begin_attachment_page(&url, &mut visited, &mut warnings));
        assert!(!begin_attachment_page(&url, &mut visited, &mut warnings));
        assert_eq!(warnings.len(), 1);
        let mut ids = HashSet::new();
        let first = serde_json::json!({"value":[{"id":"one"},{"id":"one"}]});
        let parsed = attachment_page(&first, path, &mut ids).unwrap();
        assert_eq!(parsed.items.len(), 1);
        assert!(!parsed.warnings.is_empty());
        let second = serde_json::json!({"value":[{"id":"one"},{"id":"two"}]});
        let parsed = attachment_page(&second, path, &mut ids).unwrap();
        assert_eq!(parsed.items.len(), 1);
        assert_eq!(parsed.items[0]["id"], "two");
        assert!(!parsed.warnings.is_empty());
    }

    #[test]
    fn valid_attachment_continuation_is_followed_without_warning() {
        let path = "/v1.0/me/messages/a/attachments";
        let page = serde_json::json!({"value":[{"id":"file"}],"@odata.nextLink":format!("https://graph.microsoft.com{path}?$skiptoken=opaque")});
        let parsed = attachment_page(&page, path, &mut HashSet::new()).unwrap();
        assert_eq!(parsed.next.unwrap().query(), Some("$skiptoken=opaque"));
        assert!(parsed.warnings.is_empty());
        assert!(
            attachment_page(
                &serde_json::json!({"value":null}),
                path,
                &mut HashSet::new()
            )
            .is_none()
        );
    }

    #[test]
    fn date_filter_and_order_use_same_property_without_unread_filter() {
        let now = DateTime::parse_from_rfc3339("2026-09-26T01:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let url = recent_url(30, 300, now).unwrap();
        assert_eq!(url.path(), INBOX_PATH);
        let query: std::collections::HashMap<_, _> = url.query_pairs().collect();
        assert_eq!(query["$filter"], "receivedDateTime ge 2026-08-27T01:00:00Z");
        assert_eq!(query["$orderby"], "receivedDateTime desc");
        assert_eq!(query["$top"], "100");
        assert!(recent_url(0, 300, now).is_err());
    }

    #[tokio::test]
    async fn follows_empty_pages_and_preserves_read_messages_without_duplicates() {
        let (items, warnings) = pages(vec![
            Ok(serde_json::json!({"value": [], "@odata.nextLink": format!("{INBOX_MESSAGES}?$skip=1")})),
            Ok(serde_json::json!({"value": [{"id":"a","isRead":true}], "@odata.nextLink": format!("{INBOX_MESSAGES}?$skip=2")})),
            Ok(serde_json::json!({"value": [{"id":"a"},{"id":"b","isRead":false},{"id":""}]})),
        ], 10).await;
        assert_eq!(
            items.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
            ["a", "b"]
        );
        assert!(items[0].read);
        assert!(!items[1].read);
        assert_eq!(warnings.len(), 1);
    }

    #[tokio::test]
    async fn pagination_failure_keeps_successful_pages_and_warns() {
        for failure in [
            Err(MailError::InvalidResponse),
            Ok(serde_json::json!({"missing":"value"})),
        ] {
            let (items, warnings) = pages(vec![
                Ok(serde_json::json!({"value":[{"id":"a"}],"@odata.nextLink":format!("{INBOX_MESSAGES}?$skip=1")})), failure
            ], 10).await;
            assert_eq!(items.len(), 1);
            assert_eq!(warnings.len(), 1);
        }
        assert!(
            collect_pages(Url::parse(INBOX_MESSAGES).unwrap(), 10, |_| {
                std::future::ready(Err(MailError::InvalidResponse))
            })
            .await
            .is_err()
        );
    }

    #[tokio::test]
    async fn invalid_and_repeating_next_links_are_never_fetched() {
        for link in [
            "https://evil.test/v1.0/me/messages",
            "https://graph.microsoft.com/v1.0/me/messagesAnything",
            "https://graph.microsoft.com/v1.0/me/messages/a/attachments",
            "https://user@graph.microsoft.com/v1.0/me/messages",
            "not a url",
            MESSAGES,
            INBOX_MESSAGES,
            "https://graph.microsoft.com/v1.0/me/mailFolders/sentitems/messages",
            "https://graph.microsoft.com/v1.0/me/mailFolders/inbox/messagesExtra",
        ] {
            let (items, warnings) = pages(
                vec![Ok(serde_json::json!({
                    "value":[{"id":"a"}],"@odata.nextLink":link
                }))],
                10,
            )
            .await;
            assert_eq!(items.len(), 1);
            assert_eq!(warnings.len(), 1);
        }
    }

    #[tokio::test]
    async fn inbox_pagination_accepts_canonical_path_case() {
        let (items, warnings) = pages(vec![
            Ok(serde_json::json!({"value": [{"id":"a"}], "@odata.nextLink": "https://graph.microsoft.com/v1.0/me/mailfolders/Inbox/messages?$skip=1"})),
            Ok(serde_json::json!({"value": [{"id":"b"}]})),
        ], 10).await;
        assert_eq!(items.len(), 2);
        assert!(warnings.is_empty());
        assert!(validate_graph_url(&Url::parse(INBOX_MESSAGES).unwrap()).is_ok());
        for url in [
            "https://graph.microsoft.com/v1.0/me/mailFolders/sentitems/messages",
            "https://graph.microsoft.com/v1.0/me/mailFolders/inbox/messages/a",
            "https://evil.test/v1.0/me/mailFolders/inbox/messages",
        ] {
            assert!(validate_graph_url(&Url::parse(url).unwrap()).is_err());
        }
    }

    #[tokio::test]
    async fn item_limit_reports_omission_but_exact_complete_page_does_not() {
        for (page, warning_count) in [
            (serde_json::json!({"value":[{"id":"a"},{"id":"b"}]}), 1),
            (
                serde_json::json!({"value":[{"id":"a"}], "@odata.nextLink":format!("{INBOX_MESSAGES}?$skip=1")}),
                1,
            ),
            (serde_json::json!({"value":[{"id":"a"}]}), 0),
        ] {
            let (items, warnings) = pages(vec![Ok(page)], 1).await;
            assert_eq!(items.len(), 1);
            assert_eq!(warnings.len(), warning_count);
        }
    }

    #[test]
    fn graph_html_and_text_bodies_keep_details_beyond_the_preview() {
        let html = serde_json::json!({"subject":"Notice", "body":{"contentType":"HTML","content":"<p>Full details &amp; links</p><div>Second paragraph</div>"}});
        assert_eq!(
            parse_body(&html).unwrap().body,
            "Full details & links\nSecond paragraph"
        );
        let text = serde_json::json!({"body":{"contentType":"text","content":"a < b"}});
        assert_eq!(parse_body(&text).unwrap().body, "a < b");
        assert!(parse_body(&serde_json::json!({"bodyPreview":"preview only"})).is_err());
    }

    #[test]
    fn ids_are_path_segments_and_pagination_cannot_send_tokens_elsewhere() {
        let url = message_url("a/b?token").unwrap();
        assert!(url.as_str().contains("a%2Fb%3Ftoken"));
        assert!(
            validate_graph_url(&Url::parse("https://evil.test/v1.0/me/messages").unwrap()).is_err()
        );
        assert!(
            validate_graph_url(&Url::parse("http://graph.microsoft.com/v1.0/me/messages").unwrap())
                .is_err()
        );
        assert!(message_url("").is_err());
    }
}
