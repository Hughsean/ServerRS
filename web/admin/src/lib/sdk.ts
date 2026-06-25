import { createLocalStorageTokenStore, createAdminClient, createUserClient } from '@serverrs/sdk'

export const tokenStore = createLocalStorageTokenStore(undefined, 'serverrs-admin')

// 管理员客户端（用于管理后台页面：用户管理、风险会话等）
export const api = createAdminClient({
  baseUrl: import.meta.env.VITE_API_BASE_URL ?? '',
  tokenStore,
  timeoutMs: 30_000,
})

// 认证客户端（用于登录页，AdminClient 不包含 auth 方法）
export const authClient = createUserClient({
  baseUrl: import.meta.env.VITE_API_BASE_URL ?? '',
  tokenStore,
  timeoutMs: 30_000,
})
