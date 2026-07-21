use async_trait::async_trait;
use chrono::Utc;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityLoaderTrait, EntityTrait, Order,
    PaginatorTrait, QueryFilter, QueryOrder, Set,
};
use tracing::warn;

use super::super::entities::{knowledge_chunks, knowledge_documents, knowledge_embeddings};

use crate::domain::rag::{
    KnowledgeChunk, KnowledgeDocument, KnowledgeEmbedding, NewChunk, NewDocument, NewEmbedding,
    RAGRepoT,
};
use crate::shared::error::AppError;

pub struct RAGRepo {
    db: DatabaseConnection,
}

impl RAGRepo {
    pub fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }
}

fn map_document(m: knowledge_documents::Model) -> KnowledgeDocument {
    KnowledgeDocument {
        document_id: m.document_id,
        source_type: m.source_type,
        source_id: m.source_id,
        owner_user_id: m.owner_user_id,
        visibility: m.visibility,
        title: m.title,
        content_hash: m.content_hash,
        source_version: m.source_version,
        source_updated_at: m.source_updated_at.map(|t| t.and_utc()),
        metadata: m.metadata.map(|j| j.into()),
        status: m.status,
        created_at: m.created_at.and_utc(),
        updated_at: m.updated_at.and_utc(),
        deleted_at: m.deleted_at.map(|t| t.and_utc()),
    }
}

fn map_chunk(m: knowledge_chunks::Model) -> KnowledgeChunk {
    KnowledgeChunk {
        chunk_id: m.chunk_id,
        document_id: m.document_id,
        chunk_index: m.chunk_index,
        content: m.content,
        token_count: m.token_count,
        metadata: m.metadata.map(|j| j.into()),
        status: m.status,
        created_at: m.created_at.and_utc(),
    }
}

fn map_embedding(m: knowledge_embeddings::Model) -> KnowledgeEmbedding {
    KnowledgeEmbedding {
        embedding_id: m.embedding_id,
        chunk_id: m.chunk_id,
        provider: m.provider,
        model: m.model,
        dimension: m.dimension,
        embedding_json: m.embedding_json.into(),
        created_at: m.created_at.and_utc(),
    }
}

#[async_trait]
impl RAGRepoT for RAGRepo {
    async fn save_document(&self, doc: NewDocument) -> Result<KnowledgeDocument, AppError> {
        let now = Utc::now().naive_utc();
        let active: knowledge_documents::ActiveModel = knowledge_documents::ActiveModel::builder()
            .set_source_type(doc.source_type)
            .set_source_id(doc.source_id)
            .set_owner_user_id(None)
            .set_visibility("public")
            .set_title(doc.title)
            .set_content_hash(doc.content_hash)
            .set_source_version(None)
            .set_source_updated_at(None)
            .set_metadata(doc.metadata.map(Into::into))
            .set_status(doc.status)
            .set_created_at(now)
            .set_updated_at(now)
            .set_deleted_at(None)
            .into();

        let saved = active
            .insert(&self.db)
            .await
            .map_err(|e| AppError::internal(format!("failed to save knowledge document: {e}")))?;

        Ok(map_document(saved))
    }

    async fn find_document_by_source(
        &self,
        source_type: &str,
        source_id: Option<u64>,
    ) -> Result<Option<KnowledgeDocument>, AppError> {
        let mut query = knowledge_documents::Entity::find()
            .filter(knowledge_documents::Column::SourceType.eq(source_type));

        // SeaORM conditional: match source_id = value OR source_id IS NULL
        if let Some(id) = source_id {
            query = query.filter(knowledge_documents::Column::SourceId.eq(id));
        } else {
            query = query.filter(knowledge_documents::Column::SourceId.is_null());
        }

        let row = query
            .one(&self.db)
            .await
            .map_err(|e| AppError::internal(format!("failed to query knowledge_documents: {e}")))?;

        Ok(row.map(map_document))
    }

    async fn list_documents_by_source_type(
        &self,
        source_type: &str,
    ) -> Result<Vec<KnowledgeDocument>, AppError> {
        let rows = knowledge_documents::Entity::find()
            .filter(knowledge_documents::Column::SourceType.eq(source_type))
            .order_by(knowledge_documents::Column::CreatedAt, Order::Desc)
            .all(&self.db)
            .await
            .map_err(|e| AppError::internal(format!("failed to list knowledge_documents: {e}")))?;

        Ok(rows.into_iter().map(map_document).collect())
    }

    async fn save_chunks(&self, chunks: &[NewChunk]) -> Result<Vec<KnowledgeChunk>, AppError> {
        if chunks.is_empty() {
            return Ok(Vec::new());
        }

        // Capture the first document_id for re-query after bulk insert
        let first_doc_id = chunks[0].document_id;
        let now = Utc::now().naive_utc();
        let models: Vec<knowledge_chunks::ActiveModel> = chunks
            .iter()
            .map(|c| {
                knowledge_chunks::ActiveModel::builder()
                    .set_document_id(c.document_id)
                    .set_chunk_index(c.chunk_index)
                    .set_char_start(None)
                    .set_char_end(None)
                    .set_content(c.content.clone())
                    .set_content_hash(None)
                    .set_token_count(c.token_count)
                    .set_metadata(c.metadata.clone().map(Into::into))
                    .set_status(1)
                    .set_created_at(now)
                    .set_vector_id(None)
                    .set_embedding_provider(None)
                    .set_embedding_model(None)
                    .set_embedding_dimension(None)
                    .set_indexed_at(None)
                    .into()
            })
            .collect();

        knowledge_chunks::Entity::insert_many(models)
            .exec(&self.db)
            .await
            .map_err(|e| AppError::internal(format!("failed to save knowledge chunks: {e}")))?;

        // Re-query to return the actual persisted chunks
        let rows = knowledge_chunks::Entity::find()
            .filter(knowledge_chunks::Column::DocumentId.eq(first_doc_id))
            .order_by(knowledge_chunks::Column::ChunkIndex, Order::Asc)
            .all(&self.db)
            .await
            .map_err(|e| AppError::internal(format!("failed to re-query chunks: {e}")))?;

        Ok(rows
            .into_iter()
            .map(|m| KnowledgeChunk {
                chunk_id: m.chunk_id,
                document_id: m.document_id,
                chunk_index: m.chunk_index,
                content: m.content,
                token_count: m.token_count,
                metadata: m.metadata.map(|v| v.into()),
                status: m.status,
                created_at: m.created_at.and_utc(),
            })
            .collect())
    }

    async fn find_chunks_by_document(
        &self,
        document_id: u64,
    ) -> Result<Vec<KnowledgeChunk>, AppError> {
        let rows = knowledge_chunks::Entity::find()
            .filter(knowledge_chunks::Column::DocumentId.eq(document_id))
            .order_by(knowledge_chunks::Column::ChunkIndex, Order::Asc)
            .all(&self.db)
            .await
            .map_err(|e| AppError::internal(format!("failed to query knowledge_chunks: {e}")))?;

        Ok(rows.into_iter().map(map_chunk).collect())
    }

    async fn save_embedding(&self, emb: NewEmbedding) -> Result<KnowledgeEmbedding, AppError> {
        let now = Utc::now().naive_utc();
        let active: knowledge_embeddings::ActiveModel =
            knowledge_embeddings::ActiveModel::builder()
                .set_chunk_id(emb.chunk_id)
                .set_provider(emb.provider)
                .set_model(emb.model)
                .set_dimension(emb.dimension)
                .set_embedding_json(emb.embedding_json)
                .set_created_at(now)
                .into();

        let saved = active
            .insert(&self.db)
            .await
            .map_err(|e| AppError::internal(format!("failed to save knowledge embedding: {e}")))?;

        Ok(map_embedding(saved))
    }

    async fn find_embedding_by_chunk(
        &self,
        chunk_id: u64,
    ) -> Result<Option<KnowledgeEmbedding>, AppError> {
        let row = knowledge_embeddings::Entity::find()
            .filter(knowledge_embeddings::Column::ChunkId.eq(chunk_id))
            .one(&self.db)
            .await
            .map_err(|e| {
                AppError::internal(format!("failed to query knowledge_embeddings: {e}"))
            })?;

        Ok(row.map(map_embedding))
    }

    async fn search_by_keyword(
        &self,
        query: &str,
        top_k: u64,
    ) -> Result<Vec<(KnowledgeChunk, f64)>, AppError> {
        // Use LIKE-based keyword search as a fallback (no FULLTEXT required).
        let pattern = format!("%{query}%");
        let rows = knowledge_chunks::Entity::find()
            .filter(knowledge_chunks::Column::Content.like(&pattern))
            .paginate(&self.db, top_k)
            .fetch_page(0)
            .await
            .map_err(|e| AppError::internal(format!("failed to search knowledge_chunks: {e}")))?;

        // Assign a naive relevance score: longer matches are assumed more relevant.
        let chunks: Vec<(KnowledgeChunk, f64)> = rows
            .into_iter()
            .map(|row| {
                let count = row
                    .content
                    .to_lowercase()
                    .matches(&query.to_lowercase())
                    .count();
                let score = (count as f64).min(1.0);
                (map_chunk(row), score)
            })
            .collect();

        Ok(chunks)
    }

    async fn delete_document(&self, document_id: u64) -> Result<(), AppError> {
        // Delete embeddings for all chunks of this document.
        // We do this by finding chunk IDs first, then deleting their embeddings.
        let chunks = knowledge_chunks::Entity::find()
            .filter(knowledge_chunks::Column::DocumentId.eq(document_id))
            .all(&self.db)
            .await
            .map_err(|e| AppError::internal(format!("failed to find chunks for deletion: {e}")))?;

        for chunk in &chunks {
            knowledge_embeddings::Entity::delete_many()
                .filter(knowledge_embeddings::Column::ChunkId.eq(chunk.chunk_id))
                .exec(&self.db)
                .await
                .map_err(|e| {
                    AppError::internal(format!("failed to delete embeddings for chunk: {e}"))
                })?;
        }

        // Delete chunks (database has CASCADE, but we delete explicitly for safety)
        knowledge_chunks::Entity::delete_many()
            .filter(knowledge_chunks::Column::DocumentId.eq(document_id))
            .exec(&self.db)
            .await
            .map_err(|e| AppError::internal(format!("failed to delete chunks: {e}")))?;

        // Delete the document itself
        let result = knowledge_documents::Entity::delete_by_id(document_id)
            .exec(&self.db)
            .await
            .map_err(|e| AppError::internal(format!("failed to delete document: {e}")))?;

        if result.rows_affected == 0 {
            warn!(document_id, "delete_document: no rows affected");
        }
        Ok(())
    }

    async fn list_chunks_with_embeddings(
        &self,
    ) -> Result<Vec<(KnowledgeChunk, KnowledgeEmbedding)>, AppError> {
        let rows = knowledge_chunks::Entity::find()
            .find_also_related(knowledge_embeddings::Entity)
            .all(&self.db)
            .await
            .map_err(|e| {
                AppError::internal(format!("failed to list chunks with embeddings: {e}"))
            })?;

        Ok(rows
            .into_iter()
            .filter_map(|(chunk, emb_opt)| {
                emb_opt.map(|emb| (map_chunk(chunk), map_embedding(emb)))
            })
            .collect())
    }

    async fn find_chunk_by_id(&self, chunk_id: u64) -> Result<Option<KnowledgeChunk>, AppError> {
        let row = knowledge_chunks::Entity::find_by_id(chunk_id)
            .one(&self.db)
            .await
            .map_err(|e| AppError::internal(format!("failed to find chunk {chunk_id}: {e}")))?;
        Ok(row.map(map_chunk))
    }

    async fn find_document_by_id(
        &self,
        document_id: u64,
    ) -> Result<Option<KnowledgeDocument>, AppError> {
        let row = knowledge_documents::Entity::find_by_id(document_id)
            .one(&self.db)
            .await
            .map_err(|e| {
                AppError::internal(format!("failed to find document {document_id}: {e}"))
            })?;
        Ok(row.map(map_document))
    }

    async fn update_chunk_index_metadata(
        &self,
        chunk_id: u64,
        vector_id: String,
        embedding_provider: String,
        embedding_model: String,
        embedding_dimension: u32,
    ) -> Result<(), AppError> {
        let mut active: knowledge_chunks::ActiveModel =
            knowledge_chunks::Entity::find_by_id(chunk_id)
                .one(&self.db)
                .await
                .map_err(|e| AppError::internal(format!("find chunk {chunk_id}: {e}")))?
                .ok_or_else(|| AppError::NotFound(format!("chunk {chunk_id} not found")))?
                .into();
        active.vector_id = Set(Some(vector_id));
        active.embedding_provider = Set(Some(embedding_provider));
        active.embedding_model = Set(Some(embedding_model));
        active.embedding_dimension = Set(Some(embedding_dimension));
        active.indexed_at = Set(Some(Utc::now().naive_utc()));
        active.update(&self.db).await.map_err(|e| {
            AppError::internal(format!("update chunk index metadata {chunk_id}: {e}"))
        })?;
        Ok(())
    }

    async fn mark_chunk_unindexed(&self, chunk_id: u64) -> Result<(), AppError> {
        let mut active: knowledge_chunks::ActiveModel =
            knowledge_chunks::Entity::find_by_id(chunk_id)
                .one(&self.db)
                .await
                .map_err(|e| AppError::internal(format!("find chunk {chunk_id}: {e}")))?
                .ok_or_else(|| AppError::NotFound(format!("chunk {chunk_id} not found")))?
                .into();
        active.vector_id = Set(None);
        active.indexed_at = Set(None);
        active
            .update(&self.db)
            .await
            .map_err(|e| AppError::internal(format!("mark chunk unindexed {chunk_id}: {e}")))?;
        Ok(())
    }

    async fn list_indexable_chunks(
        &self,
        limit: u64,
    ) -> Result<Vec<(KnowledgeChunk, KnowledgeDocument)>, AppError> {
        let chunk_rows = knowledge_chunks::Entity::load()
            .with(knowledge_documents::Entity)
            .filter(knowledge_chunks::Column::Status.eq(1))
            .filter(knowledge_chunks::Column::VectorId.is_null())
            .filter(knowledge_documents::Column::Status.eq(1))
            .filter(knowledge_documents::Column::DeletedAt.is_null())
            .paginate(&self.db, limit)
            .fetch_page(0)
            .await
            .map_err(|e| AppError::internal(format!("list_indexable_chunks: {e}")))?;

        Ok(chunk_rows
            .into_iter()
            .filter_map(|chunk| {
                let document = chunk.knowledge_documents.as_ref()?.clone().into();
                let chunk = chunk.into();
                Some((map_chunk(chunk), map_document(document)))
            })
            .collect())
    }
}
