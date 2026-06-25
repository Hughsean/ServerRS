# 依赖管理规则

## 基本原则

- 更新、添加、删除依赖时，优先使用对应生态的包管理器命令，不要直接手改依赖文件或锁文件。
- 依赖文件和锁文件应由工具生成或更新，避免手动编辑造成版本约束、锁文件、workspace 元数据不一致。
- 修改依赖前先确认项目使用的包管理器、workspace 结构、lockfile 类型和现有版本约束风格。
- 依赖升级应与功能修改分开，除非本次任务明确要求同时修改。
- 不要为了绕过冲突直接删除锁文件；只有在用户明确要求或项目文档要求时才重新生成锁文件。

## Rust / Cargo

- 添加依赖使用 `cargo add`，不要直接编辑 `Cargo.toml`。
- 移除依赖使用 `cargo remove`，不要只删除 `use` 或手动删 `Cargo.toml` 条目。
- 更新依赖使用 `cargo update` 或 `cargo update -p <crate>`。
- 修改 feature 时优先使用 `cargo add <crate> --features ...` 或按项目既有命令方式处理；手动调整前必须确认 workspace 和 feature 约束。
- 修改后运行 `cargo check`；涉及 feature gate 时运行对应 `--features` 检查。

示例：

```bash
cargo add serde --features derive
cargo remove unused-crate
cargo update -p tokio
cargo check
```

## JavaScript / TypeScript

- 先识别项目实际使用的包管理器：`pnpm-lock.yaml` 对应 pnpm，`yarn.lock` 对应 yarn，`package-lock.json` 对应 npm，`bun.lockb` 或 `bun.lock` 对应 bun。
- pnpm 项目使用 `pnpm add`、`pnpm remove`、`pnpm update`。
- npm 项目使用 `npm install`、`npm uninstall`、`npm update`。
- yarn 项目使用 `yarn add`、`yarn remove`、`yarn up` 或项目指定命令。
- bun 项目使用 `bun add`、`bun remove`、`bun update`。
- 不要混用包管理器；不要在 pnpm 项目里生成 `package-lock.json`，也不要在 npm 项目里生成 `pnpm-lock.yaml`。

示例：

```bash
pnpm add zod
pnpm remove lodash
pnpm update @types/node
```

## Python / Go / 其他生态

- Python 项目按现有工具使用 `uv add/remove`、`poetry add/remove/update`、`pip-tools` 或项目文档指定命令，不要随意手改锁文件。
- Go 项目使用 `go get`、`go mod tidy`、`go get module@version`，不要手动改 `go.sum`。
- 其他生态遵循项目已有包管理器和锁文件生成流程。

## 审查要求

- 依赖变更后检查 diff，确认只出现预期的 manifest、lockfile 和必要源码变更。
- 新增依赖必须说明用途、影响范围和是否引入运行时风险。
- 升级依赖必须注意破坏性变更、feature 变化、安全修复和间接依赖变化。
- 不要把依赖升级、格式化大面积文件、重构和业务修改混在一个不可审查的 diff 中。
