-- QQ Personal Secretary memory control and persistent follow-up scheduler.
-- Depends on 20260724_personal_secretary_memory.sql.

CREATE TABLE IF NOT EXISTS secretary_memory_deletions
(
    fact_id                 CHAR(36) CHARACTER SET ascii COLLATE ascii_bin PRIMARY KEY,
    command_source_event_id CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    owner_actor_id          VARCHAR(191) COLLATE utf8mb4_bin NOT NULL,
    reason                  VARCHAR(1000) NOT NULL,
    created_at              DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    CONSTRAINT fk_secretary_memory_deletion_fact
        FOREIGN KEY (fact_id) REFERENCES secretary_memory_facts(fact_id) ON DELETE RESTRICT,
    CONSTRAINT fk_secretary_memory_deletion_command
        FOREIGN KEY (command_source_event_id) REFERENCES secretary_source_events(source_event_id)
        ON DELETE RESTRICT,
    INDEX idx_secretary_memory_deletion_command (command_source_event_id)
) ENGINE = InnoDB DEFAULT CHARSET = utf8mb4 COLLATE = utf8mb4_unicode_ci
  COMMENT = 'Owner 对派生记忆执行删除的不可变审计记录；不隐式删除原始事件';

CREATE TABLE IF NOT EXISTS secretary_follow_up_items
(
    follow_up_id          CHAR(36) CHARACTER SET ascii COLLATE ascii_bin PRIMARY KEY,
    account_id            BIGINT UNSIGNED NOT NULL,
    source_memory_fact_id CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    reason_code           VARCHAR(32) NOT NULL,
    due_at_unix_secs      BIGINT NOT NULL,
    status                VARCHAR(16) NOT NULL DEFAULT 'scheduled',
    created_at            DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    updated_at            DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6)
        ON UPDATE CURRENT_TIMESTAMP(6),
    CONSTRAINT chk_secretary_follow_up_reason CHECK (reason_code IN ('commitment_due')),
    CONSTRAINT chk_secretary_follow_up_status
        CHECK (status IN ('scheduled', 'completed', 'dismissed', 'superseded')),
    CONSTRAINT fk_secretary_follow_up_account
        FOREIGN KEY (account_id) REFERENCES secretary_accounts(id) ON DELETE CASCADE,
    CONSTRAINT fk_secretary_follow_up_memory
        FOREIGN KEY (source_memory_fact_id) REFERENCES secretary_memory_facts(fact_id)
        ON DELETE RESTRICT,
    UNIQUE KEY uk_secretary_follow_up_source (source_memory_fact_id, reason_code),
    INDEX idx_secretary_follow_up_due (status, due_at_unix_secs, follow_up_id)
) ENGINE = InnoDB DEFAULT CHARSET = utf8mb4 COLLATE = utf8mb4_unicode_ci
  COMMENT = '由来源化承诺记忆生成的持久化跟进事项';

CREATE TABLE IF NOT EXISTS secretary_notification_outbox
(
    notification_id       CHAR(36) CHARACTER SET ascii COLLATE ascii_bin PRIMARY KEY,
    account_id            BIGINT UNSIGNED NOT NULL,
    follow_up_id          CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    scheduled_at_unix_secs BIGINT NOT NULL,
    notification_kind     VARCHAR(32) NOT NULL,
    payload_json          JSON NOT NULL,
    delivery_status       VARCHAR(16) NOT NULL DEFAULT 'pending',
    attempts              INT UNSIGNED NOT NULL DEFAULT 0,
    lease_token           CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NULL,
    lease_expires_at      DATETIME(6) NULL,
    last_error_code       VARCHAR(64) NULL,
    platform_message_id   VARCHAR(512) COLLATE utf8mb4_bin NULL,
    delivered_at          DATETIME(6) NULL,
    created_at            DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    updated_at            DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6)
        ON UPDATE CURRENT_TIMESTAMP(6),
    CONSTRAINT chk_secretary_notification_kind CHECK (notification_kind IN ('owner_reminder')),
    CONSTRAINT chk_secretary_notification_status
        CHECK (delivery_status IN
            ('pending', 'claimed', 'delivered', 'failed', 'suppressed', 'unknown_commit')),
    CONSTRAINT fk_secretary_notification_account
        FOREIGN KEY (account_id) REFERENCES secretary_accounts(id) ON DELETE CASCADE,
    CONSTRAINT fk_secretary_notification_follow_up
        FOREIGN KEY (follow_up_id) REFERENCES secretary_follow_up_items(follow_up_id)
        ON DELETE RESTRICT,
    UNIQUE KEY uk_secretary_notification_follow_up (follow_up_id, notification_kind),
    INDEX idx_secretary_notification_claim
        (delivery_status, scheduled_at_unix_secs, lease_expires_at, notification_id)
) ENGINE = InnoDB DEFAULT CHARSET = utf8mb4 COLLATE = utf8mb4_unicode_ci
  COMMENT = '平台无关通知 Outbox；QQ 开放平台接入前只入队、不发送';

-- 回滚顺序：notification_outbox -> follow_up_items -> memory_deletions。
