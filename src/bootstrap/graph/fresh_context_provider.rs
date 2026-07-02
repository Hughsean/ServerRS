use crate::bootstrap::fresh_context;
use crate::bootstrap::tasks::BackgroundTasks;

use super::BootstrapContext;

pub async fn init_fresh_context_integration(
    ctx: &BootstrapContext<'_>,
    background: &mut BackgroundTasks,
) -> Result<(), std::io::Error> {
    fresh_context::init_fresh_context(
        ctx.config,
        &ctx.infra.db,
        &ctx.vector.vector_store,
        &ctx.vector.embedding_provider,
        background,
    )
    .await
    .map_err(|error| std::io::Error::new(std::io::ErrorKind::Other, error.to_string()))
}
