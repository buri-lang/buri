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
//! `protoschema` and `protogen` are the other direction: a `.proto` file in a
//! package is read as a schema and *becomes* a Buri module, so that
//! `from "//proto/person.proto" import { Person };` resolves to types and
//! codecs that no one had to write down twice.
//!
//! `link` is the last action in the graph for a native artifact: the C driver
//! the objects are handed to, the `.buri/link/<key>/` directory they are
//! written into, and the manifest that says which of them this build produced
//! and which came out of the cache.
//!
//! `session` is the handle every command that needs a repository opens first:
//! the root, the loaded workspace, the source map, and the diagnostics.
//! `spawn` is the deterministic environment the one action that leaves this
//! process is started in.

pub mod actions;
pub mod buildfile;
pub mod cache;
pub mod link;
pub mod protogen;
pub mod protoschema;
pub mod regenerate;
pub mod session;
pub mod spawn;
pub mod textproto;
pub mod workspace;
