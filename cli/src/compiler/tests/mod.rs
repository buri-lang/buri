//! Unit tests of the lower crates' passes that compile a whole snippet, which
//! needs `driver`, and so live here rather than beside the pass.

/// The id a function's table mints for its local at `i`, for tests that write
/// a body by hand.
pub(crate) fn local_id(i: u32) -> crate::compiler::semantics::types::LocalId {
    use crate::compiler::semantics::layered::Dense;
    let table: Dense<_, ()> = std::iter::repeat_n((), i as usize + 1).collect();
    table.ids().last().expect("the table holds `i + 1` locals")
}

/// The id a program's table mints for its function at `i`.
pub(crate) fn func_id(i: u32) -> crate::compiler::semantics::types::FuncIdx {
    use crate::compiler::semantics::layered::Dense;
    let table: Dense<_, ()> = std::iter::repeat_n((), i as usize + 1).collect();
    table
        .ids()
        .last()
        .expect("the table holds `i + 1` functions")
}

mod derives;
mod exhaustiveness;
mod expressions;
mod lower;
mod park;
mod rc;
mod resolve;
mod standard_library;
