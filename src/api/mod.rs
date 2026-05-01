pub mod dto;
pub mod handlers;
pub mod middleware;
pub mod response;
pub mod router;

use std::sync::Arc;

use crate::application::auth::auth_service::AuthService;
use crate::application::session::session_manager::SessionManager;
use crate::application::session::session_service::SessionService;
use crate::application::user::user_service::UserService;

#[derive(Clone)]
pub struct ApiState {
    pub auth: Arc<AuthService>,
    pub user: Arc<UserService>,
    pub session: Arc<SessionManager>,
    pub query: Arc<SessionService>,
}
