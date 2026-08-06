-- THR-002: admit only typed, strong structured-reference evidence.
-- Re-running either ALTER is safe: the named check is replaced atomically.
ALTER TABLE `secretary_thread_link_hints`
  DROP CHECK `chk_secretary_thread_link_hint_kind`,
  ADD CONSTRAINT `chk_secretary_thread_link_hint_kind`
    CHECK (`signal_kind` IN (
      'explicit_project_id',
      'exact_file_source_key',
      'explicit_file_version',
      'exact_forward_source_key',
      'exact_rich_content_key'
    ));

ALTER TABLE `secretary_thread_link_candidates`
  DROP CHECK `chk_secretary_thread_link_candidate_kind`,
  ADD CONSTRAINT `chk_secretary_thread_link_candidate_kind`
    CHECK (`signal_kind` IN (
      'explicit_project_id',
      'exact_file_source_key',
      'explicit_file_version',
      'exact_forward_source_key',
      'exact_rich_content_key'
    ));

-- Fail closed if an existing database exposes a different constraint under the expected name.
SELECT (
  SELECT fail_row
  FROM (SELECT 1 AS fail_row UNION ALL SELECT 2 AS fail_row) AS mismatch
  WHERE COALESCE((
    SELECT COUNT(*) = 2
      AND COUNT(CASE WHEN tc.TABLE_NAME = 'secretary_thread_link_hints'
        AND cc.CHECK_CLAUSE LIKE '%explicit_project_id%'
        AND cc.CHECK_CLAUSE LIKE '%exact_file_source_key%'
        AND cc.CHECK_CLAUSE LIKE '%explicit_file_version%'
        AND cc.CHECK_CLAUSE LIKE '%exact_forward_source_key%'
        AND cc.CHECK_CLAUSE LIKE '%exact_rich_content_key%'
        AND cc.CHECK_CLAUSE NOT LIKE '%shared_actor%'
        AND cc.CHECK_CLAUSE NOT LIKE '%similar_topic%'
        AND cc.CHECK_CLAUSE NOT LIKE '%same_file_name%' THEN 1 END) = 1
      AND COUNT(CASE WHEN tc.TABLE_NAME = 'secretary_thread_link_candidates'
        AND cc.CHECK_CLAUSE LIKE '%explicit_project_id%'
        AND cc.CHECK_CLAUSE LIKE '%exact_file_source_key%'
        AND cc.CHECK_CLAUSE LIKE '%explicit_file_version%'
        AND cc.CHECK_CLAUSE LIKE '%exact_forward_source_key%'
        AND cc.CHECK_CLAUSE LIKE '%exact_rich_content_key%'
        AND cc.CHECK_CLAUSE NOT LIKE '%shared_actor%'
        AND cc.CHECK_CLAUSE NOT LIKE '%similar_topic%'
        AND cc.CHECK_CLAUSE NOT LIKE '%same_file_name%' THEN 1 END) = 1
    FROM INFORMATION_SCHEMA.TABLE_CONSTRAINTS tc
    JOIN INFORMATION_SCHEMA.CHECK_CONSTRAINTS cc
      ON cc.CONSTRAINT_SCHEMA = tc.CONSTRAINT_SCHEMA
     AND cc.CONSTRAINT_NAME = tc.CONSTRAINT_NAME
    WHERE tc.CONSTRAINT_SCHEMA = DATABASE()
      AND tc.CONSTRAINT_NAME IN (
        'chk_secretary_thread_link_hint_kind',
        'chk_secretary_thread_link_candidate_kind'
      )
  ), 0) <> 1
) AS thread_link_structured_reference_constraints_verified;
