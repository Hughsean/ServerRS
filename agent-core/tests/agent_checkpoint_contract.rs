use agent_core::graph::{
    AgentCheckpoint, AgentEffect, CheckpointError, CheckpointId, CheckpointStore, EffectReceipt,
    GraphId, GraphVersion, InMemoryCheckpointStore, NoEffect, NodeId, NodeResult, RunBudget, RunId,
    RunPosition, RunStep, RunTrace, StateSchemaVersion, SuspendReason, SuspendRequest, UsageDelta,
    UsageSnapshot,
};
use agent_core::{AgentBusinessState, AgentState, AgentStateError, AgentUpdate};
use std::num::NonZeroU32;
use std::time::Duration;

#[test]
fn checkpoint_versions_are_nonzero_and_default_to_one() {
    assert_eq!(GraphVersion::initial().get(), 1);
    assert_eq!(StateSchemaVersion::initial().get(), 1);
    assert!(GraphVersion::try_from(0).is_err());
    assert!(StateSchemaVersion::try_from(0).is_err());
}

#[test]
fn node_result_suspend_carries_typed_business_data() {
    let result = NodeResult::<(), (), u64>::Suspend {
        updates: Vec::new(),
        effects: Vec::new(),
        usage: UsageDelta::default(),
        request: SuspendRequest::new(SuspendReason::ExternalInput, 42),
    };

    match result {
        NodeResult::Suspend { request, .. } => {
            assert_eq!(request.reason, SuspendReason::ExternalInput);
            assert_eq!(request.data, 42);
        }
        NodeResult::Continue { .. } => panic!("expected a suspend result"),
    }
}

#[derive(Debug, Clone, Default)]
struct CheckpointBusiness {
    value: u32,
}

#[derive(Debug)]
enum CheckpointUpdate {
    Set(u32),
}

impl AgentBusinessState for CheckpointBusiness {
    type Update = CheckpointUpdate;
    type Effect = NoEffect<CheckpointUpdate>;
    type SuspendData = String;
    type ResumeInput = u32;

    fn resume_updates(input: Self::ResumeInput) -> Vec<AgentUpdate<Self::Update>> {
        vec![AgentUpdate::Business(CheckpointUpdate::Set(input))]
    }

    fn apply_update(&mut self, update: Self::Update) -> Result<(), AgentStateError> {
        match update {
            CheckpointUpdate::Set(value) => self.value = value,
        }
        Ok(())
    }
}

type CheckpointReceipt =
    <<CheckpointBusiness as AgentBusinessState>::Effect as AgentEffect>::Receipt;

fn checkpoint() -> AgentCheckpoint<CheckpointBusiness> {
    AgentCheckpoint::new(
        CheckpointId::new(),
        GraphId::try_from("checkpoint-test").unwrap(),
        GraphVersion::initial(),
        StateSchemaVersion::initial(),
        RunId::new(),
        RunPosition::new(
            RunStep::try_from(1).unwrap(),
            NodeId::try_from("resume").unwrap(),
        ),
        AgentState::new(CheckpointBusiness::default()),
        RunBudget::new(NonZeroU32::new(8).unwrap(), Duration::from_secs(30)),
        UsageSnapshot {
            steps: 1,
            llm_calls: 0,
            tool_calls: 0,
            tokens: 3,
        },
        vec![NodeId::try_from("suspend").unwrap()],
        Vec::<EffectReceipt<CheckpointReceipt>>::new(),
        SuspendRequest::new(SuspendReason::ExternalInput, "ticket-42".into()),
        RunTrace::default(),
    )
}

#[tokio::test]
async fn in_memory_store_loads_without_consuming_and_take_is_single_use() {
    let store = InMemoryCheckpointStore::<CheckpointBusiness>::new();
    let checkpoint = checkpoint();
    let id = checkpoint.id();

    store.save(checkpoint).await.unwrap();

    let loaded = store.load(id).await.unwrap();
    assert_eq!(loaded.id(), id);
    assert_eq!(loaded.position().completed_step().get(), 1);
    assert_eq!(loaded.position().next_node().as_str(), "resume");
    assert_eq!(loaded.usage().tokens, 3);
    assert_eq!(loaded.suspend().data, "ticket-42");

    let taken = store.take(id).await.unwrap();
    assert_eq!(taken.id(), id);
    assert!(matches!(
        store.take(id).await,
        Err(CheckpointError::NotFound { checkpoint_id }) if checkpoint_id == id
    ));
    assert!(matches!(
        store.save(taken).await,
        Err(CheckpointError::Duplicate { checkpoint_id }) if checkpoint_id == id
    ));
}

#[tokio::test]
async fn in_memory_store_rejects_duplicate_checkpoint_ids() {
    let store = InMemoryCheckpointStore::<CheckpointBusiness>::new();
    let checkpoint = checkpoint();
    let duplicate = checkpoint.clone();
    let id = checkpoint.id();

    store.save(checkpoint).await.unwrap();

    assert!(matches!(
        store.save(duplicate).await,
        Err(CheckpointError::Duplicate { checkpoint_id }) if checkpoint_id == id
    ));
}
