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
//! | [`example`] | `example/` | The worked monorepo — the largest body of Buri here — builds, tests, lints and formats clean. |
//! | [`incrementality`] | scratch | What the cache may and may not do, read off the `--explain` transcript. |
//! | [`hermeticity`] | scratch | That a spawn is deterministic, that a perturbed environment changes neither bytes nor verdicts, and that concurrent builds leave the cache intact. |
//! | [`heap`] | scratch | That the heap check every suite here runs under is really on — in a `buri run` artifact and in the binary `buri test` spawns — and that a program which really leaks is really reported. |
//! | [`watch`] | scratch | What `buri watch` declares as its input set, and what it re-runs when one of them moves. |
//!
//! ```text
//! cargo test -p buri --test build                          # all seven
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

mod example;
mod heap;
mod hermeticity;
mod incrementality;
mod init;
mod repositories;
mod watch;
