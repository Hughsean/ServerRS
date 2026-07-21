use super::chat_state::{ChatTurnUpdate, PersistedTurn};
use super::error_adapter::effect_error_from_application;
use super::graph::{AgentEffect, EffectEnvelope, EffectError, EffectExecutor, RunContext};
use super::memory_extraction::{
    MemoryExtractionDispatch, MemoryExtractionRequest, MemoryExtractionSchedulerT,
};
use crate::domain::agent::AgentUpdate;
use crate::domain::conversation::conversation_message::NewConversationMessage;
use crate::domain::conversation::conversation_repo::ConversationRepoT;
use crate::shared::error::AppError;
use async_trait::async_trait;
use std::sync::Arc;

#[derive(Debug, Clone)]
pub enum ChatEffect {
    PersistTurn(PersistTurnEffect),
    ScheduleMemoryExtraction(MemoryExtractionRequest),
}

#[derive(Debug, Clone)]
pub struct PersistTurnEffect {
    pub conversation_id: u64,
    pub user_id: u64,
    pub user: NewConversationMessage,
    pub assistant: NewConversationMessage,
}

#[derive(Debug, Clone)]
pub enum ChatEffectReceipt {
    TurnPersisted(PersistedTurn),
    MemoryExtractionDispatched(MemoryExtractionDispatch),
}

impl AgentEffect for ChatEffect {
    type Update = ChatTurnUpdate;
    type Receipt = ChatEffectReceipt;

    fn receipt_updates(receipt: &Self::Receipt) -> Vec<AgentUpdate<Self::Update>> {
        match receipt {
            ChatEffectReceipt::TurnPersisted(persisted) => vec![AgentUpdate::Business(
                ChatTurnUpdate::SetPersistedTurn(persisted.clone()),
            )],
            ChatEffectReceipt::MemoryExtractionDispatched(_) => Vec::new(),
        }
    }
}

#[async_trait]
pub trait TurnWriterT: Send + Sync {
    async fn save_turn_atomic(
        &self,
        conversation_id: u64,
        user_id: u64,
        user: NewConversationMessage,
        assistant: NewConversationMessage,
    ) -> Result<PersistedTurn, AppError>;
}

pub struct ConversationTurnWriter {
    repository: Arc<dyn ConversationRepoT>,
}

impl ConversationTurnWriter {
    pub fn new(repository: Arc<dyn ConversationRepoT>) -> Self {
        Self { repository }
    }
}

#[async_trait]
impl TurnWriterT for ConversationTurnWriter {
    async fn save_turn_atomic(
        &self,
        conversation_id: u64,
        user_id: u64,
        user: NewConversationMessage,
        assistant: NewConversationMessage,
    ) -> Result<PersistedTurn, AppError> {
        let (user, assistant) = self
            .repository
            .save_turn_atomic(conversation_id, user_id, user, assistant)
            .await?;
        Ok(PersistedTurn::new(user.id, assistant.id))
    }
}

pub struct ChatEffectExecutor {
    writer: Arc<dyn TurnWriterT>,
    memory_extraction_scheduler: Arc<dyn MemoryExtractionSchedulerT>,
}

impl ChatEffectExecutor {
    pub fn new(
        writer: Arc<dyn TurnWriterT>,
        memory_extraction_scheduler: Arc<dyn MemoryExtractionSchedulerT>,
    ) -> Self {
        Self {
            writer,
            memory_extraction_scheduler,
        }
    }
}

#[async_trait]
impl EffectExecutor<ChatEffect> for ChatEffectExecutor {
    async fn execute(
        &self,
        envelope: &EffectEnvelope<ChatEffect>,
        _context: &RunContext,
    ) -> Result<ChatEffectReceipt, EffectError> {
        match &envelope.effect {
            ChatEffect::PersistTurn(effect) => self
                .writer
                .save_turn_atomic(
                    effect.conversation_id,
                    effect.user_id,
                    effect.user.clone(),
                    effect.assistant.clone(),
                )
                .await
                .map(ChatEffectReceipt::TurnPersisted)
                .map_err(effect_error_from_application),
            ChatEffect::ScheduleMemoryExtraction(request) => {
                Ok(ChatEffectReceipt::MemoryExtractionDispatched(
                    self.memory_extraction_scheduler.schedule(request.clone()),
                ))
            }
        }
    }
}
