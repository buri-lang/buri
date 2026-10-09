//! What `buri test` does to answer, counted and pinned.
//!
//! An edit-and-rerun spends most of its time in macOS's check of each newly
//! written runner, then in the link (`design/PERFORMANCE.md` §6.79). A change
//! that links two runners where one did, or writes an unchanged runner as a new
//! file, is a slowdown no instruction count sees. So these pin the operations
//! themselves, which no load or thread schedule can move: the counts
//! `BURI_PROFILE=1` prints, and the files a run wrote, read off the disk.
//!
//! Each suite holds one test, so each runs in exactly one process.
use crate::harness::*;

use std::collections::HashMap;
use std::os::unix::fs::MetadataExt as _;
use std::path::Path;

/// The counts `BURI_PROFILE=1` prints for a build.
const PROFILED: [&str; 8] = [
    "suites built",
    "suites restored",
    "suites reused",
    "objects compiled",
    "objects restored",
    "links",
    "new executables launched",
    "test processes",
];

/// Counted off the disk rather than printed.
const WRITTEN: &str = "files written";

/// Three suites. `//lib/a` and `//lib/b` share `//lib/shared`; `//lib/c`
/// stands alone.
fn repo(name: &str) -> Scratch {
    let scratch = Scratch::repo(name);
    scratch.write("lib/shared/BUILD.buri", "library {\n    visibility: [\"//lib/a\", \"//lib/b\"]\n}\n");
    scratch.write("lib/shared/lib.buri", "export fn base(): Int {\n    40\n}\n");
    for s in ["a", "b"] {
        scratch.write(
            &format!("lib/{s}/BUILD.buri"),
            &format!(
                "library {{\n    dependencies: [\"//lib/shared\"]\n    test {{\n        sources: [\"test/{s}.buri\"]\n    }}\n}}\n"
            ),
        );
        scratch.write(
            &format!("lib/{s}/lib.buri"),
            &format!("from \"//lib/shared\" import {{ base }};\n\nexport fn {s}(): Int {{\n    base() + 2\n}}\n"),
        );
    }
    scratch.write("lib/c/BUILD.buri", "library {\n    test {\n        sources: [\"test/c.buri\"]\n    }\n}\n");
    scratch.write("lib/c/lib.buri", "export fn c(): Int {\n    42\n}\n");
    for s in ["a", "b", "c"] {
        scratch.write(
            &format!("lib/{s}/test/{s}.buri"),
            &format!(
                "from \"//lib/{s}\" import {{ {s} }};\nfrom \"core/testing/assert\" import * as assert;\n\n\
                 test \"{s} answers\" {{\n    assert.equal({s}(), 42);\n}}\n"
            ),
        );
    }
    scratch
}

/// Every regular file under `root`, by inode, with its modification time and
/// size. By inode, so a directory renamed whole is not its files written again.
fn files(root: &Path) -> HashMap<(u64, u64), (i64, i64, u64)> {
    let mut out = HashMap::new();
    let mut dirs = vec![root.to_path_buf()];
    while let Some(dir) = dirs.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.filter_map(Result::ok) {
            let Ok(meta) = entry.path().symlink_metadata() else { continue };
            if meta.is_dir() {
                dirs.push(entry.path());
            } else if meta.is_file() {
                out.insert((meta.dev(), meta.ino()), (meta.mtime(), meta.mtime_nsec(), meta.len()));
            }
        }
    }
    out
}

/// `"suites built 3, links 1, files written 7"`: each count named, the other
/// profiled counts zero, and files written checked only when named.
fn wanted(spec: &str) -> Vec<(&'static str, u64)> {
    let mut want: Vec<(&'static str, u64)> = PROFILED.iter().map(|&name| (name, 0)).collect();
    for part in spec.split(", ") {
        let (name, n) = part.rsplit_once(' ').unwrap_or_else(|| panic!("{part:?} names no count"));
        let n = n.parse().unwrap_or_else(|_| panic!("{part:?} is not a count"));
        match want.iter_mut().find(|(known, _)| *known == name) {
            Some((_, slot)) => *slot = n,
            None if name == WRITTEN => want.push((WRITTEN, n)),
            None => panic!("no count is called {name:?}"),
        }
    }
    want
}

/// Runs `buri test <args>` under `BURI_PROFILE=1` and holds what it did to
/// `spec`, naming each count that moved.
fn holds(scratch: &Scratch, scenario: &str, args: &[&str], spec: &str) {
    let before = files(&scratch.root);
    let run = scratch.run_with_env(args, &[("BURI_PROFILE", "1")]);
    run.ok();
    let after = files(&scratch.root);
    let written = after.iter().filter(|(file, now)| before.get(file) != Some(now)).count() as u64;
    let count = |name: &str| -> u64 {
        if name == WRITTEN {
            return written;
        }
        let prefix = format!("{name} ");
        let line = run.stderr.lines().find(|l| l.starts_with(&prefix));
        let line = line.unwrap_or_else(|| panic!("{scenario}: the profile has no `{name}` line:\n{}", indent(&run.all())));
        line[prefix.len()..].trim().parse().unwrap_or_else(|_| panic!("{scenario}: `{line}` is not a count"))
    };
    let moved: Vec<String> = wanted(spec)
        .into_iter()
        .filter_map(|(name, want)| {
            let got = count(name);
            (got != want).then(|| format!("  {name}: {want} -> {got}"))
        })
        .collect();
    assert!(
        moved.is_empty(),
        "{scenario} (`buri {}`) did different work:\n{}\n\n{}",
        args.join(" "),
        moved.join("\n"),
        indent(&run.all())
    );
}

/// The scenarios, in order on one repository. `extra` picks the backend and
/// profile; `want` is each scenario's work, as [`wanted`] reads it.
fn scenarios(scratch: &Scratch, extra: &[&str], want: [&str; 8]) {
    let args = |given: &[&'static str]| -> Vec<&str> { ["test"].iter().chain(given).chain(extra).copied().collect() };
    let [cold, rerun, edited_test, edited_shared, filtered, filtered_again, alone, alone_again] = want;
    holds(scratch, "a cold run", &args(&["//..."]), cold);
    holds(scratch, "a rerun with nothing changed", &args(&["//..."]), rerun);
    scratch.edit("lib/a/test/a.buri", "a answers", "a still answers");
    holds(scratch, "a run after editing one test", &args(&["//..."]), edited_test);
    scratch.edit("lib/shared/lib.buri", "    40", "    39 + 1");
    holds(scratch, "a run after editing a library two suites share", &args(&["//..."]), edited_shared);
    holds(scratch, "a filtered run", &args(&["//...", "--filter=c answers"]), filtered);
    holds(scratch, "the same filtered run again", &args(&["//...", "--filter=c answers"]), filtered_again);
    scratch.edit("lib/c/test/c.buri", "c answers", "c still answers");
    holds(scratch, "one edited suite named alone", &args(&["//lib/c"]), alone);
    holds(scratch, "the same suite named alone again", &args(&["//lib/c"]), alone_again);
}

/// A native run links every suite it builds into one runner. A cold run's
/// files written are left out: on Linux it also writes the musl sysroot.
const NATIVE: [&str; 8] = [
    "suites built 3, objects compiled 10, links 1, new executables launched 1, test processes 3",
    "suites reused 3, files written 0",
    "suites built 1, suites reused 2, objects compiled 2, objects restored 4, links 1, \
     new executables launched 1, test processes 1, files written 7",
    "suites built 2, suites reused 1, objects compiled 3, objects restored 5, links 1, \
     new executables launched 1, test processes 2, files written 10",
    "suites built 1, objects compiled 1, objects restored 4, links 1, new executables launched 1, \
     test processes 1, files written 5",
    "suites restored 1, test processes 1, files written 0",
    // The runner's bytes match the filtered run's, so it isn't a new file.
    "suites built 1, objects compiled 1, objects restored 4, links 1, test processes 1, files written 5",
    "suites reused 1, files written 0",
];

/// A JavaScript run builds a bundle per suite and links nothing.
const JAVASCRIPT: [&str; 8] = [
    "suites built 3, test processes 3, files written 7",
    "suites reused 3, files written 0",
    "suites built 1, suites reused 2, test processes 1, files written 2",
    "suites built 2, suites reused 1, test processes 2, files written 4",
    "suites built 1, test processes 1, files written 3",
    "suites restored 1, test processes 1, files written 0",
    "suites built 1, test processes 1, files written 2",
    "suites reused 1, files written 0",
];

/// The default: the stencil backend.
#[test]
fn a_native_test_run_does_the_pinned_work() {
    scenarios(&repo("counted-native"), &[], NATIVE);
}

/// LLVM, under `backend-llvm`. A toolchain without it refuses, having built
/// nothing.
#[test]
fn a_native_release_test_run_does_the_pinned_work() {
    let scratch = repo("counted-native-release");
    let first = scratch.run_with_env(&["test", "//lib/c", "--release"], &[("BURI_PROFILE", "1")]);
    if first.stderr.contains("test-run-unavailable") {
        first.exits(1).says("\nsuites built 0\n").says("\nlinks 0\n");
        return;
    }
    first.ok();
    std::fs::remove_dir_all(scratch.path(".buri")).unwrap();
    scenarios(&scratch, &["--release"], NATIVE);
}

#[test]
fn a_javascript_test_run_does_the_pinned_work() {
    scenarios(&repo("counted-js"), &["--output=js"], JAVASCRIPT);
}

#[test]
fn a_javascript_release_test_run_does_the_pinned_work() {
    scenarios(&repo("counted-js-release"), &["--output=js", "--release"], JAVASCRIPT);
}
