-- QQ Open Platform Gateway transport state. Business messages still enter secretary_source_events.

CREATE TABLE IF NOT EXISTS secretary_qq_gateway_sessions
(
    app_id       VARCHAR(191) COLLATE utf8mb4_bin PRIMARY KEY,
    session_id   VARCHAR(512) COLLATE utf8mb4_bin NOT NULL,
    last_sequence BIGINT UNSIGNED NOT NULL,
    updated_at   DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6)
        ON UPDATE CURRENT_TIMESTAMP(6)
) ENGINE = InnoDB DEFAULT CHARSET = utf8mb4 COLLATE = utf8mb4_unicode_ci
  COMMENT = '官方 QQ Bot Gateway RESUME 会话；仅在原始消息可靠入库后推进 sequence';

CREATE TABLE IF NOT EXISTS secretary_qq_raw_events
(
    source_event_id CHAR(36) CHARACTER SET ascii COLLATE ascii_bin PRIMARY KEY,
    app_id          VARCHAR(191) COLLATE utf8mb4_bin NOT NULL,
    event_kind      VARCHAR(32) NOT NULL,
    envelope_json   JSON NOT NULL,
    received_at     DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    CONSTRAINT fk_secretary_qq_raw_source
        FOREIGN KEY (source_event_id) REFERENCES secretary_source_events(source_event_id)
        ON DELETE CASCADE,
    INDEX idx_secretary_qq_raw_app_time (app_id, received_at, source_event_id)
) ENGINE = InnoDB DEFAULT CHARSET = utf8mb4 COLLATE = utf8mb4_unicode_ci
  COMMENT = '官方 Gateway 无损原始事件；持久化成功后才推进 Resume sequence';

-- 回滚顺序：secretary_qq_raw_events -> secretary_qq_gateway_sessions。
