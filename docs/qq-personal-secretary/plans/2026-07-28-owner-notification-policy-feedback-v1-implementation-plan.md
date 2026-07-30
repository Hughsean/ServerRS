# Owner 通知策略与反馈学习 v1 实施计划

> Steps use checkbox (`- [ ]`) syntax for tracking. 本计划明确禁止使用 Superpowers 与子 agent；实现期间不提交、不推送、不合并。

**Goal:** 为 QQBot 实现 Owner-only 的通知策略、反馈学习和可解释重评闭环，覆盖 `CMD-005`、`CMD-006`、`FUP-005`、`FUP-009`。

**Architecture:** `personal-secretary` 新增协议无关 `notification_policy` 领域、用例和端口；其纯求值器不执行 I/O。`qqbot-server` 提供 MySQL 迁移、仓储、短事务 Evaluation Worker 和 Action Graph 装配；Agenda/FollowUp 只生成候选并由该 Worker 决定是否写入既有 Owner-only Outbox。复用既有 Action Run、Checkpoint、Suspend/Resume、Effect Receipt、OwnerBinding 和 Outbox 投递状态机，不改 `qqbot`、`qq-open-platform` 的协议边界。

**Tech Stack:** Rust workspace、Tokio、SeaORM/MySQL 8、serde、chrono/chrono-tz、现有 `agent-core` Action Graph、随机隔离 `qqbot_accept_*` schema。

## Global Constraints

- 不调用 QQ、不向第三方发消息、不连接真实 QQ 平台、不使用 QQ 凭据、不新增 HTTP 管理面。
- 仅修改 QQBot/Personal Secretary 路径；不触碰数字人、HP、数字人数据库或根目录 `config.example.toml`。QQBot 配置只允许修改 `apps/qqbot-server/config/qqbot.example.toml`。
- 所有迁移仅加入 `apps/qqbot-server/database/migrations`；MySQL 测试仅使用随机 `qqbot_accept_*` schema，并在测试结束删除。
- 策略与审计禁止保存聊天正文、OpenID、Token、数据库 URL、模型推理或未脱敏错误；所有 JSON、scope key、原因和 Artifact 都做领域与 SQL 双重字节上限。
- `NotificationPolicyRevision` 永远不可变；启用、替换和停用仅通过 Family Head 指向 rule/tombstone Revision 推导。
- 所有可选匹配字段使用 `Known(T) | Absent | Unknown`；绝不把 `Unknown` 降为 `Absent`。
- Family 创建、Head 切换和停用必须在同一事务递增账号 `policy_epoch`；评估提交 CAS 验证读取到的 epoch，防止无策略读取后的 phantom Family。
- DST/tzdb 规则时间歧义的类型化原因只能是 `schedule_time_ambiguous`，保守禁止发送并要求 Owner 重新确认；不得写为 `evaluation_failed_terminal`。
- 不持有数据库事务、连接或锁跨纯函数求值；领取、读取求值、提交严格为三阶段。
- 不新增第二套发送/投递状态机；仅在已有 `secretary_notification_outbox` 写 Owner-only 行，领取与投递继续由既有官方平台循环负责。
- L2 仅经现有 Suspend/Resume 与 Effect Receipt 确认后回复“已生效”；UnknownCommit 仅返回类型化不确定状态，绝不乐观成功或自动重放。
- 不执行 `git commit`。计划中的“提交”步骤替换为 `git diff --check` 和阶段性自检，以符合仓库规则。

---

## File structure

| 路径 | 职责 |
|---|---|
| `crates/personal-secretary/src/notification_policy.rs` | ID、三态字段、候选、Family/Revision、反馈、Decision、时段、类型化校验与纯求值器。 |
| `crates/personal-secretary/src/notification_policy_service.rs` | 用例编排、端口、三阶段评估契约与统一授权函数。 |
| `crates/personal-secretary/src/infra/repo/mysql_notification_policy.rs` | MySQL 实现：Family CAS、epoch、评估领取/提交、Outbox 原子写入。 |
| `crates/personal-secretary/src/infra/repo/entities/secretary_notification_*.rs` | 新表 SeaORM entity，严格映射 `BIGINT UNSIGNED -> u64`。 |
| `crates/personal-secretary/src/agent_runtime/{action,validation}.rs` | 新 Action、风险等级、Proposal 的有界结构校验。 |
| `crates/personal-secretary/src/action_graph/effect_executor.rs` | 最终 Effect 授权和通知策略用例调用。 |
| `crates/personal-secretary/src/{planner,planner_service}.rs` | Planner 白名单、Action Graph 返回真实 Response Draft。 |
| `apps/qqbot-server/database/migrations/20260728_owner_notification_policy_feedback_v1.sql` | 所有策略/候选/请求/Decision/反馈表、Outbox 扩展、约束、索引。 |
| `apps/qqbot-server/src/notification_policy_worker.rs` | 可取消的 Evaluation Request 扫描 worker，退避且不直接发送。 |
| `apps/qqbot-server/src/bootstrap/notification_policy.rs` | MySQL store/use case/worker 装配。 |
| `apps/qqbot-server/src/{runtime/mod.rs,bootstrap/workers.rs,config/workers.rs}` | 启动、关闭和配置接线。 |
| `apps/qqbot-server/config/qqbot.example.toml` | QQBot 独立 Worker 配置样例；不得修改根目录 `config.example.toml`。 |
| `apps/qqbot-server/database/test_support/qqbot_migrations.rs` | 共享且唯一的测试迁移加载器：稳定排序、迁移记录表和幂等哨兵；四个 MySQL 测试入口通过 `#[path = "..."]` 复用。 |
| `apps/qqbot-server/tests/{qqbot_acceptance_runtime.rs,qqbot_acceptance_migrations.rs}`、`crates/personal-secretary/tests/{qqbot_acceptance_mysql.rs,mysql_ingestion.rs,mysql_action_planner.rs}` | 隔离 MySQL 真实链路、并发、故障注入与无外发验收。 |
| `docs/qq-personal-secretary/{TODO.md,HISTORY.md,specs/...}` | 证据驱动更新任务、能力状态和规格。 |

计划中的 `TODO.md` 只是现有文档文件名，不是未决实现占位符。

### Task 1: 建立通知策略领域类型与三态 MatchKey

**Files:**
- Create: `crates/personal-secretary/src/notification_policy.rs`
- Modify: `crates/personal-secretary/src/lib.rs`
- Test: `crates/personal-secretary/src/notification_policy.rs` (`#[cfg(test)]`)

**Consumes:** `SourceAccountRef`、`SourceEventId`、`ConversationRef`、`Clock`。

**Produces:** `PolicyFamilyId`、`PolicyRevisionId`、`NotificationCandidateId`、`EvaluationRequestId`、`NotificationDecisionId`、`MatchField<T>`、`NotificationMatchKeyV1`、`NotificationCandidateRef`、`NotificationPolicyEvaluator`。

- [ ] **Step 1: 写失败的领域测试。**

```rust
#[test]
fn unknown_match_field_never_matches_absent_field() {
    assert!(!MatchField::<bool>::Unknown.matches(&MatchField::Absent));
}

#[test]
fn feedback_with_unknown_required_metadata_cannot_promote_rule() {
    let key = NotificationMatchKeyV1::new(
        account(), MatchField::Unknown, MatchField::Known("actor-1".into()),
        MatchField::Known(NotificationCategory::Agenda), MatchField::Known(true),
        MatchField::Known(StructuredImportance::Normal), MatchField::Known(EventKind::AgendaDue),
    ).unwrap();
    assert_eq!(key.eligibility_for_long_term_rule(), Err(NotificationPolicyError::UnknownMatchMetadata));
}
```

- [ ] **Step 2: 运行失败测试。**

Run: `cargo test -p personal-secretary notification_policy::tests::unknown_match_field_never_matches_absent_field`

Expected: FAIL，因为 `notification_policy` 模块与 `MatchField` 尚不存在。

- [ ] **Step 3: 实现最小且完整的领域骨架。**

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", content = "value", rename_all = "snake_case")]
pub enum MatchField<T> { Known(T), Absent, Unknown }

impl<T: PartialEq> MatchField<T> {
    pub fn matches(&self, actual: &Self) -> bool {
        match (self, actual) {
            (Self::Known(expected), Self::Known(value)) => expected == value,
            (Self::Absent, Self::Absent) => true,
            _ => false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotificationMatchKeyV1 {
    pub account: SourceAccountRef,
    pub conversation_id: MatchField<String>,
    pub actor_id: MatchField<String>,
    pub category: MatchField<NotificationCategory>,
    pub mentioned_owner: MatchField<bool>,
    pub structured_importance: MatchField<StructuredImportance>,
    pub event_kind: MatchField<EventKind>,
}
```

实现每个 ID 的 `new/generate/as_str`，所有 UUID ID 上限 36 bytes；scope key 上限 512 bytes；原因、审计摘要、JSON 上限分别定义为命名常量。`NotificationCandidateRef` 必须只包含 `source_kind/source_id/source_version/account`，不含正文。对提升规则调用 `eligibility_for_long_term_rule()`：任一 required field `Unknown` 返回 `UnknownMatchMetadata`。

- [ ] **Step 4: 添加候选、规则、Decision 类型。**

```rust
pub enum RevisionKind { Rule, Tombstone }
pub enum NotificationOutcome { Remind, Delay, Suppress, CandidateExpired,
    EvaluationFailedTerminal, DeliveryWindowExpired, ScheduleTimeAmbiguous }
pub struct NotificationPolicyFamily { pub policy_family_id: PolicyFamilyId,
    pub account: SourceAccountRef, pub canonical_scope_key: String,
    pub policy_kind: NotificationPolicyKind, pub current_revision_id: PolicyRevisionId,
    pub generation: u64 }
pub struct NotificationPolicyRevision { pub policy_revision_id: PolicyRevisionId,
    pub policy_family_id: PolicyFamilyId, pub revision_number: u64,
    pub supersedes_revision_id: Option<PolicyRevisionId>, pub revision_kind: RevisionKind,
    pub rule: Option<NotificationPolicyRule>, pub command_source_event_id: SourceEventId }
```

`NotificationPolicyRevision` 不得声明 active、disabled、superseded 字段。若为 tombstone，`rule=None`；若为 rule，`rule=Some`，二者互斥。导出模块但不暴露基础设施类型。

- [ ] **Step 5: 运行领域测试与格式检查。**

Run: `cargo test -p personal-secretary notification_policy && cargo fmt --all -- --check`

Expected: PASS。

- [ ] **Step 6: 阶段性自检，不提交。**

Run: `git diff --check`

Expected: exit 0。

### Task 2: 实现纯策略优先级、静默时段与 DST 原因

**Files:**
- Modify: `crates/personal-secretary/src/notification_policy.rs`
- Test: `crates/personal-secretary/src/notification_policy.rs`

**Consumes:** Task 1 的 `NotificationMatchKeyV1`、Family Head rule、注入 `Clock`。

**Produces:** `NotificationPolicyEvaluator::evaluate(&EvaluationInput) -> EvaluationPlan`、`QuietHoursRule::validate`、`DecisionReason::ScheduleTimeAmbiguous`。

- [ ] **Step 1: 写优先级和 DST 失败测试。**

```rust
#[test]
fn fully_silent_conversation_requires_two_explicit_bypass_grants() {
    let decision = evaluator().evaluate(&input_with(
        conversation_rule(ConversationMode::FullySilent, false),
        important_contact_rule(true),
    ));
    assert_eq!(decision.outcome, NotificationOutcome::Suppress);
    assert_eq!(decision.reason, DecisionReason::ConversationFullySilent);
}

#[test]
fn unprechecked_dst_ambiguity_is_not_infrastructure_failure() {
    let decision = evaluator().evaluate(&input_for_local_time("America/New_York", "2026-11-01", "01:30"));
    assert_eq!(decision.outcome, NotificationOutcome::ScheduleTimeAmbiguous);
    assert_eq!(decision.reason, DecisionReason::ScheduleTimeAmbiguous);
}
```

- [ ] **Step 2: 运行测试确认失败。**

Run: `cargo test -p personal-secretary notification_policy::tests::fully_silent_conversation_requires_two_explicit_bypass_grants`

Expected: FAIL，因为 evaluator 尚未实现。

- [ ] **Step 3: 实现固定顺序纯函数。**

```rust
pub fn evaluate(&self, input: &EvaluationInput) -> EvaluationPlan {
    if !input.candidate_is_current { return EvaluationPlan::terminal(NotificationOutcome::CandidateExpired); }
    let conversation = input.rule(PolicyScope::Conversation);
    if conversation.is_fully_silent() && !conversation.allows_bypass_for(input.matching_rule()) {
        return EvaluationPlan::suppress(DecisionReason::ConversationFullySilent);
    }
    for scope in [PolicyScope::Conversation, PolicyScope::Contact,
                  PolicyScope::Category, PolicyScope::AccountDefault] {
        if let Some(plan) = input.rule(scope).and_then(|rule| rule.plan_for(&input.candidate.key)) { return plan; }
    }
    self.apply_quiet_hours(input)
}
```

`allows_bypass_for` 必须实现 `candidate_rule.bypass_quiet && conversation_rule.allow_bypass`。联系人重要性不可单独越过完全静默。静默时段跨午夜时使用 `start <= local || local < end`；非跨午夜使用 `start <= local && local < end`。用 `chrono_tz::Tz::from_local_datetime` 检测 `LocalResult::Ambiguous/None`，返回 `ScheduleTimeAmbiguous`，绝不映射为任何失败或 suppress 原因。

- [ ] **Step 4: 实现写入前 DST 400 天预检。**

```rust
pub fn validate_quiet_hours(rule: &QuietHoursRule, clock: &dyn Clock) -> Result<(), NotificationPolicyError> {
    rule.validate_shape()?;
    for date in dates_from(clock.now_unix_secs(), rule.effective_range(), 400)? {
        rule.validate_local_boundary(date, rule.start_local_time)?;
        rule.validate_local_boundary(date, rule.end_local_time)?;
    }
    Ok(())
}
```

如果有效日期范围排除了歧义日则允许；否则返回 `AmbiguousScheduleTime`，由 Planner 产生澄清而不是 silently adjusting time。

- [ ] **Step 5: 运行完整领域测试。**

Run: `cargo test -p personal-secretary notification_policy::tests`

Expected: PASS，覆盖 Known/Absent/Unknown、跨午夜、`America/New_York` 2026-03-08 02:30 不存在和 2026-11-01 01:30 重复、双 bypass。

### Task 3: 定义策略、评估和授权端口

**Files:**
- Create: `crates/personal-secretary/src/notification_policy_service.rs`
- Modify: `crates/personal-secretary/src/lib.rs`
- Test: `crates/personal-secretary/src/notification_policy_service.rs`

**Consumes:** Task 1–2 领域类型、`Clock`、Action Run identity。

**Produces:** `NotificationPolicyStoreT`、`NotificationPolicyUseCase`、`NotificationPolicyAuthorizationT`、`EvaluationSnapshot`、`EvaluationCommit`。

- [ ] **Step 1: 写用例不把 Unknown 当 Absent、且不跨 I/O 事务的测试 double。**

```rust
#[tokio::test]
async fn use_case_refuses_feedback_promotion_when_match_metadata_is_unknown() {
    let store = Arc::new(FakeStore::default());
    let result = NotificationPolicyUseCase::new(store, Arc::new(FixedClock(0)))
        .record_feedback(feedback_request_with_unknown()).await;
    assert!(matches!(result, Err(NotificationPolicyError::UnknownMatchMetadata)));
}
```

- [ ] **Step 2: 定义端口的三阶段接口。**

```rust
#[async_trait]
pub trait NotificationPolicyStoreT: Send + Sync {
    async fn claim_evaluation(&self, worker_id: &str, now: i64, lease_secs: u64)
        -> Result<Option<ClaimedEvaluation>, NotificationPolicyStoreError>;
    async fn load_evaluation_snapshot(&self, claim: &ClaimedEvaluation)
        -> Result<EvaluationSnapshot, NotificationPolicyStoreError>;
    async fn commit_evaluation(&self, commit: &EvaluationCommit)
        -> Result<EvaluationCommitResult, NotificationPolicyStoreError>;
    async fn recover_expired_evaluations(&self, now: i64, limit: u32)
        -> Result<u64, NotificationPolicyStoreError>;
}
```

`EvaluationSnapshot` 必含 `account_policy_epoch: u64`、所有读取 Family 的 `(PolicyFamilyId, generation)`、候选 version 和最终 OwnerBinding/Owner recipient 所需标识。`EvaluationCommit` 必含 lease token、snapshot、纯函数 `EvaluationPlan`，不携带正文。

- [ ] **Step 3: 统一四层授权函数。**

```rust
pub fn authorize_notification_policy_action(
    binding: &OwnerBindingView, command: &OwnerCommandView,
    target_account: &SourceAccountRef,
) -> Result<(), NotificationPolicyError>
```

Planner 提前检查、Action Gate、Resume 和 Effect 都调用同一函数；函数拒绝非 Owner、账号不一致、没有 OwnerCommand 或绑定不匹配。`AutomaticReplyPolicyGate` 是单独纯函数接口，读取错误和 actor identity unknown 均返回 `Denied`。

- [ ] **Step 4: 运行端口测试。**

Run: `cargo test -p personal-secretary notification_policy_service`

Expected: PASS。

### Task 4: 新增 MySQL migration 和 SeaORM entities

**Files:**
- Create: `apps/qqbot-server/database/migrations/20260728_owner_notification_policy_feedback_v1.sql`
- Create: `apps/qqbot-server/database/test_support/qqbot_migrations.rs`
- Modify: `apps/qqbot-server/tests/qqbot_acceptance_runtime.rs`
- Modify: `crates/personal-secretary/tests/qqbot_acceptance_mysql.rs`
- Modify: `crates/personal-secretary/tests/mysql_ingestion.rs`
- Modify: `crates/personal-secretary/tests/mysql_action_planner.rs`
- Create: `apps/qqbot-server/tests/qqbot_acceptance_migrations.rs`
- Create: `crates/personal-secretary/src/infra/repo/entities/secretary_notification_policy_families.rs`
- Create: `crates/personal-secretary/src/infra/repo/entities/secretary_notification_policy_revisions.rs`
- Create: `crates/personal-secretary/src/infra/repo/entities/secretary_notification_candidates.rs`
- Create: `crates/personal-secretary/src/infra/repo/entities/secretary_notification_evaluation_requests.rs`
- Create: `crates/personal-secretary/src/infra/repo/entities/secretary_notification_decisions.rs`
- Create: `crates/personal-secretary/src/infra/repo/entities/secretary_notification_feedback.rs`
- Modify: `crates/personal-secretary/src/infra/repo/entities/mod.rs`
- Test: `crates/personal-secretary/tests/qqbot_acceptance_mysql.rs`

**Consumes:** MySQL schema conventions in `20260727_personal_secretary_owner_agenda.sql`。

**Produces:** 版本化表、`secretary_accounts.policy_epoch`、DDL 约束和 entity 映射。

- [ ] **Step 1: 写 migration schema smoke test。**

```rust
#[tokio::test]
async fn notification_policy_migration_uses_unsigned_epoch_and_immutable_revision_shape() {
    let db = setup_acceptance_schema().await?;
    let ddl = show_create_table(&db, "secretary_notification_policy_revisions").await?;
    assert!(ddl.contains("revision_kind"));
    assert!(!ddl.contains(" active "));
    assert!(show_column(&db, "secretary_accounts", "policy_epoch").await?.contains("bigint unsigned"));
    // schema 的创建与删除均由 verify-qqbot-acceptance.ps1 统一管理。
    Ok(())
}
```

- [ ] **Step 2: 运行 test 确认 migration 尚不存在。**

Run: `cargo test -p personal-secretary --test qqbot_acceptance_mysql notification_policy_migration_uses_unsigned_epoch -- --nocapture`

Expected: FAIL，缺少 migration/table。

- [ ] **Step 3: 写 migration。**

`secretary_accounts` 增加 `policy_epoch BIGINT UNSIGNED NOT NULL DEFAULT 0`。新表必须具有以下不可省略的数据库约束：

```sql
CREATE TABLE secretary_notification_policy_families (
  policy_family_id CHAR(36) CHARACTER SET ascii COLLATE ascii_bin PRIMARY KEY,
  account_id BIGINT UNSIGNED NOT NULL,
  canonical_scope_key VARCHAR(512) CHARACTER SET utf8mb4 COLLATE utf8mb4_bin NOT NULL,
  policy_kind VARCHAR(64) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  -- 创建事务内暂时为 NULL；提交前必须在同一事务写入 Head，正常持久状态不得为 NULL。
  current_revision_id CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NULL,
  generation BIGINT UNSIGNED NOT NULL,
  UNIQUE KEY uk_notification_policy_family (account_id, canonical_scope_key, policy_kind),
  UNIQUE KEY uk_notification_policy_family_head (policy_family_id, current_revision_id),
  CONSTRAINT fk_notification_policy_family_account FOREIGN KEY (account_id) REFERENCES secretary_accounts(id) ON DELETE CASCADE,
  CONSTRAINT chk_notification_policy_family_generation CHECK (generation > 0)
) ENGINE=InnoDB;
```

Revision 表须包含 `UNIQUE KEY uk_notification_policy_revision_family_id (policy_family_id, policy_revision_id)`；Family 增加复合外键 `(policy_family_id, current_revision_id)` 指向该唯一键，保证 Head 不会跨 Family。创建流程仅可在一个短事务内执行 `INSERT Family(current_revision_id=NULL)` → `INSERT Revision(policy_family_id=Family)` → CAS `UPDATE Family SET current_revision_id=?, generation=...` → `policy_epoch+1`；提交前查询或约束检查不得留下 NULL Head。替换与 tombstone 同样只以同 Family 的新 Revision 更新 Head。

Outbox 扩展为可选 `notification_candidate_id`、`notification_decision_id`、`occurrence_id`，把来源 CHECK 扩为 exactly-one source；`occurrence_id` 设唯一。新增通知 kind 不得破坏 follow-up/agenda 已有唯一键与 CHECK。

- [ ] **Step 4: 生成 entities 并严格映射，并验证 Head 完整性。**

每个 entity 的 `account_id`、`generation`、`revision_number`、`policy_epoch`、`source_version` 使用 `u64`；`lease_expires_at_unix_secs` 使用 `i64`；不存在将 unsigned DB 列映射为 `i64` 或 `String` 的例外。增加 migration 集成测试：创建 Family 后 Head 非 NULL、Head Revision 属于该 Family，且尝试以另一 Family 的 Revision 更新 Head 必须被复合外键拒绝；重复执行迁移加载清单时只应用哨兵记录一次且所有新表/列均存在。

- [ ] **Step 5: 运行 migration 和 DDL 测试。**

Run: `cargo test -p personal-secretary --test qqbot_acceptance_mysql notification_policy_migration_uses_unsigned_epoch -- --nocapture`

Expected: PASS，测试 schema 名称以 `qqbot_accept_` 开头且 teardown 成功。

### Task 5: 实现 MySQL Family/Revision、policy_epoch 与反馈持久化

**Files:**
- Create: `crates/personal-secretary/src/infra/repo/mysql_notification_policy.rs`
- Modify: `crates/personal-secretary/src/infra/repo/mod.rs`
- Modify: `crates/personal-secretary/src/infra/mod.rs`
- Test: `crates/personal-secretary/tests/qqbot_acceptance_mysql.rs`

**Consumes:** Task 3 port、Task 4 entities。

**Produces:** `MySqlNotificationPolicyStore`、`build_mysql_notification_policy_store`、Family Head CAS 和 account epoch CAS。

- [ ] **Step 1: 写不可变 revision 与 epoch 原子性失败测试。**

```rust
#[tokio::test]
async fn create_replace_and_tombstone_each_increment_account_policy_epoch() {
    let before = policy_epoch(&db, &account).await?;
    let family = store.create_or_replace(rule_request(&account, None)).await?;
    assert_eq!(policy_epoch(&db, &account).await?, before + 1);
    store.create_or_replace(rule_request(&account, Some(family.clone()))).await?;
    assert_eq!(policy_epoch(&db, &account).await?, before + 2);
    store.disable(&disable_request(family)).await?;
    assert_eq!(policy_epoch(&db, &account).await?, before + 3);
    assert_revision_rows_are_never_updated(&db).await?;
}
```

- [ ] **Step 2: 实现每个策略写入的同一短事务。**

事务内顺序：锁定 account row → 读取或 `INSERT` Family → `INSERT` immutable Revision → `UPDATE Family SET current_revision_id=?, generation=generation+1 WHERE generation=?` → `UPDATE secretary_accounts SET policy_epoch=policy_epoch+1 WHERE id=?` → 写审计。若任一 CAS 影响 0 行则 rollback 并返回 `Conflict`；不得以 `created_at` 选 Head，不得 UPDATE Revision。

- [ ] **Step 3: 实现反馈和自动回复独立 gate 读取。**

`record_feedback` 以 `(account_id, command_source_event_id, feedback_kind)` 去重；只有 `promote_to_rule=true` 且 MatchKey 全部 required field 为 `Known/Absent` 时才调用 Family writer。自动回复读取使用 `load_automatic_reply_gate(account, actor)`；`actor=None` 或 store `Unavailable/Database` 时 caller 只得到 `AutomaticReplyGateDecision::Denied`。

- [ ] **Step 4: 运行 MySQL 测试。**

Run: `cargo test -p personal-secretary --test qqbot_acceptance_mysql notification_policy_family -- --nocapture`

Expected: PASS，包含重复 OwnerCommand 幂等、Family CAS 冲突、tombstone 及 epoch 递增。

### Task 6: 实现 Evaluation Request 三阶段、epoch fencing 与原子 Decision/Outbox

**Files:**
- Modify: `crates/personal-secretary/src/infra/repo/mysql_notification_policy.rs`
- Modify: `crates/personal-secretary/src/notification_policy_service.rs`
- Test: `crates/personal-secretary/tests/qqbot_acceptance_mysql.rs`

**Consumes:** Tasks 2–5。

**Produces:** candidate upsert、request claim/reclaim、snapshot、commit 与 delay reschedule。

- [ ] **Step 1: 写“无策略读后并发创建”失败测试。**

```rust
#[tokio::test]
async fn old_evaluation_is_rejected_when_empty_policy_snapshot_is_invalidated_by_new_family() {
    let claim = store.claim_evaluation("worker-a", now(), 30).await?.unwrap();
    let snapshot = store.load_evaluation_snapshot(&claim).await?;
    assert!(snapshot.family_generations.is_empty());
    let old_epoch = snapshot.account_policy_epoch;
    store.create_or_replace(account_default_remind_rule(&account)).await?;
    assert!(policy_epoch(&db, &account).await? > old_epoch);
    let result = store.commit_evaluation(&commit_remind(claim, snapshot)).await?;
    assert_eq!(result, EvaluationCommitResult::SnapshotStale);
    assert_no_outbox_for_candidate(&db, &candidate).await?;
}
```

- [ ] **Step 2: 领取短事务。**

`claim_evaluation` 必须一个短事务更新 pending 或已过期 lease 的 request：生成 `lease_token`，增加 attempt，写 `lease_expires_at`，且 WHERE 同时限制未完成 Decision。并发 worker 只能一个拿到 `Some`；过期 lease 可被重新领取；旧 token 的提交返回 `LeaseLost`。

- [ ] **Step 3: 实现事务外 snapshot 和纯求值。**

`load_evaluation_snapshot` 仅查询候选当前版本、account `policy_epoch`、相关 Family Heads 与 revision；然后用 Task 2 evaluator 在事务外计算。没有 Family 也必须记录 account epoch。不得打开 transaction 后调用 evaluator、Clock 之外 I/O 或等待。

- [ ] **Step 4: 实现最终短事务并逐项 fence。**

提交事务按此顺序检查：request lease token/未过期 → candidate source version/current state → `secretary_accounts.policy_epoch == snapshot.account_policy_epoch` → 每个 `(family_id,generation)` 仍相同 → OwnerBinding 仍指向目标 account 的 Owner → Outbox recipient 仍为该 Owner。任一失败：不写 Decision/Outbox，返回 `SnapshotStale` 或 `LeaseLost`，并创建或保留可重评 request。

成功时同一事务中：`INSERT Decision`、根据 Outcome 更新 request/candidate；仅 `Remind` 以稳定 UUIDv5 `source_kind:source_id:source_version:notification_kind` 做 `INSERT IGNORE` Outbox；`Delay` 只写 `next_allowed_at` 与新调度状态；`Suppress` 只写终态。保证无 Decision 不会有 Outbox，且同 Occurrence 只有一条有效 Outbox。

- [ ] **Step 5: 写故障边界与重启测试。**

```rust
#[tokio::test]
async fn failed_transaction_never_leaves_outbox_without_decision() { /* 注入 Decision 后、Outbox 前 SQL failure；断言两表都无本次记录 */ }
#[tokio::test]
async fn delayed_candidate_creates_fresh_request_and_decision_after_due_time() { /* 不复用旧 Decision */ }
#[tokio::test]
async fn schedule_time_ambiguous_has_terminal_reason_not_infrastructure_failure() { /* assert outcome/reason */ }
```

- [ ] **Step 6: 运行评估集成测试。**

Run: `cargo test -p personal-secretary --test qqbot_acceptance_mysql notification_evaluation -- --nocapture`

Expected: PASS，含 lease late commit、UnknownCommit 重新观察、restart reclaim、policy epoch phantom fencing、generation fencing、delay、candidate expired、delivery window expired、`schedule_time_ambiguous`。

### Task 7: 将 Agenda 和 FollowUp 改为候选源而非直接 Outbox

**Files:**
- Modify: `crates/personal-secretary/src/agenda_service.rs`
- Modify: `crates/personal-secretary/src/infra/repo/mysql_agenda.rs`
- Modify: `crates/personal-secretary/src/follow_up_service.rs`
- Modify: `crates/personal-secretary/src/infra/repo/mysql_follow_up.rs`
- Test: `crates/personal-secretary/tests/mysql_ingestion.rs`

**Consumes:** `NotificationPolicyStoreT::ensure_candidate`。

**Produces:** 稳定 Agenda/FollowUp candidate，而非提前 Outbox；升级前 `pending`、`failed`、`claimed` Outbox 的受控 reconciliation。

- [ ] **Step 1: 写先求值、后 Outbox 与遗留 Outbox 回填测试。**

```rust
#[tokio::test]
async fn due_agenda_creates_candidate_but_no_outbox_before_notification_decision() {
    agenda.enqueue_due_notifications(100).await?;
    assert_eq!(count_notification_candidates(&db).await?, 1);
    assert_eq!(count_notification_outbox(&db).await?, 0);
}
```

- [ ] **Step 2: 改写两个仓储的入队行为并执行遗留 Outbox reconciliation。**

`AgendaStoreT::enqueue_due_notifications` 和 FollowUp scan 不再插入 `secretary_notification_outbox`；替换为 candidate `INSERT IGNORE`，来源分别为 `agenda_item_id/version` 与 `follow_up_id/version`。source version 必须随业务版本变更；完成、取消、撤回或 supersede 后 evaluator 返回 `CandidateExpired`。

新增幂等 `reconcile_legacy_notification_outbox()`：把升级前 `pending`、`failed`、`claimed` 的 Agenda/FollowUp Owner-only Outbox 行转换为等价 Candidate + EvaluationRequest；启动阶段尚未启动投递 worker，因此不等待或保留 `claimed` 租约。在同一短事务内锁定旧 Outbox，创建成功后删除或标记为不可领取的迁移终态，保证旧投递 worker 不会再领取它。`delivered`、`suppressed` 与 `unknown_commit` 行只审计、不创建 Candidate、不重放。将 reconciliation 放在官方 Outbox worker 启动之前；其完成且无可领取遗留行后，才允许启动官方 Outbox worker。测试需验证重复 reconciliation 不创建重复 Candidate/Decision/Outbox，且 `pending`、`failed`、`claimed` 都不会绕过 evaluator。

- [ ] **Step 3: 运行聚焦测试。**

Run: `cargo test -p personal-secretary --test mysql_ingestion due_agenda_creates_candidate -- --nocapture`

Expected: PASS；既有 Agenda/FollowUp 行为测试调整为断言 candidate + evaluation 后的 Outbox。

### Task 8: 扩展 Action Graph、Planner 白名单与统一最终授权

**Files:**
- Modify: `crates/personal-secretary/src/agent_runtime/action.rs`
- Modify: `crates/personal-secretary/src/agent_runtime/validation.rs`
- Modify: `crates/personal-secretary/src/planner.rs`
- Modify: `crates/personal-secretary/src/action_graph/effect_executor.rs`
- Modify: `crates/personal-secretary/src/planner_service.rs`
- Test: `crates/personal-secretary/src/action_graph/tests.rs`

**Consumes:** Tasks 1–3 use case 和 authorization function。

**Produces:** 11 个类型化策略 Action，L0/L1/L2 分级，真实 Effect receipt/response。

- [ ] **Step 1: 写 Action 风险和最终授权失败测试。**

```rust
#[test]
fn notification_action_risk_levels_are_exact() {
    assert_eq!(SecretaryToolKind::ListNotificationPolicies.policy().risk, SecretaryRiskLevel::L0ReadOnly);
    assert_eq!(SecretaryToolKind::RecordNotificationFeedback.policy().risk, SecretaryRiskLevel::L1Reversible);
    assert_eq!(SecretaryToolKind::SetQuietHours.policy().risk, SecretaryRiskLevel::L2Impactful);
}

#[tokio::test]
async fn effect_rejects_cross_account_action_even_if_planner_proposed_it() { /* effect executor must return permanent authorization error */ }
```

- [ ] **Step 2: 加入精确 Action variants。**

在 `SecretaryToolKind` 和 `SecretaryAction` 加入：`ListNotificationPolicies`、`ExplainNotificationDecision`、`SetAccountDefaultNotificationMode`、`SetConversationNotificationMode`、`SetQuietHours`、`SetImportantContact`、`SetNotificationCategoryImportance`、`RecordNotificationFeedback`、`CreateSimilarNotificationRule`、`DisableNotificationPolicy`、`SetAutomaticReplyDeniedForContact`。`DisableNotificationPolicy` 字段类型必须是 `PolicyFamilyId`，不创建 `DisableAutomaticReply`。

- [ ] **Step 3: 更新 validation/planner。**

所有 ID、scope、reason、时间字符串和 JSON input 按 Task 1 常量检查字节上限。`CreateSimilarNotificationRule` 必须携带确定性的 `NotificationMatchKeyV1`，解析到 `Unknown` 时只允许 Planner 输出 `AskOwnerClarification`，不可自行扩大规则。`is_allowed_action_in_batch` 包含所有新类型；Planner prompt 只接收 envelope metadata，不能要求正文。

- [ ] **Step 4: 在 EffectExecutor 作最终强制。**

`execute` 在 load receipt 后、任何写入/读取前调用 `authorize_notification_policy_action`；Action Gate/Resume 也复用该函数。L0 List/Explain 通过 use case 返回有界类型化 summary；L1 feedback 写 receipt；L2 effect 仅在 Resume receipt 成功后构造成功响应。对 `UnknownCommit` 返回“提交状态不确定，请查询确认”的 Artifact，不调用写操作重放。

- [ ] **Step 5: 运行 Action Graph 测试。**

Run: `cargo test -p personal-secretary action_graph::tests && cargo test -p personal-secretary planner::tests`

Expected: PASS，含 L2 suspend→persisted checkpoint→restart→single CAS resume。

### Task 9: 生成有界 Response Artifact 与 List/Explain/Disable 真实结果

**Files:**
- Modify: `crates/personal-secretary/src/agent_runtime/response.rs`
- Modify: `crates/personal-secretary/src/planner_service.rs`
- Modify: `crates/personal-secretary/src/infra/repo/mysql_notification_policy.rs`
- Test: `crates/personal-secretary/tests/qqbot_acceptance_mysql.rs`

**Consumes:** Task 8 receipts、Policy/Decision summary。

**Produces:** 无正文的 `OwnerResponseDraft`，关联账号、command、run、policy/decision 和审计引用。

- [ ] **Step 1: 写 Artifact 脱敏和 schedule ambiguity Explain 测试。**

```rust
#[test]
fn explain_response_contains_typed_schedule_ambiguity_without_message_body() {
    let draft = render_decision_explanation(decision_with(DecisionReason::ScheduleTimeAmbiguous));
    assert!(draft.text.contains("规则时间存在时区歧义"));
    assert!(!draft.text.contains("原始消息"));
}
```

- [ ] **Step 2: 实现有界响应构造。**

响应字段只能使用：scope、policy id、revision id、decision id、status、priority、类型化 reason、审计引用。限制 response JSON ≤ 8 KiB、文本 ≤ 2,000 chars；不序列化 rule JSON、OpenID、token、DB error 或聊天正文。`ExplainNotificationDecision` 将 `ScheduleTimeAmbiguous` 明确翻译为“规则时间存在时区歧义，需要重新确认”，不能称“基础设施失败”。

- [ ] **Step 3: 运行测试。**

Run: `cargo test -p personal-secretary response && cargo test -p personal-secretary --test qqbot_acceptance_mysql notification_policy_response_artifact -- --nocapture`

Expected: PASS。

### Task 10: 配置、worker、bootstrap 与运行期关闭接线

**Files:**
- Create: `apps/qqbot-server/src/notification_policy_worker.rs`
- Create: `apps/qqbot-server/src/bootstrap/notification_policy.rs`
- Modify: `apps/qqbot-server/src/lib.rs`
- Modify: `apps/qqbot-server/src/bootstrap/mod.rs`
- Modify: `apps/qqbot-server/src/bootstrap/workers.rs`
- Modify: `apps/qqbot-server/src/config/workers.rs`
- Modify: `apps/qqbot-server/src/config/app.rs`
- Modify: `apps/qqbot-server/src/runtime/mod.rs`
- Modify: `apps/qqbot-server/config/qqbot.example.toml`
- Test: `apps/qqbot-server/src/notification_policy_worker.rs`, `apps/qqbot-server/src/config/tests.rs`

**Consumes:** `NotificationPolicyUseCase`、MySQL store、existing WorkerHandle lifecycle。

**Produces:** `notification_policy` config、可取消 worker，失败路径回收。

- [ ] **Step 1: 写 config 边界和 worker shutdown 测试。**

```rust
#[test]
fn notification_policy_config_rejects_zero_lease_and_invalid_backoff() { /* parse config -> ConfigError */ }

#[tokio::test]
async fn notification_policy_worker_stops_without_waiting_for_scan_interval() { /* FakeRunner + watch true */ }
```

- [ ] **Step 2: 定义配置。**

```rust
pub struct NotificationPolicyConfig {
    pub enabled: bool,
    pub scan_interval_ms: u64,
    pub batch_size: u32,
    pub lease_secs: u64,
    pub retry_initial_ms: u64,
    pub retry_max_ms: u64,
    pub max_attempts: u32,
}
```

默认值必须有界；validate：scan 1,000..=3,600,000，batch 1..=1,000，lease 1..=3,600，max attempts 1..=100，retry initial > 0 且 max >= initial。只同步更新 `apps/qqbot-server/config/qqbot.example.toml`，不得新增环境变量或修改根目录 `config.example.toml`。

- [ ] **Step 3: 实现 worker。**

每轮：`recover_expired_evaluations` → 领取至多 batch 条 → 对每条调用 use case 的 snapshot/evaluate/commit；错误按有界指数退避记录脱敏类型，不 `unwrap`，不直接触碰 Outbox 投递。使用 `watch` 循环检查 `borrow()`，`signal_and_detach` 加入 `WorkerHandles::shutdown_all` 的并发回收集合。

- [ ] **Step 4: 装配顺序。**

在 `runtime::run_with_shutdown` 中，先完成遗留 Outbox reconciliation，再启动 Agenda/FollowUp candidate producer 与策略 worker；只有 reconciliation 完成且无可领取旧行后，才启动官方平台 Outbox 消费。任一后续装配失败时使用现有 `WorkerHandles` 单 deadline `shutdown_all()` 回收。`main.rs` 不变。

- [ ] **Step 5: 运行 app 单元测试。**

Run: `cargo test -p qqbot-server notification_policy && cargo test -p qqbot-server config::tests`

Expected: PASS。

### Task 11: 完整 MySQL 运行时验收与故障注入

**Files:**
- Modify: `apps/qqbot-server/tests/qqbot_acceptance_runtime.rs`
- Modify: `crates/personal-secretary/tests/qqbot_acceptance_mysql.rs`
- Modify: `crates/personal-secretary/tests/mysql_ingestion.rs`
- Modify: `crates/personal-secretary/tests/mysql_action_planner.rs`
- Modify: `scripts/verify-qqbot-acceptance.ps1`（仅新增本地隔离测试调用；不改 L4/L5 attestation）

**Consumes:** 全部实现。

**Produces:** 真实 DB 链路证据，不连接 QQ、不发消息。

- [ ] **Step 1: 复用外层脚本的随机 schema 生命周期，并统一迁移加载清单。**

`verify-qqbot-acceptance.ps1` 负责唯一的 schema 生命周期：创建随机 `qqbot_accept_*` schema、设置 `QQBOT_TEST_DATABASE_URL`，并在 `finally` 删除。Rust 测试只验证当前 schema 名称匹配 `^qqbot_accept_[A-Za-z0-9_]+$`，绝不自行 `DROP DATABASE`，避免同一进程的后续测试失去 schema；缺少 URL 时必须明确报 prerequisite。独立运行 Rust 测试也必须经该包装脚本或显式提供已隔离的 `qqbot_accept_*` URL。

固定创建 `apps/qqbot-server/database/test_support/qqbot_migrations.rs` 作为唯一共享加载器；`qqbot_acceptance_mysql.rs`、`qqbot_acceptance_runtime.rs`、`mysql_ingestion.rs`、`mysql_action_planner.rs` 均通过 `#[path = "../../apps/qqbot-server/database/test_support/qqbot_migrations.rs"]`（按各测试文件调整相对路径）引入同一模块。加载器按文件名前缀严格排序，维护迁移记录表与幂等哨兵；新增 migration 只登记一次，并用重复加载测试断言第二次不重复执行且所有新表、列、索引已存在。

- [ ] **Step 2: 编写端到端用例。**

```rust
#[tokio::test]
async fn agenda_candidate_is_evaluated_before_owner_only_outbox_and_never_calls_qq() {
    // arrange: test DB + owner binding + agenda due + account default remind
    // act: run candidate producer then one evaluation scan
    // assert: exactly one Decision, exactly one owner-recipient Outbox, no external client invocation
}

#[tokio::test]
async fn concurrent_workers_and_late_lease_cannot_duplicate_decision_or_outbox() { /* two claims + stale submit */ }

#[tokio::test]
async fn final_commit_rechecks_owner_binding_and_outbox_recipient() { /* mutate binding after snapshot; expect SnapshotStale */ }
```

补齐：FollowUp、L2 suspend/restart/resume、double resume 拒绝、Effect Receipt、UnknownCommit observation、Decision 前失败、Decision/Outbox 边界失败、重启 reclaim、Policy generation stale、empty-policy `policy_epoch` stale、delay fresh request、所有 terminal reason、prompt injection 只生成澄清或拒绝、无 QQ/第三方发送。

- [ ] **Step 3: 执行隔离验收。**

Run: `cargo test -p personal-secretary --test qqbot_acceptance_mysql -- --nocapture && cargo test -p qqbot-server --test qqbot_acceptance_runtime -- --nocapture`

Expected: PASS；若 `QQBOT_TEST_DATABASE_URL` 缺失，测试必须清楚报告 prerequisite，不能将跳过宣称为通过。

### Task 12: 文档证据、全量验证与交付检查

**Files:**
- Modify: `docs/qq-personal-secretary/TODO.md`
- Modify: `docs/qq-personal-secretary/HISTORY.md`
- Modify: `docs/qq-personal-secretary/specs/2026-07-28-owner-notification-policy-feedback-v1-design.md`
- Modify: 对应 2026-07 月度历史/capability assessment 文件（仅在现有文件真实存在时修改）

**Consumes:** 测试输出、git status、最终 schema cleanup 证据。

**Produces:** 真实状态文档和无提交的可评审 diff。

- [ ] **Step 1: 以代码与测试证据更新 TODO/HISTORY。**

将 `CMD-005`、`CMD-006`、`FUP-005`、`FUP-009` 标为完成仅当 Task 11 的真实 MySQL 测试全部通过。修正 QA-002、B3/B4/B6/B7、Recall Spool、Artifact、Gap 叙述时只陈述本次核验过的事实；保留 L4/L5 未配置受保护环境而 `REJECTED` 的事实，不能扩大为已认证。

- [ ] **Step 2: 记录固定交付信息。**

在 HISTORY 记录日期、基线 `main@39f35b3`、分钟级验证时间、影响范围、`policy_epoch`/三态/`schedule_time_ambiguous` 关键不变量、运行命令、测试 schema cleanup、git status 和下一阶段。不得写 token、URL、OpenID、聊天正文。

- [ ] **Step 3: 运行完整验证。**

Run:

```bash
cargo fmt --all -- --check
cargo clippy -p personal-secretary -p qqbot -p qqbot-server --all-targets -- -D warnings
cargo test -p personal-secretary
cargo test -p qqbot
cargo test -p qq-open-platform
cargo test -p digital-human-server --test workspace_boundaries
git diff --check
git ls-files --others --exclude-standard -z | xargs -0 -r git diff --no-index --check /dev/null
```

Expected: QQBot/Personal Secretary 范围内的本地检查 PASS；数字人仅运行 `workspace_boundaries`，不得因本任务运行或修复数字人 clippy。受保护环境依赖的验收明确列为未运行原因。确认不包含 `.zcode/`，确认无数字人恢复、TTS、`.mcp.json`、根目录 `config.example.toml` 或主工作树未跟踪文件进入 diff。

- [ ] **Step 4: 人工生命周期走查。**

逐项确认：candidate 版本变化、Family 创建 phantom、generation 变化、lease 迟到、worker restart、transaction failure、Outbox 幂等、OwnerBinding 变化、UnknownCommit、DST tzdb 歧义、自动回复身份未知/读取失败；任一项无可执行测试时不得标记完成。

## Plan self-review

- **规格覆盖：** Task 1–2 覆盖不可变领域、三态、优先级、DST；Task 3–6 覆盖端口、统一授权、epoch/generation/lease/CAS/原子提交；Task 4 覆盖 Family Head 的循环外键创建顺序、同 Family 复合外键和迁移加载器；Task 7 覆盖 Agenda/FollowUp 前置求值与遗留 pending/failed Outbox reconciliation；Task 8–9 覆盖 Action Graph 与 Response Artifact；Task 10 覆盖 QQBot 独立配置、worker/关闭及 reconciliation-before-Outbox 启动；Task 11 覆盖由外层脚本统一管理的真实 MySQL 生命周期、迁移重复加载、故障与并发；Task 12 覆盖文档和限定 crate 验证。
- **占位符扫描：** 未使用待定占位符、或“类似 Task N”。文中出现的 `TODO.md` 仅为已有文档文件名。所有测试步骤给出目标测试、失败原因或断言，所有实现步骤给出接口、顺序或代码。
- **类型一致性：** MatchKey 三态统一为 `MatchField<T>`；结果统一为 `NotificationOutcome`；时区歧义统一为 `DecisionReason::ScheduleTimeAmbiguous`/`NotificationOutcome::ScheduleTimeAmbiguous`；并发 snapshot 统一使用 `account_policy_epoch` 与 `(PolicyFamilyId, generation)`。
