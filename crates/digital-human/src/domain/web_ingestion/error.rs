//! 网页知识摄取错误类型。

use std::fmt;

/// 网页知识摄取流水线特有的错误。
#[derive(Debug)]
pub enum WebIngestionError {
    /// 给定 URL 的 SSRF 检查失败。
    SsrfRejected { url: String, reason: String },
    /// HTTP fetch failed.
    FetchFailed { url: String, reason: String },
    /// Remote server asked the crawler to slow down.
    RateLimited {
        url: String,
        status: u16,
        retry_after_secs: Option<u64>,
    },
    /// Content type not allowed.
    ContentTypeNotAllowed { content_type: String },
    /// Body exceeds the configured maximum size.
    BodyTooLarge { size: u64, max: u64 },
    /// Clean text is too short to be useful.
    ContentTooShort { chars: usize, min: usize },
    /// Distill LLM returned invalid JSON after retries.
    DistillJsonParseFailed { error: String },
    /// Distill LLM API key is empty.
    DistillApiKeyEmpty,
    /// Quality gate rejected the content.
    QualityRejected { reason: String },
    /// Quality gate requires staging.
    QualityStaged { reason: String },
    /// Embedding provider returned mismatched dimension.
    EmbeddingDimensionMismatch { expected: usize, actual: usize },
    /// Embedding provider returned wrong number of vectors.
    EmbeddingCountMismatch { expected: usize, actual: usize },
    /// State machine transition is not allowed.
    InvalidTransition {
        from_status: String,
        from_stage: String,
        to_status: String,
        to_stage: String,
    },
    /// An entity was not found.
    NotFound { entity: String, id: u64 },
    /// Duplicate event / run key detected (idempotency violation).
    DuplicateKey { key_type: String, key: String },
    /// The requested review action conflicts with current persisted state.
    ReviewConflict { reason: String },
    /// Generic internal error.
    Internal(String),
}

impl fmt::Display for WebIngestionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SsrfRejected { url, reason } => {
                write!(f, "SSRF rejected URL {url}: {reason}")
            }
            Self::FetchFailed { url, reason } => {
                write!(f, "fetch failed for {url}: {reason}")
            }
            Self::RateLimited {
                url,
                status,
                retry_after_secs,
            } => match retry_after_secs {
                Some(seconds) => write!(
                    f,
                    "fetch rate limited for {url}: HTTP {status}, retry after {seconds}s"
                ),
                None => write!(f, "fetch rate limited for {url}: HTTP {status}"),
            },
            Self::ContentTypeNotAllowed { content_type } => {
                write!(f, "content-type not allowed: {content_type}")
            }
            Self::BodyTooLarge { size, max } => {
                write!(f, "body too large: {size} bytes (max {max})")
            }
            Self::ContentTooShort { chars, min } => {
                write!(f, "content too short: {chars} chars (min {min})")
            }
            Self::DistillJsonParseFailed { error } => {
                write!(f, "distill JSON parse failed: {error}")
            }
            Self::DistillApiKeyEmpty => {
                write!(f, "web_ingestion distill llm api key is empty")
            }
            Self::QualityRejected { reason } => {
                write!(f, "quality gate rejected: {reason}")
            }
            Self::QualityStaged { reason } => {
                write!(f, "quality gate staged: {reason}")
            }
            Self::EmbeddingDimensionMismatch { expected, actual } => {
                write!(
                    f,
                    "embedding dimension mismatch: expected {expected}, got {actual}"
                )
            }
            Self::EmbeddingCountMismatch { expected, actual } => {
                write!(
                    f,
                    "embedding count mismatch: expected {expected}, got {actual}"
                )
            }
            Self::InvalidTransition {
                from_status,
                from_stage,
                to_status,
                to_stage,
            } => {
                write!(
                    f,
                    "invalid transition ({from_status},{from_stage}) -> ({to_status},{to_stage})"
                )
            }
            Self::NotFound { entity, id } => {
                write!(f, "{entity} not found: id={id}")
            }
            Self::DuplicateKey { key_type, key } => {
                write!(f, "duplicate {key_type}: {key}")
            }
            Self::ReviewConflict { reason } => {
                write!(f, "knowledge review conflict: {reason}")
            }
            Self::Internal(msg) => write!(f, "web ingestion internal error: {msg}"),
        }
    }
}

impl std::error::Error for WebIngestionError {}

impl WebIngestionError {
    pub fn retry_after_secs(&self) -> Option<u64> {
        match self {
            Self::RateLimited {
                retry_after_secs, ..
            } => *retry_after_secs,
            _ => None,
        }
    }
}
