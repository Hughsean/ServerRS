# Agent Checkpoint + Suspend/Resume 垂直切片设计

## 1. 背景

当前 Agent 图运行时已经具备类型化状态、显式 Update、顺序与条件路由、预算、取消与超时、类型化 Effect、稳定 Effect ID、候选状态提交和 Effect Receipt。当前运行仍有四项恢复缺口：

1. 每次运行只能从图入口开始。
2. `RunContext` 只能创建新的 `RunId` 和空 Usage。
3. 成功 Receipt 只存在于当前 `GraphRunResult`。
4. 取消、超时和失败都只能结束运行，无法表达“当前节点已完整成功，等待外部输入后继续”。

本阶段增加一个最小但完整的 Checkpoint/Suspend/Resume 闭环，固定运行协议与恢复语义。第一版只使用进程内 Store，不承诺跨进程恢复。

## 2. 目标

- 节点可以通过类型化 `NodeResult::Suspend` 请求在当前节点结束后暂停。
- Runtime 只在节点 Update 和全部 Effect/Receipt Update 成功后生成 Checkpoint。
- Checkpoint 记录下一待执行节点，不记录需要重跑的当前节点。
- Resume 复用原 `RunId`、累计 Usage 和逻辑 `RunStep`。
- 恢复前的 Effect 不会再次执行，Receipt 不丢失。
- Graph、Graph Version 和 State Schema Version 不兼容时显式拒绝恢复。
- `UnknownCommit` 永远不能形成可自动恢复的安全点。
- 现有 `GraphRuntime::run`、`run_with_context` 和 `AgentRuntime::respond` 的普通完成行为保持兼容。
- 第一版提供线程安全的 `InMemoryCheckpointStore` 和完整协议测试。

## 3. 明确不做

- 不新增数据库表、列、索引、实体或迁移。
- 不实现跨进程或进程重启后的恢复。
- 不实现动态配置图、并行 Super-step 或任意 async 栈恢复。
- 不实现人工审批 UI。
- 不提供跨进程 Effect exactly-once 承诺。
- 不自动重试 `UnknownCommit`。
- 不把 LLM、工具调用和上下文读取整体迁移为 Effect。
- 不修改 QQ Bot、NapCat 或其装配代码。
- 不修复 QQ Bot 导致的 `cargo check --all-features` 基线问题。

## 4. 核心类型

### 4.1 标识与版本

新增以下强类型：

- `CheckpointId`：每次成功形成暂停点时生成的新 UUID。
- `GraphVersion`：非零整数版本；现有图默认版本为 1。
- `StateSchemaVersion`：非零整数版本；现有业务状态默认版本为 1。
- `RunPosition`：包含已经完成的 `RunStep` 和下一待执行 `NodeId`。

`GraphDefinition::new` 保留现有调用方式并使用 `GraphVersion::initial()`。另增显式版本构造入口。状态版本由业务状态协议暴露；不兼容的状态布局变更必须提升版本。

### 4.2 业务暂停与恢复协议

`AgentBusinessState` 增加两个关联类型和一个恢复映射：

- `SuspendData`：节点暂停时保存的业务关联数据，例如审批单号或外部任务标识。
- `ResumeInput`：恢复时调用方必须提供的类型化业务输入。
- `resume_updates(input)`：把 `ResumeInput` 转换为显式 `AgentUpdate<Self::Update>` 列表。

不需要业务数据的状态使用 `()`。Runtime 不接受无类型 JSON，也不直接修改业务字段。恢复输入产生的全部 Update 必须先在候选状态上原子成功，之后才允许消费 Checkpoint 和执行下一节点。

### 4.3 NodeResult

`NodeResult` 明确区分：

- `Continue`：携带 Updates、Effects 和 Usage，成功后按现有路由继续。
- `Suspend`：携带同样的节点执行内容以及 `SuspendRequest<SuspendData>`。

现有 `new`、`empty`、`with_effect` 和 `with_effects` 构造方法继续生成 `Continue`，避免普通节点产生无意义改动。新增专用 Suspend 构造方式。Suspend 不是提前返回：其 Updates、Effects、Receipt Updates 和路由选择仍按正常节点协议执行。

`SuspendRequest` 包含公共的 `SuspendReason` 与业务 `SuspendData`。业务关联标识放在 `SuspendData` 中，不使用 Runtime 可解释的字符串字段。

### 4.4 AgentCheckpoint

`AgentCheckpoint<B>` 保存：

- Checkpoint ID；
- Graph ID 和 Graph Version；
- State Schema Version；
- 原 Run ID；
- `RunPosition`；
- 当前 `AgentState<B>`；
- 原 `RunBudget` 限制；
- 累计 `UsageSnapshot`；
- 累计已访问节点；
- 累计成功 Effect Receipt；
- `SuspendRequest<B::SuspendData>`；
- `RunTrace` 的纯数据内容。

Checkpoint 不保存 Runtime、CheckpointStore、EffectExecutor、Provider、Repository、数据库连接、`Arc`、取消令牌、Deadline、Future 或其他运行依赖。Checkpoint 模块通过架构守卫检查这些禁止依赖。

第一版不把序列化能力强加给所有业务状态。未来数据库适配器必须在独立阶段为其持久化 DTO 增加明确的序列化和兼容边界。

## 5. Runtime 与 Store 接口

### 5.1 兼容入口

以下现有入口保持原签名和普通完成行为：

- `GraphRuntime::run(...)`
- `GraphRuntime::run_with_context(...)`
- `AgentRuntime::respond(...)`

Checkpoint 图使用新增的 Checkpoint 专用启动入口。该入口返回 `GraphExecutionResult<B>`：

- `Completed(GraphRunResult<B>)`
- `Suspended(SuspendedRun<B>)`

如果节点通过旧的完成式入口请求 Suspend，Runtime 在应用该结果的 Usage、Updates 和 Effects 之前返回明确的协议错误，不把暂停伪装成完成，也不留下部分提交。包含 Suspend 节点的调用方必须使用 Checkpoint 专用入口。

### 5.2 CheckpointStore

`CheckpointStore<B>` 是异步、线程安全的应用层接口，提供：

- 保存新 Checkpoint，拒绝覆盖已有 ID；
- 加载 Checkpoint 快照用于兼容性和 ResumeInput 校验；
- 原子消费 Checkpoint，保证同一 ID 只能进入一次恢复执行。

`InMemoryCheckpointStore<B>` 使用进程内线程安全 Map。它不提供数据库适配、租约、分布式锁、过期回收或进程重启恢复。

`GraphRuntime` 通过可选的 Store 依赖启用 Checkpoint 能力。普通图不需要配置 Store。节点请求 Suspend 但 Runtime 未配置 Store 时返回明确错误。

### 5.3 Resume 入口

新增：

- 使用默认取消控制恢复的 `resume(checkpoint_id, resume_input)`；
- 使用调用方取消控制与追踪信息恢复的上下文入口。

恢复上下文只能提供本次活动期间的取消控制和追踪补充，不能覆盖 Checkpoint 中的 `RunId`、预算限制或累计 Usage。

## 6. 暂停执行顺序

节点返回 `Suspend` 时，Runtime 严格按以下顺序处理：

1. `RunContext::check_ready` 预留当前 `RunStep`。
2. 执行节点 Future。
3. 记录 Usage，并在 Effect 前完成预算校验。
4. 在候选状态应用节点 Updates。
5. 顺序执行 Effects，并生成稳定 Effect ID。
6. 在候选状态应用每个 Receipt Update。
7. 全部成功后提交候选状态、Receipts 和 visited node。
8. 执行 `Goto` 或 `Branch` 路由，确定下一节点。
9. 构造包含已提交状态和下一节点的 Checkpoint。
10. 保存 Checkpoint。
11. 返回 `Suspended`。

`TransitionRule::End` 与 `NodeResult::Suspend` 同时出现是协议错误，因为不存在可恢复的下一节点。

Checkpoint 保存发生在 Effect 成功之后，因此 Store 保存失败不能通过重跑当前节点补偿。保存失败错误必须携带尚未保存的 Checkpoint 数据，并明确保留已经成功的 Effect/Receipt；Runtime 不自动重试节点或 Effect。

## 7. 恢复执行顺序

Resume 严格按以下顺序处理：

1. 加载 Checkpoint 快照。
2. 校验 Graph ID。
3. 校验 Graph Version。
4. 校验 State Schema Version。
5. 校验下一节点仍存在。
6. 在 Checkpoint 状态的候选副本应用 ResumeInput Updates。
7. 原子消费原 Checkpoint；若已被消费则停止。
8. 用原 Run ID、原预算限制、累计 Usage 和保存的 Trace 重建 RunContext。
9. 为本次恢复建立新的取消令牌和 Deadline。
10. 从保存的下一节点继续运行。
11. 返回 `Completed` 或保存一个具有新 Checkpoint ID 的 `Suspended`。

暂停等待时间不计入活动运行 Deadline。每次 Resume 获得新的活动时间窗口；steps、LLM calls、tool calls 和 tokens 始终累计，下一逻辑步骤为 `usage.steps + 1`。

ResumeInput 校验失败发生在消费 Checkpoint 之前，因此调用方可以修正输入后再次尝试。正式恢复开始后原 Checkpoint 已被消费；如果后续节点或 Effect 失败，不自动重新激活旧 Checkpoint，避免重放本次恢复期间可能已经提交的 Effect。

## 8. Effect 与 UnknownCommit

- Checkpoint 保存累计完整 Receipt，而不仅是临时调用结果。
- 恢复从下一节点开始，因此暂停节点的 Effect 不会再次分配或执行。
- Effect ID 继续使用 `RunId + RunStep + NodeId + Ordinal`。
- 恢复复用 Run ID 且继续递增 RunStep，因此恢复前后 Effect ID 稳定且不碰撞。
- 当前节点任一 Effect 失败时不生成 Checkpoint。
- Effect 执行期间取消或超时继续产生 `UnknownCommit`。
- `UnknownCommit` 直接终止运行，不保存安全 Checkpoint，不自动重试，也不自动恢复。

## 9. 错误模型

### 9.1 CheckpointError

- Checkpoint 不存在；
- Checkpoint ID 重复；
- Store 内部状态不可用。

### 9.2 ResumeError

- Graph ID 不匹配；
- Graph Version 不匹配；
- State Schema Version 不匹配；
- 下一节点不存在；
- ResumeInput Update 被拒绝；
- Checkpoint 已被其他恢复操作消费；
- 恢复后的图运行失败。

### 9.3 Checkpoint 运行错误

- 普通 `GraphRunError`；
- 未配置 CheckpointStore；
- Suspend 与 End 冲突；
- Checkpoint 保存失败，并携带未保存的 Checkpoint。

错误类型不得把 `UnknownCommit` 降级为可重试错误，也不得暗示外部副作用已回滚。

## 10. Checkpoint 生命周期与并发边界

- 每次 Suspend 生成新的 Checkpoint ID。
- Store 不允许覆盖同 ID 的 Checkpoint。
- ResumeInput 和版本校验期间原 Checkpoint 保持可用。
- 通过全部校验后，以原子消费操作取得 Checkpoint。
- 两个并发 Resume 最多一个可以消费成功；另一个得到已消费/不存在错误。
- 一旦开始执行恢复后的节点，旧 Checkpoint 不再恢复。
- 本阶段不提供租约超时、失败后自动回滚 claim 或跨进程并发语义。

## 11. 测试策略

所有生产代码遵循 RED-GREEN-REFACTOR：先写一个失败测试，确认因缺少目标能力失败，再写最小实现并运行相关回归测试。

### 11.1 类型和 Store 测试

- Checkpoint ID 唯一且强类型。
- Graph/State 版本默认值和显式值正确。
- Store 保存、加载、拒绝覆盖和原子消费正确。
- 同一 Checkpoint 只能消费一次。
- Checkpoint 源码不依赖运行时句柄和基础设施类型。

### 11.2 Runtime 协议测试

- 普通图的现有执行结果不变。
- Suspend 只发生在 Update、Effect 和 Receipt Update 全部成功后。
- 下一节点在暂停时不执行。
- Resume 从下一节点开始，不重跑暂停节点。
- Run ID 在暂停前后相同。
- RunStep 与 Usage 跨暂停单调累计。
- ResumeInput 在下一节点之前原子应用。
- 暂停前 Receipt 在完成结果中保留。
- 暂停前 Effect 不重复执行。
- 恢复后的 Effect ID 稳定且不碰撞。
- Graph ID、Graph Version、State Schema Version 不匹配时显式失败且不消费 Checkpoint。
- ResumeInput 更新失败时不执行节点且不消费 Checkpoint。
- Store 保存失败返回未保存 Checkpoint且不重跑 Effect。
- 未配置 Store 时 Suspend 显式失败。
- Suspend 与 End 冲突时显式失败。
- Effect `UnknownCommit` 时不生成 Checkpoint。
- 最小图完成一次运行、暂停、恢复、完成闭环。

### 11.3 兼容和范围验证

- `AgentRuntime::respond()` 的既有测试继续通过。
- Chat 图消息持久化继续只执行一次。
- 变更文件单独通过 rustfmt 检查。
- 默认 `cargo check` 通过。
- 默认完整 `cargo test` 通过。
- QQ Bot/NapCat、数据库、配置和依赖文件相对 `5fd6403` 零差异。

不以修改排除范围文件的方式追求 `cargo check --all-features` 通过。

## 12. 文件边界

新增：

- `src/app/agent/graph/checkpoint.rs`：标识、版本、运行位置、Suspend/Resume 数据模型和相关错误。
- `src/app/agent/graph/checkpoint_store.rs`：Store trait 与内存实现。

修改：

- `src/app/agent/graph/node.rs`：类型化 Continue/Suspend 结果。
- `src/app/agent/graph/runtime.rs`：Checkpoint 启动、暂停保存与 Resume 循环。
- `src/app/agent/graph/budget.rs`：从既有 Run ID 与 Usage 重建上下文。
- `src/app/agent/graph/definition.rs`：Graph Version。
- `src/app/agent/graph/error.rs`：协议错误映射。
- `src/app/agent/graph/mod.rs`：公开协议导出。
- `src/domain/agent/state.rs`：业务 SuspendData、ResumeInput 与恢复 Update 映射。
- `tests/architecture_guard.rs`：Checkpoint 纯数据边界守卫。

实现中可在不改变上述责任边界的前提下，把泛型错误类型放入 `checkpoint.rs`，避免让既有非泛型 `GraphRunError` 承载完整 Checkpoint。

## 13. 基线与完成条件

隔离工作树从 `5fd6403d19b0e1dfcc0e0edde3ecb3f85fb4fc2b` 创建。实施前验证：

- `cargo build`：通过。
- 默认完整 `cargo test`：507 passed，0 failed，4 ignored。

本阶段只有在第 11 节全部测试和范围验证通过后才视为完成。未经用户确认，不合并、不推送、不新增数据库能力。
