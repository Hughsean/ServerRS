use crate::domain::agent::{AgentBusinessState, AgentContext};

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
}
