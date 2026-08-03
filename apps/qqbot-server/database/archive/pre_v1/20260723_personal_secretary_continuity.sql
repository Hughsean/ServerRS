-- QQ Personal Secretary connection continuity foundation.
-- Depends on 20260723_personal_secretary_ingestion.sql.

CREATE TABLE IF NOT EXISTS secretary_connection_epochs
(
    connection_epoch_id CHAR(36) CHARACTER SET ascii COLLATE ascii_bin PRIMARY KEY,
    account_id           BIGINT UNSIGNED NOT NULL,
    source_channel       VARCHAR(32)     NOT NULL,
    status               VARCHAR(24)     NOT NULL DEFAULT 'connecting',
    started_at           DATETIME(6)     NOT NULL,
    connected_at         DATETIME(6)     NULL,
    ended_at             DATETIME(6)     NULL,
    last_event_at        DATETIME(6)     NULL,
    last_source_event_id CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NULL,
    end_reason           VARCHAR(32)     NULL,
    created_at           DATETIME(6)     NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    updated_at           DATETIME(6)     NOT NULL DEFAULT CURRENT_TIMESTAMP(6)
        ON UPDATE CURRENT_TIMESTAMP(6),

    CONSTRAINT chk_secretary_connection_status
        CHECK (status IN ('connecting', 'connected', 'disconnected', 'shutdown', 'connect_failed')),
    CONSTRAINT chk_secretary_connection_source
        CHECK (source_channel IN ('napcat', 'qq_open_platform')),
    CONSTRAINT fk_secretary_connection_account
        FOREIGN KEY (account_id) REFERENCES secretary_accounts(id) ON DELETE CASCADE,
    INDEX idx_secretary_connection_account_time (account_id, started_at),
    INDEX idx_secretary_connection_status (status, updated_at)
) ENGINE = InnoDB
  DEFAULT CHARSET = utf8mb4
  COLLATE = utf8mb4_unicode_ci
  COMMENT = '个人秘书传输连接周期、断连原因和最后成功事件';

CREATE TABLE IF NOT EXISTS secretary_event_ingestion
(
    source_event_id      CHAR(36) CHARACTER SET ascii COLLATE ascii_bin PRIMARY KEY,
    connection_epoch_id CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    observed_at          DATETIME(6) NOT NULL,

    CONSTRAINT fk_secretary_event_ingestion_event
        FOREIGN KEY (source_event_id) REFERENCES secretary_source_events(source_event_id)
        ON DELETE CASCADE,
    CONSTRAINT fk_secretary_event_ingestion_connection
        FOREIGN KEY (connection_epoch_id)
        REFERENCES secretary_connection_epochs(connection_epoch_id) ON DELETE CASCADE,
    INDEX idx_secretary_event_ingestion_epoch (connection_epoch_id, observed_at)
) ENGINE = InnoDB
  DEFAULT CHARSET = utf8mb4
  COLLATE = utf8mb4_unicode_ci
  COMMENT = '实时 SourceEvent 到连接周期的不可变来源关联';

CREATE TABLE IF NOT EXISTS secretary_ingestion_cursors
(
    id                         BIGINT UNSIGNED PRIMARY KEY AUTO_INCREMENT,
    account_id                 BIGINT UNSIGNED NOT NULL,
    conversation_id            BIGINT UNSIGNED NULL,
    scope_kind                 VARCHAR(24)     NOT NULL,
    scope_key                  VARCHAR(255)    COLLATE utf8mb4_bin NOT NULL,
    last_source_event_id       CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    last_platform_event_id     VARCHAR(191)    COLLATE utf8mb4_bin NOT NULL,
    last_occurred_at_unix_secs BIGINT          NOT NULL,
    connection_epoch_id        CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NULL,
    updated_at                 DATETIME(6)      NOT NULL,

    CONSTRAINT chk_secretary_cursor_scope
        CHECK (scope_kind IN ('account', 'conversation')),
    CONSTRAINT fk_secretary_cursor_account
        FOREIGN KEY (account_id) REFERENCES secretary_accounts(id) ON DELETE CASCADE,
    CONSTRAINT fk_secretary_cursor_conversation
        FOREIGN KEY (conversation_id) REFERENCES secretary_conversations(id) ON DELETE CASCADE,
    CONSTRAINT fk_secretary_cursor_event
        FOREIGN KEY (last_source_event_id) REFERENCES secretary_source_events(source_event_id)
        ON DELETE CASCADE,
    CONSTRAINT fk_secretary_cursor_connection
        FOREIGN KEY (connection_epoch_id)
        REFERENCES secretary_connection_epochs(connection_epoch_id) ON DELETE SET NULL,
    UNIQUE KEY uk_secretary_cursor_scope (account_id, scope_kind, scope_key),
    INDEX idx_secretary_cursor_updated (account_id, updated_at)
) ENGINE = InnoDB
  DEFAULT CHARSET = utf8mb4
  COLLATE = utf8mb4_unicode_ci
  COMMENT = '账号及会话级稳定接入游标；为历史回补提供锚点';

CREATE TABLE IF NOT EXISTS secretary_ingestion_gaps
(
    gap_id              CHAR(36) CHARACTER SET ascii COLLATE ascii_bin PRIMARY KEY,
    account_id          BIGINT UNSIGNED NOT NULL,
    connection_epoch_id CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    gap_started_at      DATETIME(6)     NOT NULL,
    gap_ended_at        DATETIME(6)     NULL,
    status              VARCHAR(24)     NOT NULL DEFAULT 'uncertain',
    reason              VARCHAR(32)     NOT NULL,
    created_at          DATETIME(6)     NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    updated_at          DATETIME(6)     NOT NULL DEFAULT CURRENT_TIMESTAMP(6)
        ON UPDATE CURRENT_TIMESTAMP(6),

    CONSTRAINT chk_secretary_gap_status
        CHECK (status IN ('uncertain', 'backfilling', 'verified_complete', 'unrecoverable')),
    CONSTRAINT fk_secretary_gap_account
        FOREIGN KEY (account_id) REFERENCES secretary_accounts(id) ON DELETE CASCADE,
    CONSTRAINT fk_secretary_gap_connection
        FOREIGN KEY (connection_epoch_id)
        REFERENCES secretary_connection_epochs(connection_epoch_id) ON DELETE CASCADE,
    UNIQUE KEY uk_secretary_gap_connection (connection_epoch_id),
    INDEX idx_secretary_gap_status (account_id, status, gap_started_at),
    INDEX idx_secretary_gap_open (account_id, gap_ended_at)
) ENGINE = InnoDB
  DEFAULT CHARSET = utf8mb4
  COLLATE = utf8mb4_unicode_ci
  COMMENT = '无法证明消息连续性的时间窗；回补验证前始终保持 uncertain';

-- 回滚顺序（仅在确认不需要连续性审计数据后人工执行）：
-- DROP TABLE secretary_ingestion_gaps;
-- DROP TABLE secretary_ingestion_cursors;
-- DROP TABLE secretary_event_ingestion;
-- DROP TABLE secretary_connection_epochs;
