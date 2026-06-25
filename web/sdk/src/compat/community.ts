/**
 * 旧 SDK 兼容层 —— CommunityApi
 *
 * 旧类名: CommunityApi（来自 web/old/server/apis/CommunityApi.ts）
 * 新实现: 内部委托给 UserClient.community
 *
 * 不提供兼容的旧方法：
 * - deleteComment（旧路径无 postId 参数，需迁移调用方）
 * - likeComment（同上）
 */

import { UserClient } from '../user-client.js'
import type {
  CommunityPost,
  CommunityComment,
  CreateCommunityPostRequest,
  UpdateCommunityPostRequest,
} from '../types.js'

/** @deprecated 请使用 UserClient.community 代替 */
export class CommunityApi {
  constructor(private client: UserClient) {}

  listPosts(params?: { status?: number; userId?: number }) {
    return this.client.community.posts(params as any) as any
  }

  getPost(postId: number) {
    return this.client.community.post(postId)
  }

  createPost(payload: CreateCommunityPostRequest) {
    return this.client.community.createPost(payload)
  }

  updatePost(postId: number, payload: UpdateCommunityPostRequest) {
    return this.client.community.updatePost(postId, payload)
  }

  deletePost(postId: number) {
    return this.client.community.deletePost(postId)
  }

  likePost(postId: number) {
    return this.client.community.likePost(postId)
  }

  listComments(postId: number) {
    return this.client.community.comments(postId) as any
  }

  createComment(postId: number, comment: CommunityComment) {
    return this.client.community.createComment(postId, comment as any)
  }

  /** @deprecated 需要 postId 参数，请改用 client.community.deleteComment(postId, commentId) */
  deleteComment(_commentId: number): never {
    throw new Error(
      'deleteComment 需要 postId 参数，请改用 client.community.deleteComment(postId, commentId)'
    )
  }

  /** @deprecated 需要 postId 参数，请改用 client.community.likeComment(postId, commentId) */
  likeComment(_commentId: number): never {
    throw new Error(
      'likeComment 需要 postId 参数，请改用 client.community.likeComment(postId, commentId)'
    )
  }
}
