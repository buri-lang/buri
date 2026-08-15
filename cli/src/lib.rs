//! The Buri toolchain, as a library so that the integration tests can drive
//! the same code paths the `buri` binary does.

pub mod ast;
pub mod build;
pub mod buildfile;
pub mod builtins;
pub mod check;
pub mod cli;
pub mod codegen;
pub mod codegen_intrinsic;
pub mod commands;
pub mod compile;
pub mod diag;
pub mod doc;
pub mod doc_api;
pub mod doc_assemble;
pub mod doc_errors;
pub mod doc_harness;
pub mod doc_md;
pub mod doc_topics;
pub mod doctest;
pub mod driver;
pub mod exhaust;
pub mod format;
pub mod gen;
pub mod hir;
pub mod infer;
pub mod infer_expr;
pub mod infer_pat;
pub mod js;
pub mod lex;
pub mod mono;
pub mod opt;
pub mod parse;
pub mod run;
pub mod stdlib;
pub mod tco;
pub mod testrun;
pub mod textproto;
pub mod tools;
pub mod types;
pub mod workspace;

/// The hand-written half of the JavaScript backend. Every global in it is
/// `$`-prefixed so the minifier can rename it and drop what a program does not
/// reach.
pub fn runtime_source() -> &'static str {
    include_str!("runtime.js")
}
pub mod cache;
