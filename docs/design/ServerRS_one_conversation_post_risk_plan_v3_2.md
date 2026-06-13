# ServerRS 单用户单长期 Conversation 重构方案（含对话关闭后置 Risk 审计）

| 版本 | 日期 | 作者 | 说明 |
|---|---:|---|---|
| v3.2 | 2026-06-14 | Hughsean / ChatGPT | 合并主设计与实施计划；删除 SessionManager；移除对话链路中的 risk_detection；Risk 改为对话关闭后的后置审计 |

---

## 0. 本版关键决策

本文件是可直接作为实现依据的合并版文档，覆盖设计方案、数据库结构、核心流程、API、实施计划和验证方式。

本版相对上一版有两个关键变更：

1. **删除传统 SessionManager**  
   ServerRS 不再把 Session 当成业务核心对象。前端不再管理 `session_id`，后端不再维护长期内存 `SessionState`、`prompt`、`messages`、`dialogue_id`。  
   核心入口改为 `ChatService` / `ChatTurnService`：每轮请求按 `auth_user.user_id` 找到唯一 Conversation，从数据库构建 PromptContext，调用 AgentRuntime，持久化消息，并触发后置异步任务。

2. **移除对话业务中的 risk_detection**  
   对话生成链路中不再做 risk pre-check，不再在当前用户消息进入 LLM 前执行风险检测，不再因为 risk 结果改变当前轮回复，不再向 system prompt 注入 risk / safety context。  
   Risk 改为**对话关闭后的后置审计**：单轮请求或流式连接结束、assistant 回复落库以后，异步任务读取已落库的 turn/window 进行审计，审计结果只进入独立的内部 audit 表，不进入 Persona、Memory、Summary、PromptBuilder 或 ChatService 的当前回复路径。

> 本文中的“对话关闭”不是业务 Session 关闭。由于本架构没有 Session，关闭含义为：  
> **本轮 `/chat/messages` 请求已完成，或流式响应已结束，assistant 消息已经持久化，连接已经关闭。**

---

## 1. 目标与非目标

### 1.1 目标

| 目标 | 说明 |
|---|---|
| 单用户单 Conversation | `conversations.user_id` 使用 UNIQUE 约束；前端不暴露多对话概念 |
| 删除 dialogue_id | API、DTO、handler、SessionState、AgentContext 均不再出现 `dialogue_id` |
| 删除业务 SessionManager | 不再用内存 Session 容器保存长期上下文 |
| 请求态 ChatService | 每轮请求从 DB 构建上下文，调用 AgentRuntime，持久化消息 |
| 长期上下文可审计 | Summary、Memory、Persona 均保留来源范围、evidence 或 input_hash |
| 用户可控 | 支持查看、禁用、清空、重置、完全遗忘 |
| Risk 后置审计 | 只在 turn closed 后异步检查，不影响当前对话回复 |

### 1.2 非目标

- 多 Conversation 列表、切换、归档
- 群聊或多用户 Conversation
- 关系升级、亲密等级、relationship_stage
- Big Five、人格障碍、临床诊断式标签
- 把 Persona 当事实源
- 把 Risk / Safety 信号写入 Persona、Memory 或普通 Summary
- 在当前对话生成前执行 risk_detection
- 用 risk_detection 结果实时阻断、替换或修改当前轮回复
- 分布式多实例并发设计

---

## 2. 架构总览

### 2.1 数据分层

```text
Layer 0: 用户显式数据          user_profiles                 ← 事实权威
Layer 1: 原始对话记录          conversation_messages          ← 原始记录
Layer 2: 压缩派生              conversation_summaries         ← 有损摘要，仅 general
Layer 3: 模型观察              user_memories                  ← LLM 提取，有 evidence
Layer 4: 行为策略快照           user_persona_snapshots         ← 派生缓存，非事实源
Layer 5: 后置 Risk 审计         post_conversation_risk_audits  ← 内部审计，不进对话
```

核心原则：

1. 只有 `user_profiles` 是事实权威。
2. `conversation_messages` 是原始记录，不把派生结论混入原始记录。
3. `conversation_summaries` 只做 general summary，不记录 risk label、crisis signal、risk level 等安全审计信息。
4. `user_memories` 只允许 `preference`、`fact`、`emotional_pattern`、`goal` 四类。
5. `user_persona_snapshots` 是从 summary + memory 派生的行为策略缓存，可删除、可重建、可过期。
6. `post_conversation_risk_audits` 是独立内部审计数据，不参与 PromptBuilder、不进入 Persona Engine、不进入 MemoryExtractor、不 bump prompt context version。

### 2.2 请求路径

```text
POST /api/v1/chat/messages
  -> Auth 提取 user_id
  -> ChatService::send_message(user_id, text, location)
  -> per-user mutex
  -> ConversationRepository::find_or_create_for_user(user_id)
  -> PromptBuilder::build(user_id, conversation_id, context_version, location)
  -> load recent messages from DB
  -> AgentRuntime::respond(...)
  -> persist user + assistant messages
  -> atomic touch_and_incr
  -> return reply to client
  -> after response closed: emit TurnClosedEvent
  -> async post-conversation tasks:
       - Memory extraction
       - Summary refresh
       - Persona scheduling trigger
       - PostConversationRiskAuditWorker
       - Vector indexing / cleanup
```

### 2.3 Risk 的新边界

**对话中不做：**

- 不在 `ChatService` 中调用 RiskDetectionService
- 不在 `AgentRuntime::respond` 前做 risk pre-check
- 不在 prompt 中注入 `user_safety_context`
- 不在 LLM tool loop 中调用 risk tool
- 不基于 risk 结果改变当前回复
- 不返回 `risk_level`、`session_closed`、`safety_triggered` 等字段

**对话关闭后才做：**

- `TurnClosedEvent` 或 `ResponseStreamClosedEvent` 触发后置审计
- 审计输入来自已落库消息，不使用未持久化临时内容
- 审计结果写入 `post_conversation_risk_audits`
- 审计数据只用于内部监控、离线分析、质量评估或人工复核
- 默认不影响后续 PromptContext；如未来要影响，也必须另设明确产品开关并重新评审

---

## 3. 数据库设计

### 3.0 约定

- 所有时间列统一使用 `DATETIME(6)`，按 UTC 存储。
- 项目未上线，允许开发阶段使用 DROP + CREATE。
- 所有外键必须显式声明 ON DELETE 行为。
- 不新增 `vector_cleanup_jobs`，向量删除复用现有 `vector_index_jobs` 的 `action='delete'`。
- 删除旧 `risk_detection_results` 对话链路依赖。若保留旧表，仅作为迁移兼容，不应被 ChatService / AgentRuntime 读取。

---

### 3.1 `conversations`

```sql
CREATE TABLE conversations (
    id              BIGINT UNSIGNED PRIMARY KEY AUTO_INCREMENT,
    user_id         BIGINT UNSIGNED NOT NULL,
    title           VARCHAR(100)    NULL COMMENT '最近主题展示名，由 milestone summary 周期性更新',
    message_count   BIGINT UNSIGNED NOT NULL DEFAULT 0,
    last_message_at DATETIME(6)     NULL,
    created_at      DATETIME(6)     NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    updated_at      DATETIME(6)     NOT NULL DEFAULT CURRENT_TIMESTAMP(6)
        ON UPDATE CURRENT_TIMESTAMP(6),

    CONSTRAINT uk_conversations_user_id UNIQUE (user_id),
    FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci
  COMMENT='用户唯一长期对话表';
```

---

### 3.2 `conversation_messages`

```sql
CREATE TABLE conversation_messages (
    id              BIGINT UNSIGNED PRIMARY KEY AUTO_INCREMENT,
    conversation_id BIGINT UNSIGNED NOT NULL,
    sender_role     VARCHAR(32)     NOT NULL COMMENT 'user|assistant|system|plugin',
    sender_user_id  BIGINT UNSIGNED NULL,
    message_type    VARCHAR(32)     NOT NULL DEFAULT 'text',
    content         JSON            NOT NULL,
    token_count     INT UNSIGNED    NULL,
    created_at      DATETIME(6)     NOT NULL DEFAULT CURRENT_TIMESTAMP(6),

    FOREIGN KEY (conversation_id) REFERENCES conversations(id) ON DELETE CASCADE,

    INDEX idx_conv_id (conversation_id, id),
    INDEX idx_conv_created (conversation_id, created_at)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci
  COMMENT='对话原始消息表';
```

---

### 3.3 `conversation_summaries`

只保留 general summary 类型。Risk 不进入 Summary。

```sql
CREATE TABLE conversation_summaries (
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

    active_rolling_general_cid BIGINT UNSIGNED
        GENERATED ALWAYS AS (
            CASE WHEN status = 1 AND summary_type = 'rolling_general'
                 THEN conversation_id ELSE NULL END
        ) STORED,

    created_at          DATETIME(6)     NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    updated_at          DATETIME(6)     NOT NULL DEFAULT CURRENT_TIMESTAMP(6)
        ON UPDATE CURRENT_TIMESTAMP(6),

    FOREIGN KEY (conversation_id) REFERENCES conversations(id) ON DELETE CASCADE,
    FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE,
    FOREIGN KEY (supersedes_id) REFERENCES conversation_summaries(summary_id)
        ON DELETE SET NULL,

    UNIQUE KEY uk_active_rolling_general (active_rolling_general_cid),
    INDEX idx_conv_status_end (conversation_id, status, message_end_id),
    INDEX idx_vector_id (vector_id)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci
  COMMENT='对话 general 摘要表';
```

不变量：

| 类型 | 规则 |
|---|---|
| `rolling_general` | 每个 conversation 最多一条 active |
| `milestone_general` | 可多条 active |
| 所有 summary | `message_start_id` / `message_end_id` 必须等于实际摘要窗口 |
| 所有 summary | 禁止写入 risk label、crisis signal、risk level、自伤风险判断等内容 |

---

### 3.4 `user_memories`

```sql
CREATE TABLE user_memories (
    memory_id               BIGINT UNSIGNED PRIMARY KEY AUTO_INCREMENT,
    user_id                 BIGINT UNSIGNED NOT NULL,

    memory_key              CHAR(64)        NULL COMMENT 'SHA256(canonical_form)',
    canonical_form          TEXT            NULL COMMENT '规范化表述',

    memory_type             VARCHAR(64)     NOT NULL
        COMMENT 'preference|fact|emotional_pattern|goal',

    content                 TEXT            NOT NULL,

    source_confidence       DECIMAL(3,2)    NOT NULL DEFAULT 0.50,
    confidence              DOUBLE          NOT NULL DEFAULT 0.7,
    salience                DOUBLE          NOT NULL DEFAULT 0.5,

    source_conversation_id  BIGINT UNSIGNED NULL,
    source_message_id       BIGINT UNSIGNED NULL,

    reinforced_at           DATETIME(6)     NULL,
    reinforce_count         INT UNSIGNED    NOT NULL DEFAULT 0,

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
    FOREIGN KEY (superseded_by) REFERENCES user_memories(memory_id)
        ON DELETE SET NULL,

    UNIQUE INDEX uk_user_memory_key (user_id, memory_key),
    UNIQUE INDEX uk_memory_vector_id (vector_id),
    INDEX idx_user_status_salience (user_id, status, salience DESC),
    FULLTEXT INDEX ft_memory_content (content)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci
  COMMENT='用户长期记忆表';
```

Memory 类型白名单：

```text
preference
fact
emotional_pattern
goal
```

禁止：

```text
profile
safety_note
risk_note
diagnosis
clinical_label
relationship_stage
big_five
```

---

### 3.5 `user_memory_evidence`

此表用稳定 source 字段解决 transcript clear 后 FK 置 NULL 导致唯一约束失效的问题。

```sql
CREATE TABLE user_memory_evidence (
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
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci
  COMMENT='记忆证据关系表';
```

Transcript clear 时：

```sql
UPDATE user_memory_evidence
SET message_id = NULL,
    source_deleted = 1
WHERE message_id IN (...);
```

---

### 3.6 `user_persona_snapshots`

```sql
CREATE TABLE user_persona_snapshots (
    snapshot_id         BIGINT UNSIGNED PRIMARY KEY AUTO_INCREMENT,
    user_id             BIGINT UNSIGNED NOT NULL,

    active_user_id      BIGINT UNSIGNED
        GENERATED ALWAYS AS (
            CASE WHEN status = 'active' THEN user_id ELSE NULL END
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

    status              VARCHAR(32)     NOT NULL DEFAULT 'active'
        COMMENT 'active|superseded|expired|error',
    supersedes_id       BIGINT UNSIGNED NULL,
    error_message       TEXT            NULL,

    created_at          DATETIME(6)     NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    expires_at          DATETIME(6)     NULL,

    UNIQUE KEY uk_active_persona_user (active_user_id),
    INDEX idx_user_status_created (user_id, status, created_at DESC),
    INDEX idx_input_hash (input_hash),

    FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE,
    FOREIGN KEY (supersedes_id) REFERENCES user_persona_snapshots(snapshot_id)
        ON DELETE SET NULL
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci
  COMMENT='用户画像派生快照表';
```

---

### 3.7 `user_context_versions`

```sql
CREATE TABLE user_context_versions (
    user_id     BIGINT UNSIGNED PRIMARY KEY,
    version     BIGINT UNSIGNED NOT NULL DEFAULT 1,
    updated_at  DATETIME(6)     NOT NULL DEFAULT CURRENT_TIMESTAMP(6)
        ON UPDATE CURRENT_TIMESTAMP(6),

    FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci
  COMMENT='用户上下文版本号';
```

Bump 时机：

- memory disable / contradicted / superseded
- persona snapshot 新建、reset、rebuild、expired
- rolling summary 新增
- transcript clear
- forget
- personalization reset

不 bump：

- 普通 user/assistant message 落库
- 普通 memory 新增
- post-conversation risk audit 写入

---

### 3.8 `user_profiles` 扩展

```sql
ALTER TABLE user_profiles
    ADD COLUMN personalization_enabled TINYINT(1) NOT NULL DEFAULT 1
        COMMENT '个性化是否启用',
    ADD COLUMN personalization_reset_at DATETIME(6) NULL
        COMMENT '最近一次重置时间';
```

---

### 3.9 `post_conversation_risk_audits`

Risk 后置审计表。它不属于对话生成链路，不被 PromptBuilder、Persona Engine、MemoryExtractor、SummaryRefreshService 读取。

```sql
CREATE TABLE post_conversation_risk_audits (
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

    INDEX idx_status_created (status, created_at),
    INDEX idx_user_created (user_id, created_at DESC),
    INDEX idx_conv_created (conversation_id, created_at DESC),
    INDEX idx_risk_level (risk_level)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci
  COMMENT='对话关闭后的后置 Risk 审计表';
```

约束：

- ChatService 只负责创建 pending audit 或发布 TurnClosedEvent，不读取 audit 结果。
- Audit 结果不影响当前回复。
- Audit 结果默认不影响下一轮 PromptContext。
- `forget` 必须删除或 discard 用户相关 audit。
- `transcript/clear` 必须将 audit 的 message FK 置 NULL，并标记 `source_deleted=1`，或直接删除相关 audit。推荐开发阶段直接删除相关 audit，语义更清楚。

---

## 4. API 设计

### 4.1 端点清单

| 方法 | 路径 | 说明 |
|---|---|---|
| POST | `/api/v1/chat/open` | 确保唯一 Conversation 存在 / 预热 |
| POST | `/api/v1/chat/messages` | 发送消息，无 session_id |
| GET | `/api/v1/chat/history` | 分页获取消息历史 |
| GET | `/api/v1/chat/memories` | 查看系统记住的信息 |
| GET | `/api/v1/chat/persona` | 查看画像摘要计数 |
| POST | `/api/v1/chat/memory/{id}/disable` | 禁用单条记忆 |
| POST | `/api/v1/chat/persona/reset` | 重置个性化 |
| POST | `/api/v1/chat/persona/rebuild` | 重建画像 |
| POST | `/api/v1/chat/transcript/clear` | 清空 transcript，保留记忆 |
| POST | `/api/v1/chat/forget` | 完全遗忘 |

删除或不再暴露：

| 方法 | 路径 | 原因 |
|---|---|---|
| POST | `/api/v1/llm/sessions` | 无业务 Session |
| POST | `/api/v1/llm/sessions/{id}/messages` | 合并到 `/chat/messages` |
| GET | `/api/v1/llm/sessions/{id}` | 无 Session 状态 |
| GET | `/api/v1/users/{user_id}/conversations` | 单 Conversation，无列表 |
| GET | `/api/v1/users/{user_id}/conversations/{id}` | 不暴露多对话资源 |
| GET | `/api/v1/chat/risk-detections` | Risk 为内部后置审计，不属于用户对话 API |

---

### 4.2 `POST /api/v1/chat/open`

```http
POST /api/v1/chat/open
Authorization: Bearer <token>
Content-Type: application/json

{}
```

Response:

```json
{
  "conversation": {
    "id": 42,
    "message_count": 1523,
    "last_message_at": "2026-06-13T08:30:00Z"
  },
  "personalization_enabled": true
}
```

语义：

- 幂等。
- 内部调用 `find_or_create_for_user(user_id)`。
- 不创建 Session。
- 不返回 `session_id`。
- 不构建系统 prompt；第一条消息请求再构建 PromptContext。

---

### 4.3 `POST /api/v1/chat/messages`

Request:

```json
{
  "text": "我今天有点焦虑",
  "emotion": "anxious",
  "location": {
    "country": "JP",
    "city": "Tokyo"
  }
}
```

Response:

```json
{
  "reply": "...",
  "conversation_id": 42,
  "tool_calls": []
}
```

禁止字段：

```text
session_id
dialogue_id
prompt
prompt_preview
session_closed
risk_level
safety_triggered
```

---

### 4.4 `GET /api/v1/chat/history`

Query:

```http
GET /api/v1/chat/history?before_id=1000&limit=50
Authorization: Bearer <token>
```

Response:

```json
{
  "messages": [
    {
      "id": 1000,
      "sender_role": "assistant",
      "message_type": "text",
      "content": {
        "text": "..."
      },
      "created_at": "2026-06-13T08:30:00Z"
    }
  ],
  "next_before_id": 950
}
```

---

### 4.5 `GET /api/v1/chat/persona`

Response:

```json
{
  "has_active_persona": true,
  "generated_at": "2026-06-12T10:00:00Z",
  "snapshot_summary": {
    "communication_preferences_count": 3,
    "stable_facts_count": 5,
    "recurring_topics_count": 2,
    "goals_count": 1,
    "sensitive_context_count": 2
  },
  "personalization_enabled": true
}
```

只返回摘要计数，不返回完整内部画像。

---

### 4.6 `POST /api/v1/chat/transcript/clear`

语义：清空 transcript 和 summary，保留 memory 和 persona。

行为：

```text
1. 获取 user lock
2. 找到唯一 conversation
3. 删除 conversation_messages
4. 删除 conversation_summaries
5. conversations.message_count = 0, title = NULL, last_message_at = NULL
6. user_memory_evidence.message_id = NULL, source_deleted = 1
7. 删除或 discard post_conversation_risk_audits 中关联该 transcript 的记录
8. enqueue vector delete jobs for summaries
9. bump context_version
```

Response:

```json
{
  "cleared_messages": true,
  "cleared_summaries": true,
  "memories_preserved": true,
  "persona_preserved": true,
  "post_risk_audits_cleared": true
}
```

---

### 4.7 `POST /api/v1/chat/forget`

语义：完全遗忘该用户的长期上下文。

行为：

```text
1. 执行 transcript/clear
2. UPDATE user_memories SET status=0 WHERE user_id=?
3. active persona snapshot -> expired
4. DELETE FROM post_conversation_risk_audits WHERE user_id=?
5. UPDATE user_profiles
     SET personalization_enabled=0,
         personalization_reset_at=UTC_TIMESTAMP(6)
6. enqueue vector delete jobs for memories and summaries
7. bump context_version
```

Response:

```json
{
  "messages_cleared": true,
  "summaries_cleared": true,
  "memories_disabled": 47,
  "persona_expired": true,
  "post_risk_audits_deleted": true,
  "personalization_disabled": true
}
```

---

## 5. 核心服务设计

### 5.1 ChatService

文件：

```text
src/application/session/chat_service.rs
```

建议命名仍可放在 `session` module 下作为过渡，但业务语义应是 ChatService，不再是 SessionManager。

```rust
pub struct ChatService {
    agent_runtime: Arc<AgentRuntime>,
    conversation_repo: Arc<dyn ConversationRepository>,
    message_repo: Arc<dyn ConversationMessageRepository>,
    user_profile_repo: Arc<dyn UserProfileRepository>,
    prompt_builder: Arc<PromptBuilder>,
    context_version_repo: Arc<dyn UserContextVersionRepository>,
    event_bus: Arc<dyn TaskEventPublisher>,
    user_locks: DashMap<u64, Arc<tokio::sync::Mutex<()>>>,
}
```

核心方法：

```rust
pub async fn open(
    &self,
    user_id: u64,
) -> Result<ChatOpenResponse, AppError>;

pub async fn send_message(
    &self,
    user_id: u64,
    text: String,
    emotion: Option<String>,
    location: Option<serde_json::Value>,
) -> Result<ChatResponse, AppError>;
```

`send_message` 流程：

```rust
pub async fn send_message(...) -> Result<ChatResponse, AppError> {
    let lock = self.user_lock(user_id);
    let _guard = lock.lock().await;

    let conversation = self.conversation_repo
        .find_or_create_for_user(user_id)
        .await?;

    let context_version = self.context_version_repo
        .get_or_create(user_id)
        .await?;

    let prompt_context = self.prompt_builder
        .build(user_id, conversation.id, context_version.version, location)
        .await?;

    let recent_messages = self.message_repo
        .find_recent_for_conversation(conversation.id, 40)
        .await?;

    // 不调用 risk_detection
    // 不做 risk pre-check
    // 不注入 risk/safety prompt

    let result = self.agent_runtime
        .respond(user_id, conversation.id, prompt_context, recent_messages, text, emotion)
        .await?;

    self.conversation_repo
        .touch_and_incr(conversation.id, 2)
        .await?;

    self.event_bus
        .publish(TaskEvent::TurnClosed {
            user_id,
            conversation_id: conversation.id,
            user_message_id: result.user_message_id,
            assistant_message_id: result.assistant_message_id,
            closed_at: Utc::now(),
        })
        .await?;

    Ok(ChatResponse {
        reply: result.reply,
        conversation_id: conversation.id,
        tool_calls: result.tool_calls,
    })
}
```

---

### 5.2 AgentRuntime

AgentRuntime 不再接收 `session_id`。

旧签名：

```rust
respond(user_id, session_id, conversation_id, ...)
```

新签名：

```rust
pub async fn respond(
    &self,
    user_id: u64,
    conversation_id: u64,
    prompt_context: PromptContext,
    recent_messages: Vec<ConversationMessage>,
    text: String,
    emotion: Option<String>,
) -> Result<AgentTurnResult, AppError>;
```

职责：

- 组装 LLM messages
- 执行 LLM/tool loop
- 生成 assistant reply
- 持久化 user + assistant message，或返回给 ChatService 统一持久化

不再负责：

- session 管理
- dialogue_id
- risk pre-check
- risk persist
- summary refresh
- memory extraction
- prompt 构建

如短期内为了降低改动量仍由 AgentRuntime 持久化消息，也必须保证 ChatService 不重复持久化。

---

### 5.3 PromptBuilder

文件：

```text
src/application/agent/prompt_builder.rs
```

PromptBuilder 是系统 prompt 的唯一入口。

输入：

```rust
pub struct PromptBuildInput {
    pub user_id: u64,
    pub conversation_id: u64,
    pub context_version: u64,
    pub location: Option<serde_json::Value>,
}
```

读取：

```text
- user_profiles
- user_persona_snapshots WHERE status='active'
- conversation_summaries WHERE status=1 AND summary_type IN ('rolling_general','milestone_general')
- active user_memories if needed
```

不读取：

```text
- post_conversation_risk_audits
- risk_detection_results
- user_safety_context
- user_safety_observations
```

Prompt 分层：

```text
[系统角色]
你是一位有同理心的心理陪伴助手。

[可信上下文 — 用户显式提供]
- 用户基本资料: {user_profile}
- 当前位置（本请求透传，不持久化）: {location}

[非可信上下文 — 历史自动推断]
以下内容来自历史对话的自动提取和压缩，不是事实，不构成诊断。
当用户当前表达与历史推断冲突时，以用户当前表达为准。

[对话摘要]
{rolling_general_summaries}

[行为偏好]
{persona.communication_preferences}
{persona.support_preferences}
{persona.style_observations}

[长期观察]
{persona.stable_facts}
{persona.recurring_topics}
{persona.goals}

[谨慎话题]
{persona.sensitive_context}

[工具规则]
...
```

不得出现：

```text
risk_level
crisis
self_harm_label
safety_observation
user_safety_context
post_conversation_risk_audit
```

---

## 6. 后置 Risk 审计设计

### 6.1 触发时机

触发事件：

```rust
TaskEvent::TurnClosed {
    user_id,
    conversation_id,
    user_message_id,
    assistant_message_id,
    closed_at,
}
```

触发条件：

```text
1. user message 已落库
2. assistant message 已落库
3. HTTP response 已返回，或 SSE/stream 已关闭
4. 不再可能影响当前回复
```

### 6.2 Worker

文件：

```text
src/application/risk/post_conversation_risk_audit_worker.rs
```

职责：

1. 接收 `TurnClosedEvent`
2. 读取 user message + assistant message
3. 可选读取最近 N 条消息作为窗口
4. 运行 risk detector
5. 写入 `post_conversation_risk_audits`
6. 不 bump `context_version`
7. 不写 Persona / Memory / Summary

伪代码：

```rust
pub async fn handle_turn_closed(&self, event: TurnClosedEvent) -> Result<(), AppError> {
    let audit = self.audit_repo
        .create_pending(PostConversationRiskAuditNew {
            user_id: event.user_id,
            conversation_id: event.conversation_id,
            audit_scope: "turn",
            user_message_ref_id: Some(event.user_message_id),
            assistant_message_ref_id: Some(event.assistant_message_id),
            user_message_id: Some(event.user_message_id),
            assistant_message_id: Some(event.assistant_message_id),
        })
        .await?;

    let input = self.message_repo
        .load_turn(event.user_message_id, event.assistant_message_id)
        .await?;

    let input_hash = sha256(canonical_json(&input));

    let result = self.detector
        .detect_after_turn_closed(input)
        .await;

    match result {
        Ok(r) => {
            self.audit_repo.mark_completed(audit.audit_id, PostRiskAuditResult {
                risk_level: r.level,
                risk_categories: r.categories,
                confidence: r.confidence,
                input_hash,
                detector_name: r.detector_name,
                detector_version: r.detector_version,
                model_name: r.model_name,
                checked_at: Utc::now(),
            }).await?;
        }
        Err(e) => {
            self.audit_repo.mark_failed(audit.audit_id, e.to_string()).await?;
        }
    }

    Ok(())
}
```

### 6.3 严格隔离规则

后置 Risk 审计结果不得被以下模块读取：

```text
ChatService
AgentRuntime
PromptBuilder
MemoryExtractor
MemorySearchTool
PersonaEngine
SummaryRefreshService
ConversationOrchestrator
```

允许读取：

```text
PostConversationRiskAuditWorker
Internal audit dashboard
Offline quality analysis
Manual review tooling
```

### 6.4 与 forget / clear 的关系

`transcript/clear`：

- 推荐删除该 conversation 下与 message 关联的 audit。
- 如果需要审计留痕，则必须清空 message FK，标记 `source_deleted=1`，并不得保留原文。

`forget`：

- 必须删除该 user 的所有 `post_conversation_risk_audits`。
- 不允许从旧 audit 重建任何 SafetyContext。
- 不允许在后续 prompt 中引用历史 audit。

---

## 7. Repository 设计

### 7.1 ConversationRepository

Trait 新增：

```rust
#[async_trait]
pub trait ConversationRepository: Send + Sync {
    async fn find_or_create_for_user(&self, user_id: u64) -> Result<Conversation, AppError>;

    async fn find_single_by_user_id(&self, user_id: u64)
        -> Result<Option<Conversation>, AppError>;

    async fn touch_and_incr(&self, conversation_id: u64, inc: u64)
        -> Result<(), AppError>;

    async fn find_messages_before(
        &self,
        conversation_id: u64,
        before_id: Option<u64>,
        limit: u64,
    ) -> Result<Vec<ConversationMessage>, AppError>;

    async fn find_messages_since(
        &self,
        conversation_id: u64,
        since_id: u64,
    ) -> Result<Vec<ConversationMessage>, AppError>;
}
```

严格 UPSERT：

```sql
INSERT INTO conversations (user_id, message_count, created_at, updated_at)
VALUES (?, 0, UTC_TIMESTAMP(6), UTC_TIMESTAMP(6))
ON DUPLICATE KEY UPDATE id = LAST_INSERT_ID(id);

SELECT *
FROM conversations
WHERE id = LAST_INSERT_ID();
```

原子计数：

```sql
UPDATE conversations
SET message_count = message_count + ?,
    last_message_at = UTC_TIMESTAMP(6),
    updated_at = UTC_TIMESTAMP(6)
WHERE id = ?;
```

---

### 7.2 ContextVersionRepository

```rust
#[async_trait]
pub trait UserContextVersionRepository: Send + Sync {
    async fn get_or_create(&self, user_id: u64) -> Result<UserContextVersion, AppError>;
    async fn bump(&self, user_id: u64, reason: ContextVersionReason) -> Result<u64, AppError>;
}
```

Bump SQL：

```sql
INSERT INTO user_context_versions (user_id, version, updated_at)
VALUES (?, 2, UTC_TIMESTAMP(6))
ON DUPLICATE KEY UPDATE
    version = version + 1,
    updated_at = UTC_TIMESTAMP(6);
```

---

### 7.3 PostConversationRiskAuditRepository

```rust
#[async_trait]
pub trait PostConversationRiskAuditRepository: Send + Sync {
    async fn create_pending(
        &self,
        new_audit: NewPostConversationRiskAudit,
    ) -> Result<PostConversationRiskAudit, AppError>;

    async fn fetch_pending(
        &self,
        limit: u64,
    ) -> Result<Vec<PostConversationRiskAudit>, AppError>;

    async fn mark_running(&self, audit_id: u64) -> Result<(), AppError>;

    async fn mark_completed(
        &self,
        audit_id: u64,
        result: PostConversationRiskAuditResult,
    ) -> Result<(), AppError>;

    async fn mark_failed(
        &self,
        audit_id: u64,
        error_message: String,
    ) -> Result<(), AppError>;

    async fn delete_for_user(&self, user_id: u64) -> Result<u64, AppError>;

    async fn delete_for_conversation(&self, conversation_id: u64) -> Result<u64, AppError>;
}
```

---

## 8. Summary 设计

### 8.1 增量 rolling summary

SummaryRefreshService 不在 AgentRuntime 内部直接 spawn。它订阅 `TurnClosedEvent` 或 `TurnCompletedEvent`。

流程：

```text
1. try_lock user lock；失败则 skip
2. 找 latest active rolling_general
3. start_id = latest.message_end_id + 1；若无 latest，start_id = first message id
4. 加载 start_id 之后的 user/assistant 消息
5. 如果新增 turn 数 < threshold，跳过
6. 构建 input：旧 rolling_general + 新窗口
7. LLM 生成新的 rolling_general
8. 在事务中：
   - insert new summary
   - disable old rolling_general
   - set supersedes_id
   - bump context_version
9. enqueue summary vector index
```

### 8.2 Summary Prompt

```text
Summarize this conversation for future continuity.
Keep it concise and factual.

Include:
- user concerns
- stable preferences
- current goals
- unresolved topics
- useful context for continuity

Do NOT include:
- risk labels
- crisis signals
- safety judgments
- self-harm risk analysis
- clinical diagnosis
- personality disorder labels

If the conversation contains sensitive or safety-related material,
summarize only the ordinary conversational context needed for continuity,
without risk classification or safety labels.
```

---

## 9. Memory 设计

### 9.1 Extractor 白名单

Prompt 中只允许：

```text
preference
fact
emotional_pattern
goal
```

禁止：

```text
profile
safety_note
risk_note
diagnosis
clinical_label
```

### 9.2 三段式 merge

```text
新 memory
  -> canonicalize
  -> memory_key = SHA256(canonical_form)
  -> Stage 1: deterministic key 查重
  -> Stage 2: embedding top_k 召回候选
  -> Stage 3: LLM merge classifier
       same          -> 不写入
       related       -> 新 memory
       new_evidence  -> evidence reinforcement + reinforce_count
       contradiction -> 标旧 contradicted 或降低 confidence，新 memory 记录 supersedes
       new           -> 新 memory
```

Memory + Evidence 必须在同一 DB transaction 内完成。

### 9.3 MemorySearchTool

MemorySearchTool 必须过滤：

```sql
WHERE user_id = ?
  AND status = 1
  AND memory_type IN ('preference','fact','emotional_pattern','goal')
  AND created_at > COALESCE(:personalization_reset_at, '1970-01-01')
```

并且：

```text
if personalization_enabled = 0:
    return empty
```

---

## 10. Persona Snapshot 设计

### 10.1 输入

Persona Engine 只读取：

```text
- user_memories: active + allowed types + after reset_at
- conversation_summaries: rolling_general / milestone_general
- recent conversation_messages: user/assistant only
- user_profiles
```

不读取：

```text
- post_conversation_risk_audits
- risk_detection_results
- safety context
- safety observations
```

### 10.2 输出 Schema

```json
{
  "communication_preferences": [],
  "support_preferences": [],
  "style_observations": {
    "tone": "neutral",
    "directness": "medium",
    "structure": "step_by_step",
    "question_frequency": "low",
    "humor": "avoid_unless_user_initiates",
    "advice_style": "actionable",
    "confidence": 0.0,
    "evidence": {
      "memory_ids": [],
      "summary_ids": []
    }
  },
  "stable_facts": [],
  "recurring_topics": [],
  "goals": [],
  "sensitive_context": [],
  "interaction_context": {
    "known_history_available": false,
    "longitudinal_confidence": 0.0,
    "allowed_reference_style": "prefer_current_context_only"
  }
}
```

### 10.3 校验

- evidence memory_ids 必须属于 source_memory_ids
- evidence summary_ids 必须属于 source_summary_ids
- confidence >= 0.8 时至少两条独立 evidence 或 reinforce_count >= 2
- 禁止 DSM/ICD 术语
- 禁止人格障碍标签
- 禁止 risk/safety 标签
- 禁止重复 evidence id
- sensitive_context 必须有 last_observed_at

---

## 11. 向量清理

不新增 `vector_cleanup_jobs`。

复用现有 `vector_index_jobs`：

```text
action = 'delete'
object_type = 'memory' | 'summary' | 'rag_chunk'
object_id = ...
collection_name = ...
vector_id = ...
status = 'pending'
```

调用点：

| 操作 | 行为 |
|---|---|
| memory disable | enqueue delete job |
| summary disable | enqueue delete job |
| transcript clear | 批量 enqueue summary delete |
| forget | 批量 enqueue memory + summary delete |
| rag chunk delete | enqueue delete job |

---

## 12. 并发控制

服务单实例运行，但 Tokio 内仍有并发。

实现：

```rust
DashMap<u64, Arc<tokio::sync::Mutex<()>>>
```

前台 API：

| 操作 | 锁 |
|---|---|
| `/chat/messages` | lock |
| `/chat/transcript/clear` | lock |
| `/chat/forget` | lock |
| `/chat/persona/reset` | lock |
| `/chat/persona/rebuild` | lock |
| `/chat/memory/{id}/disable` | lock |

后台任务：

| 任务 | 锁 |
|---|---|
| SummaryRefreshService | try_lock，失败 skip |
| MemoryExtractor | try_lock 或 epoch 校验 |
| PersonaScheduler | try_lock，失败 skip |
| PostConversationRiskAuditWorker | 不影响 prompt，可不锁；写 audit 时使用 message existence 校验 |

---

## 13. 异步任务护栏

所有后置任务都应捕获 epoch：

```rust
let task_epoch = context_version_repo.get_or_create(user_id).await?.version;
let task_created_at = Utc::now();
```

写入前检查：

```rust
if context_version_repo.get_or_create(user_id).await?.version != task_epoch {
    return Ok(());
}

let profile = user_profile_repo.find_by_user_id(user_id).await?;
if profile.personalization_reset_at.is_some_and(|t| t >= task_created_at) {
    return Ok(());
}
```

适用：

- Memory extraction
- Summary refresh
- Persona generation

PostConversationRiskAuditWorker 例外：

- 它不影响 prompt，不需要因为 context_version 改变而必然丢弃。
- 但如果 `forget` 已发生，应跳过或删除 audit。
- 如果消息已被 clear，应标记 `discarded` 或 `source_deleted=1`。

---

## 14. 实施计划

### Phase 0：确认当前代码

检查路径：

```text
src/application/session/session_manager.rs
src/application/session/conversation_orchestrator.rs
src/domain/conversation/conversation_repository.rs
src/infrastructure/persistence/implementations/seaorm_conversation_repository.rs
src/application/agent/agent_runtime.rs
src/application/agent/tools/memory_search_tool.rs
src/application/memory/memory_extractor.rs
src/application/memory/memory_service.rs
src/application/summary/summary_service.rs
src/application/rag/vector_index_service.rs
src/api/handlers/session_handler.rs
src/api/dto/session_dto.rs
src/api/router.rs
src/main.rs
src/bootstrap/repos.rs
src/bootstrap/state.rs
database/sql/init.sql
migrations/
```

---

### Phase 1：DDL

1. `conversations.user_id UNIQUE`
2. `conversation_summaries` 改成 general-only 类型
3. 新增 active rolling generated unique
4. `user_memories` 移除 `profile/safety_note`
5. 新增 `user_memory_evidence`
6. 新增 `user_persona_snapshots`
7. 新增 `user_context_versions`
8. 扩展 `user_profiles`
9. 新增 `post_conversation_risk_audits`
10. 删除或废弃旧 risk_detection 对话链路表依赖

---

### Phase 2：Conversation Repository

实现：

```text
find_or_create_for_user
find_single_by_user_id
touch_and_incr
find_messages_before
find_messages_since
```

UPSERT 必须用 MySQL 原子语义，不使用 duplicate 字符串匹配作为核心逻辑。

---

### Phase 3：删除 SessionManager，新增 ChatService

1. 新增 `chat_service.rs`
2. 移除 `SessionManager` 作为业务入口
3. 删除 `SessionState`、`SessionSnapshot`、`SessionStatus`
4. 删除 `session_id`、`dialogue_id`
5. 删除 timeout / cleanup task
6. 用 per-user mutex 串行化同用户请求

---

### Phase 4：API 重构

新增：

```text
POST /api/v1/chat/open
POST /api/v1/chat/messages
GET  /api/v1/chat/history
GET  /api/v1/chat/memories
GET  /api/v1/chat/persona
POST /api/v1/chat/memory/{id}/disable
POST /api/v1/chat/persona/reset
POST /api/v1/chat/persona/rebuild
POST /api/v1/chat/transcript/clear
POST /api/v1/chat/forget
```

删除：

```text
/api/v1/llm/sessions/*
/api/v1/users/{user_id}/conversations/*
/api/v1/chat/risk-detections
```

---

### Phase 5：PromptBuilder

1. 新增 `prompt_builder.rs`
2. `AgentRuntime` 不再内部构建 persona/system prompt
3. `ConversationOrchestrator::build_persona()` 删除或降级
4. PromptBuilder 不读取 post risk audit
5. PromptBuilder 只读 general summary、persona、memory、profile

---

### Phase 6：AgentRuntime 适配

1. 删除 `session_id` 参数
2. 删除 `risk pre-check`
3. 删除 `spawn_risk_persist`
4. 删除对 `RiskDetectionService` 的对话链路依赖
5. 保持 LLM/tool loop
6. 明确消息持久化边界：要么 AgentRuntime 负责，要么 ChatService 负责，不得重复

---

### Phase 7：PostConversationRiskAuditWorker

1. 新增 `post_conversation_risk_audits` repo
2. 新增 `TurnClosedEvent`
3. 在 response 完成后发布 event
4. Worker 异步读取已落库消息
5. 执行 detector
6. 写 audit
7. 不 bump context_version
8. 不影响当前或下一轮 prompt

---

### Phase 8：Memory / Summary / Persona

1. MemoryExtractor 类型白名单改为 4 类
2. MemoryService 实现三段式 merge + evidence
3. MemorySearchTool 加 personalization/status/type/reset_at 过滤
4. SummaryRefreshService 从 AgentRuntime 移出
5. Summary 改增量 cursor
6. PersonaEngine 排除 risk audit

---

### Phase 9：Vector 删除统一

1. VectorIndexRepository 增加 `enqueue_delete_job`
2. memory disable / forget 调用 delete job
3. summary disable / clear 调用 delete job
4. 不新增 VectorCleanupWorker

---

### Phase 10：删除旧代码

删除或废弃：

```text
src/application/session/session_manager.rs
旧 session DTO
旧 session handler
旧 conversation list handler
RiskDetectionService 在 AgentRuntime 中的依赖
risk pre-check 分支
session lifecycle task
session cleanup task
dialogue_id 字段和路由参数
```

---

## 15. 验证命令

基础检查：

```bash
cargo check
cargo fmt --check
cargo test
```

如 workspace：

```bash
cargo check --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

数据库验证：

```sql
-- 同一用户只能有一个 conversation
INSERT INTO conversations (user_id) VALUES (1);
INSERT INTO conversations (user_id) VALUES (1)
ON DUPLICATE KEY UPDATE id = LAST_INSERT_ID(id);

-- active rolling_general 只能有一条
INSERT INTO conversation_summaries
(conversation_id, user_id, summary_type, content, message_start_id, message_end_id, status)
VALUES
(1, 1, 'rolling_general', 'a', 1, 10, 1);

INSERT INTO conversation_summaries
(conversation_id, user_id, summary_type, content, message_start_id, message_end_id, status)
VALUES
(1, 1, 'rolling_general', 'b', 11, 20, 1);
-- 应失败，除非旧 rolling_general 已 disabled
```

API smoke test：

```bash
curl -X POST http://localhost:8080/api/v1/chat/open \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{}'

curl -X POST http://localhost:8080/api/v1/chat/messages \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"text":"你好"}'
```

必须验证：

| 场景 | 预期 |
|---|---|
| 重复 `/chat/open` | 不创建第二个 conversation |
| `/chat/messages` 不传 session_id | 正常回复 |
| API 响应 | 不含 prompt、dialogue_id、session_id、session_closed、risk_level |
| 同一用户并发发消息 | per-user mutex 串行 |
| message_count | 原子递增，不丢计数 |
| transcript clear | 消息和 summary 清空，memory 保留 |
| forget | memory disabled，persona expired，risk audits deleted |
| MemoryExtractor | 不产生 profile / safety_note |
| Summary | range 与实际窗口一致 |
| PromptBuilder | 不读取 risk audit |
| AgentRuntime | 不做 risk pre-check |
| TurnClosed 后 | 异步生成 post_conversation_risk_audits |
| Risk audit 写入 | 不 bump context_version |

---

## 16. 验收标准

### P0 验收

- [ ] `conversations.user_id` 唯一
- [ ] `/api/v1/chat/messages` 不需要 session_id
- [ ] API 不暴露 prompt/dialogue_id/session_id/session_closed/risk_level
- [ ] `SessionManager` 不再是请求入口
- [ ] `AgentRuntime` 不接收 session_id
- [ ] `AgentRuntime` 不执行 risk pre-check
- [ ] 当前对话生成链路不读取任何 risk audit / risk context
- [ ] `TurnClosedEvent` 后才运行 Risk 审计
- [ ] Risk audit 不 bump context_version
- [ ] Persona / Memory / Summary 均不读取 Risk audit
- [ ] Memory 类型只有 4 类
- [ ] Summary 只有 general 类型
- [ ] forget 删除 post risk audit

### P1 验收

- [ ] Summary 增量化
- [ ] Memory 三段式 merge
- [ ] Evidence 表事务写入
- [ ] Vector delete job 复用现有 `vector_index_jobs`
- [ ] async task epoch 护栏
- [ ] Persona Snapshot active 唯一
- [ ] PromptBuilder 单一入口

---

## 17. 最小首轮补丁建议

第一轮不要一次实现完整 Persona/Memory/Summary。建议只做：

1. 新增 `ConversationRepository::find_or_create_for_user`
2. `touch_and_incr` 改为原子 SQL
3. 新增 `ChatService::open/send_message`
4. 新增 `/api/v1/chat/open` 和 `/api/v1/chat/messages`
5. 绕开旧 SessionManager
6. 删除 DTO 响应中的 `prompt/dialogue_id/session_id/session_closed`
7. AgentRuntime 删除 `risk pre-check` 和 `spawn_risk_persist`
8. 在 assistant 回复落库后发布 `TurnClosedEvent`
9. 新增最小 `post_conversation_risk_audits` pending/completed 写入
10. MemoryExtractor 白名单删掉 `profile/safety_note`
11. `cargo check`

这一轮通过后，再做 Summary、Persona、Memory merge、Vector cleanup 的完整化。
