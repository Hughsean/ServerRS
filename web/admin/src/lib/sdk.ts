import { createLocalStorageTokenStore, createAdminClient } from '@serverrs/sdk'

export const tokenStore = createLocalStorageTokenStore(undefined, 'serverrs-admin')

export const api = createAdminClient({
  baseUrl: import.meta.env.VITE_API_BASE_URL ?? '',
  tokenStore,
  timeoutMs: 30_000,
})
