# 个人 QQ 智能秘书开发历史索引

> 本文件只做导航和当前阶段摘要；具体事件进入 `history/` 归档。
> 新事件必须精确到 `YYYY-MM-DD HH:mm（Asia/Shanghai）`，缺少可信分钟的旧事件不得猜测回填。

## 当前阶段

- 当前分支：`Main`，当前提交为 `23c3333`；QQBot 运行数据库使用独立容器、独立数据库和
  独立持久化卷，不复用数字人数据库。
- 当前能力：可靠入站、空窗回补、确定性 EventThread、类型化语义、跨会话关联候选、Owner
  关联审核、高影响线程变更的持久化 Suspend/Resume、授权撤销、语义失效，以及来源化人物/
  项目/承诺结构记忆、证据回读、Owner 派生记忆删除、承诺提醒 Outbox、独立 QQ 开放平台
  协议适配、类型化 Agent 动作策略门，以及可选 OpenAI-compatible/Ollama 有界线程语义提取。
- 当前边界：NapCat 保持只读；本地 QQBot MySQL 8.4 运行库已建成并通过 TLS、迁移幂等和应用
  连接验收。QQ 开放平台本地凭据可以完成鉴权和 Gateway 地址读取，但官方通道仍关闭，Owner
  OpenID 尚未通过真实 C2C 入站事件确认；本机 Ollama `qwen3:14b` 已完成真实结构化补全与提示
  注入边界验收。NapCat 已使用本机无 Token 的 HTTP `13990` 与 WebSocket `13991`，真实状态接口、
  WebSocket 握手和正式服务组合启动均通过；`6099` 仅作为 WebUI。已在聊天中暴露的 QQ 开放平台
  Secret 仍应轮换后再上线。
- 下一开发项：完成真实消息入站→持久化→线程投影→LLM 候选证据链；随后接入受约束的
  Planner/Retriever/Executor 节点与 Owner 自然语言控制。

## 历史分块

| 时间范围 | 主题 | 记录 |
|---|---|---|
| 2026-07-23～2026-07-24 | 个人秘书立项、NapCat 验证、可靠入站、Gap 回补、线程与 Owner 审核 | [2026-07 归档](history/2026-07.md) |

## 最近事件

- `2026-07-25 18:17（Asia/Shanghai）`：E2E 最终验收通过（36.54s）；清理非白名单群 338 条历史数据；下一阶段 Owner Retriever / Action Planner。
- `2026-07-25 17:05（Asia/Shanghai）`：群白名单、E2E RAII 守卫加固、跨扫描周期重启稳定性与文档修正。
- `2026-07-25 13:27（Asia/Shanghai）`：真实消息入站闭环 E2E 验收通过（187.60s，含 LLM 退避重试）。
- `2026-07-25 12:47（Asia/Shanghai）`：并发优雅关闭、可编程运行时入口与真实 E2E 验收骨架。
- `2026-07-25 10:44（Asia/Shanghai）`：QQBot 环境变量覆盖改为四类窄宏，消除配置解析样板代码。
- `2026-07-25 10:35（Asia/Shanghai）`：移除 NapCat Token 配置，HTTP 13990/WebSocket 13991 无 Token 组合验收通过。
- `2026-07-25 10:22（Asia/Shanghai）`：NapCat `6099` 端口复验，确认其为 WebUI 而非 OneBot WebSocket；HTTP/Ollama 正常，MySQL 已恢复健康。
- `2026-07-24 22:42（Asia/Shanghai）`：Ollama Qwen3 实机语义与提示注入边界验收通过。
- `2026-07-24 22:29（Asia/Shanghai）`：完成 QQBot 独立有界 LLM 线程语义提取垂直切片。
- `2026-07-24 22:11（Asia/Shanghai）`：建立独立持久化 QQBot MySQL 运行库并完成 45 表验收。
- `2026-07-24 21:37（Asia/Shanghai）`：OpenClaw QQBot 参考适配、账号隔离 Outbox 与 Agent Runtime 安全骨架。
- `2026-07-24 21:00（Asia/Shanghai）`：记忆控制、冲突回读与承诺提醒 Scheduler/Outbox 闭环。
- `2026-07-24 16:54（Asia/Shanghai）`：线程撤销、语义失效与来源化结构记忆多维批次。
- `2026-07-24 16:24（Asia/Shanghai）`：线程变更持久化 Checkpoint/Suspend/Resume 与逻辑执行闭环。
- `2026-07-24 14:33（Asia/Shanghai）`：历史分块、SeaORM 审核 CRUD 与线程变更 Suspend 预览。
- `2026-07-24 14:19（Asia/Shanghai）`：Owner 关联审核与跨主体授权闭环。
- `2026-07-24 14:10（Asia/Shanghai）`：跨群/私聊线程关联候选闭环。
- 旧事件仅有日期证据，保留原日期，不伪造分钟。

## 分块规则

- 每个归档按月份建立；达到 500 行或 100 KiB 时，再按日期和业务切片拆分。
- 归档内容按时间顺序追加，完成事项必须同步更新 `TODO.md`。
- 每条记录必须包含完成范围、数据库影响、外部系统影响、验证、Git 状态和下一项。
- 根索引只保留当前阶段和最近 10 条事件，避免无限增长。
