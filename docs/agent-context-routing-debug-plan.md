# Agent 上下文路由 Debug 可观测实施计划

> **给 agentic worker 的要求：** 必须使用子技能 `superpowers:subagent-driven-development`（推荐）或 `superpowers:executing-plans` 按任务逐步实施本计划。步骤使用复选框（`- [ ]`）格式，便于执行时跟踪进度。

**目标：** 为 Agent 上下文路由链路增加 debug 级可观测日志，同时不把路由诊断暴露到 prompt。

**架构：** 路由继续作为 `AgentContextBuilder` 的内部职责。只在 `src/app/agent/agent_context.rs` 中增加两个聚焦的辅助函数：一个记录路由决策或路由未启用状态，另一个记录最终召回数量。现有路由服务、bootstrap 接线、prompt 结构和 agent event 持久化都保持不变。

**技术栈：** Rust、Tokio、`tracing::debug!`、现有 `ContextRoutingService`、现有 `AgentContextBuilder` 测试。

## 全局约束

- 不把路由诊断写入 `AgentContext` 或系统提示词。
- 不把路由诊断写入 `agent_events`。
- 不新增数据库 schema 或持久化状态。
- 不修改 taxonomy、prototype、阈值或工具调用行为。
- 新增可观测日志必须使用 `debug!`。
- 保持当前 fallback 行为：路由或检索问题不能阻断 agent 回复。
- 不提交被忽略的文件，例如 `config.toml`、`.env`、`docs/superpowers` 或 `docs/design`。
- 提交信息使用中文；必要时可以保留技术英文。

---

## 文件结构

- 修改：`src/app/agent/agent_context.rs`
  - 职责：构建单轮 `AgentContext`，应用上下文路由预算，并输出路由决策与召回数量的 debug 日志。
- 保持不变：`src/app/context_routing/mod.rs`
  - 职责：生成 `ContextRouteDecision`。本次不需要修改。
- 保持不变：`src/app/agent/prompt_builder.rs`
  - 职责：根据 `AgentContext` 构建系统提示词。不能新增路由诊断内容。
- 保持不变：`src/test/context_router.rs`
  - 职责：手动运行的 ignored 真实 embedding 测试。本次 debug 日志集成不需要修改它。

---

### 任务 1：为 AgentContextBuilder 增加 Debug 日志

**文件：**
- 修改：`src/app/agent/agent_context.rs`

**接口：**
- 消费：`ContextRouteDecision`、`AgentContextBuilder::build(...)`、`routed_memory_top_k(...)`、`routed_rag_top_k(...)`
- 产出：
  - `fn log_context_route_decision(user_id: u64, conversation_id: Option<u64>, decision: Option<&ContextRouteDecision>, default_memory_top_k: u32, default_rag_top_k: u64)`
  - `fn log_context_retrieval_counts(user_id: u64, conversation_id: Option<u64>, decision: Option<&ContextRouteDecision>, memory_top_k: u32, rag_top_k: u64, memories_count: usize, rag_chunks_count: usize, fresh_chunks_count: usize)`

- [ ] **步骤 1：先写失败测试**

把下面测试追加到 `src/app/agent/agent_context.rs` 现有的 `#[cfg(test)] mod tests` 中：

```rust
#[test]
fn route_decision_debug_helpers_accept_disabled_and_enabled_paths() {
    let decision = ContextRouteDecision {
        fresh_context: FreshContextRoute {
            enabled: true,
            confidence: 0.91,
        },
        memory: RetrievalBudgetRoute {
            top_k: 7,
            confidence: 0.82,
            reason: "memory_positive".into(),
        },
        rag: RetrievalBudgetRoute {
            top_k: 2,
            confidence: 0.66,
            reason: "rag_low_confidence".into(),
        },
        diagnostics: ContextRouteDiagnostics {
            taxonomy: "context_routing".into(),
            top_labels: vec![
                ("context.memory.positive".into(), 0.82),
                ("context.rag.positive".into(), 0.66),
            ],
            fallback_used: false,
        },
    };

    log_context_route_decision(42, Some(9), Some(&decision), 10, 5);
    log_context_retrieval_counts(42, Some(9), Some(&decision), 7, 2, 3, 2, 1);
    log_context_route_decision(42, None, None, 10, 5);
    log_context_retrieval_counts(42, None, None, 10, 5, 0, 0, 0);
}
```

- [ ] **步骤 2：运行测试，确认它失败**

运行：

```powershell
cargo test route_decision_debug_helpers_accept_disabled_and_enabled_paths --lib
```

预期：失败，错误原因是找不到 `log_context_route_decision` 和 `log_context_retrieval_counts`。

- [ ] **步骤 3：导入 `debug`**

把 `src/app/agent/agent_context.rs` 顶部附近的 tracing import 从：

```rust
use tracing::warn;
```

改为：

```rust
use tracing::{debug, warn};
```

- [ ] **步骤 4：在路由完成后记录路由决策**

在 `AgentContextBuilder::build(...)` 中，找到 `routing_decision` 代码块。在该代码块之后、`let memory_top_k = ...` 之前插入：

```rust
        log_context_route_decision(
            user_id,
            conversation_id,
            routing_decision.as_ref(),
            max_memory_items,
            max_rag_chunks,
        );
```

- [ ] **步骤 5：在召回完成后记录召回数量**

在 `AgentContextBuilder::build(...)` 中，找到 `fresh_chunks` 的计算逻辑。在它之后、`let profile: Option<Value> = ...` 之前插入：

```rust
        log_context_retrieval_counts(
            user_id,
            conversation_id,
            routing_decision.as_ref(),
            memory_top_k,
            rag_top_k,
            memories.len(),
            rag_chunks.len(),
            fresh_chunks.len(),
        );
```

- [ ] **步骤 6：增加辅助函数**

把下面函数添加到 `routed_rag_top_k(...)` 之后、`#[cfg(test)] mod tests` 之前：

```rust
fn log_context_route_decision(
    user_id: u64,
    conversation_id: Option<u64>,
    decision: Option<&ContextRouteDecision>,
    default_memory_top_k: u32,
    default_rag_top_k: u64,
) {
    match decision {
        Some(decision) => {
            debug!(
                user_id,
                ?conversation_id,
                routing_enabled = true,
                taxonomy = %decision.diagnostics.taxonomy,
                fresh_enabled = decision.fresh_context.enabled,
                fresh_confidence = decision.fresh_context.confidence,
                memory_top_k = decision.memory.top_k,
                memory_reason = %decision.memory.reason,
                memory_confidence = decision.memory.confidence,
                rag_top_k = decision.rag.top_k,
                rag_reason = %decision.rag.reason,
                rag_confidence = decision.rag.confidence,
                fallback_used = decision.diagnostics.fallback_used,
                top_labels = ?decision.diagnostics.top_labels,
                "Agent 上下文路由决策"
            );
        }
        None => {
            debug!(
                user_id,
                ?conversation_id,
                routing_enabled = false,
                memory_top_k = default_memory_top_k,
                rag_top_k = default_rag_top_k,
                "Agent 上下文路由未启用，使用默认召回预算"
            );
        }
    }
}

fn log_context_retrieval_counts(
    user_id: u64,
    conversation_id: Option<u64>,
    decision: Option<&ContextRouteDecision>,
    memory_top_k: u32,
    rag_top_k: u64,
    memories_count: usize,
    rag_chunks_count: usize,
    fresh_chunks_count: usize,
) {
    debug!(
        user_id,
        ?conversation_id,
        routing_enabled = decision.is_some(),
        fresh_enabled = decision
            .map(|decision| decision.fresh_context.enabled)
            .unwrap_or(false),
        memory_top_k,
        rag_top_k,
        memories_count,
        rag_chunks_count,
        fresh_chunks_count,
        "Agent 上下文召回完成"
    );
}
```

- [ ] **步骤 7：运行聚焦测试，确认通过**

运行：

```powershell
cargo test route_decision_debug_helpers_accept_disabled_and_enabled_paths --lib
```

预期：通过。

- [ ] **步骤 8：运行相关既有测试**

运行：

```powershell
cargo test agent_context --lib
```

预期：`agent_context` 相关测试通过。

- [ ] **步骤 9：提交**

运行：

```powershell
git add -- src/app/agent/agent_context.rs
git commit -m "feat: 增加 Agent 上下文路由 debug 日志"
```

---

### 任务 2：验证完整回归和真实路由测试

**文件：**
- 仅验证：`src/app/agent/agent_context.rs`
- 仅验证：`src/test/context_router.rs`

**接口：**
- 消费：任务 1 增加的 debug 辅助函数。
- 产出：一个已验证的工作分支，并确认没有新增 prompt 或 event 持久化改动。

- [ ] **步骤 1：确认没有把路由诊断接入 prompt**

运行：

```powershell
git diff HEAD~1..HEAD -- src/app/agent/prompt_builder.rs src/domain/agent/mod.rs src/app/agent/agent_context.rs
```

预期：
- `src/app/agent/prompt_builder.rs` 没有 diff。
- `src/domain/agent/mod.rs` 没有 diff。
- `src/app/agent/agent_context.rs` 只包含 `debug!` 日志和辅助测试。

- [ ] **步骤 2：运行真实 embedding context router 测试**

运行：

```powershell
cargo test context_router --lib -- --ignored --nocapture
```

预期：通过。该测试依赖本地 `config.toml`、HP tunnel 和可访问的 embedding 服务。

- [ ] **步骤 3：运行默认回归测试**

运行：

```powershell
cargo test --quiet
```

预期：通过。

- [ ] **步骤 4：检查格式**

运行：

```powershell
cargo fmt --check
```

预期：通过。如果失败，运行 `cargo fmt`，然后重复步骤 2 和步骤 3。

- [ ] **步骤 5：确认没有暂存 ignored 文件**

运行：

```powershell
git status --short --ignored -- config.toml .env docs/superpowers docs/design
git diff --cached --name-only
```

预期：
- ignored 本地文件可以显示为 `!!`。
- `git diff --cached --name-only` 不能包含 ignored 文件。

- [ ] **步骤 6：如果格式化产生改动，再提交格式化**

如果任务 1 提交后 `cargo fmt` 又修改了文件，运行：

```powershell
git add -- src/app/agent/agent_context.rs
git commit -m "style: 格式化 Agent 上下文路由日志"
```

如果没有格式化改动，不要创建空提交。
