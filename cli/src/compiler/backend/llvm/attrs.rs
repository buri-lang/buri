//! The attribute discipline.
//!
//! `design/native/CODEGEN-LLVM.md` §0's third instruction, answered in its §3.
//! The effect system supplies
//! most of it for free: a language where "does this function touch the world?" is a
//! syntactic property of its signature can answer LLVM's memory-effect
//! questions without an analysis.
//!
//! # What the middle end actually delivers, which is not quite what §3.1 says
//!
//! CODEGEN-LLVM.md §3.1 reads as though `ir::Purity::Pure` already means
//! "`memory(none)` plus `willreturn`", with the theorem's two qualifiers folded
//! in. **It does not, and this is the load-bearing correction of this file.**
//!
//! `rc::infer_effects` runs one fixpoint with *two independent columns*
//! (`rc.rs:760-828`): `purity` joins over `worse(Pure < Allocating <
//! Effectful)`, and `aborts` is a separate boolean lattice. Nothing in the
//! transfer function lets `a` raise `p` — the division arm
//! (`rc.rs:806-810`) sets `a = true` and leaves `p` at `Pure`. So
//! `fn f(a: Int, b: Int): Int { a / b }` arrives here as
//! `Purity::Pure` **with** `can_abort: true`, and an abort writes to stderr and
//! `_exit`s, which is an observable effect.
//!
//! A backend that mapped `Purity::Pure -> memory(none)` on its own would
//! therefore miscompile every function that can divide. [`memory_effects`] ANDs
//! the two columns, which is what the doc's table row *"Same, and can abort |
//! add `inaccessiblemem: write`"* (CODEGEN-LLVM.md:143) says to do, and the
//! `ir::Purity::Pure` doc comment (`ir.rs:565`, "cannot abort") is the sentence
//! that is wrong about the code rather than the code being wrong about itself.
//!
//! The second qualifier — "in the absence of undefined behaviour" — is not
//! modelled anywhere in the middle end, and does not need to be: §3.4 declines
//! to tell LLVM about overflow at all, so there is nothing to fold in.
//!
//! Two further gaps in the landed fixpoint are compensated *here* rather than
//! reported as attributes we know to be lies:
//!
//!  * Only `Array` and `Template` raise `Allocating` (`rc.rs:811-813`). A
//!    struct or enum literal allocates and is left `Pure`. So a function whose
//!    body allocates would claim `memory(none)`.
//!  * An `ExprKind::Intrinsic` raises purity but never `aborts`
//!    (`rc.rs:800`), while a `CallFn` to an intrinsic *function* raises both.
//!
//! [`Observed`] is where the backend's own conservatism lands: a function this
//! backend emitted an allocation or a runtime call into is demoted before its
//! attributes are written, so the attribute describes the code that was
//! emitted rather than the code the middle end predicted. It is computed for
//! the whole program at once (`emit::observe`) and not per unit, because a
//! declaration in one unit and a definition in another must carry the same
//! bits or LLVM optimizes a call against a promise the definition does not
//! keep.
//!
//! # The reference count is memory, and LLVM was told it was not
//!
//! The third gap, and the one that was a miscompile rather than a missing
//! optimization. `incref` and `decref` store the count at `p - 16`
//! (MEMORY.md §5.1), and `p` is routinely a parameter. LangRef's
//! *Pointer Aliasing Rules* say a `getelementptr` result **is** *based on* its
//! pointer operand, and `argmem` is "accesses that are based on pointer
//! arguments to the function" — so that store is a write to argument memory,
//! full stop. Two attributes this file used to emit said it was not:
//!
//!  * `readonly` on the parameter — "the function does not write through this
//!    pointer argument. If a function writes to a readonly pointer argument,
//!    the behavior is undefined."
//!  * `memory(argmem: read)` on the function — "The location is only read.
//!    Writing to the location is immediate undefined behavior. **This includes
//!    the case where the location is read from and then the same value is
//!    written back.**"
//!
//! That last sentence is the one that decides it. The older comment in
//! [`decorate_param`] argued that the write is *unobservable* — the count is
//! not part of the value, and MEMORY.md §5.3's in-place append writes only
//! past the end of a block whose count it has just seen to be 1. Both
//! statements are true about Buri and neither is what LLVM's `read` means:
//! `read` is a promise about the *bytes*, and a store of the identical value
//! is still a store. `memory(...)`'s `write` kind has an observability
//! carve-out; `read` deliberately has none.
//!
//! Because a function's `memory(...)` describes its callees' effects too, the
//! same is true one level up: a caller of a function that increfs is a
//! function that increfs.
//!
//! So [`Observed`] carries three more bits — [`Observed::writes_args`],
//! [`Observed::reads_far`] and [`Observed::writes_far`] — and the rule this
//! file now implements is:
//!
//! | what the body does to a count | `readonly` on the parameter | `memory(...)` |
//! |---|---|---|
//! | nothing | yes | `argmem: read` |
//! | counts a value *based on* a parameter | **no** | `argmem: readwrite` |
//! | counts anything else | yes | the default location becomes `readwrite` |
//!
//! The third row keeps `readonly`, and that is not an oversight: a count
//! reached through a *load* — a `[T]`'s element — is a write through a pointer
//! that is not based on the parameter, so `readonly` on the parameter stays
//! true while the `memory(...)` claim does not. LangRef's "based on" relation
//! is `getelementptr`/`bitcast`/`inttoptr` closed transitively, and a load is
//! on none of those paths. `opt -passes=function-attrs` infers exactly this
//! split for both shapes, which is the check that the table is LLVM's and not
//! this file's opinion.
//!
//! `emit::argument_based` is the provenance analysis that decides which row a
//! count is in, and `emit::observe` propagates the answer over the call graph
//! beside the three bits that were already there — it has to, because a
//! function's `memory(...)` covers its callees.
//!
//! The same reading corrects two neighbours, both in [`MemoryEffects`]:
//! [`MemoryEffects::and_allocates`] (a function that initializes what it
//! allocated writes the *default* location, and `malloc` sets `errno`) and
//! [`narrow_for_params`] (a tagged enum's payload blob is an integer parameter
//! holding pointers, so it is argument memory).
//!
//! What survives is the whole of the purity theorem for functions that touch
//! no count: every all-scalar signature is still `memory(none)`, and every
//! counted signature whose reference-counting plan is empty — the common leaf
//! that reads a `Str` and returns part of it — still carries `readonly` and
//! `memory(argmem: read)`.
//!
//! # `willreturn` is a promise about *returning*, and purity is not one
//!
//! The fourth gap, and the second one that was a miscompile. CODEGEN-LLVM.md
//! §3.1's table has a row reading *"a function `middle` proved terminates |
//! `willreturn`, and `mustprogress`"*, and this file used to read that row as
//! `Purity::Pure` with no abort. **`middle` proves no such thing.** It has no
//! termination analysis at all: `rc::infer_effects`'s three columns are
//! purity, abortability and parkability, and none of them is about coming
//! back. Purity is a statement about *effects* — SPEC 10.4's theorem is
//! explicit that it holds of evaluations "that **terminate** without
//! aborting", so termination is the theorem's hypothesis rather than its
//! conclusion.
//!
//! What that cost: `fn f(i: Int): Int { if (i <= 0) { 0 } else { 1 + f(i - 1) } }`
//! is pure, cannot abort, and recurses. It got `memory(none)`, `willreturn`
//! and therefore `speculatable`, which is exactly the licence to **hoist the
//! recursive call above the branch that guards it** — and `default<O2>` took
//! it. The emitted body became an unconditional call to itself with the base
//! case folded in afterwards, so the program recursed until the machine stack
//! ran out. `default<O0>` printed the right answer, which is what a false
//! attribute looks like from outside.
//!
//! `mustprogress` is the same promise from the other side and is just as
//! wrong: it lets LLVM delete a loop it cannot prove terminates, and SPEC
//! 10.4 says the opposite — *"an implementation may drop a pure call only
//! where it can also show the call returns"*. Divergence is observable, so
//! neither attribute may be claimed without a proof.
//!
//! ## What is emitted instead, and how conservative it is
//!
//! [`Observed::may_diverge`] is that proof, in the only form this backend can
//! check on the IR it is holding: **no cycle**. A function with no loop in its
//! control-flow graph, which can reach no cycle in the call graph, executes a
//! bounded number of instructions and returns. `emit::observe` computes it for
//! the whole program at once, for the same reason the memory bits are computed
//! there — a declaration and its definition must carry the same attributes.
//!
//! This is **weaker than the design's row** and deliberately so: a `for` loop
//! over a list terminates, and a recursion down a `[T]` terminates, and
//! neither is proven here. Proving them wants a decreasing-measure analysis in
//! `middle`, which does not exist; the day it does, the bit it computes
//! belongs in `ir::Facts` beside `can_abort`, and this file reads it instead
//! of counting cycles. Until then a loop costs `willreturn`, and the trade is
//! not close: the attribute buys hoisting and dead-call elimination, and a
//! wrong one buys an infinite recursion.

use inkwell::attributes::{Attribute, AttributeLoc};
use inkwell::context::Context;
use inkwell::values::{CallSiteValue, FunctionValue};

use crate::compiler::middle::ir;

use super::repr::{Slot, SlotTy};
use crate::compiler::middle::layout::Scalar;

// ---------------------------------------------------------------------------
// The `memory(...)` encoding hazard — CODEGEN-LLVM.md §3.5
// ---------------------------------------------------------------------------

/// One location in LLVM's `MemoryEffects` bitmask.
///
/// **The location list has changed twice in versions this project could
/// plausibly build against**, and the bitmask is two bits per location, so a
/// literal is a silent miscompilation of the attribute on an LLVM bump — the
/// worst kind, because the IR still verifies. The numbers below are read from
/// `llvm/Support/ModRef.h` of the pinned LLVM (21.1.8):
///
/// ```text
/// enum class IRMemLocation {
///   ArgMem = 0, InaccessibleMem = 1, ErrnoMem = 2, Other = 3,
/// };
/// ```
///
/// | | locations | `memory(none)` | `memory(readwrite)` |
/// |---|---|---|---|
/// | LLVM 18-20 | ArgMem, InaccessibleMem, Other | 0 | 0x3F |
/// | **LLVM 21** | + ErrnoMem *before* Other | 0 | 0xFF |
/// | LLVM 22 | + TargetMem0, TargetMem1 | 0 | 0xFFF |
///
/// `memory(none)` is 0 in every version and the argmem-only forms are stable,
/// because `ArgMem` has been location 0 throughout; everything naming the
/// *default* location is not. So [`MemoryEffects::everything`] is the one
/// function that has to move on a version bump, and nothing anywhere else in
/// this backend writes a literal.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Location {
    ArgMem = 0,
    InaccessibleMem = 1,
    /// Present from LLVM 21. Named so that the count below is a list rather
    /// than a number somebody has to re-derive.
    ErrnoMem = 2,
    Other = 3,
}

/// `NoModRef`, `Ref`, `Mod`, `ModRef` — LLVM's `ModRefInfo`, unchanged since
/// the attribute was introduced.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum ModRef {
    None = 0,
    Ref = 1,
    Mod = 2,
    RefMod = 3,
}

/// A `memory(...)` bitmask under construction.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
struct MemoryEffects(u64);

impl MemoryEffects {
    /// Every location, at `ModRef` — the default a function with no `memory`
    /// attribute already has, spelled explicitly for the one caller that wants
    /// to say so.
    fn everything() -> MemoryEffects {
        MemoryEffects::default()
            .with(Location::ArgMem, ModRef::RefMod)
            .with(Location::InaccessibleMem, ModRef::RefMod)
            .with(Location::ErrnoMem, ModRef::RefMod)
            .with(Location::Other, ModRef::RefMod)
    }

    /// `memory(none)`: reads nothing, writes nothing. Zero in every LLVM
    /// version, which is why it is the one form that can be trusted across a
    /// bump.
    fn none() -> MemoryEffects {
        MemoryEffects(0)
    }

    /// `memory(argmem: read)` — reads the heap through its parameters and
    /// nothing else. Every `[T]`, `Str` and struct pointer.
    fn arg_read() -> MemoryEffects {
        MemoryEffects::default().with(Location::ArgMem, ModRef::Ref)
    }

    /// `memory(argmem: readwrite)` — the same, plus the reference-count store
    /// at `p - 16` for a `p` the parameter list supplied. See the module
    /// header: the count is argument memory, and `read` has no carve-out for a
    /// write nothing can observe.
    fn arg_readwrite() -> MemoryEffects {
        MemoryEffects::default().with(Location::ArgMem, ModRef::RefMod)
    }

    /// Adds `inaccessiblemem: write`: the function may write somewhere the
    /// caller cannot observe except by not returning. That is what an abort
    /// is (CODEGEN-LLVM.md §3.1).
    fn and_may_abort(self) -> MemoryEffects {
        self.with(Location::InaccessibleMem, ModRef::Mod)
    }

    /// The allocator's own effects, which are three locations and not one.
    ///
    /// CODEGEN-LLVM.md §3.1's `Alloc`-bounded row said `inaccessiblemem:
    /// readwrite` and stopped there. Two things are missing from that, and both
    /// are checkable against LLVM's own inference — `opt -passes=function-attrs`
    /// on `%p = call noalias ptr @alloc(...)` / `store ..., ptr %p` answers
    /// `memory(write, argmem: none, inaccessiblemem: readwrite)`:
    ///
    ///  * **The default location, at `Mod`.** Every allocation this backend
    ///    emits is followed by the stores that initialize the block —
    ///    `make_array`'s elements, `make_closure`'s environment,
    ///    `concat`'s two `memcpy`s. LangRef's `inaccessiblemem` covers memory
    ///    "not accessible by the current module", and the parenthetical that
    ///    excuses an allocator handing back newly accessible memory excuses the
    ///    *allocator*, not its caller. Once `buri_rt_alloc` has returned, the
    ///    block is ordinary program memory and it is not argument memory, so it
    ///    is the default location. LLVM does not apply its function-local
    ///    carve-out either, because the block escapes through the return.
    ///  * **`errnomem`, at `ModRef`.** The allocator is `malloc` underneath and
    ///    `malloc` sets `errno`.
    fn and_allocates(self) -> MemoryEffects {
        self.with(Location::InaccessibleMem, ModRef::RefMod)
            .with(Location::ErrnoMem, ModRef::RefMod)
            .with(Location::Other, ModRef::Mod)
    }

    /// Adds the default location at these bits: a reference count adjusted
    /// through a pointer that is not based on a parameter, or an element read
    /// out of a block this function allocated. See [`Observed::writes_far`].
    fn and_far(self, read: bool, write: bool) -> MemoryEffects {
        let mut out = self;
        if read {
            out = out.with(Location::Other, ModRef::Ref);
        }
        if write {
            out = out.with(Location::Other, ModRef::Mod);
        }
        out
    }

    fn with(self, loc: Location, mr: ModRef) -> MemoryEffects {
        let shift = (loc as u32).saturating_mul(2);
        let mask = (mr as u64) << shift;
        MemoryEffects(self.0 | mask)
    }

    /// Sets one location back to `NoModRef`, keeping the rest.
    fn without(self, loc: Location) -> MemoryEffects {
        let shift = (loc as u32).saturating_mul(2);
        let mask = (ModRef::RefMod as u64) << shift;
        let keep = (ModRef::None as u64) << shift;
        MemoryEffects((self.0 & !mask) | keep)
    }

    pub fn bits(self) -> u64 {
        self.0
    }

    fn is_everything(self) -> bool {
        self == MemoryEffects::everything()
    }
}

// ---------------------------------------------------------------------------
// The calling conventions
// ---------------------------------------------------------------------------

/// `fastcc`, for Buri-to-Buri calls (CODEGEN-LLVM.md §5).
///
/// Costs nothing — both sides of every Buri call are generated here, there is
/// no ABI to be compatible with, and the aggregates were already flattened
/// (VALUE-MODEL.md §5.1) so no convention has to classify one.
///
/// inkwell has no `CallConv` enum; the convention is a raw `u32` set on both
/// the function and every call site, and a mismatch between the two is a
/// miscompile LLVM will not diagnose. [`set_convention`] and
/// [`set_call_convention`] are the only two places either number appears.
pub const FAST: u32 = 8;

/// `ccc`, the platform C ABI, for the `buri_rt_*` runtime entries. This is the
/// single place in a Buri artifact where a platform ABI appears
/// (`cli/runtime/lib.rs` §2).
pub const C: u32 = 0;

pub fn set_convention(f: FunctionValue<'_>, conv: u32) {
    f.set_call_conventions(conv);
}

pub fn set_call_convention(call: CallSiteValue<'_>, conv: u32) {
    call.set_call_convention(conv);
}

// ---------------------------------------------------------------------------
// Emission
// ---------------------------------------------------------------------------

/// What this backend decided a function's effects are, which is the middle
/// end's answer narrowed by what was actually emitted.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Observed {
    /// The backend emitted a call into `cli/runtime` or an open-coded
    /// allocation. `rc.rs` only raises `Allocating` for `Array` and `Template`
    /// (`rc.rs:811-813`), so a struct literal that allocates would otherwise
    /// keep `memory(none)`.
    pub allocates: bool,
    /// The backend emitted an abort, a division that can abort, or a call to
    /// something that can. This is `Facts::can_abort` ORed with what emission
    /// found, because `rc.rs:800` raises purity for an inline intrinsic and
    /// never raises `aborts`.
    pub aborts: bool,
    /// The backend emitted a call whose callee's effects it does not know —
    /// an indirect call, or a runtime entry with a capability behind it.
    pub opaque: bool,
    /// The backend emitted a store to memory **based on** a pointer parameter,
    /// in LangRef's *Pointer Aliasing Rules* sense: an `incref`/`decref` of a
    /// value that is one of this function's parameters — the count lives at a
    /// `getelementptr` from it — or MEMORY.md §5.3's in-place append into a
    /// block a parameter points into.
    ///
    /// Takes `readonly` off every pointer parameter and raises `argmem` from
    /// `Ref` to `ModRef`. See the module header for why the write being
    /// unobservable to a Buri caller does not make the attribute true.
    pub writes_args: bool,
    /// The backend emitted a *read* of memory that is neither argument memory
    /// nor the allocator's: the count of a value this function did not receive
    /// as a parameter, or an element of a block it allocated itself.
    pub reads_far: bool,
    /// The same, for a store. A reference-count write through a pointer that
    /// is *not* based on a parameter — a count reached through a load, which
    /// LangRef's "based on" relation does not follow, or a block a callee
    /// returned. Such a store is neither `argmem` nor `inaccessiblemem`, so
    /// only the *default* location describes it.
    pub writes_far: bool,
    /// This backend has **not proven that the function returns**: the emitted
    /// body has a cycle in its control-flow graph, or the call graph has one
    /// this function can reach — itself included.
    ///
    /// The bit `willreturn`, `mustprogress` and `speculatable` are gated on.
    /// See the module header: purity says a function performs no observable
    /// effect and says nothing whatever about whether it comes back, and a
    /// `willreturn` claimed of a recursion is a licence to speculate the
    /// recursive call.
    ///
    /// `emit::observe` is where it comes from, and it is the one bit there
    /// whose seeds cannot be a local scan: a least fixpoint that starts every
    /// function optimistic converges on "proven" for a function whose only
    /// unproven callee is itself. `emit::reaches_a_cycle` seeds it.
    pub may_diverge: bool,
}

impl Observed {
    pub fn clean() -> Observed {
        Observed {
            allocates: false,
            aborts: false,
            opaque: false,
            writes_args: false,
            reads_far: false,
            writes_far: false,
            may_diverge: false,
        }
    }

    /// Everything, which is what a callee this compilation cannot see is
    /// assumed to do — the direction that costs performance and cannot be
    /// wrong.
    ///
    /// `writes_args` is part of "everything" and is the reason it has to be:
    /// `buri_rt_list_push` writes past the end of the block it is handed when
    /// it observes a count of 1 (`cli/runtime/list.rs`'s `append_dest`), and
    /// the block is routinely the caller's parameter.
    pub fn opaque() -> Observed {
        Observed {
            allocates: true,
            aborts: true,
            opaque: true,
            writes_args: true,
            reads_far: true,
            writes_far: true,
            may_diverge: true,
        }
    }

    /// Widens `self` by everything `other` found — the one direction this
    /// lattice moves.
    pub fn join(&mut self, other: Observed) {
        self.allocates |= other.allocates;
        self.aborts |= other.aborts;
        self.opaque |= other.opaque;
        self.writes_args |= other.writes_args;
        self.reads_far |= other.reads_far;
        self.writes_far |= other.writes_far;
        self.may_diverge |= other.may_diverge;
    }
}

/// The `memory(...)` bits for one function.
///
/// The whole of CODEGEN-LLVM.md §3.1's table, in one expression, with the
/// module header's correction applied: `Purity::Pure` is only `memory(none)`
/// when nothing can abort.
fn memory_effects(facts: &ir::Facts, observed: Observed) -> MemoryEffects {
    if observed.opaque {
        return MemoryEffects::everything();
    }
    let aborts = facts.can_abort || observed.aborts;
    let allocates = observed.allocates || matches!(facts.purity, ir::Purity::Allocating);
    let base = match facts.purity {
        ir::Purity::Effectful => return MemoryEffects::everything(),
        // "Reads no heap through a parameter" is not a fact the middle end
        // computes, so the honest floor for a pure function that takes a
        // pointer is `argmem: read`; a function with no pointer parameter at
        // all is narrowed to `none` by `narrow_for_params`. A function that
        // adjusts a count in a parameter's block writes that memory, so its
        // floor is `argmem: readwrite`.
        ir::Purity::Pure | ir::Purity::Allocating => {
            if observed.writes_args {
                MemoryEffects::arg_readwrite()
            } else {
                MemoryEffects::arg_read()
            }
        }
    };
    let base = if allocates { base.and_allocates() } else { base };
    // The default location, for a count adjusted through a pointer that is not
    // based on a parameter and for an element read out of a block this function
    // allocated. Naming it at `ModRef` *is* `memory(readwrite)`, and
    // [`memory`] declines to write the default out — so this is the one row
    // that gives the attribute up entirely, which is what it costs to be true.
    let base = base.and_far(observed.reads_far, observed.writes_far);
    if aborts {
        base.and_may_abort()
    } else {
        base
    }
}

/// Narrows `argmem` away where there is no argument memory to read: a function
/// whose parameters are all scalars cannot read the heap through one.
///
/// A [`SlotTy::Blob`] counts as argument memory even though it is an integer.
/// It is a tagged enum's payload area (`repr.rs`), a `Result<Str, E>` keeps its
/// `Str`'s three words inside one, and the `inttoptr` that gets them back out
/// is *based on* the parameter by LangRef's Pointer Aliasing Rules — "a pointer
/// value formed by an `inttoptr` is based on all pointer values that contribute
/// to the computation of the pointer's value". So a blob parameter is a pointer
/// parameter wearing an integer's type, and the only signature this narrows to
/// `memory(none)` is one that is genuinely all-scalar.
fn narrow_for_params(effects: MemoryEffects, params: &[Slot]) -> MemoryEffects {
    let reaches_memory =
        params.iter().any(|s| s.ty.is_pointer() || matches!(s.ty, SlotTy::Blob(_)));
    if effects.is_everything() || reaches_memory {
        effects
    } else {
        // Clear the `ArgMem` field and keep the rest — through the same
        // location table, so §3.5's hazard has no second spelling here either.
        effects.without(Location::ArgMem)
    }
}

/// Applies the whole discipline to one function.
///
/// `params` is the flattened parameter list — one entry per LLVM parameter,
/// which is what makes `readonly`, `nonnull` and `align` per-*slot* facts
/// rather than per-Buri-parameter ones.
pub fn decorate(
    ctx: &Context,
    f: FunctionValue<'_>,
    facts: &ir::Facts,
    observed: Observed,
    params: &[Slot],
    ret: &[Slot],
    heap_align: u32,
) {
    // `nounwind` on every function, on every backend. The single most valuable
    // attribute here and it costs no analysis: the language has no `throw`, no
    // unwinding `panic` and no `catch` (SPEC 6.9). LLVM without it has to
    // assume every call is a potential unwind edge.
    enum_attr(ctx, f, "nounwind", 0);

    let effects = narrow_for_params(memory_effects(facts, observed), params);
    memory(ctx, f, effects);

    // `willreturn` and `mustprogress` follow one fact — that the function
    // **returns** — and the module header is the argument for why purity is
    // not that fact. Three of the four conjuncts answer a different way of not
    // returning: an abort leaves through `_exit`, an opaque callee may do
    // anything at all, and a cycle may run forever. `Purity::Pure` is the
    // fourth and is on top of them because `speculatable` needs it and because
    // an effectful function has nothing to gain from either attribute.
    let terminates = matches!(facts.purity, ir::Purity::Pure)
        && !facts.can_abort
        && !observed.aborts
        && !observed.opaque
        && !observed.may_diverge;
    if terminates {
        enum_attr(ctx, f, "willreturn", 0);
        enum_attr(ctx, f, "mustprogress", 0);
        if effects == MemoryEffects::none() {
            // `memory(none)` + `willreturn` + `nounwind`: a call may be
            // hoisted above a branch (CODEGEN-LLVM.md §3.1).
            enum_attr(ctx, f, "speculatable", 0);
        }
    }

    // `nofree` is deliberately *not* set: `decref` frees.

    for (i, slot) in params.iter().enumerate() {
        decorate_param(ctx, f, AttributeLoc::Param(i as u32), *slot, heap_align, observed);
    }
    // A pointer return is a freshly allocated block on every path that
    // produces one — every allocating runtime entry returns a block nothing
    // else has a reference to (CODEGEN-LLVM.md §3.3, the first and most
    // valuable case) — so `noalias` on the return is unconditional, and it is
    // what lets LLVM keep a just-built aggregate's fields in registers across
    // a call.
    if let [one] = ret {
        if one.ty.is_pointer() {
            enum_attr_at(ctx, f, AttributeLoc::Return, "noalias", 0);
            enum_attr_at(ctx, f, AttributeLoc::Return, "align", u64::from(heap_align));
        }
    }

    // `noalias` on a *parameter*, case 2 of §3.3: a function with exactly one
    // pointer parameter cannot alias any other, which is vacuous and free.
    // Case 1 is the return, above. Case 3 — a parameter `middle::rc` proved
    // uniquely owned — has no representation in the landed `ir::Facts`
    // (`rc.rs`'s `FuncPlan::reuse` never reaches `lower`), so it is not
    // emitted. Emitting `noalias` where aliasing is possible is a miscompile
    // that shows up as a wrong answer months later.
    let pointers: Vec<usize> =
        params.iter().enumerate().filter(|(_, s)| s.ty.is_pointer()).map(|(i, _)| i).collect();
    if let [only] = pointers.as_slice() {
        enum_attr_at(ctx, f, AttributeLoc::Param(*only as u32), "noalias", 0);
    }
}

/// The value model's half of the table (CODEGEN-LLVM.md §3.2).
fn decorate_param(
    ctx: &Context,
    f: FunctionValue<'_>,
    loc: AttributeLoc,
    slot: Slot,
    heap_align: u32,
    observed: Observed,
) {
    match slot.ty {
        SlotTy::Scalar(Scalar::Ptr) => {
            // `readonly` on a pointer parameter is true of the *value*
            // unconditionally — values are immutable and there is no interior
            // mutability — and it is **not** what `readonly` means. LangRef:
            // "the function does not write through this pointer argument", and
            // a `getelementptr` to `p - 16` is a pointer through `p`. So the
            // three writes this language does perform through a parameter all
            // count:
            //
            //  * `incref`'s store of the count at `p - 16` (MEMORY.md §5.1),
            //  * `decref`'s store of the count, and its `free` of the block,
            //  * MEMORY.md §5.3's in-place growth — `emit::concat`'s `memmove`
            //    past the end of the left operand's block, and
            //    `cli/runtime/list.rs`'s `append_dest`.
            //
            // All three are unobservable to a *Buri* caller, which is the
            // argument this comment used to make and which is beside the
            // point: `readonly` is a promise about bytes, and none of the
            // three is a promise about bytes. `Observed::writes_args` is the
            // condition, computed over the whole call graph by
            // `emit::observe`, and the attribute is emitted exactly where it
            // is true — which is still every function whose reference-counting
            // plan is empty, the common case for a leaf that reads a `Str`.
            if !observed.writes_args {
                enum_attr_at(ctx, f, loc, "readonly", 0);
            }
            // Every heap pointer is 16-byte aligned, because the header is
            // 16 bytes and sits immediately before the payload (VALUE-MODEL.md
            // §2). `nonnull` is *not* set: a `Str`'s `base`, a closure's `env`
            // and a niche-encoded `Option` are all legitimately null, and the
            // slot table does not carry which is which across a signature
            // boundary. Setting it where null is a value is a miscompile;
            // omitting it costs a null test LLVM would have removed.
            enum_attr_at(ctx, f, loc, "align", u64::from(heap_align));
        }
        // A `Bool` is one byte with values 0 and 1 only, and a `Char` is a
        // scalar value. `range` is a type attribute in LLVM's C API and
        // inkwell 0.10 exposes only enum and type attributes by kind id, with
        // no constructor for a range's two-constant payload — so these two
        // rows of §3.2's table are the ones this backend does not emit, and
        // saying so here is cheaper than leaving a reader to find out.
        SlotTy::Scalar(_) | SlotTy::Blob(_) => {}
    }
}

/// `cold` on a call, for CODEGEN-LLVM.md §6: every abort path, every free
/// path, every `.None` arm of a `?`. This is the highest-value item on that
/// list, because reference counting puts a rarely-taken branch next to *every*
/// value that dies.
pub fn cold_call(ctx: &Context, call: CallSiteValue<'_>) {
    call_attr(ctx, call, AttributeLoc::Function, "cold", 0);
    call_attr(ctx, call, AttributeLoc::Function, "nounwind", 0);
}

/// `noreturn` + `cold` on a call that does not come back: an abort, or
/// `buri_rt_host_proc_exit_with`.
pub fn noreturn_call(ctx: &Context, call: CallSiteValue<'_>) {
    cold_call(ctx, call);
    call_attr(ctx, call, AttributeLoc::Function, "noreturn", 0);
}

pub fn mark_noreturn(ctx: &Context, f: FunctionValue<'_>) {
    enum_attr(ctx, f, "noreturn", 0);
    enum_attr(ctx, f, "cold", 0);
}

/// The `memory(...)` attribute itself. One call site in the whole backend, per
/// §3.5.
fn memory(ctx: &Context, f: FunctionValue<'_>, effects: MemoryEffects) {
    if effects.is_everything() {
        // The default. Writing it out would be noise in every dump.
        return;
    }
    enum_attr(ctx, f, "memory", effects.bits());
}

fn enum_attr(ctx: &Context, f: FunctionValue<'_>, name: &str, value: u64) {
    enum_attr_at(ctx, f, AttributeLoc::Function, name, value);
}

fn enum_attr_at(ctx: &Context, f: FunctionValue<'_>, loc: AttributeLoc, name: &str, value: u64) {
    let kind = Attribute::get_named_enum_kind_id(name);
    // A zero kind id means this LLVM does not know the attribute. Silently
    // skipping is right: an attribute is an optimization hint, and a toolchain
    // that refused to build because a hint was unavailable would be trading a
    // working artifact for a faster one.
    if kind == 0 {
        return;
    }
    f.add_attribute(loc, ctx.create_enum_attribute(kind, value));
}

fn call_attr(ctx: &Context, call: CallSiteValue<'_>, loc: AttributeLoc, name: &str, value: u64) {
    let kind = Attribute::get_named_enum_kind_id(name);
    if kind == 0 {
        return;
    }
    call.add_attribute(loc, ctx.create_enum_attribute(kind, value));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts(purity: ir::Purity, can_abort: bool) -> ir::Facts {
        // `can_park` is not an attribute question: no native backend reads
        // it yet.
        ir::Facts { params: Vec::new(), purity, can_abort, can_park: false }
    }

    /// The bits, against `llvm/Support/ModRef.h` of the pinned LLVM. These are
    /// the numbers §3.5 says must never be written as literals at a call site,
    /// so they are written as literals *here*, once, where a version bump is
    /// supposed to break them.
    #[test]
    fn the_bitmask_matches_llvm_21s_location_list() {
        assert_eq!(MemoryEffects::none().bits(), 0);
        assert_eq!(MemoryEffects::arg_read().bits(), 0b01);
        assert_eq!(MemoryEffects::arg_read().and_may_abort().bits(), 0b1001);
        // `argmem: read` (0b01), `inaccessiblemem: readwrite` (0b11 << 2),
        // `errnomem: readwrite` (0b11 << 4) and the default location at `Mod`
        // (0b10 << 6) — the three the allocator touches beside its argument.
        assert_eq!(MemoryEffects::arg_read().and_allocates().bits(), 0b1011_1101);
        // Four locations at two bits each, all `ModRef`.
        assert_eq!(MemoryEffects::everything().bits(), 0xFF);
    }

    /// The correction this file exists for: `Purity::Pure` with `can_abort`
    /// must not become `memory(none)`.
    #[test]
    fn a_pure_function_that_can_abort_is_not_memory_none() {
        let pure_clean = memory_effects(&facts(ir::Purity::Pure, false), Observed::clean());
        assert_eq!(pure_clean, MemoryEffects::arg_read());

        let pure_aborting = memory_effects(&facts(ir::Purity::Pure, true), Observed::clean());
        assert_ne!(pure_aborting, MemoryEffects::none());
        assert_eq!(pure_aborting, MemoryEffects::arg_read().and_may_abort());
    }

    /// A pure function with no pointer parameter has no argument memory to
    /// read, which is the only route to `memory(none)`.
    #[test]
    fn a_pure_scalar_function_is_memory_none() {
        let e = memory_effects(&facts(ir::Purity::Pure, false), Observed::clean());
        let scalar = Slot { offset: 0, ty: SlotTy::Scalar(Scalar::I64) };
        assert_eq!(narrow_for_params(e, &[scalar, scalar]), MemoryEffects::none());

        let pointer = Slot { offset: 0, ty: SlotTy::Scalar(Scalar::Ptr) };
        assert_eq!(narrow_for_params(e, &[scalar, pointer]), MemoryEffects::arg_read());
    }

    /// An effectful function keeps the default, and the default is never
    /// written out.
    #[test]
    fn an_effectful_function_gets_no_memory_attribute() {
        let e = memory_effects(&facts(ir::Purity::Effectful, true), Observed::clean());
        assert!(e.is_everything());
        let scalar = Slot { offset: 0, ty: SlotTy::Scalar(Scalar::I64) };
        assert!(narrow_for_params(e, &[scalar]).is_everything());
    }

    /// The module header's rule, as three assertions: a count in a
    /// parameter's block is a *write* to argument memory, and the attribute
    /// has to say so.
    #[test]
    fn counting_a_parameter_makes_argmem_readwrite() {
        let clean = memory_effects(&facts(ir::Purity::Pure, false), Observed::clean());
        assert_eq!(clean, MemoryEffects::arg_read());

        let counting =
            Observed { writes_args: true, ..Observed::clean() };
        let e = memory_effects(&facts(ir::Purity::Pure, false), counting);
        assert_eq!(e, MemoryEffects::arg_readwrite());
        assert_ne!(e, MemoryEffects::arg_read());
        // Two bits, not one: `argmem: read` is `0b01` and the write is the
        // second, so a bump that reordered the locations breaks this too.
        assert_eq!(e.bits(), 0b11);
    }

    /// A count reached through a load is in neither `argmem` nor
    /// `inaccessiblemem`, and the only `memory(...)` that covers the default
    /// location is the one that is never written out.
    #[test]
    fn counting_anything_else_gives_up_the_memory_attribute() {
        let far = Observed { reads_far: true, writes_far: true, ..Observed::clean() };
        let e = memory_effects(&facts(ir::Purity::Pure, false), far);
        // The default location at `ModRef` is what `memory(readwrite)` means,
        // so nothing narrower than the default survives for the heap. What is
        // still true and still said is that the allocator and `errno` are
        // untouched, which is why this is not literally `everything`.
        assert_ne!(e, MemoryEffects::arg_read());
        assert_ne!(e, MemoryEffects::arg_readwrite());
        assert_eq!(e.bits() & 0b1100_0000, 0b1100_0000);
    }

    /// A tagged enum's payload blob is an integer parameter that holds
    /// pointers, so it is argument memory and must not narrow to
    /// `memory(none)`.
    #[test]
    fn a_payload_blob_parameter_is_still_argument_memory() {
        let e = memory_effects(&facts(ir::Purity::Pure, false), Observed::clean());
        let blob = Slot { offset: 0, ty: SlotTy::Blob(24) };
        let tag = Slot { offset: 0, ty: SlotTy::Scalar(Scalar::I8) };
        assert_eq!(narrow_for_params(e, &[tag, blob]), MemoryEffects::arg_read());
        assert_eq!(narrow_for_params(e, &[tag]), MemoryEffects::none());
    }

    /// The lattice only ever widens, and a new bit has to be joined like the
    /// old ones or the call-graph fixpoint silently loses it.
    #[test]
    fn join_widens_every_bit() {
        let mut o = Observed::clean();
        o.join(Observed::opaque());
        assert_eq!(o, Observed::opaque());
        let mut o = Observed::opaque();
        o.join(Observed::clean());
        assert_eq!(o, Observed::opaque());
    }


    /// **The gate the miscompile was behind.** Same facts, same purity, same
    /// signature; the only difference is whether this backend proved the
    /// function returns, and it decides all three attributes.
    ///
    /// Read off the `FunctionValue` rather than out of the printed module: an
    /// attribute group is printed once at the end of a module and shared by
    /// every function that has the same set, so a `contains` over the text
    /// would answer about some other function.
    #[test]
    fn willreturn_wants_a_termination_proof_and_purity_is_not_one() {
        let ctx = Context::create();
        let module = ctx.create_module("attrs");
        let i64t = ctx.i64_type();
        let slot = Slot { offset: 0, ty: SlotTy::Scalar(Scalar::I64) };
        let decorated = |name: &str, observed: Observed| {
            let f = module.add_function(name, i64t.fn_type(&[i64t.into()], false), None);
            decorate(&ctx, f, &facts(ir::Purity::Pure, false), observed, &[slot], &[slot], 16);
            f
        };
        let has = |f: FunctionValue<'_>, name: &str| {
            f.get_enum_attribute(AttributeLoc::Function, Attribute::get_named_enum_kind_id(name))
                .is_some()
        };

        let proven = decorated("proven", Observed::clean());
        assert!(has(proven, "willreturn"));
        assert!(has(proven, "mustprogress"));
        assert!(has(proven, "speculatable"));

        let cyclic = decorated("cyclic", Observed { may_diverge: true, ..Observed::clean() });
        assert!(!has(cyclic, "willreturn"), "a function that may not return claims `willreturn`");
        assert!(!has(cyclic, "mustprogress"), "`mustprogress` lets LLVM delete a diverging loop");
        assert!(!has(cyclic, "speculatable"), "`speculatable` is what hoists the call");

        // The rest of the discipline is untouched, which is what keeps this a
        // correction rather than a retreat: `memory(none)` is a statement about
        // effects, and calling yourself is not an effect.
        for f in [proven, cyclic] {
            let memory = f
                .get_enum_attribute(
                    AttributeLoc::Function,
                    Attribute::get_named_enum_kind_id("memory"),
                )
                .expect("every function this file decorates carries `memory(...)`");
            assert_eq!(memory.get_enum_value(), MemoryEffects::none().bits());
            assert!(has(f, "nounwind"));
        }
    }

    /// What the backend observed narrows the middle end's answer, never
    /// widens it.
    #[test]
    fn an_allocation_the_middle_end_missed_demotes_the_attribute() {
        let observed = Observed { allocates: true, ..Observed::clean() };
        let e = memory_effects(&facts(ir::Purity::Pure, false), observed);
        assert_eq!(e, MemoryEffects::arg_read().and_allocates());
        assert_ne!(e, MemoryEffects::none());
    }
}
