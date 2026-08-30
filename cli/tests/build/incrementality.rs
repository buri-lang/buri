//! What the cache may and may not do.
//!
//! These are the tests that interleave edits with rebuilds, so they are Rust
//! rather than manifests: a repository case runs commands against one tree,
//! and the claims here are about what changes *between* two states of it.
//!
//! The incrementality table under "What incrementality looks like" in
//! `buri docs build/hermeticity` is written in terms of *which actions run*,
//! which nothing could observe from outside the toolchain until `--explain`. These read that transcript. Keys are
//! compared between two states of one tree and never recorded, because a key
//! includes the toolchain version and would move on every release.
use crate::harness::*;

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
from "core/effect" import {{ Alloc, Stdout }};
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

/// The rows of `buri docs build/hermeticity`'s incrementality table that this
/// toolchain can currently answer: an edit reaches the target it is in and the
/// binary above it, and nothing sideways.
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

// The toolchain is part of every key, because an artifact built by a different
// compiler is a different artifact (`buri docs build/hermeticity`) — and there
// is no test for it here any more. There used to be: `REPO.buri` pinned a toolchain by
// `sha256`, the pin went into the key, and moving it between *unpinned* and
// *pinned to this executable* was a change of toolchain identity a live
// repository could be walked through. The pin was removed, so a repository has
// nothing left to say about which compiler builds it, and the key's toolchain
// identity is `arguments::VERSION` — a constant compiled into the binary under
// test, which cannot be moved without a second binary to move it to.
//
// What survives is `cache::tests::the_toolchain_version_is_in_every_key`, which
// rebuilds the key field by field and fails if the version stops entering it.
// That is the honest boundary: the property is about the key's composition, and
// a test that drives one binary can only ever observe one version of it.

/// A test suite's sources reach the suite and nothing else. This is the row
/// the design is proudest of: editing a test may not cost a rebuild of the
/// thing under test (`buri docs build/hermeticity`).
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

/// The third direction a suite's key can move: a library the *test* code
/// depends on.
///
/// `test { dependencies }` is deliberately not part of the production closure —
/// a test dependency is not a dependency of the thing being shipped, so it must
/// not drag its tags into the tag closure — and
/// `test_key` walked the production closure. So the helper's sources were
/// compiled into the suite and hashed into nothing, and editing one served the
/// previous verdict for a suite whose code had changed. That is the worst
/// failure a cache has: not slow, wrong.
///
/// The assertion is on behaviour rather than on the transcript. A helper that
/// starts answering something else must turn a passing suite red, because a
/// `run` line with a stale verdict behind it would satisfy a test that only
/// read `--explain`.
#[test]
fn editing_a_test_only_dependency_re_runs_the_suite() {
    let scratch = Scratch::repo("test-dep-key");
    scratch.write(
        "lib/helper/BUILD.buri",
        "library {\n    sources: [\"h.buri\"]\n    visibility: [\"//...\"]\n}\n",
    );
    scratch.write("lib/helper/lib.buri", "from \"//lib/helper/h\" export { double };\n");
    scratch.write("lib/helper/h.buri", "export fn double(n: I64): I64 { n * 2 }\n");
    scratch.write(
        "lib/subject/BUILD.buri",
        "library {\n    sources: [\"x.buri\"]\n\n    test {\n        sources: \
         [\"test/x.buri\"]\n        dependencies: [\"//lib/helper\"]\n    }\n}\n",
    );
    scratch.write("lib/subject/lib.buri", "from \"//lib/subject/x\" export { triple };\n");
    scratch.write("lib/subject/x.buri", "export fn triple(n: I64): I64 { n * 3 }\n");
    scratch.write(
        "lib/subject/test/x.buri",
        "from \"//lib/subject\" import { triple };\nfrom \"//lib/helper\" import { double };\n\
         from \"core/testing/assert\" import * as assert;\n\ntest \"triple against double\" {\n  \
         assert.eq(triple(2), double(3));\n}\n",
    );

    let first = scratch.run(&["test", "//lib/subject", "--explain"]);
    first.ok();
    assert_eq!(status(&first, "test //lib/subject"), "run");
    assert_eq!(status(&scratch.run(&["test", "//lib/subject", "--explain"]), "test //lib/subject"), "cached");

    // `double` now answers something else, so `triple(2) == double(3)` is
    // false. A suite served from the cache would still say it passed.
    scratch.write("lib/helper/h.buri", "export fn double(n: I64): I64 { n * 5 }\n");
    let edited = scratch.run(&["test", "//lib/subject", "--explain"]);
    assert_eq!(
        status(&edited, "test //lib/subject"),
        "run",
        "editing a test-only dependency did not uncache the suite:\n{}",
        indent(&edited.all())
    );
    edited.exits(1);
    assert_eq!(
        edited.tests_passed(),
        0,
        "the cache served a verdict for code that had changed:\n{}",
        indent(&edited.all())
    );

    // That the watch set holds the same file is `build/watch.rs`'s to assert, beside
    // the rest of the declared set: `watch::inputs` mirrors this key, and the
    // two enumerations are checked where the enumeration is.
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
// The native row
// ---------------------------------------------------------------------------

/// The `codegen` row of the incrementality table: a native build emits one
/// object per codegen unit, an edit re-emits the unit it landed in, the
/// siblings come out of the cache, and the link runs again because one of its
/// inputs moved.
///
/// One test with two halves, because the honest boundary moves: until a native
/// backend is compiled into this toolchain there is no `codegen` action to
/// watch, and the thing that *is* true — that a native output is refused, in
/// exactly the words `repositories/cli/output_selection` pins — is what a test
/// can hold the toolchain to. When a backend lands, the same invocation starts
/// producing the transcript and the second half takes over, so this does not
/// have to be rewritten to become the test it is meant to be.
///
/// The claim itself is proved *now*, below the CLI, in `tests/native/link.rs`:
/// `editing_one_unit_re_emits_exactly_that_unit` drives the same
/// `codegen_units` path this transcript reports, with objects a C compiler
/// made. What is missing here is the wiring, not the behaviour.
#[test]
fn a_native_build_re_emits_the_unit_an_edit_landed_in() {
    let host = if cfg!(target_os = "macos") { "macos" } else { "linux" };
    let scratch = Scratch::repo("explain-native");
    scratch.write(
        "cmd/c/BUILD.buri",
        &format!("binary {{\n  outputs: [{{ platform: {} }}]\n}}\n", host.to_uppercase()),
    );
    scratch.write("cmd/c/main.buri", &program(1));

    let selector = format!("--output={host}");
    let first = scratch.run(&["build", "//cmd/c", &selector, "--explain"]);
    if first.all().contains("backend is not implemented") {
        // The half that holds until the CLI is wired to a native backend:
        // refused, and refused in the words the repository case pins — a
        // toolchain that started half-building a native artifact would fail
        // here rather than in a golden file nobody reran.
        first.exits(1);
        assert!(
            first.stderr.contains(&format!("the {host} backend is not implemented")),
            "a native output was refused in words nothing pins:\n{}",
            indent(&first.all())
        );
        assert!(
            !first.stdout.contains("codegen"),
            "a toolchain with no native backend reported a codegen action:\n{}",
            indent(&first.all())
        );
        return;
    }

    first.ok();
    let units: Vec<String> = first
        .stdout
        .lines()
        .filter(|l| l.split_whitespace().nth(1) == Some("codegen"))
        .map(|l| l.to_string())
        .collect();
    assert!(!units.is_empty(), "a native build reported no codegen action:\n{}", indent(&first.all()));

    // Nothing edited: every unit is served from the cache and the link is
    // skipped entirely, which is the case a watch loop hits on every keystroke
    // inside a comment.
    let again = scratch.run(&["build", "//cmd/c", &selector, "--explain"]);
    again.ok();
    for line in again.stdout.lines().filter(|l| l.split_whitespace().nth(1) == Some("codegen")) {
        assert!(
            line.starts_with("cached"),
            "an unchanged unit was re-emitted:\n{}",
            indent(&again.all())
        );
    }
    assert_eq!(status(&again, "link //cmd/c"), "cached");

    // One module edited. Exactly one unit re-emits, and the link runs again
    // because the ordered list of unit keys is what the link key is made of.
    scratch.write("cmd/c/main.buri", &program(2));
    let edited = scratch.run(&["build", "//cmd/c", &selector, "--explain"]);
    edited.ok();
    let ran = edited
        .stdout
        .lines()
        .filter(|l| l.split_whitespace().nth(1) == Some("codegen"))
        .filter(|l| l.starts_with("run"))
        .count();
    assert_eq!(ran, 1, "an edit reached {ran} units rather than one:\n{}", indent(&edited.all()));
    assert_eq!(status(&edited, "link //cmd/c"), "run");
}

/// A suite that names a native platform is compiled and run as a native
/// binary, and its verdict is cached on the same key a JavaScript run uses.
///
/// Two halves, for the same reason as the test above: on a toolchain that
/// cannot produce a binary for this host — no backend compiled in, no runtime
/// archive, no linker — the refusal is what is true, and it is the refusal
/// `repositories/testing/suite_platforms` pins. The platform is the *host's*,
/// because there is no cross-compilation (`ARCHITECTURE.md` §9) and a suite
/// naming the other one is refused whichever machine this is.
#[test]
fn a_suite_naming_the_host_platform_runs_natively() {
    let host = if cfg!(target_os = "macos") { "MACOS" } else { "LINUX" };
    let scratch = Scratch::repo("native-suite");
    scratch.write(
        "lib/n/BUILD.buri",
        &format!(
            "library {{\n  test {{\n    sources: [\"test/n.buri\"]\n    \
             platforms: [{host}]\n  }}\n}}\n"
        ),
    );
    scratch.write("lib/n/lib.buri", "export fn answer(): Int { 21 }\n");
    scratch.write(
        "lib/n/test/n.buri",
        "from \"//lib/n\" import { answer };\n\
         from \"core/testing/assert\" import * as assert;\n\
         \ntest \"answers\" {\n  assert.eq(answer(), 21);\n}\n",
    );

    let first = scratch.run(&["test", "//lib/n", "--explain"]);
    if first.all().contains("backend is not implemented") {
        first.exits(1);
        assert!(
            first.stderr.contains("platform-not-implemented"),
            "a native suite was refused in words nothing pins:\n{}",
            indent(&first.all())
        );
        return;
    }
    first.ok();
    assert_eq!(first.tests_passed(), 1, "a native suite ran no tests:\n{}", indent(&first.all()));
    // The objects and the link are the build's actions, reported under the
    // platform the suite named rather than under `js`.
    assert!(
        first.stdout.lines().any(|l| l.split_whitespace().nth(1) == Some("codegen")),
        "a native suite reported no codegen action:\n{}",
        indent(&first.all())
    );

    // The verdict cache is in front of all of it: nothing is compiled, linked
    // or executed a second time.
    let again = scratch.run(&["test", "//lib/n", "--explain"]);
    again.ok();
    assert_eq!(status(&again, "test //lib/n"), "cached");
    assert!(
        !again.stdout.contains("codegen"),
        "a cached suite still ran codegen:\n{}",
        indent(&again.all())
    );

    // And a failing test is a failing run: the process aborts, and the runner
    // reports the failure rather than the exit status.
    scratch.write("lib/n/lib.buri", "export fn answer(): Int { 22 }\n");
    let failed = scratch.run(&["test", "//lib/n"]);
    failed.exits(1);
    assert!(
        failed.all().contains("assert.eq failed"),
        "a native assertion failure did not name itself:\n{}",
        indent(&failed.all())
    );
}

// ---------------------------------------------------------------------------
// Which backend a suite that names none runs on
// ---------------------------------------------------------------------------
//
// Here rather than in a recorded repository case because the answer depends on
// the machine and on how this binary was built — a golden would have to be one
// of `macos`, `linux` or `js` and would be wrong on two hosts out of three —
// and because `--explain`'s rows already name the platform every action ran
// for, which is the evidence the question needs.

/// The default, in both directions at once: **natively, or a line saying why
/// not**.
///
/// Written as an equivalence rather than as an assertion that it went native,
/// for the reason `driver.rs`'s host-platform test gives: a toolchain built
/// with `--no-default-features`, a host outside macOS and Linux, and a machine
/// with no C toolchain all have to fall back, and a test asserting the
/// happy answer would pass for the wrong reason on the machine where it
/// matters. The two failure modes this catches are the two that matter — a
/// suite that fell back where it did not have to (silence would hide it, so
/// the note is what is asserted) and a suite that went native without saying so
/// where it should not have.
#[test]
fn a_suite_naming_no_platform_runs_natively_or_says_why_not() {
    let scratch = Scratch::repo("default-backend");
    scratch.write("lib/n/BUILD.buri", "library {\n  test {\n    sources: [\"test/n.buri\"]\n  }\n}\n");
    scratch.write("lib/n/lib.buri", "export fn answer(): Int { 21 }\n");
    scratch.write(
        "lib/n/test/n.buri",
        "from \"//lib/n\" import { answer };\n\
         from \"core/testing/assert\" import * as assert;\n\
         \ntest \"answers\" {\n  assert.eq(answer(), 21);\n}\n",
    );

    let run = scratch.run(&["test", "//lib/n", "--explain"]);
    run.ok();
    assert_eq!(run.tests_passed(), 1, "the default backend ran no tests:\n{}", indent(&run.all()));
    let platform = platform_of(&run, "test //lib/n");
    if platform == "js" {
        assert!(
            run.stderr.contains("//lib/n runs on javascript")
                || run.stderr.contains("names no platform runs on javascript"),
            "a suite fell back to javascript without saying why:\n{}",
            indent(&run.all())
        );
    } else {
        assert!(
            run.stdout.lines().any(|l| l.split_whitespace().nth(1) == Some("codegen")),
            "a suite reported a native platform and no codegen action:\n{}",
            indent(&run.all())
        );
        assert!(
            !run.stderr.contains("runs on javascript"),
            "a native run explained itself as a fallback:\n{}",
            indent(&run.all())
        );
    }

    // `--output=js` is the escape hatch, and it is not a fallback: nothing is
    // explained, because nothing gave way.
    let js = scratch.run(&["test", "//lib/n", "--explain", "--force", "--output=js"]);
    js.ok();
    assert_eq!(platform_of(&js, "test //lib/n"), "js");
    assert!(
        !js.stderr.contains("runs on javascript"),
        "an asked-for JavaScript run was reported as a fallback:\n{}",
        indent(&js.all())
    );

    // And `--accept` goes there on its own, whatever the default is: the mode
    // rewrites a golden file from the two sides of a failed comparison, and
    // only the JavaScript runner reports them.
    let accepted = scratch.run(&["test", "//lib/n", "--explain", "--accept"]);
    accepted.ok();
    assert_eq!(
        platform_of(&accepted, "test //lib/n"),
        "js",
        "--accept ran somewhere it has no diff to accept from:\n{}",
        indent(&accepted.all())
    );
}

/// A suite the native backend cannot compile is **refused**, and naming a
/// platform is what un-refuses it.
///
/// It used to be rerouted onto JavaScript with a note on stderr, which is how a
/// named gap becomes a wrong answer: the suite passes, on a backend nobody
/// chose, and the note goes into a stream a passing run's reader does not read
/// (buri-lang/buri#4). `json.decode` is the gap here because no native backend
/// has a body for it and the corpus already says so — if that ever changes,
/// this test fails by passing the first run, which is the right way round.
#[test]
fn a_suite_the_native_backend_cannot_compile_is_refused() {
    let scratch = Scratch::repo("native-gap");
    scratch.write("lib/g/BUILD.buri", "library {\n  test {\n    sources: [\"test/g.buri\"]\n  }\n}\n");
    scratch.write("lib/g/lib.buri", "export fn nothing(): Int { 0 }\n");
    scratch.write(
        "lib/g/test/g.buri",
        "from \"core/testing/assert\" import * as assert;\n\
         from \"core/testing/context\" import { Hermetic };\n\
         from \"core/json\" import * as json;\n\
         \ntest \"decodes\" {\n\
         \x20 let ctx = Hermetic();\n\
         \x20 let parsed = assert.ok(json.parse(ctx, \"1\"));\n\
         \x20 let n: Float = assert.ok(json.decode(ctx, parsed));\n\
         \x20 assert.eq(n, 1.0);\n}\n",
    );

    let run = scratch.run(&["test", "//lib/g"]);
    // On a host with no native backend the default is JavaScript already, and
    // there is no gap to refuse — the suite simply runs.
    if run.stderr.contains("names no platform runs on javascript") {
        run.ok();
        return;
    }
    assert!(
        run.stderr.contains("cannot run natively")
            && run.stderr.contains("no implementation of json.decode"),
        "a native gap did not refuse the suite:\n{}",
        indent(&run.all())
    );
    assert!(
        run.stderr.contains("platforms: [JS]"),
        "the refusal did not say what to do about it:\n{}",
        indent(&run.all())
    );
    assert!(
        !run.stderr.contains("runs on javascript —"),
        "a native gap still rerouted the suite:\n{}",
        indent(&run.all())
    );

    // The suite says where it belongs, and then it runs there.
    scratch.write(
        "lib/g/BUILD.buri",
        "library {\n  test {\n    sources: [\"test/g.buri\"]\n    platforms: [JS]\n  }\n}\n",
    );
    let named = scratch.run(&["test", "//lib/g"]);
    named.ok();
    assert_eq!(named.tests_passed(), 1, "the named platform ran no tests:\n{}", indent(&named.all()));
}

/// The platform an `--explain` row names, which is its fourth field.
fn platform_of(run: &Run, action_and_label: &str) -> String {
    for line in run.stdout.lines() {
        let f: Vec<&str> = line.split_whitespace().collect();
        if f.len() == 5 && format!("{} {}", f[1], f[2]) == action_and_label {
            return f[3].to_string();
        }
    }
    panic!("no `{action_and_label}` line in:\n{}", indent(&run.all()));
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

// ---------------------------------------------------------------------------
// One binary for several suites
// ---------------------------------------------------------------------------
//
// A small suite's cost is a `cc` invocation and a first execution of a file the
// operating system has never run, both of them per *binary*. So the suites that
// name no platform are compiled into one binary per tag-compatible batch
// (`commands/test.rs::run_batches`, and TAGS.md "One binary for several
// suites"). What follows is the two halves of that being an optimisation and
// nothing else: the batching is real, and the cache is still per suite.

/// A helper that writes a package with one suite of one passing test, tagged as
/// asked.
fn suite_package(scratch: &Scratch, name: &str, tags: &str) {
    scratch.write(
        &format!("lib/{name}/BUILD.buri"),
        &format!("library {{\n{tags}  test {{ sources: [\"test/{name}.buri\"] }}\n}}\n"),
    );
    scratch.write(&format!("lib/{name}/lib.buri"), "export fn one(): Int { 1 }\n");
    scratch.write(
        &format!("lib/{name}/test/{name}.buri"),
        &format!(
            "from \"//lib/{name}\" import {{ one }};\n\
             from \"core/testing/assert\" import * as assert;\n\
             \ntest \"one\" {{\n  assert.eq(one(), 1);\n}}\n"
        ),
    );
}

/// Every `--explain` row for one action, as `<status> <label>`.
fn rows(run: &Run, action: &str) -> Vec<String> {
    run.stdout
        .lines()
        .map(|l| l.split_whitespace().collect::<Vec<&str>>())
        .filter(|f| f.len() == 5 && f[1] == action)
        .map(|f| format!("{} {}", f[0], f[2]))
        .collect()
}

/// Suites that may share a binary do, and suites whose tags forbid each other
/// do not.
///
/// The predicate is `check_tags`' own, applied to the union: a batch is one
/// artifact, and two tags that forbid each other may not both be in one. So a
/// repository with an untagged pair and a `server` suite links them together —
/// `server` forbids nothing they carry — and the `client` suite is a second
/// artifact, which is exactly what it would have been without the tags being
/// consulted at all.
///
/// Two halves, as the tests above have: a toolchain that cannot build a binary
/// for this host runs every suite on JavaScript, where there is no link to
/// count.
#[test]
fn suites_share_a_binary_only_where_their_tags_allow_it() {
    let scratch = Scratch::repo("batch-tags");
    scratch.write(
        "REPO.buri",
        "tag {\n  name: \"server\"\n  doc: \"runs on infrastructure we operate\"\n  \
         forbids { tags: [\"client\"] }\n}\n\n\
         tag {\n  name: \"client\"\n  doc: \"ships to a user's machine\"\n}\n",
    );
    suite_package(&scratch, "a", "");
    suite_package(&scratch, "b", "");
    suite_package(&scratch, "c", "  tags: [\"server\"]\n");
    suite_package(&scratch, "d", "  tags: [\"client\"]\n");

    let run = scratch.run(&["test", "//...", "--explain"]);
    run.ok();
    assert_eq!(run.tests_passed(), 4, "a batched run lost a test:\n{}", indent(&run.all()));
    // Every suite is still its own `test` action, batched or not — that is the
    // cache statement, and it is per suite whatever produced the verdict.
    assert_eq!(rows(&run, "test").len(), 4, "a suite lost its own action:\n{}", indent(&run.all()));

    let links = rows(&run, "link");
    if links.is_empty() {
        // No native backend, no runtime archive, or no C toolchain: every suite
        // ran on JavaScript, which links nothing and says so.
        assert!(
            run.stderr.contains("runs on javascript"),
            "there were no links and nothing said why:\n{}",
            indent(&run.all())
        );
        return;
    }
    let shared = links
        .iter()
        .find(|l| l.contains("//lib/a") && l.contains("//lib/b") && l.contains("//lib/c"))
        .unwrap_or_else(|| {
            panic!("three compatible suites did not share a binary:\n{}", indent(&run.all()))
        });
    assert!(
        !shared.contains("//lib/d"),
        "a `client` suite was linked with a `server` one:\n{}",
        indent(&run.all())
    );
    assert_eq!(
        links.len(),
        2,
        "four suites took {} links rather than two:\n{}",
        links.len(),
        indent(&run.all())
    );
}

/// A batch is an execution strategy, not a cache key.
///
/// Two suites that ran in one binary have two verdicts under two keys, and an
/// edit that reaches one of them re-runs one of them. The second run is where
/// the claim is: if batching had merged anything, editing `//lib/a` would have
/// invalidated `//lib/b` as well — and if the batch had cached under its own
/// key, neither would have been served at all.
#[test]
fn a_batched_verdict_is_cached_and_invalidated_one_suite_at_a_time() {
    let scratch = Scratch::repo("batch-cache");
    suite_package(&scratch, "a", "");
    suite_package(&scratch, "b", "");

    let first = scratch.run(&["test", "//...", "--explain"]);
    first.ok();
    assert_eq!(first.tests_passed(), 2);
    assert_eq!(status(&first, "test //lib/a"), "run");
    assert_eq!(status(&first, "test //lib/b"), "run");

    let again = scratch.run(&["test", "//...", "--explain"]);
    again.ok();
    assert_eq!(
        (status(&again, "test //lib/a"), status(&again, "test //lib/b")),
        ("cached".to_string(), "cached".to_string()),
        "a verdict produced in a batch was not served from its suite's key:\n{}",
        indent(&again.all())
    );

    scratch.write("lib/a/lib.buri", "export fn one(): Int { 3 - 2 }\n");
    let edited = scratch.run(&["test", "//...", "--explain"]);
    edited.ok();
    assert_eq!(edited.tests_passed(), 2);
    assert_eq!(
        (status(&edited, "test //lib/a"), status(&edited, "test //lib/b")),
        ("run".to_string(), "cached".to_string()),
        "an edit to one suite reached the other's verdict:\n{}",
        indent(&edited.all())
    );
}

/// A failure in one suite of a batch does not cost another suite its report.
///
/// The whole of the isolation argument, and the thing a binary per suite was
/// buying: a failed assertion is an abort, so the process the batch runs in
/// stops — and the runner resumes at the block after the one that aborted, so
/// every other block still reports. Written against three suites so that the
/// failing one is in the middle: a report that only covered what came *before*
/// the abort would pass a two-suite version of this.
#[test]
fn one_suites_failure_leaves_the_others_reported() {
    let scratch = Scratch::repo("batch-isolation");
    suite_package(&scratch, "a", "");
    suite_package(&scratch, "b", "");
    suite_package(&scratch, "c", "");
    scratch.write("lib/b/lib.buri", "export fn one(): Int { 2 }\n");

    let run = scratch.run(&["test", "//...", "--explain"]);
    run.exits(1);
    assert_eq!(
        run.tests_passed(),
        2,
        "a failure in one suite took another suite's verdict with it:\n{}",
        indent(&run.all())
    );
    assert!(
        run.stdout.contains("FAIL //lib/b") && run.stdout.contains("assert.eq failed"),
        "the failing suite was not named:\n{}",
        indent(&run.all())
    );
    for label in ["//lib/a", "//lib/c"] {
        assert!(
            !run.stdout.contains(&format!("FAIL {label}")),
            "{label} was reported as failing:\n{}",
            indent(&run.all())
        );
    }
    // And the two that passed are cacheable on their own: a suite's verdict is
    // its own whatever its neighbour did.
    let again = scratch.run(&["test", "//...", "--explain"]);
    again.exits(1);
    assert_eq!(
        (
            status(&again, "test //lib/a"),
            status(&again, "test //lib/b"),
            status(&again, "test //lib/c")
        ),
        ("cached".to_string(), "run".to_string(), "cached".to_string()),
        "a failing suite's neighbours were not cached, or the failure was:\n{}",
        indent(&again.all())
    );
}

// ---------------------------------------------------------------------------
// `buri lint`
// ---------------------------------------------------------------------------

/// A library package: the build file, the surface, and one source module.
fn library_package(scratch: &Scratch, path: &str, exports: &str, deps: &str, source: &str) {
    scratch.write(
        &format!("{path}/BUILD.buri"),
        &format!(
            "library {{\n    sources: [\"unit.buri\"]\n{deps}    visibility: [\"//visibility:public\"]\n}}\n"
        ),
    );
    scratch.write(
        &format!("{path}/lib.buri"),
        &format!("from \"//{path}/unit\" export {{ {exports} }};\n"),
    );
    scratch.write(&format!("{path}/unit.buri"), source);
}

/// Three libraries: a leaf, a dependent of it, and a stranger to both.
fn three_libraries(name: &str) -> Scratch {
    let scratch = Scratch::repo(name);
    library_package(&scratch, "lib/money", "cents", "", "export fn cents(): I64 { 1 }\n");
    library_package(
        &scratch,
        "lib/ledger",
        "total",
        "    dependencies: [\"//lib/money\"]\n",
        "from \"//lib/money\" import { cents };\n\nexport fn total(): I64 { cents() }\n",
    );
    library_package(&scratch, "lib/kit", "one", "", "export fn one(): I64 { 1 }\n");
    scratch
}

/// The lint records are per target and keyed on the build graph, so a second
/// run over an untouched tree analyses nothing, and an edit reaches exactly the
/// targets whose closure holds the file that moved.
#[test]
fn a_lint_re_analyses_only_the_targets_an_edit_can_reach() {
    let scratch = three_libraries("lint-incremental");

    let first = scratch.run(&["lint", "//...", "--explain"]);
    first.ok();
    for label in ["//lib/money", "//lib/ledger", "//lib/kit"] {
        assert_eq!(status(&first, &format!("lint {label}")), "run", "{label} came from nowhere");
    }

    let again = scratch.run(&["lint", "//...", "--explain"]);
    again.ok();
    for label in ["//lib/money", "//lib/ledger", "//lib/kit"] {
        assert_eq!(
            status(&again, &format!("lint {label}")),
            "cached",
            "{label} was analysed twice"
        );
    }

    scratch.write("lib/money/unit.buri", "export fn cents(): I64 { 2 }\n");
    let after = scratch.run(&["lint", "//...", "--explain"]);
    after.ok();
    assert_eq!(
        (
            status(&after, "lint //lib/money"),
            status(&after, "lint //lib/ledger"),
            status(&after, "lint //lib/kit")
        ),
        ("run".to_string(), "run".to_string(), "cached".to_string()),
        "the edit did not reach exactly the closures that hold it:\n{}",
        indent(&after.all())
    );

    // The key names *which* target under *which* graph; what a source edit
    // moves is the closure inside the record, so one target keeps one entry
    // however many times its sources are edited.
    assert_eq!(keys(&first), keys(&after), "a source edit moved a lint key");
    eprintln!("lint: incremental across invocations, one record per target");
}

/// `buri clean` drops the records with the rest of the cache: the run after it
/// is a cold one.
#[test]
fn clean_drops_the_lint_records() {
    let scratch = three_libraries("lint-clean");
    scratch.run(&["lint", "//..."]).ok();
    scratch.run(&["lint", "//...", "--explain"]).ok().says("cached lint");

    scratch.run(&["clean"]).ok();
    assert!(!scratch.path(".buri/cache").exists(), "clean left the cache behind");

    // And the run after it re-analyses everything, which is what makes the
    // directory being gone mean the records are.
    scratch.run(&["lint", "//...", "--explain"]).ok().silent_about("cached lint");
}

/// A record this binary cannot read is a miss.
///
/// A record another toolchain wrote is unreachable rather than misread — its
/// key holds that toolchain's version, and this one never computes it — so what
/// is left to prove is that reaching a record which is not the shape this
/// version writes ends in re-analysis rather than in a wrong answer or a panic.
#[test]
fn a_lint_record_this_toolchain_cannot_read_is_a_miss() {
    let scratch = three_libraries("lint-garbled");
    scratch.write(
        "lib/kit/unit.buri",
        "from \"core/list\" import * as list;\n\nexport fn one(): I64 { 1 }\n",
    );
    let cold = scratch.run(&["lint", "//..."]);
    cold.exits(1).says("unused-import");

    let mut records = Vec::new();
    cache_files(&scratch.path(".buri/cache"), &mut records);
    assert!(!records.is_empty(), "the lint run wrote no records");
    for record in &records {
        std::fs::write(record, b"not a record any version of this format wrote").unwrap();
    }

    let warm = scratch.run(&["lint", "//..."]);
    assert_eq!(
        (warm.code, warm.stderr.clone()),
        (cold.code, cold.stderr.clone()),
        "an unreadable record changed what the command said"
    );
}

fn cache_files(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        if path.is_dir() {
            cache_files(&path, out);
        } else if path.file_name().is_some_and(|n| n != ".lock") {
            out.push(path);
        }
    }
}

/// `REPO.buri`'s `fail_on_finding` reaches a finding that came out of a record.
///
/// Promotion happens once, over the assembled report, rather than per target —
/// so a cached warning is promoted by the code that would have promoted a fresh
/// one. This is the test that says so out loud, because it is the one place a
/// per-target cache could plausibly have skipped a repository-wide rule.
#[test]
fn fail_on_finding_reaches_a_cached_finding() {
    let scratch = three_libraries("lint-promote");
    scratch.write("REPO.buri", "lint {\n    fail_on_finding: true\n}\n");
    scratch.write(
        "lib/kit/unit.buri",
        "from \"core/list\" import * as list;\n\nexport fn one(): I64 { 1 }\n",
    );

    let cold = scratch.run(&["lint", "//..."]);
    cold.exits(1).says("error: list is imported but not used");

    let warm = scratch.run(&["lint", "//...", "--explain"]);
    warm.exits(1);
    assert_eq!(status(&warm, "lint //lib/kit"), "cached", "the finding was not a cached one");
    assert_eq!(warm.stderr, cold.stderr, "a cached finding was not promoted");
}
