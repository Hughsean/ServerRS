use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

const FORBIDDEN_PATTERNS: &[(&str, &str)] = &[
    (
        "create_table_from_entity",
        "entity-driven table creation belongs in reviewed migrations or disposable scratch tooling",
    ),
    (
        "create_index_from_entity",
        "entity-driven index creation belongs in reviewed migrations or disposable scratch tooling",
    ),
    (
        "get_schema_builder().apply",
        "runtime code must not apply schema changes directly",
    ),
    (
        "Schema::new",
        "SeaORM schema builders must not be used for runtime schema sync",
    ),
    (
        "sea_orm::Schema",
        "SeaORM schema builders must not be imported by runtime sources",
    ),
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchemaSyncViolation {
    pub path: String,
    pub line: usize,
    pub pattern: &'static str,
    pub detail: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchemaSyncReport {
    pub violations: Vec<SchemaSyncViolation>,
}

impl fmt::Display for SchemaSyncReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "schema sync guard violations:")?;
        for violation in &self.violations {
            writeln!(
                f,
                "- {}:{}: {} ({})",
                violation.path, violation.line, violation.pattern, violation.detail
            )?;
        }
        Ok(())
    }
}

pub fn check_workspace(root: &Path) -> Result<(), SchemaSyncReport> {
    let source_root = root.join("src");
    let mut violations = Vec::new();

    if source_root.exists() {
        let mut files = Vec::new();
        collect_rust_files(&source_root, &mut files);
        for path in files {
            scan_file(root, &path, &mut violations);
        }
    }

    if violations.is_empty() {
        Ok(())
    } else {
        Err(SchemaSyncReport { violations })
    }
}

fn collect_rust_files(dir: &Path, files: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if should_skip_dir(&path) {
                continue;
            }
            collect_rust_files(&path, files);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            files.push(path);
        }
    }
}

fn should_skip_dir(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| matches!(name, "target" | ".git"))
}

fn scan_file(root: &Path, path: &Path, violations: &mut Vec<SchemaSyncViolation>) {
    let Ok(source) = fs::read_to_string(path) else {
        return;
    };
    let relative_path = normalize_relative_path(root, path);

    for (line_index, line) in source.lines().enumerate() {
        for (pattern, detail) in FORBIDDEN_PATTERNS {
            if line.contains(pattern) {
                violations.push(SchemaSyncViolation {
                    path: relative_path.clone(),
                    line: line_index + 1,
                    pattern,
                    detail,
                });
            }
        }
    }
}

fn normalize_relative_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}
