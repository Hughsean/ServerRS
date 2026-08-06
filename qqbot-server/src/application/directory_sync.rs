//! B4 账号会话目录同步 Worker。
//!
//! 周期性运行协议无关的 `DirectorySyncUseCase`。目录来源和持久化实现由启动层注入。
//!
//! 关键约束：
//! - 不在每次 WebSocket 重连时无条件下载完整目录（TTL 内跳过）。
//! - 1 MiB 上限拒绝时保持 uncertain，不提高上限、不转空数组。
//! - single-flight：同一账号同一时间只有一个同步在运行。
//! - shutdown 可抢占，超时回收。
//! - 三个列表接口全部成功不等于账号历史完整。

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use personal_secretary::{DirectorySyncError, DirectorySyncUseCase, SourceAccountRef};
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tracing::{debug, info, warn};

use crate::config::DirectorySyncConfig;
use crate::worker_lifecycle::WorkerHandle;

/// 目录同步 Worker 句柄。
pub struct DirectorySyncHandle {
    shutdown: watch::Sender<bool>,
    join: JoinHandle<()>,
}

impl DirectorySyncHandle {
    /// 发出停止信号并取出 JoinHandle，交由 [`WorkerHandle`] 统一带超时回收。
    pub fn signal_and_detach(self) -> WorkerHandle {
        let _ = self.shutdown.send(true);
        WorkerHandle::new("directory_sync", self.join)
    }
}

/// 启动目录同步 Worker。
///
/// `use_case` 已绑定 `DirectorySourceT`（NapCat 适配器）和 `DirectoryStoreT`（MySQL 仓储）。
/// `account` 是被管理的 NapCat 账号；同一账号同一时间只有一个同步在运行（由调用方保证 single-flight）。
pub fn spawn_directory_sync_worker(
    use_case: Arc<DirectorySyncUseCase>,
    account: SourceAccountRef,
    config: DirectorySyncConfig,
) -> DirectorySyncHandle {
    let (shutdown, receiver) = watch::channel(false);
    let join = tokio::spawn(run_worker(use_case, account, config, receiver));
    DirectorySyncHandle { shutdown, join }
}

async fn run_worker(
    use_case: Arc<DirectorySyncUseCase>,
    account: SourceAccountRef,
    config: DirectorySyncConfig,
    mut shutdown: watch::Receiver<bool>,
) {
    info!(
        account_id = %account.account_id,
        scan_interval_ms = config.scan_interval_ms,
        snapshot_ttl_secs = config.snapshot_ttl_secs,
        "B4 目录同步 Worker 已启动"
    );

    let mut consecutive_errors = 0_u32;
    loop {
        if *shutdown.borrow() {
            info!("B4 目录同步 Worker 收到关闭信号，退出");
            return;
        }

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
            .min(i64::MAX as u64) as i64;

        // 整体 deadline 由外层 Worker 通过 timeout + shutdown select 实现；
        // 领域层 DirectorySyncUseCase 不依赖 Tokio。
        let deadline = Duration::from_secs(config.sync_deadline_secs.max(1));
        let sync_fut = use_case.sync_once(&account, now);
        tokio::pin!(sync_fut);

        let result = tokio::select! {
            biased;
            _ = shutdown.changed() => {
                if *shutdown.borrow() {
                    info!("B4 目录同步 Worker 在同步期间收到关闭信号，退出");
                    return;
                }
                // changed() 返回但 shutdown=false：继续等待同步完成或 deadline。
                match tokio::time::timeout(deadline, &mut sync_fut).await {
                    Ok(r) => r,
                    Err(_) => Err(DirectorySyncError::Timeout),
                }
            }
            timed = tokio::time::timeout(deadline, &mut sync_fut) => {
                match timed {
                    Ok(r) => r,
                    Err(_) => Err(DirectorySyncError::Timeout),
                }
            }
        };

        match result {
            Ok(snapshot) => {
                consecutive_errors = 0;
                debug!(
                    snapshot_id = snapshot.snapshot_id.as_str(),
                    status = snapshot.status.as_str(),
                    scope_count = snapshot.scopes.len(),
                    "目录同步完成（真实 NapCat 只能到达 known_scopes_complete）"
                );
            }
            Err(DirectorySyncError::Timeout | DirectorySyncError::SourceTimeout) => {
                consecutive_errors = consecutive_errors.saturating_add(1);
                warn!(
                    consecutive_errors,
                    "目录同步超时（整体 deadline 到期），保持 uncertain"
                );
            }
            Err(DirectorySyncError::Oversized) => {
                consecutive_errors = consecutive_errors.saturating_add(1);
                warn!(
                    consecutive_errors,
                    "目录同步被 1 MiB 上限拒绝，保持 uncertain（不提高上限、不转空数组）"
                );
            }
            Err(DirectorySyncError::Malformed) => {
                consecutive_errors = consecutive_errors.saturating_add(1);
                warn!(
                    consecutive_errors,
                    "目录同步收到 malformed DTO，保持 uncertain"
                );
            }
            Err(DirectorySyncError::Unavailable) => {
                consecutive_errors = consecutive_errors.saturating_add(1);
                warn!(
                    consecutive_errors,
                    "目录同步 API 不可用（retcode 非 0），映射为 unrecoverable"
                );
            }
            Err(DirectorySyncError::InvalidIdentity(e)) => {
                consecutive_errors = consecutive_errors.saturating_add(1);
                warn!(error = %e, consecutive_errors, "目录同步身份无效");
            }
            Err(DirectorySyncError::Store(e)) => {
                consecutive_errors = consecutive_errors.saturating_add(1);
                warn!(error = %e, consecutive_errors, "目录同步存储错误");
            }
        }

        // 退避延迟：成功时按 scan_interval_ms；失败时指数退避。
        let delay = if consecutive_errors == 0 {
            config.scan_interval_ms
        } else {
            config
                .retry_initial_ms
                .saturating_mul(2_u64.saturating_pow(consecutive_errors.saturating_sub(1)))
                .min(config.retry_max_ms)
        };

        tokio::select! {
            biased;
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    info!("B4 目录同步 Worker 在退避期间收到关闭信号，退出");
                    return;
                }
            }
            _ = tokio::time::sleep(Duration::from_millis(delay)) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use personal_secretary::{
        DirectoryListEntry, DirectorySnapshot, DirectorySourceError, DirectorySourceT,
        DirectoryStoreError, DirectoryStoreT, IngestionGapId, MessageSource, ScopeKind,
    };

    /// Fake 目录来源：返回预定义列表或错误，用于测试 Worker 生命周期。
    struct FakeDirectorySource {
        friends: Vec<DirectoryListEntry>,
        groups: Vec<DirectoryListEntry>,
        recent: Vec<DirectoryListEntry>,
    }

    #[async_trait]
    impl DirectorySourceT for FakeDirectorySource {
        async fn list_friends(
            &self,
            _account: &SourceAccountRef,
        ) -> Result<Vec<DirectoryListEntry>, DirectorySourceError> {
            Ok(self.friends.clone())
        }
        async fn list_groups(
            &self,
            _account: &SourceAccountRef,
        ) -> Result<Vec<DirectoryListEntry>, DirectorySourceError> {
            Ok(self.groups.clone())
        }
        async fn list_recent_contacts(
            &self,
            _account: &SourceAccountRef,
        ) -> Result<Vec<DirectoryListEntry>, DirectorySourceError> {
            Ok(self.recent.clone())
        }
    }

    /// 内存目录存储（复用领域层测试中的模式）。
    struct InMemoryDirectoryStore {
        snapshots: std::sync::Mutex<Vec<DirectorySnapshot>>,
    }

    impl InMemoryDirectoryStore {
        fn new() -> Self {
            Self {
                snapshots: std::sync::Mutex::new(Vec::new()),
            }
        }
    }

    #[async_trait]
    impl DirectoryStoreT for InMemoryDirectoryStore {
        async fn snapshot_directory(
            &self,
            snapshot: &DirectorySnapshot,
        ) -> Result<(), DirectoryStoreError> {
            let mut snapshots = self.snapshots.lock().unwrap();
            if !snapshots
                .iter()
                .any(|s| s.snapshot_id == snapshot.snapshot_id)
            {
                snapshots.push(snapshot.clone());
            }
            Ok(())
        }
        async fn load_latest_snapshot(
            &self,
            account: &SourceAccountRef,
        ) -> Result<Option<DirectorySnapshot>, DirectoryStoreError> {
            let snapshots = self.snapshots.lock().unwrap();
            Ok(snapshots
                .iter()
                .filter(|s| s.account.account_id == account.account_id)
                .max_by_key(|s| s.created_at_unix_secs)
                .cloned())
        }
        async fn freeze_for_gap(
            &self,
            _gap_id: &IngestionGapId,
            account: &SourceAccountRef,
        ) -> Result<Option<DirectorySnapshot>, DirectoryStoreError> {
            let snapshots = self.snapshots.lock().unwrap();
            Ok(snapshots
                .iter()
                .filter(|s| s.account.account_id == account.account_id)
                .max_by_key(|s| s.created_at_unix_secs)
                .cloned())
        }
        async fn load_frozen_for_gap(
            &self,
            _gap_id: &IngestionGapId,
        ) -> Result<Option<DirectorySnapshot>, DirectoryStoreError> {
            Ok(None)
        }
        async fn has_valid_snapshot(
            &self,
            account: &SourceAccountRef,
            ttl_secs: u64,
            now_unix_secs: i64,
        ) -> Result<bool, DirectoryStoreError> {
            let snapshots = self.snapshots.lock().unwrap();
            let latest = snapshots
                .iter()
                .filter(|s| s.account.account_id == account.account_id)
                .max_by_key(|s| s.created_at_unix_secs);
            if let Some(snap) = latest {
                return Ok(now_unix_secs - snap.created_at_unix_secs < ttl_secs as i64);
            }
            Ok(false)
        }
    }

    fn account() -> SourceAccountRef {
        SourceAccountRef::new(MessageSource::NapCat, "test-account".to_string()).unwrap()
    }

    #[tokio::test]
    async fn worker_runs_sync_and_shuts_down() {
        let source = Arc::new(FakeDirectorySource {
            friends: vec![DirectoryListEntry {
                platform_id: "10001".to_string(),
                display_name: Some("Alice".to_string()),
                boundary: None,
                kind_hint: ScopeKind::Friend,
            }],
            groups: vec![],
            recent: vec![],
        });
        let store = Arc::new(InMemoryDirectoryStore::new());
        let use_case = Arc::new(
            DirectorySyncUseCase::new(source, store, DirectorySyncConfig::default().budget())
                .unwrap(),
        );

        let handle = spawn_directory_sync_worker(
            use_case,
            account(),
            DirectorySyncConfig {
                scan_interval_ms: 60_000,
                ..DirectorySyncConfig::default()
            },
        );

        // 等待至少一次同步完成（给 2 秒）。
        tokio::time::sleep(Duration::from_secs(2)).await;

        // 关闭 Worker。
        tokio::time::timeout(
            Duration::from_secs(2),
            handle
                .signal_and_detach()
                .join_with_timeout(Duration::from_secs(2)),
        )
        .await
        .unwrap();
    }
}
