-- Non-message OneBot notices (for example group_upload) carry no stable message_id.
-- They signal a bounded history repair for a specific conversation without replacing the
-- frozen Gap boundary or fabricating a SourceEvent.
CREATE TABLE IF NOT EXISTS `secretary_gap_signal_scopes` (
  `gap_id` char(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `conversation_id` bigint unsigned NOT NULL,
  `signal_kind` varchar(64) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
  `created_at` datetime(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
  `updated_at` datetime(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6) ON UPDATE CURRENT_TIMESTAMP(6),
  PRIMARY KEY (`gap_id`, `conversation_id`),
  KEY `idx_secretary_gap_signal_scope_conversation` (`conversation_id`),
  CONSTRAINT `chk_secretary_gap_signal_scope_kind`
    CHECK (`signal_kind` IN ('non_message_reference')),
  CONSTRAINT `fk_secretary_gap_signal_scope_gap`
    FOREIGN KEY (`gap_id`) REFERENCES `secretary_ingestion_gaps` (`gap_id`) ON DELETE CASCADE,
  CONSTRAINT `fk_secretary_gap_signal_scope_conversation`
    FOREIGN KEY (`conversation_id`) REFERENCES `secretary_conversations` (`id`) ON DELETE CASCADE
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci
  COMMENT='Non-message notice requested conversation repair scopes for an active ingestion gap';

-- Existing table with a mismatched shape must not be accepted as a valid durable signal ledger.
SELECT (
  SELECT fail_row
  FROM (SELECT 1 AS fail_row UNION ALL SELECT 2 AS fail_row) AS mismatch
  WHERE COALESCE((
    SELECT COUNT(*) = 5
      AND COUNT(CASE WHEN
        (column_name = 'gap_id' AND data_type = 'char'
          AND character_maximum_length = 36 AND character_set_name = 'ascii'
          AND collation_name = 'ascii_bin' AND is_nullable = 'NO')
        OR (column_name = 'conversation_id' AND column_type = 'bigint unsigned'
          AND is_nullable = 'NO')
        OR (column_name = 'signal_kind' AND data_type = 'varchar'
          AND character_maximum_length = 64 AND character_set_name = 'ascii'
          AND collation_name = 'ascii_bin' AND is_nullable = 'NO')
        OR (column_name = 'created_at' AND data_type = 'datetime'
          AND datetime_precision = 6 AND is_nullable = 'NO')
        OR (column_name = 'updated_at' AND data_type = 'datetime'
          AND datetime_precision = 6 AND is_nullable = 'NO')
        THEN 1 END) = 5
    FROM information_schema.columns
    WHERE table_schema = DATABASE()
      AND table_name = 'secretary_gap_signal_scopes'
  ), 0) <> 1
) AS non_message_history_signal_columns_verified;

SELECT (
  SELECT fail_row
  FROM (SELECT 1 AS fail_row UNION ALL SELECT 2 AS fail_row) AS mismatch
  WHERE COALESCE((
    SELECT COUNT(*) = 1
    FROM information_schema.statistics
    WHERE table_schema = DATABASE()
      AND table_name = 'secretary_gap_signal_scopes'
      AND index_name = 'idx_secretary_gap_signal_scope_conversation'
      AND seq_in_index = 1
      AND column_name = 'conversation_id'
  ), 0) <> 1
) AS non_message_history_signal_index_verified;

SELECT (
  SELECT fail_row
  FROM (SELECT 1 AS fail_row UNION ALL SELECT 2 AS fail_row) AS mismatch
  WHERE COALESCE((
    SELECT COUNT(*) = 2
      AND MIN(CASE WHEN seq_in_index = 1 THEN column_name END) = 'gap_id'
      AND MIN(CASE WHEN seq_in_index = 2 THEN column_name END) = 'conversation_id'
    FROM information_schema.statistics
    WHERE table_schema = DATABASE()
      AND table_name = 'secretary_gap_signal_scopes'
      AND index_name = 'PRIMARY'
  ), 0) <> 1
) AS non_message_history_signal_primary_key_verified;

SELECT (
  SELECT fail_row
  FROM (SELECT 1 AS fail_row UNION ALL SELECT 2 AS fail_row) AS mismatch
  WHERE COALESCE((
    SELECT COUNT(*) = 2
      AND COUNT(CASE WHEN
        (rc.constraint_name = 'fk_secretary_gap_signal_scope_gap'
          AND kcu.column_name = 'gap_id'
          AND rc.referenced_table_name = 'secretary_ingestion_gaps'
          AND kcu.referenced_column_name = 'gap_id'
          AND rc.delete_rule = 'CASCADE')
        OR (rc.constraint_name = 'fk_secretary_gap_signal_scope_conversation'
          AND kcu.column_name = 'conversation_id'
          AND rc.referenced_table_name = 'secretary_conversations'
          AND kcu.referenced_column_name = 'id'
          AND rc.delete_rule = 'CASCADE')
        THEN 1 END) = 2
    FROM information_schema.referential_constraints rc
    INNER JOIN information_schema.key_column_usage kcu
      ON kcu.constraint_schema = rc.constraint_schema
      AND kcu.table_name = rc.table_name
      AND kcu.constraint_name = rc.constraint_name
    WHERE rc.constraint_schema = DATABASE()
      AND rc.table_name = 'secretary_gap_signal_scopes'
  ), 0) <> 1
) AS non_message_history_signal_foreign_keys_verified;

SELECT (
  SELECT fail_row
  FROM (SELECT 1 AS fail_row UNION ALL SELECT 2 AS fail_row) AS mismatch
  WHERE COALESCE((
    SELECT COUNT(*) = 1
      AND COUNT(CASE WHEN
        tc.constraint_name = 'chk_secretary_gap_signal_scope_kind'
        AND cc.check_clause LIKE '%non_message_reference%'
        THEN 1 END) = 1
    FROM information_schema.table_constraints tc
    INNER JOIN information_schema.check_constraints cc
      ON cc.constraint_schema = tc.constraint_schema
      AND cc.constraint_name = tc.constraint_name
    WHERE tc.constraint_schema = DATABASE()
      AND tc.table_name = 'secretary_gap_signal_scopes'
      AND tc.constraint_type = 'CHECK'
  ), 0) <> 1
) AS non_message_history_signal_check_verified;
