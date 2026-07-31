-- Owner-confirmed thread semantic and lifecycle controls.
-- Depends on thread semantics and action planner tables.

CREATE TABLE IF NOT EXISTS secretary_thread_owner_controls
(
    control_id              CHAR(36) CHARACTER SET ascii COLLATE ascii_bin PRIMARY KEY,
    effect_id               VARCHAR(255) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    run_id                  CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    proposal_id             CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    account_id              BIGINT UNSIGNED NOT NULL,
    thread_id               CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    target_kind             VARCHAR(16) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    target_id               CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    control_kind            VARCHAR(32) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    previous_status         VARCHAR(16) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    current_status          VARCHAR(16) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    command_source_event_id CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    reason                  VARCHAR(1000) NOT NULL,
    created_at              DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),

    CONSTRAINT chk_secretary_thread_control_target
        CHECK (target_kind IN ('decision', 'question', 'thread')),
    CONSTRAINT chk_secretary_thread_control_kind
        CHECK (control_kind IN
            ('confirm_decision', 'revoke_decision', 'dismiss_question', 'close_thread', 'reopen_thread')),
    CONSTRAINT fk_secretary_thread_control_run
        FOREIGN KEY (run_id) REFERENCES secretary_action_runs(run_id) ON DELETE RESTRICT,
    CONSTRAINT fk_secretary_thread_control_account
        FOREIGN KEY (account_id) REFERENCES secretary_accounts(id) ON DELETE CASCADE,
    CONSTRAINT fk_secretary_thread_control_thread
        FOREIGN KEY (thread_id) REFERENCES secretary_event_threads(thread_id) ON DELETE RESTRICT,
    CONSTRAINT fk_secretary_thread_control_command
        FOREIGN KEY (command_source_event_id) REFERENCES secretary_source_events(source_event_id)
        ON DELETE RESTRICT,
    UNIQUE KEY uk_secretary_thread_control_effect (effect_id),
    INDEX idx_secretary_thread_control_thread (account_id, thread_id, created_at)
) ENGINE = InnoDB DEFAULT CHARSET = utf8mb4 COLLATE = utf8mb4_unicode_ci
  COMMENT = 'Owner 对线程结论、未决问题和生命周期的不可变 Effect 审计';

-- 回滚：DROP TABLE secretary_thread_owner_controls;
