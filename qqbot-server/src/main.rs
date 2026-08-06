use qqbot_server::{config::AppConfig, runtime};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    init_tracing();
    let (config, config_dir) = AppConfig::load().map_err(|error| {
        tracing::error!(error = %error, "QQBot 适配器配置加载失败");
        error
    })?;
    runtime::run(config, config_dir).await?;
    Ok(())
}

fn init_tracing() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(true)
        .init();
}
