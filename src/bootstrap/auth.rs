use std::sync::Arc;

use sea_orm::DatabaseConnection;

use crate::application::auth::auth_service::{AuthConfig, AuthService};
use crate::domain::auth::password_service::PasswordService;
use crate::domain::auth::refresh_token_store::RefreshTokenStore;
use crate::domain::auth::token_service::TokenService;
use crate::domain::tasks::task_publisher::TaskPublisher;
use crate::domain::user::user_repository::UserRepository;
use crate::infrastructure::auth::bcrypt_password_hasher::BcryptPasswordHasher;
use crate::infrastructure::auth::jwt_token_service::JwtTokenService;
use crate::infrastructure::persistence::implementations::seaorm_refresh_token_store::SeaOrmRefreshTokenStore;
use crate::shared::config::{AuthConfig as AppAuthConfig, JwtConfig};

pub struct AuthGraph {
    pub auth_service: Arc<AuthService>,
}

pub fn build_auth(
    db: &DatabaseConnection,
    jwt_config: &JwtConfig,
    auth_config: &AppAuthConfig,
    user_repo: &Arc<dyn UserRepository>,
    task_publisher: &Arc<dyn TaskPublisher>,
) -> AuthGraph {
    let password_service: Arc<dyn PasswordService> = Arc::new(BcryptPasswordHasher::default());
    let revoke_repo: Arc<SeaOrmRefreshTokenStore> =
        Arc::new(SeaOrmRefreshTokenStore::new(db.clone()));
    let jwt: Arc<JwtTokenService> = Arc::new(JwtTokenService::new_with_ttls(
        &jwt_config.secret,
        jwt_config.access_ttl_secs,
        jwt_config.refresh_ttl_secs,
    ));

    let auth_service: Arc<AuthService> = Arc::new(AuthService::new(
        Arc::clone(user_repo),
        password_service as Arc<dyn PasswordService>,
        jwt as Arc<dyn TokenService>,
        revoke_repo as Arc<dyn RefreshTokenStore>,
        Arc::clone(task_publisher),
        AuthConfig {
            max_attempts: auth_config.max_login_attempts,
            lockout_secs: auth_config.lockout_duration_secs,
            access_ttl_secs: jwt_config.access_ttl_secs,
        },
    ));

    AuthGraph { auth_service }
}
