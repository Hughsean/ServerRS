use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder};
use serde_json::Value;

use crate::bootstrap::repos::RepoGraph;
use crate::domain::conversation::conversation::Conversation;
use crate::domain::conversation::conversation_message::ConversationMessage;
use crate::domain::llm::ChatMessage;
use crate::infra::db::entities::conversation_messages;
use crate::shared::config::AppConfig;
use crate::shared::error::AppError;

pub const TEST_USER_ID_ENV: &str = "CHAT_TEST_USER_ID";
pub const TEST_QUERY_ENV: &str = "CHAT_TEST_QUERY";

pub async fn load_conversation_messages(
    repos: &RepoGraph,
    conversation: Option<&Conversation>,
) -> Result<Vec<ConversationMessage>, AppError> {
    let Some(conversation) = conversation else {
        return Ok(Vec::new());
    };
    repos
        .conv_repo
        .find_messages_by_conversation_id(conversation.id)
        .await
}

pub async fn select_test_conversation(db: &DatabaseConnection, repos: &RepoGraph) -> Conversation {
    if let Some(user_id) = std::env::var(TEST_USER_ID_ENV)
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
    {
        return repos
            .conv_repo
            .find_single_by_user_id(user_id)
            .await
            .unwrap_or_else(|error| panic!("按 {TEST_USER_ID_ENV} 读取会话失败: {error}"))
            .unwrap_or_else(|| {
                panic!("用户 {user_id} 没有已有会话，请换一个测试用户或先准备数据")
            });
    }

    let latest_message = conversation_messages::Entity::find()
        .filter(conversation_messages::Column::MessageType.eq("text"))
        .filter(conversation_messages::Column::SenderRole.is_in(["user", "assistant", "system"]))
        .order_by_desc(conversation_messages::Column::Id)
        .one(db)
        .await
        .unwrap_or_else(|error| panic!("自动选择测试会话失败: {error}"))
        .unwrap_or_else(|| {
            panic!("数据库中没有可用于 app 层测试的 text 对话消息；也可以设置 {TEST_USER_ID_ENV}")
        });

    repos
        .conv_repo
        .find_by_id(latest_message.conversation_id)
        .await
        .unwrap_or_else(|error| panic!("读取自动选择的会话失败: {error}"))
        .unwrap_or_else(|| panic!("消息引用的会话不存在: {}", latest_message.conversation_id))
}

pub async fn load_recent_chat_messages(
    repos: &RepoGraph,
    conversation_id: u64,
    config: &AppConfig,
) -> Vec<ChatMessage> {
    let limit = u64::from(config.agent.max_context_messages.max(2) - 1);
    let messages = repos
        .conv_repo
        .find_messages_before(conversation_id, None, limit)
        .await
        .unwrap_or_else(|error| panic!("读取最近会话消息失败: {error}"));
    let converted = messages
        .into_iter()
        .filter_map(conversation_message_to_chat_message)
        .collect::<Vec<_>>();
    assert!(
        !converted.is_empty(),
        "测试会话 {conversation_id} 没有可转换为 ChatMessage 的 text 消息"
    );
    converted
}

pub fn conversation_message_to_chat_message(message: ConversationMessage) -> Option<ChatMessage> {
    if !matches!(
        message.sender_role.as_str(),
        "system" | "user" | "assistant"
    ) {
        return None;
    }
    if message.message_type != "text" {
        return None;
    }

    let content = message_text(&message);
    if content.trim().is_empty() {
        return None;
    }

    Some(ChatMessage {
        role: message.sender_role,
        content,
        tool_calls: None,
        tool_call_id: None,
        name: None,
    })
}

pub fn message_text(message: &ConversationMessage) -> String {
    let Ok(value) = serde_json::from_str::<Value>(&message.content) else {
        return message.content.clone();
    };

    let mut text = value
        .get("text")
        .and_then(Value::as_str)
        .or_else(|| value.as_str())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| message.content.clone());

    if message.sender_role == "user" {
        if let Some(emotion) = value
            .get("emotion")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|emotion| !emotion.is_empty())
        {
            text = format!("{text}\n\n[user emotion: {emotion}]");
        }
    }

    text
}
