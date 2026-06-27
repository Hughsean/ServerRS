use server_rs::{bootstrap, shared::config::AppConfig};
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::{EnvFilter, fmt, layer::SubscriberExt, util::SubscriberInitExt};

#[tokio::main]
async fn main() {
    let config = AppConfig::load();
    let _guard = init_tracing(&config.logging.level);
    if let Err(err) = bootstrap::runtime::run(config).await {
        tracing::error!(error = %err, "服务器运行出错");
    }
}

pub fn init_tracing(configured_level: &str) -> WorkerGuard {
    let timer = fmt::time::OffsetTime::new(
        time::UtcOffset::from_hms(8, 0, 0).expect("valid UTC+8 offset"),
        time::macros::format_description!("[day] [hour]:[minute]:[second]"),
    );

    // logs/app.log.YYYY-MM-DD
    let file_appender = tracing_appender::rolling::daily("logs", "app.log");

    // 非阻塞文件写入
    let (file_writer, guard) = tracing_appender::non_blocking(file_appender);

    let env_filter = build_env_filter(configured_level);

    let stdout_layer = fmt::layer()
        .with_writer(std::io::stdout)
        .with_target(true)
        .with_timer(timer.clone())
        .with_line_number(true)
        .with_thread_ids(true)
        .compact();

    let file_layer = fmt::layer()
        .with_writer(file_writer)
        .with_target(true)
        .with_timer(timer)
        .with_line_number(true)
        .with_thread_ids(true)
        .with_ansi(false)
        .compact();

    tracing_subscriber::registry()
        .with(env_filter)
        .with(stdout_layer)
        .with(file_layer)
        .init();

    guard
}

fn build_env_filter(configured_level: &str) -> EnvFilter {
    if let Ok(filter) = EnvFilter::try_from_default_env() {
        return filter;
    }

    let mut filter =
        EnvFilter::try_new(configured_level).unwrap_or_else(|_| EnvFilter::new("info"));

    for directive in ["sqlx::query=warn"] {
        if let Ok(directive) = directive.parse() {
            filter = filter.add_directive(directive);
        }
    }

    filter
}
