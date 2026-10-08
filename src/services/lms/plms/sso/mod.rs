//! The POSTECH SSO → PLMS login flow, one method per leg of the handoff.

mod crypto;

use p256::{
    SecretKey,
    elliptic_curve::{rand_core::OsRng, sec1::ToEncodedPoint},
};
use serde_json::Value;

use crate::services::lms::plms::{
    PlmsError,
    http::{PLMS, Response, SSO, Session},
    navigate::PASSNI_SSO,
    page::{required_input, required_inputs},
    sso::crypto::{seed_encrypt, seed_key_iv, shared_point},
};

/// PLMS's entry point into SSO, which hands over the agent form below.
const SP_LOGIN: &str = "spLogin.php";
/// SSO endpoints: agent link, key exchange, credential check, PLMS handoff, client IP.
const AGENT_LINK: &str = "/sso/usr/login/link";
const LOGIN_INIT: &str = "/sso/usr/postech/login/init";
const LOGIN_AUTH: &str = "/sso/usr/postech/login/auth";
const LOGIN_LINK: &str = "/sso/usr/postech/login/link";
const CLIENT_IP: &str = "/sso/api/cors/get/ip";
/// The PLMS pages that finish the handoff once SSO has vouched for the user.
const LOGIN_BEFORE_PROC: &str = "/loginBeforeProc.php";
const LOGIN_PROC: &str = "/passni/coursemos/loginProc.php";
/// Present on any Moodle page rendered for a signed-in user.
const LOGOUT_LINK: &str = "/login/logout.php";

/// The hidden fields PLMS's entry page hands to SSO to identify the agent.
const AGENT_FIELDS: [&str; 4] = ["agt_id", "agt_url", "agt_r", "csmsRedirect"];

/// The code SSO returns from both the key exchange and the credential check on success.
const SSO_OK: &str = "SS0001";
/// Prefix of the codes SSO returns when StonePASS is required.
const SECOND_FACTOR: &str = "EOTP";

/// Local stateful POSTECH SSO → PLMS client. Credentials and cookies are memory-only.
pub struct PlmsClient {
    session: Session,
    authenticated: bool,
}

impl PlmsClient {
    pub fn new() -> Result<Self, PlmsError> {
        Ok(Self {
            session: Session::new()?,
            authenticated: false,
        })
    }

    pub fn authenticated(&self) -> bool {
        self.authenticated
    }

    /// Logs in without persisting credentials. Accounts requiring StonePASS return an error.
    pub async fn login(&mut self, username: &str, password: &str) -> Result<(), PlmsError> {
        self.authenticated = false;
        let page = self.open_sso_login().await?;
        let login_key = required_input(&page.body, "login_key")?;
        let shared = self.exchange_keys(&page).await?;
        self.submit_credentials(&page, &login_key, &shared, username, password)
            .await?;
        self.complete_handoff(&page, &login_key).await?;
        self.verify_session().await?;
        self.authenticated = true;
        Ok(())
    }

    pub async fn dashboard_html(&self) -> Result<String, PlmsError> {
        self.session()?;
        Ok(self.visit_dashboard().await?.body)
    }

    /// Walks PLMS's SSO entry point through to the SSO login page.
    async fn open_sso_login(&self) -> Result<Response, PlmsError> {
        self.session.get(&format!("{PLMS}/"), None).await?;
        let start = self
            .session
            .get(&format!("{PLMS}{PASSNI_SSO}{SP_LOGIN}"), None)
            .await?;
        let agent_form = required_inputs(&start.body, &AGENT_FIELDS)?;
        let page = self
            .session
            .post(
                &format!("{SSO}{AGENT_LINK}"),
                agent_form,
                Some(start.url.as_str()),
            )
            .await?;
        self.session.follow(page).await
    }

    /// ECDH with the SSO server; the shared point keys the credential envelope.
    async fn exchange_keys(&self, page: &Response) -> Result<Vec<u8>, PlmsError> {
        let secret = SecretKey::random(&mut OsRng);
        let public = secret.public_key().to_encoded_point(false);
        let exchange = self
            .session
            .post(
                &format!("{SSO}{LOGIN_INIT}"),
                vec![(
                    "user_ec_publickey".into(),
                    hex::encode(&public.as_bytes()[1..]),
                )],
                Some(page.url.as_str()),
            )
            .await?;
        let exchange: Value = serde_json::from_str(&exchange.body)?;
        // Checked before the key fields so a rejection reports its own code
        // instead of the missing qx/qy that a rejection response omits anyway.
        let code = sso_code(&exchange);
        if code != SSO_OK {
            return Err(PlmsError::AuthenticationFailed(code.into()));
        }
        let (Some(qx), Some(qy)) = (exchange["svr_qx"].as_str(), exchange["svr_qy"].as_str())
        else {
            return Err(PlmsError::InvalidSsoResponse);
        };
        shared_point(&secret, qx, qy)
    }

    /// Sends the credentials SEED-encrypted under the shared key. They are never logged.
    async fn submit_credentials(
        &self,
        page: &Response,
        login_key: &str,
        shared: &[u8],
        username: &str,
        password: &str,
    ) -> Result<(), PlmsError> {
        let data = serde_json::json!({ "login_id": username, "login_pwd": password }).to_string();
        let (key, iv) = seed_key_iv(shared)?;
        let encrypted = seed_encrypt(data.as_bytes(), key, iv)?;
        let auth = self
            .session
            .post(
                &format!("{SSO}{LOGIN_AUTH}"),
                vec![
                    ("user_data".into(), hex::encode(encrypted)),
                    ("login_key".into(), login_key.to_owned()),
                ],
                Some(page.url.as_str()),
            )
            .await?;
        let auth: Value = serde_json::from_str(&auth.body)?;
        let code = sso_code(&auth);
        if code.starts_with(SECOND_FACTOR) {
            return Err(PlmsError::SecondFactorRequired(code.into()));
        }
        if code != SSO_OK {
            return Err(PlmsError::AuthenticationFailed(code.into()));
        }
        Ok(())
    }

    /// Carries the authenticated SSO session over to a Moodle session on PLMS.
    async fn complete_handoff(&self, page: &Response, login_key: &str) -> Result<(), PlmsError> {
        let handoff = self
            .session
            .post(
                &format!("{SSO}{LOGIN_LINK}"),
                vec![("login_key".into(), login_key.to_owned())],
                Some(page.url.as_str()),
            )
            .await?;
        let handoff = self.session.follow(handoff).await?;
        if handoff.url.path().ends_with(LOGIN_BEFORE_PROC) {
            self.submit_client_ip(&handoff).await?;
        }
        Ok(())
    }

    /// `loginBeforeProc.php` has no form in its HTML: a script fetches the client
    /// IP from SSO and posts a form it builds. Both steps are reproduced here.
    async fn submit_client_ip(&self, page: &Response) -> Result<(), PlmsError> {
        let referer = page.url.as_str();
        let ip = self
            .session
            .post(&format!("{SSO}{CLIENT_IP}"), Vec::new(), Some(referer))
            .await?;
        let ip: Value = serde_json::from_str(&ip.body)?;
        let client_ip = ip["client_ip"]
            .as_str()
            .ok_or(PlmsError::InvalidSsoResponse)?;
        let proc = self
            .session
            .post(
                &format!("{PLMS}{LOGIN_PROC}"),
                vec![("pni_client_ip".into(), client_ip.into())],
                Some(referer),
            )
            .await?;
        self.session.follow(proc).await?;
        Ok(())
    }

    /// The signed-in session the Moodle queries are built on. Asking before a
    /// successful login is an error rather than a request that quietly fails.
    pub(crate) fn session(&self) -> Result<&Session, PlmsError> {
        if !self.authenticated {
            return Err(PlmsError::MissingMoodleSession);
        }
        Ok(&self.session)
    }

    /// Confirms PLMS now serves the dashboard as a signed-in user.
    async fn verify_session(&self) -> Result<(), PlmsError> {
        if !self.visit_dashboard().await?.body.contains(LOGOUT_LINK) {
            return Err(PlmsError::MissingMoodleSession);
        }
        Ok(())
    }

    /// The PLMS root, with its redirect to the signed-in dashboard followed.
    async fn visit_dashboard(&self) -> Result<Response, PlmsError> {
        let response = self.session.get(&format!("{PLMS}/"), None).await?;
        self.session.follow(response).await
    }
}

fn sso_code(response: &Value) -> &str {
    response["code"].as_str().unwrap_or("unknown")
}
