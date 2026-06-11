# ServerRS 启动装配结构 (Bootstrap Architecture)

> 最后更新: 2026-06-12  
> 适用于当前 `Main` 分支重构后状态

## 目录

1. [概述](#概述)
2. [启动流程](#启动流程)
3. [Bootstrap 模块](#bootstrap-模块)
4. [Axum State 体系](#axum-state-体系)
5. [Handler → State 映射表](#handler--state-映射表)
6. [ServiceGraph 结构](#servicegraph-结构)
7. [禁止事项](#禁止事项)

---

## 概述

ServerRS 采用 **Eager Object Graph** 模式：

- 启动时一次性构建所有实例（repository → service → state）。
- 通过 `Arc<T>` 共享所有权。
- Axum handler 使用 `#[derive(FromRef)]` 只拿自己需要的子 State。
- 不使用 Spring 式 DI 容器、`OnceLock`、`OnceCell`、全局 static。

```
main.rs
  │
  ├─ config ───────────────────────── AppConfig::load()
  ├─ db ──────────────────────────── init_db(&config.database.url)
  │
  ├─ bootstrap::repos::build_repos ── RepoGraph (14 个 SeaORM repository)
  ├─ bootstrap::tasks::BackgroundTasks ─ 后台任务句柄管理
  ├─ bootstrap::auth::build_auth ──── AuthGraph (password / jwt / auth_service)
  │
  ├─ (LLM / Qdrant / RiskDetector ── 仍直接在 main.rs 构造)
  ├─ (各 Service ────────────────── 仍直接在 main.rs 构造)
  │
  ├─ bootstrap::state::ServiceGraph ─ 聚合所有 Arc<Service>
  ├─ bootstrap::state::build_state ── AppState (FromRef)
  └─ api::router::build_router ────── axum::Router
```

---

## 启动流程

`main.rs::run()` 的执行顺序：

### 1. 基础设施

```rust
let config = AppConfig::load();
let db = init_db(&config.database.url).await?;
```

### 2. Repository (RepoGraph)

```rust
let repos = bootstrap::repos::build_repos(&db);
// 返回 RepoGraph { user_repo, profile_repo, conv_repo, risk_repo, ... }
// 随后通过 Arc::clone 提取局部变量作为兼容层
let user_repo = Arc::clone(&repos.user_repo);
let profile_repo = Arc::clone(&repos.profile_repo);
// ... 共 14 个
```

所有 SeaORM concrete repository 构造集中在 `bootstrap/repos.rs`，`main.rs` 不再直接 import SeaORM repository 类型（除 `SeaOrmRefreshTokenStore` 和 `SeaOrmVectorIndexRepository` 两个特殊构造）。

### 3. 后台任务 (BackgroundTasks)

```rust
let mut background = bootstrap::tasks::BackgroundTasks::new();
// 后续用 background.spawn(tokio::spawn(...)) 注册：
//   - task worker (tw)
//   - alert_handler cleanup interval
//   - rate_limit_handler cleanup interval
//   - session cleanup interval
// 在 shutdown 时调用 background.abort_all()
```

### 4. Auth 装配 (AuthGraph)

```rust
let auth_graph = bootstrap::auth::build_auth(
    &db, &config.jwt, &config.auth, &user_repo, &task_publisher
);
// 内部构造: BcryptPasswordHasher → JwtTokenService → SeaOrmRefreshTokenStore → AuthService
// 返回 AuthGraph { auth_service }
let auth = Arc::clone(&auth_graph.auth_service);
```

### 5. LLM / Qdrant / RiskDetector / 其余 Service

这些目前仍在 `main.rs` 内直接构造（未被拆分到 bootstrap 子模块），包括：
- `OllamaClient`, `OllamaProvider`, `OllamaEmbeddingProvider`, `PromptProvider`
- `QdrantVectorStore` (optional, `#[cfg(feature = "qdrant")]`)
- `VectorIndexService`, `RuleBasedRiskDetector`
- `UserService`, `SessionService`, `ConversationOrchestrator`
- `RetrievalService`, `IngestionService`, `MemoryService`, `SummaryService`
- `AgentRuntime`, `SessionManager`
- `PsychologyService`, `DepressionService`, `DiaryService`, `MusicService`, `CommunityService`, `ObjectService`

### 6. ServiceGraph 聚合

```rust
let services = bootstrap::state::ServiceGraph {
    auth, user, session, query, objects,
    psychology, depression, diaries, music, community,
    retrieval, ingestion, memory: memory_svc, agent_runtime,
};
```

### 7. AppState 构建 & Router

```rust
let state = bootstrap::state::build_state(&services);
// 返回 AppState，其字段为各个子 State
let app = api::router::build_router(state);
// 路由内部通过 FromRef 自动提取子 State
```

### 8. 启动 & 优雅关闭

```rust
let listener = tokio::net::TcpListener::bind(&addr).await?;
axum::serve(listener, app)
    .with_graceful_shutdown(shutdown_signal())
    .await?;
background.abort_all();  // 统一 abort 所有后台任务
```

---

## Bootstrap 模块

```
src/bootstrap/
├── mod.rs          # pub mod auth; pub mod repos; pub mod state; pub mod tasks;
├── auth.rs         # AuthGraph + build_auth()
├── repos.rs        # RepoGraph + build_repos()
├── state.rs        # ServiceGraph + build_state()
└── tasks.rs        # BackgroundTasks (spawn / abort_all)
```

### bootstrap::repos

| 结构体 | 用途 |
|--------|------|
| `RepoGraph` | 持有 14 个 `Arc<dyn Trait>` repository |
| `build_repos(db)` | 一次性构造所有 SeaORM repository |

包含的 repository:
`user_repo`, `profile_repo`, `conv_repo`, `risk_repo`, `psychology_repo`,
`depression_repo`, `diary_repo`, `music_repo`, `community_repo`,
`agent_event_repo`, `stored_object_repo`, `rag_repo`, `memory_repo`, `summary_repo`

### bootstrap::auth

| 结构体 | 用途 |
|--------|------|
| `AuthGraph` | 持有 `auth_service: Arc<AuthService>` |
| `build_auth(db, jwt_cfg, auth_cfg, user_repo, task_pub)` | 构造 password service → JWT → refresh token store → AuthService |

内部构造链：
```
BcryptPasswordHasher → Arc<dyn PasswordService>
JwtTokenService      → Arc<JwtTokenService> → as Arc<dyn TokenService>
SeaOrmRefreshTokenStore → as Arc<dyn RefreshTokenStore>
AuthService::new(user_repo, password_service, jwt, refresh_store, task_publisher, config)
```

### bootstrap::state

| 结构体 | 用途 |
|--------|------|
| `ServiceGraph` | 聚合所有 `Arc<Service>`（14 个字段） |
| `build_state(services)` | 构造 `AppState`，将 ServiceGraph 拆分为子 State |

**ServiceGraph 字段:**
`auth`, `user`, `session`, `query`, `objects`, `psychology`, `depression`,
`diaries`, `music`, `community`, `retrieval`, `ingestion`, `memory`, `agent_runtime`

### bootstrap::tasks

| 结构体 | 用途 |
|--------|------|
| `BackgroundTasks` | 封装 `Vec<JoinHandle<()>>` |
| `new()` / `default()` | 创建空集合 |
| `spawn(handle)` | 注册一个后台任务句柄 |
| `abort_all(self)` | 在 shutdown 时统一 abort 所有任务 |

---

## Axum State 体系

### AppState (总根)

```rust
#[derive(Clone, FromRef)]
pub struct AppState {
    pub auth: AuthState,
    pub user: UserState,
    pub session: SessionState,
    pub object: ObjectState,
    pub psychology: PsychologyState,
    pub depression: DepressionState,
    pub diary: DiaryState,
    pub music: MusicState,
    pub community: CommunityState,
    pub admin: AdminState,
    pub internal: InternalState,
}
```

### 子 State 定义

| State | 包含字段 | 使用者 |
|-------|---------|--------|
| `AuthState` | `auth: Arc<AuthService>` | auth_handler, auth_middleware |
| `UserState` | `user: Arc<UserService>` | user_handler |
| `SessionState` | `session: Arc<SessionManager>`, `query: Arc<SessionService>` | session_handler |
| `ObjectState` | `objects: Arc<ObjectService>` | object_handler |
| `PsychologyState` | `psychology: Arc<PsychologyService>` | psychology_handler |
| `DepressionState` | `depression: Arc<DepressionService>` | depression_handler |
| `DiaryState` | `diaries: Arc<DiaryService>` | diary_handler |
| `MusicState` | `music: Arc<MusicService>` | music_handler |
| `CommunityState` | `community: Arc<CommunityService>` | community_handler |
| `AdminState` | `user: Arc<UserService>`, `query: Arc<SessionService>` | admin_handler |
| `InternalState` | `retrieval`, `ingestion`, `memory`, `agent_runtime` | (保留，未被 handler 直接使用) |

### FromRef 机制

`AppState` 通过 `#[derive(FromRef)]` 实现 axum 的 `FromRef` trait。
当 handler 声明 `State(state): State<AuthState>` 时，axum 自动从 `AppState` 中提取 `AppState.auth` 字段。

Router 只保存一份 `AppState`：
```rust
Router::new()
    .with_state(state)  // state: AppState
```

---

## Handler → State 映射表

| Handler 文件 | `State<X>` | 调用 `state.xxx` |
|---|---|---|
| `auth_handler.rs` | `AuthState` | `state.auth.register/login/refresh/logout/verify` |
| `user_handler.rs` | `UserState` | `state.user.update_user/delete_user/get_profile/upsert_profile/list_users` |
| `session_handler.rs` | `SessionState` | `state.session.create/process_message/status`, `state.query.list_conversations/list_messages/list_risk_detections` |
| `object_handler.rs` | `ObjectState` | `state.objects.upload/get_bytes/get_metadata/delete` |
| `psychology_handler.rs` | `PsychologyState` | `state.psychology.list_categories/list_articles/toggle_favorite/...` |
| `depression_handler.rs` | `DepressionState` | `state.depression.list_scales/list_assessments/create_assessment/...` |
| `diary_handler.rs` | `DiaryState` | `state.diaries.list/get/create/update/delete` |
| `music_handler.rs` | `MusicState` | `state.music.list_tracks/get_track/stream_track/admin_create/...` |
| `community_handler.rs` | `CommunityState` | `state.community.list_posts/create_post/delete_post/like_post/...` |
| `admin_handler.rs` | `AdminState` | `state.user.list_users/admin_get_user/...`, `state.query.admin_list_risk_conversations/...` |
| `auth_middleware.rs` | `AuthState` | `state.auth.verify(token)` |

**注意:** handler 不直接取 `State<AppState>`。每个 handler 只拿本模块的子 State。

---

## ServiceGraph 结构

`ServiceGraph` 是启动期持有所有 service `Arc` 的中间结构，仅用于传递给 `build_state`：

```rust
#[derive(Clone)]
pub struct ServiceGraph {
    pub auth: Arc<AuthService>,
    pub user: Arc<UserService>,
    pub session: Arc<SessionManager>,
    pub query: Arc<SessionService>,
    pub objects: Arc<ObjectService>,
    pub psychology: Arc<PsychologyService>,
    pub depression: Arc<DepressionService>,
    pub diaries: Arc<DiaryService>,
    pub music: Arc<MusicService>,
    pub community: Arc<CommunityService>,
    pub retrieval: Arc<RetrievalService>,
    pub ingestion: Arc<IngestionService>,
    pub memory: Arc<MemoryService>,
    pub agent_runtime: Arc<AgentRuntime>,
}
```

---

---

## Agent Tool Registry

当前 Agent 工具不再在 `main.rs` 中直接手写 `vec![...]` 构造，而是通过：

- `application::agent::tool_registry::AgentToolDeps` — 工具依赖集合
- `application::agent::tool_registry::build_default_agent_tools` — 集中构建

### 使用方式

```rust
let tool_deps = AgentToolDeps {
    retrieval: Arc::clone(&retrieval),
    memory: Arc::clone(&memory_svc),
    diary_repo: Arc::clone(&diary_repo),
    depression_repo: Arc::clone(&depression_repo),
    music_repo: Arc::clone(&music_repo),
    community_repo: Arc::clone(&community_repo),
    agent_event_repo: Arc::clone(&agent_event_repo),
};

let agent_tools = build_default_agent_tools(&tool_deps)?;
```

### 默认工具顺序

| 序号 | key | Tool | 依赖 |
|------|-----|------|------|
| 1 | `knowledge_search` | KnowledgeSearchTool | `retrieval: Arc<RetrievalService>` |
| 2 | `memory_search` | MemorySearchTool | `memory: Arc<MemoryService>` |
| 3 | `diary_search` | DiarySearchTool | `diary_repo: Arc<dyn DiaryRepository>` |
| 4 | `depression_scale` | DepressionScaleTool | `depression_repo: Arc<dyn DepressionRepository>` |
| 5 | `music_recommend` | MusicRecommendTool | `music_repo: Arc<dyn MusicRepository>` |
| 6 | `community_search` | CommunitySearchTool | `community_repo: Arc<dyn CommunityRepository>` |
| 7 | `risk_escalation` | RiskEscalationTool | `agent_event_repo: Arc<dyn AgentEventRepository>` |

### 设计原则

- 工具注册顺序必须显式（通过 `order` 字段 + `sort_by_key`）。
- 工具 key 必须唯一（`validate_registration_keys` 校验）。
- `tool.name()` 必须唯一（`validate_tool_names` 校验）。
- `main.rs` 不直接依赖具体 tool 类型（只 import `AgentToolDeps` 和 `build_default_agent_tools`）。
- 本阶段不使用 `inventory`，避免分布式注册导致调试困难。
- 后续若工具数量超过 10 个，再考虑 inventory v2。
- Agent tools 通过 `src/application/agent/tool_registry.rs` 注册。
- 首个从 Java 插件迁移到 Rust AgentTool 系统的工具是 `get_time`。
- 插件配置通过 `config.toml` 的 `[plugins.*]` 段和 `src/shared/config.rs` 表示。

---

## 禁止事项（备忘）

以下模式在本项目中**不得引入**：

1. ❌ `State<AppState>` 在 handler 中 — handler 只拿子 State
2. ❌ `Extension<Arc<ApiState>>` — 已删除，不再使用
3. ❌ `OnceLock` / `OnceCell` 做核心 service 懒加载
4. ❌ `lazy_static` / `shaku` / `nject` / `inventory` 等 DI 框架
5. ❌ 全局 static container
6. ❌ `ApiState` 类型 — 已删除，不可恢复
7. ❌ handler 通过 provider 动态 resolve service
8. ❌ service 全部改成 trait（仅在 domain 层使用 trait abstraction）
9. ❌ 宏隐藏 route 定义

---

## 未来拆分评估（2026-06-12）

以下是对 `bootstrap/auth.rs`、`bootstrap/llm.rs`、`bootstrap/agent.rs` 进一步拆分的评估。

### 当前 main.rs 剩余内容（约 200 行）

| 区块 | 行数 | 内容 |
|------|------|------|
| LLM infra | ~25 | `OllamaClient`, `OllamaProvider`, `OllamaEmbeddingProvider`, `PromptProvider` |
| Qdrant | ~30 | `VectorStore` (optional), `VectorIndexService`, `ensure_collections` |
| Risk | ~2 | `RuleBasedRiskDetector` |
| Services | ~60 | `UserService`, `SessionService`, `ConversationOrchestrator`, `RetrievalService`, `IngestionService`, `MemoryService`, `SummaryService`, `PsychologyService`, `DepressionService`, `DiaryService`, `MusicService`, `CommunityService`, `ObjectService` |
| Agent | ~30 | `AgentContextBuilder`, AgentTools (×7), `AgentRuntime` |
| Session mgr | ~10 | `SessionManager` + cleanup spawn |
| ServiceGraph | ~20 | `ServiceGraph` 聚合 + `build_state` 调用 |
| Router | ~10 | `build_router` + bind + serve + shutdown |

### 拆分可行性评估

#### `bootstrap/llm.rs` — **不建议**

- OllamaClient / OllamaProvider / EmbeddingProvider 构造简单（每个 3-4 行），拆分收益低。
- **阻塞因素**：Qdrant VectorStore 构造涉及 `#[cfg(feature = "qdrant")]` 条件编译、`async` 初始化、`?` 错误传播。将其拆入独立函数需要函数本身返回 `Result<_, std::io::Error>`，并在 main.rs 中用 `?` 传播，增加了间接层但未减少代码量。
- `VectorIndexService` 构造依赖 `rag_repo`、`memory_repo`、`summary_repo`、`vector_index_repo`、`vector_store`、`embedding_provider`，参数太多。

**结论：保持现状。** LLM 和 Qdrant 构造链留在 main.rs 中更清晰。

#### `bootstrap/agent.rs` — **不建议**

- AgentRuntime 构造依赖 10 个参数 + 7 个 AgentTool + ContextBuilder。函数签名会非常长。
- AgentTools 构造依赖 7 个不同的 repository/service（`retrieval`, `memory_svc`, `diary_repo`, `depression_repo`, `music_repo`, `community_repo`, `agent_event_repo`），拆出后 main.rs 仍需持有这些变量的 `Arc`。
- RiskDetector 构造只有 1 行。

**结论：保持现状。** 参数过多会降低可读性。

#### `bootstrap/services.rs` — **不建议**

- 包含 12 个不同 Service 的构造，其中多数只有 2-3 行。
- 这些 Service 之间有复杂依赖（如 DiaryService 依赖 OllamaClient，ConversationOrchestrator 依赖 OllamaClient + PromptProvider 等），拆出后需要这些依赖作为参数传入。
- 会导致一个巨大的 `build_services()` 函数，参数列表臃肿。

**结论：保持现状。** 当前 main.rs 中的 service 构造块注释清晰，逐一阅读无压力。

### 最终建议：不再拆分

当前 `bootstrap/` 模块结构已足够：

```
bootstrap/
├── auth.rs   ← AuthGraph (password / jwt / refresh-store / AuthService)
├── repos.rs  ← RepoGraph (14 SeaORM repositories)
├── state.rs  ← ServiceGraph + build_state (aggregation → AppState)
└── tasks.rs  ← BackgroundTasks (spawn / abort_all)
```

剩余的 ~200 行在 `main.rs` 中是**顺序化的构造代码**，每个小块都有明确的 `// ── 注释 ──` 分隔。进一步拆分将：
1. 增加参数传递的样板代码
2. 破坏 Qdrant 条件编译的局部性
3. 使依赖关系跨文件追溯变得困难

**当前结构达到了"足够瘦"的平衡点。**
