use std::collections::HashSet;

use crate::domain::conversation::conversation::Conversation;
use crate::domain::conversation::conversation_message::ConversationMessage;
use crate::domain::tasks::task_event::TurnClosedEvent;
use crate::repositories::RepositorySet;
use crate::test::support::messages::{load_conversation_messages, message_text};

pub struct ConversationSnapshot {
    pub messages: Vec<ConversationMessage>,
    ids: HashSet<u64>,
}

impl ConversationSnapshot {
    pub async fn for_user(repos: &RepositorySet, user_id: u64) -> Self {
        let conversation = repos
            .conv_repo
            .find_single_by_user_id(user_id)
            .await
            .unwrap_or_else(|error| panic!("读取测试用户会话失败: {error}"));
        let messages = load_conversation_messages(repos, conversation.as_ref())
            .await
            .unwrap_or_else(|error| panic!("读取测试用户历史消息失败: {error}"));
        Self::new(conversation, messages)
    }

    pub fn new(_conversation: Option<Conversation>, messages: Vec<ConversationMessage>) -> Self {
        let ids = messages.iter().map(|message| message.id).collect();
        Self { messages, ids }
    }

    pub fn new_messages<'a>(
        &self,
        messages: &'a [ConversationMessage],
    ) -> Vec<&'a ConversationMessage> {
        messages
            .iter()
            .filter(|message| !self.ids.contains(&message.id))
            .collect()
    }
}

pub struct PersistedTurn<'a> {
    pub user_message: &'a ConversationMessage,
    pub assistant_message: &'a ConversationMessage,
}

pub async fn assert_test_user_conversation(
    repos: &RepositorySet,
    conversation_id: u64,
    user_id: u64,
) -> Conversation {
    let conversation = repos
        .conv_repo
        .find_by_id(conversation_id)
        .await
        .unwrap_or_else(|error| panic!("读取 response conversation 失败: {error}"))
        .unwrap_or_else(|| panic!("response conversation 不存在: {conversation_id}"));
    assert_eq!(conversation.user_id, user_id, "response 只能写入测试用户");
    conversation
}

pub fn find_persisted_turn<'a>(
    before: &ConversationSnapshot,
    after_messages: &'a [ConversationMessage],
    user_id: u64,
    marker: &str,
) -> PersistedTurn<'a> {
    let new_messages = before.new_messages(after_messages);
    assert_eq!(new_messages.len(), 2, "本次测试应新增两条消息");

    let user_message = new_messages
        .iter()
        .copied()
        .find(|message| message.sender_role == "user")
        .unwrap_or_else(|| panic!("新增消息中缺少 user 消息"));
    assert_eq!(user_message.sender_user_id, Some(user_id));
    assert!(
        message_text(user_message).contains(marker),
        "新增 user 消息必须包含本次测试 marker"
    );

    let assistant_message = new_messages
        .iter()
        .copied()
        .find(|message| message.sender_role == "assistant")
        .unwrap_or_else(|| panic!("新增消息中缺少 assistant 消息"));
    assert!(
        !message_text(assistant_message).trim().is_empty(),
        "新增 assistant 消息必须非空"
    );

    PersistedTurn {
        user_message,
        assistant_message,
    }
}

pub fn assert_turn_closed_event(
    event: &TurnClosedEvent,
    user_id: u64,
    conversation_id: u64,
    turn: &PersistedTurn<'_>,
) {
    assert_eq!(event.user_id, user_id);
    assert_eq!(event.conversation_id, conversation_id);
    assert_eq!(event.user_message_id, Some(turn.user_message.id));
    assert_eq!(event.assistant_message_id, Some(turn.assistant_message.id));
}

pub fn assert_conversation_unchanged(
    before: &Conversation,
    before_message_rows: usize,
    after: &Conversation,
    after_message_rows: usize,
) {
    assert_eq!(before.id, after.id);
    assert_eq!(
        before.message_count, after.message_count,
        "构建 app 层上下文不允许改变 conversations.message_count"
    );
    assert_eq!(
        before.last_message_at, after.last_message_at,
        "构建 app 层上下文不允许改变 conversations.last_message_at"
    );
    assert_eq!(
        before_message_rows, after_message_rows,
        "构建 app 层上下文不允许新增或删除 conversation_messages"
    );
}
