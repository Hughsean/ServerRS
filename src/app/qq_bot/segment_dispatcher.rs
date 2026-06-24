use std::path::PathBuf;
use std::sync::Arc;

use serde_json;
use tokio::time::{Duration, sleep};
use tracing::{error, info, warn};

use crate::domain::qq_bot::QqBotError;
use crate::domain::qq_bot::reply::{BotReply, ReplySegment};
use crate::domain::qq_bot::repository::{OutboxEntry, OutboxRepository, OutboxStatus};
use crate::domain::tts::{TtsProvider, TtsRequest};
use crate::infra::qq_bot::napcat::api::NapCatApiClient;

/// Dispatches multi-segment replies to the target group with human-like timing.
///
/// Two modes:
/// 1. **Direct send** — Sends segments one-by-one with configured delays via NapCat API
/// 2. **Outbox enqueue** — Writes segments to outbox for reliable delivery by OutboxWorker
///
/// Special segment types:
/// - `Poke` — Calls the dedicated `group_poke` API directly (not via send_group_msg)
/// - `Record` — Synthesises TTS, writes audio to local file, then sends as CQ:record
pub struct SegmentDispatcher {
    napcat_api: Option<Arc<NapCatApiClient>>,
    outbox_repo: Arc<dyn OutboxRepository>,
    bot_account_id: u64,
    tts_provider: Option<Arc<dyn TtsProvider>>,
    tts_output_dir: PathBuf,
    tts_public_url_base: String,
}

impl SegmentDispatcher {
    pub fn new(
        napcat_api: Option<Arc<NapCatApiClient>>,
        outbox_repo: Arc<dyn OutboxRepository>,
        bot_account_id: u64,
        tts_provider: Option<Arc<dyn TtsProvider>>,
        tts_output_dir: PathBuf,
        tts_public_url_base: String,
    ) -> Self {
        Self {
            napcat_api,
            outbox_repo,
            bot_account_id,
            tts_provider,
            tts_output_dir,
            tts_public_url_base,
        }
    }

    pub fn outbox_repo(&self) -> Arc<dyn OutboxRepository> {
        Arc::clone(&self.outbox_repo)
    }

    /// Send a BotReply to the group using the direct NapCat HTTP API.
    ///
    /// Sends segments one-by-one with configured delays.
    /// Special segments:
    /// - `Poke` → calls `group_poke` API directly (not via send_group_msg, no delay)
    /// - `Record` → TTS-synthesises, writes audio file, sends as CQ:record
    ///
    /// On success, returns the last segment's platform message id (if available).
    pub async fn send_direct(
        &self,
        group_id: i64,
        reply: &BotReply,
        related_turn_id: Option<u64>,
    ) -> Result<Vec<String>, QqBotError> {
        let api = self
            .napcat_api
            .as_ref()
            .ok_or_else(|| QqBotError::Internal("NapCat API client not configured".into()))?;

        let mut sent_ids = Vec::new();

        // Initial "thinking" delay
        if reply.timing_hint.initial_delay_ms > 0 {
            sleep(Duration::from_millis(reply.timing_hint.initial_delay_ms)).await;
        }

        for (i, segment) in reply.segments.iter().enumerate() {
            let result = match segment {
                // ── Poke: direct API call, no message id ────────────────
                ReplySegment::Poke { user_id, .. } => {
                    info!(group_id, target_user = user_id, "正在发送群戳一戳");
                    match api.group_poke(group_id, *user_id).await {
                        Ok(_) => {
                            info!(group_id, "戳一戳已发送");
                            Ok(None)
                        }
                        Err(e) => {
                            error!(group_id, error = %e, "发送戳一戳失败");
                            Err(e)
                        }
                    }
                }
                // ── Record: TTS → audio file → CQ:record → send_group_msg ─
                ReplySegment::Record { text, voice } => {
                    match self.synthesize_record(text, voice).await {
                        Ok(cq_string) => match api.send_group_msg(group_id, &cq_string).await {
                            Ok(data) => {
                                info!(group_id, segment = i, "语音消息已发送");
                                Ok(data.message_id)
                            }
                            Err(e) => {
                                error!(group_id, error = %e, "发送语音消息失败");
                                Err(e)
                            }
                        },
                        Err(e) => {
                            error!(group_id, error = %e, "TTS 语音合成失败");
                            Err(e)
                        }
                    }
                }
                // ── Regular segments: existing behavior ─────────────────
                _ => {
                    let message_str = segment_to_onebot_string(segment);
                    match api.send_group_msg(group_id, &message_str).await {
                        Ok(data) => {
                            info!(group_id, segment = i, "消息段已通过 NapCat API 发送");
                            Ok(data.message_id)
                        }
                        Err(e) => {
                            error!(group_id, segment = i, error = %e, "通过 NapCat API 发送消息段失败");
                            Err(e)
                        }
                    }
                }
            };

            match result {
                Ok(Some(pid)) => {
                    sent_ids.push(pid);
                }
                Ok(None) => {
                    // Poke or other action without a message id — nothing to push
                }
                Err(e) => {
                    // Enqueue remaining segments to outbox for retry
                    for remaining in &reply.segments[i..] {
                        if let Err(inner) = self
                            .enqueue_segment(group_id, None, remaining, related_turn_id)
                            .await
                        {
                            warn!(error = %inner, "将剩余消息段加入队列失败");
                        }
                    }
                    return Err(e);
                }
            }

            // Inter-segment delay (after all but the last segment)
            if i < reply.segments.len().saturating_sub(1) {
                let delay = reply
                    .timing_hint
                    .inter_segment_delays_ms
                    .get(i)
                    .copied()
                    .unwrap_or(800);
                if delay > 0 {
                    sleep(Duration::from_millis(delay)).await;
                }
            }
        }

        Ok(sent_ids)
    }

    /// Enqueue a single reply segment to the outbox for reliable delivery.
    ///
    /// Special handling:
    /// - `Poke` segments are sent immediately via the direct API (not enqueued).
    /// - `Record` segments are TTS-synthesised first, then the resulting CQ code is enqueued.
    pub async fn enqueue_segment(
        &self,
        group_id: i64,
        user_id: Option<i64>,
        segment: &ReplySegment,
        related_turn_id: Option<u64>,
    ) -> Result<OutboxEntry, QqBotError> {
        // ── Poke: send immediately, return a no-op entry ───────────────
        if let ReplySegment::Poke {
            user_id: target_user,
            ..
        } = segment
        {
            if let Some(api) = &self.napcat_api {
                if let Err(e) = api.group_poke(group_id, *target_user).await {
                    warn!(group_id, target_user, error = %e, "poke enqueue: direct send failed");
                } else {
                    info!(group_id, target_user, "poke enqueue: sent directly");
                }
            }
            // Return a dummy entry that is already marked as Sent so the worker ignores it.
            return Ok(OutboxEntry {
                outbox_id: None,
                bot_account_id: self.bot_account_id,
                qq_group_id: Some(group_id),
                qq_user_id: user_id,
                target_type: "group".into(),
                payload: serde_json::json!({"action": "group_poke", "group_id": group_id, "user_id": target_user}),
                related_turn_id,
                status: OutboxStatus::Sent,
                attempts: 1,
                max_attempts: 1,
                next_run_at: 0,
                platform_message_id: None,
                last_error: None,
            });
        }

        // ── Record: TTS-synthesise first, then enqueue as CQ:record ───
        let message_str = if let ReplySegment::Record { text, voice } = segment {
            self.synthesize_record(text, voice).await?
        } else {
            segment_to_onebot_string(segment)
        };

        let entry = OutboxEntry {
            outbox_id: None,
            bot_account_id: self.bot_account_id,
            qq_group_id: Some(group_id),
            qq_user_id: user_id,
            target_type: "group".into(),
            payload: serde_json::json!({
                "group_id": group_id,
                "message": message_str,
                "auto_escape": false,
            }),
            related_turn_id,
            status: OutboxStatus::Pending,
            attempts: 0,
            max_attempts: 3,
            next_run_at: chrono::Utc::now().timestamp_millis(),
            platform_message_id: None,
            last_error: None,
        };

        let persisted =
            self.outbox_repo.insert(&entry).await.map_err(|e| {
                QqBotError::Internal(format!("failed to enqueue outbox entry: {e}"))
            })?;

        info!(
            outbox_id = ?persisted.outbox_id,
            group_id,
            "segment enqueued to outbox"
        );

        Ok(persisted)
    }

    /// Enqueue an entire BotReply to the outbox (one entry per segment).
    ///
    /// Poke segments are sent directly and skipped in the outbox.
    /// Record segments are TTS-synthesised before enqueuing.
    pub async fn enqueue_reply(
        &self,
        group_id: i64,
        reply: &BotReply,
        related_turn_id: Option<u64>,
    ) -> Result<Vec<OutboxEntry>, QqBotError> {
        let mut entries = Vec::new();
        for segment in &reply.segments {
            let entry = self
                .enqueue_segment(group_id, None, segment, related_turn_id)
                .await?;
            entries.push(entry);

            // If enqueuing to outbox, we still add delays so segments aren't all sent at once
            let delay = reply
                .timing_hint
                .inter_segment_delays_ms
                .first()
                .copied()
                .unwrap_or(800);
            if delay > 0 {
                sleep(Duration::from_millis(delay)).await;
            }
        }
        Ok(entries)
    }

    /// Synthesise TTS audio for a Record segment, write it to disk, and return
    /// a CQ:record string pointing to the public URL.
    async fn synthesize_record(&self, text: &str, voice: &str) -> Result<String, QqBotError> {
        let provider = self.tts_provider.as_ref().ok_or_else(|| {
            QqBotError::Internal("TTS provider not configured for Record segment".into())
        })?;

        let request = TtsRequest::new(text, voice);
        let response = provider
            .synthesize(request)
            .await
            .map_err(|e| QqBotError::Internal(format!("TTS synthesis failed: {e}")))?;

        // Determine file extension from the audio format
        let ext = match response.format {
            crate::domain::tts::AudioFormat::Wav => "wav",
            crate::domain::tts::AudioFormat::Mp3 => "mp3",
            crate::domain::tts::AudioFormat::Pcm => "pcm",
            crate::domain::tts::AudioFormat::OggOpus => "ogg",
        };

        let filename = format!("{}.{}", uuid::Uuid::new_v4(), ext);
        let file_path = self.tts_output_dir.join(&filename);

        // Ensure the output directory exists
        if let Some(parent) = file_path.parent() {
            tokio::fs::create_dir_all(parent).await.map_err(|e| {
                QqBotError::Internal(format!("failed to create TTS output directory: {e}"))
            })?;
        }

        tokio::fs::write(&file_path, &response.audio_data)
            .await
            .map_err(|e| QqBotError::Internal(format!("failed to write TTS audio file: {e}")))?;

        let url = format!(
            "{}/{}",
            self.tts_public_url_base.trim_end_matches('/'),
            filename
        );
        info!(
            filename = %filename,
            size = response.audio_data.len(),
            url = %url,
            "TTS audio file written"
        );

        Ok(format!("[CQ:record,file={}]", url))
    }
}

/// Convert a ReplySegment to a OneBot-compatible message string.
///
/// Note: `Poke` and `Record` segments are handled directly by the dispatcher
/// and should NOT reach this function. If they do, we return a best-effort string.
fn segment_to_onebot_string(segment: &ReplySegment) -> String {
    match segment {
        ReplySegment::Text { content } => content.clone(),
        ReplySegment::Emoji { id } => format!("[CQ:face,id={}]", id),
        ReplySegment::Kaomoji { text } => text.clone(),
        ReplySegment::Image { path } => format!("[CQ:image,file={}]", path),
        ReplySegment::QuoteReply { message_id, text } => {
            format!("[CQ:reply,id={}]{}", message_id, text)
        }
        ReplySegment::Poke { .. } => {
            warn!("segment_to_onebot_string called on Poke — this should not happen");
            String::new()
        }
        ReplySegment::Record { .. } => {
            warn!("segment_to_onebot_string called on Record — this should not happen");
            String::new()
        }
    }
}
