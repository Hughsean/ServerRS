//! GAP-003-A/B/C 历史多页持久化聚焦测试。
//!
//! 需要 `QQBOT_TEST_DATABASE_URL` 指向 `qqbot_accept_` 前缀的隔离 MySQL schema。
//! 默认 `#[ignore]`；每次运行派生随机 schema，无论成功、错误或 panic 都精确清理。

mod common;

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use common::{isolated_db, scalar_string, scalar_u64};
use personal_secretary::{
    BackfillAnchor, BackfillAnomaly, BackfillBudget, BackfillContinuation, BackfillCursor,
    BackfillGapUseCase, BackfillHistoryItem, BackfillLease, BackfillPage, BackfillReadDirection,
    BackfillScope, BackfillScopeStatus, BackfillSourceError, BackfillStateStoreWithIngestionT,
    ConversationKind, ConversationRef, HistoryBackfillSourceT, HistoryCompleteness,
    InboundMessageEnvelope, IngestionGapReason, IngestionGapStatus, MessageSource, ScopeProgress,
    SourceAccountRef, SourceMessageRef, VerifiedActor, VerifiedActorKind,
};
use personal_secretary_mysql::{build_mysql_backfill_store, build_mysql_inbound_event_store};
use sea_orm::{ConnectionTrait, DatabaseBackend, Statement};

async fn run_scenario<F>(suffix: &str, scenario: impl FnOnce(sea_orm::DatabaseConnection) -> F)
where
    F: std::future::Future<Output = Result<(), String>> + Send + 'static,
{
    let (db, schema) = isolated_db(suffix).await;
    let outcome = tokio::spawn(scenario(db.clone())).await;
    let cleanup = db
        .execute_unprepared(&format!("DROP DATABASE IF EXISTS `{schema}`"))
        .await;
    if let Err(error) = cleanup {
        eprintln!("schema cleanup failed for {schema}: {error}");
    }
    match outcome {
        Ok(Ok(())) => {}
        Ok(Err(message)) => panic!("scenario must pass: {message}"),
        Err(panic) => std::panic::resume_unwind(panic.into_panic()),
    }
}

fn envelope(account_id: &str, message_id: &str, sequence: i64) -> InboundMessageEnvelope {
    envelope_for_group(account_id, message_id, "gap003-group", sequence)
}

fn envelope_for_group(
    account_id: &str,
    message_id: &str,
    group_id: &str,
    sequence: i64,
) -> InboundMessageEnvelope {
    InboundMessageEnvelope::new(
        SourceMessageRef::new(MessageSource::NapCat, account_id, message_id).unwrap(),
        ConversationRef::new(ConversationKind::Group, group_id).unwrap(),
        VerifiedActor::new(VerifiedActorKind::External, "gap003-sender").unwrap(),
        sequence,
        format!("gap003 message {sequence}"),
        Vec::new(),
    )
    .unwrap()
}

struct UnprovenPagingSource {
    page: Mutex<Option<BackfillPage>>,
    cursors: Mutex<Vec<Option<BackfillCursor>>>,
}

#[async_trait]
impl HistoryBackfillSourceT for UnprovenPagingSource {
    async fn fetch_page(
        &self,
        _scope: &BackfillScope,
        cursor: Option<&BackfillCursor>,
        direction: BackfillReadDirection,
        _page_size: u32,
    ) -> Result<BackfillPage, BackfillSourceError> {
        assert_eq!(direction, BackfillReadDirection::NewestToOldest);
        self.cursors.lock().unwrap().push(cursor.cloned());
        self.page
            .lock()
            .unwrap()
            .take()
            .ok_or_else(|| BackfillSourceError::Unavailable("scripted page consumed".into()))
    }

    fn history_start_evidence_proven(&self) -> bool {
        false
    }

    fn page_order_evidence_proven(&self) -> bool {
        false
    }

    fn account_conversation_set_proven(&self) -> bool {
        false
    }
}

#[tokio::test]
#[ignore]
async fn persisted_cursor_survives_lease_recovery_and_unproven_stop_keeps_gap_uncertain() {
    run_scenario("_gap003_paging", |db| async move {
        let account = SourceAccountRef::new(MessageSource::NapCat, "gap003-account").unwrap();
        let inbound = build_mysql_inbound_event_store(db.clone());
        let epoch = inbound
            .begin_connection(&account)
            .await
            .map_err(|error| format!("begin connection failed: {error}"))?;
        inbound
            .mark_connection_connected(&epoch)
            .await
            .map_err(|error| format!("mark connected failed: {error}"))?;
        inbound
            .insert_message_if_absent(
                &envelope("gap003-account", "frozen-boundary", 1).observed_in(epoch.clone()),
            )
            .await
            .map_err(|error| format!("insert boundary failed: {error}"))?;
        inbound
            .mark_connection_uncertain(&epoch, IngestionGapReason::QueueOverflow)
            .await
            .map_err(|error| format!("mark uncertain failed: {error}"))?;
        let next_epoch = inbound
            .begin_connection(&account)
            .await
            .map_err(|error| format!("begin reconnect failed: {error}"))?;
        inbound
            .mark_connection_connected(&next_epoch)
            .await
            .map_err(|error| format!("mark reconnect connected failed: {error}"))?;

        let store: Arc<dyn BackfillStateStoreWithIngestionT> =
            build_mysql_backfill_store(db.clone(), 60);
        let claimed = store
            .claim_next_gap(BackfillLease::new(60))
            .await
            .map_err(|error| format!("claim failed: {error}"))?
            .ok_or_else(|| "expected a claimable gap".to_owned())?;
        let resume_cursor = BackfillCursor::new(
            account.clone(),
            BackfillAnchor::new("resume-anchor", "opaque/not-numeric+cursor"),
        );
        let progress = ScopeProgress {
            conversation: ConversationRef::new(ConversationKind::Group, "gap003-group").unwrap(),
            status: BackfillScopeStatus::Backfilling,
            last_cursor: Some(resume_cursor.clone()),
            pages_read: 1,
            events_read: 1,
            accepted: 1,
            duplicates: 0,
            reached_boundary: false,
            anomalies: Vec::new(),
        };
        store
            .record_scope_progress(&claimed.run_id, &claimed.lease_token, &progress)
            .await
            .map_err(|error| format!("record progress failed: {error}"))?;

        let persisted = store
            .load_run_progress(&claimed.run_id)
            .await
            .map_err(|error| format!("load progress failed: {error}"))?
            .ok_or_else(|| "persisted progress missing".to_owned())?;
        assert_eq!(persisted.len(), 1);
        assert_eq!(persisted[0].last_cursor, Some(resume_cursor.clone()));

        db.execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            "UPDATE secretary_backfill_runs \
             SET lease_expires_at = UTC_TIMESTAMP(6) - INTERVAL 1 SECOND \
             WHERE backfill_run_id = ?",
            [claimed.run_id.as_str().into()],
        ))
        .await
        .map_err(|error| format!("expire lease failed: {error}"))?;

        let source = Arc::new(UnprovenPagingSource {
            page: Mutex::new(Some(BackfillPage {
                items: vec![BackfillHistoryItem {
                    envelope: envelope("gap003-account", "resume-anchor", 2),
                    anchor: resume_cursor.anchor.clone(),
                }],
                continuation: BackfillContinuation::UnprovenStop,
            })),
            cursors: Mutex::new(Vec::new()),
        });
        let source_port: Arc<dyn HistoryBackfillSourceT> = source.clone();
        let budget = BackfillBudget {
            page_size: 100,
            max_pages_per_scope: 10,
            max_events_per_run: 100,
            max_concurrency: 1,
            lease_secs: 60,
            retry_initial_ms: 10,
            retry_max_ms: 100,
        };
        let use_case = BackfillGapUseCase::new(store.clone(), source_port, budget);
        let reclaimed = use_case
            .reclaim_expired(1)
            .await
            .map_err(|error| format!("reclaim failed: {error}"))?;
        assert_eq!(reclaimed.len(), 1);
        assert!(reclaimed[0].is_resume);
        let outcome = use_case
            .resume_claimed(reclaimed.into_iter().next().unwrap())
            .await
            .map_err(|error| format!("resume failed: {error}"))?;

        assert_eq!(
            source.cursors.lock().unwrap().as_slice(),
            [Some(resume_cursor)]
        );
        assert_eq!(outcome.completeness, HistoryCompleteness::Unprovable);
        assert_eq!(outcome.gap_target_status, IngestionGapStatus::Uncertain);
        assert!(
            outcome.evidence.scopes[0]
                .anomalies
                .contains(&BackfillAnomaly::UnprovenStop)
        );
        assert!(
            outcome.evidence.scopes[0]
                .anomalies
                .contains(&BackfillAnomaly::UntrustedPageOrder)
        );
        assert_eq!(
            scalar_string(
                &db,
                "SELECT status AS value FROM secretary_ingestion_gaps WHERE gap_id = ?",
                vec![outcome.gap_id.as_str().into()],
            )
            .await,
            "uncertain"
        );
        Ok(())
    })
    .await;
}

struct TargetedScopeSource {
    scopes: Mutex<Vec<BackfillScope>>,
}

#[async_trait]
impl HistoryBackfillSourceT for TargetedScopeSource {
    async fn fetch_page(
        &self,
        scope: &BackfillScope,
        cursor: Option<&BackfillCursor>,
        direction: BackfillReadDirection,
        _page_size: u32,
    ) -> Result<BackfillPage, BackfillSourceError> {
        assert_eq!(direction, BackfillReadDirection::NewestToOldest);
        assert!(
            cursor.is_none(),
            "unseen non-message conversation must start from the newest real history page"
        );
        self.scopes.lock().unwrap().push(scope.clone());
        Ok(BackfillPage {
            items: Vec::new(),
            continuation: BackfillContinuation::UnprovenStop,
        })
    }

    fn history_start_evidence_proven(&self) -> bool {
        false
    }

    fn page_order_evidence_proven(&self) -> bool {
        false
    }

    fn account_conversation_set_proven(&self) -> bool {
        false
    }
}

#[tokio::test]
#[ignore]
async fn non_message_signal_freezes_unseen_conversation_for_backfill() {
    run_scenario("_gap003_nonmsg", |db| async move {
        let account = SourceAccountRef::new(MessageSource::NapCat, "nonmsg-account").unwrap();
        let target = ConversationRef::new(ConversationKind::Group, "nonmsg-group").unwrap();
        let inbound = build_mysql_inbound_event_store(db.clone());
        let epoch = inbound
            .begin_connection(&account)
            .await
            .map_err(|error| format!("begin connection failed: {error}"))?;
        inbound
            .mark_connection_connected(&epoch)
            .await
            .map_err(|error| format!("mark connected failed: {error}"))?;
        inbound
            .mark_connection_uncertain_for_conversation(
                &epoch,
                IngestionGapReason::NonMessageReference,
                &target,
            )
            .await
            .map_err(|error| format!("persist non-message signal failed: {error}"))?;

        let store: Arc<dyn BackfillStateStoreWithIngestionT> =
            build_mysql_backfill_store(db.clone(), 60);
        let source = Arc::new(TargetedScopeSource {
            scopes: Mutex::new(Vec::new()),
        });
        let use_case = BackfillGapUseCase::new(
            store,
            source.clone(),
            BackfillBudget {
                page_size: 10,
                max_pages_per_scope: 1,
                max_events_per_run: 10,
                max_concurrency: 1,
                lease_secs: 60,
                retry_initial_ms: 10,
                retry_max_ms: 100,
            },
        );
        let outcome = use_case
            .run_one()
            .await
            .map_err(|error| format!("run targeted backfill failed: {error}"))?
            .ok_or_else(|| "expected targeted gap claim".to_owned())?;

        assert_eq!(outcome.gap_target_status, IngestionGapStatus::Uncertain);
        assert_eq!(
            source.scopes.lock().unwrap().as_slice(),
            [BackfillScope {
                account,
                conversation: target,
                boundary_cursor: Some(BackfillCursor::new(
                    SourceAccountRef::new(MessageSource::NapCat, "nonmsg-account").unwrap(),
                    BackfillAnchor::new(
                        "__non_message_history_signal_no_prior_cursor__",
                        String::new(),
                    ),
                )),
            }]
        );
        Ok(())
    })
    .await;
}

#[tokio::test]
#[ignore]
async fn online_non_message_gap_only_reads_signaled_scopes_and_preserves_real_boundary() {
    run_scenario("_gap003_nonmsg_scopes", |db| async move {
        let account =
            SourceAccountRef::new(MessageSource::NapCat, "nonmsg-scopes-account").unwrap();
        let known = ConversationRef::new(ConversationKind::Group, "target-known").unwrap();
        let unseen = ConversationRef::new(ConversationKind::Group, "target-unseen").unwrap();
        let unrelated = ConversationRef::new(ConversationKind::Group, "unrelated-known").unwrap();
        let inbound = build_mysql_inbound_event_store(db.clone());
        let epoch = inbound
            .begin_connection(&account)
            .await
            .map_err(|error| format!("begin connection failed: {error}"))?;
        inbound
            .mark_connection_connected(&epoch)
            .await
            .map_err(|error| format!("mark connected failed: {error}"))?;
        inbound
            .insert_message_if_absent(
                &envelope_for_group(
                    "nonmsg-scopes-account",
                    "known-real-boundary",
                    &known.id,
                    10,
                )
                .observed_in(epoch.clone()),
            )
            .await
            .map_err(|error| format!("insert known cursor failed: {error}"))?;
        inbound
            .insert_message_if_absent(
                &envelope_for_group(
                    "nonmsg-scopes-account",
                    "unrelated-real-boundary",
                    &unrelated.id,
                    11,
                )
                .observed_in(epoch.clone()),
            )
            .await
            .map_err(|error| format!("insert unrelated cursor failed: {error}"))?;
        let first_gap = inbound
            .mark_connection_uncertain_for_conversation(
                &epoch,
                IngestionGapReason::NonMessageReference,
                &known,
            )
            .await
            .map_err(|error| format!("persist known signal failed: {error}"))?;
        let second_gap = inbound
            .mark_connection_uncertain_for_conversation(
                &epoch,
                IngestionGapReason::NonMessageReference,
                &unseen,
            )
            .await
            .map_err(|error| format!("persist unseen signal failed: {error}"))?;
        assert_eq!(first_gap, second_gap, "one epoch must reuse one gap");

        let store: Arc<dyn BackfillStateStoreWithIngestionT> =
            build_mysql_backfill_store(db.clone(), 60);
        let source = Arc::new(TargetedScopeSource {
            scopes: Mutex::new(Vec::new()),
        });
        let use_case = BackfillGapUseCase::new(
            store,
            source.clone(),
            BackfillBudget {
                page_size: 10,
                max_pages_per_scope: 1,
                max_events_per_run: 10,
                max_concurrency: 1,
                lease_secs: 60,
                retry_initial_ms: 10,
                retry_max_ms: 100,
            },
        );
        let outcome = use_case
            .run_one()
            .await
            .map_err(|error| format!("run multi-scope backfill failed: {error}"))?
            .ok_or_else(|| "expected multi-scope gap claim".to_owned())?;
        assert_eq!(outcome.gap_target_status, IngestionGapStatus::Uncertain);

        {
            let scopes = source.scopes.lock().unwrap();
            assert_eq!(scopes.len(), 2);
            assert_eq!(scopes[0].conversation, known);
            assert_eq!(
                scopes[0]
                    .boundary_cursor
                    .as_ref()
                    .map(|cursor| cursor.anchor.message_id.as_str()),
                Some("known-real-boundary"),
                "existing real cursor must not be overwritten by the sentinel"
            );
            assert_eq!(scopes[1].conversation, unseen);
            assert_eq!(
                scopes[1]
                    .boundary_cursor
                    .as_ref()
                    .map(|cursor| cursor.anchor.message_id.as_str()),
                Some("__non_message_history_signal_no_prior_cursor__")
            );
            assert!(
                scopes.iter().all(|scope| scope.conversation != unrelated),
                "an online gap must not scan an unrelated frozen conversation"
            );
        }
        assert_eq!(
            scalar_u64(
                &db,
                "SELECT COUNT(*) AS value FROM secretary_gap_signal_scopes WHERE gap_id = ?",
                vec![first_gap.as_str().into()],
            )
            .await,
            2
        );
        Ok(())
    })
    .await;
}
