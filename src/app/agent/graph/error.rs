use super::{EffectError, EffectId, GraphIdError, NodeId, RouteKey};
use crate::domain::agent::AgentStateError;
use crate::shared::error::AppError;

#[derive(Debug, thiserror::Error)]
pub enum GraphBuildError {
    #[error("图中存在重复节点: {node}")]
    DuplicateNode { node: NodeId },
    #[error("转移源节点不存在: {node}")]
    UnknownTransitionSource { node: NodeId },
    #[error("节点已定义转移规则: {node}")]
    DuplicateTransition { node: NodeId },
    #[error("非法 Fragment 命名空间 `{namespace}`: {error}")]
    InvalidNamespace {
        namespace: String,
        #[source]
        error: GraphIdError,
    },
    #[error("Fragment 命名空间已挂载: {namespace}")]
    DuplicateNamespace { namespace: String },
    #[error("命名空间 {namespace} 生成的节点已存在: {node}")]
    NamespaceCollision { namespace: String, node: NodeId },
    #[error("命名空间 {namespace} 与局部节点 {local} 无法生成合法 NodeId: {error}")]
    InvalidNamespacedNodeId {
        namespace: String,
        local: NodeId,
        #[source]
        error: GraphIdError,
    },
    #[error("Fragment 未设置入口节点")]
    FragmentMissingEntry,
    #[error("Fragment 入口节点不存在: {node}")]
    FragmentEntryMissing { node: NodeId },
    #[error("Fragment 至少需要声明一个出口")]
    FragmentMissingExit,
    #[error("Fragment 节点未定义转移规则: {node}")]
    FragmentMissingTransition { node: NodeId },
    #[error("Fragment 内部不能直接使用 End: {node}")]
    FragmentContainsEnd { node: NodeId },
    #[error("Fragment 节点 {from} 指向不存在的局部节点 {target}")]
    FragmentDanglingTarget { from: NodeId, target: NodeId },
    #[error("Fragment 出口名称非法 `{name}`: {error}")]
    InvalidFragmentExitName {
        name: String,
        #[source]
        error: GraphIdError,
    },
    #[error("Fragment 出口名称重复: {name}")]
    DuplicateFragmentExit { name: String },
    #[error("Fragment 出口 {name} 的源节点不存在: {node}")]
    FragmentExitSourceMissing { name: String, node: NodeId },
    #[error("Fragment 出口 {name} 不是源节点 {node} 的未连接路由 {route}")]
    InvalidFragmentExitRoute {
        name: String,
        node: NodeId,
        route: RouteKey,
    },
    #[error("Fragment 节点 {node} 的路由 {route} 没有内部目标或声明出口")]
    UnresolvedFragmentRoute { node: NodeId, route: RouteKey },
    #[error("Fragment 节点 {node} 的 Router 重复声明路由 {route}")]
    DuplicateFragmentRoute { node: NodeId, route: RouteKey },
    #[error("Fragment 节点 {node} 为 Router 未声明的路由 {route} 配置了目标")]
    UnknownFragmentRouteTarget { node: NodeId, route: RouteKey },
    #[error("Fragment 出口不存在或已经连接: {exit}")]
    UnknownFragmentExit { exit: String },
    #[error("Fragment 出口目标节点不存在: {node}")]
    UnknownFragmentExitTarget { node: NodeId },
}

#[derive(Debug, thiserror::Error)]
pub enum GraphCompileError {
    #[error("Fragment 出口尚未连接: {exit}")]
    UnresolvedFragmentExit { exit: String },
    #[error("图未设置入口节点")]
    MissingEntry,
    #[error("入口节点不存在: {node}")]
    EntryNodeMissing { node: NodeId },
    #[error("节点未定义转移规则: {node}")]
    MissingTransition { node: NodeId },
    #[error("节点 {from} 指向不存在的节点 {target}")]
    DanglingTarget { from: NodeId, target: NodeId },
    #[error("节点 {node} 的分支没有目标")]
    EmptyBranch { node: NodeId },
    #[error("节点 {node} 的 Router 重复声明路由 {route}")]
    DuplicateKnownRoute { node: NodeId, route: RouteKey },
    #[error("节点 {node} 缺少已知路由 {route} 的目标")]
    MissingRouteTarget { node: NodeId, route: RouteKey },
    #[error("节点 {node} 为 Router 未声明的路由 {route} 配置了目标")]
    UnknownRouteTarget { node: NodeId, route: RouteKey },
    #[error("入口无法到达节点: {node}")]
    UnreachableNode { node: NodeId },
    #[error("图不存在终止路径")]
    NoTerminalPath,
    #[error("节点不存在通往终态的路径: {node}")]
    NodeCannotReachEnd { node: NodeId },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeErrorKind {
    Transient,
    Timeout,
    RateLimited,
    Permanent,
    Invariant,
    Cancelled,
}

#[derive(Debug, thiserror::Error)]
#[error("{kind:?}: {message}")]
pub struct NodeError {
    kind: NodeErrorKind,
    message: String,
    budget: Option<BudgetFailure>,
    application_error: Option<AppError>,
}

#[derive(Debug, Clone, Copy)]
struct BudgetFailure {
    resource: BudgetResource,
    limit: u64,
    attempted: u64,
}

impl NodeError {
    pub fn new(kind: NodeErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            budget: None,
            application_error: None,
        }
    }

    /// 保存领域端口返回的应用错误，使兼容门面能够恢复原有 HTTP 错误语义。
    pub fn from_application(error: AppError) -> Self {
        let kind = match &error {
            AppError::Infrastructure(_) => NodeErrorKind::Transient,
            AppError::Validation(_)
            | AppError::Unauthorized
            | AppError::Forbidden(_)
            | AppError::NotFound(_)
            | AppError::Conflict(_)
            | AppError::Internal(_)
            | AppError::NotImplemented(_)
            | AppError::Gone(_) => NodeErrorKind::Permanent,
        };
        Self {
            kind,
            message: error.to_string(),
            budget: None,
            application_error: Some(error),
        }
    }

    /// 在节点内部保留 Runtime 预算错误，使外层仍返回统一的 GraphRunError。
    pub fn from_graph_run(error: GraphRunError) -> Self {
        match error {
            GraphRunError::BudgetExceeded {
                resource,
                limit,
                attempted,
            } => Self {
                kind: NodeErrorKind::Invariant,
                message: format!("{resource:?} 预算不足"),
                budget: Some(BudgetFailure {
                    resource,
                    limit,
                    attempted,
                }),
                application_error: None,
            },
            GraphRunError::Cancelled => Self::new(NodeErrorKind::Cancelled, "图运行已取消"),
            GraphRunError::DeadlineExceeded => {
                Self::new(NodeErrorKind::Timeout, "图运行超过截止时间")
            }
            other => Self::new(NodeErrorKind::Invariant, other.to_string()),
        }
    }

    pub fn kind(&self) -> NodeErrorKind {
        self.kind
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    pub fn application_error(&self) -> Option<&AppError> {
        self.application_error.as_ref()
    }

    pub(crate) fn into_graph_run(self, node: NodeId) -> GraphRunError {
        if let Some(budget) = self.budget {
            return GraphRunError::BudgetExceeded {
                resource: budget.resource,
                limit: budget.limit,
                attempted: budget.attempted,
            };
        }
        if self.kind == NodeErrorKind::Cancelled {
            return GraphRunError::Cancelled;
        }
        GraphRunError::NodeFailed { node, error: self }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BudgetResource {
    Steps,
    LlmCalls,
    ToolCalls,
    Tokens,
}

#[derive(Debug, thiserror::Error)]
pub enum GraphRunError {
    #[error("图运行已取消")]
    Cancelled,
    #[error("图运行超过截止时间")]
    DeadlineExceeded,
    #[error("{resource:?} 预算不足：上限 {limit}，请求后为 {attempted}")]
    BudgetExceeded {
        resource: BudgetResource,
        limit: u64,
        attempted: u64,
    },
    #[error("编译后的图缺少节点: {node}")]
    MissingNode { node: NodeId },
    #[error("编译后的图缺少节点 {node} 的转移规则")]
    MissingTransition { node: NodeId },
    #[error("节点 {node} 执行失败: {error}")]
    NodeFailed {
        node: NodeId,
        #[source]
        error: NodeError,
    },
    #[error("节点 {node} 返回 Effect，但图运行器未配置 EffectExecutor")]
    MissingEffectExecutor { node: NodeId },
    #[error("节点 {node} 的 Effect {effect_id} 执行失败: {error}")]
    EffectFailed {
        node: NodeId,
        effect_id: EffectId,
        #[source]
        error: EffectError,
    },
    #[error("节点 {node} 产生了非法状态更新: {error}")]
    StateUpdateFailed {
        node: NodeId,
        #[source]
        error: AgentStateError,
    },
    #[error("节点 {node} 的 Router 返回未声明路由 {route}")]
    UnknownRoute { node: NodeId, route: RouteKey },
    #[error("图到达 End 时尚未产生 AgentOutcome")]
    MissingOutcome,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shared::error::AppError;

    #[test]
    fn application_error_keeps_its_variant_and_classification() {
        let error = NodeError::from_application(AppError::Conflict("turn changed".into()));

        assert_eq!(error.kind(), NodeErrorKind::Permanent);
        assert!(matches!(
            error.application_error(),
            Some(AppError::Conflict(message)) if message == "turn changed"
        ));
    }
}
