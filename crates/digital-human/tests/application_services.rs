use std::cmp::Reverse;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use chrono::Utc;
use digital_human::app::risk::risk_stats_service::RiskStatsService;
use digital_human::app::session::chat_query_service::ChatQueryService;
use digital_human::domain::conversation::conversation::{Conversation, NewConversation};
use digital_human::domain::conversation::conversation_message::{
    ConversationMessage, NewConversationMessage,
};
use digital_human::domain::conversation::conversation_repo::ConversationRepoT;
use digital_human::domain::risk::post_conversation_risk_audit::{
    NewPostConversationRiskAudit, PostConversationRiskAudit, PostRiskAuditResult,
};
use digital_human::domain::risk::risk_repo::RiskRepoT;
use digital_human::shared::error::AppError;

#[tokio::test]
async fn chat_history_service_paginates_the_active_user_conversation() {
    let repo = Arc::new(FakeConversationRepo::with_messages(51));
    let service = ChatQueryService::new(repo.clone());

    let page = service.history(7, None, 50).await.unwrap();

    assert_eq!(page.messages.len(), 50);
    assert_eq!(page.messages[0].id, 51);
    assert_eq!(page.next_before_id, Some(51));
    assert_eq!(repo.history_queries(), vec![(11, None, 50)]);
}

#[tokio::test]
async fn chat_history_service_returns_an_empty_page_without_a_conversation() {
    let service = ChatQueryService::new(Arc::new(FakeConversationRepo::without_conversation()));

    let page = service.history(7, None, 50).await.unwrap();

    assert!(page.messages.is_empty());
    assert_eq!(page.next_before_id, None);
}

#[tokio::test]
async fn risk_stats_service_builds_a_summary_for_the_requested_window() {
    let repo = Arc::new(FakeRiskRepo::default());
    let service = RiskStatsService::new(repo.clone());

    let summary = service.summary(7).await.unwrap();

    assert_eq!(summary.total, 12);
    assert_eq!(summary.trend, vec![("2026-07-18".into(), 3)]);
    assert_eq!(summary.distribution, vec![("high".into(), 2)]);
    assert_eq!(repo.requested_days(), vec![7]);
}

struct FakeConversationRepo {
    conversation: Option<Conversation>,
    messages: Vec<ConversationMessage>,
    history_queries: Mutex<Vec<(u64, Option<u64>, u64)>>,
}

impl FakeConversationRepo {
    fn with_messages(count: u64) -> Self {
        Self {
            conversation: Some(Conversation {
                id: 11,
                user_id: 7,
                title: None,
                last_message_at: None,
                message_count: count as i32,
                created_at: Utc::now(),
            }),
            messages: (1..=count).map(test_message).collect(),
            history_queries: Mutex::new(Vec::new()),
        }
    }

    fn without_conversation() -> Self {
        Self {
            conversation: None,
            messages: Vec::new(),
            history_queries: Mutex::new(Vec::new()),
        }
    }

    fn history_queries(&self) -> Vec<(u64, Option<u64>, u64)> {
        self.history_queries.lock().unwrap().clone()
    }
}

#[async_trait]
impl ConversationRepoT for FakeConversationRepo {
    async fn find_by_id(&self, _id: u64) -> Result<Option<Conversation>, AppError> {
        unimplemented!("not used by ChatQueryService")
    }

    async fn find_by_user_id(&self, _user_id: u64) -> Result<Vec<Conversation>, AppError> {
        unimplemented!("not used by ChatQueryService")
    }

    async fn find_or_create_for_user(&self, _user_id: u64) -> Result<Conversation, AppError> {
        unimplemented!("not used by ChatQueryService")
    }

    async fn find_single_by_user_id(
        &self,
        _user_id: u64,
    ) -> Result<Option<Conversation>, AppError> {
        Ok(self.conversation.clone())
    }

    async fn save(&self, _conv: NewConversation) -> Result<Conversation, AppError> {
        unimplemented!("not used by ChatQueryService")
    }

    async fn update_title(&self, _id: u64, _title: &str) -> Result<(), AppError> {
        unimplemented!("not used by ChatQueryService")
    }

    async fn touch_and_incr(&self, _id: u64, _inc: u64) -> Result<(), AppError> {
        unimplemented!("not used by ChatQueryService")
    }

    async fn delete_by_id(&self, _id: u64) -> Result<bool, AppError> {
        unimplemented!("not used by ChatQueryService")
    }

    async fn save_message(
        &self,
        _msg: NewConversationMessage,
    ) -> Result<ConversationMessage, AppError> {
        unimplemented!("not used by ChatQueryService")
    }

    async fn find_messages_by_conversation_id(
        &self,
        _conversation_id: u64,
    ) -> Result<Vec<ConversationMessage>, AppError> {
        unimplemented!("not used by ChatQueryService")
    }

    async fn delete_messages_by_conversation_id(
        &self,
        _conversation_id: u64,
    ) -> Result<u64, AppError> {
        unimplemented!("not used by ChatQueryService")
    }

    async fn find_messages_before(
        &self,
        conversation_id: u64,
        before_id: Option<u64>,
        limit: u64,
    ) -> Result<Vec<ConversationMessage>, AppError> {
        self.history_queries
            .lock()
            .unwrap()
            .push((conversation_id, before_id, limit));
        let mut messages = self
            .messages
            .iter()
            .filter(|message| before_id.is_none_or(|before| message.id < before))
            .cloned()
            .collect::<Vec<_>>();
        messages.sort_by_key(|message| Reverse(message.id));
        messages.truncate(limit as usize);
        Ok(messages)
    }

    async fn find_messages_since(
        &self,
        _conversation_id: u64,
        _since_id: u64,
    ) -> Result<Vec<ConversationMessage>, AppError> {
        unimplemented!("not used by ChatQueryService")
    }

    async fn find_messages_by_ids(
        &self,
        _conversation_id: u64,
        _message_ids: &[u64],
    ) -> Result<Vec<ConversationMessage>, AppError> {
        unimplemented!("not used by ChatQueryService")
    }

    async fn save_turn_atomic(
        &self,
        _conversation_id: u64,
        _user_id: u64,
        _user_msg: NewConversationMessage,
        _assistant_msg: NewConversationMessage,
    ) -> Result<(ConversationMessage, ConversationMessage), AppError> {
        unimplemented!("not used by ChatQueryService")
    }
}

#[derive(Default)]
struct FakeRiskRepo {
    requested_days: Mutex<Vec<u32>>,
}

impl FakeRiskRepo {
    fn requested_days(&self) -> Vec<u32> {
        self.requested_days.lock().unwrap().clone()
    }
}

#[async_trait]
impl RiskRepoT for FakeRiskRepo {
    async fn create_pending(
        &self,
        _new_audit: NewPostConversationRiskAudit,
    ) -> Result<PostConversationRiskAudit, AppError> {
        unimplemented!("not used by RiskStatsService")
    }

    async fn fetch_pending(&self, _limit: u64) -> Result<Vec<PostConversationRiskAudit>, AppError> {
        unimplemented!("not used by RiskStatsService")
    }

    async fn mark_running(&self, _audit_id: u64) -> Result<(), AppError> {
        unimplemented!("not used by RiskStatsService")
    }

    async fn mark_completed(
        &self,
        _audit_id: u64,
        _result: PostRiskAuditResult,
    ) -> Result<(), AppError> {
        unimplemented!("not used by RiskStatsService")
    }

    async fn mark_failed(&self, _audit_id: u64, _error_message: String) -> Result<(), AppError> {
        unimplemented!("not used by RiskStatsService")
    }

    async fn delete_for_user(&self, _user_id: u64) -> Result<u64, AppError> {
        unimplemented!("not used by RiskStatsService")
    }

    async fn delete_for_conversation(&self, _conversation_id: u64) -> Result<u64, AppError> {
        unimplemented!("not used by RiskStatsService")
    }

    async fn find_by_user_id_paginated(
        &self,
        _user_id: u64,
        _limit: u64,
        _offset: u64,
    ) -> Result<(Vec<PostConversationRiskAudit>, u64), AppError> {
        unimplemented!("not used by RiskStatsService")
    }

    async fn find_by_conversation_id(
        &self,
        _conversation_id: u64,
    ) -> Result<Vec<PostConversationRiskAudit>, AppError> {
        unimplemented!("not used by RiskStatsService")
    }

    async fn find_all_paginated(
        &self,
        _limit: u64,
        _offset: u64,
        _risk_level: Option<&str>,
    ) -> Result<(Vec<PostConversationRiskAudit>, u64), AppError> {
        unimplemented!("not used by RiskStatsService")
    }

    async fn find_conversation_ids_paginated(
        &self,
        _limit: u64,
        _offset: u64,
        _risk_level: Option<&str>,
    ) -> Result<(Vec<u64>, u64), AppError> {
        unimplemented!("not used by RiskStatsService")
    }

    async fn count_all(&self) -> Result<u64, AppError> {
        Ok(12)
    }

    async fn count_trend(&self, days: u32) -> Result<Vec<(String, u64)>, AppError> {
        self.requested_days.lock().unwrap().push(days);
        Ok(vec![("2026-07-18".into(), 3)])
    }

    async fn count_by_risk_level(&self) -> Result<Vec<(String, u64)>, AppError> {
        Ok(vec![("high".into(), 2)])
    }
}

fn test_message(id: u64) -> ConversationMessage {
    ConversationMessage {
        id,
        conversation_id: 11,
        sender_role: "user".into(),
        sender_user_id: Some(7),
        message_type: "text".into(),
        content: r#"{\"text\":\"message\"}"#.into(),
        token_count: None,
        created_at: Utc::now(),
    }
}
