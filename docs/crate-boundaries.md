# ServerRS Cargo Workspace 与 crate 边界

> 最后核对：2026-07-24

后端包含 8 个 workspace member：

| package | 路径 | 职责 |
|---|---|---|
| `agent-core` | `crates/agent-core` | 通用 Agent Graph、Effect、Checkpoint、Suspend/Resume、状态机 |
| `ai-core` | `crates/ai-core` | Provider 中立的 LLM、Embedding、TTS 与工具协议 |
| `digital-human` | `crates/digital-human` | 聊天、记忆、RAG、画像、工具和数字人业务持久化 |
| `personal-secretary` | `crates/personal-secretary` | 协议无关的个人秘书身份、消息、后续上下文/日程/提醒业务 |
| `qqbot` | `crates/qqbot` | 仅保留 NapCat/OneBot HTTP、WebSocket、CQ 解析与协议事件接口 |
| `qq-open-platform` | `crates/qq-open-platform` | QQ 开放平台鉴权、HTTP 消息 API、Gateway 与类型化协议事件 |
| `digital-human-server` | `apps/digital-human-server` | 数字人 Axum API、配置、数据库连接、依赖装配和启动 |
| `qqbot-server` | `apps/qqbot-server` | NapCat 独立进程、配置、重连、统一身份映射与后续 Worker 装配 |

## 依赖方向

```text
agent-core       ai-core
     ^              ^
     |              |
     +-- digital-human
              ^
              |
  digital-human-server

qqbot ------------------+
qq-open-platform -------+--- qqbot-server
personal-secretary -----+
```

- `digital-human -> agent-core, ai-core`
- `digital-human-server -> agent-core, ai-core, digital-human`
- `qqbot-server -> qqbot, qq-open-platform, personal-secretary`
- `qqbot` 与 `qq-open-platform` 均不依赖数字人、个人秘书、AI Core、ORM 或数据库驱动
- 两个应用不互相依赖，也不在同一进程中装配

## QQBot 当前边界

`crates/qqbot` 仍是纯 NapCat 协议适配器。`crates/qq-open-platform` 是另一个独立协议适配器，
只负责 App 凭据换取、Gateway 会话、C2C/群事件和官方消息 API；两者不互相依赖。NapCat
业务路径保持只读，Owner 通知只能由官方通道经持久化 Outbox 发送。

NapCat 适配器只保留：

- NapCat HTTP API 客户端
- NapCat 正向 WebSocket 监听器
- OneBot/CQ 消息解析
- 类型化 `NapCatEvent`
- 未来业务需要实现的 `NapCatEventHandler`

画像、关系、群记忆、主动回复、Outbox、Repository、SeaORM entity 和旧 QQ 建表 SQL 均不
属于 `qqbot`。新的 `personal-secretary` 已接管协议无关的身份与消息角色；`qqbot-server`
执行 NapCat 到统一信封的映射、实时幂等落库、连接周期/游标/空窗审计，以及独立历史回补
Worker（实时与历史消息走同一幂等入口 `insert_message_if_absent`）和确定性线程批量投影
Worker，不会发送消息；线程领域类型与用例位于 `personal-secretary`，SQL 和运行调度分别位于
基础设施层和 `qqbot-server`。类型化语义提取同样通过协议无关端口进入，只生成带原始事件
来源的候选补丁；当前保守规则适配器不依赖数字人 LLM 实现。

QQ 智能秘书的能力审计、Todo 和历史统一维护在
[`docs/qq-personal-secretary/`](qq-personal-secretary/README.md)。

## 依赖版本统一

第三方依赖版本集中声明在根 `Cargo.toml` 的 `[workspace.dependencies]`。子 crate
只继承版本，并声明自己需要的 feature：

```toml
# 根 Cargo.toml
[workspace.dependencies]
tokio = "1.52.1"

# 子 crate Cargo.toml
tokio = { workspace = true, features = ["io-util"] }
```

`workspace = true` 统一版本约束，`Cargo.lock` 固定实际解析版本；子 crate 的 feature
只增加该依赖在当前构建中需要的能力。

## Cargo Feature

数字人服务器只保留 `qdrant` feature。QQBot 是独立应用，不存在 `qqbot` 或
`qq_bot` Cargo feature，也不存在可选的 QQBot 宿主依赖。

## 数据库和配置

- 代码调整本身不连接、不自动迁移、不删除 MySQL 实例中的任何表或数据。
- 数字人启用工具审批前，已有数据库需要应用
  `database/sql/migrations/20260722_agent_checkpoints.sql`；新库的 `init.sql` 已包含
  `agent_checkpoints`。
- Checkpoint MySQL 适配器是泛型 infra 实现，只依赖 `agent-core` 与领域层的归属元数据；
  `ChatTurnState` 在 repository/bootstrap 装配边界绑定，未形成 `infra -> app` 反向依赖。
- 源码中的 QQ Repository、SeaORM entity 与 `database/sql/QQ_init.sql` 已删除。
- 物理数据库中可能仍存在的旧 `qq_*` 表暂时保留，但当前代码不会读写它们。
- 数字人继续读取根目录 `config.toml`/`CONFIG_PATH`。
- QQBot 只读取 `apps/qqbot-server/config/qqbot.toml`/`QQBOT_CONFIG_PATH` 和同目录 `.env`，
  不读取数字人的根配置、`CONFIG_PATH`、根 `.env` 或 `DATABASE_URL`。QQBot 数据库只使用
  自己的 `[database]`/`QQBOT_DATABASE_URL`，迁移位于 `apps/qqbot-server/database/`。

## 验证命令

```powershell
cargo fmt --all --check
cargo check -p digital-human-server --no-default-features
cargo check -p digital-human-server
cargo check -p qqbot
cargo check -p qqbot-server
cargo test -p personal-secretary -p qqbot -p qqbot-server
cargo test --workspace --all-features --no-fail-fast
```

`apps/digital-human-server/tests/workspace_boundaries.rs` 持续检查工作区成员、双应用依赖
隔离、QQBot 无 ORM/Repository、旧 QQ SQL 缺失以及 NapCat 适配器文件完整性。
