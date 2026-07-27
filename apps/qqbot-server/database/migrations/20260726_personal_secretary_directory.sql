-- B4 账号会话目录与历史完整性证据：目录快照、Scope 条目和 Gap 冻结。
-- 约束：账号作用域、幂等、跨重启恢复、不跨账号合并平台 ID。
-- 三个列表接口全部成功不等于账号历史完整；真实 NapCat 只能到达 known_scopes_complete。
-- Forward-only migration. Depends on 20260723_personal_secretary_continuity.sql (gap_id FK).
-- 回滚顺序：gap_freeze -> scopes -> snapshots。

-- 1. secretary_directory_snapshots：每次目录同步的聚合快照。
CREATE TABLE IF NOT EXISTS secretary_directory_snapshots
(
    snapshot_id           CHAR(36) CHARACTER SET ascii COLLATE ascii_bin PRIMARY KEY,
    account_id            BIGINT UNSIGNED NOT NULL,
    source_api            VARCHAR(32)     NOT NULL,
    status                VARCHAR(32)     NOT NULL DEFAULT 'uncertain',
    evidence_json         JSON NOT NULL,
    scope_count           INT UNSIGNED    NOT NULL DEFAULT 0,
    created_at_unix_secs  BIGINT          NOT NULL,
    created_at            DATETIME(6)     NOT NULL DEFAULT CURRENT_TIMESTAMP(6),

    CONSTRAINT chk_secretary_directory_snapshot_source
        CHECK (source_api IN ('friend_group_recent', 'recent_contact', 'observed_from_history')),
    CONSTRAINT chk_secretary_directory_snapshot_status
        CHECK (status IN (
            'known_scopes_complete', 'verified_complete', 'uncertain',
            'unavailable', 'api_timeout', 'api_oversized', 'api_deferred'
        )),
    CONSTRAINT fk_secretary_directory_snapshot_account
        FOREIGN KEY (account_id) REFERENCES secretary_accounts(id) ON DELETE CASCADE,
    -- 同一账号的快照按时间索引，便于读取最新。
    INDEX idx_secretary_directory_snapshot_account_time (account_id, created_at_unix_secs)
) ENGINE = InnoDB
  DEFAULT CHARSET = utf8mb4
  COLLATE = utf8mb4_unicode_ci
  COMMENT = '账号会话目录快照：来源 API、状态、证据与 Scope 数量（账号作用域、幂等）';

-- 2. secretary_directory_scopes：每次快照内的会话 Scope 条目。
-- 平台 ID 以字符串保留精度，不经浮点数转换。禁止对 message ID 做数值加减。
CREATE TABLE IF NOT EXISTS secretary_directory_scopes
(
    id                          BIGINT UNSIGNED PRIMARY KEY AUTO_INCREMENT,
    snapshot_id                 CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    account_id                  BIGINT UNSIGNED NOT NULL,
    scope_kind                  VARCHAR(32)     NOT NULL,
    conversation_kind           VARCHAR(32)     NOT NULL,
    platform_conversation_id    VARCHAR(191)    COLLATE utf8mb4_bin NOT NULL,
    boundary_message_id         VARCHAR(191)    COLLATE utf8mb4_bin NULL,
    boundary_msg_time           VARCHAR(64)     COLLATE utf8mb4_bin NULL,
    display_name                VARCHAR(500)    NULL,
    created_at                  DATETIME(6)     NOT NULL DEFAULT CURRENT_TIMESTAMP(6),

    CONSTRAINT chk_secretary_directory_scope_kind
        CHECK (scope_kind IN (
            'friend', 'group', 'recent_unconfirmed', 'deleted', 'exited', 'inaccessible'
        )),
    CONSTRAINT chk_secretary_directory_scope_conv_kind
        CHECK (conversation_kind IN ('private', 'group', 'owner_control')),
    CONSTRAINT fk_secretary_directory_scope_snapshot
        FOREIGN KEY (snapshot_id) REFERENCES secretary_directory_snapshots(snapshot_id)
        ON DELETE CASCADE,
    CONSTRAINT fk_secretary_directory_scope_account
        FOREIGN KEY (account_id) REFERENCES secretary_accounts(id) ON DELETE CASCADE,
    -- 同一快照内同一 (kind, conversation_id) 不得重复。
    UNIQUE KEY uk_secretary_directory_scope (snapshot_id, scope_kind, platform_conversation_id),
    INDEX idx_secretary_directory_scope_account (account_id, scope_kind)
) ENGINE = InnoDB
  DEFAULT CHARSET = utf8mb4
  COLLATE = utf8mb4_unicode_ci
  COMMENT = '目录快照内的会话 Scope 条目：类别、边界与显示名（平台 ID 字符串保留精度）';

-- 3. secretary_directory_gap_freeze：Gap 创建时冻结的目录快照引用。
-- 回补过程读此冻结快照而非实时 Cursor，不跟随漂移（任务六-7）。
CREATE TABLE IF NOT EXISTS secretary_directory_gap_freeze
(
    gap_id          CHAR(36) CHARACTER SET ascii COLLATE ascii_bin PRIMARY KEY,
    snapshot_id     CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NULL,
    account_id      BIGINT UNSIGNED NOT NULL,
    frozen_at       DATETIME(6)     NOT NULL DEFAULT CURRENT_TIMESTAMP(6),

    CONSTRAINT fk_secretary_directory_freeze_gap
        FOREIGN KEY (gap_id) REFERENCES secretary_ingestion_gaps(gap_id) ON DELETE CASCADE,
    CONSTRAINT fk_secretary_directory_freeze_snapshot
        FOREIGN KEY (snapshot_id) REFERENCES secretary_directory_snapshots(snapshot_id)
        ON DELETE RESTRICT,
    CONSTRAINT fk_secretary_directory_freeze_account
        FOREIGN KEY (account_id) REFERENCES secretary_accounts(id) ON DELETE CASCADE,
    INDEX idx_secretary_directory_freeze_account (account_id)
) ENGINE = InnoDB
  DEFAULT CHARSET = utf8mb4
  COLLATE = utf8mb4_unicode_ci
  COMMENT = 'Gap 创建时冻结的目录快照引用；回补读此快照而非实时 Cursor';
