//! The middle end: monomorphization, the typed-tree passes, reference
//! counting, lowering to the IR the native backends read, and how an intrinsic
//! key is classified, which every backend shares.
//!
//! The module tree mirrors where these files lived in `buri`, and the imports
//! below give the lower crates' modules their old names.

pub mod compiler {
    pub mod backend {
        pub mod intrinsic_keys;
    }
    pub mod middle;

    use buri_semantics::compiler::{semantics, standard_library};
}

use buri_diagnostics::{diagnostics, ice, parallel, profile};
use buri_hash::hash;

mod build {
    pub use buri_hash::build::sha256;
}
