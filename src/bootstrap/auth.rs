use std::sync::Arc;

use sea_orm::DatabaseConnection;

use crate::app::auth::auth_service::{AuthConfig, AuthService};
use crate::domain::auth::password_service::PasswordServiceT;
use crate::domain::auth::refresh_token_store::RefreshTokenStoreT;
use crate::domain::auth::token_service::TokenServiceT;
use crate::domain::tasks::task_publisher::TaskPublisher;
use crate::domain::user::user_repository::UserRepoT;
use crate::infra::auth::bcrypt_password_hasher::BcryptPasswordHasher;
use crate::infra::auth::jwt_token_service::JwtTokenService;
use crate::infra::repo::seaorm_impl::refresh_token_store::RefreshTokenStoreImpl;
use crate::shared::config::{AuthConfig as AppAuthConfig, JwtConfig};

pub struct AuthGraph {
    pub auth_service: Arc<AuthService>,
    pub refresh_token_store: Arc<dyn RefreshTokenStoreT>,
    pub token_service: Arc<dyn TokenServiceT>,
}

pub fn build_auth(
    db: &DatabaseConnection,
    jwt_config: &JwtConfig,
    auth_config: &AppAuthConfig,
    user_repo: &Arc<dyn UserRepoT>,
    task_publisher: &Arc<dyn TaskPublisher>,
) -> AuthGraph {
    let password_service: Arc<dyn PasswordServiceT> = Arc::new(BcryptPasswordHasher::default());
    let revoke_repo: Arc<RefreshTokenStoreImpl> = Arc::new(RefreshTokenStoreImpl::new(
        db.clone(),
        jwt_config.refresh_ttl_secs,
    ));
    let jwt: Arc<JwtTokenService> = Arc::new(JwtTokenService::new_with_ttls(
        &jwt_config.secret,
        jwt_config.access_ttl_secs,
        jwt_config.refresh_ttl_secs,
    ));

    let refresh_token_store: Arc<dyn RefreshTokenStoreT> = revoke_repo;
    let auth_service: Arc<AuthService> = Arc::new(AuthService::new(
        Arc::clone(user_repo),
        password_service as Arc<dyn PasswordServiceT>,
        jwt.clone() as Arc<dyn TokenServiceT>,
        Arc::clone(&refresh_token_store),
        Arc::clone(task_publisher),
        AuthConfig {
            max_attempts: auth_config.max_login_attempts,
            lockout_secs: auth_config.lockout_duration_secs,
            access_ttl_secs: jwt_config.access_ttl_secs,
        },
    ));

    AuthGraph {
        auth_service,
        refresh_token_store,
        token_service: jwt.clone() as Arc<dyn TokenServiceT>,
    }
}
