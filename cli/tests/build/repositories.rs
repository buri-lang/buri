//! Whole repositories, each provoking one build-system rule.
//!
//! The `reject/` corpus builds every case as a single-package binary with no
//! dependencies, so nothing in it can express a diagnostic *about the graph* —
//! which is most of what the build system checks. Each case here is instead a
//! small repository checked in whole, with a manifest naming what the CLI does
//! in it and what that must print. See `harness/case.rs` for the format.
//!
//! One test per specification document, so cargo runs them on separate threads
//! and a failure names the document to open.
//!
//! ```text
//! BURI_BLESS=1 cargo test -p buri --test build repositories::    # record the goldens
//! BURI_KEEP=1  cargo test -p buri --test build repositories::    # keep the scratch trees
//! ```
use crate::harness::*;

/// BUILD-FILES.md: what a rule declares, and the diagnostics that fire when
/// the declaration and the code disagree.
#[test]
fn build_file_rules() {
    run_corpus(&tests_dir().join("repositories/build-files"), "build-files", 12);
}

/// LIBRARIES.md: `lib.buri` is a library's entire public surface, and the
/// boundary it draws applies to methods as much as to names.
#[test]
fn library_boundaries() {
    run_corpus(&tests_dir().join("repositories/libraries"), "libraries", 8);
}

/// TAGS.md: a tag is a property of a whole dependency closure, and the two
/// things that follow from one — what may not sit beside it, and where it may
/// be built.
#[test]
fn tag_policy() {
    run_corpus(&tests_dir().join("repositories/tags"), "tags", 7);
}

/// CLI.md: the exit codes, and the commands whose contract is about what they
/// leave on disk rather than what they compute — `gen`, `run`, `clean`,
/// `version`, the `out/` symlink, and the no-argument forms that mean the whole
/// repository from wherever they are run.
#[test]
fn cli_contract() {
    run_corpus(&tests_dir().join("repositories/cli"), "cli", 11);
}

/// CLI.md's `query`: what the graph says, asked without building anything.
/// Its own corpus because the answers are the recorded output rather than a
/// diagnostic — a query that has stopped working prints a plausible wrong
/// answer rather than failing.
#[test]
fn graph_queries() {
    run_corpus(&tests_dir().join("repositories/query"), "query", 1);
}

/// PROTO.md: a `.proto` schema is a source that becomes a module. One case for
/// the build-file half — declared, placed by `gen`, keyed on its contents,
/// internal to the rule that declared it — one for the edition the reader
/// requires and the two syntaxes it refuses, and one for each half of what it
/// otherwise refuses: the constructs that are out of scope, and the files that
/// are not schemas at all.
#[test]
fn proto_schemas() {
    run_corpus(&tests_dir().join("repositories/proto"), "proto", 5);
}

/// CLI.md's lint catalogue: the hygiene rules, which ask about a package's own
/// code rather than about the graph. Each case ends with the edit that makes
/// the finding go away, because a rule nothing can turn off is a rule nobody
/// can check the fix for.
///
/// The `repo_lint_*` cases are the other half: what `REPO.buri`'s `lint` block
/// does to the same finding — when the catalogue runs, how hard a finding
/// lands, and what a misspelled field in the block costs.
#[test]
fn lint_catalogue() {
    run_corpus(&tests_dir().join("repositories/linting"), "linting", 19);
}

/// TESTING.md: where tests live, what a test source may reach, and what the
/// runner does with a suite — the flags, the timeout, the golden-file update
/// mode, and the exact shape of a failure report.
#[test]
fn test_suites() {
    run_corpus(&tests_dir().join("repositories/testing"), "testing", 10);
}

/// The language server. Each case is a recorded session: requests in, decoded
/// responses out, so a change to what the server says shows up as a diff
/// rather than as an editor behaving differently.
#[test]
fn language_server() {
    run_corpus(&tests_dir().join("repositories/lsp"), "lsp", 44);
}
