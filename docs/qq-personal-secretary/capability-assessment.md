# 跨会话因果、主动跟进与结构化记忆能力审计

> 审计日期：2026-07-25
> 审计基线：`Main` 提交 `23c3333` + 未提交工作区（并发优雅关闭、可编程运行时入口、E2E 验收通过）
> 结论：目标可实现；当前已具备消息事实存储、连续性审计、历史回补、线程/结构记忆、承诺
> 跟进 Outbox、QQ 开放平台协议适配、可选有界 LLM 线程语义提取、类型化 Agent 动作安全底座、
> 并发优雅关闭（`RuntimeWorkers` + 25s 全局 deadline）和可编程运行时入口（`run_with_cancellation`）。
> 真实消息 E2E 验收已通过：SourceEvent->EventThread->LLM proposed 候选->精确来源证据链完整，
> 重启幂等性验证通过（scoped 计数不增加、游标不回退）。
> Owner 自然语言 Action Planner、跨线程检索与成组模型质量基准尚未完成。

## 1. 当前能力结论

| 能力 | 当前状态 | 代码事实 | 结论 |
|---|---|---|---|
| 群聊、私聊和本人消息接收 | 基础具备 | NapCat 已建模 `group`、`private`、`message_sent` 并通过有界 Worker 幂等保存；无 Token WebSocket `13991` 组合连接已通过 | 历史群聊/本人消息已实测；仍需一条新消息确认本轮自身消息上报和完整派生链 |
| NapCat 本地账号只读 | 服务端具备 | 运行时 HTTP Client 只公开读取，架构测试禁止服务端发送；主动发送仅存在于忽略型群测试 | 业务代码不能代表 Owner 发消息 |
| 判断消息回复了什么 | 已具备 | `Reply` 的账号视角 ID 已实测并解析为 `reply_to_event_id`；历史回补实现“子先父后”同账号回填 | 跨重启更大样本和非消息事件 Reply 路径仍待覆盖 |
| 讨论前因后果 | 基础具备 | 已有 `EventThread`/Claim/Decision/Question/Relation 类型和批量 MySQL 投影；Reply 优先、同会话短窗口次之 | 当前仅确定性线程骨架，尚未提取要求/分歧/结论 |
| 谁提出要求 | 基础具备 | 规则或可选 LLM 可批量生成 proposed 候选；LLM 发言人/来源必须映射到当前有界批次并经领域校验；本机 Qwen3 中文请求冒烟通过 | 复杂指代、跨线程检索和成组质量基准仍待验收 |
| 结论、分歧和话题结束 | 基础具备 | 已有结论候选、显式 supersedes 修订链、生命周期状态机和来源历史 | Owner 确认入口与自动结束条件仍未完成 |
| 同一事项跨群、跨私聊关联 | 基础具备 | 强项目 ID/文件 source_key 可生成带来源候选，Owner 可审核；相似文本不会自动合并 | 模糊指代与语义检索仍待受约束模型接入 |
| 服务器空窗恢复 | 已具备底座 | 已持久化 ConnectionEpoch、账号/会话 Cursor、uncertain Gap、回补运行与 Scope 进度；历史回补 Worker 已实现 | 真实 NapCat 无法证明账号会话集合完整，账号级 Gap 保持 uncertain；已知会话 Scope 可证回补完成 |
| 主动检查事项是否完成 | 基础具备 | 已确认承诺可进入持久化 Scheduler/Outbox，支持退避、送达和 UnknownCommit | 无人回复、工作时间与反馈策略待实现 |
| 人物/项目/承诺结构化记忆 | 基础具备 | 已有来源化类型模型、版本链、冲突拒绝和隐私边界 | 自动提取与自然语言控制待实现 |
| 记忆来源、修改、删除、TTL | 基础具备 | 可回读来源、修订、Owner 删除派生记忆并周期过期 | 彻底删除原文与索引清理待独立确认流程 |
| 自然语言控制和反馈 | 协议与动作底座具备 | QQ 开放平台 Owner 身份、类型化 Action 白名单和风险策略门已实现 | Planner、命令执行器与真实联机待完成 |
| Agent Graph、Effect、暂停恢复 | 业务底座具备 | 个人秘书已有有界状态和 L0-L3 策略；L2/L3 通过 NodeResult::Suspend；线程语义 Prompt 已有界装配 | Owner Action Planner、检索装配与通用执行节点待接入 |
| 跨进程 Checkpoint | 数字人已有实现 | 数字人业务有 MySQL Checkpoint Store | 不能直接共享数字人业务模型；可复用实现模式 |

因此，当前系统能把同账号 Reply 链和同会话短窗口批量投影为可追溯线程，但仍不能把跨群聊
消息建模为完整因果链，也不能主动跟进。历史回补和确定性线程骨架已落地；下一步应开发
类型化要求/分歧/结论提取与线程生命周期，不应直接跳到长期记忆或提醒。

## 2. 不能孤立处理每条消息的目标模型

### 2.1 原始事实层

`SourceEvent` 是不可变事实入口，至少保存：

- 本地账号主体、来源通道、平台消息 ID、事件类型；
- 会话、发送者、事件时间、接收时间；
- `reply_to_platform_message_id` 和解析后的 `reply_to_event_id`；
- 消息段、撤回/编辑关系、Artifact 引用；
- 数据保留策略和是否允许进入模型；
- 原始载荷引用及幂等键。

原始事实不被滚动摘要替代。任何结论、记忆或提醒都必须能回到一个或多个
`source_event_id`。

### 2.2 讨论线程层

`EventThread` 表示一个事项，而不是一个群：

```text
EventThread
├── participants              参与者及其会话内身份
├── source_event_ids          原始证据
├── claims                    提议、要求、事实陈述和反对意见
├── decisions                 已确认结论及修订链
├── open_questions            未解决问题和责任人
├── commitments               双方承诺及截止时间
├── status                    open/waiting/resolved/closed/reopened
├── conversation_ids          允许跨群、跨私聊
└── last_activity_at          用于跟进和结束判断
```

回复关系提供确定性因果边；相同人物、项目、文件、时间和语义只产生“候选关联”。跨会话
合并必须带置信度和来源，低置信度时请求 Owner 确认，不能为了生成顺畅摘要而静默合并。

### 2.3 结论与分歧

- `ThreadClaim`：谁在何时提出什么，类型为要求、提议、反对、确认或事实；
- `ThreadDecision`：最终结论、确认者、适用范围、来源和被哪个后续决定替代；
- `OpenQuestion`：仍未达成一致的点、候选方案、等待谁答复；
- 线程结束不能仅靠“模型觉得聊完了”，应结合明确确认、所有问题关闭、承诺状态和静默窗口；
- 已关闭线程收到新证据时进入 `reopened`，保留旧结论，不能覆盖历史。

## 3. 推荐处理循环

```text
接收并幂等落库
→ 解析回复、@、参与者和确定性实体
→ 按会话短窗口或小批次聚合
→ 检索现有线程、人物、项目和承诺
→ 生成类型化 Claim/Decision/OpenQuestion/Commitment 候选
→ 规则校验、冲突回读和低置信度确认
→ 增量更新结构化状态并保存来源
→ 由 Scheduler 检查截止、无人回复和未完成事项
```

高流量群不应每条消息调用一次 LLM。建议按回复链、当前线程和短时间窗口聚合；只有可能
形成要求、决定、承诺、风险或 Owner 查询时才进入模型。

## 4. 服务器不能 24 小时值守时的处理

### 4.1 必须接受的事实

- 只有 `qqbot-server` 退出：会持久化不确定空窗，但尚无历史回补，所以不能宣称空窗已补齐；
- NapCat 仍在线但业务服务退出：未来可由本地轻量接入进程先写持久化 Spool，再由服务消费；
- 电脑休眠、关机或 NapCat 离线：无法实时接收个人 QQ 消息，只能在恢复后尽力使用历史接口回补；
- 历史接口没有稳定锚点或无法证明完整性时，必须创建 `IngestionGap`，而不是假装没有遗漏。

### 4.2 目标恢复流程

1. 持久化 `ConnectionEpoch`、每会话 `IngestionCursor` 和最后成功事件时间；
2. 断连立即开启 Gap，恢复连接只结束实时空窗，不等于完成回补；
3. 使用最后稳定消息 ID/时间作为锚点，分页回补并走同一幂等入口；
4. 比对前后游标、时间范围和已知会话；
5. 能证明连续则关闭 Gap；不能证明则保持 `uncertain` 并通知 Owner；
6. 对重要项目和临近承诺执行一次恢复扫描。

推荐把 NapCat 接入进程配置为系统启动时自动运行，并增加本地磁盘 Spool。即使采用该方案，
电脑完全离线期间仍只能最佳努力回补，不能提供绝对不丢消息保证。

## 5. 主动跟进的正确实现

主动跟进不应依赖 Agent 在内存里持续思考，而应由持久化状态和确定性 Worker 触发：

- `Commitment`：承诺人、受益人、动作、截止时间、完成证据、状态和来源；
- `ResponseExpectation`：哪个问题在等待谁回复、服务时限和静默时长；
- `FollowUpRule`：提前量、工作时间、重要联系人、免打扰突破和升级策略；
- `FollowUpOccurrence`：一次检查或提醒，带唯一幂等键；
- `OutboxEntry`：只通过官方 Bot 提醒 Owner，失败可重试，`UnknownCommit` 不盲目重发。

例如“报价单两小时后截止”由 Scheduler 命中规则后形成候选提醒；“客户问题四小时无人回复”
需要确认线程中没有 Owner 的有效回复，再建议 Owner 处理。MVP 不通过 NapCat 自动催促负责人或
客户，也不替 Owner 作出承诺。

## 6. 结构化记忆与可控生命周期

### 6.1 人物记忆

`PersonMemory` 保存稳定身份、别名、与你的关系、职责权限、沟通偏好和历史承诺。群昵称、
私聊身份和开放平台 OpenID 不自动合并，必须通过稳定映射或 Owner 确认。

### 6.2 项目记忆

`ProjectMemory` 保存目标、关键成员、进展、已确认决策、风险阻塞和文件版本。每个字段使用
独立的 `MemoryFact`，允许新证据替代旧事实而不删除修订历史。

### 6.3 承诺记忆

`Commitment` 分别记录 Owner 和对方的承诺，包含兑现时间、状态、完成证据和来源。它既是
记忆，也是主动跟进的事实基础。

### 6.4 用户控制

所有长期记忆必须支持：

- `show_sources(memory_id)`：查看原始来源；
- `correct_memory(memory_id, patch)`：保存修订及修改者；
- `delete_memory(memory_id)`：从工作记忆和检索索引删除，按隐私策略处理原始内容；
- `set_expiry(memory_id, expires_at)`：到期后不再进入 Prompt；
- `set_conversation_policy(conversation_id, memory_mode)`：`normal`、`local_only`、
  `envelope_only` 或 `never_long_term`。

删除派生记忆和删除原始聊天是两个动作，必须分别确认。系统可以保留不含正文的最小审计
记录，但不得借审计名义继续把已删除内容提供给模型。

## 7. 自然语言控制

Owner 的自然语言必须先解析成受控 Action，再由业务服务验证权限和执行。例如：

| 用户表达 | 类型化 Action |
|---|---|
| 今天老板找过我吗 | `SearchEvents(person, time_range)` |
| 总结项目群上午讨论 | `SummarizeThread(conversation, time_range)` |
| 哪些消息需要我回复 | `ListPendingResponses(owner)` |
| 列出付款消息 | `SearchEvents(topic=payment)` |
| 这个群只提醒有人 @ 我 | `UpdateConversationPolicy` |
| 家人消息晚上也立即播报 | `UpdateFollowUpRule` |
| 这类通知不重要 | `RecordImportanceFeedback` |
| 把今天所有承诺整理成待办 | `MaterializeCommitmentsAsTasks` |
| 删除关于此事的记忆 | `DeleteMemory`，先展示范围并确认 |

只有官方 Bot Owner 控制会话能产生这些 Action。NapCat 消息即使内容写着“删除全部记忆”或
“立即转账”，仍然只是 Observation。

## 8. 安全和隐私不变量

- 跨会话理解只服务 Owner，绝不把一个会话内容发送到另一个原始会话；
- 私密会话策略在检索前过滤，不能依赖 Prompt 提醒模型自觉忽略；
- 重要结论、人物合并、日期、金额和承诺必须保存来源；
- LLM 只提出候选状态补丁或类型化 Action，不能直接写库、发消息或创建定时器；
- 自动回复第三方不属于 MVP；主动跟进默认只提醒 Owner；
- 记忆更正、删除、策略变更和跨身份绑定必须审计。
