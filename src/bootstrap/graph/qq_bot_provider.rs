use crate::bootstrap::tasks::BackgroundTasks;

use super::BootstrapContext;

#[cfg(feature = "qq_bot")]
pub async fn init_qq_bot_integration(ctx: &BootstrapContext<'_>, background: &mut BackgroundTasks) {
    use std::sync::Arc;

    use crate::bootstrap::qq_bot::init_qq_bot;
    use crate::domain::tts::TtsProvider;
    use crate::infra::qq_bot::repo::seaorm_impl::agent_turn::AgentTurnRepo;
    use crate::infra::qq_bot::repo::seaorm_impl::bot_account::BotAccountRepo;
    use crate::infra::qq_bot::repo::seaorm_impl::external_user::ExternalUserRepo;
    use crate::infra::qq_bot::repo::seaorm_impl::group::GroupRepo;
    use crate::infra::qq_bot::repo::seaorm_impl::group_member::GroupMemberRepo;
    use crate::infra::qq_bot::repo::seaorm_impl::group_memory::GroupMemoryRepo;
    use crate::infra::qq_bot::repo::seaorm_impl::group_message::GroupMessageRepo;
    use crate::infra::qq_bot::repo::seaorm_impl::group_summary::GroupSummaryRepo;
    use crate::infra::qq_bot::repo::seaorm_impl::outbox::OutboxRepo;
    use crate::infra::qq_bot::repo::seaorm_impl::relationship::RelationshipRepo;
    use crate::infra::qq_bot::repo::seaorm_impl::user_profile::QqUserProfileRepo;
    use crate::infra::tts::volcengine_provider::VolcengineTtsProvider;

    let qq_bot_bot_account_repo = Arc::new(BotAccountRepo::new(ctx.infra.db.clone()))
        as Arc<dyn crate::domain::qq_bot::repository::BotAccountRepoT>;
    let qq_bot_group_repo = Arc::new(GroupRepo::new(ctx.infra.db.clone()))
        as Arc<dyn crate::domain::qq_bot::repository::GroupRepoT>;
    let qq_bot_group_member_repo = Arc::new(GroupMemberRepo::new(ctx.infra.db.clone()))
        as Arc<dyn crate::domain::qq_bot::repository::GroupMemberRepoT>;
    let qq_bot_group_message_repo = Arc::new(GroupMessageRepo::new(ctx.infra.db.clone()))
        as Arc<dyn crate::domain::qq_bot::repository::GroupMessageRepoT>;
    let qq_bot_group_summary_repo = Arc::new(GroupSummaryRepo::new(ctx.infra.db.clone()))
        as Arc<dyn crate::domain::qq_bot::repository::GroupSummaryRepoT>;
    let qq_bot_group_memory_repo = Arc::new(GroupMemoryRepo::new(ctx.infra.db.clone()))
        as Arc<dyn crate::domain::qq_bot::repository::GroupMemoryRepoT>;
    let qq_bot_agent_turn_repo = Arc::new(AgentTurnRepo::new(ctx.infra.db.clone()))
        as Arc<dyn crate::domain::qq_bot::repository::AgentTurnRepoT>;
    let qq_bot_outbox_repo = Arc::new(OutboxRepo::new(ctx.infra.db.clone()))
        as Arc<dyn crate::domain::qq_bot::repository::OutboxRepoT>;
    let qq_bot_external_user_repo = Arc::new(ExternalUserRepo::new(ctx.infra.db.clone()))
        as Arc<dyn crate::domain::qq_bot::repository::ExternalUserRepoT>;
    let qq_bot_user_profile_repo = Arc::new(QqUserProfileRepo::new(ctx.infra.db.clone()))
        as Arc<dyn crate::domain::qq_bot::qq_profile_repo::QqUserProfileRepoT>;
    let qq_bot_relationship_repo = Arc::new(RelationshipRepo::new(ctx.infra.db.clone()))
        as Arc<dyn crate::domain::qq_bot::relationship_repo::RelationshipRepoT>;

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
            as Arc<dyn crate::domain::user::user_repo::UserRepoT>),
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
