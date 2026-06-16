use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;

use tokio::sync::RwLock;
use tracing::info;

use crate::domain::qq_bot::AttentionState;

/// In-memory attention store for the QQ bot.
/// Manages attention state across groups with atomic operations.
pub struct InMemoryAttentionStore {
    state: Arc<RwLock<AttentionState>>,
    engaged_group_id: Arc<AtomicI64>,
    cooldown_until_ms: Arc<AtomicI64>,
    /// Cooldown duration in milliseconds.
    cooldown_duration_ms: i64,
    /// Idle timeout in milliseconds — how long without messages before auto-disengage.
    idle_timeout_ms: i64,
    /// Last activity timestamp (epoch ms) for the currently engaged group.
    last_activity_ms: Arc<AtomicI64>,
}

impl InMemoryAttentionStore {
    pub fn new(cooldown_secs: u64, idle_timeout_secs: u64) -> Self {
        Self {
            state: Arc::new(RwLock::new(AttentionState::Idle)),
            engaged_group_id: Arc::new(AtomicI64::new(0)),
            cooldown_until_ms: Arc::new(AtomicI64::new(0)),
            cooldown_duration_ms: (cooldown_secs as i64) * 1000,
            idle_timeout_ms: (idle_timeout_secs as i64) * 1000,
            last_activity_ms: Arc::new(AtomicI64::new(0)),
        }
    }

    /// Try to engage with a group. Returns true if engagement is granted.
    pub async fn try_engage(&self, group_id: i64) -> bool {
        let mut state = self.state.write().await;
        match *state {
            AttentionState::Idle => {
                *state = AttentionState::Engaging(group_id);
                self.engaged_group_id.store(group_id, Ordering::SeqCst);
                self.last_activity_ms.store(now_ms(), Ordering::SeqCst);
                info!(group_id, "attention: engaging");
                true
            }
            AttentionState::Engaging(gid) | AttentionState::Engaged(gid) if gid == group_id => {
                // Already engaged with this group
                self.last_activity_ms.store(now_ms(), Ordering::SeqCst);
                true
            }
            AttentionState::Engaging(_) | AttentionState::Engaged(_) => {
                // Busy with another group
                false
            }
            AttentionState::Cooldown(gid, until) => {
                if (now_ms() as u64) >= until {
                    // Cooldown expired, can engage
                    *state = AttentionState::Engaging(group_id);
                    self.engaged_group_id.store(group_id, Ordering::SeqCst);
                    self.last_activity_ms.store(now_ms(), Ordering::SeqCst);
                    info!(group_id, "attention: engaging after cooldown");
                    true
                } else if gid == group_id {
                    // Still in cooldown for this group
                    false
                } else {
                    // Cooldown for another group — can't engage
                    false
                }
            }
        }
    }

    /// Confirm engagement (transition from Engaging -> Engaged).
    pub async fn confirm_engagement(&self, group_id: i64) {
        let mut state = self.state.write().await;
        if *state == AttentionState::Engaging(group_id) {
            *state = AttentionState::Engaged(group_id);
            info!(group_id, "attention: engaged");
        }
    }

    /// Start cooldown for the currently engaged group.
    pub async fn start_cooldown(&self) {
        let mut state = self.state.write().await;
        let group_id = self.engaged_group_id.load(Ordering::SeqCst);
        let until = now_ms() + self.cooldown_duration_ms;
        *state = AttentionState::Cooldown(group_id, until as u64);
        self.cooldown_until_ms.store(until, Ordering::SeqCst);
        info!(group_id, "attention: cooldown until {until}");
    }

    /// Get current attention state (read-only).
    pub async fn get_state(&self) -> AttentionState {
        self.state.read().await.clone()
    }

    /// Check if we can process a message from the given group.
    /// Updates last activity timestamp if it's the engaged group.
    pub async fn can_process(&self, group_id: i64) -> bool {
        let state = self.state.read().await;
        match *state {
            AttentionState::Idle => true,
            AttentionState::Engaging(gid) | AttentionState::Engaged(gid) => {
                gid == group_id
            }
            AttentionState::Cooldown(_, _) => false,
        }
    }

    /// Tick to check for idle timeout — auto-disengage if idle too long.
    pub async fn tick_idle(&self) {
        let state = self.state.read().await;
        match *state {
            AttentionState::Engaged(gid) | AttentionState::Engaging(gid) => {
                let last = self.last_activity_ms.load(Ordering::SeqCst);
                if last > 0 && (now_ms() - last) > self.idle_timeout_ms {
                    drop(state);
                    let mut state = self.state.write().await;
                    if matches!(*state, AttentionState::Engaged(_) | AttentionState::Engaging(_)) {
                        let until = now_ms() + self.cooldown_duration_ms;
                        *state = AttentionState::Cooldown(gid, until as u64);
                        self.cooldown_until_ms.store(until, Ordering::SeqCst);
                        info!(group_id = gid, "attention: idle timeout, cooldown until {until}");
                    }
                }
            }
            _ => {}
        }
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_idle_to_engaged() {
        let store = InMemoryAttentionStore::new(10, 60);
        assert!(store.try_engage(100).await);
        assert!(matches!(store.get_state().await, AttentionState::Engaging(100)));
    }

    #[tokio::test]
    async fn test_second_group_rejected_when_engaged() {
        let store = InMemoryAttentionStore::new(10, 60);
        assert!(store.try_engage(100).await);
        assert!(!store.try_engage(200).await); // different group rejected
    }

    #[tokio::test]
    async fn test_same_group_allowed_when_engaged() {
        let store = InMemoryAttentionStore::new(10, 60);
        assert!(store.try_engage(100).await);
        assert!(store.try_engage(100).await); // same group allowed
    }

    #[tokio::test]
    async fn test_cooldown_blocks_engagement() {
        let store = InMemoryAttentionStore::new(60, 60); // 60s cooldown
        assert!(store.try_engage(100).await);
        store.confirm_engagement(100).await;
        store.start_cooldown().await;
        // Should be blocked by cooldown
        assert!(!store.try_engage(100).await);
    }
}
