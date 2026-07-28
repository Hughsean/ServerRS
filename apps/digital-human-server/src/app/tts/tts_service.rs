use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use digital_human::domain::tts::{AudioFormat, TtsProvider, TtsRequest};
use digital_human::shared::config::TtsConfig;
use digital_human::shared::error::AppError;
use ring::hmac;
use tokio::process::Command;
use uuid::Uuid;

const TRANSCODE_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AudioSpec {
    pub format: AudioFormat,
    pub sample_rate: u32,
    pub channels: u8,
    pub sample_bits: u8,
}

impl AudioSpec {
    pub fn validate(self) -> Result<Self, AppError> {
        if !(8_000..=48_000).contains(&self.sample_rate) || !(1..=2).contains(&self.channels) {
            return Err(AppError::Validation("不支持的音频采样率或声道数".into()));
        }
        match self.format {
            AudioFormat::Wav | AudioFormat::Pcm if matches!(self.sample_bits, 16 | 24 | 32) => {
                Ok(self)
            }
            AudioFormat::Mp3 if self.sample_bits == 16 => Ok(self),
            AudioFormat::OggOpus if self.sample_rate == 48_000 && self.sample_bits == 16 => {
                Ok(self)
            }
            _ => Err(AppError::Validation("不支持的音频格式参数组合".into())),
        }
    }

    pub fn extension(self) -> &'static str {
        match self.format {
            AudioFormat::Wav => "wav",
            AudioFormat::Pcm => "pcm",
            AudioFormat::Mp3 => "mp3",
            AudioFormat::OggOpus => "ogg",
        }
    }

    pub fn mime_type(self) -> &'static str {
        match self.format {
            AudioFormat::Wav => "audio/wav",
            AudioFormat::Pcm => "audio/L16",
            AudioFormat::Mp3 => "audio/mpeg",
            AudioFormat::OggOpus => "audio/ogg",
        }
    }
}

#[derive(Debug, Clone)]
pub struct GeneratedAudio {
    pub file_id: Uuid,
    pub spec: AudioSpec,
    pub audio_url: String,
}

#[derive(Clone)]
pub struct TtsService {
    provider: Arc<dyn TtsProvider>,
    output_dir: PathBuf,
    public_url_base: String,
    signing_key: Arc<hmac::Key>,
    url_expiry_secs: u64,
    ffmpeg_path: String,
    default_voice: String,
    allowed_voices: Arc<Vec<String>>,
}

impl TtsService {
    pub fn new(config: &TtsConfig, provider: Arc<dyn TtsProvider>) -> Result<Self, AppError> {
        if !config.enabled {
            return Err(AppError::NotImplemented("对话语音接口未启用".into()));
        }
        if config.url_signing_key.len() < 32 {
            return Err(AppError::Infrastructure("语音 URL 签名配置无效".into()));
        }
        Ok(Self {
            provider,
            output_dir: PathBuf::from(&config.output_dir),
            public_url_base: config.public_url_base.trim_end_matches('/').to_owned(),
            signing_key: Arc::new(hmac::Key::new(
                hmac::HMAC_SHA256,
                config.url_signing_key.as_bytes(),
            )),
            url_expiry_secs: config.url_expiry_secs,
            ffmpeg_path: config.ffmpeg_path.clone(),
            default_voice: config.default_voice.clone(),
            allowed_voices: Arc::new(config.allowed_voices.clone()),
        })
    }

    pub async fn verify_ffmpeg(&self) -> Result<(), AppError> {
        let output = Command::new(&self.ffmpeg_path)
            .arg("-version")
            .output()
            .await
            .map_err(|_| AppError::Infrastructure("语音转码器不可用".into()))?;
        if output.status.success() {
            Ok(())
        } else {
            Err(AppError::Infrastructure("语音转码器不可用".into()))
        }
    }

    pub async fn synthesize(
        &self,
        text: String,
        voice: Option<String>,
        spec: AudioSpec,
    ) -> Result<GeneratedAudio, AppError> {
        let spec = spec.validate()?;
        let voice = self.select_voice(voice)?;
        let response = self
            .provider
            .synthesize(TtsRequest::new(text, voice))
            .await
            .map_err(|error| {
                tracing::warn!(error = %error, "语音合成提供方请求失败");
                AppError::Infrastructure("语音合成服务暂时不可用".into())
            })?;
        if response.audio_data.is_empty() {
            return Err(AppError::Infrastructure("语音合成服务返回了空音频".into()));
        }

        tokio::fs::create_dir_all(&self.output_dir)
            .await
            .map_err(|error| {
                tracing::error!(error = %error, "创建数字人语音目录失败");
                AppError::Infrastructure("语音文件保存失败".into())
            })?;
        let file_id = Uuid::new_v4();
        let source_path = self.output_dir.join(format!(".{file_id}.source"));
        let final_path = self.file_path(file_id, spec);
        tokio::fs::write(&source_path, response.audio_data)
            .await
            .map_err(|error| {
                tracing::error!(error = %error, "写入语音转码临时文件失败");
                AppError::Infrastructure("语音文件保存失败".into())
            })?;

        let result = self.transcode(&source_path, &final_path, spec).await;
        let _ = tokio::fs::remove_file(&source_path).await;
        result?;

        let expires = unix_now()?.saturating_add(self.url_expiry_secs);
        let signature = self.sign(file_id, expires);
        Ok(GeneratedAudio {
            file_id,
            spec,
            audio_url: format!(
                "{}/api/v1/tts/audio/{}?expires={expires}&signature={signature}",
                self.public_url_base, file_id
            ),
        })
    }

    pub fn resolve_signed_file(
        &self,
        file_id: Uuid,
        expires: u64,
        signature: &str,
    ) -> Option<(PathBuf, &'static str)> {
        if expires <= unix_now().ok()? {
            return None;
        }
        if !constant_time_eq(self.sign(file_id, expires).as_bytes(), signature.as_bytes()) {
            return None;
        }
        for (extension, mime_type) in supported_extensions() {
            let path = self.output_dir.join(format!("{file_id}.{extension}"));
            if path.exists() {
                return Some((path, mime_type));
            }
        }
        None
    }

    pub async fn cleanup_expired(&self, retention: Duration) -> Result<u64, AppError> {
        let mut entries = tokio::fs::read_dir(&self.output_dir)
            .await
            .map_err(|error| {
                if error.kind() == std::io::ErrorKind::NotFound {
                    return AppError::Infrastructure("语音目录不可用".into());
                }
                AppError::Infrastructure("语音目录读取失败".into())
            })?;
        let threshold = SystemTime::now()
            .checked_sub(retention)
            .ok_or_else(|| AppError::Internal("系统时间无效".into()))?;
        let mut removed = 0;
        while let Some(entry) = entries
            .next_entry()
            .await
            .map_err(|_| AppError::Infrastructure("语音目录读取失败".into()))?
        {
            let path = entry.path();
            if !is_managed_audio_file(&path) {
                continue;
            }
            let metadata = entry
                .metadata()
                .await
                .map_err(|_| AppError::Infrastructure("语音文件读取失败".into()))?;
            if metadata.is_file()
                && metadata
                    .modified()
                    .ok()
                    .is_some_and(|modified| modified < threshold)
            {
                tokio::fs::remove_file(path)
                    .await
                    .map_err(|_| AppError::Infrastructure("语音文件清理失败".into()))?;
                removed += 1;
            }
        }
        Ok(removed)
    }

    fn select_voice(&self, requested: Option<String>) -> Result<String, AppError> {
        let voice = requested.unwrap_or_else(|| self.default_voice.clone());
        if voice.trim().is_empty()
            || (voice != self.default_voice
                && !self.allowed_voices.iter().any(|allowed| allowed == &voice))
        {
            return Err(AppError::Validation("请求的音色不受支持".into()));
        }
        Ok(voice)
    }

    async fn transcode(
        &self,
        source: &Path,
        target: &Path,
        spec: AudioSpec,
    ) -> Result<(), AppError> {
        let codec = match spec.format {
            AudioFormat::Wav => format!("pcm_s{}le", spec.sample_bits),
            AudioFormat::Pcm => format!("pcm_s{}le", spec.sample_bits),
            AudioFormat::Mp3 => "libmp3lame".into(),
            AudioFormat::OggOpus => "libopus".into(),
        };
        let mut command = Command::new(&self.ffmpeg_path);
        command
            .args(["-y", "-nostdin", "-hide_banner", "-loglevel", "error", "-i"])
            .arg(source)
            .args([
                "-ar",
                &spec.sample_rate.to_string(),
                "-ac",
                &spec.channels.to_string(),
                "-c:a",
                &codec,
            ]);
        if spec.format == AudioFormat::Pcm {
            command.args(["-f", &format!("s{}le", spec.sample_bits)]);
        }
        command.arg(target);
        let output = tokio::time::timeout(TRANSCODE_TIMEOUT, command.output())
            .await
            .map_err(|_| AppError::Infrastructure("语音转码超时".into()))?
            .map_err(|_| AppError::Infrastructure("语音转码器不可用".into()))?;
        if !output.status.success() {
            tracing::warn!(status = ?output.status, "语音转码失败");
            return Err(AppError::Infrastructure("语音转码失败".into()));
        }
        let metadata = tokio::fs::metadata(target)
            .await
            .map_err(|_| AppError::Infrastructure("语音转码输出缺失".into()))?;
        if metadata.len() == 0 {
            return Err(AppError::Infrastructure("语音转码输出为空".into()));
        }
        Ok(())
    }

    fn file_path(&self, file_id: Uuid, spec: AudioSpec) -> PathBuf {
        self.output_dir
            .join(format!("{file_id}.{}", spec.extension()))
    }

    fn sign(&self, file_id: Uuid, expires: u64) -> String {
        hmac::sign(&self.signing_key, format!("{file_id}:{expires}").as_bytes())
            .as_ref()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }
}

fn unix_now() -> Result<u64, AppError> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .map_err(|_| AppError::Internal("系统时间无效".into()))
}
fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right)
        .fold(0u8, |difference, (left, right)| difference | (left ^ right))
        == 0
}
fn supported_extensions() -> [(&'static str, &'static str); 4] {
    [
        ("wav", "audio/wav"),
        ("pcm", "audio/L16"),
        ("mp3", "audio/mpeg"),
        ("ogg", "audio/ogg"),
    ]
}
fn is_managed_audio_file(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    let Some((id, extension)) = name.rsplit_once('.') else {
        return false;
    };
    supported_extensions()
        .iter()
        .any(|(allowed_extension, _)| extension == *allowed_extension)
        && Uuid::parse_str(id).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(format: AudioFormat, sample_rate: u32, channels: u8, sample_bits: u8) -> AudioSpec {
        AudioSpec {
            format,
            sample_rate,
            channels,
            sample_bits,
        }
    }

    #[test]
    fn validates_supported_audio_specifications() {
        assert!(spec(AudioFormat::Wav, 44_100, 2, 24).validate().is_ok());
        assert!(spec(AudioFormat::Pcm, 8_000, 1, 16).validate().is_ok());
        assert!(spec(AudioFormat::Mp3, 48_000, 2, 16).validate().is_ok());
        assert!(spec(AudioFormat::OggOpus, 48_000, 1, 16).validate().is_ok());
    }

    #[test]
    fn rejects_invalid_audio_specifications() {
        assert!(spec(AudioFormat::Wav, 7_999, 1, 16).validate().is_err());
        assert!(spec(AudioFormat::Pcm, 16_000, 3, 16).validate().is_err());
        assert!(spec(AudioFormat::Mp3, 16_000, 1, 24).validate().is_err());
        assert!(
            spec(AudioFormat::OggOpus, 24_000, 1, 16)
                .validate()
                .is_err()
        );
    }

    #[test]
    fn recognizes_only_uuid_managed_audio_files() {
        let id = Uuid::new_v4();
        assert!(is_managed_audio_file(Path::new(&format!("{id}.wav"))));
        assert!(is_managed_audio_file(Path::new(&format!("{id}.ogg"))));
        assert!(!is_managed_audio_file(Path::new("notes.wav")));
        assert!(!is_managed_audio_file(Path::new(&format!("{id}.source"))));
        assert!(!is_managed_audio_file(Path::new(&format!("{id}.wav.bak"))));
    }

    #[test]
    fn exposes_supported_extension_mime_pairs() {
        assert_eq!(
            supported_extensions(),
            [
                ("wav", "audio/wav"),
                ("pcm", "audio/L16"),
                ("mp3", "audio/mpeg"),
                ("ogg", "audio/ogg"),
            ]
        );
    }
}
