-- ============================================================================
-- fresh_context.sql
--
-- Long-running fresh context layer for news, trends, and gossip-like content.
-- This migration is additive and does not modify existing web_ingestion tables.
--
-- Apply manually after review:
--   mysql --default-character-set=utf8mb4 -u root -p digital_companion < database/sql/fresh_context.sql
-- ============================================================================

USE digital_companion;
SET NAMES utf8mb4;

CREATE TABLE IF NOT EXISTS fresh_sources
(
    id                  BIGINT UNSIGNED PRIMARY KEY AUTO_INCREMENT,
    name                VARCHAR(128) NOT NULL,
    source_kind         VARCHAR(32)  NOT NULL COMMENT 'news/rss/trend/gossip/forum/social/search',
    base_url            TEXT NULL,
    allowed_domains     JSON NULL,
    trust_level         VARCHAR(32)  NOT NULL DEFAULT 'normal' COMMENT 'official/trusted/normal/untrusted',
    reliability_score   DOUBLE       NOT NULL DEFAULT 0.5,
    crawl_interval_secs INT UNSIGNED NOT NULL DEFAULT 1800,
    default_ttl_secs    INT UNSIGNED NOT NULL DEFAULT 86400,
    risk_policy         VARCHAR(32)  NOT NULL DEFAULT 'normal' COMMENT 'normal/strict/manual_review',
    enabled             TINYINT(1)   NOT NULL DEFAULT 0,
    metadata            JSON NULL,
    created_at          DATETIME(6)  NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    updated_at          DATETIME(6)  NOT NULL DEFAULT CURRENT_TIMESTAMP(6) ON UPDATE CURRENT_TIMESTAMP(6),
    deleted_at          DATETIME(6)  NULL,
    UNIQUE KEY uk_fresh_sources_name (name),
    KEY idx_fresh_sources_kind_enabled (source_kind, enabled),
    KEY idx_fresh_sources_enabled (enabled),
    KEY idx_fresh_sources_deleted (deleted_at)
) ENGINE = InnoDB
  DEFAULT CHARSET = utf8mb4
  COLLATE = utf8mb4_unicode_ci COMMENT = 'Fresh context source configuration';

CREATE TABLE IF NOT EXISTS fresh_items
(
    id                BIGINT UNSIGNED PRIMARY KEY AUTO_INCREMENT,
    source_id         BIGINT UNSIGNED NOT NULL,
    url               TEXT NULL,
    canonical_url     TEXT NULL,
    url_hash          CHAR(64) NULL,
    title             VARCHAR(512) NULL,
    raw_text          MEDIUMTEXT NULL,
    clean_text        MEDIUMTEXT NULL,
    summary           MEDIUMTEXT NULL,
    published_at      DATETIME(6) NULL,
    fetched_at        DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    expires_at        DATETIME(6) NOT NULL,
    content_hash      CHAR(64) NOT NULL,
    status            VARCHAR(32) NOT NULL DEFAULT 'fetched' COMMENT 'fetched/distilled/published/expired/rejected',
    reliability_score DOUBLE NOT NULL DEFAULT 0.5,
    freshness_score   DOUBLE NOT NULL DEFAULT 0.5,
    heat_score        DOUBLE NOT NULL DEFAULT 0.0,
    rumor_level       VARCHAR(32) NOT NULL DEFAULT 'reported' COMMENT 'confirmed/reported/rumor/disputed',
    risk_flags        JSON NULL,
    metadata          JSON NULL,
    created_at        DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    updated_at        DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6) ON UPDATE CURRENT_TIMESTAMP(6),
    deleted_at        DATETIME(6) NULL,
    CONSTRAINT fk_fresh_items_source
        FOREIGN KEY (source_id) REFERENCES fresh_sources(id) ON DELETE CASCADE,
    UNIQUE KEY uk_fresh_items_source_content (source_id, content_hash),
    UNIQUE KEY uk_fresh_items_source_url_hash (source_id, url_hash),
    KEY idx_fresh_items_status_expires (status, expires_at),
    KEY idx_fresh_items_source_status (source_id, status),
    KEY idx_fresh_items_published_at (published_at),
    KEY idx_fresh_items_fetched_at (fetched_at),
    KEY idx_fresh_items_rumor (rumor_level),
    KEY idx_fresh_items_deleted (deleted_at)
) ENGINE = InnoDB
  DEFAULT CHARSET = utf8mb4
  COLLATE = utf8mb4_unicode_ci COMMENT = 'Fetched and distilled fresh context items';

CREATE TABLE IF NOT EXISTS fresh_topics
(
    id              BIGINT UNSIGNED PRIMARY KEY AUTO_INCREMENT,
    topic_key       CHAR(64) NOT NULL COMMENT 'Stable cluster key',
    title           VARCHAR(512) NOT NULL,
    summary         MEDIUMTEXT NULL,
    entities        JSON NULL,
    first_seen_at   DATETIME(6) NOT NULL,
    last_seen_at    DATETIME(6) NOT NULL,
    heat_score      DOUBLE NOT NULL DEFAULT 0.0,
    freshness_score DOUBLE NOT NULL DEFAULT 0.5,
    expires_at      DATETIME(6) NOT NULL,
    status          VARCHAR(32) NOT NULL DEFAULT 'published' COMMENT 'published/expired/rejected',
    risk_flags      JSON NULL,
    metadata        JSON NULL,
    created_at      DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    updated_at      DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6) ON UPDATE CURRENT_TIMESTAMP(6),
    deleted_at      DATETIME(6) NULL,
    UNIQUE KEY uk_fresh_topics_key (topic_key),
    KEY idx_fresh_topics_status_expires (status, expires_at),
    KEY idx_fresh_topics_last_seen (last_seen_at),
    KEY idx_fresh_topics_heat (heat_score),
    KEY idx_fresh_topics_deleted (deleted_at)
) ENGINE = InnoDB
  DEFAULT CHARSET = utf8mb4
  COLLATE = utf8mb4_unicode_ci COMMENT = 'Fresh context topic clusters';

CREATE TABLE IF NOT EXISTS fresh_topic_evidence
(
    id         BIGINT UNSIGNED PRIMARY KEY AUTO_INCREMENT,
    topic_id   BIGINT UNSIGNED NOT NULL,
    item_id    BIGINT UNSIGNED NOT NULL,
    stance     VARCHAR(32) NOT NULL DEFAULT 'supports' COMMENT 'supports/contradicts/background',
    confidence DOUBLE NOT NULL DEFAULT 0.5,
    created_at DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    CONSTRAINT fk_fresh_topic_evidence_topic
        FOREIGN KEY (topic_id) REFERENCES fresh_topics(id) ON DELETE CASCADE,
    CONSTRAINT fk_fresh_topic_evidence_item
        FOREIGN KEY (item_id) REFERENCES fresh_items(id) ON DELETE CASCADE,
    UNIQUE KEY uk_fresh_topic_evidence_pair (topic_id, item_id),
    KEY idx_fresh_topic_evidence_item (item_id),
    KEY idx_fresh_topic_evidence_stance (stance)
) ENGINE = InnoDB
  DEFAULT CHARSET = utf8mb4
  COLLATE = utf8mb4_unicode_ci COMMENT = 'Evidence links for fresh topics';

CREATE TABLE IF NOT EXISTS fresh_chunks
(
    id                  BIGINT UNSIGNED PRIMARY KEY AUTO_INCREMENT,
    item_id             BIGINT UNSIGNED NOT NULL,
    topic_id            BIGINT UNSIGNED NULL,
    chunk_index         INT UNSIGNED NOT NULL,
    content             TEXT NOT NULL,
    content_hash        CHAR(64) NOT NULL,
    token_count         INT UNSIGNED NULL,
    metadata            JSON NULL,
    vector_id           VARCHAR(128) NULL,
    embedding_provider  VARCHAR(64) NULL,
    embedding_model     VARCHAR(128) NULL,
    embedding_dimension INT UNSIGNED NULL,
    active              TINYINT(1) NOT NULL DEFAULT 1,
    indexed_at          DATETIME(6) NULL,
    expires_at          DATETIME(6) NOT NULL,
    created_at          DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    updated_at          DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6) ON UPDATE CURRENT_TIMESTAMP(6),
    CONSTRAINT fk_fresh_chunks_item
        FOREIGN KEY (item_id) REFERENCES fresh_items(id) ON DELETE CASCADE,
    CONSTRAINT fk_fresh_chunks_topic
        FOREIGN KEY (topic_id) REFERENCES fresh_topics(id) ON DELETE SET NULL,
    UNIQUE KEY uk_fresh_chunks_item_idx (item_id, chunk_index),
    UNIQUE KEY uk_fresh_chunks_vector_id (vector_id),
    KEY idx_fresh_chunks_active_expires (active, expires_at),
    KEY idx_fresh_chunks_topic (topic_id),
    KEY idx_fresh_chunks_indexable (active, vector_id, expires_at)
) ENGINE = InnoDB
  DEFAULT CHARSET = utf8mb4
  COLLATE = utf8mb4_unicode_ci COMMENT = 'Fresh context chunks for vector indexing';
