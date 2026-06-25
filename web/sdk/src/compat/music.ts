/**
 * 旧 SDK 兼容层 —— MusicApi
 *
 * 旧类名: MusicApi（来自 web/old/server/apis/MusicApi.ts）
 * 新实现: 内部委托给 UserClient.music
 *
 * 不提供兼容的旧方法（服务端接口已改变）：
 * - getByCategory, getByArtist, search, add, update
 * - updateWithoutFiles, delete, getCount, getCountByCategory, getAllMetadata
 */

import { UserClient } from '../user-client.js'
import type { MusicTrack } from '../types.js'

/** @deprecated 请使用 UserClient.music 代替 */
export class MusicApi {
  constructor(private client: UserClient) {}

  /** GET /api/v1/music/tracks/{id} */
  getById(musicId: number): Promise<MusicTrack> {
    return this.client.music.track(musicId)
  }

  /** GET /api/v1/music/tracks */
  getAll(): Promise<MusicTrack[]> {
    return this.client.music.tracks().then(page => page.items)
  }
}
