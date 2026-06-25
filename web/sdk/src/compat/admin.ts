/**
 * 旧 SDK 兼容层 —— AdminApi
 *
 * 旧类名: AdminApi（来自 web/old/server/apis/AdminApi.ts）
 * 新实现: 内部委托给 AdminClient.admin
 */

import { AdminClient } from '../admin-client.js'
import type { AdminUser } from '../types.js'

/** @deprecated 请使用 AdminClient.admin 代替 */
export class AdminApi {
  constructor(private client: AdminClient) {}

  getAllUsers(): Promise<AdminUser[]> {
    return this.client.admin.users().then(page => page.items)
  }

  /** @deprecated 风险对话接口已改变，请改用 client.admin.riskConversations() */
  getRiskConversations(_userId: number): never {
    throw new Error(
      '风险对话列表不再按用户筛选，请改用 client.admin.riskConversations()'
    )
  }

  /** @deprecated 风险处理接口已废弃，服务端始终返回 410 */
  processRiskDetection(_detectionId: number, _notes?: string) {
    return this.client.admin.processRiskDetection(_detectionId, _notes)
  }
}
