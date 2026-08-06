-- Owner-confirmed FollowUp dismissal controls (version fencing).
-- Depends on follow-up items, action planner and response expectation tables.

CREATE TABLE IF NOT EXISTS secretary_follow_up_owner_controls
(
    control_id              CHAR(36) CHARACTER SET ascii COLLATE ascii_bin PRIMARY KEY,
    effect_id               VARCHAR(255) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    run_id                  CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    proposal_id             CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    account_id              BIGINT UNSIGNED NOT NULL,
    follow_up_id            CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    previous_status         VARCHAR(16) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    current_status          VARCHAR(16) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    previous_source_version BIGINT UNSIGNED NOT NULL,
    current_source_version  BIGINT UNSIGNED NOT NULL,
    command_source_event_id CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    reason                  VARCHAR(1000) NOT NULL,
    created_at              DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),

    CONSTRAINT chk_secretary_follow_up_control_status
        CHECK (previous_status IN ('scheduled') AND current_status IN ('dismissed')),
    CONSTRAINT chk_secretary_follow_up_control_version
        CHECK (previous_source_version > 0
               AND current_source_version = previous_source_version + 1),
    CONSTRAINT fk_secretary_follow_up_control_run
        FOREIGN KEY (run_id) REFERENCES secretary_action_runs(run_id) ON DELETE RESTRICT,
    CONSTRAINT fk_secretary_follow_up_control_account
        FOREIGN KEY (account_id) REFERENCES secretary_accounts(id) ON DELETE CASCADE,
    CONSTRAINT fk_secretary_follow_up_control_item
        FOREIGN KEY (follow_up_id) REFERENCES secretary_follow_up_items(follow_up_id)
        ON DELETE RESTRICT,
    CONSTRAINT fk_secretary_follow_up_control_command
        FOREIGN KEY (command_source_event_id) REFERENCES secretary_source_events(source_event_id)
        ON DELETE RESTRICT,
    UNIQUE KEY uk_secretary_follow_up_control_effect (effect_id),
    INDEX idx_secretary_follow_up_control_item (account_id, follow_up_id, created_at)
) ENGINE = InnoDB DEFAULT CHARSET = utf8mb4 COLLATE = utf8mb4_unicode_ci
  COMMENT = 'Owner 对 FollowUp 的不可变 Effect 审计；版本精确递增，供并发 fencing 复盘';

-- 回滚：DROP TABLE secretary_follow_up_owner_controls;
