use chrono::{DateTime, Duration, Utc};
use serde::Serialize;

use crate::app::fresh_context::config::FreshContextUseCaseConfig;
use crate::domain::fresh_context::{FreshChunk, FreshItem, FreshSource, FreshTopic, source_kind};

#[derive(Debug, Clone)]
pub struct FreshContextPolicy {
    config: FreshContextUseCaseConfig,
}

#[derive(Debug, Clone, Serialize)]
pub struct FreshChunkPayload {
    pub kind: &'static str,
    pub vector_id: String,
    pub fresh_chunk_id: u64,
    pub fresh_item_id: u64,
    pub topic_id: Option<u64>,
    pub source_id: u64,
    pub source_kind: String,
    pub trust_level: String,
    pub rumor_level: String,
    pub active: bool,
    pub published_at_ts: Option<i64>,
    pub fetched_at_ts: i64,
    pub expires_at_ts: i64,
    pub reliability_score: f64,
    pub freshness_score: f64,
    pub heat_score: f64,
    pub risk_flags: serde_json::Value,
}

impl FreshContextPolicy {
    pub fn new(config: FreshContextUseCaseConfig) -> Self {
        Self { config }
    }

    pub fn ttl_for_source_kind(&self, kind: &str) -> Duration {
        let secs = match kind {
            source_kind::TREND | source_kind::SOCIAL => self.config.trend_ttl_secs,
            source_kind::GOSSIP | source_kind::FORUM => self.config.gossip_ttl_secs,
            source_kind::NEWS | source_kind::RSS | source_kind::SEARCH => self.config.news_ttl_secs,
            _ => self.config.background_ttl_secs,
        };
        Duration::seconds(secs.min(i64::MAX as u64) as i64)
    }

    pub fn expires_at(&self, fetched_at: DateTime<Utc>, source_kind: &str) -> DateTime<Utc> {
        fetched_at + self.ttl_for_source_kind(source_kind)
    }

    pub fn rank_score(
        &self,
        semantic_score: f64,
        freshness_score: f64,
        reliability_score: f64,
        heat_score: f64,
    ) -> f64 {
        let sum = self.config.semantic_weight
            + self.config.freshness_weight
            + self.config.reliability_weight
            + self.config.heat_weight;
        if sum <= 0.0 || !sum.is_finite() {
            return semantic_score;
        }
        (semantic_score * self.config.semantic_weight
            + freshness_score * self.config.freshness_weight
            + reliability_score * self.config.reliability_weight
            + heat_score * self.config.heat_weight)
            / sum
    }

    pub fn source_is_eligible(&self, source: &FreshSource) -> bool {
        source.enabled == 1
            && source.deleted_at.is_none()
            && source.reliability_score >= self.config.min_reliability_score
    }

    pub fn build_payload(
        &self,
        vector_id: String,
        source: &FreshSource,
        item: &FreshItem,
        topic: Option<&FreshTopic>,
        chunk: &FreshChunk,
    ) -> FreshChunkPayload {
        FreshChunkPayload {
            kind: "fresh_chunk",
            vector_id,
            fresh_chunk_id: chunk.id,
            fresh_item_id: item.id,
            topic_id: topic.map(|t| t.id).or(chunk.topic_id),
            source_id: source.id,
            source_kind: source.source_kind.clone(),
            trust_level: source.trust_level.clone(),
            rumor_level: item.rumor_level.clone(),
            active: chunk.active == 1,
            published_at_ts: item.published_at.map(|t| t.timestamp()),
            fetched_at_ts: item.fetched_at.timestamp(),
            expires_at_ts: chunk.expires_at.timestamp(),
            reliability_score: item.reliability_score,
            freshness_score: item.freshness_score,
            heat_score: item.heat_score,
            risk_flags: item
                .risk_flags
                .clone()
                .unwrap_or_else(|| serde_json::Value::Array(Vec::new())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ttl_uses_short_lived_gossip_policy() {
        let policy = FreshContextPolicy::new(FreshContextUseCaseConfig::default());
        assert_eq!(
            policy.ttl_for_source_kind(source_kind::GOSSIP),
            Duration::seconds(3 * 24 * 60 * 60)
        );
    }

    #[test]
    fn rank_score_normalizes_weights() {
        let policy = FreshContextPolicy::new(FreshContextUseCaseConfig::default());
        let score = policy.rank_score(1.0, 0.0, 0.0, 0.0);
        assert!((score - 0.55).abs() < 0.0001);
    }
}
