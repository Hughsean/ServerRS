use std::fmt;
use std::sync::Arc;
use std::time::{Duration, Instant};

use dashmap::DashMap;
use sha2::{Digest, Sha256};
use tracing::warn;

use crate::domain::auth::password_service::PasswordService;
use crate::domain::auth::refresh_token_store::RefreshTokenStore;
use crate::domain::auth::token_service::TokenService;
use crate::domain::tasks::task_event::{
    LoginAuditTask, RefreshTokenRevokedTask, RefreshTokenRotatedTask, TaskEvent, UserRegisteredTask,
};
use crate::domain::tasks::task_publisher::TaskPublisher;
use crate::domain::user::user::{NewUser, UserRole, UserStatus};
use crate::domain::user::user_repository::UserRepository;
use crate::shared::error::AppError;

// ── Supporting types ─────────────────────────────────────────────────────────

pub struct LoginAttemptRecord {
    pub failures: u32,
    pub locked_until: Option<Instant>,
}

#[derive(Clone)]
pub struct AuthConfig {
    pub max_attempts: u32,
    pub lockout_secs: u64,
    pub access_ttl_secs: u64,
}

impl Default for AuthConfig {
    fn default() -> Self {
        Self {
            max_attempts: 5,
            lockout_secs: 300,
            access_ttl_secs: 15 * 60,
        }
    }
}

#[derive(Debug, Clone)]
pub enum Role {
    Admin,
    SuperAdmin,
    User,
    Other(String),
}

impl Role {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Admin => "ADMIN",
            Self::SuperAdmin => "SUPER_ADMIN",
            Self::User => "USER",
            Self::Other(v) => v.as_str(),
        }
    }
}

impl From<&str> for Role {
    fn from(value: &str) -> Self {
        match value {
            "ADMIN" => Role::Admin,
            "SUPER_ADMIN" => Role::SuperAdmin,
            "USER" => Role::User,
            _ => match value.to_lowercase().as_str() {
                "admin" => Role::Admin,
                "super_admin" | "superadmin" => Role::SuperAdmin,
                "user" => Role::User,
                _ => Role::Other(value.to_string()),
            },
        }
    }
}

impl From<UserRole> for Role {
    fn from(value: UserRole) -> Self {
        match value {
            UserRole::User => Role::User,
            UserRole::Admin => Role::Admin,
            UserRole::SuperAdmin => Role::SuperAdmin,
        }
    }
}

impl fmt::Display for Role {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

#[derive(Debug, Clone)]
pub struct AuthenticatedUser {
    pub user_id: u64,
    pub username: String,
    pub role: Role,
}

#[derive(Debug, Clone)]
pub struct AuthTokenPair {
    pub user_id: u64,
    pub role: String,
    pub access_token: String,
    pub refresh_token: String,
    pub expires_in: u64,
    pub token_type: &'static str,
}

#[derive(Debug, Clone)]
pub struct LoginInput {
    pub username: String,
    pub password: String,
    pub device_id: Option<String>,
}

// ── AuthService ──────────────────────────────────────────────────────────────

pub struct AuthService {
    user_repo: Arc<dyn UserRepository>,
    password_service: Arc<dyn PasswordService>,
    token_service: Arc<dyn TokenService>,
    refresh_token_store: Arc<dyn RefreshTokenStore>,
    task_publisher: Arc<dyn TaskPublisher>,
    login_attempts: Arc<DashMap<String, LoginAttemptRecord>>,
    config: AuthConfig,
}

fn sha256_hex(s: &str) -> String {
    Sha256::digest(s.as_bytes())
        .iter()
        .map(|b| format!("{:02x}", b))
        .collect()
}

impl AuthService {
    pub fn new(
        user_repo: Arc<dyn UserRepository>,
        password_service: Arc<dyn PasswordService>,
        token_service: Arc<dyn TokenService>,
        refresh_token_store: Arc<dyn RefreshTokenStore>,
        task_publisher: Arc<dyn TaskPublisher>,
        config: AuthConfig,
    ) -> Self {
        Self {
            user_repo,
            password_service,
            token_service,
            refresh_token_store,
            task_publisher,
            login_attempts: Arc::new(DashMap::new()),
            config,
        }
    }

    fn issue_pair(
        &self,
        user_id: u64,
        username: &str,
        role: &str,
    ) -> Result<(String, String, u64), AppError> {
        let access = self.token_service.issue_access(user_id, username, role)?;
        let refresh = self.token_service.issue_refresh(user_id, username)?;
        Ok((access, refresh, self.config.access_ttl_secs))
    }

    fn check_lockout(&self, username: &str) -> Result<(), AppError> {
        if let Some(rec) = self.login_attempts.get(username) {
            if let Some(until) = rec.locked_until {
                if Instant::now() < until {
                    return Err(AppError::Forbidden("account temporarily locked".into()));
                }
            }
        }
        Ok(())
    }

    fn record_failure(&self, username: &str) {
        let mut rec = self
            .login_attempts
            .entry(username.to_string())
            .or_insert_with(|| LoginAttemptRecord {
                failures: 0,
                locked_until: None,
            });
        rec.failures += 1;
        if rec.failures >= self.config.max_attempts {
            rec.locked_until = Some(Instant::now() + Duration::from_secs(self.config.lockout_secs));
        }
    }

    fn clear_failures(&self, username: &str) {
        self.login_attempts.remove(username);
    }

    // ── login ────────────────────────────────────────────────────────────────

    pub async fn login(&self, input: LoginInput) -> Result<AuthTokenPair, AppError> {
        self.check_lockout(&input.username)?;

        let user = self
            .user_repo
            .find_by_username(&input.username)
            .await?
            .ok_or_else(|| {
                self.record_failure(&input.username);
                AppError::Unauthorized
            })?;

        if !user.is_active() {
            let _ = self
                .task_publisher
                .publish(TaskEvent::LoginAudit(LoginAuditTask::failed(
                    input.username.clone(),
                    input.device_id,
                    "user is disabled",
                )))
                .await;
            return Err(AppError::Forbidden("user is disabled".into()));
        }

        if !self
            .password_service
            .verify(&input.password, &user.password_hash)?
        {
            self.record_failure(&input.username);
            let _ = self
                .task_publisher
                .publish(TaskEvent::LoginAudit(LoginAuditTask::failed(
                    input.username,
                    input.device_id,
                    "invalid credentials",
                )))
                .await;
            return Err(AppError::Unauthorized);
        }

        self.clear_failures(&input.username);

        let (access_token, refresh_token, expires_in) =
            self.issue_pair(user.id, &user.username, user.role.as_str())?;

        let token_hash = sha256_hex(&refresh_token);
        self.refresh_token_store.store(user.id, token_hash).await?;

        if let Err(e) = self.user_repo.update_last_login(user.id).await {
            warn!(error = %e, user_id = user.id, "failed to update last login");
        }

        let _ = self
            .task_publisher
            .publish(TaskEvent::LoginAudit(LoginAuditTask::succeeded(
                user.username.clone(),
                input.device_id,
            )))
            .await;

        Ok(AuthTokenPair {
            user_id: user.id,
            role: user.role.as_str().to_string(),
            access_token,
            refresh_token,
            expires_in,
            token_type: "Bearer",
        })
    }

    // ── register ─────────────────────────────────────────────────────────────

    pub async fn register(
        &self,
        username: String,
        password: String,
        device_id: Option<String>,
    ) -> Result<AuthTokenPair, AppError> {
        if self.user_repo.find_by_username(&username).await?.is_some() {
            return Err(AppError::Conflict("username already exists".into()));
        }

        let hash = self.password_service.hash(&password)?;
        let user = self
            .user_repo
            .save(NewUser::new(username.clone(), hash, UserStatus::Active))
            .await?;

        let (access_token, refresh_token, expires_in) =
            self.issue_pair(user.id, &user.username, user.role.as_str())?;

        let token_hash = sha256_hex(&refresh_token);
        self.refresh_token_store.store(user.id, token_hash).await?;

        let _ = self
            .task_publisher
            .publish(TaskEvent::UserRegistered(UserRegisteredTask {
                user_id: user.id,
                username: user.username.clone(),
                device_id,
            }))
            .await;

        Ok(AuthTokenPair {
            user_id: user.id,
            role: user.role.as_str().to_string(),
            access_token,
            refresh_token,
            expires_in,
            token_type: "Bearer",
        })
    }

    // ── logout ───────────────────────────────────────────────────────────────

    pub async fn logout(&self, refresh_token_str: &str) -> Result<(), AppError> {
        let claims = self.token_service.verify_refresh(refresh_token_str)?;
        let token_hash = sha256_hex(refresh_token_str);
        self.refresh_token_store.revoke(&token_hash).await?;

        let _ = self
            .task_publisher
            .publish(TaskEvent::RefreshTokenRevoked(RefreshTokenRevokedTask {
                user_id: claims.user_id,
                username: claims.username,
                token_id: claims.token_id,
                reason: None,
            }))
            .await;

        Ok(())
    }

    // ── refresh ──────────────────────────────────────────────────────────────

    pub async fn refresh(&self, refresh_token_str: &str) -> Result<AuthTokenPair, AppError> {
        let claims = self.token_service.verify_refresh(refresh_token_str)?;
        let old_hash = sha256_hex(refresh_token_str);

        if self.refresh_token_store.is_revoked(&old_hash).await? {
            return Err(AppError::Unauthorized);
        }

        self.refresh_token_store.revoke(&old_hash).await?;

        let user = self
            .user_repo
            .find_by_id(claims.user_id)
            .await?
            .ok_or(AppError::Unauthorized)?;
        if !user.is_active() {
            return Err(AppError::Forbidden("user is disabled".into()));
        }

        let (access_token, new_refresh, expires_in) =
            self.issue_pair(claims.user_id, &claims.username, user.role.as_str())?;

        let new_hash = sha256_hex(&new_refresh);
        self.refresh_token_store
            .store(claims.user_id, new_hash)
            .await?;

        let _ = self
            .task_publisher
            .publish(TaskEvent::RefreshTokenRotated(RefreshTokenRotatedTask {
                user_id: claims.user_id,
                username: claims.username,
                old_token_id: claims.token_id,
                device_id: None,
            }))
            .await;

        Ok(AuthTokenPair {
            user_id: claims.user_id,
            role: user.role.as_str().to_string(),
            access_token,
            refresh_token: new_refresh,
            expires_in,
            token_type: "Bearer",
        })
    }

    // ── verify ───────────────────────────────────────────────────────────────

    pub async fn authenticate(&self, token: &str) -> Result<AuthenticatedUser, AppError> {
        let claims = self.token_service.verify_access(token)?;
        let user = self
            .user_repo
            .find_by_id(claims.user_id)
            .await?
            .ok_or(AppError::Unauthorized)?;
        if !user.is_active() {
            return Err(AppError::Forbidden("user is disabled".into()));
        }
        Ok(AuthenticatedUser {
            user_id: user.id,
            username: user.username,
            role: Role::from(user.role),
        })
    }
}
