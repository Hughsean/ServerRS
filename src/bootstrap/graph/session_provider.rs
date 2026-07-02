use std::sync::Arc;

use crate::app::agent::agent_runtime::AgentRuntime;
use crate::app::memory::memory_service::MemoryService;
use crate::app::session::chat_service::ChatService;
use crate::app::session::session_service::SessionService;
use crate::domain::conversation::conversation_repo::ConversationRepoT;
use crate::domain::tasks::task_publisher::TaskPublisher;

use super::BootstrapContext;

pub struct SessionServices {
    pub query: Arc<SessionService>,
    pub chat: Arc<ChatService>,
    pub conv_repo: Arc<dyn ConversationRepoT>,
}

pub fn build_session_services(
    ctx: &BootstrapContext<'_>,
    task_publisher: Arc<dyn TaskPublisher>,
    agent_runtime: Arc<AgentRuntime>,
    memory: Arc<MemoryService>,
) -> SessionServices {
    let conv_repo = Arc::clone(&ctx.repos.conv_repo);
    let query = Arc::new(SessionService::new(
        Arc::clone(&conv_repo),
        Arc::clone(&ctx.repos.risk_repo),
    ));
    let chat = Arc::new(ChatService::new(
        task_publisher,
        Arc::clone(&conv_repo),
        agent_runtime,
        memory,
        Arc::clone(&ctx.repos.context_control_repo),
        ctx.vector.vector_index.clone(),
    ));

    SessionServices {
        query,
        chat,
        conv_repo,
    }
}
