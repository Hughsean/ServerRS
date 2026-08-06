//! 实时普通消息 Spool 的 application 端口。
//!
//! 此模块不实现 WAL 或数据库。文件适配器在 IMPL-B 提供完整认证帧与 durable
//! checkpoint；MySQL 适配器在后续切片实现遗留连接周期的数据库内原子收口。

use async_trait::async_trait;

use crate::{
    ClaimedLegacyRealtimeSpoolEpoch, InboundEventStoreError, IngestionGapId, SourceAccountRef,
};

/// 遗留 epoch 的启动恢复端口。
///
/// 所有方法都只能承诺数据库内 epoch 状态、Gap 创建/复用及证据冻结的原子性。调用方必须
/// 在调用 `finalize_recovered_connected_epoch` 前完成 WAL replay、必需 hook 效果收敛和耐久
/// checkpoint；本端口绝不把文件 checkpoint 与数据库事务宣称为跨资源原子操作。
#[async_trait]
pub trait RealtimeSpoolRecoveryStoreT: Send + Sync {
    /// 单实例持有该账号的 Spool 锁后，原子领取需要在任何新连接建立前收口的遗留周期。
    /// 每次领取必须轮换租约令牌；实现必须保证结果全部属于 `account`。
    async fn claim_legacy_realtime_spool_epochs(
        &self,
        account: &SourceAccountRef,
    ) -> Result<Vec<ClaimedLegacyRealtimeSpoolEpoch>, InboundEventStoreError>;

    /// `connecting` 且没有任何归属完整帧的周期，以连接失败方式原子结束，不创建消息 Gap。
    /// 实现必须在事务内复验账号、epoch、租约令牌与租约未过期。
    async fn finish_legacy_connecting_without_frames(
        &self,
        claimed: &ClaimedLegacyRealtimeSpoolEpoch,
    ) -> Result<(), InboundEventStoreError>;

    /// 延长同一 fencing token 的租约。实现必须复验 token 尚未过期及账号/epoch 归属；
    /// 过期 token 不得复活。
    async fn renew_legacy_realtime_spool_epoch(
        &self,
        claimed: &ClaimedLegacyRealtimeSpoolEpoch,
    ) -> Result<(), InboundEventStoreError>;

    /// `connected` 周期完成 replay、hook 收敛和 durable checkpoint 后，在一个数据库事务中结束
    /// 该周期、创建或复用 uncertain Gap，并冻结该 Gap 的证据。实现必须在事务内复验账号、
    /// epoch、租约令牌与租约未过期；过期或错误令牌必须 fail-closed。
    async fn finalize_recovered_connected_epoch(
        &self,
        claimed: &ClaimedLegacyRealtimeSpoolEpoch,
    ) -> Result<IngestionGapId, InboundEventStoreError>;
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use crate::{
        ConnectionEpochId, ConnectionEpochStatus, LegacyRealtimeSpoolEpoch, MessageSource,
        RealtimeSpoolRecoveryLeaseToken,
    };

    struct RecordingStore {
        claim: ClaimedLegacyRealtimeSpoolEpoch,
        claimed_account: Mutex<Option<SourceAccountRef>>,
        finalized_claim: Mutex<Option<ClaimedLegacyRealtimeSpoolEpoch>>,
    }

    #[async_trait]
    impl RealtimeSpoolRecoveryStoreT for RecordingStore {
        async fn claim_legacy_realtime_spool_epochs(
            &self,
            account: &SourceAccountRef,
        ) -> Result<Vec<ClaimedLegacyRealtimeSpoolEpoch>, InboundEventStoreError> {
            *self.claimed_account.lock().unwrap() = Some(account.clone());
            Ok(vec![self.claim.clone()])
        }

        async fn finish_legacy_connecting_without_frames(
            &self,
            claimed: &ClaimedLegacyRealtimeSpoolEpoch,
        ) -> Result<(), InboundEventStoreError> {
            *self.finalized_claim.lock().unwrap() = Some(claimed.clone());
            Ok(())
        }

        async fn renew_legacy_realtime_spool_epoch(
            &self,
            claimed: &ClaimedLegacyRealtimeSpoolEpoch,
        ) -> Result<(), InboundEventStoreError> {
            *self.finalized_claim.lock().unwrap() = Some(claimed.clone());
            Ok(())
        }

        async fn finalize_recovered_connected_epoch(
            &self,
            claimed: &ClaimedLegacyRealtimeSpoolEpoch,
        ) -> Result<IngestionGapId, InboundEventStoreError> {
            *self.finalized_claim.lock().unwrap() = Some(claimed.clone());
            Ok(IngestionGapId::new("gap-1").unwrap())
        }
    }

    #[tokio::test]
    async fn recovery_port_carries_account_scope_and_fenced_claim_to_finalization() {
        let account = SourceAccountRef::new(MessageSource::NapCat, "account-1").unwrap();
        let claim = ClaimedLegacyRealtimeSpoolEpoch::new(
            LegacyRealtimeSpoolEpoch {
                connection_epoch_id: ConnectionEpochId::new("epoch-1").unwrap(),
                account: account.clone(),
                status: ConnectionEpochStatus::Connected,
            },
            RealtimeSpoolRecoveryLeaseToken::new("lease-1").unwrap(),
        );
        let store = RecordingStore {
            claim: claim.clone(),
            claimed_account: Mutex::new(None),
            finalized_claim: Mutex::new(None),
        };

        let claimed = store
            .claim_legacy_realtime_spool_epochs(&account)
            .await
            .unwrap();
        store
            .finalize_recovered_connected_epoch(&claimed[0])
            .await
            .unwrap();

        assert_eq!(*store.claimed_account.lock().unwrap(), Some(account));
        assert_eq!(store.finalized_claim.lock().unwrap().as_ref(), Some(&claim));
    }
}
