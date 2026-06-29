use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sea_orm::sea_query::Expr;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, ExprTrait, QueryFilter,
    QueryOrder, QuerySelect, Set,
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

        let active = fresh_topics::ActiveModel {
            id: Set(existing.id),
            topic_key: Set(existing.topic_key),
            title: Set(if topic.title.trim().is_empty() {
                existing.title
            } else {
                topic.title
            }),
            summary: Set(summary.map(Into::into)),
            entities: Set(entities.map(Into::into)),
            first_seen_at: Set(first_seen_at.naive_utc()),
            last_seen_at: Set(last_seen_at.naive_utc()),
            heat_score: Set(heat_score),
            freshness_score: Set(freshness_score),
            expires_at: Set(expires_at.naive_utc()),
            status: Set(topic.status),
            risk_flags: Set(risk_flags.map(Into::into)),
            metadata: Set(metadata.map(Into::into)),
            created_at: Set(existing.created_at.naive_utc()),
            updated_at: Set(Utc::now().naive_utc()),
            deleted_at: Set(existing.deleted_at.map(|t| t.naive_utc())),
        };
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
        let active = fresh_sources::ActiveModel {
            id: sea_orm::ActiveValue::NotSet,
            name: Set(source.name),
            source_kind: Set(source.source_kind),
            base_url: Set(source.base_url),
            allowed_domains: Set(source.allowed_domains.map(Into::into)),
            trust_level: Set(source.trust_level),
            reliability_score: Set(source.reliability_score),
            crawl_interval_secs: Set(source.crawl_interval_secs),
            default_ttl_secs: Set(source.default_ttl_secs),
            risk_policy: Set(source.risk_policy),
            enabled: Set(source.enabled),
            metadata: Set(source.metadata.map(Into::into)),
            created_at: Set(now),
            updated_at: Set(now),
            deleted_at: Set(None),
        };
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
        let active = fresh_items::ActiveModel {
            id: sea_orm::ActiveValue::NotSet,
            source_id: Set(item.source_id),
            url: Set(item.url),
            canonical_url: Set(item.canonical_url),
            url_hash: Set(item.url_hash),
            title: Set(item.title),
            raw_text: Set(item.raw_text),
            clean_text: Set(item.clean_text),
            summary: Set(item.summary),
            published_at: Set(item.published_at.map(|t| t.naive_utc())),
            fetched_at: Set(item.fetched_at.naive_utc()),
            expires_at: Set(item.expires_at.naive_utc()),
            content_hash: Set(item.content_hash),
            status: Set(item.status),
            reliability_score: Set(item.reliability_score),
            freshness_score: Set(item.freshness_score),
            heat_score: Set(item.heat_score),
            rumor_level: Set(item.rumor_level),
            risk_flags: Set(item.risk_flags.map(Into::into)),
            metadata: Set(item.metadata.map(Into::into)),
            created_at: Set(now),
            updated_at: Set(now),
            deleted_at: Set(None),
        };
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
        let active = fresh_topics::ActiveModel {
            id: sea_orm::ActiveValue::NotSet,
            topic_key: Set(topic.topic_key),
            title: Set(topic.title),
            summary: Set(topic.summary),
            entities: Set(topic.entities.map(Into::into)),
            first_seen_at: Set(topic.first_seen_at.naive_utc()),
            last_seen_at: Set(topic.last_seen_at.naive_utc()),
            heat_score: Set(topic.heat_score),
            freshness_score: Set(topic.freshness_score),
            expires_at: Set(topic.expires_at.naive_utc()),
            status: Set(topic.status),
            risk_flags: Set(topic.risk_flags.map(Into::into)),
            metadata: Set(topic.metadata.map(Into::into)),
            created_at: Set(now),
            updated_at: Set(now),
            deleted_at: Set(None),
        };
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
        let active = fresh_topic_evidence::ActiveModel {
            id: sea_orm::ActiveValue::NotSet,
            topic_id: Set(evidence.topic_id),
            item_id: Set(evidence.item_id),
            stance: Set(evidence.stance),
            confidence: Set(evidence.confidence),
            created_at: Set(Utc::now().naive_utc()),
        };
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
            let active = fresh_chunks::ActiveModel {
                id: sea_orm::ActiveValue::NotSet,
                item_id: Set(chunk.item_id),
                topic_id: Set(chunk.topic_id),
                chunk_index: Set(chunk.chunk_index),
                content: Set(chunk.content.clone()),
                content_hash: Set(chunk.content_hash.clone()),
                token_count: Set(chunk.token_count),
                metadata: Set(chunk.metadata.clone().map(Into::into)),
                vector_id: Set(None),
                embedding_provider: Set(None),
                embedding_model: Set(None),
                embedding_dimension: Set(None),
                active: Set(1),
                indexed_at: Set(None),
                expires_at: Set(chunk.expires_at.naive_utc()),
                created_at: Set(now),
                updated_at: Set(now),
            };
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
