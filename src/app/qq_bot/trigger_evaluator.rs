use std::sync::Arc;

use chrono::Utc;
use tracing::{info, warn};

use crate::app::qq_bot::emotional_state_service::EmotionalStateService;
use crate::app::qq_bot::topic_service::TopicService;
use crate::domain::llm::{ChatCompletionRequest, LlmProvider};
use crate::domain::qq_bot::attention::{BotAccount, TriggerDecision};
use crate::domain::qq_bot::config::{GroupConfig, TriggerPolicy};
use crate::domain::qq_bot::message::NormalizedMessage;
use crate::domain::qq_bot::persona::BotPersona;
use crate::domain::qq_bot::QqBotError;
use crate::infra::qq_bot::attention_store::InMemoryAttentionStore;

/// Two-layer trigger evaluator:
///
/// **Layer 1 — Rule filters** (fast path):
/// - Bot disabled? → Skip
/// - Self-message? → Skip
/// - Group disabled? → Skip
/// - In cooldown? → Skip
/// - TriggerPolicy::Always → Respond (no LLM needed)
/// - TriggerPolicy::Silent → Skip
/// - TriggerPolicy::Mention + @bot → pass to Layer 2
/// - TriggerPolicy::Keyword + keyword match → pass to Layer 2
/// - TriggerPolicy::Command + command prefix → pass to Layer 2
///
/// **Layer 2 — LLM evaluation** (slow path):
/// - Ask LLM whether to respond, wait, or skip based on context
/// - Only invoked when bot is NOT in cooldown and group is engaged or idle
pub struct TriggerEvaluator {
    llm_provider: Arc<dyn LlmProvider>,
    attention_store: Arc<InMemoryAttentionStore>,
    persona: BotPersona,
    topic_service: Option<Arc<TopicService>>,
    emotional_service: Option<Arc<EmotionalStateService>>,
}

impl TriggerEvaluator {
    pub fn new(
        llm_provider: Arc<dyn LlmProvider>,
        attention_store: Arc<InMemoryAttentionStore>,
        persona: BotPersona,
        topic_service: Option<Arc<TopicService>>,
        emotional_service: Option<Arc<EmotionalStateService>>,
    ) -> Self {
        Self {
            llm_provider,
            attention_store,
            persona,
            topic_service,
            emotional_service,
        }
    }

    /// Evaluate whether the bot should respond to a message.
    ///
    /// Returns `TriggerDecision::Respond` only when both layers pass.
    pub async fn evaluate(
        &self,
        msg: &NormalizedMessage,
        bot_account: &BotAccount,
        group_config: Option<&GroupConfig>,
    ) -> Result<TriggerDecision, QqBotError> {
        // ── Layer 1: Rule filters ──────────────────────────────────────
        if !bot_account.enabled {
            return Ok(TriggerDecision::Skip);
        }

        // Skip self-messages
        if msg.qq_user_id == Some(bot_account.self_qq_id) {
            return Ok(TriggerDecision::Skip);
        }

        let policy = group_config
            .map(|g| g.trigger_policy)
            .unwrap_or(TriggerPolicy::Mention);

        match policy {
            TriggerPolicy::Silent => return Ok(TriggerDecision::Skip),
            TriggerPolicy::Always => {
                // Always respond — but still check attention
                if self.attention_store.can_process(msg.qq_group_id).await {
                    return Ok(TriggerDecision::Respond);
                }
                return Ok(TriggerDecision::Skip); // attention busy
            }
            TriggerPolicy::Mention => {
                if !msg.at_bot {
                    return Ok(TriggerDecision::Skip); // not @bot, skip entirely
                }
            }
            TriggerPolicy::Keyword => {
                let keywords = group_config
                    .map(|g| g.reply_policy.keywords.clone())
                    .unwrap_or_default();
                let matched = keywords.iter().any(|kw| {
                    msg.normalized_text.to_lowercase().contains(&kw.to_lowercase())
                });
                if !matched && !msg.at_bot {
                    return Ok(TriggerDecision::Skip);
                }
            }
            TriggerPolicy::Command => {
                if msg.command_name.is_none() && !msg.at_bot {
                    return Ok(TriggerDecision::Skip);
                }
            }
        }

        // Check attention state — if in cooldown, skip entirely
        if !self.attention_store.can_process(msg.qq_group_id).await {
            return Ok(TriggerDecision::Skip);
        }

        // ── Layer 2: LLM evaluation ────────────────────────────────────
        // Build a lightweight prompt asking LLM whether to respond
        let decision = self.llm_decide(msg).await?;

        match decision {
            TriggerDecision::Respond => {
                // Try to engage attention
                if self.attention_store.try_engage(msg.qq_group_id).await {
                    info!(
                        group_id = msg.qq_group_id,
                        user_id = ?msg.qq_user_id,
                        "trigger: LLM decided to respond, attention engaged"
                    );
                    Ok(TriggerDecision::Respond)
                } else {
                    warn!(
                        group_id = msg.qq_group_id,
                        "trigger: LLM decided to respond but attention denied"
                    );
                    Ok(TriggerDecision::Skip)
                }
            }
            other => {
                info!(
                    group_id = msg.qq_group_id,
                    decision = ?other,
                    "trigger: LLM decided to skip/wait"
                );
                Ok(other)
            }
        }
    }

    /// Ask the LLM whether to respond to this message.
    async fn llm_decide(&self, msg: &NormalizedMessage) -> Result<TriggerDecision, QqBotError> {
        let topic_context = match &self.topic_service {
            Some(ts) => ts.get_current_topic_label(msg.qq_group_id).await,
            None => None,
        };

        let topic_hint = match &topic_context {
            Some(label) => format!("\n当前话题：{}", label),
            None => String::new(),
        };

        let emotion_hint = match &self.emotional_service {
            Some(es) => {
                let state = es.get_state(msg.qq_group_id).await;
                format!("\n当前情绪：{}（强度 {:.1}）", state.mood.label(), state.intensity)
            }
            None => String::new(),
        };

        let system_prompt = format!(
            r#"你是{}，一个群聊机器人。你需要判断是否应该回复以下消息。

判断原则：
- 如果消息@了你，或者明确在和你说话，应该回复。
- 如果消息有趣、值得回应，可以回复。
- 如果消息是闲聊、打招呼、表情包轰炸、群内日常水群，可以跳过。
- 如果消息是敏感内容、引战、广告，必须跳过。
- 不要每条消息都回复 — 你只在有充分理由时才回复。
- 如果你的当前情绪状态不适合回复（如疲惫、生气），可以 wait。
- 如果消息和当前话题无关，且话题还在进行中，可以 skip 以免打扰。

请只回复一个单词：respond / wait / skip

当前时间：{}{}{}"#,
            self.persona.nickname,
            Utc::now().format("%Y-%m-%d %H:%M:%S"),
            topic_hint,
            emotion_hint,
        );

        let user_prompt = format!(
            "群 {} | 用户 {}: {}",
            msg.qq_group_id,
            msg.qq_user_id.unwrap_or(0),
            msg.normalized_text,
        );

        let request = ChatCompletionRequest::new(vec![
            crate::domain::llm::ChatMessage {
                role: "system".into(),
                content: system_prompt,
                tool_calls: None,
                tool_call_id: None,
                name: None,
            },
            crate::domain::llm::ChatMessage {
                role: "user".into(),
                content: user_prompt,
                tool_calls: None,
                tool_call_id: None,
                name: None,
            },
        ])
        .with_temperature(0.3);

        let response = self.llm_provider.chat(request).await.map_err(|e| {
            QqBotError::Internal(format!("LLM trigger evaluation failed: {e}"))
        })?;

        let decision_text = response.content.trim().to_lowercase();
        Ok(match decision_text.as_str() {
            s if s.contains("respond") => TriggerDecision::Respond,
            s if s.contains("wait") => TriggerDecision::Wait,
            _ => TriggerDecision::Skip,
        })
    }
}
