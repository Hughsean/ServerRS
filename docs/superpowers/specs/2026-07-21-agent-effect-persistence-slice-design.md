# Agent Effect 内核与会话持久化垂直切片设计

- 日期：2026-07-21
- 状态：设计已确认，待规格复核
- 分支：`codex/agent-effect-persistence`
- 适用范围：默认非 `qq_bot` 构建中的 Agent 图运行时与 HTTP ChatAgent

## 1. 背景与目标

第一阶段已经提供类型化 `AgentState<B>`、显式 `AgentUpdate`、顺序图运行器、`ChatAgentGraph` 和可复用 `ReasoningLoop`。当前图节点仍可直接持有 Repository 或 Provider；例如 `PersistTurnNode` 直接持有 `TurnWriterT` 并写入数据库。这使节点同时承担状态计算与外部 I/O，限制节点复用，也使未来 Checkpoint 恢复无法区分“尚未发出副作用”“副作用已成功”和“提交状态未知”。

本阶段采用一个最小但可运行的垂直切片：建立通用 Effect 协议与运行器执行路径，只迁移会话回合持久化。完成后，`PersistTurnNode` 只描述 `ChatEffect::PersistTurn`，`ChatEffectExecutor` 才持有 `TurnWriterT` 并执行写库；成功 Receipt 通过显式业务 Update 回写状态。

### 目标

- 节点通过 `NodeResult` 返回类型化 Effect，不直接执行会话持久化。
- 保持 `AgentRuntime::respond()`、HTTP 返回、消息 JSON、数据库写入和应用错误语义兼容。
- 为每个 Effect 建立由 `RunId + RunStep + NodeId + Ordinal` 构成的强类型 `EffectId`。
- 单次 Run 内不提供任何自动重试路径。
- 明确区分执行前取消、执行失败、提交状态未知、以及 Effect 成功后状态更新失败。
- 成功 Receipt 仅保存在本次 `GraphRunResult`，为后续 Checkpoint 提供协议基础。

## 2. 已确认边界

- 不修改数据库表、列、索引或迁移脚本。
- 不承诺跨进程、跨 Run 或 HTTP 请求重放时的幂等。
- 不实现 Checkpoint、Suspend、Resume 或 Receipt 持久化。
- 不迁移上下文读取、LLM 调用、工具执行或 Agent Event 写入。
- 不修改任何 QQ Bot、NapCat 或 `qq_bot` feature 专属文件。
- 不增加自动重试、退避、限流 Layer 或并行 Effect。
- `AgentRuntime::respond()` 的公开签名和兼容门面保持不变。

## 3. 分层原则

Effect 的执行是应用编排职责，因此 `EffectId`、`RunStep`、Envelope、Receipt、Executor 和错误分类均位于 `app::agent::graph`。

`AgentBusinessState` 位于 Domain，不能依赖应用层的 `AgentEffect` trait。它只声明一个无应用层约束的关联类型：

```rust
pub trait AgentBusinessState: Clone + Send + Sync + 'static {
    type Update: Send + Sync + 'static;
    type Effect: Send + Sync + 'static;

    fn apply_update(&mut self, update: Self::Update) -> Result<(), AgentStateError>;
}
```

应用层在运行图时施加约束：

```rust
where
    B: AgentBusinessState,
    B::Effect: AgentEffect<Update = B::Update>,
```

这样不会形成 `domain -> app` 的反向依赖，也不需要把独立 `E` 泛型扩散到 `GraphDefinition<B>`、`GraphFragment<B>`、Router 和所有调用方。

纯图和测试业务状态使用不可构造的 `NoEffect`。Domain 自身的状态单元测试可将 `Effect` 设为 `()`，因为它们不实例化应用层运行器。

## 4. 核心类型

### 4.1 `RunStep` 与 `EffectId`

```rust
pub struct RunStep(NonZeroU32);

pub struct EffectId {
    run_id: RunId,
    step: RunStep,
    node_id: NodeId,
    ordinal: u32,
}
```

- `RunStep` 是逻辑节点执行序号，第一步为 1。
- `RunContext::check_ready` 在通过取消、截止时间和预算检查后预留一步，并返回本次 `RunStep`。
- `Ordinal` 是节点返回的 Effect 在 `NodeResult.effects` 中的零基序号。
- 同一个 `RunId`、`RunStep`、`NodeId` 和 `Ordinal` 必须生成相等的 `EffectId`；任一组成部分不同则 ID 不同。
- 本阶段只固定 ID 语义和确定性测试，不提供恢复构造入口。Checkpoint 阶段必须恢复原 `RunId` 与逻辑 Step，不能新建 Run 后声称 Effect ID 稳定。

### 4.2 Effect 协议

```rust
pub trait AgentEffect: Send + Sync + 'static {
    type Update: Send + Sync + 'static;
    type Receipt: Send + Sync + 'static;

    fn receipt_updates(receipt: &Self::Receipt) -> Vec<AgentUpdate<Self::Update>>;
}

pub struct EffectEnvelope<E> {
    pub id: EffectId,
    pub effect: E,
}

pub struct EffectReceipt<R> {
    pub effect_id: EffectId,
    pub value: R,
}
```

`AgentEffect` 不包含重试 API。本阶段运行器没有 retry loop；未来只有在 Effect 具备可证明的幂等键或外部去重能力后，才允许增加重试策略。

### 4.3 Executor 协议

```rust
#[async_trait]
pub trait EffectExecutor<E: AgentEffect>: Send + Sync {
    async fn execute(
        &self,
        envelope: &EffectEnvelope<E>,
        context: &RunContext,
    ) -> Result<E::Receipt, EffectError>;
}
```

`GraphRuntime<B>` 可通过专用构造方法附加 `Arc<dyn EffectExecutor<B::Effect>>`。不产生 Effect 的现有图可以继续使用无 Executor 的构造方式；若节点实际返回 Effect 但 Runtime 没有 Executor，则返回 `MissingEffectExecutor`，不能静默忽略。

### 4.4 `NodeResult`

```rust
pub struct NodeResult<U, E> {
    pub updates: Vec<AgentUpdate<U>>,
    pub effects: Vec<E>,
    pub usage: UsageDelta,
}
```

- 现有 `NodeResult::new(updates, usage)` 保持为无 Effect 的便捷构造，减少纯节点迁移噪声。
- 新增 `NodeResult::with_effect` 和 `NodeResult::with_effects`。
- `AgentNode<B>::execute` 返回 `NodeResult<B::Update, B::Effect>`。

## 5. Runtime 执行顺序

每个节点按以下顺序执行：

1. `RunContext::check_ready` 检查预取消、截止时间与步数预算，并返回 `RunStep`。
2. 执行节点 Future；节点仍受 Run 的取消和截止时间控制。
3. 在 `AgentState` 候选副本上应用节点的纯 Updates。若失败，立即返回，任何 Effect 都不得发出。
4. 按 `effects` 顺序为每项分配 `EffectId`，构造 Envelope。
5. Effect 发出前再次检查取消和截止时间。此时失败表示外部调用尚未开始。
6. 执行器 Future 一旦进入 Runtime 的执行选择分支，就视为可能已向外部系统发出请求。
7. 执行成功后生成 `EffectReceipt`，通过 `AgentEffect::receipt_updates` 得到显式 Updates，并应用到候选 State。
8. 全部 Effects 和 Receipt Updates 成功后，才用候选 State 替换当前 State，记录节点 Usage 与 Receipts，然后执行路由。

候选 State 只能提供内存状态原子性，不能回滚已经提交的外部副作用。如果同一节点未来返回多个 Effects，前序 Effect 成功而后序 Effect 失败时，错误必须携带已完成的 Effect ID 列表，不能暗示外部操作整体回滚。本阶段生产图中的持久化节点只产生一个 Effect。

`GraphRunResult<B>` 新增类型化 `effect_receipts`。Receipt 不写数据库、不写 Checkpoint，也不跨 Run 复用。

## 6. 错误和取消语义

### 6.1 Effect 错误分类

```rust
pub enum EffectErrorKind {
    Transient,
    Permanent,
    Timeout,
    UnknownCommit,
}
```

`EffectError` 保存分类、稳定的安全消息，以及可选的 `AppError`。普通 Executor 错误继续保留原应用错误，使 `AgentRuntime` 兼容门面能够恢复既有 HTTP 错误语义。

### 6.2 运行错误

| 场景 | 结果 |
|---|---|
| Effect 发出前已取消 | `GraphRunError::Cancelled` |
| Effect 发出前已超过截止时间 | `GraphRunError::DeadlineExceeded` |
| Executor 明确报告失败 | `GraphRunError::EffectFailed { node, effect_id, error, completed_effect_ids }` |
| Executor 执行中 Runtime 截止或取消 | `EffectErrorKind::UnknownCommit`；不重试 |
| Effect 成功，但 Receipt Update 被 State 拒绝 | `GraphRunError::PostEffectStateUpdateFailed { node, effect_id, error }` |
| 节点返回 Effect，但 Runtime 未配置 Executor | `GraphRunError::MissingEffectExecutor { node }` |

`UnknownCommit` 只表达“Runtime 无法确认外部提交状态”，不能用于调用前取消，也不能替代 Executor 已知的 `Transient`、`Permanent` 或 `Timeout`。

## 7. Chat 持久化垂直切片

### 7.1 描述与回执

```rust
pub enum ChatEffect {
    PersistTurn(PersistTurnEffect),
}

pub struct PersistTurnEffect {
    pub conversation_id: u64,
    pub user_id: u64,
    pub user: NewConversationMessage,
    pub assistant: NewConversationMessage,
}

pub enum ChatEffectReceipt {
    TurnPersisted(PersistedTurn),
}
```

`ChatEffect` 实现 `AgentEffect<Update = ChatTurnUpdate, Receipt = ChatEffectReceipt>`。`TurnPersisted` 只转换为：

```rust
AgentUpdate::Business(ChatTurnUpdate::SetPersistedTurn(...))
```

### 7.2 `PersistTurnNode`

- 移除 `Arc<dyn TurnWriterT>` 字段和构造参数。
- 继续验证 `AgentOutcome`、conversation ID、user ID，并生成与现有实现完全相同的 user/assistant JSON。
- 不调用 Repository，不持有 Provider/Repository/Writer。
- 返回一个 `ChatEffect::PersistTurn`，不直接返回 `SetPersistedTurn` Update。

### 7.3 `ChatEffectExecutor`

- 持有 `Arc<dyn TurnWriterT>`。
- 仅匹配并执行 `ChatEffect::PersistTurn`。
- 继续调用 `save_turn_atomic`，成功后返回 `ChatEffectReceipt::TurnPersisted`。
- `AppError` 转换为 `EffectError` 时保留原错误对象。
- Runtime 负责取消、截止时间、Effect ID、Receipt 记录与状态更新；Executor 不自行重试。

`TurnWriterT` 与 `ConversationTurnWriter` 移入 `chat_effect.rs`，因为它们属于持久化 Effect 的执行边界，不再属于节点实现。

### 7.4 图装配

- `ChatAgentGraphDeps` 继续接收 `turn_writer`，避免上层 Bootstrap 改动。
- `PersistTurnNode::new` 只接收 `NodeId`。
- `ChatAgentGraph` 用 `turn_writer` 构造 `ChatEffectExecutor`，并把 Executor 注入 `GraphRuntime`。
- `ChatTurnState::Effect = ChatEffect`。
- 推理、上下文和 Prompt 节点继续返回空 Effect 列表，本阶段不迁移其外部 I/O。

## 8. 兼容性要求

- `AgentRuntime::new` 与 `AgentRuntime::respond` 的公开签名不变。
- 成功回复、fallback 文本、工具追踪和 memory metadata 不变。
- 会话写入仍使用同一次 `save_turn_atomic`，user/assistant 消息字段与 JSON 字符串保持一致。
- 持久化成功后返回的 user/assistant message ID 仍写入 `ChatTurnState`。
- Repository 返回的 `AppError` 仍映射为相同的上层应用错误。
- 不增加配置项、Cargo 依赖、环境变量或数据库迁移。

## 9. 文件边界

### 新增

- `src/app/agent/graph/effect.rs`：`RunStep`、`EffectId`、Envelope、Receipt、`AgentEffect`、`EffectExecutor`、`NoEffect`、Effect 错误。
- `src/app/agent/chat_effect.rs`：Chat Effect、Receipt、Executor、`TurnWriterT`、`ConversationTurnWriter`。

### 修改

- `src/domain/agent/state.rs`：增加无应用层约束的 `AgentBusinessState::Effect`。
- `src/app/agent/graph/node.rs`：`NodeResult` 携带 Effects。
- `src/app/agent/graph/runtime.rs`：Effect 执行、候选 State、Receipt 收集与错误语义。
- `src/app/agent/graph/budget.rs`：节点预留返回 `RunStep`。
- `src/app/agent/graph/error.rs`、`id.rs`、`mod.rs`：导出和运行错误整合。
- `src/app/agent/chat_state.rs`：关联 `ChatEffect`。
- `src/app/agent/nodes/chat_turn.rs`：持久化节点纯化并移除 Writer 实现。
- `src/app/agent/nodes/mod.rs`、`src/app/agent/mod.rs`：调整导出。
- `src/app/agent/chat_graph.rs`：装配 `ChatEffectExecutor`。
- `src/app/agent/agent_runtime.rs`：仅调整 `ConversationTurnWriter` 的导入路径。
- 实现 `AgentBusinessState` 的图测试与测试业务类型：显式选择 `NoEffect` 或对应业务 Effect。

### 硬排除

- `src/app/qq_bot/**`
- `src/infra/qq_bot/**`
- `src/bootstrap/qq_bot.rs`
- `src/bootstrap/graph/qq_bot_provider.rs`
- `src/bin/qq_bot_init.rs`
- 其他路径名包含 `qq_bot`、`napcat` 的业务文件

## 10. 测试策略

所有生产代码变更遵循红—绿—重构：先增加一个能因缺少目标能力而失败的测试，确认失败原因正确，再写最小实现。

### Effect 内核

- 相同四元组生成相同 `EffectId`；Run、Step、Node、Ordinal 任一不同均生成不同 ID。
- `check_ready` 返回从 1 开始单调递增的 `RunStep`，并继续执行原步数预算限制。
- 节点纯 Update 在候选 State 上失败时，Executor 调用次数为 0。
- 无 Executor 时节点返回 Effect，得到 `MissingEffectExecutor`。
- 成功 Effect 只执行一次，Receipt Update 生效，Receipt 出现在 `GraphRunResult`。
- Executor 返回错误时调用次数严格为 1，Runtime 不重试。
- Effect 发出前预取消不会调用 Executor。
- Executor 已进入执行后再取消或到达截止时间，返回 `UnknownCommit`。
- Effect 成功但 Receipt Update 失败时，返回 `PostEffectStateUpdateFailed`。
- 多 Effect 的后序失败错误包含此前成功的 Effect IDs。

### Chat 垂直切片

- `PersistTurnNode` 生成正确 `PersistTurnEffect`，不会访问 Writer。
- `ChatEffectExecutor` 将 Effect 中的两个消息原样交给 `save_turn_atomic`，且只调用一次。
- Writer 错误保留原 `AppError`。
- Receipt 精确转换为 `SetPersistedTurn`。
- 完整 `ChatAgentGraph` 仍写入一次 user/assistant 回合并返回相同响应。
- 既有 AgentRuntime 兼容测试继续通过。

## 11. 基线与完成门槛

隔离分支基于 `793a54c`。当前基线结果：

- `cargo build`：通过。
- `cargo test`：490 passed，4 ignored，0 failed。
- `cargo build --all-features`：在本阶段开始前即失败；原因是 `793a54c` 将 `domain::user::user_repository` 重命名为 `user_repo`，但两个被排除的 QQ Bot 文件仍引用旧模块。该失败不由本阶段修复，也不能把 QQ Bot 文件带入差异。

本阶段完成门槛：

- `cargo fmt --all -- --check`
- `cargo check`
- `cargo test`
- `git diff --check`
- 相对基点 `793a54c` 的变更路径不包含 `qq_bot` 或 `napcat`
- 可选复跑 `cargo check --all-features` 时，只允许复现上述两个既有 QQ Bot 导入错误；不得出现新的非 QQ 错误

## 12. 风险与权衡

- 没有持久化 Receipt，进程崩溃后不能判定 Effect 是否已提交；这是本阶段明确接受的限制。
- Runtime 截止或取消只能停止等待，不能保证数据库驱动或远端系统回滚，因此必须返回 `UnknownCommit`，且不得自动重试。
- Candidate State 能保证内存更新在节点边界提交，但不能为多个外部 Effects 提供分布式事务。
- `AgentBusinessState::Effect` 会要求现有业务状态和测试类型显式选择 Effect 类型；这是避免图系统额外泛型扩散的有意成本。
- 仅迁移持久化意味着 LLM、工具和上下文节点暂时仍持有外部依赖；后续按独立垂直切片迁移，不在本次扩大范围。

## 13. 已冻结决策

1. 采用持久化垂直切片，不一次迁移全部外部 I/O。
2. Effect 基础设施属于 `app::agent::graph`；Domain 关联类型不绑定应用层 trait。
3. Effect ID 使用 `RunId + RunStep + NodeId + Ordinal`，Step 从 1 开始。
4. 本阶段无自动重试、无数据库变更、无跨进程幂等声明。
5. Runtime 执行中取消或超时使用 `UnknownCommit`，发出前取消不使用该分类。
6. Receipt 通过显式业务 Update 修改 State，并仅保存在本次运行结果。
7. QQ Bot 全部排除；其既有 all-features 编译失败不在本阶段修复。
