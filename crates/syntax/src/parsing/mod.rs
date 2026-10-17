//! Source text to a syntax tree.
//!
//! One pass with no feedback: no production here consults a name, a type, or a
//! build file (guides/compile-speed.md). That is what lets the formatter, the
//! linter, the `BUILD.buri` generator, and the language server's keystroke
//! path use this module without any of the stages in `compiler`.

pub mod flat;
pub mod lexer;
pub mod parser;
pub mod tree;

/// Appends `d` to `errors` and lends it back, so a caller can add what varies.
///
/// `Vec::push_mut`, which is newer than this crate's `rust-version`.
#[expect(
    clippy::indexing_slicing,
    reason = "the index is the slot the push just filled"
)]
fn pushed(
    errors: &mut Vec<crate::diagnostics::Diagnostic>,
    d: crate::diagnostics::Diagnostic,
) -> &mut crate::diagnostics::Diagnostic {
    let at = errors.len();
    errors.push(d);
    &mut errors[at]
}
