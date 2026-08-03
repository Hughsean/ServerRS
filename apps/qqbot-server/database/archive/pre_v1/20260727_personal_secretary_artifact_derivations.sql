-- B6 durable Artifact derivation queue. The source message transaction creates the job.

CREATE TABLE IF NOT EXISTS secretary_artifact_derivations
(
    source_event_id    CHAR(36) CHARACTER SET ascii COLLATE ascii_bin PRIMARY KEY,
    status             VARCHAR(16) NOT NULL DEFAULT 'pending',
    attempts           INT UNSIGNED NOT NULL DEFAULT 0,
    lease_token        CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NULL,
    lease_expires_at   DATETIME(6) NULL,
    last_error_code    VARCHAR(64) NULL,
    created_at         DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    updated_at         DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6)
        ON UPDATE CURRENT_TIMESTAMP(6),

    CONSTRAINT chk_secretary_artifact_derivation_status
        CHECK (status IN ('pending', 'claimed', 'completed', 'failed')),
    CONSTRAINT fk_secretary_artifact_derivation_source
        FOREIGN KEY (source_event_id) REFERENCES secretary_source_events(source_event_id)
        ON DELETE CASCADE,
    INDEX idx_secretary_artifact_derivation_claim
        (status, lease_expires_at, created_at)
) ENGINE = InnoDB
  DEFAULT CHARSET = utf8mb4
  COLLATE = utf8mb4_unicode_ci
  COMMENT = 'Durable derivation jobs for source-message Artifact envelopes';
