# QQBot 独立配置

QQBot 只读取本应用配置，不读取根 `config.toml`、`CONFIG_PATH` 或根 `.env`。

仓库根目录 `docker-compose.yml` 会把本目录的 `qqbot.example.toml` 复制到镜像，并显式覆盖数据库、
NapCat 和凭据环境变量。容器内通过 loopback 代理访问宿主 NapCat，使用方式见根 `README.md`。

本地开发：

1. 将 `qqbot.example.toml` 复制为本目录下的 `qqbot.toml`；
2. 如需环境变量，将 `.env.example` 复制为本目录下的 `.env`；
3. 填写独立 QQBot 数据库和 NapCat 参数；
4. `qqbot.toml`、`.env` 和所有真实凭证不得提交。

可使用 `QQBOT_CONFIG_PATH` 指向其他位置。数据库环境变量只使用
`QQBOT_DATABASE_URL`，不会读取无前缀的 `DATABASE_URL`。

NapCat HTTP 与 WebSocket 均按本机无 Token 模式接入，不存在 `http_token`、`ws_token` 或
`NAPCAT_HTTP_TOKEN` 配置。为补偿无鉴权边界，`napcat.ws_url` 与 `napcat.http_base_url` 必须使用
`localhost`、`127.0.0.1` 或 `::1`，并禁止在 URL 中嵌入凭据、查询 Token 或片段。NapCat 侧也应
只监听 loopback，不能暴露到局域网或公网。

历史回补配置位于 `[backfill]` 段，对应 `QQBOT_BACKFILL_*` 环境变量。Backfill 默认关闭；显式
启用时必须提供北京时间 `earliest_date = "YYYY-MM-DD"`（环境变量
`QQBOT_BACKFILL_EARLIEST_DATE`）。早于当天零点的消息不入库，命中下界后停止分页并持久化挂起
该 Gap，避免从最新页自动重扫。`page_size` 必须在 `1..=100`，`max_concurrency` 在 `1..=64`，
`lease_secs` 在 `1..=3600`，`retry_max_ms` 不得小于 `retry_initial_ms`。所有历史读取有明确上限
（日期/页数/事件数/并发），禁止无限循环。回补
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

线程类型化语义配置位于 `[thread_semantics]`。`[llm].enabled=false` 时，内置保守提取器仅识别
明确的请求、反对、确认、决定前缀和问句；启用 LLM 后，同一个 Worker 会改用有界模型提取器。
两条路径输出始终是带来源的 `proposed` 候选；模糊消息不会被直接确认为事实。
`max_events`、`max_total_chars`、`max_event_chars` 和 `max_batches_per_scan` 同时限制输入与单轮
工作量。超预算正文整条跳过推断，不基于截断文本猜测。LLM 只能返回不含领域 ID 的候选 DTO；
适配层按批次中的 `source_event_id` 映射发言人并生成领域 ID，最后仍必须经过相同的来源、身份、
候选数量、修订链和生命周期校验。模型不能关闭/合并/拆分线程，也不能调用任何工具。

OpenAI-compatible/Ollama 与 DeepSeek 官方 Provider 配置位于 `[llm]`。`provider` 可取
`openai_compatible`（默认）或 `deepseek`，也可通过 `QQBOT_LLM_PROVIDER` 覆盖。默认兼容端点为本机
`http://127.0.0.1:11434/v1`，只有 loopback 允许明文 HTTP，远程端点必须使用 HTTPS；URL 禁止
携带用户名、密码、查询参数或片段。DeepSeek Provider 固定使用官方
`https://api.deepseek.com/v1`，拒绝自定义端点，密钥只能来自 `QQBOT_DEEPSEEK_API_KEY` 或本地
`api_key_file`；兼容 Provider 使用 `QQBOT_LLM_API_KEY`。`model` 在启用时必填，TOML 中的
`api_key` 会被拒绝。输入字符、输出 Token、响应字节、超时和每类候选数量均有硬上限。DeepSeek
结构化 JSON 请求自动发送 `thinking.type=disabled`；若官方 JSON Output 偶发返回空正文，只在原
请求总期限内重试一次，仍失败时 Action Planner 返回不执行动作的安全回复，线程语义使用保守
提取器。认证、配置和输入上限错误继续 fail-closed。普通模型使用
`reasoning_mode = "provider_default"`；Ollama 的 Qwen3 可显式配置
`reasoning_mode = "qwen_no_think"`，适配器会在用户输入末尾添加 `/no_think` 并同时发送
`think=false`，避免思考内容耗尽结构化输出预算。DeepSeek 不接受该 Ollama 专用模式，而是由
适配器使用官方 `thinking` 对象。以上设置不会修改聊天事件原文。

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

官方通道的主动消息与被动回复是两条不同边界：主动消息没有可用的 Gateway 消息上下文，若 POST
结果不明确必须进入 `unknown_commit`，禁止盲目重发；被动回复必须使用同一 Gateway 事件提供的
权威 `msg_id`，并按目标类型使用平台允许的 `msg_seq`。调用方不得从日志、历史记录或外部输入拼装
回复消息 ID。C2C 事件消息 ID 只在平台规定的有效期内使用，过期或缺少上下文时应拒绝发送。
`owner_openid`/`QQBOT_OPEN_PLATFORM_OWNER_OPENID` 是 Owner 手机 QQ 在该 Bot 的 C2C Gateway
事件中携带的 `author.user_openid`，不是数字 QQ 号。只有它与入站发送者精确匹配时才创建
`OwnerCommand`。群内仅 `GROUP_AT_MESSAGE_CREATE` 且 `member_openid` 与同一 Owner OpenID 精确
匹配时允许进入；普通群消息不触发。Action Run 完成后的 `OwnerResponseDraft` 经独立 Outbox、
租约 fencing、权威 raw event 和当前 OwnerBinding 复验后调用对应 C2C/群被动回复 API。

`[admin]` 管理页面密码固定从 `QQBOT_ADMIN_PASSWORD` 读取，不能写入 TOML。Docker 只把页面映射
到宿主 `127.0.0.1`；登录后通过 NapCat 只读 `get_group_list` 获取群目录，并更新
`whitelist.whitelist_file`。白名单未配置、文件缺失或为空时拒绝全部群消息，私聊始终观察。
管理员密码要求 12～256 字节；会话 Cookie 为进程级随机值，设置 `HttpOnly` 与
`SameSite=Strict`，所有白名单写操作还必须携带进程级 CSRF Token。连续登录失败会触发有界节流。

`qq_open_platform.lifecycle_notifications=true` 时，官方平台 Worker 启动后向配置 Owner 发送
“秘书已上线”，收到统一关闭信号后发送“秘书正在安全下线”。每次通知都有 8 秒上限，失败只记录
类型化错误码，不阻塞其他 Worker 的全局关闭期限。容器应使用 `SIGTERM` 并预留至少 25 秒；根
Compose 当前配置为 60 秒。

生产环境建议在 MySQL URL 中使用 `ssl-mode=required`；QQBot 的独立 SeaORM 依赖已启用
Rustls。若本地数据库不支持 TLS，应显式评估认证方式，不要为了联通而关闭服务端安全控制。

## 调试日志

默认日志级别为 `info`。排查队列、重试、幂等和连接周期时可在 QQBot 自己的 `.env` 中设置：

```dotenv
QQBOT_RUST_LOG=qqbot_server=debug,qqbot=debug,qq_open_platform=debug,personal_secretary=debug
```

需要逐条观察入队和幂等路径时，可临时提升到：

```dotenv
QQBOT_RUST_LOG=qqbot_server=trace,qqbot=debug,qq_open_platform=debug,personal_secretary=trace
```

`trace/debug` 会包含连接周期、平台消息 ID、会话/参与者 ID、重试次数、队列状态、线程批次、
LLM 输入字符数、响应字节数、耗时和 Provider 返回的 Token Usage，但不会记录聊天正文、Prompt、
模型原始输出、媒体内容、API Key、数据库密码或 QQ 开放平台 Secret。日志文件仍应按个人数据
妥善保护。
