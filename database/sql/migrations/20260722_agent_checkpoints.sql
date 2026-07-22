CREATE TABLE agent_checkpoints
(
    checkpoint_id       CHAR(36)        PRIMARY KEY,
    run_id               CHAR(36)        NOT NULL,
    graph_id             VARCHAR(64)     NOT NULL,
    graph_version        INT UNSIGNED    NOT NULL,
    state_schema_version INT UNSIGNED    NOT NULL,
    user_id              BIGINT UNSIGNED NOT NULL,
    conversation_id      BIGINT UNSIGNED NOT NULL,
    next_node            VARCHAR(128)    NOT NULL,
    completed_step       INT UNSIGNED    NOT NULL,
    suspend_reason       VARCHAR(32)     NOT NULL,
    payload              JSON            NOT NULL,
    status               VARCHAR(16)     NOT NULL DEFAULT 'pending'
        COMMENT 'pending|consumed',
    expires_at           DATETIME(6)     NOT NULL,
    consumed_at          DATETIME(6)     NULL,
    created_at           DATETIME(6)     NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    updated_at           DATETIME(6)     NOT NULL DEFAULT CURRENT_TIMESTAMP(6)
        ON UPDATE CURRENT_TIMESTAMP(6),

    CONSTRAINT chk_agent_checkpoints_status
        CHECK (status IN ('pending', 'consumed')),
    CONSTRAINT chk_agent_checkpoints_suspend_reason
        CHECK (suspend_reason IN ('external_input', 'approval', 'external_event', 'business')),
    CONSTRAINT fk_agent_checkpoints_user
        FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE,
    CONSTRAINT fk_agent_checkpoints_conversation
        FOREIGN KEY (conversation_id) REFERENCES conversations(id) ON DELETE CASCADE,

    INDEX idx_agent_checkpoints_run (run_id),
    INDEX idx_agent_checkpoints_owner_status (user_id, status, expires_at),
    INDEX idx_agent_checkpoints_conversation (conversation_id),
    INDEX idx_agent_checkpoints_expiry (status, expires_at)
) ENGINE = InnoDB
  DEFAULT CHARSET = utf8mb4
  COLLATE = utf8mb4_unicode_ci
  COMMENT = 'Agent 暂停/恢复持久化快照；pending 仅可原子消费一次';
