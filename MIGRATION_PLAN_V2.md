# ServerRS V2 迁移计划

## 状态：进行中（2026-06-11）

---

## 新 API 规范

- 前缀：`/api/v1/**`
- 认证：`Authorization: Bearer <accessToken>`
- 字段命名：`camelCase`（`#[serde(rename_all = "camelCase")]`）
- 成功响应：直接返回资源 DTO 或分页 `{ items, page, pageSize, total }`
- 错误响应：Problem Details `{ type, title, status, code, message, traceId, details }`
- 普通用户身份从 JWT 解析，不通过 path 参数传 userId

## 废弃旧 Java 设计

| 废弃项 | 替换 |
|--------|------|
| `ApiResponse<T>` | 直接返回 DTO |
| RSA 密码加密登录 | HTTPS + bcrypt |
| RSA 加密 Admin API Key | role=ADMIN JWT |
| `/api/v1/users/{user_id}` 普通用户路由 | `/api/v1/users/me` |
| MySQL BLOB 存储文件 | `ObjectStorage` + `stored_objects` |
| `message_id = 0/null` 写入风险检测 | 严格 FK 绑定 |
| 内存 refresh token 撤销 | `refresh_tokens` 数据库表 |
| `init_db()` create_table_from_entity | SeaORM migration |

## 当前 Rust 修复列表

- [x] 删除 `response.rs` ApiResponse
- [ ] AppError → Problem Details
- [ ] Config 覆盖全配置节
- [ ] Router 拆分
- [ ] ApiState 扩展
- [ ] V2 Migration 替换 init_db()
- [ ] users 表增加 role/status/avatar_object_id/password_hash
- [ ] stored_objects 表
- [x] refresh_tokens 表（entity + SeaOrmRefreshTokenStore 已完成）
- [x] content_likes 防重复（entity + SeaOrmLikeRepository 已完成）
- [ ] message_count 每条消息 +1
- [ ] messageId 严格绑定 risk_detection_results
- [x] JWT claims 增加 role

## V2 数据库表

```
users, user_profiles, refresh_tokens, stored_objects,
conversations, conversation_messages, risk_detection_results, risk_actions,
psychology_categories, psychology_articles, psychology_qna, psychology_resources,
knowledge_favorites, content_likes,
depression_scales, depression_assessments,
community_posts, community_post_media, community_comments,
community_post_likes, community_comment_likes,
user_diaries, music_tracks
```

## 模块迁移状态

---

## 数据库策略纠偏（2026-06-10）

本项目采用 **Database-first** 策略：

- 现有 MySQL 数据库 `digital_companion` 是真源
- SeaORM entity 由 `sea-orm-cli generate entity` 从真实数据库生成
- `init_db()` 只负责连接，不自动建表/改表/seed
- 不使用 `sea-orm-migration`（已删除所有 migration 文件）
- 不维护 `schema.sql` / `seed.sql` 作为当前迁移真源
- Schema 变更必须由人工在数据库侧执行，再重新生成 entity

### 已废弃的错误方向

- `sea-orm-migration` 自动建表方案（已删除 `src/infrastructure/persistence/migration/`）
- V2 schema 从空库启动
- `schema.sql` / `seed.sql` 作为迁移真源
- 默认 admin seed / PHQ-9 seed / psychology category seed
- 强制 `stored_objects` 替代现有 BLOB 表结构
- `init_db()` 中 `create_table_from_entity()`（已简化为纯连接）

### 真实数据库关键发现

| 发现 | 影响 |
|------|------|
| `users.password` (非 password_hash) | entity/domain 使用 `password` |
| `users.avatar: BLOB` | 大文件仍在 DB 中 |
| `users` 无 `role` 字段 | role-based auth 暂不可落地 |
| `conversations` 无 `title_status/updated_at/closed_at` | 保留 `is_title_generated` |
| `music.file_data: String` | 文件 BLOB 存储 |
| 无 `refresh_tokens` 表 | refresh token 持久化暂用内存实现 |
| 无 `stored_objects` 表 | ObjectStorage 暂未落地 |
| 无 `content_likes` 表 | 点赞功能待补充 |
| `depression_scales.scale_id: u16` | 使用真实字段名 `scale_name/min_score/max_score` |
| `depression_assessments.assessment_date: Date` | 使用 `total_score: i16` |
| `community_post_media.media_data: String` | BLOB 存储 |
| `psychology_*.cover_image/thumbnail: Option<Vec<u8>>` | BLOB 存储 |

### 当前接入状态（2026-06-11 更新）

| 模块 | 文件 | mod.rs | handler | router | 真实实现 | 状态 |
|------|------|--------|---------|--------|---------|------|
| auth | ✅ | ✅ | ✅ | ✅ | ✅ | 完成 |
| user/profile | ✅ | ✅ | ✅ | ✅ | ✅ (旧路由+新/me) | 基本完成 |
| conversation/session | ✅ | ✅ | ✅ | ✅ | ✅ | 完成 |
| risk | ✅ | ✅ | ✅ | ✅ | ✅ (含admin方法) | 完成 |
| object storage | ✅ | ✅ | ✅ | ❌ | ⚠️ stub | 待真实实现 |
| psychology | ✅ | ✅ | ✅ | ❌ | ⚠️ stub | 待真实实现 |
| depression | ✅ | ✅ | ✅ | ❌ | ⚠️ stub | 待真实实现 |
| community | ✅ | ✅ | ✅ | ❌ | ⚠️ stub | 待真实实现 |
| diary | ✅ | ✅ | ✅ | ❌ | ⚠️ stub | 待真实实现 |
| music | ✅ | ✅ | ✅ | ❌ | ⚠️ stub | 待真实实现 |
| admin | ✅ | ✅ | ✅ | ❌ | ⚠️ stub | 待真实实现 |
| OpenAPI | ❌ | ❌ | ❌ | ❌ | ❌ | 未开始 |
| LLM provider 抽象 | ❌ | ❌ | ❌ | ❌ | ❌ | 未开始 |
| mail | ❌ | ❌ | ❌ | ❌ | ❌ | 未开始 |

### 当前测试状态

```
cargo fmt:         ✅ 通过
cargo check --all-targets: ✅ 通过（250 warnings）
cargo test:        ✅ 通过（23 unit + 7 integration = 30 passed, 0 failed）
```

---

## 里程碑：业务模块落地（2026-06-11）

Depression / Diary / Music / Psychology / Community 五个业务模块现在拥有完整的四层架构：

| 模块 | domain trait | domain model | domain mod.rs | SeaORM repo | application service | application mod.rs | handler | router 路由 | 状态 |
|------|-------------|-------------|--------------|------------|-------------------|-------------------|---------|-----------|------|
| depression | DepressionRepository | DepressionScale, DepressionAssessment | ✅ | seaorm_depression_repository | DepressionService | ✅ | depression_handler | ✅ mounted | 完成 |
| diary | DiaryRepository | UserDiary | ✅ | seaorm_diary_repository | DiaryService | ✅ | diary_handler | ✅ mounted | 完成 |
| music | MusicRepository | MusicTrack | ✅ | seaorm_music_repository | MusicService | ✅ | music_handler | ✅ mounted | 完成 |
| psychology | PsychologyRepository | PsychologyCategory/Article/QnA/Resource | ✅ | seaorm_psychology_repository | PsychologyService | ✅ | psychology_handler | ✅ mounted | 完成 |
| community | CommunityRepository | Post/Comment/PostMedia | ✅ | seaorm_community_repository | CommunityService | ✅ | community_handler | ✅ mounted | 完成 |

### 数据库限制 —— 以下功能有 entity 但无路由

| 限制 | 原因 | 影响 |
|------|------|------|
| 点赞 endpoints 未挂载 | 数据库中无 `content_likes` 表 | `toggle_like` 等 handler 存在但未在 router 注册 |
| admin 路由未挂载 | `users` 表无 `role` 字段，无法区分管理员 | `admin_handler` 存在（包含 `list_users`, `patch_user`, `delete_user`, risk conversation 管理等）但未在 router 注册 |
| 无 `stored_objects` 表 | ObjectStorage 仍用内存 stub | `object_handler` 和 `object_service` 存在但 blob 对象管理未落地 |
| 无 `refresh_tokens` 表 | 撤销列表仍用 `InMemoryRefreshTokenRevocationRepository` | 重启后撤销记录丢失 |
| music 使用 `file_data` BLOB | 数据库中 `music.file_data` 是 `String` 类型存储文件内容，而非 `object_id` | 文件存储仍嵌在行内，不支持流式分块 |

### main.rs 变化

- **不再使用 `Stub*Repo`** 的模块：Community（`SeaOrmCommunityRepository`）
- **仍使用 `Stub*Repo`** 的模块：Depression, Diary, Music, Psychology —— 虽然 SeaORM 实现已存在，main.rs 中尚未切换过来
- **Stub 文件残留**：`src/infrastructure/persistence/implementations/stub_repositories.rs` 仍包含 `StubDepressionRepo`, `StubDiaryRepo`, `StubMusicRepo`, `StubPsychologyRepo` 四个空实现

### 测试中的 Mock

测试文件 `tests/common/mod.rs` 包含以下 **Mock** 实现（in-memory 模拟，不依赖 MySQL）：

| Mock | 模拟接口 | 用途 |
|------|---------|------|
| MockUserRepo | UserRepository | 用户 CRUD 测试 |
| MockProfileRepo | UserProfileRepository | 个人资料测试 |
| MockConvRepo | ConversationRepository | 会话测试 |
| MockRiskRepo | RiskRepository | 风险检测测试 |
| MockRevokeRepo | RefreshTokenRevocationRepository + RefreshTokenStore | Token 撤销测试 |
| MockLlmClient | LlmClient | LLM 调用模拟 |
| MockPromptProvider | PromptProvider | Prompt 提供模拟 |
| MockRiskDetector | RiskDetector | 风险检测模拟 |

这些 Mock 仅在集成测试中使用，不属于生产代码。

### OpenAPI 规范

**状态：Pending** — 尚未开始。包含以下子任务：

- [ ] 添加 `utoipa` 依赖（`Cargo.toml`）
- [ ] 为所有 DTO 添加 `#[derive(ToSchema)]` 和 `#[schema(...)]`
- [ ] 为所有 handler 添加 `#[utoipa::path(...)]` 注解
- [ ] 构建 `OpenApi` 结构体并生成 OpenAPI JSON
- [ ] 挂载 `/docs` 或 `/openapi.json` 路由
- [ ] 验证生成的 specs 与实际路由一致

---

## 2026-06-11 更新

### SQL 补丁执行

数据库变更通过 `database/patches/` 下的 SQL 脚本手动执行，而非由 Rust 服务自动执行。

当前补丁：

| 文件 | 变更内容 |
|------|---------|
| `database/patches/20260611_001_auth_role_refresh_likes.sql` | 新增 `users.role` 列、`refresh_tokens` 表、`content_likes` 表 |

补丁命名约定：`YYYYMMDD_NNN_<description>.sql`

执行策略：
- 补丁由 DBA 或开发者在 MySQL 客户端中手动执行
- Rust 服务不包含自动 migration 引擎
- 执行补丁后，重新运行 `sea-orm-cli generate entity` 以更新 SeaORM 实体

### 新实体生成

执行 SQL 补丁后重新生成的 SeaORM 实体文件：

| 实体 | 文件 | 状态 |
|------|------|------|
| `content_likes` | `src/infrastructure/persistence/entities/content_likes.rs` | 已完成 |
| `refresh_tokens` | `src/infrastructure/persistence/entities/refresh_tokens.rs` | 已完成 |

生成命令（在项目根目录执行）：

```
sea-orm-cli generate entity -o src/infrastructure/persistence/entities
```

两个实体均已在 `entities/mod.rs` 和 `entities/prelude.rs` 中注册。

### Refresh Token 数据库持久化

背景：原实现 `InMemoryRefreshTokenRevocationRepository` 在进程重启后丢失所有撤销记录，且不支持跨实例共享。

现状（2026-06-11）：

- **新接口**：`RefreshTokenStore` trait（位于 `application::auth::auth_service`）
- **数据库实现**：`SeaOrmRefreshTokenStore`（位于 `infrastructure::persistence::implementations::seaorm_refresh_token_store`）
  - `store()` — 写入 `refresh_tokens` 表，SHA-256 token hash, jti, user_id, expires_at
  - `is_revoked()` — 检查 token hash 是否存在且未过期、未被撤销
  - `revoke()` — 设置 `revoked_at` 时间戳
  - `cleanup_expired()` — 删除已过期且已撤销的记录
- **内存回退**：`InMemoryRefreshTokenRevocationRepository` 同时实现 `RefreshTokenRevocationRepository`（旧接口）和 `RefreshTokenStore`（新接口），作为未切换时的保留方案

| 方面 | 旧（内存） | 新（数据库） |
|------|-----------|-------------|
| 位置 | `src/infrastructure/auth/in_memory_refresh_token_revocation_repository.rs` | `src/infrastructure/persistence/implementations/seaorm_refresh_token_store.rs` |
| 数据模型 | `HashMap<String, u64>` | `refresh_tokens` 表 |
| 持久性 | 进程重启丢失 | 持久化到 MySQL |
| 撤销检查 | 内存查找 | SQL 查询 |
| main.rs 引用 | `InMemoryRefreshTokenRevocationRepository::new()` | 尚未切换 |

待办项：将 `main.rs` 中的 `revoke_repo` 从 `InMemoryRefreshTokenRevocationRepository` 切换到 `SeaOrmRefreshTokenStore`，同时完全废弃内存实现。

### content_likes 点赞持久化

| 组件 | 位置 | 状态 |
|------|------|------|
| 数据库表 | `database/patches/20260611_001_auth_role_refresh_likes.sql` | 完成 |
| SQL DDL | `CREATE TABLE content_likes` (like_id, user_id, content_type, content_id, created_at) 含唯一约束 `uk_content_likes_user_content` | 已执行 |
| SeaORM entity | `src/infrastructure/persistence/entities/content_likes.rs` | 完成 |
| Repository trait | `src/domain/like/mod.rs` — `ContentLikeRepository` | 完成 |
| SeaORM 实现 | `src/infrastructure/persistence/implementations/seaorm_like_repository.rs` — `SeaOrmLikeRepository` | 完成 |
| Domain 模块 | `src/domain/like/mod.rs` | 完成 |
| integration mod | `src/domain/mod.rs` — `pub mod like;` | 完成 |

`ContentLikeRepository` trait 方法：

| 方法 | 功能 |
|------|------|
| `toggle(user_id, content_type, content_id)` | 切换点赞状态，返回新的状态（true=已点赞，false=取消点赞） |
| `is_liked(user_id, content_type, content_id)` | 查询是否已点赞 |
| `count_by_content(content_type, content_id)` | 查询某内容的点赞总数 |
| `delete(user_id, content_type, content_id)` | 删除指定点赞记录 |

点赞功能已具备完整基础设施：domain trait、SeaORM repository、handler 中的 `like_post` / `unlike_post` / `like_comment` / `unlike_comment`。唯一缺失的是 router 中的路由注册（受 `users` 表缺少 `role` 字段影响 — 这些 handler 与社区模块一起在 router 中整体挂载时一并管理）。

### Role-based 认证

JWT 层面：

- `JwtTokenService` 在 access token payload 中包含 `role` 字段（`src/infrastructure/auth/jwt_token_service.rs`）
- `AuthService::issue_pair()` 读取用户 domain 模型中的 `role` 字段写入 token
- `TokenClaims` 结构体包含 `role: String`
- 认证中间件解析 JWT 后，`AuthenticatedUser` 中的 role 信息可由 handler 使用

Domain 层面：

- `UserRole` enum（`src/domain/user/user.rs`）：`User` / `Admin` / `SuperAdmin`
- SeaORM 实体 `users.rs` 已包含 `role: String` 字段（由 SQL 补丁添加 `role VARCHAR(32)` 列后重新生成）
- `seaorm_user_repository.rs` 在映射 `Model → domain User` 时读取 `m.role` 并转换为 `UserRole`

当前限制：

- SQL 补丁 `20260611_001` 已在数据库中添加 `users.role` 列（默认 `'USER'`）
- `admin_handler.rs` 包含完整的角色检查逻辑和处理函数
- **router 中未挂载 admin 路由** — 需在 `build_router()` 中添加 `.route()` 调用并加入 `require_role(ADMIN)` 中间件
- 生产数据库中的现有用户 `role` 均为 `'USER'`，Admin 用户的 role 需要手动更新

### Handler Extractor 不统一问题

两个 extractor 模式并存，需要统一：

| 模式 | 使用的 handler 文件 | 数量 |
|------|---------------------|------|
| `State(state): State<ApiState>` | auth_handler, community_handler, diary_handler, user_handler, psychology_handler, depression_handler, music_handler | ~50 处 |
| `Extension(state): Extension<Arc<ApiState>>` | admin_handler, object_handler, session_handler | ~17 处 |

`ApiState` 本身实现了 `Clone`（通过 `#[derive(Clone)]`），且所有字段均为 `Arc<T>`，所以两种方式在功能上等价，但不一致的 extractor 模式破坏了代码风格统一。

统一方向：建议将全部 handler 改为 `State(state): State<ApiState>`，因为：

- `ApiState` 已是 `Clone`
- `State<ApiState>` 是 Axum 的标准实践
- `admin_handler` / `object_handler` / `session_handler` 需要从当前的 `Extension<Arc<ApiState>>` 迁移

**注意**：部分 handler（如 `auth_handler.rs` 中的 `health`, `register`, `login`, `refresh_token`, `logout`）不使用 `state` 参数，不需要修改。

### Router 合并状态

当前 router（`src/api/router.rs`）仅注册了 auth / user / session 三条主路径：

```
build_router() → Router
├── /health → health
├── /api/v1/auth/register → register
├── /api/v1/auth/login → login
├── /api/v1/auth/refresh → refresh_token
├── /api/v1/auth/logout → logout
├── /api/v1/auth/me → me
├── [protected layer (require_bearer_auth)]
│   ├── /api/v1/users/me → get_me / patch_me / delete_me
│   ├── /api/v1/users/me/profile → get_profile / put_profile
│   ├── /api/v1/conversations → list_conversations
│   ├── /api/v1/conversations/{conv_id} → list_conversation_messages
│   ├── /api/v1/llm/sessions → create_session
│   ├── /api/v1/llm/sessions/{session_id}/messages → post_message
│   ├── /api/v1/llm/sessions/{session_id} → get_session_status
│   └── /api/v1/risk-detections → list_risk_detections
```

**尚未注册的模块路由**：

| 模块 | handler 前缀 | 建议路由 |
|------|-------------|---------|
| depression | `depression_handler` | `/api/v1/depression/scales`, `/api/v1/depression/assessments` |
| diary | `diary_handler` | `/api/v1/diaries` |
| music | `music_handler` | `/api/v1/music/tracks` |
| psychology | `psychology_handler` | `/api/v1/psychology/categories`, `/api/v1/psychology/articles`, `/api/v1/psychology/qna`, `/api/v1/psychology/resources`, `/api/v1/psychology/favorites` |
| community | `community_handler` | `/api/v1/community/posts`, `/api/v1/community/comments`, `/api/v1/community/likes` |
| admin | `admin_handler` | `/api/v1/admin/users`, `/api/v1/admin/risk-detections` |
| object storage | `object_handler` | `/api/v1/objects` |

这些路由应该通过 `Router::nest()` 或逐个 `.route()` 合并到 `build_router()` 中。同时需要将 `Cargo.toml` 中的 `axum-extra` 依赖（如果需要 `nest` 的 `path` 参数）或其他依赖检查确认。

### 数据库优先策略仍然有效

**Database-first 策略依然有效**，没有被任何 element 推翻：

1. SQL 补丁 (`database/patches/`) 是唯一的 schema 变更载体
2. SeaORM entity (`src/infrastructure/persistence/entities/`) 始终由 `sea-orm-cli generate entity` 从真实数据库生成
3. `init_db()` 不做任何 DDL（CREATE / ALTER / DROP）
4. 不需要 `sea-orm-migration` 或 Flyway 等数据库迁移框架
5. 不需要维护独立的 `schema.sql`

这一策略的验证闭环：

```
人工修改数据库（直接 SQL DDL 或运行补丁脚本）
    │
    ▼
sea-orm-cli generate entity（生成 SeaORM 实体）
    │
    ▼
SeaORM repository 使用最新实体访问数据库
    │
    ▼
Rust 编译检查确保实体与代码一致
```

2026-06-11 的变更（role 列、refresh_tokens 表、content_likes 表）全部遵循该闭环：先在数据库执行 SQL 补丁 `20260611_001_auth_role_refresh_likes.sql`，再重新生成 entity，然后编写 repository 实现。
