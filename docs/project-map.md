# ServerRS 项目地图

ServerRS 是个人 QQ 智能秘书 Rust workspace。NapCat 提供个人账号的只读实时事件、目录和历史；
QQ 开放平台只向绑定 Owner 提供受约束的通知与控制入口；MySQL 保存不可变来源事件及其派生状态。

## 目录

```text
qqbot-server/
├── config/                # QQBot TOML、环境变量说明
├── database/              # Baseline v2、活动迁移和历史归档
├── src/application/       # Worker 与应用编排
├── src/bootstrap/         # 依赖装配
├── src/config/            # 配置加载与校验
├── src/infrastructure/    # 文件、MySQL、LLM、健康快照适配
└── src/runtime/           # 连接与关闭生命周期
crates/
├── agent-core/            # 状态图基础协议
├── personal-secretary/    # 领域与应用核心
├── personal-secretary-mysql/ # MySQL 适配器
├── qqbot/                 # NapCat 只读适配器
└── qq-open-platform/      # 官方 QQ 协议适配器
tools/architecture-tests/  # 架构边界门禁
docker/                    # 容器入口脚本
docker-compose.yml         # QQBot + MySQL 一键启动
```

## 运行链路

1. NapCat WebSocket 事件先进入有界 admission，再由 blocking writer 同步持久化到普通消息 Spool。
2. durable receipt 后进入统一 ingestion，MySQL 事务按账号与平台事件 ID 幂等写入 SourceEvent。
3. Recall/Artifact 必需效果收敛后推进连续 checkpoint；崩溃恢复重放完整认证帧。
4. 线程投影、语义、关联、跟进和通知由有界 Worker 异步派生。
5. NapCat HTTP 只用于能力、目录和历史读取，不暴露发送或账号变更 action。
6. QQ 开放平台投递必须经过 Owner 绑定、策略、租约、fencing 和幂等回执。

## 配置与数据

- 本地配置：`qqbot-server/config/qqbot.toml` 与同目录 `.env`，均被 Git 忽略。
- Compose 配置：根目录 `.env`，模板为 `.env.example`。
- 数据库：`qqbot-server/database/baseline/20260806_qqbot_schema_v2.sql`。
- Spool：默认相对配置目录写入 `data/`；Compose 将其挂载到独立命名卷。
- 真实凭据只允许来自环境变量或被忽略的本地密钥文件。

## 常用命令

```powershell
docker compose up -d --build
cargo fmt --all -- --check
cargo check --workspace --all-targets
cargo clippy -p personal-secretary -p personal-secretary-mysql -p qqbot -p qq-open-platform -p qqbot-server --all-targets -- -D warnings
cargo test -p architecture-tests --test workspace_boundaries
```

详细产品状态见 `docs/qq-personal-secretary/TODO.md`，已完成事实见同目录 `HISTORY.md` 与
`history/` 月度归档。
