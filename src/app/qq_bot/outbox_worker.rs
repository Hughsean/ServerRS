use std::sync::Arc;

use tokio::time::{interval, Duration};
use tracing::{error, info, warn};

use crate::domain::qq_bot::repository::{OutboxEntry, OutboxRepository};
use crate::domain::qq_bot::QqBotError;
use crate::infra::qq_bot::napcat::api::NapCatApiClient;

/// Background worker that polls the outbox table and sends pending messages.
///
/// Implements a reliable outbox pattern:
/// 1. Poll for due entries (pending status, next_run_at <= now)
/// 2. Mark as Sending
/// 3. Send via NapCat API
/// 4. Mark as Sent (success) or Failed (retry later)
pub struct OutboxWorker {
    outbox_repo: Arc<dyn OutboxRepository>,
    napcat_api: Option<Arc<NapCatApiClient>>,
    poll_interval_secs: u64,
    batch_size: u32,
}

impl OutboxWorker {
    pub fn new(
        outbox_repo: Arc<dyn OutboxRepository>,
        napcat_api: Option<Arc<NapCatApiClient>>,
        poll_interval_secs: u64,
        batch_size: u32,
    ) -> Self {
        Self {
            outbox_repo,
            napcat_api,
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
                warn!(error = %e, "outbox worker batch processing failed");
            }
        }
    }

    /// Process a single batch of due outbox entries.
    async fn process_batch(&self) -> Result<(), QqBotError> {
        let entries = self.outbox_repo.fetch_due(self.batch_size).await.map_err(|e| {
            QqBotError::Internal(format!("outbox fetch_due failed: {e}"))
        })?;

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

                // Mark as failed after max attempts
                if entry.attempts + 1 >= entry.max_attempts {
                    if let Err(inner) = self.outbox_repo
                        .mark_failed(entry.outbox_id.unwrap_or(0), &e.to_string())
                        .await
                    {
                        error!(error = %inner, "failed to mark outbox entry as failed");
                    }
                }
            }
        }

        Ok(())
    }

    /// Process a single outbox entry: send via NapCat API.
    async fn process_entry(&self, entry: &OutboxEntry) -> Result<(), QqBotError> {
        let api = self.napcat_api.as_ref().ok_or_else(|| {
            QqBotError::Internal("NapCat API client not configured".into())
        })?;

        let outbox_id = entry.outbox_id.unwrap_or(0);

        // Extract parameters from payload
        let group_id = entry.qq_group_id.ok_or_else(|| {
            QqBotError::Internal("outbox entry missing group_id".into())
        })?;
        let message = entry.payload.get("message")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                QqBotError::Internal("outbox entry missing message in payload".into())
            })?;

        // Send via NapCat API
        let response = api.send_group_msg(group_id, message).await?;

        // Mark as sent
        let platform_id = response.message_id.unwrap_or_default();
        self.outbox_repo.mark_sent(outbox_id, &platform_id).await.map_err(|e| {
            QqBotError::Internal(format!("failed to mark outbox as sent: {e}"))
        })?;

        info!(
            outbox_id,
            group_id,
            platform_message_id = %platform_id,
            "outbox entry sent successfully"
        );

        Ok(())
    }
}
