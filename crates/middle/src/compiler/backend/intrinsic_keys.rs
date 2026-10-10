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
//! What is **not** here is anything a backend decides for itself: each
//! backend's `open_coded_key` names the keys it in particular turns into
//! instructions. `core/list`'s closure keys are no backend's at all —
//! `middle::lower` builds them as loops (`lower/lists.rs`).

use crate::compiler::semantics::builtins::conversion_is_exact;
use crate::compiler::semantics::types::{Prim, Tables, Ty, TyKind};

/// The `Equal`/`Ordered`/`Hash`/`Show` leaves at `Bool` and `Char`, plus
/// `Char::toU32`, and `Str`'s `show`.
///
/// These are four *language* answers, and two backends must not give different
/// ones.
pub fn prim_trait_op(key: &str) -> bool {
    matches!(
        key,
        "bool.equal"
            | "bool.compare"
            | "bool.hash"
            | "bool.show"
            | "character.equal"
            | "character.compare"
            | "character.hash"
            | "character.show"
            | "character.toU32"
            | "str.show"
    )
}

/// Whether a key has a body on every native backend with no table row or
/// open-coded sequence of its own: a loop `middle::lower` builds, a `core/bits`
/// operation, a derive leaf, a `number.<T>.<op>` operation, or a
/// [`prim_trait_op`]. Each backend's `implemented` adds what it claims alone.
pub fn native_body(key: &str) -> bool {
    crate::compiler::middle::lower::lowers(key)
        || bits_op(key)
        || prim_trait_op(key)
        || derive_key(key).is_some()
        || numeric_key(key)
}

/// `core/lazy`'s one declaration, and the node the split pass leaves where a
/// call to it stood.
///
/// Two names for one feature, because they belong to two moments. [`LAZY_LOAD`]
/// is the key `core/lazy`'s bodyless `load` monomorphizes to.
/// `middle::chunks` finds every call to it, decides which of them can be split,
/// and rewrites the call into a [`lazy_chunk_key`] node carrying the chunk's
/// number. **No backend ever sees `lazy.load`**: a native build has the call
/// replaced by its argument and a JavaScript one has it replaced by the chunk
/// node. So this is what two passes agree through, rather than something a code
/// generator implements.
pub const LAZY_LOAD: &str = "lazy.load";

/// The prefix a chunk node's name carries. The chunk's number follows it.
pub const LAZY_CHUNK: &str = "lazy.chunk.";

/// The name of the node that fetches chunk `n`.
pub fn lazy_chunk_key(n: usize) -> String {
    format!("{LAZY_CHUNK}{n}")
}

/// The chunk a node names, or `None` for every other intrinsic.
pub fn lazy_chunk_of(key: &str) -> Option<usize> {
    key.strip_prefix(LAZY_CHUNK)?.parse().ok()
}

/// The `core/bits` operations, asked ahead of emission.
///
/// The unsigned-width family is spelled out rather than derived from a suffix,
/// because `core/bits` declares exactly these and a rule that accepted
/// `shlU16` would claim something that does not exist.
pub fn bits_op(key: &str) -> bool {
    matches!(
        key,
        "bits.shiftLeft"
            | "bits.shiftRight"
            | "bits.shiftRightArithmetic"
            | "bits.popCount"
            | "bits.leadingZeros"
            | "bits.trailingZeros"
            | "bits.rotateLeft"
            | "bits.rotateRight"
            | "bits.shiftLeftU8"
            | "bits.shiftRightU8"
            | "bits.shiftLeftU32"
            | "bits.shiftRightU32"
            | "bits.shiftLeftU64"
            | "bits.shiftRightU64"
            | "bits.rotateLeftU8"
            | "bits.rotateRightU8"
            | "bits.rotateLeftU32"
            | "bits.rotateRightU32"
            | "bits.rotateLeftU64"
            | "bits.rotateRightU64"
            | "bits.byteSwapU32"
            | "bits.byteSwapU64"
            | "bits.popCountU64"
            | "bits.leadingZerosU64"
            | "bits.trailingZerosU64"
    )
}

/// `derivePrimShow.I64`, `derivePrimHash.U8` and `derivePrimJson.Bool` and
/// their siblings, split into the operation and the primitive it is at.
///
/// The three type-directed leaves `middle::derives` bottoms out at, and the
/// set is spelled here rather than in each backend because it is a fact about
/// that pass: it emits these three names and no others (`derives.rs`'s table
/// of eight, of which the other five are the `deriveArray*` loops).
pub fn derive_key(key: &str) -> Option<(&str, Prim)> {
    let (name, target) = key.split_once('.')?;
    if !matches!(name, "derivePrimShow" | "derivePrimHash" | "derivePrimJson") {
        return None;
    }
    let prim = Prim::all().iter().copied().find(|p| p.name() == target)?;
    Some((name, prim))
}

/// Which arm of `core/json`'s `Json` a primitive encodes to.
///
/// `$json_of`'s primitive arm (`backend/js/runtime.js`) read as a three-way
/// answer: a `Bool` is a JSON boolean; a `Str` or a `Char` is a JSON string,
/// because a `Char` is a one-scalar string and JavaScript already makes it
/// one; and everything else is JSON's single number type, a double.
///
/// VALUE-MODEL.md §12 row 10 is byte-for-byte agreement on that encoding, so
/// the mapping is one function here rather than one `match` per backend.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum JsonArm {
    Bool,
    /// `.Num(Float)`. Every numeric primitive widens to the double JSON has,
    /// which rounds above 2^53 — and rounds in the same place on JavaScript,
    /// which is what `json.buri`'s header says a JSON number is.
    Num,
    Str,
}

/// The arm a primitive encodes to. Total: every primitive is one of the three.
pub fn json_arm(prim: Prim) -> JsonArm {
    match prim {
        Prim::Bool => JsonArm::Bool,
        Prim::Str | Prim::Template | Prim::Char => JsonArm::Str,
        _ => JsonArm::Num,
    }
}

/// Where that arm sits in `Json`'s variant list.
///
/// **By name**, for the reason `types::result_shape` is one: `core/json`
/// declares `Null` first, and a backend that hard-coded `1`, `2`, `3` would be
/// reading a declaration order out of a table that records it.
/// `middle::derives` builds the *compound* arms of the same enum through its
/// own `Environment::json_variant`, asking the same question of the same declaration;
/// this is the half a **backend** needs, because a primitive leaf is an
/// intrinsic and never reaches that pass's builder.
///
/// `None` for a type that is not a declared enum with such a variant — a
/// `derive ToJson` whose result is not `core/json`'s `Json`, which is refused
/// rather than guessed at.
pub fn json_variant(tables: &Tables, ty: &Ty, arm: JsonArm) -> Option<usize> {
    let TyKind::Con(id, _) = ty.kind() else { return None };
    let name = match arm {
        JsonArm::Bool => "Bool",
        JsonArm::Num => "Num",
        JsonArm::Str => "Str",
    };
    tables.tycon(*id).variants().iter().position(|v| v.name == name)
}

/// The target of a numeric conversion's operation name: `toI64`, `wrapToU8`.
///
/// `Char`, `Str`, `Bool` and `Template` are excluded as targets even though
/// `Prim::all` lists them: `U32.toChar` answers a `Result` because not every
/// `U32` is a Unicode scalar, and `conversion_is_exact` — which classifies by
/// `is_integer`/`is_float` — would call it exact by falling into its
/// integer-to-float arm. Each backend asks for `toChar` by name.
pub fn conversion_target(op: &str) -> Option<Prim> {
    let name = op.strip_prefix("wrapTo").or_else(|| op.strip_prefix("to"))?;
    Prim::all().iter().copied().find(|p| p.name() == name && (p.is_integer() || p.is_float()))
}

/// The shapes a fallible `toT()` conversion comes in.
///
/// SPEC 6.2.1 gives one rule — `x.toT()` answers `Result<T, RangeError>`
/// wherever not every `x` fits a `T` — and the rule reaches four different
/// questions, because "does not fit" is not one machine test. Both backends
/// switch on it to emit the range check, and [`numeric_key`] asks it whether a
/// key with an inexact target has a body, so a claim never outruns an
/// implementation.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum CheckedKind {
    /// Both integers: a range at the source's own width.
    Ints,
    /// A float into an integer, which can also be fractional, `NaN` or
    /// infinite. Refused past sixty-four bits, where neither backend has a
    /// single conversion for it.
    FloatToInt,
    /// `U32 -> Char`: not a range but a set, with the surrogate block cut out
    /// of the middle of it.
    ToChar,
    /// `F64 -> F32`, which has no integer range to test at all.
    ToF32,
}

/// Which fallible shape a conversion is, or `None` when the pair is exact,
/// modular, or one no backend has a body for (a float into a 128-bit integer).
pub fn checked_kind(from: Prim, to: Prim) -> Option<CheckedKind> {
    if from == to {
        return None;
    }
    if from.is_integer() && to.is_integer() {
        return (!conversion_is_exact(from, to)).then_some(CheckedKind::Ints);
    }
    if from.is_float() && to.is_integer() && to.bits() <= 64 {
        return Some(CheckedKind::FloatToInt);
    }
    // `U32` is the only source the language declares `toChar` on
    // (`semantics/builtins.rs`), and both backends test the bounds at that
    // width.
    if from == Prim::U32 && to == Prim::Char {
        return Some(CheckedKind::ToChar);
    }
    if from == Prim::F64 && to == Prim::F32 {
        return Some(CheckedKind::ToF32);
    }
    None
}

/// The `number.<T>.<op>` operations both native backends emit a body for,
/// asked before emission.
///
/// `missing_intrinsics` is asked of the *monomorphized* program, before
/// `middle::lower` runs — so `Bounded` is still two segments there
/// (`number.minValue`) and three by the time the body is emitted. Both
/// spellings answer yes, because both describe an operation the backends
/// compile.
///
/// The list is what the backends dispatch on, not `number.*`: claiming a key
/// with no body would turn a diagnostic that names the operation into one that
/// names an IR shape. That is why a conversion is claimed only where it is
/// exact or [`checked_kind`] names its range test — `F64.toI128` has no body
/// on either backend.
pub fn numeric_key(key: &str) -> bool {
    if key == "number.minValue" || key == "number.maxValue" {
        return true;
    }
    let mut parts = key.split('.');
    let (Some("number"), Some(name), Some(op), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return false;
    };
    let Some(prim) = Prim::all().iter().copied().find(|p| p.name() == name) else {
        return false;
    };
    if matches!(
        op,
        "add"
            | "subtract"
            | "multiply"
            | "divide"
            | "remainder"
            | "negate"
            | "abs"
            | "signum"
            | "equal"
            | "compare"
            | "show"
            | "hash"
            | "wrappingAdd"
            | "wrappingSubtract"
            | "wrappingMultiply"
            | "minValue"
            | "maxValue"
    ) {
        return true;
    }
    // `Checked`, `Wrapping` and `Saturating` are declared on the integer types
    // only (`semantics/builtins.rs`), so a float spelling of one is a key that
    // does not exist rather than one a backend declines.
    if matches!(
        op,
        "checkedAdd"
            | "checkedSubtract"
            | "checkedMultiply"
            | "checkedDivide"
            | "checkedRemainder"
            | "checkedNegate"
            | "checkedPower"
            | "saturatingAdd"
            | "saturatingSubtract"
            | "saturatingMultiply"
    ) {
        return prim.is_integer();
    }
    // `U32.toChar` answers a `Result<Char, RangeError>` and `Char` is not a
    // numeric target, so [`conversion_target`] does not name it.
    if op == "toChar" {
        return checked_kind(prim, Prim::Char).is_some();
    }
    conversion_target(op).is_some_and(|to| {
        op.starts_with("wrapTo") || conversion_is_exact(prim, to) || checked_kind(prim, to).is_some()
    })
}

/// Which loop a closure-taking `list.*` key is.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Step {
    Map,
    Filter,
    /// `filterMap`: `map` keeping only the `.Some` payloads, written into one
    /// block the length of the list.
    FilterMap,
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
    /// context argument — `Allocator`, for the block they build — and only the
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
        "list.filterMap" => call(Step::FilterMap, None, 2, None),
        "list.filterMapCtx" => call(Step::FilterMap, Some(1), 2, None),
        // `sortBy(self, ctx, order)`: the `C: Allocator` bound is for the block the
        // sort builds, and the comparator never sees it — so `ctx` is `None`
        // here for the same reason it is on `map`.
        "list.sortBy" => call(Step::Sort, None, 2, None),
        _ => None,
    }
}

// -- the closure trampoline --------------------------------------------------
//
// A *runtime-driven* closure key is the other half of `list_call`: an entry the
// archive has a body for, which reaches its step back through a generated
// C-ABI **entry thunk** rather than through a loop in the IR.
//
// `cli/runtime/list.rs`'s header says why the archive has none of the loops
// above — "a Buri closure's `code` is a thunk at the *flattened* signature of
// its own element type, so calling one from C would mean synthesizing a
// parameter list that depends on `T`". The trampoline is the answer to that
// sentence and not a contradiction of it: the runtime never synthesizes
// anything, because the four words of [`StepCall`]'s ABI carry a function
// **the backend generated at the call site**, which is where the element type
// is known. What crosses the C boundary is three pointers and a number — the
// backend's own state, which item this is, one element in, one element out — at
// every element type there is.
//
// Nothing here replaces `list_call`. A loop the code generator sees is faster
// than any call per element could be, so a key that can be a loop stays one
// (`middle/lower/lists.rs`); this table is for the operations whose *body*
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
    /// The closure. **Always the last argument**, which is what lets the two
    /// backends emit one C signature from one `runtime_table.rs` row: the
    /// frame-threaded one appends the step's four words after the flattened
    /// arguments, the LLVM one writes them at the closure's own position, and
    /// the two agree because the closure is where the arguments end.
    /// `the_step_is_the_last_argument`, below, is that claim as a test.
    pub func: usize,
    /// How many arguments the declaration takes, so that "the closure is last"
    /// is checkable rather than remembered.
    pub arity: usize,
    /// Which of the **closure's own parameters** receives the runtime's index,
    /// where the closure takes one.
    ///
    /// This is the one field that indexes into the step's signature rather than
    /// into the intrinsic's argument list, and it has to: the index is not a
    /// Buri argument at all. It is the loop counter, which only the runtime
    /// has, so it arrives as the second word of `StepEntry` and the entry thunk
    /// writes it into the parameter this names.
    ///
    /// `None` is a step that is not told where it is, whose closure is
    /// `fn(C, A) => B`. The word still crosses (one C signature, not one per
    /// key); the thunk ignores it, and no slot is reserved for it in the state
    /// record.
    pub index: Option<usize>,
}

/// The table. Two rows.
///
/// `host.HostTasks.parallel` is what the mechanism was built for. Four
/// words, and today one walk — the native body runs the steps in index
/// order on the calling thread (`cli/runtime/rt.rs`) — with a scheduler behind
/// them in D4. It is the row that makes the index parameter necessary:
/// `effect Tasks` hands the step its item's own index, and only the side
/// driving the walk knows one.
pub fn step_call(key: &str) -> Option<StepCall> {
    match key {
        // `parallel(self, ctx, items, f)`, with `f: fn(C, Int, A) => B`. The
        // receiver carries the *effect* and `ctx` carries the *authority*, and
        // they are two arguments because they are two values: argument 0
        // dispatches — it is the scheduler, and for `TestTasks` it is a live
        // handle the runtime reads — while argument 1 is the caller's whole
        // context, which is what `Tasks` promises every step. Pointing this
        // column at argument 0 is what handed a step the implementation, so a
        // step reading a clock out of its context read the scheduler's bytes
        // instead. The index is the closure's second parameter, between that
        // context and the element.
        "host.HostTasks.parallel" => {
            Some(StepCall { kind: Step::Map, ctx: Some(1), func: 3, arity: 4, index: Some(1) })
        }
        // `platform/effect/testing`'s scheduler, at the same four arguments. The
        // double reaches its steps through this boundary rather than through a
        // Buri loop of its own, and that is the point of it: what a test runs
        // its program through is the mechanism the program will ship on, with
        // one decision — the order — changed. Argument 0 is the live handle
        // this one genuinely reads; argument 1 is the context, as above.
        "host_testing.TestTasks.parallel" => {
            Some(StepCall { kind: Step::Map, ctx: Some(1), func: 3, arity: 4, index: Some(1) })
        }
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
pub const STEP_KEYS: &[&str] = &["host.HostTasks.parallel", "host_testing.TestTasks.parallel"];

/// Whether this key **hands its step the caller's context**, and so waits
/// exactly when the step waits.
///
/// The two tables above already record it, in the one column that separates
/// `map` from `mapCtx`: a step handed a context can reach every effect the
/// caller holds, so it may dial a socket, sleep, or ask an actor, and a step
/// that cannot be handed one may not. That is the whole of the question
/// [`crate::compiler::backend::js::park::suspends`] cannot answer on its own —
/// `suspends` is a list of keys whose wait is the *key's* own, and a
/// combinator's wait is its caller's — so `js::park`'s parkability walk asks
/// this one instead and reads the step that actually arrived.
///
/// Asked of both tables because a key belongs to one or the other: `mapCtx` is
/// open-coded and `Tasks.parallel` is runtime-driven.
pub fn ctx_step_key(key: &str) -> bool {
    list_call(key).is_some_and(|c| c.ctx.is_some()) || step_call(key).is_some_and(|c| c.ctx.is_some())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The three type-directed leaves, and nothing else.
    ///
    /// Both directions, because both are wrong in a way nothing else reports:
    /// a name this misses is an intrinsic a backend refuses at a key it
    /// actually implements, and a name it accepts that `middle::derives` never
    /// emits is a claim about a body that does not exist.
    #[test]
    fn the_derive_leaves_are_the_three_that_are_emitted() {
        for name in ["derivePrimShow", "derivePrimHash", "derivePrimJson"] {
            let key = format!("{name}.I64");
            assert_eq!(derive_key(&key), Some((name, Prim::I64)), "{key}");
        }
        for key in [
            // The array loops are not leaves and take a code pointer.
            "deriveArrayShow",
            "deriveArrayJson",
            // Unqualified is what `middle::lower` produces for a `derivePrim*`
            // at something that is not a primitive, which is a bug in
            // `derives.rs` and is deliberately not claimed.
            "derivePrimJson",
            "derivePrimShow",
            // A primitive that is not one.
            "derivePrimJson.Point",
            "str.concat",
        ] {
            assert!(derive_key(key).is_none(), "{key}");
        }
    }

    /// [`json_arm`] is total, and it agrees with `$json_of`'s three tests.
    ///
    /// The `Char` row is the one worth an assertion of its own: it is a JSON
    /// **string** and not a number, which is what `runtime.js` does with
    /// `p === "c"` and what a reader expecting `Char::toU32` would get wrong.
    #[test]
    fn every_primitive_encodes_to_one_json_arm() {
        assert_eq!(json_arm(Prim::Bool), JsonArm::Bool);
        assert_eq!(json_arm(Prim::Str), JsonArm::Str);
        assert_eq!(json_arm(Prim::Char), JsonArm::Str);
        assert_eq!(json_arm(Prim::Template), JsonArm::Str);
        // Everything else is a number, asserted over the whole roll rather
        // than over a list here, so that a primitive added later cannot
        // quietly acquire a fourth answer or fall through to the wrong one.
        for prim in Prim::all() {
            if matches!(*prim, Prim::Bool | Prim::Str | Prim::Char | Prim::Template) {
                continue;
            }
            assert_eq!(json_arm(*prim), JsonArm::Num, "{}", prim.name());
        }
    }

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
        for key in [
            "list.map",
            "list.mapCtx",
            "tasks.parallel",
            // `core/tasks::parallel` forwards to the effect method and is
            // ordinary Buri, so the key that reaches a backend is the `impl`'s.
            "host.HostTasks",
            "host.HostTasks.parallelly",
        ] {
            assert_eq!(step_key(key), STEP_KEYS.contains(&key), "{key}");
        }
    }

    /// The index is a property of the *key*, and only one key has one.
    ///
    /// It cannot be derived from the signature — two steps can take the same
    /// types and mean different things by the second parameter — so it is in
    /// the table, and both backends read it from here. What it names is a
    /// position in the *closure's* parameters, and the two facts that make that
    /// position usable are asserted rather than assumed: it is not the element
    /// (which is last and travels through `arg`), and it is not the context
    /// (which is a Buri argument and travels in the record).
    #[test]
    fn only_a_key_that_promises_an_index_is_given_one() {
        let tasks = step_call("host.HostTasks.parallel").expect("the scheduler");
        assert_eq!(tasks.index, Some(1), "`effect Tasks` names the index second");
        assert_ne!(tasks.index, Some(tasks.arity - 1), "the index is not the element");
        // A step told where it is still takes its context out of the record,
        // and that context is still the argument the table names — argument 1,
        // `ctx`, and not argument 0, which is the scheduler. The two are
        // different values and the assertion says which is which, because
        // naming the receiver here is the bug this row is the fix for.
        assert_eq!(tasks.ctx, Some(1), "`parallel`'s context is its own parameter");
        assert_ne!(tasks.ctx, Some(0), "argument 0 is the implementation, not the context");
        for key in STEP_KEYS {
            let call = step_call(key).expect(key);
            assert_ne!(call.ctx, Some(call.func), "{key}: the closure is not its own context");
            assert_ne!(call.ctx, None, "{key}: a step is always handed a context");
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
        }
        for key in ["list.map", "list.mapCtx", "list.filterCtx", "list.sortBy"] {
            assert!(list_call(key).is_some(), "{key}");
            assert!(step_call(key).is_none(), "{key}");
            assert!(!step_key(key), "{key}");
        }
    }

    /// Which keys hand their step a context, in both directions.
    ///
    /// This is the list `js::park`'s parkability walk reads to decide that a
    /// combinator waits when its step does, so it is the one place a `*Ctx`
    /// spelling and a plain one are told apart. Both halves are asserted: a
    /// key missing from it is a step that waits and is not waited for, and a
    /// key wrongly in it is a `map` or a comparator paying a promise for a
    /// context it was never handed.
    #[test]
    fn only_a_key_handed_a_context_is_a_ctx_step() {
        for key in [
            "list.foldCtx",
            "list.foldResultCtx",
            "list.mapCtx",
            "list.filterCtx",
            "list.filterMapCtx",
            "host.HostTasks.parallel",
            "host_testing.TestTasks.parallel",
        ] {
            assert!(ctx_step_key(key), "{key} is handed the caller's context");
        }
        for key in [
            "list.fold",
            "list.foldResult",
            "list.map",
            "list.filter",
            "list.sortBy",
            "list.any",
            "list.all",
            "list.find",
            "list.findIndex",
            "list.count",
            "list.length",
            "str.split",
        ] {
            assert!(!ctx_step_key(key), "{key} is not");
        }
    }
}
