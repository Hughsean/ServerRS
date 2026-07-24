# QQBot 独立配置

QQBot 不读取数字人的根 `config.toml`、`CONFIG_PATH` 或根 `.env`。

本地开发：

1. 将 `qqbot.example.toml` 复制为本目录下的 `qqbot.toml`；
2. 如需环境变量，将 `.env.example` 复制为本目录下的 `.env`；
3. 填写独立 QQBot 数据库和 NapCat 参数；
4. `qqbot.toml`、`.env` 和所有真实凭证不得提交。

可使用 `QQBOT_CONFIG_PATH` 指向其他位置。数据库环境变量只使用
`QQBOT_DATABASE_URL`，不会读取数字人的 `DATABASE_URL`。

历史回补配置位于 `[backfill]` 段，对应 `QQBOT_BACKFILL_*` 环境变量：`page_size` 必须在
`1..=100`，`max_concurrency` 在 `1..=64`，`lease_secs` 在 `1..=3600`，`retry_max_ms` 不得
小于 `retry_initial_ms`。所有历史读取有明确上限（页数/事件数/并发），禁止无限循环。回补
Worker 与实时 WebSocket 接收解耦，NapCat 重连成功后唤醒一次扫描；仅领取空窗已结束
（`gap_ended_at IS NOT NULL`）的 Gap；`max_concurrency` 经 `JoinSet` 产生真实并发；`lease_secs`
用于回补运行续租与崩溃恢复；关闭时通过取消标志优雅退出。真实 NapCat 无法证明账号会话集合
完整时 Gap 保持 `uncertain`，证据不足回到 `uncertain` 的 Gap 受退避约束避免热循环与饿死，
不因 Worker 跑完误标完整。

确定性线程投影配置位于 `[thread_projection]` 段，对应 `QQBOT_THREAD_PROJECTION_*` 环境变量。
Worker 只批量读取已经落库的 `SourceEvent`，不调用 LLM：结构化 Reply 优先归并，同一会话在
`same_conversation_window_secs` 内次之；相同发送者只增加同会话证据，不允许单凭发送者跨群
合并。`batch_size` 与 `max_batches_per_scan` 限制单轮工作量，租约避免多进程重复提交，数据库
错误按 `retry_initial_ms..retry_max_ms` 指数退避。

线程类型化语义配置位于 `[thread_semantics]`。当前内置保守提取器仅识别明确的请求、反对、
确认、决定前缀和问句，输出始终是带来源的 `proposed` 候选；模糊消息不会被编造为事实。
`max_events`、`max_total_chars`、`max_event_chars` 和 `max_batches_per_scan` 同时限制输入与单轮
工作量。超预算正文整条跳过推断，不基于截断文本猜测。未来接入 LLM 时仍必须经过相同的
来源、身份、候选数量、修订链和生命周期校验。

跨会话线程关联配置位于 `[thread_links]`，对应 `QQBOT_THREAD_LINKS_*`。Worker 只识别严格格式
项目 ID 与精确文件 `source_key`，数据库仅保存 SHA-256 指纹；同名人物、相似话题和相同文件名
不会生成候选。所有命中只写入 `proposed` 候选、类型化理由与来源，不会自动合并线程。
`max_events`、`max_total_chars` 和 `max_batches_per_scan` 保证每轮有界，失败指数退避且关闭可取消。

承诺跟进配置位于 `[follow_up]`，对应 `QQBOT_FOLLOW_UP_*`。Worker 每轮先有界标记到期记忆，
再把已确认且有期限的承诺物化为跟进事项；到期事项只进入
`secretary_notification_outbox`。启用 `[qq_open_platform]` 后，官方通道 Worker 按被管理账号
隔离领取通知，只向配置的 Owner OpenID 发送；租约、幂等回执、退避和 `unknown_commit`
会阻止提交结果不明时盲目重发。NapCat 仍不提供业务发送能力。

QQ 开放平台配置位于 `[qq_open_platform]`，参考 OpenClaw QQBot 通道的协议实现，但保持本项目
的独立洋葱边界。组合 Token 的含义是 `AppID:AppSecret`；只把 `app_id` 和 `owner_openid`
写入本地忽略的 `qqbot.toml`，Secret 必须来自 `QQBOT_OPEN_PLATFORM_CLIENT_SECRET` 或
`client_secret_file`。TOML 中的 `client_secret` 字段会被拒绝。不要把真实组合 Token 写进命令
历史、文档或 Git；已经在聊天或终端暴露的 Secret 必须先轮换。

生产环境建议在 MySQL URL 中使用 `ssl-mode=required`；QQBot 的独立 SeaORM 依赖已启用
Rustls。若本地数据库不支持 TLS，应显式评估认证方式，不要为了联通而关闭服务端安全控制。

## 调试日志

默认日志级别为 `info`。排查队列、重试、幂等和连接周期时可在 QQBot 自己的 `.env` 中设置：

```dotenv
RUST_LOG=qqbot_server=debug,qqbot=debug,qq_open_platform=debug,personal_secretary=debug
```

需要逐条观察入队和幂等路径时，可临时提升到：

```dotenv
RUST_LOG=qqbot_server=trace,qqbot=debug,qq_open_platform=debug,personal_secretary=trace
```

`trace/debug` 会包含连接周期、平台消息 ID、会话/参与者 ID、重试次数、队列状态和线程批次
数量，但不会记录聊天正文、媒体内容、Token、数据库密码或 QQ 开放平台 Secret。日志文件仍应
按个人数据妥善保护。
