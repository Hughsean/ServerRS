# 个人 QQ 智能秘书执行看板

> 最后整理：2026-08-03（Asia/Shanghai）
> 本文件只保留当前工作、下一批切片、未完成项和外部阻塞。已完成事项及分钟级证据进入
> [`HISTORY.md`](HISTORY.md) 与 [`history/`](history/)，不再在 TODO 中重复维护长篇交付报告。
>
> 开发规则：按垂直切片推进；一个切片完成、复核并提交后再进入下一切片。旧验收矩阵、签名
> attestation 和依赖真实 QQ/NapCat 的仓库内人工验收测试已移除；测试数量与风险相称。需要用户
> 凭据、QQ/NapCat 实机或远端管理权限的
> 事项单列为 `EXTERNAL`，不得伪造完成，也不得阻塞可在本地继续的任务。
>
> **提交硬规则：任何 QQBot 代码、配置、迁移或测试提交，必须在同一提交中同步本文件、
> `HISTORY.md` 和对应月份的 `history/YYYY-MM.md`。缺少任一项时不得提交，也不得宣称切片完成。**

## 0. 当前状态

- 当前分支：`claude/qqbot-project-commitment-memory-v1`（HEAD `1779c71`）。
- 当前切片：QQBot 旧测试与废弃验收基础设施清理；删除全部长期忽略且失真的聚合验收目标、
  acceptance workflow/脚本/矩阵，以及依赖真实 QQ/NapCat 的旧人工测试；保留现行 Action
  Planner、参与者因果、项目承诺 MySQL 聚焦测试和 NapCat 本地 mock 测试。
- 当前架构判断：不可变 `SourceEvent`、内容信封和语义投影方向保持不变，不进行全量重写。
- 下一步：进入 `CMD-009` 跨阶段有界状态、长期事件检索排序和冲突驱动回读。
- 当前安全边界：NapCat 只读；只有绑定 Owner 的 QQ 开放平台控制消息可成为 `OwnerCommand`；
  所有第三方自动回复继续延期；群管理员只是群角色，不构成系统 Owner。

## 1. 立即执行顺序

### 1.1 收口当前切片

- [x] `TEST-CLEANUP-001` 删除 10,287 行、38 项全部 `#[ignore]` 的旧
  `mysql_ingestion.rs` 聚合测试，以及旧 `qqbot_acceptance_mysql` / `qqbot_acceptance_runtime`
  验收目标；这些目标默认不运行、长期与现行状态机漂移，不能继续充当可靠门禁。
- [x] `TEST-CLEANUP-002` 删除废弃的验收矩阵 JSON、PowerShell 门禁与 attestation helper、GitHub
  workflow；历史计划和月度记录仍保留，作为当时发生过的事实而非可执行入口。
- [x] `TEST-CLEANUP-003` 删除仓库内真实 QQ/NapCat E2E、双账号主动群测试和旧 live 契约测试；
  NapCat 仍保持只读，未来实机验证作为明确授权的外部上线步骤，不再常驻开发测试集。
- [x] `TEST-CLEANUP-VERIFY` 已通过格式检查、QQBot 三个 crate 全 targets 编译、严格 Clippy、
  personal-secretary 238/238、qqbot 63/63、qqbot-server 118/118（2 项 live LLM ignored）和
  workspace boundaries 19/19；保留的 13 项 MySQL 测试均成功编译并按预期保持显式 ignored。

- [x] `DB-BASELINE-001` 将 33 个压缩前迁移移入 `database/archive/pre_v1`，从随机隔离 MySQL
  的最终结构生成 `baseline/20260803_qqbot_schema_v1.sql`；基线只含最终 DDL，不含业务数据、
  测试数据、凭据或历史 `ALTER/DROP`。为兼容连接池，按“全部建表 → 统一添加外键 → 建 View”
  三阶段执行，不依赖连接级 `FOREIGN_KEY_CHECKS`。
- [x] `DB-BASELINE-VERIFY` 已验证旧 33 迁移链与新基线的归一化 `SHOW CREATE` 语义哈希一致；
  空库加载、重复加载、完整旧链采用、部分结构拒绝、Recall WAL 恢复和 workspace boundaries
  均真实通过。4 个本切片随机 schema 已精确清理，未触碰既有业务库或其他测试库。

- [x] `MEM-011` 完成结构化记忆候选生产、持久化、Owner 查询/批准/拒绝、来源引用、版本 fencing、
  Suspend/Resume、Effect Receipt 与响应产物；确认不会把未审批候选当成长期事实。
- [x] `MEM-011-P0-CURSOR` 修复按会话分批与账号级全局游标的冲突。对于按接收顺序交错的
  `群 A -> 群 B -> 群 A`，不得因一次领取两个 A 事件而把中间 B 事件永久越过；游标只能推进到
  实际处理过的连续全局前缀，或改为具有明确一致性语义的会话级游标。交错会话回归测试已通过。
- [x] `MEM-011-P0-EVIDENCE` 强制候选来源 Actor 与 `SourceEvent` 的权威 Actor 一致；Person 的
  `person_event_id`、Commitment 的 promisor/beneficiary 事件必须进入来源集合。批准时从原始事件
  复验身份，不得仅信任候选来源表里的冗余 Actor 字段。提取、领域和审批三层绑定已验证。
- [x] `MEM-011-P1-PRIVACY` 远程 LLM 输入使用批次内不透明引用，不直接发送群号、QQ 号、OpenID
  或其他稳定平台标识；模型输出在本地映射回账号作用域参与者。
- [x] `MEM-011-P1-INVALIDATION` 来源事件仍在但正文投影缺失时，候选也必须失效；失效查询不得因
  `INNER JOIN secretary_message_contents` 无结果而让候选永久停留在 proposed。
- [x] `MEM-011-P1-DDL` 收紧 `approve_conflict` 审计版本 CHECK：冲突只能保持版本不变，普通
  approve/reject 才允许精确 `previous + 1`。
- [x] `MEM-011-P0-TRUST-CURSOR` 修复信任配置切换造成的历史事件永久丢失。新增
  `secretary_memory_candidate_deferred` 持久化逐事件延期状态：远程模式领取时把批次范围内被过滤的
  local_only 事件 INSERT IGNORE 写入延期表；本地模式优先消费延期事件后提交删除对应行。延期消费
  不推进主游标，提交清理保证幂等。已覆盖 Codex 反例（local_only 在前 → normal 在后 → 远程 →
  本地）的 MySQL 回归测试。
- [x] `MEM-011-VERIFY` 只运行受影响 crate 的格式、编译、严格 Clippy、领域测试，以及覆盖候选
  生成→审批→事实落库→二次 Resume 拒绝的隔离 MySQL 主路径；交错会话不丢事件和来源
  Actor 绑定两条关键回归证据已通过（P0-1/P0-2），并补了 P0-6 配置切换不丢历史的回归测试。
  候选 7/7 全部通过，其余 MySQL 22/30（8 条基线失败非本切片新增）。
- [x] `MEM-011-DOC` 已完成三轮 Codex 评审修复（P0-1/P0-2/P1-3/P1-4/P1-5/P0-6），真实结果
  已写入 `HISTORY.md`/月度历史；2026-08-02 11:06 Codex 在独立目标目录完成严格 Clippy，并用
  随机隔离 schema 重跑候选 MySQL 7/7。切片代码与文档同步完成，进入单次提交。

### 1.2 Agent 上下文垂直切片（下一切片，P0）

- [x] `CTX-001` 定义协议无关、有界的 `AgentEventView`，至少包含：
  `source_event_id`、账号作用域参与者引用、会话引用、发生时间、消息角色、内容策略、有限正文摘录、
  Reply 父事件、@目标、Thread 引用及来源可信度。稳定 ID 用于授权与关联，昵称只作显示。
- [x] `CTX-002` 将 `PlanNode` 已取得的 `retrieved` 证据真实序列化到 Planner LLM 输入；同时传入
  命令来源事件、会话与类型化关系。所有模型可见实体使用请求内临时引用并在本地 fail-closed 回映。
- [x] `CTX-003` 从事件仓储填充最多 3～8 条最近事件窗口；内容受
  `normal/local_only/envelope_only/never_long_term` 约束，列表、正文和总字节均有硬上限。
- [x] `CTX-004` 让检索动作结果可以进入下一轮有界规划，形成最小
  `Plan -> Read/Search -> Replan -> Respond` 闭环；限制最大轮次和总输入预算，不保存完整 Thought。
  - 实现：`ReplanDecisionNode` + `ReplanRouter`（`action_graph/nodes.rs`）；
    `PlannerToolObservation` + `QueryEffectResultV1`（`planner.rs`）；EffectExecutor 结构化 JSON
    result_ref；`SecretaryAgentState` 新增 `replan_round`/`planning_observations`；
    `LlmActionPlanner` 新增 `tool_observations` 输入与系统提示更新；
    Graph 拓扑 `Plan → L0Execute → ReplanDecision → (Plan|BuildResponse) → End`。
  - 预算：`MAX_REPLAN_ROUNDS=2`，单条观察 2000 字符、合计 4000 字符；只允许 L0ReadOnly 查询工具
    触发 Replan；非查询工具或不可解析 result_ref 保守进入 BuildResponse。
  - **已闭合**：P0-1 类型化 Observation TempRefMap 投影、P0-2 最终回答持久化、typed_events
    空分支 fail-closed、缺失映射拒绝、预算耗尽安全摘要及 typed_events 完整边界校验。
  - **验证闭合**：完整 Graph、隐私投影及随机隔离 MySQL 主路径测试均已通过；真实持久化边界下
    Planner 两次、Effect Receipt 一条、Response 一份，重建仓储后回执仍可读取且不重复。
- [x] `CTX-004-P0-PRIVACY` Query Effect 必须保存类型化实体字段；LLM 投影阶段将事件/Actor/会话/
  Thread 映射为 TempRefMap 临时引用。禁止对包含稳定 ID 的 summary 做原样透传或字符串替换。
  第四轮修复：GetThreadContext/ResolveReference 已移出 Replan 白名单（7 工具）；typed_events 为空时
  只输出有界计数摘要，绝不回退 raw summary；映射缺失时 `build_llm_views` 返回 Err（fail-closed）。
  新增 `validate_tool_observation` 校验 typed_events 数量/去重/字段长度/集合一致性。
- [x] `CTX-004-P0-RESPONSE` Replan 最终 NoAction/回答必须形成有界 `OwnerResponseDraft`；预算耗尽时
  QueryEffectResultV1 也必须渲染为安全中文摘要，不能把 JSON 原文放入响应。Outcome 路径已正确；
  BuildResponseNode 的 last_receipt 路径现在先尝试解析 QueryEffectResultV1 提取 summary，解析失败
  才回退 `build_action_response_draft`。
- [x] `CTX-004-P1-CONSISTENCY` 解析 QueryEffectResultV1 时校验 version、tool_kind 与 receipt 一致，
  并让声明允许 Replan 的工具都真正产生该结构。白名单已收敛到 7 个实际产生 QERV1 的 L0ReadOnly
  工具；`ReplanDecisionNode` 校验 version==1 与 tool_kind 匹配，不匹配时保守终止 Replan。
- [x] `CTX-004-P1-STATE` `validate_agent_state` 必须校验 replan_round、观察数量、单条/总字符数和
  proposal 去重，保证反序列化的旧/异常 Checkpoint 也受界限约束。`validate_agent_state` 已扩展 Replan
  字段校验；`apply_update` 中 `ObservationAppended` 按 proposal_id 去重并用 saturating_add 推进轮次。
- [x] `CTX-004-VERIFY`（Graph ✓；MySQL ✓ 已真实通过）增加一条真实 Graph 主路径（第一次 Search/Read、第二次收到 Observation、
  最终 Respond）以及一条随机隔离 MySQL 主路径；断言 Planner 两次、Effect 一次、响应一份、无真实
  稳定 ID 入模。不要再用只测 Node/Router 的碎片测试替代闭环证据。
- [x] `CTX-005` 最小验证覆盖：发送者/@/Reply 可见、跨账号不串联、隐私正文不入模、
  `retrieved` 确实进入请求、超长视图有界；关键 MySQL 用例已在随机隔离 schema 真实通过。
- [x] `CTX-P0-REFS` 临时引用必须 fail-closed：同一 Actor/会话/Thread 在整个请求内复用同一标签；
  Reply 必须指向父事件已有的 `evt_N`；命令事件引用必须出现在输入中；evidence 及所有 Action ID
  字段统一通过本地映射恢复。未知引用（包括格式合法的 UUID）返回 InvalidOutput，不再以兼容名义放行。
- [x] `CTX-P0-LOCAL-ONLY` 将已验证的 LLM loopback 属性传入最近窗口策略；远程模型永远不能获得
  local_only 正文，本地模型也只有策略显式允许时才能读取。AppConfig→UseCase→Node→Retriever
  与 LlmActionPlanner 链路已复核通过。
- [x] `CTX-P1-THREAD` 最近窗口使用 `secretary_effective_thread_events`，不得返回 merge/split 前的
  失效 Thread ID。
- [x] `CTX-P1-PROJECTION` `secretary_message_contents` 缺失时按受限信封处理，不能由 CASE 的 ELSE
  回退成 normal。当前明确降级为 never_long_term，正文不会进入模型。
- [x] `CTX-P1-JSON` MySQL JSON 列 `mentioned_actor_ids` 使用 `CAST(... AS CHAR)` 后再反序列化；
  随机隔离 schema 中的关键 MySQL 用例已真实通过并完成清理。

### 1.3 参与者与因果关系（P0）

- [x] `ID-004-P0-ACTION-KIND` 参与者 kind 必须贯穿 Retriever → Effect → Context。已实现：
  `ThreadActorRef` 增加 `platform_identity_kind`、`PlannerRetrievedExcerpt` 增加 `actor_kind`、
  `QueryEffectTypedEvent` 增加 `actor_kind`（旧 result_ref 反序列化保守兜底 External）；
  新增严格路径 `participant_context_by_ref(&AccountScopedParticipantRef, …)`（三元组精确读取，
  无歧义），宽松 `participant_context` 保留 fail-closed 歧义拒绝；`TempRefMap.actors` 与
  `actor_refs` 均映射完整 `AccountScopedParticipantRef`（key = `{kind}:{actor_id}`）；
  Effect `GetParticipantContext` 携带 `actor_kind` 并按完整引用读取；by-name 唯一候选同样
  走 `participant_context_by_ref`。9.3 第 11 段真实闭环：同 ID 双 kind → by-name("Alice-主")
  唯一命中 Owner → 复合 run → Response 成功且携带 Owner 显示名（已通过）。
- [x] `THR-013-P0-NAME-ESTABLISHMENT` 按名匹配必须验证当前显示名/群名片的
  `established_by_event_id`，alias 必须验证 alias 对象自己的 `source_event_id`。by-name SQL
  三个匹配分支各自增加独立单事件有效性门（派生表驱动，语义与 `single_event_valid` 一致：
  事件缺失/撤回/never_long_term/无投影即失效），不依赖有界 `source_event_ids_json`。
  9.3 第 12 段反例：Frank 建立事件被 10 条同值观察淘汰 → 删除建立事件 → 显示名分支不命中；
  改名后旧显示名进入别名（来源指向已删除事件）→ 别名分支不命中；有效显示名仍可命中（已通过）。

- [x] `ID-005-P0-LATEST-SOURCE` 有界来源淘汰最旧保留最新：档案与会话观察的来源列表满 10 条时
  `remove(0)` 淘汰最旧、保留第 11 条建立事件；显示名/群名片/群角色按 `established_by_event_id`
  单列独立校验（来源列表可能已把建立事件淘汰）。9.3 反例：10 条旧观察 → 第 11 条改值 →
  删除第 11 条投影 → 显示名失效，且来源列表含新事件、不含最旧事件。
- [x] `THR-013-P0-NAME-SCOPE` `participants_by_display_name` 真实使用 conversation/thread：
  群名片只在解析出的目标会话内匹配（无会话参数时观察不 JOIN，绝不跨群）；Profile 与
  Observation 匹配前必须通过 JSON_TABLE 来源有效性门（存在/无撤回/非 never_long_term/有投影/
  列表非空）；alias 用 `JSON_TABLE('$.alias')` 只搜别名字段，不搜 source_event_id。
  9.3 反例：Dave 群 A "A-名片" 不在群 B 查询命中、无会话不命中、删除建立事件投影后不命中。
- [x] `ID-004-P1-KIND-KEY` 档案与会话观察新增 `platform_identity_kind` 列并纳入唯一键
  （`(account_id, platform_identity_kind, current_head)` / `(account_id, conversation_id,
  platform_identity_kind, actor_platform_id)`）；写入按身份种类隔离，读取按账号内稳定 ID
  全量取出并在跨命名空间歧义时 fail-closed 拒绝（绝不静默合并）。9.3 反例：同账号同稳定 ID
  以 external 与 owner 并存两行、观察两行，participant_context 返回明确错误，
  by-name 按显示名解析出 Owner 身份。

- [x] `ID-005-P0-HISTORY` 档案历史唯一键改为仅约束当前版本：新增 STORED 生成列
  `current_head = IF(current=1, actor_platform_id, NULL)` + `UNIQUE(account_id, current_head)`，
  历史行（NULL）互不冲突，任意有界历史版本可共存；第三次资料变化已由 9.3 反例验证。
- [x] `ID-005-P0-SCOPE` 群名片/群角色改为会话作用域观察
  `secretary_participant_conversation_observations`（`UNIQUE(account_id, conversation_id,
  actor_platform_id)` + 来源 ≤10 CHECK）；`participant_context` 经 conversation/thread 解析
  observation 会话，A 群 Owner / B 群 Member 互不污染已由 9.3 跨群断言验证。
- [x] `ID-005-P0-PRIVACY` 档案写入经 `participant_observation_allowed`（单事件 content_mode +
  applied tombstone + 会话 memory_mode 任一受限即跳过）；读取侧 `source_refs_valid`（JSON_TABLE
  逐来源校验缺失/撤回/never_long_term/投影缺失）失效闭环，person/commitment LEFT JOIN
  在 `source_event_id IS NULL` 时 fail-closed；9.3 覆盖删除 e3 投影后承诺/受益人清空。
- [x] `THR-011-P1-EFFECTIVE` 线程参与者和线程承诺查询统一改为 JOIN
  `secretary_effective_thread_events`；merge/split 有效线程测试（`_merge` 场景）真实通过。
- [x] `THR-013-P0-CLOSED-LOOP` 提供单一复合 L0 查询 `GetParticipantContextByName { name,
  conversation_ref?, thread_id? }`：按显示名/别名/群名片有界解析（LIKE 转义、LIMIT 1..=5），
  唯一候选再读完整上下文，零/多候选返回有界中文摘要；NL 提示第 7 意图改指该工具，
  两步查询在单次命令内可达，9.3 复合 run 真实通过。
- [x] `THR-013-P1-REFS` `GetParticipantContext`/`GetParticipantContextByName` 的
  `conversation_ref` 只要出现就必须在 `TempRefMap` 中成功解析，未注册返回
  `InvalidOutput`（不再静默降级为 None）；9.2 扩展测试覆盖未注册引用。

- [x] `ID-004` 将参与者稳定身份显式建模为账号作用域引用，例如
  `(account_id, platform_kind, platform_actor_id)`；消除仅依赖外层查询维持账号隔离的隐患。
- [x] `ID-005` 补充可选显示名、群名片、历史别名、群角色和可信来源；别名不得用于授权，也不得
  仅凭同名把跨群或跨账号人物合并。
- [x] `THR-011` 建立可重建的事件关系视图：`sent_by`、`mentions`、`replies_to`、`member_of_thread`。
  原始 JSON 继续保留，关系视图只作索引和查询投影。
- [x] `THR-012` 明确区分消息发送者、回复链根发送者、话题发起者、要求提出者、承诺人、负责人、
  决策提议者和最终确认者。无法由来源证明时返回“未确认”，不得把最早发言人当成确定发起人。
- [x] `THR-013` Owner 可通过 `GetEventCausalContext`/`GetParticipantContext` 两个 L0 只读 Action
  查询“谁提出、在回复谁、谁参与、谁负责、职责与沟通偏好”；响应为安全中文有界摘要并携带
  可回读 `source_event_id`，LLM 只看到 `evt_N/actor_N` 临时引用，跨账号不泄露。
- [x] `MEM-002` 已确认人物记忆补充职责/沟通偏好（`PersonMemory` 字段 + `into_attributes`
  投影）；未批准候选绝不进入确认字段；envelope_only/never_long_term/已召回来源不支撑人物事实；
  人物记忆保留 `source_event_ids` 且失效后不作为有效返回。

### 1.4 继续业务 TODO（P0）

- [x] `MEM-003` 项目记忆闭环 v1：项目成员使用带身份 kind 的稳定引用，旧裸 ID 明确标记未知；
  `list_projects`/`query_project` 已接入 Planner、Retriever、Effect 与证据回读，账号隔离和全部来源
  fail-closed 已由真实 MySQL 验证。
- [x] `MEM-004` 承诺记忆闭环 v1：`list_commitments` 支持状态、期限和类型化参与者过滤；
  单条与批量 `CompleteFollowUp` 在同事务内完成 Pending → superseded → Fulfilled，Dismiss/Snooze
  不改变承诺语义；无期限承诺不进入 Scheduler。
- [x] `MEM-003/MEM-004-TEST`：3 个聚焦 MySQL 场景真实通过，覆盖项目跨账号与召回失效、
  完整 Owner 授权链与版本回滚、幂等重放、无期限不调度及批量 all-or-nothing。测试派生 schema
  名称已限制在 MySQL 64 字节内，可兼容正式验收脚本生成的较长基础名。
- [ ] `CMD-009` 在 `AgentEventView` 上补齐跨阶段有界状态、长期事件检索排序和冲突驱动回读。
- [ ] `CMD-010` 完成 Owner 越权、提示注入和跨会话指代歧义的关键端到端防线；不建立庞大矩阵。

## 2. 可靠事件、空窗与恢复

- [ ] `EVT-006` 完成入站批处理与可观察背压；已有有界队列、重试 Worker、幂等消费和并发关闭
  不重做，只补真实缺口。
- [ ] `EVT-007` 补 Reply 子先父后解析的跨重启场景，以及确有业务需求的非消息 Reply 路径。
- [ ] `EVT-009` 明确正文静态加密/密钥轮换/脱敏边界；继续执行
  `normal/local_only/envelope_only/never_long_term` 保存策略。
- [ ] `EVT-010` 将 NapCat HTTP 能力收敛为只读业务端口；编译边界持续禁止
  `send_group_msg`、`send_private_msg`、`group_poke` 等写接口。
- [ ] `GAP-003` 补历史多页方向、边界和完整性证据；NapCat 无法证明完整时必须保持 uncertain。
- [ ] `GAP-007` 仅评估“普通消息实时入站”的本地磁盘 Spool。Recall Spool 已完成，不得把两者
  混写为同一任务；若引入，必须先定义加密、容量、锁、恢复和健康告警。
- [ ] `GAP-008` 分别演练电脑关机/休眠、NapCat 离线和 MySQL 离线；属于需要环境配合的部分移入
  `EXTERNAL-TEST`，本地可完成的故障注入仍可先做。

## 3. 线程语义与跨会话关联

- [ ] `THR-002` 补文件版本和非 Reply 的结构化引用入口；发送者、相似话题或相同文件名不能单独
  触发跨会话关联。
- [ ] `THR-004` 完成跨线程排序检索、复杂指代与一小组真实质量样本；候选必须引用当前批次来源。
- [ ] `THR-005` 增加结论修订链的有界分页，不改变既有不可变 revision 语义。
- [ ] `THR-006` 定义有来源证据的自动结束条件；不因一段时间无人发言直接宣称事项已经解决。
- [ ] `THR-008` 完成低置信度确认话术；QQ 开放平台投递属于外部验收，不阻塞本地响应产物。
- [ ] `THR-009` 完成跨会话检索授权过滤、受限内容的既有派生状态失效与必要防泄露测试。
- [ ] `THR-010` 完成已确认语义的人工迁移/重新确认以及话题重新打开流程。

## 4. Owner 控制面与通知

- [ ] `FUP-007` 本地继续维护账号隔离、租约 fencing、重试、送达回执和 `unknown_commit`；真实
  Owner QQ 投递仅在轮换凭据后验收。
- [ ] `CMD-002` 完成 QQ 开放平台真实联机、Gateway Resume 和 Owner 回执验收；执行前必须通知
  用户并确认本地凭据，禁止写入 Git、TOML、日志或文档。
- [ ] `CMD-003` 将剩余写命令逐项接入统一 OwnerBinding、同账号、Resume 与 Effect 复验边界。
- [ ] `CMD-004` 补真实自然语言到现有类型化 Action 的关键映射质量，不新增不受约束的自由工具。
- [ ] `CMD-008` 接入线程拆分/合并的 QQ Owner 自然语言入口；后端 Suspend/Resume 不重写。

## 5. 可观测性与上线强化

- [ ] `OPS-001` 把 WebSocket、Worker、Recall Spool 和 Gap 的安全有界快照并入 Owner 状态查询。
- [ ] `OPS-002` 展示回补进度和关键失败原因；日志只允许类型化错误，不输出正文、Token、OpenID、
  数据库 URL、密钥或本地敏感路径。
- [ ] `OPS-003` 完成待处理事项与线程的分页、跨线程聚合和运行期健康详情。
- [ ] `OPS-004` 提供失败派生任务的有界重处理入口，继续使用租约、fencing 和审计。
- [ ] `OPS-005` 建立最小生产指标：入站吞吐、端到端延迟、队列/Spool backlog、LLM 调用率与成本、
  线程误关联反馈、提醒误报反馈。
- [ ] `OPS-006` 使用合成数据压测高流量群，验证背压、批处理、内存和 Token 上限；不依赖真实 QQ。
- [ ] `OPS-007` 演练 QQBot 独立数据库备份恢复、密钥轮换、数据导出和彻底删除；禁止触碰数字人库。

## 6. 外部阻塞与人工验收

- [ ] `EXTERNAL ENV-002` 轮换已经暴露的 QQ 开放平台 Secret，并通过本地环境变量或忽略文件配置。
- [ ] `EXTERNAL ENV-003` NapCat 实机确认免打扰消息、自身消息上报和一条新消息完整派生链。
- [ ] `EXTERNAL ENV-004` NapCat 双账号历史多页方向、空页原因、跨重启覆盖和 PacketBackend 兼容。
- [ ] `EXTERNAL QA-004` 如未来恢复远端发布门禁，再配置 GitHub protected Environment、受保护
  runner 的可信签名密钥以及 branch protection required check；当前不阻塞本地业务开发。
- [ ] `EXTERNAL CMD-LIVE` QQ 开放平台真实 Owner 投递和交互回执；不得使用已暴露凭据。
- [ ] `EXTERNAL OPS-LIVE` 电脑休眠/断网/NapCat 退出的实机恢复演练。

## 7. 后期延期

- [ ] `DEFERRED KB-001` 文档、网页个人知识库和向量检索。
- [ ] `DEFERRED MM-001` 图片、语音和文件内容理解。
- [ ] `DEFERRED AUTO-001` 代表 Owner 向第三方自动回复；必须重新进行权限、误发和隐私风险评审。

## 8. 已完成能力索引

下列能力已经存在，后续任务应复用而不是重新实现：可靠入站与账号幂等、Connection Epoch、Gap
回补、Recall durable WAL/MySQL inbox、Thread 投影、语义候选、跨会话关联候选、结构化记忆与来源
回读、Agenda、FollowUp、ResponseExpectation、统一通知策略、Owner Outbox、Action Graph、L2
Suspend/Resume、MySQL Checkpoint、Effect Receipt、OwnerBinding、群白名单、NapCat 只读边界、QQ
开放平台协议适配、运行期健康聚合和 QQBot 独立数据库。

准确完成时间、测试证据、迁移、外部影响和提交记录统一查询 [`HISTORY.md`](HISTORY.md) 与
[`history/`](history/)。若本索引与代码或历史证据冲突，以当前代码和可复现证据为准，并立即修正
本看板，不得为了维持勾选状态歪曲事实。
