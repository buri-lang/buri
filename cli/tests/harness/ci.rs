//! **A skip is a host's answer, and on CI there is no such host.**
//!
//! Half the native suite opens with a guard — `if !supported() { return; }` —
//! and that guard is load-bearing: a contributor on a machine with no C
//! compiler, or on a triple no stencil library is built for, gets a suite that
//! runs and says so rather than a wall of red about a backend they were never
//! going to use. `cli/tests/README.md` states that as the rule, and it is the
//! right rule *for a host*.
//!
//! It is the wrong rule for a runner. Every job in `.github/workflows/ci.yml`
//! that runs this suite installs clang, lld, mold, llvm and a JavaScript
//! engine, and `cli/tests/ci.rs` asserts — from inside the same run — that each
//! of them is on `PATH` and that the stencil libraries and the runtime archive
//! are non-empty bytes. A guard that fires there is not "this host cannot"; it
//! is "something the workflow guarantees stopped being true", and the only
//! honest report of that is a failure.
//!
//! So the same guard reads `BURI_CI`:
//!
//! * **Unset** — the developer's machine. [`skipped`] prints the reason and the
//!   test returns, exactly as before.
//! * **`BURI_CI=1`** — a runner. [`skipped`] **panics**, naming the guard and
//!   the reason, so a vacuous pass becomes a red X on the step that would
//!   otherwise have reported it green.
//!
//! `BURI_CI` is set once, in the workflow-level `env:` block, which is the
//! definition of "every job": GitHub hands a workflow's environment to every
//! job and every step in it, and a job-level `env:` overrides only the keys it
//! names. `cli/tests/ci.rs` asserts that line is still there, because a
//! mechanism that can be deleted without a test noticing is a mechanism that
//! will be.
//!
//! ## The second kind of skip, which is not a host's answer either
//!
//! Two tests in `cli/tests/build/repositories.rs` assert milliseconds. They are
//! meaningless in a debug profile and they cost minutes, so they do not run in
//! the `test` matrix — they run in `language-server-budget`, which builds
//! `--release` and sets `BURI_PERF=1`. That is a *deferral*, not a skip: the
//! assertion is made on this workflow, in a job written for it.
//!
//! [`deferred_to`] is how such a test says so. It never panics, because the job
//! it names is where the panic would come from; it prints a line naming the
//! job, and `cli/tests/ci.rs` asserts that job exists and still asks for those
//! tests. A deferral whose job has been deleted is a skip, and that is the
//! failure the meta-test catches.

// Each test binary that includes this file uses a subset of it.
#![allow(dead_code)]

/// Whether this process is a CI runner, by the workflow's own declaration.
///
/// Exactly `1`, not "set": `BURI_CI=0` is a contributor turning the strictness
/// off to reproduce a runner's *other* behaviour, and an "is it set" test would
/// read that as a yes.
pub fn on() -> bool {
    std::env::var_os("BURI_CI").is_some_and(|value| value == "1")
}

/// A guard fired. Print it and return `true` on a host; panic on a runner.
///
/// The return value is `true` — "yes, skip" — so a caller reads as
/// `if ci::skipped(..) { return; }` and the panic arm needs no `unreachable!`
/// beside it.
///
/// `domain` is the suite, `why` is the sentence the guard already had. Both go
/// into the panic, because the person reading a red step needs to know which
/// runner assumption broke and not merely that one did.
///
/// Deliberately not `#[must_use]`: half the call sites are a guard that already
/// knows it is returning (`else { ci::skipped(..); return; }`) and threading a
/// bool through those to satisfy a lint would be noise around the one line
/// that matters.
pub fn skipped(domain: &str, why: &str) -> bool {
    if on() {
        panic!(
            "BURI_CI=1 and `{domain}` skipped: {why}.\n\
             \n\
             Every runner in .github/workflows/ci.yml installs this suite's tools, and \
             `cli/tests/ci.rs` holds the stencil libraries and the runtime archive to being real \
             bytes, so this guard cannot fire on a runner that is still set up the way the \
             workflow says it is. Something the workflow guarantees stopped being true — that is \
             the bug, and a test that returned quietly here would have reported it as a pass."
        );
    }
    eprintln!("{domain}: skipped ({why})");
    true
}

/// This test is not run here, and the job that does run it is named.
///
/// Never panics, on a runner or off one: the assertion is made elsewhere in
/// this same workflow. The printed line is the evidence, and
/// `cli/tests/ci.rs::every_deferral_names_a_job_that_still_asks_for_it` is what
/// holds the name to a job that exists.
pub fn deferred_to(domain: &str, job: &str, why: &str) -> bool {
    eprintln!("{domain}: deferred to the `{job}` job ({why})");
    true
}
