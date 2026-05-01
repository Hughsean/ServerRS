use std::sync::Arc;
use tracing::warn;

use crate::domain::auth::password_hasher::PasswordHasher;
use crate::domain::auth::password_verifier::PasswordVerifier;
use crate::domain::auth::refresh_token_issuer::RefreshTokenIssuer;
use crate::domain::auth::refresh_token_revocation_repository::RefreshTokenRevocationRepository;
use crate::domain::auth::refresh_token_verifier::RefreshTokenVerifier;
use crate::domain::auth::token_issuer::TokenIssuer;
use crate::domain::auth::token_verifier::TokenVerifier;
use crate::domain::tasks::task_event::{
    LoginAuditTask, RefreshTokenRevokedTask, RefreshTokenRotatedTask, TaskEvent, UserRegisteredTask,
};
use crate::domain::tasks::task_publisher::TaskPublisher;
use crate::domain::user::user::{NewUser, UserStatus};
use crate::domain::user::user_repository::UserRepository;
use crate::shared::error::AppError;

/// Unified auth service — replaces LoginUseCase, RegisterUseCase, LogoutUseCase,
/// RefreshTokenUseCase, and VerifyAccessTokenUseCase.
pub struct AuthService {
    user_repo: Arc<dyn UserRepository>,
    password_hasher: Arc<dyn PasswordHasher>,
    password_verifier: Arc<dyn PasswordVerifier>,
    token_issuer: Arc<dyn TokenIssuer>,
    token_verifier: Arc<dyn TokenVerifier>,
    refresh_token_issuer: Arc<dyn RefreshTokenIssuer>,
    refresh_token_verifier: Arc<dyn RefreshTokenVerifier>,
    revocation_repo: Arc<dyn RefreshTokenRevocationRepository>,
    task_publisher: Arc<dyn TaskPublisher>,
}

#[derive(Debug, Clone)]
pub struct AuthenticatedUser {
    pub user_id: u64,
    pub username: String,
}

#[derive(Debug, Clone)]
pub struct LoginInput {
    pub username: String,
    pub password: String,
    pub device_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct AuthTokenPair {
    pub user_id: u64,
    pub access_token: String,
    pub refresh_token: String,
}

impl AuthService {
    pub fn new(
        user_repo: Arc<dyn UserRepository>,
        password_hasher: Arc<dyn PasswordHasher>,
        password_verifier: Arc<dyn PasswordVerifier>,
        token_issuer: Arc<dyn TokenIssuer>,
        token_verifier: Arc<dyn TokenVerifier>,
        refresh_token_issuer: Arc<dyn RefreshTokenIssuer>,
        refresh_token_verifier: Arc<dyn RefreshTokenVerifier>,
        revocation_repo: Arc<dyn RefreshTokenRevocationRepository>,
        task_publisher: Arc<dyn TaskPublisher>,
    ) -> Self {
        Self {
            user_repo,
            password_hasher,
            password_verifier,
            token_issuer,
            token_verifier,
            refresh_token_issuer,
            refresh_token_verifier,
            revocation_repo,
            task_publisher,
        }
    }

    // ── Login ──

    pub async fn login(&self, input: LoginInput) -> Result<AuthTokenPair, AppError> {
        let user = self
            .user_repo
            .find_by_username(&input.username)
            .await?
            .ok_or(AppError::Unauthorized)?;

        if !user.is_active() {
            let _ = self
                .task_publisher
                .publish(TaskEvent::LoginAudit(LoginAuditTask::failed(
                    input.username,
                    input.device_id,
                    "user is disabled",
                )))
                .await;
            return Err(AppError::Forbidden("user is disabled".into()));
        }

        if !self
            .password_verifier
            .verify(&input.password, &user.password_hash)?
        {
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

        let access_token = self.token_issuer.issue(user.id, &user.username)?;
        let refresh_token = self
            .refresh_token_issuer
            .issue_refresh(user.id, &user.username)?;

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
            access_token,
            refresh_token,
        })
    }

    // ── Register ──

    pub async fn register(
        &self,
        username: String,
        password: String,
        device_id: Option<String>,
    ) -> Result<AuthTokenPair, AppError> {
        if self.user_repo.find_by_username(&username).await?.is_some() {
            return Err(AppError::Conflict("username already exists".into()));
        }

        let hash = self.password_hasher.hash(&password)?;
        let user = self
            .user_repo
            .save(NewUser::new(username.clone(), hash, UserStatus::Active))
            .await?;

        let access_token = self.token_issuer.issue(user.id, &user.username)?;
        let refresh_token = self
            .refresh_token_issuer
            .issue_refresh(user.id, &user.username)?;

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
            access_token,
            refresh_token,
        })
    }

    // ── Logout ──

    pub async fn logout(
        &self,
        refresh_token: &str,
        reason: Option<String>,
    ) -> Result<bool, AppError> {
        let claims = self.refresh_token_verifier.verify_refresh(refresh_token)?;

        self.revocation_repo
            .revoke(claims.token_id.clone(), claims.expires_at)
            .await?;

        let _ = self
            .task_publisher
            .publish(TaskEvent::RefreshTokenRevoked(RefreshTokenRevokedTask {
                user_id: claims.user_id,
                username: claims.username,
                token_id: claims.token_id,
                reason,
            }))
            .await;

        Ok(true)
    }

    // ── Refresh token ──

    pub async fn refresh(
        &self,
        refresh_token: &str,
        device_id: Option<String>,
    ) -> Result<AuthTokenPair, AppError> {
        let claims = self.refresh_token_verifier.verify_refresh(refresh_token)?;

        if self.revocation_repo.is_revoked(&claims.token_id).await? {
            return Err(AppError::Unauthorized);
        }

        self.revocation_repo
            .revoke(claims.token_id.clone(), claims.expires_at)
            .await?;

        let access_token = self.token_issuer.issue(claims.user_id, &claims.username)?;
        let new_refresh = self
            .refresh_token_issuer
            .issue_refresh(claims.user_id, &claims.username)?;

        let _ = self
            .task_publisher
            .publish(TaskEvent::RefreshTokenRotated(RefreshTokenRotatedTask {
                user_id: claims.user_id,
                username: claims.username,
                old_token_id: claims.token_id,
                device_id,
            }))
            .await;

        Ok(AuthTokenPair {
            user_id: claims.user_id,
            access_token,
            refresh_token: new_refresh,
        })
    }

    // ── Verify access token (sync) ──

    pub fn verify(&self, token: &str) -> Result<AuthenticatedUser, AppError> {
        let claims = self.token_verifier.verify(token)?;
        Ok(AuthenticatedUser {
            user_id: claims.user_id,
            username: claims.username,
        })
    }
}
