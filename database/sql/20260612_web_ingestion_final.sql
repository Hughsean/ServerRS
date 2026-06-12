-- ============================================================================
-- 20260612_web_ingestion_final.sql
-- Web Ingestion + Knowledge Auto Upgrade — New Tables for ServerRS
--
-- Creates the complete table set for the web ingestion pipeline:
--   web_sources, web_source_urls, web_crawl_jobs, web_pages,
--   knowledge_ingestion_runs, knowledge_publish_records,
--   knowledge_chunk_manifests, knowledge_vector_manifests,
--   domain_event_outbox, web_ingestion_audit_logs
--
-- All constraints follow the Phase 1 hard constraints from the task book:
--   - All IDs: BIGINT UNSIGNED
--   - All SHA256 hashes: CHAR(64)
--   - URL unique: (source_id, url_hash) — NOT global unique
--   - Publishing: one active record per (source_id, page_id) via generated column
--   - Deterministic event_key via UNIQUE(event_key)
--   - InnoDB + utf8mb4 + utf8mb4_unicode_ci
--
-- Database: digital_companion (same as existing init.sql)
-- ============================================================================

USE digital_companion;

-- ============================================================================
-- 1. web_sources — 网页来源策略配置表
-- ============================================================================
CREATE TABLE web_sources
(
    id               BIGINT UNSIGNED PRIMARY KEY AUTO_INCREMENT COMMENT '来源ID',
    name             VARCHAR(128)    NOT NULL COMMENT '来源名称',
    description      TEXT            NULL COMMENT '来源描述',
    approval_status  VARCHAR(32)     NOT NULL DEFAULT 'pending'
                     COMMENT '审核状态: pending/approved/rejected/disabled',
    trust_level      VARCHAR(32)     NOT NULL DEFAULT 'normal'
                     COMMENT '信任级别: official/trusted/normal/untrusted',
    auto_publish     TINYINT(1)      NOT NULL DEFAULT 0
                     COMMENT '是否自动发布（仍需通过质量门控）',
    allowed_domains  JSON            NULL
                     COMMENT '允许抓取的域名列表（JSON数组）',
    default_language VARCHAR(16)     NOT NULL DEFAULT 'zh'
                     COMMENT '默认语言代码',
    enabled          TINYINT(1)      NOT NULL DEFAULT 0
                     COMMENT '是否启用: 1启用, 0禁用',
    created_at       DATETIME(6)     NOT NULL DEFAULT CURRENT_TIMESTAMP(6)
                     COMMENT '创建时间',
    updated_at       DATETIME(6)     NOT NULL DEFAULT CURRENT_TIMESTAMP(6)
                     ON UPDATE CURRENT_TIMESTAMP(6)
                     COMMENT '更新时间',
    deleted_at       DATETIME(6)     NULL COMMENT '软删除时间',
    INDEX idx_web_sources_approval (approval_status),
    INDEX idx_web_sources_enabled (enabled),
    INDEX idx_web_sources_trust (trust_level)
) ENGINE = InnoDB
  DEFAULT CHARSET = utf8mb4
  COLLATE = utf8mb4_unicode_ci COMMENT = '网页来源策略配置表';

-- ============================================================================
-- 2. web_source_urls — 来源下的待抓取URL
-- ============================================================================
CREATE TABLE web_source_urls
(
    id                  BIGINT UNSIGNED PRIMARY KEY AUTO_INCREMENT COMMENT 'URL记录ID',
    source_id           BIGINT UNSIGNED NOT NULL COMMENT '关联来源ID',
    url                 TEXT            NOT NULL COMMENT '原始URL',
    canonical_url       TEXT            NULL COMMENT '规范化URL',
    url_hash            CHAR(64)        NOT NULL COMMENT '规范化URL的SHA-256',
    enabled             TINYINT(1)      NOT NULL DEFAULT 1
                        COMMENT '是否启用: 1启用, 0禁用',
    crawl_interval_secs INT UNSIGNED    NOT NULL DEFAULT 86400
                        COMMENT '抓取间隔（秒），默认24小时',
    last_crawled_at     DATETIME(6)     NULL COMMENT '最近一次抓取时间',
    last_content_hash   CHAR(64)        NULL COMMENT '最近一次抓取的content_hash',
    created_at          DATETIME(6)     NOT NULL DEFAULT CURRENT_TIMESTAMP(6)
                        COMMENT '创建时间',
    updated_at          DATETIME(6)     NOT NULL DEFAULT CURRENT_TIMESTAMP(6)
                        ON UPDATE CURRENT_TIMESTAMP(6)
                        COMMENT '更新时间',
    deleted_at          DATETIME(6)     NULL COMMENT '软删除时间',
    -- source 级唯一，允许同一URL属于不同source
    UNIQUE KEY uk_web_source_urls_source_hash (source_id, url_hash),
    INDEX idx_web_source_urls_source (source_id),
    INDEX idx_web_source_urls_enabled (enabled),
    INDEX idx_web_source_urls_last_crawled (last_crawled_at),
    FOREIGN KEY (source_id) REFERENCES web_sources (id) ON DELETE CASCADE
) ENGINE = InnoDB
  DEFAULT CHARSET = utf8mb4
  COLLATE = utf8mb4_unicode_ci COMMENT = '来源待抓取URL表';

-- ============================================================================
-- 3. web_crawl_jobs — 定时抓取批次
-- ============================================================================
CREATE TABLE web_crawl_jobs
(
    id           BIGINT UNSIGNED PRIMARY KEY AUTO_INCREMENT COMMENT '抓取批次ID',
    source_id    BIGINT UNSIGNED NULL COMMENT '关联来源ID（可为NULL表示跨来源批次）',
    status       VARCHAR(32)     NOT NULL DEFAULT 'pending'
                 COMMENT '状态: pending/running/succeeded/failed/dead/cancelled',
    scheduled_at DATETIME(6)     NOT NULL COMMENT '计划执行时间',
    started_at   DATETIME(6)     NULL COMMENT '实际开始时间',
    finished_at  DATETIME(6)     NULL COMMENT '完成时间',
    last_error   TEXT            NULL COMMENT '最近一次错误信息',
    created_at   DATETIME(6)     NOT NULL DEFAULT CURRENT_TIMESTAMP(6) COMMENT '创建时间',
    updated_at   DATETIME(6)     NOT NULL DEFAULT CURRENT_TIMESTAMP(6)
                 ON UPDATE CURRENT_TIMESTAMP(6) COMMENT '更新时间',
    INDEX idx_web_crawl_jobs_status_scheduled (status, scheduled_at),
    INDEX idx_web_crawl_jobs_source (source_id),
    INDEX idx_web_crawl_jobs_created (created_at),
    FOREIGN KEY (source_id) REFERENCES web_sources (id) ON DELETE SET NULL
) ENGINE = InnoDB
  DEFAULT CHARSET = utf8mb4
  COLLATE = utf8mb4_unicode_ci COMMENT = '定时抓取批次表';

-- ============================================================================
-- 4. web_pages — source 下的网页实体
-- ============================================================================
CREATE TABLE web_pages
(
    id                    BIGINT UNSIGNED PRIMARY KEY AUTO_INCREMENT COMMENT '网页实体ID',
    source_id             BIGINT UNSIGNED NOT NULL COMMENT '关联来源ID',
    source_url_id         BIGINT UNSIGNED NULL COMMENT '关联URL记录ID',
    url                   TEXT            NOT NULL COMMENT '当前URL',
    canonical_url         TEXT            NULL COMMENT '规范化URL',
    url_hash              CHAR(64)        NOT NULL COMMENT '规范化URL的SHA-256',
    latest_content_hash   CHAR(64)        NULL COMMENT '最近一次成功抓取的content_hash',
    latest_success_run_id BIGINT UNSIGNED NULL
                          COMMENT '最近一次成功ingestion run的ID',
    last_fetched_at       DATETIME(6)     NULL COMMENT '最近一次抓取时间',
    created_at            DATETIME(6)     NOT NULL DEFAULT CURRENT_TIMESTAMP(6)
                          COMMENT '创建时间',
    updated_at            DATETIME(6)     NOT NULL DEFAULT CURRENT_TIMESTAMP(6)
                          ON UPDATE CURRENT_TIMESTAMP(6)
                          COMMENT '更新时间',
    deleted_at            DATETIME(6)     NULL COMMENT '软删除时间',
    -- source 级唯一，允许同一URL属于不同source
    UNIQUE KEY uk_web_pages_source_hash (source_id, url_hash),
    INDEX idx_web_pages_source (source_id),
    INDEX idx_web_pages_source_url (source_url_id),
    INDEX idx_web_pages_latest_run (latest_success_run_id),
    FOREIGN KEY (source_id) REFERENCES web_sources (id) ON DELETE CASCADE,
    FOREIGN KEY (source_url_id) REFERENCES web_source_urls (id) ON DELETE SET NULL
) ENGINE = InnoDB
  DEFAULT CHARSET = utf8mb4
  COLLATE = utf8mb4_unicode_ci COMMENT = '网页实体表';

-- ============================================================================
-- 5. knowledge_ingestion_runs — 内容版本处理流程
-- ============================================================================
CREATE TABLE knowledge_ingestion_runs
(
    id                  BIGINT UNSIGNED PRIMARY KEY AUTO_INCREMENT COMMENT 'Run ID',
    source_id           BIGINT UNSIGNED NOT NULL COMMENT '关联来源ID',
    source_url_id       BIGINT UNSIGNED NULL COMMENT '关联URL记录ID',
    crawl_job_id        BIGINT UNSIGNED NULL COMMENT '关联抓取批次ID',
    page_id             BIGINT UNSIGNED NOT NULL COMMENT '关联网页实体ID',
    content_hash        CHAR(64)        NOT NULL COMMENT '内容SHA-256',
    content_key         CHAR(64)        NOT NULL COMMENT 'sha256(source_id+page_id+content_hash)',
    run_key             CHAR(64)        NOT NULL COMMENT '幂等键：sha256(source_id+page_id+content_hash+llm_prompt_version+chunker_version+embedding_model+pipeline_version)',
    version_key         CHAR(64)        NOT NULL COMMENT '版本键，= run_key',
    status              VARCHAR(32)     NOT NULL DEFAULT 'pending'
                        COMMENT '状态: pending/running/staged/published/rejected/skipped/failed/dead/cancelled',
    stage               VARCHAR(32)     NOT NULL DEFAULT 'pending'
                        COMMENT '阶段: pending/fetching/fetched/unchanged/cleaning/cleaned/distilling/distilled/quality_checked/chunking/chunked/embedding/embedded/indexing/indexed/staging/publishing/published/rejected/skipped/failed/dead/cancelled',
    llm_provider        VARCHAR(64)     NULL COMMENT '使用的LLM provider',
    llm_model           VARCHAR(128)    NULL COMMENT '使用的LLM模型名',
    llm_prompt_version  VARCHAR(64)     NULL COMMENT 'prompt版本标识',
    llm_input_tokens    INT UNSIGNED    NULL COMMENT 'LLM输入token数',
    llm_output_tokens   INT UNSIGNED    NULL COMMENT 'LLM输出token数',
    chunker_version     VARCHAR(64)     NULL COMMENT 'chunker版本标识',
    embedding_provider  VARCHAR(64)     NULL COMMENT 'embedding provider',
    embedding_model     VARCHAR(128)    NULL COMMENT 'embedding模型名',
    embedding_dimension INT UNSIGNED    NULL COMMENT 'embedding维度',
    quality_score       DOUBLE          NULL COMMENT '质量分数 0.0-1.0',
    quality_result      JSON            NULL COMMENT '质量门控详细结果',
    risk_flags          JSON            NULL COMMENT '风险标记（JSON数组）',
    should_publish      TINYINT(1)      NULL COMMENT '质量门控是否建议发布',
    last_error          TEXT            NULL COMMENT '最近一次错误信息',
    retry_count         INT UNSIGNED    NOT NULL DEFAULT 0 COMMENT '重试次数',
    started_at          DATETIME(6)     NULL COMMENT '开始处理时间',
    finished_at         DATETIME(6)     NULL COMMENT '完成时间',
    created_at          DATETIME(6)     NOT NULL DEFAULT CURRENT_TIMESTAMP(6) COMMENT '创建时间',
    updated_at          DATETIME(6)     NOT NULL DEFAULT CURRENT_TIMESTAMP(6)
                        ON UPDATE CURRENT_TIMESTAMP(6) COMMENT '更新时间',
    UNIQUE KEY uk_ingestion_runs_run_key (run_key),
    UNIQUE KEY uk_ingestion_runs_version_key (version_key),
    UNIQUE KEY uk_ingestion_runs_content_key (content_key),
    INDEX idx_ingestion_runs_page (page_id),
    INDEX idx_ingestion_runs_source (source_id),
    INDEX idx_ingestion_runs_crawl_job (crawl_job_id),
    INDEX idx_ingestion_runs_status_stage (status, stage),
    INDEX idx_ingestion_runs_created (created_at),
    FOREIGN KEY (source_id) REFERENCES web_sources (id) ON DELETE CASCADE,
    FOREIGN KEY (source_url_id) REFERENCES web_source_urls (id) ON DELETE SET NULL,
    FOREIGN KEY (crawl_job_id) REFERENCES web_crawl_jobs (id) ON DELETE SET NULL,
    FOREIGN KEY (page_id) REFERENCES web_pages (id) ON DELETE CASCADE
) ENGINE = InnoDB
  DEFAULT CHARSET = utf8mb4
  COLLATE = utf8mb4_unicode_ci COMMENT = '内容版本处理流程表';

-- ============================================================================
-- 6. knowledge_publish_records — 发布版本记录
--
--   同页最多一个 active 记录的约束通过 active_page_key 列实现：
--     - active=1 时，应用层写入 "{source_id}:{page_id}"
--     - active=0 时，写入 NULL
--     - UNIQUE(active_page_key) 保证同页最多一个 active（NULL 可重复）
--
--   superseded_by_record_id / rolled_back_from_record_id 是自引用逻辑外键，
--   约束由应用层 publish/rollback service + 事务锁保证，不在此处建物理 FK。
-- ============================================================================
CREATE TABLE knowledge_publish_records
(
    id                        BIGINT UNSIGNED PRIMARY KEY AUTO_INCREMENT COMMENT '发布记录ID',
    source_id                 BIGINT UNSIGNED NOT NULL COMMENT '关联来源ID',
    page_id                   BIGINT UNSIGNED NOT NULL COMMENT '关联网页实体ID',
    run_id                    BIGINT UNSIGNED NOT NULL COMMENT '关联ingestion run ID',
    document_id               BIGINT UNSIGNED NOT NULL COMMENT '关联knowledge_documents.document_id',
    version_key               CHAR(64)        NOT NULL COMMENT '版本键',
    content_hash              CHAR(64)        NOT NULL COMMENT '内容SHA-256',
    publish_status            VARCHAR(32)     NOT NULL DEFAULT 'staged'
                              COMMENT '发布状态: staged/publishing/published/superseded/rolled_back/failed',
    active                    TINYINT(1)      NOT NULL DEFAULT 0
                              COMMENT '是否当前活跃版本: 1活跃, 0非活跃',
    -- 应用层维护: active=1 时写入 "{source_id}:{page_id}"，active=0 时写入 NULL
    -- UNIQUE 约束允许 NULL 重复，因此可实现"同页最多一个 active"
    active_page_key           VARCHAR(128)    NULL
                              COMMENT 'active=1时为source_id:page_id，active=0时为NULL',
    activated_at              DATETIME(6)     NULL COMMENT '激活时间',
    superseded_at             DATETIME(6)     NULL COMMENT '被新版本替代的时间',
    superseded_by_record_id   BIGINT UNSIGNED NULL COMMENT '替代此版本的发布记录ID（应用层FK，无物理约束）',
    rolled_back_from_record_id BIGINT UNSIGNED NULL COMMENT '从哪个记录rollback到此版本（应用层FK，无物理约束）',
    created_at                DATETIME(6)     NOT NULL DEFAULT CURRENT_TIMESTAMP(6) COMMENT '创建时间',
    updated_at                DATETIME(6)     NOT NULL DEFAULT CURRENT_TIMESTAMP(6)
                              ON UPDATE CURRENT_TIMESTAMP(6) COMMENT '更新时间',
    -- 同页最多一个 active：NULL 可重复，"{source_id}:{page_id}" 唯一
    UNIQUE KEY uk_publish_records_one_active_page (active_page_key),
    INDEX idx_publish_records_page (source_id, page_id),
    INDEX idx_publish_records_run (run_id),
    INDEX idx_publish_records_document (document_id),
    INDEX idx_publish_records_status (publish_status),
    INDEX idx_publish_records_version_key (version_key),
    -- 物理 FK 仅引用已存在的表（无自引用）
    FOREIGN KEY (source_id) REFERENCES web_sources (id) ON DELETE CASCADE,
    FOREIGN KEY (page_id) REFERENCES web_pages (id) ON DELETE CASCADE,
    FOREIGN KEY (run_id) REFERENCES knowledge_ingestion_runs (id) ON DELETE CASCADE,
    FOREIGN KEY (document_id) REFERENCES knowledge_documents (document_id) ON DELETE CASCADE
) ENGINE = InnoDB
  DEFAULT CHARSET = utf8mb4
  COLLATE = utf8mb4_unicode_ci COMMENT = '知识发布版本记录表';

-- ============================================================================
-- 7. knowledge_chunk_manifests — web ingestion 版本chunk到knowledge_chunks的映射
-- ============================================================================
CREATE TABLE knowledge_chunk_manifests
(
    id                BIGINT UNSIGNED PRIMARY KEY AUTO_INCREMENT COMMENT 'Manifest ID',
    publish_record_id BIGINT UNSIGNED NOT NULL COMMENT '关联发布记录ID',
    run_id            BIGINT UNSIGNED NOT NULL COMMENT '关联ingestion run ID',
    document_id       BIGINT UNSIGNED NOT NULL COMMENT '关联knowledge_documents.document_id',
    chunk_id          BIGINT UNSIGNED NOT NULL COMMENT '关联knowledge_chunks.chunk_id',
    version_key       CHAR(64)        NOT NULL COMMENT '版本键',
    chunk_hash        CHAR(64)        NOT NULL COMMENT 'chunk幂等哈希: sha256(version_key+chunk_type+chunk_index+normalized_content+chunker_version)',
    chunk_type        VARCHAR(32)     NOT NULL DEFAULT 'atomic'
                      COMMENT 'chunk类型: document_summary/section_summary/atomic',
    chunk_index       INT UNSIGNED    NOT NULL COMMENT 'chunk在版本内的序号',
    active            TINYINT(1)      NOT NULL DEFAULT 0
                      COMMENT '是否活跃: 1活跃, 0非活跃（被新版本替代）',
    created_at        DATETIME(6)     NOT NULL DEFAULT CURRENT_TIMESTAMP(6) COMMENT '创建时间',
    updated_at        DATETIME(6)     NOT NULL DEFAULT CURRENT_TIMESTAMP(6)
                      ON UPDATE CURRENT_TIMESTAMP(6) COMMENT '更新时间',
    UNIQUE KEY uk_chunk_manifests_version_hash (version_key, chunk_hash),
    UNIQUE KEY uk_chunk_manifests_chunk_id (chunk_id),
    INDEX idx_chunk_manifests_publish_record (publish_record_id),
    INDEX idx_chunk_manifests_run (run_id),
    INDEX idx_chunk_manifests_document (document_id),
    INDEX idx_chunk_manifests_active (active),
    FOREIGN KEY (publish_record_id) REFERENCES knowledge_publish_records (id) ON DELETE CASCADE,
    FOREIGN KEY (run_id) REFERENCES knowledge_ingestion_runs (id) ON DELETE CASCADE,
    FOREIGN KEY (document_id) REFERENCES knowledge_documents (document_id) ON DELETE CASCADE,
    FOREIGN KEY (chunk_id) REFERENCES knowledge_chunks (chunk_id) ON DELETE CASCADE
) ENGINE = InnoDB
  DEFAULT CHARSET = utf8mb4
  COLLATE = utf8mb4_unicode_ci COMMENT = 'Web Ingestion Chunk 映射表';

-- ============================================================================
-- 8. knowledge_vector_manifests — web ingestion 版本向量到Qdrant的映射
-- ============================================================================
CREATE TABLE knowledge_vector_manifests
(
    id                  BIGINT UNSIGNED PRIMARY KEY AUTO_INCREMENT COMMENT 'Vector Manifest ID',
    publish_record_id   BIGINT UNSIGNED NOT NULL COMMENT '关联发布记录ID',
    run_id              BIGINT UNSIGNED NOT NULL COMMENT '关联ingestion run ID',
    document_id         BIGINT UNSIGNED NOT NULL COMMENT '关联knowledge_documents.document_id',
    chunk_id            BIGINT UNSIGNED NOT NULL COMMENT '关联knowledge_chunks.chunk_id',
    chunk_hash          CHAR(64)        NOT NULL COMMENT '关联的chunk_hash',
    qdrant_collection   VARCHAR(128)    NOT NULL COMMENT 'Qdrant collection名称',
    qdrant_point_id     CHAR(64)        NOT NULL COMMENT 'Qdrant point ID: sha256(qdrant_collection+chunk_hash+embedding_model)',
    embedding_provider  VARCHAR(64)     NOT NULL COMMENT 'embedding provider',
    embedding_model     VARCHAR(128)    NOT NULL COMMENT 'embedding模型名',
    embedding_dimension INT UNSIGNED    NOT NULL COMMENT 'embedding维度',
    active              TINYINT(1)      NOT NULL DEFAULT 0
                        COMMENT '是否活跃: 1活跃, 0非活跃',
    created_at          DATETIME(6)     NOT NULL DEFAULT CURRENT_TIMESTAMP(6) COMMENT '创建时间',
    updated_at          DATETIME(6)     NOT NULL DEFAULT CURRENT_TIMESTAMP(6)
                        ON UPDATE CURRENT_TIMESTAMP(6) COMMENT '更新时间',
    UNIQUE KEY uk_vector_manifests_qdrant_point (qdrant_collection, qdrant_point_id),
    UNIQUE KEY uk_vector_manifests_chunk_model (chunk_id, embedding_model),
    INDEX idx_vector_manifests_publish_record (publish_record_id),
    INDEX idx_vector_manifests_run (run_id),
    INDEX idx_vector_manifests_document (document_id),
    INDEX idx_vector_manifests_active (active),
    FOREIGN KEY (publish_record_id) REFERENCES knowledge_publish_records (id) ON DELETE CASCADE,
    FOREIGN KEY (run_id) REFERENCES knowledge_ingestion_runs (id) ON DELETE CASCADE,
    FOREIGN KEY (document_id) REFERENCES knowledge_documents (document_id) ON DELETE CASCADE,
    FOREIGN KEY (chunk_id) REFERENCES knowledge_chunks (chunk_id) ON DELETE CASCADE
) ENGINE = InnoDB
  DEFAULT CHARSET = utf8mb4
  COLLATE = utf8mb4_unicode_ci COMMENT = 'Web Ingestion 向量映射表';

-- ============================================================================
-- 9. domain_event_outbox — 持久化核心流程事件
-- ============================================================================
CREATE TABLE domain_event_outbox
(
    id             BIGINT UNSIGNED PRIMARY KEY AUTO_INCREMENT COMMENT '事件ID',
    event_key      CHAR(64)        NOT NULL COMMENT '幂等事件键: sha256(event_type+aggregate_type+aggregate_id+run_id+version_key)',
    event_type     VARCHAR(128)    NOT NULL COMMENT '事件类型: CrawlJobCreated/UrlDiscovered/PageFetched/...',
    aggregate_type VARCHAR(64)     NOT NULL COMMENT '聚合类型: web_crawl_job/web_page/knowledge_ingestion_run/knowledge_publish_record',
    aggregate_id   BIGINT UNSIGNED NOT NULL COMMENT '聚合根ID',
    payload        JSON            NOT NULL COMMENT '事件负载（仅放ID和小型元数据，禁止放全文/向量）',
    status         VARCHAR(32)     NOT NULL DEFAULT 'pending'
                   COMMENT '事件状态: pending/processing/published/failed/dead',
    retry_count    INT UNSIGNED    NOT NULL DEFAULT 0 COMMENT '已重试次数',
    max_retries    INT UNSIGNED    NOT NULL DEFAULT 5 COMMENT '最大重试次数',
    next_retry_at  DATETIME(6)     NULL COMMENT '下次重试时间',
    locked_by      VARCHAR(128)    NULL COMMENT '锁定者标识: {worker_id}:{uuid}',
    locked_until   DATETIME(6)     NULL COMMENT '锁过期时间',
    last_error     TEXT            NULL COMMENT '最近一次错误信息',
    created_at     DATETIME(6)     NOT NULL DEFAULT CURRENT_TIMESTAMP(6) COMMENT '创建时间',
    updated_at     DATETIME(6)     NOT NULL DEFAULT CURRENT_TIMESTAMP(6)
                   ON UPDATE CURRENT_TIMESTAMP(6) COMMENT '更新时间',
    published_at   DATETIME(6)     NULL COMMENT '发布（成功处理）时间',
    UNIQUE KEY uk_outbox_event_key (event_key),
    INDEX idx_outbox_claim (status, next_retry_at, created_at),
    INDEX idx_outbox_locked_by (locked_by),
    INDEX idx_outbox_aggregate (aggregate_type, aggregate_id),
    INDEX idx_outbox_event_type (event_type),
    INDEX idx_outbox_created (created_at)
) ENGINE = InnoDB
  DEFAULT CHARSET = utf8mb4
  COLLATE = utf8mb4_unicode_ci COMMENT = '领域事件发件箱表';

-- ============================================================================
-- 10. web_ingestion_audit_logs — web ingestion 审计日志
-- ============================================================================
CREATE TABLE web_ingestion_audit_logs
(
    id                BIGINT UNSIGNED PRIMARY KEY AUTO_INCREMENT COMMENT '审计日志ID',
    source_id         BIGINT UNSIGNED NULL COMMENT '关联来源ID',
    source_url_id     BIGINT UNSIGNED NULL COMMENT '关联URL记录ID',
    page_id           BIGINT UNSIGNED NULL COMMENT '关联网页实体ID',
    run_id            BIGINT UNSIGNED NULL COMMENT '关联ingestion run ID',
    publish_record_id BIGINT UNSIGNED NULL COMMENT '关联发布记录ID',
    action            VARCHAR(64)     NOT NULL COMMENT '操作类型',
    status            VARCHAR(32)     NOT NULL DEFAULT 'info'
                      COMMENT '状态: info/warning/error/success',
    message           TEXT            NOT NULL COMMENT '日志消息',
    metadata          JSON            NULL COMMENT '附加元数据',
    created_at        DATETIME(6)     NOT NULL DEFAULT CURRENT_TIMESTAMP(6) COMMENT '创建时间',
    INDEX idx_audit_source (source_id),
    INDEX idx_audit_page (page_id),
    INDEX idx_audit_run (run_id),
    INDEX idx_audit_publish_record (publish_record_id),
    INDEX idx_audit_action (action),
    INDEX idx_audit_created (created_at),
    FOREIGN KEY (source_id) REFERENCES web_sources (id) ON DELETE SET NULL,
    FOREIGN KEY (source_url_id) REFERENCES web_source_urls (id) ON DELETE SET NULL,
    FOREIGN KEY (page_id) REFERENCES web_pages (id) ON DELETE SET NULL,
    FOREIGN KEY (run_id) REFERENCES knowledge_ingestion_runs (id) ON DELETE SET NULL,
    FOREIGN KEY (publish_record_id) REFERENCES knowledge_publish_records (id) ON DELETE SET NULL
) ENGINE = InnoDB
  DEFAULT CHARSET = utf8mb4
  COLLATE = utf8mb4_unicode_ci COMMENT = 'Web Ingestion 审计日志表';
