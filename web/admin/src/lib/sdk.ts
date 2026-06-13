import { createLocalStorageTokenStore, createServerRsClient } from '@serverrs/sdk'

export const tokenStore = createLocalStorageTokenStore(undefined, 'serverrs-admin')

export const api = createServerRsClient({
  baseUrl: import.meta.env.VITE_API_BASE_URL ?? '',
  tokenStore,
  timeoutMs: 30_000,
})
