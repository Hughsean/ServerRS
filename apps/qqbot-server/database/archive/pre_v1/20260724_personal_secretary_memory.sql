-- QQ Personal Secretary source-backed structured memory facts.
-- Depends on ingestion. Does not store complete chat history inside memory rows.

CREATE TABLE IF NOT EXISTS secretary_memory_facts
(
    fact_id               CHAR(36) CHARACTER SET ascii COLLATE ascii_bin PRIMARY KEY,
    account_id            BIGINT UNSIGNED NOT NULL,
    fact_kind             VARCHAR(24) NOT NULL,
    subject_key           VARCHAR(191) COLLATE utf8mb4_bin NOT NULL,
    fact_json             JSON NOT NULL,
    fact_status           VARCHAR(16) NOT NULL,
    confidence_bps        SMALLINT UNSIGNED NOT NULL,
    valid_until_unix_secs BIGINT NULL,
    supersedes_fact_id    CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NULL,
    created_at            DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    updated_at            DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6)
        ON UPDATE CURRENT_TIMESTAMP(6),

    CONSTRAINT chk_secretary_memory_fact_kind
        CHECK (fact_kind IN ('person', 'project', 'commitment')),
    CONSTRAINT chk_secretary_memory_fact_status
        CHECK (fact_status IN ('proposed', 'confirmed', 'superseded', 'expired', 'deleted')),
    CONSTRAINT chk_secretary_memory_fact_confidence CHECK (confidence_bps <= 10000),
    CONSTRAINT chk_secretary_memory_fact_not_self
        CHECK (supersedes_fact_id IS NULL OR supersedes_fact_id <> fact_id),
    CONSTRAINT fk_secretary_memory_fact_account
        FOREIGN KEY (account_id) REFERENCES secretary_accounts(id) ON DELETE CASCADE,
    CONSTRAINT fk_secretary_memory_fact_supersedes
        FOREIGN KEY (supersedes_fact_id) REFERENCES secretary_memory_facts(fact_id)
        ON DELETE RESTRICT,
    UNIQUE KEY uk_secretary_memory_fact_supersedes (supersedes_fact_id),
    INDEX idx_secretary_memory_fact_active
        (account_id, fact_kind, fact_status, valid_until_unix_secs, updated_at),
    INDEX idx_secretary_memory_fact_subject
        (account_id, subject_key, fact_status, updated_at)
) ENGINE = InnoDB
  DEFAULT CHARSET = utf8mb4
  COLLATE = utf8mb4_unicode_ci
  COMMENT = '人物、项目、承诺的类型化版本事实；摘要只导航，原始事件负责证据';

CREATE TABLE IF NOT EXISTS secretary_memory_fact_sources
(
    fact_id         CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    source_event_id CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    PRIMARY KEY (fact_id, source_event_id),
    CONSTRAINT fk_secretary_memory_source_fact
        FOREIGN KEY (fact_id) REFERENCES secretary_memory_facts(fact_id) ON DELETE CASCADE,
    CONSTRAINT fk_secretary_memory_source_event
        FOREIGN KEY (source_event_id) REFERENCES secretary_source_events(source_event_id)
        ON DELETE RESTRICT,
    INDEX idx_secretary_memory_source_event (source_event_id, fact_id)
) ENGINE = InnoDB
  DEFAULT CHARSET = utf8mb4
  COLLATE = utf8mb4_unicode_ci
  COMMENT = '长期记忆到无损 SourceEvent 的可回读来源引用';

-- 回滚顺序：memory_fact_sources -> memory_facts。
