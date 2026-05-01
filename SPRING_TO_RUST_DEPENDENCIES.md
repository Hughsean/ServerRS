# Server(Spring) 迁移到 ServerRS(Rust) 依赖建议（Axum + Tokio + SeaORM）

## 说明
- 生成时间: 2026-04-22
- 数据来源: crates.io API（联网实时查询）
- 选型原则:
  - 优先 Axum/Tokio 生态
  - 数据库层优先 SeaORM（避免直接在业务层使用 sqlx）
  - 仅推荐稳定版（非 alpha/rc）用于生产
  - 优先近 12 个月仍有发布的包，避免长期不更新依赖
  - 覆盖当前 Spring 项目的核心能力：Web、认证与安全、MySQL、邮件、异步任务、定时清理、外部 HTTP 调用、网页抓取、日志与监控

## 必选依赖（首批迁移）

| crate | 推荐稳定版 | 用途（对应 Spring） | 活跃度（稳定版发布时间） |
|---|---:|---|---|
| axum | 0.8.9 | HTTP 路由/提取器/中间件体系，替代 spring-boot-starter-web | 2026-04-14（8天前） |
| tokio | 1.52.1 | 异步运行时、任务调度、定时任务基础（替代 @Async / @Scheduled 执行底座） | 2026-04-16（6天前） |
| tower-http | 0.6.8 | CORS、Trace、Timeout、压缩等中间件（对应 WebConfig/Security 周边能力） | 2025-12-08（135天前） |
| serde | 1.0.228 | 序列化/反序列化（替代 Jackson 的对象映射） | 2025-09-27（207天前） |
| serde_json | 1.0.149 | JSON 读写（替代 Jackson JSON 节点/对象处理） | 2026-01-06（106天前） |
| validator | 0.20.0 | 请求 DTO 字段校验（非空、长度、邮箱、正则等，配合 serde 反序列化） | 2025-01-20（457天前） |
| sea-orm | 1.1.20 | MySQL 异步 ORM 与实体关系建模（替代 MyBatis + JDBC） | 2026-03-31（22天前，最新 2.0 为 rc） |
| jsonwebtoken | 10.3.0 | JWT 生成与校验（替代 java-jwt） | 2026-01-27（85天前） |
| bcrypt | 0.19.0 | 密码哈希与校验（与现有 BCrypt 口令兼容） | 2026-03-03（50天前） |
| rsa | 0.9.10 | RSA 公私钥加载与 OAEP 解密（对应登录 RSA 加密链路） | 2026-01-06（106天前，最新 0.10 为 rc） |
| reqwest | 0.13.2 | 外部 HTTP 调用（天气/IP/LLM/新闻等插件调用） | 2026-02-06（75天前） |
| scraper | 0.26.0 | HTML 解析与正文抽取（替代 Jsoup） | 2026-03-18（36天前） |
| lettre | 0.11.21 | SMTP 邮件发送（替代 spring-boot-starter-mail） | 2026-04-04（18天前） |
| tracing | 0.1.44 | 结构化日志埋点 | 2025-12-18（125天前） |
| tracing-subscriber | 0.3.23 | 日志格式化、过滤、输出配置 | 2026-03-13（40天前） |
| config | 0.15.22 | 配置加载（支持 yml/env 分层，替代 application.yml 绑定） | 2026-03-17（36天前） |
| thiserror | 2.0.18 | 业务错误类型定义 | 2026-01-18（94天前） |
| anyhow | 1.0.102 | 应用层错误聚合与上下文 | 2026-02-20（62天前） |
| chrono | 0.4.44 | 日期时间处理 | 2026-02-23（58天前） |
| uuid | 1.23.1 | 会话 ID/请求 ID 生成 | 2026-04-16（7天前） |

## 可选依赖（按功能启用）

| crate | 推荐稳定版 | 何时需要 | 活跃度 |
|---|---:|---|---|
| axum-extra | 0.12.6 | 需要 typed-header、cookie、更丰富 extractor 时 | 2026-04-14（8天前） |
| sea-orm-migration | 1.1.20 | 数据库迁移管理（推荐替代手写 migration SQL 管理） | 2026-03-31（22天前） |
| sea-orm-cli | 1.1.20 | 生成/执行迁移与实体代码（开发工具） | 2026-03-31（22天前） |
| governor | 0.10.4 | 登录防爆破/接口限流（对应 LoginAttemptTracker 一类能力） | 2025-12-16（127天前） |
| moka | 0.12.15 | 高并发内存缓存（会话态、短期风控结果缓存） | 2026-03-22（32天前） |
| metrics | 0.24.3 | 统一指标采集（Actuator 指标替代方案之一） | 2025-11-28（145天前） |
| metrics-exporter-prometheus | 0.18.1 | 暴露 Prometheus 指标端点（/metrics） | 2025-12-07（136天前） |
| lapin | 4.5.0 | 需要 RabbitMQ 异步风控队列时（对应 docs/RABBITMQ_INTEGRATION.md） | 2026-04-18（4天前） |

## 建议的 Cargo 依赖片段（可直接作为起点）

```toml
[dependencies]
axum = { version = "0.8.9", features = ["macros", "multipart", "http2"] }
tokio = { version = "1.52.1", features = ["rt-multi-thread", "macros", "signal", "time", "sync", "fs"] }
tower-http = { version = "0.6.8", features = ["cors", "trace", "timeout", "compression-br", "request-id", "limit"] }

serde = { version = "1.0.228", features = ["derive"] }
serde_json = "1.0.149"
validator = { version = "0.20.0", features = ["derive"] }

sea-orm = { version = "1.1.20", default-features = false, features = ["macros", "runtime-tokio-rustls", "sqlx-mysql", "with-chrono", "with-uuid"] }
# 若需要在当前工程内直接执行迁移，可启用：
# sea-orm-migration = { version = "1.1.20", default-features = false, features = ["runtime-tokio-rustls", "sqlx-mysql"] }

jsonwebtoken = "10.3.0"
bcrypt = "0.19.0"
rsa = { version = "0.9.10", features = ["pem", "pkcs8"] }

reqwest = { version = "0.13.2", default-features = false, features = ["json", "rustls-tls", "gzip", "brotli"] }
scraper = "0.26.0"
lettre = { version = "0.11.21", default-features = false, features = ["tokio1-rustls-tls", "smtp-transport", "builder"] }

tracing = "0.1.44"
tracing-subscriber = { version = "0.3.23", features = ["env-filter", "fmt", "json"] }

config = { version = "0.15.22", default-features = false, features = ["yaml"] }
thiserror = "2.0.18"
anyhow = "1.0.102"
chrono = { version = "0.4.44", features = ["serde"] }
uuid = { version = "1.23.1", features = ["v4", "serde"] }
```

## 不建议直接采用的点（本次筛选中）
- 避免直接追预发布版本:
  - sea-orm 2.0.0-rc.38（建议先用稳定版 1.1.20）
  - rsa 0.10.0-rc.17（建议先用稳定版 0.9.10）
- argon2 当前最新是 rc 分支，稳定版发布时间较早；若你需要与现有 Spring BCrypt 数据平滑兼容，优先保留 bcrypt。

关于 validator 的使用建议:
- 虽然 validator 最近一次稳定发布较早，但在 Rust Web 生态中仍是最通用的结构化校验方案。
- 建议只在输入边界层（HTTP DTO）使用 validator，避免把校验逻辑散落到 service/repository。

补充说明:
- SeaORM 底层依赖 sqlx 作为驱动层是正常现象（传递依赖），但业务代码不需要直接使用 sqlx API。

## 迁移映射小结
- spring-boot-starter-web -> axum + tower-http
- spring-boot-starter-security + java-jwt -> jsonwebtoken + tower-http(中间件) + bcrypt + rsa
- spring-boot-starter-jdbc + mybatis + mysql-connector-j -> sea-orm(mysql) + sea-orm-migration
- spring-boot-starter-mail -> lettre
- jsoup -> scraper
- @Async / @Scheduled -> tokio::spawn + tokio::time
- actuator -> tracing + metrics(+prometheus exporter)

## 实施计划（强解耦 + 任务流）
1. 分层边界重建
  - 将系统切分为 api 层（Axum handler）、application 层（用例编排）、domain 层（规则与策略）、infrastructure 层（SeaORM/HTTP/MQ/邮件）。
2. 输入边界统一校验
  - 所有请求先进入 DTO（serde + validator），校验失败直接在 api 层返回统一错误，不进入业务用例。
3. 用例化编排替代 Controller 直连 Service
  - 每个业务场景定义独立 UseCase（如 CreateSession、DetectRisk、SendAlert），handler 只做参数转换与调用。
4. 大任务拆分为任务流
  - 对重任务（风险检测、邮件通知、外部抓取）采用“提交任务 -> 队列/调度 -> Worker 执行 -> 结果回写”的流程，不使用单个同步长链路。
5. 领域事件驱动解耦
  - 关键节点发布事件（MessageReceived、RiskDetected、HighRiskConfirmed），由订阅者处理后续动作，减少模块间直接依赖。
6. 可替换基础设施接口
  - 在 application/domain 定义 trait（Repository、Notifier、RiskEngine、Fetcher），SeaORM/SMTP/RabbitMQ 作为实现注入，便于测试与替换。
7. 先纵切后横扩
  - 先完成一条完整纵向链路（例如 登录 + JWT + 用户查询），稳定后再逐模块并行迁移，避免一次性大爆炸重构。
