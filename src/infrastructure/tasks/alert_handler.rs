use std::collections::HashMap;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use tokio::sync::RwLock;
use tracing::{info, warn};

use crate::domain::tasks::task_event::TaskEvent;
use crate::domain::tasks::task_handler::TaskHandler;

/// Configuration for the alert handler.
#[derive(Debug, Clone)]
pub struct AlertConfig {
    /// Webhook URL for alert delivery (e.g. Slack, DingTalk, Feishu).
    pub webhook_url: Option<String>,
    /// Minimum interval between alerts for the same user (seconds), to avoid spam.
    pub min_interval_secs: u64,
}

impl Default for AlertConfig {
    fn default() -> Self {
        Self {
            webhook_url: None,
            min_interval_secs: 300, // 5 minutes
        }
    }
}

/// Sends alerts for high-risk events via webhook and structured logging.
///
/// Monitored events:
/// - `RiskDetected` with Crisis / High level → immediate alert
/// - `LoginAudit` failures (if frequency threshold crossed → delegated to RateLimitHandler)
pub struct AlertHandler {
    config: AlertConfig,
    client: reqwest::Client,
    /// Tracks last alert time per (event_type, user_id) to suppress duplicates.
    last_alert: RwLock<HashMap<String, Instant>>,
}

impl AlertHandler {
    pub fn new(config: AlertConfig) -> Self {
        Self {
            config,
            client: reqwest::Client::new(),
            last_alert: RwLock::new(HashMap::new()),
        }
    }

    /// Check whether an alert for the given key should be suppressed.
    async fn should_throttle(&self, key: &str) -> bool {
        let map = self.last_alert.read().await;
        if let Some(last) = map.get(key) {
            if last.elapsed() < Duration::from_secs(self.config.min_interval_secs) {
                return true;
            }
        }
        false
    }

    /// Record that an alert was just sent.
    async fn record_alert(&self, key: String) {
        self.last_alert.write().await.insert(key, Instant::now());
    }

    async fn send_webhook(&self, title: &str, body: &str) {
        let Some(ref url) = self.config.webhook_url else {
            return;
        };

        let payload = serde_json::json!({
            "msgtype": "text",
            "text": {
                "content": format!("【{title}】\n{body}")
            }
        });

        match self
            .client
            .post(url)
            .json(&payload)
            .timeout(Duration::from_secs(5))
            .send()
            .await
        {
            Ok(resp) if resp.status().is_success() => {
                info!("alert delivered: {title}");
            }
            Ok(resp) => {
                warn!(status = %resp.status(), "webhook rejected: {title}");
            }
            Err(e) => {
                warn!(error = %e, "webhook failed: {title}");
            }
        }
    }

    /// Clean up expired throttle records (call periodically).
    pub async fn cleanup(&self) {
        let threshold = Duration::from_secs(self.config.min_interval_secs);
        let mut map = self.last_alert.write().await;
        map.retain(|_, v| v.elapsed() < threshold);
    }
}

#[async_trait]
impl TaskHandler for AlertHandler {
    async fn handle(&self, event: &TaskEvent) {
        match event {
            TaskEvent::RiskDetected(t) => {
                let level = &t.risk_level;
                if level != "Crisis" && level != "High" {
                    return;
                }

                let key = format!("risk:{}:{}", t.user_id, level);
                if self.should_throttle(&key).await {
                    return;
                }
                self.record_alert(key.clone()).await;

                let title = format!(
                    "{} 风险预警",
                    if level == "Crisis" {
                        "🚨 危机"
                    } else {
                        "⚠️ 高危"
                    }
                );
                let body = format!(
                    "用户ID: {}\n风险等级: {}\n置信度: {:.0}%\n对话ID: {:?}",
                    t.user_id,
                    level,
                    t.confidence * 100.0,
                    t.conversation_id
                );

                warn!(user_id = t.user_id, risk_level = %level, confidence = t.confidence, "ALERT: {title}");
                self.send_webhook(&title, &body).await;
            }

            TaskEvent::LoginAudit(t) if !t.success => {
                let key = format!("login_fail:{}", t.username);
                // LoginAudit alerts are rate-limited by the caller;
                // here we just log at warn level — RateLimitHandler escalates.
                if !self.should_throttle(&key).await {
                    self.record_alert(key.clone()).await;
                    warn!(
                        username = %t.username,
                        reason = %t.reason.as_deref().unwrap_or("?"),
                        "login failure alert"
                    );
                }
            }

            _ => {}
        }
    }
}
