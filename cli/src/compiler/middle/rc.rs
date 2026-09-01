//! Reference counting: own/borrow inference, elision, and in-place reuse.
//!
//! Non-atomic reference counting with static elision, over a size-class
//! allocator. Non-atomic because the language has no threads, which is a
//! language decision this pass gets to spend rather than one it has to defend.
//!
//! # The count is non-atomic *per block*, not per program
//!
//! That sentence is still true of every program this toolchain compiles, and
//! it is no longer true by construction. Since G2 an `incref` and a `decref`
//! are each **two** counts behind one branch: bit 63 of the block's `cap`
//! (`layout::CAP_SHARED_FLAG`) says the block may be reached from more than
//! one thread, and it chooses the atomic form. Both backends open-code the
//! fork — `backend/llvm/emit.rs::fork_on_shared`, and the three reference
//! stencils in `backend/stencil/sources.rs::memory` — and `cli/runtime`'s own
//! `buri_rt_incref`/`buri_rt_decref` take it too, so a block reached from a
//! generic path is counted the same way as one reached from emitted code.
//!
//! **G3 sets the bit, and this pass is what decides.** [`crosses_tasks`] asks
//! one question of the whole program — *can any value of it come to be
//! reachable from a second carrier* — and [`Plan::crosses_tasks`] carries the
//! answer to `ir::Program`, to both native backends, and into `main` as a call
//! to `buri_rt_values_may_cross_tasks`. A program that says so marks **every
//! block it allocates**; a program that does not marks none and is bit for bit
//! the program it was before track G.
//!
//! ## Why the whole program, and not [`sharing`]
//!
//! [`sharing`] computes *where a second reference to a value comes into
//! existence*, which reads like the same question and is not. It is a question
//! about **sites** — where an `incref` goes — and the mark is a question about
//! **blocks**, and specifically about a transitive closure of them: a `[Str]`
//! handed to a step is a block whose *elements* the step counts, and a `Str`
//! inside a closure's environment is a block two carriers count. So a mark
//! derived from a call site has to be a deep, type-directed walk of everything
//! reachable from the arguments — `Helper::Walk`'s shape, which G5's
//! `Helper::Copy` has since generalised — and a *shallow* one is precisely the
//! under-count
//! MEMORY.md §5.5 forbids:
//!
//! > **An over-set bit costs one copy.** … **An under-counted reference is a
//! > silent aliasing bug** — two names for a list, one of them written
//! > through, and a wrong answer with no crash to find it by.
//!
//! The whole-program answer is the over-set end of that asymmetry, and it is
//! sound by construction rather than by audit: a value reaches a carrier by a
//! route this compiler cannot see — the runtime's own blocks, a `Str` built
//! inside `host.rs`, whatever an FFI hands in one day — and it is marked
//! anyway, because the *allocator* is what marks. What it costs is atomic
//! reference counting throughout a program that uses `core/tasks`, which is
//! the price MEMORY.md §5.4 puts on threads rather than a price this shape
//! adds. **Narrowing it is now possible and has not been done**: G5's
//! `Helper::Copy` is the deep type-directed walk a per-value mark needs, and
//! spending it that way is an optimisation over an answer that is already
//! correct rather than a fix for one that is wrong.
//!
//! Two properties of the count survive the fork, and both were the reason the
//! bit is in `cap` rather than in the count itself (MEMORY.md §5.1,
//! `layout::CAP_SHARED_FLAG`'s own doc):
//!
//!  * **`IMMORTAL` saturation.** The atomic arms add and subtract a *delta* of
//!    `0` for an `IMMORTAL` block and `1` otherwise, which is the branchless
//!    `select` of the unshared arm written as an `atomicrmw` operand. A plain
//!    `fetch_add(1)` would wrap `u64::MAX` to zero and free every literal.
//!  * **The `rc == 1` uniqueness test.** Still unforked, and since G3 it has a
//!    second half: `buri_rt_unique_cap` answers `None` for a **marked** block,
//!    whatever its count. G2's argument for the bare count — *a thread holding
//!    no reference cannot make a second one* — has a premise the run baton was
//!    keeping true, that the caller holds the reference it is testing, and a
//!    borrowed parameter does not. Two carriers reading `1` off one borrowed
//!    list would each take the licence. Failing the test on the mark is the
//!    over-set direction again: it costs a copy, and it is why the elision and
//!    reuse this pass plans stay sound with the baton gone.
//!
//! Three things, in order of how much they matter:
//!
//!  * **Inference.** Each parameter is `own` or `borrow`. A borrowed parameter
//!    needs no `incref` at the call and no `decref` at the end of the callee.
//!  * **Elision.** A count that goes up and back down within one basic block,
//!    with nothing between that can observe it, is removed entirely.
//!  * **Reuse.** A uniquely-owned allocation whose last use is the construction
//!    of a same-size-class value is written through instead of freed and
//!    re-allocated. This is what makes a functional update of a list element
//!    not copy the list.
//!
//! **Native branch only** (`middle::native`): JavaScript is garbage collected
//! and has no use for any of it.
//!
//! Design: `design/native/MEMORY.md` §5.2–5.3.
//!
//! # Where the operations go, and why this pass does not emit them
//!
//! `incref` and `decref` are **IR instructions**, not tree nodes:
//! [`ir::Inst::IncRef`] and [`ir::Inst::DecRef`] are in `middle::ir`'s
//! instruction set, marked as placed from this pass's plan, and both backends
//! open-code them (MEMORY.md §5.1). The layer-A tree has nowhere to put one —
//! there is no statement form for "run this for effect on a value that has
//! already been computed" that is not also a value — so materialising them
//! here would mean inventing a second representation of something the IR
//! already spells.
//!
//! But the *analysis* has to happen on the tree. Ownership is a fixpoint over
//! the call graph, and the call graph is exact only in layer A
//! (`monomorphize.rs`, no dynamic dispatch); last use is a property of
//! evaluation order, which the tree states and the CFG has already flattened.
//!
//! So this pass computes and returns a [`Plan`], and `middle::lower` places
//! the instructions from it. That is the contract, in full:
//!
//! * **[`Plan::funcs`] is indexed by `FuncIdx`**, one [`FuncPlan`] per entry in
//!   `Program::funcs`.
//! * **[`FuncPlan::facts`] fills [`ir::Facts`]** — the ownership column, the
//!   purity fixpoint, `can_abort` and `can_park` — which `lower` copies onto
//!   `ir::Func` rather than recomputing. `Facts` documents these as
//!   conservative out of `lower` alone; this pass is what makes them exact.
//! * **[`FuncPlan::sites`] is keyed by [`NodeId`]**, and a `NodeId` is *the
//!   index of an expression in `typed::walk`'s pre-order over the function
//!   body*, counting from zero at the body itself. [`preorder`] is that
//!   numbering, exported so that `lower` numbers nodes with this module's
//!   function rather than with a second copy of the rule.
//! * A [`Site`] says: at node `n`, [`Position::Before`] (before the node's own
//!   code) or [`Position::After`] (after the node's value exists), apply
//!   [`RcOp`] to the SSA value currently holding [`Site::local`]. Sites at one
//!   `(node, position)` fire **in list order**, which matters where an arm
//!   entry increfs three bindings and then decrefs the value they came out of.
//! * `DecRef`'s `drop` field — the per-type drop glue — is `lower`'s to fill
//!   from [`super::layout`], because the glue is generated per *layout* and
//!   this pass has no layout table (`middle::native` is handed a `Program` and
//!   no `Tables`). [`FuncPlan::sites`] names the local, and the local's type is
//!   `Func::locals[l].ty`, which is all `Layouts::of` needs.
//!
//! ## The one place this and `lower` have to agree
//!
//! `lower`'s header says "`middle::rc` inserts them over the CFG, where the
//! basic blocks it needs exist". The pipeline says otherwise and the pipeline
//! is right: `middle::native` calls `rc::run(program)` on the *tree*, before a
//! CFG exists, and it does so because ownership is a whole-program fixpoint
//! over the call graph and `lower` runs one function at a time. So the split
//! is: the analysis here, the
//! placement in `lower`, and this plan is the interface between them.
//!
//! What `lower` has to add is small and mechanical, because it is already
//! holding both halves as it goes: it numbers the nodes it lowers with
//! [`preorder`] and, at each one, emits [`ir::Inst::IncRef`] or
//! [`ir::Inst::DecRef`] for the sites at that node — [`Target::Local`] resolved
//! through the environment it already keeps, [`Target::Node`] being the value
//! it has just produced. Nothing else in the CFG changes, and no dataflow is
//! recomputed there.
//!
//! # What is proved and what is assumed
//!
//! **Landed.** Own/borrow inference; purity, `can_abort` and `can_park`; the
//! insertion plan — incref on a duplicating or capturing use, decref at last
//! use, a drop at the entry of a branch that does not use a live value, a drop
//! at function entry for an owned parameter nothing reads.
//!
//! **Analysis only, and deliberately so.** [`FuncPlan::reuse`] pairs a dying
//! value with a construction in the same arm (MEMORY.md §5.3's "same basic
//! block, matching size class"), behind [`Options::reuse`], which is on.
//! Nothing consumes it, and the reason is not that the transformation is hard:
//! **the construction it would rewrite does not allocate.** A struct, a tuple,
//! an enum and a closure record are register or stack values in both backends
//! (`stencil/emit.rs`'s `MakeStruct` writes into the frame), so
//! `.Cons(f(h), t)` writing into the matched cell has no cell to write into.
//! The allocation probe in `cli/tests/native/` measures it: a thousand
//! functional record updates allocate exactly as many blocks as ten.
//!
//! What *does* allocate is a `Str`'s bytes and a `[T]`'s elements, and the
//! in-place growth for both landed where each operation lives —
//! `cli/runtime/list.rs`'s `append_dest` for `push` and `concat`, and each
//! backend's open-coded `str.concat`. MEMORY.md §5.3 has the list, the growth
//! policy and what is excluded. This pairing stays here, correct and inert,
//! against a layout change that puts aggregates on the heap; the test above is
//! what would notice.
//!
//! **Assumed, and written down because it is a contract with the runtime.**
//! A runtime intrinsic **borrows** its arguments and returns a fresh count;
//! a call through a function *value* **owns** its arguments, because a code
//! pointer cannot carry a per-callee convention. Both are the conventions
//! `VALUE-MODEL.md` §10's runtime already follows, and a runtime function that
//! wanted a count would be the thing that changed.
//!
//! The second of those is about a call's **arguments** and never about the
//! closure it is made *through*. [`ir::Inst::CallIndirect`] loads `code`,
//! passes `env` and returns; what frees an environment is the closure value's
//! own drop, so a call **borrows** its callee. `child_modes` said otherwise and
//! `collect_consuming` said this, and while the two disagreed every closure a
//! program called leaked its environment —
//! `tests::a_called_closure_is_not_consumed_by_the_call`.
//!
//! # Loops, which is where the counts are easiest to get wrong
//!
//! `middle::tail_calls` runs before this pass, so a tail-recursive function is
//! an [`ExprKind::Loop`] of [`ExprKind::Continue`]s by the time it arrives, and
//! a loop body is where a missing drop costs a block *per iteration* rather
//! than once. Four rules, and they are what the loop tests assert:
//!
//! * **A `Loop`'s entries are branches.** One per member of the merged
//!   tail-recursive group, entered from outside, so they are balanced against
//!   each other exactly as a `match`'s arms are.
//! * **A `Continue` carries its arguments into the next iteration's
//!   parameters**, so they are consuming uses at the parameters' own ownership
//!   — the loop's variables *are* the function's parameters, which is what
//!   makes `Continue(acc, xs)` with `xs` unchanged cost nothing at all.
//! * **A jump does not fall through.** The arguments are scanned against an
//!   *empty* liveness, which is what makes a value built in this iteration and
//!   not passed on a last use, and therefore a drop before the back edge.
//! * **A pre-jump drop is keyed on the last argument's `After`**, not on the
//!   `Continue`'s: `lower::continue_` sets the jump as the block's terminator
//!   and starts a fresh unreachable block, so anything placed after the node
//!   would be emitted into a block nothing reaches. A jump with no arguments
//!   has nothing to read and uses its own `Before`. Where every branch of an
//!   enclosing `if` or `match` jumps, the drops that would have gone after it
//!   go before each of those jumps instead — and a `match`'s scrutinee cannot
//!   be dropped at an arm's *entry*, because a payload binding points into it.
//!
//! The invariant that makes one traversal enough: the counts at a back edge
//! are the counts at the loop header. `tests::a_loop_drops_what_it_does_not_
//! carry_before_the_back_edge` checks it by replay, and the native suites
//! re-check it as `buri_rt_live_blocks` holding steady per iteration, which is
//! the same statement about the running program.
//!
//! **Not scanned: a `Lambda`'s body.** `middle::closures` runs immediately
//! before this pass and lifts every lambda to a top-level function with a plan
//! of its own, and `lower` aborts on one that survives. So a `Lambda` here is
//! treated as what it will become — a construction over its captures — and its
//! body carries no operations. While `closures` is a stub, that is a leak
//! inside a lambda and nothing worse; it goes away when the stub does.
//!
//! **Which types carry a count, and where the answer comes from.** Whether a
//! type is reference counted is a question about its *fields*, and the fields
//! of a nominal or context type are a `Tables` question that this pass cannot
//! ask — `middle::native` is handed a `Program`. So `monomorphize` records
//! them: [`monomorphize::Shapes`] is every declared type's field list, taken
//! by the pass that still had a `Tables`, and [`Syntactic`] substitutes into
//! it. [`Counted`] is the classifier interface, and a type it cannot classify is
//! listed in [`FuncPlan::unclassified`] and gets **no reference operations at
//! all** — that direction is a leak, and the other direction is an increment
//! applied to an integer, which is a miscompile.
//!
//! Before the shapes, the answer came from the program's descriptors and its
//! literals alone, which left two holes: a `Ty::Ctx` is written down nowhere,
//! so no literal ever names one, and an `Option<T>` that only ever arrives
//! *from* `list.get` or `list.find` is constructed by no `.Some(..)` either.
//! Both were `Unknown`, so the payload `list.get` retained was released by
//! nothing. `tests::a_context_and_an_option_no_literal_builds_are_both_counted`
//! is that pair, and it also asserts the whole list is empty.
//!
//! **Both native backends build a [`Syntactic`] from the same `Program`** and
//! ask it rather than their own layout table wherever they emit one half of a
//! pair this pass completes (`stencil::emit`'s `rc_counted`). A classifier
//! that answered differently there would be a retain with no release or a
//! release with no retain, so the answer travelling *with* the program is what
//! keeps the two halves from disagreeing.

#![allow(
    clippy::arithmetic_side_effects,
    reason = "every counter here indexes a tree already in memory: a pre-order \
              node number bounded by the node count, a subtree size that is a \
              sum of subtree sizes, and a parameter index bounded by a \
              signature. The one subtraction is a `saturating_sub`."
)]

use crate::compiler::middle::ir;
use crate::compiler::middle::monomorphize::{self, Desc, Func, FuncKind, Program};
use crate::compiler::semantics::typed::{self, Expr, ExprKind, Stmt};
use crate::compiler::semantics::types::{self, FuncIdx, LocalId, Prim, Ty};
use crate::hash::{Map as HashMap, Set as HashSet};

// ---------------------------------------------------------------------------
// The plan
// ---------------------------------------------------------------------------

/// An expression's index in `typed::walk`'s pre-order over one function body.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct NodeId(pub u32);

impl NodeId {
    pub fn index(self) -> usize {
        self.0 as usize
    }
}

/// Where a reference operation goes relative to the node it is keyed on.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Position {
    /// Before the node's own code runs. Used for a drop at the entry of a
    /// branch, and for a drop of a parameter nothing reads.
    Before,
    /// After the node's value exists. Used for the increment a duplicating use
    /// needs.
    After,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RcOp {
    IncRef,
    DecRef,
}

/// What a reference operation applies to.
///
/// Both cases are things `lower` is holding when it reaches the node: a local
/// is in its environment, and a node's value is what it just emitted. A
/// temporary has no name, which is why it is named by the node that produced
/// it — `f(g(x))` where `f` borrows has to drop what `g` returned, and there is
/// no binding to hang that on.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Target {
    Local(LocalId),
    Node(NodeId),
}

/// One reference operation, and where it goes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Site {
    pub node: NodeId,
    pub at: Position,
    pub op: RcOp,
    pub target: Target,
}

impl Site {
    /// The local this operates on, where it operates on one.
    pub fn local(&self) -> Option<LocalId> {
        match self.target {
            Target::Local(l) => Some(l),
            Target::Node(_) => None,
        }
    }
}

/// A dying value paired with a construction that can be written into it.
///
/// MEMORY.md §5.3: the two have to be in the same basic block and the size
/// classes have to match. "Same arm of one match" is the first half; the size
/// class is a layout question, so `fields` is recorded and `lower` compares the
/// classes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Reuse {
    /// The local whose allocation is about to be freed.
    pub token: LocalId,
    /// The construction that would allocate.
    pub at: NodeId,
    /// How many fields the construction writes, for the size-class test.
    pub fields: usize,
}

/// Everything this pass learned about one function.
#[derive(Clone, Debug)]
pub struct FuncPlan {
    /// One per parameter, in declaration order.
    pub params: Vec<ir::Ownership>,
    pub purity: ir::Purity,
    pub can_abort: bool,
    /// Whether this instantiation, or anything it calls, can reach a host
    /// operation that blocks — see [`suspends`]. Nothing reads it yet; it is
    /// computed here because this is the pass that already walks the call
    /// graph, and because the answer is per *instantiation* rather than per
    /// source function, which is the only place it is worth asking.
    pub can_park: bool,
    /// Sorted by `(node, position)`, and stable within one key.
    pub sites: Vec<Site>,
    pub reuse: Vec<Reuse>,
    /// Types [`Counted`] could not answer for, which is why they carry no
    /// reference operations.
    pub unclassified: Vec<Ty>,
    /// Projections whose duplicating use may be answered by asking the parent
    /// rather than unconditionally: the node is a field or element read, and
    /// the local is a parent this expression is the last use of. Empty unless
    /// [`Options::sharing`] is on. See MEMORY.md §5.5.
    pub inherits: Vec<(NodeId, LocalId)>,
}

impl Default for FuncPlan {
    /// What a function nothing is known about gets: every parameter owned,
    /// effectful, abort-capable and park-capable — the conservative row
    /// `ir::Facts` describes.
    fn default() -> FuncPlan {
        FuncPlan {
            params: Vec::new(),
            purity: ir::Purity::Effectful,
            can_abort: true,
            can_park: true,
            sites: Vec::new(),
            reuse: Vec::new(),
            unclassified: Vec::new(),
            inherits: Vec::new(),
        }
    }
}

impl FuncPlan {
    /// The four columns `ir::Facts` wants, so `lower` copies rather than
    /// recomputes.
    pub fn facts(&self) -> ir::Facts {
        ir::Facts {
            params: self.params.clone(),
            purity: self.purity,
            can_abort: self.can_abort,
            can_park: self.can_park,
        }
    }

    /// The operations at one node, in the order they must be emitted.
    pub fn at(&self, node: NodeId, at: Position) -> impl Iterator<Item = &Site> {
        self.sites.iter().filter(move |s| s.node == node && s.at == at)
    }
}

/// What `lower` reads.
#[derive(Clone, Debug, Default)]
pub struct Plan {
    /// One per `Program::funcs` entry, by index.
    pub funcs: Vec<FuncPlan>,
    /// Whether any value of this program can come to be reachable from a
    /// second carrier — see [`crosses_tasks`].
    ///
    /// A **whole-program** answer, not a per-function or per-value one, and
    /// [`crosses_tasks`]'s doc is the argument for that. It reaches
    /// `ir::Program::crosses_tasks`, and from there both native backends,
    /// which emit `buri_rt_values_may_cross_tasks()` into `main` when it is
    /// true. Every block the program allocates is then counted atomically and
    /// no block of any other program is.
    pub crosses_tasks: bool,
}

impl Plan {
    pub fn func(&self, f: FuncIdx) -> Option<&FuncPlan> {
        self.funcs.get(f.index())
    }
}

/// What the pass is allowed to do.
#[derive(Clone, Copy, Debug)]
pub struct Options {
    /// Whether to pair dying values with constructions. On by default: the
    /// pairing is inert until a backend acts on it, and a backend being
    /// brought up can ignore the field or turn this off and see the same
    /// program without it.
    pub reuse: bool,
    /// Whether to answer JavaScript's question instead of the native one:
    /// *where does a second reference to a value come into existence*, with no
    /// releases anywhere. Off by default, and [`sharing`] is the only caller
    /// that turns it on. MEMORY.md §5.5 is what it is for.
    ///
    /// Three things change under it, and each is a place where the native
    /// convention says something a garbage collector makes false:
    ///
    ///  * The list operations that grow a list **consume** their receiver, so
    ///    a caller that goes on using one duplicates it. Native does not need
    ///    this — `append_dest` tests the count at run time — and JavaScript
    ///    has no count to test.
    ///  * A **lambda's body is scanned**, because `middle::closures` does not
    ///    run on this branch and there is no lifted function to carry a plan
    ///    of its own. Its parameters are owned: every runtime function that
    ///    calls one hands it values it has already marked.
    ///  * A projection out of a **dying** parent records [`FuncPlan::inherits`]
    ///    rather than only a duplicating use, which is Perceus's drop
    ///    specialisation with the answer deferred to run time.
    pub sharing: bool,
}

impl Default for Options {
    fn default() -> Options {
        Options { reuse: true, sharing: false }
    }
}

// ---------------------------------------------------------------------------
// Which types carry a count
// ---------------------------------------------------------------------------

/// Whether a value of a type holds a reference to a counted allocation.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Answer {
    Yes,
    No,
    /// Not answerable from anything the program carries. Treated as `No` —
    /// which is a leak — and recorded in [`FuncPlan::unclassified`] so that it
    /// is a list somebody can assert is empty rather than a silent one.
    Unknown,
}

/// The classifier.
pub trait Counted {
    fn counted(&mut self, ty: &Ty) -> Answer;
}

/// The answer from what a program carries about its own types.
///
/// A `Str` and a `[T]` are counted by construction (VALUE-MODEL.md §3, §4); a
/// function value carries a closure environment (§7); a numeric or boolean
/// primitive is not counted at all. Everything else is decided by its fields,
/// and [`monomorphize::Shapes`] is where the fields come from — the declared
/// ones, recorded by the pass that still had a `Tables`, so a nominal or
/// context type is answered whether or not any body in this program happens to
/// construct one.
///
/// **Both native backends build one of these from the same `Program`** and ask
/// it rather than their own layout table wherever they emit one half of a pair
/// this pass completes (`stencil::emit`'s `rc_counted`). So the shapes are
/// the single source of the answer, and the two halves cannot disagree.
///
/// The descriptor and body scans below stay as the answer for a `Program` that
/// carries no shapes — every unit test in this file builds one, and so does
/// every caller that assembles a `Program` by hand.
///
/// The name is now narrower than the type: this is no longer only what syntax
/// can say. It stays because `stencil/mod.rs` and `llvm/emit.rs` name it, and
/// a rename would be an edit to two files for a word.
pub struct Syntactic {
    shapes: monomorphize::Shapes,
    prim_of: HashMap<Ty, Prim>,
    fields_of: HashMap<Ty, Vec<Ty>>,
    memo: HashMap<Ty, Answer>,
    /// Which leaves answer `Yes`. See [`Leaves`].
    leaves: Leaves,
    /// The types the walk is inside, under [`Leaves::Lists`]. See
    /// [`Syntactic::answer`] for why only that question keeps one.
    visiting: HashSet<Ty>,
}

/// What a walk of a type is looking for.
///
/// The walk is the same either way — a nominal type is its fields, a tuple is
/// its components — and only the leaves differ. Two questions share it because
/// getting the *walk* right is the whole difficulty: the shapes, the
/// substitution and the recursion bound are what took the work, and a second
/// copy of them is a second chance to answer a recursive type wrong.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Leaves {
    /// Anything that holds a reference-counted allocation: a `Str`, a `[T]`, a
    /// function value's environment. What the native branch asks.
    Counted,
    /// A `[T]` and nothing else — the only thing an in-place operation ever
    /// writes to, so the only thing a sharing mark is about. A `Str` is an
    /// immutable JavaScript string and a function value is a closure, and
    /// neither can be written through.
    Lists,
}

impl Syntactic {
    pub fn new(program: &Program) -> Syntactic {
        let mut prim_of = HashMap::default();
        let mut fields_of: HashMap<Ty, Vec<Ty>> = HashMap::default();
        // The descriptor graph, read back as types: `desc_index` says which
        // descriptor a type has, and a descriptor says what its components are.
        let mut ty_of: Vec<Option<Ty>> = vec![None; program.descriptors.len()];
        for (ty, i) in &program.desc_index {
            if let Some(slot) = ty_of.get_mut(*i) {
                *slot = Some(ty.clone());
            }
        }
        let lookup = |d: usize, ty_of: &[Option<Ty>]| ty_of.get(d).cloned().flatten();
        for (i, d) in program.descriptors.iter().enumerate() {
            let Some(ty) = lookup(i, &ty_of) else { continue };
            match d {
                Desc::Prim(p) => {
                    prim_of.insert(ty, *p);
                }
                Desc::Struct { fields, .. } => {
                    let cs = fields.iter().filter_map(|f| lookup(f.ty, &ty_of)).collect();
                    fields_of.insert(ty, cs);
                }
                Desc::Enum { variants, .. } => {
                    let cs = variants
                        .iter()
                        .flat_map(|v| v.fields.iter().filter_map(|f| lookup(f.ty, &ty_of)))
                        .collect();
                    fields_of.insert(ty, cs);
                }
                Desc::Unit => {
                    fields_of.insert(ty, Vec::new());
                }
                Desc::Option(inner) => {
                    let cs = lookup(*inner, &ty_of).into_iter().collect();
                    fields_of.insert(ty, cs);
                }
                Desc::Tuple(_) | Desc::Array(_) | Desc::Opaque(_) | Desc::Reserved => {}
            }
        }
        // Descriptors exist only for the types a structural operation reached,
        // and a program that derives nothing has none at all. The bodies say
        // the rest: a literal names its own primitive, a primitive operation
        // names the type of its operands, and a construction names its
        // components — which is a struct's field types without a `Tables`.
        for f in &program.funcs {
            let Some(body) = f.body() else { continue };
            typed::walk(body, &mut |e| match &e.kind {
                ExprKind::Str(_) => {
                    prim_of.entry(e.ty.clone()).or_insert(Prim::Str);
                }
                // An interpolation names `Template`, and nothing else in a
                // body does: a text run is not an `ExprKind::Str` and no
                // literal has the type. Without this row every `Template` was
                // `Answer::Unknown` — read as "not counted" — so the block
                // `lower::template`'s `str.concat` chain ends holding was
                // never dropped, and `println("[${x}]")` in a loop grew the
                // heap by a block an iteration.
                ExprKind::Template { .. } => {
                    prim_of.entry(e.ty.clone()).or_insert(Prim::Template);
                }
                ExprKind::Bool(_) => {
                    prim_of.entry(e.ty.clone()).or_insert(Prim::Bool);
                }
                ExprKind::Char(_) => {
                    prim_of.entry(e.ty.clone()).or_insert(Prim::Char);
                }
                ExprKind::Int(..) => {
                    prim_of.entry(e.ty.clone()).or_insert(Prim::I64);
                }
                ExprKind::Float(_) => {
                    prim_of.entry(e.ty.clone()).or_insert(Prim::F64);
                }
                ExprKind::Prim { prim, args, .. } => {
                    if let Some(a) = args.first() {
                        prim_of.entry(a.ty.clone()).or_insert(*prim);
                    }
                }
                ExprKind::StructLit { fields, .. } => {
                    fields_of
                        .entry(e.ty.clone())
                        .or_insert_with(|| fields.iter().map(|x| x.ty.clone()).collect());
                }
                ExprKind::EnumLit { args, .. } => {
                    // One variant at a time: a type is counted if *any*
                    // variant carries a counted field, so the union is the
                    // right accumulation.
                    let entry = fields_of.entry(e.ty.clone()).or_default();
                    for a in args {
                        if !entry.contains(&a.ty) {
                            entry.push(a.ty.clone());
                        }
                    }
                }
                _ => {}
            });
        }
        Syntactic {
            shapes: program.shapes.clone(),
            prim_of,
            fields_of,
            memo: HashMap::default(),
            leaves: Leaves::Counted,
            visiting: HashSet::default(),
        }
    }

    /// The same table, asked whether a value can reach a **list**.
    pub fn for_lists(program: &Program) -> Syntactic {
        Syntactic { leaves: Leaves::Lists, ..Syntactic::new(program) }
    }

    /// Whether a leaf that carries a count but is never written through
    /// answers yes.
    fn opaque_leaf(&self) -> Answer {
        match self.leaves {
            Leaves::Counted => Answer::Yes,
            Leaves::Lists => Answer::No,
        }
    }

    /// The answer for one type, with the recursion handled the way each
    /// question needs it.
    ///
    /// **Counted** stops at a depth bound and says `Yes`: a type that reaches
    /// itself is behind a pointer (VALUE-MODEL.md §4), so it carries a count
    /// whatever else it holds, and the bound is what makes that terminate
    /// without a visited set threaded through every arm.
    ///
    /// **Lists** cannot borrow that shortcut. "Reaches a list" is a least
    /// fixed point, and an expression tree that reaches only itself and an
    /// `Int` reaches no list at all — answering `Yes` there marks every node
    /// of every interpreter in the language for nothing. So the cycle is cut
    /// with a visited set and contributes nothing, which is the fixed point;
    /// an answer computed inside one is not memoised, because it is only true
    /// under the assumption the outer walk is still testing.
    fn answer(&mut self, ty: &Ty, depth: usize) -> Answer {
        if let Some(a) = self.memo.get(ty) {
            return *a;
        }
        if self.leaves == Leaves::Lists {
            if !self.visiting.insert(ty.clone()) {
                return Answer::No;
            }
            let a = self.walk(ty, depth);
            self.visiting.remove(ty);
            if self.visiting.is_empty() {
                self.memo.insert(ty.clone(), a);
            }
            return a;
        }
        if depth == 0 {
            return Answer::Yes;
        }
        let a = self.walk(ty, depth);
        self.memo.insert(ty.clone(), a);
        a
    }

    /// One level of the type, with the components asked through
    /// [`Syntactic::answer`].
    fn walk(&mut self, ty: &Ty, depth: usize) -> Answer {
        if depth == 0 {
            return match self.leaves {
                Leaves::Counted => Answer::Yes,
                Leaves::Lists => Answer::Unknown,
            };
        }
        match ty {
            Ty::Array(_) => Answer::Yes,
            Ty::Fn(..) => self.opaque_leaf(),
            Ty::Unit => Answer::No,
            Ty::Tuple(ts) => {
                let parts: Vec<Answer> = ts.iter().map(|t| self.answer(t, depth - 1)).collect();
                join(&parts)
            }
            // A context value is a record of the values bound to its effects
            // (SPEC 11.3), and one of those is as often a closure as it is a
            // zero-sized marker. Nothing writes a `Ty::Ctx` down, so no
            // literal in any body names one and the scans below never see it:
            // without the shapes every context in the program was `Unknown`.
            Ty::Ctx(id) => match self.shapes.ctxs.get(id.index()) {
                Some(bindings) => self.join_of(&bindings.clone(), depth),
                None => Answer::Unknown,
            },
            Ty::Con(con, args) => match self.shapes.cons.get(con.index()) {
                Some(monomorphize::ConShape::Prim(p)) => {
                    if matches!(p, Prim::Str | Prim::Template) {
                        self.opaque_leaf()
                    } else {
                        Answer::No
                    }
                }
                Some(monomorphize::ConShape::Fields(declared)) => {
                    // The declared fields still carry the type's own
                    // `Ty::Param`s, so this instantiation's arguments are what
                    // turn `Option<T>`'s payload into the `Str` it is here.
                    let fields: Vec<Ty> = declared
                        .clone()
                        .iter()
                        .map(|f| types::substitute(f, args, None))
                        .collect();
                    self.join_of(&fields, depth)
                }
                None => self.scanned(ty, depth),
            },
            _ => Answer::Unknown,
        }
    }

    /// The answer for a `Ty::Con` from the descriptor graph and the bodies.
    fn scanned(&mut self, ty: &Ty, depth: usize) -> Answer {
        if let Some(p) = self.prim_of.get(ty).copied() {
            if matches!(p, Prim::Str | Prim::Template) {
                self.opaque_leaf()
            } else {
                Answer::No
            }
        } else if let Some(fields) = self.fields_of.get(ty).cloned() {
            self.join_of(&fields, depth)
        } else {
            Answer::Unknown
        }
    }

    fn join_of(&mut self, fields: &[Ty], depth: usize) -> Answer {
        let parts: Vec<Answer> = fields.iter().map(|t| self.answer(t, depth - 1)).collect();
        join(&parts)
    }
}

/// A value is counted when any component is, unknown when a component is
/// unknown and none is counted, and uncounted only when every component is.
fn join(parts: &[Answer]) -> Answer {
    if parts.contains(&Answer::Yes) {
        Answer::Yes
    } else if parts.contains(&Answer::Unknown) {
        Answer::Unknown
    } else {
        Answer::No
    }
}

impl Counted for Syntactic {
    fn counted(&mut self, ty: &Ty) -> Answer {
        // Eight is past the nesting any concrete type in the standard library
        // has, and a type deeper than that is one behind a pointer anyway.
        let a = self.answer(ty, 8);
        // The two questions have opposite safe directions. A release the
        // native branch is unsure about is a leak; a mark this branch is
        // unsure about is an aliased list nobody copied, so an unanswerable
        // type is marked rather than skipped.
        match (self.leaves, a) {
            (Leaves::Lists, Answer::Unknown) => Answer::Yes,
            _ => a,
        }
    }
}

// ---------------------------------------------------------------------------
// The pass
// ---------------------------------------------------------------------------

/// Infers ownership, decides where every reference operation goes, and pairs
/// dying values with constructions.
///
/// Returns the plan rather than changing the tree: the operations are IR
/// instructions, and `lower` places them. See the module docs for the contract.
pub fn run(program: &Program) -> Plan {
    let mut counted = Syntactic::new(program);
    analyze(program, &mut counted, &Options::default())
}

/// The same analysis, asked JavaScript's question: where does a second
/// reference to a value come into existence?
///
/// The plan that comes back is read for its [`RcOp::IncRef`] sites and its
/// [`FuncPlan::inherits`] and for nothing else — a garbage collector has no
/// use for a release, and the JavaScript backend emits none. `reuse` is off
/// because nothing on this branch reads it. MEMORY.md §5.5.
pub fn sharing(program: &Program) -> Plan {
    let mut counted = Syntactic::for_lists(program);
    analyze(program, &mut counted, &Options { reuse: false, sharing: true })
}

/// The same, against a caller's own [`Counted`] and options — which is how wave
/// 2 hands in a `middle::layout`-backed classifier.
pub fn analyze(program: &Program, counted: &mut dyn Counted, opts: &Options) -> Plan {
    let ownership = infer_ownership(program, counted, opts);
    let (purity, can_abort, can_park) = infer_effects(program);
    let mut funcs: Vec<FuncPlan> = Vec::with_capacity(program.funcs.len());
    for (i, f) in program.funcs.iter().enumerate() {
        let params = ownership.get(i).cloned().unwrap_or_default();
        let mut plan = FuncPlan {
            params,
            purity: purity.get(i).copied().unwrap_or(ir::Purity::Effectful),
            can_abort: can_abort.get(i).copied().unwrap_or(true),
            can_park: can_park.get(i).copied().unwrap_or(true),
            sites: Vec::new(),
            reuse: Vec::new(),
            unclassified: Vec::new(),
            inherits: Vec::new(),
        };
        if let Some(body) = f.body() {
            let mut sizes: Vec<u32> = Vec::new();
            subtree_sizes(body, &mut sizes);
            let (child_at, child_ids) = child_index(&sizes);
            let mut scan = Scan {
                func: f,
                counted,
                ownership: &ownership,
                sizes: &sizes,
                child_at,
                child_ids,
                owned: HashSet::default(),
                sites: Vec::new(),
                reuse: Vec::new(),
                unclassified: Vec::new(),
                used: HashSet::default(),
                pending: Vec::new(),
                floor: 0,
                jumps: Vec::new(),
                diverged: false,
                named: vec![None; sizes.len()],
                self_params: plan.params.clone(),
                inherits: Vec::new(),
                opts,
            };
            for (k, p) in f.params.iter().enumerate() {
                if plan.params.get(k).copied() == Some(ir::Ownership::Own) && scan.is_counted(*p) {
                    scan.owned.insert(*p);
                }
            }
            // A `let` always owns what it binds, and the scan runs backwards —
            // so the obligations are collected before it, or a use scanned
            // before its binder would not know there was one.
            let mut bound: Vec<LocalId> = Vec::new();
            typed::walk(body, &mut |e| {
                if let ExprKind::Block { stmts, .. } = &e.kind {
                    for st in stmts {
                        if let Stmt::Let { pattern, .. } = st {
                            pattern.binds(&mut bound);
                        }
                    }
                }
            });
            for b in bound {
                if scan.is_counted(b) {
                    scan.owned.insert(b);
                }
            }
            // A lambda's parameters arrive owned. `middle::closures` does not
            // run on the branch this option serves, so the body is scanned
            // here rather than as a lifted function — and every runtime
            // function that calls a lambda marks what it hands over first.
            if opts.sharing {
                let mut params: Vec<LocalId> = Vec::new();
                typed::walk(body, &mut |e| {
                    if let ExprKind::Lambda { params: ps, .. } = &e.kind {
                        params.extend(ps.iter().copied());
                    }
                });
                for p in params {
                    if scan.is_counted(p) {
                        scan.owned.insert(p);
                    }
                }
            }
            scan.expr(body, NodeId(0), &Live::default(), Mode::Own);
            // Anything a borrow left pending at the root is dropped before the
            // function returns.
            scan.flush(NodeId(0));
            // An owned parameter nothing reads at all is dropped where it
            // arrives. Liveness is the wrong question here: a `match` that
            // consumes its scrutinee takes it out of the live set, and that is
            // a parameter that *was* read.
            for (k, p) in f.params.iter().enumerate() {
                if plan.params.get(k).copied() == Some(ir::Ownership::Own)
                    && scan.is_counted(*p)
                    && !scan.used.contains(p)
                {
                    scan.sites.push(Site {
                        node: NodeId(0),
                        at: Position::Before,
                        op: RcOp::DecRef,
                        target: Target::Local(*p),
                    });
                }
            }
            plan.sites = scan.sites;
            plan.reuse = scan.reuse;
            plan.unclassified = scan.unclassified;
            plan.inherits = scan.inherits;
            plan.sites.sort_by_key(|s| (s.node.0, matches!(s.at, Position::After)));
        }
        funcs.push(plan);
    }
    // The whole-program escape answer. `program.funcs` is the
    // post-monomorphization set, so an intrinsic present in it is one the
    // program can *reach* — the same reachability `infer_effects`'s fixpoint
    // computes, asked of a set of keys rather than propagated to callers,
    // because the mark it feeds is set once for the whole artifact and there
    // is no caller to attribute it to.
    let crosses_tasks = program
        .funcs
        .iter()
        .any(|f| matches!(&f.kind, FuncKind::Intrinsic(key) if crosses_tasks(key)));
    Plan { funcs, crosses_tasks }
}

// ---------------------------------------------------------------------------
// Ownership: a fixpoint over the exact call graph
// ---------------------------------------------------------------------------

/// Which parameters a callee takes a count for.
///
/// MEMORY.md §5.2: a parameter is **borrowed** if the callee neither stores it
/// in a constructed value, nor returns it, nor passes it to a function that
/// owns it. Everything starts borrowed and is promoted, which is the direction
/// that terminates: promotion only ever adds, so the iteration is monotone.
///
/// The call graph is exact, so the answer is a fact rather than an
/// approximation — the whole reason this is worth doing here rather than in a
/// backend.
/// The list operations that write into their receiver where they can.
///
/// Under [`Options::sharing`] each of these **consumes** its receiver, so a
/// caller that keeps the list duplicates it and the duplication is a mark the
/// backend can see. Native does not need the promotion: `cli/runtime/list.rs`'s
/// `append_dest` asks the count at run time, and a count is the thing
/// JavaScript does not have.
const GROWS_ITS_RECEIVER: &[&str] = &[
    "list.push",
    "list.concat",
    "list.reverse",
    "list.take",
    "list.drop",
    "list.slice",
];

fn infer_ownership(
    program: &Program,
    counted: &mut dyn Counted,
    opts: &Options,
) -> Vec<Vec<ir::Ownership>> {
    let mut own: Vec<Vec<ir::Ownership>> = program
        .funcs
        .iter()
        .map(|f| {
            f.params
                .iter()
                .map(|p| {
                    let ty = f.locals.get(p.index()).map(|l| l.ty.clone()).unwrap_or(Ty::Error);
                    // A value with no count to take is `Own` by convention and
                    // costs nothing either way; saying `Borrow` would make a
                    // backend's signature depend on the classifier's uncertainty.
                    match counted.counted(&ty) {
                        Answer::Yes => ir::Ownership::Borrow,
                        _ => ir::Ownership::Own,
                    }
                })
                .collect()
        })
        .collect();
    // An intrinsic borrows what it is given (see the module docs), so its row
    // never moves — except for the growing list operations under `sharing`,
    // whose receiver is seeded owned before the fixpoint so that callers are
    // promoted against it.
    if opts.sharing {
        for (i, f) in program.funcs.iter().enumerate() {
            let FuncKind::Intrinsic(key) = &f.kind else { continue };
            if !GROWS_ITS_RECEIVER.contains(&key.as_str()) {
                continue;
            }
            if let Some(slot) = own.get_mut(i).and_then(|r| r.first_mut()) {
                *slot = ir::Ownership::Own;
            }
        }
    }
    let mut changed = true;
    while changed {
        changed = false;
        for (i, f) in program.funcs.iter().enumerate() {
            let Some(body) = f.body() else { continue };
            let mut consumed: HashSet<LocalId> = HashSet::default();
            consuming_uses(body, &own, i, &mut consumed);
            let Some(row) = own.get(i) else { continue };
            let promoted: Vec<ir::Ownership> = f
                .params
                .iter()
                .zip(row.iter())
                .map(|(p, o)| {
                    if *o == ir::Ownership::Borrow && consumed.contains(p) {
                        ir::Ownership::Own
                    } else {
                        *o
                    }
                })
                .collect();
            if Some(&promoted) != own.get(i) {
                if let Some(slot) = own.get_mut(i) {
                    *slot = promoted;
                }
                changed = true;
            }
        }
        for (target, k) in loop_variables_taken(program, &own) {
            match own.get_mut(target).and_then(|r| r.get_mut(k)) {
                Some(o) if *o == ir::Ownership::Borrow => {
                    *o = ir::Ownership::Own;
                    changed = true;
                }
                _ => {}
            }
        }
    }
    own
}

/// The loop variables a `Continue` hands a value the iteration will not
/// outlive, as `(function, parameter)` pairs.
///
/// A **borrowed** parameter is one the caller keeps a count for across the
/// whole call. That covers an argument passed straight through — the loop's
/// `Continue(acc, xs)` with `xs` unchanged, which the module header says costs
/// nothing — and it covers nothing else: a value this iteration built has no
/// owner left the moment the jump is taken, and the drop the borrow convention
/// puts before the back edge frees the next iteration's variable.
/// `middle/rc.rs`'s own `a_jump_owns_what_it_did_not_pass_through` is that
/// shape; `stepV` below was the program.
fn loop_variables_taken(program: &Program, own: &[Vec<ir::Ownership>]) -> Vec<(usize, usize)> {
    let mut out: Vec<(usize, usize)> = Vec::new();
    for (i, f) in program.funcs.iter().enumerate() {
        let Some(body) = f.body() else { continue };
        let row = own.get(i);
        typed::walk(body, &mut |e| {
            let ExprKind::Continue { func, args, .. } = &e.kind else { return };
            let target = func.map_or(i, FuncIdx::index);
            for (k, a) in args.iter().enumerate() {
                let through = match &a.kind {
                    ExprKind::Local(l) => f
                        .params
                        .iter()
                        .position(|p| p == l)
                        .and_then(|j| row.and_then(|r| r.get(j)))
                        .is_some_and(|o| *o == ir::Ownership::Borrow),
                    _ => false,
                };
                if !through {
                    out.push((target, k));
                }
            }
        });
    }
    out
}

/// Every local this body consumes: stores in a construction, returns, captures,
/// or passes to a parameter that owns it.
fn consuming_uses(
    body: &Expr,
    own: &[Vec<ir::Ownership>],
    self_index: usize,
    out: &mut HashSet<LocalId>,
) {
    // Repeated to a fixpoint: whether a `match` consumes its scrutinee depends
    // on whether the payloads it binds are consumed, and those are found by
    // this same walk. It terminates because the set only grows and is bounded
    // by the function's locals.
    loop {
        let before = out.len();
        collect_consuming(body, own, self_index, out);
        if out.len() == before {
            return;
        }
    }
}

fn collect_consuming(
    body: &Expr,
    own: &[Vec<ir::Ownership>],
    self_index: usize,
    out: &mut HashSet<LocalId>,
) {
    // The tail of a function is returned, and a `let` transfers into a local
    // whose own last use decides the rest, so both count as consuming.
    let consume = |e: &Expr, out: &mut HashSet<LocalId>| {
        if let ExprKind::Local(l) = &e.kind {
            out.insert(*l);
        }
    };
    typed::walk(body, &mut |e| match &e.kind {
        ExprKind::StructLit { fields: args, .. }
        | ExprKind::EnumLit { args, .. }
        | ExprKind::Tuple(args)
        | ExprKind::Array(args)
        | ExprKind::Closure { env: args, .. }
        | ExprKind::CallValue { args, .. } => {
            args.iter().for_each(|a| consume(a, out));
        }
        ExprKind::CtxLit { bindings } => bindings.iter().for_each(|(_, a)| consume(a, out)),
        ExprKind::StructUpdate { base, updates, .. } => {
            consume(base, out);
            updates.iter().for_each(|(_, a)| consume(a, out));
        }
        ExprKind::Lambda { captures, .. } => out.extend(captures.iter().copied()),
        // Taking a value apart and keeping a piece takes the whole: the piece
        // is a reference into it, and it outlives the match. This is where a
        // uniquely-owned value becomes a dying one, and therefore where reuse
        // becomes possible (MEMORY.md §5.3).
        ExprKind::Match { scrutinee, arms } => {
            let kept = arms.iter().any(|a| {
                let mut bound = Vec::new();
                a.pattern.binds(&mut bound);
                bound.iter().any(|b| out.contains(b))
            });
            if kept {
                consume(scrutinee, out);
            }
        }
        ExprKind::CallFn { func, args } => {
            let row = func.func().and_then(|f| own.get(f.index()));
            for (k, a) in args.iter().enumerate() {
                let owns = row.and_then(|r| r.get(k)).copied().unwrap_or(ir::Ownership::Own);
                if owns == ir::Ownership::Own {
                    consume(a, out);
                }
            }
        }
        // A jump carries its arguments into the next iteration's parameters,
        // which outlive this one — so it takes them exactly as a call does.
        // `None` re-enters this function's own loop, so the row to read is its
        // own, which is what makes the fixpoint close over a loop.
        ExprKind::Continue { func, args, .. } => {
            let row = own.get(func.map_or(self_index, FuncIdx::index));
            for (k, a) in args.iter().enumerate() {
                let owns = row.and_then(|r| r.get(k)).copied().unwrap_or(ir::Ownership::Own);
                if owns == ir::Ownership::Own {
                    consume(a, out);
                }
            }
        }
        _ => {}
    });
    // The returned value, through whatever tail position leads to it.
    for t in tails(body) {
        consume(t, out);
    }
}

/// Every expression whose value the enclosing one returns unchanged.
fn tails(e: &Expr) -> Vec<&Expr> {
    match &e.kind {
        ExprKind::Block { tail: Some(t), .. } => tails(t),
        ExprKind::If { then, else_, .. } => {
            let mut out = tails(then);
            out.extend(tails(else_));
            out
        }
        ExprKind::Match { arms, .. } => arms.iter().flat_map(|a| tails(&a.body)).collect(),
        // A loop is a whole function body, so what an entry answers is what
        // the function returns. Without this, a tail-recursive function's base
        // case did not count as returning its accumulator, and the parameter
        // carrying it was inferred borrowed.
        ExprKind::Loop { entries } => entries.iter().flat_map(|x| tails(x)).collect(),
        _ => vec![e],
    }
}

// ---------------------------------------------------------------------------
// Purity, abortability and parkability, the other three columns of `ir::Facts`
// ---------------------------------------------------------------------------

/// The intrinsics this compiler knows the effect of. Everything else is
/// `Effectful`, which is the answer that costs an attribute rather than
/// correctness.
///
/// The `derive*` family is here because `middle::derives` emits it and this
/// module is where its effect is written down: rendering and encoding allocate,
/// comparing and hashing do not.
fn intrinsic_purity(name: &str) -> ir::Purity {
    match name {
        "derivePrimHash" | "deriveArrayHash" | "deriveArrayEq" | "deriveArrayCompare" => {
            ir::Purity::Pure
        }
        "derivePrimShow" | "deriveArrayShow" | "derivePrimJson" | "deriveArrayJson" => {
            ir::Purity::Allocating
        }
        _ => ir::Purity::Effectful,
    }
}

/// Whether an intrinsic key names a host operation that **blocks**: the call
/// does not return until something outside this program — a disk, a socket, a
/// clock, a terminal — is ready.
///
/// This is the seed of the `can_park` column, and it is a list of *keys*
/// rather than of effects on purpose. `Fs` is an effect; `host.HostFs` and
/// `host_testing.TestFs` are two implementations of it, and only the first
/// one waits. A per-instantiation answer can tell them apart because they are
/// different `Func` slots, and that difference is the whole point of asking
/// the question here rather than at the signature.
///
/// Everything absent is *not* suspending, so an omission is the direction that
/// costs correctness rather than performance. That is why the whole
/// `host.HostFs` surface is in by prefix rather than method by method, and why
/// a new blocking host operation belongs here on the day it is added.
pub fn suspends(key: &str) -> bool {
    key.starts_with("host.HostFs.")
        // Every `Listen` operation waits on something outside the program: a
        // bind resolves a name, an accept waits for a client — the longest wait
        // a program can make — and a respond writes to a socket a peer may be
        // reading slowly. By prefix for `host.HostFs`'s reason, and because a
        // fifth operation added here should not need an edit there to be
        // correct. `host.HostSockets` is deliberately absent: a frame is
        // enqueued rather than delivered, which is the whole of what
        // `socketSendText` promises.
        || key.starts_with("host.HostListen.")
        || matches!(
            key,
            "host.HostNet.fetch"
                | "host.HostClock.sleepMillis"
                | "host.HostStdin.readLine"
                | "host.HostStdin.readBytes"
                // `Tasks.parallel` is the one entry here that does not wait on
                // anything *outside* the program: it waits on the program's own
                // tasks. It belongs on the list all the same, and for the same
                // consequence — the call does not return until something else
                // has finished, so a caller of it is a function that may be in
                // the middle of a call while other work runs. On JavaScript
                // that is literally an `await`; on the natives it is what makes
                // the caller's frame outlive a scheduling decision.
                | "host.HostTasks.parallel"
        )
}

/// Whether an intrinsic key **hands a value of this program to another
/// carrier**: after the call, a block the caller made may be counted, read and
/// released by a thread that is not this one.
///
/// This is the seed of `Plan::crosses_tasks`, and it is the escape-analysis
/// question the multi-threaded mark asks (`middle::layout::CAP_SHARED_FLAG`,
/// MEMORY.md §5.1). Like [`suspends`] it is a list of **keys** rather than of
/// effects, and for the sharper form of the same reason: `Tasks` is an effect
/// and `testing_context.TestTasks` is an implementation of it that runs every
/// step on the calling thread, so the answer is a property of the
/// implementation the program actually bound.
///
/// **By prefix, not method by method**, which is the direction an omission has
/// to cost performance rather than correctness. Every `host.HostTasks` row is
/// one today and every row track F adds — `send`, `ask`, a detached `start` —
/// is one on the day it lands, without an edit here to remember. A key absent
/// from this list is a value the program is promised nobody else can see, and
/// the promise is kept by non-atomic counts on both backends, so an omission
/// here is a silent aliasing bug. `host.HostTasks` is spelled once and covers
/// the surface.
pub fn crosses_tasks(key: &str) -> bool {
    key.starts_with("host.HostTasks.")
}

fn worse(a: ir::Purity, b: ir::Purity) -> ir::Purity {
    let rank = |p: ir::Purity| match p {
        ir::Purity::Pure => 0u8,
        ir::Purity::Allocating => 1,
        ir::Purity::Effectful => 2,
    };
    if rank(a) >= rank(b) {
        a
    } else {
        b
    }
}

/// Purity, abortability and parkability: three fixpoints over the same exact
/// call graph, computed in one iteration because they are the same walk.
///
/// The graph is the *post-monomorphization* one, so every column is per
/// instantiation. `can_park` is the one that needs that: `fs.readText` at a
/// context binding `host.HostFs` and `fs.readText` at one binding
/// `host_testing.TestFs` are two `Func` slots reached from two `Key::Fn`
/// entries, and only the first reaches a call that waits.
fn infer_effects(program: &Program) -> (Vec<ir::Purity>, Vec<bool>, Vec<bool>) {
    let mut purity: Vec<ir::Purity> = program
        .funcs
        .iter()
        .map(|f| match &f.kind {
            // A body that has not been built aborts when reached, and an
            // intrinsic is whatever the runtime does.
            FuncKind::Unbuilt => ir::Purity::Pure,
            FuncKind::Intrinsic(key) => intrinsic_purity(key),
            FuncKind::Body(_) => ir::Purity::Pure,
        })
        .collect();
    let mut aborts: Vec<bool> = program
        .funcs
        .iter()
        .map(|f| matches!(f.kind, FuncKind::Unbuilt | FuncKind::Intrinsic(_)))
        .collect();
    // An unbuilt body is lowered to an abort, which is the one thing that
    // certainly does not wait, so only a suspending intrinsic seeds `true`.
    let mut parks: Vec<bool> = program
        .funcs
        .iter()
        .map(|f| match &f.kind {
            FuncKind::Intrinsic(key) => suspends(key),
            FuncKind::Unbuilt | FuncKind::Body(_) => false,
        })
        .collect();
    let mut changed = true;
    while changed {
        changed = false;
        for (i, f) in program.funcs.iter().enumerate() {
            let Some(body) = f.body() else { continue };
            let mut p = ir::Purity::Pure;
            let mut a = false;
            let mut k = false;
            typed::walk(body, &mut |e| match &e.kind {
                ExprKind::CallFn { func, .. } => {
                    if let Some(c) = func.func() {
                        p = worse(p, purity.get(c.index()).copied().unwrap_or(ir::Purity::Effectful));
                        a = a || aborts.get(c.index()).copied().unwrap_or(true);
                        k = k || parks.get(c.index()).copied().unwrap_or(true);
                    } else {
                        p = ir::Purity::Effectful;
                        a = true;
                        k = true;
                    }
                }
                // A jump into another function's loop is a call, and one
                // whose effects propagate the same way.
                ExprKind::Continue { func: Some(c), .. } => {
                    p = worse(p, purity.get(c.index()).copied().unwrap_or(ir::Purity::Effectful));
                    a = a || aborts.get(c.index()).copied().unwrap_or(true);
                    k = k || parks.get(c.index()).copied().unwrap_or(true);
                }
                // An indirect call reaches a code pointer this pass cannot
                // name, so it is whatever the worst function in the program is.
                ExprKind::CallValue { .. } | ExprKind::CallTrait { .. } => {
                    p = ir::Purity::Effectful;
                    a = true;
                    k = true;
                }
                ExprKind::Intrinsic { name, .. } => {
                    p = worse(p, intrinsic_purity(name));
                    k = k || suspends(name);
                }
                ExprKind::CtxGet { .. } | ExprKind::CtxLit { .. } | ExprKind::CtxCall { .. } => {
                    p = ir::Purity::Effectful;
                }
                // Division by zero aborts (SPEC 6.10), and so does an
                // exhausted allocation budget (MEMORY.md §7.2).
                ExprKind::Prim { op, .. } => {
                    if matches!(op, typed::PrimOp::Div | typed::PrimOp::Rem) {
                        a = true;
                    }
                }
                ExprKind::Array(_) | ExprKind::Template { .. } => {
                    p = worse(p, ir::Purity::Allocating);
                }
                _ => {}
            });
            // A function that *is* a suspending intrinsic has no body and is
            // never reached here; one with a body starts at `false` and only
            // ever climbs, so the seed is not lost.
            if purity.get(i).copied() != Some(p)
                || aborts.get(i).copied() != Some(a)
                || parks.get(i).copied() != Some(k)
            {
                if let Some(slot) = purity.get_mut(i) {
                    *slot = p;
                }
                if let Some(slot) = aborts.get_mut(i) {
                    *slot = a;
                }
                if let Some(slot) = parks.get_mut(i) {
                    *slot = k;
                }
                changed = true;
            }
        }
    }
    (purity, aborts, parks)
}

// ---------------------------------------------------------------------------
// Node numbering
// ---------------------------------------------------------------------------

/// [`typed::children`], collected.
///
/// This and `typed::walk` have to agree, because [`NodeId`] is defined by the
/// latter and computed by the former; they are now the same arms, and
/// `preorder_matches_typed_walk` still says so. A `Vec` rather than the
/// callback because the numbering needs each child's *index* and needs to
/// stop early.
fn kids(e: &Expr) -> Vec<&Expr> {
    let mut out: Vec<&Expr> = Vec::new();
    typed::children(e, &mut |k| out.push(k));
    out
}

/// Walks a body handing each node its [`NodeId`]. This is the numbering
/// [`FuncPlan::sites`] is keyed by; `lower` uses this function rather than a
/// second copy of the rule.
pub fn preorder(body: &Expr, f: &mut impl FnMut(NodeId, &Expr)) {
    fn go(e: &Expr, next: &mut u32, f: &mut impl FnMut(NodeId, &Expr)) {
        let id = NodeId(*next);
        *next += 1;
        f(id, e);
        for k in kids(e) {
            go(k, next, f);
        }
    }
    go(body, &mut 0, f);
}

/// The number of nodes in each subtree, in pre-order — so that a node's `k`th
/// child's id is computable from its own without a second traversal.
/// Every node's children, as one flat array with a start index per node.
///
/// The preorder array `subtree_sizes` builds says how big a subtree is, which
/// is enough to find the next sibling and not enough to find the `k`th child
/// without walking to it. This walks each node's children once — the total is
/// one entry per edge, so it is linear — and [`Scan::child`] is then a lookup.
fn child_index(sizes: &[u32]) -> (Vec<u32>, Vec<u32>) {
    let n = sizes.len();
    let mut at: Vec<u32> = Vec::with_capacity(n.saturating_add(1));
    let mut ids: Vec<u32> = Vec::with_capacity(n);
    for (i, size) in sizes.iter().enumerate() {
        at.push(u32::try_from(ids.len()).unwrap_or(u32::MAX));
        let node = u32::try_from(i).unwrap_or(u32::MAX);
        let end = node.saturating_add(*size);
        let mut cur = node.saturating_add(1);
        while cur < end {
            ids.push(cur);
            cur = cur.saturating_add(sizes.get(cur as usize).copied().unwrap_or(1));
        }
    }
    at.push(u32::try_from(ids.len()).unwrap_or(u32::MAX));
    (at, ids)
}

fn subtree_sizes(e: &Expr, out: &mut Vec<u32>) -> u32 {
    let me = out.len();
    out.push(0);
    let mut total = 1u32;
    for k in kids(e) {
        total += subtree_sizes(k, out);
    }
    if let Some(slot) = out.get_mut(me) {
        *slot = total;
    }
    total
}

// ---------------------------------------------------------------------------
// The insertion plan
// ---------------------------------------------------------------------------

/// Whether the value an expression produces is being taken (a count has to come
/// with it) or only read.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Mode {
    Own,
    Borrow,
}

type Live = HashSet<LocalId>;

struct Scan<'a> {
    func: &'a Func,
    counted: &'a mut dyn Counted,
    ownership: &'a [Vec<ir::Ownership>],
    sizes: &'a [u32],
    /// Where node `i`'s children start in `child_ids`, by node id.
    child_at: Vec<u32>,
    /// Every node's children, in order, flattened. See [`child_index`].
    child_ids: Vec<u32>,
    /// Locals this function has an obligation to drop.
    owned: HashSet<LocalId>,
    sites: Vec<Site>,
    reuse: Vec<Reuse>,
    unclassified: Vec<Ty>,
    /// Every local the body mentions at all, which is what decides whether a
    /// parameter is dropped where it arrives.
    used: HashSet<LocalId>,
    /// Owned locals whose last use has been seen and whose drop has to land
    /// *after* the operation that read them. A borrow does not free anything,
    /// so the drop cannot go where the read is: `size(p)` has to keep `p`
    /// alive across the call and drop it after.
    pending: Vec<LocalId>,
    /// How far into [`Scan::pending`] the expression currently being scanned
    /// owns. Entries below it were raised by a sibling and belong to the parent
    /// that will flush after *every* sibling has run.
    ///
    /// The scan runs right to left, so an entry raised before this one started
    /// belongs to an expression that runs **later**. Without the mark,
    /// [`Scan::flush`] drained the whole list, and the first sibling to flush
    /// emitted a drop for a base its right-hand neighbour had not read yet —
    /// `p.a.reverse(ctx).concat(ctx, p.b)`, where the `reverse` call's own
    /// flush freed `p` between `p.a` and `p.b`.
    floor: usize,
    /// Where a drop goes when the code that would have run after this
    /// expression never does, because every path out of it is a jump: one key
    /// per `Continue` reached, each of them the last point before the back
    /// edge.
    jumps: Vec<(NodeId, Position)>,
    /// Whether every path out of the expression just scanned is a jump.
    diverged: bool,
    /// [`Scan::names_in`]'s memo, one slot per node, filled only for the nodes
    /// that are asked — a short-circuit's right operand. See that function for
    /// why it holds the *unfiltered* names.
    named: Vec<Option<Vec<LocalId>>>,
    /// The function's own parameter ownership, for a `Continue` that re-enters
    /// the loop it is inside: the loop's variables *are* the parameters
    /// (`typed::ExprKind::Loop`).
    self_params: Vec<ir::Ownership>,
    /// [`FuncPlan::inherits`], as it is found.
    inherits: Vec<(NodeId, LocalId)>,
    opts: &'a Options,
}


impl Scan<'_> {
    fn is_counted(&mut self, l: LocalId) -> bool {
        let Some(local) = self.func.locals.get(l.index()) else { return false };
        let ty = local.ty.clone();
        self.counted_ty(&ty)
    }

    fn counted_ty(&mut self, ty: &Ty) -> bool {
        match self.counted.counted(ty) {
            Answer::Yes => true,
            Answer::No => false,
            Answer::Unknown => {
                if !self.unclassified.contains(ty) {
                    self.unclassified.push(ty.clone());
                }
                false
            }
        }
    }

    fn push(&mut self, node: NodeId, at: Position, op: RcOp, target: Target) {
        self.sites.push(Site { node, at, op, target });
    }

    /// Scans something and reports what jumped out of it: the keys of the
    /// `Continue`s inside, and whether *every* path out was one of them.
    ///
    /// A branch that ends in a jump has no "after", so a drop the enclosing
    /// expression wanted to put there has to go before the jump instead.
    fn scoped<T>(&mut self, f: impl FnOnce(&mut Self) -> T) -> (T, Vec<(NodeId, Position)>, bool) {
        let outer_jumps = std::mem::take(&mut self.jumps);
        let outer_diverged = std::mem::replace(&mut self.diverged, false);
        let out = f(self);
        let jumps = std::mem::replace(&mut self.jumps, outer_jumps);
        let diverged = std::mem::replace(&mut self.diverged, outer_diverged);
        self.jumps.extend(jumps.iter().copied());
        (out, jumps, diverged)
    }

    /// The drops raised inside the expression being scanned, taken off the
    /// list. Anything below [`Scan::floor`] stays: it belongs to a sibling that
    /// has not run yet.
    fn take_pending(&mut self) -> Vec<LocalId> {
        let floor = self.floor.min(self.pending.len());
        let mut pending = self.pending.split_off(floor);
        pending.sort_by_key(|l| l.0);
        pending.dedup();
        pending
    }

    /// Emits the pending drops before each of a set of jumps, rather than
    /// after a node whose "after" is unreachable.
    fn flush_at(&mut self, keys: &[(NodeId, Position)]) {
        let pending = self.take_pending();
        for (node, at) in keys {
            for l in &pending {
                self.push(*node, *at, RcOp::DecRef, Target::Local(*l));
            }
        }
    }

    /// Emits the drops the children raised, after this node's own code. Every
    /// construct with children flushes; a branching one flushes per branch, so
    /// a drop cannot end up on a path that never made the value.
    fn flush(&mut self, node: NodeId) {
        let pending = self.take_pending();
        for l in pending {
            self.push(node, Position::After, RcOp::DecRef, Target::Local(l));
        }
    }

    /// What a **projection** does with the drops its base raised, which is not
    /// what every other construct does with them.
    ///
    /// `p.a`, `p.0`, `xs[i]` and `ctx.f` read their base without taking it, and
    /// the value they produce is *words copied out of the base* — a `Str`'s
    /// three words, a `[T]`'s two — with no count of their own. So the base has
    /// to outlive whatever goes on to read those words, and that is the
    /// **parent**, not the projection.
    ///
    /// Under [`Mode::Own`] the projection has just increfed what it produced,
    /// so the alias carries a count and the base may die here: this flushes,
    /// exactly as it always did.
    ///
    /// Under [`Mode::Borrow`] it must not. `"[${p.a}][${p.b}]"` is the shape
    /// and it was a wrong answer rather than a crash: the last mention of `p`
    /// is `p.b`, so flushing here dropped the pair — and with it the two string
    /// blocks its fields named — *before* the `str.concat` chain that reads
    /// them ran, and `malloc` handed the freed block straight back to the next
    /// concatenation. `match (xs[i]) { .. }` is the same bug one construct
    /// over, and there it was a segmentation fault. Leaving the drop pending
    /// hands it to the enclosing construct, which flushes after its own code:
    /// [`Scan::children`] after every sibling has been read, [`Scan::match_`]
    /// after the arms.
    ///
    /// This is the same deferral, for the same reason, that
    /// [`Scan::children`]'s doc comment describes for a bare `Local` handed to
    /// a borrowing construct. A projection of one is no less an alias than the
    /// local itself, and it took three shapes to notice.
    fn project(&mut self, id: NodeId, mode: Mode) {
        if mode == Mode::Own {
            self.flush(id);
        }
    }

    /// The count a projection takes, and the temporary it takes it out of.
    ///
    /// Two cases, and only the second is new.
    ///
    /// * A base this function can still **name** — a local, or a projection
    ///   chain down to one — is dropped by whoever owns that name, and
    ///   [`Scan::project`] is what decides when. The projection's own value is
    ///   an alias, so it needs a count only where the parent takes it.
    ///
    /// * A base that is **fresh** has no name and no owner: it is a value this
    ///   expression built, holding counts it took on its way out of whatever
    ///   built it, and the words this projection copies out of it are the only
    ///   part of it anyone goes on to read. Nothing else can reach it, so it is
    ///   released *here* — after the field it hands on has been increfed, so
    ///   the count never touches zero in between. What the projection then
    ///   holds is an owned reference with no name, which is what a temporary
    ///   is, and [`fresh`] says so: a borrowing parent drops it through
    ///   [`Scan::drop_temporary`] and an owning one takes it, exactly as for
    ///   any other temporary.
    ///
    /// Without the release, **`crypto.sha256Text(ctx, "x").0` leaked the
    /// digest's `[U8]` block.** A struct is a stack value in both backends
    /// (this module's header, on reuse), so there was nothing to free the
    /// aggregate; its one counted field was handed on carrying the count the
    /// call had taken for it, and no site anywhere named the temporary the
    /// field came out of. It is `crypto/sha256.buri`'s five live blocks, and
    /// `a_projection_of_a_temporary_releases_it` is the plan.
    ///
    /// **Not a premature free.** The release only ever fires on a base
    /// [`fresh`] answers for — a construction or a call result, never a
    /// [`ExprKind::Local`] or an alias of one — so no name is left pointing at
    /// what it frees, and the one field that outlives it was increfed on the
    /// line above.
    /// Scans a scope that runs later and elsewhere — a lambda's body, under
    /// [`Options::sharing`]. The marks it finds belong to this function's
    /// plan; its liveness and its pending releases do not touch the outer
    /// scan's, which is why all four cursors are swapped out around it.
    ///
    /// An empty liveness is the honest start: nothing in the enclosing
    /// function runs after the lambda's body, because the body does not run
    /// where it is written.
    fn nested(&mut self, body: &Expr, id: NodeId) {
        let pending = std::mem::take(&mut self.pending);
        let jumps = std::mem::take(&mut self.jumps);
        let floor = std::mem::replace(&mut self.floor, 0);
        let diverged = std::mem::replace(&mut self.diverged, false);
        self.expr(body, id, &Live::default(), Mode::Own);
        self.pending = pending;
        self.jumps = jumps;
        self.floor = floor;
        self.diverged = diverged;
    }

    fn projected(
        &mut self,
        e: &Expr,
        base: &Expr,
        id: NodeId,
        bid: NodeId,
        mode: Mode,
        live: &Live,
    ) {
        let takes = fresh(base);
        if (mode == Mode::Own || takes) && self.counted_ty(&e.ty.clone()) {
            self.push(id, Position::After, RcOp::IncRef, Target::Node(id));
            // Perceus's drop specialisation, with the answer deferred: a field
            // read out of a parent this expression is the last use of is a
            // second reference only if the parent was one.
            if self.opts.sharing {
                if let Some(root) = borrowed_root(base) {
                    if self.owned.contains(&root) && !live.contains(&root) {
                        self.inherits.push((id, root));
                    }
                }
            }
        }
        if takes && self.counted_ty(&base.ty.clone()) {
            self.push(id, Position::After, RcOp::DecRef, Target::Node(bid));
        }
        self.project(id, mode);
    }

    /// The id of the `k`th child of the node at `id`, from the subtree sizes —
    /// the same preorder numbering `walk` assigns, so a site recorded here and
    /// a site read there name the same node.
    ///
    /// A lookup, not a walk. A preorder array records a subtree's size and not
    /// where its siblings start, so finding the `k`th child by walking is
    /// `O(k)` — and every caller asks for `k = 0, 1, 2, …` in turn, which makes
    /// the pass quadratic in the width of the widest node. A `match` is as wide
    /// as its enum: `wide-match/20k` is one 10,000-arm match and that was 50 M
    /// steps of the walk. [`child_index`] pays for all of them once.
    ///
    /// Out of range falls back to the walk, which is what the array cannot
    /// answer and what the walk used to do.
    fn child(&self, id: NodeId, k: usize) -> NodeId {
        let first = self.child_at.get(id.0 as usize).copied().unwrap_or(0) as usize;
        let end = self.child_at.get(id.0 as usize + 1).copied().unwrap_or(0) as usize;
        if let Some(found) = first.checked_add(k).filter(|i| *i < end) {
            if let Some(c) = self.child_ids.get(found) {
                return NodeId(*c);
            }
        }
        let mut cur = id.0 + 1;
        for _ in 0..k {
            cur += self.sizes.get(cur as usize).copied().unwrap_or(1);
        }
        NodeId(cur)
    }

    /// Drops, at the entry of a branch, every owned local the branch does not
    /// use but a sibling does. This is what makes the counts agree at a join
    /// without a merge block to put anything in.
    fn balance(&mut self, node: NodeId, mine: &Live, theirs: &Live) {
        let mut extra: Vec<LocalId> = theirs.difference(mine).copied().collect();
        extra.sort_by_key(|l| l.0);
        for l in extra {
            if self.owned.contains(&l) {
                self.push(node, Position::Before, RcOp::DecRef, Target::Local(l));
            }
        }
    }

    /// Scans one expression, given what is live *after* it, and answers what is
    /// live before it. Emits every site the expression itself needs.
    ///
    /// The deferred drops this expression raises are its own: [`Scan::floor`]
    /// is what stops a construct nested inside it from flushing a drop a
    /// sibling raised, and it is restored so the parent can still flush both.
    fn expr(&mut self, e: &Expr, id: NodeId, live: &Live, mode: Mode) -> Live {
        let outer = std::mem::replace(&mut self.floor, self.pending.len());
        let out = self.expr_at(e, id, live, mode);
        self.floor = outer;
        out
    }

    fn expr_at(&mut self, e: &Expr, id: NodeId, live: &Live, mode: Mode) -> Live {
        match &e.kind {
            ExprKind::Local(l) => {
                let mut before = live.clone();
                self.used.insert(*l);
                if !self.is_counted(*l) {
                    return before;
                }
                let last = !live.contains(l);
                if mode == Mode::Own {
                    // Either the binding is still needed after this — so the
                    // count this use takes has to be a new one — or nothing
                    // here owns a count to give away in the first place.
                    if !last || !self.owned.contains(l) {
                        self.push(id, Position::After, RcOp::IncRef, Target::Local(*l));
                    }
                } else if last && self.owned.contains(l) {
                    // Read for the last time and owned by this function: the
                    // drop belongs after whatever is reading it.
                    self.pending.push(*l);
                }
                before.insert(*l);
                before
            }
            ExprKind::Lambda { captures, body, .. } => {
                // An environment is a construction over the captures, and the
                // body does not run here.
                let mut before = live.clone();
                for c in captures {
                    self.used.insert(*c);
                    if self.is_counted(*c) {
                        let last = !before.contains(c);
                        if !last || !self.owned.contains(c) {
                            self.push(id, Position::After, RcOp::IncRef, Target::Local(*c));
                        }
                        before.insert(*c);
                    }
                }
                // Under `sharing` the body is scanned here, because nothing
                // lifts it into a function with a plan of its own. It is a
                // scope that runs later and elsewhere, so it starts from an
                // empty liveness and leaves this scan's own bookkeeping alone.
                if self.opts.sharing {
                    let bid = self.child(id, 0);
                    self.nested(body, bid);
                }
                before
            }
            ExprKind::Block { stmts, tail } => {
                let mut live_after = live.clone();
                let children = stmts.len() + usize::from(tail.is_some());
                // Backwards: the tail first, then each statement, so a
                // binding's last use is known before its binder is reached.
                if let Some(t) = tail {
                    let tid = self.child(id, children.saturating_sub(1));
                    live_after = self.expr(t, tid, &live_after, mode);
                    self.flush(tid);
                }
                for (k, s) in stmts.iter().enumerate().rev() {
                    let sid = self.child(id, k);
                    match s {
                        Stmt::Let { pattern, value, .. } => {
                            let mut bound: Vec<LocalId> = Vec::new();
                            pattern.binds(&mut bound);
                            bound.sort_by_key(|l| l.0);
                            // Bound and never read: dropped where it was bound
                            // rather than at the end. Which ones those are has
                            // to be decided *here*, because the liveness the
                            // initializer is scanned against is the one with
                            // the binding already removed — but the drops
                            // themselves are pushed below, after that scan.
                            let mut unread: Vec<LocalId> = Vec::new();
                            for b in &bound {
                                if self.is_counted(*b) {
                                    self.owned.insert(*b);
                                    if !live_after.contains(b) {
                                        unread.push(*b);
                                    }
                                    live_after.remove(b);
                                }
                            }
                            live_after = self.expr(value, sid, &live_after, Mode::Own);
                            // **After the initializer, and the ordering is the
                            // whole of it.** The initializer is scanned at
                            // *this* node, so its own operations land at
                            // `(sid, After)` too, and sites at one key run in
                            // the order they were pushed ([`FuncPlan::at`], and
                            // [`analyze`]'s sort is stable). Every shape that
                            // gives the binding a count puts an `incref`
                            // there — a projection out of an aggregate
                            // ([`Scan::projected`]), a second read of a local
                            // something after this still uses — so pushing the
                            // drop first made the pair *release then retain*.
                            //
                            // On a value whose count was one that is a
                            // use-after-free rather than a leak, and the counts
                            // balance either way, which is why
                            // [`check_balance`] never saw it: the release frees
                            // the block, the retain writes through a header the
                            // block cache is using as free-list storage, and
                            // the crash is an unrelated allocation later
                            // (`reports/llvm-parallel-listen-fix.md`). This is
                            // the order `Stmt::Expr` below always had.
                            for b in unread {
                                self.push(sid, Position::After, RcOp::DecRef, Target::Local(b));
                            }
                            self.flush(sid);
                        }
                        Stmt::Expr(x) => {
                            live_after = self.expr(x, sid, &live_after, Mode::Borrow);
                            self.drop_temporary(x, sid, sid);
                            self.flush(sid);
                        }
                    }
                }
                live_after
            }
            ExprKind::If { cond, then, else_ } => {
                let then_id = self.child(id, 1);
                let else_id = self.child(id, 2);
                let ((lt, le), jumps, diverged) = self.scoped(|me| {
                    let lt = me.expr(then, then_id, live, mode);
                    me.flush(then_id);
                    let le = me.expr(else_, else_id, live, mode);
                    me.flush(else_id);
                    (lt, le)
                });
                self.balance(then_id, &lt, &le);
                self.balance(else_id, &le, &lt);
                let union: Live = lt.union(&le).copied().collect();
                let cid = self.child(id, 0);
                let out = self.expr(cond, cid, &union, Mode::Borrow);
                // What the condition read for the last time is dropped after
                // the branches, not between the test and the jump — unless
                // both branches jump, in which case "after" never runs and the
                // drop belongs before each jump instead.
                if diverged && !jumps.is_empty() {
                    self.flush_at(&jumps);
                    self.diverged = true;
                } else {
                    self.flush(id);
                }
                out
            }
            ExprKind::Match { .. } => self.match_(e, id, live, mode),
            // A loop is always a whole function body, and its entries are
            // alternative starting points chosen by the caller — so they are
            // branches, and they are balanced against each other exactly as a
            // `match`'s arms are. The loop's variables are the function's
            // parameters, which is why nothing extra is owned here.
            ExprKind::Loop { entries } => {
                let mut befores: Vec<Live> = Vec::new();
                let mut ids: Vec<NodeId> = Vec::new();
                let (_, jumps, diverged) = self.scoped(|me| {
                    for (k, entry) in entries.iter().enumerate() {
                        let eid = me.child(id, k);
                        let lb = me.expr(entry, eid, live, mode);
                        me.flush(eid);
                        befores.push(lb);
                        ids.push(eid);
                    }
                });
                let union: Live = befores.iter().flat_map(|b| b.iter().copied()).collect();
                for (b, eid) in befores.iter().zip(ids.iter()) {
                    self.balance(*eid, b, &union);
                }
                if diverged && !jumps.is_empty() {
                    self.diverged = true;
                }
                union
            }
            // A jump: the values go into the loop's variables and nothing
            // after it runs. So the arguments are scanned against an *empty*
            // liveness — which is what makes a value built in this iteration
            // and not carried through it a last use, and therefore a drop
            // before the back edge rather than a block leaked per iteration.
            ExprKind::Continue { func, args, .. } => {
                let row: Vec<ir::Ownership> = match func {
                    Some(f) => {
                        self.ownership.get(f.index()).cloned().unwrap_or_default()
                    }
                    // Back into the enclosing loop, whose variables are this
                    // function's own parameters.
                    None => self.self_params.clone(),
                };
                let key = jump_key(id, args, self);
                let mut after = Live::default();
                for (k, arg) in args.iter().enumerate().rev() {
                    let aid = self.child(id, k);
                    let m = match row.get(k) {
                        Some(ir::Ownership::Borrow) => Mode::Borrow,
                        _ => Mode::Own,
                    };
                    after = self.expr(arg, aid, &after, m);
                    if m == Mode::Borrow {
                        self.drop_temporary(arg, aid, key.0);
                    }
                }
                // Everything this iteration still holds and is not passing on
                // dies here; the drops go before the jump, because after it is
                // the next iteration's header.
                self.flush_at(&[key]);
                self.jumps.push(key);
                self.diverged = true;
                after
            }
            ExprKind::And { lhs, rhs } | ExprKind::Or { lhs, rhs } => {
                self.short_circuit(id, lhs, rhs, live)
            }
            ExprKind::Coalesce { lhs, rhs, .. } => self.short_circuit(id, lhs, rhs, live),
            ExprKind::Try { base, .. } => {
                // An early exit leaves the function, so nothing after it runs.
                // Scanning the operand in the enclosing liveness is what keeps
                // a drop off a path that never reached it.
                let bid = self.child(id, 0);
                let out = self.expr(base, bid, live, mode);
                self.flush(id);
                out
            }
            // A projection reads its base without taking it, which is the whole
            // of borrowing at the tree level. What it *produces* is a reference
            // the parent still owns, so taking it needs a count of its own —
            // and where the base is a temporary, the base dies here.
            // [`Scan::projected`] is both halves.
            ExprKind::Field { base, .. }
            | ExprKind::TupleIndex { base, .. }
            | ExprKind::CtxGet { base, .. } => {
                let bid = self.child(id, 0);
                let out = self.expr(base, bid, live, Mode::Borrow);
                self.projected(e, base, id, bid, mode, live);
                out
            }
            ExprKind::Index { base, index, .. } => {
                let iid = self.child(id, 1);
                let after = self.expr(index, iid, live, Mode::Borrow);
                let bid = self.child(id, 0);
                let out = self.expr(base, bid, &after, Mode::Borrow);
                self.projected(e, base, id, bid, mode, live);
                out
            }
            // A functional update **consumes** its base: what comes out is a
            // new value and the old one has no reader left. The projections
            // the update reads out of the base keep it live across its own
            // siblings, so the generic scan sees the base read as a
            // duplication — true of a count, false of a reference. Under
            // `sharing` the base is scanned last and borrowed, which is what
            // makes `S { ..s, xs: s.xs.push(x) }` write through in a loop.
            ExprKind::StructUpdate { base, updates, .. }
                if self.opts.sharing && dies_here(base, &self.owned, live) =>
            {
                let mut after = live.clone();
                for (k, (_, value)) in updates.iter().enumerate().rev() {
                    let kid = self.child(id, k + 1);
                    after = self.expr(value, kid, &after, Mode::Own);
                }
                let bid = self.child(id, 0);
                let after = self.expr(base, bid, &after, Mode::Borrow);
                self.flush(id);
                after
            }
            _ => self.children(e, id, live),
        }
    }

    /// A `match`, which is where ownership of a *payload* is decided.
    ///
    /// If the scrutinee is a local whose last use this is, the match consumes
    /// it: the payloads it binds are increfed out of the value and the value
    /// itself is dropped, which is MEMORY.md §5.3's dying value and the moment
    /// reuse replaces the drop with a write.
    fn match_(&mut self, e: &Expr, id: NodeId, live: &Live, mode: Mode) -> Live {
        let ExprKind::Match { scrutinee, arms } = &e.kind else { return live.clone() };
        let token = match &scrutinee.kind {
            ExprKind::Local(l)
                if self.is_counted(*l) && self.owned.contains(l) && !live.contains(l) =>
            {
                Some(*l)
            }
            _ => None,
        };
        let owns = token.is_some();
        let mut befores: Vec<Live> = Vec::new();
        let mut ids: Vec<NodeId> = Vec::new();
        let outer_jumps = std::mem::take(&mut self.jumps);
        let outer_diverged = std::mem::replace(&mut self.diverged, true);
        let mut k = 1usize;
        for a in arms {
            let gid = a.guard.as_ref().map(|_| {
                let g = self.child(id, k);
                k += 1;
                g
            });
            let bid = self.child(id, k);
            k += 1;
            let mut bound: Vec<LocalId> = Vec::new();
            a.pattern.binds(&mut bound);
            bound.sort_by_key(|l| l.0);
            // A `..rest` binding is a block this arm allocates, not a piece of
            // the scrutinee: both backends' `ArraySlice` calls the allocator,
            // copies and retains every element. So the arm owns it however the
            // scrutinee is held, and it takes no count out of the scrutinee.
            let mut fresh_bound: Vec<LocalId> = Vec::new();
            a.pattern.fresh_binds(&mut fresh_bound);
            fresh_bound.retain(|b| self.is_counted(*b));
            fresh_bound.sort_by_key(|l| l.0);
            for b in &fresh_bound {
                self.owned.insert(*b);
            }
            if owns {
                for b in &bound {
                    if self.is_counted(*b) {
                        self.owned.insert(*b);
                    }
                }
            }
            let before_arm = std::mem::replace(&mut self.diverged, false);
            let mut lb = self.expr(&a.body, bid, live, mode);
            self.flush(bid);
            // Every arm has to jump for the match to.
            self.diverged = before_arm && self.diverged;
            if let (Some(g), Some(gid)) = (a.guard.as_ref(), gid) {
                lb = self.expr(g, gid, &lb, Mode::Borrow);
                self.flush(gid);
            }
            // A fresh binding the arm never reads is dropped where it is bound,
            // for the reason `Stmt::Let` drops one: the allocation happened
            // whether or not a use for it did.
            for b in &fresh_bound {
                if !lb.contains(b) {
                    self.push(bid, Position::Before, RcOp::DecRef, Target::Local(*b));
                }
            }
            if let Some(t) = token {
                let used: Vec<LocalId> = bound
                    .iter()
                    .copied()
                    .filter(|b| lb.contains(b) && !fresh_bound.contains(b))
                    .collect();
                for b in used {
                    if self.is_counted(b) {
                        self.push(bid, Position::Before, RcOp::IncRef, Target::Local(b));
                    }
                }
                // The entry drop is for an arm that is *done* with the
                // scrutinee. An arm that still reads it — `assert.some`'s
                // `.None => failExpected("some", o)` — has already had its own
                // last-use drop placed after the read, and a second one here
                // would take the count to zero before the arm ran.
                if !lb.contains(&t) {
                    self.push(bid, Position::Before, RcOp::DecRef, Target::Local(t));
                    if self.opts.reuse {
                        if let Some(fields) = construction(&a.body) {
                            self.reuse.push(Reuse { token: t, at: bid, fields });
                        }
                    }
                }
            }
            for b in &bound {
                lb.remove(b);
            }
            befores.push(lb);
            ids.push(bid);
        }
        let arm_jumps = std::mem::replace(&mut self.jumps, outer_jumps);
        self.jumps.extend(arm_jumps.iter().copied());
        let arms_diverged = std::mem::replace(&mut self.diverged, outer_diverged);
        let mut union: Live = befores.iter().flat_map(|b| b.iter().copied()).collect();
        // A consumed scrutinee is disposed of by every arm — at the entry where
        // the arm does not read it, at its own last use where it does — so it
        // is out of the balancing question entirely. Leaving it in made
        // `balance` add, to each arm that was done with it, the drop that arm
        // had already been given: two decrements against one count.
        if let Some(t) = token {
            union.remove(&t);
        }
        let pairs: Vec<(NodeId, Live)> =
            ids.iter().copied().zip(befores).collect();
        for (bid, b) in &pairs {
            self.balance(*bid, b, &union);
        }
        let before = union;
        let sid = self.child(id, 0);
        // A counted compound scrutinee is owned for `Scan::children`'s reason:
        // borrowed, its tail's alias would be dropped inside it, before the
        // arms read the payload.
        let promoted =
            !owns && compound(scrutinee) && self.counted_ty(&scrutinee.ty.clone());
        let smode = if owns || promoted { Mode::Own } else { Mode::Borrow };
        let out = self.expr(scrutinee, sid, &before, smode);
        // A scrutinee read for the last time is dropped after the arms, which
        // are the things reading what it holds — a payload binding points into
        // it, so dropping at an arm's entry would free what the arm is about
        // to read. Where every arm jumps there is no "after the arms", and the
        // last point before each back edge is where it goes instead.
        // Whether the code after the arms is reachable at all.
        let falls_through = !arms_diverged || arm_jumps.is_empty();
        if falls_through {
            self.flush(id);
        } else {
            self.flush_at(&arm_jumps);
            self.diverged = true;
        }
        // A scrutinee the match *built* has no binding and no owner: the arms
        // increfed what they kept out of it (`owns` is false, so every payload
        // use takes a count of its own), and after them nothing names it.
        // `match (q.pop(ctx))` leaked the `Option` and, with it, the lists the
        // queue it answered still pointed at.
        //
        // A path out of the arms either takes a back edge or falls through, and
        // never both — a `Continue` is a tail call, so everything on the path
        // that reaches one ends there. So the drop goes before *every* jump and
        // after the arms, and exactly one of the two runs. Emitting it only
        // after the arms when some arm jumped is what leaked one queue per
        // iteration of `match (q.pop(ctx)) { .Some(..) => drain(..), .None => acc }`:
        // the recursive arm is the back edge, the `.None` arm is the
        // fall-through, and only the second was disposing of the `Option`.
        if !owns && (fresh(scrutinee) || promoted) && self.counted_ty(&scrutinee.ty.clone()) {
            for (node, at) in &arm_jumps {
                self.push(*node, *at, RcOp::DecRef, Target::Node(sid));
            }
            if falls_through {
                self.push(id, Position::After, RcOp::DecRef, Target::Node(sid));
            }
        }
        // Deliberately *not* removed from `out`: the scrutinee is read here,
        // so it is live before this expression however the arms dispose of it.
        // Removing it made a second consuming `match` on the same local — a
        // shape the standard library does not have and a program can — look
        // like a first use, so both matches emitted a drop.
        out
    }

    /// `&&`, `||` and `??`: the right operand may not run at all, so a local
    /// whose last use is inside it is kept alive across the whole expression
    /// and dropped after it. One extra pair of operations on the taken path,
    /// and a correct count on the skipped one.
    fn short_circuit(&mut self, id: NodeId, lhs: &Expr, rhs: &Expr, live: &Live) -> Live {
        let rid = self.child(id, 1);
        // Which owned locals die inside the operand — [`Scan::names_in`], not a
        // scan. The right operand used to be scanned **twice**: once as a probe
        // whose sites were thrown away, and once in the liveness that keeping
        // the probe's answer alive produces. A probe re-entered `expr`, which
        // re-entered `short_circuit` for a nested `&&`, so a chain of *n* of
        // them cost 2ⁿ scans — `middle/derives.rs`'s `eq_fields` builds exactly
        // that chain, right-nested, one link per field, and it is why
        // `proto/binary.buri` took minutes to compile.
        //
        // The probe answered one question and its whole answer is `deferred`
        // below: `(probe \ live) ∩ owned`. A local is in `probe` and not in
        // `live` exactly when the operand *names* it and does not bind it
        // itself, which is a syntactic property of the subtree and needs no
        // liveness at all — see [`Scan::names_in`] for why the two agree.
        let mut deferred: Vec<LocalId> = self.names_in(rhs, rid);
        deferred.retain(|l| !live.contains(l) && self.owned.contains(l));
        let mut kept: Live = live.clone();
        kept.extend(deferred.iter().copied());
        let after_rhs = self.expr(rhs, rid, &kept, Mode::Borrow);
        self.flush(rid);
        for l in &deferred {
            self.push(id, Position::After, RcOp::DecRef, Target::Local(*l));
        }
        let lid = self.child(id, 0);
        let out = self.expr(lhs, lid, &after_rhs, Mode::Borrow);
        self.flush(id);
        out
    }

    /// Every local a subtree **names**: mentioned somewhere inside it and not
    /// bound inside it either. Sorted, and memoised on the node.
    ///
    /// # Why this is the same answer the probe scan gave
    ///
    /// [`Scan::short_circuit`] wants `(probe \ live) ∩ owned`, where `probe` is
    /// what [`Scan::expr`] answers for the operand against the enclosing
    /// liveness. Every construct's transfer function is one of two shapes:
    /// `live_in = live_out ∪ G` for something that can fall through, and
    /// `live_in = G` for something every path out of which is a `Continue`
    /// (which scans its arguments against an *empty* liveness on purpose). `G`
    /// is the same set either way, so **`probe \ live` is `G \ live`** and the
    /// enclosing liveness never enters the answer. `G` is what this computes:
    ///
    /// * [`ExprKind::Local`] contributes itself; a local the layout classifier does
    ///   not count is filtered out by `owned` at the caller, which only ever
    ///   holds counted locals.
    /// * [`ExprKind::Lambda`] contributes its **captures and not its body**,
    ///   exactly as `expr_at` does — the body does not run here.
    /// * a `let` pattern and a `match` arm's pattern bind inside, and `expr_at`
    ///   removes what they bind from the set on the way out, so this does too.
    ///   Local ids are unique per function, so subtracting them at the end is
    ///   the same as scoping them.
    /// * a `match`'s consumed scrutinee is removed from the arms' union and put
    ///   straight back by the scan of the scrutinee itself (which is the local),
    ///   so it stays named.
    ///
    /// The one shape this would answer more generously than the scan is a
    /// **condition or scrutinee position that diverges** — `if (continue) {…}`
    /// — where `expr_at` throws the branches' sets away because the condition is
    /// scanned last and answers `G_cond` alone. No such tree exists: a condition
    /// and a scrutinee are not tail positions, so `tail_calls` never puts a
    /// `Continue` in one. A right operand *is* a tail position, and that case is
    /// covered: `expr(lhs, G_rhs)` still keeps `G_rhs`.
    ///
    /// # Why it is memoised
    ///
    /// The chains this exists for are right-nested (`middle/derives.rs`'s
    /// `eq_fields` says so in its own doc comment), so asking each link about
    /// the whole tail below it is quadratic on its own. A chain's tail is a
    /// short-circuit operand too, so caching exactly those nodes makes each link
    /// pay for its own left operand and nothing else.
    ///
    /// The cache holds the *unfiltered* names deliberately. `owned` grows while
    /// the scan runs — [`Scan::match_`] adds an arm's payload bindings before it
    /// scans that arm — so a set filtered when it was first computed could be
    /// stale by the time it is read again. Filtering at the use site cannot be.
    fn names_in(&mut self, e: &Expr, id: NodeId) -> Vec<LocalId> {
        if let Some(hit) = self.named.get(id.0 as usize).and_then(Option::as_ref) {
            return hit.clone();
        }
        let mut names: Live = Live::default();
        let mut bound: Vec<LocalId> = Vec::new();
        self.collect_names(e, id, &mut names, &mut bound);
        for b in &bound {
            names.remove(b);
        }
        let mut out: Vec<LocalId> = names.into_iter().collect();
        out.sort_by_key(|l| l.0);
        if let Some(slot) = self.named.get_mut(id.0 as usize) {
            *slot = Some(out.clone());
        }
        out
    }

    /// [`Scan::names_in`]'s walk. It descends exactly where [`Scan::expr_at`]
    /// descends, which is where [`kids`] goes with the two exceptions above.
    fn collect_names(&mut self, e: &Expr, id: NodeId, names: &mut Live, bound: &mut Vec<LocalId>) {
        match &e.kind {
            ExprKind::Local(l) => {
                names.insert(*l);
                return;
            }
            // The environment is the captures; the body is a scope of its own
            // and `expr_at` does not scan it here either.
            ExprKind::Lambda { captures, .. } => {
                names.extend(captures.iter().copied());
                return;
            }
            ExprKind::Block { stmts, .. } => {
                for s in stmts {
                    if let Stmt::Let { pattern, .. } = s {
                        pattern.binds(bound);
                    }
                }
            }
            ExprKind::Match { arms, .. } => {
                for a in arms {
                    a.pattern.binds(bound);
                }
            }
            // The nesting this whole function exists for: the tail of a chain
            // is asked once and answered from the cache ever after.
            ExprKind::And { lhs, rhs }
            | ExprKind::Or { lhs, rhs }
            | ExprKind::Coalesce { lhs, rhs, .. } => {
                let lid = self.child(id, 0);
                self.collect_names(lhs, lid, names, bound);
                let rid = self.child(id, 1);
                let tail = self.names_in(rhs, rid);
                names.extend(tail);
                return;
            }
            _ => {}
        }
        for (k, kid) in kids(e).into_iter().enumerate() {
            let kid_id = self.child(id, k);
            self.collect_names(kid, kid_id, names, bound);
        }
    }

    /// The default: every child is evaluated, left to right, and each takes
    /// what the construct's own convention says.
    ///
    /// # The one thing right-to-left scanning gets wrong on its own
    ///
    /// "A child's live-after is everything to its right" is the rule, and it
    /// makes the *rightmost* mention of a local its last use. That is right for
    /// a local whose value is a register, and wrong for a **borrowed local
    /// handed to the construct directly**.
    ///
    /// `f(s, g(s))` is the shape, and a `Str` is what makes it bite. The first
    /// argument is `s` itself: the construct is holding three words copied out
    /// of `s`, with no count of their own, and it goes on holding them while
    /// `g(s)` runs and until `f` is called. But the rightmost mention of `s` is
    /// inside `g`, so a drop placed after *that* frees the block those three
    /// words point into, before `f` ever reads them. `"${s} ${x} ${s.len()}"`
    /// is the same shape — a template is a `str.concat` chain over holes that
    /// are all evaluated first (`lower::template`) — and it was a wrong answer
    /// rather than a crash, because the freed block is usually handed straight
    /// back by `malloc` to the next allocation in the same expression.
    ///
    /// So a bare `Local` child under a borrowing convention is **kept alive
    /// across its siblings** and dropped by this construct instead. That is the
    /// same deferral [`Scan::short_circuit`] makes for the same reason, and one
    /// extra pair of operations is what it costs on the path where the local
    /// really did die inside a sibling.
    ///
    /// **A projection of a local is the same alias one step down**, which is
    /// what [`borrowed_root`] answers: `f(p.b, g(p))` holds two words copied
    /// out of `p`'s block while `g(p)` runs, so `p` is what has to be kept, not
    /// `p.b`, which has no count of its own to keep.
    ///
    /// An *owning* child needs nothing: it increfs what it takes (a
    /// construction's field, an owned parameter), so the alias it holds carries
    /// a count and cannot be freed underneath it.
    fn children(&mut self, e: &Expr, id: NodeId, live: &Live) -> Live {
        let kids = kids(e);
        let modes = child_modes(e, kids.len(), self.ownership);
        let mut kept: Vec<LocalId> = Vec::new();
        for (k, kid) in kids.iter().enumerate() {
            if modes.get(k).copied().unwrap_or(Mode::Borrow) != Mode::Borrow {
                continue;
            }
            let Some(l) = borrowed_root(kid) else { continue };
            if self.is_counted(l)
                && self.owned.contains(&l)
                && !live.contains(&l)
                && !kept.contains(&l)
            {
                kept.push(l);
            }
        }
        kept.sort_by_key(|l| l.0);
        let mut after = live.clone();
        after.extend(kept.iter().copied());
        // Right to left: a child's "live after" is everything the children to
        // its right go on to use, plus whatever `kept` is holding open.
        for (k, kid) in kids.iter().enumerate().rev() {
            let kid_id = self.child(id, k);
            let m = modes.get(k).copied().unwrap_or(Mode::Borrow);
            // A counted compound child is owned, not borrowed: an arm may
            // answer an alias of a local, and a borrowed scan drops that local
            // inside the arm, before this construct reads the value.
            if m == Mode::Borrow && compound(kid) && self.counted_ty(&kid.ty.clone()) {
                after = self.expr(kid, kid_id, &after, Mode::Own);
                self.push(id, Position::After, RcOp::DecRef, Target::Node(kid_id));
                continue;
            }
            after = self.expr(kid, kid_id, &after, m);
            if m == Mode::Borrow {
                self.drop_temporary(kid, kid_id, id);
            }
        }
        for l in kept {
            self.pending.push(l);
        }
        self.flush(id);
        after
    }

    /// A fresh counted value passed to something that only borrows it has
    /// nobody left to drop it. `f(g(x))` where `f` borrows is the shape: what
    /// `g` returned is dropped after `f` returns, and it is named by the node
    /// that produced it because it has no binding.
    fn drop_temporary(&mut self, kid: &Expr, kid_id: NodeId, at: NodeId) {
        if !fresh(kid) {
            return;
        }
        if self.counted_ty(&kid.ty.clone()) {
            self.push(at, Position::After, RcOp::DecRef, Target::Node(kid_id));
        }
    }
}

/// Where the operations that have to happen *before* a back edge go.
///
/// Not the `Continue` node's own `After`: `lower::continue_` sets the jump as
/// the block's terminator and then starts a fresh unreachable block, so an
/// instruction placed after the node would be emitted into a block nothing
/// reaches and dropped with it. The last argument's `After` is the last point
/// that is still in the jumping block and is past every argument's own code; a
/// jump with no arguments has nothing to read, so its `Before` will do.
fn jump_key(id: NodeId, args: &[Expr], scan: &Scan<'_>) -> (NodeId, Position) {
    match args.len() {
        0 => (id, Position::Before),
        n => (scan.child(id, n.saturating_sub(1)), Position::After),
    }
}

/// A block, an `if`, or a `match`: a value that arrives through a tail, whose
/// paths may answer aliases of different locals.
fn compound(e: &Expr) -> bool {
    matches!(e.kind, ExprKind::Block { .. } | ExprKind::If { .. } | ExprKind::Match { .. })
}

/// The local a borrowed child is an alias of: itself where it is one, and the
/// base of a projection chain where it is words copied out of one.
///
/// [`Scan::project`] defers the base's drop to the parent; this is the other
/// half of the same invariant, and it is what keeps the base alive across the
/// siblings the parent evaluates after the projection.
/// Whether an expression is a local this function owns and nothing after it
/// reads: the base of a functional update that is taking it over rather than
/// reading beside it.
fn dies_here(e: &Expr, owned: &HashSet<LocalId>, live: &Live) -> bool {
    match &e.kind {
        ExprKind::Local(l) => owned.contains(l) && !live.contains(l),
        _ => false,
    }
}

fn borrowed_root(e: &Expr) -> Option<LocalId> {
    match &e.kind {
        ExprKind::Local(l) => Some(*l),
        ExprKind::Field { base, .. }
        | ExprKind::TupleIndex { base, .. }
        | ExprKind::CtxGet { base, .. }
        | ExprKind::Index { base, .. } => borrowed_root(base),
        _ => None,
    }
}

/// Whether an expression produces a *new* reference rather than another name
/// for one that already exists.
///
/// Asked of every tail, because `middle::inline` replaces a call with the
/// callee's **body** and a body is a `Block`. `"[${show(ctx, xs)}]"` is the
/// shape: `show` is one call, so the inliner pastes it in, the hole stops being
/// an `ExprKind::CallFn`, and the string it returns had nobody left to drop it.
/// All of them, so that a branch answering a borrowed alias is not dropped on
/// the strength of a branch beside it that allocates.
fn fresh(e: &Expr) -> bool {
    let tails = tails(e);
    !tails.is_empty() && tails.into_iter().all(fresh_leaf)
}

fn fresh_leaf(e: &Expr) -> bool {
    // A projection is an alias, so it is a new reference exactly when its base
    // is a temporary: [`Scan::projected`] increfs the field and releases the
    // base there, and what comes out is an owned reference with no name.
    // `mk().a.b` is the same statement twice, which is why this recurses.
    if let ExprKind::Field { base, .. }
    | ExprKind::TupleIndex { base, .. }
    | ExprKind::CtxGet { base, .. }
    | ExprKind::Index { base, .. } = &e.kind
    {
        return fresh(base);
    }
    matches!(
        e.kind,
        ExprKind::CallFn { .. }
            | ExprKind::CallValue { .. }
            | ExprKind::CallTrait { .. }
            | ExprKind::Intrinsic { .. }
            | ExprKind::StructLit { .. }
            | ExprKind::StructUpdate { .. }
            | ExprKind::EnumLit { .. }
            | ExprKind::Tuple(_)
            | ExprKind::Array(_)
            | ExprKind::Template { .. }
            | ExprKind::Lambda { .. }
            | ExprKind::Closure { .. }
            | ExprKind::CtxLit { .. }
            | ExprKind::Str(_)
    )
}

/// What each child of a construct takes.
///
/// A construction and an owning parameter take; a projection, a primitive
/// operation, a condition and a runtime intrinsic read. The intrinsic row is
/// the runtime ABI this module's header states.
fn child_modes(e: &Expr, n: usize, own: &[Vec<ir::Ownership>]) -> Vec<Mode> {
    match &e.kind {
        ExprKind::StructLit { .. }
        | ExprKind::EnumLit { .. }
        | ExprKind::Tuple(_)
        | ExprKind::Array(_)
        | ExprKind::CtxLit { .. }
        | ExprKind::StructUpdate { .. }
        // A closure environment is a construction over what it captured, which
        // is what `middle::closures` turned a `Lambda` into.
        | ExprKind::Closure { .. } => vec![Mode::Own; n],
        // A code pointer cannot carry a per-callee convention, so an indirect
        // call owns what it is *passed* — and borrows what it is *reached
        // through*.
        //
        // [`kids`] puts the callee first and the arguments after it, so the
        // first mode is the closure value's and the rest are the arguments'.
        // [`ir::Inst::CallIndirect`] is "load `code` and `env`, then
        // `call_indirect`": the environment pointer is handed to the code and
        // neither half of it is released there. What frees an environment is
        // the closure *value*'s own drop — `stencil::glue::Helper::EnvGlue`
        // — so an `Own` here was a release
        // nothing performed, and every closure that was ever called leaked its
        // environment.
        //
        // [`collect_consuming`] has always said this: its `CallValue` arm
        // consumes `args` and not `callee`, so ownership inference already
        // treated a parameter that is only *called* as borrowed. The two
        // functions describe one convention and now agree.
        //
        // `Borrow` on the callee is also what puts the drop back on a *fresh*
        // one: `(fn(x) => x + n)(3)`, which `middle::inline` produces whenever
        // it pastes a one-call higher-order function into its caller, is a
        // borrowing child, so [`Scan::drop_temporary`] drops it after the call.
        ExprKind::CallValue { .. } => (0..n)
            .map(|k| if k == 0 { Mode::Borrow } else { Mode::Own })
            .collect(),
        ExprKind::CallFn { func, .. } => {
            let row = func.func().and_then(|f| own.get(f.index()));
            (0..n)
                .map(|k| match row.and_then(|r| r.get(k)) {
                    Some(ir::Ownership::Borrow) => Mode::Borrow,
                    _ => Mode::Own,
                })
                .collect()
        }
        _ => vec![Mode::Borrow; n],
    }
}

/// Whether an arm's result is a construction, and how many fields it writes —
/// the half of MEMORY.md §5.3's reuse condition that is visible in the tree.
/// The size class is `lower`'s to compare, from the layout table.
fn construction(body: &Expr) -> Option<usize> {
    match &body.kind {
        ExprKind::StructLit { fields, .. } => Some(fields.len()),
        ExprKind::EnumLit { args, .. } => Some(args.len()),
        ExprKind::Tuple(items) | ExprKind::Array(items) => Some(items.len()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compiler::middle::monomorphize;
    use crate::diagnostics::{Diagnostics, SourceMap, Span};

    fn compile(src: &str) -> Program {
        let mut map = SourceMap::new();
        let analysis = crate::compiler::driver::analyze_snippet(
            &mut map,
            "rc_test.buri",
            src,
            crate::compiler::modules::Role::Entry,
        );
        let errors: Vec<String> = analysis
            .diagnostics
            .items
            .iter()
            .filter(|d| d.is_error())
            .map(|d| d.message.clone())
            .collect();
        assert!(errors.is_empty(), "the snippet did not compile: {errors:?}");
        let entry = analysis.checked.entry.expect("the snippet exports `main`");
        let mut diags = Diagnostics::new();
        let paths: Vec<String> = analysis.loaded.modules.iter().map(|m| m.path.clone()).collect();
        // Deliberately *not* through `middle::run`: the inliner pastes a
        // one-call function into its caller and dead-code elimination then
        // takes the original away, so a test about a two-line function would
        // be a test about a function that is no longer there. This pass reads
        // the tree, and the tree is the same shape either way — the balance
        // tests below run over every function of the standard library that the
        // snippet reaches, which is where the interesting shapes come from.
        monomorphize::run(&analysis.checked, paths, &mut diags, monomorphize::Roots::Main(entry))
    }

    fn find(program: &Program, name: &str) -> FuncIdx {
        let i = program
            .funcs
            .iter()
            .position(|f| f.debug_name.ends_with(name))
            .unwrap_or_else(|| panic!("no function named {name}"));
        FuncIdx(i as u32)
    }

    // -- the escape question ------------------------------------------------

    /// **A program that can reach a task boundary is marked, and one that
    /// cannot is not.**
    ///
    /// [`crosses_tasks`]'s two directions. The positive half is easy and the
    /// negative half is the one worth having: this answer puts a whole program
    /// on atomic reference counting, so a `true` reached by accident — an
    /// import that pulls `core/tasks` in without calling it, a key spelled by
    /// prefix that matches something else — is a cost every program pays.
    ///
    /// The question is asked of the **post-monomorphization** program, so it
    /// is reachability and not mention: a `Tasks` binding the entry never
    /// calls through is not a function in `program.funcs`.
    #[test]
    fn only_a_program_that_can_reach_a_task_boundary_is_marked() {
        let plain = run(&compile(
            r#"
from "core/effect" import { Alloc, Stdout };
from "core/host" import * as host;
from "core/io" import * as io;
from "core/str" import * as str;

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout };
  let _ = io.println(ctx, str.format(ctx, "${1 + 1}")).ignore();
  .Ok(())
}
"#,
        ));
        assert!(
            !plain.crosses_tasks,
            "a program with no task boundary in it was put on atomic counting"
        );

        let program = compile(
            r#"
from "core/effect" import { Alloc, Stdout, Tasks };
from "core/host" import * as host;
from "core/io" import * as io;
from "core/str" import * as str;
from "core/tasks" import * as tasks;

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout, Tasks: host.tasks };
  let doubled = tasks.parallel(ctx, [1, 2, 3], fn(c, i, n) => n * 2);
  let _ = io.println(ctx, str.format(ctx, "${doubled.len()}")).ignore();
  .Ok(())
}
"#,
        );
        assert!(run(&program).crosses_tasks, "a program that fans out was not marked");
    }

    /// The key list is a **prefix**, and that is the direction an omission has
    /// to be wrong in.
    ///
    /// `crosses_tasks` is what decides whether a whole program's blocks are
    /// counted atomically, and a key missing from it is a value the program is
    /// promised nobody else can see — a promise kept by non-atomic counts on
    /// both backends. So the surface is spelled once, and every row track F
    /// adds to `host.HostTasks` is covered on the day it lands rather than on
    /// the day somebody remembers.
    #[test]
    fn every_task_host_key_crosses_and_nothing_else_does() {
        assert!(crosses_tasks("host.HostTasks.parallel"));
        // The rows that do not exist yet, and are covered anyway.
        assert!(crosses_tasks("host.HostTasks.start"));
        assert!(crosses_tasks("host.HostTasks.send"));

        // Everything that waits but hands nothing over: `suspends` and
        // `crosses_tasks` are different questions about the same list, and
        // `Tasks.parallel` is the one key on both.
        for key in [
            "host.HostFs.readText",
            "host.HostNet.fetch",
            "host.HostClock.sleepMillis",
            "host.HostStdin.readLine",
            "host.HostStdout.println",
            "testing_context.TestTasks.parallel",
        ] {
            assert!(!crosses_tasks(key), "{key} put its program on atomic counting");
        }
        assert!(suspends("host.HostTasks.parallel") && crosses_tasks("host.HostTasks.parallel"));
    }

    // -- the balance checker ------------------------------------------------

    /// Replays a function's plan along **every** path and asserts the counts
    /// balance: every owned local ends at zero, no count ever goes negative,
    /// and every branch of a join agrees.
    ///
    /// This walks forward, in evaluation order, which is the opposite of the
    /// direction the analysis works in — so it is a check and not a rerun.
    struct Balance<'a> {
        func: &'a Func,
        plan: &'a FuncPlan,
        sizes: Vec<u32>,
        counted: Syntactic,
        own: &'a [Vec<ir::Ownership>],
        /// A node whose `After` operations are held back, because the binding
        /// they belong to does not exist until the value has been computed.
        suppress: Option<NodeId>,
        /// What the state is when the function — and therefore the loop header
        /// — is entered: one count per owned counted parameter.
        header: State,
        /// Arm bodies whose pattern allocated before they were entered, and
        /// what it bound. `..rest` is the only such binding (VALUE-MODEL.md
        /// §4.2): the count exists because the pattern called the allocator,
        /// so there is no site to read it from and the replay has to know.
        fresh_at: HashMap<NodeId, Vec<LocalId>>,
        errors: Vec<String>,
    }

    #[derive(Clone, PartialEq, Eq, Debug, Default)]
    struct State {
        locals: Vec<(LocalId, i32)>,
        temps: Vec<(NodeId, i32)>,
        /// Every path out of the code just replayed was a jump, so this state
        /// never reaches the join it would otherwise be compared at.
        diverged: bool,
    }

    impl State {
        fn bump(&mut self, l: LocalId, by: i32) {
            match self.locals.iter_mut().find(|(k, _)| *k == l) {
                Some((_, v)) => *v += by,
                None => self.locals.push((l, by)),
            }
        }

        fn bump_temp(&mut self, n: NodeId, by: i32) {
            match self.temps.iter_mut().find(|(k, _)| *k == n) {
                Some((_, v)) => *v += by,
                None => self.temps.push((n, by)),
            }
        }

        fn normalize(&mut self) {
            self.locals.retain(|(_, v)| *v != 0);
            let _ = self.diverged;
            self.temps.retain(|(_, v)| *v != 0);
            self.locals.sort_by_key(|(l, _)| l.0);
            self.temps.sort_by_key(|(n, _)| n.0);
        }
    }

    impl Balance<'_> {
        fn counted_local(&mut self, l: LocalId) -> bool {
            let Some(local) = self.func.locals.get(l.index()) else { return false };
            let ty = local.ty.clone();
            matches!(self.counted.counted(&ty), Answer::Yes)
        }

        fn child(&self, id: NodeId, k: usize) -> NodeId {
            let mut cur = id.0 + 1;
            for _ in 0..k {
                cur += self.sizes.get(cur as usize).copied().unwrap_or(1);
            }
            NodeId(cur)
        }

        fn sites(&mut self, id: NodeId, at: Position, st: &mut State) {
            if at == Position::After && self.suppress == Some(id) {
                return;
            }
            let ops: Vec<Site> = self.plan.at(id, at).copied().collect();
            for s in ops {
                let by = if s.op == RcOp::IncRef { 1 } else { -1 };
                match s.target {
                    Target::Local(l) => st.bump(l, by),
                    Target::Node(n) => st.bump_temp(n, by),
                }
            }
        }

        /// Runs the branches from one incoming state and requires that they
        /// agree — which is what "balanced along every path" means at a join.
        fn join(&mut self, branches: Vec<(&Expr, NodeId, Mode)>, st: &mut State) {
            let mut ends: Vec<State> = Vec::new();
            for (e, id, m) in branches {
                let mut copy = st.clone();
                self.walk(e, id, m, &mut copy);
                copy.normalize();
                ends.push(copy);
            }
            // A branch that jumped is checked against the loop header
            // instead, at the jump; it never arrives here.
            let arrivals: Vec<&State> = ends.iter().filter(|s| !s.diverged).collect();
            if let Some(first) = arrivals.first() {
                for other in arrivals.iter().skip(1) {
                    if *other != *first {
                        self.errors.push(format!(
                            "branches disagree in {}: {first:?} vs {other:?}",
                            self.func.debug_name
                        ));
                    }
                }
                *st = (*first).clone();
            } else if let Some(first) = ends.first() {
                // Everything jumped, so nothing falls through.
                *st = first.clone();
                st.diverged = true;
            }
        }

        fn walk(&mut self, e: &Expr, id: NodeId, mode: Mode, st: &mut State) {
            for l in self.fresh_at.get(&id).cloned().unwrap_or_default() {
                st.bump(l, 1);
            }
            self.sites(id, Position::Before, st);
            match &e.kind {
                ExprKind::Local(l) => {
                    self.sites(id, Position::After, st);
                    if mode == Mode::Own && self.counted_local(*l) {
                        st.bump(*l, -1);
                    }
                    return;
                }
                // Not descended into, exactly as the analysis does not: a
                // lambda's body is a scope of its own, and `middle::closures`
                // is what turns one into a function with a plan of its own.
                ExprKind::Lambda { .. } => {}
                ExprKind::Block { stmts, tail } => {
                    let children = stmts.len() + usize::from(tail.is_some());
                    for (k, s) in stmts.iter().enumerate() {
                        let sid = self.child(id, k);
                        match s {
                            Stmt::Let { pattern, value, .. } => {
                                // The drop of a binding nothing reads is keyed
                                // on the value's node, and it happens *after*
                                // the binding exists.
                                let held = self.suppress.replace(sid);
                                self.walk(value, sid, Mode::Own, st);
                                self.suppress = held;
                                let mut bound = Vec::new();
                                pattern.binds(&mut bound);
                                for b in bound {
                                    if self.counted_local(b) {
                                        st.bump(b, 1);
                                    }
                                }
                                // A binding nothing reads is dropped here.
                                self.sites(sid, Position::After, st);
                            }
                            Stmt::Expr(x) => {
                                self.walk(x, sid, Mode::Borrow, st);
                                let ty = x.ty.clone();
                                if fresh(x) && matches!(self.counted.counted(&ty), Answer::Yes) {
                                    st.bump_temp(sid, 1);
                                }
                                self.sites(sid, Position::After, st);
                            }
                        }
                    }
                    if let Some(t) = tail {
                        let tid = self.child(id, children.saturating_sub(1));
                        self.walk(t, tid, mode, st);
                    }
                }
                ExprKind::If { cond, then, else_ } => {
                    self.walk(cond, self.child(id, 0), Mode::Borrow, st);
                    self.join(
                        vec![
                            (then, self.child(id, 1), mode),
                            (else_, self.child(id, 2), mode),
                        ],
                        st,
                    );
                }
                ExprKind::Match { scrutinee, arms } => {
                    let sid = self.child(id, 0);
                    // The scrutinee's own mode is decided by the plan: a drop
                    // of it at an arm entry is what "the match consumed it"
                    // looks like from outside.
                    let consumed = self.consumes_scrutinee(scrutinee);
                    // The same promotion the scan makes for a compound scrutinee.
                    let promoted = !consumed
                        && compound(scrutinee)
                        && matches!(
                            self.counted.counted(&scrutinee.ty.clone()),
                            Answer::Yes
                        );
                    self.walk(
                        scrutinee,
                        sid,
                        if consumed || promoted { Mode::Own } else { Mode::Borrow },
                        st,
                    );
                    if promoted {
                        st.bump_temp(sid, 1);
                    }
                    if consumed {
                        if let ExprKind::Local(l) = &scrutinee.kind {
                            // `Own` mode took the count; the arm's own decref
                            // is the one the plan wrote, so give it back here.
                            st.bump(*l, 1);
                        }
                    }
                    let mut k = 1usize;
                    let mut branches: Vec<(&Expr, NodeId, Mode)> = Vec::new();
                    for a in arms {
                        if a.guard.is_some() {
                            k += 1;
                        }
                        let bid = self.child(id, k);
                        let mut fresh_bound: Vec<LocalId> = Vec::new();
                        a.pattern.fresh_binds(&mut fresh_bound);
                        fresh_bound.retain(|b| self.counted_local(*b));
                        if !fresh_bound.is_empty() {
                            self.fresh_at.insert(bid, fresh_bound);
                        }
                        branches.push((&a.body, bid, mode));
                        k += 1;
                    }
                    self.join(branches, st);
                }
                ExprKind::Loop { entries } => {
                    let branches: Vec<(&Expr, NodeId, Mode)> = entries
                        .iter()
                        .enumerate()
                        .map(|(k, x)| (x, self.child(id, k), mode))
                        .collect();
                    self.join(branches, st);
                }
                ExprKind::Continue { func, args, .. } => {
                    let row: Vec<ir::Ownership> = match func {
                        Some(f) => self.own.get(f.index()).cloned().unwrap_or_default(),
                        None => self.plan.params.clone(),
                    };
                    for (k, arg) in args.iter().enumerate() {
                        let aid = self.child(id, k);
                        let m = match row.get(k) {
                            Some(ir::Ownership::Borrow) => Mode::Borrow,
                            _ => Mode::Own,
                        };
                        self.walk(arg, aid, m, st);
                        if m == Mode::Borrow && fresh(arg) {
                            let ty = arg.ty.clone();
                            if matches!(self.counted.counted(&ty), Answer::Yes) {
                                st.bump_temp(aid, 1);
                            }
                        }
                    }
                    // The jump installs the arguments in the loop's variables,
                    // which are this function's parameters.
                    if func.is_none() {
                        for (k, p) in self.func.params.iter().enumerate() {
                            if row.get(k).copied() == Some(ir::Ownership::Own)
                                && self.counted_local(*p)
                            {
                                st.bump(*p, 1);
                            }
                        }
                    }
                    self.sites(id, Position::After, st);
                    let mut end = st.clone();
                    end.normalize();
                    // One traversal of the cycle proves the invariant: the
                    // state at the back edge has to be the state the header
                    // started in, or the next iteration starts richer or
                    // poorer than this one did and the difference is a leak or
                    // a double free per iteration.
                    let want = if func.is_none() { self.header.clone() } else { State::default() };
                    let mut want = want;
                    want.normalize();
                    if end.locals != want.locals || end.temps != want.temps {
                        self.errors.push(format!(
                            "the back edge in {} does not restore the header: {end:?} vs {want:?}",
                            self.func.debug_name
                        ));
                    }
                    st.diverged = true;
                    return;
                }
                ExprKind::And { lhs, rhs } | ExprKind::Or { lhs, rhs } => {
                    self.walk(lhs, self.child(id, 0), Mode::Borrow, st);
                    // Both the taken and the skipped path.
                    self.join(vec![(rhs, self.child(id, 1), Mode::Borrow)], st);
                }
                ExprKind::Coalesce { lhs, rhs, .. } => {
                    self.walk(lhs, self.child(id, 0), Mode::Borrow, st);
                    self.join(vec![(rhs, self.child(id, 1), Mode::Borrow)], st);
                }
                _ => {
                    let kids = kids(e);
                    let modes = child_modes(e, kids.len(), self.own);
                    for (k, kid) in kids.iter().enumerate() {
                        let kid_id = self.child(id, k);
                        let m = modes.get(k).copied().unwrap_or(Mode::Borrow);
                        let counted = matches!(
                            self.counted.counted(&kid.ty.clone()),
                            Answer::Yes
                        );
                        // The same promotion `Scan::children` makes: a counted
                        // compound child is owned, and its value is a
                        // temporary the sites drop.
                        if m == Mode::Borrow && compound(kid) && counted {
                            self.walk(kid, kid_id, Mode::Own, st);
                            st.bump_temp(kid_id, 1);
                            continue;
                        }
                        self.walk(kid, kid_id, m, st);
                        if m == Mode::Borrow && fresh(kid) && counted {
                            st.bump_temp(kid_id, 1);
                        }
                    }
                }
            }
            self.sites(id, Position::After, st);
            if mode == Mode::Own {
                // A value this node produced with a count of its own — a field
                // read in an owning position — is taken by whatever asked for
                // it, which is this node's parent.
                let taken: Vec<Site> = self
                    .plan
                    .at(id, Position::After)
                    .filter(|s| s.op == RcOp::IncRef && s.target == Target::Node(id))
                    .copied()
                    .collect();
                for _ in taken {
                    st.bump_temp(id, -1);
                }
            }
            for (_, v) in &st.locals {
                if *v < 0 {
                    self.errors.push(format!("a count went negative in {}", self.func.debug_name));
                    break;
                }
            }
        }

        /// Whether the plan says the match consumed its scrutinee: a `DecRef`
        /// of the scrutinee's local at an arm entry.
        fn consumes_scrutinee(&self, scrutinee: &Expr) -> bool {
            let ExprKind::Local(l) = &scrutinee.kind else { return false };
            self.plan
                .sites
                .iter()
                .any(|s| s.op == RcOp::DecRef && s.target == Target::Local(*l) && s.at == Position::Before)
        }
    }

    /// Every function in a program, checked.
    fn check_balance(program: &Program) -> Vec<String> {
        let mut counted = Syntactic::new(program);
        let plan = analyze(program, &mut counted, &Options::default());
        let own: Vec<Vec<ir::Ownership>> =
            plan.funcs.iter().map(|f| f.params.clone()).collect();
        let mut errors = Vec::new();
        for (i, f) in program.funcs.iter().enumerate() {
            let Some(body) = f.body() else { continue };
            let Some(fp) = plan.funcs.get(i) else { continue };
            let mut sizes = Vec::new();
            subtree_sizes(body, &mut sizes);
            let mut b = Balance {
                func: f,
                plan: fp,
                sizes,
                counted: Syntactic::new(program),
                own: &own,
                suppress: None,
                header: State::default(),
                fresh_at: HashMap::default(),
                errors: Vec::new(),
            };
            let mut st = State::default();
            for (k, p) in f.params.iter().enumerate() {
                if fp.params.get(k).copied() == Some(ir::Ownership::Own) && b.counted_local(*p) {
                    st.bump(*p, 1);
                }
            }
            st.normalize();
            b.header = st.clone();
            b.walk(body, NodeId(0), Mode::Own, &mut st);
            st.normalize();
            if st.diverged {
                // Every path jumped, and each jump was checked against the
                // header where it happened.
                errors.extend(b.errors);
                continue;
            }
            if !st.locals.is_empty() || !st.temps.is_empty() {
                b.errors.push(format!("{} ends holding {st:?}", f.debug_name));
            }
            errors.extend(b.errors);
        }
        errors
    }

    /// The shape MEMORY.md §5.2 is about: an owned scrutinee, a payload kept
    /// past the match, a borrow across a call, and two branches that use
    /// different values.
    const TREE: &str = r#"
from "core/effect" import { Alloc, Stdout };
from "core/host" import * as host;
from "core/io" import * as io;

enum Tree { Leaf, Node(Str, [Tree]) }

export fn label(t: Tree, other: Str): Str {
  match (t) {
    .Leaf => other,
    .Node(name, kids) => if (kids.len() > 0) { name } else { other },
  }
}

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout };
  let t = Tree.Node("root", [Tree.Leaf]);
  let _ = io.println(ctx, label(t, "none")).ignore();
  .Ok(())
}
"#;

    const PROGRAM: &str = r#"
from "core/effect" import { Alloc, Stdout };
from "core/host" import * as host;
from "core/io" import * as io;

struct P { name: Str, n: Int }

/// Reads and returns nothing of its argument: borrowed.
export fn size(p: P): Int {
  p.n
}

/// Stores its argument in a constructed value: owned.
export fn wrap(p: P): [P] {
  [p]
}

/// Uses one argument twice, which is where an increment comes from.
export fn twice(s: Str): [Str] {
  [s, s]
}

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout };
  let p = P { name: "a", n: 1 };
  let n = size(p);
  let xs = wrap(p);
  let ys = twice("b");
  let _ = io.println(ctx, "${n} ${xs.len()} ${ys.len()}").ignore();
  .Ok(())
}
"#;

    /// The same, plus the two passes on the native branch that *change node
    /// kinds*: `tail_calls` turns a tail-recursive body into a `Loop` of
    /// `Continue`s, and `closures` turns a `Lambda` into a `Closure`. This is
    /// the tree `middle::native` hands to `rc`, and the shapes below only exist
    /// in it.
    fn compile_native(src: &str) -> Program {
        let mut program = compile(src);
        crate::compiler::middle::tail_calls::rewrite(&mut program);
        crate::compiler::middle::closures::run(&mut program);
        program
    }

    /// A tail-recursive function churning an aggregate per iteration — the
    /// shape the LLVM backend's live-block test leaked three blocks an
    /// iteration on.
    const CHURN: &str = r#"
from "core/effect" import { Alloc, Stdout };
from "core/host" import * as host;
from "core/io" import * as io;

struct Row { name: Str, tags: [Str] }

export fn churn<C: Alloc>(ctx: C, n: Int, acc: [Str]): [Str] {
  if (n <= 0) {
    acc
  } else {
    let row = Row { name: "x", tags: ["a", "b"] };
    let next = acc.push(ctx, row.name);
    churn(ctx, n - 1, next)
  }
}

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout };
  let out = churn(ctx, 3, []);
  let _ = io.println(ctx, "${out.len()}").ignore();
  .Ok(())
}
"#;

    fn loop_body(program: &Program, name: &str) -> FuncIdx {
        let f = find(program, name);
        let body = program.funcs.get(f.index()).and_then(|x| x.body());
        assert!(
            matches!(body.map(|b| &b.kind), Some(ExprKind::Loop { .. })),
            "{name} was expected to be a loop after `tail_calls`"
        );
        f
    }

    /// The numbering is defined as `typed::walk`'s pre-order, and a loop is
    /// where it stopped being: `kids` did not descend into `Loop` or
    /// `Continue`, so a whole loop body counted as one node and every site
    /// keyed after it named the wrong expression. `lower` builds its own table
    /// from [`preorder`], so the two agreed with each other and both were
    /// wrong about the tree.
    #[test]
    fn preorder_agrees_across_a_loop() {
        let program = compile_native(CHURN);
        let mut loops = 0;
        for f in &program.funcs {
            let Some(body) = f.body() else { continue };
            if matches!(body.kind, ExprKind::Loop { .. }) {
                loops += 1;
            }
            let mut by_walk: Vec<String> = Vec::new();
            typed::walk(body, &mut |e| by_walk.push(format!("{:?}", std::ptr::from_ref(e))));
            let mut by_preorder: Vec<String> = Vec::new();
            preorder(body, &mut |_, e| by_preorder.push(format!("{:?}", std::ptr::from_ref(e))));
            assert_eq!(by_walk, by_preorder, "{} numbers differently", f.debug_name);
            let mut sizes = Vec::new();
            let total = subtree_sizes(body, &mut sizes);
            assert_eq!(total as usize, by_walk.len(), "{} sizes short", f.debug_name);
        }
        assert!(loops > 0, "the snippet has a tail-recursive function");
    }

    /// A tail-recursive drain: every iteration builds *both* of its loop
    /// variables, and neither is the caller's to keep alive.
    const DRAIN: &str = r#"
from "core/effect" import { Alloc, Stdout };
from "core/host" import * as host;
from "core/io" import * as io;
from "core/list" import * as list;

export fn drain<C: Alloc>(ctx: C, xs: [Int], acc: [Int]): [Int] {
  match (xs.first()) {
    .Some(v) => drain(ctx, xs.drop(ctx, 1), acc.push(ctx, v)),
    .None => acc,
  }
}

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout };
  let out = drain(ctx, [1, 2, 3, 4], []);
  let _ = io.println(ctx, "${out.len()}").ignore();
  .Ok(())
}
"#;

    /// A loop variable a jump *rebuilds* is owned, and a borrowed parameter is
    /// one the caller keeps alive across the whole call.
    ///
    /// `xs` is read and never stored, so ownership inference called it
    /// borrowed — and a borrowed argument that is a fresh value has nobody to
    /// drop it, so `Scan::drop_temporary` put the drop before the back edge and
    /// the next iteration read a freed list. `drain([1, 2, 3, 4], [])` answered
    /// `[1, 2, 3, 0]` natively and `[1, 2, 3, 4]` on JavaScript.
    #[test]
    fn a_jump_owns_what_it_did_not_pass_through() {
        let program = compile_native(DRAIN);
        let i = loop_body(&program, "drain");
        let mut counted = Syntactic::new(&program);
        let plan = analyze(&program, &mut counted, &Options::default());
        let fp = plan.func(i).expect("a plan");
        // `ctx` is handed straight through and stays the caller's; `xs` and
        // `acc` are rebuilt, so the loop variable takes the count.
        assert_eq!(fp.params.get(1).copied(), Some(ir::Ownership::Own));
        assert_eq!(fp.params.get(2).copied(), Some(ir::Ownership::Own));
        // Nothing the jump carries is dropped before the jump.
        let f = program.funcs.get(i.index()).expect("a function");
        let body = f.body().expect("a body");
        let mut sizes: Vec<u32> = Vec::new();
        subtree_sizes(body, &mut sizes);
        let mut carried: Vec<NodeId> = Vec::new();
        preorder(body, &mut |id, e| {
            let ExprKind::Continue { args, .. } = &e.kind else { return };
            let mut cur = id.0 + 1;
            for _ in 0..args.len() {
                carried.push(NodeId(cur));
                cur += sizes.get(cur as usize).copied().unwrap_or(1);
            }
        });
        assert!(!carried.is_empty(), "the snippet has a jump");
        for n in carried {
            assert!(
                !fp.sites
                    .iter()
                    .any(|s| s.op == RcOp::DecRef && s.target == Target::Node(n)),
                "n{} is carried into the next iteration and dropped before it",
                n.0
            );
        }
        assert_eq!(check_balance(&program), Vec::<String>::new());
    }

    /// The leak repro, as a property of the plan: what an iteration builds and
    /// does not carry through the jump is dropped before the back edge, and the
    /// counts at the back edge are the counts at the header.
    #[test]
    fn a_loop_drops_what_it_does_not_carry_before_the_back_edge() {
        let program = compile_native(CHURN);
        let mut counted = Syntactic::new(&program);
        let plan = analyze(&program, &mut counted, &Options::default());
        let f = loop_body(&program, "churn");
        let fp = plan.func(f).expect("a plan");
        assert!(
            !fp.sites.is_empty(),
            "a loop body with an allocation per iteration has reference operations in it"
        );
        // `row` is built each iteration and only its `name` is carried on, so
        // the row itself dies inside the loop.
        let func = program.funcs.get(f.index()).expect("a function");
        let named: Vec<String> = fp
            .sites
            .iter()
            .filter_map(|s| match s.target {
                Target::Local(l) => func.locals.get(l.index()).map(|x| {
                    format!("{} {}", if s.op == RcOp::IncRef { "inc" } else { "dec" }, x.name)
                }),
                Target::Node(_) => None,
            })
            .collect();
        assert!(named.iter().any(|x| x == "dec row"), "{named:?}");
        assert_eq!(check_balance(&program), Vec::<String>::new());
    }

    /// VALUE-MODEL.md §4.2: `..rest` binds a block the arm allocated, so the
    /// arm drops it and takes no count out of the scrutinee — whether the
    /// scrutinee is borrowed (`tell`) or consumed (`take`).
    #[test]
    fn a_rest_binding_is_dropped_and_never_increfed() {
        let src = r#"
from "core/effect" import { Alloc, Stdout };
from "core/host" import * as host;
from "core/io" import * as io;
from "core/list" import * as list;
from "core/str" import * as str;

export fn tell<C: Alloc>(ctx: C, xs: [Str]): Int {
  match (xs) {
    [] => 0,
    [_h, ..rest] => rest.len() + xs.len(),
  }
}

export fn take<C: Alloc>(ctx: C, xs: [Str]): [Str] {
  match (xs) {
    [] => [],
    [_h, ..rest] => rest.push(ctx, "z"),
  }
}

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout };
  let _ = io.println(ctx, "${tell(ctx, ["a", "b"])} ${take(ctx, ["a", "b"]).len()}").ignore();
  .Ok(())
}
"#;
        let program = compile_native(src);
        let mut counted = Syntactic::new(&program);
        let plan = analyze(&program, &mut counted, &Options::default());
        for name in ["tell", "take"] {
            let i = find(&program, name);
            let fp = plan.func(i).expect("a plan");
            let func = program.funcs.get(i.index()).expect("a function");
            let rest = func
                .locals
                .iter()
                .position(|l| l.name == "rest")
                .map(|k| LocalId(k as u32))
                .unwrap_or_else(|| panic!("{name} binds a local named rest"));
            let ops: Vec<RcOp> = fp
                .sites
                .iter()
                .filter(|s| s.target == Target::Local(rest))
                .map(|s| s.op)
                .collect();
            assert_eq!(ops, vec![RcOp::DecRef], "{name}: {ops:?}");
        }
        assert_eq!(check_balance(&program), Vec::<String>::new());
    }

    /// A binding nothing reads is dropped **after** the operations that gave it
    /// its count, not before them.
    ///
    /// `let x = <init>;` scans the initializer at the *statement's own node*,
    /// so the initializer's sites and the drop of an unread `x` land at the
    /// same `(node, After)` key — and a plan's sites run in the order they were
    /// pushed. Pushing the drop first made the pair *release then retain*: on a
    /// value whose count was one, the release frees the block and the retain
    /// then writes into a header the allocator is already using as free-list
    /// storage. The crash is an unrelated allocation later, which is what made
    /// it look like a backend fault for a wave
    /// (`reports/llvm-parallel-listen-fix.md`).
    ///
    /// Both shapes that put an `incref` at that key are here: a **projection**
    /// out of an aggregate (`Scan::projected`), and a **second read** of a
    /// local something after it still uses (`ExprKind::Local` under
    /// `Mode::Own`). Neither may be preceded at its own key by the drop.
    #[test]
    fn an_unread_binding_is_dropped_after_its_own_incref() {
        let src = r#"
from "core/effect" import { Alloc, Stdout };
from "core/host" import * as host;
from "core/io" import * as io;

struct Pair { n: Int, tags: [Str] }

/// The projection shape: `stale` is words copied out of `pair`, and nothing
/// reads it.
export fn projected(pair: Pair): Int {
  let stale = pair.tags;
  pair.n
}

/// The alias shape: `stale` is a second name for `tags`, which is read again
/// after it.
export fn aliased(tags: [Str]): Int {
  let stale = tags;
  tags.len()
}

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout };
  let p = Pair { n: 1, tags: ["a"] };
  let _ = io.println(ctx, "${projected(p)} ${aliased(["b"])}").ignore();
  .Ok(())
}
"#;
        let program = compile_native(src);
        let mut counted = Syntactic::new(&program);
        let plan = analyze(&program, &mut counted, &Options::default());
        for name in ["projected", "aliased"] {
            let i = find(&program, name);
            let fp = plan.func(i).expect("a plan");
            let func = program.funcs.get(i.index()).expect("a function");
            let stale = func
                .locals
                .iter()
                .position(|l| l.name == "stale")
                .map(|k| LocalId(k as u32))
                .unwrap_or_else(|| panic!("{name} binds a local named stale"));
            let drop = fp
                .sites
                .iter()
                .position(|s| s.target == Target::Local(stale) && s.op == RcOp::DecRef)
                .unwrap_or_else(|| panic!("{name}: the unread binding is never dropped"));
            let site = fp.sites[drop];
            // The whole claim: at the drop's own key, an increment runs first.
            // A key with no increment at all would pass this vacuously, so the
            // increment is asserted to exist as well.
            let increments: Vec<usize> = fp
                .sites
                .iter()
                .enumerate()
                .filter(|(_, s)| s.node == site.node && s.at == site.at && s.op == RcOp::IncRef)
                .map(|(k, _)| k)
                .collect();
            assert!(
                !increments.is_empty(),
                "{name}: nothing gives the binding a count, so the drop has nothing to give back"
            );
            assert!(
                increments.iter().all(|k| *k < drop),
                "{name}: the drop at {drop} runs before the increments at {increments:?} \
                 on node {:?} — release then retain, which frees the block and then \
                 writes through the freed header",
                site.node
            );
        }
        assert_eq!(check_balance(&program), Vec::<String>::new());
    }

    /// A merged mutually recursive group: one `Loop` with an entry per member,
    /// and a `Continue` that names the function it re-enters.
    #[test]
    fn a_merged_group_balances_at_every_entry() {
        let src = r#"
from "core/effect" import { Alloc, Stdout };
from "core/host" import * as host;
from "core/io" import * as io;

export fn even(n: Int, s: Str, t: Str): Str {
  if (n <= 0) { s } else { odd(n - 1, t, s) }
}

export fn odd(n: Int, s: Str, t: Str): Str {
  if (n <= 0) { t } else { even(n - 1, s, t) }
}

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout };
  let _ = io.println(ctx, even(4, "a", "b")).ignore();
  .Ok(())
}
"#;
        let program = compile_native(src);
        let merged = program
            .funcs
            .iter()
            .filter_map(|f| f.body())
            .filter_map(|b| match &b.kind {
                ExprKind::Loop { entries } => Some(entries.len()),
                _ => None,
            })
            .max()
            .unwrap_or(0);
        assert!(merged >= 2, "the two functions were merged into one loop");
        assert_eq!(check_balance(&program), Vec::<String>::new());
    }

    /// A closure built inside a loop: the environment takes a count of every
    /// value it captures, once per iteration, and gives it back.
    #[test]
    fn a_closure_in_a_loop_captures_by_incrementing() {
        let src = r#"
from "core/effect" import { Alloc, Stdout };
from "core/host" import * as host;
from "core/io" import * as io;

export fn tag<C: Alloc>(ctx: C, n: Int, prefix: Str, acc: [Str]): [Str] {
  if (n <= 0) {
    acc
  } else {
    let named = acc.map(ctx, fn(x) => prefix);
    tag(ctx, n - 1, prefix, named)
  }
}

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout };
  let out = tag(ctx, 2, "p", ["a"]);
  let _ = io.println(ctx, "${out.len()}").ignore();
  .Ok(())
}
"#;
        let program = compile_native(src);
        let has_closure = program.funcs.iter().filter_map(|f| f.body()).any(|b| {
            let mut found = false;
            typed::walk(b, &mut |e| {
                if matches!(e.kind, ExprKind::Closure { .. }) {
                    found = true;
                }
            });
            found
        });
        assert!(has_closure, "`closures` lifted the lambda");
        let mut counted = Syntactic::new(&program);
        let plan = analyze(&program, &mut counted, &Options::default());
        let f = loop_body(&program, "tag");
        let func = program.funcs.get(f.index()).expect("a function");
        let fp = plan.func(f).expect("a plan");
        // The environment captures `prefix`, which the next iteration also
        // needs, so the capture increments rather than transfers.
        let incs: Vec<String> = fp
            .sites
            .iter()
            .filter(|s| s.op == RcOp::IncRef)
            .filter_map(|s| match s.target {
                Target::Local(l) => func.locals.get(l.index()).map(|x| x.name.clone()),
                Target::Node(_) => None,
            })
            .collect();
        assert!(incs.iter().any(|x| x == "prefix"), "{incs:?}");
        assert_eq!(check_balance(&program), Vec::<String>::new());
    }

    /// The numbering the plan is keyed by is `typed::walk`'s, node for node.
    #[test]
    fn preorder_matches_typed_walk() {
        let program = compile(PROGRAM);
        for f in &program.funcs {
            let Some(body) = f.body() else { continue };
            let mut by_walk: Vec<String> = Vec::new();
            typed::walk(body, &mut |e| by_walk.push(format!("{:?}", std::ptr::from_ref(e))));
            let mut by_preorder: Vec<String> = Vec::new();
            let mut ids: Vec<u32> = Vec::new();
            preorder(body, &mut |id, e| {
                ids.push(id.0);
                by_preorder.push(format!("{:?}", std::ptr::from_ref(e)));
            });
            assert_eq!(by_walk, by_preorder, "{} numbers differently", f.debug_name);
            assert_eq!(ids, (0..ids.len() as u32).collect::<Vec<u32>>());
            // And the subtree sizes agree with the node count.
            let mut sizes = Vec::new();
            let total = subtree_sizes(body, &mut sizes);
            assert_eq!(total as usize, by_walk.len());
            assert_eq!(sizes.len(), by_walk.len());
        }
    }

    /// MEMORY.md §5.2's rule, on the three shapes it names.
    #[test]
    fn a_parameter_is_borrowed_unless_the_body_takes_it() {
        let program = compile(PROGRAM);
        let mut counted = Syntactic::new(&program);
        let plan = analyze(&program, &mut counted, &Options::default());
        let borrowed = plan.func(find(&program, "size")).expect("a plan").params.clone();
        assert_eq!(borrowed, vec![ir::Ownership::Borrow], "reading a field borrows");
        let owned = plan.func(find(&program, "wrap")).expect("a plan").params.clone();
        assert_eq!(owned, vec![ir::Ownership::Own], "storing takes the count");
    }

    /// A value used twice is incremented once: the second use is the one that
    /// transfers.
    #[test]
    fn a_second_use_is_an_increment() {
        let program = compile(PROGRAM);
        let mut counted = Syntactic::new(&program);
        let plan = analyze(&program, &mut counted, &Options::default());
        let twice = plan.func(find(&program, "twice")).expect("a plan");
        let incs = twice.sites.iter().filter(|s| s.op == RcOp::IncRef).count();
        let decs = twice.sites.iter().filter(|s| s.op == RcOp::DecRef).count();
        assert_eq!((incs, decs), (1, 0), "{:?}", twice.sites);
    }

    /// The whole program's counts balance, on every path.
    #[test]
    fn every_count_balances_on_every_path() {
        let program = compile(PROGRAM);
        assert_eq!(check_balance(&program), Vec::<String>::new());
    }

    /// Including the shapes that make it hard: a branch that uses a value the
    /// other branch does not, a match that consumes what it matched, and a
    /// short-circuit whose right operand may not run.
    #[test]
    fn branches_and_short_circuits_balance_too() {
        let program = compile(TREE);
        assert_eq!(check_balance(&program), Vec::<String>::new());
    }

    /// A chain of short circuits costs one scan per operand, not one per path
    /// through them.
    ///
    /// [`Scan::short_circuit`] used to scan its right operand **twice** — a
    /// probe whose sites were thrown away, then the real scan — and a nested
    /// `&&` inside that operand doubled again, so `n` links cost 2ⁿ scans.
    /// `middle/derives.rs`'s `eq_fields` right-nests exactly one link per field
    /// and says so in its own doc comment, which is how
    /// `cli/tests/conformance/lib/proto/test/binary.buri` came to take minutes
    /// to compile on the native path.
    ///
    /// Sixty links is 10¹⁸ scans if the probe is ever put back, so this test
    /// does not fail slowly: it does not finish. That is the point of the
    /// number — a chain short enough to fail *quickly* would not be a
    /// regression test for an exponential.
    #[test]
    fn a_chain_of_short_circuits_is_scanned_once_per_operand() {
        const LINKS: usize = 60;
        let mut chain = format!("(a{n} == b{n})", n = LINKS - 1);
        for i in (0..LINKS - 1).rev() {
            chain = format!("((a{i} == b{i}) && {chain})");
        }
        let params: Vec<String> = (0..LINKS).map(|i| format!("a{i}: Str, b{i}: Str")).collect();
        let args: Vec<String> = (0..LINKS).map(|i| format!("\"x{i}\", \"x{i}\"")).collect();
        let src = format!(
            r#"
from "core/effect" import {{ Alloc, Stdout }};
from "core/host" import * as host;
from "core/io" import * as io;

export fn same({params}): Bool {{
  {chain}
}}

export fn main(): Result<(), Str> {{
  let ctx = context {{ Alloc: host.alloc, Stdout: host.stdout }};
  let _ = io.println(ctx, "${{same({args})}}").ignore();
  .Ok(())
}}
"#,
            params = params.join(", "),
            args = args.join(", "),
        );
        let program = compile(&src);
        assert_eq!(check_balance(&program), Vec::<String>::new());
    }

    /// A scrutinee a short circuit is keeping alive is **not** consumed by the
    /// `match` that reads it, and its payload is a borrowed view.
    ///
    /// The deferral is what decides this: the right operand holds `o`'s last
    /// use, so `short_circuit` keeps `o` live across the whole expression and
    /// drops it afterwards — and a `match` whose scrutinee is still live after
    /// it takes no count out of it ([`Scan::match_`]'s `token`).
    ///
    /// The discarded probe scan used to decide it the *other* way and leave the
    /// decision behind: it ran against the liveness *before* the deferral, so
    /// its `match` did own the scrutinee, and `owns` put the payload binding
    /// into [`Scan::owned`] — where the real scan then found it. The result was
    /// a `DecRef` of `s` with no `IncRef` anywhere, against a count `o`'s own
    /// drop was already going to release: one block, released twice.
    #[test]
    fn a_deferred_scrutinee_is_not_consumed_by_the_match_that_reads_it() {
        const SRC: &str = r#"
from "core/effect" import { Alloc, Stdout };
from "core/host" import * as host;
from "core/io" import * as io;

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout };
  let o: Option<Str> = .Some("s".concat(ctx, "x"));
  let flag = 1 < 2;
  let ok = flag && match (o) {
    .Some(s) => s.len() > 0,
    .None => false,
  };
  let _ = io.println(ctx, "${ok}").ignore();
  .Ok(())
}
"#;
        let program = compile(SRC);
        let i = find(&program, "main");
        let mut counted = Syntactic::new(&program);
        let plan = analyze(&program, &mut counted, &Options::default());
        let fp = plan.func(i).expect("a plan");
        let sites = named_sites(&program, i, fp);
        assert_eq!(check_balance(&program), Vec::<String>::new(), "{sites:?}");
        assert!(
            !sites.iter().any(|s| s.starts_with("dec s ")),
            "`s` is a view into `o`, which the deferral drops: {sites:?}"
        );
        assert!(
            sites.iter().any(|s| s.starts_with("dec o ")),
            "`o` itself is still dropped: {sites:?}"
        );
    }

    /// One plan's sites as `"<op> <what> <before|after> n<node>"`, in plan
    /// order — a form an expectation can be written in.
    fn named_sites(program: &Program, i: FuncIdx, fp: &FuncPlan) -> Vec<String> {
        let f = program.funcs.get(i.index()).expect("a function");
        fp.sites
            .iter()
            .map(|s| {
                let what = match s.target {
                    Target::Local(l) => f
                        .locals
                        .get(l.index())
                        .map(|x| x.name.clone())
                        .unwrap_or_else(|| format!("l{}", l.0)),
                    Target::Node(n) => format!("n{}", n.0),
                };
                let op = if s.op == RcOp::IncRef { "inc" } else { "dec" };
                let at = if s.at == Position::Before { "before" } else { "after" };
                format!("{op} {what} {at} n{}", s.node.0)
            })
            .collect()
    }

    /// The placement itself, on the shape the design argues about: the payloads
    /// that survive the arm are incremented out of the value, the value is
    /// dropped there, a borrow across a call is dropped after the call, and the
    /// branch that does not use a value drops it on entry.
    #[test]
    fn a_consuming_match_dups_what_it_keeps_and_drops_what_it_matched() {
        let program = compile(TREE);
        let i = find(&program, "label");
        let mut counted = Syntactic::new(&program);
        let plan = analyze(&program, &mut counted, &Options::default());
        let fp = plan.func(i).expect("a plan");
        assert_eq!(fp.params, vec![ir::Ownership::Own, ir::Ownership::Own]);
        assert_eq!(
            named_sites(&program, i, fp),
            vec![
                // `.Leaf => other`: the matched value dies at the arm entry.
                "dec t before n3",
                // `.Node(name, kids)`: what the arm keeps is incremented out of
                // it first, and then it dies.
                "inc name before n4",
                "inc kids before n4",
                "dec t before n4",
                // `kids.len()` borrows, so the drop lands after the call.
                "dec kids after n6",
                // The branch that returns `name` has no use for `other`.
                "dec other before n9",
                // And the branch that returns `other` has none for `name`.
                "dec name before n11",
            ]
        );
    }

    /// A projection is words copied out of its base, so the base has to reach
    /// the construct that reads them.
    ///
    /// `two(one(p.a), p.b)`: `p`'s last mention is `p.b`, which is evaluated
    /// **after** `one(p.a)`. The drop belongs after `two`, and it used to land
    /// after `one` — early enough that `p.b` read a block `p`'s drop glue had
    /// already freed. `sortcheck/cmd/q2` is the same three lines with a
    /// `concat` in place of `two`, and it printed `[0, 0, 0]`.
    #[test]
    fn a_projection_outlives_the_siblings_evaluated_after_it() {
        const SRC: &str = r#"
struct Pair { a: [Int], b: [Int] }

fn one(xs: [Int]): Int { xs.len() }
fn two(n: Int, ys: [Int]): Int { n + ys.len() }

export fn main(): Result<(), Str> {
  let p = Pair { a: [1], b: [2, 3] };
  let n = two(one(p.a), p.b);
  .Ok(())
}
"#;
        let program = compile(SRC);
        let i = find(&program, "main");
        let mut counted = Syntactic::new(&program);
        let plan = analyze(&program, &mut counted, &Options::default());
        let fp = plan.func(i).expect("a plan");
        // `n7` is the call to `two`; `n8` is the call to `one` inside it.
        assert_eq!(named_sites(&program, i, fp), vec!["dec p after n7"]);
        assert_eq!(check_balance(&program), Vec::<String>::new());
    }

    /// A consumed scrutinee is disposed of **once per arm**, and the arm that
    /// still reads it does so at its own last use rather than at its entry.
    ///
    /// This is `core/testing/assert`'s `some` written out: the `.None` arm
    /// hands the `Option` itself to the failure report, which is what
    /// `assert.ok` does not do with its `Result` — and why the two behaved
    /// differently. The `.Some` arm used to carry two drops (the match's own,
    /// and one `Scan::balance` added because the other arm put the scrutinee in
    /// the union), so `assert.some` freed the payload it was answering with.
    #[test]
    fn a_consumed_scrutinee_is_dropped_once_on_every_arm() {
        const SRC: &str = r#"
fn label(what: Str, got: Option<Str>): Str { what }

fn make(s: Str): Option<Str> { .Some(s) }

export fn some(o: Option<Str>): Str {
  match (o) {
    .Some(v) => v,
    .None => label("some", o),
  }
}

export fn main(): Result<(), Str> {
  let _ = some(make("x"));
  .Ok(())
}
"#;
        let program = compile(SRC);
        let i = find(&program, "some");
        let mut counted = Syntactic::new(&program);
        let plan = analyze(&program, &mut counted, &Options::default());
        let fp = plan.func(i).expect("a plan");
        assert_eq!(fp.params, vec![ir::Ownership::Own]);
        assert_eq!(
            named_sites(&program, i, fp),
            vec![
                // `.Some(v) => v`: the payload is increfed out and the value
                // dies at the entry, because the arm is done with it.
                "inc v before n3",
                "dec o before n3",
                // `.None => label("some", o)`: `label` borrows, so the drop is
                // after the call — and there is no second one at the entry.
                "dec o after n4",
            ]
        );
        assert_eq!(check_balance(&program), Vec::<String>::new());
    }

    /// A value the match itself built has no binding and no owner.
    ///
    /// The arms take a count for whatever they keep out of it — `owns` is
    /// false, so every payload use is an increment — and after the arms nothing
    /// names it. `match (q.pop(ctx))` leaked the `Option` and the two lists the
    /// queue it answered still pointed at, once per iteration of a drain.
    #[test]
    fn a_scrutinee_the_match_built_is_dropped_after_the_arms() {
        const SRC: &str = r#"
from "core/effect" import { Alloc, Stdout };
from "core/host" import * as host;
from "core/io" import * as io;
from "core/list" import * as list;

fn two<C: Alloc>(ctx: C, n: Int): ([Int], [Int]) {
  (list.range(ctx, 0, n), list.range(ctx, 0, n + 1))
}

export fn sizes<C: Alloc>(ctx: C, n: Int): Int {
  match (two(ctx, n)) {
    (a, b) => a.len() + b.len(),
  }
}

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout };
  let _ = io.println(ctx, "${sizes(ctx, 2)}").ignore();
  .Ok(())
}
"#;
        let program = compile(SRC);
        let i = find(&program, "sizes");
        let mut counted = Syntactic::new(&program);
        let plan = analyze(&program, &mut counted, &Options::default());
        let fp = plan.func(i).expect("a plan");
        // `n1` is the `match`, `n2` its scrutinee, and the drop is after the
        // arms have read what they keep out of it.
        assert_eq!(named_sites(&program, i, fp), vec!["dec n2 after n1"]);
    }

    /// A fresh value reached through a branch is still fresh.
    ///
    /// `middle::inline` replaces a call with the callee's body, so the thing a
    /// borrowing construct is handed stops being an `ExprKind::CallFn` — and
    /// `fresh` said no, and nothing dropped it. `"[${show(ctx, xs)}]"` was the
    /// program: one call, so the inliner pasted it in, and the string leaked.
    #[test]
    fn a_fresh_value_behind_a_branch_is_still_dropped() {
        const SRC: &str = r#"
from "core/effect" import { Alloc, Stdout };
from "core/host" import * as host;
from "core/io" import * as io;
from "core/str" import * as str;

fn size(s: Str): Int { s.len() }

export fn shown<C: Alloc>(ctx: C, n: Int): Int {
  size(if (n > 0) { str.format(ctx, "v${n}") } else { str.format(ctx, "z") })
}

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout };
  let _ = io.println(ctx, "${shown(ctx, 2)}").ignore();
  .Ok(())
}
"#;
        let program = compile(SRC);
        let i = find(&program, "shown");
        let mut counted = Syntactic::new(&program);
        let plan = analyze(&program, &mut counted, &Options::default());
        let fp = plan.func(i).expect("a plan");
        // `n1` is the call to `size`, `n2` the `if` it borrows. The rest are
        // the two templates' own holes.
        assert!(
            named_sites(&program, i, fp).contains(&String::from("dec n2 after n1")),
            "{:?}",
            named_sites(&program, i, fp)
        );
    }

    /// A bigger program, so that the balance is checked over the standard
    /// library the snippet reaches rather than only over what the snippet
    /// writes: lists, strings, options, closures and a fold.
    #[test]
    fn the_standard_library_balances_too() {
        let src = r#"
from "core/effect" import { Alloc, Stdout };
from "core/host" import * as host;
from "core/io" import * as io;
from "core/list" import * as list;

struct Row { name: Str, tags: [Str] }

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout };
  let rows = [
    Row { name: "a", tags: ["x", "y"] },
    Row { name: "b", tags: [] },
  ];
  let names = rows.map(ctx, fn(r) => r.name);
  let joined = names.join(ctx, ", ");
  let first: Option<Row> = rows.first();
  let shown = match (first) {
    .Some(r) => r.name,
    .None => "none",
  };
  let total = rows.fold(fn(acc: Int, r: Row) => acc + r.tags.len(), 0);
  let _ = io.println(ctx, "${joined} ${shown} ${total}").ignore();
  .Ok(())
}
"#;
        let program = compile(src);
        assert!(program.funcs.len() > 5, "the snippet reaches the library");
        assert_eq!(check_balance(&program), Vec::<String>::new());
    }


    /// A match that consumes its scrutinee pairs the dying value with the
    /// construction in the arm — MEMORY.md §5.3's reuse, in its analysis form.
    #[test]
    fn a_dying_value_is_paired_with_a_construction() {
        let src = r#"
from "core/effect" import { Alloc, Stdout };
from "core/host" import * as host;
from "core/io" import * as io;

enum Pair { One(Str), Two(Str, Str) }

export fn swap(p: Pair): Pair {
  match (p) {
    .One(a) => .One(a),
    .Two(a, b) => .Two(b, a),
  }
}

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout };
  let p = Pair.Two("a", "b");
  let q = swap(p);
  let _ = io.println(ctx, match (q) { .One(a) => a, .Two(a, _) => a }).ignore();
  .Ok(())
}
"#;
        let program = compile(src);
        let mut counted = Syntactic::new(&program);
        let plan = analyze(&program, &mut counted, &Options::default());
        let swap = plan.func(find(&program, "swap")).expect("a plan");
        assert_eq!(swap.params, vec![ir::Ownership::Own], "the match consumes it");
        assert_eq!(swap.reuse.len(), 2, "one per arm: {:?}", swap.reuse);
        assert!(swap.reuse.iter().any(|r| r.fields == 2), "{:?}", swap.reuse);
        // Turning the pairing off leaves everything else where it was.
        let mut counted = Syntactic::new(&program);
        let without = analyze(&program, &mut counted, &Options { reuse: false, sharing: false });
        let swap_off = without.func(find(&program, "swap")).expect("a plan");
        assert!(swap_off.reuse.is_empty());
        assert_eq!(swap_off.sites, swap.sites);
    }

    /// A scrutinee that is **still live after the match** is not dying, so
    /// there is nothing to reuse and nothing is paired.
    ///
    /// This is the edge that makes reuse sound rather than fast: writing into
    /// a cell something else still reads is the one way MEMORY.md §5.3's
    /// mutation becomes observable, and the guard against it is the same
    /// `!live.contains(l)` that decides the drop.
    #[test]
    fn a_value_used_after_the_construction_is_not_paired() {
        let src = r#"
from "core/effect" import { Alloc, Stdout };
from "core/host" import * as host;
from "core/io" import * as io;

enum Pair { One(Str), Two(Str, Str) }

export fn first(p: Pair): Str {
  match (p) {
    .One(a) => a,
    .Two(a, _b) => a,
  }
}

export fn swapped(p: Pair, other: Pair): Pair {
  let q: Pair = match (p) {
    .One(a) => .One(a),
    .Two(a, b) => .Two(b, a),
  };
  // `p` is read *after* the construction, so the match above did not consume
  // it and its cell is not dying at the point the arm built a new one.
  let n = match (p) { .One(_a) => 1, .Two(_a, _b) => 2 };
  if (n == 1) { q } else { other }
}

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout };
  let _ = io.println(ctx, first(swapped(Pair.Two("a", "b"), Pair.One("c")))).ignore();
  .Ok(())
}
"#;
        let program = compile(src);
        let mut counted = Syntactic::new(&program);
        let plan = analyze(&program, &mut counted, &Options::default());
        let swapped = plan.func(find(&program, "swapped")).expect("a plan");
        assert!(
            swapped.reuse.is_empty(),
            "a scrutinee read after the arms was paired anyway: {:?}",
            swapped.reuse
        );
    }

    /// An arm whose body is **not a construction** pairs nothing, and an arm
    /// whose construction has a different field count is recorded with *its
    /// own* count rather than the scrutinee's.
    ///
    /// [`Reuse::fields`] is the shape half of MEMORY.md §5.3's condition, and
    /// `lower` compares size classes with it. A pairing that reported the
    /// wrong count would be a write into a block too small for it, so the
    /// number is asserted per arm rather than in aggregate.
    #[test]
    fn only_a_construction_pairs_and_it_carries_its_own_shape() {
        let src = r#"
from "core/effect" import { Alloc, Stdout };
from "core/host" import * as host;
from "core/io" import * as io;

enum Shape { Nil, One(Str), Two(Str, Str) }

export fn reshape(s: Shape, fallback: Str): Shape {
  match (s) {
    .Nil => .Nil,
    // A construction of one field, out of a scrutinee whose live variant
    // carries one.
    .One(a) => .One(a),
    // A construction of *two* fields out of the same scrutinee type.
    .Two(a, b) => .Two(b, a),
  }
}

export fn pick(s: Shape, d: Str): Str {
  match (s) {
    // Not a construction at all: every arm answers a binding.
    .Nil => d,
    .One(a) => a,
    .Two(a, _b) => a,
  }
}

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout };
  let a = reshape(Shape.Two("a", "b"), "z");
  let _ = io.println(ctx, pick(a, "z")).ignore();
  .Ok(())
}
"#;
        let program = compile(src);
        let mut counted = Syntactic::new(&program);
        let plan = analyze(&program, &mut counted, &Options::default());
        let reshape = plan.func(find(&program, "reshape")).expect("a plan");
        let mut shapes: Vec<usize> = reshape.reuse.iter().map(|r| r.fields).collect();
        shapes.sort_unstable();
        assert_eq!(
            shapes,
            vec![0, 1, 2],
            "each arm pairs with its own field count: {:?}",
            reshape.reuse
        );
        // Every pairing names the scrutinee and no other local.
        let scrutinee = reshape.reuse.first().map(|r| r.token);
        assert!(
            reshape.reuse.iter().all(|r| Some(r.token) == scrutinee),
            "a pairing named something other than the dying scrutinee: {:?}",
            reshape.reuse
        );
        // `pick` consumes its scrutinee just as `reshape` does, and pairs
        // nothing: no arm of it builds anything, so there is no allocation for
        // the dying cell to become.
        let pick = plan.func(find(&program, "pick")).expect("a plan");
        assert!(
            pick.reuse.is_empty(),
            "an arm that is not a construction was paired: {:?}",
            pick.reuse
        );
    }

    /// The purity column, which is the other half of what `ir::Facts` wants.
    #[test]
    fn purity_is_a_fixpoint_over_the_call_graph() {
        let src = r#"
from "core/effect" import { Alloc, Stdout };
from "core/host" import * as host;
from "core/io" import * as io;

export fn double(n: Int): Int { n * 2 }

export fn quadruple(n: Int): Int { double(double(n)) }

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout };
  let _ = io.println(ctx, "${quadruple(2)}").ignore();
  .Ok(())
}
"#;
        let program = compile(src);
        let mut counted = Syntactic::new(&program);
        let plan = analyze(&program, &mut counted, &Options::default());
        assert_eq!(
            plan.func(find(&program, "double")).map(|f| f.purity),
            Some(ir::Purity::Pure)
        );
        assert_eq!(
            plan.func(find(&program, "quadruple")).map(|f| f.purity),
            Some(ir::Purity::Pure),
            "purity propagates through a call"
        );
        assert_eq!(
            plan.func(find(&program, "main")).map(|f| f.purity),
            Some(ir::Purity::Effectful),
            "printing is not pure"
        );
        assert_eq!(plan.func(find(&program, "double")).map(|f| f.can_abort), Some(false));
    }

    /// A type the classifier cannot answer for carries no operations and is named,
    /// which is the difference between a leak and a wrong answer.
    #[test]
    fn what_the_classifier_cannot_answer_is_recorded_rather_than_guessed() {
        struct Nothing;
        impl Counted for Nothing {
            fn counted(&mut self, _ty: &Ty) -> Answer {
                Answer::Unknown
            }
        }
        let program = compile(PROGRAM);
        let plan = analyze(&program, &mut Nothing, &Options::default());
        let main = plan.func(find(&program, "main")).expect("a plan");
        assert!(main.sites.is_empty(), "nothing classified, nothing emitted");
        assert!(!main.unclassified.is_empty(), "and every type is named");
    }

    /// The two shapes [`Syntactic`] could not classify from bodies alone, and
    /// the leak each one was.
    ///
    /// A `Ty::Ctx` is written down nowhere, so no literal names one; an
    /// `Option<Str>` that only ever arrives *from* `list.get` is constructed by
    /// no `.Some(..)` either. Both were `Answer::Unknown`, which this pass
    /// reads as "not counted" and for which it emits nothing at all — so the
    /// string `list.get` retained into the payload was never released.
    /// `monomorphize::Shapes` is what answers them now.
    #[test]
    fn a_context_and_an_option_no_literal_builds_are_both_counted() {
        const SRC: &str = r#"
from "core/effect" import { Alloc, Stdout };
from "core/host" import * as host;
from "core/io" import * as io;
from "core/list" import * as list;
from "core/str" import * as str;

export fn showFirst<C: Alloc>(ctx: C, o: Option<Str>): Str {
  match (o) { .Some(v) => str.format(ctx, "S${v}"), .None => "N" }
}

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout };
  let built = list.range(ctx, 0, 3).mapCtx(ctx, fn(c, i) => str.format(c, "n${i}"));
  let _ = io.println(ctx, showFirst(ctx, built.get(1))).ignore();
  .Ok(())
}
"#;
        let program = compile(SRC);
        let mut counted = Syntactic::new(&program);
        let f = program.funcs.get(find(&program, "showFirst").index()).expect("a function");
        let ctx_ty = f.locals.first().map(|l| l.ty.clone()).expect("the context parameter");
        let opt_ty = f.locals.get(1).map(|l| l.ty.clone()).expect("the `Option<Str>` parameter");
        assert!(matches!(ctx_ty, Ty::Ctx(_)), "the first parameter is the context");
        // `host.alloc` and `host.stdout` are zero-sized markers, so the answer
        // here is `No` — which is the point: the defect was `Unknown`, which
        // means "no operations at all" for a type that may well hold a
        // closure, and it was `Unknown` for *every* context in the program.
        assert_ne!(counted.counted(&ctx_ty), Answer::Unknown);
        assert_eq!(counted.counted(&opt_ty), Answer::Yes);

        // And nothing in the whole program is left unanswered, which is the
        // property the leak was a symptom of rather than one function's plan.
        let plan = analyze(&program, &mut Syntactic::new(&program), &Options::default());
        let unanswered: Vec<&Ty> = plan.funcs.iter().flat_map(|f| f.unclassified.iter()).collect();
        assert_eq!(unanswered, Vec::<&Ty>::new());
    }

    /// A `match` on a value it built, where one arm takes the back edge and
    /// another falls through.
    ///
    /// The drop of the scrutinee went either before every jump — only where
    /// *every* arm jumped — or after the arms, and a `match` that both
    /// continues and falls out has paths of each kind. So the recursive arm
    /// disposed of nothing, and a drain leaked its `Option` once an iteration.
    /// Both keys are emitted now; a path takes exactly one of them.
    #[test]
    fn a_fresh_scrutinee_is_dropped_on_the_arm_that_jumps_too() {
        const SRC: &str = r#"
from "core/effect" import { Alloc, Stdout };
from "core/host" import * as host;
from "core/io" import * as io;
from "core/list" import * as list;

fn takeOne<C: Alloc>(ctx: C, xs: [Int]): Option<(Int, [Int])> {
  match (xs.first()) {
    .Some(v) => .Some((v, xs.drop(ctx, 1))),
    .None => .None,
  }
}

export fn drain<C: Alloc>(ctx: C, xs: [Int], acc: [Int]): [Int] {
  match (takeOne(ctx, xs)) {
    .Some(t) => {
      let (v, rest) = t;
      drain(ctx, rest, acc.push(ctx, v))
    },
    .None => acc,
  }
}

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout };
  let _ = io.println(ctx, "${drain(ctx, [1, 2], []).len()}").ignore();
  .Ok(())
}
"#;
        let program = compile_native(SRC);
        let i = loop_body(&program, "drain");
        let mut counted = Syntactic::new(&program);
        let plan = analyze(&program, &mut counted, &Options::default());
        let fp = plan.func(i).expect("a plan");
        assert_eq!(
            named_sites(&program, i, fp),
            vec![
                // `n3` is `takeOne(ctx, xs)`, the value the match built. It is
                // dropped after the match — the `.None` arm's path — *and*
                // before the back edge at `n11`, which is the `.Some` arm's.
                // Before, only the first of the two was there.
                "dec n3 after n2",
                "dec xs after n3",
                "inc t after n7",
                "dec acc after n11",
                "dec n3 after n11",
            ]
        );
    }

    /// **Calling a closure does not consume it.**
    ///
    /// `child_modes` gave every child of an [`ExprKind::CallValue`] `Own`, and
    /// [`kids`] puts the *callee* first — so the closure a call was made
    /// through was treated as handed over to the call. Nothing on the other
    /// side takes it: [`ir::Inst::CallIndirect`] is a load of `code` and a pass
    /// of `env`, and what frees an environment is the closure value's own drop.
    /// So every closure a program ever called leaked its environment, and a
    /// closure it merely built and dropped did not — which is why the shape hid
    /// in the corpus for so long.
    ///
    /// `collect_consuming` had it right all along: its `CallValue` arm consumes
    /// `args` and not `callee`. The two functions describe one convention and
    /// disagreed about it.
    ///
    /// The plan for `twice`, before and after:
    ///
    /// ```text
    /// before:  inc g after n5
    /// after:   dec g after n7
    /// ```
    ///
    /// — an increment with no decrement anywhere, against a drop at the last
    /// use and no increment at all, because the first call no longer takes a
    /// count it was never going to give back. This is
    /// `codegen/tail_calls.buri`'s seventeen live blocks and
    /// `semantics/evaluation.buri`'s twenty.
    #[test]
    fn a_called_closure_is_not_consumed_by_the_call() {
        const SRC: &str = r#"
from "core/effect" import { Alloc, Stdout };
from "core/host" import * as host;
from "core/io" import * as io;

export fn twice<C: Alloc>(ctx: C, n: Int): Int {
  let g: fn(Int) => Int = fn(x) => x + n;
  g(100) + g(200)
}

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout };
  let _ = io.println(ctx, "${twice(ctx, 1)}").ignore();
  .Ok(())
}
"#;
        let program = compile_native(SRC);
        let i = find(&program, "twice");
        let mut counted = Syntactic::new(&program);
        let plan = analyze(&program, &mut counted, &Options::default());
        let fp = plan.func(i).expect("a plan");
        // `n5` is the first call and `n7` the second. One drop, at the last
        // use, and nothing else: a call reads the closure and gives it back.
        assert_eq!(named_sites(&program, i, fp), vec!["dec g after n7"]);
    }

    /// **A projection of a temporary releases the temporary.**
    ///
    /// A struct is a stack value in both backends, so `mk(ctx).a` allocated
    /// nothing for the `Pair` and everything for its two `[Str]` fields — and
    /// the moment one field was copied out, the other had no name anywhere and
    /// the copy carried the count `mk` had taken for it. `Scan::project` is
    /// written for a base this function can *name*, whose owner decides when it
    /// dies; a base with no name has no owner, so the projection is where it
    /// dies.
    ///
    /// Two positions, because they fail differently:
    ///
    /// ```text
    /// firstLen — borrowed by `len`   before: (nothing at all)
    ///                                 after: dec n2 after n1
    ///                                        inc n2 after n2
    ///                                        dec n3 after n2
    ///
    /// keep     — returned            before: inc n1 after n1
    ///                                 after: inc n1 after n1
    ///                                        dec n2 after n1
    /// ```
    ///
    /// The increment always precedes the release, so the field the projection
    /// hands on never reaches zero in between; `keep`'s parent takes the
    /// reference and `firstLen`'s borrows it, which is why only the second has
    /// a drop of the projection itself. This is `crypto/sha256.buri`'s five
    /// live blocks — `crypto.sha256Text(ctx, "x").0`, once per digest.
    #[test]
    fn a_projection_of_a_temporary_releases_it() {
        const SRC: &str = r#"
from "core/effect" import { Alloc, Stdout };
from "core/host" import * as host;
from "core/io" import * as io;
from "core/list" import * as list;

struct Pair { a: [Str], b: [Str] }

fn mk<C: Alloc>(ctx: C): Pair { Pair { a: ["x".repeat(ctx, 8)], b: ["y".repeat(ctx, 8)] } }

export fn firstLen<C: Alloc>(ctx: C): Int { mk(ctx).a.len() }

export fn keep<C: Alloc>(ctx: C): [Str] { mk(ctx).a }

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout };
  let _ = io.println(ctx, "${firstLen(ctx)} ${keep(ctx).len()}").ignore();
  .Ok(())
}
"#;
        let program = compile_native(SRC);
        let mut counted = Syntactic::new(&program);
        let plan = analyze(&program, &mut counted, &Options::default());

        // `n1` is `len`, `n2` the projection, `n3` the `mk` call: the field is
        // increfed, the `Pair` released, and the borrowed projection dropped
        // after the call that read it.
        let i = find(&program, "firstLen");
        assert_eq!(
            named_sites(&program, i, plan.func(i).expect("a plan")),
            vec!["dec n2 after n1", "inc n2 after n2", "dec n3 after n2"]
        );

        // Returned instead: `n1` is the projection and `n2` the `mk` call. The
        // increment was already there — it is the release that was missing, and
        // without it `b` was freed by nobody and `a` came back one count high.
        let i = find(&program, "keep");
        assert_eq!(
            named_sites(&program, i, plan.func(i).expect("a plan")),
            vec!["inc n1 after n1", "dec n2 after n1"]
        );
    }

    /// This pass reads the tree and does not write it, which is the whole of
    /// its isolation from the JavaScript backend: even if `middle::run` ever
    /// called it, no artifact could move.
    #[test]
    fn the_tree_is_unchanged() {
        let program = compile(PROGRAM);
        let before: Vec<String> =
            program.funcs.iter().map(|f| format!("{:?}", f.body())).collect();
        let plan = run(&program);
        let after: Vec<String> =
            program.funcs.iter().map(|f| format!("{:?}", f.body())).collect();
        assert_eq!(before, after);
        assert_eq!(plan.funcs.len(), program.funcs.len());
    }

    // -- the `can_park` column ----------------------------------------------
    //
    // Hand-built programs rather than snippets, because the question is about
    // two *instantiations* of one source function and a snippet cannot put
    // them side by side without dragging the whole hermetic test context in
    // with them. What monomorphization guarantees, and what these stand in
    // for: a `Key::Fn(id, targs)` per context, one `Func` slot each, and the
    // callee of every `CallFn` already a `FuncIdx`.

    fn parked(program: &Program) -> Vec<bool> {
        let (_, _, parks) = infer_effects(program);
        parks
    }

    fn intrinsic_func(name: &str, key: &str) -> Func {
        Func {
            symbol: name.to_string(),
            debug_name: name.to_string(),
            params: Vec::new(),
            locals: Vec::new(),
            kind: FuncKind::Intrinsic(key.to_string()),
            ret: Ty::Unit,
            desc: None,
            span: Span::default(),
        }
    }

    fn body_func(name: &str, body: Expr) -> Func {
        Func {
            symbol: name.to_string(),
            debug_name: name.to_string(),
            params: Vec::new(),
            locals: Vec::new(),
            kind: FuncKind::Body(body),
            ret: Ty::Unit,
            desc: None,
            span: Span::default(),
        }
    }

    fn call_to(to: u32) -> Expr {
        Expr::new(
            ExprKind::CallFn { func: typed::Callee::Func(FuncIdx(to)), args: Vec::new() },
            Ty::Unit,
            Span::default(),
        )
    }

    /// A body that calls each of these in turn.
    fn calls(to: &[u32]) -> Expr {
        Expr::new(
            ExprKind::Tuple(to.iter().map(|t| call_to(*t)).collect()),
            Ty::Unit,
            Span::default(),
        )
    }

    fn hand_built(funcs: Vec<Func>) -> Program {
        Program {
            funcs,
            roots: monomorphize::ProgramRoots::Main(FuncIdx(0)),
            descriptors: Vec::new(),
            desc_modules: Vec::new(),
            desc_index: HashMap::default(),
            ctx_layouts: HashMap::default(),
            shapes: Default::default(),
            stylesheet: String::new(),
            inline_styles: false,
            themes: false,
        }
    }

    /// Naive iteration has to go round a cycle more than once, and a cycle
    /// that reaches nothing blocking has to *stay* at `false` rather than
    /// climbing because it is a cycle.
    #[test]
    fn parkability_is_a_fixpoint_across_a_cycle() {
        // 0 main -> 1 and 5; 1 <-> 2, and 2 -> 3 -> 4, the blocking intrinsic.
        // 5 <-> 6 is a second cycle, reaching only 7, which does not block.
        let program = hand_built(vec![
            body_func("main", calls(&[1, 5])),
            body_func("a", calls(&[2])),
            body_func("b", calls(&[1, 3])),
            body_func("c", calls(&[4])),
            intrinsic_func("readFile", "host.HostFs.readFile"),
            body_func("x", calls(&[6])),
            body_func("y", calls(&[5, 7])),
            intrinsic_func("nowMillis", "host.HostClock.nowMillis"),
        ]);
        assert_eq!(
            parked(&program),
            vec![true, true, true, true, true, false, false, false],
            "the blocking half is `true` all the way back to `main`, and reading \
             the clock does not make the other half wait"
        );
    }

    /// The case the column exists for: one source function, two contexts, two
    /// answers.
    ///
    /// `fs.readText<C: Alloc + Fs>` at a context binding `host.HostFs` reaches
    /// a call that waits on a disk; the same source at the hermetic test
    /// context reaches `host_testing.TestFs`, which is a page of memory.
    /// Monomorphization has already made them two `Func` slots, so the
    /// fixpoint separates them with no further analysis.
    #[test]
    fn one_source_function_at_two_contexts_gets_two_answers() {
        let program = hand_built(vec![
            body_func("main", calls(&[1, 2])),
            body_func("fs:readText<HostFs>", calls(&[3])),
            body_func("fs:readText<TestFs>", calls(&[4])),
            intrinsic_func("HostFs.readFile", "host.HostFs.readFile"),
            intrinsic_func("TestFs.readFile", "host_testing.TestFs.readFile"),
        ]);
        let parks = parked(&program);
        assert!(parks[1], "`readText` at `host.HostFs` waits on the disk");
        assert!(!parks[2], "`readText` at the test context reaches only memory");
        assert!(parks[0], "and a caller of both waits, because one half of it does");
    }

    /// Every key in the seed list, and the near misses beside them: reading
    /// the clock is not sleeping on it, and the whole `HostFs` surface is in
    /// by prefix rather than by enumeration.
    #[test]
    fn the_seed_list_is_the_blocking_host_calls_and_nothing_else() {
        for key in [
            "host.HostFs.readFile",
            "host.HostFs.writeFile",
            "host.HostFs.syncFile",
            "host.HostNet.fetch",
            "host.HostClock.sleepMillis",
            "host.HostStdin.readLine",
            "host.HostStdin.readBytes",
        ] {
            assert!(suspends(key), "{key} blocks");
        }
        for key in [
            "host.HostClock.nowMillis",
            "host.HostStdout.println",
            "host.HostRand.nextInt",
            "host_testing.TestFs.readFile",
            "host_testing.TestClock.sleepMillis",
            "derivePrimHash",
        ] {
            assert!(!suspends(key), "{key} does not block");
        }
    }

    /// An indirect call reaches a code pointer this pass cannot name, so it is
    /// `true` — the same answer the purity column gives it, for the same
    /// reason.
    #[test]
    fn a_call_through_a_function_value_is_conservatively_parkable() {
        let callee = Expr::new(ExprKind::Unit, Ty::Unit, Span::default());
        let indirect = Expr::new(
            ExprKind::CallValue { callee: Box::new(callee), args: Vec::new() },
            Ty::Unit,
            Span::default(),
        );
        let program = hand_built(vec![body_func("main", indirect)]);
        assert_eq!(parked(&program), vec![true]);
    }

    /// A host call written as an intrinsic *node* rather than reached as an
    /// intrinsic *function* counts too — the two spellings must not disagree.
    #[test]
    fn an_inline_intrinsic_node_seeds_the_column() {
        let node = Expr::new(
            ExprKind::Intrinsic {
                name: "host.HostNet.fetch".to_string(),
                targs: Vec::new(),
                args: Vec::new(),
            },
            Ty::Unit,
            Span::default(),
        );
        let program = hand_built(vec![body_func("main", node)]);
        assert_eq!(parked(&program), vec![true]);
    }

    /// The column reaches `ir::Facts` from the plan, on both branches of the
    /// pipeline: [`run`] is the native one and [`sharing`] is the one the
    /// JavaScript backend runs for itself.
    #[test]
    fn the_column_is_on_the_plan_from_both_entry_points() {
        let program = hand_built(vec![
            body_func("main", calls(&[1, 2])),
            body_func("waits", calls(&[3])),
            body_func("does not", calls(&[4])),
            intrinsic_func("HostFs.readFile", "host.HostFs.readFile"),
            intrinsic_func("TestFs.readFile", "host_testing.TestFs.readFile"),
        ]);
        for plan in [run(&program), sharing(&program)] {
            let waits = plan.func(FuncIdx(1)).expect("a plan");
            let not = plan.func(FuncIdx(2)).expect("a plan");
            assert!(waits.can_park);
            assert!(!not.can_park);
            assert!(waits.facts().can_park, "and `facts` is what `lower` copies");
            assert!(!not.facts().can_park);
        }
    }

    /// The same question of a real program: arithmetic waits on nothing, and a
    /// function that reads a file waits.
    #[test]
    fn a_compiled_program_agrees() {
        let src = r#"
from "core/effect" import { Alloc, Fs, Stdout };
from "core/fs" import * as fs;
from "core/host" import * as host;
from "core/io" import * as io;

export fn double(n: Int): Int { n * 2 }

export fn load<C: Alloc + Fs>(ctx: C, path: Str): Str {
  match (fs.readText(ctx, path)) { .Ok(text) => text, .Err(_) => "" }
}

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout, Fs: host.fs };
  let text = load(ctx, "a.txt");
  let _ = io.println(ctx, "${text}${double(2)}").ignore();
  .Ok(())
}
"#;
        let program = compile(src);
        let plan = run(&program);
        assert_eq!(
            plan.func(find(&program, "double")).map(|f| f.can_park),
            Some(false),
            "multiplication waits on nothing"
        );
        assert_eq!(
            plan.func(find(&program, "load")).map(|f| f.can_park),
            Some(true),
            "and a read of the host filesystem does"
        );
    }
}
