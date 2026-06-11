CREATE TABLE IF NOT EXISTS stored_objects
(
    object_id       BIGINT UNSIGNED PRIMARY KEY AUTO_INCREMENT COMMENT '对象ID',
    bucket          VARCHAR(64)      NOT NULL COMMENT '存储桶/业务分组',
    object_key      VARCHAR(512)     NOT NULL COMMENT '对象存储键',
    original_name   VARCHAR(255)     NOT NULL COMMENT '原始文件名',
    mime_type       VARCHAR(128)     NOT NULL COMMENT 'MIME类型',
    size_bytes      BIGINT UNSIGNED  NOT NULL COMMENT '文件大小',
    sha256          CHAR(64)         NOT NULL COMMENT '内容SHA-256',
    storage_backend VARCHAR(32)      NOT NULL DEFAULT 'LOCAL' COMMENT '存储后端',
    public_url      TEXT             NULL COMMENT '公开访问URL',
    created_by      BIGINT UNSIGNED  NULL COMMENT '上传用户ID',
    created_at      TIMESTAMP        NOT NULL DEFAULT CURRENT_TIMESTAMP COMMENT '创建时间',
    UNIQUE KEY uk_stored_objects_bucket_key (bucket, object_key),
    KEY idx_stored_objects_created_by (created_by),
    KEY idx_stored_objects_sha256 (sha256),
    CONSTRAINT fk_stored_objects_created_by FOREIGN KEY (created_by) REFERENCES users (id) ON DELETE SET NULL
) ENGINE = InnoDB
  DEFAULT CHARSET = utf8mb4
  COLLATE = utf8mb4_unicode_ci COMMENT = '对象存储元数据表';
