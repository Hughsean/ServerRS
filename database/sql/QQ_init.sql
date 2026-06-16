-- ============================================================================
-- QQ_init.sql — QQ Bot (赛博猫猫) 模块数据库初始化
--
-- Must be executed AFTER init.sql (depends on `users` table).
--   mysql -u root -p < database/sql/init.sql
--   mysql -u root -p < database/sql/QQ_init.sql
-- ============================================================================

USE digital_companion;
SET NAMES utf8mb4;

-- ============================================================================
-- 1. qq_bot_accounts — 机器人账号注册信息
-- ============================================================================
CREATE TABLE qq_bot_accounts
(
    bot_account_id  BIGINT UNSIGNED PRIMARY KEY AUTO_INCREMENT COMMENT '机器人账号ID',
    platform        VARCHAR(32)     NOT NULL DEFAULT 'qq' COMMENT '平台标识',
    self_qq_id      BIGINT          NOT NULL UNIQUE COMMENT '机器人自身QQ号',
    display_name    VARCHAR(128)    NULL COMMENT '显示名称',
    adapter         VARCHAR(64)     NOT NULL DEFAULT 'napcat' COMMENT '适配器类型',
    connection_mode VARCHAR(32)     NOT NULL DEFAULT 'websocket' COMMENT '连接方式',
    enabled         TINYINT(1)      NOT NULL DEFAULT 1 COMMENT '是否启用',
    config          JSON            NULL COMMENT '额外配置',
    created_at      DATETIME(6)     NOT NULL DEFAULT CURRENT_TIMESTAMP(6) COMMENT '创建时间',
    updated_at      DATETIME(6)     NOT NULL DEFAULT CURRENT_TIMESTAMP(6) ON UPDATE CURRENT_TIMESTAMP(6) COMMENT '更新时间',
    INDEX idx_self_qq_id (self_qq_id),
    INDEX idx_enabled (enabled)
) ENGINE = InnoDB
  DEFAULT CHARSET = utf8mb4
  COLLATE = utf8mb4_unicode_ci COMMENT = '机器人账号注册信息表';

-- ============================================================================
-- 2. qq_external_users — QQ外部用户基础信息（画像基础）
-- ============================================================================
CREATE TABLE qq_external_users
(
    qq_user_id      BIGINT          NOT NULL PRIMARY KEY COMMENT 'QQ号',
    internal_user_id BIGINT UNSIGNED NULL COMMENT '关联平台用户ID',
    nickname        VARCHAR(100)    NULL COMMENT 'QQ昵称',
    avatar_url      VARCHAR(512)    NULL COMMENT '头像URL',
    last_seen_at    BIGINT          NULL COMMENT '最后活跃时间戳',
    memory_enabled  TINYINT(1)      NOT NULL DEFAULT 0 COMMENT '记忆功能启用',
    persona_enabled TINYINT(1)      NOT NULL DEFAULT 0 COMMENT '人格功能启用',
    created_at      DATETIME(6)     NOT NULL DEFAULT CURRENT_TIMESTAMP(6) COMMENT '创建时间',
    updated_at      DATETIME(6)     NOT NULL DEFAULT CURRENT_TIMESTAMP(6) ON UPDATE CURRENT_TIMESTAMP(6) COMMENT '更新时间',
    FOREIGN KEY (internal_user_id) REFERENCES users (id) ON DELETE SET NULL,
    INDEX idx_internal_user_id (internal_user_id),
    INDEX idx_last_seen_at (last_seen_at)
) ENGINE = InnoDB
  DEFAULT CHARSET = utf8mb4
  COLLATE = utf8mb4_unicode_ci COMMENT = 'QQ外部用户基础信息表';

-- ============================================================================
-- 3. qq_groups — 群聊配置
-- ============================================================================
CREATE TABLE qq_groups
(
    qq_group_id         BIGINT          NOT NULL PRIMARY KEY COMMENT '群号',
    group_name          VARCHAR(200)    NULL COMMENT '群名称',
    bot_account_id      BIGINT UNSIGNED NOT NULL COMMENT '所属机器人账号ID',
    enabled             TINYINT(1)      NOT NULL DEFAULT 1 COMMENT '是否启用',
    trigger_policy      VARCHAR(20)     NOT NULL DEFAULT 'mention' COMMENT '触发策略: mention/keyword/command/always/silent',
    cooldown_secs       BIGINT UNSIGNED NOT NULL DEFAULT 30 COMMENT '回复冷却秒数',
    max_segments        INT UNSIGNED    NOT NULL DEFAULT 5 COMMENT '最大回复段数',
    max_chars_per_segment INT UNSIGNED  NOT NULL DEFAULT 80 COMMENT '每段最大字符数',
    allow_proactive     TINYINT(1)      NOT NULL DEFAULT 0 COMMENT '是否允许主动发送',
    keywords            JSON            NULL COMMENT '触发关键词列表',
    memory_policy       VARCHAR(15)     NOT NULL DEFAULT 'off' COMMENT '记忆策略: off/group_only/opt_in_user',
    last_seen_at        BIGINT          NULL COMMENT '最后活跃时间戳',
    created_at          DATETIME(6)     NOT NULL DEFAULT CURRENT_TIMESTAMP(6) COMMENT '创建时间',
    updated_at          DATETIME(6)     NOT NULL DEFAULT CURRENT_TIMESTAMP(6) ON UPDATE CURRENT_TIMESTAMP(6) COMMENT '更新时间',
    FOREIGN KEY (bot_account_id) REFERENCES qq_bot_accounts (bot_account_id) ON DELETE CASCADE,
    INDEX idx_bot_account_id (bot_account_id),
    INDEX idx_enabled (enabled)
) ENGINE = InnoDB
  DEFAULT CHARSET = utf8mb4
  COLLATE = utf8mb4_unicode_ci COMMENT = '群聊配置表';

-- ============================================================================
-- 4. qq_group_members — 群成员关系
-- ============================================================================
CREATE TABLE qq_group_members
(
    qq_group_id   BIGINT       NOT NULL COMMENT '群号',
    qq_user_id    BIGINT       NOT NULL COMMENT 'QQ号',
    card          VARCHAR(100) NULL COMMENT '群名片',
    nickname      VARCHAR(100) NULL COMMENT 'QQ昵称',
    role          VARCHAR(10)  NULL COMMENT '群角色: owner/admin/member',
    title         VARCHAR(100) NULL COMMENT '群头衔',
    join_time     BIGINT       NULL COMMENT '加群时间戳',
    last_seen_at  BIGINT       NULL COMMENT '最后发言时间戳',
    status        VARCHAR(10)  NOT NULL DEFAULT 'active' COMMENT '状态: active/left/kicked/unknown',
    created_at    DATETIME(6)  NOT NULL DEFAULT CURRENT_TIMESTAMP(6) COMMENT '创建时间',
    updated_at    DATETIME(6)  NOT NULL DEFAULT CURRENT_TIMESTAMP(6) ON UPDATE CURRENT_TIMESTAMP(6) COMMENT '更新时间',
    PRIMARY KEY (qq_group_id, qq_user_id),
    FOREIGN KEY (qq_group_id) REFERENCES qq_groups (qq_group_id) ON DELETE CASCADE,
    FOREIGN KEY (qq_user_id) REFERENCES qq_external_users (qq_user_id) ON DELETE CASCADE,
    INDEX idx_qq_user_id (qq_user_id),
    INDEX idx_role (role),
    INDEX idx_status (status)
) ENGINE = InnoDB
  DEFAULT CHARSET = utf8mb4
  COLLATE = utf8mb4_unicode_ci COMMENT = '群成员关系表';

-- ============================================================================
-- 5. qq_group_messages — 群消息存储（幂等）
-- ============================================================================
CREATE TABLE qq_group_messages
(
    id                  BIGINT UNSIGNED PRIMARY KEY AUTO_INCREMENT COMMENT '消息ID',
    bot_account_id      BIGINT UNSIGNED NOT NULL COMMENT '机器人账号ID',
    qq_group_id         BIGINT          NOT NULL COMMENT '群号',
    qq_user_id          BIGINT          NULL COMMENT '发送者QQ号',
    platform_message_id VARCHAR(64)     NOT NULL COMMENT 'OneBot消息ID',
    direction           VARCHAR(10)     NOT NULL COMMENT '方向: inbound/outbound',
    raw_text            TEXT            NOT NULL COMMENT '原始消息文本',
    normalized_text     TEXT            NOT NULL COMMENT '清洗后文本',
    segments            JSON            NOT NULL COMMENT '解析后的消息段',
    at_bot              TINYINT(1)      NOT NULL DEFAULT 0 COMMENT '是否@机器人',
    command_name        VARCHAR(50)     NULL COMMENT '命令名',
    sent_at             BIGINT          NOT NULL COMMENT '发送时间戳',
    status              VARCHAR(15)     NOT NULL DEFAULT 'pending' COMMENT '处理状态: pending/ignored/processed/failed',
    created_at          DATETIME(6)     NOT NULL DEFAULT CURRENT_TIMESTAMP(6) COMMENT '创建时间',
    FOREIGN KEY (bot_account_id) REFERENCES qq_bot_accounts (bot_account_id) ON DELETE CASCADE,
    FOREIGN KEY (qq_group_id) REFERENCES qq_groups (qq_group_id) ON DELETE CASCADE,
    FOREIGN KEY (qq_user_id) REFERENCES qq_external_users (qq_user_id) ON DELETE SET NULL,
    UNIQUE KEY uk_platform_message (bot_account_id, platform_message_id),
    INDEX idx_qq_group_id (qq_group_id),
    INDEX idx_qq_user_id (qq_user_id),
    INDEX idx_sent_at (sent_at),
    INDEX idx_status (status)
) ENGINE = InnoDB
  DEFAULT CHARSET = utf8mb4
  COLLATE = utf8mb4_unicode_ci COMMENT = '群消息存储表';

-- ============================================================================
-- 6. qq_agent_turns — 对话轮次记录
-- ============================================================================
CREATE TABLE qq_agent_turns
(
    turn_id             BIGINT UNSIGNED PRIMARY KEY AUTO_INCREMENT COMMENT '轮次ID',
    bot_account_id      BIGINT UNSIGNED NOT NULL COMMENT '机器人账号ID',
    qq_group_id         BIGINT          NOT NULL COMMENT '群号',
    trigger_message_id  BIGINT UNSIGNED NOT NULL COMMENT '触发消息ID',
    response_message_id BIGINT UNSIGNED NULL COMMENT '回复消息ID',
    trigger_type        VARCHAR(10)     NOT NULL COMMENT '触发类型: mention/keyword/command/always/manual',
    qq_user_id          BIGINT          NULL COMMENT '触发者QQ号',
    internal_user_id    BIGINT UNSIGNED NULL COMMENT '关联平台用户ID',
    prompt_version      VARCHAR(50)     NULL COMMENT '提示词版本',
    model_name          VARCHAR(100)    NULL COMMENT '模型名称',
    reasoning_enabled   TINYINT(1)      NULL COMMENT '是否启用推理',
    input_token_count   INT UNSIGNED    NULL COMMENT '输入Token数',
    output_token_count  INT UNSIGNED    NULL COMMENT '输出Token数',
    latency_ms          INT UNSIGNED    NULL COMMENT '延迟毫秒',
    status              VARCHAR(15)     NOT NULL DEFAULT 'created' COMMENT '状态: created/responded/failed/cancelled',
    error_message       TEXT            NULL COMMENT '错误信息',
    trace_id            VARCHAR(64)     NULL UNIQUE COMMENT '追踪ID',
    created_at          DATETIME(6)     NOT NULL DEFAULT CURRENT_TIMESTAMP(6) COMMENT '创建时间',
    updated_at          DATETIME(6)     NOT NULL DEFAULT CURRENT_TIMESTAMP(6) ON UPDATE CURRENT_TIMESTAMP(6) COMMENT '更新时间',
    FOREIGN KEY (bot_account_id) REFERENCES qq_bot_accounts (bot_account_id) ON DELETE CASCADE,
    FOREIGN KEY (trigger_message_id) REFERENCES qq_group_messages (id) ON DELETE CASCADE,
    FOREIGN KEY (response_message_id) REFERENCES qq_group_messages (id) ON DELETE SET NULL,
    FOREIGN KEY (internal_user_id) REFERENCES users (id) ON DELETE SET NULL,
    INDEX idx_qq_group_id (qq_group_id),
    INDEX idx_qq_user_id (qq_user_id),
    INDEX idx_status (status),
    INDEX idx_created_at (created_at)
) ENGINE = InnoDB
  DEFAULT CHARSET = utf8mb4
  COLLATE = utf8mb4_unicode_ci COMMENT = '对话轮次记录表';

-- ============================================================================
-- 7. qq_message_outbox — 出站消息重试队列
-- ============================================================================
CREATE TABLE qq_message_outbox
(
    outbox_id          BIGINT UNSIGNED PRIMARY KEY AUTO_INCREMENT COMMENT '出站ID',
    bot_account_id     BIGINT UNSIGNED NOT NULL COMMENT '机器人账号ID',
    qq_group_id        BIGINT          NULL COMMENT '目标群号',
    qq_user_id         BIGINT          NULL COMMENT '目标QQ号',
    target_type        VARCHAR(10)     NOT NULL DEFAULT 'group' COMMENT '目标类型: group/private',
    payload            JSON            NOT NULL COMMENT '消息载荷',
    related_turn_id    BIGINT UNSIGNED NULL COMMENT '关联轮次ID',
    status             VARCHAR(15)     NOT NULL DEFAULT 'pending' COMMENT '状态: pending/sending/sent/failed/cancelled',
    attempts           INT UNSIGNED    NOT NULL DEFAULT 0 COMMENT '已尝试次数',
    max_attempts       INT UNSIGNED    NOT NULL DEFAULT 3 COMMENT '最大尝试次数',
    next_run_at        BIGINT          NOT NULL COMMENT '下次执行时间戳',
    platform_message_id VARCHAR(64)    NULL COMMENT '发送成功后的平台消息ID',
    last_error         TEXT            NULL COMMENT '最后错误信息',
    created_at         DATETIME(6)     NOT NULL DEFAULT CURRENT_TIMESTAMP(6) COMMENT '创建时间',
    updated_at         DATETIME(6)     NOT NULL DEFAULT CURRENT_TIMESTAMP(6) ON UPDATE CURRENT_TIMESTAMP(6) COMMENT '更新时间',
    FOREIGN KEY (bot_account_id) REFERENCES qq_bot_accounts (bot_account_id) ON DELETE CASCADE,
    FOREIGN KEY (related_turn_id) REFERENCES qq_agent_turns (turn_id) ON DELETE SET NULL,
    INDEX idx_status_next_run (status, next_run_at),
    INDEX idx_related_turn_id (related_turn_id)
) ENGINE = InnoDB
  DEFAULT CHARSET = utf8mb4
  COLLATE = utf8mb4_unicode_ci COMMENT = '出站消息重试队列表';

-- ============================================================================
-- 8. qq_group_summaries — 群聊滚动摘要
-- ============================================================================
CREATE TABLE qq_group_summaries
(
    summary_id      BIGINT UNSIGNED PRIMARY KEY AUTO_INCREMENT COMMENT '摘要ID',
    qq_group_id     BIGINT          NOT NULL COMMENT '群号',
    summary_type    VARCHAR(30)     NOT NULL COMMENT '摘要类型: rolling_group/milestone_group',
    content         TEXT            NOT NULL COMMENT '摘要内容',
    message_start_id BIGINT UNSIGNED NOT NULL COMMENT '起始消息ID',
    message_end_id   BIGINT UNSIGNED NOT NULL COMMENT '结束消息ID',
    supersedes_id   BIGINT UNSIGNED NULL COMMENT '取代的旧摘要ID',
    token_count     INT UNSIGNED    NULL COMMENT 'Token数',
    status          TINYINT(1)      NOT NULL DEFAULT 1 COMMENT '是否活跃: 1=active, 0=disabled',
    vector_id       VARCHAR(64)     NULL COMMENT '向量存储ID',
    created_at      DATETIME(6)     NOT NULL DEFAULT CURRENT_TIMESTAMP(6) COMMENT '创建时间',
    FOREIGN KEY (qq_group_id) REFERENCES qq_groups (qq_group_id) ON DELETE CASCADE,
    FOREIGN KEY (supersedes_id) REFERENCES qq_group_summaries (summary_id) ON DELETE SET NULL,
    INDEX idx_qq_group_id (qq_group_id),
    INDEX idx_status (status)
) ENGINE = InnoDB
  DEFAULT CHARSET = utf8mb4
  COLLATE = utf8mb4_unicode_ci COMMENT = '群聊滚动摘要表';

-- ============================================================================
-- 9. qq_group_memories — 群聊记忆（画像核心存储）
-- ============================================================================
CREATE TABLE qq_group_memories
(
    group_memory_id  BIGINT UNSIGNED PRIMARY KEY AUTO_INCREMENT COMMENT '记忆ID',
    qq_group_id      BIGINT          NOT NULL COMMENT '群号',
    memory_key       VARCHAR(100)    NULL COMMENT '记忆键',
    canonical_form   VARCHAR(200)    NULL COMMENT '规范化形式',
    memory_type      VARCHAR(30)     NOT NULL COMMENT '记忆类型: group_preference/group_fact/group_rule/recurring_topic/inside_joke',
    content          TEXT            NOT NULL COMMENT '记忆内容',
    confidence       DOUBLE          NOT NULL DEFAULT 0.0 COMMENT '置信度',
    salience         DOUBLE          NOT NULL DEFAULT 0.0 COMMENT '显著性',
    source_message_id BIGINT UNSIGNED NULL COMMENT '来源消息ID',
    reinforce_count  INT UNSIGNED    NOT NULL DEFAULT 0 COMMENT '强化次数',
    status           TINYINT         NOT NULL DEFAULT 1 COMMENT '状态: 1=active, 0=disabled, -1=contradicted',
    created_at       DATETIME(6)     NOT NULL DEFAULT CURRENT_TIMESTAMP(6) COMMENT '创建时间',
    updated_at       DATETIME(6)     NOT NULL DEFAULT CURRENT_TIMESTAMP(6) ON UPDATE CURRENT_TIMESTAMP(6) COMMENT '更新时间',
    FOREIGN KEY (qq_group_id) REFERENCES qq_groups (qq_group_id) ON DELETE CASCADE,
    FOREIGN KEY (source_message_id) REFERENCES qq_group_messages (id) ON DELETE SET NULL,
    INDEX idx_qq_group_id (qq_group_id),
    INDEX idx_memory_type (memory_type),
    INDEX idx_status (status),
    INDEX idx_salience (salience)
) ENGINE = InnoDB
  DEFAULT CHARSET = utf8mb4
  COLLATE = utf8mb4_unicode_ci COMMENT = '群聊记忆表';

-- ============================================================================
-- 10. qq_user_profiles — QQ用户画像数据（画像核心存储）
-- ============================================================================
CREATE TABLE qq_user_profiles
(
    qq_user_id        BIGINT          NOT NULL PRIMARY KEY COMMENT 'QQ号',
    interest_tags     JSON            NULL COMMENT '兴趣标签列表',
    active_hours      JSON            NULL COMMENT '活跃时段分布',
    speaking_style    VARCHAR(50)     NULL COMMENT '说话风格分类',
    topic_frequency   JSON            NULL COMMENT '话题频率统计',
    total_messages    INT UNSIGNED    NOT NULL DEFAULT 0 COMMENT '消息总数',
    avg_message_length DOUBLE         NOT NULL DEFAULT 0.0 COMMENT '平均消息长度',
    emoji_usage_rate  DOUBLE          NOT NULL DEFAULT 0.0 COMMENT '表情使用率',
    first_seen_at     BIGINT          NULL COMMENT '首次发现时间戳',
    last_summary_at   BIGINT          NULL COMMENT '上次画像更新时间戳',
    raw_profile       TEXT            NULL COMMENT 'LLM生成的完整画像文本',
    created_at        DATETIME(6)     NOT NULL DEFAULT CURRENT_TIMESTAMP(6) COMMENT '创建时间',
    updated_at        DATETIME(6)     NOT NULL DEFAULT CURRENT_TIMESTAMP(6) ON UPDATE CURRENT_TIMESTAMP(6) COMMENT '更新时间',
    FOREIGN KEY (qq_user_id) REFERENCES qq_external_users (qq_user_id) ON DELETE CASCADE
	) ENGINE = InnoDB
	  DEFAULT CHARSET = utf8mb4
	  COLLATE = utf8mb4_unicode_ci COMMENT = 'QQ用户画像数据表';

-- 11. 猫猫与群友关系表
CREATE TABLE IF NOT EXISTS qq_relationships (
    id                 BIGINT UNSIGNED AUTO_INCREMENT PRIMARY KEY COMMENT '自增主键',
    qq_group_id        BIGINT          NOT NULL COMMENT '群号',
    qq_user_id         BIGINT          NOT NULL COMMENT '群友QQ号',
    familiarity        FLOAT           NOT NULL DEFAULT 0.1 COMMENT '熟悉度 0.0~1.0',
    interaction_count  INT UNSIGNED    NOT NULL DEFAULT 0 COMMENT '累计互动次数',
    last_interaction_at BIGINT         NULL COMMENT '上次互动时间戳',
    rapport            VARCHAR(32)     NOT NULL DEFAULT 'neutral' COMMENT '亲密度等级: friendly/neutral/awkward/playful/respectful',
    nickname_preference VARCHAR(64)    NULL COMMENT '偏好的称呼',
    known_interests    JSON            NULL COMMENT '已知兴趣列表',
    known_avoid_topics JSON            NULL COMMENT '应避免的话题列表',
    created_at         DATETIME(6)     NOT NULL DEFAULT CURRENT_TIMESTAMP(6) COMMENT '创建时间',
    updated_at         DATETIME(6)     NOT NULL DEFAULT CURRENT_TIMESTAMP(6) ON UPDATE CURRENT_TIMESTAMP(6) COMMENT '更新时间',
    UNIQUE KEY uk_group_user (qq_group_id, qq_user_id),
    FOREIGN KEY (qq_user_id) REFERENCES qq_external_users (qq_user_id) ON DELETE CASCADE
) ENGINE = InnoDB
  DEFAULT CHARSET = utf8mb4
  COLLATE = utf8mb4_unicode_ci COMMENT = '猫猫与群友关系状态表';
