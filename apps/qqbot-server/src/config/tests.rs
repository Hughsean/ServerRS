//! 配置加载、校验、环境变量覆盖与白名单解析的单元测试。
//!
//! 这些测试覆盖 happy path 与多种拒绝路径（非法字段、越界、未知字段、凭据注入、
//! 环境变量前缀、白名单空数组/非法群号），拆分前已存在且行为必须保持不变。

use super::*;

fn parse(content: &str) -> Result<AppConfig, ConfigError> {
    let config: AppConfig = toml::from_str(content).map_err(|source| ConfigError::Parse {
        path: PathBuf::from("test.toml"),
        source,
    })?;
    config.validate(std::path::Path::new("."))?;
    Ok(config)
}

#[test]
fn accepts_independent_qqbot_configuration() {
    let config = parse(
        r#"
[napcat]
ws_url = "ws://127.0.0.1:6700"
http_base_url = "http://127.0.0.1:3000"
self_qq_id = 12345

[database]
url = "mysql://serverrs:password@127.0.0.1:3306/serverrs_qq"
"#,
    )
    .unwrap();

    assert_eq!(config.napcat.self_qq_id, 12345);
    assert_eq!(config.napcat.reconnect_max_secs, 60);
    assert_eq!(config.database.max_connections, 5);
    assert_eq!(config.ingestion.queue_capacity, 1_024);
    assert!(config.realtime_spool.enabled);
    assert_eq!(config.realtime_spool.admission_capacity, 1_024);
    assert!(config.thread_projection.enabled);
    assert_eq!(config.thread_projection.batch_size, 100);
    assert_eq!(config.thread_projection.same_conversation_window_secs, 300);
    assert!(config.thread_semantics.enabled);
    assert_eq!(config.thread_semantics.max_events, 50);
    assert_eq!(config.thread_semantics.max_total_chars, 50_000);
    assert!(config.thread_links.enabled);
    assert_eq!(config.thread_links.max_events, 100);
    assert_eq!(config.thread_links.max_total_chars, 100_000);
    assert!(!config.llm.enabled);
    assert_eq!(config.llm.base_url, "http://127.0.0.1:11434/v1");
}

#[test]
fn unauthenticated_napcat_rejects_token_fields_and_non_loopback_urls() {
    let token_error = toml::from_str::<AppConfig>(
        r#"
[napcat]
ws_url = "ws://127.0.0.1:6700"
http_base_url = "http://127.0.0.1:3000"
http_token = "deprecated"
self_qq_id = 12345

[database]
url = "mysql://serverrs:password@127.0.0.1:3306/serverrs_qq"
"#,
    )
    .unwrap_err();
    assert!(token_error.to_string().contains("unknown field"));

    let remote_error = parse(
        r#"
[napcat]
ws_url = "ws://192.0.2.10:6700"
http_base_url = "http://127.0.0.1:3000"
self_qq_id = 12345

[database]
url = "mysql://serverrs:password@127.0.0.1:3306/serverrs_qq"
"#,
    )
    .unwrap_err();
    assert!(remote_error.to_string().contains("loopback host"));

    let query_token_error = parse(
        r#"
[napcat]
ws_url = "ws://127.0.0.1:6700?access_token=deprecated"
http_base_url = "http://127.0.0.1:3000"
self_qq_id = 12345

[database]
url = "mysql://serverrs:password@127.0.0.1:3306/serverrs_qq"
"#,
    )
    .unwrap_err();
    assert!(query_token_error.to_string().contains("must not contain"));
}

#[test]
fn accepts_bounded_loopback_llm_configuration() {
    let config = parse(
        r#"
[napcat]
ws_url = "ws://127.0.0.1:6700"
http_base_url = "http://127.0.0.1:3000"
self_qq_id = 12345

[database]
url = "mysql://serverrs:password@127.0.0.1:3306/serverrs_qq"

[llm]
enabled = true
base_url = "http://127.0.0.1:11434/v1"
model = "qwen3:8b"
reasoning_mode = "qwen_no_think"
max_input_chars = 50000
max_output_tokens = 1500
max_candidates_per_kind = 10
"#,
    )
    .unwrap();
    assert!(config.llm.enabled);
    assert_eq!(config.llm.model, "qwen3:8b");
    assert_eq!(config.llm.reasoning_mode, LlmReasoningMode::QwenNoThink);
}

#[test]
fn rejects_llm_secret_in_toml_and_remote_plain_http() {
    let secret_error = toml::from_str::<AppConfig>(
        r#"
[napcat]
ws_url = "ws://127.0.0.1:6700"
http_base_url = "http://127.0.0.1:3000"
self_qq_id = 12345

[database]
url = "mysql://serverrs:password@127.0.0.1:3306/serverrs_qq"

[llm]
enabled = true
base_url = "https://api.example.com/v1"
model = "model"
api_key = "must-not-be-accepted"
"#,
    )
    .unwrap_err();
    assert!(secret_error.to_string().contains("unknown field"));

    let transport_error = parse(
        r#"
[napcat]
ws_url = "ws://127.0.0.1:6700"
http_base_url = "http://127.0.0.1:3000"
self_qq_id = 12345

[database]
url = "mysql://serverrs:password@127.0.0.1:3306/serverrs_qq"

[llm]
enabled = true
base_url = "http://192.0.2.10:8000/v1"
model = "model"
"#,
    )
    .unwrap_err();
    assert!(transport_error.to_string().contains("loopback"));
}

#[test]
fn rejects_unknown_business_configuration() {
    let error = toml::from_str::<AppConfig>(
        r#"
[napcat]
ws_url = "ws://127.0.0.1:6700"
http_base_url = "http://127.0.0.1:3000"
self_qq_id = 12345

[database]
url = "mysql://serverrs:password@127.0.0.1:3306/serverrs_qq"

[business]
auto_reply = true
"#,
    )
    .unwrap_err();

    assert!(error.to_string().contains("unknown field"));
}

#[test]
fn official_qq_secret_cannot_be_written_in_toml() {
    let error = toml::from_str::<AppConfig>(
        r#"
[napcat]
ws_url = "ws://127.0.0.1:6700"
http_base_url = "http://127.0.0.1:3000"
self_qq_id = 12345

[database]
url = "mysql://serverrs:password@127.0.0.1:3306/serverrs_qq"

[qq_open_platform]
enabled = true
app_id = "test-app"
owner_openid = "test-owner"
client_secret = "must-not-be-accepted"
"#,
    )
    .unwrap_err();

    assert!(error.to_string().contains("unknown field"));
}

#[test]
fn official_qq_owner_openid_respects_database_boundary() {
    let config = QqOpenPlatformConfig {
        enabled: true,
        app_id: "test-app".into(),
        owner_openid: "x".repeat(192),
        client_secret_file: Some(PathBuf::from("unused-in-validation")),
        ..QqOpenPlatformConfig::default()
    };
    let error = config.validate().unwrap_err();
    assert!(error.to_string().contains("1..=191"));
}

#[test]
fn rejects_non_websocket_listener_url() {
    let error = parse(
        r#"
[napcat]
ws_url = "http://127.0.0.1:6700"
http_base_url = "http://127.0.0.1:3000"
self_qq_id = 12345

[database]
url = "mysql://serverrs:password@127.0.0.1:3306/serverrs_qq"
"#,
    )
    .unwrap_err();

    assert!(error.to_string().contains("napcat.ws_url"));
}

#[test]
fn rejects_zero_or_unbounded_ingestion_configuration() {
    let zero_capacity = parse(
        r#"
[napcat]
ws_url = "ws://127.0.0.1:6700"
http_base_url = "http://127.0.0.1:3000"
self_qq_id = 12345

[database]
url = "mysql://serverrs:password@127.0.0.1:3306/serverrs_qq"

[ingestion]
queue_capacity = 0
"#,
    )
    .unwrap_err();
    assert!(zero_capacity.to_string().contains("queue_capacity"));

    let unbounded_capacity = parse(
        r#"
[napcat]
ws_url = "ws://127.0.0.1:6700"
http_base_url = "http://127.0.0.1:3000"
self_qq_id = 12345

[database]
url = "mysql://serverrs:password@127.0.0.1:3306/serverrs_qq"

[ingestion]
queue_capacity = 65537
"#,
    )
    .unwrap_err();
    assert!(unbounded_capacity.to_string().contains("queue_capacity"));
}

#[test]
fn rejects_invalid_ingestion_batch_bounds_and_capacity_relation() {
    let cases = [
        (
            "batch_size = 0",
            "batch_size",
            "queue_capacity = 64\nbatch_size = 0",
        ),
        (
            "batch_flush_ms = 0",
            "batch_flush_ms",
            "queue_capacity = 64\nbatch_flush_ms = 0",
        ),
        (
            "queue_capacity < batch_size",
            "queue_capacity must be >= batch_size",
            "queue_capacity = 2\nbatch_size = 3",
        ),
    ];
    for (label, expected, ingestion) in cases {
        let error = parse(&format!(
            r#"
[napcat]
ws_url = "ws://127.0.0.1:6700"
http_base_url = "http://127.0.0.1:3000"
self_qq_id = 12345

[database]
url = "mysql://serverrs@127.0.0.1:3306/serverrs_qq"

[ingestion]
{ingestion}
"#
        ))
        .unwrap_err();
        assert!(
            error.to_string().contains(expected),
            "{label} must be rejected: {error}"
        );
    }
}

#[test]
fn ingestion_batch_env_overrides_are_applied_before_validation() {
    let mut config = parse(
        r#"
[napcat]
ws_url = "ws://127.0.0.1:6700"
http_base_url = "http://127.0.0.1:3000"
self_qq_id = 12345

[database]
url = "mysql://serverrs@127.0.0.1:3306/serverrs_qq"
"#,
    )
    .unwrap();
    unsafe {
        std::env::set_var("QQBOT_INGESTION_QUEUE_CAPACITY", "128");
        std::env::set_var("QQBOT_INGESTION_BATCH_SIZE", "32");
        std::env::set_var("QQBOT_INGESTION_BATCH_FLUSH_MS", "25");
    }
    let result = config.apply_env_overrides();
    unsafe {
        std::env::remove_var("QQBOT_INGESTION_QUEUE_CAPACITY");
        std::env::remove_var("QQBOT_INGESTION_BATCH_SIZE");
        std::env::remove_var("QQBOT_INGESTION_BATCH_FLUSH_MS");
    }
    result.unwrap();
    assert_eq!(config.ingestion.queue_capacity, 128);
    assert_eq!(config.ingestion.batch_size, 32);
    assert_eq!(config.ingestion.batch_flush_ms, 25);
    config.validate(std::path::Path::new(".")).unwrap();
}

#[test]
fn example_configuration_remains_parseable() {
    let content = include_str!("../../config/qqbot.example.toml");
    let config: AppConfig = toml::from_str(content).unwrap();
    config.validate(std::path::Path::new(".")).unwrap();
}

#[test]
fn realtime_spool_env_overrides_are_applied_before_validation() {
    let mut config = parse(
        r#"
[napcat]
ws_url = "ws://127.0.0.1:6700"
http_base_url = "http://127.0.0.1:3000"
self_qq_id = 12345

[database]
url = "mysql://serverrs@127.0.0.1:3306/serverrs_qq"
"#,
    )
    .unwrap();
    unsafe {
        std::env::set_var("QQBOT_REALTIME_SPOOL_ADMISSION_CAPACITY", "64");
        std::env::set_var("QQBOT_REALTIME_SPOOL_RECOVERY_LEASE_SECS", "30");
        std::env::set_var("QQBOT_REALTIME_SPOOL_ENABLED", "false");
    }
    let result = config.apply_env_overrides();
    unsafe {
        std::env::remove_var("QQBOT_REALTIME_SPOOL_ADMISSION_CAPACITY");
        std::env::remove_var("QQBOT_REALTIME_SPOOL_RECOVERY_LEASE_SECS");
        std::env::remove_var("QQBOT_REALTIME_SPOOL_ENABLED");
    }
    result.unwrap();
    assert_eq!(config.realtime_spool.admission_capacity, 64);
    assert_eq!(config.realtime_spool.recovery_lease_secs, 30);
    assert!(!config.realtime_spool.enabled);
    config.validate(std::path::Path::new(".")).unwrap();
}

#[test]
fn realtime_spool_rejects_shared_recall_paths() {
    let error = parse(
        r#"
[napcat]
ws_url = "ws://127.0.0.1:6700"
http_base_url = "http://127.0.0.1:3000"
self_qq_id = 12345

[database]
url = "mysql://serverrs@127.0.0.1:3306/serverrs_qq"

[recall_wal]
path = "shared.wal"

[realtime_spool]
wal_path = "shared.wal"
"#,
    )
    .unwrap_err();
    assert!(error.to_string().contains("independent paths"));
}

#[test]
fn default_backfill_configuration_loads_and_validates() {
    let config = parse(
        r#"
[napcat]
ws_url = "ws://127.0.0.1:6700"
http_base_url = "http://127.0.0.1:3000"
self_qq_id = 12345

[database]
url = "mysql://serverrs:password@127.0.0.1:3306/serverrs_qq"
"#,
    )
    .unwrap();

    assert!(config.backfill.enabled);
    assert_eq!(config.backfill.page_size, 100);
    assert_eq!(config.backfill.max_concurrency, 2);
    // 默认配置必须能构造出合法的有界预算。
    assert!(config.backfill.budget().is_ok());
}

#[test]
fn backfill_page_size_must_be_between_1_and_100() {
    let too_small = parse(
        r#"
[napcat]
ws_url = "ws://127.0.0.1:6700"
http_base_url = "http://127.0.0.1:3000"
self_qq_id = 12345

[database]
url = "mysql://serverrs:password@127.0.0.1:3306/serverrs_qq"

[backfill]
page_size = 0
"#,
    )
    .unwrap_err();
    assert!(too_small.to_string().contains("page_size"));

    let too_large = parse(
        r#"
[napcat]
ws_url = "ws://127.0.0.1:6700"
http_base_url = "http://127.0.0.1:3000"
self_qq_id = 12345

[database]
url = "mysql://serverrs:password@127.0.0.1:3306/serverrs_qq"

[backfill]
page_size = 101
"#,
    )
    .unwrap_err();
    assert!(too_large.to_string().contains("page_size"));
}

#[test]
fn backfill_zero_or_over_limit_fields_are_rejected() {
    let zero_concurrency = parse(
        r#"
[napcat]
ws_url = "ws://127.0.0.1:6700"
http_base_url = "http://127.0.0.1:3000"
self_qq_id = 12345

[database]
url = "mysql://serverrs:password@127.0.0.1:3306/serverrs_qq"

[backfill]
max_concurrency = 0
"#,
    )
    .unwrap_err();
    assert!(zero_concurrency.to_string().contains("max_concurrency"));

    let over_limit_concurrency = parse(
        r#"
[napcat]
ws_url = "ws://127.0.0.1:6700"
http_base_url = "http://127.0.0.1:3000"
self_qq_id = 12345

[database]
url = "mysql://serverrs:password@127.0.0.1:3306/serverrs_qq"

[backfill]
max_concurrency = 65
"#,
    )
    .unwrap_err();
    assert!(
        over_limit_concurrency
            .to_string()
            .contains("max_concurrency")
    );
}

#[test]
fn backfill_retry_max_below_initial_is_rejected() {
    let error = parse(
        r#"
[napcat]
ws_url = "ws://127.0.0.1:6700"
http_base_url = "http://127.0.0.1:3000"
self_qq_id = 12345

[database]
url = "mysql://serverrs:password@127.0.0.1:3306/serverrs_qq"

[backfill]
retry_initial_ms = 1000
retry_max_ms = 500
"#,
    )
    .unwrap_err();
    assert!(error.to_string().contains("retry_max_ms"));
}

#[test]
fn backfill_unknown_field_is_rejected() {
    let error = toml::from_str::<AppConfig>(
        r#"
[napcat]
ws_url = "ws://127.0.0.1:6700"
http_base_url = "http://127.0.0.1:3000"
self_qq_id = 12345

[database]
url = "mysql://serverrs:password@127.0.0.1:3306/serverrs_qq"

[backfill]
auto_mark_complete = true
"#,
    )
    .unwrap_err();
    assert!(error.to_string().contains("unknown field"));
}

#[test]
fn thread_projection_bounds_and_unknown_fields_are_rejected() {
    let invalid_window = parse(
        r#"
[napcat]
ws_url = "ws://127.0.0.1:6700"
http_base_url = "http://127.0.0.1:3000"
self_qq_id = 12345

[database]
url = "mysql://serverrs:password@127.0.0.1:3306/serverrs_qq"

[thread_projection]
same_conversation_window_secs = 0
"#,
    )
    .unwrap_err();
    assert!(
        invalid_window
            .to_string()
            .contains("same_conversation_window_secs")
    );

    let unknown = toml::from_str::<AppConfig>(
        r#"
[napcat]
ws_url = "ws://127.0.0.1:6700"
http_base_url = "http://127.0.0.1:3000"
self_qq_id = 12345

[database]
url = "mysql://serverrs:password@127.0.0.1:3306/serverrs_qq"

[thread_projection]
llm_per_message = true
"#,
    )
    .unwrap_err();
    assert!(unknown.to_string().contains("unknown field"));
}

#[test]
fn thread_semantics_bounds_and_unknown_fields_are_rejected() {
    let invalid_budget = parse(
        r#"
[napcat]
ws_url = "ws://127.0.0.1:6700"
http_base_url = "http://127.0.0.1:3000"
self_qq_id = 12345

[database]
url = "mysql://serverrs:password@127.0.0.1:3306/serverrs_qq"

[thread_semantics]
max_total_chars = 0
"#,
    )
    .unwrap_err();
    assert!(invalid_budget.to_string().contains("max_total_chars"));

    let unknown = toml::from_str::<AppConfig>(
        r#"
[napcat]
ws_url = "ws://127.0.0.1:6700"
http_base_url = "http://127.0.0.1:3000"
self_qq_id = 12345

[database]
url = "mysql://serverrs:password@127.0.0.1:3306/serverrs_qq"

[thread_semantics]
trust_model_output = true
"#,
    )
    .unwrap_err();
    assert!(unknown.to_string().contains("unknown field"));
}

#[test]
fn follow_up_bounds_and_unknown_fields_are_rejected() {
    let invalid = parse(
        r#"
[napcat]
ws_url = "ws://127.0.0.1:6700"
http_base_url = "http://127.0.0.1:3000"
self_qq_id = 12345

[database]
url = "mysql://serverrs:password@127.0.0.1:3306/serverrs_qq"

[follow_up]
scan_interval_ms = 999
"#,
    )
    .unwrap_err();
    assert!(invalid.to_string().contains("follow_up.scan_interval_ms"));

    let unknown = toml::from_str::<AppConfig>(
        r#"
[napcat]
ws_url = "ws://127.0.0.1:6700"
http_base_url = "http://127.0.0.1:3000"
self_qq_id = 12345

[database]
url = "mysql://serverrs:password@127.0.0.1:3306/serverrs_qq"

[follow_up]
send_via_napcat = true
"#,
    )
    .unwrap_err();
    assert!(unknown.to_string().contains("unknown field"));
}

#[test]
fn agenda_bounds_unknown_fields_and_env_overrides_are_checked() {
    let invalid = parse(
        r#"
[napcat]
ws_url = "ws://127.0.0.1:6700"
http_base_url = "http://127.0.0.1:3000"
self_qq_id = 12345

[database]
url = "mysql://serverrs:password@127.0.0.1:3306/serverrs_qq"

[agenda]
scan_interval_ms = 999
"#,
    )
    .unwrap_err();
    assert!(invalid.to_string().contains("agenda.scan_interval_ms"));

    let unknown = toml::from_str::<AppConfig>(
        r#"
[napcat]
ws_url = "ws://127.0.0.1:6700"
http_base_url = "http://127.0.0.1:3000"
self_qq_id = 12345

[database]
url = "mysql://serverrs:password@127.0.0.1:3306/serverrs_qq"

[agenda]
send_via_napcat = true
"#,
    )
    .unwrap_err();
    assert!(unknown.to_string().contains("unknown field"));

    let mut config = parse(
        r#"
[napcat]
ws_url = "ws://127.0.0.1:6700"
http_base_url = "http://127.0.0.1:3000"
self_qq_id = 12345

[database]
url = "mysql://serverrs:password@127.0.0.1:3306/serverrs_qq"
"#,
    )
    .unwrap();
    // SAFETY: 单线程单元测试，仅在测试期间临时设置后立即移除。
    unsafe {
        std::env::set_var("QQBOT_AGENDA_ENABLED", "false");
        std::env::set_var("QQBOT_AGENDA_BATCH_SIZE", "50");
    }
    config.apply_env_overrides().unwrap();
    unsafe {
        std::env::remove_var("QQBOT_AGENDA_ENABLED");
        std::env::remove_var("QQBOT_AGENDA_BATCH_SIZE");
    }
    assert!(!config.agenda.enabled);
    assert_eq!(config.agenda.batch_size, 50);
}

#[test]
fn backfill_env_overrides_only_use_qqbot_prefix() {
    // 通过环境变量覆盖 page_size 并校验；保证只使用 QQBOT_BACKFILL_* 前缀。
    // SAFETY: 单线程单元测试，仅在测试期间临时设置后立即移除。
    unsafe {
        std::env::set_var("QQBOT_BACKFILL_PAGE_SIZE", "50");
    }
    let config: AppConfig = toml::from_str(
        r#"
[napcat]
ws_url = "ws://127.0.0.1:6700"
http_base_url = "http://127.0.0.1:3000"
self_qq_id = 12345

[database]
url = "mysql://serverrs:password@127.0.0.1:3306/serverrs_qq"
"#,
    )
    .unwrap();
    let mut config = config;
    config.apply_env_overrides().unwrap();
    config.validate(std::path::Path::new(".")).unwrap();
    assert_eq!(config.backfill.page_size, 50);
    // SAFETY: 同上，测试结束清理。
    unsafe {
        std::env::remove_var("QQBOT_BACKFILL_PAGE_SIZE");
    }
}

// ===== 白名单单元测试 =====

use std::io::Write;

/// 创建临时白名单 JSON 文件，返回文件路径。
fn write_whitelist_json(groups: &[i64]) -> tempfile::NamedTempFile {
    let mut file = tempfile::NamedTempFile::new().expect("create temp file");
    let json = serde_json::json!({ "groups": groups });
    write!(file, "{json}").expect("write temp file");
    file
}

#[test]
fn whitelist_loads_allowed_groups() {
    let file = write_whitelist_json(&[671260344, 123456]);
    let config = WhitelistConfig {
        whitelist_file: Some(file.path().to_path_buf()),
    };
    let groups = config.load_groups(std::path::Path::new(".")).unwrap();
    assert!(groups.contains(&671260344));
    assert!(groups.contains(&123456));
    assert_eq!(groups.len(), 2);
}

#[test]
fn whitelist_rejects_non_listed_group() {
    let file = write_whitelist_json(&[671260344]);
    let config = WhitelistConfig {
        whitelist_file: Some(file.path().to_path_buf()),
    };
    let groups = config.load_groups(std::path::Path::new(".")).unwrap();
    assert!(groups.contains(&671260344));
    // 非白名单群不在集合中
    assert!(!groups.contains(&999999999));
}

#[test]
fn whitelist_empty_config_means_no_filtering() {
    // 不配 whitelist_file 时返回空集合，表示不启用白名单（放行所有群）。
    let config = WhitelistConfig::default();
    let groups = config.load_groups(std::path::Path::new(".")).unwrap();
    assert!(groups.is_empty());
}

#[test]
fn whitelist_deduplicates_repeated_group_ids() {
    // 重复群号应该去重。
    let file = write_whitelist_json(&[671260344, 671260344, 123456]);
    let config = WhitelistConfig {
        whitelist_file: Some(file.path().to_path_buf()),
    };
    let groups = config.load_groups(std::path::Path::new(".")).unwrap();
    assert_eq!(groups.len(), 2);
}

#[test]
fn whitelist_rejects_empty_groups_array() {
    let file = write_whitelist_json(&[]);
    let config = WhitelistConfig {
        whitelist_file: Some(file.path().to_path_buf()),
    };
    let result = config.validate(std::path::Path::new("."));
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("不能为空"));
}

#[test]
fn whitelist_rejects_nonexistent_file() {
    let config = WhitelistConfig {
        whitelist_file: Some(PathBuf::from("/nonexistent/whitelist.json")),
    };
    let result = config.validate(std::path::Path::new("."));
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("不存在"));
}

#[test]
fn whitelist_rejects_invalid_json() {
    let mut file = tempfile::NamedTempFile::new().unwrap();
    write!(file, "{{invalid json}}").unwrap();
    let config = WhitelistConfig {
        whitelist_file: Some(file.path().to_path_buf()),
    };
    let result = config.validate(std::path::Path::new("."));
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("JSON 格式错误"));
}

#[test]
fn whitelist_resolves_relative_path_from_config_dir() {
    // 相对路径应以 config_dir 为基准。
    let file = write_whitelist_json(&[671260344]);
    let file_name = file
        .path()
        .file_name()
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();
    let dir = file.path().parent().unwrap().to_path_buf();
    let config = WhitelistConfig {
        whitelist_file: Some(PathBuf::from(file_name)),
    };
    let groups = config.load_groups(&dir).unwrap();
    assert!(groups.contains(&671260344));
}

#[test]
fn whitelist_absolute_path_ignores_config_dir() {
    let file = write_whitelist_json(&[671260344]);
    let config = WhitelistConfig {
        whitelist_file: Some(file.path().to_path_buf()),
    };
    // 绝对路径不应受 config_dir 影响。
    let groups = config
        .load_groups(std::path::Path::new("/some/other/dir"))
        .unwrap();
    assert!(groups.contains(&671260344));
}

// 评审第三轮 P1-3：HeartbeatConfig TOML 解析、默认值与环境变量覆盖测试。

#[test]
fn heartbeat_config_uses_defaults_when_section_absent() {
    // 缺失 [napcat.heartbeat] 段时使用默认宽松配置。
    let config = parse(
        r#"
[napcat]
ws_url = "ws://127.0.0.1:6700"
http_base_url = "http://127.0.0.1:3000"
self_qq_id = 12345

[database]
url = "mysql://serverrs:password@127.0.0.1:3306/serverrs_qq"
"#,
    )
    .unwrap();
    assert!(config.napcat.heartbeat.enabled);
    assert_eq!(config.napcat.heartbeat.startup_grace_secs, 300);
    assert_eq!(config.napcat.heartbeat.timeout_multiplier, 3);
}

#[test]
fn heartbeat_config_parses_custom_values() {
    let config = parse(
        r#"
[napcat]
ws_url = "ws://127.0.0.1:6700"
http_base_url = "http://127.0.0.1:3000"
self_qq_id = 12345

[napcat.heartbeat]
enabled = false
startup_grace_secs = 600
timeout_multiplier = 5

[database]
url = "mysql://serverrs:password@127.0.0.1:3306/serverrs_qq"
"#,
    )
    .unwrap();
    assert!(!config.napcat.heartbeat.enabled);
    assert_eq!(config.napcat.heartbeat.startup_grace_secs, 600);
    assert_eq!(config.napcat.heartbeat.timeout_multiplier, 5);
}

#[test]
fn heartbeat_config_rejects_zero_grace() {
    let error = parse(
        r#"
[napcat]
ws_url = "ws://127.0.0.1:6700"
http_base_url = "http://127.0.0.1:3000"
self_qq_id = 12345

[napcat.heartbeat]
startup_grace_secs = 0

[database]
url = "mysql://serverrs:password@127.0.0.1:3306/serverrs_qq"
"#,
    )
    .unwrap_err();
    assert!(error.to_string().contains("napcat.heartbeat"));
}

// 评审第四轮 P2：拼写错误的字段必须被拒绝，不能静默回退到默认值。
// 例如 enable=false（应为 enabled=false）不能导致 watchdog 保持启用。
#[test]
fn heartbeat_config_rejects_misspelled_field() {
    let error = toml::from_str::<AppConfig>(
        r#"
[napcat]
ws_url = "ws://127.0.0.1:6700"
http_base_url = "http://127.0.0.1:3000"
self_qq_id = 12345

[napcat.heartbeat]
enable = false

[database]
url = "mysql://serverrs:password@127.0.0.1:3306/serverrs_qq"
"#,
    )
    .unwrap_err();
    assert!(
        error.to_string().contains("unknown field"),
        "misspelled field must be rejected, got: {error}"
    );
}

#[test]
fn heartbeat_env_override_disables_watchdog() {
    // 评审第三轮 P1-3：环境变量覆盖 enabled=false（模拟不发送兼容心跳的 NapCat 实现）。
    // 直接调用 apply_env_overrides 验证环境变量覆盖逻辑。
    let mut config = parse(
        r#"
[napcat]
ws_url = "ws://127.0.0.1:6700"
http_base_url = "http://127.0.0.1:3000"
self_qq_id = 12345

[database]
url = "mysql://serverrs:password@127.0.0.1:3306/serverrs_qq"
"#,
    )
    .unwrap();
    assert!(config.napcat.heartbeat.enabled, "TOML default enabled");

    // Rust 2024: set_var/remove_var 在多线程上下文中是不安全的；测试是单线程的。
    unsafe {
        std::env::set_var("NAPCAT_HEARTBEAT_ENABLED", "false");
    }
    config.apply_env_overrides().unwrap();
    unsafe {
        std::env::remove_var("NAPCAT_HEARTBEAT_ENABLED");
    }

    assert!(
        !config.napcat.heartbeat.enabled,
        "env override NAPCAT_HEARTBEAT_ENABLED=false must disable watchdog"
    );
}

#[test]
fn memory_candidate_local_only_trust_requires_loopback_llm_endpoint() {
    // 远程 LLM：local_only 正文绝不能进入记忆提取（P0-2 信任判定对象是 llm.base_url）
    let remote = parse(
        r#"
[napcat]
ws_url = "ws://127.0.0.1:6700"
http_base_url = "http://127.0.0.1:3000"
self_qq_id = 12345

[database]
url = "mysql://serverrs:password@127.0.0.1:3306/serverrs_qq"

[llm]
enabled = true
base_url = "https://api.example.com/v1"
model = "model"
"#,
    )
    .unwrap();
    assert!(
        !remote.llm_endpoint_verified_loopback(),
        "remote LLM endpoint must not allow local_only content"
    );

    // 回环 LLM：NapCat 端点即使同样是回环也不改变判定——对象是 LLM 而非 NapCat。
    let loopback = parse(
        r#"
[napcat]
ws_url = "ws://127.0.0.1:6700"
http_base_url = "http://127.0.0.1:3000"
self_qq_id = 12345

[database]
url = "mysql://serverrs:password@127.0.0.1:3306/serverrs_qq"

[llm]
enabled = true
base_url = "http://127.0.0.1:11434/v1"
model = "model"
"#,
    )
    .unwrap();
    assert!(loopback.llm_endpoint_verified_loopback());
}

#[test]
fn default_reply_reconcile_configuration_loads_and_validates() {
    let config = parse(
        r#"
[napcat]
ws_url = "ws://127.0.0.1:6700"
http_base_url = "http://127.0.0.1:3000"
self_qq_id = 12345

[database]
url = "mysql://serverrs:password@127.0.0.1:3306/serverrs_qq"
"#,
    )
    .unwrap();

    assert!(config.reply_reconcile.enabled);
    assert_eq!(config.reply_reconcile.batch_size, 100);
    assert_eq!(config.reply_reconcile.lease_secs, 60);
    // 默认配置必须能构造出合法的有界预算。
    let budget = config.reply_reconcile.budget();
    assert_eq!(budget.batch_size, 100);
    assert!(budget.retry_max_ms >= budget.retry_initial_ms);
}

#[test]
fn reply_reconcile_unknown_field_is_rejected() {
    let error = toml::from_str::<AppConfig>(
        r#"
[napcat]
ws_url = "ws://127.0.0.1:6700"
http_base_url = "http://127.0.0.1:3000"
self_qq_id = 12345

[database]
url = "mysql://serverrs:password@127.0.0.1:3306/serverrs_qq"

[reply_reconcile]
unexpected = 1
"#,
    )
    .unwrap_err();
    assert!(error.to_string().contains("unknown field"));
}

#[test]
fn reply_reconcile_bounds_are_rejected() {
    let error = parse(
        r#"
[napcat]
ws_url = "ws://127.0.0.1:6700"
http_base_url = "http://127.0.0.1:3000"
self_qq_id = 12345

[database]
url = "mysql://serverrs:password@127.0.0.1:3306/serverrs_qq"

[reply_reconcile]
batch_size = 0
"#,
    )
    .unwrap_err();
    assert!(error.to_string().contains("reply_reconcile"));
}
