//! The Buri toolchain, as a library so that the integration tests can drive
//! the same code paths the `buri` binary does.
//!
//! The directories say who owns what:
//!
//!   * `parsing` — source text to a syntax tree. It is not inside `compiler`
//!     because the formatter, the linter, and the language server read the
//!     same tree, and none of them wants the rest of the compiler.
//!   * `compiler` — one directory per stage after parsing: `semantics`,
//!     `middle`, `backend`, with `driver` running the front of the pipeline.
//!   * `build` — what a repository declares and what the toolchain does with
//!     it: the graph, the build files, the action cache.
//!   * `commands` — one file per `buri` subcommand, plus the table that
//!     dispatches them and the argument parser.
//!   * `documentation` — the prose the binary ships and the machinery that
//!     serves, assembles, and compiles the examples in it.
//!   * `language_server` — the protocol, over the analysis `build` already runs.
//!
//!   * `languages` — the files a build reads that are not Buri: JSON today,
//!     each checked against its schema and laid out through `layout`.
//!
//! `diagnostics`, `formatting`, `json`, `layout` and `parallel` are at the top level
//! because more than one of the above depends on them and none of them owns
//! them.
//!
//! `allocator` is the binary's global allocator. It lives here so the bench
//! installs the same one, and its source sits in `cli/runtime/` because every
//! compiled program installs it too.
//!
//! Most of these live in crates of their own under `crates/` and are
//! re-exported here at their old paths. `design/CRATES.md` has the graph.

// The coverage gate's nightly build, so a race can be left out of what it counts.
#![cfg_attr(buri_coverage, feature(coverage_attribute))]

#[path = "../runtime/allocator.rs"]
pub mod allocator;
pub mod build;
pub mod commands;
pub mod compiler;
pub mod documentation;
pub mod language_server;

// The lower crates, at the paths their modules had when they were part of this
// one. `design/CRATES.md` draws the graph.
pub use buri_diagnostics::{diagnostics, ice, json, parallel, profile};
pub use buri_hash::hash;
pub use buri_project::languages;
pub use buri_syntax::{formatting, layout, parsing};
