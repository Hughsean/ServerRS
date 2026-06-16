use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::domain::qq_bot::attention::BotAccount;
use crate::domain::qq_bot::config::{ExternalUser, GroupConfig, GroupMember, TriggerPolicy};
use crate::domain::qq_bot::message::{NormalizedMessage, ProcessStatus};
use crate::domain::qq_bot::qq_profile_repository::QqUserProfileRepository;
use crate::domain::qq_bot::relationship::RelationshipState;
use crate::domain::qq_bot::relationship_repository::RelationshipRepository;
use crate::domain::qq_bot::repository::{
    AgentTurnRepository, BotAccountRepository, ExternalUserRepository, GroupMemberRepository,
    GroupMemory, GroupMemoryRepository, GroupMessageRepository, GroupRepository,
    GroupSummary, GroupSummaryRepository, OutboxEntry, OutboxRepository, OutboxStatus,
};
use crate::domain::qq_bot::turn::{AgentTurn, TurnStatus};
use crate::domain::qq_bot::user_profile::UserProfile;
use crate::shared::error::AppError;

/// In-memory mock implementation of all QQ Bot repositories.
/// Useful for testing and development before SeaORM entities are generated.
pub struct MockQqBotRepositories {
    pub bot_accounts: Arc<MockBotAccountRepo>,
    pub external_users: Arc<MockExternalUserRepo>,
    pub groups: Arc<MockGroupRepo>,
    pub group_members: Arc<MockGroupMemberRepo>,
    pub messages: Arc<MockGroupMessageRepo>,
    pub turns: Arc<MockAgentTurnRepo>,
    pub outbox: Arc<MockOutboxRepo>,
    pub summaries: Arc<MockGroupSummaryRepo>,
    pub memories: Arc<MockGroupMemoryRepo>,
    pub user_profiles: Arc<MockUserProfileRepo>,
    pub relationships: Arc<MockRelationshipRepo>,
}

impl MockQqBotRepositories {
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        Self {
            bot_accounts: Arc::new(MockBotAccountRepo::new()),
            external_users: Arc::new(MockExternalUserRepo::new()),
            groups: Arc::new(MockGroupRepo::new()),
            group_members: Arc::new(MockGroupMemberRepo::new()),
            messages: Arc::new(MockGroupMessageRepo::new()),
            turns: Arc::new(MockAgentTurnRepo::new()),
            outbox: Arc::new(MockOutboxRepo::new()),
            summaries: Arc::new(MockGroupSummaryRepo::new()),
            memories: Arc::new(MockGroupMemoryRepo::new()),
            user_profiles: Arc::new(MockUserProfileRepo::new()),
            relationships: Arc::new(MockRelationshipRepo::new()),
        }
    }
}

// ── BotAccount ──────────────────────────────────────────────────────────

pub struct MockBotAccountRepo {
    store: Arc<RwLock<HashMap<i64, BotAccount>>>,
}

impl MockBotAccountRepo {
    pub fn new() -> Self { Self { store: Arc::new(RwLock::new(HashMap::new())) } }
}

#[async_trait]
impl BotAccountRepository for MockBotAccountRepo {
    async fn find_by_self_qq_id(&self, self_qq_id: i64) -> Result<Option<BotAccount>, AppError> {
        Ok(self.store.read().await.get(&self_qq_id).cloned())
    }
    async fn find_enabled(&self) -> Result<Vec<BotAccount>, AppError> {
        Ok(self.store.read().await.values().filter(|a| a.enabled).cloned().collect())
    }
    async fn upsert(&self, account: &BotAccount) -> Result<BotAccount, AppError> {
        self.store.write().await.insert(account.self_qq_id, account.clone());
        Ok(account.clone())
    }
}

// ── ExternalUser ────────────────────────────────────────────────────────

pub struct MockExternalUserRepo {
    store: Arc<RwLock<HashMap<i64, ExternalUser>>>,
}

impl MockExternalUserRepo {
    pub fn new() -> Self { Self { store: Arc::new(RwLock::new(HashMap::new())) } }
}

#[async_trait]
impl ExternalUserRepository for MockExternalUserRepo {
    async fn find_by_qq_user_id(&self, qq_user_id: i64) -> Result<Option<ExternalUser>, AppError> {
        Ok(self.store.read().await.get(&qq_user_id).cloned())
    }
    async fn upsert(&self, user: &ExternalUser) -> Result<ExternalUser, AppError> {
        self.store.write().await.insert(user.qq_user_id, user.clone());
        Ok(user.clone())
    }
    async fn update_last_seen(&self, qq_user_id: i64, _last_seen_at: i64) -> Result<(), AppError> {
        if let Some(u) = self.store.write().await.get_mut(&qq_user_id) {
            u.last_seen_at = Some(_last_seen_at);
        }
        Ok(())
    }
}

// ── Group ───────────────────────────────────────────────────────────────

pub struct MockGroupRepo {
    store: Arc<RwLock<HashMap<i64, GroupConfig>>>,
}

impl MockGroupRepo {
    pub fn new() -> Self { Self { store: Arc::new(RwLock::new(HashMap::new())) } }
}

#[async_trait]
impl GroupRepository for MockGroupRepo {
    async fn find_by_group_id(&self, qq_group_id: i64) -> Result<Option<GroupConfig>, AppError> {
        Ok(self.store.read().await.get(&qq_group_id).cloned())
    }
    async fn find_enabled_by_bot(&self, _bot_account_id: u64) -> Result<Vec<GroupConfig>, AppError> {
        Ok(self.store.read().await.values().filter(|g| g.enabled).cloned().collect())
    }
    async fn upsert(&self, group: &GroupConfig) -> Result<GroupConfig, AppError> {
        self.store.write().await.insert(group.qq_group_id, group.clone());
        Ok(group.clone())
    }
    async fn update_last_seen(&self, qq_group_id: i64, _last_seen_at: i64) -> Result<(), AppError> {
        if let Some(g) = self.store.write().await.get_mut(&qq_group_id) {
            g.group_name = Some(format!("group_{qq_group_id}"));
        }
        Ok(())
    }
    async fn update_trigger_policy(&self, qq_group_id: i64, policy: TriggerPolicy) -> Result<(), AppError> {
        if let Some(g) = self.store.write().await.get_mut(&qq_group_id) {
            g.trigger_policy = policy;
        }
        Ok(())
    }
    async fn set_enabled(&self, qq_group_id: i64, enabled: bool) -> Result<(), AppError> {
        if let Some(g) = self.store.write().await.get_mut(&qq_group_id) {
            g.enabled = enabled;
        }
        Ok(())
    }
}

// ── GroupMember ─────────────────────────────────────────────────────────

pub struct MockGroupMemberRepo {
    store: Arc<RwLock<HashMap<(i64, i64), GroupMember>>>,
}

impl MockGroupMemberRepo {
    pub fn new() -> Self { Self { store: Arc::new(RwLock::new(HashMap::new())) } }
}

#[async_trait]
impl GroupMemberRepository for MockGroupMemberRepo {
    async fn find(&self, qq_group_id: i64, qq_user_id: i64) -> Result<Option<GroupMember>, AppError> {
        Ok(self.store.read().await.get(&(qq_group_id, qq_user_id)).cloned())
    }
    async fn upsert(&self, member: &GroupMember) -> Result<GroupMember, AppError> {
        self.store.write().await.insert((member.qq_group_id, member.qq_user_id), member.clone());
        Ok(member.clone())
    }
    async fn update_last_seen(&self, qq_group_id: i64, qq_user_id: i64, _last_seen_at: i64) -> Result<(), AppError> {
        if let Some(m) = self.store.write().await.get_mut(&(qq_group_id, qq_user_id)) {
            m.last_seen_at = Some(_last_seen_at);
        }
        Ok(())
    }
    async fn list_by_group(&self, qq_group_id: i64) -> Result<Vec<GroupMember>, AppError> {
        Ok(self.store.read().await.values().filter(|m| m.qq_group_id == qq_group_id).cloned().collect())
    }
}

// ── GroupMessage ────────────────────────────────────────────────────────

pub struct MockGroupMessageRepo {
    store: Arc<RwLock<HashMap<u64, NormalizedMessage>>>,
    next_id: Arc<RwLock<u64>>,
    by_platform: Arc<RwLock<HashMap<(u64, String), u64>>>, // (bot_account_id, platform_id) -> internal_id
}

impl MockGroupMessageRepo {
    pub fn new() -> Self {
        Self {
            store: Arc::new(RwLock::new(HashMap::new())),
            next_id: Arc::new(RwLock::new(1)),
            by_platform: Arc::new(RwLock::new(HashMap::new())),
        }
    }
}

#[async_trait]
impl GroupMessageRepository for MockGroupMessageRepo {
    async fn insert(&self, msg: &NormalizedMessage) -> Result<NormalizedMessage, AppError> {
        let key = (msg.bot_account_id, msg.platform_message_id.clone());
        if let Some(existing_id) = self.by_platform.read().await.get(&key) {
            if let Some(existing) = self.store.read().await.get(existing_id) {
                return Ok(existing.clone());
            }
        }
        let mut id = self.next_id.write().await;
        let mut m = msg.clone();
        m.id = Some(*id);
        self.by_platform.write().await.insert(key, *id);
        self.store.write().await.insert(*id, m.clone());
        *id += 1;
        Ok(m)
    }
    async fn find_by_platform_id(&self, bot_account_id: u64, platform_message_id: &str) -> Result<Option<NormalizedMessage>, AppError> {
        let key = (bot_account_id, platform_message_id.to_string());
        if let Some(id) = self.by_platform.read().await.get(&key) {
            return Ok(self.store.read().await.get(id).cloned());
        }
        Ok(None)
    }
    async fn recent_by_group(&self, qq_group_id: i64, limit: u32) -> Result<Vec<NormalizedMessage>, AppError> {
        let mut msgs: Vec<NormalizedMessage> = self.store.read().await.values()
            .filter(|m| m.qq_group_id == qq_group_id)
            .cloned()
            .collect();
        msgs.sort_by(|a, b| a.sent_at.cmp(&b.sent_at));
        msgs.truncate(limit as usize);
        Ok(msgs)
    }
    async fn update_status(&self, id: u64, status: ProcessStatus, error: Option<&str>) -> Result<(), AppError> {
        if let Some(_m) = self.store.write().await.get_mut(&id) {
            let _ = status;
            let _ = error;
            // ProcessStatus field not in NormalizedMessage, skip
        }
        Ok(())
    }
}

// ── AgentTurn ───────────────────────────────────────────────────────────

pub struct MockAgentTurnRepo {
    store: Arc<RwLock<HashMap<u64, AgentTurn>>>,
    next_id: Arc<RwLock<u64>>,
}

impl MockAgentTurnRepo {
    pub fn new() -> Self {
        Self { store: Arc::new(RwLock::new(HashMap::new())), next_id: Arc::new(RwLock::new(1)) }
    }
}

#[async_trait]
impl AgentTurnRepository for MockAgentTurnRepo {
    async fn insert(&self, turn: &AgentTurn) -> Result<AgentTurn, AppError> {
        let mut id = self.next_id.write().await;
        let mut t = turn.clone();
        t.turn_id = Some(*id);
        self.store.write().await.insert(*id, t.clone());
        *id += 1;
        Ok(t)
    }
    async fn update_response(&self, turn_id: u64, response_message_id: u64, status: TurnStatus) -> Result<(), AppError> {
        if let Some(t) = self.store.write().await.get_mut(&turn_id) {
            t.response_message_id = Some(response_message_id);
            t.status = status;
        }
        Ok(())
    }
    async fn update_status(&self, turn_id: u64, status: TurnStatus, error: Option<&str>) -> Result<(), AppError> {
        if let Some(t) = self.store.write().await.get_mut(&turn_id) {
            t.status = status;
            t.error_message = error.map(|s| s.to_string());
        }
        Ok(())
    }
    async fn find_by_trace_id(&self, _trace_id: &str) -> Result<Option<AgentTurn>, AppError> {
        Ok(self.store.read().await.values().find(|t| t.trace_id.as_deref() == Some(_trace_id)).cloned())
    }
    async fn recent_by_group(&self, qq_group_id: i64, limit: u32) -> Result<Vec<AgentTurn>, AppError> {
        let mut turns: Vec<AgentTurn> = self.store.read().await.values()
            .filter(|t| t.qq_group_id == qq_group_id)
            .cloned()
            .collect();
        turns.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        turns.truncate(limit as usize);
        Ok(turns)
    }
}

// ── Outbox ──────────────────────────────────────────────────────────────

pub struct MockOutboxRepo {
    store: Arc<RwLock<HashMap<u64, OutboxEntry>>>,
    next_id: Arc<RwLock<u64>>,
}

impl MockOutboxRepo {
    pub fn new() -> Self {
        Self { store: Arc::new(RwLock::new(HashMap::new())), next_id: Arc::new(RwLock::new(1)) }
    }
}

#[async_trait]
impl OutboxRepository for MockOutboxRepo {
    async fn insert(&self, entry: &OutboxEntry) -> Result<OutboxEntry, AppError> {
        let mut id = self.next_id.write().await;
        let mut e = entry.clone();
        e.outbox_id = Some(*id);
        self.store.write().await.insert(*id, e.clone());
        *id += 1;
        Ok(e)
    }
    async fn fetch_due(&self, limit: u32) -> Result<Vec<OutboxEntry>, AppError> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis() as i64;
        let mut entries: Vec<OutboxEntry> = self.store.read().await.values()
            .filter(|e| matches!(e.status, OutboxStatus::Pending) && e.next_run_at <= now)
            .cloned()
            .collect();
        entries.sort_by(|a, b| a.next_run_at.cmp(&b.next_run_at));
        entries.truncate(limit as usize);
        Ok(entries)
    }
    async fn mark_sent(&self, outbox_id: u64, platform_message_id: &str) -> Result<(), AppError> {
        if let Some(e) = self.store.write().await.get_mut(&outbox_id) {
            e.status = OutboxStatus::Sent;
            e.platform_message_id = Some(platform_message_id.to_string());
        }
        Ok(())
    }
    async fn mark_failed(&self, outbox_id: u64, error: &str) -> Result<(), AppError> {
        if let Some(e) = self.store.write().await.get_mut(&outbox_id) {
            e.status = OutboxStatus::Failed;
            e.last_error = Some(error.to_string());
            e.attempts += 1;
        }
        Ok(())
    }
    async fn mark_cancelled(&self, outbox_id: u64) -> Result<(), AppError> {
        if let Some(e) = self.store.write().await.get_mut(&outbox_id) {
            e.status = OutboxStatus::Cancelled;
        }
        Ok(())
    }
}

// ── GroupSummary ────────────────────────────────────────────────────────

pub struct MockGroupSummaryRepo {
    store: Arc<RwLock<HashMap<u64, GroupSummary>>>,
    next_id: Arc<RwLock<u64>>,
}

impl MockGroupSummaryRepo {
    pub fn new() -> Self {
        Self { store: Arc::new(RwLock::new(HashMap::new())), next_id: Arc::new(RwLock::new(1)) }
    }
}

#[async_trait]
impl GroupSummaryRepository for MockGroupSummaryRepo {
    async fn find_active_rolling(&self, qq_group_id: i64) -> Result<Option<GroupSummary>, AppError> {
        Ok(self.store.read().await.values()
            .find(|s| s.qq_group_id == qq_group_id && s.status)
            .cloned())
    }
    async fn insert(&self, summary: &GroupSummary) -> Result<GroupSummary, AppError> {
        let mut id = self.next_id.write().await;
        let mut s = summary.clone();
        s.summary_id = Some(*id);
        self.store.write().await.insert(*id, s.clone());
        *id += 1;
        Ok(s)
    }
    async fn disable(&self, summary_id: u64) -> Result<(), AppError> {
        if let Some(s) = self.store.write().await.get_mut(&summary_id) {
            s.status = false;
        }
        Ok(())
    }
}

// ── GroupMemory ─────────────────────────────────────────────────────────

pub struct MockGroupMemoryRepo {
    store: Arc<RwLock<HashMap<u64, GroupMemory>>>,
    next_id: Arc<RwLock<u64>>,
}

impl MockGroupMemoryRepo {
    pub fn new() -> Self {
        Self { store: Arc::new(RwLock::new(HashMap::new())), next_id: Arc::new(RwLock::new(1)) }
    }
}

#[async_trait]
impl GroupMemoryRepository for MockGroupMemoryRepo {
    async fn find_active_by_group(&self, qq_group_id: i64, limit: u32) -> Result<Vec<GroupMemory>, AppError> {
        let mut memories: Vec<GroupMemory> = self.store.read().await.values()
            .filter(|m| m.qq_group_id == qq_group_id && m.status == 1)
            .cloned()
            .collect();
        memories.sort_by(|a, b| b.salience.partial_cmp(&a.salience).unwrap_or(std::cmp::Ordering::Equal));
        memories.truncate(limit as usize);
        Ok(memories)
    }
    async fn upsert(&self, memory: &GroupMemory) -> Result<GroupMemory, AppError> {
        let mut store = self.store.write().await;
        // Try to find existing by same memory_key / canonical_form for dedup
        if let Some(mem_key) = &memory.memory_key {
            if let Some(existing) = store.values_mut().find(|m| m.memory_key.as_deref() == Some(mem_key) && m.qq_group_id == memory.qq_group_id) {
                // 更新现有记录
                existing.content = memory.content.clone();
                existing.confidence = memory.confidence;
                existing.salience = memory.salience;
                existing.reinforce_count += 1;
                return Ok(existing.clone());
            }
        }
        let mut id = self.next_id.write().await;
        let mut m = memory.clone();
        m.group_memory_id = Some(*id);
        store.insert(*id, m.clone());
        *id += 1;
        Ok(m)
    }
    async fn disable(&self, group_memory_id: u64) -> Result<(), AppError> {
        if let Some(m) = self.store.write().await.get_mut(&group_memory_id) {
            m.status = 0;
        }
        Ok(())
    }
}

// ── UserProfile ─────────────────────────────────────────────────────────

pub struct MockUserProfileRepo {
    store: Arc<RwLock<HashMap<i64, UserProfile>>>,
}

impl MockUserProfileRepo {
    pub fn new() -> Self {
        Self { store: Arc::new(RwLock::new(HashMap::new())) }
    }
}

#[async_trait]
impl QqUserProfileRepository for MockUserProfileRepo {
    async fn find_by_qq_user_id(&self, qq_user_id: i64) -> Result<Option<UserProfile>, AppError> {
        Ok(self.store.read().await.get(&qq_user_id).cloned())
    }
    async fn upsert(&self, profile: &UserProfile) -> Result<UserProfile, AppError> {
        self.store.write().await.insert(profile.qq_user_id, profile.clone());
        Ok(profile.clone())
    }
    async fn update_stats(
        &self,
        qq_user_id: i64,
        total_messages: u32,
        avg_message_length: f64,
        emoji_usage_rate: f64,
    ) -> Result<(), AppError> {
        if let Some(p) = self.store.write().await.get_mut(&qq_user_id) {
            p.total_messages = total_messages;
            p.avg_message_length = avg_message_length;
            p.emoji_usage_rate = emoji_usage_rate;
        }
        Ok(())
    }
    async fn update_summary_at(&self, qq_user_id: i64, last_summary_at: i64) -> Result<(), AppError> {
        if let Some(p) = self.store.write().await.get_mut(&qq_user_id) {
            p.last_summary_at = Some(last_summary_at);
        }
        Ok(())
    }
}

// ── Relationship ────────────────────────────────────────────────────────

type RelKey = (i64, i64); // (qq_group_id, qq_user_id)

pub struct MockRelationshipRepo {
    store: Arc<RwLock<HashMap<RelKey, RelationshipState>>>,
    next_id: Arc<RwLock<u64>>,
}

impl MockRelationshipRepo {
    pub fn new() -> Self {
        Self {
            store: Arc::new(RwLock::new(HashMap::new())),
            next_id: Arc::new(RwLock::new(1)),
        }
    }
}

#[async_trait]
impl RelationshipRepository for MockRelationshipRepo {
    async fn find(&self, qq_group_id: i64, qq_user_id: i64) -> Result<Option<RelationshipState>, AppError> {
        let store = self.store.read().await;
        Ok(store.get(&(qq_group_id, qq_user_id)).cloned())
    }

    async fn upsert(&self, rel: &RelationshipState) -> Result<RelationshipState, AppError> {
        let mut store = self.store.write().await;
        let key = (rel.qq_group_id, rel.qq_user_id);
        let mut updated = rel.clone();
        if updated.id.is_none() {
            let mut next_id = self.next_id.write().await;
            updated.id = Some(*next_id);
            *next_id += 1;
        }
        store.insert(key, updated.clone());
        Ok(updated)
    }

    async fn find_by_group(&self, qq_group_id: i64) -> Result<Vec<RelationshipState>, AppError> {
        let store = self.store.read().await;
        let rels: Vec<RelationshipState> = store
            .iter()
            .filter(|((gid, _), _)| *gid == qq_group_id)
            .map(|(_, v)| v.clone())
            .collect();
        Ok(rels)
    }

    async fn increment_interaction(&self, qq_group_id: i64, qq_user_id: i64) -> Result<(), AppError> {
        let mut store = self.store.write().await;
        let key = (qq_group_id, qq_user_id);
        if let Some(rel) = store.get_mut(&key) {
            rel.interaction_count = rel.interaction_count.saturating_add(1);
            rel.familiarity = (0.1 + rel.interaction_count as f32 * 0.015).min(1.0);
            rel.last_interaction_at = Some(chrono::Utc::now().timestamp());
            rel.rapport = crate::domain::qq_bot::RapportLevel::from_familiarity(rel.familiarity);
        } else {
            // Create new
            let mut next_id = self.next_id.write().await;
            let new_rel = RelationshipState {
                id: Some(*next_id),
                qq_group_id,
                qq_user_id,
                familiarity: 0.1,
                interaction_count: 1,
                last_interaction_at: Some(chrono::Utc::now().timestamp()),
                rapport: crate::domain::qq_bot::RapportLevel::Neutral,
                nickname_preference: None,
                known_interests: Vec::new(),
                known_avoid_topics: Vec::new(),
            };
            *next_id += 1;
            store.insert(key, new_rel);
        }
        Ok(())
    }
}
