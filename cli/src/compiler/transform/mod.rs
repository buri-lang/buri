//! Whole-program passes over the typed tree.
//!
//! All three run after the front end and before the backend, which is the one
//! point where every module is present, every type is concrete, and nothing
//! has been committed to JavaScript yet. They run in this order:
//! `monomorphize` (one concrete function per instantiation, which makes the
//! call graph exact), `optimize` (decisions taken from that exact graph), then
//! `tail_calls` (the elimination SPEC 8.3 requires, which JavaScript will not
//! do for us).

pub mod monomorphize;
pub mod optimize;
pub mod tail_calls;
