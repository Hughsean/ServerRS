/// 初始化测试日志器，配合 `cargo test -- --nocapture` 查看 tracing 输出。
pub fn init() {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        tracing_subscriber::EnvFilter::new(
            "server_rs=debug,\
             server_rs::app::agent=trace,\
             server_rs::app::context_routing=debug,\
             server_rs::app::fresh_context=trace,\
             server_rs::app::session=debug,\
             reqwest=warn,\
             qdrant_client=warn,\
             hyper=warn,\
             tower_http=warn",
        )
    });

    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_test_writer()
        .try_init();
}
