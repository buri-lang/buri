//! What a repository declares: its build files, the textproto they are written
//! in, the bundled platforms, the languages it checks, and the vocabulary of the
//! build graph.
//!
//! Below the checker, because the checker reads platforms, packages and module
//! locations. Loading a whole workspace runs generators and tools, so that half
//! is in `buri`.
//!
//! The module tree mirrors where these files lived in `buri`, and the imports
//! below give the lower crates' modules their old names.

pub mod build {
    pub mod buildfile;
    pub mod platforms;
    pub mod textproto;
    pub mod workspace;
}
pub mod languages;

use buri_diagnostics::{diagnostics, json};
use buri_docs::documentation;
use buri_syntax::layout;
