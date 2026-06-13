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

## 浏览器用法

```ts
import { createLocalStorageTokenStore, createServerRsClient } from '@serverrs/sdk'

const client = createServerRsClient({
  baseUrl: 'http://127.0.0.1:3000',
  tokenStore: createLocalStorageTokenStore(),
})

await client.auth.login({ username: 'admin', password: 'password123' })
const reviews = await client.admin.knowledgeReviews({ status: 'staged' })
```

同域部署时 `baseUrl` 可以设为空字符串。SSR、Node.js 和测试环境建议使用
`createMemoryTokenStore()`，或注入自己的 `TokenStore` 与 `fetch` 实现。

SDK 按业务域提供 `auth`、`users`、`sessions`、`psychology`、`depression`、
`diaries`、`community`、`music`、`objects` 和 `admin` 客户端。
