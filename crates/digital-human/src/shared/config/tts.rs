use serde::Deserialize;

// ── TtsConfig ──

#[derive(Debug, Clone, Deserialize)]
pub struct TtsConfig {
    /// 是否启用数字人对话语音接口；默认关闭以保持未配置部署的启动兼容性。
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_tts_provider")]
    pub provider: String,
    #[serde(default = "default_tts_base_url")]
    pub base_url: String,
    #[serde(default)]
    pub api_key: String,
    #[serde(default)]
    pub resource_id: String,
    #[serde(default = "default_tts_model")]
    pub model: String,
    #[serde(default = "default_tts_default_voice")]
    pub default_voice: String,
    #[serde(default = "default_tts_default_encoding")]
    pub default_encoding: String,
    #[serde(default = "default_tts_timeout_secs")]
    pub timeout_secs: u64,
    #[serde(default = "default_tts_sample_rate")]
    pub sample_rate: u32,
    /// 数字人对话语音文件的独立保存目录；不得指向 QQBot 的 TTS 目录。
    #[serde(default = "default_tts_output_dir")]
    pub output_dir: String,
    /// 对外暴露的数字人服务基地址，用于生成带签名的下载 URL。
    #[serde(default = "default_tts_public_url_base")]
    pub public_url_base: String,
    /// HMAC 签名密钥；仅在启用对话语音接口时必填。
    #[serde(default)]
    pub url_signing_key: String,
    #[serde(default = "default_tts_url_expiry_secs")]
    pub url_expiry_secs: u64,
    #[serde(default = "default_tts_audio_retention_secs")]
    pub audio_retention_secs: u64,
    #[serde(default = "default_tts_audio_cleanup_interval_secs")]
    pub audio_cleanup_interval_secs: u64,
    #[serde(default = "default_tts_ffmpeg_path")]
    pub ffmpeg_path: String,
    /// 限制可被 API 指定的音色；空集合仅允许 default_voice。
    #[serde(default)]
    pub allowed_voices: Vec<String>,
}

impl Default for TtsConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            provider: default_tts_provider(),
            base_url: default_tts_base_url(),
            api_key: String::new(),
            resource_id: String::new(),
            model: default_tts_model(),
            default_voice: default_tts_default_voice(),
            default_encoding: default_tts_default_encoding(),
            timeout_secs: default_tts_timeout_secs(),
            sample_rate: default_tts_sample_rate(),
            output_dir: default_tts_output_dir(),
            public_url_base: default_tts_public_url_base(),
            url_signing_key: String::new(),
            url_expiry_secs: default_tts_url_expiry_secs(),
            audio_retention_secs: default_tts_audio_retention_secs(),
            audio_cleanup_interval_secs: default_tts_audio_cleanup_interval_secs(),
            ffmpeg_path: default_tts_ffmpeg_path(),
            allowed_voices: Vec::new(),
        }
    }
}

fn default_tts_provider() -> String {
    "volcengine".into()
}
fn default_tts_base_url() -> String {
    "https://openspeech.bytedance.com/api/v3/tts/unidirectional".into()
}
fn default_tts_model() -> String {
    "seed-tts-2.0-standard".into()
}
fn default_tts_default_voice() -> String {
    "zh_female_wanwanxiaohe_moon_bigtts".into()
}
fn default_tts_default_encoding() -> String {
    "wav".into()
}
fn default_tts_timeout_secs() -> u64 {
    30
}
fn default_tts_sample_rate() -> u32 {
    24000
}
fn default_tts_output_dir() -> String {
    "data/digital-human-tts".into()
}
fn default_tts_public_url_base() -> String {
    "http://127.0.0.1:8080".into()
}
fn default_tts_url_expiry_secs() -> u64 {
    300
}
fn default_tts_audio_retention_secs() -> u64 {
    3600
}
fn default_tts_audio_cleanup_interval_secs() -> u64 {
    300
}
fn default_tts_ffmpeg_path() -> String {
    "ffmpeg".into()
}
