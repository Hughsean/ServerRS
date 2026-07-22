//! Chat 待审批收件箱：业务中立的纯数据查询端口与决策审计端口。
//!
//! 这里的端口只描述"当前用户有哪些待审批 Checkpoint"与"记录审批决策"两个
//! 业务用例，不暴露 SeaORM、数据库连接或 Entity。通用
//! `agent_core::graph::CheckpointStore` 继续保持业务无关，不增加
//! `user_id`/`conversation_id` 等 Chat 专属查询。

use agent_core::graph::{CheckpointId, RunId, SuspendReason};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::shared::error::AppError;

/// 待审批列表的默认分页大小。
pub const PENDING_APPROVAL_DEFAULT_LIMIT: u32 = 20;
/// 待审批列表允许的最大分页大小。
pub const PENDING_APPROVAL_MAX_LIMIT: u32 = 100;
/// 审批决策审计事件类型，写入 `agent_events.event_type`。
pub const CHAT_APPROVAL_DECISION_EVENT: &str = "tool_approval_decision";

/// 待审批工具调用的纯数据预览。
///
/// 工具参数属于敏感数据，只允许通过当前用户受保护的接口返回。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChatApprovalToolCallPreview {
    pub id: String,
    pub name: String,
    pub arguments: Value,
}

/// 待审批内容的纯数据预览。
///
/// 只包含用户做出批准/拒绝决定所需的最小信息；不包含完整 Checkpoint
/// payload、消息历史、记忆、画像、Effect Receipt 或内部 Trace。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChatApprovalPreview {
    pub approval_id: Uuid,
    pub prompt: String,
    pub tool_calls: Vec<ChatApprovalToolCallPreview>,
}

/// 从业务暂停数据中提取审批预览的端口。
///
/// 由 app 层为具体的 `SuspendData` 实现，基础设施适配器只依赖该领域接口，
/// 避免 `infra -> app` 反向依赖。返回 `None` 表示该暂停不是工具审批。
pub trait ChatApprovalPreviewSource {
    fn approval_preview(&self) -> Option<ChatApprovalPreview>;
}

/// 当前认证用户的一条待审批 Checkpoint（非消费式读取结果）。
#[derive(Debug, Clone, PartialEq)]
pub struct PendingChatApproval {
    pub checkpoint_id: CheckpointId,
    pub run_id: RunId,
    pub conversation_id: u64,
    pub reason: SuspendReason,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub approval: ChatApprovalPreview,
}

/// 待审批列表页。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PendingApprovalPage {
    pub items: Vec<PendingChatApproval>,
}

/// 当前用户待审批 Checkpoint 的查询端口。
///
/// 查询是非消费式的：不得把 Checkpoint 标记为 consumed、不得触发工具执行、
/// 不得修改运行状态。真正的消费仍然只能由 Resume 流程完成。
#[async_trait]
pub trait ChatApprovalQueryT: Send + Sync {
    /// 列出 `user_id` 名下 pending 且未过期的待审批 Checkpoint。
    ///
    /// 可选 `conversation_id` 过滤仍必须同时受 `user_id` 约束；结果按
    /// `created_at DESC, checkpoint_id DESC` 稳定排序。
    async fn list_pending_approvals(
        &self,
        user_id: u64,
        conversation_id: Option<u64>,
        limit: u32,
    ) -> Result<PendingApprovalPage, AppError>;

    /// 读取单个待审批 Checkpoint。
    ///
    /// 其他用户、已过期、已消费或不存在的 Checkpoint 统一返回 `Ok(None)`，
    /// 避免 ID 枚举。
    async fn get_pending_approval(
        &self,
        user_id: u64,
        checkpoint_id: CheckpointId,
    ) -> Result<Option<PendingChatApproval>, AppError>;
}

/// 审批决策（与 HTTP `decision` 字段一一对应）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChatApprovalDecision {
    Approve,
    Reject,
}

impl ChatApprovalDecision {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Approve => "approve",
            Self::Reject => "reject",
        }
    }
}

/// 审批决策审计事件的最小字段集合。
///
/// 绝不包含完整 Checkpoint payload、消息历史、工具参数或认证信息。
#[derive(Debug, Clone)]
pub struct ChatApprovalDecisionEvent {
    pub user_id: u64,
    pub conversation_id: u64,
    pub checkpoint_id: CheckpointId,
    pub run_id: RunId,
    pub approval_id: Uuid,
    pub decision: ChatApprovalDecision,
}

/// 审批决策审计端口。
///
/// 审计是最佳努力的：调用方必须在审计失败时仅记录日志，不能让已经成功
/// 完成的 Resume 被客户端误认为失败，也不得因此触发工具重放。
#[async_trait]
pub trait ChatApprovalAuditT: Send + Sync {
    async fn record_decision(&self, event: ChatApprovalDecisionEvent) -> Result<(), AppError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pending_approval_limits_are_documented_constants() {
        assert_eq!(PENDING_APPROVAL_DEFAULT_LIMIT, 20);
        assert_eq!(PENDING_APPROVAL_MAX_LIMIT, 100);
    }

    #[test]
    fn decision_serializes_to_wire_values() {
        assert_eq!(ChatApprovalDecision::Approve.as_str(), "approve");
        assert_eq!(ChatApprovalDecision::Reject.as_str(), "reject");
        assert_eq!(
            serde_json::to_value(ChatApprovalDecision::Approve).unwrap(),
            serde_json::json!("approve")
        );
        assert_eq!(
            serde_json::to_value(ChatApprovalDecision::Reject).unwrap(),
            serde_json::json!("reject")
        );
    }

    #[test]
    fn approval_preview_round_trips_json() {
        let preview = ChatApprovalPreview {
            approval_id: Uuid::new_v4(),
            prompt: "模型请求执行受控工具，请确认是否允许。".into(),
            tool_calls: vec![ChatApprovalToolCallPreview {
                id: "call-1".into(),
                name: "fetch_web_content".into(),
                arguments: serde_json::json!({"url": "https://example.com"}),
            }],
        };

        let value = serde_json::to_value(&preview).unwrap();
        assert_eq!(
            value["tool_calls"][0]["name"],
            serde_json::json!("fetch_web_content")
        );
        let restored: ChatApprovalPreview = serde_json::from_value(value).unwrap();
        assert_eq!(restored, preview);
    }

    #[test]
    fn audit_event_type_matches_documented_name() {
        assert_eq!(CHAT_APPROVAL_DECISION_EVENT, "tool_approval_decision");
    }
}
