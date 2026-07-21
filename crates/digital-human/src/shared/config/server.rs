use serde::Deserialize;

// ── ServerConfig ──

#[derive(Debug, Clone, Deserialize)]
pub struct ServerConfig {
    #[serde(default = "default_host")]
    pub host: String,
    #[serde(default = "default_port")]
    pub port: u16,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            host: default_host(),
            port: default_port(),
        }
    }
}

fn default_host() -> String {
    "0.0.0.0".into()
}
fn default_port() -> u16 {
    8080
}

// ── DatabaseConfig ──

#[derive(Debug, Clone, Deserialize)]
pub struct DatabaseConfig {
    pub url: String,
    #[serde(default = "default_db_max_conn")]
    pub max_connections: u32,
    #[serde(default)]
    pub tunnel: Option<String>,
}

impl Default for DatabaseConfig {
    fn default() -> Self {
        Self {
            url: String::new(),
            max_connections: default_db_max_conn(),
            tunnel: None,
        }
    }
}

fn default_db_max_conn() -> u32 {
    10
}

// ── SessionConfig ──

#[derive(Debug, Clone, Deserialize)]
pub struct SessionConfig {
    #[serde(default = "default_session_timeout")]
    pub timeout_seconds: u64,
    #[serde(default = "default_cleanup_interval")]
    pub cleanup_interval_secs: u64,
    #[serde(default)]
    pub cleanup_interval_ms: Option<u64>,
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            timeout_seconds: default_session_timeout(),
            cleanup_interval_secs: default_cleanup_interval(),
            cleanup_interval_ms: None,
        }
    }
}

impl SessionConfig {
    pub fn cleanup_interval_seconds(&self) -> u64 {
        self.cleanup_interval_ms
            .map(|ms| (ms / 1000).max(1))
            .unwrap_or(self.cleanup_interval_secs)
    }
}

fn default_session_timeout() -> u64 {
    1800
}
fn default_cleanup_interval() -> u64 {
    300
}
