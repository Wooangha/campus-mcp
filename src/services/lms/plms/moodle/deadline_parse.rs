//! Explicit assignment dates only. PLMS renders unzoned dates in Korea time,
//! independently of the machine running the helper.
use chrono::{DateTime, FixedOffset, NaiveDate, NaiveDateTime, TimeZone};
use regex::Regex;
use scraper::{Html, Selector};
use std::sync::LazyLock;
static KOREAN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
    r"^(\d{4})년\s*(\d{1,2})월\s*(\d{1,2})일\s*(?:\([^)]*\))?\s*,?\s*(\d{1,2}):(\d{2})(?::(\d{2}))?$"
).unwrap()
});

pub(super) fn parse(cell_html: &str) -> Option<i64> {
    let cell = Html::parse_fragment(cell_html);
    for node in cell.select(&Selector::parse("[datetime]").unwrap()) {
        if let Some(parsed) = parse_text(node.attr("datetime")?) {
            return Some(parsed);
        }
    }
    parse_text(&cell.root_element().text().collect::<Vec<_>>().join(" "))
}
fn parse_text(text: &str) -> Option<i64> {
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if let Ok(date) = DateTime::parse_from_rfc3339(&text) {
        return Some(date.timestamp());
    }
    let korea = FixedOffset::east_opt(9 * 3600).unwrap();
    if let Some(parts) = KOREAN.captures(&text) {
        let year = parts[1].parse().ok()?;
        let month = parts[2].parse().ok()?;
        let day = parts[3].parse().ok()?;
        let hour = parts[4].parse().ok()?;
        let minute = parts[5].parse().ok()?;
        let second = parts
            .get(6)
            .map(|m| m.as_str().parse::<u32>())
            .transpose()
            .ok()?
            .unwrap_or(0);
        let naive = NaiveDate::from_ymd_opt(year, month, day)?.and_hms_opt(hour, minute, second)?;
        return korea
            .from_local_datetime(&naive)
            .single()
            .map(|d| d.timestamp());
    }
    for format in [
        "%Y-%m-%dT%H:%M:%S",
        "%Y-%m-%dT%H:%M",
        "%Y-%m-%d %H:%M:%S",
        "%Y-%m-%d %H:%M",
        "%A, %d %B %Y, %I:%M %p",
        "%A, %d %B %Y, %H:%M",
        "%d %B %Y, %I:%M %p",
        "%d %B %Y, %H:%M",
        "%A, %B %d, %Y, %I:%M %p",
        "%B %d, %Y, %I:%M %p",
    ] {
        if let Ok(naive) = NaiveDateTime::parse_from_str(&text, format) {
            return korea
                .from_local_datetime(&naive)
                .single()
                .map(|d| d.timestamp());
        }
    }
    None
}
#[cfg(test)]
mod tests {
    use super::*;
    fn expected() -> i64 {
        DateTime::parse_from_rfc3339("2026-10-20T23:59:00+09:00")
            .unwrap()
            .timestamp()
    }
    #[test]
    fn korean_english_and_iso_dates_use_explicit_korea_time() {
        for text in [
            "2026년 10월 20일(화요일), 23:59",
            "Tuesday, 20 October 2026, 11:59 PM",
            "Tuesday, 20 October 2026, 23:59",
            "20 October 2026, 11:59 pm",
            "2026-10-20 23:59",
            "2026-10-20T23:59:00",
        ] {
            assert_eq!(parse(text), Some(expected()), "{text}");
        }
    }
    #[test]
    fn datetime_attribute_has_priority_and_honors_its_offset() {
        assert_eq!(
            parse(r#"<time datetime="2026-10-20T14:59:00Z">unrecognized locale</time>"#),
            Some(expected())
        );
        assert_eq!(
            parse(r#"<time datetime="2026-10-20T23:59">2020-01-01 00:00</time>"#),
            Some(expected())
        );
    }
    #[test]
    fn no_invented_midnight_or_invalid_date() {
        for text in [
            "2026-10-20",
            "2026년 10월 20일",
            "Tuesday, 20 October 2026",
            "2026년 2월 30일, 23:59",
            "2026-10-20 25:00",
            "-",
            "No due date",
        ] {
            assert_eq!(parse(text), None, "{text}");
        }
    }
}
