use lms_helper::{
    content::{self, Content, ContentCache},
    lms::{Lms, LmsConfig, Scope, plms::PlmsClient},
    submissions::{Assignment, SubmissionError},
};
use serde_json::{Value, json};
use std::time::Instant;
use url::Url;

const SETUP: &str = "PLMS configuration is missing or invalid. Configure LMS_SERVICE=plms, PLMS_USERNAME and PLMS_PASSWORD in the server environment.";
const LOGIN: &str = "PLMS sign-in failed. Check the server configuration and try again.";
const FETCH: &str =
    "PLMS could not complete this request. The session was cleared; try again later.";
const INVALID_URL: &str = "Expected a PLMS assignment URL with a positive id, or an announcement URL with positive id and bwid.";

#[derive(Default)]
pub struct LmsBackend {
    client: Option<PlmsClient>,
    cooldown: Option<(Instant, u64)>,
}

impl LmsBackend {
    pub fn new() -> Self {
        Self::default()
    }

    async fn client(&mut self) -> Result<&PlmsClient, String> {
        self.check_cooldown()?;
        if self.client.is_none() {
            let config = LmsConfig::from_env().map_err(|_| SETUP.to_string())?;
            let LmsConfig::Plms(config) = config;
            self.client = Some(
                PlmsClient::connect(&config)
                    .await
                    .map_err(|_| LOGIN.to_string())?,
            );
        }
        Ok(self.client.as_ref().expect("client initialized"))
    }

    fn check_cooldown(&mut self) -> Result<(), String> {
        if let Some((started, seconds)) = self.cooldown {
            let remaining = seconds.saturating_sub(started.elapsed().as_secs());
            if remaining > 0 {
                return Err(retry_message(remaining));
            }
            self.cooldown = None;
        }
        Ok(())
    }

    fn finish_call<T>(&mut self, result: Result<T, FetchError>) -> Result<T, String> {
        match result {
            Err(FetchError::RetryAfter(seconds)) => {
                let seconds = seconds.max(1);
                self.cooldown = Some((Instant::now(), seconds));
                Err(retry_message(seconds))
            }
            Err(FetchError::Failed) => self.finish(Err(FETCH.into())),
            Ok(value) => Ok(value),
        }
    }

    fn finish<T>(&mut self, result: Result<T, String>) -> Result<T, String> {
        if result.is_err() {
            self.client = None;
        }
        result
    }

    pub async fn call(&mut self, name: &str, args: &Value) -> Result<Value, String> {
        let request = Request::parse(name, args)?;
        let client = self.client().await?;
        let result = match request {
            Request::Courses(scope) => client.courses(scope).await.map(|rows| {
                json!({"courses": rows.iter().map(|c| json!({
                    "id": c.id, "name": c.name, "url": c.url,
                    "term": c.term.as_ref().map(|t| json!({"year": t.year,"code":t.code,"label":t.label()})),
                    "ends": c.ends,
                })).collect::<Vec<_>>()})
            }).map_err(|_| FetchError::Failed),
            Request::Deadlines(limit) => client.upcoming(limit as u32).await.map(|rows| {
                json!({"deadlines": rows.iter().take(limit).map(|d| json!({
                    "name":d.name,"course":d.course,"kind":d.kind.label(),"due":d.due,
                    "due_text":d.due_text,"url":d.url,"action":d.action,
                })).collect::<Vec<_>>(),"limit":limit,"complete":false,"selection":"upcoming calendar window; submitted assignments may be absent"})
            }).map_err(|_| FetchError::Failed),
            Request::Notices(scope, limit) => client.collect_announcements(scope).await.map(|(rows,warnings)| {
                json!({"notices":rows.iter().take(limit).map(|n| json!({
                    "course":n.course,"title":n.title,"date":n.date,"url":n.url,
                })).collect::<Vec<_>>(),"limit":limit,"truncated":rows.len()>limit,
                "partial":!warnings.is_empty(),"warnings":warnings.iter().map(|_| "A course notice board could not be read; the list is incomplete.").collect::<Vec<_>>()})
            }).map_err(|_| FetchError::Failed),
            Request::Assignments(limit) => client.submission_assignments().await.map(|rows| {
                json!({"assignments":rows.iter().take(limit).map(|a| json!({
                    "url":a.url,"title":a.title,"course":a.course,
                })).collect::<Vec<_>>(),"limit":limit,"truncated":rows.len()>limit})
            }).map_err(FetchError::from_submission),
            Request::Submission(url) => client.submission_status(&Assignment {
                url: url.clone(), title: String::new(), course: String::new(),
            }).await.map(|s| json!({"url":url,"status":s.status.label(),
                "files":s.files.iter().map(|f| json!({"name":f.name,"url":f.url})).collect::<Vec<_>>(),
                "online_text":s.online_text,"modified":s.modified,"due":s.due,
            })).map_err(FetchError::from_submission),
        };
        // Board collection can return partial results after an expired session.
        // Keep those results, but start the next request with a fresh session.
        if result
            .as_ref()
            .is_ok_and(|value| value.get("partial") == Some(&Value::Bool(true)))
        {
            self.client = None;
        }
        self.finish_call(result)
    }

    pub async fn read_content(
        &mut self,
        url: &str,
        cache: &mut ContentCache,
    ) -> Result<Content, String> {
        let url = validate_url(url, false)?;
        let client = self.client().await?;
        let result = content::read_lms(client, &url, cache)
            .await
            .map_err(|_| FETCH.to_string());
        // Attachment downloads may report authentication failures inside Content.
        if result
            .as_ref()
            .is_ok_and(|content| content.attachments.iter().any(|file| file.error.is_some()))
        {
            self.client = None;
        }
        self.finish(result)
    }
}

enum FetchError {
    Failed,
    RetryAfter(u64),
}
impl FetchError {
    fn from_submission(error: SubmissionError) -> Self {
        match error {
            SubmissionError::RetryAfter(seconds) => Self::RetryAfter(seconds),
            _ => Self::Failed,
        }
    }
}
fn retry_message(seconds: u64) -> String {
    format!("PLMS requested a pause. Retry after {seconds} seconds.")
}

#[derive(Debug)]
enum Request {
    Courses(Scope),
    Deadlines(usize),
    Notices(Scope, usize),
    Assignments(usize),
    Submission(String),
}
impl Request {
    fn parse(name: &str, args: &Value) -> Result<Self, String> {
        if !args.is_object() {
            return Err("Tool arguments must be an object.".into());
        }
        Ok(match name {
            "lms_courses" => Self::Courses(scope(args)?),
            "lms_deadlines" => Self::Deadlines(limit(args)?),
            "lms_notices" => Self::Notices(scope(args)?, limit(args)?),
            "lms_assignments" => Self::Assignments(limit(args)?),
            "lms_submission" => Self::Submission(validate_url(
                args.get("url").and_then(Value::as_str).ok_or(INVALID_URL)?,
                true,
            )?),
            _ => return Err("Unknown LMS tool.".into()),
        })
    }
}
fn scope(args: &Value) -> Result<Scope, String> {
    match args.get("all_terms") {
        None | Some(Value::Bool(false)) => Ok(Scope::CurrentTerm),
        Some(Value::Bool(true)) => Ok(Scope::All),
        _ => Err("all_terms must be a boolean.".into()),
    }
}
fn limit(args: &Value) -> Result<usize, String> {
    match args.get("limit") {
        None => Ok(30),
        Some(value) => value
            .as_u64()
            .filter(|n| (1..=100).contains(n))
            .map(|n| n as usize)
            .ok_or_else(|| "limit must be an integer from 1 to 100.".into()),
    }
}
fn validate_url(raw: &str, assignment_only: bool) -> Result<String, String> {
    let invalid = || INVALID_URL.to_string();
    let url = Url::parse(raw).map_err(|_| invalid())?;
    if url.scheme() != "https"
        || url.host_str() != Some("plms.postech.ac.kr")
        || url.port_or_known_default() != Some(443)
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return Err(invalid());
    }
    let keys: &[&str] = match url.path() {
        "/mod/assign/view.php" => &["id"],
        "/mod/ubboard/article.php" if !assignment_only => &["id", "bwid"],
        _ => return Err(invalid()),
    };
    let pairs = url.query_pairs().collect::<Vec<_>>();
    if pairs.len() != keys.len()
        || !keys.iter().all(|key| {
            pairs
                .iter()
                .filter(|(k, v)| {
                    k == key
                        && !v.is_empty()
                        && v.bytes().all(|b| b.is_ascii_digit())
                        && v.parse::<u64>().is_ok_and(|n| n > 0)
                })
                .count()
                == 1
        })
    {
        return Err(invalid());
    }
    Ok(url.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_supported_read_only_plms_urls_are_accepted() {
        let assignment = "https://plms.postech.ac.kr/mod/assign/view.php?id=233087";
        let notice = "https://plms.postech.ac.kr/mod/ubboard/article.php?bwid=3&id=2";
        assert_eq!(validate_url(assignment, true).unwrap(), assignment);
        assert!(validate_url(notice, false).is_ok());
        assert!(validate_url(notice, true).is_err());
        for bad in [
            "http://plms.postech.ac.kr/mod/assign/view.php?id=1",
            "https://evil.example/mod/assign/view.php?id=1",
            "https://plms.postech.ac.kr.evil.example/mod/assign/view.php?id=1",
            "https://user:secret@plms.postech.ac.kr/mod/assign/view.php?id=1",
            "https://plms.postech.ac.kr:444/mod/assign/view.php?id=1",
            "https://plms.postech.ac.kr/mod/assign/view.php?id=1#anything",
            "https://plms.postech.ac.kr/mod/assign/view.php?id=0",
            "https://plms.postech.ac.kr/mod/assign/view.php?id=-1",
            "https://plms.postech.ac.kr/mod/assign/view.php?id=1&id=2",
            "https://plms.postech.ac.kr/mod/assign/view.php?id=1&action=submit",
            "https://plms.postech.ac.kr/mod/assign/view.php?id=1&bwid=2",
            "https://plms.postech.ac.kr/mod/ubboard/article.php?id=1",
            "https://plms.postech.ac.kr/mod/assign/view.php",
        ] {
            assert_eq!(validate_url(bad, false).unwrap_err(), INVALID_URL, "{bad}");
        }
    }

    #[test]
    fn arguments_have_strict_bounds_and_defaults() {
        assert_eq!(limit(&json!({})).unwrap(), 30);
        assert_eq!(scope(&json!({})).unwrap(), Scope::CurrentTerm);
        assert_eq!(scope(&json!({"all_terms":true})).unwrap(), Scope::All);
        for n in [
            json!(0),
            json!(101),
            json!(-1),
            json!(1.5),
            json!("10"),
            Value::Null,
        ] {
            assert!(limit(&json!({"limit":n})).is_err());
        }
        assert!(scope(&json!({"all_terms":"false"})).is_err());
        assert!(Request::parse("lms_courses", &Value::Null).is_err());
    }

    #[tokio::test]
    async fn invalid_requests_fail_before_login_without_exposing_input() {
        let mut backend = LmsBackend::new();
        assert_eq!(
            backend
                .call(
                    "lms_submission",
                    &json!({"url":"https://user:secret@evil.example"})
                )
                .await
                .unwrap_err(),
            INVALID_URL
        );
        assert!(
            backend
                .call("lms_notices", &json!({"limit":101}))
                .await
                .is_err()
        );
        assert!(backend.call("lms_read", &json!({})).await.is_err());
        assert!(backend.client.is_none());
    }

    #[tokio::test]
    async fn server_retry_after_blocks_all_queries_and_content_before_login() {
        let mut backend = LmsBackend::new();
        let failure = FetchError::from_submission(SubmissionError::RetryAfter(120));
        assert_eq!(
            backend.finish_call::<()>(Err(failure)).unwrap_err(),
            retry_message(120)
        );
        for name in [
            "lms_courses",
            "lms_deadlines",
            "lms_notices",
            "lms_assignments",
        ] {
            let error = backend.call(name, &json!({})).await.unwrap_err();
            assert!(error.starts_with("PLMS requested a pause. Retry after "));
        }
        let url = "https://plms.postech.ac.kr/mod/assign/view.php?id=1";
        assert!(
            backend
                .call("lms_submission", &json!({"url":url}))
                .await
                .unwrap_err()
                .contains("pause")
        );
        let dir = tempfile::tempdir().unwrap();
        let mut cache = ContentCache::new(lms_helper::content::ContentConfig {
            cache_dir: dir.path().into(),
            max_file_bytes: 1024,
            max_total_bytes: 1024,
        });
        assert!(
            backend
                .read_content(url, &mut cache)
                .await
                .unwrap_err()
                .contains("pause")
        );
        assert!(backend.client.is_none());
        // No actual wait or credential access is needed to exercise expiry.
        backend.cooldown = Some((Instant::now() - std::time::Duration::from_secs(121), 120));
        assert!(backend.check_cooldown().is_ok());
        assert!(backend.cooldown.is_none());
    }

    #[test]
    fn failed_operations_are_not_retained_as_success() {
        let mut backend = LmsBackend::new();
        assert_eq!(backend.finish::<()>(Err(FETCH.into())).unwrap_err(), FETCH);
        assert!(backend.client.is_none());
        assert_eq!(backend.finish(Ok(42)).unwrap(), 42);
    }
}
