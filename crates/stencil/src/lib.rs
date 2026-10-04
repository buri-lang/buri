//! The copy-and-patch backend: native debug builds from a library of
//! precompiled stencils, which `build.rs` builds with the host's C compiler.
//!
//! The module tree mirrors where these files lived in `buri`, and the imports
//! below give the lower crates' modules their old names.

pub mod compiler {
    pub mod backend {
        pub mod stencil;

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
