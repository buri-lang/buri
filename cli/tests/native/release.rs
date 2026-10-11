//! `buri test --release`: Buri test suites built by LLVM through the `buri`
//! binary, under the heap check.
//!
//! `release/` is a repository of suites, and one program, written for what the
//! release backend emits: 128-bit arithmetic, conversions at every width, values
//! too wide for registers. The conformance corpus covers the rest, and the files
//! of it that `coverage_agrees` leaves on JavaScript run here.

use std::path::{Path, PathBuf};
use std::process::Command;

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

/// A fresh directory for `name`, under this process's own.
fn scratch(name: &str) -> PathBuf {
    crate::sweep::once();
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("native-release-{}", std::process::id()))
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// `buri test --release` over `targets` in `repo`, which must pass whole.
fn passes(repo: &Path, targets: &[&str]) -> String {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_buri"));
    cmd.current_dir(repo).args(["test", "--release"]).args(targets);
    let ran = crate::shared::ran_command(crate::shared::heap_checked(&mut cmd));
    assert!(
        ran.status == 0 && ran.stdout.contains(" passed, 0 failed, 0 skipped ("),
        "buri test --release {targets:?}:\n{}\n{}",
        ran.stdout,
        ran.stderr
    );
    ran.stdout
}

macro_rules! skip_unless_executable {
    () => {
        if let Some(why) = crate::llvm::can_execute() {
            crate::ci::skipped("llvm", why);
            return;
        }
    };
}

/// **Every suite in `release/` passes.**
#[test]
fn the_release_suites_pass() {
    skip_unless_executable!();
    let repo = scratch("suites");
    copy(&Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/native/release"), &repo);
    passes(&repo, &["//lib/..."]);
}

/// **A program fans wide values out over the host's threads.** The host's
/// implementations hold no bytes, so each task's context is nothing at all.
#[test]
fn a_program_fans_out_wide_values_on_the_host() {
    skip_unless_executable!();
    let repo = scratch("fanout");
    copy(&Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/native/release"), &repo);
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_buri"));
    cmd.current_dir(&repo).args(["run", "--release", "//cmd/fanout"]);
    let ran = crate::shared::ran_command(crate::shared::heap_checked(&mut cmd));
    assert_eq!((ran.status, ran.stdout.as_str()), (0, "a2 b5 c8\n"), "{}", ran.stderr);
}

/// **The native set's files in a package that also holds a JavaScript-only
/// one pass under `--release`.** `coverage_agrees` moves a corpus package to
/// the native backends whole or not at all, so `numbers/integers.buri` stays
/// on JavaScript there for `numbers/floats.buri`'s sake. Here each such
/// package keeps only its native-set files and moves.
#[cfg(feature = "backend-stencil")]
#[test]
fn the_native_files_of_a_mixed_package_pass_under_release() {
    skip_unless_executable!();
    let corpus = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/conformance");
    let repo = scratch("mixed");
    std::fs::create_dir_all(repo.join("lib")).unwrap();
    std::fs::copy(corpus.join("REPO.buri"), repo.join("REPO.buri")).unwrap();
    let mut targets = Vec::new();
    for package in crate::conformance::javascript_only_packages() {
        let mut kept = Vec::new();
        for file in std::fs::read_dir(corpus.join("lib").join(package).join("test")).unwrap() {
            let name = file.unwrap().file_name().to_string_lossy().to_string();
            if crate::conformance::excluded_because(&format!("{package}/{name}")).is_none() {
                kept.push(format!("\"test/{name}\""));
            }
        }
        if kept.is_empty() {
            continue;
        }
        kept.sort();
        let at = repo.join("lib").join(package);
        copy(&corpus.join("lib").join(package), &at);
        let build = std::fs::read_to_string(at.join("BUILD.buri")).unwrap();
        // The test block's own `sources`, the second list in the file.
        let (head, rest) = build.split_at(build.find("test {").expect("a test block"));
        let open = rest.find("sources: [").unwrap() + "sources: [".len();
        let close = open + rest[open..].find(']').unwrap();
        let rest = format!("{}{}{}", &rest[..open], kept.join(", "), &rest[close..]);
        let build = format!("{head}{rest}").replace("backends: [JS]", "backends: [NATIVE]");
        std::fs::write(at.join("BUILD.buri"), build).unwrap();
        targets.push(format!("//lib/{package}"));
    }
    assert!(targets.len() >= 4, "fewer mixed packages than expected: {targets:?}");
    let targets: Vec<&str> = targets.iter().map(String::as_str).collect();
    passes(&repo, &targets);
}

/// **A value nested seventy structs deep, with a `Str` at the bottom, keeps its
/// count.** Generated, because the source is the same line seventy times.
#[test]
fn a_value_nested_seventy_structs_deep_keeps_its_count() {
    skip_unless_executable!();
    const DEPTH: usize = 70;
    let repo = scratch("deep");
    let at = repo.join("lib/deep");
    std::fs::create_dir_all(at.join("test")).unwrap();
    let release = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/native/release");
    std::fs::copy(release.join("REPO.buri"), repo.join("REPO.buri")).unwrap();
    std::fs::copy(release.join("lib/wide/BUILD.buri"), at.join("BUILD.buri")).unwrap();
    std::fs::write(at.join("lib.buri"), "").unwrap();
    let mut source = String::from(
        "from \"core/str\" import * as str;\n\
         from \"core/testing/assert\" import * as assert;\n\
         from \"platform/effect\" import { Allocator };\n\
         from \"platform/effect/testing\" import { alloc };\n\n\
         struct S0 { s: Str }\n",
    );
    let mut value = "S0 { s: str.format(ctx, \"${7}\") }".to_string();
    for i in 1..=DEPTH {
        source.push_str(&format!("struct S{i} {{ a: S{} }}\n", i - 1));
        value = format!("S{i} {{ a: {value} }}");
    }
    source.push_str(&format!(
        "\ntest \"deep\" {{\n    let ctx = context {{ Allocator: alloc() }};\n    \
         let d = {value};\n    assert.equal(d{}.s, \"7\");\n}}\n",
        ".a".repeat(DEPTH)
    ));
    std::fs::write(at.join("test/wide.buri"), source).unwrap();
    passes(&repo, &["//lib/deep"]);
}
