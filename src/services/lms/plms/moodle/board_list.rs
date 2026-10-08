//! PLMS ubboard list rows and bounded pagination. URLs are reconstructed from
//! numeric identifiers, so list query controls never reach content readers.
use crate::services::lms::plms::{PlmsError, http::PLMS, page::AnnouncementRow};
use scraper::{Html, Selector};
use url::Url;

pub(super) struct BoardPage {
    pub rows: Vec<AnnouncementRow>,
    pub next: Option<Url>,
}

pub(super) fn board_url(base: &Url, href: &str) -> Option<Url> {
    let url = base.join(href).ok()?;
    if url.origin().ascii_serialization() != PLMS
        || !url.username().is_empty()
        || url.password().is_some()
        || url.path() != "/mod/ubboard/view.php"
    {
        return None;
    }
    let id = number(&url, "id")?;
    let page = number(&url, "page").unwrap_or(1).max(1);
    Url::parse(&format!("{PLMS}/mod/ubboard/view.php?id={id}&page={page}")).ok()
}
fn number(url: &Url, key: &str) -> Option<u64> {
    url.query_pairs().find(|(k, _)| k == key)?.1.parse().ok()
}

pub(super) fn parse(html: &str, base: &Url) -> Result<BoardPage, PlmsError> {
    let doc = Html::parse_document(html);
    let select = |s: &str| Selector::parse(s).unwrap();
    let table = doc
        .select(&select("table.table-ubboard-list"))
        .next()
        .ok_or(PlmsError::InvalidMoodleResponse)?;
    let board_id = number(base, "id").ok_or(PlmsError::InvalidMoodleResponse)?;
    let mut rows = Vec::new();
    for row in table.select(&select("tr")) {
        let Some(link) = row.select(&select("td.t-subject a[href]")).next() else {
            continue;
        };
        let Some(url) = link.attr("href").and_then(|h| base.join(h).ok()) else {
            continue;
        };
        if url.origin().ascii_serialization() != PLMS
            || url.path() != "/mod/ubboard/article.php"
            || number(&url, "id") != Some(board_id)
        {
            continue;
        }
        let Some(id) = number(&url, "bwid") else {
            continue;
        };
        let title = link
            .text()
            .collect::<String>()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        let date = row
            .select(&select("td.t-date"))
            .next()
            .map(|d| d.text().collect::<String>().trim().to_owned())
            .unwrap_or_default();
        rows.push(AnnouncementRow {
            title,
            date,
            url: format!("{PLMS}/mod/ubboard/article.php?id={board_id}&bwid={id}"),
        });
    }
    let current = number(base, "page").unwrap_or(1);
    let next = doc
        .select(&select("a[href]"))
        .filter_map(|a| board_url(base, a.attr("href")?))
        .filter(|u| {
            number(u, "id") == Some(board_id) && number(u, "page").is_some_and(|p| p > current)
        })
        .min_by_key(|u| number(u, "page"));
    Ok(BoardPage { rows, next })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn list_rows_remove_search_controls_and_reject_other_boards() {
        let base = Url::parse(&format!("{PLMS}/mod/ubboard/view.php?id=42&page=1")).unwrap();
        let page=parse(r#"<table class="table-ubboard-list"><tr><td class="t-subject"><a href="article.php?keyfield&amp;keyword&amp;ls=15&amp;page=1&amp;categoryid&amp;bwid=7&amp;id=42">A &amp; <b>B</b></a></td><td class="t-date">2026-09-02</td></tr><tr><td class="t-subject"><a href="article.php?id=43&amp;bwid=8">Wrong board</a></td></tr></table><a href="view.php?id=42&amp;page=3">3</a><a href="view.php?id=42&amp;page=2">Next</a><a href="https://evil.test/mod/ubboard/view.php?id=42&amp;page=2">Bad</a>"#,&base).unwrap();
        assert_eq!(page.rows.len(), 1);
        assert_eq!(page.rows[0].title, "A & B");
        assert_eq!(page.rows[0].date, "2026-09-02");
        assert_eq!(
            page.rows[0].url,
            format!("{PLMS}/mod/ubboard/article.php?id=42&bwid=7")
        );
        assert_eq!(number(&page.next.unwrap(), "page"), Some(2));
    }
    #[test]
    fn empty_list_is_distinct_from_login_or_changed_markup() {
        let base = Url::parse(&format!("{PLMS}/mod/ubboard/view.php?id=42")).unwrap();
        assert!(
            parse(
                "<table class='table-ubboard-list'><tr><td>None</td></tr></table>",
                &base
            )
            .unwrap()
            .rows
            .is_empty()
        );
        assert!(parse("<form>login</form>", &base).is_err());
        assert!(board_url(&base, "/login/logout.php?id=42").is_none());
    }
}
