//! The JavaScript backend: the `node` and `web` platforms' artifacts.
//!
//! The module tree mirrors where these files lived in `buri`, and the imports
//! below give the lower crates' modules their old names.

pub mod compiler {
    pub mod backend {
        pub mod js;

        use buri_backend::compiler::backend::*;
    }

    use buri_middle::compiler::middle;
    use buri_semantics::compiler::semantics;
}

use buri_diagnostics::{diagnostics, ice, parallel};
use buri_hash::hash;

mod build {
    pub mod cache {
        pub use buri_hash::build::action_key::ActionKey;
    }
}
