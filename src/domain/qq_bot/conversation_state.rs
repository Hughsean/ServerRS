use serde::{Deserialize, Serialize};

/// 当前对话状态（每个回复周期构建）
///
/// 不分析话题内容（LLM 自己会看消息列表），只提供 LLM 从消息列表里看不到的信息：
/// - 自己上一条说了什么（防重复）
/// - 自己在这个对话里说了几句（防刷屏）
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ConversationState {
    /// 自己上一条回复的摘要（防重复）
    pub last_self_message_summary: Option<String>,
    /// 猫猫在本轮对话中已经说了几句
    pub self_message_count_in_thread: u32,
}

impl ConversationState {
    pub fn from_recent_messages(
        recent_messages: &[crate::domain::qq_bot::message::NormalizedMessage],
        _bot_qq_id: i64,
    ) -> Self {
        // 找出猫猫自己发的最后一条消息
        let last_self_msg = recent_messages.iter().rev().find(|m| {
            m.direction == crate::domain::qq_bot::message::MessageDirection::Outbound
        });

        let last_self_message_summary = last_self_msg.map(|m| {
            let text: String = m.normalized_text.chars().take(30).collect();
            if text.len() < m.normalized_text.len() {
                format!("{}…", text)
            } else {
                text
            }
        });

        // 统计猫猫在最后 10 条中说了几句
        let self_msg_count = recent_messages.iter().rev().take(10).filter(|m| {
            m.direction == crate::domain::qq_bot::message::MessageDirection::Outbound
        }).count() as u32;

        Self {
            last_self_message_summary,
            self_message_count_in_thread: self_msg_count,
        }
    }

    pub fn describe(&self) -> String {
        let mut parts = Vec::new();

        parts.push(format!(
            "你已经在当前对话中说了 {} 次",
            self.self_message_count_in_thread
        ));

        if let Some(ref summary) = self.last_self_message_summary {
            parts.push(format!("你上一句是：\"{}\"", summary));
        }

        parts.join("\n")
    }
}
