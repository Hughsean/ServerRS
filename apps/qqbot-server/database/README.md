# QQBot 独立数据库

本目录只属于 `qqbot-server`/个人 QQ 智能秘书，不属于数字人数据库。

- QQ 表不得写入仓库根目录的 `database/sql/init.sql` 或其 migrations；
- 不复用数字人的 `users`、`conversations`、`user_memories` 等业务表；
- 新空库按 `migrations/` 文件名顺序执行即可建立当前结构；
- 运行时只读取 `QQBOT_DATABASE_URL` 或 `qqbot.toml` 的 `[database]`；
- 所有表使用 `secretary_*` 前缀，后续迁移只在本目录演进。

当前基线迁移：

```text
migrations/20260723_personal_secretary_ingestion.sql
migrations/20260723_personal_secretary_continuity.sql
```

第一项迁移创建账号、会话、入站事件和消息内容；第二项迁移增加连接周期、事件来源关联、
账号/会话游标和不确定空窗。回滚顺序写在各迁移文件末尾，执行回滚前必须先确认不再需要
个人秘书数据。
