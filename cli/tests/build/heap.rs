//! **The heap check, and the proof that it has teeth.**
//!
//! `cli/tests/harness/mod.rs`'s heap-check section makes one claim about this
//! whole binary and its siblings: *every Buri program a suite runs is asked, on
//! its way out, whether it gave back every block it took, and a program that
//! did not is a failing test.* A claim like that is worth exactly as much as
//! the evidence that it is switched on — a harness that quietly stopped setting
//! the variable, or a `buri test` that stopped forwarding it into the binaries
//! it spawns, would leave every suite in this repository green and asserting
//! nothing about memory at all.
//!
//! So this module asserts the mechanism itself, from the outside, in the places
//! a Buri program actually runs:
//!
//! | Test | What it shows |
//! |---|---|
//! | [`an_artifact_buri_ran_is_asked_for_its_blocks_back`] | `buri run`'s native artifact answers the check, and answers `live=0`. |
//! | [`buri_test_runs_its_test_binaries_under_the_heap_check`] | the check reaches the *test binary* `buri test` spawns, which has an explicit environment of its own and had to be told. |
//! | [`a_release_artifact_is_asked_for_its_blocks_back_or_is_refused_by_name`] | the same of the *optimizing* pipeline, on a toolchain that has one — and a refusal by name on one that has not. |
//!
//! Nothing here reads a pass, an IR or a counter: what every row observes is a
//! line the runtime printed on standard error and the status a command exited
//! with, both of which a person running `BURI_RT_HEAP_CHECK=1 buri test` sees.
//!
//! **The other direction — a program that really leaks is really reported — is
//! not a row here, because there is no longer a program to write it with.** It
//! used to be `core/actor`'s discarded-message defect, provoked on purpose and
//! pinned at its exact size; that defect is fixed, and nothing a *correct*
//! program can do leaks (`proc.exit` is the one way to stop while holding
//! blocks, and the runtime quiets the audit for it by name, because a program
//! that chose where to stop is entitled to be holding values). A simulated
//! report would prove nothing a switched-off check could not fake. Where that
//! direction is still asserted is over the native corpus, on real defects with
//! exact counts asserted both ways: `native::agreement`'s `agree_leaking` rows
//! and `native::conformance`'s leak ledger.
//!
//! ```text
//! cargo test -p buri --test build heap::
//! ```

use crate::harness::{ci, Run, Scratch};

/// The platform a binary here declares.
///
/// **Named, rather than left to the default.** A `binary` that declares no
/// outputs builds for JavaScript, and a JavaScript artifact is a module `bun`
/// runs — the heap check is the *native* runtime's, so a defaulted binary would
/// be a test that asserted nothing and said so nowhere.
fn host_platform() -> &'static str {
    if cfg!(target_os = "macos") {
        "MACOS"
    } else {
        "LINUX"
    }
}

fn native_binary(platform: &str) -> String {
    format!("binary {{\n  outputs: [{{ platform: {platform} }}]\n}}\n")
}

const LIBRARY: &str = "library {\n    test {\n        sources: [\"test/counting.buri\"]\n    }\n}\n";

/// A program that allocates. A program that does not would answer `allocated=0`
/// and prove nothing: the audit it passed is the audit of an empty heap.
const COUNTING: &str = r#"from "core/host" import { stdout, alloc };
from "core/io" import * as io;

export fn main(): Result<(), Str> {
    let letters = [1, 2, 3].map(alloc, fn(n) => "buri".repeat(alloc, n));
    let _ = io.println(stdout, "${letters.len()}").ignore();
    .Ok(())
}
"#;

/// The same, as a test block, so that the binary `buri test` spawns has a heap
/// to be audited.
const COUNTING_TEST: &str = r#"from "core/effect" import { Alloc };
from "core/host/testing" import { alloc };
from "core/testing/assert" import * as assert;

test "a suite that allocates" {
    let ctx = context { Alloc: alloc() };
    let letters = [1, 2, 3].mapCtx(ctx, fn(c, n) => "buri".repeat(c, n));
    assert.eq(letters.len(), 3);
}
"#;

/// Whether the run reached the native runtime at all.
///
/// The heap check is that runtime's, so a toolchain built without a native
/// backend, on a host no stencil library is built for, or on a machine with no
/// C toolchain, runs nothing these rows can ask. That is a *host's* answer and
/// not a runner's: `ci::skipped` prints it here and panics under `BURI_CI=1`,
/// where every one of those inputs is asserted before the suite starts.
fn asked(run: &Run) -> bool {
    run.all().contains("buri heap check:")
}

fn no_native_run(what: &str, run: &Run) {
    ci::skipped(
        "build::heap",
        &format!("`{what}` never reached the native runtime, so there was no heap check to read:\n{}", run.all()),
    );
}

/// A native artifact `buri run` executed answers the check, and answers clean.
///
/// `BURI_RT_HEAP_REPORT=1` is what turns a silent pass into a receipt — the
/// runtime prints one line naming what it allocated and what was live at exit —
/// and reading that line is how this test can tell "the check was on and the
/// program was clean" from "the check was never on".
#[test]
fn an_artifact_buri_ran_is_asked_for_its_blocks_back() {
    let scratch = Scratch::repo("heap-run");
    scratch.write("cmd/counting/BUILD.buri", &native_binary(host_platform()));
    scratch.write("cmd/counting/main.buri", COUNTING);

    let run =
        scratch.run_with_env(&["run", "//cmd/counting"], &[("BURI_RT_HEAP_REPORT", "1")]);
    if !asked(&run) {
        no_native_run("buri run //cmd/counting", &run);
        return;
    }
    run.ok().says("buri heap check: ok (").says("live=0");
    assert!(
        !run.all().contains("allocated=0"),
        "the artifact allocated nothing, so the audit it passed was of an empty heap:\n{}",
        run.all()
    );
}

/// The same of the binary `buri test` spawns, which is the case that had to be
/// arranged rather than inherited.
///
/// An action's process is given an **explicit** environment
/// (`cli/src/build/spawn.rs`), so a variable set on `buri` reaches a test
/// binary only because `cli/src/commands/test.rs` forwards it by name. This is
/// the row that fails the day that forwarding is dropped — and it would fail
/// quietly in every other suite in the repository, which is why it is written
/// here.
#[test]
fn buri_test_runs_its_test_binaries_under_the_heap_check() {
    let scratch = Scratch::repo("heap-suite");
    scratch.write("lib/counting/BUILD.buri", LIBRARY);
    scratch.write("lib/counting/lib.buri", "export fn version(): Int { 1 }\n");
    scratch.write("lib/counting/test/counting.buri", COUNTING_TEST);

    let run = scratch
        .run_with_env(&["test", "//lib/counting", "--force"], &[("BURI_RT_HEAP_REPORT", "1")]);
    if !asked(&run) {
        no_native_run("buri test //lib/counting", &run);
        return;
    }
    run.ok().says("buri heap check: ok (").says("live=0");
    assert_eq!(run.tests_passed(), 1, "the suite did not run:\n{}", run.all());
    assert!(
        !run.all().contains("allocated=0"),
        "the test binary allocated nothing, so the audit it passed was of an empty heap:\n{}",
        run.all()
    );
}

/// The release pipeline answers the same question, or says why it cannot.
///
/// **Two arms and no skip.** `--release` routes to the optimizing backend
/// (`backend::select`), which a default toolchain is built without — so this
/// row cannot assert the receipt unconditionally, and it must not quietly pass
/// on the toolchain that has no LLVM either. What it asserts instead is total:
/// a release artifact this toolchain *did* build answers the check, and one it
/// would not build is a refusal that says so by name. Nothing is skipped,
/// nothing is gated, and the row means something on every host.
#[test]
fn a_release_artifact_is_asked_for_its_blocks_back_or_is_refused_by_name() {
    let scratch = Scratch::repo("heap-release");
    scratch.write("cmd/counting/BUILD.buri", &native_binary(host_platform()));
    scratch.write("cmd/counting/main.buri", COUNTING);

    let run = scratch
        .run_with_env(&["run", "//cmd/counting", "--release"], &[("BURI_RT_HEAP_REPORT", "1")]);
    if asked(&run) {
        run.ok().says("buri heap check: ok (").says("live=0");
        assert!(
            !run.all().contains("allocated=0"),
            "the release artifact allocated nothing, so the audit it passed was of an \
             empty heap:\n{}",
            run.all()
        );
        return;
    }
    assert_ne!(
        run.code, 0,
        "a release artifact ran and was never asked for its blocks back:\n{}",
        run.all()
    );
    run.says("native-artifact-not-available");
}
