//! The worked monorepo in `cli/tests/example` is the build system's own
//! corpus: every rule shape, the tag policy, the testing surface, and a
//! package with both a library and a binary.
use std::path::{Path, PathBuf};

fn example_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().join("cli/tests/example")
}

fn check_target(kind: buri::build::workspace::RuleKind, package: &str, with_tests: bool) -> String {
    let root = example_root();
    let mut map = buri::diagnostics::SourceMap::new();
    let mut diagnostics = buri::diagnostics::Diagnostics::new();
    let workspace =
        buri::build::workspace::Workspace::load(&root, &mut map, &mut diagnostics).unwrap();
    let Some(id) = workspace.package_by_path(package) else { panic!("no package {package}") };
    let unit = buri::compiler::modules::Unit {
        target: Some(buri::build::workspace::TargetId { package: id, kind }),
        platform: None,
        with_tests,
    };
    let mut cache = buri::parsing::parser::Cache::new();
    let analysis = buri::compiler::driver::analyze(Some(&workspace), &mut map, &mut cache, &unit);
    let mut out = String::new();
    for d in diagnostics.items.iter().chain(analysis.diagnostics.items.iter()) {
        out.push_str(&map.render(d, false));
    }
    out
}

#[test]
fn every_library_checks() {
    use buri::build::workspace::RuleKind::Library;
    let mut out = String::new();
    for package in ["lib/money", "lib/ledger", "lib/store", "lib/kit", "tools/report"] {
        out.push_str(&check_target(Library, package, true));
    }
    assert!(out.is_empty(), "the example libraries do not check:\n{out}");
}

#[test]
fn every_binary_checks() {
    use buri::build::workspace::RuleKind::Binary;
    let mut out = String::new();
    for package in ["cmd/server", "cmd/web", "cmd/basket", "tools/report"] {
        out.push_str(&check_target(Binary, package, true));
    }
    assert!(out.is_empty(), "the example binaries do not check:\n{out}");
}
