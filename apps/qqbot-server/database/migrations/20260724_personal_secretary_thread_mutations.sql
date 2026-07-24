-- QQ Personal Secretary owner-approved logical thread mutations.
-- Depends on ingestion, threads, and thread_links. Original thread membership is never moved.

CREATE TABLE IF NOT EXISTS secretary_thread_mutation_proposals
(
    proposal_id            CHAR(36) CHARACTER SET ascii COLLATE ascii_bin PRIMARY KEY,
    account_id             BIGINT UNSIGNED NOT NULL,
    mutation_kind          VARCHAR(16) NOT NULL,
    proposal_status        VARCHAR(24) NOT NULL DEFAULT 'awaiting_approval',
    impact_json            JSON NOT NULL,
    decision               VARCHAR(16) NULL,
    command_source_event_id CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NULL,
    effect_id              VARCHAR(255) CHARACTER SET ascii COLLATE ascii_bin NULL,
    last_error             VARCHAR(1000) NULL,
    created_at             DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    updated_at             DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6)
        ON UPDATE CURRENT_TIMESTAMP(6),
    completed_at           DATETIME(6) NULL,

    CONSTRAINT chk_secretary_thread_mutation_kind
        CHECK (mutation_kind IN ('merge', 'split')),
    CONSTRAINT chk_secretary_thread_mutation_status
        CHECK (proposal_status IN
            ('awaiting_approval', 'approved', 'rejected', 'applying', 'applied',
             'failed', 'unknown_commit')),
    CONSTRAINT chk_secretary_thread_mutation_decision
        CHECK (decision IS NULL OR decision IN ('approve', 'reject')),
    CONSTRAINT fk_secretary_thread_mutation_account
        FOREIGN KEY (account_id) REFERENCES secretary_accounts(id) ON DELETE CASCADE,
    CONSTRAINT fk_secretary_thread_mutation_command
        FOREIGN KEY (command_source_event_id) REFERENCES secretary_source_events(source_event_id)
        ON DELETE RESTRICT,
    UNIQUE KEY uk_secretary_thread_mutation_effect (effect_id),
    INDEX idx_secretary_thread_mutation_account
        (account_id, proposal_status, created_at, proposal_id)
) ENGINE = InnoDB
  DEFAULT CHARSET = utf8mb4
  COLLATE = utf8mb4_unicode_ci
  COMMENT = 'Owner 审批的线程逻辑变更 Proposal；保存有界影响快照与 Effect 幂等键';

CREATE TABLE IF NOT EXISTS secretary_thread_merge_aliases
(
    merged_thread_id    CHAR(36) CHARACTER SET ascii COLLATE ascii_bin PRIMARY KEY,
    canonical_thread_id CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    proposal_id         CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    active              BOOLEAN NOT NULL DEFAULT TRUE,
    created_at          DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    updated_at          DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6)
        ON UPDATE CURRENT_TIMESTAMP(6),

    CONSTRAINT chk_secretary_thread_merge_not_self
        CHECK (merged_thread_id <> canonical_thread_id),
    CONSTRAINT fk_secretary_thread_merge_source
        FOREIGN KEY (merged_thread_id) REFERENCES secretary_event_threads(thread_id)
        ON DELETE CASCADE,
    CONSTRAINT fk_secretary_thread_merge_canonical
        FOREIGN KEY (canonical_thread_id) REFERENCES secretary_event_threads(thread_id)
        ON DELETE CASCADE,
    CONSTRAINT fk_secretary_thread_merge_proposal
        FOREIGN KEY (proposal_id) REFERENCES secretary_thread_mutation_proposals(proposal_id)
        ON DELETE CASCADE,
    INDEX idx_secretary_thread_merge_canonical (canonical_thread_id, active),
    INDEX idx_secretary_thread_merge_proposal (proposal_id)
) ENGINE = InnoDB
  DEFAULT CHARSET = utf8mb4
  COLLATE = utf8mb4_unicode_ci
  COMMENT = '可撤销线程合并别名；不改写原始 thread_events';

CREATE TABLE IF NOT EXISTS secretary_thread_mutation_checkpoints
(
    checkpoint_id    CHAR(36) CHARACTER SET ascii COLLATE ascii_bin PRIMARY KEY,
    proposal_id      CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    checkpoint_json  JSON NOT NULL,
    checkpoint_status VARCHAR(16) NOT NULL DEFAULT 'active',
    created_at       DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    consumed_at      DATETIME(6) NULL,

    CONSTRAINT chk_secretary_thread_mutation_checkpoint_status
        CHECK (checkpoint_status IN ('active', 'consumed')),
    CONSTRAINT fk_secretary_thread_mutation_checkpoint_proposal
        FOREIGN KEY (proposal_id) REFERENCES secretary_thread_mutation_proposals(proposal_id)
        ON DELETE CASCADE,
    UNIQUE KEY uk_secretary_thread_mutation_checkpoint_proposal (proposal_id),
    INDEX idx_secretary_thread_mutation_checkpoint_status (checkpoint_status, created_at)
) ENGINE = InnoDB
  DEFAULT CHARSET = utf8mb4
  COLLATE = utf8mb4_unicode_ci
  COMMENT = '线程变更 Graph Checkpoint；恢复时 CAS 单次消费，支持进程重启';

CREATE TABLE IF NOT EXISTS secretary_thread_split_overrides
(
    source_event_id     CHAR(36) CHARACTER SET ascii COLLATE ascii_bin PRIMARY KEY,
    original_thread_id  CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    effective_thread_id CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    proposal_id         CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    active              BOOLEAN NOT NULL DEFAULT TRUE,
    created_at          DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    updated_at          DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6)
        ON UPDATE CURRENT_TIMESTAMP(6),

    CONSTRAINT chk_secretary_thread_split_changes_thread
        CHECK (original_thread_id <> effective_thread_id),
    CONSTRAINT fk_secretary_thread_split_event
        FOREIGN KEY (source_event_id) REFERENCES secretary_source_events(source_event_id)
        ON DELETE CASCADE,
    CONSTRAINT fk_secretary_thread_split_original
        FOREIGN KEY (original_thread_id) REFERENCES secretary_event_threads(thread_id)
        ON DELETE CASCADE,
    CONSTRAINT fk_secretary_thread_split_effective
        FOREIGN KEY (effective_thread_id) REFERENCES secretary_event_threads(thread_id)
        ON DELETE CASCADE,
    CONSTRAINT fk_secretary_thread_split_proposal
        FOREIGN KEY (proposal_id) REFERENCES secretary_thread_mutation_proposals(proposal_id)
        ON DELETE CASCADE,
    INDEX idx_secretary_thread_split_effective (effective_thread_id, active),
    INDEX idx_secretary_thread_split_proposal (proposal_id)
) ENGINE = InnoDB
  DEFAULT CHARSET = utf8mb4
  COLLATE = utf8mb4_unicode_ci
  COMMENT = '可撤销线程拆分覆盖；原始成员关系保持不变';

CREATE OR REPLACE VIEW secretary_effective_thread_events AS
SELECT te.source_event_id,
       COALESCE(split.effective_thread_id, alias.canonical_thread_id, te.thread_id) AS thread_id,
       te.thread_id AS projected_thread_id,
       te.added_at
FROM secretary_thread_events te
LEFT JOIN secretary_thread_split_overrides split
  ON split.source_event_id = te.source_event_id AND split.active = TRUE
LEFT JOIN secretary_thread_merge_aliases alias
  ON alias.merged_thread_id = te.thread_id AND alias.active = TRUE;

-- 回滚顺序：view -> split_overrides -> mutation_checkpoints -> merge_aliases -> mutation_proposals。
