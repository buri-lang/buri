//! What every backend shares: the `Backend` trait, targets and options, the
//! runtime's symbol rule and table, and where reference counts live.
//!
//! The module tree mirrors where these files lived in `buri`, and the imports
//! below give the lower crates' modules their old names.

pub mod compiler {
    pub mod backend;

    use buri_middle::compiler::middle;
    use buri_semantics::compiler::semantics;
    #[cfg(test)]
    use buri_semantics::compiler::standard_library;
}

use buri_diagnostics::diagnostics;
use buri_hash::hash;

mod build {
    pub use buri_project::build::buildfile;

    pub mod cache {
        pub use buri_hash::build::action_key::ActionKey;
    }
}
