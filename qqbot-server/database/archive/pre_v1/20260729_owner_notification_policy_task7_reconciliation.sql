-- Task 7: durable FollowUp source fence and singleton legacy reconciliation lease.
-- This migration is expand-only. Existing FollowUp rows start at source version 1.

ALTER TABLE secretary_follow_up_items
    ADD COLUMN source_version BIGINT UNSIGNED NOT NULL DEFAULT 1 AFTER source_memory_fact_id,
    ADD CONSTRAINT chk_secretary_follow_up_source_version CHECK (source_version > 0),
    ADD INDEX idx_secretary_follow_up_scan_source_version
        (status, due_at_unix_secs, source_version, follow_up_id);

CREATE TABLE secretary_notification_reconciliation_leases
(
    lease_name       VARCHAR(64) CHARACTER SET ascii COLLATE ascii_bin PRIMARY KEY,
    lease_token      CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NULL,
    lease_expires_at DATETIME(6) NULL,
    updated_at       DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6)
        ON UPDATE CURRENT_TIMESTAMP(6),
    CONSTRAINT chk_secretary_notification_reconciliation_lease_name
        CHECK (lease_name = 'legacy_owner_outbox_v1')
) ENGINE = InnoDB DEFAULT CHARSET = utf8mb4 COLLATE = utf8mb4_unicode_ci
  COMMENT = 'Legacy Owner Outbox reconciliation singleton lease';

INSERT INTO secretary_notification_reconciliation_leases (lease_name)
VALUES ('legacy_owner_outbox_v1');
