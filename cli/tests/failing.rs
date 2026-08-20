//! **What `buri test` prints when a test fails**, recorded case by case.
//!
//! Every other suite here asks whether the toolchain gets the right answer.
//! This one asks what it says when a program does not, which is the output a
//! person reads most often and the one nothing else pins: the conformance
//! canary asserts that a failing suite prints the word `FAIL`, and
//! `repositories/testing/failure_format` records one report of one string
//! comparison. Neither says what a failed `assert.some` looks like, what a
//! division by zero inside a test looks like, or whether a title with a colon
//! in it survives codegen.
//!
//! ```text
//! cli/tests/failing/aborts/
//!   CASE.textproto      the manifest — what to run, and what it must exit
//!   repo/               a Buri repository, copied into a scratch tree and run in
//!   expected/fail.txt   what `buri test` printed, recorded
//! ```
//!
//! The schema is the repository case, `harness/case.rs`, unchanged — a failing
//! test needs a package, a rule and a suite, which is a repository, and there
//! is no second manifest dialect to learn. `expected/*.txt` holds the bytes the
//! CLI wrote with the elapsed time normalised, and nothing else is
//! substituted: a source location in a report is repository-relative, so
//! `--> lib/money/test/cents.buri:4:1` in a recorded file is also a path you
//! can open.
//!
//! ```text
//! cargo test -p buri --test failing                 # compare
//! BURI_BLESS=1 cargo test -p buri --test failing    # record
//! BURI_KEEP=1  cargo test -p buri --test failing    # keep the scratch trees
//! ```
//!
//! `exit` is hand-written in every manifest and is never blessed, so blessing
//! can rewrite what a report *says* and can never turn a failing suite into a
//! passing one.
//!
//! # What this suite pins, and what it does not
//!
//! **The JavaScript backend.** A suite that declares no `test.platforms` is
//! executed as JavaScript, which is what `buri test` has always done and what
//! every case here runs on. The native path through `commands/test.rs::run_native`
//! produces a deliberately worse report — there is no native test runner, so
//! the binary stops at the first failure and, in a suite of more than one test,
//! says it cannot attribute it — and it is reachable only where a backend, a
//! runtime archive and a linker for the host are all present. Pinning it here
//! would make this suite fail under `--no-default-features`, so it is not here:
//! `native/conformance.rs` drives the native side, and
//! `repositories/testing/suite_platforms` records the refusal a platform this
//! toolchain cannot produce a binary for gets.
//!
//! **The report, not the catalogue of diagnostics.** A test source that does
//! not compile is a `buri test` failure, and `does_not_compile` records the
//! *shape* of that — stderr, a summary of zeros on stdout, exit 1 — rather than
//! the diagnostic, because `cli/tests/reject` already holds a hundred of
//! those with their rendered text and their JSON, and a second copy here would
//! be a second place to update when a message is reworded.
//!
//! # The shape of a report, and where each rule is pinned
//!
//! Five properties hold across every case here, and each has the case that
//! would catch it breaking:
//!
//! * **One line per `FAIL`.** A title is printed as the quoted string it was
//!   written as, so a `"` and a newline in one are escaped — `titles`.
//! * **A message is part of the report.** Every line of it is indented under
//!   the `FAIL` line, over as many lines as the message has — `long_values`.
//! * **A value is rendered in the syntax it is written in.** `.None` and
//!   `.Some(3)` for an `Option`, `Hollow {}` for a struct with no fields,
//!   `.Red` for a payloadless variant — `assertion_kinds`, `composite_values`.
//! * **A test is reported where it was declared.** `locate` matches on
//!   (title, module), so two files of one suite may share a title and each
//!   reports at its own line — `many_modules`. Two tests sharing a title
//!   inside one file do not compile — `duplicate_titles`.
//! * **A report is structural.** A hand-written `impl Show` does not reach one,
//!   and `assert.eq` carries no `Show` bound — `hand_written_show`.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::string_slice,
    clippy::arithmetic_side_effects,
    clippy::print_stdout,
    clippy::print_stderr,
    reason = "test code. The lint set in `Cargo.toml` pins a promise about the \
              toolchain — that no input panics it — and a harness that drives \
              the toolchain is not the toolchain. A test that unwraps fails on \
              the line that broke, which is what a test is for, and threading \
              `?` through an assertion buys nothing. `clippy.toml` exempts \
              `#[test]` functions already; this covers the helpers around them."
)]

#[path = "harness/mod.rs"]
mod harness;

use harness::*;
use std::path::PathBuf;

/// The corpus root. Beside the suite that reads it, the way `formatting/` sits
/// beside `formatting.rs`.
fn corpus() -> PathBuf {
    tests_dir().join("failing")
}

/// Every case, compared against its recorded report.
///
/// One test rather than one per case, because `run_corpus` collects mismatches
/// and reports all of them: a reworded failure moves every case that shows it,
/// and reading eighteen diffs at once is the point of recording them.
#[test]
fn recorded_failure_reports() {
    run_corpus(&corpus(), "failing", 18);
}

/// A case is a manifest, a repository, and the goldens the manifest names —
/// no more and no less.
///
/// A stray file in a case directory is either a leftover or somebody reaching
/// for an option the schema does not have, and a golden nothing names is a
/// record of a run that no longer happens. `formatting.rs` makes the same
/// check about its own two-file cases and for the same reason.
#[test]
fn every_case_is_a_manifest_a_repository_and_the_goldens_it_names() {
    let mut wrong = Vec::new();
    for dir in case_dirs(&corpus(), "CASE.textproto", 18) {
        let name = dir.file_name().unwrap().to_string_lossy().to_string();

        let mut entries: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(Result::ok)
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();
        entries.sort();
        if entries != ["CASE.textproto", "expected", "repo"] {
            wrong.push(format!("{name}: holds {entries:?}, not the three a case is"));
        }

        // Every `golden:` the manifest names, against every file recorded.
        let case = load_case(&dir);
        let mut named: Vec<String> = case
            .steps
            .iter()
            .filter_map(|s| match s {
                Step::Run { golden: Some(g), .. } => Some(g.clone()),
                _ => None,
            })
            .collect();
        named.sort();
        named.dedup();
        let mut recorded: Vec<String> = std::fs::read_dir(dir.join("expected"))
            .unwrap()
            .filter_map(Result::ok)
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();
        recorded.sort();
        if named != recorded {
            wrong.push(format!(
                "{name}: the manifest names {named:?} and expected/ holds {recorded:?}"
            ));
        }
    }
    assert!(wrong.is_empty(), "{} case(s) malformed:\n  {}", wrong.len(), wrong.join("\n  "));
}

/// No recorded report holds an elapsed time, a scratch path, or a byte count.
///
/// The harness substitutes all three before a golden is compared or written,
/// so a recorded file that still holds one was written by something else — and
/// would be a suite that passes on the machine it was blessed on and nowhere
/// else. Checked over the recorded bytes rather than asserted about the
/// normaliser, because it is the files that have to be right.
#[test]
fn no_recorded_report_pins_a_time_or_a_path() {
    let mut wrong = Vec::new();
    for dir in case_dirs(&corpus(), "CASE.textproto", 18) {
        let name = dir.file_name().unwrap().to_string_lossy().to_string();
        let Ok(entries) = std::fs::read_dir(dir.join("expected")) else { continue };
        for e in entries.filter_map(Result::ok) {
            let file = e.file_name().to_string_lossy().to_string();
            let Ok(text) = std::fs::read_to_string(e.path()) else { continue };
            for (n, line) in text.lines().enumerate() {
                let at = format!("{name}/{file}:{}", n + 1);
                if line.contains('/') && (line.contains("/tmp") || line.contains("/target/")) {
                    wrong.push(format!("{at}: an absolute path — {line:?}"));
                }
                // `(0.0s)` and `(0.0s, N cached)` are what `normalise` writes.
                // Anything else between `(` and `s)` is a real measurement.
                if let Some(i) = line.find('(') {
                    let rest = &line[i + 1..];
                    let end = rest.find("s)").or_else(|| rest.find("s,"));
                    if let Some(j) = end {
                        let inner = &rest[..j];
                        let numeric =
                            !inner.is_empty() && inner.chars().all(|c| c.is_ascii_digit() || c == '.');
                        if numeric && inner != "0.0" {
                            wrong.push(format!("{at}: an elapsed time — {line:?}"));
                        }
                    }
                }
            }
        }
    }
    assert!(
        wrong.is_empty(),
        "{} recorded line(s) pin the machine rather than the product:\n  {}",
        wrong.len(),
        wrong.join("\n  ")
    );
}

/// Every case says out loud what it is about, and runs `test`.
///
/// A corpus entry that never invokes the command the corpus is named for is
/// an entry in the wrong corpus, and one whose `doc` was copied from its
/// neighbour is one nobody can select from a listing.
#[test]
fn every_case_documents_itself_and_runs_the_test_command() {
    let mut docs: Vec<(String, String)> = Vec::new();
    let mut wrong = Vec::new();
    for dir in case_dirs(&corpus(), "CASE.textproto", 18) {
        let case = load_case(&dir);
        let runs_test = case.steps.iter().any(|s| match s {
            Step::Run { args, .. } => args.first().is_some_and(|a| a == "test"),
            _ => false,
        });
        if !runs_test {
            wrong.push(format!("{}: never runs `buri test`", case.name));
        }
        if case.doc.len() < 30 {
            wrong.push(format!("{}: `doc` says almost nothing — {:?}", case.name, case.doc));
        }
        for (other, doc) in &docs {
            if *doc == case.doc {
                wrong.push(format!("{}: has {other}'s `doc` word for word", case.name));
            }
        }
        docs.push((case.name.clone(), case.doc.clone()));
    }
    assert!(wrong.is_empty(), "{} case(s):\n  {}", wrong.len(), wrong.join("\n  "));
}
