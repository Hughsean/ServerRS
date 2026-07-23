# QQBot 独立配置

QQBot 不读取数字人的根 `config.toml`、`CONFIG_PATH` 或根 `.env`。

本地开发：

1. 将 `qqbot.example.toml` 复制为本目录下的 `qqbot.toml`；
2. 如需环境变量，将 `.env.example` 复制为本目录下的 `.env`；
3. 填写独立 QQBot 数据库和 NapCat 参数；
4. `qqbot.toml`、`.env` 和所有真实凭证不得提交。

可使用 `QQBOT_CONFIG_PATH` 指向其他位置。数据库环境变量只使用
`QQBOT_DATABASE_URL`，不会读取数字人的 `DATABASE_URL`。

生产环境建议在 MySQL URL 中使用 `ssl-mode=required`；QQBot 的独立 SeaORM 依赖已启用
Rustls。若本地数据库不支持 TLS，应显式评估认证方式，不要为了联通而关闭服务端安全控制。

## 调试日志

默认日志级别为 `info`。排查队列、重试、幂等和连接周期时可在 QQBot 自己的 `.env` 中设置：

```dotenv
RUST_LOG=qqbot_server=debug,qqbot=debug,personal_secretary=debug
```

需要逐条观察入队和幂等路径时，可临时提升到：

```dotenv
RUST_LOG=qqbot_server=trace,qqbot=debug,personal_secretary=trace
```

`trace/debug` 会包含连接周期、平台消息 ID、会话/参与者 ID、重试次数和队列状态，但不会记录
聊天正文、媒体内容、Token、数据库密码或 QQ 开放平台 Secret。日志文件仍应按个人数据妥善保护。
