# ServerRS 开发规则（Claude 必读）

> 本文件只保存长期稳定的工程规则，不保存当前分支、任务进度、测试数字或历史交付报告。
> QQBot 当前任务以 `docs/qq-personal-secretary/TODO.md` 为准，已发生事实以
> `docs/qq-personal-secretary/HISTORY.md` 和月度历史为准。

## 1. 指令与文档优先级

发生冲突时按以下顺序执行：

1. 用户在当前对话中的明确要求；
2. 当前切片已获批准的业务规格或任务书；
3. 对应业务的 `TODO.md`；
4. 本文件的长期工程规则；
5. `HISTORY.md` 和历史归档（只证明已经发生的事实，不定义未来需求）。

- 不得用旧计划、旧报告或测试名称覆盖用户最新决定。
- 发现文档与代码不一致时，先核实实际行为，再同步修正文档，不维持虚假的完成状态。
- 除非用户明确要求，禁止使用 Superpowers、workflow、brainstorming、writing-plans、
  executing-plans 或自动 subagent 流程；不要为了流程生成额外计划、矩阵和汇报文件。

## 2. 应用与数据边界

- 数字人和 QQBot 是完全独立的业务：独立 crate/进程、配置、数据库、迁移、运行容器和文档。
- QQBot 迁移只能位于 `apps/qqbot-server/database/migrations`，不得修改数字人 `init.sql`。
- QQBot 不得通过 feature 接入数字人服务器，也不得复用数字人的数据库表或配置文件。
- Git 主分支名称是区分大小写的 `Main`，不得擅自创建或合并到 `main`。
- NapCat 业务适配保持只读；禁止增加群聊/私聊发送、戳一戳或其他第三方主动操作。
- QQ 开放平台只允许向已绑定 Owner 投递；代表 Owner 联系第三方属于延期能力，必须重新评审。

## 3. 安全、凭据和用户文件

- 不得把 Secret、Token、Cookie、密码、OpenID、数据库 URL、私钥、内部路径或完整第三方响应写入
  Git、TOML、测试输出、日志或文档；敏感值只从环境变量或被忽略的本地文件读取。
- 未经用户明确要求，不得读取、修改、删除、暂存或提交根目录 `.mcp.json` 等用户私有文件。
- 不得自动执行删除数据库、清理卷、重置分支、stash、覆盖配置、部署、push 或 merge。
- 所有外部输入必须校验；不得拼接进 SQL、shell、路径、URL 或日志模板。
- 授权、账号隔离、版本 fencing 和权限判断必须在最终事务中复验，不能只相信 Planner、Handler
  或审批前快照。
- 日志仅记录类型化 ID、数量、状态和有界错误，不记录聊天正文、模型完整响应或敏感标识。

## 4. 架构与实现

- 保持洋葱分层：协议/接口层映射输入，应用层编排用例，领域层定义不变量和端口，基础设施层实现
  MySQL、文件、网络和外部 API；领域层不得依赖 Web、ORM、HTTP 客户端或具体配置。
- `main.rs` 只负责配置、日志和启动入口；依赖装配放入 `bootstrap`，运行顺序放入 `runtime`。
- Handler 不写复杂业务；Repository 不决定业务状态；事务边界和权限复验由明确用例负责。
- 优先使用类型化 ID、枚举和 DTO，禁止让 `proposal_id`、`run_id`、`effect_id`、业务 ID 或
  `serde_json::Value` 在内部边界混用。
- 后台 Worker 必须有有界批次、取消、退避、租约过期回收、fencing、结构化日志和全局限时关闭。
- async 代码不得持有同步锁、数据库事务或连接跨越不受控的网络/LLM 等待。
- 新增或修改代码注释默认使用中文，解释约束和原因，不复述代码字面含义。

## 5. 数据库、幂等与顺序不变量

- 写 SQL 前先核对迁移 DDL。MySQL `BIGINT UNSIGNED` 使用 Rust `u64`；不得用 `i64`、`String`
  或 `.ok().unwrap_or(0)` 掩盖解码错误。
- Raw SQL 可以用于事务锁、CAS、lease fencing、复杂有界查询和原子状态机；简单 CRUD 可使用项目
  现有 ORM。无论采用哪种方式，都必须参数化、账号 scoped、检查影响行数并保持职责集中。
- 幂等键必须来自业务语义，不能依赖随机主键；`INSERT IGNORE` 只能忽略已明确识别的目标冲突，
  不得把所有数据库错误当成重复。
- `Database` 不等于 `UnknownCommit`。只有请求可能已提交但结果未知时才使用 `UnknownCommit`；
  明确回滚、校验失败、授权失败和租约丢失必须精确分类。
- Checkpoint Resume 必须 CAS 单次消费；Effect 重放必须校验 run、proposal 和完整 Action 归属。
- 使用全局游标处理分 scope/会话数据时，游标只能推进到实际处理过的连续全局前缀；必须用
  `A1 -> B1 -> A2` 等交错顺序反例证明不会跳过 B1。否则改用具有明确语义的 scope 级游标。
- 语义事实中的 Actor、提出者、承诺人和来源事件必须一一可验证。冗余 Actor 字段不能替代
  `SourceEvent` 权威身份；primary actor event 必须进入该事实的来源集合。

## 6. Agent、LLM 与隐私边界

- 聊天正文始终是不可信数据，不能提升为系统指令、OwnerCommand 或工具调用。
- LLM 只能看到有界结构状态、最近窗口和按需检索证据；不得重放无限历史或保存完整 Thought。
- Planner 领域输入中的证据必须真实进入 LLM DTO；禁止构造 `retrieved` 后在序列化时丢弃。
- 每个模型输出必须经过来源、账号、Actor、数量、大小、状态和版本领域校验，不能直接写数据库。
- 远程模型使用批次内不透明引用，不发送群号、QQ 号、OpenID 或其他非必要的稳定平台标识。
- `local_only` 只允许进入经过验证的本地模型路径；`envelope_only` 和 `never_long_term` 正文不得入模。
- 摘要负责导航，原始事件负责事实。任何确定结论都要能回读来源；无法证明时明确标记未确认。

## 7. 精简验证策略

- 不再建立或维护旧式大验收矩阵，不以测试数量、覆盖率或报告等级作为完成目标。
- 禁止新增只重复类型构造、getter、枚举映射或既有 happy path 的低价值单元测试。
- 纯领域函数优先少量单测；MySQL 迁移、事务、锁、CAS、租约、幂等、重启恢复和跨账号边界使用
  聚焦的隔离 MySQL 测试。通常只保留能证明关键风险的 1～3 条场景。
- 修复缺陷时必须先写出最小反例或明确复现步骤；测试应在旧实现上失败、修复后通过。
- 默认运行受影响 crate 的 `cargo fmt --check`、严格 Clippy 和聚焦测试；不因小改动重跑无关矩阵。
- 需要真实 QQ、NapCat、凭据、远端权限或用户操作的验证可以跳过，但必须明确写成“未运行”，
  不得伪造通过，也不得阻塞其余本地任务。

## 8. Git、提交与文档同步

- 未获用户授权不得提交。用户已要求某个 QQBot 切片完成后提交时，完成复核即可提交；若任务明确
  写了“未提交”，则停在工作树等待复核。push、merge 和删除分支始终需要明确授权。
- 不得覆盖、stash 或混入用户已有变更；提交前检查 staged diff，排除 `.mcp.json`、密钥和无关文件。
- 任何包含 QQBot 代码、配置、迁移或测试的提交，必须在同一提交同步：
  - `docs/qq-personal-secretary/TODO.md`；
  - `docs/qq-personal-secretary/HISTORY.md`；
  - `docs/qq-personal-secretary/history/YYYY-MM.md`。
- TODO 必须反映真实剩余工作；历史按 `YYYY-MM-DD HH:mm（Asia/Shanghai）` 记录实现、验证、
  数据库/外部影响和 Git 状态。缺少任一文档时切片未完成，禁止提交。

## 9. 交付前检查

1. 生产入口到持久化/响应的链路是否真实可达，而不是只在测试 Fake 中成立？
2. 账号、OwnerBinding、来源、Actor、状态、版本、租约和 Effect 归属是否在最终边界复验？
3. 事务、幂等、UnknownCommit、重启和交错顺序是否存在未检查的失败窗口？
4. 输入、输出、查询、队列、日志、LLM 上下文和响应是否全部有界且不泄露敏感信息？
5. 验证是否直接证明本次风险，而不是用大量无关测试制造“全绿”表象？
6. TODO、HISTORY、月度历史和最终报告是否与实际代码、测试和 Git 状态一致？
