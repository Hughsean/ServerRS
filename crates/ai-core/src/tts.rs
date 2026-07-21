use async_trait::async_trait;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AudioFormat {
    Wav,
    Mp3,
    Pcm,
    OggOpus,
}

impl AudioFormat {
    /// Returns the provider-neutral wire name used by adapters.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Wav => "wav",
            Self::Mp3 => "mp3",
            Self::Pcm => "pcm",
            Self::OggOpus => "ogg_opus",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value.to_lowercase().as_str() {
            "wav" => Some(Self::Wav),
            "mp3" => Some(Self::Mp3),
            "pcm" => Some(Self::Pcm),
            "ogg_opus" | "ogg" => Some(Self::OggOpus),
            _ => None,
        }
    }

    /// Compatibility helper for existing adapters.
    pub fn from_str(value: &str) -> Option<Self> {
        Self::parse(value)
    }
}

impl std::str::FromStr for AudioFormat {
    type Err = TtsError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
            .ok_or_else(|| TtsError::InvalidResponse(format!("unsupported audio format: {value}")))
    }
}

#[derive(Debug, Clone)]
pub struct TtsRequest {
    pub text: String,
    pub voice: String,
    pub format: AudioFormat,
    pub speed_ratio: f64,
    pub volume_ratio: f64,
    pub pitch_ratio: f64,
}

impl TtsRequest {
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

#[derive(Debug, Clone)]
pub struct TtsResponse {
    pub audio_data: Vec<u8>,
    pub format: AudioFormat,
    pub duration_secs: Option<f64>,
}

#[derive(Debug, Clone, thiserror::Error)]
pub enum TtsError {
    #[error("TTS connection error: {0}")]
    Connection(String),
    #[error("TTS timeout: {0}")]
    Timeout(String),
    #[error("TTS authentication error: {0}")]
    Authentication(String),
    #[error("TTS invalid response: {0}")]
    InvalidResponse(String),
    #[error("TTS provider error: {0}")]
    ProviderError(String),
}

#[async_trait]
pub trait TtsProvider: Send + Sync {
    async fn synthesize(&self, request: TtsRequest) -> Result<TtsResponse, TtsError>;
}
