use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use syn::spanned::Spanned;
use syn::visit::Visit;
use syn::{
    Attribute, ItemConst, ItemEnum, ItemFn, ItemMod, ItemStatic, ItemStruct, ItemTrait, ItemType,
    UseTree, Visibility,
};

#[derive(Debug, Clone, Default)]
pub struct FeatureSet {
    declared_features: BTreeSet<String>,
    enabled_features: BTreeSet<String>,
    required_feature_paths: BTreeMap<String, BTreeSet<String>>,
}

impl FeatureSet {
    #[allow(dead_code)]
    pub fn new<D, E, DS, ES>(declared_features: D, enabled_features: E) -> Self
    where
        D: IntoIterator<Item = DS>,
        E: IntoIterator<Item = ES>,
        DS: AsRef<str>,
        ES: AsRef<str>,
    {
        Self {
            declared_features: declared_features
                .into_iter()
                .map(|feature| normalize_feature_name(feature.as_ref()))
                .collect(),
            enabled_features: enabled_features
                .into_iter()
                .map(|feature| normalize_feature_name(feature.as_ref()))
                .collect(),
            required_feature_paths: BTreeMap::new(),
        }
    }

    #[allow(dead_code)]
    pub fn from_env() -> Self {
        let manifest_dir = std::env::var("CARGO_MANIFEST_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("."));
        let manifest = ManifestFeatures::load(&manifest_dir.join("Cargo.toml"));
        Self {
            declared_features: manifest.declared_features,
            enabled_features: cargo_enabled_features(),
            required_feature_paths: manifest.required_feature_paths,
        }
    }

    fn is_enabled(&self, feature: &str) -> bool {
        self.enabled_features.contains(feature)
    }

    fn disabled_feature_for_source_path(&self, relative_path: &str) -> Option<&str> {
        self.declared_features
            .iter()
            .find(|feature| feature_matches_source_path(relative_path, feature))
            .filter(|feature| !self.is_enabled(feature))
            .map(String::as_str)
    }

    fn has_disabled_required_feature(&self, relative_path: &str) -> bool {
        self.required_feature_paths
            .get(relative_path)
            .is_some_and(|required_features| {
                required_features
                    .iter()
                    .any(|feature| !self.is_enabled(feature))
            })
    }
}

#[derive(Debug, Default)]
struct ManifestFeatures {
    declared_features: BTreeSet<String>,
    required_feature_paths: BTreeMap<String, BTreeSet<String>>,
}

impl ManifestFeatures {
    fn load(cargo_toml: &Path) -> Self {
        let Ok(source) = fs::read_to_string(cargo_toml) else {
            return Self::default();
        };

        let mut manifest = Self::default();
        let mut section = ManifestSection::Other;
        let mut bin_path: Option<String> = None;
        let mut bin_required_features = BTreeSet::new();

        for line in source.lines() {
            let trimmed = line
                .split_once('#')
                .map(|(line, _)| line)
                .unwrap_or(line)
                .trim();
            if trimmed.is_empty() {
                continue;
            }

            if trimmed.starts_with('[') {
                flush_bin_required_features(
                    &mut manifest.required_feature_paths,
                    &mut bin_path,
                    &mut bin_required_features,
                );
                section = match trimmed {
                    "[features]" => ManifestSection::Features,
                    "[[bin]]" => ManifestSection::Bin,
                    _ => ManifestSection::Other,
                };
                continue;
            }

            match section {
                ManifestSection::Features => {
                    if let Some((name, _)) = trimmed.split_once('=') {
                        manifest
                            .declared_features
                            .insert(normalize_feature_name(trim_toml_string(name.trim())));
                    }
                }
                ManifestSection::Bin => {
                    if let Some((key, value)) = trimmed.split_once('=') {
                        match key.trim() {
                            "path" => {
                                bin_path =
                                    Some(normalize_manifest_path(trim_toml_string(value.trim())));
                            }
                            "required-features" => {
                                bin_required_features =
                                    parse_toml_string_array(value).into_iter().collect();
                            }
                            _ => {}
                        }
                    }
                }
                ManifestSection::Other => {}
            }
        }

        flush_bin_required_features(
            &mut manifest.required_feature_paths,
            &mut bin_path,
            &mut bin_required_features,
        );

        manifest
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ManifestSection {
    Features,
    Bin,
    Other,
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

#[derive(Debug, Clone, Copy)]
struct InfraOnlyExternalCrate {
    path: &'static str,
    reason: &'static str,
}

const BUSINESS_LAYER_FORBIDDEN_EXTERNAL_CRATES: &[InfraOnlyExternalCrate] = &[
    InfraOnlyExternalCrate {
        path: "sea_orm",
        reason: "database infrastructure",
    },
    InfraOnlyExternalCrate {
        path: "sqlx",
        reason: "database infrastructure",
    },
    InfraOnlyExternalCrate {
        path: "qdrant_client",
        reason: "vector-store infrastructure",
    },
    InfraOnlyExternalCrate {
        path: "redis",
        reason: "cache infrastructure",
    },
    InfraOnlyExternalCrate {
        path: "mongodb",
        reason: "database infrastructure",
    },
    InfraOnlyExternalCrate {
        path: "tokio_tungstenite",
        reason: "websocket infrastructure",
    },
    InfraOnlyExternalCrate {
        path: "reqwest",
        reason: "http client infrastructure",
    },
];

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
        if should_skip_path(&relative_path, &features) {
            continue;
        }

        let Ok(source) = fs::read_to_string(&file) else {
            continue;
        };
        let sanitized_lines = sanitized_source_lines(&source);

        match syn::parse_file(&source) {
            Ok(file) => check_ast_boundaries(&relative_path, &file, &mut violations),
            Err(_) => {
                // 解析失败时保留旧检查路径，让架构保护在开发中的不完整源码上仍能工作。
                check_layer_dependencies(&relative_path, &sanitized_lines, &mut violations);
                check_business_infrastructure_boundaries(
                    &relative_path,
                    &sanitized_lines,
                    &mut violations,
                );
            }
        }
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

fn check_ast_boundaries(
    relative_path: &str,
    file: &syn::File,
    violations: &mut Vec<ArchitectureViolation>,
) {
    if relative_path.starts_with("src/app/") && relative_path.contains("qdrant") {
        violations.push(ArchitectureViolation {
            path: relative_path.to_string(),
            line: 1,
            rule: "app filenames must not expose adapter-specific names".into(),
            detail: relative_path.to_string(),
        });
    }

    let mut visitor = AstBoundaryVisitor {
        relative_path,
        violations,
    };
    visitor.visit_file(file);
}

struct AstBoundaryVisitor<'a> {
    relative_path: &'a str,
    violations: &'a mut Vec<ArchitectureViolation>,
}

impl AstBoundaryVisitor<'_> {
    fn layer(&self) -> Option<&'static str> {
        layer_for_path(self.relative_path)
    }

    fn push(
        &mut self,
        span: proc_macro2::Span,
        rule: impl Into<String>,
        detail: impl Into<String>,
    ) {
        self.violations.push(ArchitectureViolation {
            path: self.relative_path.to_string(),
            line: span.start().line.max(1),
            rule: rule.into(),
            detail: detail.into(),
        });
    }

    fn check_path(&mut self, segments: &[String], span: proc_macro2::Span) {
        let Some(layer) = self.layer() else {
            return;
        };
        let Some(first) = segments.first().map(String::as_str) else {
            return;
        };

        let referenced_layer = match first {
            "crate" | "server_rs" => segments.get(1).map(String::as_str),
            _ => None,
        };
        if let Some(referenced_layer) = referenced_layer {
            if forbidden_layers(layer).contains(&referenced_layer) {
                self.push(
                    span,
                    format!("{layer} layer must not depend on {referenced_layer}"),
                    segments.join("::"),
                );
            }
        }

        if matches!(layer, "api" | "app" | "domain" | "shared") {
            if let Some(reason) = BUSINESS_LAYER_FORBIDDEN_EXTERNAL_CRATES
                .iter()
                .find(|external| external.path == first)
                .map(|external| external.reason)
            {
                self.push(
                    span,
                    "business layers must not import infrastructure-only crates",
                    format!("{} [{reason}]", segments.join("::")),
                );
            }
            if segments
                .iter()
                .any(|segment| segment == "DatabaseConnection")
            {
                self.push(
                    span,
                    "business layers must not import infrastructure-only crates",
                    format!("{} [database infrastructure]", segments.join("::")),
                );
            }
        }
    }

    fn check_public_adapter_name(&mut self, visibility: &Visibility, ident: &syn::Ident) {
        if self.layer() == Some("app")
            && is_public(visibility)
            && is_adapter_name(&ident.to_string())
        {
            self.push(
                ident.span(),
                "app public APIs must not expose adapter-specific names",
                ident.to_string(),
            );
        }
    }

    fn check_domain_identifier(&mut self, ident: &syn::Ident) {
        if self.layer() == Some("domain") && is_adapter_name(&ident.to_string()) {
            self.push(
                ident.span(),
                "domain layer must not expose adapter-specific names",
                ident.to_string(),
            );
        }
    }
}

impl<'ast> Visit<'ast> for AstBoundaryVisitor<'_> {
    fn visit_item_mod(&mut self, node: &'ast ItemMod) {
        if has_cfg_test(&node.attrs) {
            return;
        }
        self.check_public_adapter_name(&node.vis, &node.ident);
        syn::visit::visit_item_mod(self, node);
    }

    fn visit_item_use(&mut self, node: &'ast syn::ItemUse) {
        for segments in use_tree_paths(&node.tree) {
            self.check_path(&segments, node.span());
            if self.layer() == Some("api")
                && segments.iter().any(|segment| is_repository_name(segment))
            {
                self.push(
                    node.span(),
                    "api layer must not import repository ports",
                    segments.join("::"),
                );
            }
        }
        syn::visit::visit_item_use(self, node);
    }

    fn visit_path(&mut self, node: &'ast syn::Path) {
        let segments = node
            .segments
            .iter()
            .map(|segment| segment.ident.to_string())
            .collect::<Vec<_>>();
        self.check_path(&segments, node.span());
        if self.layer() == Some("api") && segments.iter().any(|segment| is_repository_name(segment))
        {
            self.push(
                node.span(),
                "api layer must not hold repository ports",
                quote_path(node),
            );
        }
        syn::visit::visit_path(self, node);
    }

    fn visit_item_struct(&mut self, node: &'ast ItemStruct) {
        self.check_public_adapter_name(&node.vis, &node.ident);
        syn::visit::visit_item_struct(self, node);
    }

    fn visit_item_enum(&mut self, node: &'ast ItemEnum) {
        self.check_public_adapter_name(&node.vis, &node.ident);
        syn::visit::visit_item_enum(self, node);
    }

    fn visit_item_trait(&mut self, node: &'ast ItemTrait) {
        self.check_public_adapter_name(&node.vis, &node.ident);
        syn::visit::visit_item_trait(self, node);
    }

    fn visit_item_type(&mut self, node: &'ast ItemType) {
        self.check_public_adapter_name(&node.vis, &node.ident);
        syn::visit::visit_item_type(self, node);
    }

    fn visit_item_fn(&mut self, node: &'ast ItemFn) {
        self.check_public_adapter_name(&node.vis, &node.sig.ident);
        syn::visit::visit_item_fn(self, node);
    }

    fn visit_item_const(&mut self, node: &'ast ItemConst) {
        self.check_public_adapter_name(&node.vis, &node.ident);
        syn::visit::visit_item_const(self, node);
    }

    fn visit_item_static(&mut self, node: &'ast ItemStatic) {
        self.check_public_adapter_name(&node.vis, &node.ident);
        syn::visit::visit_item_static(self, node);
    }

    fn visit_ident(&mut self, ident: &'ast syn::Ident) {
        self.check_domain_identifier(ident);
        syn::visit::visit_ident(self, ident);
    }
}

fn has_cfg_test(attributes: &[Attribute]) -> bool {
    attributes.iter().any(|attribute| {
        attribute.path().is_ident("cfg")
            && attribute.meta.require_list().is_ok_and(|list| {
                list.tokens
                    .to_string()
                    .split_whitespace()
                    .collect::<String>()
                    == "test"
            })
    })
}

fn use_tree_paths(tree: &UseTree) -> Vec<Vec<String>> {
    fn collect(tree: &UseTree, prefix: &mut Vec<String>, paths: &mut Vec<Vec<String>>) {
        match tree {
            UseTree::Path(node) => {
                prefix.push(node.ident.to_string());
                collect(&node.tree, prefix, paths);
                prefix.pop();
            }
            UseTree::Name(node) => {
                let mut path = prefix.clone();
                path.push(node.ident.to_string());
                paths.push(path);
            }
            UseTree::Rename(node) => {
                let mut path = prefix.clone();
                path.push(node.ident.to_string());
                paths.push(path);
            }
            UseTree::Glob(_) => paths.push(prefix.clone()),
            UseTree::Group(node) => {
                for tree in &node.items {
                    collect(tree, prefix, paths);
                }
            }
        }
    }

    let mut paths = Vec::new();
    collect(tree, &mut Vec::new(), &mut paths);
    paths
}

fn quote_path(path: &syn::Path) -> String {
    path.segments
        .iter()
        .map(|segment| segment.ident.to_string())
        .collect::<Vec<_>>()
        .join("::")
}

fn is_public(visibility: &Visibility) -> bool {
    matches!(visibility, Visibility::Public(_))
}

fn is_repository_name(name: &str) -> bool {
    name.ends_with("RepoT") || name.ends_with("Repository")
}

fn is_adapter_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    ["qdrant", "seaorm", "sqlx", "redis"]
        .iter()
        .any(|adapter| lower.contains(adapter))
}

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

fn should_skip_path(relative_path: &str, features: &FeatureSet) -> bool {
    if relative_path.starts_with("src/test/") {
        return true;
    }

    features
        .disabled_feature_for_source_path(relative_path)
        .is_some()
        || features.has_disabled_required_feature(relative_path)
}

fn normalize_feature_name(feature: &str) -> String {
    feature.trim().replace('-', "_")
}

fn cargo_enabled_features() -> BTreeSet<String> {
    std::env::vars_os()
        .filter_map(|(key, _)| key.into_string().ok())
        .filter_map(|key| {
            key.strip_prefix("CARGO_FEATURE_")
                .map(|feature| normalize_feature_name(&feature.to_ascii_lowercase()))
        })
        .collect()
}

fn feature_matches_source_path(relative_path: &str, feature: &str) -> bool {
    relative_path.contains(&format!("/{feature}/"))
        || relative_path.ends_with(&format!("/{feature}.rs"))
}

fn trim_toml_string(value: &str) -> &str {
    value.trim().trim_matches('"').trim_matches('\'')
}

fn normalize_manifest_path(path: &str) -> String {
    path.replace('\\', "/")
}

fn parse_toml_string_array(value: &str) -> BTreeSet<String> {
    let value = value.trim();
    let Some(inner) = value.strip_prefix('[').and_then(|v| v.strip_suffix(']')) else {
        return BTreeSet::new();
    };

    inner
        .split(',')
        .map(trim_toml_string)
        .filter(|feature| !feature.is_empty())
        .map(normalize_feature_name)
        .collect()
}

fn flush_bin_required_features(
    required_feature_paths: &mut BTreeMap<String, BTreeSet<String>>,
    bin_path: &mut Option<String>,
    bin_required_features: &mut BTreeSet<String>,
) {
    if let Some(path) = bin_path.take() {
        if !bin_required_features.is_empty() {
            required_feature_paths.insert(path, std::mem::take(bin_required_features));
        }
    }
    bin_required_features.clear();
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

fn check_business_infrastructure_boundaries(
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
        if let Some(reason) = forbidden_external_import_reason(line) {
            violations.push(ArchitectureViolation {
                path: relative_path.to_string(),
                line: *line_no,
                rule: "business layers must not import infrastructure-only crates".into(),
                detail: format!("{} [{reason}]", line.trim()),
            });
            continue;
        }

        if line.contains("DatabaseConnection") || line.contains("crate::infra::db::entities") {
            violations.push(ArchitectureViolation {
                path: relative_path.to_string(),
                line: *line_no,
                rule: "business layers must not import infrastructure-only crates".into(),
                detail: format!("{} [database infrastructure]", line.trim()),
            });
        }
    }
}

fn forbidden_external_import_reason(line: &str) -> Option<&'static str> {
    BUSINESS_LAYER_FORBIDDEN_EXTERNAL_CRATES
        .iter()
        .find(|external| references_external_crate(line, external.path))
        .map(|external| external.reason)
}

fn references_external_crate(line: &str, crate_path: &str) -> bool {
    let compact = line.split_whitespace().collect::<String>();
    let needle = format!("{crate_path}::");

    if compact.contains(&format!("use{needle}")) {
        return true;
    }

    compact.match_indices(&needle).any(|(index, _)| {
        index == 0
            || compact[..index]
                .chars()
                .next_back()
                .is_none_or(|previous| !previous.is_ascii_alphanumeric() && previous != '_')
    })
}

fn check_api_state_boundaries(
    relative_path: &str,
    lines: &[(usize, String)],
    violations: &mut Vec<ArchitectureViolation>,
) {
    for (line_no, line) in lines {
        let compact = line.split_whitespace().collect::<String>();

        if relative_path.starts_with("src/api/") && compact.contains("State<AppState>") {
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
    check_service_graph_build_boundaries(relative_path, lines, violations);

    for (line_no, line) in lines {
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

            if is_child_provider_reexport(trimmed) {
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

fn check_service_graph_build_boundaries(
    relative_path: &str,
    lines: &[(usize, String)],
    violations: &mut Vec<ArchitectureViolation>,
) {
    if !relative_path.starts_with("src/bootstrap/") {
        return;
    }

    let mut in_service_graph_impl = false;
    let mut service_graph_impl_depth = 0i32;
    let mut service_graph_impl_started = false;
    let mut in_build_fn = false;
    let mut build_fn_depth = 0i32;
    let mut build_fn_started = false;

    for (line_no, line) in lines {
        if !in_service_graph_impl && is_service_graph_impl_line(line) {
            in_service_graph_impl = true;
            service_graph_impl_depth = 0;
            service_graph_impl_started = false;
        }

        if in_service_graph_impl && !in_build_fn && is_build_fn_line(line) {
            in_build_fn = true;
            build_fn_depth = 0;
            build_fn_started = false;
        }

        if in_build_fn && directly_constructs_service(line) {
            violations.push(ArchitectureViolation {
                path: relative_path.to_string(),
                line: *line_no,
                rule: "ServiceGraph::build must not directly construct services".into(),
                detail: line.trim().to_string(),
            });
        }

        if in_service_graph_impl {
            update_brace_scope(
                line,
                &mut service_graph_impl_depth,
                &mut service_graph_impl_started,
            );
            if service_graph_impl_started && service_graph_impl_depth <= 0 {
                in_service_graph_impl = false;
            }
        }

        if in_build_fn {
            update_brace_scope(line, &mut build_fn_depth, &mut build_fn_started);
            if build_fn_started && build_fn_depth <= 0 {
                in_build_fn = false;
            }
        }
    }
}

fn is_service_graph_impl_line(line: &str) -> bool {
    let compact = line.split_whitespace().collect::<String>();
    compact.starts_with("impl") && compact.contains("ServiceGraph")
}

fn is_build_fn_line(line: &str) -> bool {
    let compact = line.split_whitespace().collect::<String>();
    compact.contains("fnbuild(")
}

fn directly_constructs_service(line: &str) -> bool {
    line.contains("Arc::new(") || line.contains("::new(")
}

fn update_brace_scope(line: &str, depth: &mut i32, started: &mut bool) {
    for ch in line.chars() {
        match ch {
            '{' => {
                *depth += 1;
                *started = true;
            }
            '}' => {
                *depth -= 1;
            }
            _ => {}
        }
    }
}

fn is_child_provider_reexport(trimmed_line: &str) -> bool {
    let Some((module, exported_items)) = provider_reexport(trimmed_line) else {
        return false;
    };

    !is_public_provider_api(module, &exported_items)
}

fn provider_reexport(trimmed_line: &str) -> Option<(&str, Vec<String>)> {
    let rest = trimmed_line.strip_prefix("pub use ")?;
    let (module, exported) = rest.split_once("::")?;
    let module = module.trim();
    if !module.ends_with("_provider") {
        return None;
    }

    Some((module, exported_items(exported)))
}

fn exported_items(exported: &str) -> Vec<String> {
    let exported = exported.trim().trim_end_matches(';').trim();
    let exported = exported
        .strip_prefix('{')
        .and_then(|inner| inner.strip_suffix('}'))
        .unwrap_or(exported);

    exported
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(|item| item.split_whitespace().next().unwrap_or(item).to_string())
        .collect()
}

fn is_public_provider_api(module: &str, exported_items: &[String]) -> bool {
    let Some(base) = module.strip_suffix("_provider") else {
        return false;
    };
    if exported_items.is_empty() {
        return false;
    }

    let services_type = format!("{}Services", pascal_case(base));
    let builder_fn = format!("build_{base}_services");

    exported_items
        .iter()
        .all(|item| item == &services_type || item == &builder_fn)
}

fn pascal_case(snake_case: &str) -> String {
    let mut out = String::new();
    for part in snake_case.split('_').filter(|part| !part.is_empty()) {
        let mut chars = part.chars();
        if let Some(first) = chars.next() {
            out.extend(first.to_uppercase());
            out.push_str(chars.as_str());
        }
    }
    out
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
    } else if relative_path.starts_with("src/cli/") || relative_path == "src/bin/cli.rs" {
        Some("client_adapter")
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
        "client_adapter" => &["api", "app", "bootstrap", "domain", "infra", "shared"],
        "bootstrap" => &[],
        _ => &[],
    }
}

fn references_layer(line: &str, forbidden_layer: &str) -> bool {
    let compact = line.split_whitespace().collect::<String>();
    let trimmed = line.trim_start();

    compact.contains(&format!("crate::{forbidden_layer}::"))
        || compact.contains(&format!("crate::{{{forbidden_layer}::"))
        || compact.contains(&format!("server_rs::{forbidden_layer}::"))
        || compact.contains(&format!("server_rs::{{{forbidden_layer}::"))
        || compact.contains(&format!(",{forbidden_layer}::"))
        || compact.contains(&format!("usecrate::{forbidden_layer}"))
        || compact.contains(&format!("useserver_rs::{forbidden_layer}"))
        || trimmed.starts_with(&format!("{forbidden_layer}::"))
}
