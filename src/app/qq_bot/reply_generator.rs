use std::sync::Arc;

use serde_json;
use tracing::{info, warn};

use crate::domain::llm::{ChatCompletionRequest, ChatMessage, LlmProvider};
use crate::domain::qq_bot::config::GroupConfig;
use crate::domain::qq_bot::message::NormalizedMessage;
use crate::domain::qq_bot::persona::BotPersona;
use crate::domain::qq_bot::reply::{BotReply, ReplySegment, TimingHint};
use crate::domain::qq_bot::QqBotError;

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

        let request = ChatCompletionRequest::new(messages)
            .with_temperature(0.7);

        let response = self.llm_provider.chat(request).await.map_err(|e| {
            QqBotError::Internal(format!("LLM reply generation failed: {e}"))
        })?;

        // Parse the LLM response as BotReply JSON
        let reply = self.parse_reply(&response.content)?;
        info!(
            group_id = msg.qq_group_id,
            segment_count = reply.segments.len(),
            "reply generated"
        );

        Ok(reply)
    }

    /// Parse LLM response text into a `BotReply`.
    ///
    /// Tries to extract JSON from the response (handles markdown code fences).
    /// Falls back to wrapping the entire response as a single text segment.
    fn parse_reply(&self, text: &str) -> Result<BotReply, QqBotError> {
        // Try to find JSON in the response (may be wrapped in ```json ... ```)
        let json_str = if let Some(start) = text.find("```json") {
            let inner = &text[start + 7..];
            if let Some(end) = inner.find("```") {
                inner[..end].trim()
            } else {
                text.trim()
            }
        } else if let Some(start) = text.find('{') {
            let inner = &text[start..];
            if let Some(end) = inner.rfind('}') {
                &inner[..=end]
            } else {
                text.trim()
            }
        } else {
            text.trim()
        };

        // Attempt structured parse
        if let Ok(reply) = serde_json::from_str::<BotReply>(json_str) {
            // Clamp segments count and char length
            let segments: Vec<ReplySegment> = reply
                .segments
                .into_iter()
                .take(self.max_segments as usize)
                .map(|s| match s {
                    ReplySegment::Text { ref content } if content.len() > self.max_chars_per_segment as usize => {
                        ReplySegment::Text {
                            content: content.chars().take(self.max_chars_per_segment as usize).collect(),
                        }
                    }
                    ReplySegment::Record { ref text, voice } if text.len() > self.max_chars_per_segment as usize => {
                        ReplySegment::Record {
                            text: text.chars().take(self.max_chars_per_segment as usize).collect(),
                            voice,
                        }
                    }
                    other => other,
                })
                .collect();

            if segments.is_empty() {
                // Fallback: use raw text if parsed to empty
                return Ok(BotReply {
                    segments: vec![ReplySegment::Text {
                        content: text.chars().take(self.max_chars_per_segment as usize).collect(),
                    }],
                    timing_hint: TimingHint {
                        initial_delay_ms: self.default_initial_delay_ms,
                        inter_segment_delays_ms: vec![self.default_inter_segment_delay_ms],
                        show_typing: true,
                    },
                    emotion_change: None,
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

            return Ok(BotReply { segments, timing_hint: timing, emotion_change: reply.emotion_change });
        }

        // Fallback: wrap raw text as single segment
        warn!("failed to parse LLM reply as BotReply JSON, using fallback");
        Ok(BotReply {
            segments: vec![ReplySegment::Text {
                content: text.chars().take(self.max_chars_per_segment as usize).collect(),
            }],
            timing_hint: TimingHint {
                initial_delay_ms: self.default_initial_delay_ms,
                inter_segment_delays_ms: Vec::new(),
                show_typing: true,
            },
            emotion_change: None,
        })
    }
}
