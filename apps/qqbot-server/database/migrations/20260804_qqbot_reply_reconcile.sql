-- EVT-007-MSG 延迟 Reply 后台修复的持久化候选队列（Codex 第四轮复核）。
--
-- 候选队列语义（本表是真实来源，不扫描全部 source_events）：
--   - unresolved Reply SourceEvent 入库时，同一事务 INSERT 候选行；
--   - 任意路径成功解析后，同一事务 DELETE 候选行；
--   - 领取时从本表 LEFT JOIN source_events（按退避/租约条件筛选，LIMIT 有界，
--     FOR UPDATE SKIP LOCKED），不扫描全部普通消息。
CREATE TABLE IF NOT EXISTS `secretary_reply_reconcile_claims` (
  `source_event_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `lease_token` char(36) CHARACTER SET ascii COLLATE ascii_bin DEFAULT NULL,
  `lease_expires_at` datetime(6) DEFAULT NULL,
  `attempts` int unsigned NOT NULL DEFAULT '0',
  `last_error` varchar(512) COLLATE utf8mb4_unicode_ci DEFAULT NULL,
  `next_eligible_at` datetime(6) DEFAULT NULL,
  `created_at` datetime(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
  `updated_at` datetime(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6) ON UPDATE CURRENT_TIMESTAMP(6),
  PRIMARY KEY (`source_event_id`),
  KEY `idx_secretary_reply_reconcile_eligible` (`lease_expires_at`,`next_eligible_at`,`source_event_id`),
  KEY `idx_secretary_reply_reconcile_token` (`lease_token`),
  CONSTRAINT `fk_secretary_reconcile_claim_source`
    FOREIGN KEY (`source_event_id`)
    REFERENCES `secretary_source_events` (`source_event_id`)
    ON DELETE CASCADE
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci
  COMMENT='Reply 修复候选队列；每个 unresolved Reply 子事件一行';

-- 结构验证（fail-closed，参见 migrations/README.md）：
-- 若表已存在但结构不符，IF NOT EXISTS 不会覆盖，后续 INSERT IGNORE 可能静默
-- 失败。每项校验使用一个条件性多行标量子查询：结构正确时子查询为零行并
-- 返回 NULL；结构不符时产生两行，MySQL 必须以 1242 中止迁移。该机制不依赖
-- 会话变量、存储过程或迁移执行器读取 SELECT 结果。
SELECT (
  SELECT fail_row
  FROM (SELECT 1 AS fail_row UNION ALL SELECT 2 AS fail_row) AS column_mismatch
  WHERE COALESCE((
    SELECT
      COUNT(*) = 8
      AND COUNT(CASE WHEN COLUMN_NAME = 'source_event_id'
        AND DATA_TYPE = 'char' AND CHARACTER_MAXIMUM_LENGTH = 36
        AND IS_NULLABLE = 'NO' AND CHARACTER_SET_NAME = 'ascii'
        AND COLLATION_NAME = 'ascii_bin' THEN 1 END) = 1
      AND COUNT(CASE WHEN COLUMN_NAME = 'lease_token'
        AND DATA_TYPE = 'char' AND CHARACTER_MAXIMUM_LENGTH = 36
        AND IS_NULLABLE = 'YES' AND CHARACTER_SET_NAME = 'ascii'
        AND COLLATION_NAME = 'ascii_bin' THEN 1 END) = 1
      AND COUNT(CASE WHEN COLUMN_NAME = 'lease_expires_at'
        AND DATA_TYPE = 'datetime' AND DATETIME_PRECISION = 6
        AND IS_NULLABLE = 'YES' THEN 1 END) = 1
      AND COUNT(CASE WHEN COLUMN_NAME = 'attempts'
        AND COLUMN_TYPE = 'int unsigned' AND IS_NULLABLE = 'NO'
        AND COLUMN_DEFAULT = '0' THEN 1 END) = 1
      AND COUNT(CASE WHEN COLUMN_NAME = 'last_error'
        AND DATA_TYPE = 'varchar' AND CHARACTER_MAXIMUM_LENGTH = 512
        AND IS_NULLABLE = 'YES' AND CHARACTER_SET_NAME = 'utf8mb4'
        AND COLLATION_NAME = 'utf8mb4_unicode_ci' THEN 1 END) = 1
      AND COUNT(CASE WHEN COLUMN_NAME = 'next_eligible_at'
        AND DATA_TYPE = 'datetime' AND DATETIME_PRECISION = 6
        AND IS_NULLABLE = 'YES' THEN 1 END) = 1
      AND COUNT(CASE WHEN COLUMN_NAME = 'created_at'
        AND DATA_TYPE = 'datetime' AND DATETIME_PRECISION = 6
        AND IS_NULLABLE = 'NO'
        AND COLUMN_DEFAULT = 'CURRENT_TIMESTAMP(6)' THEN 1 END) = 1
      AND COUNT(CASE WHEN COLUMN_NAME = 'updated_at'
        AND DATA_TYPE = 'datetime' AND DATETIME_PRECISION = 6
        AND IS_NULLABLE = 'NO'
        AND COLUMN_DEFAULT = 'CURRENT_TIMESTAMP(6)'
        AND EXTRA = 'DEFAULT_GENERATED on update CURRENT_TIMESTAMP(6)' THEN 1 END) = 1
    FROM INFORMATION_SCHEMA.COLUMNS
    WHERE TABLE_SCHEMA = DATABASE()
      AND TABLE_NAME = 'secretary_reply_reconcile_claims'
  ), 0) <> 1
) AS reply_reconcile_columns_verified;

SELECT (
  SELECT fail_row
  FROM (SELECT 1 AS fail_row UNION ALL SELECT 2 AS fail_row) AS index_mismatch
  WHERE COALESCE((
    SELECT
      COUNT(*) = 3
      AND COUNT(CASE WHEN INDEX_NAME = 'PRIMARY' AND NON_UNIQUE = 0
        AND INDEXED_COLUMNS = 'source_event_id' THEN 1 END) = 1
      AND COUNT(CASE WHEN INDEX_NAME = 'idx_secretary_reply_reconcile_eligible'
        AND NON_UNIQUE = 1
        AND INDEXED_COLUMNS = 'lease_expires_at,next_eligible_at,source_event_id'
        THEN 1 END) = 1
      AND COUNT(CASE WHEN INDEX_NAME = 'idx_secretary_reply_reconcile_token'
        AND NON_UNIQUE = 1 AND INDEXED_COLUMNS = 'lease_token' THEN 1 END) = 1
    FROM (
      SELECT INDEX_NAME, MIN(NON_UNIQUE) AS NON_UNIQUE,
             GROUP_CONCAT(COLUMN_NAME ORDER BY SEQ_IN_INDEX SEPARATOR ',') AS INDEXED_COLUMNS
      FROM INFORMATION_SCHEMA.STATISTICS
      WHERE TABLE_SCHEMA = DATABASE()
        AND TABLE_NAME = 'secretary_reply_reconcile_claims'
      GROUP BY INDEX_NAME
    ) AS reply_reconcile_indexes
  ), 0) <> 1
) AS reply_reconcile_indexes_verified;

SELECT (
  SELECT fail_row
  FROM (SELECT 1 AS fail_row UNION ALL SELECT 2 AS fail_row) AS fk_mismatch
  WHERE COALESCE((
    SELECT
      COUNT(*) = 1
      AND COUNT(CASE WHEN rc.CONSTRAINT_NAME = 'fk_secretary_reconcile_claim_source'
        AND kcu.COLUMN_NAME = 'source_event_id'
        AND kcu.REFERENCED_TABLE_SCHEMA = DATABASE()
        AND kcu.REFERENCED_TABLE_NAME = 'secretary_source_events'
        AND kcu.REFERENCED_COLUMN_NAME = 'source_event_id'
        AND rc.DELETE_RULE = 'CASCADE' THEN 1 END) = 1
    FROM INFORMATION_SCHEMA.REFERENTIAL_CONSTRAINTS rc
    JOIN INFORMATION_SCHEMA.KEY_COLUMN_USAGE kcu
      ON kcu.CONSTRAINT_SCHEMA = rc.CONSTRAINT_SCHEMA
     AND kcu.TABLE_NAME = rc.TABLE_NAME
     AND kcu.CONSTRAINT_NAME = rc.CONSTRAINT_NAME
    WHERE rc.CONSTRAINT_SCHEMA = DATABASE()
      AND rc.TABLE_NAME = 'secretary_reply_reconcile_claims'
  ), 0) <> 1
) AS reply_reconcile_fk_verified;

-- 幂等回填历史 unresolved 事件。`INSERT IGNORE` 不覆盖已有候选行；
-- 已解析事件的候选行随解析事务一同清理，不残留。
INSERT IGNORE INTO `secretary_reply_reconcile_claims` (`source_event_id`)
SELECT `source_event_id`
FROM `secretary_source_events`
WHERE `reply_to_platform_event_id` IS NOT NULL
  AND `reply_to_event_id` IS NULL;
