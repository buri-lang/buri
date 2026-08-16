//! What the cache may and may not do.
//!
//! These are the tests that interleave edits with rebuilds, so they are Rust
//! rather than manifests: a repository case runs commands against one tree,
//! and the claims here are about what changes *between* two states of it.
//!
//! The incrementality table at HERMETICITY-AND-CACHING.md:107-118 is written
//! in terms of *which actions run*, which nothing could observe from outside
//! the toolchain until `--explain`. These read that transcript. Keys are
//! compared between two states of one tree and never recorded, because a key
//! includes the toolchain version and would move on every release.

mod harness;
use harness::*;

use std::collections::BTreeMap;

/// The `--explain` transcript as a map from `"<action> <label>"` to its key,
/// so a test can ask what changed rather than diffing whole output.
///
/// ```text
/// keyed  compile //lib/money js 2d04ba5dd12a
/// run    link //cmd/web js da7d2503ec1c
/// ```
fn keys(run: &Run) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for line in run.stdout.lines() {
        let f: Vec<&str> = line.split_whitespace().collect();
        // status, action, label, platform, key
        if f.len() == 5 && matches!(f[0], "run" | "cached" | "keyed") {
            out.insert(format!("{} {}", f[1], f[2]), f[4].to_string());
        }
    }
    assert!(!out.is_empty(), "no --explain lines in:\n{}", indent(&run.all()));
    out
}

/// What `--explain` said about one action, as `run` / `cached` / `keyed`.
fn status(run: &Run, action_and_label: &str) -> String {
    for line in run.stdout.lines() {
        let f: Vec<&str> = line.split_whitespace().collect();
        if f.len() == 5 && format!("{} {}", f[1], f[2]) == action_and_label {
            return f[0].to_string();
        }
    }
    panic!("no `{action_and_label}` line in:\n{}", indent(&run.all()));
}

/// A program printing `answer=<n>`, so a stale artifact is caught by what it
/// computes rather than by whether some file was rewritten.
fn program(answer: i32) -> String {
    format!(
        r#"
from "core/cap" import {{ Alloc, Stdout }};
from "core/host" import * as host;

fn answer(): Int {{ {answer} }}

export fn main(): Result<(), Str> {{
  let ctx = context {{ Alloc: host.alloc, Stdout: host.stdout }};
  let _ = ctx.println("answer=${{answer()}}");
  .Ok(())
}}
"#
    )
}

/// A content-keyed cache must not be able to hold a wrong answer. This builds,
/// edits a source, rebuilds, and checks the program's *behaviour* changed —
/// not merely that some file was rewritten.
#[test]
fn the_cache_cannot_serve_a_stale_answer() {
    let scratch = Scratch::repo("cache-check");
    scratch.binary_package("cmd/c", &program(1));

    scratch.run(&["build", "//cmd/c"]).ok();
    assert_eq!(scratch.exec_js("cmd/c").stdout, "answer=1\n");

    // A second build with no edit must be served from the cache.
    scratch.run(&["build", "//cmd/c"]).says("cached");
    assert_eq!(scratch.exec_js("cmd/c").stdout, "answer=1\n");

    // An edit must invalidate it.
    scratch.write("cmd/c/main.buri", &program(2));
    scratch.run(&["build", "//cmd/c"]).silent_about("cached");
    assert_eq!(
        scratch.exec_js("cmd/c").stdout,
        "answer=2\n",
        "the cache served a stale artifact"
    );

    // And going back must hit the entry the first build left.
    scratch.write("cmd/c/main.buri", &program(1));
    scratch.run(&["build", "//cmd/c"]).says("cached");
    assert_eq!(scratch.exec_js("cmd/c").stdout, "answer=1\n");
    eprintln!("cache: invalidates on edit, hits on revert, never stale");
}

/// Keys are over content, not timestamps. Rewriting a file with the bytes it
/// already held is what checking a branch out and back looks like to the
/// filesystem, and it must rebuild nothing.
#[test]
fn rewriting_a_file_with_its_own_bytes_rebuilds_nothing() {
    let scratch = Scratch::repo("cache-touch");
    scratch.binary_package("cmd/c", &program(7));
    scratch.run(&["build", "//cmd/c"]).ok();

    let source = scratch.read("cmd/c/main.buri");
    scratch.write("cmd/c/main.buri", &source);
    scratch
        .run(&["build", "//cmd/c"])
        .says("cached");

    // And `--force` is the way past it, so the cache is an optimisation rather
    // than something a user cannot get out from under.
    scratch
        .run(&["build", "//cmd/c", "--force"])
        .ok()
        .silent_about("cached");
    eprintln!("cache: keyed on content, and --force overrides it");
}

// ---------------------------------------------------------------------------
// Which actions an edit reaches
// ---------------------------------------------------------------------------

/// The rows of HERMETICITY-AND-CACHING.md:107-118 that this toolchain can
/// currently answer: an edit reaches the target it is in and the binary above
/// it, and nothing sideways.
///
/// The worked monorepo rather than a toy, because "the sibling did not change"
/// is only evidence when there is a sibling that could have.
#[test]
fn an_edit_reaches_its_own_target_and_the_link_above_it() {
    let example = Scratch::copy_of("explain-edit", &example_repo());
    let before = keys(example.run(&["build", "//cmd/web", "--explain"]).ok());

    // A body edit inside //lib/money, in a module //lib/ledger does not name.
    example.edit(
        "lib/money/cents.buri",
        "let whole = self.0 / 100;",
        "let whole = (self.0) / 100;",
    );
    let after = keys(example.run(&["build", "//cmd/web", "--explain"]).ok());

    assert_ne!(
        before["compile //lib/money"], after["compile //lib/money"],
        "editing //lib/money did not change its own key"
    );
    assert_ne!(
        before["link //cmd/web"], after["link //cmd/web"],
        "editing a library did not change the artifact that links it"
    );
    // The two that must not move. //lib/ledger depends on //lib/money and
    // //cmd/web contains the edit's transitive dependent, and neither of their
    // *own* sources changed.
    assert_eq!(
        before["compile //lib/ledger"], after["compile //lib/ledger"],
        "editing //lib/money changed //lib/ledger's key, which its sources did not earn"
    );
    assert_eq!(
        before["compile //cmd/web"], after["compile //cmd/web"],
        "editing //lib/money changed //cmd/web's own key"
    );
}

/// Tags are policy, not input. Adding one changes what may be built, never
/// what is built, so nothing recompiles. `build/cache.rs::tags_are_not_in_the_key`
/// asserts this from the inside; this asserts it through the CLI.
#[test]
fn adding_a_tag_recompiles_nothing() {
    let example = Scratch::copy_of("explain-tag", &example_repo());
    let before = keys(example.run(&["build", "//cmd/web", "--explain"]).ok());

    example.edit(
        "lib/money/BUILD.buri",
        "  visibility: [\"//visibility:public\"]",
        "  tags: [\"stable\"]\n  visibility: [\"//visibility:public\"]",
    );
    let run = example.run(&["build", "//cmd/web", "--explain"]);
    run.ok();

    assert_eq!(before, keys(&run), "adding a tag moved a key");
    assert_eq!(
        status(&run, "link //cmd/web"),
        "cached",
        "adding a tag caused a relink:\n{}",
        indent(&run.all())
    );
}

/// The toolchain is part of every key, because an artifact built by a
/// different compiler is a different artifact (HERMETICITY:116).
///
/// The `sha256` half of the pin rather than the `version` half, and that is a
/// consequence of the pin now being *enforced*: a `REPO.buri` naming a version
/// this toolchain is not gets exit 2 before a key is computed
/// (`hermeticity.rs`), so the version cannot be moved in a live repository and
/// still be observed. Both fields go into the key the same way, and the
/// sentinel — a `sha256` of nothing but zeros — stays a sentinel while its
/// bytes change, which is what makes it the field this can move.
#[test]
fn changing_the_toolchain_pin_changes_every_key() {
    let example = Scratch::copy_of("explain-toolchain", &example_repo());
    let before = keys(example.run(&["build", "//cmd/web", "--explain"]).ok());

    example.edit("REPO.buri", "sha256: \"0000", "sha256: \"00000");
    let after = keys(example.run(&["build", "//cmd/web", "--explain"]).ok());

    assert_eq!(before.len(), after.len(), "the action graph changed shape");
    for (action, key) in &before {
        assert_ne!(
            key, &after[action],
            "{action} kept its key across a toolchain change"
        );
    }
}

/// A test suite's sources reach the suite and nothing else. This is the row
/// the design is proudest of: editing a test may not cost a rebuild of the
/// thing under test (HERMETICITY:117).
#[test]
fn editing_a_test_source_reaches_only_its_suite() {
    let example = Scratch::copy_of("explain-test-edit", &example_repo());
    let before = keys(example.run(&["build", "//cmd/web", "--explain"]).ok());
    let suite_before = keys(example.run(&["test", "//lib/money", "--explain"]).ok());

    example.edit(
        "lib/money/test/cents.buri",
        "test \"",
        "test \"x ",
    );
    let run = example.run(&["build", "//cmd/web", "--explain"]);
    run.ok();

    assert_eq!(
        before,
        keys(&run),
        "editing a test source changed a production key"
    );
    // The positive half: the suite's own key did move, so the test above is
    // about where the edit stopped rather than about an edit that did nothing.
    let suite_after = keys(example.run(&["test", "//lib/money", "--explain"]).ok());
    assert_ne!(
        suite_before["test //lib/money"], suite_after["test //lib/money"],
        "editing a test source did not change its own suite's key"
    );
    assert_eq!(
        status(&run, "link //cmd/web"),
        "cached",
        "editing a test source forced a relink:\n{}",
        indent(&run.all())
    );
}

/// `--force` is the way past the cache, so it is an optimisation rather than
/// something a user cannot get out from under.
#[test]
fn force_turns_every_hit_into_a_run() {
    let example = Scratch::copy_of("explain-force", &example_repo());
    example.run(&["build", "//cmd/web", "--explain"]).ok();

    let cached = example.run(&["build", "//cmd/web", "--explain"]);
    assert_eq!(status(&cached, "link //cmd/web"), "cached");

    let forced = example.run(&["build", "//cmd/web", "--explain", "--force"]);
    forced.ok();
    assert_eq!(
        status(&forced, "link //cmd/web"),
        "run",
        "--force was served from the cache:\n{}",
        indent(&forced.all())
    );
}

/// A test suite is an ordinary action, so the whole of the table applies to it:
/// unchanged inputs are served from the cache, `--force` goes around it, and an
/// edit to any input the suite has invalidates it (TESTING.md:392-395).
///
/// Read off `--explain` rather than off the summary line, because the summary
/// says how many *tests* were cached and this is a claim about the *action* —
/// a suite that quietly stopped running would satisfy the summary and not this.
#[test]
fn a_test_suite_is_cached_and_force_re_runs_it() {
    let example = Scratch::copy_of("explain-test-cache", &example_repo());

    let first = example.run(&["test", "//lib/money", "--explain"]);
    first.ok();
    assert_eq!(status(&first, "test //lib/money"), "run");

    let again = example.run(&["test", "//lib/money", "--explain"]);
    again.ok();
    assert_eq!(
        status(&again, "test //lib/money"),
        "cached",
        "an unchanged suite re-ran:\n{}",
        indent(&again.all())
    );
    // The count in the summary is the same fact in the words a person reads.
    assert!(
        again.stdout.contains("cached)"),
        "a cached suite did not say so in its summary:\n{}",
        indent(&again.all())
    );

    let forced = example.run(&["test", "//lib/money", "--explain", "--force"]);
    forced.ok();
    assert_eq!(
        status(&forced, "test //lib/money"),
        "run",
        "--force was served from the cache:\n{}",
        indent(&forced.all())
    );

    // Still cached afterwards: `--force` goes around the entry rather than
    // through it.
    let after = example.run(&["test", "//lib/money", "--explain"]);
    after.ok();
    assert_eq!(status(&after, "test //lib/money"), "cached");

    // And the negative twin, from each of the two directions a suite's key can
    // move: the code under test, and the suite's own source.
    example.edit("lib/money/cents.buri", "fn fromCents", "fn fromCents ");
    let edited = example.run(&["test", "//lib/money", "--explain"]);
    edited.ok();
    assert_eq!(
        status(&edited, "test //lib/money"),
        "run",
        "editing the library under test did not uncache its suite:\n{}",
        indent(&edited.all())
    );

    example.run(&["test", "//lib/money", "--explain"]).ok();
    example.edit("lib/money/test/cents.buri", "test \"", "test \"x ");
    let retested = example.run(&["test", "//lib/money", "--explain"]);
    retested.ok();
    assert_eq!(
        status(&retested, "test //lib/money"),
        "run",
        "editing a test source did not uncache its own suite:\n{}",
        indent(&retested.all())
    );
}

/// `--filter` and `--accept` are the two modes that must not be served from the
/// cache: one runs a different subset, and the other exists to write to the
/// source tree. A cache hit in either would be a silent wrong answer.
#[test]
fn filtering_and_accepting_never_come_from_the_cache() {
    let example = Scratch::copy_of("explain-test-modes", &example_repo());
    example.run(&["test", "//lib/store", "--explain"]).ok();
    assert_eq!(status(&example.run(&["test", "//lib/store", "--explain"]), "test //lib/store"), "cached");

    let filtered = example.run(&["test", "//lib/store", "--explain", "--filter=golden"]);
    filtered.ok();
    assert_eq!(
        status(&filtered, "test //lib/store"),
        "run",
        "a filtered run was served from the cache:\n{}",
        indent(&filtered.all())
    );

    let accepted = example.run(&["test", "//lib/store", "--explain", "--accept"]);
    accepted.ok();
    assert_eq!(
        status(&accepted, "test //lib/store"),
        "run",
        "--accept was served from the cache:\n{}",
        indent(&accepted.all())
    );

    // A filtered run must not leave its partial result behind for the next
    // whole one to find.
    let whole = example.run(&["test", "//lib/store", "--explain"]);
    whole.ok();
    assert_eq!(status(&whole, "test //lib/store"), "cached");
    assert_eq!(
        whole.tests_passed(),
        example.run(&["test", "//lib/store", "--force"]).tests_passed(),
        "a filtered run poisoned the cache with a partial result:\n{}",
        indent(&whole.all())
    );
}

// ---------------------------------------------------------------------------
// What the key is made of
// ---------------------------------------------------------------------------
//
// `build/cache.rs` asserts these on the `KeyBuilder`, where "the platform is in
// the key" is a statement about the key. What follows asserts them through the
// CLI, where it is a statement about a repository — the two are different
// claims, and a builder that composed correctly while `action_key` forgot to
// call it would satisfy only the first.

/// Rule identity — the label, the rule kind, and the ordered source paths — is
/// in the key alongside the contents. Renaming a source moves the key although
/// not one byte the compiler reads has changed, which is what "paths are in the
/// key" means when you can watch it.
///
/// A source nothing imports, so that the rename is *only* a rename: renaming a
/// module something imports means editing the import too, and then the contents
/// moved and the test proves nothing.
#[test]
fn renaming_a_source_moves_the_key_though_the_bytes_did_not() {
    let scratch = Scratch::repo("explain-rule-identity");
    scratch.write(
        "cmd/c/BUILD.buri",
        "binary {\n  sources: [\"extra.buri\"]\n  outputs: [{ platform: JS }]\n}\n",
    );
    scratch.write("cmd/c/main.buri", &program(3));
    scratch.write("cmd/c/extra.buri", "export fn spare(): Int { 1 }\n");
    let before = keys(scratch.run(&["build", "//cmd/c", "--explain"]).ok());

    let source = scratch.read("cmd/c/extra.buri");
    scratch.write("cmd/c/spare.buri", &source);
    std::fs::remove_file(scratch.path("cmd/c/extra.buri")).expect("removing the old name");
    scratch.edit("cmd/c/BUILD.buri", "\"extra.buri\"", "\"spare.buri\"");

    let run = scratch.run(&["build", "//cmd/c", "--explain"]);
    run.ok();
    let after = keys(&run);
    assert_ne!(
        before["compile //cmd/c"], after["compile //cmd/c"],
        "renaming a source left the key alone, so a rule's identity is not in it"
    );
    assert_eq!(
        status(&run, "link //cmd/c"),
        "run",
        "a renamed source was served from the cache:\n{}",
        indent(&run.all())
    );
}

/// "Dependencies enter as keys, not contents."
///
/// Observable from outside as this: a dependency's key and its dependent's key
/// move together, and the dependent's *own* contribution does not move at all.
/// The `compile` line for a closure member is that member's own identity and
/// sources and nothing from below it, so an edit downstairs that moved a
/// dependent's `compile` key would mean contents were leaking upward.
#[test]
fn a_dependency_reaches_its_dependent_as_a_key_and_not_as_contents() {
    let example = Scratch::copy_of("explain-dep-keys", &example_repo());
    let before = keys(example.run(&["build", "//cmd/web", "--explain"]).ok());

    example.edit("lib/money/cents.buri", "let whole = self.0 / 100;", "let whole = (self.0) / 100;");
    let after = keys(example.run(&["build", "//cmd/web", "--explain"]).ok());

    // The dependency moved, and the artifact that links it moved with it.
    assert_ne!(before["compile //lib/money"], after["compile //lib/money"]);
    assert_ne!(before["link //cmd/web"], after["link //cmd/web"]);
    // And every dependent's own contribution stayed exactly where it was —
    // which is only possible if what entered the link key was //lib/money's
    // key rather than //lib/money's bytes.
    for member in ["compile //lib/ledger", "compile //cmd/web"] {
        assert_eq!(
            before[member], after[member],
            "{member} moved on an edit to a dependency, so contents are entering keys"
        );
    }
}

/// A suite's key is built on the key of the code under test — `test_key` folds
/// the target's whole `action_key` in through `dependency` — so an edit to the
/// code moves the suite, and an edit to a package on nobody's path to it does
/// not.
///
/// Two separate repositories rather than two targets in one, because the
/// example monorepo is a connected graph on purpose: every library in it is
/// reachable from something, which is what makes it a good corpus for the
/// positive claims and a useless one for "nothing here can see that".
#[test]
fn a_suite_is_keyed_on_the_key_of_what_it_tests() {
    let scratch = Scratch::repo("explain-suite-keys");
    let library = |answer: i32| {
        format!("from \"//lib/n/inner\" export {{ answer }};\nfrom \"//lib/n/inner\" import {{ answer }};\n\nexport fn twice(): Int {{ answer() * {answer} }}\n")
    };
    scratch.write(
        "lib/n/BUILD.buri",
        "library {\n  sources: [\"inner.buri\"]\n  test { sources: [\"test/n.buri\"] }\n}\n",
    );
    scratch.write("lib/n/lib.buri", &library(2));
    scratch.write("lib/n/inner.buri", "export fn answer(): Int { 21 }\n");
    scratch.write(
        "lib/n/test/n.buri",
        "from \"//lib/n\" import { twice };\nfrom \"core/testing/assert\" import * as assert;\n\ntest \"twice\" {\n  assert.eq(twice(), 42);\n}\n",
    );
    // An unrelated package: nothing depends on it and it depends on nothing.
    scratch.write("lib/m/BUILD.buri", "library {\n  test { sources: [\"test/m.buri\"] }\n}\n");
    scratch.write("lib/m/lib.buri", "export fn one(): Int { 1 }\n");
    scratch.write(
        "lib/m/test/m.buri",
        "from \"//lib/m\" import { one };\nfrom \"core/testing/assert\" import * as assert;\n\ntest \"one\" {\n  assert.eq(one(), 1);\n}\n",
    );

    let n_before = keys(scratch.run(&["test", "//lib/n", "--explain"]).ok());
    let m_before = keys(scratch.run(&["test", "//lib/m", "--explain"]).ok());

    // An edit to the code under test, in a module the suite never names. It
    // computes the same answer, so the suite still passes and what is being
    // watched is the key rather than the verdict.
    scratch.write("lib/n/inner.buri", "export fn answer(): Int { 20 + 1 }\n");

    let n_after = keys(scratch.run(&["test", "//lib/n", "--explain"]).ok());
    let m_after = keys(scratch.run(&["test", "//lib/m", "--explain"]).ok());

    assert_ne!(
        n_before["test //lib/n"], n_after["test //lib/n"],
        "editing the code under test left its suite's key alone"
    );
    assert_eq!(
        m_before["test //lib/m"], m_after["test //lib/m"],
        "editing //lib/n moved a suite with no path to it"
    );
}
