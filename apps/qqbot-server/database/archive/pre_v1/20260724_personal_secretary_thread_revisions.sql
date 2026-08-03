-- QQ Personal Secretary reversible thread mutations and semantic invalidation evidence.
-- Depends on thread_mutations and thread_semantics.

CREATE TABLE IF NOT EXISTS secretary_thread_mutation_reversions
(
    reversion_id           CHAR(36) CHARACTER SET ascii COLLATE ascii_bin PRIMARY KEY,
    proposal_id            CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    command_source_event_id CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    reason                 VARCHAR(1000) NOT NULL,
    created_at             DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),

    CONSTRAINT fk_secretary_thread_reversion_proposal
        FOREIGN KEY (proposal_id) REFERENCES secretary_thread_mutation_proposals(proposal_id)
        ON DELETE CASCADE,
    CONSTRAINT fk_secretary_thread_reversion_command
        FOREIGN KEY (command_source_event_id) REFERENCES secretary_source_events(source_event_id)
        ON DELETE RESTRICT,
    UNIQUE KEY uk_secretary_thread_reversion_proposal (proposal_id),
    UNIQUE KEY uk_secretary_thread_reversion_command (command_source_event_id)
) ENGINE = InnoDB
  DEFAULT CHARSET = utf8mb4
  COLLATE = utf8mb4_unicode_ci
  COMMENT = 'Owner 对已应用线程变更的不可变撤销审计；逻辑覆盖停用但原始成员不变';

CREATE TABLE IF NOT EXISTS secretary_thread_semantic_invalidations
(
    invalidation_id   CHAR(36) CHARACTER SET ascii COLLATE ascii_bin PRIMARY KEY,
    proposal_id      CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    thread_id        CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    invalidation_kind VARCHAR(24) NOT NULL,
    created_at       DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),

    CONSTRAINT chk_secretary_thread_semantic_invalidation_kind
        CHECK (invalidation_kind IN ('mutation_applied', 'mutation_reverted')),
    CONSTRAINT fk_secretary_thread_invalidation_proposal
        FOREIGN KEY (proposal_id) REFERENCES secretary_thread_mutation_proposals(proposal_id)
        ON DELETE CASCADE,
    CONSTRAINT fk_secretary_thread_invalidation_thread
        FOREIGN KEY (thread_id) REFERENCES secretary_event_threads(thread_id)
        ON DELETE CASCADE,
    UNIQUE KEY uk_secretary_thread_semantic_invalidation
        (proposal_id, thread_id, invalidation_kind),
    INDEX idx_secretary_thread_semantic_invalidation_thread
        (thread_id, created_at, invalidation_id)
) ENGINE = InnoDB
  DEFAULT CHARSET = utf8mb4
  COLLATE = utf8mb4_unicode_ci
  COMMENT = '线程变更后的语义失效证据；旧派生事实保留审计但不再作为当前状态读取';

-- 回滚顺序：semantic_invalidations -> mutation_reversions。
