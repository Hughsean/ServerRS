# 个人 QQ 智能秘书 Todo

> 最后更新：2026-07-23
> 维护规则：完成项必须同步写入 `HISTORY.md`，不得仅勾选。
> 当前开发阶段：阶段 2「空窗、回补和连续性」已完成连接周期/游标/空窗及有界入站队列；
> 历史回补、批处理和本地 Spool 仍待实现。

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
- [ ] `PARTIAL EVT-007` 已把可命中的 `Reply` 平台 ID 解析为 `reply_to_event_id`；
  实际 `qqbot-server` 入库已验证父消息解析成功；父消息尚未入库时的待回填状态仍待实现。
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
- [ ] `TODO GAP-004` 实现回补 Worker；实时和历史事件走同一幂等入口。
- [ ] `PARTIAL GAP-005` 连接结束会创建 `IngestionGap(status=uncertain)`，重连只补齐空窗结束
  时间；队列溢出也会幂等创建 `reason=queue_overflow` 的 Gap。数据库离线叠加进程崩溃、以及
  历史接口无法证明连续性时仍待覆盖。
- [ ] `TODO GAP-006` 通过官方 Bot/控制面向 Owner 展示空窗，不把“已重连”说成“已补齐”。
- [ ] `TODO GAP-007` 评估本地磁盘 Spool 与开机自启；记录容量、加密和损坏恢复策略。
- [ ] `TODO GAP-008` 对电脑关机/休眠、NapCat 离线、MySQL 离线分别做故障演练。

验收：断连、恢复、回补和不确定空窗均有可观察状态；任何无法验证的丢失都不会被隐藏。

## 3. 跨会话因果线程（P0）

- [ ] `TODO THR-001` 定义 `EventThread`、`ThreadClaim`、`ThreadDecision`、`OpenQuestion` 和
  `ThreadRelation`。
- [ ] `TODO THR-002` 先使用 Reply、引用、发送者、明确项目 ID、文件版本等确定性关系建边。
- [ ] `TODO THR-003` 按会话/回复链/短时间窗口聚合消息，禁止默认逐条调用 LLM。
- [ ] `TODO THR-004` 类型化提取“谁提出了什么要求、谁反对、谁确认”。
- [ ] `TODO THR-005` 保存结论修订链，不允许新总结静默覆盖旧结论。
- [ ] `TODO THR-006` 建模 `open/waiting/resolved/closed/reopened` 生命周期和结束条件。
- [ ] `TODO THR-007` 生成跨群、跨私聊的线程候选链接，保存置信度和采用理由。
- [ ] `TODO THR-008` 同名人物、相似话题和相同文件名不得自动合并；低置信度请求 Owner 确认。
- [ ] `TODO THR-009` 跨会话检索前执行隐私策略过滤，并增加防泄露测试。
- [ ] `TODO THR-010` 支持查看线程来源、拆分错误线程、合并已确认线程和重新打开话题。

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
- [ ] `TODO CMD-003` 所有命令先通过 `OwnerCommand` 权限边界，非 Owner 默认拒绝。
- [ ] `TODO CMD-004` 定义类型化查询 Action：事件搜索、讨论总结、待回复和承诺列表。
- [ ] `TODO CMD-005` 定义类型化策略 Action：群提醒、重要联系人、静默时间和自动回复禁用。
- [ ] `TODO CMD-006` 定义反馈 Action：重要、不重要、类似消息规则和联系人策略。
- [ ] `TODO CMD-007` 定义记忆 Action：查看来源、修正、删除、TTL 和会话长期记忆开关。
- [ ] `TODO CMD-008` 高影响 Action 展示影响范围并使用 Agent Suspend/Resume 等待确认。
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
