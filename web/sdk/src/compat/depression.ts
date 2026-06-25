/**
 * 旧 SDK 兼容层 —— DepressionScaleApi + DepressionAssessmentApi
 *
 * 旧类名: DepressionScaleApi, DepressionAssessmentApi（来自 web/old/server/apis/）
 * 新实现: 内部委托给 UserClient.depression
 */

import { UserClient } from '../user-client.js'
import type { DepressionScale, DepressionAssessment } from '../types.js'

/** @deprecated 请使用 UserClient.depression 代替 */
export class DepressionScaleApi {
  constructor(private client: UserClient) {}

  listScales(): Promise<DepressionScale[]> {
    return this.client.depression.scales()
  }
}

/** @deprecated 请使用 UserClient.depression 代替 */
export class DepressionAssessmentApi {
  constructor(private client: UserClient) {}

  get(assessmentId: number): Promise<DepressionAssessment> {
    return this.client.depression.assessment(assessmentId)
  }

  listByUser(): Promise<DepressionAssessment[]> {
    return this.client.depression.assessments().then(page => page.items)
  }

  create(payload: any): Promise<DepressionAssessment> {
    return this.client.depression.createAssessment(payload)
  }

  delete(assessmentId: number): Promise<void> {
    return this.client.depression.deleteAssessment(assessmentId) as any
  }
}
