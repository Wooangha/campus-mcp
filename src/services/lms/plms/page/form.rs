//! The hidden forms the handoff pages carry, and the fields they submit.

use std::sync::LazyLock;

use regex::Regex;
use url::Url;

use super::unescape;
use crate::services::lms::plms::PlmsError;

/// A form ready to be submitted: where it posts, and the fields it carries.
pub(crate) type FormPost = (Url, Vec<(String, String)>);

/// The handoff form PLMS expects, preferred over any other form on the page.
const SP_LOGIN_DATA: &str = "/passni/sso/spLoginData.php";

static FORM: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?is)<form\b([^>]*)>(.*?)</form>").unwrap());
static ACTION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?i)\baction=["']([^"']+)["']"#).unwrap());
static INPUT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?is)<input\b[^>]*\bname=["']([^"']+)["'][^>]*>"#).unwrap());
static VALUE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?i)\bvalue=["']([^"']*)["']"#).unwrap());

/// The named hidden inputs a page must carry, in the order they were asked for.
pub(crate) fn required_inputs(
    html: &str,
    names: &[&'static str],
) -> Result<Vec<(String, String)>, PlmsError> {
    names
        .iter()
        .map(|name| {
            INPUT
                .captures_iter(html)
                .find(|tag| &tag[1] == *name)
                .map(|tag| (name.to_string(), input_value(&tag[0])))
                .ok_or(PlmsError::MissingInput(name))
        })
        .collect()
}

/// One named hidden input's value.
pub(crate) fn required_input(html: &str, name: &'static str) -> Result<String, PlmsError> {
    required_inputs(html, &[name])?
        .pop()
        .map(|(_, value)| value)
        .ok_or(PlmsError::MissingInput(name))
}

/// The form a browser would auto-submit on this page: the PLMS handoff form if
/// the page carries one, otherwise the first form with an action.
pub(crate) fn first_form(html: &str, base: &Url) -> Result<Option<FormPost>, PlmsError> {
    // Collected up front: `find` on a lazy iterator consumes it when it misses,
    // which would leave the fallback below with nothing to return.
    let candidates: Vec<(String, &str)> = FORM
        .captures_iter(html)
        .filter_map(|form| {
            let action = ACTION.captures(&form[1]).map(|found| unescape(&found[1]))?;
            let body = form.get(2)?.as_str();
            (!action.is_empty()).then_some((action, body))
        })
        .collect();
    let Some((action, body)) = candidates
        .iter()
        .find(|(action, _)| action.contains(SP_LOGIN_DATA))
        .or_else(|| candidates.first())
    else {
        return Ok(None);
    };

    let fields = INPUT
        .captures_iter(body)
        .map(|tag| (unescape(&tag[1]), input_value(&tag[0])))
        .collect();
    Ok(Some((base.join(action)?, fields)))
}

/// An input tag's value; one rendered without `value` submits an empty string.
fn input_value(tag: &str) -> String {
    VALUE
        .captures(tag)
        .map(|found| unescape(&found[1]))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> Url {
        Url::parse("https://sso.postech.ac.kr/sso/usr/login/link").unwrap()
    }

    #[test]
    fn first_form_falls_back_to_the_first_form() {
        let html = r#"<form action="/sso/usr/next" method="post">
            <input type="hidden" name="login_key" value="abc">
        </form>"#;
        let (action, fields) = first_form(html, &base()).unwrap().unwrap();
        assert_eq!(action.path(), "/sso/usr/next");
        assert_eq!(fields, vec![("login_key".to_string(), "abc".to_string())]);
    }

    #[test]
    fn first_form_prefers_the_sp_login_data_form() {
        let html = r#"<form action="/sso/usr/other"><input name="a" value="1"></form>
            <form action="/passni/sso/spLoginData.php"><input name="b" value="2"></form>"#;
        let (action, fields) = first_form(html, &base()).unwrap().unwrap();
        assert_eq!(action.path(), "/passni/sso/spLoginData.php");
        assert_eq!(fields, vec![("b".to_string(), "2".to_string())]);
    }

    #[test]
    fn first_form_returns_none_without_an_action() {
        assert!(
            first_form("<form><input name=\"a\"></form>", &base())
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn required_inputs_reads_a_valueless_input_as_empty() {
        let html = r#"<input type="hidden" name="agt_id" value="plms">
            <input type="hidden" name="agt_r">"#;
        let found = required_inputs(html, &["agt_id", "agt_r"]).unwrap();
        assert_eq!(
            found,
            vec![
                ("agt_id".to_string(), "plms".to_string()),
                ("agt_r".to_string(), String::new()),
            ]
        );
    }

    #[test]
    fn required_inputs_reports_a_genuinely_missing_input() {
        let error =
            required_inputs("<input name=\"agt_id\" value=\"plms\">", &["agt_url"]).unwrap_err();
        assert!(matches!(error, PlmsError::MissingInput("agt_url")));
    }

    #[test]
    fn required_inputs_unescapes_entities() {
        let html = r#"<input name="agt_url" value="https://plms.postech.ac.kr/?a=1&amp;b=2">"#;
        let found = required_inputs(html, &["agt_url"]).unwrap();
        assert_eq!(found[0].1, "https://plms.postech.ac.kr/?a=1&b=2");
    }

    #[test]
    fn required_input_reads_a_single_value() {
        let html = r#"<input type="hidden" name="login_key" value="k-123">"#;
        assert_eq!(required_input(html, "login_key").unwrap(), "k-123");
    }
}
