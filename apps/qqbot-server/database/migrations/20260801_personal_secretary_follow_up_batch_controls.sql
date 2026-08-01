-- Owner-confirmed batch FollowUp dismissal controls.
-- A single batch effect owns one audit row per target, so the single-column
-- effect unique key is replaced by a composite (effect_id, follow_up_id) key.
-- Must run after 20260801_personal_secretary_follow_up_snooze.sql.

ALTER TABLE secretary_follow_up_owner_controls
    DROP INDEX uk_secretary_follow_up_control_effect,
    ADD UNIQUE KEY uk_secretary_follow_up_control_effect_item (effect_id, follow_up_id);

-- 既有单条 dismiss/snooze 行不受影响：每个 effect_id 仍只有一行，复合键保持唯一；
-- 既有审计行不删除、不改写。

-- 回滚：
-- ALTER TABLE secretary_follow_up_owner_controls
--     DROP INDEX uk_secretary_follow_up_control_effect_item,
--     ADD UNIQUE KEY uk_secretary_follow_up_control_effect (effect_id);
