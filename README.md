# ServerRS QQBot

本仓库只包含个人 QQ 智能秘书。运行链路由 `qqbot-server`、独立 MySQL 和宿主机 NapCat 组成。

## Docker Compose 一键启动

前置条件：Docker Desktop 已启动，NapCat 已在宿主机登录并提供 HTTP/WebSocket 服务。

1. 将 `.env.example` 复制为 `.env`，填写数据库密码、QQ 号和两个不同的 Spool 密钥。
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

Compose 使用命名卷 `serverrs-qqbot_qqbot-mysql-data` 与
`serverrs-qqbot_qqbot-spool-data`。`docker compose down` 不删除数据；不要对生产实例执行
`docker compose down -v`。MySQL 首次创建卷时自动加载 Schema Baseline v2，后续升级必须按
`qqbot-server/database/README.md` 执行增量迁移，重建镜像不会重放基线。

MySQL 默认映射到宿主 `3307`，仅供本机运维；QQBot 容器通过 Compose 内部网络连接数据库。
`.env` 中的数据库密码建议使用随机十六进制字符串，避免 URL 保留字符需要额外编码。

停止服务但保留数据：

```powershell
docker compose down
```

## 本地开发

QQBot 配置与环境变量说明见 `qqbot-server/config/README.md`，数据库基线与升级规则见
`qqbot-server/database/README.md`。
