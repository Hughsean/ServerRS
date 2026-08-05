//! 普通消息实时 Spool 的协议无关契约。
//!
//! 这里不定义文件、加密、线程或数据库实现。`DurableSpoolReceipt` 仅是当前
//! 进程 writer 到 replay 的通知；进程重启时，恢复资格只能来自完整且已认证的
//! WAL 帧，绝不能依赖旧进程是否收到 receipt。

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{
    ArtifactId, ConnectionEpochId, ConnectionEpochStatus, InboundMessageEnvelope,
    IngestMessageOutcome, RecallCorrelationKey, SourceAccountRef,
};

macro_rules! spool_id {
    ($name:ident, $field:literal) => {
        #[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            pub fn new(value: impl Into<String>) -> Result<Self, RealtimeSpoolError> {
                let value = value.into();
                if value.trim().is_empty() {
                    return Err(RealtimeSpoolError::EmptyField($field));
                }
                Ok(Self(value))
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }
    };
}

spool_id!(RealtimeSpoolAdmissionId, "realtime_spool_admission_id");
spool_id!(RealtimeSpoolGenerationId, "realtime_spool_generation_id");
spool_id!(RealtimeSpoolRecordId, "realtime_spool_record_id");
spool_id!(
    RealtimeSpoolRecoveryLeaseToken,
    "realtime_spool_recovery_lease_token"
);

/// 已由 reader 接受、但尚未取得本地 durable receipt 的业务帧。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RealtimeSpoolAdmission {
    admission_id: RealtimeSpoolAdmissionId,
    connection_epoch_id: ConnectionEpochId,
    message: InboundMessageEnvelope,
}

impl RealtimeSpoolAdmission {
    pub fn new(
        admission_id: RealtimeSpoolAdmissionId,
        connection_epoch_id: ConnectionEpochId,
        message: InboundMessageEnvelope,
    ) -> Result<Self, RealtimeSpoolError> {
        message
            .validate()
            .map_err(|_| RealtimeSpoolError::InvalidMessage)?;
        if message.connection_epoch_id.as_ref() != Some(&connection_epoch_id) {
            return Err(RealtimeSpoolError::EpochMismatch);
        }
        Ok(Self {
            admission_id,
            connection_epoch_id,
            message,
        })
    }

    pub fn admission_id(&self) -> &RealtimeSpoolAdmissionId {
        &self.admission_id
    }

    pub fn connection_epoch_id(&self) -> &ConnectionEpochId {
        &self.connection_epoch_id
    }

    pub fn message(&self) -> &InboundMessageEnvelope {
        &self.message
    }
}

/// reader 的 admission 结果。普通业务拒绝可恢复；Spool 失效必须结束当前 epoch。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RealtimeSpoolAdmissionResult {
    Admitted(Box<RealtimeSpoolAdmission>),
    Recoverable(RealtimeSpoolRejection),
    Fatal(RealtimeSpoolFatal),
}

/// 不改变连接连续性的普通业务拒绝。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RealtimeSpoolRejection {
    InvalidBusinessFrame,
    UnsupportedBusinessFrame,
}

/// 必须传播到 connection loop 的脱敏 Spool 故障类别。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RealtimeSpoolFatalKind {
    KeyUnavailable,
    LockUnavailable,
    CapacityExhausted,
    AppendFailed,
    SyncFailed,
    WriterStopped,
    CheckpointFailed,
    RecoveryCorruptFrame,
    RecoveryInvariantViolation,
    ReconciliationFailed,
}

impl RealtimeSpoolFatalKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::KeyUnavailable => "key_unavailable",
            Self::LockUnavailable => "lock_unavailable",
            Self::CapacityExhausted => "capacity_exhausted",
            Self::AppendFailed => "append_failed",
            Self::SyncFailed => "sync_failed",
            Self::WriterStopped => "writer_stopped",
            Self::CheckpointFailed => "checkpoint_failed",
            Self::RecoveryCorruptFrame => "recovery_corrupt_frame",
            Self::RecoveryInvariantViolation => "recovery_invariant_violation",
            Self::ReconciliationFailed => "reconciliation_failed",
        }
    }
}

/// 不携带账号、消息锚点、路径或底层错误文本的 fatal 结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RealtimeSpoolFatal {
    pub kind: RealtimeSpoolFatalKind,
}

impl RealtimeSpoolFatal {
    pub const fn new(kind: RealtimeSpoolFatalKind) -> Self {
        Self { kind }
    }
}

/// 当前进程内 writer 成功同步单帧后发给 MySQL replay 的通知。
///
/// 该类型不能作为启动恢复资格：旧进程可以在同步成功、投递 receipt 前崩溃。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DurableSpoolReceipt {
    pub admission_id: RealtimeSpoolAdmissionId,
    pub generation_id: RealtimeSpoolGenerationId,
    pub record_id: RealtimeSpoolRecordId,
}

impl DurableSpoolReceipt {
    pub fn new(
        admission_id: RealtimeSpoolAdmissionId,
        generation_id: RealtimeSpoolGenerationId,
        record_id: RealtimeSpoolRecordId,
    ) -> Self {
        Self {
            admission_id,
            generation_id,
            record_id,
        }
    }
}

/// WAL 扫描确认可安全 replay 的一条完整帧。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveredRealtimeSpoolFrame {
    generation_id: RealtimeSpoolGenerationId,
    record_id: RealtimeSpoolRecordId,
    connection_epoch_id: ConnectionEpochId,
    message: InboundMessageEnvelope,
}

impl RecoveredRealtimeSpoolFrame {
    pub fn new(
        generation_id: RealtimeSpoolGenerationId,
        record_id: RealtimeSpoolRecordId,
        connection_epoch_id: ConnectionEpochId,
        message: InboundMessageEnvelope,
    ) -> Result<Self, RealtimeSpoolError> {
        message
            .validate()
            .map_err(|_| RealtimeSpoolError::InvalidMessage)?;
        if message.connection_epoch_id.as_ref() != Some(&connection_epoch_id) {
            return Err(RealtimeSpoolError::EpochMismatch);
        }
        Ok(Self {
            generation_id,
            record_id,
            connection_epoch_id,
            message,
        })
    }

    pub fn generation_id(&self) -> &RealtimeSpoolGenerationId {
        &self.generation_id
    }

    pub fn record_id(&self) -> &RealtimeSpoolRecordId {
        &self.record_id
    }

    pub fn connection_epoch_id(&self) -> &ConnectionEpochId {
        &self.connection_epoch_id
    }

    pub fn message(&self) -> &InboundMessageEnvelope {
        &self.message
    }
}

/// 文件适配器对一帧扫描的类型化结论。
///
/// 完整帧版本、认证或反序列化失败必须全局 fail-closed；仅最后一个未完成尾帧可截断。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RealtimeSpoolRecoveryFrame {
    Replay(Box<RecoveredRealtimeSpoolFrame>),
    TruncateIncompleteFinalTail,
    GlobalFailClosed(RealtimeSpoolFatal),
}

impl RealtimeSpoolRecoveryFrame {
    pub fn is_startup_replay_eligible(&self) -> bool {
        matches!(self, Self::Replay(_))
    }

    pub fn is_truncatable_final_tail(&self) -> bool {
        matches!(self, Self::TruncateIncompleteFinalTail)
    }
}

/// 配置为 checkpoint 前必须收敛的 post-hook 稳定键。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RealtimeSpoolHookKey {
    Recall(RecallCorrelationKey),
    Artifact(ArtifactId),
}

impl RealtimeSpoolHookKey {
    pub fn recall(correlation: RecallCorrelationKey) -> Self {
        Self::Recall(correlation)
    }

    pub fn artifact(artifact_id: ArtifactId) -> Self {
        Self::Artifact(artifact_id)
    }
}

/// 一条恢复帧的 MySQL replay 与 hook 收敛事实。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RealtimeSpoolReplayProgress {
    frame: RecoveredRealtimeSpoolFrame,
    ingestion: Option<IngestMessageOutcome>,
    required_hooks: Vec<RealtimeSpoolHookKey>,
    converged_hooks: Vec<RealtimeSpoolHookKey>,
}

impl RealtimeSpoolReplayProgress {
    pub fn pending(
        frame: RecoveredRealtimeSpoolFrame,
        required_hooks: impl IntoIterator<Item = RealtimeSpoolHookKey>,
    ) -> Self {
        Self {
            frame,
            ingestion: None,
            required_hooks: unique_hook_keys(required_hooks),
            converged_hooks: Vec::new(),
        }
    }

    pub fn with_ingestion(mut self, ingestion: IngestMessageOutcome) -> Self {
        self.ingestion = Some(ingestion);
        self
    }

    pub fn with_converged_hook(mut self, hook: RealtimeSpoolHookKey) -> Self {
        if !self.converged_hooks.contains(&hook) {
            self.converged_hooks.push(hook);
        }
        self
    }

    pub fn frame(&self) -> &RecoveredRealtimeSpoolFrame {
        &self.frame
    }

    pub fn ingestion(&self) -> Option<&IngestMessageOutcome> {
        self.ingestion.as_ref()
    }

    pub fn required_hooks(&self) -> &[RealtimeSpoolHookKey] {
        &self.required_hooks
    }

    pub fn converged_hooks(&self) -> &[RealtimeSpoolHookKey] {
        &self.converged_hooks
    }

    pub fn checkpoint_eligibility(&self) -> RealtimeSpoolCheckpointEligibility {
        if self.ingestion.is_none() {
            return RealtimeSpoolCheckpointEligibility::PendingMysqlCommit;
        }
        if self
            .required_hooks
            .iter()
            .any(|required| !self.converged_hooks.contains(required))
        {
            return RealtimeSpoolCheckpointEligibility::PendingHookConvergence;
        }
        RealtimeSpoolCheckpointEligibility::Eligible
    }
}

fn unique_hook_keys(
    hooks: impl IntoIterator<Item = RealtimeSpoolHookKey>,
) -> Vec<RealtimeSpoolHookKey> {
    let mut unique = Vec::new();
    for hook in hooks {
        if !unique.contains(&hook) {
            unique.push(hook);
        }
    }
    unique
}

/// 一个帧是否能被耐久 checkpoint 越过。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RealtimeSpoolCheckpointEligibility {
    Eligible,
    PendingMysqlCommit,
    PendingHookConvergence,
}

/// 按 WAL 自然记录顺序计算的连续 checkpoint 前缀。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RealtimeSpoolCheckpointPrefix {
    generation_id: RealtimeSpoolGenerationId,
    record_ids: Vec<RealtimeSpoolRecordId>,
}

impl RealtimeSpoolCheckpointPrefix {
    pub fn generation_id(&self) -> &RealtimeSpoolGenerationId {
        &self.generation_id
    }

    pub fn record_ids(&self) -> &[RealtimeSpoolRecordId] {
        &self.record_ids
    }
}

/// 不对 opaque record ID 排序；调用方必须按 WAL 扫描顺序传入。
pub fn checkpointable_prefix(
    generation_id: RealtimeSpoolGenerationId,
    progress: &[RealtimeSpoolReplayProgress],
) -> RealtimeSpoolCheckpointPrefix {
    let record_ids = progress
        .iter()
        .take_while(|entry| {
            entry.frame.generation_id() == &generation_id
                && entry.checkpoint_eligibility() == RealtimeSpoolCheckpointEligibility::Eligible
        })
        .map(|entry| entry.frame.record_id().clone())
        .collect();
    RealtimeSpoolCheckpointPrefix {
        generation_id,
        record_ids,
    }
}

/// 恢复时从连续性存储读取的未结束连接周期。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegacyRealtimeSpoolEpoch {
    pub connection_epoch_id: ConnectionEpochId,
    pub account: SourceAccountRef,
    pub status: ConnectionEpochStatus,
}

/// 连续性仓储原子领取的遗留 epoch。所有完成写入必须携带本对象并复验未过期令牌。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaimedLegacyRealtimeSpoolEpoch {
    epoch: LegacyRealtimeSpoolEpoch,
    lease_token: RealtimeSpoolRecoveryLeaseToken,
}

impl ClaimedLegacyRealtimeSpoolEpoch {
    pub fn new(
        epoch: LegacyRealtimeSpoolEpoch,
        lease_token: RealtimeSpoolRecoveryLeaseToken,
    ) -> Self {
        Self { epoch, lease_token }
    }

    pub fn epoch(&self) -> &LegacyRealtimeSpoolEpoch {
        &self.epoch
    }

    pub fn lease_token(&self) -> &RealtimeSpoolRecoveryLeaseToken {
        &self.lease_token
    }
}

/// 连接周期的分阶段恢复计划。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LegacyRealtimeSpoolRecoveryPlan {
    FinishConnectingWithoutFrames {
        claim: ClaimedLegacyRealtimeSpoolEpoch,
    },
    RecoverConnected {
        claim: ClaimedLegacyRealtimeSpoolEpoch,
        frames: Vec<RecoveredRealtimeSpoolFrame>,
    },
    GlobalFailClosed(RealtimeSpoolFatal),
}

/// `RecoverConnected` 的强制阶段。文件 checkpoint 后，才能提交 MySQL 的 epoch/Gap 原子收口。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectedEpochRecoveryStage {
    ReplayMysql,
    ConvergeRequiredHooks,
    DurableCheckpoint,
    FinalizeEpochAndUncertainGap,
}

impl LegacyRealtimeSpoolRecoveryPlan {
    pub fn for_claim(
        claim: ClaimedLegacyRealtimeSpoolEpoch,
        frames: Vec<RecoveredRealtimeSpoolFrame>,
    ) -> Self {
        let epoch = claim.epoch();
        let frames_belong_to_epoch = frames.iter().all(|frame| {
            frame.connection_epoch_id() == &epoch.connection_epoch_id
                && frame.message().source.account_ref() == epoch.account
        });
        if !frames_belong_to_epoch {
            return Self::GlobalFailClosed(RealtimeSpoolFatal::new(
                RealtimeSpoolFatalKind::RecoveryInvariantViolation,
            ));
        }

        match epoch.status {
            ConnectionEpochStatus::Connecting if frames.is_empty() => {
                Self::FinishConnectingWithoutFrames { claim }
            }
            ConnectionEpochStatus::Connecting => Self::GlobalFailClosed(RealtimeSpoolFatal::new(
                RealtimeSpoolFatalKind::RecoveryInvariantViolation,
            )),
            ConnectionEpochStatus::Connected => Self::RecoverConnected { claim, frames },
            ConnectionEpochStatus::Disconnected
            | ConnectionEpochStatus::Shutdown
            | ConnectionEpochStatus::ConnectFailed => Self::GlobalFailClosed(
                RealtimeSpoolFatal::new(RealtimeSpoolFatalKind::RecoveryInvariantViolation),
            ),
        }
    }

    pub fn connected_stages(&self) -> &'static [ConnectedEpochRecoveryStage] {
        match self {
            Self::RecoverConnected { .. } => &[
                ConnectedEpochRecoveryStage::ReplayMysql,
                ConnectedEpochRecoveryStage::ConvergeRequiredHooks,
                ConnectedEpochRecoveryStage::DurableCheckpoint,
                ConnectedEpochRecoveryStage::FinalizeEpochAndUncertainGap,
            ],
            Self::FinishConnectingWithoutFrames { .. } | Self::GlobalFailClosed(_) => &[],
        }
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum RealtimeSpoolError {
    #[error("{0} must not be empty")]
    EmptyField(&'static str),
    #[error("realtime spool message is invalid")]
    InvalidMessage,
    #[error("realtime spool epoch does not match message ownership")]
    EpochMismatch,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        ArtifactKind, ConversationKind, ConversationRef, MessageSource, SourceEventId,
        SourceMessageRef, VerifiedActor, VerifiedActorKind,
    };

    fn epoch() -> ConnectionEpochId {
        ConnectionEpochId::new("epoch-1").unwrap()
    }

    fn account() -> SourceAccountRef {
        SourceAccountRef::new(MessageSource::NapCat, "account-1").unwrap()
    }

    fn message(message_id: &str) -> InboundMessageEnvelope {
        InboundMessageEnvelope::new(
            SourceMessageRef::new(MessageSource::NapCat, "account-1", message_id).unwrap(),
            ConversationRef::new(ConversationKind::Group, "conversation-1").unwrap(),
            VerifiedActor::new(VerifiedActorKind::External, "actor-1").unwrap(),
            100,
            "message",
            Vec::new(),
        )
        .unwrap()
        .observed_in(epoch())
    }

    fn frame(record: &str) -> RecoveredRealtimeSpoolFrame {
        RecoveredRealtimeSpoolFrame::new(
            RealtimeSpoolGenerationId::new("generation-1").unwrap(),
            RealtimeSpoolRecordId::new(record).unwrap(),
            epoch(),
            message(record),
        )
        .unwrap()
    }

    fn accepted() -> IngestMessageOutcome {
        IngestMessageOutcome::Accepted {
            source_event_id: SourceEventId::new("event-1").unwrap(),
            reply_to_event_id: None,
        }
    }

    fn claimed_epoch(status: ConnectionEpochStatus) -> ClaimedLegacyRealtimeSpoolEpoch {
        ClaimedLegacyRealtimeSpoolEpoch::new(
            LegacyRealtimeSpoolEpoch {
                connection_epoch_id: epoch(),
                account: account(),
                status,
            },
            RealtimeSpoolRecoveryLeaseToken::new("lease-1").unwrap(),
        )
    }

    #[test]
    fn complete_recovered_frame_is_eligible_without_a_prior_process_receipt() {
        let recovered = RealtimeSpoolRecoveryFrame::Replay(Box::new(frame("record-1")));

        assert!(recovered.is_startup_replay_eligible());
        assert!(!recovered.is_truncatable_final_tail());
    }

    #[test]
    fn only_an_incomplete_final_tail_can_be_truncated() {
        let corrupt = RealtimeSpoolRecoveryFrame::GlobalFailClosed(RealtimeSpoolFatal::new(
            RealtimeSpoolFatalKind::RecoveryCorruptFrame,
        ));

        assert!(!corrupt.is_startup_replay_eligible());
        assert!(!corrupt.is_truncatable_final_tail());
        assert!(
            RealtimeSpoolRecoveryFrame::TruncateIncompleteFinalTail.is_truncatable_final_tail()
        );
    }

    #[test]
    fn checkpoint_waits_for_all_required_hook_effects_and_stops_at_first_gap() {
        let recall = RealtimeSpoolHookKey::recall(
            RecallCorrelationKey::new(
                account(),
                MessageSource::NapCat,
                ConversationRef::new(ConversationKind::Group, "conversation-1").unwrap(),
                "message-1",
            )
            .unwrap(),
        );
        let artifact = RealtimeSpoolHookKey::artifact(ArtifactId::for_source_segment(
            &SourceEventId::new("event-1").unwrap(),
            0,
            ArtifactKind::Image,
        ));
        let first = RealtimeSpoolReplayProgress::pending(frame("record-1"), [recall.clone()])
            .with_ingestion(accepted())
            .with_converged_hook(recall);
        let second = RealtimeSpoolReplayProgress::pending(frame("record-2"), [artifact])
            .with_ingestion(accepted());
        let third =
            RealtimeSpoolReplayProgress::pending(frame("record-3"), []).with_ingestion(accepted());

        assert_eq!(
            second.checkpoint_eligibility(),
            RealtimeSpoolCheckpointEligibility::PendingHookConvergence
        );
        assert_eq!(
            checkpointable_prefix(
                RealtimeSpoolGenerationId::new("generation-1").unwrap(),
                &[first, second, third],
            )
            .record_ids(),
            vec![RealtimeSpoolRecordId::new("record-1").unwrap()]
        );
    }

    #[test]
    fn recall_and_artifact_keys_are_stable_across_replay() {
        let recall_a = RecallCorrelationKey::new(
            account(),
            MessageSource::NapCat,
            ConversationRef::new(ConversationKind::Group, "conversation-1").unwrap(),
            "message-1",
        )
        .unwrap();
        let recall_b = recall_a.clone();
        let event = SourceEventId::new("event-1").unwrap();

        assert_eq!(recall_a.key_string(), recall_b.key_string());
        assert_eq!(
            ArtifactId::for_source_segment(&event, 2, ArtifactKind::File),
            ArtifactId::for_source_segment(&event, 2, ArtifactKind::File)
        );
    }

    #[test]
    fn connecting_epoch_requires_no_complete_message_frames() {
        let no_frame_plan = LegacyRealtimeSpoolRecoveryPlan::for_claim(
            claimed_epoch(ConnectionEpochStatus::Connecting),
            Vec::new(),
        );
        let frame_plan = LegacyRealtimeSpoolRecoveryPlan::for_claim(
            claimed_epoch(ConnectionEpochStatus::Connecting),
            vec![frame("record-1")],
        );

        assert!(matches!(
            no_frame_plan,
            LegacyRealtimeSpoolRecoveryPlan::FinishConnectingWithoutFrames { .. }
        ));
        assert!(matches!(
            frame_plan,
            LegacyRealtimeSpoolRecoveryPlan::GlobalFailClosed(_)
        ));
    }

    #[test]
    fn connected_epoch_has_fixed_replay_checkpoint_finalization_order() {
        let plan = LegacyRealtimeSpoolRecoveryPlan::for_claim(
            claimed_epoch(ConnectionEpochStatus::Connected),
            vec![frame("record-1")],
        );

        assert_eq!(
            plan.connected_stages(),
            &[
                ConnectedEpochRecoveryStage::ReplayMysql,
                ConnectedEpochRecoveryStage::ConvergeRequiredHooks,
                ConnectedEpochRecoveryStage::DurableCheckpoint,
                ConnectedEpochRecoveryStage::FinalizeEpochAndUncertainGap,
            ]
        );
    }

    #[test]
    fn recovery_plan_preserves_the_fenced_claim_for_finalization() {
        let claim = claimed_epoch(ConnectionEpochStatus::Connected);
        let expected_token = claim.lease_token().clone();
        let plan = LegacyRealtimeSpoolRecoveryPlan::for_claim(claim, vec![frame("record-1")]);

        match plan {
            LegacyRealtimeSpoolRecoveryPlan::RecoverConnected { claim, .. } => {
                assert_eq!(claim.epoch().account, account());
                assert_eq!(claim.lease_token(), &expected_token);
            }
            other => panic!("expected connected recovery plan, got {other:?}"),
        }
    }
}
