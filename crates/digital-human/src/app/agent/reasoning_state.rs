use crate::domain::agent::{AgentBusinessState, AgentContext, AgentToolCall};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolApprovalStatus {
    NotRequired,
    Pending,
    Approved,
    Rejected,
}

/// 可复用推理子图所需的最小业务状态能力。
///
/// 业务图通过实现该 trait 暴露工具执行上下文、深度和审计作用域；推理节点不需要
/// 依赖具体业务状态，也不获得可变引用。
pub trait ReasoningState: AgentBusinessState {
    fn reasoning_context(&self) -> Option<&AgentContext>;

    fn reasoning_tool_depth(&self) -> usize;

    fn reasoning_user_id(&self) -> u64;

    fn reasoning_conversation_id(&self) -> Option<u64>;

    fn increment_reasoning_tool_depth() -> Self::Update;

    /// 为受控工具调用构造业务更新和类型化暂停数据。
    /// 默认实现保持通用推理子图兼容；启用审批配置的业务状态必须覆盖此方法。
    fn request_tool_approval(
        &self,
        _calls: &[AgentToolCall],
    ) -> Option<(Self::Update, Self::SuspendData)> {
        None
    }

    fn tool_approval_status(&self) -> ToolApprovalStatus {
        ToolApprovalStatus::NotRequired
    }

    fn clear_tool_approval_update() -> Option<Self::Update> {
        None
    }
}
