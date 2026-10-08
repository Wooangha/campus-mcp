//! OAuth2 for the Outlook mailbox: a one-time device-code sign-in, then a
//! refresh token kept on disk so later runs need no browser.
//!
//! The mailbox lives in Microsoft 365, which retired password auth, so this is
//! the only way in. The sign-in itself, with its second factor, happens in the
//! user's browser — the credentials never reach this process.

use std::{
    io::Write,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use serde_json::Value;

use super::MailError;
use super::OutlookClient;

/// Microsoft's public "Graph Command Line Tools" client, pre-consented for the
/// delegated scopes below, so no per-tenant app registration is needed.
const CLIENT_ID: &str = "14d82eec-204b-4c2f-b7e8-296a70dab67e";
/// `offline_access` is what makes the sign-in yield a refresh token.
const SCOPE: &str = "offline_access Mail.Read";
/// How long to keep polling for the browser sign-in before giving up.
const DEVICE_FLOW_TIMEOUT: Duration = Duration::from_secs(300);

impl OutlookClient {
    /// Runs the browser sign-in and stores the refresh token it returns.
    pub async fn login(&self) -> Result<(), MailError> {
        self.device_flow().await.map(|_| ())
    }

    /// Refreshes the stored sign-in; browser login is always explicit.
    pub(super) async fn access_token(&self) -> Result<String, MailError> {
        let refresh = load_refresh_token()?.ok_or_else(|| {
            MailError::Auth("로그인이 필요합니다. 먼저 `mail login`을 실행하세요".into())
        })?;
        match self.redeem_refresh_token(&refresh).await {
            Err(MailError::Auth(_)) => Err(MailError::Auth(
                "저장된 로그인이 만료되었습니다. `mail login`으로 다시 로그인하세요".into(),
            )),
            result => result,
        }
    }

    /// Trades a refresh token for an access token, saving the rotated refresh
    /// token that comes back with it.
    async fn redeem_refresh_token(&self, refresh: &str) -> Result<String, MailError> {
        let response = self
            .http
            .post(self.token_url())
            .form(&[
                ("client_id", CLIENT_ID),
                ("grant_type", "refresh_token"),
                ("refresh_token", refresh),
                ("scope", SCOPE),
            ])
            .send()
            .await?;
        let token: Value = serde_json::from_str(&response.text().await?)?;
        finish_token_response(&token)
    }

    /// The device-code sign-in: show the user a code, then poll until they have
    /// signed in through their browser.
    async fn device_flow(&self) -> Result<String, MailError> {
        let start = self
            .http
            .post(self.device_code_url())
            .form(&[("client_id", CLIENT_ID), ("scope", SCOPE)])
            .send()
            .await?;
        let start: Value = serde_json::from_str(&start.text().await?)?;
        let Some(device_code) = start["device_code"].as_str() else {
            return Err(oauth_error(&start));
        };
        // Microsoft's `message` already spells out the URL and code in full.
        if let Some(message) = start["message"].as_str() {
            println!("{message}");
        }
        let mut interval = Duration::from_secs(start["interval"].as_u64().unwrap_or(5));

        let deadline = Instant::now() + DEVICE_FLOW_TIMEOUT;
        loop {
            if Instant::now() >= deadline {
                return Err(MailError::Auth("로그인 대기 시간이 지났습니다".into()));
            }
            tokio::time::sleep(interval).await;
            let poll = self
                .http
                .post(self.token_url())
                .form(&[
                    ("client_id", CLIENT_ID),
                    ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
                    ("device_code", device_code),
                ])
                .send()
                .await?;
            let poll: Value = serde_json::from_str(&poll.text().await?)?;
            match poll["error"].as_str() {
                None => return finish_token_response(&poll),
                // Still waiting on the browser; keep polling.
                Some("authorization_pending") => continue,
                // The server asked us to back off; obey and keep going.
                Some("slow_down") => interval += Duration::from_secs(5),
                Some(_) => return Err(oauth_error(&poll)),
            }
        }
    }
}

/// Extracts the access token from a token response, persisting the refresh
/// token that rides along with it.
fn finish_token_response(token: &Value) -> Result<String, MailError> {
    if token["error"].is_string() {
        return Err(oauth_error(token));
    }
    let access = token["access_token"]
        .as_str()
        .filter(|value| !value.is_empty())
        .ok_or(MailError::InvalidResponse)?;
    if let Some(refresh) = token["refresh_token"].as_str() {
        save_refresh_token(refresh)?;
    }
    Ok(access.to_owned())
}

/// Turns an OAuth error body into a message, preferring its human description.
fn oauth_error(body: &Value) -> MailError {
    let code = body["error"].as_str().unwrap_or("unknown");
    let described = body["error_description"]
        .as_str()
        // The Microsoft description is a paragraph; its first line is the gist.
        .and_then(|text| text.lines().next())
        .unwrap_or(code);
    MailError::Auth(described.to_owned())
}

/// Whether a stored sign-in is present. A read error counts as absent: the
/// caller only wants to know whether to prompt for a browser sign-in.
pub(super) fn has_stored_login() -> bool {
    matches!(load_refresh_token(), Ok(Some(_)))
}

/// Where the refresh token lives: user-only, outside the repo.
fn token_path() -> Result<PathBuf, MailError> {
    let home =
        std::env::var("HOME").map_err(|_| MailError::Auth("HOME 환경변수가 없습니다".into()))?;
    Ok(PathBuf::from(home).join(".config/lms-helper/outlook-token.json"))
}

fn load_refresh_token() -> Result<Option<String>, MailError> {
    let path = token_path()?;
    match std::fs::read_to_string(&path) {
        Ok(contents) => {
            let stored: Value = serde_json::from_str(&contents)?;
            Ok(stored["refresh_token"].as_str().map(str::to_owned))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

/// Replaces the stored token only after the new file is fully written.
fn save_refresh_token(refresh: &str) -> Result<(), MailError> {
    save_refresh_token_at(&token_path()?, refresh)
}

fn save_refresh_token_at(path: &Path, refresh: &str) -> Result<(), MailError> {
    let parent = path.parent().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "token path has no parent")
    })?;
    std::fs::create_dir_all(parent)?;
    // tempfile creates an owner-only file on Unix, alongside the destination
    // so persist can atomically replace it on the same filesystem.
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    let body = serde_json::json!({ "refresh_token": refresh }).to_string();
    file.write_all(body.as_bytes())?;
    file.as_file().sync_all()?;
    file.persist(path).map_err(|error| error.error)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replacement_writes_complete_json_with_private_permissions() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("token.json");
        std::fs::write(&path, "old token").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        }
        save_refresh_token_at(&path, "new token").unwrap();
        let stored: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(stored["refresh_token"], "new token");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn failed_replacement_keeps_destination_and_cleans_up_tempfile() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("token.json");
        std::fs::create_dir(&path).unwrap();
        std::fs::write(path.join("marker"), "keep").unwrap();
        assert!(save_refresh_token_at(&path, "new token").is_err());
        assert_eq!(
            std::fs::read_to_string(path.join("marker")).unwrap(),
            "keep"
        );
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn an_oauth_error_reads_back_its_first_description_line() {
        let body = serde_json::json!({
            "error": "authorization_declined",
            "error_description": "AADSTS65004: 사용자가 동의를 거부했습니다.\r\nTrace ID: abc",
        });
        let MailError::Auth(message) = oauth_error(&body) else {
            panic!("expected a MailAuth error");
        };
        assert_eq!(message, "AADSTS65004: 사용자가 동의를 거부했습니다.");
    }

    #[test]
    fn an_oauth_error_falls_back_to_the_code() {
        let MailError::Auth(message) =
            oauth_error(&serde_json::json!({ "error": "invalid_grant" }))
        else {
            panic!("expected a MailAuth error");
        };
        assert_eq!(message, "invalid_grant");
    }

    #[test]
    fn a_token_response_without_an_access_token_is_rejected() {
        let error = finish_token_response(&serde_json::json!({ "token_type": "Bearer" }));
        assert!(matches!(error, Err(MailError::InvalidResponse)));
    }
}
