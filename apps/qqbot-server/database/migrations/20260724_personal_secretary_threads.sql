-- QQ Personal Secretary deterministic event-thread projection.
-- Depends on 20260723_personal_secretary_ingestion.sql and remains isolated from digital-human tables.

CREATE TABLE IF NOT EXISTS secretary_event_threads
(
    thread_id                    CHAR(36) CHARACTER SET ascii COLLATE ascii_bin PRIMARY KEY,
    account_id                   BIGINT UNSIGNED NOT NULL,
    status                       VARCHAR(16) NOT NULL DEFAULT 'open',
    root_event_id                CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    latest_event_id              CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    opened_at_unix_secs          BIGINT NOT NULL,
    latest_occurred_at_unix_secs BIGINT NOT NULL,
    created_at                   DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    updated_at                   DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6)
        ON UPDATE CURRENT_TIMESTAMP(6),

    CONSTRAINT chk_secretary_event_thread_status
        CHECK (status IN ('open', 'waiting', 'resolved', 'closed', 'reopened')),
    CONSTRAINT fk_secretary_event_thread_account
        FOREIGN KEY (account_id) REFERENCES secretary_accounts(id) ON DELETE CASCADE,
    CONSTRAINT fk_secretary_event_thread_root
        FOREIGN KEY (root_event_id) REFERENCES secretary_source_events(source_event_id)
        ON DELETE CASCADE,
    CONSTRAINT fk_secretary_event_thread_latest
        FOREIGN KEY (latest_event_id) REFERENCES secretary_source_events(source_event_id)
        ON DELETE CASCADE,
    INDEX idx_secretary_event_thread_account_status (account_id, status, updated_at)
) ENGINE = InnoDB
  DEFAULT CHARSET = utf8mb4
  COLLATE = utf8mb4_unicode_ci
  COMMENT = '个人秘书确定性事件线程；生命周期与消息正文分离';

CREATE TABLE IF NOT EXISTS secretary_thread_events
(
    source_event_id CHAR(36) CHARACTER SET ascii COLLATE ascii_bin PRIMARY KEY,
    thread_id       CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    added_at        DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),

    CONSTRAINT fk_secretary_thread_event_source
        FOREIGN KEY (source_event_id) REFERENCES secretary_source_events(source_event_id)
        ON DELETE CASCADE,
    CONSTRAINT fk_secretary_thread_event_thread
        FOREIGN KEY (thread_id) REFERENCES secretary_event_threads(thread_id)
        ON DELETE CASCADE,
    INDEX idx_secretary_thread_event_thread (thread_id, added_at, source_event_id)
) ENGINE = InnoDB
  DEFAULT CHARSET = utf8mb4
  COLLATE = utf8mb4_unicode_ci
  COMMENT = 'SourceEvent 到 EventThread 的可审计成员投影；每个事件至多属于一个线程';

CREATE TABLE IF NOT EXISTS secretary_thread_relations
(
    relation_id     CHAR(36) CHARACTER SET ascii COLLATE ascii_bin PRIMARY KEY,
    thread_id       CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    from_event_id   CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    to_event_id     CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    relation_kind   VARCHAR(64) NOT NULL,
    confidence_bps  SMALLINT UNSIGNED NOT NULL,
    reason          VARCHAR(255) NOT NULL,
    created_at      DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),

    CONSTRAINT chk_secretary_thread_relation_kind
        CHECK (relation_kind IN
            ('reply', 'same_conversation_window', 'same_actor_within_conversation_window',
             'explicit_project_id', 'file_version')),
    CONSTRAINT chk_secretary_thread_relation_confidence
        CHECK (confidence_bps <= 10000),
    CONSTRAINT fk_secretary_thread_relation_thread
        FOREIGN KEY (thread_id) REFERENCES secretary_event_threads(thread_id) ON DELETE CASCADE,
    CONSTRAINT fk_secretary_thread_relation_from
        FOREIGN KEY (from_event_id) REFERENCES secretary_source_events(source_event_id)
        ON DELETE CASCADE,
    CONSTRAINT fk_secretary_thread_relation_to
        FOREIGN KEY (to_event_id) REFERENCES secretary_source_events(source_event_id)
        ON DELETE CASCADE,
    UNIQUE KEY uk_secretary_thread_relation
        (thread_id, from_event_id, to_event_id, relation_kind),
    INDEX idx_secretary_thread_relation_to (to_event_id, relation_kind)
) ENGINE = InnoDB
  DEFAULT CHARSET = utf8mb4
  COLLATE = utf8mb4_unicode_ci
  COMMENT = '线程确定性来源边；reason 只保存证据类型，不保存消息正文';

CREATE TABLE IF NOT EXISTS secretary_thread_projection_claims
(
    source_event_id CHAR(36) CHARACTER SET ascii COLLATE ascii_bin PRIMARY KEY,
    lease_token     CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NULL,
    lease_expires_at DATETIME(6) NULL,
    attempts        INT UNSIGNED NOT NULL DEFAULT 0,
    last_error      VARCHAR(512) NULL,
    updated_at      DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6)
        ON UPDATE CURRENT_TIMESTAMP(6),

    CONSTRAINT fk_secretary_thread_projection_source
        FOREIGN KEY (source_event_id) REFERENCES secretary_source_events(source_event_id)
        ON DELETE CASCADE,
    INDEX idx_secretary_thread_projection_claim
        (lease_expires_at, updated_at, source_event_id),
    INDEX idx_secretary_thread_projection_token (lease_token)
) ENGINE = InnoDB
  DEFAULT CHARSET = utf8mb4
  COLLATE = utf8mb4_unicode_ci
  COMMENT = '独立线程投影消费者租约；不复用 source_events.processing_status';

-- 回滚顺序（仅在确认不再需要线程投影后人工执行）：
-- DROP TABLE secretary_thread_projection_claims;
-- DROP TABLE secretary_thread_relations;
-- DROP TABLE secretary_thread_events;
-- DROP TABLE secretary_event_threads;
