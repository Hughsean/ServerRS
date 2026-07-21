/// 初始化测试日志器，配合 `cargo test -- --nocapture` 查看 tracing 输出。
pub fn init() {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        tracing_subscriber::EnvFilter::new(
            "digital_human=debug,\
             digital_human::app::agent=trace,\
             digital_human::app::context_routing=debug,\
             digital_human::app::fresh_context=trace,\
             digital_human::app::session=debug,\
             reqwest=warn,\
             qdrant_client=warn,\
             hyper=warn,\
             tower_http=warn",
        )
    });

    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_line_number(true)
        .with_test_writer()
        .pretty()
        .try_init();
}
