use async_trait::async_trait;
use server_rs::app::agent::chat_effect::{
    ChatEffect, ChatEffectExecutor, ChatEffectReceipt, PersistTurnEffect, TurnWriterT,
};
use server_rs::app::agent::chat_state::{ChatTurnUpdate, PersistedTurn};
use server_rs::app::agent::graph::{
    AgentEffect, EffectEnvelope, EffectExecutor, EffectId, NodeId, RunBudget, RunContext, RunId,
    RunStep, RunTrace,
};
use server_rs::app::agent::memory_extraction::{
    MemoryExtractionDispatch, MemoryExtractionRequest, MemoryExtractionSchedulerT,
};
use server_rs::domain::agent::AgentUpdate;
use server_rs::domain::conversation::conversation_message::NewConversationMessage;
use server_rs::shared::error::AppError;
use std::num::NonZeroU32;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

#[derive(Debug)]
struct RecordedTurn {
    conversation_id: u64,
    user_id: u64,
    user: NewConversationMessage,
    assistant: NewConversationMessage,
}

#[derive(Default)]
struct RecordingTurnWriter {
    calls: AtomicUsize,
    recorded: Mutex<Option<RecordedTurn>>,
}

#[async_trait]
impl TurnWriterT for RecordingTurnWriter {
    async fn save_turn_atomic(
        &self,
        conversation_id: u64,
        user_id: u64,
        user: NewConversationMessage,
        assistant: NewConversationMessage,
    ) -> Result<PersistedTurn, AppError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        *self.recorded.lock().unwrap() = Some(RecordedTurn {
            conversation_id,
            user_id,
            user,
            assistant,
        });
        Ok(PersistedTurn::new(101, 102))
    }
}

struct FailingTurnWriter {
    calls: AtomicUsize,
}

#[derive(Default)]
struct RecordingMemoryScheduler {
    requests: Mutex<Vec<MemoryExtractionRequest>>,
}

impl MemoryExtractionSchedulerT for RecordingMemoryScheduler {
    fn schedule(&self, request: MemoryExtractionRequest) -> MemoryExtractionDispatch {
        self.requests.lock().unwrap().push(request);
        MemoryExtractionDispatch::Scheduled
    }
}

#[async_trait]
impl TurnWriterT for FailingTurnWriter {
    async fn save_turn_atomic(
        &self,
        _conversation_id: u64,
        _user_id: u64,
        _user: NewConversationMessage,
        _assistant: NewConversationMessage,
    ) -> Result<PersistedTurn, AppError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Err(AppError::Conflict("turn changed".into()))
    }
}

fn message(
    conversation_id: u64,
    sender_role: &str,
    sender_user_id: Option<u64>,
    content: serde_json::Value,
) -> NewConversationMessage {
    NewConversationMessage {
        conversation_id,
        sender_role: sender_role.into(),
        sender_user_id,
        message_type: "text".into(),
        content: content.to_string(),
        token_count: None,
    }
}

fn persist_turn() -> PersistTurnEffect {
    PersistTurnEffect {
        conversation_id: 9,
        user_id: 7,
        user: message(
            9,
            "user",
            Some(7),
            serde_json::json!({"text": "hello", "emotion": "calm"}),
        ),
        assistant: message(9, "assistant", None, serde_json::json!({"text": "world"})),
    }
}

fn envelope() -> EffectEnvelope<ChatEffect> {
    EffectEnvelope {
        id: EffectId::new(
            RunId::new(),
            RunStep::try_from(1).unwrap(),
            NodeId::try_from("persist_turn").unwrap(),
            0,
        ),
        effect: ChatEffect::PersistTurn(persist_turn()),
    }
}

fn run_context() -> RunContext {
    RunContext::new(
        RunBudget::new(NonZeroU32::new(8).unwrap(), Duration::from_secs(30)),
        CancellationToken::new(),
        RunTrace::default(),
    )
}

#[tokio::test]
async fn chat_effect_executor_writes_once_and_returns_typed_receipt() {
    let writer = Arc::new(RecordingTurnWriter::default());
    let executor = ChatEffectExecutor::new(
        writer.clone(),
        Arc::new(RecordingMemoryScheduler::default()),
    );

    let receipt = executor.execute(&envelope(), &run_context()).await.unwrap();

    assert_eq!(writer.calls.load(Ordering::SeqCst), 1);
    let recorded = writer.recorded.lock().unwrap();
    let recorded = recorded.as_ref().unwrap();
    assert_eq!(recorded.conversation_id, 9);
    assert_eq!(recorded.user_id, 7);
    assert_eq!(recorded.user.conversation_id, 9);
    assert_eq!(recorded.user.sender_role, "user");
    assert_eq!(recorded.user.sender_user_id, Some(7));
    assert_eq!(recorded.user.message_type, "text");
    assert_eq!(recorded.user.token_count, None);
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&recorded.user.content).unwrap(),
        serde_json::json!({"text": "hello", "emotion": "calm"})
    );
    assert_eq!(recorded.assistant.conversation_id, 9);
    assert_eq!(recorded.assistant.sender_role, "assistant");
    assert_eq!(recorded.assistant.sender_user_id, None);
    assert_eq!(recorded.assistant.message_type, "text");
    assert_eq!(recorded.assistant.token_count, None);
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&recorded.assistant.content).unwrap(),
        serde_json::json!({"text": "world"})
    );
    match receipt {
        ChatEffectReceipt::TurnPersisted(persisted) => {
            assert_eq!(persisted.user_message_id(), 101);
            assert_eq!(persisted.assistant_message_id(), 102);
        }
        ChatEffectReceipt::MemoryExtractionDispatched(_) => {
            panic!("expected turn persistence receipt")
        }
    }
}

#[test]
fn persisted_receipt_becomes_one_business_update() {
    let receipt = ChatEffectReceipt::TurnPersisted(PersistedTurn::new(101, 102));
    let updates = ChatEffect::receipt_updates(&receipt);

    match updates.as_slice() {
        [AgentUpdate::Business(ChatTurnUpdate::SetPersistedTurn(persisted))] => {
            assert_eq!(persisted.user_message_id(), 101);
            assert_eq!(persisted.assistant_message_id(), 102);
        }
        _ => panic!("expected one SetPersistedTurn business update"),
    }
}

#[test]
fn memory_dispatch_receipt_does_not_mutate_chat_state() {
    let receipt = ChatEffectReceipt::MemoryExtractionDispatched(
        MemoryExtractionDispatch::SkippedRecentFailure,
    );

    assert!(ChatEffect::receipt_updates(&receipt).is_empty());
}

#[tokio::test]
async fn chat_effect_executor_preserves_writer_application_error() {
    let writer = Arc::new(FailingTurnWriter {
        calls: AtomicUsize::new(0),
    });
    let executor = ChatEffectExecutor::new(
        writer.clone(),
        Arc::new(RecordingMemoryScheduler::default()),
    );

    let error = executor
        .execute(&envelope(), &run_context())
        .await
        .unwrap_err();

    assert_eq!(writer.calls.load(Ordering::SeqCst), 1);
    assert!(matches!(
        error.application_error(),
        Some(AppError::Conflict(message)) if message == "turn changed"
    ));
}

#[tokio::test]
async fn chat_effect_executor_dispatches_memory_extraction_request() {
    let writer = Arc::new(RecordingTurnWriter::default());
    let scheduler = Arc::new(RecordingMemoryScheduler::default());
    let executor = ChatEffectExecutor::new(writer, scheduler.clone());
    let request = MemoryExtractionRequest {
        user_id: 7,
        conversation_id: 9,
        source_message_id: 101,
        user_message: "hello".into(),
        assistant_reply: "world".into(),
        context_version: 23,
    };
    let envelope = EffectEnvelope {
        id: EffectId::new(
            RunId::new(),
            RunStep::try_from(2).unwrap(),
            NodeId::try_from("schedule_memory_extraction").unwrap(),
            0,
        ),
        effect: ChatEffect::ScheduleMemoryExtraction(request.clone()),
    };

    let receipt = executor.execute(&envelope, &run_context()).await.unwrap();

    assert_eq!(scheduler.requests.lock().unwrap().as_slice(), &[request]);
    assert!(matches!(
        receipt,
        ChatEffectReceipt::MemoryExtractionDispatched(MemoryExtractionDispatch::Scheduled)
    ));
}
