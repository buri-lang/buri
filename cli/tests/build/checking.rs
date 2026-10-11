//! What the checker says about a whole repository: the refusals a single-file
//! `reject/` case can't express, because they're about one module reaching
//! into another.
//!
//! ```text
//! BURI_BLESS=1 cargo test -p buri --test build checking::    # record the goldens
//! ```
use crate::harness::*;

#[test]
fn checking_across_modules() {
    run_corpus(&tests_dir().join("repositories/checking"), "checking", 4);
}
