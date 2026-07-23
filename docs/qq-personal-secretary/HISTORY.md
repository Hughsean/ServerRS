# 个人 QQ 智能秘书开发历史

> 最后更新：2026-07-23
> 本文件只记录已经发生且有证据的事项；未来计划统一放在 `TODO.md`。

## 2026-07-23：产品方向切换为个人智能秘书

- 形成 V2.0 需求草案：NapCat 作为个人 QQ 全消息感知通道，QQ 开放平台作为 Owner 控制与
  提醒通道，知识库延后。
- 明确 NapCat 在 MVP 中只读，不代表 Owner 自动回复任何联系人或群。
- 明确采用有界结构化状态、最近窗口和按需来源检索，而不是反复重放全部历史。
- 文档状态：需求草案仍位于 Main 根目录且未跟踪，本次未移动或覆盖。

## 2026-07-23：首个统一身份和消息入口切片

- 分支/工作区：`codex/qq-personal-secretary`，隔离工作树
  `.worktrees/qq-personal-secretary`。
- 新增 `crates/personal-secretary`，定义消息来源、账号主体、会话、可信发送者、消息角色和
  幂等键。
- 指令不变量：只有 QQ 开放平台 Owner 控制会话中的已验证 Owner 是 `OwnerCommand`；
  NapCat 中 Owner 自己发送的内容是 `OwnerObservation`。
- NapCat 新增私聊和 `message_sent` 事件；群聊行为保持兼容。
- `qqbot-server` 将 NapCat 消息映射到统一身份边界，不记录消息正文，不发送消息，不写数据库。
- 数据库影响：无迁移、无表变更、无现有数据读写。
- 外部验证：本地 NapCat WebSocket 实机握手成功；未进行真实聊天正文采集。
- 自动验证：个人秘书、QQ 适配器和服务端聚焦测试 24 项通过，架构边界 9 项通过；格式化和
  差异检查通过。
- 已知基线：严格 Clippy 被 `qqbot/napcat/api.rs` 7 个既有 `needless_question_mark` 告警
  阻断；本次范围在仅豁免该规则后通过。
- Git 状态：截至本记录，改动尚未提交或合并。

## 2026-07-23：因果链、主动跟进和结构化记忆审计

- 确认 Reply ID 目前只停留在协议消息段，尚未解析为持久化事件因果边。
- 确认当前没有 SourceEvent、EventThread、Decision、OpenQuestion、Commitment、Scheduler、
  Secretary Memory 或 Owner 自然语言 Action。
- 确认 `agent-core` 的 Graph、Effect、Checkpoint、Suspend/Resume 可以复用，但尚未建立个人
  秘书业务状态和节点。
- 建立 `docs/qq-personal-secretary/` 作为后续 QQ 智能秘书项目文档的正式入口。
- 建立能力审计、分阶段 Todo 和本历史记录；下一开发切片确定为可靠事件存储。

## 2026-07-23：独立配置与可靠消息持久化切片

- QQBot 配置入口独立为 `apps/qqbot-server/config/qqbot.toml`，示例和本地 `.env` 也只位于
  该目录；仅使用 `QQBOT_CONFIG_PATH`、`QQBOT_DATABASE_URL` 和 QQBot/NapCat 专属变量。
- QQBot 数据库迁移独立位于 `apps/qqbot-server/database/migrations`；新增四张
  `secretary_*` 表，不修改数字人的 `database/sql/init.sql` 和迁移目录，不复用旧 `qq_*` 表。
- 新增协议无关的消息内容段、@成员、@全体和 Reply 建模，并将 NapCat 载荷映射到个人秘书域。
- 新增 `InboundEventStoreT` 与 SeaORM/MySQL 实现；消息按“来源账号 + 平台消息 ID”幂等落库，
  同一平台消息 ID 在不同账号主体下保持独立。
- `qqbot-server` 改为先落库：新事件才允许进入后续处理，Duplicate 不重复处理。
- Docker MySQL 8 隔离验收通过：首条插入、重复去重、Reply 因果边、@成员、@全体和跨账号
  隔离均验证成功。
- 未完成：连接周期、Cursor、Gap、历史回补、有界队列/背压、Reply 待回填和非消息事件。
- Git 状态：截至本记录，改动仍位于隔离工作树，尚未提交或合并。

## 2026-07-23：连接周期、游标与不确定空窗切片

- 新增协议无关的 `ConnectionEpochId`、连接状态/结束原因、`IngestionCursorScope` 和
  `IngestionGapStatus`；连接健康和历史完整性是两个独立状态。
- 新增独立连续性迁移，建立连接周期、实时事件来源关联、账号/会话游标和空窗四张
  `secretary_*` 表；没有修改数字人数据库或根 `init.sql`。
- NapCat WebSocket 握手成功前先创建连接周期；握手成功后持久化 `connected`，消息落库时
  在同一事务更新连接最后事件、账号游标和会话游标。
- 远端断开、传输失败和进程退出都会结束周期；曾经成功连接的周期创建
  `IngestionGap(status=uncertain)`。下次连接只写入空窗结束时间，不会把状态改成已补齐。
- QQBot 的 SeaORM 运行时启用 Rustls；隔离 MySQL 使用 `ssl-mode=required` 完成 TLS 验收。
- Docker MySQL 8 验收：两份迁移可重复执行，消息幂等/Reply/@ 测试与连接周期/双层游标/
  空窗幂等测试连续两轮通过。
- 未完成：NapCat 历史 API 契约、回补 Worker、Gap 完整性判定、有界队列/背压和本地 Spool。
- 平台门槛：进入 QQ 开放平台连接阶段前必须先通知用户并确认凭据只保存在 QQBot 本地配置。
- Git 状态：截至本记录，改动仍位于隔离工作树，尚未提交或合并。

## 2026-07-23：NapCat 历史接口只读收口与契约探测

- 对照 NapCat 官方接口文档和本机在线实例，验证群聊/私聊历史接口可返回标准消息、发送者、
  `message_id` 和 `message_seq` 字段；验证过程没有输出或保存实际聊天正文及联系人 ID。
- 新增 `get_group_msg_history`、`get_friend_msg_history` 和 `get_msg` 类型化读取方法；ID 全部
  按字符串解析，历史单页限制为 1–100。
- 从 NapCat HTTP 客户端删除个人账号发送、戳一戳等修改能力，并增加源码架构守卫。
- 发现重复探测可能返回成功空页；因此只完成读取适配器，尚未启动自动回补，也没有把任何
  `IngestionGap` 标记为 `verified_complete`。
- 新增 `napcat-history-contract.md`，记录官方契约、本机证据和锚点/分页/PacketBackend 未决项。
- QQ 开放平台仍未开始接入；到达该阶段前继续执行“先通知用户、再配置本地凭据”的门槛。

## 2026-07-23：有界入站队列、背压与可调试日志切片

- `qqbot-server` 的 NapCat 回调改为只做类型映射和非阻塞 `try_send`；MySQL 幂等写入从
  WebSocket 回调移到独立有界 `mpsc` Worker。
- Worker 对临时数据库错误执行有上限的指数退避，单条消息成功或确认为重复后再消费下一条；
  无效事件停止重试并留下可审计错误。
- 队列满时不阻塞 WebSocket，聚合丢弃计数并为当前连接周期幂等创建
  `IngestionGap(status=uncertain, reason=queue_overflow)`，不把无法证明的连续性伪装成完整。
- 连接结束时限时排空 Worker；超时会中止内存队列并依赖不确定空窗与后续历史回补。当前尚无
  本地磁盘 Spool，因此数据库长期离线叠加进程崩溃仍可能丢失尚未落库的事件。
- 新增独立 QQBot 队列容量、重试初始/上限和退出排空超时配置；不读取数字人配置。
- 新增 `trace/debug/warn/error` 结构化日志，覆盖入队、持久化尝试、幂等结果、重试、溢出 Gap
  和 Worker 排空；日志不记录聊天正文或凭据。
- 数据库影响：没有新增迁移或修改数字人表；复用 QQBot 自己的连续性表记录溢出 Gap。
- 验证：个人秘书/QQ 适配器/QQBot 服务端聚焦测试 31 项通过；隔离 MySQL 真实集成测试 2 项
  通过；跨业务架构边界 13 项通过；严格 Clippy、格式和差异检查通过。数据库重试恢复、队列满
  立即返回和容量配置上下限均有自动测试。
- QQ 开放平台仍未开始接入。

## 2026-07-23：双 NapCat 实例只读契约复验

- 两个本机 NapCat 实例均验证为不同的在线测试账号；双方互为好友，并同时属于指定测试群。
- 通过 WebUI 内部临时调试适配器对两个账号执行只读 OneBot 调用；没有发送消息、撤回、退群、
  修改网络配置或输出聊天正文及真实账号 ID。
- 双方的好友列表、群列表、群成员、群历史和私聊历史读取均成功；群历史重复读取保持稳定。
- 精确 `message_seq` 是包含式锚点；不存在的相邻序号返回失败，不能用序号加减实现可靠翻页。
- `get_msg` 在消息所属账号内往返成功，跨账号查询同一消息 ID 失败；这为“来源账号 + 平台消息
  ID”幂等键提供了实机证据。
- 新增忽略型 `napcat_live` 契约测试；第一实例的真实 OneBot HTTP 读取和 WebSocket 握手 2 项
  通过。第二实例尚未配置 OneBot HTTP/WebSocket 服务器。
- 第一实例当前关闭本人消息上报；本轮测试群/私聊历史样本较少且只有文本段，因此 @、Reply、
  本人消息、撤回、断连和多页稳定翻页仍不能标记完成。
- 数据库影响：无。QQ 开放平台仍未开始接入。

## 2026-07-23：双账号测试群主动契约与真实入库验收

- 获得仅在指定测试群发送主动测试消息的授权；测试代码没有 `send_private_msg`，没有向好友或
  其他群发消息。
- 临时为第一实例开启本人消息上报，并为第二实例增加独立 WebSocket；每次运行都先保存完整
  配置，`finally` 清理后重新读取验证，两份配置均与原快照逐字一致。
- 新增忽略型 `napcat_active_group_live` 契约测试，覆盖未 @、明确 @、本人消息、Reply、撤回
  通知、Listener 重建、双账号历史、`get_msg` 和精确历史锚点；最终 1 项实机测试通过。
- 发现并验证 NapCat 消息 ID 按观察账号分域：回复方必须使用自己收到的父消息 ID，接收方的
  `Reply` 会转换为其账号视角的 ID。双账号各 18 条历史样本的消息 ID 交集为 0。
- 撤回通知已确认存在，但当前 Listener 只记录“尚未建模”的 Debug 日志；该缺口登记为
  `EVT-008`，没有把撤回误报为已经持久化。
- 启动真实 `qqbot-server` 连接一次性 MySQL 8：8 张 QQBot 自有 `secretary_*` 表成功迁移；
  四条测试群消息全部入库，其中 Owner 1、External 3、@ 1、Reply 1，Reply 父事件解析成功，
  四条均关联连接周期，账号/会话两级游标均存在。
- 运行期使用 `trace/debug/info/warn` 结构化日志；日志未记录聊天正文、访问令牌或 WebUI 凭据。
- 最终回归：个人秘书/QQ 适配器/QQBot 服务端 31 项通过，跨业务隔离守卫 13 项通过；严格
  Clippy、格式化和差异空白检查通过。顺带等价化简 6 处只读 API 的冗余 `Ok(...?)`。
- 所有主动测试消息均通过发送账号撤回，真实服务进程已停止，NapCat 配置已恢复；一次性
  MySQL 容器和本次主动验收构建目录均已删除。
- QQ 开放平台仍未开始接入。

## 后续记录模板

```text
## YYYY-MM-DD：切片名称

- 分支/提交：
- 完成范围：
- 未完成范围：
- 数据库影响：表、迁移、兼容与回滚
- 外部系统影响：NapCat、QQ 开放平台、模型、定时器
- 验证：测试命令、通过数量、实机场景
- 文档变化：
- 下一项与阻塞：
```
