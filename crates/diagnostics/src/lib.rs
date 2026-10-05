//! Diagnostics and the source map, with the two small utilities every crate
//! above them uses: `json` and `parallel`.
//!
//! The module tree mirrors where these files lived in `buri`, and the imports
//! below give the lower crates' modules their old names, so a path like
//! `crate::documentation::page_of_code` reads the same in every crate.

pub mod diagnostics;
pub mod json;
pub mod parallel;
pub mod profile;

use buri_docs::documentation;
use buri_hash::hash;
