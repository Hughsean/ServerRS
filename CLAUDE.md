# 项目规则（Claude 必读）

> 本文件是项目的唯一核心规则入口。优先放在仓库根目录 `CLAUDE.md`，避免规则文件过多导致 Claude Code 未加载或加载不完整。

## P0：工作边界与变更安全

- 修改前先阅读与任务直接相关的代码、类型定义、调用方、配置和测试；不要只凭文件名猜测实现。
- 只解决用户明确提出的问题，保持 diff 小而可审查；不要把格式化、重构、依赖升级和业务修改混在一起。
- 保持现有 public API、数据库 schema、配置格式、序列化字段、路由、事件名、环境变量和错误码兼容；确需破坏性变更时必须说明影响和迁移方式。
- 不要删除看似无用但可能被外部调用、宏、反射、序列化、配置或 feature gate 使用的代码，除非已通过搜索和调用点确认。
- 不要提交临时调试输出、硬编码路径、密钥、token、账号、内部 IP、生产 URL 或本地环境假设。
- 不要自动执行破坏性操作，例如删除文件、清空目录、重置分支、覆盖配置、生产迁移、部署、推送或发布；除非用户明确要求并确认上下文安全。
- 不要自动 `git commit`；只有用户明确要求提交时才提交。

## P0：安全、配置与隐私

- 所有外部输入都必须校验，包括 HTTP 参数、路径参数、查询参数、JSON body、文件名、回调参数和第三方事件。
- 不要把用户输入直接拼接到 SQL、shell 命令、路径、URL、日志模板或 HTML 中；注意 SQL 注入、命令注入、路径穿越和 SSRF。
- 鉴权、授权、租户隔离和用户归属校验不能只依赖前端或调用方假设。
- 日志和错误响应不得泄露 access token、refresh token、cookie、authorization header、密码、验证码、私钥、完整手机号、身份证号、SQL、内部路径、堆栈或第三方敏感响应。
- 新增配置项时同步更新配置结构体、默认值、示例配置和文档；必填配置缺失应在启动阶段失败。
- 配置读取应集中在项目已有配置模块，不要在业务代码中到处读取环境变量。

## P1：架构分层

- 保持由外向内依赖：接口层调用应用层，应用层依赖领域抽象，基础设施层实现领域抽象。
- 领域层不得依赖 Web 框架、ORM、消息队列、HTTP 客户端、具体配置格式或外部 SDK。
- API/Handler 只负责请求解析、参数校验、认证上下文提取、调用服务和响应转换；不要写复杂业务流程。
- Service 负责用例编排、事务边界、权限检查、状态流转和领域服务调用；不要依赖 HTTP extractor 或直接读环境变量。
- Repository 只负责持久化读写和数据库模型映射；不要承担业务决策；不要把 ORM 查询对象、连接池或数据库内部 entity 泄漏到上层，除非项目已有明确抽象。
- 基础设施层负责数据库、缓存、文件系统、网络、外部 API、消息队列、向量库等具体实现。

## P1：启动装配边界

- `main.rs` 只负责加载配置、初始化日志、调用启动入口和记录顶层错误，目标保持在 50 行以内。
- `runtime::run(config)` 只表达启动顺序，不直接承载大量依赖构造细节，目标保持在 80 行以内。
- 不要在 `main.rs` 中直接构造 repository、service、worker、LLM provider、embedding provider、vector store、agent、bot、router 或定时任务。
- 启动装配应下沉到 `bootstrap` 或等价模块，例如：
  - `bootstrap::infra`：数据库、SSH tunnel、对象存储、外部客户端、LLM/Embedding provider。
  - `bootstrap::task_flow`：任务发布器、worker、限流、告警和周期任务。
  - `bootstrap::vector`：Qdrant、向量库、向量索引。
  - `bootstrap::services`：领域服务、RAG、Memory、Summary、Agent 上下文。
  - `bootstrap::api`：AppState、Router、中间件和 HTTP serve。
  - `bootstrap::qq_bot`：QQ Bot 仓库、TTS、bot 初始化和 feature gate 逻辑。
- 多个相关依赖应封装为 graph/context 结构体，例如 `RepoGraph`、`LlmGraph`、`VectorGraph`、`DomainServices`，不要在入口处散落大量 `Arc::clone`。
- feature gate 相关的大块初始化必须移入对应 bootstrap 模块，入口层只保留条件调用。

## P1：Rust 代码规则

- 遵循项目现有 Rust edition、lint、feature、模块组织和导入风格；提交前优先运行 `cargo fmt`。
- 不要引入未使用依赖、未使用 feature、未使用导入、死代码或无意义抽象。
- 优先用清晰的类型表达约束，避免用裸字符串、裸整数、`serde_json::Value` 穿透内部边界。
- 不要过度 `clone`；需要 clone 时确认所有权、生命周期和异步边界确实需要。
- async 上下文中不要执行阻塞 IO、长时间 CPU 计算，或持有同步锁、事务、连接、文件句柄跨 `.await`，除非已确认安全。
- 后台任务必须有取消语义、错误处理和日志；不要静默吞掉错误。
- 生产路径不要用 `unwrap()`、`expect()`、`panic!()` 处理可恢复错误；使用项目统一错误类型并保留排查上下文。
- 使用项目现有 `tracing`/日志方案；不要提交 `println!`、`dbg!`、临时 `eprintln!`。

## P1：依赖管理

- 添加、删除、升级依赖时优先使用生态包管理器命令，不要手改 lockfile。
- Rust 使用 `cargo add`、`cargo remove`、`cargo update -p <crate>`；修改后运行 `cargo check`，涉及 feature gate 时运行对应 `--features` 检查。
- JavaScript/TypeScript 先识别包管理器：`pnpm-lock.yaml` 用 pnpm，`yarn.lock` 用 yarn，`package-lock.json` 用 npm，`bun.lock`/`bun.lockb` 用 bun；不要混用。
- Python、Go 等生态遵循项目已有工具，例如 `uv`、`poetry`、`pip-tools`、`go get`、`go mod tidy`。
- 新增依赖必须说明用途、影响范围和运行时风险；依赖升级应与功能变更分开，除非任务明确要求。

## P2：测试、文档与交付说明

- 修改业务逻辑时优先补充单元测试或集成测试；修 bug 时尽量先补能复现问题的测试。
- 测试覆盖成功路径、失败路径、边界条件和权限/校验失败场景；不要只测 happy path。
- Rust 项目优先运行 `cargo fmt`、`cargo check`、`cargo test`；无法运行时说明原因和建议用户运行的命令。
- 新增命令、配置项、环境变量、接口、外部依赖或运行步骤时，同步更新文档、README 或示例配置。
- 交付时说明修改范围、关键决策、潜在风险和验证方式；发现邻近问题但未修改时单独列出。

## P2：中文注释与提交信息

- 新增或修改代码注释默认使用中文；注释解释“为什么”和约束，不重复代码字面含义。
- 公共 API、复杂业务逻辑、错误处理分支、并发/异步边界、兼容性处理、非显然算法应补充中文说明。
- TODO/FIXME/NOTE 标签可保留英文，但说明内容必须使用中文，例如 `// TODO: 补充边界条件测试`。
- 生成 Git commit message、PR 标题、PR 描述和变更摘要时默认使用中文。
- 如使用 Conventional Commits，类型前缀可保留英文，但描述必须是中文，例如 `feat: 增加状态栏 provider 请求统计`。
