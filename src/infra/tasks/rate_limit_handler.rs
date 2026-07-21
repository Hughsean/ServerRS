use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use tokio::sync::RwLock;
use tracing::warn;

use crate::domain::tasks::task_event::TaskEvent;
use crate::domain::tasks::task_handler::TaskHandler;
use crate::domain::user::user::UserStatus;
use crate::domain::user::user_repo::UserRepoT;

/// 速率限制/暴力破解防护处理的配置。
#[derive(Debug, Clone)]
pub struct RateLimitConfig {
    /// 触发锁定前的最大失败登录次数。
    pub max_failures: usize,
    /// 计数失败次数的时间窗口（秒）。
    pub window_secs: u64,
    /// 锁定账号的持续时间（秒）。0 表示不自动解锁。
    pub lock_duration_secs: u64,
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        Self {
            max_failures: 5,
            window_secs: 300,         // 5 minutes
            lock_duration_secs: 1800, // 30 minutes
        }
    }
}

/// 跟踪每个账号的登录失败次数，并在超过阈值时锁定账号。
///
/// Uses in-memory state. On lock, updates the user's status via `UserRepository`.
pub struct RateLimitHandler {
    config: RateLimitConfig,
    user_repo: Arc<dyn UserRepoT>,
    /// username → list of failure timestamps
    failures: RwLock<HashMap<String, Vec<Instant>>>,
    /// username → locked_until Instant
    locked: RwLock<HashMap<String, Instant>>,
}

impl RateLimitHandler {
    pub fn new(config: RateLimitConfig, user_repo: Arc<dyn UserRepoT>) -> Self {
        Self {
            config,
            user_repo,
            failures: RwLock::new(HashMap::new()),
            locked: RwLock::new(HashMap::new()),
        }
    }

    /// 检查账号当前是否被锁定。
    pub async fn is_locked(&self, username: &str) -> bool {
        let locked = self.locked.read().await;
        if let Some(until) = locked.get(username) {
            if *until > Instant::now() {
                return true;
            }
        }
        false
    }

    /// 手动解锁账号。
    pub async fn unlock(&self, username: &str) {
        self.locked.write().await.remove(username);
    }

    /// 清理过期的条目（定期调用）。
    pub async fn cleanup(&self) {
        let now = Instant::now();
        {
            let mut failures = self.failures.write().await;
            let window = Duration::from_secs(self.config.window_secs);
            for (_, timestamps) in failures.iter_mut() {
                timestamps.retain(|t| now - *t < window);
            }
            failures.retain(|_, v| !v.is_empty());
        }
        {
            let mut locked = self.locked.write().await;
            locked.retain(|_, until| *until > now);
        }
    }
}

#[async_trait]
impl TaskHandler for RateLimitHandler {
    fn name(&self) -> &str {
        "RateLimitHandler"
    }

    async fn handle(&self, event: &TaskEvent) {
        match event {
            TaskEvent::LoginAudit(t) if !t.success => {
                let now = Instant::now();
                let window = Duration::from_secs(self.config.window_secs);

                // Record failure
                {
                    let mut failures = self.failures.write().await;
                    let entry = failures.entry(t.username.clone()).or_default();
                    entry.retain(|ts| now - *ts < window);
                    entry.push(now);

                    let count = entry.len();
                    if count < self.config.max_failures {
                        // Not at threshold yet — just track
                        return;
                    }
                    // Threshold reached — clear failures to avoid repeated locking
                    entry.clear();
                }

                // Lock the account
                warn!(
                    username = %t.username,
                    failures = self.config.max_failures,
                    window_secs = self.config.window_secs,
                    "超出速率限制：正在锁定账号"
                );

                {
                    let mut locked = self.locked.write().await;
                    locked.insert(
                        t.username.clone(),
                        now + Duration::from_secs(self.config.lock_duration_secs),
                    );
                }

                // Persist lock to database
                if let Some(user) = self
                    .user_repo
                    .find_by_username(&t.username)
                    .await
                    .ok()
                    .flatten()
                {
                    use crate::domain::user::user::UserUpdate;
                    let _ = self
                        .user_repo
                        .update(
                            user.id,
                            UserUpdate {
                                email: None,
                                phone: None,
                                nickname: None,
                                status: Some(UserStatus::Disabled),
                                role: None,
                            },
                        )
                        .await;
                    warn!(user_id = user.id, username = %t.username, "账号因速率限制已被锁定");
                }
            }

            // On successful login, clear failure history for that user
            TaskEvent::LoginAudit(t) if t.success => {
                self.failures.write().await.remove(&t.username);
            }

            _ => {}
        }
    }
}
