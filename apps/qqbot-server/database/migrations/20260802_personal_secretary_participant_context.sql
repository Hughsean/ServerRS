-- QQ Personal Secretary account-scoped participant profile observations + structural relation view.
-- 依赖 ingestion（secretary_source_events / secretary_message_contents）、
-- threading（secretary_effective_thread_events / secretary_event_threads）。
--
-- 身份 = SourceAccountRef + PlatformIdentityKind + 平台稳定主体 ID。
-- 昵称、群名片、备注和别名只用于显示与指代候选解析，绝不构成授权；
-- 群角色只描述群内权限，不提升为系统 Owner。同一平台 ID 在不同被管理
-- 账号下是不同参与者（表按 account_id 强制隔离）。

CREATE TABLE IF NOT EXISTS secretary_participant_profiles
(
    profile_id           BIGINT UNSIGNED PRIMARY KEY AUTO_INCREMENT,
    account_id           BIGINT UNSIGNED NOT NULL,
    -- 平台身份种类（owner/official_bot/external）是身份命名空间的一部分：
    -- 身份 = account + platform_identity_kind + 平台稳定主体 ID，同账号下不同
    -- 命名空间出现相同稳定字符串也不合并。这是身份键，不是权限角色；
    -- 群内权限由会话观察表 group_role 表达，绝不用于判定系统 Owner。
    platform_identity_kind VARCHAR(16) NOT NULL,
    actor_platform_id    VARCHAR(191) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    display_name         VARCHAR(200) NOT NULL DEFAULT '',
    aliases_json         JSON NOT NULL,
    trust                VARCHAR(16) NOT NULL DEFAULT 'observed',
    confirmed            TINYINT(1) NOT NULL DEFAULT 0,
    invalidated          TINYINT(1) NOT NULL DEFAULT 0,
    invalidation_reason  VARCHAR(200) NULL,
    source_event_ids_json JSON NOT NULL,
    -- 建立当前显示名的来源事件（首次观察或最近一次旋转触发事件）；
    -- 显示名变化时旧显示名进入 aliases 且来源精确引用本列，而非触发变化的新事件。
    -- 当前显示名的有效性独立校验本列：来源列表是有界的，可能淘汰掉更早的建立事件。
    established_by_event_id CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NULL,
    directory_snapshot_id CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NULL,
    current              TINYINT(1) NOT NULL DEFAULT 1,
    -- 仅当前版本参与唯一约束：current=1 时取 actor_platform_id，历史行（current=0）为
    -- NULL（MySQL 唯一键允许多个 NULL），因此任意数量的历史版本都不会撞唯一键。
    current_head         VARCHAR(191) GENERATED ALWAYS AS
                         (IF(current = 1, actor_platform_id, NULL)) STORED,
    first_seen_at        DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    updated_at           DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6)
        ON UPDATE CURRENT_TIMESTAMP(6),

    CONSTRAINT chk_secretary_participant_actor_kind
        CHECK (platform_identity_kind IN ('owner', 'official_bot', 'external')),
    CONSTRAINT chk_secretary_participant_trust
        CHECK (trust IN ('verified', 'observed', 'inferred')),
    CONSTRAINT chk_secretary_participant_alias_count
        CHECK (JSON_LENGTH(aliases_json) <= 10),
    CONSTRAINT fk_secretary_participant_account
        FOREIGN KEY (account_id) REFERENCES secretary_accounts(id) ON DELETE CASCADE,
    UNIQUE KEY uk_secretary_participant_current
        (account_id, platform_identity_kind, current_head),
    INDEX idx_secretary_participant_actor
        (account_id, platform_identity_kind, actor_platform_id, current),
    INDEX idx_secretary_participant_updated (account_id, updated_at)
) ENGINE = InnoDB
  DEFAULT CHARSET = utf8mb4
  COLLATE = utf8mb4_unicode_ci
  COMMENT = '账号作用域参与者稳定档案（昵称/别名）；群名片与群角色见会话观察表';

-- 会话（群）作用域观察资料：同一 Actor 在不同群的名片/角色互不覆盖；
-- 群属性按 account_id + conversation_id + platform_identity_kind + actor_id 唯一，
-- 绝不污染私聊或其他群。established_by_event_id 记录建立当前名片/角色的来源事件，
-- 读取侧独立校验该事件；来源列表有界并淘汰最旧时，当前值的有效性不受影响。
CREATE TABLE IF NOT EXISTS secretary_participant_conversation_observations
(
    observation_id       BIGINT UNSIGNED PRIMARY KEY AUTO_INCREMENT,
    account_id           BIGINT UNSIGNED NOT NULL,
    conversation_id      BIGINT UNSIGNED NOT NULL,
    platform_identity_kind VARCHAR(16) NOT NULL,
    actor_platform_id    VARCHAR(191) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    group_card           VARCHAR(200) NULL,
    group_role           VARCHAR(16) NOT NULL DEFAULT 'unknown',
    established_by_event_id CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NULL,
    source_event_ids_json JSON NOT NULL,
    invalidated          TINYINT(1) NOT NULL DEFAULT 0,
    invalidation_reason  VARCHAR(200) NULL,
    first_seen_at        DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    updated_at           DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6)
        ON UPDATE CURRENT_TIMESTAMP(6),

    CONSTRAINT chk_secretary_observation_group_role
        CHECK (group_role IN ('owner', 'admin', 'member', 'unknown')),
    CONSTRAINT chk_secretary_observation_kind
        CHECK (platform_identity_kind IN ('owner', 'official_bot', 'external')),
    CONSTRAINT chk_secretary_observation_source_count
        CHECK (JSON_LENGTH(source_event_ids_json) <= 10),
    CONSTRAINT fk_secretary_observation_account
        FOREIGN KEY (account_id) REFERENCES secretary_accounts(id) ON DELETE CASCADE,
    CONSTRAINT fk_secretary_observation_conversation
        FOREIGN KEY (conversation_id) REFERENCES secretary_conversations(id) ON DELETE CASCADE,
    UNIQUE KEY uk_secretary_observation_scope
        (account_id, conversation_id, platform_identity_kind, actor_platform_id),
    INDEX idx_secretary_observation_actor
        (account_id, platform_identity_kind, actor_platform_id),
    INDEX idx_secretary_observation_updated (account_id, updated_at)
) ENGINE = InnoDB
  DEFAULT CHARSET = utf8mb4
  COLLATE = utf8mb4_unicode_ci
  COMMENT = '会话作用域群名片/群角色观察；只显示不授权，按会话隔离';

-- 结构关系可重建投影：sent_by / replies_to / member_of_thread / thread_root_by / mentions。
-- 只读 VIEW，不产生第二个可变副本；语义角色（requested_by / assigned_to / promised_by /
-- benefits）只在查询时从已确认 Thread Claim 与承诺记忆派生，不落入本视图。
CREATE OR REPLACE VIEW secretary_event_relations AS
-- SentBy：事件由该发送者发出（结构事实，账号强制过滤）。
SELECT e.account_id, e.source_event_id,
       e.actor_platform_id AS subject_actor_id,
       e.actor_kind AS subject_actor_kind,
       'sent_by' AS relation_kind,
       NULL AS thread_id,
       1 AS confirmed,
       e.occurred_at_unix_secs
FROM secretary_source_events e
UNION ALL
-- RepliesTo：事件回复父事件，客体是父事件发送者（同账号强制）。
SELECT e.account_id, e.source_event_id,
       p.actor_platform_id AS subject_actor_id,
       p.actor_kind AS subject_actor_kind,
       'replies_to' AS relation_kind,
       NULL AS thread_id,
       1 AS confirmed,
       e.occurred_at_unix_secs
FROM secretary_source_events e
JOIN secretary_source_events p
  ON p.source_event_id = e.reply_to_event_id AND p.account_id = e.account_id
UNION ALL
-- MemberOfThread：事件属于有效线程（合并/拆分后的有效线程）。
SELECT e.account_id, e.source_event_id,
       e.actor_platform_id AS subject_actor_id,
       e.actor_kind AS subject_actor_kind,
       'member_of_thread' AS relation_kind,
       ev.thread_id AS thread_id,
       1 AS confirmed,
       e.occurred_at_unix_secs
FROM secretary_source_events e
JOIN secretary_effective_thread_events ev ON ev.source_event_id = e.source_event_id
UNION ALL
-- ThreadRootBy：根事件，客体是线程发起人（发起人不是 Owner 判定）。
SELECT e.account_id, e.source_event_id,
       e.actor_platform_id AS subject_actor_id,
       e.actor_kind AS subject_actor_kind,
       'thread_root_by' AS relation_kind,
       t.thread_id AS thread_id,
       1 AS confirmed,
       e.occurred_at_unix_secs
FROM secretary_source_events e
JOIN secretary_effective_thread_events ev ON ev.source_event_id = e.source_event_id
JOIN secretary_event_threads t
  ON t.thread_id = ev.thread_id AND t.account_id = e.account_id
WHERE e.source_event_id = t.root_event_id
UNION ALL
-- Mentions：@ 到的人。协议只携带 actor_id，不携带身份种类，统一按 external
-- 观察处理；仅产生 mentions 关系，绝不自动成为负责人/承诺人/受益方。
SELECT e.account_id, e.source_event_id,
       jt.actor_id AS subject_actor_id,
       'external' AS subject_actor_kind,
       'mentions' AS relation_kind,
       NULL AS thread_id,
       1 AS confirmed,
       e.occurred_at_unix_secs
FROM secretary_source_events e
JOIN secretary_message_contents m ON m.source_event_id = e.source_event_id
JOIN JSON_TABLE(CAST(m.mentioned_actor_ids AS CHAR), '$[*]'
    COLUMNS (actor_id VARCHAR(191) PATH '$')) AS jt;

-- 回滚顺序（仅在确认不再需要参与者档案后人工执行）：
-- DROP VIEW secretary_event_relations;
-- DROP TABLE secretary_participant_conversation_observations;
-- DROP TABLE secretary_participant_profiles;
