//! Full text and attachment metadata, shared by LMS and mail readers.
mod cache;
mod html;
pub use cache::{ContentCache, ContentConfig};
pub use html::{element_text, html_text};

use std::path::PathBuf;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Content {
    pub title: String,
    pub body: String,
    pub attachments: Vec<Attachment>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attachment {
    pub name: String,
    /// Authenticated download endpoint, never a local file path.
    pub url: String,
    pub inline: bool,
    pub local_path: Option<PathBuf>,
    pub error: Option<String>,
}
impl Attachment {
    pub fn remote(name: String, url: String, inline: bool) -> Self {
        Self {
            name,
            url,
            inline,
            local_path: None,
            error: None,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ContentError {
    #[error("본문 영역을 찾지 못했습니다")]
    MissingBody,
    #[error("지원하지 않는 첨부 형식입니다")]
    Unsupported,
    #[error("허용되지 않은 다운로드 주소입니다")]
    InvalidUrl,
    #[error("다운로드 크기 제한을 초과했습니다")]
    TooLarge,
    #[error("인증이 필요하거나 파일 대신 로그인 페이지가 반환되었습니다")]
    Authentication,
    #[error("HTTP {0}")]
    HttpStatus(u16),
    #[error("다운로드 통신 실패")]
    Network,
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// Bound bytes while streaming, including servers that omit Content-Length.
pub async fn read_limited(
    mut response: reqwest::Response,
    max: usize,
) -> Result<Vec<u8>, ContentError> {
    if !response.status().is_success() {
        return Err(ContentError::HttpStatus(response.status().as_u16()));
    }
    if response
        .content_length()
        .is_some_and(|size| size > max as u64)
    {
        return Err(ContentError::TooLarge);
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| ContentError::Network)? {
        if chunk.len() > max.saturating_sub(body.len()) {
            return Err(ContentError::TooLarge);
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

/// Fetch LMS text and download each source attachment with a shared budget.
pub async fn read_lms(
    lms: &dyn crate::services::lms::Lms,
    url: &str,
    cache: &mut ContentCache,
) -> Result<Content, crate::services::lms::LmsError> {
    let mut content = lms.read_content(url).await?;
    for file in &mut content.attachments {
        if cache.limit() == 0 {
            file.error = Some(ContentError::TooLarge.to_string());
            continue;
        }
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        match lms.download_attachment(&file.url, cache.limit()).await {
            Ok(bytes) => match cache.save(&bytes, &file.name) {
                Ok(path) => file.local_path = Some(path),
                Err(error) => file.error = Some(error.to_string()),
            },
            Err(error) => file.error = Some(error.to_string()),
        }
    }
    Ok(content)
}
