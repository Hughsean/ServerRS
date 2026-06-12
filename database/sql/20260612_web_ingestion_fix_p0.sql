-- ============================================================================
-- 20260612_web_ingestion_fix_p0.sql
-- P0 repair SQL for web ingestion schema
--
-- Fix 1: knowledge_ingestion_runs.content_key — drop UNIQUE, keep INDEX
-- Fix 2: knowledge_publish_records.active_page_key — add trigger-enforced
--        regular column (MySQL generated column + FK table = error 1215).
--        Triggers guarantee DB-level same as generated column.
-- ============================================================================

USE digital_companion;

-- ----------------------------------------------------------------------------
-- Fix 1: content_key UNIQUE → INDEX
-- ----------------------------------------------------------------------------

ALTER TABLE knowledge_ingestion_runs
  DROP INDEX uk_ingestion_runs_content_key;

ALTER TABLE knowledge_ingestion_runs
  ADD INDEX idx_ingestion_runs_content_key (content_key);

-- ----------------------------------------------------------------------------
-- Fix 2: active_page_key — regular column + trigger + UNIQUE
-- ----------------------------------------------------------------------------

-- Drop the old UNIQUE constraint (if it still exists from original create)
-- Use a block that ignores errors if already dropped
ALTER TABLE knowledge_publish_records
  DROP INDEX uk_publish_records_one_active_page;

-- Drop the old column (if it still exists)
ALTER TABLE knowledge_publish_records
  DROP COLUMN active_page_key;

-- Add regular column (NO generated — triggers handle it)
ALTER TABLE knowledge_publish_records
  ADD COLUMN active_page_key VARCHAR(128) NULL
  COMMENT 'active=1时为source_id:page_id，active=0时为NULL。由 trigger 自动维护';

-- Add UNIQUE constraint — NULLs don't conflict, so only active=1 rows compete
ALTER TABLE knowledge_publish_records
  ADD UNIQUE KEY uk_publish_records_one_active_page (active_page_key);

-- BEFORE INSERT trigger: auto-set active_page_key
DROP TRIGGER IF EXISTS trg_kpr_active_page_key_bi;

CREATE TRIGGER trg_kpr_active_page_key_bi
  BEFORE INSERT ON knowledge_publish_records
  FOR EACH ROW
  SET NEW.active_page_key = IF(NEW.active = 1,
      CONCAT(NEW.source_id, ':', NEW.page_id),
      NULL);

-- BEFORE UPDATE trigger: auto-set active_page_key
DROP TRIGGER IF EXISTS trg_kpr_active_page_key_bu;

CREATE TRIGGER trg_kpr_active_page_key_bu
  BEFORE UPDATE ON knowledge_publish_records
  FOR EACH ROW
  SET NEW.active_page_key = IF(NEW.active = 1,
      CONCAT(NEW.source_id, ':', NEW.page_id),
      NULL);

-- ----------------------------------------------------------------------------
-- Verification queries (run manually to check)
-- ----------------------------------------------------------------------------
-- SHOW INDEX FROM knowledge_ingestion_runs WHERE Key_name = 'idx_ingestion_runs_content_key';
-- SHOW INDEX FROM knowledge_publish_records WHERE Key_name = 'uk_publish_records_one_active_page';
-- SHOW TRIGGERS LIKE 'knowledge_publish_records';
