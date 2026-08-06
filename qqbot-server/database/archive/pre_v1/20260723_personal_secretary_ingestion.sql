-- QQ Personal Secretary reliable inbound message foundation.
-- Forward-only migration: only creates new secretary_* tables and does not touch legacy qq_* data.

CREATE TABLE IF NOT EXISTS secretary_accounts
(
    id                  BIGINT UNSIGNED PRIMARY KEY AUTO_INCREMENT,
    source_channel      VARCHAR(32)     NOT NULL,
    platform_account_id VARCHAR(191)    COLLATE utf8mb4_bin NOT NULL,
    status              VARCHAR(16)     NOT NULL DEFAULT 'active',
    created_at          DATETIME(6)     NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    updated_at          DATETIME(6)     NOT NULL DEFAULT CURRENT_TIMESTAMP(6)
        ON UPDATE CURRENT_TIMESTAMP(6),

    CONSTRAINT chk_secretary_accounts_source
        CHECK (source_channel IN ('napcat', 'qq_open_platform')),
    CONSTRAINT chk_secretary_accounts_status
        CHECK (status IN ('active', 'disabled')),
    UNIQUE KEY uk_secretary_account_source (source_channel, platform_account_id)
) ENGINE = InnoDB
  DEFAULT CHARSET = utf8mb4
  COLLATE = utf8mb4_unicode_ci
  COMMENT = '个人秘书接入账号主体；不同 NapCat 账号或官方 Bot 严格隔离';

CREATE TABLE IF NOT EXISTS secretary_conversations
(
    id                       BIGINT UNSIGNED PRIMARY KEY AUTO_INCREMENT,
    account_id               BIGINT UNSIGNED NOT NULL,
    conversation_kind        VARCHAR(32)     NOT NULL,
    platform_conversation_id VARCHAR(191)    COLLATE utf8mb4_bin NOT NULL,
    memory_mode              VARCHAR(32)     NOT NULL DEFAULT 'normal',
    created_at               DATETIME(6)     NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    updated_at               DATETIME(6)     NOT NULL DEFAULT CURRENT_TIMESTAMP(6)
        ON UPDATE CURRENT_TIMESTAMP(6),

    CONSTRAINT chk_secretary_conversation_kind
        CHECK (conversation_kind IN ('private', 'group', 'owner_control')),
    CONSTRAINT chk_secretary_conversation_memory
        CHECK (memory_mode IN ('normal', 'local_only', 'envelope_only', 'never_long_term')),
    CONSTRAINT fk_secretary_conversations_account
        FOREIGN KEY (account_id) REFERENCES secretary_accounts(id) ON DELETE CASCADE,
    UNIQUE KEY uk_secretary_conversation
        (account_id, conversation_kind, platform_conversation_id),
    INDEX idx_secretary_conversation_memory (account_id, memory_mode)
) ENGINE = InnoDB
  DEFAULT CHARSET = utf8mb4
  COLLATE = utf8mb4_unicode_ci
  COMMENT = '个人秘书协议无关会话及数据保留策略';

CREATE TABLE IF NOT EXISTS secretary_source_events
(
    source_event_id             CHAR(36) CHARACTER SET ascii COLLATE ascii_bin PRIMARY KEY,
    account_id                  BIGINT UNSIGNED NOT NULL,
    conversation_id             BIGINT UNSIGNED NOT NULL,
    source_channel              VARCHAR(32)     NOT NULL,
    platform_event_id           VARCHAR(191)    COLLATE utf8mb4_bin NOT NULL,
    event_type                  VARCHAR(32)     NOT NULL,
    actor_platform_id           VARCHAR(191)    COLLATE utf8mb4_bin NOT NULL,
    actor_kind                  VARCHAR(32)     NOT NULL,
    message_role                VARCHAR(32)     NOT NULL,
    occurred_at_unix_secs       BIGINT          NOT NULL,
    reply_to_platform_event_id  VARCHAR(191)    COLLATE utf8mb4_bin NULL,
    reply_to_event_id           CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NULL,
    processing_status           VARCHAR(16)     NOT NULL DEFAULT 'pending',
    received_at                 DATETIME(6)     NOT NULL,
    created_at                  DATETIME(6)     NOT NULL DEFAULT CURRENT_TIMESTAMP(6),

    CONSTRAINT chk_secretary_source_event_type
        CHECK (event_type IN ('message')),
    CONSTRAINT chk_secretary_source_actor_kind
        CHECK (actor_kind IN ('owner', 'official_bot', 'external')),
    CONSTRAINT chk_secretary_source_message_role
        CHECK (message_role IN
            ('owner_command', 'owner_observation', 'external_observation', 'assistant_output')),
    CONSTRAINT chk_secretary_source_processing
        CHECK (processing_status IN ('pending', 'processing', 'processed', 'failed', 'ignored')),
    CONSTRAINT fk_secretary_source_account
        FOREIGN KEY (account_id) REFERENCES secretary_accounts(id) ON DELETE CASCADE,
    CONSTRAINT fk_secretary_source_conversation
        FOREIGN KEY (conversation_id) REFERENCES secretary_conversations(id) ON DELETE CASCADE,
    CONSTRAINT fk_secretary_source_reply
        FOREIGN KEY (reply_to_event_id) REFERENCES secretary_source_events(source_event_id)
        ON DELETE SET NULL,
    UNIQUE KEY uk_secretary_source_delivery (account_id, platform_event_id),
    INDEX idx_secretary_source_conversation_time
        (conversation_id, occurred_at_unix_secs, source_event_id),
    INDEX idx_secretary_source_actor_time
        (account_id, actor_platform_id, occurred_at_unix_secs),
    INDEX idx_secretary_source_processing (processing_status, received_at),
    INDEX idx_secretary_source_reply_platform
        (account_id, reply_to_platform_event_id)
) ENGINE = InnoDB
  DEFAULT CHARSET = utf8mb4
  COLLATE = utf8mb4_unicode_ci
  COMMENT = '个人秘书不可变入站事件信封和确定性回复关系';

CREATE TABLE IF NOT EXISTS secretary_message_contents
(
    source_event_id       CHAR(36) CHARACTER SET ascii COLLATE ascii_bin PRIMARY KEY,
    normalized_text       MEDIUMTEXT      NOT NULL,
    segments              JSON            NOT NULL,
    mentioned_actor_ids   JSON            NOT NULL,
    mention_all           TINYINT(1)      NOT NULL DEFAULT 0,
    content_mode          VARCHAR(32)     NOT NULL DEFAULT 'normal',
    created_at            DATETIME(6)     NOT NULL DEFAULT CURRENT_TIMESTAMP(6),

    CONSTRAINT chk_secretary_content_mode
        CHECK (content_mode IN ('normal', 'local_only', 'envelope_only')),
    CONSTRAINT fk_secretary_message_event
        FOREIGN KEY (source_event_id) REFERENCES secretary_source_events(source_event_id)
        ON DELETE CASCADE
) ENGINE = InnoDB
  DEFAULT CHARSET = utf8mb4
  COLLATE = utf8mb4_unicode_ci
  COMMENT = '个人秘书消息正文、结构化消息段、@目标和内容策略';

-- 回滚顺序（仅在确认不需要新业务数据后人工执行）：
-- DROP TABLE secretary_message_contents;
-- DROP TABLE secretary_source_events;
-- DROP TABLE secretary_conversations;
-- DROP TABLE secretary_accounts;
