//! 协议无关的富消息 Artifact 引用领域模型。
//!
//! 本模块只描述 Artifact 信封（envelope）和可用性状态，不依赖 NapCat、OneBot、QQ、
//! SeaORM、MySQL、Axum、Tokio 或任何 HTTP 客户端。
//!
//! 核心约束（任务八）：
//! - 不自动下载；URL、签名 URL 不写日志。
//! - 不让 LLM 默认看到完整 URL、JSON、XML 或 forward payload。
//! - URL、raw payload、文件名、描述和嵌套层数必须有上限。
//! - 合并转发不得无限递归展开。
//! - Artifact 只能通过 source_event_id 按需检索。
//! - 撤回、TTL、Owner 删除和内容策略变化必须传播失效。
//! - `never_long_term` 不生成长期 Artifact；`envelope_only` 只保存最小信封。
//! - 严格账号隔离。

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{ContentTrustLevel, ConversationRef, SourceAccountRef, SourceEventId};

/// Artifact 唯一标识。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ArtifactId(String);

impl ArtifactId {
    pub fn new(value: impl Into<String>) -> Result<Self, ArtifactError> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(ArtifactError::InvalidIdentity(
                "artifact_id must not be empty".into(),
            ));
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn for_source_segment(
        source_event_id: &SourceEventId,
        segment_ordinal: usize,
        kind: ArtifactKind,
    ) -> Self {
        let name = format!(
            "{}:{segment_ordinal}:{}",
            source_event_id.as_str(),
            kind.as_str()
        );
        Self(uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_OID, name.as_bytes()).to_string())
    }
}

/// Artifact 种类（任务八：image/file/record/video/forward/JSON/XML/rich card）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactKind {
    Image,
    File,
    Record,
    Video,
    Forward,
    RichJson,
    RichXml,
    RichCard,
}

impl ArtifactKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Image => "image",
            Self::File => "file",
            Self::Record => "record",
            Self::Video => "video",
            Self::Forward => "forward",
            Self::RichJson => "rich_json",
            Self::RichXml => "rich_xml",
            Self::RichCard => "rich_card",
        }
    }

    pub fn parse_from_str(value: &str) -> Option<Self> {
        match value {
            "image" => Some(Self::Image),
            "file" => Some(Self::File),
            "record" => Some(Self::Record),
            "video" => Some(Self::Video),
            "forward" => Some(Self::Forward),
            "rich_json" => Some(Self::RichJson),
            "rich_xml" => Some(Self::RichXml),
            "rich_card" => Some(Self::RichCard),
            _ => None,
        }
    }
}

/// Artifact 可用性状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactAvailability {
    /// 可用。
    Available,
    /// TTL 已过期。
    Expired,
    /// 被撤回（B3 传播）。
    Recalled,
    /// Owner 删除。
    OwnerDeleted,
    /// 内容策略阻止（如 `never_long_term` 后续变化）。
    PolicyBlocked,
}

impl ArtifactAvailability {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Available => "available",
            Self::Expired => "expired",
            Self::Recalled => "recalled",
            Self::OwnerDeleted => "owner_deleted",
            Self::PolicyBlocked => "policy_blocked",
        }
    }

    pub fn parse_from_str(value: &str) -> Option<Self> {
        match value {
            "available" => Some(Self::Available),
            "expired" => Some(Self::Expired),
            "recalled" => Some(Self::Recalled),
            "owner_deleted" => Some(Self::OwnerDeleted),
            "policy_blocked" => Some(Self::PolicyBlocked),
            _ => None,
        }
    }

    /// 是否仍可被检索（只有 Available 为真）。
    pub fn is_available(self) -> bool {
        matches!(self, Self::Available)
    }
}

// ===== 字段上限常量（任务八-4）=====

/// 平台引用（如 file ID、URL 的 host 部分）最大长度。
pub const MAX_PLATFORM_REFERENCE_CHARS: usize = 500;
/// 文件名最大长度。
pub const MAX_DISPLAY_NAME_CHARS: usize = 500;
/// MIME 类型最大长度。
pub const MAX_MIME_TYPE_CHARS: usize = 200;
/// hash/source_key 最大长度。
pub const MAX_HASH_CHARS: usize = 500;
/// 描述最大长度。
pub const MAX_DESCRIPTION_CHARS: usize = 2000;
/// 合并转发最大嵌套层数（防止无限递归展开，任务八-5）。
pub const MAX_FORWARD_NESTING: u32 = 5;

/// 有界字符串：截断到上限并保证 UTF-8 字符边界安全。
fn bounded(value: impl Into<String>, max_chars: usize) -> String {
    let s = value.into();
    if s.chars().count() <= max_chars {
        return s;
    }
    s.chars().take(max_chars).collect()
}

/// Artifact 信封：富消息引用的最小元数据（任务八）。
///
/// 不自动下载；URL 不写日志；不让 LLM 默认看到完整 URL、JSON、XML 或 forward payload。
/// 所有字段有上限，超长按字符截断。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactEnvelope {
    pub artifact_id: ArtifactId,
    pub account: SourceAccountRef,
    pub source_event_id: SourceEventId,
    pub conversation: ConversationRef,
    pub artifact_kind: ArtifactKind,
    /// 平台引用（如 file ID 或 file URL 的 host 部分，有界）。
    pub platform_reference: String,
    /// 有界文件名。
    pub display_name: Option<String>,
    /// 可用时的 MIME 类型（有界）。
    pub mime_type: Option<String>,
    /// 可用时的文件大小（字节）。
    pub size_bytes: Option<u64>,
    /// 可用时的 hash 或 source_key（有界）。
    pub hash_or_source_key: Option<String>,
    /// 有界描述。
    pub description: Option<String>,
    pub availability: ArtifactAvailability,
    pub content_policy: ContentTrustLevel,
    pub created_at_unix_secs: i64,
    /// TTL 过期时间（Unix 秒）。`None` 表示不过期（但 `never_long_term` 不应生成长期 Artifact）。
    pub ttl_expires_at_unix_secs: Option<i64>,
}

impl ArtifactEnvelope {
    /// 构造一个新的可用 Artifact 信封，自动对所有字段做有界截断。
    ///
    /// `never_long_term` 内容策略不生成长期 Artifact（调用方应先检查）。
    /// `envelope_only` 只保存最小信封（display_name/mime_type/hash/description 为 None）。
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        artifact_id: ArtifactId,
        account: SourceAccountRef,
        source_event_id: SourceEventId,
        conversation: ConversationRef,
        artifact_kind: ArtifactKind,
        platform_reference: impl Into<String>,
        content_policy: ContentTrustLevel,
        created_at_unix_secs: i64,
        ttl_expires_at_unix_secs: Option<i64>,
    ) -> Result<Self, ArtifactError> {
        if matches!(content_policy, ContentTrustLevel::NeverLongTerm) {
            return Err(ArtifactError::PolicyViolation(
                "never_long_term must not create persistent artifacts".into(),
            ));
        }

        Ok(Self {
            artifact_id,
            account,
            source_event_id,
            conversation,
            artifact_kind,
            platform_reference: bounded(platform_reference, MAX_PLATFORM_REFERENCE_CHARS),
            // display_name/mime_type/hash/description 初始为 None，
            // 由 with_* 方法填充（envelope_only 时 with_* 方法会跳过）。
            display_name: None,
            mime_type: None,
            size_bytes: None,
            hash_or_source_key: None,
            description: None,
            availability: ArtifactAvailability::Available,
            content_policy,
            created_at_unix_secs,
            ttl_expires_at_unix_secs,
        })
    }

    /// 设置有界文件名。
    pub fn with_display_name(mut self, name: Option<impl Into<String>>) -> Self {
        if !matches!(self.content_policy, ContentTrustLevel::EnvelopeOnly) {
            self.display_name = name.map(|n| bounded(n, MAX_DISPLAY_NAME_CHARS));
        }
        self
    }

    /// 设置有界 MIME 类型。
    pub fn with_mime_type(mut self, mime: Option<impl Into<String>>) -> Self {
        if !matches!(self.content_policy, ContentTrustLevel::EnvelopeOnly) {
            self.mime_type = mime.map(|m| bounded(m, MAX_MIME_TYPE_CHARS));
        }
        self
    }

    /// 设置文件大小。
    pub fn with_size_bytes(mut self, size: Option<u64>) -> Self {
        if !matches!(self.content_policy, ContentTrustLevel::EnvelopeOnly) {
            self.size_bytes = size;
        }
        self
    }

    /// 设置有界 hash/source_key。
    pub fn with_hash_or_source_key(mut self, hash: Option<impl Into<String>>) -> Self {
        if !matches!(self.content_policy, ContentTrustLevel::EnvelopeOnly) {
            self.hash_or_source_key = hash.map(|h| bounded(h, MAX_HASH_CHARS));
        }
        self
    }

    /// 设置有界描述。
    pub fn with_description(mut self, desc: Option<impl Into<String>>) -> Self {
        if !matches!(self.content_policy, ContentTrustLevel::EnvelopeOnly) {
            self.description = desc.map(|d| bounded(d, MAX_DESCRIPTION_CHARS));
        }
        self
    }

    /// 标记为已撤回（B3 传播）。
    pub fn mark_recalled(&mut self) {
        self.availability = ArtifactAvailability::Recalled;
    }

    /// 标记为已过期。
    pub fn mark_expired(&mut self) {
        self.availability = ArtifactAvailability::Expired;
    }

    /// 标记为 Owner 已删除。
    pub fn mark_owner_deleted(&mut self) {
        self.availability = ArtifactAvailability::OwnerDeleted;
    }

    /// 标记为内容策略阻止。
    pub fn mark_policy_blocked(&mut self) {
        self.availability = ArtifactAvailability::PolicyBlocked;
    }

    /// 检查 TTL 是否已过期（当前时间 > TTL 过期时间）。
    pub fn is_ttl_expired(&self, now_unix_secs: i64) -> bool {
        self.ttl_expires_at_unix_secs
            .map(|expires| now_unix_secs >= expires)
            .unwrap_or(false)
    }
}

/// Artifact 领域错误。
#[derive(Debug, Error, PartialEq, Eq)]
pub enum ArtifactError {
    #[error("invalid artifact identity: {0}")]
    InvalidIdentity(String),
    #[error("artifact policy violation: {0}")]
    PolicyViolation(String),
    #[error("artifact store error: {0}")]
    Store(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ConversationKind, MessageSource};

    fn account() -> SourceAccountRef {
        SourceAccountRef::new(MessageSource::NapCat, "acc-1".to_string()).unwrap()
    }

    fn conv() -> ConversationRef {
        ConversationRef::new(ConversationKind::Group, "g-1".to_string()).unwrap()
    }

    fn source_event_id() -> SourceEventId {
        SourceEventId::new("evt-1").unwrap()
    }

    #[test]
    fn artifact_id_rejects_empty() {
        assert!(ArtifactId::new("").is_err());
        assert!(ArtifactId::new("  ").is_err());
        assert!(ArtifactId::new("art-1").is_ok());
    }

    #[test]
    fn artifact_kind_roundtrips() {
        for kind in [
            ArtifactKind::Image,
            ArtifactKind::File,
            ArtifactKind::Record,
            ArtifactKind::Video,
            ArtifactKind::Forward,
            ArtifactKind::RichJson,
            ArtifactKind::RichXml,
            ArtifactKind::RichCard,
        ] {
            let s = kind.as_str();
            assert_eq!(ArtifactKind::parse_from_str(s), Some(kind));
        }
    }

    #[test]
    fn availability_is_available_only_for_available() {
        assert!(ArtifactAvailability::Available.is_available());
        assert!(!ArtifactAvailability::Expired.is_available());
        assert!(!ArtifactAvailability::Recalled.is_available());
        assert!(!ArtifactAvailability::OwnerDeleted.is_available());
        assert!(!ArtifactAvailability::PolicyBlocked.is_available());
    }

    #[test]
    fn never_long_term_rejects_persistent_artifact() {
        let result = ArtifactEnvelope::new(
            ArtifactId::new("art-1").unwrap(),
            account(),
            source_event_id(),
            conv(),
            ArtifactKind::Image,
            "file-123",
            ContentTrustLevel::NeverLongTerm,
            1000,
            None,
        );
        assert!(result.is_err());
    }

    #[test]
    fn envelope_only_strips_non_envelope_fields() {
        let envelope = ArtifactEnvelope::new(
            ArtifactId::new("art-1").unwrap(),
            account(),
            source_event_id(),
            conv(),
            ArtifactKind::Image,
            "file-123",
            ContentTrustLevel::EnvelopeOnly,
            1000,
            None,
        )
        .unwrap()
        .with_display_name(Some("photo.jpg"))
        .with_mime_type(Some("image/jpeg"))
        .with_description(Some("a photo"))
        .with_hash_or_source_key(Some("abc123"))
        .with_size_bytes(Some(1024));

        // envelope_only 只保存最小信封：display_name/mime_type/hash/description 应为 None。
        assert_eq!(envelope.display_name, None);
        assert_eq!(envelope.mime_type, None);
        assert_eq!(envelope.hash_or_source_key, None);
        assert_eq!(envelope.description, None);
        assert_eq!(envelope.size_bytes, None);
        // 但 artifact_id/account/source_event_id/kind/platform_reference 保留。
        assert!(envelope.availability.is_available());
    }

    #[test]
    fn normal_artifact_preserves_all_fields() {
        let envelope = ArtifactEnvelope::new(
            ArtifactId::new("art-1").unwrap(),
            account(),
            source_event_id(),
            conv(),
            ArtifactKind::Image,
            "file-123",
            ContentTrustLevel::Normal,
            1000,
            Some(2000),
        )
        .unwrap()
        .with_display_name(Some("photo.jpg"))
        .with_mime_type(Some("image/jpeg"))
        .with_description(Some("a photo"))
        .with_hash_or_source_key(Some("abc123"))
        .with_size_bytes(Some(1024));

        assert_eq!(envelope.display_name.as_deref(), Some("photo.jpg"));
        assert_eq!(envelope.mime_type.as_deref(), Some("image/jpeg"));
        assert_eq!(envelope.hash_or_source_key.as_deref(), Some("abc123"));
        assert_eq!(envelope.description.as_deref(), Some("a photo"));
        assert_eq!(envelope.size_bytes, Some(1024));
    }

    #[test]
    fn long_fields_are_truncated() {
        let long_name = "x".repeat(MAX_DISPLAY_NAME_CHARS + 1000);
        let envelope = ArtifactEnvelope::new(
            ArtifactId::new("art-1").unwrap(),
            account(),
            source_event_id(),
            conv(),
            ArtifactKind::File,
            "x".repeat(MAX_PLATFORM_REFERENCE_CHARS + 1000),
            ContentTrustLevel::Normal,
            1000,
            None,
        )
        .unwrap()
        .with_display_name(Some(long_name.clone()));

        assert!(envelope.platform_reference.chars().count() <= MAX_PLATFORM_REFERENCE_CHARS);
        assert!(envelope.display_name.as_ref().unwrap().chars().count() <= MAX_DISPLAY_NAME_CHARS);
    }

    #[test]
    fn mark_recalled_updates_availability() {
        let mut envelope = ArtifactEnvelope::new(
            ArtifactId::new("art-1").unwrap(),
            account(),
            source_event_id(),
            conv(),
            ArtifactKind::Image,
            "file-123",
            ContentTrustLevel::Normal,
            1000,
            None,
        )
        .unwrap();
        assert!(envelope.availability.is_available());
        envelope.mark_recalled();
        assert_eq!(envelope.availability, ArtifactAvailability::Recalled);
        assert!(!envelope.availability.is_available());
    }

    #[test]
    fn is_ttl_expired_checks_correctly() {
        let envelope = ArtifactEnvelope::new(
            ArtifactId::new("art-1").unwrap(),
            account(),
            source_event_id(),
            conv(),
            ArtifactKind::Image,
            "file-123",
            ContentTrustLevel::Normal,
            1000,
            Some(2000),
        )
        .unwrap();
        assert!(!envelope.is_ttl_expired(1999));
        assert!(envelope.is_ttl_expired(2000));
        assert!(envelope.is_ttl_expired(2001));
    }
}
