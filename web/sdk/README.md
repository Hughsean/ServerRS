# @serverrs/sdk

ServerRS 的通用 TypeScript SDK，可用于浏览器、SSR 和 Node.js。它不依赖 Vue
或其他 UI 框架，同时覆盖公开接口、普通用户接口和管理员接口。

## 在其他前端项目中安装

先在 ServerRS 仓库构建 SDK，再从普通用户前端项目安装本地包：

```powershell
pnpm --dir D:\WorkBench\ServerRS\web\sdk build
pnpm add D:\WorkBench\ServerRS\web\sdk
```

发布到私有 npm registry 后，也可以直接使用 `pnpm add @serverrs/sdk`。

## 快速开始

```ts
import { createLocalStorageTokenStore, createUserClient, createAdminClient } from '@serverrs/sdk'

// 普通用户客户端
const client = createUserClient({
  baseUrl: 'http://127.0.0.1:3000',
  tokenStore: createLocalStorageTokenStore(),
})

await client.auth.login({ username: 'user', password: 'password123' })
const turn = await client.chat.sendMessage({ text: '你好' })
if ('status' in turn && turn.status === 'suspended') {
  await client.chat.resumeCheckpoint(turn.checkpoint_id, {
    approval_id: turn.approval.approval_id,
    decision: 'approve',
  })
}
await client.diaries.list()
```

```ts
// 管理员客户端（仅包含管理接口，不暴露聊天/日记等普通 API）
import { createAdminClient } from '@serverrs/sdk'

const admin = createAdminClient({ baseUrl, tokenStore })
const reviews = await admin.admin.knowledgeReviews({ status: 'staged' })
```

## 客户端选择

| 客户端 | 导入 | 适用场景 |
|--------|------|----------|
| `UserClient` | `createUserClient()` | 普通用户前端 App |
| `AdminClient` | `createAdminClient()` | 管理后台 |
| `ServerRsClient` | `createServerRsClient()` | 旧代码迁移（已废弃） |

## 兼容旧版 SDK

```ts
import { DiariesApi, AdminApi } from '@serverrs/sdk/compat'

const diaries = new DiariesApi(userClient)
await diaries.listDiaries()
```

详细兼容列表见 `src/compat/` 目录。

## 按业务域划分

SDK 按业务域提供以下客户端：

### UserClient
- `auth` — 注册、登录、刷新令牌、登出
- `users` — 个人信息查看与修改
- `chat` — 对话、消息发送、记忆管理、画像
- `psychology` — 心理知识库阅读与收藏
- `depression` — 抑郁量表与评估记录
- `diaries` — 日记 CRUD
- `community` — 社区帖子、评论、点赞
- `music` — 音乐曲库浏览与播放
- `objects` — 文件上传与下载

### AdminClient
- `admin.users` — 用户管理（查/改/删）
- `admin.riskConversations` — 风险对话查看
- `admin.tracks` — 音乐管理 CRUD
- `admin.psychology*` — 心理知识库管理 CRUD
- `admin.knowledgeReviews` — 知识审核

## Token 存储

同域部署时 `baseUrl` 可以设为空字符串。SSR、Node.js 和测试环境建议使用
`createMemoryTokenStore()`，或注入自己的 `TokenStore` 与 `fetch` 实现。
