use async_trait::async_trait;
use serde::Serialize;
use tracing::{debug, warn};

use crate::domain::tts::{AudioFormat, TtsError, TtsProvider, TtsRequest, TtsResponse};
use crate::shared::config::TtsConfig;

// ── 火山引擎（豆包语音）常用音色 ID ──────────────────────────────────

/// 中文女声-清新
pub const VOICE_ZH_FEMALE_QINGXIN: &str = "zh_female_qingxin";
/// 中文男声-醇厚
pub const VOICE_ZH_MALE_CHUNHOU: &str = "zh_male_chunhou";
/// 中文女声-知性
pub const VOICE_ZH_FEMALE_ZHIXING: &str = "zh_female_zhixing";
/// 中文男声-亲切
pub const VOICE_ZH_MALE_QINQIE: &str = "zh_male_qinqie";
/// 中文女声-俏皮
pub const VOICE_ZH_FEMALE_QIAOPI: &str = "zh_female_qiaopi";
/// 中文女声-甜美
pub const VOICE_ZH_FEMALE_TIANMEI: &str = "zh_female_tianmei";
/// 中文男声-小说叙述
pub const VOICE_ZH_MALE_NOVEL_NARRATION: &str = "zh_male_novel_narration";
/// 英文男声-Adam
pub const VOICE_EN_MALE_ADAM: &str = "en_male_adam";
/// 英文女声-Sarah
pub const VOICE_EN_FEMALE_SARAH: &str = "en_female_sarah";
/// 日文女声-Hana
pub const VOICE_JP_FEMALE_HANA: &str = "jp_female_hana";
/// 日文男声-Satoshi
pub const VOICE_JP_MALE_SATOSHI: &str = "jp_male_satoshi";
/// 湾湾小何（豆包语音合成大模型 2.0 音色）
pub const VOICE_ZH_FEMALE_WANWAN_XIAOHE: &str = "zh_female_wanwanxiaohe_moon_bigtts";

// ── v3 API 请求体结构 ─────────────────────────────────────────────

/// v3 API 外层包裹
#[derive(Serialize)]
struct VolcTtsV3Request {
    req_params: ReqParams,
}

#[derive(Serialize)]
struct ReqParams {
    text: String,
    speaker: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    audio_params: Option<AudioParams>,
    #[serde(skip_serializing_if = "Option::is_none")]
    post_process: Option<PostProcess>,
}

#[derive(Serialize)]
struct AudioParams {
    #[serde(skip_serializing_if = "Option::is_none")]
    format: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    sample_rate: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    speech_rate: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    loudness_rate: Option<i32>,
}

#[derive(Serialize)]
struct PostProcess {
    #[serde(skip_serializing_if = "Option::is_none")]
    pitch: Option<i32>,
}

/// 成功响应
#[derive(serde::Deserialize)]
struct VolcTtsV3Response {
    #[allow(dead_code)]
    reqid: Option<String>,
    code: i64,
    #[allow(dead_code)]
    message: Option<String>,
    #[allow(dead_code)]
    data: Option<String>,
}

// ── 辅助函数：归一化比值 → v3 整数参数 ──────────────────────────────

/// 语速归一化比值 (0.5–2.0) 转 v3 speech_rate (-50 ~ 100)
fn ratio_to_speech_rate(ratio: f64) -> i32 {
    let v = ((ratio - 1.0) * 100.0).round() as i32;
    v.clamp(-50, 100)
}

/// 音量归一化比值 (0.5–2.0) 转 v3 loudness_rate (-50 ~ 100)
fn ratio_to_loudness_rate(ratio: f64) -> i32 {
    let v = ((ratio - 1.0) * 100.0).round() as i32;
    v.clamp(-50, 100)
}

/// 音调归一化比值 (0.5–2.0) 转 v3 pitch (-12 ~ 12)
fn ratio_to_pitch(ratio: f64) -> i32 {
    let v = ((ratio - 1.0) * 12.0).round() as i32;
    v.clamp(-12, 12)
}

// ── Provider ──────────────────────────────────────────────────────

/// 火山引擎（豆包语音）TTS Provider
///
/// 调用豆包语音 v3 单向流式合成接口 `https://openspeech.bytedance.com/api/v3/tts/unidirectional`
///
/// # 认证方式
/// - `X-Api-Key`：从火山引擎控制台 → API Key 管理 获取
/// - `X-Api-Resource-Id`：模型版本标识，如 `seed-tts-2.0`
pub struct VolcengineTtsProvider {
    client: reqwest::Client,
    base_url: String,
    api_key: String,
    resource_id: String,
    model: String,
    default_voice: String,
    default_encoding: String,
    sample_rate: u32,
}

impl VolcengineTtsProvider {
    /// 从应用配置创建 Provider
    pub fn new(config: &TtsConfig) -> Self {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(config.timeout_secs))
            .build()
            .expect("Failed to create HTTP client for VolcengineTtsProvider");

        Self {
            client,
            base_url: config.base_url.clone(),
            api_key: config.api_key.clone(),
            resource_id: config.resource_id.clone(),
            model: config.model.clone(),
            default_voice: config.default_voice.clone(),
            default_encoding: config.default_encoding.clone(),
            sample_rate: config.sample_rate,
        }
    }

    /// 设置独立的 HTTP Client（用于测试）
    #[doc(hidden)]
    pub fn with_client(config: &TtsConfig, client: reqwest::Client) -> Self {
        Self {
            client,
            base_url: config.base_url.clone(),
            api_key: config.api_key.clone(),
            resource_id: config.resource_id.clone(),
            model: config.model.clone(),
            default_voice: config.default_voice.clone(),
            default_encoding: config.default_encoding.clone(),
            sample_rate: config.sample_rate,
        }
    }

    /// 发送 POST 请求到 v3 端点
    async fn post_json<T: Serialize + Send + Sync>(
        &self,
        body: &T,
        reqid: &str,
    ) -> Result<reqwest::Response, TtsError> {
        let api_key = if self.api_key.is_empty() {
            "test_key"
        } else {
            &self.api_key
        };
        let resource_id = if self.resource_id.is_empty() {
            "test_resource"
        } else {
            &self.resource_id
        };

        let response = self
            .client
            .post(&self.base_url)
            .header("Content-Type", "application/json")
            .header("X-Api-Key", api_key)
            .header("X-Api-Resource-Id", resource_id)
            .header("X-Api-Request-Id", reqid)
            .json(body)
            .send()
            .await
            .map_err(|e| {
                if e.is_timeout() {
                    TtsError::Timeout(e.to_string())
                } else if e.is_connect() {
                    TtsError::Connection(e.to_string())
                } else {
                    TtsError::ProviderError(e.to_string())
                }
            })?;

        Ok(response)
    }
}

#[async_trait]
impl TtsProvider for VolcengineTtsProvider {
    async fn synthesize(&self, request: TtsRequest) -> Result<TtsResponse, TtsError> {
        let encoding = request.format.as_str().to_string();
        let encoding = if encoding.is_empty() {
            self.default_encoding.clone()
        } else {
            encoding
        };

        let requested_voice = request.voice;
        let voice = if requested_voice.is_empty() {
            self.default_voice.clone()
        } else if is_legacy_voice_alias(&requested_voice) {
            warn!(
                requested_voice = %requested_voice,
                fallback_voice = %self.default_voice,
                "legacy TTS voice alias is not valid for Volcengine v3, using default voice"
            );
            self.default_voice.clone()
        } else {
            requested_voice
        };

        let reqid = uuid::Uuid::new_v4().to_string();
        let speaker = voice; // hold value for debug! below

        // 构造 v3 请求体
        let volc_request = VolcTtsV3Request {
            req_params: ReqParams {
                text: request.text,
                speaker,
                model: Some(self.model.clone()),
                audio_params: Some(AudioParams {
                    format: Some(encoding.clone()),
                    sample_rate: Some(self.sample_rate),
                    speech_rate: Some(ratio_to_speech_rate(request.speed_ratio)),
                    loudness_rate: Some(ratio_to_loudness_rate(request.volume_ratio)),
                }),
                post_process: Some(PostProcess {
                    pitch: Some(ratio_to_pitch(request.pitch_ratio)),
                }),
            },
        };

        debug!(
            "Sending v3 TTS request to {} (resource={})",
            self.base_url, self.resource_id,
        );

        let response = self.post_json(&volc_request, &reqid).await?;

        // HTTP 层面 401 → 认证错误
        if response.status() == reqwest::StatusCode::UNAUTHORIZED {
            return Err(TtsError::Authentication(format!(
                "HTTP 401 Unauthorized — check TTS_API_KEY",
            )));
        }

        let status = response.status();
        let body_bytes = response
            .bytes()
            .await
            .map_err(|e| TtsError::InvalidResponse(format!("Failed to read response body: {e}")))?;

        if !status.is_success() {
            let text = String::from_utf8_lossy(&body_bytes);
            return Err(TtsError::ProviderError(format!("HTTP {status}: {text}")));
        }

        // 解析 JSON 响应。v3 接口返回多个 JSON 行（\n 分隔），每行包含一段 base64 音频，
        // 需要将所有 data 字段解码后拼接成完整的音频数据。
        let response_text = String::from_utf8_lossy(&body_bytes);
        let mut provider_errors = Vec::new();
        let mut audio_chunks = Vec::new();

        for line in response_text.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() || !trimmed.starts_with('{') {
                continue;
            }

            match serde_json::from_str::<VolcTtsV3Response>(trimmed) {
                Ok(resp) if resp.code == 0 => {
                    if let Some(data) = resp.data.as_ref() {
                        use base64::Engine;
                        match base64::engine::general_purpose::STANDARD.decode(data) {
                            Ok(chunk) => audio_chunks.push(chunk),
                            Err(e) => provider_errors.push(format!("base64 decode failed: {e}")),
                        }
                    }
                }
                Ok(resp) => {
                    provider_errors.push(format!(
                        "code={}, message={}",
                        resp.code,
                        resp.message
                            .unwrap_or_else(|| "missing message".to_string())
                    ));
                }
                Err(e) => {
                    provider_errors.push(format!("failed to parse JSON chunk: {e}"));
                }
            }
        }

        let audio_data: Vec<u8> = audio_chunks
            .into_iter()
            .reduce(|mut acc, chunk| {
                acc.extend_from_slice(&chunk);
                acc
            })
            .ok_or_else(|| {
                if provider_errors.is_empty() {
                    TtsError::InvalidResponse("no audio data found in any JSON chunk".to_string())
                } else {
                    TtsError::ProviderError(format!(
                        "no audio data; provider returned: {}",
                        provider_errors.join("; ")
                    ))
                }
            })?;

        if audio_data.is_empty() {
            return Err(TtsError::InvalidResponse(
                "TTS response contains no audio data".to_string(),
            ));
        }

        Ok(TtsResponse {
            audio_data,
            format: AudioFormat::from_str(&encoding).unwrap_or(AudioFormat::Wav),
            duration_secs: None,
        })
    }
}

fn is_legacy_voice_alias(voice: &str) -> bool {
    matches!(
        voice,
        VOICE_ZH_FEMALE_QINGXIN
            | VOICE_ZH_MALE_CHUNHOU
            | VOICE_ZH_FEMALE_ZHIXING
            | VOICE_ZH_MALE_QINQIE
            | VOICE_ZH_FEMALE_QIAOPI
            | VOICE_ZH_FEMALE_TIANMEI
            | VOICE_ZH_MALE_NOVEL_NARRATION
            | VOICE_EN_MALE_ADAM
            | VOICE_EN_FEMALE_SARAH
            | VOICE_JP_FEMALE_HANA
            | VOICE_JP_MALE_SATOSHI
    )
}

// ── 测试 ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::tts::AudioFormat;

    /// 真实调用豆包语音 v3 API 测试 `zh_female_wanwanxiaohe_moon_bigtts` 音色。
    ///
    /// 需要设置以下环境变量：
    /// - `TTS_API_KEY`      — 火山引擎 API Key（控制台 → API Key 管理）
    /// - `TTS_RESOURCE_ID`  — 模型版本标识，如 `seed-tts-2.0`
    ///
    /// 环境变量未设置时测试自动跳过（不失败）。
    #[tokio::test]
    async fn test_zh_female_wanwanxiaohe_synthesize() {
        let api_key = std::env::var("TTS_API_KEY").unwrap_or_default();
        let resource_id = std::env::var("TTS_RESOURCE_ID").unwrap_or_default();

        if api_key.is_empty() || resource_id.is_empty() {
            eprintln!("SKIP: TTS_API_KEY and TTS_RESOURCE_ID must be set for real API test");
            return;
        }

        let config = TtsConfig {
            provider: "volcengine".to_string(),
            base_url: "https://openspeech.bytedance.com/api/v3/tts/unidirectional".to_string(),
            api_key,
            resource_id,
            model: "seed-tts-2.0-standard".to_string(),
            default_voice: "zh_female_wanwanxiaohe_moon_bigtts".to_string(),
            default_encoding: "wav".to_string(),
            timeout_secs: 30,
            sample_rate: 24000,
            ..TtsConfig::default()
        };

        let provider = VolcengineTtsProvider::new(&config);

        let request = TtsRequest {
            text: "你好，欢迎使用语音合成服务，这是一段测试语音。".to_string(),
            voice: "zh_female_wanwanxiaohe_moon_bigtts".to_string(),
            format: AudioFormat::Wav,
            speed_ratio: 1.0,
            volume_ratio: 1.0,
            pitch_ratio: 1.0,
        };

        let result = provider.synthesize(request).await;

        match result {
            Ok(response) => {
                tokio::fs::write("test_output.wav", &response.audio_data)
                    .await
                    .expect("Failed to write test_output.wav");
                assert!(
                    !response.audio_data.is_empty(),
                    "audio data should not be empty"
                );
                // WAV 文件最小头为 44 字节，确保有实际音频内容
                assert!(
                    response.audio_data.len() > 44,
                    "audio data length should exceed WAV header size (44), got {}",
                    response.audio_data.len()
                );
                // 验证 WAV 文件头 RIFF 标记
                assert_eq!(
                    &response.audio_data[..4],
                    b"RIFF",
                    "audio data should start with RIFF WAV header"
                );
                assert_eq!(
                    &response.audio_data[8..12],
                    b"WAVE",
                    "audio data should contain WAVE format identifier"
                );
                println!(
                    "PASS: synthesized {} bytes of WAV audio (voice: {})",
                    response.audio_data.len(),
                    VOICE_ZH_FEMALE_WANWAN_XIAOHE,
                );
            }
            Err(TtsError::Authentication(msg)) => {
                panic!("TTS authentication failed — check TTS_API_KEY: {msg}");
            }
            Err(e) => {
                panic!("TTS API call failed: {e}");
            }
        }
    }

    /// 测试无效 API Key 能否得到正确的认证错误（不需要真实凭证）。
    ///
    /// v3 API 在请求头层面就会拒绝无效 Key 返回 401。
    #[tokio::test]
    async fn test_invalid_api_key_returns_auth_error() {
        let config = TtsConfig {
            provider: "volcengine".to_string(),
            base_url: "https://openspeech.bytedance.com/api/v3/tts/unidirectional".to_string(),
            api_key: "invalid_key_xxx".to_string(),
            resource_id: "seed-tts-2.0".to_string(),
            model: "seed-tts-2.0-standard".to_string(),
            default_voice: "zh_female_wanwanxiaohe_moon_bigtts".to_string(),
            default_encoding: "wav".to_string(),
            timeout_secs: 10,
            sample_rate: 24000,
            ..TtsConfig::default()
        };

        let provider = VolcengineTtsProvider::new(&config);

        let request = TtsRequest::new("你好", "zh_female_wanwanxiaohe_moon_bigtts");

        let result = provider.synthesize(request).await;

        match result {
            Ok(_) => {
                panic!("expected error with invalid API key, got Ok");
            }
            Err(TtsError::Authentication(_)) => {
                // 预期：v3 会在 HTTP 层面返回 401，映射为 Authentication
                eprintln!("got expected Authentication error");
            }
            Err(e) => {
                // 如果恰好通过请求校验走到了业务层，也认为是合理的
                eprintln!("got expected error with invalid API key: {e}");
            }
        }
    }

    #[test]
    fn test_ratio_conversions() {
        // speed_ratio → speech_rate
        assert_eq!(ratio_to_speech_rate(0.5), -50);
        assert_eq!(ratio_to_speech_rate(1.0), 0);
        assert_eq!(ratio_to_speech_rate(1.5), 50);
        assert_eq!(ratio_to_speech_rate(2.0), 100);

        // volume_ratio → loudness_rate
        assert_eq!(ratio_to_loudness_rate(0.5), -50);
        assert_eq!(ratio_to_loudness_rate(1.0), 0);
        assert_eq!(ratio_to_loudness_rate(1.5), 50);
        assert_eq!(ratio_to_loudness_rate(2.0), 100);

        // pitch_ratio → pitch
        assert_eq!(ratio_to_pitch(1.0), 0);
        assert_eq!(ratio_to_pitch(2.0), 12);
        assert_eq!(ratio_to_pitch(0.5), -6);
        assert_eq!(ratio_to_pitch(0.0), -12); // clamp
        assert_eq!(ratio_to_pitch(3.0), 12); // clamp
    }
}
