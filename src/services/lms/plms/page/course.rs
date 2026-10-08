//! The "과목공지" (course notices) block PLMS renders on every course page.

use std::sync::LazyLock;

use regex::Regex;

use super::{text_block, text_of, unescape};

static ITEM: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?is)<li[^>]*class="[^"]*announcement-list-item[^"]*"[^>]*>(.*?)</li>"#).unwrap()
});
static HREF: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"(?i)\bhref="([^"]+)""#).unwrap());
static SUBJECT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?is)<div[^>]*class="[^"]*subject[^"]*"[^>]*\btitle="([^"]*)""#).unwrap()
});
static DATE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?is)<div[^>]*class="[^"]*date[^"]*"[^>]*>(.*?)</div>"#).unwrap()
});
static BOARD: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?is)<div[^>]*class="[^"]*course-announcement-header[^"]*"[^>]*>.*?<a[^>]*\bhref="([^"]+)"[^>]*class="[^"]*btn-more"#).unwrap()
});

static SUBJECT_HEADING: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?is)<div[^>]*class="[^"]*article-subject[^"]*"[^>]*>(.*?)</div>"#).unwrap()
});
static INFO: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?is)<div[^>]*class="[^"]*article-info[^"]*"[^>]*>(.*?)</div>\s*</div>"#).unwrap()
});
static INFO_FIELD: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?is)<strong>\s*([^<]+?)\s*</strong>\s*:\s*([^<]*)").unwrap());
static CONTENT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?is)<div[^>]*class="[^"]*article-content[^"]*"[^>]*>(.*?)<div[^>]*class="[^"]*(?:article-files|article-buttons|article-lists)"#).unwrap()
});
static FILES: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?is)<div[^>]*class="[^"]*article-files[^"]*"[^>]*>(.*?)</ul>"#).unwrap()
});
static FILE_LINK: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?is)<a\b[^>]*\bhref="([^"]+)"[^>]*>(.*?)</a>"#).unwrap());

/// One posting as the block lists it, before it is tied to its course.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AnnouncementRow {
    pub(crate) title: String,
    pub(crate) date: String,
    pub(crate) url: String,
}

/// The postings the course page shows, newest first as PLMS orders them. The
/// block is a preview: it holds the latest few, not the whole board.
pub(crate) fn course_announcements(html: &str) -> Vec<AnnouncementRow> {
    ITEM.captures_iter(html)
        .filter_map(|item| {
            let body = item.get(1)?.as_str();
            // The subject's `title` attribute carries the full text; the element
            // below it is truncated for the narrow block.
            let title = SUBJECT.captures(body).map(|found| unescape(&found[1]))?;
            Some(AnnouncementRow {
                title,
                date: DATE
                    .captures(body)
                    .map(|found| text_of(&found[1]))
                    .unwrap_or_default(),
                url: HREF
                    .captures(body)
                    .map(|found| unescape(&found[1]))
                    .unwrap_or_default(),
            })
        })
        .collect()
}

/// A posting's own page: what it says, who wrote it, and what it carries.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct ArticlePage {
    pub(crate) title: String,
    pub(crate) author: String,
    pub(crate) posted: String,
    pub(crate) body: String,
    pub(crate) attachments: Vec<(String, String)>,
}

/// Reads a `ubboard` article page.
pub(crate) fn article(html: &str) -> ArticlePage {
    let info = INFO.captures(html).map(|found| found[1].to_owned());
    let fields = info.as_deref().map(info_fields).unwrap_or_default();
    ArticlePage {
        title: SUBJECT_HEADING
            .captures(html)
            .map(|found| text_of(&found[1]))
            .unwrap_or_default(),
        author: field(&fields, "작성자"),
        posted: field(&fields, "작성일"),
        body: CONTENT
            .captures(html)
            .map(|found| text_block(&found[1]))
            .unwrap_or_default(),
        attachments: attachments(html),
    }
}

/// The `<strong>label</strong> : value` pairs of the article's info row.
fn info_fields(html: &str) -> Vec<(String, String)> {
    INFO_FIELD
        .captures_iter(html)
        .map(|found| (text_of(&found[1]), text_of(&found[2])))
        .collect()
}

fn field(fields: &[(String, String)], label: &str) -> String {
    fields
        .iter()
        .find(|(name, _)| name == label)
        .map(|(_, value)| value.clone())
        .unwrap_or_default()
}

/// Attached files as `(name, url)`, empty when the posting carries none.
fn attachments(html: &str) -> Vec<(String, String)> {
    let Some(block) = FILES.captures(html) else {
        return Vec::new();
    };
    FILE_LINK
        .captures_iter(&block[1])
        .map(|found| (text_of(&found[2]), unescape(&found[1])))
        .filter(|(name, _)| !name.is_empty())
        .collect()
}

/// The board the block links to with its "more" button, for the full list.
pub(crate) fn announcement_board(html: &str) -> Option<String> {
    BOARD.captures(html).map(|found| unescape(&found[1]))
}

#[cfg(test)]
mod tests {
    use super::*;

    const BLOCK: &str = r#"
    <div class="course-announcement row no-gutters">
      <div class="course-announcement-header">
        <h5>과목공지</h5>
        <div class="actions">
          <a href="https://plms.postech.ac.kr/mod/ubboard/view.php?id=193604" class="btn btn-default btn-more">
            <i class="fa fa-plus"></i>
          </a>
        </div>
      </div>
      <div class="course-announcement-body">
        <ul class="announcement-list">
          <li class="announcement-list-item">
            <a href="https://plms.postech.ac.kr/mod/ubboard/article.php?id=193604&amp;bwid=89200">
              <div class="subject" title="[HW 3] Grading Completed">[HW 3] Grading...</div>
              <div class="date">2025-12-22</div>
            </a>
          </li>
          <li class="announcement-list-item">
            <a href="https://plms.postech.ac.kr/mod/ubboard/article.php?id=193604&amp;bwid=89081">
              <div class="subject" title="[Exam 3] Grading Completed">[Exam 3] Grading...</div>
              <div class="date">2025-12-18</div>
            </a>
          </li>
        </ul>
      </div>
    </div>"#;

    #[test]
    fn the_block_yields_each_posting_in_order() {
        let found = course_announcements(BLOCK);
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].title, "[HW 3] Grading Completed");
        assert_eq!(found[0].date, "2025-12-22");
        assert_eq!(
            found[0].url,
            "https://plms.postech.ac.kr/mod/ubboard/article.php?id=193604&bwid=89200"
        );
        assert_eq!(found[1].title, "[Exam 3] Grading Completed");
    }

    #[test]
    fn the_full_title_comes_from_the_attribute_not_the_truncated_text() {
        let found = course_announcements(BLOCK);
        assert!(!found[0].title.ends_with("..."));
    }

    #[test]
    fn the_more_button_points_at_the_board() {
        assert_eq!(
            announcement_board(BLOCK).as_deref(),
            Some("https://plms.postech.ac.kr/mod/ubboard/view.php?id=193604")
        );
    }

    const ARTICLE: &str = r#"
    <div class="article-subject"><h3>Important Notifications</h3></div>
    <div class="article-info">
      <div class="row w-100 mx-0">
        <div class="col-info"><strong>작성자</strong> : 차선호 </div>
        <div class="col-info"><strong>작성일</strong> : 2026-09-02 23:34:15 </div>
        <div class="col-info"><strong>조회수</strong> : 31 </div>
      </div>
    </div>
    <div class="article-content">
      <div class="text_to_html"><p dir="ltr">Hello.</p><br /><ol><li><p>Register for Slack</p></li></ol></div>
    </div>
    <div class="article-files">
      <div class="d-inline-block align-top"><strong>첨부파일</strong></div>
      <div class="d-inline-block ml-2"><ul class="files">
        <li><a href="https://plms.postech.ac.kr/pluginfile.php/1/mod_ubboard/attachment/1/a.png?forcedownload=1"><img src="x" alt="" /> 유고결석.png </a></li>
      </ul></div>
    </div>
    <div class="lists article-lists"></div>"#;

    #[test]
    fn an_article_yields_its_title_author_and_date() {
        let found = article(ARTICLE);
        assert_eq!(found.title, "Important Notifications");
        assert_eq!(found.author, "차선호");
        assert_eq!(found.posted, "2026-09-02 23:34:15");
    }

    #[test]
    fn an_articles_body_stops_before_the_page_furniture() {
        let found = article(ARTICLE);
        assert_eq!(found.body, "Hello.\n\nRegister for Slack");
        assert!(!found.body.contains("첨부파일"));
    }

    #[test]
    fn an_articles_attachments_come_back_named() {
        let found = article(ARTICLE);
        assert_eq!(found.attachments.len(), 1);
        assert_eq!(found.attachments[0].0, "유고결석.png");
        assert!(found.attachments[0].1.ends_with("a.png?forcedownload=1"));
    }

    #[test]
    fn an_article_without_attachments_reports_none() {
        let plain = ARTICLE
            .split("<div class=\"article-files\">")
            .next()
            .unwrap();
        assert!(article(plain).attachments.is_empty());
    }

    #[test]
    fn a_course_page_without_the_block_yields_nothing() {
        assert!(course_announcements("<html><body>no block</body></html>").is_empty());
        assert_eq!(announcement_board("<html></html>"), None);
    }
}
