use async_trait::async_trait;
use chrono::{DateTime, Utc};

use crate::shared::error::AppError;

#[derive(Debug, Clone, Default)]
pub struct PersonaSnapshotSummary {
    pub communication_preferences_count: usize,
    pub stable_facts_count: usize,
    pub recurring_topics_count: usize,
    pub goals_count: usize,
    pub sensitive_context_count: usize,
}

#[derive(Debug, Clone)]
pub struct PersonaView {
    pub has_active_persona: bool,
    pub generated_at: Option<DateTime<Utc>>,
    pub snapshot_summary: PersonaSnapshotSummary,
    pub personalization_enabled: bool,
}

#[derive(Debug, Clone)]
pub struct PersonaResetResult {
    pub reset: bool,
}

#[derive(Debug, Clone)]
pub struct PersonaRebuildResult {
    pub snapshot_id: u64,
}

#[derive(Debug, Clone)]
pub struct TranscriptClearResult {
    pub cleared_messages: bool,
    pub cleared_summaries: bool,
    pub memories_preserved: bool,
    pub persona_preserved: bool,
    pub post_risk_audits_cleared: bool,
    pub summary_ids: Vec<u64>,
}

#[derive(Debug, Clone)]
pub struct ForgetResult {
    pub messages_cleared: bool,
    pub summaries_cleared: bool,
    pub memories_disabled: u64,
    pub persona_expired: bool,
    pub post_risk_audits_deleted: bool,
    pub personalization_disabled: bool,
    pub summary_ids: Vec<u64>,
    pub memory_ids: Vec<u64>,
}

#[async_trait]
pub trait UserContextControlRepoT: Send + Sync {
    async fn persona_view(&self, user_id: u64) -> Result<PersonaView, AppError>;
    async fn refresh_persona_if_stale(
        &self,
        user_id: u64,
    ) -> Result<Option<PersonaRebuildResult>, AppError>;
    async fn reset_persona(&self, user_id: u64) -> Result<PersonaResetResult, AppError>;
    async fn rebuild_persona(&self, user_id: u64) -> Result<PersonaRebuildResult, AppError>;
    async fn clear_transcript(&self, user_id: u64) -> Result<TranscriptClearResult, AppError>;
    async fn forget(&self, user_id: u64) -> Result<ForgetResult, AppError>;
}
