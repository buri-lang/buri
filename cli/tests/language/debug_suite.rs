//! The conformance suite as checked in, run once per test run in the default
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
//! Once per run, not once per process: nextest starts a process per test, so
//! the first process to ask leaves the answer on disk for the other two
//! (`sweep::once_per_run`). That was 80 CPU-seconds a suite.
//!
//! The run is memoized even when it fails. A failure is recorded and handed to
//! every test that asks, rather than left out for the next test to try again,
//! so a flaky run cannot pass on its second attempt.
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
        read_back(&sweep::once_per_run("conformance-run", || {
            written(&run_unmodified())
        }))
    });
    match answer {
        Ok(run) => run,
        Err(said) => panic!("the unmodified conformance suite could not be run:\n{said}"),
    }
}

fn run_unmodified() -> Result<Run, String> {
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
}

/// An answer as one process leaves it for the next: a header of the field
/// lengths, then the fields.
fn written(answer: &Result<Run, String>) -> String {
    match answer {
        Ok(run) => format!(
            "{} {} {} {}\n{}{}{}",
            run.code,
            run.what.len(),
            run.stdout.len(),
            run.stderr.len(),
            run.what,
            run.stdout,
            run.stderr
        ),
        Err(said) => format!("panicked\n{said}"),
    }
}

/// [`written`] read back.
fn read_back(text: &str) -> Result<Run, String> {
    let (head, rest) = text.split_once('\n').expect("an answer has a header");
    if head == "panicked" {
        return Err(rest.to_string());
    }
    let Some((code, lengths)) = head.split_once(' ') else {
        panic!("{head:?} is not an answer's header");
    };
    let lengths: Vec<usize> = lengths.split(' ').map(|n| n.parse().unwrap()).collect();
    let [what, stdout, stderr] = lengths[..] else {
        panic!("{head:?} is not an answer's header");
    };
    let (what, rest) = rest.split_at(what);
    let (stdout, rest) = rest.split_at(stdout);
    assert_eq!(rest.len(), stderr, "an answer's stderr is cut short");
    Ok(Run {
        code: code.parse().unwrap(),
        stdout: stdout.to_string(),
        stderr: rest.to_string(),
        what: what.to_string(),
    })
}

/// What another process wrote reads back as the same answer, with the bytes
/// a run's output can hold.
#[test]
fn an_answer_reads_back_as_it_was_written() {
    let run = Run {
        code: 3,
        stdout: String::from("é 🙂 1 passed\n"),
        stderr: String::from("error: one\n  |\n"),
        what: String::from("buri test //... --force"),
    };
    let Ok(back) = read_back(&written(&Ok(run))) else {
        panic!("a run read back as a panic");
    };
    assert_eq!(
        (
            back.code,
            back.stdout.as_str(),
            back.stderr.as_str(),
            back.what.as_str()
        ),
        (
            3,
            "é 🙂 1 passed\n",
            "error: one\n  |\n",
            "buri test //... --force"
        )
    );
    let panicked = Err(String::from("leaked a block\nin 🙂"));
    assert_eq!(read_back(&written(&panicked)).err(), panicked.err());
}
