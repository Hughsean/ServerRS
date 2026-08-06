-- B6 富消息 Artifact 引用：Artifact 信封表。
-- 约束：账号隔离、有界、TTL、撤回/删除/内容策略失效传播。
-- 不自动下载；URL 不写日志；不让 LLM 默认看到完整 URL/JSON/XML/forward payload。
-- Forward-only migration. Depends on 20260723_personal_secretary_ingestion.sql (account FK).
-- 回滚顺序：直接 DROP TABLE secretary_artifacts。

CREATE TABLE IF NOT EXISTS secretary_artifacts
(
    artifact_id              CHAR(36) CHARACTER SET ascii COLLATE ascii_bin PRIMARY KEY,
    account_id               BIGINT UNSIGNED NOT NULL,
    source_event_id          CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    conversation_id          BIGINT UNSIGNED NOT NULL,
    artifact_kind            VARCHAR(32)     NOT NULL,
    -- 平台引用（有界，不存完整 URL）
    platform_reference       VARCHAR(500)    COLLATE utf8mb4_bin NOT NULL,
    display_name             VARCHAR(500)    NULL,
    mime_type                VARCHAR(200)    NULL,
    size_bytes               BIGINT UNSIGNED NULL,
    hash_or_source_key       VARCHAR(500)    NULL,
    description              VARCHAR(2000)   NULL,
    availability             VARCHAR(32)     NOT NULL DEFAULT 'available',
    content_policy           VARCHAR(32)     NOT NULL DEFAULT 'normal',
    created_at_unix_secs     BIGINT          NOT NULL,
    ttl_expires_at_unix_secs BIGINT          NULL,
    created_at               DATETIME(6)     NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    updated_at               DATETIME(6)     NOT NULL DEFAULT CURRENT_TIMESTAMP(6)
        ON UPDATE CURRENT_TIMESTAMP(6),

    CONSTRAINT chk_secretary_artifact_kind
        CHECK (artifact_kind IN (
            'image', 'file', 'record', 'video', 'forward',
            'rich_json', 'rich_xml', 'rich_card'
        )),
    CONSTRAINT chk_secretary_artifact_availability
        CHECK (availability IN (
            'available', 'expired', 'recalled', 'owner_deleted', 'policy_blocked'
        )),
    CONSTRAINT chk_secretary_artifact_policy
        CHECK (content_policy IN ('normal', 'local_only', 'envelope_only', 'never_long_term')),
    CONSTRAINT fk_secretary_artifact_account
        FOREIGN KEY (account_id) REFERENCES secretary_accounts(id) ON DELETE CASCADE,
    CONSTRAINT fk_secretary_artifact_source_event
        FOREIGN KEY (source_event_id) REFERENCES secretary_source_events(source_event_id)
        ON DELETE CASCADE,
    CONSTRAINT fk_secretary_artifact_conversation
        FOREIGN KEY (conversation_id) REFERENCES secretary_conversations(id) ON DELETE CASCADE,
    -- 按 source_event_id 按需检索（严格账号隔离）。
    INDEX idx_secretary_artifact_event (account_id, source_event_id, availability),
    -- TTL 过期清理。
    INDEX idx_secretary_artifact_ttl (ttl_expires_at_unix_secs),
    -- 撤回失效传播。
    INDEX idx_secretary_artifact_source_event (source_event_id, availability)
) ENGINE = InnoDB
  DEFAULT CHARSET = utf8mb4
  COLLATE = utf8mb4_unicode_ci
  COMMENT = '富消息 Artifact 信封（不自动下载；有界；TTL；撤回/删除/策略失效传播）';
