//! The JavaScript backend's corners: repositories whose programs reach the
//! parts of the generator and the minifier no other suite reaches through
//! `buri`.
//!
//! A test bundle skips the minifier, so each case runs its programs with
//! `buri run`, once in debug and once with `--release`, which also mangles.
//! See `harness/case.rs` for the format.
//!
//! ```text
//! BURI_BLESS=1 cargo test -p buri --test build javascript::   # record the goldens
//! ```
use crate::harness::*;

#[test]
fn the_javascript_backend() {
    run_corpus(&tests_dir().join("repositories/javascript"), "javascript", 1);
}
