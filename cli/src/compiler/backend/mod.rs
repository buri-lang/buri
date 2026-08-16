//! The JavaScript backend.
//!
//! `generate` turns the transformed typed tree into the JavaScript AST that
//! `javascript` prints and minifies, and `intrinsics` supplies the bodies of
//! the operations the standard library declares without one. `runtime.js` is
//! the hand-written half, next to the code that emits calls into it.

pub mod generate;
pub mod intrinsics;
pub mod javascript;

/// The hand-written half of the JavaScript backend. Every global in it is
/// `$`-prefixed so the minifier can rename it and drop what a program does not
/// reach.
pub fn runtime_source() -> &'static str {
    include_str!("runtime.js")
}
