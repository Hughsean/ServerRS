use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::Value;

/// A post-conversation risk audit record.
///
/// Audits are produced *after* a chat turn closes (assistant message persisted,
/// HTTP/SSE connection closed). They never enter the conversation generation
/// path — not the PromptBuilder, Persona, Memory, or Summary.
///
/// See design doc §3.9 / §6 for the full isolation rules.
#[derive(Debug, Clone, Serialize)]
pub struct PostConversationRiskAudit {
    pub audit_id: u64,
    pub user_id: u64,
    pub conversation_id: u64,

    /// `turn` | `recent_window` | `manual_recheck`
    pub audit_scope: String,

    /// Stable source refs — survive transcript clear (FK may be nulled).
    pub user_message_ref_id: Option<u64>,
    pub assistant_message_ref_id: Option<u64>,

    /// FK to conversation_messages; cleared (set NULL) on transcript clear.
    pub user_message_id: Option<u64>,
    pub assistant_message_id: Option<u64>,

    /// `pending` | `running` | `completed` | `failed` | `discarded`
    pub status: String,

    /// `none` | `low` | `medium` | `high` | `crisis` (only meaningful once completed)
    pub risk_level: Option<String>,
    pub risk_categories: Option<Value>,
    pub confidence: Option<f64>,

    pub input_hash: Option<String>,
    pub detector_name: Option<String>,
    pub detector_version: Option<String>,
    pub model_name: Option<String>,

    pub checked_at: Option<DateTime<Utc>>,
    pub error_message: Option<String>,
    pub metadata: Option<Value>,

    /// Set to 1 when the source messages were cleared; the audit is retained
    /// for traceability but the original text is gone.
    pub source_deleted: bool,

    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Input for creating a pending audit. The worker fills in the result later.
#[derive(Debug, Clone)]
pub struct NewPostConversationRiskAudit {
    pub user_id: u64,
    pub conversation_id: u64,
    pub audit_scope: String,
    pub user_message_ref_id: Option<u64>,
    pub assistant_message_ref_id: Option<u64>,
    pub user_message_id: Option<u64>,
    pub assistant_message_id: Option<u64>,
}

/// Result payload used when marking an audit completed.
#[derive(Debug, Clone)]
pub struct PostRiskAuditResult {
    pub risk_level: String,
    pub risk_categories: Option<Value>,
    pub confidence: Option<f64>,
    pub input_hash: Option<String>,
    pub detector_name: Option<String>,
    pub detector_version: Option<String>,
    pub model_name: Option<String>,
    pub checked_at: DateTime<Utc>,
}
