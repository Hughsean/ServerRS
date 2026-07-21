#[path = "build_support/architecture_guard.rs"]
mod architecture_guard;

fn main() {
    let manifest_dir = std::path::PathBuf::from(
        std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR must be set"),
    );
    let features = architecture_guard::FeatureSet::from_env();

    architecture_guard::emit_rerun_directives();
    println!("cargo::rerun-if-changed=../../crates/digital-human/src");
    println!("cargo::rerun-if-changed=../../crates/qqbot/src");

    let digital_human = manifest_dir.join("../../crates/digital-human");
    for root in [&manifest_dir, &digital_human] {
        if let Err(report) = architecture_guard::check_workspace(root, features.clone()) {
            eprintln!("{report}");
            panic!("architecture layering check failed");
        }
    }

    if std::env::var_os("CARGO_FEATURE_QQ_BOT").is_some() {
        let qqbot = manifest_dir.join("../../crates/qqbot");
        if let Err(report) = architecture_guard::check_workspace(&qqbot, features) {
            eprintln!("{report}");
            panic!("architecture layering check failed");
        }
    }
}
