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
fn workspace_has_exactly_the_eight_intended_members() {
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
            "crates/qqbot".to_owned(),
            "crates/qq-open-platform".to_owned(),
        ])
    );
}

#[test]
fn the_two_applications_have_independent_dependency_graphs() {
    let digital = dependencies(&manifest("crates/digital-human/Cargo.toml"));
    let personal_secretary = dependencies(&manifest("crates/personal-secretary/Cargo.toml"));
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
    for forbidden in ["axum", "digital-human", "qqbot"] {
        assert!(
            !personal_secretary.contains(forbidden),
            "personal-secretary must not depend on {forbidden}"
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
fn personal_secretary_domain_is_qq_protocol_neutral() {
    // 只检查领域层和应用层文件（排除 infra 仓储实现，它合理使用数据库/SeaORM）。
    let domain_root = workspace_root().join("crates/personal-secretary/src");
    let domain_files = [
        "agent_runtime.rs",
        "backfill.rs",
        "backfill_service.rs",
        "continuity.rs",
        "follow_up.rs",
        "follow_up_service.rs",
        "inbound.rs",
        "memory.rs",
        "memory_service.rs",
        "store.rs",
        "thread_service.rs",
        "thread_semantic_service.rs",
        "thread_semantics.rs",
        "thread_link_service.rs",
        "thread_links.rs",
        "thread_mutations.rs",
        "threading.rs",
        "lib.rs",
    ];
    let sources = domain_files
        .iter()
        .map(|name| {
            fs::read_to_string(domain_root.join(name))
                .unwrap_or_else(|error| panic!("failed to read {name}: {error}"))
        })
        .collect::<Vec<_>>()
        .join("\n");

    for forbidden in [
        "qqbot::",
        "napcat::",
        "qq_open_platform::",
        "digital_human::",
        "reqwest",
        "tokio::spawn",
        "DatabaseConnection",
        "sea_orm",
    ] {
        assert!(
            !sources.contains(forbidden),
            "personal-secretary domain/app source contains concrete QQ/Digital Human marker {forbidden}"
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
        "NapCatApiClient",
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
fn qqbot_server_does_not_send_through_the_local_qq_account() {
    let sources = rust_sources("apps/qqbot-server/src");
    for forbidden in ["send_group_msg", "send_private_msg"] {
        assert!(
            !sources.contains(forbidden),
            "qqbot-server must not call NapCat mutation {forbidden}"
        );
    }
}

#[test]
fn napcat_callback_does_not_wait_for_mysql() {
    let runtime = fs::read_to_string(workspace_root().join("apps/qqbot-server/src/runtime.rs"))
        .expect("qqbot-server runtime must be readable");
    let worker =
        fs::read_to_string(workspace_root().join("apps/qqbot-server/src/ingestion_worker.rs"))
            .expect("qqbot-server ingestion worker must be readable");

    assert!(runtime.contains("try_enqueue"));
    assert!(runtime.contains("spawn_ingestion_worker"));
    assert!(!runtime.contains("insert_message_if_absent"));
    assert!(worker.contains("mpsc::channel"));
    assert!(worker.contains("insert_message_if_absent"));
}

#[test]
fn backfill_worker_is_decoupled_from_realtime_websocket() {
    let runtime = fs::read_to_string(workspace_root().join("apps/qqbot-server/src/runtime.rs"))
        .expect("qqbot-server runtime must be readable");
    let backfill_worker =
        fs::read_to_string(workspace_root().join("apps/qqbot-server/src/backfill/worker.rs"))
            .expect("qqbot-server backfill worker must be readable");
    let napcat_history_source = fs::read_to_string(
        workspace_root().join("apps/qqbot-server/src/backfill/napcat_history_source.rs"),
    )
    .expect("qqbot-server napcat history source must be readable");

    // 回补独立装配在 runtime 中，与实时 WebSocket 接收解耦。
    assert!(runtime.contains("spawn_backfill_worker"));
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
        "send_group_msg",
        "send_private_msg",
        "group_poke",
        "friend_poke",
        "delete_msg",
    ] {
        assert!(
            !api.contains(forbidden),
            "NapCat personal-account adapter must not expose mutation {forbidden}"
        );
    }
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

    assert!(
        workspace_root()
            .join("apps/qqbot-server/database/migrations/20260723_personal_secretary_ingestion.sql")
            .is_file(),
        "QQBot migration must live under apps/qqbot-server/database"
    );
    assert!(
        workspace_root()
            .join(
                "apps/qqbot-server/database/migrations/20260723_personal_secretary_continuity.sql"
            )
            .is_file(),
        "QQBot continuity migration must live under apps/qqbot-server/database"
    );
    assert!(
        workspace_root()
            .join("apps/qqbot-server/database/migrations/20260723_personal_secretary_backfill.sql")
            .is_file(),
        "QQBot backfill migration must live under apps/qqbot-server/database"
    );
    assert!(
        workspace_root()
            .join("apps/qqbot-server/database/migrations/20260724_personal_secretary_threads.sql")
            .is_file(),
        "QQBot thread migration must live under apps/qqbot-server/database"
    );
    assert!(
        workspace_root()
            .join(
                "apps/qqbot-server/database/migrations/20260724_personal_secretary_thread_semantics.sql"
            )
            .is_file(),
        "QQBot thread semantics migration must live under apps/qqbot-server/database"
    );
    assert!(
        workspace_root()
            .join(
                "apps/qqbot-server/database/migrations/20260724_personal_secretary_thread_links.sql"
            )
            .is_file(),
        "QQBot thread links migration must live under apps/qqbot-server/database"
    );
    for migration in [
        "20260724_personal_secretary_memory.sql",
        "20260724_personal_secretary_memory_controls_followups.sql",
        "20260724_personal_secretary_qq_open_platform.sql",
    ] {
        assert!(
            workspace_root()
                .join("apps/qqbot-server/database/migrations")
                .join(migration)
                .is_file(),
            "QQBot-owned migration is missing: {migration}"
        );
    }
}

#[test]
fn qqbot_configuration_is_owned_by_the_qqbot_application() {
    let config_source =
        fs::read_to_string(workspace_root().join("apps/qqbot-server/src/config.rs"))
            .expect("QQBot config source must be readable");

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
        "error.rs",
        "event.rs",
        "listener.rs",
        "message_parser.rs",
        "mod.rs",
    ] {
        assert!(
            crate_root.join("napcat").join(required).is_file(),
            "missing NapCat adapter file {required}"
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
