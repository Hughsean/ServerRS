use serde::Deserialize;

// ── JwtConfig ──

#[derive(Debug, Clone, Deserialize)]
pub struct JwtConfig {
    #[serde(default = "default_jwt_secret")]
    pub secret: String,
    #[serde(default = "default_access_ttl", alias = "expiration_secs")]
    pub access_ttl_secs: u64,
    #[serde(default = "default_refresh_ttl")]
    pub refresh_ttl_secs: u64,
}

impl Default for JwtConfig {
    fn default() -> Self {
        Self {
            secret: default_jwt_secret(),
            access_ttl_secs: default_access_ttl(),
            refresh_ttl_secs: default_refresh_ttl(),
        }
    }
}

fn default_jwt_secret() -> String {
    "CHANGE_ME_USE_A_LONG_RANDOM_SECRET".into()
}
fn default_access_ttl() -> u64 {
    900
}
fn default_refresh_ttl() -> u64 {
    2_592_000
}

// ── AuthConfig ──

#[derive(Debug, Clone, Deserialize)]
pub struct AuthConfig {
    #[serde(default = "default_max_login_attempts")]
    pub max_login_attempts: u32,
    #[serde(default = "default_lockout_duration")]
    pub lockout_duration_secs: u64,
}

impl Default for AuthConfig {
    fn default() -> Self {
        Self {
            max_login_attempts: default_max_login_attempts(),
            lockout_duration_secs: default_lockout_duration(),
        }
    }
}

fn default_max_login_attempts() -> u32 {
    5
}
fn default_lockout_duration() -> u64 {
    900
}

// ── StorageConfig ──

#[derive(Debug, Clone, Deserialize)]
pub struct StorageConfig {
    #[serde(default = "default_storage_backend")]
    pub backend: String,
    #[serde(default = "default_storage_base_path")]
    pub base_path: String,
    #[serde(default = "default_storage_base_url")]
    pub base_url: String,
    #[serde(default = "default_max_avatar_bytes")]
    pub max_avatar_bytes: u64,
    #[serde(default = "default_max_image_bytes")]
    pub max_image_bytes: u64,
    #[serde(default = "default_max_audio_bytes")]
    pub max_audio_bytes: u64,
    #[serde(default = "default_max_document_bytes")]
    pub max_document_bytes: u64,
    #[serde(default = "default_max_video_bytes")]
    pub max_video_bytes: u64,
}

impl Default for StorageConfig {
    fn default() -> Self {
        Self {
            backend: default_storage_backend(),
            base_path: default_storage_base_path(),
            base_url: default_storage_base_url(),
            max_avatar_bytes: default_max_avatar_bytes(),
            max_image_bytes: default_max_image_bytes(),
            max_audio_bytes: default_max_audio_bytes(),
            max_document_bytes: default_max_document_bytes(),
            max_video_bytes: default_max_video_bytes(),
        }
    }
}

fn default_storage_backend() -> String {
    "LOCAL".into()
}
fn default_storage_base_path() -> String {
    "./uploads".into()
}
fn default_storage_base_url() -> String {
    "http://localhost:8080/files".into()
}
fn default_max_avatar_bytes() -> u64 {
    2 * 1024 * 1024
}
fn default_max_image_bytes() -> u64 {
    10 * 1024 * 1024
}
fn default_max_audio_bytes() -> u64 {
    50 * 1024 * 1024
}
fn default_max_document_bytes() -> u64 {
    20 * 1024 * 1024
}
fn default_max_video_bytes() -> u64 {
    200 * 1024 * 1024
}
