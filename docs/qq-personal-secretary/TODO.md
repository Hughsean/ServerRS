# 个人 QQ 智能秘书 Todo

> 最后更新：2026-08-01
> 维护规则：完成项必须同步写入 `HISTORY.md`，不得仅勾选；新增具体事件使用
> `YYYY-MM-DD HH:mm（Asia/Shanghai）`，精确到分钟，不得用猜测时间回填旧事件。
> 当前开发阶段：在 `deepseek/qqbot-batch-snooze-follow-up-v1` 连续收口全部
> 可本地完成的 QQBot TODO。旧验收矩阵保留为历史证据，不再作为日常开发门禁；只执行与改动
> 风险相称的检查和隔离 MySQL 主路径验收。需要用户操作或 NapCat 实机的事项跳过但不伪造完成。

## 0. 当前完成与阻塞

- [x] `DONE QA-001` 建立独立于业务实现的机器验收基础设施：JSON 验收矩阵、L1-L6
  证据等级、P0/P1 合并门禁、精确测试发现、隔离 MySQL schema 生命周期、逐项日志以及
  JSON/Markdown 自动报告。首批 5 个黑盒验收测试已落地，报告只能由脚本生成。
- [x] `RETIRED QA-002` 旧机器验收矩阵不再作为日常开发和合并门禁；历史报告继续保留，当前按
  受影响 crate、关键状态机和隔离 MySQL 主路径验证，不再用 L4/L5 attestation 阻断业务推进。
- [x] `DONE QA-003` Release Hardening v1.1 代码侧收尾：attestation 的 .NET RSA-PSS
  合法签名、签名篡改、claim 篡改测试通过；Recall Spool backlog、最老记录年龄、容量占比、
  quarantine 数量和 allowlist 最近错误已接入 B7 第五子系统。隔离 MySQL 矩阵的全部 14 个
  具体检查均实际 PASS，但 8 个要求的最低 L4/L5 证据因缺少独立 attestation 而降为 L3，
  合并门禁保持 `REJECTED`。
- [ ] `BLOCKED QA-004` GitHub 管理侧尚未配置：protected Environment、受保护 runner 使用的
  固定可信公钥/签发密钥托管，以及 branch protection 的
  `QQBot Acceptance Gate / acceptance` required check。本轮仅记录，不伪造完成。
- [x] `DONE NPOLICY-007` Owner Notification Policy Feedback v1 的 Task 7 完成代码与 L3
  验收：`NPOLICY-PERSISTENCE-001`、`NPOLICY-MIGRATION-001`、
  `NPOLICY-EVALUATION-001`、`NPOLICY-DELIVERY-001`、
  `NPOLICY-RECONCILIATION-001` 均为 PASS。FollowUp/Agenda 扫描只生成
  Candidate/Request，不直接写 legacy Outbox；legacy Outbox 仅保留给明确的兼容状态机 fixture
  与全局租约协调。完整门禁仍为 `REJECTED`，不得据此声称 Release Gate 已批准。
- [x] `DONE B3-RECALL-004` `2026-07-29 23:29（Asia/Shanghai）` 修复 Recall WAL 恢复韧性
  验收注入器：删除 `secretary_recall_inbox` 后同步删除其测试迁移记录，确保恢复步骤确实重建
  inbox；Worker 无需新 Recall 即按现有周期重试，WAL 仅在 MySQL enqueue 成功后 checkpoint。
  `B3-RECALL-004-RESILIENCE` 实际 PASS（L3），五项 `NPOLICY-*` 仍 PASS；完整 Release Gate
  仍为 `REJECTED`，因 L4/L5 独立 attestation 缺失。隔离 schema 已清理，未连接或发送 QQ。
- [x] `DONE QA-005` `2026-07-30 10:19（Asia/Shanghai）` 验收脚本凭据 P1 已关闭：隔离 schema
  的创建和清理仅由容器内 `MYSQL_PWD="$MYSQL_ROOT_PASSWORD"` 展开认证，宿主机 `docker`
  命令行不再包含 root 密码。PowerShell 5.1 解析、`B3-RECALL-004-RESILIENCE` 隔离 MySQL smoke
  和 `git diff --check` 均通过；smoke schema `qqbot_accept_20260730101342_6c0e95c5` 已清理。
  当前发现 4 个既有遗留 schema，均未删除：`qqbot_accept_20260729142255_697e4cce`、
  `qqbot_accept_20260729214037_978a58d9`、`qqbot_accept_20260729214904_ba276172`、
  `qqbot_accept_20260730000216_ef44aea1`。

- [x] `DONE ID-002` 建模来源账号、会话、可信发送者、消息角色和账号作用域幂等键。
- [x] `DONE ID-003` 保证 NapCat Owner 消息只能是 Observation。
- [x] `DONE ING-001` NapCat 接收群聊和普通私聊。
- [x] `DONE ING-002` 接收并标记本人 `message_sent`。
- [x] `DONE ING-003` 保留 CQ `Reply`、`At`、文本和常见媒体消息段。
- [x] `DONE ARCH-001` QQ 协议、个人秘书业务和数字人业务保持 crate/进程隔离。
- [x] `DONE ENV-001` 使用隔离 Docker MySQL 完成独立 QQBot 数据库集成验收。
- [x] `DONE ENV-005` 已按本地 QQBot 配置建立持久化 MySQL 8.4 运行容器
  `serverrs-qqbot-mysql`、数据库 `qq_personal_secretary` 和独立 Docker 卷；只绑定
  `127.0.0.1:3306`，强制 TLS，11 个迁移按依赖顺序执行并通过幂等复跑，项目自身已完成真实
  连接验收。未修改数字人 `docker-compose`、`init.sql` 或数据库。
- [x] `DONE ENV-006` NapCat HTTP/WebSocket 统一采用本机无 Token 模式。QQBot 已删除
  `http_token`、`NAPCAT_HTTP_TOKEN` 和 HTTP Authorization 装配；配置层强制两个地址只能使用
  loopback，并拒绝 URL 凭据、查询 Token 和片段。HTTP `13990` 状态接口、WebSocket `13991`
  真实握手及 qqbot-server/MySQL/Ollama 组合启动均已通过。
- [x] `DONE LLM-001` 已建立 QQBot 独立 `[llm]` 配置、OpenAI-compatible/Ollama 客户端和有界
  线程语义提取垂直切片。API Key 不进入 TOML；远程端点强制 HTTPS；输入字符、输出 Token、
  响应字节、超时和候选数均有上限。模型只能产生引用当前批次 `source_event_id` 的候选 DTO，
  不能直接读写数据库、调用工具或发送消息。本机 Ollama `qwen3:14b` 已通过正常中文请求和
  提示注入正文实机测试；Qwen3 使用显式 `qwen_no_think` 方言避免思考内容耗尽 JSON 输出预算。
- [ ] `BLOCKED ENV-002` 用户提供的 QQ 开放平台凭据已暴露，禁止用于上线；先在平台轮换 Secret，
  再通过本地环境变量或忽略文件配置替换凭据，不得写入文档或 Git。
- [ ] `PARTIAL ENV-003` 两个 NapCat 测试账号均在线；已在唯一获批测试群实测未 @、@、本人
  消息、跨账号 Reply、撤回通知、双 WebSocket 和主动重连。测试临时开启的本人消息上报与第二
  实例 WebSocket 已逐字恢复，未测试私聊主动发送；群免打扰状态是否影响上报仍需单独取证。
  2026-07-25 先确认 `6099` 只是 WebUI，随后当前账号启用无 Token HTTP `13990` 和 WebSocket
  `13991` 并通过组合连接；仍待用一条新真实消息确认本轮“自身消息上报”配置和完整派生链。
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
  Worker；WebSocket 回调不再等待 MySQL。批处理和 LLM 消费链已实现（线程投影/语义/关联
  Worker）。并发优雅关闭（`RuntimeWorkers` + 25s 全局 deadline）和可编程运行时入口
  （`run_with_cancellation` + watch 信号）已完成；E2E 真实消息验收已通过
  （SourceEvent->EventThread->LLM proposed 候选->精确来源证据链完整，重启幂等性验证通过）。
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
- [x] `DONE GAP-006` 新增 Owner 只读 `GetSecretaryStatus` Action，按被管理账号展示未闭合空窗、
  仍开放空窗与最早起点；文案明确区分“无未闭合空窗”和“仍存在不确定/不可恢复空窗”，不会
  把传输重连表述为历史已补齐。结果进入既有 OwnerResponseDraft，不新增 QQ 写接口。
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
- [ ] `PARTIAL THR-004` 已实现协议无关提取端口、保守批量提取器、可选 LLM 批量提取器和 MySQL
  候选闭环。模型只消费有界线程事件，不接收完整历史；返回的发言人和来源必须映射到当前批次，
  再经过领域来源/身份/数量/修订链校验后形成 `proposed` 候选。本机 Qwen3 单请求质量冒烟已
  通过；Owner 现可按 Thread ID 有界查询参与者、要求/意见、结论、开放问题及来源 ID。跨线程
  排序检索、复杂指代和成组质量基准仍待实现。
- [ ] `PARTIAL THR-005` 已持久化结论与 `supersedes_id` 唯一修订链，并禁止候选引用本线程外
  或非 confirmed 旧结论；Owner 可经 L2 审批确认或撤销结论，变更、命令来源和 Effect Receipt
  在同一事务中提交。单线程查询已展示有界结论及来源，完整修订链分页仍待实现。
- [ ] `PARTIAL THR-006` 已实现 `open/waiting/resolved/closed/reopened` 状态机、不可变状态历史和
  来源表；Owner 可在忽略/回答开放问题后经 L2 审批关闭线程，也可从 closed/resolved 重开。
  最终事务复验账号绑定、命令、租约和期望状态；自动结束条件仍待实现。
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
  Effect ID 重复执行幂等，提交结果不明不得自动重放。现已增加 Owner 授权撤销、不可变撤销
  审计、Alias/Override 停用、关联提示刷新、旧 proposed 候选过期、语义失效证据和游标重置。
  已确认语义的人工迁移/重新确认与重新打开话题仍待实现。

验收：给定多群和私聊的同一项目消息，系统能输出来源明确的要求、分歧、结论和未决问题；
错误关联可撤销，且不会向第三方会话泄露内容。

## 4. 人物、项目与承诺记忆（P0）

- [x] `DONE MEM-001` 定义 `MemoryFact` 公共字段：类型化 Payload、状态、置信度、来源、TTL 和版本。
- [ ] `PARTIAL MEM-002` 已定义 `PersonMemory` 的稳定 Actor、关系、职责和沟通偏好；别名与权限
  仍待 ParticipantIdentity/Owner 控制面接入。
- [ ] `PARTIAL MEM-003` 已定义 `ProjectMemory` 的目标、成员、进展、决定引用、风险、阻塞和
  Artifact 引用；自动提取与项目查询仍待实现。
- [ ] `PARTIAL MEM-004` 已定义 `CommitmentMemory` 的承诺人、受益人、动作、期限、状态和完成
  来源证据；已确认且有期限的承诺会进入持久化 Scheduler，自动提取/确认入口仍待实现。
- [x] `DONE MEM-005` 长期记忆版本通过 `secretary_memory_fact_sources` 引用无损 SourceEvent，
  不把完整聊天轨迹复制进事实 JSON。
- [x] `DONE MEM-006` Owner/控制面可按 Fact ID 有界回读原始事件最小片段、会话、Actor 和时间；
  正文策略不允许时不返回片段。
- [x] `DONE MEM-007` 同账号、同类型、同 subject 使用不可变单向修订链；旧版本标记为
  `superseded`，跟进 Scheduler 会重算并抑制旧通知。
- [x] `DONE MEM-008` 已实现经本地绑定授权的 `OwnerCommand` 删除指定派生记忆及不可变审计；
  操作明确不隐式删除原始聊天记录，避免把“忘记派生记忆”误做成不可恢复的数据清除。
- [x] `DONE MEM-009` TTL 查询过滤、有界到期标记、常驻周期清理 Worker 与
  `never_long_term`/`envelope_only` 写边界均已实现。
- [x] `DONE MEM-010` 同 subject 出现并行新事实时拒绝静默覆盖，必须先回读来源并显式提供
  `supersedes_fact_id`，形成可审计冲突修订。

验收：人物、项目和承诺均为结构化状态；每项事实可查来源、可修正、可删除、可过期；设置为
`never_long_term` 的会话不会生成长期记忆。

## 5. 主动跟进和提醒（P0）

- [x] `DONE FUP-001` 已定义承诺跟进与来源化 `ResponseExpectation`：开放问题、线程、提出者、
  来源版本、期限和状态均持久化；两者统一进入 Notification Candidate/Decision/Outbox 规则链。
- [ ] `PARTIAL FUP-002` 已确认承诺与等待回复均生成幂等候选；等待回复会以“是否需要处理”的
  Owner 提醒进入策略链。对未到期候选进行提前确认/批量确认仍待控制 Action。
- [x] `DONE FUP-003` 常驻 Scheduler 已支持临近/逾期承诺、外部联系人问题超时未回复，以及
  来源化项目阻塞事实持续 24 小时后的升级；三类来源均有有界扫描、幂等候选、指数退避和快速关闭。
- [x] `DONE FUP-004` 承诺完成、取消、事实删除/到期/修订，以及开放问题已回答、线程终态或
  同线程出现本人后续回复，都会以来源版本 fencing 终止事项并抑制尚未发送的通知。
- [x] `DONE FUP-005` 统一通知策略已支持账号/会话/联系人/类别优先级、跨午夜与 DST 安全静默
  时段、显式双重 bypass、重要联系人和确定性元数据规则；Delay 到期会按当前策略重评。
- [x] `DONE FUP-006` 每个来源承诺/理由只生成一个 FollowUp，每个 FollowUp/通知类型只生成一个
  Outbox occurrence，重复扫描不重复入队。
- [ ] `PARTIAL FUP-007` 平台无关 Outbox 已接入按账号隔离领取、租约 fencing、指数退避、送达
  回执和 `unknown_commit`；官方 Bot 只向配置的 Owner OpenID 发送。隔离 MySQL 已覆盖跨账号、
  错误租约、重试、送达和提交结果不明；真实 QQ 投递仍待替换凭据后的联机验收。
- [ ] `PARTIAL FUP-008` Agenda 写操作已支持 Owner 确认/拒绝、稍后提醒、完成、取消和改期；
  全部经 L2 Suspend/Resume、账号验权、单次消费、版本 fencing 和不可变审计。Owner 现可按
  FollowUp ID 与来源版本忽略或推迟单条通用跟进：旧通知被压制，到达新时间后按新来源版本
  重新进入统一策略求值；也可一次性按明确 ID/版本全有或全无地忽略或统一推迟最多 20 条跟进。
  提前确认/批量确认等其他批处理仍待实现。Owner 也可忽略线程开放问题并关闭/重开线程。
- [x] `DONE FUP-009` 每次策略求值追加类型化 Decision，区分 remind/delay/suppress/过期/终态
  失败；Owner 可解释查询决策原因，并记录重要/不重要反馈及受限的长期规则提升。
- [x] `DONE FUP-010` NapCat 业务路径不含主动发送；官方通道也只消费 Owner 通知 Outbox，禁止
  自动催促客户、负责人或群成员。

验收：服务重启后提醒不丢不重；报价截止、客户久未回复等场景能依据结构化证据提醒 Owner，
并且不会自动代表 Owner 联系第三方。

## 6. Owner 自然语言控制（P0）

> 进入 `CMD-001/CMD-002`、开始连接 QQ 开放平台前，必须先明确通知用户并等待其提供/确认
> 本地凭据配置；禁止把 App ID、Secret 或 Owner OpenID 写入 Git。

- [x] `DONE CMD-001` 新增独立 `qq-open-platform` 协议 crate，与 NapCat、个人秘书和数字人隔离。
- [ ] `PARTIAL CMD-002` 已实现 App 凭据换取、Gateway Identify/Resume/Heartbeat、C2C/群事件映射、
  原始信封持久化后推进 sequence、Owner OpenID 绑定和官方 C2C 通知；真实联机和交互回执待验收。
- [ ] `PARTIAL CMD-003` 线程关联审核已强制验证 `OwnerCommand`、本地 Owner 账号绑定及同一
  被管理账号；普通观察、未绑定和跨账号命令默认拒绝。其余命令类型仍须逐项接入同一边界。
- [ ] `PARTIAL CMD-004` 已接入事件检索、来源回读、线程检索、指代解析、近期事项、提醒草稿、
  Owner 澄清及类型化 Agenda Action；支持创建日程/任务/提醒、查询、改期、稍后提醒、完成和
  取消；新增秘书状态、待处理事项和单线程因果上下文三类 L0 查询，并支持确认/撤销线程结论、
  忽略开放问题、关闭/重开线程，以及按稳定 ID/版本忽略或推迟单条 FollowUp。L2 写操作经既有
  Suspend/Resume 审批后写入 MySQL，并以版本化 Outbox 仅通知 Owner；真实 QQ 开放平台联机
  投递仍待凭据确认后的人工验收。
- [x] `DONE CMD-005` 已定义并接入类型化策略 Action：账号/群会话提醒、重要联系人、静默时间、
  类别优先级和独立的联系人自动回复禁用门，所有变更均为 L2 Suspend/Resume。
- [x] `DONE CMD-006` 已定义并接入重要/不重要反馈、确定性“类似消息”规则和联系人策略 Action；
  匹配键不含正文，关键元数据未知时禁止提升长期规则。
- [x] `DONE CMD-007` 已接入六类记忆 Action：列出记忆、查看有界来源、不可变修正、删除派生
  记忆、以新版本设置/取消 TTL、按账号与会话设置长期记忆模式。写操作均为 L2 审批，修订 ID
  由 Effect ID 确定性生成以支持崩溃后幂等重放；真实隔离 MySQL 已验证授权模式切换与重放。
- [ ] `PARTIAL CMD-008` 线程拆分/合并后端已展示精确有界影响范围，由节点主动返回类型化
  `NodeResult::Suspend(Approval)`；Proposal、QQBot 独立 MySQL Checkpoint、进程重启后 Resume、
  `OwnerCommand`/本地绑定验权、Effect Receipt 和拒绝零 Effect 均已接入。QQ 开放平台自然
  语言控制入口尚未开发，进入 `CMD-001/CMD-002` 前仍须先通知用户并配置本地凭据。
- [ ] `PARTIAL CMD-009` 已建立无完整思维文本的有界结构状态：固定目标/约束、最多 8 条近期事件
  引用、最多 100 条证据引用和单一待处理 Proposal；Owner Planner 已接入 24 小时有界检索，
  但跨阶段检查点摘要、长期事件检索排序和冲突驱动回读仍待实现。
- [ ] `PARTIAL CMD-010` LLM 语义切片已把聊天正文标记为不可信输入，并覆盖越界来源、隐私省略
  正文不入模、未知字段拒绝和候选数量上限；真实 Qwen3 提示注入正文也只能得到领域校验通过的
  当前批次候选或安全拒绝。Owner Action Retriever 已覆盖账号隔离和
  `normal/local_only/envelope_only/never_long_term` 严格度矩阵；Owner 身份越权、Prompt 注入、
  跨会话指代歧义的端到端矩阵仍待实现。

## 7. 控制面、可观测性和生产强化

- [ ] `PARTIAL OPS-001` 已通过内部 reader 与周期结构化日志展示 WebSocket、历史完整性、MySQL、
  Worker 和 Recall Spool 健康状态；Owner 只读 Action 已展示账号级 Gap、线程、跟进、通知求值
  和 Outbox 状态。运行期 WebSocket/Worker/Spool 快照仍待安全地并入 Owner 查询。
- [ ] `PARTIAL OPS-002` 已为连接周期、入队、幂等结果、重试、溢出 Gap、Worker 排空、官方
  Gateway/Token/Outbox、线程 Effect/撤销、LLM 输入/响应预算与 Token Usage、结构记忆写入/
  过期/来源回读/删除和跟进扫描增加结构化日志；Recall Spool 已提供 backlog、最老记录年龄、
  容量占比、quarantine 数量与 allowlist 最近错误。控制面展示和回补进度仍待实现。
- [ ] `PARTIAL OPS-003` 已通过类型化 Owner Action 展示记忆及来源、待处理承诺/项目阻塞/回复
  期待/Agenda、Outbox 异常、线程参与者、要求、结论、开放问题和来源 ID；分页、跨线程聚合和
  运行期健康详情仍待完成。
- [ ] `PARTIAL OPS-004` 已提供记忆修正/删除、通知策略、Agenda 和线程语义/生命周期入口，以及
  单条 FollowUp 忽略/推迟入口；写操作均有不可变审计与 Effect Receipt；失败派生任务的有界
  重新处理入口仍待实现。
- [ ] `TODO OPS-005` 建立吞吐、延迟、LLM 调用率、误关联和提醒误报指标。
- [ ] `TODO OPS-006` 压测高流量群，验证背压、批处理和成本上限。
- [ ] `TODO OPS-007` 备份恢复、密钥轮换、数据导出和彻底删除演练。

## 8. 后期扩展

- [ ] `DEFERRED KB-001` 文档/网页个人知识库和向量检索。
- [ ] `DEFERRED MM-001` 图片、语音和文件内容理解。
- [ ] `DEFERRED AUTO-001` 代表 Owner 向第三方自动回复；必须重新进行权限和风险评审。
