# ServerRS 项目地图 — 从入门到精通

> 作者：项目代码自动分析
> 目标：让一个完全不懂这个项目的人，看完这个文件就能知道
> "这个项目是干什么的"、"代码放在哪"、"每个文件是干啥的"

---

## 一、这个项目到底是干啥的？

 **一句话版**：这是一个 AI 聊天伴侣后端服务器。用户注册后，可以跟 AI 聊天，AI 会维护长期记忆和用户画像，支持 RAG 知识检索、音乐、抑郁评估、日记、社区、QQ Bot、Web 知识摄入与后台审核。
 **正经版**：ServerRS（代号 Digital Companion）是一个用 **Rust** 写的 **Web 后端服务**，使用 Axum 提供 HTTP API，MySQL 存业务数据，Ollama/OpenAI-compatible Provider 做对话和结构化提取，Qdrant 做向量搜索。

---

## 二、项目长什么样？—— 目录结构总览

 ```
 D:\WorkSpace\ServerRS/
 ├── Cargo.toml            # Rust 项目配置（依赖声明）
 ├── config.toml           # 服务器配置文件（数据库/Ollama/JWT 等）
 ├── .env                  # 环境变量（密钥、数据库密码等）
 │
 ├── src/                  # ★ 核心代码（Rust 源码）
│   ├── main.rs           # 程序入口（加载配置、初始化日志、调用 runtime）
 │   ├── lib.rs            # 模块声明（各模块的入口）
 │   ├── api/              # ★ 网络层（路由 + 请求处理）
 │   ├── app/              # ★ 业务层（服务的具体逻辑）
 │   ├── domain/           # ★ 领域层（核心数据结构和接口）
 │   ├── infra/            # ★ 基础设施（数据库/LLM/存储的具体实现）
 │   ├── bootstrap/        # 组装代码（把各组件拼装起来）
 │   └── shared/           # 共享代码（配置、错误定义）
 │
 ├── database/
 │   └── sql/
 │       ├── init.sql      # ★ 数据库建表语句（所有表结构）
 │       └── mock.sql      # 模拟数据（开发测试用）
 │
 ├── web/
 │   ├── admin/            # ★ 管理后台前端（Vue 3 + TypeScript）
 │   └── sdk/              # TypeScript SDK（前端调用 API）
 │
├── docs/                 # 项目说明、设计文档、历史计划
 ├── data/                 # 运行时数据（MySQL 数据文件、Qdrant 数据等）
├── scripts/              # 运维脚本（知识摄入、API/压测/Ollama 检测等）
 └── examples/             # （空目录，留作示例代码占位）
 ```

---

## 三、分层架构（六层）—— 代码从用户到数据库的完整路径

 当用户发一个 HTTP 请求时，数据经过以下六层：

 ```
 用户浏览器/App
     ↓  HTTP 请求
 ┌──────────────────┐
 │  第1层: api/     │  ← 路由 + 请求处理（接收请求、检查权限、返回响应）
 │  （路由器层）    │     文件：src/api/router.rs + handlers/*.rs
 └──────┬───────────┘
        ↓ 调用 Service
 ┌──────────────────┐
 │  第2层: app/     │  ← 业务逻辑（真正的功能实现）
 │  （服务层）      │     文件：src/app/*/*_service.rs
 └──────┬───────────┘
        ↓ 调用 Repository 接口
 ┌──────────────────┐
 │  第3层: domain/  │  ← 纯数据结构和接口定义（像合同的条款）
 │  （领域层）      │     文件：src/domain/*/mod.rs
 └──────┬───────────┘
        ↓ 调用 Repository 实现
 ┌──────────────────┐
 │  第4层: infra/   │  ← 具体实现（操作数据库、调 LLM、存文件）
│  （基础设施层）  │     文件：src/infra/db/**/*.rs
 └──────┬───────────┘
        ↓ SQL/LLM API
 ┌──────────────────┐
 │  MySQL / Qdrant  │  ← 数据库和向量引擎
 │  / Ollama /      │
 │  文件系统        │
 └──────────────────┘

 还有两个"胶水"模块：
   - bootstrap/      ← 把上面所有层粘在一起（依赖注入）
   - shared/         ← 配置、错误类型等共享工具
 ```

---

## 四、逐层逐文件详解

---

### 4.1 入口区：`src/main.rs`（程序的起点）

```rust
use server_rs::{bootstrap, shared::config::AppConfig};

#[tokio::main]
async fn main() {
    let config = AppConfig::load();
    let _guard = init_tracing(&config.logging.level);
    if let Err(err) = bootstrap::runtime::run(config).await {
        tracing::error!(error = %err, "服务器运行出错");
    }
}
```

`init_tracing` 会同时写 stdout 和 `logs/app.log.YYYY-MM-DD`，并返回非阻塞日志 guard，避免进程退出前日志丢失。

 通俗理解：`main.rs` 从 600+ 行精简到几十行，现在只做三件事：
- 检查预算（加载配置）
- 交代主厨（调用 `bootstrap::runtime::run`）
- 事后复盘（记录顶层错误，同时保留非阻塞日志 guard）

 真正的"做菜"流程——连接数据库、构建服务、启动 HTTP 服务器——全部交给 `bootstrap::runtime` 的 **6 阶段流水线**：
 `Infra → Repos → Tasks → Vector → Services → HTTP`
 每个阶段封装在独立的 bootstrap 模块中（详见 4.7 节）。

---

### 4.2 `src/lib.rs` —— 模块总目录

 只有 5 行：
 ```rust
 pub mod api;
 pub mod app;
 pub mod bootstrap;
 pub mod domain;
 pub mod infra;
 pub mod shared;
 ```
 顾名思义，这是整栋大楼的**楼层索引图**。其他文件通过 `use server_rs::api::xxx` 互相引用。

---

### 4.3 第1层 — API 层（`src/api/`）—— 接待员

 **职责**：接收 HTTP 请求、解析参数、检查身份、调用业务服务、返回 JSON 响应。

 ```
 src/api/
 ├── router.rs           ★ 路由总表（哪个 URL 对应哪个函数）
 ├── state.rs            状态结构（把所有 Service 打包成一个 AppState）
 ├── error.rs            错误处理
 ├── dto/                DTO（请求/响应数据结构）
 │   ├── mod.rs
 │   ├── auth_dto.rs     登录/注册的请求/响应格式
 │   ├── chat_dto.rs     聊天的请求/响应格式
 │   ├── music_dto.rs    音乐的请求/响应格式
 │   └── user_dto.rs     用户的请求/响应格式
 ├── handlers/           ★ 请求处理器（每个文件处理一类业务）
 │   ├── mod.rs
 │   ├── auth_handler.rs     注册/登录/登出/令牌刷新
 │   ├── chat_handler.rs     聊天发送/历史/记忆/人物画像
 │   ├── user_handler.rs     获取/修改个人信息
 │   ├── diary_handler.rs    日记 CRUD
 │   ├── music_handler.rs    音乐浏览/播放
 │   ├── community_handler.rs 社区帖子/评论/点赞
 │   ├── psychology_handler.rs 心理知识库阅读
 │   ├── depression_handler.rs 抑郁量表/评估
 │   ├── object_handler.rs   文件上传/下载
 │   ├── admin_handler.rs    管理员操作用户/风险/知识审核
 │   └── knowledge_review_handler.rs  知识摄入审核/发布
 └── middleware/
     ├── mod.rs
     └── auth_middleware.rs   ★ 身份认证中间件（检查 JWT Token）
 ```

#### `router.rs` —— 路由总表（关键文件）

 这个文件定义了：**用户访问哪个 URL，系统调用哪个函数**。

 **公开路由（不需要登录）：**
 | HTTP 方法 | 路径 | 谁调用 | 干啥的 |
 |-----------|------|--------|--------|
 | GET | /health | 监控 | 健康检查 |
 | POST | /api/v1/auth/register | 任何人 | 注册账号 |
 | POST | /api/v1/auth/login | 任何人 | 登录 |
 | POST | /api/v1/auth/refresh | 任何人 | 刷新令牌 |
 | POST | /api/v1/auth/logout | 任何人 | 退出登录 |
 | GET | /api/v1/psychology/categories | 任何人 | 看心理文章分类 |
 | GET | /api/v1/psychology/articles | 任何人 | 看心理文章列表 |
 | GET | /api/v1/psychology/qna | 任何人 | 看心理问答 |
 | GET | /api/v1/psychology/resources | 任何人 | 看自助资源 |
 | GET | /api/v1/music/tracks | 任何人 | 看音乐列表 |
 | GET | /api/v1/music/tracks/{id}/stream | 任何人 | 听音乐 |
 | GET | /api/v1/depression/scales | 任何人 | 看看抑郁量表 |
 | GET | /api/v1/community/posts | 任何人 | 看社区帖子 |

 **需要登录的路由：**
 | HTTP 方法 | 路径 | 干啥的 |
 |-----------|------|--------|
 | GET | /api/v1/auth/me | 看自己的信息 |
 | PATCH | /api/v1/users/me | 改自己信息 |
 | POST | /api/v1/chat/open | 开始聊天 |
 | POST | /api/v1/chat/messages | 发消息给 AI |
| GET | /api/v1/chat/history | 看聊天历史 |
| GET | /api/v1/chat/memories | 看 AI 记住了什么 |
| GET | /api/v1/chat/persona | 看当前用户画像快照 |
| POST | /api/v1/chat/persona/reset | 重置用户画像 |
| POST | /api/v1/chat/persona/rebuild | 重建用户画像 |
| POST | /api/v1/chat/transcript/clear | 清空对话文本和摘要 |
| POST | /api/v1/chat/forget | 清空用户上下文（记忆/摘要/画像） |
| POST | /api/v1/diaries | 写日记 |
| POST | /api/v1/depression/assessments | 做抑郁测试 |
| POST | /api/v1/community/posts | 发帖子 |
 | POST | /api/v1/objects/upload | 上传文件 |

 **需要管理员身份的路由：**
 | HTTP 方法 | 路径 | 干啥的 |
 |-----------|------|--------|
 | GET | /api/v1/admin/users | 查看所有用户 |
 | DELETE | /api/v1/admin/users/{id} | 删除用户 |
| GET | /api/v1/admin/risk-conversations | 看风险对话 |
| GET | /api/v1/admin/risk-conversations/{id} | 看风险对话详情 |
| POST | /api/v1/admin/risk-detections/{id}/process | 标记风险处理结果 |
| POST | /api/v1/admin/music | 创建音乐 |
| POST | /api/v1/admin/psychology/articles | 创建心理文章 |
| GET | /api/v1/admin/web-ingestion/reviews | 知识审核列表 |
| GET | /api/v1/admin/web-ingestion/reviews/{publish_record_id} | 知识审核详情 |
| POST | /api/v1/admin/web-ingestion/reviews/{publish_record_id}/publish | 审核通过发布 |
 | GET | /api/v1/admin/stats/users | 用户统计（总数 + 7 日趋势） |
 | GET | /api/v1/admin/stats/risks | 风险统计（总数 + 趋势 + 风险等级分布） |
 | GET | /api/v1/admin/stats/music | 音乐统计（总数 + 7 日趋势） |
 | GET | /api/v1/admin/stats/reviews | 审核统计（总数 + 7 日趋势） |

---

### 4.4 第2层 — 服务层（`src/app/`）—— 业务大脑

 **职责**：每个 `xxx_service.rs` 文件实现了某类业务的核心逻辑。

 ```
 src/app/
 ├── mod.rs              声明所有子模块
 │
 ├── auth/
 │   ├── mod.rs
 │   └── auth_service.rs      ★ 注册、登录、令牌刷新逻辑
 │
 ├── user/
 │   ├── mod.rs
 │   └── user_service.rs      ★ 用户资料 CRUD
 │
 ├── session/                 ★★★ 最核心的聊天区
 │   ├── mod.rs
 │   ├── chat_service.rs      ★ 聊天主入口（发消息、收回复）
 │   └── session_service.rs   查询会话数据
 │
 ├── agent/                   ★★★ Agent（智能体）区
 │   ├── mod.rs
 │   ├── agent_runtime.rs     ★ Agent 运行时（决定 AI 用哪个工具）
 │   ├── agent_context.rs     构建 Agent 上下文（记忆+RAG+摘要）
 │   ├── prompt_builder.rs    构建发送给 LLM 的提示词
 │   ├── tool_registry.rs     工具注册列表
 │   └── tools/               ★ Agent 能用的工具列表
 │       ├── get_weather_tool.rs      查天气
 │       ├── knowledge_search_tool.rs  搜知识库
 │       ├── memory_search_tool.rs     搜长期记忆
 │       ├── diary_search_tool.rs      搜日记
 │       ├── depression_scale_tool.rs  查抑郁评估记录
 │       ├── music_recommend_tool.rs   推荐音乐
 │       ├── community_search_tool.rs  搜社区帖子
 │       ├── get_time_tool.rs          查时间
 │       ├── fetch_web_content_tool.rs 抓网页
 │       └── baidu_baike_tool.rs       查百度百科
 │
 ├── memory/                   ★★★ 记忆区
 │   ├── mod.rs
 │   ├── memory_service.rs     ★ 记忆的存储/查询/向量化
 │   └── memory_extractor.rs   从对话中提取事实记忆
 │
 ├── rag/                      ★★★ RAG（知识库检索）
 │   ├── mod.rs
 │   ├── chunking.rs           文本分块
 │   ├── ingestion_service.rs  知识入库
 │   ├── retrieval_service.rs  ★ 检索知识（关键词+向量混合搜索）
 │   └── vector_index_service.rs  向量索引管理（Qdrant）
 │
 ├── risk/                     ★★★ 风险检测
 │   ├── mod.rs
 │   ├── risk_detection_service.rs      风险检测逻辑
 │   └── post_conversation_risk_audit_worker.rs  ★ 对话后置审计
 │
 ├── summary/
 │   ├── mod.rs
 │   ├── summary_service.rs          对话摘要生成
 │   └── summary_refresh_handler.rs  异步刷新摘要
 │
 ├── community/                社区
 │   ├── mod.rs
 │   └── community_service.rs  帖子/评论/点赞
 │
 ├── depression/               抑郁评估
 │   ├── mod.rs
 │   └── depression_service.rs 量表答题计分
 │
 ├── diary/                    日记
 │   ├── mod.rs
 │   └── diary_service.rs      日记 CRUD
 │
 ├── music/                    音乐
 │   ├── mod.rs
 │   └── music_service.rs      曲库 CRUD + 流式播放
 │
 ├── psychology/               心理知识库
 │   ├── mod.rs
 │   └── psychology_service.rs 知识分类/文章/问答/资源
 │
 ├── storage/                  文件存储
 │   ├── mod.rs
 │   └── object_service.rs     上传/下载/删除文件
 │
 ├── qq_bot/                   ★★★ QQ 机器人区
 │   ├── mod.rs
 │   ├── qq_bot_service.rs     ★ QQ 机器人主服务（接收/发送消息）
 │   ├── message_ingestion.rs   消息接入与分派
 │   ├── reply_generator.rs     回复生成
 │   ├── context_builder.rs     构建对话上下文
 │   ├── outbox_worker.rs       发件箱消息投递
 │   ├── segment_dispatcher.rs  消息段分发
 │   ├── trigger_evaluator.rs   触发条件评估
 │   ├── topic_service.rs       话题管理
 │   ├── relationship_service.rs 关系管理
 │   ├── profile_builder.rs     用户画像构建
 │   ├── proactive_evaluator.rs 主动推送评估
 │   └── emotional_state_service.rs 情绪状态服务
 │
 └── web_ingestion/            ★★★ 知识自动爬取流水线
     ├── mod.rs
     ├── dispatcher.rs          分发器（取任务→执行）
     ├── scheduler.rs           调度器（定时触发爬取）
     ├── extractor.rs           网页内容提取（HTML→纯文本）
     ├── industrial_chunker.rs  ★ 工业级文本分块（章节/摘要/原子块）
     ├── quality_gate.rs        质量门控（评分决定自动发布还是人工审核）
     ├── review_service.rs      审核发布服务
     ├── pipeline_context.rs    管道上下文
     ├── state_machine_adapter.rs 状态机适配
     ├── event_types.rs          事件类型
     ├── hash.rs                 哈希工具
     ├── handlers/              各个阶段的事件处理
     └── services/              子服务
 ```

---

### 4.5 第3层 — 领域层（`src/domain/`）—— 合同/接口/数据结构

 **职责**：定义"有什么数据"、"能做什么操作"（接口），但不写具体实现。
 就像签合同——规定双方责任，但不规定具体怎么干活。

 ```
 src/domain/
 ├── mod.rs
 ├── user/
 │   ├── mod.rs
 │   ├── user.rs                用户数据结构
 │   ├── user_profile.rs        用户画像数据结构
 │   ├── user_repository.rs          ★ 用户仓库接口（定义：能查询用户）
 │   ├── user_context_version.rs    上下文版本号结构
 │   ├── user_context_control.rs    上下文控制流结构
 │   └── user_profile_repository.rs 画像仓库接口
 │
 ├── auth/
 │   ├── mod.rs
 │   ├── password_service.rs    密码加密接口
 │   ├── token_service.rs       JWT 令牌接口
 │   ├── refresh_token_store.rs 刷新令牌接口
 │   └── refresh_token_revocation_repository.rs 令牌吊销接口
 │
 ├── conversation/
 │   ├── mod.rs
 │   ├── conversation.rs        会话数据结构
 │   ├── conversation_message.rs 消息数据结构
 │   └── conversation_repository.rs ★ 会话仓库接口
 │
 ├── memory/
 │   └── mod.rs                  记忆数据结构
 │
 ├── rag/
 │   └── mod.rs                  RAG 数据结构
 │
 ├── risk/
 │   ├── mod.rs
 │   ├── detection_types.rs         风险检测类型定义
 │   ├── risk_detection_result.rs   风险检测结果
 │   ├── risk_detector.rs          ★ 风险检测器接口
 │   ├── risk_repository.rs         风险仓库接口
 │   └── post_conversation_risk_audit.rs 后置审计数据结构
 │
 ├── llm/
 │   ├── mod.rs
 │   └── tools.rs                LLM 工具定义
 │
 ├── storage/
 │   └── mod.rs                  对象存储接口
 │
 ├── vector_store/
 │   ├── mod.rs
 │   └── types.rs                向量存储类型
 │
 ├── tasks/
 │   ├── mod.rs
 │   ├── task_event.rs           任务事件定义
 │   ├── task_handler.rs         ★ 任务处理器接口
 │   └── task_publisher.rs       任务发布器接口
 │
 ├── web_ingestion/              知识摄入领域
 │   ├── mod.rs
 │   ├── error.rs                错误类型定义
 │   ├── event_types.rs          事件类型
 │   ├── status.rs               状态枚举
 │   ├── state_machine.rs        ★ 状态机定义
 │   ├── fetcher.rs              网页抓取接口
 │   ├── distiller.rs            LLM 蒸馏接口
 │   ├── repository.rs           仓库接口
 │   └── review.rs               审核数据结构
 │
 ├── community/
 │   └── mod.rs                  社区数据结构
 ├── depression/
 │   └── mod.rs                  抑郁评估数据结构
 ├── diary/
 │   └── mod.rs                  日记数据结构
 ├── music/
 │   └── mod.rs                  音乐数据结构
 ├── psychology/
 │   └── mod.rs                  心理知识库数据结构
 ├── like/
 │   └── mod.rs                  点赞数据结构
 ├── summary/
 │   └── mod.rs                  摘要数据结构
 ├── vector_index/
 │   └── mod.rs                  向量索引数据结构
 ├── agent/
 │   └── mod.rs                  Agent 事件数据结构
 │
 ├── tts/                       ★★★ TTS 语音合成
 │   ├── mod.rs                 音频格式、请求/响应结构体
 │   │                          TtsRequest/TtsResponse/TtsError
 │   │                          └ AudioFormat(Wav/Mp3/Pcm/OggOpus)
 │   └── (TtsProvider trait)    语音合成接口（async trait）
 │
 ├── qq_bot/                    ★★★ QQ 机器人领域
 │   ├── mod.rs                 模块声明
 │   ├── bot_state.rs           机器人状态
 │   ├── config.rs              机器人配置结构
 │   ├── conversation_state.rs  会话状态
 │   ├── error.rs               错误类型
 │   ├── message.rs             消息数据结构
 │   ├── reply.rs               回复数据结构
 │   ├── turn.rs                对话轮次结构
 │   ├── persona.rs             人设定义
 │   ├── attention.rs           注意力机制
 │   ├── proactive.rs           主动行为定义
 │   ├── user_profile.rs        用户画像接口
 │   ├── relationship.rs        关系数据结构
 │   ├── repository.rs          ★ 核心仓库接口
 │   ├── relationship_repository.rs 关系仓库接口
 │   ├── qq_profile_repository.rs   QQ 画像仓库接口
 │   ├── topic_state.rs         话题状态
 │   └── (各类仓库接口)
 │
 └── tasks/
     └── ...                     任务相关
 ```

---

### 4.6 第4层 — 基础设施层（`src/infra/`）—— 干活的人

 **职责**：实现领域层定义的接口。领域层说"我要能查询用户"，
 基础设施层说"好的，我用 MySQL 查"。

 ```
 src/infra/
 ├── mod.rs
 │
├── db/                         ★ 数据库实现
│   ├── seaorm_db.rs            连接 MySQL
│   ├── entities/               ★ SeaORM 实体（1 文件 = 1 表）
│   │   ├── users.rs / user_profiles.rs / user_persona_snapshots.rs
│   │   ├── conversations.rs / conversation_messages.rs / conversation_summaries.rs
│   │   ├── user_memories.rs / user_memory_evidence.rs / user_context_versions.rs
│   │   ├── knowledge_documents.rs / knowledge_chunks.rs / knowledge_embeddings.rs
│   │   ├── vector_index_jobs.rs / vector_index_records.rs
│   │   ├── web_sources.rs / web_source_urls.rs / web_pages.rs / web_crawl_jobs.rs
│   │   ├── knowledge_ingestion_runs.rs / knowledge_publish_records.rs
│   │   ├── knowledge_chunk_manifests.rs / knowledge_vector_manifests.rs
│   │   ├── domain_event_outbox.rs / web_ingestion_audit_logs.rs
│   │   ├── qq_*                 QQ Bot 相关表实体
│   │   └── prelude.rs           实体导出的快捷引用
│   └── imp/                     ★ domain trait 的 SeaORM 实现
│       ├── user_repo.rs / user_profile_repo.rs
│       ├── user_context_version_repo.rs / user_context_control_repo.rs
│       ├── conversation_repo.rs / conversation_summary_repo.rs
│       ├── memory_repo.rs / rag_repo.rs / vector_index_repo.rs
│       ├── risk_repo.rs / agent_repo.rs / refresh_token_store.rs
│       ├── music_repo.rs / diary_repo.rs / depression_repo.rs
│       ├── psychology_repo.rs / community_repo.rs / like_repo.rs
│       ├── stored_object_repo.rs
│       └── stub_repo.rs         测试用桩实现
│
 ├── llm/                           AI 模型实现
 │   ├── mod.rs
 │   ├── ollama_client.rs           调用 Ollama API
 │   ├── ollama_provider.rs         LLM 聊天实现
 │   ├── ollama_embedding_provider.rs 向量嵌入实现
 │   ├── mock_provider.rs           测试用的假 LLM
 │   └── prompt_provider.rs         提示词模板
 │
 ├── auth/                          认证实现
 │   ├── mod.rs
 │   ├── jwt_token_service.rs       JWT 令牌生成/验证
 │   ├── bcrypt_password_hasher.rs  密码加密
 │   └── in_memory_refresh_token_revocation_repository.rs 令牌吊销
 │
 ├── tts/                          语音合成
 │   ├── mod.rs                    模块声明
 │   └── volcengine_provider.rs    ★ 火山引擎（豆包语音）TTS 实现
 │                                  └ 调用 v3 API 合成语音
	 │                                  └ 支持中文/英文/日文 13 种音色
	 │
	 ├── qq_bot/                        ★★★ QQ 机器人基础设施
	 │   ├── mod.rs                    模块声明
	 │   ├── attention_store.rs        注意力存储
	 │   ├── napcat/                   NapCat 协议适配
	 │   │   ├── mod.rs
	 │   │   ├── api.rs                ★ NapCat HTTP API 封装
	 │   │   ├── listener.rs           ★ WebSocket 事件监听
	 │   │   ├── notice_handler.rs     通知事件处理
	 │   │   └── message_parser.rs     消息解析
	 │   ├── repositories/             ★ 数据库仓库实现
	 │   │   ├── mod.rs
	 │   │   ├── seaorm_agent_turn_repository.rs   对话轮次
	 │   │   ├── seaorm_bot_account_repository.rs  机器人账号
	 │   │   ├── seaorm_external_user_repository.rs 外部用户
	 │   │   ├── seaorm_group_member_repository.rs 群成员
	 │   │   ├── seaorm_group_message_repository.rs 群消息
	 │   │   ├── seaorm_group_memory_repository.rs 群记忆
	 │   │   ├── seaorm_group_repository.rs        群组
	 │   │   ├── seaorm_group_summary_repository.rs 群摘要
	 │   │   ├── seaorm_outbox_repository.rs       发件箱
	 │   │   ├── seaorm_relationship_repository.rs  关系
	 │   │   ├── seaorm_user_profile_repository.rs  用户画像
	 │   │   └── mock.rs               模拟仓库（测试用）
	 │   └── models/                   数据模型
	 │       ├── mod.rs
	 │       └── qq_bot_accounts.rs
	 │
	 ├── detector/                      风险检测实现
 │   ├── mod.rs
 │   └── rule_based_detector.rs    ★ 基于规则的风险检测器
 │
 ├── storage/
 │   ├── mod.rs
 │   └── local_storage.rs           本地文件存储
 │
 ├── vector_store/
 │   ├── mod.rs
 │   ├── qdrant_vector_store.rs     ★ Qdrant 向量存储实现
 │   └── mock_vector_store.rs        测试用的假向量存储
 │
 ├── tasks/                         后台任务
 │   ├── mod.rs
 │   ├── in_memory_task_flow.rs     内存任务管道
 │   ├── logging_handler.rs         日志处理器
 │   ├── alert_handler.rs           告警处理器
 │   └── rate_limit_handler.rs      限流处理器
 │
 ├── ssh_tunnel.rs                  SSH 隧道管理（ssh -L/-R 子进程）
 │
 └── web_ingestion/                 知识摄入实现
     ├── mod.rs
     ├── fetcher.rs                 用 Reqwest 抓网页
     ├── distiller.rs               用 LLM 蒸馏摘要
     ├── repositories.rs            知识摄入的 MySQL 操作
     └── review_repository.rs       审核记录的 MySQL 操作
 ```

---

### 4.7 胶水层 `src/bootstrap/` —— 组装车间

 **职责**：把各种零件（Repository、Service、Provider）组装到一起。

 ```
 src/bootstrap/
 ├── mod.rs          模块入口（声明所有子模块）
 ├── repos.rs        ★ 创建所有 Repository（数据仓库）
 ├── auth.rs         创建认证服务
 ├── infra.rs        ★ 基础设施装配（SSH 隧道、数据库连接、LLM Provider）
 ├── tasks.rs        ★ 任务系统装配（任务发布器、Worker、限流/告警处理器）
 ├── vector.rs       ★ 向量/RAG 装配（Embedding Provider、Qdrant 向量库、向量索引服务）
 ├── state.rs        ★ 业务服务装配，把所有 Service 打包成 ServiceGraph
 ├── runtime.rs      ★ 顶层编排（6 阶段顺序启动：Infra → Repos → Tasks → Vector → Services → HTTP）
 ├── qq_bot.rs       创建 QQ 机器人服务（feature gate 控制）
 └── web_ingestion.rs  初始化知识摄入模块
 ```

---

### 4.8 共享层 `src/shared/`

 ```
 src/shared/
 ├── mod.rs
 ├── config/          ★★ 配置子模块目录（AppConfig 拆分为多个领域文件）
 │   ├── mod.rs                配置模块入口 + AppConfig 结构体
 │   ├── server.rs             ServerConfig, DatabaseConfig, SessionConfig
 │   ├── auth_storage.rs       JwtConfig, AuthConfig, StorageConfig
 │   ├── llm_agent_rag.rs      LlmConfig, AgentConfig, RagConfig, EmbeddingConfig
 │   ├── mail_cors_log.rs      MailConfig, CorsConfig, LoggingConfig, DetectorConfig, OllamaConfig
 │   ├── plugins.rs            PluginsConfig + 5 个插件配置
 │   ├── web_ingestion.rs      WebIngestionConfig, DistillLlmConfig
 │   ├── tts.rs                TtsConfig
 │   ├── qdrant.rs             QdrantConfig
 │   ├── qq_bot.rs             QqBotConfig
 │   └── display_config.rs     Display for AppConfig 实现
 ├── error.rs       错误类型定义
 └── llm_json.rs    LLM JSON 清洗/提取（处理 <think>、markdown fence、首个 JSON 值）
 ```

 `config/` 定义了所有配置项：
 - `server`：监听地址和端口（默认 0.0.0.0:8080）
 - `database`：MySQL 连接
 - `jwt`：JWT 密钥、过期时间
 - `auth`：登录限制（最大尝试次数、锁定时间）
 - `storage`：文件存储后端和大小限制
 - `ollama`：Ollama LLM 配置
 - `llm`：AI 模型配置
 - `agent`：Agent 开关（记忆/RAG/摘要）
 - `rag`：RAG 分块大小、检索参数
 - `qdrant`：向量数据库配置
 - `embedding`：向量嵌入配置
 - `web_ingestion`：知识摄入全部配置
 - `tts`：语音合成配置（火山引擎 API Key、模型、音色等）
 - `qq_bot`：QQ 机器人配置（QQ 号、NapCat 连接地址等）
 - `ssh_tunnels`：SSH 隧道配置（多组跳板机定义，数据库和 Ollama 可引用）
 - `plugins`：Agent 工具配置（天气、新闻、搜索等）
 - `mail`：邮件配置
 - `cors`：跨域配置
 - `logging`：日志级别

---

## 五、数据库表全解（52 张表）

> 建表语句在：`database/sql/init.sql`
> 每个表的 Rust 实体在：`src/infra/db/entities/`

### 5.1 用户与账号（5 张表）

 | 表名 | 中文名 | 存什么 | 关键字段 |
 |------|--------|--------|----------|
 | `users` | 用户表 | 账号信息 | id, username, password(加密), email, phone, role(USER/ADMIN) |
 | `refresh_tokens` | 刷新令牌表 | 记住登录状态 | token_hash, user_id, expires_at |
 | `user_profiles` | 用户画像表 | AI 对用户的了解 | interests, personality_traits, emotional_tendency |
 | `user_context_versions` | 上下文版本号 | 标记画像/记忆是否更新 | version(数字, 每次都+1) |
 | `user_persona_snapshots` | 画像快照表 | AI 生成的用户画像缓存 | snapshot_data(JSON), 每用户1条 active |

### 5.2 对话（4 张表）

 | 表名 | 中文名 | 存什么 |
 |------|--------|--------|
 | `conversations` | 会话表 | 每个用户 1 条，记录对话主题、消息总数 |
 | `conversation_messages` | 消息表 | 每条聊天消息（谁发的、发的啥） |
 | `conversation_summaries` | 摘要表 | 聊完一个话题后生成的摘要（有版本追溯） |
 | `agent_events` | Agent 事件表 | AI 调用工具的记录 |

### 5.3 记忆系统（3 张表）

 | 表名 | 中文名 | 存什么 |
 |------|--------|--------|
 | `user_memories` | 记忆表 | AI 从聊天中提取的长期记忆（"用户叫张三，喜欢猫"） |
 | `user_memory_evidence` | 记忆证据表 | 这条记忆是从哪条消息/摘要提取的（追溯用） |
 | `user_persona_snapshots` | 画像快照表 |（同 5.1）|

### 5.4 心理健康（6 张表）

 | 表名 | 中文名 | 存什么 |
 |------|--------|--------|
 | `psychology_categories` | 分类表 | 知识分类树（如"焦虑症/抑郁症/..."） |
 | `psychology_articles` | 文章表 | 心理科普文章 |
 | `psychology_qna` | 问答表 | 常见心理问题及答案 |
 | `psychology_resources` | 资源表 | 自助资源（热线电话/书籍推荐等） |
 | `user_knowledge_favorites` | 收藏表 | 用户收藏了哪些文章/问答/资源 |
 | `content_likes` | 点赞表 | 用户点赞记录 |

### 5.5 抑郁评估（2 张表）

 | 表名 | 中文名 | 存什么 |
 |------|--------|--------|
 | `depression_scales` | 量表定义表 | PHQ-9/SDS/BDI 等量表的题目和评分标准 |
 | `depression_assessments` | 评估记录表 | 用户每次答题的结果 |

### 5.6 社区（4 张表）

 | 表名 | 中文名 | 存什么 |
 |------|--------|--------|
 | `community_posts` | 帖子表 | 用户发的帖子 |
 | `community_post_media` | 媒体表 | 帖子附带的图片/视频二进制数据 |
 | `community_comments` | 评论表 | 帖子评论（支持楼中楼） |
 | `content_likes` | 点赞表 |（同 5.4，也用于社区点赞）|

### 5.7 其他业务（3 张表）

 | 表名 | 中文名 | 存什么 |
 |------|--------|--------|
 | `user_diaries` | 日记表 | 用户写的日记 |
 | `music` | 音乐表 | 音乐曲库 |
 | `stored_objects` | 对象存储表 | 上传文件的元数据 |

### 5.8 QQ 机器人（11 张表）

 | 表名 | 中文名 | 存什么 |
 |------|--------|--------|
 | `qq_bot_accounts` | 机器人账号表 | 机器人 QQ 号与登录态 |
 | `qq_external_users` | 外部用户表 | QQ 用户信息映射 |
 | `qq_relationships` | 关系表 | 机器人对用户的亲密/信任度 |
 | `qq_user_profiles` | 用户画像表 | AI 对用户的性格/情绪画像 |
 | `qq_groups` | 群组表 | 群组信息与配置 |
 | `qq_group_members` | 群成员表 | 群成员列表与角色 |
 | `qq_group_messages` | 群消息表 | 群聊消息记录 |
 | `qq_group_memories` | 群记忆表 | 群聊长期记忆 |
 | `qq_group_summaries` | 群摘要表 | 群聊话题摘要 |
 | `qq_agent_turns` | 对话轮次表 | 每次 LLM 调用的完整记录 |
 | `qq_message_outbox` | 发件箱表 | 待发送的消息队列 |

### 5.9 知识摄入（15 张表）— 最复杂的一块

 | 表名 | 中文名 | 存什么 |
 |------|--------|--------|
 | `web_sources` | 来源表 | 要爬取的网站配置（种子 URL、规则） |
 | `web_source_urls` | URL 表 | 从来源发现的待爬 URL 队列 |
| `web_pages` | 网页表 | 网页实体索引：URL、hash、latest run 指针；正文不在这里 |
 | `web_crawl_jobs` | 爬取任务表 | 爬虫任务记录 |
 | `knowledge_ingestion_runs` | 运行记录表 | 每次知识摄入流水线的运行记录 |
 | `knowledge_documents` | 文档表 | 爬取/蒸馏后生成的文档 |
 | `knowledge_chunks` | 分块表 | 文档被切成的文本块 |
| `knowledge_embeddings` | 向量表 | 文本块的向量 JSON；Qdrant 索引用它写入 point |
 | `knowledge_publish_records` | 发布记录表 | 发布的版本（staged→published→superseded） |
 | `knowledge_chunk_manifests` | 分块映射表 | 发布版本→分块的映射 |
 | `knowledge_vector_manifests` | 向量映射表 | 分块→Qdrant 向量的映射 |
 | `domain_event_outbox` | 事件发件箱 | 异步事件（publish 等） |
 | `web_ingestion_audit_logs` | 审计日志 | 所有操作的日志 |
 | `vector_index_jobs` | 索引任务表 | 向量索引任务 |
 | `vector_index_records` | 索引记录表 | 向量索引记录 |

### 5.9 风险检测（1 张表）

 | 表名 | 中文名 | 存什么 |
 |------|--------|--------|
 | `post_conversation_risk_audits` | 风险审计表 | 对话后的风险检测结果 |

---

## 六、一条请求的完整旅程（举例：用户发消息给 AI）

 假设用户已登录，发送一条消息"今天天气怎么样？"

 ```
 ┌── 1. HTTP 请求 ──────────────────────────────────────────┐
 │ POST /api/v1/chat/messages                               │
 │ Headers: Authorization: Bearer eyJxxx...                 │
 │ Body: { "message": "今天天气怎么样？" }                    │
 └──────────────────────────────────────────────────────────┘
        │
        ▼
 ┌── 2. src/api/router.rs ───────────────────────────────────┐
 │ 匹配路由 → 调用 chat_send_message 函数                     │
 └──────────────────────────────────────────────────────────┘
        │
        ▼
 ┌── 3. src/api/middleware/auth_middleware.rs ──────────────┐
 │ 解析 JWT Token → 确认用户身份 → 通过                      │
 └──────────────────────────────────────────────────────────┘
        │
        ▼
 ┌── 4. src/api/handlers/chat_handler.rs ──────────────────┐
 │ chat_send_message():                                     │
 │ ① 解析请求体 → ② 调用 chat_service.send_message()        │
 └──────────────────────────────────────────────────────────┘
        │
        ▼
┌── 5. src/app/session/chat_service.rs ───────────────────┐
│ ChatService::send_message():                              │
│ ① 按 user_id 加锁，找到/创建用户唯一会话                   │
│ ② 从数据库加载最近历史消息                                │
│ ③ 调用 AgentRuntime 生成回复                              │
│ ④ 发布 TurnClosedEvent，返回结果                           │
└──────────────────────────────────────────────────────────┘
       │
       ▼
┌── 6. src/app/agent/agent_runtime.rs ────────────────────┐
│ AgentRuntime::respond():                                  │
│ ① 追加当前用户消息并按 max_context_messages 截断            │
│ ② 构建摘要、长期记忆、RAG、用户画像上下文                  │
│ ③ 构建 system prompt + 工具描述                            │
│ ④ 调用 LLM（Ollama/OpenAI-compatible API）                 │
│ ⑤ LLM 决定要不要用工具 → 发现涉及天气                      │
│ ⑥ 调用 get_weather_tool                                    │
│ ⑦ 把工具结果发给 LLM 生成最终回复                          │
│ ⑧ 原子保存用户消息和 AI 回复                               │
│ ⑨ 异步提取当前轮次记忆                                    │
└──────────────────────────────────────────────────────────┘
        │
        ▼
 ┌── 7. src/app/agent/tools/get_weather_tool.rs ──────────┐
 │ GetWeatherTool::call():                                  │
 │ ① 调和风天气 API 查合肥实时天气                          │
 │ ② 返回天气数据                                            │
 └──────────────────────────────────────────────────────────┘
        │
        ▼
 ┌── 8. LLM 生成回复 ─────────────────────────────────────┐
 │ "合肥当前温度 28°C，晴，适合出门散步 ☀️"                   │
 └──────────────────────────────────────────────────────────┘
        │
        ▼
┌── 9. 后台异步任务 ──────────────────────────────────────┐
│ ① SummaryRefreshHandler → 检查是否需要生成摘要             │
│ ② PostConversationRiskAuditWorker → 检查是否有风险内容      │
│ ③ MemoryExtractor → 当前轮次内异步提取长期记忆              │
└──────────────────────────────────────────────────────────┘
        │
        ▼
 ┌── 10. HTTP 响应 ────────────────────────────────────────┐
 │ { "reply": "合肥当前温度 28°C，晴，适合出门散步 ☀️",      │
 │   "tool_calls": [...] }                                   │
 └──────────────────────────────────────────────────────────┘
 ```

---

## 七、文件依赖关系图（谁依赖谁）

 ```
 main.rs（几十行）
   └── bootstrap::runtime::run()    ← 6 阶段顺序启动
        │
        ├── ① bootstrap::infra      SSH 隧道、数据库连接、LLM Provider
        ├── ② bootstrap::repos      → 依赖 infra/db/ 和 domain/*/ 接口
        ├── ③ bootstrap::tasks      任务系统装配（发布器、Worker、限流/告警）
        ├── ④ bootstrap::vector     Embedding Provider、Qdrant 向量库、向量索引服务
        ├── ⑤ bootstrap::state      构造所有 Service（Chat/Memory/RAG/Agent/...）
        └── ⑥ API / HTTP Serve      Axum 路由、中间件、优雅关闭
               │
               ├── api/           ← HTTP 层
               │   ├── router.rs  → 引用 handlers/* 注册路由
               │   ├── handlers/* → 调用 app/*_service 执行业务
               │   └── middleware/ → 检查 JWT 身份
               │
               ├── app/           ← 业务逻辑层
               │   ├── */*_service.rs  → 调用 domain/* 中的接口
               │   └── agent/     → 调用 app/memory/, app/rag/, app/summary/, tools/
               │
               ├── domain/        ← 定义接口和数据结构（纯 Rust 结构体 + trait）
               │   └── */         → 被 app/ 和 infra/ 双方引用
               │
               └── infra/         ← 实现 domain 接口
                   ├── db/          → 操作 MySQL（SeaORM）
                   ├── llm/         → 调 Ollama API
                   ├── tts/         → 调火山引擎语音 API
                   ├── qq_bot/      → NapCat QQ 协议适配
                   ├── vector_store/ → 操作 Qdrant（QdrantVectorStore）
                   └── detector/    → 风险检测规则

 依赖方向（从右到左）：
   main.rs → bootstrap → api/app → domain ← infra
                      ↑                    ↑
                      └──  glue code ──────┘
 ```

 **关键原则**：
 - `domain/` **不依赖** `app/` 或 `infra/`（纯接口层）
 - `infra/` **依赖** `domain/`（实现接口）
 - `app/` **依赖** `domain/`（调用接口）
 - `api/` **依赖** `app/`（调用服务）
 - `main.rs` **依赖** `bootstrap::runtime`（顶层编排）

---

## 八、配置文件速查（`config.toml`）

| 配置段 | 关键字段 | 代码默认值 | 说明 |
|--------|---------|--------|------|
| `[server]` | host, port | 0.0.0.0:8080 | 监听地址 |
| `[database]` | url, max_connections, tunnel | 配置段内必填, 10, none | MySQL 连接；有 tunnel 时 url 必须使用 `{ip}` / `{port}` 模板 |
| `[jwt]` | secret, access_ttl_secs, refresh_ttl_secs | CHANGE_ME..., 900, 2592000 | JWT 密钥与 TTL |
| `[llm]` | provider, base_url, chat_model, enable_reasoning, tunnel | openai, 配置段内必填, qwen2.5:14b, true, none | 聊天模型；有 tunnel 时 base_url 必须使用 `{ip}` / `{port}` 模板 |
| `[embedding]` | provider, base_url, model, dimension, tunnel | ollama, 配置段内必填, nomic-embed-text, 768, none | 向量嵌入；有 tunnel 时 base_url 必须使用 `{ip}` / `{port}` 模板；dimension 会作为 `dimensions` 请求字段 |
| `[agent]` | enabled, memory_enabled, max_context_messages | false, true, 50 | Agent 开关与上下文窗口 |
| `[qdrant]` | enabled, url, tunnel, rag/memory/summary collection | false, 配置段内必填, none, rag_chunks/user_memories/conversation_summaries | 向量数据库；有 tunnel 时 url 必须使用 `{ip}` / `{port}` 模板 |
| `[web_ingestion]` | enabled, scheduler_enabled, dispatcher_enabled, outbox_batch_size, dispatcher_parallelism, auto_publish | false, false, false, 20, 1, false | 知识摄入；蒸馏 base_url 有 tunnel 时必须使用 `{ip}` / `{port}` 模板；dispatcher 按 handler 配额领取 outbox 事件并并发处理 |
| `[tts]` | provider, api_key, resource_id, model | volcengine, "", "", seed-tts-2.0-standard | 语音合成 |
| `[qq_bot]` | enabled, self_qq_id, http_base_url, ws_host/ws_port | false, 0, http://127.0.0.1:3000, 0.0.0.0:6700 | QQ 机器人 |
| `[ssh_tunnels.*]` | host, user, local_port, remote_port, direction, bind_address | 无 | SSH 隧道（数据库/Ollama 引用） |
| `[plugins.weather]` | api_key | "" | 天气 API |
| `[plugins.news]` | rss_urls | 中国新闻网 | 新闻源 |

	环境变量会覆盖配置文件（如 `DATABASE_URL`、`JWT_SECRET`、`LLM_BASE_URL`、`TTS_API_KEY` 等）。

---

## 九、前端项目

### 9.1 管理后台（`web/admin/`）

 **技术栈**：Vue 3 + TypeScript + Pinia + Vue Router + Rolldown

 ```
 web/admin/src/
 ├── main.ts             入口
 ├── App.vue             根组件
 ├── router/
 │   └── index.ts        路由（登录/控制台/用户管理/风险/知识审核/心理学/音乐）
 ├── stores/
 │   └── auth.ts         登录状态管理
 ├── views/
 │   ├── LoginView.vue          登录页
 │   ├── DashboardView.vue      控制台首页
 │   ├── UsersView.vue          用户管理
 │   ├── RiskView.vue           风险对话列表
 │   ├── RiskDetailView.vue     风险对话详情
 │   ├── KnowledgeReviewsView.vue    知识审核列表
 │   ├── KnowledgeReviewDetailView.vue 知识审核详情
 │   ├── PsychologyView.vue     心理知识库管理
 │   ├── MusicView.vue          音乐管理
 │   └── NotFoundView.vue       404
 ├── layouts/
 │   └── AdminLayout.vue    后台布局（侧边栏 + 顶栏 + 暗色模式切换）
 ├── components/
 │   ├── ThemeToggle.vue     暗色模式切换按钮
 │   ├── ToastProvider.vue   浮动通知提示系统
 │   └── ConfirmDialog.vue   确认弹窗（替代 window.confirm）
 ├── lib/
 │   └── sdk.ts             SDK 封装（调用后端 API）
 ├── utils/
 │   ├── format.ts           格式化工具
 │   └── toast.ts           Toast 状态管理
 └── assets/
     ├── base.css            CSS 变量体系 + 暗色模式变量
     └── main.css            全局样式
 ```

### 9.2 TypeScript SDK（`web/sdk/`）

 给前端用的 API 调用封装：
 ```
 web/sdk/src/
 ├── index.ts            SDK 入口（UserClient / AdminClient / 兼容层）
 ├── client.ts           向后兼容的合体版（ServerRsClient，已废弃）
 ├── user-client.ts      UserClient（普通用户 API：auth/chat/diaries/psychology/...）
 ├── admin-client.ts     AdminClient（管理员 API：admin/* 仅管理操作）
 ├── http.ts             HTTP 请求封装（fetch）
 ├── types.ts            所有 API 的数据类型定义
 └── compat/             旧版 SDK 兼容层（DiariesApi/CommunityApi/AdminApi 等）
     ├── index.ts
     ├── diaries.ts
     ├── community.ts
     ├── psychology.ts
     ├── music.ts
     ├── depression.ts
     └── admin.ts
 ```

---

## 十、开发指南

### 前置条件

 | 工具 | 用途 |
 |------|------|
 | Rust ≥ 1.85 | 编译后端 |
 | MySQL 8.0+ | 数据库 |
| Ollama | 运行聊天模型与 embedding 模型（按 `config.toml` 配置拉取） |
 | Qdrant（可选） | 向量数据库 |
 | Node.js 22+ | 前端开发 |

### 启动步骤

 1. **初始化数据库**：`mysql -u root -p < database/sql/init.sql`
2. **启动 Ollama**：`ollama serve`，然后按配置拉取模型，例如 `ollama pull qwen2.5:14b` 和 `ollama pull nomic-embed-text`
 3. **启动 Qdrant**（如果配置了 qdrant.enabled=true）
 4. **启动后端**：
    ```
    copy .env.example .env    # 编辑数据库密码等
    cargo run                 # 编译并启动
    ```
 5. **访问**：`http://localhost:8080/health`

### 代码贡献原则

 | 原则 | 说明 |
 |------|------|
 | 分层依赖 | api → app → domain ← infra |
 | 接口隔离 | domain 定义 trait，infra 实现，app 使用 |
 | 配置先行 | 新增功能先加 config.toml 配置项 |
 | 建表先写 | 新增业务先写 init.sql 再加 Rust 实体 |
 | 测试覆盖 | Service 层写单元测试，Handler 层写集成测试 |

---

## 十一、常见问题

 **Q：这个项目是做什么的？**
 A：一个 AI 聊天伴侣后端。用户可以注册、聊天、写日记、做抑郁评估、逛社区、听音乐。
 AI 会记住用户的信息，越来越了解用户。

**Q：AI 能力是怎么来的？**
A：聊天、记忆提取、画像构建等通过 `LlmProvider` 调用 Ollama/OpenAI-compatible 接口。模型名由 `config.toml` 的 `[llm]`、`[embedding]`、`[web_ingestion.distill_llm]` 决定。

 **Q：项目为什么分这么多层？**
 A：为了"解耦"。业务代码（app/）不需要知道数据存在 MySQL 还是 MongoDB；
 数据库代码（infra/）改动了，业务逻辑不用改。方便测试、维护和扩展。

 **Q：有哪些外部依赖？**
 A：MySQL（数据存储）、Ollama（AI 推理）、Qdrant（向量搜索，可选），
 以及和风天气（查天气）、中国新闻网 RSS（新闻）、火山引擎豆包语音（TTS 语音合成）、
 NapCat（QQ 机器人协议适配）等外部 API。

 **Q：TTS（文字转语音）功能是怎么实现的？**
 A：通过火山引擎（豆包语音）v3 API 将文字合成为语音（WAV/MP3/OGG 格式），
 提供 13 种音色（中/英/日文），支持语速、音量、音调调节。

 **Q：QQ 机器人（QQ Bot）是什么？**
A：ServerRS 内置了一个 QQ 机器人模块，通过 NapCat/OneBot 11 对接 QQ，
可自动回复群聊/私聊消息、管理群话题、维护用户画像和关系状态、主动推送内容，并支持 TTS 语音段。
机器人有自己的完整数据库表和服务链路，支持多账号、多群组、长期记忆。

 **Q：什么是向量搜索？为什么需要 Qdrant？**
 A：传统搜索是"关键字匹配"（搜"苹果"只能找到有"苹果"二字的文章），
 向量搜索是"语义匹配"（搜"苹果"还能找到"iPhone""乔布斯""水果"等相关的）。
 Qdrant 是专门做向量搜索的数据库。

 **Q：知识摄入（Web Ingestion）是干什么的？**
 A：自动爬取指定网站的内容，用 AI 理解、分块、去重后存入知识库。
 这样 AI 就能用这些知识来回答用户问题。整个过程全自动。

 **Q：这个项目有前端吗？**
 A：有一个 Vue 3 写的内容管理后台（管理用户、审核风险、管理知识库等）。
 用户端的聊天界面不在此项目中。

---

*最后核对时间：2026-06-27*
*基于当前工作区代码同步*
