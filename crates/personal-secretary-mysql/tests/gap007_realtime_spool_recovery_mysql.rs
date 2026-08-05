//! GAP-007-IMPL-C real MySQL recovery lease, fencing and epoch finalization.

mod common;

use personal_secretary::{
    InboundEventStoreError, MessageSource, RealtimeSpoolRecoveryLeaseToken, SourceAccountRef,
};
use personal_secretary_mysql::{
    build_mysql_inbound_event_store, build_mysql_realtime_spool_recovery_store,
};
use sea_orm::{ConnectionTrait, DatabaseBackend, Statement};

#[tokio::test]
#[ignore = "requires QQBOT_TEST_DATABASE_URL pointing to an isolated qqbot_accept_* schema"]
async fn recovery_claims_are_account_scoped_fenced_and_finalize_epochs_atomically() {
    let (db, schema) = common::isolated_db("gap007").await;
    let test_db = db.clone();
    let task = tokio::spawn(async move {
        let inbound = build_mysql_inbound_event_store(test_db.clone());
        let recovery = build_mysql_realtime_spool_recovery_store(test_db.clone(), 60);
        let account_a = SourceAccountRef::new(MessageSource::NapCat, "account-a").unwrap();
        let account_b = SourceAccountRef::new(MessageSource::NapCat, "account-b").unwrap();

        let connecting = inbound.begin_connection(&account_a).await.unwrap();
        let connected = inbound.begin_connection(&account_a).await.unwrap();
        inbound.mark_connection_connected(&connected).await.unwrap();
        let other = inbound.begin_connection(&account_b).await.unwrap();

        let claims = recovery
            .claim_legacy_realtime_spool_epochs(&account_a)
            .await
            .unwrap();
        assert_eq!(claims.len(), 2);
        assert!(
            claims
                .iter()
                .all(|claim| claim.epoch().account == account_a)
        );
        assert!(
            claims
                .iter()
                .all(|claim| claim.epoch().connection_epoch_id != other)
        );

        let connecting_claim = claims
            .iter()
            .find(|claim| claim.epoch().connection_epoch_id == connecting)
            .unwrap()
            .clone();
        let stale = personal_secretary::ClaimedLegacyRealtimeSpoolEpoch::new(
            connecting_claim.epoch().clone(),
            RealtimeSpoolRecoveryLeaseToken::new("00000000-0000-0000-0000-000000000000").unwrap(),
        );
        assert!(matches!(
            recovery
                .finish_legacy_connecting_without_frames(&stale)
                .await,
            Err(InboundEventStoreError::LeaseLost)
        ));
        recovery
            .finish_legacy_connecting_without_frames(&connecting_claim)
            .await
            .unwrap();

        let connected_claim = claims
            .iter()
            .find(|claim| claim.epoch().connection_epoch_id == connected)
            .unwrap();
        let gap = recovery
            .finalize_recovered_connected_epoch(connected_claim)
            .await
            .unwrap();

        let rows = test_db
            .query_all_raw(Statement::from_sql_and_values(
                DatabaseBackend::MySql,
                "SELECT connection_epoch_id, status, end_reason FROM secretary_connection_epochs \
                 WHERE connection_epoch_id IN (?, ?) ORDER BY connection_epoch_id",
                vec![connecting.as_str().into(), connected.as_str().into()],
            ))
            .await
            .unwrap();
        assert_eq!(rows.len(), 2);
        let statuses = rows
            .iter()
            .map(|row| {
                (
                    row.try_get::<String>("", "status").unwrap(),
                    row.try_get::<String>("", "end_reason").unwrap(),
                )
            })
            .collect::<Vec<_>>();
        assert!(statuses.contains(&(
            "connect_failed".into(),
            "spool_recovery_connect_failed".into()
        )));
        assert!(statuses.contains(&("disconnected".into(), "spool_recovery".into())));

        assert_eq!(
            common::scalar_u64(
                &test_db,
                "SELECT COUNT(*) AS value FROM secretary_ingestion_gaps g \
                 INNER JOIN secretary_directory_gap_freeze f ON f.gap_id = g.gap_id \
                 WHERE g.gap_id = ? AND g.status = 'uncertain'",
                vec![gap.as_str().into()],
            )
            .await,
            1
        );
        assert_eq!(
            common::scalar_u64(
                &test_db,
                "SELECT COUNT(*) AS value FROM secretary_realtime_spool_recovery_claims",
                vec![],
            )
            .await,
            0
        );
        let other_claim = recovery
            .claim_legacy_realtime_spool_epochs(&account_b)
            .await
            .unwrap()
            .pop()
            .unwrap();
        test_db
            .execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::MySql,
                "UPDATE secretary_realtime_spool_recovery_claims \
                 SET lease_expires_at = NOW(6) - INTERVAL 1 SECOND \
                 WHERE connection_epoch_id = ?",
                vec![other_claim.epoch().connection_epoch_id.as_str().into()],
            ))
            .await
            .unwrap();
        assert!(matches!(
            recovery
                .renew_legacy_realtime_spool_epoch(&other_claim)
                .await,
            Err(InboundEventStoreError::LeaseLost)
        ));
        let replacement = recovery
            .claim_legacy_realtime_spool_epochs(&account_b)
            .await
            .unwrap()
            .pop()
            .unwrap();
        assert_ne!(replacement.lease_token(), other_claim.lease_token());
        recovery
            .finish_legacy_connecting_without_frames(&replacement)
            .await
            .unwrap();

        common::try_apply_qqbot_migrations(&test_db)
            .await
            .expect("migration replay must remain idempotent");
    });

    let result = task.await;
    common::drop_schema(&db, &schema).await;
    result.unwrap();
}
