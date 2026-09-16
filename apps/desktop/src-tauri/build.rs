use std::path::Path;

/// The one place a version is written down.
///
/// This crate is its own workspace — Tauri's dependency tree has no business
/// slowing `cargo test` at the top of the project — which is exactly why it
/// cannot say `version.workspace = true` and inherit the number like every
/// other crate. So the number is duplicated here, and a duplicate that nothing
/// checks is a duplicate that drifts: a release folder named for one version
/// holding a window that reports another, and a log line sending whoever reads
/// it to the wrong code.
///
/// Refused at build time rather than caught by a test, because this crate is
/// built by `tauri build` rather than by the workspace's own `cargo test`, and
/// a check that does not run during the build that produces the wrong binary is
/// not a check.
const WORKSPACE_MANIFEST: &str = "../../../Cargo.toml";

fn main() {
    println!("cargo:rerun-if-changed={WORKSPACE_MANIFEST}");
    refuse_a_version_of_its_own();
    tauri_build::build()
}

fn refuse_a_version_of_its_own() {
    let path = Path::new(WORKSPACE_MANIFEST);
    let text = std::fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
    let manifest: toml::Table = text.parse().expect("the workspace manifest parses");
    let workspace = manifest["workspace"]["package"]["version"]
        .as_str()
        .expect("the workspace names a version")
        .to_owned();
    let mine = std::env::var("CARGO_PKG_VERSION").expect("cargo names this crate's version");
    assert!(
        workspace == mine,
        "this crate says {mine} and the workspace says {workspace}. The workspace is the one \
         that is right: set version = \"{workspace}\" in apps/desktop/src-tauri/Cargo.toml.",
    );
}
