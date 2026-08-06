//! 账号会话目录同步应用层：端口与用例编排。
//!
//! 本模块只依赖领域对象（[`crate::directory`]）和抽象端口，不依赖 NapCat、SeaORM、
//! MySQL 或 `qqbot-server`。外层（`qqbot-server`）实现 [`DirectorySourceT`]，
//! 基础设施层（`personal-secretary/src/infra`）实现 [`DirectoryStoreT`]。
//!
//! 用例职责：
//! 1. 从 NapCat 只读 API（`get_friend_list`/`get_group_list`/`get_recent_contact`）读取会话列表；
//! 2. 有界聚合为 `DirectorySnapshot`，去重、账号作用域、不跨账号合并平台 ID；
//! 3. 持久化快照，支持跨重启恢复与幂等；
//! 4. Gap 创建时冻结目录证据（回补过程不跟随实时 Cursor 漂移）；
//! 5. 1 MiB 上限拒绝时记录为有界失败，保持 `uncertain`，不提高上限、不转空数组。
//!
//! 关键约束（任务六-5/-6/-10/-11）：
//! - 三个列表接口全部成功**不等于**账号历史完整；
//! - 真实 NapCat 无法证明账号会话全集，目录状态只能到达 `KnownScopesComplete`；
//! - 不在每次 WebSocket 重连时无条件下载完整目录（TTL 内跳过）；
//! - 目录同步具备 single-flight、TTL、批次上限、整体 deadline、指数退避、shutdown、任务回收。

use std::sync::Arc;

use async_trait::async_trait;

use crate::{
    ConversationRef, DirectoryError, DirectoryEvidence, DirectorySnapshot, DirectorySnapshotId,
    DirectorySourceApi, IngestionGapId, ScopeBoundary, ScopeKind, SourceAccountRef,
};

/// 目录来源端口：外层 NapCat 适配器实现，按账号视角返回只读会话列表。
///
/// 所有返回的 ID（peerUin、groupId、msgTime 等）必须是字符串形式，保留精度，
/// 不经浮点数转换。禁止对 message ID 做数值加减。
#[async_trait]
pub trait DirectorySourceT: Send + Sync {
    /// 读取好友列表。返回 `(platform_user_id, display_name)` 有界列表。
    async fn list_friends(
        &self,
        account: &SourceAccountRef,
    ) -> Result<Vec<DirectoryListEntry>, DirectorySourceError>;

    /// 读取群列表。返回 `(platform_group_id, display_name, boundary)` 有界列表。
    async fn list_groups(
        &self,
        account: &SourceAccountRef,
    ) -> Result<Vec<DirectoryListEntry>, DirectorySourceError>;

    /// 读取最近联系人。返回 `(platform_id, display_name, boundary, kind_hint)` 有界列表。
    /// `kind_hint` 用于区分好友/群/最近未确认。
    async fn list_recent_contacts(
        &self,
        account: &SourceAccountRef,
    ) -> Result<Vec<DirectoryListEntry>, DirectorySourceError>;
}

/// 目录列表条目：平台 ID + 显示名 + 可选边界。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectoryListEntry {
    /// 平台会话 ID（字符串形式，保留精度）。
    pub platform_id: String,
    /// 显示名（有界，用于审计）。
    pub display_name: Option<String>,
    /// 空窗前的稳定边界（可选，`get_recent_contact` 可提供）。
    pub boundary: Option<ScopeBoundary>,
    /// Scope 类别提示（好友/群/最近未确认）。
    pub kind_hint: ScopeKind,
}

/// 目录来源错误。区分临时失败与永久失败。
#[derive(Debug, Clone, thiserror::Error)]
pub enum DirectorySourceError {
    /// API 超时（整体 deadline 到期）。保持 `uncertain`，可重试。
    #[error("directory source timeout: {0}")]
    Timeout(String),
    /// API 响应被 1 MiB 大小上限拒绝。保持 `uncertain`，不提高上限。
    #[error("directory source response oversized: {0}")]
    Oversized(String),
    /// API 返回 malformed DTO。保持 `uncertain`。
    #[error("directory source malformed response: {0}")]
    Malformed(String),
    /// API 不可用（retcode 非 0、PermissionDenied）。可能永久。
    #[error("directory source unavailable: {0}")]
    Unavailable(String),
    /// 临时网络错误。可重试，不能永久固化为不支持（任务六-13）。
    #[error("directory source transient error: {0}")]
    Transient(String),
}

/// 目录存储端口：基础设施层（MySQL）实现。
///
/// 快照绑定 `account_id`，有账号作用域唯一键；幂等；跨重启恢复。
#[async_trait]
pub trait DirectoryStoreT: Send + Sync {
    /// 持久化一次目录同步快照。同一账号多次同步产生不同 snapshot_id。
    /// 幂等：相同 snapshot_id 重复写入不报错。
    async fn snapshot_directory(
        &self,
        snapshot: &DirectorySnapshot,
    ) -> Result<(), DirectoryStoreError>;

    /// 读取某账号最新一次成功同步的快照。无快照返回 `None`。
    async fn load_latest_snapshot(
        &self,
        account: &SourceAccountRef,
    ) -> Result<Option<DirectorySnapshot>, DirectoryStoreError>;

    /// 冻结目录证据到某个 Gap：回补过程读此快照而非实时 Cursor（任务六-7）。
    /// 返回冻结时的快照状态，供回补用例填充 `BackfillEvidence`。
    async fn freeze_for_gap(
        &self,
        gap_id: &IngestionGapId,
        account: &SourceAccountRef,
    ) -> Result<Option<DirectorySnapshot>, DirectoryStoreError>;

    /// 读取冻结到某 Gap 的目录快照。
    async fn load_frozen_for_gap(
        &self,
        gap_id: &IngestionGapId,
    ) -> Result<Option<DirectorySnapshot>, DirectoryStoreError>;

    /// 该账号是否有 TTL 内的有效快照（避免每次重连无条件下载完整目录）。
    async fn has_valid_snapshot(
        &self,
        account: &SourceAccountRef,
        ttl_secs: u64,
        now_unix_secs: i64,
    ) -> Result<bool, DirectoryStoreError>;
}

/// 目录存储错误。
#[derive(Debug, Clone, thiserror::Error)]
pub enum DirectoryStoreError {
    #[error("directory store invalid data: {0}")]
    InvalidData(String),
    #[error("directory store unavailable: {0}")]
    Unavailable(String),
    #[error("directory store database error: {0}")]
    Database(String),
}

/// 目录同步配置。有界预算，禁止无限循环。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DirectorySyncBudget {
    /// 快照 TTL（秒）。TTL 内跳过完整下载。
    pub snapshot_ttl_secs: u64,
    /// 单次同步的整体 deadline（秒）。
    pub sync_deadline_secs: u64,
    /// 单次同步的条目上限（防止单账号会话过多导致无界处理）。
    pub max_entries: u32,
    /// 错误退避初始延迟（毫秒）。
    pub retry_initial_ms: u64,
    /// 错误退避最大延迟（毫秒）。
    pub retry_max_ms: u64,
}

impl Default for DirectorySyncBudget {
    fn default() -> Self {
        Self {
            snapshot_ttl_secs: 3600,
            sync_deadline_secs: 30,
            max_entries: 5000,
            retry_initial_ms: 1000,
            retry_max_ms: 60000,
        }
    }
}

impl DirectorySyncBudget {
    pub fn validate(&self) -> Result<(), DirectoryError> {
        if self.snapshot_ttl_secs == 0 {
            return Err(DirectoryError::Sync(
                "snapshot_ttl_secs must be positive".into(),
            ));
        }
        if self.sync_deadline_secs == 0 {
            return Err(DirectoryError::Sync(
                "sync_deadline_secs must be positive".into(),
            ));
        }
        if self.max_entries == 0 {
            return Err(DirectoryError::Sync("max_entries must be positive".into()));
        }
        if self.retry_max_ms < self.retry_initial_ms {
            return Err(DirectoryError::Sync(
                "retry_max_ms must be >= retry_initial_ms".into(),
            ));
        }
        Ok(())
    }
}

/// 目录同步用例。协议无关，由外层 Worker 驱动。
///
/// 编排 single-flight（通过 `Mutex<Option<JoinHandle>>` 在外层实现）、TTL、批次上限、
/// 整体 deadline、指数退避、shutdown。本用例只负责单次同步的逻辑编排。
pub struct DirectorySyncUseCase {
    source: Arc<dyn DirectorySourceT>,
    store: Arc<dyn DirectoryStoreT>,
    budget: DirectorySyncBudget,
}

impl DirectorySyncUseCase {
    pub fn new(
        source: Arc<dyn DirectorySourceT>,
        store: Arc<dyn DirectoryStoreT>,
        budget: DirectorySyncBudget,
    ) -> Result<Self, DirectoryError> {
        budget.validate()?;
        Ok(Self {
            source,
            store,
            budget,
        })
    }

    /// 执行一次目录同步。返回同步后的快照状态。
    ///
    /// 如果 TTL 内已有有效快照，跳过完整下载（任务六-11）。
    /// 1 MiB 上限拒绝时记录为有界失败，保持 `uncertain`，不提高上限（任务六-12）。
    ///
    /// 注意：本用例按顺序调用三个列表接口；整体 deadline / 超时由外层运行时
    /// （`qqbot-server` 的 Worker）通过 `tokio::time::timeout` 包装控制，
    /// 或由 `DirectorySourceT` 的基础设施实现内部并发执行。领域层不依赖 Tokio。
    pub async fn sync_once(
        &self,
        account: &SourceAccountRef,
        now_unix_secs: i64,
    ) -> Result<DirectorySnapshot, DirectorySyncError> {
        // TTL 内跳过完整下载。
        let has_valid = self
            .store
            .has_valid_snapshot(account, self.budget.snapshot_ttl_secs, now_unix_secs)
            .await
            .map_err(DirectorySyncError::Store)?;
        if has_valid
            && let Some(existing) = self
                .store
                .load_latest_snapshot(account)
                .await
                .map_err(DirectorySyncError::Store)?
        {
            return Ok(existing);
        }
        // has_valid=true 但 load=None（并发删除）：继续完整下载。

        // 三个列表接口顺序调用。整体 deadline 由外层 Worker 控制。
        // 1 MiB 上限拒绝等错误映射为 DirectorySyncError。
        let friends = self
            .source
            .list_friends(account)
            .await
            .map_err(map_source_error)?;
        let groups = self
            .source
            .list_groups(account)
            .await
            .map_err(map_source_error)?;
        let recent = self
            .source
            .list_recent_contacts(account)
            .await
            .map_err(map_source_error)?;

        // 聚合为会话 Scope 列表，去重，账号作用域，不跨账号合并平台 ID。
        let scopes = self.aggregate_scopes(friends, groups, recent);

        // 构建证据。
        let evidence = DirectoryEvidence {
            source_api: Some(DirectorySourceApi::FriendGroupRecent),
            friend_count: scopes
                .iter()
                .filter(|s| s.scope_kind == ScopeKind::Friend)
                .count() as u32,
            group_count: scopes
                .iter()
                .filter(|s| s.scope_kind == ScopeKind::Group)
                .count() as u32,
            recent_count: scopes
                .iter()
                .filter(|s| s.scope_kind == ScopeKind::RecentUnconfirmed)
                .count() as u32,
            any_oversized: false,
            any_timeout: false,
            any_malformed: false,
            any_unavailable: false,
            probed_at_unix_secs: now_unix_secs,
        };

        // 根据证据推导状态（真实 NapCat 只能到达 KnownScopesComplete）。
        let status = evidence.derive_status();

        // 快照 ID 使用 UUID，避免账号/时间戳拼接超过 CHAR(36)。
        let snapshot = DirectorySnapshot {
            snapshot_id: DirectorySnapshotId::new(uuid::Uuid::new_v4().to_string())
                .map_err(|e| DirectorySyncError::InvalidIdentity(e.to_string()))?,
            account: account.clone(),
            source_api: DirectorySourceApi::FriendGroupRecent,
            status,
            evidence,
            scopes,
            created_at_unix_secs: now_unix_secs,
        };

        // 持久化快照（幂等）。
        self.store
            .snapshot_directory(&snapshot)
            .await
            .map_err(DirectorySyncError::Store)?;

        Ok(snapshot)
    }

    /// 把三个列表条目聚合为去重的会话 Scope 列表。
    ///
    /// 去重规则：同一 `(kind, conversation_id)` 只保留一条。
    /// 好友和群优先于最近未确认；如果最近联系人在好友/群列表中，则归类为好友/群。
    fn aggregate_scopes(
        &self,
        friends: Vec<DirectoryListEntry>,
        groups: Vec<DirectoryListEntry>,
        recent: Vec<DirectoryListEntry>,
    ) -> Vec<crate::ConversationScope> {
        use std::collections::HashSet;

        let mut seen: HashSet<(ScopeKind, String)> = HashSet::new();
        let mut scopes: Vec<crate::ConversationScope> = Vec::new();
        let max = self.budget.max_entries as usize;

        // 好友优先。
        for entry in friends.into_iter().take(max.saturating_sub(scopes.len())) {
            push_scope(
                &mut seen,
                &mut scopes,
                entry,
                ScopeKind::Friend,
                crate::ConversationKind::Private,
            );
        }

        // 群。
        for entry in groups.into_iter().take(max.saturating_sub(scopes.len())) {
            push_scope(
                &mut seen,
                &mut scopes,
                entry,
                ScopeKind::Group,
                crate::ConversationKind::Group,
            );
        }

        // 最近联系人：如果已在好友/群列表中则跳过，否则标记为 RecentUnconfirmed。
        for entry in recent.into_iter().take(max.saturating_sub(scopes.len())) {
            // 尝试好友和群两种 key，如果都未见过则作为 RecentUnconfirmed。
            let friend_key = (ScopeKind::Friend, entry.platform_id.clone());
            let group_key = (ScopeKind::Group, entry.platform_id.clone());
            if seen.contains(&friend_key) || seen.contains(&group_key) {
                continue;
            }
            // 最近联系人可能是好友或群，但目录未确认。按 kind_hint 归类。
            let conv_kind = match entry.kind_hint {
                ScopeKind::Group => crate::ConversationKind::Group,
                _ => crate::ConversationKind::Private,
            };
            push_scope(
                &mut seen,
                &mut scopes,
                entry,
                ScopeKind::RecentUnconfirmed,
                conv_kind,
            );
        }

        scopes
    }

    /// 退避延迟（毫秒）。指数增长，封顶。
    pub fn backoff_ms(&self, attempt: u32) -> u64 {
        let base = self.budget.retry_initial_ms;
        let max = self.budget.retry_max_ms;
        let exp = 2u64.saturating_pow(attempt.min(31));
        base.saturating_mul(exp).min(max)
    }
}

/// 辅助函数：去重并推入一个会话 Scope。
/// 已在 `seen` 中存在的 `(kind, platform_id)` 跳过；`ConversationRef::new` 失败也跳过。
fn push_scope(
    seen: &mut std::collections::HashSet<(ScopeKind, String)>,
    scopes: &mut Vec<crate::ConversationScope>,
    entry: DirectoryListEntry,
    kind: ScopeKind,
    conv_kind: crate::ConversationKind,
) {
    let key = (kind, entry.platform_id.clone());
    if !seen.insert(key) {
        return;
    }
    if let Ok(conv) = ConversationRef::new(conv_kind, entry.platform_id) {
        scopes.push(crate::ConversationScope {
            conversation: conv,
            scope_kind: kind,
            boundary: entry.boundary,
            display_name: entry.display_name,
        });
    }
}

/// 辅助函数：把 `DirectorySourceError` 映射为 `DirectorySyncError`。
fn map_source_error(e: DirectorySourceError) -> DirectorySyncError {
    match e {
        DirectorySourceError::Timeout(_) | DirectorySourceError::Transient(_) => {
            DirectorySyncError::SourceTimeout
        }
        DirectorySourceError::Oversized(_) => DirectorySyncError::Oversized,
        DirectorySourceError::Malformed(_) => DirectorySyncError::Malformed,
        DirectorySourceError::Unavailable(_) => DirectorySyncError::Unavailable,
    }
}

/// 目录同步错误。
#[derive(Debug, thiserror::Error)]
pub enum DirectorySyncError {
    #[error("directory sync timeout")]
    Timeout,
    #[error("directory sync source timeout")]
    SourceTimeout,
    #[error("directory sync source response oversized")]
    Oversized,
    #[error("directory sync source malformed")]
    Malformed,
    #[error("directory sync source unavailable")]
    Unavailable,
    #[error("directory sync invalid identity: {0}")]
    InvalidIdentity(String),
    #[error("directory sync store error: {0}")]
    Store(#[from] DirectoryStoreError),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::DirectoryStatus;
    use crate::directory::DirectoryEvidence;
    use async_trait::async_trait;

    /// Fake 目录来源：返回预定义列表，用于测试聚合与去重逻辑。
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

    /// 内存目录存储：用于测试用例编排。
    struct InMemoryDirectoryStore {
        snapshots: std::sync::Mutex<Vec<DirectorySnapshot>>,
        frozen: std::sync::Mutex<std::collections::HashMap<String, DirectorySnapshot>>,
    }

    impl InMemoryDirectoryStore {
        fn new() -> Self {
            Self {
                snapshots: std::sync::Mutex::new(Vec::new()),
                frozen: std::sync::Mutex::new(std::collections::HashMap::new()),
            }
        }
    }

    #[async_trait]
    impl DirectoryStoreT for InMemoryDirectoryStore {
        async fn snapshot_directory(
            &self,
            snapshot: &DirectorySnapshot,
        ) -> Result<(), DirectoryStoreError> {
            self.snapshots_lock(snapshot);
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
            gap_id: &IngestionGapId,
            account: &SourceAccountRef,
        ) -> Result<Option<DirectorySnapshot>, DirectoryStoreError> {
            let snapshots = self.snapshots.lock().unwrap();
            let latest = snapshots
                .iter()
                .filter(|s| s.account.account_id == account.account_id)
                .max_by_key(|s| s.created_at_unix_secs)
                .cloned();
            if let Some(snap) = &latest {
                self.frozen
                    .lock()
                    .unwrap()
                    .insert(gap_id.as_str().to_string(), snap.clone());
            }
            Ok(latest)
        }
        async fn load_frozen_for_gap(
            &self,
            gap_id: &IngestionGapId,
        ) -> Result<Option<DirectorySnapshot>, DirectoryStoreError> {
            Ok(self.frozen.lock().unwrap().get(gap_id.as_str()).cloned())
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

    impl InMemoryDirectoryStore {
        fn snapshots_lock(&self, snapshot: &DirectorySnapshot) {
            let mut snapshots = self.snapshots.lock().unwrap();
            // 幂等：相同 snapshot_id 不重复。
            if !snapshots
                .iter()
                .any(|s| s.snapshot_id == snapshot.snapshot_id)
            {
                snapshots.push(snapshot.clone());
            }
        }
    }

    fn account(id: &str) -> SourceAccountRef {
        SourceAccountRef::new(crate::MessageSource::NapCat, id.to_string()).unwrap()
    }

    fn friend_entry(id: &str, name: &str) -> DirectoryListEntry {
        DirectoryListEntry {
            platform_id: id.to_string(),
            display_name: Some(name.to_string()),
            boundary: None,
            kind_hint: ScopeKind::Friend,
        }
    }

    fn group_entry(id: &str, name: &str) -> DirectoryListEntry {
        DirectoryListEntry {
            platform_id: id.to_string(),
            display_name: Some(name.to_string()),
            boundary: None,
            kind_hint: ScopeKind::Group,
        }
    }

    fn recent_entry(id: &str, name: &str) -> DirectoryListEntry {
        DirectoryListEntry {
            platform_id: id.to_string(),
            display_name: Some(name.to_string()),
            boundary: Some(ScopeBoundary::new("msg-1", "1000")),
            kind_hint: ScopeKind::RecentUnconfirmed,
        }
    }

    #[tokio::test]
    async fn sync_once_aggregates_friends_groups_recent_and_dedupes() {
        let source = Arc::new(FakeDirectorySource {
            friends: vec![friend_entry("f-1", "Alice"), friend_entry("f-2", "Bob")],
            groups: vec![group_entry("g-1", "Group A")],
            recent: vec![
                recent_entry("f-1", "Alice"),   // 已在好友列表，跳过
                recent_entry("r-1", "Unknown"), // 未确认，保留
            ],
        });
        let store = Arc::new(InMemoryDirectoryStore::new());
        let use_case =
            DirectorySyncUseCase::new(source, store.clone(), DirectorySyncBudget::default())
                .unwrap();

        let snapshot = use_case.sync_once(&account("acc-1"), 1000).await.unwrap();

        // 三个列表接口全部成功且有条目 -> KnownScopesComplete。
        assert_eq!(snapshot.status, DirectoryStatus::KnownScopesComplete);
        // f-1 不重复（好友优先于最近未确认）。
        let friend_count = snapshot
            .scopes
            .iter()
            .filter(|s| s.scope_kind == ScopeKind::Friend)
            .count();
        assert_eq!(friend_count, 2);
        let group_count = snapshot
            .scopes
            .iter()
            .filter(|s| s.scope_kind == ScopeKind::Group)
            .count();
        assert_eq!(group_count, 1);
        let recent_count = snapshot
            .scopes
            .iter()
            .filter(|s| s.scope_kind == ScopeKind::RecentUnconfirmed)
            .count();
        assert_eq!(recent_count, 1); // r-1 only; f-1 deduped
        // 总共 4 个不重复 Scope。
        assert_eq!(snapshot.scopes.len(), 4);
        // 快照已持久化。
        assert!(snapshot.validate_no_duplicate_scopes().is_ok());
    }

    #[tokio::test]
    async fn sync_once_skips_download_when_valid_snapshot_in_ttl() {
        let source = Arc::new(FakeDirectorySource {
            friends: vec![friend_entry("f-1", "Alice")],
            groups: vec![],
            recent: vec![],
        });
        let store = Arc::new(InMemoryDirectoryStore::new());
        let use_case = DirectorySyncUseCase::new(
            source,
            store.clone(),
            DirectorySyncBudget {
                snapshot_ttl_secs: 3600,
                ..Default::default()
            },
        )
        .unwrap();

        // 第一次同步。
        let snap1 = use_case.sync_once(&account("acc-1"), 1000).await.unwrap();
        assert_eq!(snap1.evidence.friend_count, 1);

        // 第二次同步在 TTL 内，应跳过完整下载，返回已有快照。
        let snap2 = use_case.sync_once(&account("acc-1"), 2000).await.unwrap();
        // 应返回与第一次相同的快照（snapshot_id 相同）。
        assert_eq!(snap1.snapshot_id, snap2.snapshot_id);
    }

    #[tokio::test]
    async fn backoff_grows_exponentially_and_caps() {
        let use_case = DirectorySyncUseCase::new(
            Arc::new(FakeDirectorySource {
                friends: vec![],
                groups: vec![],
                recent: vec![],
            }),
            Arc::new(InMemoryDirectoryStore::new()),
            DirectorySyncBudget {
                retry_initial_ms: 100,
                retry_max_ms: 1000,
                ..Default::default()
            },
        )
        .unwrap();

        assert_eq!(use_case.backoff_ms(0), 100);
        assert_eq!(use_case.backoff_ms(1), 200);
        assert_eq!(use_case.backoff_ms(2), 400);
        assert_eq!(use_case.backoff_ms(3), 800);
        assert_eq!(use_case.backoff_ms(4), 1000); // capped
        assert_eq!(use_case.backoff_ms(10), 1000); // capped
    }

    #[test]
    fn budget_rejects_invalid_values() {
        assert!(
            DirectorySyncBudget {
                snapshot_ttl_secs: 0,
                ..Default::default()
            }
            .validate()
            .is_err()
        );
        assert!(
            DirectorySyncBudget {
                sync_deadline_secs: 0,
                ..Default::default()
            }
            .validate()
            .is_err()
        );
        assert!(
            DirectorySyncBudget {
                max_entries: 0,
                ..Default::default()
            }
            .validate()
            .is_err()
        );
        assert!(
            DirectorySyncBudget {
                retry_initial_ms: 100,
                retry_max_ms: 50, // < initial
                ..Default::default()
            }
            .validate()
            .is_err()
        );
        assert!(DirectorySyncBudget::default().validate().is_ok());
    }

    #[tokio::test]
    async fn freeze_for_gap_returns_latest_snapshot() {
        let store = Arc::new(InMemoryDirectoryStore::new());
        // 先写入一个快照。
        let snapshot = DirectorySnapshot {
            snapshot_id: DirectorySnapshotId::new("snap-1").unwrap(),
            account: account("acc-1"),
            source_api: DirectorySourceApi::FriendGroupRecent,
            status: DirectoryStatus::KnownScopesComplete,
            evidence: DirectoryEvidence::default(),
            scopes: vec![],
            created_at_unix_secs: 1000,
        };
        store.snapshot_directory(&snapshot).await.unwrap();

        let gap_id = IngestionGapId::new("gap-1").unwrap();
        let frozen = store
            .freeze_for_gap(&gap_id, &account("acc-1"))
            .await
            .unwrap();
        assert!(frozen.is_some());
        assert_eq!(frozen.unwrap().snapshot_id, snapshot.snapshot_id);

        // 回读冻结的快照。
        let reloaded = store.load_frozen_for_gap(&gap_id).await.unwrap();
        assert!(reloaded.is_some());
        assert_eq!(reloaded.unwrap().snapshot_id, snapshot.snapshot_id);
    }
}
