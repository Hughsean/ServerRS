import {
  HttpClient,
  type ServerRsClientConfig,
  type TokenStore,
} from './http.js'
import type {
  AdminPatchUserRequest,
  AdminRiskConversationDetail,
  AdminRiskDetection,
  AdminUser,
  Article,
  ArticleWriteRequest,
  AuthUser,
  Category,
  CategoryWriteRequest,
  CommunityComment,
  CommunityPage,
  CommunityPost,
  Conversation,
  ConversationMessage,
  CreateCommunityCommentRequest,
  CreateCommunityPostRequest,
  CreateDepressionAssessmentRequest,
  CreateDiaryRequest,
  CreateMusicTrackRequest,
  DeletedResponse,
  DepressionAssessment,
  DepressionAssessmentPage,
  DepressionScale,
  Diary,
  Favorite,
  FavoriteStatus,
  HealthResponse,
  KnowledgeReviewDetail,
  KnowledgeReviewPage,
  LikeStatus,
  LoginRequest,
  LoginResponse,
  MessageRequest,
  MessageResponse,
  MusicTrack,
  MusicTrackPage,
  Paginated,
  PaginatedAdminUsers,
  PaginatedRiskConversations,
  PatchMeRequest,
  PsychologyResource,
  PsychologyResourceWriteRequest,
  Qna,
  QnaWriteRequest,
  RefreshResponse,
  RegisterRequest,
  ReviewPublishRequest,
  RiskDetectionPage,
  SessionCreateRequest,
  SessionCreateResponse,
  SessionStatus,
  StoredObject,
  UpdateCommunityPostRequest,
  UpdateDiaryRequest,
  UpdateMusicTrackRequest,
  UpsertProfileRequest,
  UserMe,
  UserProfile,
} from './types.js'

export interface PageQuery {
  page?: number
  pageSize?: number
}

export interface PsychologyListQuery extends PageQuery {
  categoryId?: number
  contentType?: string
}

export interface MusicListQuery extends PageQuery {
  category?: string
  search?: string
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

export class ServerRsClient {
  readonly http: HttpClient
  readonly auth: AuthApi
  readonly users: UserApi
  readonly sessions: SessionApi
  readonly psychology: PsychologyApi
  readonly depression: DepressionApi
  readonly diaries: DiaryApi
  readonly community: CommunityApi
  readonly music: MusicApi
  readonly objects: ObjectApi
  readonly admin: AdminApi

  constructor(config: ServerRsClientConfig) {
    this.http = new HttpClient(config)
    this.auth = new AuthApi(this.http)
    this.users = new UserApi(this.http)
    this.sessions = new SessionApi(this.http)
    this.psychology = new PsychologyApi(this.http)
    this.depression = new DepressionApi(this.http)
    this.diaries = new DiaryApi(this.http)
    this.community = new CommunityApi(this.http)
    this.music = new MusicApi(this.http)
    this.objects = new ObjectApi(this.http)
    this.admin = new AdminApi(this.http)
  }

  health(): Promise<HealthResponse> {
    return this.http.request('GET', '/health', { auth: false })
  }
}

export function createServerRsClient(config: ServerRsClientConfig): ServerRsClient {
  return new ServerRsClient(config)
}

class AuthApi {
  constructor(private readonly http: HttpClient) {}

  async register(payload: RegisterRequest): Promise<LoginResponse> {
    const response = await this.http.request<LoginResponse>('POST', '/api/v1/auth/register', {
      auth: false,
      body: payload,
    })
    await saveLogin(this.http.tokenStore, response)
    return response
  }

  async login(payload: LoginRequest): Promise<LoginResponse> {
    const response = await this.http.request<LoginResponse>('POST', '/api/v1/auth/login', {
      auth: false,
      body: payload,
    })
    await saveLogin(this.http.tokenStore, response)
    return response
  }

  async refresh(refreshToken?: string): Promise<RefreshResponse> {
    const token = refreshToken ?? (await this.http.tokenStore?.getRefreshToken?.())
    if (!token) throw new Error('A refresh token is required')
    const response = await this.http.request<RefreshResponse>('POST', '/api/v1/auth/refresh', {
      auth: false,
      body: { refresh_token: token },
    })
    await this.http.tokenStore?.setTokens({
      accessToken: response.accessToken,
      refreshToken: response.refreshToken,
    })
    return response
  }

  async logout(reason?: string): Promise<void> {
    try {
      const refreshToken = await this.http.tokenStore?.getRefreshToken?.()
      if (refreshToken) {
        await this.http.request<void>('POST', '/api/v1/auth/logout', {
          auth: false,
          body: { refresh_token: refreshToken, reason },
        })
      }
    } finally {
      await this.http.tokenStore?.clear()
    }
  }

  me(): Promise<AuthUser> {
    return this.http.request('GET', '/api/v1/auth/me')
  }
}

class UserApi {
  constructor(private readonly http: HttpClient) {}

  me(): Promise<UserMe> {
    return this.http.request('GET', '/api/v1/users/me')
  }

  updateMe(payload: PatchMeRequest): Promise<UserMe> {
    return this.http.request('PATCH', '/api/v1/users/me', { body: payload })
  }

  deleteMe(): Promise<void> {
    return this.http.request('DELETE', '/api/v1/users/me')
  }

  profile(): Promise<UserProfile> {
    return this.http.request('GET', '/api/v1/users/me/profile')
  }

  updateProfile(payload: UpsertProfileRequest): Promise<UserProfile> {
    return this.http.request('PUT', '/api/v1/users/me/profile', { body: payload })
  }
}

class SessionApi {
  constructor(private readonly http: HttpClient) {}

  create(payload: SessionCreateRequest): Promise<SessionCreateResponse> {
    return this.http.request('POST', '/api/v1/llm/sessions', { body: payload })
  }

  sendMessage(sessionId: string, payload: MessageRequest): Promise<MessageResponse> {
    return this.http.request('POST', `/api/v1/llm/sessions/${segment(sessionId)}/messages`, {
      body: payload,
    })
  }

  status(sessionId: string): Promise<SessionStatus> {
    return this.http.request('GET', `/api/v1/llm/sessions/${segment(sessionId)}`)
  }

  conversations(userId: number): Promise<Conversation[]> {
    return this.http.request('GET', `/api/v1/users/${userId}/conversations`)
  }

  messages(userId: number, conversationId: number): Promise<ConversationMessage[]> {
    return this.http.request(
      'GET',
      `/api/v1/users/${userId}/conversations/${conversationId}`,
    )
  }

  riskDetections(query: { page?: number; size?: number } = {}): Promise<RiskDetectionPage> {
    return this.http.request('GET', '/api/v1/risk-detections', { query })
  }
}

class PsychologyApi {
  constructor(private readonly http: HttpClient) {}

  categories(): Promise<Category[]> {
    return this.http.request('GET', '/api/v1/psychology/categories', { auth: false })
  }

  categoryTree(): Promise<Category[]> {
    return this.http.request('GET', '/api/v1/psychology/categories/tree', { auth: false })
  }

  articles(query: PsychologyListQuery = {}): Promise<Paginated<Article>> {
    return this.http.request('GET', '/api/v1/psychology/articles', {
      auth: false,
      query,
    })
  }

  article(id: number): Promise<Article> {
    return this.http.request('GET', `/api/v1/psychology/articles/${id}`, { auth: false })
  }

  qna(query: PsychologyListQuery = {}): Promise<Paginated<Qna>> {
    return this.http.request('GET', '/api/v1/psychology/qna', { auth: false, query })
  }

  qnaItem(id: number): Promise<Qna> {
    return this.http.request('GET', `/api/v1/psychology/qna/${id}`, { auth: false })
  }

  resources(query: PsychologyListQuery = {}): Promise<Paginated<PsychologyResource>> {
    return this.http.request('GET', '/api/v1/psychology/resources', { auth: false, query })
  }

  resource(id: number): Promise<PsychologyResource> {
    return this.http.request('GET', `/api/v1/psychology/resources/${id}`, { auth: false })
  }

  favorites(query: PsychologyListQuery = {}): Promise<Paginated<Favorite>> {
    return this.http.request('GET', '/api/v1/psychology/favorites', { query })
  }

  toggleFavorite(contentType: string, contentId: number): Promise<FavoriteStatus> {
    return this.http.request('POST', '/api/v1/psychology/favorites', {
      body: { contentType, contentId },
    })
  }

  favoriteStatus(contentType: string, contentId: number): Promise<FavoriteStatus> {
    return this.http.request('GET', '/api/v1/psychology/favorites/check', {
      query: { contentType, contentId },
    })
  }

  toggleLike(contentType: string, contentId: number): Promise<LikeStatus> {
    return this.http.request('POST', '/api/v1/psychology/likes', {
      body: { contentType, contentId },
    })
  }
}

class DepressionApi {
  constructor(private readonly http: HttpClient) {}

  scales(): Promise<DepressionScale[]> {
    return this.http.request('GET', '/api/v1/depression/scales', { auth: false })
  }

  scale(id: number): Promise<DepressionScale> {
    return this.http.request('GET', `/api/v1/depression/scales/${id}`, { auth: false })
  }

  assessments(query: { page?: number; size?: number } = {}): Promise<DepressionAssessmentPage> {
    return this.http.request('GET', '/api/v1/depression/assessments', { query })
  }

  assessment(id: number): Promise<DepressionAssessment> {
    return this.http.request('GET', `/api/v1/depression/assessments/${id}`)
  }

  createAssessment(payload: CreateDepressionAssessmentRequest): Promise<DepressionAssessment> {
    return this.http.request('POST', '/api/v1/depression/assessments', { body: payload })
  }

  deleteAssessment(id: number): Promise<DeletedResponse> {
    return this.http.request('DELETE', `/api/v1/depression/assessments/${id}`)
  }
}

class DiaryApi {
  constructor(private readonly http: HttpClient) {}

  list(query: PageQuery = {}): Promise<Diary[]> {
    return this.http.request('GET', '/api/v1/diaries', { query })
  }

  get(id: number): Promise<Diary> {
    return this.http.request('GET', `/api/v1/diaries/${id}`)
  }

  create(payload: CreateDiaryRequest): Promise<Diary> {
    return this.http.request('POST', '/api/v1/diaries', { body: payload })
  }

  update(id: number, payload: UpdateDiaryRequest): Promise<Diary> {
    return this.http.request('PATCH', `/api/v1/diaries/${id}`, { body: payload })
  }

  delete(id: number): Promise<DeletedResponse> {
    return this.http.request('DELETE', `/api/v1/diaries/${id}`)
  }
}

class CommunityApi {
  constructor(private readonly http: HttpClient) {}

  posts(query: PageQuery = {}): Promise<CommunityPage<CommunityPost>> {
    return this.http.request('GET', '/api/v1/community/posts', { auth: false, query })
  }

  post(id: number): Promise<CommunityPost> {
    return this.http.request('GET', `/api/v1/community/posts/${id}`, { auth: false })
  }

  createPost(payload: CreateCommunityPostRequest): Promise<CommunityPost> {
    return this.http.request('POST', '/api/v1/community/posts', { body: payload })
  }

  updatePost(id: number, payload: UpdateCommunityPostRequest): Promise<CommunityPost> {
    return this.http.request('PUT', `/api/v1/community/posts/${id}`, { body: payload })
  }

  deletePost(id: number): Promise<void> {
    return this.http.request('DELETE', `/api/v1/community/posts/${id}`)
  }

  comments(postId: number, query: PageQuery = {}): Promise<CommunityPage<CommunityComment>> {
    return this.http.request('GET', `/api/v1/community/posts/${postId}/comments`, {
      auth: false,
      query,
    })
  }

  createComment(
    postId: number,
    payload: CreateCommunityCommentRequest,
  ): Promise<CommunityComment> {
    return this.http.request('POST', `/api/v1/community/posts/${postId}/comments`, {
      body: payload,
    })
  }

  deleteComment(postId: number, commentId: number): Promise<void> {
    return this.http.request(
      'DELETE',
      `/api/v1/community/posts/${postId}/comments/${commentId}`,
    )
  }

  likePost(postId: number): Promise<void> {
    return this.http.request('POST', `/api/v1/community/posts/${postId}/like`)
  }

  unlikePost(postId: number): Promise<void> {
    return this.http.request('DELETE', `/api/v1/community/posts/${postId}/like`)
  }

  likeComment(postId: number, commentId: number): Promise<void> {
    return this.http.request(
      'POST',
      `/api/v1/community/posts/${postId}/comments/${commentId}/like`,
    )
  }

  unlikeComment(postId: number, commentId: number): Promise<void> {
    return this.http.request(
      'DELETE',
      `/api/v1/community/posts/${postId}/comments/${commentId}/like`,
    )
  }
}

class MusicApi {
  constructor(private readonly http: HttpClient) {}

  tracks(query: MusicListQuery = {}): Promise<MusicTrackPage> {
    return this.http.request('GET', '/api/v1/music/tracks', { auth: false, query })
  }

  track(id: number): Promise<MusicTrack> {
    return this.http.request('GET', `/api/v1/music/tracks/${id}`, { auth: false })
  }

  stream(id: number): Promise<Blob> {
    return this.http.request('GET', `/api/v1/music/tracks/${id}/stream`, {
      auth: false,
      responseType: 'blob',
    })
  }
}

class ObjectApi {
  constructor(private readonly http: HttpClient) {}

  upload(file: Blob, bucket?: string, filename?: string): Promise<StoredObject> {
    const form = new FormData()
    if (filename) {
      form.append('file', file, filename)
    } else {
      form.append('file', file)
    }
    return this.http.request('POST', '/api/v1/objects/upload', {
      query: { bucket },
      body: form,
    })
  }

  download(id: number): Promise<Blob> {
    return this.http.request('GET', `/api/v1/objects/${id}`, { responseType: 'blob' })
  }

  metadata(id: number): Promise<StoredObject> {
    return this.http.request('GET', `/api/v1/objects/${id}/metadata`)
  }

  delete(id: number): Promise<DeletedResponse> {
    return this.http.request('DELETE', `/api/v1/objects/${id}`)
  }
}

class AdminApi {
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

  processRiskDetection(id: number, notes?: string): Promise<AdminRiskDetection> {
    return this.http.request('POST', `/api/v1/admin/risk-detections/${id}/process`, {
      body: { notes },
    })
  }

  createTrack(payload: CreateMusicTrackRequest): Promise<MusicTrack> {
    return this.http.request('POST', '/api/v1/admin/music', { body: payload })
  }

  tracks(query: MusicListQuery & { status?: 0 | 1 } = {}): Promise<MusicTrackPage> {
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

  psychologyArticles(query: AdminPsychologyQuery = {}): Promise<Paginated<Article>> {
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

  psychologyQna(query: AdminPsychologyQuery = {}): Promise<Paginated<Qna>> {
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

  psychologyResources(query: AdminPsychologyQuery = {}): Promise<Paginated<PsychologyResource>> {
    return this.http.request('GET', '/api/v1/admin/psychology/resources', { query })
  }

  psychologyResource(id: number): Promise<PsychologyResource> {
    return this.http.request('GET', `/api/v1/admin/psychology/resources/${id}`)
  }

  createPsychologyResource(
    payload: PsychologyResourceWriteRequest,
  ): Promise<PsychologyResource> {
    return this.http.request('POST', '/api/v1/admin/psychology/resources', { body: payload })
  }

  updatePsychologyResource(
    id: number,
    payload: PsychologyResourceWriteRequest,
  ): Promise<PsychologyResource> {
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
    return this.http.request(
      'POST',
      `/api/v1/admin/web-ingestion/reviews/${id}/publish`,
      { body: { notes } },
    )
  }
}

async function saveLogin(store: TokenStore | undefined, response: LoginResponse): Promise<void> {
  await store?.setTokens({
    accessToken: response.accessToken,
    refreshToken: response.refreshToken,
  })
}

function segment(value: string): string {
  return encodeURIComponent(value)
}
