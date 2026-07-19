-- ============================================================================
-- init.sql — Complete database initialization for Digital Companion (ServerRS)
--
-- Squashed schema includes:
--   - authentication, refresh tokens, roles, and content likes
--   - agent, RAG, memory, Qdrant lifecycle, and stored objects
--   - complete web ingestion, publishing, outbox, and audit pipeline
--   - P0 web-ingestion index/trigger corrections
--   - MEDIUMTEXT/JSON mid-pipeline artifact persistence
--
-- All patch columns, indexes, constraints, and triggers are folded into the
-- final CREATE TABLE / CREATE TRIGGER statements below. No follow-up migration
-- is required for a fresh development database.
--
-- Execution:
--   mysql -u root -p < database/sql/init.sql
-- ============================================================================

DROP DATABASE IF EXISTS digital_companion;
CREATE DATABASE IF NOT EXISTS digital_companion
    DEFAULT CHARACTER SET utf8mb4
    DEFAULT COLLATE utf8mb4_unicode_ci;

USE digital_companion;
SET NAMES utf8mb4;

-- ============================================================================
-- 1. users — 用户基础信息表
--    (role column from patch 001 already folded in)
-- ============================================================================
CREATE TABLE users
(
    id            BIGINT UNSIGNED PRIMARY KEY AUTO_INCREMENT COMMENT '用户ID',
    username      VARCHAR(50)  NOT NULL UNIQUE COMMENT '用户名',
    password      VARCHAR(255) NOT NULL DEFAULT '__QQ_AUTO_REGISTERED__' COMMENT '密码(加密存储)，QQ用户注册时写入固定标记字符串，标记用户无法密码登录',
    email         VARCHAR(100) UNIQUE COMMENT '邮箱',
    phone         VARCHAR(20) UNIQUE COMMENT '手机号',
    avatar        BLOB COMMENT '头像二进制数据',
    nickname      VARCHAR(50) COMMENT '昵称',
    created_at    DATETIME(6)    NOT NULL DEFAULT CURRENT_TIMESTAMP(6) COMMENT '创建时间',
    updated_at    DATETIME(6)    NOT NULL DEFAULT CURRENT_TIMESTAMP(6) ON UPDATE CURRENT_TIMESTAMP(6) COMMENT '更新时间',
    last_login_at DATETIME(6)    NULL COMMENT '最后登录时间',
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
    personalization_enabled TINYINT(1)      NOT NULL DEFAULT 1 COMMENT '个性化是否启用',
    personalization_reset_at DATETIME(6)    NULL COMMENT '最近一次重置时间',
    created_at              DATETIME(6)       NOT NULL DEFAULT CURRENT_TIMESTAMP(6) COMMENT '创建时间',
    updated_at              DATETIME(6)       NOT NULL DEFAULT CURRENT_TIMESTAMP(6) ON UPDATE CURRENT_TIMESTAMP(6) COMMENT '更新时间',
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
    created_at       DATETIME(6)       NOT NULL DEFAULT CURRENT_TIMESTAMP(6) COMMENT '创建时间',
    updated_at       DATETIME(6)       NOT NULL DEFAULT CURRENT_TIMESTAMP(6) ON UPDATE CURRENT_TIMESTAMP(6) COMMENT '更新时间',
    FOREIGN KEY (user_id) REFERENCES users (id) ON DELETE CASCADE,
    INDEX idx_user_id (user_id),
    INDEX idx_created_at (created_at)
) ENGINE = InnoDB
  DEFAULT CHARSET = utf8mb4
  COLLATE = utf8mb4_unicode_ci COMMENT = '用户日记表';

-- ============================================================================
-- 5. conversations — 会话元数据表
-- 5. conversations — 用户唯一长期对话表
-- ============================================================================
CREATE TABLE conversations
(
    id              BIGINT UNSIGNED PRIMARY KEY AUTO_INCREMENT COMMENT '会话ID',
    user_id         BIGINT UNSIGNED NOT NULL COMMENT '用户ID',
    title           VARCHAR(100)    NULL COMMENT '最近主题展示名，由 milestone summary 周期性更新',
    message_count   BIGINT UNSIGNED NOT NULL DEFAULT 0 COMMENT '消息数量',
    last_message_at DATETIME(6)     NULL COMMENT '最近一条消息时间',
    created_at      DATETIME(6)     NOT NULL DEFAULT CURRENT_TIMESTAMP(6) COMMENT '创建时间',
    updated_at      DATETIME(6)     NOT NULL DEFAULT CURRENT_TIMESTAMP(6)
        ON UPDATE CURRENT_TIMESTAMP(6) COMMENT '更新时间',

    CONSTRAINT uk_conversations_user_id UNIQUE (user_id),
    CONSTRAINT fk_conversations_user
        FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE
) ENGINE = InnoDB
  DEFAULT CHARSET = utf8mb4
  COLLATE = utf8mb4_unicode_ci COMMENT = '用户唯一长期对话表';

-- ============================================================================
-- 6. conversation_messages — 会话消息表
-- 6. conversation_messages — 对话原始消息表
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
    created_at      DATETIME(6)     NOT NULL DEFAULT CURRENT_TIMESTAMP(6) COMMENT '创建时间',

    FOREIGN KEY (conversation_id) REFERENCES conversations(id) ON DELETE CASCADE,
    INDEX idx_conv_id (conversation_id, id),
    INDEX idx_conv_created (conversation_id, created_at)
) ENGINE = InnoDB
  DEFAULT CHARSET = utf8mb4
  COLLATE = utf8mb4_unicode_ci COMMENT = '对话原始消息表';

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
    created_at     DATETIME(6)       NOT NULL DEFAULT CURRENT_TIMESTAMP(6) COMMENT '创建时间',
    updated_at     DATETIME(6)       NOT NULL DEFAULT CURRENT_TIMESTAMP(6) ON UPDATE CURRENT_TIMESTAMP(6) COMMENT '更新时间',
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
    created_at DATETIME(6)       NOT NULL DEFAULT CURRENT_TIMESTAMP(6) COMMENT '创建时间',
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
    created_at      DATETIME(6)        NOT NULL DEFAULT CURRENT_TIMESTAMP(6) COMMENT '创建时间',
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
    created_at        DATETIME(6)       NOT NULL DEFAULT CURRENT_TIMESTAMP(6) COMMENT '创建时间',
    updated_at        DATETIME(6)       NOT NULL DEFAULT CURRENT_TIMESTAMP(6) ON UPDATE CURRENT_TIMESTAMP(6) COMMENT '更新时间',
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
    created_at        DATETIME(6) DEFAULT CURRENT_TIMESTAMP(6) COMMENT '创建时间',
    updated_at        DATETIME(6) DEFAULT CURRENT_TIMESTAMP(6) ON UPDATE CURRENT_TIMESTAMP(6) COMMENT '更新时间'
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
    created_at      DATETIME(6) DEFAULT CURRENT_TIMESTAMP(6) COMMENT '创建时间',
    updated_at      DATETIME(6) DEFAULT CURRENT_TIMESTAMP(6) ON UPDATE CURRENT_TIMESTAMP(6) COMMENT '更新时间',
    FOREIGN KEY (user_id) REFERENCES users (id) ON DELETE CASCADE,
    FOREIGN KEY (scale_id) REFERENCES depression_scales (scale_id),
    INDEX idx_user_assessment (user_id, assessment_date),
    INDEX idx_scale (scale_id)
) ENGINE = InnoDB
  DEFAULT CHARSET = utf8mb4
  COLLATE = utf8mb4_unicode_ci COMMENT = '抑郁评估记录表';


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
    created_at    DATETIME(6)         NOT NULL DEFAULT CURRENT_TIMESTAMP(6) COMMENT '创建时间',
    updated_at    DATETIME(6)         NOT NULL DEFAULT CURRENT_TIMESTAMP(6) ON UPDATE CURRENT_TIMESTAMP(6) COMMENT '更新时间',
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
    publish_date DATETIME(6)         NULL COMMENT '发布时间',
    created_at   DATETIME(6)         NOT NULL DEFAULT CURRENT_TIMESTAMP(6) COMMENT '创建时间',
    updated_at   DATETIME(6)         NOT NULL DEFAULT CURRENT_TIMESTAMP(6) ON UPDATE CURRENT_TIMESTAMP(6) COMMENT '更新时间',
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
    created_at   DATETIME(6)         NOT NULL DEFAULT CURRENT_TIMESTAMP(6) COMMENT '创建时间',
    updated_at   DATETIME(6)         NOT NULL DEFAULT CURRENT_TIMESTAMP(6) ON UPDATE CURRENT_TIMESTAMP(6) COMMENT '更新时间',
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
    created_at    DATETIME(6)         NOT NULL DEFAULT CURRENT_TIMESTAMP(6) COMMENT '创建时间',
    updated_at    DATETIME(6)         NOT NULL DEFAULT CURRENT_TIMESTAMP(6) ON UPDATE CURRENT_TIMESTAMP(6) COMMENT '更新时间',
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
    created_at   DATETIME(6)       NOT NULL DEFAULT CURRENT_TIMESTAMP(6) COMMENT '收藏时间',
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
    created_at  DATETIME(6)       NOT NULL DEFAULT CURRENT_TIMESTAMP(6) COMMENT '创建时间',
    updated_at  DATETIME(6)       NOT NULL DEFAULT CURRENT_TIMESTAMP(6) ON UPDATE CURRENT_TIMESTAMP(6) COMMENT '更新时间',
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
    memory_id               BIGINT UNSIGNED PRIMARY KEY AUTO_INCREMENT,
    user_id                 BIGINT UNSIGNED NOT NULL,

    memory_key              CHAR(64)        NULL COMMENT 'SHA256(canonical_form)',
    canonical_form          TEXT            NULL COMMENT '规范化表述',

    memory_type             VARCHAR(64)     NOT NULL
        COMMENT 'preference|fact|emotional_pattern|goal',

    content                 TEXT            NOT NULL,

    source_confidence       DECIMAL(3,2)    NOT NULL DEFAULT 0.50
        COMMENT 'LLM 提取时的原始置信度',
    confidence              DOUBLE          NOT NULL DEFAULT 0.7
        COMMENT '当前综合置信度（由 evidence 更新）',
    salience                DOUBLE          NOT NULL DEFAULT 0.5
        COMMENT '重要性 0-1',

    source_conversation_id  BIGINT UNSIGNED NULL,
    source_message_id       BIGINT UNSIGNED NULL,

    reinforced_at           DATETIME(6)     NULL
        COMMENT '最近一次被独立新证据加强',
    reinforce_count         INT UNSIGNED    NOT NULL DEFAULT 0
        COMMENT '被独立证据加强的次数',

    contradicted_at         DATETIME(6)     NULL,
    superseded_by           BIGINT UNSIGNED NULL,

    status                  TINYINT         NOT NULL DEFAULT 1
        COMMENT '1=active 0=disabled -1=contradicted',

    canonicalizer_version   VARCHAR(64)     NULL,
    merge_decision          VARCHAR(32)     NULL
        COMMENT 'same|related|new_evidence|contradiction|new',
    merge_reason            TEXT            NULL,

    metadata                JSON            NULL,
    last_accessed_at        DATETIME(6)     NULL,
    access_count            INT UNSIGNED    NOT NULL DEFAULT 0,

    vector_id               VARCHAR(128)    NULL,
    embedding_provider      VARCHAR(64)     NULL,
    embedding_model         VARCHAR(128)    NULL,
    embedding_dimension     INT UNSIGNED    NULL,
    indexed_at              DATETIME(6)     NULL,

    created_at              DATETIME(6)     NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    updated_at              DATETIME(6)     NOT NULL DEFAULT CURRENT_TIMESTAMP(6)
        ON UPDATE CURRENT_TIMESTAMP(6),

    FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE,
    FOREIGN KEY (source_conversation_id) REFERENCES conversations(id)
        ON DELETE SET NULL,
    FOREIGN KEY (source_message_id) REFERENCES conversation_messages(id)
        ON DELETE SET NULL,

    UNIQUE INDEX uk_user_memory_key (user_id, memory_key),
    UNIQUE INDEX uk_memory_vector_id (vector_id),
    INDEX idx_user_status_salience (user_id, status, salience DESC),
    FULLTEXT INDEX ft_memory_content (content),
    CONSTRAINT chk_user_memories_type
        CHECK (memory_type IN ('preference','fact','emotional_pattern','goal'))
) ENGINE = InnoDB
  DEFAULT CHARSET = utf8mb4
  COLLATE = utf8mb4_unicode_ci
  COMMENT = '用户长期记忆表';


-- ============================================================================
-- 26. conversation_summaries — 会话摘要表
--     (base from patch 002,
--      +vector_id/+embedding_provider/+embedding_model/+embedding_dimension/
--      +indexed_at from patch 003,
-- 26. conversation_summaries — 对话 general 摘要表
-- ============================================================================
CREATE TABLE conversation_summaries
(
    summary_id          BIGINT UNSIGNED PRIMARY KEY AUTO_INCREMENT,
    conversation_id     BIGINT UNSIGNED NOT NULL,
    user_id             BIGINT UNSIGNED NOT NULL,

    summary_type        VARCHAR(32)     NOT NULL
        COMMENT 'rolling_general|milestone_general',
    content             TEXT            NOT NULL,

    message_start_id    BIGINT UNSIGNED NOT NULL,
    message_end_id      BIGINT UNSIGNED NOT NULL,

    supersedes_id       BIGINT UNSIGNED NULL,

    token_count         INT UNSIGNED    NULL,

    vector_id           VARCHAR(128)    NULL,
    embedding_provider  VARCHAR(64)     NULL,
    embedding_model     VARCHAR(128)    NULL,
    embedding_dimension INT UNSIGNED    NULL,
    indexed_at          DATETIME(6)     NULL,

    status              TINYINT         NOT NULL DEFAULT 1 COMMENT '1=active 0=disabled',

    created_at          DATETIME(6)     NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    updated_at          DATETIME(6)     NOT NULL DEFAULT CURRENT_TIMESTAMP(6)
        ON UPDATE CURRENT_TIMESTAMP(6),

    FOREIGN KEY (conversation_id) REFERENCES conversations(id) ON DELETE CASCADE,
    FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE,

    -- Functional index avoids MySQL's generated-column + cascading-FK restriction.
    UNIQUE KEY uk_active_rolling_general ((
        CASE WHEN status = 1 AND summary_type = 'rolling_general'
             THEN conversation_id ELSE NULL END
    )),
    INDEX idx_conv_type_status_end (conversation_id, summary_type, status, message_end_id),
    INDEX idx_user_status (user_id, status),
    INDEX idx_vector_id (vector_id),
    CONSTRAINT chk_conversation_summaries_type
        CHECK (summary_type IN ('rolling_general','milestone_general')),
    CONSTRAINT chk_conversation_summaries_range
        CHECK (message_start_id <= message_end_id)
) ENGINE = InnoDB
  DEFAULT CHARSET = utf8mb4
  COLLATE = utf8mb4_unicode_ci
  COMMENT = '对话 general 摘要表';

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

-- ============================================================================
-- 30. web_sources — 网页来源策略配置表
-- ============================================================================
CREATE TABLE web_sources
(
    id               BIGINT UNSIGNED PRIMARY KEY AUTO_INCREMENT COMMENT '来源ID',
    name             VARCHAR(128)    NOT NULL COMMENT '来源名称',
    description      TEXT            NULL COMMENT '来源描述',
    approval_status  VARCHAR(32)     NOT NULL DEFAULT 'pending'
                     COMMENT '审核状态: pending/approved/rejected/disabled',
    trust_level      VARCHAR(32)     NOT NULL DEFAULT 'normal'
                     COMMENT '信任级别: official/trusted/normal/untrusted',
    auto_publish     TINYINT(1)      NOT NULL DEFAULT 0
                     COMMENT '是否自动发布（仍需通过质量门控）',
    allowed_domains  JSON            NULL
                     COMMENT '允许抓取的域名列表（JSON数组）',
    default_language VARCHAR(16)     NOT NULL DEFAULT 'zh'
                     COMMENT '默认语言代码',
    enabled          TINYINT(1)      NOT NULL DEFAULT 0
                     COMMENT '是否启用: 1启用, 0禁用',
    created_at       DATETIME(6)     NOT NULL DEFAULT CURRENT_TIMESTAMP(6)
                     COMMENT '创建时间',
    updated_at       DATETIME(6)     NOT NULL DEFAULT CURRENT_TIMESTAMP(6)
                     ON UPDATE CURRENT_TIMESTAMP(6)
                     COMMENT '更新时间',
    deleted_at       DATETIME(6)     NULL COMMENT '软删除时间',
    INDEX idx_web_sources_approval (approval_status),
    INDEX idx_web_sources_enabled (enabled),
    INDEX idx_web_sources_trust (trust_level)
) ENGINE = InnoDB
  DEFAULT CHARSET = utf8mb4
  COLLATE = utf8mb4_unicode_ci COMMENT = '网页来源策略配置表';

-- ============================================================================
-- 31. web_source_urls — 来源下的待抓取 URL
-- ============================================================================
CREATE TABLE web_source_urls
(
    id                  BIGINT UNSIGNED PRIMARY KEY AUTO_INCREMENT COMMENT 'URL记录ID',
    source_id           BIGINT UNSIGNED NOT NULL COMMENT '关联来源ID',
    url                 TEXT            NOT NULL COMMENT '原始URL',
    canonical_url       TEXT            NULL COMMENT '规范化URL',
    url_hash            CHAR(64)        NOT NULL COMMENT '规范化URL的SHA-256',
    enabled             TINYINT(1)      NOT NULL DEFAULT 1
                        COMMENT '是否启用: 1启用, 0禁用',
    crawl_interval_secs INT UNSIGNED    NOT NULL DEFAULT 86400
                        COMMENT '抓取间隔（秒），默认24小时',
    last_crawled_at     DATETIME(6)     NULL COMMENT '最近一次抓取时间',
    last_content_hash   CHAR(64)        NULL COMMENT '最近一次抓取的content_hash',
    created_at          DATETIME(6)     NOT NULL DEFAULT CURRENT_TIMESTAMP(6)
                        COMMENT '创建时间',
    updated_at          DATETIME(6)     NOT NULL DEFAULT CURRENT_TIMESTAMP(6)
                        ON UPDATE CURRENT_TIMESTAMP(6)
                        COMMENT '更新时间',
    deleted_at          DATETIME(6)     NULL COMMENT '软删除时间',
    UNIQUE KEY uk_web_source_urls_source_hash (source_id, url_hash),
    INDEX idx_web_source_urls_source (source_id),
    INDEX idx_web_source_urls_enabled (enabled),
    INDEX idx_web_source_urls_last_crawled (last_crawled_at),
    FOREIGN KEY (source_id) REFERENCES web_sources (id) ON DELETE CASCADE
) ENGINE = InnoDB
  DEFAULT CHARSET = utf8mb4
  COLLATE = utf8mb4_unicode_ci COMMENT = '来源待抓取URL表';

-- ============================================================================
-- 32. web_crawl_jobs — 定时抓取批次
-- ============================================================================
CREATE TABLE web_crawl_jobs
(
    id           BIGINT UNSIGNED PRIMARY KEY AUTO_INCREMENT COMMENT '抓取批次ID',
    source_id    BIGINT UNSIGNED NULL COMMENT '关联来源ID（可为NULL表示跨来源批次）',
    status       VARCHAR(32)     NOT NULL DEFAULT 'pending'
                 COMMENT '状态: pending/running/succeeded/failed/dead/cancelled',
    scheduled_at DATETIME(6)     NOT NULL COMMENT '计划执行时间',
    started_at   DATETIME(6)     NULL COMMENT '实际开始时间',
    finished_at  DATETIME(6)     NULL COMMENT '完成时间',
    last_error   TEXT            NULL COMMENT '最近一次错误信息',
    created_at   DATETIME(6)     NOT NULL DEFAULT CURRENT_TIMESTAMP(6) COMMENT '创建时间',
    updated_at   DATETIME(6)     NOT NULL DEFAULT CURRENT_TIMESTAMP(6)
                 ON UPDATE CURRENT_TIMESTAMP(6) COMMENT '更新时间',
    INDEX idx_web_crawl_jobs_status_scheduled (status, scheduled_at),
    INDEX idx_web_crawl_jobs_source (source_id),
    INDEX idx_web_crawl_jobs_created (created_at),
    FOREIGN KEY (source_id) REFERENCES web_sources (id) ON DELETE SET NULL
) ENGINE = InnoDB
  DEFAULT CHARSET = utf8mb4
  COLLATE = utf8mb4_unicode_ci COMMENT = '定时抓取批次表';

-- ============================================================================
-- 33. web_pages — source 下的网页实体
-- ============================================================================
CREATE TABLE web_pages
(
    id                    BIGINT UNSIGNED PRIMARY KEY AUTO_INCREMENT COMMENT '网页实体ID',
    source_id             BIGINT UNSIGNED NOT NULL COMMENT '关联来源ID',
    source_url_id         BIGINT UNSIGNED NULL COMMENT '关联URL记录ID',
    url                   TEXT            NOT NULL COMMENT '当前URL',
    canonical_url         TEXT            NULL COMMENT '规范化URL',
    url_hash              CHAR(64)        NOT NULL COMMENT '规范化URL的SHA-256',
    latest_content_hash   CHAR(64)        NULL COMMENT '最近一次成功抓取的content_hash',
    latest_success_run_id BIGINT UNSIGNED NULL COMMENT '最近一次成功ingestion run的ID',
    last_fetched_at       DATETIME(6)     NULL COMMENT '最近一次抓取时间',
    created_at            DATETIME(6)     NOT NULL DEFAULT CURRENT_TIMESTAMP(6)
                          COMMENT '创建时间',
    updated_at            DATETIME(6)     NOT NULL DEFAULT CURRENT_TIMESTAMP(6)
                          ON UPDATE CURRENT_TIMESTAMP(6)
                          COMMENT '更新时间',
    deleted_at            DATETIME(6)     NULL COMMENT '软删除时间',
    UNIQUE KEY uk_web_pages_source_hash (source_id, url_hash),
    INDEX idx_web_pages_source (source_id),
    INDEX idx_web_pages_source_url (source_url_id),
    INDEX idx_web_pages_latest_run (latest_success_run_id),
    FOREIGN KEY (source_id) REFERENCES web_sources (id) ON DELETE CASCADE,
    FOREIGN KEY (source_url_id) REFERENCES web_source_urls (id) ON DELETE SET NULL
) ENGINE = InnoDB
  DEFAULT CHARSET = utf8mb4
  COLLATE = utf8mb4_unicode_ci COMMENT = '网页实体表';

-- ============================================================================
-- 34. knowledge_ingestion_runs — 内容版本处理流程
-- ============================================================================
CREATE TABLE knowledge_ingestion_runs
(
    id                  BIGINT UNSIGNED PRIMARY KEY AUTO_INCREMENT COMMENT 'Run ID',
    source_id           BIGINT UNSIGNED NOT NULL COMMENT '关联来源ID',
    source_url_id       BIGINT UNSIGNED NULL COMMENT '关联URL记录ID',
    crawl_job_id        BIGINT UNSIGNED NULL COMMENT '关联抓取批次ID',
    page_id             BIGINT UNSIGNED NOT NULL COMMENT '关联网页实体ID',
    content_hash        CHAR(64)        NOT NULL COMMENT '内容SHA-256',
    content_key         CHAR(64)        NOT NULL COMMENT 'sha256(source_id+page_id+content_hash)',
    run_key             CHAR(64)        NOT NULL COMMENT '完整处理配置的幂等键',
    version_key         CHAR(64)        NOT NULL COMMENT '版本键，= run_key',
    status              VARCHAR(32)     NOT NULL DEFAULT 'pending'
                        COMMENT 'pending/running/staged/published/rejected/skipped/failed/dead/cancelled',
    stage               VARCHAR(32)     NOT NULL DEFAULT 'pending'
                        COMMENT '当前处理阶段',
    llm_provider        VARCHAR(64)     NULL COMMENT '使用的LLM provider',
    llm_model           VARCHAR(128)    NULL COMMENT '使用的LLM模型名',
    llm_prompt_version  VARCHAR(64)     NULL COMMENT 'prompt版本标识',
    llm_input_tokens    INT UNSIGNED    NULL COMMENT 'LLM输入token数',
    llm_output_tokens   INT UNSIGNED    NULL COMMENT 'LLM输出token数',
    chunker_version     VARCHAR(64)     NULL COMMENT 'chunker版本标识',
    embedding_provider  VARCHAR(64)     NULL COMMENT 'embedding provider',
    embedding_model     VARCHAR(128)    NULL COMMENT 'embedding模型名',
    embedding_dimension INT UNSIGNED    NULL COMMENT 'embedding维度',
    quality_score       DOUBLE          NULL COMMENT '质量分数 0.0-1.0',
    quality_result      JSON            NULL COMMENT '质量门控详细结果',
    risk_flags          JSON            NULL COMMENT '风险标记（JSON数组）',
    should_publish      TINYINT(1)      NULL COMMENT '质量门控是否建议发布',
    last_error          TEXT            NULL COMMENT '最近一次错误信息',
    retry_count         INT UNSIGNED    NOT NULL DEFAULT 0 COMMENT '重试次数',
    started_at          DATETIME(6)     NULL COMMENT '开始处理时间',
    finished_at         DATETIME(6)     NULL COMMENT '完成时间',
    created_at          DATETIME(6)     NOT NULL DEFAULT CURRENT_TIMESTAMP(6) COMMENT '创建时间',
    updated_at          DATETIME(6)     NOT NULL DEFAULT CURRENT_TIMESTAMP(6)
                        ON UPDATE CURRENT_TIMESTAMP(6) COMMENT '更新时间',
    fetched_body_text   MEDIUMTEXT      NULL COMMENT '原始抓取的网页正文（max ~5MB from fetcher）',
    clean_text          MEDIUMTEXT      NULL COMMENT '经 extractor 清洗后的纯文本',
    distilled_json      JSON            NULL COMMENT 'Distill LLM 返回的完整结构化 JSON',
    UNIQUE KEY uk_ingestion_runs_run_key (run_key),
    UNIQUE KEY uk_ingestion_runs_version_key (version_key),
    INDEX idx_ingestion_runs_content_key (content_key),
    INDEX idx_ingestion_runs_page (page_id),
    INDEX idx_ingestion_runs_source (source_id),
    INDEX idx_ingestion_runs_source_url (source_url_id),
    INDEX idx_ingestion_runs_crawl_job (crawl_job_id),
    INDEX idx_ingestion_runs_status_stage (status, stage),
    INDEX idx_ingestion_runs_created (created_at),
    FOREIGN KEY (source_id) REFERENCES web_sources (id) ON DELETE CASCADE,
    FOREIGN KEY (source_url_id) REFERENCES web_source_urls (id) ON DELETE SET NULL,
    FOREIGN KEY (crawl_job_id) REFERENCES web_crawl_jobs (id) ON DELETE SET NULL,
    FOREIGN KEY (page_id) REFERENCES web_pages (id) ON DELETE CASCADE
) ENGINE = InnoDB
  DEFAULT CHARSET = utf8mb4
  COLLATE = utf8mb4_unicode_ci COMMENT = '内容版本处理流程表';

-- ============================================================================
-- 35. knowledge_publish_records — 发布版本记录
-- ============================================================================
CREATE TABLE knowledge_publish_records
(
    id                         BIGINT UNSIGNED PRIMARY KEY AUTO_INCREMENT COMMENT '发布记录ID',
    source_id                  BIGINT UNSIGNED NOT NULL COMMENT '关联来源ID',
    page_id                    BIGINT UNSIGNED NOT NULL COMMENT '关联网页实体ID',
    run_id                     BIGINT UNSIGNED NOT NULL COMMENT '关联ingestion run ID',
    document_id                BIGINT UNSIGNED NOT NULL COMMENT '关联knowledge_documents.document_id',
    version_key                CHAR(64)        NOT NULL COMMENT '版本键',
    content_hash               CHAR(64)        NOT NULL COMMENT '内容SHA-256',
    publish_status             VARCHAR(32)     NOT NULL DEFAULT 'staged'
                               COMMENT 'staged/publishing/published/superseded/rolled_back/failed',
    active                     TINYINT(1)      NOT NULL DEFAULT 0
                               COMMENT '是否当前活跃版本',
    active_page_key            VARCHAR(128)    NULL
                               COMMENT 'active=1时为source_id:page_id，由trigger自动维护',
    activated_at               DATETIME(6)     NULL COMMENT '激活时间',
    superseded_at              DATETIME(6)     NULL COMMENT '被新版本替代的时间',
    superseded_by_record_id    BIGINT UNSIGNED NULL COMMENT '替代此版本的发布记录ID（应用层FK）',
    rolled_back_from_record_id BIGINT UNSIGNED NULL COMMENT 'rollback来源记录ID（应用层FK）',
    created_at                 DATETIME(6)     NOT NULL DEFAULT CURRENT_TIMESTAMP(6) COMMENT '创建时间',
    updated_at                 DATETIME(6)     NOT NULL DEFAULT CURRENT_TIMESTAMP(6)
                               ON UPDATE CURRENT_TIMESTAMP(6) COMMENT '更新时间',
    UNIQUE KEY uk_publish_records_one_active_page (active_page_key),
    INDEX idx_publish_records_page (source_id, page_id),
    INDEX idx_publish_records_run (run_id),
    INDEX idx_publish_records_document (document_id),
    INDEX idx_publish_records_status (publish_status),
    INDEX idx_publish_records_version_key (version_key),
    FOREIGN KEY (source_id) REFERENCES web_sources (id) ON DELETE CASCADE,
    FOREIGN KEY (page_id) REFERENCES web_pages (id) ON DELETE CASCADE,
    FOREIGN KEY (run_id) REFERENCES knowledge_ingestion_runs (id) ON DELETE CASCADE,
    FOREIGN KEY (document_id) REFERENCES knowledge_documents (document_id) ON DELETE CASCADE
) ENGINE = InnoDB
  DEFAULT CHARSET = utf8mb4
  COLLATE = utf8mb4_unicode_ci COMMENT = '知识发布版本记录表';

CREATE TRIGGER trg_kpr_active_page_key_bi
  BEFORE INSERT ON knowledge_publish_records
  FOR EACH ROW
  SET NEW.active_page_key = IF(
      NEW.active = 1,
      CONCAT(NEW.source_id, ':', NEW.page_id),
      NULL
  );

CREATE TRIGGER trg_kpr_active_page_key_bu
  BEFORE UPDATE ON knowledge_publish_records
  FOR EACH ROW
  SET NEW.active_page_key = IF(
      NEW.active = 1,
      CONCAT(NEW.source_id, ':', NEW.page_id),
      NULL
  );

-- ============================================================================
-- 36. knowledge_chunk_manifests — ingestion chunk 映射
-- ============================================================================
CREATE TABLE knowledge_chunk_manifests
(
    id                BIGINT UNSIGNED PRIMARY KEY AUTO_INCREMENT COMMENT 'Manifest ID',
    publish_record_id BIGINT UNSIGNED NOT NULL COMMENT '关联发布记录ID',
    run_id            BIGINT UNSIGNED NOT NULL COMMENT '关联ingestion run ID',
    document_id       BIGINT UNSIGNED NOT NULL COMMENT '关联knowledge_documents.document_id',
    chunk_id          BIGINT UNSIGNED NOT NULL COMMENT '关联knowledge_chunks.chunk_id',
    version_key       CHAR(64)        NOT NULL COMMENT '版本键',
    chunk_hash        CHAR(64)        NOT NULL COMMENT 'chunk幂等哈希',
    chunk_type        VARCHAR(32)     NOT NULL DEFAULT 'atomic'
                      COMMENT 'document_summary/section_summary/atomic',
    chunk_index       INT UNSIGNED    NOT NULL COMMENT 'chunk在版本内的序号',
    active            TINYINT(1)      NOT NULL DEFAULT 0 COMMENT '是否活跃',
    created_at        DATETIME(6)     NOT NULL DEFAULT CURRENT_TIMESTAMP(6) COMMENT '创建时间',
    updated_at        DATETIME(6)     NOT NULL DEFAULT CURRENT_TIMESTAMP(6)
                      ON UPDATE CURRENT_TIMESTAMP(6) COMMENT '更新时间',
    UNIQUE KEY uk_chunk_manifests_version_hash (version_key, chunk_hash),
    UNIQUE KEY uk_chunk_manifests_chunk_id (chunk_id),
    INDEX idx_chunk_manifests_publish_record (publish_record_id),
    INDEX idx_chunk_manifests_run (run_id),
    INDEX idx_chunk_manifests_document (document_id),
    INDEX idx_chunk_manifests_active (active),
    FOREIGN KEY (publish_record_id) REFERENCES knowledge_publish_records (id) ON DELETE CASCADE,
    FOREIGN KEY (run_id) REFERENCES knowledge_ingestion_runs (id) ON DELETE CASCADE,
    FOREIGN KEY (document_id) REFERENCES knowledge_documents (document_id) ON DELETE CASCADE,
    FOREIGN KEY (chunk_id) REFERENCES knowledge_chunks (chunk_id) ON DELETE CASCADE
) ENGINE = InnoDB
  DEFAULT CHARSET = utf8mb4
  COLLATE = utf8mb4_unicode_ci COMMENT = 'Web Ingestion Chunk 映射表';

-- ============================================================================
-- 37. knowledge_vector_manifests — ingestion 向量映射
-- ============================================================================
CREATE TABLE knowledge_vector_manifests
(
    id                  BIGINT UNSIGNED PRIMARY KEY AUTO_INCREMENT COMMENT 'Vector Manifest ID',
    publish_record_id   BIGINT UNSIGNED NOT NULL COMMENT '关联发布记录ID',
    run_id              BIGINT UNSIGNED NOT NULL COMMENT '关联ingestion run ID',
    document_id         BIGINT UNSIGNED NOT NULL COMMENT '关联knowledge_documents.document_id',
    chunk_id            BIGINT UNSIGNED NOT NULL COMMENT '关联knowledge_chunks.chunk_id',
    chunk_hash          CHAR(64)        NOT NULL COMMENT '关联的chunk_hash',
    vector_index_name   VARCHAR(128)    NOT NULL COMMENT '向量索引名称',
    vector_point_id     CHAR(64)        NOT NULL COMMENT '确定性向量 point ID',
    embedding_provider  VARCHAR(64)     NOT NULL COMMENT 'embedding provider',
    embedding_model     VARCHAR(128)    NOT NULL COMMENT 'embedding模型名',
    embedding_dimension INT UNSIGNED    NOT NULL COMMENT 'embedding维度',
    active              TINYINT(1)      NOT NULL DEFAULT 0 COMMENT '是否活跃',
    created_at          DATETIME(6)     NOT NULL DEFAULT CURRENT_TIMESTAMP(6) COMMENT '创建时间',
    updated_at          DATETIME(6)     NOT NULL DEFAULT CURRENT_TIMESTAMP(6)
                        ON UPDATE CURRENT_TIMESTAMP(6) COMMENT '更新时间',
    UNIQUE KEY uk_vector_manifests_vector_point (vector_index_name, vector_point_id),
    UNIQUE KEY uk_vector_manifests_chunk_model (chunk_id, embedding_model),
    INDEX idx_vector_manifests_publish_record (publish_record_id),
    INDEX idx_vector_manifests_run (run_id),
    INDEX idx_vector_manifests_document (document_id),
    INDEX idx_vector_manifests_active (active),
    FOREIGN KEY (publish_record_id) REFERENCES knowledge_publish_records (id) ON DELETE CASCADE,
    FOREIGN KEY (run_id) REFERENCES knowledge_ingestion_runs (id) ON DELETE CASCADE,
    FOREIGN KEY (document_id) REFERENCES knowledge_documents (document_id) ON DELETE CASCADE,
    FOREIGN KEY (chunk_id) REFERENCES knowledge_chunks (chunk_id) ON DELETE CASCADE
) ENGINE = InnoDB
  DEFAULT CHARSET = utf8mb4
  COLLATE = utf8mb4_unicode_ci COMMENT = 'Web Ingestion 向量映射表';

-- ============================================================================
-- 38. domain_event_outbox — 持久化核心流程事件
-- ============================================================================
CREATE TABLE domain_event_outbox
(
    id             BIGINT UNSIGNED PRIMARY KEY AUTO_INCREMENT COMMENT '事件ID',
    event_key      CHAR(64)        NOT NULL COMMENT '确定性幂等事件键',
    event_type     VARCHAR(128)    NOT NULL COMMENT '领域事件类型',
    aggregate_type VARCHAR(64)     NOT NULL COMMENT '聚合类型',
    aggregate_id   BIGINT UNSIGNED NOT NULL COMMENT '聚合根ID',
    payload        JSON            NOT NULL COMMENT '仅存ID和小型元数据，禁止全文/向量',
    status         VARCHAR(32)     NOT NULL DEFAULT 'pending'
                   COMMENT 'pending/processing/published/failed/dead',
    retry_count    INT UNSIGNED    NOT NULL DEFAULT 0 COMMENT '已重试次数',
    max_retries    INT UNSIGNED    NOT NULL DEFAULT 5 COMMENT '最大重试次数',
    next_retry_at  DATETIME(6)     NULL COMMENT '下次重试时间',
    locked_by      VARCHAR(128)    NULL COMMENT '锁定者标识',
    locked_until   DATETIME(6)     NULL COMMENT '锁过期时间',
    last_error     TEXT            NULL COMMENT '最近一次错误信息',
    created_at     DATETIME(6)     NOT NULL DEFAULT CURRENT_TIMESTAMP(6) COMMENT '创建时间',
    updated_at     DATETIME(6)     NOT NULL DEFAULT CURRENT_TIMESTAMP(6)
                   ON UPDATE CURRENT_TIMESTAMP(6) COMMENT '更新时间',
    published_at   DATETIME(6)     NULL COMMENT '成功处理时间',
    UNIQUE KEY uk_outbox_event_key (event_key),
    INDEX idx_outbox_claim (status, next_retry_at, created_at),
    INDEX idx_outbox_locked_by (locked_by),
    INDEX idx_outbox_aggregate (aggregate_type, aggregate_id),
    INDEX idx_outbox_event_type (event_type),
    INDEX idx_outbox_created (created_at)
) ENGINE = InnoDB
  DEFAULT CHARSET = utf8mb4
  COLLATE = utf8mb4_unicode_ci COMMENT = '领域事件发件箱表';

-- ============================================================================
-- 39. web_ingestion_audit_logs — Web ingestion 审计日志
-- ============================================================================
CREATE TABLE web_ingestion_audit_logs
(
    id                BIGINT UNSIGNED PRIMARY KEY AUTO_INCREMENT COMMENT '审计日志ID',
    source_id         BIGINT UNSIGNED NULL COMMENT '关联来源ID',
    source_url_id     BIGINT UNSIGNED NULL COMMENT '关联URL记录ID',
    page_id           BIGINT UNSIGNED NULL COMMENT '关联网页实体ID',
    run_id            BIGINT UNSIGNED NULL COMMENT '关联ingestion run ID',
    publish_record_id BIGINT UNSIGNED NULL COMMENT '关联发布记录ID',
    action            VARCHAR(64)     NOT NULL COMMENT '操作类型',
    status            VARCHAR(32)     NOT NULL DEFAULT 'info'
                      COMMENT 'info/warning/error/success',
    message           TEXT            NOT NULL COMMENT '日志消息',
    metadata          JSON            NULL COMMENT '附加元数据',
    created_at        DATETIME(6)     NOT NULL DEFAULT CURRENT_TIMESTAMP(6) COMMENT '创建时间',
    INDEX idx_audit_source (source_id),
    INDEX idx_audit_source_url (source_url_id),
    INDEX idx_audit_page (page_id),
    INDEX idx_audit_run (run_id),
    INDEX idx_audit_publish_record (publish_record_id),
    INDEX idx_audit_action (action),
    INDEX idx_audit_created (created_at),
    FOREIGN KEY (source_id) REFERENCES web_sources (id) ON DELETE SET NULL,
    FOREIGN KEY (source_url_id) REFERENCES web_source_urls (id) ON DELETE SET NULL,
    FOREIGN KEY (page_id) REFERENCES web_pages (id) ON DELETE SET NULL,
    FOREIGN KEY (run_id) REFERENCES knowledge_ingestion_runs (id) ON DELETE SET NULL,
    FOREIGN KEY (publish_record_id) REFERENCES knowledge_publish_records (id) ON DELETE SET NULL
) ENGINE = InnoDB
  DEFAULT CHARSET = utf8mb4
  COLLATE = utf8mb4_unicode_ci COMMENT = 'Web Ingestion 审计日志表';

-- ============================================================================
-- 40. user_memory_evidence — 记忆证据关系表（新增）
-- ============================================================================
CREATE TABLE user_memory_evidence
(
    evidence_id         BIGINT UNSIGNED PRIMARY KEY AUTO_INCREMENT,
    memory_id           BIGINT UNSIGNED NOT NULL,

    source_type         VARCHAR(32)     NOT NULL
        COMMENT 'message|summary|manual',
    source_ref_id       BIGINT UNSIGNED NOT NULL
        COMMENT '原始来源 ID；即使 FK 清空也保留，用于审计和去重',

    message_id          BIGINT UNSIGNED NULL,
    summary_id          BIGINT UNSIGNED NULL,
    source_deleted      TINYINT(1)      NOT NULL DEFAULT 0,

    evidence_type       VARCHAR(32)     NOT NULL
        COMMENT 'source|reinforcement|contradiction|manual',

    confidence          DECIMAL(4,3)    NULL,
    extractor_version   VARCHAR(64)     NULL,

    created_at          DATETIME(6)     NOT NULL DEFAULT CURRENT_TIMESTAMP(6),

    UNIQUE KEY uk_memory_source_type (
        memory_id,
        source_type,
        source_ref_id,
        evidence_type
    ),

    INDEX idx_memory_id (memory_id),
    INDEX idx_message_id (message_id),
    INDEX idx_summary_id (summary_id),

    FOREIGN KEY (memory_id) REFERENCES user_memories(memory_id)
        ON DELETE CASCADE,
    FOREIGN KEY (message_id) REFERENCES conversation_messages(id)
        ON DELETE SET NULL,
    FOREIGN KEY (summary_id) REFERENCES conversation_summaries(summary_id)
        ON DELETE SET NULL
) ENGINE = InnoDB
  DEFAULT CHARSET = utf8mb4
  COLLATE = utf8mb4_unicode_ci
  COMMENT = '记忆证据关系表（全链路审计）';

-- ============================================================================
-- 41. user_persona_snapshots — 用户画像派生快照表（新增）
-- ============================================================================
CREATE TABLE user_persona_snapshots
(
    snapshot_id         BIGINT UNSIGNED PRIMARY KEY AUTO_INCREMENT,
    user_id             BIGINT UNSIGNED NOT NULL,

    status              VARCHAR(32)     NOT NULL DEFAULT 'active'
        COMMENT 'active|superseded|expired|error',

    -- MySQL 8: 用 active_marker 保证每个用户最多一条 active。
    -- 不让生成列依赖 user_id（user_id 同时参与 ON DELETE CASCADE 外键），
    -- 避免 InnoDB 在生成列 + 唯一索引 + 外键组合下报 ERROR 1215。
    active_marker       TINYINT
        GENERATED ALWAYS AS (
            CASE WHEN status = 'active' THEN 1 ELSE NULL END
        ) STORED,

    snapshot_data       JSON            NOT NULL,

    source_memory_ids   JSON            NOT NULL,
    source_summary_ids  JSON            NULL,
    source_recent_message_ids JSON      NULL,

    input_hash          CHAR(64)        NOT NULL,

    model_name          VARCHAR(128)    NOT NULL,
    prompt_version      VARCHAR(64)     NOT NULL,
    schema_version      VARCHAR(64)     NOT NULL,
    generation_ms       INT UNSIGNED    NOT NULL,

    supersedes_id       BIGINT UNSIGNED NULL,
    error_message       TEXT            NULL,

    created_at          DATETIME(6)     NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    expires_at          DATETIME(6)     NULL,

    UNIQUE KEY uk_active_persona_user (user_id, active_marker),

    INDEX idx_user_status_created (user_id, status, created_at),
    INDEX idx_persona_supersedes_id (supersedes_id),
    INDEX idx_input_hash (input_hash),

    CONSTRAINT fk_user_persona_snapshots_user
        FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE
) ENGINE = InnoDB
  DEFAULT CHARSET = utf8mb4
  COLLATE = utf8mb4_unicode_ci
  COMMENT = '用户画像派生快照表（纯缓存，1:N，最多一条 active）';

-- ============================================================================
-- 42. user_context_versions — 用户上下文版本号（新增）
-- ============================================================================
CREATE TABLE user_context_versions
(
    user_id     BIGINT UNSIGNED PRIMARY KEY,
    version     BIGINT UNSIGNED NOT NULL DEFAULT 1,
    updated_at  DATETIME(6)     NOT NULL DEFAULT CURRENT_TIMESTAMP(6)
        ON UPDATE CURRENT_TIMESTAMP(6),

    CONSTRAINT fk_user_context_versions_user
        FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE
) ENGINE = InnoDB
  DEFAULT CHARSET = utf8mb4
  COLLATE = utf8mb4_unicode_ci
  COMMENT = '用户上下文版本号（bump on memory/persona/summary/control change）';

-- ============================================================================
-- 43. post_conversation_risk_audits — 对话关闭后置 Risk 审计表（新增）
-- ============================================================================
CREATE TABLE post_conversation_risk_audits
(
    audit_id            BIGINT UNSIGNED PRIMARY KEY AUTO_INCREMENT,
    user_id             BIGINT UNSIGNED NOT NULL,
    conversation_id     BIGINT UNSIGNED NOT NULL,

    audit_scope         VARCHAR(32)     NOT NULL
        COMMENT 'turn|recent_window|manual_recheck',

    user_message_ref_id BIGINT UNSIGNED NULL,
    assistant_message_ref_id BIGINT UNSIGNED NULL,

    user_message_id     BIGINT UNSIGNED NULL,
    assistant_message_id BIGINT UNSIGNED NULL,

    status              VARCHAR(32)     NOT NULL DEFAULT 'pending'
        COMMENT 'pending|running|completed|failed|discarded',

    risk_level          VARCHAR(32)     NULL
        COMMENT 'none|low|medium|high|crisis',
    risk_categories     JSON            NULL,
    confidence          DECIMAL(4,3)    NULL,

    input_hash          CHAR(64)        NULL,
    detector_name       VARCHAR(128)    NULL,
    detector_version    VARCHAR(64)     NULL,
    model_name          VARCHAR(128)    NULL,

    checked_at          DATETIME(6)     NULL,
    error_message       TEXT            NULL,
    metadata            JSON            NULL,

    source_deleted      TINYINT(1)      NOT NULL DEFAULT 0,

    created_at          DATETIME(6)     NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    updated_at          DATETIME(6)     NOT NULL DEFAULT CURRENT_TIMESTAMP(6)
        ON UPDATE CURRENT_TIMESTAMP(6),

    FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE,
    FOREIGN KEY (conversation_id) REFERENCES conversations(id) ON DELETE CASCADE,
    FOREIGN KEY (user_message_id) REFERENCES conversation_messages(id)
        ON DELETE SET NULL,
    FOREIGN KEY (assistant_message_id) REFERENCES conversation_messages(id)
        ON DELETE SET NULL,

    INDEX idx_user_status (user_id, status),
    INDEX idx_conv_created (conversation_id, created_at),
    INDEX idx_status (status),
    INDEX idx_risk_level (risk_level),
    INDEX idx_source_deleted (source_deleted)
) ENGINE = InnoDB
  DEFAULT CHARSET = utf8mb4
  COLLATE = utf8mb4_unicode_ci
  COMMENT = '对话关闭后置 Risk 审计表（独立于对话生成链路）';

-- ============================================================================
-- 44. ALTER TABLE statements for self-referencing foreign keys
-- ============================================================================
ALTER TABLE conversation_summaries
    ADD FOREIGN KEY (supersedes_id) REFERENCES conversation_summaries(summary_id)
        ON DELETE SET NULL;

ALTER TABLE user_persona_snapshots
    ADD CONSTRAINT fk_user_persona_snapshots_supersedes
    FOREIGN KEY (supersedes_id) REFERENCES user_persona_snapshots(snapshot_id)
        ON DELETE SET NULL;

ALTER TABLE user_memories
    ADD FOREIGN KEY (superseded_by) REFERENCES user_memories(memory_id)
        ON DELETE SET NULL;
