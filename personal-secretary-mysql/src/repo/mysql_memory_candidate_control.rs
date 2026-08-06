//! MySQL 记忆候选控制仓储：Owner 对候选的 Approve/Reject 单事务落库。
//!
//! 复用 `mysql_follow_up_control::authorization` 的共享授权与 Receipt 层，
//! 不复制授权 SQL。批准事务原子执行：Receipt 复检 -> 锁账号 -> Action 租约复验
//! -> OwnerCommand/绑定复验 -> 竞争窗口复检 -> 锁候选 -> 校验账号/状态/版本
//! -> 来源复验（仍属本账号、未撤回、允许长期记忆）-> active MemoryFact 冲突检查
//! -> 写 Confirmed Fact 与精确来源 -> 候选 approved 版本 +1 -> 不可变审计
//! -> Effect Receipt -> commit。任一步失败全部回滚。
//! 内容冲突是确定性业务结果（approve_conflict 审计 + 冲突 Receipt，候选保持
//! proposed 且版本不变），不是运行失败。
//!
//! 错误分类：Database -> UnknownCommit；授权、版本与来源失效是确定性失败，
//! 不得伪装成提交不明。

use async_trait::async_trait;
use sea_orm::{
    ConnectionTrait, DatabaseBackend, DatabaseConnection, FromQueryResult, Statement,
    TransactionTrait,
};

use crate::{
    ContentTrustLevel, MemoryCandidate, MemoryCandidateControlEffectRequest,
    MemoryCandidateControlStoreError, MemoryCandidateControlStoreT, MemoryCandidateId,
    MemoryCandidateSource, MemoryCandidateStatus, MemoryCandidateVersion, MemoryFact, MemoryFactId,
    MemoryPayload, SecretaryAction, SecretaryActionReceipt, SourceEventId, ThreadActorRef,
    candidate_to_confirmed_fact,
};

use super::mysql_follow_up_control::authorization::{
    ControlEffectCtx, database_error, insert_receipt_and_commit, load_receipt, lock_account,
    stable_id, verify_action_lease, verify_owner_command,
};

/// 候选版本列是 BIGINT UNSIGNED，行模型必须用 `u64`。
pub(crate) struct MySqlMemoryCandidateControlStore {
    db: DatabaseConnection,
}

impl MySqlMemoryCandidateControlStore {
    pub(crate) fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }
}

/// 控制结果（审计字段 + 有界结果引用）。批准会新建或引用事实；冲突不改任何
/// 业务行（候选保持 proposed，供 Owner 后续决定拒绝或保留）；拒绝无事实。
enum ControlOutcome {
    ApproveCreated {
        previous_version: u64,
        fact_id: MemoryFactId,
    },
    ApproveReferenced {
        previous_version: u64,
        fact_id: MemoryFactId,
    },
    ApproveConflict {
        previous_version: u64,
        fact_id: MemoryFactId,
    },
    Rejected {
        previous_version: u64,
    },
}

impl ControlOutcome {
    fn result_ref(&self, candidate_id: &MemoryCandidateId) -> String {
        match self {
            Self::ApproveCreated { fact_id, .. } => format!(
                "记忆候选 {} 已批准，已形成记忆 {}",
                candidate_id.as_str(),
                fact_id.as_str()
            ),
            Self::ApproveReferenced { fact_id, .. } => format!(
                "记忆候选 {} 已批准，与既有记忆 {} 完全一致，已合并来源，未重复创建",
                candidate_id.as_str(),
                fact_id.as_str()
            ),
            // CMD-009 目标 C：冲突走结构化版本化回执（JSON），供 ReplanDecisionNode
            // 解析后执行一次 L0 回读；摘要是有界中文说明，不含数据库 JSON。
            Self::ApproveConflict { fact_id, .. } => {
                serde_json::to_string(&crate::MemoryCandidateConflictResultV1 {
                    version: 1,
                    candidate_id: candidate_id.clone(),
                    fact_id: fact_id.clone(),
                    reason_code: crate::MemoryConflictReasonCode::ActiveFactPayloadDiffers,
                    summary: "记忆候选与现行记忆内容冲突，未做任何修改；请选择拒绝该候选或保留现状"
                        .into(),
                })
                .expect("MemoryCandidateConflictResultV1 serialization cannot fail")
            }
            Self::Rejected { .. } => format!("记忆候选 {} 已拒绝", candidate_id.as_str()),
        }
    }

    fn audit_fields(
        &self,
    ) -> (
        &'static str,
        &'static str,
        &'static str,
        u64,
        u64,
        Option<&MemoryFactId>,
    ) {
        let (kind, previous, current) = match self {
            Self::ApproveCreated { .. } | Self::ApproveReferenced { .. } => {
                ("approve", "proposed", "approved")
            }
            // 冲突是确定性业务结果：候选保持 proposed、版本不变，Owner 凭审计
            // 与 Receipt 的冲突说明决定后续动作，因此不推进状态机。
            Self::ApproveConflict { .. } => ("approve_conflict", "proposed", "proposed"),
            Self::Rejected { .. } => ("reject", "proposed", "rejected"),
        };
        let (previous_version, current_version) = match self {
            Self::ApproveCreated {
                previous_version, ..
            }
            | Self::ApproveReferenced {
                previous_version, ..
            }
            | Self::ApproveConflict {
                previous_version, ..
            }
            | Self::Rejected { previous_version } => {
                let current = if matches!(self, Self::ApproveConflict { .. }) {
                    *previous_version
                } else {
                    *previous_version + 1
                };
                (*previous_version, current)
            }
        };
        let fact_id = match self {
            Self::ApproveCreated { fact_id, .. }
            | Self::ApproveReferenced { fact_id, .. }
            | Self::ApproveConflict { fact_id, .. } => Some(fact_id),
            Self::Rejected { .. } => None,
        };
        (
            kind,
            previous,
            current,
            previous_version,
            current_version,
            fact_id,
        )
    }
}

#[async_trait]
impl MemoryCandidateControlStoreT for MySqlMemoryCandidateControlStore {
    async fn apply_effect(
        &self,
        request: &MemoryCandidateControlEffectRequest,
    ) -> Result<SecretaryActionReceipt, MemoryCandidateControlStoreError> {
        let transaction = self.db.begin().await.map_err(database_error)?;
        let ctx = ControlEffectCtx::from(request);
        if let Some(receipt) = load_receipt(&transaction, &ctx).await? {
            transaction.commit().await.map_err(database_error)?;
            return Ok(receipt);
        }
        let account_id = lock_account(&transaction, &ctx).await?;
        verify_action_lease(&transaction, &ctx, account_id).await?;
        verify_owner_command(&transaction, &ctx, account_id).await?;
        // 竞争窗口内可能已有并发 Effect 写入回执；提交前再校验一次碰撞。
        if let Some(receipt) = load_receipt(&transaction, &ctx).await? {
            transaction.commit().await.map_err(database_error)?;
            return Ok(receipt);
        }

        let (candidate_id, reason, outcome) = match &request.action {
            SecretaryAction::ApproveMemoryCandidate {
                candidate_id,
                expected_candidate_version,
                reason,
            } => {
                let outcome = apply_approve(
                    &transaction,
                    request,
                    account_id,
                    candidate_id,
                    *expected_candidate_version,
                )
                .await?;
                (candidate_id, reason, outcome)
            }
            SecretaryAction::RejectMemoryCandidate {
                candidate_id,
                expected_candidate_version,
                reason,
            } => {
                let outcome = apply_reject(
                    &transaction,
                    account_id,
                    candidate_id,
                    *expected_candidate_version,
                )
                .await?;
                (candidate_id, reason, outcome)
            }
            _ => {
                return Err(MemoryCandidateControlStoreError::InvalidData(
                    "action is not a memory candidate control".into(),
                ));
            }
        };
        let control_id = stable_id("memory-candidate-control", &request.effect_id);
        insert_control_audit(
            &transaction,
            request,
            account_id,
            &control_id,
            candidate_id,
            reason,
            &outcome,
        )
        .await?;
        insert_receipt_and_commit(transaction, &ctx, outcome.result_ref(candidate_id))
            .await
            .map_err(MemoryCandidateControlStoreError::from)
    }
}

/// 批准候选：proposed -> approved（版本精确 +1），并原子写入 Confirmed MemoryFact。
async fn apply_approve(
    db: &sea_orm::DatabaseTransaction,
    request: &MemoryCandidateControlEffectRequest,
    account_id: u64,
    candidate_id: &MemoryCandidateId,
    expected_version: u64,
) -> Result<ControlOutcome, MemoryCandidateControlStoreError> {
    let row = CandidateRow::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "SELECT candidate_status, candidate_version, candidate_kind, subject_key, \
         CAST(payload_json AS CHAR) AS payload_json \
         FROM secretary_memory_candidates WHERE candidate_id = ? FOR UPDATE",
        [candidate_id.as_str().into()],
    ))
    .one(db)
    .await
    .map_err(database_error)?
    .ok_or_else(|| {
        MemoryCandidateControlStoreError::InvalidData("memory candidate was not found".into())
    })?;
    if row.candidate_status != "proposed" {
        return Err(MemoryCandidateControlStoreError::InvalidData(
            "memory candidate is no longer proposed".into(),
        ));
    }
    if row.candidate_version != expected_version {
        return Err(MemoryCandidateControlStoreError::InvalidData(
            "memory candidate version does not match the displayed version".into(),
        ));
    }
    let payload: MemoryPayload = serde_json::from_str(&row.payload_json).map_err(|error| {
        MemoryCandidateControlStoreError::InvalidData(format!(
            "stored candidate payload is invalid: {error}"
        ))
    })?;

    // 复验所有来源（LEFT JOIN：来源事件、会话或正文投影缺失本身即失效）：
    // 仍属本账号、未被撤回、会话/正文允许长期记忆、来源 Actor 与原始事件的
    // 权威 Actor 一致（事实身份与证据强绑定，不信任候选来源表的快照字段）。
    let source_rows = CandidateSourceRow::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        r#"
SELECT source.source_event_id, source.actor_platform_id,
       event.actor_platform_id AS event_actor_platform_id,
       source.content_trust_level, source.occurred_at_unix_secs,
       event.account_id, conversation.memory_mode, content.content_mode
FROM secretary_memory_candidate_sources source
LEFT JOIN secretary_source_events event ON event.source_event_id = source.source_event_id
LEFT JOIN secretary_conversations conversation ON conversation.id = event.conversation_id
LEFT JOIN secretary_message_contents content ON content.source_event_id = event.source_event_id
WHERE source.candidate_id = ? FOR UPDATE
"#,
        [candidate_id.as_str().into()],
    ))
    .all(db)
    .await
    .map_err(database_error)?;
    if source_rows.is_empty() {
        return Err(MemoryCandidateControlStoreError::InvalidData(
            "memory candidate has no verifiable sources".into(),
        ));
    }
    let mut sources = Vec::with_capacity(source_rows.len());
    for source in source_rows {
        // LEFT JOIN 下事件行缺失（理论上来源行会被级联删除，防御异常数据）。
        let event_actor = source.event_actor_platform_id.as_deref().ok_or_else(|| {
            MemoryCandidateControlStoreError::InvalidData(
                "candidate source event is missing".into(),
            )
        })?;
        if source.account_id != Some(account_id) {
            return Err(MemoryCandidateControlStoreError::Unauthorized);
        }
        if event_actor != source.actor_platform_id {
            return Err(MemoryCandidateControlStoreError::InvalidData(
                "candidate source actor does not match the authoritative source event actor".into(),
            ));
        }
        if source.memory_mode.as_deref() != Some("normal")
            || source.content_mode.as_deref() != Some("normal")
        {
            return Err(MemoryCandidateControlStoreError::InvalidData(
                "candidate source event, conversation, or content projection is missing \
                 or no longer permits long-term memory derivation"
                    .into(),
            ));
        }
        let withdrawn = TombstoneRow::find_by_statement(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            "SELECT 1 AS value FROM secretary_message_tombstones \
             WHERE source_event_id = ? AND account_id = ? AND status = 'applied' LIMIT 1",
            [source.source_event_id.clone().into(), account_id.into()],
        ))
        .one(db)
        .await
        .map_err(database_error)?;
        if withdrawn.is_some() {
            return Err(MemoryCandidateControlStoreError::InvalidData(
                "candidate source event was withdrawn".into(),
            ));
        }
        sources.push(MemoryCandidateSource {
            source_event_id: SourceEventId::new(source.source_event_id.clone()).map_err(
                |error| MemoryCandidateControlStoreError::InvalidData(error.to_string()),
            )?,
            actor: ThreadActorRef {
                account: request.account.clone(),
                actor_id: source.actor_platform_id,
                platform_identity_kind: None,
            },
            occurred_at_unix_secs: source.occurred_at_unix_secs,
            content_trust_level: parse_trust(&source.content_trust_level)?,
        });
    }

    // 用候选内容构造 Confirmed Fact（Commitment -> Pending）。
    let fact_id = MemoryFactId::new(stable_id("memory-candidate-approve", &request.effect_id))
        .map_err(|error| MemoryCandidateControlStoreError::InvalidData(error.to_string()))?;
    let candidate = MemoryCandidate {
        candidate_id: candidate_id.clone(),
        account: request.account.clone(),
        subject_key: row.subject_key.clone(),
        payload,
        status: MemoryCandidateStatus::Proposed,
        version: MemoryCandidateVersion::new(expected_version)
            .map_err(|error| MemoryCandidateControlStoreError::InvalidData(error.to_string()))?,
        extractor_version: String::new(),
        deterministic_fingerprint: String::new(),
        sources,
    };
    // payload -> Confirmed Fact 的领域转换（含 Commitment Pending 化与完整校验）。
    let fact = candidate_to_confirmed_fact(&candidate, fact_id)
        .map_err(|error| MemoryCandidateControlStoreError::InvalidData(error.to_string()))?;

    // 检查当前 active MemoryFact（同 kind + subject）。
    let existing = ActiveFactRow::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "SELECT fact_id, CAST(fact_json AS CHAR) AS fact_json \
         FROM secretary_memory_facts \
         WHERE account_id = ? AND fact_kind = ? AND subject_key = ? \
           AND fact_status IN ('proposed', 'confirmed') \
         ORDER BY fact_id LIMIT 1 FOR UPDATE",
        [
            account_id.into(),
            row.candidate_kind.clone().into(),
            row.subject_key.clone().into(),
        ],
    ))
    .one(db)
    .await
    .map_err(database_error)?;

    let outcome = match existing {
        None => {
            let fact_json = serde_json::to_string(&fact).map_err(|error| {
                MemoryCandidateControlStoreError::InvalidData(format!(
                    "cannot serialize memory fact: {error}"
                ))
            })?;
            db.execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::MySql,
                r#"
INSERT INTO secretary_memory_facts
    (fact_id, account_id, fact_kind, subject_key, fact_json, fact_status,
     confidence_bps, valid_until_unix_secs, supersedes_fact_id)
VALUES (?, ?, ?, ?, ?, 'confirmed', ?, NULL, NULL)
"#,
                [
                    fact.fact_id.as_str().into(),
                    account_id.into(),
                    row.candidate_kind.clone().into(),
                    row.subject_key.clone().into(),
                    fact_json.into(),
                    fact.confidence_bps.into(),
                ],
            ))
            .await
            .map_err(database_error)?;
            for source_event_id in &fact.source_event_ids {
                db.execute_raw(Statement::from_sql_and_values(
                    DatabaseBackend::MySql,
                    "INSERT INTO secretary_memory_fact_sources (fact_id, source_event_id) \
                     VALUES (?, ?)",
                    [
                        fact.fact_id.as_str().into(),
                        source_event_id.as_str().into(),
                    ],
                ))
                .await
                .map_err(database_error)?;
            }
            ControlOutcome::ApproveCreated {
                previous_version: row.candidate_version,
                fact_id: fact.fact_id.clone(),
            }
        }
        Some(existing) => {
            let existing_fact_id = MemoryFactId::new(existing.fact_id).map_err(|error| {
                MemoryCandidateControlStoreError::InvalidData(error.to_string())
            })?;
            let existing_fact: MemoryFact =
                serde_json::from_str(&existing.fact_json).map_err(|error| {
                    MemoryCandidateControlStoreError::InvalidData(format!(
                        "stored memory fact is invalid: {error}"
                    ))
                })?;
            if existing_fact.payload != fact.payload {
                // 内容不同的 active fact：保守拒绝本次提交，不自动 supersede。
                // 冲突是确定性业务结果而非运行失败：写入包含旧 Fact ID 与
                // Candidate ID 的审计与 Receipt，Owner 据此选择拒绝或保留，
                // 候选保持 proposed 且版本不变（result_ref 已含冲突说明）。
                return Ok(ControlOutcome::ApproveConflict {
                    previous_version: row.candidate_version,
                    fact_id: existing_fact_id,
                });
            }
            // 已存在完全相同的 active fact：候选 approved，引用既有事实，并把
            // 本候选的新来源合并进事实来源链，防止 Owner 查看记忆来源时
            // 丢失这次新证据（与 fact_json 同步，避免存储行与 JSON 漂移）。
            let mut merged_source_ids = existing_fact.source_event_ids.clone();
            for source_event_id in &fact.source_event_ids {
                if !merged_source_ids.contains(source_event_id) {
                    db.execute_raw(Statement::from_sql_and_values(
                        DatabaseBackend::MySql,
                        "INSERT IGNORE INTO secretary_memory_fact_sources (fact_id, source_event_id) \
                         VALUES (?, ?)",
                        [
                            existing_fact_id.as_str().into(),
                            source_event_id.as_str().into(),
                        ],
                    ))
                    .await
                    .map_err(database_error)?;
                    merged_source_ids.push(source_event_id.clone());
                }
            }
            if merged_source_ids != existing_fact.source_event_ids {
                let mut merged_fact = existing_fact.clone();
                merged_fact.source_event_ids = merged_source_ids;
                let merged_json = serde_json::to_string(&merged_fact).map_err(|error| {
                    MemoryCandidateControlStoreError::InvalidData(format!(
                        "cannot serialize merged memory fact: {error}"
                    ))
                })?;
                db.execute_raw(Statement::from_sql_and_values(
                    DatabaseBackend::MySql,
                    "UPDATE secretary_memory_facts SET fact_json = ? \
                     WHERE fact_id = ? AND account_id = ?",
                    [
                        merged_json.into(),
                        existing_fact_id.as_str().into(),
                        account_id.into(),
                    ],
                ))
                .await
                .map_err(database_error)?;
            }
            ControlOutcome::ApproveReferenced {
                previous_version: row.candidate_version,
                fact_id: existing_fact_id,
            }
        }
    };

    let updated = db
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            "UPDATE secretary_memory_candidates \
             SET candidate_status = 'approved', candidate_version = candidate_version + 1, \
                 updated_at = CURRENT_TIMESTAMP(6) \
             WHERE candidate_id = ? AND candidate_status = 'proposed' AND candidate_version = ?",
            [candidate_id.as_str().into(), expected_version.into()],
        ))
        .await
        .map_err(database_error)?;
    if updated.rows_affected() != 1 {
        return Err(MemoryCandidateControlStoreError::LeaseLost);
    }
    Ok(outcome)
}

/// 拒绝候选：proposed -> rejected（版本精确 +1）；不创建任何业务行。
async fn apply_reject(
    db: &sea_orm::DatabaseTransaction,
    account_id: u64,
    candidate_id: &MemoryCandidateId,
    expected_version: u64,
) -> Result<ControlOutcome, MemoryCandidateControlStoreError> {
    let row = RejectRow::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "SELECT account_id, candidate_status, candidate_version \
         FROM secretary_memory_candidates WHERE candidate_id = ? FOR UPDATE",
        [candidate_id.as_str().into()],
    ))
    .one(db)
    .await
    .map_err(database_error)?
    .ok_or_else(|| {
        MemoryCandidateControlStoreError::InvalidData("memory candidate was not found".into())
    })?;
    if row.account_id != account_id {
        return Err(MemoryCandidateControlStoreError::Unauthorized);
    }
    if row.candidate_status != "proposed" {
        return Err(MemoryCandidateControlStoreError::InvalidData(
            "memory candidate is no longer proposed".into(),
        ));
    }
    if row.candidate_version != expected_version {
        return Err(MemoryCandidateControlStoreError::InvalidData(
            "memory candidate version does not match the displayed version".into(),
        ));
    }
    let updated = db
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            "UPDATE secretary_memory_candidates \
             SET candidate_status = 'rejected', candidate_version = candidate_version + 1, \
                 updated_at = CURRENT_TIMESTAMP(6) \
             WHERE candidate_id = ? AND candidate_status = 'proposed' AND candidate_version = ?",
            [candidate_id.as_str().into(), expected_version.into()],
        ))
        .await
        .map_err(database_error)?;
    if updated.rows_affected() != 1 {
        return Err(MemoryCandidateControlStoreError::LeaseLost);
    }
    Ok(ControlOutcome::Rejected {
        previous_version: row.candidate_version,
    })
}

/// 每条批准/拒绝写一行不可变审计（control_id 由 effect_id 稳定派生，重放不新增）。
async fn insert_control_audit(
    db: &sea_orm::DatabaseTransaction,
    request: &MemoryCandidateControlEffectRequest,
    account_id: u64,
    control_id: &str,
    candidate_id: &MemoryCandidateId,
    reason: &str,
    outcome: &ControlOutcome,
) -> Result<(), MemoryCandidateControlStoreError> {
    let (kind, previous, current, previous_version, current_version, fact_id) =
        outcome.audit_fields();
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        r#"
INSERT INTO secretary_memory_candidate_controls
    (control_id, effect_id, run_id, proposal_id, account_id, candidate_id, control_kind,
     previous_status, current_status, previous_candidate_version, current_candidate_version,
     fact_id, command_source_event_id, reason)
VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
"#,
        [
            control_id.into(),
            request.effect_id.clone().into(),
            request.run_id.as_str().into(),
            request.proposal_id.clone().into(),
            account_id.into(),
            candidate_id.as_str().into(),
            kind.into(),
            previous.into(),
            current.into(),
            previous_version.into(),
            current_version.into(),
            fact_id.map(|id| id.as_str().to_owned()).into(),
            request.command_source_event_id.as_str().into(),
            reason.to_owned().into(),
        ],
    ))
    .await
    .map_err(database_error)?;
    Ok(())
}

fn parse_trust(value: &str) -> Result<ContentTrustLevel, MemoryCandidateControlStoreError> {
    match value {
        "normal" => Ok(ContentTrustLevel::Normal),
        "local_only" => Ok(ContentTrustLevel::LocalOnly),
        "envelope_only" => Ok(ContentTrustLevel::EnvelopeOnly),
        "never_long_term" => Ok(ContentTrustLevel::NeverLongTerm),
        other => Err(MemoryCandidateControlStoreError::InvalidData(format!(
            "unknown content trust level {other}"
        ))),
    }
}

#[derive(Debug, FromQueryResult)]
struct CandidateRow {
    candidate_status: String,
    candidate_version: u64,
    candidate_kind: String,
    subject_key: String,
    payload_json: String,
}

#[derive(Debug, FromQueryResult)]
struct RejectRow {
    account_id: u64,
    candidate_status: String,
    candidate_version: u64,
}

#[derive(Debug, FromQueryResult)]
struct CandidateSourceRow {
    source_event_id: String,
    actor_platform_id: String,
    /// 原始 SourceEvent 的权威 Actor（LEFT JOIN，事件缺失时为 None）。
    event_actor_platform_id: Option<String>,
    content_trust_level: String,
    occurred_at_unix_secs: i64,
    /// LEFT JOIN 后事件缺失时为 None。
    account_id: Option<u64>,
    /// LEFT JOIN 后会话/正文缺失时为 None。
    memory_mode: Option<String>,
    content_mode: Option<String>,
}

#[derive(Debug, FromQueryResult)]
struct ActiveFactRow {
    fact_id: String,
    fact_json: String,
}

#[derive(Debug, FromQueryResult)]
struct TombstoneRow {
    // `SELECT 1 AS value` 在 MySQL 中返回 BIGINT；行模型必须匹配列类型。
    #[allow(dead_code)]
    value: i64,
}
