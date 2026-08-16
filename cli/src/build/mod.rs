//! The build system: what a repository declares, and what the toolchain does
//! with it.
//!
//! `workspace` is the graph — packages, targets, labels, visibility, tags,
//! platforms — read from the `BUILD.buri` and `REPO.buri` files that
//! `buildfile` types and `textproto` parses. `actions` is what a target's
//! build actually does, `cache` decides whether it has to happen at all, and
//! `regenerate` writes back the fields of a build file that merely restate the
//! sources.
//!
//! `session` is the handle every command that needs a repository opens first:
//! the root, the loaded workspace, the source map, and the diagnostics.

pub mod actions;
pub mod buildfile;
pub mod cache;
pub mod regenerate;
pub mod session;
pub mod textproto;
pub mod workspace;
