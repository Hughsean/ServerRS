# Phase 5 + 6 迁移 Agent 提示词

## 你的角色

你是 Rust Clean Architecture 专家，负责将 Java Spring Boot 项目剩余功能迁移到 Rust。项目已在 `C:\Users\X\.WorkBench\ServerRS`，Java 源码在 `Server\src\main\java\dev\x\`。

## 核心原则

1. **Domain 层**只定义纯 trait + 实体，零外部依赖
2. **Application 层**是聚合 Service（不用 trait，直接用具体类型）
3. **Infrastructure 层**用 SeaORM 实现 repository，自动 CRUD
4. **API 层**用 axum handler + DTO + middleware
5. 所有 `edit_file` 操作前必须先 `read_file`（工具要求）
6. 不在 `new_text` / `old_text` 中使用空字符串
7. 编译命令：`cargo check`（`cd` = `C:\Users\X\.WorkBench\ServerRS`）

## 已完成（不要重复）

- ✅ 认证授权（AuthService）
- ✅ 用户 CRUD + 画像（UserService）
- ✅ 对话记录 + 消息（SessionService）
- ✅ LLM 会话管理（SessionManager + OllamaClient + fallback）
- ✅ 风险检测（RuleBasedRiskDetector + RiskDetectionService）
- ✅ TaskEvent 系统（8 种事件 + 可插拔处理器 + 溢出保护）
- ✅ 7 个集成测试（`tests/` 目录，无需 MySQL）

## Phase 5：心理知识库 + 抑郁评估

### 5.1 新增 SeaORM 实体（`src/infrastructure/persistence/entities/`）

每个实体参考 Java 的 `Server/src/main/java/dev/x/entity/` 下对应文件：

```rust
// psychology_category.rs
#[derive(Clone, Debug, DeriveEntityModel)]
#[sea_orm(table_name = "psychology_categories")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub category_id: u32,
    pub category_name: String,
    pub parent_id: Option<u32>,
    pub description: Option<String>,
    pub sort_order: i32,
    pub status: i32,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}
// Relation + ActiveModelBehavior 同上
```

需要创建的实体（参考 Java 字段类型）：
- `psychology_category.rs`
- `psychology_article.rs`（Byte/Blob 字段 `cover_image`）
- `psychology_qna.rs`
- `psychology_resource.rs`（Byte/Blob 字段 `file_data`, `thumbnail`）
- `depression_scale.rs`
- `depression_assessment.rs`
- `user_knowledge_favorite.rs`

### 5.2 Domain 层实体（`src/domain/psychology/`）

在 `domain/psychology/` 下创建纯实体（不含 ORM 注解）：
- `psychology_category.rs`
- `psychology_article.rs`
- `psychology_qna.rs`
- `psychology_resource.rs`
- `depression_scale.rs`
- `depression_assessment.rs`
- `repository.rs`（包含统一的 `PsychologyRepository` trait）

### 5.3 Infrastructure repository

```rust
// seaorm_psychology_repository.rs
pub struct SeaOrmPsychologyRepository { db: DatabaseConnection }
// 实现 PsychologyRepository trait，使用 SeaORM 自动 CRUD
```

### 5.4 Application Service

```rust
// psychology_service.rs
pub struct PsychologyService {
    repo: Arc<PsychologyRepository>,
}
// 提供所有心理知识 CRUD 方法
```

### 5.5 API 层

- DTO: `src/api/dto/psychology_dto.rs`
- Handler: `src/api/handlers/psychology_handler.rs`
- 路由注册（`src/api/router.rs`）：
  ```
  GET  /api/v1/psychology/categories
  GET  /api/v1/psychology/articles?category_id=&page=&size=
  GET  /api/v1/psychology/qna?category_id=&page=&size=
  GET  /api/v1/psychology/resources?category_id=&type=&page=&size=
  GET  /api/v1/psychology/depression-scales
  POST /api/v1/psychology/depression-assessments
  GET  /api/v1/psychology/depression-assessments
  ```

### 5.6 接线

1. `ApiState` 添加 `pub psychology: Arc<PsychologyService>`
2. `main.rs` 创建 `PsychologyService` 并注入
3. `database.rs` 调用 `create_table_from_entity` 创建新表

## Phase 6：社区 + 日记 + 音乐 + 后台

### 6.1 社区

参考 Java `entity/CommunityPost.java`, `CommunityComment.java`, `CommunityPostMedia.java`

- `domain/community/` 实体 + repository trait
- SeaORM 实体
- `CommunityService`（聚合 CRUD）
- API: posts CRUD, comments

### 6.2 日记

参考 Java `entity/UserDiary.java`

- `domain/diary/` 实体 + trait
- SeaORM 实体
- `DiaryService`
- API: CRUD

### 6.3 音乐

参考 Java `entity/Music.java`（含 `file_data` BLOB, `cover_image` BLOB）

- `domain/music/` 实体 + trait
- SeaORM 实体
- `MusicService`
- API: 列表/搜索/上传

### 6.4 管理后台

参考 Java `controller/AdminController.java`

- Admin API key 认证中间件
- 风险检测处理接口（标记已处理）
- AdminService

## 每步验证

```bash
cargo check 2>&1  # 编译通过再继续
```

## Java 参考文件位置

```
Server/src/main/java/dev/x/
├── entity/           # 所有实体定义
├── mapper/           # MyBatis SQL（参考查询逻辑）
├── controller/       # API 端点定义
├── service/          # 业务逻辑
└── dto/              # 请求/响应 DTO
```

直接读取 Java 文件获取字段名、类型、SQL 查询逻辑。
