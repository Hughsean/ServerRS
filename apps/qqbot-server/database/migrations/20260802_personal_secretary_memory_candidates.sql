-- Structured memory candidate production and Owner approval.
-- Candidates are proposed memory drafts; Owner approval writes a Confirmed
-- MemoryFact (and, for commitments, flips the commitment to Pending so the
-- existing FollowUp scan chain can pick it up).

-- 1) 候选主表：proposed -> approved / rejected / invalidated，版本精确 +1。
--    deterministic_fingerprint 由 account + kind + subject + payload + 排序来源
--    + extractor_version 稳定派生；同账号同 fingerprint 只能存在一个候选
--    （重复扫描 / Worker 重启 / 模型重复输出靠该唯一键去重）。
CREATE TABLE IF NOT EXISTS secretary_memory_candidates
(
    candidate_id              CHAR(36) CHARACTER SET ascii COLLATE ascii_bin PRIMARY KEY,
    account_id                BIGINT UNSIGNED NOT NULL,
    candidate_kind            VARCHAR(16) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    subject_key               VARCHAR(191) NOT NULL,
    payload_json              JSON NOT NULL,
    candidate_status          VARCHAR(16) CHARACTER SET ascii COLLATE ascii_bin NOT NULL
                                  DEFAULT 'proposed',
    candidate_version         BIGINT UNSIGNED NOT NULL DEFAULT 1,
    extractor_version         VARCHAR(32) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    deterministic_fingerprint CHAR(64) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    created_at                DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    updated_at                DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6)
                                  ON UPDATE CURRENT_TIMESTAMP(6),

    CONSTRAINT chk_secretary_memory_candidate_kind
        CHECK (candidate_kind IN ('person', 'project', 'commitment')),
    CONSTRAINT chk_secretary_memory_candidate_status
        CHECK (candidate_status IN ('proposed', 'approved', 'rejected', 'invalidated')),
    CONSTRAINT chk_secretary_memory_candidate_version
        CHECK (candidate_version > 0),
    CONSTRAINT fk_secretary_memory_candidate_account
        FOREIGN KEY (account_id) REFERENCES secretary_accounts(id) ON DELETE CASCADE,
    UNIQUE KEY uk_secretary_memory_candidate_fingerprint
        (account_id, deterministic_fingerprint),
    INDEX idx_secretary_memory_candidate_status (account_id, candidate_status, updated_at)
) ENGINE = InnoDB DEFAULT CHARSET = utf8mb4 COLLATE = utf8mb4_unicode_ci
  COMMENT = '结构化记忆候选（proposed 待 Owner 审批；fingerprint 唯一去重）';

-- 2) 候选精确来源：来源事件 + 发送者 + 时间 + 当时的内容信任级别。
--    批准时按该表复验：来源必须仍属本账号、未被撤回、允许长期记忆。
CREATE TABLE IF NOT EXISTS secretary_memory_candidate_sources
(
    candidate_id          CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    source_event_id       CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    account_id            BIGINT UNSIGNED NOT NULL,
    actor_platform_id     VARCHAR(191) COLLATE utf8mb4_bin NOT NULL,
    content_trust_level   VARCHAR(16) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    occurred_at_unix_secs BIGINT NOT NULL,
    created_at            DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),

    PRIMARY KEY (candidate_id, source_event_id),
    CONSTRAINT chk_secretary_candidate_source_trust
        CHECK (content_trust_level IN ('normal', 'local_only', 'envelope_only', 'never_long_term')),
    CONSTRAINT fk_secretary_candidate_source_candidate
        FOREIGN KEY (candidate_id) REFERENCES secretary_memory_candidates(candidate_id)
        ON DELETE CASCADE,
    CONSTRAINT fk_secretary_candidate_source_event
        FOREIGN KEY (source_event_id) REFERENCES secretary_source_events(source_event_id)
        ON DELETE CASCADE,
    CONSTRAINT fk_secretary_candidate_source_account
        FOREIGN KEY (account_id) REFERENCES secretary_accounts(id) ON DELETE CASCADE,
    INDEX idx_secretary_candidate_source_event (source_event_id)
) ENGINE = InnoDB DEFAULT CHARSET = utf8mb4 COLLATE = utf8mb4_unicode_ci
  COMMENT = '记忆候选的精确来源；批准与失效均按此表复验';

-- 3) 提取处理状态：每账号一行，含持久化游标（received_at + 事件 ID）、
--    租约 token/到期与 fencing（lease_token 精确匹配）。
CREATE TABLE IF NOT EXISTS secretary_memory_candidate_processing_state
(
    account_id           BIGINT UNSIGNED PRIMARY KEY,
    last_received_at     DATETIME(6) NULL,
    last_source_event_id CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NULL,
    lease_token          CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NULL,
    lease_expires_at     DATETIME(6) NULL,
    attempts             INT UNSIGNED NOT NULL DEFAULT 0,
    last_error           VARCHAR(512) NULL,
    updated_at           DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6)
                             ON UPDATE CURRENT_TIMESTAMP(6),

    CONSTRAINT fk_secretary_candidate_state_account
        FOREIGN KEY (account_id) REFERENCES secretary_accounts(id) ON DELETE CASCADE,
    INDEX idx_secretary_candidate_state_lease (lease_token)
) ENGINE = InnoDB DEFAULT CHARSET = utf8mb4 COLLATE = utf8mb4_unicode_ci
  COMMENT = '记忆候选提取的持久化游标与租约（崩溃后从游标恢复）';

-- 4) Owner 审批的不可变审计：每次批准/拒绝/冲突检测一条。
--    approve_conflict 是确定性业务结果而非失败：候选保持 proposed、版本不变，
--    审计记录旧 fact_id 与 candidate_id，Owner 凭 Receipt 的冲突说明决定后续动作；
--    fact_id 在 approve / approve_conflict 时引用；reject 不创建
--    MemoryFact/FollowUp/Outbox，fact_id 保持 NULL。
CREATE TABLE IF NOT EXISTS secretary_memory_candidate_controls
(
    control_id                  CHAR(36) CHARACTER SET ascii COLLATE ascii_bin PRIMARY KEY,
    effect_id                   VARCHAR(255) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    run_id                      CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    proposal_id                 CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    account_id                  BIGINT UNSIGNED NOT NULL,
    candidate_id                CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    control_kind                VARCHAR(16) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    previous_status             VARCHAR(16) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    current_status              VARCHAR(16) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    previous_candidate_version  BIGINT UNSIGNED NOT NULL,
    current_candidate_version   BIGINT UNSIGNED NOT NULL,
    fact_id                     CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NULL,
    command_source_event_id     CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    reason                      VARCHAR(1000) NOT NULL,
    created_at                  DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),

    CONSTRAINT chk_secretary_candidate_control_kind
        CHECK (control_kind IN ('approve', 'approve_conflict', 'reject')),
    CONSTRAINT chk_secretary_candidate_control_status
        CHECK ((control_kind = 'approve' AND previous_status IN ('proposed')
                AND current_status IN ('approved'))
               OR (control_kind = 'reject' AND previous_status IN ('proposed')
                   AND current_status IN ('rejected'))
               OR (control_kind = 'approve_conflict' AND previous_status IN ('proposed')
                   AND current_status IN ('proposed'))),
    CONSTRAINT chk_secretary_candidate_control_version
        CHECK (previous_candidate_version > 0
               AND ((control_kind = 'approve_conflict'
                     AND current_candidate_version = previous_candidate_version)
                    OR (control_kind IN ('approve', 'reject')
                        AND current_candidate_version = previous_candidate_version + 1))),
    CONSTRAINT fk_secretary_candidate_control_run
        FOREIGN KEY (run_id) REFERENCES secretary_action_runs(run_id) ON DELETE RESTRICT,
    CONSTRAINT fk_secretary_candidate_control_account
        FOREIGN KEY (account_id) REFERENCES secretary_accounts(id) ON DELETE CASCADE,
    CONSTRAINT fk_secretary_candidate_control_candidate
        FOREIGN KEY (candidate_id) REFERENCES secretary_memory_candidates(candidate_id)
        ON DELETE RESTRICT,
    CONSTRAINT fk_secretary_candidate_control_fact
        FOREIGN KEY (fact_id) REFERENCES secretary_memory_facts(fact_id) ON DELETE RESTRICT,
    CONSTRAINT fk_secretary_candidate_control_command
        FOREIGN KEY (command_source_event_id) REFERENCES secretary_source_events(source_event_id)
        ON DELETE RESTRICT,
    UNIQUE KEY uk_secretary_candidate_control_effect (effect_id, candidate_id),
    INDEX idx_secretary_candidate_control_candidate (account_id, candidate_id, created_at)
) ENGINE = InnoDB DEFAULT CHARSET = utf8mb4 COLLATE = utf8mb4_unicode_ci
  COMMENT = 'Owner 对记忆候选的不可变 Effect 审计；版本精确递增，供并发 fencing 复盘';

-- 5) 被过滤事件的延期状态：远程模式（allow_local_only=false）领取批次时，
--    把批次范围内被过滤的 local_only 事件 INSERT IGNORE 到这里；本地模式
--    （已验证 loopback 端点）领取时优先消费延期事件，避免账号全局游标推进后
--    较早的 local_only 事件永久不可达。延期消费提交时删除对应行；撤回或降级
--    为 envelope_only/never_long_term 的延期行在消费前清理。行内 received_at
--    保证按全局顺序消费。
CREATE TABLE IF NOT EXISTS secretary_memory_candidate_deferred
(
    account_id          BIGINT UNSIGNED NOT NULL,
    source_event_id     CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    received_at         DATETIME(6) NOT NULL,
    created_at          DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),

    PRIMARY KEY (account_id, source_event_id),
    CONSTRAINT fk_secretary_candidate_deferred_account
        FOREIGN KEY (account_id) REFERENCES secretary_accounts(id) ON DELETE CASCADE,
    CONSTRAINT fk_secretary_candidate_deferred_event
        FOREIGN KEY (source_event_id) REFERENCES secretary_source_events(source_event_id)
        ON DELETE CASCADE,
    INDEX idx_secretary_candidate_deferred_order (account_id, received_at, source_event_id)
) ENGINE = InnoDB DEFAULT CHARSET = utf8mb4 COLLATE = utf8mb4_unicode_ci
  COMMENT = '远程模式被过滤的 local_only 事件；切换本地模型后先于主游标消费';

-- 回滚（仅在确认不需要新业务数据后人工执行）：
-- DROP TABLE secretary_memory_candidate_deferred;
-- DROP TABLE secretary_memory_candidate_controls;
-- DROP TABLE secretary_memory_candidate_processing_state;
-- DROP TABLE secretary_memory_candidate_sources;
-- DROP TABLE secretary_memory_candidates;
