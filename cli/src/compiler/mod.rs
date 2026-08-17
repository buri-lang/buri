//! The compiler, one directory per stage.
//!
//! A module's source travels in one direction and never comes back:
//!
//! ```text
//! parsing  ->  semantics  ->  middle  ->  backend
//!  tree         typed         typed       JavaScript, and later a native object
//! ```
//!
//! `parsing` is a sibling of this module rather than a stage inside it,
//! because the formatter and the linter read a syntax tree without ever
//! reaching a type. Everything from `semantics` on is the compiler's alone.
//!
//! The three files here are the parts that are about a compilation rather than
//! about a stage of one: `modules` decides which files are in it, `driver`
//! runs the front end over them, and `standard_library` supplies the modules
//! every compilation gets without asking.

pub mod backend;
pub mod driver;
pub mod middle;
pub mod modules;
pub mod semantics;
pub mod standard_library;
