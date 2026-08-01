# 个人 QQ 智能秘书开发历史索引

> 本文件只做导航和当前阶段摘要；具体事件进入 `history/` 归档。
> 新事件必须精确到 `YYYY-MM-DD HH:mm（Asia/Shanghai）`，缺少可信分钟的旧事件不得猜测回填。

## 当前阶段

- 主干分支：`Main`（`ea2226a`）；Owner 通知策略响应工件已合并。QQBot 运行数据库使用独立容器、独立数据库和
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
- 当前边界：NapCat 保持只读；Task 7 的五项 L3 验收以及
  `B3-RECALL-004-RESILIENCE` 的 L3 resilience 检查均实际通过。全局门禁仍为 `REJECTED`：
  多项 B3/B4/B6/B7 缺少 L4/L5 独立证明，B3 requirement 因缺少 L5 attestation 显示 FAIL；
  不把 `REJECTED` 改写为 `APPROVED`。
- 当前开发分支：`deepseek/qqbot-owner-work-close-v1`。旧验收矩阵只保留历史用途，不再作为日常开发门禁；
  接下来继续主动跟进、线程语义与离线恢复任务。需要用户操作或 NapCat 实机验证的事项单独跳过。

## 历史分块

| 时间范围 | 主题 | 记录 |
|---|---|---|
| 2026-07-23～2026-07-24 | 个人秘书立项、NapCat 验证、可靠入站、Gap 回补、线程与 Owner 审核 | [2026-07 归档](history/2026-07.md) |
| 2026-08-01～ | 上线前 TODO 连续收口 | [2026-08 归档](history/2026-08.md) |

## 最近事件

- `2026-08-01 19:41（Asia/Shanghai）`：完成 Owner 待办关闭闭环。新增单条/批量完成 FollowUp
  与单条/批量关闭 ResponseExpectation 的 L2 Action；最终事务复验 Owner 授权、租约、账号、
  状态和来源版本，按确定顺序锁定全部目标与通知，原子更新业务状态、压制 pending/failed
  Outbox、写不可变逐目标审计及单一 Effect Receipt。关闭回复期待明确写 `dismissed`，不冒充
  已收到真实回复的 `resolved`，也不修改线程或开放问题。独立复核修正了完成/关闭动作被误标为
  可逆的产品语义，并增加回归断言。6 条随机隔离 MySQL 新旧闭环、218 个领域单测、105 个服务
  单测和 19 项应用边界检查全部通过；临时 schema 均已清理，未连接或发送 QQ。

- `2026-08-01 17:44（Asia/Shanghai）`：完成 Owner 批量推迟 FollowUp 的 L2 控制闭环。同一
  Action 最多携带 20 个明确 ID/版本并共用新到期时间；最终事务以 MySQL 时钟复验时间窗口，
  按确定顺序锁定全部目标和通知，任一状态、版本、时间或投递冲突即整批回滚。成功后逐项推进
  due/版本、压制旧通知、写多条不可变审计，并只写一条 Effect Receipt。新 due 到达后各目标以
  新版本重新进入统一策略链。独立复核的批量推迟、单条推迟和批量忽略三条隔离 MySQL 场景均
  通过，临时 schema 全部清理；未连接或发送 QQ。

- `2026-08-01 17:24（Asia/Shanghai）`：完成 Owner 批量忽略 FollowUp 的 L2 控制闭环。单次
  Action 最多携带 20 个明确 FollowUp ID 与来源版本；最终事务按确定顺序锁定全部目标及其
  legacy/policy-owned Outbox，任一目标状态、版本或投递状态不安全即整批回滚。成功时统一推进
  全部版本、压制 pending/failed 通知、为每个目标写不可变审计，并只写一条 Effect Receipt。
  独立复核补强了批量审计 ID 的无歧义派生和 Outbox 行锁顺序；批量成功/失败全回滚、单条忽略
  与单条推迟三条随机隔离 MySQL 场景均通过，临时 schema 全部清理。未连接或发送 QQ。

- `2026-08-01 14:42（Asia/Shanghai）`：完成 Owner 推迟单个 FollowUp 的 L2 控制闭环。新到期
  时间在最终事务中以 MySQL 当前时钟复验，必须晚于原到期且不超过 365 天；事务以账号、状态和
  来源版本 fencing 更新 due/版本，并同时锁定和压制 legacy 与 policy-owned 的 pending/failed
  Outbox。claimed/unknown_commit 保守拒绝，delivered 历史不改写。到达新时间后扫描会为新版本
  生成 Candidate/Request，经统一策略求值形成新的 Outbox occurrence，旧通知不会复活。独立复核
  发现原测试在 OwnerBinding 建立前求值，修正 fixture 顺序后随机隔离 MySQL 完整闭环通过，临时
  schema 已清理；严格 Clippy 与 19 项应用边界测试通过。未连接或发送 QQ。

- `2026-08-01 13:45（Asia/Shanghai）`：补齐 FollowUp 忽略对现行 Task 7 通知链的覆盖。旧实现
  只按 Outbox 的 legacy `follow_up_id` 查找，但 policy-owned Outbox 通过 Candidate 引用来源且
  `follow_up_id` 为空；现已在同一事务内锁定并压制两种来源形态。MySQL 测试改为真实
  FollowUp→Candidate→Evaluation→Decision→policy-owned Outbox，再执行 Owner 审批忽略并通过；
  临时 schema 已清理。

- `2026-08-01 13:38（Asia/Shanghai）`：完成 Owner 忽略单个 FollowUp 的 L2 控制闭环。
  Action 使用 `follow_up_id + expected_source_version` fencing；Resume 后的专用事务复验租约、
  OwnerCommand、唯一有效绑定和账号归属，原子更新 dismissed/版本、压制 pending/failed Outbox、
  写不可变审计与通用 Effect Receipt。claimed/unknown_commit 会保守拒绝，delivered 历史不改写。
  独立评审将 ID 上限收紧到数据库 `CHAR(36)`，增加 Proposal/Action 一致性验证，并锁定 Outbox
  行消除投递竞态；真实 MySQL 首轮修复 SQL 数字下划线、次轮修复 unsigned 解码后主路径通过，
  随机 schema 已清理。未连接或发送 QQ。

- `2026-08-01 13:16（Asia/Shanghai）`：待处理事项现携带真实可选来源版本：FollowUp 使用
  `source_version`、Agenda 使用 `version`、回复期待使用其来源版本，Outbox 明确为无版本，
  为后续忽略/推迟操作提供 fencing。Owner 响应仅在版本存在时展示。随机隔离 MySQL 覆盖
  FollowUp、Agenda、Outbox 与跨账号隔离；首轮发现测试 SQL 错把 Rust 字面量 `90_000` 放入
  MySQL，已修正为 `90000` 后通过，临时 schema 已清理。未连接或发送 QQ。

- `2026-07-31 22:50（Asia/Shanghai）`：完成 Owner 线程语义与生命周期控制切片。新增确认/撤销
  结论、忽略开放问题、关闭/重开线程四类 L2 Action；最终 MySQL 事务复验 Action 租约、
  OwnerCommand、唯一有效绑定、账号和目标状态，并原子提交业务状态、不可变控制审计和通用
  Effect Receipt。关闭要求不存在开放问题且使用期望状态 CAS；状态历史引用命令 SourceEvent。
  一条随机隔离 MySQL 测试依次执行四条 OwnerCommand 的 Suspend→模拟重启→Resume，并验证
  Checkpoint 只能消费一次、四份审计与回执一一对应；临时 schema 已清理。未连接或发送 QQ。

- `2026-07-31 22:32（Asia/Shanghai）`：完成 Owner 状态、待办与线程因果上下文只读切片。
  新增 `GetSecretaryStatus`、`ListPendingOwnerWork`、`GetThreadContext` 三类 L0 Action，复用
  Retriever/Action Graph/Receipt/OwnerResponseDraft；所有 MySQL 查询强制账号过滤并限制数量和
  展示长度。状态明确展示未闭合 Gap，待办聚合回复期待、跟进、Agenda 与异常 Outbox，线程上下文
  返回参与者、要求/意见、结论、开放问题及来源 ID。真实 MySQL 首轮发现 `COUNT(*)` 在 8.4 返回
  有符号 BIGINT，已改为非负校验后转换；两条隔离 MySQL 场景通过，随机 schema 已清理。未连接
  NapCat/QQ 开放平台，未发送消息，数据库无迁移。

- `2026-07-31 22:14（Asia/Shanghai）`：项目记忆中的非空 blockers 现会在默认持续 24 小时后
  生成 `project_blocked` 跟进，进入统一 Notification Policy，而不是扫描时直接写 Outbox。项目
  事实修订、删除或过期后沿既有来源版本 reconciliation 自动终止旧跟进。Owner 通知仅展示有界
  项目键和最多五个有界阻塞摘要。随机隔离 MySQL 验证项目事实→FollowUp→类型化 Candidate；
  严格 Clippy 通过，临时 schema 已清理，未连接或发送 QQ。

- `2026-07-31 22:09（Asia/Shanghai）`：完成“长期无人回复”主动跟进闭环。新增来源化
  `secretary_response_expectations`，从外部联系人提出的开放问题生成稳定 Candidate；默认四小时
  未见同线程本人回复时进入统一 Notification Policy，只有 Remind 决策才形成 Owner Outbox。
  本人后续回复、问题 answered/dismissed 或线程 resolved/closed 会推进来源版本、终止期待并抑制
  未发送通知。真实隔离 MySQL 覆盖：开放问题→Candidate/Request→Decision→Owner Outbox 内容
  重建→本人回复→resolved/suppressed；随机 schema 已清理。未连接或发送 QQ。

- `2026-07-31 21:52（Asia/Shanghai）`：完成 Owner 记忆控制 Action 垂直切片。新增列出记忆、
  来源回读、不可变修正、删除派生记忆、设置/取消 TTL、会话长期记忆模式六类白名单 Action；
  全部进入风险门、Planner DTO、Action Graph、MySQL Store 与运行时装配。L2 修订使用 Effect ID
  派生稳定 Fact ID，崩溃重放不会生成第二版本；会话模式更新在事务内复验 OwnerCommand、账号
  绑定和会话作用域。验证：fmt、严格 Clippy、personal-secretary 217 项、qqbot-server 98 项
  均通过；随机隔离 MySQL 中记忆来源/会话模式/删除/跟进闭环通过，schema 已清理。未连接 NapCat
  或 QQ 开放平台，未发送消息，数据库无迁移。

- `2026-07-30 10:19（Asia/Shanghai）`：验收脚本凭据 P1 已关闭。隔离 schema 的创建和清理改为
  仅在容器内部通过 `MYSQL_PWD="$MYSQL_ROOT_PASSWORD"` 认证，宿主机 `docker` 命令行只保留
  字面环境变量引用，不含 root 密码。PowerShell 5.1 解析、`B3-RECALL-004-RESILIENCE` 隔离
  MySQL smoke 和 `git diff --check` 均通过；smoke schema
  `qqbot_accept_20260730101342_6c0e95c5` 已清理。盘点确认有 4 个既有遗留 schema，均未在无授权下
  删除：`qqbot_accept_20260729142255_697e4cce`、`qqbot_accept_20260729214037_978a58d9`、
  `qqbot_accept_20260729214904_ba276172`、`qqbot_accept_20260730000216_ef44aea1`。未提交、未推送、
  未合并、未 stash，未连接或发送 QQ。

- `2026-07-29 23:29（Asia/Shanghai）`：完成 `B3-RECALL-004-RESILIENCE` 的 L3 修复与真实
  隔离 MySQL 验收。根因是验收注入器删除 `secretary_recall_inbox` 后仍保留
  `qqbot_test_schema_migrations` 记录，幂等加载器因此跳过已记录迁移，MySQL 从未恢复该表，并非
  Recall WAL Worker 的 wake/retry/checkpoint 状态机故障。注入器现同步删除该迁移记录，恢复时
  确实重建 inbox；既有 Worker 自动周期 drain，无需新 Recall，且仅在 enqueue 成功后 checkpoint。
  `B3-RECALL-004-RESILIENCE` 实际 PASS，五项 `NPOLICY-*` 仍 PASS；本轮随机隔离 schema 均已
  清理。验收脚本同时兼容本机 PowerShell 5.1/.NET Framework 的 JSON、SHA-256、相对路径、原生命令
  stderr、Docker port 与 schema 命令调用。无数据库迁移，无真实 QQ 连接或消息发送；未提交、未推送、
  未合并、未 stash。完整 Release Gate 仍为 `REJECTED`，因为缺 L4/L5 独立 attestation。

- `2026-07-29 22:57（Asia/Shanghai）`：Owner Notification Policy Feedback v1 的 Task 7
  代码与 L3 验收完成。`NPOLICY-PERSISTENCE-001`、`NPOLICY-MIGRATION-001`、
  `NPOLICY-EVALUATION-001`、`NPOLICY-DELIVERY-001`、`NPOLICY-RECONCILIATION-001`
  全部 PASS；修复的是第二条 FollowUp 投递状态机测试遗漏 legacy Outbox fixture，而非恢复
  扫描直写 Outbox 的旧行为。完整门禁仍为 `REJECTED`：B3/B4/B6/B7 多项缺 L4/L5 独立证明，
  且 `B3-RECALL-004-RESILIENCE` 确有 WAL 在 MySQL 恢复后未于超时内转存的失败。验收脚本本轮
  创建的隔离 schema 已清理；后续盘点确认有 4 个先前遗留的 `qqbot_accept_*` schema，未在无明确授权下删除。
  未提交、未推送、未合并，未连接或发送 QQ 消息。

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
