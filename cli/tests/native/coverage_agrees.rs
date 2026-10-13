//! Coverage never changes what a program does: the conformance corpus under
//! `buri test --coverage` and `--coverage=mcdc` gives every test the verdict
//! it gets without coverage, on each backend. LLVM's run is
//! `llvm::coverage_changes_no_verdict_under_release`.
//!
//! The stdlib MC/DC gate (`coverage/stdlib`) runs the corpus under
//! `--coverage=mcdc` and stops on a failure, but it probes the standard library
//! too, never runs plain `--coverage`, and isn't a test. #297 was an `if` that
//! `--coverage` sent down the wrong side natively; this is the net for the
//! next one. The corpus holds that case in `semantics/conditions.buri`.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Each coverage mode, against the run without one.
const MODES: [&[&str]; 3] = [&[], &["--coverage"], &["--coverage=mcdc"]];

/// A copy of the corpus at `name`, with every package the native set can build
/// moved to the native backends when `native`.
fn corpus(name: &str, native: bool) -> PathBuf {
    crate::sweep::once();
    let repo = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("native-coverage-agrees-{}", std::process::id()))
        .join(name);
    let _ = std::fs::remove_dir_all(&repo);
    copy(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/conformance"),
        &repo,
    );
    if native {
        let javascript_only = crate::conformance::javascript_only_packages();
        for package in std::fs::read_dir(repo.join("lib")).unwrap() {
            let package = package.unwrap();
            if javascript_only.contains(package.file_name().to_str().unwrap()) {
                continue;
            }
            let build = package.path().join("BUILD.buri");
            let text = std::fs::read_to_string(&build).unwrap();
            std::fs::write(&build, text.replace("backends: [JS]", "backends: [NATIVE]")).unwrap();
        }
    }
    repo
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

/// What `buri test //... --verbose` said about each test, without times or
/// the coverage report: its exit status, each package's header, each test's
/// line and anything a failure printed, sorted, and the summary.
fn verdicts(repo: &Path, flags: &[&str], coverage: &[&str]) -> (i32, Vec<String>, String) {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_buri"));
    cmd.current_dir(repo)
        .args(["test", "//...", "--verbose"])
        .args(flags)
        .args(coverage);
    let ran = crate::shared::ran_command(&mut cmd);
    let mut lines = Vec::new();
    let mut summary = None;
    for line in ran.stdout.lines() {
        if line.contains(" passed, ") && line.contains(" failed, ") {
            summary = Some(line.rsplit_once(" (").map_or(line, |(s, _)| s).to_string());
            break;
        }
        let fields: Vec<&str> = line
            .split("  ")
            .map(str::trim)
            .filter(|f| !f.is_empty())
            .collect();
        if line.starts_with("//") {
            // label, backend, how many tests; then times.
            lines.push(
                fields
                    .iter()
                    .take(3)
                    .copied()
                    .collect::<Vec<_>>()
                    .join("  "),
            );
        } else if line.starts_with("  ok ") || line.starts_with("  FAIL ") {
            // verdict, file, name; then a time.
            lines.push(fields[..fields.len().saturating_sub(1)].join("  "));
        } else if !line.is_empty() && !line.starts_with("shared build") {
            lines.push(line.to_string());
        }
    }
    lines.sort();
    let summary = summary.unwrap_or_else(|| {
        panic!(
            "{coverage:?} {flags:?} printed no summary:\n{}\n{}",
            ran.stdout, ran.stderr
        )
    });
    (ran.status, lines, summary)
}

/// The corpus on one backend, in each mode, each in a copy of its own.
pub(crate) fn agrees(backend: &str, native: bool, flags: &[&str]) {
    let plain = verdicts(
        &corpus(&format!("{backend}-plain"), native),
        flags,
        MODES[0],
    );
    assert!(
        plain.0 == 0,
        "{backend}: the corpus fails without coverage: {}",
        plain.2
    );
    for mode in &MODES[1..] {
        let name = format!(
            "{backend}{}",
            mode[0].trim_start_matches('-').replace('=', "-")
        );
        let covered = verdicts(&corpus(&name, native), flags, mode);
        let (was, now): (BTreeSet<&String>, BTreeSet<&String>) =
            (plain.1.iter().collect(), covered.1.iter().collect());
        let differs: Vec<String> = was
            .symmetric_difference(&now)
            .map(|l| format!("{} {l}", if now.contains(l) { "+" } else { "-" }))
            .collect();
        assert!(
            covered == plain,
            "{backend}: {mode:?} changed what the corpus does.\nwithout: {}\nwith:    {}\n{}",
            plain.2,
            covered.2,
            differs.join("\n")
        );
    }
}

#[test]
fn coverage_changes_no_verdict_on_javascript() {
    agrees("javascript", false, &["--output=js"]);
}

#[test]
fn coverage_changes_no_verdict_on_stencil() {
    if !crate::stencil::supported() {
        return;
    }
    agrees("stencil", true, &[]);
}
