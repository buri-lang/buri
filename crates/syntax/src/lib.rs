//! Source text to a syntax tree and back: the lexer, the parser, the tree, the
//! formatter and the layout engine it shares with the other languages.
//!
//! The module tree mirrors where these files lived in `buri`, and the imports
//! below give the lower crates' modules their old names, so a path like
//! `crate::diagnostics::Span` reads the same in every crate.

pub mod formatting;
pub mod layout;
pub mod parsing;

use buri_diagnostics::{diagnostics, ice};
use buri_hash::hash;

mod compiler {
    pub use buri_stdlib::compiler::standard_library;
}
