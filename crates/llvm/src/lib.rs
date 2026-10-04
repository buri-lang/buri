//! The LLVM backend: native release builds. `buri` depends on this crate only
//! under `backend-llvm`, because it needs LLVM 21 installed.
//!
//! The module tree mirrors where these files lived in `buri`, and the imports
//! below give the lower crates' modules their old names.

pub mod compiler {
    pub mod backend {
        pub mod llvm;

        use buri_backend::compiler::backend::*;
    }

    use buri_middle::compiler::middle;
    use buri_semantics::compiler::semantics;
}

use buri_diagnostics::{diagnostics, parallel};
use buri_hash::hash;

mod build {
    pub use buri_project::build::buildfile;
}
