use serde::{Deserialize, Serialize};

use crate::domain::qq_bot::bot_state::Mood;

/// 模拟人类多段对话的结构化回复。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BotReply {
    /// 有序的回复段列表。
    pub segments: Vec<ReplySegment>,
    /// 段的发送时序提示。
    #[serde(default)]
    pub timing_hint: TimingHint,
    /// 可选的机器人情绪变化提示。
    /// LLM 可以包含此字段，告诉机器人发出回复后的感受。
    #[serde(default)]
    pub emotion_change: Option<EmotionChange>,
    /// 从 LLM 回复中提取的可选关系提示。
    /// LLM 可以建议如何称呼特定用户。
    #[serde(default)]
    pub relationship_hints: Option<RelationshipHints>,
}

/// LLM 可以在回复中包含的关系提示。
/// 用于自动更新昵称偏好和已知兴趣。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelationshipHints {
    /// 此提示适用的 QQ 用户。
    pub target_user_id: i64,
    /// 称呼此用户的首选昵称。
    #[serde(default)]
    pub nickname_preference: Option<String>,
    /// LLM 从对话中推断出的已知兴趣。
    #[serde(default)]
    pub known_interests: Vec<String>,
    /// 与此用户应避免的话题。
    #[serde(default)]
    pub known_avoid_topics: Vec<String>,
}

/// 回复后更新机器人情绪状态的提示。
/// LLM 填充此字段以表达回复如何反映其心情。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmotionChange {
    /// 新情绪。
    pub mood: Mood,
    /// 情绪强度 (0.0~1.0)。
    pub intensity: f32,
    /// 情绪变化的原因。
    #[serde(default)]
    pub reason: Option<String>,
}

/// 回复的单个段。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ReplySegment {
    /// Plain text.
    Text { content: String },
    /// QQ emoji by id.
    Emoji { id: i32 },
    /// Kaomoji / emoticon text like (´▽`ʃ♡ƪ).
    Kaomoji { text: String },
    /// Local image file path or URL.
    Image { path: String },
    /// Quote-reply to a specific message.
    QuoteReply { message_id: u64, text: String },
    /// Poke a user in a group.
    Poke { user_id: i64, group_id: i64 },
    /// TTS voice message — text will be synthesised by TTS provider.
    Record { text: String, voice: String },
}

/// Timing hints for delivering multi-segment replies.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TimingHint {
    /// Delay in ms before the FIRST segment is sent.
    /// Simulates "thinking" time. Default: 1000~2000 ms.
    #[serde(default = "default_initial_delay_ms")]
    pub initial_delay_ms: u64,
    /// Delays in ms BETWEEN consecutive segments.
    /// Simulates typing pause. If empty, uses 800ms for each gap.
    #[serde(default)]
    pub inter_segment_delays_ms: Vec<u64>,
    /// Whether to show "typing" indicator before sending.
    #[serde(default)]
    pub show_typing: bool,
}

fn default_initial_delay_ms() -> u64 {
    1500
}
