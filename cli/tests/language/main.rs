//! **What the language does**, on the backend that defines it.
//!
//! One binary, one module per suite. Everything here runs on the JavaScript
//! backend, which is the reference: `native/` asks whether the other backends
//! agree, and `native/agreement.rs` is the bridge between the two.
//!
//! | Module | Corpus | Question |
//! |---|---|---|
//! | [`conformance`] | `conformance/`, `reject/`, `crash/` | Does a program mean what SPEC says, and is a program that must not compile refused with the diagnostic recorded beside it? |
//! | [`standard_library`] | `core/*` | Does the standard library typecheck against itself? |
//! | [`corpus`] | every `.buri` in the repository | Does everything meant to compile parse, does every build file read, is formatting a fixed point, is the tree-sitter grammar generated? |
//! | [`golden_javascript`] | `golden_javascript/` | What does the backend *compile to*, construct by construct? |
//!
//! ```text
//! cargo test -p buri --test language                       # all four
//! cargo test -p buri --test language conformance::         # one of them
//! ```

#[path = "../harness/mod.rs"]
mod harness;

mod conformance;
mod corpus;
mod golden_javascript;
mod standard_library;
