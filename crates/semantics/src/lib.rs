//! The checker: name resolution, type inference, effects, exhaustiveness, and
//! the typed tree they produce. With it, the data a compilation is made of and
//! the questions about the standard library that need the checker's types.
//!
//! The module tree mirrors where these files lived in `buri`, and the imports
//! below give the lower crates' modules their old names.

pub mod compiler {
    pub mod modules;
    pub mod semantics;
    pub mod standard_library;
}

use buri_diagnostics::{diagnostics, ice};
use buri_docs::documentation;
use buri_hash::hash;
use buri_syntax::{formatting, parsing};

mod build {
    pub use buri_hash::build::sha256;
    pub use buri_project::build::{buildfile, workspace};
}
