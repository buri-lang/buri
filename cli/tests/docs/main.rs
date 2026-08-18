//! **The documentation**, held to the same bar as the code.
//!
//! | Module | Question |
//! |---|---|
//! | [`documents`] | What the documents *are*: every fence scannable and tagged, every link resolving, and the checked-in `SPEC.md` and `README.md` still what `buri docs assemble` produces. |
//! | [`examples`] | What the documents *show*: every fenced example compiles, and every one that cannot says why. |
//!
//! ```text
//! cargo test -p buri --test docs
//! ```

mod documents;
mod examples;
