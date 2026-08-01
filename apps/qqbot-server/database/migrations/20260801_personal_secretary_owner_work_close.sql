-- Owner-confirmed work close controls: follow-up completion and
-- response-expectation dismissal.
-- Must run after 20260801_personal_secretary_follow_up_batch_controls.sql.

-- 1) FollowUp 审计允许 complete：previous_status=scheduled -> current_status=completed，
--    版本精确 +1；due 列对 complete 行不做要求（可以记录也可以保持 NULL）。
--    既有 dismiss/snooze 行不删除、不改写，约束重定义后仍全部兼容。
ALTER TABLE secretary_follow_up_owner_controls
    DROP CHECK chk_secretary_follow_up_control_kind,
    DROP CHECK chk_secretary_follow_up_control_status,
    DROP CHECK chk_secretary_follow_up_control_due,
    ADD CONSTRAINT chk_secretary_follow_up_control_kind
        CHECK (control_kind IN ('dismiss', 'snooze', 'complete')),
    ADD CONSTRAINT chk_secretary_follow_up_control_status
        CHECK (previous_status IN ('scheduled')
               AND current_status IN ('dismissed', 'scheduled', 'completed')),
    ADD CONSTRAINT chk_secretary_follow_up_control_due
        CHECK ((control_kind = 'dismiss' AND current_status = 'dismissed')
               OR (control_kind = 'snooze' AND current_status = 'scheduled'
                   AND previous_due_at_unix_secs IS NOT NULL
                   AND current_due_at_unix_secs IS NOT NULL
                   AND current_due_at_unix_secs > previous_due_at_unix_secs)
               OR (control_kind = 'complete' AND current_status = 'completed'));

-- 2) ResponseExpectation 的 Owner 关闭审计：active -> dismissed 且版本精确 +1。
--    resolved 只由扫描器依据真实回复、问题关闭或线程终态写入，Owner 手工关闭
--    一律走 dismissed，不得改写成 resolved。
CREATE TABLE IF NOT EXISTS secretary_response_expectation_owner_controls
(
    control_id              CHAR(36) CHARACTER SET ascii COLLATE ascii_bin PRIMARY KEY,
    effect_id               VARCHAR(255) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    run_id                  CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    proposal_id             CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    account_id              BIGINT UNSIGNED NOT NULL,
    expectation_id          CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    previous_status         VARCHAR(16) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    current_status          VARCHAR(16) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    previous_source_version BIGINT UNSIGNED NOT NULL,
    current_source_version  BIGINT UNSIGNED NOT NULL,
    command_source_event_id CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    reason                  VARCHAR(1000) NOT NULL,
    created_at              DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),

    CONSTRAINT chk_secretary_expectation_control_status
        CHECK (previous_status IN ('active') AND current_status IN ('dismissed')),
    CONSTRAINT chk_secretary_expectation_control_version
        CHECK (previous_source_version > 0
               AND current_source_version = previous_source_version + 1),
    CONSTRAINT fk_secretary_expectation_control_run
        FOREIGN KEY (run_id) REFERENCES secretary_action_runs(run_id) ON DELETE RESTRICT,
    CONSTRAINT fk_secretary_expectation_control_account
        FOREIGN KEY (account_id) REFERENCES secretary_accounts(id) ON DELETE CASCADE,
    CONSTRAINT fk_secretary_expectation_control_item
        FOREIGN KEY (expectation_id) REFERENCES secretary_response_expectations(expectation_id)
        ON DELETE RESTRICT,
    CONSTRAINT fk_secretary_expectation_control_command
        FOREIGN KEY (command_source_event_id) REFERENCES secretary_source_events(source_event_id)
        ON DELETE RESTRICT,
    UNIQUE KEY uk_secretary_expectation_control_effect_item (effect_id, expectation_id),
    INDEX idx_secretary_expectation_control_item (account_id, expectation_id, created_at)
) ENGINE = InnoDB DEFAULT CHARSET = utf8mb4 COLLATE = utf8mb4_unicode_ci
  COMMENT = 'Owner 对 ResponseExpectation 的不可变 Effect 审计；版本精确递增，供并发 fencing 复盘';

-- 回滚：
-- DROP TABLE secretary_response_expectation_owner_controls;
-- ALTER TABLE secretary_follow_up_owner_controls
--     DROP CHECK chk_secretary_follow_up_control_due,
--     DROP CHECK chk_secretary_follow_up_control_status,
--     DROP CHECK chk_secretary_follow_up_control_kind,
--     ADD CONSTRAINT chk_secretary_follow_up_control_kind
--         CHECK (control_kind IN ('dismiss', 'snooze')),
--     ADD CONSTRAINT chk_secretary_follow_up_control_status
--         CHECK (previous_status IN ('scheduled')
--                AND current_status IN ('dismissed', 'scheduled')),
--     ADD CONSTRAINT chk_secretary_follow_up_control_due
--         CHECK ((control_kind = 'dismiss' AND current_status = 'dismissed')
--                OR (control_kind = 'snooze' AND current_status = 'scheduled'
--                    AND previous_due_at_unix_secs IS NOT NULL
--                    AND current_due_at_unix_secs IS NOT NULL
--                    AND current_due_at_unix_secs > previous_due_at_unix_secs));
