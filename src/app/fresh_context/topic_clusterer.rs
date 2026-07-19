use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tracing::warn;

use crate::app::fresh_context::config::FreshContextUseCaseConfig;
use crate::domain::fresh_context::{
    FreshContextRepoT, FreshItem, NewFreshTopic, NewFreshTopicEvidence, fresh_status,
};
use crate::shared::error::AppError;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FreshTopicClusterStats {
    pub active_seen: usize,
    pub topics_upserted: usize,
    pub evidences_linked: usize,
    pub chunks_assigned: u64,
    pub skipped: usize,
    pub failed: usize,
}

pub struct FreshTopicClustererService {
    repo: Arc<dyn FreshContextRepoT>,
    config: FreshContextUseCaseConfig,
}

impl FreshTopicClustererService {
    pub fn new(repo: Arc<dyn FreshContextRepoT>, config: FreshContextUseCaseConfig) -> Self {
        Self { repo, config }
    }

    pub async fn run_tick(&self) -> Result<FreshTopicClusterStats, AppError> {
        let now = Utc::now();
        let items = self
            .repo
            .list_active_items(now, self.config.max_topic_items_per_tick as u64)
            .await?;
        let mut stats = FreshTopicClusterStats {
            active_seen: items.len(),
            ..FreshTopicClusterStats::default()
        };

        for item in items {
            match self.cluster_item(item, now).await {
                Ok(FreshTopicOutcome::Clustered { chunks_assigned }) => {
                    stats.topics_upserted += 1;
                    stats.evidences_linked += 1;
                    stats.chunks_assigned += chunks_assigned;
                }
                Ok(FreshTopicOutcome::Skipped) => stats.skipped += 1,
                Err(error) => {
                    stats.failed += 1;
                    warn!(error = %error, "Fresh Context topic clustering failed");
                }
            }
        }

        Ok(stats)
    }

    async fn cluster_item(
        &self,
        item: FreshItem,
        now: DateTime<Utc>,
    ) -> Result<FreshTopicOutcome, AppError> {
        if item.status != fresh_status::PUBLISHED || item.expires_at <= now {
            return Ok(FreshTopicOutcome::Skipped);
        }

        let topic = build_topic_from_item(&item)?;
        let topic = self.repo.upsert_topic(topic).await?;
        self.repo
            .link_topic_evidence(NewFreshTopicEvidence {
                topic_id: topic.id,
                item_id: item.id,
                stance: "supports".into(),
                confidence: evidence_confidence(&item),
            })
            .await?;
        let chunks_assigned = self
            .repo
            .assign_topic_to_item_chunks(item.id, topic.id)
            .await?;
        Ok(FreshTopicOutcome::Clustered { chunks_assigned })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FreshTopicOutcome {
    Clustered { chunks_assigned: u64 },
    Skipped,
}

fn build_topic_from_item(item: &FreshItem) -> Result<NewFreshTopic, AppError> {
    let distilled = item.metadata.as_ref().and_then(|m| m.get("distilled"));
    let raw_key = topic_key_material(item, distilled);
    let topic_key = sha256_hex(&normalize_key(&raw_key));
    let title = non_empty(
        distilled
            .and_then(|d| d.get("title"))
            .and_then(Value::as_str)
            .or(item.title.as_deref()),
    )
    .unwrap_or_else(|| "Fresh topic".into());
    let summary = non_empty(
        distilled
            .and_then(|d| d.get("summary"))
            .and_then(Value::as_str)
            .or(item.summary.as_deref()),
    );
    let first_seen_at = item.published_at.unwrap_or(item.fetched_at);
    let entities = distilled
        .and_then(|d| d.get("entities"))
        .filter(|value| !value.is_null())
        .cloned();

    Ok(NewFreshTopic {
        topic_key,
        title,
        summary,
        entities,
        first_seen_at,
        last_seen_at: item.fetched_at,
        heat_score: item.heat_score,
        freshness_score: item.freshness_score,
        expires_at: item.expires_at,
        status: fresh_status::PUBLISHED.into(),
        risk_flags: item.risk_flags.clone(),
        metadata: Some(json!({
            "clusterer": "fresh_context_v1",
            "source_item_id": item.id,
            "topic_key_material": raw_key,
            "keywords": distilled.and_then(|d| d.get("keywords")).cloned(),
            "rumor_level": item.rumor_level,
        })),
    })
}

fn topic_key_material(item: &FreshItem, distilled: Option<&Value>) -> String {
    if let Some(hint) = distilled
        .and_then(|d| d.get("topic_key_hint"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        return hint.to_string();
    }

    if let Some(title) = item
        .title
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        return title.to_string();
    }

    if let Some(url) = item
        .canonical_url
        .as_deref()
        .or(item.url.as_deref())
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        return url.to_string();
    }

    item.content_hash.clone()
}

fn evidence_confidence(item: &FreshItem) -> f64 {
    ((item.reliability_score + item.freshness_score) / 2.0).clamp(0.0, 1.0)
}

fn non_empty(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn normalize_key(value: &str) -> String {
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

fn sha256_hex(input: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(input.as_bytes());
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use tokio::sync::Mutex;

    use super::*;
    use crate::domain::fresh_context::{
        FreshChunk, FreshItemDistillUpdate, FreshSource, FreshTopic, FreshTopicEvidence,
        NewFreshChunk, NewFreshItem, NewFreshSource, rumor_level,
    };
    use async_trait::async_trait;

    #[test]
    fn topic_key_uses_distilled_hint_when_available() {
        let item = test_item(1, "shared-topic");
        let topic = build_topic_from_item(&item).unwrap();
        assert_eq!(topic.topic_key, sha256_hex("shared-topic"));
        assert_eq!(topic.title, "标题 1");
    }

    #[tokio::test]
    async fn clusterer_links_items_and_assigns_chunks() {
        let item = test_item(1, "shared-topic");
        let repo = Arc::new(MockFreshRepo::new(vec![item]));
        let clusterer =
            FreshTopicClustererService::new(repo.clone(), FreshContextUseCaseConfig::default());

        let stats = clusterer.run_tick().await.unwrap();

        assert_eq!(stats.active_seen, 1);
        assert_eq!(stats.topics_upserted, 1);
        assert_eq!(stats.evidences_linked, 1);
        assert_eq!(stats.chunks_assigned, 1);
        assert_eq!(repo.topics.lock().await.len(), 1);
        assert_eq!(repo.evidence.lock().await.len(), 1);
    }

    fn test_item(id: u64, topic_hint: &str) -> FreshItem {
        let now = Utc::now();
        FreshItem {
            id,
            source_id: 1,
            url: Some(format!("https://example.com/{id}")),
            canonical_url: Some(format!("https://example.com/{id}")),
            url_hash: Some(format!("hash-{id}")),
            title: Some(format!("标题 {id}")),
            raw_text: None,
            clean_text: Some("正文".into()),
            summary: Some("摘要".into()),
            published_at: Some(now),
            fetched_at: now,
            expires_at: now + chrono::Duration::hours(1),
            content_hash: format!("content-{id}"),
            status: fresh_status::PUBLISHED.into(),
            reliability_score: 0.8,
            freshness_score: 0.6,
            heat_score: 0.4,
            rumor_level: rumor_level::REPORTED.into(),
            risk_flags: None,
            metadata: Some(json!({
                "distilled": {
                    "title": format!("标题 {id}"),
                    "summary": "摘要",
                    "topic_key_hint": topic_hint,
                    "entities": [{"name": "Alice", "entity_type": "person"}],
                    "keywords": ["测试"]
                }
            })),
            created_at: now,
            updated_at: now,
            deleted_at: None,
        }
    }

    struct MockFreshRepo {
        items: Vec<FreshItem>,
        topics: Mutex<Vec<FreshTopic>>,
        evidence: Mutex<Vec<FreshTopicEvidence>>,
        chunks: Mutex<Vec<FreshChunk>>,
    }

    impl MockFreshRepo {
        fn new(items: Vec<FreshItem>) -> Self {
            let now = Utc::now();
            Self {
                chunks: Mutex::new(vec![FreshChunk {
                    id: 1,
                    item_id: 1,
                    topic_id: None,
                    chunk_index: 0,
                    content: "正文".into(),
                    content_hash: "chunk".into(),
                    token_count: Some(2),
                    metadata: None,
                    vector_id: Some("fresh_chunk:1".into()),
                    embedding_provider: Some("mock".into()),
                    embedding_model: Some("mock".into()),
                    embedding_dimension: Some(8),
                    active: 1,
                    indexed_at: Some(now),
                    expires_at: now + chrono::Duration::hours(1),
                    created_at: now,
                    updated_at: now,
                }]),
                items,
                topics: Mutex::new(Vec::new()),
                evidence: Mutex::new(Vec::new()),
            }
        }
    }

    #[async_trait]
    impl FreshContextRepoT for MockFreshRepo {
        async fn insert_source(&self, _source: NewFreshSource) -> Result<FreshSource, AppError> {
            unimplemented!("not used by topic tests")
        }

        async fn list_enabled_sources(&self, _limit: u64) -> Result<Vec<FreshSource>, AppError> {
            unimplemented!("not used by topic tests")
        }

        async fn find_source_by_id(
            &self,
            _source_id: u64,
        ) -> Result<Option<FreshSource>, AppError> {
            unimplemented!("not used by topic tests")
        }

        async fn insert_item(&self, _item: NewFreshItem) -> Result<FreshItem, AppError> {
            unimplemented!("not used by topic tests")
        }

        async fn find_item_by_source_content(
            &self,
            _source_id: u64,
            _content_hash: &str,
        ) -> Result<Option<FreshItem>, AppError> {
            unimplemented!("not used by topic tests")
        }

        async fn find_item_by_id(&self, _item_id: u64) -> Result<Option<FreshItem>, AppError> {
            unimplemented!("not used by topic tests")
        }

        async fn list_active_items(
            &self,
            now: DateTime<Utc>,
            limit: u64,
        ) -> Result<Vec<FreshItem>, AppError> {
            Ok(self
                .items
                .iter()
                .filter(|item| item.status == fresh_status::PUBLISHED && item.expires_at > now)
                .take(limit as usize)
                .cloned()
                .collect())
        }

        async fn list_chunkable_items(
            &self,
            _now: DateTime<Utc>,
            _limit: u64,
        ) -> Result<Vec<FreshItem>, AppError> {
            unimplemented!("not used by topic tests")
        }

        async fn list_items_by_status(
            &self,
            _status: &str,
            _now: DateTime<Utc>,
            _limit: u64,
        ) -> Result<Vec<FreshItem>, AppError> {
            unimplemented!("not used by topic tests")
        }

        async fn expire_items(&self, _now: DateTime<Utc>) -> Result<u64, AppError> {
            unimplemented!("not used by topic tests")
        }

        async fn update_item_status_if_current(
            &self,
            _item_id: u64,
            _expected_status: &str,
            _new_status: &str,
            _metadata: Option<serde_json::Value>,
        ) -> Result<bool, AppError> {
            unimplemented!("not used by topic tests")
        }

        async fn update_item_distill_result_if_current(
            &self,
            _item_id: u64,
            _expected_status: &str,
            _new_status: &str,
            _update: FreshItemDistillUpdate,
        ) -> Result<bool, AppError> {
            unimplemented!("not used by topic tests")
        }

        async fn insert_topic(&self, topic: NewFreshTopic) -> Result<FreshTopic, AppError> {
            self.upsert_topic(topic).await
        }

        async fn upsert_topic(&self, topic: NewFreshTopic) -> Result<FreshTopic, AppError> {
            let mut topics = self.topics.lock().await;
            if let Some(existing) = topics
                .iter_mut()
                .find(|existing| existing.topic_key == topic.topic_key)
            {
                existing.last_seen_at = existing.last_seen_at.max(topic.last_seen_at);
                existing.expires_at = existing.expires_at.max(topic.expires_at);
                return Ok(existing.clone());
            }
            let now = Utc::now();
            let saved = FreshTopic {
                id: topics.len() as u64 + 1,
                topic_key: topic.topic_key,
                title: topic.title,
                summary: topic.summary,
                entities: topic.entities,
                first_seen_at: topic.first_seen_at,
                last_seen_at: topic.last_seen_at,
                heat_score: topic.heat_score,
                freshness_score: topic.freshness_score,
                expires_at: topic.expires_at,
                status: topic.status,
                risk_flags: topic.risk_flags,
                metadata: topic.metadata,
                created_at: now,
                updated_at: now,
                deleted_at: None,
            };
            topics.push(saved.clone());
            Ok(saved)
        }

        async fn find_topic_by_key(&self, topic_key: &str) -> Result<Option<FreshTopic>, AppError> {
            Ok(self
                .topics
                .lock()
                .await
                .iter()
                .find(|topic| topic.topic_key == topic_key)
                .cloned())
        }

        async fn link_topic_evidence(
            &self,
            evidence: NewFreshTopicEvidence,
        ) -> Result<FreshTopicEvidence, AppError> {
            let mut rows = self.evidence.lock().await;
            if let Some(existing) = rows
                .iter()
                .find(|row| row.topic_id == evidence.topic_id && row.item_id == evidence.item_id)
            {
                return Ok(existing.clone());
            }
            let saved = FreshTopicEvidence {
                topic_id: evidence.topic_id,
                item_id: evidence.item_id,
                stance: evidence.stance,
                confidence: evidence.confidence,
                created_at: Utc::now(),
            };
            rows.push(saved.clone());
            Ok(saved)
        }

        async fn assign_topic_to_item_chunks(
            &self,
            item_id: u64,
            topic_id: u64,
        ) -> Result<u64, AppError> {
            let mut count = 0;
            for chunk in &mut *self.chunks.lock().await {
                if chunk.item_id == item_id && chunk.topic_id != Some(topic_id) {
                    chunk.topic_id = Some(topic_id);
                    count += 1;
                }
            }
            Ok(count)
        }

        async fn insert_chunks(
            &self,
            _chunks: &[NewFreshChunk],
        ) -> Result<Vec<FreshChunk>, AppError> {
            unimplemented!("not used by topic tests")
        }

        async fn find_chunk_by_id(&self, _chunk_id: u64) -> Result<Option<FreshChunk>, AppError> {
            unimplemented!("not used by topic tests")
        }

        async fn find_chunks_by_item(&self, _item_id: u64) -> Result<Vec<FreshChunk>, AppError> {
            unimplemented!("not used by topic tests")
        }

        async fn list_indexable_chunks(
            &self,
            _now: DateTime<Utc>,
            _limit: u64,
        ) -> Result<Vec<FreshChunk>, AppError> {
            unimplemented!("not used by topic tests")
        }

        async fn mark_chunk_indexed(
            &self,
            _chunk_id: u64,
            _vector_id: String,
            _embedding_provider: String,
            _embedding_model: String,
            _embedding_dimension: u32,
        ) -> Result<bool, AppError> {
            unimplemented!("not used by topic tests")
        }

        async fn list_expired_indexed_chunks(
            &self,
            _now: DateTime<Utc>,
            _limit: u64,
        ) -> Result<Vec<FreshChunk>, AppError> {
            unimplemented!("not used by topic tests")
        }

        async fn mark_chunk_vector_deleted(
            &self,
            _chunk_id: u64,
            _vector_id: &str,
        ) -> Result<bool, AppError> {
            unimplemented!("not used by topic tests")
        }
    }
}
