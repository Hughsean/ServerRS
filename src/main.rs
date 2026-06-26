use server_rs::{bootstrap, shared};
use shared::config::AppConfig;

#[tokio::main]
async fn main() {
    let config = AppConfig::load();
    init_tracing(&config.logging.level);
    if let Err(err) = bootstrap::runtime::run(config).await {
        tracing::error!(error = %err, "服务器运行出错");
    }
}

fn init_tracing(configured_level: &str) {
    let env_filter = std::env::var("RUST_LOG").unwrap_or_default();
    let combined = if env_filter.is_empty() {
        format!("{configured_level},sqlx=warn")
    } else if env_filter.contains("sqlx") {
        // 用户明确设置了 sqlx 级别 — 尊重它。
        env_filter
    } else {
        // 追加 sqlx=warn 以默认抑制 sqlx 查询日志。
        format!("{},sqlx=warn", env_filter)
    };
    let f = tracing_subscriber::EnvFilter::new(&combined);
    tracing_subscriber::fmt()
        .with_env_filter(f)
        .with_target(true)
        // .with_file(true)
        .with_line_number(true)
        .with_thread_ids(true)
        .compact()
        .init();
}
