//! Diagnostics and the source map, with the small utilities every crate
//! above them uses: `json`, `parallel` and the `progress` block.
//!
//! The module tree mirrors where these files lived in `buri`, and the imports
//! below give the lower crates' modules their old names, so a path like
//! `crate::documentation::page_of_code` reads the same in every crate.

pub mod diagnostics;
pub mod json;
pub mod parallel;
pub mod profile;
pub mod progress;

use buri_docs::documentation;
use buri_hash::hash;
