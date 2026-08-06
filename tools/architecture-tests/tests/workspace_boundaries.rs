use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn manifest(relative: &str) -> toml::Value {
    let path = workspace_root().join(relative);
    toml::from_str(
        &fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display())),
    )
    .unwrap_or_else(|error| panic!("failed to parse {}: {error}", path.display()))
}

fn dependencies(manifest: &toml::Value) -> BTreeSet<String> {
    manifest
        .get("dependencies")
        .and_then(toml::Value::as_table)
        .expect("manifest must have a dependencies table")
        .keys()
        .cloned()
        .collect()
}

fn rust_sources(relative: &str) -> String {
    let mut files = Vec::new();
    collect_rs_files(&workspace_root().join(relative), &mut files);
    files.sort();
    files
        .into_iter()
        .map(|path| fs::read_to_string(path).expect("Rust source must be readable"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn collect_rs_files(path: &Path, output: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(path).expect("source directory must be readable") {
        let path = entry.expect("directory entry must be readable").path();
        if path.is_dir() {
            collect_rs_files(&path, output);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            output.push(path);
        }
    }
}

#[test]
fn workspace_has_exactly_the_seven_intended_members() {
    let root = manifest("Cargo.toml");
    let members = root["workspace"]["members"]
        .as_array()
        .expect("workspace.members must be an array")
        .iter()
        .map(|value| value.as_str().expect("member must be a string").to_owned())
        .collect::<BTreeSet<_>>();

    assert_eq!(
        members,
        BTreeSet::from([
            "qqbot-server".to_owned(),
            "agent-core".to_owned(),
            "personal-secretary".to_owned(),
            "personal-secretary-mysql".to_owned(),
            "qqbot".to_owned(),
            "qq-open-platform".to_owned(),
            "tools/architecture-tests".to_owned(),
        ])
    );
}

#[test]
fn qqbot_crates_have_an_onion_dependency_graph() {
    let personal_secretary = dependencies(&manifest("personal-secretary/Cargo.toml"));
    let personal_secretary_mysql = dependencies(&manifest("personal-secretary-mysql/Cargo.toml"));
    let qq = dependencies(&manifest("qqbot/Cargo.toml"));
    let official_qq = dependencies(&manifest("qq-open-platform/Cargo.toml"));
    let qq_server = dependencies(&manifest("qqbot-server/Cargo.toml"));

    for forbidden in ["agent-core", "personal-secretary", "sea-orm", "sqlx"] {
        assert!(
            !qq.contains(forbidden),
            "qqbot must not depend on {forbidden}"
        );
    }
    for forbidden in [
        "axum",
        "personal-secretary-mysql",
        "qqbot",
        "reqwest",
        "sea-orm",
        "sqlx",
    ] {
        assert!(
            !personal_secretary.contains(forbidden),
            "personal-secretary must not depend on {forbidden}"
        );
    }
    for required in ["personal-secretary", "sea-orm"] {
        assert!(
            personal_secretary_mysql.contains(required),
            "personal-secretary-mysql must depend on {required}"
        );
    }
    for forbidden in ["axum", "qqbot", "qq-open-platform", "reqwest"] {
        assert!(
            !personal_secretary_mysql.contains(forbidden),
            "personal-secretary-mysql must not depend on {forbidden}"
        );
    }
    for forbidden in [
        "agent-core",
        "personal-secretary",
        "qqbot",
        "sea-orm",
        "sqlx",
    ] {
        assert!(
            !official_qq.contains(forbidden),
            "qq-open-platform must not depend on {forbidden}"
        );
    }

    assert!(qq_server.contains("qqbot"));
    assert!(qq_server.contains("personal-secretary"));
    assert!(qq_server.contains("personal-secretary-mysql"));
    assert!(qq_server.contains("qq-open-platform"));
    assert!(!personal_secretary.contains("qq-open-platform"));
    assert!(!qq.contains("qq-open-platform"));
}

#[test]
fn qq_open_platform_is_a_protocol_adapter_without_business_or_database_code() {
    let sources = rust_sources("qq-open-platform/src");
    for forbidden in [
        "personal_secretary",
        "sea_orm",
        "sqlx",
        "DatabaseConnection",
        "send_private_msg",
        "send_group_msg",
    ] {
        assert!(
            !sources.contains(forbidden),
            "QQ Open Platform adapter contains forbidden marker {forbidden}"
        );
    }
    for required in ["QqBotCredentials", "TokenManager", "QqGatewayClient"] {
        assert!(
            sources.contains(required),
            "QQ Open Platform adapter is missing {required}"
        );
    }
}

#[test]
fn personal_secretary_domain_and_application_are_infrastructure_neutral() {
    let sources = rust_sources("personal-secretary/src");
    for forbidden in [
        "qqbot::",
        "napcat::",
        "qq_open_platform::",
        "reqwest",
        "tokio::spawn",
        "DatabaseConnection",
        "DbErr",
        "sea_orm",
        "sqlx",
        "build_mysql_",
    ] {
        assert!(
            !sources.contains(forbidden),
            "personal-secretary domain/app source contains concrete QQ/Digital Human marker {forbidden}"
        );
    }
}

#[test]
fn qqbot_server_application_layer_is_protocol_and_infrastructure_neutral() {
    let sources = rust_sources("qqbot-server/src/application");
    for forbidden in [
        "use sea_orm",
        "sqlx::",
        "DatabaseConnection",
        "personal_secretary_mysql",
        "use reqwest",
        "use qqbot::",
        "use qq_open_platform::",
        "std::fs::",
        "use ring::",
        "crate::health_runtime",
        "crate::llm",
        "crate::qq_open_platform_mysql",
        "crate::recall",
    ] {
        assert!(
            !sources.contains(forbidden),
            "qqbot-server application layer contains outer-layer marker {forbidden}"
        );
    }
}

#[test]
fn realtime_message_spool_is_independent_and_uses_durable_sync() {
    let source = fs::read_to_string(
        workspace_root().join("qqbot-server/src/infrastructure/realtime_spool.rs"),
    )
    .expect("read realtime message spool adapter");

    for required in [
        "WAL_MAGIC: &[u8; 4] = b\"QMWL\"",
        "CHECKPOINT_MAGIC: &[u8; 4] = b\"QMCP\"",
        "const TOTAL_BYTES: u64 = 512 * 1024 * 1024",
        "MOVEFILE_WRITE_THROUGH",
        ".sync_all()",
    ] {
        assert!(
            source.contains(required),
            "realtime message spool is missing required durability marker {required}"
        );
    }
    for forbidden in [
        "b\"RSPL\"",
        "RecallWal",
        "RecallWalConfig",
        "QQBOT_RECALL",
        ".sync_data()",
        "crate::recall",
    ] {
        assert!(
            !source.contains(forbidden),
            "realtime message spool shares or weakens a forbidden boundary: {forbidden}"
        );
    }
}

#[test]
fn personal_secretary_backfill_is_protocol_neutral_and_no_send_calls() {
    let sources = rust_sources("personal-secretary/src");
    // 回补领域与应用层不得出现任何发送/撤回调用或 HTTP/数据库实现。
    for forbidden in [
        "send_group_msg",
        "send_private_msg",
        "group_poke",
        "delete_msg",
        "reqwest",
        "NapCatReadOnlyClient",
        "NapCatCapabilityReadT",
        "NapCatDirectoryReadT",
        "NapCatHistoryReadT",
    ] {
        assert!(
            !sources.contains(forbidden),
            "personal-secretary backfill source contains forbidden marker {forbidden}"
        );
    }
    // 回补领域模型和用例必须存在。
    assert!(sources.contains("BackfillGapUseCase"));
    assert!(sources.contains("HistoryBackfillSourceT"));
    assert!(sources.contains("BackfillStateStoreT"));
    assert!(sources.contains("HistoryCompleteness"));
    assert!(sources.contains("ThreadProjectionUseCase"));
    assert!(sources.contains("DeterministicThreadPlanner"));
    assert!(sources.contains("ThreadSemanticUseCase"));
    assert!(sources.contains("validate_semantic_patch"));
}

#[test]
fn gap003_backfill_contract_is_typed_and_protocol_neutral() {
    let root = workspace_root();
    let domain = fs::read_to_string(root.join("personal-secretary/src/domain/backfill.rs"))
        .expect("backfill domain source must be readable");
    let application =
        fs::read_to_string(root.join("personal-secretary/src/application/backfill_service.rs"))
            .expect("backfill application source must be readable");
    let sources = format!("{domain}\n{application}");

    for required in [
        "BackfillReadDirection",
        "NewestToOldest",
        "BackfillContinuation",
        "ProvenHistoryStart",
        "UnprovenStop",
        "page_order_evidence_proven",
        "UntrustedPageOrder",
        "HistoryBackfillSourceT",
    ] {
        assert!(
            sources.contains(required),
            "typed GAP-003 contract is missing {required}"
        );
    }
    assert!(
        !domain.contains("next_cursor: Option<BackfillCursor>"),
        "backfill page must not conflate end evidence with Option<Cursor>"
    );
    for forbidden in [
        "use qqbot::",
        "use qq_open_platform::",
        "use reqwest",
        "use sea_orm",
        "DatabaseConnection",
        "sqlx::",
    ] {
        assert!(
            !sources.contains(forbidden),
            "backfill domain/application depends on outer protocol or storage marker {forbidden}"
        );
    }
}

#[test]
fn napcat_readonly_action_whitelist_is_exactly_seven_and_history_consumer_uses_only_port() {
    let root = workspace_root();
    let api = fs::read_to_string(root.join("qqbot/src/napcat/api.rs"))
        .expect("NapCat API source must be readable");
    let as_path = api
        .split("const fn as_path")
        .nth(1)
        .and_then(|tail| tail.split("#[derive(Debug, Clone)]").next())
        .expect("ReadOnlyAction::as_path body must exist");
    let expected = [
        "get_version_info",
        "get_status",
        "get_friend_list",
        "get_group_list",
        "get_recent_contact",
        "get_group_msg_history",
        "get_friend_msg_history",
    ];
    assert_eq!(as_path.matches("=> \"").count(), expected.len());
    for action in expected {
        assert_eq!(
            as_path.matches(&format!("=> \"{action}\"")).count(),
            1,
            "read-only action {action} must appear exactly once in the private path mapping"
        );
    }

    let adapter =
        fs::read_to_string(root.join("qqbot-server/src/adapters/napcat_history_source.rs"))
            .expect("NapCat history adapter source must be readable");
    assert!(adapter.contains("client: Arc<dyn NapCatHistoryReadT>"));
    assert!(!adapter.contains("client: Arc<NapCatReadOnlyClient>"));
    assert!(!adapter.contains("reverse_order"));
    assert!(adapter.contains("HistoryReadDirection::TowardOlder"));
}

#[test]
fn qqbot_does_not_depend_on_personal_secretary_or_database() {
    let qq_manifest = manifest("qqbot/Cargo.toml");
    let deps = dependencies(&qq_manifest);
    for forbidden in ["personal-secretary", "sea-orm", "sqlx"] {
        assert!(
            !deps.contains(forbidden),
            "qqbot must not depend on {forbidden}"
        );
    }
    let sources = rust_sources("qqbot/src");
    for forbidden in [
        "personal_secretary",
        "sea_orm",
        "DatabaseConnection",
        "BackfillStateStoreT",
        "HistoryBackfillSourceT",
    ] {
        assert!(
            !sources.contains(forbidden),
            "qqbot source must not contain personal-secretary/backfill marker {forbidden}"
        );
    }
}

#[test]
fn qqbot_server_napcat_paths_do_not_send_through_the_local_qq_account() {
    let root = workspace_root();
    let paths = [
        "qqbot-server/src/adapters/napcat_directory.rs",
        "qqbot-server/src/adapters/napcat_history_source.rs",
        "qqbot-server/src/bootstrap/thread_pipeline.rs",
        "qqbot-server/src/runtime/connection_loop.rs",
    ];
    let sources = paths
        .iter()
        .map(|path| fs::read_to_string(root.join(path)).expect("NapCat server source must exist"))
        .collect::<String>();
    for forbidden in [
        "reqwest::Client",
        "call_api",
        "action: &str",
        "action: String",
        "path: &str",
        "path: String",
        "params: Value",
        "params: serde_json::Value",
        "send_msg",
        "send_group_msg",
        "send_private_msg",
        "delete_msg",
        "group_poke",
        "friend_poke",
        "set_group_",
        "set_friend_",
        "set_qq_",
        "set_online_",
        "upload_",
        "create_group_file",
        "delete_group_file",
        "mark_msg_as_read",
    ] {
        assert!(
            !sources.contains(forbidden),
            "qqbot-server NapCat wiring contains forbidden marker {forbidden}"
        );
    }
}

#[test]
fn qqbot_llm_adapter_only_produces_bounded_semantic_candidates() {
    let source =
        fs::read_to_string(workspace_root().join("qqbot-server/src/infrastructure/llm.rs"))
            .expect("QQBot LLM adapter must be readable");
    for forbidden in [
        "sea_orm",
        "sqlx",
        "send_group_msg",
        "send_private_msg",
        "std::process",
        "Command::new",
    ] {
        assert!(
            !source.contains(forbidden),
            "QQBot LLM adapter contains forbidden direct infrastructure marker {forbidden}"
        );
    }
    for required in [
        "max_input_chars",
        "max_response_bytes",
        "source_event_ids",
        "validate_semantic_patch",
        "ThreadSemanticExtractorT",
    ] {
        assert!(
            source.contains(required),
            "QQBot LLM adapter is missing bounded semantic marker {required}"
        );
    }
}

#[test]
fn napcat_callback_does_not_wait_for_mysql() {
    let callback =
        fs::read_to_string(workspace_root().join("qqbot-server/src/runtime/handlers.rs"))
            .expect("qqbot-server callback handler must be readable");
    let spool_runtime = fs::read_to_string(
        workspace_root().join("qqbot-server/src/runtime/realtime_spool_runtime.rs"),
    )
    .expect("qqbot-server realtime spool runtime must be readable");
    let worker = fs::read_to_string(
        workspace_root().join("qqbot-server/src/application/ingestion_worker.rs"),
    )
    .expect("qqbot-server ingestion worker must be readable");

    assert!(callback.contains("try_admit"));
    assert!(!callback.contains("insert_message_if_absent"));
    assert!(!callback.contains("RealtimeMessageSpool"));
    assert!(!callback.contains("sync_all"));
    assert!(spool_runtime.contains("spawn_realtime_spool_writer"));
    assert!(spool_runtime.contains("std::thread::Builder"));
    assert!(spool_runtime.contains("std_mpsc::sync_channel"));
    assert!(worker.contains("mpsc::channel"));
    assert!(worker.contains("insert_message_if_absent"));
    assert!(worker.contains("RealtimeSpoolCheckpointT"));
    assert!(!worker.contains("RealtimeMessageSpool"));
}

#[test]
fn backfill_worker_is_decoupled_from_realtime_websocket() {
    let runtime = rust_sources("qqbot-server/src/runtime");
    let backfill_worker = fs::read_to_string(
        workspace_root().join("qqbot-server/src/application/backfill/worker.rs"),
    )
    .expect("qqbot-server backfill worker must be readable");
    let napcat_history_source = fs::read_to_string(
        workspace_root().join("qqbot-server/src/adapters/napcat_history_source.rs"),
    )
    .expect("qqbot-server napcat history source must be readable");

    // A2 后回补装配下沉到 bootstrap，与实时 WebSocket 接收解耦。
    // runtime + bootstrap 合并检查仍包含回补装配与并发/取消语义。
    let bootstrap = rust_sources("qqbot-server/src/bootstrap");
    let runtime_and_bootstrap = format!("{runtime}{bootstrap}");
    assert!(runtime_and_bootstrap.contains("spawn_backfill_worker"));
    assert!(backfill_worker.contains("BackfillRunner"));
    // 必须使用 JoinSet 真正并发，而非串行 await + 信号量假并发。
    assert!(
        backfill_worker.contains("JoinSet"),
        "backfill worker must use JoinSet for real concurrency"
    );
    // 关闭必须有取消标志，避免 shutdown 永久挂起。
    assert!(
        backfill_worker.contains("AtomicBool"),
        "backfill worker must have a shutdown cancellation flag"
    );
    // 回补不调用 NapCat 发送/撤回，只走只读历史接口与统一幂等入口。
    for forbidden in [
        "send_group_msg",
        "send_private_msg",
        "group_poke",
        "delete_msg",
    ] {
        assert!(
            !napcat_history_source.contains(forbidden),
            "napcat history source must not call mutation {forbidden}"
        );
    }
}

#[test]
fn napcat_adapter_exposes_no_personal_account_mutations() {
    let api = fs::read_to_string(workspace_root().join("qqbot/src/napcat/api.rs"))
        .expect("NapCat API source must be readable");
    for forbidden in [
        "send_msg",
        "send_group_msg",
        "send_private_msg",
        "delete_msg",
        "group_poke",
        "friend_poke",
        "set_group_",
        "set_friend_",
        "set_qq_",
        "set_online_",
        "upload_",
        "create_group_file",
        "delete_group_file",
        "mark_msg_as_read",
    ] {
        assert!(
            !api.contains(forbidden),
            "NapCat personal-account adapter must not expose mutation {forbidden}"
        );
    }
}

#[test]
fn napcat_http_surface_is_private_and_trait_allowlisted() {
    let napcat_mod = fs::read_to_string(workspace_root().join("qqbot/src/napcat/mod.rs"))
        .expect("NapCat module source must be readable");
    let api = fs::read_to_string(workspace_root().join("qqbot/src/napcat/api.rs"))
        .expect("NapCat API source must be readable");
    let qqbot_sources = rust_sources("qqbot/src");
    let server_sources = rust_sources("qqbot-server/src");

    assert!(napcat_mod.contains("mod api;"));
    assert!(!napcat_mod.contains("pub mod api;"));
    assert!(!qqbot_sources.contains("NapCatApiClient"));
    assert!(!server_sources.contains("NapCatApiClient"));
    assert!(!api.contains("type NapCatApiClient"));
    for required in [
        "pub trait NapCatCapabilityReadT: Send + Sync",
        "pub trait NapCatDirectoryReadT: Send + Sync",
        "pub trait NapCatHistoryReadT: Send + Sync",
    ] {
        assert!(api.contains(required));
    }

    let impl_start = api
        .find("impl NapCatReadOnlyClient")
        .expect("NapCatReadOnlyClient inherent impl must exist");
    let impl_end = api[impl_start..]
        .find("#[async_trait]\nimpl NapCatDirectoryReadT")
        .expect("directory trait impl must follow the inherent impl");
    let inherent_impl = &api[impl_start..impl_start + impl_end];
    assert!(!inherent_impl.contains("pub async fn"));
    for forbidden in [
        "pub fn call",
        "pub fn request",
        "pub fn post",
        "pub fn execute_action",
        "pub fn call_api",
        "pub fn raw_action",
        "pub async fn call",
        "pub async fn request",
        "pub async fn post",
        "pub async fn execute_action",
        "pub async fn call_api",
        "pub async fn raw_action",
    ] {
        assert!(!api.contains(forbidden));
    }

    fn trait_methods<'a>(api: &'a str, trait_name: &str) -> Vec<&'a str> {
        let marker = format!("pub trait {trait_name}");
        let start = api.find(&marker).expect("read-only trait must exist");
        let body = &api[start..];
        let end = body.find("\n}\n").expect("read-only trait must close");
        body[..end]
            .lines()
            .filter_map(|line| line.trim().strip_prefix("async fn "))
            .filter_map(|rest| rest.find('(').map(|end| &rest[..end]))
            .collect()
    }

    assert_eq!(
        trait_methods(&api, "NapCatCapabilityReadT"),
        ["get_version_info", "get_status"]
    );
    assert_eq!(
        trait_methods(&api, "NapCatDirectoryReadT"),
        ["get_friend_list", "get_group_list", "get_recent_contact"]
    );
    assert_eq!(
        trait_methods(&api, "NapCatHistoryReadT"),
        ["get_group_msg_history", "get_friend_msg_history"]
    );
}

#[test]
fn napcat_consumers_hold_only_their_minimum_read_capability() {
    let root = workspace_root();
    let directory = fs::read_to_string(root.join("qqbot-server/src/adapters/napcat_directory.rs"))
        .expect("NapCat directory adapter must be readable");
    let history =
        fs::read_to_string(root.join("qqbot-server/src/adapters/napcat_history_source.rs"))
            .expect("NapCat history adapter must be readable");
    let capabilities = fs::read_to_string(root.join("qqbot/src/napcat/capabilities.rs"))
        .expect("NapCat capability source must be readable");

    assert!(directory.contains("client: Arc<dyn NapCatDirectoryReadT>"));
    assert!(!directory.contains("Arc<NapCatReadOnlyClient>"));
    assert!(history.contains("client: Arc<dyn NapCatHistoryReadT>"));
    assert!(!history.contains("Arc<NapCatReadOnlyClient>"));
    assert!(capabilities.contains("client: &dyn NapCatCapabilityReadT"));
    assert!(!capabilities.contains("NapCatReadOnlyClient"));
}

#[test]
fn qqbot_database_is_owned_by_the_qqbot_application() {
    assert!(
        !workspace_root().join("database/sql").exists(),
        "the removed legacy database SQL tree must not return"
    );
    let qqbot_database = workspace_root().join("qqbot-server/database");
    let baseline = qqbot_database.join("baseline/20260806_qqbot_schema_v2.sql");
    assert!(baseline.is_file(), "QQBot Schema Baseline v2 is missing");
    let baseline_sql = fs::read_to_string(baseline).expect("QQBot baseline must be readable");
    for required_object in [
        "secretary_accounts",
        "secretary_source_events",
        "secretary_action_runs",
        "secretary_participant_profiles",
    ] {
        assert!(
            baseline_sql.contains(required_object),
            "QQBot baseline is missing required object: {required_object}"
        );
    }

    let archived_migrations = fs::read_dir(qqbot_database.join("archive/pre_v1"))
        .expect("QQBot pre-v1 migration archive must be readable")
        .filter_map(Result::ok)
        .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "sql"))
        .count();
    assert_eq!(
        archived_migrations, 33,
        "QQBot pre-v1 archive must retain all 33 historical migrations"
    );
    let pre_v2_files = fs::read_dir(qqbot_database.join("archive/pre_v2"))
        .expect("QQBot pre-v2 migration archive must be readable")
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect::<BTreeSet<_>>();
    assert_eq!(
        pre_v2_files,
        BTreeSet::from([
            "20260803_qqbot_schema_v1.sql".into(),
            "20260804_qqbot_reply_reconcile.sql".into(),
            "20260805_qqbot_realtime_spool_recovery.sql".into(),
            "20260806_qqbot_artifact_reprocess.sql".into(),
            "20260806_qqbot_non_message_history_signals.sql".into(),
            "20260806_qqbot_notification_reconciliation_seed.sql".into(),
            "20260806_qqbot_thread_decision_revision_paging.sql".into(),
            "20260806_qqbot_thread_link_structured_references.sql".into(),
            "20260806_qqbot_thread_semantic_reconfirmation.sql".into(),
            "README.md".into(),
        ]),
        "QQBot pre-v2 archive must contain exactly Baseline v1 and its eight folded migrations"
    );
    let active_migrations = fs::read_dir(qqbot_database.join("migrations"))
        .expect("QQBot post-v2 migrations directory must be readable")
        .filter_map(Result::ok)
        .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "sql"))
        .count();
    assert_eq!(
        active_migrations, 0,
        "folded v1-to-v2 SQL must not remain in the active migrations directory"
    );
    assert!(
        qqbot_database.join("migrations/README.md").is_file(),
        "QQBot post-baseline migrations directory must remain application-owned"
    );
}

#[test]
fn qqbot_configuration_is_owned_by_the_qqbot_application() {
    // A1 后 config 已拆为目录，按目录递归收集。
    let config_source = rust_sources("qqbot-server/src/config");

    assert!(
        workspace_root()
            .join("qqbot-server/config/qqbot.example.toml")
            .is_file(),
        "QQBot TOML example must live under qqbot-server/config"
    );
    assert!(
        workspace_root()
            .join("qqbot-server/config/.env.example")
            .is_file(),
        "QQBot environment example must live under qqbot-server/config"
    );
    assert!(config_source.contains("QQBOT_CONFIG_PATH"));
    assert!(config_source.contains("QQBOT_DATABASE_URL"));
    assert!(!config_source.contains("dotenvy::dotenv()"));
    assert!(!config_source.contains("std::env::var(\"DATABASE_URL\")"));
    assert!(!config_source.contains("std::env::var_os(\"CONFIG_PATH\")"));
}

#[test]
fn qqbot_is_a_napcat_adapter_without_business_or_database_modules() {
    let crate_root = workspace_root().join("qqbot/src");
    let entries = fs::read_dir(&crate_root)
        .expect("qqbot source root must be readable")
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect::<BTreeSet<_>>();

    assert_eq!(entries, BTreeSet::from(["lib.rs".into(), "napcat".into()]));
    for required in [
        "api.rs",
        "capabilities.rs",
        "error.rs",
        "event.rs",
        "heartbeat.rs",
        "message_parser.rs",
        "mod.rs",
        "segments.rs",
    ] {
        assert!(
            crate_root.join("napcat").join(required).is_file(),
            "missing NapCat adapter file {required}"
        );
    }
    // listener 已拆分为 listener/ 目录（transport/dispatch/message_event/notice_event/bounds/mod）。
    let listener_dir = crate_root.join("napcat").join("listener");
    assert!(
        listener_dir.is_dir(),
        "missing NapCat listener/ directory (split from listener.rs)"
    );
    for required in [
        "bounds.rs",
        "dispatch.rs",
        "message_event.rs",
        "mod.rs",
        "notice_event.rs",
        "transport.rs",
    ] {
        assert!(
            listener_dir.join(required).is_file(),
            "missing NapCat listener/ module file {required}"
        );
    }

    let sources = rust_sources("qqbot/src");
    for forbidden in ["sea_orm", "sqlx", "repository", "DatabaseConnection"] {
        assert!(
            !sources.contains(forbidden),
            "QQBot adapter source still contains database/business marker {forbidden}"
        );
    }
    assert!(!workspace_root().join("database/sql/QQ_init.sql").exists());
}

#[test]
fn neutral_agent_core_does_not_pull_in_infrastructure_frameworks() {
    let agent = dependencies(&manifest("agent-core/Cargo.toml"));
    for forbidden in ["axum", "sea-orm", "sqlx", "qdrant-client", "reqwest"] {
        assert!(!agent.contains(forbidden));
    }
}
