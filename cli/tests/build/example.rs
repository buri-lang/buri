//! The worked monorepo in `cli/tests/example` is the build system's own
//! corpus: every rule shape, the tag policy, the testing surface, and a
//! package with both a library and a binary.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::string_slice,
    clippy::arithmetic_side_effects,
    clippy::print_stdout,
    clippy::print_stderr,
    reason = "test code. The lint set in `Cargo.toml` pins a promise about the \
              toolchain — that no input panics it — and a harness that drives \
              the toolchain is not the toolchain. A test that unwraps fails on \
              the line that broke, which is what a test is for, and threading \
              `?` through an assertion buys nothing. `clippy.toml` exempts \
              `#[test]` functions already; this covers the helpers around them."
)]
use std::path::{Path, PathBuf};

fn example_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().join("cli/tests/example")
}

fn check_target(kind: buri::build::workspace::RuleKind, pkg: &str, with_tests: bool) -> String {
    let root = example_root();
    let mut map = buri::diagnostics::SourceMap::new();
    let mut diags = buri::diagnostics::Diagnostics::new();
    let ws = buri::build::workspace::Workspace::load(&root, &mut map, &mut diags).unwrap();
    let Some(id) = ws.pkg_by_path(pkg) else { panic!("no package {pkg}") };
    let unit = buri::compiler::modules::Unit {
        target: Some(buri::build::workspace::TargetId { pkg: id, kind }),
        platform: None,
        with_tests,
    };
    let mut cache = buri::parsing::parser::Cache::new();
    let analysis = buri::compiler::driver::analyze(Some(&ws), &mut map, &mut cache, &unit);
    let mut out = String::new();
    for d in diags.items.iter().chain(analysis.diags.items.iter()) {
        out.push_str(&map.render(d, false));
    }
    out
}

#[test]
fn every_library_checks() {
    use buri::build::workspace::RuleKind::Library;
    let mut out = String::new();
    for pkg in ["lib/money", "lib/ledger", "lib/store", "lib/kit", "tools/report"] {
        out.push_str(&check_target(Library, pkg, true));
    }
    assert!(out.is_empty(), "the example libraries do not check:\n{out}");
}

#[test]
fn every_binary_checks() {
    use buri::build::workspace::RuleKind::Binary;
    let mut out = String::new();
    for pkg in ["cmd/server", "cmd/web", "cmd/basket", "tools/report"] {
        out.push_str(&check_target(Binary, pkg, true));
    }
    assert!(out.is_empty(), "the example binaries do not check:\n{out}");
}
