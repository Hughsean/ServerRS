//! 独立 QQBot 验收测试。
//!
//! 这些测试描述跨模块业务不变量，不复用生产实现中的 helper，也不把 Fake Store 当作
//! 生产闭环证据。除纯身份约束外，测试必须指向隔离 MySQL schema，并由
//! `scripts/verify-qqbot-acceptance.ps1` 逐项运行。

use std::sync::Arc;

use personal_secretary::{
    ArtifactEnvelope, ArtifactId, ArtifactKind, ArtifactUseCase, ConnectionEndReason,
    ContentSegment, ContentTrustLevel, ConversationKind, ConversationRef, ConversationScope,
    DecisionReason, DirectoryEvidence, DirectorySnapshot, DirectorySnapshotId, DirectorySourceApi,
    DirectoryStatus, EvaluationCommit, EvaluationCommitResult, EvaluationPlan, EventKind,
    InboundMessageEnvelope, MatchField, MessageSource, NotificationCategory,
    NotificationFeedbackRequest, NotificationOutcome, NotificationPolicyDisableRequest,
    NotificationPolicyEvaluator, NotificationPolicyKind, NotificationPolicyRule,
    NotificationPolicyStoreError, NotificationPolicyStoreT, NotificationPolicyWriteRequest,
    RecallCorrelationKey, RecallEvent, RecallEventId, RecallKind, RecallUseCase, ScopeKind,
    SourceAccountRef, SourceMessageRef, StructuredImportance, TombstoneStatus, VerifiedActor,
    VerifiedActorKind, build_mysql_artifact_store, build_mysql_directory_store,
    build_mysql_inbound_event_store, build_mysql_notification_policy_store,
    build_mysql_recall_store,
};
use sea_orm::{ConnectionTrait, Database, DatabaseBackend, DatabaseConnection, Statement};
use sha2::{Digest, Sha256};
use uuid::Uuid;

#[path = "../../../apps/qqbot-server/database/test_support/qqbot_migrations.rs"]
mod qqbot_migrations;

fn account(subject: &str) -> SourceAccountRef {
    SourceAccountRef::new(MessageSource::NapCat, subject).expect("valid account fixture")
}

fn group(group_id: &str) -> ConversationRef {
    ConversationRef::new(ConversationKind::Group, group_id).expect("valid group fixture")
}

fn message(account_subject: &str, group_id: &str, message_id: &str) -> InboundMessageEnvelope {
    InboundMessageEnvelope::new(
        SourceMessageRef::new(MessageSource::NapCat, account_subject, message_id)
            .expect("valid source fixture"),
        group(group_id),
        VerifiedActor::new(VerifiedActorKind::External, "acceptance-sender")
            .expect("valid actor fixture"),
        1_800_000_000,
        "acceptance message",
        Vec::new(),
    )
    .expect("valid inbound fixture")
}

fn recall(
    recall_event_id: impl Into<String>,
    account_subject: &str,
    group_id: &str,
    message_id: &str,
) -> RecallEvent {
    let account = account(account_subject);
    RecallEvent {
        recall_event_id: RecallEventId::new(recall_event_id).expect("valid recall id fixture"),
        account: account.clone(),
        kind: RecallKind::Group,
        correlation: RecallCorrelationKey::new(
            account,
            MessageSource::NapCat,
            group(group_id),
            message_id,
        )
        .expect("valid recall correlation fixture"),
        operator_platform_id: Some("acceptance-operator".into()),
        occurred_at_unix_secs: 1_800_000_100,
    }
}

async fn isolated_db() -> DatabaseConnection {
    let url = std::env::var("QQBOT_TEST_DATABASE_URL")
        .expect("QQBOT_TEST_DATABASE_URL must point to an isolated qqbot_accept_* schema");
    let schema = url
        .split('?')
        .next()
        .and_then(|value| value.rsplit('/').next())
        .unwrap_or_default();
    assert!(
        schema.starts_with("qqbot_accept_"),
        "refusing to run acceptance tests against non-isolated schema: {schema}"
    );
    let db = Database::connect(url)
        .await
        .expect("connect isolated acceptance MySQL");
    qqbot_migrations::apply_qqbot_migrations(
        &db,
        &std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../apps/qqbot-server/database/migrations"),
    )
    .await;
    db
}

fn policy_rule(account: SourceAccountRef) -> NotificationPolicyRule {
    NotificationPolicyRule {
        match_key: personal_secretary::NotificationMatchKeyV1::new(
            account,
            MatchField::Absent,
            MatchField::Absent,
            MatchField::Known(NotificationCategory::Agenda),
            MatchField::Known(false),
            MatchField::Known(StructuredImportance::Normal),
            MatchField::Known(EventKind::AgendaDue),
        )
        .expect("valid policy rule match key"),
        outcome: NotificationOutcome::Suppress,
        bypass_quiet: false,
    }
}

fn policy_write_request(
    account: SourceAccountRef,
    policy_family_id: Option<personal_secretary::PolicyFamilyId>,
) -> NotificationPolicyWriteRequest {
    NotificationPolicyWriteRequest {
        account: account.clone(),
        policy_family_id,
        canonical_scope_key: "agenda:all".into(),
        policy_kind: NotificationPolicyKind::Category,
        rule: policy_rule(account),
        command_source_event_id: None,
        audit_summary: "acceptance policy mutation".into(),
    }
}

fn automatic_reply_rule(account: SourceAccountRef, actor_id: &str) -> NotificationPolicyRule {
    NotificationPolicyRule {
        match_key: personal_secretary::NotificationMatchKeyV1::new(
            account,
            MatchField::Absent,
            MatchField::Known(actor_id.into()),
            MatchField::Absent,
            MatchField::Absent,
            MatchField::Absent,
            MatchField::Absent,
        )
        .expect("valid automatic reply match key"),
        outcome: NotificationOutcome::Suppress,
        bypass_quiet: false,
    }
}

fn automatic_reply_write_request(
    account: SourceAccountRef,
    actor_id: &str,
    policy_family_id: Option<personal_secretary::PolicyFamilyId>,
) -> NotificationPolicyWriteRequest {
    NotificationPolicyWriteRequest {
        account: account.clone(),
        policy_family_id,
        canonical_scope_key: format!("contact:{actor_id}"),
        policy_kind: NotificationPolicyKind::AutomaticReplyDenied,
        rule: automatic_reply_rule(account, actor_id),
        command_source_event_id: None,
        audit_summary: "acceptance automatic reply denial".into(),
    }
}

async fn policy_epoch(db: &DatabaseConnection, account: &SourceAccountRef) -> u64 {
    scalar_u64(
        db,
        "SELECT policy_epoch AS value FROM secretary_accounts \
         WHERE source_channel = ? AND platform_account_id = ?",
        vec![
            account.channel.as_str().into(),
            account.account_id.clone().into(),
        ],
    )
    .await
}
async fn scalar_u64(db: &DatabaseConnection, sql: &str, values: Vec<sea_orm::Value>) -> u64 {
    let row = db
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            sql,
            values,
        ))
        .await
        .expect("acceptance query must execute")
        .expect("acceptance query must return one row");
    match row.try_get::<u64>("", "value") {
        Ok(value) => value,
        // MySQL 8 的 COUNT 聚合即使显式 CAST 为 UNSIGNED，驱动仍可能将结果元数据
        // 标为 BIGINT；仅测试辅助函数兼容该聚合输出，持久化 BIGINT UNSIGNED 列仍必须解码为 u64。
        Err(_) => row
            .try_get::<i64>("", "value")
            .expect("MySQL count must decode as an integer")
            .try_into()
            .expect("MySQL count must not be negative"),
    }
}

async fn scalar_string(db: &DatabaseConnection, sql: &str, values: Vec<sea_orm::Value>) -> String {
    db.query_one_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        sql,
        values,
    ))
    .await
    .expect("acceptance query must execute")
    .expect("acceptance query must return one row")
    .try_get::<String>("", "value")
    .expect("acceptance scalar must decode as string")
}

fn unix_now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock must be after Unix epoch")
        .as_secs()
        .try_into()
        .expect("fixture timestamp must fit i64")
}

fn reminder_occurrence_id(source_kind: &str, source_id: &str, source_version: u64) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"notification-policy-occurrence-v2\0");
    hasher.update(source_kind.as_bytes());
    hasher.update([0]);
    hasher.update(source_id.as_bytes());
    hasher.update([0]);
    hasher.update(source_version.to_be_bytes());
    hasher.update([0]);
    hasher.update(b"owner_policy_reminder");
    let digest = hasher.finalize();
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    Uuid::from_bytes(bytes).to_string()
}

async fn insert_policy_evaluation_fixture(
    db: &DatabaseConnection,
    managed_account: &SourceAccountRef,
    command_account_id: &str,
    owner_actor_id: &str,
) -> (String, String, personal_secretary::NotificationMatchKeyV1) {
    insert_policy_evaluation_fixture_with_status(
        db,
        managed_account,
        command_account_id,
        owner_actor_id,
        "pending",
    )
    .await
}

async fn insert_policy_evaluation_fixture_with_status(
    db: &DatabaseConnection,
    managed_account: &SourceAccountRef,
    command_account_id: &str,
    owner_actor_id: &str,
    candidate_status: &str,
) -> (String, String, personal_secretary::NotificationMatchKeyV1) {
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "INSERT INTO secretary_accounts (source_channel, platform_account_id) VALUES ('qq_open_platform', ?)",
        [command_account_id.into()],
    ))
    .await
    .expect("command account fixture must persist");
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "INSERT INTO secretary_owner_bindings \
         (binding_id, managed_account_id, command_account_id, owner_actor_id, status) \
         SELECT ?, managed.id, command.id, ?, 'active' \
         FROM secretary_accounts AS managed CROSS JOIN secretary_accounts AS command \
         WHERE managed.source_channel = ? AND managed.platform_account_id = ? \
           AND command.source_channel = 'qq_open_platform' AND command.platform_account_id = ?",
        [
            Uuid::new_v4().to_string().into(),
            owner_actor_id.into(),
            managed_account.channel.as_str().into(),
            managed_account.account_id.clone().into(),
            command_account_id.into(),
        ],
    ))
    .await
    .expect("unique active owner binding fixture must persist");
    let match_key = personal_secretary::NotificationMatchKeyV1::new(
        managed_account.clone(),
        MatchField::Absent,
        MatchField::Absent,
        MatchField::Known(NotificationCategory::Agenda),
        MatchField::Known(false),
        MatchField::Known(StructuredImportance::Normal),
        MatchField::Known(EventKind::AgendaDue),
    )
    .expect("valid evaluation match key");
    let source_event_id = build_mysql_inbound_event_store(db.clone())
        .insert_message_if_absent(&message(
            &managed_account.account_id,
            "policy-fixture",
            "agenda-source",
        ))
        .await
        .expect("agenda source fixture must persist")
        .source_event_id()
        .clone();
    let agenda_item_id = Uuid::new_v4().to_string();
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "INSERT INTO secretary_agenda_items \
         (item_id, account_id, item_kind, title, scheduled_at_unix_secs, timezone_name, item_status, version, created_command_event_id, current_command_event_id, create_idempotency_key) \
         SELECT ?, id, 'reminder', 'policy fixture', 1, 'UTC', 'scheduled', 1, ?, ?, ? \
         FROM secretary_accounts WHERE source_channel = ? AND platform_account_id = ?",
        [
            agenda_item_id.clone().into(),
            source_event_id.as_str().into(),
            source_event_id.as_str().into(),
            format!("policy-fixture-{agenda_item_id}").into(),
            managed_account.channel.as_str().into(),
            managed_account.account_id.clone().into(),
        ],
    ))
    .await
    .expect("fresh agenda fixture must persist");
    let candidate_id = Uuid::new_v4().to_string();
    let request_id = Uuid::new_v4().to_string();
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "INSERT INTO secretary_notification_candidates \
         (notification_candidate_id, account_id, source_kind, source_id, source_version, match_key_json, candidate_status) \
         SELECT ?, id, 'agenda', ?, 1, CAST(? AS JSON), ? FROM secretary_accounts \
         WHERE source_channel = ? AND platform_account_id = ?",
        [
            candidate_id.clone().into(),
            agenda_item_id.into(),
            serde_json::to_string(&match_key)
                .expect("match key fixture serializes")
                .into(),
            candidate_status.into(),
            managed_account.channel.as_str().into(),
            managed_account.account_id.clone().into(),
        ],
    ))
    .await
    .expect("notification candidate fixture must persist");
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "INSERT INTO secretary_notification_evaluation_requests \
         (evaluation_request_id, notification_candidate_id, evaluation_generation, trigger_kind) \
         VALUES (?, ?, 1, 'candidate_created')",
        [request_id.clone().into(), candidate_id.into()],
    ))
    .await
    .expect("evaluation request fixture must persist");
    (request_id, command_account_id.to_owned(), match_key)
}

async fn claim_and_snapshot(
    store: &Arc<dyn NotificationPolicyStoreT>,
    request_id: &str,
) -> (
    personal_secretary::ClaimedEvaluation,
    personal_secretary::EvaluationSnapshot,
) {
    let claim = store
        .claim_evaluation("acceptance-worker", unix_now_secs(), 60)
        .await
        .expect("evaluation claim must succeed")
        .expect("fixture request must be claimed");
    assert_eq!(
        claim.evaluation_request_id.as_str(),
        request_id,
        "该场景只能领取其自身的 Evaluation Request，不能依赖隔离 schema 的全局队列顺序"
    );
    let snapshot = store
        .load_evaluation_snapshot(&claim)
        .await
        .expect("evaluation snapshot must load");
    (claim, snapshot)
}

fn evaluation_commit(
    claim: personal_secretary::ClaimedEvaluation,
    snapshot: personal_secretary::EvaluationSnapshot,
    plan: personal_secretary::EvaluationPlan,
) -> EvaluationCommit {
    EvaluationCommit {
        claim,
        snapshot,
        plan,
    }
}

#[tokio::test]
#[ignore = "requires isolated MySQL schema created by verify-qqbot-acceptance.ps1"]
async fn notification_policy_family_create_replace_tombstone_advances_epoch_and_keeps_revisions_immutable()
 {
    let db = isolated_db().await;
    let account_subject = format!("accept-policy-{}", Uuid::new_v4().simple());
    let account = account(&account_subject);
    build_mysql_inbound_event_store(db.clone())
        .begin_connection(&account)
        .await
        .expect("account bootstrap must succeed");
    let store = build_mysql_notification_policy_store(db.clone());
    let before = policy_epoch(&db, &account).await;

    let created = store
        .create_or_replace(&policy_write_request(account.clone(), None))
        .await
        .expect("first policy revision must persist");
    assert_eq!(policy_epoch(&db, &account).await, before + 1);
    assert_eq!(created.generation, 2);
    let first_revision_id = created.current_revision_id.as_str().to_owned();
    let first_rule_json = scalar_string(
        &db,
        "SELECT CAST(rule_json AS CHAR) AS value FROM secretary_notification_policy_revisions \
         WHERE policy_revision_id = ?",
        vec![first_revision_id.clone().into()],
    )
    .await;

    let replaced = store
        .create_or_replace(&policy_write_request(
            account.clone(),
            Some(created.policy_family_id.clone()),
        ))
        .await
        .expect("replacement policy revision must persist");
    assert_eq!(policy_epoch(&db, &account).await, before + 2);
    assert_eq!(replaced.generation, 3);
    assert_ne!(replaced.current_revision_id.as_str(), first_revision_id);
    assert_eq!(
        scalar_string(
            &db,
            "SELECT CAST(rule_json AS CHAR) AS value FROM secretary_notification_policy_revisions \
             WHERE policy_revision_id = ?",
            vec![first_revision_id.into()],
        )
        .await,
        first_rule_json,
        "历史 revision 绝不可被替换操作原地修改",
    );

    let disabled = store
        .disable(&NotificationPolicyDisableRequest {
            account: account.clone(),
            policy_family_id: replaced.policy_family_id.clone(),
            expected_generation: replaced.generation,
            command_source_event_id: None,
            audit_summary: "acceptance policy tombstone".into(),
        })
        .await
        .expect("tombstone must persist");
    assert_eq!(policy_epoch(&db, &account).await, before + 3);
    assert_eq!(disabled.generation, 4);
    assert_eq!(
        scalar_string(
            &db,
            "SELECT revision_kind AS value FROM secretary_notification_policy_revisions \
             WHERE policy_revision_id = ?",
            vec![disabled.current_revision_id.as_str().into()],
        )
        .await,
        "tombstone",
    );

    let stale_disable = store
        .disable(&NotificationPolicyDisableRequest {
            account,
            policy_family_id: disabled.policy_family_id,
            expected_generation: replaced.generation,
            command_source_event_id: None,
            audit_summary: "stale acceptance policy tombstone".into(),
        })
        .await;
    assert_eq!(stale_disable, Err(NotificationPolicyStoreError::Conflict));
}

#[tokio::test]
#[ignore = "requires isolated MySQL schema created by verify-qqbot-acceptance.ps1"]
async fn notification_policy_write_is_idempotent_for_the_same_owner_command() {
    let db = isolated_db().await;
    let account_subject = format!("accept-policy-idempotent-{}", Uuid::new_v4().simple());
    let account = account(&account_subject);
    let inbound = build_mysql_inbound_event_store(db.clone());
    inbound
        .begin_connection(&account)
        .await
        .expect("account bootstrap must succeed");
    let command_source_event_id = inbound
        .insert_message_if_absent(&message(&account_subject, "671260344", "owner-command"))
        .await
        .expect("owner command source event must persist")
        .source_event_id()
        .as_str()
        .to_owned();
    let store = build_mysql_notification_policy_store(db.clone());
    let mut request = policy_write_request(account.clone(), None);
    request.command_source_event_id = Some(command_source_event_id.clone());
    let before = policy_epoch(&db, &account).await;

    let first = store
        .create_or_replace(&request)
        .await
        .expect("first policy command must persist");
    let replay = store
        .create_or_replace(&request)
        .await
        .expect("same policy command must be idempotent");

    assert_eq!(replay, first);
    assert_eq!(policy_epoch(&db, &account).await, before + 1);
    assert_eq!(
        scalar_u64(
            &db,
            "SELECT CAST(COUNT(*) AS UNSIGNED) AS value FROM secretary_notification_policy_revisions \
             WHERE command_source_event_id = ?",
            vec![command_source_event_id.into()],
        )
        .await,
        1,
    );
}

#[tokio::test]
#[ignore = "requires isolated MySQL schema created by verify-qqbot-acceptance.ps1"]
async fn automatic_reply_denial_scopes_by_account_actor_and_current_head() {
    let db = isolated_db().await;
    let account_a = account(&format!("accept-auto-a-{}", Uuid::new_v4().simple()));
    let account_b = account(&format!("accept-auto-b-{}", Uuid::new_v4().simple()));
    let inbound = build_mysql_inbound_event_store(db.clone());
    inbound
        .begin_connection(&account_a)
        .await
        .expect("first account bootstrap must succeed");
    inbound
        .begin_connection(&account_b)
        .await
        .expect("second account bootstrap must succeed");
    let store = build_mysql_notification_policy_store(db);
    let denied = store
        .create_or_replace(&automatic_reply_write_request(
            account_a.clone(),
            "actor-a",
            None,
        ))
        .await
        .expect("automatic reply denial must persist");

    assert!(
        store
            .automatic_reply_is_denied(&account_a, "actor-a")
            .await
            .expect("matching actor lookup must succeed")
    );
    assert!(
        !store
            .automatic_reply_is_denied(&account_a, "actor-b")
            .await
            .expect("wrong actor lookup must succeed")
    );
    assert!(
        !store
            .automatic_reply_is_denied(&account_b, "actor-a")
            .await
            .expect("cross-account lookup must succeed")
    );

    store
        .disable(&NotificationPolicyDisableRequest {
            account: account_a.clone(),
            policy_family_id: denied.policy_family_id,
            expected_generation: denied.generation,
            command_source_event_id: None,
            audit_summary: "acceptance automatic reply denial tombstone".into(),
        })
        .await
        .expect("automatic reply tombstone must persist");
    assert!(
        !store
            .automatic_reply_is_denied(&account_a, "actor-a")
            .await
            .expect("tombstone lookup must succeed")
    );
}

#[tokio::test]
#[ignore = "requires isolated MySQL schema created by verify-qqbot-acceptance.ps1"]
async fn notification_policy_evaluation_remind_is_fenced_and_uses_verified_command_recipient() {
    let db = isolated_db().await;
    let managed_subject = format!("accept-policy-eval-{}", Uuid::new_v4().simple());
    let managed_account = account(&managed_subject);
    build_mysql_inbound_event_store(db.clone())
        .begin_connection(&managed_account)
        .await
        .expect("managed account bootstrap must succeed");
    let command_account_id = format!("accept-command-{}", Uuid::new_v4().simple());
    let owner_actor_id = "accept-owner";
    let (request_id, command_account_id, _) = insert_policy_evaluation_fixture(
        &db,
        &managed_account,
        &command_account_id,
        owner_actor_id,
    )
    .await;
    let store: Arc<dyn NotificationPolicyStoreT> =
        build_mysql_notification_policy_store(db.clone());
    let (claim, snapshot) = claim_and_snapshot(&store, &request_id).await;
    assert_eq!(claim.evaluation_request_id.as_str(), request_id);
    assert_eq!(snapshot.owner_binding.managed_account, managed_account);
    assert_eq!(
        snapshot.owner_binding.command_account.account_id,
        command_account_id
    );
    assert_eq!(snapshot.owner_binding.owner_actor_id, owner_actor_id);

    let evaluator = NotificationPolicyEvaluator;
    let plan = evaluator.evaluate(
        &snapshot
            .evaluation_input(unix_now_secs())
            .expect("snapshot input is valid"),
    );
    assert_eq!(plan.outcome, NotificationOutcome::Remind);
    let commit = evaluation_commit(claim, snapshot, plan);
    let result = store
        .commit_evaluation(&commit)
        .await
        .expect("fenced remind commit must succeed");
    assert_eq!(result, EvaluationCommitResult::Applied);
    assert_eq!(
        scalar_u64(
            &db,
            "SELECT CAST(COUNT(*) AS UNSIGNED) AS value FROM secretary_notification_decisions \
             WHERE evaluation_request_id = ?",
            vec![request_id.clone().into()],
        )
        .await,
        1,
        "a successful evaluation must produce exactly one decision",
    );
    assert_eq!(
        scalar_u64(
            &db,
            "SELECT CAST(COUNT(*) AS UNSIGNED) AS value FROM secretary_notification_outbox AS outbox \
             INNER JOIN secretary_accounts AS command_account ON command_account.id = outbox.command_account_id \
             WHERE outbox.notification_kind = 'owner_policy_reminder' \
               AND command_account.source_channel = 'qq_open_platform' \
               AND command_account.platform_account_id = ? AND outbox.owner_actor_id = ?",
            vec![command_account_id.into(), owner_actor_id.into()],
        )
        .await,
        1,
        "outbox recipient must come from the revalidated command binding, not candidate account",
    );
    assert_eq!(
        scalar_string(
            &db,
            "SELECT request_status AS value FROM secretary_notification_evaluation_requests \
             WHERE evaluation_request_id = ?",
            vec![request_id.clone().into()],
        )
        .await,
        "completed",
    );
    let source_id = scalar_string(
        &db,
        "SELECT candidate.source_id AS value FROM secretary_notification_candidates AS candidate \
         INNER JOIN secretary_notification_evaluation_requests AS request \
           ON request.notification_candidate_id = candidate.notification_candidate_id \
         WHERE request.evaluation_request_id = ?",
        vec![request_id.clone().into()],
    )
    .await;
    assert_eq!(
        scalar_string(
            &db,
            "SELECT occurrence_id AS value FROM secretary_notification_outbox AS outbox \
             INNER JOIN secretary_notification_decisions AS decision \
               ON decision.notification_decision_id = outbox.notification_decision_id \
             WHERE decision.evaluation_request_id = ?",
            vec![request_id.clone().into()],
        )
        .await,
        reminder_occurrence_id("agenda", &source_id, 1),
        "occurrence must derive from stable source identity, not generated candidate UUID",
    );
    assert_eq!(
        store.commit_evaluation(&commit).await,
        Ok(EvaluationCommitResult::LeaseLost),
        "the consumed lease must not commit a second decision or outbox row",
    );
}

#[tokio::test]
#[ignore = "requires isolated MySQL schema created by verify-qqbot-acceptance.ps1"]
async fn notification_policy_evaluation_delay_reclaims_and_appends_decision_without_outbox() {
    let db = isolated_db().await;
    let managed_account = account(&format!("accept-policy-delay-{}", Uuid::new_v4().simple()));
    build_mysql_inbound_event_store(db.clone())
        .begin_connection(&managed_account)
        .await
        .expect("managed account bootstrap must succeed");
    let (request_id, _, _) = insert_policy_evaluation_fixture(
        &db,
        &managed_account,
        &format!("accept-command-delay-{}", Uuid::new_v4().simple()),
        "accept-owner-delay",
    )
    .await;
    let store: Arc<dyn NotificationPolicyStoreT> =
        build_mysql_notification_policy_store(db.clone());
    let (claim, snapshot) = claim_and_snapshot(&store, &request_id).await;
    let first_next_allowed_at = unix_now_secs() + 1;
    let first = evaluation_commit(
        claim,
        snapshot,
        EvaluationPlan {
            outcome: NotificationOutcome::Delay,
            reason: DecisionReason::CategoryPolicy,
            next_allowed_at_unix_secs: Some(first_next_allowed_at),
        },
    );
    assert_eq!(
        store.commit_evaluation(&first).await,
        Ok(EvaluationCommitResult::Applied)
    );
    assert_eq!(
        scalar_string(
            &db,
            "SELECT request_status AS value FROM secretary_notification_evaluation_requests \
             WHERE evaluation_request_id = ?",
            vec![request_id.clone().into()],
        )
        .await,
        "pending"
    );
    assert_eq!(
        scalar_string(
            &db,
            "SELECT candidate_status AS value FROM secretary_notification_candidates AS candidate \
             INNER JOIN secretary_notification_evaluation_requests AS request \
               ON request.notification_candidate_id = candidate.notification_candidate_id \
             WHERE request.evaluation_request_id = ?",
            vec![request_id.clone().into()],
        )
        .await,
        "delayed"
    );
    assert_eq!(
        scalar_u64(
            &db,
            "SELECT CAST(COUNT(*) AS UNSIGNED) AS value FROM secretary_notification_outbox AS outbox \
             INNER JOIN secretary_notification_decisions AS decision \
               ON decision.notification_decision_id = outbox.notification_decision_id \
             WHERE decision.evaluation_request_id = ?",
            vec![request_id.clone().into()],
        )
        .await,
        0
    );

    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "UPDATE secretary_notification_evaluation_requests \
         SET next_allowed_at_unix_secs = ? WHERE evaluation_request_id = ?",
        [unix_now_secs().into(), request_id.clone().into()],
    ))
    .await
    .expect("fixture may advance the deterministic delay window");
    let (claim, snapshot) = claim_and_snapshot(&store, &request_id).await;
    let second = evaluation_commit(
        claim,
        snapshot,
        EvaluationPlan {
            outcome: NotificationOutcome::Delay,
            reason: DecisionReason::CategoryPolicy,
            next_allowed_at_unix_secs: Some(unix_now_secs() + 60),
        },
    );
    assert_eq!(
        store.commit_evaluation(&second).await,
        Ok(EvaluationCommitResult::Applied)
    );
    assert_eq!(
        scalar_u64(
            &db,
            "SELECT CAST(COUNT(*) AS UNSIGNED) AS value FROM secretary_notification_decisions \
             WHERE evaluation_request_id = ?",
            vec![request_id.clone().into()],
        )
        .await,
        2,
        "each delay cycle must retain its immutable Decision audit row",
    );
    assert_eq!(
        scalar_u64(
            &db,
            "SELECT CAST(COUNT(*) AS UNSIGNED) AS value FROM secretary_notification_decisions \
             WHERE evaluation_request_id = ? AND previous_decision_id IS NOT NULL",
            vec![request_id.into()],
        )
        .await,
        1,
        "the second cycle must link to the first Decision",
    );
}

#[tokio::test]
#[ignore = "requires isolated MySQL schema created by verify-qqbot-acceptance.ps1"]
async fn notification_policy_evaluation_claim_reclaims_expired_lease_and_rejects_old_token() {
    let db = isolated_db().await;
    let managed_account = account(&format!("accept-policy-lease-{}", Uuid::new_v4().simple()));
    build_mysql_inbound_event_store(db.clone())
        .begin_connection(&managed_account)
        .await
        .expect("managed account bootstrap must succeed");
    let (request_id, _, _) = insert_policy_evaluation_fixture(
        &db,
        &managed_account,
        &format!("accept-command-lease-{}", Uuid::new_v4().simple()),
        "accept-owner-lease",
    )
    .await;
    let store: Arc<dyn NotificationPolicyStoreT> =
        build_mysql_notification_policy_store(db.clone());
    let now = unix_now_secs();
    let first_claim = store
        .claim_evaluation("first-worker", now, 1)
        .await
        .expect("first claim must succeed")
        .expect("fixture request must be claimed");
    let first_snapshot = store
        .load_evaluation_snapshot(&first_claim)
        .await
        .expect("first lease snapshot must load");
    assert!(
        store
            .claim_evaluation("second-worker", now, 60)
            .await
            .expect("active lease lookup must succeed")
            .is_none(),
        "a non-expired lease cannot be claimed concurrently",
    );

    let second_claim = store
        .claim_evaluation("second-worker", now + 2, 60)
        .await
        .expect("expired lease reclaim must succeed")
        .expect("expired fixture request must be reclaimed");
    assert_ne!(first_claim.lease_token, second_claim.lease_token);
    assert_eq!(second_claim.attempt, first_claim.attempt + 1);
    assert_eq!(
        store
            .commit_evaluation(&evaluation_commit(
                first_claim,
                first_snapshot,
                EvaluationPlan {
                    outcome: NotificationOutcome::Suppress,
                    reason: DecisionReason::CategoryPolicy,
                    next_allowed_at_unix_secs: None,
                },
            ))
            .await,
        Ok(EvaluationCommitResult::LeaseLost),
        "the old lease token must never write after reclaim",
    );
    assert_eq!(
        scalar_string(
            &db,
            "SELECT request_status AS value FROM secretary_notification_evaluation_requests \
             WHERE evaluation_request_id = ?",
            vec![request_id.clone().into()],
        )
        .await,
        "claimed"
    );
    // 后续检查共享同一隔离 schema；此处特意保留的过期 claim 不能污染 recovery 场景。
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "DELETE FROM secretary_notification_evaluation_requests WHERE evaluation_request_id = ?",
        [request_id.into()],
    ))
    .await
    .expect("expired-lease fixture cleanup must succeed");
}

#[tokio::test]
#[ignore = "requires isolated MySQL schema created by verify-qqbot-acceptance.ps1"]
async fn notification_policy_evaluation_stale_candidate_requeues_without_decision_or_outbox() {
    let db = isolated_db().await;
    let managed_account = account(&format!("accept-policy-stale-{}", Uuid::new_v4().simple()));
    build_mysql_inbound_event_store(db.clone())
        .begin_connection(&managed_account)
        .await
        .expect("managed account bootstrap must succeed");
    let (request_id, _, _) = insert_policy_evaluation_fixture(
        &db,
        &managed_account,
        &format!("accept-command-stale-{}", Uuid::new_v4().simple()),
        "accept-owner-stale",
    )
    .await;
    let store: Arc<dyn NotificationPolicyStoreT> =
        build_mysql_notification_policy_store(db.clone());
    let (claim, snapshot) = claim_and_snapshot(&store, &request_id).await;
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "UPDATE secretary_notification_candidates AS candidate \
         INNER JOIN secretary_notification_evaluation_requests AS request \
           ON request.notification_candidate_id = candidate.notification_candidate_id \
         SET candidate.source_version = candidate.source_version + 1 \
         WHERE request.evaluation_request_id = ?",
        [request_id.clone().into()],
    ))
    .await
    .expect("fixture must advance candidate source version");
    assert_eq!(
        store
            .commit_evaluation(&evaluation_commit(
                claim,
                snapshot,
                EvaluationPlan {
                    outcome: NotificationOutcome::Remind,
                    reason: DecisionReason::AccountDefaultPolicy,
                    next_allowed_at_unix_secs: None,
                },
            ))
            .await,
        Ok(EvaluationCommitResult::SnapshotStale)
    );
    assert_eq!(
        scalar_string(
            &db,
            "SELECT request_status AS value FROM secretary_notification_evaluation_requests \
             WHERE evaluation_request_id = ?",
            vec![request_id.clone().into()],
        )
        .await,
        "pending",
        "a stale request must be eligible for a newly generated evaluation",
    );
    assert_eq!(
        scalar_u64(
            &db,
            "SELECT CAST(COUNT(*) AS UNSIGNED) AS value FROM secretary_notification_decisions \
             WHERE evaluation_request_id = ?",
            vec![request_id.clone().into()],
        )
        .await,
        0,
        "a stale snapshot must not create a Decision",
    );
    // 本检查验证重排语义后不再需要该无 Decision 的 fixture；删除它以免同一隔离
    // schema 后续检查领取到旧 pending Request，而不是自己的场景数据。
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "DELETE FROM secretary_notification_evaluation_requests WHERE evaluation_request_id = ?",
        [request_id.into()],
    ))
    .await
    .expect("stale fixture request cleanup must succeed");
}

#[tokio::test]
#[ignore = "requires isolated MySQL schema created by verify-qqbot-acceptance.ps1"]
async fn notification_policy_evaluation_remind_duplicate_occurrence_is_idempotent() {
    let db = isolated_db().await;
    let managed = account(&format!(
        "accept-policy-idempotent-{}",
        Uuid::new_v4().simple()
    ));
    build_mysql_inbound_event_store(db.clone())
        .begin_connection(&managed)
        .await
        .expect("managed account bootstrap");
    let (request_id, _, _) = insert_policy_evaluation_fixture(
        &db,
        &managed,
        &format!("accept-command-idempotent-{}", Uuid::new_v4().simple()),
        "accept-owner-idempotent",
    )
    .await;
    let source_id = scalar_string(
        &db,
        "SELECT candidate.source_id AS value FROM secretary_notification_candidates candidate \
         INNER JOIN secretary_notification_evaluation_requests request \
           ON request.notification_candidate_id = candidate.notification_candidate_id \
         WHERE request.evaluation_request_id = ?",
        vec![request_id.clone().into()],
    )
    .await;
    let store: Arc<dyn NotificationPolicyStoreT> =
        build_mysql_notification_policy_store(db.clone());
    let (claim, snapshot) = claim_and_snapshot(&store, &request_id).await;
    let commit = evaluation_commit(
        claim,
        snapshot,
        EvaluationPlan {
            outcome: NotificationOutcome::Remind,
            reason: DecisionReason::AccountDefaultPolicy,
            next_allowed_at_unix_secs: None,
        },
    );
    assert_eq!(
        store.commit_evaluation(&commit).await,
        Ok(EvaluationCommitResult::Applied)
    );
    assert_eq!(
        store.commit_evaluation(&commit).await,
        Ok(EvaluationCommitResult::LeaseLost),
        "同一租约的重复提交不能重新写入 Decision 或 Outbox"
    );
    assert_eq!(
        scalar_u64(
            &db,
            "SELECT CAST(COUNT(*) AS UNSIGNED) AS value FROM secretary_notification_outbox \
             WHERE occurrence_id = ?",
            vec![reminder_occurrence_id("agenda", &source_id, 1).into()],
        )
        .await,
        1,
        "一次有效候选只能产生一个稳定 occurrence 的 Outbox 行",
    );
    assert_eq!(
        scalar_u64(
            &db,
            "SELECT CAST(COUNT(*) AS UNSIGNED) AS value FROM secretary_notification_decisions \
             WHERE evaluation_request_id = ?",
            vec![request_id.into()],
        )
        .await,
        1,
        "重复提交不得追加第二条 Decision",
    );
}

#[tokio::test]
#[ignore = "requires isolated MySQL schema created by verify-qqbot-acceptance.ps1"]
async fn notification_policy_evaluation_epoch_drift_requeues_empty_snapshot() {
    let db = isolated_db().await;
    let managed = account(&format!("accept-policy-epoch-{}", Uuid::new_v4().simple()));
    build_mysql_inbound_event_store(db.clone())
        .begin_connection(&managed)
        .await
        .expect("managed account bootstrap");
    let (request_id, _, _) = insert_policy_evaluation_fixture(
        &db,
        &managed,
        &format!("accept-command-epoch-{}", Uuid::new_v4().simple()),
        "accept-owner-epoch",
    )
    .await;
    let store: Arc<dyn NotificationPolicyStoreT> =
        build_mysql_notification_policy_store(db.clone());
    let (claim, snapshot) = claim_and_snapshot(&store, &request_id).await;
    assert!(
        snapshot.family_generations.is_empty(),
        "fixture must prove an empty snapshot is fenced"
    );
    store
        .create_or_replace(&policy_write_request(managed.clone(), None))
        .await
        .expect("new family must advance epoch");
    assert_eq!(
        store
            .commit_evaluation(&evaluation_commit(
                claim,
                snapshot,
                EvaluationPlan {
                    outcome: NotificationOutcome::Remind,
                    reason: DecisionReason::AccountDefaultPolicy,
                    next_allowed_at_unix_secs: None
                }
            ))
            .await,
        Ok(EvaluationCommitResult::SnapshotStale)
    );
    assert_eq!(scalar_u64(&db, "SELECT CAST(COUNT(*) AS UNSIGNED) AS value FROM secretary_notification_decisions WHERE evaluation_request_id = ?", vec![request_id.clone().into()]).await, 0);
    assert_eq!(scalar_string(&db, "SELECT request_status AS value FROM secretary_notification_evaluation_requests WHERE evaluation_request_id = ?", vec![request_id.clone().into()]).await, "pending");
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "DELETE FROM secretary_notification_evaluation_requests WHERE evaluation_request_id = ?",
        [request_id.into()],
    ))
    .await
    .expect("cleanup stale request");
}

#[tokio::test]
#[ignore = "requires isolated MySQL schema created by verify-qqbot-acceptance.ps1"]
async fn notification_policy_evaluation_family_head_drift_rejects_old_snapshot() {
    let db = isolated_db().await;
    let managed = account(&format!(
        "accept-policy-family-fence-{}",
        Uuid::new_v4().simple()
    ));
    build_mysql_inbound_event_store(db.clone())
        .begin_connection(&managed)
        .await
        .expect("managed account bootstrap");
    let store: Arc<dyn NotificationPolicyStoreT> =
        build_mysql_notification_policy_store(db.clone());
    let family = store
        .create_or_replace(&policy_write_request(managed.clone(), None))
        .await
        .expect("initial family");
    let (request_id, _, _) = insert_policy_evaluation_fixture(
        &db,
        &managed,
        &format!("accept-command-family-{}", Uuid::new_v4().simple()),
        "accept-owner-family",
    )
    .await;
    let (claim, snapshot) = claim_and_snapshot(&store, &request_id).await;
    store
        .create_or_replace(&policy_write_request(
            managed,
            Some(family.policy_family_id),
        ))
        .await
        .expect("head replacement");
    assert_eq!(
        store
            .commit_evaluation(&evaluation_commit(
                claim,
                snapshot,
                EvaluationPlan {
                    outcome: NotificationOutcome::Suppress,
                    reason: DecisionReason::CategoryPolicy,
                    next_allowed_at_unix_secs: None
                }
            ))
            .await,
        Ok(EvaluationCommitResult::SnapshotStale)
    );
    assert_eq!(scalar_u64(&db, "SELECT CAST(COUNT(*) AS UNSIGNED) AS value FROM secretary_notification_decisions WHERE evaluation_request_id = ?", vec![request_id.clone().into()]).await, 0);
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "DELETE FROM secretary_notification_evaluation_requests WHERE evaluation_request_id = ?",
        [request_id.into()],
    ))
    .await
    .expect("cleanup stale request");
}

#[tokio::test]
#[ignore = "requires isolated MySQL schema created by verify-qqbot-acceptance.ps1"]
async fn notification_policy_evaluation_owner_binding_drift_rejects_old_snapshot() {
    let db = isolated_db().await;
    let managed = account(&format!(
        "accept-policy-binding-fence-{}",
        Uuid::new_v4().simple()
    ));
    build_mysql_inbound_event_store(db.clone())
        .begin_connection(&managed)
        .await
        .expect("managed account bootstrap");
    let (request_id, _, _) = insert_policy_evaluation_fixture(
        &db,
        &managed,
        &format!("accept-command-binding-{}", Uuid::new_v4().simple()),
        "accept-owner-binding",
    )
    .await;
    let store: Arc<dyn NotificationPolicyStoreT> =
        build_mysql_notification_policy_store(db.clone());
    let (claim, snapshot) = claim_and_snapshot(&store, &request_id).await;
    db.execute_raw(Statement::from_sql_and_values(DatabaseBackend::MySql, "UPDATE secretary_owner_bindings SET owner_actor_id = 'changed-owner' WHERE owner_actor_id = 'accept-owner-binding'", [])).await.expect("binding drift fixture");
    assert_eq!(
        store
            .commit_evaluation(&evaluation_commit(
                claim,
                snapshot,
                EvaluationPlan {
                    outcome: NotificationOutcome::Remind,
                    reason: DecisionReason::AccountDefaultPolicy,
                    next_allowed_at_unix_secs: None
                }
            ))
            .await,
        Ok(EvaluationCommitResult::SnapshotStale)
    );
    assert_eq!(scalar_u64(&db, "SELECT CAST(COUNT(*) AS UNSIGNED) AS value FROM secretary_notification_decisions WHERE evaluation_request_id = ?", vec![request_id.clone().into()]).await, 0);
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "DELETE FROM secretary_notification_evaluation_requests WHERE evaluation_request_id = ?",
        [request_id.into()],
    ))
    .await
    .expect("cleanup stale request");
}

#[tokio::test]
#[ignore = "requires isolated MySQL schema created by verify-qqbot-acceptance.ps1"]
async fn notification_policy_evaluation_terminal_outcomes_do_not_enqueue_or_recover() {
    for (outcome, reason, expected_candidate) in [
        (
            NotificationOutcome::ScheduleTimeAmbiguous,
            DecisionReason::ScheduleTimeAmbiguous,
            "failed_terminal",
        ),
        (
            NotificationOutcome::DeliveryWindowExpired,
            DecisionReason::AccountDefaultPolicy,
            "expired",
        ),
    ] {
        let db = isolated_db().await;
        let managed = account(&format!(
            "accept-policy-terminal-{}",
            Uuid::new_v4().simple()
        ));
        build_mysql_inbound_event_store(db.clone())
            .begin_connection(&managed)
            .await
            .expect("managed account bootstrap");
        let (request_id, _, _) = insert_policy_evaluation_fixture(
            &db,
            &managed,
            &format!("accept-command-terminal-{}", Uuid::new_v4().simple()),
            "accept-owner-terminal",
        )
        .await;
        let store: Arc<dyn NotificationPolicyStoreT> =
            build_mysql_notification_policy_store(db.clone());
        let (claim, snapshot) = claim_and_snapshot(&store, &request_id).await;
        assert_eq!(
            store
                .commit_evaluation(&evaluation_commit(
                    claim,
                    snapshot,
                    EvaluationPlan {
                        outcome,
                        reason,
                        next_allowed_at_unix_secs: None
                    }
                ))
                .await,
            Ok(EvaluationCommitResult::Applied)
        );
        assert_eq!(scalar_string(&db, "SELECT request_status AS value FROM secretary_notification_evaluation_requests WHERE evaluation_request_id = ?", vec![request_id.clone().into()]).await, "terminal");
        assert_eq!(scalar_string(&db, "SELECT candidate.candidate_status AS value FROM secretary_notification_candidates candidate INNER JOIN secretary_notification_evaluation_requests request ON request.notification_candidate_id = candidate.notification_candidate_id WHERE request.evaluation_request_id = ?", vec![request_id.clone().into()]).await, expected_candidate);
        assert_eq!(
            scalar_u64(
                &db,
                "SELECT CAST(COUNT(*) AS UNSIGNED) AS value FROM secretary_notification_outbox outbox \
                 INNER JOIN secretary_notification_decisions decision \
                   ON decision.notification_decision_id = outbox.notification_decision_id \
                 WHERE decision.evaluation_request_id = ?",
                vec![request_id.clone().into()],
            )
            .await,
            0,
        );
        store
            .recover_expired_evaluations(unix_now_secs() + 86_400, 10)
            .await
            .expect("global recovery may process other parallel fixtures but must complete");
        assert_eq!(
            scalar_string(
                &db,
                "SELECT request_status AS value FROM secretary_notification_evaluation_requests \
                 WHERE evaluation_request_id = ?",
                vec![request_id.clone().into()],
            )
            .await,
            "terminal",
            "recovery must not reopen this terminal request",
        );
        assert_eq!(
            scalar_u64(
                &db,
                "SELECT CAST(COUNT(*) AS UNSIGNED) AS value FROM secretary_notification_evaluation_requests \
                 WHERE evaluation_request_id = ? AND lease_token IS NULL AND lease_expires_at_unix_secs IS NULL",
                vec![request_id.clone().into()],
            )
            .await,
            1,
            "terminal request must remain without a lease",
        );
        assert_eq!(
            scalar_u64(
                &db,
                "SELECT CAST(COUNT(*) AS UNSIGNED) AS value FROM secretary_notification_decisions \
                 WHERE evaluation_request_id = ?",
                vec![request_id.into()],
            )
            .await,
            1,
            "recovery must not append a Decision for this terminal request",
        );
    }
}

#[tokio::test]
#[ignore = "requires isolated MySQL schema created by verify-qqbot-acceptance.ps1"]
async fn notification_policy_evaluation_outbox_failure_rolls_back_decision_and_lease() {
    let db = isolated_db().await;
    let managed_account = account(&format!(
        "accept-policy-outbox-rollback-{}",
        Uuid::new_v4().simple()
    ));
    build_mysql_inbound_event_store(db.clone())
        .begin_connection(&managed_account)
        .await
        .expect("managed account bootstrap must succeed");
    let (request_id, _, _) = insert_policy_evaluation_fixture(
        &db,
        &managed_account,
        &format!("accept-command-outbox-rollback-{}", Uuid::new_v4().simple()),
        "accept-owner-outbox-rollback",
    )
    .await;
    let store: Arc<dyn NotificationPolicyStoreT> =
        build_mysql_notification_policy_store(db.clone());
    let (claim, snapshot) = claim_and_snapshot(&store, &request_id).await;
    let lease_token = claim.lease_token.as_str().to_owned();

    let candidate_id = scalar_string(
        &db,
        "SELECT notification_candidate_id AS value FROM secretary_notification_evaluation_requests \
         WHERE evaluation_request_id = ?",
        vec![request_id.clone().into()],
    )
    .await;
    let trigger_name = format!("policy_outbox_fail_{}", Uuid::new_v4().simple());
    // 触发器只影响此 fixture 的 candidate；避免替换共享 schema 的 CHECK 约束而污染并行测试。
    db.execute_unprepared(&format!(
        "CREATE TRIGGER {trigger_name} BEFORE INSERT ON secretary_notification_outbox \
         FOR EACH ROW SET NEW.notification_kind = IF(NEW.notification_candidate_id = '{candidate_id}', \
         'invalid_policy_notification_kind', NEW.notification_kind)"
    ))
    .await
    .expect("isolated fixture must install candidate-scoped outbox fault");

    let commit_result = store
        .commit_evaluation(&evaluation_commit(
            claim,
            snapshot,
            EvaluationPlan {
                outcome: NotificationOutcome::Remind,
                reason: DecisionReason::AccountDefaultPolicy,
                next_allowed_at_unix_secs: None,
            },
        ))
        .await;

    // 在读取断言状态前撤销故障注入；之后的测试不依赖本测试的执行顺序。
    db.execute_unprepared(&format!("DROP TRIGGER {trigger_name}"))
        .await
        .expect("candidate-scoped outbox fault must be removed");
    let decision_count = scalar_u64(
        &db,
        "SELECT CAST(COUNT(*) AS UNSIGNED) AS value FROM secretary_notification_decisions \
         WHERE evaluation_request_id = ?",
        vec![request_id.clone().into()],
    )
    .await;
    let outbox_count = scalar_u64(
        &db,
        "SELECT CAST(COUNT(*) AS UNSIGNED) AS value FROM secretary_notification_outbox AS outbox \
         INNER JOIN secretary_notification_evaluation_requests AS request \
           ON request.notification_candidate_id = outbox.notification_candidate_id \
         WHERE request.evaluation_request_id = ?",
        vec![request_id.clone().into()],
    )
    .await;
    let request_status = scalar_string(
        &db,
        "SELECT request_status AS value FROM secretary_notification_evaluation_requests \
         WHERE evaluation_request_id = ?",
        vec![request_id.clone().into()],
    )
    .await;
    let persisted_lease_token = scalar_string(
        &db,
        "SELECT lease_token AS value FROM secretary_notification_evaluation_requests \
         WHERE evaluation_request_id = ?",
        vec![request_id.clone().into()],
    )
    .await;
    let candidate_status = scalar_string(
        &db,
        "SELECT candidate.candidate_status AS value FROM secretary_notification_candidates AS candidate \
         INNER JOIN secretary_notification_evaluation_requests AS request \
           ON request.notification_candidate_id = candidate.notification_candidate_id \
         WHERE request.evaluation_request_id = ?",
        vec![request_id.clone().into()],
    )
    .await;
    // 本场景有意保留 claimed 状态用于读取事务回滚结果；断言前清理以免污染共享队列。
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "DELETE FROM secretary_notification_evaluation_requests WHERE evaluation_request_id = ?",
        [request_id.into()],
    ))
    .await
    .expect("rollback fixture request must be removed from the shared queue");

    assert_eq!(commit_result, Err(NotificationPolicyStoreError::Database));
    assert_eq!(
        decision_count, 0,
        "outbox failure must roll back the Decision inserted earlier in the transaction",
    );
    assert_eq!(
        outbox_count, 0,
        "failed transaction must not leave an outbox row"
    );
    assert_eq!(request_status, "claimed");
    assert_eq!(
        persisted_lease_token, lease_token,
        "failed transaction must not alter the active lease",
    );
    assert_eq!(candidate_status, "pending");
}

#[tokio::test]
#[ignore = "requires isolated MySQL schema created by verify-qqbot-acceptance.ps1"]
async fn notification_policy_evaluation_suppress_is_terminal_without_outbox_or_recovery() {
    let db = isolated_db().await;
    let managed_account = account(&format!(
        "accept-policy-suppress-{}",
        Uuid::new_v4().simple()
    ));
    build_mysql_inbound_event_store(db.clone())
        .begin_connection(&managed_account)
        .await
        .expect("managed account bootstrap must succeed");
    let (request_id, _, _) = insert_policy_evaluation_fixture(
        &db,
        &managed_account,
        &format!("accept-command-suppress-{}", Uuid::new_v4().simple()),
        "accept-owner-suppress",
    )
    .await;
    let store: Arc<dyn NotificationPolicyStoreT> =
        build_mysql_notification_policy_store(db.clone());
    let (claim, snapshot) = claim_and_snapshot(&store, &request_id).await;
    assert_eq!(
        store
            .commit_evaluation(&evaluation_commit(
                claim,
                snapshot,
                EvaluationPlan {
                    outcome: NotificationOutcome::Suppress,
                    reason: DecisionReason::CategoryPolicy,
                    next_allowed_at_unix_secs: None,
                },
            ))
            .await,
        Ok(EvaluationCommitResult::Applied)
    );
    assert_eq!(
        scalar_string(
            &db,
            "SELECT request_status AS value FROM secretary_notification_evaluation_requests \
             WHERE evaluation_request_id = ?",
            vec![request_id.clone().into()],
        )
        .await,
        "terminal"
    );
    assert_eq!(
        scalar_string(
            &db,
            "SELECT candidate_status AS value FROM secretary_notification_candidates AS candidate \
             INNER JOIN secretary_notification_evaluation_requests AS request \
               ON request.notification_candidate_id = candidate.notification_candidate_id \
             WHERE request.evaluation_request_id = ?",
            vec![request_id.clone().into()],
        )
        .await,
        "suppressed"
    );
    assert_eq!(
        store
            .recover_expired_evaluations(unix_now_secs() + 86_400, 10)
            .await,
        Ok(0),
        "recovery must not reopen terminal requests",
    );
    assert_eq!(
        scalar_u64(
            &db,
            "SELECT CAST(COUNT(*) AS UNSIGNED) AS value FROM secretary_notification_outbox AS outbox \
             INNER JOIN secretary_notification_decisions AS decision \
               ON decision.notification_decision_id = outbox.notification_decision_id \
             WHERE decision.evaluation_request_id = ?",
            vec![request_id.into()],
        )
        .await,
        0,
        "Suppress must never enqueue a delivery",
    );
}

#[tokio::test]
#[ignore = "requires isolated MySQL schema created by verify-qqbot-acceptance.ps1"]
async fn notification_feedback_promotion_persists_a_durable_rule_once() {
    let db = isolated_db().await;
    let account_subject = format!("accept-feedback-promotion-{}", Uuid::new_v4().simple());
    let account = account(&account_subject);
    let inbound = build_mysql_inbound_event_store(db.clone());
    inbound
        .begin_connection(&account)
        .await
        .expect("account bootstrap must succeed");
    let candidate_source_event_id = inbound
        .insert_message_if_absent(&message(&account_subject, "671260344", "candidate-source"))
        .await
        .expect("candidate source message must persist")
        .source_event_id()
        .as_str()
        .to_owned();
    let command_source_event_id = inbound
        .insert_message_if_absent(&message(&account_subject, "671260344", "feedback-command"))
        .await
        .expect("feedback command source message must persist")
        .source_event_id()
        .as_str()
        .to_owned();
    let candidate_id = Uuid::new_v4().to_string();
    let match_key = personal_secretary::NotificationMatchKeyV1::new(
        account.clone(),
        MatchField::Absent,
        MatchField::Absent,
        MatchField::Known(NotificationCategory::Agenda),
        MatchField::Known(false),
        MatchField::Known(StructuredImportance::Normal),
        MatchField::Known(EventKind::AgendaDue),
    )
    .expect("valid promotion match key");
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "INSERT INTO secretary_notification_candidates \
         (notification_candidate_id, account_id, source_kind, source_id, source_version, match_key_json) \
         SELECT ?, id, ?, ?, ?, CAST(? AS JSON) FROM secretary_accounts \
         WHERE source_channel = ? AND platform_account_id = ?",
        [
            candidate_id.clone().into(),
            "agenda".into(),
            candidate_source_event_id.clone().into(),
            1_u64.into(),
            serde_json::to_string(&match_key)
                .expect("match key fixture serializes")
                .into(),
            account.channel.as_str().into(),
            account.account_id.clone().into(),
        ],
    ))
    .await
    .expect("notification candidate must persist");
    let store = build_mysql_notification_policy_store(db.clone());
    let request = NotificationFeedbackRequest {
        candidate: personal_secretary::NotificationCandidateRef::new(
            "agenda",
            candidate_source_event_id,
            1,
            account.clone(),
        )
        .expect("candidate reference is valid"),
        match_key,
        important: false,
        promote_to_rule: true,
        command_source_event_id: command_source_event_id.clone(),
    };
    let before = policy_epoch(&db, &account).await;

    store
        .record_feedback(&request)
        .await
        .expect("feedback promotion must persist");
    store
        .record_feedback(&request)
        .await
        .expect("feedback promotion replay must be idempotent");

    assert_eq!(policy_epoch(&db, &account).await, before + 1);
    assert_eq!(
        scalar_u64(
            &db,
            "SELECT CAST(COUNT(*) AS UNSIGNED) AS value FROM secretary_notification_feedback \
             WHERE command_source_event_id = ?",
            vec![command_source_event_id.into()],
        )
        .await,
        1,
    );
    assert_eq!(
        scalar_u64(
            &db,
            "SELECT CAST(COUNT(*) AS UNSIGNED) AS value \
             FROM secretary_notification_policy_revisions WHERE audit_summary = ?",
            vec!["owner notification feedback promotion".into()],
        )
        .await,
        1,
    );
    assert_eq!(
        scalar_string(
            &db,
            "SELECT revision_kind AS value FROM secretary_notification_policy_revisions \
             WHERE audit_summary = ?",
            vec!["owner notification feedback promotion".into()],
        )
        .await,
        "rule",
    );
}

#[test]
#[ignore = "executed only by verify-qqbot-acceptance.ps1"]
fn acceptance_recall_identity_rejects_database_truncation() {
    assert!(RecallEventId::new("recall-group-1839717811-671260344-1234567890123456789").is_err());
    assert!(RecallEventId::new(Uuid::new_v4().to_string()).is_ok());
}

#[tokio::test]
#[ignore = "requires isolated MySQL schema created by verify-qqbot-acceptance.ps1"]
async fn acceptance_recall_is_persisted_as_source_event() {
    let db = isolated_db().await;
    let account_subject = format!("accept-recall-source-{}", Uuid::new_v4().simple());
    let inbound = build_mysql_inbound_event_store(db.clone());
    inbound
        .begin_connection(&account(&account_subject))
        .await
        .expect("account bootstrap must succeed");
    let recall_id = Uuid::new_v4().to_string();
    RecallUseCase::new(build_mysql_recall_store(db.clone()))
        .handle_recall(&recall(
            &recall_id,
            &account_subject,
            "671260344",
            "987654321012345678",
        ))
        .await
        .expect("recall persistence must succeed");
    assert_eq!(
        scalar_u64(
            &db,
            "SELECT COUNT(*) AS value FROM secretary_source_events WHERE source_event_id = ?",
            vec![recall_id.into()]
        )
        .await,
        1
    );
}

#[tokio::test]
#[ignore = "requires isolated MySQL schema created by verify-qqbot-acceptance.ps1"]
async fn acceptance_pending_recall_auto_applies_after_message_ingestion() {
    let db = isolated_db().await;
    let account_subject = format!("accept-pending-{}", Uuid::new_v4().simple());
    let group_id = "671260344";
    let message_id = "887766554433221100";
    let inbound = build_mysql_inbound_event_store(db.clone());
    inbound
        .begin_connection(&account(&account_subject))
        .await
        .expect("account bootstrap must succeed");
    let status = RecallUseCase::new(build_mysql_recall_store(db.clone()))
        .handle_recall(&recall(
            Uuid::new_v4().to_string(),
            &account_subject,
            group_id,
            message_id,
        ))
        .await
        .expect("pending recall persistence must succeed");
    assert_eq!(status, TombstoneStatus::Pending);
    inbound
        .insert_message_if_absent(&message(&account_subject, group_id, message_id))
        .await
        .expect("original message ingestion must succeed");
    assert_eq!(
        scalar_string(
            &db,
            "SELECT status AS value FROM secretary_message_tombstones WHERE correlation_key = ?",
            vec![format!("napcat:{account_subject}:group:{group_id}:{message_id}").into()]
        )
        .await,
        "applied"
    );
}

#[tokio::test]
#[ignore = "requires isolated MySQL schema created by verify-qqbot-acceptance.ps1"]
async fn acceptance_recall_propagates_to_existing_artifacts() {
    let db = isolated_db().await;
    let account_subject = format!("accept-artifact-{}", Uuid::new_v4().simple());
    let group_id = "671260344";
    let message_id = "112233445566778899";
    let source_event_id = build_mysql_inbound_event_store(db.clone())
        .insert_message_if_absent(&message(&account_subject, group_id, message_id))
        .await
        .expect("source message ingestion must succeed")
        .source_event_id()
        .clone();
    let artifact_id = ArtifactId::new(Uuid::new_v4().to_string()).expect("valid artifact id");
    let envelope = ArtifactEnvelope::new(
        artifact_id.clone(),
        account(&account_subject),
        source_event_id,
        group(group_id),
        ArtifactKind::Image,
        "platform-file-reference",
        ContentTrustLevel::Normal,
        1_800_000_000,
        Some(1_800_003_600),
    )
    .expect("valid artifact envelope");
    ArtifactUseCase::new(build_mysql_artifact_store(db.clone()))
        .create(&envelope)
        .await
        .expect("artifact creation must succeed");
    RecallUseCase::new(build_mysql_recall_store(db.clone()))
        .handle_recall(&recall(
            Uuid::new_v4().to_string(),
            &account_subject,
            group_id,
            message_id,
        ))
        .await
        .expect("recall persistence must succeed");
    assert_eq!(
        scalar_string(
            &db,
            "SELECT availability AS value FROM secretary_artifacts WHERE artifact_id = ?",
            vec![artifact_id.as_str().into()]
        )
        .await,
        "recalled"
    );
}

#[tokio::test]
#[ignore = "requires isolated MySQL schema created by verify-qqbot-acceptance.ps1"]
async fn acceptance_gap_creation_freezes_latest_directory_snapshot() {
    let db = isolated_db().await;
    let account_subject = format!("accept-directory-{}", Uuid::new_v4().simple());
    let account = account(&account_subject);
    let inbound = build_mysql_inbound_event_store(db.clone());
    let directory = build_mysql_directory_store(db.clone());
    let epoch = inbound
        .begin_connection(&account)
        .await
        .expect("connection epoch must start");
    directory
        .snapshot_directory(&DirectorySnapshot {
            snapshot_id: DirectorySnapshotId::new(Uuid::new_v4().to_string())
                .expect("valid snapshot id"),
            account: account.clone(),
            source_api: DirectorySourceApi::FriendGroupRecent,
            status: DirectoryStatus::KnownScopesComplete,
            evidence: DirectoryEvidence {
                source_api: Some(DirectorySourceApi::FriendGroupRecent),
                group_count: 1,
                probed_at_unix_secs: 1_800_000_000,
                ..DirectoryEvidence::default()
            },
            scopes: vec![ConversationScope {
                conversation: group("671260344"),
                scope_kind: ScopeKind::Group,
                boundary: None,
                display_name: Some("acceptance-group".into()),
            }],
            created_at_unix_secs: 1_800_000_000,
        })
        .await
        .expect("directory snapshot must persist");
    inbound
        .mark_connection_connected(&epoch)
        .await
        .expect("connection must become connected");
    let gap_id = inbound
        .finish_connection(&epoch, ConnectionEndReason::TransportError)
        .await
        .expect("connection finish must succeed")
        .expect("connected epoch must produce a gap");
    assert_eq!(
        scalar_u64(
            &db,
            "SELECT COUNT(*) AS value FROM secretary_directory_gap_freeze WHERE gap_id = ?",
            vec![gap_id.as_str().into()]
        )
        .await,
        1
    );
}

#[tokio::test]
#[ignore = "requires isolated MySQL schema created by verify-qqbot-acceptance.ps1"]
async fn acceptance_artifact_poison_job_fails_without_starving_later_work() {
    let db = isolated_db().await;
    let account_subject = format!("accept-artifact-poison-{}", Uuid::new_v4().simple());
    db.execute_raw(Statement::from_string(
        DatabaseBackend::MySql,
        "UPDATE secretary_artifact_derivations SET status = 'completed' WHERE status = 'pending'",
    ))
    .await
    .expect("test must isolate its derivation queue");
    let inbound = build_mysql_inbound_event_store(db.clone());
    let poison = inbound
        .insert_message_if_absent(&message(&account_subject, "671260344", "poison"))
        .await
        .expect("poison source message must persist")
        .source_event_id()
        .clone();
    let normal = InboundMessageEnvelope::new(
        SourceMessageRef::new(MessageSource::NapCat, &account_subject, "normal")
            .expect("normal source"),
        group("671260344"),
        VerifiedActor::new(VerifiedActorKind::External, "acceptance-sender").expect("actor"),
        1_800_000_001,
        "image",
        vec![ContentSegment::Media {
            kind: personal_secretary::MediaKind::Image,
            source_key: "normal-image-key".into(),
            source_url: None,
            display_name: None,
        }],
    )
    .expect("normal inbound envelope");
    let normal = inbound
        .insert_message_if_absent(&normal)
        .await
        .expect("normal source message must persist")
        .source_event_id()
        .clone();
    db.execute_raw(Statement::from_sql_and_values(DatabaseBackend::MySql, "UPDATE secretary_message_contents SET segments = CAST(? AS JSON) WHERE source_event_id = ?", [r#"{"not":"a segment list"}"#.into(), poison.as_str().into()])).await.expect("test must inject incompatible persisted segment JSON");
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "UPDATE secretary_artifact_derivations SET created_at = DATE_SUB(created_at, INTERVAL 1 SECOND) WHERE source_event_id = ?",
        [poison.as_str().into()],
    ))
    .await
    .expect("poison job must be the first claim candidate");
    let artifacts = ArtifactUseCase::new(build_mysql_artifact_store(db.clone()));
    artifacts
        .derive_pending(60, 2)
        .await
        .expect("derivation run must finish both jobs");
    assert_eq!(
        scalar_string(
            &db,
            "SELECT status AS value FROM secretary_artifact_derivations WHERE source_event_id = ?",
            vec![poison.as_str().into()]
        )
        .await,
        "failed"
    );
    assert_eq!(scalar_string(&db, "SELECT last_error_code AS value FROM secretary_artifact_derivations WHERE source_event_id = ?", vec![poison.as_str().into()]).await, "invalid_segments_json");
    assert_eq!(
        scalar_string(
            &db,
            "SELECT status AS value FROM secretary_artifact_derivations WHERE source_event_id = ?",
            vec![normal.as_str().into()]
        )
        .await,
        "completed"
    );
    assert_eq!(
        scalar_u64(
            &db,
            "SELECT COUNT(*) AS value FROM secretary_artifacts WHERE source_event_id = ?",
            vec![normal.as_str().into()]
        )
        .await,
        1
    );
}
