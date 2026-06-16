//! QQ Bot cold‑initialization binary.
//!
//! Connects to NapCat via the OneBot HTTP API and populates the database
//! with all known groups and members. Intended for first‑time setup or
//! full re‑sync.
//!
//! # Usage
//!
//! ```bash
//! cargo run --bin qq_bot_init
//! ```
//!
//! Configuration is read from `config.toml` (or `$CONFIG_PATH`) just like
//! the main server.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use server_rs::domain::qq_bot::{
    BotAccount, ExternalUser, GroupConfig, GroupMember, MemoryPolicy, ReplyPolicy, TriggerPolicy,
};
use server_rs::domain::qq_bot::repository::{
    BotAccountRepository, ExternalUserRepository, GroupMemberRepository, GroupRepository,
};
use server_rs::infra::persistence::seaorm_db::init_db;
use server_rs::infra::qq_bot::napcat::api::NapCatApiClient;
use server_rs::infra::qq_bot::repositories::seaorm_bot_account_repository::SeaOrmBotAccountRepository;
use server_rs::infra::qq_bot::repositories::seaorm_external_user_repository::SeaOrmExternalUserRepository;
use server_rs::infra::qq_bot::repositories::seaorm_group_member_repository::SeaOrmGroupMemberRepository;
use server_rs::infra::qq_bot::repositories::seaorm_group_repository::SeaOrmGroupRepository;
use server_rs::shared::config::AppConfig;

#[tokio::main]
async fn main() {
    let config = AppConfig::load();
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .compact()
        .init();

    if !config.qq_bot.enabled {
        tracing::warn!("qq_bot.enabled = false — nothing to initialise");
        return;
    }

    if config.qq_bot.self_qq_id == 0 {
        tracing::error!("qq_bot.self_qq_id is 0 — cannot initialise without a known bot QQ number");
        std::process::exit(1);
    }

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_secs() as i64;

    // ── Database ──────────────────────────────────────────────────
    let db = init_db(&config.database.url, 5)
        .await
        .expect("failed to connect to database");
    tracing::info!("数据库已连接");

    let bot_account_repo: Arc<dyn BotAccountRepository> =
        Arc::new(SeaOrmBotAccountRepository::new(db.clone()));
    let group_repo: Arc<dyn GroupRepository> =
        Arc::new(SeaOrmGroupRepository::new(db.clone()));
    let group_member_repo: Arc<dyn GroupMemberRepository> =
        Arc::new(SeaOrmGroupMemberRepository::new(db.clone()));
    let external_user_repo: Arc<dyn ExternalUserRepository> =
        Arc::new(SeaOrmExternalUserRepository::new(db.clone()));

    // ── NapCat API client ─────────────────────────────────────────
    let token = if config.qq_bot.http_token.is_empty() {
        None
    } else {
        Some(config.qq_bot.http_token.clone())
    };
    let api = NapCatApiClient::new(config.qq_bot.http_base_url.clone(), token);

    tracing::info!("querying NapCat API at {}", config.qq_bot.http_base_url);

    // ── Step 1: Login info → BotAccount ────────────────────────────
    let login = api.get_login_info().await.unwrap_or_else(|e| {
        tracing::error!(error = %e, "failed to get login info — is NapCat running?");
        std::process::exit(1);
    });
    tracing::info!(user_id = login.user_id, nickname = %login.nickname, "NapCat login info");

    let bot_account = BotAccount {
        bot_account_id: 0, // auto‑increment
        platform: "qq".into(),
        self_qq_id: login.user_id,
        display_name: Some(login.nickname),
        adapter: "napcat".into(),
        connection_mode: "websocket".into(),
        enabled: true,
    };
    let saved_account = bot_account_repo
        .upsert(&bot_account)
        .await
        .expect("failed to upsert bot account");
    let bot_account_id = saved_account.bot_account_id;
    tracing::info!(bot_account_id, "bot account upserted");

    // ── Step 2: Group list ─────────────────────────────────────────
    let groups = api.get_group_list().await.unwrap_or_else(|e| {
        tracing::error!(error = %e, "failed to get group list");
        std::process::exit(1);
    });
    tracing::info!(count = groups.len(), "groups fetched from NapCat");

    let mut total_members = 0usize;
    let mut total_external_users = 0usize;

    for g in &groups {
        // ── Group info → GroupConfig ───────────────────────────────
        let group_info = api
            .get_group_info(g.group_id)
            .await
            .unwrap_or_else(|e| {
                tracing::warn!(group_id = g.group_id, error = %e, "get_group_info failed, using list data");
                g.clone()
            });

        let group_config = GroupConfig {
            qq_group_id: group_info.group_id,
            group_name: Some(group_info.group_name.clone()),
            bot_account_id,
            enabled: true,
            trigger_policy: TriggerPolicy::Mention,
            reply_policy: ReplyPolicy {
                cooldown_secs: 30,
                max_segments: 5,
                max_chars_per_segment: 80,
                allow_proactive: false,
                keywords: vec![],
            },
            memory_policy: MemoryPolicy::GroupOnly,
        };
        group_repo.upsert(&group_config).await.unwrap_or_else(|e| {
            tracing::error!(group_id = g.group_id, error = %e, "failed to upsert group");
            std::process::exit(1);
        });
        tracing::info!(group_id = g.group_id, name = %group_info.group_name, "group upserted");

        // ── Member list → ExternalUser + GroupMember ───────────────
        let members = api
            .get_group_member_list(g.group_id)
            .await
            .unwrap_or_else(|e| {
                tracing::warn!(group_id = g.group_id, error = %e, "get_group_member_list failed, skipping");
                vec![]
            });

        for m in &members {
            // Ensure ExternalUser exists
            if external_user_repo
                .find_by_qq_user_id(m.user_id)
                .await
                .unwrap_or(None)
                .is_none()
            {
                let ext_user = ExternalUser {
                    qq_user_id: m.user_id,
                    internal_user_id: None,
                    nickname: Some(m.nickname.clone()),
                    avatar_url: None,
                    last_seen_at: Some(now),
                    memory_enabled: true,
                    persona_enabled: false,
                };
                external_user_repo.upsert(&ext_user).await.unwrap_or_else(|e| {
                    tracing::error!(user_id = m.user_id, error = %e, "failed to upsert external user");
                    std::process::exit(1);
                });
                total_external_users += 1;
            }

            // Upsert GroupMember
            let member = GroupMember {
                qq_group_id: g.group_id,
                qq_user_id: m.user_id,
                card: m.card.clone(),
                nickname: Some(m.nickname.clone()),
                role: m.role.clone(),
                title: m.title.clone(),
                join_time: m.join_time,
                last_seen_at: m.last_sent_time,
                status: "active".into(),
            };
            group_member_repo.upsert(&member).await.unwrap_or_else(|e| {
                tracing::error!(
                    group_id = g.group_id,
                    user_id = m.user_id,
                    error = %e,
                    "failed to upsert group member"
                );
                std::process::exit(1);
            });
            total_members += 1;
        }

        tracing::info!(
            group_id = g.group_id,
            members = members.len(),
            "group members synced"
        );
    }

    // ── Summary ────────────────────────────────────────────────────
    tracing::info!(
        groups = groups.len(),
        members = total_members,
        external_users = total_external_users,
        bot_account_id = bot_account_id,
        "QQ Bot cold‑init complete"
    );
}
