-- ServerRS Web Ingestion – Artifact persistence for mid-pipeline
-- Date: 2026-06-13
-- Purpose: Add columns to knowledge_ingestion_runs so that PageFetched →
--          PageCleaned → PageDistilled → QualityChecked handlers can persist
--          and retrieve intermediate artifacts without using outbox payload.
--
-- Execution:
--   mysql -u root -p digital_companion < database/sql/20260613_artifact_persistence.sql
--
-- After execution, regenerate SeaORM entities:
--   sea-orm-cli generate entity -o src/infrastructure/persistence/entities
--
-- Existing installations that already have these columns must also apply:
--   database/sql/20260613_expand_ingestion_artifacts.sql

ALTER TABLE knowledge_ingestion_runs
  ADD COLUMN fetched_body_text MEDIUMTEXT NULL COMMENT '原始抓取的网页正文（max ~5MB from fetcher）',
  ADD COLUMN clean_text MEDIUMTEXT NULL COMMENT '经 extractor 清洗后的纯文本',
  ADD COLUMN distilled_json JSON NULL COMMENT 'Distill LLM 返回的完整结构化 JSON（DistilledDocument）';
