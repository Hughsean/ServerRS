-- ============================================================================
-- Patch: 20260611_003_qdrant_vector_index.sql
--
-- Adds vector_id and indexing metadata columns to support the Qdrant
-- VectorStore integration.  These columns track which entries have been
-- indexed in Qdrant and with what embedding provider/model.
--
-- IMPORTANT:
--   - This file is for manual execution only — the Rust service does NOT
--     auto-apply patches or run migrations.
--   - After executing this patch against the real database, re-run
--     sea-orm-cli generate entity to regenerate entity types.
-- ============================================================================

-- ── knowledge_chunks ────────────────────────────────────────────────────────

ALTER TABLE knowledge_chunks
  ADD COLUMN vector_id VARCHAR(128) NULL,
  ADD COLUMN embedding_provider VARCHAR(64) NULL,
  ADD COLUMN embedding_model VARCHAR(128) NULL,
  ADD COLUMN embedding_dimension INT UNSIGNED NULL,
  ADD COLUMN indexed_at DATETIME(6) NULL,
  ADD UNIQUE KEY uk_knowledge_chunks_vector_id (vector_id);

-- ── user_memories ───────────────────────────────────────────────────────────

ALTER TABLE user_memories
  ADD COLUMN vector_id VARCHAR(128) NULL,
  ADD COLUMN embedding_provider VARCHAR(64) NULL,
  ADD COLUMN embedding_model VARCHAR(128) NULL,
  ADD COLUMN embedding_dimension INT UNSIGNED NULL,
  ADD COLUMN indexed_at DATETIME(6) NULL,
  ADD UNIQUE KEY uk_user_memories_vector_id (vector_id);

-- ── conversation_summaries ──────────────────────────────────────────────────

ALTER TABLE conversation_summaries
  ADD COLUMN vector_id VARCHAR(128) NULL,
  ADD COLUMN embedding_provider VARCHAR(64) NULL,
  ADD COLUMN embedding_model VARCHAR(128) NULL,
  ADD COLUMN embedding_dimension INT UNSIGNED NULL,
  ADD COLUMN indexed_at DATETIME(6) NULL,
  ADD UNIQUE KEY uk_conversation_summaries_vector_id (vector_id);
