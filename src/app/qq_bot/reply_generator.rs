use std::sync::Arc;

use tracing::{info, warn};

use crate::domain::llm::{ChatCompletionRequest, ChatMessage, LlmProvider};
use crate::domain::qq_bot::QqBotError;
use crate::domain::qq_bot::config::GroupConfig;
use crate::domain::qq_bot::message::NormalizedMessage;
use crate::domain::qq_bot::persona::BotPersona;
use crate::domain::qq_bot::reply::{BotReply, ReplySegment, TimingHint};
use crate::shared::llm_json::parse_llm_json;

/// Generates structured multi-segment replies using LLM.
///
/// The LLM is prompted with the conversation context and asked to produce
/// a JSON response following the `BotReply` schema (segments + timing_hint).
pub struct ReplyGenerator {
    llm_provider: Arc<dyn LlmProvider>,
    #[allow(dead_code)]
    persona: BotPersona,
    max_segments: u32,
    max_chars_per_segment: u32,
    default_inter_segment_delay_ms: u64,
    default_initial_delay_ms: u64,
}

impl ReplyGenerator {
    pub fn new(
        llm_provider: Arc<dyn LlmProvider>,
        persona: BotPersona,
        max_segments: u32,
        max_chars_per_segment: u32,
        default_inter_segment_delay_ms: u64,
        default_initial_delay_ms: u64,
    ) -> Self {
        Self {
            llm_provider,
            persona,
            max_segments,
            max_chars_per_segment,
            default_inter_segment_delay_ms,
            default_initial_delay_ms,
        }
    }

    /// Generate a reply for the given message with the provided context.
    ///
    /// `context` should contain system prompt + recent history messages
    /// as built by `ContextBuilder`.
    pub async fn generate_reply(
        &self,
        msg: &NormalizedMessage,
        context: Vec<ChatMessage>,
        _group_config: Option<&GroupConfig>,
    ) -> Result<BotReply, QqBotError> {
        // Append the current user message as the final input
        let user_display_name = format!("用户{}", msg.qq_user_id.unwrap_or(0));
        let user_content = format!("[{}] {}", user_display_name, msg.normalized_text);

        let mut messages = context;
        messages.push(ChatMessage {
            role: "user".into(),
            content: user_content,
            tool_calls: None,
            tool_call_id: None,
            name: None,
        });

        let request = ChatCompletionRequest::new(messages).with_temperature(0.7);

        let response = self
            .llm_provider
            .chat(request)
            .await
            .map_err(|e| QqBotError::Internal(format!("LLM reply generation failed: {e}")))?;

        // Parse the LLM response as BotReply JSON
        let reply = self.parse_reply(&response.content)?;
        info!(segment_count = reply.segments.len(), "reply generated");

        Ok(reply)
    }

    /// Generate a proactive reply (no triggering user message).
    ///
    /// Instead of appending a user message, appends a system instruction
    /// that tells the LLM to proactively say something.
    pub async fn generate_proactive_reply(
        &self,
        context: Vec<ChatMessage>,
    ) -> Result<BotReply, QqBotError> {
        let mut messages = context;
        messages.push(ChatMessage {
            role: "system".into(),
            content: "现在群里有些冷场，你决定主动说点什么来活跃气氛。\n请根据上面的上下文直接输出回复 JSON，不要包含本指令的思考过程。".into(),
            tool_calls: None,
            tool_call_id: None,
            name: None,
        });

        let request = ChatCompletionRequest::new(messages).with_temperature(0.7);

        let response = self.llm_provider.chat(request).await.map_err(|e| {
            QqBotError::Internal(format!("LLM proactive reply generation failed: {e}"))
        })?;

        let reply = self.parse_reply(&response.content)?;
        info!(
            segment_count = reply.segments.len(),
            "proactive reply generated"
        );

        Ok(reply)
    }

    /// Parse LLM response text into a `BotReply`.
    ///
    /// Tries to extract JSON from the response (handles markdown code fences).
    /// Falls back to wrapping the entire response as a single text segment.
    fn parse_reply(&self, text: &str) -> Result<BotReply, QqBotError> {
        // Attempt structured parse
        if let Ok(reply) = parse_llm_json::<BotReply>(text) {
            // Clamp segments count and char length
            let segments: Vec<ReplySegment> = reply
                .segments
                .into_iter()
                .take(self.max_segments as usize)
                .filter_map(|s| match s {
                    ReplySegment::Text { ref content }
                        if content.len() > self.max_chars_per_segment as usize =>
                    {
                        Some(ReplySegment::Text {
                            content: content
                                .chars()
                                .take(self.max_chars_per_segment as usize)
                                .collect(),
                        })
                    }
                    ReplySegment::Record { ref text, voice }
                        if text.len() > self.max_chars_per_segment as usize =>
                    {
                        Some(ReplySegment::Record {
                            text: text
                                .chars()
                                .take(self.max_chars_per_segment as usize)
                                .collect(),
                            voice,
                        })
                    }
                    ReplySegment::Image { path } if is_placeholder_image_path(&path) => {
                        warn!(path = %path, "LLM generated placeholder image URL, dropping segment");
                        None
                    }
                    ReplySegment::Emoji { id } if !is_allowed_emoji_id(id) => {
                        warn!(id, "LLM generated unsupported QQ face id, dropping segment");
                        None
                    }
                    other => Some(other),
                })
                .collect();

            if segments.is_empty() {
                // Fallback: use raw text if parsed to empty
                return Ok(BotReply {
                    segments: vec![ReplySegment::Text {
                        content: text
                            .chars()
                            .take(self.max_chars_per_segment as usize)
                            .collect(),
                    }],
                    timing_hint: TimingHint {
                        initial_delay_ms: self.default_initial_delay_ms,
                        inter_segment_delays_ms: vec![self.default_inter_segment_delay_ms],
                        show_typing: true,
                    },
                    emotion_change: None,
                    relationship_hints: None,
                });
            }

            // Fill missing inter_segment delays
            let timing = TimingHint {
                initial_delay_ms: if reply.timing_hint.initial_delay_ms > 0 {
                    reply.timing_hint.initial_delay_ms
                } else {
                    self.default_initial_delay_ms
                },
                inter_segment_delays_ms: if reply.timing_hint.inter_segment_delays_ms.is_empty() {
                    vec![self.default_inter_segment_delay_ms; segments.len().saturating_sub(1)]
                } else {
                    reply.timing_hint.inter_segment_delays_ms
                },
                show_typing: reply.timing_hint.show_typing,
            };

            return Ok(BotReply {
                segments,
                timing_hint: timing,
                emotion_change: reply.emotion_change,
                relationship_hints: reply.relationship_hints,
            });
        }

        // Fallback: wrap raw text as single segment
        warn!("无法将 LLM 回复解析为 BotReply JSON，使用回退方案");
        Ok(BotReply {
            segments: vec![ReplySegment::Text {
                content: text
                    .chars()
                    .take(self.max_chars_per_segment as usize)
                    .collect(),
            }],
            timing_hint: TimingHint {
                initial_delay_ms: self.default_initial_delay_ms,
                inter_segment_delays_ms: Vec::new(),
                show_typing: true,
            },
            emotion_change: None,
            relationship_hints: None,
        })
    }
}

fn is_placeholder_image_path(path: &str) -> bool {
    let normalized = path.trim().to_lowercase();
    normalized.is_empty()
        || normalized.contains("example.com")
        || normalized.contains("placeholder")
        || normalized.contains("cat.jpg")
}

fn is_allowed_emoji_id(id: i32) -> bool {
    matches!(
        id,
        1 | 2
            | 3
            | 4
            | 5
            | 6
            | 8
            | 9
            | 10
            | 11
            | 12
            | 13
            | 14
            | 15
            | 16
            | 19
            | 20
            | 21
            | 22
            | 23
            | 24
            | 25
            | 26
            | 27
            | 28
            | 30
            | 32
            | 33
            | 34
            | 39
            | 42
            | 49
            | 53
            | 63
            | 66
            | 74
            | 75
            | 76
            | 77
            | 78
            | 79
            | 85
            | 96
            | 97
            | 98
            | 99
            | 100
            | 101
            | 104
            | 105
            | 106
            | 107
            | 109
            | 111
            | 118
    )
}
