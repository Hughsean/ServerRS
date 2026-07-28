# 数字人对话语音接口（前端交接）

## 调用接口

```http
POST /api/v1/chat/messages-with-audio
Authorization: Bearer <accessToken>
Content-Type: application/json
```

普通文本对话可使用此接口获取文字与语音。旧接口 `POST /api/v1/chat/messages` 保留兼容，但不会返回语音。

## 请求体

```json
{
  "text": "你好，介绍一下你自己",
  "format": "wav",
  "sampleRate": 24000,
  "channels": 1,
  "sampleBits": 16
}
```

| 字段 | 必填 | 说明 |
| --- | --- | --- |
| `text` | 是 | 用户输入，不能为空。 |
| `format` | 是 | `wav`、`mp3`、`pcm`、`ogg_opus`；`ogg` 是 `ogg_opus` 的兼容写法。 |
| `sampleRate` | 是 | 8000～48000。推荐 24000。 |
| `channels` | 是 | 1 或 2。推荐 1。 |
| `sampleBits` | 是 | WAV/PCM：16、24、32；MP3：仅 16；Ogg Opus：仅 16 且采样率必须 48000。 |
| `emotion` | 否 | 对话上下文情绪字段。 |
| `location` | 否 | 对话上下文位置字段。 |
| `voice` | 否 | 当前部署**未配置可选音色**，前端必须省略该字段；服务端使用默认音色。 |

> 当前 `tts.allowed_voices = []`，传入默认音色以外的 `voice` 会返回 `400 VALIDATION_ERROR`。后续若部署方配置白名单，前端只能使用部署方提供的音色标识，不能让用户自由输入。

## 完成响应：`200 OK`

```json
{
  "conversationId": 9,
  "reply": "你好，我是你的数字人助手。",
  "toolCalls": [],
  "audio": {
    "audioUrl": "http://127.0.0.1:8080/api/v1/tts/audio/<file-id>?expires=<unix-seconds>&signature=<signature>",
    "format": "wav",
    "sampleRate": 24000,
    "channels": 1,
    "sampleBits": 16
  }
}
```

前端先渲染 `reply`，再播放 `audio.audioUrl`：

```ts
const player = new Audio(response.audio.audioUrl);
await player.play();
```

`audioUrl` 是短时签名能力链接：禁止写日志、持久化、分享或自行拼接；过期后提示用户重新生成。播放失败不能覆盖已成功返回的文字回复。

## 审批暂停响应：`202 Accepted`

需审批的受控工具调用不会生成语音，响应保持既有 snake_case 协议：

```json
{
  "status": "suspended",
  "conversation_id": 9,
  "checkpoint_id": "<uuid>",
  "run_id": "<uuid>",
  "reason": "approval",
  "approval": {
    "approval_id": "<uuid>",
    "prompt": "模型请求执行受控工具，请确认是否允许。",
    "tool_calls": []
  }
}
```

前端不得自动播放或自动批准。待用户确认后调用既有恢复接口：

```http
POST /api/v1/chat/checkpoints/{checkpoint_id}/resume
Authorization: Bearer <accessToken>
Content-Type: application/json
```

```json
{
  "approval_id": "<uuid>",
  "decision": "approve"
}
```

恢复接口当前只返回文本，不生成语音。

## 错误响应

```json
{
  "code": "VALIDATION_ERROR",
  "message": "request validation failed: ..."
}
```

| 状态码 | code | 处理建议 |
| --- | --- | --- |
| 400 | `VALIDATION_ERROR` | 修正请求参数或移除不受支持的 `voice`。 |
| 401 | `UNAUTHORIZED` | 刷新 Token，失败后重新登录。 |
| 404 | `NOT_FOUND` | 签名音频链接已过期或不可用，重新发起对话。 |
| 501 | `NOT_IMPLEMENTED` | 服务端未启用 TTS。 |
| 502 | `INFRASTRUCTURE_ERROR` | TTS 或 ffmpeg 暂时不可用，可提示稍后重试。 |
