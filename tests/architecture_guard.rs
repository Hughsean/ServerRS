#[path = "../build_support/architecture_guard.rs"]
mod architecture_guard;

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use architecture_guard::{FeatureSet, check_workspace};

#[test]
fn allows_current_layer_dependency_directions() {
    let workspace = TestWorkspace::new("allows_current_layer_dependency_directions");
    workspace.write(
        "src/domain/user.rs",
        "use crate::shared::error::AppError;\npub struct User;\n",
    );
    workspace.write(
        "src/app/user_service.rs",
        "use crate::domain::user::User;\nuse crate::shared::error::AppError;\n",
    );
    workspace.write(
        "src/infra/user_repo.rs",
        "use crate::domain::user::User;\nuse crate::infra::db::entities;\nuse crate::shared::error::AppError;\n",
    );
    workspace.write(
        "src/api/user_handler.rs",
        "use crate::api::UserState;\nuse crate::app::user_service::UserService;\nuse crate::domain::user::User;\nuse crate::shared::error::AppError;\n",
    );
    workspace.write(
        "src/bootstrap/state.rs",
        "use crate::api::router;\nuse crate::app::user_service::UserService;\nuse crate::domain::user::User;\nuse crate::infra::user_repo::UserRepo;\nuse crate::shared::config::AppConfig;\n",
    );

    check_workspace(workspace.path(), FeatureSet::default()).expect("valid layer directions");
}

#[test]
fn rejects_business_layer_back_edges() {
    let workspace = TestWorkspace::new("rejects_business_layer_back_edges");
    workspace.write(
        "src/domain/user.rs",
        "use crate::app::user_service::UserService;\n",
    );
    workspace.write(
        "src/app/user_service.rs",
        "use crate::infra::db::seaorm_impl::user_repo::UserRepo;\n",
    );
    workspace.write(
        "src/infra/user_repo.rs",
        "use crate::app::user_service::UserService;\n",
    );
    workspace.write(
        "src/api/user_handler.rs",
        "use crate::bootstrap::state::ServiceGraph;\n",
    );

    let report = check_workspace(workspace.path(), FeatureSet::default())
        .expect_err("invalid layer directions must fail");

    assert_contains(&report.to_string(), "domain layer must not depend on app");
    assert_contains(&report.to_string(), "app layer must not depend on infra");
    assert_contains(&report.to_string(), "infra layer must not depend on app");
    assert_contains(
        &report.to_string(),
        "api layer must not depend on bootstrap",
    );
}

#[test]
fn rejects_api_and_bootstrap_regressions() {
    let workspace = TestWorkspace::new("rejects_api_and_bootstrap_regressions");
    workspace.write(
        "src/api/handlers/chat_handler.rs",
        "use axum::extract::State;\nuse crate::api::AppState;\nasync fn h(State(_state): State<AppState>) {}\n",
    );
    workspace.write(
        "src/api/state.rs",
        "use std::sync::Arc;\nuse crate::bootstrap::state::ServiceGraph;\npub struct AppState { services: Arc<ServiceGraph> }\n",
    );
    workspace.write(
        "src/bootstrap/state.rs",
        "use std::sync::Arc;\nfn build() { let service = Arc::new(UserService::new()); }\n",
    );
    workspace.write(
        "src/bootstrap/graph/mod.rs",
        "pub mod agent_context_provider;\npub use agent_context_provider::build_agent_context_builder;\n",
    );

    let report = check_workspace(workspace.path(), FeatureSet::default())
        .expect_err("bootstrap regressions must fail");

    assert_contains(
        &report.to_string(),
        "handlers must extract narrow Axum state",
    );
    assert_contains(&report.to_string(), "AppState must not wrap ServiceGraph");
    assert_contains(
        &report.to_string(),
        "ServiceGraph::build must not directly construct services",
    );
    assert_contains(
        &report.to_string(),
        "graph provider modules must stay private",
    );
    assert_contains(
        &report.to_string(),
        "graph child providers must not be re-exported",
    );
}

#[test]
fn skips_qq_bot_sources_unless_feature_is_enabled() {
    let workspace = TestWorkspace::new("skips_qq_bot_sources_unless_feature_is_enabled");
    workspace.write(
        "src/app/qq_bot/service.rs",
        "use crate::infra::qq_bot::napcat::api::NapCatApiClient;\n",
    );

    check_workspace(workspace.path(), FeatureSet { qq_bot: false })
        .expect("qq_bot sources are ignored when feature is disabled");

    let report = check_workspace(workspace.path(), FeatureSet { qq_bot: true })
        .expect_err("qq_bot sources are checked when feature is enabled");
    assert_contains(&report.to_string(), "app layer must not depend on infra");
}

#[test]
fn ignores_cfg_test_blocks_for_test_only_infra_fakes() {
    let workspace = TestWorkspace::new("ignores_cfg_test_blocks_for_test_only_infra_fakes");
    workspace.write(
        "src/app/memory_service.rs",
        r#"
use crate::domain::memory::MemoryRepoT;

#[cfg(test)]
mod tests {
    use crate::infra::llm::mock_provider::MockEmbeddingProvider;
}
"#,
    );

    check_workspace(workspace.path(), FeatureSet::default())
        .expect("test-only infra fakes are allowed inside cfg(test) blocks");
}

fn assert_contains(haystack: &str, needle: &str) {
    assert!(
        haystack.contains(needle),
        "expected report to contain {needle:?}, got:\n{haystack}"
    );
}

struct TestWorkspace {
    root: PathBuf,
}

impl TestWorkspace {
    fn new(name: &str) -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock must be valid")
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "server_rs_architecture_guard_{name}_{nonce}_{}",
            std::process::id()
        ));
        fs::create_dir_all(&root).expect("test workspace root");
        Self { root }
    }

    fn path(&self) -> &Path {
        &self.root
    }

    fn write(&self, relative_path: &str, contents: &str) {
        let path = self.root.join(relative_path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("test workspace parent");
        }
        fs::write(path, contents).expect("test workspace file");
    }
}

impl Drop for TestWorkspace {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}
