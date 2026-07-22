# Digital Human Agent Checkpoint + Suspend/Resume

> 状态：已接入数字人 HTTP Chat 主链路（2026-07-22）。QQBot 不在本功能范围内。
> 待审批收件箱（列表/详情/决策审计/CLI）见
> [digital-human-approval-inbox.md](digital-human-approval-inbox.md)。

## 业务场景

模型可以生成工具调用，但部分工具不应立即执行。配置为受控工具后，Chat Agent 会在
“模型已给出调用参数、工具尚未产生外部副作用”的边界暂停，把完整运行快照写入 MySQL，
等待当前登录用户批准或拒绝。

真实暂停节点是 `ApprovalGateNode`，位置如下：

```text
reasoning.llm
  -> reasoning.approval_gate
       -> 普通工具：继续
       -> 受控工具：NodeResult::Suspend(...)
  -> reasoning.tools
```

暂停成功时，`reasoning.tools` 尚未执行，对话消息也尚未持久化。批准后从
`reasoning.tools` 继续；拒绝后生成“用户拒绝工具”的失败观察，再由 LLM 完成本轮回复。

## 配置

```toml
[agent]
# 空列表保持旧行为：工具自动执行。
approval_required_tools = ["fetch_web_content"]
# Checkpoint 有效期，范围 1..=2592000 秒。
checkpoint_ttl_secs = 86400
```

也可以通过环境变量覆盖：

```text
AGENT_APPROVAL_REQUIRED_TOOLS=fetch_web_content,web_search
AGENT_CHECKPOINT_TTL_SECS=86400
```

配置的名称必须与已注册工具名完全一致，不能重复或带首尾空格。

## HTTP 协议

`POST /api/v1/chat/messages` 正常完成时仍返回原有 `200` JSON，不增加字段：

```json
{
  "conversation_id": 9,
  "reply": "完成后的回复",
  "tool_calls": []
}
```

命中审批门时返回 `202 Accepted`：

```json
{
  "status": "suspended",
  "conversation_id": 9,
  "checkpoint_id": "2bb282b3-f4ad-41a6-bf1b-bf5c51fdc760",
  "run_id": "90b4891f-cf68-4c1a-ad83-32d9d8494d18",
  "reason": "approval",
  "approval": {
    "approval_id": "02f941ab-0fb8-4c44-999c-9ff896ef415a",
    "prompt": "模型请求执行受控工具，请确认是否允许。",
    "tool_calls": [
      {
        "id": "call-1",
        "name": "fetch_web_content",
        "arguments": { "url": "https://example.com" }
      }
    ]
  }
}
```

恢复接口同样需要 Bearer Token：

```http
POST /api/v1/chat/checkpoints/{checkpoint_id}/resume
Content-Type: application/json

{
  "approval_id": "02f941ab-0fb8-4c44-999c-9ff896ef415a",
  "decision": "approve"
}
```

`decision` 只能是 `approve` 或 `reject`。恢复完成返回原有 `200` Chat JSON；如果后续又
遇到受控工具，则返回新的 `202` 和新 Checkpoint。错误用户、错误审批 ID、过期或已消费
Checkpoint 都不会再次执行工具。

TypeScript SDK 的 `chat.sendMessage` 返回 `ChatTurnResponse` 联合类型，并提供
`chat.resumeCheckpoint(checkpointId, request)`。

## 待审批收件箱

客户端丢失 `202` 响应后，可以重新发现自己的待审批任务：

```http
GET /api/v1/chat/checkpoints/pending?conversation_id=9&limit=20
GET /api/v1/chat/checkpoints/{checkpoint_id}
```

两个端点都必须 Bearer 鉴权且只返回当前用户 pending 且未过期的记录，均为
非消费式查询：不消费 Checkpoint、不执行工具、不修改运行状态。详情接口对
其他用户、已过期、已消费或不存在的 Checkpoint 统一返回 `404`。列表只暴露
最小审批信息（审批 ID、提示、工具调用、过期时间），绝不返回完整 Checkpoint
payload、消息历史、记忆、画像或内部 Trace。

Resume 成功后会以最佳努力向 `agent_events` 写入
`event_type = tool_approval_decision` 的审计事件（user_id、conversation_id、
checkpoint_id、run_id、approval_id、decision），审计失败不影响已完成的
Resume 结果，也不触发工具重放。

TypeScript SDK 提供 `chat.listPendingApprovals()` 与 `chat.getCheckpoint()`；
内置 CLI 提供 `/approvals`、`/approve`、`/reject` 命令。完整的权限边界、
并发语义与 CLI 用法见
[digital-human-approval-inbox.md](digital-human-approval-inbox.md)。

## 数据库与部署

新安装的完整结构位于 `database/sql/init.sql`。已有数据库必须先应用：

```text
database/sql/migrations/20260722_agent_checkpoints.sql
```

应用不会在启动时自动建表。本次实现也没有连接或修改任何在线数据库实例。

`agent_checkpoints` 保存 `AgentCheckpoint<ChatTurnState>` 的 JSON 快照，同时保存
`user_id`、`conversation_id`、图版本、状态版本、下一节点、状态和过期时间等可校验元数据。
快照包含对话上下文，属于敏感业务数据，应沿用主数据库的访问控制、备份与静态加密策略。
新 Checkpoint 保存时会尽力清理已过期记录。

待审批收件箱**不新增表、不修改现有表**：列表/详情复用 `agent_checkpoints` 及其
`(user_id, status, expires_at)` 索引，审批决策审计复用 `agent_events`，过期判断复用
`expires_at`，因此无需额外迁移。

## 跨进程与并发语义

- 不依赖进程内存：另一服务进程连接同一 MySQL、运行相同图版本即可恢复。
- 保留暂停前的原始 `RunId`、预算、用量、访问节点和 Effect Receipt。
- 恢复前校验 `GraphId`、Graph Version、State Schema Version、下一节点和归属用户。
- `take` 在事务中执行 `status='pending' AND expires_at>now` 的条件更新；并发竞争只有一个
  进程能把状态改为 `consumed`，败者回滚。
- 用户归属与审批 ID 在消费前校验，非法恢复不会消耗合法 Checkpoint。

## 明确限制

这不是“任意指令点崩溃恢复”。快照只在节点主动返回 `NodeResult::Suspend(...)` 的安全边界
产生。恢复操作会先原子消费 Checkpoint，再执行后续节点；如果恢复进程随后在外部写入附近
崩溃，系统不会自动重放同一 Checkpoint，也不把 `UnknownCommit` 宣称为可安全恢复。该策略
优先避免重复工具副作用，不提供跨外部系统的 exactly-once 保证。
