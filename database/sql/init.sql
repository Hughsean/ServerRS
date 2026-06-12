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
