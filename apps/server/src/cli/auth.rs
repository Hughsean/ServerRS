//! 认证:登录、token 缓存读写、自动刷新。
//!
//! token 缓存为 ~/.serverrs-cli/token.json,内含 access_token 与 refresh_token。
//! 损坏的缓存文件被当作无缓存处理,不崩溃。

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::cli::config::CliConfig;
use crate::cli::dto::{LoginResponse, RefreshResponse};
use crate::cli::error::CliError;

/// 持久化的 token 凭证。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenCache {
    pub access_token: String,
    pub refresh_token: String,
}

impl From<LoginResponse> for TokenCache {
    fn from(r: LoginResponse) -> Self {
        Self {
            access_token: r.access_token,
            refresh_token: r.refresh_token,
        }
    }
}

impl From<RefreshResponse> for TokenCache {
    fn from(r: RefreshResponse) -> Self {
        Self {
            access_token: r.access_token,
            refresh_token: r.refresh_token,
        }
    }
}

/// 写 token 缓存到文件。会创建目录,并尝试设置 0600 权限(Unix)。
pub fn save_token_cache(config: &CliConfig, cache: &TokenCache) -> Result<(), CliError> {
    crate::cli::config::ensure_token_dir(config)?;
    let json = serde_json::to_string_pretty(cache)?;
    let path = &config.token_path;
    std::fs::write(path, json)?;

    // Unix 下限制文件权限为 0600,防止其他用户读取 token。
    // Windows 无等效 chmod,跳过(依赖用户目录 ACL)。
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(path)?.permissions();
        perms.set_mode(0o600);
        std::fs::set_permissions(path, perms)?;
    }

    Ok(())
}

/// 读 token 缓存。文件不存在返回 None;损坏(非合法 JSON)也返回 None,
/// 而非报错 -- 当作无缓存走重新登录。
pub fn load_token_cache(config: &CliConfig) -> Result<Option<TokenCache>, CliError> {
    let path: &Path = &config.token_path;
    if !path.exists() {
        return Ok(None);
    }
    let content = std::fs::read_to_string(path)?;
    match serde_json::from_str::<TokenCache>(&content) {
        Ok(cache) => Ok(Some(cache)),
        Err(_) => {
            // 损坏的缓存:当作无缓存,不崩溃
            Ok(None)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn temp_config(dir: &Path) -> CliConfig {
        CliConfig {
            base_url: "http://x".into(),
            token_path: dir.join("token.json"),
        }
    }

    fn unique_id() -> String {
        // 用静态计数器避免并行测试冲突(不能用随机数/时间戳)
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        COUNTER.fetch_add(1, Ordering::SeqCst).to_string()
    }

    #[test]
    fn save_then_load_roundtrip() {
        let tmp = std::env::temp_dir().join(format!("cli-test-{}", unique_id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let cfg = temp_config(&tmp);
        let cache = TokenCache {
            access_token: "a".into(),
            refresh_token: "r".into(),
        };
        save_token_cache(&cfg, &cache).unwrap();
        let loaded = load_token_cache(&cfg).unwrap().unwrap();
        assert_eq!(loaded.access_token, "a");
        assert_eq!(loaded.refresh_token, "r");
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn load_returns_none_when_missing() {
        let tmp = std::env::temp_dir().join(format!("cli-test-none-{}", unique_id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let cfg = temp_config(&tmp);
        assert!(load_token_cache(&cfg).unwrap().is_none());
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn load_returns_none_when_corrupted() {
        let tmp = std::env::temp_dir().join(format!("cli-test-bad-{}", unique_id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let cfg = temp_config(&tmp);
        std::fs::write(&cfg.token_path, "not json {{{").unwrap();
        // 损坏文件应被当作无缓存,而非报错
        assert!(load_token_cache(&cfg).unwrap().is_none());
        std::fs::remove_dir_all(&tmp).ok();
    }
}
