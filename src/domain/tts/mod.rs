//! TTS（文字转语音）领域层模块
//!
//! 定义语音合成所需的接口和类型。目前支持火山引擎 TTS API，
//! 可扩展支持其他 TTS 提供商。

use async_trait::async_trait;

// ── 音频编码格式 ──────────────────────────────────────────────────

/// 支持的音频编码格式
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AudioFormat {
    Wav,
    Mp3,
    Pcm,
    OggOpus,
}

impl AudioFormat {
    /// 返回火山引擎 API 使用的字符串值
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Wav => "wav",
            Self::Mp3 => "mp3",
            Self::Pcm => "pcm",
            Self::OggOpus => "ogg_opus",
        }
    }

    /// 从字符串解析音频格式
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "wav" => Some(Self::Wav),
            "mp3" => Some(Self::Mp3),
            "pcm" => Some(Self::Pcm),
            "ogg_opus" | "ogg" => Some(Self::OggOpus),
            _ => None,
        }
    }
}

// ── 请求 / 响应 ─────────────────────────────────────────────────────

/// TTS 合成请求
#[derive(Debug, Clone)]
pub struct TtsRequest {
    /// 待合成的文本
    pub text: String,
    /// 音色 ID（如 "zh_female_qingxin"）
    pub voice: String,
    /// 期望的音频编码格式
    pub format: AudioFormat,
    /// 语速（0.5 ~ 2.0，1.0 为正常）
    pub speed_ratio: f64,
    /// 音量（0.5 ~ 2.0，1.0 为正常）
    pub volume_ratio: f64,
    /// 音调（0.5 ~ 2.0，1.0 为正常）
    pub pitch_ratio: f64,
}

impl TtsRequest {
    /// 使用纯文本和默认参数快速构建请求
    pub fn new(text: impl Into<String>, voice: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            voice: voice.into(),
            format: AudioFormat::Wav,
            speed_ratio: 1.0,
            volume_ratio: 1.0,
            pitch_ratio: 1.0,
        }
    }
}

/// TTS 合成响应
#[derive(Debug, Clone)]
pub struct TtsResponse {
    /// 合成的音频二进制数据
    pub audio_data: Vec<u8>,
    /// 实际的音频编码格式
    pub format: AudioFormat,
    /// 音频时长（秒），由 API 返回，可能为空
    pub duration_secs: Option<f64>,
}

// ── 错误类型 ────────────────────────────────────────────────────────

/// TTS 相关错误
#[derive(Debug, Clone)]
pub enum TtsError {
    /// 网络连接失败
    Connection(String),
    /// 请求超时
    Timeout(String),
    /// 认证失败（token/invalid 等）
    Authentication(String),
    /// API 返回了无法解析的响应
    InvalidResponse(String),
    /// 服务商返回的业务错误
    ProviderError(String),
}

impl std::fmt::Display for TtsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Connection(m) => write!(f, "TTS connection error: {m}"),
            Self::Timeout(m) => write!(f, "TTS timeout: {m}"),
            Self::Authentication(m) => write!(f, "TTS authentication error: {m}"),
            Self::InvalidResponse(m) => write!(f, "TTS invalid response: {m}"),
            Self::ProviderError(m) => write!(f, "TTS provider error: {m}"),
        }
    }
}

impl std::error::Error for TtsError {}

// ── Provider 接口 ──────────────────────────────────────────────────

/// TTS Provider 领域接口
///
/// 所有文本转语音服务商都应实现此 trait。
/// 用法与 [`crate::domain::llm::LlmProvider`] 一致。
#[async_trait]
pub trait TtsProvider: Send + Sync {
    /// 将文本合成为语音
    ///
    /// # 参数
    /// - `request`: 合成参数（文本、音色、格式等）
    ///
    /// # 返回
    /// - `Ok(TtsResponse)`: 包含音频数据和格式信息
    /// - `Err(TtsError)`: 合成失败
    async fn synthesize(&self, request: TtsRequest) -> Result<TtsResponse, TtsError>;
}
