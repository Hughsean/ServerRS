// 新 SDK 入口 —— 更新代码推荐使用以下导入
export { UserClient, createUserClient, type PageQuery, type PsychologyListQuery, type MusicListQuery } from './user-client.js'
export { AdminClient, createAdminClient, type KnowledgeReviewQuery, type RiskConversationQuery, type AdminPsychologyQuery } from './admin-client.js'

// 向后兼容：保留旧的合体版
export { ServerRsClient, createServerRsClient } from './client.js'

// 底层类型和工具
export * from './http.js'
export * from './types.js'

// 旧 SDK 兼容层（需显式导入）
//   import { DiariesApi } from '@serverrs/sdk/compat'
