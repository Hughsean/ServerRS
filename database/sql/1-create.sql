DROP DATABASE IF EXISTS digital_companion;
-- 创建数据库
CREATE DATABASE IF NOT EXISTS digital_companion DEFAULT CHARACTER
    SET utf8mb4 DEFAULT COLLATE utf8mb4_unicode_ci;

-- 使用数据库
USE digital_companion;

-- 创建用户基础信息表
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
    INDEX idx_username (username),
    INDEX idx_email (email),
    INDEX idx_phone (phone)
) ENGINE = InnoDB
  DEFAULT CHARSET = utf8mb4
  COLLATE = utf8mb4_unicode_ci COMMENT = '用户基础信息表';

-- 用户画像表
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

-- 创建用户日记表
CREATE TABLE user_diaries
(
    id               BIGINT UNSIGNED PRIMARY KEY AUTO_INCREMENT COMMENT '日记ID',
    user_id          BIGINT UNSIGNED NOT NULL COMMENT '用户ID',
    title            VARCHAR(100)    NOT NULL DEFAULT '无标题' COMMENT '日记标题',
    content          TEXT            NOT NULL COMMENT '日记内容',
    mood_description VARCHAR(255) COMMENT '心情描述，使用大模型评估',
    created_at       TIMESTAMP       NOT NULL DEFAULT CURRENT_TIMESTAMP COMMENT '创建时间',
    updated_at       TIMESTAMP       NOT NULL DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP COMMENT '更新时间',
    -- 外键关联用户表
    FOREIGN KEY (user_id) REFERENCES users (id) ON DELETE CASCADE,
    -- 索引设计
    INDEX idx_user_id (user_id),
    INDEX idx_created_at (created_at)
) ENGINE = InnoDB
  DEFAULT CHARSET = utf8mb4
  COLLATE = utf8mb4_unicode_ci
    COMMENT = '用户日记表';


-- 会话元数据表（不再存整段消息，仅存会话属性）
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
  COLLATE = utf8mb4_unicode_ci COMMENT ='会话元数据表';


-- 会话消息表（每条消息一行）
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


-- 用户交流社区帖子表
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


-- 用户交流社区媒体表
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


-- 用户交流社区评论表
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


-- 量表定义表 (存储不同抑郁量表的元数据)
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
  COLLATE = utf8mb4_unicode_ci
    COMMENT = '抑郁量表定义表';

-- 评估记录表 (存储每次评估的详细结果)
CREATE TABLE depression_assessments
(
    assessment_id   BIGINT UNSIGNED PRIMARY KEY AUTO_INCREMENT COMMENT '评估ID',
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
  COLLATE = utf8mb4_unicode_ci
    COMMENT = '抑郁评估记录表';

-- 风险检测结果表：存储对单条用户消息的风险与意图检测结果
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

-- 心理知识库分类表
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

-- 心理知识库文章表
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

-- 心理知识库问答表
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

-- 心理资源库表
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

-- 用户知识库收藏表
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

-- 音乐表
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
