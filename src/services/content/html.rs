//! DOM-based extraction preserves text order without executing HTML.
use scraper::{ElementRef, Html};

pub fn html_text(html: &str) -> String {
    element_text(Html::parse_fragment(html).root_element())
}

pub fn element_text(element: ElementRef<'_>) -> String {
    fn walk(element: ElementRef<'_>, text: &mut String) {
        let name = element.value().name();
        if matches!(name, "script" | "style" | "noscript" | "head") {
            return;
        }
        let block = matches!(
            name,
            "p" | "div" | "li" | "tr" | "h1" | "h2" | "h3" | "h4" | "br" | "section"
        );
        if block {
            text.push('\n');
        }
        let href = (name == "a")
            .then(|| element.value().attr("href"))
            .flatten()
            .filter(|href| href.starts_with("https://") || href.starts_with("http://"));
        if href.is_some() {
            text.push(' ');
        }
        let label_start = text.len();
        for child in element.children() {
            if let Some(value) = child.value().as_text() {
                text.push_str(value);
            } else if let Some(child) = ElementRef::wrap(child) {
                walk(child, text);
            }
        }
        if let Some(href) = href {
            let shown = text[label_start..]
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ");
            text.truncate(label_start);
            let target = href.replace('<', "%3C").replace('>', "%3E");
            if shown.is_empty() || shown == href {
                text.push_str(&format!("<{target}>"));
            } else {
                let label = shown
                    .replace('\\', "\\\\")
                    .replace('[', "\\[")
                    .replace(']', "\\]");
                text.push_str(&format!("[{label}](<{target}>)"));
            }
            // HTML knows exactly where the URL ends. Retain that boundary
            // instead of trying to split punctuation or Hangul off later.
            text.push(' ');
        }
        if matches!(name, "td" | "th") {
            text.push(' ');
        }
        if block {
            text.push('\n');
        }
    }
    let mut text = String::new();
    walk(element, &mut text);
    text.lines()
        .map(|line| line.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn nested_text_entities_and_links_survive_without_scripts() {
        assert_eq!(
            html_text(
                "<div>첫째<div>둘째 &amp; 셋째</div><script>secret()</script><a href='https://example.com'>원문</a></div>"
            ),
            "첫째\n둘째 & 셋째\n[원문](<https://example.com>)"
        );
    }
    #[test]
    fn anchor_boundaries_separate_adjacent_prose_and_do_not_duplicate_raw_urls() {
        assert_eq!(
            html_text(r#"참고<a href="https://example.com/report">문서</a>를확인하세요"#),
            "참고 [문서](<https://example.com/report>) 를확인하세요"
        );
        assert_eq!(
            html_text(
                r#"<a href="https://example.com/report"><b>https://example.com/report</b></a>다음내용"#
            ),
            "<https://example.com/report> 다음내용"
        );
        assert_eq!(
            html_text(r#"<a href="https://example.com/report"></a>다음내용"#),
            "<https://example.com/report> 다음내용"
        );
    }
    #[test]
    fn legitimate_url_punctuation_and_query_are_preserved_exactly() {
        let href = "https://example.com/a_(b),c.;?q=(one,two)&lang=ko#한글";
        let html = r#"<a href="https://example.com/a_(b),c.;?q=(one,two)&amp;lang=ko#한글">링크</a>다음<a href="https://example.com/next?q=a%29b">https://example.com/next?q=a%29b</a>."#;
        assert_eq!(
            html_text(html),
            format!("[링크](<{href}>) 다음 <https://example.com/next?q=a%29b> .")
        );
    }
    #[test]
    fn explicit_destinations_preserve_unbalanced_parentheses_and_escape_labels() {
        assert_eq!(
            html_text(r#"<a href="https://example.com/?q=one)two">안내 [본문]</a>다음"#),
            "[안내 \\[본문\\]](<https://example.com/?q=one)two>) 다음"
        );
        assert_eq!(
            html_text(
                r#"<a href="https://example.com/?q=one)two">https://example.com/?q=one)two</a>다음"#
            ),
            "<https://example.com/?q=one)two> 다음"
        );
        assert_eq!(
            html_text(r#"<a href="https://example.com/?q=&lt;x&gt;">문서</a>"#),
            "[문서](<https://example.com/?q=%3Cx%3E>)"
        );
    }
}
