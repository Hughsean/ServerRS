# QQBot 独立验收基础设施

这里定义个人 QQ 智能秘书的机器可检查验收门禁。业务 Agent 的交付报告不能直接修改
PASS/FAIL；结果由 `scripts/verify-qqbot-acceptance.ps1` 根据测试发现和实际退出码生成。

## 核心规则

1. P0/P1 的 `required_for_merge=true` 项必须全部 PASS。
2. 测试文件或测试名称不存在时为 MISSING，不能被“0 tests”当作成功。
3. 需要 MySQL 的测试必须使用隔离 schema。脚本默认在本地 Docker MySQL 中创建随机
   `qqbot_accept_*` schema，结束后删除。
4. Fake 或纯领域测试最多提供 L1/L2 证据，不能满足要求 L4/L5 的生产闭环。
5. 测试失败、缺失、被阻塞都拒绝合并。`-AllowExpectedFailures` 仅用于建立红灯基线，
   不得用于 CI 或合并验收。
6. 报告写入 `target/qqbot-acceptance/`，不会修改业务文档或 Git 状态。
7. L4/L5 仅能由受保护 CI 显式传入的**仓库外** attestation 提升。它必须绑定 clean
   candidate 的 `git_head`、tree hash、矩阵和测试文件 SHA-256、仓库外验收运行报告的
   SHA-256、受保护签发者及签名/验证 claim；缺任一字段时证据最多为 L3。

## 运行方式

在仓库根目录执行：

```powershell
./scripts/verify-qqbot-acceptance.ps1
```

首次建立红灯基线或查看全部缺口：

```powershell
./scripts/verify-qqbot-acceptance.ps1 -AllowExpectedFailures
```

只检查矩阵结构和测试是否存在，不执行测试：

```powershell
./scripts/verify-qqbot-acceptance.ps1 -ListOnly -AllowExpectedFailures
```

若不使用默认 Docker 容器，可传入一个专用的隔离 schema。为了防止误伤，URL 的数据库
名称必须以 `qqbot_accept_` 开头：

```powershell
./scripts/verify-qqbot-acceptance.ps1 `
  -DatabaseUrl "mysql://user:password@127.0.0.1:3306/qqbot_accept_ci_123"
```

## 输出

脚本生成：

- `target/qqbot-acceptance/latest.json`：机器读取结果；
- `target/qqbot-acceptance/latest.md`：人类评审摘要；
- `target/qqbot-acceptance/logs/*.log`：每个检查的完整输出。

状态含义：

| 状态 | 含义 |
|---|---|
| PASS | 测试存在且本次真实执行通过 |
| FAIL | 测试存在但执行失败 |
| MISSING | 测试文件或精确测试名称不存在 |
| BLOCKED | 缺少隔离 MySQL 等必要环境 |
| NOT_RUN | `-ListOnly` 模式下测试存在但未运行 |

## 修改约束

业务实现任务不得删除、重命名、忽略或放宽本目录矩阵及其验收测试。若需求确实变化，必须
单独提交矩阵变更，并由独立评审确认后再修改业务代码。

`.github/workflows/qqbot-acceptance.yml` 会在 QQBot、personal-secretary、验收矩阵或脚本
变化时运行同一门禁，并上传 14 天有效的验收证据。关键验收文件目前没有仓库账号级
`CODEOWNERS`，因为项目尚未声明可用的 GitHub reviewer；启用分支保护时应把该工作流设为
required check，并为这些文件配置独立 reviewer。

## 外部 GitHub 配置（尚未配置）

仓库中的 workflow YAML 不能单独建立 L4/L5 attestation 的信任边界。当前仓库**尚未确认**以下
GitHub 管理侧配置，不能把它们记录为已完成：

1. 为 attestation 签发任务建立受保护的 GitHub Environment，并限制可使用该 Environment 的
   分支、审批者与部署权限；私钥只能保存在该 Environment 的 Secret 中。
2. 在受保护 runner 上配置固定的、仓库外的可信 RSA 公钥路径，以及
   `QQBOT_ACCEPTANCE_TRUSTED_ISSUER`；私钥、PEM、attestation 和运行报告均不得进入 checkout、
   日志或 artifact。
3. 在目标分支 protection/ruleset 中把 GitHub 展示的
   `QQBot Acceptance Gate / acceptance` 设为 required check，并限制绕过权限；同时为 verifier、
   helper、验收矩阵、验收测试与 workflow 设置独立评审规则。

配置完成前，本地和普通 CI 运行只能提供其实际执行的 L1/L3 证据；即使测试通过，也不能伪装为
受保护环境签发的 L4/L5 证明。
