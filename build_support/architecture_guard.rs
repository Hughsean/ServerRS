use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, Default)]
pub struct FeatureSet {
    pub qq_bot: bool,
}

impl FeatureSet {
    #[allow(dead_code)]
    pub fn from_env() -> Self {
        Self {
            qq_bot: std::env::var_os("CARGO_FEATURE_QQ_BOT").is_some(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchitectureViolation {
    pub path: String,
    pub line: usize,
    pub rule: String,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchitectureReport {
    pub violations: Vec<ArchitectureViolation>,
}

impl fmt::Display for ArchitectureReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "architecture layering violations:")?;
        for violation in &self.violations {
            writeln!(
                f,
                "- {}:{}: {} ({})",
                violation.path, violation.line, violation.rule, violation.detail
            )?;
        }
        Ok(())
    }
}

#[allow(dead_code)]
pub fn emit_rerun_directives() {
    println!("cargo::rerun-if-changed=build.rs");
    println!("cargo::rerun-if-changed=build_support/architecture_guard.rs");
    println!("cargo::rerun-if-changed=src");
}

pub fn check_workspace(
    manifest_dir: &Path,
    features: FeatureSet,
) -> Result<(), ArchitectureReport> {
    let mut violations = Vec::new();

    for file in rust_source_files(manifest_dir) {
        let relative_path = normalize_relative_path(manifest_dir, &file);
        if should_skip_path(&relative_path, features) {
            continue;
        }

        let Ok(source) = fs::read_to_string(&file) else {
            continue;
        };
        let sanitized_lines = sanitized_source_lines(&source);

        check_layer_dependencies(&relative_path, &sanitized_lines, &mut violations);
        check_database_infrastructure_boundaries(&relative_path, &sanitized_lines, &mut violations);
        check_api_state_boundaries(&relative_path, &sanitized_lines, &mut violations);
        check_bootstrap_boundaries(&relative_path, &sanitized_lines, &mut violations);
        check_global_container_patterns(&relative_path, &sanitized_lines, &mut violations);
    }

    if violations.is_empty() {
        Ok(())
    } else {
        Err(ArchitectureReport { violations })
    }
}

const CHILD_PROVIDER_MODULES: &[&str] = &[
    "agent_context_provider",
    "agent_runtime_provider",
    "agent_tool_provider",
    "content_provider",
    "fresh_context_provider",
    "memory_extractor_provider",
    "memory_service_provider",
    "object_provider",
    "qq_bot_provider",
    "rag_ingestion_provider",
    "rag_retrieval_provider",
    "risk_audit_provider",
    "risk_detection_provider",
    "summary_handler_provider",
    "summary_service_provider",
    "web_ingestion_provider",
    "wellbeing_provider",
];

fn rust_source_files(manifest_dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let src = manifest_dir.join("src");
    collect_rs_files(&src, &mut files);
    files
}

fn collect_rs_files(path: &Path, files: &mut Vec<PathBuf>) {
    let Ok(metadata) = fs::metadata(path) else {
        return;
    };

    if metadata.is_file() {
        if path.extension().is_some_and(|extension| extension == "rs") {
            files.push(path.to_path_buf());
        }
        return;
    }

    let Ok(entries) = fs::read_dir(path) else {
        return;
    };

    for entry in entries.flatten() {
        collect_rs_files(&entry.path(), files);
    }
}

fn normalize_relative_path(manifest_dir: &Path, path: &Path) -> String {
    path.strip_prefix(manifest_dir)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

fn should_skip_path(relative_path: &str, features: FeatureSet) -> bool {
    if relative_path.starts_with("src/test/") {
        return true;
    }

    !features.qq_bot
        && (relative_path.contains("/qq_bot/")
            || relative_path.ends_with("/qq_bot.rs")
            || relative_path.ends_with("src/bin/qq_bot_init.rs"))
}

fn sanitized_source_lines(source: &str) -> Vec<(usize, String)> {
    let lines = source.lines().collect::<Vec<_>>();
    let mut sanitized = Vec::new();
    let mut index = 0;

    while index < lines.len() {
        let line = lines[index];
        if is_cfg_test_attr(line) {
            index = skip_cfg_test_item(&lines, index);
            continue;
        }

        sanitized.push((index + 1, strip_line_comment(line).to_string()));
        index += 1;
    }

    sanitized
}

fn is_cfg_test_attr(line: &str) -> bool {
    let compact = line.split_whitespace().collect::<String>();
    compact == "#[cfg(test)]"
}

fn skip_cfg_test_item(lines: &[&str], attr_index: usize) -> usize {
    let mut index = attr_index + 1;

    while index < lines.len() && lines[index].trim_start().starts_with("#[") {
        index += 1;
    }

    let mut depth = 0i32;
    let mut started_block = false;

    while index < lines.len() {
        let line = strip_line_comment(lines[index]);

        for ch in line.chars() {
            match ch {
                '{' => {
                    depth += 1;
                    started_block = true;
                }
                '}' => {
                    depth -= 1;
                }
                _ => {}
            }
        }

        index += 1;

        if started_block && depth <= 0 {
            break;
        }
        if !started_block && line.trim_end().ends_with(';') {
            break;
        }
    }

    index
}

fn strip_line_comment(line: &str) -> &str {
    line.split_once("//")
        .map(|(before_comment, _)| before_comment)
        .unwrap_or(line)
}

fn check_layer_dependencies(
    relative_path: &str,
    lines: &[(usize, String)],
    violations: &mut Vec<ArchitectureViolation>,
) {
    let Some(layer) = layer_for_path(relative_path) else {
        return;
    };

    for (line_no, line) in lines {
        for forbidden in forbidden_layers(layer) {
            if references_layer(line, forbidden) {
                violations.push(ArchitectureViolation {
                    path: relative_path.to_string(),
                    line: *line_no,
                    rule: format!("{layer} layer must not depend on {forbidden}"),
                    detail: line.trim().to_string(),
                });
            }
        }
    }
}

fn check_database_infrastructure_boundaries(
    relative_path: &str,
    lines: &[(usize, String)],
    violations: &mut Vec<ArchitectureViolation>,
) {
    if !matches!(
        layer_for_path(relative_path),
        Some("api" | "app" | "domain" | "shared")
    ) {
        return;
    }

    for (line_no, line) in lines {
        if line.contains("sea_orm::")
            || line.contains("sqlx::")
            || line.contains("DatabaseConnection")
            || line.contains("crate::infra::db::entities")
        {
            violations.push(ArchitectureViolation {
                path: relative_path.to_string(),
                line: *line_no,
                rule: "business layers must not import database infrastructure".into(),
                detail: line.trim().to_string(),
            });
        }
    }
}

fn check_api_state_boundaries(
    relative_path: &str,
    lines: &[(usize, String)],
    violations: &mut Vec<ArchitectureViolation>,
) {
    for (line_no, line) in lines {
        let compact = line.split_whitespace().collect::<String>();

        if relative_path.starts_with("src/api/handlers/") && compact.contains("State<AppState>") {
            violations.push(ArchitectureViolation {
                path: relative_path.to_string(),
                line: *line_no,
                rule: "handlers must extract narrow Axum state".into(),
                detail: line.trim().to_string(),
            });
        }

        if relative_path == "src/api/state.rs"
            && (line.contains("ServiceGraph") || compact.contains("Arc<ServiceGraph>"))
        {
            violations.push(ArchitectureViolation {
                path: relative_path.to_string(),
                line: *line_no,
                rule: "AppState must not wrap ServiceGraph".into(),
                detail: line.trim().to_string(),
            });
        }
    }
}

fn check_bootstrap_boundaries(
    relative_path: &str,
    lines: &[(usize, String)],
    violations: &mut Vec<ArchitectureViolation>,
) {
    for (line_no, line) in lines {
        if relative_path == "src/bootstrap/state.rs"
            && (line.contains("Arc::new(") || line.contains("::new("))
        {
            violations.push(ArchitectureViolation {
                path: relative_path.to_string(),
                line: *line_no,
                rule: "ServiceGraph::build must not directly construct services".into(),
                detail: line.trim().to_string(),
            });
        }

        if relative_path == "src/bootstrap/graph/mod.rs" {
            let trimmed = line.trim_start();
            if trimmed.starts_with("pub mod ") && trimmed.contains("_provider") {
                violations.push(ArchitectureViolation {
                    path: relative_path.to_string(),
                    line: *line_no,
                    rule: "graph provider modules must stay private".into(),
                    detail: line.trim().to_string(),
                });
            }

            if trimmed.starts_with("pub use ")
                && CHILD_PROVIDER_MODULES
                    .iter()
                    .any(|module| trimmed.contains(module))
            {
                violations.push(ArchitectureViolation {
                    path: relative_path.to_string(),
                    line: *line_no,
                    rule: "graph child providers must not be re-exported".into(),
                    detail: line.trim().to_string(),
                });
            }
        }
    }
}

fn check_global_container_patterns(
    relative_path: &str,
    lines: &[(usize, String)],
    violations: &mut Vec<ArchitectureViolation>,
) {
    for (line_no, line) in lines {
        let service_container_once_lock = line.contains("OnceLock<")
            && (line.contains("Service") || line.contains("Graph") || line.contains("Container"));
        if line.contains("lazy_static!") || service_container_once_lock {
            violations.push(ArchitectureViolation {
                path: relative_path.to_string(),
                line: *line_no,
                rule: "global service containers are forbidden".into(),
                detail: line.trim().to_string(),
            });
        }
    }
}

fn layer_for_path(relative_path: &str) -> Option<&'static str> {
    if relative_path.starts_with("src/api/") {
        Some("api")
    } else if relative_path.starts_with("src/app/") {
        Some("app")
    } else if relative_path.starts_with("src/domain/") {
        Some("domain")
    } else if relative_path.starts_with("src/infra/") {
        Some("infra")
    } else if relative_path.starts_with("src/shared/") {
        Some("shared")
    } else if relative_path.starts_with("src/bootstrap/") {
        Some("bootstrap")
    } else {
        None
    }
}

fn forbidden_layers(layer: &str) -> &'static [&'static str] {
    match layer {
        "shared" => &["api", "app", "bootstrap", "domain", "infra"],
        "domain" => &["api", "app", "bootstrap", "infra"],
        "app" => &["api", "bootstrap", "infra"],
        "infra" => &["api", "app", "bootstrap"],
        "api" => &["bootstrap", "infra"],
        "bootstrap" => &[],
        _ => &[],
    }
}

fn references_layer(line: &str, forbidden_layer: &str) -> bool {
    line.contains(&format!("crate::{forbidden_layer}::"))
        || line.contains(&format!("use crate::{forbidden_layer}"))
        || line.contains(&format!("server_rs::{forbidden_layer}::"))
        || line.contains(&format!("use server_rs::{forbidden_layer}"))
}
