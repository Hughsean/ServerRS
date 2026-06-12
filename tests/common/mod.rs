use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::http::Request;
use serde_json::Value;
use tower::util::ServiceExt;

use ServerRS::api;
use ServerRS::application::auth::auth_service::AuthService;
use ServerRS::application::community::community_service::CommunityService;
use ServerRS::application::depression::depression_service::DepressionService;
use ServerRS::application::diary::diary_service::DiaryService;
use ServerRS::application::music::music_service::MusicService;
use ServerRS::application::psychology::psychology_service::PsychologyService;
use ServerRS::application::session::conversation_orchestrator::ConversationOrchestrator;
use ServerRS::application::session::risk_detection_service::RiskDetectionService;
use ServerRS::application::session::session_manager::SessionManager;
use ServerRS::application::session::session_service::SessionService;
use ServerRS::application::storage::object_service::ObjectService;
use ServerRS::application::user::user_service::UserService;
use ServerRS::domain::auth::password_service::PasswordService;
use ServerRS::domain::auth::refresh_token_revocation_repository::RefreshTokenRevocationRepository;
use ServerRS::domain::auth::token_service::TokenService;
use ServerRS::domain::community::{
    ArticleStatus, Comment, CommunityRepository, NewComment, NewPost, NewPostMedia, Post,
    PostMedia, PostUpdate,
};
use ServerRS::domain::conversation::conversation::{Conversation, NewConversation};
use ServerRS::domain::conversation::conversation_message::{
    ConversationMessage, NewConversationMessage,
};
use ServerRS::domain::conversation::conversation_repository::ConversationRepository;
use ServerRS::domain::depression::{
    DepressionAssessment, DepressionRepository, DepressionScale, NewDepressionAssessment,
};
use ServerRS::domain::diary::{DiaryRepository, NewUserDiary, UserDiary, UserDiaryUpdate};
use ServerRS::domain::like::{ContentLike, ContentLikeRepository};
use ServerRS::domain::llm::{ChatMessage, ChatResponse, LlmClient, PromptProvider};
use ServerRS::domain::music::{MusicRepository, MusicTrack, MusicTrackUpdate, NewMusicTrack};
use ServerRS::domain::psychology::{
    KnowledgeFavorite, NewContentLike, NewKnowledgeFavorite, NewPsychologyArticle,
    NewPsychologyCategory, NewPsychologyQna, NewPsychologyResource, PsychologyArticle,
    PsychologyCategory, PsychologyQna, PsychologyRepository, PsychologyResource,
};
use ServerRS::domain::risk::detection_types::DetectionResult;
use ServerRS::domain::risk::risk_detection_result::{NewRiskDetectionResult, RiskDetectionResult};
use ServerRS::domain::risk::risk_detector::RiskDetector;
use ServerRS::domain::risk::risk_repository::RiskRepository;
use ServerRS::domain::storage::{StoredObject, StoredObjectRepository};
use ServerRS::domain::tasks::task_publisher::TaskPublisher;
use ServerRS::domain::user::user::{NewUser, User, UserRole, UserStatus, UserUpdate};
use ServerRS::domain::user::user_profile::{NewUserProfile, UserProfile, UserProfileUpdate};
use ServerRS::domain::user::user_profile_repository::UserProfileRepository;
use ServerRS::domain::user::user_repository::UserRepository;
use ServerRS::infrastructure::auth::bcrypt_password_hasher::BcryptPasswordHasher;
use ServerRS::infrastructure::auth::jwt_token_service::JwtTokenService;
use ServerRS::infrastructure::storage::local_storage::LocalObjectStorage;
use ServerRS::infrastructure::tasks::in_memory_task_flow::new_task_channel;
use ServerRS::infrastructure::tasks::logging_handler::LoggingHandler;
use ServerRS::shared::error::AppError;
use async_trait::async_trait;
use chrono::Utc;

// ── Mock repositories (in-memory, no MySQL needed) ──

struct MockStoredObjectRepo {
    objects: std::sync::Mutex<Vec<StoredObject>>,
}

impl MockStoredObjectRepo {
    fn new() -> Self {
        Self {
            objects: std::sync::Mutex::new(Vec::new()),
        }
    }
}

#[async_trait]
impl StoredObjectRepository for MockStoredObjectRepo {
    async fn save(&self, mut object: StoredObject) -> Result<StoredObject, AppError> {
        let mut objects = self.objects.lock().unwrap();
        object.id = objects.len() as u64 + 1;
        objects.push(object.clone());
        Ok(object)
    }

    async fn find_by_id(&self, id: u64) -> Result<Option<StoredObject>, AppError> {
        Ok(self
            .objects
            .lock()
            .unwrap()
            .iter()
            .find(|object| object.id == id)
            .cloned())
    }

    async fn delete_by_id(&self, id: u64) -> Result<(), AppError> {
        self.objects
            .lock()
            .unwrap()
            .retain(|object| object.id != id);
        Ok(())
    }

    async fn find_by_sha256(&self, sha256: &str) -> Result<Option<StoredObject>, AppError> {
        Ok(self
            .objects
            .lock()
            .unwrap()
            .iter()
            .find(|object| object.sha256 == sha256)
            .cloned())
    }
}

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
                    role: UserRole::User,
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
                    role: UserRole::User,
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
            role: nu.role,
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
            risk_level: ServerRS::domain::risk::detection_types::RiskLevel::None,
            polarity: ServerRS::domain::risk::detection_types::Polarity::Neutral,
            intent: ServerRS::domain::risk::detection_types::IntentLabel::Narrative,
            target: ServerRS::domain::risk::detection_types::TargetLabel::Unknown,
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
    async fn find_all_paginated(
        &self,
        _: u64,
        _: u64,
        _: Option<ServerRS::domain::risk::detection_types::RiskLevel>,
    ) -> Result<(Vec<RiskDetectionResult>, u64), AppError> {
        Ok((vec![], 0))
    }
    async fn mark_processed(
        &self,
        _: u64,
        notes: Option<String>,
    ) -> Result<RiskDetectionResult, AppError> {
        Ok(RiskDetectionResult {
            id: 1,
            user_id: 0,
            message_id: None,
            conversation_id: None,
            risk_level: ServerRS::domain::risk::detection_types::RiskLevel::None,
            polarity: ServerRS::domain::risk::detection_types::Polarity::Neutral,
            intent: ServerRS::domain::risk::detection_types::IntentLabel::Narrative,
            target: ServerRS::domain::risk::detection_types::TargetLabel::Unknown,
            confidence: 0.5,
            evidence: "[]".into(),
            reason: None,
            raw_payload: None,
            model_name: None,
            detector_version: None,
            is_processed: true,
            process_notes: notes,
            created_at: Utc::now(),
        })
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

// ── Mock RefreshTokenStore (used by AuthService) ──

struct MockRefreshTokenStore;

#[async_trait]
impl ServerRS::domain::auth::refresh_token_store::RefreshTokenStore for MockRefreshTokenStore {
    async fn store(&self, _: u64, _: String) -> Result<(), AppError> {
        Ok(())
    }
    async fn is_revoked(&self, _: &str) -> Result<bool, AppError> {
        Ok(false)
    }
    async fn revoke(&self, _: &str) -> Result<(), AppError> {
        Ok(())
    }
    async fn cleanup_expired(&self, _: u64) -> Result<usize, AppError> {
        Ok(0)
    }
}

// ── Mock LLM client ──
struct MockLlmClient;

#[async_trait]
impl LlmClient for MockLlmClient {
    async fn chat(&self, _messages: &[ChatMessage]) -> String {
        "你好，我是小美，有什么可以帮你的？".to_string()
    }

    async fn chat_raw(
        &self,
        _messages: &[ChatMessage],
        _tools: Option<&[serde_json::Value]>,
    ) -> Result<ChatResponse, String> {
        let response: ChatResponse = serde_json::from_value(serde_json::json!({
            "choices": [{
                "message": {
                    "content": "你好，我是小美，有什么可以帮你的？",
                    "tool_calls": null
                }
            }]
        }))
        .map_err(|e| e.to_string())?;
        Ok(response)
    }
}

// ── Mock prompt provider ──
struct MockPromptProvider;

impl PromptProvider for MockPromptProvider {
    fn get_prompt(&self, _date_time: &str) -> String {
        "你是一名心理陪伴师。".to_string()
    }
}

// ── Mock risk detector ──
struct MockRiskDetector;

impl RiskDetector for MockRiskDetector {
    fn evaluate(&self, _text: &str) -> DetectionResult {
        DetectionResult::unknown()
    }
}

// ── Mock PsychologyRepository ──

struct MockPsychologyRepo {
    categories: std::sync::Mutex<Vec<PsychologyCategory>>,
    articles: std::sync::Mutex<Vec<PsychologyArticle>>,
    qnas: std::sync::Mutex<Vec<PsychologyQna>>,
    resources: std::sync::Mutex<Vec<PsychologyResource>>,
    favorites: std::sync::Mutex<Vec<KnowledgeFavorite>>,
    likes: std::sync::Mutex<Vec<NewContentLike>>,
}

impl MockPsychologyRepo {
    fn new() -> Self {
        let now = Utc::now();
        Self {
            categories: std::sync::Mutex::new(vec![
                PsychologyCategory {
                    id: 1,
                    parent_id: None,
                    name: "情绪管理".into(),
                    description: Some("情绪管理相关文章".into()),
                    sort_order: 1,
                    is_enabled: true,
                    created_at: now,
                },
                PsychologyCategory {
                    id: 2,
                    parent_id: None,
                    name: "人际关系".into(),
                    description: Some("人际关系相关文章".into()),
                    sort_order: 2,
                    is_enabled: true,
                    created_at: now,
                },
            ]),
            articles: std::sync::Mutex::new(vec![]),
            qnas: std::sync::Mutex::new(vec![]),
            resources: std::sync::Mutex::new(vec![]),
            favorites: std::sync::Mutex::new(vec![]),
            likes: std::sync::Mutex::new(vec![]),
        }
    }
}

#[async_trait]
impl PsychologyRepository for MockPsychologyRepo {
    // Categories
    async fn find_category_by_id(&self, id: u64) -> Result<Option<PsychologyCategory>, AppError> {
        Ok(self
            .categories
            .lock()
            .unwrap()
            .iter()
            .find(|c| c.id == id)
            .cloned())
    }
    async fn list_categories(&self) -> Result<Vec<PsychologyCategory>, AppError> {
        Ok(self.categories.lock().unwrap().clone())
    }
    async fn create_category(
        &self,
        new: NewPsychologyCategory,
    ) -> Result<PsychologyCategory, AppError> {
        let mut cats = self.categories.lock().unwrap();
        let id = cats.len() as u64 + 1;
        let cat = PsychologyCategory {
            id,
            parent_id: new.parent_id,
            name: new.name,
            description: new.description,
            sort_order: new.sort_order,
            is_enabled: new.is_enabled,
            created_at: Utc::now(),
        };
        cats.push(cat.clone());
        Ok(cat)
    }
    async fn update_category(
        &self,
        id: u64,
        new: NewPsychologyCategory,
    ) -> Result<PsychologyCategory, AppError> {
        let mut cats = self.categories.lock().unwrap();
        let cat = cats
            .iter_mut()
            .find(|c| c.id == id)
            .ok_or_else(|| AppError::NotFound("category".into()))?;
        cat.parent_id = new.parent_id;
        cat.name = new.name;
        cat.description = new.description;
        cat.sort_order = new.sort_order;
        cat.is_enabled = new.is_enabled;
        Ok(cat.clone())
    }
    async fn delete_category(&self, id: u64) -> Result<bool, AppError> {
        let mut cats = self.categories.lock().unwrap();
        let before = cats.len();
        cats.retain(|c| c.id != id);
        Ok(cats.len() < before)
    }

    // Articles
    async fn find_article_by_id(&self, id: u64) -> Result<Option<PsychologyArticle>, AppError> {
        Ok(self
            .articles
            .lock()
            .unwrap()
            .iter()
            .find(|a| a.id == id)
            .cloned())
    }
    async fn list_articles(
        &self,
        page: u64,
        page_size: u64,
        _search: Option<String>,
        _category_id: Option<u64>,
        _is_featured: Option<bool>,
    ) -> Result<(Vec<PsychologyArticle>, u64), AppError> {
        let items = self.articles.lock().unwrap();
        let total = items.len() as u64;
        let offset = ((page.saturating_sub(1)) * page_size) as usize;
        let page_items: Vec<PsychologyArticle> = items
            .iter()
            .skip(offset)
            .take(page_size as usize)
            .cloned()
            .collect();
        Ok((page_items, total))
    }
    async fn create_article(
        &self,
        new: NewPsychologyArticle,
    ) -> Result<PsychologyArticle, AppError> {
        let mut articles = self.articles.lock().unwrap();
        let id = articles.len() as u64 + 1;
        let now = Utc::now();
        let article = PsychologyArticle {
            id,
            category_id: new.category_id,
            title: new.title,
            summary: new.summary,
            content: new.content,
            tags: new.tags,
            view_count: 0,
            like_count: 0,
            is_published: new.is_published,
            created_at: now,
            updated_at: now,
        };
        articles.push(article.clone());
        Ok(article)
    }
    async fn update_article(
        &self,
        id: u64,
        new: NewPsychologyArticle,
    ) -> Result<PsychologyArticle, AppError> {
        let mut articles = self.articles.lock().unwrap();
        let article = articles
            .iter_mut()
            .find(|a| a.id == id)
            .ok_or_else(|| AppError::NotFound("article".into()))?;
        article.category_id = new.category_id;
        article.title = new.title;
        article.summary = new.summary;
        article.content = new.content;
        article.is_published = new.is_published;
        article.updated_at = Utc::now();
        Ok(article.clone())
    }
    async fn delete_article(&self, id: u64) -> Result<bool, AppError> {
        let mut articles = self.articles.lock().unwrap();
        let before = articles.len();
        articles.retain(|a| a.id != id);
        Ok(articles.len() < before)
    }

    // QnA
    async fn find_qna_by_id(&self, id: u64) -> Result<Option<PsychologyQna>, AppError> {
        Ok(self
            .qnas
            .lock()
            .unwrap()
            .iter()
            .find(|q| q.id == id)
            .cloned())
    }
    async fn list_qnas(
        &self,
        page: u64,
        page_size: u64,
        _category_id: Option<u64>,
        _is_verified: Option<bool>,
    ) -> Result<(Vec<PsychologyQna>, u64), AppError> {
        let items = self.qnas.lock().unwrap();
        let total = items.len() as u64;
        let offset = ((page.saturating_sub(1)) * page_size) as usize;
        let page_items: Vec<PsychologyQna> = items
            .iter()
            .skip(offset)
            .take(page_size as usize)
            .cloned()
            .collect();
        Ok((page_items, total))
    }
    async fn create_qna(&self, new: NewPsychologyQna) -> Result<PsychologyQna, AppError> {
        let mut qnas = self.qnas.lock().unwrap();
        let id = qnas.len() as u64 + 1;
        let qna = PsychologyQna {
            id,
            category_id: new.category_id,
            question: new.question,
            answer: new.answer,
            tags: new.tags,
            view_count: 0,
            like_count: 0,
            is_published: new.is_published,
            created_at: Utc::now(),
        };
        qnas.push(qna.clone());
        Ok(qna)
    }
    async fn update_qna(&self, id: u64, new: NewPsychologyQna) -> Result<PsychologyQna, AppError> {
        let mut qnas = self.qnas.lock().unwrap();
        let qna = qnas
            .iter_mut()
            .find(|q| q.id == id)
            .ok_or_else(|| AppError::NotFound("qna".into()))?;
        qna.category_id = new.category_id;
        qna.question = new.question;
        qna.answer = new.answer;
        qna.tags = new.tags;
        qna.is_published = new.is_published;
        Ok(qna.clone())
    }
    async fn delete_qna(&self, id: u64) -> Result<bool, AppError> {
        let mut qnas = self.qnas.lock().unwrap();
        let before = qnas.len();
        qnas.retain(|q| q.id != id);
        Ok(qnas.len() < before)
    }

    // Resources
    async fn find_resource_by_id(&self, id: u64) -> Result<Option<PsychologyResource>, AppError> {
        Ok(self
            .resources
            .lock()
            .unwrap()
            .iter()
            .find(|r| r.id == id)
            .cloned())
    }
    async fn list_resources(
        &self,
        page: u64,
        page_size: u64,
        _category_id: Option<u64>,
        _resource_type: Option<String>,
    ) -> Result<(Vec<PsychologyResource>, u64), AppError> {
        let items = self.resources.lock().unwrap();
        let total = items.len() as u64;
        let offset = ((page.saturating_sub(1)) * page_size) as usize;
        let page_items: Vec<PsychologyResource> = items
            .iter()
            .skip(offset)
            .take(page_size as usize)
            .cloned()
            .collect();
        Ok((page_items, total))
    }
    async fn create_resource(
        &self,
        new: NewPsychologyResource,
    ) -> Result<PsychologyResource, AppError> {
        let mut resources = self.resources.lock().unwrap();
        let id = resources.len() as u64 + 1;
        let resource = PsychologyResource {
            id,
            category_id: new.category_id,
            title: new.title,
            description: new.description,
            resource_type: new.resource_type,
            object_id: new.object_id,
            external_url: new.external_url,
            tags: new.tags,
            view_count: 0,
            like_count: 0,
            is_published: new.is_published,
            created_at: Utc::now(),
        };
        resources.push(resource.clone());
        Ok(resource)
    }
    async fn update_resource(
        &self,
        id: u64,
        new: NewPsychologyResource,
    ) -> Result<PsychologyResource, AppError> {
        let mut resources = self.resources.lock().unwrap();
        let resource = resources
            .iter_mut()
            .find(|r| r.id == id)
            .ok_or_else(|| AppError::NotFound("resource".into()))?;
        resource.category_id = new.category_id;
        resource.title = new.title;
        resource.description = new.description;
        resource.resource_type = new.resource_type;
        resource.object_id = new.object_id;
        resource.external_url = new.external_url;
        resource.tags = new.tags;
        resource.is_published = new.is_published;
        Ok(resource.clone())
    }
    async fn delete_resource(&self, id: u64) -> Result<bool, AppError> {
        let mut resources = self.resources.lock().unwrap();
        let before = resources.len();
        resources.retain(|r| r.id != id);
        Ok(resources.len() < before)
    }

    // Favorites
    async fn toggle_favorite(&self, new: NewKnowledgeFavorite) -> Result<bool, AppError> {
        let mut favs = self.favorites.lock().unwrap();
        if let Some(pos) = favs.iter().position(|f| {
            f.user_id == new.user_id
                && f.content_type == new.content_type
                && f.content_id == new.content_id
        }) {
            favs.remove(pos);
            Ok(false)
        } else {
            let new_id = favs.len() as u64 + 1;
            favs.push(KnowledgeFavorite {
                id: new_id,
                user_id: new.user_id,
                content_type: new.content_type,
                content_id: new.content_id,
                created_at: Utc::now(),
            });
            Ok(true)
        }
    }
    async fn check_favorite(
        &self,
        user_id: u64,
        content_type: &str,
        content_id: u64,
    ) -> Result<bool, AppError> {
        let favs = self.favorites.lock().unwrap();
        Ok(favs.iter().any(|f| {
            f.user_id == user_id && f.content_type == content_type && f.content_id == content_id
        }))
    }
    async fn list_favorites(
        &self,
        user_id: u64,
        content_type: Option<&str>,
    ) -> Result<Vec<KnowledgeFavorite>, AppError> {
        let favs = self.favorites.lock().unwrap();
        Ok(favs
            .iter()
            .filter(|f| {
                f.user_id == user_id && content_type.map_or(true, |ct| f.content_type == ct)
            })
            .cloned()
            .collect())
    }
    async fn toggle_like(&self, new: NewContentLike) -> Result<bool, AppError> {
        let mut likes = self.likes.lock().unwrap();
        if let Some(pos) = likes
            .iter()
            .position(|l| l.content_type == new.content_type && l.content_id == new.content_id)
        {
            likes.remove(pos);
            Ok(false)
        } else {
            likes.push(new);
            Ok(true)
        }
    }
}

// ── Mock DepressionRepository ──

struct MockDepressionRepo {
    scales: std::sync::Mutex<Vec<DepressionScale>>,
    assessments: std::sync::Mutex<Vec<DepressionAssessment>>,
}

impl MockDepressionRepo {
    fn new() -> Self {
        Self {
            scales: std::sync::Mutex::new(vec![
                DepressionScale {
                    scale_id: 1,
                    scale_name: "PHQ-9".into(),
                    scale_description: Some("Patient Health Questionnaire".into()),
                    min_score: 0,
                    max_score: 27,
                    questions: serde_json::json!(["Little interest or pleasure in doing things"]),
                    severity_ranges: serde_json::json!([{"min": 0, "max": 4, "label": "none"}]),
                    created_at: None,
                    updated_at: None,
                },
                DepressionScale {
                    scale_id: 2,
                    scale_name: "GAD-7".into(),
                    scale_description: Some("Generalized Anxiety Disorder".into()),
                    min_score: 0,
                    max_score: 21,
                    questions: serde_json::json!(["Feeling nervous, anxious or on edge"]),
                    severity_ranges: serde_json::json!([{"min": 0, "max": 4, "label": "none"}]),
                    created_at: None,
                    updated_at: None,
                },
            ]),
            assessments: std::sync::Mutex::new(vec![]),
        }
    }
}

#[async_trait]
impl DepressionRepository for MockDepressionRepo {
    async fn find_scale_by_id(&self, id: u16) -> Result<Option<DepressionScale>, AppError> {
        Ok(self
            .scales
            .lock()
            .unwrap()
            .iter()
            .find(|s| s.scale_id == id)
            .cloned())
    }
    async fn list_scales(&self) -> Result<Vec<DepressionScale>, AppError> {
        Ok(self.scales.lock().unwrap().clone())
    }
    async fn save_assessment(
        &self,
        new: NewDepressionAssessment,
        total_score: i16,
    ) -> Result<DepressionAssessment, AppError> {
        let mut assessments = self.assessments.lock().unwrap();
        let id = assessments.len() as u64 + 1;
        let assessment = DepressionAssessment {
            assessment_id: id,
            user_id: new.user_id,
            scale_id: new.scale_id,
            assessment_date: Utc::now().date_naive(),
            answers: new.answers,
            total_score,
            notes: new.notes,
            created_at: None,
            updated_at: None,
        };
        assessments.push(assessment.clone());
        Ok(assessment)
    }
    async fn find_assessment_by_id(
        &self,
        id: u64,
    ) -> Result<Option<DepressionAssessment>, AppError> {
        Ok(self
            .assessments
            .lock()
            .unwrap()
            .iter()
            .find(|a| a.assessment_id == id)
            .cloned())
    }
    async fn find_assessments_by_user_id(
        &self,
        user_id: u64,
        limit: u64,
        offset: u64,
    ) -> Result<(Vec<DepressionAssessment>, u64), AppError> {
        let items = self.assessments.lock().unwrap();
        let total = items.len() as u64;
        let filtered: Vec<DepressionAssessment> = items
            .iter()
            .filter(|a| a.user_id == user_id)
            .cloned()
            .collect();
        let page: Vec<DepressionAssessment> = filtered
            .iter()
            .skip(offset as usize)
            .take(limit as usize)
            .cloned()
            .collect();
        Ok((page, total))
    }
    async fn update_assessment(
        &self,
        id: u64,
        notes: Option<String>,
    ) -> Result<DepressionAssessment, AppError> {
        let mut assessments = self.assessments.lock().unwrap();
        let assessment = assessments
            .iter_mut()
            .find(|a| a.assessment_id == id)
            .ok_or_else(|| AppError::NotFound("assessment".into()))?;
        assessment.notes = notes;
        Ok(assessment.clone())
    }
    async fn delete_assessment(&self, id: u64) -> Result<u64, AppError> {
        let mut assessments = self.assessments.lock().unwrap();
        let before = assessments.len();
        assessments.retain(|a| a.assessment_id != id);
        Ok((before - assessments.len()) as u64)
    }
}

// ── Mock DiaryRepository ──

struct MockDiaryRepo {
    diaries: std::sync::Mutex<Vec<UserDiary>>,
}

impl MockDiaryRepo {
    fn new() -> Self {
        Self {
            diaries: std::sync::Mutex::new(vec![]),
        }
    }
}

#[async_trait]
impl DiaryRepository for MockDiaryRepo {
    async fn save(&self, diary: NewUserDiary) -> Result<UserDiary, AppError> {
        let mut diaries = self.diaries.lock().unwrap();
        let id = diaries.len() as u64 + 1;
        let now = Utc::now();
        let entry = UserDiary {
            id,
            user_id: diary.user_id,
            title: diary.title,
            content: diary.content,
            mood_description: None,
            created_at: now,
            updated_at: now,
        };
        diaries.push(entry.clone());
        Ok(entry)
    }
    async fn find_by_id(&self, id: u64) -> Result<Option<UserDiary>, AppError> {
        Ok(self
            .diaries
            .lock()
            .unwrap()
            .iter()
            .find(|d| d.id == id)
            .cloned())
    }
    async fn find_by_user_id(
        &self,
        user_id: u64,
        limit: u64,
        offset: u64,
    ) -> Result<(Vec<UserDiary>, u64), AppError> {
        let items = self.diaries.lock().unwrap();
        let total = items.len() as u64;
        let filtered: Vec<UserDiary> = items
            .iter()
            .filter(|d| d.user_id == user_id)
            .cloned()
            .collect();
        let page: Vec<UserDiary> = filtered
            .iter()
            .skip(offset as usize)
            .take(limit as usize)
            .cloned()
            .collect();
        Ok((page, total))
    }
    async fn update(&self, id: u64, update: UserDiaryUpdate) -> Result<UserDiary, AppError> {
        let mut diaries = self.diaries.lock().unwrap();
        let entry = diaries
            .iter_mut()
            .find(|d| d.id == id)
            .ok_or_else(|| AppError::NotFound("diary".into()))?;
        if let Some(t) = update.title {
            entry.title = t;
        }
        if let Some(c) = update.content {
            entry.content = c;
        }
        if let Some(m) = update.mood_description {
            entry.mood_description = m;
        }
        entry.updated_at = Utc::now();
        Ok(entry.clone())
    }
    async fn update_mood(&self, id: u64, mood_description: String) -> Result<(), AppError> {
        let mut diaries = self.diaries.lock().unwrap();
        if let Some(entry) = diaries.iter_mut().find(|d| d.id == id) {
            entry.mood_description = Some(mood_description);
        }
        Ok(())
    }
    async fn delete_by_id(&self, id: u64) -> Result<bool, AppError> {
        let mut diaries = self.diaries.lock().unwrap();
        let before = diaries.len();
        diaries.retain(|d| d.id != id);
        Ok(diaries.len() < before)
    }
}

// ── Mock MusicRepository ──

struct MockMusicRepo {
    tracks: std::sync::Mutex<Vec<MusicTrack>>,
}

impl MockMusicRepo {
    fn new() -> Self {
        let now = Utc::now();
        Self {
            tracks: std::sync::Mutex::new(vec![
                MusicTrack {
                    music_id: 1,
                    title: "月光".into(),
                    artist: Some("贝多芬".into()),
                    album: None,
                    category: Some("classical".into()),
                    description: Some("经典钢琴曲".into()),
                    duration: Some(300),
                    file_data: "".into(),
                    file_size: 0,
                    mime_type: "audio/mpeg".into(),
                    cover_image: None,
                    lyrics: None,
                    tags: None,
                    mood_tags: None,
                    status: 1,
                    created_at: now,
                    updated_at: now,
                },
                MusicTrack {
                    music_id: 2,
                    title: "小夜曲".into(),
                    artist: Some("舒伯特".into()),
                    album: None,
                    category: Some("classical".into()),
                    description: None,
                    duration: None,
                    file_data: "".into(),
                    file_size: 0,
                    mime_type: "audio/mpeg".into(),
                    cover_image: None,
                    lyrics: None,
                    tags: None,
                    mood_tags: None,
                    status: 1,
                    created_at: now,
                    updated_at: now,
                },
            ]),
        }
    }
}

#[async_trait]
impl MusicRepository for MockMusicRepo {
    async fn save(&self, track: NewMusicTrack) -> Result<MusicTrack, AppError> {
        let mut tracks = self.tracks.lock().unwrap();
        let id = tracks.len() as u64 + 1;
        let now = Utc::now();
        let entry = MusicTrack {
            music_id: id,
            title: track.title,
            artist: track.artist,
            album: track.album,
            category: track.category,
            description: track.description,
            duration: track.duration,
            file_data: track.file_data,
            file_size: track.file_size,
            mime_type: track.mime_type,
            cover_image: track.cover_image,
            lyrics: track.lyrics,
            tags: track.tags,
            mood_tags: track.mood_tags,
            status: 1,
            created_at: now,
            updated_at: now,
        };
        tracks.push(entry.clone());
        Ok(entry)
    }
    async fn find_by_id(&self, id: u64) -> Result<Option<MusicTrack>, AppError> {
        Ok(self
            .tracks
            .lock()
            .unwrap()
            .iter()
            .find(|t| t.music_id == id)
            .cloned())
    }
    async fn find_all(
        &self,
        category: Option<String>,
        _search: Option<String>,
        limit: u64,
        offset: u64,
    ) -> Result<(Vec<MusicTrack>, u64), AppError> {
        let tracks = self.tracks.lock().unwrap();
        let _total = tracks.len() as u64;
        let filtered: Vec<MusicTrack> = match category {
            Some(ref cat) => tracks
                .iter()
                .filter(|t| t.category.as_deref() == Some(cat.as_str()))
                .cloned()
                .collect(),
            None => tracks.clone(),
        };
        let total_filtered = filtered.len() as u64;
        let page: Vec<MusicTrack> = filtered
            .iter()
            .skip(offset as usize)
            .take(limit as usize)
            .cloned()
            .collect();
        Ok((page, total_filtered))
    }
    async fn update(&self, id: u64, update: MusicTrackUpdate) -> Result<MusicTrack, AppError> {
        let mut tracks = self.tracks.lock().unwrap();
        let track = tracks
            .iter_mut()
            .find(|t| t.music_id == id)
            .ok_or_else(|| AppError::NotFound("track".into()))?;
        if let Some(t) = update.title {
            track.title = t;
        }
        if let Some(a) = update.artist {
            track.artist = a;
        }
        if let Some(a) = update.album {
            track.album = a;
        }
        if let Some(c) = update.category {
            track.category = c;
        }
        if let Some(d) = update.duration {
            track.duration = d;
        }
        track.updated_at = Utc::now();
        Ok(track.clone())
    }
    async fn delete_by_id(&self, id: u64) -> Result<bool, AppError> {
        let mut tracks = self.tracks.lock().unwrap();
        let before = tracks.len();
        tracks.retain(|t| t.music_id != id);
        Ok(tracks.len() < before)
    }
}

// ── Mock CommunityRepository ──

struct MockCommunityRepo {
    posts: std::sync::Mutex<Vec<Post>>,
    comments: std::sync::Mutex<Vec<Comment>>,
    media: std::sync::Mutex<Vec<PostMedia>>,
}

impl MockCommunityRepo {
    fn new() -> Self {
        let now = Utc::now();
        Self {
            posts: std::sync::Mutex::new(vec![Post {
                post_id: 1,
                user_id: 1,
                title: Some("欢迎".into()),
                content: "欢迎来到社区".into(),
                extra_metadata: None,
                likes_count: 0,
                comments_count: 0,
                status: ArticleStatus::Published,
                created_at: now,
                updated_at: now,
            }]),
            comments: std::sync::Mutex::new(vec![]),
            media: std::sync::Mutex::new(vec![]),
        }
    }
}

#[async_trait]
impl CommunityRepository for MockCommunityRepo {
    async fn list_posts(&self, limit: u64, offset: u64) -> Result<Vec<Post>, AppError> {
        let posts = self.posts.lock().unwrap();
        let page: Vec<Post> = posts
            .iter()
            .skip(offset as usize)
            .take(limit as usize)
            .cloned()
            .collect();
        Ok(page)
    }
    async fn count_posts(&self) -> Result<u64, AppError> {
        Ok(self.posts.lock().unwrap().len() as u64)
    }
    async fn find_post_by_id(&self, post_id: u64) -> Result<Option<Post>, AppError> {
        Ok(self
            .posts
            .lock()
            .unwrap()
            .iter()
            .find(|p| p.post_id == post_id)
            .cloned())
    }
    async fn save_post(&self, new_post: NewPost) -> Result<Post, AppError> {
        let mut posts = self.posts.lock().unwrap();
        let id = posts.len() as u64 + 1;
        let now = Utc::now();
        let post = Post {
            post_id: id,
            user_id: new_post.user_id,
            title: new_post.title,
            content: new_post.content,
            extra_metadata: new_post.extra_metadata,
            likes_count: 0,
            comments_count: 0,
            status: new_post.status,
            created_at: now,
            updated_at: now,
        };
        posts.push(post.clone());
        Ok(post)
    }
    async fn update_post(&self, post_id: u64, update: PostUpdate) -> Result<Post, AppError> {
        let mut posts = self.posts.lock().unwrap();
        let post = posts
            .iter_mut()
            .find(|p| p.post_id == post_id)
            .ok_or_else(|| AppError::NotFound("post".into()))?;
        if let Some(t) = update.title {
            post.title = t;
        }
        if let Some(c) = update.content {
            post.content = c;
        }
        if let Some(s) = update.status {
            post.status = s;
        }
        post.updated_at = Utc::now();
        Ok(post.clone())
    }
    async fn delete_post(&self, post_id: u64) -> Result<bool, AppError> {
        let mut posts = self.posts.lock().unwrap();
        let before = posts.len();
        posts.retain(|p| p.post_id != post_id);
        Ok(posts.len() < before)
    }
    async fn incr_comments_count(&self, post_id: u64) -> Result<(), AppError> {
        let mut posts = self.posts.lock().unwrap();
        if let Some(post) = posts.iter_mut().find(|p| p.post_id == post_id) {
            post.comments_count += 1;
        }
        Ok(())
    }
    async fn decr_comments_count(&self, post_id: u64) -> Result<(), AppError> {
        let mut posts = self.posts.lock().unwrap();
        if let Some(post) = posts.iter_mut().find(|p| p.post_id == post_id) {
            post.comments_count = post.comments_count.saturating_sub(1);
        }
        Ok(())
    }
    async fn list_comments_by_post(
        &self,
        post_id: u64,
        limit: u64,
        offset: u64,
    ) -> Result<Vec<Comment>, AppError> {
        let comments = self.comments.lock().unwrap();
        let filtered: Vec<Comment> = comments
            .iter()
            .filter(|c| c.post_id == post_id)
            .cloned()
            .collect();
        let _total = filtered.len();
        let page: Vec<Comment> = filtered
            .iter()
            .skip(offset as usize)
            .take(limit as usize)
            .cloned()
            .collect();
        Ok(page)
    }
    async fn count_comments_by_post(&self, post_id: u64) -> Result<u64, AppError> {
        let comments = self.comments.lock().unwrap();
        Ok(comments.iter().filter(|c| c.post_id == post_id).count() as u64)
    }
    async fn find_comment_by_id(&self, comment_id: u64) -> Result<Option<Comment>, AppError> {
        Ok(self
            .comments
            .lock()
            .unwrap()
            .iter()
            .find(|c| c.comment_id == comment_id)
            .cloned())
    }
    async fn save_comment(&self, new_comment: NewComment) -> Result<Comment, AppError> {
        let mut comments = self.comments.lock().unwrap();
        let id = comments.len() as u64 + 1;
        let now = Utc::now();
        let comment = Comment {
            comment_id: id,
            post_id: new_comment.post_id,
            user_id: new_comment.user_id,
            parent_comment_id: new_comment.parent_comment_id,
            content: new_comment.content,
            attachments: new_comment.attachments,
            likes_count: 0,
            status: new_comment.status,
            created_at: now,
            updated_at: now,
        };
        comments.push(comment.clone());
        Ok(comment)
    }
    async fn update_comment(
        &self,
        comment_id: u64,
        content: Option<String>,
        status: Option<ArticleStatus>,
    ) -> Result<Comment, AppError> {
        let mut comments = self.comments.lock().unwrap();
        let comment = comments
            .iter_mut()
            .find(|c| c.comment_id == comment_id)
            .ok_or_else(|| AppError::NotFound("comment".into()))?;
        if let Some(c) = content {
            comment.content = c;
        }
        if let Some(s) = status {
            comment.status = s;
        }
        comment.updated_at = Utc::now();
        Ok(comment.clone())
    }
    async fn delete_comment(&self, comment_id: u64) -> Result<bool, AppError> {
        let mut comments = self.comments.lock().unwrap();
        let before = comments.len();
        comments.retain(|c| c.comment_id != comment_id);
        Ok(comments.len() < before)
    }
    async fn list_media_by_post(&self, post_id: u64) -> Result<Vec<PostMedia>, AppError> {
        let media = self.media.lock().unwrap();
        Ok(media
            .iter()
            .filter(|m| m.post_id == post_id)
            .cloned()
            .collect())
    }
    async fn save_media(&self, new_media: NewPostMedia) -> Result<PostMedia, AppError> {
        let mut media = self.media.lock().unwrap();
        let id = media.len() as u64 + 1;
        let entry = PostMedia {
            media_id: id,
            post_id: new_media.post_id,
            media_type: new_media.media_type,
            mime_type: new_media.mime_type,
            media_data: new_media.media_data,
            created_at: Utc::now(),
        };
        media.push(entry.clone());
        Ok(entry)
    }

    async fn like_post(&self, post_id: u64, _user_id: u64) -> Result<(), AppError> {
        let mut posts = self.posts.lock().unwrap();
        if let Some(post) = posts.iter_mut().find(|p| p.post_id == post_id) {
            post.likes_count += 1;
        }
        Ok(())
    }

    async fn unlike_post(&self, post_id: u64, _user_id: u64) -> Result<(), AppError> {
        let mut posts = self.posts.lock().unwrap();
        if let Some(post) = posts.iter_mut().find(|p| p.post_id == post_id) {
            post.likes_count = post.likes_count.saturating_sub(1);
        }
        Ok(())
    }

    async fn like_comment(&self, comment_id: u64, _user_id: u64) -> Result<(), AppError> {
        let mut comments = self.comments.lock().unwrap();
        if let Some(comment) = comments.iter_mut().find(|c| c.comment_id == comment_id) {
            comment.likes_count += 1;
        }
        Ok(())
    }

    async fn unlike_comment(&self, comment_id: u64, _user_id: u64) -> Result<(), AppError> {
        let mut comments = self.comments.lock().unwrap();
        if let Some(comment) = comments.iter_mut().find(|c| c.comment_id == comment_id) {
            comment.likes_count = comment.likes_count.saturating_sub(1);
        }
        Ok(())
    }
}

// ── Mock ContentLikeRepository ──

struct MockContentLikeRepo {
    likes: std::sync::Mutex<Vec<ContentLike>>,
}

impl MockContentLikeRepo {
    fn new() -> Self {
        Self {
            likes: std::sync::Mutex::new(vec![]),
        }
    }
}

#[async_trait]
impl ContentLikeRepository for MockContentLikeRepo {
    async fn toggle(
        &self,
        user_id: u64,
        content_type: &str,
        content_id: u64,
    ) -> Result<bool, AppError> {
        let mut likes = self.likes.lock().unwrap();
        if let Some(pos) = likes.iter().position(|l| {
            l.user_id == user_id && l.content_type == content_type && l.content_id == content_id
        }) {
            likes.remove(pos);
            Ok(false)
        } else {
            let id = likes.len() as u64 + 1;
            likes.push(ContentLike {
                like_id: id,
                user_id,
                content_type: content_type.to_string(),
                content_id,
                created_at: Utc::now(),
            });
            Ok(true)
        }
    }

    async fn is_liked(
        &self,
        user_id: u64,
        content_type: &str,
        content_id: u64,
    ) -> Result<bool, AppError> {
        let likes = self.likes.lock().unwrap();
        Ok(likes.iter().any(|l| {
            l.user_id == user_id && l.content_type == content_type && l.content_id == content_id
        }))
    }

    async fn count_by_content(&self, content_type: &str, content_id: u64) -> Result<u64, AppError> {
        let likes = self.likes.lock().unwrap();
        Ok(likes
            .iter()
            .filter(|l| l.content_type == content_type && l.content_id == content_id)
            .count() as u64)
    }

    async fn delete(
        &self,
        user_id: u64,
        content_type: &str,
        content_id: u64,
    ) -> Result<bool, AppError> {
        let mut likes = self.likes.lock().unwrap();
        let before = likes.len();
        likes.retain(|l| {
            !(l.user_id == user_id && l.content_type == content_type && l.content_id == content_id)
        });
        Ok(likes.len() < before)
    }
}

pub async fn test_app() -> Router {
    let state = build_test_state();
    api::router::build_router(state)
}

fn build_test_state() -> api::AppState {
    let user_repo: Arc<dyn UserRepository> = Arc::new(MockUserRepo::new());
    let profile_repo: Arc<dyn UserProfileRepository> = Arc::new(MockProfileRepo {
        profiles: std::sync::Mutex::new(vec![]),
    });
    let conv_repo: Arc<dyn ConversationRepository> = Arc::new(MockConvRepo);
    let risk_repo: Arc<dyn RiskRepository> = Arc::new(MockRiskRepo);

    let password_service: Arc<dyn PasswordService> = Arc::new(BcryptPasswordHasher::default());
    let _revoke_repo = Arc::new(MockRevokeRepo);
    let jwt: Arc<JwtTokenService> = Arc::new(JwtTokenService::new(
        "test-secret-key-for-integration-tests",
        86400,
    ));

    let (tp, tw) = new_task_channel(256);
    tokio::spawn(tw.with_handler(Arc::new(LoggingHandler)).run());
    let task_publisher: Arc<dyn TaskPublisher> = Arc::new(tp);

    let refresh_token_store: Arc<
        dyn ServerRS::domain::auth::refresh_token_store::RefreshTokenStore,
    > = Arc::new(MockRefreshTokenStore);

    let auth: Arc<AuthService> = Arc::new(AuthService::new(
        Arc::clone(&user_repo),
        Arc::clone(&password_service) as Arc<dyn PasswordService>,
        Arc::clone(&jwt) as Arc<dyn TokenService>,
        Arc::clone(&refresh_token_store),
        Arc::clone(&task_publisher),
        ServerRS::application::auth::auth_service::AuthConfig::default(),
    ));
    let user: Arc<UserService> = Arc::new(UserService::new(
        Arc::clone(&user_repo),
        Arc::clone(&profile_repo),
    ));
    let _query: Arc<SessionService> = Arc::new(SessionService::new(
        Arc::clone(&conv_repo),
        Arc::clone(&risk_repo),
    ));

    let llm: Arc<dyn LlmClient> = Arc::new(MockLlmClient);
    let prompt_provider: Arc<dyn PromptProvider> = Arc::new(MockPromptProvider);
    let orchestrator: Arc<ConversationOrchestrator> = Arc::new(ConversationOrchestrator::new(
        Arc::clone(&task_publisher),
        Arc::clone(&llm),
        Arc::clone(&prompt_provider),
        Arc::clone(&conv_repo) as Arc<dyn ConversationRepository>,
        Arc::clone(&profile_repo),
    ));

    // ── Mock repos (in-memory, no stub_repositories) ──
    // These must be constructed BEFORE SessionManager since SessionManager now needs AgentRuntime
    use ServerRS::application::agent::agent_context::AgentContextBuilder;
    use ServerRS::application::agent::agent_runtime::{AgentRuntime, AgentRuntimeSettings};
    use ServerRS::application::memory::memory_extractor::MemoryExtractor;
    use ServerRS::application::memory::memory_service::MemoryService;
    use ServerRS::application::rag::chunking::ChunkingService;
    use ServerRS::application::rag::ingestion_service::IngestionService;
    use ServerRS::application::rag::retrieval_service::RetrievalService;
    use ServerRS::domain::agent::AgentEventRepository;
    use ServerRS::domain::memory::MemoryRepository;
    use ServerRS::domain::rag::RAGRepository;
    use ServerRS::domain::summary::SummaryRepository;

    struct MockRAGRepo;
    #[async_trait::async_trait]
    impl RAGRepository for MockRAGRepo {
        async fn save_document(
            &self,
            _d: ServerRS::domain::rag::NewDocument,
        ) -> Result<ServerRS::domain::rag::KnowledgeDocument, AppError> {
            Err(AppError::Internal("mock".into()))
        }
        async fn find_document_by_source(
            &self,
            _s: &str,
            _id: Option<u64>,
        ) -> Result<Option<ServerRS::domain::rag::KnowledgeDocument>, AppError> {
            Ok(None)
        }
        async fn list_documents_by_source_type(
            &self,
            _s: &str,
        ) -> Result<Vec<ServerRS::domain::rag::KnowledgeDocument>, AppError> {
            Ok(vec![])
        }
        async fn save_chunks(
            &self,
            _c: &[ServerRS::domain::rag::NewChunk],
        ) -> Result<Vec<ServerRS::domain::rag::KnowledgeChunk>, AppError> {
            Err(AppError::Internal("mock".into()))
        }
        async fn find_chunks_by_document(
            &self,
            _id: u64,
        ) -> Result<Vec<ServerRS::domain::rag::KnowledgeChunk>, AppError> {
            Ok(vec![])
        }
        async fn save_embedding(
            &self,
            _e: ServerRS::domain::rag::NewEmbedding,
        ) -> Result<ServerRS::domain::rag::KnowledgeEmbedding, AppError> {
            Err(AppError::Internal("mock".into()))
        }
        async fn find_embedding_by_chunk(
            &self,
            _id: u64,
        ) -> Result<Option<ServerRS::domain::rag::KnowledgeEmbedding>, AppError> {
            Ok(None)
        }
        async fn search_by_keyword(
            &self,
            _q: &str,
            _k: u64,
        ) -> Result<Vec<(ServerRS::domain::rag::KnowledgeChunk, f64)>, AppError> {
            Ok(vec![])
        }
        async fn delete_document(&self, _id: u64) -> Result<(), AppError> {
            Err(AppError::Internal("mock".into()))
        }
        async fn list_chunks_with_embeddings(
            &self,
        ) -> Result<
            Vec<(
                ServerRS::domain::rag::KnowledgeChunk,
                ServerRS::domain::rag::KnowledgeEmbedding,
            )>,
            AppError,
        > {
            Ok(vec![])
        }

        async fn find_chunk_by_id(
            &self,
            _: u64,
        ) -> Result<Option<ServerRS::domain::rag::KnowledgeChunk>, AppError> {
            Ok(None)
        }

        async fn find_document_by_id(
            &self,
            _: u64,
        ) -> Result<Option<ServerRS::domain::rag::KnowledgeDocument>, AppError> {
            Ok(None)
        }

        async fn update_chunk_index_metadata(
            &self,
            _: u64,
            _: String,
            _: String,
            _: String,
            _: u32,
        ) -> Result<(), AppError> {
            Ok(())
        }
        async fn mark_chunk_unindexed(&self, _: u64) -> Result<(), AppError> {
            Ok(())
        }
        async fn list_indexable_chunks(
            &self,
            _: u64,
        ) -> Result<
            Vec<(
                ServerRS::domain::rag::KnowledgeChunk,
                ServerRS::domain::rag::KnowledgeDocument,
            )>,
            AppError,
        > {
            Ok(vec![])
        }
    }

    struct MockMemRepo;
    #[async_trait::async_trait]
    impl MemoryRepository for MockMemRepo {
        async fn save_memory(
            &self,
            _m: ServerRS::domain::memory::NewMemory,
        ) -> Result<ServerRS::domain::memory::UserMemory, AppError> {
            Err(AppError::Internal("mock".into()))
        }
        async fn find_by_id(
            &self,
            _id: u64,
        ) -> Result<Option<ServerRS::domain::memory::UserMemory>, AppError> {
            Ok(None)
        }
        async fn find_by_user_id(
            &self,
            _uid: u64,
            _s: Option<i8>,
        ) -> Result<Vec<ServerRS::domain::memory::UserMemory>, AppError> {
            Ok(vec![])
        }
        async fn search_by_user(
            &self,
            _uid: u64,
            _q: &str,
            _k: u32,
        ) -> Result<Vec<ServerRS::domain::memory::UserMemory>, AppError> {
            Ok(vec![])
        }
        async fn update_memory(
            &self,
            _id: u64,
            _c: Option<String>,
            _conf: Option<f64>,
        ) -> Result<ServerRS::domain::memory::UserMemory, AppError> {
            Err(AppError::Internal("mock".into()))
        }
        async fn disable_memory(&self, _id: u64) -> Result<(), AppError> {
            Ok(())
        }
        async fn delete_memory(&self, _id: u64) -> Result<bool, AppError> {
            Ok(true)
        }
        async fn find_memories_by_conversation(
            &self,
            _: u64,
        ) -> Result<Vec<ServerRS::domain::memory::UserMemory>, AppError> {
            Ok(vec![])
        }
        async fn update_memory_index_metadata(
            &self,
            _: u64,
            _: String,
            _: String,
            _: String,
            _: u32,
        ) -> Result<(), AppError> {
            Ok(())
        }
        async fn touch_memory_access(&self, _: u64) -> Result<(), AppError> {
            Ok(())
        }
        async fn find_by_memory_key(
            &self,
            _: u64,
            _: &str,
        ) -> Result<Option<ServerRS::domain::memory::UserMemory>, AppError> {
            Ok(None)
        }
        async fn list_indexable_memories(
            &self,
            _: Option<u64>,
            _: u64,
        ) -> Result<Vec<ServerRS::domain::memory::UserMemory>, AppError> {
            Ok(vec![])
        }
    }

    struct MockSumRepo;
    #[async_trait::async_trait]
    impl ServerRS::domain::summary::SummaryRepository for MockSumRepo {
        async fn find_latest_by_conversation(
            &self,
            _: u64,
        ) -> Result<Option<ServerRS::domain::memory::ConversationSummary>, AppError> {
            Ok(None)
        }
        async fn save_summary(
            &self,
            _: ServerRS::domain::memory::NewSummary,
        ) -> Result<ServerRS::domain::memory::ConversationSummary, AppError> {
            Err(AppError::Internal("mock".into()))
        }
        async fn find_by_id(
            &self,
            _: u64,
        ) -> Result<Option<ServerRS::domain::memory::ConversationSummary>, AppError> {
            Ok(None)
        }
        async fn disable_summary(&self, _: u64) -> Result<(), AppError> {
            Ok(())
        }
        async fn list_indexable_summaries(
            &self,
            _: u64,
        ) -> Result<Vec<ServerRS::domain::memory::ConversationSummary>, AppError> {
            Ok(vec![])
        }
        async fn update_summary_index_metadata(
            &self,
            _: u64,
            _: String,
            _: String,
            _: String,
            _: u32,
        ) -> Result<(), AppError> {
            Ok(())
        }
    }

    struct MockAgentEventRepo;
    #[async_trait::async_trait]
    impl AgentEventRepository for MockAgentEventRepo {
        async fn log_event(
            &self,
            _e: ServerRS::domain::agent::NewAgentEvent,
        ) -> ServerRS::domain::agent::AgentEvent {
            ServerRS::domain::agent::AgentEvent {
                event_id: 0,
                user_id: 0,
                conversation_id: None,
                session_id: None,
                trace_id: None,
                event_type: String::new(),
                tool_name: None,
                payload: serde_json::Value::Null,
                created_at: Utc::now(),
            }
        }
    }

    let rag_repo: Arc<dyn RAGRepository> = Arc::new(MockRAGRepo);
    let memory_repo: Arc<dyn MemoryRepository> = Arc::new(MockMemRepo);
    let summary_repo: Arc<dyn SummaryRepository> = Arc::new(MockSumRepo);
    let agent_event_repo: Arc<dyn AgentEventRepository> = Arc::new(MockAgentEventRepo);

    let retrieval: Arc<RetrievalService> =
        Arc::new(RetrievalService::new(Arc::clone(&rag_repo), None));
    let chunking = ChunkingService::new();
    let ingestion: Arc<IngestionService> =
        Arc::new(IngestionService::new(Arc::clone(&rag_repo), chunking, None));

    let memory_extractor: Arc<MemoryExtractor> =
        Arc::new(MemoryExtractor::new(Arc::new(MockLlmProvider::new("[]"))));
    let memory_svc: Arc<MemoryService> = Arc::new(MemoryService::new(
        Arc::clone(&memory_repo),
        memory_extractor,
    ));

    let summary_service: Arc<ServerRS::application::summary::summary_service::SummaryService> =
        Arc::new(
            ServerRS::application::summary::summary_service::SummaryService::new(
                Arc::clone(&summary_repo),
                None,
            ),
        );
    let context_builder: Arc<AgentContextBuilder> = Arc::new(AgentContextBuilder::new(
        Arc::clone(&memory_svc),
        Arc::clone(&retrieval),
        Arc::clone(&summary_service),
        Arc::clone(&conv_repo),
        Arc::clone(&profile_repo),
    ));

    use ServerRS::infrastructure::detector::rule_based_detector::RuleBasedRiskDetector;
    use ServerRS::infrastructure::llm::mock_provider::MockLlmProvider;
    let agent_llm: Arc<dyn ServerRS::domain::llm::LlmProvider> =
        Arc::new(MockLlmProvider::new("mock reply"));
    let agent_risk_detector: Arc<dyn RiskDetector> = Arc::new(RuleBasedRiskDetector::new());

    let agent_risk_detection_service = Arc::new(RiskDetectionService::new(
        Arc::clone(&risk_repo),
        Arc::clone(&task_publisher),
        Arc::clone(&agent_risk_detector),
    ));

    let agent_runtime: Arc<AgentRuntime> = Arc::new(AgentRuntime::new(
        Arc::clone(&agent_llm),
        // Arc::clone(&rag_repo),
        Arc::clone(&memory_svc),
        Arc::clone(&agent_risk_detection_service),
        Arc::clone(&agent_event_repo),
        Arc::clone(&conv_repo),
        Arc::clone(&profile_repo),
        context_builder,
        Arc::clone(&summary_service),
        Vec::new(),
        AgentRuntimeSettings::default(),
    ));

    let session: Arc<SessionManager> = Arc::new(SessionManager::new(
        Arc::clone(&task_publisher),
        Arc::clone(&orchestrator),
        Arc::clone(&agent_runtime),
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

    let psychology: Arc<PsychologyService> =
        Arc::new(PsychologyService::new(Arc::new(MockPsychologyRepo::new())));
    let depression: Arc<DepressionService> =
        Arc::new(DepressionService::new(Arc::new(MockDepressionRepo::new())));
    let diaries: Arc<DiaryService> =
        Arc::new(DiaryService::new(Arc::new(MockDiaryRepo::new()), None));
    let music: Arc<MusicService> = Arc::new(MusicService::new(Arc::new(MockMusicRepo::new())));
    let community: Arc<CommunityService> =
        Arc::new(CommunityService::new(Arc::new(MockCommunityRepo::new())));

    let local_storage: Arc<dyn ServerRS::domain::storage::ObjectStorage> = Arc::new(
        LocalObjectStorage::new(std::path::PathBuf::from("data/test-objects")),
    );
    let stored_object_repo: Arc<dyn StoredObjectRepository> = Arc::new(MockStoredObjectRepo::new());
    let objects: Arc<ObjectService> = Arc::new(ObjectService::new(
        Arc::clone(&local_storage),
        Arc::clone(&stored_object_repo),
        Default::default(),
    ));

    api::AppState {
        auth: api::AuthState {
            auth: Arc::clone(&auth),
        },
        user: api::UserState {
            user: Arc::clone(&user),
        },
        session: api::SessionState {
            session: Arc::clone(&session),
            query: Arc::clone(&_query),
        },
        object: api::ObjectState {
            objects: Arc::clone(&objects),
        },
        psychology: api::PsychologyState {
            psychology: Arc::clone(&psychology),
        },
        depression: api::DepressionState {
            depression: Arc::clone(&depression),
        },
        diary: api::DiaryState {
            diaries: Arc::clone(&diaries),
        },
        music: api::MusicState {
            music: Arc::clone(&music),
        },
        community: api::CommunityState {
            community: Arc::clone(&community),
        },
        admin: api::AdminState {
            user: Arc::clone(&user),
            query: Arc::clone(&_query),
        },
        internal: api::InternalState {
            retrieval: Arc::clone(&retrieval),
            ingestion: Arc::clone(&ingestion),
            memory: Arc::clone(&memory_svc),
            agent_runtime: Arc::clone(&agent_runtime),
        },
    }
}

// ── HTTP helpers (oneshot-based) ──

pub async fn read_body(response: axum::response::Response) -> Value {
    let status = response.status();
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    if body.is_empty() {
        return serde_json::json!({ "__status": status.as_u16() });
    }
    serde_json::from_slice(&body).unwrap_or_else(|_| {
        serde_json::json!({ "__status": status.as_u16(), "__body": String::from_utf8_lossy(&body).to_string() })
    })
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

pub async fn patch_auth(app: &Router, path: &str, body: &Value, token: &str) -> Value {
    let req = Request::builder()
        .method("PATCH")
        .uri(path)
        .header("Content-Type", "application/json")
        .header("Authorization", format!("Bearer {token}"))
        .body(Body::from(body.to_string()))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    read_body(resp).await
}

pub async fn get(app: &Router, path: &str) -> Value {
    let req = Request::get(path).body(Body::empty()).unwrap();
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

pub async fn post_auth(app: &Router, path: &str, body: &Value, token: &str) -> Value {
    let req = Request::post(path)
        .header("Content-Type", "application/json")
        .header("Authorization", format!("Bearer {token}"))
        .body(Body::from(body.to_string()))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    read_body(resp).await
}
