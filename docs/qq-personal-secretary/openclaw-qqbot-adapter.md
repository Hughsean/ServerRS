# OpenClaw QQBot 参考适配与 Agent Runtime 边界

> 核对时间：2026-07-24 21:30（Asia/Shanghai）
> 范围：只记录公开协议事实、项目内实现选择和安全边界；不记录真实凭据。

## 参考来源

- OpenClaw QQBot 文档：<https://docs.openclaw.ai/channels/qqbot>
- 腾讯维护的 OpenClaw QQBot 插件：<https://github.com/tencent-connect/openclaw-qqbot>
- OpenClaw channel CLI：<https://github.com/openclaw/openclaw/blob/main/docs/cli/channels.md>

参考插件使用 MIT License。本项目只复用公开协议结论并以 Rust 独立实现，没有复制其运行时或
JavaScript 业务结构。

## 已确认的协议事实

- CLI 的组合 Token 表示 `AppID:AppSecret`。
- Access Token 通过 `POST https://bots.qq.com/app/getAppAccessToken` 获取。
- 官方 API 基址为 `https://api.sgroup.qq.com`，请求使用 `Authorization: QQBot <access_token>`。
- C2C 消息目标为 `/v2/users/{openid}/messages`，群目标为
  `/v2/groups/{group_openid}/messages`。
- Gateway 从 `/gateway` 获取地址，使用 Identify/Resume、Heartbeat/ACK 和 sequence 恢复。
- App ID、access token、Gateway session 和 OpenID 必须按 Bot 账号隔离，不能跨账号推断身份。

## 项目内实现

`qq-open-platform` 是纯协议适配器：

- 凭据类型的 Debug 输出永远遮蔽 Secret；
- 每个 App 实例独立缓存 Token，并对并发刷新做 singleflight；
- HTTP 有连接/请求超时，401 只刷新一次 Token；
- Gateway 只申请当前消费的群/C2C 与互动 Intent；
- 心跳缺少 ACK 会断开，由宿主指数退避重连；
- 标准化事件和原始 JSON 均持久化成功后才推进 Resume sequence；
- 只输出类型化 C2C/群事件，不依赖数据库、个人秘书或 NapCat。

`qqbot-server` 负责基础设施装配：MySQL Gateway session/raw event、Owner OpenID 绑定、入站身份
映射和 Outbox 投递。通知领取按被管理账号过滤，不能让 Bot A 消费 Bot B 的消息；外部 POST
结果不明进入 `unknown_commit`，不盲目重试。

## 凭据边界

- TOML 只允许 App ID、Owner OpenID 和 Secret 文件路径；明文 `client_secret` 字段直接拒绝。
- Secret 只允许来自 `QQBOT_OPEN_PLATFORM_CLIENT_SECRET` 或 Git 忽略的本地文件。
- Token、Secret、聊天正文和完整外部错误体不进入日志。
- 已在聊天、截图、终端历史或提交中暴露的 Secret 必须在 QQ 开放平台轮换，旧值不得用于上线。

## Agent Runtime 约束

依据项目补充设计，Agent 只提出白名单类型化 Action，服务负责校验、执行、审计和回执。工作
状态只保存有界目标、固定约束、近期事件引用、证据引用和一个待处理 Proposal；完整原始轨迹
保留在外部事件日志，不保存完整自然语言思维过程。

当前动作按风险分为：

- L0：只读检索、来源回读、线程查询、指代解析和近期事项；
- L1：可逆草稿；
- L2：创建/改期/取消日程、任务和提醒，必须带幂等键并暂停确认；
- L3：对外发送 Owner 消息，必须带幂等键并暂停确认，提交结果不明不得自动重放。

动作集合中不存在任意 SQL、HTTP、Shell、文件系统或 NapCat 发送入口。下一步是在此策略门之后
实现 Retriever/Planner/Executor 节点，而不是让模型直接调用基础设施。
