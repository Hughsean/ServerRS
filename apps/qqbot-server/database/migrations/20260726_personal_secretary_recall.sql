-- B3 消息撤回闭环：撤回事件（可审计）与 tombstone（被撤回原消息失效记录）。
-- 约束：不物理删除审计历史；保留撤回事件、原消息信封、被撤回状态、失效原因、来源关系、投影时间。
-- 关联键：(account_id, channel, conversation, platform_message_id)，禁止单 message_id 跨账号关联。
-- Forward-only migration. Depends on 20260723_personal_secretary_ingestion.sql (account FK).
-- 回滚顺序：tombstones -> recall_events。

-- 1. secretary_recall_events：每条撤回通知的审计记录。撤回本身也是一条 SourceEvent。
CREATE TABLE IF NOT EXISTS secretary_recall_events
(
    recall_event_id        CHAR(36) CHARACTER SET ascii COLLATE ascii_bin PRIMARY KEY,
    account_id             BIGINT UNSIGNED NOT NULL,
    recall_kind            VARCHAR(16)     NOT NULL,
    -- 关联键的四元组（禁止单 message_id 跨账号）：
    channel                VARCHAR(32)     NOT NULL,
    conversation_kind      VARCHAR(32)     NOT NULL,
    platform_conversation_id VARCHAR(191)  COLLATE utf8mb4_bin NOT NULL,
    platform_message_id    VARCHAR(191)    COLLATE utf8mb4_bin NOT NULL,
    correlation_key        VARCHAR(500)    COLLATE utf8mb4_bin NOT NULL,
    operator_platform_id   VARCHAR(191)    COLLATE utf8mb4_bin NULL,
    occurred_at_unix_secs  BIGINT          NOT NULL,
    created_at             DATETIME(6)     NOT NULL DEFAULT CURRENT_TIMESTAMP(6),

    CONSTRAINT chk_secretary_recall_kind
        CHECK (recall_kind IN ('group', 'friend')),
    CONSTRAINT chk_secretary_recall_channel
        CHECK (channel IN ('napcat', 'qq_open_platform')),
    CONSTRAINT chk_secretary_recall_conv_kind
        CHECK (conversation_kind IN ('private', 'group', 'owner_control')),
    CONSTRAINT fk_secretary_recall_event_account
        FOREIGN KEY (account_id) REFERENCES secretary_accounts(id) ON DELETE CASCADE,
    -- 同一关联键的撤回幂等：相同 (account_id, correlation_key) 只保留一条。
    UNIQUE KEY uk_secretary_recall_correlation (account_id, correlation_key),
    INDEX idx_secretary_recall_account_time (account_id, occurred_at_unix_secs),
    INDEX idx_secretary_recall_correlation_lookup
        (account_id, channel, conversation_kind, platform_conversation_id, platform_message_id)
) ENGINE = InnoDB
  DEFAULT CHARSET = utf8mb4
  COLLATE = utf8mb4_unicode_ci
  COMMENT = '撤回事件审计记录（撤回本身也是 SourceEvent；关联键禁止单 message_id 跨账号）';

-- 2. secretary_message_tombstones：被撤回原消息的失效记录。
-- 撤回先到时 status=pending，source_event_id=NULL；原消息后到时关联并 status=applied。
-- 不物理删除审计历史：保留被撤回状态、失效原因、投影时间。
CREATE TABLE IF NOT EXISTS secretary_message_tombstones
(
    tombstone_id           BIGINT UNSIGNED PRIMARY KEY AUTO_INCREMENT,
    account_id             BIGINT UNSIGNED NOT NULL,
    -- 被撤回原消息的 SourceEvent ID。撤回先到时为 NULL（pending）。
    source_event_id        CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NULL,
    recall_event_id        CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    -- 关联键（用于匹配原消息）。
    channel                VARCHAR(32)     NOT NULL,
    conversation_kind      VARCHAR(32)     NOT NULL,
    platform_conversation_id VARCHAR(191)  COLLATE utf8mb4_bin NOT NULL,
    platform_message_id    VARCHAR(191)    COLLATE utf8mb4_bin NOT NULL,
    correlation_key        VARCHAR(500)    COLLATE utf8mb4_bin NOT NULL,
    status                 VARCHAR(24)     NOT NULL DEFAULT 'pending',
    invalidation_reason    VARCHAR(500)    NOT NULL,
    invalidated_at_unix_secs BIGINT        NOT NULL,
    created_at             DATETIME(6)     NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    updated_at             DATETIME(6)     NOT NULL DEFAULT CURRENT_TIMESTAMP(6)
        ON UPDATE CURRENT_TIMESTAMP(6),

    CONSTRAINT chk_secretary_tombstone_status
        CHECK (status IN ('pending', 'applied', 'idempotent_reapply')),
    CONSTRAINT chk_secretary_tombstone_channel
        CHECK (channel IN ('napcat', 'qq_open_platform')),
    CONSTRAINT chk_secretary_tombstone_conv_kind
        CHECK (conversation_kind IN ('private', 'group', 'owner_control')),
    CONSTRAINT fk_secretary_tombstone_account
        FOREIGN KEY (account_id) REFERENCES secretary_accounts(id) ON DELETE CASCADE,
    CONSTRAINT fk_secretary_tombstone_recall
        FOREIGN KEY (recall_event_id) REFERENCES secretary_recall_events(recall_event_id)
        ON DELETE CASCADE,
    -- source_event_id 关联 secretary_source_events，但允许 NULL（pending）。
    -- ON DELETE SET NULL：原消息物理删除时 tombstone 保留审计。
    CONSTRAINT fk_secretary_tombstone_source_event
        FOREIGN KEY (source_event_id) REFERENCES secretary_source_events(source_event_id)
        ON DELETE SET NULL,
    -- 同一关联键的 tombstone 唯一：撤回先到创建 pending，原消息后到更新为 applied。
    UNIQUE KEY uk_secretary_tombstone_correlation (account_id, correlation_key),
    INDEX idx_secretary_tombstone_source_event (source_event_id, status),
    INDEX idx_secretary_tombstone_account_status (account_id, status),
    INDEX idx_secretary_tombstone_pending_lookup
        (account_id, channel, conversation_kind, platform_conversation_id, platform_message_id, status)
) ENGINE = InnoDB
  DEFAULT CHARSET = utf8mb4
  COLLATE = utf8mb4_unicode_ci
  COMMENT = '被撤回原消息的 tombstone 记录（pending/applied；不物理删除审计历史）';
