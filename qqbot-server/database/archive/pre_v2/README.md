# Baseline v2 前归档

本目录保留 Baseline v1 和从 v1 升级到 v2 的 8 个增量 SQL，仅用于审计、旧库升级和迁移
故障重放测试。全新数据库不得执行本目录文件，只执行
`baseline/20260806_qqbot_schema_v2.sql`。

测试迁移器只在确认数据库已登记 Baseline v1，或完整登记 33 个 pre-v1 迁移后，按文件名
顺序补齐这里的 8 个增量并登记采用 Baseline v2。部分旧链或无权威记录的既有业务结构必须
fail-closed。
