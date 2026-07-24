# 个人 QQ 智能秘书开发历史索引

> 本文件只做导航和当前阶段摘要；具体事件进入 `history/` 归档。
> 新事件必须精确到 `YYYY-MM-DD HH:mm（Asia/Shanghai）`，缺少可信分钟的旧事件不得猜测回填。

## 当前阶段

- 开发来源分支：`claude/qqbot-history-backfill`，安全检查点为 `995e291`；后续提交与 Main 合并
  状态以 Git 历史为准。
- 当前能力：可靠入站、空窗回补、确定性 EventThread、类型化语义、跨会话关联候选、Owner
  关联审核、高影响线程变更的持久化 Suspend/Resume、授权撤销、语义失效，以及来源化人物/
  项目/承诺结构记忆、证据回读、Owner 派生记忆删除、承诺提醒 Outbox、独立 QQ 开放平台
  协议适配与类型化 Agent 动作策略门。
- 当前边界：NapCat 保持只读；QQ 开放平台代码已接入但未使用真实凭据联机。已暴露的旧 Secret
  必须轮换，App ID/新 Secret/Owner OpenID 尚未安全配置。
- 下一开发项：轮换并本地配置 QQ 开放平台凭据，完成 Owner-only 联机冒烟；随后接入受约束的
  Planner/Retriever/Executor 节点与自然语言控制。

## 历史分块

| 时间范围 | 主题 | 记录 |
|---|---|---|
| 2026-07-23～2026-07-24 | 个人秘书立项、NapCat 验证、可靠入站、Gap 回补、线程与 Owner 审核 | [2026-07 归档](history/2026-07.md) |

## 最近事件

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
