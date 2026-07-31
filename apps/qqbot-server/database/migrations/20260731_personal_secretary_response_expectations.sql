-- Source-backed response expectations for unanswered external questions.
-- Depends on thread semantics and notification policy feedback v1.

CREATE TABLE IF NOT EXISTS secretary_response_expectations
(
    expectation_id      CHAR(36) CHARACTER SET ascii COLLATE ascii_bin PRIMARY KEY,
    account_id          BIGINT UNSIGNED NOT NULL,
    source_question_id  CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    thread_id           CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    source_version      BIGINT UNSIGNED NOT NULL DEFAULT 1,
    due_at_unix_secs    BIGINT NOT NULL,
    expectation_status  VARCHAR(16) CHARACTER SET ascii COLLATE ascii_bin NOT NULL DEFAULT 'active',
    created_at          DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    updated_at          DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6)
        ON UPDATE CURRENT_TIMESTAMP(6),

    CONSTRAINT chk_secretary_response_expectation_version CHECK (source_version > 0),
    CONSTRAINT chk_secretary_response_expectation_status
        CHECK (expectation_status IN ('active', 'resolved', 'dismissed', 'superseded')),
    CONSTRAINT fk_secretary_response_expectation_account
        FOREIGN KEY (account_id) REFERENCES secretary_accounts(id) ON DELETE CASCADE,
    CONSTRAINT fk_secretary_response_expectation_question
        FOREIGN KEY (source_question_id) REFERENCES secretary_thread_open_questions(question_id)
        ON DELETE CASCADE,
    CONSTRAINT fk_secretary_response_expectation_thread
        FOREIGN KEY (thread_id) REFERENCES secretary_event_threads(thread_id) ON DELETE CASCADE,
    UNIQUE KEY uk_secretary_response_expectation_question (source_question_id),
    INDEX idx_secretary_response_expectation_due
        (expectation_status, due_at_unix_secs, expectation_id)
) ENGINE = InnoDB DEFAULT CHARSET = utf8mb4 COLLATE = utf8mb4_unicode_ci
  COMMENT = '外部联系人开放问题的来源化回复期待；本人后续回复或线程终态会自动结束';

ALTER TABLE secretary_notification_candidates
    DROP CHECK chk_secretary_notification_candidate_kind,
    ADD CONSTRAINT chk_secretary_notification_candidate_kind
        CHECK (source_kind IN ('agenda', 'follow_up', 'response_expectation'));

-- 回滚顺序：先恢复 candidate kind CHECK，再删除 response_expectations。
