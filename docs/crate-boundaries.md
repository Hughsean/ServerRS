# Crate 边界

ServerRS 当前只维护 QQBot 个人智能秘书，workspace 由 7 个成员组成。

| Crate | 路径 | 职责 |
| --- | --- | --- |
| `agent-core` | `agent-core` | Provider 中立的状态图与 Checkpoint 基础协议 |
| `personal-secretary` | `personal-secretary` | 领域模型、应用用例和基础设施端口 |
| `personal-secretary-mysql` | `personal-secretary-mysql` | MySQL 仓储适配器与事务实现 |
| `qqbot` | `qqbot` | NapCat/OneBot 只读协议适配器 |
| `qq-open-platform` | `qq-open-platform` | QQ 开放平台 Gateway 与 Owner 投递协议适配器 |
| `qqbot-server` | `qqbot-server` | 配置、依赖装配、Worker 和进程生命周期 |
| `architecture-tests` | `tools/architecture-tests` | 源码与依赖方向门禁 |

```text
agent-core
    ^
    |
personal-secretary <- personal-secretary-mysql
          ^                    ^
          |                    |
          +------ qqbot-server +------ qqbot
                    |
                    +----------------- qq-open-platform
```

依赖规则：

- `personal-secretary` 只定义领域与应用，不依赖 ORM、HTTP、NapCat 或 QQ 开放平台。
- `personal-secretary-mysql` 只实现领域端口，不依赖具体 QQ 协议。
- `qqbot` 仅提供 NapCat 只读能力，不依赖业务层或数据库。
- `qq-open-platform` 仅提供官方协议能力，不决定 Owner 授权和通知策略。
- `qqbot-server` 是唯一组合根；应用层不直接依赖 ORM、文件系统和外部协议实现。
- QQBot 数据库、配置、基线和迁移均归 `qqbot-server` 所有。

架构门禁：

```powershell
cargo test -p architecture-tests --test workspace_boundaries
```
