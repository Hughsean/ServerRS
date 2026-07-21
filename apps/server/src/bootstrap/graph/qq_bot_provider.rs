use crate::bootstrap::tasks::BackgroundTasks;

use super::BootstrapContext;

#[cfg(feature = "qq_bot")]
pub async fn init_qq_bot_integration(ctx: &BootstrapContext<'_>, background: &mut BackgroundTasks) {
    use std::sync::Arc;

    use crate::bootstrap::qq_bot::init_qq_bot;
    use crate::domain::tts::TtsProvider;
    use crate::infra::tts::volcengine_provider::VolcengineTtsProvider;

    let repos = qqbot::repositories::build_repositories(&ctx.infra.db);

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

    let _qq_bot_deps =
        init_qq_bot(
            ctx.config,
            Arc::clone(&ctx.infra.ollama_provider),
            qq_bot_tts_provider,
            background,
            repos.bot_account,
            repos.group,
            repos.group_member,
            repos.group_message,
            repos.group_summary,
            repos.group_memory,
            repos.agent_turn,
            repos.outbox,
            Some(Arc::clone(&ctx.repos.user_repo)
                as Arc<dyn crate::domain::user::user_repo::UserRepoT>),
            Some(repos.external_user),
            Some(repos.user_profile),
            Some(repos.relationship),
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
