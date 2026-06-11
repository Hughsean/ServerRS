-- ============================================================================
-- Patch: 20260611_004_agent_vector_lifecycle.sql
--
-- Adds vector index management tables, optimizes existing tables for the
-- vector index lifecycle (visibility, salience, memory dedup, etc.),
-- and extends agent_events for tracing.
--
-- Prerequisites:
--   20260611_001_auth_role_refresh_likes.sql
--   20260611_002_agent_rag_memory.sql
--   20260611_003_qdrant_vector_index.sql (MUST be applied first)
-- ============================================================================

-- ── vector_index_records ────────────────────────────────────────────────────

CREATE TABLE IF NOT EXISTS `vector_index_records` (
  `record_id` BIGINT UNSIGNED NOT NULL AUTO_INCREMENT,
  `vector_id` VARCHAR(128) NOT NULL,
  `collection_name` VARCHAR(128) NOT NULL,
  `object_type` VARCHAR(64) NOT NULL,
  `object_id` BIGINT UNSIGNED NOT NULL,
  `owner_user_id` BIGINT UNSIGNED NULL,
  `source_table` VARCHAR(64) NOT NULL,
  `source_hash` CHAR(64) NULL,
  `embedding_provider` VARCHAR(64) NOT NULL,
  `embedding_model` VARCHAR(128) NOT NULL,
  `embedding_dimension` INT UNSIGNED NOT NULL,
  `payload` JSON NOT NULL,
  `index_status` VARCHAR(32) NOT NULL DEFAULT 'indexed',
  `indexed_at` DATETIME(6) NULL,
  `failed_at` DATETIME(6) NULL,
  `error_message` TEXT NULL,
  `created_at` DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
  `updated_at` DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6) ON UPDATE CURRENT_TIMESTAMP(6),
  PRIMARY KEY (`record_id`),
  UNIQUE KEY `uk_vector_index_records_vector_id` (`vector_id`),
  KEY `idx_vector_index_records_object` (`object_type`, `object_id`),
  KEY `idx_vector_index_records_collection_status` (`collection_name`, `index_status`),
  KEY `idx_vector_index_records_owner_type` (`owner_user_id`, `object_type`),
  CONSTRAINT `fk_vector_index_records_owner`
    FOREIGN KEY (`owner_user_id`) REFERENCES `users` (`id`) ON DELETE SET NULL
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;

-- ── vector_index_jobs ───────────────────────────────────────────────────────

CREATE TABLE IF NOT EXISTS `vector_index_jobs` (
  `job_id` BIGINT UNSIGNED NOT NULL AUTO_INCREMENT,
  `action` VARCHAR(32) NOT NULL,
  `object_type` VARCHAR(64) NOT NULL,
  `object_id` BIGINT UNSIGNED NOT NULL,
  `collection_name` VARCHAR(128) NOT NULL,
  `vector_id` VARCHAR(128) NULL,
  `priority` INT NOT NULL DEFAULT 100,
  `status` VARCHAR(32) NOT NULL DEFAULT 'pending',
  `attempts` INT UNSIGNED NOT NULL DEFAULT 0,
  `max_attempts` INT UNSIGNED NOT NULL DEFAULT 5,
  `next_run_at` DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
  `locked_at` DATETIME(6) NULL,
  `locked_by` VARCHAR(128) NULL,
  `last_error` TEXT NULL,
  `created_at` DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
  `updated_at` DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6) ON UPDATE CURRENT_TIMESTAMP(6),
  PRIMARY KEY (`job_id`),
  KEY `idx_vector_index_jobs_status_next` (`status`, `next_run_at`, `priority`),
  KEY `idx_vector_index_jobs_object` (`object_type`, `object_id`),
  KEY `idx_vector_index_jobs_vector_id` (`vector_id`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;

-- ── knowledge_documents ─────────────────────────────────────────────────────

ALTER TABLE `knowledge_documents`
  ADD COLUMN `owner_user_id` BIGINT UNSIGNED NULL AFTER `source_id`,
  ADD COLUMN `visibility` VARCHAR(32) NOT NULL DEFAULT 'public' AFTER `owner_user_id`,
  ADD COLUMN `source_version` VARCHAR(128) NULL AFTER `content_hash`,
  ADD COLUMN `source_updated_at` DATETIME(6) NULL AFTER `source_version`,
  ADD COLUMN `deleted_at` DATETIME(6) NULL AFTER `updated_at`,
  ADD KEY `idx_knowledge_documents_owner_status` (`owner_user_id`, `status`),
  ADD KEY `idx_knowledge_documents_visibility_status` (`visibility`, `status`),
  ADD CONSTRAINT `fk_knowledge_documents_owner`
    FOREIGN KEY (`owner_user_id`) REFERENCES `users` (`id`) ON DELETE SET NULL;

-- ── knowledge_chunks ────────────────────────────────────────────────────────

ALTER TABLE `knowledge_chunks`
  ADD COLUMN `status` TINYINT NOT NULL DEFAULT 1 AFTER `metadata`,
  ADD COLUMN `content_hash` CHAR(64) NULL AFTER `content`,
  ADD COLUMN `char_start` INT UNSIGNED NULL AFTER `chunk_index`,
  ADD COLUMN `char_end` INT UNSIGNED NULL AFTER `char_start`,
  ADD KEY `idx_knowledge_chunks_document_status` (`document_id`, `status`),
  ADD KEY `idx_knowledge_chunks_vector_id` (`vector_id`);

-- ── user_memories ───────────────────────────────────────────────────────────

ALTER TABLE `user_memories`
  ADD COLUMN `memory_key` CHAR(64) NULL AFTER `memory_type`,
  ADD COLUMN `salience` DOUBLE NOT NULL DEFAULT 0.5 AFTER `confidence`,
  ADD COLUMN `last_accessed_at` DATETIME(6) NULL AFTER `updated_at`,
  ADD COLUMN `access_count` INT UNSIGNED NOT NULL DEFAULT 0 AFTER `last_accessed_at`,
  ADD COLUMN `expires_at` DATETIME(6) NULL AFTER `access_count`,
  ADD KEY `idx_user_memories_user_key` (`user_id`, `memory_key`),
  ADD KEY `idx_user_memories_user_salience` (`user_id`, `status`, `salience`),
  ADD KEY `idx_user_memories_expires_at` (`expires_at`),
  ADD KEY `idx_user_memories_vector_id` (`vector_id`);

-- ── conversation_summaries ──────────────────────────────────────────────────

ALTER TABLE `conversation_summaries`
  ADD COLUMN `status` TINYINT NOT NULL DEFAULT 1 AFTER `token_count`,
  ADD COLUMN `summary_version` INT UNSIGNED NOT NULL DEFAULT 1 AFTER `status`,
  ADD COLUMN `source_message_count` INT UNSIGNED NULL AFTER `summary_version`,
  ADD KEY `idx_conversation_summaries_conv_status` (`conversation_id`, `status`, `updated_at`),
  ADD KEY `idx_conversation_summaries_vector_id` (`vector_id`);

-- ── agent_events ────────────────────────────────────────────────────────────

ALTER TABLE `agent_events`
  ADD COLUMN `trace_id` VARCHAR(64) NULL AFTER `session_id`,
  ADD COLUMN `turn_id` VARCHAR(64) NULL AFTER `trace_id`,
  ADD COLUMN `severity` VARCHAR(32) NOT NULL DEFAULT 'info' AFTER `event_type`,
  ADD COLUMN `tool_name` VARCHAR(128) NULL AFTER `severity`,
  ADD KEY `idx_agent_events_trace` (`trace_id`),
  ADD KEY `idx_agent_events_turn` (`turn_id`),
  ADD KEY `idx_agent_events_type_time` (`event_type`, `created_at`);
