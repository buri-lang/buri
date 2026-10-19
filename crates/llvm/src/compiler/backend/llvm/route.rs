//! What this backend emits for each intrinsic key, decided once per program.
//!
//! `middle::lower` gives every key it writes an id. This file reads each key
//! once and answers which of the backend's generators — or which runtime row —
//! emits it, so a key nothing emits is refused before LLVM is started, and
//! emission reads its route by id and never meets a key it has no answer for.

use super::emit::{open_coded_key, prim_leaf};
use crate::compiler::backend::intrinsic_keys::{
    bits_op, derive_key, number_key, numeric_key, Leaf, Num,
};
use crate::compiler::backend::runtime_table::{self as runtime, Entry};
use crate::compiler::middle::ir;
use crate::compiler::semantics::layered::Dense;
use crate::compiler::semantics::types::Prim;

/// Every key a lowered program names, resolved.
pub struct Routes<'p> {
    pub(crate) calls: Dense<ir::IntrinsicId, Route<'p>>,
    pub(crate) bodies: Dense<ir::RuntimeId, Route<'p>>,
}

impl<'p> Routes<'p> {
    /// Every key `lowered` names, resolved, or every key this backend has no
    /// route for.
    pub fn of(lowered: &'p ir::Program) -> Result<Routes<'p>, Vec<String>> {
        let (calls, bodies) = crate::compiler::backend::resolve_program(lowered, route, route)?;
        Ok(Routes { calls, bodies })
    }
}

/// Which generator emits a key, in the order of decreasing knowledge, which is
/// also increasing cost: an operation this backend can *generate* is
/// generated, and an operation the archive has a body for is called.
#[derive(Clone, Copy)]
pub(crate) enum Route<'p> {
    /// `number.<T>.<op>`: the number and the operation.
    Numeric(Num, &'p str),
    CopyOut,
    /// A [`prim_leaf`]: the primitive and the operation.
    Leaf(Prim, &'p str),
    /// `core/bits`: the operation after `bits.`.
    Bits(&'p str),
    /// An [`open_coded_key`], by its key.
    OpenCoded(&'p str),
    Derived(Leaf, Prim),
    Row(&'static Entry),
}

fn route(key: &str) -> Option<Route<'_>> {
    let numeric = number_key(key)
        .filter(|_| numeric_key(key))
        .map(|(prim, op)| Route::Numeric(prim, op));
    let copy_out = || (key == "alloc.copyOut").then_some(Route::CopyOut);
    let leaf = || prim_leaf(key).map(|(prim, op)| Route::Leaf(prim, op));
    let bits = || {
        key.strip_prefix("bits.")
            .filter(|_| bits_op(key))
            .map(Route::Bits)
    };
    let open = || open_coded_key(key).then_some(Route::OpenCoded(key));
    let derived = || derive_key(key).map(|(leaf, prim)| Route::Derived(leaf, prim));
    numeric
        .or_else(copy_out)
        .or_else(leaf)
        .or_else(bits)
        .or_else(open)
        .or_else(derived)
        .or_else(|| runtime::entry(key).map(Route::Row))
}
