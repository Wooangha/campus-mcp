//! Scraping the handoff pages. PLMS and SSO serve markup a browser would act on
//! without a user: hidden forms that auto-submit, and scripts that navigate.
//! These are targeted regexes, not a parser — the pages are small and fixed.

mod course;
mod form;
mod script;

use std::sync::LazyLock;

use regex::Regex;

pub(crate) use course::{
    AnnouncementRow, ArticlePage, announcement_board, article, course_announcements,
};
pub(crate) use form::{FormPost, first_form, required_input, required_inputs};
pub(crate) use script::{moodle_sesskey, passni_error_code, script_redirects};

static TAG: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)<[^>]*>").unwrap());
/// Tags that end a line of prose when a fragment is read as text.
static BREAK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?is)<br\s*/?>|</(?:p|div|li|tr|h[1-6]|ol|ul|blockquote|table)\s*>").unwrap()
});
/// A link, so its target can be kept when it differs from the text shown.
static LINK: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?is)<a\b[^>]*\bhref="([^"]*)"[^>]*>(.*?)</a>"#).unwrap());
/// A list item whose text is wrapped in a paragraph, which would otherwise
/// space the items apart as if each were a paragraph of its own.
static LIST_ITEM_PARAGRAPH: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?is)<li([^>]*)>\s*<p[^>]*>(.*?)</p>\s*</li>").unwrap());
/// Three or more newlines, which a blank line between blocks does not need.
static BLANK_RUN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\n{3,}").unwrap());

/// The visible text of a small HTML fragment, with tags dropped, entities
/// resolved and runs of whitespace collapsed. Tags close up rather than
/// becoming spaces, so `<a>...</a>, 23:59` keeps its punctuation tight.
pub(crate) fn text_of(html: &str) -> String {
    unescape(&TAG.replace_all(html, ""))
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// A block of HTML prose as readable text: line breaks where the markup broke
/// lines, and a link's target kept alongside its text when the two differ.
pub(crate) fn text_block(html: &str) -> String {
    let linked = LINK.replace_all(html, |found: &regex::Captures| {
        let href = &found[1];
        let shown = text_of(&found[2]);
        // Generated angle delimiters must survive the later HTML-tag removal.
        let target = href
            .replace('<', "%3C")
            .replace('>', "%3E")
            .replace("&lt;", "%3C")
            .replace("&gt;", "%3E");
        let link = if shown.is_empty() || shown == unescape(href) {
            format!("&lt;{target}&gt;")
        } else {
            let label = shown
                .replace('\\', "\\\\")
                .replace('[', "\\[")
                .replace(']', "\\]");
            format!("[{label}](&lt;{target}&gt;)")
        };
        format!(" {link} ")
    });
    let unwrapped = LIST_ITEM_PARAGRAPH.replace_all(&linked, "<li$1>$2</li>");
    let broken = BREAK.replace_all(&unwrapped, "\n");
    let text = unescape(&TAG.replace_all(&broken, ""));
    let lines: Vec<String> = text
        .lines()
        .map(|line| line.split_whitespace().collect::<Vec<_>>().join(" "))
        .collect();
    BLANK_RUN
        .replace_all(&lines.join("\n"), "\n\n")
        .trim()
        .to_owned()
}

/// The entities these pages actually use. `&amp;` is resolved last so that an
/// escaped entity such as `&amp;lt;` survives as text rather than becoming `<`.
fn unescape(value: &str) -> String {
    value
        .replace("&quot;", "\"")
        .replace("&#34;", "\"")
        .replace("&#034;", "\"")
        .replace("&#x27;", "'")
        .replace("&#39;", "'")
        .replace("&#039;", "'")
        .replace("&apos;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&nbsp;", " ")
        .replace("&raquo;", "\u{bb}")
        .replace("&amp;", "&")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_of_reads_moodles_formatted_time() {
        let html = r#"<a href="/calendar/view.php?view=day&amp;time=1">2026년 9월 17일(목요일)</a>, 23:59"#;
        assert_eq!(text_of(html), "2026년 9월 17일(목요일), 23:59");
    }

    #[test]
    fn text_block_keeps_the_shape_of_the_prose() {
        let html = "<p>First line.</p><br /><ol><li>One</li><li>Two</li></ol><p>Last.</p>";
        assert_eq!(text_block(html), "First line.\n\nOne\nTwo\n\nLast.");
    }

    #[test]
    fn text_block_keeps_list_items_together() {
        let html = "<p>Steps:</p><ol><li><p>One</p></li><li><p>Two</p></li></ol><p>Done.</p>";
        // The list rides directly under its lead-in line; only the closing
        // </ol> opens a blank line before the prose that follows.
        assert_eq!(text_block(html), "Steps:\nOne\nTwo\n\nDone.");
    }

    #[test]
    fn text_block_keeps_a_links_target_when_the_text_differs() {
        let html = r#"<p>Join <a href="https://example.com/x">here</a>.</p>"#;
        assert_eq!(text_block(html), "Join [here](<https://example.com/x>) .");
    }

    #[test]
    fn text_block_does_not_repeat_a_link_that_shows_its_own_url() {
        let html = r#"<p><a href="https://example.com/x">https://example.com/x</a></p>"#;
        assert_eq!(text_block(html), "<https://example.com/x>");
    }

    #[test]
    fn numeric_apostrophes_are_resolved() {
        assert_eq!(text_of("Today&#039;s Class"), "Today's Class");
        assert_eq!(text_of("Today&#39;s Class"), "Today's Class");
        assert_eq!(text_of("Today&#x27;s Class"), "Today's Class");
    }

    #[test]
    fn text_of_keeps_an_escaped_entity_as_text() {
        assert_eq!(text_of("&amp;lt;b&amp;gt;"), "&lt;b&gt;");
    }
    #[test]
    fn posting_links_keep_a_boundary_before_following_hangul() {
        let html = r#"참고<a href="https://example.com/a_(b)?x=1&amp;y=2">원문</a>다음내용"#;
        assert_eq!(
            text_block(html),
            "참고 [원문](<https://example.com/a_(b)?x=1&y=2>) 다음내용"
        );
        let html = r#"<a href="https://example.com/path?q=a%29b">https://example.com/path?q=a%29b</a>다음"#;
        assert_eq!(text_block(html), "<https://example.com/path?q=a%29b> 다음");
    }
    #[test]
    fn posting_link_delimiters_preserve_unbalanced_url_parentheses() {
        let html = r#"<a href="https://example.com/?q=one)two&amp;v=1">안내</a>다음"#;
        assert_eq!(
            text_block(html),
            "[안내](<https://example.com/?q=one)two&v=1>) 다음"
        );
    }
}
