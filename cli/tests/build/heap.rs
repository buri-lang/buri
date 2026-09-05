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
//! | [`a_message_an_actor_never_delivered_is_this_repositorys_known_leak`] | a program that really does leak is really reported — the direction that cannot be faked by a check that is off — and the one leak this repository knows about is pinned at its exact size. |
//!
//! Nothing here reads a pass, an IR or a counter: what every row observes is a
//! line the runtime printed on standard error and the status a command exited
//! with, both of which a person running `BURI_RT_HEAP_CHECK=1 buri test` sees.
//!
//! ```text
//! cargo test -p buri --test build heap::
//! ```

use crate::harness::{ci, run_in_unchecked, Run, Scratch};

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

/// **A program that really leaks**, so that the check is proved in the
/// direction a check that is switched off cannot fake — and so that the one
/// leak this repository knows about is pinned rather than merely tolerated.
///
/// A message posted to an actor and still in its mailbox when `stop` closes it
/// has its payload released by nobody. `core/actor::stop` closes the mailbox
/// and then pops what is left, binding each block and letting it go; the block
/// itself is given back and what it pointed at is not. Three things narrow it:
/// a message that is *delivered* is clean even when the step throws its payload
/// away, an actor's state is clean however large it is, and `alloc.copyAcross`
/// — the deep copy every crossing is made with — is clean on its own. So it is
/// the discard path, and it is a defect in the code the compiler inserts rather
/// than in the ten lines of `core/actor` that read.
///
/// The payload is `70000` bytes because a block that size is given a mapping of
/// its own rather than a pooled one, which is what makes the number in the
/// runtime's sentence the payload's rather than an allocator's rounding.
///
/// **When the defect is fixed this test fails**, saying so and asking for its
/// own deletion together with `harness::KNOWN_LEAKS`'s row. That is deliberate,
/// and it is the ratchet `native::agreement`'s `agree_leaking` already keeps: a
/// fixture that is allowed to quietly stop leaking is a fixture that quietly
/// stops proving anything.
const LEAKING_TEST: &str = r#"from "core/actor" import * as actor;
from "core/actor" import { Actor, Reply };
from "core/effect" import { Alloc, Tasks };
from "core/host/testing" import { alloc, tasks };
from "core/testing/assert" import * as assert;

enum Keep {
    Put(Str),
    Get(Reply<Str>),
}

fn keeper<C: Alloc + Tasks>(initial: Str): Actor<C, Str, Keep> {
    Actor {
        state: initial,
        step: fn(c, held, message) => {
            match (message) {
                .Put(next) => next,
                .Get(reply) => {
                    let _ = reply.answer(c, held).ignore();
                    held
                },
            }
        },
    }
}

test "a message a stop discards" {
    let ctx = context { Alloc: alloc(), Tasks: tasks() };
    let address = actor.start(ctx, keeper(""));
    let _ = address.send(ctx, .Put("d".repeat(ctx, 70000))).ignore();
    assert.eq(address.stop(ctx), .Ok(()));
}
"#;

/// The sentence the runtime prints about [`LEAKING_TEST`], in full.
///
/// The **exact** count, asserted in both directions: a program that leaks a
/// block more is a different defect and this test says so, and a program that
/// has stopped leaking is the fix landing and this test says that instead.
const LEAKING_SAID: &str =
    "buri heap check: leak: 1 block(s) and 70000 byte(s) were allocated and never freed";

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

/// And the direction that matters: a program that leaks is reported, as a
/// failure a person reading `buri test` cannot miss.
///
/// [`run_in_unchecked`] rather than `Scratch::run`, because the harness's own
/// assertion would panic on this run before the test could read it — which is
/// the assertion working, and is exactly what this row exists to demonstrate
/// from the other side.
///
/// See [`LEAKING_TEST`] for what the defect is and for what to do on the day it
/// is fixed.
#[test]
fn a_message_an_actor_never_delivered_is_this_repositorys_known_leak() {
    let scratch = Scratch::repo("heap-leak");
    scratch.write("lib/leaking/BUILD.buri", LIBRARY);
    scratch.write("lib/leaking/lib.buri", "export fn version(): Int { 1 }\n");
    scratch.write("lib/leaking/test/counting.buri", LEAKING_TEST);

    let run = run_in_unchecked(&scratch.root, &["test", "//lib/leaking", "--force"], &[]);
    if !asked(&run) {
        no_native_run("buri test //lib/leaking", &run);
        return;
    }
    assert_ne!(run.code, 0, "a leaking suite exited zero:\n{}", run.all());
    assert!(
        run.all().contains(LEAKING_SAID),
        "this program is the repository's one known leak, and the runtime no longer says\n    \
         {LEAKING_SAID}\nabout it. If it leaks a different number of blocks, that is a \
         second defect and this test is where it was found. If it leaks nothing, the \
         defect is fixed — congratulations: delete this test and the matching row in \
         `harness::KNOWN_LEAKS`, which exists only to let \
         `repositories::concurrency_and_memory` past the same leak. What the run said:\n{}",
        run.all()
    );
    run.says("//lib/leaking");
    assert!(
        run.all().contains("toolchain bug"),
        "the report blamed the suite rather than the toolchain:\n{}",
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
