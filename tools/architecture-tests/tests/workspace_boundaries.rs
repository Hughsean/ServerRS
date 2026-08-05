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
fn workspace_has_exactly_the_ten_intended_members() {
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
            "apps/digital-human-server".to_owned(),
            "apps/qqbot-server".to_owned(),
            "crates/agent-core".to_owned(),
            "crates/ai-core".to_owned(),
            "crates/digital-human".to_owned(),
            "crates/personal-secretary".to_owned(),
            "crates/personal-secretary-mysql".to_owned(),
            "crates/qqbot".to_owned(),
            "crates/qq-open-platform".to_owned(),
            "tools/architecture-tests".to_owned(),
        ])
    );
}

#[test]
fn the_two_applications_have_independent_dependency_graphs() {
    let digital = dependencies(&manifest("crates/digital-human/Cargo.toml"));
    let personal_secretary = dependencies(&manifest("crates/personal-secretary/Cargo.toml"));
    let personal_secretary_mysql =
        dependencies(&manifest("crates/personal-secretary-mysql/Cargo.toml"));
    let qq = dependencies(&manifest("crates/qqbot/Cargo.toml"));
    let official_qq = dependencies(&manifest("crates/qq-open-platform/Cargo.toml"));
    let digital_server = dependencies(&manifest("apps/digital-human-server/Cargo.toml"));
    let qq_server = dependencies(&manifest("apps/qqbot-server/Cargo.toml"));

    assert!(digital.contains("agent-core"));
    assert!(digital.contains("ai-core"));
    assert!(!digital.contains("qqbot"));

    for forbidden in [
        "agent-core",
        "ai-core",
        "digital-human",
        "personal-secretary",
        "sea-orm",
        "sqlx",
    ] {
        assert!(
            !qq.contains(forbidden),
            "qqbot must not depend on {forbidden}"
        );
    }
    for forbidden in [
        "axum",
        "digital-human",
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
    for forbidden in [
        "axum",
        "digital-human",
        "qqbot",
        "qq-open-platform",
        "reqwest",
    ] {
        assert!(
            !personal_secretary_mysql.contains(forbidden),
            "personal-secretary-mysql must not depend on {forbidden}"
        );
    }
    for forbidden in [
        "agent-core",
        "ai-core",
        "digital-human",
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

    for required in ["agent-core", "ai-core", "digital-human"] {
        assert!(
            digital_server.contains(required),
            "digital-human-server must depend on {required}"
        );
    }
    assert!(!digital_server.contains("qqbot"));
    assert!(!digital_server.contains("personal-secretary"));
    assert!(qq_server.contains("qqbot"));
    assert!(qq_server.contains("personal-secretary"));
    assert!(qq_server.contains("personal-secretary-mysql"));
    assert!(qq_server.contains("qq-open-platform"));
    assert!(!qq_server.contains("digital-human"));
    assert!(!digital_server.contains("qq-open-platform"));
    assert!(!digital.contains("qq-open-platform"));
    assert!(!personal_secretary.contains("qq-open-platform"));
    assert!(!qq.contains("qq-open-platform"));
}

#[test]
fn qq_open_platform_is_a_protocol_adapter_without_business_or_database_code() {
    let sources = rust_sources("crates/qq-open-platform/src");
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
    let sources = rust_sources("crates/personal-secretary/src");
    for forbidden in [
        "qqbot::",
        "napcat::",
        "qq_open_platform::",
        "digital_human::",
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
    let sources = rust_sources("apps/qqbot-server/src/application");
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
        workspace_root().join("apps/qqbot-server/src/infrastructure/realtime_spool.rs"),
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
    let sources = rust_sources("crates/personal-secretary/src");
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
    let domain = fs::read_to_string(root.join("crates/personal-secretary/src/domain/backfill.rs"))
        .expect("backfill domain source must be readable");
    let application = fs::read_to_string(
        root.join("crates/personal-secretary/src/application/backfill_service.rs"),
    )
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
    let api = fs::read_to_string(root.join("crates/qqbot/src/napcat/api.rs"))
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
        fs::read_to_string(root.join("apps/qqbot-server/src/adapters/napcat_history_source.rs"))
            .expect("NapCat history adapter source must be readable");
    assert!(adapter.contains("client: Arc<dyn NapCatHistoryReadT>"));
    assert!(!adapter.contains("client: Arc<NapCatReadOnlyClient>"));
    assert!(!adapter.contains("reverse_order"));
    assert!(adapter.contains("HistoryReadDirection::TowardOlder"));
}

#[test]
fn qqbot_does_not_depend_on_personal_secretary_or_database() {
    let qq_manifest = manifest("crates/qqbot/Cargo.toml");
    let deps = dependencies(&qq_manifest);
    for forbidden in ["personal-secretary", "sea-orm", "sqlx"] {
        assert!(
            !deps.contains(forbidden),
            "qqbot must not depend on {forbidden}"
        );
    }
    let sources = rust_sources("crates/qqbot/src");
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
        "apps/qqbot-server/src/adapters/napcat_directory.rs",
        "apps/qqbot-server/src/adapters/napcat_history_source.rs",
        "apps/qqbot-server/src/bootstrap/thread_pipeline.rs",
        "apps/qqbot-server/src/runtime/connection_loop.rs",
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
        fs::read_to_string(workspace_root().join("apps/qqbot-server/src/infrastructure/llm.rs"))
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
    // A2 后 runtime 已拆为目录，按目录递归收集。
    let runtime = rust_sources("apps/qqbot-server/src/runtime");
    let worker = fs::read_to_string(
        workspace_root().join("apps/qqbot-server/src/application/ingestion_worker.rs"),
    )
    .expect("qqbot-server ingestion worker must be readable");

    assert!(runtime.contains("try_enqueue"));
    assert!(runtime.contains("spawn_ingestion_worker"));
    assert!(!runtime.contains("insert_message_if_absent"));
    assert!(worker.contains("mpsc::channel"));
    assert!(worker.contains("insert_message_if_absent"));
}

#[test]
fn backfill_worker_is_decoupled_from_realtime_websocket() {
    let runtime = rust_sources("apps/qqbot-server/src/runtime");
    let backfill_worker = fs::read_to_string(
        workspace_root().join("apps/qqbot-server/src/application/backfill/worker.rs"),
    )
    .expect("qqbot-server backfill worker must be readable");
    let napcat_history_source = fs::read_to_string(
        workspace_root().join("apps/qqbot-server/src/adapters/napcat_history_source.rs"),
    )
    .expect("qqbot-server napcat history source must be readable");

    // A2 后回补装配下沉到 bootstrap，与实时 WebSocket 接收解耦。
    // runtime + bootstrap 合并检查仍包含回补装配与并发/取消语义。
    let bootstrap = rust_sources("apps/qqbot-server/src/bootstrap");
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
    let api = fs::read_to_string(workspace_root().join("crates/qqbot/src/napcat/api.rs"))
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
    let napcat_mod = fs::read_to_string(workspace_root().join("crates/qqbot/src/napcat/mod.rs"))
        .expect("NapCat module source must be readable");
    let api = fs::read_to_string(workspace_root().join("crates/qqbot/src/napcat/api.rs"))
        .expect("NapCat API source must be readable");
    let qqbot_sources = rust_sources("crates/qqbot/src");
    let server_sources = rust_sources("apps/qqbot-server/src");

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
    let directory =
        fs::read_to_string(root.join("apps/qqbot-server/src/adapters/napcat_directory.rs"))
            .expect("NapCat directory adapter must be readable");
    let history =
        fs::read_to_string(root.join("apps/qqbot-server/src/adapters/napcat_history_source.rs"))
            .expect("NapCat history adapter must be readable");
    let capabilities = fs::read_to_string(root.join("crates/qqbot/src/napcat/capabilities.rs"))
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
    let digital_init = fs::read_to_string(workspace_root().join("database/sql/init.sql"))
        .expect("digital-human init.sql must be readable");
    assert!(!digital_init.contains("secretary_"));
    assert!(!digital_init.contains("personal_secretary"));

    let digital_migrations = workspace_root().join("database/sql/migrations");
    let leaked = fs::read_dir(digital_migrations)
        .expect("digital-human migrations must be readable")
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.contains("secretary") || name.contains("qqbot"))
        .collect::<Vec<_>>();
    assert!(
        leaked.is_empty(),
        "QQBot migrations leaked into Digital Human database: {leaked:?}"
    );

    let qqbot_database = workspace_root().join("apps/qqbot-server/database");
    let baseline = qqbot_database.join("baseline/20260803_qqbot_schema_v1.sql");
    assert!(baseline.is_file(), "QQBot Schema Baseline v1 is missing");
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
    assert!(
        qqbot_database.join("migrations/README.md").is_file(),
        "QQBot post-baseline migrations directory must remain application-owned"
    );
}

#[test]
fn qqbot_configuration_is_owned_by_the_qqbot_application() {
    // A1 后 config 已拆为目录，按目录递归收集。
    let config_source = rust_sources("apps/qqbot-server/src/config");

    assert!(
        workspace_root()
            .join("apps/qqbot-server/config/qqbot.example.toml")
            .is_file(),
        "QQBot TOML example must live under apps/qqbot-server/config"
    );
    assert!(
        workspace_root()
            .join("apps/qqbot-server/config/.env.example")
            .is_file(),
        "QQBot environment example must live under apps/qqbot-server/config"
    );
    assert!(config_source.contains("QQBOT_CONFIG_PATH"));
    assert!(config_source.contains("QQBOT_DATABASE_URL"));
    assert!(!config_source.contains("dotenvy::dotenv()"));
    assert!(!config_source.contains("std::env::var(\"DATABASE_URL\")"));
    assert!(!config_source.contains("std::env::var_os(\"CONFIG_PATH\")"));
}

#[test]
fn qqbot_is_a_napcat_adapter_without_business_or_database_modules() {
    let crate_root = workspace_root().join("crates/qqbot/src");
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

    let sources = rust_sources("crates/qqbot/src");
    for forbidden in ["sea_orm", "sqlx", "repository", "DatabaseConnection"] {
        assert!(
            !sources.contains(forbidden),
            "QQBot adapter source still contains database/business marker {forbidden}"
        );
    }
    assert!(!workspace_root().join("database/sql/QQ_init.sql").exists());
}

#[test]
fn qqbot_is_not_a_feature_of_the_digital_human_server() {
    let manifest =
        fs::read_to_string(workspace_root().join("apps/digital-human-server/Cargo.toml"))
            .expect("digital-human-server manifest must be readable");
    let source = rust_sources("apps/digital-human-server/src");

    assert!(!manifest.contains("qq_bot"));
    assert!(!manifest.contains("qqbot"));
    assert!(!source.contains("qqbot::"));
    assert!(!source.contains("feature = \"qq_bot\""));
}

#[test]
fn neutral_core_crates_do_not_pull_in_infrastructure_or_business_frameworks() {
    let agent = dependencies(&manifest("crates/agent-core/Cargo.toml"));
    let ai = dependencies(&manifest("crates/ai-core/Cargo.toml"));
    for forbidden in ["axum", "sea-orm", "sqlx", "qdrant-client", "reqwest"] {
        assert!(!agent.contains(forbidden));
        assert!(!ai.contains(forbidden));
    }
}

#[test]
fn digital_human_repository_implementation_stays_private() {
    let repository =
        fs::read_to_string(workspace_root().join("crates/digital-human/src/infra/repo/mod.rs"))
            .expect("digital-human repository module must be readable");

    assert!(repository.contains("pub(crate) mod seaorm_impl;"));
    assert!(!repository.contains("\npub mod seaorm_impl;"));
}

#[test]
fn no_qq_entities_remain_in_digital_human() {
    let entities = workspace_root().join("crates/digital-human/src/infra/repo/entities");
    let leaked = fs::read_dir(entities)
        .expect("digital-human entities directory must be readable")
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with("qq_"))
        .collect::<Vec<_>>();

    assert!(
        leaked.is_empty(),
        "QQ entities leaked into digital-human: {leaked:?}"
    );
}
