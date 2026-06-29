use std::sync::Arc;

use chrono::{DateTime, Utc};
use tracing::{trace, warn};

use crate::app::fresh_context::policy::FreshContextPolicy;
use crate::domain::fresh_context::{
    FreshChunk, FreshContextRepoT, FreshItem, FreshSource, fresh_status,
};
use crate::domain::llm::EmbeddingProvider;
use crate::domain::vector_store::{VectorCondition, VectorFilter, VectorStoreT};
use crate::shared::config::FreshContextConfig;
use crate::shared::error::AppError;

#[derive(Debug, Clone)]
pub struct FreshRetrievedContext {
    pub content: String,
    pub score: f64,
    pub rumor_level: String,
    pub source_kind: String,
    pub fetched_at: DateTime<Utc>,
    pub published_at: Option<DateTime<Utc>>,
    pub expires_at: DateTime<Utc>,
}

pub struct FreshRetrievalService {
    repo: Arc<dyn FreshContextRepoT>,
    vector_store: Arc<dyn VectorStoreT>,
    embedding_provider: Arc<dyn EmbeddingProvider>,
    policy: FreshContextPolicy,
    config: FreshContextConfig,
}

impl FreshRetrievalService {
    pub fn new(
        repo: Arc<dyn FreshContextRepoT>,
        vector_store: Arc<dyn VectorStoreT>,
        embedding_provider: Arc<dyn EmbeddingProvider>,
        config: FreshContextConfig,
    ) -> Self {
        Self {
            repo,
            vector_store,
            embedding_provider,
            policy: FreshContextPolicy::new(config.clone()),
            config,
        }
    }

    pub async fn retrieve_for_query(
        &self,
        query: &str,
    ) -> Result<Vec<FreshRetrievedContext>, AppError> {
        if !should_use_fresh_context(query) {
            return Ok(Vec::new());
        }

        let limit = self.config.max_retrieval_chunks;
        let query_vector = self.embed_query(query).await?;
        let now = Utc::now();
        let filter = VectorFilter::new()
            .with_condition(VectorCondition::MatchBool {
                key: "active".into(),
                value: true,
            })
            .with_condition(VectorCondition::RangeI64 {
                key: "expires_at_ts".into(),
                gt: Some(now.timestamp()),
                gte: None,
                lt: None,
                lte: None,
            });

        let hits = self
            .vector_store
            .search(&self.config.qdrant_collection, query_vector, filter, limit)
            .await
            .map_err(|e| AppError::internal(format!("fresh context vector search failed: {e}")))?;

        let mut contexts = Vec::new();
        for hit in hits {
            let Some(chunk_id) = fresh_chunk_id_from_payload(&hit.payload) else {
                warn!(hit_id = %hit.id, "Fresh Context Qdrant hit missing fresh_chunk_id");
                continue;
            };
            match self.validate_hit(chunk_id, hit.score as f64, now).await {
                Ok(Some(context)) => contexts.push(context),
                Ok(None) => {}
                Err(error) => warn!(
                    chunk_id,
                    error = %error,
                    "Fresh Context hit validation failed"
                ),
            }
        }

        contexts.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        contexts.truncate(limit);
        Ok(contexts)
    }

    async fn validate_hit(
        &self,
        chunk_id: u64,
        semantic_score: f64,
        now: DateTime<Utc>,
    ) -> Result<Option<FreshRetrievedContext>, AppError> {
        let Some(chunk) = self.repo.find_chunk_by_id(chunk_id).await? else {
            trace!(chunk_id, "fresh chunk missing in MySQL; skipping");
            return Ok(None);
        };
        if chunk.vector_id.is_none() || chunk.active != 1 || chunk.expires_at <= now {
            return Ok(None);
        }

        let Some(item) = self.repo.find_item_by_id(chunk.item_id).await? else {
            return Ok(None);
        };
        if item.status != fresh_status::PUBLISHED
            || item.deleted_at.is_some()
            || item.expires_at <= now
        {
            return Ok(None);
        }

        let Some(source) = self.repo.find_source_by_id(item.source_id).await? else {
            return Ok(None);
        };
        if source.enabled != 1 || source.deleted_at.is_some() {
            return Ok(None);
        }

        let score = self.policy.rank_score(
            semantic_score.clamp(0.0, 1.0),
            item.freshness_score,
            item.reliability_score,
            item.heat_score,
        );
        Ok(Some(FreshRetrievedContext {
            content: format_context(&source, &item, &chunk),
            score,
            rumor_level: item.rumor_level,
            source_kind: source.source_kind,
            fetched_at: item.fetched_at,
            published_at: item.published_at,
            expires_at: chunk.expires_at,
        }))
    }

    async fn embed_query(&self, query: &str) -> Result<Vec<f32>, AppError> {
        let vectors = self
            .embedding_provider
            .embed(&[query.to_string()])
            .await
            .map_err(|e| {
                AppError::internal(format!("fresh context query embedding failed: {e}"))
            })?;
        vectors
            .into_iter()
            .next()
            .filter(|vector| !vector.is_empty())
            .ok_or_else(|| {
                AppError::internal("fresh context query embedding returned empty vector")
            })
    }
}

pub fn should_use_fresh_context(query: &str) -> bool {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return false;
    }
    const KEYWORDS: &[&str] = &[
        "最近",
        "最新",
        "今天",
        "昨日",
        "昨天",
        "现在",
        "目前",
        "近期",
        "新闻",
        "热搜",
        "热榜",
        "趋势",
        "八卦",
        "爆料",
        "瓜",
        "发生了什么",
        "current",
        "latest",
        "recent",
        "today",
        "now",
        "news",
        "trending",
        "trend",
        "gossip",
    ];
    KEYWORDS.iter().any(|keyword| q.contains(keyword))
}

fn fresh_chunk_id_from_payload(payload: &serde_json::Value) -> Option<u64> {
    payload
        .get("fresh_chunk_id")
        .and_then(|value| value.as_u64())
}

fn format_context(source: &FreshSource, item: &FreshItem, chunk: &FreshChunk) -> String {
    let title = item.title.as_deref().unwrap_or("未命名");
    let summary = item.summary.as_deref().unwrap_or("");
    let url = item
        .canonical_url
        .as_deref()
        .or(item.url.as_deref())
        .unwrap_or("");
    let published_at = item
        .published_at
        .map(|t| t.to_rfc3339())
        .unwrap_or_else(|| "unknown".into());

    format!(
        "标题: {title}\n\
         来源: {source_name} ({source_kind}, trust={trust_level})\n\
         rumor_level: {rumor_level}\n\
         published_at: {published_at}\n\
         fetched_at: {fetched_at}\n\
         expires_at: {expires_at}\n\
         url: {url}\n\
         摘要: {summary}\n\
         摘录: {content}",
        source_name = source.name,
        source_kind = source.source_kind,
        trust_level = source.trust_level,
        rumor_level = item.rumor_level,
        fetched_at = item.fetched_at.to_rfc3339(),
        expires_at = chunk.expires_at.to_rfc3339(),
        content = chunk.content
    )
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use async_trait::async_trait;
    use serde_json::json;

    use super::*;
    use crate::domain::fresh_context::{
        FreshItemDistillUpdate, FreshTopic, FreshTopicEvidence, NewFreshChunk, NewFreshItem,
        NewFreshSource, NewFreshTopic, NewFreshTopicEvidence, rumor_level, source_kind,
    };
    use crate::infra::llm::mock_provider::MockEmbeddingProvider;
    use crate::infra::vector_store::mock_vector_store::MockVectorStore;

    #[test]
    fn fresh_intent_detector_is_conservative() {
        assert!(should_use_fresh_context("最近有什么 AI 新闻"));
        assert!(should_use_fresh_context("today's trending gossip"));
        assert!(!should_use_fresh_context("解释一下二叉树"));
    }

    #[tokio::test]
    async fn retrieval_filters_active_and_unexpired_hits() {
        let now = Utc::now();
        let repo = Arc::new(MockFreshRepo {
            source: test_source(),
            item: test_item(now),
            chunk: test_chunk(now),
        });
        let vector_store = Arc::new(MockVectorStore::new());
        vector_store
            .ensure_collection(
                "fresh_test",
                8,
                crate::domain::vector_store::VectorDistance::Cosine,
            )
            .await
            .unwrap();
        vector_store
            .upsert_points(
                "fresh_test",
                vec![crate::domain::vector_store::VectorPoint {
                    id: "fresh_chunk:1".into(),
                    vector: vec![1.0; 8],
                    payload: json!({
                        "fresh_chunk_id": 1,
                        "active": true,
                        "expires_at_ts": (now + chrono::Duration::hours(1)).timestamp()
                    }),
                }],
            )
            .await
            .unwrap();

        let service = FreshRetrievalService::new(
            repo,
            vector_store,
            Arc::new(MockEmbeddingProvider::new(8)),
            FreshContextConfig {
                qdrant_collection: "fresh_test".into(),
                max_retrieval_chunks: 3,
                ..FreshContextConfig::default()
            },
        );

        let results = service.retrieve_for_query("最近有什么新闻").await.unwrap();
        assert_eq!(results.len(), 1);
        assert!(results[0].content.contains("rumor_level"));
        assert_eq!(results[0].rumor_level, rumor_level::REPORTED);
    }

    fn test_source() -> FreshSource {
        let now = Utc::now();
        FreshSource {
            id: 1,
            name: "测试源".into(),
            source_kind: source_kind::RSS.into(),
            base_url: Some("https://example.com/feed.xml".into()),
            allowed_domains: None,
            trust_level: "normal".into(),
            reliability_score: 0.8,
            crawl_interval_secs: 1800,
            default_ttl_secs: 86_400,
            risk_policy: "normal".into(),
            enabled: 1,
            metadata: None,
            created_at: now,
            updated_at: now,
            deleted_at: None,
        }
    }

    fn test_item(now: DateTime<Utc>) -> FreshItem {
        FreshItem {
            id: 1,
            source_id: 1,
            url: Some("https://example.com/a".into()),
            canonical_url: Some("https://example.com/a".into()),
            url_hash: Some("hash".into()),
            title: Some("新鲜标题".into()),
            raw_text: None,
            clean_text: Some("正文".into()),
            summary: Some("摘要".into()),
            published_at: Some(now),
            fetched_at: now,
            expires_at: now + chrono::Duration::hours(1),
            content_hash: "hash".into(),
            status: fresh_status::PUBLISHED.into(),
            reliability_score: 0.8,
            freshness_score: 0.7,
            heat_score: 0.1,
            rumor_level: rumor_level::REPORTED.into(),
            risk_flags: None,
            metadata: None,
            created_at: now,
            updated_at: now,
            deleted_at: None,
        }
    }

    fn test_chunk(now: DateTime<Utc>) -> FreshChunk {
        FreshChunk {
            id: 1,
            item_id: 1,
            topic_id: None,
            chunk_index: 0,
            content: "新鲜上下文摘录".into(),
            content_hash: "chunk_hash".into(),
            token_count: Some(8),
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
        }
    }

    struct MockFreshRepo {
        source: FreshSource,
        item: FreshItem,
        chunk: FreshChunk,
    }

    #[async_trait]
    impl FreshContextRepoT for MockFreshRepo {
        async fn insert_source(&self, _source: NewFreshSource) -> Result<FreshSource, AppError> {
            unimplemented!("not used by retrieval tests")
        }

        async fn list_enabled_sources(&self, _limit: u64) -> Result<Vec<FreshSource>, AppError> {
            unimplemented!("not used by retrieval tests")
        }

        async fn find_source_by_id(&self, source_id: u64) -> Result<Option<FreshSource>, AppError> {
            Ok((self.source.id == source_id).then(|| self.source.clone()))
        }

        async fn insert_item(&self, _item: NewFreshItem) -> Result<FreshItem, AppError> {
            unimplemented!("not used by retrieval tests")
        }

        async fn find_item_by_source_content(
            &self,
            _source_id: u64,
            _content_hash: &str,
        ) -> Result<Option<FreshItem>, AppError> {
            unimplemented!("not used by retrieval tests")
        }

        async fn find_item_by_id(&self, item_id: u64) -> Result<Option<FreshItem>, AppError> {
            Ok((self.item.id == item_id).then(|| self.item.clone()))
        }

        async fn list_active_items(
            &self,
            _now: DateTime<Utc>,
            _limit: u64,
        ) -> Result<Vec<FreshItem>, AppError> {
            unimplemented!("not used by retrieval tests")
        }

        async fn list_chunkable_items(
            &self,
            _now: DateTime<Utc>,
            _limit: u64,
        ) -> Result<Vec<FreshItem>, AppError> {
            unimplemented!("not used by retrieval tests")
        }

        async fn list_items_by_status(
            &self,
            _status: &str,
            _now: DateTime<Utc>,
            _limit: u64,
        ) -> Result<Vec<FreshItem>, AppError> {
            unimplemented!("not used by retrieval tests")
        }

        async fn expire_items(&self, _now: DateTime<Utc>) -> Result<u64, AppError> {
            unimplemented!("not used by retrieval tests")
        }

        async fn update_item_status_if_current(
            &self,
            _item_id: u64,
            _expected_status: &str,
            _new_status: &str,
            _metadata: Option<serde_json::Value>,
        ) -> Result<bool, AppError> {
            unimplemented!("not used by retrieval tests")
        }

        async fn update_item_distill_result_if_current(
            &self,
            _item_id: u64,
            _expected_status: &str,
            _new_status: &str,
            _update: FreshItemDistillUpdate,
        ) -> Result<bool, AppError> {
            unimplemented!("not used by retrieval tests")
        }

        async fn insert_topic(&self, _topic: NewFreshTopic) -> Result<FreshTopic, AppError> {
            unimplemented!("not used by retrieval tests")
        }

        async fn upsert_topic(&self, _topic: NewFreshTopic) -> Result<FreshTopic, AppError> {
            unimplemented!("not used by retrieval tests")
        }

        async fn find_topic_by_key(
            &self,
            _topic_key: &str,
        ) -> Result<Option<FreshTopic>, AppError> {
            unimplemented!("not used by retrieval tests")
        }

        async fn link_topic_evidence(
            &self,
            _evidence: NewFreshTopicEvidence,
        ) -> Result<FreshTopicEvidence, AppError> {
            unimplemented!("not used by retrieval tests")
        }

        async fn assign_topic_to_item_chunks(
            &self,
            _item_id: u64,
            _topic_id: u64,
        ) -> Result<u64, AppError> {
            unimplemented!("not used by retrieval tests")
        }

        async fn insert_chunks(
            &self,
            _chunks: &[NewFreshChunk],
        ) -> Result<Vec<FreshChunk>, AppError> {
            unimplemented!("not used by retrieval tests")
        }

        async fn find_chunk_by_id(&self, chunk_id: u64) -> Result<Option<FreshChunk>, AppError> {
            Ok((self.chunk.id == chunk_id).then(|| self.chunk.clone()))
        }

        async fn find_chunks_by_item(&self, _item_id: u64) -> Result<Vec<FreshChunk>, AppError> {
            unimplemented!("not used by retrieval tests")
        }

        async fn list_indexable_chunks(
            &self,
            _now: DateTime<Utc>,
            _limit: u64,
        ) -> Result<Vec<FreshChunk>, AppError> {
            unimplemented!("not used by retrieval tests")
        }

        async fn mark_chunk_indexed(
            &self,
            _chunk_id: u64,
            _vector_id: String,
            _embedding_provider: String,
            _embedding_model: String,
            _embedding_dimension: u32,
        ) -> Result<bool, AppError> {
            unimplemented!("not used by retrieval tests")
        }

        async fn list_expired_indexed_chunks(
            &self,
            _now: DateTime<Utc>,
            _limit: u64,
        ) -> Result<Vec<FreshChunk>, AppError> {
            unimplemented!("not used by retrieval tests")
        }

        async fn mark_chunk_vector_deleted(
            &self,
            _chunk_id: u64,
            _vector_id: &str,
        ) -> Result<bool, AppError> {
            unimplemented!("not used by retrieval tests")
        }
    }
}
