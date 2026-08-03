-- Action Planner 第一批迁移：运行、Checkpoint、Effect 幂等、响应草稿、审计。
-- 约束 3：CAS 领取 + lease fencing + INSERT IGNORE 幂等创建。
-- 回滚顺序：audit -> responses -> effect_receipts -> checkpoints -> runs。

-- 1. action_runs：每个 OwnerCommand 对应一个运行，CAS 领取与 lease fencing 的载体。
CREATE TABLE IF NOT EXISTS secretary_action_runs
(
    run_id                  CHAR(36) CHARACTER SET ascii COLLATE ascii_bin PRIMARY KEY,
    account_id              BIGINT UNSIGNED NOT NULL,
    command_source_event_id CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    command_text            VARCHAR(4000) NOT NULL,
    conversation_id         VARCHAR(191) COLLATE utf8mb4_bin NOT NULL,
    occurred_at_unix_secs   BIGINT NOT NULL,
    timezone_offset_secs    BIGINT NOT NULL,
    recent_events_json      JSON NOT NULL,
    -- planner_version 用于幂等键：同一命令在相同 Planner 版本下只创建一个运行。
    -- 版本升级后允许重新规划，但仍受业务幂等约束。
    planner_version         VARCHAR(32) NOT NULL DEFAULT 'v1',
    status                  VARCHAR(16) NOT NULL DEFAULT 'pending',
    worker_id               VARCHAR(191) NULL,
    lease_token             CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NULL,
    lease_expires_at        DATETIME(6) NULL,
    next_eligible_at        DATETIME(6) NULL,
    attempt                 INT UNSIGNED NOT NULL DEFAULT 0,
    last_error              VARCHAR(1000) NULL,
    last_checkpoint_json    JSON NULL,
    response_draft_json     JSON NULL,
    created_at              DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    updated_at              DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6)
        ON UPDATE CURRENT_TIMESTAMP(6),
    completed_at            DATETIME(6) NULL,

    CONSTRAINT chk_secretary_action_run_status
        CHECK (status IN ('pending', 'running', 'suspended', 'completed', 'failed')),
    CONSTRAINT fk_secretary_action_run_account
        FOREIGN KEY (account_id) REFERENCES secretary_accounts(id) ON DELETE CASCADE,
    CONSTRAINT fk_secretary_action_run_command
        FOREIGN KEY (command_source_event_id) REFERENCES secretary_source_events(source_event_id)
        ON DELETE RESTRICT,
    -- P0 修复：业务幂等键。同一账号同一命令在同一 planner_version 下只创建一个运行。
    -- run_id 虽为主键但由 source_event_id 派生，此唯一键是业务层幂等的真正保障。
    UNIQUE KEY uk_secretary_action_run_command (account_id, command_source_event_id, planner_version),
    UNIQUE KEY uk_secretary_action_run_lease (lease_token),
    INDEX idx_secretary_action_claim (status, next_eligible_at, created_at),
    INDEX idx_secretary_action_account (account_id, status, created_at),
    INDEX idx_secretary_action_lease_expiry (status, lease_expires_at)
) ENGINE = InnoDB
  DEFAULT CHARSET = utf8mb4
  COLLATE = utf8mb4_unicode_ci
  COMMENT = 'Action Planner 运行；CAS 领取 + lease fencing + 业务幂等创建';

-- 2. action_checkpoints：Graph Checkpoint 完整持久化（CAS 单次消费）。
-- 保存完整 AgentCheckpoint JSON，checkpoint_status 实现 CAS 单次消费语义。
CREATE TABLE IF NOT EXISTS secretary_action_checkpoints
(
    checkpoint_id      CHAR(36) CHARACTER SET ascii COLLATE ascii_bin PRIMARY KEY,
    run_id             CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    checkpoint_json    JSON NOT NULL,
    checkpoint_status  VARCHAR(16) NOT NULL DEFAULT 'active',
    created_at         DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    consumed_at        DATETIME(6) NULL,

    CONSTRAINT chk_secretary_action_checkpoint_status
        CHECK (checkpoint_status IN ('active', 'consumed')),
    CONSTRAINT fk_secretary_action_checkpoint_run
        FOREIGN KEY (run_id) REFERENCES secretary_action_runs(run_id) ON DELETE CASCADE,
    INDEX idx_secretary_action_checkpoint_run (run_id, created_at),
    INDEX idx_secretary_action_checkpoint_status (checkpoint_status, created_at)
) ENGINE = InnoDB
  DEFAULT CHARSET = utf8mb4
  COLLATE = utf8mb4_unicode_ci
  COMMENT = 'Action Graph 完整 Checkpoint；恢复时 CAS 单次消费（checkpoint_status active->consumed），支持进程重启';

-- 3. action_effect_receipts：Effect 幂等记录，用 effect_id 去重。
CREATE TABLE IF NOT EXISTS secretary_action_effect_receipts
(
    effect_id    VARCHAR(255) CHARACTER SET ascii COLLATE ascii_bin PRIMARY KEY,
    run_id       CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    proposal_json JSON NOT NULL,
    result_ref   VARCHAR(1000) NOT NULL,
    created_at   DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),

    CONSTRAINT fk_secretary_action_effect_run
        FOREIGN KEY (run_id) REFERENCES secretary_action_runs(run_id) ON DELETE CASCADE,
    INDEX idx_secretary_action_effect_run (run_id, created_at)
) ENGINE = InnoDB
  DEFAULT CHARSET = utf8mb4
  COLLATE = utf8mb4_unicode_ci
  COMMENT = 'Action Effect 幂等回执；effect_id 全局唯一，INSERT IGNORE 去重';

-- 4. action_responses：Owner 响应草稿持久化（有界摘录，约束 7）。
CREATE TABLE IF NOT EXISTS secretary_action_responses
(
    response_id       CHAR(36) CHARACTER SET ascii COLLATE ascii_bin PRIMARY KEY,
    run_id            CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    response_json     JSON NOT NULL,
    serialized_bytes  INT UNSIGNED NOT NULL,
    invalidated       BOOLEAN NOT NULL DEFAULT FALSE,
    created_at        DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),

    CONSTRAINT fk_secretary_action_response_run
        FOREIGN KEY (run_id) REFERENCES secretary_action_runs(run_id) ON DELETE CASCADE,
    CONSTRAINT chk_secretary_action_response_bytes
        CHECK (serialized_bytes <= 65536),
    UNIQUE KEY uk_secretary_action_response_run (run_id)
) ENGINE = InnoDB
  DEFAULT CHARSET = utf8mb4
  COLLATE = utf8mb4_unicode_ci
  COMMENT = 'Owner 响应草稿；64KB 限制由应用层验证，来源失效时标记 invalidated';

-- 5. action_audit：审计事件，不可变追加日志。
CREATE TABLE IF NOT EXISTS secretary_action_audit
(
    audit_id     CHAR(36) CHARACTER SET ascii COLLATE ascii_bin PRIMARY KEY,
    run_id       CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    event_kind   VARCHAR(32) NOT NULL,
    detail_json  JSON NOT NULL,
    created_at   DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),

    CONSTRAINT chk_secretary_action_audit_kind
        CHECK (event_kind IN ('created', 'claimed', 'suspended', 'resumed',
                              'effect_applied', 'completed', 'failed', 'released')),
    CONSTRAINT fk_secretary_action_audit_run
        FOREIGN KEY (run_id) REFERENCES secretary_action_runs(run_id) ON DELETE CASCADE,
    INDEX idx_secretary_action_audit_run (run_id, created_at)
) ENGINE = InnoDB
  DEFAULT CHARSET = utf8mb4
  COLLATE = utf8mb4_unicode_ci
  COMMENT = 'Action 运行审计；不可变追加，用于排查与合规';

-- 回滚顺序（子表先于父表）：
-- DROP TABLE IF EXISTS secretary_action_audit;
-- DROP TABLE IF EXISTS secretary_action_responses;
-- DROP TABLE IF EXISTS secretary_action_effect_receipts;
-- DROP TABLE IF EXISTS secretary_action_checkpoints;
-- DROP TABLE IF EXISTS secretary_action_runs;
