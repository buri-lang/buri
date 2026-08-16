//! The worked monorepo in `cli/tests/example` is the build system's own
//! corpus: every rule shape, the tag policy, the testing surface, and a
//! package with both a library and a binary.

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
        platform: buri::build::buildfile::Platform::Js,
        with_tests,
    };
    let analysis = buri::compiler::driver::analyze(Some(&ws), &mut map, &unit);
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
    for pkg in ["lib/money", "lib/ledger", "lib/store", "tools/report"] {
        out.push_str(&check_target(Library, pkg, true));
    }
    assert!(out.is_empty(), "the example libraries do not check:\n{out}");
}

#[test]
fn every_binary_checks() {
    use buri::build::workspace::RuleKind::Binary;
    let mut out = String::new();
    for pkg in ["cmd/server", "cmd/web", "tools/report"] {
        out.push_str(&check_target(Binary, pkg, true));
    }
    assert!(out.is_empty(), "the example binaries do not check:\n{out}");
}
