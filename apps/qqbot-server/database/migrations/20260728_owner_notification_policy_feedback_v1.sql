-- Owner notification policy and feedback learning v1.
-- Policy revisions are append-only; family heads and account policy_epoch provide optimistic fencing.

ALTER TABLE secretary_accounts
    ADD COLUMN policy_epoch BIGINT UNSIGNED NOT NULL DEFAULT 0 AFTER status;

CREATE TABLE secretary_notification_policy_families
(
    policy_family_id     CHAR(36) CHARACTER SET ascii COLLATE ascii_bin PRIMARY KEY,
    account_id           BIGINT UNSIGNED NOT NULL,
    canonical_scope_key  VARCHAR(512) CHARACTER SET utf8mb4 COLLATE utf8mb4_bin NOT NULL,
    policy_kind          VARCHAR(64) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    -- 创建事务内暂为 NULL；提交前必须设置为属于本 Family 的 revision。
    current_revision_id  CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NULL,
    generation           BIGINT UNSIGNED NOT NULL,
    created_at           DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    updated_at           DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6)
        ON UPDATE CURRENT_TIMESTAMP(6),
    CONSTRAINT chk_secretary_notification_policy_family_generation CHECK (generation > 0),
    CONSTRAINT fk_secretary_notification_policy_family_account
        FOREIGN KEY (account_id) REFERENCES secretary_accounts(id) ON DELETE CASCADE,
    UNIQUE KEY uk_secretary_notification_policy_family
        (account_id, canonical_scope_key, policy_kind),
    UNIQUE KEY uk_secretary_notification_policy_family_head
        (policy_family_id, current_revision_id)
) ENGINE = InnoDB DEFAULT CHARSET = utf8mb4 COLLATE = utf8mb4_unicode_ci
  COMMENT = '通知策略稳定 Family 与可 CAS 更新的 Head';

CREATE TABLE secretary_notification_policy_revisions
(
    policy_revision_id        CHAR(36) CHARACTER SET ascii COLLATE ascii_bin PRIMARY KEY,
    policy_family_id          CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    revision_number           BIGINT UNSIGNED NOT NULL,
    supersedes_revision_id    CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NULL,
    revision_kind             VARCHAR(16) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    rule_json                 JSON NULL,
    command_source_event_id   CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NULL,
    audit_summary             VARCHAR(1024) CHARACTER SET utf8mb4 COLLATE utf8mb4_bin NOT NULL,
    created_at                DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    CONSTRAINT chk_secretary_notification_policy_revision_number CHECK (revision_number > 0),
    CONSTRAINT chk_secretary_notification_policy_revision_kind
        CHECK (revision_kind IN ('rule', 'tombstone')),
    CONSTRAINT chk_secretary_notification_policy_revision_shape
        CHECK ((revision_kind = 'rule' AND rule_json IS NOT NULL)
            OR (revision_kind = 'tombstone' AND rule_json IS NULL)),
    CONSTRAINT fk_secretary_notification_policy_revision_family
        FOREIGN KEY (policy_family_id) REFERENCES secretary_notification_policy_families(policy_family_id)
        ON DELETE CASCADE,
    CONSTRAINT fk_secretary_notification_policy_revision_supersedes
        FOREIGN KEY (supersedes_revision_id) REFERENCES secretary_notification_policy_revisions(policy_revision_id)
        ON DELETE RESTRICT,
    CONSTRAINT fk_secretary_notification_policy_revision_command
        FOREIGN KEY (command_source_event_id) REFERENCES secretary_source_events(source_event_id)
        ON DELETE RESTRICT,
    UNIQUE KEY uk_secretary_notification_policy_revision_number
        (policy_family_id, revision_number),
    UNIQUE KEY uk_secretary_notification_policy_revision_family_id
        (policy_family_id, policy_revision_id)
) ENGINE = InnoDB DEFAULT CHARSET = utf8mb4 COLLATE = utf8mb4_unicode_ci
  COMMENT = '不可变通知策略 revision；停用使用 tombstone';

ALTER TABLE secretary_notification_policy_families
    ADD CONSTRAINT fk_secretary_notification_policy_family_head
        FOREIGN KEY (policy_family_id, current_revision_id)
        REFERENCES secretary_notification_policy_revisions(policy_family_id, policy_revision_id)
        ON DELETE RESTRICT;

CREATE TABLE secretary_notification_candidates
(
    notification_candidate_id CHAR(36) CHARACTER SET ascii COLLATE ascii_bin PRIMARY KEY,
    account_id                BIGINT UNSIGNED NOT NULL,
    source_kind               VARCHAR(32) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    source_id                 CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    source_version            BIGINT UNSIGNED NOT NULL,
    match_key_json            JSON NOT NULL,
    candidate_status          VARCHAR(32) CHARACTER SET ascii COLLATE ascii_bin NOT NULL DEFAULT 'pending',
    created_at                DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    updated_at                DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6)
        ON UPDATE CURRENT_TIMESTAMP(6),
    CONSTRAINT chk_secretary_notification_candidate_kind
        CHECK (source_kind IN ('agenda', 'follow_up')),
    CONSTRAINT chk_secretary_notification_candidate_status
        CHECK (candidate_status IN ('pending', 'delayed', 'reminded', 'suppressed', 'expired', 'failed_terminal')),
    CONSTRAINT fk_secretary_notification_candidate_account
        FOREIGN KEY (account_id) REFERENCES secretary_accounts(id) ON DELETE CASCADE,
    UNIQUE KEY uk_secretary_notification_candidate_source
        (account_id, source_kind, source_id, source_version),
    INDEX idx_secretary_notification_candidate_pending
        (account_id, candidate_status, updated_at, notification_candidate_id)
) ENGINE = InnoDB DEFAULT CHARSET = utf8mb4 COLLATE = utf8mb4_unicode_ci
  COMMENT = 'Agenda 或 FollowUp 产生的版本化通知候选';

CREATE TABLE secretary_notification_evaluation_requests
(
    evaluation_request_id      CHAR(36) CHARACTER SET ascii COLLATE ascii_bin PRIMARY KEY,
    notification_candidate_id  CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    evaluation_generation      BIGINT UNSIGNED NOT NULL,
    trigger_kind               VARCHAR(32) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    request_status             VARCHAR(16) CHARACTER SET ascii COLLATE ascii_bin NOT NULL DEFAULT 'pending',
    lease_token                CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NULL,
    lease_expires_at_unix_secs BIGINT NULL,
    attempt                    BIGINT UNSIGNED NOT NULL DEFAULT 0,
    next_allowed_at_unix_secs  BIGINT NULL,
    created_at                 DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    updated_at                 DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6)
        ON UPDATE CURRENT_TIMESTAMP(6),
    CONSTRAINT chk_secretary_notification_evaluation_generation CHECK (evaluation_generation > 0),
    CONSTRAINT chk_secretary_notification_evaluation_status
        CHECK (request_status IN ('pending', 'claimed', 'completed', 'terminal')),
    CONSTRAINT fk_secretary_notification_evaluation_candidate
        FOREIGN KEY (notification_candidate_id) REFERENCES secretary_notification_candidates(notification_candidate_id)
        ON DELETE CASCADE,
    UNIQUE KEY uk_secretary_notification_evaluation_generation
        (notification_candidate_id, evaluation_generation),
    INDEX idx_secretary_notification_evaluation_claim
        (request_status, next_allowed_at_unix_secs, lease_expires_at_unix_secs, evaluation_request_id)
) ENGINE = InnoDB DEFAULT CHARSET = utf8mb4 COLLATE = utf8mb4_unicode_ci
  COMMENT = '通知候选的可租约三阶段求值请求';

CREATE TABLE secretary_notification_decisions
(
    notification_decision_id   CHAR(36) CHARACTER SET ascii COLLATE ascii_bin PRIMARY KEY,
    evaluation_request_id      CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    notification_candidate_id  CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    previous_decision_id       CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NULL,
    policy_revision_id         CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NULL,
    evaluator_version          VARCHAR(64) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    outcome                    VARCHAR(32) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    reason_code                VARCHAR(256) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    next_allowed_at_unix_secs  BIGINT NULL,
    created_at                 DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    CONSTRAINT chk_secretary_notification_decision_outcome
        CHECK (outcome IN ('remind', 'delay', 'suppress', 'candidate_expired',
            'evaluation_failed_terminal', 'delivery_window_expired', 'schedule_time_ambiguous')),
    CONSTRAINT fk_secretary_notification_decision_request
        FOREIGN KEY (evaluation_request_id) REFERENCES secretary_notification_evaluation_requests(evaluation_request_id)
        ON DELETE RESTRICT,
    CONSTRAINT fk_secretary_notification_decision_candidate
        FOREIGN KEY (notification_candidate_id) REFERENCES secretary_notification_candidates(notification_candidate_id)
        ON DELETE RESTRICT,
    CONSTRAINT fk_secretary_notification_decision_previous
        FOREIGN KEY (previous_decision_id) REFERENCES secretary_notification_decisions(notification_decision_id)
        ON DELETE RESTRICT,
    CONSTRAINT fk_secretary_notification_decision_revision
        FOREIGN KEY (policy_revision_id) REFERENCES secretary_notification_policy_revisions(policy_revision_id)
        ON DELETE RESTRICT,
    UNIQUE KEY uk_secretary_notification_decision_request (evaluation_request_id),
    INDEX idx_secretary_notification_decision_candidate (notification_candidate_id, created_at)
) ENGINE = InnoDB DEFAULT CHARSET = utf8mb4 COLLATE = utf8mb4_unicode_ci
  COMMENT = '追加式通知策略决策审计';

CREATE TABLE secretary_notification_feedback
(
    feedback_id                CHAR(36) CHARACTER SET ascii COLLATE ascii_bin PRIMARY KEY,
    account_id                 BIGINT UNSIGNED NOT NULL,
    notification_candidate_id  CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    important                  BOOLEAN NOT NULL,
    promote_to_rule            BOOLEAN NOT NULL DEFAULT FALSE,
    command_source_event_id    CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    audit_summary              VARCHAR(1024) CHARACTER SET utf8mb4 COLLATE utf8mb4_bin NOT NULL,
    created_at                 DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    CONSTRAINT fk_secretary_notification_feedback_account
        FOREIGN KEY (account_id) REFERENCES secretary_accounts(id) ON DELETE CASCADE,
    CONSTRAINT fk_secretary_notification_feedback_candidate
        FOREIGN KEY (notification_candidate_id) REFERENCES secretary_notification_candidates(notification_candidate_id)
        ON DELETE RESTRICT,
    CONSTRAINT fk_secretary_notification_feedback_command
        FOREIGN KEY (command_source_event_id) REFERENCES secretary_source_events(source_event_id)
        ON DELETE RESTRICT,
    UNIQUE KEY uk_secretary_notification_feedback_command
        (account_id, command_source_event_id, important),
    INDEX idx_secretary_notification_feedback_candidate (notification_candidate_id, created_at)
) ENGINE = InnoDB DEFAULT CHARSET = utf8mb4 COLLATE = utf8mb4_unicode_ci
  COMMENT = 'Owner 对单条通知的结构化重要性反馈';

ALTER TABLE secretary_notification_outbox
    ADD COLUMN notification_candidate_id CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NULL AFTER agenda_version,
    ADD COLUMN notification_decision_id CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NULL AFTER notification_candidate_id,
    ADD COLUMN occurrence_id CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NULL AFTER notification_decision_id,
    DROP CHECK chk_secretary_notification_source,
    ADD CONSTRAINT chk_secretary_notification_source
        CHECK ((follow_up_id IS NOT NULL AND agenda_item_id IS NULL AND agenda_version IS NULL
                    AND notification_candidate_id IS NULL AND notification_decision_id IS NULL)
            OR (follow_up_id IS NULL AND agenda_item_id IS NOT NULL AND agenda_version IS NOT NULL
                    AND notification_candidate_id IS NULL AND notification_decision_id IS NULL)
            OR (follow_up_id IS NULL AND agenda_item_id IS NULL AND agenda_version IS NULL
                    AND notification_candidate_id IS NOT NULL AND notification_decision_id IS NOT NULL)),
    ADD CONSTRAINT fk_secretary_notification_outbox_candidate
        FOREIGN KEY (notification_candidate_id) REFERENCES secretary_notification_candidates(notification_candidate_id)
        ON DELETE RESTRICT,
    ADD CONSTRAINT fk_secretary_notification_outbox_decision
        FOREIGN KEY (notification_decision_id) REFERENCES secretary_notification_decisions(notification_decision_id)
        ON DELETE RESTRICT,
    ADD UNIQUE KEY uk_secretary_notification_occurrence (occurrence_id);
