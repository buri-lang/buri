//! **What the language does**, on the backend that defines it.
//!
//! One binary, one module per suite. Everything here runs on the JavaScript
//! backend, which is the reference: `native/` asks whether the other backends
//! agree, and `native/agreement.rs` is the bridge between the two. The corpus
//! says so itself — `conformance/lib/*/BUILD.buri` declares
//! `test { platforms: [JS] }` — because a suite that names no platform runs
//! natively now, and a reference that moved with the default would not be one.
//!
//! | Module | Corpus | Question |
//! |---|---|---|
//! | [`conformance`] | `conformance/`, `reject/`, `crash/` | Does a program mean what SPEC says, and is a program that must not compile refused with the diagnostic recorded beside it? |
//! | [`standard_library`] | `core/*` | Does the standard library typecheck against itself? |
//! | [`corpus`] | every `.buri` in the repository | Does everything meant to compile parse, does every build file read, **is every source already what `buri format` writes**, is formatting a fixed point, is the tree-sitter grammar generated? |
//! | [`golden_javascript`] | `golden_javascript/` | What does the backend *compile to*, construct by construct? |
//! | [`scoped_bodies`] | `repositories/lsp/*/repo`, `example/` | Does an analysis that checks one file's bodies answer what a whole-closure one answers, for that file? |
//! | [`sharing`] | `runtime.js`, two generated programs | Is a list this backend did not allocate never written to, and is growing one in a loop linear? |
//!
//! ```text
//! cargo test -p buri --test language                       # all six
//! cargo test -p buri --test language conformance::         # one of them
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

mod conformance;
mod corpus;
mod golden_javascript;
mod scoped_bodies;
mod sharing;
mod standard_library;
