# Agent Effect Persistence Slice Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a typed Agent Effect execution path and migrate only Chat turn persistence from direct node I/O to `ChatEffectExecutor` without changing public chat behavior.

**Architecture:** `AgentBusinessState` gains an unconstrained associated `Effect` type so Domain does not depend on App. `GraphRuntime` constrains that type with the application-layer `AgentEffect` protocol, assigns stable `EffectId`s, executes Effects once, applies Receipt-derived Updates to a candidate State, and exposes Receipts in the run result. `PersistTurnNode` becomes pure while `ChatEffectExecutor` owns `TurnWriterT` and performs the existing atomic write.

**Tech Stack:** Rust 2024, Tokio, `async-trait`, existing ServerRS graph runtime, existing `AppError`, built-in Rust tests and Cargo.

## Global Constraints

- Work only in `D:\WorkSpace\ServerRS\.worktrees\agent-effect-persistence` on `codex/agent-effect-persistence`.
- Base commit is `793a54c319d5f08095d6783bb82474f416da608f`.
- Do not modify database tables, migrations, configuration, dependencies, environment variables, or public HTTP contracts.
- Do not claim cross-process or cross-Run idempotency; do not add any automatic retry loop.
- Do not implement Checkpoint, Suspend, Resume, LLM Effects, tool Effects, context Effects, or Agent Event Effects.
- Do not modify paths containing `qq_bot` or `napcat`, including `src/app/qq_bot/**`, `src/infra/qq_bot/**`, `src/bootstrap/qq_bot.rs`, `src/bootstrap/graph/qq_bot_provider.rs`, and `src/bin/qq_bot_init.rs`.
- Preserve `AgentRuntime::new`, `AgentRuntime::respond`, response text, tool traces, persisted message JSON, and original `AppError` variants.
- Use test-first red-green-refactor for every behavior change; each red run must fail for the named missing behavior before production code is written.
- The required build gate is the default non-QQ feature set. The base already fails `--all-features` in two excluded QQ Bot imports; do not repair or mask that failure.
- Use Chinese Git commit messages.

---

## File Structure

### New files

- `src/app/agent/graph/effect.rs`: Effect identity, protocol, Envelope, Receipt, executor, error classification, and `NoEffect<U>`.
- `src/app/agent/chat_effect.rs`: Chat persistence Effect descriptions, typed Receipt conversion, Writer port/adapter, and executor.
- `tests/agent_effect_contract.rs`: Public Effect ID and `NodeResult` contract tests.
- `tests/chat_effect_persistence.rs`: Chat Effect executor, payload, Receipt, and application-error contract tests.

### Existing files with focused changes

- `src/domain/agent/state.rs`: add `AgentBusinessState::Effect` without an App-layer trait bound.
- `src/app/agent/graph/node.rs`: make `NodeResult` carry `Vec<E>`.
- `src/app/agent/graph/budget.rs`: return a typed `RunStep` when reserving a step and expose an active-run check that does not reserve another step.
- `src/app/agent/graph/runtime.rs`: execute Effects, maintain candidate State, collect Receipts, and enforce cancellation/error semantics.
- `src/app/agent/graph/error.rs`: add Effect-specific `GraphRunError` variants.
- `src/app/agent/graph/mod.rs`: export Effect types and update graph-only test businesses.
- `src/app/agent/graph/fragment.rs`: update wrapped node result signatures and test businesses.
- `src/app/agent/chat_state.rs`: associate `ChatTurnState` with `ChatEffect` after the vertical slice is introduced.
- `src/app/agent/nodes/chat_turn.rs`: make `PersistTurnNode` pure and move Writer concerns out.
- `src/app/agent/nodes/chat_context.rs`, `src/app/agent/nodes/reasoning.rs`: only update `NodeResult` type signatures; retain their current I/O behavior.
- `src/app/agent/nodes/mod.rs`, `src/app/agent/mod.rs`: adjust exports.
- `src/app/agent/chat_graph.rs`: construct and attach `ChatEffectExecutor`.
- `src/app/agent/agent_runtime.rs`: import `ConversationTurnWriter` from `chat_effect`.
- `src/app/agent/subgraphs/reasoning_loop.rs`: assign `NoEffect<AlternateReasoningUpdate>` to the alternate test state.
- `src/domain/agent/mod.rs`: assign `Effect = ()` to Domain-only test state.

---

### Task 1: Define the application-layer Effect protocol and stable identity

**Files:**
- Create: `tests/agent_effect_contract.rs`
- Create: `src/app/agent/graph/effect.rs`
- Modify: `src/app/agent/graph/mod.rs`

**Interfaces:**
- Consumes: existing `RunId`, `NodeId`, `RunContext`, `AgentUpdate`, and `AppError`.
- Produces: `RunStep`, `EffectId`, `AgentEffect`, `EffectEnvelope<E>`, `EffectReceipt<R>`, `EffectExecutor<E>`, `EffectErrorKind`, `EffectError`, and `NoEffect<U>`.

- [ ] **Step 1: Write the failing public Effect identity test**

Create `tests/agent_effect_contract.rs` with this first test:

```rust
use server_rs::app::agent::graph::{EffectId, NodeId, RunId, RunStep};

fn node(value: &str) -> NodeId {
    NodeId::try_from(value).unwrap()
}

#[test]
fn effect_id_is_stable_and_each_coordinate_is_significant() {
    let run = RunId::new();
    let other_run = RunId::new();
    let step_1 = RunStep::try_from(1).unwrap();
    let step_2 = RunStep::try_from(2).unwrap();

    let original = EffectId::new(run, step_1, node("persist"), 0);
    assert_eq!(
        original,
        EffectId::new(run, step_1, node("persist"), 0)
    );
    assert_ne!(
        original,
        EffectId::new(other_run, step_1, node("persist"), 0)
    );
    assert_ne!(
        original,
        EffectId::new(run, step_2, node("persist"), 0)
    );
    assert_ne!(
        original,
        EffectId::new(run, step_1, node("other"), 0)
    );
    assert_ne!(
        original,
        EffectId::new(run, step_1, node("persist"), 1)
    );
    assert_eq!(original.run_id(), run);
    assert_eq!(original.step(), step_1);
    assert_eq!(original.node_id(), &node("persist"));
    assert_eq!(original.ordinal(), 0);
}
```

- [ ] **Step 2: Run the test and verify the red state**

Run:

```powershell
cargo test --test agent_effect_contract effect_id_is_stable_and_each_coordinate_is_significant --target-dir D:\WorkSpace\ServerRS\.worktrees\.cargo-target-agent-effect
```

Expected: compilation fails because `EffectId` and `RunStep` are not exported from `app::agent::graph`.

- [ ] **Step 3: Implement the Effect protocol and identity types**

Create `src/app/agent/graph/effect.rs` with these exact public contracts:

```rust
use super::{NodeId, RunContext, RunId};
use crate::domain::agent::AgentUpdate;
use crate::shared::error::AppError;
use async_trait::async_trait;
use std::convert::Infallible;
use std::fmt::{Display, Formatter};
use std::marker::PhantomData;
use std::num::NonZeroU32;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RunStep(NonZeroU32);

impl RunStep {
    pub fn get(self) -> u32 {
        self.0.get()
    }
}

impl TryFrom<u32> for RunStep {
    type Error = &'static str;

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        NonZeroU32::new(value)
            .map(Self)
            .ok_or("RunStep 必须从 1 开始")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct EffectId {
    run_id: RunId,
    step: RunStep,
    node_id: NodeId,
    ordinal: u32,
}

impl EffectId {
    pub fn new(run_id: RunId, step: RunStep, node_id: NodeId, ordinal: u32) -> Self {
        Self { run_id, step, node_id, ordinal }
    }

    pub fn run_id(&self) -> RunId { self.run_id }
    pub fn step(&self) -> RunStep { self.step }
    pub fn node_id(&self) -> &NodeId { &self.node_id }
    pub fn ordinal(&self) -> u32 { self.ordinal }
}

impl Display for EffectId {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "{}/{}/{}/{}",
            self.run_id, self.step.get(), self.node_id, self.ordinal
        )
    }
}

pub trait AgentEffect: Send + Sync + 'static {
    type Update: Send + Sync + 'static;
    type Receipt: Send + Sync + 'static;

    fn receipt_updates(receipt: &Self::Receipt) -> Vec<AgentUpdate<Self::Update>>;
}

#[derive(Debug)]
pub struct EffectEnvelope<E> {
    pub id: EffectId,
    pub effect: E,
}

#[derive(Debug)]
pub struct EffectReceipt<R> {
    pub effect_id: EffectId,
    pub value: R,
}

#[async_trait]
pub trait EffectExecutor<E: AgentEffect>: Send + Sync {
    async fn execute(
        &self,
        envelope: &EffectEnvelope<E>,
        context: &RunContext,
    ) -> Result<E::Receipt, EffectError>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EffectErrorKind {
    Transient,
    Permanent,
    Timeout,
    UnknownCommit,
}

#[derive(Debug, thiserror::Error)]
#[error("{kind:?}: {message}")]
pub struct EffectError {
    kind: EffectErrorKind,
    message: String,
    application_error: Option<AppError>,
}

impl EffectError {
    pub fn new(kind: EffectErrorKind, message: impl Into<String>) -> Self {
        Self { kind, message: message.into(), application_error: None }
    }

    pub fn unknown_commit(message: impl Into<String>) -> Self {
        Self::new(EffectErrorKind::UnknownCommit, message)
    }

    pub fn from_application(error: AppError) -> Self {
        let kind = match &error {
            AppError::Infrastructure(_) => EffectErrorKind::Transient,
            AppError::Validation(_)
            | AppError::Unauthorized
            | AppError::Forbidden(_)
            | AppError::NotFound(_)
            | AppError::Conflict(_)
            | AppError::Internal(_)
            | AppError::NotImplemented(_)
            | AppError::Gone(_) => EffectErrorKind::Permanent,
        };
        Self {
            kind,
            message: error.to_string(),
            application_error: Some(error),
        }
    }

    pub fn kind(&self) -> EffectErrorKind { self.kind }
    pub fn message(&self) -> &str { &self.message }
    pub fn application_error(&self) -> Option<&AppError> { self.application_error.as_ref() }
}

pub enum NoEffect<U> {
    #[doc(hidden)]
    Never(Infallible, PhantomData<fn() -> U>),
}

impl<U: Send + Sync + 'static> AgentEffect for NoEffect<U> {
    type Update = U;
    type Receipt = Infallible;

    fn receipt_updates(receipt: &Self::Receipt) -> Vec<AgentUpdate<Self::Update>> {
        match *receipt {}
    }
}
```

Add `mod effect;` and re-export every public type from `src/app/agent/graph/mod.rs`:

```rust
pub use effect::{
    AgentEffect, EffectEnvelope, EffectError, EffectErrorKind, EffectExecutor, EffectId,
    EffectReceipt, NoEffect, RunStep,
};
```

- [ ] **Step 4: Run the identity contract test and graph module tests**

Run:

```powershell
cargo test --test agent_effect_contract --target-dir D:\WorkSpace\ServerRS\.worktrees\.cargo-target-agent-effect
cargo test app::agent::graph --lib --target-dir D:\WorkSpace\ServerRS\.worktrees\.cargo-target-agent-effect
```

Expected: the integration test passes; all existing graph tests pass.

- [ ] **Step 5: Format, inspect, and commit Task 1**

Run:

```powershell
cargo fmt --all
git diff --check
git status --short
```

Verify only `effect.rs`, `graph/mod.rs`, and `tests/agent_effect_contract.rs` changed, then commit:

```powershell
git add src/app/agent/graph/effect.rs src/app/agent/graph/mod.rs tests/agent_effect_contract.rs
git commit -m "新增：定义 Agent Effect 基础协议"
```

---

### Task 2: Associate Effects with business state and extend `NodeResult`

**Files:**
- Modify: `tests/agent_effect_contract.rs`
- Modify: `src/domain/agent/state.rs`
- Modify: `src/domain/agent/mod.rs`
- Modify: `src/app/agent/graph/node.rs`
- Modify: `src/app/agent/graph/mod.rs`
- Modify: `src/app/agent/graph/fragment.rs`
- Modify: `src/app/agent/graph/runtime.rs`
- Modify: `src/app/agent/chat_state.rs`
- Modify: `src/app/agent/nodes/chat_context.rs`
- Modify: `src/app/agent/nodes/chat_turn.rs`
- Modify: `src/app/agent/nodes/reasoning.rs`
- Modify: `src/app/agent/subgraphs/reasoning_loop.rs`

**Interfaces:**
- Consumes: `AgentEffect`, `NoEffect<U>`, existing `AgentUpdate<U>` and `UsageDelta`.
- Produces: `AgentBusinessState::Effect`, `NodeResult<U, E>`, `NodeResult::with_effect`, and `NodeResult::with_effects`.

- [ ] **Step 1: Add a failing `NodeResult` Effect contract test**

Append to `tests/agent_effect_contract.rs`:

```rust
use server_rs::app::agent::graph::{AgentEffect, NodeResult, UsageDelta};
use server_rs::domain::agent::AgentUpdate;

struct TestEffect;

impl AgentEffect for TestEffect {
    type Update = ();
    type Receipt = ();

    fn receipt_updates(_receipt: &Self::Receipt) -> Vec<AgentUpdate<Self::Update>> {
        Vec::new()
    }
}

#[test]
fn node_result_carries_an_explicit_effect_without_mutating_state() {
    let result = NodeResult::<(), TestEffect>::with_effect(
        Vec::new(),
        TestEffect,
        UsageDelta::default(),
    );

    assert!(result.updates.is_empty());
    assert_eq!(result.effects.len(), 1);
}
```

- [ ] **Step 2: Run the contract test and verify the red state**

Run:

```powershell
cargo test --test agent_effect_contract node_result_carries_an_explicit_effect_without_mutating_state --target-dir D:\WorkSpace\ServerRS\.worktrees\.cargo-target-agent-effect
```

Expected: compilation fails because `NodeResult` has one generic parameter and no `with_effect` constructor.

- [ ] **Step 3: Add the associated Effect and new `NodeResult` shape**

Change the Domain trait in `src/domain/agent/state.rs` without importing App types:

```rust
pub trait AgentBusinessState: Clone + Send + Sync + 'static {
    type Update: Send + Sync + 'static;
    type Effect: Send + Sync + 'static;

    fn apply_update(&mut self, update: Self::Update) -> Result<(), AgentStateError>;
}
```

Replace `NodeResult<U>` in `src/app/agent/graph/node.rs` with:

```rust
#[derive(Debug)]
pub struct NodeResult<U, E> {
    pub updates: Vec<AgentUpdate<U>>,
    pub effects: Vec<E>,
    pub usage: UsageDelta,
}

impl<U, E> NodeResult<U, E> {
    pub fn new(updates: Vec<AgentUpdate<U>>, usage: UsageDelta) -> Self {
        Self { updates, effects: Vec::new(), usage }
    }

    pub fn empty() -> Self {
        Self { updates: Vec::new(), effects: Vec::new(), usage: UsageDelta::default() }
    }

    pub fn with_effect(
        updates: Vec<AgentUpdate<U>>,
        effect: E,
        usage: UsageDelta,
    ) -> Self {
        Self { updates, effects: vec![effect], usage }
    }

    pub fn with_effects(
        updates: Vec<AgentUpdate<U>>,
        effects: Vec<E>,
        usage: UsageDelta,
    ) -> Self {
        Self { updates, effects, usage }
    }
}
```

Update the node trait return type:

```rust
) -> Result<NodeResult<B::Update, B::Effect>, NodeError>;
```

- [ ] **Step 4: Assign explicit Effect types and update all result signatures**

Use these exact associated types:

```rust
// src/domain/agent/mod.rs Domain-only test
type Effect = ();

// src/app/agent/chat_state.rs until Task 6 replaces it with ChatEffect
type Effect = NoEffect<ChatTurnUpdate>;

// graph/mod.rs and graph/fragment.rs tests
type Effect = NoEffect<()>;

// graph/runtime.rs tests
type Effect = NoEffect<TestUpdate>;

// subgraphs/reasoning_loop.rs alternate test state
type Effect = NoEffect<AlternateReasoningUpdate>;
```

Add `use super::graph::NoEffect;` in `chat_state.rs`. The graph test modules already import `NoEffect` through `super::*`; add `NoEffect` to the existing graph import in `subgraphs/reasoning_loop.rs`.

Update every `AgentNode` and fragment wrapper signature to the associated pair:

```rust
Result<NodeResult<B::Update, B::Effect>, NodeError>
Result<NodeResult<ChatTurnUpdate, ChatTurnState::Effect>, NodeError>
```

Where Rust does not allow `ChatTurnState::Effect` directly, use the fully qualified form:

```rust
NodeResult<ChatTurnUpdate, <ChatTurnState as AgentBusinessState>::Effect>
```

Change the reasoning helper to preserve inference for any Effect type:

```rust
fn final_assistant_result<U, E>(content: String, tokens: u64) -> NodeResult<U, E> {
    NodeResult::new(
        vec![AgentUpdate::AppendMessages(vec![AgentMessage::assistant(
            content,
            Vec::new(),
        )])],
        UsageDelta { tokens },
    )
}
```

- [ ] **Step 5: Run contract, compile, and existing Agent tests**

Run:

```powershell
cargo test --test agent_effect_contract --target-dir D:\WorkSpace\ServerRS\.worktrees\.cargo-target-agent-effect
cargo check --target-dir D:\WorkSpace\ServerRS\.worktrees\.cargo-target-agent-effect
cargo test app::agent --lib --target-dir D:\WorkSpace\ServerRS\.worktrees\.cargo-target-agent-effect
```

Expected: contract tests pass, default compilation passes, and all existing Agent tests pass with empty Effect lists.

- [ ] **Step 6: Format, inspect, and commit Task 2**

Run:

```powershell
cargo fmt --all
git diff --check
git diff --name-only
```

Confirm no QQ Bot or NapCat path appears, then commit:

```powershell
git add tests/agent_effect_contract.rs src/domain/agent/state.rs src/domain/agent/mod.rs src/app/agent/graph/node.rs src/app/agent/graph/mod.rs src/app/agent/graph/fragment.rs src/app/agent/graph/runtime.rs src/app/agent/chat_state.rs src/app/agent/nodes/chat_context.rs src/app/agent/nodes/chat_turn.rs src/app/agent/nodes/reasoning.rs src/app/agent/subgraphs/reasoning_loop.rs
git commit -m "重构：让 Agent 节点显式返回 Effect"
```

---

### Task 3: Make logical Run steps explicit

**Files:**
- Modify: `src/app/agent/graph/budget.rs`
- Modify: `src/app/agent/graph/runtime.rs`

**Interfaces:**
- Consumes: `RunStep` from Task 1 and existing step budget logic.
- Produces: `RunContext::check_ready(...) -> Result<RunStep, GraphRunError>` and `RunContext::check_active()`.

- [ ] **Step 1: Write a failing monotonic RunStep test**

Add this unit test to `src/app/agent/graph/budget.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn check_ready_returns_monotonic_steps_starting_at_one() {
        let context = RunContext::new(
            RunBudget::for_test(3),
            CancellationToken::new(),
            RunTrace::default(),
        );
        let graph_limit = NonZeroU32::new(3).unwrap();

        assert_eq!(context.check_ready(graph_limit).unwrap().get(), 1);
        assert_eq!(context.check_ready(graph_limit).unwrap().get(), 2);
        assert_eq!(context.budget().snapshot().steps, 2);
    }
}
```

- [ ] **Step 2: Run the test and verify the red state**

Run:

```powershell
cargo test app::agent::graph::budget::tests::check_ready_returns_monotonic_steps_starting_at_one --lib --target-dir D:\WorkSpace\ServerRS\.worktrees\.cargo-target-agent-effect
```

Expected: compilation fails because `check_ready` currently returns `()`.

- [ ] **Step 3: Return `RunStep` from the existing reservation path**

Change `RunBudgetHandle::reserve_step` so it returns the exact reserved value:

Add `RunStep` to the existing `use super::{...};` list in `budget.rs`, then change the method to:

```rust
pub(crate) fn reserve_step(
    &self,
    graph_max_steps: NonZeroU32,
) -> Result<RunStep, GraphRunError> {
    let mut usage = self.lock_usage();
    let limit = self.limits.max_steps.get().min(graph_max_steps.get());
    let attempted = usage.steps.saturating_add(1);
    if attempted > limit {
        return Err(GraphRunError::BudgetExceeded {
            resource: BudgetResource::Steps,
            limit: u64::from(limit),
            attempted: u64::from(attempted),
        });
    }
    let step = RunStep::try_from(attempted)
        .expect("a reserved graph step is always non-zero");
    usage.steps = attempted;
    Ok(step)
}
```

Split active checks from reservation:

```rust
pub(crate) fn check_active(&self) -> Result<(), GraphRunError> {
    if self.cancellation.is_cancelled() {
        return Err(GraphRunError::Cancelled);
    }
    if Instant::now() >= self.deadline {
        return Err(GraphRunError::DeadlineExceeded);
    }
    Ok(())
}

pub(crate) fn check_ready(
    &self,
    graph_max_steps: NonZeroU32,
) -> Result<RunStep, GraphRunError> {
    self.check_active()?;
    self.budget.reserve_step(graph_max_steps)
}
```

In `runtime.rs`, retain both typed and numeric forms:

```rust
let run_step = context.check_ready(self.graph.policy().max_steps())?;
let step = run_step.get();
```

- [ ] **Step 4: Run budget and runtime tests**

Run:

```powershell
cargo test app::agent::graph::budget --lib --target-dir D:\WorkSpace\ServerRS\.worktrees\.cargo-target-agent-effect
cargo test app::agent::graph::runtime --lib --target-dir D:\WorkSpace\ServerRS\.worktrees\.cargo-target-agent-effect
```

Expected: new RunStep test and all existing budget, deadline, cancellation, and step-limit tests pass.

- [ ] **Step 5: Format and commit Task 3**

```powershell
cargo fmt --all
git diff --check
git add src/app/agent/graph/budget.rs src/app/agent/graph/runtime.rs
git commit -m "重构：显式标识 Agent 图运行步骤"
```

---

### Task 4: Execute Effects once and collect typed Receipts

**Files:**
- Modify: `src/app/agent/graph/error.rs`
- Modify: `src/app/agent/graph/runtime.rs`

**Interfaces:**
- Consumes: `AgentEffect`, `EffectEnvelope`, `EffectExecutor`, `EffectReceipt`, `EffectId`, and `RunStep`.
- Produces: `GraphRuntime::with_effect_executor`, Effect-aware `GraphRunResult`, candidate-State application, `MissingEffectExecutor`, and `EffectFailed`.

- [ ] **Step 1: Add a failing successful-Effect runtime test**

Extend the runtime test harness in `src/app/agent/graph/runtime.rs` with:

```rust
use std::sync::atomic::{AtomicUsize, Ordering};

#[derive(Debug, Clone)]
enum TestEffect {
    Set(i32),
}

#[derive(Debug)]
enum TestReceipt {
    Set(i32),
}

impl AgentEffect for TestEffect {
    type Update = TestUpdate;
    type Receipt = TestReceipt;

    fn receipt_updates(receipt: &Self::Receipt) -> Vec<AgentUpdate<Self::Update>> {
        match receipt {
            TestReceipt::Set(value) => {
                vec![AgentUpdate::Business(TestUpdate::Set(*value))]
            }
        }
    }
}

struct TestEffectExecutor {
    calls: AtomicUsize,
}

#[async_trait]
impl EffectExecutor<TestEffect> for TestEffectExecutor {
    async fn execute(
        &self,
        envelope: &EffectEnvelope<TestEffect>,
        _context: &RunContext,
    ) -> Result<TestReceipt, EffectError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        match &envelope.effect {
            TestEffect::Set(value) => Ok(TestReceipt::Set(*value)),
        }
    }
}
```

Change only the runtime test business to `type Effect = TestEffect`, add `Behavior::Effect(i32)`, and make that behavior return a terminal outcome plus one Effect:

```rust
Behavior::Effect(value) => Ok(NodeResult::with_effect(
    vec![AgentUpdate::SetOutcome(AgentOutcome::Respond("done".into()))],
    TestEffect::Set(value),
    UsageDelta::default(),
)),
```

Add the test:

```rust
#[tokio::test]
async fn effect_receipt_updates_candidate_state_and_is_returned() {
    let executor = Arc::new(TestEffectExecutor {
        calls: AtomicUsize::new(0),
    });
    let runtime = GraphRuntime::with_effect_executor(
        single_node_graph(Behavior::Effect(7)),
        executor.clone(),
    );

    let result = runtime
        .run(
            AgentState::new(TestBusiness::default()),
            RunBudget::for_test(8),
        )
        .await
        .unwrap();

    assert_eq!(executor.calls.load(Ordering::SeqCst), 1);
    assert_eq!(result.state.business().value, 7);
    assert_eq!(result.effect_receipts.len(), 1);
    assert_eq!(result.effect_receipts[0].effect_id.run_id(), result.run_id);
    assert_eq!(result.effect_receipts[0].effect_id.step().get(), 1);
    assert_eq!(result.effect_receipts[0].effect_id.node_id(), &node_id("only"));
    assert_eq!(result.effect_receipts[0].effect_id.ordinal(), 0);
}
```

Add the missing-executor contract while the same harness is red:

```rust
#[tokio::test]
async fn effect_without_executor_is_rejected() {
    let error = GraphRuntime::new(single_node_graph(Behavior::Effect(7)))
        .run(
            AgentState::new(TestBusiness::default()),
            RunBudget::for_test(8),
        )
        .await
        .unwrap_err();

    assert!(matches!(
        error,
        GraphRunError::MissingEffectExecutor { node } if node == node_id("only")
    ));
}
```

- [ ] **Step 2: Run the test and verify the red state**

Run:

```powershell
cargo test app::agent::graph::runtime::tests::effect_receipt_updates_candidate_state_and_is_returned --lib --target-dir D:\WorkSpace\ServerRS\.worktrees\.cargo-target-agent-effect
cargo test app::agent::graph::runtime::tests::effect_without_executor_is_rejected --lib --target-dir D:\WorkSpace\ServerRS\.worktrees\.cargo-target-agent-effect
```

Expected: compilation fails because `GraphRuntime::with_effect_executor`, `GraphRunResult::effect_receipts`, and `GraphRunError::MissingEffectExecutor` do not exist.

- [ ] **Step 3: Add Effect-specific run errors**

Add these variants to `GraphRunError` in `error.rs`:

```rust
#[error("节点 {node} 返回 Effect，但图运行器未配置 EffectExecutor")]
MissingEffectExecutor { node: NodeId },

#[error("节点 {node} 的 Effect {effect_id} 执行失败: {error}")]
EffectFailed {
    node: NodeId,
    effect_id: EffectId,
    #[source]
    error: EffectError,
},
```

Import `EffectError` and `EffectId` from the graph module.

- [ ] **Step 4: Implement candidate-State Effect execution**

Give `GraphRuntime<B>` an optional executor and two constructors:

Add `AgentEffect`, `EffectEnvelope`, `EffectExecutor`, `EffectId`, and `EffectReceipt` to the existing graph imports in `runtime.rs`, then use:

```rust
pub struct GraphRuntime<B>
where
    B: AgentBusinessState,
    B::Effect: AgentEffect<Update = B::Update>,
{
    graph: Arc<CompiledGraph<B>>,
    effect_executor: Option<Arc<dyn EffectExecutor<B::Effect>>>,
}

impl<B> GraphRuntime<B>
where
    B: AgentBusinessState,
    B::Effect: AgentEffect<Update = B::Update>,
{
    pub fn new(graph: CompiledGraph<B>) -> Self {
        Self { graph: Arc::new(graph), effect_executor: None }
    }

    pub fn with_effect_executor(
        graph: CompiledGraph<B>,
        effect_executor: Arc<dyn EffectExecutor<B::Effect>>,
    ) -> Self {
        Self { graph: Arc::new(graph), effect_executor: Some(effect_executor) }
    }
}
```

After node execution and usage validation, replace direct State mutation with this algorithm:

```rust
let mut candidate = state.clone();
candidate
    .apply_updates(result.updates)
    .map_err(|error| GraphRunError::StateUpdateFailed {
        node: current.clone(),
        error,
    })?;

let mut node_receipts = Vec::with_capacity(result.effects.len());

for (ordinal, effect) in result.effects.into_iter().enumerate() {
    context.check_active()?;
    let ordinal = u32::try_from(ordinal).map_err(|_| GraphRunError::NodeFailed {
        node: current.clone(),
        error: NodeError::new(NodeErrorKind::Invariant, "节点返回的 Effect 数量超过 u32"),
    })?;
    let effect_id = EffectId::new(
        context.run_id(),
        run_step,
        current.clone(),
        ordinal,
    );
    let executor = self.effect_executor.as_ref().ok_or_else(|| {
        GraphRunError::MissingEffectExecutor { node: current.clone() }
    })?;
    let envelope = EffectEnvelope { id: effect_id.clone(), effect };
    let value = executor.execute(&envelope, &context).await.map_err(|error| {
        GraphRunError::EffectFailed {
            node: current.clone(),
            effect_id: effect_id.clone(),
            error,
        }
    })?;
    let receipt_updates = B::Effect::receipt_updates(&value);
    candidate.apply_updates(receipt_updates).map_err(|error| {
        GraphRunError::StateUpdateFailed {
            node: current.clone(),
            error,
        }
    })?;
    node_receipts.push(EffectReceipt { effect_id, value });
}

state = candidate;
effect_receipts.extend(node_receipts);
```

Initialize `effect_receipts` before the run loop and add it to the success result:

```rust
pub struct GraphRunResult<B>
where
    B: AgentBusinessState,
    B::Effect: AgentEffect<Update = B::Update>,
{
    pub state: AgentState<B>,
    pub usage: UsageSnapshot,
    pub visited: Vec<NodeId>,
    pub run_id: RunId,
    pub effect_receipts: Vec<EffectReceipt<<B::Effect as AgentEffect>::Receipt>>,
}
```

- [ ] **Step 5: Run the happy-path test and all existing graph tests**

```powershell
cargo test app::agent::graph::runtime::tests::effect_receipt_updates_candidate_state_and_is_returned --lib --target-dir D:\WorkSpace\ServerRS\.worktrees\.cargo-target-agent-effect
cargo test app::agent::graph::runtime::tests::effect_without_executor_is_rejected --lib --target-dir D:\WorkSpace\ServerRS\.worktrees\.cargo-target-agent-effect
cargo test app::agent::graph --lib --target-dir D:\WorkSpace\ServerRS\.worktrees\.cargo-target-agent-effect
```

Expected: both new Effect tests and all prior graph tests pass.

- [ ] **Step 6: Format and commit Task 4**

```powershell
cargo fmt --all
git diff --check
git add src/app/agent/graph/error.rs src/app/agent/graph/runtime.rs
git commit -m "新增：执行 Agent Effect 并收集回执"
```

---

### Task 5: Enforce no-retry, UnknownCommit, and post-Effect failure semantics

**Files:**
- Modify: `src/app/agent/graph/runtime.rs`
- Modify: `src/app/agent/graph/error.rs`

**Interfaces:**
- Consumes: Task 4 Effect execution loop and `RunContext::check_active`.
- Produces: regression evidence for one-attempt and pre-dispatch behavior, plus red-green changes for completed Effect IDs, `PostEffectStateUpdateFailed`, and in-flight `UnknownCommit`.

- [ ] **Step 1: Expand the runtime test harness without changing production behavior**

Replace the Task 4 fake Effect and Receipt definitions with these complete variants:

```rust
#[derive(Debug, Clone)]
enum TestEffect {
    Set(i32),
    Fail,
    RejectUpdate,
    Pending(Arc<tokio::sync::Notify>),
}

#[derive(Debug)]
enum TestReceipt {
    Set(i32),
    Reject,
}

impl AgentEffect for TestEffect {
    type Update = TestUpdate;
    type Receipt = TestReceipt;

    fn receipt_updates(receipt: &Self::Receipt) -> Vec<AgentUpdate<Self::Update>> {
        match receipt {
            TestReceipt::Set(value) => {
                vec![AgentUpdate::Business(TestUpdate::Set(*value))]
            }
            TestReceipt::Reject => vec![AgentUpdate::Business(TestUpdate::Reject)],
        }
    }
}

impl TestEffectExecutor {
    fn recording() -> Self {
        Self {
            calls: AtomicUsize::new(0),
        }
    }
}

#[async_trait]
impl EffectExecutor<TestEffect> for TestEffectExecutor {
    async fn execute(
        &self,
        envelope: &EffectEnvelope<TestEffect>,
        _context: &RunContext,
    ) -> Result<TestReceipt, EffectError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        match &envelope.effect {
            TestEffect::Set(value) => Ok(TestReceipt::Set(*value)),
            TestEffect::Fail => Err(EffectError::from_application(AppError::Conflict(
                "turn changed".into(),
            ))),
            TestEffect::RejectUpdate => Ok(TestReceipt::Reject),
            TestEffect::Pending(started) => {
                started.notify_one();
                std::future::pending::<Result<TestReceipt, EffectError>>().await
            }
        }
    }
}
```

Add `TestUpdate::Reject` and replace its `apply_update` body with:

```rust
match update {
    TestUpdate::Set(value) => {
        self.value = value;
        Ok(())
    }
    TestUpdate::Reject => Err(AgentStateError::Business("rejected".into())),
}
```

Add these `Behavior` variants:

```rust
Effects(Vec<TestEffect>),
RejectThenEffect(TestEffect),
CancelThenEffects(Vec<TestEffect>),
UsageThenEffects(UsageDelta, Vec<TestEffect>),
```

and these exact `FakeNode::execute` arms:

```rust
Behavior::Effects(ref effects) => Ok(NodeResult::with_effects(
    vec![AgentUpdate::SetOutcome(AgentOutcome::Respond("done".into()))],
    effects.clone(),
    UsageDelta::default(),
)),
Behavior::RejectThenEffect(ref effect) => Ok(NodeResult::with_effect(
    vec![AgentUpdate::Business(TestUpdate::Reject)],
    effect.clone(),
    UsageDelta::default(),
)),
Behavior::CancelThenEffects(ref effects) => {
    _context.cancellation().cancel();
    Ok(NodeResult::with_effects(
        vec![AgentUpdate::SetOutcome(AgentOutcome::Respond("done".into()))],
        effects.clone(),
        UsageDelta::default(),
    ))
}
Behavior::UsageThenEffects(usage, ref effects) => Ok(NodeResult::with_effects(
    vec![AgentUpdate::SetOutcome(AgentOutcome::Respond("done".into()))],
    effects.clone(),
    usage,
)),
```

- [ ] **Step 2: Lock Task 4's one-attempt and pre-dispatch contracts with green regression tests**

Add all four tests before touching production code:

```rust
#[tokio::test]
async fn failed_effect_is_not_retried_and_preserves_application_error() {
    let executor = Arc::new(TestEffectExecutor::recording());
    let runtime = GraphRuntime::with_effect_executor(
        single_node_graph(Behavior::Effects(vec![TestEffect::Fail])),
        executor.clone(),
    );

    let error = runtime
        .run(
            AgentState::new(TestBusiness::default()),
            RunBudget::for_test(8),
        )
        .await
        .unwrap_err();

    assert_eq!(executor.calls.load(Ordering::SeqCst), 1);
    match error {
        GraphRunError::EffectFailed { error, .. } => assert!(matches!(
            error.application_error(),
            Some(AppError::Conflict(message)) if message == "turn changed"
        )),
        other => panic!("unexpected error: {other:?}"),
    }
}

#[tokio::test]
async fn invalid_pure_update_prevents_effect_dispatch() {
    let executor = Arc::new(TestEffectExecutor::recording());
    let runtime = GraphRuntime::with_effect_executor(
        single_node_graph(Behavior::RejectThenEffect(TestEffect::Set(7))),
        executor.clone(),
    );

    let error = runtime
        .run(
            AgentState::new(TestBusiness::default()),
            RunBudget::for_test(8),
        )
        .await
        .unwrap_err();

    assert!(matches!(error, GraphRunError::StateUpdateFailed { .. }));
    assert_eq!(executor.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn cancellation_before_effect_dispatch_does_not_call_executor() {
    let executor = Arc::new(TestEffectExecutor::recording());
    let runtime = GraphRuntime::with_effect_executor(
        single_node_graph(Behavior::CancelThenEffects(vec![TestEffect::Set(7)])),
        executor.clone(),
    );

    let error = runtime
        .run(
            AgentState::new(TestBusiness::default()),
            RunBudget::for_test(8),
        )
        .await
        .unwrap_err();

    assert!(matches!(error, GraphRunError::Cancelled));
    assert_eq!(executor.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn usage_budget_failure_prevents_effect_dispatch() {
    let executor = Arc::new(TestEffectExecutor::recording());
    let runtime = GraphRuntime::with_effect_executor(
        single_node_graph(Behavior::UsageThenEffects(
            UsageDelta { tokens: 1 },
            vec![TestEffect::Set(7)],
        )),
        executor.clone(),
    );

    let error = runtime
        .run(
            AgentState::new(TestBusiness::default()),
            RunBudget::for_test(8).with_tokens(0),
        )
        .await
        .unwrap_err();

    assert!(matches!(
        error,
        GraphRunError::BudgetExceeded {
            resource: BudgetResource::Tokens,
            ..
        }
    ));
    assert_eq!(executor.calls.load(Ordering::SeqCst), 0);
}
```

Run:

```powershell
cargo test app::agent::graph::runtime::tests::failed_effect_is_not_retried_and_preserves_application_error --lib --target-dir D:\WorkSpace\ServerRS\.worktrees\.cargo-target-agent-effect
cargo test app::agent::graph::runtime::tests::invalid_pure_update_prevents_effect_dispatch --lib --target-dir D:\WorkSpace\ServerRS\.worktrees\.cargo-target-agent-effect
cargo test app::agent::graph::runtime::tests::cancellation_before_effect_dispatch_does_not_call_executor --lib --target-dir D:\WorkSpace\ServerRS\.worktrees\.cargo-target-agent-effect
cargo test app::agent::graph::runtime::tests::usage_budget_failure_prevents_effect_dispatch --lib --target-dir D:\WorkSpace\ServerRS\.worktrees\.cargo-target-agent-effect
```

Expected: all four pass against Task 4. They document already-implemented invariants and therefore do not justify any production change.

- [ ] **Step 3: Write the failing post-Effect State rejection test**

```rust
#[tokio::test]
async fn successful_effect_with_rejected_receipt_update_has_distinct_error() {
    let executor = Arc::new(TestEffectExecutor::recording());
    let runtime = GraphRuntime::with_effect_executor(
        single_node_graph(Behavior::Effects(vec![TestEffect::RejectUpdate])),
        executor.clone(),
    );

    let error = runtime
        .run(
            AgentState::new(TestBusiness::default()),
            RunBudget::for_test(8),
        )
        .await
        .unwrap_err();

    assert_eq!(executor.calls.load(Ordering::SeqCst), 1);
    assert!(matches!(
        error,
        GraphRunError::PostEffectStateUpdateFailed { .. }
    ));
}
```

Run it and verify compilation fails because `PostEffectStateUpdateFailed` does not yet exist. Do not accept the Task 4 `StateUpdateFailed` result.

- [ ] **Step 4: Implement and verify the distinct post-Effect error**

Add to `GraphRunError`:

```rust
#[error("节点 {node} 的 Effect {effect_id} 已成功，但回执状态更新失败: {error}")]
PostEffectStateUpdateFailed {
    node: NodeId,
    effect_id: EffectId,
    #[source]
    error: AgentStateError,
},
```

Change only the Receipt Update error mapping:

```rust
candidate.apply_updates(receipt_updates).map_err(|error| {
    GraphRunError::PostEffectStateUpdateFailed {
        node: current.clone(),
        effect_id: effect_id.clone(),
        error,
    }
})?;
```

Run:

```powershell
cargo test app::agent::graph::runtime::tests::successful_effect_with_rejected_receipt_update_has_distinct_error --lib --target-dir D:\WorkSpace\ServerRS\.worktrees\.cargo-target-agent-effect
```

Expected: pass with exactly one executor call.

- [ ] **Step 5: Write the failing completed-Effect-ID test**

```rust
#[tokio::test]
async fn later_effect_failure_reports_already_completed_effect_ids() {
    let executor = Arc::new(TestEffectExecutor::recording());
    let runtime = GraphRuntime::with_effect_executor(
        single_node_graph(Behavior::Effects(vec![TestEffect::Set(1), TestEffect::Fail])),
        executor.clone(),
    );

    let error = runtime
        .run(
            AgentState::new(TestBusiness::default()),
            RunBudget::for_test(8),
        )
        .await
        .unwrap_err();

    match error {
        GraphRunError::EffectFailed {
            effect_id,
            completed_effect_ids,
            ..
        } => {
            assert_eq!(completed_effect_ids.len(), 1);
            assert_eq!(completed_effect_ids[0].ordinal(), 0);
            assert_eq!(effect_id.ordinal(), 1);
        }
        other => panic!("unexpected error: {other:?}"),
    }
    assert_eq!(executor.calls.load(Ordering::SeqCst), 2);
}
```

Run it and verify compilation fails because `EffectFailed` does not yet expose `completed_effect_ids`.

- [ ] **Step 6: Track completed Effect IDs and verify the red test turns green**

Extend `EffectFailed` with:

```rust
completed_effect_ids: Vec<EffectId>,
```

Initialize `completed_effect_ids` beside `node_receipts`. Clone it into `EffectFailed` when an executor returns an error, and append the current ID only after its Receipt Update succeeds:

```rust
let mut completed_effect_ids = Vec::with_capacity(result.effects.len());

let value = executor.execute(&envelope, &context).await.map_err(|error| {
    GraphRunError::EffectFailed {
        node: current.clone(),
        effect_id: effect_id.clone(),
        error,
        completed_effect_ids: completed_effect_ids.clone(),
    }
})?;

let receipt_updates = B::Effect::receipt_updates(&value);
candidate.apply_updates(receipt_updates).map_err(|error| {
    GraphRunError::PostEffectStateUpdateFailed {
        node: current.clone(),
        effect_id: effect_id.clone(),
        error,
    }
})?;
completed_effect_ids.push(effect_id.clone());
node_receipts.push(EffectReceipt { effect_id, value });
```

Run:

```powershell
cargo test app::agent::graph::runtime::tests::later_effect_failure_reports_already_completed_effect_ids --lib --target-dir D:\WorkSpace\ServerRS\.worktrees\.cargo-target-agent-effect
```

Expected: pass with ordinal 0 reported complete and ordinal 1 reported failed.

- [ ] **Step 7: Write failing in-flight cancellation and deadline tests**

Add both tests before changing the executor await path:

```rust
#[tokio::test]
async fn cancellation_during_effect_execution_reports_unknown_commit() {
    let token = CancellationToken::new();
    let cancel = token.clone();
    let started = Arc::new(tokio::sync::Notify::new());
    let context = RunContext::new(RunBudget::for_test(8), token, RunTrace::default());
    let executor = Arc::new(TestEffectExecutor::recording());
    let runtime = GraphRuntime::with_effect_executor(
        single_node_graph(Behavior::Effects(vec![TestEffect::Pending(started.clone())])),
        executor,
    );

    let mut run = tokio::spawn(async move {
        runtime
            .run_with_context(AgentState::new(TestBusiness::default()), context)
            .await
    });
    started.notified().await;
    cancel.cancel();

    let joined = match tokio::time::timeout(Duration::from_secs(1), &mut run).await {
        Ok(joined) => joined,
        Err(_) => {
            run.abort();
            panic!("runtime did not observe in-flight cancellation");
        }
    };
    let error = joined.unwrap().unwrap_err();

    match error {
        GraphRunError::EffectFailed { error, .. } => {
            assert_eq!(error.kind(), EffectErrorKind::UnknownCommit)
        }
        other => panic!("unexpected error: {other:?}"),
    }
}

#[tokio::test]
async fn deadline_during_effect_execution_reports_unknown_commit() {
    let started = Arc::new(tokio::sync::Notify::new());
    let budget = RunBudget::new(
        NonZeroU32::new(8).unwrap(),
        Duration::from_millis(500),
    );
    let context = RunContext::new(budget, CancellationToken::new(), RunTrace::default());
    let executor = Arc::new(TestEffectExecutor::recording());
    let runtime = GraphRuntime::with_effect_executor(
        single_node_graph(Behavior::Effects(vec![TestEffect::Pending(started.clone())])),
        executor,
    );

    let mut run = tokio::spawn(async move {
        runtime
            .run_with_context(AgentState::new(TestBusiness::default()), context)
            .await
    });
    tokio::time::timeout(Duration::from_secs(1), started.notified())
        .await
        .expect("executor did not start before the Run deadline");

    let joined = match tokio::time::timeout(Duration::from_secs(2), &mut run).await {
        Ok(joined) => joined,
        Err(_) => {
            run.abort();
            panic!("runtime did not observe the in-flight deadline");
        }
    };
    let error = joined.unwrap().unwrap_err();

    match error {
        GraphRunError::EffectFailed { error, .. } => {
            assert_eq!(error.kind(), EffectErrorKind::UnknownCommit)
        }
        other => panic!("unexpected error: {other:?}"),
    }
}
```

Run both tests. Expected red: Task 4/Step 6 still directly awaits the pending executor, so each test reaches its explicit outer-timeout panic instead of returning `UnknownCommit`.

- [ ] **Step 8: Wrap the in-flight executor Future without adding a retry path**

Keep `context.check_active()?` immediately before constructing the Envelope. Replace the direct executor await with:

```rust
let cancellation = context.cancellation().clone();
let deadline = tokio::time::Instant::from_std(context.deadline());
let execution = executor.execute(&envelope, &context);
let value = tokio::select! {
    biased;
    _ = cancellation.cancelled() => {
        Err(EffectError::unknown_commit(
            "Effect 执行期间运行被取消，外部提交状态未知",
        ))
    }
    _ = tokio::time::sleep_until(deadline) => {
        Err(EffectError::unknown_commit(
            "Effect 执行期间超过截止时间，外部提交状态未知",
        ))
    }
    result = execution => result,
};

let value = value.map_err(|error| GraphRunError::EffectFailed {
    node: current.clone(),
    effect_id: effect_id.clone(),
    error,
    completed_effect_ids: completed_effect_ids.clone(),
})?;
```

There must be exactly one `executor.execute(...)` call and no loop, recursion, backoff, or second attempt.

- [ ] **Step 9: Run the complete runtime suite**

```powershell
cargo test app::agent::graph::runtime --lib --target-dir D:\WorkSpace\ServerRS\.worktrees\.cargo-target-agent-effect
```

Expected: all old and new runtime tests pass, including one-attempt, candidate validation, pre-dispatch cancellation, completed IDs, in-flight cancellation/deadline, and post-Effect State failure.

- [ ] **Step 10: Format and commit Task 5**

```powershell
cargo fmt --all
git diff --check
git add src/app/agent/graph/runtime.rs src/app/agent/graph/error.rs
git commit -m "完善：区分 Agent Effect 提交状态"
```

---

### Task 6: Migrate Chat turn persistence to `ChatEffectExecutor`

**Files:**
- Create: `src/app/agent/chat_effect.rs`
- Create: `tests/chat_effect_persistence.rs`
- Modify: `src/app/agent/mod.rs`
- Modify: `src/app/agent/chat_state.rs`
- Modify: `src/app/agent/nodes/chat_turn.rs`
- Modify: `src/app/agent/nodes/mod.rs`
- Modify: `src/app/agent/chat_graph.rs`
- Modify: `src/app/agent/agent_runtime.rs`

**Interfaces:**
- Consumes: Effect runtime from Tasks 1-5, existing `ChatTurnUpdate`, `PersistedTurn`, `ConversationRepoT`, `NewConversationMessage`, and `TurnWriterT` behavior.
- Produces: `ChatEffect`, `PersistTurnEffect`, `ChatEffectReceipt`, `ChatEffectExecutor`, pure `PersistTurnNode`, and Effect-aware `ChatAgentGraph`.

- [ ] **Step 1: Write all failing Chat persistence boundary tests**

In `src/app/agent/nodes/chat_turn.rs`, delete `RecordedTurn`, both Writer fakes, and both old persistence tests. Replace them with this pure-node test and import `ChatEffect`:

```rust
use crate::app::agent::chat_effect::ChatEffect;

#[tokio::test]
async fn chat_persist_builds_compatible_effect_without_writer() {
    let mut state = AgentState::new(ChatTurnState::new(
        7,
        9,
        "hello".into(),
        Some("calm".into()),
        None,
        vec![],
    ));
    state
        .apply_updates(vec![AgentUpdate::SetOutcome(AgentOutcome::Respond(
            "world".into(),
        ))])
        .unwrap();
    let node = PersistTurnNode::new(id("persist"));

    let result = node.execute(&state, &run_context()).await.unwrap();

    assert!(result.updates.is_empty());
    assert_eq!(result.effects.len(), 1);
    match &result.effects[0] {
        ChatEffect::PersistTurn(effect) => {
            assert_eq!(effect.conversation_id, 9);
            assert_eq!(effect.user_id, 7);
            assert_eq!(effect.user.sender_role, "user");
            assert_eq!(effect.user.sender_user_id, Some(7));
            assert_eq!(
                serde_json::from_str::<serde_json::Value>(&effect.user.content).unwrap(),
                serde_json::json!({"text": "hello", "emotion": "calm"})
            );
            assert_eq!(effect.assistant.sender_role, "assistant");
            assert_eq!(
                serde_json::from_str::<serde_json::Value>(&effect.assistant.content).unwrap(),
                serde_json::json!({"text": "world"})
            );
        }
    }
}
```

Create `tests/chat_effect_persistence.rs` with the complete executor contract suite:

```rust
use async_trait::async_trait;
use server_rs::app::agent::chat_effect::{
    ChatEffect, ChatEffectExecutor, ChatEffectReceipt, PersistTurnEffect, TurnWriterT,
};
use server_rs::app::agent::chat_state::{ChatTurnUpdate, PersistedTurn};
use server_rs::app::agent::graph::{
    AgentEffect, EffectEnvelope, EffectExecutor, EffectId, NodeId, RunBudget, RunContext, RunId,
    RunStep, RunTrace,
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
        assistant: message(
            9,
            "assistant",
            None,
            serde_json::json!({"text": "world"}),
        ),
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
        RunBudget::new(
            NonZeroU32::new(8).unwrap(),
            Duration::from_secs(30),
        ),
        CancellationToken::new(),
        RunTrace::default(),
    )
}

#[tokio::test]
async fn chat_effect_executor_writes_once_and_returns_typed_receipt() {
    let writer = Arc::new(RecordingTurnWriter::default());
    let executor = ChatEffectExecutor::new(writer.clone());

    let receipt = executor
        .execute(&envelope(), &run_context())
        .await
        .unwrap();

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

#[tokio::test]
async fn chat_effect_executor_preserves_writer_application_error() {
    let writer = Arc::new(FailingTurnWriter {
        calls: AtomicUsize::new(0),
    });
    let executor = ChatEffectExecutor::new(writer.clone());

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
```

Run the two new persistence targets before creating `chat_effect.rs`:

```powershell
cargo test app::agent::nodes::chat_turn::tests::chat_persist_builds_compatible_effect_without_writer --lib --target-dir D:\WorkSpace\ServerRS\.worktrees\.cargo-target-agent-effect
cargo test --test chat_effect_persistence --target-dir D:\WorkSpace\ServerRS\.worktrees\.cargo-target-agent-effect
```

Expected red: `chat_effect` and `ChatEffectExecutor` do not exist, and `PersistTurnNode::new` still requires a Writer.

- [ ] **Step 2: Define the Chat Effect, Receipt, Writer boundary, and executor**

Create `src/app/agent/chat_effect.rs` with these contracts:

```rust
use super::chat_state::{ChatTurnUpdate, PersistedTurn};
use super::graph::{
    AgentEffect, EffectEnvelope, EffectError, EffectExecutor, RunContext,
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
}

impl AgentEffect for ChatEffect {
    type Update = ChatTurnUpdate;
    type Receipt = ChatEffectReceipt;

    fn receipt_updates(receipt: &Self::Receipt) -> Vec<AgentUpdate<Self::Update>> {
        match receipt {
            ChatEffectReceipt::TurnPersisted(persisted) => vec![AgentUpdate::Business(
                ChatTurnUpdate::SetPersistedTurn(persisted.clone()),
            )],
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
}

impl ChatEffectExecutor {
    pub fn new(writer: Arc<dyn TurnWriterT>) -> Self {
        Self { writer }
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
                .map_err(EffectError::from_application),
        }
    }
}
```

Export `pub mod chat_effect;` from `src/app/agent/mod.rs`.

- [ ] **Step 3: Make `PersistTurnNode` pure**

Remove `TurnWriterT`, `ConversationTurnWriter`, `ConversationRepoT`, `AppError`, `Arc`, and direct I/O from `nodes/chat_turn.rs`. Remove the Writer types from `nodes/mod.rs` re-exports. Import the new Effect description exactly as follows:

```rust
use crate::app::agent::chat_effect::{ChatEffect, PersistTurnEffect};
```

Replace the node's stored Writer and constructor with:

```rust
pub struct PersistTurnNode {
    id: NodeId,
}

impl PersistTurnNode {
    pub fn new(id: NodeId) -> Self {
        Self { id }
    }
}
```

Keep all current state validation and message construction, but replace `save_turn_atomic(...).await` and the persisted Update with:

```rust
Ok(NodeResult::with_effect(
    Vec::new(),
    ChatEffect::PersistTurn(PersistTurnEffect {
        conversation_id,
        user_id,
        user: NewConversationMessage {
            conversation_id,
            sender_role: "user".into(),
            sender_user_id: Some(user_id),
            message_type: "text".into(),
            content: user_content.to_string(),
            token_count: None,
        },
        assistant: NewConversationMessage {
            conversation_id,
            sender_role: "assistant".into(),
            sender_user_id: None,
            message_type: "text".into(),
            content: assistant_content.to_string(),
            token_count: None,
        },
    }),
    UsageDelta::default(),
))
```

Change `ChatTurnState` to:

```rust
type Effect = ChatEffect;
```

and add `use super::chat_effect::ChatEffect;` in `chat_state.rs`.

- [ ] **Step 4: Update Chat assembly boundaries and prove an executor is required**

Keep `ChatAgentGraphDeps::turn_writer` unchanged. Update imports and the node constructor, but deliberately leave `GraphRuntime::new(compiled)` in place for the red graph test:

```rust
definition.add_node(Arc::new(PersistTurnNode::new(node("persist_turn"))))?;
```

Update imports:

```rust
// chat_graph.rs
use crate::app::agent::chat_effect::{ChatEffectExecutor, TurnWriterT};

// agent_runtime.rs
use super::chat_effect::ConversationTurnWriter;
```

Move `FakeTurnWriter` usage in `chat_graph.rs` tests to the new `TurnWriterT` import without changing test behavior.

Run:

```powershell
cargo test --test chat_effect_persistence --target-dir D:\WorkSpace\ServerRS\.worktrees\.cargo-target-agent-effect
cargo test app::agent::nodes::chat_turn::tests::chat_persist_builds_compatible_effect_without_writer --lib --target-dir D:\WorkSpace\ServerRS\.worktrees\.cargo-target-agent-effect
cargo test app::agent::chat_graph::tests::chat_graph_runs_the_complete_turn_and_persists_it --lib --target-dir D:\WorkSpace\ServerRS\.worktrees\.cargo-target-agent-effect
```

Expected: the executor and pure-node tests pass. The full graph test fails with `MissingEffectExecutor`, proving assembly must attach the executor.

- [ ] **Step 5: Attach `ChatEffectExecutor` and turn the graph test green**

Construct the executor before moving dependency fields into nodes/subgraphs:

```rust
let effect_executor = Arc::new(ChatEffectExecutor::new(Arc::clone(
    &dependencies.turn_writer,
)));
```

Attach it when constructing `ChatAgentGraph`:

```rust
Ok(Self {
    runtime: GraphRuntime::with_effect_executor(compiled, effect_executor),
    budget,
})
```

Run:

```powershell
cargo test app::agent::chat_graph::tests::chat_graph_runs_the_complete_turn_and_persists_it --lib --target-dir D:\WorkSpace\ServerRS\.worktrees\.cargo-target-agent-effect
```

Expected: pass, with the existing fake Writer call count still equal to one.

- [ ] **Step 6: Write the failing facade application-error test**

In the existing `agent_runtime.rs` test module, add:

```rust
#[test]
fn graph_facade_restores_effect_application_error_variants() {
    use crate::app::agent::graph::{
        EffectError, EffectId, NodeId, RunId, RunStep,
    };

    let error = GraphRunError::EffectFailed {
        node: NodeId::try_from("persist_turn").unwrap(),
        effect_id: EffectId::new(
            RunId::new(),
            RunStep::try_from(1).unwrap(),
            NodeId::try_from("persist_turn").unwrap(),
            0,
        ),
        error: EffectError::from_application(AppError::Conflict("turn changed".into())),
        completed_effect_ids: Vec::new(),
    };

    assert!(matches!(
        map_graph_run_error(error),
        AppError::Conflict(message) if message == "turn changed"
    ));
}
```

Run:

```powershell
cargo test app::agent::agent_runtime::tests::graph_facade_restores_effect_application_error_variants --lib --target-dir D:\WorkSpace\ServerRS\.worktrees\.cargo-target-agent-effect
```

Expected red: the current facade only unwraps `NodeFailed`, so it returns `AppError::Internal` for `EffectFailed`.

- [ ] **Step 7: Preserve Effect application errors in the facade**

Replace `map_graph_run_error` with:

```rust
fn map_graph_run_error(error: GraphRunError) -> AppError {
    let application_error = match &error {
        GraphRunError::NodeFailed { error, .. } => error.application_error(),
        GraphRunError::EffectFailed { error, .. } => error.application_error(),
        _ => None,
    };
    if let Some(application_error) = application_error {
        return application_error.clone();
    }
    AppError::Internal(format!("Agent 图运行失败: {error}"))
}
```

Rerun the test from Step 6. Expected: pass with the original `AppError::Conflict` and message.

- [ ] **Step 8: Run Chat vertical-slice and compatibility tests**

Run:

```powershell
cargo test --test chat_effect_persistence --target-dir D:\WorkSpace\ServerRS\.worktrees\.cargo-target-agent-effect
cargo test app::agent::nodes::chat_turn --lib --target-dir D:\WorkSpace\ServerRS\.worktrees\.cargo-target-agent-effect
cargo test app::agent::chat_graph --lib --target-dir D:\WorkSpace\ServerRS\.worktrees\.cargo-target-agent-effect
cargo test app::agent::agent_runtime --lib --target-dir D:\WorkSpace\ServerRS\.worktrees\.cargo-target-agent-effect
```

Expected: executor/payload/Receipt tests pass; the pure node produces one Effect without a Writer; the full graph writes one turn; AgentRuntime compatibility tests pass without changed output or application-error variants.

- [ ] **Step 9: Format, inspect, and commit Task 6**

```powershell
cargo fmt --all
git diff --check
git diff --name-only
```

Confirm no QQ Bot or NapCat path appears, then commit:

```powershell
git add tests/chat_effect_persistence.rs src/app/agent/chat_effect.rs src/app/agent/mod.rs src/app/agent/chat_state.rs src/app/agent/nodes/chat_turn.rs src/app/agent/nodes/mod.rs src/app/agent/chat_graph.rs src/app/agent/agent_runtime.rs
git commit -m "重构：通过 Effect 持久化 Agent 会话回合"
```

---

### Task 7: Full non-QQ verification and scope audit

**Files:**
- Verify only; modify production files only if a new failure is proven to originate in Tasks 1-6.

**Interfaces:**
- Consumes: the completed vertical slice.
- Produces: evidence that default non-QQ behavior is clean and the branch contains no QQ Bot changes.

- [ ] **Step 1: Run formatting and whitespace gates**

```powershell
cargo fmt --all -- --check
git diff --check 793a54c..HEAD
```

Expected: both commands exit 0.

- [ ] **Step 2: Run default compilation and complete tests**

```powershell
cargo check --target-dir D:\WorkSpace\ServerRS\.worktrees\.cargo-target-agent-effect
cargo test --target-dir D:\WorkSpace\ServerRS\.worktrees\.cargo-target-agent-effect
```

Expected: both commands exit 0; the four existing environment-dependent tests remain ignored unless their baseline changed independently.

- [ ] **Step 3: Audit the branch scope**

```powershell
git diff --name-status 793a54c..HEAD
git diff --name-only 793a54c..HEAD -- ":(glob)**/qq_bot/**" ":(glob)**/qq_bot*" ":(glob)**/napcat/**" ":(glob)**/napcat*"
git status --short --branch
```

Expected: the QQ/NapCat query prints nothing and the worktree is clean.

- [ ] **Step 4: Reconfirm the known all-features baseline limitation**

```powershell
cargo check --all-features --target-dir D:\WorkSpace\ServerRS\.worktrees\.cargo-target-agent-effect
```

Expected at this base: non-zero exit caused only by the two pre-existing old `user_repository` imports in `src/app/qq_bot/profile_builder.rs` and `src/bootstrap/qq_bot.rs`. If any non-QQ error appears, stop and fix that regression with a new failing test. Do not edit either excluded QQ file.

- [ ] **Step 5: Review commit history and request code review**

```powershell
git log --oneline --decorate 793a54c..HEAD
git status --short --branch
```

Expected: design/plan commits plus focused implementation commits are present, and the worktree is clean. Invoke the requesting-code-review workflow before proposing merge into `Main`.
