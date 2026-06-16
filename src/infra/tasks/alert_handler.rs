use std::collections::HashMap;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use tokio::sync::RwLock;
use tracing::{info, warn};

use crate::domain::risk::detection_types::RiskLevel;
use crate::domain::tasks::task_event::TaskEvent;
use crate::domain::tasks::task_handler::TaskHandler;

/// 告警处理的配置。
#[derive(Debug, Clone)]
pub struct AlertConfig {
    /// 告警投递的 Webhook URL（如 Slack、钉钉、飞书）。
    pub webhook_url: Option<String>,
    /// 同一用户的最短告警间隔（秒），避免重复推送。
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

/// 通过 Webhook 和结构化日志发送高风险事件告警。
///
/// 监控的事件：
/// - `RiskDetected` 危机/高风险级别 → 立即告警
/// - `LoginAudit` failures (if frequency threshold crossed → delegated to RateLimitHandler)
pub struct AlertHandler {
    config: AlertConfig,
    client: reqwest::Client,
    /// 跟踪每个 (事件类型, 用户ID) 的最后告警时间，用于去重。
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

    /// 检查给定 key 的告警是否应被抑制。
    async fn should_throttle(&self, key: &str) -> bool {
        let map = self.last_alert.read().await;
        if let Some(last) = map.get(key) {
            if last.elapsed() < Duration::from_secs(self.config.min_interval_secs) {
                return true;
            }
        }
        false
    }

    /// 记录刚刚发送的告警。
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
                info!("告警已投递: {title}");
            }
            Ok(resp) => {
                warn!(status = %resp.status(), "Webhook 拒绝: {title}");
            }
            Err(e) => {
                warn!(error = %e, "Webhook 失败: {title}");
            }
        }
    }

    /// 清理过期的节流记录（定期调用）。
    pub async fn cleanup(&self) {
        let threshold = Duration::from_secs(self.config.min_interval_secs);
        let mut map = self.last_alert.write().await;
        map.retain(|_, v| v.elapsed() < threshold);
    }
}

/// 人类可读的风险等级标签。
fn risk_level_label(level: RiskLevel) -> &'static str {
    match level {
        RiskLevel::Crisis => "Crisis",
        RiskLevel::High => "High",
        RiskLevel::Medium => "Medium",
        RiskLevel::Low => "Low",
        RiskLevel::None => "None",
        RiskLevel::Unknown => "Unknown",
    }
}

#[async_trait]
impl TaskHandler for AlertHandler {
    fn name(&self) -> &str {
        "AlertHandler"
    }

    async fn handle(&self, event: &TaskEvent) {
        match event {
            TaskEvent::RiskDetected(t) => {
                if t.risk_level != RiskLevel::Crisis && t.risk_level != RiskLevel::High {
                    return;
                }

                let level_label = risk_level_label(t.risk_level);
                let key = format!("risk:{}:{:?}", t.user_id, t.risk_level);
                if self.should_throttle(&key).await {
                    return;
                }
                self.record_alert(key.clone()).await;

                let emoji = if t.risk_level == RiskLevel::Crisis {
                    "🚨 危机"
                } else {
                    "⚠️ 高危"
                };
                let title = format!("{emoji} 风险预警");
                let body = format!(
                    "用户ID: {}\n风险等级: {}\n置信度: {:.0}%\n对话ID: {:?}",
                    t.user_id,
                    level_label,
                    t.confidence * 100.0,
                    t.conversation_id
                );

                warn!(user_id = t.user_id, risk_level = ?t.risk_level, confidence = t.confidence, "告警: {title}");
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
