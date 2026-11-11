//! **A test that cannot run fails. It never passes quietly.**
//!
//! Half the native suite opens with a guard: `if !supported() { return; }`. It
//! used to return quietly on a developer's machine and fail only under
//! `BURI_CI=1`. That let a landing pass a suite CI then failed: node was missing
//! from the dev shell, so `conformance::float_suites_pass_under_node` returned
//! early on every local run and passed.
//!
//! So a guard that fires is a failure on every machine. [`missing`] panics,
//! naming the guard and what this machine lacks. Every tool the suite runs is
//! in the flake's dev shell, so inside `nix develop` no guard fires; outside it,
//! the failure says what to install.
//!
//! ## A failed build is a failure too
//!
//! A guard asks the machine, never the program under test. A test that builds
//! a native program and sees it fail calls [`native_program_failed`], which
//! names what this machine lacks when [`native_host_gap`] finds something, and
//! otherwise blames the program.
//!
//! ## Deferrals are not skips
//!
//! Three tests in `cli/tests/build/repositories.rs` assert time. They are
//! meaningless in a debug profile and they cost minutes, so they do not run in
//! the `test` matrix: they run in `language-server-budget`, which builds
//! `--release` and sets `BURI_PERF=1`. The assertion is made on this workflow,
//! in a job written for it.
//!
//! [`deferred_to`] is how such a test says so. It prints a line naming the job,
//! and `cli/tests/ci.rs` asserts that job exists and still asks for those
//! tests. A deferral whose job has been deleted is a skip, and that is the
//! failure the meta-test catches.

// Each test binary that includes this file uses a subset of it.
#![allow(dead_code)]

/// A guard fired: this machine lacks what the test needs. Always panics.
///
/// `domain` is the suite, `why` is what is missing. Both go into the panic, so
/// the person reading it knows which prerequisite to install.
///
/// It returns `bool` so that guards read `if !ready && ci::missing(..) {
/// return; }`; it never actually returns.
pub fn missing(domain: &str, why: &str) -> bool {
    panic!(
        "`{domain}` cannot run here: {why}.\n\
         \n\
         A test that cannot run fails rather than passing quietly. Every tool the suite needs \
         is in the flake's dev shell, so run it inside `nix develop` (cli/tests/README.md has \
         the command)."
    );
}

/// Why this machine cannot build a native artifact for itself, or `None`.
///
/// The build system's own answer — [`buri::build::actions::native_gap`] at the
/// debug profile, then whether a C driver is on `PATH` for the link — so a
/// guard asks the machine and never the program under test.
pub fn native_host_gap() -> Option<String> {
    use buri::build::{actions, link};
    use buri::compiler::backend::{Profile, Target};
    let Some(platform) = link::host_platform() else {
        return Some(String::from(
            "no native backend exists for this host's platform",
        ));
    };
    let target = Target {
        platform,
        arch: link::host_arch(),
    };
    if let Some(gap) = actions::native_gap(target, Profile::Debug) {
        return Some(gap.reason);
    }
    link::select(target).err().map(|refusal| refusal.message)
}

/// A native program under test did not build or did not run.
///
/// Panics either way. When [`native_host_gap`] names a missing prerequisite,
/// through [`missing`]; on a machine that has everything, the program itself
/// failed, and the panic says so with what the command printed.
pub fn native_program_failed(domain: &str, what: &str, output: &str) {
    match native_host_gap() {
        Some(why) => {
            missing(domain, &why);
        }
        None => panic!(
            "{domain}: {what}. This machine has everything a native build needs, so the program \
             under test is what broke:\n{output}"
        ),
    }
}

/// This test is not run here, and the job that does run it is named.
///
/// Never panics: the assertion is made elsewhere in this same workflow. The printed line is the evidence, and
/// `cli/tests/ci.rs::every_deferral_names_a_job_that_still_asks_for_it` is what
/// holds the name to a job that exists.
pub fn deferred_to(domain: &str, job: &str, why: &str) -> bool {
    eprintln!("{domain}: deferred to the `{job}` job ({why})");
    true
}
