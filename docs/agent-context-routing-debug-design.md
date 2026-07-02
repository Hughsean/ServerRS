# Agent 上下文路由 Debug 可观测设计

日期：2026-06-30

## 背景

`context_router` 分支已经验证了基于 embedding 的语义路由能力。当前主对话链路中，`AgentRuntime.respond()` 会调用 `AgentContextBuilder.build()` 构建本轮 `AgentContext`，并由 `PromptBuilder` 将上下文写入系统提示词。

现有代码已经具备核心接线：

- `bootstrap::state::ServiceGraph::build()` 在 `context_routing.enabled=true` 时初始化 `ContextRoutingService`。
- `AgentContextBuilder` 可接收 `ContextRoutingService`。
- `AgentContextBuilder.build()` 中的路由结果会影响 `Memory`、`RAG`、`Fresh Context` 的召回预算。

本次工作要把这条链路收成可用的生产形态，并用 `debug` 日志提供可观测性。

## 目标

1. 路由只作为内部调度策略，不写入 prompt，不影响模型对用户的直接表达。
2. 在 `AgentContextBuilder` 构建上下文时，用 `debug!` 记录本轮路由决策。
3. 在实际召回完成后，用 `debug!` 记录各类上下文的召回数量。
4. 保持失败容错：路由或检索问题不应阻断 agent 回复。
5. 保持配置开关行为：`context_routing.enabled=false` 时沿用原有默认召回预算。

## 非目标

- 不新增数据库表。
- 不写入 `agent_events`。
- 不把路由诊断放进 `AgentContext` 或系统提示词。
- 不改变工具调用流程。
- 不在本次工作中调整分类 taxonomy、prototype 或阈值策略。

## 设计

### 集成点

集成点保持在 `src/app/agent/agent_context.rs`。

`AgentContextBuilder.build()` 是单轮对话上下文构建的汇合点，天然能同时看到：

- 用户与对话信息。
- 最近消息。
- 默认 memory/RAG 预算。
- 路由决策。
- 最终召回结果。

因此 debug 日志也放在这里，不向 `AgentRuntime` 或 `PromptBuilder` 扩散。

### 路由决策日志

当存在 `ContextRoutingService` 时，在 `route(...)` 返回后记录一条 `debug!` 日志。

日志字段：

- `user_id`
- `conversation_id`
- `routing_enabled = true`
- `taxonomy`
- `fresh_enabled`
- `fresh_confidence`
- `memory_top_k`
- `memory_reason`
- `memory_confidence`
- `rag_top_k`
- `rag_reason`
- `rag_confidence`
- `fallback_used`
- `top_labels`

当未配置路由服务时，记录较轻的一条 `debug!`：

- `routing_enabled = false`
- `memory_top_k = max_memory_items`
- `rag_top_k = max_rag_chunks`

这能帮助排查“为什么没有看到路由行为”。

### 召回结果日志

在 `memories`、`rag_chunks`、`fresh_chunks` 都完成后，记录一条 `debug!` 日志。

日志字段：

- `user_id`
- `conversation_id`
- `memory_top_k`
- `rag_top_k`
- `fresh_enabled`
- `memories_count`
- `rag_chunks_count`
- `fresh_chunks_count`

这条日志回答“路由之后实际拼进上下文的东西有多少”。

### 错误处理

路由层沿用 `ContextRoutingService::route()` 的 fallback 行为。如果发生低置信或内部 fallback，通过 `fallback_used=true` 暴露给 debug 日志。

检索层沿用现状：

- Memory/RAG 召回失败使用空结果。
- Fresh Context 召回失败记录 `warn!` 并使用空结果。

本次不引入新的错误传播路径。

## 测试策略

1. 保留 `src/test/context_router.rs` 中的真实 embedding ignored 测试，用于手动验证 HP tunnel + 真实分类 + 路由结果。
2. 保留或补充轻量单元测试，验证路由决策仍能覆盖 memory/RAG budget。
3. 不强测 debug 日志字符串，避免日志格式变成脆弱公共契约。
4. 回归命令：
   - `cargo test context_router --lib -- --ignored --nocapture`
   - `cargo test --quiet`

## 验收标准

- 开启 `context_routing.enabled=true` 时，agent 上下文构建会使用路由结果控制 Memory/RAG/Fresh Context。
- debug 日志能看出本轮路由判断和最终召回数量。
- 关闭 `context_routing.enabled=false` 时，仍使用原有默认预算。
- 路由诊断不进入 prompt。
- 默认测试套件通过。
