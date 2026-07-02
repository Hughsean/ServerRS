use std::sync::Arc;

use tokio::time::{Duration, interval};
use tracing::{error, info, warn};

use crate::domain::qq_bot::repository::{OutboxEntry, OutboxRepository};
use crate::domain::qq_bot::{GroupMessageGateway, QqBotError};

/// Background worker that polls the outbox table and sends pending messages.
///
/// Implements a reliable outbox pattern:
/// 1. Poll for due entries (pending status, next_run_at <= now)
/// 2. Mark as Sending
/// 3. Send via the message gateway
/// 4. Mark as Sent (success) or Failed (retry later)
pub struct OutboxWorker {
    outbox_repo: Arc<dyn OutboxRepository>,
    message_gateway: Option<Arc<dyn GroupMessageGateway>>,
    poll_interval_secs: u64,
    batch_size: u32,
}

impl OutboxWorker {
    pub fn new(
        outbox_repo: Arc<dyn OutboxRepository>,
        message_gateway: Option<Arc<dyn GroupMessageGateway>>,
        poll_interval_secs: u64,
        batch_size: u32,
    ) -> Self {
        Self {
            outbox_repo,
            message_gateway,
            poll_interval_secs,
            batch_size,
        }
    }

    /// Start the outbox worker loop. Runs forever (until cancelled).
    pub async fn run(self: Arc<Self>) {
        let mut ticker = interval(Duration::from_secs(self.poll_interval_secs));
        info!(
            interval_secs = self.poll_interval_secs,
            batch_size = self.batch_size,
            "outbox worker started"
        );

        loop {
            ticker.tick().await;

            if let Err(e) = self.process_batch().await {
                warn!(error = %e, "发件箱 Worker 批次处理失败");
            }
        }
    }

    /// Process a single batch of due outbox entries.
    async fn process_batch(&self) -> Result<(), QqBotError> {
        let entries = self
            .outbox_repo
            .fetch_due(self.batch_size)
            .await
            .map_err(|e| QqBotError::Internal(format!("outbox fetch_due failed: {e}")))?;

        if entries.is_empty() {
            return Ok(());
        }

        info!(count = entries.len(), "outbox worker processing batch");

        for entry in &entries {
            if let Err(e) = self.process_entry(entry).await {
                warn!(
                    outbox_id = ?entry.outbox_id,
                    error = %e,
                    "outbox entry processing failed"
                );

                let outbox_id = entry.outbox_id.unwrap_or(0);

                if entry.attempts + 1 >= entry.max_attempts {
                    if let Err(inner) = self
                        .outbox_repo
                        .mark_failed(outbox_id, &e.to_string())
                        .await
                    {
                        error!(error = %inner, "failed to mark outbox entry as failed");
                    }
                } else {
                    let next_run_at = now_ms() + retry_delay_ms(entry.attempts);
                    if let Err(inner) = self
                        .outbox_repo
                        .mark_retry(outbox_id, &e.to_string(), next_run_at)
                        .await
                    {
                        error!(error = %inner, "failed to mark outbox entry for retry");
                    }
                }
            }
        }

        Ok(())
    }

    /// Process a single outbox entry: send via the message gateway.
    async fn process_entry(&self, entry: &OutboxEntry) -> Result<(), QqBotError> {
        let api = self
            .message_gateway
            .as_ref()
            .ok_or_else(|| QqBotError::Internal("QQ message gateway not configured".into()))?;

        let outbox_id = entry.outbox_id.unwrap_or(0);

        // Extract parameters from payload
        let group_id = entry
            .qq_group_id
            .ok_or_else(|| QqBotError::Internal("outbox entry missing group_id".into()))?;
        let message = entry
            .payload
            .get("message")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                QqBotError::Internal("outbox entry missing message in payload".into())
            })?;

        // Send via the configured message gateway.
        let platform_id = api.send_group_msg(group_id, message).await?;

        // Mark as sent
        let platform_id = platform_id.unwrap_or_default();
        self.outbox_repo
            .mark_sent(outbox_id, &platform_id)
            .await
            .map_err(|e| QqBotError::Internal(format!("failed to mark outbox as sent: {e}")))?;

        info!(
            outbox_id,
            group_id,
            platform_message_id = %platform_id,
            "outbox entry sent successfully"
        );

        Ok(())
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

fn retry_delay_ms(attempts_before_failure: u32) -> i64 {
    let multiplier = 1_i64 << attempts_before_failure.min(6);
    (5_000 * multiplier).min(60_000)
}
