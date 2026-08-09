CREATE TABLE IF NOT EXISTS `secretary_owner_response_outbox` (
  `response_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `delivery_status` varchar(16) COLLATE utf8mb4_unicode_ci NOT NULL DEFAULT 'pending',
  `attempts` int unsigned NOT NULL DEFAULT '0',
  `lease_token` char(36) CHARACTER SET ascii COLLATE ascii_bin DEFAULT NULL,
  `lease_expires_at` datetime(6) DEFAULT NULL,
  `next_eligible_at` datetime(6) DEFAULT NULL,
  `last_error_code` varchar(64) COLLATE utf8mb4_unicode_ci DEFAULT NULL,
  `platform_message_id` varchar(512) CHARACTER SET utf8mb4 COLLATE utf8mb4_bin DEFAULT NULL,
  `delivered_at` datetime(6) DEFAULT NULL,
  `created_at` datetime(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
  `updated_at` datetime(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6) ON UPDATE CURRENT_TIMESTAMP(6),
  PRIMARY KEY (`response_id`),
  UNIQUE KEY `uk_secretary_owner_response_lease` (`lease_token`),
  KEY `idx_secretary_owner_response_claim`
    (`delivery_status`,`next_eligible_at`,`lease_expires_at`,`response_id`),
  CONSTRAINT `fk_secretary_owner_response_outbox_response`
    FOREIGN KEY (`response_id`) REFERENCES `secretary_action_responses` (`response_id`)
    ON DELETE CASCADE,
  CONSTRAINT `chk_secretary_owner_response_delivery_status`
    CHECK (`delivery_status` IN
      ('pending','claimed','delivered','failed','suppressed','unknown_commit'))
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci
  COMMENT='OwnerCommand 响应草稿的 QQ 开放平台被动回复 Outbox';

SELECT (
  SELECT fail_row
  FROM (SELECT 1 AS fail_row UNION ALL SELECT 2 AS fail_row) AS mismatch
  WHERE COALESCE((
    SELECT COUNT(*) = 11
      AND COUNT(CASE WHEN COLUMN_NAME = 'response_id' AND DATA_TYPE = 'char'
        AND CHARACTER_MAXIMUM_LENGTH = 36 AND IS_NULLABLE = 'NO'
        AND CHARACTER_SET_NAME = 'ascii' AND COLLATION_NAME = 'ascii_bin' THEN 1 END) = 1
      AND COUNT(CASE WHEN COLUMN_NAME = 'delivery_status' AND DATA_TYPE = 'varchar'
        AND CHARACTER_MAXIMUM_LENGTH = 16 AND IS_NULLABLE = 'NO' THEN 1 END) = 1
      AND COUNT(CASE WHEN COLUMN_NAME = 'lease_token' AND DATA_TYPE = 'char'
        AND CHARACTER_MAXIMUM_LENGTH = 36 AND IS_NULLABLE = 'YES' THEN 1 END) = 1
      AND COUNT(CASE WHEN COLUMN_NAME = 'lease_expires_at' AND DATA_TYPE = 'datetime'
        AND DATETIME_PRECISION = 6 AND IS_NULLABLE = 'YES' THEN 1 END) = 1
      AND COUNT(CASE WHEN COLUMN_NAME = 'platform_message_id' AND DATA_TYPE = 'varchar'
        AND CHARACTER_MAXIMUM_LENGTH = 512 AND IS_NULLABLE = 'YES' THEN 1 END) = 1
    FROM INFORMATION_SCHEMA.COLUMNS
    WHERE TABLE_SCHEMA = DATABASE()
      AND TABLE_NAME = 'secretary_owner_response_outbox'
  ), 0) <> 1
) AS owner_response_outbox_columns_verified;

SELECT (
  SELECT fail_row
  FROM (SELECT 1 AS fail_row UNION ALL SELECT 2 AS fail_row) AS mismatch
  WHERE COALESCE((
    SELECT COUNT(*) = 3
      AND COUNT(CASE WHEN INDEX_NAME = 'PRIMARY' AND NON_UNIQUE = 0
        AND INDEXED_COLUMNS = 'response_id' THEN 1 END) = 1
      AND COUNT(CASE WHEN INDEX_NAME = 'uk_secretary_owner_response_lease' AND NON_UNIQUE = 0
        AND INDEXED_COLUMNS = 'lease_token' THEN 1 END) = 1
      AND COUNT(CASE WHEN INDEX_NAME = 'idx_secretary_owner_response_claim' AND NON_UNIQUE = 1
        AND INDEXED_COLUMNS = 'delivery_status,next_eligible_at,lease_expires_at,response_id'
        THEN 1 END) = 1
    FROM (
      SELECT INDEX_NAME, MIN(NON_UNIQUE) AS NON_UNIQUE,
             GROUP_CONCAT(COLUMN_NAME ORDER BY SEQ_IN_INDEX SEPARATOR ',') AS INDEXED_COLUMNS
      FROM INFORMATION_SCHEMA.STATISTICS
      WHERE TABLE_SCHEMA = DATABASE()
        AND TABLE_NAME = 'secretary_owner_response_outbox'
      GROUP BY INDEX_NAME
    ) AS indexes_found
  ), 0) <> 1
) AS owner_response_outbox_indexes_verified;

SELECT (
  SELECT fail_row
  FROM (SELECT 1 AS fail_row UNION ALL SELECT 2 AS fail_row) AS mismatch
  WHERE COALESCE((
    SELECT COUNT(*) = 1
      AND COUNT(CASE WHEN rc.CONSTRAINT_NAME = 'fk_secretary_owner_response_outbox_response'
        AND kcu.COLUMN_NAME = 'response_id'
        AND kcu.REFERENCED_TABLE_NAME = 'secretary_action_responses'
        AND kcu.REFERENCED_COLUMN_NAME = 'response_id'
        AND rc.DELETE_RULE = 'CASCADE' THEN 1 END) = 1
    FROM INFORMATION_SCHEMA.REFERENTIAL_CONSTRAINTS rc
    JOIN INFORMATION_SCHEMA.KEY_COLUMN_USAGE kcu
      ON kcu.CONSTRAINT_SCHEMA = rc.CONSTRAINT_SCHEMA
     AND kcu.TABLE_NAME = rc.TABLE_NAME
     AND kcu.CONSTRAINT_NAME = rc.CONSTRAINT_NAME
    WHERE rc.CONSTRAINT_SCHEMA = DATABASE()
      AND rc.TABLE_NAME = 'secretary_owner_response_outbox'
  ), 0) <> 1
) AS owner_response_outbox_fk_verified;
