import type { ApiErrorBody } from './types.js'

export interface TokenPair {
  accessToken: string
  refreshToken?: string
}

export interface TokenStore {
  getAccessToken(): string | null | Promise<string | null>
  getRefreshToken?(): string | null | Promise<string | null>
  setTokens(tokens: TokenPair): void | Promise<void>
  clear(): void | Promise<void>
}

export interface ServerRsClientConfig {
  baseUrl: string
  fetch?: typeof globalThis.fetch
  tokenStore?: TokenStore
  timeoutMs?: number
  headers?: Record<string, string>
  onUnauthorized?: () => void | Promise<void>
}

export interface RequestOptions {
  query?: object
  body?: unknown
  headers?: Record<string, string>
  signal?: AbortSignal
  auth?: boolean
  responseType?: 'json' | 'blob' | 'arrayBuffer' | 'text'
}

export class ServerRsApiError extends Error {
  readonly status: number
  readonly code: string
  readonly details: unknown

  constructor(status: number, code: string, message: string, details?: unknown) {
    super(message)
    this.name = 'ServerRsApiError'
    this.status = status
    this.code = code
    this.details = details
  }
}

export class HttpClient {
  readonly baseUrl: string
  readonly tokenStore?: TokenStore
  private readonly fetchImpl: typeof globalThis.fetch
  private readonly timeoutMs: number
  private readonly defaultHeaders: Record<string, string>
  private readonly onUnauthorized?: () => void | Promise<void>

  constructor(config: ServerRsClientConfig) {
    const fetchImpl = config.fetch ?? globalThis.fetch
    if (!fetchImpl) {
      throw new Error('A fetch implementation is required')
    }
    this.baseUrl = config.baseUrl.replace(/\/+$/, '')
    this.fetchImpl = fetchImpl.bind(globalThis)
    this.tokenStore = config.tokenStore
    this.timeoutMs = config.timeoutMs ?? 20_000
    this.defaultHeaders = { Accept: 'application/json', ...config.headers }
    this.onUnauthorized = config.onUnauthorized
  }

  async request<T>(method: string, path: string, options: RequestOptions = {}): Promise<T> {
    const url = `${this.baseUrl}${path.startsWith('/') ? path : `/${path}`}${buildQuery(options.query)}`
    const controller = new AbortController()
    const timeout = setTimeout(() => controller.abort(), this.timeoutMs)
    const signal = combineSignals(controller.signal, options.signal)
    const headers = new Headers(this.defaultHeaders)

    for (const [key, value] of Object.entries(options.headers ?? {})) {
      headers.set(key, value)
    }
    if (options.auth !== false && this.tokenStore) {
      const accessToken = await this.tokenStore.getAccessToken()
      if (accessToken) headers.set('Authorization', `Bearer ${accessToken}`)
    }

    let body: BodyInit | undefined
    if (options.body instanceof FormData || options.body instanceof Blob) {
      body = options.body
    } else if (options.body !== undefined) {
      headers.set('Content-Type', 'application/json')
      body = JSON.stringify(options.body)
    }

    try {
      const response = await this.fetchImpl(url, { method, headers, body, signal })
      if (!response.ok) {
        const details = await readErrorBody(response)
        if (response.status === 401) await this.onUnauthorized?.()
        throw new ServerRsApiError(
          response.status,
          details.code ?? `HTTP_${response.status}`,
          details.message ?? response.statusText,
          details,
        )
      }

      if (response.status === 204) return undefined as T
      switch (options.responseType) {
        case 'blob':
          return (await response.blob()) as T
        case 'arrayBuffer':
          return (await response.arrayBuffer()) as T
        case 'text':
          return (await response.text()) as T
        default: {
          const text = await response.text()
          return (text ? JSON.parse(text) : undefined) as T
        }
      }
    } catch (error) {
      if (error instanceof ServerRsApiError) throw error
      if (error instanceof DOMException && error.name === 'AbortError') {
        throw new ServerRsApiError(0, 'REQUEST_ABORTED', 'Request timed out or was aborted')
      }
      throw new ServerRsApiError(
        0,
        'NETWORK_ERROR',
        error instanceof Error ? error.message : 'Network request failed',
        error,
      )
    } finally {
      clearTimeout(timeout)
    }
  }
}

export function createMemoryTokenStore(initial: Partial<TokenPair> = {}): TokenStore {
  let accessToken = initial.accessToken ?? null
  let refreshToken = initial.refreshToken ?? null
  return {
    getAccessToken: () => accessToken,
    getRefreshToken: () => refreshToken,
    setTokens: (tokens) => {
      accessToken = tokens.accessToken
      refreshToken = tokens.refreshToken ?? refreshToken
    },
    clear: () => {
      accessToken = null
      refreshToken = null
    },
  }
}

export function createLocalStorageTokenStore(
  storage?: Pick<Storage, 'getItem' | 'setItem' | 'removeItem'>,
  prefix = 'serverrs',
): TokenStore {
  const target = storage ?? globalThis.localStorage
  if (!target) throw new Error('localStorage is not available in this environment')
  const accessKey = `${prefix}:accessToken`
  const refreshKey = `${prefix}:refreshToken`
  return {
    getAccessToken: () => target.getItem(accessKey),
    getRefreshToken: () => target.getItem(refreshKey),
    setTokens: ({ accessToken, refreshToken }) => {
      target.setItem(accessKey, accessToken)
      if (refreshToken) target.setItem(refreshKey, refreshToken)
    },
    clear: () => {
      target.removeItem(accessKey)
      target.removeItem(refreshKey)
    },
  }
}

function buildQuery(query?: object): string {
  if (!query) return ''
  const search = new URLSearchParams()
  for (const [key, value] of Object.entries(query)) {
    if (value === undefined || value === null || value === '') continue
    if (Array.isArray(value)) {
      value.forEach((item) => search.append(key, String(item)))
    } else {
      search.set(key, String(value))
    }
  }
  const result = search.toString()
  return result ? `?${result}` : ''
}

function combineSignals(internal: AbortSignal, external?: AbortSignal): AbortSignal {
  if (!external) return internal
  const controller = new AbortController()
  const abort = () => controller.abort()
  internal.addEventListener('abort', abort, { once: true })
  external.addEventListener('abort', abort, { once: true })
  return controller.signal
}

async function readErrorBody(response: Response): Promise<ApiErrorBody> {
  const text = await response.text()
  if (!text) return {}
  try {
    return JSON.parse(text) as ApiErrorBody
  } catch {
    return { message: text }
  }
}
