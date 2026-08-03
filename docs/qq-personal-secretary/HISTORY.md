# 个人 QQ 智能秘书开发历史索引

> 本文件只做导航和当前阶段摘要；具体事件进入 `history/` 归档。
> 新事件必须精确到 `YYYY-MM-DD HH:mm（Asia/Shanghai）`，缺少可信分钟的旧事件不得猜测回填。

## 当前阶段

- 主干分支：`Main`（`ea2226a`）；Owner 通知策略响应工件已合并。QQBot 运行数据库使用独立容器、独立数据库和
  独立持久化卷，不复用数字人数据库。
- 当前开发分支：`claude/qqbot-participant-causality-v1`，基线提交 `38dd23c`（有界 Replan 闭环）；
  参与者稳定身份 + 事件因果关系 + 人物上下文 v1 已完成。23:49 复核的 2 个跨层 P0
  （kind 在 Effect/TempRef 边界丢失、按名查询未验证当前值与 alias 自身建立来源）已于
  2026-08-03 00:22 修复并通过闭环反例；2026-08-03 10:19 Codex 最终复核通过，切片批准提交。
- 当前能力：可靠入站、空窗回补、确定性 EventThread、类型化语义、跨会话关联候选、Owner
  关联审核、高影响线程变更的持久化 Suspend/Resume、授权撤销、语义失效，以及来源化人物/
  项目/承诺结构记忆、证据回读、Owner 派生记忆删除、承诺提醒 Outbox、独立 QQ 开放平台
  协议适配、类型化 Agent 动作策略门、可选 OpenAI-compatible/Ollama 有界线程语义提取、
  并发优雅关闭（RuntimeWorkers + 25s 全局 deadline）、可编程运行时入口（run_with_cancellation）、
  群白名单过滤、Owner Retriever、受约束 Action Planner、真实 Effect Receipt、响应产物，
  以及 Action Run 的持久化 Suspend/Resume CAS 闭环。
  **新增（本轮）**：协议无关 AgendaItem/Mutation、创建/查询/改期/稍后提醒/完成/取消 Action、
  L2 Owner 审批、不可变审计、版本 fencing、到期 Scheduler 和复用的 Owner-only Outbox。
- 当前边界：NapCat 保持只读；旧验收矩阵只保留历史用途，不再作为日常开发门禁。结构化记忆候选
  (MEM-011) 已提交（`94ef5d9`）。Agent 有界事件证据视图 v1 已完成：有界最近窗口与 retrieved
  真实入模，模型仅看到请求内临时引用，local_only 只向已验证 loopback 模型开放；关键 MySQL
  关系视图用例已在随机隔离 schema 通过。CTX-004 的生产路径 P0/P1 已闭合，全 Graph 闭环集成测试、
  隐私视图测试及随机隔离 MySQL 主路径测试均已完成；切片进入提交候选。
  **本轮（未提交）**：参与者稳定身份与事件因果关系闭环 v1 已完成初版与既定主路径验证 ——
  `AccountScopedParticipantRef` 账号作用域身份、`secretary_participant_profiles` 档案表与
  `secretary_event_relations` 可重建 VIEW、`EventCausalContext`/`ParticipantContext` 两个 L0
  只读投影、类型化因果角色（发送者/回复根/线程发起人/要求者/承诺人/负责人/受益方）、确认人物
  记忆（职责/沟通偏好）接线与 envelope_only/失效来源保护、Planner 临时引用 fail-closed；
  9.1/9.2/9.3 三组指定测试与第十节命令通过，随机隔离 schema 已清理；但独立复核发现档案
  第三次变化唯一键冲突、群角色/群名片缺少会话作用域、隐私来源失效不完整、有效线程投影不一致、
  未知 conversation_ref 静默降级及自然语言两步查询不可达，故未批准为切片完成。

## 历史分块

| 时间范围 | 主题 | 记录 |
|---|---|---|
| 2026-07-23～2026-07-24 | 个人秘书立项、NapCat 验证、可靠入站、Gap 回补、线程与 Owner 审核 | [2026-07 归档](history/2026-07.md) |
| 2026-08-01～ | 上线前 TODO 连续收口 | [2026-08 归档](history/2026-08.md) |

## 最近事件

- `2026-08-02 23:49（Asia/Shanghai）`：Codex 提交前闭环复核未批准：
  - P0：by-name 候选已携带 PlatformIdentityKind，但 Effect 只取 stable ID 再调用仅接受 actor ID 的
    participant_context；报告中的双 kind 测试只验证候选仓储，复合 L0 Action 仍会报多命名空间歧义。
    同样，TempRefMap actor 映射也只保存字符串 ID，完整账号作用域参与者引用尚未贯穿 Action。
  - P0：按名 SQL 只验证 bounded source list，没有独立验证 Profile/Observation 的
    established_by_event_id；alias JSON_TABLE 只投影 alias 文本，没有验证该 alias 自己的来源事件。
    建立来源被窗口淘汰后再删除，直接上下文会失效，但按名解析仍可能命中。
  - 已确认 23:40 的局部修改本身有效：保留最新来源、按会话限制 Observation JOIN、来源列表门、
    alias 字段定向搜索及 kind 入数据库键均已进入生产代码；需要补的是跨层传递和属性级来源门。

- `2026-08-02 23:40（Asia/Shanghai）`：第二次复核的 2 P0 + 1 P1 修复完成并复验通过
  （详见 [`history/2026-08.md`](history/2026-08.md) 同时间条目）：
  - P0 最新来源截断：档案/观察来源列表满 10 条时淘汰最旧保留第 11 条建立事件；
    显示名/群名片/群角色增加 `established_by_event_id` 单列独立失效校验；
    observation 表新增 `established_by_event_id` 列。
  - P0 按名查询：`participants_by_display_name` 真实使用 conversation/thread 过滤群名片
    （无会话绝不跨群）；Profile/Observation 匹配前经 JSON_TABLE 来源有效性门；
    alias 只搜 `$.alias` 字段（不再误中 source_event_id）。
  - P1 身份键：profiles/observations 新增 `platform_identity_kind` 并纳入唯一键；
    读取按账号内稳定 ID 全量取出，跨命名空间歧义 fail-closed 拒绝，绝不静默合并。
  - 测试：9.3 新增第 8/9/10 段反例（来源淘汰 + 建立事件失效、跨群同名片隔离 +
    失效来源不参与解析、external/owner 同 ID 并存 + 歧义拒绝）全部真实通过；
    fmt/clippy 零告警、lib 238+118、9.1/9.2 通过，随机 schema finally 清理。
  - 未提交、未推送、未合并，`.mcp.json` 未触碰。

- `2026-08-02 23:11（Asia/Shanghai）`：Codex 第二次复核参与者/因果关系 v1，原 4 P0 + 4 P1
  修复主体得到确认，但仍不批准提交：
  - P0：Profile 与 Conversation Observation 的有界来源都使用 `push -> truncate(10)`，列表满后
    丢弃最新事件；当前显示值仍由该事件更新，却无法在它被撤回/删除后通过 `source_refs_valid` 失效。
  - P0：复合按名查询的仓储实现忽略 conversation/thread，跨所有群观察匹配群名片，并且没有在
    匹配前验证 Profile/Observation 来源；受限或已删除来源仍可决定人物解析结果。
  - P1：领域身份包含 platform kind，但档案当前键仍只有 account + actor ID，同账号不同身份命名空间
    的相同字符串会合并。现有测试未覆盖上述三个反例，TODO 已新增聚焦修复项。
  - 已确认有效：仅当前版本唯一键、会话观察直接读取、正文投影缺失 fail-closed、有效线程查询、
    未知 TempRef 拒绝、snapshot 类型和 alias establishment 来源修复均真实进入生产路径。

- `2026-08-02 22:56（Asia/Shanghai）`：参与者/因果关系 v1 修复轮次完成，22:15 复核的 4 个 P0 + 4 个
  P1 全部修复并复验通过，停候 Codex 复核（详见 [`history/2026-08.md`](history/2026-08.md) 同时间条目）：
  - P0 修复：`current_head` 生成列唯一键只约束当前版本；群名片/群角色改为会话作用域观察表；
    写入端逐事件 content_mode 门禁 + 读取端 JSON_TABLE 来源校验 + person/commitment LEFT JOIN
    fail-closed；新增复合 L0 动作 `GetParticipantContextByName` 闭合 NL 人物查询链。
  - P1 修复：线程参与者/承诺统一走 `secretary_effective_thread_events`；未知 conversation_ref
    fail-closed 返回 `InvalidOutput`；`directory_snapshot_id` 改为 `CHAR(36)`；alias 来源改用
    `established_by_event_id` 精确引用建立显示名的来源事件。
  - 测试：9.3 主路径扩为 8 段（含第三次资料变化、跨群角色、投影删除、alias 来源、复合查询），
    merge/split 有效线程测试新增并通过；9.2 扩展未注册引用 fail-closed；9.1 不变。
  - 验证：fmt/clippy 零告警，lib 238+118 通过，9.1/9.2/9.3 全绿，随机隔离 schema finally 清理，
    工作树边界无数字人文件；未提交、未推送、未合并，`.mcp.json` 未触碰。

- `2026-08-02 22:15（Asia/Shanghai）`：Codex 对参与者/因果关系 v1 做独立复核，结论为**不批准提交**。
  - P0：`UNIQUE(account_id, actor_platform_id, current)` 使第二条历史行无法产生，同一参与者第三次
    显示资料变化会回滚消息入站；现有 MySQL 测试未覆盖。
  - P0：群名片与群角色按账号 + Actor 全局保存，无法表达同一 Actor 在不同群的不同身份；
    `participant_context` 接收 conversation/thread 参数却未据此过滤或验证证据。
  - P0：档案只检查会话 memory_mode，且已确认人物/承诺查询在正文投影缺失时 fail-open；受限或
    被删除的单事件来源仍可能支撑长期人物上下文。
  - P0：提示要求 `ResolveReference -> GetParticipantContext`，但二者不在 Replan 白名单，单次
    Owner 自然语言人物查询没有可达执行链。
  - P1：线程参与者与承诺仍查询原始 `secretary_thread_events`；未知可选 conversation_ref 被静默
    降级为 None。TODO 已新增对应修复项，原有测试通过仍如实保留为“已覆盖主路径”。

- `2026-08-02 22:08（Asia/Shanghai）`：参与者稳定身份 + 事件因果关系 + 人物上下文 v1 实现与验证完成
  （任务切片：ID-004/ID-005 + THR-011/THR-012/THR-013 + MEM-002），工作树停候 Codex 评审，未提交。
  详见 [`history/2026-08.md`](history/2026-08.md) 同时间条目：
  - 领域层：`AccountScopedParticipantRef`（账号作用域身份，复用 `ParticipantIdentity`）、
    `GroupRole`（群主/管理员/成员/未知，协议字段归一化）、`ParticipantContextView`、
    `EventCausalContextView` 与 `EventRelationKind`（SentBy/RepliesTo/MemberOfThread/ThreadRootBy/
    Mentions/RequestedBy/AssignedTo/PromisedBy/Benefits）及其有界校验纯函数。
  - 投影：`secretary_participant_profiles`（档案表，显示名/群名片/群角色/有界别名/来源）、
    `secretary_event_relations` VIEW（5 个 UNION ALL 分支 + `JSON_TABLE`，可重建只读投影）。
  - 生产写入：`ObservedSenderProfile` 入站档案，事务内幂等 upsert（首次观察/无变化追加来源/
    变化旋转旧行进别名）；`never_long_term` 会话跳过；修复未知/空群角色字符串导致整条消息
    入库失败的 CHECK 约束缺陷（`GroupRole::parse_protocol` 归一化）。
  - 两个 L0 只读 Action：`GetEventCausalContext { source_event_id }`、
    `GetParticipantContext { actor_id, conversation_ref?, thread_id? }`，接入风险策略、Planner
    allowlist、序列化校验与 EffectExecutor；执行结果为有界安全中文摘要 + 类型化事件，LLM 只看到
    `evt_N/actor_N/thread_N/conv_N` 临时引用，未注册引用 fail-closed。
  - 人物记忆（MEM-002）：已确认 `PersonMemory` 的职责/沟通偏好进入参与者上下文；未批准候选
    绝不进入确认字段；envelope_only/never_long_term/已召回来源不支撑人物事实；记忆保留
    `source_event_ids` 且失效后不作为有效返回。
  - 验证（第十节命令全部真实通过）：fmt/check/clippy 零告警；personal-secretary lib 238 条、
    qqbot-server lib 118 条（2 条 MySQL 忽略）、workspace_boundaries 19 条全部通过；
    9.1 领域表驱动（严格角色分离/未确认不伪装/昵称群角色不授权）、9.2 Fake Planner 隐私
    fail-closed、9.3 随机隔离 MySQL 主路径（Alice 要求→Bob 回复并 @Carol→Carol 承诺；确认语义
    Alice=要求者、Carol=承诺人、Alice=受益方、负责人为空；账号 B 复用 actor_id/message_id 零关联；
    envelope_only 来源不泄漏；PlannerUseCase 全闭环恰好一条 Response Artifact）均真实通过，
    随机 schema 在 finally 清理；另发现本会话两次旧结构失败运行残留 2 个 `qqbot_accept_pc*`
    schema（panic 路径未清理，已修复测试结构）与 07-29/07-30 历史残留 5 个，待用户授权删除。
  - 未提交、推送或合并，`.mcp.json` 未触碰。

- `2026-08-02 20:28（Asia/Shanghai）`：CTX-004-VERIFY 完成 MySQL 主路径集成测试并真实通过：
  - **MySQL Replan 闭环测试**（`mysql_replan_two_rounds_effect_and_response_singleton`）：
    在随机隔离 `qqbot_accept_*` schema 中使用 `ReplanPlanner`（Search→NoAction）+ 真实
    MySQL ActionStore + MySQL CheckpointStore（`with_checkpoint_db`）通过 `PlannerUseCase::run_once`
    运行完整 Replan 闭环路径（`ensure_action_run → claim_pending_run → execute_claimed →
    build_action_graph → run_checkpointed → mark_completed`）。
    Planner 调用 2 次、Effect Receipt 持久化 1 条、Response 持久化 1 份、
    响应文本不含 JSON 字段（`query_effect`/`version`/`tool_kind`/`typed_events`）。
    模拟重启（重建 `build_mysql_action_store`）后 `load_effect_receipt` 返回持久化回执，
    effect_receipts 仍为 1 行。测试完成后清理（action_runs / owner_bindings / source_events / accounts）。
  - 发现的次要事实：MySQL `apply_effect` 和 `load_effect_receipt` 均硬编码 `tool_kind: None`，
    `tool_kind` 不作为持久化字段存储；生产 EffectExecutor 在新执行与缓存重放返回前都会从当前
    Proposal 补回该字段，因此不影响 Replan 一致性校验，仅作为仓储返回值完整性的后续优化。
  - `2026-08-02 20:33（Asia/Shanghai）` Codex 使用健康的 `serverrs-qqbot-mysql` 创建一次性随机
    `qqbot_accept_*` schema 独立重跑该测试，结果 1/1 通过；finally 路径已删除临时 schema，
    未触碰数字人数据库或真实 QQ。
  - 独立验证：`cargo fmt --check` 通过；两个 crate 严格 Clippy 通过；
    personal-secretary 236 条 lib 测试通过；qqbot-server 117 条 lib 测试通过。
  - 未提交、推送或合并，`.mcp.json` 未触碰。

- `2026-08-02 20:05（Asia/Shanghai）`：Codex 独立复核 CTX-004-VERIFY 的 Graph 闭环证据与隐私视图验证：
  - **全 Graph 集成测试**（`replan_full_graph_two_rounds_search_then_no_action`）：
    使用 RecordingPlanner + FakeEffectExecutor 运行生产等价的 4 个不同节点、7 次节点访问
    （Plan→L0Execute→ReplanDecision→Plan→L0Execute→ReplanDecision→BuildResponse→End），
    断言 Planner 恰好调用 2 次、Effect 执行 1 次、响应（Outcome + ResponseReady）各 1 份。
  - **预算耗尽路径**（`replan_full_graph_budget_exhausted_finishes`）：
    验证 MAX_REPLAN_ROUNDS=2 且每轮都返回 Proposal 时，ReplanRouter 正确终止循环；
    BuildResponseNode 从 last_receipt 构造响应并设置 Outcome，图正常终止不报 MissingOutcome。
  - **隐私视图测试**（qqbot-server action_planner）：
    `observation_with_typed_events_maps_to_temp_refs_not_real_ids` 验证
    typed_events 中的真实事件/Actor ID 不出现在序列化的 LLM 输入 JSON 中，
    临时引用 evt_N/actor_N 正确出现。
    `observation_without_typed_events_only_shows_count` 验证
    typed_events 为空时只输出有界计数，绝不泄露原始 summary。
  - 独立验证：personal-secretary 236 条测试（+2）全部通过；qqbot-server 117 条测试（+2）全部通过；
    两个 crate 严格 Clippy（`-D warnings`）与 `cargo fmt --check` 均通过。
  - MySQL 主路径：仓库中尚无 CTX-004 对应测试，不能仅以环境变量未设置解释为“跳过”；
    Docker MySQL 当前健康，需先补一条最小随机隔离 MySQL 主路径再真实运行。
  - 未提交、推送或合并，`.mcp.json` 未触碰。

- `2026-08-02 19:46（Asia/Shanghai）`：Codex 独立复核 CTX-004 第四轮修复，三个 P0 与一个 P1 已闭合：
  - P0-PRIVACY：GetThreadContext/ResolveReference 移出 Replan 白名单（9→7）；typed_events 为空
    时绝不复用 raw summary，只输出有界计数；映射缺失 fail-closed（build_llm_views 返回 Result）；
    validate_tool_observation 新增 typed_events 数量/去重/字段长度/集合一致性校验。
  - P0-RESPONSE：BuildResponseNode 的 last_receipt 路径先解析 QueryEffectResultV1 提取 summary，
    避免向 Owner 显示结构化 JSON。
  - 白名单更新：ResolveReference 和 GetThreadContext 不再触发 Replan（摘要含不可安全投影的稳定 ID）。
  - 独立验证：格式检查、两个相关 crate 严格 Clippy、29 条 action_graph 聚焦测试、25 条
    action_planner 聚焦测试及 `git diff --check` 均通过。仍缺 CTX-004-VERIFY 闭环测试。
    未提交、推送或合并，未连接或发送 QQ，`.mcp.json` 未触碰。

- `2026-08-02 19:14（Asia/Shanghai）`：CTX-004 第三轮修复复核部分通过，原文档误写尚未到达的
  21:30，现按实际复核分钟纠正。Search/Read 已增加 typed_events 并正确投影 evt_N/actor_N；
  BuildResponse 的 Outcome 路径已直接生成 OwnerResponseDraft。剩余 P0：typed_events 为空时仍回退
  raw summary + String::replace，GetThreadContext/ResolveReference 仍可能含真实稳定 ID；typed event
  映射缺失时直接回退真实 event ID。两轮预算耗尽但无 Outcome 时仍可能向 Owner 显示 Query JSON。
  typed_events 的数量/字段/集合一致性也未纳入状态校验，全 Graph/MySQL 证据仍缺。当前未提交、
  推送或合并，未连接或发送 QQ，`.mcp.json` 未触碰。

- `2026-08-02 18:54（Asia/Shanghai）`：CTX-004 第二轮修复复核仍未批准。代码虽预注册
  observation.source_event_ids 并生成 source_event_refs，但随后以 `String::replace` 处理 summary，
  没有类型化 Actor/Thread/Claim 引用字段；Search/Read 的 actor_id 和 GetThreadContext 的 thread_id、
  actor_id、claimant/raised_by 仍原样入模。BuildResponse 修复也把 Outcome 文本绑定为 `_text` 后弃用，
  仍从 last_receipt 构造草稿，因此最终 Planner 回答继续丢失且 JSON 回显风险仍在。白名单缩减和
  version/tool_kind 校验方向正确，state 校验函数已补字段但真实 Checkpoint restore 入口尚未证明调用。
  未新增全 Graph/MySQL 闭环证据；未提交、推送或合并，未连接或发送 QQ，`.mcp.json` 未触碰。

- `2026-08-02 18:25（Asia/Shanghai）`：Codex 独立复核 CTX-004，结论未批准。确认 Graph 分支、
  两轮预算、旧回执保守终止和基础编译方向成立，格式、两个 crate 严格 Clippy、29 项 action_graph
  测试和 diff check 通过；但代码明确把包含 source_event_id、actor_id、thread_id 的 summary 原样
  放入 Observation LLM 视图，`source_event_refs` 反而固定为空，违反上一切片的临时引用边界并使
  Search→Read 无法引用新命中事件。第二轮 Planner 的 Outcome 没有生成 ResponseReady，完成时会
  回退 last_receipt，丢失最终回答并可能向 Owner 回显 QueryEffectResultV1 JSON。另有白名单与实际
  结构化结果不一致、Checkpoint 状态新字段未纳入整体校验、缺少真实 Graph/MySQL 闭环证据。当前
  未提交、推送或合并，未连接或发送 QQ，`.mcp.json` 未触碰。

- `2026-08-02 15:30（Asia/Shanghai）`：完成 CTX-004 有界多轮 Replan 闭环。
  - 领域新增：`PlannerToolObservation`（`planner.rs`）— Replan 工具观察，含 proposal_id、
    tool_kind、success、有界摘要和来源事件 ID；`QueryEffectResultV1`（`planner.rs`）—
    查询型 Effect 的结构化 JSON 结果，版本化、`deny_unknown_fields`。
  - 新增函数 `is_replan_observation_tool`：只有 13 种 L0ReadOnly 查询工具允许触发 Replan；
    写操作、审批和已产生 Outcome 的路径不进入循环。
  - 常量：`MAX_REPLAN_ROUNDS=2`、`MAX_TOOL_OBSERVATIONS=2`、`MAX_TOOL_OBSERVATION_CHARS=2_000`、
    `MAX_TOOL_OBSERVATION_TOTAL_CHARS=4_000`。
  - `SecretaryAgentState` 扩展：新增 `replan_round: u8`（`#[serde(default)]`，0-based）和
    `planning_observations: Vec<PlannerToolObservation>`（`#[serde(default)]`），旧 Checkpoint 兼容。
  - `SecretaryAgentUpdate` 新增 `ObservationAppended(PlannerToolObservation)`。
  - `SecretaryActionEffectExecutor`：查询工具（SearchRecentEvents/ReadSourceEvent 等）的 result_ref
    改为结构化 JSON（`QueryEffectResultV1`），非查询工具保持纯文本。
  - Graph 拓扑变更：`PlanNode` → `L0ExecuteNode` → `ReplanDecisionNode` → `(Plan|BuildResponse)` → `End`；
    `ReplanDecisionNode` 解析 receipt 中的 QueryEffectResultV1，追加观察到状态；
    `ReplanRouter` 基于预算、tool_kind 和 Outcome 选择 continue/finish。
  - `PlanNode` 在 round≥1 时从状态读取 observations，生成 `PlannerInput` 时填充。
  - LLM Action Planner：`PlannerLlmInput` 新增 `tool_observations`（`ObservationLlmView`）、
    `replan_round` 和 `remaining_query_budget`；观察摘要标记"[不可信工具数据]"；System Prompt 增加
    不可信数据说明和预算耗尽规则。
  - 测试：29 项 action_graph 测试通过（含 8 项新增 ReplanDecisionNode/ReplanRouter 测试），
    234 项 personal-secretary lib 测试通过，25 项 qqbot-server action_planner 测试通过，
    19 项 workspace_boundaries 测试通过。
  - 格式、严格 Clippy 通过；未连接 NapCat/QQ 开放平台，未发送消息，`.mcp.json` 未触碰。
  - 当前分支 `deepseek/qqbot-bounded-replan-v1`，基于 `c251942`；未提交、未推送、未合并。

- `2026-08-02 13:57（Asia/Shanghai）`：Codex 完成 AgentEventView 最终短复验并批准切片。
  source_event_id、thread_id、memory_source_event_ids 和 conversation 全部仅经 TempRefMap 恢复，
  未登记引用 fail-closed；删除直接解析旁路。MySQL `mentioned_actor_ids` 已 CAST 为 CHAR。使用容器
  当前凭据在随机隔离 schema 独立重跑 `recent_event_views_account_scoped_with_mentions_reply_and_thread`，
  1/1 通过并在 finally 清理。格式、严格 Clippy 和既有单元测试由交付方通过，Codex 复核 diff check
  通过；未连接 NapCat/QQ 开放平台，未发送消息，`.mcp.json` 未触碰。

- `2026-08-02 13:41（Asia/Shanghai）`：AgentEventView 第一轮修复复核仍未批准。确认 loopback
  从 AppConfig 经 PlannerUseCase/ActionRunContext/PlanNode 注入 Retriever 与 LlmActionPlanner，
  local_only 远程正文泄露已关闭；Actor/Mention 标签复用、Reply 指向父 evt、命令事件引用、有效
  Thread 视图和缺失正文投影降级也已落实。剩余 P0 是“兼容”分支仍直接接受非 evt_/thread_ 的
  原始 ID，conversation raw 回退及 memory_source_event_ids 也绕过 TempRefMap。启动本地 MySQL
  后在随机隔离 schema 重跑唯一测试，Thread fixture 已修复，但查询在 mentioned_actor_ids JSON
  直接解码为 String 时失败；需 `CAST(... AS CHAR)`。schema 已在 finally 清理；未提交、推送、
  合并，也未连接或发送 QQ。

- `2026-08-02 12:07（Asia/Shanghai）`：Codex 独立复核 Agent 有界事件证据视图，结论未批准。
  领域视图与 retrieved 入模方向成立，格式、两个 crate 严格 Clippy 和 diff check 通过；但发现两个
  P0：local_only 正文被无条件标记可见，且临时引用只保存事件 evidence 映射，未知合法 UUID 被
  直接接受、其他未知引用被静默丢弃，Action 的 source_event_id/thread_id 等字段也未回映。同一
  Actor/会话/Thread 每条事件生成不同标签，Reply 生成无法解析且不指向父事件的独立标签。另有
  两个 P1：查询使用原始 `secretary_thread_events` 而非有效投影视图，正文投影缺失会回退为 normal。
  随机隔离 schema 中真实运行新增 MySQL 测试时，fixture 的 Thread ID 超过 `CHAR(36)`，测试在
  Retriever 查询前失败；schema 已在 finally 清理。原文档使用未来时间 13:00 并宣称完成，现已按
  12:07 的可验证复核时间纠正。当前未提交、推送或合并，未连接或发送 QQ。

- `2026-08-02 11:06（Asia/Shanghai）`：Codex 完成 `MEM-011` 第三轮短复验。确认 deferred
  逐事件延期路径在远程过滤、本地优先领取、租约 fencing、主游标不倒退和提交清理之间闭合；
  该迁移文件在基线 `4e476af` 中不存在，属于本切片首次落地，不是对已发布迁移的改写。使用独立
  `target/codex-review` 运行两个相关 crate 严格 Clippy 通过；启动本地 QQBot MySQL 容器，在随机
  `qqbot_accept_codex_*` schema 中真实重跑候选测试 7/7，通过后在 finally 删除 schema。批准当前
  切片进入单次提交；数字人 CLI 的并行改动及未跟踪 `.mcp.json` 明确排除，不推送、不合并。

- `2026-08-02 01:15（Asia/Shanghai）`：Codex 第三轮复核结构化记忆候选，确认第二轮 P0-1/P0-2/P1-3/
  P1-4/P1-5 修复正确通过，但发现新 P0-6：`local_only(L1,t1)` → `normal(N1,t2)` → 远程领取 N1
  推进游标到 t2 → 切本地后 L1 永久不可达。修复方案：新增 `secretary_memory_candidate_deferred`
  持久化逐事件延期表；远程 `claim` 把批次内被过滤 local_only INSERT IGNORE 入延期表（范围在
  游标之前或等位）；本地 `claim` 优先 `claim_deferred_batch` 消费最早会话首个连续前缀但
  `next_cursor` 恒为当前主游标（不推进）；`commit` 删除已处理延期行。新增 Codex 指定回归测试
  `memory_candidate_local_only_before_normal_survives_remote_then_local`，验证远程越过后本地仍
  可领取。fmt ✓、clippy -D warnings ✓、单元 223+112 ✓、候选 MySQL 7/7 ✓、其余 22/30（8 基线
  失败）。修正交付报告基线为 `4e476af`（非 `71a0898`）。TODO/HISTORY/月度历史已同步。当前
  未提交、未推送、未合并，`.mcp.json` 未触碰。

- `2026-08-02 00:04（Asia/Shanghai）`：Codex 第二轮复核结构化记忆候选。确认连续同会话前缀、
  Actor—来源三层绑定、LLM 临时引用、缺失正文投影失效和精确版本 CHECK 均已落实；格式与两个
  crate 严格 Clippy 通过，并在 `serverrs-qqbot-mysql` 随机隔离 schema 中真实重跑 6 条候选 MySQL
  测试，全部通过，schema 已清理。新发现阻断边界：处理状态只有账号级全局游标，远程模型会过滤
  local_only；当 local_only 事件早于后续 normal 事件时，处理 normal 会永久越过前者，之后切换
  本地模型无法补提。另纠正交付报告基线：实际 HEAD 为 `4e476af`，不是 `71a0898`。当前未批准、
  未提交、推送或合并，也未连接或发送 QQ。

- `2026-08-01 23:12（Asia/Shanghai）`：重构根目录 `CLAUDE.md`，从 171 行历史事故与重复清单
  调整为长期工程规则。明确当前指令/规格/TODO/长期规则/历史的优先级，禁止 Superpowers 和旧式
  验收矩阵，固化数字人/QQBot 隔离、`Main` 大小写、`.mcp.json` 保护、精简风险验证及每次 QQBot
  提交同步 TODO/HISTORY/月度历史。针对本轮复核遗漏，新增两个强制评审不变量：全局游标不得越过
  交错 scope 事件；语义 Actor 必须与权威 SourceEvent 及 primary 来源形成不可绕过的绑定。
  本次只修改规则与文档，未改业务代码，未提交、推送或合并。

- `2026-08-01 23:08（Asia/Shanghai）`：Codex 独立复核结构化记忆候选修复报告。格式检查、两
  crate 严格 Clippy、personal-secretary 223 个领域单测、qqbot-server 109 个单测均通过；使用
  `serverrs-qqbot-mysql` 新建随机隔离 schema 重跑 4 条 memory candidate MySQL 场景，全部通过，
  schema 已在 finally 清理。复核同时确认两个 P0：账号全局游标配合按会话选取会永久跳过交错
  会话事件；候选来源 Actor/primary actor event 未被强制绑定到权威 SourceEvent，可能形成来源
  不支持的 Confirmed Fact。另记录远程 LLM 稳定平台 ID 暴露、缺失正文投影未失效和 DDL CHECK
  过宽三项 P1。当前切片保持未完成，未提交、推送、合并，也未连接或发送 QQ。

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
