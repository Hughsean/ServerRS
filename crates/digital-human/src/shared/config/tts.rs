use serde::Deserialize;

// ── TtsConfig ──

#[derive(Debug, Clone, Deserialize)]
pub struct TtsConfig {
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
}

impl Default for TtsConfig {
    fn default() -> Self {
        Self {
            provider: default_tts_provider(),
            base_url: default_tts_base_url(),
            api_key: String::new(),
            resource_id: String::new(),
            model: default_tts_model(),
            default_voice: default_tts_default_voice(),
            default_encoding: default_tts_default_encoding(),
            timeout_secs: default_tts_timeout_secs(),
            sample_rate: default_tts_sample_rate(),
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
