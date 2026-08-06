//! OneBot 应用层 Heartbeat / Lifecycle 处理（B1）。
//!
//! 关键约束：
//! - WebSocket Ping/Pong 不等价于 OneBot `meta_event/heartbeat`；高频 Heartbeat
//!   不得当普通聊天消息持久化。
//! - 用 `tokio::select!` 同时监听 WebSocket 消息、Heartbeat deadline 与 shutdown。
//! - Heartbeat 超时后主动结束当前监听连接，返回类型化错误，由宿主进入 Epoch/Gap/Backfill。
//! - Heartbeat interval 设合理上下界，拒绝 0、负数、溢出和异常巨大值。
//! - 首次 Heartbeat 有启动宽限；旧版本兼容策略可配置（默认宽松，不因未见到首个
//!   Heartbeat 就热重连）。
//! - 普通文本流量不能错误掩盖已启用的 OneBot Heartbeat 超时。

use std::time::Duration;

use serde::Deserialize;

/// OneBot 生命周期状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LifecycleState {
    /// 已建立 WebSocket，尚未收到首个 Heartbeat / Lifecycle。
    Connected,
    /// 收到 lifecycle（connect），但尚未收到 Heartbeat。
    LifecycleReceived,
    /// 已收到至少一个 Heartbeat，进入稳态监控。
    Heartbeating,
    /// 连接结束（关闭或超时）。
    Ended,
}

/// OneBot `meta_event` 类型化载荷。Heartbeat 可能携带 `interval`、`status` 等；
/// Lifecycle 携带 `sub_type`（enable/connect/disable）。
#[derive(Debug, Clone)]
pub enum MetaEvent {
    /// `meta_event_type=heartbeat`。interval 为声明的心跳间隔（秒），缺失视为未知。
    Heartbeat { interval_secs: Option<u64> },
    /// `meta_event_type=lifecycle`。
    Lifecycle { sub_type: String },
}

/// 从 OneBot 原始事件解析 `meta_event`。非 meta_event 返回 None。
pub fn parse_meta_event(raw: &serde_json::Value) -> Option<MetaEvent> {
    if raw.get("post_type").and_then(serde_json::Value::as_str) != Some("meta_event") {
        return None;
    }
    match raw
        .get("meta_event_type")
        .and_then(serde_json::Value::as_str)
    {
        Some("heartbeat") => {
            let interval_secs = raw
                .get("interval")
                .and_then(serde_json::Value::as_u64)
                .map(|secs| secs / 1000);
            Some(MetaEvent::Heartbeat { interval_secs })
        }
        Some("lifecycle") => {
            let sub_type = raw
                .get("sub_type")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("")
                .to_string();
            Some(MetaEvent::Lifecycle { sub_type })
        }
        _ => None,
    }
}

/// Heartbeat 监控配置。所有时间字段均设合理上下界，拒绝异常值。
/// 评审第三轮 P1-3：实现 `Deserialize` 与 `Default`，使 QQBot 独立 TOML
/// `[napcat.heartbeat]` 可加载；缺失时使用默认宽松配置。
/// 评审第四轮 P2：`deny_unknown_fields` 拒绝拼写错误（如 `enable=false` 误写为
/// `enabled=true` 默认），防止安全相关配置静默回退到默认值导致意外重连。
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HeartbeatConfig {
    /// 是否启用 OneBot Heartbeat 超时监控。禁用时仍解析 meta_event，但不据此重连。
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    /// 收到首个 Heartbeat 的启动宽限期（秒）。
    #[serde(default = "default_startup_grace_secs")]
    pub startup_grace_secs: u64,
    /// Heartbeat interval 下界（秒），声明值低于此值按此值计算 deadline。
    #[serde(default = "default_min_interval_secs")]
    pub min_interval_secs: u64,
    /// Heartbeat interval 上界（秒），声明值高于此值按此值计算 deadline。
    #[serde(default = "default_max_interval_secs")]
    pub max_interval_secs: u64,
    /// Heartbeat 缺失声明 interval 时使用的默认间隔（秒）。
    #[serde(default = "default_default_interval_secs")]
    pub default_interval_secs: u64,
    /// 超时容忍倍数：deadline = interval * multiplier + grace。
    #[serde(default = "default_timeout_multiplier")]
    pub timeout_multiplier: u32,
}

fn default_enabled() -> bool {
    true
}
fn default_startup_grace_secs() -> u64 {
    300
}
fn default_min_interval_secs() -> u64 {
    5
}
fn default_max_interval_secs() -> u64 {
    300
}
fn default_default_interval_secs() -> u64 {
    30
}
fn default_timeout_multiplier() -> u32 {
    3
}

impl Default for HeartbeatConfig {
    fn default() -> Self {
        // 默认宽松：启用监控，但启动宽限 300s 给足，避免不发送 Heartbeat 的兼容实现
        // 在 startup_grace 内被频繁热重连（评审 P1：与文档"未见首个 Heartbeat 不热重连"一致）。
        // 真正不发送 Heartbeat 的实现建议显式配置 enabled=false。
        Self {
            enabled: true,
            startup_grace_secs: 300,
            min_interval_secs: 5,
            max_interval_secs: 300,
            default_interval_secs: 30,
            timeout_multiplier: 3,
        }
    }
}

impl HeartbeatConfig {
    /// 校验配置边界，拒绝 0、负数、溢出和异常巨大值（评审 P1：注释须与实现一致）。
    /// 所有时间字段上限 3600s，避免异常巨大值导致 deadline 失效或连接长期假在线。
    pub fn validate(&self) -> Result<(), String> {
        const MAX_TIME_BOUND_SECS: u64 = 3600;
        const MAX_MULTIPLIER: u32 = 10;
        if self.startup_grace_secs == 0 || self.startup_grace_secs > MAX_TIME_BOUND_SECS {
            return Err(format!(
                "heartbeat.startup_grace_secs must be in 1..={MAX_TIME_BOUND_SECS}"
            ));
        }
        if self.min_interval_secs == 0
            || self.min_interval_secs > MAX_TIME_BOUND_SECS
            || self.max_interval_secs < self.min_interval_secs
            || self.max_interval_secs > MAX_TIME_BOUND_SECS
        {
            return Err(format!(
                "heartbeat min/max interval must be in 1..={MAX_TIME_BOUND_SECS} and max >= min"
            ));
        }
        if self.default_interval_secs < self.min_interval_secs
            || self.default_interval_secs > self.max_interval_secs
        {
            return Err("heartbeat.default_interval_secs out of [min, max]".into());
        }
        if self.timeout_multiplier == 0 || self.timeout_multiplier > MAX_MULTIPLIER {
            return Err(format!(
                "heartbeat.timeout_multiplier must be in 1..={MAX_MULTIPLIER}"
            ));
        }
        Ok(())
    }

    /// 把声明 interval 规整到上下界内。
    fn clamp_interval(&self, declared: Option<u64>) -> u64 {
        match declared {
            Some(secs) => secs.clamp(self.min_interval_secs, self.max_interval_secs),
            None => self.default_interval_secs,
        }
    }
}

/// Heartbeat 监控状态机。
#[derive(Debug)]
pub struct HeartbeatState {
    config: HeartbeatConfig,
    lifecycle: LifecycleState,
    declared_interval_secs: u64,
    /// 单调时钟基准（秒），由 `tokio::time::Instant` 转换。
    last_heartbeat_at: Option<tokio::time::Instant>,
    last_business_event_at: Option<tokio::time::Instant>,
    connected_at: tokio::time::Instant,
}

impl HeartbeatState {
    pub fn new(config: HeartbeatConfig) -> Self {
        Self {
            config,
            lifecycle: LifecycleState::Connected,
            declared_interval_secs: config.default_interval_secs,
            last_heartbeat_at: None,
            last_business_event_at: None,
            connected_at: tokio::time::Instant::now(),
        }
    }

    pub fn lifecycle(&self) -> LifecycleState {
        self.lifecycle
    }

    pub fn last_heartbeat_at(&self) -> Option<tokio::time::Instant> {
        self.last_heartbeat_at
    }

    pub fn last_business_event_at(&self) -> Option<tokio::time::Instant> {
        self.last_business_event_at
    }

    /// 处理 meta_event，更新生命周期与声明 interval。
    pub fn observe_meta(&mut self, event: &MetaEvent) {
        match event {
            MetaEvent::Lifecycle { sub_type } => {
                if matches!(self.lifecycle, LifecycleState::Connected) {
                    self.lifecycle = LifecycleState::LifecycleReceived;
                }
                tracing::debug!(sub_type = %sub_type, "OneBot lifecycle 事件");
            }
            MetaEvent::Heartbeat { interval_secs } => {
                self.declared_interval_secs = self.config.clamp_interval(*interval_secs);
                self.last_heartbeat_at = Some(tokio::time::Instant::now());
                self.lifecycle = LifecycleState::Heartbeating;
                // 高频 Heartbeat 不得每次打 info 日志。
                tracing::trace!(
                    interval_secs = self.declared_interval_secs,
                    "OneBot heartbeat 续期"
                );
            }
        }
    }

    /// 记录业务事件时间（非 meta_event 的消息/通知）。
    /// 普通文本流量不能错误掩盖已启用的 Heartbeat 超时：这里只更新业务时间戳，
    /// 不重置 Heartbeat deadline。
    pub fn observe_business_event(&mut self) {
        self.last_business_event_at = Some(tokio::time::Instant::now());
    }

    /// 计算下一次 Heartbeat deadline 的状态。
    ///
    /// 三态结果避免把"超时已过期"误判为"监控禁用"（评审 P0-3）：
    /// - [`HeartbeatDeadline::Disabled`]：监控未启用，监听器应永不据此重连。
    /// - [`HeartbeatDeadline::Waiting(dur)`]：距下次超时还剩 `dur`，监听器应等待该时长。
    /// - [`HeartbeatDeadline::Expired`]：deadline 已过，监听器必须立即返回
    ///   [`NapCatError::HeartbeatTimeout`]，即使期间处理过业务事件（业务事件不重置
    ///   Heartbeat deadline，故不能掩盖已启用的超时）。
    ///
    /// 规则：
    /// - 未收到首个 Heartbeat 时，deadline = startup_grace（从连接建立起算）。
    /// - 已进入 Heartbeating 时，deadline = interval * multiplier（从最近一次 Heartbeat 起算）。
    pub fn heartbeat_deadline(&self) -> HeartbeatDeadline {
        if !self.config.enabled {
            return HeartbeatDeadline::Disabled;
        }
        if self.lifecycle == LifecycleState::Ended {
            return HeartbeatDeadline::Disabled;
        }
        let interval = Duration::from_secs(self.declared_interval_secs);
        let grace = Duration::from_secs(self.config.startup_grace_secs);
        let multiplier = self.config.timeout_multiplier;
        let deadline_offset = match self.lifecycle {
            LifecycleState::Connected | LifecycleState::LifecycleReceived => grace,
            LifecycleState::Heartbeating => {
                interval.checked_mul(multiplier).unwrap_or(grace).max(grace)
            }
            LifecycleState::Ended => return HeartbeatDeadline::Disabled,
        };
        let base = self.last_heartbeat_at.unwrap_or(self.connected_at);
        let absolute = base + deadline_offset;
        match absolute.checked_duration_since(tokio::time::Instant::now()) {
            Some(remaining) if remaining.is_zero() => HeartbeatDeadline::Expired,
            Some(remaining) => HeartbeatDeadline::Waiting(remaining),
            // absolute 已在现在之前（checked_duration_since 返回 None）：超时已发生。
            None => HeartbeatDeadline::Expired,
        }
    }
}

/// Heartbeat deadline 的三态结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeartbeatDeadline {
    /// 监控禁用：监听器不应据此重连（用 pending future 永不触发）。
    Disabled,
    /// 距下次超时还剩的时长。
    Waiting(Duration),
    /// deadline 已过：监听器必须立即返回 HeartbeatTimeout。
    Expired,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> HeartbeatConfig {
        HeartbeatConfig {
            enabled: true,
            startup_grace_secs: 60,
            min_interval_secs: 5,
            max_interval_secs: 300,
            default_interval_secs: 30,
            timeout_multiplier: 3,
        }
    }

    #[test]
    fn parse_meta_event_extracts_heartbeat_and_lifecycle() {
        let hb = parse_meta_event(&serde_json::json!({
            "post_type":"meta_event","meta_event_type":"heartbeat","interval":30000
        }));
        assert!(matches!(
            hb,
            Some(MetaEvent::Heartbeat {
                interval_secs: Some(30)
            })
        ));

        let lc = parse_meta_event(&serde_json::json!({
            "post_type":"meta_event","meta_event_type":"lifecycle","sub_type":"connect"
        }));
        match lc {
            Some(MetaEvent::Lifecycle { sub_type }) => assert_eq!(sub_type, "connect"),
            other => panic!("expected lifecycle, got {other:?}"),
        }

        // 非.meta_event 返回 None。
        assert!(parse_meta_event(&serde_json::json!({"post_type":"message"})).is_none());
        // 未知 meta_event_type 返回 None。
        assert!(
            parse_meta_event(&serde_json::json!({
                "post_type":"meta_event","meta_event_type":"other"
            }))
            .is_none()
        );
    }

    #[test]
    fn config_rejects_zero_and_inverted_bounds() {
        let mut c = cfg();
        c.startup_grace_secs = 0;
        assert!(c.validate().is_err());

        let mut c = cfg();
        c.max_interval_secs = 1;
        assert!(c.validate().is_err());

        let mut c = cfg();
        c.timeout_multiplier = 0;
        assert!(c.validate().is_err());
    }

    #[test]
    fn heartbeat_state_transitions_and_clamps_interval() {
        let mut s = HeartbeatState::new(cfg());
        assert_eq!(s.lifecycle(), LifecycleState::Connected);

        // 声明 2s 低于下界 5s，应被钳制为 5s。
        s.observe_meta(&MetaEvent::Heartbeat {
            interval_secs: Some(2),
        });
        assert_eq!(s.lifecycle(), LifecycleState::Heartbeating);
        assert!(s.last_heartbeat_at().is_some());

        // 声明 100000s 高于上界 300s，应被钳制为 300s。
        s.observe_meta(&MetaEvent::Heartbeat {
            interval_secs: Some(100000),
        });
        assert_eq!(s.declared_interval_secs, 300);
    }

    #[test]
    fn disabled_config_returns_disabled_deadline() {
        let mut c = cfg();
        c.enabled = false;
        let s = HeartbeatState::new(c);
        assert!(matches!(
            s.heartbeat_deadline(),
            HeartbeatDeadline::Disabled
        ));
    }

    #[test]
    fn expired_deadline_is_reported_not_disabled() {
        // 评审 P0-3：deadline 已过期时必须返回 Expired，不能与 Disabled 混淆。
        // 构造一个 0 启动宽限 + 极短 interval 的配置，收到心跳后立即过期。
        let mut c = cfg();
        c.startup_grace_secs = 1;
        c.min_interval_secs = 1;
        c.default_interval_secs = 1;
        c.max_interval_secs = 1;
        c.timeout_multiplier = 1;
        let s = HeartbeatState::new(c);
        // 等待超过 startup_grace（1s），未收心跳 -> Expired。
        std::thread::sleep(std::time::Duration::from_millis(1100));
        assert!(matches!(s.heartbeat_deadline(), HeartbeatDeadline::Expired));
    }

    #[test]
    fn business_event_does_not_reset_heartbeat_deadline() {
        let mut s = HeartbeatState::new(cfg());
        s.observe_meta(&MetaEvent::Heartbeat {
            interval_secs: Some(5),
        });
        let d1 = s.heartbeat_deadline();
        // 业务事件不应改变 Heartbeat deadline 基准。
        s.observe_business_event();
        let d2 = s.heartbeat_deadline();
        // 两者都应是 Waiting（非 Expired、非 Disabled），且剩余时长接近。
        assert!(matches!(d1, HeartbeatDeadline::Waiting(_)));
        assert!(matches!(d2, HeartbeatDeadline::Waiting(_)));
        assert!(s.last_business_event_at().is_some());
    }
}
