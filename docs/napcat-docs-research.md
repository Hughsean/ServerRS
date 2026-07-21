# NapCatQQ 官方文档研读笔记

> 研读时间：2026-06-16
> 最后核对：2026-07-21（仅核对本项目集成方式，未重新抓取官方站）
> 文档站：https://napneko.github.io/
> 目标：记录当时对 NapCatQQ / OneBot 11 的研读结论。官方文档版本会变化，升级 NapCat 前应重新核对官方站。

## 当前 ServerRS 对接方式

- NapCat 是独立的 `qqbot-server` 应用，不再是数字人服务器的 Cargo feature。
- `crates/qqbot` 只保留 HTTP API、正向 WebSocket、CQ 解析和类型化协议事件。
- 连接参数来自独立 `qqbot.toml` 的 `[napcat]`，也可通过 `NAPCAT_*` 环境变量覆盖。
- 当前占位 handler 不回复、不持久化；旧 QQBot 业务与数据库代码已经删除。
- 后续业务只需实现 `NapCatEventHandler`，不需要修改协议适配器。

---

## 一、文档站点结构

文档站使用 VitePress 构建，完整导航侧边栏如下：

| 分类 | 页面 |
|------|------|
| **快速开始** | 目录导航、什么是 NapCatQQ、启动方式、Shell 安装、Framework 安装 |
| **配置** | 基础配置（WebUI/文件配置）、高级配置（FFmpeg/PacketBackend）|
| **使用** | 接入框架、社区资源 |
| **开发** | 请求接口、上报事件、消息类型、处理文件、插件开发（含 API 参考）|
| **API 文档** | 版本选择（研读时记录为 v4.18.6，OpenAPI 规范；当前版本需以官方站为准）|
| **协议** | 协议概述、网络通讯、事件基础结构、事件字段详情、基础接口、消息元素定义、差异实现说明 |
| **其余** | 喵喵、安全、联系 |

> **注意**：配置页面的基础配置 /config/basic 无法直接通过 WebFetch 获取，但已在本地保存 HTML 副本。

---

## 二、网络通讯模型（OneBot 11）

### 2.1 HTTP 通信

| 模式 | 描述 | 场景 |
|------|------|------|
| **HTTP 服务端** | NapCat 作为 HTTP 请求接收方，接收接口调用并回应 | 正向 API 调用 |
| **HTTP 客户端** | NapCat 作为 HTTP 请求发起方，将事件推送至应用框架 | 事件上报（反向） |

**API 请求格式**（NapCat 作为服务端）：
```
POST http://<host>:<port>/<action>
Content-Type: application/json

{
  "user_id": 123456789,
  "message": "你好"
}
```

**响应格式**：
```json
{
  "status": "ok",
  "retcode": 0,
  "data": { "message_id": 1234 }
}
```

**事件推送格式**（NapCat 作为客户端）：
```json
{
  "post_type": "message",
  "message_type": "private",
  "user_id": 123456789,
  "message": "你好"
}
```

### 2.2 WebSocket 通信

| 模式 | 描述 | 场景 |
|------|------|------|
| **正向 WebSocket（WS 服务端）** | NapCat 作为 WS 服务端，外部客户端连接 | 接收事件 + 调用 API 双工 |
| **反向 WebSocket（WS 客户端）** | NapCat 作为 WS 客户端，主动连接应用框架 | 事件上报 + API 调用双工 |

**WebSocket API 请求格式（使用 echo 字段匹配响应）**：
```json
{
  "action": "send_group_msg",
  "params": {
    "group_id": 123456,
    "message": "大家好！"
  },
  "echo": "自定义标识"
}
```

**WebSocket 响应格式**：
```json
{
  "status": "ok",
  "retcode": 0,
  "data": { "message_id": 5678 },
  "echo": "自定义标识"
}
```

**推荐方案**：文档推荐优先使用 **WebSocket**，实时性更好，支持双向通信。

---

## 三、事件系统（Event System）

### 3.1 事件基础字段

所有事件共有的三个基础字段：

| 字段 | 类型 | 说明 |
|------|------|------|
| `time` | `number` | 事件发生时间戳（秒） |
| `post_type` | `string` | 事件类型（`message`/`message_sent`/`notice`/`request`/`meta_event`） |
| `self_id` | `number` | 收到事件的机器人 QQ 号 |

### 3.2 事件分类总览

| post_type | 大类 | 说明 |
|-----------|------|------|
| `meta_event` | 元事件 | 心跳（heartbeat）、生命周期（lifecycle）|
| `message` | 消息事件 | 私聊、群聊 |
| `message_sent` | 消息发送事件 | 机器人自己发出的消息 |
| `notice` | 通知事件 | 群/好友各种通知 |
| `request` | 请求事件 | 好友请求、加群请求 |

### 3.3 元事件（Meta Event）

**心跳事件**（`meta_event_type: "heartbeat"`）：
```json
{
  "post_type": "meta_event",
  "meta_event_type": "heartbeat",
  "status": { "online": true, "good": true },
  "interval": 30000
}
```

**生命周期事件**（`meta_event_type: "lifecycle"`）：
```json
{
  "post_type": "meta_event",
  "meta_event_type": "lifecycle",
  "sub_type": "connect"  // enable / disable / connect
}
```

### 3.4 消息事件（Message Event）

**私聊消息**（`message_type: "private"`）：

| 字段 | 类型 | 说明 |
|------|------|------|
| `sub_type` | `'friend' \| 'group' \| 'other'` | 子类型 |
| `message_id` | `number` | 消息 ID |
| `user_id` | `number` | 发送者 QQ 号 |
| `message` | `OB11Segment[]` | 消息段数组 |
| `raw_message` | `string` | 原始消息（CQ码形式）|
| `sender` | `FriendSender` | `user_id`, `nickname`, `sex`, `age` |

**群聊消息**（`message_type: "group"`）：

| 字段 | 类型 | 说明 |
|------|------|------|
| `sub_type` | `'normal' \| 'anonymous' \| 'notice'` | 子类型 |
| `message_id` | `number` | 消息 ID |
| `user_id` | `number` | 发送者 QQ 号 |
| `group_id` | `number` | 群号 |
| `message` | `OB11Segment[]` | 消息段数组 |
| `raw_message` | `string` | 原始消息 |
| `anonymous` | `any \| null` | 匿名信息 |
| `sender` | `GroupSender` | `user_id`, `nickname`, `card`, `role`（`owner`/`admin`/`member`）, `title`, `level` |

### 3.5 通知事件（Notice Event）- 完整列表

| notice_type | 说明 | 关键字段 |
|-------------|------|---------|
| `group_upload` | 群文件上传 | `group_id`, `user_id`, `file`: `{id, name, size, busid}` |
| `group_admin` | 群管理员变动 | `sub_type`: `set`/`unset`, `group_id`, `user_id` |
| `group_decrease` | 群成员减少 | `sub_type`: `leave`/`kick`/`kick_me`/`disband`, `group_id`, `user_id`, `operator_id` |
| `group_increase` | 群成员增加 | `sub_type`: `approve`/`invite`, `group_id`, `user_id`, `operator_id` |
| `group_ban` | 群禁言 | `sub_type`: `ban`/`lift_ban`, `group_id`, `operator_id`, `user_id`, `duration`(秒) |
| `group_recall` | 群消息撤回 | `group_id`, `user_id`, `operator_id`, `message_id` |
| `friend_add` | 新添加好友 | `user_id` |
| `friend_recall` | 好友消息撤回 | `user_id`, `message_id` |
| `group_card` | 群名片变更 | `group_id`, `user_id`, `card_new`, `card_old` |
| `essence` | 群精华消息 | `sub_type`: `add`/`delete`, `group_id`, `message_id`, `sender_id`, `operator_id` |
| `group_msg_emoji_like` | 表情回应 | `group_id`, `user_id`, `message_id`, `likes`: `[{emoji_id, count}]` |
| `notify` + `poke` | 戳一戳 | `sub_type`: `poke`, `target_id`, `user_id`, `group_id`(群)/`sender_id`(好友) |
| `notify` + `group_name` | 群名变更 | `group_id`, `user_id`, `name_new` |
| `notify` + `title` | 群头衔变更 | `group_id`, `user_id`, `title` |
| `notify` + `gray_tip` | 群灰条消息 | `group_id`, `user_id`, `message_id`, `busi_id`, `content`(JSON), `raw_info` |
| `notify` + `profile_like` | 资料点赞 | `operator_id`, `operator_nick`, `times` |
| `notify` + `input_status` | 输入状态 | `status_text`, `event_type`, `user_id`, `group_id` |
| `bot_offline` | 机器人离线 | `user_id`, `tag`, `message` |

### 3.6 请求事件（Request Event）

| request_type | 说明 | 关键字段 |
|-------------|------|---------|
| `friend` | 好友请求 | `user_id`, `comment`, `flag` |
| `group` | 群请求 | `sub_type`: `add`/`invite`, `group_id`, `user_id`, `comment`, `flag` |

处理方式：通过 `set_friend_add_request` / `set_group_add_request` API 响应。

---

## 四、API 接口大全

### 4.1 账号相关

| Action | 功能 | 参数 |
|--------|------|------|
| `get_login_info` | 获取登录信息 | 无参 |
| `get_status` | 获取在线状态 | 无参 |
| `get_version_info` | 获取版本信息 | 无参 |
| `set_self_longnick` | 设置个性签名 | `longNick: string` |
| `set_online_status` | 设置在线状态 | — |
| `set_qq_avatar` | 设置 QQ 头像 | — |

### 4.2 好友相关

| Action | 功能 | 参数 |
|--------|------|------|
| `get_friend_list` | 获取好友列表 | `no_cache?` |
| `send_private_msg` | 发送私聊消息 | `user_id`, `message` |
| `delete_msg` | 撤回消息 | `message_id` |
| `get_msg` | 获取消息 | `message_id` |
| `send_like` | 好友点赞 | `user_id`, `times` |
| `set_friend_add_request` | 处理好友请求 | `flag`, `approve`, `remark` |
| `set_friend_remark` | 设置备注 | `user_id`, `remark` |
| `delete_friend` | 删除好友 | `user_id` |
| `friend_poke` | 戳一戳好友 | `user_id` |
| `get_friend_msg_history` | 获取私聊历史 | `user_id`, `count` |
| `forward_friend_single_msg` | 转发好友消息 | `user_id`, `message_id` |

### 4.3 群相关（核心 API）

| Action | 功能 | 参数 |
|--------|------|------|
| `get_group_list` | 获取群列表 | `no_cache?` |
| `get_group_info` | 获取群信息 | `group_id`, `no_cache?` |
| `get_group_info_ex` | 获取群扩展信息 | `group_id` |
| `send_group_msg` | 发送群消息 | `group_id`, `message` |
| `set_group_add_request` | 处理加群请求 | `flag`, `approve`, `reason` |
| `set_group_kick` | 踢出群成员 | `group_id`, `user_id`, `reject_add_request` |
| `set_group_ban` | 群禁言 | `group_id`, `user_id`, `duration` |
| `set_group_whole_ban` | 全员禁言 | `group_id`, `enable` |
| `set_group_admin` | 设置管理员 | `group_id`, `user_id`, `enable` |
| `set_group_card` | 设置群名片 | `group_id`, `user_id`, `card` |
| `set_group_name` | 设置群名称 | `group_id`, `group_name` |
| `set_group_leave` | 退出群聊 | `group_id`, `is_dismiss` |
| `set_group_special_title` | 设置专属头衔 | `group_id`, `user_id`, `special_title` |
| `get_group_member_info` | 获取群成员信息 | `group_id`, `user_id`, `no_cache?` |
| **`get_group_member_list`** | **获取群成员列表** | **`group_id`, `no_cache?`** |
| `get_group_honor_info` | 获取群荣誉信息 | `group_id`, `type` |
| `get_essence_msg_list` | 获取精华消息列表 | `group_id` |
| `set_essence_msg` | 设置精华消息 | `message_id` |
| `delete_essence_msg` | 删除精华消息 | `message_id` |
| `group_poke` | 群内戳一戳 | `group_id`, `user_id` |
| **`_send_group_notice`** | **发送群公告** | **`group_id`, `content`** |
| **`_get_group_notice`** | **获取群公告** | **`group_id`** |
| **`_del_group_notice`** | **删除群公告** | **`group_id`, `notice_id`** |
| `get_group_at_all_remain` | 获取 @全体剩余次数 | `group_id` |
| `get_group_msg_history` | 获取群消息历史 | `group_id`, `count` |
| `set_group_portrait` | 设置群头像 | `group_id`, `file`, `cache` |
| `set_group_remark` | 设置群备注 | `group_id`, `remark` |
| `set_group_sign` | 群签到 | `group_id` |
| `get_group_shut_list` | 获取禁言列表 | `group_id` |
| `send_group_sign` | 发送群签到 | `group_id` |

### 4.4 消息相关

| Action | 功能 | 参数 |
|--------|------|------|
| `send_msg` | 发送消息 | `message_type`, `user_id`/`group_id`, `message` |
| `get_image` | 获取图片 | `file` |
| `get_record` | 获取语音 | `file`, `out_format?` |
| `get_file` | 获取文件 | `file`, `type` |
| `ocr_image` | OCR 识别 | `image` |
| `get_forward_msg` | 获取合并转发消息 | `message_id` |
| `mark_msg_as_read` | 标记消息已读 | — |

### 4.5 文件相关

| Action | 功能 | 参数 |
|--------|------|------|
| `upload_group_file` | 上传群文件 | `group_id`, `file`, `name`, `folder?` |
| `delete_group_file` | 删除群文件 | `group_id`, `file_id`, `busid` |
| `get_group_root_files` | 群根目录文件列表 | `group_id` |
| `get_group_files_by_folder` | 群子目录文件列表 | `group_id`, `folder_id` |
| `get_group_file_url` | 获取群文件 URL | `group_id`, `file_id`, `busid` |
| `upload_private_file` | 上传私聊文件 | `user_id`, `file`, `name` |
| `download_file` | 下载文件 | `url`, `thread_count`, `headers` |

### 4.6 转发与分享

| Action | 功能 | 参数 |
|--------|------|------|
| `send_group_forward_msg` | 发送合并转发(群) | `group_id`, `messages` |
| `send_private_forward_msg` | 发送合并转发(私聊) | `user_id`, `messages` |
| `send_forward_msg` | 发送合并转发 | `messages` |

### 4.7 其他实用 API

| Action | 功能 | 参数 |
|--------|------|------|
| `get_cookies` | 获取 Cookies | `domain?` |
| `get_stranger_info` | 获取陌生人信息 | `user_id`, `no_cache?` |
| `get_recent_contact` | 获取最近联系人 | `count` |
| `translate_en2zh` | 英文翻译成中文 | `text` |
| `check_url_safely` | 检查 URL 安全性 | `url` |
| `get_robot_uin_range` | 获取机器人 UIN 范围 | 无参 |
| `get_online_clients` | 获取在线客户端列表 | 无参 |
| `get_credentials` | 获取凭证信息 | `domain?` |

> 完整 API 用例参考：https://napcat.apifox.cn

---

## 五、消息段类型（Message Segments）

基本结构：
```json
{ "type": "text", "data": { "text": "你好" } }
```

### 5.1 文本与 @

| type | data 字段 | 说明 |
|------|-----------|------|
| `text` | `text: string` | 纯文本 |
| `at` | `qq: string`（`"all"` = @全体） | @某人 |
| `reply` | `id: string` | 回复消息 |

### 5.2 表情

| type | data 字段（发送） | 说明 |
|------|------------------|------|
| `face` | `id: string` | QQ 表情 |
| `mface` | `emoji_id`, `emoji_package_id`, `key?`, `summary?` | 商城表情 |
| `dice` | —（随机生成） | 骰子（接收时含 `result: "1"-"6"`）|
| `rps` | —（随机生成） | 石头剪刀布（接收时含 `result`）|
| `poke` | `type: string`, `id: string` | 戳一戳 |

### 5.3 多媒体

| type | data 字段（发送） | 说明 |
|------|------------------|------|
| `image` | `file: string`(路径/URL/Base64), `url?`, `summary?`, `sub_type?` | 图片 |
| `record` | `file: string`(路径/URL/Base64) | 语音 |
| `video` | `file: string`, `thumb?` | 视频 |
| `file` | `file: string`, `name?` | 文件 |

### 5.4 富媒体

| type | data 字段 | 说明 |
|------|-----------|------|
| `json` | `data: string \| object` | JSON 卡片 |
| `music` | `type: string`（`qq`/`163`/`kugou`/`custom` 等）, `id?`, `url?` | 音乐分享（仅发送）|
| `forward` | `id: string` | 合并转发 |

**合计 15 种消息段类型。**

### 5.5 资源 URL 格式说明

NapCat 支持的资源 URL 格式（扩展自标准 OneBot）：
- `base64://<base64数据>` — Base64 编码
- 本地文件路径（如 `/path/to/file`）
- `file://<哈希ID>` — NapCat 文件哈希引用
- 标准 `http://` / `https://`
- `data:...` — Data URI

---

## 六、NapCat 与标准 OneBot 11 的差异

| 差异点 | 说明 |
|--------|------|
| **无持久数据库** | NapCat 不存储历史数据，使用 LRU 缓存管理消息和文件 |
| **LRU 缓存** | 大约 5000 条消息后会因 LRU 策略过期清除 |
| **消息 ID 生成** | 基于哈希算法生成的正整数，非连续数字，每条 ID 唯一 |
| **已撤回消息** | 已撤回的消息无法再次获取或恢复 |
| **资源 URL 扩展** | 额外支持 `base64://`、`file://`、本地路径等 |
| **专有 API** | 大量 `nc_` 前缀和 `_` 前缀的扩展 API |

> NapCat 尽量遵守 OneBot 11 规范，并在无法实现或需扩展时进行差异化实现。

---

## 七、配置说明

### 7.1 OneBot 服务配置（onebot11_qq号.json）

`network` 下四种服务类型，每种可配置多个实例：

**通用配置项**：
- `name: string` — 唯一标识（不能重复）
- `enable: boolean` — 是否启用
- `messagePostFormat: "string" | "array"` — 消息上报格式
- `token: string` — 鉴权密钥
- `debug: boolean` — 是否 raw 数据上报
- `reportSelfMessage: boolean` — 是否上报自身消息

**HTTP 服务端**特有：
- `host`, `port`, `enableCors`, `enableWebsocket`

**HTTP 客户端**特有：
- `url` — 上报地址

**WebSocket 服务端（正向）** 特有：
- `host`, `port`, `heartInterval`（心跳周期）`enableForcePushEvent`

**WebSocket 客户端（反向）** 特有：
- `url` — 连接地址
- `reconnectInterval` — 重连间隔
- `heartInterval` — 心跳周期

**顶层配置项**：
- `musicSignUrl: string` — 音乐签名 URL
- `enableLocalFile2Url: boolean` — 是否本地文件转 URL
- `parseMultMsg: boolean` — 是否解析合并转发消息

### 7.2 NapCat 基础配置（napcat_qq号.json）

| 配置项 | 类型 | 说明 | 默认值 |
|--------|------|------|--------|
| `fileLog` | `boolean` | 是否开启文件日志 | `true` |
| `consoleLog` | `boolean` | 是否开启控制台日志 | `true` |
| `fileLogLevel` | `string` | 文件日志等级（debug/info/error） | `"debug"` |
| `consoleLogLevel` | `string` | 控制台日志等级 | `"info"` |
| `packetServer` | `string` | 数据包服务器地址 | `""` |

### 7.3 WebUI 配置（webui.json）

| 配置项 | 说明 | 默认值 |
|--------|------|--------|
| `host` | WebUI 监听地址 | `"0.0.0.0"` |
| `port` | WebUI 端口 | `6099` |
| `token` | 登录密钥 | 自动生成 |
| `loginRate` | 每分钟登录次数限制 | `3` |

### 7.4 PacketBackend（高级配置）

Native 已内置于 NapCat（v3.6.0+），支持以下扩展功能：
- 设置群头衔
- 发送 poke
- 独立 Rkey 获取
- 陌生人状态获取
- 伪造合并转发
- 文件直链获取
- MarkDown
- 群签到
- 小程序卡片分享
- AI 声聊
- 高性能 OCR

---

## 八、关键要点总结

### 8.1 两端对接最佳实践

1. **推荐协议**：WebSocket（双向双工，实时性好）
2. **推荐模式**：NapCat 作为 WebSocket 客户端（反向 WS），连接到我们的 Rust 服务端
3. **端口规划**：
   - WebUI：6099
   - 反向 WS 上报：自定义（如 8082）
   - HTTP API 调用：自定义（如 3000）
4. **鉴权**：公网部署必须启用 Token

### 8.2 事件处理优先级

```rust
// 按 post_type 分发
match post_type {
    "meta_event" => handle_heartbeat/lifecycle,    // 心跳+生命周期
    "message" | "message_sent" => handle_message,  // 消息处理
    "notice" => handle_notice,                      // 通知处理
    "request" => handle_request,                    // 请求处理
}
```

### 8.3 核心 API 调用流程

```rust
// 通过 WebSocket 发送请求
let request = json!({
    "action": "send_group_msg",
    "params": { "group_id": 123, "message": [...] },
    "echo": "unique_id_001"
});
// 通过 echo 字段匹配响应
```

### 8.4 注意事项

- 消息 ID 是哈希值而非连续数字
- LRU 缓存约 5000 条，历史数据可能过期
- 配置文件名格式：`onebot11_{QQ号}.json` / `napcat_{QQ号}.json`
- v4.5.3+ 支持载入 `./config/onebot11.json` 作为默认配置
- WebUI 默认地址 `0.0.0.0:6099`，端口被占用自动 +1（最多 100 次）
- 低于 v4.4.7 不要将注释写入配置文件
