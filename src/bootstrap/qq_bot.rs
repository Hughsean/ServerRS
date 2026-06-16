//! Bootstrap the QQ Bot (赛博猫猫) subsystem.
//!
//! Responsibilities:
//! - Dependency assembly for all QQ Bot services
//! - Master switch gate (qq_bot.enabled)
//! - Attention store initialisation
//! - LLM provider wiring with optional override config
//! - Outbox worker spawning via BackgroundTasks
//! - NapCat WebSocket listener startup for message + notice events

use std::path::PathBuf;
use std::sync::Arc;

use tracing::info;

use crate::app::qq_bot::context_builder::ContextBuilder;
use crate::app::qq_bot::emotional_state_service::EmotionalStateService;
use crate::app::qq_bot::message_ingestion::MessageIngestionService;
use crate::app::qq_bot::outbox_worker::OutboxWorker;
use crate::app::qq_bot::proactive_evaluator::ProactiveEvaluator;
use crate::app::qq_bot::profile_builder::{ProfileBuilder, ProfileBuilderConfig};
use crate::app::qq_bot::qq_bot_service::QqBotService;
use crate::app::qq_bot::relationship_service::RelationshipService;
use crate::app::qq_bot::reply_generator::ReplyGenerator;
use crate::app::qq_bot::segment_dispatcher::SegmentDispatcher;
use crate::app::qq_bot::topic_service::TopicService;
use crate::app::qq_bot::trigger_evaluator::TriggerEvaluator;
use crate::bootstrap::tasks::BackgroundTasks;
use crate::domain::llm::LlmProvider;
use crate::domain::qq_bot::persona::BotPersona;
use crate::domain::qq_bot::qq_profile_repository::QqUserProfileRepository;
use crate::domain::qq_bot::relationship_repository::RelationshipRepository;
use crate::domain::qq_bot::repository::{
    AgentTurnRepository, BotAccountRepository, ExternalUserRepository, GroupMemberRepository,
    GroupMemoryRepository, GroupMessageRepository, GroupRepository, GroupSummaryRepository,
    OutboxRepository,
};
use crate::domain::tts::TtsProvider;
use crate::domain::user::user_repository::UserRepository;
use crate::infra::qq_bot::attention_store::InMemoryAttentionStore;
use crate::infra::qq_bot::napcat::api::NapCatApiClient;
use crate::infra::qq_bot::napcat::listener::{GroupMessageHandler, NapCatListener};
use crate::infra::qq_bot::napcat::notice_handler::NapCatGroupNoticeHandler;
use crate::shared::config::AppConfig;
use crate::shared::error::AppError;

/// Dependency container for the QQ Bot subsystem.
pub struct QqBotDependencies {
    pub service: Arc<QqBotService>,
    pub attention_store: Arc<InMemoryAttentionStore>,
    pub napcat_api: Option<Arc<NapCatApiClient>>,
}

/// Initialise the QQ Bot subsystem.
///
/// Returns `None` if the module is disabled (`qq_bot.enabled = false`).
pub async fn init_qq_bot(
    config: &AppConfig,
    llm_provider: Arc<dyn LlmProvider>,
    tts_provider: Option<Arc<dyn TtsProvider>>,
    background: &mut BackgroundTasks,
    // Repositories (pass in mock or real impls from bootstrap)
    bot_account_repo: Arc<dyn BotAccountRepository>,
    group_repo: Arc<dyn GroupRepository>,
    group_member_repo: Arc<dyn GroupMemberRepository>,
    group_message_repo: Arc<dyn GroupMessageRepository>,
    group_summary_repo: Arc<dyn GroupSummaryRepository>,
    group_memory_repo: Arc<dyn GroupMemoryRepository>,
    agent_turn_repo: Arc<dyn AgentTurnRepository>,
    outbox_repo: Arc<dyn OutboxRepository>,
    // Profile & user repos (optional for profile building)
    user_repo: Option<Arc<dyn UserRepository>>,
    external_user_repo: Option<Arc<dyn ExternalUserRepository>>,
    user_profile_repo: Option<Arc<dyn QqUserProfileRepository>>,
    // Relationship repo (optional)
    relationship_repo: Option<Arc<dyn RelationshipRepository>>,
) -> Result<Option<QqBotDependencies>, AppError> {
    let qc = &config.qq_bot;

    // ── Master switch ──────────────────────────────────────────────
    if !qc.enabled {
        info!("qq_bot module is disabled");
        return Ok(None);
    }

    // ── Bot persona ────────────────────────────────────────────────
    let persona = BotPersona::default();
    info!(
        nickname = %persona.nickname,
        "QQ Bot persona loaded"
    );

    // ── Attention store ────────────────────────────────────────────
    let attention_store = Arc::new(InMemoryAttentionStore::new(
        qc.cooldown_secs,
        qc.idle_timeout_secs,
    ));

    // ── NapCat API client ──────────────────────────────────────────
    let napcat_api = if qc.self_qq_id != 0 {
        let token = if qc.http_token.is_empty() {
            None
        } else {
            Some(qc.http_token.clone())
        };
        Some(Arc::new(NapCatApiClient::new(
            qc.http_base_url.clone(),
            token,
        )))
    } else {
        None
    };

    // ── LLM provider (with optional override config) ───────────────
    let trigger_llm = Arc::clone(&llm_provider);
    let reply_llm = Arc::clone(&llm_provider);
    let profile_llm = Arc::clone(&llm_provider);

    // ── Profile builder (optional) ─────────────────────────────────
    // Clone before potential move — notice handler needs them later
    let pb_user_repo = user_repo.clone();
    let pb_external_user_repo = external_user_repo.clone();
    let pb_user_profile_repo = user_profile_repo.clone();

    let profile_builder = if qc.profile_enabled {
        if let (Some(user_repo), Some(external_user_repo), Some(user_profile_repo)) =
            (pb_user_repo, pb_external_user_repo, pb_user_profile_repo)
        {
            let pb = Arc::new(ProfileBuilder::new(
                user_repo,
                external_user_repo,
                user_profile_repo,
                Arc::clone(&group_memory_repo),
                Arc::clone(&group_message_repo),
                profile_llm,
                ProfileBuilderConfig {
                    user_profile_threshold: qc.user_profile_threshold,
                    group_profile_threshold: qc.group_profile_threshold,
                },
            ));

            // Spawn periodic cleanup task
            let cleanup_interval = tokio::time::Duration::from_secs(qc.profile_cleanup_interval_secs);
            let pb_cleanup = Arc::clone(&pb);
            background.spawn(tokio::spawn(async move {
                loop {
                    tokio::time::sleep(cleanup_interval).await;
                    if let Err(e) = pb_cleanup.cleanup().await {
                        tracing::error!(error = %e, "profile cleanup failed");
                    }
                }
            }));
            info!("QQ Bot profile builder enabled");
            Some(pb)
        } else {
            tracing::warn!("qq_bot profile_enabled=true but missing user/external_user/profile repos");
            None
        }
    } else {
        None
    };

    // ── Domain services ────────────────────────────────────────────
    let emotional_service = Arc::new(EmotionalStateService::new());

    // Topic service (纯内存，无外部依赖)
    let topic_service = Arc::new(TopicService::new());

    // Relationship service (可选)
    let relationship_service = relationship_repo.map(|repo| {
        Arc::new(RelationshipService::new(repo))
    });

    let ingestion = Arc::new(MessageIngestionService::new(
        Arc::clone(&group_message_repo),
    ));

    let trigger = Arc::new(TriggerEvaluator::new(
        trigger_llm,
        Arc::clone(&attention_store),
        persona.clone(),
        Some(Arc::clone(&topic_service)),
        Some(Arc::clone(&emotional_service)),
    ));

    let context_builder = Arc::new(ContextBuilder::new(
        Arc::clone(&group_message_repo),
        Arc::clone(&group_member_repo),
        Arc::clone(&group_summary_repo),
        Arc::clone(&group_memory_repo),
        persona.clone(),
        20, // max_recent_messages
        Some(Arc::clone(&emotional_service)),
        Some(Arc::clone(&topic_service)),
        relationship_service.clone(),
    ));

    let reply_generator = Arc::new(ReplyGenerator::new(
        reply_llm,
        persona.clone(),
        4,   // max_segments
        80,  // max_chars_per_segment
        qc.inter_segment_delay_ms,
        qc.initial_delay_ms,
    ));

    let segment_dispatcher = Arc::new(SegmentDispatcher::new(
        napcat_api.clone(),
        Arc::clone(&outbox_repo),
        0, // bot_account_id — set after account init
        tts_provider,
        PathBuf::from(&qc.tts_output_dir),
        qc.tts_public_url_base.clone(),
    ));

    let service = Arc::new(QqBotService::new(
        ingestion,
        trigger,
        context_builder.clone(),
        reply_generator.clone(),
        segment_dispatcher.clone(),
        profile_builder,
        Arc::clone(&bot_account_repo),
        Arc::clone(&group_repo),
        Arc::clone(&agent_turn_repo),
        Arc::clone(&group_message_repo),
        Arc::clone(&attention_store),
        persona,
        emotional_service.clone(),
        topic_service.clone(),
        relationship_service,
    ));

    // ── Initialise bot account cache ───────────────────────────────
    if qc.self_qq_id != 0 {
        if let Err(e) = service.init(qc.self_qq_id).await {
            tracing::warn!(error = %e, "qq_bot: failed to init bot account, continuing anyway");
        }
    }

    // ── Spawn outbox worker (background) ───────────────────────────
    let outbox_worker = OutboxWorker::new(
        Arc::clone(&outbox_repo),
        napcat_api.clone(),
        qc.outbox_poll_interval_secs,
        qc.outbox_batch_size,
    );
    background.spawn(tokio::spawn(async move {
        Arc::new(outbox_worker).run().await;
    }));
    info!(
        poll_interval_secs = qc.outbox_poll_interval_secs,
        batch_size = qc.outbox_batch_size,
        "qq_bot outbox worker started"
    );

    // ── Spawn NapCat WebSocket listener (forward WS) ───────────────
    // Derive WS URL from http_base_url by replacing the scheme.
    // NapCat typically exposes WebSocket on the same host:port as HTTP.
    let ws_url = qc.http_base_url.replace("http://", "ws://");
    info!(
        ws_url = %ws_url,
        "starting NapCat forward WebSocket listener"
    );

    // Get the bot_account_id for notice handler
    let bot_account_id = if qc.self_qq_id != 0 {
        bot_account_repo
            .find_by_self_qq_id(qc.self_qq_id)
            .await
            .map_err(|e| AppError::Internal(format!("failed to find bot account: {e}")))?
            .map(|a| a.bot_account_id)
            .unwrap_or(0)
    } else {
        0
    };

    // Create notice handler (requires external_user_repo)
    let notice_handler: Option<Arc<dyn crate::infra::qq_bot::napcat::listener::GroupNoticeHandler>> =
        if let Some(ref external_user_repo) = external_user_repo {
            Some(Arc::new(NapCatGroupNoticeHandler::new(
                Arc::clone(&group_member_repo),
                Arc::clone(external_user_repo),
                napcat_api.clone(),
                bot_account_id,
            )))
        } else {
            tracing::warn!("external_user_repo not available — group notice events will not be synced");
            None
        };

    // Build and start the listener in a background task
    let listener_handler: Arc<dyn GroupMessageHandler> = Arc::clone(&service) as Arc<dyn GroupMessageHandler>;
    let mut listener_builder = NapCatListener::new(ws_url, qc.self_qq_id, listener_handler);
    if let Some(ref nh) = notice_handler {
        listener_builder = listener_builder.with_notice_handler(Arc::clone(nh));
    }

    background.spawn(tokio::spawn(async move {
        if let Err(e) = listener_builder.run_forward().await {
            tracing::error!(error = %e, "NapCat listener stopped with error");
        }
    }));

    // ── Proactive evaluator (后台轮询) ────────────────────────────
    if qc.proactive_check_interval_secs > 0 {
        let evaluator = Arc::new(ProactiveEvaluator::new(
            Arc::clone(&group_repo),
            Arc::clone(&group_message_repo),
            context_builder,
            reply_generator,
            segment_dispatcher,
            Arc::clone(&attention_store),
            topic_service,
            emotional_service,
            llm_provider,
            qc.self_qq_id,
            bot_account_id,
            std::time::Duration::from_secs(qc.proactive_check_interval_secs),
            std::time::Duration::from_secs(qc.proactive_cooldown_secs),
        ));

        background.spawn(tokio::spawn(async move {
            evaluator.run().await;
        }));
        info!(
            interval_secs = qc.proactive_check_interval_secs,
            cooldown_secs = qc.proactive_cooldown_secs,
            "qq_bot proactive evaluator started"
        );
    }

    info!("QQ Bot (赛博猫猫) module initialised");

    Ok(Some(QqBotDependencies {
        service,
        attention_store,
        napcat_api,
    }))
}
