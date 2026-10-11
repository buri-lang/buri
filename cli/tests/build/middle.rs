//! The middle end's corners: repositories whose programs reach the parts of
//! monomorphization, the tree passes and lowering no other suite reaches
//! through `buri`.
//!
//! Each case builds natively in debug mode and on JavaScript, so the two agree
//! on every block. See `harness/case.rs` for the format.
//!
//! ```text
//! BURI_BLESS=1 cargo test -p buri --test build middle::   # record the goldens
//! ```
use crate::harness::*;

#[test]
fn the_middle_end() {
    run_corpus(&tests_dir().join("repositories/middle"), "middle", 9);
}
