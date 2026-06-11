use std::sync::Arc;

use crate::domain::llm::EmbeddingProvider;
use crate::domain::rag::{NewChunk, NewDocument, NewEmbedding, RAGRepository};
use crate::shared::error::AppError;

use super::chunking::ChunkingService;

/// Ingests content into the knowledge base: chunks the text, persists
/// the document + chunks, and optionally generates embeddings.
use super::vector_index_service::VectorIndexService;

pub struct IngestionService {
    repo: Arc<dyn RAGRepository>,
    chunking: ChunkingService,
    embedding: Option<Arc<dyn EmbeddingProvider>>,
    vector_index: Option<Arc<VectorIndexService>>,
}

impl IngestionService {
    pub fn new(
        repo: Arc<dyn RAGRepository>,
        chunking: ChunkingService,
        embedding: Option<Arc<dyn EmbeddingProvider>>,
    ) -> Self {
        Self {
            repo,
            chunking,
            embedding,
            vector_index: None,
        }
    }

    pub fn with_vector_index(mut self, vi: Arc<VectorIndexService>) -> Self {
        self.vector_index = Some(vi);
        self
    }

    /// Ingest a new document.
    ///
    /// Returns the assigned `document_id` on success.
    pub async fn ingest(
        &self,
        source_type: &str,
        source_id: Option<u64>,
        title: Option<String>,
        content: &str,
        metadata: Option<serde_json::Value>,
    ) -> Result<u64, AppError> {
        // 1. compute content hash for dedup / later reference
        let content_hash = Self::compute_hash(content);

        // 2. save the document
        let doc = self
            .repo
            .save_document(NewDocument {
                source_type: source_type.to_string(),
                source_id,
                title,
                content_hash,
                metadata,
                status: 1, // active
            })
            .await?;

        // 3. chunk the content
        let chunks_raw = self.chunking.chunk_text(content, 512, 64);

        // 4. persist chunks
        let new_chunks: Vec<NewChunk> = chunks_raw
            .iter()
            .enumerate()
            .map(|(i, text)| NewChunk {
                document_id: doc.document_id,
                chunk_index: i as u32,
                content: text.clone(),
                token_count: None,
                metadata: None,
            })
            .collect();

        let saved_chunks = self.repo.save_chunks(&new_chunks).await?;

        // 5. optionally embed (legacy path — kept for compatibility)
        if let Some(ref emb_provider) = self.embedding {
            let texts: Vec<String> = saved_chunks.iter().map(|c| c.content.clone()).collect();
            match emb_provider.embed(&texts).await {
                Ok(embeddings) => {
                    for (chunk, vec) in saved_chunks.iter().zip(embeddings.iter()) {
                        let emb_val: serde_json::Value =
                            serde_json::to_value(vec).unwrap_or(serde_json::Value::Null);
                        let _ = self
                            .repo
                            .save_embedding(NewEmbedding {
                                chunk_id: chunk.chunk_id,
                                provider: "llm_provider".into(),
                                model: "default".into(),
                                dimension: vec.len() as u32,
                                embedding_json: emb_val,
                            })
                            .await;
                    }
                }
                Err(e) => {
                    tracing::warn!(?e, "embedding generation failed during ingest");
                }
            }
        }

        // 6. index chunks via VectorIndexService (Qdrant + records + metadata)
        if let Some(ref vi) = self.vector_index {
            for chunk in &saved_chunks {
                if let Err(e) = vi.index_knowledge_chunk(chunk, Some(&doc)).await {
                    tracing::warn!(
                        chunk_id = chunk.chunk_id,
                        document_id = doc.document_id,
                        error = %e,
                        "failed to index knowledge chunk after ingest"
                    );
                }
            }
        }

        Ok(doc.document_id)
    }

    fn compute_hash(content: &str) -> String {
        use sha2::{Digest, Sha256};
        Sha256::digest(content.as_bytes())
            .iter()
            .map(|b| format!("{:02x}", b))
            .collect()
    }
}
