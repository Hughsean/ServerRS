# 个人 QQ 智能秘书执行看板

> 最后整理：2026-08-06（Asia/Shanghai）
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

- 当前分支：`Main`；`GAP-003-A/B/C`、`GAP-007`、`GAP-008-LOCAL`、`THR-002`、`THR-004`、
  `THR-005`、`THR-006`、`THR-008`、`THR-009`、`THR-010` 已随各自切片提交收口；用户未
  跟踪的 `.mcp.json` 未读取或触碰。
- 当前状态：`GAP-003-A/B/C` 已完成实现与 Codex 独立复核。2026-08-06 双账号 NapCat 4.18.14
  实测确认 `reverseOrder=true` 才是向更旧读取，响应数组仍为旧到新；客户端已在协议边界归一化为
  新到旧并保持末项 continuation。账号间 cursor 不可复用也已实测。6099 单次重启后的最近历史
  可读已有局部证据；空页原因、完整跨重启分页覆盖和 PacketBackend 行为仍属于 `EXTERNAL ENV-004`，
  完成前只能有界恢复候选事件，不能完成 Scope。
- 当前评估：`GAP-007-A/B/C` 与 `GAP-007-IMPL-A/B/C/D` 已完成并通过 Codex 独立复核。普通消息
  callback 只做 bounded admission，blocking writer 在 `sync_all` 后产生 durable receipt，再进入统一
  MySQL ingestion；必需 Recall/Artifact hook 收敛后才推进连续 checkpoint。fatal 与关闭超时保留开放
  epoch/WAL 并 fail-closed，启动先按账号领取、续租、replay、checkpoint，再原子结束 epoch、创建或
  复用 uncertain Gap。健康快照只暴露有界数值与类型化错误。
- 当前架构判断：不可变 `SourceEvent`、内容信封和语义投影方向保持不变，不进行全量重写。
- 当前实机证据：`EXTERNAL ENV-003` 的 6099 自身消息上报、授权群历史回读及
  `qqbot-server -> 隔离 QQBot MySQL -> SourceEvent/正文/线程` 完整派生链已通过；测试结束后
  `reportSelfMessage=false`，随机 schema 和临时文件无残留。6099 当前 HTTP `3001`/WS `6701`
  正常；6100 重新登录后受认证 Debug adapter 可读授权群历史，但没有配置 HTTP/WS 服务。22:17
  用户把 6099 对应账号的授权群切为 `4（接收不提醒）`，6100 发送唯一标记后 6099 WebSocket
  成功收到同一消息，免打扰、自身消息上报和完整派生链三项证据闭合，ENV-003 已完成。
- 下一步：只继续上线所需外部事项；需要 QQ 开放平台新凭据、实际断网/休眠或远端管理权限的事项
  保留为 `EXTERNAL`。知识库、多模态理解和第三方自动回复已按产品决定停止，不属于当前目标。
  LLM 已新增 DeepSeek 官方 Provider，本地忽略配置已切换为 `deepseek-chat`；后续需要真实模型的
  测试只使用该 Provider，不再回退 Ollama。专用 API Key 未设置时客户端启动即 fail-closed。
  `OPS-001` 已把 WebSocket、Worker、Recall/Realtime Spool、入站和 Gap 的有界健康快照并入
  Owner 状态查询；`FUP-007` 本地送达回执、租约 fencing、重试和 `unknown_commit` 已完成；
  真实整机休眠、断网和退出 NapCat 仍留在 `EXTERNAL OPS-LIVE`。`EVT-007-NONMSG-FILE` 已由
  授权群真实上传/Reply 样本完成；`EVT-007-NONMSG-CARD` 也已由真实 Ark/JSON 卡片证明为普通
  消息 Reply 并完成历史解析收口，`EVT-009` 已按产品决策取消。
- 当前安全边界：NapCat 只读；只有绑定 Owner 的 QQ 开放平台控制消息可成为 `OwnerCommand`；
  所有第三方自动回复已按产品决定停止；群管理员只是群角色，不构成系统 Owner。
- 当前本地门禁：2026-08-06 22:03 独立重跑 `fmt`、`diff check`、workspace all-targets check、
  QQBot 五个 crate 严格 Clippy、领域 293/293、NapCat 71/71、QQBot Server 198 passed/3 ignored、
  workspace boundaries 24/24；Docker MySQL 保留的 19 个测试目标共 52/52 真实通过，所有随机
  `qqbot_accept_evt007_*` schema 均已清理。

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

- [x] `ARCH-QQBOT-002` 将 `qqbot-server/src` 的业务 Worker、协议适配器和技术实现分别迁入
  `application/`、`adapters/`、`infrastructure/`；bootstrap/config/runtime 保持组合根职责。
  QQ Open Platform 改为注入 OwnerBinding、GatewaySession 和 raw-event 端口，SeaORM SQL 实现
  下沉 infrastructure；NapCat 目录/历史映射移入 adapters；ingestion 的队列错误和健康报告改为
  应用类型/端口。架构门禁持续禁止 application 反向依赖 QQ 协议、MySQL、HTTP、文件和加密实现。

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
  第四轮修复时 GetThreadContext/ResolveReference 因缺少安全投影而移出 Replan 白名单；CMD-010
  为 ResolveReference 增加类型化来源投影与歧义 OpenReference 后安全恢复该只读工具，
  GetThreadContext 仍不进入 Replan。typed_events 为空时只输出有界计数摘要，绝不回退 raw
  summary；映射缺失时 `build_llm_views` 返回 Err（fail-closed）。
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
- [x] `CMD-009` 跨阶段有界状态、长期事件检索排序和冲突驱动回读（详见 HISTORY 2026-08-03）：
  - 目标 A：`AgentWorkingContextV1` 版本化有界工作上下文（证据/会话/线程/参与者/事实引用、
    未解决指代、冲突上下文，逐项硬上限 + 32 KiB 序列化上限 + Checkpoint JSON 持久化 +
    `serde(default)` 兼容）；状态更新只经类型化 `SecretaryAgentUpdate`；Planner 只接收有界投影，
    真实稳定 ID 不进入 LLM 输入（事件/会话/线程/参与者/事实分别映射为
    `evt_N/conv_N/thread_N/actor_N/fact_N`，未登记引用 fail-closed）；状态更新在副本上完成并
    校验后原子替换，非法或超限更新不会留下半更新状态。
  - 目标 B：`SearchRecentEvents` 保持名称兼容，新增可选 `since/until/conversation/thread/actor`
    硬过滤；未指定 since 时可检索 24 小时以前的长期事件（移除 24h 窗口限制）；
    排序确定：硬过滤 → 文本相关性（前缀 > 包含）→ occurred_at DESC → source_event_id DESC；
    LIKE `%`/`_`/转义符全部转义；始终账号隔离 + 撤回/内容策略过滤。
  - 目标 C：候选批准冲突是确定性业务结果——结构化 `MemoryCandidateConflictResultV1` 回执、
    候选保持 proposed 且版本不变、不覆盖/supersede/重放；`ReplanDecisionNode` 经
    `MemoryUseCase::evidence` 执行一次 L0 回读（账号/Confirmed 事实状态/全来源有效复验，
    远程模型拒绝 local_only 来源，任一不满足即 fail-closed），回读结果进入工作上下文并允许
    恰好一次 Replan；冲突轮 PlanNode 结构性
    allowlist（只允许 AskOwnerClarification / CorrectMemoryFact）；`load_with_sources` 排除
    已撤回来源；响应渲染有界中文冲突说明，不含内部稳定 ID。
  - 验证：领域 Graph 与 working_context 模块测试、LLM 映射测试（全部结构化引用真实 ID
    不入模、未登记 fact_ref 拒绝）、2 个 CMD-009 隔离 MySQL 场景与 8 个 Action Planner
    隔离 MySQL 场景真实通过；workspace boundaries 19/19；严格 Clippy 与格式检查通过。
- [x] `CMD-010` Owner 越权、提示注入和跨会话指代歧义防线（详见 HISTORY 2026-08-03）：
  - 目标 A：Owner 越权防线——NapCat 群主/管理员/“@Owner”/同 ID 伪指令只产生观察事件，
    `ensure_action_run` 在插入前即事务内复验 OwnerCommand 与 active binding，绝不创建 ActionRun；
    领取与 Resume 重新读取原始 SourceEvent
    （`message_role='owner_command'` + `actor_kind='owner'`）并 JOIN 当前 active OwnerBinding，
    校验完整四元组（managed account + command account + owner actor + identity kind）；
    Agenda 等写 Effect 在租约校验后复验 OwnerCommand，审批后、提交前撤销/替换 binding 一律
    拒绝，且不写审计、Receipt 与业务状态；共享授权 helper 统一各写入路径，Planner/Checkpoint
    缓存身份一概不信任。
  - 目标 B：提示注入防线——聊天正文、检索结果、Observation、昵称、群名片、历史记忆一律视为
    不可信数据，只有 `PlannerInput.command` 对应的已验证 OwnerCommand 是权威请求；注入字符串
    不提升权限；所有引用继续使用 evt_N/actor_N/conv_N/thread_N/fact_N 临时映射，未登记/
    跨账号引用 fail-closed；Observation 只增加证据，不改变 Owner 身份/风险/审批/allowlist；
    非 L0 Action 的 Proposal 必须引用本轮 OwnerCommand 的 command_event_ref，仅引用不可信
    历史事件的写 Proposal 拒绝；L2/L3 继续 Suspend；拒绝日志只记有界原因码/Action kind/
    类型化 ID，不记录正文。证据门同时位于 LLM adapter 与领域 `PlanNode`，替换 Planner 实现也
    不能绕过。
  - 目标 C：跨会话歧义防线——非显式指代（他/这个人/那件事/这条消息）只在 Owner 指定或当前
    证据所属的 conversation/thread 作用域内解析；同账号两群同昵称/同 ID 不同 identity kind
    不静默绑定；0 个或多个候选返回有界 OpenReference/澄清且不执行写 Action；只有 Owner 明确
    提供已登记 conversation_ref/thread_ref/actor_ref/event_ref 才允许精确解析；所有查询
    account scoped。`ResolveReference` 重新进入有界 Replan：唯一结果只投影类型化来源，歧义结果
    只形成 OpenReference；工作上下文存在 OpenReference 时领域层只允许 AskOwnerClarification。
  - 验证：领域引用解析与 LLM 映射测试（无作用域歧义、同 actor 不同 kind 歧义、写 Proposal
    无 command 证据拒绝、OpenReference 强制澄清等）、CMD-010 2/2、Action Planner 8/8、
    Participant/Causality 2/2 隔离 MySQL 场景真实通过，派生 schema 随机命名且 finally 清理；
    personal-secretary 248/248、qqbot-server 124/124（另 2 项 live LLM ignored）、workspace
    boundaries 19/19；严格 Clippy、格式与 diff 检查通过。

## 2. 可靠事件、空窗与恢复

- [x] `EVT-006` 入站微批处理与可观察背压（详见 HISTORY 2026-08-03 与 2026-08-04）：
  - 目标 A：有界微批 Worker——`batch_size`/`batch_flush_ms` 配置化，Worker 用 `try_recv` 快速
    填充 + 满批立即提交 + 未满等待超时；保持消息入队顺序；channel 关闭后处理残余并排空。
  - 目标 B：批量数据库事务——新增 `InboundEventStoreT::insert_messages_if_absent` 端口方法，
    MySQL 单事务处理整批，单条入口委托批入口，提取共享 `process_message_in_transaction`
    helper，账号/会话/SourceEvent/Cursor/Epoch 完整性不变；post-hook 在事务提交后执行。
  - 目标 C：poison 隔离——`InvalidData` 触发有界二分队列（≤1 条标记 invalid 并跳过，
    ＞1 条二分继续定位），暂时错误整体重试，不丢邻居，不用递归。
  - 目标 D：背压与可观察——`try_enqueue` 满时立即返回错误并聚合 overflow 计数；
    `IngestionMetrics` 原子计数器追踪 queue_depth/high_watermark/accepted/duplicates/
    invalid/retries/dropped/batches_committed/last_batch_size 等；`IngestionMetricsProducer`
    实现 `HealthSnapshotProducer` 并接入 B7 聚合器。
  - 验证：7 个 Worker 单元测试（含 1000 条合成微批、poison 二分隔离、queue_depth 水印、
    批次计数、重试恢复）全部通过；初版 MySQL 聚焦测试 1/1、CMD-010 回归 2/2 真实通过；
    增强场景真实证明数据库中途失败整批零 SourceEvent、恢复后全部 Accepted、再次重放全部
    Duplicate 且事实数不增加；随机 schema 已精确清理。常规门禁为 personal-secretary 248/248、
    qqbot-server 131 passed（2 ignored）、workspace boundaries 19/19。
- [x] `EVT-007-MSG` 消息 Reply 子事件先于父事件到达时，支持持久化、跨重启、幂等的关系解析。
  仅覆盖 NapCat 群消息与私聊消息（本切片）；不得把文件、卡片、通知等非消息 Reply 混入。
  实现收敛于 `personal-secretary-mysql`（`resolve_reply` 会话/通道校验 +
  `resolve_pending_replies_in_txn` 事务内回填与线程投影失效 + 提交后自愈 + Duplicate 父
  重放修复 + 后台修复 Worker），新增增量迁移 `20260804_qqbot_reply_reconcile.sql`
 （含候选队列表 + 结构验证 + 幂等回填；索引 inline，未分拆）。
  五轮 Codex 复核共修复 15 个 P1 + 4 个 P2：
  ① reconcile fencing：query_one_raw FOR UPDATE 复验 + fenced_clear 检查 DELETE RI=1；
  ② 终态空线程 semantic_state DELETE 先于语义派生撤消；
  ③ Relation 清理覆盖入边方向（`r.from_event_id OR r.to_event_id`）；
  ④ 终态父线程拒绝自动接纳 Reply 子事件（planner 新增 `reply_parent_thread_is_terminal`/
  `reply_child_thread_is_terminal`/`previous_thread_is_terminal` 三级终态判定）；
  ⑤ 候选队列重构（`secretary_reply_reconcile_claims` 为唯一真实候选来源，主路径同一事务
  INSERT 候选行、解析后 DELETE，reconcile claim 查询从此表出发不再扫描全部 source_events）；
  ⑥ ReconcileCandidateRow.attempts 类型修正（i64→u32 匹配 INT UNSIGNED）；
  ⑦ 迁移结构校验改为连接池无关的条件性多行标量子查询，错误列类型/索引顺序/FK 删除规则
  均产生真实 MySQL 1242 错误，迁移加载器只在整文件成功后登记 migration record；
  ⑧ 终态线程语义测试使用真实 semantic store 领取批次，Reply 解析后旧补丁提交必须 LeaseLost
  且零派生写入。
  `evt007_delayed_reply_mysql` 20 场景全绿（场景 20 同时覆盖正确迁移重放与列/索引/FK 三条
  fail-closed 负向重放）。
  所有 6 个回归套件全绿，Docker 验证完成。
- [x] `EVT-007-NONMSG-FILE` 群文件 Reply 子先父后解析。2026-08-06 在唯一授权群实测：
  `group_upload` notice 只有 `file{id,name,size,busid}`、没有稳定 `message_id`，不得伪造成
  `SourceEvent`；可引用父节点是群历史中的 `file` 段消息，其真实 `message_id` 与后续 Reply 段
  `data.id` 一致。协议层仅产生 `GroupUpload` 类型化历史信号，runtime 以有界非阻塞队列持久化
  精确会话的 `NonMessageReference` Gap；队列满/关闭经独立 fatal 通道结束 epoch，防止 transport
  吞掉 handler 错误而静默漏信号。MySQL 只在在线状态回补 signal scope，已有真实 cursor 保留、
  未见会话使用 sentinel 从最新页开始；连接结束后仍复用完整冻结 Scope。新增迁移
  `20260806_qqbot_non_message_history_signals.sql` 建立受 FK/CHECK 约束的持久化 scope ledger。
  隔离 MySQL 覆盖文件父子解析、未见会话、已有 cursor、多 scope 和未通知会话排除。
- [x] `EVT-007-NONMSG-CARD` Ark/JSON 卡片 Reply。2026-08-06 通过 6099 的受认证 Debug schema
  调用 `ArkShareGroup` 得到 Ark JSON，只向授权群发送一个 `json` 段并回复；群历史确认卡片本身
  拥有稳定 `message_id`，Reply 段 `data.id` 精确指向该 ID。因此它不是非消息 notice，也不需要
  新建 Gap/SourceEvent 模型，复用 `EVT-007-MSG` 的持久化 pending 解析。修复历史适配器重复解析
  导致 json/xml/card/forward 降级 Unknown 的缺陷，统一复用实时结构化段解析器；新增 JSON Rich
  历史单测和卡片父后到 MySQL 场景。
- [x] `EVT-009-CANCELLED` 产品决策不实施数据库消息正文静态加密、密钥轮换、历史重加密或
  密文搜索索引；不新增相关迁移、配置、Worker 和密钥依赖。现有
  `normal/local_only/envelope_only/never_long_term` 继续作为应用层内容保存与读取策略执行；
  若未来出现新的合规或部署要求，必须重新建立独立切片评估，不恢复本次未提交实现。
- [x] `EVT-010-A` NapCat HTTP action 收敛为封闭、类型化的 7 项只读白名单；任意 action、
  path、URL 参数和 OneBot 原始响应均不对消费者公开。
- [x] `EVT-010-B` 按 Capability/Directory/History 拆分最小只读能力端口；组合根负责构造
  客户端并向各消费者注入对应 trait object。
- [x] `EVT-010-C` 固化编译边界和 fake HTTP 负向测试，持续禁止 NapCat 写接口；Codex 独立
  复核后 `qqbot` 55 单元 + 10 fake HTTP + 2 heartbeat、`qqbot-server` 138/138（2 ignored）及
  workspace boundaries 21/21 通过，严格 Clippy、全工作区 check、fmt 与 diff check 通过。
- [x] `GAP-003-A` 领域/application 层固定从最新向
  更旧读取，以互斥的 `Next` / `ProvenHistoryStart` / `UnprovenStop` 替代
  `Option<Cursor>`；QQBot 公开查询只接受类型化方向，OneBot 布尔映射保持私有。
- [x] `GAP-003-B` 逐页校验账号/会话/锚点/
  continuation 和单锚点重叠连续性；只有冻结边界返回 `Duplicate` 才命中，
  边界后更旧消息不写入；请求方向不充当响应页序证据，未验证来源只能恢复候选并保持
  `Unprovable`；复用现有租约、fencing、`last_cursor` 和原子 finalize。
- [x] `GAP-003-C` NapCat 非空页只从真实末锚点
  构造 `Next`，空页/OwnerControl 只能 `UnprovenStop`，短页不终止，opaque
  `message_seq` 原样传递，身份错误与底层响应详情脱敏。Codex 独立验证：领域 262/262、
  `qqbot-server` 142 passed（2 ignored）、QQBot 55 单元 + 10 fake HTTP + 2 heartbeat、
  workspace boundaries 23/23、GAP-003 MySQL 1/1 与 EVT-007 MySQL 20/20 通过。
- [x] `GAP-007-A` 完成普通消息状态与失效模型：运行期 `DurablySpooled` 不是平台 ACK 或跨重启
  恢复证据；重启依据完整认证 WAL 帧，当前 transport 吞掉 handler 错误并继续连接。
- [x] `GAP-007-B` 完成 bounded admission、blocking writer、运行期 receipt、WAL-based recovery、
  Heartbeat 公平性、全局损坏 fail-closed、稳定 hook idempotency key、512 MiB、Windows `sync_all`
  和崩溃收敛的遗留 epoch 恢复契约。
- [x] `GAP-007-C` 决策为 **GO，Codex 独立复核通过；尚未实现**。详见
  [`gap-007-realtime-ingestion-spool-decision.md`](specs/gap-007-realtime-ingestion-spool-decision.md)。
- [x] `GAP-007-IMPL-A` 完成 typed admission/durable receipt/fatal、WAL 恢复资格、checkpoint
  eligibility 与遗留 epoch 分阶段恢复端口。Codex 复核补齐账号作用域领取、typed lease token
  fencing、受控构造与紧凑枚举布局；Recall/Artifact 稳定效果键、恢复顺序和端口传递均有单元测试。
  `personal-secretary` 270/270、严格 Clippy、all-targets check、架构边界 23/23 与 diff 检查通过。
- [x] `GAP-007-IMPL-B` 实现独立 AEAD WAL、512 MiB 总预算（按平台实际分配量计账）、独占锁、
  最终尾部截断、完整帧认证/解码 fail-closed、连续 checkpoint、compact 和 Windows 写穿替换协议；
  普通消息与 Recall 使用独立 magic/key/路径/状态，未接入 WebSocket runtime。Spool 聚焦测试 9/9、
  `qqbot-server` 151 passed/2 ignored、架构边界 24/24、严格 Clippy 与格式门禁通过。
- [x] `GAP-007-IMPL-C` 接入 reader/Heartbeat、bounded admission、blocking writer、fatal 传播、
  MySQL replay、必需 hook 收敛、连续 checkpoint、同 epoch Gap 重试、启动恢复、健康快照和有界关闭。
  MySQL recovery claim 使用账号作用域、typed token、未过期复验与续租；真实 MySQL 1/1、
  `personal-secretary` 270/270、`qqbot-server` 159 passed/2 ignored、架构边界 24/24、严格 Clippy、
  workspace all-targets check、fmt 与 diff check 通过。
- [x] `GAP-007-IMPL-D` 完成专用 OS writer 线程、bounded admission、阻塞 writer 下 Tokio timer
  公平性、receipt 前完整帧恢复、关闭 deadline detach 并保留 WAL、尾部撕裂/完整帧损坏、活动 WAL
  预算、MySQL 离线不推进 checkpoint、必需 hook 失败不推进 checkpoint，以及 WAL 创建、append、
  truncate、checkpoint、compact、原子替换后文件/父目录同步点故障注入。`personal-secretary` 270/270、
  `qqbot-server` 169 passed/2 ignored、架构边界 24/24；真实 MySQL recovery 1/1、EVT-006 1/1、
  EVT-007 20/20 通过。
- [x] `GAP-008-LOCAL` 完成本地可重复故障演练：NapCat 连接中断快速返回类型化错误且不泄露
  URL/Token；长重连退避可被 shutdown 抢占；watch false 变化不误关、sender 丢失不挂死；MySQL
  持续不可用不推进 Spool checkpoint；writer 关闭超时保留已同步 WAL；隔离 MySQL recovery
  claim/fencing/finalize 1/1 通过。真实关机/休眠、断网和 NapCat 进程退出恢复继续由
  `EXTERNAL OPS-LIVE` 承担。

## 3. 线程语义与跨会话关联

- [x] `THR-002` 完成文件版本和非 Reply 结构化引用入口。Forward 使用大小写敏感的精确源键；
  JSON/XML/Card 使用完整、未截断载荷的类型域分隔 SHA-256，正文仍只保留有界 envelope；文件
  版本必须显式携带当前键与上一版本键，并只与上一版本的精确文件身份匹配。候选始终为
  `proposed`，账号隔离不变；发送者、相似话题、相同文件名、缺少完整摘要的 Rich 哨兵均不能
  产生候选。NapCat 4.18.14 标准文件段没有版本父指针，因此生产适配器不猜测版本关系。验证：
  `personal-secretary` 273/273、QQBot 57 单元 + 10 fake HTTP + 3 heartbeat、`qqbot-server`
  175 passed/2 ignored、架构边界 24/24；THR-002 MySQL 1/1、EVT-007 回归 20/20，迁移重放、
  弱信号 CHECK、账号隔离与幂等均通过。
- [x] `THR-004` 完成跨线程排序检索、复杂指代与一小组真实质量样本。线程检索统一使用
  `secretary_effective_thread_events`，按精确 > 前缀 > 包含、线程最新事件时间与 Thread ID 确定排序；
  LIKE 通配符按字面转义，账号、撤回、缺失投影和内容策略在 SQL 候选阶段过滤。每个结果携带
  代表 `SourceEvent`、发送者、会话、时间、受控摘录和相关性等级，经 `QueryEffectTypedEvent`
  投影为请求内临时引用，不再只返回线程计数。`local_only` 是否参与候选/计数/排序由已验证
  loopback 策略下沉到 Store，远程路径不会先排序后过滤。复杂指代只识别固定的当前/上一条消息、
  回复父消息、被回复者、当前线程和线程发起人；上一条逐条回读复验当前会话，回复与发起人只使用
  已确认因果关系，缺证据继续 OpenReference。语义候选现有领域门持续要求所有来源属于当前领取
  批次。完整门禁同时修复 NapCat 本地 fake HTTP 偶发被代理接管的问题：只读客户端显式禁用代理，
  避免 loopback Token 暴露。验证：领域 277/277、QQBot 57 单元 + 10 fake HTTP + 3 heartbeat、
  `qqbot-server` 175 passed/2 ignored、架构边界 24/24；THR-004 1/1、CMD-010 2/2、参与者因果
  2/2 隔离 MySQL 回归通过，严格 Clippy、workspace all-targets check、fmt 与 diff check 全绿。
- [x] `THR-005` 完成结论修订链的有界分页，不改变既有不可变 revision 语义。新增绑定 Thread 的
  强类型游标与 `1..=50` 页面边界，按 `(created_at DESC, decision_id DESC)` 稳定 keyset 读取，返回
  confidence、supersedes、创建时间和来源事件；SQL 强制账号归属，不使用 OFFSET 或更新 revision。
  MySQL 原子重建既有线程索引为 `(thread_id, created_at, decision_id, status)`，迁移记录丢失后仍可
  重放并 fail-closed 复验精确形状。隔离 MySQL 覆盖同微秒排序、三页无重无漏、账号/游标隔离、
  首页面重放、迁移重放及读取前后行级快照不变；领域 280/280、`qqbot-server` 175 passed/2 ignored、
  架构 24/24、Action Planner 6/6、项目承诺 3/3 与 THR-005 1/1 通过。
- [x] `THR-006` 完成有来源证据的自动结束条件。自动 `resolved` 只接受显式完成、显式已解决或
  显式无需继续处理三类封闭证据；来源必须是本次已领取、正文完整且逐条匹配对应类型的事件，审计
  reason 使用固定代码并持久化 status source。规则与 LLM 提取器共用 application 层确定性派生器，
  不读取 wall clock，也不存在静默/闲置超时分支；含糊“应该解决了”、无人发言、空正文、开放问题
  或同批新增问题均保持原状态。领域 282/282、`qqbot-server` 175 passed/2 ignored、架构 24/24、
  THR-006 MySQL 1/1 与 EVT-007 20/20 通过；无迁移或 schema 变更。
- [x] `THR-008` 完成低置信度确认话术。新增只读 `ListThreadLinkCandidates` Action，MySQL
  按账号直接过滤并有界返回 `proposed` 候选；85% 以下标记为低置信度，90% 和 95% 边界使用
  统一类型化分档，但任何分档都明确要求 Owner 接受或拒绝、绝不自动合并。响应包含两侧有界来源
  摘录，`OwnerResponseDraft` 合并精确来源事件并先持久化；候选状态和通知 Outbox 均不改变，真实
  QQ 开放平台投递继续属于外部验收。领域 285/285、`qqbot-server` 176 passed/2 ignored、架构
  24/24、THR-008 MySQL 1/1、Action Planner MySQL 6/6 通过，严格 Clippy、check、fmt 与 diff
  check 全绿；无迁移或 schema 变更。
- [x] `THR-009` 完成跨会话检索授权过滤、受限内容的既有派生状态失效与必要防泄露测试。检索候选、
  因果上下文、回复父事件、参与者、按名解析、记忆候选及旧 Action/Owner 草稿均执行来源授权；
  `local_only` 仅允许显式本地模型授权，`OwnerCommand` 只保留信封。会话降级同事务失效语义、
  线程链接、记忆候选/事实、旧 Planner 租约和已持久化草稿。领域 285/285、`qqbot-server` 176
  passed/2 ignored、架构 24/24；THR-009、Action Planner、Project/Commitment、THR-004、
  THR-005、THR-008 MySQL 隔离回归通过，严格 Clippy 与 workspace check 通过；无迁移或 schema 变更。
- [x] `THR-010` 完成已确认语义的人工迁移/重新确认以及话题重新打开流程。新增
  `reconfirm_thread_semantics` 类型化 L2 Owner Action；迁移后的语义失效可在 Owner 绑定、Action
  lease 和同账号线程边界复验后写入不可变确认边界，Worker 可重新计算；Split 撤销后的空有效线程
  原子关闭并保留 Owner 状态审计。真实 THR-010 MySQL 1/1、迁移重放与既有检索/Planner/因果回归
  通过，严格 Clippy、workspace check、fmt 和架构门禁通过。

## 4. Owner 控制面与通知

- [x] `FUP-007` 本地已完成账号隔离、租约 fencing、重试、送达回执和 `unknown_commit`；真实
  Owner QQ 投递仍属于 `EXTERNAL`，仅在轮换凭据后验收。
- [x] `CMD-002` 完成 QQ 开放平台真实联机、Gateway Resume 和 Owner 回执验收；执行前必须通知
  用户并确认本地凭据，禁止写入 Git、TOML、日志或文档。2026-08-06 22:35 新凭据真实换取 Token、
  Gateway Identify 和持久会话 Resume 均成功；Owner C2C 投递返回官方 `500/11255`，按既有分类为
  `unknown_commit`，未盲目重试。用户随后向机器人发起 C2C 消息；服务使用该 Gateway 事件的权威
  消息 ID 完成被动回复，平台返回非空回执，真实交互闭环已于 22:56 完成。
- [x] `CMD-003` 已完成本地可验证的剩余写命令收口：记忆纠正、删除、TTL 修订和会话记忆模式
  统一进入专用原子 Effect 事务，在业务变更与 Receipt 之间复验 OwnerBinding、同账号、原始
  OwnerCommand、Action lease 和完整 proposal；重复 Effect、碰撞、跨账号、过期租约与 Binding
  撤销均 fail-closed。L3 `SendOwnerMessage` 的真实 QQ 投递仍属于 `CMD-002/EXTERNAL`，不在
  本地收口范围内。
- [x] `CMD-004` 已为无需实体、时间或版本解析的高频中文只读命令增加严格完整短语路由，覆盖秘书
  状态、待处理事项、近期安排、通知规则、长期记忆、待审批记忆、线程关联候选、项目与承诺。
  所有结果仍生成现有类型化 Proposal 并经过 Action Graph/白名单/Receipt；混合写指令、带目标
  查询、SQL/发送意图、Replan 和已有工作上下文不命中确定性路由，继续交由受约束 Planner 或澄清。
- [x] `CMD-008` 已接入线程拆分/合并的 QQ Owner 自然语言入口：Planner 只接受已登记的
  `thread_ref`/`event_ref`，L2 Gate 继续复用现有 Suspend/Resume；Effect 阶段由线程变更
  Store 重新读取完整影响预览，并在同一 OwnerBinding、账号和幂等 Effect 边界内提交。

## 5. 可观测性与上线强化

- [x] `LLM-001` 新增 DeepSeek 官方 API Provider。`provider=deepseek` 固定
  `https://api.deepseek.com/v1`，拒绝自定义端点与 Ollama 专用 `qwen_no_think`；密钥只读取
  `QQBOT_DEEPSEEK_API_KEY` 或本地 `api_key_file`，缺失时 fail-closed。现有
  `openai_compatible`/Ollama 配置保持兼容；DeepSeek 仍复用既有输入、输出 Token、响应字节、超时、
  JSON 结构化边界和无工具策略。本地配置已切换到 `deepseek-chat`，真实 API 调用等待用户设置专用
  密钥后执行；QQBot Server 203 passed/3 ignored、架构 24/24、严格 Clippy 与 workspace check 通过。
- [x] `OPS-001` 把 WebSocket、Worker、Recall/Realtime Spool、入站指标和 Gap 的安全有界快照
  并入 Owner 状态查询；仅展示固定子系统名、四态状态和有界数值，不暴露账号、epoch、路径、
  正文或凭据。
- [x] `OPS-002` 展示回补进度和关键失败原因；健康采样按托管账号区分
  `uncertain/backfilling/unrecoverable` Gap，输出活跃回补的页数、事件、Accepted、Duplicate、
  anomaly 和预算耗尽计数；`failure_class/reason` 只映射为固定错误码，未知值统一为
  `backfill_failure_unknown`。日志和 Owner 查询不输出正文、Token、OpenID、数据库 URL、密钥或
  本地敏感路径；Owner 查询继续只读取 HealthAggregator 缓存，不额外触发 SQL。
- [x] `OPS-003` 完成待处理事项与线程的 keyset 分页和跨线程聚合展示；分页严格绑定账号、
  查询和固定排序键，Owner 工具只接收本轮 `cursor_N` 临时引用（真实游标留在服务端），健康详情沿用 OPS-001/OPS-002 的有界
  HealthAggregator 快照。MySQL 隔离回归覆盖三页无重复/遗漏、同时间/相关性稳定排序、NULL 到期、
  账号隔离和查询游标错配拒绝。
- [x] `OPS-004` 提供失败 Artifact 派生任务的有界 Owner 重处理入口：L2 Action 只接收
  `limit=1..=100` 与有界 reason，Effect 事务内复验托管账号、OwnerCommand、Action run、
  未过期 lease token 和完整 proposal；按稳定顺序锁定本账号最旧失败任务并重排为 pending，
  同事务写入精确目标集合的不可变审计与幂等 Effect Receipt。重复 Effect 不重复重排，伪造/
  过期租约、跨账号目标和无效预算 fail-closed；真实隔离 MySQL 覆盖有界顺序、账号隔离、
  fencing、幂等、迁移重放和审计原子性。
- [x] `OPS-005` 建立最小生产指标：复用固定无标签 `HealthAggregator` 快照，暴露入站累计入队/
  提交量、MySQL commit 端到端延迟、队列及 Recall/Realtime Spool backlog；所有 LLM 消费者共享
  调用成功/失败、Token、usage 缺失和延迟计数，只有同时配置输入/输出每百万 Token 微美元单价时
  才估算成本，未配置不伪造价格。反馈指标按托管账号统计 Owner 已批准且成功应用的 split 结构
  纠错与明确 `important=false` 的提醒反馈；merge、拒绝/未完成 split 和普通反馈均不误算。
- [x] `OPS-006` 使用 20,000 条合成突发消息验证高流量群边界：首次 await 前容量 512 的队列
  精确接收 512 条，其余 19,488 条同步返回明确背压；已接收消息以 8 个 64 条事务批次全部排空，
  单批不越界，queue depth/in-flight 最终归零。LLM 聚焦测试同时证明超限输入在网络请求前
  fail-closed，配置的输出 Token 上限进入客户端请求边界；不依赖真实 QQ、NapCat、MySQL 或模型。
- [x] `OPS-007` 完成 QQBot 独立数据库与 Spool 密钥轮换演练。脚本强制使用
  `serverrs-qqbot-mysql` 和随机 `qqbot_accept_ops007_*` schema：Baseline+增量加载后执行
  `mysqldump --single-transaction --hex-blob`、异名恢复及 84 个对象/规范数据比对；按账号导出
  JSONL 后删除目标账号，扫描所有显式账号引用并确认正文级联清除、控制账号不受影响。Recall 与
  Realtime Spool 证明旧代文件存在时换钥 fail-closed，只有服务停机、pending/quarantine 均为零并
  安全退役旧代文件后才能启用新钥。演练未触碰数字人库、现有 QQBot 业务库或真实数据。

## 6. 外部阻塞与人工验收

- [x] `EXTERNAL ENV-002` 已轮换暴露的 QQ 开放平台 Secret，并写入本地忽略 `.env`；非敏感结构检查
  确认 App ID、Secret、Owner OpenID 均非空，真实 Token 获取与 Gateway Identify/Resume 进一步
  证明新凭据有效。凭据值未输出、未写入 TOML、Git、日志或文档。
- [x] `EXTERNAL ENV-003` NapCat 实机确认免打扰消息、自身消息上报和一条新消息完整派生链。
  2026-08-06 先完成自身消息与派生链：6099 临时开启 `reportSelfMessage` 后在唯一授权群发送消息，WebSocket
  收到自身事件且历史回读成功；随机隔离 QQBot schema 中 SourceEvent、正文投影和线程成员均落库，
  随后完整恢复配置并清理 schema/临时文件。20:30 对 6099 执行真实进程重启
  后账号未自动恢复登录；20:56 用户在 WebUI 完成登录确认，OneBot HTTP `3001`、WS `6701` 和
  授权群历史读取已恢复。22:17 用户在 QQ 客户端把 6099 对应账号的授权群切为
  `4（接收不提醒）`；权威状态读取确认后，Codex 先连接 6099 WebSocket，再由 6100 只向授权群
  发送唯一标记，发送成功且 6099 收到同一群消息。测试消息按授权保留，ENV-003 完成。
- [ ] `EXTERNAL ENV-004` NapCat 双账号历史多页方向、空页原因、跨重启覆盖和
  PacketBackend 兼容。双账号 NapCat 4.18.14 已确认向旧方向必须使用 `reverseOrder=true`、返回数组
  为旧到新且 cursor 受账号主体约束；空页语义、完整跨重启分页覆盖和 PacketBackend 行为尚未验证，
  不得由本次方向证据或 Fake/HTTP 测试推导为已完成。2026-08-06 续读两实例各 4 页
  `count=10`：原始页首 opaque `message_seq` 可连续推进且页序旧到新；跨账号复用 cursor 返回
  retcode 200。短页后仍返回 1 条锚点重叠，不能解释为历史终点；`nc_get_packet_status` 两实例均
  返回 retcode 400/空数据。6099 重启前后授权群最近 10 条历史摘要一致，且重启后
  `online=true/good=true`；但 20:30 再次执行 `Process/Restart` 后未自动登录，说明重启恢复不稳定；
  20:56 用户手动确认后业务端口和授权群历史读取恢复。
  6100 继续以 opaque cursor 读取到页计数 `10,10,10,10,8,1` 后停在包含式锚点且 cursor 不再推进，
  没有出现可解释空页。两实例 `packetBackend=auto`、`packetServer` 为空，
  `nc_get_packet_status` 均为 failed/400/null。21:24 在两个账号重新登录后重跑：6099 页计数为
  `10,10,10,10,10,5,1`，6100 为 `10,10,10,10,10,6,1`，两边均因包含式锚点不再推进而
  `no_progress`，没有可解释空页。交叉 cursor 双向均返回目标账号视图中的 1 条非同锚点结果，不能
  作为 continuation 安全复用，应用层账号绑定保持不变。两实例都明确报告当前 QQ
  `9.9.33-51802-x64` 与 NapCat `v4.18.14` 的 PacketBackend 不兼容。空页原因、稳定自动重登和
  PacketBackend 兼容仍无正向证据，生产继续以 `UnprovenStop`/uncertain fail-closed，本项保持未完成。
- [x] `EXTERNAL QA-004-CANCELLED` 远端 protected Environment、受保护 runner 签名密钥和 branch
  protection required check 属于未来发布治理，按产品决定停止，不属于当前上线收尾目标。
- [x] `EXTERNAL CMD-LIVE` QQ 开放平台真实 Owner 投递和交互回执；新凭据已用于 Token、Gateway
  Identify/Resume 和真实 C2C 验收。首次主动投递返回 `500/11255`/`unknown_commit`，未自动重发；
  用户随后向机器人发起消息，基于该 Gateway 事件权威消息 ID 的被动回复成功并取得非空平台回执。
- [ ] `EXTERNAL OPS-LIVE` 电脑休眠/断网/NapCat 退出的实机恢复演练。NapCat 进程重启已真实证明
  WebUI 与业务端口分阶段恢复且 6099 本次需要用户手动确认登录后才恢复；电脑休眠与物理网络断开
  仍需用户参与，不能以进程重启代替。

## 7. 产品范围外

- [x] `DEFERRED KB-001-CANCELLED` 文档、网页个人知识库和向量检索按 2026-08-06 产品决定停止，
  不属于当前 QQBot 收尾目标；不新增领域、仓储、迁移、配置或 Worker。
- [x] `DEFERRED MM-001-CANCELLED` 图片、语音和文件内容理解按同一决定停止。
- [x] `DEFERRED AUTO-001-CANCELLED` 代表 Owner 向第三方自动回复按同一决定停止；现有安全边界
  继续禁止第三方自动发送。

## 8. 已完成能力索引

下列能力已经存在，后续任务应复用而不是重新实现：可靠入站与账号幂等、Connection Epoch、Gap
回补、Recall durable WAL/MySQL inbox、Thread 投影、语义候选、跨会话关联候选、结构化记忆与来源
回读、Agenda、FollowUp、ResponseExpectation、统一通知策略、Owner Outbox、Action Graph、L2
Suspend/Resume、MySQL Checkpoint、Effect Receipt、OwnerBinding、群白名单、NapCat 只读边界、QQ
开放平台协议适配、运行期健康聚合和 QQBot 独立数据库。

架构边界已按洋葱依赖方向拆分：`personal-secretary/src/domain` 保存领域模型与规则，
`personal-secretary/src/application` 保存用例与端口，`personal-secretary-mysql` 独立实现 SeaORM/MySQL
适配器和集成测试；`qqbot-server/src/application` 保存运行编排 Worker，`adapters` 保存 NapCat/QQ
开放平台映射，`infrastructure` 保存 LLM、健康、Recall WAL 与 QQ Open Platform SQL 实现；
`tools/architecture-tests` 负责全 workspace 静态架构门禁。内层禁止反向依赖 MySQL、SQLx、QQ
协议或具体基础设施类型。

测试资产已完成一轮保守去重：`mysql_action_planner.rs` 删除被更强 Replan/重启场景覆盖的两条旧
happy path，保留 6 条账号隔离、租约、事务、Suspend/Resume、CAS 与多轮 Replan MySQL 证据；
NapCat 协议测试和近期 CMD-009/CMD-010/项目承诺/EVT-006 测试继续保留。

准确完成时间、测试证据、迁移、外部影响和提交记录统一查询 [`HISTORY.md`](HISTORY.md) 与
[`history/`](history/)。若本索引与代码或历史证据冲突，以当前代码和可复现证据为准，并立即修正
本看板，不得为了维持勾选状态歪曲事实。
