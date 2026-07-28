# 个人 QQ 智能秘书开发历史索引

> 本文件只做导航和当前阶段摘要；具体事件进入 `history/` 归档。
> 新事件必须精确到 `YYYY-MM-DD HH:mm（Asia/Shanghai）`，缺少可信分钟的旧事件不得猜测回填。

## 当前阶段

- 主干分支：`main`（`93710d3`）；Release Hardening 与 Owner Agenda/Reminder v1 已合并。QQBot 运行数据库使用独立容器、独立数据库和
  独立持久化卷，不复用数字人数据库。
- 最近完成分支：`gpt/qqbot-owner-agenda-v1`，功能提交 `79a04f7`，通过非快进合并提交
  `93710d3` 进入 `main`；未推送远端。
- 当前能力：可靠入站、空窗回补、确定性 EventThread、类型化语义、跨会话关联候选、Owner
  关联审核、高影响线程变更的持久化 Suspend/Resume、授权撤销、语义失效，以及来源化人物/
  项目/承诺结构记忆、证据回读、Owner 派生记忆删除、承诺提醒 Outbox、独立 QQ 开放平台
  协议适配、类型化 Agent 动作策略门、可选 OpenAI-compatible/Ollama 有界线程语义提取、
  并发优雅关闭（RuntimeWorkers + 25s 全局 deadline）、可编程运行时入口（run_with_cancellation）、
  群白名单过滤、Owner Retriever、受约束 Action Planner、真实 Effect Receipt、响应产物，
  以及 Action Run 的持久化 Suspend/Resume CAS 闭环。
  **新增（本轮）**：协议无关 AgendaItem/Mutation、创建/查询/改期/稍后提醒/完成/取消 Action、
  L2 Owner 审批、不可变审计、版本 fencing、到期 Scheduler 和复用的 Owner-only Outbox。
- 当前边界：NapCat 保持只读；B4/B3/B6/B7 生产链的独立验收矩阵 15/15 实际通过；Agenda
  在 MySQL 8.4.10 从空 schema 完成真实迁移与业务闭环，Action Planner MySQL 5/5 通过。
  合并门禁仍为 `REJECTED`，原因是 GitHub protected Environment、固定可信公钥/签发密钥托管
  和 required check 尚未配置，当前运行没有受保护的仓库外 L4/L5 attestation。
- 下一开发项：在明确通知用户并重新确认本地凭据与测试范围后，验收真实 Owner 指令→审批/拒绝
  →到期通知链路。GitHub 管理侧仍需配置 protected Environment、
  固定可信公钥与 `QQBot Acceptance Gate / acceptance` required check。

## 历史分块

| 时间范围 | 主题 | 记录 |
|---|---|---|
| 2026-07-23～2026-07-24 | 个人秘书立项、NapCat 验证、可靠入站、Gap 回补、线程与 Owner 审核 | [2026-07 归档](history/2026-07.md) |

## 最近事件

- `2026-07-28 11:48（Asia/Shanghai）`：将 Release Hardening 检查点 `d66f4eb` 与 Owner Agenda
  功能提交 `79a04f7` 通过非快进提交 `93710d3` 安全合并至 `main`；未推送远端。

- `2026-07-28 11:22（Asia/Shanghai）`：Owner Agenda/Reminder v1 代码与真实 MySQL 收口；Codex
  修复 `UNIX_TIMESTAMP` DECIMAL 解码和新迁移重复执行冲突。Agenda MySQL 1/1、Action Planner
  MySQL 5/5、常规三 crate 测试、严格 Clippy、workspace boundaries 19/19、既有验收矩阵
  15/15 实际通过；所有临时 schema 已删除，未连接或发送 QQ 消息。

- `2026-07-27 21:32（Asia/Shanghai）`：Release Hardening v1.1 代码侧收尾；提取并验证
  .NET RSA-PSS attestation（合法签名/签名篡改/claim 篡改），Recall Spool 指标接入 B7；
  fmt、严格 clippy、相关 crate 测试、workspace boundaries 与隔离 MySQL 14 个检查实际通过。
  GitHub protected Environment、固定可信公钥/签发密钥托管和 required check 尚未配置，故无
  合规 L4/L5 attestation，门禁按规则保持 `REJECTED`。

- `2026-07-26 21:55（Asia/Shanghai）`：建立 QQBot 独立机器验收基础设施；新增 JSON
  验收矩阵、L1-L6 证据等级、隔离 MySQL 执行器、精确测试发现与自动合并门禁；首批 5 个
  黑盒测试真实复现撤回 ID 约束、SourceEvent 缺失、pending 未自动关联、Gap 未冻结及
  Artifact 未随撤回失效，当前结果 5 FAIL + 6 MISSING，门禁 `REJECTED`。
- `2026-07-26 16:20（Asia/Shanghai）`：P0/P1 修复（评审反馈 6.5/10 未批准）；修复三态 Heartbeat 状态机（Expired 立即返回）、结构化段成为语义事实来源（normalized_text/at_bot 从 segments 生成）、RecentContactData DTO 修正为真实字段 chatType/peerUin/peerName、能力探测接入运行时真实调用接口、HeartbeatConfig validate 真正限制异常值、结构化段总字节数上限、历史 Unknown 有界、Action Planner 装配失败保证回收；B1/B2/B5 降为 PARTIAL；fmt/clippy/test/workspace_boundaries 全绿。
- `2026-07-26 14:35（Asia/Shanghai）`：NapCat Adapter Hardening v1 + QQBot 分层合理化（阶段检查点，分支 `glm/qqbot-napcat-hardening-v1`，未提交待 Codex 评审）；阶段 A 行为保持拆分（config/runtime+bootstrap/agent_runtime+action_graph/mysql_action_store），阶段 B 完成 B1 Heartbeat/Lifecycle、B2 结构化消息段优先+CQ 回退、B5 能力版本探测；fmt/clippy/test/workspace_boundaries 全绿，MySQL 集成与实机 NapCat 验收列为未验证。
- `2026-07-26 00:50（Asia/Shanghai）`：Owner Retriever/Action Planner 第五轮修复与全板块完整性验收；修复稳定 run ID、Suspend/Resume 租约、响应事务、Effect 幂等、信任矩阵和迁移约束。
- `2026-07-25 18:17（Asia/Shanghai）`：E2E 最终验收通过（36.54s）；清理非白名单群 338 条历史数据；下一阶段 Owner Retriever / Action Planner。
- `2026-07-25 17:05（Asia/Shanghai）`：群白名单、E2E RAII 守卫加固、跨扫描周期重启稳定性与文档修正。
- `2026-07-25 13:27（Asia/Shanghai）`：真实消息入站闭环 E2E 验收通过（187.60s，含 LLM 退避重试）。
- `2026-07-25 12:47（Asia/Shanghai）`：并发优雅关闭、可编程运行时入口与真实 E2E 验收骨架。
- `2026-07-25 10:44（Asia/Shanghai）`：QQBot 环境变量覆盖改为四类窄宏，消除配置解析样板代码。
- `2026-07-25 10:35（Asia/Shanghai）`：移除 NapCat Token 配置，HTTP 13990/WebSocket 13991 无 Token 组合验收通过。
- `2026-07-24 22:42（Asia/Shanghai）`：Ollama Qwen3 实机语义与提示注入边界验收通过。
- 旧事件仅有日期证据，保留原日期，不伪造分钟。

## 分块规则

- 每个归档按月份建立；达到 500 行或 100 KiB 时，再按日期和业务切片拆分。
- 归档内容按时间顺序追加，完成事项必须同步更新 `TODO.md`。
- 每条记录必须包含完成范围、数据库影响、外部系统影响、验证、Git 状态和下一项。
- 根索引只保留当前阶段和最近 10 条事件，避免无限增长。
