# ServerRS 启动装配结构

> 最后核对: 2026-07-02
> 代码基准: 当前工作区 `src/main.rs` + `src/bootstrap/*`

## 概述

ServerRS 现在采用显式的 6 阶段启动流水线：

```text
main.rs
  └─ bootstrap::runtime::run(config)
       ├─ 1. InfraContext    SSH 隧道、MySQL、LLM Provider
       ├─ 2. RepoGraph       SeaORM 仓库集合
       ├─ 3. TaskContext     后台任务、任务发布器、清理循环
       ├─ 4. VectorContext   Embedding、Qdrant、VectorIndex
       ├─ 5. ServiceGraph    provider-based 业务服务、Agent、QQ Bot、Web Ingestion
       └─ 6. HTTP Serve      Axum Router、CORS、静态 TTS、优雅关闭
```

`main.rs` 只负责加载配置、初始化日志、调用 `bootstrap::runtime::run`。日志同时写 stdout 和 `logs/app.log.YYYY-MM-DD`。

## 启动流程

### 1. `bootstrap::infra`

代码: `src/bootstrap/infra.rs`

职责：
- 按配置启动 SSH tunnel。
- 初始化 MySQL 连接池。
- 创建聊天用 `OllamaProvider`，类型是 `Arc<dyn LlmProvider>`。

产物：

```rust
pub struct InfraContext {
    pub _ssh_manager: Option<SshTunnelManager>,
    pub db: DatabaseConnection,
    pub ollama_provider: Arc<dyn LlmProvider>,
}
```

注意：旧的 `OllamaClient` 主链路已经不再作为业务服务依赖使用；`DiaryService`、`MemoryExtractor`、`AgentRuntime` 都使用新的 `LlmProvider` 抽象。

### 2. `bootstrap::repos`

代码: `src/bootstrap/repos.rs`

职责：集中构造 DB repository。`build_repos` 需要传入 Qdrant 的 memory/summary collection 名称，用于忘记/清空上下文时同步删除对应向量。

包含的仓库：

```text
user_repo, profile_repo, context_version_repo, context_control_repo,
conv_repo, risk_repo, psychology_repo, depression_repo, diary_repo,
music_repo, community_repo, agent_event_repo, stored_object_repo,
rag_repo, memory_repo, summary_repo
```

### 3. `bootstrap::tasks`

代码: `src/bootstrap/tasks.rs`

职责：
- 创建 `TaskContext`。
- 创建任务发布器。
- 注册告警、限流等后台 handler。
- 保存所有后台 `JoinHandle`，服务关闭时统一 `abort_all()`。

`runtime.rs` 还会额外注册 refresh token 定期清理任务。

### 4. `bootstrap::vector`

代码: `src/bootstrap/vector.rs`

职责：
- 创建 `OllamaEmbeddingProvider`。
- 当 `[qdrant].enabled=true` 时创建 `QdrantVectorStore`。
- 创建 `VectorIndexService`。
- 启动时调用 `ensure_collections()`，确保 RAG、记忆、摘要三个 collection 存在且维度正确。

Embedding 请求会把 `[embedding].dimension` 作为 `dimensions` 字段传给 Ollama/OpenAI-compatible `/embeddings`。如果已有 Qdrant collection 维度和配置不一致，启动会失败，需要换 collection 名称或重建 collection。

### 5. `bootstrap::state` + `bootstrap::graph`

代码: `src/bootstrap/state.rs`

职责：构造 `ServiceGraph`，然后转换为 Axum `AppState`。`ServiceGraph::build` 保留服务图的显式编排；具体服务族的构造下沉到 `src/bootstrap/graph/*_provider.rs`，避免单个函数承载所有 `Arc::clone` 和 `Service::new` 细节。

当前 provider：

```text
graph::identity_provider Auth/User/TokenService 装配
graph::risk_provider     风险子系统总编排
graph::risk_detection_provider 风险检测服务
graph::risk_audit_provider 后置风险审计 handler
graph::rag_provider      RAG 子系统总编排
graph::rag_retrieval_provider RetrievalService
graph::rag_ingestion_provider IngestionService
graph::memory_provider   Memory 子系统总编排
graph::memory_extractor_provider MemoryExtractor
graph::memory_service_provider MemoryService
graph::summary_provider  Summary 子系统总编排
graph::summary_service_provider SummaryService
graph::summary_handler_provider SummaryRefreshHandler
graph::agent_provider    Agent 子系统总编排
graph::agent_context_provider AgentContextBuilder、Fresh Retrieval、Context Routing
graph::agent_tool_provider Agent tools 注册
graph::agent_runtime_provider AgentRuntime 和 settings
graph::session_provider  SessionService 和 ChatService
graph::domain_provider   领域服务总编排
graph::object_provider   ObjectService 和本地对象存储
graph::wellbeing_provider Psychology/Depression/Diary 服务
graph::content_provider  Music/Community 服务
graph::integration_provider 集成子系统总编排
graph::qq_bot_provider QQ Bot 启动和后台任务注册（feature `qq_bot`）
graph::web_ingestion_provider Web Ingestion 启动和 KnowledgeReviewService
graph::fresh_context_provider Fresh Context 启动和后台任务注册
```

认证图 `AuthGraph` 由 `runtime.rs` 构造一次，同时供 refresh token 清理任务和 `ServiceGraph::build` 使用；不要在服务图内部再次调用 `build_auth`。

主要服务：

```text
AuthService, UserService, ChatService, SessionService,
PsychologyService, DepressionService, DiaryService, MusicService,
CommunityService, ObjectService,
RetrievalService, IngestionService, MemoryService,
AgentRuntime, KnowledgeReviewService
```

`ServiceGraph::build` 只保留服务图拓扑：
- 各业务服务族由 `graph::*_provider` 构造。
- 后台 handler 由对应 provider 返回，并在 `TaskContext` 上集中注册。
- QQ Bot、Web Ingestion、Fresh Context 通过 `integration_provider` 统一启动。
- QQ Bot 的 NapCat/注意力实现不得泄漏到 `app::qq_bot`：应用层只依赖
  `domain::qq_bot::{AttentionStore, GroupMessageGateway, GroupMessageHandler}` 端口，
  具体实现由 `bootstrap::qq_bot` 装配。

### 6. `bootstrap::runtime`

代码: `src/bootstrap/runtime.rs`

职责：
- 按 6 阶段编排启动。
- 构造 Axum router。
- 挂载 `/tts` 静态目录（仅 QQ Bot + TTS 配置可用时）。
- 监听 Ctrl+C / SIGTERM。
- 关闭时停止后台任务并关闭 SSH tunnel。

## Axum State

`AppState` 位于 `src/api/state.rs`，通过 `#[derive(FromRef)]` 拆成子 State。Handler 只拿自己需要的状态，不直接依赖完整 object graph。

当前子 State：

| State | 主要字段 | 使用者 |
|---|---|---|
| `AuthState` | `Arc<AuthService>` | auth handler / auth middleware |
| `UserState` | `Arc<UserService>` | user handler |
| `ChatState` | `Arc<ChatService>`, `Arc<dyn ConversationRepoT>` | chat handler |
| `ObjectState` | `Arc<ObjectService>` | object handler |
| `PsychologyState` | `Arc<PsychologyService>` | psychology handler |
| `DepressionState` | `Arc<DepressionService>` | depression handler |
| `DiaryState` | `Arc<DiaryService>` | diary handler |
| `MusicState` | `Arc<MusicService>` | music handler |
| `CommunityState` | `Arc<CommunityService>` | community handler |
| `AdminState` | user/query/review/music/risk | admin/stats/review handlers |
| `InternalState` | retrieval/ingestion/memory/agent_runtime | 内部保留 |
| `SignatureState` | `Arc<dyn TokenServiceT>` | signature handler |

## Agent 工具注册

代码: `src/app/agent/tool_registry.rs`

默认工具由 `build_default_agent_tools(&AgentToolDeps, agent_enabled)` 构造，当前包括：

| key | 作用 |
|---|---|
| `knowledge_search` | RAG 知识库检索 |
| `memory_search` | 用户长期记忆检索 |
| `diary_search` | 日记检索 |
| `depression_scale` | 抑郁评估记录查询 |
| `music_recommend` | 音乐推荐 |
| `community_search` | 社区内容搜索 |
| `get_time` | 时间查询 |
| `fetch_web_content` | 网页抓取工具 |
| `get_baidu_baike` | 百度百科查询 |
| `get_weather` | 天气查询 |

工具注册顺序显式、key 唯一、tool name 唯一。`main.rs` 不直接依赖具体工具类型。

## 主链路上下文

聊天主入口是 `ChatService::send_message`：

1. 按用户加锁。
2. 找到或创建用户唯一 conversation。
3. 从数据库加载最近历史消息。
4. 调用 `AgentRuntime::respond`。
5. `AgentRuntime` 追加当前用户消息并按 `[agent].max_context_messages` 截断。
6. 构建摘要、记忆、RAG、用户画像上下文。
7. 调 LLM 和工具。
8. 原子保存用户消息和助手回复。
9. 发布 `TurnClosedEvent`，触发后置摘要/风险审计。
10. 异步执行当前轮次的记忆提取。

记忆提取只看当前用户消息 + 当前助手回复；长期记忆/画像/RAG 是主对话上下文，不会再喂给记忆提取器，避免污染。

## 禁止事项

以下模式不要引回项目：

1. Handler 直接拿 `State<AppState>`。
2. 全局 service container / `OnceLock` / `lazy_static` DI。
3. 在 `main.rs` 重新堆业务构造代码。
4. 让 app 层直接依赖 SeaORM 实体。
5. 绕过 `bootstrap::runtime` 启动后台 worker。

## 架构防回退检查

代码库根目录的 `build.rs` 会在 `cargo check` / `cargo test` / `cargo build` 阶段运行 `build_support::architecture_guard`。这是轻量文本扫描，不做 AST 分析，目标是强制当前分层方向：

```text
shared   -> 只能依赖 shared/标准库/第三方库
domain   -> shared
app      -> domain/shared/app
infra    -> domain/shared/infra
api      -> api/app/domain/shared
bootstrap -> api/app/domain/infra/shared/bootstrap
```

额外禁止：
- `api` 依赖 `bootstrap` 或 `infra`。
- handler 直接提取 `State<AppState>`。
- `AppState` 包装 `ServiceGraph`。
- `bootstrap/state.rs` 直接 `Arc::new` / `Service::new` 构造业务服务。
- `bootstrap/graph` 子 provider 对外 `pub mod` 或 `pub use`。
- `api/app/domain/shared` 引入数据库基础设施类型。
- `OnceLock` / `lazy_static` 风格的全局 service container。

`#[cfg(test)]` 测试模块会被忽略，允许 app 层测试使用 infra mock。`qq_bot` 相关源码在默认 feature 下不扫描；启用 `qq_bot` feature 时会参与检查。
