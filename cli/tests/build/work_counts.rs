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

/// Every regular file under `root` but the home, by inode, with its modification time and
/// size. By inode, so a directory renamed whole is not its files written again.
fn files(root: &Path) -> HashMap<(u64, u64), (i64, i64, u64)> {
    let mut out = HashMap::new();
    let mut dirs = vec![root.to_path_buf()];
    while let Some(dir) = dirs.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.filter_map(Result::ok) {
            let Ok(meta) = entry.path().symlink_metadata() else { continue };
            if meta.is_dir() && entry.file_name() != HOME {
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
    holds_exiting(scratch, scenario, args, &[], 0, spec);
}

/// A `BURI_HOME` of the repository's own, which [`files`] doesn't count: `buri`
/// keeps the runners it has started there (`build/programs.rs`), and a home
/// another test shares would make a first start depend on what ran before.
fn home_of(scratch: &Scratch) -> String {
    scratch.path(HOME).display().to_string()
}

const HOME: &str = ".home";

/// [`holds`], with more of the environment and the exit code it expects.
fn holds_exiting(scratch: &Scratch, scenario: &str, args: &[&str], env: &[(&str, &str)], exit: i32, spec: &str) {
    let before = files(&scratch.root);
    let home = home_of(scratch);
    let env: Vec<(&str, &str)> =
        [("BURI_PROFILE", "1"), ("BURI_HOME", home.as_str())].into_iter().chain(env.iter().copied()).collect();
    let run = scratch.run_with_env(args, &env);
    run.exits(exit);
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
fn scenarios(scratch: &Scratch, extra: &[&str], want: [String; 8]) {
    let args = |given: &[&'static str]| -> Vec<&str> { ["test"].iter().chain(given).chain(extra).copied().collect() };
    let [cold, rerun, edited_test, edited_shared, filtered, filtered_again, alone, alone_again] = want.each_ref().map(String::as_str);
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

/// Whether a run's runner is one of the files it writes. On macOS it is a
/// symbolic link into the store of programs macOS has checked
/// (`build/programs.rs`), which [`files`] doesn't count. Linux checks nothing,
/// so there is no store and the runner is a file.
const KEPT_BY_BYTES: bool = cfg!(target_os = "macos");

/// A native run links every suite it builds into one runner. A cold run's
/// files written are left out: on Linux it also writes the musl sysroot.
fn native(release: bool) -> [String; 8] {
    let runner = u64::from(!KEPT_BY_BYTES);
    // Under LLVM's `-O3`, `//lib/c`'s runner folds to the bytes of a runner an
    // earlier step linked, so on macOS the store already holds it.
    let filtered = u64::from(!(release && KEPT_BY_BYTES));
    [
        "suites built 3, objects compiled 10, links 1, new executables launched 1, test processes 3".into(),
        "suites reused 3, files written 0".into(),
        format!(
            "suites built 1, suites reused 2, objects compiled 2, objects restored 4, links 1, \
             new executables launched 1, test processes 1, files written {}",
            6 + runner
        ),
        format!(
            "suites built 2, suites reused 1, objects compiled 3, objects restored 5, links 1, \
             new executables launched 1, test processes 2, files written {}",
            9 + runner
        ),
        format!(
            "suites built 1, objects compiled 1, objects restored 4, links 1, new executables launched {filtered}, \
             test processes 1, files written {}",
            4 + runner
        ),
        "suites restored 1, test processes 1, files written 0".into(),
        // The runner's bytes match the filtered run's, so it isn't a new file.
        "suites built 1, objects compiled 1, objects restored 4, links 1, test processes 1, files written 5".into(),
        "suites reused 1, files written 0".into(),
    ]
}

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
    scenarios(&repo("counted-native"), &[], native(false));
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
    scenarios(&scratch, &["--release"], native(true));
}

#[test]
fn a_javascript_test_run_does_the_pinned_work() {
    scenarios(&repo("counted-js"), &["--output=js"], JAVASCRIPT.map(String::from));
}

#[test]
fn a_javascript_release_test_run_does_the_pinned_work() {
    scenarios(&repo("counted-js-release"), &["--output=js", "--release"], JAVASCRIPT.map(String::from));
}

/// Suite `s`, whose one test asserts `same(k)` is `want`.
fn suite_asserting(scratch: &Scratch, s: &str, k: u32, want: u32) {
    scratch.write(
        &format!("lib/{s}/test/{s}.buri"),
        &format!(
            "from \"//lib/{s}\" import {{ same }};\nfrom \"core/testing/assert\" import * as assert;\n\n\
             test \"{s}\" {{\n    assert.equal(same({k}), {want});\n}}\n"
        ),
    );
}

/// A failing suite's verdict isn't cached, so every run starts its runner
/// again from the cached build. An edit to another suite links that one a
/// runner of its own, and the failing suite's runner stays where it already
/// is rather than being written, and checked by macOS, again.
#[test]
fn a_failing_suites_runner_stays_put_while_another_suite_is_edited() {
    let scratch = Scratch::repo("counted-failing");
    for s in ["e", "f"] {
        scratch.write(
            &format!("lib/{s}/BUILD.buri"),
            &format!("library {{\n    test {{\n        sources: [\"test/{s}.buri\"]\n    }}\n}}\n"),
        );
        scratch.write(&format!("lib/{s}/lib.buri"), "export fn same(k: Int): Int {\n    k\n}\n");
    }
    suite_asserting(&scratch, "e", 0, 0);
    suite_asserting(&scratch, "f", 0, 1);
    // A runner per suite, so the cold run leaves `//lib/f`'s at its own path.
    let env = [("BURI_TEST_BATCH_BYTES", "1")];
    let args = ["test", "//..."];
    // The two runners share three units, which a run emits once.
    holds_exiting(
        &scratch,
        "a cold run",
        &args,
        &env,
        1,
        "suites built 2, objects compiled 7, objects restored 3, links 2, new executables launched 2, \
         test processes 2",
    );
    for k in 1..=3 {
        suite_asserting(&scratch, "e", k, k);
        holds_exiting(
            &scratch,
            &format!("edit {k}"),
            &args,
            &env,
            1,
            "suites built 1, suites restored 1, objects compiled 1, objects restored 4, links 1, \
             new executables launched 1, test processes 2",
        );
    }
}

/// A runner whose bytes an earlier run already started runs from that file, in
/// another repository or after `buri clean`, so macOS doesn't check it again.
/// Linux checks nothing and keeps no store, so there each runner is new.
#[test]
fn a_runner_an_earlier_run_started_is_not_a_new_executable() {
    let first = repo("counted-kept-first");
    let second = repo("counted-kept-second");
    let home = home_of(&first);
    let env = [("BURI_HOME", home.as_str())];
    let args = ["test", "//..."];
    let cold = "suites built 3, objects compiled 10, links 1, test processes 3";
    let first_start = format!("{cold}, new executables launched 1");
    let again = if KEPT_BY_BYTES { cold } else { first_start.as_str() };
    holds_exiting(&first, "a cold run", &args, &env, 0, &first_start);
    holds_exiting(&second, "the same tree in another repository", &args, &env, 0, again);
    first.run(&["clean"]).ok();
    holds_exiting(&first, "a run after buri clean", &args, &env, 0, again);
}

/// Two runners, each with a failing suite that isn't its first: `server`
/// suites `a`, `d` and `e`, then `client` suites `b` and `c`, which may not
/// share one. `d` and `c` fail.
fn two_runners_failing(name: &str) -> Scratch {
    let scratch = Scratch::repo(name);
    scratch.write(
        "REPO.buri",
        "tag {\n  name: \"server\"\n  doc: \"runs on infrastructure we operate\"\n  \
         forbids { tags: [\"client\"] }\n}\n\n\
         tag {\n  name: \"client\"\n  doc: \"ships to a user's machine\"\n}\n",
    );
    let suites = [("a", "server", 0), ("d", "server", 1), ("e", "server", 0), ("b", "client", 0), ("c", "client", 1)];
    for (s, tag, want) in suites {
        scratch.write(
            &format!("lib/{s}/BUILD.buri"),
            &format!(
                "library {{\n    tags: [\"{tag}\"]\n    test {{\n        sources: [\"test/{s}.buri\"]\n    }}\n}}\n"
            ),
        );
        scratch.write(&format!("lib/{s}/lib.buri"), "export fn same(k: Int): Int {\n    k\n}\n");
        suite_asserting(&scratch, s, 0, want);
    }
    scratch
}

/// Each step of [`failing_groups`]: what it changes, then the run.
enum Step {
    Edit(&'static str, u32, u32),
    Run(&'static str, &'static [&'static str]),
}

const ALL: &[&str] = &["//..."];
const FILTERED: &[&str] = &["//...", "--filter=d"];

const FAILING_GROUPS: [Step; 15] = [
    Step::Run("a cold run", ALL),
    Step::Run("a rerun", ALL),
    Step::Edit("e", 1, 1),
    Step::Run("an edit", ALL),
    Step::Edit("e", 2, 2),
    Step::Run("another edit", ALL),
    Step::Edit("e", 3, 4),
    Step::Run("an edit that fails", ALL),
    Step::Run("a rerun with three failing", ALL),
    Step::Edit("e", 4, 4),
    Step::Run("an edit that passes again", ALL),
    // `//lib/b` is the first suite in the runner that holds the failing `//lib/c`. A
    // value no earlier edit used, so its runner holds bytes no earlier run started.
    Step::Edit("b", 5, 5),
    Step::Run("an edit to a failing suite's runner-mate", ALL),
    Step::Run("a filtered run", FILTERED),
    Step::Run("the same filtered run", FILTERED),
];

/// Runs [`FAILING_GROUPS`] then a rerun, holding each run to its line of `want`.
fn failing_groups(scratch: &Scratch, extra: &[&str], want: [&str; 11]) {
    let mut want = want.into_iter();
    let runs = FAILING_GROUPS.iter().chain([&Step::Run("a rerun after the filtered runs", ALL)]);
    for step in runs {
        match *step {
            Step::Edit(s, k, w) => suite_asserting(scratch, s, k, w),
            Step::Run(scenario, given) => {
                let args: Vec<&str> = ["test"].iter().chain(given).chain(extra).copied().collect();
                holds_exiting(scratch, scenario, &args, &[], 1, want.next().expect("a count per run"));
            }
        }
    }
}

/// A failing suite's runner starts again on every run, and only a link is a new
/// executable: an edit launches one, a rerun or a whole run after a filtered
/// one none. The first edit's runner holds one suite where the cold run's held
/// three, which changes `core_testing_assert`'s object (PERFORMANCE.md §6.80).
const NATIVE_FAILING: [&str; 11] = [
    // The two runners share three units, which a run emits once.
    "suites built 5, objects compiled 13, objects restored 3, links 2, new executables launched 2, \
     test processes 5",
    "suites reused 3, suites restored 2, test processes 2, files written 0",
    "suites built 1, suites reused 2, suites restored 2, objects compiled 2, objects restored 3, links 1, \
     new executables launched 1, test processes 3",
    "suites built 1, suites reused 2, suites restored 2, objects compiled 1, objects restored 4, links 1, \
     new executables launched 1, test processes 3",
    "suites built 1, suites reused 2, suites restored 2, objects compiled 1, objects restored 4, links 1, \
     new executables launched 1, test processes 3",
    "suites reused 2, suites restored 3, test processes 3, files written 0",
    "suites built 1, suites reused 2, suites restored 2, objects compiled 1, objects restored 4, links 1, \
     new executables launched 1, test processes 3",
    "suites built 1, suites reused 2, suites restored 2, objects compiled 1, objects restored 4, links 1, \
     new executables launched 1, test processes 3",
    "suites built 1, objects compiled 1, objects restored 4, links 1, new executables launched 1, test processes 1",
    "suites restored 1, test processes 1, files written 0",
    "suites reused 3, suites restored 2, test processes 2, files written 0",
];

const JAVASCRIPT_FAILING: [&str; 11] = [
    "suites built 5, test processes 5",
    "suites reused 3, suites restored 2, test processes 2, files written 0",
    "suites built 1, suites reused 2, suites restored 2, test processes 3, files written 2",
    "suites built 1, suites reused 2, suites restored 2, test processes 3, files written 2",
    "suites built 1, suites reused 2, suites restored 2, test processes 3, files written 3",
    "suites reused 2, suites restored 3, test processes 3, files written 0",
    "suites built 1, suites reused 2, suites restored 2, test processes 3, files written 2",
    "suites built 1, suites reused 2, suites restored 2, test processes 3, files written 2",
    "suites built 1, test processes 1, files written 3",
    "suites restored 1, test processes 1, files written 0",
    // `//lib/d`'s whole bundle again, where its filtered one was. A bundle has no launch check.
    "suites reused 3, suites restored 2, test processes 2, files written 1",
];

#[test]
fn a_native_failing_suites_runner_is_never_a_new_executable_again() {
    failing_groups(&two_runners_failing("counted-failing-groups"), &[], NATIVE_FAILING);
}

#[test]
fn a_native_release_failing_suites_runner_is_never_a_new_executable_again() {
    let scratch = two_runners_failing("counted-failing-groups-release");
    let first = scratch.run(&["test", "//lib/a", "--release"]);
    if first.stderr.contains("test-run-unavailable") {
        first.exits(1);
        return;
    }
    first.ok();
    std::fs::remove_dir_all(scratch.path(".buri")).unwrap();
    failing_groups(&scratch, &["--release"], NATIVE_FAILING);
}

#[test]
fn a_javascript_failing_suites_bundle_is_written_only_when_it_changes() {
    failing_groups(&two_runners_failing("counted-failing-groups-js"), &["--output=js"], JAVASCRIPT_FAILING);
}
