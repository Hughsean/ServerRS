export type JsonValue =
  | null
  | boolean
  | number
  | string
  | JsonValue[]
  | { [key: string]: JsonValue }

export interface ApiErrorBody {
  code?: string
  message?: string
  [key: string]: unknown
}

export interface AuthUser {
  id: number
  username: string
  role: string
}

export interface LoginRequest {
  username: string
  password: string
  device_id?: string
}

export type RegisterRequest = LoginRequest

export interface LoginResponse {
  accessToken: string
  refreshToken: string
  expiresIn: number
  tokenType: string
  user: AuthUser
}

export interface RefreshResponse {
  accessToken: string
  refreshToken: string
  expiresIn: number
}

export interface HealthResponse {
  status: string
  timestamp: string
}

// ── User ────────────────────────────────────────────────────────────────────

export interface UserMe {
  id: number
  username: string
  email: string | null
  phone: string | null
  nickname: string | null
  role: string
  status: string
  createdAt: string
}

export interface PatchMeRequest {
  email?: string
  phone?: string
  nickname?: string
}

export interface UserProfile {
  userId: number
  nickname: string | null
  interests: string[] | null
  personalityTraits: string[] | null
  interactionPreferences: string[] | null
  emotionalTendency: string[] | null
  learningRecords: string[] | null
}

export interface UpsertProfileRequest {
  nickname?: string
  interests?: string[]
  personalityTraits?: string[]
  interactionPreferences?: string[]
  emotionalTendency?: string[]
  learningRecords?: string[]
}

// ── Chat API (sessionless, per-user conversation) ──────────────────────────

export interface ChatOpenRequest {
  // No required fields; user_id comes from the Bearer token.
}

export interface ChatConversationInfo {
  id: number
  message_count: number
  last_message_at: string | null
}

export interface ChatOpenResponse {
  conversation: ChatConversationInfo
  personalization_enabled: boolean
}

export interface ChatMessageRequest {
  text: string
  emotion?: string
  location?: Record<string, JsonValue>
}

export interface ChatToolCallItem {
  name: string
  arguments: JsonValue
}

export interface ChatMessageResponse {
  conversation_id: number
  reply: string
  tool_calls: ChatToolCallItem[]
}

export interface ChatHistoryQuery {
  before_id?: number
  limit?: number
}

export interface ChatMessageItem {
  id: number
  sender_role: string
  message_type: string
  content: JsonValue
  created_at: string
}

export interface ChatHistoryResponse {
  messages: ChatMessageItem[]
  next_before_id: number | null
}

export interface ChatMemoryQuery {
  /** Query param: `type` */
  type?: string
  limit?: number
}

export interface ChatMemoryItem {
  memory_id: number
  memory_type: string
  content: string
  confidence: number
  reinforce_count: number
  created_at: string
  reinforced_at: string | null
}

export interface ChatMemoryResponse {
  memories: ChatMemoryItem[]
  total_active: number
}

export interface ChatPersonaSnapshotSummary {
  communication_preferences_count: number
  stable_facts_count: number
  recurring_topics_count: number
  goals_count: number
  sensitive_context_count: number
}

export interface ChatPersonaResponse {
  has_active_persona: boolean
  generated_at: string | null
  snapshot_summary: ChatPersonaSnapshotSummary
  personalization_enabled: boolean
}

export interface DisableMemoryResponse {
  memory_id: number
  disabled: boolean
}

export interface PersonaResetResponse {
  reset: boolean
}

export interface PersonaRebuildResponse {
  snapshot_id: number
}

export interface TranscriptClearResponse {
  cleared_messages: boolean
  cleared_summaries: boolean
  memories_preserved: boolean
  persona_preserved: boolean
  post_risk_audits_cleared: boolean
}

export interface ForgetResponse {
  messages_cleared: boolean
  summaries_cleared: boolean
  memories_disabled: number
  persona_expired: boolean
  post_risk_audits_deleted: boolean
  personalization_disabled: boolean
}

// ── Conversation (legacy/admin) ────────────────────────────────────────────

export interface Conversation {
  id: number
  user_id: number
  title: string | null
  is_title_generated: boolean
  last_message_at: string | null
  message_count: number
  created_at: string
}

export interface ConversationMessage {
  id: number
  conversation_id: number
  sender_role: string
  sender_user_id: number | null
  message_type: string
  content: string
  token_count: number | null
  created_at: string
}

// ── Depression ─────────────────────────────────────────────────────────────

export interface DepressionScale {
  scaleId: number
  scaleName: string
  scaleDescription: string | null
  minScore: number
  maxScore: number
}

export interface DepressionAssessment {
  assessmentId: number
  userId: number
  scaleId: number
  assessmentDate: string
  answers: JsonValue
  totalScore: number
  severityLevel: string
  notes: string | null
  createdAt: string
  updatedAt: string
}

export interface DepressionAssessmentPage {
  items: DepressionAssessment[]
  total: number
}

export interface CreateDepressionAssessmentRequest {
  scaleId: number
  answers: JsonValue
  notes?: string
}

// ── Diary ──────────────────────────────────────────────────────────────────

export interface Diary {
  id: number
  userId: number
  title: string
  content: string
  moodDescription: string | null
  createdAt: string
  updatedAt: string
}

export interface CreateDiaryRequest {
  content: string
  title?: string
}

export interface UpdateDiaryRequest {
  title?: string
  content?: string
}

// ── Community ──────────────────────────────────────────────────────────────

export interface CommunityPost {
  post_id: number
  user_id: number
  title: string | null
  content: string
  likes_count: number
  comments_count: number
}

export interface CommunityPage<T> {
  items: T[]
  page: number
  page_size: number
  total: number
}

export interface CreateCommunityPostRequest {
  title?: string
  content: string
}

export interface UpdateCommunityPostRequest {
  title?: string
  content?: string
}

export interface CreateCommunityCommentRequest {
  content: string
  parent_comment_id?: number
}

export interface CommunityComment {
  comment_id: number
  post_id: number
  user_id: number
  parent_comment_id: number | null
  content: string
  likes_count: number
}

// ── Psychology ─────────────────────────────────────────────────────────────

export interface Category {
  categoryId: number
  categoryName: string
  parentId: number | null
  description: string | null
  sortOrder: number
  isEnabled: boolean
  children: Category[]
}

export interface Article {
  articleId: number
  categoryId: number | null
  title: string
  summary: string | null
  content: string
  author: string | null
  source: string | null
  tags: string | null
  viewCount: number
  likeCount: number
  isFeatured: boolean
  isPublished: boolean
  createdAt: string
  updatedAt: string
}

export interface Qna {
  qnaId: number
  categoryId: number | null
  question: string
  answer: string
  expertName: string | null
  expertTitle: string | null
  tags: string | null
  viewCount: number
  likeCount: number
  isVerified: boolean
  isPublished: boolean
  createdAt: string
}

export interface PsychologyResource {
  resourceId: number
  categoryId: number | null
  resourceType: string
  title: string
  description: string | null
  objectId: number | null
  externalUrl: string | null
  fileSize: number | null
  mimeType: string | null
  duration: number | null
  tags: string | null
  viewCount: number
  likeCount: number
  isPublished: boolean
  createdAt: string
}

export interface CategoryWriteRequest {
  parentId?: number
  name: string
  description?: string
  sortOrder?: number
  isEnabled?: boolean
}

export interface ArticleWriteRequest {
  categoryId: number
  title: string
  summary?: string
  content: string
  author?: string
  source?: string
  tags?: JsonValue
  isFeatured?: boolean
  isPublished?: boolean
}

export interface QnaWriteRequest {
  categoryId: number
  question: string
  answer: string
  expertName?: string
  expertTitle?: string
  tags?: JsonValue
  isVerified?: boolean
  isPublished?: boolean
}

export interface PsychologyResourceWriteRequest {
  categoryId: number
  title: string
  description?: string
  /** Server allows any string, not a fixed enum */
  resourceType: string
  externalUrl?: string
  tags?: JsonValue
  isPublished?: boolean
}

export interface Paginated<T> {
  items: T[]
  page: number
  pageSize: number
  total: number
}

export interface Favorite {
  id: number
  contentType: string
  contentId: number
}

export interface FavoriteStatus {
  favorited: boolean
}

export interface LikeStatus {
  liked: boolean
}

// ── Music ──────────────────────────────────────────────────────────────────

export interface MusicTrack {
  musicId: number
  title: string
  artist: string | null
  album: string | null
  category: string | null
  description: string | null
  duration: number | null
  fileSize: number
  mimeType: string
  lyrics: string | null
  tags: JsonValue | null
  moodTags: JsonValue | null
  status: number
}

export interface MusicTrackPage {
  items: MusicTrack[]
  total: number
  page: number
  pageSize: number
}

export interface CreateMusicTrackRequest {
  title: string
  artist?: string
  album?: string
  category?: string
  description?: string
  duration?: number
  fileData: string
  mimeType: string
  coverImage?: string
  lyrics?: string
  tags?: JsonValue
  moodTags?: JsonValue
}

export interface UpdateMusicTrackRequest {
  title?: string
  artist?: string | null
  album?: string | null
  category?: string | null
  description?: string | null
  duration?: number | null
  lyrics?: string | null
  tags?: JsonValue | null
  moodTags?: JsonValue | null
  status?: number
}

// ── Object Storage ─────────────────────────────────────────────────────────

export interface StoredObject {
  id: number
  bucket: string
  objectKey: string
  mimeType: string
  sizeBytes: number
  publicUrl: string
  createdAt: string
}

// ── Admin ──────────────────────────────────────────────────────────────────

export interface AdminUser {
  id: number
  username: string
  email: string | null
  phone: string | null
  nickname: string | null
  status: string
  role: string
  created_at: string
  updated_at: string
  last_login_at: string | null
}

export interface AdminPatchUserRequest {
  status?: 0 | 1
  role?: 'USER' | 'ADMIN' | 'SUPER_ADMIN'
}

export interface DeletedResponse {
  deleted: boolean
}

export interface PaginatedAdminUsers {
  items: AdminUser[]
  page: number
  page_size: number
  total: number
}

/**
 * Admin-facing risk audit row from post_conversation_risk_audit.
 * Replaces the old AdminRiskDetection model.
 */
export interface RiskAuditAdminDto {
  audit_id: number
  conversation_id: number
  audit_scope: string
  status: string
  risk_level: string | null
  confidence: number | null
  detector_name: string | null
  error_message: string | null
  source_deleted: boolean
  created_at: string
}

export interface AdminRiskConversationDetail {
  conversation: Conversation
  messages: ConversationMessage[]
  risk_audits: RiskAuditAdminDto[]
}

export interface PaginatedRiskConversations {
  items: Conversation[]
  page: number
  page_size: number
  total: number
}

export type KnowledgePublishStatus =
  | 'staged'
  | 'published'
  | 'superseded'
  | 'rolled_back'
  | 'failed'

export interface KnowledgeReview {
  publish_record_id: number
  source_id: number
  source_name: string
  page_id: number
  run_id: number
  document_id: number
  version_key: string
  title: string | null
  source_url: string
  publish_status: KnowledgePublishStatus
  active: boolean
  run_status: string
  run_stage: string
  quality_score: number | null
  quality_result: JsonValue | null
  risk_flags: JsonValue | null
  should_publish: boolean | null
  created_at: string
  updated_at: string
}

export interface KnowledgeReviewPage {
  items: KnowledgeReview[]
  page: number
  page_size: number
  total: number
}

export interface KnowledgeReviewAudit {
  action: string
  status: string
  message: string
  metadata: JsonValue | null
  created_at: string
}

export interface KnowledgeReviewDetail {
  review: KnowledgeReview
  clean_text: string | null
  distilled_json: JsonValue | null
  audit_logs: KnowledgeReviewAudit[]
}

export interface ReviewPublishRequest {
  publish_record_id: number
  event_id: number
  event_status: string
  already_requested: boolean
}

// ── Signature ──────────────────────────────────────────────────────────────

export interface SignatureCreateRequest {
  appId: string
  appKey: string
  expiresIn?: number
}

export interface SignatureCreateResponse {
  token: string
  issuedAt: string
  expiresAt: string
}

export interface SignatureVerifyRequest {
  token: string
  appKey: string
}

export interface SignatureVerifyResponse {
  valid: boolean
  appId?: string
  issuedAt?: string
  expiresAt?: string
}

// ── Admin statistics ──

export interface StringCount {
  label: string
  count: number
}

export interface CountTrendResponse {
  total: number
  trend: StringCount[]
}

export interface RiskStatsResponse {
  total: number
  trend: StringCount[]
  distribution: StringCount[]
}
