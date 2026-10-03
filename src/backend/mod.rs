mod attachments;
pub mod lms;
pub mod mail;

use async_trait::async_trait;
use attachments::Attachments;
use lms_helper::content::{ContentCache, ContentConfig};
use rmcp::model::CallToolResult;
use serde_json::{Value, json};
use std::{path::PathBuf, time::Duration};
use tokio::sync::Mutex;

#[async_trait]
pub trait Backend: Send + Sync {
    async fn call(&self, name: &str, args: Value) -> Result<CallToolResult, String>;
}

pub struct LiveBackend {
    lms: Mutex<lms::LmsBackend>,
    mail: Mutex<mail::MailBackend>,
    attachments: Mutex<Attachments>,
    cache_dir: PathBuf,
}
impl LiveBackend {
    pub fn new(cache_dir: PathBuf) -> Self {
        Self {
            lms: Mutex::new(lms::LmsBackend::default()),
            mail: Mutex::new(mail::MailBackend::default()),
            attachments: Mutex::new(Attachments::default()),
            cache_dir,
        }
    }
    fn cache(&self) -> ContentCache {
        ContentCache::new(ContentConfig {
            cache_dir: self.cache_dir.clone(),
            max_file_bytes: 20 * 1024 * 1024,
            max_total_bytes: 50 * 1024 * 1024,
        })
    }
    async fn run(&self, name: &str, args: Value) -> Result<CallToolResult, String> {
        match name {
            "lms_read" | "mail_read" => {
                let mut cache = self.cache();
                let content = if name == "lms_read" {
                    self.lms
                        .lock()
                        .await
                        .read_content(args["url"].as_str().ok_or("url is required")?, &mut cache)
                        .await?
                } else {
                    self.mail
                        .lock()
                        .await
                        .read_content(args["id"].as_str().ok_or("id is required")?, &mut cache)
                        .await?
                };
                Ok(CallToolResult::structured(
                    self.attachments.lock().await.register(content),
                ))
            }
            "attachment_read" => self.attachments.lock().await.read(&args),
            "mail_list" => Ok(CallToolResult::structured(stamp(
                self.mail.lock().await.list(&args).await?,
            ))),
            name if name.starts_with("lms_") => Ok(CallToolResult::structured(stamp(
                self.lms.lock().await.call(name, &args).await?,
            ))),
            _ => Err("Unknown tool".into()),
        }
    }
}
#[async_trait]
impl Backend for LiveBackend {
    async fn call(&self, name: &str, args: Value) -> Result<CallToolResult, String> {
        tokio::time::timeout(Duration::from_secs(180), self.run(name, args))
            .await
            .map_err(|_| {
                "Request timed out; results are not cached as a success. Retry later.".to_string()
            })?
    }
}

pub fn error_result(message: String) -> CallToolResult {
    CallToolResult::structured_error(json!({"error": message}))
}

fn stamp(mut value: Value) -> Value {
    if let Some(object) = value.as_object_mut() {
        object.insert("fetched_at".into(), json!(lms_helper::time::now_unix()));
        object.insert("untrusted_source".into(), json!(true));
    }
    value
}
