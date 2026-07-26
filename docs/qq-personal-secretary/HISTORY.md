# 个人 QQ 智能秘书开发历史索引

> 本文件只做导航和当前阶段摘要；具体事件进入 `history/` 归档。
> 新事件必须精确到 `YYYY-MM-DD HH:mm（Asia/Shanghai）`，缺少可信分钟的旧事件不得猜测回填。

## 当前阶段

- 主干分支：`Main`（`bfa9005`），Owner Retriever/Action Planner 已全部提交；QQBot 运行数据库使用独立容器、独立数据库和
  独立持久化卷，不复用数字人数据库。
- 进行中分支：`glm/qqbot-napcat-hardening-v1`（基于 `Main`，未提交、未推送，等待 Codex 第三轮评审），
  承载 NapCat Adapter Hardening v1 + 分层合理化：阶段 A 行为保持拆分（config/runtime+bootstrap/
  agent_runtime+action_graph/mysql_action_store）+ 阶段 B **PARTIAL**（B1 Heartbeat/Lifecycle、
  B2 结构化消息段优先+CQ 回退、B5 能力版本探测，已修复第三轮评审 P0/P1 并新增 mock 集成测试，
  仍待实机与 MySQL 验收）；
  详见 `napcat-adapter-architecture.md`。
- 当前能力：可靠入站、空窗回补、确定性 EventThread、类型化语义、跨会话关联候选、Owner
  关联审核、高影响线程变更的持久化 Suspend/Resume、授权撤销、语义失效，以及来源化人物/
  项目/承诺结构记忆、证据回读、Owner 派生记忆删除、承诺提醒 Outbox、独立 QQ 开放平台
  协议适配、类型化 Agent 动作策略门、可选 OpenAI-compatible/Ollama 有界线程语义提取、
  并发优雅关闭（RuntimeWorkers + 25s 全局 deadline）、可编程运行时入口（run_with_cancellation）、
  群白名单过滤、Owner Retriever、受约束 Action Planner、真实 Effect Receipt、响应产物，
  以及 Action Run 的持久化 Suspend/Resume CAS 闭环。
- 当前边界：NapCat 保持只读；本地 QQBot MySQL 8.4 运行库已建成并通过 TLS、迁移幂等和应用
  连接验收。QQ 开放平台本地凭据可以完成鉴权和 Gateway 地址读取，但官方通道仍关闭，Owner
  OpenID 尚未通过真实 C2C 入站事件确认。NapCat HTTP `13990`、WebSocket `13991` 和 Ollama
  `11434` 曾完成实机验收，但在 2026-07-26 00:50 当前复验中均未监听，不能视为当前在线；
  MySQL 容器保持健康。已在聊天中暴露的 QQ 开放平台 Secret 仍应轮换后再上线。
- 下一开发项：恢复 NapCat/Ollama 后重跑真实入站 E2E；随后接通 Owner 审批入口、响应投递和
  日程/提醒执行器。QQ 开放平台通道保持关闭，启用前必须再次通知并确认本地凭据。

## 历史分块

| 时间范围 | 主题 | 记录 |
|---|---|---|
| 2026-07-23～2026-07-24 | 个人秘书立项、NapCat 验证、可靠入站、Gap 回补、线程与 Owner 审核 | [2026-07 归档](history/2026-07.md) |

## 最近事件

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
