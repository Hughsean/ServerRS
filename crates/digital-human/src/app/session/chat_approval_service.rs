//! Chat 待审批收件箱的应用服务。
//!
//! 负责两个用例：
//! - 当前用户待审批 Checkpoint 的非消费式查询（列表与详情）；
//! - Resume 成功后的审批决策审计（最佳努力，失败仅记录日志）。

use std::sync::Arc;

use crate::app::agent::graph::CheckpointId;
use crate::domain::agent::{
    ChatApprovalAuditT, ChatApprovalDecisionEvent, ChatApprovalQueryT,
    PENDING_APPROVAL_DEFAULT_LIMIT, PENDING_APPROVAL_MAX_LIMIT, PendingApprovalPage,
    PendingChatApproval,
};
use crate::shared::error::AppError;

/// 待审批收件箱服务。只依赖领域端口，不接触 SeaORM 或数据库连接。
pub struct ChatApprovalService {
    approval_query: Arc<dyn ChatApprovalQueryT>,
    approval_audit: Arc<dyn ChatApprovalAuditT>,
}

impl ChatApprovalService {
    pub fn new(
        approval_query: Arc<dyn ChatApprovalQueryT>,
        approval_audit: Arc<dyn ChatApprovalAuditT>,
    ) -> Self {
        Self {
            approval_query,
            approval_audit,
        }
    }

    /// 列出当前用户的待审批 Checkpoint。
    ///
    /// `limit` 默认 [`PENDING_APPROVAL_DEFAULT_LIMIT`]，最大
    /// [`PENDING_APPROVAL_MAX_LIMIT`]；`conversation_id` 过滤仍受当前用户约束。
    pub async fn list_pending(
        &self,
        user_id: u64,
        conversation_id: Option<u64>,
        limit: Option<u32>,
    ) -> Result<PendingApprovalPage, AppError> {
        let limit = limit.unwrap_or(PENDING_APPROVAL_DEFAULT_LIMIT);
        if limit == 0 || limit > PENDING_APPROVAL_MAX_LIMIT {
            return Err(AppError::Validation(
                "limit must be between 1 and 100".into(),
            ));
        }
        self.approval_query
            .list_pending_approvals(user_id, conversation_id, limit)
            .await
    }

    /// 读取当前用户的单个待审批 Checkpoint。
    ///
    /// 其他用户、已过期、已消费或不存在统一映射为 `NotFound`，避免 ID 枚举。
    pub async fn get_pending(
        &self,
        user_id: u64,
        checkpoint_id: CheckpointId,
    ) -> Result<PendingChatApproval, AppError> {
        self.approval_query
            .get_pending_approval(user_id, checkpoint_id)
            .await?
            .ok_or_else(|| AppError::NotFound("Checkpoint 不存在或已失效".into()))
    }

    /// 最佳努力记录审批决策审计。
    ///
    /// 审计失败只写 warn 日志并返回 `false`：已经成功完成的 Resume 绝不能
    /// 因为审计写入失败而被客户端误认为失败，也不得触发工具重放。
    pub async fn audit_decision(&self, event: ChatApprovalDecisionEvent) -> bool {
        let checkpoint_id = event.checkpoint_id;
        if let Err(error) = self.approval_audit.record_decision(event).await {
            tracing::warn!(%checkpoint_id, %error, "failed to record tool approval decision audit");
            return false;
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use agent_core::graph::RunId;
    use async_trait::async_trait;
    use chrono::{Duration, Utc};

    use super::*;
    use crate::domain::agent::{
        ChatApprovalDecision, ChatApprovalPreview, ChatApprovalToolCallPreview,
    };

    #[derive(Default)]
    struct FakeApprovalQuery {
        calls: Mutex<Vec<(u64, Option<u64>, u32)>>,
        page: PendingApprovalPage,
        detail: Option<PendingChatApproval>,
        /// 模拟行属主；查询者不是属主时详情返回 None（与真实适配器一致）。
        owner: u64,
    }

    #[async_trait]
    impl ChatApprovalQueryT for FakeApprovalQuery {
        async fn list_pending_approvals(
            &self,
            user_id: u64,
            conversation_id: Option<u64>,
            limit: u32,
        ) -> Result<PendingApprovalPage, AppError> {
            self.calls
                .lock()
                .unwrap()
                .push((user_id, conversation_id, limit));
            Ok(self.page.clone())
        }

        async fn get_pending_approval(
            &self,
            user_id: u64,
            _checkpoint_id: CheckpointId,
        ) -> Result<Option<PendingChatApproval>, AppError> {
            if user_id != self.owner {
                return Ok(None);
            }
            Ok(self.detail.clone())
        }
    }

    #[derive(Default)]
    struct FakeApprovalAudit {
        events: Mutex<Vec<ChatApprovalDecisionEvent>>,
        fail: bool,
    }

    #[async_trait]
    impl ChatApprovalAuditT for FakeApprovalAudit {
        async fn record_decision(&self, event: ChatApprovalDecisionEvent) -> Result<(), AppError> {
            if self.fail {
                return Err(AppError::Infrastructure("audit store down".into()));
            }
            self.events.lock().unwrap().push(event);
            Ok(())
        }
    }

    fn pending_item() -> PendingChatApproval {
        PendingChatApproval {
            checkpoint_id: CheckpointId::new(),
            run_id: RunId::new(),
            conversation_id: 9,
            reason: agent_core::graph::SuspendReason::Approval,
            created_at: Utc::now(),
            expires_at: Utc::now() + Duration::hours(1),
            approval: ChatApprovalPreview {
                approval_id: uuid::Uuid::new_v4(),
                prompt: "approve".into(),
                tool_calls: vec![ChatApprovalToolCallPreview {
                    id: "call-1".into(),
                    name: "controlled_tool".into(),
                    arguments: serde_json::json!({"value": 7}),
                }],
            },
        }
    }

    fn service(
        query: Arc<FakeApprovalQuery>,
        audit: Arc<FakeApprovalAudit>,
    ) -> ChatApprovalService {
        ChatApprovalService::new(query, audit)
    }

    #[tokio::test]
    async fn list_pending_applies_the_default_limit_and_owner_scope() {
        let query = Arc::new(FakeApprovalQuery::default());
        let audit = Arc::new(FakeApprovalAudit::default());
        let service = service(query.clone(), audit);

        let page = service.list_pending(7, None, None).await.unwrap();

        assert!(page.items.is_empty());
        assert_eq!(query.calls.lock().unwrap().as_slice(), &[(7, None, 20)]);
    }

    #[tokio::test]
    async fn list_pending_forwards_conversation_and_limit() {
        let query = Arc::new(FakeApprovalQuery::default());
        let audit = Arc::new(FakeApprovalAudit::default());
        let service = service(query.clone(), audit);

        service.list_pending(7, Some(9), Some(50)).await.unwrap();

        assert_eq!(query.calls.lock().unwrap().as_slice(), &[(7, Some(9), 50)]);
    }

    #[tokio::test]
    async fn list_pending_rejects_out_of_range_limits() {
        let service = service(
            Arc::new(FakeApprovalQuery::default()),
            Arc::new(FakeApprovalAudit::default()),
        );

        assert!(matches!(
            service.list_pending(7, None, Some(0)).await,
            Err(AppError::Validation(_))
        ));
        assert!(matches!(
            service.list_pending(7, None, Some(101)).await,
            Err(AppError::Validation(_))
        ));
        assert!(service.list_pending(7, None, Some(100)).await.is_ok());
    }

    #[tokio::test]
    async fn get_pending_maps_missing_to_not_found() {
        let service = service(
            Arc::new(FakeApprovalQuery::default()),
            Arc::new(FakeApprovalAudit::default()),
        );

        let error = service
            .get_pending(7, CheckpointId::new())
            .await
            .unwrap_err();

        assert!(matches!(error, AppError::NotFound(_)));
    }

    #[tokio::test]
    async fn get_pending_returns_the_detail() {
        let item = pending_item();
        let checkpoint_id = item.checkpoint_id;
        let query = Arc::new(FakeApprovalQuery {
            detail: Some(item),
            owner: 7,
            ..FakeApprovalQuery::default()
        });
        let service = service(query, Arc::new(FakeApprovalAudit::default()));

        let detail = service.get_pending(7, checkpoint_id).await.unwrap();

        assert_eq!(detail.checkpoint_id, checkpoint_id);
    }

    #[tokio::test]
    async fn get_pending_hides_other_users_checkpoint() {
        let query = Arc::new(FakeApprovalQuery {
            detail: Some(pending_item()),
            owner: 7,
            ..FakeApprovalQuery::default()
        });
        let service = service(query, Arc::new(FakeApprovalAudit::default()));

        let error = service
            .get_pending(8, CheckpointId::new())
            .await
            .unwrap_err();

        assert!(matches!(error, AppError::NotFound(_)));
    }

    #[tokio::test]
    async fn audit_decision_records_the_minimal_event() {
        let audit = Arc::new(FakeApprovalAudit::default());
        let service = service(Arc::new(FakeApprovalQuery::default()), audit.clone());
        let event = ChatApprovalDecisionEvent {
            user_id: 7,
            conversation_id: 9,
            checkpoint_id: CheckpointId::new(),
            run_id: RunId::new(),
            approval_id: uuid::Uuid::new_v4(),
            decision: ChatApprovalDecision::Approve,
        };
        let checkpoint_id = event.checkpoint_id;

        assert!(service.audit_decision(event).await);

        let events = audit.events.lock().unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].checkpoint_id, checkpoint_id);
        assert_eq!(events[0].decision, ChatApprovalDecision::Approve);
    }

    #[tokio::test]
    async fn audit_decision_swallows_failures_as_best_effort() {
        let audit = Arc::new(FakeApprovalAudit {
            fail: true,
            ..FakeApprovalAudit::default()
        });
        let service = service(Arc::new(FakeApprovalQuery::default()), audit);
        let event = ChatApprovalDecisionEvent {
            user_id: 7,
            conversation_id: 9,
            checkpoint_id: CheckpointId::new(),
            run_id: RunId::new(),
            approval_id: uuid::Uuid::new_v4(),
            decision: ChatApprovalDecision::Reject,
        };

        // 审计失败必须返回 false 而不是错误，调用方据此继续返回 Resume 结果。
        assert!(!service.audit_decision(event).await);
    }
}
