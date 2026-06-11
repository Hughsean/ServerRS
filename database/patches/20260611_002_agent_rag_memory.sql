-- 20260611_002_agent_rag_memory.sql
-- Purpose: Agent runtime, RAG retrieval, long-term memory, conversation summaries, agent event tracing.
-- Execution policy: Manual patch. NOT auto-executed by Rust service.

CREATE TABLE IF NOT EXISTS `knowledge_documents` (
  `document_id` BIGINT UNSIGNED NOT NULL AUTO_INCREMENT,
  `source_type` VARCHAR(64) NOT NULL,
  `source_id` BIGINT UNSIGNED NULL,
  `title` VARCHAR(255) NULL,
  `content_hash` CHAR(64) NOT NULL,
  `metadata` JSON NULL,
  `status` TINYINT NOT NULL DEFAULT 1,
  `created_at` DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
  `updated_at` DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6) ON UPDATE CURRENT_TIMESTAMP(6),
  PRIMARY KEY (`document_id`),
  UNIQUE KEY `uk_knowledge_documents_source` (`source_type`, `source_id`),
  KEY `idx_knowledge_documents_status` (`status`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;

CREATE TABLE IF NOT EXISTS `knowledge_chunks` (
  `chunk_id` BIGINT UNSIGNED NOT NULL AUTO_INCREMENT,
  `document_id` BIGINT UNSIGNED NOT NULL,
  `chunk_index` INT UNSIGNED NOT NULL,
  `content` TEXT NOT NULL,
  `token_count` INT UNSIGNED NULL,
  `metadata` JSON NULL,
  `created_at` DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
  PRIMARY KEY (`chunk_id`),
  UNIQUE KEY `uk_knowledge_chunks_doc_idx` (`document_id`, `chunk_index`),
  FULLTEXT KEY `ft_knowledge_chunks_content` (`content`),
  CONSTRAINT `fk_knowledge_chunks_document`
    FOREIGN KEY (`document_id`) REFERENCES `knowledge_documents` (`document_id`)
    ON DELETE CASCADE
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;

CREATE TABLE IF NOT EXISTS `knowledge_embeddings` (
  `embedding_id` BIGINT UNSIGNED NOT NULL AUTO_INCREMENT,
  `chunk_id` BIGINT UNSIGNED NOT NULL,
  `provider` VARCHAR(64) NOT NULL,
  `model` VARCHAR(128) NOT NULL,
  `dimension` INT UNSIGNED NOT NULL,
  `embedding_json` JSON NOT NULL,
  `created_at` DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
  PRIMARY KEY (`embedding_id`),
  UNIQUE KEY `uk_knowledge_embeddings_chunk_model` (`chunk_id`, `provider`, `model`),
  CONSTRAINT `fk_knowledge_embeddings_chunk`
    FOREIGN KEY (`chunk_id`) REFERENCES `knowledge_chunks` (`chunk_id`)
    ON DELETE CASCADE
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;

CREATE TABLE IF NOT EXISTS `user_memories` (
  `memory_id` BIGINT UNSIGNED NOT NULL AUTO_INCREMENT,
  `user_id` BIGINT UNSIGNED NOT NULL,
  `memory_type` VARCHAR(64) NOT NULL,
  `content` TEXT NOT NULL,
  `confidence` DOUBLE NOT NULL DEFAULT 0.7,
  `source_conversation_id` BIGINT UNSIGNED NULL,
  `source_message_id` BIGINT UNSIGNED NULL,
  `status` TINYINT NOT NULL DEFAULT 1,
  `metadata` JSON NULL,
  `created_at` DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
  `updated_at` DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6) ON UPDATE CURRENT_TIMESTAMP(6),
  PRIMARY KEY (`memory_id`),
  KEY `idx_user_memories_user_status` (`user_id`, `status`),
  FULLTEXT KEY `ft_user_memories_content` (`content`),
  CONSTRAINT `fk_user_memories_user`
    FOREIGN KEY (`user_id`) REFERENCES `users` (`id`) ON DELETE CASCADE,
  CONSTRAINT `fk_user_memories_conversation`
    FOREIGN KEY (`source_conversation_id`) REFERENCES `conversations` (`id`) ON DELETE SET NULL,
  CONSTRAINT `fk_user_memories_message`
    FOREIGN KEY (`source_message_id`) REFERENCES `conversation_messages` (`id`) ON DELETE SET NULL
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;

CREATE TABLE IF NOT EXISTS `user_memory_embeddings` (
  `embedding_id` BIGINT UNSIGNED NOT NULL AUTO_INCREMENT,
  `memory_id` BIGINT UNSIGNED NOT NULL,
  `provider` VARCHAR(64) NOT NULL,
  `model` VARCHAR(128) NOT NULL,
  `dimension` INT UNSIGNED NOT NULL,
  `embedding_json` JSON NOT NULL,
  `created_at` DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
  PRIMARY KEY (`embedding_id`),
  UNIQUE KEY `uk_user_memory_embeddings_memory_model` (`memory_id`, `provider`, `model`),
  CONSTRAINT `fk_user_memory_embeddings_memory`
    FOREIGN KEY (`memory_id`) REFERENCES `user_memories` (`memory_id`) ON DELETE CASCADE
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;

CREATE TABLE IF NOT EXISTS `conversation_summaries` (
  `summary_id` BIGINT UNSIGNED NOT NULL AUTO_INCREMENT,
  `conversation_id` BIGINT UNSIGNED NOT NULL,
  `user_id` BIGINT UNSIGNED NOT NULL,
  `summary_type` VARCHAR(64) NOT NULL DEFAULT 'rolling',
  `content` TEXT NOT NULL,
  `message_start_id` BIGINT UNSIGNED NULL,
  `message_end_id` BIGINT UNSIGNED NULL,
  `token_count` INT UNSIGNED NULL,
  `created_at` DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
  `updated_at` DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6) ON UPDATE CURRENT_TIMESTAMP(6),
  PRIMARY KEY (`summary_id`),
  KEY `idx_conversation_summaries_conversation` (`conversation_id`),
  KEY `idx_conversation_summaries_user` (`user_id`),
  CONSTRAINT `fk_conversation_summaries_conversation`
    FOREIGN KEY (`conversation_id`) REFERENCES `conversations` (`id`) ON DELETE CASCADE,
  CONSTRAINT `fk_conversation_summaries_user`
    FOREIGN KEY (`user_id`) REFERENCES `users` (`id`) ON DELETE CASCADE
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;

CREATE TABLE IF NOT EXISTS `agent_events` (
  `event_id` BIGINT UNSIGNED NOT NULL AUTO_INCREMENT,
  `user_id` BIGINT UNSIGNED NOT NULL,
  `conversation_id` BIGINT UNSIGNED NULL,
  `session_id` VARCHAR(64) NULL,
  `event_type` VARCHAR(64) NOT NULL,
  `payload` JSON NOT NULL,
  `created_at` DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
  PRIMARY KEY (`event_id`),
  KEY `idx_agent_events_user_time` (`user_id`, `created_at`),
  KEY `idx_agent_events_conversation` (`conversation_id`),
  CONSTRAINT `fk_agent_events_user`
    FOREIGN KEY (`user_id`) REFERENCES `users` (`id`) ON DELETE CASCADE,
  CONSTRAINT `fk_agent_events_conversation`
    FOREIGN KEY (`conversation_id`) REFERENCES `conversations` (`id`) ON DELETE SET NULL
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;
