-- Owner Agenda/Reminder v1. Reuses the existing Action Graph and notification outbox.

ALTER TABLE secretary_action_runs
    ADD COLUMN timezone_name VARCHAR(64) CHARACTER SET ascii COLLATE ascii_bin
        NOT NULL DEFAULT 'UTC' AFTER timezone_offset_secs;

CREATE TABLE IF NOT EXISTS secretary_agenda_items
(
    item_id                   CHAR(36) CHARACTER SET ascii COLLATE ascii_bin PRIMARY KEY,
    account_id                BIGINT UNSIGNED NOT NULL,
    item_kind                 VARCHAR(16) NOT NULL,
    title                     VARCHAR(500) NOT NULL,
    scheduled_at_unix_secs    BIGINT NULL,
    timezone_name             VARCHAR(64) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    item_status               VARCHAR(16) NOT NULL DEFAULT 'scheduled',
    version                   BIGINT UNSIGNED NOT NULL DEFAULT 1,
    created_command_event_id  CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    current_command_event_id  CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    create_idempotency_key    VARCHAR(191) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    created_at                DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    updated_at                DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6)
        ON UPDATE CURRENT_TIMESTAMP(6),
    CONSTRAINT chk_secretary_agenda_kind
        CHECK (item_kind IN ('schedule', 'task', 'reminder')),
    CONSTRAINT chk_secretary_agenda_status
        CHECK (item_status IN ('scheduled', 'completed', 'cancelled')),
    CONSTRAINT chk_secretary_agenda_version CHECK (version > 0),
    CONSTRAINT fk_secretary_agenda_account
        FOREIGN KEY (account_id) REFERENCES secretary_accounts(id) ON DELETE CASCADE,
    CONSTRAINT fk_secretary_agenda_created_command
        FOREIGN KEY (created_command_event_id) REFERENCES secretary_source_events(source_event_id)
        ON DELETE RESTRICT,
    CONSTRAINT fk_secretary_agenda_current_command
        FOREIGN KEY (current_command_event_id) REFERENCES secretary_source_events(source_event_id)
        ON DELETE RESTRICT,
    UNIQUE KEY uk_secretary_agenda_create (account_id, create_idempotency_key),
    INDEX idx_secretary_agenda_due
        (account_id, item_status, scheduled_at_unix_secs, item_id)
) ENGINE = InnoDB DEFAULT CHARSET = utf8mb4 COLLATE = utf8mb4_unicode_ci
  COMMENT = 'Owner 日程、任务和提醒；UTC 时间 + IANA timezone + version fencing';

CREATE TABLE IF NOT EXISTS secretary_agenda_mutation_audit
(
    audit_id                CHAR(36) CHARACTER SET ascii COLLATE ascii_bin PRIMARY KEY,
    item_id                 CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    account_id              BIGINT UNSIGNED NOT NULL,
    command_source_event_id CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    run_id                  CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    effect_id               VARCHAR(255) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    mutation_kind           VARCHAR(16) NOT NULL,
    from_version            BIGINT UNSIGNED NULL,
    to_version              BIGINT UNSIGNED NOT NULL,
    detail_json             JSON NOT NULL,
    created_at              DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    CONSTRAINT chk_secretary_agenda_mutation_kind
        CHECK (mutation_kind IN ('create', 'reschedule', 'complete', 'cancel', 'snooze')),
    CONSTRAINT fk_secretary_agenda_audit_item
        FOREIGN KEY (item_id) REFERENCES secretary_agenda_items(item_id) ON DELETE RESTRICT,
    CONSTRAINT fk_secretary_agenda_audit_account
        FOREIGN KEY (account_id) REFERENCES secretary_accounts(id) ON DELETE CASCADE,
    CONSTRAINT fk_secretary_agenda_audit_command
        FOREIGN KEY (command_source_event_id) REFERENCES secretary_source_events(source_event_id)
        ON DELETE RESTRICT,
    CONSTRAINT fk_secretary_agenda_audit_run
        FOREIGN KEY (run_id) REFERENCES secretary_action_runs(run_id) ON DELETE RESTRICT,
    UNIQUE KEY uk_secretary_agenda_effect (effect_id),
    INDEX idx_secretary_agenda_audit_item (item_id, to_version)
) ENGINE = InnoDB DEFAULT CHARSET = utf8mb4 COLLATE = utf8mb4_unicode_ci
  COMMENT = 'Agenda mutation 不可变审计；effect_id 同时作为业务幂等键';

-- 泛化现有 Owner-only outbox 来源；保持 claim/lease/retry/unknown_commit 状态机不变。
ALTER TABLE secretary_notification_outbox
    ADD COLUMN agenda_item_id CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NULL AFTER follow_up_id,
    ADD COLUMN agenda_version BIGINT UNSIGNED NULL AFTER agenda_item_id,
    MODIFY follow_up_id CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NULL,
    DROP CHECK chk_secretary_notification_kind,
    ADD CONSTRAINT chk_secretary_notification_kind
        CHECK (notification_kind IN ('owner_reminder', 'owner_agenda_reminder')),
    ADD CONSTRAINT fk_secretary_notification_agenda
        FOREIGN KEY (agenda_item_id) REFERENCES secretary_agenda_items(item_id) ON DELETE RESTRICT,
    ADD CONSTRAINT chk_secretary_notification_source
        CHECK ((follow_up_id IS NOT NULL AND agenda_item_id IS NULL AND agenda_version IS NULL)
            OR (follow_up_id IS NULL AND agenda_item_id IS NOT NULL AND agenda_version IS NOT NULL)),
    ADD UNIQUE KEY uk_secretary_notification_agenda
        (agenda_item_id, agenda_version, notification_kind);
