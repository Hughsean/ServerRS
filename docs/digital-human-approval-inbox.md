# Digital Human Agent 审批收件箱（Approval Inbox）

> 状态：已接入数字人 HTTP Chat 主链路（2026-07-22）。
> 前置文档：[digital-human-suspend-resume.md](digital-human-suspend-resume.md)。
> QQBot / NapCat 不在本功能范围内。

## 解决什么问题

受控工具触发审批门后，Chat 返回 `202 Accepted` 和 `checkpoint_id`。如果客户端
刷新页面、重启或丢失了这个响应，用户可以重新发现属于自己的待审批任务，
再安全地批准或拒绝，而不需要服务端或管理员介入。

本功能只新增**非消费式查询**与**审批决策审计**：

- `GET /api/v1/chat/checkpoints/pending` — 当前用户的待审批列表。
- `GET /api/v1/chat/checkpoints/{checkpoint_id}` — 当前用户的待审批详情。
- Resume 成功后以最佳努力写入 `tool_approval_decision` 审计事件。

恢复协议 `POST /api/v1/chat/checkpoints/{checkpoint_id}/resume`、正常聊天
`200` JSON 与暂停 `202` JSON 均保持不变。

## 查询待审批任务

```http
GET /api/v1/chat/checkpoints/pending
GET /api/v1/chat/checkpoints/pending?conversation_id=9&limit=20
Authorization: Bearer <token>
```

响应（每项与详情接口结构一致）：

```json
{
  "items": [
    {
      "status": "pending",
      "checkpoint_id": "2bb282b3-f4ad-41a6-bf1b-bf5c51fdc760",
      "run_id": "90b4891f-cf68-4c1a-ad83-32d9d8494d18",
      "conversation_id": 9,
      "reason": "approval",
      "created_at": "2026-07-22T01:00:00+00:00",
      "expires_at": "2026-07-23T01:00:00+00:00",
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
  ]
}
```

## 权限边界

- 两个端点都必须 Bearer 鉴权，且只查询 `user_id = 当前用户` 的记录。
- 可选 `conversation_id` 过滤仍同时受当前用户约束；其他用户的 Checkpoint
  绝不会出现。
- `limit` 默认 20，范围 `1..=100`；越界返回 `400`。
- 列表按 `created_at DESC, checkpoint_id DESC` 稳定排序。
- 只返回 `status = pending` 且未过期的记录。
- 详情接口对其他用户、已过期、已消费或不存在的 Checkpoint 统一返回
  `404`，避免 ID 枚举；无效的 checkpoint_id 同样返回 `404`。
- 工具参数属于敏感数据，只通过当前用户受保护接口返回。

## 查询不会消费 Checkpoint

列表和详情都是只读查询：

- 不会把 Checkpoint 标记为 `consumed`；
- 不会触发工具执行；
- 不会修改运行状态；
- 真正的消费仍然只能由 `POST .../resume` 的原子 `take` 完成。

因此客户端可以放心轮询或反复查询。列表后发生并发消费是正常竞态：后续
`resume` 可能返回 `404/409`，客户端重新查询列表即可。

## 过期与并发消费行为

- `expires_at` 到期后 Checkpoint 不再出现在列表/详情，也无法恢复；
  恢复请求会得到 `404`。
- 两个恢复请求竞争同一 Checkpoint 时，`take` 的条件更新保证只有一个
  成功（跨进程同样成立），败者得到 `404/409`，工具绝不会被执行两次。
- 新 Checkpoint 保存时会尽力清理已过期记录（既有行为）。

## 不会通过 API 暴露的敏感字段

列表/详情只包含用户做决定所需的最小信息。绝不返回：

- 完整 Checkpoint `payload`（运行快照 JSON）；
- 对话消息历史、记忆、画像、Prompt；
- Effect Receipt、已访问节点或内部 Trace；
- 其他用户的任何 Checkpoint 数据。

如果 Checkpoint 数据损坏或元数据与 payload 不一致，查询**失败关闭**：
返回错误并记录安全日志，绝不静默返回未经校验的数据。

## 刷新或客户端重启后的恢复流程

```text
1. GET /api/v1/chat/checkpoints/pending        # 重新发现待审批任务
2. GET /api/v1/chat/checkpoints/{id}           # （可选）确认详情仍在有效期
3. POST /api/v1/chat/checkpoints/{id}/resume   # approval_id 来自第 1/2 步
```

`approval_id` 始终来自服务端返回的审批信息，用户不需要手工输入。
Resume 保留原始 `RunId`；再次遇到受控工具时返回新的 `202` 与新
Checkpoint，客户端重复同一流程。

## 审批决策审计

Resume 成功（决策已被接受、Checkpoint 已被消费）后，服务端以最佳努力向
`agent_events` 写入一条 `event_type = tool_approval_decision` 的事件，只含：

- `user_id`、`conversation_id`
- `checkpoint_id`、`run_id`、`approval_id`
- `decision`（`approve` / `reject`）

不记录完整 Checkpoint payload、消息历史、工具参数或认证信息。审计写入
失败只记录 warn 日志：已经成功完成的 Resume 不会被客户端误认为失败，
也不会触发工具重放。Resume 失败（错误用户、错误审批 ID、过期或并发消费）
不会产生审计记录。

## CLI 使用方式

内置 CLI（`cargo run -p digital-human-server --bin cli`）支持完整审批闭环：

```text
/approvals [limit]          查询当前待审批任务（默认 20）
/approve [checkpoint_id]    批准并恢复（展示工具摘要后需 y/N 确认）
/reject [checkpoint_id]     拒绝并恢复（同样需确认）
```

- 收到 `202 suspended` 时，CLI 展示 checkpoint ID、过期时间、审批提示、
  工具名与格式化参数，并提示使用 `/approve` 或 `/reject`；绝不自动批准。
- `/approve`、`/reject` 不带参数时只使用当前 Session 最近一次明确保存的
  待审批项；没有保存项时要求显式提供 checkpoint ID。
- 带 ID 时 CLI 先通过详情接口取得合法的 `approval_id`，用户无需手工输入。
- Resume 完成后清理对应待审批状态；再次暂停时替换为新 Checkpoint。
- `404/409` 会提示"已过期、已消费或被其他恢复请求处理"，并允许用户重新
  执行 `/approvals`。

TypeScript SDK 对应 `chat.listPendingApprovals()` 与
`chat.getCheckpoint()`，示例见 [web/sdk/README.md](../web/sdk/README.md)。

## 数据库影响

零迁移。列表与详情复用 `agent_checkpoints` 表及其
`(user_id, status, expires_at)` 索引；审计复用 `agent_events` 表；过期
判断复用 `expires_at`。拒绝仍是正常 Resume，Checkpoint 最终进入
`consumed`。应用不会在启动时自动执行任何迁移。

## 不在承诺范围

- `UnknownCommit` 仍不会自动重试或自动恢复。
- 不提供跨外部系统的 exactly-once 保证。
- 不是任意指令点崩溃恢复；快照只在 `NodeResult::Suspend(...)` 的安全边界
  产生。
