# ServerRS 管理后台

Vue 3 管理页面，直接使用工作区中的 `@serverrs/sdk`。开发环境默认把 `/api`
和 `/health` 代理到 `http://127.0.0.1:3000`。

```powershell
cd web
pnpm install
pnpm dev
```

管理员账号必须具有 `ADMIN` 或 `SUPER_ADMIN` 角色。生产部署时可通过
`VITE_API_BASE_URL` 指定完整服务端地址；同域部署则保持为空。
