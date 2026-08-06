-- OPS-004：Owner 有界重处理失败 Artifact 派生任务的不可变审计。
CREATE TABLE IF NOT EXISTS `secretary_artifact_reprocess_audit` (
  `audit_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `effect_id` varchar(255) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `run_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `proposal_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `account_id` bigint unsigned NOT NULL,
  `command_source_event_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `requested_limit` smallint unsigned NOT NULL,
  `requeued_count` smallint unsigned NOT NULL,
  `requeued_source_event_ids` json NOT NULL,
  `reason` varchar(1000) NOT NULL,
  `created_at` datetime(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
  PRIMARY KEY (`audit_id`),
  UNIQUE KEY `uk_secretary_artifact_reprocess_effect` (`effect_id`),
  KEY `idx_secretary_artifact_reprocess_account` (`account_id`, `created_at`),
  CONSTRAINT `fk_secretary_artifact_reprocess_account`
    FOREIGN KEY (`account_id`) REFERENCES `secretary_accounts` (`id`) ON DELETE CASCADE,
  CONSTRAINT `fk_secretary_artifact_reprocess_command`
    FOREIGN KEY (`command_source_event_id`) REFERENCES `secretary_source_events` (`source_event_id`) ON DELETE RESTRICT,
  CONSTRAINT `fk_secretary_artifact_reprocess_run`
    FOREIGN KEY (`run_id`) REFERENCES `secretary_action_runs` (`run_id`) ON DELETE RESTRICT,
  CONSTRAINT `chk_secretary_artifact_reprocess_limit` CHECK (`requested_limit` BETWEEN 1 AND 100),
  CONSTRAINT `chk_secretary_artifact_reprocess_count` CHECK (`requeued_count` <= `requested_limit`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci
  COMMENT='OPS-004 失败 Artifact 派生重处理不可变审计';

-- 结构不符时必须失败，禁止 IF NOT EXISTS 静默掩盖错误结构。
SELECT (
  SELECT fail_row FROM (SELECT 1 AS fail_row UNION ALL SELECT 2 AS fail_row) AS mismatch
  WHERE COALESCE((
    SELECT COUNT(*) = 11
      AND COUNT(CASE WHEN COLUMN_NAME = 'audit_id' AND DATA_TYPE = 'char' AND CHARACTER_MAXIMUM_LENGTH = 36 AND IS_NULLABLE = 'NO' THEN 1 END) = 1
      AND COUNT(CASE WHEN COLUMN_NAME = 'effect_id' AND DATA_TYPE = 'varchar' AND CHARACTER_MAXIMUM_LENGTH = 255 AND IS_NULLABLE = 'NO' THEN 1 END) = 1
      AND COUNT(CASE WHEN COLUMN_NAME = 'requested_limit' AND DATA_TYPE = 'smallint' AND COLUMN_TYPE = 'smallint unsigned' THEN 1 END) = 1
      AND COUNT(CASE WHEN COLUMN_NAME = 'requeued_count' AND DATA_TYPE = 'smallint' AND COLUMN_TYPE = 'smallint unsigned' THEN 1 END) = 1
      AND COUNT(CASE WHEN COLUMN_NAME = 'requeued_source_event_ids' AND DATA_TYPE = 'json' AND IS_NULLABLE = 'NO' THEN 1 END) = 1
      AND COUNT(CASE WHEN COLUMN_NAME = 'reason' AND DATA_TYPE = 'varchar' AND CHARACTER_MAXIMUM_LENGTH = 1000 AND IS_NULLABLE = 'NO' THEN 1 END) = 1
    FROM INFORMATION_SCHEMA.COLUMNS
    WHERE TABLE_SCHEMA = DATABASE() AND TABLE_NAME = 'secretary_artifact_reprocess_audit'
  ), 0) <> 1
) AS artifact_reprocess_columns_verified;

SELECT (
  SELECT fail_row FROM (SELECT 1 AS fail_row UNION ALL SELECT 2 AS fail_row) AS mismatch
  WHERE COALESCE((
    SELECT COUNT(*) = 3
      AND COUNT(CASE WHEN CONSTRAINT_NAME = 'fk_secretary_artifact_reprocess_account' AND DELETE_RULE = 'CASCADE' THEN 1 END) = 1
      AND COUNT(CASE WHEN CONSTRAINT_NAME = 'fk_secretary_artifact_reprocess_command' AND DELETE_RULE = 'RESTRICT' THEN 1 END) = 1
      AND COUNT(CASE WHEN CONSTRAINT_NAME = 'fk_secretary_artifact_reprocess_run' AND DELETE_RULE = 'RESTRICT' THEN 1 END) = 1
    FROM INFORMATION_SCHEMA.REFERENTIAL_CONSTRAINTS
    WHERE CONSTRAINT_SCHEMA = DATABASE() AND TABLE_NAME = 'secretary_artifact_reprocess_audit'
  ), 0) <> 1
) AS artifact_reprocess_fk_verified;
