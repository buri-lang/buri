//! The conformance suite as checked in, run once per process in the default
//! (debug) mode.
//!
//! Three tests ask about that one run: [`crate::conformance`]'s
//! `conformance_suite_passes`, the debug half of its `release_and_debug_agree`,
//! and [`crate::corpus`]'s `formatting_the_corpus_preserves_what_it_means`,
//! whose "before" copy is the unmodified corpus. Each used to make its own
//! copy and run `buri test //... --force` in it, which is the same inputs three
//! times: the same tree, the same flags (`--debug` is the default mode, so
//! naming it changes nothing a build reads), and the same environment. So the
//! run happens here, once, and each test asserts what it always asserted on
//! the answer.
//!
//! The run is memoized even when it fails. A failure is recorded and handed to
//! every test that asks, rather than leaving the `OnceLock` empty for the next test
//! to try again, so a flaky run cannot pass on its second attempt.
use crate::harness::*;

use std::sync::OnceLock;

/// `buri test //... --force` over a fresh copy of `cli/tests/conformance`,
/// with no mode flag and nothing added to the environment.
///
/// The answer comes back whatever the exit status, because the tests that
/// read it differ in what they assert about that. A run that panicked the
/// harness itself — a leak, or a run past the hang cap — panics every caller
/// with the same message.
pub fn unmodified_conformance_run() -> &'static Run {
    static RUN: OnceLock<Result<Run, String>> = OnceLock::new();
    let answer = RUN.get_or_init(|| {
        std::panic::catch_unwind(|| {
            let suite = Scratch::copy_of("conformance", &tests_dir().join("conformance"));
            let run = suite.run(&["test", "//...", "--force"]);
            if run.code != 0 {
                // The directory is the evidence, and no test is panicking yet
                // for `Scratch`'s own drop to notice.
                eprintln!("scratch `conformance` kept at {}", suite.root.display());
                std::mem::forget(suite);
            }
            run
        })
        .map_err(|panic| {
            panic
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| panic.downcast_ref::<&str>().map(|s| (*s).to_string()))
                .unwrap_or_else(|| String::from("a panic with no message"))
        })
    });
    match answer {
        Ok(run) => run,
        Err(said) => panic!("the unmodified conformance suite could not be run:\n{said}"),
    }
}
