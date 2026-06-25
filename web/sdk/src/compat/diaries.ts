/**
 * 旧 SDK 兼容层 —— DiariesApi
 *
 * 旧类名: DiariesApi（来自 web/old/server/apis/DiariesApi.ts）
 * 新实现: 内部委托给 UserClient.diaries
 */

import { UserClient } from '../user-client.js'
import type { Diary, CreateDiaryRequest, UpdateDiaryRequest } from '../types.js'

/** @deprecated 请使用 UserClient.diaries 代替 */
export class DiariesApi {
  constructor(private client: UserClient) {}

  async createDiary(data: CreateDiaryRequest): Promise<Diary> {
    return this.client.diaries.create(data)
  }

  async updateDiary(id: number, data: UpdateDiaryRequest): Promise<Diary> {
    return this.client.diaries.update(id, data)
  }

  async deleteDiary(id: number): Promise<void> {
    await this.client.diaries.delete(id)
  }

  async getDiary(id: number): Promise<Diary> {
    return this.client.diaries.get(id)
  }

  async listDiaries(): Promise<Diary[]> {
    return this.client.diaries.list()
  }
}
