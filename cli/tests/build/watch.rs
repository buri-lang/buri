//! `buri test --watch`: what is watched, and what a change to it re-runs.
//!
//! There is no PTY here and there does not need to be one. `--watch` refuses a
//! stdout that is not a terminal, and every test harness in this repository
//! pipes stdout — so the *refusals* are the thing the CLI can be asked about
//! directly (`repositories/testing/watch_refusals`), and the *loop* is driven
//! here as a library, with the terminal flag injected and a file mutation
//! scripted between two passes.
//!
//! That split is the point of `commands::watch::Watch` taking its interval, its
//! pass limit, and its "is a terminal" flag as fields rather than reading them
//! from the environment: the poll-detect-settle-rerun cycle is the same code in
//! both, and what a test replaces is the terminal and the clock.
use crate::harness::*;

use buri::build::session::{Rendering, Session};
use buri::build::workspace::Workspace;
use buri::commands::watch::{Pass, Snapshot, Watch};
use buri::diagnostics::{Diagnostics, SourceMap};
use std::path::{Path, PathBuf};
use std::time::Duration;

// ---------------------------------------------------------------------------
// A repository with two suites that cannot see each other
// ---------------------------------------------------------------------------

fn library(answer: &str) -> String {
    format!("export fn answer(): Int {{ {answer} }}\n")
}

fn suite(label: &str) -> String {
    format!(
        "from \"{label}\" import {{ answer }};\n\
         from \"core/testing/assert\" import * as assert;\n\
         \n\
         test \"answers\" {{\n  assert.eq(answer(), 21);\n}}\n"
    )
}

/// Two libraries, neither depending on the other, so that "the sibling stayed
/// cached" is evidence rather than a coincidence of there being nothing else.
///
/// `//lib/b` carries the two declarations `//lib/a` does not — an extra
/// `sources` entry and a `test { data }` file — because the declared set is a
/// union over several lists and a repository that exercises one of them proves
/// nothing about the others.
fn two_suites(name: &str) -> Scratch {
    let scratch = Scratch::repo(name);
    scratch.write("lib/a/BUILD.buri", "library {\n  test { sources: [\"test/a.buri\"] }\n}\n");
    scratch.write("lib/a/lib.buri", &library("21"));
    scratch.write("lib/a/test/a.buri", &suite("//lib/a"));

    scratch.write(
        "lib/b/BUILD.buri",
        "library {\n  sources: [\"extra.buri\"]\n\
         \n  test {\n    sources: [\"test/b.buri\"]\n    data: [\"test/golden.txt\"]\n  }\n}\n",
    );
    scratch.write("lib/b/lib.buri", &library("21"));
    scratch.write("lib/b/extra.buri", "export fn spare(): Int { 1 }\n");
    scratch.write("lib/b/test/b.buri", &suite("//lib/b"));
    scratch.write("lib/b/test/golden.txt", "recorded\n");
    scratch
}

/// The declared input set for `buri test //...` in a repository, computed the
/// way the loop computes it.
///
/// A `Session` built here rather than through `session::open`, because `open`
/// finds the root from the working directory and a test binary's working
/// directory is shared by every test in it.
fn declared_set(root: &Path) -> Vec<PathBuf> {
    let mut map = SourceMap::new();
    let mut diagnostics = Diagnostics::new();
    let workspace = Workspace::load(root, &mut map, &mut diagnostics).expect("the workspace loads");
    let s = Session {
        root: root.to_path_buf(),
        map,
        parsed: buri::parsing::parser::Cache::new(),
        diagnostics,
        workspace: std::rc::Rc::new(workspace),
        rendering: Rendering::Human { color: false },
    };
    let targets = s.resolve_targets(&["//...".to_string()]).expect("//... resolves");
    buri::commands::watch::inputs(&s, &targets)
}

fn names(root: &Path, paths: &[PathBuf]) -> Vec<String> {
    paths
        .iter()
        .map(|p| p.strip_prefix(root).unwrap_or(p).display().to_string())
        .collect()
}

// ---------------------------------------------------------------------------
// What is watched
// ---------------------------------------------------------------------------

/// The declared set is every file the keys are computed from, plus the two
/// files that decide what the keys are — and nothing else.
///
/// The "nothing else" half is where the design's two free properties live: a
/// build writes into `.buri/`, which nobody declared, so the loop cannot wake
/// itself; and `.git/`, `target/` and `node_modules/` are absent without an
/// ignore list, because they were never in.
#[test]
fn the_declared_set_is_the_inputs_and_the_files_that_decide_them() {
    let scratch = two_suites("watch-set");
    let set = declared_set(&scratch.root);
    let listed = names(&scratch.root, &set);

    for want in [
        "REPO.buri",
        "lib/a/BUILD.buri",
        "lib/a/lib.buri",
        "lib/a/test/a.buri",
        "lib/b/BUILD.buri",
        "lib/b/lib.buri",
        "lib/b/extra.buri",
        "lib/b/test/b.buri",
        "lib/b/test/golden.txt",
    ] {
        assert!(
            listed.iter().any(|p| p == want),
            "the declared set does not name {want}:\n{}",
            indent(&listed.join("\n"))
        );
    }

    // Built once, so that there is something under `.buri/` to have been
    // watched by mistake.
    scratch.run(&["test", "//..."]).ok();
    for path in &listed {
        assert!(
            !path.starts_with(".buri"),
            "the loop watches something the build writes: {path}"
        );
    }
    let mut sorted = listed.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(sorted.len(), listed.len(), "a file is swept twice per sweep");
}

/// A library the suite's *test* code depends on is watched, because it is in
/// the suite's key.
///
/// `test { dependencies }` is not part of the production closure — a test
/// dependency is not a dependency of the thing being shipped — so neither
/// `actions::test_key` nor this set saw it, and both were fixed together. They
/// have to move together: a file in the key and not in the set is a suite the
/// loop stops re-running, and a file in the set and not in the key is a sweep
/// that wakes a pass with nothing to do.
#[test]
fn a_test_only_dependency_is_watched() {
    let scratch = Scratch::repo("watch-test-dep");
    scratch.write(
        "lib/helper/BUILD.buri",
        "library {\n  sources: [\"h.buri\"]\n  visibility: [\"//...\"]\n}\n",
    );
    scratch.write("lib/helper/lib.buri", "from \"//lib/helper/h\" export { double };\n");
    scratch.write("lib/helper/h.buri", "export fn double(n: I64): I64 { n * 2 }\n");
    scratch.write(
        "lib/subject/BUILD.buri",
        "library {\n  test {\n    sources: [\"test/x.buri\"]\n    \
         dependencies: [\"//lib/helper\"]\n  }\n}\n",
    );
    scratch.write("lib/subject/lib.buri", &library("21"));
    scratch.write(
        "lib/subject/test/x.buri",
        "from \"//lib/subject\" import { answer };\n\
         from \"//lib/helper\" import { double };\n\
         from \"core/testing/assert\" import * as assert;\n\
         \ntest \"answers\" {\n  assert.eq(answer(), double(0) + 21);\n}\n",
    );

    let set = declared_set(&scratch.root);
    let listed = names(&scratch.root, &set);
    for want in ["lib/helper/BUILD.buri", "lib/helper/lib.buri", "lib/helper/h.buri"] {
        assert!(
            listed.iter().any(|p| p == want),
            "a test-only dependency's {want} is not watched:\n{}",
            indent(&listed.join("\n"))
        );
    }
}

/// A sweep is `stat`, so what it reports is a file whose modification time or
/// length moved — and a file a build file names that is not there.
#[test]
fn a_sweep_sees_an_edit_and_a_deletion() {
    let scratch = two_suites("watch-sweep");
    let set = declared_set(&scratch.root);
    let before = Snapshot::sweep(&set);

    scratch.write("lib/a/lib.buri", &library("20 + 1"));
    let after = Snapshot::sweep(&set);
    assert_eq!(
        names(&scratch.root, &before.difference(&after)),
        vec!["lib/a/lib.buri".to_string()],
        "an edit was not the only thing the sweep saw"
    );

    std::fs::remove_file(scratch.path("lib/b/extra.buri")).unwrap();
    let gone = Snapshot::sweep(&set);
    assert_eq!(
        names(&scratch.root, &after.difference(&gone)),
        vec!["lib/b/extra.buri".to_string()],
        "a declared source that went away is not a change"
    );
}

// ---------------------------------------------------------------------------
// The loop
// ---------------------------------------------------------------------------

/// The whole cycle, headless: a pass, a file mutation scripted to land while
/// the loop is sweeping, and a second pass that re-runs the suite the edit
/// reached and serves the other one from the cache.
///
/// The incrementality is the cache's rather than the loop's, which is exactly
/// what is being read here: the loop re-runs the *whole* invocation, and
/// `--explain` says that one suite ran and one did not.
#[test]
fn an_edit_re_runs_its_suite_and_leaves_the_sibling_cached() {
    let scratch = two_suites("watch-loop");
    let set = declared_set(&scratch.root);

    let watch = Watch {
        interval: Duration::from_millis(20),
        passes: Some(2),
        // The injected terminal flag. Production always has one — `--watch`
        // without a terminal is refused at parsing — and clearing it here means
        // the run separator does not land in the middle of cargo's output.
        interactive: false,
        header_first: false,
        root: scratch.root.clone(),
    };

    let mut transcripts: Vec<(Vec<String>, String)> = Vec::new();
    let code = watch.drive(|trigger| {
        let run = scratch.run(&["test", "//...", "--explain"]);
        transcripts.push((names(&scratch.root, &trigger.changed), run.all()));
        if trigger.pass == 1 {
            // The scripted mutation. On a thread, and after a delay, because
            // the loop stamps the declared set the moment the opening pass
            // returns: an edit that lands *before* that stamp is one the loop
            // has already accounted for, and the case worth testing is the one
            // that lands while it is sweeping.
            let path = scratch.path("lib/a/lib.buri");
            let text = library("20 + 1");
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(120));
                std::fs::write(&path, text).expect("the scripted edit lands");
            });
        }
        Pass { code: run.code, inputs: set.clone(), output: String::new(), quiet: true }
    });

    assert_eq!(code, 0, "a passing repository did not exit 0 from the loop");
    assert_eq!(transcripts.len(), 2, "the loop did not run a second pass");
    assert!(
        transcripts[0].0.is_empty(),
        "the opening pass claimed a trigger: {:?}",
        transcripts[0].0
    );
    assert_eq!(
        transcripts[1].0,
        vec!["lib/a/lib.buri".to_string()],
        "the second pass was triggered by the wrong file"
    );

    let second = &transcripts[1].1;
    assert_eq!(
        status(second, "test //lib/a"),
        "run",
        "the edited suite was served from the cache:\n{}",
        indent(second)
    );
    assert_eq!(
        status(second, "test //lib/b"),
        "cached",
        "an untouched sibling re-ran:\n{}",
        indent(second)
    );
}

/// A `BUILD.buri` is in the set although it is in no key's input list, because
/// it decides what the keys are: a new source, a new dependency edge, a
/// changed suite. A pass opens its own `Session`, so the edit is picked up
/// whole rather than through a graph loaded before it.
///
/// This is also the answer to "what about a genuinely new file": sources are
/// declared, not globbed, so a new file is not an input until a `BUILD.buri`
/// names it — and naming it is a change to a file the loop is watching.
#[test]
fn a_build_file_is_watched_and_a_new_source_arrives_through_it() {
    let scratch = two_suites("watch-build-file");
    let before = declared_set(&scratch.root);
    assert!(
        !names(&scratch.root, &before).iter().any(|p| p == "lib/a/more.buri"),
        "an undeclared file was watched"
    );

    // A new file on its own is not an input, and the sweep does not see it.
    scratch.write("lib/a/more.buri", "export fn spare(): Int { 2 }\n");
    let snapshot = Snapshot::sweep(&before);
    assert!(
        snapshot.difference(&Snapshot::sweep(&before)).is_empty(),
        "an undeclared file moved the sweep"
    );

    // Declaring it is a change to `lib/a/BUILD.buri`, which is watched — and
    // the next pass's set has the new source in it.
    scratch.write(
        "lib/a/BUILD.buri",
        "library {\n  sources: [\"more.buri\"]\n  test { sources: [\"test/a.buri\"] }\n}\n",
    );
    let changed = snapshot.difference(&Snapshot::sweep(&before));
    assert_eq!(
        names(&scratch.root, &changed),
        vec!["lib/a/BUILD.buri".to_string()],
        "editing a build file did not wake the loop"
    );
    assert!(
        names(&scratch.root, &declared_set(&scratch.root))
            .iter()
            .any(|p| p == "lib/a/more.buri"),
        "a newly declared source did not enter the set"
    );
}

/// A repository that stopped loading is a state, not an exit. The pass reports
/// the diagnostics, exits 2, and still hands back a set — every `BUILD.buri` is
/// in it whether or not its package survived parsing, so the edit that fixes
/// the one that broke is an edit the loop is still watching for.
#[test]
fn a_broken_build_file_keeps_the_loop_watching() {
    let scratch = two_suites("watch-broken");
    scratch.write("lib/a/BUILD.buri", "library {\n  sources: [\n");
    assert!(
        names(&scratch.root, &declared_set(&scratch.root))
            .iter()
            .any(|p| p == "lib/a/BUILD.buri"),
        "a build file that stopped parsing stopped being watched"
    );

    let watch = Watch {
        interval: Duration::from_millis(20),
        passes: Some(2),
        interactive: false,
        header_first: false,
        root: scratch.root.clone(),
    };
    let mut codes = Vec::new();
    let mut triggers: Vec<Vec<String>> = Vec::new();
    watch.drive(|trigger| {
        let run = scratch.run(&["test", "//..."]);
        codes.push(run.code);
        triggers.push(names(&scratch.root, &trigger.changed));
        if trigger.pass == 1 {
            let path = scratch.path("lib/a/BUILD.buri");
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(120));
                std::fs::write(&path, "library {\n  test { sources: [\"test/a.buri\"] }\n}\n")
                    .expect("the repair lands");
            });
        }
        // Recomputed every pass, exactly as `one_pass` does it — including the
        // pass whose repository would not load.
        let inputs = declared_set(&scratch.root);
        Pass { code: run.code, inputs, output: String::new(), quiet: true }
    });

    assert_eq!(codes[0], 2, "an unparseable build file was not exit 2");
    assert_eq!(
        triggers[1],
        vec!["lib/a/BUILD.buri".to_string()],
        "the loop stopped watching the file that broke it"
    );
    assert_eq!(codes[1], 0, "the repaired repository did not pass");
}

/// What `--explain` said about one action, as `run` / `cached` / `keyed`.
/// The same reader `build/incrementality.rs` uses; the claim there is about two
/// invocations and here it is about two passes of one loop.
fn status(text: &str, action_and_label: &str) -> String {
    for line in text.lines() {
        let f: Vec<&str> = line.split_whitespace().collect();
        if f.len() == 5 && format!("{} {}", f[1], f[2]) == action_and_label {
            return f[0].to_string();
        }
    }
    panic!("no `{action_and_label}` line in:\n{}", indent(text));
}
