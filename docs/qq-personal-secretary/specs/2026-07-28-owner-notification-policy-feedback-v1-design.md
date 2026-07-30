# Owner 通知策略与反馈学习 v1 设计

- 日期：2026-07-28
- 分支：`gpt/qqbot-owner-policy-feedback-v1`
- 基线：`main@39f35b3`
- 范围：`CMD-005`、`CMD-006`、`FUP-005`、`FUP-009`

## 目标与边界

Owner 通过自然语言管理提醒策略，并对单条提醒标记重要或不重要。系统只决定是否向 Owner 创建通知候选的 Owner-only Outbox 记录；不调用 QQ、不向第三方发消息、不新增 HTTP 管理面，也不连接真实 QQ 平台。

`personal-secretary` 保存协议无关的领域模型、求值器、用例和端口；`qqbot` 与 `qq-open-platform` 仅做协议映射；`qqbot-server` 实现 MySQL、Planner、Worker 和装配。所有迁移仅加入 `apps/qqbot-server/database/migrations`，测试仅连接随机 `qqbot_accept_*` schema。

## 策略与数据模型

新增协议无关 `notification_policy` 领域模块：

- `NotificationPolicyFamily`：稳定身份，以 `(account_id, canonical_scope_key, policy_kind)` 唯一；保存 `current_revision_id` 与单调 `generation`。账号另有单调 `policy_epoch`：任何 Family 创建、Head 切换或停用都在同一事务递增；求值读取该快照，最终提交 CAS 验证，防止“未命中任何策略”后并发创建 Family 的幻读。
- `NotificationPolicyRevision`：完全不可变的规则或 tombstone，包含 `revision_kind=rule|tombstone`、`policy_family_id`、`policy_revision_id`、`revision_number`、`supersedes_revision_id`、OwnerCommand 来源和审计时间；不保存可变的 active/disabled/superseded 状态。有效性、停用与替代均由 Family Head 推导；停用写 tombstone revision，再以 generation CAS 切换 Family Head，历史 Revision 永不 UPDATE。
- `NotificationMatchKeyV1`：只使用账户、会话、联系人、通知类别、是否 @Owner、结构化重要性、事件类型。每个可选匹配字段统一为 `Known(T) | Absent | Unknown`；`Unknown` 不得降级为 `Absent`。不含正文、主题、LLM、Embedding 或向量数据。关键元数据未知时，单条反馈不得提升为长期规则。
- `NotificationFeedback`：目标候选或来源事件、important/unimportant、是否提升规则、OwnerCommand 幂等键和审计引用；没有正文。
- `NotificationCandidateRef`：`source_kind/source_id/source_version/account_id`，所有重评均做版本 fencing。
- `EvaluationRequest`：稳定 `evaluation_request_id`、候选及版本、`evaluation_generation`、触发类型、租约令牌和尝试次数。Decision 对其唯一，领取、续租、提交使用 fencing 与过期回收。
- `NotificationDecision`：追加式记录，引用候选、前一 Decision、命中 revision、求值器版本、结果、类型化原因、`next_allowed_at` 与时间。

自动回复拒绝是独立策略种类，严格绑定 `account_id + actor_id`，不按昵称匹配。可复用 Family/Revision 持久化框架，但使用独立 `AutomaticReplyPolicyGate`，不得参与 `NotificationPolicyEvaluator`。未来自动回复的身份不明或策略读取失败默认拒绝；联系人级 denied 优先于未来全局允许。该策略默认无 TTL，但可撤销。

Owner 显式配置可在 `never_long_term/envelope_only` 会话中保存，因为其不依赖聊天正文；这些内容策略只禁止由单条反馈或聊天内容推导长期相似规则。JSON、canonical scope key、类型化原因、审计摘要和 Response Artifact 都有明确字节上限，并在领域校验与 MySQL 约束双重执行；持久化和日志禁止写入原始 OpenID、Token、数据库 URL、聊天正文、模型推理及未脱敏错误原文。

## 策略求值

统一 `NotificationPolicyEvaluator` 是纯函数，按以下顺序决定结果：

1. 候选有效性；
2. 会话完全静默硬门；
3. 仅当候选命中策略 `bypass_quiet=true` **且**会话静默策略 `allow_bypass=true` 时，允许突破会话完全静默；
4. 会话策略；
5. 联系人策略；
6. 通知类别策略；
7. 账号默认策略；
8. 静默时段；
9. `remind/delay/suppress`。

同级使用 Family Head，不依赖 `created_at` 选最新；冲突时保守抑制。静默时段存储为 `timezone_name`（有效 IANA 名称）、本地 `start_local_time/end_local_time`、重复规则 `every_local_day` 与可选 `effective_from_local_date/effective_until_local_date`。v1 仅接受每日本地重复时段：创建/变更时用注入 Clock 检查从生效日到未来 400 天内每个 DST 转换日；若某一日的开始或结束本地时间为重复或不存在时间，则拒绝配置并要求 Owner 指定不触及转换日的有效日期范围，或改用无歧义时间。求值若命中尚未预检的转换日或 tzdb 变化导致歧义，保守禁止发送，产生可解释 `schedule_time_ambiguous`，要求 Owner 重新确认规则；这不是 `evaluation_failed_terminal`。重要联系人只有显式 `bypass_quiet` 且满足会话双重授权时才可突破静默。

“以后类似消息”仅表示版本化、可解释、可停用的 MatchKeyV1 确定性规则。创建前展示匹配范围，范围不明确必须澄清，创建和提升均经 L2 Suspend/Resume。

## 事务、调度与 Outbox

Agenda 和 FollowUp 先生成稳定候选，再在既有 Owner-only Outbox 前调用求值。不得把 Claim、读取、纯函数求值与提交保持在同一长事务中；严格采用三个阶段：

1. **领取短事务**：以 `evaluation_request_id` 和状态 CAS 写入 `lease_token`、租约期限与 attempt；仅成功领取者继续。
2. **事务外有界读取与纯函数求值**：读取候选当前版本、账号 `policy_epoch` 和 Family Head `generation` 快照，执行不含 IO 的有界求值；不持有数据库锁。
3. **提交短事务**：再次验证租约 token、候选版本、账号 `policy_epoch`、每个读取 Family 的 generation 快照，以及 OwnerBinding 和 Owner-only Outbox 接收方；仅全部一致时，原子写入 Decision 与后续状态。任一验证失败时拒绝旧计算，由新的 EvaluationRequest 重评。

最终提交的原子结果为：

- `remind`：Decision + 基于 `source_kind/source_id/source_version/notification_kind` 的稳定 Occurrence ID 的 `INSERT IGNORE` Outbox；
- `delay`：Decision + 延迟调度状态；
- `suppress`：Decision + 候选终态。

延迟到期重新读取候选和当前 Family Heads，创建新的 EvaluationRequest 与 Decision；不得复用旧结果。候选完成、取消、到期、撤回或被替代时记录 `candidate_expired`。持续基础设施失败记录 `evaluation_failed_terminal`；超过投递窗口记录 `delivery_window_expired`；DST/tzdb 规则时间歧义记录 `schedule_time_ambiguous` 并要求重新确认；只有策略明确阻止才记录 `suppress`。这些终态均可解释，不能把故障伪装为策略抑制。重评失败有界退避和保留期/次数上限，服务重启后继续运行，不依赖内存定时器。

## Action Graph、授权与响应

新增类型化 Actions：`ListNotificationPolicies`、`ExplainNotificationDecision`、`SetAccountDefaultNotificationMode`、`SetConversationNotificationMode`、`SetQuietHours`、`SetImportantContact`、`SetNotificationCategoryImportance`、`RecordNotificationFeedback`、`CreateSimilarNotificationRule`、`DisableNotificationPolicy`、`SetAutomaticReplyDeniedForContact`。`DisableNotificationPolicy` 接受类型化 `PolicyFamilyId`，可撤销任意策略 Family（含自动回复拒绝）；不以歧义的“DisableAutomaticReply”命名创建拒绝策略。

查询/解释是 L0；明确单条反馈是 L1；持久策略、静默时段、重要联系人、相似规则和自动回复拒绝是 L2，复用既有 Suspend/Resume、持久化 Checkpoint、单次 CAS 和 Effect Receipt。Planner、Action Gate、Resume、Effect 均调用同一个领域授权函数；Effect 是最终强制边界。OwnerBinding、OwnerCommand、同账户校验始终必需，聊天正文不能越过 OwnerCommand 边界。

查询、解释与已确认写入的 Effect 均通过现有 Action Graph 产生有界 `OwnerResponseDraft/Response Artifact`，关联账户、OwnerCommand、Action Run、Policy/Decision ID 和审计引用。响应仅含类型化摘要、作用域、状态、优先级和原因。L2 仅在 Resume 后 Effect Receipt 证实成功时回复“已生效”；UnknownCommit 只能回复类型化不确定状态，不乐观成功、不自动重放。

## 验证

领域单测覆盖三态匹配、优先级、会话双重 bypass 授权、跨午夜、上述固定 IANA 时区与具体 DST 转换日期、候选版本 fencing、反馈提升限制、解释、自动回复门隔离，以及身份未知和仓储失败时 `AutomaticReplyPolicyGate` 默认拒绝。时间测试使用注入 Clock 与固定 IANA/DST 日期，不用本机时区或当前时间。

随机隔离 MySQL 集成测试覆盖 Owner/账户/重复命令、L2 Suspend→重启→Resume、二次 Resume CAS 拒绝、Family Head CAS、无策略读取后并发创建 Family 使旧 `policy_epoch` 求值提交被拒绝、并发策略变更拒绝旧 generation 的求值提交、Effect Receipt、UnknownCommit/LeaseLost、Agenda/FollowUp Outbox 前置求值、延迟重评与终态、查询/解释/停用响应产物、提示注入无效、最终事务重验 OwnerBinding 与 Owner-only Outbox 接收方、无 QQ 与第三方发送。

原子性和故障注入还必须覆盖：Decision 写入前失败、Decision 与 Outbox 的提交边界失败、并发 Worker 领取、旧租约迟到提交、UnknownCommit 后重新观察数据库状态、重启后重复领取。不得出现无 Decision 的 Outbox，或同一 Occurrence 的多条有效 Outbox。

执行 `cargo fmt --all -- --check`、严格 clippy、三个 crate 测试、`digital-human-server --test workspace_boundaries`、`git diff --check`，并删除测试 schema。更新 TODO、HISTORY、月度历史和必要的 capability assessment，以当前代码与测试证据校正 QA-002、B3/B4/B6/B7、Recall Spool、Artifact、Gap 状态；记录分钟级基线、影响、验证、Git 状态和下一阶段。
