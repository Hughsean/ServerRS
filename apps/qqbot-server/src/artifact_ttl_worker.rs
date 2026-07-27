//! B6 Artifact TTL 周期 Worker：把到期信封标记为 expired。
//!
//! 不下载、不写 URL 到日志；只做有界 DB 更新。

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use personal_secretary::ArtifactUseCase;
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tracing::{debug, info, warn};

use crate::config::ArtifactConfig;
use crate::worker_lifecycle::WorkerHandle;

pub struct ArtifactTtlHandle {
    shutdown: watch::Sender<bool>,
    join: JoinHandle<()>,
}

impl ArtifactTtlHandle {
    pub fn signal_and_detach(self) -> WorkerHandle {
        let _ = self.shutdown.send(true);
        WorkerHandle::new("artifact_ttl", self.join)
    }
}

pub fn spawn_artifact_ttl_worker(
    use_case: Arc<ArtifactUseCase>,
    config: ArtifactConfig,
) -> ArtifactTtlHandle {
    let (shutdown, receiver) = watch::channel(false);
    let join = tokio::spawn(run_worker(use_case, config, receiver));
    ArtifactTtlHandle { shutdown, join }
}

async fn run_worker(
    use_case: Arc<ArtifactUseCase>,
    config: ArtifactConfig,
    mut shutdown: watch::Receiver<bool>,
) {
    info!(
        scan_interval_ms = config.ttl_scan_interval_ms,
        "B6 Artifact TTL Worker 已启动"
    );
    let mut consecutive_errors = 0_u32;
    loop {
        if *shutdown.borrow() {
            info!("B6 Artifact TTL Worker 收到关闭信号，退出");
            return;
        }
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
            .min(i64::MAX as u64) as i64;
        let derivation = use_case.derive_pending(config.default_ttl_secs, 50).await;
        let expiration = use_case.expire_due(now).await;
        match (derivation, expiration) {
            (Ok(derived), Ok(expired)) => {
                consecutive_errors = 0;
                if derived > 0 || expired > 0 {
                    debug!(derived, expired, "Artifact 派生与 TTL 扫描完成");
                }
            }
            (derivation, expiration) => {
                consecutive_errors = consecutive_errors.saturating_add(1);
                warn!(
                    derivation_error = derivation.err().map(|error| error.to_string()),
                    expiration_error = expiration.err().map(|error| error.to_string()),
                    consecutive_errors,
                    "Artifact 派生或 TTL 扫描失败"
                );
            }
        }

        let delay_ms = if consecutive_errors == 0 {
            config.ttl_scan_interval_ms
        } else {
            let factor = 1_u64 << consecutive_errors.min(5);
            (config.ttl_scan_interval_ms.saturating_mul(factor)).min(300_000)
        };
        tokio::select! {
            _ = shutdown.changed() => {
                if *shutdown.borrow() {
                    info!("B6 Artifact TTL Worker 在等待期间收到关闭信号，退出");
                    return;
                }
            }
            _ = tokio::time::sleep(Duration::from_millis(delay_ms)) => {}
        }
    }
}
