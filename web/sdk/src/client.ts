// client.ts —— 向后兼容的合体版（组合 UserClient + AdminApi）
// 新代码请直接使用 UserClient / AdminClient

import { type ServerRsClientConfig, HttpClient } from './http.js'
import { UserClient } from './user-client.js'
import { AdminApi } from './admin-client.js'

/**
 * 合体版客户端 —— 同时包含用户端和管理员端 API。
 *
 * @deprecated 新代码请根据角色选择 UserClient 或 AdminClient。
 */
export class ServerRsClient {
  readonly http: HttpClient
  readonly auth: UserClient['auth']
  readonly users: UserClient['users']
  readonly chat: UserClient['chat']
  readonly psychology: UserClient['psychology']
  readonly depression: UserClient['depression']
  readonly diaries: UserClient['diaries']
  readonly community: UserClient['community']
  readonly music: UserClient['music']
  readonly objects: UserClient['objects']
  readonly admin: AdminApi

  constructor(config: ServerRsClientConfig) {
    const client = new UserClient(config)
    this.http = client.http
    this.auth = client.auth
    this.users = client.users
    this.chat = client.chat
    this.psychology = client.psychology
    this.depression = client.depression
    this.diaries = client.diaries
    this.community = client.community
    this.music = client.music
    this.objects = client.objects
    this.admin = new AdminApi(client.http)
  }

  health() { return this.http.request('GET', '/health', { auth: false }) }
}

export function createServerRsClient(config: ServerRsClientConfig): ServerRsClient {
  return new ServerRsClient(config)
}

