//! 后端 API 响应的反序列化结构体。
//!
//! 字段名严格匹配后端 serde 默认的 snake_case 序列化
//! (auth/chat/user_dto 均未设 rename_all)。
//!
//! 字段需完整反映后端响应,即使当前未读取(反序列化需要),
//! 故整个模块允许 dead_code。

#![allow(dead_code)]

use serde::Deserialize;

// ── Auth ──
//
// 注意:后端 auth_handler.rs 重新定义了 LoginResponse/RefreshResponse
// (非 auth_dto.rs 的版本),且带 #[serde(rename_all = "camelCase")],
// 故实际 JSON 字段为 camelCase: accessToken / refreshToken / expiresIn / tokenType。
// /auth/me 返回 UserInfo {id, username, role}(无 rename_all,字段名原样)。

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoginResponse {
    pub access_token: String,
    pub refresh_token: String,
    #[serde(default)]
    pub expires_in: u64,
    #[serde(default)]
    pub token_type: String,
    pub user: LoginUser,
}

#[derive(Debug, Deserialize)]
pub struct LoginUser {
    pub id: u64,
    pub username: String,
    #[serde(default)]
    pub role: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RefreshResponse {
    pub access_token: String,
    pub refresh_token: String,
    #[serde(default)]
    pub expires_in: u64,
}

#[derive(Debug, Deserialize)]
pub struct AuthUser {
    pub id: u64,
    pub username: String,
    // 后端 UserInfo 其余字段(role)按需忽略,只取必要部分
}

// ── Chat ──

#[derive(Debug, Deserialize)]
pub struct ChatOpenResponse {
    pub conversation: ChatConversationInfo,
    pub personalization_enabled: bool,
}

#[derive(Debug, Deserialize)]
pub struct ChatConversationInfo {
    pub id: u64,
    pub message_count: u64,
    pub last_message_at: Option<String>,
}

#[derive(Debug, serde::Serialize)]
pub struct ChatMessageRequest {
    pub text: String,
}

#[derive(Debug, Deserialize)]
pub struct ChatMessageResponse {
    pub conversation_id: u64,
    pub reply: String,
    #[serde(default)]
    pub tool_calls: Vec<ChatToolCallItem>,
}

#[derive(Debug, Deserialize)]
pub struct ChatToolCallItem {
    pub name: String,
    pub arguments: serde_json::Value,
}

#[derive(Debug, Deserialize)]
pub struct ChatHistoryResponse {
    pub messages: Vec<ChatMessageItem>,
    pub next_before_id: Option<u64>,
}

#[derive(Debug, Deserialize)]
pub struct ChatMessageItem {
    pub id: u64,
    pub sender_role: String,
    pub message_type: String,
    pub content: serde_json::Value,
    pub created_at: String,
}

#[derive(Debug, Deserialize)]
pub struct ChatMemoryResponse {
    pub memories: Vec<ChatMemoryItem>,
    pub total_active: usize,
}

#[derive(Debug, Deserialize)]
pub struct ChatMemoryItem {
    pub memory_id: u64,
    pub memory_type: String,
    pub content: String,
    pub confidence: f64,
    pub reinforce_count: u32,
    pub created_at: String,
    #[serde(default)]
    pub reinforced_at: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ChatPersonaResponse {
    pub has_active_persona: bool,
    pub generated_at: Option<String>,
    pub snapshot_summary: ChatPersonaSnapshotSummary,
    pub personalization_enabled: bool,
}

#[derive(Debug, Deserialize, Default)]
pub struct ChatPersonaSnapshotSummary {
    #[serde(default)]
    pub communication_preferences_count: usize,
    #[serde(default)]
    pub stable_facts_count: usize,
    #[serde(default)]
    pub recurring_topics_count: usize,
    #[serde(default)]
    pub goals_count: usize,
    #[serde(default)]
    pub sensitive_context_count: usize,
}

#[derive(Debug, Deserialize)]
pub struct PersonaRebuildResponse {
    pub snapshot_id: u64,
}

#[derive(Debug, Deserialize)]
pub struct PersonaResetResponse {
    pub reset: bool,
}

#[derive(Debug, Deserialize)]
pub struct TranscriptClearResponse {
    pub cleared_messages: bool,
    pub cleared_summaries: bool,
    pub memories_preserved: bool,
    pub persona_preserved: bool,
    pub post_risk_audits_cleared: bool,
}

#[derive(Debug, Deserialize)]
pub struct ForgetResponse {
    pub messages_cleared: bool,
    pub summaries_cleared: bool,
    pub memories_disabled: u64,
    pub persona_expired: bool,
    pub post_risk_audits_deleted: bool,
    pub personalization_disabled: bool,
}

// ── User Profile ──

#[derive(Debug, Deserialize)]
pub struct UserProfileResponse {
    pub id: u64,
    pub user_id: u64,
    #[serde(default)]
    pub interests: Option<Vec<String>>,
    #[serde(default)]
    pub personality_traits: Option<Vec<String>>,
    #[serde(default)]
    pub interaction_preferences: Option<Vec<String>>,
    #[serde(default)]
    pub emotional_tendency: Option<Vec<String>>,
    #[serde(default)]
    pub learning_records: Option<Vec<String>>,
    pub created_at: String,
    pub updated_at: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn login_response_deserializes() {
        // 后端实际返回 camelCase + 嵌套 user 对象
        let json = r#"{"accessToken":"a","refreshToken":"r","expiresIn":86400,"tokenType":"Bearer","user":{"id":1,"username":"alice","role":"user"}}"#;
        let r: LoginResponse = serde_json::from_str(json).unwrap();
        assert_eq!(r.access_token, "a");
        assert_eq!(r.refresh_token, "r");
        assert_eq!(r.user.id, 1);
        assert_eq!(r.user.username, "alice");
    }

    #[test]
    fn refresh_response_deserializes_camelCase() {
        let json = r#"{"accessToken":"na","refreshToken":"nr","expiresIn":86400}"#;
        let r: RefreshResponse = serde_json::from_str(json).unwrap();
        assert_eq!(r.access_token, "na");
        assert_eq!(r.refresh_token, "nr");
    }

    #[test]
    fn chat_message_response_defaults_empty_tool_calls() {
        // 后端可能省略空 tool_calls 字段
        let json = r#"{"conversation_id":7,"reply":"hi"}"#;
        let r: ChatMessageResponse = serde_json::from_str(json).unwrap();
        assert_eq!(r.reply, "hi");
        assert!(r.tool_calls.is_empty());
    }

    #[test]
    fn persona_summary_defaults_all_zero_when_missing() {
        let json = r#"{"has_active_persona":false,"generated_at":null,"snapshot_summary":{},"personalization_enabled":false}"#;
        let r: ChatPersonaResponse = serde_json::from_str(json).unwrap();
        assert!(!r.has_active_persona);
        assert_eq!(r.snapshot_summary.stable_facts_count, 0);
    }

    #[test]
    fn user_profile_handles_null_fields() {
        let json = r#"{"id":1,"user_id":1,"interests":null,"personality_traits":null,"interaction_preferences":null,"emotional_tendency":null,"learning_records":null,"created_at":"t","updated_at":"t"}"#;
        let r: UserProfileResponse = serde_json::from_str(json).unwrap();
        assert!(r.interests.is_none());
    }

    #[test]
    fn memory_item_with_optional_reinforced_at() {
        let json = r#"{"memory_id":7,"memory_type":"fact","content":"x","confidence":0.9,"reinforce_count":2,"created_at":"t"}"#;
        let r: ChatMemoryItem = serde_json::from_str(json).unwrap();
        assert!(r.reinforced_at.is_none());
    }
}
