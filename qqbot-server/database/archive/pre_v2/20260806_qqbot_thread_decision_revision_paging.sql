-- THR-005: stable keyset pagination for immutable decision revisions.
-- MySQL 8.4 has no ADD INDEX IF NOT EXISTS. Rebuild the existing named secondary
-- index atomically so a lost migration-ledger write remains safely replayable.
ALTER TABLE `secretary_thread_decisions`
  DROP INDEX `idx_secretary_thread_decision_thread`,
  ADD INDEX `idx_secretary_thread_decision_thread`
    (`thread_id`,`created_at`,`decision_id`,`status`);

-- Fail closed if the named index has a different shape or uniqueness.
SELECT (
  SELECT fail_row
  FROM (SELECT 1 AS fail_row UNION ALL SELECT 2 AS fail_row) AS mismatch
  WHERE COALESCE((
    SELECT COUNT(*) = 1
      AND COUNT(CASE WHEN NON_UNIQUE = 1
        AND indexed_columns = 'thread_id,created_at,decision_id,status' THEN 1 END) = 1
    FROM (
      SELECT INDEX_NAME, MIN(NON_UNIQUE) AS NON_UNIQUE,
             GROUP_CONCAT(COLUMN_NAME ORDER BY SEQ_IN_INDEX SEPARATOR ',') AS indexed_columns
      FROM INFORMATION_SCHEMA.STATISTICS
      WHERE TABLE_SCHEMA = DATABASE()
        AND TABLE_NAME = 'secretary_thread_decisions'
        AND INDEX_NAME = 'idx_secretary_thread_decision_thread'
      GROUP BY INDEX_NAME
    ) AS indexes_found
  ), 0) <> 1
) AS thread_decision_revision_page_index_verified;
