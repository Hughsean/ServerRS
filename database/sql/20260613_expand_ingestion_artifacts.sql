-- ServerRS Web Ingestion - expand persisted text artifacts
-- Date: 2026-06-13
--
-- Existing installations may have created these columns as TEXT, whose
-- 65,535-byte limit is smaller than the fetcher's default 5 MiB body limit.
-- MODIFY COLUMN is safe to run repeatedly and preserves existing data.
--
-- Execution:
--   mysql -u root -p digital_companion < database/sql/20260613_expand_ingestion_artifacts.sql

ALTER TABLE knowledge_ingestion_runs
  MODIFY COLUMN fetched_body_text MEDIUMTEXT NULL
    COMMENT '原始抓取的网页正文（max ~5MB from fetcher）',
  MODIFY COLUMN clean_text MEDIUMTEXT NULL
    COMMENT '经 extractor 清洗后的纯文本';
