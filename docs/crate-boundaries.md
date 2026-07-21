# ServerRS Cargo Workspace 与 crate 边界

> 最后核对：2026-07-21

后端已拆分为 5 个 workspace member：

| crate | 路径 | 职责 |
|---|---|---|
| `agent-core` | `crates/agent-core` | 通用 Agent Graph、Effect、Checkpoint、Suspend/Resume、状态机 |
| `ai-core` | `crates/ai-core` | Provider 中立的 LLM、Embedding、TTS 与工具协议 |
| `digital-human` | `crates/digital-human` | 聊天、记忆、RAG、画像、工具和数字人业务持久化 |
| `qqbot` | `crates/qqbot` | QQ 消息、群知识、运营、NapCat、Outbox 和 QQ 持久化 |
| `server` | `apps/server` | Axum、进程配置、数据库连接、依赖装配和启动 |

## 依赖方向

```text
agent-core       ai-core
     ^              ^
     |              |\
     +-- digital-human  qqbot
              \       /
               server
```

- `digital-human -> agent-core, ai-core`
- `qqbot -> ai-core`
- `server -> agent-core, ai-core, digital-human`，启用 QQ feature 时再依赖 `qqbot`
- 禁止 `agent-core` 或 `ai-core` 依赖业务 crate
- 禁止 `digital-human` 与 `qqbot` 相互依赖

具体 SeaORM Repository 类型不对宿主公开。业务 crate 通过各自的
`repositories::build_repositories` 返回领域端口聚合；数据库连接由 `server` 建立。

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

`workspace = true` 统一版本约束，`Cargo.lock` 固定本次实际解析版本。子 crate 的
feature 是增量能力声明，不会产生第二套 Tokio 版本，也不会让所有 crate 自动依赖 Tokio。

## Feature

- 默认构建启用 `qdrant`。
- QQ 业务使用 `qqbot` feature；兼容旧命令，`qq_bot` 仍保留为底层 feature。
- 未启用 QQ feature 时，`server` 不会编译可选的 `qqbot` dependency。

```powershell
cargo check -p server --no-default-features
cargo check -p server --features qqbot
```

## 数据库与配置兼容性

本次拆分只移动 Rust 代码归属，不修改 `database/sql`、表名、字段名或现有数据。
数字人实体保留在 `digital-human`，11 个 `qq_*` 实体归属 `qqbot`；跨业务关联继续
通过标量 ID 表达，业务 crate 不互相引用实体。

配置文件格式保持不变。`server` 负责读取完整配置，将数字人部分交给
`digital-human` 校验，并在启用 QQ feature 时把 `[qq_bot]` 解析为
`qqbot::QqBotConfig`。

## 验证命令

```powershell
cargo fmt --all --check
cargo check --workspace --all-features
cargo test --workspace --no-fail-fast
cargo check -p agent-core
cargo check -p ai-core
cargo check -p digital-human
cargo check -p qqbot
cargo check -p server --no-default-features
cargo check -p server --features qqbot
```

`apps/server/tests/workspace_boundaries.rs` 会持续检查 workspace 成员、内部依赖方向、
中立核心依赖、业务源码隔离、Repository 可见性和 QQ 实体归属。
