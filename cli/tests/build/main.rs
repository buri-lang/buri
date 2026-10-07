//! **The build system**, driven as a user drives it.
//!
//! Every module here runs the real `buri` binary in a repository nobody else
//! can see — a scratch tree under `CARGO_TARGET_TMPDIR`, named with the process
//! id, built by `harness::Scratch`. Nothing writes inside a checked-in tree.
//!
//! | Module | Corpus | Question |
//! |---|---|---|
//! | [`init`] | scratch | That `buri init` writes a repository that builds and tests, and that a second run refuses. |
//! | [`repositories`] | `repositories/` | One repository per build-system rule, each with a manifest of what the CLI does in it and the output that produces. |
//! | [`custom_platforms`] | `repositories/custom-platforms/` | That a repository platform's artifact answers when the host it was written for calls it, and that editing its `js` file rebuilds it. |
//! | [`closed_stdout`] | `example/` | That a reader leaving early, as `head -1` does, ends every command by `SIGPIPE`, quietly, and that a pipe left open prints what a file would. |
//! | [`example`] | `example/` | The worked monorepo — the largest body of Buri here — builds, tests, lints and formats clean. |
//! | [`incrementality`] | scratch | What the cache may and may not do, read off the `--explain` transcript. |
//! | [`hermeticity`] | scratch | That a spawn is deterministic, that a perturbed environment changes neither bytes nor verdicts, and that concurrent builds leave the cache intact. |
//! | [`generators`] | scratch | That a generated module reaches the host's native backend and its linker, and that the generator the toolchain ships is compiled once per repository. |
//! | [`heap`] | scratch | That the heap check every suite here runs under is really on — in a `buri run` artifact and in the binary `buri test` spawns — and that a program which really leaks is really reported. |
//! | [`monorepo`] | scratch | A large repository's shape, scaled down: what a warm run, a comment edit and a generator's input edit may not redo, and that a link does not start the C driver. |
//! | [`profile`] | scratch | That `BURI_PROFILE=1` reports each phase a run went through, that a run without it prints nothing extra, and that checking and emitting large shapes is linear in their size. |
//! | [`scheduling`] | scratch | That suites build and run side by side, report in suite order, and are reused across an edit that cannot change them. |
//! | [`verbose`] | `repositories/testing/verbose/` | What `buri test --verbose`'s goldens blank out: the unit each time is spelled in, a verdict cached before there were times, and the list each pass of a watch loop prints. |
//! | [`watch`] | scratch | What `buri watch` declares as its input set, and what it re-runs when one of them moves. |
//! | [`serving`] | `repositories/serving/` | That `buri run` on a page builds the artifact and serves it — the shell for every route, the files beside it as themselves, and `--watch` rebuilding into the next request. |
//! | [`web_storage`] | `repositories/platform/storage_on_a_page/` | That a page's `Storage` reaches IndexedDB through `web`'s `main.mjs`, survives a reload, and turns a full quota or a refusal into an error. |
//!
//! ```text
//! cargo test -p buri --test build                          # all nine
//! BURI_BLESS=1 cargo test -p buri --test build repositories::  # record the goldens
//! BURI_KEEP=1  cargo test -p buri --test build             # keep the scratch trees
//! ```

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

#[path = "../harness/mod.rs"]
mod harness;

mod closed_stdout;
mod custom_platforms;
mod example;
mod generators;
mod heap;
mod hermeticity;
mod incrementality;
mod init;
mod instances;
mod monorepo;
mod profile;
mod repositories;
mod scheduling;
mod serving;
mod verbose;
mod watch;
mod web_storage;
