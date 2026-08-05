//! GAP-003-A/B/C 历史多页持久化聚焦测试。
//!
//! 需要 `QQBOT_TEST_DATABASE_URL` 指向 `qqbot_accept_` 前缀的隔离 MySQL schema。
//! 默认 `#[ignore]`；每次运行派生随机 schema，无论成功、错误或 panic 都精确清理。

mod common;

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use common::{isolated_db, scalar_string};
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
    InboundMessageEnvelope::new(
        SourceMessageRef::new(MessageSource::NapCat, account_id, message_id).unwrap(),
        ConversationRef::new(ConversationKind::Group, "gap003-group").unwrap(),
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
