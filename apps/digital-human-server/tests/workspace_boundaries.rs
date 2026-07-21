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
fn workspace_has_exactly_the_six_intended_members() {
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
            "crates/qqbot".to_owned(),
        ])
    );
}

#[test]
fn the_two_applications_have_independent_dependency_graphs() {
    let digital = dependencies(&manifest("crates/digital-human/Cargo.toml"));
    let qq = dependencies(&manifest("crates/qqbot/Cargo.toml"));
    let digital_server = dependencies(&manifest("apps/digital-human-server/Cargo.toml"));
    let qq_server = dependencies(&manifest("apps/qqbot-server/Cargo.toml"));

    assert!(digital.contains("agent-core"));
    assert!(digital.contains("ai-core"));
    assert!(!digital.contains("qqbot"));

    for forbidden in ["agent-core", "ai-core", "digital-human", "sea-orm", "sqlx"] {
        assert!(
            !qq.contains(forbidden),
            "qqbot must not depend on {forbidden}"
        );
    }

    for required in ["agent-core", "ai-core", "digital-human"] {
        assert!(
            digital_server.contains(required),
            "digital-human-server must depend on {required}"
        );
    }
    assert!(!digital_server.contains("qqbot"));
    assert!(qq_server.contains("qqbot"));
    assert!(!qq_server.contains("digital-human"));
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
