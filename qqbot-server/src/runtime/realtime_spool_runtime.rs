//! Runtime bridge between the non-blocking NapCat reader, blocking durable Spool and MySQL replay.

use std::sync::Arc;
use std::sync::mpsc as std_mpsc;

use personal_secretary::{
    ConnectionEpochId, ConnectionEpochStatus, InboundMessageEnvelope,
    LegacyRealtimeSpoolRecoveryPlan, PersonalSecretaryStoreT, RealtimeSpoolAdmission,
    RealtimeSpoolAdmissionId, RealtimeSpoolFatal, RealtimeSpoolFatalKind,
    RealtimeSpoolRecoveryStoreT, RealtimeSpoolReplayProgress, RecoveredRealtimeSpoolFrame,
    SourceAccountRef, checkpointable_prefix,
};
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::ingestion_worker::{IngestionQueue, fire_post_hooks, required_hook_keys};
use crate::realtime_spool::RealtimeMessageSpool;

struct RealtimeSpoolCheckpointAdapter(Arc<RealtimeMessageSpool>);

#[async_trait::async_trait]
impl crate::ingestion_worker::RealtimeSpoolCheckpointT for RealtimeSpoolCheckpointAdapter {
    async fn advance_checkpoint(
        &self,
        prefix: personal_secretary::RealtimeSpoolCheckpointPrefix,
    ) -> Result<(), RealtimeSpoolFatal> {
        let spool = Arc::clone(&self.0);
        tokio::task::spawn_blocking(move || spool.advance_checkpoint(&prefix))
            .await
            .map_err(|_| RealtimeSpoolFatal::new(RealtimeSpoolFatalKind::WriterStopped))?
            .map_err(|error| RealtimeSpoolFatal::new(error.kind))
    }
}

pub(super) fn checkpoint_adapter(
    spool: Arc<RealtimeMessageSpool>,
) -> Arc<dyn crate::ingestion_worker::RealtimeSpoolCheckpointT> {
    Arc::new(RealtimeSpoolCheckpointAdapter(spool))
}

#[derive(Debug, thiserror::Error, Clone, Copy, PartialEq, Eq)]
pub enum RealtimeSpoolAdmissionError {
    #[error("realtime spool admission queue is full")]
    Full,
    #[error("realtime spool writer is closed")]
    Closed,
    #[error("realtime spool admission is invalid")]
    Invalid,
}

#[derive(Clone)]
pub struct RealtimeSpoolAdmissionQueue {
    sender: std_mpsc::SyncSender<RealtimeSpoolAdmission>,
    connection_epoch_id: ConnectionEpochId,
    fatal_sender: mpsc::UnboundedSender<RealtimeSpoolFatal>,
}

impl RealtimeSpoolAdmissionQueue {
    pub fn try_admit(
        &self,
        message: InboundMessageEnvelope,
    ) -> Result<(), RealtimeSpoolAdmissionError> {
        let message = message.observed_in(self.connection_epoch_id.clone());
        let admission_id = RealtimeSpoolAdmissionId::new(Uuid::new_v4().to_string())
            .map_err(|_| RealtimeSpoolAdmissionError::Invalid)?;
        let admission =
            RealtimeSpoolAdmission::new(admission_id, self.connection_epoch_id.clone(), message)
                .map_err(|_| RealtimeSpoolAdmissionError::Invalid)?;
        match self.sender.try_send(admission) {
            Ok(()) => Ok(()),
            Err(std_mpsc::TrySendError::Full(_)) => {
                let _ = self.fatal_sender.send(RealtimeSpoolFatal::new(
                    RealtimeSpoolFatalKind::CapacityExhausted,
                ));
                Err(RealtimeSpoolAdmissionError::Full)
            }
            Err(std_mpsc::TrySendError::Disconnected(_)) => {
                let _ = self.fatal_sender.send(RealtimeSpoolFatal::new(
                    RealtimeSpoolFatalKind::WriterStopped,
                ));
                Err(RealtimeSpoolAdmissionError::Closed)
            }
        }
    }
}

#[derive(Debug, Default)]
pub struct RealtimeSpoolWriterReport {
    pub durable_receipts: u64,
}

pub struct RealtimeSpoolWriterHandle {
    completion: tokio::sync::oneshot::Receiver<RealtimeSpoolWriterReport>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl RealtimeSpoolWriterHandle {
    pub async fn wait(&mut self) -> Result<RealtimeSpoolWriterReport, ()> {
        let report = (&mut self.completion).await.map_err(|_| ())?;
        if let Some(thread) = self.thread.take() {
            thread.join().map_err(|_| ())?;
        }
        Ok(report)
    }

    pub fn detach(&mut self) {
        self.thread.take();
    }
}

pub fn spawn_realtime_spool_writer(
    spool: Arc<RealtimeMessageSpool>,
    ingestion: IngestionQueue,
    connection_epoch_id: ConnectionEpochId,
    admission_capacity: usize,
    fatal_sender: mpsc::UnboundedSender<RealtimeSpoolFatal>,
) -> (RealtimeSpoolAdmissionQueue, RealtimeSpoolWriterHandle) {
    let (sender, receiver) = std_mpsc::sync_channel(admission_capacity);
    let queue = RealtimeSpoolAdmissionQueue {
        sender,
        connection_epoch_id,
        fatal_sender: fatal_sender.clone(),
    };
    let (completion_sender, completion) = tokio::sync::oneshot::channel();
    let writer_fatal_sender = fatal_sender.clone();
    let thread = std::thread::Builder::new()
        .name("qqbot-realtime-spool-writer".into())
        .spawn(move || {
            let report = run_writer(spool, ingestion, receiver, writer_fatal_sender);
            let _ = completion_sender.send(report);
        });
    let thread = match thread {
        Ok(thread) => Some(thread),
        Err(_) => {
            let _ = fatal_sender.send(RealtimeSpoolFatal::new(
                RealtimeSpoolFatalKind::WriterStopped,
            ));
            None
        }
    };
    (queue, RealtimeSpoolWriterHandle { completion, thread })
}

fn run_writer(
    spool: Arc<RealtimeMessageSpool>,
    ingestion: IngestionQueue,
    receiver: std_mpsc::Receiver<RealtimeSpoolAdmission>,
    fatal_sender: mpsc::UnboundedSender<RealtimeSpoolFatal>,
) -> RealtimeSpoolWriterReport {
    let mut report = RealtimeSpoolWriterReport::default();
    while let Ok(admission) = receiver.recv() {
        let receipt = match spool.append(&admission) {
            Ok(receipt) => receipt,
            Err(error) => {
                spool.telemetry().mark_fatal(error.kind);
                let _ = fatal_sender.send(RealtimeSpoolFatal::new(error.kind));
                break;
            }
        };
        if receipt.admission_id != *admission.admission_id() {
            spool
                .telemetry()
                .mark_fatal(RealtimeSpoolFatalKind::RecoveryInvariantViolation);
            let _ = fatal_sender.send(RealtimeSpoolFatal::new(
                RealtimeSpoolFatalKind::RecoveryInvariantViolation,
            ));
            break;
        }
        let frame = match RecoveredRealtimeSpoolFrame::new(
            receipt.generation_id,
            receipt.record_id,
            admission.connection_epoch_id().clone(),
            admission.message().clone(),
        ) {
            Ok(frame) => frame,
            Err(_) => {
                spool
                    .telemetry()
                    .mark_fatal(RealtimeSpoolFatalKind::RecoveryInvariantViolation);
                let _ = fatal_sender.send(RealtimeSpoolFatal::new(
                    RealtimeSpoolFatalKind::RecoveryInvariantViolation,
                ));
                break;
            }
        };
        if ingestion.blocking_enqueue_spooled(frame).is_err() {
            spool
                .telemetry()
                .mark_fatal(RealtimeSpoolFatalKind::WriterStopped);
            let _ = fatal_sender.send(RealtimeSpoolFatal::new(
                RealtimeSpoolFatalKind::WriterStopped,
            ));
            break;
        }
        report.durable_receipts = report.durable_receipts.saturating_add(1);
    }
    report
}

#[allow(clippy::too_many_arguments)]
pub async fn recover_realtime_spool_before_connect(
    spool: Arc<RealtimeMessageSpool>,
    account: &SourceAccountRef,
    recovery_store: Arc<dyn RealtimeSpoolRecoveryStoreT>,
    ingestion_store: Arc<dyn PersonalSecretaryStoreT>,
    recall_use_case: Option<&Arc<personal_secretary::RecallUseCase>>,
    artifact_use_case: Option<&Arc<personal_secretary::ArtifactUseCase>>,
    artifact_default_ttl_secs: u64,
) -> Result<(), RealtimeSpoolFatal> {
    spool.telemetry().set_reconciliation_pending(true);
    let result = recover_realtime_spool_before_connect_inner(
        Arc::clone(&spool),
        account,
        recovery_store,
        ingestion_store,
        recall_use_case,
        artifact_use_case,
        artifact_default_ttl_secs,
    )
    .await;
    spool.telemetry().set_reconciliation_pending(false);
    if let Err(fatal) = result {
        spool.telemetry().mark_fatal(fatal.kind);
        return Err(fatal);
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn recover_realtime_spool_before_connect_inner(
    spool: Arc<RealtimeMessageSpool>,
    account: &SourceAccountRef,
    recovery_store: Arc<dyn RealtimeSpoolRecoveryStoreT>,
    ingestion_store: Arc<dyn PersonalSecretaryStoreT>,
    recall_use_case: Option<&Arc<personal_secretary::RecallUseCase>>,
    artifact_use_case: Option<&Arc<personal_secretary::ArtifactUseCase>>,
    artifact_default_ttl_secs: u64,
) -> Result<(), RealtimeSpoolFatal> {
    let recovery_spool = Arc::clone(&spool);
    let frames = tokio::task::spawn_blocking(move || recovery_spool.recover_pending())
        .await
        .map_err(|_| RealtimeSpoolFatal::new(RealtimeSpoolFatalKind::WriterStopped))?
        .map_err(|error| RealtimeSpoolFatal::new(error.kind))?;
    let mut claims = Vec::new();
    loop {
        let claimed = recovery_store
            .claim_legacy_realtime_spool_epochs(account)
            .await
            .map_err(|_| RealtimeSpoolFatal::new(RealtimeSpoolFatalKind::ReconciliationFailed))?;
        if claimed.is_empty() {
            break;
        }
        claims.extend(claimed);
    }

    for frame in &frames {
        if !claims.iter().any(|claim| {
            claim.epoch().connection_epoch_id == *frame.connection_epoch_id()
                && claim.epoch().account == *account
        }) {
            return Err(RealtimeSpoolFatal::new(
                RealtimeSpoolFatalKind::RecoveryInvariantViolation,
            ));
        }
    }

    for claim in &claims {
        let owned_frames = frames
            .iter()
            .filter(|frame| frame.connection_epoch_id() == &claim.epoch().connection_epoch_id)
            .cloned()
            .collect::<Vec<_>>();
        if matches!(
            LegacyRealtimeSpoolRecoveryPlan::for_claim(claim.clone(), owned_frames),
            LegacyRealtimeSpoolRecoveryPlan::GlobalFailClosed(_)
        ) {
            return Err(RealtimeSpoolFatal::new(
                RealtimeSpoolFatalKind::RecoveryInvariantViolation,
            ));
        }
    }

    for frame in &frames {
        let claim = claims
            .iter()
            .find(|claim| claim.epoch().connection_epoch_id == *frame.connection_epoch_id())
            .ok_or_else(|| {
                RealtimeSpoolFatal::new(RealtimeSpoolFatalKind::RecoveryInvariantViolation)
            })?;
        recovery_store
            .renew_legacy_realtime_spool_epoch(claim)
            .await
            .map_err(|_| RealtimeSpoolFatal::new(RealtimeSpoolFatalKind::ReconciliationFailed))?;
        let outcomes = ingestion_store
            .insert_messages_if_absent(&[frame.message().clone()])
            .await
            .map_err(|_| RealtimeSpoolFatal::new(RealtimeSpoolFatalKind::ReconciliationFailed))?;
        let outcome = outcomes.first().ok_or_else(|| {
            RealtimeSpoolFatal::new(RealtimeSpoolFatalKind::RecoveryInvariantViolation)
        })?;
        let hooks = required_hook_keys(
            frame.message(),
            outcome,
            recall_use_case.is_some(),
            artifact_use_case.is_some(),
        )?;
        fire_post_hooks(
            outcome,
            frame.message(),
            recall_use_case,
            artifact_use_case,
            artifact_default_ttl_secs,
        )
        .await
        .map_err(|_| RealtimeSpoolFatal::new(RealtimeSpoolFatalKind::ReconciliationFailed))?;
        let mut progress = RealtimeSpoolReplayProgress::pending(frame.clone(), hooks.clone())
            .with_ingestion(outcome.clone());
        for hook in hooks {
            progress = progress.with_converged_hook(hook);
        }
        let prefix = checkpointable_prefix(frame.generation_id().clone(), &[progress]);
        let checkpoint_spool = Arc::clone(&spool);
        tokio::task::spawn_blocking(move || checkpoint_spool.advance_checkpoint(&prefix))
            .await
            .map_err(|_| RealtimeSpoolFatal::new(RealtimeSpoolFatalKind::WriterStopped))?
            .map_err(|error| RealtimeSpoolFatal::new(error.kind))?;
    }

    for claim in claims {
        recovery_store
            .renew_legacy_realtime_spool_epoch(&claim)
            .await
            .map_err(|_| RealtimeSpoolFatal::new(RealtimeSpoolFatalKind::ReconciliationFailed))?;
        match claim.epoch().status {
            ConnectionEpochStatus::Connecting => recovery_store
                .finish_legacy_connecting_without_frames(&claim)
                .await
                .map_err(|_| {
                    RealtimeSpoolFatal::new(RealtimeSpoolFatalKind::ReconciliationFailed)
                })?,
            ConnectionEpochStatus::Connected => {
                recovery_store
                    .finalize_recovered_connected_epoch(&claim)
                    .await
                    .map_err(|_| {
                        RealtimeSpoolFatal::new(RealtimeSpoolFatalKind::ReconciliationFailed)
                    })?;
            }
            _ => {
                return Err(RealtimeSpoolFatal::new(
                    RealtimeSpoolFatalKind::RecoveryInvariantViolation,
                ));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::Mutex;

    use async_trait::async_trait;
    use base64::Engine;
    use personal_secretary::{
        ClaimedLegacyRealtimeSpoolEpoch, ConnectionEndReason, ConnectionEpochStatus,
        ConversationKind, ConversationRef, InboundEventStoreError, InboundEventStoreT,
        InboundMessageEnvelope, IngestMessageOutcome, IngestionContinuityStoreT, IngestionGapId,
        IngestionGapReason, LegacyRealtimeSpoolEpoch, MessageSource,
        RealtimeSpoolRecoveryLeaseToken, SourceEventId, SourceMessageRef, VerifiedActor,
        VerifiedActorKind,
    };
    use tempfile::TempDir;

    use super::*;

    fn message(id: &str) -> InboundMessageEnvelope {
        InboundMessageEnvelope::new(
            SourceMessageRef::new(MessageSource::NapCat, "account-1", id).unwrap(),
            ConversationRef::new(ConversationKind::Group, "conversation-1").unwrap(),
            VerifiedActor::new(VerifiedActorKind::External, "actor-1").unwrap(),
            100,
            "message",
            Vec::new(),
        )
        .unwrap()
    }

    fn open_spool(temp: &TempDir) -> Arc<RealtimeMessageSpool> {
        open_spool_with_delay(temp, std::time::Duration::ZERO)
    }

    fn open_spool_with_delay(
        temp: &TempDir,
        append_delay: std::time::Duration,
    ) -> Arc<RealtimeMessageSpool> {
        let key_env = format!("QQBOT_TEST_SPOOL_KEY_{}", Uuid::new_v4().simple());
        unsafe {
            std::env::set_var(
                &key_env,
                base64::engine::general_purpose::STANDARD.encode([7_u8; 32]),
            );
        }
        let mut config = crate::realtime_spool::RealtimeMessageSpoolConfig::new(
            temp.path().join("message.wal"),
            temp.path().join("message.checkpoint"),
            temp.path().join("quarantine"),
            key_env,
        );
        config.append_delay = append_delay;
        Arc::new(RealtimeMessageSpool::open(config).unwrap().spool)
    }

    fn account() -> SourceAccountRef {
        SourceAccountRef::new(MessageSource::NapCat, "account-1").unwrap()
    }

    fn claim(status: ConnectionEpochStatus) -> ClaimedLegacyRealtimeSpoolEpoch {
        ClaimedLegacyRealtimeSpoolEpoch::new(
            LegacyRealtimeSpoolEpoch {
                connection_epoch_id: ConnectionEpochId::new("epoch-1").unwrap(),
                account: account(),
                status,
            },
            RealtimeSpoolRecoveryLeaseToken::new("lease-1").unwrap(),
        )
    }

    struct FakeIngestionStore {
        inserted: Mutex<u64>,
        unavailable: bool,
    }

    impl Default for FakeIngestionStore {
        fn default() -> Self {
            Self {
                inserted: Mutex::new(0),
                unavailable: false,
            }
        }
    }

    #[async_trait]
    impl InboundEventStoreT for FakeIngestionStore {
        async fn insert_message_if_absent(
            &self,
            _message: &InboundMessageEnvelope,
        ) -> Result<IngestMessageOutcome, InboundEventStoreError> {
            *self.inserted.lock().unwrap() += 1;
            if self.unavailable {
                return Err(InboundEventStoreError::Unavailable);
            }
            Ok(IngestMessageOutcome::Accepted {
                source_event_id: SourceEventId::new("source-event-1").unwrap(),
                reply_to_event_id: None,
            })
        }
    }

    #[async_trait]
    impl IngestionContinuityStoreT for FakeIngestionStore {
        async fn begin_connection(
            &self,
            _account: &SourceAccountRef,
        ) -> Result<ConnectionEpochId, InboundEventStoreError> {
            Err(InboundEventStoreError::Unavailable)
        }

        async fn mark_connection_connected(
            &self,
            _connection_epoch_id: &ConnectionEpochId,
        ) -> Result<(), InboundEventStoreError> {
            Err(InboundEventStoreError::Unavailable)
        }

        async fn finish_connection(
            &self,
            _connection_epoch_id: &ConnectionEpochId,
            _reason: ConnectionEndReason,
        ) -> Result<Option<IngestionGapId>, InboundEventStoreError> {
            Err(InboundEventStoreError::Unavailable)
        }

        async fn mark_connection_uncertain(
            &self,
            _connection_epoch_id: &ConnectionEpochId,
            _reason: IngestionGapReason,
        ) -> Result<IngestionGapId, InboundEventStoreError> {
            Err(InboundEventStoreError::Unavailable)
        }
    }

    struct FakeRecoveryStore {
        claims: Mutex<VecDeque<Vec<ClaimedLegacyRealtimeSpoolEpoch>>>,
        renewed: Mutex<u64>,
        finished_connecting: Mutex<u64>,
        finalized_connected: Mutex<u64>,
    }

    impl FakeRecoveryStore {
        fn new(claim: ClaimedLegacyRealtimeSpoolEpoch) -> Self {
            Self {
                claims: Mutex::new(VecDeque::from([vec![claim], Vec::new()])),
                renewed: Mutex::new(0),
                finished_connecting: Mutex::new(0),
                finalized_connected: Mutex::new(0),
            }
        }
    }

    #[async_trait]
    impl RealtimeSpoolRecoveryStoreT for FakeRecoveryStore {
        async fn claim_legacy_realtime_spool_epochs(
            &self,
            _account: &SourceAccountRef,
        ) -> Result<Vec<ClaimedLegacyRealtimeSpoolEpoch>, InboundEventStoreError> {
            Ok(self.claims.lock().unwrap().pop_front().unwrap_or_default())
        }

        async fn finish_legacy_connecting_without_frames(
            &self,
            _claimed: &ClaimedLegacyRealtimeSpoolEpoch,
        ) -> Result<(), InboundEventStoreError> {
            *self.finished_connecting.lock().unwrap() += 1;
            Ok(())
        }

        async fn renew_legacy_realtime_spool_epoch(
            &self,
            _claimed: &ClaimedLegacyRealtimeSpoolEpoch,
        ) -> Result<(), InboundEventStoreError> {
            *self.renewed.lock().unwrap() += 1;
            Ok(())
        }

        async fn finalize_recovered_connected_epoch(
            &self,
            _claimed: &ClaimedLegacyRealtimeSpoolEpoch,
        ) -> Result<IngestionGapId, InboundEventStoreError> {
            *self.finalized_connected.lock().unwrap() += 1;
            Ok(IngestionGapId::new("gap-1").unwrap())
        }
    }

    #[tokio::test]
    async fn admission_full_is_fatal_without_waiting_for_writer() {
        let (sender, _receiver) = std_mpsc::sync_channel(1);
        let (fatal_sender, mut fatal_receiver) = mpsc::unbounded_channel();
        let queue = RealtimeSpoolAdmissionQueue {
            sender,
            connection_epoch_id: ConnectionEpochId::new("epoch-1").unwrap(),
            fatal_sender,
        };
        queue.try_admit(message("message-1")).unwrap();
        assert_eq!(
            queue.try_admit(message("message-2")),
            Err(RealtimeSpoolAdmissionError::Full)
        );
        assert_eq!(
            fatal_receiver.recv().await.unwrap().kind,
            RealtimeSpoolFatalKind::CapacityExhausted
        );
    }

    #[tokio::test]
    async fn writer_syncs_frame_before_reporting_closed_ingestion() {
        let temp = TempDir::new().unwrap();
        let spool = open_spool(&temp);
        let (fatal_sender, mut fatal_receiver) = mpsc::unbounded_channel();
        let (queue, mut worker) = spawn_realtime_spool_writer(
            Arc::clone(&spool),
            IngestionQueue::for_test(),
            ConnectionEpochId::new("epoch-1").unwrap(),
            2,
            fatal_sender,
        );
        queue.try_admit(message("message-1")).unwrap();
        drop(queue);
        worker.wait().await.unwrap();
        assert_eq!(
            fatal_receiver.recv().await.unwrap().kind,
            RealtimeSpoolFatalKind::WriterStopped
        );
        assert_eq!(spool.recover_pending().unwrap().len(), 1);
        let snapshot = spool.telemetry().snapshot();
        assert_eq!(snapshot.pending_frames, 1);
        assert!(!snapshot.usable);
    }

    #[tokio::test]
    async fn startup_replays_connected_epoch_before_checkpoint_and_finalization() {
        let temp = TempDir::new().unwrap();
        let spool = open_spool(&temp);
        let epoch = ConnectionEpochId::new("epoch-1").unwrap();
        let admission = RealtimeSpoolAdmission::new(
            RealtimeSpoolAdmissionId::new("admission-1").unwrap(),
            epoch.clone(),
            message("message-1").observed_in(epoch),
        )
        .unwrap();
        spool.append(&admission).unwrap();
        let recovery = Arc::new(FakeRecoveryStore::new(claim(
            ConnectionEpochStatus::Connected,
        )));
        let ingestion = Arc::new(FakeIngestionStore::default());

        recover_realtime_spool_before_connect(
            Arc::clone(&spool),
            &account(),
            recovery.clone(),
            ingestion.clone(),
            None,
            None,
            0,
        )
        .await
        .unwrap();

        assert_eq!(*ingestion.inserted.lock().unwrap(), 1);
        assert_eq!(*recovery.finalized_connected.lock().unwrap(), 1);
        assert!(spool.recover_pending().unwrap().is_empty());
    }

    #[tokio::test]
    async fn connecting_epoch_with_complete_frame_fails_closed() {
        let temp = TempDir::new().unwrap();
        let spool = open_spool(&temp);
        let epoch = ConnectionEpochId::new("epoch-1").unwrap();
        let admission = RealtimeSpoolAdmission::new(
            RealtimeSpoolAdmissionId::new("admission-1").unwrap(),
            epoch.clone(),
            message("message-1").observed_in(epoch),
        )
        .unwrap();
        spool.append(&admission).unwrap();
        let recovery = Arc::new(FakeRecoveryStore::new(claim(
            ConnectionEpochStatus::Connecting,
        )));
        let ingestion = Arc::new(FakeIngestionStore::default());

        let fatal = recover_realtime_spool_before_connect(
            Arc::clone(&spool),
            &account(),
            recovery.clone(),
            ingestion.clone(),
            None,
            None,
            0,
        )
        .await
        .unwrap_err();

        assert_eq!(
            fatal.kind,
            RealtimeSpoolFatalKind::RecoveryInvariantViolation
        );
        assert_eq!(*ingestion.inserted.lock().unwrap(), 0);
        assert_eq!(*recovery.finished_connecting.lock().unwrap(), 0);
        assert_eq!(spool.recover_pending().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn checkpoint_before_epoch_finalization_converges_on_next_start() {
        let temp = TempDir::new().unwrap();
        let spool = open_spool(&temp);
        let recovery = Arc::new(FakeRecoveryStore::new(claim(
            ConnectionEpochStatus::Connected,
        )));
        let ingestion = Arc::new(FakeIngestionStore::default());

        recover_realtime_spool_before_connect(
            spool,
            &account(),
            recovery.clone(),
            ingestion.clone(),
            None,
            None,
            0,
        )
        .await
        .unwrap();

        assert_eq!(*ingestion.inserted.lock().unwrap(), 0);
        assert_eq!(*recovery.finalized_connected.lock().unwrap(), 1);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn blocked_writer_does_not_starve_runtime_timers() {
        let temp = TempDir::new().unwrap();
        let spool = open_spool_with_delay(&temp, std::time::Duration::from_millis(250));
        let (fatal_sender, _fatal_receiver) = mpsc::unbounded_channel();
        let (queue, mut worker) = spawn_realtime_spool_writer(
            spool,
            IngestionQueue::for_test(),
            ConnectionEpochId::new("epoch-1").unwrap(),
            2,
            fatal_sender,
        );
        queue.try_admit(message("message-1")).unwrap();
        let timer = tokio::time::timeout(
            std::time::Duration::from_millis(80),
            tokio::time::sleep(std::time::Duration::from_millis(20)),
        )
        .await;
        assert!(
            timer.is_ok(),
            "blocking writer must not starve runtime timers"
        );
        drop(queue);
        worker.wait().await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn shutdown_deadline_detaches_writer_and_retains_synced_wal() {
        let temp = TempDir::new().unwrap();
        let spool = open_spool_with_delay(&temp, std::time::Duration::from_millis(150));
        let (fatal_sender, _fatal_receiver) = mpsc::unbounded_channel();
        let (queue, mut worker) = spawn_realtime_spool_writer(
            Arc::clone(&spool),
            IngestionQueue::for_test(),
            ConnectionEpochId::new("epoch-1").unwrap(),
            2,
            fatal_sender,
        );
        queue.try_admit(message("message-1")).unwrap();
        drop(queue);
        assert!(
            !crate::runtime::connection_loop::drain_spool_writer(
                &mut worker,
                std::time::Duration::from_millis(20),
            )
            .await
        );
        tokio::time::sleep(std::time::Duration::from_millis(220)).await;
        assert_eq!(spool.recover_pending().unwrap().len(), 1);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn mysql_unavailable_never_advances_checkpoint() {
        let temp = TempDir::new().unwrap();
        let spool = open_spool(&temp);
        let store = Arc::new(FakeIngestionStore {
            inserted: Mutex::new(0),
            unavailable: true,
        });
        let epoch = ConnectionEpochId::new("epoch-1").unwrap();
        let config = crate::config::IngestionConfig {
            queue_capacity: 4,
            batch_size: 1,
            batch_flush_ms: 1,
            retry_initial_ms: 1,
            retry_max_ms: 2,
            ..Default::default()
        };
        let (fatal_sender, _fatal_receiver) = mpsc::unbounded_channel();
        let (ingestion, ingestion_worker) = crate::ingestion_worker::spawn_spooled_ingestion_worker(
            store.clone(),
            epoch.clone(),
            config,
            None,
            None,
            0,
            None,
            None,
            checkpoint_adapter(Arc::clone(&spool)),
            fatal_sender.clone(),
        );
        let (queue, mut writer) =
            spawn_realtime_spool_writer(Arc::clone(&spool), ingestion, epoch, 2, fatal_sender);
        queue.try_admit(message("message-1")).unwrap();
        drop(queue);
        writer.wait().await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
        assert!(*store.inserted.lock().unwrap() > 0);
        ingestion_worker.abort();
        let _ = ingestion_worker.await;
        assert_eq!(spool.recover_pending().unwrap().len(), 1);
    }
}
