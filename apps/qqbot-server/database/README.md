# QQBot 独立数据库

本目录只属于 `qqbot-server` / 个人 QQ 智能秘书，不属于数字人数据库。QQBot 表不得写入仓库
根目录的数字人 `database/sql/init.sql`，也不得复用数字人的业务表、配置或迁移记录。

## 当前结构

```text
database/
├── baseline/20260806_qqbot_schema_v2.sql  # 全新数据库的最终结构
├── migrations/                            # Baseline v2 之后的增量迁移
├── archive/pre_v1/                        # 压缩前 33 个历史迁移，仅供审计/旧库核对
├── archive/pre_v2/                        # Baseline v1 + 8 个 v1→v2 增量
└── test_support/qqbot_migrations.rs        # 隔离 MySQL 测试共用加载器
```

Baseline v2 包含 83 张 `secretary_*` 表和 2 个 View，只保存最终 DDL，不含业务数据、测试数据、
凭据或历史 `ALTER/DROP` 过程。

## 执行规则

- 全新空库：先执行 `baseline/20260806_qqbot_schema_v2.sql`，再按文件名字典序执行
  `migrations/` 中的增量文件。
- 已登记 Baseline v1，或已完整执行压缩前 33 个迁移的数据库：不得重放 v2 Baseline。确认
  权威记录后，从 `archive/pre_v2/` 补齐 8 个折叠增量，再登记采用 v2。
- 只有部分旧迁移、已有 `secretary_*` 表却没有完整迁移记录的数据库：必须 fail-closed，先人工
  核对结构；不得把 Baseline 覆盖到未知或部分结构上。
- `archive/pre_v1/` 和 `archive/pre_v2/` 永不参与全新数据库加载，也不得用于生产环境选择性修表。
- 后续每个结构变化继续新增一个小型增量 SQL；不要持续改写 Baseline v2。

测试加载器会用 `qqbot_test_schema_migrations` 记录 Baseline 和后续增量，只服务随机隔离测试
schema。生产运行仍只读取 `QQBOT_DATABASE_URL` 或 `qqbot.toml` 的 `[database]`，数据库中不保存
Token、App Secret 或数据库 URL。
