-- QQ Personal Secretary history backfill runs, scopes, gap boundaries and reclaim schedule.
-- Forward-only migration. Depends on 20260723_personal_secretary_continuity.sql.
-- Only adds new secretary_* tables; does not modify existing tables.

CREATE TABLE IF NOT EXISTS secretary_backfill_runs
(
    backfill_run_id          CHAR(36) CHARACTER SET ascii COLLATE ascii_bin PRIMARY KEY,
    gap_id                   CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    account_id               BIGINT UNSIGNED NOT NULL,
    connection_epoch_id      CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    status                   VARCHAR(24)     NOT NULL DEFAULT 'pending',
    lease_expires_at         DATETIME(6)     NULL,
    completeness             VARCHAR(24)     NOT NULL DEFAULT 'unprovable',
    failure_class            VARCHAR(64)     NULL,
    pages_read               INT UNSIGNED    NOT NULL DEFAULT 0,
    events_read              INT UNSIGNED    NOT NULL DEFAULT 0,
    accepted                 INT UNSIGNED    NOT NULL DEFAULT 0,
    duplicates               INT UNSIGNED    NOT NULL DEFAULT 0,
    budget_exhausted         TINYINT(1)      NOT NULL DEFAULT 0,
    anomaly_count            INT UNSIGNED    NOT NULL DEFAULT 0,
    created_at               DATETIME(6)     NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    updated_at               DATETIME(6)     NOT NULL DEFAULT CURRENT_TIMESTAMP(6)
        ON UPDATE CURRENT_TIMESTAMP(6),
    completed_at             DATETIME(6)     NULL,

    CONSTRAINT chk_secretary_backfill_run_status
        CHECK (status IN ('pending', 'backfilling', 'verified_complete', 'unprovable', 'unrecoverable')),
    CONSTRAINT chk_secretary_backfill_completeness
        CHECK (completeness IN ('proven_complete', 'known_scopes_complete', 'unprovable', 'unrecoverable')),
    CONSTRAINT fk_secretary_backfill_run_gap
        FOREIGN KEY (gap_id) REFERENCES secretary_ingestion_gaps(gap_id) ON DELETE CASCADE,
    CONSTRAINT fk_secretary_backfill_run_account
        FOREIGN KEY (account_id) REFERENCES secretary_accounts(id) ON DELETE CASCADE,
    CONSTRAINT fk_secretary_backfill_run_connection
        FOREIGN KEY (connection_epoch_id)
        REFERENCES secretary_connection_epochs(connection_epoch_id) ON DELETE CASCADE,
    -- 同一个 Gap 可被多次回补（证据不足回到 uncertain 后可再次领取），因此 gap_id 不设唯一键；
    -- 当前活动运行唯一性由 Gap 状态原子转换（uncertain -> backfilling）保证。
    INDEX idx_secretary_backfill_run_gap (gap_id),
    INDEX idx_secretary_backfill_run_claim (status, lease_expires_at),
    INDEX idx_secretary_backfill_run_status (account_id, status)
) ENGINE = InnoDB
  DEFAULT CHARSET = utf8mb4
  COLLATE = utf8mb4_unicode_ci
  COMMENT = '一次历史回补运行：Gap、租约、进度、完整性证据与终态（一个 Gap 可有多条历史运行）';

-- 租约所有权令牌单独建表，使已应用过早期 backfill 草稿表的环境在重复执行本迁移时也能
-- 通过 CREATE TABLE IF NOT EXISTS 获得 fencing 能力，无需依赖 MySQL 不支持的
-- ADD COLUMN IF NOT EXISTS。
CREATE TABLE IF NOT EXISTS secretary_backfill_leases
(
    backfill_run_id    CHAR(36) CHARACTER SET ascii COLLATE ascii_bin PRIMARY KEY,
    lease_token        CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    updated_at         DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6)
        ON UPDATE CURRENT_TIMESTAMP(6),

    CONSTRAINT fk_secretary_backfill_lease_run
        FOREIGN KEY (backfill_run_id) REFERENCES secretary_backfill_runs(backfill_run_id)
        ON DELETE CASCADE
) ENGINE = InnoDB
  DEFAULT CHARSET = utf8mb4
  COLLATE = utf8mb4_unicode_ci
  COMMENT = '回补运行当前租约持有者的 fencing token；每次过期接管均轮换';

CREATE TABLE IF NOT EXISTS secretary_backfill_scopes
(
    id                       BIGINT UNSIGNED PRIMARY KEY AUTO_INCREMENT,
    backfill_run_id          CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    account_id               BIGINT UNSIGNED NOT NULL,
    conversation_id          BIGINT UNSIGNED NOT NULL,
    scope_kind               VARCHAR(24)     NOT NULL,
    scope_key                VARCHAR(255)    COLLATE utf8mb4_bin NOT NULL,
    status                   VARCHAR(24)     NOT NULL DEFAULT 'pending',
    last_anchor_message_id   VARCHAR(191)    COLLATE utf8mb4_bin NULL,
    last_anchor_message_seq  VARCHAR(64)     COLLATE utf8mb4_bin NULL,
    pages_read               INT UNSIGNED    NOT NULL DEFAULT 0,
    events_read              INT UNSIGNED    NOT NULL DEFAULT 0,
    accepted                 INT UNSIGNED    NOT NULL DEFAULT 0,
    duplicates               INT UNSIGNED    NOT NULL DEFAULT 0,
    reached_boundary         TINYINT(1)      NOT NULL DEFAULT 0,
    anomalies                JSON            NULL,
    created_at               DATETIME(6)     NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    updated_at               DATETIME(6)     NOT NULL DEFAULT CURRENT_TIMESTAMP(6)
        ON UPDATE CURRENT_TIMESTAMP(6),

    CONSTRAINT chk_secretary_backfill_scope_status
        CHECK (status IN ('pending', 'backfilling', 'verified_complete', 'unprovable', 'unrecoverable')),
    CONSTRAINT chk_secretary_backfill_scope_kind
        CHECK (scope_kind IN ('group', 'private', 'owner_control')),
    CONSTRAINT fk_secretary_backfill_scope_run
        FOREIGN KEY (backfill_run_id) REFERENCES secretary_backfill_runs(backfill_run_id)
        ON DELETE CASCADE,
    CONSTRAINT fk_secretary_backfill_scope_account
        FOREIGN KEY (account_id) REFERENCES secretary_accounts(id) ON DELETE CASCADE,
    CONSTRAINT fk_secretary_backfill_scope_conversation
        FOREIGN KEY (conversation_id) REFERENCES secretary_conversations(id) ON DELETE CASCADE,
    UNIQUE KEY uk_secretary_backfill_scope (backfill_run_id, scope_key),
    INDEX idx_secretary_backfill_scope_status (backfill_run_id, status)
) ENGINE = InnoDB
  DEFAULT CHARSET = utf8mb4
  COLLATE = utf8mb4_unicode_ci
  COMMENT = '单个会话 Scope 的回补进度与证据；锚点绑定账号视角平台消息 ID';

-- Gap 创建时对账号下所有会话游标的快照：回补边界必须是空窗开始前的固定状态，而非领取时
-- 的实时漂移游标。首写获胜（ON DUPLICATE KEY 不更新），保证捕获最早连续性中断点。
CREATE TABLE IF NOT EXISTS secretary_gap_boundaries
(
    id                              BIGINT UNSIGNED PRIMARY KEY AUTO_INCREMENT,
    gap_id                          CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    account_id                      BIGINT UNSIGNED NOT NULL,
    conversation_id                 BIGINT UNSIGNED NOT NULL,
    conversation_kind               VARCHAR(24)     NOT NULL,
    platform_conversation_id        VARCHAR(191)    COLLATE utf8mb4_bin NOT NULL,
    boundary_message_id             VARCHAR(191)    COLLATE utf8mb4_bin NOT NULL,
    boundary_occurred_at_unix_secs  BIGINT          NOT NULL,
    created_at                      DATETIME(6)     NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    updated_at                      DATETIME(6)     NOT NULL DEFAULT CURRENT_TIMESTAMP(6)
        ON UPDATE CURRENT_TIMESTAMP(6),

    CONSTRAINT chk_secretary_gap_boundary_kind
        CHECK (conversation_kind IN ('private', 'group', 'owner_control')),
    CONSTRAINT fk_secretary_gap_boundary_gap
        FOREIGN KEY (gap_id) REFERENCES secretary_ingestion_gaps(gap_id) ON DELETE CASCADE,
    CONSTRAINT fk_secretary_gap_boundary_account
        FOREIGN KEY (account_id) REFERENCES secretary_accounts(id) ON DELETE CASCADE,
    CONSTRAINT fk_secretary_gap_boundary_conversation
        FOREIGN KEY (conversation_id) REFERENCES secretary_conversations(id) ON DELETE CASCADE,
    UNIQUE KEY uk_secretary_gap_boundary (gap_id, conversation_id),
    INDEX idx_secretary_gap_boundary_gap (gap_id)
) ENGINE = InnoDB
  DEFAULT CHARSET = utf8mb4
  COLLATE = utf8mb4_unicode_ci
  COMMENT = 'Gap 创建时冻结的会话游标快照；回补边界按平台消息 ID 匹配，非领取时实时游标';

-- Gap 回到 uncertain 后的再次领取退避调度：防止对不可证 Gap 热循环回补，并避免饿死后续 Gap。
CREATE TABLE IF NOT EXISTS secretary_gap_reclaim_schedule
(
    gap_id              CHAR(36) CHARACTER SET ascii COLLATE ascii_bin PRIMARY KEY,
    next_eligible_at    DATETIME(6)     NULL,
    updated_at          DATETIME(6)     NOT NULL DEFAULT CURRENT_TIMESTAMP(6)
        ON UPDATE CURRENT_TIMESTAMP(6),

    CONSTRAINT fk_secretary_gap_reclaim_gap
        FOREIGN KEY (gap_id) REFERENCES secretary_ingestion_gaps(gap_id) ON DELETE CASCADE
) ENGINE = InnoDB
  DEFAULT CHARSET = utf8mb4
  COLLATE = utf8mb4_unicode_ci
  COMMENT = 'uncertain Gap 的再次领取退避时间；为 NULL 或已过期即立即可领取';

-- 回滚顺序（仅在确认不需要回补审计数据后人工执行）：
-- DROP TABLE secretary_gap_reclaim_schedule;
-- DROP TABLE secretary_gap_boundaries;
-- DROP TABLE secretary_backfill_scopes;
-- DROP TABLE secretary_backfill_leases;
-- DROP TABLE secretary_backfill_runs;
