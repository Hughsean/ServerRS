//! 启动 QQ 机器人（赛博猫猫）子系统。
//!
//! 职责：
//! - 组装所有 QQ 机器人服务的依赖
//! - 主开关（qq_bot.enabled）
//! - 注意力存储初始化
//! - LLM Provider 接线（可选覆盖配置）
//! - 通过 BackgroundTasks 启动发件箱 Worker
//! - NapCat WebSocket 监听器启动（消息 + 通知事件）

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
    // 仓库（从 bootstrap 传入 mock 或真实实现）
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

    // ── 主开关 ──────────────────────────────────────────────
    if !qc.enabled {
        info!("qq_bot 模块已禁用");
        return Ok(None);
    }

    // ── 机器人人设 ────────────────────────────────────────────────
    let persona = BotPersona::default();
    info!(
        nickname = %persona.nickname,
        "QQ Bot 人设已加载"
    );

    // ── 注意力存储 ────────────────────────────────────────────
    let attention_store = Arc::new(InMemoryAttentionStore::new(
        qc.cooldown_secs,
        qc.idle_timeout_secs,
    ));

    // ── NapCat API 客户端 ──────────────────────────────────────────
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

    // ── LLM Provider（可选覆盖配置）───────────────
    let trigger_llm = Arc::clone(&llm_provider);
    let reply_llm = Arc::clone(&llm_provider);
    let profile_llm = Arc::clone(&llm_provider);

    // ── 画像构建器（可选）────────────────────────────────
    // 在可能移动前克隆 — 通知处理器后续需要它们
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

            // 启动定期清理任务
            let cleanup_interval =
                tokio::time::Duration::from_secs(qc.profile_cleanup_interval_secs);
            let pb_cleanup = Arc::clone(&pb);
            background.spawn(tokio::spawn(async move {
                loop {
                    tokio::time::sleep(cleanup_interval).await;
                    if let Err(e) = pb_cleanup.cleanup().await {
                        tracing::error!(error = %e, "画像清理失败");
                    }
                }
            }));
            info!("QQ Bot 画像构建器已启用");
            Some(pb)
        } else {
            tracing::warn!("qq_bot profile_enabled=true 但缺少用户/外部用户/画像仓库");
            None
        }
    } else {
        None
    };

    // ── 领域服务 ────────────────────────────────────────────
    let emotional_service = Arc::new(EmotionalStateService::new());

    // Topic service (纯内存，无外部依赖)
    let topic_service = Arc::new(TopicService::new());

    // Relationship service (可选)
    let relationship_service =
        relationship_repo.map(|repo| Arc::new(RelationshipService::new(repo)));

    let ingestion = Arc::new(MessageIngestionService::new(Arc::clone(
        &group_message_repo,
    )));

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
        4,  // max_segments
        80, // max_chars_per_segment
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

    // ── 初始化机器人账号缓存 ───────────────────────────────
    if qc.self_qq_id != 0 {
        if let Err(e) = service.init(qc.self_qq_id).await {
            tracing::warn!(error = %e, "qq_bot: 初始化机器人账号失败，继续运行");
        }
    }

    // ── 启动发件箱 Worker（后台）──────────────────────────
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
        "qq_bot 发件箱 Worker 已启动"
    );

    // ── 启动 NapCat WebSocket 监听器（正向 WS，自动重连）──
    // 优先使用显式配置的 ws_url，否则从 http_base_url 推导
    let ws_url = qc
        .ws_url
        .clone()
        .unwrap_or_else(|| qc.http_base_url.replace("http://", "ws://"));
    info!(
        ws_url = %ws_url,
        "正在启动 NapCat 正向 WebSocket 监听器"
    );

    // 创建通知处理器的 bot_account_id
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

    // 创建通知处理器（需要 external_user_repo）
    let notice_handler: Option<
        Arc<dyn crate::infra::qq_bot::napcat::listener::GroupNoticeHandler>,
    > = if let Some(ref external_user_repo) = external_user_repo {
        Some(Arc::new(NapCatGroupNoticeHandler::new(
            Arc::clone(&group_member_repo),
            Arc::clone(external_user_repo),
            napcat_api.clone(),
            bot_account_id,
        )))
    } else {
        tracing::warn!("external_user_repo 不可用 — 群通知事件将不会同步");
        None
    };

    // 构建监听器
    let listener_handler: Arc<dyn GroupMessageHandler> =
        Arc::clone(&service) as Arc<dyn GroupMessageHandler>;
    let mut listener_builder = NapCatListener::new(ws_url, qc.self_qq_id, listener_handler);
    if let Some(ref nh) = notice_handler {
        listener_builder = listener_builder.with_notice_handler(Arc::clone(nh));
    }

    // 在后台循环运行，断开时自动重连（退避间隔逐步增加）
    background.spawn(tokio::spawn(async move {
        let mut backoff_secs = 1u64;
        loop {
            if let Err(e) = listener_builder.run_forward().await {
                tracing::error!(error = %e, "NapCat 监听器运行出错");
            } else {
                // run_forward 正常返回（WS 被远端关闭）
                tracing::warn!("NapCat WebSocket 连接已关闭，准备重连");
            }

            // 退避等待后重试，最长不超过 60 秒
            let wait_secs = std::cmp::min(backoff_secs, 60);
            tracing::info!(wait_secs = %wait_secs, "NapCat WebSocket 将在 {} 秒后重连", wait_secs);
            tokio::time::sleep(tokio::time::Duration::from_secs(wait_secs)).await;
            backoff_secs = std::cmp::min(backoff_secs * 2, 120);
        }
    }));

    // ── 主动评估器（后台轮询）────────────────────────────
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
            "qq_bot 主动评估器已启动"
        );
    }

    info!("QQ Bot（赛博猫猫）模块初始化完成");

    Ok(Some(QqBotDependencies {
        service,
        attention_store,
        napcat_api,
    }))
}
