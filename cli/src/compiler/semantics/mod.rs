//! Names and types: the syntax tree in, the typed tree out.
//!
//! `resolve` runs first and settles what every name means; `inference` then
//! checks one body at a time, with `expressions` and `patterns` as its two
//! halves and `types` holding the tables and the unifier both of them use.
//! `exhaustiveness` is the one check that runs over a body the others have
//! already typed, and `builtins` is what the primitives carry before any
//! source file is read.
//!
//! `typed` is the output — the tree `middle` and `backend` consume.

pub mod builtins;
pub mod consteval;
pub mod exhaustiveness;
pub mod expressions;
pub mod inference;
pub mod patterns;
pub mod resolve;
pub mod styles;
pub mod typed;
pub mod types;
