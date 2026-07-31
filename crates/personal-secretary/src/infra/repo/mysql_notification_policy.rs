use async_trait::async_trait;
use sea_orm::{
    ConnectionTrait, DatabaseBackend, DatabaseConnection, DbErr, FromQueryResult, RuntimeErr,
    Statement, TransactionTrait,
};
use sha2::{Digest, Sha256};

use crate::{
    ClaimedEvaluation, DecisionReason, EvaluationCommit, EvaluationCommitResult,
    EvaluationSnapshot, FamilyGenerationSnapshot, MAX_EVALUATION_POLICY_FAMILIES, MessageSource,
    NotificationFeedbackRequest, NotificationOutcome, NotificationPolicyDisableRequest,
    NotificationPolicyFamily, NotificationPolicyKind, NotificationPolicyResponseArtifact,
    NotificationPolicyRevision, NotificationPolicyRule, NotificationPolicyStoreError,
    NotificationPolicyStoreT, NotificationPolicyWriteRequest, OwnerBindingSnapshot, PolicyFamilyId,
    PolicyRevisionId, PolicyRuleSnapshot, RevisionKind, SecretaryActionReceipt, SourceAccountRef,
};

pub(crate) struct MySqlNotificationPolicyStore {
    db: DatabaseConnection,
}

impl MySqlNotificationPolicyStore {
    pub(crate) fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }
}

#[async_trait]
impl NotificationPolicyStoreT for MySqlNotificationPolicyStore {
    async fn authorization_for_owner_command(
        &self,
        target_account: &SourceAccountRef,
        command_source_event_id: &crate::SourceEventId,
    ) -> Result<crate::NotificationPolicyAuthorizationContext, NotificationPolicyStoreError> {
        #[derive(FromQueryResult)]
        struct AuthorizationRow {
            owner_binding_source_channel: String,
            owner_binding_platform_account_id: String,
            command_source_channel: String,
            command_platform_account_id: String,
            owner_actor_id: String,
            command_actor_id: String,
            command_role: String,
            target_source_channel: String,
            target_platform_account_id: String,
        }
        let rows = AuthorizationRow::find_by_statement(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            r#"SELECT managed.source_channel AS owner_binding_source_channel,
                       managed.platform_account_id AS owner_binding_platform_account_id,
                       command_account.source_channel AS command_source_channel,
                       command_account.platform_account_id AS command_platform_account_id,
                       binding.owner_actor_id,
                       command.actor_platform_id AS command_actor_id,
                       command.message_role AS command_role,
                       target.source_channel AS target_source_channel,
                       target.platform_account_id AS target_platform_account_id
                FROM secretary_source_events AS command
                INNER JOIN secretary_accounts AS command_account ON command_account.id = command.account_id
                INNER JOIN secretary_owner_bindings AS binding
                  ON binding.command_account_id = command.account_id
                 AND binding.owner_actor_id = command.actor_platform_id
                 AND binding.status = 'active'
                INNER JOIN secretary_accounts AS managed ON managed.id = binding.managed_account_id
                INNER JOIN secretary_accounts AS target
                  ON target.source_channel = ? AND target.platform_account_id = ?
                WHERE command.source_event_id = ?
                  AND managed.id = target.id
                LIMIT 2"#,
            [
                target_account.channel.as_str().into(),
                target_account.account_id.clone().into(),
                command_source_event_id.as_str().into(),
            ],
        ))
        .all(&self.db)
        .await
        .map_err(database_error)?;
        let row = match rows.as_slice() {
            [] => return Err(NotificationPolicyStoreError::Unauthorized),
            [row] => row,
            // 授权不可依赖查询顺序；重复的精确 active binding 一律拒绝。
            _ => return Err(NotificationPolicyStoreError::Conflict),
        };
        Ok(crate::NotificationPolicyAuthorizationContext {
            owner_binding_account: source_account_from_db(
                &row.owner_binding_source_channel,
                row.owner_binding_platform_account_id.clone(),
            )?,
            owner_actor_id: row.owner_actor_id.clone(),
            command_account: source_account_from_db(
                &row.command_source_channel,
                row.command_platform_account_id.clone(),
            )?,
            command_actor_id: row.command_actor_id.clone(),
            command_role: match row.command_role.as_str() {
                "owner_command" => crate::MessageRole::OwnerCommand,
                "external_observation" => crate::MessageRole::ExternalObservation,
                _ => return Err(NotificationPolicyStoreError::Database),
            },
            target_account: source_account_from_db(
                &row.target_source_channel,
                row.target_platform_account_id.clone(),
            )?,
        })
    }

    async fn create_or_replace(
        &self,
        request: &NotificationPolicyWriteRequest,
    ) -> Result<NotificationPolicyFamily, NotificationPolicyStoreError> {
        request
            .rule
            .match_key
            .eligibility_for_long_term_rule()
            .map_err(policy_error)?;
        validate_write_request(request)?;

        let transaction = self.db.begin().await.map_err(database_error)?;
        let account_id = lock_account(&transaction, &request.account).await?;
        if let Some(existing) = find_command_revision(
            &transaction,
            account_id,
            request.command_source_event_id.as_deref(),
        )
        .await?
        {
            transaction.commit().await.map_err(database_error)?;
            return to_domain_family(existing, request.account.clone());
        }
        let family = match &request.policy_family_id {
            Some(family_id) => load_family_for_update(&transaction, account_id, family_id).await?,
            None => match find_family_by_scope_for_update(
                &transaction,
                account_id,
                &request.canonical_scope_key,
                request.policy_kind,
            )
            .await?
            {
                Some(family) => family,
                None => create_family(&transaction, account_id, request).await?,
            },
        };
        let revision = append_revision(
            &transaction,
            &family,
            RevisionKind::Rule,
            Some(&request.rule),
            request.command_source_event_id.as_deref(),
            &request.audit_summary,
        )
        .await?;
        let updated = update_family_head(
            &transaction,
            &family.policy_family_id,
            family.generation,
            &revision.policy_revision_id,
        )
        .await?;
        increment_policy_epoch(&transaction, account_id).await?;
        transaction.commit().await.map_err(database_error)?;
        to_domain_family(updated, request.account.clone())
    }

    async fn disable(
        &self,
        request: &NotificationPolicyDisableRequest,
    ) -> Result<NotificationPolicyFamily, NotificationPolicyStoreError> {
        if request.audit_summary.trim().is_empty()
            || request.audit_summary.len() > crate::MAX_NOTIFICATION_AUDIT_SUMMARY_BYTES
        {
            return Err(NotificationPolicyStoreError::Conflict);
        }
        let transaction = self.db.begin().await.map_err(database_error)?;
        let account_id = lock_account(&transaction, &request.account).await?;
        if let Some(existing) = find_command_revision(
            &transaction,
            account_id,
            request.command_source_event_id.as_deref(),
        )
        .await?
        {
            transaction.commit().await.map_err(database_error)?;
            return to_domain_family(existing, request.account.clone());
        }
        let family =
            load_family_for_update(&transaction, account_id, &request.policy_family_id).await?;
        if family.generation != request.expected_generation {
            return Err(NotificationPolicyStoreError::Conflict);
        }
        let revision = append_revision(
            &transaction,
            &family,
            RevisionKind::Tombstone,
            None,
            request.command_source_event_id.as_deref(),
            &request.audit_summary,
        )
        .await?;
        let updated = update_family_head(
            &transaction,
            &family.policy_family_id,
            family.generation,
            &revision.policy_revision_id,
        )
        .await?;
        increment_policy_epoch(&transaction, account_id).await?;
        transaction.commit().await.map_err(database_error)?;
        to_domain_family(updated, request.account.clone())
    }

    async fn record_feedback(
        &self,
        request: &NotificationFeedbackRequest,
    ) -> Result<(), NotificationPolicyStoreError> {
        if request.promote_to_rule {
            request
                .match_key
                .eligibility_for_long_term_rule()
                .map_err(policy_error)?;
        }
        let transaction = self.db.begin().await.map_err(database_error)?;
        let account_id = lock_account(&transaction, &request.candidate.account).await?;
        let candidate_id = candidate_id(&transaction, account_id, &request.candidate).await?;
        let feedback_id = feedback_id(request);
        let result = transaction
            .execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::MySql,
                "INSERT IGNORE INTO secretary_notification_feedback \
                 (feedback_id, account_id, notification_candidate_id, important, promote_to_rule, \
                  command_source_event_id, audit_summary) VALUES (?, ?, ?, ?, ?, ?, ?)",
                [
                    feedback_id.into(),
                    account_id.into(),
                    candidate_id.into(),
                    request.important.into(),
                    request.promote_to_rule.into(),
                    request.command_source_event_id.clone().into(),
                    "owner notification feedback".into(),
                ],
            ))
            .await
            .map_err(database_error)?;
        if result.rows_affected() > 1 {
            return Err(NotificationPolicyStoreError::Database);
        }
        if result.rows_affected() == 1 && request.promote_to_rule {
            promote_feedback_to_rule(&transaction, account_id, request).await?;
        }
        transaction.commit().await.map_err(database_error)?;
        Ok(())
    }

    async fn apply_effect(
        &self,
        request: &crate::NotificationPolicyEffectRequest,
    ) -> Result<SecretaryActionReceipt, NotificationPolicyStoreError> {
        let transaction = self.db.begin().await.map_err(database_error)?;
        if let Some(receipt) = load_policy_effect_receipt(&transaction, request).await? {
            transaction.commit().await.map_err(database_error)?;
            return Ok(receipt);
        }
        let account_id = lock_account(&transaction, &request.account).await?;
        verify_action_lease(&transaction, request, account_id).await?;
        verify_effect_owner_authorization(&transaction, request, account_id).await?;
        if let Some(receipt) = load_policy_effect_receipt(&transaction, request).await? {
            transaction.commit().await.map_err(database_error)?;
            return Ok(receipt);
        }

        let result_ref = apply_policy_effect_action(&transaction, request, account_id).await?;
        let inserted = transaction
            .execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::MySql,
                "INSERT IGNORE INTO secretary_action_effect_receipts \
                 (effect_id, run_id, proposal_json, result_ref) VALUES (?, ?, ?, ?)",
                [
                    request.effect_id.clone().into(),
                    request.run_id.clone().into(),
                    request.proposal_json.clone().into(),
                    result_ref.clone().into(),
                ],
            ))
            .await
            .map_err(database_error)?;
        if inserted.rows_affected() == 0 {
            let receipt = load_policy_effect_receipt(&transaction, request)
                .await?
                .ok_or(NotificationPolicyStoreError::Database)?;
            transaction.commit().await.map_err(database_error)?;
            return Ok(receipt);
        }
        if inserted.rows_affected() != 1 {
            return Err(NotificationPolicyStoreError::Database);
        }
        transaction.commit().await.map_err(database_error)?;
        Ok(SecretaryActionReceipt {
            proposal_id: request.proposal_id.clone(),
            result_ref,
            tool_kind: Some(request.action.kind()),
        })
    }

    async fn list_policy_artifacts(
        &self,
        account: &SourceAccountRef,
        limit: u16,
    ) -> Result<Vec<NotificationPolicyResponseArtifact>, NotificationPolicyStoreError> {
        if !(1..=20).contains(&limit) {
            return Err(NotificationPolicyStoreError::Conflict);
        }
        let account_id = load_account_id(&self.db, account).await?;
        #[derive(FromQueryResult)]
        struct PolicyArtifactRow {
            canonical_scope_key: String,
            policy_family_id: String,
            current_revision_id: String,
            generation: u64,
            policy_kind: String,
            revision_kind: String,
        }
        let rows = PolicyArtifactRow::find_by_statement(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            "SELECT family.canonical_scope_key, family.policy_family_id, family.current_revision_id, \
                    family.generation, family.policy_kind, revision.revision_kind \
             FROM secretary_notification_policy_families AS family \
             INNER JOIN secretary_notification_policy_revisions AS revision \
               ON revision.policy_family_id = family.policy_family_id \
              AND revision.policy_revision_id = family.current_revision_id \
             WHERE family.account_id = ? \
             ORDER BY family.policy_kind, family.canonical_scope_key, family.policy_family_id \
             LIMIT ?",
            [account_id.into(), u64::from(limit).into()],
        ))
        .all(&self.db)
        .await
        .map_err(database_error)?;
        rows.into_iter()
            .map(|row| {
                let priority = policy_kind_name(parse_policy_kind(&row.policy_kind)?).to_owned();
                Ok(NotificationPolicyResponseArtifact {
                    scope: row.canonical_scope_key,
                    policy_family_id: Some(
                        PolicyFamilyId::new(row.policy_family_id).map_err(policy_error)?,
                    ),
                    policy_revision_id: Some(
                        PolicyRevisionId::new(row.current_revision_id).map_err(policy_error)?,
                    ),
                    decision_id: None,
                    status: row.revision_kind,
                    priority,
                    typed_reason: format!("generation:{}", row.generation),
                    audit_reference: "policy_family_head".into(),
                })
            })
            .collect()
    }

    async fn explain_decision_artifact(
        &self,
        account: &SourceAccountRef,
        decision_id: &crate::NotificationDecisionId,
    ) -> Result<Option<NotificationPolicyResponseArtifact>, NotificationPolicyStoreError> {
        let account_id = load_account_id(&self.db, account).await?;
        #[derive(FromQueryResult)]
        struct DecisionArtifactRow {
            notification_decision_id: String,
            policy_family_id: Option<String>,
            policy_revision_id: Option<String>,
            outcome: String,
            reason_code: String,
            audit_reference: String,
        }
        let row = DecisionArtifactRow::find_by_statement(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            "SELECT decision.notification_decision_id, revision.policy_family_id, \
                    decision.policy_revision_id, decision.outcome, decision.reason_code, \
                    decision.evaluation_request_id AS audit_reference \
             FROM secretary_notification_decisions AS decision \
             INNER JOIN secretary_notification_candidates AS candidate \
               ON candidate.notification_candidate_id = decision.notification_candidate_id \
             LEFT JOIN secretary_notification_policy_revisions AS revision \
               ON revision.policy_revision_id = decision.policy_revision_id \
             WHERE decision.notification_decision_id = ? AND candidate.account_id = ?",
            [decision_id.as_str().into(), account_id.into()],
        ))
        .one(&self.db)
        .await
        .map_err(database_error)?;
        row.map(|row| {
            Ok(NotificationPolicyResponseArtifact {
                scope: "notification_decision".into(),
                policy_family_id: row
                    .policy_family_id
                    .map(PolicyFamilyId::new)
                    .transpose()
                    .map_err(policy_error)?,
                policy_revision_id: row
                    .policy_revision_id
                    .map(PolicyRevisionId::new)
                    .transpose()
                    .map_err(policy_error)?,
                decision_id: Some(
                    crate::NotificationDecisionId::new(row.notification_decision_id)
                        .map_err(policy_error)?,
                ),
                status: row.outcome,
                priority: "decision".into(),
                typed_reason: row.reason_code,
                audit_reference: row.audit_reference,
            })
        })
        .transpose()
    }

    async fn claim_evaluation(
        &self,
        worker_id: &str,
        now_unix_secs: i64,
        lease_secs: u64,
    ) -> Result<Option<ClaimedEvaluation>, NotificationPolicyStoreError> {
        if worker_id.trim().is_empty() || lease_secs == 0 {
            return Err(NotificationPolicyStoreError::Conflict);
        }
        let lease_expires_at_unix_secs = now_unix_secs
            .checked_add(
                i64::try_from(lease_secs).map_err(|_| NotificationPolicyStoreError::Conflict)?,
            )
            .ok_or(NotificationPolicyStoreError::Conflict)?;
        let transaction = self.db.begin().await.map_err(database_error)?;
        let Some(row) = EvaluationRequestRow::find_by_statement(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            "SELECT request.evaluation_request_id, request.attempt \
             FROM secretary_notification_evaluation_requests AS request \
             WHERE ((request.request_status = 'pending' \
                     AND (request.next_allowed_at_unix_secs IS NULL \
                          OR request.next_allowed_at_unix_secs <= ?)) \
                    OR (request.request_status = 'claimed' \
                        AND request.lease_expires_at_unix_secs < ?)) \
             ORDER BY request.created_at, request.evaluation_request_id \
             LIMIT 1 FOR UPDATE SKIP LOCKED",
            [now_unix_secs.into(), now_unix_secs.into()],
        ))
        .one(&transaction)
        .await
        .map_err(database_error)?
        else {
            transaction.commit().await.map_err(database_error)?;
            return Ok(None);
        };
        let lease_token = uuid::Uuid::new_v4().to_string();
        let updated = transaction
            .execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::MySql,
                "UPDATE secretary_notification_evaluation_requests \
             SET request_status = 'claimed', lease_token = ?, lease_expires_at_unix_secs = ?, \
                 attempt = attempt + 1 \
             WHERE evaluation_request_id = ? AND attempt = ?",
                [
                    lease_token.clone().into(),
                    lease_expires_at_unix_secs.into(),
                    row.evaluation_request_id.clone().into(),
                    row.attempt.into(),
                ],
            ))
            .await
            .map_err(database_error)?;
        if updated.rows_affected() != 1 {
            return Err(NotificationPolicyStoreError::Conflict);
        }
        transaction.commit().await.map_err(database_error)?;
        Ok(Some(ClaimedEvaluation {
            evaluation_request_id: crate::EvaluationRequestId::new(row.evaluation_request_id)
                .map_err(policy_error)?,
            lease_token,
            lease_expires_at_unix_secs,
            attempt: row.attempt + 1,
        }))
    }

    async fn load_evaluation_snapshot(
        &self,
        claim: &ClaimedEvaluation,
    ) -> Result<EvaluationSnapshot, NotificationPolicyStoreError> {
        let rows = EvaluationSnapshotRow::find_by_statement(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            "SELECT request.evaluation_request_id, candidate.account_id AS managed_account_id, \
                    candidate.source_kind, candidate.source_id, candidate.source_version, \
                    candidate.candidate_status, CAST(candidate.match_key_json AS CHAR) AS match_key_json, \
                    account.policy_epoch, account.source_channel AS managed_source_channel, \
                    account.platform_account_id AS managed_platform_account_id, \
                    command_account.source_channel AS command_source_channel, \
                    command_account.platform_account_id AS command_platform_account_id, \
                    binding.owner_actor_id \
             FROM secretary_notification_evaluation_requests AS request \
             INNER JOIN secretary_notification_candidates AS candidate \
               ON candidate.notification_candidate_id = request.notification_candidate_id \
             INNER JOIN secretary_accounts AS account ON account.id = candidate.account_id \
             INNER JOIN secretary_owner_bindings AS binding \
               ON binding.managed_account_id = account.id AND binding.status = 'active' \
             INNER JOIN secretary_accounts AS command_account ON command_account.id = binding.command_account_id \
             WHERE request.evaluation_request_id = ? AND request.request_status = 'claimed' \
               AND request.lease_token = ? AND request.lease_expires_at_unix_secs >= UNIX_TIMESTAMP()",
            [
                claim.evaluation_request_id.as_str().into(),
                claim.lease_token.clone().into(),
            ],
        ))
        .all(&self.db)
        .await
        .map_err(database_error)?;
        let row = match rows.as_slice() {
            [row] => row,
            [] => return Err(NotificationPolicyStoreError::Conflict),
            // 一个候选只能由唯一 active Owner binding 快照解释。
            _ => return Err(NotificationPolicyStoreError::Conflict),
        };
        let managed_account = source_account_from_db(
            &row.managed_source_channel,
            row.managed_platform_account_id.clone(),
        )?;
        let command_account = source_account_from_db(
            &row.command_source_channel,
            row.command_platform_account_id.clone(),
        )?;
        let candidate_match_key: crate::NotificationMatchKeyV1 =
            serde_json::from_str(&row.match_key_json)
                .map_err(|_| NotificationPolicyStoreError::Database)?;
        if candidate_match_key.account != managed_account {
            return Err(NotificationPolicyStoreError::Database);
        }
        let heads = PolicyHeadRow::find_by_statement(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            "SELECT family.policy_family_id, family.current_revision_id, family.generation, \
                    family.policy_kind, revision.revision_kind, \
                    CAST(revision.rule_json AS CHAR) AS rule_json \
             FROM secretary_notification_policy_families AS family \
             INNER JOIN secretary_notification_policy_revisions AS revision \
               ON revision.policy_family_id = family.policy_family_id \
              AND revision.policy_revision_id = family.current_revision_id \
             WHERE family.account_id = ? \
             ORDER BY family.policy_kind, family.canonical_scope_key, family.policy_family_id \
             LIMIT ?",
            [
                row.managed_account_id.into(),
                (MAX_EVALUATION_POLICY_FAMILIES as u64 + 1).into(),
            ],
        ))
        .all(&self.db)
        .await
        .map_err(database_error)?;
        if heads.len() > MAX_EVALUATION_POLICY_FAMILIES {
            return Err(NotificationPolicyStoreError::Conflict);
        }
        let mut family_generations = Vec::with_capacity(heads.len());
        let mut active_rules = Vec::new();
        for head in heads {
            let policy_family_id =
                PolicyFamilyId::new(head.policy_family_id).map_err(policy_error)?;
            let policy_revision_id =
                PolicyRevisionId::new(head.current_revision_id).map_err(policy_error)?;
            family_generations.push(FamilyGenerationSnapshot {
                policy_family_id: policy_family_id.clone(),
                current_revision_id: policy_revision_id.clone(),
                generation: head.generation,
            });
            let policy_kind = parse_policy_kind(&head.policy_kind)?;
            match head.revision_kind.as_str() {
                "tombstone" => {}
                "rule" => {
                    let rule_json = head
                        .rule_json
                        .ok_or(NotificationPolicyStoreError::Database)?;
                    let rule: NotificationPolicyRule = serde_json::from_str(&rule_json)
                        .map_err(|_| NotificationPolicyStoreError::Database)?;
                    if rule.match_key.account != managed_account {
                        return Err(NotificationPolicyStoreError::Database);
                    }
                    active_rules.push(PolicyRuleSnapshot {
                        policy_family_id,
                        policy_revision_id,
                        generation: head.generation,
                        policy_kind,
                        rule,
                    });
                }
                _ => return Err(NotificationPolicyStoreError::Database),
            }
        }
        Ok(EvaluationSnapshot {
            evaluation_request_id: crate::EvaluationRequestId::new(
                row.evaluation_request_id.clone(),
            )
            .map_err(policy_error)?,
            candidate: crate::NotificationCandidateRef::new(
                row.source_kind.clone(),
                row.source_id.clone(),
                row.source_version,
                managed_account.clone(),
            )
            .map_err(policy_error)?,
            candidate_match_key,
            candidate_is_current: matches!(row.candidate_status.as_str(), "pending" | "delayed"),
            account_policy_epoch: row.policy_epoch,
            family_generations,
            active_rules,
            owner_binding: OwnerBindingSnapshot {
                managed_account,
                command_account,
                owner_actor_id: row.owner_actor_id.clone(),
            },
        })
    }

    async fn commit_evaluation(
        &self,
        commit: &EvaluationCommit,
    ) -> Result<EvaluationCommitResult, NotificationPolicyStoreError> {
        let transaction = self.db.begin().await.map_err(database_error)?;
        let Some(request) = EvaluationRequestCommitRow::find_by_statement(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            "SELECT request.evaluation_request_id, request.notification_candidate_id, \
                    candidate.account_id, candidate.source_kind, candidate.source_id, \
                    candidate.source_version, candidate.candidate_status, account.policy_epoch, \
                    (SELECT decision.notification_decision_id \
                     FROM secretary_notification_decisions AS decision \
                     WHERE decision.evaluation_request_id = request.evaluation_request_id \
                     ORDER BY decision.created_at DESC, decision.notification_decision_id DESC \
                     LIMIT 1) AS previous_decision_id \
             FROM secretary_notification_evaluation_requests AS request \
             INNER JOIN secretary_notification_candidates AS candidate \
               ON candidate.notification_candidate_id = request.notification_candidate_id \
             INNER JOIN secretary_accounts AS account ON account.id = candidate.account_id \
             WHERE request.evaluation_request_id = ? AND request.request_status = 'claimed' \
               AND request.lease_token = ? AND request.lease_expires_at_unix_secs >= UNIX_TIMESTAMP() \
             FOR UPDATE",
            [
                commit.claim.evaluation_request_id.as_str().into(),
                commit.claim.lease_token.clone().into(),
            ],
        ))
        .one(&transaction)
        .await
        .map_err(database_error)? else {
            transaction.commit().await.map_err(database_error)?;
            return Ok(EvaluationCommitResult::LeaseLost);
        };
        let stale = request.source_kind != commit.snapshot.candidate.source_kind
            || request.source_id != commit.snapshot.candidate.source_id
            || request.source_version != commit.snapshot.candidate.source_version
            || request.policy_epoch != commit.snapshot.account_policy_epoch
            || !matches!(request.candidate_status.as_str(), "pending" | "delayed")
            || commit.snapshot.owner_binding.managed_account != commit.snapshot.candidate.account;
        if stale
            || !source_is_current(
                &transaction,
                request.account_id,
                &request.source_kind,
                &request.source_id,
                request.source_version,
            )
            .await?
        {
            requeue_stale_request(
                &transaction,
                &request.evaluation_request_id,
                &commit.claim.lease_token,
            )
            .await?;
            transaction.commit().await.map_err(database_error)?;
            return Ok(EvaluationCommitResult::SnapshotStale);
        }
        let command_account_id = load_locked_binding_command_account(
            &transaction,
            request.account_id,
            &commit.snapshot.owner_binding.command_account,
            &commit.snapshot.owner_binding.owner_actor_id,
        )
        .await?;
        if command_account_id.is_none() {
            requeue_stale_request(
                &transaction,
                &request.evaluation_request_id,
                &commit.claim.lease_token,
            )
            .await?;
            transaction.commit().await.map_err(database_error)?;
            return Ok(EvaluationCommitResult::SnapshotStale);
        }
        for family in &commit.snapshot.family_generations {
            let head = PolicyHeadFenceRow::find_by_statement(Statement::from_sql_and_values(
                DatabaseBackend::MySql,
                "SELECT policy_family_id, current_revision_id, generation \
                 FROM secretary_notification_policy_families \
                 WHERE policy_family_id = ? AND account_id = ? FOR UPDATE",
                [
                    family.policy_family_id.as_str().into(),
                    request.account_id.into(),
                ],
            ))
            .one(&transaction)
            .await
            .map_err(database_error)?;
            if !matches!(head, Some(row) if row.generation == family.generation
                && row.current_revision_id == family.current_revision_id.as_str())
            {
                requeue_stale_request(
                    &transaction,
                    &request.evaluation_request_id,
                    &commit.claim.lease_token,
                )
                .await?;
                transaction.commit().await.map_err(database_error)?;
                return Ok(EvaluationCommitResult::SnapshotStale);
            }
        }
        let winning_revision_id = winning_revision_id(&commit.snapshot, &commit.plan)?;
        let next_allowed_at = match commit.plan.outcome {
            NotificationOutcome::Delay => commit
                .plan
                .next_allowed_at_unix_secs
                .filter(|value| *value > 0)
                .ok_or(NotificationPolicyStoreError::Conflict)?,
            _ => 0,
        };
        let decision_id = uuid::Uuid::new_v4().to_string();
        transaction
            .execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::MySql,
                "INSERT INTO secretary_notification_decisions \
                 (notification_decision_id, evaluation_request_id, notification_candidate_id, \
                  previous_decision_id, policy_revision_id, evaluator_version, outcome, reason_code, \
                  next_allowed_at_unix_secs) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
                [
                    decision_id.clone().into(),
                    request.evaluation_request_id.clone().into(),
                    request.notification_candidate_id.clone().into(),
                    request.previous_decision_id.clone().into(),
                    winning_revision_id.map(|id| id.as_str().to_owned()).into(),
                    "notification-policy-v1".into(),
                    outcome_name(commit.plan.outcome).into(),
                    decision_reason_name(commit.plan.reason).into(),
                    if commit.plan.outcome == NotificationOutcome::Delay {
                        next_allowed_at.into()
                    } else {
                        sea_orm::Value::BigInt(None)
                    },
                ],
            ))
            .await
            .map_err(database_error)?;
        match commit.plan.outcome {
            NotificationOutcome::Remind => {
                let command_account_id =
                    command_account_id.ok_or(NotificationPolicyStoreError::Database)?;
                let occurrence_id = notification_occurrence_id(
                    &request.source_kind,
                    &request.source_id,
                    request.source_version,
                    "owner_policy_reminder",
                );
                let payload = serde_json::json!({
                    "candidate_id": request.notification_candidate_id,
                    "source_version": request.source_version,
                    "owner_actor_id": commit.snapshot.owner_binding.owner_actor_id,
                });
                let outbox_insert = transaction
                    .execute_raw(Statement::from_sql_and_values(
                        DatabaseBackend::MySql,
                        "INSERT IGNORE INTO secretary_notification_outbox \
                         (notification_id, account_id, command_account_id, owner_actor_id, \
                          scheduled_at_unix_secs, notification_kind, payload_json, \
                          notification_candidate_id, notification_decision_id, occurrence_id) \
                         VALUES (?, ?, ?, ?, UNIX_TIMESTAMP(), 'owner_policy_reminder', CAST(? AS JSON), ?, ?, ?)",
                        [
                            uuid::Uuid::new_v4().to_string().into(),
                            request.account_id.into(),
                            command_account_id.into(),
                            commit.snapshot.owner_binding.owner_actor_id.clone().into(),
                            serde_json::to_string(&payload).map_err(|_| NotificationPolicyStoreError::Database)?.into(),
                            request.notification_candidate_id.clone().into(),
                            decision_id.into(),
                            occurrence_id.clone().into(),
                        ],
                    ))
                    .await
                    .map_err(database_error)?;
                match outbox_insert.rows_affected() {
                    1 => {}
                    // INSERT IGNORE 会把约束错误降为 warning；只有确认同一 occurrence
                    // 已实际存在时，0 才能视为幂等重放，避免静默吞掉 Decision 后的写入失败。
                    0 => {
                        let existing = transaction
                            .query_one_raw(Statement::from_sql_and_values(
                                DatabaseBackend::MySql,
                                "SELECT notification_id FROM secretary_notification_outbox \
                                 WHERE occurrence_id = ? FOR UPDATE",
                                [occurrence_id.into()],
                            ))
                            .await
                            .map_err(database_error)?;
                        if existing.is_none() {
                            return Err(NotificationPolicyStoreError::Database);
                        }
                    }
                    _ => return Err(NotificationPolicyStoreError::Database),
                }
                complete_request_and_candidate(
                    &transaction,
                    &request,
                    "completed",
                    "reminded",
                    None,
                )
                .await?;
            }
            NotificationOutcome::Delay => {
                complete_request_and_candidate(
                    &transaction,
                    &request,
                    "pending",
                    "delayed",
                    Some(next_allowed_at),
                )
                .await?;
            }
            outcome => {
                let candidate_status = candidate_status_name(outcome);
                complete_request_and_candidate(
                    &transaction,
                    &request,
                    "terminal",
                    candidate_status,
                    None,
                )
                .await?;
            }
        }
        transaction.commit().await.map_err(database_error)?;
        Ok(EvaluationCommitResult::Applied)
    }

    async fn recover_expired_evaluations(
        &self,
        now_unix_secs: i64,
        limit: u32,
    ) -> Result<u64, NotificationPolicyStoreError> {
        if limit == 0 {
            return Ok(0);
        }
        let transaction = self.db.begin().await.map_err(database_error)?;
        let requests = EvaluationRequestRow::find_by_statement(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            "SELECT evaluation_request_id, attempt FROM secretary_notification_evaluation_requests \
             WHERE request_status = 'claimed' AND lease_expires_at_unix_secs < ? \
             ORDER BY lease_expires_at_unix_secs, evaluation_request_id LIMIT ? FOR UPDATE SKIP LOCKED",
            [now_unix_secs.into(), limit.into()],
        )).all(&transaction).await.map_err(database_error)?;
        let mut recovered = 0;
        for request in requests {
            let result = transaction.execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::MySql,
                "UPDATE secretary_notification_evaluation_requests \
                 SET request_status = 'pending', lease_token = NULL, lease_expires_at_unix_secs = NULL \
                 WHERE evaluation_request_id = ? AND request_status = 'claimed' \
                   AND lease_expires_at_unix_secs < ?",
                [request.evaluation_request_id.into(), now_unix_secs.into()],
            )).await.map_err(database_error)?;
            recovered += result.rows_affected();
        }
        transaction.commit().await.map_err(database_error)?;
        Ok(recovered)
    }

    async fn automatic_reply_is_denied(
        &self,
        account: &crate::SourceAccountRef,
        actor_id: &str,
    ) -> Result<bool, NotificationPolicyStoreError> {
        if actor_id.trim().is_empty() {
            return Ok(true);
        }
        let account_id = load_account_id(&self.db, account).await?;
        let canonical_scope_key = automatic_reply_scope_key(actor_id);
        #[derive(FromQueryResult)]
        struct RuleRow {
            rule_json: String,
        }
        let row = RuleRow::find_by_statement(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            "SELECT CAST(revision.rule_json AS CHAR) AS rule_json \
             FROM secretary_notification_policy_families AS family \
             INNER JOIN secretary_notification_policy_revisions AS revision \
                 ON revision.policy_family_id = family.policy_family_id \
                AND revision.policy_revision_id = family.current_revision_id \
             WHERE family.account_id = ? AND family.policy_kind = ? \
               AND family.canonical_scope_key = ? AND revision.revision_kind = 'rule'",
            [
                account_id.into(),
                policy_kind_name(crate::NotificationPolicyKind::AutomaticReplyDenied).into(),
                canonical_scope_key.into(),
            ],
        ))
        .one(&self.db)
        .await
        .map_err(database_error)?;
        let Some(row) = row else {
            return Ok(false);
        };
        let rule: crate::NotificationPolicyRule = serde_json::from_str(&row.rule_json)
            .map_err(|_| NotificationPolicyStoreError::Database)?;
        Ok(matches!(rule.match_key.actor_id, crate::MatchField::Known(value) if value == actor_id))
    }
}

#[derive(FromQueryResult)]
struct EvaluationSnapshotRow {
    evaluation_request_id: String,
    managed_account_id: u64,
    source_kind: String,
    source_id: String,
    source_version: u64,
    candidate_status: String,
    match_key_json: String,
    policy_epoch: u64,
    managed_source_channel: String,
    managed_platform_account_id: String,
    command_source_channel: String,
    command_platform_account_id: String,
    owner_actor_id: String,
}

#[derive(FromQueryResult)]
struct PolicyHeadRow {
    policy_family_id: String,
    current_revision_id: String,
    generation: u64,
    policy_kind: String,
    revision_kind: String,
    rule_json: Option<String>,
}

#[derive(FromQueryResult)]
struct EvaluationRequestCommitRow {
    evaluation_request_id: String,
    notification_candidate_id: String,
    account_id: u64,
    source_kind: String,
    source_id: String,
    source_version: u64,
    candidate_status: String,
    policy_epoch: u64,
    previous_decision_id: Option<String>,
}

#[derive(FromQueryResult)]
struct PolicyHeadFenceRow {
    current_revision_id: String,
    generation: u64,
}

#[derive(FromQueryResult)]
struct EvaluationRequestRow {
    evaluation_request_id: String,
    attempt: u64,
}

struct FamilyRow {
    policy_family_id: String,
    canonical_scope_key: String,
    policy_kind: String,
    current_revision_id: Option<String>,
    generation: u64,
}

impl FromQueryResult for FamilyRow {
    fn from_query_result(result: &sea_orm::QueryResult, pre: &str) -> Result<Self, sea_orm::DbErr> {
        Ok(Self {
            policy_family_id: result.try_get(pre, "policy_family_id")?,
            canonical_scope_key: result.try_get(pre, "canonical_scope_key")?,
            policy_kind: result.try_get(pre, "policy_kind")?,
            current_revision_id: result.try_get(pre, "current_revision_id")?,
            generation: result.try_get(pre, "generation")?,
        })
    }
}

#[derive(FromQueryResult)]
struct PolicyEffectReceiptRow {
    proposal_json: String,
    result_ref: String,
    run_id: String,
}

async fn verify_action_lease(
    db: &sea_orm::DatabaseTransaction,
    request: &crate::NotificationPolicyEffectRequest,
    account_id: u64,
) -> Result<(), NotificationPolicyStoreError> {
    let verified = db
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            "UPDATE secretary_action_runs SET updated_at = UTC_TIMESTAMP(6) \
             WHERE run_id = ? AND lease_token = ? AND status = 'running' AND account_id = ? \
               AND command_source_event_id = ? AND lease_expires_at IS NOT NULL \
               AND lease_expires_at >= UTC_TIMESTAMP(6)",
            [
                request.run_id.clone().into(),
                request.lease_token.clone().into(),
                account_id.into(),
                request.command_source_event_id.as_str().into(),
            ],
        ))
        .await
        .map_err(database_error)?;
    if verified.rows_affected() != 1 {
        return Err(NotificationPolicyStoreError::Conflict);
    }
    Ok(())
}

async fn verify_effect_owner_authorization(
    db: &sea_orm::DatabaseTransaction,
    request: &crate::NotificationPolicyEffectRequest,
    managed_account_id: u64,
) -> Result<(), NotificationPolicyStoreError> {
    // 第一步：读取命令事件并锁定，确认其为 owner_command。
    let command_row = CommandAuthRow::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "SELECT account_id, actor_platform_id, message_role \
         FROM secretary_source_events \
         WHERE source_event_id = ? FOR UPDATE",
        [request.command_source_event_id.as_str().into()],
    ))
    .one(db)
    .await
    .map_err(database_error)?
    .ok_or(NotificationPolicyStoreError::Unauthorized)?;
    if command_row.message_role != "owner_command" {
        return Err(NotificationPolicyStoreError::Unauthorized);
    }

    // 第二步：读取目标账号的所有 active OwnerBinding（最多 2 条用于检测重复），
    // 不做按 command account 的预先 JOIN，确保重复 binding 能被显式捕获。
    let bindings = BindingAuthRow::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "SELECT binding_id, command_account_id, owner_actor_id \
         FROM secretary_owner_bindings \
         WHERE managed_account_id = ? AND status = 'active' \
         LIMIT 2 FOR UPDATE",
        [managed_account_id.into()],
    ))
    .all(db)
    .await
    .map_err(database_error)?;
    match bindings.as_slice() {
        [] => Err(NotificationPolicyStoreError::Unauthorized),
        [binding] => {
            if binding.command_account_id == command_row.account_id
                && binding.owner_actor_id == command_row.actor_platform_id
            {
                Ok(())
            } else {
                Err(NotificationPolicyStoreError::Unauthorized)
            }
        }
        _ => Err(NotificationPolicyStoreError::Conflict),
    }
}

#[derive(FromQueryResult)]
struct CommandAuthRow {
    account_id: u64,
    actor_platform_id: String,
    message_role: String,
}

#[derive(FromQueryResult)]
struct BindingAuthRow {
    #[allow(dead_code)]
    binding_id: String,
    command_account_id: u64,
    owner_actor_id: String,
}

async fn load_policy_effect_receipt(
    db: &sea_orm::DatabaseTransaction,
    request: &crate::NotificationPolicyEffectRequest,
) -> Result<Option<SecretaryActionReceipt>, NotificationPolicyStoreError> {
    let row = PolicyEffectReceiptRow::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "SELECT CAST(proposal_json AS CHAR) AS proposal_json, result_ref, run_id \
         FROM secretary_action_effect_receipts WHERE effect_id = ?",
        [request.effect_id.clone().into()],
    ))
    .one(db)
    .await
    .map_err(database_error)?;
    row.map(|row| {
        if row.run_id != request.run_id {
            return Err(NotificationPolicyStoreError::Conflict);
        }
        let proposal: crate::SecretaryActionProposal = serde_json::from_str(&row.proposal_json)
            .map_err(|_| NotificationPolicyStoreError::Database)?;
        if proposal.proposal_id != request.proposal_id || proposal.action != request.action {
            return Err(NotificationPolicyStoreError::Conflict);
        }
        Ok(SecretaryActionReceipt {
            proposal_id: proposal.proposal_id,
            result_ref: row.result_ref,
            tool_kind: Some(request.action.kind()),
        })
    })
    .transpose()
}

async fn apply_policy_effect_action(
    db: &sea_orm::DatabaseTransaction,
    request: &crate::NotificationPolicyEffectRequest,
    account_id: u64,
) -> Result<String, NotificationPolicyStoreError> {
    match &request.action {
        crate::SecretaryAction::SetAccountDefaultNotificationMode {
            canonical_scope_key,
            match_key,
            outcome,
            bypass_quiet,
        } => {
            apply_policy_write_effect(
                db,
                request,
                account_id,
                canonical_scope_key,
                crate::NotificationPolicyKind::AccountDefault,
                crate::NotificationPolicyRule {
                    match_key: match_key.clone(),
                    outcome: *outcome,
                    bypass_quiet: *bypass_quiet,
                    conversation: None,
                    quiet_hours: None,
                },
                "owner set account default notification mode",
            )
            .await
        }
        crate::SecretaryAction::SetConversationNotificationMode {
            canonical_scope_key,
            match_key,
            outcome,
            bypass_quiet,
            fully_silent,
            allow_bypass,
        } => {
            apply_policy_write_effect(
                db,
                request,
                account_id,
                canonical_scope_key,
                crate::NotificationPolicyKind::Conversation,
                crate::NotificationPolicyRule {
                    match_key: match_key.clone(),
                    outcome: *outcome,
                    bypass_quiet: *bypass_quiet,
                    conversation: Some(crate::ConversationNotificationRule {
                        mode: if *fully_silent {
                            crate::ConversationMode::FullySilent
                        } else {
                            crate::ConversationMode::Normal
                        },
                        allow_bypass: *allow_bypass,
                    }),
                    quiet_hours: None,
                },
                "owner set conversation notification mode",
            )
            .await
        }
        crate::SecretaryAction::SetQuietHours {
            canonical_scope_key,
            match_key,
            quiet_hours,
        } => {
            apply_policy_write_effect(
                db,
                request,
                account_id,
                canonical_scope_key,
                crate::NotificationPolicyKind::QuietHours,
                crate::NotificationPolicyRule {
                    match_key: match_key.clone(),
                    outcome: crate::NotificationOutcome::Suppress,
                    bypass_quiet: false,
                    conversation: None,
                    quiet_hours: Some(quiet_hours.clone()),
                },
                "owner set notification quiet hours",
            )
            .await
        }
        crate::SecretaryAction::SetImportantContact {
            canonical_scope_key,
            match_key,
            outcome,
            bypass_quiet,
        } => {
            apply_policy_write_effect(
                db,
                request,
                account_id,
                canonical_scope_key,
                crate::NotificationPolicyKind::Contact,
                basic_policy_rule(match_key, *outcome, *bypass_quiet),
                "owner set important notification contact",
            )
            .await
        }
        crate::SecretaryAction::SetNotificationCategoryImportance {
            canonical_scope_key,
            match_key,
            outcome,
            bypass_quiet,
        } => {
            apply_policy_write_effect(
                db,
                request,
                account_id,
                canonical_scope_key,
                crate::NotificationPolicyKind::Category,
                basic_policy_rule(match_key, *outcome, *bypass_quiet),
                "owner set notification category importance",
            )
            .await
        }
        crate::SecretaryAction::CreateSimilarNotificationRule {
            canonical_scope_key,
            match_key,
            outcome,
            bypass_quiet,
        } => {
            match_key
                .eligibility_for_long_term_rule()
                .map_err(policy_error)?;
            apply_policy_write_effect(
                db,
                request,
                account_id,
                canonical_scope_key,
                crate::NotificationPolicyKind::SimilarNotification,
                basic_policy_rule(match_key, *outcome, *bypass_quiet),
                "owner created similar notification rule",
            )
            .await
        }
        crate::SecretaryAction::SetAutomaticReplyDeniedForContact {
            canonical_scope_key,
            match_key,
        } => {
            apply_policy_write_effect(
                db,
                request,
                account_id,
                canonical_scope_key,
                crate::NotificationPolicyKind::AutomaticReplyDenied,
                basic_policy_rule(match_key, crate::NotificationOutcome::Suppress, false),
                "owner denied automatic reply for contact",
            )
            .await
        }
        crate::SecretaryAction::DisableNotificationPolicy {
            policy_family_id,
            expected_generation,
        } => {
            let family = load_family_for_update(db, account_id, policy_family_id).await?;
            if family.generation != *expected_generation {
                return Err(NotificationPolicyStoreError::Conflict);
            }
            let revision = append_revision(
                db,
                &family,
                RevisionKind::Tombstone,
                None,
                Some(request.command_source_event_id.as_str()),
                "owner disabled notification policy",
            )
            .await?;
            let updated = update_family_head(
                db,
                &family.policy_family_id,
                family.generation,
                &revision.policy_revision_id,
            )
            .await?;
            increment_policy_epoch(db, account_id).await?;
            policy_family_result_ref(
                updated,
                request.account.clone(),
                "tombstone",
                "policy_disabled",
            )
        }
        crate::SecretaryAction::RecordNotificationFeedback {
            candidate,
            match_key,
            important,
            promote_to_rule,
        } => {
            if candidate.account != request.account || match_key.account != request.account {
                return Err(NotificationPolicyStoreError::Conflict);
            }
            if *promote_to_rule {
                match_key
                    .eligibility_for_long_term_rule()
                    .map_err(policy_error)?;
            }
            let feedback = NotificationFeedbackRequest {
                candidate: candidate.clone(),
                match_key: match_key.clone(),
                important: *important,
                promote_to_rule: *promote_to_rule,
                command_source_event_id: request.command_source_event_id.as_str().to_owned(),
            };
            let candidate_id = candidate_id(db, account_id, &feedback.candidate).await?;
            let inserted = db
                .execute_raw(Statement::from_sql_and_values(
                    DatabaseBackend::MySql,
                    "INSERT IGNORE INTO secretary_notification_feedback \
                     (feedback_id, account_id, notification_candidate_id, important, promote_to_rule, \
                      command_source_event_id, audit_summary) VALUES (?, ?, ?, ?, ?, ?, ?)",
                    [
                        feedback_id(&feedback).into(),
                        account_id.into(),
                        candidate_id.into(),
                        feedback.important.into(),
                        feedback.promote_to_rule.into(),
                        feedback.command_source_event_id.clone().into(),
                        "owner notification feedback".into(),
                    ],
                ))
                .await
                .map_err(database_error)?;
            if inserted.rows_affected() > 1 {
                return Err(NotificationPolicyStoreError::Database);
            }
            if inserted.rows_affected() == 1 && feedback.promote_to_rule {
                promote_feedback_to_rule(db, account_id, &feedback).await?;
            }
            Ok("{\"scope\":\"notification_feedback\",\"status\":\"recorded\",\"priority\":\"feedback\",\"typed_reason\":\"feedback_recorded\",\"audit_reference\":\"notification_feedback\"}".into())
        }
        _ => Err(NotificationPolicyStoreError::Conflict),
    }
}

fn basic_policy_rule(
    match_key: &crate::NotificationMatchKeyV1,
    outcome: crate::NotificationOutcome,
    bypass_quiet: bool,
) -> crate::NotificationPolicyRule {
    crate::NotificationPolicyRule {
        match_key: match_key.clone(),
        outcome,
        bypass_quiet,
        conversation: None,
        quiet_hours: None,
    }
}

async fn apply_policy_write_effect(
    db: &sea_orm::DatabaseTransaction,
    request: &crate::NotificationPolicyEffectRequest,
    account_id: u64,
    canonical_scope_key: &str,
    policy_kind: crate::NotificationPolicyKind,
    rule: crate::NotificationPolicyRule,
    audit_summary: &str,
) -> Result<String, NotificationPolicyStoreError> {
    rule.match_key
        .eligibility_for_long_term_rule()
        .map_err(policy_error)?;
    if canonical_scope_key.trim().is_empty()
        || canonical_scope_key.len() > crate::MAX_CANONICAL_SCOPE_KEY_BYTES
    {
        return Err(NotificationPolicyStoreError::Conflict);
    }
    let family =
        match find_family_by_scope_for_update(db, account_id, canonical_scope_key, policy_kind)
            .await?
        {
            Some(family) => family,
            None => {
                match create_policy_family(db, account_id, canonical_scope_key, policy_kind).await {
                    Ok(family) => family,
                    Err(error) if is_policy_family_duplicate_key(&error) => {
                        find_family_by_scope_for_update(
                            db,
                            account_id,
                            canonical_scope_key,
                            policy_kind,
                        )
                        .await?
                        .ok_or(NotificationPolicyStoreError::Database)?
                    }
                    Err(error) => return Err(database_error(error)),
                }
            }
        };
    let revision = append_revision(
        db,
        &family,
        RevisionKind::Rule,
        Some(&rule),
        Some(request.command_source_event_id.as_str()),
        audit_summary,
    )
    .await?;
    let updated = update_family_head(
        db,
        &family.policy_family_id,
        family.generation,
        &revision.policy_revision_id,
    )
    .await?;
    increment_policy_epoch(db, account_id).await?;
    policy_family_result_ref(updated, request.account.clone(), "rule", "policy_written")
}

async fn create_policy_family(
    db: &sea_orm::DatabaseTransaction,
    account_id: u64,
    canonical_scope_key: &str,
    policy_kind: crate::NotificationPolicyKind,
) -> Result<FamilyRow, DbErr> {
    let policy_family_id = PolicyFamilyId::generate();
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "INSERT INTO secretary_notification_policy_families \
         (policy_family_id, account_id, canonical_scope_key, policy_kind, current_revision_id, generation) \
         VALUES (?, ?, ?, ?, NULL, 1)",
        [
            policy_family_id.as_str().into(),
            account_id.into(),
            canonical_scope_key.to_owned().into(),
            policy_kind_name(policy_kind).into(),
        ],
    ))
    .await?;
    Ok(FamilyRow {
        policy_family_id: policy_family_id.as_str().to_owned(),
        canonical_scope_key: canonical_scope_key.to_owned(),
        policy_kind: policy_kind_name(policy_kind).to_owned(),
        current_revision_id: None,
        generation: 1,
    })
}

fn policy_family_result_ref(
    family: FamilyRow,
    account: SourceAccountRef,
    status: &str,
    typed_reason: &str,
) -> Result<String, NotificationPolicyStoreError> {
    let family = to_domain_family(family, account)?;
    serde_json::to_string(&serde_json::json!({
        "scope": family.canonical_scope_key,
        "policy_family_id": family.policy_family_id,
        "policy_revision_id": family.current_revision_id,
        "decision_id": null,
        "status": status,
        "priority": policy_kind_name(family.policy_kind),
        "typed_reason": typed_reason,
        "audit_reference": format!("generation:{}", family.generation),
    }))
    .map_err(|_| NotificationPolicyStoreError::Database)
}

async fn load_account_id(
    db: &DatabaseConnection,
    account: &crate::SourceAccountRef,
) -> Result<u64, NotificationPolicyStoreError> {
    #[derive(FromQueryResult)]
    struct AccountRow {
        id: u64,
    }
    AccountRow::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "SELECT id FROM secretary_accounts WHERE source_channel = ? AND platform_account_id = ?",
        [
            account.channel.as_str().into(),
            account.account_id.clone().into(),
        ],
    ))
    .one(db)
    .await
    .map_err(database_error)?
    .map(|row| row.id)
    .ok_or(NotificationPolicyStoreError::Conflict)
}

async fn lock_account(
    db: &sea_orm::DatabaseTransaction,
    account: &crate::SourceAccountRef,
) -> Result<u64, NotificationPolicyStoreError> {
    #[derive(FromQueryResult)]
    struct AccountRow {
        id: u64,
    }
    AccountRow::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "SELECT id FROM secretary_accounts WHERE source_channel = ? AND platform_account_id = ? FOR UPDATE",
        [account.channel.as_str().into(), account.account_id.clone().into()],
    ))
    .one(db)
    .await
    .map_err(database_error)?
    .map(|row| row.id)
    .ok_or(NotificationPolicyStoreError::Conflict)
}

async fn promote_feedback_to_rule(
    db: &sea_orm::DatabaseTransaction,
    account_id: u64,
    request: &NotificationFeedbackRequest,
) -> Result<(), NotificationPolicyStoreError> {
    let (canonical_scope_key, policy_kind) = feedback_policy_scope(request)?;
    let family =
        match find_family_by_scope_for_update(db, account_id, &canonical_scope_key, policy_kind)
            .await?
        {
            Some(family) => family,
            None => {
                create_feedback_family(db, account_id, &canonical_scope_key, policy_kind).await?
            }
        };
    let rule = crate::NotificationPolicyRule {
        match_key: request.match_key.clone(),
        outcome: if request.important {
            crate::NotificationOutcome::Remind
        } else {
            crate::NotificationOutcome::Suppress
        },
        bypass_quiet: false,
        conversation: None,
        quiet_hours: None,
    };
    let revision = append_revision(
        db,
        &family,
        RevisionKind::Rule,
        Some(&rule),
        Some(&request.command_source_event_id),
        "owner notification feedback promotion",
    )
    .await?;
    update_family_head(
        db,
        &family.policy_family_id,
        family.generation,
        &revision.policy_revision_id,
    )
    .await?;
    increment_policy_epoch(db, account_id).await
}

fn feedback_policy_scope(
    request: &NotificationFeedbackRequest,
) -> Result<(String, crate::NotificationPolicyKind), NotificationPolicyStoreError> {
    let match_key = &request.match_key;
    match (
        &match_key.conversation,
        &match_key.actor_id,
        &match_key.category,
    ) {
        (
            crate::MatchField::Known(conversation),
            crate::MatchField::Absent,
            crate::MatchField::Absent,
        ) => Ok((
            format!(
                "conversation:{}:{}",
                conversation.kind.as_str(),
                conversation.id
            ),
            crate::NotificationPolicyKind::Conversation,
        )),
        (
            crate::MatchField::Absent,
            crate::MatchField::Known(actor_id),
            crate::MatchField::Absent,
        ) => Ok((
            format!("contact:{actor_id}"),
            crate::NotificationPolicyKind::Contact,
        )),
        (
            crate::MatchField::Absent,
            crate::MatchField::Absent,
            crate::MatchField::Known(category),
        ) => Ok((
            format!("category:{}", notification_category_name(*category)),
            crate::NotificationPolicyKind::Category,
        )),
        (crate::MatchField::Absent, crate::MatchField::Absent, crate::MatchField::Absent) => Ok((
            "account:default".into(),
            crate::NotificationPolicyKind::AccountDefault,
        )),
        _ => Err(NotificationPolicyStoreError::Conflict),
    }
}

async fn find_family_by_scope_for_update(
    db: &sea_orm::DatabaseTransaction,
    account_id: u64,
    canonical_scope_key: &str,
    policy_kind: crate::NotificationPolicyKind,
) -> Result<Option<FamilyRow>, NotificationPolicyStoreError> {
    FamilyRow::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "SELECT policy_family_id, canonical_scope_key, policy_kind, current_revision_id, generation \
         FROM secretary_notification_policy_families \
         WHERE account_id = ? AND canonical_scope_key = ? AND policy_kind = ? FOR UPDATE",
        [
            account_id.into(),
            canonical_scope_key.to_owned().into(),
            policy_kind_name(policy_kind).into(),
        ],
    ))
    .one(db)
    .await
    .map_err(database_error)
}

async fn create_feedback_family(
    db: &sea_orm::DatabaseTransaction,
    account_id: u64,
    canonical_scope_key: &str,
    policy_kind: crate::NotificationPolicyKind,
) -> Result<FamilyRow, NotificationPolicyStoreError> {
    let policy_family_id = PolicyFamilyId::generate();
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "INSERT INTO secretary_notification_policy_families \
         (policy_family_id, account_id, canonical_scope_key, policy_kind, current_revision_id, generation) \
         VALUES (?, ?, ?, ?, NULL, 1)",
        [
            policy_family_id.as_str().into(),
            account_id.into(),
            canonical_scope_key.to_owned().into(),
            policy_kind_name(policy_kind).into(),
        ],
    ))
    .await
    .map_err(database_error)?;
    Ok(FamilyRow {
        policy_family_id: policy_family_id.as_str().to_owned(),
        canonical_scope_key: canonical_scope_key.to_owned(),
        policy_kind: policy_kind_name(policy_kind).to_owned(),
        current_revision_id: None,
        generation: 1,
    })
}

async fn find_command_revision(
    db: &sea_orm::DatabaseTransaction,
    account_id: u64,
    command_source_event_id: Option<&str>,
) -> Result<Option<FamilyRow>, NotificationPolicyStoreError> {
    let Some(command_source_event_id) = command_source_event_id else {
        return Ok(None);
    };
    FamilyRow::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "SELECT family.policy_family_id, family.canonical_scope_key, family.policy_kind, \
                family.current_revision_id, family.generation \
         FROM secretary_notification_policy_revisions AS revision \
         INNER JOIN secretary_notification_policy_families AS family \
             ON family.policy_family_id = revision.policy_family_id \
         WHERE family.account_id = ? AND revision.command_source_event_id = ? \
         ORDER BY revision.revision_number DESC LIMIT 1 FOR UPDATE",
        [account_id.into(), command_source_event_id.to_owned().into()],
    ))
    .one(db)
    .await
    .map_err(database_error)
}

async fn create_family(
    db: &sea_orm::DatabaseTransaction,
    account_id: u64,
    request: &NotificationPolicyWriteRequest,
) -> Result<FamilyRow, NotificationPolicyStoreError> {
    let policy_family_id = PolicyFamilyId::generate();
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "INSERT INTO secretary_notification_policy_families \
         (policy_family_id, account_id, canonical_scope_key, policy_kind, current_revision_id, generation) \
         VALUES (?, ?, ?, ?, NULL, 1)",
        [
            policy_family_id.as_str().into(),
            account_id.into(),
            request.canonical_scope_key.clone().into(),
            policy_kind_name(request.policy_kind).into(),
        ],
    ))
    .await
    .map_err(database_error)?;
    Ok(FamilyRow {
        policy_family_id: policy_family_id.as_str().to_owned(),
        canonical_scope_key: request.canonical_scope_key.clone(),
        policy_kind: policy_kind_name(request.policy_kind).to_owned(),
        current_revision_id: None,
        generation: 1,
    })
}

async fn load_family_for_update(
    db: &sea_orm::DatabaseTransaction,
    account_id: u64,
    family_id: &PolicyFamilyId,
) -> Result<FamilyRow, NotificationPolicyStoreError> {
    FamilyRow::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "SELECT policy_family_id, canonical_scope_key, policy_kind, current_revision_id, generation \
         FROM secretary_notification_policy_families \
         WHERE policy_family_id = ? AND account_id = ? FOR UPDATE",
        [family_id.as_str().into(), account_id.into()],
    ))
    .one(db)
    .await
    .map_err(database_error)?
    .ok_or(NotificationPolicyStoreError::Conflict)
}

async fn append_revision(
    db: &sea_orm::DatabaseTransaction,
    family: &FamilyRow,
    revision_kind: RevisionKind,
    rule: Option<&crate::NotificationPolicyRule>,
    command_source_event_id: Option<&str>,
    audit_summary: &str,
) -> Result<NotificationPolicyRevision, NotificationPolicyStoreError> {
    let policy_revision_id = PolicyRevisionId::generate();
    let revision_number = family.generation;
    let revision = NotificationPolicyRevision {
        policy_revision_id: policy_revision_id.clone(),
        policy_family_id: PolicyFamilyId::new(family.policy_family_id.clone())
            .map_err(policy_error)?,
        revision_number,
        supersedes_revision_id: family
            .current_revision_id
            .clone()
            .map(PolicyRevisionId::new)
            .transpose()
            .map_err(policy_error)?,
        revision_kind,
        rule: rule.cloned(),
    };
    revision.validate().map_err(policy_error)?;
    let rule_json = revision
        .rule
        .as_ref()
        .map(serde_json::to_value)
        .transpose()
        .map_err(|_| NotificationPolicyStoreError::Conflict)?;
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "INSERT INTO secretary_notification_policy_revisions \
         (policy_revision_id, policy_family_id, revision_number, supersedes_revision_id, revision_kind, \
          rule_json, command_source_event_id, audit_summary) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        [
            policy_revision_id.as_str().into(),
            family.policy_family_id.clone().into(),
            revision_number.into(),
            family.current_revision_id.clone().into(),
            revision_kind_name(revision_kind).into(),
            rule_json.into(),
            command_source_event_id.map(str::to_owned).into(),
            audit_summary.into(),
        ],
    ))
    .await
    .map_err(database_error)?;
    Ok(revision)
}

async fn update_family_head(
    db: &sea_orm::DatabaseTransaction,
    policy_family_id: &str,
    expected_generation: u64,
    revision_id: &PolicyRevisionId,
) -> Result<FamilyRow, NotificationPolicyStoreError> {
    let result = db
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            "UPDATE secretary_notification_policy_families \
             SET current_revision_id = ?, generation = generation + 1 \
             WHERE policy_family_id = ? AND generation = ?",
            [
                revision_id.as_str().into(),
                policy_family_id.into(),
                expected_generation.into(),
            ],
        ))
        .await
        .map_err(database_error)?;
    if result.rows_affected() != 1 {
        return Err(NotificationPolicyStoreError::Conflict);
    }
    load_family_row(db, policy_family_id).await
}

async fn load_family_row(
    db: &sea_orm::DatabaseTransaction,
    policy_family_id: &str,
) -> Result<FamilyRow, NotificationPolicyStoreError> {
    let row = FamilyRow::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "SELECT policy_family_id, canonical_scope_key, policy_kind, current_revision_id, generation \
         FROM secretary_notification_policy_families WHERE policy_family_id = ?",
        [policy_family_id.into()],
    ))
    .one(db)
    .await
    .map_err(database_error)?
    .ok_or(NotificationPolicyStoreError::Conflict)?;
    Ok(row)
}

fn to_domain_family(
    row: FamilyRow,
    account: crate::SourceAccountRef,
) -> Result<NotificationPolicyFamily, NotificationPolicyStoreError> {
    Ok(NotificationPolicyFamily {
        policy_family_id: PolicyFamilyId::new(row.policy_family_id).map_err(policy_error)?,
        account,
        canonical_scope_key: row.canonical_scope_key,
        policy_kind: parse_policy_kind(&row.policy_kind)?,
        current_revision_id: PolicyRevisionId::new(
            row.current_revision_id
                .ok_or(NotificationPolicyStoreError::Conflict)?,
        )
        .map_err(policy_error)?,
        generation: row.generation,
    })
}

async fn increment_policy_epoch(
    db: &sea_orm::DatabaseTransaction,
    account_id: u64,
) -> Result<(), NotificationPolicyStoreError> {
    let result = db
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            "UPDATE secretary_accounts SET policy_epoch = policy_epoch + 1 WHERE id = ?",
            [account_id.into()],
        ))
        .await
        .map_err(database_error)?;
    if result.rows_affected() != 1 {
        return Err(NotificationPolicyStoreError::Conflict);
    }
    Ok(())
}

async fn source_is_current(
    db: &sea_orm::DatabaseTransaction,
    account_id: u64,
    source_kind: &str,
    source_id: &str,
    source_version: u64,
) -> Result<bool, NotificationPolicyStoreError> {
    let sql = match source_kind {
        "agenda" => {
            "SELECT 1 AS present FROM secretary_agenda_items \
             WHERE item_id = ? AND account_id = ? AND version = ? AND item_status = 'scheduled' \
             FOR UPDATE"
        }
        "follow_up" => {
            "SELECT 1 AS present \
             FROM secretary_follow_up_items AS item \
             INNER JOIN secretary_memory_facts AS fact \
               ON fact.fact_id = item.source_memory_fact_id \
             WHERE item.follow_up_id = ? AND item.account_id = ? AND item.source_version = ? \
               AND item.status = 'scheduled' \
               AND fact.fact_status = 'confirmed' \
               AND NOT EXISTS ( \
                   SELECT 1 FROM secretary_memory_facts AS successor \
                   WHERE successor.supersedes_fact_id = fact.fact_id \
               ) \
               AND ( \
                    (item.reason_code = 'commitment_due' AND fact.fact_kind = 'commitment' \
                     AND JSON_UNQUOTE(JSON_EXTRACT(fact.fact_json, '$.payload.data.status')) \
                         IN ('pending', 'proposed')) \
                    OR (item.reason_code = 'project_blocked' AND fact.fact_kind = 'project' \
                        AND JSON_LENGTH(JSON_EXTRACT(fact.fact_json, '$.payload.data.blockers')) > 0) \
               ) \
             FOR UPDATE"
        }
        "response_expectation" => {
            "SELECT 1 AS present FROM secretary_response_expectations \
             WHERE expectation_id = ? AND account_id = ? AND source_version = ? \
               AND expectation_status = 'active' FOR UPDATE"
        }
        _ => return Ok(false),
    };
    let row = db
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            sql,
            [source_id.into(), account_id.into(), source_version.into()],
        ))
        .await
        .map_err(database_error)?;
    Ok(row.is_some())
}

async fn candidate_id(
    db: &sea_orm::DatabaseTransaction,
    account_id: u64,
    candidate: &crate::NotificationCandidateRef,
) -> Result<String, NotificationPolicyStoreError> {
    #[derive(FromQueryResult)]
    struct CandidateRow {
        notification_candidate_id: String,
    }
    CandidateRow::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "SELECT notification_candidate_id FROM secretary_notification_candidates \
         WHERE account_id = ? AND source_kind = ? AND source_id = ? AND source_version = ? FOR UPDATE",
        [
            account_id.into(),
            candidate.source_kind.clone().into(),
            candidate.source_id.clone().into(),
            candidate.source_version.into(),
        ],
    ))
    .one(db)
    .await
    .map_err(database_error)?
    .map(|row| row.notification_candidate_id)
    .ok_or(NotificationPolicyStoreError::Conflict)
}

fn feedback_id(request: &NotificationFeedbackRequest) -> String {
    let mut hasher = Sha256::new();
    hasher.update(request.candidate.account.channel.as_str().as_bytes());
    hasher.update([0]);
    hasher.update(request.candidate.account.account_id.as_bytes());
    hasher.update([0]);
    hasher.update(request.command_source_event_id.as_bytes());
    hasher.update([u8::from(request.important)]);
    let digest = hasher.finalize();
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    uuid::Uuid::from_bytes(bytes).to_string()
}

fn validate_write_request(
    request: &NotificationPolicyWriteRequest,
) -> Result<(), NotificationPolicyStoreError> {
    if request.canonical_scope_key.trim().is_empty()
        || request.canonical_scope_key.len() > crate::MAX_CANONICAL_SCOPE_KEY_BYTES
        || request.audit_summary.trim().is_empty()
        || request.audit_summary.len() > crate::MAX_NOTIFICATION_AUDIT_SUMMARY_BYTES
    {
        return Err(NotificationPolicyStoreError::Conflict);
    }
    Ok(())
}

fn automatic_reply_scope_key(actor_id: &str) -> String {
    format!("contact:{actor_id}")
}

fn notification_category_name(category: crate::NotificationCategory) -> &'static str {
    match category {
        crate::NotificationCategory::Agenda => "agenda",
        crate::NotificationCategory::FollowUp => "follow_up",
    }
}
fn policy_kind_name(kind: crate::NotificationPolicyKind) -> &'static str {
    match kind {
        crate::NotificationPolicyKind::AccountDefault => "account_default",
        crate::NotificationPolicyKind::Conversation => "conversation",
        crate::NotificationPolicyKind::Contact => "contact",
        crate::NotificationPolicyKind::Category => "category",
        crate::NotificationPolicyKind::QuietHours => "quiet_hours",
        crate::NotificationPolicyKind::SimilarNotification => "similar_notification",
        crate::NotificationPolicyKind::AutomaticReplyDenied => "automatic_reply_denied",
    }
}

fn revision_kind_name(kind: RevisionKind) -> &'static str {
    match kind {
        RevisionKind::Rule => "rule",
        RevisionKind::Tombstone => "tombstone",
    }
}

fn parse_policy_kind(
    value: &str,
) -> Result<crate::NotificationPolicyKind, NotificationPolicyStoreError> {
    match value {
        "account_default" => Ok(crate::NotificationPolicyKind::AccountDefault),
        "conversation" => Ok(crate::NotificationPolicyKind::Conversation),
        "contact" => Ok(crate::NotificationPolicyKind::Contact),
        "category" => Ok(crate::NotificationPolicyKind::Category),
        "quiet_hours" => Ok(crate::NotificationPolicyKind::QuietHours),
        "similar_notification" => Ok(crate::NotificationPolicyKind::SimilarNotification),
        "automatic_reply_denied" => Ok(crate::NotificationPolicyKind::AutomaticReplyDenied),
        _ => Err(NotificationPolicyStoreError::Database),
    }
}

fn parse_message_source(value: &str) -> Result<MessageSource, NotificationPolicyStoreError> {
    match value {
        "napcat" => Ok(MessageSource::NapCat),
        "qq_open_platform" => Ok(MessageSource::QqOpenPlatform),
        _ => Err(NotificationPolicyStoreError::Database),
    }
}

fn source_account_from_db(
    source_channel: &str,
    platform_account_id: String,
) -> Result<SourceAccountRef, NotificationPolicyStoreError> {
    SourceAccountRef::new(parse_message_source(source_channel)?, platform_account_id)
        .map_err(|_| NotificationPolicyStoreError::Database)
}

fn outcome_name(outcome: NotificationOutcome) -> &'static str {
    match outcome {
        NotificationOutcome::Remind => "remind",
        NotificationOutcome::Delay => "delay",
        NotificationOutcome::Suppress => "suppress",
        NotificationOutcome::CandidateExpired => "candidate_expired",
        NotificationOutcome::EvaluationFailedTerminal => "evaluation_failed_terminal",
        NotificationOutcome::DeliveryWindowExpired => "delivery_window_expired",
        NotificationOutcome::ScheduleTimeAmbiguous => "schedule_time_ambiguous",
    }
}

fn decision_reason_name(reason: DecisionReason) -> &'static str {
    match reason {
        DecisionReason::CandidateExpired => "candidate_expired",
        DecisionReason::ConversationFullySilent => "conversation_fully_silent",
        DecisionReason::ConversationPolicy => "conversation_policy",
        DecisionReason::ContactPolicy => "contact_policy",
        DecisionReason::CategoryPolicy => "category_policy",
        DecisionReason::AccountDefaultPolicy => "account_default_policy",
        DecisionReason::QuietHours => "quiet_hours",
        DecisionReason::ScheduleTimeAmbiguous => "schedule_time_ambiguous",
        DecisionReason::InvalidQuietHours => "invalid_quiet_hours",
    }
}

fn candidate_status_name(outcome: NotificationOutcome) -> &'static str {
    match outcome {
        NotificationOutcome::Remind => "reminded",
        NotificationOutcome::Delay => "delayed",
        NotificationOutcome::Suppress => "suppressed",
        NotificationOutcome::CandidateExpired | NotificationOutcome::DeliveryWindowExpired => {
            "expired"
        }
        NotificationOutcome::EvaluationFailedTerminal
        | NotificationOutcome::ScheduleTimeAmbiguous => "failed_terminal",
    }
}

fn notification_occurrence_id(
    source_kind: &str,
    source_id: &str,
    source_version: u64,
    notification_kind: &str,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"notification-policy-occurrence-v2\0");
    hasher.update(source_kind.as_bytes());
    hasher.update([0]);
    hasher.update(source_id.as_bytes());
    hasher.update([0]);
    hasher.update(source_version.to_be_bytes());
    hasher.update([0]);
    hasher.update(notification_kind.as_bytes());
    let digest = hasher.finalize();
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    uuid::Uuid::from_bytes(bytes).to_string()
}

fn winning_revision_id(
    snapshot: &EvaluationSnapshot,
    plan: &crate::EvaluationPlan,
) -> Result<Option<PolicyRevisionId>, NotificationPolicyStoreError> {
    let kind = match plan.reason {
        DecisionReason::ConversationPolicy => Some(NotificationPolicyKind::Conversation),
        DecisionReason::ContactPolicy => Some(NotificationPolicyKind::Contact),
        DecisionReason::CategoryPolicy => Some(NotificationPolicyKind::Category),
        DecisionReason::AccountDefaultPolicy => Some(NotificationPolicyKind::AccountDefault),
        // v1 尚未持久化 ConversationNotificationRule 或 QuietHoursRule；这些原因没有
        // 可审计的 NotificationPolicyRevision，绝不从不匹配的 rule 猜测 revision。
        DecisionReason::ConversationFullySilent
        | DecisionReason::QuietHours
        | DecisionReason::CandidateExpired
        | DecisionReason::ScheduleTimeAmbiguous
        | DecisionReason::InvalidQuietHours => None,
    };
    let Some(kind) = kind else {
        return Ok(None);
    };
    snapshot
        .matching_rule_snapshot(kind)
        .map_err(|_| NotificationPolicyStoreError::Conflict)
        .map(|rule| rule.map(|rule| rule.policy_revision_id.clone()))
}

async fn load_locked_binding_command_account(
    db: &sea_orm::DatabaseTransaction,
    managed_account_id: u64,
    command_account: &SourceAccountRef,
    owner_actor_id: &str,
) -> Result<Option<u64>, NotificationPolicyStoreError> {
    #[derive(FromQueryResult)]
    struct BindingRow {
        command_account_id: u64,
    }
    let rows = BindingRow::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "SELECT binding.command_account_id FROM secretary_owner_bindings AS binding \
         INNER JOIN secretary_accounts AS command_account \
           ON command_account.id = binding.command_account_id \
         WHERE binding.managed_account_id = ? AND binding.owner_actor_id = ? \
           AND binding.status = 'active' AND command_account.source_channel = ? \
           AND command_account.platform_account_id = ? FOR UPDATE",
        [
            managed_account_id.into(),
            owner_actor_id.to_owned().into(),
            command_account.channel.as_str().into(),
            command_account.account_id.clone().into(),
        ],
    ))
    .all(db)
    .await
    .map_err(database_error)?;
    match rows.as_slice() {
        [row] => Ok(Some(row.command_account_id)),
        [] => Ok(None),
        // 多条 active binding 不能由查询顺序决定实际 Owner 收件人。
        _ => Err(NotificationPolicyStoreError::Conflict),
    }
}

async fn requeue_stale_request(
    db: &sea_orm::DatabaseTransaction,
    evaluation_request_id: &str,
    lease_token: &str,
) -> Result<(), NotificationPolicyStoreError> {
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "UPDATE secretary_notification_evaluation_requests \
         SET request_status = 'pending', lease_token = NULL, lease_expires_at_unix_secs = NULL \
         WHERE evaluation_request_id = ? AND request_status = 'claimed' AND lease_token = ?",
        [
            evaluation_request_id.to_owned().into(),
            lease_token.to_owned().into(),
        ],
    ))
    .await
    .map_err(database_error)?;
    Ok(())
}

async fn complete_request_and_candidate(
    db: &sea_orm::DatabaseTransaction,
    request: &EvaluationRequestCommitRow,
    request_status: &str,
    candidate_status: &str,
    next_allowed_at_unix_secs: Option<i64>,
) -> Result<(), NotificationPolicyStoreError> {
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "UPDATE secretary_notification_evaluation_requests \
         SET request_status = ?, lease_token = NULL, lease_expires_at_unix_secs = NULL, \
             next_allowed_at_unix_secs = ? WHERE evaluation_request_id = ?",
        [
            request_status.to_owned().into(),
            next_allowed_at_unix_secs.into(),
            request.evaluation_request_id.clone().into(),
        ],
    ))
    .await
    .map_err(database_error)?;
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "UPDATE secretary_notification_candidates SET candidate_status = ? \
         WHERE notification_candidate_id = ?",
        [
            candidate_status.to_owned().into(),
            request.notification_candidate_id.clone().into(),
        ],
    ))
    .await
    .map_err(database_error)?;
    Ok(())
}

fn is_policy_family_duplicate_key(error: &DbErr) -> bool {
    let sqlx_error = match error {
        DbErr::Exec(RuntimeErr::SqlxError(error)) | DbErr::Query(RuntimeErr::SqlxError(error)) => {
            error.as_ref()
        }
        _ => return false,
    };
    let sea_orm::sqlx::Error::Database(database_error) = sqlx_error else {
        return false;
    };
    database_error.code().as_deref() == Some("1062")
        && database_error
            .message()
            .contains("uk_secretary_notification_policy_family")
}

fn policy_error(_: crate::NotificationPolicyError) -> NotificationPolicyStoreError {
    NotificationPolicyStoreError::Conflict
}

fn database_error(_: sea_orm::DbErr) -> NotificationPolicyStoreError {
    NotificationPolicyStoreError::Database
}
