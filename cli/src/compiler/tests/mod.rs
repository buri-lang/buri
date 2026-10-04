//! Unit tests of the lower crates' passes that compile a whole snippet, which
//! needs `driver`, and so live here rather than beside the pass.

mod exhaustiveness;
mod expressions;
mod resolve;
mod standard_library;
