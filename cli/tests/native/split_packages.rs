//! The conformance packages that stay on JavaScript for one file, run natively
//! through `buri test` without that file.
//!
//! `coverage_agrees` moves every package the native set can build to the debug
//! backend, but a package with one excluded file stays on JavaScript whole. So
//! `numbers/integers.buri` ran natively only in process (`conformance`), never
//! through `buri`. This copies the corpus, keeps each such package's native-set
//! files, and runs them on the debug backend under the heap check.

use std::path::{Path, PathBuf};
use std::process::Command;

/// A copy of the corpus whose JavaScript-only packages hold their native-set
/// files alone, on the native backend. Answers the copy and the packages.
fn split_corpus() -> (PathBuf, Vec<String>) {
    crate::sweep::once();
    let repo = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("native-split-packages-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&repo);
    copy(&Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/conformance"), &repo);
    let mut packages = Vec::new();
    for package in crate::conformance::javascript_only_packages() {
        let dir = repo.join("lib").join(package);
        let mut native: Vec<String> = std::fs::read_dir(dir.join("test"))
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|f| f.ends_with(".buri"))
            .filter(|f| crate::conformance::excluded_because(&format!("{package}/{f}")).is_none())
            .map(|f| format!("\"test/{f}\""))
            .collect();
        if native.is_empty() {
            continue;
        }
        native.sort();
        std::fs::write(
            dir.join("BUILD.buri"),
            format!(
                "library {{\n    sources: []\n\n    test {{\n        sources: [{}]\n        backends: [NATIVE]\n    }}\n}}\n",
                native.join(", ")
            ),
        )
        .unwrap();
        packages.push(format!("//lib/{package}"));
    }
    (repo, packages)
}

fn copy(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        if entry.file_name() == ".buri" {
            continue;
        }
        let dest = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy(&entry.path(), &dest);
        } else {
            std::fs::copy(entry.path(), dest).unwrap();
        }
    }
}

#[test]
fn the_native_files_of_javascript_only_packages_pass_on_the_debug_backend() {
    if !crate::stencil::supported() {
        return;
    }
    let (repo, packages) = split_corpus();
    assert!(packages.len() >= 5, "only {packages:?} have native-set files");
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_buri"));
    cmd.current_dir(&repo).arg("test").args(&packages);
    let ran = crate::shared::ran_command(crate::shared::heap_checked(&mut cmd));
    let summary = ran.stdout.lines().find(|l| l.contains(" passed, ")).unwrap_or_default();
    assert!(
        ran.status == 0 && summary.contains(" 0 failed, 0 skipped"),
        "{packages:?} on the debug backend:\n{}\n{}",
        ran.stdout,
        ran.stderr
    );
    let passed: usize = summary.split(' ').next().and_then(|n| n.parse().ok()).unwrap_or(0);
    assert!(passed >= 400, "only {passed} tests ran: {summary}");
    let _ = std::fs::remove_dir_all(&repo);
}
