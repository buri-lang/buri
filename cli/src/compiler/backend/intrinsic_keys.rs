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

// -- the closure trampoline --------------------------------------------------
//
// A *runtime-driven* closure key is the other half of `list_call`: an entry the
// archive has a body for, which reaches its step back through a generated
// C-ABI **entry thunk** rather than through a loop the backend open-codes.
//
// `cli/runtime/list.rs`'s header says why the archive has none of the loops
// above — "a Buri closure's `code` is a thunk at the *flattened* signature of
// its own element type, so calling one from C would mean synthesizing a
// parameter list that depends on `T`". The trampoline is the answer to that
// sentence and not a contradiction of it: the runtime never synthesizes
// anything, because the four words of [`StepCall`]'s ABI carry a function
// **the backend generated at the call site**, which is where the element type
// is known. What crosses the C boundary is three pointers — the backend's own
// state, one element in, one element out — at every element type there is.
//
// Nothing here replaces `list_call`. An open-coded loop is faster than any
// call per element could be (`stencil/lists.rs`'s header measures it), so a key
// that can be a loop stays one; this table is for the operations whose *body*
// is the runtime's — a scheduler, a socket, a task pool — and which happen to
// take a closure.

/// One runtime-driven closure-taking key.
pub struct StepCall {
    /// Which loop it is, in the vocabulary [`list_call`] already has: the same
    /// operation reached a second way, so it is named the same way.
    pub kind: Step,
    /// The context, where the *step* takes one — the index into the Buri
    /// argument list, as [`ListCall::ctx`].
    pub ctx: Option<usize>,
    /// The closure. **Always the last argument**, which is what lets one C
    /// signature be described by two tables that disagree about everything
    /// else: `runtime_table.rs` has no per-argument column and appends the
    /// step's four words after the flattened arguments, `llvm/runtime.rs` has
    /// one and writes them at the closure's own position, and the two agree
    /// because the closure is where the arguments end.
    /// `the_step_is_the_last_argument`, below, is that claim as a test.
    pub func: usize,
    /// How many arguments the declaration takes, so that "the closure is last"
    /// is checkable rather than remembered.
    pub arity: usize,
}

/// The table. One row today, and it is a **pilot**: `list.mapCtxStep` is
/// `list.mapCtx` with its step reached through the trampoline instead of
/// open-coded, and it exists so that the mechanism `core/tasks` needs lands
/// green — with a conformance fixture and an agreement row behind it — before
/// there is a scheduler to land it with.
///
/// It is deliberately **non-suspending**: the runtime calls the step and comes
/// back, exactly as an open-coded loop would, so the only thing under test is
/// the boundary. `Tasks.parallel` is the same four words with a scheduler
/// behind them.
pub fn step_call(key: &str) -> Option<StepCall> {
    match key {
        "list.mapCtxStep" => Some(StepCall { kind: Step::Map, ctx: Some(1), func: 2, arity: 3 }),
        _ => None,
    }
}

/// Whether a key is runtime-driven, asked ahead of emission.
pub fn step_key(key: &str) -> bool {
    step_call(key).is_some()
}

/// Every key [`step_call`] answers for, so that a reader — and the tests
/// below — can enumerate them rather than rediscover them from a `match`.
/// `the_table_and_the_roll_agree`, below, is what keeps the two from drifting.
pub const STEP_KEYS: &[&str] = &["list.mapCtxStep"];

#[cfg(test)]
mod tests {
    use super::*;

    /// The invariant the two runtime tables rest on. See [`StepCall::func`].
    #[test]
    fn the_step_is_the_last_argument() {
        for key in STEP_KEYS {
            let call = step_call(key).unwrap_or_else(|| panic!("{key}"));
            assert_eq!(call.func + 1, call.arity, "{key}");
        }
    }

    /// The roll and the table name the same keys. A key in one and not the
    /// other is a table whose two readers disagree about what is in it.
    #[test]
    fn the_table_and_the_roll_agree() {
        assert!(!STEP_KEYS.is_empty(), "a mechanism with no key is a mechanism with no test");
        for key in STEP_KEYS {
            assert!(step_key(key), "{key} is on the roll and not in the table");
        }
        for key in ["list.map", "list.mapCtx", "tasks.parallel", "list.mapCtxStepped"] {
            assert_eq!(step_key(key), STEP_KEYS.contains(&key), "{key}");
        }
    }

    /// A runtime-driven key is **not** an open-coded one. The two tables name
    /// disjoint sets of keys, because a key in both would be emitted twice —
    /// whichever the backend asked about first would win, silently.
    #[test]
    fn the_two_closure_tables_are_disjoint() {
        for key in STEP_KEYS {
            assert!(step_call(key).is_some(), "{key}");
            assert!(list_call(key).is_none(), "{key}");
            assert!(!list_closure_key(key), "{key}");
        }
        for key in ["list.map", "list.mapCtx", "list.filterCtx", "list.sortBy"] {
            assert!(list_call(key).is_some(), "{key}");
            assert!(step_call(key).is_none(), "{key}");
            assert!(!step_key(key), "{key}");
        }
    }

    /// The pilot is `mapCtx`'s twin, and the twin's shape is `mapCtx`'s shape:
    /// same loop, same context position, same closure position. A twin that
    /// had drifted would be testing a different operation than the one whose
    /// answer it is compared against.
    #[test]
    fn the_pilot_is_map_ctx_read_a_second_way() {
        let twin = step_call("list.mapCtxStep").expect("the pilot");
        let original = list_call("list.mapCtx").expect("its open-coded twin");
        assert!(twin.kind == Step::Map && original.kind == Step::Map);
        assert_eq!(twin.ctx, original.ctx);
        assert_eq!(twin.func, original.func);
    }
}
