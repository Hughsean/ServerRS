//! Action Planner 后台扫描配置。

use serde::Deserialize;

use super::ConfigError;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct ActionPlannerConfig {
    pub enabled: bool,
    pub max_batches_per_scan: u32,
    pub lease_secs: u64,
    pub scan_interval_ms: u64,
    pub retry_initial_ms: u64,
    pub retry_max_ms: u64,
}

impl Default for ActionPlannerConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_batches_per_scan: 10,
            lease_secs: 60,
            scan_interval_ms: 2000,
            retry_initial_ms: 500,
            retry_max_ms: 10_000,
        }
    }
}

impl ActionPlannerConfig {
    pub(super) fn validate(&self) -> Result<(), ConfigError> {
        if self.max_batches_per_scan == 0 || self.max_batches_per_scan > 100 {
            return Err(ConfigError::Invalid(
                "action_planner.max_batches_per_scan must be between 1 and 100".into(),
            ));
        }
        if self.lease_secs == 0 || self.lease_secs > 3600 || self.scan_interval_ms == 0 {
            return Err(ConfigError::Invalid(
                "action_planner lease and scan interval must be positive and bounded".into(),
            ));
        }
        if self.retry_initial_ms == 0 || self.retry_max_ms < self.retry_initial_ms {
            return Err(ConfigError::Invalid(
                "action_planner retry delays must be positive and max >= initial".into(),
            ));
        }
        Ok(())
    }
}
