//! Source text to a syntax tree.
//!
//! One pass with no feedback: no production here consults a name, a type, or a
//! build file (SPEC 13.1). That is what lets the formatter, the linter, the
//! `BUILD.buri` generator, and the language server's keystroke path use this
//! module without any of the stages in `compiler`.

pub mod lexer;
pub mod parser;
pub mod tree;
