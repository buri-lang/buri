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
//! BURI_BLESS=1 cargo test -p buri --test repos    # record the goldens
//! BURI_KEEP=1  cargo test -p buri --test repos    # keep the scratch trees
//! ```

mod harness;
use harness::*;

/// BUILD-FILES.md: what a rule declares, and the diagnostics that fire when
/// the declaration and the code disagree.
#[test]
fn build_file_rules() {
    run_corpus(&tests_dir().join("repos/build-files"), "build-files", 8);
}

/// LIBRARIES.md: `lib.buri` is a library's entire public surface, and the
/// boundary it draws applies to methods as much as to names.
#[test]
fn library_boundaries() {
    run_corpus(&tests_dir().join("repos/libraries"), "libraries", 5);
}

/// TAGS.md: a tag is a property of a whole dependency closure, and the two
/// things that follow from one — what may not sit beside it, and where it may
/// be built.
#[test]
fn tag_policy() {
    run_corpus(&tests_dir().join("repos/tags"), "tags", 7);
}

/// CLI.md: the exit codes, and the commands whose contract is about what they
/// leave on disk rather than what they compute.
#[test]
fn cli_contract() {
    run_corpus(&tests_dir().join("repos/cli"), "cli", 2);
}

/// CLI.md's lint catalogue: the hygiene rules, which ask about a package's own
/// code rather than about the graph. Each case ends with the edit that makes
/// the finding go away, because a rule nothing can turn off is a rule nobody
/// can check the fix for.
#[test]
fn lint_catalogue() {
    run_corpus(&tests_dir().join("repos/linting"), "linting", 7);
}

/// The language server. Each case is a recorded session: requests in, decoded
/// responses out, so a change to what the server says shows up as a diff
/// rather than as an editor behaving differently.
#[test]
fn language_server() {
    run_corpus(&tests_dir().join("repos/lsp"), "lsp", 4);
}
