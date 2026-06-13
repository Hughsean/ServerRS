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

export interface SessionCreateRequest {
  user_id: number
  dialogue_id?: number
  location?: Record<string, JsonValue>
}

export interface SessionCreateResponse {
  session_id: string
  prompt: string
  location: Record<string, JsonValue> | null
  user_profile: JsonValue | null
  timeout_seconds: number
  dialogue_id: number | null
}

export interface MessageRequest {
  text: string
  emotion?: string
}

export interface MessageResponse {
  session_id: string
  reply: string
  session_closed: boolean
  dialogue_id: number | null
  title?: string
}

export interface SessionStatus {
  session_id: string
  user_id: number
  dialogue_id: number | null
  timeout_seconds: number
}

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

export interface RiskDetection {
  id: number
  conversation_id: number | null
  risk_level: string
  polarity: string
  intent: string
  reason: string | null
  confidence: number
  created_at: string
}

export interface RiskDetectionPage {
  items: RiskDetection[]
  total: number
  page: number
  size: number
}

export interface Category {
  categoryId: number
  categoryName: string
  parentId: number | null
  children: Category[]
}

export interface Article {
  articleId: number
  title: string
  summary: string | null
  author: string | null
  tags: string | null
  viewCount: number
  likeCount: number
  isFeatured: boolean
}

export interface Qna {
  qnaId: number
  question: string
  answer: string
  expertName: string | null
  isVerified: boolean
}

export interface PsychologyResource {
  resourceId: number
  resourceType: string
  title: string
  fileSize: number | null
  mimeType: string | null
}

export interface Paginated<T> {
  items: T[]
  page: number
  page_size: number
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

export interface StoredObject {
  id: number
  bucket: string
  objectKey: string
  mimeType: string
  sizeBytes: number
  publicUrl: string
  createdAt: string
}

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

export interface AdminRiskDetection {
  id: number
  conversation_id: number | null
  risk_level: string
  polarity: string
  intent: string
  confidence: number
  reason: string | null
  is_processed: boolean
  process_notes: string | null
  created_at: string
}

export interface AdminRiskConversationDetail {
  conversation: Conversation
  messages: ConversationMessage[]
  risk_detections: AdminRiskDetection[]
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
