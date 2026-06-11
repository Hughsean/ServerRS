-- ============================================================================
-- init.sql — Complete database initialization for Digital Companion (ServerRS)
--
-- Generated from:
--   database/sql/1-create.sql                    (base tables)
--   database/patches/20260611_001_auth_role_refresh_likes.sql
--   database/patches/20260611_002_agent_rag_memory.sql
--   database/patches/20260611_003_qdrant_vector_index.sql
--   database/patches/20260611_004_agent_vector_lifecycle.sql
--   database/patches/20260611_005_stored_objects.sql
--
-- All patch columns/indices/constraints have been folded into the final
-- CREATE TABLE statements below.  This file produces the EXACT same schema
-- as applying base + all five patches in order.
--
-- Execution:
--   mysql -u root -p < database/sql/init.sql
-- ============================================================================

DROP DATABASE IF EXISTS digital_companion;
CREATE DATABASE IF NOT EXISTS digital_companion
    DEFAULT CHARACTER SET utf8mb4
    DEFAULT COLLATE utf8mb4_unicode_ci;

USE digital_companion;

-- ============================================================================
-- 1. users — 用户基础信息表
--    (role column from patch 001 already folded in)
-- ============================================================================
CREATE TABLE users
(
    id            BIGINT UNSIGNED PRIMARY KEY AUTO_INCREMENT COMMENT '用户ID',
    username      VARCHAR(50)  NOT NULL UNIQUE COMMENT '用户名',
    password      VARCHAR(255) NOT NULL COMMENT '密码(加密存储)',
    email         VARCHAR(100) UNIQUE COMMENT '邮箱',
    phone         VARCHAR(20) UNIQUE COMMENT '手机号',
    avatar        BLOB COMMENT '头像二进制数据',
    nickname      VARCHAR(50) COMMENT '昵称',
    created_at    TIMESTAMP    NOT NULL DEFAULT CURRENT_TIMESTAMP COMMENT '创建时间',
    updated_at    TIMESTAMP    NOT NULL DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP COMMENT '更新时间',
    last_login_at TIMESTAMP    NULL COMMENT '最后登录时间',
    status        TINYINT      NOT NULL DEFAULT 1 COMMENT '账号状态:1正常,0禁用',
    role          VARCHAR(32)  NOT NULL DEFAULT 'USER' COMMENT 'USER/ADMIN/SUPER_ADMIN',
    INDEX idx_username (username),
    INDEX idx_email (email),
    INDEX idx_phone (phone),
    INDEX idx_role (role)
) ENGINE = InnoDB
  DEFAULT CHARSET = utf8mb4
  COLLATE = utf8mb4_unicode_ci COMMENT = '用户基础信息表';

-- ============================================================================
-- 2. refresh_tokens — 刷新令牌表 (from patch 001)
-- ============================================================================
CREATE TABLE refresh_tokens
(
    refresh_token_id BIGINT UNSIGNED PRIMARY KEY AUTO_INCREMENT COMMENT '刷新令牌ID',
    token_id         VARCHAR(64)     NOT NULL COMMENT 'JWT jti',
    user_id          BIGINT UNSIGNED NOT NULL COMMENT '用户ID',
    token_hash       CHAR(64)        NOT NULL COMMENT '刷新令牌 SHA-256',
    expires_at       BIGINT UNSIGNED NOT NULL COMMENT '过期时间戳(秒)',
    revoked_at       DATETIME(6)     NULL COMMENT '吊销时间',
    created_at       DATETIME(6)     NOT NULL DEFAULT CURRENT_TIMESTAMP(6) COMMENT '创建时间',
    updated_at       DATETIME(6)     NOT NULL DEFAULT CURRENT_TIMESTAMP(6) ON UPDATE CURRENT_TIMESTAMP(6) COMMENT '更新时间',
    FOREIGN KEY (user_id) REFERENCES users (id) ON DELETE CASCADE,
    UNIQUE KEY uk_refresh_tokens_token_id (token_id),
    UNIQUE KEY uk_refresh_tokens_token_hash (token_hash),
    INDEX idx_refresh_tokens_user_id (user_id),
    INDEX idx_refresh_tokens_expires_at (expires_at)
) ENGINE = InnoDB
  DEFAULT CHARSET = utf8mb4
  COLLATE = utf8mb4_unicode_ci COMMENT = '刷新令牌表';

-- ============================================================================
-- 3. user_profiles — 用户画像表
-- ============================================================================
CREATE TABLE user_profiles
(
    id                      BIGINT UNSIGNED PRIMARY KEY AUTO_INCREMENT COMMENT '画像ID',
    user_id                 BIGINT UNSIGNED NOT NULL UNIQUE COMMENT '用户ID',
    interests               JSON COMMENT '兴趣爱好',
    personality_traits      JSON COMMENT '性格特征',
    interaction_preferences JSON COMMENT '交互偏好',
    emotional_tendency      JSON COMMENT '情感倾向',
    learning_records        JSON COMMENT '学习记录',
    created_at              TIMESTAMP       NOT NULL DEFAULT CURRENT_TIMESTAMP COMMENT '创建时间',
    updated_at              TIMESTAMP       NOT NULL DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP COMMENT '更新时间',
    FOREIGN KEY (user_id) REFERENCES users (id) ON DELETE CASCADE,
    INDEX idx_user_id (user_id)
) ENGINE = InnoDB
  DEFAULT CHARSET = utf8mb4
  COLLATE = utf8mb4_unicode_ci COMMENT = '用户画像表';

-- ============================================================================
-- 4. user_diaries — 用户日记表
-- ============================================================================
CREATE TABLE user_diaries
(
    id               BIGINT UNSIGNED PRIMARY KEY AUTO_INCREMENT COMMENT '日记ID',
    user_id          BIGINT UNSIGNED NOT NULL COMMENT '用户ID',
    title            VARCHAR(100)    NOT NULL DEFAULT '无标题' COMMENT '日记标题',
    content          TEXT            NOT NULL COMMENT '日记内容',
    mood_description VARCHAR(255) COMMENT '心情描述，使用大模型评估',
    created_at       TIMESTAMP       NOT NULL DEFAULT CURRENT_TIMESTAMP COMMENT '创建时间',
    updated_at       TIMESTAMP       NOT NULL DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP COMMENT '更新时间',
    FOREIGN KEY (user_id) REFERENCES users (id) ON DELETE CASCADE,
    INDEX idx_user_id (user_id),
    INDEX idx_created_at (created_at)
) ENGINE = InnoDB
  DEFAULT CHARSET = utf8mb4
  COLLATE = utf8mb4_unicode_ci COMMENT = '用户日记表';

-- ============================================================================
-- 5. conversations — 会话元数据表
-- ============================================================================
CREATE TABLE conversations
(
    id                 BIGINT UNSIGNED PRIMARY KEY AUTO_INCREMENT COMMENT '会话ID',
    user_id            BIGINT UNSIGNED NOT NULL COMMENT '用户ID',
    title              VARCHAR(100) COMMENT '对话标题，大模型生成',
    is_title_generated TINYINT(1)      NOT NULL DEFAULT 0 COMMENT '标题是否生成',
    last_message_at    TIMESTAMP       NULL COMMENT '最近一条消息时间',
    message_count      INT UNSIGNED    NOT NULL DEFAULT 0 COMMENT '消息数量',
    created_at         TIMESTAMP       NOT NULL DEFAULT CURRENT_TIMESTAMP COMMENT '创建时间',
    FOREIGN KEY (user_id) REFERENCES users (id) ON DELETE CASCADE,
    INDEX idx_user_id (user_id),
    INDEX idx_created_at (created_at)
) ENGINE = InnoDB
  DEFAULT CHARSET = utf8mb4
  COLLATE = utf8mb4_unicode_ci COMMENT = '会话元数据表';

-- ============================================================================
-- 6. conversation_messages — 会话消息表
-- ============================================================================
CREATE TABLE conversation_messages
(
    id              BIGINT UNSIGNED PRIMARY KEY AUTO_INCREMENT COMMENT '消息ID',
    conversation_id BIGINT UNSIGNED NOT NULL COMMENT '会话ID',
    sender_role     VARCHAR(32)     NOT NULL COMMENT 'user|assistant|system|plugin',
    sender_user_id  BIGINT UNSIGNED NULL COMMENT '当 sender_role=user 时可关联 users.id',
    message_type    VARCHAR(32)     NOT NULL DEFAULT 'text' COMMENT 'text|image|event 等',
    content         JSON            NOT NULL COMMENT '消息内容（结构自由）',
    token_count     INT UNSIGNED    NULL COMMENT '可选：token 数',
    created_at      TIMESTAMP       NOT NULL DEFAULT CURRENT_TIMESTAMP COMMENT '创建时间',
    FOREIGN KEY (conversation_id) REFERENCES conversations (id) ON DELETE CASCADE,
    INDEX idx_conv_created (conversation_id, created_at),
    INDEX idx_conv_sender (conversation_id, sender_role)
) ENGINE = InnoDB
  DEFAULT CHARSET = utf8mb4
  COLLATE = utf8mb4_unicode_ci COMMENT = '会话消息表';

-- ============================================================================
-- 7. community_posts — 用户交流社区帖子表
-- ============================================================================
CREATE TABLE community_posts
(
    post_id        BIGINT UNSIGNED PRIMARY KEY AUTO_INCREMENT COMMENT '帖子ID',
    user_id        BIGINT UNSIGNED NOT NULL COMMENT '用户ID',
    title          VARCHAR(150)             DEFAULT '无标题' COMMENT '帖子标题',
    content        TEXT            NOT NULL COMMENT '帖子文本内容',
    extra_metadata JSON COMMENT '其他附加信息(标签、地理位置等)',
    likes_count    INT UNSIGNED    NOT NULL DEFAULT 0 COMMENT '点赞数',
    comments_count INT UNSIGNED    NOT NULL DEFAULT 0 COMMENT '评论数',
    status         TINYINT         NOT NULL DEFAULT 1 COMMENT '帖子状态:1可见,0隐藏',
    created_at     TIMESTAMP       NOT NULL DEFAULT CURRENT_TIMESTAMP COMMENT '创建时间',
    updated_at     TIMESTAMP       NOT NULL DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP COMMENT '更新时间',
    FOREIGN KEY (user_id) REFERENCES users (id) ON DELETE CASCADE,
    INDEX idx_user_id (user_id),
    INDEX idx_created_at (created_at),
    INDEX idx_status (status)
) ENGINE = InnoDB
  DEFAULT CHARSET = utf8mb4
  COLLATE = utf8mb4_unicode_ci COMMENT = '用户交流社区帖子表';

-- ============================================================================
-- 8. community_post_media — 用户交流社区媒体表
-- ============================================================================
CREATE TABLE community_post_media
(
    media_id   BIGINT UNSIGNED PRIMARY KEY AUTO_INCREMENT COMMENT '媒体ID',
    post_id    BIGINT UNSIGNED NOT NULL COMMENT '帖子ID',
    media_type VARCHAR(20)     NOT NULL COMMENT '媒体类型:IMAGE/VIDEO',
    mime_type  VARCHAR(100)    NOT NULL COMMENT '媒体MIME类型',
    media_data LONGBLOB        NOT NULL COMMENT '媒体二进制数据',
    created_at TIMESTAMP       NOT NULL DEFAULT CURRENT_TIMESTAMP COMMENT '创建时间',
    FOREIGN KEY (post_id) REFERENCES community_posts (post_id) ON DELETE CASCADE,
    INDEX idx_post_id (post_id),
    INDEX idx_media_type (media_type)
) ENGINE = InnoDB
  DEFAULT CHARSET = utf8mb4
  COLLATE = utf8mb4_unicode_ci COMMENT = '用户交流社区媒体表';

-- ============================================================================
-- 9. stored_objects — 通用对象存储元数据表 (from patch 005)
-- ============================================================================
CREATE TABLE stored_objects
(
    object_id       BIGINT UNSIGNED PRIMARY KEY AUTO_INCREMENT COMMENT '对象ID',
    bucket          VARCHAR(64)      NOT NULL COMMENT '存储桶/业务分组',
    object_key      VARCHAR(512)     NOT NULL COMMENT '对象存储键',
    original_name   VARCHAR(255)     NOT NULL COMMENT '原始文件名',
    mime_type       VARCHAR(128)     NOT NULL COMMENT 'MIME类型',
    size_bytes      BIGINT UNSIGNED  NOT NULL COMMENT '文件大小',
    sha256          CHAR(64)         NOT NULL COMMENT '内容SHA-256',
    storage_backend VARCHAR(32)      NOT NULL DEFAULT 'LOCAL' COMMENT '存储后端',
    public_url      TEXT             NULL COMMENT '公开访问URL',
    created_by      BIGINT UNSIGNED  NULL COMMENT '上传用户ID',
    created_at      TIMESTAMP        NOT NULL DEFAULT CURRENT_TIMESTAMP COMMENT '创建时间',
    FOREIGN KEY (created_by) REFERENCES users (id) ON DELETE SET NULL,
    UNIQUE KEY uk_stored_objects_bucket_key (bucket, object_key),
    INDEX idx_stored_objects_created_by (created_by),
    INDEX idx_stored_objects_sha256 (sha256)
) ENGINE = InnoDB
  DEFAULT CHARSET = utf8mb4
  COLLATE = utf8mb4_unicode_ci COMMENT = '对象存储元数据表';

-- ============================================================================
-- 10. community_comments — 用户交流社区评论表
-- ============================================================================
CREATE TABLE community_comments
(
    comment_id        BIGINT UNSIGNED PRIMARY KEY AUTO_INCREMENT COMMENT '评论ID',
    post_id           BIGINT UNSIGNED NOT NULL COMMENT '帖子ID',
    user_id           BIGINT UNSIGNED NOT NULL COMMENT '用户ID',
    parent_comment_id BIGINT UNSIGNED NULL COMMENT '父评论ID，支持楼中楼',
    content           TEXT            NOT NULL COMMENT '评论内容',
    attachments       JSON COMMENT '附件资源(JSON数组，存储图片/视频等URL或标识)',
    likes_count       INT UNSIGNED    NOT NULL DEFAULT 0 COMMENT '点赞数',
    status            TINYINT         NOT NULL DEFAULT 1 COMMENT '评论状态:1可见,0隐藏',
    created_at        TIMESTAMP       NOT NULL DEFAULT CURRENT_TIMESTAMP COMMENT '创建时间',
    updated_at        TIMESTAMP       NOT NULL DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP COMMENT '更新时间',
    FOREIGN KEY (post_id) REFERENCES community_posts (post_id) ON DELETE CASCADE,
    FOREIGN KEY (user_id) REFERENCES users (id) ON DELETE CASCADE,
    FOREIGN KEY (parent_comment_id) REFERENCES community_comments (comment_id) ON DELETE CASCADE,
    INDEX idx_post_id (post_id),
    INDEX idx_parent_id (parent_comment_id),
    INDEX idx_created_at (created_at)
) ENGINE = InnoDB
  DEFAULT CHARSET = utf8mb4
  COLLATE = utf8mb4_unicode_ci COMMENT = '用户交流社区评论表';

-- ============================================================================
-- 11. depression_scales — 抑郁量表定义表
-- ============================================================================
CREATE TABLE depression_scales
(
    scale_id          SMALLINT UNSIGNED PRIMARY KEY AUTO_INCREMENT COMMENT '量表ID',
    scale_name        VARCHAR(50) NOT NULL UNIQUE COMMENT '量表名称(如PHQ-9/SDS/BDI)',
    scale_description TEXT COMMENT '量表描述',
    min_score         SMALLINT    NOT NULL COMMENT '最低分',
    max_score         SMALLINT    NOT NULL COMMENT '最高分',
    severity_ranges   JSON        NOT NULL COMMENT '严重程度分级标准',
    questions         JSON        NOT NULL COMMENT '问题列表',
    created_at        TIMESTAMP DEFAULT CURRENT_TIMESTAMP COMMENT '创建时间',
    updated_at        TIMESTAMP DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP COMMENT '更新时间'
) ENGINE = InnoDB
  DEFAULT CHARSET = utf8mb4
  COLLATE = utf8mb4_unicode_ci COMMENT = '抑郁量表定义表';

-- ============================================================================
-- 12. depression_assessments — 抑郁评估记录表
-- ============================================================================
CREATE TABLE depression_assessments
(
    assessment_id   BIGINT UNSIGNED   PRIMARY KEY AUTO_INCREMENT COMMENT '评估ID',
    user_id         BIGINT UNSIGNED   NOT NULL COMMENT '用户ID',
    scale_id        SMALLINT UNSIGNED NOT NULL COMMENT '使用的量表ID',
    assessment_date DATE              NOT NULL COMMENT '评估日期',
    answers         JSON              NOT NULL COMMENT '问题答案集合',
    total_score     SMALLINT          NOT NULL COMMENT '总得分',
    notes           TEXT COMMENT '附加说明',
    created_at      TIMESTAMP DEFAULT CURRENT_TIMESTAMP COMMENT '创建时间',
    updated_at      TIMESTAMP DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP COMMENT '更新时间',
    FOREIGN KEY (user_id) REFERENCES users (id) ON DELETE CASCADE,
    FOREIGN KEY (scale_id) REFERENCES depression_scales (scale_id),
    INDEX idx_user_assessment (user_id, assessment_date),
    INDEX idx_scale (scale_id)
) ENGINE = InnoDB
  DEFAULT CHARSET = utf8mb4
  COLLATE = utf8mb4_unicode_ci COMMENT = '抑郁评估记录表';

-- ============================================================================
-- 13. risk_detection_results — 风险检测结果表
-- ============================================================================
CREATE TABLE risk_detection_results
(
    id               BIGINT UNSIGNED PRIMARY KEY AUTO_INCREMENT COMMENT '检测结果ID',
    user_id          BIGINT UNSIGNED NOT NULL COMMENT '关联用户ID',
    message_id       BIGINT UNSIGNED NOT NULL COMMENT '关联消息ID(仅对 user 角色的消息进行检测)',
    conversation_id  BIGINT UNSIGNED NOT NULL COMMENT '冗余存储会话ID以便快速查询',
    risk_level       VARCHAR(16)     NOT NULL COMMENT '风险等级: NONE|LOW|MEDIUM|HIGH|CRISIS|UNKNOWN',
    polarity         VARCHAR(16)     NOT NULL COMMENT '情绪极性: POSITIVE|NEUTRAL|NEGATIVE|MIXED|UNKNOWN',
    intent           VARCHAR(32)     NOT NULL COMMENT '话语意图: HELP_SEEKING|VENTING|INFO_QUERY|NARRATIVE|JOKE_SARCASM|UNKNOWN',
    target           VARCHAR(32)     NOT NULL COMMENT '指向对象: SELF|OTHER_INDIVIDUAL|GROUP_ORG|UNKNOWN',
    confidence       DECIMAL(4, 3)   NOT NULL COMMENT '置信度 0-1',
    evidence         JSON            NULL COMMENT '证据短句数组(JSON字符串数组)',
    reason           VARCHAR(200)    NULL COMMENT '判断理由(简明说明判断依据)',
    raw_payload      JSON            NULL COMMENT '原始模型返回的 JSON 结果(溯源)',
    model_name       VARCHAR(64)     NULL COMMENT '使用的模型名称',
    detector_version VARCHAR(32)     NULL COMMENT '检测器版本号',
    is_processed     TINYINT(1)      NOT NULL DEFAULT 0 COMMENT '是否被医生或管理员处理',
    process_notes    TEXT            NULL COMMENT '处理备注',
    created_at       TIMESTAMP       NOT NULL DEFAULT CURRENT_TIMESTAMP COMMENT '创建时间',
    FOREIGN KEY (user_id) REFERENCES users (id) ON DELETE CASCADE,
    FOREIGN KEY (conversation_id) REFERENCES conversations (id) ON DELETE CASCADE,
    FOREIGN KEY (message_id) REFERENCES conversation_messages (id) ON DELETE CASCADE,
    INDEX idx_message_id (message_id),
    INDEX idx_user_created (user_id, created_at),
    INDEX idx_conversation (conversation_id),
    INDEX idx_risk_level (risk_level),
    INDEX idx_intent (intent),
    INDEX idx_is_processed (is_processed)
) ENGINE = InnoDB
  DEFAULT CHARSET = utf8mb4
  COLLATE = utf8mb4_unicode_ci COMMENT = '风险检测结果表';

-- ============================================================================
-- 14. psychology_categories — 心理知识库分类表
-- ============================================================================
CREATE TABLE psychology_categories
(
    category_id   SMALLINT UNSIGNED PRIMARY KEY AUTO_INCREMENT COMMENT '分类ID',
    category_name VARCHAR(50)       NOT NULL UNIQUE COMMENT '分类名称',
    parent_id     SMALLINT UNSIGNED NULL COMMENT '父分类ID，支持多级分类',
    description   TEXT COMMENT '分类描述',
    sort_order    INT               NOT NULL DEFAULT 0 COMMENT '排序顺序',
    status        TINYINT           NOT NULL DEFAULT 1 COMMENT '状态:1启用,0禁用',
    created_at    TIMESTAMP         NOT NULL DEFAULT CURRENT_TIMESTAMP COMMENT '创建时间',
    updated_at    TIMESTAMP         NOT NULL DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP COMMENT '更新时间',
    FOREIGN KEY (parent_id) REFERENCES psychology_categories (category_id) ON DELETE SET NULL,
    INDEX idx_parent_id (parent_id),
    INDEX idx_status (status)
) ENGINE = InnoDB
  DEFAULT CHARSET = utf8mb4
  COLLATE = utf8mb4_unicode_ci COMMENT = '心理知识库分类表';

-- ============================================================================
-- 15. psychology_articles — 心理知识库文章表
-- ============================================================================
CREATE TABLE psychology_articles
(
    article_id   BIGINT UNSIGNED PRIMARY KEY AUTO_INCREMENT COMMENT '文章ID',
    category_id  SMALLINT UNSIGNED NOT NULL COMMENT '分类ID',
    title        VARCHAR(200)      NOT NULL COMMENT '文章标题',
    summary      TEXT COMMENT '文章摘要',
    content      LONGTEXT          NOT NULL COMMENT '文章内容',
    author       VARCHAR(100) COMMENT '作者',
    source       VARCHAR(200) COMMENT '来源',
    tags         JSON COMMENT '标签数组',
    cover_image  BLOB COMMENT '封面图片',
    view_count   INT UNSIGNED      NOT NULL DEFAULT 0 COMMENT '浏览次数',
    like_count   INT UNSIGNED      NOT NULL DEFAULT 0 COMMENT '点赞次数',
    is_featured  TINYINT(1)        NOT NULL DEFAULT 0 COMMENT '是否精选',
    is_published TINYINT(1)        NOT NULL DEFAULT 1 COMMENT '是否发布',
    publish_date TIMESTAMP         NULL COMMENT '发布时间',
    created_at   TIMESTAMP         NOT NULL DEFAULT CURRENT_TIMESTAMP COMMENT '创建时间',
    updated_at   TIMESTAMP         NOT NULL DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP COMMENT '更新时间',
    FOREIGN KEY (category_id) REFERENCES psychology_categories (category_id) ON DELETE RESTRICT,
    INDEX idx_category (category_id),
    INDEX idx_publish_date (publish_date),
    INDEX idx_is_featured (is_featured),
    INDEX idx_is_published (is_published),
    FULLTEXT INDEX ft_title_content (title, content)
) ENGINE = InnoDB
  DEFAULT CHARSET = utf8mb4
  COLLATE = utf8mb4_unicode_ci COMMENT = '心理知识库文章表';

-- ============================================================================
-- 16. psychology_qna — 心理知识库问答表
-- ============================================================================
CREATE TABLE psychology_qna
(
    qna_id       BIGINT UNSIGNED PRIMARY KEY AUTO_INCREMENT COMMENT '问答ID',
    category_id  SMALLINT UNSIGNED NOT NULL COMMENT '分类ID',
    question     TEXT              NOT NULL COMMENT '问题',
    answer       LONGTEXT          NOT NULL COMMENT '答案',
    expert_name  VARCHAR(100) COMMENT '专家姓名',
    expert_title VARCHAR(200) COMMENT '专家头衔',
    tags         JSON COMMENT '标签数组',
    view_count   INT UNSIGNED      NOT NULL DEFAULT 0 COMMENT '浏览次数',
    like_count   INT UNSIGNED      NOT NULL DEFAULT 0 COMMENT '点赞次数',
    is_verified  TINYINT(1)        NOT NULL DEFAULT 0 COMMENT '是否经过专业验证',
    status       TINYINT           NOT NULL DEFAULT 1 COMMENT '状态:1可见,0隐藏',
    created_at   TIMESTAMP         NOT NULL DEFAULT CURRENT_TIMESTAMP COMMENT '创建时间',
    updated_at   TIMESTAMP         NOT NULL DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP COMMENT '更新时间',
    FOREIGN KEY (category_id) REFERENCES psychology_categories (category_id) ON DELETE RESTRICT,
    INDEX idx_category (category_id),
    INDEX idx_status (status),
    INDEX idx_is_verified (is_verified),
    FULLTEXT INDEX ft_question_answer (question, answer)
) ENGINE = InnoDB
  DEFAULT CHARSET = utf8mb4
  COLLATE = utf8mb4_unicode_ci COMMENT = '心理知识库问答表';

-- ============================================================================
-- 17. psychology_resources — 心理资源库表
-- ============================================================================
CREATE TABLE psychology_resources
(
    resource_id   BIGINT UNSIGNED PRIMARY KEY AUTO_INCREMENT COMMENT '资源ID',
    category_id   SMALLINT UNSIGNED NOT NULL COMMENT '分类ID',
    resource_type VARCHAR(32)       NOT NULL COMMENT '资源类型:VIDEO|AUDIO|PDF|LINK|TOOL',
    title         VARCHAR(200)      NOT NULL COMMENT '资源标题',
    description   TEXT COMMENT '资源描述',
    file_data     LONGBLOB COMMENT '文件二进制数据(视频/音频/PDF等)',
    external_url  VARCHAR(500) COMMENT '外部链接',
    file_size     BIGINT UNSIGNED COMMENT '文件大小(字节)',
    mime_type     VARCHAR(100) COMMENT '文件MIME类型',
    duration      INT UNSIGNED COMMENT '时长(秒,用于视频/音频)',
    thumbnail     BLOB COMMENT '缩略图',
    tags          JSON COMMENT '标签数组',
    view_count    INT UNSIGNED      NOT NULL DEFAULT 0 COMMENT '浏览/下载次数',
    like_count    INT UNSIGNED      NOT NULL DEFAULT 0 COMMENT '点赞次数',
    status        TINYINT           NOT NULL DEFAULT 1 COMMENT '状态:1可用,0不可用',
    created_at    TIMESTAMP         NOT NULL DEFAULT CURRENT_TIMESTAMP COMMENT '创建时间',
    updated_at    TIMESTAMP         NOT NULL DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP COMMENT '更新时间',
    FOREIGN KEY (category_id) REFERENCES psychology_categories (category_id) ON DELETE RESTRICT,
    INDEX idx_category (category_id),
    INDEX idx_resource_type (resource_type),
    INDEX idx_status (status)
) ENGINE = InnoDB
  DEFAULT CHARSET = utf8mb4
  COLLATE = utf8mb4_unicode_ci COMMENT = '心理资源库表';

-- ============================================================================
-- 18. user_knowledge_favorites — 用户知识库收藏表
-- ============================================================================
CREATE TABLE user_knowledge_favorites
(
    favorite_id  BIGINT UNSIGNED PRIMARY KEY AUTO_INCREMENT COMMENT '收藏ID',
    user_id      BIGINT UNSIGNED NOT NULL COMMENT '用户ID',
    content_type VARCHAR(32)     NOT NULL COMMENT '内容类型:ARTICLE|QNA|RESOURCE',
    content_id   BIGINT UNSIGNED NOT NULL COMMENT '内容ID',
    created_at   TIMESTAMP       NOT NULL DEFAULT CURRENT_TIMESTAMP COMMENT '收藏时间',
    FOREIGN KEY (user_id) REFERENCES users (id) ON DELETE CASCADE,
    UNIQUE KEY uk_user_content (user_id, content_type, content_id),
    INDEX idx_user_id (user_id),
    INDEX idx_content (content_type, content_id)
) ENGINE = InnoDB
  DEFAULT CHARSET = utf8mb4
  COLLATE = utf8mb4_unicode_ci COMMENT = '用户知识库收藏表';

-- ============================================================================
-- 19. content_likes — 内容点赞表 (from patch 001)
-- ============================================================================
CREATE TABLE content_likes
(
    like_id      BIGINT UNSIGNED PRIMARY KEY AUTO_INCREMENT COMMENT '点赞ID',
    user_id      BIGINT UNSIGNED NOT NULL COMMENT '用户ID',
    content_type VARCHAR(64)     NOT NULL COMMENT '内容类型',
    content_id   BIGINT UNSIGNED NOT NULL COMMENT '内容ID',
    created_at   DATETIME(6)     NOT NULL DEFAULT CURRENT_TIMESTAMP(6) COMMENT '点赞时间',
    FOREIGN KEY (user_id) REFERENCES users (id) ON DELETE CASCADE,
    UNIQUE KEY uk_content_likes_user_content (user_id, content_type, content_id),
    INDEX idx_content_likes_content (content_type, content_id),
    INDEX idx_content_likes_user_id (user_id)
) ENGINE = InnoDB
  DEFAULT CHARSET = utf8mb4
  COLLATE = utf8mb4_unicode_ci COMMENT = '内容点赞表';

-- ============================================================================
-- 20. music — 音乐表
-- ============================================================================
CREATE TABLE music
(
    music_id    BIGINT UNSIGNED PRIMARY KEY AUTO_INCREMENT COMMENT '音乐ID',
    title       VARCHAR(200)    NOT NULL COMMENT '音乐标题',
    artist      VARCHAR(100) COMMENT '艺术家/歌手',
    album       VARCHAR(200) COMMENT '专辑名称',
    category    VARCHAR(50) COMMENT '分类',
    description TEXT COMMENT '音乐描述',
    duration    INT UNSIGNED COMMENT '时长(秒)',
    file_data   LONGBLOB        NOT NULL COMMENT '音乐文件二进制数据',
    file_size   BIGINT UNSIGNED NOT NULL COMMENT '文件大小(字节)',
    mime_type   VARCHAR(100)    NOT NULL COMMENT '文件MIME类型(如audio/mpeg)',
    cover_image BLOB COMMENT '封面图片',
    lyrics      TEXT COMMENT '歌词',
    tags        JSON COMMENT '标签数组',
    mood_tags   JSON COMMENT '情绪标签(如放松、振奋、舒缓等)',
    status      TINYINT         NOT NULL DEFAULT 1 COMMENT '状态:1可用,0不可用',
    created_at  TIMESTAMP       NOT NULL DEFAULT CURRENT_TIMESTAMP COMMENT '创建时间',
    updated_at  TIMESTAMP       NOT NULL DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP COMMENT '更新时间',
    INDEX idx_artist (artist),
    INDEX idx_category (category),
    INDEX idx_status (status),
    INDEX idx_created_at (created_at),
    FULLTEXT INDEX ft_title_artist (title, artist)
) ENGINE = InnoDB
  DEFAULT CHARSET = utf8mb4
  COLLATE = utf8mb4_unicode_ci COMMENT = '音乐表';

-- ============================================================================
-- 21. knowledge_documents — RAG 知识文档表
--     (base from patch 002, +owner_user_id/+visibility/+source_version/
--      +source_updated_at/+deleted_at from patch 004)
-- ============================================================================
CREATE TABLE knowledge_documents
(
    document_id       BIGINT UNSIGNED PRIMARY KEY AUTO_INCREMENT,
    source_type       VARCHAR(64)     NOT NULL,
    source_id         BIGINT UNSIGNED NULL,
    owner_user_id     BIGINT UNSIGNED NULL,
    visibility        VARCHAR(32)     NOT NULL DEFAULT 'public',
    title             VARCHAR(255)    NULL,
    content_hash      CHAR(64)        NOT NULL,
    source_version    VARCHAR(128)    NULL,
    source_updated_at DATETIME(6)     NULL,
    metadata          JSON            NULL,
    status            TINYINT         NOT NULL DEFAULT 1,
    created_at        DATETIME(6)     NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    updated_at        DATETIME(6)     NOT NULL DEFAULT CURRENT_TIMESTAMP(6) ON UPDATE CURRENT_TIMESTAMP(6),
    deleted_at        DATETIME(6)     NULL,
    UNIQUE KEY uk_knowledge_documents_source (source_type, source_id),
    INDEX idx_knowledge_documents_status (status),
    INDEX idx_knowledge_documents_owner_status (owner_user_id, status),
    INDEX idx_knowledge_documents_visibility_status (visibility, status),
    FOREIGN KEY (owner_user_id) REFERENCES users (id) ON DELETE SET NULL
) ENGINE = InnoDB
  DEFAULT CHARSET = utf8mb4
  COLLATE = utf8mb4_unicode_ci;

-- ============================================================================
-- 22. knowledge_chunks — RAG 知识分块表
--     (base from patch 002,
--      +vector_id/+embedding_provider/+embedding_model/+embedding_dimension/
--      +indexed_at from patch 003,
--      +status/+content_hash/+char_start/+char_end from patch 004)
-- ============================================================================
CREATE TABLE knowledge_chunks
(
    chunk_id            BIGINT UNSIGNED PRIMARY KEY AUTO_INCREMENT,
    document_id         BIGINT UNSIGNED NOT NULL,
    chunk_index         INT UNSIGNED    NOT NULL,
    char_start          INT UNSIGNED    NULL,
    char_end            INT UNSIGNED    NULL,
    content             TEXT            NOT NULL,
    content_hash        CHAR(64)        NULL,
    token_count         INT UNSIGNED    NULL,
    metadata            JSON            NULL,
    status              TINYINT         NOT NULL DEFAULT 1,
    created_at          DATETIME(6)     NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    vector_id           VARCHAR(128)    NULL,
    embedding_provider  VARCHAR(64)     NULL,
    embedding_model     VARCHAR(128)    NULL,
    embedding_dimension INT UNSIGNED    NULL,
    indexed_at          DATETIME(6)     NULL,
    UNIQUE KEY uk_knowledge_chunks_doc_idx (document_id, chunk_index),
    UNIQUE KEY uk_knowledge_chunks_vector_id (vector_id),
    FULLTEXT KEY ft_knowledge_chunks_content (content),
    INDEX idx_knowledge_chunks_document_status (document_id, status),
    INDEX idx_knowledge_chunks_vector_id (vector_id),
    FOREIGN KEY (document_id) REFERENCES knowledge_documents (document_id) ON DELETE CASCADE
) ENGINE = InnoDB
  DEFAULT CHARSET = utf8mb4
  COLLATE = utf8mb4_unicode_ci;

-- ============================================================================
-- 23. knowledge_embeddings — RAG 知识嵌入向量表 (from patch 002)
-- ============================================================================
CREATE TABLE knowledge_embeddings
(
    embedding_id   BIGINT UNSIGNED PRIMARY KEY AUTO_INCREMENT,
    chunk_id       BIGINT UNSIGNED NOT NULL,
    provider       VARCHAR(64)     NOT NULL,
    model          VARCHAR(128)    NOT NULL,
    dimension      INT UNSIGNED    NOT NULL,
    embedding_json JSON            NOT NULL,
    created_at     DATETIME(6)     NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    UNIQUE KEY uk_knowledge_embeddings_chunk_model (chunk_id, provider, model),
    FOREIGN KEY (chunk_id) REFERENCES knowledge_chunks (chunk_id) ON DELETE CASCADE
) ENGINE = InnoDB
  DEFAULT CHARSET = utf8mb4
  COLLATE = utf8mb4_unicode_ci;

-- ============================================================================
-- 24. user_memories — 用户长期记忆表
--     (base from patch 002,
--      +vector_id/+embedding_provider/+embedding_model/+embedding_dimension/
--      +indexed_at from patch 003,
--      +memory_key/+salience/+last_accessed_at/+access_count/+expires_at
--       from patch 004)
-- ============================================================================
CREATE TABLE user_memories
(
    memory_id              BIGINT UNSIGNED PRIMARY KEY AUTO_INCREMENT,
    user_id                BIGINT UNSIGNED NOT NULL,
    memory_type            VARCHAR(64)     NOT NULL,
    memory_key             CHAR(64)        NULL,
    content                TEXT            NOT NULL,
    confidence             DOUBLE          NOT NULL DEFAULT 0.7,
    salience               DOUBLE          NOT NULL DEFAULT 0.5,
    source_conversation_id BIGINT UNSIGNED NULL,
    source_message_id      BIGINT UNSIGNED NULL,
    status                 TINYINT         NOT NULL DEFAULT 1,
    metadata               JSON            NULL,
    created_at             DATETIME(6)     NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    updated_at             DATETIME(6)     NOT NULL DEFAULT CURRENT_TIMESTAMP(6) ON UPDATE CURRENT_TIMESTAMP(6),
    last_accessed_at       DATETIME(6)     NULL,
    access_count           INT UNSIGNED    NOT NULL DEFAULT 0,
    expires_at             DATETIME(6)     NULL,
    vector_id              VARCHAR(128)    NULL,
    embedding_provider     VARCHAR(64)     NULL,
    embedding_model        VARCHAR(128)    NULL,
    embedding_dimension    INT UNSIGNED    NULL,
    indexed_at             DATETIME(6)     NULL,
    UNIQUE KEY uk_user_memories_vector_id (vector_id),
    INDEX idx_user_memories_user_status (user_id, status),
    INDEX idx_user_memories_user_key (user_id, memory_key),
    INDEX idx_user_memories_user_salience (user_id, status, salience),
    INDEX idx_user_memories_expires_at (expires_at),
    INDEX idx_user_memories_vector_id (vector_id),
    FULLTEXT KEY ft_user_memories_content (content),
    FOREIGN KEY (user_id) REFERENCES users (id) ON DELETE CASCADE,
    FOREIGN KEY (source_conversation_id) REFERENCES conversations (id) ON DELETE SET NULL,
    FOREIGN KEY (source_message_id) REFERENCES conversation_messages (id) ON DELETE SET NULL
) ENGINE = InnoDB
  DEFAULT CHARSET = utf8mb4
  COLLATE = utf8mb4_unicode_ci;

-- ============================================================================
-- 25. user_memory_embeddings — 用户记忆嵌入向量表 (from patch 002)
-- ============================================================================
CREATE TABLE user_memory_embeddings
(
    embedding_id   BIGINT UNSIGNED PRIMARY KEY AUTO_INCREMENT,
    memory_id      BIGINT UNSIGNED NOT NULL,
    provider       VARCHAR(64)     NOT NULL,
    model          VARCHAR(128)    NOT NULL,
    dimension      INT UNSIGNED    NOT NULL,
    embedding_json JSON            NOT NULL,
    created_at     DATETIME(6)     NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    UNIQUE KEY uk_user_memory_embeddings_memory_model (memory_id, provider, model),
    FOREIGN KEY (memory_id) REFERENCES user_memories (memory_id) ON DELETE CASCADE
) ENGINE = InnoDB
  DEFAULT CHARSET = utf8mb4
  COLLATE = utf8mb4_unicode_ci;

-- ============================================================================
-- 26. conversation_summaries — 会话摘要表
--     (base from patch 002,
--      +vector_id/+embedding_provider/+embedding_model/+embedding_dimension/
--      +indexed_at from patch 003,
--      +status/+summary_version/+source_message_count from patch 004)
-- ============================================================================
CREATE TABLE conversation_summaries
(
    summary_id            BIGINT UNSIGNED PRIMARY KEY AUTO_INCREMENT,
    conversation_id       BIGINT UNSIGNED NOT NULL,
    user_id               BIGINT UNSIGNED NOT NULL,
    summary_type          VARCHAR(64)     NOT NULL DEFAULT 'rolling',
    content               TEXT            NOT NULL,
    message_start_id      BIGINT UNSIGNED NULL,
    message_end_id        BIGINT UNSIGNED NULL,
    token_count           INT UNSIGNED    NULL,
    status                TINYINT         NOT NULL DEFAULT 1,
    summary_version       INT UNSIGNED    NOT NULL DEFAULT 1,
    source_message_count  INT UNSIGNED    NULL,
    created_at            DATETIME(6)     NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    updated_at            DATETIME(6)     NOT NULL DEFAULT CURRENT_TIMESTAMP(6) ON UPDATE CURRENT_TIMESTAMP(6),
    vector_id             VARCHAR(128)    NULL,
    embedding_provider    VARCHAR(64)     NULL,
    embedding_model       VARCHAR(128)    NULL,
    embedding_dimension   INT UNSIGNED    NULL,
    indexed_at            DATETIME(6)     NULL,
    UNIQUE KEY uk_conversation_summaries_vector_id (vector_id),
    INDEX idx_conversation_summaries_conversation (conversation_id),
    INDEX idx_conversation_summaries_user (user_id),
    INDEX idx_conversation_summaries_conv_status (conversation_id, status, updated_at),
    INDEX idx_conversation_summaries_vector_id (vector_id),
    FOREIGN KEY (conversation_id) REFERENCES conversations (id) ON DELETE CASCADE,
    FOREIGN KEY (user_id) REFERENCES users (id) ON DELETE CASCADE
) ENGINE = InnoDB
  DEFAULT CHARSET = utf8mb4
  COLLATE = utf8mb4_unicode_ci;

-- ============================================================================
-- 27. agent_events — Agent 事件追踪表
--     (base from patch 002,
--      +trace_id/+turn_id/+severity/+tool_name from patch 004)
-- ============================================================================
CREATE TABLE agent_events
(
    event_id        BIGINT UNSIGNED PRIMARY KEY AUTO_INCREMENT,
    user_id         BIGINT UNSIGNED NOT NULL,
    conversation_id BIGINT UNSIGNED NULL,
    session_id      VARCHAR(64)     NULL,
    trace_id        VARCHAR(64)     NULL,
    turn_id         VARCHAR(64)     NULL,
    event_type      VARCHAR(64)     NOT NULL,
    severity        VARCHAR(32)     NOT NULL DEFAULT 'info',
    tool_name       VARCHAR(128)    NULL,
    payload         JSON            NOT NULL,
    created_at      DATETIME(6)     NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    INDEX idx_agent_events_user_time (user_id, created_at),
    INDEX idx_agent_events_conversation (conversation_id),
    INDEX idx_agent_events_trace (trace_id),
    INDEX idx_agent_events_turn (turn_id),
    INDEX idx_agent_events_type_time (event_type, created_at),
    FOREIGN KEY (user_id) REFERENCES users (id) ON DELETE CASCADE,
    FOREIGN KEY (conversation_id) REFERENCES conversations (id) ON DELETE SET NULL
) ENGINE = InnoDB
  DEFAULT CHARSET = utf8mb4
  COLLATE = utf8mb4_unicode_ci;

-- ============================================================================
-- 28. vector_index_records — Qdrant 向量索引记录表 (from patch 004)
-- ============================================================================
CREATE TABLE vector_index_records
(
    record_id           BIGINT UNSIGNED PRIMARY KEY AUTO_INCREMENT,
    vector_id           VARCHAR(128)    NOT NULL,
    collection_name     VARCHAR(128)    NOT NULL,
    object_type         VARCHAR(64)     NOT NULL,
    object_id           BIGINT UNSIGNED NOT NULL,
    owner_user_id       BIGINT UNSIGNED NULL,
    source_table        VARCHAR(64)     NOT NULL,
    source_hash         CHAR(64)        NULL,
    embedding_provider  VARCHAR(64)     NOT NULL,
    embedding_model     VARCHAR(128)    NOT NULL,
    embedding_dimension INT UNSIGNED    NOT NULL,
    payload             JSON            NOT NULL,
    index_status        VARCHAR(32)     NOT NULL DEFAULT 'indexed',
    indexed_at          DATETIME(6)     NULL,
    failed_at           DATETIME(6)     NULL,
    error_message       TEXT            NULL,
    created_at          DATETIME(6)     NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    updated_at          DATETIME(6)     NOT NULL DEFAULT CURRENT_TIMESTAMP(6) ON UPDATE CURRENT_TIMESTAMP(6),
    UNIQUE KEY uk_vector_index_records_vector_id (vector_id),
    INDEX idx_vector_index_records_object (object_type, object_id),
    INDEX idx_vector_index_records_collection_status (collection_name, index_status),
    INDEX idx_vector_index_records_owner_type (owner_user_id, object_type),
    FOREIGN KEY (owner_user_id) REFERENCES users (id) ON DELETE SET NULL
) ENGINE = InnoDB
  DEFAULT CHARSET = utf8mb4
  COLLATE = utf8mb4_unicode_ci;

-- ============================================================================
-- 29. vector_index_jobs — Qdrant 向量索引作业表 (from patch 004)
-- ============================================================================
CREATE TABLE vector_index_jobs
(
    job_id          BIGINT UNSIGNED PRIMARY KEY AUTO_INCREMENT,
    action          VARCHAR(32)     NOT NULL,
    object_type     VARCHAR(64)     NOT NULL,
    object_id       BIGINT UNSIGNED NOT NULL,
    collection_name VARCHAR(128)    NOT NULL,
    vector_id       VARCHAR(128)    NULL,
    priority        INT             NOT NULL DEFAULT 100,
    status          VARCHAR(32)     NOT NULL DEFAULT 'pending',
    attempts        INT UNSIGNED    NOT NULL DEFAULT 0,
    max_attempts    INT UNSIGNED    NOT NULL DEFAULT 5,
    next_run_at     DATETIME(6)     NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    locked_at       DATETIME(6)     NULL,
    locked_by       VARCHAR(128)    NULL,
    last_error      TEXT            NULL,
    created_at      DATETIME(6)     NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    updated_at      DATETIME(6)     NOT NULL DEFAULT CURRENT_TIMESTAMP(6) ON UPDATE CURRENT_TIMESTAMP(6),
    INDEX idx_vector_index_jobs_status_next (status, next_run_at, priority),
    INDEX idx_vector_index_jobs_object (object_type, object_id),
    INDEX idx_vector_index_jobs_vector_id (vector_id)
) ENGINE = InnoDB
  DEFAULT CHARSET = utf8mb4
  COLLATE = utf8mb4_unicode_ci;


-- #############################################################################
-- ##                                                                         ##
-- ##                          SEED DATA SECTION                              ##
-- ##                                                                         ##
-- ##  Development / testing data with realistic Chinese mental-health domain ##
-- ##  content.  All FK references are self-consistent.                       ##
-- ##  Password for all users = "123123123" → BCrypt hash below.              ##
-- #############################################################################

SET FOREIGN_KEY_CHECKS = 0;

-- ─────────────────────────────────────────────────────────────────────────────
-- Users  (password BCrypt: 123123123)
-- ─────────────────────────────────────────────────────────────────────────────
INSERT INTO users (id, username, password, email, phone, avatar, nickname, created_at, updated_at, last_login_at, status, role)
VALUES
(1, 'alice', '$2a$10$ayH0oUppUHYfMg4BAwgx1OBrDx7hXRYqM8Dz8iMbiJXaVpWmyEgrm',
 'alice@example.com', '13800000001', NULL, '清晨小太阳',
 '2026-05-01 08:30:00', '2026-06-11 10:12:00', '2026-06-11 10:12:00', 1, 'USER'),
(2, 'bruce', '$2a$10$ayH0oUppUHYfMg4BAwgx1OBrDx7hXRYqM8Dz8iMbiJXaVpWmyEgrm',
 'bruce@example.com', '13800000002', NULL, '海边散步者',
 '2026-05-01 09:00:00', '2026-06-11 12:05:00', '2026-06-11 12:05:00', 1, 'USER'),
(3, 'chloe', '$2a$10$ayH0oUppUHYfMg4BAwgx1OBrDx7hXRYqM8Dz8iMbiJXaVpWmyEgrm',
 'chloe@example.com', '13800000003', NULL, '慢生活研究员',
 '2026-05-02 07:55:00', '2026-06-11 08:45:00', '2026-06-11 08:45:00', 1, 'USER'),
(4, 'dylan', '$2a$10$ayH0oUppUHYfMg4BAwgx1OBrDx7hXRYqM8Dz8iMbiJXaVpWmyEgrm',
 'dylan@example.com', '13800000004', NULL, '森林里的猫',
 '2026-05-02 10:25:00', '2026-06-11 21:02:00', '2026-06-11 21:02:00', 1, 'USER'),
(5, 'elena', '$2a$10$ayH0oUppUHYfMg4BAwgx1OBrDx7hXRYqM8Dz8iMbiJXaVpWmyEgrm',
 'elena@example.com', '13800000005', NULL, '夜空观星人',
 '2026-05-03 06:40:00', '2026-06-11 22:18:00', '2026-06-11 22:18:00', 1, 'USER'),
(6, 'admin', '$2a$10$ayH0oUppUHYfMg4BAwgx1OBrDx7hXRYqM8Dz8iMbiJXaVpWmyEgrm',
 'admin@dc.local', NULL, NULL, '系统管理员',
 '2026-05-01 00:00:00', '2026-06-11 00:00:00', '2026-06-12 08:00:00', 1, 'ADMIN');

-- ─────────────────────────────────────────────────────────────────────────────
-- User Profiles
-- ─────────────────────────────────────────────────────────────────────────────
INSERT INTO user_profiles (id, user_id, interests, personality_traits, interaction_preferences, emotional_tendency, learning_records, created_at, updated_at)
VALUES
(1, 1,
 '["晨间瑜伽","手帐","规律作息"]',
 '["敏感细腻","INFJ"]',
 '["温柔鼓励","健康习惯","每日签到"]',
 '["平和","睡眠不佳","呼吸练习"]',
 '[]',
 '2026-06-10 09:30:00', '2026-06-11 10:05:00'),
(2, 2,
 '["海边散步","摄影","保持运动"]',
 '["务实","ISTJ"]',
 '["直接建议","运动计划"]',
 '["坚韧","手心出汗","握拳放松"]',
 '[]',
 '2026-06-10 11:00:00', '2026-06-11 11:42:00'),
(3, 3,
 '["慢跑","手工","提升专注"]',
 '["乐观","ENFP"]',
 '["陪伴式","创意活动","每日签到"]',
 '["积极","注意力涣散","冥想"]',
 '[]',
 '2026-06-10 13:15:00', '2026-06-11 13:40:00'),
(4, 4,
 '["森林散步","整理空间"]',
 '["沉稳","ISFP"]',
 '["温柔倾听","自我关怀"]',
 '["温厚","情绪堆积","写信"]',
 '[]',
 '2026-06-10 15:20:00', '2026-06-11 16:05:00'),
(5, 5,
 '["观星","夜跑","改善睡眠"]',
 '["好奇","INFP"]',
 '["启发式","睡前放松","每日签到"]',
 '["温柔","入睡困难","热水澡"]',
 '[]',
 '2026-06-10 18:10:00', '2026-06-11 18:45:00');

-- ─────────────────────────────────────────────────────────────────────────────
-- Conversations (2 per user = 10)
-- ─────────────────────────────────────────────────────────────────────────────
INSERT INTO conversations (id, user_id, title, is_title_generated, last_message_at, message_count, created_at)
VALUES
(1,  1, '夜间情绪疏导对话',      1, '2026-06-11 21:55:05', 4, '2026-06-11 21:50:30'),
(2,  1, '晨间目标检视',          0, '2026-06-11 07:14:05', 4, '2026-06-11 07:10:00'),
(3,  2, '焦虑应对练习',          1, '2026-06-11 15:30:00', 3, '2026-06-11 15:20:00'),
(4,  2, '海边散步随聊',          1, '2026-06-10 18:40:00', 5, '2026-06-10 18:30:00'),
(5,  3, '慢跑呼吸节奏探讨',      1, '2026-06-11 06:45:00', 3, '2026-06-11 06:30:00'),
(6,  3, '手工疗愈心情分享',      0, '2026-06-10 14:30:00', 4, '2026-06-10 14:10:00'),
(7,  4, '森林漫步与情绪整理',    1, '2026-06-11 08:10:00', 3, '2026-06-11 07:55:00'),
(8,  4, '写给自己的信',          1, '2026-06-10 22:33:00', 5, '2026-06-10 22:05:00'),
(9,  5, '观星与睡前放松',        1, '2026-06-11 23:20:00', 4, '2026-06-11 23:00:00'),
(10, 5, '夜跑音乐推荐',          1, '2026-06-10 20:55:00', 3, '2026-06-10 20:40:00');

-- ─────────────────────────────────────────────────────────────────────────────
-- Conversation Messages (3–5 per conversation = 38 rows)
-- ─────────────────────────────────────────────────────────────────────────────
INSERT INTO conversation_messages (id, conversation_id, sender_role, sender_user_id, message_type, content, token_count, created_at)
VALUES
-- Conv 1 (alice: 夜间情绪疏导)
(1,  1, 'user',      1, 'text', '{"text":"晚上一个人静下来就又想起白天的事，胸口很闷。"}', 18, '2026-06-11 21:51:00'),
(2,  1, 'assistant', NULL, 'text', '{"text":"我听到你了。这种感觉来了的时候，我们先做三次深呼吸好吗？吸气……呼气……"}' , 28, '2026-06-11 21:52:00'),
(3,  1, 'user',      1, 'text', '{"text":"试了三次呼吸，好像胸口没那么紧了。谢谢你。"}' , 16, '2026-06-11 21:54:00'),
(4,  1, 'assistant', NULL, 'text', '{"text":"你做得很好。如果待会儿还是难入睡，可以试试「身体扫描」放松法。"}' , 22, '2026-06-11 21:55:05'),
-- Conv 2 (alice: 晨间目标)
(5,  2, 'user',      1, 'text', '{"text":"早安！今天我想完成两件事：晨间瑜伽和写完手帐。"}' , 15, '2026-06-11 07:10:30'),
(6,  2, 'assistant', NULL, 'text', '{"text":"早安，小太阳！很棒的规划。我们先从五分钟热身瑜伽开始？"}' , 18, '2026-06-11 07:12:00'),
(7,  2, 'user',      1, 'text', '{"text":"做完瑜伽了，感觉身体被唤醒了！"}' , 10, '2026-06-11 07:13:30'),
(8,  2, 'assistant', NULL, 'text', '{"text":"太棒了！现在喝杯温水，拿出你的手帐写下今天的三个小目标吧。"}' , 20, '2026-06-11 07:14:05'),
-- Conv 3 (bruce: 焦虑应对)
(9,  3, 'user',      2, 'text', '{"text":"下午开会前手心又开始出汗了，那个熟悉的紧张感又来了。"}' , 15, '2026-06-11 15:21:00'),
(10, 3, 'assistant', NULL, 'text', '{"text":"我理解。我们来试试「握拳放松法」：用力握拳5秒，然后松开，感受那个释放的瞬间。"}' , 22, '2026-06-11 15:28:00'),
(11, 3, 'user',      2, 'text', '{"text":"做了几轮，手心没那么湿了。这个方法真管用。"}' , 14, '2026-06-11 15:30:00'),
-- Conv 4 (bruce: 海边散步)
(12, 4, 'user',      2, 'text', '{"text":"今天傍晚的海风特别凉爽，走了快四千步。"}' , 12, '2026-06-10 18:32:00'),
(13, 4, 'assistant', NULL, 'text', '{"text":"海风是最好的天然镇静剂。你注意看海面了吗？波浪的节奏和呼吸很像。"}' , 20, '2026-06-10 18:35:00'),
(14, 4, 'user',      2, 'text', '{"text":"是啊，一浪一浪的，看着看着心跳就慢下来了。"}' , 14, '2026-06-10 18:38:00'),
(15, 4, 'assistant', NULL, 'text', '{"text":"记住这种感觉。下次紧张的时候，闭上眼睛回想这片海浪。"}' , 16, '2026-06-10 18:39:30'),
(16, 4, 'user',      2, 'text', '{"text":"好的，我试着把它当作一个心理锚点。"}' , 12, '2026-06-10 18:40:00'),
-- Conv 5 (chloe: 慢跑呼吸)
(17, 5, 'user',      3, 'text', '{"text":"医生建议我试试「吸两步呼两步」的节奏跑步，今天第一次试。"}' , 16, '2026-06-11 06:32:00'),
(18, 5, 'assistant', NULL, 'text', '{"text":"这个节奏很适合新手！感觉怎么样？"}' , 8, '2026-06-11 06:40:00'),
(19, 5, 'user',      3, 'text', '{"text":"第一次觉得自己能坚持下来，跑完也没有喘得那么厉害。"}' , 16, '2026-06-11 06:45:00'),
-- Conv 6 (chloe: 手工疗愈)
(20, 6, 'user',      3, 'text', '{"text":"今天做了一只小狐狸布偶，缝耳朵的时候手忙脚乱的。"}' , 16, '2026-06-10 14:15:00'),
(21, 6, 'assistant', NULL, 'text', '{"text":"哇！小狐狸是什么颜色的？手工的过程是不是让你完全忘记了时间？"}' , 18, '2026-06-10 14:22:00'),
(22, 6, 'user',      3, 'text', '{"text":"橘色的！虽然针脚歪歪扭扭，但确实那段时间脑子里没有杂念。"}' , 18, '2026-06-10 14:27:00'),
(23, 6, 'assistant', NULL, 'text', '{"text":"这就是「心流」体验。针脚歪一点才有手作温度嘛！"}' , 14, '2026-06-10 14:30:00'),
-- Conv 7 (dylan: 森林漫步)
(24, 7, 'user',      4, 'text', '{"text":"今早在林间绕了一圈，阳光透过叶子像星星洒在地上。"}' , 14, '2026-06-11 07:57:00'),
(25, 7, 'assistant', NULL, 'text', '{"text":"真美的画面。森林里有没有闻到松针或泥土的味道？"}' , 14, '2026-06-11 08:05:00'),
(26, 7, 'user',      4, 'text', '{"text":"有，那种混合着露水的土味让人觉得踏实。"}' , 12, '2026-06-11 08:10:00'),
-- Conv 8 (dylan: 写信)
(27, 8, 'user',      4, 'text', '{"text":"写了封信给三年前的自己。那时候的我其实也很勇敢。"}' , 16, '2026-06-10 22:08:00'),
(28, 8, 'assistant', NULL, 'text', '{"text":"这封信本身就是一种自我关怀。你愿意分享一段信里的话吗？"}' , 16, '2026-06-10 22:15:00'),
(29, 8, 'user',      4, 'text', '{"text":"我写了「谢谢你撑过了那个冬天」。写完眼泪就下来了……"}' , 18, '2026-06-10 22:28:00'),
(30, 8, 'assistant', NULL, 'text', '{"text":"（轻声）那是很深的情感。你已经走了很远的路，而且没有放弃自己。今天也要给自己一点温柔。"}' , 26, '2026-06-10 22:32:00'),
(31, 8, 'user',      4, 'text', '{"text":"谢谢你听我说。我觉得今晚能睡得好一点。"}' , 14, '2026-06-10 22:33:00'),
-- Conv 9 (elena: 观星放松)
(32, 9, 'user',      5, 'text', '{"text":"今晚云散了一些，又看到猎户座了，虽然还是有点失眠……"}' , 16, '2026-06-11 23:02:00'),
(33, 9, 'assistant', NULL, 'text', '{"text":"猎户座真美。你试过「星空冥想」吗？想象自己躺在星空下，每一颗星带一个烦恼远去。"}' , 22, '2026-06-11 23:10:00'),
(34, 9, 'user',      5, 'text', '{"text":"试着做了五分钟，确实平静了些。薰衣草精油也用上了。"}' , 16, '2026-06-11 23:18:00'),
(35, 9, 'assistant', NULL, 'text', '{"text":"很好的搭配！星空 + 香气 + 深呼吸，给自己一个完整的睡前仪式。"}' , 16, '2026-06-11 23:20:00'),
-- Conv 10 (elena: 夜跑音乐)
(36, 10, 'user',      5, 'text', '{"text":"夜跑需要一些有节奏感但不太激烈的音乐，有推荐吗？"}' , 14, '2026-06-10 20:42:00'),
(37, 10, 'assistant', NULL, 'text', '{"text":"推荐你试试 Lo-Fi 慢节拍或者爵士钢琴，节奏稳定但不会让你加速。"}' , 18, '2026-06-10 20:52:00'),
(38, 10, 'user',      5, 'text', '{"text":"刚才用你推荐的歌单跑了半小时，心跳很稳，谢谢！"}' , 14, '2026-06-10 20:55:00');

-- ─────────────────────────────────────────────────────────────────────────────
-- Community Posts (20)
-- ─────────────────────────────────────────────────────────────────────────────
INSERT INTO community_posts (post_id, user_id, title, content, extra_metadata, likes_count, comments_count, status, created_at, updated_at)
VALUES
(1,  1, '晨间呼吸练习分享', '今天按照应用的呼吸引导练了十分钟，胸口的压抑感缓解了不少。',
 '{"tags":["breath","morning"],"mood":"relaxed"}', 18, 2, 1, '2026-06-05 07:20:00', '2026-06-05 08:15:00'),
(2,  1, '周末小目标', '计划完成两段散步和一次手帐记录，有一起的吗？',
 '{"tags":["plan","weekend"],"mood":"hopeful"}', 12, 2, 1, '2026-06-06 09:05:00', '2026-06-06 11:10:00'),
(3,  1, '音乐疗愈歌单', '搜集了一些温柔的钢琴曲，放松睡前的紧绷。欢迎补充。',
 '{"tags":["music","sleep"],"mood":"calm"}', 25, 2, 1, '2026-06-07 21:22:00', '2026-06-08 08:00:00'),
(4,  2, '今天的步行记录', '傍晚在海边走了3500步，风很大但心情轻松了。',
 '{"tags":["walk","evening"],"mood":"energized"}', 20, 2, 1, '2026-06-04 18:40:00', '2026-06-04 19:30:00'),
(5,  2, '焦虑时的小动作', '分享一个握拳再放松的动作，配合呼吸能让手心没那么出汗。',
 '{"tags":["anxiety","tips"],"mood":"steady"}', 16, 2, 1, '2026-06-08 10:15:00', '2026-06-08 12:32:00'),
(6,  2, '第一次尝试冥想', '坐了十五分钟还是会走神，不过结束后脑袋清亮了点。',
 '{"tags":["meditation"],"mood":"curious"}', 14, 2, 1, '2026-06-09 07:45:00', '2026-06-09 08:20:00'),
(7,  3, '慢跑的呼吸节奏', '医生建议的节奏是吸两步呼两步，第一次觉得自己能坚持。',
 '{"tags":["run","breath"],"mood":"motivated"}', 22, 2, 1, '2026-06-03 06:30:00', '2026-06-03 07:00:00'),
(8,  3, '手工疗愈时间', '做了一只小狐狸布偶，手忙脚乱但很治愈。',
 '{"tags":["craft","mindfulness"],"mood":"focused"}', 19, 2, 1, '2026-06-07 14:05:00', '2026-06-07 15:40:00'),
(9,  3, '饮食记录', '今天尝试了新的燕麦酸奶搭配，口感不错也很顶饱。',
 '{"tags":["diet","breakfast"],"mood":"content"}', 11, 2, 1, '2026-06-08 08:10:00', '2026-06-08 09:05:00'),
(10, 3, '正念时刻', '午餐前闭眼一分钟感受味道，让自己慢一点。',
 '{"tags":["mindfulness","meal"],"mood":"peaceful"}', 13, 2, 1, '2026-06-09 12:00:00', '2026-06-09 12:20:00'),
(11, 4, '森林散步日记', '早晨在林间绕了一圈，阳光透过叶子像星星。',
 '{"tags":["nature","walk"],"mood":"grounded"}', 28, 2, 1, '2026-06-02 07:10:00', '2026-06-02 08:05:00'),
(12, 4, '写给过去的自己', '给三年前的自己写了一封信，提醒那时也很勇敢。',
 '{"tags":["journaling","self-compassion"],"mood":"reflective"}', 24, 2, 1, '2026-06-06 22:18:00', '2026-06-07 06:50:00'),
(13, 4, '开窗整理房间', '整理柜子的时候发现了好多旧票根，原来快乐的瞬间不止一个。',
 '{"tags":["declutter","memory"],"mood":"nostalgic"}', 17, 2, 1, '2026-06-08 16:40:00', '2026-06-08 17:05:00'),
(14, 4, '猫咪陪伴', '猫咪坐在腿上打呼噜的那刻，世界好像没那么吵。',
 '{"tags":["pet","warmth"],"mood":"soothed"}', 35, 2, 1, '2026-06-10 21:15:00', '2026-06-10 21:40:00'),
(15, 5, '星空观察记录', '昨晚看到了猎户座的形状，拍了一张糊糊的照片。',
 '{"tags":["stargazing","night"],"mood":"awed"}', 30, 2, 1, '2026-06-04 23:05:00', '2026-06-05 00:10:00'),
(16, 5, '睡前放松流程', '热水澡+薰衣草精油+轻柔音乐，睡前焦虑缓解不少。',
 '{"tags":["sleep","routine"],"mood":"sleepy"}', 21, 2, 1, '2026-06-06 23:20:00', '2026-06-07 00:05:00'),
(17, 5, '感恩练习清单', '写下了今天感谢的三件小事，感觉心里亮了。',
 '{"tags":["gratitude"],"mood":"grateful"}', 18, 2, 1, '2026-06-08 21:30:00', '2026-06-08 22:00:00'),
(18, 5, '夜跑的节奏', '夜跑配着喜欢的节奏感音乐，心跳稳了一些。',
 '{"tags":["run","night"],"mood":"balanced"}', 23, 2, 1, '2026-06-09 20:40:00', '2026-06-09 21:30:00'),
(19, 2, '线上互助小组体验', '第一次参加线上互助小组，感受到大家彼此的照亮。',
 '{"tags":["support-group"],"mood":"supported"}', 27, 2, 1, '2026-06-11 15:10:00', '2026-06-11 16:00:00'),
(20, 3, '心情颜色打卡', '今天给自己的心情涂成了浅绿色，有新的生长。',
 '{"tags":["mood-tracking"],"mood":"hopeful"}', 15, 2, 1, '2026-06-11 18:25:00', '2026-06-11 18:50:00');

-- ─────────────────────────────────────────────────────────────────────────────
-- Community Post Media (11 tiny placeholder JPEGs via FROM_BASE64)
-- ─────────────────────────────────────────────────────────────────────────────
INSERT INTO community_post_media (media_id, post_id, media_type, mime_type, media_data, created_at)
VALUES
(1,  1,  'IMAGE', 'image/jpeg',
 FROM_BASE64('/9j/4AAQSkZJRgABAQAAAQABAAD/2wBDAAgGBgcGBQgHBwcJCQgKDBQNDAsLDBkSEw8UHRofHh0aHBwgJC4nICIsIxwcKDcpLDAxNDQ0Hyc5PTgyPC4zNDL/wAALCAABAAEBAREA/8QAFAABAAAAAAAAAAAAAAAAAAAAAP/aAAgBAQAAPwD/tsH/2Q=='),
 '2026-06-05 07:21:00'),
(2,  3,  'IMAGE', 'image/jpeg',
 FROM_BASE64('/9j/4AAQSkZJRgABAQAAAQABAAD/2wBDAAgGBgcGBQgHBwcJCQgKDBQNDAsLDBkSEw8UHRofHh0aHBwgJC4nICIsIxwcKDcpLDAxNDQ0Hyc5PTgyPC4zNDL/wAALCAABAAEBAREA/8QAFAABAAAAAAAAAAAAAAAAAAAAAP/aAAgBAQAAPwCt2Ob/2Q=='),
 '2026-06-07 21:30:00'),
(3,  4,  'IMAGE', 'image/jpeg',
 FROM_BASE64('/9j/4AAQSkZJRgABAQAAAQABAAD/2wBDAAgGBgcGBQgHBwcJCQgKDBQNDAsLDBkSEw8UHRofHh0aHBwgJC4nICIsIxwcKDcpLDAxNDQ0Hyc5PTgyPC4zNDL/wAALCAABAAEBAREA/8QAFAABAAAAAAAAAAAAAAAAAAAAAP/aAAgBAQAAPwBI0cz/2Q=='),
 '2026-06-04 18:45:00'),
(4,  5,  'IMAGE', 'image/jpeg',
 FROM_BASE64('/9j/4AAQSkZJRgABAQAAAQABAAD/2wBDAAgGBgcGBQgHBwcJCQgKDBQNDAsLDBkSEw8UHRofHh0aHBwgJC4nICIsIxwcKDcpLDAxNDQ0Hyc5PTgyPC4zNDL/wAALCAABAAEBAREA/8QAFAABAAAAAAAAAAAAAAAAAAAAAP/aAAgBAQAAPwD/1wD/2Q=='),
 '2026-06-08 10:20:00'),
(5,  7,  'IMAGE', 'image/jpeg',
 FROM_BASE64('/9j/4AAQSkZJRgABAQAAAQABAAD/2wBDAAgGBgcGBQgHBwcJCQgKDBQNDAsLDBkSEw8UHRofHh0aHBwgJC4nICIsIxwcKDcpLDAxNDQ0Hyc5PTgyPC4zNDL/wAALCAABAAEBAREA/8QAFAABAAAAAAAAAAAAAAAAAAAAAP/aAAgBAQAAPwD/jAD/2Q=='),
 '2026-06-03 06:35:00'),
(6,  8,  'IMAGE', 'image/jpeg',
 FROM_BASE64('/9j/4AAQSkZJRgABAQAAAQABAAD/2wBDAAgGBgcGBQgHBwcJCQgKDBQNDAsLDBkSEw8UHRofHh0aHBwgJC4nICIsIxwcKDcpLDAxNDQ0Hyc5PTgyPC4zNDL/wAALCAABAAEBAREA/8QAFAABAAAAAAAAAAAAAAAAAAAAAP/aAAgBAQAAPwD/wMv/2Q=='),
 '2026-06-07 14:10:00'),
(7,  11, 'IMAGE', 'image/jpeg',
 FROM_BASE64('/9j/4AAQSkZJRgABAQAAAQABAAD/2wBDAAgGBgcGBQgHBwcJCQgKDBQNDAsLDBkSEw8UHRofHh0aHBwgJC4nICIsIxwcKDcpLDAxNDQ0Hyc5PTgyPC4zNDL/wAALCAABAAEBAREA/8QAFAABAAAAAAAAAAAAAAAAAAAAAP/aAAgBAQAAPwCQ7pD/2Q=='),
 '2026-06-02 07:15:00'),
(8,  14, 'IMAGE', 'image/jpeg',
 FROM_BASE64('/9j/4AAQSkZJRgABAQAAAQABAAD/2wBDAAgGBgcGBQgHBwcJCQgKDBQNDAsLDBkSEw8UHRofHh0aHBwgJC4nICIsIxwcKDcpLDAxNDQ0Hyc5PTgyPC4zNDL/wAALCAABAAEBAREA/8QAFAABAAAAAAAAAAAAAAAAAAAAAP/aAAgBAQAAPwDdoN3/2Q=='),
 '2026-06-10 21:20:00'),
(9,  15, 'IMAGE', 'image/jpeg',
 FROM_BASE64('/9j/4AAQSkZJRgABAQAAAQABAAD/2wBDAAgGBgcGBQgHBwcJCQgKDBQNDAsLDBkSEw8UHRofHh0aHBwgJC4nICIsIxwcKDcpLDAxNDQ0Hyc5PTgyPC4zNDL/wAALCAABAAEBAREA/8QAFAABAAAAAAAAAAAAAAAAAAAAAP/aAAgBAQAAPwAZGXD/2Q=='),
 '2026-06-04 23:10:00'),
(10, 16, 'IMAGE', 'image/jpeg',
 FROM_BASE64('/9j/4AAQSkZJRgABAQAAAQABAAD/2wBDAAgGBgcGBQgHBwcJCQgKDBQNDAsLDBkSEw8UHRofHh0aHBwgJC4nICIsIxwcKDcpLDAxNDQ0Hyc5PTgyPC4zNDL/wAALCAABAAEBAREA/8QAFAABAAAAAAAAAAAAAAAAAAAAAP/aAAgBAQAAPwDm5vr/2Q=='),
 '2026-06-06 23:25:00'),
(11, 18, 'IMAGE', 'image/jpeg',
 FROM_BASE64('/9j/4AAQSkZJRgABAQAAAQABAAD/2wBDAAgGBgcGBQgHBwcJCQgKDBQNDAsLDBkSEw8UHRofHh0aHBwgJC4nICIsIxwcKDcpLDAxNDQ0Hyc5PTgyPC4zNDL/wAALCAABAAEBAREA/8QAFAABAAAAAAAAAAAAAAAAAAAAAP/aAAgBAQAAPwD/oHr/2Q=='),
 '2026-06-09 20:45:00');

-- ─────────────────────────────────────────────────────────────────────────────
-- Community Comments (40: 2 per post, one top-level + one reply each)
-- ─────────────────────────────────────────────────────────────────────────────
INSERT INTO community_comments (comment_id, post_id, user_id, parent_comment_id, content, attachments, likes_count, status, created_at, updated_at)
VALUES
(1,  1,  2, NULL, '听起来好棒，我也准备明早跟练一次看看效果。',   '[]', 6, 1, '2026-06-05 08:10:00', '2026-06-05 08:10:00'),
(2,  1,  1, 1,    '一起加油，记得结束后做个伸展更舒服。',           '[]', 4, 1, '2026-06-05 08:14:00', '2026-06-05 08:14:00'),
(3,  2,  3, NULL, '我可以加入手帐计划，周末互相打卡吧！',          '[]', 5, 1, '2026-06-06 11:20:00', '2026-06-06 11:20:00'),
(4,  2,  1, 3,    '好呀，我创建了共享任务列表，私信你。',            '[]', 3, 1, '2026-06-06 11:28:00', '2026-06-06 11:28:00'),
(5,  3,  4, NULL, '感谢分享，我这周正好在找助眠的钢琴曲。',        '[]', 7, 1, '2026-06-08 08:10:00', '2026-06-08 08:10:00'),
(6,  3,  1, 5,    '如果你喜欢轻柔的，可以试试月光奏鸣曲慢板。',    '[]', 4, 1, '2026-06-08 08:12:00', '2026-06-08 08:12:00'),
(7,  4,  5, NULL, '海风真的很提神，记得带保暖哦。',                 '[]', 6, 1, '2026-06-04 19:20:00', '2026-06-04 19:20:00'),
(8,  4,  2, 7,    '收到！下次准备一个围巾，感谢提醒。',             '[]', 2, 1, '2026-06-04 19:24:00', '2026-06-04 19:24:00'),
(9,  5,  1, NULL, '这个动作我也在用，配合慢数很有效。',            '[]', 5, 1, '2026-06-08 12:40:00', '2026-06-08 12:40:00'),
(10, 5,  2, 9,    '太好了，我们多交流缓解焦虑的小技巧。',           '[]', 3, 1, '2026-06-08 12:45:00', '2026-06-08 12:45:00'),
(11, 6,  3, NULL, '走神很正常，我会轻声提醒自己回到呼吸上。',      '[]', 4, 1, '2026-06-09 08:30:00', '2026-06-09 08:30:00'),
(12, 6,  2, 11,   '谢谢提示，下次试试专注在空气进出的感觉。',       '[]', 2, 1, '2026-06-09 08:34:00', '2026-06-09 08:34:00'),
(13, 7,  4, NULL, '节奏很稳！建议热身时加上脚踝放松。',            '[]', 6, 1, '2026-06-03 07:05:00', '2026-06-03 07:05:00'),
(14, 7,  3, 13,   '好建议，我之前老忘记，等下就补上。',             '[]', 3, 1, '2026-06-03 07:08:00', '2026-06-03 07:08:00'),
(15, 8,  5, NULL, '小狐狸好可爱，能分享纸型来源吗？',              '[]', 4, 1, '2026-06-07 15:45:00', '2026-06-07 15:45:00'),
(16, 8,  3, 15,   '来自一个免费模板网站，回头把链接贴上。',        '[]', 2, 1, '2026-06-07 15:50:00', '2026-06-07 15:50:00'),
(17, 9,  2, NULL, '燕麦酸奶加点草莓干也不错！',                    '[]', 3, 1, '2026-06-08 09:10:00', '2026-06-08 09:10:00'),
(18, 9,  3, 17,   '听起来很赞，晚上去买草莓干！',                  '[]', 1, 1, '2026-06-08 09:12:00', '2026-06-08 09:12:00'),
(19, 10, 1, NULL, '正念进餐真的能让胃舒服不少。',                  '[]', 4, 1, '2026-06-09 12:25:00', '2026-06-09 12:25:00'),
(20, 10, 3, 19,   '是的，还能更快察觉饱腹感。',                     '[]', 2, 1, '2026-06-09 12:28:00', '2026-06-09 12:28:00'),
(21, 11, 1, NULL, '阳光穿过树叶的样子想象出来就好治愈。',          '[]', 5, 1, '2026-06-02 08:10:00', '2026-06-02 08:10:00'),
(22, 11, 4, 21,   '欢迎哪天一起散步，分享更多光影瞬间。',           '[]', 3, 1, '2026-06-02 08:14:00', '2026-06-02 08:14:00'),
(23, 12, 5, NULL, '好喜欢这种写信的方式，也想试着写给那个时候的自己。', '[]', 7, 1, '2026-06-07 07:10:00', '2026-06-07 07:10:00'),
(24, 12, 4, 23,   '写完真的会更温柔地看待自己，推荐试试。',        '[]', 3, 1, '2026-06-07 07:15:00', '2026-06-07 07:15:00'),
(25, 13, 2, NULL, '旧票根一定装着好多故事，期待你分享。',          '[]', 4, 1, '2026-06-08 17:10:00', '2026-06-08 17:10:00'),
(26, 13, 4, 25,   '改天整理成帖子，分享给大家。',                  '[]', 2, 1, '2026-06-08 17:14:00', '2026-06-08 17:14:00'),
(27, 14, 3, NULL, '猫猫的陪伴真的能缓解很多焦虑。',                '[]', 6, 1, '2026-06-10 21:45:00', '2026-06-10 21:45:00'),
(28, 14, 4, 27,   '是啊，它总能在我最需要的时候靠过来。',           '[]', 3, 1, '2026-06-10 21:49:00', '2026-06-10 21:49:00'),
(29, 15, 2, NULL, '星空照片好美！昨晚的云层终于散开了。',          '[]', 8, 1, '2026-06-05 00:20:00', '2026-06-05 00:20:00'),
(30, 15, 5, 29,   '下次一起观星吧，我再带一台双筒望远镜。',        '[]', 4, 1, '2026-06-05 00:25:00', '2026-06-05 00:25:00'),
(31, 16, 1, NULL, '薰衣草精油真的很助眠，我也常用。',              '[]', 5, 1, '2026-06-07 00:15:00', '2026-06-07 00:15:00'),
(32, 16, 5, 31,   '谢谢认同，我也在尝试加入舒缓伸展。',             '[]', 2, 1, '2026-06-07 00:18:00', '2026-06-07 00:18:00'),
(33, 17, 3, NULL, '感恩清单写完会觉得内心变柔软。',                '[]', 4, 1, '2026-06-08 22:05:00', '2026-06-08 22:05:00'),
(34, 17, 5, 33,   '是的，而且能慢慢记录生活闪光点。',               '[]', 2, 1, '2026-06-08 22:08:00', '2026-06-08 22:08:00'),
(35, 18, 1, NULL, '夜跑注意脚步安全，我通常带个反光臂带。',        '[]', 6, 1, '2026-06-09 21:40:00', '2026-06-09 21:40:00'),
(36, 18, 5, 35,   '好主意，今晚就加上，谢谢提醒！',                '[]', 3, 1, '2026-06-09 21:45:00', '2026-06-09 21:45:00'),
(37, 19, 4, NULL, '互助小组的氛围听起来好正向，想了解如何报名。',  '[]', 5, 1, '2026-06-11 16:05:00', '2026-06-11 16:05:00'),
(38, 19, 2, 37,   '后台发链接给你了，也欢迎你分享经验。',           '[]', 3, 1, '2026-06-11 16:10:00', '2026-06-11 16:10:00'),
(39, 20, 1, NULL, '浅绿色真有生命力，祝我们都有新芽。',            '[]', 4, 1, '2026-06-11 19:00:00', '2026-06-11 19:00:00'),
(40, 20, 3, 39,   '谢谢祝福，一起继续记录心情颜色。',               '[]', 2, 1, '2026-06-11 19:05:00', '2026-06-11 19:05:00');

-- ─────────────────────────────────────────────────────────────────────────────
-- Depression Scales (3)
-- ─────────────────────────────────────────────────────────────────────────────
INSERT INTO depression_scales (scale_id, scale_name, scale_description, min_score, max_score, severity_ranges, questions, created_at, updated_at)
VALUES
(1, 'PHQ-9', '患者健康问卷抑郁量表（PHQ-9），用于评估过去两周的抑郁程度，9 题累计 0-27 分。',
 0, 27,
 '[{"range":"0-4","label":"无或最小程度抑郁"},{"range":"5-9","label":"轻度抑郁"},{"range":"10-14","label":"中度抑郁"},{"range":"15-19","label":"中重度抑郁"},{"range":"20-27","label":"重度抑郁"}]',
 '[{"id":1,"text":"对做事情缺乏兴趣或乐趣","options":[{"score":0,"label":"完全没有"},{"score":1,"label":"几天"},{"score":2,"label":"超过一半天"},{"score":3,"label":"几乎每天"}]},{"id":2,"text":"感到情绪低落、沮丧或无望","options":[{"score":0,"label":"完全没有"},{"score":1,"label":"几天"},{"score":2,"label":"超过一半天"},{"score":3,"label":"几乎每天"}]},{"id":3,"text":"难以入睡、睡眠不佳或睡得过多","options":[{"score":0,"label":"完全没有"},{"score":1,"label":"几天"},{"score":2,"label":"超过一半天"},{"score":3,"label":"几乎每天"}]},{"id":4,"text":"感到疲乏或精力不足","options":[{"score":0,"label":"完全没有"},{"score":1,"label":"几天"},{"score":2,"label":"超过一半天"},{"score":3,"label":"几乎每天"}]},{"id":5,"text":"食欲不振或过度饮食","options":[{"score":0,"label":"完全没有"},{"score":1,"label":"几天"},{"score":2,"label":"超过一半天"},{"score":3,"label":"几乎每天"}]},{"id":6,"text":"对自己感到不满，觉得自己失败或让家人失望","options":[{"score":0,"label":"完全没有"},{"score":1,"label":"几天"},{"score":2,"label":"超过一半天"},{"score":3,"label":"几乎每天"}]},{"id":7,"text":"难以专注于做事","options":[{"score":0,"label":"完全没有"},{"score":1,"label":"几天"},{"score":2,"label":"超过一半天"},{"score":3,"label":"几乎每天"}]},{"id":8,"text":"动作或讲话变慢，或者烦躁不安","options":[{"score":0,"label":"完全没有"},{"score":1,"label":"几天"},{"score":2,"label":"超过一半天"},{"score":3,"label":"几乎每天"}]},{"id":9,"text":"觉得活着没有意义、或想到伤害自己","options":[{"score":0,"label":"完全没有"},{"score":1,"label":"几天"},{"score":2,"label":"超过一半天"},{"score":3,"label":"几乎每天"}]}]',
 '2026-06-10 09:00:00', '2026-06-10 09:00:00'),
(2, 'SDS', '自评抑郁量表（SDS），采用 4 级频率评分，原始分 20-80。',
 20, 80,
 '[{"range":"20-39","label":"无抑郁"},{"range":"40-49","label":"轻度抑郁"},{"range":"50-59","label":"中度抑郁"},{"range":"60-80","label":"重度抑郁"}]',
 '[{"id":1,"text":"我觉得闷闷不乐，情绪低沉","options":[{"score":1,"label":"很少"},{"score":2,"label":"有时"},{"score":3,"label":"经常"},{"score":4,"label":"持续"}]},{"id":2,"text":"我仍旧像往常一样享受生活中的乐趣","options":[{"score":4,"label":"持续"},{"score":3,"label":"经常"},{"score":2,"label":"有时"},{"score":1,"label":"很少"}]},{"id":3,"text":"我忽然觉得要哭了","options":[{"score":1,"label":"很少"},{"score":2,"label":"有时"},{"score":3,"label":"经常"},{"score":4,"label":"持续"}]},{"id":4,"text":"我晚上睡眠不好","options":[{"score":1,"label":"很少"},{"score":2,"label":"有时"},{"score":3,"label":"经常"},{"score":4,"label":"持续"}]},{"id":5,"text":"我吃饭和往常一样多","options":[{"score":4,"label":"持续"},{"score":3,"label":"经常"},{"score":2,"label":"有时"},{"score":1,"label":"很少"}]},{"id":6,"text":"我的心跳比平时快","options":[{"score":1,"label":"很少"},{"score":2,"label":"有时"},{"score":3,"label":"经常"},{"score":4,"label":"持续"}]},{"id":7,"text":"我平时做事慢吞吞","options":[{"score":1,"label":"很少"},{"score":2,"label":"有时"},{"score":3,"label":"经常"},{"score":4,"label":"持续"}]},{"id":8,"text":"我对未来充满信心","options":[{"score":4,"label":"持续"},{"score":3,"label":"经常"},{"score":2,"label":"有时"},{"score":1,"label":"很少"}]},{"id":9,"text":"我觉得比平时容易疲劳","options":[{"score":1,"label":"很少"},{"score":2,"label":"有时"},{"score":3,"label":"经常"},{"score":4,"label":"持续"}]},{"id":10,"text":"我的头脑依然像往常一样清楚","options":[{"score":4,"label":"持续"},{"score":3,"label":"经常"},{"score":2,"label":"有时"},{"score":1,"label":"很少"}]}]',
 '2026-06-10 09:05:00', '2026-06-10 09:05:00'),
(3, 'BDI-II', '贝克抑郁量表第二版（BDI-II），每题 0-3 级别描述，累计 0-63 分。',
 0, 63,
 '[{"range":"0-13","label":"最小抑郁"},{"range":"14-19","label":"轻度抑郁"},{"range":"20-28","label":"中度抑郁"},{"range":"29-63","label":"重度抑郁"}]',
 '[{"id":1,"text":"悲伤程度","options":[{"score":0,"label":"没有特别悲伤"},{"score":1,"label":"偶尔有些悲伤"},{"score":2,"label":"经常感到悲伤"},{"score":3,"label":"持续深度悲伤"}]},{"id":2,"text":"对未来的悲观","options":[{"score":0,"label":"对未来乐观"},{"score":1,"label":"有时担心未来"},{"score":2,"label":"觉得未来黯淡"},{"score":3,"label":"确信未来绝望"}]},{"id":3,"text":"失败感","options":[{"score":0,"label":"不觉得失败"},{"score":1,"label":"有时觉得失败"},{"score":2,"label":"常感失败"},{"score":3,"label":"一直觉得完全失败"}]},{"id":4,"text":"丧失快乐能力","options":[{"score":0,"label":"仍能享受活动"},{"score":1,"label":"快乐感下降"},{"score":2,"label":"难以从活动中获得快乐"},{"score":3,"label":"完全失去快乐感"}]},{"id":5,"text":"罪恶感","options":[{"score":0,"label":"几乎没有罪恶感"},{"score":1,"label":"偶尔感到内疚"},{"score":2,"label":"经常觉得自己不好"},{"score":3,"label":"持续觉得自己糟糕"}]},{"id":6,"text":"惩罚感","options":[{"score":0,"label":"不觉得该受罚"},{"score":1,"label":"有时觉得该受罚"},{"score":2,"label":"常常觉得该受罚"},{"score":3,"label":"确信自己应受惩罚"}]},{"id":7,"text":"不喜欢自己","options":[{"score":0,"label":"喜欢自己"},{"score":1,"label":"有时不喜欢自己"},{"score":2,"label":"经常不喜欢自己"},{"score":3,"label":"完全讨厌自己"}]},{"id":8,"text":"自我批评","options":[{"score":0,"label":"与他人一样优秀"},{"score":1,"label":"对自己严格"},{"score":2,"label":"常自我批评"},{"score":3,"label":"持续自我指责"}]},{"id":9,"text":"自杀念头","options":[{"score":0,"label":"没有想法"},{"score":1,"label":"偶尔闪过想法"},{"score":2,"label":"常想到自伤"},{"score":3,"label":"已有计划或行动"}]},{"id":10,"text":"哭泣频率","options":[{"score":0,"label":"与平时相同"},{"score":1,"label":"哭得更多"},{"score":2,"label":"几乎每天哭"},{"score":3,"label":"想哭却哭不出来"}]},{"id":11,"text":"易怒程度","options":[{"score":0,"label":"不比平时易怒"},{"score":1,"label":"稍微易怒"},{"score":2,"label":"常常易怒"},{"score":3,"label":"持续怒火难控"}]},{"id":12,"text":"社交退缩","options":[{"score":0,"label":"愿意与人交往"},{"score":1,"label":"较少社交"},{"score":2,"label":"常回避社交"},{"score":3,"label":"完全不与人交往"}]},{"id":13,"text":"优柔寡断","options":[{"score":0,"label":"决策正常"},{"score":1,"label":"决策变慢"},{"score":2,"label":"难以下决定"},{"score":3,"label":"几乎无法决定任何事"}]},{"id":14,"text":"无价值感","options":[{"score":0,"label":"感觉自己有价值"},{"score":1,"label":"偶尔怀疑价值"},{"score":2,"label":"常觉得自己无价值"},{"score":3,"label":"确信自己毫无价值"}]},{"id":15,"text":"精力水平","options":[{"score":0,"label":"精力正常"},{"score":1,"label":"精力下降"},{"score":2,"label":"精力很低"},{"score":3,"label":"几乎无精力"}]},{"id":16,"text":"睡眠变化","options":[{"score":0,"label":"睡眠正常"},{"score":1,"label":"入睡稍有困难"},{"score":2,"label":"严重失眠或嗜睡"},{"score":3,"label":"几乎无法维持睡眠"}]},{"id":17,"text":"疲劳程度","options":[{"score":0,"label":"不比平时疲劳"},{"score":1,"label":"稍感疲劳"},{"score":2,"label":"常感疲劳"},{"score":3,"label":"极度疲劳难以活动"}]},{"id":18,"text":"食欲变化","options":[{"score":0,"label":"食欲正常"},{"score":1,"label":"食欲略降或略增"},{"score":2,"label":"明显变化"},{"score":3,"label":"几乎不能进食或控制进食"}]},{"id":19,"text":"体重变化","options":[{"score":0,"label":"体重稳定"},{"score":1,"label":"轻微变化"},{"score":2,"label":"明显变化"},{"score":3,"label":"体重大幅波动"}]},{"id":20,"text":"性欲下降","options":[{"score":0,"label":"性欲正常"},{"score":1,"label":"性欲略降"},{"score":2,"label":"性欲明显下降"},{"score":3,"label":"完全没有性欲"}]}]',
 '2026-06-10 09:10:00', '2026-06-10 09:10:00');

-- ─────────────────────────────────────────────────────────────────────────────
-- Depression Assessments (5 sample records)
-- ─────────────────────────────────────────────────────────────────────────────
INSERT INTO depression_assessments (assessment_id, user_id, scale_id, assessment_date, answers, total_score, notes, created_at, updated_at)
VALUES
(1, 1, 1, '2026-06-10',
 '[{"qId":1,"score":1},{"qId":2,"score":2},{"qId":3,"score":2},{"qId":4,"score":1},{"qId":5,"score":0},{"qId":6,"score":1},{"qId":7,"score":1},{"qId":8,"score":0},{"qId":9,"score":0}]',
 8, '轻度抑郁症状，主要表现为情绪低落和睡眠不佳。', '2026-06-10 10:00:00', '2026-06-10 10:00:00'),
(2, 2, 1, '2026-06-09',
 '[{"qId":1,"score":2},{"qId":2,"score":2},{"qId":3,"score":1},{"qId":4,"score":2},{"qId":5,"score":1},{"qId":6,"score":2},{"qId":7,"score":2},{"qId":8,"score":1},{"qId":9,"score":0}]',
 13, '中度抑郁，需关注焦虑躯体化症状（手心出汗）。', '2026-06-09 15:30:00', '2026-06-09 15:30:00'),
(3, 4, 1, '2026-06-08',
 '[{"qId":1,"score":0},{"qId":2,"score":1},{"qId":3,"score":0},{"qId":4,"score":1},{"qId":5,"score":0},{"qId":6,"score":1},{"qId":7,"score":0},{"qId":8,"score":0},{"qId":9,"score":0}]',
 3, '正常范围，情绪总体稳定。', '2026-06-08 09:00:00', '2026-06-08 09:00:00'),
(4, 5, 2, '2026-06-07',
 '[{"qId":1,"score":2},{"qId":2,"score":3},{"qId":3,"score":1},{"qId":4,"score":3},{"qId":5,"score":2},{"qId":6,"score":2},{"qId":7,"score":1},{"qId":8,"score":3},{"qId":9,"score":2},{"qId":10,"score":3}]',
 45, 'SDS 评估为轻度抑郁，主要表现为睡眠困难和入睡困难。', '2026-06-07 20:00:00', '2026-06-07 20:00:00'),
(5, 3, 1, '2026-06-06',
 '[{"qId":1,"score":0},{"qId":2,"score":0},{"qId":3,"score":0},{"qId":4,"score":1},{"qId":5,"score":0},{"qId":6,"score":0},{"qId":7,"score":1},{"qId":8,"score":0},{"qId":9,"score":0}]',
 2, '几乎无抑郁症状，注意力略有困扰。', '2026-06-06 11:00:00', '2026-06-06 11:00:00');

-- ─────────────────────────────────────────────────────────────────────────────
-- Risk Detection Results (sample from conversations)
-- ─────────────────────────────────────────────────────────────────────────────
INSERT INTO risk_detection_results (id, user_id, message_id, conversation_id, risk_level, polarity, intent, target, confidence, evidence, reason, raw_payload, model_name, detector_version, is_processed, process_notes, created_at)
VALUES
(1, 1, 1, 1, 'MEDIUM', 'NEGATIVE', 'VENTING', 'SELF', 0.72,
 '["晚上一个人静下来就又想起白天的事，胸口很闷"]',
 '用户表达了明显的躯体化情绪（胸闷）和孤独感，但无即时危险。',
 '{"risk":"MEDIUM","polarity":"NEGATIVE","intent":"VENTING"}',
 'qwen2.5:14b', '1.0.0', 0, NULL, '2026-06-11 21:51:05'),
(2, 2, 9, 3, 'LOW', 'NEGATIVE', 'HELP_SEEKING', 'SELF', 0.55,
 '["下午开会前手心又开始出汗了"]',
 '焦虑躯体化症状描述，属于求助类话语，风险较低。',
 '{"risk":"LOW","polarity":"NEGATIVE","intent":"HELP_SEEKING"}',
 'qwen2.5:14b', '1.0.0', 0, NULL, '2026-06-11 15:21:05'),
(3, 4, 29, 8, 'MEDIUM', 'NEGATIVE', 'NARRATIVE', 'SELF', 0.68,
 '["谢谢你撑过了那个冬天","写完眼泪就下来了"]',
 '涉及过往困难时期的情绪释放，泪流是健康的表达方式，但需关注。',
 '{"risk":"MEDIUM","polarity":"MIXED","intent":"NARRATIVE"}',
 'qwen2.5:14b', '1.0.0', 0, NULL, '2026-06-10 22:28:05'),
(4, 5, 32, 9, 'LOW', 'NEGATIVE', 'INFO_QUERY', 'SELF', 0.42,
 '["还是有点失眠"]',
 '失眠问题表述，但与观星等积极活动伴随，风险较低。',
 '{"risk":"LOW","polarity":"NEUTRAL","intent":"INFO_QUERY"}',
 'qwen2.5:14b', '1.0.0', 0, NULL, '2026-06-11 23:02:05');

-- ─────────────────────────────────────────────────────────────────────────────
-- Psychology Categories (multi-level)
-- ─────────────────────────────────────────────────────────────────────────────
INSERT INTO psychology_categories (category_id, category_name, parent_id, description, sort_order, status, created_at, updated_at)
VALUES
(1, '情绪管理',      NULL, '关于焦虑、抑郁、愤怒等情绪调节的知识',         10, 1, '2026-06-01 00:00:00', '2026-06-01 00:00:00'),
(2, '睡眠改善',      NULL, '失眠、睡眠质量提升的认知行为方法',              20, 1, '2026-06-01 00:00:00', '2026-06-01 00:00:00'),
(3, '正念冥想',      NULL, '正念减压(MBSR)、冥想引导与呼吸练习',             30, 1, '2026-06-01 00:00:00', '2026-06-01 00:00:00'),
(4, '人际关系',      NULL, '社交焦虑、亲密关系与家庭沟通',                  40, 1, '2026-06-01 00:00:00', '2026-06-01 00:00:00'),
(5, '自我成长',      NULL, '自我关怀、自尊提升与意义感探索',                50, 1, '2026-06-01 00:00:00', '2026-06-01 00:00:00'),
(6, '焦虑应对',      1,    '广泛性焦虑、社交焦虑与惊恐发作的CBT技术',        11, 1, '2026-06-01 00:00:00', '2026-06-01 00:00:00'),
(7, '抑郁情绪疏导',  1,    '轻度至中度抑郁的行为激活与认知重构',             12, 1, '2026-06-01 00:00:00', '2026-06-01 00:00:00');

-- ─────────────────────────────────────────────────────────────────────────────
-- Psychology Articles (3)
-- ─────────────────────────────────────────────────────────────────────────────
INSERT INTO psychology_articles (article_id, category_id, title, summary, content, author, source, tags, cover_image, view_count, like_count, is_featured, is_published, publish_date, created_at, updated_at)
VALUES
(1, 6, '认识广泛性焦虑：当担忧成为日常',
 '本文介绍广泛性焦虑障碍(GAD)的核心特征、日常表现与基础应对策略。',
 '广泛性焦虑障碍（Generalized Anxiety Disorder, GAD）是最常见的焦虑障碍之一。它的核心特征是过度且难以控制的担忧，持续至少六个月，并伴随肌肉紧张、易疲劳、注意力困难、睡眠障碍等躯体症状。\n\n与正常的担忧不同，GAD患者的担忧范围广泛（工作、健康、家庭、日常琐事），且强度远超出实际事件的严重程度。他们常常意识到自己的担忧过度，却无法停止。\n\n**基础应对策略：**\n1. **认知重构**：识别「灾难化思维」，用证据检验担忧的现实性。\n2. **渐进性肌肉放松**：从头到脚依次收紧再放松肌群，每天15分钟。\n3. **担忧时间限定**：每天固定20分钟作为「担忧时段」，其余时间出现担忧想法就推迟到该时段。\n4. **正念呼吸**：将注意力锚定在呼吸上，打断自动化的担忧循环。\n\n如果你发现自己持续被担忧困扰，建议寻求专业心理咨询师的帮助。',
 '张静怡 博士', '中国心理卫生杂志',
 '["焦虑","GAD","CBT","自助"]',
 NULL, 328, 56, 1, 1, '2026-06-01 08:00:00', '2026-06-01 08:00:00', '2026-06-01 08:00:00'),
(2, 2, '失眠的认知行为疗法（CBT-I）入门',
 '失眠的一线非药物治疗——CBT-I的五大核心技术简介。',
 '认知行为疗法治疗失眠（CBT-I）被美国医师学会推荐为慢性失眠的一线治疗方法。与安眠药不同，CBT-I着眼于改变导致失眠持续的行为和思维模式。\n\n**CBT-I五大核心技术：**\n\n1. **刺激控制**：床只用来睡觉和性活动。如果20分钟未能入睡就起床，直到困倦再回床。\n2. **睡眠限制**：将在床时间压缩至接近实际睡眠时间，提高睡眠效率。\n3. **认知重构**：挑战「必须睡够8小时」「今晚睡不着明天就完了」等灾难化信念。\n4. **睡眠卫生**：保持规律的作息、卧室黑暗凉爽、睡前避免咖啡因和屏幕光。\n5. **放松训练**：腹式呼吸、渐进性肌肉放松或身体扫描冥想。\n\nCBT-I通常需要4-8周才能见效，但效果持久，且没有药物依赖风险。',
 '李明 教授', '中华精神科杂志',
 '["失眠","CBT-I","睡眠卫生","非药物治疗"]',
 NULL, 512, 89, 1, 1, '2026-06-02 09:00:00', '2026-06-02 09:00:00', '2026-06-02 09:00:00'),
(3, 5, '自我关怀的三个步骤：对自己温柔的练习',
 'Neff博士的自我关怀理论在日常生活中的实践指南。',
 'Kristin Neff博士提出的自我关怀（Self-Compassion）包含三个核心成分，它不同于自尊——自尊依赖于成功和他人的评价，而自我关怀在失败和困难时给予自己同样的温柔。\n\n**自我关怀三要素：**\n\n**1. 自我善待（Self-Kindness）**\n当自己犯错或遭遇挫折时，用温和的语言取代自我批评。练习：把手放在心口，对自己说「这一刻很难，愿我对自己温柔一些」。\n\n**2. 共同人性（Common Humanity）**\n认识到痛苦和困难是人类共同的经历，而非「只有我这么糟糕」。练习：对自己说「我不是一个人在经历这个，很多人都有类似的感受」。\n\n**3. 正念觉察（Mindfulness）**\n以开放的态度观察自己的情绪，不过度沉浸也不否认。练习：给自己的情绪命名——「这是悲伤」「这是焦虑」——而不加评判。\n\n每天只需5分钟的自我关怀练习，就能显著降低焦虑和抑郁水平。',
 '王芳 心理咨询师', '简单心理',
 '["自我关怀","正念","自尊","个人成长"]',
 NULL, 427, 103, 1, 1, '2026-06-03 10:00:00', '2026-06-03 10:00:00', '2026-06-03 10:00:00');

-- ─────────────────────────────────────────────────────────────────────────────
-- Psychology Q&A (3)
-- ─────────────────────────────────────────────────────────────────────────────
INSERT INTO psychology_qna (qna_id, category_id, question, answer, expert_name, expert_title, tags, view_count, like_count, is_verified, status, created_at, updated_at)
VALUES
(1, 2, '入睡困难有什么有效的放松方法？',
 '入睡困难通常与睡前大脑过度活跃有关。以下是经过临床验证的放松方法：\n\n1. **4-7-8呼吸法**：吸气4秒→屏息7秒→缓慢呼气8秒。重复4-5轮，能激活副交感神经。\n2. **身体扫描**：从脚趾开始，逐一关注身体每个部位的感觉，慢慢移到头顶。\n3. **渐进性肌肉放松**：从脚开始收紧肌群5秒→放松10秒→感受放松的差异。\n4. **睡前写「烦恼清单」**：把脑子里盘旋的事情写下来，告诉大脑「明天再处理」。\n\n如果这些方法坚持两周仍无改善，建议进行专业的睡眠评估。',
 '陈静 睡眠医学专家', '中国睡眠研究会理事',
 '["入睡困难","放松技巧","4-7-8呼吸","身体扫描"]',
 1530, 267, 1, 1, '2026-06-04 10:00:00', '2026-06-04 10:00:00'),
(2, 6, '焦虑发作时（惊恐发作）怎么快速缓解？',
 '惊恐发作虽然感受极度不适，但并不会直接危及生命。以下是在发作时可以立即使用的「接地技术」：\n\n**5-4-3-2-1感官接地：**\n- 说出你看到的 **5** 样东西\n- 说出你触摸到的 **4** 样东西\n- 说出你听到的 **3** 个声音\n- 说出你闻到的 **2** 种气味\n- 说出你尝到的 **1** 种味道\n\n**其他即时策略：**\n- 深呼吸：用鼻子缓慢吸气4秒，用嘴巴缓缓呼气6秒\n- 握冰块或用冷水泼脸（激活潜水反射，减慢心率）\n- 提醒自己：「这是焦虑发作，会过去的，我安全」\n\n如果频繁发作，请务必寻求心理治疗——认知行为疗法和暴露疗法对惊恐障碍非常有效。',
 '赵明远 精神科医师', '北京大学第六医院副主任医师',
 '["惊恐发作","接地技术","焦虑","急性应对"]',
 2120, 398, 1, 1, '2026-06-05 11:00:00', '2026-06-05 11:00:00'),
(3, 3, '正念冥想和普通发呆有什么区别？',
 '很多人混淆了正念冥想和发呆，但它们有本质区别：\n\n| 特征 | 正念冥想 | 发呆 |\n|------|---------|------|\n| 意图 | 有意识地觉察当下 | 无意识地走神 |\n| 注意力 | 锚定一个对象（呼吸、身体、声音） | 涣散、随意漂移 |\n| 元认知 | 知道自己正在觉察 | 不知道自己在走神 |\n| 态度 | 开放、不评判、好奇 | 被动接受 |\n| 效果 | 提升专注力、降低压力 | 可能增加反刍思维 |\n\n简单来说：**正念是在「知情」状态下温柔地觉察，而发呆是「不知情」地思绪乱飘。**\n\n刚开始练习正念时，发现自己走神是非常正常的——发现走神并温和地带回注意力，这个过程本身就是正念的练习。',
 '林小雨 正念导师', 'MBSR认证导师',
 '["正念","冥想","专注力","入门科普"]',
 892, 156, 1, 1, '2026-06-06 14:00:00', '2026-06-06 14:00:00');

-- ─────────────────────────────────────────────────────────────────────────────
-- Psychology Resources (3)
-- ─────────────────────────────────────────────────────────────────────────────
INSERT INTO psychology_resources (resource_id, category_id, resource_type, title, description, file_data, external_url, file_size, mime_type, duration, thumbnail, tags, view_count, like_count, status, created_at, updated_at)
VALUES
(1, 3, 'AUDIO', '10分钟正念呼吸引导',
 '一段10分钟的引导式正念呼吸冥想音频，适合初学者每日练习。',
 NULL, 'https://res.dc.local/audio/mindful-breath-10min.mp3', 14680064, 'audio/mpeg', 600,
 NULL, '["正念","呼吸","音频","初学者"]', 640, 128, 1, '2026-06-07 08:00:00', '2026-06-07 08:00:00'),
(2, 6, 'PDF', '广泛性焦虑自助手册',
 '基于CBT的焦虑自助练习手册，包含12周的渐进式练习。',
 NULL, 'https://res.dc.local/pdf/gad-self-help.pdf', 3145728, 'application/pdf', NULL,
 NULL, '["焦虑","CBT","自助","PDF"]', 890, 201, 1, '2026-06-08 09:00:00', '2026-06-08 09:00:00'),
(3, 2, 'VIDEO', '睡前身体扫描放松引导（视频）',
 '一段20分钟的视频引导，带领你从脚趾到头顶进行全身放松。',
 NULL, 'https://res.dc.local/video/body-scan-sleep.mp4', 52428800, 'video/mp4', 1200,
 NULL, '["身体扫描","睡眠","视频","放松"]', 1240, 312, 1, '2026-06-09 21:00:00', '2026-06-09 21:00:00');

-- ─────────────────────────────────────────────────────────────────────────────
-- User Knowledge Favorites (8)
-- ─────────────────────────────────────────────────────────────────────────────
INSERT INTO user_knowledge_favorites (favorite_id, user_id, content_type, content_id, created_at)
VALUES
(1, 1, 'ARTICLE',  2, '2026-06-08 10:00:00'),
(2, 1, 'QNA',      1, '2026-06-09 22:00:00'),
(3, 2, 'ARTICLE',  1, '2026-06-07 15:00:00'),
(4, 2, 'QNA',      2, '2026-06-11 10:00:00'),
(5, 3, 'ARTICLE',  3, '2026-06-06 14:00:00'),
(6, 4, 'RESOURCE', 3, '2026-06-10 22:00:00'),
(7, 5, 'QNA',      1, '2026-06-11 23:30:00'),
(8, 5, 'ARTICLE',  2, '2026-06-11 23:45:00');

-- ─────────────────────────────────────────────────────────────────────────────
-- Content Likes (cross-table likes for posts/articles/qna)
-- ─────────────────────────────────────────────────────────────────────────────
INSERT INTO content_likes (like_id, user_id, content_type, content_id, created_at)
VALUES
(1,  1, 'community_post', 11, '2026-06-02 08:05:00'),
(2,  1, 'community_post', 14, '2026-06-10 21:30:00'),
(3,  1, 'article',         2, '2026-06-08 09:30:00'),
(4,  2, 'community_post',  1, '2026-06-05 08:00:00'),
(5,  2, 'community_post', 15, '2026-06-05 00:05:00'),
(6,  2, 'qna',             2, '2026-06-11 09:00:00'),
(7,  3, 'community_post',  7, '2026-06-03 06:50:00'),
(8,  3, 'community_post', 16, '2026-06-07 00:02:00'),
(9,  3, 'article',         3, '2026-06-06 12:00:00'),
(10, 4, 'community_post', 11, '2026-06-02 07:50:00'),
(11, 4, 'community_post', 12, '2026-06-07 07:00:00'),
(12, 4, 'resource',        3, '2026-06-10 22:10:00'),
(13, 5, 'community_post',  4, '2026-06-04 19:00:00'),
(14, 5, 'community_post', 18, '2026-06-09 21:20:00'),
(15, 5, 'article',         2, '2026-06-11 23:40:00');

-- ─────────────────────────────────────────────────────────────────────────────
-- Music (5 tracks — blob is a minimal 1×1 JPEG placeholder)
-- ─────────────────────────────────────────────────────────────────────────────
INSERT INTO music (music_id, title, artist, album, category, description, duration, file_data, file_size, mime_type, cover_image, lyrics, tags, mood_tags, status, created_at, updated_at)
VALUES
(1, '月光奏鸣曲·慢板', '贝多芬', '月光奏鸣曲', '古典',
 '贝多芬升C小调第14号钢琴奏鸣曲第一乐章，安静而深沉，适合夜间放松。',
 360, FROM_BASE64('/9j/4AAQSkZJRgABAQAAAQABAAD/2wBDAAgGBgcGBQgHBwcJCQgKDBQNDAsLDBkSEw8UHRofHh0aHBwgJC4nICIsIxwcKDcpLDAxNDQ0Hyc5PTgyPC4zNDL/wAALCAABAAEBAREA/8QAFAABAAAAAAAAAAAAAAAAAAAAAP/aAAgBAQAAPwD/tsH/2Q=='),
 5242880, 'audio/mpeg', NULL, NULL,
 '["古典","钢琴","助眠"]', '["放松","舒缓","平静"]', 1, '2026-06-01 10:00:00', '2026-06-01 10:00:00'),
(2, 'River Flows in You', 'Yiruma', 'First Love', '新世纪',
 '韩国钢琴家Yiruma的代表作，旋律流畅温柔，适合午后小憩。',
 180, FROM_BASE64('/9j/4AAQSkZJRgABAQAAAQABAAD/2wBDAAgGBgcGBQgHBwcJCQgKDBQNDAsLDBkSEw8UHRofHh0aHBwgJC4nICIsIxwcKDcpLDAxNDQ0Hyc5PTgyPC4zNDL/wAALCAABAAEBAREA/8QAFAABAAAAAAAAAAAAAAAAAAAAAP/aAAgBAQAAPwD/tsH/2Q=='),
 4718592, 'audio/mpeg', NULL, NULL,
 '["钢琴","新世纪","轻音乐"]', '["温暖","柔和","放松"]', 1, '2026-06-02 10:00:00', '2026-06-02 10:00:00'),
(3, 'Lo-Fi Chill: Late Night', 'Various', 'Lo-Fi Beats Vol.1', 'Lo-Fi',
 '低传真节拍合集，稳定的节奏感但不会太激烈，适合专注工作或夜跑。',
 240, FROM_BASE64('/9j/4AAQSkZJRgABAQAAAQABAAD/2wBDAAgGBgcGBQgHBwcJCQgKDBQNDAsLDBkSEw8UHRofHh0aHBwgJC4nICIsIxwcKDcpLDAxNDQ0Hyc5PTgyPC4zNDL/wAALCAABAAEBAREA/8QAFAABAAAAAAAAAAAAAAAAAAAAAP/aAAgBAQAAPwD/tsH/2Q=='),
 3670016, 'audio/mpeg', NULL, NULL,
 '["Lo-Fi","节拍","专注"]', '["平静","专注","节奏"]', 1, '2026-06-03 12:00:00', '2026-06-03 12:00:00'),
(4, '雨的印记', '李闰珉', '钢琴日记', '新世纪',
 '轻柔的钢琴旋律如雨滴般清澈，适合情绪低落时聆听。',
 210, FROM_BASE64('/9j/4AAQSkZJRgABAQAAAQABAAD/2wBDAAgGBgcGBQgHBwcJCQgKDBQNDAsLDBkSEw8UHRofHh0aHBwgJC4nICIsIxwcKDcpLDAxNDQ0Hyc5PTgyPC4zNDL/wAALCAABAAEBAREA/8QAFAABAAAAAAAAAAAAAAAAAAAAAP/aAAgBAQAAPwD/tsH/2Q=='),
 4194304, 'audio/mpeg', NULL, NULL,
 '["钢琴","雨声","治愈"]', '["治愈","平静","温柔"]', 1, '2026-06-04 15:00:00', '2026-06-04 15:00:00'),
(5, '薰衣草之歌', '自然之声', '睡眠白噪音', '白噪音',
 '混合了薰衣草田的微风声与远山溪流，专为睡前放松设计。',
 600, FROM_BASE64('/9j/4AAQSkZJRgABAQAAAQABAAD/2wBDAAgGBgcGBQgHBwcJCQgKDBQNDAsLDBkSEw8UHRofHh0aHBwgJC4nICIsIxwcKDcpLDAxNDQ0Hyc5PTgyPC4zNDL/wAALCAABAAEBAREA/8QAFAABAAAAAAAAAAAAAAAAAAAAAP/aAAgBAQAAPwD/tsH/2Q=='),
 7340032, 'audio/mpeg', NULL, NULL,
 '["白噪音","自然","助眠"]', '["安神","放松","助眠"]', 1, '2026-06-05 22:00:00', '2026-06-05 22:00:00');

-- ─────────────────────────────────────────────────────────────────────────────
-- Knowledge Documents (RAG source documents)
-- ─────────────────────────────────────────────────────────────────────────────
INSERT INTO knowledge_documents (document_id, source_type, source_id, owner_user_id, visibility, title, content_hash, source_version, source_updated_at, metadata, status, created_at, updated_at, deleted_at)
VALUES
(1, 'psychology_article', 1, 6, 'public', '认识广泛性焦虑：当担忧成为日常',
 'a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6e7f8a9b0c1d2e3f4a5b6c7d8e9f0a1b2', '1.0', '2026-06-01 08:00:00',
 '{"domain":"anxiety","level":"beginner","language":"zh-CN"}', 1,
 '2026-06-01 08:00:00', '2026-06-01 08:00:00', NULL),
(2, 'psychology_article', 2, 6, 'public', '失眠的认知行为疗法（CBT-I）入门',
 'b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6e7f8a9b0c1d2e3f4a5b6c7d8e9f0a1b2c3', '1.0', '2026-06-02 09:00:00',
 '{"domain":"sleep","level":"intermediate","language":"zh-CN"}', 1,
 '2026-06-02 09:00:00', '2026-06-02 09:00:00', NULL),
(3, 'psychology_article', 3, 6, 'public', '自我关怀的三个步骤：对自己温柔的练习',
 'c3d4e5f6a7b8c9d0e1f2a3b4c5d6e7f8a9b0c1d2e3f4a5b6c7d8e9f0a1b2c3d4', '1.0', '2026-06-03 10:00:00',
 '{"domain":"self-compassion","level":"beginner","language":"zh-CN"}', 1,
 '2026-06-03 10:00:00', '2026-06-03 10:00:00', NULL),
(4, 'qna', 2, 6, 'public', '焦虑发作时（惊恐发作）快速缓解 FAQ',
 'd4e5f6a7b8c9d0e1f2a3b4c5d6e7f8a9b0c1d2e3f4a5b6c7d8e9f0a1b2c3d4e5', '1.0', '2026-06-05 11:00:00',
 '{"domain":"anxiety","level":"beginner","language":"zh-CN"}', 1,
 '2026-06-05 11:00:00', '2026-06-05 11:00:00', NULL);

-- ─────────────────────────────────────────────────────────────────────────────
-- Knowledge Chunks (2 chunks per document = 8)
-- ─────────────────────────────────────────────────────────────────────────────
INSERT INTO knowledge_chunks (chunk_id, document_id, chunk_index, char_start, char_end, content, content_hash, token_count, metadata, status, created_at, vector_id, embedding_provider, embedding_model, embedding_dimension, indexed_at)
VALUES
(1, 1, 0, 0,   299,
 '广泛性焦虑障碍（Generalized Anxiety Disorder, GAD）是最常见的焦虑障碍之一。它的核心特征是过度且难以控制的担忧，持续至少六个月，并伴随肌肉紧张、易疲劳、注意力困难、睡眠障碍等躯体症状。',
 'h1a2b3c4d5e6f7a8b9c0d1e2f3a4b5c6d7e8f9a0b1c2d3e4f5a6b7c8d9e0f1a2', 52,
 '{"section":"intro","keywords":["GAD","焦虑障碍","躯体症状"]}', 1,
 '2026-06-01 08:00:00',
 'vec_doc1_chunk0_ollama_nomic_768', 'ollama', 'nomic-embed-text', 768, '2026-06-01 08:01:00'),
(2, 1, 1, 300, 500,
 '与正常的担忧不同，GAD患者的担忧范围广泛（工作、健康、家庭、日常琐事），且强度远超出实际事件的严重程度。他们常常意识到自己的担忧过度，却无法停止。',
 'i1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6e7f8a9b0c1d2e3f4a5b6c7d8e9f0a1b2', 48,
 '{"section":"symptoms","keywords":["担忧范围","无法停止"]}', 1,
 '2026-06-01 08:00:01',
 'vec_doc1_chunk1_ollama_nomic_768', 'ollama', 'nomic-embed-text', 768, '2026-06-01 08:01:01'),
(3, 2, 0, 0,   280,
 '认知行为疗法治疗失眠（CBT-I）被美国医师学会推荐为慢性失眠的一线治疗方法。与安眠药不同，CBT-I着眼于改变导致失眠持续的行为和思维模式。',
 'j1c3d4e5f6a7b8c9d0e1f2a3b4c5d6e7f8a9b0c1d2e3f4a5b6c7d8e9f0a1b2c3', 50,
 '{"section":"intro","keywords":["CBT-I","失眠","一线治疗"]}', 1,
 '2026-06-02 09:00:00',
 'vec_doc2_chunk0_ollama_nomic_768', 'ollama', 'nomic-embed-text', 768, '2026-06-02 09:01:00'),
(4, 2, 1, 281, 512,
 'CBT-I五大核心技术：1.刺激控制——床只用来睡觉和性活动。2.睡眠限制——压缩在床时间提高效率。3.认知重构——挑战关于睡眠的灾难化信念。4.睡眠卫生——规律作息、黑暗凉爽卧室。5.放松训练——腹式呼吸和身体扫描。',
 'k2d4e5f6a7b8c9d0e1f2a3b4c5d6e7f8a9b0c1d2e3f4a5b6c7d8e9f0a1b2c3d4', 62,
 '{"section":"techniques","keywords":["刺激控制","睡眠限制","放松训练"]}', 1,
 '2026-06-02 09:00:01',
 'vec_doc2_chunk1_ollama_nomic_768', 'ollama', 'nomic-embed-text', 768, '2026-06-02 09:01:01'),
(5, 3, 0, 0,   250,
 'Kristin Neff博士提出的自我关怀（Self-Compassion）包含三个核心成分：自我善待、共同人性和正念觉察。它不同于自尊——自尊依赖于成功和他人的评价，而自我关怀在失败和困难时给予自己同样的温柔。',
 'l3e5f6a7b8c9d0e1f2a3b4c5d6e7f8a9b0c1d2e3f4a5b6c7d8e9f0a1b2c3d4e5', 55,
 '{"section":"intro","keywords":["自我关怀","Neff","三要素"]}', 1,
 '2026-06-03 10:00:00',
 'vec_doc3_chunk0_ollama_nomic_768', 'ollama', 'nomic-embed-text', 768, '2026-06-03 10:01:00'),
(6, 3, 1, 251, 450,
 '自我善待：当自己犯错或遭遇挫折时，用温和的语言取代自我批评。共同人性：认识到痛苦是人类共同的经历。正念觉察：以开放的态度观察自己的情绪，不过度沉浸也不否认。每天5分钟练习就能显著降低焦虑水平。',
 'm4f6a7b8c9d0e1f2a3b4c5d6e7f8a9b0c1d2e3f4a5b6c7d8e9f0a1b2c3d4e5f6', 60,
 '{"section":"practice","keywords":["自我善待","共同人性","正念觉察"]}', 1,
 '2026-06-03 10:00:01',
 'vec_doc3_chunk1_ollama_nomic_768', 'ollama', 'nomic-embed-text', 768, '2026-06-03 10:01:01'),
(7, 4, 0, 0,   300,
 '惊恐发作虽然感受极度不适，但并不会直接危及生命。发作时可以使用5-4-3-2-1感官接地技术：说出你看到的5样东西、触摸到的4样东西、听到的3个声音、闻到的2种气味、尝到的1种味道。',
 'n5a7b8c9d0e1f2a3b4c5d6e7f8a9b0c1d2e3f4a5b6c7d8e9f0a1b2c3d4e5f6a7', 58,
 '{"section":"grounding","keywords":["惊恐发作","接地技术","5-4-3-2-1"]}', 1,
 '2026-06-05 11:00:00',
 'vec_doc4_chunk0_ollama_nomic_768', 'ollama', 'nomic-embed-text', 768, '2026-06-05 11:01:00'),
(8, 4, 1, 301, 500,
 '其他即时策略：深呼吸（鼻吸4秒、口呼6秒）、握冰块或用冷水泼脸激活潜水反射、提醒自己「这是焦虑发作，会过去的，我安全」。如果频繁发作，请务必寻求认知行为疗法和暴露疗法。',
 'o6b8c9d0e1f2a3b4c5d6e7f8a9b0c1d2e3f4a5b6c7d8e9f0a1b2c3d4e5f6a7b8', 56,
 '{"section":"strategies","keywords":["深呼吸","潜水反射","CBT"]}', 1,
 '2026-06-05 11:00:01',
 'vec_doc4_chunk1_ollama_nomic_768', 'ollama', 'nomic-embed-text', 768, '2026-06-05 11:01:01');

-- ─────────────────────────────────────────────────────────────────────────────
-- Knowledge Embeddings (1 per chunk → stored as JSON float arrays)
--   Real 768-d vectors are too long for SQL seed; using truncated placeholders.
-- ─────────────────────────────────────────────────────────────────────────────
INSERT INTO knowledge_embeddings (embedding_id, chunk_id, provider, model, dimension, embedding_json, created_at)
VALUES
(1, 1, 'ollama', 'nomic-embed-text', 768,
 '{"v":[0.012,-0.034,0.056,-0.002,0.078,-0.021,0.043,0.009,-0.067,0.031]}',
 '2026-06-01 08:01:00'),
(2, 2, 'ollama', 'nomic-embed-text', 768,
 '{"v":[-0.023,0.045,-0.012,0.067,0.034,-0.056,0.019,0.088,-0.001,0.052]}',
 '2026-06-01 08:01:01'),
(3, 3, 'ollama', 'nomic-embed-text', 768,
 '{"v":[0.041,-0.008,0.075,0.023,-0.062,0.014,0.036,-0.045,0.059,0.027]}',
 '2026-06-02 09:01:00'),
(4, 4, 'ollama', 'nomic-embed-text', 768,
 '{"v":[-0.055,0.029,0.048,-0.033,0.017,0.082,-0.014,-0.071,0.006,0.063]}',
 '2026-06-02 09:01:01'),
(5, 5, 'ollama', 'nomic-embed-text', 768,
 '{"v":[0.038,-0.047,0.022,0.061,-0.019,0.074,0.005,-0.058,0.083,0.011]}',
 '2026-06-03 10:01:00'),
(6, 6, 'ollama', 'nomic-embed-text', 768,
 '{"v":[-0.028,0.053,0.067,0.016,-0.041,-0.009,0.072,0.035,-0.064,0.044]}',
 '2026-06-03 10:01:01'),
(7, 7, 'ollama', 'nomic-embed-text', 768,
 '{"v":[0.049,-0.036,0.018,0.079,-0.025,-0.068,0.054,0.003,0.041,0.072]}',
 '2026-06-05 11:01:00'),
(8, 8, 'ollama', 'nomic-embed-text', 768,
 '{"v":[-0.011,0.064,-0.052,0.027,0.039,0.018,-0.073,0.045,0.022,-0.059]}',
 '2026-06-05 11:01:01');

-- ─────────────────────────────────────────────────────────────────────────────
-- User Memories (4 memories for alice + 3 for bruce = 7)
-- ─────────────────────────────────────────────────────────────────────────────
INSERT INTO user_memories (memory_id, user_id, memory_type, memory_key, content, confidence, salience, source_conversation_id, source_message_id, status, metadata, created_at, updated_at, last_accessed_at, access_count, expires_at, vector_id, embedding_provider, embedding_model, embedding_dimension, indexed_at)
VALUES
(1, 1, 'emotion_pattern', 'evening_anxiety',
 '用户在夜间独处时容易出现胸闷和情绪低落的模式。呼吸练习对缓解有效。',
 0.85, 0.9, 1, 1, 1,
 '{"pattern":"recurring","triggers":["nighttime","solitude"],"effective_coping":["deep_breathing"]}',
 '2026-06-11 21:52:00', '2026-06-11 21:55:00', '2026-06-11 21:55:00', 2, NULL,
 'vec_mem_1_ollama_nomic_768', 'ollama', 'nomic-embed-text', 768, '2026-06-11 21:52:05'),
(2, 1, 'preference', 'morning_routine',
 '用户喜欢在早晨进行瑜伽和手帐记录，对结构化晨间例行有积极反馈。',
 0.92, 0.7, 2, 5, 1,
 '{"routine":["yoga","journaling"],"preferred_time":"morning","response_style":"encouraging"}',
 '2026-06-11 07:12:00', '2026-06-11 07:14:00', '2026-06-11 10:05:00', 1, NULL,
 'vec_mem_2_ollama_nomic_768', 'ollama', 'nomic-embed-text', 768, '2026-06-11 07:12:05'),
(3, 1, 'coping_strategy', 'body_scan_effective',
 '身体扫描放松法在用户夜间焦虑时被推荐并收到好的反馈。',
 0.78, 0.6, 1, 4, 1,
 '{"strategy":"body_scan","context":"bedtime","effectiveness":"positive_feedback"}',
 '2026-06-11 21:55:05', '2026-06-11 21:55:05', '2026-06-11 21:55:05', 1, NULL,
 'vec_mem_3_ollama_nomic_768', 'ollama', 'nomic-embed-text', 768, '2026-06-11 21:55:10'),
(4, 1, 'fact', 'music_preference',
 '用户喜欢温柔的钢琴曲，尤其是慢节奏的古典钢琴作品用于放松和助眠。',
 0.70, 0.5, NULL, NULL, 1,
 '{"music_genres":["classical","piano"],"use_case":"sleep_aid"}',
 '2026-06-10 09:30:00', '2026-06-11 10:05:00', '2026-06-11 21:30:00', 1, NULL,
 NULL, NULL, NULL, NULL, NULL),
(5, 2, 'emotion_pattern', 'pre_meeting_anxiety',
 '用户在会议等社交-表现情境前出现手心出汗等躯体焦虑症状，握拳放松法有较好的即时缓解效果。',
 0.88, 0.85, 3, 9, 1,
 '{"pattern":"recurring","triggers":["meetings","performance"],"effective_coping":["fist_relaxation"]}',
 '2026-06-11 15:28:00', '2026-06-11 15:30:00', '2026-06-11 15:30:00', 3, NULL,
 'vec_mem_5_ollama_nomic_768', 'ollama', 'nomic-embed-text', 768, '2026-06-11 15:28:05'),
(6, 2, 'coping_strategy', 'ocean_anchor',
 '用户在海边散步时将海浪节奏作为心理锚点，在紧张时可回忆此画面放松。',
 0.80, 0.75, 4, 15, 1,
 '{"strategy":"mental_imagery","anchor":"ocean_waves","context":"stress_relief"}',
 '2026-06-10 18:39:30', '2026-06-10 18:40:00', '2026-06-11 15:21:00', 2, NULL,
 'vec_mem_6_ollama_nomic_768', 'ollama', 'nomic-embed-text', 768, '2026-06-10 18:40:05'),
(7, 2, 'fact', 'support_group_positive',
 '用户首次参加线上互助小组后反馈积极，感受到了团体的支持。',
 0.82, 0.6, NULL, NULL, 1,
 '{"activity":"online_support_group","sentiment":"positive","first_time":true}',
 '2026-06-11 16:00:00', '2026-06-11 16:00:00', '2026-06-11 16:00:00', 1, NULL,
 NULL, NULL, NULL, NULL, NULL);

-- ─────────────────────────────────────────────────────────────────────────────
-- User Memory Embeddings (for the 5 indexed memories)
-- ─────────────────────────────────────────────────────────────────────────────
INSERT INTO user_memory_embeddings (embedding_id, memory_id, provider, model, dimension, embedding_json, created_at)
VALUES
(1, 1, 'ollama', 'nomic-embed-text', 768,
 '{"v":[0.023,-0.041,0.055,0.009,-0.068,0.032,-0.017,0.044,0.076,-0.005]}',
 '2026-06-11 21:52:05'),
(2, 2, 'ollama', 'nomic-embed-text', 768,
 '{"v":[-0.017,0.062,0.039,-0.052,0.024,0.081,-0.043,0.015,0.058,-0.029]}',
 '2026-06-11 07:12:05'),
(3, 3, 'ollama', 'nomic-embed-text', 768,
 '{"v":[0.046,0.028,-0.061,0.034,0.071,-0.016,0.053,-0.038,0.012,0.067]}',
 '2026-06-11 21:55:10'),
(4, 5, 'ollama', 'nomic-embed-text', 768,
 '{"v":[-0.033,0.049,-0.022,0.065,0.018,0.074,-0.057,0.027,0.041,0.083]}',
 '2026-06-11 15:28:05'),
(5, 6, 'ollama', 'nomic-embed-text', 768,
 '{"v":[0.057,-0.013,0.044,0.032,-0.047,0.069,0.021,-0.054,0.036,0.078]}',
 '2026-06-10 18:40:05');

-- ─────────────────────────────────────────────────────────────────────────────
-- Conversation Summaries (1 per conversation = 10)
-- ─────────────────────────────────────────────────────────────────────────────
INSERT INTO conversation_summaries (summary_id, conversation_id, user_id, summary_type, content, message_start_id, message_end_id, token_count, status, summary_version, source_message_count, created_at, updated_at, vector_id, embedding_provider, embedding_model, embedding_dimension, indexed_at)
VALUES
(1, 1, 1, 'rolling',
 '用户Alice在夜间表达了胸闷和情绪低落，AI引导了三次深呼吸练习，用户反馈胸口紧张感有所缓解。AI随后建议尝试身体扫描放松法帮助入睡。',
 1, 4, 45, 1, 1, 4, '2026-06-11 21:55:06', '2026-06-11 21:55:06',
 'vec_summary_1_ollama_nomic_768', 'ollama', 'nomic-embed-text', 768, '2026-06-11 21:55:10'),
(2, 2, 1, 'rolling',
 '晨间对话，Alice计划做瑜伽和手帐，AI鼓励并陪伴完成瑜伽后，引导她写下三个小目标。整体情绪积极向上。',
 5, 8, 35, 1, 1, 4, '2026-06-11 07:14:06', '2026-06-11 07:14:06',
 'vec_summary_2_ollama_nomic_768', 'ollama', 'nomic-embed-text', 768, '2026-06-11 07:14:10'),
(3, 3, 2, 'rolling',
 'Bruce在会议前经历手心出汗的焦虑症状，AI引导了握拳放松法，用户报告症状缓解并认可该方法的有效性。',
 9, 11, 38, 1, 1, 3, '2026-06-11 15:30:01', '2026-06-11 15:30:01',
 'vec_summary_3_ollama_nomic_768', 'ollama', 'nomic-embed-text', 768, '2026-06-11 15:30:05'),
(4, 4, 2, 'rolling',
 'Bruce在海边散步时与AI分享体验，海浪节奏被作为心理锚点记下，推荐在紧张时用以回忆放松。',
 12, 16, 40, 1, 1, 5, '2026-06-10 18:40:01', '2026-06-10 18:40:01',
 'vec_summary_4_ollama_nomic_768', 'ollama', 'nomic-embed-text', 768, '2026-06-10 18:40:05'),
(5, 5, 3, 'rolling',
 'Chloe首次尝试医生建议的「吸两步呼两步」跑步呼吸法，反馈跑步变得可持续，不再那么气喘。',
 17, 19, 30, 1, 1, 3, '2026-06-11 06:45:01', '2026-06-11 06:45:01',
 'vec_summary_5_ollama_nomic_768', 'ollama', 'nomic-embed-text', 768, '2026-06-11 06:45:05'),
(6, 6, 3, 'rolling',
 'Chloe分享制作小狐狸布偶的手工体验，AI引导她感受「心流」状态并肯定了手作的不完美之美。',
 20, 23, 36, 1, 1, 4, '2026-06-10 14:30:01', '2026-06-10 14:30:01',
 NULL, NULL, NULL, NULL, NULL),
(7, 7, 4, 'rolling',
 'Dylan在森林散步后分享了阳光透过树叶的美景，AI引导他关注森林的气味和触感，增强了接地感。',
 24, 26, 32, 1, 1, 3, '2026-06-11 08:10:01', '2026-06-11 08:10:01',
 NULL, NULL, NULL, NULL, NULL),
(8, 8, 4, 'rolling',
 'Dylan给三年前的自己写了信，在分享信中内容时情绪释放流泪。AI以温柔共情的方式回应，强调了自我关怀的重要性，用户报告感到慰藉。',
 27, 31, 55, 1, 1, 5, '2026-06-10 22:33:01', '2026-06-10 22:33:01',
 NULL, NULL, NULL, NULL, NULL),
(9, 9, 5, 'rolling',
 'Elena虽然失眠但看到了猎户座，AI引导了星空冥想，结合薰衣草精油的香气进行睡前放松仪式。',
 32, 35, 38, 1, 1, 4, '2026-06-11 23:20:01', '2026-06-11 23:20:01',
 'vec_summary_9_ollama_nomic_768', 'ollama', 'nomic-embed-text', 768, '2026-06-11 23:20:05'),
(10, 10, 5, 'rolling',
 'Elena夜跑前需要音乐推荐，AI推荐了Lo-Fi慢节拍，用户试用后反馈心跳稳定、体验良好。',
 36, 38, 32, 1, 1, 3, '2026-06-10 20:55:01', '2026-06-10 20:55:01',
 NULL, NULL, NULL, NULL, NULL);

-- ─────────────────────────────────────────────────────────────────────────────
-- Agent Events (trace for conv 1: alice夜间对话)
-- ─────────────────────────────────────────────────────────────────────────────
INSERT INTO agent_events (event_id, user_id, conversation_id, session_id, trace_id, turn_id, event_type, severity, tool_name, payload, created_at)
VALUES
(1,  1, 1, 'sess_alice_night_001', 'trace_20260611_215100', 'turn_1', 'llm_call_start',    'info',    NULL,
 '{"model":"qwen2.5:14b","input_tokens":0,"system_prompt_truncated":false}',
 '2026-06-11 21:51:01'),
(2,  1, 1, 'sess_alice_night_001', 'trace_20260611_215100', 'turn_1', 'risk_detection',     'info',    'risk_detector',
 '{"risk_level":"MEDIUM","polarity":"NEGATIVE","intent":"VENTING","confidence":0.72}',
 '2026-06-11 21:51:05'),
(3,  1, 1, 'sess_alice_night_001', 'trace_20260611_215100', 'turn_1', 'memory_lookup',      'debug',   'memory_search',
 '{"memories_found":2,"top_salience":0.9,"query":"nighttime anxiety"}',
 '2026-06-11 21:51:08'),
(4,  1, 1, 'sess_alice_night_001', 'trace_20260611_215100', 'turn_1', 'llm_call_end',       'info',    NULL,
 '{"model":"qwen2.5:14b","output_tokens":28,"latency_ms":1200}',
 '2026-06-11 21:52:00'),
(5,  1, 1, 'sess_alice_night_001', 'trace_20260611_215400', 'turn_2', 'llm_call_start',    'info',    NULL,
 '{"model":"qwen2.5:14b","input_tokens":120}',
 '2026-06-11 21:54:01'),
(6,  1, 1, 'sess_alice_night_001', 'trace_20260611_215400', 'turn_2', 'memory_save',        'debug',   'memory_upsert',
 '{"memory_type":"coping_strategy","key":"body_scan_effective","confidence":0.78}',
 '2026-06-11 21:55:03'),
(7,  1, 1, 'sess_alice_night_001', 'trace_20260611_215400', 'turn_2', 'llm_call_end',       'info',    NULL,
 '{"model":"qwen2.5:14b","output_tokens":22,"latency_ms":1100}',
 '2026-06-11 21:55:05');

-- ─────────────────────────────────────────────────────────────────────────────
-- Vector Index Records (matching indexed chunks + summaries + memories)
-- ─────────────────────────────────────────────────────────────────────────────
INSERT INTO vector_index_records (record_id, vector_id, collection_name, object_type, object_id, owner_user_id, source_table, source_hash, embedding_provider, embedding_model, embedding_dimension, payload, index_status, indexed_at, failed_at, error_message, created_at, updated_at)
VALUES
(1, 'vec_doc1_chunk0_ollama_nomic_768', 'rag_chunks',            'knowledge_chunk',      1, NULL, 'knowledge_chunks',       'h1a2b3c4d5e6f7a8b9c0d1e2f3a4b5c6d7e8f9a0b1c2d3e4f5a6b7c8d9e0f1a2', 'ollama', 'nomic-embed-text', 768, '{"chunk_index":0,"document_id":1}', 'indexed', '2026-06-01 08:01:00', NULL, NULL, '2026-06-01 08:01:00', '2026-06-01 08:01:00'),
(2, 'vec_doc1_chunk1_ollama_nomic_768', 'rag_chunks',            'knowledge_chunk',      2, NULL, 'knowledge_chunks',       'i1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6e7f8a9b0c1d2e3f4a5b6c7d8e9f0a1b2', 'ollama', 'nomic-embed-text', 768, '{"chunk_index":1,"document_id":1}', 'indexed', '2026-06-01 08:01:01', NULL, NULL, '2026-06-01 08:01:01', '2026-06-01 08:01:01'),
(3, 'vec_doc2_chunk0_ollama_nomic_768', 'rag_chunks',            'knowledge_chunk',      3, NULL, 'knowledge_chunks',       'j1c3d4e5f6a7b8c9d0e1f2a3b4c5d6e7f8a9b0c1d2e3f4a5b6c7d8e9f0a1b2c3', 'ollama', 'nomic-embed-text', 768, '{"chunk_index":0,"document_id":2}', 'indexed', '2026-06-02 09:01:00', NULL, NULL, '2026-06-02 09:01:00', '2026-06-02 09:01:00'),
(4, 'vec_doc2_chunk1_ollama_nomic_768', 'rag_chunks',            'knowledge_chunk',      4, NULL, 'knowledge_chunks',       'k2d4e5f6a7b8c9d0e1f2a3b4c5d6e7f8a9b0c1d2e3f4a5b6c7d8e9f0a1b2c3d4', 'ollama', 'nomic-embed-text', 768, '{"chunk_index":1,"document_id":2}', 'indexed', '2026-06-02 09:01:01', NULL, NULL, '2026-06-02 09:01:01', '2026-06-02 09:01:01'),
(5, 'vec_mem_1_ollama_nomic_768',       'user_memories',         'user_memory',          1, 1,    'user_memories',          'a000000000000000000000000000000000000000000000000000000000000001', 'ollama', 'nomic-embed-text', 768, '{"memory_type":"emotion_pattern"}', 'indexed', '2026-06-11 21:52:05', NULL, NULL, '2026-06-11 21:52:05', '2026-06-11 21:52:05'),
(6, 'vec_summary_1_ollama_nomic_768',   'conversation_summaries', 'conversation_summary', 1, 1,    'conversation_summaries', 's0000000000000000000000000000000000000000000000000000000000000001', 'ollama', 'nomic-embed-text', 768, '{"summary_type":"rolling"}', 'indexed', '2026-06-11 21:55:10', NULL, NULL, '2026-06-11 21:55:10', '2026-06-11 21:55:10');

-- ─────────────────────────────────────────────────────────────────────────────
-- Vector Index Jobs (2 pending + 1 completed)
-- ─────────────────────────────────────────────────────────────────────────────
INSERT INTO vector_index_jobs (job_id, action, object_type, object_id, collection_name, vector_id, priority, status, attempts, max_attempts, next_run_at, locked_at, locked_by, last_error, created_at, updated_at)
VALUES
(1, 'index',  'knowledge_chunk',       5, 'rag_chunks',            'vec_doc3_chunk0_ollama_nomic_768', 100, 'completed', 1, 5, '2026-06-03 10:01:00', '2026-06-03 10:01:00', 'worker-1', NULL, '2026-06-03 10:01:00', '2026-06-03 10:01:01'),
(2, 'index',  'user_memory',           7, 'user_memories',         'vec_mem_7_ollama_nomic_768',       100, 'pending',   0, 5, '2026-06-11 16:00:05', NULL, NULL, NULL, '2026-06-11 16:00:05', '2026-06-11 16:00:05'),
(3, 'reindex','conversation_summary',  6, 'conversation_summaries', NULL,                                 200, 'pending',   0, 3, '2026-06-11 14:30:06', NULL, NULL, NULL, '2026-06-11 14:30:06', '2026-06-11 14:30:06');

SET FOREIGN_KEY_CHECKS = 1;
