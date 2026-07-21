use std::sync::Arc;

use crate::app::auth::auth_service::AuthService;
use crate::app::user::user_service::UserService;
use crate::bootstrap::auth::AuthGraph;
use crate::domain::auth::token_service::TokenServiceT;

use super::BootstrapContext;

pub struct IdentityServices {
    pub auth: Arc<AuthService>,
    pub user: Arc<UserService>,
    pub token_service: Arc<dyn TokenServiceT>,
}

pub fn build_identity_services(
    ctx: &BootstrapContext<'_>,
    auth_graph: &AuthGraph,
) -> IdentityServices {
    let user = Arc::new(UserService::new(
        Arc::clone(&ctx.repos.user_repo),
        Arc::clone(&ctx.repos.profile_repo),
    ));

    IdentityServices {
        auth: Arc::clone(&auth_graph.auth_service),
        user,
        token_service: Arc::clone(&auth_graph.token_service),
    }
}
