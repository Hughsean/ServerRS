# Agent Context Routing Debug Observability Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add debug-level observability to the agent context routing path without exposing routing diagnostics to the prompt.

**Architecture:** Keep routing as an internal `AgentContextBuilder` concern. Add two focused helper functions in `src/app/agent/agent_context.rs`: one logs the route decision or disabled state, and one logs final retrieval counts. The existing routing service, bootstrap wiring, prompt shape, and agent event persistence remain unchanged.

**Tech Stack:** Rust, Tokio, `tracing::debug!`, existing `ContextRoutingService`, existing `AgentContextBuilder` tests.

## Global Constraints

- Do not write route diagnostics into `AgentContext` or the system prompt.
- Do not write route diagnostics into `agent_events`.
- Do not add database schema or persistent state.
- Do not change taxonomy, prototype, threshold, or tool-call behavior.
- Use `debug!` for the new observability logs.
- Keep current fallback behavior: routing or retrieval issues must not block agent replies.
- Do not commit ignored files such as `config.toml`, `.env`, `docs/superpowers`, or `docs/design`.
- Commit messages should be Chinese; technical English is acceptable where necessary.

---

## File Structure

- Modify: `src/app/agent/agent_context.rs`
  - Responsibility: Build per-turn `AgentContext`, apply context routing budgets, and emit debug logs for route decisions and retrieval counts.
- Keep: `src/app/context_routing/mod.rs`
  - Responsibility: Produce `ContextRouteDecision`. No change needed.
- Keep: `src/app/agent/prompt_builder.rs`
  - Responsibility: Build system prompt from `AgentContext`. No route diagnostics should be added.
- Keep: `src/test/context_router.rs`
  - Responsibility: Manual ignored real embedding test. No change needed for debug logging.

---

### Task 1: Add Debug Logs To Agent Context Builder

**Files:**
- Modify: `src/app/agent/agent_context.rs`

**Interfaces:**
- Consumes: `ContextRouteDecision`, `AgentContextBuilder::build(...)`, `routed_memory_top_k(...)`, `routed_rag_top_k(...)`
- Produces:
  - `fn log_context_route_decision(user_id: u64, conversation_id: Option<u64>, decision: Option<&ContextRouteDecision>, default_memory_top_k: u32, default_rag_top_k: u64)`
  - `fn log_context_retrieval_counts(user_id: u64, conversation_id: Option<u64>, decision: Option<&ContextRouteDecision>, memory_top_k: u32, rag_top_k: u64, memories_count: usize, rag_chunks_count: usize, fresh_chunks_count: usize)`

- [ ] **Step 1: Write the failing test**

Append this test to the existing `#[cfg(test)] mod tests` in `src/app/agent/agent_context.rs`:

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

- [ ] **Step 2: Run test to verify it fails**

Run:

```powershell
cargo test route_decision_debug_helpers_accept_disabled_and_enabled_paths --lib
```

Expected: FAIL with unresolved functions `log_context_route_decision` and `log_context_retrieval_counts`.

- [ ] **Step 3: Import `debug`**

Change the tracing import near the top of `src/app/agent/agent_context.rs` from:

```rust
use tracing::warn;
```

to:

```rust
use tracing::{debug, warn};
```

- [ ] **Step 4: Log route decision after routing**

In `AgentContextBuilder::build(...)`, after the `routing_decision` block and before `let memory_top_k = ...`, insert:

```rust
        log_context_route_decision(
            user_id,
            conversation_id,
            routing_decision.as_ref(),
            max_memory_items,
            max_rag_chunks,
        );
```

- [ ] **Step 5: Log final retrieval counts**

In `AgentContextBuilder::build(...)`, after `fresh_chunks` is computed and before `let profile: Option<Value> = ...`, insert:

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

- [ ] **Step 6: Add helper functions**

Add these functions below `routed_rag_top_k(...)` and above `#[cfg(test)] mod tests`:

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

- [ ] **Step 7: Run focused test to verify it passes**

Run:

```powershell
cargo test route_decision_debug_helpers_accept_disabled_and_enabled_paths --lib
```

Expected: PASS.

- [ ] **Step 8: Run related existing tests**

Run:

```powershell
cargo test agent_context --lib
```

Expected: PASS for agent context tests.

- [ ] **Step 9: Commit**

Run:

```powershell
git add -- src/app/agent/agent_context.rs
git commit -m "feat: 增加 Agent 上下文路由 debug 日志"
```

---

### Task 2: Verify Full Regression And Real Routing Test

**Files:**
- Verify only: `src/app/agent/agent_context.rs`
- Verify only: `src/test/context_router.rs`

**Interfaces:**
- Consumes: debug helper functions from Task 1
- Produces: verified working branch with no prompt or event persistence changes

- [ ] **Step 1: Verify no prompt integration was added**

Run:

```powershell
git diff HEAD~1..HEAD -- src/app/agent/prompt_builder.rs src/domain/agent/mod.rs src/app/agent/agent_context.rs
```

Expected:
- No diff for `src/app/agent/prompt_builder.rs`
- No diff for `src/domain/agent/mod.rs`
- `src/app/agent/agent_context.rs` only contains `debug!` logging and the helper test

- [ ] **Step 2: Run real embedding context router test**

Run:

```powershell
cargo test context_router --lib -- --ignored --nocapture
```

Expected: PASS. This test requires local `config.toml`, HP tunnel, and reachable embedding service.

- [ ] **Step 3: Run default regression suite**

Run:

```powershell
cargo test --quiet
```

Expected: PASS.

- [ ] **Step 4: Check formatting**

Run:

```powershell
cargo fmt --check
```

Expected: PASS. If it fails, run `cargo fmt`, then repeat Step 2 and Step 3.

- [ ] **Step 5: Confirm ignored files are not staged**

Run:

```powershell
git status --short --ignored -- config.toml .env docs/superpowers docs/design
git diff --cached --name-only
```

Expected:
- Ignored local files may appear with `!!`
- `git diff --cached --name-only` must not include ignored files

- [ ] **Step 6: Final commit if formatting changed**

If `cargo fmt` changed files after Task 1 commit, run:

```powershell
git add -- src/app/agent/agent_context.rs
git commit -m "style: 格式化 Agent 上下文路由日志"
```

If no formatting changed, do not create an empty commit.
