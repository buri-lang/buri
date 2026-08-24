//! How an intrinsic key is classified, for every backend that classifies it
//! the same way.
//!
//! A backend asks two questions of a `Body::Intrinsic` key before it emits
//! anything: is this one I open-code, and if so which shape is it. The answers
//! below are properties of the *language* — `core/bits` declares exactly six
//! unsigned-width shifts, `core/list` declares exactly these closure-taking
//! entries with exactly these argument positions — so they are one table here
//! rather than one per code generator. A second copy of such a table is a
//! second chance for two backends to disagree about what a program means.
//!
//! What is **not** here is anything a backend decides for itself. The stencil
//! backend open-codes a strict subset of `core/list` and keeps its own
//! [`super::stencil::lists::list_call`] for that reason, and each backend's
//! `open_coded_key` names the keys it in particular turns into instructions.

use crate::compiler::semantics::types::Prim;

/// The `Eq`/`Ord`/`Hash`/`Show` leaves at `Bool` and `Char`, plus
/// `Char::toU32`, and `Str`'s `show`.
///
/// These are four *language* answers, and two backends must not give different
/// ones.
pub fn prim_trait_op(key: &str) -> bool {
    matches!(
        key,
        "bool.eq"
            | "bool.compare"
            | "bool.hash"
            | "bool.show"
            | "char.eq"
            | "char.compare"
            | "char.hash"
            | "char.show"
            | "char.toU32"
            | "str.show"
    )
}

/// The `core/bits` operations, asked ahead of emission.
///
/// The unsigned-width family is spelled out rather than derived from a suffix,
/// because `core/bits` declares exactly these six (`bits.buri:24-29`) and a
/// rule that accepted `shlU16` would claim something that does not exist.
pub fn bits_op(key: &str) -> bool {
    matches!(
        key,
        "bits.shl"
            | "bits.shr"
            | "bits.sar"
            | "bits.popCount"
            | "bits.leadingZeros"
            | "bits.trailingZeros"
            | "bits.rotateLeft"
            | "bits.rotateRight"
            | "bits.shlU8"
            | "bits.shrU8"
            | "bits.shlU32"
            | "bits.shrU32"
            | "bits.shlU64"
            | "bits.shrU64"
    )
}

/// `derivePrimShow.I64` and `derivePrimHash.U8` and their siblings, split into
/// the operation and the primitive it is at.
pub fn derive_key(key: &str) -> Option<(&str, Prim)> {
    let (name, target) = key.split_once('.')?;
    if !matches!(name, "derivePrimShow" | "derivePrimHash") {
        return None;
    }
    let prim = Prim::all().iter().copied().find(|p| p.name() == target)?;
    Some((name, prim))
}

/// Which loop a closure-taking `list.*` key is.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Step {
    Map,
    Filter,
    Fold,
    /// `foldResult` and `foldResultCtx`: a fold that stops at the first `.Err`.
    FoldResult,
    Sort,
    Any,
    All,
    Count,
    /// `find`: the first element the predicate keeps, as an `Option<T>`.
    Find,
    /// `findIndex`: that element's index, as an `Option<Int>`.
    FindIndex,
}

/// One such key, with the argument positions `core/list` declares.
pub struct ListCall {
    pub kind: Step,
    /// The context, where the *step* takes one. `map` and `mapCtx` both have a
    /// context argument — `Alloc`, for the block they build — and only the
    /// second passes it on, because a lambda may not capture one (SPEC 10.6).
    pub ctx: Option<usize>,
    pub func: usize,
    /// `fold`'s initial accumulator.
    pub init: Option<usize>,
}

/// The table, in `list.buri`'s order. Receiver first, context second,
/// everything else after (SPEC 10.7), which is what fixes every index here.
pub fn list_call(key: &str) -> Option<ListCall> {
    let call = |kind, ctx, func, init| Some(ListCall { kind, ctx, func, init });
    match key {
        "list.fold" => call(Step::Fold, None, 1, Some(2)),
        "list.foldCtx" => call(Step::Fold, Some(1), 2, Some(3)),
        "list.foldResult" => call(Step::FoldResult, None, 1, Some(2)),
        "list.foldResultCtx" => call(Step::FoldResult, Some(1), 2, Some(3)),
        "list.find" => call(Step::Find, None, 1, None),
        "list.findIndex" => call(Step::FindIndex, None, 1, None),
        "list.any" => call(Step::Any, None, 1, None),
        "list.all" => call(Step::All, None, 1, None),
        "list.count" => call(Step::Count, None, 1, None),
        "list.map" => call(Step::Map, None, 2, None),
        "list.mapCtx" => call(Step::Map, Some(1), 2, None),
        "list.filter" => call(Step::Filter, None, 2, None),
        "list.filterCtx" => call(Step::Filter, Some(1), 2, None),
        // `sortBy(self, ctx, order)`: the `C: Alloc` bound is for the block the
        // sort builds, and the comparator never sees it — so `ctx` is `None`
        // here for the same reason it is on `map`.
        "list.sortBy" => call(Step::Sort, None, 2, None),
        _ => None,
    }
}

/// The keys a backend with a general closure call emits a loop for, asked
/// ahead of emission.
pub fn list_closure_key(key: &str) -> bool {
    list_call(key).is_some()
}
