//! NapCat 连接健康观测与回补唤醒。
//!
//! [`ConnectionObserver`] 在 WebSocket 握手完成后通知宿主持久化连接状态并唤醒历史回补。
//! 重连成功不等于历史已补齐：仅唤醒回补 Worker 尽快扫描 uncertain Gap；Gap 是否转为
//! `verified_complete` 由回补用例的证据判定决定，不由重连决定。

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use personal_secretary::{ConnectionEpochId, PersonalSecretaryStoreT};
use qqbot::napcat::{NapCatConnectionObserver, NapCatError};

use crate::health_runtime::RuntimeHealthState;

/// 供 ConnectionObserver 和 runtime 共享的回补唤醒句柄。
/// 实际持有 `BackfillHandle` 的唤醒通知；避免在观察者中持有整个 JoinHandle。
pub(crate) struct BackfillWake {
    notify: Arc<tokio::sync::Notify>,
}

impl BackfillWake {
    pub(crate) fn new(notify: Arc<tokio::sync::Notify>) -> Self {
        Self { notify }
    }

    pub(crate) fn wake(&self) {
        self.notify.notify_one();
    }
}

pub(super) struct ConnectionObserver {
    store: Arc<dyn PersonalSecretaryStoreT>,
    connection_epoch_id: ConnectionEpochId,
    connected: AtomicBool,
    backfill_wake: Option<Arc<BackfillWake>>,
    health_state: Option<Arc<RuntimeHealthState>>,
}

impl ConnectionObserver {
    pub(super) fn new(
        store: Arc<dyn PersonalSecretaryStoreT>,
        connection_epoch_id: ConnectionEpochId,
        backfill_wake: Option<Arc<BackfillWake>>,
        health_state: Option<Arc<RuntimeHealthState>>,
    ) -> Self {
        Self {
            store,
            connection_epoch_id,
            connected: AtomicBool::new(false),
            backfill_wake,
            health_state,
        }
    }

    pub(super) fn was_connected(&self) -> bool {
        self.connected.load(Ordering::Acquire)
    }

    pub(super) fn mark_disconnected(&self) {
        self.connected.store(false, Ordering::Release);
        if let Some(health) = &self.health_state {
            health.set_websocket_connected(false);
        }
    }
}

#[async_trait::async_trait]
impl NapCatConnectionObserver for ConnectionObserver {
    async fn connected(&self) -> Result<(), NapCatError> {
        self.store
            .mark_connection_connected(&self.connection_epoch_id)
            .await
            .map_err(|error| NapCatError::Handler(error.to_string()))?;
        self.connected.store(true, Ordering::Release);
        if let Some(health) = &self.health_state {
            // WS connected 只更新 websocket 子系统；history_completeness 独立。
            health.set_websocket_connected(true);
        }
        // 重连成功不等于历史已补齐：仅唤醒回补 Worker 尽快扫描 uncertain Gap。
        // Gap 是否转为 verified_complete 由回补用例的证据判定决定，不由重连决定。
        if let Some(wake) = &self.backfill_wake {
            wake.wake();
        }
        Ok(())
    }
}
