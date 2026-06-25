/**
 * 旧 SDK 兼容层 —— 统一入口
 *
 * 提供与 web/old/server/index.ts 相同类名/方法签名，
 * 方便旧代码迁移到新 SDK。
 *
 * 使用方法：
 * ```ts
 * import { DiariesApi, AdminApi } from '@serverrs/sdk/compat'
 * ```
 *
 * 需迁移的类（不提供兼容，因为接口已完全重构）：
 * - UsersApi（登录注册 API 完全不同，无 RSA 加密）
 * - LlmSessionsApi（会话模型从 session 改为 sessionless）
 * - ConversationsApi（被 chat API 替代）
 * - SignatureApi（已废弃）
 * - TestApi（已废弃）
 * - RiskDetectionApi（已废弃，返回 410）
 */

export { DiariesApi } from './diaries.js'
export { CommunityApi } from './community.js'
export { PsychologyApi } from './psychology.js'
export { MusicApi } from './music.js'
export { DepressionScaleApi, DepressionAssessmentApi } from './depression.js'
export { AdminApi } from './admin.js'
