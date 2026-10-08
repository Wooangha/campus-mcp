//! Size-bounded attachment storage using content hashes, never remote filenames.
use super::ContentError;
use crate::services::config::{ConfigError, Vars, optional, required};
use std::{io::Write, path::PathBuf};

#[derive(Debug, Clone)]
pub struct ContentConfig {
    pub cache_dir: PathBuf,
    pub max_file_bytes: usize,
    pub max_total_bytes: usize,
}
impl ContentConfig {
    pub fn from_vars(vars: &impl Vars) -> Result<Self, ConfigError> {
        fn mb(vars: &impl Vars, key: &'static str, default: usize) -> Result<usize, ConfigError> {
            optional(vars, key)
                .unwrap_or_else(|| default.to_string())
                .parse::<usize>()
                .ok()
                .filter(|n| (1..=1024).contains(n))
                .map(|n| n * 1024 * 1024)
                .ok_or_else(|| ConfigError::Invalid {
                    var: key,
                    reason: "1~1024 사이의 MB 단위 정수가 필요합니다".into(),
                })
        }
        let cache_dir = match optional(vars, "CONTENT_CACHE_DIR") {
            Some(dir) => PathBuf::from(dir),
            None => PathBuf::from(required(vars, "HOME")?).join(".cache/campus-mcp/content"),
        };
        Ok(Self {
            cache_dir,
            max_file_bytes: mb(vars, "CONTENT_MAX_FILE_MB", 20)?,
            max_total_bytes: mb(vars, "CONTENT_MAX_TOTAL_MB", 100)?,
        })
    }
}
pub struct ContentCache {
    config: ContentConfig,
    remaining: usize,
}
impl ContentCache {
    pub fn new(config: ContentConfig) -> Self {
        let remaining = config.max_total_bytes;
        Self { config, remaining }
    }
    pub fn limit(&self) -> usize {
        self.config.max_file_bytes.min(self.remaining)
    }
    pub fn save(&mut self, bytes: &[u8], name: &str) -> Result<PathBuf, ContentError> {
        if bytes.len() > self.limit() {
            return Err(ContentError::TooLarge);
        }
        let hash = hex::encode(openssl::sha::sha256(bytes));
        let extension = std::path::Path::new(name)
            .extension()
            .and_then(|s| s.to_str())
            .filter(|s| s.len() <= 10 && s.chars().all(|c| c.is_ascii_alphanumeric()))
            .unwrap_or("bin");
        std::fs::create_dir_all(&self.config.cache_dir)?;
        let path = self.config.cache_dir.join(format!("{hash}.{extension}"));
        let mut file = tempfile::NamedTempFile::new_in(&self.config.cache_dir)?;
        file.write_all(bytes)?;
        file.as_file().sync_all()?;
        file.persist(&path).map_err(|e| e.error)?;
        self.remaining -= bytes.len();
        Ok(path)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn remote_names_cannot_escape_cache_and_total_budget_is_enforced() {
        let dir = tempfile::tempdir().unwrap();
        let mut cache = ContentCache::new(ContentConfig {
            cache_dir: dir.path().into(),
            max_file_bytes: 4,
            max_total_bytes: 5,
        });
        let path = cache.save(b"abc", "../../secret.txt").unwrap();
        assert_eq!(path.parent(), Some(dir.path()));
        assert_eq!(std::fs::read(&path).unwrap(), b"abc");
        assert_eq!(cache.limit(), 2);
        assert!(matches!(
            cache.save(b"abc", "x"),
            Err(ContentError::TooLarge)
        ));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }
}
