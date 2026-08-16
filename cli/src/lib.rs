//! The Buri toolchain, as a library so that the integration tests can drive
//! the same code paths the `buri` binary does.
//!
//! The directories say who owns what:
//!
//!   * `parsing` — source text to a syntax tree. It is not inside `compiler`
//!     because the formatter, the linter, and the language server read the
//!     same tree, and none of them wants the rest of the compiler.
//!   * `compiler` — one directory per stage after parsing: `semantics`,
//!     `transform`, `backend`, with `driver` running the front of the pipeline.
//!   * `build` — what a repository declares and what the toolchain does with
//!     it: the graph, the build files, the action cache.
//!   * `commands` — one file per `buri` subcommand, plus the table that
//!     dispatches them and the argument parser.
//!   * `documentation` — the prose the binary ships and the machinery that
//!     serves, assembles, and compiles the examples in it.
//!   * `language_server` — the protocol, over the analysis `build` already runs.
//!
//! `diagnostics`, `formatting`, and `json` are at the top level because more
//! than one of the above depends on them and none of them owns them.

pub mod build;
pub mod commands;
pub mod compiler;
pub mod diagnostics;
pub mod documentation;
pub mod formatting;
pub mod json;
pub mod language_server;
pub mod parsing;
