# ServerRS Cargo Workspace 与 crate 边界

> 最后核对：2026-07-22

后端包含 6 个 workspace member：

| package | 路径 | 职责 |
|---|---|---|
| `agent-core` | `crates/agent-core` | 通用 Agent Graph、Effect、Checkpoint、Suspend/Resume、状态机 |
| `ai-core` | `crates/ai-core` | Provider 中立的 LLM、Embedding、TTS 与工具协议 |
| `digital-human` | `crates/digital-human` | 聊天、记忆、RAG、画像、工具和数字人业务持久化 |
| `qqbot` | `crates/qqbot` | 仅保留 NapCat/OneBot HTTP、WebSocket、CQ 解析与协议事件接口 |
| `digital-human-server` | `apps/digital-human-server` | 数字人 Axum API、配置、数据库连接、依赖装配和启动 |
| `qqbot-server` | `apps/qqbot-server` | NapCat 独立进程、配置、重连、退出与待接入业务回调 |

## 依赖方向

```text
agent-core       ai-core
     ^              ^
     |              |
     +-- digital-human
              ^
              |
  digital-human-server

qqbot <--- qqbot-server
```

- `digital-human -> agent-core, ai-core`
- `digital-human-server -> agent-core, ai-core, digital-human`
- `qqbot-server -> qqbot`
- `qqbot` 不依赖数字人、AI Core、ORM 或数据库驱动
- 两个应用不互相依赖，也不在同一进程中装配

## QQBot 当前边界

现阶段 QQBot 业务等待重新设计。`crates/qqbot` 只保留：

- NapCat HTTP API 客户端
- NapCat 正向 WebSocket 监听器
- OneBot/CQ 消息解析
- 类型化 `NapCatEvent`
- 未来业务需要实现的 `NapCatEventHandler`

画像、关系、群记忆、主动回复、Outbox、领域服务、Repository、SeaORM entity 和旧
QQ 建表 SQL 均不再属于当前代码基线。`qqbot-server` 的占位 handler 只记录事件元数据，
不会回复消息、写数据库或改变外部状态。

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
- QQBot 独立读取 `qqbot.toml`/`QQBOT_CONFIG_PATH`，配置只包含 NapCat 连接与重连参数，
  不接受数据库或旧业务配置。

## 验证命令

```powershell
cargo fmt --all --check
cargo check -p digital-human-server --no-default-features
cargo check -p digital-human-server
cargo check -p qqbot
cargo check -p qqbot-server
cargo test --workspace --all-features --no-fail-fast
```

`apps/digital-human-server/tests/workspace_boundaries.rs` 持续检查工作区成员、双应用依赖
隔离、QQBot 无 ORM/Repository、旧 QQ SQL 缺失以及 NapCat 适配器文件完整性。
