-- QQ Personal Secretary typed thread semantics and lifecycle evidence.
-- Depends on 20260724_personal_secretary_threads.sql.

CREATE TABLE IF NOT EXISTS secretary_thread_semantic_state
(
    thread_id                    CHAR(36) CHARACTER SET ascii COLLATE ascii_bin PRIMARY KEY,
    last_added_at                DATETIME(6) NULL,
    last_source_event_id         CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NULL,
    lease_token                  CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NULL,
    lease_expires_at             DATETIME(6) NULL,
    attempts                     INT UNSIGNED NOT NULL DEFAULT 0,
    last_error                   VARCHAR(512) NULL,
    updated_at                   DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6)
        ON UPDATE CURRENT_TIMESTAMP(6),

    CONSTRAINT fk_secretary_thread_semantic_state_thread
        FOREIGN KEY (thread_id) REFERENCES secretary_event_threads(thread_id) ON DELETE CASCADE,
    CONSTRAINT fk_secretary_thread_semantic_state_event
        FOREIGN KEY (last_source_event_id) REFERENCES secretary_source_events(source_event_id)
        ON DELETE CASCADE,
    INDEX idx_secretary_thread_semantic_claim (lease_expires_at, updated_at, thread_id),
    INDEX idx_secretary_thread_semantic_token (lease_token)
) ENGINE = InnoDB
  DEFAULT CHARSET = utf8mb4
  COLLATE = utf8mb4_unicode_ci
  COMMENT = '线程语义批处理游标与独立租约；失败不推进游标';

CREATE TABLE IF NOT EXISTS secretary_thread_claims
(
    claim_id          CHAR(36) CHARACTER SET ascii COLLATE ascii_bin PRIMARY KEY,
    thread_id         CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    claim_kind        VARCHAR(24) NOT NULL,
    claimant_channel  VARCHAR(32) NOT NULL,
    claimant_account  VARCHAR(191) COLLATE utf8mb4_bin NOT NULL,
    claimant_actor_id VARCHAR(191) COLLATE utf8mb4_bin NOT NULL,
    statement         TEXT NOT NULL,
    status            VARCHAR(16) NOT NULL DEFAULT 'proposed',
    confidence_bps    SMALLINT UNSIGNED NOT NULL,
    created_at        DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    updated_at        DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6)
        ON UPDATE CURRENT_TIMESTAMP(6),

    CONSTRAINT chk_secretary_thread_claim_kind
        CHECK (claim_kind IN ('request', 'objection', 'confirmation')),
    CONSTRAINT chk_secretary_thread_claim_status
        CHECK (status IN ('proposed', 'contested', 'confirmed', 'withdrawn')),
    CONSTRAINT chk_secretary_thread_claim_confidence CHECK (confidence_bps <= 10000),
    CONSTRAINT fk_secretary_thread_claim_thread
        FOREIGN KEY (thread_id) REFERENCES secretary_event_threads(thread_id) ON DELETE CASCADE,
    INDEX idx_secretary_thread_claim_thread (thread_id, claim_kind, status, created_at)
) ENGINE = InnoDB
  DEFAULT CHARSET = utf8mb4
  COLLATE = utf8mb4_unicode_ci
  COMMENT = '谁提出要求、反对或确认的类型化候选；不静默提升为已确认事实';

CREATE TABLE IF NOT EXISTS secretary_thread_claim_sources
(
    claim_id        CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    source_event_id CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    PRIMARY KEY (claim_id, source_event_id),
    CONSTRAINT fk_secretary_thread_claim_source_claim
        FOREIGN KEY (claim_id) REFERENCES secretary_thread_claims(claim_id) ON DELETE CASCADE,
    CONSTRAINT fk_secretary_thread_claim_source_event
        FOREIGN KEY (source_event_id) REFERENCES secretary_source_events(source_event_id)
        ON DELETE CASCADE,
    INDEX idx_secretary_thread_claim_source_event (source_event_id)
) ENGINE = InnoDB
  DEFAULT CHARSET = utf8mb4
  COLLATE = utf8mb4_unicode_ci;

CREATE TABLE IF NOT EXISTS secretary_thread_decisions
(
    decision_id    CHAR(36) CHARACTER SET ascii COLLATE ascii_bin PRIMARY KEY,
    thread_id      CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    statement      TEXT NOT NULL,
    status         VARCHAR(16) NOT NULL DEFAULT 'proposed',
    confidence_bps SMALLINT UNSIGNED NOT NULL,
    supersedes_id  CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NULL,
    created_at     DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    updated_at     DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6)
        ON UPDATE CURRENT_TIMESTAMP(6),

    CONSTRAINT chk_secretary_thread_decision_status
        CHECK (status IN ('proposed', 'confirmed', 'superseded', 'revoked')),
    CONSTRAINT chk_secretary_thread_decision_confidence CHECK (confidence_bps <= 10000),
    CONSTRAINT fk_secretary_thread_decision_thread
        FOREIGN KEY (thread_id) REFERENCES secretary_event_threads(thread_id) ON DELETE CASCADE,
    CONSTRAINT fk_secretary_thread_decision_supersedes
        FOREIGN KEY (supersedes_id) REFERENCES secretary_thread_decisions(decision_id)
        ON DELETE RESTRICT,
    UNIQUE KEY uk_secretary_thread_decision_supersedes (supersedes_id),
    INDEX idx_secretary_thread_decision_thread (thread_id, status, created_at)
) ENGINE = InnoDB
  DEFAULT CHARSET = utf8mb4
  COLLATE = utf8mb4_unicode_ci
  COMMENT = '线程结论及显式修订链；新结论不得静默覆盖旧结论';

CREATE TABLE IF NOT EXISTS secretary_thread_decision_sources
(
    decision_id     CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    source_event_id CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    PRIMARY KEY (decision_id, source_event_id),
    CONSTRAINT fk_secretary_thread_decision_source_decision
        FOREIGN KEY (decision_id) REFERENCES secretary_thread_decisions(decision_id)
        ON DELETE CASCADE,
    CONSTRAINT fk_secretary_thread_decision_source_event
        FOREIGN KEY (source_event_id) REFERENCES secretary_source_events(source_event_id)
        ON DELETE CASCADE,
    INDEX idx_secretary_thread_decision_source_event (source_event_id)
) ENGINE = InnoDB
  DEFAULT CHARSET = utf8mb4
  COLLATE = utf8mb4_unicode_ci;

CREATE TABLE IF NOT EXISTS secretary_thread_open_questions
(
    question_id       CHAR(36) CHARACTER SET ascii COLLATE ascii_bin PRIMARY KEY,
    thread_id         CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    raised_by_channel VARCHAR(32) NOT NULL,
    raised_by_account VARCHAR(191) COLLATE utf8mb4_bin NOT NULL,
    raised_by_actor_id VARCHAR(191) COLLATE utf8mb4_bin NOT NULL,
    question          TEXT NOT NULL,
    status            VARCHAR(16) NOT NULL DEFAULT 'open',
    confidence_bps    SMALLINT UNSIGNED NOT NULL,
    created_at        DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    updated_at        DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6)
        ON UPDATE CURRENT_TIMESTAMP(6),

    CONSTRAINT chk_secretary_thread_question_status
        CHECK (status IN ('open', 'answered', 'dismissed')),
    CONSTRAINT chk_secretary_thread_question_confidence CHECK (confidence_bps <= 10000),
    CONSTRAINT fk_secretary_thread_question_thread
        FOREIGN KEY (thread_id) REFERENCES secretary_event_threads(thread_id) ON DELETE CASCADE,
    INDEX idx_secretary_thread_question_thread (thread_id, status, created_at)
) ENGINE = InnoDB
  DEFAULT CHARSET = utf8mb4
  COLLATE = utf8mb4_unicode_ci
  COMMENT = '线程未决问题及提出者；开放问题阻止线程关闭';

CREATE TABLE IF NOT EXISTS secretary_thread_question_sources
(
    question_id    CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    source_event_id CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    PRIMARY KEY (question_id, source_event_id),
    CONSTRAINT fk_secretary_thread_question_source_question
        FOREIGN KEY (question_id) REFERENCES secretary_thread_open_questions(question_id)
        ON DELETE CASCADE,
    CONSTRAINT fk_secretary_thread_question_source_event
        FOREIGN KEY (source_event_id) REFERENCES secretary_source_events(source_event_id)
        ON DELETE CASCADE,
    INDEX idx_secretary_thread_question_source_event (source_event_id)
) ENGINE = InnoDB
  DEFAULT CHARSET = utf8mb4
  COLLATE = utf8mb4_unicode_ci;

CREATE TABLE IF NOT EXISTS secretary_thread_status_history
(
    change_id       CHAR(36) CHARACTER SET ascii COLLATE ascii_bin PRIMARY KEY,
    thread_id       CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    from_status     VARCHAR(16) NOT NULL,
    to_status       VARCHAR(16) NOT NULL,
    authority       VARCHAR(24) NOT NULL,
    reason          VARCHAR(1000) NOT NULL,
    created_at      DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),

    CONSTRAINT chk_secretary_thread_history_from
        CHECK (from_status IN ('open', 'waiting', 'resolved', 'closed', 'reopened')),
    CONSTRAINT chk_secretary_thread_history_to
        CHECK (to_status IN ('open', 'waiting', 'resolved', 'closed', 'reopened')),
    CONSTRAINT chk_secretary_thread_history_authority
        CHECK (authority IN ('evidence_derived', 'owner_confirmed', 'system_recovery')),
    CONSTRAINT fk_secretary_thread_history_thread
        FOREIGN KEY (thread_id) REFERENCES secretary_event_threads(thread_id) ON DELETE CASCADE,
    INDEX idx_secretary_thread_history_thread (thread_id, created_at, change_id)
) ENGINE = InnoDB
  DEFAULT CHARSET = utf8mb4
  COLLATE = utf8mb4_unicode_ci
  COMMENT = '线程生命周期不可变审计历史';

CREATE TABLE IF NOT EXISTS secretary_thread_status_sources
(
    change_id       CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    source_event_id CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    PRIMARY KEY (change_id, source_event_id),
    CONSTRAINT fk_secretary_thread_status_source_change
        FOREIGN KEY (change_id) REFERENCES secretary_thread_status_history(change_id)
        ON DELETE CASCADE,
    CONSTRAINT fk_secretary_thread_status_source_event
        FOREIGN KEY (source_event_id) REFERENCES secretary_source_events(source_event_id)
        ON DELETE CASCADE,
    INDEX idx_secretary_thread_status_source_event (source_event_id)
) ENGINE = InnoDB
  DEFAULT CHARSET = utf8mb4
  COLLATE = utf8mb4_unicode_ci;

-- 回滚顺序（仅在确认不再需要类型化线程状态后人工执行）：
-- DROP TABLE secretary_thread_status_sources;
-- DROP TABLE secretary_thread_status_history;
-- DROP TABLE secretary_thread_question_sources;
-- DROP TABLE secretary_thread_open_questions;
-- DROP TABLE secretary_thread_decision_sources;
-- DROP TABLE secretary_thread_decisions;
-- DROP TABLE secretary_thread_claim_sources;
-- DROP TABLE secretary_thread_claims;
-- DROP TABLE secretary_thread_semantic_state;
