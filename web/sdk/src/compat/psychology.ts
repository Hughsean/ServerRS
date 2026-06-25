/**
 * 旧 SDK 兼容层 —— PsychologyApi
 *
 * 旧类名: PsychologyApi（来自 web/old/server/apis/PsychologyApi.ts）
 * 新实现: 内部委托给 UserClient.psychology
 *
 * 不提供兼容的旧方法（路径/参数已改变或迁移到 AdminApi）：
 * - createCategory, createArticle（管理员接口，使用 AdminClient）
 * - getCategoryChildren, getArticlesByCategory, getFeaturedArticles
 * - getLatestArticles, searchArticles, likeArticle
 * - getQnaByCategory, getVerifiedQna, searchQna, likeQna
 * - getResourcesByCategory, getResourcesByType, likeResource
 */

import { UserClient } from '../user-client.js'

/** @deprecated 请使用 UserClient.psychology 代替 */
export class PsychologyApi {
  constructor(private client: UserClient) {}

  /** GET /api/v1/psychology/categories */
  getCategories() {
    return this.client.psychology.categories() as any
  }

  /** GET /api/v1/psychology/categories/tree */
  getCategoryTree() {
    return this.client.psychology.categoryTree() as any
  }

  /** GET /api/v1/psychology/articles/{id} */
  getArticle(articleId: number) {
    return this.client.psychology.article(articleId) as any
  }

  /** GET /api/v1/psychology/qna/{id} */
  getQna(qnaId: number) {
    return this.client.psychology.qnaItem(qnaId) as any
  }

  /** GET /api/v1/psychology/resources/{id} */
  getResource(resourceId: number) {
    return this.client.psychology.resource(resourceId) as any
  }

  /** GET /api/v1/psychology/favorites */
  getUserFavorites() {
    return this.client.psychology.favorites() as any
  }

  /** GET /api/v1/psychology/favorites/check */
  checkFavorite(contentType: string, contentId: number) {
    return this.client.psychology.favoriteStatus(contentType, contentId) as any
  }

  /** POST /api/v1/psychology/favorites */
  toggleFavorite(contentType: string, contentId: number) {
    return this.client.psychology.toggleFavorite(contentType, contentId) as any
  }

  /** POST /api/v1/psychology/likes */
  likeArticle(articleId: number) {
    return this.client.psychology.toggleLike('ARTICLE', articleId)
  }

  likeQna(qnaId: number) {
    return this.client.psychology.toggleLike('QNA', qnaId)
  }

  likeResource(resourceId: number) {
    return this.client.psychology.toggleLike('RESOURCE', resourceId)
  }
}
