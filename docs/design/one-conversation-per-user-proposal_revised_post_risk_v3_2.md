# 技术方案：单用户单长期 Conversation 架构重构 & 后置 Risk 审计

| 版本 | 日期 | 作者 | 说明 |
|---|---:|---|---|
| v3.2-revised | 2026-06-14 | Hughsean / ChatGPT | 基于旧版 `one-conversation-per-user-proposal` 修订；删除业务 SessionManager；移除对话链路 risk_detection；Risk 改为对话关闭后的后置审计 |

---

## 0. 修订摘要

本文件用于替代旧版 `one-conversation-per-user-proposal`。

相对旧版，本版做出以下强制修订：

1. **删除传统业务 SessionManager**
   - 不再以 `session_id` / `dialogue_id` 路由对话。
   - 不再维护长期内存 `SessionState.messages` / `SessionState.prompt`。
   - 前端只需要 Bearer Token，不需要管理 session。
   - 后端每轮请求通过 `auth_user.user_id` 找到唯一长期 Conversation。

2. **单用户单长期 Conversation**
   - `conversations.user_id` 必须唯一。
   - API 不提供多 conversation 列表、切换、归档、删除等产品语义。
   - 所有长期上下文来自 DB：messages、summaries、memories、persona snapshots。

3. **Risk 从对话主链路移除**
   - 对话生成前不做 risk pre-check。
   - 对话生成中不调用 RiskDetectionService。
   - PromptBuilder 不读取 risk / safety context。
   - AgentRuntime 不因 risk 结果替换、阻断或修改当前回复。
   - API 不返回 `risk_level`、`safety_triggered`、`session_closed` 等字段。

4. **Risk 改为对话关闭后的后置审计**
   - “对话关闭”指本轮 `/chat/messages` 请求完成，或流式响应结束，assistant 消息已落库，连接已关闭。
   - 后置 Worker 读取已落库 turn/window，写入 `post_conversation_risk_audits`。
   - 审计数据只供内部审计、质量分析、人工复核，不进入 Persona、Memory、Summary、PromptBuilder。

5. **Summary 只保留 general**
   - 不再有 `rolling_safety` / `milestone_safety`。
   - Summary 禁止记录 risk label、crisis signal、risk level、自伤风险判断等内容。

6. **Memory 类型收敛**
   - 只允许 `preference`、`fact`、`emotional_pattern`、`goal`。
   - 禁止 `profile`、`safety_note`、`risk_note`、诊断类标签。

7. **用户控制即时生效**
   - 通过 `user_context_versions` 让 PromptContext / runtime cache 感知变更。
   - `clear`、`forget`、`persona reset`、memory disable 等操作必须 bump context_version。

---

## 1. 背景与目标

### 1.1 当前问题

旧系统存在以下问题：

- 用户可创建多个 Conversation，长期记忆和摘要碎片化。
- 前端需要管理对话列表、session_id 或 dialogue_id，产品复杂度过高。
- 长期上下文依赖内存 Session，clear / forget / reset 后容易继续使用旧 prompt。
- `prompt`、`dialogue_id`、`session_id` 等内部字段可能从 API 泄露。
- Memory 类型混入 `profile` / `safety_note`，容易把推断、风险和事实混在一起。
- Summary 可能包含 safety-relevant context，进而污染 Persona。
- Risk/Safety 被放入当前对话链路，会影响实时回复，和“后置审计”新目标冲突。

### 1.2 目标

| 目标 | 说明 |
|---|---|
| 单用户单 Conversation | DB 层 `conversations.user_id` UNIQUE；API 不暴露多对话概念 |
| 删除 dialogue_id | API、DTO、handler、AgentContext、数据库写入路径均不再依赖 dialogue_id |
| 删除业务 SessionManager | 不再用内存 Session 保存长期上下文 |
| 请求态 ChatService | 每轮请求按 user_id 编排：加载 DB 上下文、调用 AgentRuntime、持久化消息 |
| 长期上下文可审计 | Summary 有准确 message range；Memory 有 evidence；Persona 有 input_hash |
| 用户可控 | 支持查看记忆、禁用记忆、清空 transcript、重置个性化、完全遗忘 |
| Risk 后置审计 | 只在 turn closed 后异步检查，不影响当前对话回复 |

### 1.3 非目标

- 多 Conversation 产品模型
- 群聊 / 多用户会话
- 心理诊断、临床评估、人格障碍判断
- Big Five、relationship_stage、亲密等级
- 把 Persona 当事实源
- 把 Risk/Safety 信号写入 Persona、Memory 或 general Summary
- 对话生成前 risk pre-check
- risk 结果实时阻断或替换当前回复
- 分布式多实例部署

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
2. `conversation_messages` 是原始记录，不混入模型派生结论。
3. `conversation_summaries` 是有损压缩，只做 general continuity，不记录 risk/safety 审计结论。
4. `user_memories` 是模型观察，不是事实源，必须有 evidence。
5. `user_persona_snapshots` 是行为策略缓存，可删除、可重建、可过期。
6. `post_conversation_risk_audits` 是内部后置审计，不进入对话生成、Persona、Memory、Summary。

### 2.2 新请求路径

```text
POST /api/v1/chat/messages
  -> Auth 提取 user_id
  -> ChatService::send_message(user_id, text, location)
  -> 获取 per-user mutex
  -> ConversationRepository::find_or_create_for_user(user_id)
  -> UserContextVersionRepository::get_or_create(user_id)
  -> PromptBuilder::build(user_id, conversation_id, version, location)
  -> load recent conversation_messages
  -> AgentRuntime::respond(...)
  -> persist user + assistant messages
  -> ConversationRepository::touch_and_incr(conversation_id, 2)
  -> 返回 reply 给客户端
  -> response closed 后发布 TurnClosedEvent
  -> 后置异步任务：
       - Memory extraction
       - Summary refresh
       - Persona scheduling trigger
       - PostConversationRiskAuditWorker
       - Vector index / delete jobs
```

### 2.3 Risk 新边界

对话中不做：

```text
ChatService 不调用 RiskDetectionService
AgentRuntime::respond 前不做 risk pre-check
PromptBuilder 不读取 user_safety_context / risk_detection_results
LLM tool loop 不调用 risk tool
不基于 risk 结果改变当前回复
API 不返回 risk_level / safety_triggered / session_closed
```

对话关闭后才做：

```text
TurnClosedEvent / ResponseStreamClosedEvent 触发后置审计
审计输入来自已落库消息
审计结果写入 post_conversation_risk_audits
审计结果不 bump context_version
审计结果不进入下一轮 PromptContext
```

---

## 3. 数据库设计

### 3.0 设计约定

- 所有时间列统一使用 `DATETIME(6)`，按 UTC 存储。
- 开发阶段允许 DROP + CREATE。
- 所有外键必须明确 ON DELETE 行为。
- 所有外键约束名必须全库唯一，避免 MySQL `ERROR 1826 Duplicate foreign key constraint name`。
- 所有 `users.id` 相关外键必须与 `users.id` 类型完全一致，通常为 `BIGINT UNSIGNED`。
- 所有表使用 InnoDB。
- 不新增 `vector_cleanup_jobs`，向量删除复用 `vector_index_jobs(action='delete')`。
- 不再把 `risk_detection_results` 用作对话链路依赖。若旧表保留，只作为迁移兼容或历史数据表。

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
    CONSTRAINT fk_conversations_user
        FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci
  COMMENT='用户唯一长期对话表';
```

不变量：

```text
每个 user_id 最多一条 conversation。
重复 open/message 必须复用同一 conversation。
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

    CONSTRAINT fk_conversation_messages_conversation
        FOREIGN KEY (conversation_id) REFERENCES conversations(id) ON DELETE CASCADE,

    INDEX idx_conv_id (conversation_id, id),
    INDEX idx_conv_created (conversation_id, created_at)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci
  COMMENT='对话原始消息表';
```

---

### 3.3 `conversation_summaries`

只保留 general summary。Risk 不进入 Summary。

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

    CONSTRAINT fk_conversation_summaries_conversation
        FOREIGN KEY (conversation_id) REFERENCES conversations(id) ON DELETE CASCADE,
    CONSTRAINT fk_conversation_summaries_user
        FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE,
    CONSTRAINT fk_conversation_summaries_supersedes
        FOREIGN KEY (supersedes_id) REFERENCES conversation_summaries(summary_id)
        ON DELETE SET NULL,

    UNIQUE KEY uk_active_rolling_general (active_rolling_general_cid),
    INDEX idx_conv_status_end (conversation_id, status, message_end_id),
    INDEX idx_summary_vector_id (vector_id)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci
  COMMENT='对话 general 摘要表';
```

不变量：

| 类型 | 规则 |
|---|---|
| `rolling_general` | 每个 conversation 最多一条 active |
| `milestone_general` | 可多条 active |
| 所有 summary | `message_start_id` / `message_end_id` 必须等于实际摘要窗口 |
| 所有 summary | 禁止 risk label、crisis signal、risk level、自伤风险判断、诊断判断 |

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

    CONSTRAINT fk_user_memories_user
        FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE,
    CONSTRAINT fk_user_memories_source_conversation
        FOREIGN KEY (source_conversation_id) REFERENCES conversations(id)
        ON DELETE SET NULL,
    CONSTRAINT fk_user_memories_source_message
        FOREIGN KEY (source_message_id) REFERENCES conversation_messages(id)
        ON DELETE SET NULL,
    CONSTRAINT fk_user_memories_superseded_by
        FOREIGN KEY (superseded_by) REFERENCES user_memories(memory_id)
        ON DELETE SET NULL,

    UNIQUE KEY uk_user_memory_key (user_id, memory_key),
    UNIQUE KEY uk_memory_vector_id (vector_id),
    INDEX idx_user_status_salience (user_id, status, salience DESC),
    FULLTEXT INDEX ft_memory_content (content)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci
  COMMENT='用户长期记忆表';
```

允许类型：

```text
preference
fact
emotional_pattern
goal
```

禁止类型：

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

使用稳定 source 字段，避免 transcript clear 后 FK 置 NULL 导致唯一约束失效。

```sql
CREATE TABLE user_memory_evidence (
    evidence_id         BIGINT UNSIGNED PRIMARY KEY AUTO_INCREMENT,
    memory_id           BIGINT UNSIGNED NOT NULL,

    source_type         VARCHAR(32)     NOT NULL COMMENT 'message|summary|manual',
    source_ref_id       BIGINT UNSIGNED NOT NULL COMMENT '原始来源 ID；即使 FK 清空也保留',

    message_id          BIGINT UNSIGNED NULL,
    summary_id          BIGINT UNSIGNED NULL,
    source_deleted      TINYINT(1)      NOT NULL DEFAULT 0,

    evidence_type       VARCHAR(32)     NOT NULL
        COMMENT 'source|reinforcement|contradiction|manual',

    confidence          DECIMAL(4,3)    NULL,
    extractor_version   VARCHAR(64)     NULL,

    created_at          DATETIME(6)     NOT NULL DEFAULT CURRENT_TIMESTAMP(6),

    CONSTRAINT fk_user_memory_evidence_memory
        FOREIGN KEY (memory_id) REFERENCES user_memories(memory_id)
        ON DELETE CASCADE,
    CONSTRAINT fk_user_memory_evidence_message
        FOREIGN KEY (message_id) REFERENCES conversation_messages(id)
        ON DELETE SET NULL,
    CONSTRAINT fk_user_memory_evidence_summary
        FOREIGN KEY (summary_id) REFERENCES conversation_summaries(summary_id)
        ON DELETE SET NULL,

    UNIQUE KEY uk_memory_source_type (
        memory_id,
        source_type,
        source_ref_id,
        evidence_type
    ),
    INDEX idx_memory_id (memory_id),
    INDEX idx_message_id (message_id),
    INDEX idx_summary_id (summary_id)
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

MySQL 8 推荐用 `(user_id, active_marker)` 保证“每个用户最多一条 active”，避免 generated column 直接复用外键列作为唯一键时出现兼容性问题。

```sql
CREATE TABLE user_persona_snapshots (
    snapshot_id         BIGINT UNSIGNED PRIMARY KEY AUTO_INCREMENT,
    user_id             BIGINT UNSIGNED NOT NULL,

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

    status              VARCHAR(32)     NOT NULL DEFAULT 'active'
        COMMENT 'active|superseded|expired|error',
    supersedes_id       BIGINT UNSIGNED NULL,
    error_message       TEXT            NULL,

    created_at          DATETIME(6)     NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    expires_at          DATETIME(6)     NULL,

    CONSTRAINT fk_user_persona_snapshots_user
        FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE,
    CONSTRAINT fk_user_persona_snapshots_supersedes
        FOREIGN KEY (supersedes_id) REFERENCES user_persona_snapshots(snapshot_id)
        ON DELETE SET NULL,

    UNIQUE KEY uk_active_persona_user (user_id, active_marker),
    INDEX idx_user_status_created (user_id, status, created_at DESC),
    INDEX idx_input_hash (input_hash)
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

    CONSTRAINT fk_user_context_versions_user
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

后置 Risk 审计表。该表不属于对话生成链路。

```sql
CREATE TABLE post_conversation_risk_audits (
    audit_id             BIGINT UNSIGNED PRIMARY KEY AUTO_INCREMENT,
    user_id              BIGINT UNSIGNED NOT NULL,
    conversation_id      BIGINT UNSIGNED NOT NULL,

    audit_scope          VARCHAR(32)     NOT NULL
        COMMENT 'turn|recent_window|manual_recheck',

    user_message_ref_id      BIGINT UNSIGNED NULL,
    assistant_message_ref_id BIGINT UNSIGNED NULL,

    user_message_id          BIGINT UNSIGNED NULL,
    assistant_message_id     BIGINT UNSIGNED NULL,

    status               VARCHAR(32)     NOT NULL DEFAULT 'pending'
        COMMENT 'pending|running|completed|failed|discarded',

    risk_level           VARCHAR(32)     NULL
        COMMENT 'none|low|medium|high|crisis',
    risk_categories      JSON            NULL,
    confidence           DECIMAL(4,3)    NULL,

    input_hash           CHAR(64)        NULL,
    detector_name        VARCHAR(128)    NULL,
    detector_version     VARCHAR(64)     NULL,
    model_name           VARCHAR(128)    NULL,

    checked_at           DATETIME(6)     NULL,
    error_message        TEXT            NULL,
    metadata             JSON            NULL,

    source_deleted       TINYINT(1)      NOT NULL DEFAULT 0,

    created_at           DATETIME(6)     NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    updated_at           DATETIME(6)     NOT NULL DEFAULT CURRENT_TIMESTAMP(6)
        ON UPDATE CURRENT_TIMESTAMP(6),

    CONSTRAINT fk_post_risk_audits_user
        FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE,
    CONSTRAINT fk_post_risk_audits_conversation
        FOREIGN KEY (conversation_id) REFERENCES conversations(id) ON DELETE CASCADE,
    CONSTRAINT fk_post_risk_audits_user_message
        FOREIGN KEY (user_message_id) REFERENCES conversation_messages(id)
        ON DELETE SET NULL,
    CONSTRAINT fk_post_risk_audits_assistant_message
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

```text
ChatService 不读取 audit 结果。
AgentRuntime 不读取 audit 结果。
PromptBuilder 不读取 audit 结果。
PersonaEngine 不读取 audit 结果。
MemoryExtractor 不读取 audit 结果。
SummaryRefreshService 不读取 audit 结果。
Audit 写入不 bump context_version。
forget 必须删除该用户 audit。
```

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

删除或不暴露：

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
- 内部执行 `find_or_create_for_user(user_id)`。
- 不创建 Session。
- 不返回 `session_id`。
- 不构建 system prompt；第一条消息请求再构建 PromptContext。

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
timeout_seconds
risk_level
safety_triggered
```

---

### 4.4 `GET /api/v1/chat/history`

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

### 4.5 `GET /api/v1/chat/memories`

```http
GET /api/v1/chat/memories?type=preference,fact&limit=50
Authorization: Bearer <token>
```

Response:

```json
{
  "memories": [
    {
      "memory_id": 42,
      "memory_type": "preference",
      "content": "用户偏好直接、结构化的建议",
      "confidence": 0.85,
      "reinforce_count": 3,
      "created_at": "2026-05-10T10:00:00Z",
      "reinforced_at": "2026-06-12T10:00:00Z"
    }
  ],
  "total_active": 89
}
```

---

### 4.6 `GET /api/v1/chat/persona`

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

### 4.7 `POST /api/v1/chat/transcript/clear`

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

### 4.8 `POST /api/v1/chat/forget`

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

建议文件：

```text
src/application/session/chat_service.rs
```

短期可以仍放在 `session` module 下，但业务语义必须是 ChatService，不再是 SessionManager。

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

### 5.2 AgentRuntime

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

```text
组装 LLM messages
执行 LLM/tool loop
生成 assistant reply
持久化 user + assistant message，或返回给 ChatService 统一持久化
```

不再负责：

```text
session 管理
dialogue_id
risk pre-check
risk persist
summary refresh
memory extraction
prompt 构建
```

短期如果保留 AgentRuntime 持久化消息，ChatService 不得重复持久化。

### 5.3 PromptBuilder

建议文件：

```text
src/application/agent/prompt_builder.rs
```

读取：

```text
user_profiles
user_persona_snapshots WHERE status='active'
conversation_summaries WHERE status=1 AND summary_type IN ('rolling_general','milestone_general')
active user_memories if needed
```

不读取：

```text
post_conversation_risk_audits
risk_detection_results
user_safety_context
user_safety_observations
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

禁止出现：

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

事件：

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
user message 已落库
assistant message 已落库
HTTP response 已完成，或 SSE/stream 已关闭
不再可能影响当前回复
```

### 6.2 Worker

建议文件：

```text
src/application/risk/post_conversation_risk_audit_worker.rs
```

职责：

```text
接收 TurnClosedEvent
读取 user message + assistant message
可选读取最近 N 条消息作为窗口
运行 detector
写入 post_conversation_risk_audits
不 bump context_version
不写 Persona / Memory / Summary
```

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

### 6.3 隔离规则

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

### 6.4 与 clear / forget 的关系

`transcript/clear`：

```text
推荐删除该 conversation 下与消息关联的 audit。
如需保留审计留痕，必须清空 message FK，标记 source_deleted=1，并不得保留原文。
```

`forget`：

```text
必须删除该 user 的所有 post_conversation_risk_audits。
不允许从旧 audit 重建 SafetyContext。
不允许在后续 prompt 中引用历史 audit。
```

---

## 7. Repository 设计

### 7.1 ConversationRepository

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

### 7.2 UserContextVersionRepository

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

SummaryRefreshService 订阅 `TurnClosedEvent`，但不在 AgentRuntime 中直接 spawn。

流程：

```text
1. try_lock user lock；失败则 skip
2. 找 latest active rolling_general
3. start_id = latest.message_end_id + 1；若无 latest，start_id = first relevant message id
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

只允许：

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

必须过滤：

```sql
WHERE user_id = ?
  AND status = 1
  AND memory_type IN ('preference','fact','emotional_pattern','goal')
  AND created_at > COALESCE(:personalization_reset_at, '1970-01-01')
```

如果：

```text
personalization_enabled = 0
```

则返回空结果。

---

## 10. Persona Snapshot 设计

### 10.1 输入

Persona Engine 只读取：

```text
active user_memories + allowed types + after reset_at
conversation_summaries: rolling_general / milestone_general
recent conversation_messages: user / assistant
user_profiles
```

不读取：

```text
post_conversation_risk_audits
risk_detection_results
user_safety_context
user_safety_observations
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

## 11. 向量索引与删除

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

服务单实例运行，但 Tokio 内仍有同用户并发。

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
| PostConversationRiskAuditWorker | 不影响 prompt，可不锁；写 audit 时校验 message existence |

第一阶段建议同用户整轮串行，优先保证 correctness。

---

## 13. 异步任务护栏

Memory、Summary、Persona 任务应捕获 epoch：

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

```text
Memory extraction
Summary refresh
Persona generation
```

PostConversationRiskAuditWorker 例外：

```text
它不影响 prompt，不需要因为 context_version 改变而必然丢弃。
但如果 forget 已发生，应跳过或删除 audit。
如果消息已被 clear，应标记 discarded 或 source_deleted=1。
```

---

## 14. 实施计划

### Phase 0：确认当前代码

检查：

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
10. 删除或废弃旧 risk_detection 对话链路依赖

### Phase 2：Conversation Repository

实现：

```text
find_or_create_for_user
find_single_by_user_id
touch_and_incr
find_messages_before
find_messages_since
```

UPSERT 必须用 MySQL 原子语义。

### Phase 3：删除 SessionManager，新增 ChatService

1. 新增 `chat_service.rs`
2. 让新 `/chat/*` 路由完全绕过 `SessionManager`
3. 移除 `SessionState`、`SessionSnapshot`、`SessionStatus`
4. 删除 `session_id`、`dialogue_id`
5. 删除 timeout / cleanup task
6. 用 per-user mutex 串行化同用户请求

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

### Phase 5：PromptBuilder

1. 新增 `prompt_builder.rs`
2. `AgentRuntime` 不再内部构建 persona/system prompt
3. `ConversationOrchestrator::build_persona()` 删除或降级
4. PromptBuilder 不读取 post risk audit
5. PromptBuilder 只读 general summary、persona、memory、profile

### Phase 6：AgentRuntime 适配

1. 删除 `session_id` 参数
2. 删除 `risk pre-check`
3. 删除 `spawn_risk_persist`
4. 删除对 `RiskDetectionService` 的对话链路依赖
5. 保留 LLM/tool loop
6. 明确消息持久化边界

### Phase 7：PostConversationRiskAuditWorker

1. 新增 `post_conversation_risk_audits` repo
2. 新增 `TurnClosedEvent`
3. 在 response 完成后发布 event
4. Worker 异步读取已落库消息
5. 执行 detector
6. 写 audit
7. 不 bump context_version
8. 不影响当前或下一轮 prompt

### Phase 8：Memory / Summary / Persona

1. MemoryExtractor 类型白名单改为 4 类
2. MemoryService 实现三段式 merge + evidence
3. MemorySearchTool 加 personalization/status/type/reset_at 过滤
4. SummaryRefreshService 从 AgentRuntime 移出
5. Summary 改增量 cursor
6. PersonaEngine 排除 risk audit

### Phase 9：Vector 删除统一

1. VectorIndexRepository 增加 `enqueue_delete_job`
2. memory disable / forget 调用 delete job
3. summary disable / clear 调用 delete job
4. 不新增 VectorCleanupWorker

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

基础：

```bash
cargo check
cargo fmt --check
cargo test
```

Workspace：

```bash
cargo check --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

全局搜索：

```bash
grep -R "dialogue_id" -n src
grep -R "session_id" -n src
grep -R "prompt_preview\|prompt:" -n src/api src/application
grep -R "risk_detection\|RiskDetection\|user_safety_context" -n src/application src/api
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

---

## 16. 验收标准

### P0

- [ ] `conversations.user_id` 唯一
- [ ] `/api/v1/chat/messages` 不需要 session_id
- [ ] API 不暴露 prompt/dialogue_id/session_id/session_closed/risk_level
- [ ] `SessionManager` 不再是请求入口
- [ ] `AgentRuntime` 不接收 session_id
- [ ] `AgentRuntime` 不执行 risk pre-check
- [ ] 当前对话生成链路不读取 risk audit / risk context
- [ ] `TurnClosedEvent` 后才运行 Risk 审计
- [ ] Risk audit 不 bump context_version
- [ ] Persona / Memory / Summary 均不读取 Risk audit
- [ ] Memory 类型只有 4 类
- [ ] Summary 只有 general 类型
- [ ] forget 删除 post risk audit

### P1

- [ ] Summary 增量化
- [ ] Memory 三段式 merge
- [ ] Evidence 表事务写入
- [ ] Vector delete job 复用现有 `vector_index_jobs`
- [ ] async task epoch 护栏
- [ ] Persona Snapshot active 唯一
- [ ] PromptBuilder 单一入口

---

## 17. 最小首轮补丁建议

第一轮只做主链路闭环：

```text
1. ConversationRepository::find_or_create_for_user
2. touch_and_incr 原子 SQL
3. ChatService::open/send_message
4. /api/v1/chat/open 和 /api/v1/chat/messages
5. 绕开旧 SessionManager
6. 删除 DTO 响应中的 prompt/dialogue_id/session_id/session_closed
7. AgentRuntime 删除 risk pre-check 和 spawn_risk_persist
8. assistant 回复落库后发布 TurnClosedEvent
9. 新增最小 post_conversation_risk_audits pending/completed 写入
10. MemoryExtractor 白名单删掉 profile/safety_note
11. cargo check
```

这一轮通过后，再做 Summary、Persona、Memory merge、Vector cleanup 的完整化。

---

## 18. 执行红线

出现以下情况必须暂停修正：

```text
/chat/messages 仍需要 session_id
AgentRuntime 仍做 risk pre-check
PromptBuilder 读取 risk/safety context
MemoryExtractor 还能生成 safety_note/profile
API 仍返回 prompt/dialogue_id/risk_level
同一用户能创建多个 conversation
message_count 不是 SQL 原子自增
TurnClosedEvent 在 assistant 消息落库前发出
Risk audit 写入后 bump context_version
```
