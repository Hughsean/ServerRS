use serde::{Deserialize, Serialize};
use validator::Validate;

use digital_human::domain::tts::AudioFormat;

use crate::app::tts::tts_service::{AudioSpec, GeneratedAudio};

/// 对话与语音合成请求。音频格式由服务端校验后交给 ffmpeg 转码。
#[derive(Debug, Deserialize, Validate)]
#[serde(rename_all = "camelCase")]
pub struct ChatMessageWithAudioRequest {
    #[validate(length(min = 1))]
    pub text: String,
    #[validate(length(max = 200))]
    pub emotion: Option<String>,
    #[serde(default)]
    pub location: Option<std::collections::HashMap<String, serde_json::Value>>,
    #[validate(length(min = 1, max = 128))]
    pub format: String,
    pub sample_rate: u32,
    pub channels: u8,
    pub sample_bits: u8,
    #[validate(length(min = 1, max = 128))]
    pub voice: Option<String>,
}

impl ChatMessageWithAudioRequest {
    pub fn audio_spec(&self) -> Result<AudioSpec, String> {
        let format =
            AudioFormat::parse(&self.format).ok_or_else(|| "不支持的音频格式".to_string())?;
        AudioSpec {
            format,
            sample_rate: self.sample_rate,
            channels: self.channels,
            sample_bits: self.sample_bits,
        }
        .validate()
        .map_err(|error| error.to_string())
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatMessageWithAudioResponse {
    pub conversation_id: u64,
    pub reply: String,
    pub tool_calls: Vec<ChatAudioToolCallItem>,
    pub audio: ChatAudioResponse,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatAudioToolCallItem {
    pub name: String,
    pub arguments: serde_json::Value,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatAudioResponse {
    pub audio_url: String,
    pub format: &'static str,
    pub sample_rate: u32,
    pub channels: u8,
    pub sample_bits: u8,
}

impl From<GeneratedAudio> for ChatAudioResponse {
    fn from(audio: GeneratedAudio) -> Self {
        Self {
            audio_url: audio.audio_url,
            format: audio.spec.format.as_str(),
            sample_rate: audio.spec.sample_rate,
            channels: audio.spec.channels,
            sample_bits: audio.spec.sample_bits,
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct SignedAudioQuery {
    pub expires: u64,
    pub signature: String,
}
