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
#[test]
fn changing_the_toolchain_pin_changes_every_key() {
    let example = Scratch::copy_of("explain-toolchain", &example_repo());
    let before = keys(example.run(&["build", "//cmd/web", "--explain"]).ok());

    example.edit("REPO.buri", "version: \"0.3.0\"", "version: \"0.3.1\"");
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
