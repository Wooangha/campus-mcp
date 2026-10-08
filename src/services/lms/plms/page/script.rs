//! The navigations a handoff page performs from script rather than from markup.

use std::sync::LazyLock;

use regex::Regex;
use url::Url;

use super::unescape;
use crate::services::lms::plms::PlmsError;

static SCRIPT_LOCATION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?i)(?:window\.|document\.)?location(?:\.href)?\s*=\s*["']([^"']+)["']"#).unwrap()
});
static SESSKEY: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#""sesskey"\s*:\s*"([A-Za-z0-9]+)""#).unwrap());
static ERR_CODE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?i)\berr_code\s*=\s*["']([^"']*)["']"#).unwrap());

/// Every `location = "..."` target in document order. PLMS hands back pages whose
/// script branches on an error code, so the caller decides which branch is live.
pub(crate) fn script_redirects(html: &str, base: &Url) -> Result<Vec<Url>, PlmsError> {
    SCRIPT_LOCATION
        .captures_iter(html)
        .map(|found| base.join(&unescape(&found[1])).map_err(Into::into))
        .collect()
}

/// Moodle's per-session key, embedded in the page's JS config blob. Every
/// service call has to quote it back.
pub(crate) fn moodle_sesskey(html: &str) -> Option<String> {
    SESSKEY.captures(html).map(|found| found[1].to_owned())
}

/// The `err_code` a PLMS handoff page carries. An empty string is the success
/// case: the page's own guard then skips its error branch.
pub(crate) fn passni_error_code(html: &str) -> Option<String> {
    ERR_CODE.captures(html).map(|found| unescape(&found[1]))
}

#[cfg(test)]
mod tests {
    use super::*;

    const HANDOFF: &str = r#"<script>
        function fSSOInitialize() {
            var err_code = '';
            if (err_code != '') {
                location.href = '/passni/coursemos/error.php' + '?errorCode=' + err_code;
            } else {
                document.location.href = '/passni/coursemos/loginBeforeProc.php';
            }
        }
        </script>"#;

    #[test]
    fn script_redirects_collects_both_branches_in_order() {
        let base = Url::parse("https://plms.postech.ac.kr/passni/sso/spLoginData.php").unwrap();
        let targets = script_redirects(HANDOFF, &base).unwrap();
        let paths: Vec<_> = targets.iter().map(|target| target.path()).collect();
        assert_eq!(
            paths,
            vec![
                "/passni/coursemos/error.php",
                "/passni/coursemos/loginBeforeProc.php"
            ]
        );
    }

    #[test]
    fn passni_error_code_is_empty_on_success() {
        assert_eq!(passni_error_code(HANDOFF).as_deref(), Some(""));
    }

    #[test]
    fn passni_error_code_reads_a_real_code() {
        assert_eq!(
            passni_error_code("var err_code = 'PN0012';").as_deref(),
            Some("PN0012")
        );
    }

    #[test]
    fn moodle_sesskey_is_read_from_the_page_config() {
        let html = r#"<script>M.cfg = {"wwwroot":"https:\/\/plms.postech.ac.kr","sesskey":"dKK0DKe8E5","theme":"coursemos"};</script>"#;
        assert_eq!(moodle_sesskey(html).as_deref(), Some("dKK0DKe8E5"));
    }

    #[test]
    fn moodle_sesskey_is_absent_when_signed_out() {
        assert_eq!(moodle_sesskey("<html></html>"), None);
    }

    #[test]
    fn passni_error_code_is_absent_on_an_ordinary_page() {
        assert_eq!(passni_error_code("<html><body>hi</body></html>"), None);
    }
}
