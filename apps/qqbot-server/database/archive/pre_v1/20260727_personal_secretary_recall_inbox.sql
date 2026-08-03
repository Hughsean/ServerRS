-- B3 durable recall inbox. A callback is successful only after this row is committed.
-- Depends on secretary_accounts from the ingestion migration.

CREATE TABLE IF NOT EXISTS secretary_recall_inbox
(
    recall_event_id       CHAR(36) CHARACTER SET ascii COLLATE ascii_bin PRIMARY KEY,
    account_id            BIGINT UNSIGNED NOT NULL,
    correlation_key       VARCHAR(500) COLLATE utf8mb4_bin NOT NULL,
    event_json             JSON NOT NULL,
    status                 VARCHAR(16) NOT NULL DEFAULT 'pending',
    attempts               INT UNSIGNED NOT NULL DEFAULT 0,
    next_attempt_at        DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    lease_token            CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NULL,
    lease_expires_at       DATETIME(6) NULL,
    last_error_code        VARCHAR(64) NULL,
    created_at             DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    updated_at             DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6)
        ON UPDATE CURRENT_TIMESTAMP(6),

    CONSTRAINT chk_secretary_recall_inbox_status
        CHECK (status IN ('pending', 'claimed', 'applied', 'failed')),
    CONSTRAINT fk_secretary_recall_inbox_account
        FOREIGN KEY (account_id) REFERENCES secretary_accounts(id) ON DELETE CASCADE,
    UNIQUE KEY uk_secretary_recall_inbox_correlation (account_id, correlation_key),
    INDEX idx_secretary_recall_inbox_claim
        (status, next_attempt_at, lease_expires_at, created_at)
) ENGINE = InnoDB
  DEFAULT CHARSET = utf8mb4
  COLLATE = utf8mb4_unicode_ci
  COMMENT = 'Durable recall inbox with lease, retry and inspectable failure state';
