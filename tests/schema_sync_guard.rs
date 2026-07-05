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
