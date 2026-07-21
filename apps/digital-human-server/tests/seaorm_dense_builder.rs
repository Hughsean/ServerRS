use std::fs;
use std::path::{Path, PathBuf};

use syn::spanned::Spanned;
use syn::visit::Visit;

#[derive(Default)]
struct ActiveModelLiteralVisitor {
    lines: Vec<usize>,
}

impl<'ast> Visit<'ast> for ActiveModelLiteralVisitor {
    fn visit_expr_struct(&mut self, node: &'ast syn::ExprStruct) {
        if node
            .path
            .segments
            .last()
            .is_some_and(|segment| segment.ident == "ActiveModel")
        {
            self.lines.push(node.path.span().start().line);
        }
        syn::visit::visit_expr_struct(self, node);
    }
}

#[test]
fn production_code_uses_dense_active_model_builders() {
    let mut rust_files = Vec::new();
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    for src in [
        manifest_dir.join("src"),
        manifest_dir.join("../../crates/digital-human/src"),
    ] {
        collect_rust_files(&src, &mut rust_files);
    }
    rust_files.sort();

    let mut violations = Vec::new();
    for path in rust_files {
        let source = fs::read_to_string(&path).expect("读取 Rust 源文件失败");
        let syntax = syn::parse_file(&source).expect("解析 Rust 源文件失败");
        let mut visitor = ActiveModelLiteralVisitor::default();
        visitor.visit_file(&syntax);
        for line in visitor.lines {
            let relative = path.strip_prefix(manifest_dir).unwrap_or(&path);
            violations.push(format!("{}:{line}", relative.display()));
        }
    }

    assert!(
        violations.is_empty(),
        "请使用 SeaORM 2.0 Dense ActiveModel::builder()，不要显式构造 ActiveModel：\n{}",
        violations.join("\n")
    );
}

fn collect_rust_files(dir: &Path, output: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).expect("读取源码目录失败") {
        let path = entry.expect("读取目录项失败").path();
        if path.is_dir() {
            collect_rust_files(&path, output);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            output.push(path);
        }
    }
}
