# 个人 QQ 智能秘书 Todo

> 最后更新：2026-07-24
> 维护规则：完成项必须同步写入 `HISTORY.md`，不得仅勾选；新增具体事件使用
> `YYYY-MM-DD HH:mm（Asia/Shanghai）`，精确到分钟，不得用猜测时间回填旧事件。
> 当前开发阶段：阶段 3「跨会话因果线程」正在推进；历史回补 Worker、Gap 状态机、线程
> 投影、Owner 关联审核和高影响线程变更后端闭环已完成，控制面和长期记忆仍待实现。

## 0. 当前完成与阻塞

- [x] `DONE ID-001` 建立 `personal-secretary` 协议无关业务 crate。
- [x] `DONE ID-002` 建模来源账号、会话、可信发送者、消息角色和账号作用域幂等键。
- [x] `DONE ID-003` 保证 NapCat Owner 消息只能是 Observation。
- [x] `DONE ING-001` NapCat 接收群聊和普通私聊。
- [x] `DONE ING-002` 接收并标记本人 `message_sent`。
- [x] `DONE ING-003` 保留 CQ `Reply`、`At`、文本和常见媒体消息段。
- [x] `DONE ARCH-001` QQ 协议、个人秘书业务和数字人业务保持 crate/进程隔离。
- [x] `DONE ENV-001` 使用隔离 Docker MySQL 完成独立 QQBot 数据库集成验收。
- [ ] `BLOCKED ENV-002` 在本地环境配置 QQ 开放平台 App ID/Secret；不得写入文档或 Git。
- [ ] `PARTIAL ENV-003` 两个 NapCat 测试账号均在线；已在唯一获批测试群实测未 @、@、本人
  消息、跨账号 Reply、撤回通知、双 WebSocket 和主动重连。测试临时开启的本人消息上报与第二
  实例 WebSocket 已逐字恢复，未测试私聊主动发送；群免打扰状态是否影响上报仍需单独取证。
- [ ] `PARTIAL ENV-004` 双账号群/私聊历史、同参数稳定读取、精确锚点包含语义和同账号
  `get_msg` 已实测；主动样本再次确认跨账号消息 ID 不可互用，正确 Reply 必须使用发送账号
  自己观察到的父消息 ID。多页方向、空页原因、跨重启覆盖和 PacketBackend 兼容性仍待验证。

## 1. 下一垂直切片：可靠事件存储（P0）

- [ ] `PARTIAL EVT-001` 已定义 `SourceEventId`、消息内容段、会话、可信发送者、
  `ConnectionEpoch`、`IngestionCursor` 和 `IngestionGap` 类型；`ParticipantIdentity` 仍待实现。
- [x] `DONE EVT-002` 独立设计 `secretary_*` MySQL 表、唯一键、索引和外键；迁移位于
  `apps/qqbot-server/database/migrations`，不修改数字人 `init.sql` 或既有 `qq_*` 表。
- [x] `DONE EVT-003` 定义协议无关的 `InboundEventStoreT` 并实现 SeaORM/MySQL 仓储。
- [x] `DONE EVT-004` 在单事务中执行账号作用域幂等插入，返回 `Accepted/Duplicate`。
- [x] `DONE EVT-005` `qqbot-server` 先持久化再允许进入后续处理；Duplicate 不重复投递。
- [ ] `PARTIAL EVT-006` 已增加有界 `mpsc` 队列、非阻塞 `try_send` 背压和独立数据库重试
  Worker；WebSocket 回调不再等待 MySQL。批处理和 LLM 消费链尚未实现。
- [ ] `PARTIAL EVT-007` 已把可命中的 `Reply` 平台 ID 解析为 `reply_to_event_id`；历史回补
  已实现“子先父后”同账号回填（父消息后到时自动回填未解析的 `reply_to_event_id`，幂等且
  不跨账号）。剩余条件：跨重启覆盖更大样本、非消息事件 Reply 路径仍待覆盖。
- [ ] `PARTIAL EVT-008` 已确认 NapCat 实时上报 `group_recall`，当前 Listener 仅记录 Debug 后
  忽略；撤回事件信封、编辑、通知持久化和对原消息状态的投影仍待实现。
- [ ] `TODO EVT-009` 增加正文加密/脱敏边界和 `normal/local_only/envelope_only` 保存策略。
- [ ] `TODO EVT-010` 将 NapCat HTTP 接口拆成只读业务端口；`qqbot-server` 永远不能调用
  `send_group_msg`、`send_private_msg` 或 `group_poke`。

验收：重复事件只产生一条 `SourceEvent`；服务重启后仍能继续消费；数据库慢或 LLM 超时时
WebSocket 接入不会同步卡死；每条派生状态可追溯到事件。

## 2. 空窗、回补和连续性（P0）

- [x] `DONE GAP-001` 持久化连接周期、连接/断连时间、结束原因和最后成功事件。
- [x] `DONE GAP-002` 新消息在同一事务中推进账号级和会话级稳定游标。
- [ ] `PARTIAL GAP-003` 已增加 NapCat `get_msg`、好友/群历史分页的只读类型化适配器，并完成
  双账号无正文契约探测；确认精确锚点包含、无效相邻锚点失败、消息 ID 账号作用域、
  `get_msg` 同账号往返和主动重连后继续接收。多页翻页方向和完整性证明仍待完成。
- [x] `DONE GAP-004` 实现回补 Worker；实时和历史事件走同一幂等入口。独立有界 Worker
  与实时 WebSocket 接收解耦，按 `uncertain -> backfilling` 原子领取 Gap，有界分页，崩溃
  恢复，重连唤醒，多任务经 `JoinSet` 真正并发受 `max_concurrency` 限制；历史消息经统一
  `insert_message_if_absent`。仅领取空窗已结束（`gap_ended_at IS NOT NULL`）的 Gap；过期运行
  只做有界领取，实际恢复进入同一并发集合，合法长任务不受固定短超时截断；错误退避不会被
  周期扫描或重连唤醒绕过。
- [x] `DONE GAP-005` Gap 回补状态和完整性证据。回补运行与 Scope 进度持久化于
  `secretary_backfill_*`；完整性证据判定集中于领域层 `HistoryCompleteness`，真实 NapCat
  无法证明账号会话集合完整时 Gap 保持 `uncertain`，不因 Worker 跑完误标完整。证据不足回到
  `uncertain` 的 Gap 受 `secretary_gap_reclaim_schedule.next_eligible_at` 退避约束，可再次
  回补（运行表 gap_id 无唯一键）且不饿死后续 Gap；回补边界读 `secretary_gap_boundaries`
  创建时快照（非实时漂移游标），按平台消息 ID 匹配。每次领取/接管轮换租约令牌，进度续租
  和终态提交均执行 fencing，旧 Worker 不可迟到覆盖；恢复继承已消耗事件预算。
- [ ] `TODO GAP-006` 通过官方 Bot/控制面向 Owner 展示空窗，不把“已重连”说成“已补齐”。
- [ ] `TODO GAP-007` 评估本地磁盘 Spool 与开机自启；记录容量、加密和损坏恢复策略。
- [ ] `TODO GAP-008` 对电脑关机/休眠、NapCat 离线、MySQL 离线分别做故障演练。

验收：断连、恢复、回补和不确定空窗均有可观察状态；任何无法验证的丢失都不会被隐藏。

## 3. 跨会话因果线程（P0）

- [x] `DONE THR-001` 定义 `EventThread`、`ThreadClaim`、`ThreadDecision`、`OpenQuestion` 和
  `ThreadRelation`。
- [ ] `PARTIAL THR-002` 已使用结构化 Reply、同会话短窗口和窗口内可信发送者建立确定性边；
  跨会话层新增严格格式项目 ID 与文件 `source_key` 强提示。发送者、相似话题和文件名不会单独
  触发跨会话关联；文件版本与非 Reply 引用仍待结构化提示入口。
- [x] `DONE THR-003` 独立有界 Worker 按 Reply 链/会话/短时间窗口批量投影，拥有独立消费
  租约、扫描上限、错误退避和可取消关闭；默认路径不调用 LLM，也不复用通用
  `processing_status`。
- [ ] `PARTIAL THR-004` 已实现协议无关提取端口、保守批量提取器和 MySQL 候选闭环；明确的
  请求/反对/确认前缀会生成 `proposed` 类型化候选，保存参与者、置信度和来源事件。模糊语义、
  上下文指代和未来 LLM 提取仍待实现，任何提取器输出都必须经过同一来源/身份/数量校验。
- [ ] `PARTIAL THR-005` 已持久化结论与 `supersedes_id` 唯一修订链，并禁止候选引用本线程外
  或非 confirmed 旧结论；Owner 确认、撤销和查看完整修订链的控制面仍待实现。
- [ ] `PARTIAL THR-006` 已实现 `open/waiting/resolved/closed/reopened` 状态机、不可变状态历史和
  来源表；关闭必须有 `OwnerCommand` 证据且无开放问题。自动结束条件和 Owner 控制入口仍待实现。
- [x] `DONE THR-007` 使用明确项目 ID 或精确文件 `source_key` 的不可逆指纹生成跨群/私聊
  `proposed` 线程关联候选，保存万分制置信度、类型化理由和双方来源；不改写线程成员。
- [ ] `PARTIAL THR-008` 领域校验和数据库约束已禁止同名人物、相似话题、相同文件名成为关联
  依据，所有候选保持 `proposed`；已实现 Owner 分页查看双方来源、接受/拒绝、账号绑定验权和
  幂等审计。QQ 开放平台交互通知与低置信度确认话术仍待 CMD 控制面接入。
- [ ] `PARTIAL THR-009` 语义批处理和跨会话关联扫描均已排除 `never_long_term` 会话与
  `envelope_only` 正文，并有 MySQL 防派生测试；跨会话检索授权过滤、既有派生状态清理和完整
  防泄露矩阵仍待实现。
- [ ] `PARTIAL THR-010` 已支持有界查看候选双方来源及接受/拒绝审计；拆分/合并现已形成
  `Proposal -> 持久化 Checkpoint -> NodeResult::Suspend(Approval) -> Owner Resume 验权 ->
  类型化 Effect/Receipt` 闭环。Merge 使用 canonical alias，Split 使用事件 override，原始
  `secretary_thread_events` 不搬移；有效线程视图已接入后续 Reply 投影、语义和跨会话扫描，
  Effect ID 重复执行幂等，提交结果不明不得自动重放。Owner 撤销/反向操作、既有已确认语义的
  修订迁移和重新打开话题仍待实现。

验收：给定多群和私聊的同一项目消息，系统能输出来源明确的要求、分歧、结论和未决问题；
错误关联可撤销，且不会向第三方会话泄露内容。

## 4. 人物、项目与承诺记忆（P0）

- [ ] `TODO MEM-001` 定义 `MemoryFact` 公共字段：类型、值、状态、置信度、来源、有效期和版本。
- [ ] `TODO MEM-002` 定义 `PersonMemory`：稳定身份、别名、关系、职责、权限和沟通偏好。
- [ ] `TODO MEM-003` 定义 `ProjectMemory`：目标、成员、进展、决定、风险、阻塞和文件版本。
- [ ] `TODO MEM-004` 定义 `Commitment`：承诺人、受益人、动作、期限、状态和完成证据。
- [ ] `TODO MEM-005` 建立原始事件、线程状态和长期记忆的来源引用。
- [ ] `TODO MEM-006` 实现查看来源和按来源回读最小原文片段。
- [ ] `TODO MEM-007` 实现记忆修正，保留修订链并重新计算受影响状态。
- [ ] `TODO MEM-008` 实现删除派生记忆、原始内容删除和检索索引清理的不同确认流程。
- [ ] `TODO MEM-009` 实现 TTL/到期清理和“永不进入长期记忆”会话策略。
- [ ] `TODO MEM-010` 实现冲突驱动回读，不用滚动摘要直接覆盖冲突事实。

验收：人物、项目和承诺均为结构化状态；每项事实可查来源、可修正、可删除、可过期；设置为
`never_long_term` 的会话不会生成长期记忆。

## 5. 主动跟进和提醒（P0）

- [ ] `TODO FUP-001` 定义 `ResponseExpectation`、`FollowUpRule`、`FollowUpOccurrence`。
- [ ] `TODO FUP-002` 从明确要求和承诺中生成待确认候选，而不是直接生效。
- [ ] `TODO FUP-003` 持久化 Scheduler 查询临近截止、逾期、长期无人回复和阻塞事项。
- [ ] `TODO FUP-004` 识别“是否已回复/完成”的证据，不能只按时间机械提醒。
- [ ] `TODO FUP-005` 支持工作时间、静默时段、重要联系人、群策略和升级规则。
- [ ] `TODO FUP-006` 每次跟进生成唯一 occurrence，重复扫描不重复提醒。
- [ ] `TODO FUP-007` 通过 Outbox 和官方 Bot 只提醒 Owner；失败重试和 UnknownCommit 可观察。
- [ ] `TODO FUP-008` 支持 Owner 确认、稍后提醒、完成、忽略、改期和关闭线程。
- [ ] `TODO FUP-009` 记录“为何提醒”和“为何未提醒”，支持重要/不重要反馈。
- [ ] `TODO FUP-010` 明确禁止 MVP 通过 NapCat 自动催促客户、负责人或群成员。

验收：服务重启后提醒不丢不重；报价截止、客户久未回复等场景能依据结构化证据提醒 Owner，
并且不会自动代表 Owner 联系第三方。

## 6. Owner 自然语言控制（P0）

> 进入 `CMD-001/CMD-002`、开始连接 QQ 开放平台前，必须先明确通知用户并等待其提供/确认
> 本地凭据配置；禁止把 App ID、Secret 或 Owner OpenID 写入 Git。

- [ ] `TODO CMD-001` 新增独立 `qq-open-platform` 协议 crate。
- [ ] `TODO CMD-002` 实现 App 凭证、Webhook/WebSocket、事件幂等和 Owner OpenID 绑定。
- [ ] `PARTIAL CMD-003` 线程关联审核已强制验证 `OwnerCommand`、本地 Owner 账号绑定及同一
  被管理账号；普通观察、未绑定和跨账号命令默认拒绝。其余命令类型仍须逐项接入同一边界。
- [ ] `TODO CMD-004` 定义类型化查询 Action：事件搜索、讨论总结、待回复和承诺列表。
- [ ] `TODO CMD-005` 定义类型化策略 Action：群提醒、重要联系人、静默时间和自动回复禁用。
- [ ] `TODO CMD-006` 定义反馈 Action：重要、不重要、类似消息规则和联系人策略。
- [ ] `TODO CMD-007` 定义记忆 Action：查看来源、修正、删除、TTL 和会话长期记忆开关。
- [ ] `PARTIAL CMD-008` 线程拆分/合并后端已展示精确有界影响范围，由节点主动返回类型化
  `NodeResult::Suspend(Approval)`；Proposal、QQBot 独立 MySQL Checkpoint、进程重启后 Resume、
  `OwnerCommand`/本地绑定验权、Effect Receipt 和拒绝零 Effect 均已接入。QQ 开放平台自然
  语言控制入口尚未开发，进入 `CMD-001/CMD-002` 前仍须先通知用户并配置本地凭据。
- [ ] `TODO CMD-009` 查询 Prompt 只包含有界状态、最近窗口和按需来源证据。
- [ ] `TODO CMD-010` 增加自然语言歧义、越权、Prompt 注入和跨会话泄露测试。

## 7. 控制面、可观测性和生产强化

- [ ] `TODO OPS-001` 展示 NapCat/官方 Bot/MySQL/Worker 健康状态。
- [ ] `PARTIAL OPS-002` 已为连接周期、入队、幂等结果、重试、溢出 Gap 和 Worker 排空增加
  `trace/debug/warn` 结构化日志；控制面展示、队列积压指标和回补进度仍待实现。
- [ ] `TODO OPS-003` 展示线程、记忆、承诺、提醒、Outbox 和来源。
- [ ] `TODO OPS-004` 提供修正、删除、策略和重新处理入口，所有操作审计。
- [ ] `TODO OPS-005` 建立吞吐、延迟、LLM 调用率、误关联和提醒误报指标。
- [ ] `TODO OPS-006` 压测高流量群，验证背压、批处理和成本上限。
- [ ] `TODO OPS-007` 备份恢复、密钥轮换、数据导出和彻底删除演练。

## 8. 后期扩展

- [ ] `DEFERRED KB-001` 文档/网页个人知识库和向量检索。
- [ ] `DEFERRED MM-001` 图片、语音和文件内容理解。
- [ ] `DEFERRED AUTO-001` 代表 Owner 向第三方自动回复；必须重新进行权限和风险评审。
