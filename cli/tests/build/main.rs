//! **The build system**, driven as a user drives it.
//!
//! Every module here runs the real `buri` binary in a repository nobody else
//! can see — a scratch tree under `CARGO_TARGET_TMPDIR`, named with the process
//! id, built by `harness::Scratch`. Nothing writes inside a checked-in tree.
//!
//! | Module | Corpus | Question |
//! |---|---|---|
//! | [`repositories`] | `repositories/` | One repository per build-system rule, each with a manifest of what the CLI does in it and the output that produces. |
//! | [`example`] | `example/` | The worked monorepo — the largest body of Buri here — builds, tests, lints and formats clean. |
//! | [`incrementality`] | scratch | What the cache may and may not do, read off the `--explain` transcript. |
//! | [`hermeticity`] | scratch | That a spawn is deterministic, that a perturbed environment changes neither bytes nor verdicts, and that concurrent builds leave the cache intact. |
//! | [`watch`] | scratch | What `buri watch` declares as its input set, and what it re-runs when one of them moves. |
//!
//! ```text
//! cargo test -p buri --test build                          # all five
//! BURI_BLESS=1 cargo test -p buri --test build repositories::  # record the goldens
//! BURI_KEEP=1  cargo test -p buri --test build             # keep the scratch trees
//! ```

#[path = "../harness/mod.rs"]
mod harness;

mod example;
mod hermeticity;
mod incrementality;
mod repositories;
mod watch;
