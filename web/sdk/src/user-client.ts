/**
 * UserClient —— 普通用户 API 客户端。
 *
 * 包含用户端所需的所有 API（auth/users/chat/psychology/depression/diaries/community/music/objects）。
 * 不包含管理员接口，防止在普通用户代码中意外使用管理员权限。
 */

import { HttpClient, type ServerRsClientConfig, type TokenStore } from './http.js'
import type {
  Article,
  AuthUser,
  Category,
  ChatCheckpointResumeRequest,
  ChatHistoryResponse,
  ChatMemoryResponse,
  ChatOpenResponse,
  ChatPersonaResponse,
  ChatTurnResponse,
  CommunityComment,
  CommunityPage,
  CommunityPost,
  CreateCommunityCommentRequest,
  CreateCommunityPostRequest,
  CreateDepressionAssessmentRequest,
  CreateDiaryRequest,
  DeletedResponse,
  DepressionAssessment,
  DepressionAssessmentPage,
  DepressionScale,
  Diary,
  DisableMemoryResponse,
  Favorite,
  FavoriteStatus,
  ForgetResponse,
  HealthResponse,
  LikeStatus,
  LoginRequest,
  LoginResponse,
  MusicTrack,
  MusicTrackPage,
  Paginated,
  PatchMeRequest,
  PendingChatApproval,
  PendingChatApprovalListResponse,
  PendingApprovalQuery,
  PersonaRebuildResponse,
  PersonaResetResponse,
  PsychologyResource,
  Qna,
  RefreshResponse,
  RegisterRequest,
  SignatureCreateRequest,
  SignatureCreateResponse,
  SignatureVerifyRequest,
  SignatureVerifyResponse,
  StoredObject,
  TranscriptClearResponse,
  UpdateCommunityPostRequest,
  UpdateDiaryRequest,
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

/**
 * 普通用户客户端 —— 包含用户端所需的所有 API。
 */
export class UserClient {
  readonly http: HttpClient
  readonly auth: AuthApi
  readonly users: UserApi
  readonly chat: ChatApi
  readonly psychology: PsychologyApi
  readonly depression: DepressionApi
  readonly diaries: DiaryApi
  readonly community: CommunityApi
  readonly music: MusicApi
  readonly objects: ObjectApi
  readonly signature: SignatureApi

  constructor(config: ServerRsClientConfig) {
    this.http = new HttpClient(config)
    this.auth = new AuthApi(this.http)
    this.users = new UserApi(this.http)
    this.chat = new ChatApi(this.http)
    this.psychology = new PsychologyApi(this.http)
    this.depression = new DepressionApi(this.http)
    this.diaries = new DiaryApi(this.http)
    this.community = new CommunityApi(this.http)
    this.music = new MusicApi(this.http)
    this.objects = new ObjectApi(this.http)
    this.signature = new SignatureApi(this.http)
  }

  health(): Promise<HealthResponse> {
    return this.http.request('GET', '/health', { auth: false })
  }
}

export function createUserClient(config: ServerRsClientConfig): UserClient {
  return new UserClient(config)
}

// ── Auth ──

export class AuthApi {
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
    if (!token) throw new Error('需要提供 refresh token')
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

// ── User ──

export class UserApi {
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

// ── Chat ──

export class ChatApi {
  constructor(private readonly http: HttpClient) {}

  open(payload: Record<string, never> = {}): Promise<ChatOpenResponse> {
    return this.http.request('POST', '/api/v1/chat/open', { body: payload })
  }

  sendMessage(payload: {
    text: string
    emotion?: string
    location?: Record<string, unknown>
  }): Promise<ChatTurnResponse> {
    return this.http.request('POST', '/api/v1/chat/messages', { body: payload })
  }

  resumeCheckpoint(
    checkpointId: string,
    payload: ChatCheckpointResumeRequest,
  ): Promise<ChatTurnResponse> {
    return this.http.request(
      'POST',
      `/api/v1/chat/checkpoints/${encodeURIComponent(checkpointId)}/resume`,
      { body: payload },
    )
  }

  /**
   * 列出当前用户的待审批 Checkpoint（非消费式查询）。
   *
   * 页面刷新或客户端重启后，用它重新发现待审批任务；查询不会消费
   * Checkpoint，也不会触发工具执行。
   */
  listPendingApprovals(query?: PendingApprovalQuery): Promise<PendingChatApprovalListResponse> {
    return this.http.request('GET', '/api/v1/chat/checkpoints/pending', {
      query: { conversation_id: query?.conversationId, limit: query?.limit },
    })
  }

  /**
   * 读取当前用户的单个待审批 Checkpoint（非消费式查询）。
   *
   * 其他用户、已过期、已消费或不存在的 Checkpoint 统一返回 404。
   */
  getCheckpoint(checkpointId: string): Promise<PendingChatApproval> {
    return this.http.request(
      'GET',
      `/api/v1/chat/checkpoints/${encodeURIComponent(checkpointId)}`,
    )
  }

  history(query?: { beforeId?: number; limit?: number }): Promise<ChatHistoryResponse> {
    return this.http.request('GET', '/api/v1/chat/history', { query })
  }

  memories(query?: { type?: string; limit?: number }): Promise<ChatMemoryResponse> {
    return this.http.request('GET', '/api/v1/chat/memories', { query })
  }

  persona(): Promise<ChatPersonaResponse> {
    return this.http.request('GET', '/api/v1/chat/persona')
  }

  disableMemory(id: number): Promise<DisableMemoryResponse> {
    return this.http.request('POST', `/api/v1/chat/memory/${id}/disable`)
  }

  personaReset(): Promise<PersonaResetResponse> {
    return this.http.request('POST', '/api/v1/chat/persona/reset')
  }

  personaRebuild(): Promise<PersonaRebuildResponse> {
    return this.http.request('POST', '/api/v1/chat/persona/rebuild')
  }

  transcriptClear(): Promise<TranscriptClearResponse> {
    return this.http.request('POST', '/api/v1/chat/transcript/clear')
  }

  forget(): Promise<ForgetResponse> {
    return this.http.request('POST', '/api/v1/chat/forget')
  }
}

// ── Psychology ──

export class PsychologyApi {
  constructor(private readonly http: HttpClient) {}

  categories(): Promise<Category[]> {
    return this.http.request('GET', '/api/v1/psychology/categories', { auth: false })
  }

  categoryTree(): Promise<Category[]> {
    return this.http.request('GET', '/api/v1/psychology/categories/tree', { auth: false })
  }

  articles(query: PsychologyListQuery = {}): Promise<Paginated<Article>> {
    return this.http.request('GET', '/api/v1/psychology/articles', { auth: false, query })
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
      body: { content_type: contentType, content_id: contentId },
    })
  }

  favoriteStatus(contentType: string, contentId: number): Promise<FavoriteStatus> {
    return this.http.request('GET', '/api/v1/psychology/favorites/check', {
      query: { content_type: contentType, content_id: contentId },
    })
  }

  toggleLike(contentType: string, contentId: number): Promise<LikeStatus> {
    return this.http.request('POST', '/api/v1/psychology/likes', {
      body: { content_type: contentType, content_id: contentId },
    })
  }
}

// ── Depression ──

export class DepressionApi {
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

// ── Diary ──

export class DiaryApi {
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

// ── Community ──

export class CommunityApi {
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

  createComment(postId: number, payload: CreateCommunityCommentRequest): Promise<CommunityComment> {
    return this.http.request('POST', `/api/v1/community/posts/${postId}/comments`, {
      body: payload,
    })
  }

  deleteComment(postId: number, commentId: number): Promise<void> {
    return this.http.request('DELETE', `/api/v1/community/posts/${postId}/comments/${commentId}`)
  }

  likePost(postId: number): Promise<void> {
    return this.http.request('POST', `/api/v1/community/posts/${postId}/like`)
  }

  unlikePost(postId: number): Promise<void> {
    return this.http.request('DELETE', `/api/v1/community/posts/${postId}/like`)
  }

  likeComment(postId: number, commentId: number): Promise<void> {
    return this.http.request('POST', `/api/v1/community/posts/${postId}/comments/${commentId}/like`)
  }

  unlikeComment(postId: number, commentId: number): Promise<void> {
    return this.http.request('DELETE', `/api/v1/community/posts/${postId}/comments/${commentId}/like`)
  }
}

// ── Music ──

export class MusicApi {
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

// ── Object ──

export class ObjectApi {
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

// ── Helper ──

async function saveLogin(store: TokenStore | undefined, response: LoginResponse): Promise<void> {
  await store?.setTokens({
    accessToken: response.accessToken,
    refreshToken: response.refreshToken,
  })
}

// ── Signature ──

export class SignatureApi {
  constructor(private readonly http: HttpClient) {}

  /** POST /api/v1/signature/create — 使用 appKey 签发 JWT 签名 */
  create(payload: SignatureCreateRequest): Promise<SignatureCreateResponse> {
    return this.http.request('POST', '/api/v1/signature/create', {
      auth: false,
      body: payload,
    })
  }

  /** POST /api/v1/signature/verify — 验证 JWT 签名 */
  verify(payload: SignatureVerifyRequest): Promise<SignatureVerifyResponse> {
    return this.http.request('POST', '/api/v1/signature/verify', {
      auth: false,
      body: payload,
    })
  }
}
