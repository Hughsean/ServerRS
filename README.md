# ServerRS QQBot

本仓库只包含个人 QQ 智能秘书。运行链路由 `qqbot-server`、独立 MySQL 和宿主机 NapCat 组成。

## Docker Compose 一键启动

前置条件：Docker Desktop 已启动，NapCat 已在宿主机登录并提供 HTTP/WebSocket 服务。

1. 将 `.env.example` 复制为 `.env`，填写数据库密码、管理员密码、NapCat 登录 QQ 号和两个不同的 Spool 密钥。
2. 可用以下 PowerShell 命令分别生成两个密钥，命令需要执行两次：

   ```powershell
   [Convert]::ToBase64String([Security.Cryptography.RandomNumberGenerator]::GetBytes(32))
   ```

3. 启动：

   ```powershell
   docker compose up -d --build
   docker compose logs -f qqbot
   ```

默认假定宿主 NapCat HTTP 为 `3001`、WebSocket 为 `6701`。容器使用 `socat` 把它们转发到容器
回环地址 `127.0.0.1:3000/6700`，因此不会放宽 QQBot 现有的 loopback 安全校验。若端口不同，修改
`.env` 中的 `NAPCAT_HOST_HTTP_PORT` 和 `NAPCAT_HOST_WS_PORT`。

历史 Backfill 默认关闭。需要临时启用时，必须同时设置
`QQBOT_BACKFILL_EARLIEST_DATE=YYYY-MM-DD`（北京时间）；早于当天零点的历史消息不会入库，命中
日期下界后停止继续翻页并挂起对应 Gap，避免周期性重扫。

Compose 使用命名卷 `serverrs-qqbot_qqbot-mysql-data` 与
`serverrs-qqbot_qqbot-spool-data`。`docker compose down` 不删除数据；不要对生产实例执行
`docker compose down -v`。MySQL 首次创建卷时自动加载 Schema Baseline v2；后续每次启动由
一次性 `qqbot-migrate` 服务在 QQBot 启动前按字典序执行尚未登记的增量迁移，不会重放基线、删除
卷或改写已登记迁移。

若启用 QQ 开放平台交互，`QQBOT_OPEN_PLATFORM_OWNER_OPENID` 必须填写 Owner 手机 QQ 对该机器人
产生的 C2C `user_openid`，不能填写数字 QQ 号。NapCat 继续只读观测个人 QQ；只有该 OpenID 的
C2C 消息，或同一 OpenID 在群内明确 @Bot 的消息，会成为 `OwnerCommand` 并收到秘书回复；普通
群消息不会触发。若平台不能证明群内身份与 Owner OpenID 一致，则保持只观察、不回复。

管理员页面默认位于 `http://127.0.0.1:8080`，只映射宿主回环地址。登录后从 NapCat 只读获取
当前账号加入的群聊，并可运行期增删群观察白名单；修改持久化在 Spool 命名卷中。群白名单默认
为空并拒绝全部群，NapCat 私聊始终全部观察。管理员密码只从 `QQBOT_ADMIN_PASSWORD` 读取，要求
12～256 字节且不得与数据库或 API 密钥复用。

MySQL 默认映射到宿主 `3307`，仅供本机运维；QQBot 容器通过 Compose 内部网络连接数据库。
`.env` 中的数据库密码建议使用随机十六进制字符串，避免 URL 保留字符需要额外编码。

停止服务但保留数据：

```powershell
docker compose down
```

Compose 为 QQBot 设置 60 秒停止宽限。官方平台与生命周期通知启用时，服务启动后会向 Owner
发送“秘书已上线”；收到 `SIGTERM` 后先发送“秘书正在安全下线”并排空 Worker。使用
`docker compose stop qqbot` 或 `docker compose down`，不要使用不会触发优雅关闭的 `docker kill`。

## 本地开发

QQBot 配置与环境变量说明见 `qqbot-server/config/README.md`，数据库基线与升级规则见
`qqbot-server/database/README.md`。
