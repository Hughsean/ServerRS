# 技术方案：单用户单会话架构重构 & 长期上下文引擎

| 版本 | 日期 | 作者 | 说明 |
|---|---|---|---|
| v3.0 | 2026-06-13 | Hughsean | P0 修正：active 唯一约束、safety_note 隔离、clear/forget 语义、context_version、evidence 关系表、向量清理、DDL 统一 |

---

## 1. 背景与目标

### 1.1 当前问题

- 用户可创建多个 Conversation，记忆和摘要碎片化
- 切换对话时上下文断裂
- 前端需要管理对话列表

### 1.2 目标

| 目标 | 说明 |
|---|---|
| 单用户单 Conversation | DB 层 `user_id` UNIQUE；API 不暴露多对话概念 |
| 长期上下文可用 | 摘要链 + 记忆网络 + 证据链路，支撑跨时间对话连续性 |
| 可审计的个性化 | 所有派生数据有 evidence、input_hash、可追溯 |
| 用户可控 | 用户可查看、禁用、清空、重置系统记住的信息 |

### 1.3 非目标

- 群聊 / 多用户对话
- 心理诊断或临床评估
- 分布式多实例部署

---

## 2. 数据分层（架构基础）

```
Layer 0: 用户显式数据         user_profile              ← 事实权威
Layer 1: 原始记录             conversation_messages      ← 不可篡改
Layer 2: 压缩派生             conversation_summaries     ← 有损，记录覆盖范围
Layer 3: 模型观察             user_memories              ← LLM 提取，有 confidence，可修正/矛盾
Layer 4: 行为策略快照          user_persona_snapshots     ← 从 L2+L3 派生，纯缓存，永不当事实源
Layer 5: 安全信号              user_safety_context        ← 从 risk 聚合，仅影响安全策略
                               user_safety_observations   ← 安全相关观察记录
```

**核心原则**：

1. 只有 Layer 0 具备事实权威。L1-L5 都是派生/观察数据，system prompt 中标注为"非可信上下文"。
2. Persona Snapshot 是纯缓存：可随时删除重建，不影响数据完整性。
3. Safety 信号不进入 Persona：`risk_detection_results` 和 `safety_observations` 不参与画像生成。
4. 用户控制操作必须即时生效：通过 `context_version` 机制使 active session 感知变更。

---

## 3. 数据库设计

### 3.0 设计约定

- **所有时间列统一使用 `DATETIME(6)`**，按 UTC 存储
- **开发阶段使用 DROP TABLE + CREATE TABLE**，不写 ALTER 迁移脚本
- **所有外键明确声明 ON DELETE 行为**

### 3.1 `conversations`

```sql
CREATE TABLE conversations (
    id              BIGINT UNSIGNED PRIMARY KEY AUTO_INCREMENT,
    user_id         BIGINT UNSIGNED NOT NULL,
    title           VARCHAR(100)    NULL
        COMMENT '最近主题展示名，由 summary milestone 周期性更新',
    message_count   BIGINT UNSIGNED NOT NULL DEFAULT 0,
    last_message_at DATETIME(6)     NULL,
    created_at      DATETIME(6)     NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    updated_at      DATETIME(6)     NOT NULL DEFAULT CURRENT_TIMESTAMP(6)
        ON UPDATE CURRENT_TIMESTAMP(6),

    CONSTRAINT uk_user_id UNIQUE (user_id),
    FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci
  COMMENT='用户对话主表（每用户仅一行）';
```

**说明**：`title` 不再由首轮 LLM 生成（单会话下无意义），改由 `milestone` 摘要周期性更新。不展示时用 NULL。

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
  COMMENT='对话消息表';
```

**索引说明**：
- `(conversation_id, id)` — 游标分页 `WHERE conversation_id=? AND id<? ORDER BY id DESC`
- `(conversation_id, created_at)` — 时间范围查询

### 3.3 `conversation_summaries`

```sql
CREATE TABLE conversation_summaries (
    summary_id          BIGINT UNSIGNED PRIMARY KEY AUTO_INCREMENT,
    conversation_id     BIGINT UNSIGNED NOT NULL,
    user_id             BIGINT UNSIGNED NOT NULL,

    summary_type        VARCHAR(32)     NOT NULL COMMENT 'rolling_general|rolling_safety|milestone_general|milestone_safety',
    content             TEXT            NOT NULL,

    -- 覆盖范围
    message_start_id    BIGINT UNSIGNED NOT NULL,
    message_end_id      BIGINT UNSIGNED NOT NULL,

    -- 被取代的旧摘要
    supersedes_id       BIGINT UNSIGNED NULL,

    token_count         INT UNSIGNED    NULL,

    -- 向量索引
    vector_id           VARCHAR(128)    NULL,
    embedding_provider  VARCHAR(64)     NULL,
    embedding_model     VARCHAR(128)    NULL,
    embedding_dimension INT UNSIGNED    NULL,
    indexed_at          DATETIME(6)     NULL,

    -- 状态控制
    status              TINYINT         NOT NULL DEFAULT 1 COMMENT '1=active 0=disabled',
    created_at          DATETIME(6)     NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    updated_at          DATETIME(6)     NOT NULL DEFAULT CURRENT_TIMESTAMP(6)
        ON UPDATE CURRENT_TIMESTAMP(6),

    FOREIGN KEY (conversation_id) REFERENCES conversations(id) ON DELETE CASCADE,
    FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE,
    FOREIGN KEY (supersedes_id) REFERENCES conversation_summaries(summary_id)
        ON DELETE SET NULL,

    INDEX idx_conv_status_end (conversation_id, status, message_end_id),
    INDEX idx_vector_id (vector_id)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci
  COMMENT='对话摘要表';
```

**不变量**：

| 类型 | 规则 |
|---|---|
| `rolling_general` | 每个 conversation 最多一条 active；新 rolling 产生后旧 rolling `status=0` 或 `superseded` |
| `milestone` | 可多条 active；不一定 supersede |

### 3.4 `user_memories`

**`safety_note` 类型已从此表移除**。安全相关观察存入独立的 `user_safety_observations` 表（§3.8）。

```sql
CREATE TABLE user_memories (
    memory_id               BIGINT UNSIGNED PRIMARY KEY AUTO_INCREMENT,
    user_id                 BIGINT UNSIGNED NOT NULL,

    -- 去重与关联
    memory_key              CHAR(64)        NULL COMMENT 'SHA256(canonical_form)',
    canonical_form          TEXT            NULL COMMENT '规范化表述（去重用）',

    memory_type             VARCHAR(64)     NOT NULL
        COMMENT 'preference|fact|emotional_pattern|goal',

    content                 TEXT            NOT NULL,

    -- 置信度
    source_confidence       DECIMAL(3,2)    NOT NULL DEFAULT 0.50
        COMMENT 'LLM 提取时的原始置信度',
    confidence              DOUBLE          NOT NULL DEFAULT 0.7
        COMMENT '当前综合置信度（由 evidence 更新）',
    salience                DOUBLE          NOT NULL DEFAULT 0.5
        COMMENT '重要性 0-1',

    -- 来源
    source_conversation_id  BIGINT UNSIGNED NULL,
    source_message_id       BIGINT UNSIGNED NULL,

    -- 强化
    reinforced_at           DATETIME(6)     NULL
        COMMENT '最近一次被独立新证据加强',
    reinforce_count         INT UNSIGNED    NOT NULL DEFAULT 0
        COMMENT '被独立证据加强的次数',

    -- 矛盾
    contradicted_at         DATETIME(6)     NULL,
    superseded_by           BIGINT UNSIGNED NULL,

    -- 状态：1=active 0=disabled -1=contradicted
    status                  TINYINT         NOT NULL DEFAULT 1,

    -- 元数据
    canonicalizer_version   VARCHAR(64)     NULL,
    merge_decision          VARCHAR(32)     NULL
        COMMENT 'same|related|new_evidence|contradiction',
    merge_reason            TEXT            NULL,

    metadata                JSON            NULL,
    last_accessed_at        DATETIME(6)     NULL,
    access_count            INT UNSIGNED    NOT NULL DEFAULT 0,

    -- 向量索引
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

    UNIQUE INDEX idx_vector_id (vector_id),
    UNIQUE INDEX idx_user_key (user_id, memory_key),
    INDEX idx_user_status_salience (user_id, status, salience DESC),
    FULLTEXT INDEX ft_content (content)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci
  COMMENT='用户长期记忆表';
```

**Persona Engine 查询时必须排除疑似安全内容**（belt-and-suspenders 防御）：

```sql
WHERE user_id = ?
  AND status = 1
  AND memory_type IN ('preference', 'fact', 'emotional_pattern', 'goal')
ORDER BY salience DESC, confidence DESC
LIMIT ?
```

### 3.5 `user_memory_evidence` — 证据关系表（新增）

```sql
CREATE TABLE user_memory_evidence (
    evidence_id         BIGINT UNSIGNED PRIMARY KEY AUTO_INCREMENT,
    memory_id           BIGINT UNSIGNED NOT NULL,

    -- 证据来源
    message_id          BIGINT UNSIGNED NULL,
    summary_id          BIGINT UNSIGNED NULL,

    evidence_type       VARCHAR(32)     NOT NULL
        COMMENT 'source|reinforcement|contradiction|manual',

    confidence          DECIMAL(4,3)    NULL
        COMMENT '此条 evidence 贡献的置信度',

    extractor_version   VARCHAR(64)     NULL,

    created_at          DATETIME(6)     NOT NULL DEFAULT CURRENT_TIMESTAMP(6),

    -- 同一 (memory + message + evidence_type) 不能重复
    UNIQUE KEY uk_mem_msg_type (memory_id, message_id, evidence_type),

    -- 跨来源去重键（解决 message_id / summary_id 为 NULL 时 UK 不生效的问题）
    source_type        VARCHAR(32)     GENERATED ALWAYS AS (
        CASE
            WHEN message_id IS NOT NULL THEN 'message'
            WHEN summary_id IS NOT NULL THEN 'summary'
            ELSE 'manual'
        END
    ) STORED,
    source_id          BIGINT UNSIGNED GENERATED ALWAYS AS (
        COALESCE(message_id, summary_id, 0)
    ) STORED,
    UNIQUE KEY uk_memory_source_type (memory_id, source_type, source_id, evidence_type),

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
  COMMENT='记忆证据关系表（全链路审计）';
```

**用途**：
- 链路上可回答"某条 memory 由哪些消息派生"
- 反向查询"某条消息派生了哪些 memory"
- clear/forget 时根据 `message_id` 精确清理关联的 evidence 和 memory
- 去重约束防止同一 evidence 被重复计入

### 3.6 `user_persona_snapshots` — 派生快照（重设计）

```sql
CREATE TABLE user_persona_snapshots (
    snapshot_id         BIGINT UNSIGNED PRIMARY KEY AUTO_INCREMENT,
    user_id             BIGINT UNSIGNED NOT NULL,

    -- ★ 用 GENERATED 列保证 MySQL 下同一用户最多一条 active
    --    当 status='active' 时 active_user_id = user_id，否则为 NULL
    --    UNIQUE 约束在 NULL 上不生效（MySQL 允许多 NULL），实现 at-most-one-active
    active_user_id      BIGINT UNSIGNED
        GENERATED ALWAYS AS (
            CASE WHEN status = 'active' THEN user_id ELSE NULL END
        ) STORED,

    -- 画像内容（JSON，结构见 §4.2）
    snapshot_data       JSON            NOT NULL,

    -- 生成输入溯源
    source_memory_ids   JSON            NOT NULL,
    source_summary_ids  JSON            NULL,
    source_recent_message_ids JSON      NULL
        COMMENT '实际使用的消息 ID 列表（非 range）',

    -- 输入指纹
    input_hash          CHAR(64)        NOT NULL,

    -- 生成元数据
    model_name          VARCHAR(128)    NOT NULL,
    prompt_version      VARCHAR(64)     NOT NULL,
    schema_version      VARCHAR(64)     NOT NULL,
    generation_ms       INT UNSIGNED    NOT NULL,

    -- 生命周期
    status              VARCHAR(32)     NOT NULL DEFAULT 'active'
        COMMENT 'active|superseded|expired|error',
    supersedes_id       BIGINT UNSIGNED NULL,
    error_message       TEXT            NULL,

    created_at          DATETIME(6)     NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    expires_at          DATETIME(6)     NULL,

    -- ★ 保证 at-most-one-active
    UNIQUE KEY uk_active_user (active_user_id),

    INDEX idx_user_status_created (user_id, status, created_at DESC),
    INDEX idx_input_hash (input_hash),

    FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE,
    FOREIGN KEY (supersedes_id) REFERENCES user_persona_snapshots(snapshot_id)
        ON DELETE SET NULL
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci
  COMMENT='用户画像派生快照表（纯缓存，1:N，最多一条 active）';
```

**MySQL 兼容性说明**：
- `GENERATED ALWAYS AS (...) STORED` 在 MySQL 5.7+ / 8.0+ 均支持
- `UNIQUE(active_user_id)` — MySQL 允许多行 `NULL`，所以 `superseded/expired/error` 记录不受约束
- `active` 状态的记录只有一条（`user_id` 非 NULL 时触发 UNIQUE）

### 3.7 `user_safety_context` — 安全上下文

```sql
CREATE TABLE user_safety_context (
    user_id                 BIGINT UNSIGNED PRIMARY KEY,

    -- 最近高风险时间
    last_high_risk_at       DATETIME(6)     NULL,
    last_crisis_at          DATETIME(6)     NULL,

    -- 滚动窗口统计
    total_detections_90d    INT UNSIGNED    NOT NULL DEFAULT 0,
    high_risk_count_90d     INT UNSIGNED    NOT NULL DEFAULT 0,
    crisis_count_90d        INT UNSIGNED    NOT NULL DEFAULT 0,

    -- 当前安全策略标记
    active_patterns         JSON            NOT NULL,
    recommended_strategy    VARCHAR(64)     NOT NULL DEFAULT 'standard'
        COMMENT 'standard|elevated_concern|safety_focused',

    -- 策略理由
    strategy_reason         TEXT            NULL,

    -- ★ 审计字段
    source_risk_result_ids  JSON            NULL,
    window_start_at         DATETIME(6)     NOT NULL,
    window_end_at           DATETIME(6)     NOT NULL,
    input_hash              CHAR(64)        NOT NULL,
    detector_versions       JSON            NULL,
    schema_version          VARCHAR(64)     NOT NULL,
    updater_version         VARCHAR(64)     NOT NULL,

    -- 生命周期
    generated_at            DATETIME(6)     NOT NULL,
    next_refresh_at         DATETIME(6)     NOT NULL,
    data_window_days        INT UNSIGNED    NOT NULL DEFAULT 90,

    FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci
  COMMENT='用户安全上下文（仅影响安全响应，不进入画像）';
```

### 3.8 `user_safety_observations` — 安全观察（新增）

从 `user_memories` 中独立出来的安全相关记录。

```sql
CREATE TABLE user_safety_observations (
    observation_id      BIGINT UNSIGNED PRIMARY KEY AUTO_INCREMENT,
    user_id             BIGINT UNSIGNED NOT NULL,
    risk_result_id      BIGINT UNSIGNED NULL
        COMMENT '来源 risk_detection_results.id',

    pattern             VARCHAR(128)    NOT NULL
        COMMENT 'self_harm_reference|suicidal_ideation|violence_risk|...',
    severity            VARCHAR(32)     NOT NULL
        COMMENT 'monitor|concern|immediate',
    confidence          DECIMAL(4,3)    NOT NULL,

    observed_at         DATETIME(6)     NOT NULL,
    expires_at          DATETIME(6)     NOT NULL,
    status              TINYINT         NOT NULL DEFAULT 1
        COMMENT '1=active 0=expired',

    created_at          DATETIME(6)     NOT NULL DEFAULT CURRENT_TIMESTAMP(6),

    FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE,
    FOREIGN KEY (risk_result_id) REFERENCES risk_detection_results(id)
        ON DELETE SET NULL,

    INDEX idx_user_status (user_id, status),
    INDEX idx_expires (expires_at)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci
  COMMENT='用户安全观察表（独立于长期记忆）';
```

### 3.9 `user_context_versions` — 上下文版本号

```sql
CREATE TABLE user_context_versions (
    user_id     BIGINT UNSIGNED PRIMARY KEY,
    version     BIGINT UNSIGNED NOT NULL DEFAULT 1,
    updated_at  DATETIME(6)     NOT NULL DEFAULT CURRENT_TIMESTAMP(6)
        ON UPDATE CURRENT_TIMESTAMP(6),

    FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci
  COMMENT='用户上下文版本号（bump on memory/persona/safety/summary change）';
```

**版本号 bump 时机**：
- memory 被 disable / contradicted / superseded
- persona snapshot 被 reset / 新建
- safety context 被刷新
- summary 被新增（rolling update）
- chat/clear 或 chat/forget 执行

### 3.10 `user_profiles` 扩展

```sql
ALTER TABLE user_profiles
    ADD COLUMN personalization_enabled TINYINT(1) NOT NULL DEFAULT 1
        COMMENT '个性化是否启用',
    ADD COLUMN personalization_reset_at DATETIME(6) NULL
        COMMENT '最近一次重置时间，Persona Engine 只使用此时间之后的 memories';
```

### 3.11 完整 ER 关系

```
users (1)
  ├── (1) conversations
  │       └── (N) conversation_messages
  │       └── (N) conversation_summaries
  │       └── (N) risk_detection_results
  ├── (N) user_memories
  │       └── (N) user_memory_evidence
  ├── (N) user_persona_snapshots    ← 1:N, at most one active
  ├── (1) user_safety_context
  ├── (N) user_safety_observations
  ├── (1) user_context_versions
  └── (1) user_profiles
```

---

## 4. Persona Snapshot 数据模型

### 4.1 设计原则

1. **纯派生缓存**。删除后从 L2+L3 重建，不影响数据完整性。
2. **每项结论有 evidence**。追溯到 memory_ids / summary_ids。
3. **不存抽象人格分数**。只存"可被回答策略使用的观察"。
4. **不含 safety 信号**。安全数据不进入此结构。

### 4.2 `snapshot_data` JSON Schema

```json
{
  "communication_preferences": [
    {
      "preference": "用户更接受直接、结构化的建议而非开放式提问",
      "confidence": 0.82,
      "evidence": { "memory_ids": [11, 29, 45], "summary_ids": [7] }
    }
  ],

  "support_preferences": [
    {
      "preference": "谈论压力话题时，先共情再给行动建议",
      "confidence": 0.88,
      "evidence": { "memory_ids": [88, 94, 117], "summary_ids": [3, 7, 12] }
    }
  ],

  "style_observations": {
    "tone": "warm",
    "directness": "medium",
    "structure": "step_by_step",
    "question_frequency": "low",
    "humor": "avoid_unless_user_initiates",
    "advice_style": "actionable",
    "confidence": 0.74,
    "evidence": { "memory_ids": [11, 22, 29, 45], "summary_ids": [7, 12] }
  },

  "stable_facts": [
    {
      "fact": "用户有一只叫 Luna 的猫",
      "fact_type": "pet",
      "confidence": 0.95,
      "evidence": { "memory_ids": [5], "reinforce_count": 3 }
    }
  ],

  "recurring_topics": [
    {
      "topic": "职场人际关系",
      "frequency": "frequent",
      "last_discussed_at": "2026-06-12T08:30:00Z",
      "evidence": { "memory_ids": [11, 29, 45, 67, 101], "summary_ids": [3, 7, 12] }
    }
  ],

  "goals": [
    {
      "goal": "改善与上级的沟通方式",
      "status": "active",
      "stated_at": "2026-05-20T12:00:00Z",
      "last_referenced_at": "2026-06-10T08:30:00Z",
      "evidence": { "memory_ids": [29, 67] }
    }
  ],

  "sensitive_context": [
    {
      "topic": "父母关系",
      "handling": "avoid_unsolicited_probe",
      "confidence": 0.76,
      "last_observed_at": "2026-06-13T08:30:00Z",
      "evidence": { "memory_ids": [101, 122] }
    }
  ],

  "interaction_context": {
    "known_history_available": true,
    "longitudinal_confidence": 0.76,
    "allowed_reference_style": "may_reference_past_when_relevant"
  }
}
```

### 4.3 输出校验

除 JSON Schema 结构校验外，增加业务校验：

1. **证据归属**：所有 `evidence.memory_ids` ⊆ `source_memory_ids`；所有 `evidence.summary_ids` ⊆ `source_summary_ids`
2. **证据充分性**：`confidence >= 0.8` → 至少 2 条独立 evidence，或 `memory.reinforce_count >= 2`
3. **禁止诊断词**：输出中不得出现 DSM/ICD 术语、人格障碍标签、临床病理化描述
4. **禁止安全泄露**：sensitive_context 只能引用 `memory_type IN ('preference','fact','emotional_pattern','goal')`，不得引用 safety 数据
5. **evidence ID 去重**：同一 `memory_ids` 数组内无重复
6. **sensitive_context 时效性**：每条必须有 `last_observed_at`
7. **禁止冗余 evidence**：如 `"evidence": {"memory_ids": [5,5,5]}` 应被拒绝

校验失败 → `status='error'` + `error_message` 记录具体失败项。

### 4.4 全链路审计能力

设计保证以下问题可回答：

| 审计问题 | 答案来源 |
|---|---|
| 这条 persona 用了哪些 memory？ | `source_memory_ids` |
| 这些 memory 由哪些消息派生？ | `user_memory_evidence.message_id` → `conversation_messages` |
| 输入数据是否与上次生成时相同？ | `input_hash` 对比 |
| 用哪个模型/提示词版本生成的？ | `model_name` + `prompt_version` + `schema_version` |
| 是否成功？ | `status` + `error_message` |
| 生成了多久？ | `generation_ms` |
| 为什么 confidence 是 0.82？ | `snapshot_data` 内每个 field 的 `evidence` |
| 这条 memory 被强化了几次？ | `user_memory_evidence` WHERE `evidence_type='reinforcement'` |

---

## 5. Memory 强化与去重

### 5.1 三段式去重流程

```
新对话提取 memory A
        │
        ▼
Stage 1: deterministic key
  canonical_form = LLM 规范化表述
  memory_key = SHA256(canonical_form)
  查 user_memories WHERE user_id=? AND memory_key=?
        │
  ┌─────┴─────┐
  │  命中      │  未命中
  └─────┬─────┘   │
        │         ▼
        │    Stage 2: embedding top_k
        │      向量召回 top 5 相似旧 memory
        │         │
        │    ┌────┴────┐
        │    │ 有相似   │ 无相似 → INSERT
        │    └────┬────┘
        │         │
        ▼         ▼
    Stage 3: LLM merge classifier
      判断: same / related / new_evidence / contradiction
        │
  ┌─────┼─────┬──────────┐
  │     │     │          │
 same  related new_ev    contradiction
  │     │     │          │
  │     │     ▼          ▼
  │     │  INSERT      旧 memory:
  │     │  user_       confidence *= 0.5
  │     │  memory_     若 confidence < 0.3:
  │     │  evidence     status = -1
  │     │  (type=       contradicted_at = NOW()
  │     │   reinforce)  INSERT 新 memory
  │     │  reinforce_   (supersedes 指向旧)
  │     │  count += 1
  │     │  reinforced_
  │     │  at = NOW()
  │     │  confidence
  │     │  = f(source_confidence,
  │     │      reinforce_count)
  │     │
  │     ▼
  │  merge_decision 记录
  │  canonicalizer_version 记录
```

### 5.2 `reinforce_count` 规则

- **仅在新消息独立支撑同一结论时 +1**
- Persona Engine 的读取不改变 `reinforce_count`
- 同一条 message 不能对同一 memory 产生多条 reinforce evidence（`uk_mem_msg_type` 保证）

---

## 6. Safety Context — 独立数据流

### 6.1 数据隔离保证

```
risk_detection_results ──→ SafetyContextUpdater ──→ user_safety_context
                                                         │
user_safety_observations ←── (独立写入)                    │
                                                         │
                    ┌────────────────────────────────────┘
                    │
                    ▼
          AgentRuntime 安全快速路径
              │
              ├── 当前消息 HIGH/CRISIS → 立即安全响应
              ├── 最近 N 轮（或最近 30 分钟）多次 MEDIUM+ → 启用 elevated safety prompt
              └── 仅 90d 历史有高风险，当前无风险 → 不注入具体模式

Persona Engine ──→ user_persona_snapshots ──→ 普通回复策略
                    ↑
                    │
              ┌─────┴─────┐
              │ memory    │ summary
              │ (不含     │
              │  safety_  │
              │  note)    │
              └───────────┘
```

### 6.2 激活规则（三层）

| 层级 | 触发条件 | 行为 |
|---|---|---|
| **L1: current_turn_risk** | 当前消息 risk_level = HIGH/CRISIS | 立即走安全响应，不入 LLM |
| **L2: recent_turn_risk_window** | 最近 N 轮（或最近 30 分钟内）MEDIUM ≥ 3 或 HIGH ≥ 1 | 启用 elevated safety prompt，提示模型关注但不主动提及 |
| **L3: historical_safety_ctx** | user_safety_context 存在，但 L1/L2 不满足 | 不注入具体模式到 system prompt，仅调整内部监测敏感度 |

**关键**：L3 的历史统计不直接出现在普通对话的 system prompt 中。它们仅作为 AgentRuntime 内部策略参数。

---

## 7. System Prompt 注入规范

### 7.1 分层结构

```
═══════════════════════════════════════════════════
[系统角色]
你是一位有同理心的专业心理陪伴助手...

═══════════════════════════════════════════════════
[可信上下文 — 用户显式提供]
- 用户基本资料: {user_profile}
- 当前位置（本请求透传）: {location}

═══════════════════════════════════════════════════
[非可信上下文 — 从历史对话自动推断]

以下内容来自历史对话的自动提取和推断。
**不是事实，不构成诊断或判断。**
当用户当前表达与此冲突时，以用户当前表达为准。

[对话摘要 — 可能有信息损失]
{latest_summaries}

[行为偏好 — 可用于调整表达风格]
{persona.communication_preferences}
{persona.support_preferences}
{persona.style_observations}

[长期观察 — 用户提及的信息]
{persona.stable_facts}
{persona.recurring_topics}
{persona.goals}

[谨慎话题]
{persona.sensitive_context}

═══════════════════════════════════════════════════
[安全提示 — 仅当前对话存在活跃风险信号时出现]
{仅在 L2 满足时注入}

═══════════════════════════════════════════════════
[工具使用规则 + 安全规则]
```

### 7.2 主动失效机制

无 Session 模型下，改用 PromptContext 缓存按 `(user_id, context_version)` 校验：

```
每次 POST /chat/messages 处理前:
  current_version = user_context_versions.version

  if prompt_cache miss for (user_id, current_version):
      build PromptContext from DB (persona + summaries + memories + profile)
      cache under (user_id, current_version)

  AgentRuntime 使用该 PromptContext 构建 system prompt
```

第一阶段可不做 prompt cache，每轮都从 DB 构建 PromptContext。
`context_version` 仍保留，用于异步任务 epoch 校验和后续缓存失效。

**bump context_version 的操作**：
- memory disable/contradicted
- persona snapshot status 变更
- safety context 刷新
- rolling summary 新增
- chat/clear 或 chat/forget
- personalization_reset

**不 bump 的操作**：
- 普通消息发送和回复持久化
- memory 新增（不影响已有 system prompt）

---

## 8. API 设计

### 8.1 端点清单

| 方法 | 路径 | 说明 |
|---|---|---|
| POST | `/api/v1/chat/open` | 打开/复用会话 |
| POST | `/api/v1/chat/messages` | 发送消息 |
| GET | `/api/v1/chat/history` | 分页获取消息历史 |
| GET | `/api/v1/chat/memories` | 查看系统记住的记忆 |
| GET | `/api/v1/chat/persona` | 查看当前活跃画像 |
| POST | `/api/v1/chat/memory/{id}/disable` | 禁用单条记忆 |
| POST | `/api/v1/chat/persona/rebuild` | 从现有 memories 重建画像 |
| POST | `/api/v1/chat/persona/reset` | 重置个性化（过期画像 + 禁用个性化） |
| POST | `/api/v1/chat/transcript/clear` | 清空对话记录（保留记忆） |
| POST | `/api/v1/chat/forget` | 完全遗忘（清空对话 + 禁用记忆 + 过期画像 + 关闭 session） |
| GET | `/api/v1/chat/risk-detections` | 用户风险检测列表 |

### 8.2 接口详细定义

#### 8.2.1 打开/预热对话

```
POST /api/v1/chat/open
Authorization: Bearer <token>

Request: { }   // 空 body，或传空 JSON

Response 200:
{
    "conversation": {
        "id": 42,
        "message_count": 1523,
        "last_message_at": "2026-06-13T08:30:00Z"
    },
    "personalization_enabled": true
}
```

**幂等**：重复调用永远 200。内部执行 `find_or_create_conversation` (UPSERT)，不创建会话。
`open` 用于前端预热：触发 PromptContext 预加载（如有缓存层），验证 Token 有效。

#### 8.2.2 发送消息

```
POST /api/v1/chat/messages
Authorization: Bearer <token>

Request:
{
    "text": "...",
    "emotion": "anxious",              // 可选
    "location": { ... }                // 可选，传递瞬态上下文
}

Response 200:
{
    "reply": "...",
    "conversation_id": 42,
    "tool_calls": [...]
}
```

流程：`ChatService::send_message(user_id, text, ...)`，内部 `find_or_create_conversation`，无 Session 概念。
每轮消息独立编排，不依赖任何 session 概念。

#### 8.2.3 用户控制端点

**清空对话记录（保留长期记忆）**：

```
POST /api/v1/chat/transcript/clear
Authorization: Bearer <token>

Response 200:
{
    "cleared_messages": true,
    "cleared_summaries": true,
    "memories_preserved": true,
    "persona_preserved": true
}
```

行为：
- `DELETE FROM conversation_messages WHERE conversation_id=?`
- `DELETE FROM conversation_summaries WHERE conversation_id=?`
- `UPDATE conversations SET message_count=0, title=NULL, updated_at=NOW()`
- `UPDATE user_memory_evidence SET message_id=NULL WHERE message_id IN (deleted)`
- 清理关联的向量索引（Qdrant points + vector_index_records）
- bump `context_version`
- 至此为止的对话上下文已丢失，下一轮自动从 DB 最新状态重建

**完全遗忘（重置一切）**：

```
POST /api/v1/chat/forget
Authorization: Bearer <token>

Response 200:
{
    "messages_cleared": true,
    "summaries_cleared": true,
    "memories_disabled": 47,
    "persona_expired": true,
    "safety_context_reset": true,
}
```

行为：
- 执行 transcript/clear 的所有操作
- `UPDATE user_memories SET status=0 WHERE user_id=?`
- 标记 active persona snapshot 为 `expired`
- `DELETE FROM user_safety_context WHERE user_id=?` 并立即重新计算
- `UPDATE user_profiles SET personalization_reset_at=NOW()`
- bump `context_version`
- 至此为止的对话上下文已丢失，下一轮自动从 DB 最新状态重建

**重置个性化**：

```
POST /api/v1/chat/persona/reset
Authorization: Bearer <token>

Response 200:
{
    "persona_expired": true,
    "personalization_disabled": true,
    "message": "个性化已关闭。后续对话积累足够证据后可重新开启。"
}
```

行为：
- 标记 active persona snapshot 为 `expired`
- `UPDATE user_profiles SET personalization_enabled=0, personalization_reset_at=NOW()`
- bump `context_version`

**重建画像**：

```
POST /api/v1/chat/persona/rebuild
Authorization: Bearer <token>

Response 200:
{
    "snapshot_id": 15,
    "message": "画像已从当前记忆重建。",
    "source_memory_count": 89
}
```

行为：
- 从当前 active memories 重新执行 Persona Engine 合成
- 生成新 snapshot，旧 snapshot status → `superseded`
- `UPDATE user_profiles SET personalization_enabled=1`
- bump `context_version`

#### 8.2.4 用户可见查询

**查看记忆**：

```
GET /api/v1/chat/memories?type=preference,fact&limit=50
Authorization: Bearer <token>

Response 200:
{
    "memories": [
        {
            "memory_id": 42,
            "memory_type": "preference",
            "content": "用户偏好直接、结构化的建议",
            "confidence": 0.85,
            "reinforce_count": 3,
            "created_at": "2026-05-10T...",
            "reinforced_at": "2026-06-12T..."
        }
    ],
    "total_active": 89
}
```

**查看画像**：

```
GET /api/v1/chat/persona
Authorization: Bearer <token>

Response 200:
{
    "has_active_persona": true,
    "generated_at": "2026-06-12T10:00:00Z",
    "snapshot_summary": {
        "communication_preferences_count": 3,
        "stable_facts_count": 5,
        "recurring_topics_count": 2,
        "goals_count": 1,
        "sensitive_context_count": 2
    }
    "personalization_enabled": true
}
```

只返回摘要计数，不返回完整画像内容（内部推断不应直接暴露原始数据给用户，但应让用户知道"系统记住了什么类型的信息"）。

---

## 9. 核心流程

### 9.1 打开/预热对话流程

```
POST /chat/open
    │
    ▼
ChatService::open(user_id)
    │
    ▼
ConversationRepository.find_or_create_by_user_id()   // UPSERT (幂等)
    │
    ▼
返回 200 { conversation, personalization_enabled }
```

`open` 不构建 system prompt、不创建 Session。
后续第一条 `/chat/messages` 触发完整的 PromptContext 加载。

### 9.2 消息处理流程

```
POST /chat/messages
    │
    ▼
ChatService::send_message(user_id, text, location)
    │
    ▼
get_user_lock(user_id).lock()         // per-user mutex
    │
    ▼
find_or_create_conversation(user_id)   // UPSERT
    │
    ▼
load PromptContext from DB:
  - conversation_summaries (status=1, rolling_general, ORDER BY message_end_id DESC LIMIT 3)
  - user_persona_snapshots (status='active')
  - user_profile
  - user_safety_context (仅用于安全策略，不注入 prompt)
    │
    ▼
build_system_message() → §7 分层 prompt
    │
    ▼
从 DB 加载最近 N 条消息 → recent_messages
    │
    ▼
AgentRuntime::respond(user_id, conversation_id, recent_messages, system_prompt, ...)
  ├── Safety pre-check (L1: 当前消息)
  │     └── CRISIS → 安全响应
  ├── LLM loop (with tools)
  │     └── system prompt 按 §7 分层
  ├── Persist user + assistant messages
  └── Spawn async:
        ├── Memory extraction (含 3-stage 去重 + evidence 写入)
        ├── Risk persist + SafetyContextUpdater
        └── Summary refresh (rolling_general / rolling_safety, 触发条件: len>=8 && len%6==0)
    │
    ▼
返回 ChatResponse { reply, conversation_id, tool_calls }
```

### 9.3 Persona Engine 流程

```
Scheduler: 每 24h
    │
    ▼
SELECT user_id FROM conversations WHERE updated_at > NOW() - INTERVAL 30 DAY
    │ (仅活跃用户)
    ▼
对每个用户检查是否需要生成:
  1. personalization_enabled = 0 → skip
  2. 无 active persona → 需要
  3. 上次生成后新增 memories >= 50 条 → 需要
  4. 距上次 >= 24h → 需要
    │
    ▼
收集输入:
  1. user_memories WHERE memory_type IN
       ('preference','fact','emotional_pattern','goal')
     AND status=1
     AND created_at > personalization_reset_at
     ORDER BY salience DESC, confidence DESC LIMIT 200
  2. conversation_summaries WHERE status=1
     ORDER BY message_end_id DESC LIMIT 10
  3. conversation_messages WHERE sender_role IN ('user','assistant')
     ORDER BY id DESC LIMIT 100
  4. user_profile
  ★ 不读取 risk_detection_results
  ★ 不读取 user_safety_observations
    │
    ▼
计算 input_hash
对比上次 snapshot 的 input_hash → 相同则 skip
    │
    ▼
LLM Persona Synthesis (§10.3 prompt)
    │
    ▼
业务校验 (§4.3)
    │
  ┌─┴─┐
  │通过│ 失败 → INSERT snapshot (status='error', error_message=...)
  └─┬─┘
    │
旧 snapshot.status → 'superseded'
INSERT 新 snapshot
    │
    ▼
bump context_version
发布 TaskEvent::PersonaUpdated
```

---

## 10. Prompt 模板

### 10.1 Persona Synthesis Prompt

```
你是一个用户沟通偏好分析工具（非心理诊断工具）。

## 任务
基于用户与 AI 心理陪伴助手的对话历史，推断用户的沟通偏好和支持需求。
**不要推断心理诊断、人格分数或临床结论。**

## 输入数据

### 长期观察（从对话提取的记忆）
{memories_block}

### 对话摘要
{summaries_block}

### 最近对话样本（仅用于风格分析）
{recent_dialogues}

## 输出
严格按以下 JSON Schema 输出，不要输出任何其他文字：

{
  "communication_preferences": [
    {
      "preference": "具体偏好描述",
      "confidence": 0.0-1.0,
      "evidence": {"memory_ids": [], "summary_ids": []}
    }
  ],
  "support_preferences": [
    {
      "preference": "具体支持偏好描述",
      "confidence": 0.0-1.0,
      "evidence": {"memory_ids": [], "summary_ids": []}
    }
  ],
  "style_observations": {
    "tone": "warm|neutral|professional",
    "directness": "low|medium|high",
    "structure": "free_form|step_by_step|question_answer",
    "question_frequency": "low|medium|high",
    "humor": "welcome|avoid_unless_user_initiates|avoid",
    "advice_style": "actionable|exploratory|reflective",
    "confidence": 0.0-1.0,
    "evidence": {"memory_ids": [], "summary_ids": []}
  },
  "stable_facts": [
    {
      "fact": "用户提及的事实",
      "fact_type": "pet|family|work|hobby|health|other",
      "confidence": 0.0-1.0,
      "evidence": {"memory_ids": [], "reinforce_count": 0}
    }
  ],
  "recurring_topics": [
    {
      "topic": "话题",
      "frequency": "rare|occasional|frequent",
      "last_discussed_at": "ISO8601 或 null",
      "evidence": {"memory_ids": [], "summary_ids": []}
    }
  ],
  "goals": [
    {
      "goal": "目标描述",
      "status": "active|completed|abandoned",
      "stated_at": "ISO8601 或 null",
      "last_referenced_at": "ISO8601 或 null",
      "evidence": {"memory_ids": []}
    }
  ],
  "sensitive_context": [
    {
      "topic": "话题",
      "handling": "avoid_unsolicited_probe|gentle_only|acknowledge_if_user_raises",
      "confidence": 0.0-1.0,
      "last_observed_at": "ISO8601",
      "evidence": {"memory_ids": []}
    }
  ],
  "interaction_context": {
    "known_history_available": true|false,
    "longitudinal_confidence": 0.0-1.0,
    "allowed_reference_style": "may_reference_past_when_relevant|prefer_current_context_only"
  }
}

## 规则
1. 所有 evidence.memory_ids 必须来自输入中提供的记忆 ID
2. 所有 evidence.summary_ids 必须来自输入中提供的摘要 ID
3. 没有足够证据的维度，confidence 应降低
4. 不得输出任何诊断术语、人格标签、病理化描述
5. interaction_context.allowed_reference_style 为 "prefer_current_context_only"
   当 longitudinal_confidence < 0.5 时
6. sensitive_context 仅列出用户明确表现出不适或回避的话题
```

### 10.2 Memory Merge Classifier Prompt

```
判断新提取的观察与已有记忆的关系。输出 JSON:

{
  "decision": "same|related|new_evidence|contradiction|new",
  "canonical_form": "规范化表述",
  "reason": "简短的判断理由"
}

- same: 新观察与已有记忆表达同一事实，无需更新
- related: 相关但不同，应作为新记忆保存
- new_evidence: 新观察独立支撑已有记忆，应增强其置信度
- contradiction: 新观察与已有记忆矛盾
- new: 全新的观察，应创建新记忆
```

---

## 11. 向量/Embedding 清理策略

### 11.1 复用 vector_index_jobs

不新增 `vector_cleanup_jobs` 表，复用现有 `vector_index_jobs` 表。
该表字段完备（`action`, `object_type`, `object_id`, `collection_name`, `vector_id`, `status`, `attempts`），天然支持 `action='delete'`。

```
操作 (e.g. memory disable, summary delete, forget)
    │
    ▼
MySQL: UPDATE status=0 / DELETE
    │
    ▼
VectorIndexRepository.enqueue_delete_job(object_type, object_id, vector_id, collection_name)
    │  → INSERT INTO vector_index_jobs (action='delete', status='pending')
    ▼
VectorIndexWorker (现有后台处理)
    │
    ├── Qdrant: delete_points(collection, vector_id)
    ├── MySQL: UPDATE vector_index_records SET index_status='deleted'
    └── MySQL: UPDATE target SET vector_id=NULL, indexed_at=NULL
```

**调用处**：
- `MemoryService::disable_memory()` → `enqueue_delete_job('memory', id, ...)`
- `SummaryService::disable_summary()` → `enqueue_delete_job('summary', id, ...)`
- `forget` / `transcript/clear` → 批量 enqueue

**不新增 `vector_cleanup_jobs` 表，不新增 `VectorCleanupWorker`。**


## 12. 并发控制

单实例部署，但以下操作可能并发冲突：

| 冲突场景 | 保护方式 |
|---|---|
| `/chat/messages` 与 `/chat/clear` | 用户级 mutex |
| `/chat/messages` 与 `/chat/forget` | 用户级 mutex |
| `/chat/messages` 与 `/persona/reset` | 用户级 mutex |
| `/chat/messages` 与 `/persona/rebuild` | 用户级 mutex |
| `/chat/messages` 与 `/memory/{id}/disable` | 用户级 mutex |
| Persona Scheduler 与 `/persona/reset` | try_lock（失败 skip） + `active_user_id` UNIQUE 约束 |
| MemoryExtractor 与 `/memory/{id}/disable` | try_lock（失败 skip） |
| SummaryRefreshService 与 `/chat/clear` | try_lock（失败 skip） |
| SafetyContextUpdater 与 risk 写入 | try_lock（失败 skip） |

**实现**：`ChatService` 内 `DashMap<UserId, Arc<Mutex<()>>>`。
前台操作（API handler）`lock()`；后台任务 `try_lock()`，失败则跳过当前周期。DB 事务保证最终一致性。

---

## 13. 统一迁移脚本

开发阶段，使用 DROP + CREATE 策略：

```sql
-- ============================================================================
-- V2__one_conversation_per_user.sql
-- 策略: DROP + CREATE（开发阶段）
-- ============================================================================

SET FOREIGN_KEY_CHECKS = 0;

-- 按依赖倒序删除
DROP TABLE IF EXISTS user_safety_observations;
DROP TABLE IF EXISTS user_memory_evidence;
DROP TABLE IF EXISTS user_memory_embeddings;
DROP TABLE IF EXISTS user_persona_snapshots;
DROP TABLE IF EXISTS user_safety_context;
DROP TABLE IF EXISTS user_context_versions;
DROP TABLE IF EXISTS user_memories;
DROP TABLE IF EXISTS conversation_summaries;
DROP TABLE IF EXISTS risk_detection_results;
DROP TABLE IF EXISTS conversation_messages;
DROP TABLE IF EXISTS agent_events;
DROP TABLE IF EXISTS conversations;

SET FOREIGN_KEY_CHECKS = 1;

-- 按依赖顺序创建（使用 §3 中的完整 CREATE TABLE 语句）
-- conversations → conversation_messages → conversation_summaries
-- → user_memories → user_memory_evidence → user_persona_snapshots
-- → user_safety_context → user_safety_observations
-- → user_context_versions

-- 扩展 user_profiles
ALTER TABLE user_profiles
    ADD COLUMN IF NOT EXISTS personalization_enabled TINYINT(1) NOT NULL DEFAULT 1,
    ADD COLUMN IF NOT EXISTS personalization_reset_at DATETIME(6) NULL;

-- 初始化所有用户的 context_version
INSERT INTO user_context_versions (user_id, version)
    SELECT id, 1 FROM users
    ON DUPLICATE KEY UPDATE user_id = user_id;
```

---

## 14. 配置

```toml
[persona]
enabled = true
generation_interval_secs = 86400
incremental_memory_threshold = 50
incremental_cooldown_secs = 21600
initial_message_threshold = 20
max_memories = 200
max_summaries = 10
max_recent_messages = 100
max_concurrent_generations = 5
llm_temperature = 0.1
llm_max_tokens = 2048
active_user_window_days = 30
snapshot_schema_version = "v1"

[safety_context]
enabled = true
refresh_interval_secs = 3600
data_window_days = 90
schema_version = "v1"
updater_version = "1.0"


```

---

## 15. 代码变更清单

### 15.1 新增文件

| 文件 | 说明 |
|---|---|
| `src/domain/persona/mod.rs` | Persona 模块 |
| `src/domain/persona/snapshot.rs` | PersonaSnapshot + PersonaSnapshotRepository trait |
| `src/domain/safety/mod.rs` | Safety 模块 |
| `src/domain/safety/safety_context.rs` | SafetyContext + SafetyContextRepository trait |
| `src/domain/safety/safety_observation.rs` | SafetyObservation + SafetyObservationRepository trait |
| `src/domain/memory/memory_evidence.rs` | MemoryEvidence + MemoryEvidenceRepository trait |
| `src/application/persona/mod.rs` | Persona 应用模块 |
| `src/application/persona/persona_engine.rs` | Persona Snapshot 合成引擎 + 校验 |
| `src/application/persona/persona_scheduler.rs` | 定时调度器 |
| `src/application/safety/safety_context_updater.rs` | Safety Context 更新器 |
| `src/application/memory/memory_merge_classifier.rs` | 三段式去重 + Merge 分类 |
| `src/infrastructure/persistence/entities/user_persona_snapshots.rs` | SeaORM Entity |
| `src/infrastructure/persistence/entities/user_safety_context.rs` | SeaORM Entity |
| `src/infrastructure/persistence/entities/user_safety_observations.rs` | SeaORM Entity |
| `src/infrastructure/persistence/entities/user_memory_evidence.rs` | SeaORM Entity |
| `src/infrastructure/persistence/entities/user_context_versions.rs` | SeaORM Entity |
| `src/infrastructure/persistence/implementations/seaorm_persona_repository.rs` |  |
| `src/infrastructure/persistence/implementations/seaorm_safety_context_repository.rs` |  |
| `src/infrastructure/persistence/implementations/seaorm_safety_observation_repository.rs` |  |
| `src/infrastructure/persistence/implementations/seaorm_memory_evidence_repository.rs` |  |
| `src/infrastructure/persistence/implementations/seaorm_context_version_repository.rs` |  |

### 15.2 修改文件

| 文件 | 变更 |
|---|---|
| `src/domain/mod.rs` | 新增模块 |
| `src/domain/conversation/conversation_repository.rs` | 新增 `find_or_create_for_user()` |
| `src/domain/memory/mod.rs` | UserMemory 新增字段 |
| `src/domain/tasks/task_event.rs` | 新增事件类型 |
| `src/application/session/chat_service.rs` | 新增：每轮消息编排器（替代 SessionManager） |
| `src/application/session/conversation_orchestrator.rs` | 新增 `ensure_user_conversation()`；新增 `build_persona_v2()` |
| `src/application/session/session_service.rs` | 新增查询方法 |
| `src/application/agent/agent_runtime.rs` | `build_system_message()` 按 §7 分层；L1/L2/L3 安全策略 |
| `src/application/agent/agent_context.rs` | AgentContext 新增 persona 字段 |
| `src/application/memory/memory_service.rs` | 集成三段式去重 + evidence 写入 |
| `src/application/memory/memory_extractor.rs` | 提取输出纳入 merge 流程 |
| `src/infrastructure/persistence/entities/conversations.rs` | 同步 DDL |
| `src/infrastructure/persistence/entities/conversation_messages.rs` | 同步 DDL |
| `src/infrastructure/persistence/entities/conversation_summaries.rs` | 同步 DDL |
| `src/infrastructure/persistence/entities/user_memories.rs` | 同步 DDL |
| `src/api/router.rs` | 路由调整 |
| `src/api/state.rs` | 新增状态 |
| `src/api/dto/chat_dto.rs` | DTO 重构（删除 session DTO，新增 chat DTO） |
| `src/api/handlers/chat_handler.rs` | Handler 重构（删除 session handler，新增 chat handler） |
| `src/main.rs` | 注入依赖 + 启动 Scheduler/Worker |
| `src/bootstrap/repos.rs` | 新增 repo |
| `src/bootstrap/state.rs` | ServiceGraph 扩展 |
| `src/shared/config.rs` | 新增配置段 |

---

## 16. 实施计划

| 阶段 | 内容 | 工时 |
|---|---|---|
| Phase 1 | 数据库 DDL + 迁移脚本 | 0.5d |
| Phase 2 | Domain 层实体 + Repository trait | 1.5d |
| Phase 3 | Infrastructure 层 SeaORM 实现 | 1.5d |
| Phase 4 | Memory 增强（三段去重 + evidence 表 + merge classifier） | 1.5d |
| Phase 5 | Persona Engine + Scheduler + 校验 | 2d |
| Phase 6 | Safety Context Updater + Safety Observations | 1d |
| Phase 8 | context_version + Session 重构 + system prompt 分层 | 1.5d |
| Phase 9 | API 重构（Handler/DTO/Router + 用户控制端点 + 查询端点） | 1.5d |
| Phase 10 | 并发控制（per-user mutex） | 0.5d |
| Phase 11 | 集成测试 | 1.5d |
| **合计** | | **13.5d** |

---

## 17. 风险与缓解

| 风险 | 缓解 |
|---|---|
| LLM 输出不符合 JSON Schema | 严格校验 + 失败标记 error + 下次调度重试 |
| Persona 生成中的幻觉 evidence | 业务校验：所有 evidence ID 必须属于输入集合 |
| 并发 active snapshot | `active_user_id` GENERATED 列 + UNIQUE 约束 + per-user mutex |
| clear 后向量未清理 | vector_index_jobs (action='delete') 异步重试 + 监控 |
| 上下文版本检查成为性能瓶颈 | user_context_versions 是主键查询；仅在发消息时检查 |
| 三段去重 LLM 调用成本 | memory_key 精确匹配为第一道过滤器；仅候选相似 memory 才进入 LLM 判断 |
| 单会话消息量过大 | 游标分页 + summary 优先 + 可选的按月归档 |
