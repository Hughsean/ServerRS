# ServerRS 项目移交文档

## 项目概述

将 Java Spring Boot 项目（数字人陪伴系统）逐步迁移到 Rust Clean Architecture。
Java 项目路径：`Server/`，Rust 项目路径：`ServerRS/`。

## 技术栈

| 维度 | 选择 |
|------|------|
| Web 框架 | Axum 0.8 |
| 异步运行时 | Tokio |
| 数据库 | MySQL + SeaORM 1.1（自动 CRUD） |
| 认证 | JWT (HS256) + bcrypt |
| LLM | Ollama (OpenAI 兼容 API) + fallback 文本 |
| 配置 | TOML 文件 |

## 架构

```
Clean Architecture 四层：

Domain      → 纯 trait + 实体（零依赖）
Application → 聚合 Service（AuthService / UserService / SessionService / SessionManager）
Infrastructure → SeaORM repo / Ollama / JWT / 规则检测器
API         → axum handlers + DTO + middleware

依赖方向：API → Application → Domain ← Infrastructure
```

### ApiState（DI 容器）

```rust
pub struct ApiState {
    pub auth: Arc<AuthService>,        // login/register/logout/refresh/verify
    pub user: Arc<UserService>,        // CRUD + profile
    pub session: Arc<SessionManager>,  // LLM 会话生命周期
    pub query: Arc<SessionService>,    // 对话列表 + 风险检测列表
}
```

### TaskEvent 系统

- `TaskPublisher` trait（domain 层）
- `ResilientTaskPublisher`（infrastructure 层，try_send 非阻塞）
- `TaskHandler` trait（可插拔处理器）
- `LoggingHandler`（默认实现：所有事件打日志）
- 8 种事件：LoginAudit / UserRegistered / RefreshTokenRevoked / RefreshTokenRotated / SessionCreated / SessionExpired / ConversationCreated / RiskDetected

## 项目文件清单（~80 个 .rs 文件）

```
src/
├── main.rs                     # DI 装配（~130 行）
├── lib.rs                      # 库入口（供测试）
├── shared/
│   ├── config.rs               # TOML 配置
│   └── error.rs                # AppError 统一错误类型
├── domain/
│   ├── auth/                   # 6 个 trait（password/token/refresh）
│   ├── user/                   # User / UserProfile 实体 + 2 个 trait
│   ├── conversation/           # Conversation / Message 实体 + 1 个 trait
│   ├── risk/                   # DetectionResult / RiskDetectionResult + 1 个 trait
│   └── tasks/                  # TaskEvent / TaskPublisher / TaskHandler
├── application/
│   ├── auth/auth_service.rs    # 统一认证服务（5→1）
│   ├── user/user_service.rs    # 用户 + 画像 CRUD（5→1）
│   └── session/
│       ├── session_manager.rs  # LLM 会话生命周期（~390 行）
│       ├── session_service.rs  # 对话 + 风险查询（2→1）
│       └── risk_detection_service.rs
├── infrastructure/
│   ├── auth/                   # bcrypt / JWT
│   ├── llm/                    # OllamaClient + PromptProvider
│   ├── detector/               # RuleBasedRiskDetector（关键词匹配）
│   ├── tasks/                  # ResilientTaskPublisher + TaskWorker
│   └── persistence/
│       ├── database.rs         # SeaORM 连接 + 自动建表
│       ├── entities/           # SeaORM 实体（user/user_profile/conversation/conversation_message/risk_detection_result）
│       └── seaorm_*_repository.rs  # 4 个 repository 实现
└── api/
    ├── router.rs               # axum 路由
    ├── mod.rs                  # ApiState
    ├── response.rs             # ApiResponse<T>
    ├── dto/                    # auth_dto / user_dto / session_dto / risk_dto
    ├── handlers/               # auth / user / session
    └── middleware/              # Bearer token 认证
```

## API 端点

| 方法 | 路径 | 认证 | 说明 |
|------|------|:---:|------|
| GET | /health | - | 健康检查 |
| POST | /api/v1/auth/register | - | 注册 |
| POST | /api/v1/auth/login | - | 登录 |
| POST | /api/v1/auth/refresh | - | 刷新 token |
| POST | /api/v1/auth/logout | - | 登出 |
| GET | /api/v1/users | Bearer | 用户列表 |
| GET | /api/v1/users/{id} | Bearer | 用户画像 |
| PUT | /api/v1/users/{id} | Bearer | 更新用户 |
| DELETE | /api/v1/users/{id} | Bearer | 删除用户 |
| PUT | /api/v1/users/{id}/profile | Bearer | 更新画像 |
| GET | /api/v1/conversations/{user_id} | Bearer | 对话列表 |
| GET | /api/v1/conversations/{user_id}/{conv_id} | Bearer | 对话消息 |
| POST | /api/v1/llm/sessions | Bearer | 创建会话 |
| POST | /api/v1/llm/sessions/{id}/messages | Bearer | 发送消息 |
| GET | /api/v1/llm/sessions/{id} | Bearer | 会话状态 |
| GET | /api/v1/risk-detections | Bearer | 风险记录 |

## 测试

```
cargo test  # 7 个集成测试全部通过（无需 MySQL/Ollama）
```

测试使用 Mock Repository + Ollama fallback。

## 配置（config.toml）

```toml
[server]
host = "0.0.0.0"
port = 8080

[database]
url = "mysql://root:passwd@127.0.0.1:3306/digital_companion"
max_connections = 10

[jwt]
secret = "change-me-in-production"
expiration_secs = 86400

# Phase 5/6 会用到：
# [ollama], [detector], [session]
```

## 迁移进度

| 阶段 | 内容 | Java 文件参考 | 状态 |
|:---|------|------|:---:|
| Phase 1 | 认证授权 | SecurityController, JwtService, UserService(login/register) | ✅ |
| Phase 2 | 数据库 + 用户管理 | UserController, UserProfileController | ✅ |
| Phase 3 | 会话 + LLM + 对话 | SessionController, SessionManager, OllamaClient, plugins | ✅ |
| Phase 4 | 风险检测 | RiskDetectionController, RuleBasedRiskDetector, LlmRiskDetector | ✅ |
| Phase 5 | 心理知识 + 抑郁评估 | PsychologyKnowledgeController, DepressionAssessmentController | ⬜ |
| Phase 6 | 社区 + 日记 + 音乐 + 后台 | CommunityController, UserDiaryController, MusicController, AdminController | ⬜ |

## 待完成（Phase 5 + 6）

### Phase 5：心理知识库
- `PsychologyCategory` 实体 + SeaORM + repository
- `PsychologyArticle` 实体 + SeaORM + repository
- `PsychologyQna` 实体 + SeaORM + repository
- `PsychologyResource` 实体 + SeaORM + repository
- `PsychologyService`（聚合 CRUD）
- API endpoints：categories / articles / qna / resources
- `DepressionScale` + `DepressionAssessment` 实体
- 抑郁评估 API

### Phase 6：社区 + 日记 + 音乐 + 后台
- `CommunityPost` / `CommunityComment` / `CommunityPostMedia`
- `UserDiary`
- `Music`
- `AdminController`（管理后台）
- `UserKnowledgeFavorite`（收藏）

## 关键设计决策

1. **不使用 proc macro**：保持显式代码，避免编译期黑盒
2. **聚合 Service 替代细小 Use Case**：5 个 auth use case → 1 个 AuthService
3. **TaskEvent 统一事件总线**：所有异步旁路操作走同一 channel
4. **Ollama fallback**：`chat()` 失败时返回固定文本，不阻塞用户
5. **Mock Repository 测试**：集成测试无需 MySQL
