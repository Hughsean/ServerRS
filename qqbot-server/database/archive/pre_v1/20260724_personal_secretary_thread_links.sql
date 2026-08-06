-- QQ Personal Secretary cross-conversation thread-link candidates.
-- Depends on 20260724_personal_secretary_threads.sql. Never merges thread membership automatically.

CREATE TABLE IF NOT EXISTS secretary_owner_bindings
(
    binding_id CHAR(36) CHARACTER SET ascii COLLATE ascii_bin PRIMARY KEY,
    managed_account_id BIGINT UNSIGNED NOT NULL,
    command_account_id BIGINT UNSIGNED NOT NULL,
    owner_actor_id VARCHAR(191) COLLATE utf8mb4_bin NOT NULL,
    status VARCHAR(16) NOT NULL DEFAULT 'active',
    created_at DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    updated_at DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6)
        ON UPDATE CURRENT_TIMESTAMP(6),
    CONSTRAINT chk_secretary_owner_binding_status CHECK (status IN ('active', 'revoked')),
    CONSTRAINT fk_secretary_owner_binding_managed_account
        FOREIGN KEY (managed_account_id) REFERENCES secretary_accounts(id) ON DELETE CASCADE,
    CONSTRAINT fk_secretary_owner_binding_command_account
        FOREIGN KEY (command_account_id) REFERENCES secretary_accounts(id) ON DELETE CASCADE,
    UNIQUE KEY uk_secretary_owner_binding_managed
        (managed_account_id, command_account_id, owner_actor_id),
    INDEX idx_secretary_owner_binding_command
        (command_account_id, owner_actor_id, status)
) ENGINE = InnoDB DEFAULT CHARSET = utf8mb4 COLLATE = utf8mb4_unicode_ci
  COMMENT = '本地配置建立的 Owner 控制身份到被管理账号的显式授权；不从聊天内容推断';

CREATE TABLE IF NOT EXISTS secretary_thread_link_scan_state
(
    source_event_id CHAR(36) CHARACTER SET ascii COLLATE ascii_bin PRIMARY KEY,
    lease_token CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NULL,
    lease_expires_at DATETIME(6) NULL,
    attempts INT UNSIGNED NOT NULL DEFAULT 0,
    last_error VARCHAR(512) NULL,
    completed_at DATETIME(6) NULL,
    updated_at DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6)
        ON UPDATE CURRENT_TIMESTAMP(6),
    CONSTRAINT fk_secretary_thread_link_scan_event
        FOREIGN KEY (source_event_id) REFERENCES secretary_source_events(source_event_id)
        ON DELETE CASCADE,
    INDEX idx_secretary_thread_link_scan_claim
        (completed_at, lease_expires_at, updated_at, source_event_id),
    INDEX idx_secretary_thread_link_scan_token (lease_token)
) ENGINE = InnoDB DEFAULT CHARSET = utf8mb4 COLLATE = utf8mb4_unicode_ci
  COMMENT = '跨会话关联候选消费者的独立租约；完成后不重复扫描事件';

CREATE TABLE IF NOT EXISTS secretary_thread_link_hints
(
    hint_id CHAR(36) CHARACTER SET ascii COLLATE ascii_bin PRIMARY KEY,
    account_id BIGINT UNSIGNED NOT NULL,
    conversation_id BIGINT UNSIGNED NOT NULL,
    thread_id CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    source_event_id CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    signal_kind VARCHAR(32) NOT NULL,
    fingerprint_sha256 CHAR(64) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    created_at DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    CONSTRAINT chk_secretary_thread_link_hint_kind
        CHECK (signal_kind IN ('explicit_project_id', 'exact_file_source_key')),
    CONSTRAINT fk_secretary_thread_link_hint_account
        FOREIGN KEY (account_id) REFERENCES secretary_accounts(id) ON DELETE CASCADE,
    CONSTRAINT fk_secretary_thread_link_hint_conversation
        FOREIGN KEY (conversation_id) REFERENCES secretary_conversations(id) ON DELETE CASCADE,
    CONSTRAINT fk_secretary_thread_link_hint_thread
        FOREIGN KEY (thread_id) REFERENCES secretary_event_threads(thread_id) ON DELETE CASCADE,
    CONSTRAINT fk_secretary_thread_link_hint_event
        FOREIGN KEY (source_event_id) REFERENCES secretary_source_events(source_event_id)
        ON DELETE CASCADE,
    UNIQUE KEY uk_secretary_thread_link_hint
        (source_event_id, signal_kind, fingerprint_sha256),
    INDEX idx_secretary_thread_link_hint_match
        (account_id, signal_kind, fingerprint_sha256, thread_id, conversation_id)
) ENGINE = InnoDB DEFAULT CHARSET = utf8mb4 COLLATE = utf8mb4_unicode_ci
  COMMENT = '强关联信号的不可逆指纹；不保存项目编号、文件源键或文件名明文';

CREATE TABLE IF NOT EXISTS secretary_thread_link_candidates
(
    candidate_id CHAR(36) CHARACTER SET ascii COLLATE ascii_bin PRIMARY KEY,
    account_id BIGINT UNSIGNED NOT NULL,
    left_thread_id CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    right_thread_id CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    left_conversation_id BIGINT UNSIGNED NOT NULL,
    right_conversation_id BIGINT UNSIGNED NOT NULL,
    signal_kind VARCHAR(32) NOT NULL,
    fingerprint_sha256 CHAR(64) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    status VARCHAR(16) NOT NULL DEFAULT 'proposed',
    confidence_bps SMALLINT UNSIGNED NOT NULL,
    reason_code VARCHAR(64) NOT NULL,
    created_at DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    updated_at DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6)
        ON UPDATE CURRENT_TIMESTAMP(6),
    CONSTRAINT chk_secretary_thread_link_candidate_kind
        CHECK (signal_kind IN ('explicit_project_id', 'exact_file_source_key')),
    CONSTRAINT chk_secretary_thread_link_candidate_status
        CHECK (status IN ('proposed', 'accepted', 'rejected', 'expired')),
    CONSTRAINT chk_secretary_thread_link_candidate_confidence CHECK (confidence_bps <= 10000),
    CONSTRAINT chk_secretary_thread_link_distinct_threads CHECK (left_thread_id < right_thread_id),
    CONSTRAINT chk_secretary_thread_link_distinct_conversations
        CHECK (left_conversation_id <> right_conversation_id),
    CONSTRAINT fk_secretary_thread_link_candidate_account
        FOREIGN KEY (account_id) REFERENCES secretary_accounts(id) ON DELETE CASCADE,
    CONSTRAINT fk_secretary_thread_link_candidate_left_thread
        FOREIGN KEY (left_thread_id) REFERENCES secretary_event_threads(thread_id) ON DELETE CASCADE,
    CONSTRAINT fk_secretary_thread_link_candidate_right_thread
        FOREIGN KEY (right_thread_id) REFERENCES secretary_event_threads(thread_id) ON DELETE CASCADE,
    CONSTRAINT fk_secretary_thread_link_candidate_left_conversation
        FOREIGN KEY (left_conversation_id) REFERENCES secretary_conversations(id) ON DELETE CASCADE,
    CONSTRAINT fk_secretary_thread_link_candidate_right_conversation
        FOREIGN KEY (right_conversation_id) REFERENCES secretary_conversations(id) ON DELETE CASCADE,
    UNIQUE KEY uk_secretary_thread_link_candidate
        (account_id, left_thread_id, right_thread_id, signal_kind, fingerprint_sha256),
    INDEX idx_secretary_thread_link_candidate_status
        (account_id, status, confidence_bps, updated_at)
) ENGINE = InnoDB DEFAULT CHARSET = utf8mb4 COLLATE = utf8mb4_unicode_ci
  COMMENT = '跨群/私聊线程关联候选；只有 Owner 控制面可在后续采用，绝不自动合并';

CREATE TABLE IF NOT EXISTS secretary_thread_link_candidate_sources
(
    candidate_id CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    source_event_id CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    PRIMARY KEY (candidate_id, source_event_id),
    CONSTRAINT fk_secretary_thread_link_source_candidate
        FOREIGN KEY (candidate_id) REFERENCES secretary_thread_link_candidates(candidate_id)
        ON DELETE CASCADE,
    CONSTRAINT fk_secretary_thread_link_source_event
        FOREIGN KEY (source_event_id) REFERENCES secretary_source_events(source_event_id)
        ON DELETE CASCADE,
    INDEX idx_secretary_thread_link_source_event (source_event_id)
) ENGINE = InnoDB DEFAULT CHARSET = utf8mb4 COLLATE = utf8mb4_unicode_ci;

CREATE TABLE IF NOT EXISTS secretary_thread_link_reviews
(
    review_id CHAR(36) CHARACTER SET ascii COLLATE ascii_bin PRIMARY KEY,
    candidate_id CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    review_action VARCHAR(16) NOT NULL,
    owner_channel VARCHAR(32) NOT NULL,
    owner_account VARCHAR(191) COLLATE utf8mb4_bin NOT NULL,
    owner_actor_id VARCHAR(191) COLLATE utf8mb4_bin NOT NULL,
    command_source_event_id CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    created_at DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    CONSTRAINT chk_secretary_thread_link_review_action
        CHECK (review_action IN ('accept', 'reject')),
    CONSTRAINT chk_secretary_thread_link_review_channel
        CHECK (owner_channel IN ('napcat', 'qq_open_platform')),
    CONSTRAINT fk_secretary_thread_link_review_candidate
        FOREIGN KEY (candidate_id) REFERENCES secretary_thread_link_candidates(candidate_id)
        ON DELETE CASCADE,
    CONSTRAINT fk_secretary_thread_link_review_command
        FOREIGN KEY (command_source_event_id) REFERENCES secretary_source_events(source_event_id)
        ON DELETE RESTRICT,
    UNIQUE KEY uk_secretary_thread_link_review_candidate (candidate_id),
    UNIQUE KEY uk_secretary_thread_link_review_command (command_source_event_id),
    INDEX idx_secretary_thread_link_review_owner
        (owner_channel, owner_account, created_at)
) ENGINE = InnoDB DEFAULT CHARSET = utf8mb4 COLLATE = utf8mb4_unicode_ci
  COMMENT = 'Owner 对关联候选的不可变审核；命令事件、身份和动作均可追溯';

-- 回滚顺序：reviews -> candidate_sources -> candidates -> hints -> scan_state -> owner_bindings。
