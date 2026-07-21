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
fn workspace_has_exactly_the_five_intended_members() {
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
            "apps/server".to_owned(),
            "crates/agent-core".to_owned(),
            "crates/ai-core".to_owned(),
            "crates/digital-human".to_owned(),
            "crates/qqbot".to_owned(),
        ])
    );
}

#[test]
fn crate_dependency_directions_are_enforced() {
    let agent = dependencies(&manifest("crates/agent-core/Cargo.toml"));
    let ai = dependencies(&manifest("crates/ai-core/Cargo.toml"));
    let digital = dependencies(&manifest("crates/digital-human/Cargo.toml"));
    let qq = dependencies(&manifest("crates/qqbot/Cargo.toml"));
    let server = dependencies(&manifest("apps/server/Cargo.toml"));

    for business in ["digital-human", "qqbot"] {
        assert!(
            !agent.contains(business),
            "agent-core must not depend on {business}"
        );
        assert!(
            !ai.contains(business),
            "ai-core must not depend on {business}"
        );
    }
    assert!(digital.contains("agent-core"));
    assert!(digital.contains("ai-core"));
    assert!(!digital.contains("qqbot"));
    assert_eq!(
        qq.intersection(&BTreeSet::from([
            "ai-core".to_owned(),
            "agent-core".to_owned(),
            "digital-human".to_owned(),
        ]))
        .cloned()
        .collect::<BTreeSet<_>>(),
        BTreeSet::from(["ai-core".to_owned()])
    );
    for dependency in ["agent-core", "ai-core", "digital-human", "qqbot"] {
        assert!(
            server.contains(dependency),
            "server must depend on {dependency}"
        );
    }
}

#[test]
fn neutral_core_crates_do_not_pull_in_infrastructure_or_business_frameworks() {
    let agent = dependencies(&manifest("crates/agent-core/Cargo.toml"));
    let ai = dependencies(&manifest("crates/ai-core/Cargo.toml"));
    for forbidden in ["axum", "sea-orm", "sqlx", "qdrant-client", "reqwest"] {
        assert!(
            !agent.contains(forbidden),
            "agent-core must not depend on {forbidden}"
        );
        assert!(
            !ai.contains(forbidden),
            "ai-core must not depend on {forbidden}"
        );
    }
}

#[test]
fn business_crates_do_not_reference_each_other_even_in_source() {
    let digital = rust_sources("crates/digital-human/src");
    let qq = rust_sources("crates/qqbot/src");

    assert!(!digital.contains("qqbot::"));
    assert!(!digital.contains("qq_bot"));
    assert!(!qq.contains("digital_human::"));
}

#[test]
fn concrete_repository_modules_are_private_to_their_business_crates() {
    let digital_repo =
        fs::read_to_string(workspace_root().join("crates/digital-human/src/infra/repo/mod.rs"))
            .expect("digital-human repository module must be readable");
    let qq_repo =
        fs::read_to_string(workspace_root().join("crates/qqbot/src/infra/qq_bot/repo/mod.rs"))
            .expect("qqbot repository module must be readable");

    assert!(digital_repo.contains("pub(crate) mod seaorm_impl;"));
    assert!(qq_repo.contains("pub(crate) mod seaorm_impl;"));
    assert!(!digital_repo.contains("\npub mod seaorm_impl;"));
    assert!(!qq_repo.contains("\npub mod seaorm_impl;"));
}

#[test]
fn qq_entities_are_owned_only_by_qqbot() {
    let digital_entities = workspace_root().join("crates/digital-human/src/infra/repo/entities");
    let leaked = fs::read_dir(digital_entities)
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
