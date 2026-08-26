//! `buri init`, driven as a user drives it.
//!
//! The scaffold's whole claim is that what it writes is a working repository,
//! and that claim is only worth anything if the real binary generates it and
//! the real binary then builds and tests it. Nothing here is asserted against
//! the constants in `commands/init.rs` — `init.rs`'s own unit tests do that,
//! and repeating it here would prove the table agrees with itself rather than
//! that a repository came out.
use crate::harness::*;

/// The one test: generate, then every command the page promises is clean on
/// what came out. A scaffold that stops compiling is a broken `buri init`
/// however well the bytes match what the binary ships, and one the formatter
/// would rewrite is a first session that opens with a diff nobody asked for.
#[test]
fn a_generated_repository_builds_and_tests() {
    // Deliberately not `Scratch::repo`: `buri init` is refused inside one, and
    // this directory is the empty one a user starts from.
    let scratch = Scratch::empty("init");
    scratch.run(&["init"]).ok().says("wrote REPO.buri").says("wrote cmd/hello/main.buri");
    assert!(scratch.path(".gitignore").is_file(), "the ignore file lands with its dot");
    assert!(
        scratch.path(".claude/skills/buri-cli/SKILL.md").is_file(),
        "`buri init` installs the agent skills"
    );

    scratch.run(&["build", "//..."]).ok();
    let tested = scratch.run(&["test", "//..."]);
    tested.ok();
    assert!(tested.tests_passed() >= 1, "the generated suite must actually run a test");

    // The other three claims `buri docs cli init` makes, in the same run: the
    // scaffold is the formatter's own output, so a first `format --check` and
    // a first `gen --check` must both find nothing to say.
    scratch.run(&["lint", "//..."]).ok();
    scratch.run(&["format", "--check"]).ok();
    scratch.run(&["gen", "--check", "//..."]).ok();

    // A second run is the safety contract, and it is the CLI's exit code that
    // carries it.
    scratch.run(&["init"]).exits(2).says("already a Buri repository");

    // The other half of that contract, and the one that is not a collision: a
    // `REPO.buri` nested inside a repository is no root at all, so writing one
    // would leave the enclosing repository worse than `buri init` found it.
    // Once with a relative target, once from a subdirectory, because those are
    // the two spellings whose upward walk had to be made to work.
    scratch.run(&["init", "packages/nested"]).exits(2).says("is inside the Buri repository at");
    run_in(&scratch.path("lib/greeting"), &["init"])
        .exits(2)
        .says("is inside the Buri repository at");
    assert!(!scratch.path("packages").exists(), "a refused run writes nothing at all");
}
