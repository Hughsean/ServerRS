-- 允许撤回通知作为统一 SourceEvent 入库。
-- 原 CHECK 只接受 event_type='message'；B3 要求撤回本身也是可审计 SourceEvent。
-- Forward-only. Depends on 20260723_personal_secretary_ingestion.sql.
--
-- message_role 对 recall 行使用 external_observation：
-- 撤回是对既有消息的观察事实，不是 Owner 指令或助手输出。

ALTER TABLE secretary_source_events
    DROP CHECK chk_secretary_source_event_type;

ALTER TABLE secretary_source_events
    ADD CONSTRAINT chk_secretary_source_event_type
        CHECK (event_type IN ('message', 'recall'));

-- 回滚（执行前确认不存在 event_type='recall' 的行）：
-- ALTER TABLE secretary_source_events DROP CHECK chk_secretary_source_event_type;
-- ALTER TABLE secretary_source_events ADD CONSTRAINT chk_secretary_source_event_type
--   CHECK (event_type IN ('message'));
