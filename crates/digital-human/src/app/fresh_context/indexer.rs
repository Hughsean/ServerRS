use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde_json::json;
use sha2::{Digest, Sha256};
use tracing::warn;

use crate::app::fresh_context::config::FreshContextUseCaseConfig;
use crate::app::fresh_context::policy::FreshContextPolicy;
use crate::domain::fresh_context::{
    FreshChunk, FreshContextRepoT, FreshItem, FreshSource, NewFreshChunk, fresh_status,
};
use crate::domain::llm::EmbeddingProvider;
use crate::domain::vector_store::{VectorDistance, VectorPoint, VectorStoreT};
use crate::shared::error::AppError;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FreshIndexStats {
    pub expired_vectors_seen: usize,
    pub expired_vectors_deleted: usize,
    pub published_seen: usize,
    pub chunks_created: usize,
    pub indexable_seen: usize,
    pub chunks_indexed: usize,
    pub skipped: usize,
    pub failed: usize,
}

pub struct FreshIndexerService {
    repo: Arc<dyn FreshContextRepoT>,
    vector_store: Arc<dyn VectorStoreT>,
    embedding_provider: Arc<dyn EmbeddingProvider>,
    policy: FreshContextPolicy,
    config: FreshContextUseCaseConfig,
    vector_index_name: String,
    embedding_provider_name: String,
    embedding_model: String,
}

impl FreshIndexerService {
    pub fn new(
        repo: Arc<dyn FreshContextRepoT>,
        vector_store: Arc<dyn VectorStoreT>,
        embedding_provider: Arc<dyn EmbeddingProvider>,
        config: FreshContextUseCaseConfig,
        vector_index_name: String,
        embedding_provider_name: String,
        embedding_model: String,
    ) -> Self {
        Self {
            repo,
            vector_store,
            embedding_provider,
            policy: FreshContextPolicy::new(config.clone()),
            config,
            vector_index_name,
            embedding_provider_name,
            embedding_model,
        }
    }

    pub async fn ensure_collection(&self) -> Result<(), AppError> {
        let dimension = self.probe_dimension().await?;
        self.vector_store
            .ensure_collection(&self.vector_index_name, dimension, VectorDistance::Cosine)
            .await
            .map_err(|e| {
                AppError::internal(format!(
                    "failed to ensure fresh context collection '{}': {e}",
                    self.vector_index_name
                ))
            })
    }

    pub async fn run_tick(&self) -> Result<FreshIndexStats, AppError> {
        let now = Utc::now();
        let mut stats = FreshIndexStats::default();

        let (expired_seen, expired_deleted) = self.cleanup_expired_vectors(now).await?;
        stats.expired_vectors_seen = expired_seen;
        stats.expired_vectors_deleted = expired_deleted;

        let items = self
            .repo
            .list_chunkable_items(now, self.config.max_pipeline_items_per_tick as u64)
            .await?;
        stats.published_seen = items.len();

        for item in items {
            match self.ensure_item_chunks(&item).await {
                Ok(created) => stats.chunks_created += created,
                Err(error) => {
                    stats.failed += 1;
                    warn!(
                        item_id = item.id,
                        error = %error,
                        "Fresh Context item chunking failed"
                    );
                }
            }
        }

        let chunks = self
            .repo
            .list_indexable_chunks(now, self.config.max_indexable_chunks_per_tick as u64)
            .await?;
        stats.indexable_seen = chunks.len();

        for chunk in chunks {
            match self.index_chunk(&chunk, now).await {
                Ok(FreshIndexOutcome::Indexed) => stats.chunks_indexed += 1,
                Ok(FreshIndexOutcome::Skipped) => stats.skipped += 1,
                Err(error) => {
                    stats.failed += 1;
                    warn!(
                        chunk_id = chunk.id,
                        error = %error,
                        "Fresh Context chunk indexing failed"
                    );
                }
            }
        }

        Ok(stats)
    }

    async fn cleanup_expired_vectors(
        &self,
        now: DateTime<Utc>,
    ) -> Result<(usize, usize), AppError> {
        let chunks = self
            .repo
            .list_expired_indexed_chunks(now, self.config.max_expired_vectors_per_tick as u64)
            .await?;
        if chunks.is_empty() {
            return Ok((0, 0));
        }
        let seen = chunks.len();

        let vector_ids = chunks
            .iter()
            .filter_map(|chunk| chunk.vector_id.clone())
            .collect::<Vec<_>>();
        self.vector_store
            .delete_points(&self.vector_index_name, vector_ids)
            .await
            .map_err(|e| AppError::internal(format!("fresh expired vector delete failed: {e}")))?;

        let mut deleted = 0usize;
        for chunk in chunks {
            let Some(vector_id) = chunk.vector_id.as_deref() else {
                continue;
            };
            if self
                .repo
                .mark_chunk_vector_deleted(chunk.id, vector_id)
                .await?
            {
                deleted += 1;
            }
        }
        Ok((seen, deleted))
    }

    async fn ensure_item_chunks(&self, item: &FreshItem) -> Result<usize, AppError> {
        if !self.repo.find_chunks_by_item(item.id).await?.is_empty() {
            return Ok(0);
        }

        let text = fresh_item_index_text(item);
        let chunk_texts =
            chunk_text_by_chars(&text, self.config.chunk_size, self.config.chunk_overlap)?;
        if chunk_texts.is_empty() {
            return Ok(0);
        }

        let chunks = chunk_texts
            .into_iter()
            .enumerate()
            .map(|(index, content)| NewFreshChunk {
                item_id: item.id,
                topic_id: None,
                chunk_index: index as u32,
                content_hash: fresh_chunk_hash(item.id, index as u32, &content),
                token_count: Some(content.chars().count() as u32),
                metadata: Some(json!({
                    "chunker": "fresh_context_v1",
                    "item_id": item.id,
                    "title": item.title,
                    "url": item.canonical_url.as_ref().or(item.url.as_ref()),
                })),
                content,
                expires_at: item.expires_at,
            })
            .collect::<Vec<_>>();

        let saved = self.repo.insert_chunks(&chunks).await?;
        Ok(saved.len())
    }

    async fn index_chunk(
        &self,
        chunk: &FreshChunk,
        now: DateTime<Utc>,
    ) -> Result<FreshIndexOutcome, AppError> {
        if chunk.vector_id.is_some() || chunk.active != 1 || chunk.expires_at <= now {
            return Ok(FreshIndexOutcome::Skipped);
        }

        let Some(item) = self.repo.find_item_by_id(chunk.item_id).await? else {
            return Ok(FreshIndexOutcome::Skipped);
        };
        if item.status != fresh_status::PUBLISHED || item.expires_at <= now {
            return Ok(FreshIndexOutcome::Skipped);
        }

        let Some(source) = self.repo.find_source_by_id(item.source_id).await? else {
            return Ok(FreshIndexOutcome::Skipped);
        };
        if !source_is_indexable(&source) {
            return Ok(FreshIndexOutcome::Skipped);
        }

        let vector_id = fresh_chunk_vector_id(chunk.id);
        let vector = self.embed_chunk(chunk).await?;
        let embedding_dimension = vector.len() as u32;
        let payload = serde_json::to_value(self.policy.build_payload(
            vector_id.clone(),
            &source,
            &item,
            None,
            chunk,
        ))
        .map_err(|e| AppError::internal(format!("serialize fresh chunk payload: {e}")))?;

        self.vector_store
            .upsert_points(
                &self.vector_index_name,
                vec![VectorPoint {
                    id: vector_id.clone(),
                    vector,
                    payload,
                }],
            )
            .await
            .map_err(|e| AppError::internal(format!("fresh chunk vector upsert failed: {e}")))?;

        let applied = self
            .repo
            .mark_chunk_indexed(
                chunk.id,
                vector_id,
                self.embedding_provider_name.clone(),
                self.embedding_model.clone(),
                embedding_dimension,
            )
            .await?;
        Ok(if applied {
            FreshIndexOutcome::Indexed
        } else {
            FreshIndexOutcome::Skipped
        })
    }

    async fn probe_dimension(&self) -> Result<usize, AppError> {
        let vectors = self
            .embedding_provider
            .embed(&["fresh context dimension probe".to_string()])
            .await
            .map_err(|e| AppError::internal(format!("fresh embedding probe failed: {e}")))?;
        vectors
            .first()
            .map(Vec::len)
            .filter(|dimension| *dimension > 0)
            .ok_or_else(|| AppError::internal("fresh embedding probe returned empty vector"))
    }

    async fn embed_chunk(&self, chunk: &FreshChunk) -> Result<Vec<f32>, AppError> {
        let vectors = self
            .embedding_provider
            .embed(std::slice::from_ref(&chunk.content))
            .await
            .map_err(|e| AppError::internal(format!("embed fresh chunk {}: {e}", chunk.id)))?;
        vectors
            .into_iter()
            .next()
            .filter(|vector| !vector.is_empty())
            .ok_or_else(|| AppError::internal("fresh embedding returned empty vector"))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FreshIndexOutcome {
    Indexed,
    Skipped,
}

fn source_is_indexable(source: &FreshSource) -> bool {
    source.enabled == 1 && source.deleted_at.is_none()
}

pub fn fresh_chunk_vector_id(chunk_id: u64) -> String {
    format!("fresh_chunk:{chunk_id}")
}

fn fresh_item_index_text(item: &FreshItem) -> String {
    let mut parts = Vec::new();
    if let Some(title) = non_empty(item.title.as_deref()) {
        parts.push(format!("标题：{title}"));
    }
    if let Some(summary) = non_empty(item.summary.as_deref()) {
        parts.push(format!("摘要：{summary}"));
    }
    if let Some(clean_text) = non_empty(item.clean_text.as_deref()) {
        parts.push(format!("正文：{clean_text}"));
    }
    parts.join("\n")
}

fn non_empty(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn chunk_text_by_chars(
    text: &str,
    chunk_size: usize,
    overlap: usize,
) -> Result<Vec<String>, AppError> {
    if chunk_size == 0 {
        return Err(AppError::Validation(
            "fresh chunk_size must be positive".into(),
        ));
    }
    if overlap >= chunk_size {
        return Err(AppError::Validation(
            "fresh chunk_overlap must be less than chunk_size".into(),
        ));
    }

    let chars = text.trim().chars().collect::<Vec<_>>();
    if chars.is_empty() {
        return Ok(Vec::new());
    }

    let step = chunk_size - overlap;
    let mut chunks = Vec::new();
    let mut start = 0usize;
    while start < chars.len() {
        let end = (start + chunk_size).min(chars.len());
        let chunk = chars[start..end].iter().collect::<String>();
        if !chunk.trim().is_empty() {
            chunks.push(chunk);
        }
        if end == chars.len() {
            break;
        }
        start += step;
    }
    Ok(chunks)
}

fn fresh_chunk_hash(item_id: u64, chunk_index: u32, content: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(item_id.to_le_bytes());
    hasher.update(chunk_index.to_le_bytes());
    hasher.update(content.as_bytes());
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use async_trait::async_trait;
    use tokio::sync::Mutex;

    use super::*;
    use crate::domain::fresh_context::{
        FreshItemDistillUpdate, FreshTopic, FreshTopicEvidence, NewFreshItem, NewFreshSource,
        NewFreshTopic, NewFreshTopicEvidence, rumor_level, source_kind,
    };
    use crate::domain::vector_store::{VectorCondition, VectorFilter};
    use crate::infra::llm::mock_provider::MockEmbeddingProvider;
    use crate::infra::vector_store::mock_vector_store::MockVectorStore;

    #[test]
    fn chunk_text_handles_unicode_boundaries() {
        let chunks = chunk_text_by_chars("你好世界abcdef", 4, 1).unwrap();
        assert_eq!(chunks[0], "你好世界");
        assert_eq!(chunks[1], "界abc");
    }

    #[tokio::test]
    async fn indexer_creates_chunks_and_indexes_payload() {
        let item = test_item();
        let source = test_source();
        let repo = Arc::new(MockFreshRepo::new(vec![item], source));
        let vector_store = Arc::new(MockVectorStore::new());
        let embedding_provider = Arc::new(MockEmbeddingProvider::new(8));
        let config = FreshContextUseCaseConfig {
            chunk_size: 16,
            chunk_overlap: 4,
            max_pipeline_items_per_tick: 10,
            max_indexable_chunks_per_tick: 10,
            ..FreshContextUseCaseConfig::default()
        };
        let indexer = FreshIndexerService::new(
            repo.clone(),
            vector_store.clone(),
            embedding_provider,
            config,
            "fresh_test".into(),
            "mock".into(),
            "mock-embedding".into(),
        );
        indexer.ensure_collection().await.unwrap();

        let stats = indexer.run_tick().await.unwrap();

        assert_eq!(stats.published_seen, 1);
        assert!(stats.chunks_created > 0);
        assert_eq!(stats.indexable_seen, stats.chunks_created);
        assert_eq!(stats.chunks_indexed, stats.chunks_created);

        let hits = vector_store
            .search(
                "fresh_test",
                vec![0.0; 8],
                VectorFilter::default()
                    .with_condition(VectorCondition::MatchBool {
                        key: "active".into(),
                        value: true,
                    })
                    .with_condition(VectorCondition::RangeI64 {
                        key: "expires_at_ts".into(),
                        gt: Some(Utc::now().timestamp()),
                        gte: None,
                        lt: None,
                        lte: None,
                    }),
                10,
            )
            .await
            .unwrap();
        assert_eq!(hits.len(), stats.chunks_indexed);
        assert_eq!(hits[0].payload["kind"], "fresh_chunk");
        assert_eq!(hits[0].payload["fresh_item_id"], 1);
    }

    #[tokio::test]
    async fn indexer_deletes_expired_vectors() {
        let mut item = test_item();
        item.expires_at = Utc::now() - chrono::Duration::minutes(1);
        let source = test_source();
        let repo = Arc::new(MockFreshRepo::new(vec![item], source));
        repo.chunks.lock().await.push(expired_indexed_chunk());
        let vector_store = Arc::new(MockVectorStore::new());
        vector_store
            .ensure_collection("fresh_test", 8, VectorDistance::Cosine)
            .await
            .unwrap();
        vector_store
            .upsert_points(
                "fresh_test",
                vec![VectorPoint {
                    id: "fresh_chunk:1".into(),
                    vector: vec![0.0; 8],
                    payload: json!({"vector_id": "fresh_chunk:1"}),
                }],
            )
            .await
            .unwrap();
        let embedding_provider = Arc::new(MockEmbeddingProvider::new(8));
        let config = FreshContextUseCaseConfig {
            max_expired_vectors_per_tick: 10,
            ..FreshContextUseCaseConfig::default()
        };
        let indexer = FreshIndexerService::new(
            repo.clone(),
            vector_store.clone(),
            embedding_provider,
            config,
            "fresh_test".into(),
            "mock".into(),
            "mock-embedding".into(),
        );

        let stats = indexer.run_tick().await.unwrap();

        assert_eq!(stats.expired_vectors_seen, 1);
        assert_eq!(stats.expired_vectors_deleted, 1);
        assert!(
            vector_store
                .search("fresh_test", vec![0.0; 8], VectorFilter::default(), 10)
                .await
                .unwrap()
                .is_empty()
        );
        assert!(repo.chunks.lock().await[0].vector_id.is_none());
    }

    fn test_source() -> FreshSource {
        let now = Utc::now();
        FreshSource {
            id: 1,
            name: "Fresh RSS".into(),
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

    fn test_item() -> FreshItem {
        let now = Utc::now();
        FreshItem {
            id: 1,
            source_id: 1,
            url: Some("https://example.com/a".into()),
            canonical_url: Some("https://example.com/a".into()),
            url_hash: Some("hash".into()),
            title: Some("新鲜标题".into()),
            raw_text: None,
            clean_text: Some(
                "这是一段用于 Fresh Context 索引测试的中文正文，它需要被安全切分。".into(),
            ),
            summary: Some("一段摘要".into()),
            published_at: Some(now),
            fetched_at: now,
            expires_at: now + chrono::Duration::hours(1),
            content_hash: "content_hash".into(),
            status: fresh_status::PUBLISHED.into(),
            reliability_score: 0.8,
            freshness_score: 0.7,
            heat_score: 0.2,
            rumor_level: rumor_level::REPORTED.into(),
            risk_flags: None,
            metadata: None,
            created_at: now,
            updated_at: now,
            deleted_at: None,
        }
    }

    fn expired_indexed_chunk() -> FreshChunk {
        let now = Utc::now();
        FreshChunk {
            id: 1,
            item_id: 1,
            topic_id: None,
            chunk_index: 0,
            content: "过期内容".into(),
            content_hash: "expired_chunk".into(),
            token_count: Some(4),
            metadata: None,
            vector_id: Some("fresh_chunk:1".into()),
            embedding_provider: Some("mock".into()),
            embedding_model: Some("mock".into()),
            embedding_dimension: Some(8),
            active: 0,
            indexed_at: Some(now - chrono::Duration::minutes(2)),
            expires_at: now - chrono::Duration::minutes(1),
            created_at: now,
            updated_at: now,
        }
    }

    struct MockFreshRepo {
        items: Mutex<Vec<FreshItem>>,
        chunks: Mutex<Vec<FreshChunk>>,
        source: FreshSource,
    }

    impl MockFreshRepo {
        fn new(items: Vec<FreshItem>, source: FreshSource) -> Self {
            Self {
                items: Mutex::new(items),
                chunks: Mutex::new(Vec::new()),
                source,
            }
        }
    }

    #[async_trait]
    impl FreshContextRepoT for MockFreshRepo {
        async fn insert_source(&self, _source: NewFreshSource) -> Result<FreshSource, AppError> {
            unimplemented!("not used by indexer tests")
        }

        async fn list_enabled_sources(&self, _limit: u64) -> Result<Vec<FreshSource>, AppError> {
            unimplemented!("not used by indexer tests")
        }

        async fn find_source_by_id(&self, source_id: u64) -> Result<Option<FreshSource>, AppError> {
            Ok((self.source.id == source_id).then(|| self.source.clone()))
        }

        async fn insert_item(&self, _item: NewFreshItem) -> Result<FreshItem, AppError> {
            unimplemented!("not used by indexer tests")
        }

        async fn find_item_by_source_content(
            &self,
            _source_id: u64,
            _content_hash: &str,
        ) -> Result<Option<FreshItem>, AppError> {
            unimplemented!("not used by indexer tests")
        }

        async fn find_item_by_id(&self, item_id: u64) -> Result<Option<FreshItem>, AppError> {
            Ok(self
                .items
                .lock()
                .await
                .iter()
                .find(|item| item.id == item_id)
                .cloned())
        }

        async fn list_active_items(
            &self,
            _now: DateTime<Utc>,
            _limit: u64,
        ) -> Result<Vec<FreshItem>, AppError> {
            unimplemented!("not used by indexer tests")
        }

        async fn list_chunkable_items(
            &self,
            now: DateTime<Utc>,
            limit: u64,
        ) -> Result<Vec<FreshItem>, AppError> {
            let chunks = self.chunks.lock().await;
            Ok(self
                .items
                .lock()
                .await
                .iter()
                .filter(|item| {
                    item.status == fresh_status::PUBLISHED
                        && item.expires_at > now
                        && !chunks.iter().any(|chunk| chunk.item_id == item.id)
                })
                .take(limit as usize)
                .cloned()
                .collect())
        }

        async fn list_items_by_status(
            &self,
            _status: &str,
            _now: DateTime<Utc>,
            _limit: u64,
        ) -> Result<Vec<FreshItem>, AppError> {
            unimplemented!("not used by indexer tests")
        }

        async fn expire_items(&self, _now: DateTime<Utc>) -> Result<u64, AppError> {
            unimplemented!("not used by indexer tests")
        }

        async fn update_item_status_if_current(
            &self,
            _item_id: u64,
            _expected_status: &str,
            _new_status: &str,
            _metadata: Option<serde_json::Value>,
        ) -> Result<bool, AppError> {
            unimplemented!("not used by indexer tests")
        }

        async fn update_item_distill_result_if_current(
            &self,
            _item_id: u64,
            _expected_status: &str,
            _new_status: &str,
            _update: FreshItemDistillUpdate,
        ) -> Result<bool, AppError> {
            unimplemented!("not used by indexer tests")
        }

        async fn insert_topic(&self, _topic: NewFreshTopic) -> Result<FreshTopic, AppError> {
            unimplemented!("not used by indexer tests")
        }

        async fn find_topic_by_key(
            &self,
            _topic_key: &str,
        ) -> Result<Option<FreshTopic>, AppError> {
            unimplemented!("not used by indexer tests")
        }

        async fn link_topic_evidence(
            &self,
            _evidence: NewFreshTopicEvidence,
        ) -> Result<FreshTopicEvidence, AppError> {
            unimplemented!("not used by indexer tests")
        }

        async fn upsert_topic(&self, _topic: NewFreshTopic) -> Result<FreshTopic, AppError> {
            unimplemented!("not used by indexer tests")
        }

        async fn assign_topic_to_item_chunks(
            &self,
            _item_id: u64,
            _topic_id: u64,
        ) -> Result<u64, AppError> {
            unimplemented!("not used by indexer tests")
        }

        async fn insert_chunks(
            &self,
            chunks: &[NewFreshChunk],
        ) -> Result<Vec<FreshChunk>, AppError> {
            let mut saved = self.chunks.lock().await;
            let now = Utc::now();
            let mut rows = Vec::with_capacity(chunks.len());
            for chunk in chunks {
                if let Some(existing) = saved
                    .iter()
                    .find(|existing| {
                        existing.item_id == chunk.item_id
                            && existing.chunk_index == chunk.chunk_index
                    })
                    .cloned()
                {
                    rows.push(existing);
                    continue;
                }
                let row = FreshChunk {
                    id: saved.len() as u64 + 1,
                    item_id: chunk.item_id,
                    topic_id: chunk.topic_id,
                    chunk_index: chunk.chunk_index,
                    content: chunk.content.clone(),
                    content_hash: chunk.content_hash.clone(),
                    token_count: chunk.token_count,
                    metadata: chunk.metadata.clone(),
                    vector_id: None,
                    embedding_provider: None,
                    embedding_model: None,
                    embedding_dimension: None,
                    active: 1,
                    indexed_at: None,
                    expires_at: chunk.expires_at,
                    created_at: now,
                    updated_at: now,
                };
                saved.push(row.clone());
                rows.push(row);
            }
            Ok(rows)
        }

        async fn find_chunk_by_id(&self, chunk_id: u64) -> Result<Option<FreshChunk>, AppError> {
            Ok(self
                .chunks
                .lock()
                .await
                .iter()
                .find(|chunk| chunk.id == chunk_id)
                .cloned())
        }

        async fn find_chunks_by_item(&self, item_id: u64) -> Result<Vec<FreshChunk>, AppError> {
            Ok(self
                .chunks
                .lock()
                .await
                .iter()
                .filter(|chunk| chunk.item_id == item_id)
                .cloned()
                .collect())
        }

        async fn list_indexable_chunks(
            &self,
            now: DateTime<Utc>,
            limit: u64,
        ) -> Result<Vec<FreshChunk>, AppError> {
            Ok(self
                .chunks
                .lock()
                .await
                .iter()
                .filter(|chunk| {
                    chunk.active == 1 && chunk.vector_id.is_none() && chunk.expires_at > now
                })
                .take(limit as usize)
                .cloned()
                .collect())
        }

        async fn mark_chunk_indexed(
            &self,
            chunk_id: u64,
            vector_id: String,
            embedding_provider: String,
            embedding_model: String,
            embedding_dimension: u32,
        ) -> Result<bool, AppError> {
            let mut chunks = self.chunks.lock().await;
            let Some(chunk) = chunks
                .iter_mut()
                .find(|chunk| chunk.id == chunk_id && chunk.vector_id.is_none())
            else {
                return Ok(false);
            };
            chunk.vector_id = Some(vector_id);
            chunk.embedding_provider = Some(embedding_provider);
            chunk.embedding_model = Some(embedding_model);
            chunk.embedding_dimension = Some(embedding_dimension);
            chunk.indexed_at = Some(Utc::now());
            Ok(true)
        }

        async fn list_expired_indexed_chunks(
            &self,
            now: DateTime<Utc>,
            limit: u64,
        ) -> Result<Vec<FreshChunk>, AppError> {
            Ok(self
                .chunks
                .lock()
                .await
                .iter()
                .filter(|chunk| chunk.vector_id.is_some() && chunk.expires_at <= now)
                .take(limit as usize)
                .cloned()
                .collect())
        }

        async fn mark_chunk_vector_deleted(
            &self,
            chunk_id: u64,
            vector_id: &str,
        ) -> Result<bool, AppError> {
            let mut chunks = self.chunks.lock().await;
            let Some(chunk) = chunks.iter_mut().find(|chunk| {
                chunk.id == chunk_id && chunk.vector_id.as_deref() == Some(vector_id)
            }) else {
                return Ok(false);
            };
            chunk.vector_id = None;
            chunk.embedding_provider = None;
            chunk.embedding_model = None;
            chunk.embedding_dimension = None;
            chunk.indexed_at = None;
            chunk.active = 0;
            Ok(true)
        }
    }
}
