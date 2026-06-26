use server_rs::{bootstrap, shared::config::AppConfig};

#[tokio::main]
async fn main() {
    let config = AppConfig::load();
    init_tracing(&config.logging.level);
    if let Err(err) = bootstrap::runtime::run(config).await {
        tracing::error!(error = %err, "服务器运行出错");
    }
}

fn init_tracing(_configured_level: &str) {
    let timer = tracing_subscriber::fmt::time::OffsetTime::new(
        time::UtcOffset::from_hms(8, 0, 0).expect("valid UTC+8 offset"),
        time::macros::format_description!("[month]-[day] [hour]:[minute]:[second]"),
    );

    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_target(true)
        .with_timer(timer)
        .with_line_number(true)
        .with_thread_ids(true)
        .compact()
        .init();
}
