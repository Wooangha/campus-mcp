//! The HTTP session every step shares: one cookie jar, and no automatic
//! redirects so each hop of the handoff can be inspected before it is taken.

use std::sync::Arc;

use reqwest::{
    Client as HttpClient, Method, StatusCode,
    cookie::Jar,
    header::{ACCEPT, ACCEPT_LANGUAGE, CONTENT_TYPE, LOCATION, ORIGIN, REFERER},
};
use url::Url;

use crate::services::lms::plms::PlmsError;

pub(crate) const PLMS: &str = "https://plms.postech.ac.kr";
pub(crate) const SSO: &str = "https://sso.postech.ac.kr";
const USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/140.0.0.0 Safari/537.36";

/// One fetched page, kept whole because the handoff is driven from its body.
#[derive(Debug)]
pub(crate) struct Response {
    pub(crate) url: Url,
    pub(crate) status: StatusCode,
    pub(crate) location: Option<String>,
    pub(crate) body: String,
}

/// Cookie-bearing HTTP session. Cookies live in memory for its lifetime only.
pub(crate) struct Session {
    http: HttpClient,
}

impl Session {
    pub(crate) fn new() -> Result<Self, PlmsError> {
        let http = HttpClient::builder()
            .cookie_provider(Arc::new(Jar::default()))
            .redirect(reqwest::redirect::Policy::none())
            .user_agent(USER_AGENT)
            .timeout(std::time::Duration::from_secs(30))
            .build()?;
        Ok(Self { http })
    }

    /// Follow only same-origin read-only content/file URLs, never SSO forms.
    pub(crate) async fn content_get(
        &self,
        url: &str,
        file: bool,
    ) -> Result<reqwest::Response, crate::services::content::ContentError> {
        use crate::services::content::ContentError;
        let mut url = Url::parse(url).map_err(|_| ContentError::InvalidUrl)?;
        for _ in 0..5 {
            validate_content_url(&url, file)?;
            let response = self
                .http
                .get(url.clone())
                .send()
                .await
                .map_err(|_| ContentError::Network)?;
            if response.status().is_redirection() {
                let location = response
                    .headers()
                    .get(LOCATION)
                    .and_then(|v| v.to_str().ok())
                    .ok_or(ContentError::Authentication)?;
                url = url.join(location).map_err(|_| ContentError::InvalidUrl)?;
                continue;
            }
            if file
                && response
                    .headers()
                    .get(CONTENT_TYPE)
                    .and_then(|v| v.to_str().ok())
                    .is_some_and(|v| v.starts_with("text/html"))
            {
                return Err(ContentError::Authentication);
            }
            return Ok(response);
        }
        Err(ContentError::InvalidUrl)
    }

    /// Course/notice list fetches never follow redirects or perform form actions.
    pub(crate) async fn board_page(&self, url: &str) -> Result<String, PlmsError> {
        let parsed = Url::parse(url)?;
        let allowed_path = matches!(parsed.path(), "/course/view.php" | "/mod/ubboard/view.php");
        if parsed.origin().ascii_serialization() != PLMS
            || !parsed.username().is_empty()
            || parsed.password().is_some()
            || !allowed_path
            || !parsed
                .query_pairs()
                .all(|(k, v)| matches!(k.as_ref(), "id" | "page") && v.parse::<u64>().is_ok())
        {
            return Err(crate::services::content::ContentError::InvalidUrl.into());
        }
        let response = self
            .http
            .get(parsed)
            .send()
            .await
            .map_err(|_| crate::services::content::ContentError::Network)?;
        let bytes = crate::services::content::read_limited(response, 4 * 1024 * 1024).await?;
        let body = String::from_utf8_lossy(&bytes).into_owned();
        if !body.contains("/login/logout.php") {
            return Err(PlmsError::MissingMoodleSession);
        }
        Ok(body)
    }

    /// Submission monitoring only follows plain same-origin read requests.
    pub(crate) async fn submission_page(
        &self,
        url: &str,
    ) -> Result<String, crate::services::submissions::SubmissionError> {
        use crate::services::submissions::SubmissionError;
        let parsed =
            Url::parse(url).map_err(|_| SubmissionError::Source("잘못된 PLMS 주소".into()))?;
        if parsed.origin().ascii_serialization() != PLMS {
            return Err(SubmissionError::Source("허용되지 않은 PLMS 주소".into()));
        }
        let response = self
            .http
            .get(parsed)
            .timeout(std::time::Duration::from_secs(30))
            .send()
            .await
            .map_err(|e| SubmissionError::Source(e.without_url().to_string()))?;
        let status = response.status();
        if status == StatusCode::TOO_MANY_REQUESTS || status == StatusCode::SERVICE_UNAVAILABLE {
            let delay = response
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| {
                    v.parse::<u64>().ok().or_else(|| {
                        chrono::DateTime::parse_from_rfc2822(v).ok().map(|t| {
                            (t.timestamp() - crate::services::time::now_unix()).max(1) as u64
                        })
                    })
                })
                .unwrap_or(600);
            return Err(SubmissionError::RetryAfter(delay));
        }
        if status.is_redirection() || status == StatusCode::UNAUTHORIZED {
            return Err(SubmissionError::SessionExpired);
        }
        if status == StatusCode::FORBIDDEN || status == StatusCode::NOT_FOUND {
            return Err(SubmissionError::Unreadable(format!("PLMS HTTP {status}")));
        }
        if !status.is_success() {
            return Err(SubmissionError::Source(format!("PLMS HTTP {status}")));
        }
        let body = response
            .text()
            .await
            .map_err(|e| SubmissionError::Source(e.without_url().to_string()))?;
        if !body.contains("/login/logout.php") {
            return Err(SubmissionError::SessionExpired);
        }
        Ok(body)
    }

    pub(crate) async fn get(
        &self,
        url: &str,
        referer: Option<&str>,
    ) -> Result<Response, PlmsError> {
        self.request(Method::GET, url, None, referer).await
    }

    pub(crate) async fn post(
        &self,
        url: &str,
        form: Vec<(String, String)>,
        referer: Option<&str>,
    ) -> Result<Response, PlmsError> {
        self.request(Method::POST, url, Some(form), referer).await
    }

    pub(crate) async fn post_json(
        &self,
        url: &str,
        body: String,
        referer: Option<&str>,
    ) -> Result<Response, PlmsError> {
        let mut request = self
            .http
            .post(url)
            .header(ACCEPT, "application/json, text/javascript, */*; q=0.01")
            .header(CONTENT_TYPE, "application/json")
            .header("X-Requested-With", "XMLHttpRequest")
            .body(body);
        if let Some(referer) = referer {
            request = request
                .header(REFERER, referer)
                .header(ORIGIN, Url::parse(referer)?.origin().ascii_serialization());
        }
        let response = request.send().await?;
        let url = response.url().clone();
        let status = response.status();
        debug_log(format!("POST {} -> {}", url.path(), status));
        Ok(Response {
            url,
            status,
            location: None,
            body: response.text().await?,
        })
    }

    async fn request(
        &self,
        method: Method,
        url: &str,
        form: Option<Vec<(String, String)>>,
        referer: Option<&str>,
    ) -> Result<Response, PlmsError> {
        let mut request = self
            .http
            .request(method.clone(), url)
            .header(ACCEPT, "text/html,application/json;q=0.9,*/*;q=0.8")
            .header(ACCEPT_LANGUAGE, "ko-KR,ko;q=0.9,en;q=0.8")
            .header("Sec-Fetch-Site", "same-site")
            .header("Sec-Fetch-Mode", "navigate")
            .header("Sec-Fetch-Dest", "document")
            .header("Upgrade-Insecure-Requests", "1");
        if let Some(referer) = referer {
            request = request.header(REFERER, referer);
            if method == Method::POST {
                request =
                    request.header(ORIGIN, Url::parse(referer)?.origin().ascii_serialization())
            }
        }
        if let Some(form) = form {
            request = request
                .header(
                    CONTENT_TYPE,
                    "application/x-www-form-urlencoded;charset=UTF-8",
                )
                .form(&form)
        }
        let response = request.send().await?;
        let url = response.url().clone();
        let status = response.status();
        debug_log(format!("{} {} -> {}", method, url.path(), status));
        let location = response
            .headers()
            .get(LOCATION)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        Ok(Response {
            url,
            status,
            location,
            body: response.text().await?,
        })
    }
}

fn validate_content_url(
    url: &Url,
    file: bool,
) -> Result<(), crate::services::content::ContentError> {
    let allowed = if file {
        url.path().starts_with("/pluginfile.php/") || url.path() == "/mod/ubboard/download.php"
    } else {
        matches!(
            url.path(),
            "/mod/assign/view.php" | "/mod/ubboard/article.php"
        ) && url
            .query_pairs()
            .all(|(key, _)| matches!(key.as_ref(), "id" | "bwid"))
    };
    if url.origin().ascii_serialization() != PLMS
        || !allowed
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(crate::services::content::ContentError::InvalidUrl);
    }
    Ok(())
}

pub(crate) fn origin(url: &Url) -> String {
    url.origin().ascii_serialization()
}

/// Guards every hop: the handoff must stay on PLMS or POSTECH SSO.
pub(crate) fn trusted(url: &Url) -> Result<(), PlmsError> {
    let origin = origin(url);
    if origin == PLMS || origin == SSO {
        Ok(())
    } else {
        Err(PlmsError::UnexpectedRedirect(origin))
    }
}

/// Opt-in tracing. Never given a credential, a cookie or a form value.
pub(crate) fn debug_log(message: String) {
    if std::env::var("PLMS_DEBUG").as_deref() == Ok("1") {
        eprintln!("[plms-debug] {message}");
    }
    if let Ok(path) = std::env::var("PLMS_DEBUG_FILE") {
        use std::io::Write;

        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
        {
            let _ = writeln!(file, "{message}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_downloads_stay_on_read_only_plms_endpoints() {
        for address in [
            "https://evil.test/pluginfile.php/x",
            "https://plms.postech.ac.kr/login/logout.php",
            "https://plms.postech.ac.kr/mod/assign/view.php?id=1&action=submit",
        ] {
            let url = Url::parse(address).unwrap();
            assert!(validate_content_url(&url, false).is_err());
            assert!(validate_content_url(&url, true).is_err());
        }
        assert!(
            validate_content_url(
                &Url::parse("https://plms.postech.ac.kr/pluginfile.php/1/file.pdf?forcedownload=1")
                    .unwrap(),
                true
            )
            .is_ok()
        );
    }

    #[test]
    fn a_hop_off_the_two_known_origins_is_refused() {
        assert!(matches!(
            trusted(&Url::parse("https://evil.example/").unwrap()),
            Err(PlmsError::UnexpectedRedirect(_))
        ));
    }

    #[test]
    fn both_handoff_origins_are_trusted() {
        assert!(trusted(&Url::parse(&format!("{PLMS}/login/index.php")).unwrap()).is_ok());
        assert!(trusted(&Url::parse(&format!("{SSO}/sso/usr/login/link")).unwrap()).is_ok());
    }
}
