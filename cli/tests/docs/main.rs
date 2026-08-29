//! **The documentation**, held to the same bar as the code.
//!
//! | Module | Question |
//! |---|---|
//! | [`documents`] | What the documents *are*: every fence scannable and tagged, every link resolving, and the checked-in `SPEC.md` still what `buri docs assemble` produces. |
//! | [`examples`] | What the documents *show*: every fenced example compiles, and every one that cannot says why. |
//!
//! ```text
//! cargo test -p buri --test docs
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

mod documents;
mod examples;
