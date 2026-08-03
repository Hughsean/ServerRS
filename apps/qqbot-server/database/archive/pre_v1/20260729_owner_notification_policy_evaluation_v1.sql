-- Persist the verified Owner command recipient for policy-driven notifications.
-- Existing follow-up and agenda rows keep these fields NULL.
ALTER TABLE secretary_notification_outbox
    ADD COLUMN command_account_id BIGINT UNSIGNED NULL AFTER account_id,
    ADD COLUMN owner_actor_id VARCHAR(191) COLLATE utf8mb4_bin NULL AFTER command_account_id,
    DROP CHECK chk_secretary_notification_kind,
    ADD CONSTRAINT chk_secretary_notification_kind
        CHECK (notification_kind IN ('owner_reminder', 'owner_agenda_reminder', 'owner_policy_reminder')),
    DROP CHECK chk_secretary_notification_source,
    ADD CONSTRAINT chk_secretary_notification_source
        CHECK ((follow_up_id IS NOT NULL AND agenda_item_id IS NULL AND agenda_version IS NULL
                    AND notification_candidate_id IS NULL AND notification_decision_id IS NULL
                    AND command_account_id IS NULL AND owner_actor_id IS NULL)
            OR (follow_up_id IS NULL AND agenda_item_id IS NOT NULL AND agenda_version IS NOT NULL
                    AND notification_candidate_id IS NULL AND notification_decision_id IS NULL
                    AND command_account_id IS NULL AND owner_actor_id IS NULL)
            OR (follow_up_id IS NULL AND agenda_item_id IS NULL AND agenda_version IS NULL
                    AND notification_candidate_id IS NOT NULL AND notification_decision_id IS NOT NULL
                    AND command_account_id IS NOT NULL AND owner_actor_id IS NOT NULL)),
    ADD CONSTRAINT fk_secretary_notification_outbox_command_account
        FOREIGN KEY (command_account_id) REFERENCES secretary_accounts(id) ON DELETE RESTRICT,
    ADD INDEX idx_secretary_notification_policy_recipient
        (command_account_id, owner_actor_id, delivery_status, notification_id);

ALTER TABLE secretary_notification_decisions
    DROP INDEX uk_secretary_notification_decision_request,
    ADD INDEX idx_secretary_notification_decision_request
        (evaluation_request_id, created_at, notification_decision_id);
