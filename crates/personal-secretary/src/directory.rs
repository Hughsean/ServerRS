//! 协议无关的账号会话目录领域模型。
//!
//! 本模块描述"账号在 NapCat 上有哪些会话"的目录快照与证据，不依赖 NapCat、OneBot、
//! QQ、SeaORM、MySQL、Axum、Tokio 或任何 HTTP 客户端。协议适配和持久化由外层实现
//! 应用层定义的端口（见 [`crate::directory_service`]）。
//!
//! 核心不变量：三个列表接口（`get_friend_list`/`get_group_list`/`get_recent_contact`）
//! 全部调用成功**不等于**账号历史完整。真实 NapCat 无法枚举账号全部会话，目录状态
//! 只能到达 `KnownScopesComplete` 或 `Uncertain`，不得进入 `VerifiedComplete`。
//! 目录状态不建第二套完整性状态机：它映射到既有的 [`crate::HistoryCompleteness`]，
//! 通过 `account_conversation_set_proven` 标志喂入 [`crate::BackfillEvidence`]。
//!
//! 平台消息 ID、`peerUin`、`msgTime` 必须兼容字符串与数字，且不得经过浮点数转换。
//! 禁止对 message ID 做数值加减。

use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{ConversationRef, SourceAccountRef};

/// 目录快照的稳定标识。同一账号多次同步产生不同 snapshot_id，便于审计与回读。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct DirectorySnapshotId(String);

impl DirectorySnapshotId {
    /// 快照 ID 必须能放入 `CHAR(36)`；生产路径使用 UUID。
    pub fn new(value: impl Into<String>) -> Result<Self, DirectoryError> {
        let value = value.into();
        let trimmed = value.trim();
        if trimmed.is_empty() {
            return Err(DirectoryError::InvalidIdentity(
                "directory snapshot_id must not be empty".into(),
            ));
        }
        if trimmed.len() > 36 {
            return Err(DirectoryError::InvalidIdentity(format!(
                "directory snapshot_id exceeds CHAR(36) limit (len={})",
                trimmed.len()
            )));
        }
        if trimmed.len() != value.len() {
            return Err(DirectoryError::InvalidIdentity(
                "directory snapshot_id must not contain leading/trailing whitespace".into(),
            ));
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// 产生目录快照的来源 API。用于审计与回读，不存储 API 的完整响应。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DirectorySourceApi {
    /// `get_friend_list` + `get_group_list` + `get_recent_contact` 组合。
    FriendGroupRecent,
    /// `get_recent_contact` 单独。
    RecentContact,
    /// 仅历史查询中观察到的会话（无主动目录同步）。
    ObservedFromHistory,
}

impl DirectorySourceApi {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::FriendGroupRecent => "friend_group_recent",
            Self::RecentContact => "recent_contact",
            Self::ObservedFromHistory => "observed_from_history",
        }
    }

    pub fn parse_from_str(value: &str) -> Option<Self> {
        match value {
            "friend_group_recent" => Some(Self::FriendGroupRecent),
            "recent_contact" => Some(Self::RecentContact),
            "observed_from_history" => Some(Self::ObservedFromHistory),
            _ => None,
        }
    }
}

/// 目录同步状态。区分任务六-3 的多种情况，但**不建第二套完整性状态机**：
/// 通过 [`DirectoryStatus::to_history_completeness`] 映射到既有的
/// [`crate::HistoryCompleteness`]。
///
/// 核心约束：真实 NapCat 无法证明账号会话全集时，只能进入
/// `KnownScopesComplete` 或 `Uncertain`，不得进入 `VerifiedComplete`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DirectoryStatus {
    /// 已知会话 Scope 已枚举完整，但账号会话集合不可证完整（真实 NapCat 常态）。
    /// 映射到 `HistoryCompleteness::KnownScopesComplete`，Gap 保持 `uncertain`。
    KnownScopesComplete,
    /// 账号会话集合可证完整（仅确定性 Fake 来源可达；真实 NapCat 不可达）。
    /// 映射到 `HistoryCompleteness::ProvenComplete`。
    VerifiedComplete,
    /// 证据不足：部分接口未返回、超时或返回 malformed。映射到 `Unprovable`。
    Uncertain,
    /// API 不可用（retcode 非 0、PermissionDenied）。映射到 `Unrecoverable`。
    Unavailable,
    /// API 超时（整体 deadline 到期）。映射到 `Unprovable`。
    ApiTimeout,
    /// API 响应被 1 MiB 大小上限拒绝。映射到 `Unprovable`。
    /// 不得提高上限、不得把错误转成空数组成功。
    ApiOversized,
    /// 列表 API 能力延迟验证（Deferred）。映射到 `Unprovable`。
    /// 临时网络错误不能永久固化为不支持。
    ApiDeferred,
}

impl DirectoryStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::KnownScopesComplete => "known_scopes_complete",
            Self::VerifiedComplete => "verified_complete",
            Self::Uncertain => "uncertain",
            Self::Unavailable => "unavailable",
            Self::ApiTimeout => "api_timeout",
            Self::ApiOversized => "api_oversized",
            Self::ApiDeferred => "api_deferred",
        }
    }

    pub fn parse_from_str(value: &str) -> Option<Self> {
        match value {
            "known_scopes_complete" => Some(Self::KnownScopesComplete),
            "verified_complete" => Some(Self::VerifiedComplete),
            "uncertain" => Some(Self::Uncertain),
            "unavailable" => Some(Self::Unavailable),
            "api_timeout" => Some(Self::ApiTimeout),
            "api_oversized" => Some(Self::ApiOversized),
            "api_deferred" => Some(Self::ApiDeferred),
            _ => None,
        }
    }

    /// 映射到既有的 `HistoryCompleteness`，不建第二套完整性状态机。
    ///
    /// - `KnownScopesComplete` -> `KnownScopesComplete`（Gap 保持 uncertain）
    /// - `VerifiedComplete` -> `ProvenComplete`（仅确定性 Fake 可达）
    /// - `Uncertain`/`ApiTimeout`/`ApiOversized`/`ApiDeferred` -> `Unprovable`
    /// - `Unavailable` -> `Unrecoverable`
    pub fn to_history_completeness(self) -> crate::HistoryCompleteness {
        use crate::HistoryCompleteness as HC;
        match self {
            Self::KnownScopesComplete => HC::KnownScopesComplete,
            Self::VerifiedComplete => HC::ProvenComplete,
            Self::Uncertain | Self::ApiTimeout | Self::ApiOversized | Self::ApiDeferred => {
                HC::Unprovable
            }
            Self::Unavailable => HC::Unrecoverable,
        }
    }

    /// 是否可以安全声称"已知 Scope 已完整"。只有 `KnownScopesComplete` 和
    /// `VerifiedComplete` 为真。`Uncertain` 等不得声称完整。
    pub fn known_scopes_complete(self) -> bool {
        matches!(self, Self::KnownScopesComplete | Self::VerifiedComplete)
    }
}

/// 会话 Scope 的类别。区分已知好友/群、最近出现但目录未确认的会话、
/// 已删除/退出/不可访问的会话。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScopeKind {
    /// 已知好友会话（来自 `get_friend_list`）。
    Friend,
    /// 已知群会话（来自 `get_group_list`）。
    Group,
    /// 最近出现但目录未确认的会话（来自 `get_recent_contact` 但不在好友/群列表中）。
    RecentUnconfirmed,
    /// 已删除或退出的会话（历史中存在但当前目录不可见）。
    Deleted,
    /// 已退出或被踢出的群会话。
    Exited,
    /// 不可访问的会话（权限不足、被封禁等）。
    Inaccessible,
}

impl ScopeKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Friend => "friend",
            Self::Group => "group",
            Self::RecentUnconfirmed => "recent_unconfirmed",
            Self::Deleted => "deleted",
            Self::Exited => "exited",
            Self::Inaccessible => "inaccessible",
        }
    }

    pub fn parse_from_str(value: &str) -> Option<Self> {
        match value {
            "friend" => Some(Self::Friend),
            "group" => Some(Self::Group),
            "recent_unconfirmed" => Some(Self::RecentUnconfirmed),
            "deleted" => Some(Self::Deleted),
            "exited" => Some(Self::Exited),
            "inaccessible" => Some(Self::Inaccessible),
            _ => None,
        }
    }
}

/// 会话 Scope 的边界快照：空窗前的最后已知消息 ID 与时间。
///
/// 平台消息 ID 和时间戳均为字符串，兼容 NapCat 返回的字符串或数字字段，
/// **不经浮点数转换**（任务六-9）。禁止对 message ID 做数值加减（任务六-8）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScopeBoundary {
    /// 平台消息 ID（字符串形式，保留精度）。
    pub message_id: String,
    /// 消息时间戳（字符串形式，Unix 秒或毫秒，由来源决定）。
    pub msg_time: String,
}

impl ScopeBoundary {
    pub fn new(message_id: impl Into<String>, msg_time: impl Into<String>) -> Self {
        Self {
            message_id: message_id.into(),
            msg_time: msg_time.into(),
        }
    }
}

/// 目录中单个会话 Scope 的条目。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConversationScope {
    pub conversation: ConversationRef,
    pub scope_kind: ScopeKind,
    /// 空窗前的稳定边界（Gap 创建时冻结，回补过程不得跟随实时 Cursor 漂移）。
    pub boundary: Option<ScopeBoundary>,
    /// 平台显示名（好友昵称、群名等），有界，用于审计而非业务决策。
    pub display_name: Option<String>,
}

/// 目录同步的证据。不存储 API 的完整响应，只记录聚合后的事实。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DirectoryEvidence {
    /// 来源 API。
    pub source_api: Option<DirectorySourceApi>,
    /// 好友列表条目数。
    pub friend_count: u32,
    /// 群列表条目数。
    pub group_count: u32,
    /// 最近联系人条目数。
    pub recent_count: u32,
    /// 是否有接口被 1 MiB 上限拒绝。
    pub any_oversized: bool,
    /// 是否有接口超时。
    pub any_timeout: bool,
    /// 是否有接口返回 malformed DTO。
    pub any_malformed: bool,
    /// 是否有接口返回非 0 retcode（不可用）。
    pub any_unavailable: bool,
    /// 探测时间（Unix 秒）。
    pub probed_at_unix_secs: i64,
}

impl DirectoryEvidence {
    /// 根据证据推导目录状态。不直接声称完整。
    ///
    /// 判定顺序（任务六-3/-12）：
    /// 1. 任一接口不可用且无成功响应 -> `Unavailable`
    /// 2. 任一接口超时 -> `ApiTimeout`（保持 uncertain）
    /// 3. 任一接口被上限拒绝 -> `ApiOversized`（保持 uncertain，不提高上限）
    /// 4. 任一接口 malformed -> `Uncertain`
    /// 5. 所有接口成功且有会话条目 -> `KnownScopesComplete`（真实 NapCat 常态）
    /// 6. 所有接口成功但无会话条目 -> `Uncertain`（空目录歧义）
    ///
    /// 永远不返回 `VerifiedComplete`：真实 NapCat 无法证明账号会话全集。
    /// `VerifiedComplete` 仅由确定性 Fake 来源在应用层显式设置。
    pub fn derive_status(&self) -> DirectoryStatus {
        if self.any_unavailable
            && self.friend_count == 0
            && self.group_count == 0
            && self.recent_count == 0
        {
            return DirectoryStatus::Unavailable;
        }
        if self.any_timeout {
            return DirectoryStatus::ApiTimeout;
        }
        if self.any_oversized {
            return DirectoryStatus::ApiOversized;
        }
        if self.any_malformed {
            return DirectoryStatus::Uncertain;
        }
        // 所有接口成功：真实 NapCat 只能证明已知 Scope，不能证明全集。
        if self.friend_count > 0 || self.group_count > 0 || self.recent_count > 0 {
            DirectoryStatus::KnownScopesComplete
        } else {
            // 空目录：可能是新账号，也可能是 API 返回空但未报错。保持 uncertain。
            DirectoryStatus::Uncertain
        }
    }
}

/// 账号会话目录快照：某次同步在某个账号下观察到的全部已知会话。
///
/// 绑定 `account_id`，不跨账号合并平台 ID（任务六-4）。有稳定 snapshot_id，
/// 可跨重启恢复，幂等，不在每次 WS 重连时无条件下载完整目录（任务六-11）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DirectorySnapshot {
    pub snapshot_id: DirectorySnapshotId,
    pub account: SourceAccountRef,
    pub source_api: DirectorySourceApi,
    pub status: DirectoryStatus,
    pub evidence: DirectoryEvidence,
    /// 本次同步观察到的会话 Scope 列表。已按 `(kind, id)` 去重。
    pub scopes: Vec<ConversationScope>,
    pub created_at_unix_secs: i64,
}

impl DirectorySnapshot {
    /// 去重校验：同一快照内不得有重复的 `(kind, conversation_id)`。
    /// 跨账号合并由持久化层 account_id 作用域保证。
    pub fn validate_no_duplicate_scopes(&self) -> Result<(), DirectoryError> {
        let mut seen = HashSet::new();
        for scope in &self.scopes {
            let key = (scope.scope_kind.as_str(), scope.conversation.id.as_str());
            if !seen.insert(key) {
                return Err(DirectoryError::DuplicateScope {
                    kind: scope.scope_kind.as_str().to_string(),
                    conversation_id: scope.conversation.id.clone(),
                });
            }
        }
        Ok(())
    }
}

/// 目录领域错误。
#[derive(Debug, Error, PartialEq, Eq)]
pub enum DirectoryError {
    #[error("invalid directory identity: {0}")]
    InvalidIdentity(String),
    #[error("duplicate scope in snapshot: kind={kind}, conversation_id={conversation_id}")]
    DuplicateScope {
        kind: String,
        conversation_id: String,
    },
    #[error("directory sync error: {0}")]
    Sync(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ConversationKind, ConversationRef};

    fn conv(kind: ConversationKind, id: &str) -> ConversationRef {
        ConversationRef::new(kind, id.to_string()).unwrap()
    }

    fn account(id: &str) -> SourceAccountRef {
        SourceAccountRef::new(crate::MessageSource::NapCat, id.to_string()).unwrap()
    }

    #[test]
    fn snapshot_id_rejects_empty() {
        assert!(DirectorySnapshotId::new("").is_err());
        assert!(DirectorySnapshotId::new("  ").is_err());
        assert!(DirectorySnapshotId::new("snap-1").is_ok());
    }

    #[test]
    fn derive_status_returns_unavailable_when_all_apis_fail_and_no_entries() {
        let evidence = DirectoryEvidence {
            any_unavailable: true,
            ..Default::default()
        };
        assert_eq!(evidence.derive_status(), DirectoryStatus::Unavailable);
    }

    #[test]
    fn derive_status_returns_known_scopes_complete_when_all_succeed_with_entries() {
        let evidence = DirectoryEvidence {
            friend_count: 5,
            group_count: 3,
            ..Default::default()
        };
        // 真实 NapCat 常态：只能证明已知 Scope，不能证明全集。
        assert_eq!(
            evidence.derive_status(),
            DirectoryStatus::KnownScopesComplete
        );
    }

    #[test]
    fn derive_status_returns_api_timeout_when_any_timeout() {
        let evidence = DirectoryEvidence {
            friend_count: 5,
            any_timeout: true,
            ..Default::default()
        };
        // 即使部分接口成功，有超时仍保持 uncertain。
        assert_eq!(evidence.derive_status(), DirectoryStatus::ApiTimeout);
    }

    #[test]
    fn derive_status_returns_api_oversized_when_any_oversized() {
        let evidence = DirectoryEvidence {
            friend_count: 5,
            any_oversized: true,
            ..Default::default()
        };
        // 被上限拒绝：保持 uncertain，不提高上限、不转空数组。
        assert_eq!(evidence.derive_status(), DirectoryStatus::ApiOversized);
    }

    #[test]
    fn derive_status_returns_uncertain_for_empty_directory() {
        let evidence = DirectoryEvidence::default();
        // 空目录歧义：不声称完整。
        assert_eq!(evidence.derive_status(), DirectoryStatus::Uncertain);
    }

    #[test]
    fn derive_status_never_returns_verified_complete() {
        // 无论证据多充分，derive_status 永不返回 VerifiedComplete。
        // 真实 NapCat 无法证明账号会话全集。
        let evidence = DirectoryEvidence {
            friend_count: 1000,
            group_count: 500,
            recent_count: 200,
            ..Default::default()
        };
        assert_ne!(evidence.derive_status(), DirectoryStatus::VerifiedComplete);
    }

    #[test]
    fn to_history_completeness_maps_correctly() {
        use crate::HistoryCompleteness as HC;
        assert_eq!(
            DirectoryStatus::KnownScopesComplete.to_history_completeness(),
            HC::KnownScopesComplete
        );
        assert_eq!(
            DirectoryStatus::VerifiedComplete.to_history_completeness(),
            HC::ProvenComplete
        );
        assert_eq!(
            DirectoryStatus::Uncertain.to_history_completeness(),
            HC::Unprovable
        );
        assert_eq!(
            DirectoryStatus::ApiTimeout.to_history_completeness(),
            HC::Unprovable
        );
        assert_eq!(
            DirectoryStatus::ApiOversized.to_history_completeness(),
            HC::Unprovable
        );
        assert_eq!(
            DirectoryStatus::ApiDeferred.to_history_completeness(),
            HC::Unprovable
        );
        assert_eq!(
            DirectoryStatus::Unavailable.to_history_completeness(),
            HC::Unrecoverable
        );
    }

    #[test]
    fn known_scopes_complete_only_true_for_complete_states() {
        assert!(DirectoryStatus::KnownScopesComplete.known_scopes_complete());
        assert!(DirectoryStatus::VerifiedComplete.known_scopes_complete());
        assert!(!DirectoryStatus::Uncertain.known_scopes_complete());
        assert!(!DirectoryStatus::Unavailable.known_scopes_complete());
        assert!(!DirectoryStatus::ApiTimeout.known_scopes_complete());
        assert!(!DirectoryStatus::ApiOversized.known_scopes_complete());
        assert!(!DirectoryStatus::ApiDeferred.known_scopes_complete());
    }

    #[test]
    fn validate_no_duplicate_scopes_detects_duplicates() {
        let snapshot = DirectorySnapshot {
            snapshot_id: DirectorySnapshotId::new("s1").unwrap(),
            account: account("acc-1"),
            source_api: DirectorySourceApi::FriendGroupRecent,
            status: DirectoryStatus::KnownScopesComplete,
            evidence: DirectoryEvidence::default(),
            scopes: vec![
                ConversationScope {
                    conversation: conv(ConversationKind::Group, "g-1"),
                    scope_kind: ScopeKind::Group,
                    boundary: None,
                    display_name: None,
                },
                ConversationScope {
                    conversation: conv(ConversationKind::Group, "g-1"),
                    scope_kind: ScopeKind::Group,
                    boundary: None,
                    display_name: None,
                },
            ],
            created_at_unix_secs: 1000,
        };
        assert!(snapshot.validate_no_duplicate_scopes().is_err());
    }

    #[test]
    fn validate_no_duplicate_scopes_accepts_unique_scopes() {
        let snapshot = DirectorySnapshot {
            snapshot_id: DirectorySnapshotId::new("s1").unwrap(),
            account: account("acc-1"),
            source_api: DirectorySourceApi::FriendGroupRecent,
            status: DirectoryStatus::KnownScopesComplete,
            evidence: DirectoryEvidence::default(),
            scopes: vec![
                ConversationScope {
                    conversation: conv(ConversationKind::Group, "g-1"),
                    scope_kind: ScopeKind::Group,
                    boundary: None,
                    display_name: None,
                },
                ConversationScope {
                    conversation: conv(ConversationKind::Private, "f-1"),
                    scope_kind: ScopeKind::Friend,
                    boundary: None,
                    display_name: None,
                },
            ],
            created_at_unix_secs: 1000,
        };
        assert!(snapshot.validate_no_duplicate_scopes().is_ok());
    }

    #[test]
    fn scope_boundary_uses_strings_not_floats() {
        // 大数值 peerUin/msgTime 以字符串保留精度，不经浮点。
        let boundary = ScopeBoundary::new("9999999999999999", "1753526400");
        assert_eq!(boundary.message_id, "9999999999999999");
        assert_eq!(boundary.msg_time, "1753526400");
    }
}
