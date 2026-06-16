use std::sync::Arc;

use tracing::info;

use crate::app::qq_bot::emotional_state_service::EmotionalStateService;
use crate::domain::llm::ChatMessage;
use crate::domain::qq_bot::config::GroupConfig;
use crate::domain::qq_bot::message::NormalizedMessage;
use crate::domain::qq_bot::persona::BotPersona;
use crate::domain::qq_bot::repository::{
    GroupMemberRepository, GroupMessageRepository, GroupMemoryRepository, GroupSummaryRepository,
};
use crate::domain::qq_bot::QqBotError;

/// Builds conversation context for the LLM when generating a reply.
///
/// Assembles:
/// - System prompt with persona definition
/// - Time and emotional state awareness
/// - Recent N messages from the group
/// - Active group summary (rolling)
/// - Known group memories
/// - Current attention state info
pub struct ContextBuilder {
    message_repo: Arc<dyn GroupMessageRepository>,
    member_repo: Arc<dyn GroupMemberRepository>,
    summary_repo: Arc<dyn GroupSummaryRepository>,
    memory_repo: Arc<dyn GroupMemoryRepository>,
    persona: BotPersona,
    max_recent_messages: u32,
    emotional_service: Option<Arc<EmotionalStateService>>,
}

impl ContextBuilder {
    pub fn new(
        message_repo: Arc<dyn GroupMessageRepository>,
        member_repo: Arc<dyn GroupMemberRepository>,
        summary_repo: Arc<dyn GroupSummaryRepository>,
        memory_repo: Arc<dyn GroupMemoryRepository>,
        persona: BotPersona,
        max_recent_messages: u32,
        emotional_service: Option<Arc<EmotionalStateService>>,
    ) -> Self {
        Self {
            message_repo,
            member_repo,
            summary_repo,
            memory_repo,
            persona,
            max_recent_messages,
            emotional_service,
        }
    }

    /// Build the full context (system prompt + recent history) for reply generation.
    ///
    /// Returns a `Vec<ChatMessage>` where the first element is the system prompt,
    /// followed by alternating user/assistant messages representing recent history.
    pub async fn build_context(
        &self,
        msg: &NormalizedMessage,
        group_config: Option<&GroupConfig>,
    ) -> Result<Vec<ChatMessage>, QqBotError> {
        let mut messages: Vec<ChatMessage> = Vec::new();

        // ── Recent history (loaded first for conversation analysis) ────
        let recent = self.message_repo
            .recent_by_group(msg.qq_group_id, self.max_recent_messages)
            .await
            .map_err(|e| QqBotError::Internal(format!("failed to load recent messages: {e}")))?;

        // ── System prompt ──────────────────────────────────────────────
        let system = self.build_system_prompt(msg, group_config, &recent).await?;
        messages.push(ChatMessage {
            role: "system".into(),
            content: system,
            tool_calls: None,
            tool_call_id: None,
            name: None,
        });

        for hist_msg in &recent {
            // Skip the triggering message itself (it'll be added as user input)
            if hist_msg.id == msg.id {
                continue;
            }

            let role = match hist_msg.direction {
                crate::domain::qq_bot::message::MessageDirection::Inbound => "user",
                crate::domain::qq_bot::message::MessageDirection::Outbound => "assistant",
            };

            let display_name = if role == "user" {
                self.resolve_sender_name(hist_msg.qq_group_id, hist_msg.qq_user_id.unwrap_or(0)).await
            } else {
                self.persona.nickname.clone()
            };

            let content = format!("[{}] {}", display_name, hist_msg.normalized_text);
            messages.push(ChatMessage {
                role: role.into(),
                content,
                tool_calls: None,
                tool_call_id: None,
                name: None,
            });
        }

        info!(
            group_id = msg.qq_group_id,
            context_size = messages.len(),
            "context built for reply generation"
        );

        Ok(messages)
    }

    /// Build the system prompt incorporating persona, group context, memories, and summary.
    async fn build_system_prompt(
        &self,
        msg: &NormalizedMessage,
        _group_config: Option<&GroupConfig>,
        recent_messages: &[NormalizedMessage],
    ) -> Result<String, QqBotError> {
        let mut parts: Vec<String> = Vec::new();

        // Identity
        parts.push(format!(
            r#"你是{}，一个活跃在QQ群聊中的AI猫娘机器人。
你的性格特点是：{}

## 行为规则
{}"#,
            self.persona.nickname,
            self.persona.tone.style,
            self.persona.rules.join("\n"),
        ));

        // Tone guidance
        let tone = &self.persona.tone;
        parts.push(format!(
            r#"## 语气风格
- 使用"{}"作为称呼后缀
- 适度使用颜文字（倾向度：{}%）
- 可适度使用表情（倾向度：{}%）
- 典型语气：{}"#,
            tone.address_pattern,
            (tone.kaomoji_affinity * 100.0) as u32,
            (tone.sticker_affinity * 100.0) as u32,
            tone.style,
        ));

        // ── 感知状态（时间 + 情绪，紧凑一段） ──
        let now_ts = chrono::Utc::now().timestamp();
        let last_msg_minutes_ago = ((now_ts - msg.sent_at) / 60).max(0) as u64;

        let temporal = crate::domain::qq_bot::bot_state::TemporalContext::from_now(
            last_msg_minutes_ago,
            None,
            false,
            0,
        );

        let emotion_text = if let Some(ref es) = self.emotional_service {
            let state = es.get_state(msg.qq_group_id).await;
            state.describe()
        } else {
            String::new()
        };

        let mut state_line = format!("当前时间：{} {}", temporal.date_str.clone(), chrono::Local::now().format("%H:%M").to_string());
        if !emotion_text.is_empty() {
            state_line.push_str(&format!("\n{}", emotion_text));
            state_line.push_str("\n你的情绪会影响你的语气，但不要让情绪完全主导行为。");
        }

        // ── 对话状态（防重复、防刷屏） ──
        let conv = crate::domain::qq_bot::conversation_state::ConversationState::from_recent_messages(
            recent_messages,
            0,
        );
        let conv_text = conv.describe();
        if !conv_text.is_empty() {
            state_line.push_str(&format!("\n{}", conv_text));
        }

        parts.push(state_line);

        // Group summary (if available)
        if let Ok(Some(summary)) = self.summary_repo.find_active_rolling(msg.qq_group_id).await {
            if !summary.content.is_empty() {
                parts.push(format!("## 群聊上下文摘要\n{}", summary.content));
            }
        }

        // Group memories (if available)
        if let Ok(memories) = self.memory_repo.find_active_by_group(msg.qq_group_id, 5).await {
            if !memories.is_empty() {
                let mem_lines: Vec<String> = memories
                    .iter()
                    .map(|m| format!("- {} (置信度: {:.2})", m.content, m.confidence))
                    .collect();
                parts.push(format!("## 群聊记忆\n{}", mem_lines.join("\n")));
            }
        }

        // ── 话题状态 ──
        let conv_state = crate::domain::qq_bot::conversation_state::ConversationState::from_recent_messages(
            recent_messages,
            0, // bot_qq_id — 暂不校验
        );
        let conv_text = conv_state.describe();
        if !conv_text.is_empty() {
            parts.push(format!("## 对话状态\n{}", conv_text));
        }

        // ── 情绪表现指导 ──
        if !emotion_text.is_empty() {
            parts.push(
                r#"## 情绪对回复的影响
你的情绪会影响你的语气，但不要让情绪完全主导行为。"#.to_string(),
            );
        }

        // Format rules
        parts.push(
            r#"## 回复格式
你必须以 JSON 格式回复，格式如下：

{
  "segments": [
    { "type": "text", "content": "说话内容" },
    { "type": "emoji", "id": 123 },
    { "type": "kaomoji", "text": "(´▽`ʃ♡ƪ)" },
    { "type": "image", "path": "https://example.com/cat.jpg" },
    { "type": "quote_reply", "message_id": 123456, "text": "回复内容" },
    { "type": "poke", "user_id": 1234567890, "group_id": 123456789 },
    { "type": "record", "text": "语音内容", "voice": "zh_female_qiaopi" }
  ],
  "timing_hint": {
    "initial_delay_ms": 1500,
    "inter_segment_delays_ms": [800, 1200],
    "show_typing": true
  },
  "emotion_change": {
    "mood": "happy",
    "intensity": 0.7,
    "reason": "群友夸了猫猫"
  }
}

可用的回复类型：
- text：普通文本
- emoji：QQ 表情（id 为数字）
- kaomoji：颜文字
- image：图片（path 为 URL）
- quote_reply：引用回复（message_id 和 text 都必需）
- poke：拍一拍群成员（user_id 为目标成员的 QQ 号，group_id 为群号）
- record：语音消息（text 为要说的话，voice 为音色 ID，可用音色：zh_female_qingxin 清新、zh_female_qiaopi 俏皮、zh_female_tianmei 甜美、zh_male_chunhou 醇厚等）

可用的情绪（emotion_change.mood）：
- happy：开心
- neutral：平静
- sad：难过
- angry：生气
- surprised：惊讶
- tired：疲惫

要求：
- segments 数组包含 1~4 段回复
- 每段 text 不超过 40 字
- 多段回复之间用不同内容分段，模拟真人分次发送
- poke 应适度使用，不要频繁拍群友
- record 用于想用语音表达的内容
- timing_hint 控制发送节奏
- emotion_change 告诉猫猫现在的心情，可选但建议填写，intensity 填 0.0~1.0
- 不要在回复中包含本格式说明的思考过程
- 回复要像是群聊中的真实发言，不要像机器人朗诵"#.to_string(),
        );

        Ok(parts.join("\n\n"))
    }

    /// Resolve a display name for a QQ user (card > nickname > QQ号).
    async fn resolve_sender_name(&self, group_id: i64, user_id: i64) -> String {
        if let Ok(Some(member)) = self.member_repo.find(group_id, user_id).await {
            return member
                .card
                .or(member.nickname)
                .unwrap_or_else(|| format!("{}", user_id));
        }
        format!("{}", user_id)
    }
}
