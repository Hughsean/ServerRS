# NapCat 历史回补接口契约记录

> 验证日期：2026-07-23
> 范围：本机两个 NapCat 测试账号的只读历史接口，以及仅限获批测试群的主动消息和 WebSocket
> 契约；不记录账号、好友号、访问令牌或聊天正文。

## 官方契约

NapCat 官方接口列表声明支持：

- `get_msg(message_id)`；
- `get_friend_msg_history(user_id, count)`；
- `get_group_msg_history(group_id, count)`。

扩展接口文档还给出了私聊历史的 `message_seq`、`count` 和 `reverseOrder` 参数。

来源：

- <https://napneko.github.io/onebot/api>
- <https://napneko.github.io/develop/api/doc>

## 本机只读验证

两个本机 NapCat 实例均成功登录不同账号；两个账号互为好友且都在指定测试群中。在不输出实际
ID 和正文的前提下：

- `get_friend_msg_history` 成功返回历史页；消息包含 `self_id`、`user_id`、`time`、
  `message_id`、`message_seq`、`message_type`、`sender`、`raw_message`、`message` 和
  `post_type` 等字段；
- `get_group_msg_history` 成功返回历史页；除公共消息字段外还包含 `group_id`、`group_name`
  和群成员 `sender.role`；
- 请求接受字符串形式的会话 ID、`message_seq="0"`、`count=100` 和 `reverseOrder`；
- 相同参数连续读取返回相同账号内的消息 ID 序列；现有精确 `message_seq` 锚点会包含在结果中；
- 不存在的相邻 `message_seq` 返回 `failed/retcode=200`，不会自动寻找最近消息，因此回补不能
  通过对序号简单加减来翻页；
- `get_msg` 已使用群历史返回的真实消息 ID 完成同账号往返验证；同一消息 ID 交给另一个账号
  会失败，证明 NapCat 消息 ID 必须按账号主体解释，不能跨账号全局去重；
- 两个账号都能读取测试群和双方私聊历史，但本轮测试群每个账号仅返回一条、私聊各返回两条；
  不同账号返回的消息 ID 不重叠，排序方向也不一致，样本不足以定义跨账号统一顺序；
- 群消息包含可信 `sender.user_id/nickname/card/role`，消息对象包含 `message_id`、`message_seq`、
  `user_id`、`group_id` 和结构化 `message`；只读基线样本只有 `text` 段；
- 第一实例的真实 OneBot HTTP 只读客户端和 WebSocket 握手均通过自动测试；第二实例当前没有
  启用 OneBot HTTP/WebSocket 服务器，只能通过 WebUI 内部只读调试适配器验证；
- 第一实例基线配置的 `reportSelfMessage=false`，第二实例基线没有 OneBot 端点；主动验收只做
  临时变更，结束后两份配置均与快照逐字一致。

早期重复探测出现过成功空页，本次非空结果不能推翻该风险：接口成功仍不能单独证明某个时间窗
已完整覆盖。

## 获批测试群主动验证

仅在指定测试群发送并撤回带随机测试标记的消息，没有调用 `send_private_msg`。主动契约测试
完成以下验证：

- 两个账号的独立正向 WebSocket 均能并行接收同一群消息；发送账号收到 `message_sent`，另一
  账号收到普通 `message`，`is_self` 分类正确；
- 未 @ 消息、明确 @ 第一账号、第一账号本人消息均可实时解析；@ 目标进入结构化 `At` 段，
  `at_bot` 只在被提及账号视角为真；
- NapCat 消息 ID 是账号视角局部标识。账号 B 回复账号 A 的消息时，必须使用 B 接收到的父消息
  ID；回复到达 A 后，`Reply` 段自动转换成 A 视角的父消息 ID，并可用 A 的 `get_msg` 回查原文；
- 撤回后原始 WebSocket 收到 `notice_type=group_recall`，当前 Listener 会留下 Debug 日志但尚未
  把它映射成协议无关事件；
- 主动终止并重新建立第一账号 Listener 后，新群消息继续到达；这只证明连接可恢复，不证明
  断连空窗已补齐；
- 两个账号各自返回的主动测试历史页均包含完整标记，单页各 18 条，消息 ID 交集为 0；
  `get_msg` 同账号往返和精确 `message_seq` 包含式锚点再次通过；
- 所有测试消息在结束或失败清理路径中撤回，临时开启的本人消息上报和第二实例 WebSocket
  端点均恢复到原始配置。

实际 `qqbot-server` 还连接隔离 MySQL 完成了四条群消息入库：本人 1 条、外部 3 条、@ 1 条、
Reply 1 条；四条均关联连接周期，Reply 的 `reply_to_event_id` 成功解析，账号/会话两级游标均
推进。该验收只使用 QQBot 自有 `secretary_*` 表。

## 当前实现边界

`crates/qqbot::napcat::NapCatApiClient` 已移除个人账号发送、戳一戳和撤回能力，只公开读取
操作。历史消息 ID、序列号和账号 ID 统一解析为字符串，避免 64 位 ID 在其他语言或 JSON
链路中丢失精度；单页数量限制为 1–100。

`crates/qqbot/tests/napcat_live.rs` 是只读忽略型实机契约测试；
`crates/qqbot/tests/napcat_active_group_live.rs` 是必须显式配置两个账号和获批群号的主动测试，
源码只包含 `send_group_msg`，并在统一清理路径撤回自己发送的测试消息。

当前只完成类型化适配器，尚未启动自动回补 Worker。开始回补前仍必须继续验证：

1. 如何只使用真实存在的包含式锚点稳定翻页，以及何时没有“下一锚点”；
2. 正序/倒序在更多消息、两个账号视角和跨重启情况下的稳定含义；
3. 空页是确实无历史、缓存未加载、权限限制还是 PacketBackend 暂时不可用；
4. 免打扰群、群临时会话、私聊主动载荷和跨重启历史的覆盖范围；
5. NapCat 进程真实断线、网络抖动和服务重启期间的双 WebSocket 行为；
6. 如何证明一个 `IngestionGap` 可从 `uncertain` 转为 `verified_complete`。

在上述条件没有证据前，系统只能保存回补候选事件，不能宣称空窗已经补齐。
