//! The debug backend's corners: repositories whose programs reach the parts of
//! the copy-and-patch backend no other suite reaches through `buri`.
//!
//! Each case builds natively in debug mode, and most run on JavaScript too, so
//! the two backends agree on every block. See `harness/case.rs` for the format.
//!
//! ```text
//! BURI_BLESS=1 cargo test -p buri --test build stencil::   # record the goldens
//! ```
use crate::harness::*;

#[test]
fn the_debug_backend() {
    run_corpus(&tests_dir().join("repositories/stencil"), "stencil", 8);
}
