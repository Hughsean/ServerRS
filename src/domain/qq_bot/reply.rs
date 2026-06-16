use serde::{Deserialize, Serialize};

use crate::domain::qq_bot::bot_state::Mood;

/// A structured reply that simulates human-like multi-segment conversation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BotReply {
    /// Ordered list of reply segments.
    pub segments: Vec<ReplySegment>,
    /// Timing hints for delivering segments.
    #[serde(default)]
    pub timing_hint: TimingHint,
    /// Optional emotion change hint for the bot to update its emotional state.
    /// The LLM can include this to tell the bot how it feels after saying the reply.
    #[serde(default)]
    pub emotion_change: Option<EmotionChange>,
    /// Optional relationship hints extracted from the LLM's reply.
    /// The LLM can suggest how to address a specific user.
    #[serde(default)]
    pub relationship_hints: Option<RelationshipHints>,
}

/// Relationship hints that the LLM can include in its reply.
/// Used to automatically update nickname preferences and known interests.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelationshipHints {
    /// The QQ user this hint applies to.
    pub target_user_id: i64,
    /// Preferred nickname to call this user.
    #[serde(default)]
    pub nickname_preference: Option<String>,
    /// Known interests the LLM inferred from the conversation.
    #[serde(default)]
    pub known_interests: Vec<String>,
    /// Topics to avoid with this user.
    #[serde(default)]
    pub known_avoid_topics: Vec<String>,
}

/// Hint for updating the bot's emotional state after a reply.
/// The LLM fills this in to express how the reply reflects its mood.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmotionChange {
    /// New mood.
    pub mood: Mood,
    /// Intensity of the mood (0.0~1.0).
    pub intensity: f32,
    /// Why the emotion changed.
    #[serde(default)]
    pub reason: Option<String>,
}

/// A single segment of the reply.
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
