-- GAP-007-IMPL-C ordinary-message Spool legacy epoch recovery fencing.
CREATE TABLE IF NOT EXISTS `secretary_realtime_spool_recovery_claims` (
  `connection_epoch_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `account_id` bigint unsigned NOT NULL,
  `lease_token` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `lease_expires_at` datetime(6) NOT NULL,
  `created_at` datetime(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
  `updated_at` datetime(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6) ON UPDATE CURRENT_TIMESTAMP(6),
  PRIMARY KEY (`connection_epoch_id`),
  KEY `idx_secretary_realtime_spool_claim_lease` (`lease_expires_at`,`connection_epoch_id`),
  KEY `idx_secretary_realtime_spool_claim_account` (`account_id`,`lease_expires_at`),
  UNIQUE KEY `uk_secretary_realtime_spool_claim_token` (`lease_token`),
  CONSTRAINT `fk_secretary_realtime_spool_claim_epoch`
    FOREIGN KEY (`connection_epoch_id`) REFERENCES `secretary_connection_epochs` (`connection_epoch_id`)
    ON DELETE CASCADE,
  CONSTRAINT `fk_secretary_realtime_spool_claim_account`
    FOREIGN KEY (`account_id`) REFERENCES `secretary_accounts` (`id`)
    ON DELETE CASCADE
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci
  COMMENT='Ordinary-message Spool startup recovery lease and fencing token';

SELECT (
  SELECT fail_row
  FROM (SELECT 1 AS fail_row UNION ALL SELECT 2 AS fail_row) AS mismatch
  WHERE COALESCE((
    SELECT COUNT(*) = 6
      AND COUNT(CASE WHEN COLUMN_NAME = 'connection_epoch_id' AND DATA_TYPE = 'char'
        AND CHARACTER_MAXIMUM_LENGTH = 36 AND IS_NULLABLE = 'NO'
        AND CHARACTER_SET_NAME = 'ascii' AND COLLATION_NAME = 'ascii_bin' THEN 1 END) = 1
      AND COUNT(CASE WHEN COLUMN_NAME = 'account_id' AND COLUMN_TYPE = 'bigint unsigned'
        AND IS_NULLABLE = 'NO' THEN 1 END) = 1
      AND COUNT(CASE WHEN COLUMN_NAME = 'lease_token' AND DATA_TYPE = 'char'
        AND CHARACTER_MAXIMUM_LENGTH = 36 AND IS_NULLABLE = 'NO'
        AND CHARACTER_SET_NAME = 'ascii' AND COLLATION_NAME = 'ascii_bin' THEN 1 END) = 1
      AND COUNT(CASE WHEN COLUMN_NAME = 'lease_expires_at' AND DATA_TYPE = 'datetime'
        AND DATETIME_PRECISION = 6 AND IS_NULLABLE = 'NO' THEN 1 END) = 1
      AND COUNT(CASE WHEN COLUMN_NAME = 'created_at' AND DATA_TYPE = 'datetime'
        AND DATETIME_PRECISION = 6 AND IS_NULLABLE = 'NO' THEN 1 END) = 1
      AND COUNT(CASE WHEN COLUMN_NAME = 'updated_at' AND DATA_TYPE = 'datetime'
        AND DATETIME_PRECISION = 6 AND IS_NULLABLE = 'NO' THEN 1 END) = 1
    FROM INFORMATION_SCHEMA.COLUMNS
    WHERE TABLE_SCHEMA = DATABASE()
      AND TABLE_NAME = 'secretary_realtime_spool_recovery_claims'
  ), 0) <> 1
) AS realtime_spool_recovery_columns_verified;

SELECT (
  SELECT fail_row
  FROM (SELECT 1 AS fail_row UNION ALL SELECT 2 AS fail_row) AS mismatch
  WHERE COALESCE((
    SELECT COUNT(*) = 4
      AND COUNT(CASE WHEN INDEX_NAME = 'PRIMARY' AND NON_UNIQUE = 0
        AND INDEXED_COLUMNS = 'connection_epoch_id' THEN 1 END) = 1
      AND COUNT(CASE WHEN INDEX_NAME = 'idx_secretary_realtime_spool_claim_lease'
        AND NON_UNIQUE = 1 AND INDEXED_COLUMNS = 'lease_expires_at,connection_epoch_id' THEN 1 END) = 1
      AND COUNT(CASE WHEN INDEX_NAME = 'idx_secretary_realtime_spool_claim_account'
        AND NON_UNIQUE = 1 AND INDEXED_COLUMNS = 'account_id,lease_expires_at' THEN 1 END) = 1
      AND COUNT(CASE WHEN INDEX_NAME = 'uk_secretary_realtime_spool_claim_token'
        AND NON_UNIQUE = 0 AND INDEXED_COLUMNS = 'lease_token' THEN 1 END) = 1
    FROM (
      SELECT INDEX_NAME, MIN(NON_UNIQUE) AS NON_UNIQUE,
             GROUP_CONCAT(COLUMN_NAME ORDER BY SEQ_IN_INDEX SEPARATOR ',') AS INDEXED_COLUMNS
      FROM INFORMATION_SCHEMA.STATISTICS
      WHERE TABLE_SCHEMA = DATABASE()
        AND TABLE_NAME = 'secretary_realtime_spool_recovery_claims'
      GROUP BY INDEX_NAME
    ) AS indexes_found
  ), 0) <> 1
) AS realtime_spool_recovery_indexes_verified;

SELECT (
  SELECT fail_row
  FROM (SELECT 1 AS fail_row UNION ALL SELECT 2 AS fail_row) AS mismatch
  WHERE COALESCE((
    SELECT COUNT(*) = 2
      AND COUNT(CASE WHEN rc.CONSTRAINT_NAME = 'fk_secretary_realtime_spool_claim_epoch'
        AND kcu.COLUMN_NAME = 'connection_epoch_id'
        AND kcu.REFERENCED_TABLE_NAME = 'secretary_connection_epochs'
        AND kcu.REFERENCED_COLUMN_NAME = 'connection_epoch_id'
        AND rc.DELETE_RULE = 'CASCADE' THEN 1 END) = 1
      AND COUNT(CASE WHEN rc.CONSTRAINT_NAME = 'fk_secretary_realtime_spool_claim_account'
        AND kcu.COLUMN_NAME = 'account_id'
        AND kcu.REFERENCED_TABLE_NAME = 'secretary_accounts'
        AND kcu.REFERENCED_COLUMN_NAME = 'id'
        AND rc.DELETE_RULE = 'CASCADE' THEN 1 END) = 1
    FROM INFORMATION_SCHEMA.REFERENTIAL_CONSTRAINTS rc
    JOIN INFORMATION_SCHEMA.KEY_COLUMN_USAGE kcu
      ON kcu.CONSTRAINT_SCHEMA = rc.CONSTRAINT_SCHEMA
     AND kcu.TABLE_NAME = rc.TABLE_NAME
     AND kcu.CONSTRAINT_NAME = rc.CONSTRAINT_NAME
    WHERE rc.CONSTRAINT_SCHEMA = DATABASE()
      AND rc.TABLE_NAME = 'secretary_realtime_spool_recovery_claims'
  ), 0) <> 1
) AS realtime_spool_recovery_fk_verified;
