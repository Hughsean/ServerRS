# QQBot 独立数据库

本目录只属于 `qqbot-server`/个人 QQ 智能秘书，不属于数字人数据库。

- QQ 表不得写入仓库根目录的 `database/sql/init.sql` 或其 migrations；
- 不复用数字人的 `users`、`conversations`、`user_memories` 等业务表；
- 新空库按 `migrations/` 依赖顺序执行即可建立当前结构：先 `ingestion`，再
  `continuity`（依赖 ingestion），再执行 `backfill`（依赖 continuity）和 `threads`
  （依赖 ingestion），最后执行 `thread_links`、`thread_semantics` 和
  `thread_mutations`、`thread_revisions`、`memory`，最后执行 `memory_controls_followups` 和
  `qq_open_platform`，最后执行 `action_planner`；不要按文件名字典序执行，同一天的
  `backfill` 在字典序上会早于它依赖的 `ingestion`；
- 运行时只读取 `QQBOT_DATABASE_URL` 或 `qqbot.toml` 的 `[database]`；
- 所有表使用 `secretary_*` 前缀，后续迁移只在本目录演进。

当前基线迁移：

```text
migrations/20260723_personal_secretary_ingestion.sql
migrations/20260723_personal_secretary_continuity.sql
migrations/20260723_personal_secretary_backfill.sql
migrations/20260724_personal_secretary_threads.sql
migrations/20260724_personal_secretary_thread_semantics.sql
migrations/20260724_personal_secretary_thread_links.sql
migrations/20260724_personal_secretary_thread_mutations.sql
migrations/20260724_personal_secretary_thread_revisions.sql
migrations/20260724_personal_secretary_memory.sql
migrations/20260724_personal_secretary_memory_controls_followups.sql
migrations/20260724_personal_secretary_qq_open_platform.sql
migrations/20260725_personal_secretary_action_planner.sql
migrations/20260726_personal_secretary_action_planner_hardening.sql
```

第一项迁移创建账号、会话、入站事件和消息内容；第二项迁移增加连接周期、事件来源关联、
账号/会话游标和不确定空窗；第三项迁移增加历史回补运行（`secretary_backfill_runs`）和
会话 Scope 进度（`secretary_backfill_scopes`），以及 Gap 边界快照
（`secretary_gap_boundaries`，创建时冻结，回补边界按平台消息 ID 匹配）与再次领取退避调度
（`secretary_gap_reclaim_schedule`）。运行表保存 Gap/账号引用、Scope、状态与租约期限；
独立的 `secretary_backfill_leases` 保存当前租约所有权令牌，兼容重复执行迁移为早期草稿表
补上 fencing 能力。其余字段包括起止
锚点、页数/读取数/Accepted/Duplicate、完整性证据与失败分类。回滚顺序写在各迁移文件末尾，
执行回滚前必须先确认不再需要个人秘书数据。回补运行表不保存聊天正文、Token 或完整原始
HTTP 响应。

第四项迁移增加确定性事件线程、事件成员、来源关系边和独立投影租约。线程消费者不复用
`secretary_source_events.processing_status`；Reply 优先，同会话短窗口次之，发送者只作为
同会话窗口内的附加证据，不会单凭同名人物或相同文件名跨会话自动合并。关系原因不保存
消息正文。

第五项迁移增加线程语义游标/租约、要求/反对/确认候选、结论修订链、未决问题、生命周期
历史及全部来源连接表。所有自动提取结果默认是 `proposed`，不会静默成为确认事实；关闭线程
必须经过领域校验，来源必须包含已验证的 `OwnerCommand` 且不能仍有开放问题。会话策略为
`never_long_term` 或正文策略为 `envelope_only` 的事件不会进入语义提取。

第六项迁移增加 Owner 控制账号绑定、跨会话关联扫描租约、强提示指纹、关联候选、来源和
不可变审核表。项目 ID 与精确文件
`source_key` 只以 SHA-256 保存；候选固定从 `proposed` 开始，数据库约束不同线程/不同会话，
不会修改 `secretary_thread_events`。同名人物、相似话题和文件显示名不进入强提示表。审核必须
由 QQ 开放平台 `OwnerCommand` 事件及本地显式绑定共同授权；绑定只保存账号标识，不保存 App
Secret。

第七项迁移增加线程拆分/合并 Proposal、QQBot 专用持久化 Graph Checkpoint、可撤销 Merge
Alias、Split Override 和 `secretary_effective_thread_events` 视图。审批前只保存有界影响快照；
Resume 必须由 QQ 开放平台 `OwnerCommand` 与本地 Owner 绑定共同授权。原始
`secretary_thread_events` 永不物理搬移；第一条 Merge 线程是 canonical thread，Split 生成
新的逻辑线程。Checkpoint 使用 CAS 单次消费，Effect ID 全局唯一且重复执行返回既有结果；
提交结果不明由 Agent Runtime 进入 `UnknownCommit`，不得自动重放。

第八项迁移增加线程变更撤销审计和语义失效证据。撤销只停用逻辑 Alias/Override，不删除
Proposal、Checkpoint、Effect Receipt 或原始成员；应用和撤销都会刷新强提示线程、让仍处于
`proposed` 的旧关联候选过期、记录受影响线程，并重置语义游标。失效时间之后重新提取的语义
才能成为当前状态，旧派生记录继续保留用于审计。

第九项迁移增加来源化结构记忆：`secretary_memory_facts` 保存人物、项目、承诺的有界类型化
状态、置信度、TTL 和单向修订链，`secretary_memory_fact_sources` 保存无损 SourceEvent 引用。
来源属于 `never_long_term` 会话或 `envelope_only` 内容时拒绝写入；到期事实标记为 `expired`，
修订将旧版本标记为 `superseded`，不会覆盖或删除历史证据。

第十项迁移增加派生记忆删除审计、承诺跟进事项和平台无关通知 Outbox。派生记忆删除必须引用
本地显式绑定授权的 QQ 开放平台 `OwnerCommand`，只把事实标为 `deleted`，不会连带删除原始
SourceEvent。调度 Worker 对每轮数量、时间视野和错误退避均设上限；承诺到期后只生成唯一的
`pending` 通知。领取按来源账号强隔离；投递结果不明进入 `unknown_commit`，不自动重试。

第十一项迁移增加 QQ 开放平台 Gateway Resume 会话和无损原始入站信封。Resume sequence 只有
在标准化事件与原始 JSON 均可靠落库后才推进；App ID 是会话主键，避免两个 Bot 账号共用
Session/OpenID 命名空间。表中不保存 App Secret 或 access token。

第十二项迁移增加 Owner Action Planner 运行、完整 Agent Checkpoint、Effect Receipt、响应产物
与不可变审计。OwnerCommand 的 run ID 使用来源事件和 Planner 版本派生的稳定 UUIDv5，严格
匹配 `CHAR(36)`；挂起会将 run 转为 `suspended` 并释放 Worker 租约，Resume 必须同时匹配
run、checkpoint、proposal 和 command source，并通过 CAS 获取新的恢复租约。响应草稿与 run
完成状态在同一事务中提交。

第十三项迁移补齐消息级 `never_long_term` 数据库约束，使会话级与消息级内容策略都能按
`never_long_term > envelope_only > local_only > normal` 取最严格值。
