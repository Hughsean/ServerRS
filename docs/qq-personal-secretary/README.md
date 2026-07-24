# 个人 QQ 智能秘书项目文档

> 文档入口版本：V1.0
> 最后更新：2026-07-24
> 当前开发工作树：`claude/qqbot-history-backfill`（尚未提交或合并 Main）

本目录是个人 QQ 智能秘书业务的长期文档入口，用于保留需求演进、能力审计、开发
Todo、历史记录和未来规划。后续新增的 QQBot/个人秘书项目文档应放在本目录或其子目录，
不再散落到仓库根目录，也不放入已被 `.gitignore` 忽略的 `docs/design`。

## 文档索引

| 文档 | 用途 |
|---|---|
| [capability-assessment.md](capability-assessment.md) | 当前能力、缺口、目标数据模型、空窗恢复和主动跟进设计 |
| [TODO.md](TODO.md) | 唯一执行清单；记录优先级、依赖、状态和验收标准 |
| [HISTORY.md](HISTORY.md) | 已完成开发、重要验证、数据库影响和 Git 落点 |
| [napcat-history-contract.md](napcat-history-contract.md) | NapCat 历史接口、双账号测试群主动契约、真实入库证据和未决风险 |

仓库根目录现有的 `qq_*requirements*.md` 是用户创建且尚未纳入 Git 的需求草案。本次不
移动、不删除、不覆盖；待用户确认归档策略后，再复制到本目录的 `requirements/` 子目录。

## 状态约定

- `DONE`：代码和验收均完成；必须在 `HISTORY.md` 留证据。
- `IN PROGRESS`：当前开发切片。
- `TODO`：已确认但尚未开发。
- `BLOCKED`：缺少外部服务、凭证、产品决策或实机证据。
- `DEFERRED`：明确不属于当前阶段。

## 更新规则

每个开发切片结束时必须同时更新：

1. `TODO.md`：完成项、下一项和阻塞条件；
2. `HISTORY.md`：新增事件按 `YYYY-MM-DD HH:mm（Asia/Shanghai）` 记录，包含分支/提交、
   实际改动、数据库影响和验证结果；缺少可信时间的旧记录不得猜测分钟；
3. 如果产品或架构结论变化，更新 `capability-assessment.md` 并写明替代关系；
4. 文档不得写入 QQ token、Bot Secret、数据库密码、聊天正文或个人隐私数据；
5. “已实现”只能依据当前代码和测试，不以规划、接口名称或模型能力推断。
