use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sea_orm::sea_query::Expr;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, ExprTrait, QueryFilter,
    QueryOrder, QuerySelect,
};

use crate::domain::fresh_context::{
    FreshChunk, FreshContextRepoT, FreshItem, FreshItemDistillUpdate, FreshSource, FreshTopic,
    FreshTopicEvidence, NewFreshChunk, NewFreshItem, NewFreshSource, NewFreshTopic,
    NewFreshTopicEvidence, fresh_status,
};
use crate::shared::error::AppError;

use super::super::entities::{
    fresh_chunks, fresh_items, fresh_sources, fresh_topic_evidence, fresh_topics,
};

pub struct FreshContextRepo {
    db: DatabaseConnection,
}

impl FreshContextRepo {
    pub fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }

    async fn update_existing_topic(
        &self,
        existing: FreshTopic,
        topic: NewFreshTopic,
    ) -> Result<FreshTopic, AppError> {
        let first_seen_at = std::cmp::Ord::min(existing.first_seen_at, topic.first_seen_at);
        let last_seen_at = std::cmp::Ord::max(existing.last_seen_at, topic.last_seen_at);
        let heat_score = existing.heat_score.max(topic.heat_score);
        let freshness_score = existing.freshness_score.max(topic.freshness_score);
        let expires_at = std::cmp::Ord::max(existing.expires_at, topic.expires_at);
        let summary = topic.summary.or(existing.summary);
        let entities = topic.entities.or(existing.entities);
        let risk_flags = topic.risk_flags.or(existing.risk_flags);
        let metadata = topic.metadata.or(existing.metadata);

        let active: fresh_topics::ActiveModel = fresh_topics::ActiveModel::builder()
            .set_id(existing.id)
            .set_topic_key(existing.topic_key)
            .set_title(if topic.title.trim().is_empty() {
                existing.title
            } else {
                topic.title
            })
            .set_summary(summary)
            .set_entities(entities.map(Into::into))
            .set_first_seen_at(first_seen_at.naive_utc())
            .set_last_seen_at(last_seen_at.naive_utc())
            .set_heat_score(heat_score)
            .set_freshness_score(freshness_score)
            .set_expires_at(expires_at.naive_utc())
            .set_status(topic.status)
            .set_risk_flags(risk_flags.map(Into::into))
            .set_metadata(metadata.map(Into::into))
            .set_created_at(existing.created_at.naive_utc())
            .set_updated_at(Utc::now().naive_utc())
            .set_deleted_at(existing.deleted_at.map(|t| t.naive_utc()))
            .into();
        let saved = active
            .update(&self.db)
            .await
            .map_err(|e| AppError::internal(format!("update fresh topic: {e}")))?;
        Ok(map_topic(saved))
    }
}

fn map_source(m: fresh_sources::Model) -> FreshSource {
    FreshSource {
        id: m.id,
        name: m.name,
        source_kind: m.source_kind,
        base_url: m.base_url,
        allowed_domains: m.allowed_domains.map(Into::into),
        trust_level: m.trust_level,
        reliability_score: m.reliability_score,
        crawl_interval_secs: m.crawl_interval_secs,
        default_ttl_secs: m.default_ttl_secs,
        risk_policy: m.risk_policy,
        enabled: m.enabled,
        metadata: m.metadata.map(Into::into),
        created_at: m.created_at.and_utc(),
        updated_at: m.updated_at.and_utc(),
        deleted_at: m.deleted_at.map(|t| t.and_utc()),
    }
}

fn map_item(m: fresh_items::Model) -> FreshItem {
    FreshItem {
        id: m.id,
        source_id: m.source_id,
        url: m.url,
        canonical_url: m.canonical_url,
        url_hash: m.url_hash,
        title: m.title,
        raw_text: m.raw_text,
        clean_text: m.clean_text,
        summary: m.summary,
        published_at: m.published_at.map(|t| t.and_utc()),
        fetched_at: m.fetched_at.and_utc(),
        expires_at: m.expires_at.and_utc(),
        content_hash: m.content_hash,
        status: m.status,
        reliability_score: m.reliability_score,
        freshness_score: m.freshness_score,
        heat_score: m.heat_score,
        rumor_level: m.rumor_level,
        risk_flags: m.risk_flags.map(Into::into),
        metadata: m.metadata.map(Into::into),
        created_at: m.created_at.and_utc(),
        updated_at: m.updated_at.and_utc(),
        deleted_at: m.deleted_at.map(|t| t.and_utc()),
    }
}

fn map_topic(m: fresh_topics::Model) -> FreshTopic {
    FreshTopic {
        id: m.id,
        topic_key: m.topic_key,
        title: m.title,
        summary: m.summary,
        entities: m.entities.map(Into::into),
        first_seen_at: m.first_seen_at.and_utc(),
        last_seen_at: m.last_seen_at.and_utc(),
        heat_score: m.heat_score,
        freshness_score: m.freshness_score,
        expires_at: m.expires_at.and_utc(),
        status: m.status,
        risk_flags: m.risk_flags.map(Into::into),
        metadata: m.metadata.map(Into::into),
        created_at: m.created_at.and_utc(),
        updated_at: m.updated_at.and_utc(),
        deleted_at: m.deleted_at.map(|t| t.and_utc()),
    }
}

fn map_chunk(m: fresh_chunks::Model) -> FreshChunk {
    FreshChunk {
        id: m.id,
        item_id: m.item_id,
        topic_id: m.topic_id,
        chunk_index: m.chunk_index,
        content: m.content,
        content_hash: m.content_hash,
        token_count: m.token_count,
        metadata: m.metadata.map(Into::into),
        vector_id: m.vector_id,
        embedding_provider: m.embedding_provider,
        embedding_model: m.embedding_model,
        embedding_dimension: m.embedding_dimension,
        active: m.active,
        indexed_at: m.indexed_at.map(|t| t.and_utc()),
        expires_at: m.expires_at.and_utc(),
        created_at: m.created_at.and_utc(),
        updated_at: m.updated_at.and_utc(),
    }
}

fn map_evidence(m: fresh_topic_evidence::Model) -> FreshTopicEvidence {
    FreshTopicEvidence {
        topic_id: m.topic_id,
        item_id: m.item_id,
        stance: m.stance,
        confidence: m.confidence,
        created_at: m.created_at.and_utc(),
    }
}

#[async_trait]
impl FreshContextRepoT for FreshContextRepo {
    async fn insert_source(&self, source: NewFreshSource) -> Result<FreshSource, AppError> {
        let now = Utc::now().naive_utc();
        let active: fresh_sources::ActiveModel = fresh_sources::ActiveModel::builder()
            .set_name(source.name)
            .set_source_kind(source.source_kind)
            .set_base_url(source.base_url)
            .set_allowed_domains(source.allowed_domains.map(Into::into))
            .set_trust_level(source.trust_level)
            .set_reliability_score(source.reliability_score)
            .set_crawl_interval_secs(source.crawl_interval_secs)
            .set_default_ttl_secs(source.default_ttl_secs)
            .set_risk_policy(source.risk_policy)
            .set_enabled(source.enabled)
            .set_metadata(source.metadata.map(Into::into))
            .set_created_at(now)
            .set_updated_at(now)
            .set_deleted_at(None)
            .into();
        let saved = active
            .insert(&self.db)
            .await
            .map_err(|e| AppError::internal(format!("insert fresh source: {e}")))?;
        Ok(map_source(saved))
    }

    async fn list_enabled_sources(&self, limit: u64) -> Result<Vec<FreshSource>, AppError> {
        let rows = fresh_sources::Entity::find()
            .filter(fresh_sources::Column::Enabled.eq(1))
            .filter(fresh_sources::Column::DeletedAt.is_null())
            .order_by_asc(fresh_sources::Column::Id)
            .limit(limit)
            .all(&self.db)
            .await
            .map_err(|e| AppError::internal(format!("list fresh sources: {e}")))?;
        Ok(rows.into_iter().map(map_source).collect())
    }

    async fn find_source_by_id(&self, source_id: u64) -> Result<Option<FreshSource>, AppError> {
        let row = fresh_sources::Entity::find_by_id(source_id)
            .filter(fresh_sources::Column::DeletedAt.is_null())
            .one(&self.db)
            .await
            .map_err(|e| AppError::internal(format!("find fresh source: {e}")))?;
        Ok(row.map(map_source))
    }

    async fn insert_item(&self, item: NewFreshItem) -> Result<FreshItem, AppError> {
        let now = Utc::now().naive_utc();
        let active: fresh_items::ActiveModel = fresh_items::ActiveModel::builder()
            .set_source_id(item.source_id)
            .set_url(item.url)
            .set_canonical_url(item.canonical_url)
            .set_url_hash(item.url_hash)
            .set_title(item.title)
            .set_raw_text(item.raw_text)
            .set_clean_text(item.clean_text)
            .set_summary(item.summary)
            .set_published_at(item.published_at.map(|t| t.naive_utc()))
            .set_fetched_at(item.fetched_at.naive_utc())
            .set_expires_at(item.expires_at.naive_utc())
            .set_content_hash(item.content_hash)
            .set_status(item.status)
            .set_reliability_score(item.reliability_score)
            .set_freshness_score(item.freshness_score)
            .set_heat_score(item.heat_score)
            .set_rumor_level(item.rumor_level)
            .set_risk_flags(item.risk_flags.map(Into::into))
            .set_metadata(item.metadata.map(Into::into))
            .set_created_at(now)
            .set_updated_at(now)
            .set_deleted_at(None)
            .into();
        let saved = active
            .insert(&self.db)
            .await
            .map_err(|e| AppError::internal(format!("insert fresh item: {e}")))?;
        Ok(map_item(saved))
    }

    async fn find_item_by_source_content(
        &self,
        source_id: u64,
        content_hash: &str,
    ) -> Result<Option<FreshItem>, AppError> {
        let row = fresh_items::Entity::find()
            .filter(fresh_items::Column::SourceId.eq(source_id))
            .filter(fresh_items::Column::ContentHash.eq(content_hash))
            .filter(fresh_items::Column::DeletedAt.is_null())
            .one(&self.db)
            .await
            .map_err(|e| AppError::internal(format!("find fresh item: {e}")))?;
        Ok(row.map(map_item))
    }

    async fn find_item_by_id(&self, item_id: u64) -> Result<Option<FreshItem>, AppError> {
        let row = fresh_items::Entity::find_by_id(item_id)
            .filter(fresh_items::Column::DeletedAt.is_null())
            .one(&self.db)
            .await
            .map_err(|e| AppError::internal(format!("find fresh item by id: {e}")))?;
        Ok(row.map(map_item))
    }

    async fn list_active_items(
        &self,
        now: DateTime<Utc>,
        limit: u64,
    ) -> Result<Vec<FreshItem>, AppError> {
        let rows = fresh_items::Entity::find()
            .filter(fresh_items::Column::Status.eq(fresh_status::PUBLISHED))
            .filter(fresh_items::Column::ExpiresAt.gt(now.naive_utc()))
            .filter(fresh_items::Column::DeletedAt.is_null())
            .order_by_desc(fresh_items::Column::FetchedAt)
            .limit(limit)
            .all(&self.db)
            .await
            .map_err(|e| AppError::internal(format!("list active fresh items: {e}")))?;
        Ok(rows.into_iter().map(map_item).collect())
    }

    async fn list_chunkable_items(
        &self,
        now: DateTime<Utc>,
        limit: u64,
    ) -> Result<Vec<FreshItem>, AppError> {
        let rows = fresh_items::Entity::find()
            .filter(fresh_items::Column::Status.eq(fresh_status::PUBLISHED))
            .filter(fresh_items::Column::ExpiresAt.gt(now.naive_utc()))
            .filter(fresh_items::Column::DeletedAt.is_null())
            .filter(Expr::cust(
                "NOT EXISTS (SELECT 1 FROM fresh_chunks WHERE fresh_chunks.item_id = fresh_items.id)",
            ))
            .order_by_asc(fresh_items::Column::FetchedAt)
            .limit(limit)
            .all(&self.db)
            .await
            .map_err(|e| AppError::internal(format!("list chunkable fresh items: {e}")))?;
        Ok(rows.into_iter().map(map_item).collect())
    }

    async fn list_items_by_status(
        &self,
        status: &str,
        now: DateTime<Utc>,
        limit: u64,
    ) -> Result<Vec<FreshItem>, AppError> {
        let rows = fresh_items::Entity::find()
            .filter(fresh_items::Column::Status.eq(status))
            .filter(fresh_items::Column::ExpiresAt.gt(now.naive_utc()))
            .filter(fresh_items::Column::DeletedAt.is_null())
            .order_by_asc(fresh_items::Column::FetchedAt)
            .limit(limit)
            .all(&self.db)
            .await
            .map_err(|e| AppError::internal(format!("list fresh items by status: {e}")))?;
        Ok(rows.into_iter().map(map_item).collect())
    }

    async fn expire_items(&self, now: DateTime<Utc>) -> Result<u64, AppError> {
        let result = fresh_items::Entity::update_many()
            .col_expr(
                fresh_items::Column::Status,
                Expr::value(fresh_status::EXPIRED),
            )
            .filter(fresh_items::Column::ExpiresAt.lte(now.naive_utc()))
            .filter(fresh_items::Column::Status.ne(fresh_status::EXPIRED))
            .exec(&self.db)
            .await
            .map_err(|e| AppError::internal(format!("expire fresh items: {e}")))?;

        fresh_chunks::Entity::update_many()
            .col_expr(fresh_chunks::Column::Active, Expr::value(0))
            .filter(fresh_chunks::Column::ExpiresAt.lte(now.naive_utc()))
            .filter(fresh_chunks::Column::Active.eq(1))
            .exec(&self.db)
            .await
            .map_err(|e| AppError::internal(format!("expire fresh chunks: {e}")))?;

        Ok(result.rows_affected)
    }

    async fn update_item_status_if_current(
        &self,
        item_id: u64,
        expected_status: &str,
        new_status: &str,
        metadata: Option<serde_json::Value>,
    ) -> Result<bool, AppError> {
        let mut update = fresh_items::Entity::update_many()
            .col_expr(fresh_items::Column::Status, Expr::value(new_status))
            .col_expr(
                fresh_items::Column::UpdatedAt,
                Expr::value(Utc::now().naive_utc()),
            )
            .filter(fresh_items::Column::Id.eq(item_id))
            .filter(fresh_items::Column::Status.eq(expected_status))
            .filter(fresh_items::Column::DeletedAt.is_null());
        if let Some(metadata) = metadata {
            update = update.col_expr(
                fresh_items::Column::Metadata,
                Expr::value(sea_orm::JsonValue::from(metadata)),
            );
        }
        let result = update
            .exec(&self.db)
            .await
            .map_err(|e| AppError::internal(format!("update fresh item status: {e}")))?;
        Ok(result.rows_affected == 1)
    }

    async fn update_item_distill_result_if_current(
        &self,
        item_id: u64,
        expected_status: &str,
        new_status: &str,
        update: FreshItemDistillUpdate,
    ) -> Result<bool, AppError> {
        let result = fresh_items::Entity::update_many()
            .col_expr(fresh_items::Column::Status, Expr::value(new_status))
            .col_expr(fresh_items::Column::Title, Expr::value(update.title))
            .col_expr(fresh_items::Column::Summary, Expr::value(update.summary))
            .col_expr(
                fresh_items::Column::PublishedAt,
                Expr::value(update.published_at.map(|t| t.naive_utc())),
            )
            .col_expr(
                fresh_items::Column::FreshnessScore,
                Expr::value(update.freshness_score),
            )
            .col_expr(
                fresh_items::Column::HeatScore,
                Expr::value(update.heat_score),
            )
            .col_expr(
                fresh_items::Column::RumorLevel,
                Expr::value(update.rumor_level),
            )
            .col_expr(
                fresh_items::Column::RiskFlags,
                Expr::value(update.risk_flags.map(sea_orm::JsonValue::from)),
            )
            .col_expr(
                fresh_items::Column::Metadata,
                Expr::value(update.metadata.map(sea_orm::JsonValue::from)),
            )
            .col_expr(
                fresh_items::Column::UpdatedAt,
                Expr::value(Utc::now().naive_utc()),
            )
            .filter(fresh_items::Column::Id.eq(item_id))
            .filter(fresh_items::Column::Status.eq(expected_status))
            .filter(fresh_items::Column::DeletedAt.is_null())
            .exec(&self.db)
            .await
            .map_err(|e| AppError::internal(format!("update fresh distill result: {e}")))?;
        Ok(result.rows_affected == 1)
    }

    async fn insert_topic(&self, topic: NewFreshTopic) -> Result<FreshTopic, AppError> {
        let now = Utc::now().naive_utc();
        let active: fresh_topics::ActiveModel = fresh_topics::ActiveModel::builder()
            .set_topic_key(topic.topic_key)
            .set_title(topic.title)
            .set_summary(topic.summary)
            .set_entities(topic.entities.map(Into::into))
            .set_first_seen_at(topic.first_seen_at.naive_utc())
            .set_last_seen_at(topic.last_seen_at.naive_utc())
            .set_heat_score(topic.heat_score)
            .set_freshness_score(topic.freshness_score)
            .set_expires_at(topic.expires_at.naive_utc())
            .set_status(topic.status)
            .set_risk_flags(topic.risk_flags.map(Into::into))
            .set_metadata(topic.metadata.map(Into::into))
            .set_created_at(now)
            .set_updated_at(now)
            .set_deleted_at(None)
            .into();
        let saved = active
            .insert(&self.db)
            .await
            .map_err(|e| AppError::internal(format!("insert fresh topic: {e}")))?;
        Ok(map_topic(saved))
    }

    async fn upsert_topic(&self, topic: NewFreshTopic) -> Result<FreshTopic, AppError> {
        if let Some(existing) = self.find_topic_by_key(&topic.topic_key).await? {
            return self.update_existing_topic(existing, topic).await;
        }

        match self.insert_topic(topic.clone()).await {
            Ok(saved) => Ok(saved),
            Err(insert_error) => {
                if let Some(existing) = self.find_topic_by_key(&topic.topic_key).await? {
                    self.update_existing_topic(existing, topic).await
                } else {
                    Err(insert_error)
                }
            }
        }
    }

    async fn find_topic_by_key(&self, topic_key: &str) -> Result<Option<FreshTopic>, AppError> {
        let row = fresh_topics::Entity::find()
            .filter(fresh_topics::Column::TopicKey.eq(topic_key))
            .filter(fresh_topics::Column::DeletedAt.is_null())
            .one(&self.db)
            .await
            .map_err(|e| AppError::internal(format!("find fresh topic: {e}")))?;
        Ok(row.map(map_topic))
    }

    async fn link_topic_evidence(
        &self,
        evidence: NewFreshTopicEvidence,
    ) -> Result<FreshTopicEvidence, AppError> {
        let active: fresh_topic_evidence::ActiveModel =
            fresh_topic_evidence::ActiveModel::builder()
                .set_topic_id(evidence.topic_id)
                .set_item_id(evidence.item_id)
                .set_stance(evidence.stance)
                .set_confidence(evidence.confidence)
                .set_created_at(Utc::now().naive_utc())
                .into();
        let saved = active.insert(&self.db).await;
        let saved = match saved {
            Ok(saved) => saved,
            Err(err) => {
                let existing = fresh_topic_evidence::Entity::find()
                    .filter(fresh_topic_evidence::Column::TopicId.eq(evidence.topic_id))
                    .filter(fresh_topic_evidence::Column::ItemId.eq(evidence.item_id))
                    .one(&self.db)
                    .await
                    .map_err(|e| {
                        AppError::internal(format!(
                            "find existing fresh topic evidence after insert failure: {e}"
                        ))
                    })?;
                match existing {
                    Some(existing) => existing,
                    None => {
                        return Err(AppError::internal(format!(
                            "link fresh topic evidence: {err}"
                        )));
                    }
                }
            }
        };
        Ok(map_evidence(saved))
    }

    async fn assign_topic_to_item_chunks(
        &self,
        item_id: u64,
        topic_id: u64,
    ) -> Result<u64, AppError> {
        let now = Utc::now().naive_utc();
        let result = fresh_chunks::Entity::update_many()
            .col_expr(fresh_chunks::Column::TopicId, Expr::value(Some(topic_id)))
            .col_expr(fresh_chunks::Column::UpdatedAt, Expr::value(now))
            .filter(fresh_chunks::Column::ItemId.eq(item_id))
            .filter(
                fresh_chunks::Column::TopicId
                    .is_null()
                    .or(fresh_chunks::Column::TopicId.ne(topic_id)),
            )
            .exec(&self.db)
            .await
            .map_err(|e| AppError::internal(format!("assign fresh chunk topic: {e}")))?;
        Ok(result.rows_affected)
    }

    async fn insert_chunks(&self, chunks: &[NewFreshChunk]) -> Result<Vec<FreshChunk>, AppError> {
        let mut saved = Vec::with_capacity(chunks.len());
        for chunk in chunks {
            let now = Utc::now().naive_utc();
            let active: fresh_chunks::ActiveModel = fresh_chunks::ActiveModel::builder()
                .set_item_id(chunk.item_id)
                .set_topic_id(chunk.topic_id)
                .set_chunk_index(chunk.chunk_index)
                .set_content(chunk.content.clone())
                .set_content_hash(chunk.content_hash.clone())
                .set_token_count(chunk.token_count)
                .set_metadata(chunk.metadata.clone().map(Into::into))
                .set_vector_id(None)
                .set_embedding_provider(None)
                .set_embedding_model(None)
                .set_embedding_dimension(None)
                .set_active(1)
                .set_indexed_at(None)
                .set_expires_at(chunk.expires_at.naive_utc())
                .set_created_at(now)
                .set_updated_at(now)
                .into();
            let row = match active.insert(&self.db).await {
                Ok(row) => row,
                Err(err) => {
                    let existing = fresh_chunks::Entity::find()
                        .filter(fresh_chunks::Column::ItemId.eq(chunk.item_id))
                        .filter(fresh_chunks::Column::ChunkIndex.eq(chunk.chunk_index))
                        .one(&self.db)
                        .await
                        .map_err(|e| {
                            AppError::internal(format!(
                                "find existing fresh chunk after insert failure: {e}"
                            ))
                        })?;
                    match existing {
                        Some(existing) if existing.content_hash == chunk.content_hash => existing,
                        Some(_) => {
                            return Err(AppError::Conflict(format!(
                                "fresh chunk item={} index={} already exists with different hash",
                                chunk.item_id, chunk.chunk_index
                            )));
                        }
                        None => {
                            return Err(AppError::internal(format!("insert fresh chunk: {err}")));
                        }
                    }
                }
            };
            saved.push(map_chunk(row));
        }
        Ok(saved)
    }

    async fn find_chunks_by_item(&self, item_id: u64) -> Result<Vec<FreshChunk>, AppError> {
        let rows = fresh_chunks::Entity::find()
            .filter(fresh_chunks::Column::ItemId.eq(item_id))
            .order_by_asc(fresh_chunks::Column::ChunkIndex)
            .all(&self.db)
            .await
            .map_err(|e| AppError::internal(format!("find fresh chunks by item: {e}")))?;
        Ok(rows.into_iter().map(map_chunk).collect())
    }

    async fn find_chunk_by_id(&self, chunk_id: u64) -> Result<Option<FreshChunk>, AppError> {
        let row = fresh_chunks::Entity::find_by_id(chunk_id)
            .one(&self.db)
            .await
            .map_err(|e| AppError::internal(format!("find fresh chunk by id: {e}")))?;
        Ok(row.map(map_chunk))
    }

    async fn list_indexable_chunks(
        &self,
        now: DateTime<Utc>,
        limit: u64,
    ) -> Result<Vec<FreshChunk>, AppError> {
        let rows = fresh_chunks::Entity::find()
            .filter(fresh_chunks::Column::Active.eq(1))
            .filter(fresh_chunks::Column::VectorId.is_null())
            .filter(fresh_chunks::Column::ExpiresAt.gt(now.naive_utc()))
            .order_by_asc(fresh_chunks::Column::CreatedAt)
            .limit(limit)
            .all(&self.db)
            .await
            .map_err(|e| AppError::internal(format!("list indexable fresh chunks: {e}")))?;
        Ok(rows.into_iter().map(map_chunk).collect())
    }

    async fn mark_chunk_indexed(
        &self,
        chunk_id: u64,
        vector_id: String,
        embedding_provider: String,
        embedding_model: String,
        embedding_dimension: u32,
    ) -> Result<bool, AppError> {
        let now = Utc::now().naive_utc();
        let result = fresh_chunks::Entity::update_many()
            .col_expr(fresh_chunks::Column::VectorId, Expr::value(Some(vector_id)))
            .col_expr(
                fresh_chunks::Column::EmbeddingProvider,
                Expr::value(Some(embedding_provider)),
            )
            .col_expr(
                fresh_chunks::Column::EmbeddingModel,
                Expr::value(Some(embedding_model)),
            )
            .col_expr(
                fresh_chunks::Column::EmbeddingDimension,
                Expr::value(Some(embedding_dimension)),
            )
            .col_expr(fresh_chunks::Column::IndexedAt, Expr::value(Some(now)))
            .col_expr(fresh_chunks::Column::UpdatedAt, Expr::value(now))
            .filter(fresh_chunks::Column::Id.eq(chunk_id))
            .filter(fresh_chunks::Column::Active.eq(1))
            .filter(fresh_chunks::Column::VectorId.is_null())
            .filter(fresh_chunks::Column::ExpiresAt.gt(now))
            .exec(&self.db)
            .await
            .map_err(|e| AppError::internal(format!("mark fresh chunk indexed: {e}")))?;
        Ok(result.rows_affected == 1)
    }

    async fn list_expired_indexed_chunks(
        &self,
        now: DateTime<Utc>,
        limit: u64,
    ) -> Result<Vec<FreshChunk>, AppError> {
        let rows = fresh_chunks::Entity::find()
            .filter(fresh_chunks::Column::VectorId.is_not_null())
            .filter(fresh_chunks::Column::ExpiresAt.lte(now.naive_utc()))
            .order_by_asc(fresh_chunks::Column::ExpiresAt)
            .limit(limit)
            .all(&self.db)
            .await
            .map_err(|e| AppError::internal(format!("list expired indexed fresh chunks: {e}")))?;
        Ok(rows.into_iter().map(map_chunk).collect())
    }

    async fn mark_chunk_vector_deleted(
        &self,
        chunk_id: u64,
        vector_id: &str,
    ) -> Result<bool, AppError> {
        let now = Utc::now().naive_utc();
        let result = fresh_chunks::Entity::update_many()
            .col_expr(
                fresh_chunks::Column::VectorId,
                Expr::value(Option::<String>::None),
            )
            .col_expr(
                fresh_chunks::Column::EmbeddingProvider,
                Expr::value(Option::<String>::None),
            )
            .col_expr(
                fresh_chunks::Column::EmbeddingModel,
                Expr::value(Option::<String>::None),
            )
            .col_expr(
                fresh_chunks::Column::EmbeddingDimension,
                Expr::value(Option::<u32>::None),
            )
            .col_expr(
                fresh_chunks::Column::IndexedAt,
                Expr::value(Option::<chrono::NaiveDateTime>::None),
            )
            .col_expr(fresh_chunks::Column::Active, Expr::value(0))
            .col_expr(fresh_chunks::Column::UpdatedAt, Expr::value(now))
            .filter(fresh_chunks::Column::Id.eq(chunk_id))
            .filter(fresh_chunks::Column::VectorId.eq(vector_id))
            .exec(&self.db)
            .await
            .map_err(|e| AppError::internal(format!("mark fresh chunk vector deleted: {e}")))?;
        Ok(result.rows_affected == 1)
    }
}
