-- Action Planner 验收发现的内容策略约束修正。
-- Retriever 已支持消息级 never_long_term；数据库必须允许该值，才能保证
-- conversation/message 两侧取更严格策略的矩阵完整可达。

ALTER TABLE secretary_message_contents
    DROP CHECK chk_secretary_content_mode;

ALTER TABLE secretary_message_contents
    ADD CONSTRAINT chk_secretary_content_mode
        CHECK (content_mode IN ('normal', 'local_only', 'envelope_only', 'never_long_term'));

-- 回滚（执行前确认不存在 content_mode='never_long_term' 的行）：
-- ALTER TABLE secretary_message_contents DROP CHECK chk_secretary_content_mode;
-- ALTER TABLE secretary_message_contents ADD CONSTRAINT chk_secretary_content_mode
--   CHECK (content_mode IN ('normal', 'local_only', 'envelope_only'));
