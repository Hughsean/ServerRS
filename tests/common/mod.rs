use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::http::Request;
use serde_json::Value;
use tower::util::ServiceExt;

use ServerRS::api;
use ServerRS::application::auth::auth_service::AuthService;
use ServerRS::application::session::risk_detection_service::RiskDetectionService;
use ServerRS::application::session::session_manager::SessionManager;
use ServerRS::application::session::session_service::SessionService;
use ServerRS::application::user::user_service::UserService;
use ServerRS::domain::auth::password_hasher::PasswordHasher;
use ServerRS::domain::auth::password_verifier::PasswordVerifier;
use ServerRS::domain::auth::refresh_token_issuer::RefreshTokenIssuer;
use ServerRS::domain::auth::refresh_token_revocation_repository::RefreshTokenRevocationRepository;
use ServerRS::domain::auth::refresh_token_verifier::RefreshTokenVerifier;
use ServerRS::domain::auth::token_issuer::TokenIssuer;
use ServerRS::domain::auth::token_verifier::TokenVerifier;
use ServerRS::domain::conversation::conversation::{Conversation, NewConversation};
use ServerRS::domain::conversation::conversation_message::{
    ConversationMessage, NewConversationMessage,
};
use ServerRS::domain::conversation::conversation_repository::ConversationRepository;
use ServerRS::domain::risk::risk_detection_result::{NewRiskDetectionResult, RiskDetectionResult};
use ServerRS::domain::risk::risk_repository::RiskRepository;
use ServerRS::domain::tasks::task_publisher::TaskPublisher;
use ServerRS::domain::user::user::{NewUser, User, UserStatus, UserUpdate};
use ServerRS::domain::user::user_profile::{NewUserProfile, UserProfile, UserProfileUpdate};
use ServerRS::domain::user::user_profile_repository::UserProfileRepository;
use ServerRS::domain::user::user_repository::UserRepository;
use ServerRS::infrastructure::auth::bcrypt_password_hasher::BcryptPasswordHasher;
use ServerRS::infrastructure::auth::bcrypt_password_verifier::BcryptPasswordVerifier;
use ServerRS::infrastructure::auth::jwt_token_service::JwtTokenService;
use ServerRS::infrastructure::llm::ollama_client::OllamaClient;
use ServerRS::infrastructure::llm::prompt_provider::PromptProvider;
use ServerRS::infrastructure::tasks::in_memory_task_flow::{
    ResilientTaskPublisher, new_task_channel,
};
use ServerRS::shared::error::AppError;
use async_trait::async_trait;
use chrono::Utc;

// ── Mock repositories (in-memory, no MySQL needed) ──

struct MockUserRepo {
    users: std::sync::Mutex<Vec<User>>,
}

impl MockUserRepo {
    fn new() -> Self {
        let default_hash = bcrypt::hash("password123!", bcrypt::DEFAULT_COST).unwrap();
        let now = Utc::now();
        Self {
            users: std::sync::Mutex::new(vec![
                User {
                    id: 1,
                    username: "demo_user".into(),
                    password_hash: default_hash.clone(),
                    email: None,
                    phone: None,
                    nickname: Some("Demo".into()),
                    status: UserStatus::Active,
                    created_at: now,
                    updated_at: now,
                    last_login_at: None,
                },
                User {
                    id: 2,
                    username: "locked_user".into(),
                    password_hash: default_hash,
                    email: None,
                    phone: None,
                    nickname: Some("Locked".into()),
                    status: UserStatus::Disabled,
                    created_at: now,
                    updated_at: now,
                    last_login_at: None,
                },
            ]),
        }
    }
}

#[async_trait]
impl UserRepository for MockUserRepo {
    async fn find_by_id(&self, id: u64) -> Result<Option<User>, AppError> {
        Ok(self
            .users
            .lock()
            .unwrap()
            .iter()
            .find(|u| u.id == id)
            .cloned())
    }
    async fn find_by_username(&self, username: &str) -> Result<Option<User>, AppError> {
        Ok(self
            .users
            .lock()
            .unwrap()
            .iter()
            .find(|u| u.username == username)
            .cloned())
    }
    async fn find_by_email(&self, _: &str) -> Result<Option<User>, AppError> {
        Ok(None)
    }
    async fn find_by_phone(&self, _: &str) -> Result<Option<User>, AppError> {
        Ok(None)
    }
    async fn find_all(&self) -> Result<Vec<User>, AppError> {
        Ok(self.users.lock().unwrap().clone())
    }
    async fn save(&self, nu: NewUser) -> Result<User, AppError> {
        let mut users = self.users.lock().unwrap();
        let id = (users.len() + 1) as u64;
        let now = Utc::now();
        let user = User {
            id,
            username: nu.username,
            password_hash: nu.password_hash,
            email: nu.email,
            phone: nu.phone,
            nickname: nu.nickname,
            status: nu.status,
            created_at: now,
            updated_at: now,
            last_login_at: None,
        };
        users.push(user.clone());
        Ok(user)
    }
    async fn update(&self, id: u64, up: UserUpdate) -> Result<User, AppError> {
        let mut users = self.users.lock().unwrap();
        let user = users
            .iter_mut()
            .find(|u| u.id == id)
            .ok_or(AppError::NotFound("user not found".into()))?;
        if let Some(e) = up.email {
            user.email = e;
        }
        if let Some(p) = up.phone {
            user.phone = p;
        }
        if let Some(n) = up.nickname {
            user.nickname = n;
        }
        if let Some(s) = up.status {
            user.status = s;
        }
        user.updated_at = Utc::now();
        Ok(user.clone())
    }
    async fn delete_by_id(&self, id: u64) -> Result<bool, AppError> {
        let mut users = self.users.lock().unwrap();
        let before = users.len();
        users.retain(|u| u.id != id);
        Ok(users.len() < before)
    }
    async fn update_last_login(&self, _: u64) -> Result<(), AppError> {
        Ok(())
    }
}

// ── Mock profile repo ──
struct MockProfileRepo {
    profiles: std::sync::Mutex<Vec<UserProfile>>,
}

#[async_trait]
impl UserProfileRepository for MockProfileRepo {
    async fn find_by_user_id(&self, uid: u64) -> Result<Option<UserProfile>, AppError> {
        Ok(self
            .profiles
            .lock()
            .unwrap()
            .iter()
            .find(|p| p.user_id == uid)
            .cloned())
    }
    async fn save(&self, np: NewUserProfile) -> Result<UserProfile, AppError> {
        let mut profiles = self.profiles.lock().unwrap();
        let id = (profiles.len() + 1) as u64;
        let now = Utc::now();
        let profile = UserProfile {
            id,
            user_id: np.user_id,
            interests: np.interests,
            personality_traits: np.personality_traits,
            interaction_preferences: np.interaction_preferences,
            emotional_tendency: np.emotional_tendency,
            learning_records: np.learning_records,
            created_at: now,
            updated_at: now,
        };
        profiles.push(profile.clone());
        Ok(profile)
    }
    async fn update(&self, uid: u64, up: UserProfileUpdate) -> Result<UserProfile, AppError> {
        let mut profiles = self.profiles.lock().unwrap();
        let profile = profiles
            .iter_mut()
            .find(|p| p.user_id == uid)
            .ok_or(AppError::NotFound("not found".into()))?;
        if let Some(i) = up.interests {
            profile.interests = i;
        }
        if let Some(pt) = up.personality_traits {
            profile.personality_traits = pt;
        }
        profile.updated_at = Utc::now();
        Ok(profile.clone())
    }
    async fn delete_by_user_id(&self, _: u64) -> Result<bool, AppError> {
        Ok(true)
    }
}

// ── Mock conversation repo ──
struct MockConvRepo;

#[async_trait]
impl ConversationRepository for MockConvRepo {
    async fn find_by_id(&self, _: u64) -> Result<Option<Conversation>, AppError> {
        Ok(None)
    }
    async fn find_by_user_id(&self, _: u64) -> Result<Vec<Conversation>, AppError> {
        Ok(vec![])
    }
    async fn save(&self, nc: NewConversation) -> Result<Conversation, AppError> {
        Ok(Conversation {
            id: 1,
            user_id: nc.user_id,
            title: nc.title,
            is_title_generated: false,
            last_message_at: None,
            message_count: 0,
            created_at: Utc::now(),
        })
    }
    async fn update_title(&self, _: u64, _: &str) -> Result<(), AppError> {
        Ok(())
    }
    async fn touch_and_incr(&self, _: u64, _: i32) -> Result<(), AppError> {
        Ok(())
    }
    async fn delete_by_id(&self, _: u64) -> Result<bool, AppError> {
        Ok(true)
    }
    async fn save_message(
        &self,
        _: NewConversationMessage,
    ) -> Result<ConversationMessage, AppError> {
        Ok(ConversationMessage {
            id: 1,
            conversation_id: 1,
            sender_role: "user".into(),
            sender_user_id: None,
            message_type: "text".into(),
            content: "{}".into(),
            token_count: None,
            created_at: Utc::now(),
        })
    }
    async fn find_messages_by_conversation_id(
        &self,
        _: u64,
    ) -> Result<Vec<ConversationMessage>, AppError> {
        Ok(vec![])
    }
    async fn delete_messages_by_conversation_id(&self, _: u64) -> Result<u64, AppError> {
        Ok(0)
    }
}

// ── Mock risk repo ──
struct MockRiskRepo;

#[async_trait]
impl RiskRepository for MockRiskRepo {
    async fn save(&self, _: NewRiskDetectionResult) -> Result<RiskDetectionResult, AppError> {
        Ok(RiskDetectionResult {
            id: 1,
            user_id: 0,
            message_id: None,
            conversation_id: None,
            risk_level: "None".into(),
            polarity: "Neutral".into(),
            intent: "Narrative".into(),
            target: "Unknown".into(),
            confidence: 0.5,
            evidence: "[]".into(),
            reason: None,
            raw_payload: None,
            model_name: None,
            detector_version: None,
            is_processed: false,
            process_notes: None,
            created_at: Utc::now(),
        })
    }
    async fn find_by_user_id_paginated(
        &self,
        _: u64,
        _: u64,
        _: u64,
    ) -> Result<(Vec<RiskDetectionResult>, u64), AppError> {
        Ok((vec![], 0))
    }
    async fn find_by_conversation_id(&self, _: u64) -> Result<Vec<RiskDetectionResult>, AppError> {
        Ok(vec![])
    }
    async fn delete_by_conversation_id(&self, _: u64) -> Result<u64, AppError> {
        Ok(0)
    }
}

// ── Mock revocation repo (in-memory) ──
struct MockRevokeRepo;
#[async_trait]
impl RefreshTokenRevocationRepository for MockRevokeRepo {
    async fn revoke(&self, _: String, _: u64) -> Result<(), AppError> {
        Ok(())
    }
    async fn is_revoked(&self, _: &str) -> Result<bool, AppError> {
        Ok(false)
    }
    async fn cleanup_expired(&self, _: u64) -> Result<usize, AppError> {
        Ok(0)
    }
}

// ── App builder ──

pub async fn test_app() -> Router {
    let user_repo: Arc<dyn UserRepository> = Arc::new(MockUserRepo::new());
    let profile_repo: Arc<dyn UserProfileRepository> = Arc::new(MockProfileRepo {
        profiles: std::sync::Mutex::new(vec![]),
    });
    let conv_repo: Arc<dyn ConversationRepository> = Arc::new(MockConvRepo);
    let risk_repo: Arc<dyn RiskRepository> = Arc::new(MockRiskRepo);

    let pw_hash: Arc<dyn PasswordHasher> = Arc::new(BcryptPasswordHasher::default());
    let pw_verify: Arc<dyn PasswordVerifier> = Arc::new(BcryptPasswordVerifier);
    let revoke_repo: Arc<dyn RefreshTokenRevocationRepository> = Arc::new(MockRevokeRepo);
    let jwt = Arc::new(JwtTokenService::new(
        "test-secret-key-for-integration-tests",
        86400,
    ));

    let (tp, tw) = new_task_channel(256);
    tokio::spawn(tw.run());
    let task_publisher: Arc<dyn TaskPublisher> = Arc::new(tp);

    let auth: Arc<AuthService> = Arc::new(AuthService::new(
        Arc::clone(&user_repo),
        pw_hash,
        pw_verify,
        Arc::clone(&jwt) as Arc<dyn TokenIssuer>,
        Arc::clone(&jwt) as Arc<dyn TokenVerifier>,
        Arc::clone(&jwt) as Arc<dyn RefreshTokenIssuer>,
        Arc::clone(&jwt) as Arc<dyn RefreshTokenVerifier>,
        Arc::clone(&revoke_repo),
        Arc::clone(&task_publisher),
    ));
    let user: Arc<UserService> = Arc::new(UserService::new(
        Arc::clone(&user_repo),
        Arc::clone(&profile_repo),
    ));
    let query: Arc<SessionService> = Arc::new(SessionService::new(
        Arc::clone(&conv_repo),
        Arc::clone(&risk_repo),
    ));
    let risk_detect: Arc<RiskDetectionService> = Arc::new(RiskDetectionService::new(
        Arc::clone(&risk_repo),
        Arc::clone(&task_publisher),
    ));

    // Use a dummy Ollama that always returns fallback (no real server needed)
    let ollama = OllamaClient::new("http://127.0.0.1:1/v1".into(), "test-model".into());
    let session: Arc<SessionManager> = Arc::new(SessionManager::new(
        Arc::clone(&task_publisher),
        risk_detect,
        ollama,
        PromptProvider::new(None),
        Arc::clone(&conv_repo) as Arc<dyn ConversationRepository>,
        Arc::clone(&profile_repo),
        120,
    ));
    tokio::spawn({
        let s = Arc::clone(&session);
        async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(60)).await;
                s.cleanup().await;
            }
        }
    });

    let state = api::ApiState {
        auth,
        user,
        session,
        query,
    };
    api::router::build_router(state)
}

// ── HTTP helpers ──

pub async fn read_body(response: axum::response::Response) -> Value {
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    serde_json::from_slice(&body).unwrap()
}

pub async fn post(app: &Router, path: &str, body: &Value) -> Value {
    let req = Request::post(path)
        .header("Content-Type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    read_body(resp).await
}

pub async fn put_auth(app: &Router, path: &str, body: &Value, token: &str) -> Value {
    let req = Request::put(path)
        .header("Content-Type", "application/json")
        .header("Authorization", format!("Bearer {token}"))
        .body(Body::from(body.to_string()))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    read_body(resp).await
}

pub async fn get_auth(app: &Router, path: &str, token: &str) -> Value {
    let req = Request::get(path)
        .header("Authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    read_body(resp).await
}
