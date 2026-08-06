//! 协议无关的健康状态领域模型。
//!
//! 本模块描述类型化健康状态、子系统快照和聚合规则，不依赖 NapCat、OneBot、QQ、
//! SeaORM、MySQL、Axum、Tokio 或任何 HTTP 客户端。
//!
//! 核心约束（任务九）：
//! - 状态区分 healthy/degraded/uncertain/unavailable。
//! - 禁止把 WebSocket 已连接等同于历史完整。
//! - 禁止把 Worker 执行完等同于 verified complete。
//! - 禁止把 Deferred 显示为 Unavailable。
//! - 禁止把临时错误固化为永久不支持。
//! - 每次读取健康状态不调用昂贵外部 API（使用有界缓存快照）。

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// 健康状态四态（任务九）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HealthStatus {
    /// 健康：所有子系统正常。
    Healthy,
    /// 降级：部分子系统不可用或降级，但核心功能仍可用。
    Degraded,
    /// 不确定：子系统状态无法确认（如 Deferred、Unknown）。
    Uncertain,
    /// 不可用：核心子系统不可用。
    Unavailable,
}

impl HealthStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Healthy => "healthy",
            Self::Degraded => "degraded",
            Self::Uncertain => "uncertain",
            Self::Unavailable => "unavailable",
        }
    }

    /// 聚合多个子系统状态：取最差状态。
    ///
    /// 优先级（从好到差）：Healthy < Degraded < Uncertain < Unavailable。
    /// 但 `Deferred` 不算 `Unavailable`（任务九：禁止把 Deferred 显示为 Unavailable）。
    pub fn aggregate(statuses: &[HealthStatus]) -> Self {
        if statuses.is_empty() {
            return Self::Uncertain;
        }
        let mut worst = Self::Healthy;
        for status in statuses {
            worst = match (worst, *status) {
                (Self::Unavailable, _) | (_, Self::Unavailable) => Self::Unavailable,
                (Self::Uncertain, _) | (_, Self::Uncertain) => Self::Uncertain,
                (Self::Degraded, _) | (_, Self::Degraded) => Self::Degraded,
                (Self::Healthy, Self::Healthy) => Self::Healthy,
            };
        }
        worst
    }
}

/// 单个子系统的健康快照。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubsystemHealth {
    /// 子系统名称（如 "napcat_http"、"websocket"、"mysql"）。
    pub name: String,
    pub status: HealthStatus,
    /// 最近成功时间（Unix 秒）。
    pub last_success_at_unix_secs: Option<i64>,
    /// 最近错误描述（有界，不记录完整正文/凭据/签名 URL）。
    pub last_error: Option<String>,
    /// 固定键、无标签的有界运行指标；禁止放入身份、路径或正文。
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub metrics: BTreeMap<String, u64>,
}

/// 完整健康快照：聚合所有子系统状态。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HealthSnapshot {
    /// 聚合状态（最差子系统状态）。
    pub overall_status: HealthStatus,
    /// 各子系统状态列表。
    pub subsystems: Vec<SubsystemHealth>,
    /// 快照时间（Unix 秒）。
    pub snapshot_at_unix_secs: i64,
    /// 稳定 ID 携带（任务九：account_id/connection_epoch_id/gap_id/snapshot_id/source_event_id/artifact_id）。
    pub account_id: Option<String>,
    pub connection_epoch_id: Option<String>,
}

impl HealthSnapshot {
    /// 从子系统列表构造快照并聚合状态。
    pub fn new(subsystems: Vec<SubsystemHealth>, snapshot_at_unix_secs: i64) -> Self {
        let overall =
            HealthStatus::aggregate(&subsystems.iter().map(|s| s.status).collect::<Vec<_>>());
        Self {
            overall_status: overall,
            subsystems,
            snapshot_at_unix_secs,
            account_id: None,
            connection_epoch_id: None,
        }
    }

    /// 携带稳定 ID（任务九）。
    pub fn with_account_id(mut self, account_id: impl Into<String>) -> Self {
        self.account_id = Some(account_id.into());
        self
    }

    pub fn with_connection_epoch_id(mut self, epoch_id: impl Into<String>) -> Self {
        self.connection_epoch_id = Some(epoch_id.into());
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aggregate_returns_worst_status() {
        assert_eq!(
            HealthStatus::aggregate(&[HealthStatus::Healthy, HealthStatus::Healthy]),
            HealthStatus::Healthy
        );
        assert_eq!(
            HealthStatus::aggregate(&[HealthStatus::Healthy, HealthStatus::Degraded]),
            HealthStatus::Degraded
        );
        assert_eq!(
            HealthStatus::aggregate(&[HealthStatus::Degraded, HealthStatus::Uncertain]),
            HealthStatus::Uncertain
        );
        assert_eq!(
            HealthStatus::aggregate(&[HealthStatus::Uncertain, HealthStatus::Unavailable]),
            HealthStatus::Unavailable
        );
        // 单个最差状态决定整体。
        assert_eq!(
            HealthStatus::aggregate(&[
                HealthStatus::Healthy,
                HealthStatus::Healthy,
                HealthStatus::Unavailable
            ]),
            HealthStatus::Unavailable
        );
    }

    #[test]
    fn aggregate_empty_returns_uncertain() {
        assert_eq!(HealthStatus::aggregate(&[]), HealthStatus::Uncertain);
    }

    #[test]
    fn snapshot_aggregates_subsystems() {
        let snapshot = HealthSnapshot::new(
            vec![
                SubsystemHealth {
                    name: "napcat_http".into(),
                    status: HealthStatus::Healthy,
                    last_success_at_unix_secs: Some(1000),
                    last_error: None,
                    metrics: BTreeMap::new(),
                },
                SubsystemHealth {
                    name: "websocket".into(),
                    status: HealthStatus::Degraded,
                    last_success_at_unix_secs: Some(900),
                    last_error: Some("reconnecting".into()),
                    metrics: BTreeMap::new(),
                },
            ],
            1000,
        );
        // 最差状态 = Degraded。
        assert_eq!(snapshot.overall_status, HealthStatus::Degraded);
        assert_eq!(snapshot.subsystems.len(), 2);
    }

    #[test]
    fn connected_is_not_equal_to_history_complete() {
        // 任务九：禁止把 WebSocket 已连接等同于历史完整。
        // WebSocket 连接健康（Healthy）不应自动推导历史完整性为 Healthy。
        let ws_connected = SubsystemHealth {
            name: "websocket".into(),
            status: HealthStatus::Healthy,
            last_success_at_unix_secs: Some(1000),
            last_error: None,
            metrics: BTreeMap::new(),
        };
        let history_completeness = SubsystemHealth {
            name: "history_completeness".into(),
            status: HealthStatus::Uncertain, // 真实 NapCat 无法证明完整
            last_success_at_unix_secs: None,
            last_error: Some("account conversation set unprovable".into()),
            metrics: BTreeMap::new(),
        };
        let snapshot = HealthSnapshot::new(vec![ws_connected, history_completeness], 1000);
        // 整体应为 Uncertain（历史完整性不确定），不能因 WS 连接就标 Healthy。
        assert_eq!(snapshot.overall_status, HealthStatus::Uncertain);
    }
}
