# 赛博猫猫 完整行为能力架构设计

> 日期：2026-06-16
> 状态：设计稿 v1
> 蓝图参考：`docs/human-chat-behavior-map.md`

## 一、设计原则

1. **渐进增强** — 当前 8 个 app service、10+ domain types 不动原有接口，只加新层
2. **LLM 原生驱动** — 所有"人性化"行为通过增强 LLM 上下文来实现，而非硬编码规则
3. **状态显式化** — 情绪、时间、社交关系、话题等状态显式构建为 LLM 可读的「感知上下文」
4. **可观测** — 每个状态维度的变化记录到 DB，可回溯、可调试
5. **各维度松耦合** — 时间感知不依赖情绪，情绪不依赖社交关系，可单独开关

---

## 二、新增领域类型

### 2.1 BotState — 机器人综合状态

```rust
// src/domain/qq_bot/bot_state.rs

/// 机器人的综合状态快照，在每个回复周期前构建。
/// 不持久化，仅作为 LLM 上下文的一部分。
pub struct BotState {
    // ── 情绪状态 ──
    pub emotional_state: EmotionalState,
    
    // ── 时间感知 ──
    pub temporal_context: TemporalContext,
    
    // ── 当前对话状态 ──
    pub conversation_state: ConversationState,
    
    // ── 社交关系摘要 ──
    pub relationship_context: RelationshipContext,
}

/// 情绪状态（持久化）
pub struct EmotionalState {
    // 主情绪 valence-arousal 模型
    pub mood: Mood,                          // Happy | Neutral | Sad | Angry | Surprised | Tired
    pub intensity: f32,                      // 0.0 ~ 1.0
    pub decay_started_at: Option<i64>,       // 开始衰减的时间戳
    // 本轮的触发原因（用于 LLM 提示）
    pub trigger_event: Option<String>,
}
```

### 2.2 TemporalContext — 时间感知

```rust
/// 时间上下文（每个请求动态构建，不持久化）
pub struct TemporalContext {
    pub time_of_day: TimeOfDay,             // Morning | Noon | Afternoon | Evening | Night | LateNight
    pub day_of_week: Weekday,
    pub is_weekend: bool,
    pub season: Season,                     // Spring | Summer | Autumn | Winter
    pub upcoming_holiday: Option<String>,    // 附近的节日
    pub minutes_since_last_activity: u64,   // 距离上次群消息的分钟数
    pub hours_since_bot_spoke: u64,         // 猫猫上次说话的间隔（小时）
    pub is_first_interaction_today: bool,   // 今天第一次说话
}
```

### 2.3 RelationshipState — 社交关系

```rust
// src/domain/qq_bot/relationship.rs

/// 对某个群友的关系状态（持久化）
pub struct RelationshipState {
    pub qq_group_id: i64,
    pub qq_user_id: i64,
    pub familiarity: f32,                    // 0.0(陌生人) ~ 1.0(死党)
    pub interaction_count: u32,              // 累计互动次数
    pub last_interaction_at: Option<i64>,    // 上次互动时间
    pub rapport: RapportLevel,               // Friendly | Neutral | Awkward | Playful | Respectful
    pub nickname_preference: Option<String>, // 群友喜欢的称呼
    pub known_interests: Vec<String>,        // 已知兴趣（轻量级，从对话中提取）
    pub known_avoid_topics: Vec<String>,     // 应避免的话题
}
```

### 2.4 ConversationState — 话题状态

```rust
/// 当前对话话题状态（每个回复周期构建）
pub struct ConversationState {
    pub current_topic: Option<Topic>,
    pub recent_topics: Vec<Topic>,           // 最近 3 个话题
    pub thread_id: Option<u64>,              // 当前对话线程
    pub message_count_in_thread: u32,        // 本线程的消息数
    pub self_message_count_in_thread: u32,   // 猫猫在本线程说了几句
    pub last_self_message_summary: Option<String>, // 自己上一条说的摘要（防重复）
}

pub struct Topic {
    pub label: String,                       // e.g. "美食", "游戏", "工作吐槽"
    pub confidence: f32,
    pub participants: Vec<i64>,              // 参与者的 QQ 号
}
```

### 2.5 ProactiveIntent — 主动行为意图

```rust
/// 主动发言意图（由 ProactiveEvaluator 生成）
pub struct ProactiveIntent {
    pub action_type: ProactiveAction,
    pub reasoning: String,
    pub priority: u8,                        // 0-10
    pub target_user: Option<i64>,
    pub suggested_content: Option<String>,
}

pub enum ProactiveAction {
    GreetNewcomer,       // 欢迎新人
    FillSilence,         // 冷场救急
    Bubble,              // 长时间沉默后冒泡
    GreetMorning,        // 早安
    RespondToPoke,       // 被拍后回应
    ShareInterest,       // 自发分享
    ShowConcern,         // 关心群友
}
```

---

## 三、新增 / 改造的 Service

### 3.1 EmotionalStateService（新增）

```rust
pub struct EmotionalStateService {
    repo: Arc<dyn EmotionalStateRepository>,
    // 情绪衰减规则（内置）：每 N 分钟衰减 intensity * 0.1
}
```

职责：
- 读取/写入群级别的机器人情绪状态（按 `bot_account_id + qq_group_id`）
- 触发情绪变化时更新 intensity + 设置 decay_started_at
- 每轮对话前读取当前情绪，衰减后返回
- 提供 `apply_decay()` — 自动随时间衰减

放在 `src/app/qq_bot/emotional_state_service.rs`

### 3.2 RelationshipService（新增）

```rust
pub struct RelationshipService {
    user_repo: Arc<dyn ExternalUserRepository>,
    relationship_repo: Arc<dyn RelationshipRepository>,
    message_repo: Arc<dyn GroupMessageRepository>,
}
```

职责：
- 每次收到消息时更新 `interaction_count`、`last_interaction_at`
- 计算 `familiarity` = 基于互动次数 + 时间的曲线
- 为 ContextBuilder 提供 `build_relationship_context(user_ids) → String`
- 从 LLM 回复中解析 nickname_preference、known_interests

放在 `src/app/qq_bot/relationship_service.rs`

### 3.3 ProactiveEvaluator（新增）

```rust
pub struct ProactiveEvaluator {
    attention_store: Arc<InMemoryAttentionStore>,
    relationship_service: Arc<RelationshipService>,
    message_repo: Arc<dyn GroupMessageRepository>,
    llm_provider: Arc<dyn LlmProvider>,
}
```

职责：
- 由定时器驱动（不是由消息触发），按群逐个 tick
- 判断当前群聊是否需要猫猫主动说话，按需生成回复
- 生成的主动发言走现有的 ReplyGenerator + SegmentDispatcher 管道

#### 混合判断逻辑（方案 C）

采用**规则层 + LLM 语义层**两级过滤，在省 token 和灵活性之间取得平衡。

```
tick() → 
  Layer1 规则过滤器（零 token）→ 
    通过 → Layer2 LLM 语义判断（轻量级）→ 
      需要说话 → 生成内容 → 走正常回复管道
```

**Layer1 — 规则过滤器（每个 tick 执行，零 token 开销）**

快速扫一遍群的最后几条消息，判断是否需要进入语义层：

| 规则 | 条件 | 动作 |
|------|------|------|
| 上图冷场 | 最后一条是图片/视频/链接，且 0 人回应 | ✅ 进入 Layer2 |
| 提问冷场 | 最后一条是问句，且 0 人回应 | ✅ 进入 Layer2 |
| 情绪回应 | 最后一条包含明显负面情绪词，且无人安慰 | ✅ 进入 Layer2 |
| @未回 | 有人 @猫猫但猫猫最后一条回复不是回他 | ✅ 进入 Layer2 |
| 话题中断 | 最后 3 条来自同一个人连续说，突然停了 | ✅ 进入 Layer2 |
| 纯沉默 | 以上都不满足，就是没人说话 | ❌ 跳过（除非超长沉默走下面的超时规则） |
| 超时沉默 | 距离最后一条消息 > 30 分钟，且猫咪上次说话 > 2h | ✅ 进入 Layer2（冒泡） |

规则过滤器拦截掉大约 80% 的不需要救场的 tick。只有被规则标记的场景才调用 LLM。

**Layer2 — LLM 语义判断**

每次通过规则层后，调用一次轻量级 LLM，传入：

```
系统：你是群里的猫猫。判断以下场景你是否应该说话。只需回答 yes/no 和一句话理由。

最近消息：
[用户A] 有人周末去猫咖吗？
[用户B] （图片）
[用户C] 好想去但没时间

沉默时间：3 分钟
你上次说话：12 分钟前（内容是"周末去猫咖怎么样喵~"）
```

温度 0.3，max_tokens 50，单次调用消耗极少量 token。

如果 LLM 判断需要说话，它生成的内容直接走 ReplyGenerator 管道，跳过 TriggerEvaluator（因为已经评估过了）。

**调度方式**：
- 作为 BackgroundTasks 中的一个轮询任务
- 每个活跃群每 30 秒 tick 一次，但只在「群沉默时间 ≥ N 秒」时才执行 Layer1
- N 根据群活跃度动态调整：活跃群 N=120s（2分钟不聊天才检查），冷群 N=60s

### 3.4 ContextBuilder — 改造

现有 ContextBuilder 需要增强，但不改变对外接口：

新增内部方法：
- `build_temporal_context(sent_at) → String` — 生成时间感知段落
- `build_emotional_context(state) → String` — 生成情绪状态段落
- `build_relationship_context(msg, relationships) → String` — 生成社交关系段落
- `build_conversation_context(recent_messages) → String` — 生成话题分析段落
- `build_self_awareness_context(turns) → String` — 生成自己上轮说了什么的提醒

这些全部拼接到 system prompt 的头部。

### 3.5 TriggerEvaluator — 改造

扩展 Layer 2（LLM 评估）的上下文，让它也能判断：
- 是否需要主动接话（别人在聊的话题，猫猫是否应该参与）
- 是否是合适的发言时机
- 情绪状态是否适合现在回应

同时，为 ProactiveEvaluator 的 tick 场景，增加一个不经过 TriggerEvaluator 的快捷方式（主动行为已经经过评估了，直接放行）。

### 3.6 QqBotService — 改造

- 新增 `tick_proactive()` 方法，由 BackgroundTasks 定期调用
- 改造 `handle_group_message()`，增加情绪更新步骤（收到消息后更新情绪）
- 在 context 构建阶段注入 BotState

---

## 四、数据流（完整版）

```
  ┌─────────────────────────────────────────────────────┐
  │  NapCat WebSocket 事件                              │
  │  (消息 / 拍一拍 / 群事件)                            │
  └──────────┬──────────────────────────────────────────┘
             │ NormalizedMessage
             ▼
  ┌─────────────────┐
  │ MessageIngestion │─→ 持久化消息
  └────────┬────────┘
           │
           ▼
  ┌─────────────────┐     ┌────────────────────┐
  │  ProfileBuilder  │────→│ RelationshipService │←── 更新互动计数
  └─────────────────┘     └────────────────────┘
           │
           ▼
  ┌──────────────────────────────────────────────┐
  │  TriggerEvaluator (Layer1 规则 + Layer2 LLM)  │
  │  ← 新增：情绪/时间/话题作为评估上下文           │
  └──────────┬───────────────────────────────────┘
             │ TriggerDecision::Respond
             ▼
  ┌──────────────────────────────────────────────┐
  │  BotState Builder (新)                        │
  │  ├─ EmotionalStateService → 读取+衰减情绪     │
  │  ├─ TemporalContext   → 当前时间信息          │
  │  ├─ RelationshipService → 群友关系信息        │
  │  └─ ConversationState  → 话题分析             │
  └──────────┬───────────────────────────────────┘
             ▼
  ┌─────────────────┐
  │ ContextBuilder   │──→ 增强 System Prompt
  │ (旧 + 5 个新段)  │    + 时间/情绪/关系/话题/自忆
  └────────┬────────┘
           ▼
  ┌─────────────────┐
  │ ReplyGenerator   │──→ LLM → BotReply JSON
  └────────┬────────┘
           ▼
  ┌─────────────────┐     ┌───────────────────────┐
  │  EmotionalState  │←────│ 从 BotReply 中解析     │
  │  Service (更新)   │     │ 情绪变化                │
  └─────────────────┘     └───────────────────────┘
           │
           ▼
  ┌─────────────────┐
  │ SegmentDispatcher│──→ NapCat API
  └─────────────────┘
           │
           ▼
  ┌─────────────────┐
  │ AgentTurn Recorder│
  └─────────────────┘


  ┌──────────────────────────────────────────────────────────┐
  │  后台轮询: ProactiveEvaluator（每 30s / 群 tick）        │
  │                                                          │
  │  ┌─ 快速检查：群沉默 < N 秒？ → 跳过本轮                │
  │  │                                                      │
  │  ▼ Layer1 — 规则过滤器（零 token）                       │
  │  ├─ 上图冷场？ / 提问冷场？ / 情绪未回应？                │
  │  ├─ @未回？ / 超时沉默？ / 话题中断？                     │
  │  └─ 都不满足 → 跳过（80% 的 tick 在这里被过滤）          │
  │                                                          │
  │  ▼ Layer2 — LLM 语义判断（轻量，温度 0.3，≤50 tokens）   │
  │  ├─ 收到最近消息 + 沉默时间 + 自己上次说的 + 关系状态     │
  │  ├─ LLM 回答 yes/no + 一句话理由                         │
  │  └─ yes → 生成 ProactiveIntent → 走 ReplyGenerator      │
  │                                                          │
  └──────────────────────────────────────────────────────────┘
```

---

## 五、新增存储

### 5.1 bot_emotional_states 表

| 字段 | 类型 | 说明 |
|------|------|------|
| id | u64 PK | |
| bot_account_id | u64 | |
| qq_group_id | i64 | |
| mood | varchar(32) | happy/neutral/sad/angry/surprised/tired |
| intensity | float | 0.0~1.0 |
| trigger_event | text? | 触发原因 JSON |
| decay_started_at | bigint? | 开始衰减时间戳 |
| updated_at | bigint | |

### 5.2 relationships 表

| 字段 | 类型 | 说明 |
|------|------|------|
| id | u64 PK | |
| qq_group_id | i64 | |
| qq_user_id | i64 | |
| familiarity | float | 0.0~1.0 |
| interaction_count | u32 | |
| last_interaction_at | bigint? | |
| rapport | varchar(32) | friendly/neutral/awkward/playful/respectful |
| nickname_preference | varchar(64)? | |
| known_interests | json? | ["猫", "游戏", "编程"] |
| known_avoid_topics | json? | |

### 5.3 Repository 新接口

```rust
#[async_trait]
pub trait EmotionalStateRepository: Send + Sync {
    async fn find(&self, bot_account_id: u64, qq_group_id: i64) -> Result<Option<EmotionalState>, AppError>;
    async fn upsert(&self, state: &EmotionalState) -> Result<EmotionalState, AppError>;
}

#[async_trait]
pub trait RelationshipRepository: Send + Sync {
    async fn find(&self, qq_group_id: i64, qq_user_id: i64) -> Result<Option<RelationshipState>, AppError>;
    async fn upsert(&self, rel: &RelationshipState) -> Result<RelationshipState, AppError>;
    async fn find_by_group(&self, qq_group_id: i64) -> Result<Vec<RelationshipState>, AppError>;
    async fn increment_interaction(&self, qq_group_id: i64, qq_user_id: i64) -> Result<(), AppError>;
}
```

---

## 六、LLM 上下文示例（增强后的 System Prompt 片段）

在每个回复请求的 system prompt 中，注入以下段落：

```
## 当前时间
2026-06-16 星期二 下午 14:28（午后工作时间）
距离群里上一条消息已经过去 2 分钟
你今天已经在群里说过 3 次话，上次是 12 分钟前

## 你当前的情绪
心情：开心 😊 (强度 0.7)
原因：刚才群友夸了你可爱

## 对说话对象的关系
用户[12345678]（小明）→ 熟悉度 0.85，死党级别
- 你们经常互相开玩笑
- 他喜欢猫，对香菜过敏
- 你叫他"小明"他会开心

## 当前话题
主要话题：讨论周末去哪里玩
参与者：小明、小红、大黄
你在这个话题里已经说了 1 句

## 你刚说过的话
"周末去猫咖怎么样喵~"
（不要重复同样的建议）
```

---

## 七、实施分期

### Phase 1：时间感知 + 情绪基础（2-3 天）

文件改动：
- 新增：`src/domain/qq_bot/bot_state.rs`（TemporalContext + EmotionalState）
- 新增：`src/domain/qq_bot/emotional_state_repository.rs`
- 新增：`src/app/qq_bot/emotional_state_service.rs`
- 新增：`src/infra/qq_bot/repositories/seaorm_emotional_state_repository.rs`
- 改造：`src/app/qq_bot/context_builder.rs` — 注入时间+情绪段落
- 改造：`src/app/qq_bot/qq_bot_service.rs` — 在流程中读取+更新情绪
- 改造：`src/bootstrap/qq_bot.rs` — 注入新 service
- 改造：`src/main.rs` — 初始化新 repo

### Phase 2：话题管理 + 上下文增强（3-4 天）

文件改动：
- 新增：`src/domain/qq_bot/conversation_state.rs`
- 改造：`src/app/qq_bot/context_builder.rs` — 话题分析 + 自忆段落
- 改造：`src/app/qq_bot/trigger_evaluator.rs` — 话题感知的触发判断
- 改造：`src/app/qq_bot/reply_generator.rs` — 防重复逻辑（利用 last_self_message_summary）

### Phase 3：社交关系（2-3 天）

文件改动：
- 新增：`src/domain/qq_bot/relationship.rs`
- 新增：`src/domain/qq_bot/relationship_repository.rs`
- 新增：`src/app/qq_bot/relationship_service.rs`
- 新增：`src/infra/qq_bot/repositories/seaorm_relationship_repository.rs`
- 改造：`src/app/qq_bot/context_builder.rs` — 关系段落
- 改造：`src/app/qq_bot/qq_bot_service.rs` — 每次消息更新互动计数
- 改造：`src/bootstrap/qq_bot.rs` + `src/main.rs`

### Phase 4：主动行为 + 完整整合（3-4 天）

文件改动：
- 新增：`src/domain/qq_bot/proactive.rs`
- 新增：`src/app/qq_bot/proactive_evaluator.rs`
- 改造：`src/app/qq_bot/qq_bot_service.rs` — 新增 `tick_proactive()`
- 改造：`src/app/qq_bot/trigger_evaluator.rs` — 支持跳过规则层
- 改造：`src/bootstrap/qq_bot.rs` — 注册轮询任务
- 改造：`src/main.rs`

---

## 八、不变的部分

以下文件**不需要任何改动**：
- `src/domain/qq_bot/reply.rs` — 回复段类型不变
- `src/domain/qq_bot/message.rs` — 消息结构不变
- `src/domain/qq_bot/config.rs` — 配置结构不变（新增字段不算"改"）
- `src/domain/qq_bot/persona.rs` — 人格定义不变
- `src/app/qq_bot/segment_dispatcher.rs` — 分发逻辑不变
- `src/app/qq_bot/message_ingestion.rs` — 消息摄入不变
- `src/app/qq_bot/outbox_worker.rs` — 出站 worker 不变
- `src/infra/qq_bot/napcat/api.rs` — API 客户端不变
- `src/infra/qq_bot/napcat/listener.rs` — 监听器不变

---

## 九、风险与缓解

| 风险 | 缓解 |
|------|------|
| Token 消耗增加（system prompt 变长） | 每个上下文段落控制在 3-5 行，可开关 |
| 情绪系统可能导致"精分" | 情绪衰减机制 + intensity 阈值限制突变 |
| 主动行为太频繁惹人烦 | 每个群有单独的 cooldown，主动行为受 attention_store 约束 |
| 关系数据不准 | familiarity 基于可计算的互动频率，不依赖 LLM 猜测 |
| 启动时冷启动（无历史关系/情绪） | 所有新字段有合理默认值，空状态也能工作 |

## 十、模型能力要求分析

### 核心认知：这套设计对 LLM 的要求不是变高了，而是变低了

直觉上，加入这么多维度似乎需要更强的模型。但实际情况相反——

**旧的实现：** LLM 需要从零散的聊天记录中自己领悟"现在是什么氛围""我该用什么语气""对面是谁"。这依赖模型的**隐性理解能力**。

**新的实现：** 我们把时间、情绪、关系、话题信息显式写成结构化文本喂给 LLM。LLM 只需要做**阅读理解 + JSON 格式化输出**。

### 逐项能力门槛

| 能力 | LLM 实际要做的事 | Qwen2.5:14B | 7B 级别 |
|------|-----------------|:-----------:|:-------:|
| 时间感知 | 读一行「当前时间：下午2点」→ 语气调整 | ✅ 无压力 | ✅ 无压力 |
| 情绪连贯 | 读「心情：开心(0.7)」→ 回复带 😊 | ✅ 无压力 | ✅ 无压力 |
| 话题跟随 | 读「当前话题：周末去哪」→ 别跑题 | ✅ 够用 | ✅ 够用 |
| 防重复 | 读「你刚说过：…」→ 不说第二遍 | ✅ 够用 | ✅ 够用 |
| 社交区分 | 读「小明(死党)」→ 语气随意 | ✅ 够用 | ⚠️ 可能生硬 |
| 主动判断 | 读「消息列表」+ 10 字理由 → yes/no | ✅ 够用 | ✅ 够用 |
| 多线程对话 | 理解 @ 和引用关系 | ⚠️ 能处理简单情况 | ⚠️ 吃力 |
| 省略/指代 | 猜"那个""就他"指什么 | ⚠️ 常见情况 ok | ❌ 基本不行 |

### 降级方案

对于模型能力不足的场景（7B 或指代理解），不依赖模型自己"悟"，而是在 context 中**显式消歧**：

```
[用户A] 那个周末去不去？
        └─ 注：上个话题提到了"周末去猫咖"
```

这种标注由 ContextBuilder 在组装上下文时完成，不需要 LLM 参与，只消耗很简单的规则匹配。

### 结论

**当前 Qwen2.5:14B 完全能胜任整套设计。** 即使以后换 7B 或更小的模型，核心功能不会崩，只是社交细腻度和指代理解上会差一些——而这些维度可以独立关掉，不影响其他能力。
