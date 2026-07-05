# Hybrid Persistence Foundation And Schema Audit Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Establish the first executable foundation for the hybrid persistence architecture: SeaORM migrations as physical schema history, SeaORM entities as logical contract, schema-sync guardrails, and a repeatable read-only schema audit of the legacy `digital_companion` database.

**Architecture:** Runtime startup remains unchanged. A separate `migration/` crate owns schema evolution through `sea-orm-migration`. Application `src/` code is guarded against entity-driven schema sync. Live schema inspection is read-only through dbx connection `DQL`. The old `digital_companion` database remains a source for audit and later ETL only; the target runtime database name is `server_rs`.

**Tech Stack:** Rust 2024, SeaORM `2.0.0-rc.42`, `sea-orm-migration 2.0.0-rc.42`, MySQL, dbx, PowerShell.

## Global Constraints

- Dependency changes must be made with CLI commands such as `cargo add`; do not manually edit dependency entries in any `Cargo.toml`.
- Do not modify `.env` or `config.toml` in this phase; those are local runtime files and cutover is not ready.
- Do not run destructive SQL against `digital_companion`; audit queries must be `SELECT` only.
- Do not use `sea-orm-cli generate entity` as an authority. Generated entities may be used only as comparison input in a later audit/fix plan.
- Do not introduce automatic schema sync in runtime startup, tests, or developer scripts.
- Do not implement baseline schema creation, ETL, ID mapping tables, or Qdrant rebuild in this plan. Those require the audit output from this plan.
- Commit each task separately when executed.

**Spec Position:** `docs/superpowers/specs/2026-07-05-entity-first-migration-design.md`

---

## File Structure

| File | Responsibility |
|---|---|
| `migration/Cargo.toml` | Created by `cargo new` and updated only by `cargo add`; owns migration crate dependencies |
| `migration/src/lib.rs` | Exposes `Migrator` and the ordered migration list |
| `migration/src/main.rs` | CLI entrypoint for `cargo run --manifest-path migration/Cargo.toml -- <command>` |
| `migration/src/m20260705_000001_foundation_marker.rs` | No-op marker migration used to verify the migration crate and migration table wiring |
| `build_support/schema_sync_guard.rs` | Static scanner that rejects runtime schema-sync APIs in `src/` |
| `tests/schema_sync_guard.rs` | Integration tests for the schema-sync guard |
| `docs/schema/mysql-schema-audit-queries.sql` | Read-only MySQL information_schema audit queries |
| `docs/schema/hybrid-persistence-runbook.md` | Developer runbook for migrations, audit, and cutover boundaries |
| `docs/schema/2026-07-05-digital-companion-schema-audit.md` | Initial dbx audit report for `digital_companion` |
| `.env.example` | Template updated to point to `server_rs`, not `digital_companion` |
| `config.example.toml` | Template updated to point to `server_rs`, not `digital_companion` |
| `docs/project-map.md` | Updates old SQL-first documentation to hybrid entity/migration/ETL ownership |

---

### Task 1: Add The SeaORM Migration Crate

**Files:**
- Create: `migration/Cargo.toml`
- Create: `migration/src/lib.rs`
- Create: `migration/src/main.rs`
- Create: `migration/src/m20260705_000001_foundation_marker.rs`

**Interfaces:**
- Produces `migration::Migrator`
- Produces CLI command:

```powershell
cargo run --manifest-path migration/Cargo.toml -- status
```

- [ ] **Step 1: Confirm migration dependency features**

Run:

```powershell
cargo info sea-orm-migration@2.0.0-rc.42 --registry crates-io
```

Expected:

- Output contains `version: 2.0.0-rc.42`
- Output lists `runtime-tokio`
- Output lists `sqlx-mysql`
- Output lists `with-chrono`
- Output lists `with-json`

- [ ] **Step 2: Create the migration crate with Cargo**

Run:

```powershell
cargo new migration --lib
Push-Location migration
cargo add sea-orm-migration@2.0.0-rc.42 --features runtime-tokio,sqlx-mysql,with-chrono,with-json
cargo add tokio@1 --features macros,rt-multi-thread
cargo add async-trait@0.1
Pop-Location
```

Expected:

- `migration/Cargo.toml` exists
- `migration/Cargo.lock` exists if Cargo creates a package-local lockfile
- No dependency entry was hand-edited

- [ ] **Step 3: Replace `migration/src/lib.rs`**

Use this content:

```rust
pub use sea_orm_migration::prelude::*;

mod m20260705_000001_foundation_marker;

pub struct Migrator;

#[async_trait::async_trait]
impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![Box::new(m20260705_000001_foundation_marker::Migration)]
    }
}
```

- [ ] **Step 4: Add `migration/src/main.rs`**

Use this content:

```rust
#[tokio::main]
async fn main() {
    sea_orm_migration::cli::run_cli(migration::Migrator).await;
}
```

- [ ] **Step 5: Add the marker migration**

Create `migration/src/m20260705_000001_foundation_marker.rs` with this content:

```rust
use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, _manager: &SchemaManager) -> Result<(), DbErr> {
        Ok(())
    }

    async fn down(&self, _manager: &SchemaManager) -> Result<(), DbErr> {
        Ok(())
    }
}
```

- [ ] **Step 6: Verify the migration crate compiles**

Run:

```powershell
cargo check --manifest-path migration/Cargo.toml
```

Expected: compile succeeds.

- [ ] **Step 7: Commit**

Run:

```powershell
git add migration
git commit -m "chore: add sea-orm migration crate"
```

---

### Task 2: Add Schema Sync Static Guardrails

**Files:**
- Create: `build_support/schema_sync_guard.rs`
- Create: `tests/schema_sync_guard.rs`

**Interfaces:**
- Produces `schema_sync_guard::check_workspace(root: &Path) -> Result<(), SchemaSyncReport>`
- Guards `src/` against runtime use of SeaORM entity/schema-sync APIs.
- Allows `migration/` to use migration APIs because it is outside the scanned `src/` tree.

- [ ] **Step 1: Add failing tests**

Create `tests/schema_sync_guard.rs` with these tests:

```rust
#[path = "../build_support/schema_sync_guard.rs"]
mod schema_sync_guard;

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use schema_sync_guard::check_workspace;

#[test]
fn allows_current_sources_without_schema_sync() {
    check_workspace(Path::new(env!("CARGO_MANIFEST_DIR")))
        .expect("runtime sources must not contain schema sync APIs");
}

#[test]
fn rejects_entity_driven_schema_sync_in_runtime_sources() {
    let workspace = TestWorkspace::new("rejects_entity_driven_schema_sync_in_runtime_sources");
    workspace.write(
        "src/bin/sync_schema.rs",
        "use sea_orm::{DatabaseConnection, Schema};\n\
         async fn sync(db: &DatabaseConnection) {\n\
             let schema = Schema::new(sea_orm::DatabaseBackend::MySql);\n\
             db.get_schema_builder().apply(schema.create_table_from_entity(crate::infra::repo::entities::users::Entity)).await.unwrap();\n\
         }\n",
    );

    let report = check_workspace(workspace.path())
        .expect_err("schema sync APIs in runtime sources must fail");
    let report = report.to_string();

    assert!(report.contains("create_table_from_entity"));
    assert!(report.contains("get_schema_builder().apply"));
    assert!(report.contains("Schema::new"));
}

struct TestWorkspace {
    root: PathBuf,
}

impl TestWorkspace {
    fn new(name: &str) -> Self {
        let mut root = std::env::temp_dir();
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time before unix epoch")
            .as_nanos();
        root.push(format!("server_rs_schema_sync_guard_{name}_{nanos}"));
        fs::create_dir_all(&root).expect("create temp workspace");
        Self { root }
    }

    fn path(&self) -> &Path {
        &self.root
    }

    fn write(&self, relative_path: &str, contents: &str) {
        let path = self.root.join(relative_path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create parent dirs");
        }
        fs::write(path, contents).expect("write test file");
    }
}

impl Drop for TestWorkspace {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}
```

Run:

```powershell
cargo test --test schema_sync_guard
```

Expected before implementation: test target fails to compile because `build_support/schema_sync_guard.rs` does not exist.

- [ ] **Step 2: Add the guard implementation**

Create `build_support/schema_sync_guard.rs` with these rules:

```rust
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
```

- [ ] **Step 3: Verify guard tests**

Run:

```powershell
cargo test --test schema_sync_guard
```

Expected: tests pass.

- [ ] **Step 4: Commit**

Run:

```powershell
git add build_support/schema_sync_guard.rs tests/schema_sync_guard.rs
git commit -m "test: guard against runtime schema sync"
```

---

### Task 3: Add Read-Only Schema Audit Artifacts

**Files:**
- Create: `docs/schema/mysql-schema-audit-queries.sql`
- Create: `docs/schema/hybrid-persistence-runbook.md`
- Create: `docs/schema/2026-07-05-digital-companion-schema-audit.md`

**Interfaces:**
- Defines the exact read-only audit query set.
- Records the first dbx inspection of `digital_companion`.
- Captures table, column, index, foreign key, trigger, generated-column, text/blob, and domain-classification information needed by the baseline plan.

- [ ] **Step 1: Add audit query file**

Create `docs/schema/mysql-schema-audit-queries.sql` with this content:

```sql
-- Read-only schema audit queries for the legacy digital_companion database.
-- These queries must be executed against dbx connection DQL or an equivalent read-only connection.

SELECT
    TABLE_NAME,
    TABLE_TYPE,
    ENGINE,
    TABLE_COLLATION,
    TABLE_ROWS,
    CREATE_TIME,
    UPDATE_TIME
FROM information_schema.TABLES
WHERE TABLE_SCHEMA = DATABASE()
ORDER BY TABLE_NAME;

SELECT
    TABLE_NAME,
    COLUMN_NAME,
    ORDINAL_POSITION,
    COLUMN_TYPE,
    DATA_TYPE,
    IS_NULLABLE,
    COLUMN_DEFAULT,
    EXTRA,
    COLUMN_KEY,
    CHARACTER_SET_NAME,
    COLLATION_NAME,
    GENERATION_EXPRESSION,
    COLUMN_COMMENT
FROM information_schema.COLUMNS
WHERE TABLE_SCHEMA = DATABASE()
ORDER BY TABLE_NAME, ORDINAL_POSITION;

SELECT
    TABLE_NAME,
    INDEX_NAME,
    NON_UNIQUE,
    SEQ_IN_INDEX,
    COLUMN_NAME,
    SUB_PART,
    INDEX_TYPE,
    COLLATION,
    CARDINALITY
FROM information_schema.STATISTICS
WHERE TABLE_SCHEMA = DATABASE()
ORDER BY TABLE_NAME, INDEX_NAME, SEQ_IN_INDEX;

SELECT
    TABLE_NAME,
    CONSTRAINT_NAME,
    COLUMN_NAME,
    REFERENCED_TABLE_NAME,
    REFERENCED_COLUMN_NAME,
    ORDINAL_POSITION
FROM information_schema.KEY_COLUMN_USAGE
WHERE TABLE_SCHEMA = DATABASE()
  AND REFERENCED_TABLE_NAME IS NOT NULL
ORDER BY TABLE_NAME, CONSTRAINT_NAME, ORDINAL_POSITION;

SELECT
    CONSTRAINT_NAME,
    TABLE_NAME,
    CONSTRAINT_TYPE
FROM information_schema.TABLE_CONSTRAINTS
WHERE TABLE_SCHEMA = DATABASE()
ORDER BY TABLE_NAME, CONSTRAINT_NAME;

SELECT
    TRIGGER_NAME,
    EVENT_MANIPULATION,
    EVENT_OBJECT_TABLE,
    ACTION_TIMING,
    ACTION_ORDER,
    ACTION_STATEMENT
FROM information_schema.TRIGGERS
WHERE TRIGGER_SCHEMA = DATABASE()
ORDER BY EVENT_OBJECT_TABLE, TRIGGER_NAME;

SELECT
    TABLE_NAME,
    COLUMN_NAME,
    COLUMN_TYPE,
    DATA_TYPE,
    IS_NULLABLE,
    COLUMN_DEFAULT,
    EXTRA
FROM information_schema.COLUMNS
WHERE TABLE_SCHEMA = DATABASE()
  AND DATA_TYPE IN ('text', 'mediumtext', 'longtext', 'blob', 'mediumblob', 'longblob', 'json')
ORDER BY TABLE_NAME, ORDINAL_POSITION;
```

- [ ] **Step 2: Add the runbook**

Create `docs/schema/hybrid-persistence-runbook.md` with these sections:

````markdown
# Hybrid Persistence Runbook

## Ownership

- SeaORM entities in `src/infra/repo/entities` are the Rust-facing logical contract.
- `migration/` owns MySQL physical schema history.
- ETL owns selective legacy data movement from `digital_companion` to `server_rs`.
- Qdrant and vector projection state are rebuilt derived artifacts.

## Daily Schema Workflow

1. Edit the SeaORM entity when the Rust-facing logical contract changes.
2. Add or update a `sea-orm-migration` migration for the MySQL physical schema change.
3. Run `cargo check`.
4. Run `cargo check --manifest-path migration/Cargo.toml`.
5. Apply migrations to a disposable or local target database with `DATABASE_URL` pointing at `server_rs`.
6. Do not run schema sync against `digital_companion`, shared dev, staging, or production.

## Local Migration Commands

PowerShell:

```powershell
$env:DATABASE_URL = "mysql://user:password@127.0.0.1:3306/server_rs"
cargo run --manifest-path migration/Cargo.toml -- status
cargo run --manifest-path migration/Cargo.toml -- up
```

## Legacy Database Rule

`digital_companion` is read-only during this transition. Use dbx connection `DQL` only for inspection and later ETL source reads.

## Schema Audit

Run the queries in `docs/schema/mysql-schema-audit-queries.sql` against dbx connection `DQL`. Store the results in `docs/schema/2026-07-05-digital-companion-schema-audit.md`.

The audit report must classify each table into one of these domains:

- AI Core
- Vector Projection
- QQ Bot
- Fresh Context
- Non-AI Content
- Migration/Internal

## Cutover Boundary

Do not update `.env` or `config.toml` to `server_rs` until the baseline migration, selective ETL, ID mapping validation, and vector rebuild validation have passed.
````

- [ ] **Step 3: Generate the initial audit report with dbx**

Use dbx connection `DQL` and database `digital_companion`.

Read-only dbx operations:

- list tables for `DQL`
- describe tables as needed for columns and keys
- execute each `SELECT` query from `docs/schema/mysql-schema-audit-queries.sql`

Create `docs/schema/2026-07-05-digital-companion-schema-audit.md` with this structure:

```markdown
# digital_companion Schema Audit - 2026-07-05

## Source

- Tool: dbx
- Connection: DQL
- Database: digital_companion
- Access mode: read-only inspection

## Summary

## Tables

## Columns

## Indexes

## Foreign Keys

## Triggers

## Text Blob Json Columns

## Generated Columns And Defaults

## Domain Classification

## Entity Comparison Inputs

## Baseline Risks

## Follow-Up Decisions
```

Fill each section with the dbx results or a concise summary derived from the dbx results. The report must explicitly list:

- all `TEXT`, `MEDIUMTEXT`, `LONGTEXT`, `BLOB`, `MEDIUMBLOB`, `LONGBLOB`, and `JSON` columns
- all triggers, or the statement that dbx returned zero triggers
- all generated columns, or the statement that dbx returned zero generated columns
- all tables whose names start with `qq_`
- all tables whose names start with `fresh_`
- all tables that look like vector projection state
- all tables that are candidates for AI Core ETL

- [ ] **Step 4: Commit**

Run:

```powershell
git add docs/schema/mysql-schema-audit-queries.sql docs/schema/hybrid-persistence-runbook.md docs/schema/2026-07-05-digital-companion-schema-audit.md
git commit -m "docs: add hybrid persistence schema audit"
```

---

### Task 4: Update Documentation And Configuration Templates

**Files:**
- Modify: `.env.example`
- Modify: `config.example.toml`
- Modify: `docs/project-map.md`

**Interfaces:**
- Template default database becomes `server_rs`.
- Documentation no longer presents `database/sql/init.sql` as the active schema source.

- [ ] **Step 1: Update template database names**

In `.env.example`, replace:

```text
/digital_companion
```

with:

```text
/server_rs
```

In `config.example.toml`, replace:

```text
/digital_companion
```

with:

```text
/server_rs
```

Do not modify `.env` or `config.toml`.

- [ ] **Step 2: Update `docs/project-map.md` schema ownership**

Replace the directory comment for `database/sql/init.sql` with:

```text
├── database/
│   └── sql/
│       ├── init.sql      # 历史 SQL 参考；新 schema 由 migration/ 管理
│       └── mock.sql      # 历史模拟数据参考
```

Replace the old database initialization instruction:

```text
mysql -u root -p < database/sql/init.sql
```

with:

```powershell
$env:DATABASE_URL = "mysql://user:password@127.0.0.1:3306/server_rs"
cargo run --manifest-path migration/Cargo.toml -- up
```

Replace the old "建表先写" rule with:

```text
| Schema 变更 | 先更新实体表达逻辑契约，再写 sea-orm-migration 表达 MySQL 物理 schema 和迁移历史 |
```

Add a short note near the database section:

```text
当前持久化策略是混合架构：SeaORM entities 是 Rust 逻辑契约，migration/ 是 MySQL 物理 schema 历史，ETL 负责从旧 digital_companion 选择性迁移数据，Qdrant/向量投影从新 MySQL 事实重建。database/sql/*.sql 只作为历史参考和审计输入。
```

- [ ] **Step 3: Verify old SQL-first wording is gone from active docs**

Run:

```powershell
rg -n "建表先写|初始化数据库.*init.sql|init.sql.*所有表结构|sea-orm-cli generate entity" docs/project-map.md .env.example config.example.toml
```

Expected:

- No match for `建表先写`
- No match for `初始化数据库.*init.sql`
- No match for `init.sql.*所有表结构`
- No match for `sea-orm-cli generate entity`

- [ ] **Step 4: Commit**

Run:

```powershell
git add .env.example config.example.toml docs/project-map.md
git commit -m "docs: document hybrid persistence workflow"
```

---

### Task 5: Full Verification

**Files:**
- Reads all files changed by Tasks 1-4.

**Interfaces:**
- Confirms the migration crate compiles.
- Confirms runtime source guard passes.
- Confirms the main application still compiles.

- [ ] **Step 1: Format**

Run:

```powershell
cargo fmt --all
cargo fmt --manifest-path migration/Cargo.toml
```

Expected: formatting completes.

- [ ] **Step 2: Verify migration crate**

Run:

```powershell
cargo check --manifest-path migration/Cargo.toml
```

Expected: compile succeeds.

- [ ] **Step 3: Verify schema-sync guard**

Run:

```powershell
cargo test --test schema_sync_guard
```

Expected: tests pass.

- [ ] **Step 4: Verify main crate**

Run:

```powershell
cargo check
```

Expected: compile succeeds.

- [ ] **Step 5: Verify docs and templates**

Run:

```powershell
rg -n "DATABASE_URL=.*digital_companion|url = .*digital_companion" .env.example config.example.toml
rg -n "建表先写|初始化数据库.*init.sql|init.sql.*所有表结构|sea-orm-cli generate entity" docs/project-map.md .env.example config.example.toml
```

Expected:

- First command returns no matches.
- Second command returns no matches.

- [ ] **Step 6: Final status**

Run:

```powershell
git status --short
```

Expected: clean working tree after all commits.

---

## Out Of Scope For This Plan

- Fixing specific entity type mappings such as `MEDIUMTEXT`, `LONGBLOB`, `ignore`, or `select_as = "text"`.
- Writing the baseline migration that creates all application tables.
- Creating the `server_rs` database.
- Running ETL from `digital_companion`.
- Creating ETL ID mapping tables.
- Rebuilding Qdrant collections or vector projection tables.
- Updating `.env` or `config.toml` to point to `server_rs`.

---

## Next Plans

1. Entity contract cleanup and baseline migration plan: use the audit report to fix entity text/blob semantics and create the first real MySQL baseline migration.
2. Selective ETL plan: implement the approved DAG, ID mapping records, row-count reports, and exclusion of QQ/Fresh Context/non-AI domains.
3. Vector rebuild plan: rebuild Qdrant collections and vector projection tables from target MySQL facts.
4. Cutover plan: validate application behavior against `server_rs`, then update local/runtime configuration.

---

## Self-Review Checklist

- The plan does not make entities generate the database directly.
- Migrations own MySQL physical schema history.
- `digital_companion` remains read-only.
- Dependency changes are CLI-driven.
- Schema sync is guarded out of runtime source code.
- ETL and Qdrant rebuild are explicitly separated from schema migration.
- `.env` and `config.toml` remain untouched until cutover validation.
