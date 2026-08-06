-- Owner-confirmed FollowUp snooze controls (version fencing).
-- Extends secretary_follow_up_owner_controls with control_kind and due-time columns.
-- Must run after 20260801_personal_secretary_follow_up_owner_controls.sql.

ALTER TABLE secretary_follow_up_owner_controls
    ADD COLUMN control_kind VARCHAR(16) CHARACTER SET ascii COLLATE ascii_bin
        NOT NULL DEFAULT 'dismiss' AFTER reason,
    ADD COLUMN previous_due_at_unix_secs BIGINT NULL AFTER current_source_version,
    ADD COLUMN current_due_at_unix_secs BIGINT NULL AFTER previous_due_at_unix_secs,
    DROP CHECK chk_secretary_follow_up_control_status,
    ADD CONSTRAINT chk_secretary_follow_up_control_kind
        CHECK (control_kind IN ('dismiss', 'snooze')),
    ADD CONSTRAINT chk_secretary_follow_up_control_status
        CHECK (previous_status IN ('scheduled')
               AND current_status IN ('dismissed', 'scheduled')),
    ADD CONSTRAINT chk_secretary_follow_up_control_due
        CHECK ((control_kind = 'dismiss' AND current_status = 'dismissed')
               OR (control_kind = 'snooze' AND current_status = 'scheduled'
                   AND previous_due_at_unix_secs IS NOT NULL
                   AND current_due_at_unix_secs IS NOT NULL
                   AND current_due_at_unix_secs > previous_due_at_unix_secs));

-- 历史 dismiss 行：control_kind 默认 'dismiss'，due 列保持 NULL，约束自动兼容。

-- 回滚：
-- ALTER TABLE secretary_follow_up_owner_controls
--     DROP CHECK chk_secretary_follow_up_control_due,
--     DROP CHECK chk_secretary_follow_up_control_status,
--     DROP CHECK chk_secretary_follow_up_control_kind,
--     DROP COLUMN current_due_at_unix_secs,
--     DROP COLUMN previous_due_at_unix_secs,
--     DROP COLUMN control_kind,
--     ADD CONSTRAINT chk_secretary_follow_up_control_status
--         CHECK (previous_status IN ('scheduled') AND current_status IN ('dismissed'));
