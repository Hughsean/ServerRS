use crate::bootstrap::infra::InfraContext;
use crate::bootstrap::repos::RepoGraph;
use crate::bootstrap::vector::VectorContext;
use crate::shared::config::AppConfig;

/// Service provider 构造阶段共享的只读上下文。
pub struct BootstrapContext<'a> {
    pub config: &'a AppConfig,
    pub infra: &'a InfraContext,
    pub repos: &'a RepoGraph,
    pub vector: &'a VectorContext,
}
