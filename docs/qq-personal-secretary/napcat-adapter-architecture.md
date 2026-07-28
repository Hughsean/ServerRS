# NapCat 适配器架构（Continuity Recall v1）

> 分支：`glm/qqbot-continuity-recall-v1`。本文件说明 `crates/qqbot` 协议适配层与
> `apps/qqbot-server` 运行时分层在 Continuity Recall v1 阶段的结构。
> **状态：B4/B3/B6/B7 领域层+基础设施+运行时装配完成**，单元测试+clippy+fmt+build+
> workspace_boundaries 全绿；MySQL 集成测试受 action_planner_hardening 既有 DROP CHECK
> 问题阻塞（非本轮引入）；实机 NapCat 13990/13991 当前在线。
> NapCat 在本阶段保持只读：不得加入发送、撤回、禁言、踢人、点赞、设精华、标记已读等写操作。
> 当前为本机无 Token 模式（HTTP `127.0.0.1:13990`，WebSocket `127.0.0.1:13991`）。

## 1. 模块结构

```text
crates/qqbot/src/napcat/
├── mod.rs              # 模块入口与 re-export
├── api.rs              # 只读 HTTP 客户端 NapCatApiClient（固定只读 allowlist）
├── capabilities.rs     # B5 能力/版本探测，类型化 CapabilitySnapshot
├── error.rs            # NapCatError（含 HeartbeatTimeout 类型化错误）
├── event.rs           # MessageSegment / 事件类型（B2 扩展段类型 + B3 撤回事件）
├── heartbeat.rs       # B1 OneBot Heartbeat/Lifecycle 监控状态机
├── listener/          # 正向 WebSocket 监听器（从 listener.rs 990 行拆分）
│   ├── mod.rs         # NapCatListener + run_forward 三态 deadline 驱动循环
│   ├── transport.rs   # WS 建连、单条帧读取、Ping/Pong/Close
│   ├── dispatch.rs    # 帧边界 + JSON + meta_event/notice/message 路由
│   ├── message_event.rs # 消息事件 DTO 解析与 Group/PrivateMessage 构造
│   ├── notice_event.rs  # 通知事件 DTO 解析（含 group_recall/friend_recall）
│   └── bounds.rs      # 帧/raw_event/字段有界与 actor ID 校验
├── message_parser.rs  # CQ raw 回退解析与 normalize_text
└── segments.rs         # B2 结构化 message 数组优先解析

crates/personal-secretary/src/
├── directory.rs       # B4 账号会话目录领域模型
├── directory_service.rs # B4 目录同步用例与端口
├── recall.rs          # B3 消息撤回领域模型（RecallEvent/Tombstone/CorrelationKey）
├── recall_service.rs  # B3 撤回用例与端口
├── artifact.rs        # B6 富消息 Artifact 信封领域模型
├── artifact_service.rs # B6 Artifact 用例与端口
├── health.rs          # B7 健康状态四态与快照
├── health_service.rs  # B7 健康聚合器与有界缓存
└── infra/repo/
    ├── mysql_directory.rs  # B4 MySQL 目录快照仓储
    ├── mysql_recall.rs     # B3 MySQL 撤回仓储
    └── mysql_artifact.rs   # B6 MySQL Artifact 仓储

apps/qqbot-server/src/
├── config/             # A1 拆分 + 新增 directory_sync 配置段
├── runtime/            # A2 拆分 + handlers 新增撤回路径
├── bootstrap/          # A2 拆分 + thread_pipeline 装配 DirectorySyncWorker
├── directory_sync.rs  # B4 目录同步 Worker + NapCat DirectorySourceT 适配器
├── recall.rs          # B3 撤回事件处理器
└── ...
```

## 2. 消息解析优先级（B2）

1. 优先解析 OneBot 结构化 `message` 数组（`segments::parse_structured_segments`）。
2. 结构化字段不存在 / 非数组 / 为空时，回退 CQ raw parser（`message_parser::parse_message_segments`）。
3. 结构化与等价 CQ 字符串生成等价 canonical segment。
4. ID 字段兼容字符串与数字，进入内部后转换成明确类型。
5. 强制上限：段数 `MAX_SEGMENTS`、单段文本 `MAX_SEGMENT_TEXT_CHARS`、元数据 `MAX_META_CHARS`、
   总字节 `MAX_MESSAGE_TOTAL_BYTES`，超长按字符截断。
6. 未知段保留类型名与有界 raw，不静默删除、不存无限大小。
7. `raw_message` 有界截断后保留作审计信息；`normalized_text` 不是唯一事实来源。
8. 段类型：text / at / reply / face / image / record / video / file / forward / rich(json/xml/card) / unknown。

## 3. Heartbeat / Lifecycle（B1）

- 类型化解析 `post_type=meta_event`，`meta_event_type=heartbeat|lifecycle`。
- `HeartbeatConfig`：启动宽限、interval 上下界、超时倍数；拒绝 0/负数/溢出/异常巨大值。
- `HeartbeatState`：维护最后协议心跳时间、最后业务事件时间、声明 interval、生命周期状态
  （Connected → LifecycleReceived → Heartbeating → Ended）。
- `listener.run_forward` 用 `tokio::select!`（biased）同时监听 WebSocket 消息、Heartbeat deadline、
  关闭信号；超时返回 `NapCatError::HeartbeatTimeout`。
- 运行时映射 `ConnectionEndReason::HeartbeatTimeout`，结束 ConnectionEpoch、只创建一个 Gap、唤醒 Backfill、
  进入有上限指数退避无限重连（不因有限次数退出）。
- 关闭信号在任何等待点都能抢占，不被 watchdog 阻塞。
- 高频 Heartbeat 不持久化、不打 info 日志（只用 trace）。
- 普通文本流量只更新业务时间戳，不重置 Heartbeat deadline，不掩盖已启用的超时。
- 旧版本兼容：默认宽松启动宽限（60s）+ 超时倍数（3x），不因未见到首个 Heartbeat 就热重连。

## 4. 能力探测（B5）

- `api.rs` 只读方法固定 allowlist：`get_login_info` / `get_status` / `get_version_info` /
  `get_friend_list` / `get_recent_contact` / `get_group_list` / `get_group_member_info` /
  `get_group_member_list` / `get_group_msg_history` / `get_friend_msg_history` / `get_msg`。
- `CapabilitySnapshot::probe` 类型化探测实现/版本、Heartbeat、结构化消息、recent/friend/group/
  history API、forward/file/record 元数据、在线状态。
- API 不存在时按功能降级，不致命；关键缺失有结构化 warning。
- 不通过动态字符串开放任意 NapCat Action；不调用任何写接口。
- `workspace_boundaries` 测试强制只读 allowlist 与新接口覆盖。

## 5. 运行时连接循环（A2 + B1 集成）

`runtime::connection_loop::run_connection_loop` 每次连接：
1. `begin_connection` 建立 ConnectionEpoch（失败回收 Worker）。
2. 装配本轮 ingestion Worker + listener（注入 HeartbeatConfig）+ observer。
3. `tokio::select!`：shutdown → `ProcessShutdown`；listener 返回 →
   `Ok`=`RemoteClosed`，`Handler`=`ObserverRejected`，`HeartbeatTimeout`=`HeartbeatTimeout`，其余=`TransportError`。
4. 排空 ingestion Worker（超时 abort + 回收）。
5. `finish_connection` 产生 Gap，唤醒 Backfill。
6. 关闭则退出；否则有上限指数退避无限重连（shutdown 可抢占）。

## 6. 安全边界（不变）

- NapCat 只读，无写操作。
- 无 `NAPCAT_HTTP_TOKEN` / WebSocket Token / URL 查询凭据。
- 不向任何 QQ 会话主动发送消息；唯一获批测试群 `671260344`（仅被动读取）。
- 不启用 QQ 开放平台通道。
- 日志不记录完整消息正文、Token、数据库密码。

## 7. 待续

- B4 账号会话目录与历史完整性证据（接 B5 的只读会话发现 API）。
- B3 消息撤回闭环（持久化/关联/失效/Retriever 过滤）。
- B6 富消息 Artifact 引用（有界 envelope，当前已部分体现在段类型与 Rich envelope）。
- B7 健康状态与结构化日志（类型化 NapCat 健康快照）。
- MySQL 集成测试与实机 NapCat 验收真实运行。
