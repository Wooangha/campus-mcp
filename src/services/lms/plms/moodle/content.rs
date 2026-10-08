//! Assignment instructions and announcement bodies, excluding student uploads.
use crate::services::{
    content::{Attachment, Content, ContentError, element_text},
    lms::plms::{PlmsClient, PlmsError},
};
use scraper::{Html, Selector};

impl PlmsClient {
    pub async fn read_content(&self, url: &str) -> Result<Content, PlmsError> {
        let response = self.session()?.content_get(url, false).await?;
        let body = crate::services::content::read_limited(response, 4 * 1024 * 1024).await?;
        let html = String::from_utf8_lossy(&body);
        if !html.contains("/login/logout.php") {
            return Err(ContentError::Authentication.into());
        }
        parse(&html, url).map_err(Into::into)
    }
    pub async fn download_attachment(&self, url: &str, max: usize) -> Result<Vec<u8>, PlmsError> {
        if max == 0 {
            return Err(ContentError::TooLarge.into());
        }
        let response = self.session()?.content_get(url, true).await?;
        Ok(crate::services::content::read_limited(response, max).await?)
    }
}
fn parse(html: &str, url: &str) -> Result<Content, ContentError> {
    let base = url::Url::parse(url).map_err(|_| ContentError::InvalidUrl)?;
    let doc = Html::parse_document(html);
    let assignment = base.path() == "/mod/assign/view.php";
    let body_selector = Selector::parse(if assignment {
        "#intro, .activity-description"
    } else {
        ".article-content"
    })
    .unwrap();
    let Some(body) = doc.select(&body_selector).next() else {
        return Err(ContentError::MissingBody);
    };
    let title_selector = Selector::parse(if assignment {
        "#region-main h2"
    } else {
        ".article-subject"
    })
    .unwrap();
    let title = doc
        .select(&title_selector)
        .next()
        .map(element_text)
        .unwrap_or_default();
    let mut content = Content {
        title,
        body: element_text(body),
        ..Content::default()
    };
    let file_selector=Selector::parse(if assignment {"#intro a[href], #intro img[src], .activity-description a[href], .activity-description img[src]"} else {".article-files a[href], .article-content a[href], .article-content img[src]"}).unwrap();
    for element in doc.select(&file_selector) {
        let inline = element.value().name() == "img";
        let attr = if inline { "src" } else { "href" };
        let Some(raw) = element.value().attr(attr) else {
            continue;
        };
        let Ok(url) = base.join(raw) else { continue };
        if !matches!(url.scheme(), "http" | "https") {
            continue;
        }
        if inline && (url.path().starts_with("/theme/") || url.path().starts_with("/pix/")) {
            continue;
        }
        if url.path().contains("/assignsubmission_") {
            continue;
        }
        if !inline
            && !url.path().contains("pluginfile.php")
            && !url.path().contains("/download.php")
        {
            continue;
        }
        let name = if inline {
            element.value().attr("alt").unwrap_or_default().to_owned()
        } else {
            element.text().collect::<String>().trim().to_owned()
        };
        let name = if name.trim().is_empty() {
            url.path_segments()
                .and_then(|mut p| p.next_back())
                .unwrap_or("attachment")
                .to_owned()
        } else {
            name
        };
        if !content.attachments.iter().any(|a| a.url == url.as_str()) {
            content
                .attachments
                .push(Attachment::remote(name, url.into(), inline));
        }
    }
    Ok(content)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn nested_instructions_and_images_are_read_without_student_uploads() {
        let html = r#"<section id="region-main"><h2>Project</h2><div id="intro"><p>Instructions</p><img src="/theme/image.php/core/f/pdf"><div>Nested details</div><a href="/pluginfile.php/1/mod_assign/introattachment/0/rules.pdf">rules.pdf</a><img src="/pluginfile.php/1/mod_assign/intro/image.png"></div><a href="/pluginfile.php/1/assignsubmission_file/submission_files/2/answer.zip">answer.zip</a></section>"#;
        let c = parse(html, "https://plms.postech.ac.kr/mod/assign/view.php?id=1").unwrap();
        assert!(c.body.contains("Nested details"));
        assert_eq!(c.attachments.len(), 2);
        assert!(!c.attachments.iter().any(|a| a.name.contains("answer")));
    }
    #[test]
    fn announcements_keep_body_and_file_list() {
        let html = r#"<div class="article-subject">Notice</div><div class="article-content">First<div>Second</div></div><div class="article-files"><a href="/pluginfile.php/1/a.pdf">a.pdf</a></div>"#;
        let c = parse(
            html,
            "https://plms.postech.ac.kr/mod/ubboard/article.php?id=1&bwid=2",
        )
        .unwrap();
        assert_eq!(c.body, "First\nSecond");
        assert_eq!(c.attachments.len(), 1);
        assert_eq!(c.attachments[0].name, "a.pdf");
        assert!(
            parse(
                "<html>login</html>",
                "https://plms.postech.ac.kr/mod/assign/view.php?id=1"
            )
            .is_err()
        );
    }
}
