//! Emulates what a browser would do with the pages the handoff serves: follow a
//! redirect, take a scripted navigation, or auto-submit a form.

use url::Url;

use crate::services::lms::plms::{
    PlmsError,
    http::{PLMS, Response, SSO, Session, debug_log, origin, trusted},
    page::{FormPost, first_form, passni_error_code, script_redirects},
};

/// Where the PLMS side of the handoff lives; everything else on PLMS is the app.
const PASSNI: &str = "/passni/";
/// The leg of the handoff that answers with scripted pages rather than redirects.
pub(crate) const PASSNI_SSO: &str = "/passni/sso/";
/// The dead branch of the handoff page's `err_code` guard.
const HANDOFF_ERROR: &str = "/error.php";
/// Cap on the auto-navigation loop, comfortably above the hops the flow needs.
const MAX_NAVIGATIONS: usize = 8;

/// What a browser would do next with a page mid-handoff.
#[derive(Debug)]
enum Step {
    Get(Url),
    Post(FormPost),
    Stop,
}

impl Session {
    /// Drives redirects, scripted navigations and auto-submitted forms until the
    /// handoff lands somewhere a browser would stop.
    pub(crate) async fn follow(&self, mut response: Response) -> Result<Response, PlmsError> {
        for _ in 0..MAX_NAVIGATIONS {
            let current = response.url.clone();
            response = match next_step(&response)? {
                Step::Stop => return Ok(response),
                Step::Get(target) => {
                    trusted(&target)?;
                    self.get(target.as_str(), Some(current.as_str())).await?
                }
                Step::Post((action, form)) => {
                    trusted(&action)?;
                    debug_log(format!(
                        "auto-post {} with fields: {}",
                        action.path(),
                        field_names(&form)
                    ));
                    self.post(action.as_str(), form, Some(current.as_str()))
                        .await?
                }
            };
        }
        Err(PlmsError::NavigationLimit)
    }
}

/// Reads one page and decides what a browser would do with it next.
fn next_step(response: &Response) -> Result<Step, PlmsError> {
    let current = &response.url;
    if response.status.is_redirection()
        && let Some(location) = &response.location
    {
        return Ok(Step::Get(current.join(location)?));
    }
    // Anywhere on PLMS outside the handoff is the destination.
    if origin(current) == PLMS && !current.path().starts_with(PASSNI) {
        return Ok(Step::Stop);
    }
    if current.path().starts_with(PASSNI_SSO) {
        // The handoff page scripts an error branch and a success branch; its own
        // `err_code` guard decides which one a browser would take.
        let error_code = passni_error_code(&response.body);
        if let Some(code) = error_code.as_deref().filter(|code| !code.is_empty()) {
            return Err(PlmsError::HandoffRejected(code.to_owned()));
        }
        let mut targets = script_redirects(&response.body, current)?.into_iter();
        let target = match error_code {
            // The guard was present and empty, so the error branch is dead.
            Some(_) => targets.find(|target| !target.path().ends_with(HANDOFF_ERROR)),
            None => targets.next(),
        };
        if let Some(target) = target {
            return Ok(Step::Get(target));
        }
    }
    if (origin(current) == SSO || current.path().starts_with(PASSNI_SSO))
        && let Some(form) = first_form(&response.body, current)?
    {
        return Ok(Step::Post(form));
    }
    Ok(Step::Stop)
}

/// Field names only: a handoff form's values carry SSO tokens.
fn field_names(form: &[(String, String)]) -> String {
    form.iter()
        .map(|(name, _)| name.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use reqwest::StatusCode;

    use super::*;

    fn page(url: &str, status: u16, location: Option<&str>, body: &str) -> Response {
        Response {
            url: Url::parse(url).unwrap(),
            status: StatusCode::from_u16(status).unwrap(),
            location: location.map(str::to_owned),
            body: body.to_owned(),
        }
    }

    const HANDOFF: &str = r#"<script>
        var err_code = '';
        if (err_code != '') {
            location.href = '/passni/coursemos/error.php' + '?errorCode=' + err_code;
        } else {
            document.location.href = '/passni/coursemos/loginBeforeProc.php';
        }
        </script>"#;

    #[test]
    fn a_redirect_is_followed() {
        let response = page(
            "https://plms.postech.ac.kr/",
            303,
            Some("https://plms.postech.ac.kr/login/index.php"),
            "",
        );
        let Step::Get(target) = next_step(&response).unwrap() else {
            panic!("expected a redirect");
        };
        assert_eq!(target.path(), "/login/index.php");
    }

    #[test]
    fn the_handoff_takes_the_live_branch_not_the_error_branch() {
        let response = page(
            "https://plms.postech.ac.kr/passni/sso/spLoginData.php",
            200,
            None,
            HANDOFF,
        );
        let Step::Get(target) = next_step(&response).unwrap() else {
            panic!("expected a scripted navigation");
        };
        assert_eq!(target.path(), "/passni/coursemos/loginBeforeProc.php");
    }

    #[test]
    fn a_handoff_error_code_is_reported_rather_than_followed() {
        let body = HANDOFF.replace("var err_code = ''", "var err_code = 'PN0012'");
        let response = page(
            "https://plms.postech.ac.kr/passni/sso/spLoginData.php",
            200,
            None,
            &body,
        );
        let error = next_step(&response).unwrap_err();
        assert!(matches!(error, PlmsError::HandoffRejected(code) if code == "PN0012"));
    }

    #[test]
    fn an_sso_page_auto_posts_its_form() {
        let response = page(
            "https://sso.postech.ac.kr/sso/usr/postech/login/link",
            200,
            None,
            r#"<form action="https://plms.postech.ac.kr/passni/sso/spLoginData.php">
                <input type="hidden" name="pni_data" value="DEAD" />
            </form>"#,
        );
        let Step::Post((action, form)) = next_step(&response).unwrap() else {
            panic!("expected an auto-post");
        };
        assert_eq!(action.path(), "/passni/sso/spLoginData.php");
        assert_eq!(field_names(&form), "pni_data");
    }

    #[test]
    fn the_dashboard_is_the_end_of_the_walk() {
        let response = page("https://plms.postech.ac.kr/", 200, None, "<html></html>");
        assert!(matches!(next_step(&response).unwrap(), Step::Stop));
    }
}
