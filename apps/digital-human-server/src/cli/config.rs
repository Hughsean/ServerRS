//! CLI 配置:服务端地址与 token 缓存路径。
//!
//! 服务端地址来源优先级:命令行 --url > 环境变量 SERVERRS_CLI_URL > 默认值。
//! token 缓存固定存于 ~/.serverrs-cli/token.json。

use std::path::PathBuf;

use crate::cli::error::CliError;

#[derive(Debug, Clone)]
pub struct CliConfig {
    /// 后端 API 基地址,如 http://127.0.0.1:8080
    pub base_url: String,
    /// token 缓存文件路径
    pub token_path: PathBuf,
}

impl CliConfig {
    /// 从命令行参数与环境变量加载配置。
    ///
    /// `url_arg` 为 --url 参数值(可选),环境变量 SERVERRS_CLI_URL 作为次选。
    pub fn load(url_arg: Option<String>) -> Result<Self, CliError> {
        let base_url = url_arg
            .or_else(|| std::env::var("SERVERRS_CLI_URL").ok())
            .unwrap_or_else(|| "http://127.0.0.1:8080".to_string());
        // 去掉末尾斜杠,避免拼接路径时出现双斜杠
        let base_url = base_url.trim_end_matches('/').to_string();

        let token_path = home_token_path()?;

        Ok(Self {
            base_url,
            token_path,
        })
    }
}

/// 返回 ~/.serverrs-cli/token.json 路径。
/// 若无法获取 home 目录则返回错误(启动阶段致命)。
fn home_token_path() -> Result<PathBuf, CliError> {
    let home = dirs_or_home()?;
    let dir = home.join(".serverrs-cli");
    Ok(dir.join("token.json"))
}

/// 跨平台获取用户 home 目录。
fn dirs_or_home() -> Result<PathBuf, CliError> {
    // Windows: %USERPROFILE%;Unix: $HOME
    #[cfg(windows)]
    {
        if let Ok(p) = std::env::var("USERPROFILE") {
            return Ok(PathBuf::from(p));
        }
    }
    #[cfg(not(windows))]
    {
        if let Ok(p) = std::env::var("HOME") {
            return Ok(PathBuf::from(p));
        }
    }
    Err(CliError::Io(std::io::Error::new(
        std::io::ErrorKind::NotFound,
        "无法确定用户 home 目录",
    )))
}

/// 确保 token 缓存目录存在(登录写缓存前调用)。
pub fn ensure_token_dir(config: &CliConfig) -> Result<(), CliError> {
    if let Some(parent) = config.token_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, OnceLock};

    // 这三个测试都读写同一个环境变量 SERVERRS_CLI_URL,必须串行执行,
    // 否则并行测试会互相读到对方设置的值。用全局 mutex 保证互斥。
    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    // edition 2024 中 env::set_var/remove_var 为 unsafe(多线程不安全)。
    // 测试在 env_lock 保护下串行运行,用 unsafe 块包裹。
    fn set_env(key: &str, val: &str) {
        unsafe {
            std::env::set_var(key, val);
        }
    }
    fn remove_env(key: &str) {
        unsafe {
            std::env::remove_var(key);
        }
    }

    #[test]
    fn load_uses_default_when_no_arg_no_env() {
        let _guard = env_lock().lock().unwrap();
        // 临时清除环境变量
        let old = std::env::var("SERVERRS_CLI_URL").ok();
        remove_env("SERVERRS_CLI_URL");

        let cfg = CliConfig::load(None).unwrap();
        assert_eq!(cfg.base_url, "http://127.0.0.1:8080");
        assert!(cfg.token_path.to_string_lossy().ends_with("token.json"));

        if let Some(v) = old {
            set_env("SERVERRS_CLI_URL", &v);
        }
    }

    #[test]
    fn load_strips_trailing_slash() {
        let _guard = env_lock().lock().unwrap();
        let old = std::env::var("SERVERRS_CLI_URL").ok();
        set_env("SERVERRS_CLI_URL", "http://localhost:9000/");
        let cfg = CliConfig::load(None).unwrap();
        assert_eq!(cfg.base_url, "http://localhost:9000");
        if let Some(v) = old {
            set_env("SERVERRS_CLI_URL", &v);
        } else {
            remove_env("SERVERRS_CLI_URL");
        }
    }

    #[test]
    fn url_arg_overrides_env() {
        let _guard = env_lock().lock().unwrap();
        let old = std::env::var("SERVERRS_CLI_URL").ok();
        set_env("SERVERRS_CLI_URL", "http://from-env:1234");
        let cfg = CliConfig::load(Some("http://from-arg:5678".into())).unwrap();
        assert_eq!(cfg.base_url, "http://from-arg:5678");
        if let Some(v) = old {
            set_env("SERVERRS_CLI_URL", &v);
        } else {
            remove_env("SERVERRS_CLI_URL");
        }
    }
}
