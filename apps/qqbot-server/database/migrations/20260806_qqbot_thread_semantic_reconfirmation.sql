-- THR-010：线程逻辑迁移后，Owner 显式重新确认语义派生。
-- 失效表保留为不可变审计；本表只记录确认边界，读取时以时间顺序收敛。
ALTER TABLE `secretary_thread_owner_controls`
  DROP CHECK `chk_secretary_thread_control_kind`,
  ADD CONSTRAINT `chk_secretary_thread_control_kind` CHECK (
    `control_kind` IN (
      'confirm_decision', 'revoke_decision', 'dismiss_question',
      'reconfirm_thread_semantics', 'close_thread', 'reopen_thread'
    )
  );

SELECT (
  SELECT fail_row
  FROM (SELECT 1 AS fail_row UNION ALL SELECT 2 AS fail_row) AS mismatch
  WHERE COALESCE((
    SELECT COUNT(*) = 1
       AND COUNT(CASE WHEN cc.CHECK_CLAUSE LIKE '%confirm_decision%'
                          AND cc.CHECK_CLAUSE LIKE '%revoke_decision%'
                          AND cc.CHECK_CLAUSE LIKE '%dismiss_question%'
                          AND cc.CHECK_CLAUSE LIKE '%reconfirm_thread_semantics%'
                          AND cc.CHECK_CLAUSE LIKE '%close_thread%'
                          AND cc.CHECK_CLAUSE LIKE '%reopen_thread%' THEN 1 END) = 1
    FROM INFORMATION_SCHEMA.TABLE_CONSTRAINTS tc
    JOIN INFORMATION_SCHEMA.CHECK_CONSTRAINTS cc
      ON cc.CONSTRAINT_SCHEMA = tc.CONSTRAINT_SCHEMA
     AND cc.CONSTRAINT_NAME = tc.CONSTRAINT_NAME
    WHERE tc.CONSTRAINT_SCHEMA = DATABASE()
      AND tc.TABLE_NAME = 'secretary_thread_owner_controls'
      AND tc.CONSTRAINT_NAME = 'chk_secretary_thread_control_kind'
  ), 0) <> 1
) AS thread_semantic_reconfirmation_control_kind_verified;

CREATE TABLE IF NOT EXISTS `secretary_thread_semantic_reconfirmations` (
  `reconfirmation_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `thread_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `command_source_event_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `effect_id` varchar(255) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `reason` varchar(1000) COLLATE utf8mb4_unicode_ci NOT NULL,
  `created_at` datetime(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
  PRIMARY KEY (`reconfirmation_id`),
  UNIQUE KEY `uk_secretary_thread_semantic_reconfirmation_effect` (`effect_id`),
  KEY `idx_secretary_thread_semantic_reconfirmation_thread`
    (`thread_id`,`created_at`,`reconfirmation_id`),
  CONSTRAINT `fk_secretary_thread_reconfirmation_thread`
    FOREIGN KEY (`thread_id`) REFERENCES `secretary_event_threads` (`thread_id`)
    ON DELETE RESTRICT,
  CONSTRAINT `fk_secretary_thread_reconfirmation_command`
    FOREIGN KEY (`command_source_event_id`) REFERENCES `secretary_source_events` (`source_event_id`)
    ON DELETE RESTRICT
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci
  COMMENT='Owner 重新确认线程语义的不可变审计边界';

-- Fail closed if an existing database has a different reconfirmation shape.
-- This also makes a lost migration-ledger write observable on replay instead
-- of allowing CREATE TABLE IF NOT EXISTS to silently accept drift.
SELECT (
  SELECT fail_row
  FROM (SELECT 1 AS fail_row UNION ALL SELECT 2 AS fail_row) AS mismatch
  WHERE COALESCE((
    SELECT COUNT(*) = 6
       AND COUNT(CASE WHEN COLUMN_NAME = 'reconfirmation_id'
                          AND COLUMN_TYPE = 'char(36)'
                          AND IS_NULLABLE = 'NO' THEN 1 END) = 1
       AND COUNT(CASE WHEN COLUMN_NAME = 'thread_id'
                          AND COLUMN_TYPE = 'char(36)'
                          AND IS_NULLABLE = 'NO' THEN 1 END) = 1
       AND COUNT(CASE WHEN COLUMN_NAME = 'command_source_event_id'
                          AND COLUMN_TYPE = 'char(36)'
                          AND IS_NULLABLE = 'NO' THEN 1 END) = 1
       AND COUNT(CASE WHEN COLUMN_NAME = 'effect_id'
                          AND COLUMN_TYPE = 'varchar(255)'
                          AND IS_NULLABLE = 'NO' THEN 1 END) = 1
       AND COUNT(CASE WHEN COLUMN_NAME = 'reason'
                          AND COLUMN_TYPE = 'varchar(1000)'
                          AND IS_NULLABLE = 'NO' THEN 1 END) = 1
       AND COUNT(CASE WHEN COLUMN_NAME = 'created_at'
                          AND COLUMN_TYPE = 'datetime(6)'
                          AND IS_NULLABLE = 'NO' THEN 1 END) = 1
    FROM INFORMATION_SCHEMA.COLUMNS
    WHERE TABLE_SCHEMA = DATABASE()
      AND TABLE_NAME = 'secretary_thread_semantic_reconfirmations'
  ), 0) <> 1
) AS thread_semantic_reconfirmation_columns_verified;

SELECT (
  SELECT fail_row
  FROM (SELECT 1 AS fail_row UNION ALL SELECT 2 AS fail_row) AS mismatch
  WHERE COALESCE((
    SELECT COUNT(*) = 4
       AND COUNT(CASE WHEN INDEX_NAME = 'PRIMARY'
                          AND NON_UNIQUE = 0
                          AND indexed_columns = 'reconfirmation_id' THEN 1 END) = 1
       AND COUNT(CASE WHEN INDEX_NAME = 'uk_secretary_thread_semantic_reconfirmation_effect'
                          AND NON_UNIQUE = 0
                          AND indexed_columns = 'effect_id' THEN 1 END) = 1
       AND COUNT(CASE WHEN INDEX_NAME = 'idx_secretary_thread_semantic_reconfirmation_thread'
                          AND NON_UNIQUE = 1
                          AND indexed_columns = 'thread_id,created_at,reconfirmation_id' THEN 1 END) = 1
       AND COUNT(CASE WHEN INDEX_NAME = 'fk_secretary_thread_reconfirmation_command'
                          AND NON_UNIQUE = 1
                          AND indexed_columns = 'command_source_event_id' THEN 1 END) = 1
    FROM (
      SELECT INDEX_NAME, MIN(NON_UNIQUE) AS NON_UNIQUE,
             GROUP_CONCAT(COLUMN_NAME ORDER BY SEQ_IN_INDEX SEPARATOR ',') AS indexed_columns
      FROM INFORMATION_SCHEMA.STATISTICS
      WHERE TABLE_SCHEMA = DATABASE()
        AND TABLE_NAME = 'secretary_thread_semantic_reconfirmations'
      GROUP BY INDEX_NAME
    ) AS indexes_found
  ), 0) <> 1
) AS thread_semantic_reconfirmation_indexes_verified;

SELECT (
  SELECT fail_row
  FROM (SELECT 1 AS fail_row UNION ALL SELECT 2 AS fail_row) AS mismatch
  WHERE COALESCE((
    SELECT COUNT(*) = 2
       AND COUNT(CASE WHEN CONSTRAINT_NAME = 'fk_secretary_thread_reconfirmation_thread'
                          AND REFERENCED_TABLE_NAME = 'secretary_event_threads'
                          AND COLUMN_NAME = 'thread_id' THEN 1 END) = 1
       AND COUNT(CASE WHEN CONSTRAINT_NAME = 'fk_secretary_thread_reconfirmation_command'
                          AND REFERENCED_TABLE_NAME = 'secretary_source_events'
                          AND COLUMN_NAME = 'command_source_event_id' THEN 1 END) = 1
    FROM INFORMATION_SCHEMA.KEY_COLUMN_USAGE
    WHERE CONSTRAINT_SCHEMA = DATABASE()
      AND TABLE_NAME = 'secretary_thread_semantic_reconfirmations'
      AND CONSTRAINT_NAME IN (
        'fk_secretary_thread_reconfirmation_thread',
        'fk_secretary_thread_reconfirmation_command'
      )
  ), 0) <> 1
) AS thread_semantic_reconfirmation_foreign_keys_verified;
