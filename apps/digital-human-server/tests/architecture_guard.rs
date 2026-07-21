#[path = "../build_support/architecture_guard.rs"]
mod architecture_guard;

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use architecture_guard::{FeatureSet, check_workspace};
use syn::visit::Visit;

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
fn rejects_grouped_import_layer_back_edges() {
    let workspace = TestWorkspace::new("rejects_grouped_import_layer_back_edges");
    workspace.write(
        "src/app/user_service.rs",
        "use crate::{domain::user::User, infra::db::seaorm_impl::user_repo::UserRepo};\n",
    );
    workspace.write(
        "src/domain/user.rs",
        "use crate::{\n    shared::error::AppError,\n    infra::db::seaorm_impl::user_repo::UserRepo,\n};\n",
    );

    let report = check_workspace(workspace.path(), FeatureSet::default())
        .expect_err("grouped imports must not bypass layer checks");

    assert_contains(&report.to_string(), "app layer must not depend on infra");
    assert_contains(&report.to_string(), "domain layer must not depend on infra");
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
        "use std::sync::Arc;\npub struct ServiceGraph;\nimpl ServiceGraph { pub fn build() { let service = Arc::new(UserService::new()); } }\n",
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
fn rejects_business_layer_imports_of_infra_only_external_crates() {
    let workspace =
        TestWorkspace::new("rejects_business_layer_imports_of_infra_only_external_crates");
    workspace.write("src/app/cache_service.rs", "use redis::Client;\n");
    workspace.write(
        "src/domain/search.rs",
        "use qdrant_client::qdrant::SearchPoints;\n",
    );

    let report = check_workspace(workspace.path(), FeatureSet::default())
        .expect_err("business layers must not import infra-only external crates");

    assert_contains(
        &report.to_string(),
        "business layers must not import infrastructure-only crates",
    );
}

#[test]
fn rejects_business_layer_http_client_adapter_crates() {
    let workspace = TestWorkspace::new("rejects_business_layer_http_client_adapter_crates");
    workspace.write(
        "src/app/agent/http_tool.rs",
        "use reqwest::Client;\npub struct Tool { client: Client }\n",
    );
    workspace.write(
        "src/shared/config/url.rs",
        "fn parse(raw: &str) { let _ = reqwest::Url::parse(raw); }\n",
    );

    let report = check_workspace(workspace.path(), FeatureSet::default())
        .expect_err("business layers must not import HTTP client adapter crates");

    assert_contains(
        &report.to_string(),
        "business layers must not import infrastructure-only crates",
    );
    assert_contains(&report.to_string(), "http client infrastructure");
}

#[test]
fn rejects_api_repository_ports_in_imports_and_state_types() {
    let workspace = TestWorkspace::new("rejects_api_repository_ports_in_imports_and_state_types");
    workspace.write(
        "src/api/state.rs",
        "use std::sync::Arc;\nuse crate::domain::risk::RiskRepoT;\npub struct AdminState { risk: Arc<dyn RiskRepoT> }\n",
    );

    let report = check_workspace(workspace.path(), FeatureSet::default())
        .expect_err("API repository ports must be rejected");

    assert_contains(
        &report.to_string(),
        "api layer must not import repository ports",
    );
    assert_contains(
        &report.to_string(),
        "api layer must not hold repository ports",
    );
}

#[test]
fn rejects_api_state_domain_ports_regardless_of_type_name_or_alias() {
    let workspace =
        TestWorkspace::new("rejects_api_state_domain_ports_regardless_of_type_name_or_alias");
    workspace.write(
        "src/api/state.rs",
        r#"
use std::sync::Arc;
use crate::domain::conversation::ConversationGateway as ChatStorage;

pub struct ChatState {
    storage: Arc<dyn ChatStorage>,
    direct: Arc<dyn crate::domain::memory::MemoryStore>,
}
"#,
    );

    let report = check_workspace(workspace.path(), FeatureSet::default())
        .expect_err("API State must not depend on renamed or aliased domain ports");

    assert_contains(
        &report.to_string(),
        "API State must depend on application services, not domain ports",
    );
}

#[test]
fn allows_the_explicit_api_state_security_port() {
    let workspace = TestWorkspace::new("allows_the_explicit_api_state_security_port");
    workspace.write(
        "src/api/state.rs",
        r#"
use std::sync::Arc;
use crate::domain::auth::token_service::TokenServiceT;

pub struct SignatureState {
    token_service: Arc<dyn TokenServiceT>,
}
"#,
    );

    check_workspace(workspace.path(), FeatureSet::default())
        .expect("the approved API security port must remain allowed");
}

#[test]
fn rejects_adapter_vocabulary_outside_infrastructure() {
    let workspace = TestWorkspace::new("rejects_adapter_vocabulary_outside_infrastructure");
    workspace.write("src/domain/vector.rs", "pub struct QdrantLocation;\n");
    workspace.write(
        "src/app/qdrant_activation_service.rs",
        "pub struct QdrantActivationService;\n",
    );

    let report = check_workspace(workspace.path(), FeatureSet::default())
        .expect_err("adapter vocabulary must be rejected outside infrastructure");

    assert_contains(
        &report.to_string(),
        "domain layer must not expose adapter-specific names",
    );
    assert_contains(
        &report.to_string(),
        "app filenames must not expose adapter-specific names",
    );
    assert_contains(
        &report.to_string(),
        "app public APIs must not expose adapter-specific names",
    );
}

#[test]
fn classifies_cli_as_an_explicit_client_adapter() {
    let workspace = TestWorkspace::new("classifies_cli_as_an_explicit_client_adapter");
    workspace.write(
        "src/cli/client.rs",
        "use reqwest::Client;\npub struct ApiClient(Client);\n",
    );
    workspace.write(
        "src/bin/cli.rs",
        "use crate::app::chat_service::ChatService;\n",
    );

    let report = check_workspace(workspace.path(), FeatureSet::default())
        .expect_err("CLI must not bypass backend layers");

    assert_contains(
        &report.to_string(),
        "client_adapter layer must not depend on app",
    );
}

#[test]
fn rejects_inline_business_layer_infra_only_external_crate_references() {
    let workspace =
        TestWorkspace::new("rejects_inline_business_layer_infra_only_external_crate_references");
    workspace.write(
        "src/shared/config/url.rs",
        "fn parse(raw: &str) { let _ = reqwest::Url::parse(raw); }\n",
    );

    let report = check_workspace(workspace.path(), FeatureSet::default())
        .expect_err("inline external crate references must not bypass checks");

    assert_contains(&report.to_string(), "http client infrastructure");
}

#[test]
fn rejects_service_graph_build_construction_after_file_move() {
    let workspace = TestWorkspace::new("rejects_service_graph_build_construction_after_file_move");
    workspace.write(
        "src/bootstrap/service_graph.rs",
        "use std::sync::Arc;\npub struct ServiceGraph;\nimpl ServiceGraph { pub fn build() { let service = Arc::new(UserService::new()); } }\n",
    );

    let report = check_workspace(workspace.path(), FeatureSet::default())
        .expect_err("ServiceGraph::build checks must follow the type, not a fixed file path");

    assert_contains(
        &report.to_string(),
        "ServiceGraph::build must not directly construct services",
    );
}

#[test]
fn skips_declared_feature_sources_by_convention() {
    let workspace = TestWorkspace::new("skips_declared_feature_sources_by_convention");
    workspace.write(
        "src/app/discord_bot/service.rs",
        "use crate::infra::discord_bot::client::DiscordClient;\n",
    );

    check_workspace(
        workspace.path(),
        FeatureSet::new(["discord_bot"], std::iter::empty::<&str>()),
    )
    .expect("disabled feature sources are ignored by feature-name convention");

    let report = check_workspace(
        workspace.path(),
        FeatureSet::new(["discord_bot"], ["discord_bot"]),
    )
    .expect_err("enabled feature sources are checked");
    assert_contains(&report.to_string(), "app layer must not depend on infra");
}

#[test]
fn rejects_app_state_extraction_anywhere_in_api_layer() {
    let workspace = TestWorkspace::new("rejects_app_state_extraction_anywhere_in_api_layer");
    workspace.write(
        "src/api/http/chat_handler.rs",
        "use axum::extract::State;\nuse crate::api::AppState;\nasync fn h(State(_state): State<AppState>) {}\n",
    );

    let report = check_workspace(workspace.path(), FeatureSet::default())
        .expect_err("all api handlers must use narrow state regardless of directory layout");
    assert_contains(
        &report.to_string(),
        "handlers must extract narrow Axum state",
    );
}

#[test]
fn rejects_new_graph_child_provider_reexports_by_convention() {
    let workspace = TestWorkspace::new("rejects_new_graph_child_provider_reexports_by_convention");
    workspace.write(
        "src/bootstrap/graph/mod.rs",
        "mod new_detail_provider;\npub use new_detail_provider::build_new_detail;\n",
    );

    let report = check_workspace(workspace.path(), FeatureSet::default())
        .expect_err("new child providers must be blocked without updating a hard-coded list");

    assert_contains(
        &report.to_string(),
        "graph child providers must not be re-exported",
    );
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

#[test]
fn checkpoint_model_fields_exclude_runtime_and_infrastructure_handles() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../crates/agent-core/src/graph/checkpoint.rs");
    let source = fs::read_to_string(&path).expect("checkpoint source must be readable");
    let syntax = syn::parse_file(&source).expect("checkpoint source must parse");
    let checkpoint = syntax
        .items
        .iter()
        .find_map(|item| match item {
            syn::Item::Struct(item) if item.ident == "AgentCheckpoint" => Some(item),
            _ => None,
        })
        .expect("AgentCheckpoint struct must exist");
    let mut collector = TypePathCollector::default();
    for field in &checkpoint.fields {
        collector.visit_type(&field.ty);
    }

    for forbidden in [
        "Arc",
        "CancellationToken",
        "Instant",
        "CheckpointStore",
        "EffectExecutor",
        "Provider",
        "Repository",
        "Connection",
        "Deadline",
        "Future",
    ] {
        assert!(
            !collector
                .segments
                .iter()
                .any(|segment| segment == forbidden),
            "AgentCheckpoint field types must not contain {forbidden}: {:?}",
            collector.segments
        );
    }
}

#[test]
fn agent_runtime_facade_does_not_construct_graph_dependencies() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../crates/digital-human/src/app/agent/agent_runtime.rs");
    let source = fs::read_to_string(&path).expect("AgentRuntime source must be readable");

    assert!(
        source.contains(".chat_graph") && source.contains(".run(state)"),
        "AgentRuntime must delegate each turn to the prebuilt ChatAgentGraph"
    );
    for forbidden in [
        "pub struct AgentRuntimeSettings",
        "ChatAgentGraphDeps",
        "DefaultChatContextProvider::new",
        "ConversationTurnWriter::new",
        "AsyncMemoryExtractionScheduler::new",
        "extract_and_save_at_version",
        "tokio::spawn",
        "last_extraction_failure",
        "spawn_memory_extraction",
    ] {
        assert!(
            !source.contains(forbidden),
            "AgentRuntime facade must not construct or orchestrate dependency `{forbidden}`"
        );
    }

    let provider_path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("src/bootstrap/graph/agent_runtime_provider.rs");
    let provider = fs::read_to_string(&provider_path).expect("bootstrap provider must be readable");
    for required in [
        "ChatAgentGraph::new",
        "DefaultChatContextProvider::new",
        "ConversationTurnWriter::new",
        "AsyncMemoryExtractionScheduler::new",
        "AgentRuntime::from_graph",
    ] {
        assert!(
            provider.contains(required),
            "bootstrap provider must own graph dependency construction token `{required}`"
        );
    }
}

#[derive(Default)]
struct TypePathCollector {
    segments: Vec<String>,
}

impl<'ast> Visit<'ast> for TypePathCollector {
    fn visit_path_segment(&mut self, segment: &'ast syn::PathSegment) {
        self.segments.push(segment.ident.to_string());
        syn::visit::visit_path_segment(self, segment);
    }
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
            "digital_human_server_architecture_guard_{name}_{nonce}_{}",
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
