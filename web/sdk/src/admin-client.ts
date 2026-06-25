/**
 * AdminClient —— 管理员 API 客户端。
 *
 * 仅包含管理员后台所需的管理接口。
 * 不包含聊天、日记等普通用户操作，防止在管理员代码中引入不必要的依赖。
 */

import { HttpClient, type ServerRsClientConfig } from './http.js'
import type {
  AdminPatchUserRequest,
  AdminRiskConversationDetail,
  AdminUser,
  Article,
  ArticleWriteRequest,
  Category,
  CategoryWriteRequest,
  CountTrendResponse,
  CreateMusicTrackRequest,
  DeletedResponse,
  HealthResponse,
  KnowledgeReviewDetail,
  KnowledgeReviewPage,
  MusicTrack,
  MusicTrackPage,
  PaginatedAdminUsers,
  PaginatedRiskConversations,
  PsychologyResource,
  PsychologyResourceWriteRequest,
  Qna,
  QnaWriteRequest,
  ReviewPublishRequest,
  RiskAuditAdminDto,
  RiskStatsResponse,
  UpdateMusicTrackRequest,
} from './types.js'

export interface PageQuery {
  page?: number
  pageSize?: number
}

export interface KnowledgeReviewQuery extends PageQuery {
  status?: string
  sourceId?: number
}

export interface RiskConversationQuery extends PageQuery {
  riskLevel?: string
}

export interface AdminPsychologyQuery extends PageQuery {
  search?: string
  categoryId?: number
  resourceType?: string
  isVerified?: boolean
  isPublished?: boolean
}

/**
 * 管理员客户端 —— 仅包含管理员后台所需的管理接口。
 */
export class AdminClient {
  readonly http: HttpClient
  readonly admin: AdminApi

  constructor(config: ServerRsClientConfig) {
    this.http = new HttpClient(config)
    this.admin = new AdminApi(this.http)
  }

  health(): Promise<HealthResponse> {
    return this.http.request('GET', '/health', { auth: false })
  }
}

export function createAdminClient(config: ServerRsClientConfig): AdminClient {
  return new AdminClient(config)
}

// ── Admin API ──

export class AdminApi {
  constructor(private readonly http: HttpClient) {}

  users(query: PageQuery = {}): Promise<PaginatedAdminUsers> {
    return this.http.request('GET', '/api/v1/admin/users', { query })
  }

  user(id: number): Promise<AdminUser> {
    return this.http.request('GET', `/api/v1/admin/users/${id}`)
  }

  updateUser(id: number, payload: AdminPatchUserRequest): Promise<AdminUser> {
    return this.http.request('PATCH', `/api/v1/admin/users/${id}`, { body: payload })
  }

  deleteUser(id: number): Promise<void> {
    return this.http.request('DELETE', `/api/v1/admin/users/${id}`)
  }

  riskConversations(query: RiskConversationQuery = {}): Promise<PaginatedRiskConversations> {
    return this.http.request('GET', '/api/v1/admin/risk-conversations', { query })
  }

  riskConversation(id: number): Promise<AdminRiskConversationDetail> {
    return this.http.request('GET', `/api/v1/admin/risk-conversations/${id}`)
  }

  /** @deprecated 该接口已废弃，现在始终返回 410。 */
  processRiskDetection(_id: number, _notes?: string): Promise<RiskAuditAdminDto> {
    return this.http.request('POST', `/api/v1/admin/risk-detections/${_id}/process`, {
      body: { notes: _notes },
    })
  }

  createTrack(payload: CreateMusicTrackRequest): Promise<MusicTrack> {
    return this.http.request('POST', '/api/v1/admin/music', { body: payload })
  }

  tracks(query: Record<string, unknown> = {}): Promise<MusicTrackPage> {
    return this.http.request('GET', '/api/v1/admin/music', { query })
  }

  updateTrack(id: number, payload: UpdateMusicTrackRequest): Promise<MusicTrack> {
    return this.http.request('PATCH', `/api/v1/admin/music/${id}`, { body: payload })
  }

  deleteTrack(id: number): Promise<DeletedResponse> {
    return this.http.request('DELETE', `/api/v1/admin/music/${id}`)
  }

  psychologyCategories(): Promise<Category[]> {
    return this.http.request('GET', '/api/v1/admin/psychology/categories')
  }

  psychologyCategory(id: number): Promise<Category> {
    return this.http.request('GET', `/api/v1/admin/psychology/categories/${id}`)
  }

  createPsychologyCategory(payload: CategoryWriteRequest): Promise<Category> {
    return this.http.request('POST', '/api/v1/admin/psychology/categories', { body: payload })
  }

  updatePsychologyCategory(id: number, payload: CategoryWriteRequest): Promise<Category> {
    return this.http.request('PUT', `/api/v1/admin/psychology/categories/${id}`, { body: payload })
  }

  deletePsychologyCategory(id: number): Promise<DeletedResponse> {
    return this.http.request('DELETE', `/api/v1/admin/psychology/categories/${id}`)
  }

  psychologyArticles(query: AdminPsychologyQuery = {}): Promise<unknown> {
    return this.http.request('GET', '/api/v1/admin/psychology/articles', { query })
  }

  psychologyArticle(id: number): Promise<Article> {
    return this.http.request('GET', `/api/v1/admin/psychology/articles/${id}`)
  }

  createPsychologyArticle(payload: ArticleWriteRequest): Promise<Article> {
    return this.http.request('POST', '/api/v1/admin/psychology/articles', { body: payload })
  }

  updatePsychologyArticle(id: number, payload: ArticleWriteRequest): Promise<Article> {
    return this.http.request('PUT', `/api/v1/admin/psychology/articles/${id}`, { body: payload })
  }

  deletePsychologyArticle(id: number): Promise<DeletedResponse> {
    return this.http.request('DELETE', `/api/v1/admin/psychology/articles/${id}`)
  }

  psychologyQna(query: AdminPsychologyQuery = {}): Promise<unknown> {
    return this.http.request('GET', '/api/v1/admin/psychology/qna', { query })
  }

  psychologyQnaItem(id: number): Promise<Qna> {
    return this.http.request('GET', `/api/v1/admin/psychology/qna/${id}`)
  }

  createPsychologyQna(payload: QnaWriteRequest): Promise<Qna> {
    return this.http.request('POST', '/api/v1/admin/psychology/qna', { body: payload })
  }

  updatePsychologyQna(id: number, payload: QnaWriteRequest): Promise<Qna> {
    return this.http.request('PUT', `/api/v1/admin/psychology/qna/${id}`, { body: payload })
  }

  deletePsychologyQna(id: number): Promise<DeletedResponse> {
    return this.http.request('DELETE', `/api/v1/admin/psychology/qna/${id}`)
  }

  psychologyResources(query: AdminPsychologyQuery = {}): Promise<unknown> {
    return this.http.request('GET', '/api/v1/admin/psychology/resources', { query })
  }

  psychologyResource(id: number): Promise<PsychologyResource> {
    return this.http.request('GET', `/api/v1/admin/psychology/resources/${id}`)
  }

  createPsychologyResource(payload: PsychologyResourceWriteRequest): Promise<PsychologyResource> {
    return this.http.request('POST', '/api/v1/admin/psychology/resources', { body: payload })
  }

  updatePsychologyResource(id: number, payload: PsychologyResourceWriteRequest): Promise<PsychologyResource> {
    return this.http.request('PUT', `/api/v1/admin/psychology/resources/${id}`, { body: payload })
  }

  deletePsychologyResource(id: number): Promise<DeletedResponse> {
    return this.http.request('DELETE', `/api/v1/admin/psychology/resources/${id}`)
  }

  knowledgeReviews(query: KnowledgeReviewQuery = {}): Promise<KnowledgeReviewPage> {
    return this.http.request('GET', '/api/v1/admin/web-ingestion/reviews', { query })
  }

  knowledgeReview(id: number): Promise<KnowledgeReviewDetail> {
    return this.http.request('GET', `/api/v1/admin/web-ingestion/reviews/${id}`)
  }

  publishKnowledgeReview(id: number, notes?: string): Promise<ReviewPublishRequest> {
    return this.http.request('POST', `/api/v1/admin/web-ingestion/reviews/${id}/publish`, {
      body: { notes },
    })
  }

  statsUsers(): Promise<CountTrendResponse> {
    return this.http.request('GET', '/api/v1/admin/stats/users')
  }

  statsMusic(): Promise<CountTrendResponse> {
    return this.http.request('GET', '/api/v1/admin/stats/music')
  }

  statsReviews(): Promise<CountTrendResponse> {
    return this.http.request('GET', '/api/v1/admin/stats/reviews')
  }

  statsRisks(): Promise<RiskStatsResponse> {
    return this.http.request('GET', '/api/v1/admin/stats/risks')
  }
}
