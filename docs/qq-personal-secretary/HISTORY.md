# 个人 QQ 智能秘书开发历史索引

> 本文件只做导航和当前阶段摘要；具体事件进入 `history/` 归档。
> 新事件必须精确到 `YYYY-MM-DD HH:mm（Asia/Shanghai）`，缺少可信分钟的旧事件不得猜测回填。

## 当前阶段

- **本轮（OPS-007，已完成）**：新增 fail-closed 运维演练脚本，只允许专用 QQBot MySQL 容器与
  随机 `qqbot_accept_ops007_*` schema。真实完成 Baseline/增量加载、单事务备份、异名恢复、
  84 个对象与规范数据比对、单账号 JSONL 导出、账号彻底删除及所有显式账号引用残留扫描；
  控制账号保持不变。Recall/Realtime Spool 的空 backlog 换钥各 1/1，旧代文件未退役时新钥
  必须拒绝打开。所有随机 schema 与临时文件均已清理，数字人库和现有 QQBot 业务库未触碰。

- **本轮（CMD-003，已完成）**：记忆纠正、删除、TTL 修订和会话记忆模式四类剩余 Owner 写
  Action 改由专用 MySQL Effect 事务执行；事务内锁定托管账号，复验未过期 Action lease、
  原始 OwnerCommand、唯一 active OwnerBinding、完整 proposal 与目标账号，再原子提交业务
  变更和幂等 Receipt。会话模式失效时排除当前运行，避免把正在执行的 Owner Effect 自身撤销。
  新增隔离 MySQL CMD-003 场景覆盖四类成功写入、重复回放、Effect 碰撞、跨账号、过期租约、
  Binding 撤销及无副作用；真实场景 1/1 通过。L3 `SendOwnerMessage` 仍等待 CMD-002 的外部
  QQ 投递验收。

- **本轮（OPS-006，已完成）**：新增无需真实 QQ/NapCat/MySQL/模型的确定性合成负载门禁。
  20,000 条消息在首次 await 前突发灌入容量 512 的 callback 队列，精确收敛为 512 条入队和
  19,488 条明确背压；Worker 以 8 个 64 条事务批次排空，最大批次不越界，queue depth 与
  in-flight 回到零。LLM 客户端验证超限输入在网络请求前拒绝，并保留配置的输出 Token 上限。

- **本轮（OPS-005，已完成）**：现有 `HealthAggregator` 新增最小生产指标，不建立平行监控体系。
  入站累计记录入队/提交量及从进队到 MySQL commit 的 count/sum/max/last 延迟，累计计数供外部按
  时间窗口差分为吞吐率；既有队列、Recall/Realtime Spool backlog 保持同一快照。所有 LLM
  消费者共享调用、成功/失败、Token、usage 缺失和延迟指标；只有显式成对配置输入/输出单价时
  才输出微美元估算成本。反馈按托管账号只统计已批准并成功应用的 split 结构纠错和
  `important=false` 通知反馈。Docker MySQL 账号隔离 1/1、qqbot-server 189 passed/3 ignored，
  受影响严格 Clippy、workspace check、领域与架构回归均通过。

- **本轮（OPS-004，已完成）**：新增 `retry_failed_artifact_derivations` 类型化 L2 Action，
  Owner 只能提交 1..=100 的批量预算和有界原因，不能指定事件、账号或任意过滤条件。MySQL
  Effect 在单事务内复验托管账号、OwnerCommand、Action run、未过期租约和完整 proposal，按
  `updated_at/source_event_id` 稳定锁定本账号最旧失败任务并重排为 pending，同时写入精确目标
  集合的不可变审计与幂等 Receipt。真实隔离 MySQL 1/1 及 CMD-009 2/2、CMD-010 2/2、
  Action Planner 6/6 回归通过；并修正一条与来源撤回后事实 fail-closed 契约冲突的旧测试断言。

- **本轮（OPS-003，已完成）**：待处理 Owner 工作和跨线程搜索新增稳定 keyset 分页，分别按
  `due_at IS NULL → due_at → source_kind → source_id` 与
  `match_rank DESC → latest_at DESC → thread_id ASC` 排序；游标字段私有且反序列化复验，
  线程游标绑定规范化查询，Owner/账号过滤先于排序和分页。工具观察只暴露本轮 `cursor_N` 临时引用，
  真实 keyset 字段留在服务端，不把稳定线程 ID 放入模型摘要。真实隔离 MySQL 覆盖三页无重漏、同键稳定排序、NULL 到期和
  账号隔离；领域 290/290、qqbot-server 183 passed/2 ignored、严格 Clippy 通过。

- **本轮（OPS-002，已完成）**：健康 Worker 现在按托管账号采样 `uncertain`、`backfilling`、
  `unrecoverable` Gap，以及活跃回补运行的页数、事件、Accepted、Duplicate、anomaly 和预算耗尽
  计数；最近失败原因只经过固定 allowlist 映射，未知文本统一为 `backfill_failure_unknown`，采样
  失败为 `backfill_sample_failed`。Owner 状态仍读取缓存快照，不触发额外 SQL；未新增迁移。
  `qqbot-server` 健康单测 4/4、严格 Clippy 与格式检查通过，Docker MySQL 聚合 SQL 只读验证通过。

- 主干分支：`Main`；GAP-003-A/B/C 独立提交 `7278677` 及其前置 QQBot 开发线已收口，尚未
  推送远端。QQBot 运行数据库使用独立容器、独立数据库和
  独立持久化卷，不复用数字人数据库。
- 当前开发分支：`Main`。
  2026-08-03 已完成 QQBot Schema Baseline v1、项目/承诺记忆闭环、旧测试基础设施清理、
  CMD-009（跨阶段有界状态、长期事件检索排序、冲突驱动回读）、CMD-010（Owner 越权、
  提示注入与跨会话指代歧义防线）与 EVT-006（入站微批处理、可观察背压）。
- **本轮（EVT-007-MSG）**：NapCat 群/私聊消息 Reply 子先父后解析已完成五轮复核。持久化
  unresolved 候选、事务内父子回填、Duplicate/Backfill 共用幂等入口、后台 reconciliation、
  线程投影与语义租约 fencing、终态线程边界及迁移 fail-closed 均已闭合；非消息 Reply 继续
  拆分为等待真实样本的 `EVT-007-NONMSG`。Docker 隔离 MySQL 20/20 与常规门禁全绿，随本提交收口。
- **本轮（EVT-010-A/B/C，未提交）**：NapCat HTTP 客户端收敛为 7 项封闭、类型化的只读
  action 白名单，公开能力拆为 Capability/Directory/History 三个最小端口；任意 action/path、
  OneBot 原始响应和旧 `NapCatApiClient` 均不再进入公共 API。fake HTTP 覆盖全部 action、参数、
  1 MiB 流式限流、超时与错误脱敏，架构测试约束消费者最小能力和写 action 禁区。Codex 独立
  复核修复私有 action 枚举的严格 Clippy 告警后，全部受影响门禁通过；无数据库或外部系统操作。
- **GAP-003-A/B/C（已提交 `7278677`）**：回补契约改为固定“新到旧”与
  `Next` / `ProvenHistoryStart` / `UnprovenStop` 三态证据；用例在写入前校验账号、
  会话、锚点、单页唯一性、单锚点重叠和 continuation，冻结边界只以幂等入口
  `Duplicate` 为到达证据且不写入页内更旧消息。Codex 复核进一步分离请求方向与响应页序
  证据：NapCat 在外部验证前可有界恢复候选，但不能完成 Scope 或据页内位置跳过事件。
  NapCat 空页与 OwnerControl 只产生 `UnprovenStop`，错误详情脱敏。无数据库迁移；Rust 门禁、
  GAP-003 MySQL 1/1 与 EVT-007 MySQL 20/20 通过。2026-08-06 双账号 NapCat 4.18.14 实测推翻
  原布尔映射：`reverseOrder=true` 才向更旧读取，响应仍按旧到新返回；客户端已在私有协议边界
  校正映射并归一化页序，账号间 cursor 不可复用。空页、跨重启与 PacketBackend 仍待验证。
- **本轮（GAP-007-A/B/C，已通过）**：普通消息本地磁盘 Spool 的架构决策经三轮 Codex 复核收口。
  receipt 只驱动运行期 replay，启动恢复以完整认证 WAL 帧为准；pending Gap 不能只驻留内存。
  遗留 `connected` epoch 必须保持可写完成 replay、hook 收敛与耐久 checkpoint，再由 MySQL 事务结束
  epoch、创建 Gap 和冻结证据；文件与数据库之间采用崩溃收敛协议，不宣称跨资源原子。遗留
  `connecting` epoch 有帧时 fail-closed。A/B/C 已完成，建立未完成的 IMPL-A/B/C/D；本轮只有 Markdown。
- **本轮（GAP-007-IMPL-A/B/C/D，已完成）**：普通消息 Spool 已完成领域契约、独立 AEAD WAL、
  runtime/health、MySQL recovery 与故障注入闭环。IMPL-D 将不可取消的文件同步移入专用 OS writer
  线程，关闭超时仅 detach 并保留 WAL；同步点、预算、MySQL 离线与必需 hook 失败均验证不越过
  checkpoint。真实 MySQL recovery 1/1、EVT-006 1/1、EVT-007 20/20 通过。
- **本轮（GAP-008-LOCAL，已完成）**：补齐可自动化的离线与关闭演练。NapCat 传输中断在有界
  时间内返回脱敏连接错误，重连退避可被 shutdown 立即抢占；watch 关闭通道忽略 false 变化并在
  sender 丢失时结束。既有故障注入复验 MySQL 离线不推进 checkpoint、writer 超时保留已同步 WAL。
  隔离 MySQL recovery claim/fencing/finalize 1/1 通过。完整门禁还发现并修复 fake HTTP 未声明
  `Connection: close` 导致的并发能力探测不确定性。真实整机休眠、断网和 NapCat 进程退出仍由
  `EXTERNAL OPS-LIVE` 验收。
- **本轮（THR-009，已完成）**：跨会话检索在 SQL 候选阶段执行来源授权过滤，受限来源不能通过按
  ID、因果、参与者、按名解析、记忆候选或旧 Action/Owner 草稿旁路读取正文。`local_only` 仅对显式
  授权的本地模型开放；会话降级在同一事务内失效语义、线程链接、记忆派生、Planner 租约和草稿。
  领域 285/285、`qqbot-server` 176 passed/2 ignored、架构 24/24，THR-009 及既有 MySQL 回归
  通过；无迁移或 schema 变更。
- **本轮（FUP-007，本地部分已完成）**：FollowUp 通知 Outbox 的领取按 managed account
  隔离，送达/失败回执同时复验 notification、lease token、`claimed` 状态和未过期租约；
  Retryable 使用有界指数退避，Permanent/UnknownCommit 分别进入终态，正确回执保存平台消息
  ID，过期租约由后续领取收敛为 `unknown_commit`。新增真实隔离 MySQL 状态机测试 1/1，
  Project/Commitment 回归 3/3；领域 286/286、服务器 177 passed/2 ignored、架构 24/24、
  workspace check、严格 Clippy、fmt 和 diff check 全绿。真实 Owner QQ 投递仍是 EXTERNAL，
  未伪造联机验收。
- **本轮（OPS-001，本地已完成）**：`get_secretary_status` 现在读取与运行时相同的有界
  `HealthAggregator` 快照，追加 WebSocket、Worker、历史 Gap、Recall/Realtime Spool、入站
  指标和 MySQL 子系统状态。输出只允许固定名称、四态状态、类型化错误码和有界数值，健康快照
  中的账号/epoch 字段不会进入 Owner 文本；未启用健康 Worker 时仍保守显示不确定。新增脱敏、
  有界输出单测，workspace check、严格 Clippy、fmt 和架构门禁保持通过。
- **本轮（CMD-008，本地已完成）**：线程拆分/合并接入统一 Action Planner。Planner 只恢复已登记
  的临时线程/事件引用；L2 Gate 继续 Suspend，Resume 后由线程变更 Store 重新构造完整影响预览，
  复验 OwnerBinding、托管账号和现有幂等 Effect，再执行 Merge/Split。新增 Planner/UseCase 单测，
  THR-010 真实 MySQL 回归 1/1 通过；未连接 QQ 开放平台或发送外部消息。
- **本轮（THR-010，已完成）**：线程逻辑迁移后的旧语义不再永久失效；新增
  `reconfirm_thread_semantics` 类型化 L2 Owner Action 与不可变重新确认边界，事务内复验 OwnerBinding、
  Action lease 和账号/线程归属后清除语义状态，允许 Worker 重新计算。Split 撤销在同一事务内关闭
  无物理成员且无 active overlay 的空有效线程，并写入 Owner 状态历史；终态与并发边界保持 CAS。
  迁移包含结构 fail-closed 校验并可安全重放。领域 286/286、`qqbot-server` 177 passed/2 ignored、
  架构 24/24、THR-010 MySQL 1/1 及 Action Planner、THR-004、THR-005、THR-009、Participant
  Causality 回归通过；严格 Clippy、workspace check、fmt 与 diff check 全绿。
- **本轮（THR-002，已完成）**：跨会话候选新增三类强证据：显式文件版本、精确 Forward 引用和
  完整 Rich 载荷摘要。文件版本采用当前文件键指向上一版本键的显式关系，不从文件名、发送者或
  相似正文推断；Forward 键大小写敏感，Rich 摘要覆盖未截断载荷并按 JSON/XML/Card 域分隔。
  MySQL 只允许五类强 signal，文件版本提示与上一版本精确文件提示跨类型匹配，结果仍只到
  `proposed`。NapCat 4.18.14 标准文件段未提供版本父指针，因此适配器 fail-closed，不猜测。
  THR-002 隔离 MySQL 1/1、EVT-007 回归 20/20 及完整 Rust/架构门禁通过。
- **本轮（THR-004，已完成）**：跨线程检索改为有效线程投影和类型化代表来源，相关性按精确、
  前缀、包含分级，再以线程最新事件时间和 Thread ID 稳定排序；LIKE 字面转义，账号、撤回、缺失正文
  与内容策略在 SQL 候选阶段过滤。远程模型的候选、计数和排序完全排除 `local_only`，只有已验证
  loopback 且策略允许时才在 Store 层纳入。SearchEventThreads 现在向 Replan 提供 typed events，
  不再只返回计数。复杂指代增加当前/上一条、回复父消息/被回复者、当前线程/线程发起人的有界
  确定性解析，缺少会话或权威因果关系仍 fail-closed 澄清。真实质量样本与隔离 MySQL 覆盖排序、
  字面通配符、账号/隐私隔离及 merge/split 有效投影。
- **本轮（THR-005，已完成）**：结论修订链新增协议无关的有界 keyset 分页，游标绑定 Thread 并
  校验反序列化字段，结果返回强类型 Decision ID、置信度、显式 supersedes、微秒创建时间和来源。
  MySQL 按账号归属与 `(created_at DESC, decision_id DESC)` 读取，不使用 OFFSET、不改写 revision；
  既有线程索引被原子重建为可重放的分页前缀。隔离 MySQL 证明同微秒稳定排序、三页无重无漏、
  跨账号/跨线程 fail-closed、迁移重放和读取前后修订快照完全一致。
- **本轮（THR-006，已完成）**：自动 `resolved` 收敛为三类显式来源证据：明确完成、明确已解决、
  明确无需继续处理。领域门逐条复验来源属于领取批次、正文未省略、类型与固定 reason 一致；开放
  问题或同批新增问题阻断结束。application 在任意语义提取器之后运行同一确定性派生器，不读取时间
  或静默状态，含糊表达不会结束线程。MySQL 事务沿用既有 status history/source 审计，无新迁移。
- **本轮（CMD-009）**：`AgentWorkingContextV1` 版本化有界工作上下文（引用/开放指代/冲突
  上下文，硬上限 + 32 KiB 序列化上限 + Checkpoint JSON 持久化 + 旧 Checkpoint 兼容）；
  `SearchRecentEvents` 扩展可选时间窗/会话/线程/Actor 硬过滤并移除 24 小时窗口限制，
  排序确定（硬过滤 → 相关性前缀>包含 → 时间 → source_event_id，LIKE 全转义）；
  记忆候选批准冲突改为结构化 `MemoryCandidateConflictResultV1` 回执 → 恰好一次 L0 回读 →
  冲突轮 allowlist（AskOwnerClarification/CorrectMemoryFact），不覆盖/supersede/重放。
  Codex 复核发现并修复冲突路由方向、local_only 冲突回读、结构化引用未完整投影、旧自由文本
  反向泄露与半更新状态问题；聚焦 MySQL 复验通过，随本提交收口。
- **本轮（CMD-010）**：Owner 越权、提示注入与跨会话指代歧义三条防线。A：只有已验证的 QQ
  开放平台 OwnerCommand 能创建/领取/恢复 ActionRun 与执行写 Effect —— 最终事务重新读取原始
  SourceEvent（`message_role='owner_command'` + `actor_kind='owner'`）并 JOIN 当前 active
  OwnerBinding 校验完整四元组（managed + command + owner actor + identity kind），共享
  `owner_authorization::verify_owner_command` helper，审批后、提交前撤销/替换 binding 整体
  拒绝且零副作用；NapCat 群主/管理员/“@Owner”/同 ID 只产生观察事件。B：聊天正文、检索结果、
  Observation、昵称、群名片、历史记忆均为不可信数据，只有 `PlannerInput.command` 对应的
  已验证 OwnerCommand 是权威请求；临时引用继续 fail-closed，非 L0 写 Proposal 必须引用
  本轮 OwnerCommand 的 `command_event_ref`。C：非显式指代只在显式/当前证据所属
  conversation/thread 作用域内解析，0 或多个候选返回有界 OpenReference/澄清，绝不静默跨群
  绑定；显式 `conversation_ref/thread_ref/actor_ref/event_ref` 才允许精确解析；查询一律
  account scoped。
  Codex 复核进一步把 Run 创建授权和写 Proposal 证据门下沉到 MySQL/领域边界，恢复
  ResolveReference 的类型化 Replan/OpenReference 澄清闭环，并将 Agenda 数据库失败精确分类为
  UnknownCommit；聚焦 MySQL 主路径与测试 schema 清理均已复验通过。
- 当前能力：可靠入站、空窗回补、确定性 EventThread、类型化语义、跨会话关联候选、Owner
  关联审核、高影响线程变更的持久化 Suspend/Resume、授权撤销、语义失效，以及来源化人物/
  项目/承诺结构记忆、证据回读、Owner 派生记忆删除、承诺提醒 Outbox、独立 QQ 开放平台
  协议适配、类型化 Agent 动作策略门、可选 OpenAI-compatible/Ollama 有界线程语义提取、
  并发优雅关闭（RuntimeWorkers + 25s 全局 deadline）、可编程运行时入口（run_with_cancellation）、
  群白名单过滤、Owner Retriever、受约束 Action Planner、真实 Effect Receipt、响应产物，
  以及 Action Run 的持久化 Suspend/Resume CAS 闭环。
  **新增（本轮）**：协议无关 AgendaItem/Mutation、创建/查询/改期/稍后提醒/完成/取消 Action、
  L2 Owner 审批、不可变审计、版本 fencing、到期 Scheduler 和复用的 Owner-only Outbox。
- 当前边界：NapCat 保持只读；旧验收矩阵、attestation 门禁与仓库内真实 QQ/NapCat 人工验收
  测试已经删除，历史文档只记录其曾经发生，不再提供可执行入口。结构化记忆候选
  (MEM-011) 已提交（`94ef5d9`）。Agent 有界事件证据视图 v1 已完成：有界最近窗口与 retrieved
  真实入模，模型仅看到请求内临时引用，local_only 只向已验证 loopback 模型开放；关键 MySQL
  关系视图用例已在随机隔离 schema 通过。CTX-004 的生产路径 P0/P1 已闭合，全 Graph 闭环集成测试、
  隐私视图测试及随机隔离 MySQL 主路径测试均已完成；切片进入提交候选。
  **数据库基线（本轮）**：QQBot 空库使用 78 表 + 2 View 的 Baseline v1；压缩前 33 个迁移保留在
  QQBot 自有归档中但不再参与加载，数字人数据库与 `init.sql` 未改动。
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

- `2026-08-06 03:37（Asia/Shanghai）`：`THR-008` 完成。新增只读线程关联候选 Action，MySQL
  按账号直接过滤并有界展示 `proposed`；置信度分档只控制“低置信度/中等置信度/强证据”确认话术，所有
  候选都明确要求 Owner 接受或拒绝且不自动合并。响应草稿携带两侧有界来源摘录和精确来源事件，
  在本地 MySQL 持久化完成后即构成本切片产物，不依赖 QQ 开放平台投递。领域 285/285、
  `qqbot-server` 176 passed/2 ignored、架构 24/24；THR-008 1/1 与 Action Planner 6/6 隔离
  MySQL 通过，严格 Clippy、workspace check、fmt 与 diff check 全绿。
- `2026-08-06 03:18（Asia/Shanghai）`：`THR-006` 完成。新增封闭的
  `ThreadResolutionEvidenceKind` 与显式文本分类器，自动解决只对本批完整来源事件生成
  `open/waiting/reopened -> resolved`；OwnerCommand 产生 owner authority，其他明确陈述为
  evidence-derived。开放问题、含糊“应该解决了”和静默时间均不触发。领域 282/282、
  `qqbot-server` 175 passed/2 ignored、架构 24/24；THR-006 1/1、EVT-007 20/20 隔离 MySQL
  通过，严格 Clippy、workspace check、fmt 与 diff check 全绿。
- `2026-08-06 02:53（Asia/Shanghai）`：`THR-005` 完成。新增绑定 Thread 的强类型修订游标与
  `1..=50` 页面边界；MySQL 以 `(created_at, decision_id)` 逆序 keyset 分页并强制账号归属，返回
  confidence、supersedes、创建时间和来源。迁移原子重建既有索引为
  `(thread_id, created_at, decision_id, status)`，删除迁移记录后重放成功。领域 280/280、
  `qqbot-server` 175 passed/2 ignored、架构 24/24；THR-005 1/1、Action Planner 6/6、项目承诺
  3/3 隔离 MySQL 回归通过，严格 Clippy、workspace check、fmt 与 diff check 全绿。
- `2026-08-06 02:16（Asia/Shanghai）`：`THR-004` 完成。`ThreadSearchResult` 增加代表事件、发送者、
  会话、代表时间、内容策略与封闭相关性等级；MySQL 以有效线程 View 检索，远程/local loopback
  内容边界直接参与 SQL 候选和排序。`SearchEventThreads` 形成 typed observation。复杂指代只对
  固定中文质量样本走当前运行窗口或已确认 causal context，其他表达仍走作用域候选并在零/多解时
  澄清。领域 277/277、QQBot 57 + 10 + 3、`qqbot-server` 175 passed/2 ignored、架构 24/24；
  THR-004 1/1、CMD-010 2/2、参与者因果 2/2 隔离 MySQL 回归通过。
- `2026-08-06 01:51（Asia/Shanghai）`：`THR-002` 完成。领域新增显式文件版本引用，关联提取器
  支持精确 Forward 与完整 Rich 内容摘要；NapCat 富消息在协议边界计算未截断载荷摘要，缺少摘要
  时继续保留 Artifact，但不形成强关联。MySQL 增量迁移扩展强信号 CHECK，文件版本通过跨类型
  hint 匹配上一版本文件身份。真实 MySQL 覆盖账号隔离、弱信号拒绝、幂等与迁移重放 1/1，
  EVT-007 20/20 回归通过且 schema 全部清理；`personal-secretary` 273/273、QQBot
  57 + 10 + 3、`qqbot-server` 175 passed/2 ignored、架构边界 24/24。
- `2026-08-06 00:38（Asia/Shanghai）`：`GAP-007-IMPL-D` 故障注入与 writer 隔离完成。专用 OS
  writer 线程避免同步文件 I/O 占用 Tokio blocking pool；关闭 deadline 超时 detach 并保留 WAL。
  WAL 创建、append、尾部 truncate、checkpoint、compact、替换后文件/父目录同步点均有注入恢复
  证据；MySQL 离线和必需 hook 失败不推进 checkpoint。`qqbot-server` 169 passed/2 ignored，
  `personal-secretary` 270/270，架构边界 24/24；真实 MySQL recovery 1/1、EVT-006 1/1、
  EVT-007 20/20 通过。
- `2026-08-06 00:05（Asia/Shanghai）`：`GAP-007-IMPL-C` runtime/health 与 MySQL 恢复闭环完成。
  NapCat callback 改为 bounded admission，文件 I/O 由 blocking writer 执行；`sync_all` receipt 后才进入
  ingestion，MySQL commit 与必需 Recall/Artifact hook 收敛后才推进连续 checkpoint。Spool fatal、队列满
  或关闭超时会保留开放 epoch/WAL 并 fail-closed；启动前按账号领取、续租和 fencing 遗留 epoch，完成
  replay/checkpoint 后原子结束 epoch、创建或复用 uncertain Gap。新增脱敏健康 telemetry、5 个 runtime
  恢复测试与 1 个真实 MySQL 测试；Rust/架构/严格 Clippy 门禁通过。
- `2026-08-05 23:25（Asia/Shanghai）`：`GAP-007-IMPL-B` 文件适配器完成并通过 Codex 独立复核。
  新增独立普通消息 AEAD WAL（不同于 Recall magic/key/路径）、实际分配量预算（活动 WAL 240 MiB、
  compact 临时 240 MiB、quarantine 16 MiB、元数据 16 MiB，总计 512 MiB）、进程锁、最终不完整尾部
  截断、完整帧认证/解码 fail-closed、generation 绑定 AAD、连续 checkpoint、compact 及 Windows
  `MOVEFILE_WRITE_THROUGH` 替换。新增 9 个聚焦测试；`qqbot-server` 151 passed/2 ignored，严格
  Clippy、all-targets check、fmt、diff check 与 workspace boundaries 24/24 通过。尚未接入 runtime、
  MySQL replay 或健康 Worker。
- `2026-08-05 23:03（Asia/Shanghai）`：`GAP-007-IMPL-A` Codex 独立复核完成。直接修复恢复端口
  缺少账号作用域与 fencing 的 P1：领取改为显式接收 `SourceAccountRef` 并返回携带 typed lease
  token 的 claim，`connecting`/`connected` 收口只能传递该 claim，后续 MySQL 实现必须事务内复验
  账号、epoch、token 与租约未过期。同步封闭 admission、恢复帧和 replay progress 的关键字段，
  修复大枚举内联完整消息造成的 Clippy 告警。`personal-secretary` all-targets check、严格 Clippy、
  270/270 单元测试、架构边界 23/23、fmt 与 diff 检查通过；无 Docker/MySQL 或外部系统操作。
- `2026-08-05 22:50（Asia/Shanghai）`：开始 `GAP-007-IMPL-A`，只在
  `personal-secretary` 新增协议中立的实时 Spool 领域/application 契约与纯单元测试：typed
  admission/recoverable/fatal、仅运行期的 `DurableSpoolReceipt`、完整认证 WAL 帧的启动 replay
  资格、连续 checkpoint 前缀、Recall 关联键与确定性 `ArtifactId` 的效果收敛，以及遗留
  `connecting`/`connected` epoch 的分阶段恢复端口。恢复端口明确只保证数据库内 epoch/Gap
  原子收口，不声称文件 checkpoint 跨资源原子。未实现文件 WAL、AEAD、锁、compact、runtime、
  MySQL 适配器、迁移或配置；未执行编译、Clippy、测试、Docker/MySQL、网络或真实 QQ/NapCat，
  等待 Codex 独立复核与门禁执行。
- `2026-08-05 22:37（Asia/Shanghai）`：GAP-007-A/B/C 最终规格经 Codex 独立复核通过，结论为
  **GO，尚未实现**。最终修复遗留 epoch 顺序：旧 `connected` epoch 先完成 WAL replay、hook 收敛和
  耐久 checkpoint，再以 MySQL 原子事务结束 epoch 并创建 Gap；跨文件/MySQL 流程只承诺崩溃收敛。
  TODO 已建立未完成的 IMPL-A/B/C/D；未修改或验证任何生产 Spool。

- `2026-08-05 21:17（Asia/Shanghai）`：GAP-003-A/B/C Codex 独立复核完成。修复未验证的
  NapCat 响应方向被误当完整性证据的问题，新增 `UntrustedPageOrder` 与独立来源能力门；
  全部 Rust 门禁、隔离 MySQL 1/1 及 EVT-007 回归 20/20 通过，schema 全部清理。

- `2026-08-05 20:43（Asia/Shanghai）`：GAP-003-A/B/C 实现切片完成。新增协议无关的
  方向/continuation 契约，多页连续性与冻结边界幂等判定，类型化 QQBot 方向及
  NapCat 空页无证据停止/脱敏边界。测试代码覆盖 Fake 多页、HTTP JSON、适配器、
  架构和 ignored 随机隔离 MySQL 租约恢复。未执行任何编译或测试，不声称真实
  NapCat 分页方向、空页原因或 PacketBackend 行为已验证；等待 Codex 独立门禁。

- `2026-08-05 15:31（Asia/Shanghai）`：EVT-007-MSG 第五轮 Codex 复核完成。迁移原先用
  `SELECT CASE ... HAVING` 返回零行，执行器会忽略结果并错误登记成功；现改为单语句条件性多行
  标量子查询，结构错误稳定触发 MySQL 1242，并严格复验列类型/默认值/字符集/排序规则、索引
  列顺序及 FK `ON DELETE CASCADE`。测试迁移加载器新增返回错误的入口，只在整文件成功后写
  migration record；场景 20 覆盖正确重放及错误列、错误索引顺序、错误 FK 规则三条负向重放。
  场景 17 改用真实 semantic store 领取批次，Reply 解析撤销租约后旧补丁提交返回 LeaseLost，
  且不产生派生行。最终 `evt007_delayed_reply_mysql` 20/20、personal-secretary 251/251、
  qqbot-server 138 passed（2 ignored）、三 crate 严格 Clippy、workspace all-targets check、fmt 与
  diff 检查全绿；本轮随机 schema 残留为零。未连接真实 QQ/NapCat，随同一原子提交收口。

- `2026-08-04 18:06（Asia/Shanghai）`：EVT-007-MSG 延迟 Reply 子先父后解析闭环。仅 NapCat
  群/私聊消息 Reply；非消息 Reply 拆为 EVT-007-NONMSG（待真实样本）。`resolve_reply` 增加
  会话与通道校验（跨账号/跨会话同名消息 ID fail-closed）；父事件入库事务内
  `resolve_pending_replies_in_txn` 幂等回填 pending 子事件并失效旧线程投影（复用既有
  `te.source_event_id IS NULL` 重领取机制）；Duplicate 父重放同样修复 pending；提交后自愈
  短事务覆盖并发交错窗口；无需新表/新迁移。新增 `evt007_delayed_reply_mysql` 聚焦测试
  8/8 真实通过（详见 [`history/2026-08.md`](history/2026-08.md) 同时间条目），随机 schema
  已精确清理；EVT-006 1/1、CMD-010 2/2、participant_causality 2/2 回归未破坏。
  未连接真实 QQ/NapCat；未提交，工作树等待 Codex 复核；未 push/merge/stash。

- `2026-08-04 20:20（Asia/Shanghai）`：按 Codex 复核意见完成 EVT-007-MSG 的 4 个 P1 修复与
  5 类补充测试并全量复验。P1：解析时同步撤销投影领取（旧计划 commit 判 LeaseLost）、变空
  旧线程标记 closed（authority=system_recovery）并清除语义批处理状态与租约（不删除线程
  行）、提交后自愈失败只记日志保持已提交契约（unresolved 等待父重放可恢复）、投影失效
  只删 reply/same_conversation_window/same_actor_within_conversation_window 边。补充测试：
  领取后提交前解析的 LeaseLost 并发、旧线程 closed/root/语义/租约清理断言、自愈失败契约
  （故障注入 trigger + 恢复后父重放）、非 Reply 证据边保留、真实 `BackfillGapUseCase`
  路径（父经回补统一入口到达并解析）。修复测试暴露的 `COUNT(*)` 有符号 BIGINT 解码缺陷
  （`CountRow` 改为 `i64`）。`evt007_delayed_reply_mysql` 12/12 真实通过；EVT-006 1/1、
  CMD-010 2/2、participant_causality 2/2、cmd009 2/2、action_planner 6/6、
  project_commitment 3/3 回归通过；personal-secretary 248/248、qqbot-server 131/131
  （2 ignored）、受影响 3 crate 严格 Clippy、fmt、git diff --check 通过；数字人侧
  ai-core/digital-human-server 的预先存在 Clippy 错误与本次无关。
  未连接真实 QQ/NapCat；未提交，工作树等待 Codex 复核；未 push/merge/stash。

- `2026-08-04 22:30（Asia/Shanghai）`：按 Codex 第二轮复核（4 个 P1 + 1 个 P2）完成修复并
  全量复验。P1-1 持久化 reconciliation：新增增量迁移 `20260804_qqbot_reply_reconcile.sql`
  （`secretary_reply_reconcile_claims` 候选退避簿）+ 领域层
  `ReplyReconcileStoreT`/`ReconcilePendingRepliesUseCase`（有界领取、租约/fencing、
  SKIP LOCKED、指数退避、跨重启）+ MySQL 实现 + `qqbot-server` 后台修复 Worker
  （`[reply_reconcile]` 配置段，FakeRunner 单测覆盖生命周期/错误恢复/退避）+ 主路径
  解析联动清理退避簿；自愈失败日志只记 error_code/stage/计数（P2）。P1-2 空线程关闭
  原子化：FOR SHARE 锁读状态 → 条件 UPDATE（status=读值 AND NOT EXISTS 成员）检查影响
  行数，投影 commit 对目标线程 FOR UPDATE 复验（终态线程拒绝新成员）。P1-3 已提交语义
  派生撤销：claims→withdrawn、decisions→revoked、open questions→dismissed、
  expectations→dismissed（同一事务，保留审计）。P1-4 保留边迁移：投影 commit 把事件
  的 explicit_project_id/file_version 边 `thread_id` 迁移到事件当前线程。`evt007_
  delayed_reply_mysql` 15/15 真实通过（新增：reconcile 退避/有界/跨路径清理、终态线程
  不写虚假历史且拒绝新成员、语义派生撤销），EVT-006 1/1、CMD-010 2/2、
  participant_causality 2/2、cmd009 2/2、action_planner 6/6、project_commitment 3/3
  回归通过；personal-secretary 248/248、qqbot-server 138/138（2 ignored，含新增
  reply_reconcile 配置与 Worker 单测）、受影响 3 crate 严格 Clippy、fmt、workspace
  all-targets check、git diff --check 全部通过；数字人侧 Clippy 错误仍为预先存在。
  未连接真实 QQ/NapCat；未提交，工作树等待 Codex 复核；未 push/merge/stash。

- `2026-08-05 12:00（Asia/Shanghai）`：按 Codex 第三轮复核（6 个 P1 + 2 个 P2）完成修复。
  **P1-1 reconcile fencing**：`ClaimedPendingReply` 新增 `lease_token`；处理时先 FOR UPDATE
  锁定退避簿行并复验 `lease_token = ? AND lease_expires_at >= now()`（RI=0 放弃）；所有完成/
  退避写入以 token + 未过期条件锁定并检查影响行数，旧 Worker 不能覆盖新租约。
  **P1-2 终态线程派生撤销**：`close_empty_thread_in_txn` 分离状态迁移与派生撤销——终态线程
  跳过关闭 + 历史写入，但仍在锁内确认空成员后调用 `revoke_semantic_derivations_in_txn`
  撤销 claims/decisions/questions/expectations；场景 14 注入 claim 验证 resolved 线程上
  claim 标记 withdrawn。**P1-3 投影遇终态整体失败**：`commit_projection` 对终态目标由
  `continue` 改为 `rollback + return Err(LeaseLost)`，不再部分提交 plan 的 relations 或
  清除 claims（事件由下次领取重新规划）。**P1-4 跨线程关系删除**：`resolve_pending_replies_in_txn`
  删除子事件所有出边（移除 `relation_kind IN (...)` 限制），移除 `commit_projection` 中的
  关系迁移代码；场景 11 断言 explicit_project_id 与其它出边一并删除。
  **P1-5 pending 扫描索引**：新迁移 `20260805_qqbot_reconcile_index_fk.sql` 添加
  `idx_secretary_source_reply_pending (reply_to_event_id, received_at)`。
  **P1-6 shutdown 热轮询**：`shutdown_changed` 由 `yield_now()` 改为 `sleep(100ms)`。
  **P2-1 退避簿 FK**：同迁移添加 `ON DELETE CASCADE` FK。**P2-2 退避修正**：reconcile store
  退避公式改为 `1 << (attempts-1)`（首败=initial，非 initial*2）；`retry_max_ms` 加上界
  3,600,000；`chrono::Duration::milliseconds` 前安全截断 i64。单元测试 248+138 全部通过；
  Docker Desktop 不可用时 MySQL 15 场景未重跑，其余门禁（fmt/clippy/check/diff --check）全绿。
  未连接真实 QQ/NapCat；未提交，工作树等待 Codex 复核；未 push/merge/stash。

- `2026-08-04 14:54（Asia/Shanghai）`：继续完成 `qqbot-server` 洋葱边界重构。源文件按
  `application/adapters/infrastructure` 物理分层并用显式模块路径保持现有 crate API；QQ Open
  Platform 编排不再接收 `DatabaseConnection` 或构造 MySQL store，而是注入 OwnerBinding、
  GatewaySession 和 raw-event 端口，SQL 实现下沉 infrastructure。NapCat 目录/历史 source 移入
  adapters；ingestion 用应用层队列错误和健康报告端口，application 生产代码不再引用 NapCat 或
  infrastructure 具体类型。新增架构守卫后 20/20，QQBot 131/131（2 ignored），fmt/all-targets
  check 通过。无数据库迁移，未连接真实 QQ/NapCat；未 commit/push/merge/stash。

- `2026-08-04 14:38（Asia/Shanghai）`：完成 QQBot 个人秘书洋葱目录与编译依赖重构。
  `personal-secretary` 物理拆为 domain/application 并移除 SeaORM；新增
  `personal-secretary-mysql` 承载 55 个仓储文件和全部 MySQL 测试；Planner 通过
  `ActionCheckpointStoreFactoryT` 端口获取持久化 Checkpoint，不再持有 `DatabaseConnection`。
  全仓架构门禁从数字人测试目录迁至 `tools/architecture-tests`，取消服务层 DB 豁免。
  fmt、all-targets check、严格 Clippy、248 项领域/应用测试、131 项 QQBot 测试、19 项架构门禁，
  以及 MySQL Action Planner 6/6、CMD-010 2/2、EVT-006 1/1 均通过；随机 schema 已清理。
  无数据库迁移，未连接真实 QQ/NapCat；未 commit/push/merge/stash。

- `2026-08-04 13:56（Asia/Shanghai）`：保守精简 QQBot 旧 MySQL 测试资产。删除
  `mysql_action_planner.rs` 中已被更强多轮 Replan/重启场景覆盖的完整生命周期 happy path 与
  单轮 Retriever/Effect roundtrip，共减少 326 行；保留的 6 条账号隔离、租约、事务、
  Suspend/Resume、CAS 与多轮 Replan 场景在随机隔离 MySQL schema 中 6/6 通过，schema 已清理。
  fmt、all-targets check、严格 Clippy、两 crate 单元测试和 workspace boundaries 均通过；未提交、
  未 push/merge/stash，未连接真实 QQ/NapCat。

- `2026-08-04 13:38（Asia/Shanghai）`：Docker 命名管道权限恢复后，增强版
  `evt006_ingestion_batch_mysql` 1/1 与 CMD-010 Owner 安全回归 2/2 再次真实通过；增强场景覆盖
  数据库中途失败整批回滚、恢复重试与幂等重放，随机 `evt006s1` schema 已清理。EVT-006
  复核与验证全部闭合，进入唯一提交；未连接真实 QQ/NapCat。

- `2026-08-03 22:04（Asia/Shanghai）`：EVT-006 入站微批处理 + 可观察背压 + 合成负载闭环
  实现、Worker 聚焦验证与文档同步完成。详见
  [`history/2026-08.md`](history/2026-08.md) 同时间条目（实现、验证与 Git 状态）。

- `2026-08-04 13:28（Asia/Shanghai）`：Codex 独立复核 EVT-006，修复共享健康指标未接入运行时、
  排空后 queue depth 不归零、overflow Gap 健康状态无法恢复，以及批量统一时间戳触发
  `RecordNotInserted` 的幂等 upsert 缺陷；收紧入站错误日志为固定 error code。随机隔离 MySQL
  初版 `evt006_ingestion_batch_mysql` 1/1、CMD-010 Owner 安全回归 2/2 真实通过，schema 已清理；
  后续补强整批回滚/恢复重试断言，已通过格式、严格 Clippy 与编译，但当前沙箱拒绝 Docker
  命名管道，增强场景尚待最终复跑，故未提交。未连接真实 QQ/NapCat，未 push/merge/stash。

- `2026-08-03 21:10（Asia/Shanghai）`：Codex 完成 CMD-010 独立复核与修复。新增
  `ensure_action_run` 创建前 OwnerCommand 授权、领域 `PlanNode` 非 L0 命令证据门和 OpenReference
  强制澄清门；ResolveReference 以类型化来源恢复有界 Replan，显式失效会话不再降级为 thread-only；
  Agenda 错误恢复 Unauthorized/LeaseLost/Database 分类。真实 MySQL 中 CMD-010 2/2、Action
  Planner 8/8、Participant/Causality 2/2 通过；personal-secretary 248/248、qqbot-server
  124/124、workspace boundaries 19/19，严格 Clippy/格式/diff 均通过。未连接真实 QQ/NapCat。

- `2026-08-03 20:18（Asia/Shanghai）`：CMD-010 Owner 越权、提示注入与跨会话指代歧义防线
  实现、聚焦验证与文档同步完成，停在工作树等待复核；未 commit/push/merge/stash。详见
  [`history/2026-08.md`](history/2026-08.md) 同时间条目（实现、验证与 Git 状态）。

- `2026-08-03 18:45（Asia/Shanghai）`：Codex 完成 CMD-009 独立复核。修复冲突上下文写入后
  Router 反向结束导致第二轮 Planner 不可达、远程模型可能读取 local_only 事实来源、工作上下文
  仅投影 fact_ref 而丢失 event/conversation/thread/actor refs、旧开放引用自由文本可能回放稳定 ID、
  缺失账号与 Proposed 事实回读 fail-open、状态合并失败留下半更新，以及测试整数解码吞错。
  `personal-secretary` 245/245、`qqbot-server` 121/121、workspace boundaries 19/19；随机隔离
  MySQL 中 CMD-009 2/2、Action Planner 8/8 通过，schema 已清理；未连接真实 QQ/NapCat。

- `2026-08-03 16:38（Asia/Shanghai）`：CMD-009 验证与文档同步完成，停在工作树等待 Codex
  评审；未 commit/push/merge/stash。详见 [`history/2026-08.md`](history/2026-08.md)
  同时间条目（实现、测试、验证与 Git 状态）。

- `2026-08-03 14:41（Asia/Shanghai）`：开始并完成 QQBot 旧测试清理的文件级变更：删除
  10,287 行且 38 项全部默认忽略的 `mysql_ingestion.rs`、两套旧 acceptance 测试目标、真实
  QQ/NapCat E2E/live 测试，以及配套验收矩阵、PowerShell attestation/门禁和 GitHub workflow。
  保留现行 `mysql_action_planner`、参与者因果、项目承诺 MySQL 聚焦测试及 NapCat 本地 mock；
  数字人测试和业务未触碰。14:43 完成验证：格式、三个相关 crate 全 targets 编译、严格 Clippy、
  personal-secretary 238/238、qqbot 63/63、qqbot-server 118/118（2 ignored）与 workspace
  boundaries 19/19 全部通过；净删除 17,389 行、增加 47 行文档。

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
- `2026-08-05 21:00（Asia/Shanghai）`：EVT-007-MSG 第四轮复核反馈修复（P1×2 + P2×3）。
  P1-1：终态父线程强制新建线程——`force_new_thread` 标志跳过 reply-child + previous，
  父在终态线程时子事件不入同会话其他 open 线程；新增 3 条领域单测（含反例覆盖）。
  P1-2：场景 17 改为预置 resolved 线程（走终端分支而非 open→closed）；场景 19 新增
  过期令牌测试；场景 20 新增完整 migration 重放（删 migration record + 重执行 DDL +
  INSERT IGNORE）。P2-3：Reply 关系抑制条件只检查 `reply_parent_thread_is_terminal`，
  不混入 child-thread 状态。P2-4：迁移增加 INFORMATION_SCHEMA 结构验证（列/索引/FK
  三重检查 fail-closed）。P2-5：TODO.md 移除已删除的 20260805 迁移引用，更新工作树
  状态。领域测试 251 全绿，evt007 20/20，回归 5/5，全部门禁通过。未提交。

- `2026-08-05 19:30（Asia/Shanghai）`：EVT-007-MSG 第四轮 Codex 复核修复完成。6 项强制修复：
  ① reconcile fencing `query_one_raw` FOR UPDATE + `fenced_clear` 检查 RI；
  ② 终态空线程先 DELETE `secretary_thread_semantic_state` 再撤消派生；
  ③ Relation 清理覆盖入边方向（`from_event_id OR to_event_id`）；
  ④ 终态父线程拒绝 Reply 子事件（planner 三级终态判定 + 新线程）；
  ⑤ 候选队列重构（`secretary_reply_reconcile_claims` 为唯一真实候选源）；
  ⑥ `ReconcileCandidateRow.attempts` 类型修正。`evt007_delayed_reply_mysql` 20/20，
  6 个回归套件全绿，Docker 验证完成。详见 `history/2026-08.md`。未提交。

- `2026-07-24 22:42（Asia/Shanghai）`：Ollama Qwen3 实机语义与提示注入边界验收通过。
- 旧事件仅有日期证据，保留原日期，不伪造分钟。

## 分块规则

- 每个归档按月份建立；达到 500 行或 100 KiB 时，再按日期和业务切片拆分。
- 归档内容按时间顺序追加，完成事项必须同步更新 `TODO.md`。
- 每条记录必须包含完成范围、数据库影响、外部系统影响、验证、Git 状态和下一项。
- 根索引只保留当前阶段和最近 10 条事件，避免无限增长。
