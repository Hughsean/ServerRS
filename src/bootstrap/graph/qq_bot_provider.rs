use crate::bootstrap::tasks::BackgroundTasks;

use super::BootstrapContext;

#[cfg(feature = "qq_bot")]
pub async fn init_qq_bot_integration(ctx: &BootstrapContext<'_>, background: &mut BackgroundTasks) {
    use std::sync::Arc;

    use crate::bootstrap::qq_bot::init_qq_bot;
    use crate::domain::tts::TtsProvider;
    use crate::infra::qq_bot::repositories::seaorm_agent_turn_repository::SeaOrmAgentTurnRepository;
    use crate::infra::qq_bot::repositories::seaorm_bot_account_repository::SeaOrmBotAccountRepository;
    use crate::infra::qq_bot::repositories::seaorm_external_user_repository::SeaOrmExternalUserRepository;
    use crate::infra::qq_bot::repositories::seaorm_group_member_repository::SeaOrmGroupMemberRepository;
    use crate::infra::qq_bot::repositories::seaorm_group_memory_repository::SeaOrmGroupMemoryRepository;
    use crate::infra::qq_bot::repositories::seaorm_group_message_repository::SeaOrmGroupMessageRepository;
    use crate::infra::qq_bot::repositories::seaorm_group_repository::SeaOrmGroupRepository;
    use crate::infra::qq_bot::repositories::seaorm_group_summary_repository::SeaOrmGroupSummaryRepository;
    use crate::infra::qq_bot::repositories::seaorm_outbox_repository::SeaOrmOutboxRepository;
    use crate::infra::qq_bot::repositories::seaorm_relationship_repository::SeaOrmRelationshipRepository;
    use crate::infra::qq_bot::repositories::seaorm_user_profile_repository::SeaOrmQqUserProfileRepository;
    use crate::infra::tts::volcengine_provider::VolcengineTtsProvider;

    let qq_bot_bot_account_repo = Arc::new(SeaOrmBotAccountRepository::new(ctx.infra.db.clone()))
        as Arc<dyn crate::domain::qq_bot::repository::BotAccountRepository>;
    let qq_bot_group_repo = Arc::new(SeaOrmGroupRepository::new(ctx.infra.db.clone()))
        as Arc<dyn crate::domain::qq_bot::repository::GroupRepository>;
    let qq_bot_group_member_repo = Arc::new(SeaOrmGroupMemberRepository::new(ctx.infra.db.clone()))
        as Arc<dyn crate::domain::qq_bot::repository::GroupMemberRepository>;
    let qq_bot_group_message_repo =
        Arc::new(SeaOrmGroupMessageRepository::new(ctx.infra.db.clone()))
            as Arc<dyn crate::domain::qq_bot::repository::GroupMessageRepository>;
    let qq_bot_group_summary_repo =
        Arc::new(SeaOrmGroupSummaryRepository::new(ctx.infra.db.clone()))
            as Arc<dyn crate::domain::qq_bot::repository::GroupSummaryRepository>;
    let qq_bot_group_memory_repo = Arc::new(SeaOrmGroupMemoryRepository::new(ctx.infra.db.clone()))
        as Arc<dyn crate::domain::qq_bot::repository::GroupMemoryRepository>;
    let qq_bot_agent_turn_repo = Arc::new(SeaOrmAgentTurnRepository::new(ctx.infra.db.clone()))
        as Arc<dyn crate::domain::qq_bot::repository::AgentTurnRepository>;
    let qq_bot_outbox_repo = Arc::new(SeaOrmOutboxRepository::new(ctx.infra.db.clone()))
        as Arc<dyn crate::domain::qq_bot::repository::OutboxRepository>;
    let qq_bot_external_user_repo =
        Arc::new(SeaOrmExternalUserRepository::new(ctx.infra.db.clone()))
            as Arc<dyn crate::domain::qq_bot::repository::ExternalUserRepository>;
    let qq_bot_user_profile_repo =
        Arc::new(SeaOrmQqUserProfileRepository::new(ctx.infra.db.clone()))
            as Arc<dyn crate::domain::qq_bot::qq_profile_repository::QqUserProfileRepository>;
    let qq_bot_relationship_repo = Arc::new(SeaOrmRelationshipRepository::new(ctx.infra.db.clone()))
        as Arc<dyn crate::domain::qq_bot::relationship_repository::RelationshipRepository>;

    let qq_bot_tts_provider: Option<Arc<dyn TtsProvider>> = if ctx.config.qq_bot.enabled
        && ctx.config.qq_bot.self_qq_id != 0
        && !ctx.config.tts.api_key.is_empty()
    {
        tracing::info!("正在为 QQ 机器人语音消息初始化 VolcengineTtsProvider");
        Some(Arc::new(VolcengineTtsProvider::new(&ctx.config.tts)) as Arc<dyn TtsProvider>)
    } else {
        if ctx.config.qq_bot.enabled {
            tracing::warn!("未配置 TTS API 密钥 — 语音消息将不可用");
        }
        None
    };

    let _qq_bot_deps = init_qq_bot(
        ctx.config,
        Arc::clone(&ctx.infra.ollama_provider),
        qq_bot_tts_provider,
        background,
        qq_bot_bot_account_repo,
        qq_bot_group_repo,
        qq_bot_group_member_repo,
        qq_bot_group_message_repo,
        qq_bot_group_summary_repo,
        qq_bot_group_memory_repo,
        qq_bot_agent_turn_repo,
        qq_bot_outbox_repo,
        Some(Arc::clone(&ctx.repos.user_repo)
            as Arc<dyn crate::domain::user::user_repository::UserRepoT>),
        Some(Arc::clone(&qq_bot_external_user_repo)),
        Some(Arc::clone(&qq_bot_user_profile_repo)),
        Some(Arc::clone(&qq_bot_relationship_repo)),
    )
    .await
    .unwrap_or_else(|error| {
        tracing::warn!(error = %error, "qq_bot 初始化失败 — 将继续运行而不启动它");
        None
    });
}

#[cfg(not(feature = "qq_bot"))]
pub async fn init_qq_bot_integration(
    _ctx: &BootstrapContext<'_>,
    _background: &mut BackgroundTasks,
) {
}
