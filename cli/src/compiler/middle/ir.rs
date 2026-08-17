//! Block-argument SSA, native backends only. **Wave 1a.**
//!
//! A per-function control-flow graph of basic blocks with **block parameters**
//! and no phi instruction, every value defined once. That is Cranelift's own IR
//! shape, which is not a coincidence: it was chosen so that lowering to CLIF is
//! a transliteration and lowering to LLVM is a mechanical block-parameters-to-
//! phis rewrite, rather than a compromise between the two.
//!
//! The alternative — no shared CFG, and each native backend building its own
//! SSA — is rejected on `LLVM-tips.md:2`, "avoid mem2reg, generate optimized
//! SSA form". Cranelift's `FunctionBuilder` would build SSA for us and LLVM
//! would not, so one backend's SSA would be real and the other's an artifact of
//! `alloca`, which is exactly the divergence that makes two backends disagree.
//!
//! The JavaScript backend does not consume this, deliberately: everything it
//! needs from the shared work is in layer A, and going from a CFG back to
//! structured JavaScript needs a relooper.
//!
//! The shape, from `design/native/CODEGEN-CRANELIFT.md` §1: `Func { sig,
//! blocks, unit, facts }`, `Block { params, insts, term }`, and a `Term` of
//! `Jump` / `Branch` / `Switch` / `Return` / `Unreachable`, where `Switch`'s
//! `default` is `None` wherever the middle end proved the table total — which
//! for an enum is always.
//!
//! # Where this differs from the sketch in the design, and why
//!
//! Three places. Each is a decision rather than a drift, so each is named here
//! and a backend reading the design will find the correspondence.
//!
//!  * **A value's type is written down once.** The sketch spells a block's
//!    parameters `Vec<(ValueId, Type)>`. An instruction result needs a type
//!    too — LLVM cannot build a phi without one — so the types have to live
//!    somewhere anyway, and [`Code`] holds one row per value. A `(ValueId,
//!    Type)` pair *beside* that table is the same fact in two places, which is
//!    the skew this compiler keeps deleting (`monomorphize::DescField`,
//!    `tail_calls::Plan`). So [`Block::params`] is a list of values and
//!    [`Code::ty_of`] is how anything learns a type.
//!  * **Every edge is a [`Target`].** The sketch gives `Switch`'s default a
//!    bare `BlockId` and no arguments. One shape for all five kinds of edge
//!    means [`Term::targets`] exists, which is what a verifier, a predecessor
//!    map and both backends' phi-filling passes each want.
//!  * **Aggregates are values.** The sketch says a signature carries flattened
//!    scalar leaves (VALUE-MODEL.md §5.1). Flattening an *enum* into leaves is
//!    a statement about its bytes — the payload is a union — so it cannot be
//!    done without the layout table, and the interface wave 0 agreed for
//!    [`super::layout`] answers sizes and field offsets rather than leaves. So
//!    this IR keeps a struct, list, closure or context as one SSA value of
//!    [`Type::Agg`], carrying the source `Ty` whose layout it has, and each
//!    backend flattens at the ABI boundary from `Layouts`. A lowering that
//!    flattened would be computing the value model a second time, in the one
//!    place the design says there must be exactly one.
//!
//! Zero-sized values are kept, not dropped: `()` and a context of empty
//! implementations are ordinary values of [`Type::Unit`] and [`Type::Agg`]
//! here, and CODEGEN-CRANELIFT.md §2.2 drops them where a *signature* is
//! built, from the layout table. Dropping them here would mean lowering
//! deciding what is zero-sized, which is the same second implementation of the
//! value model.
//!
//! # What is opaque on purpose
//!
//! [`Inst::IncRef`], [`Inst::DecRef`] and [`Inst::Structural`] are placeholders
//! for passes that land later. Wave 1e's `middle::rc` emits the first two, and
//! wave 1e's `middle::derives` replaces the third with a call to a generated
//! function. They are in the instruction set now so that the backends written
//! in wave 2 have something to lower and so that the shape of the pass that
//! fills them is fixed rather than negotiated.

use std::fmt::{self, Write as _};

use crate::compiler::semantics::types::{FuncIdx, Prim, Ty};
use crate::diagnostics::{Invariant as _, Span};

// ---------------------------------------------------------------------------
// Identifiers
// ---------------------------------------------------------------------------

macro_rules! ir_id {
    ($name:ident, $doc:literal) => {
        #[doc = $doc]
        #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
        pub struct $name(pub u32);

        impl $name {
            pub fn index(self) -> usize {
                self.0 as usize
            }
        }
    };
}

ir_id!(ValueId, "One SSA value: a block parameter or an instruction result.");
ir_id!(BlockId, "One basic block within a function. `BlockId(0)` is the entry.");
ir_id!(TypeId, "One source type, interned in [`Program::types`].");

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// What a value is, at the machine level.
///
/// Scalars are the register shapes of VALUE-MODEL.md §1. Everything else is an
/// [`Type::Agg`] naming the source type whose layout it has, which is what a
/// backend hands to `Layouts::of`.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Type {
    /// `Bool` in a register. One byte in memory, values 0 and 1 only.
    I1,
    I8,
    I16,
    I32,
    I64,
    I128,
    F32,
    F64,
    /// A raw address. Produced by nothing in lowering today; the RC hooks and
    /// the backends' own open-coding are what need it to exist.
    Ptr,
    /// `()`, and anything else with no bytes. Never loaded, never stored, and
    /// dropped where a signature is built (VALUE-MODEL.md §8).
    Unit,
    /// A struct, tuple, enum, list, `Str`, closure or context: one value whose
    /// layout is the source type's.
    Agg(TypeId),
}

impl Type {
    /// The register shape of a primitive, where it has one.
    ///
    /// `Str` and `Template` are aggregates (VALUE-MODEL.md §3) and answer
    /// `None`, because naming their type needs the interner.
    pub fn of_prim(p: Prim) -> Option<Type> {
        Some(match p {
            Prim::Bool => Type::I1,
            Prim::I8 | Prim::U8 => Type::I8,
            Prim::I16 | Prim::U16 => Type::I16,
            Prim::I32 | Prim::U32 => Type::I32,
            Prim::I64 | Prim::U64 => Type::I64,
            Prim::I128 | Prim::U128 => Type::I128,
            Prim::F32 => Type::F32,
            Prim::F64 => Type::F64,
            Prim::Char => Type::I32,
            Prim::Str | Prim::Template => return None,
        })
    }

    /// Whether this is an integer a `Switch` may discriminate on.
    pub fn is_integer(self) -> bool {
        matches!(self, Type::I1 | Type::I8 | Type::I16 | Type::I32 | Type::I64 | Type::I128)
    }
}

/// One interned source type: what a backend asks the layout table about, and
/// what the printer names.
pub struct TypeInfo {
    /// The type as a program would write it — `Point`, `[Int]`, `Option<Str>`.
    /// For reading the IR, and for nothing else.
    pub name: String,
    pub ty: Ty,
}

// ---------------------------------------------------------------------------
// Instructions
// ---------------------------------------------------------------------------

/// A compile-time constant. The front end's spelling, not the target's: an
/// integer is a magnitude and a sign rather than a two's-complement bit
/// pattern, because choosing the width is the layout table's job.
#[derive(Clone, Debug)]
pub enum Const {
    Unit,
    Bool(bool),
    Int { bits: u128, negative: bool },
    Float(f64),
    /// UTF-8 bytes. A literal is `IMMORTAL` with a null `base`
    /// (VALUE-MODEL.md §3), so it touches no allocator.
    Str(String),
    Char(char),
    /// A null pointer: a closure with no environment, a literal `Str`'s base.
    Null,
    /// A value nothing reads, at the type of its result.
    ///
    /// One producer: the padding of a merged tail-call group's argument list.
    /// A member with fewer parameters than the widest one has nothing to pass
    /// for the extra slots, and the entry it selects never reads them. LLVM
    /// spells it `poison`; Cranelift has no such value and a zero of each leaf
    /// type is the honest stand-in, since the claim is that nothing observes
    /// it.
    Undef,
}

/// A one-operand primitive operation, at the type in the instruction.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum UnOp {
    Neg,
    Not,
    BitNot,
}

/// A two-operand primitive operation, at the type in the instruction.
///
/// The operand type is a `Prim` rather than a [`Type`] because signedness is
/// not a register shape: `I64` and `U64` are the same bits and different
/// division, comparison and rendering. Carrying the source primitive is one
/// field that answers all three, instead of a signed/unsigned pair per
/// operation.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    /// Truncates toward zero. Division by zero aborts (SPEC 6.2).
    Div,
    /// Takes the sign of the dividend.
    Rem,
    BitAnd,
    BitOr,
    BitXor,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

impl BinOp {
    /// Whether the result is a `Bool` rather than a value of the operand type.
    pub fn is_comparison(self) -> bool {
        matches!(self, BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge)
    }
}

/// A structural operation at a type, before `middle::derives` has generated
/// the function that implements it. **Wave 1e replaces every one of these with
/// an ordinary [`Inst::Call`]** (VALUE-MODEL.md §9), so a backend that meets
/// one has been handed a tree the native branch did not finish.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum StructuralOp {
    Eq,
    Ne,
    /// Three-way comparison, answering an `Order`.
    Cmp,
    /// `Show::show`, and the rendering of a template hole.
    Show,
    Hash,
    ToJson,
}

/// One instruction. Every one either defines its results or is executed for
/// effect; none of them transfers control, which is [`Term`]'s job.
#[derive(Clone, Debug)]
pub enum Inst {
    Const {
        dest: ValueId,
        value: Const,
    },
    Unary {
        dest: ValueId,
        op: UnOp,
        prim: Prim,
        arg: ValueId,
    },
    Binary {
        dest: ValueId,
        op: BinOp,
        prim: Prim,
        lhs: ValueId,
        rhs: ValueId,
    },

    // -- aggregates ---------------------------------------------------------
    /// A struct, tuple or context value, from its fields in declaration order.
    MakeStruct {
        dest: ValueId,
        fields: Vec<ValueId>,
    },
    /// One variant of an enum, from the variant's fields in declaration order.
    MakeEnum {
        dest: ValueId,
        variant: u32,
        fields: Vec<ValueId>,
    },
    /// A `[T]` of exactly these elements: one allocation (VALUE-MODEL.md §4).
    MakeArray {
        dest: ValueId,
        elems: Vec<ValueId>,
    },
    /// `{ code, env }` (VALUE-MODEL.md §7). `env` is `None` for a lambda that
    /// captures nothing, which is a null environment and a direct call at
    /// every site the middle end can see.
    MakeClosure {
        dest: ValueId,
        func: FuncIdx,
        env: Option<ValueId>,
    },
    /// A field of a struct, tuple or context, by declaration index.
    GetField {
        dest: ValueId,
        agg: ValueId,
        index: u32,
    },
    /// A field of the payload of a *known* variant. Reaching one whose tag is
    /// something else is a lowering bug, not a run-time condition: a payload
    /// projection is only ever emitted where a test has just established the
    /// tag (CODEGEN-CRANELIFT.md §3.1).
    GetPayload {
        dest: ValueId,
        agg: ValueId,
        variant: u32,
        index: u32,
    },
    /// The discriminant, as an `i32`. The width it is *stored* at is the
    /// layout table's answer (VALUE-MODEL.md §6) and widening on load is the
    /// backend's; the IR names the variant number, which is what a `Switch`
    /// discriminates on.
    GetTag {
        dest: ValueId,
        agg: ValueId,
    },
    /// The element count of a `[T]`. Always O(1) (VALUE-MODEL.md §4).
    ArrayLen {
        dest: ValueId,
        array: ValueId,
    },
    /// An element, with the bounds check already done. Every emission site is
    /// guarded by a comparison against [`Inst::ArrayLen`] in a dominating
    /// block, which is what `list.get`'s `Option` return means in the source.
    ArrayGet {
        dest: ValueId,
        array: ValueId,
        index: ValueId,
    },
    /// `xs[from..]`, for the `..rest` of an array pattern.
    ArraySlice {
        dest: ValueId,
        array: ValueId,
        from: ValueId,
    },

    // -- calls --------------------------------------------------------------
    /// A direct call. After monomorphization every call to a known function is
    /// one of these, because there is no dynamic dispatch in the language.
    Call {
        dests: Vec<ValueId>,
        func: FuncIdx,
        args: Vec<ValueId>,
    },
    /// A call through a closure value: load `code` and `env`, then
    /// `call_indirect` (CODEGEN-CRANELIFT.md §3.2).
    CallIndirect {
        dests: Vec<ValueId>,
        callee: ValueId,
        args: Vec<ValueId>,
    },
    /// An operation the runtime supplies, by intrinsic key — `str.concat`,
    /// `host.HostFs.readFile`. One symbol each (VALUE-MODEL.md §10).
    CallIntrinsic {
        dests: Vec<ValueId>,
        key: String,
        args: Vec<ValueId>,
    },
    /// See [`StructuralOp`]: wave 1e turns this into a [`Inst::Call`].
    Structural {
        dest: ValueId,
        op: StructuralOp,
        ty: TypeId,
        args: Vec<ValueId>,
    },

    // -- reference counting -------------------------------------------------
    /// A saturating increment of the header count (MEMORY.md §5.1). Open-coded
    /// by both backends, never called. **Emitted by wave 1e's `middle::rc`.**
    IncRef {
        value: ValueId,
    },
    /// A decrement, with the per-type `drop` to call on the cold path where
    /// the count reaches zero. **Emitted by wave 1e's `middle::rc`.**
    DecRef {
        value: ValueId,
        drop: Option<FuncIdx>,
    },

    /// `buri_abort(msg)`, which does not return (SPEC 6.10). It is an
    /// instruction rather than a terminator so that [`Term`] stays the five
    /// cases the design names; the block it appears in ends immediately, with
    /// [`Term::Unreachable`], and [`verify`] checks that.
    Abort {
        message: String,
    },
}

impl Inst {
    /// The values this instruction defines.
    pub fn results(&self) -> &[ValueId] {
        match self {
            Inst::Const { dest, .. }
            | Inst::Unary { dest, .. }
            | Inst::Binary { dest, .. }
            | Inst::MakeStruct { dest, .. }
            | Inst::MakeEnum { dest, .. }
            | Inst::MakeArray { dest, .. }
            | Inst::MakeClosure { dest, .. }
            | Inst::GetField { dest, .. }
            | Inst::GetPayload { dest, .. }
            | Inst::GetTag { dest, .. }
            | Inst::ArrayLen { dest, .. }
            | Inst::ArrayGet { dest, .. }
            | Inst::ArraySlice { dest, .. }
            | Inst::Structural { dest, .. } => std::slice::from_ref(dest),
            Inst::Call { dests, .. }
            | Inst::CallIndirect { dests, .. }
            | Inst::CallIntrinsic { dests, .. } => dests,
            Inst::IncRef { .. } | Inst::DecRef { .. } | Inst::Abort { .. } => &[],
        }
    }

    /// The values this instruction reads, in operand order.
    pub fn operands(&self, out: &mut Vec<ValueId>) {
        match self {
            Inst::Const { .. } | Inst::Abort { .. } => {}
            Inst::Unary { arg, .. } => out.push(*arg),
            Inst::Binary { lhs, rhs, .. } => {
                out.push(*lhs);
                out.push(*rhs);
            }
            Inst::MakeStruct { fields, .. } | Inst::MakeEnum { fields, .. } => {
                out.extend_from_slice(fields)
            }
            Inst::MakeArray { elems, .. } => out.extend_from_slice(elems),
            Inst::MakeClosure { env, .. } => out.extend(env.iter().copied()),
            Inst::GetField { agg, .. }
            | Inst::GetPayload { agg, .. }
            | Inst::GetTag { agg, .. } => out.push(*agg),
            Inst::ArrayLen { array, .. } => out.push(*array),
            Inst::ArrayGet { array, index: other, .. }
            | Inst::ArraySlice { array, from: other, .. } => {
                out.push(*array);
                out.push(*other);
            }
            Inst::Call { args, .. } | Inst::CallIntrinsic { args, .. } => {
                out.extend_from_slice(args)
            }
            Inst::CallIndirect { callee, args, .. } => {
                out.push(*callee);
                out.extend_from_slice(args);
            }
            Inst::Structural { args, .. } => out.extend_from_slice(args),
            Inst::IncRef { value } | Inst::DecRef { value, .. } => out.push(*value),
        }
    }
}

// ---------------------------------------------------------------------------
// Blocks and terminators
// ---------------------------------------------------------------------------

/// One edge: where control goes, and what the destination's parameters are
/// bound to on *this* edge.
///
/// Per-edge arguments are why there is no critical-edge splitting anywhere in
/// this design (CODEGEN-LLVM.md §2.1): the case that forces edge splitting in
/// a mutable-slot IR — two predecessors wanting different values in one slot —
/// is unrepresentable here.
#[derive(Clone, Debug)]
pub struct Target {
    pub block: BlockId,
    pub args: Vec<ValueId>,
}

impl Target {
    pub fn new(block: BlockId, args: Vec<ValueId>) -> Target {
        Target { block, args }
    }

    /// An edge to a block with no parameters.
    pub fn to(block: BlockId) -> Target {
        Target { block, args: Vec::new() }
    }
}

#[derive(Clone, Debug)]
pub enum Term {
    Jump(Target),
    Branch {
        cond: ValueId,
        then: Target,
        else_: Target,
    },
    /// A discriminant switch. `default` is `None` where the middle end proved
    /// the table total, which for an enum is always; `Profile::defensive_aborts`
    /// is what decides whether a backend emits an unreachable default anyway
    /// (CODEGEN-CRANELIFT.md §3.1).
    Switch {
        on: ValueId,
        cases: Vec<(u64, Target)>,
        default: Option<Target>,
    },
    Return(Vec<ValueId>),
    Unreachable,
}

impl Term {
    pub fn targets(&self) -> Vec<&Target> {
        match self {
            Term::Jump(t) => vec![t],
            Term::Branch { then, else_, .. } => vec![then, else_],
            Term::Switch { cases, default, .. } => {
                cases.iter().map(|(_, t)| t).chain(default.iter()).collect()
            }
            Term::Return(_) | Term::Unreachable => Vec::new(),
        }
    }

    /// The values read by the terminator itself, not counting block arguments.
    pub fn operands(&self, out: &mut Vec<ValueId>) {
        match self {
            Term::Jump(_) | Term::Unreachable => {}
            Term::Branch { cond, .. } => out.push(*cond),
            Term::Switch { on, .. } => out.push(*on),
            Term::Return(vs) => out.extend_from_slice(vs),
        }
    }
}

pub struct Block {
    /// The parameters, which are this block's phis in the other notation.
    /// Their types are in [`Code`], because a value's type is written down
    /// once.
    pub params: Vec<ValueId>,
    pub insts: Vec<Inst>,
    pub term: Term,
}

// ---------------------------------------------------------------------------
// Functions
// ---------------------------------------------------------------------------

/// The flattened signature. Aggregates are one entry each here and are
/// flattened into scalar leaves by the backend, from the layout table — see
/// the module header.
pub struct Sig {
    pub params: Vec<Type>,
    /// Exactly one entry today, `()` included — a zero-sized result is
    /// dropped where the machine signature is built, with the zero-sized
    /// parameters (VALUE-MODEL.md §8), rather than here. A `Vec` because
    /// VALUE-MODEL.md §5.1 returns an aggregate as its scalar leaves, and that
    /// rewrite is a backend's or a later legalization's rather than a change
    /// to this type.
    pub rets: Vec<Type>,
}

/// Whether the callee takes a reference count for a parameter or relies on the
/// caller's (MEMORY.md §5.2).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Ownership {
    Own,
    Borrow,
}

/// What SPEC 10.4's purity theorem says about a function, which is what
/// CODEGEN-LLVM.md §3.1 turns into `memory(...)`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Purity {
    /// No `ctx`, no effect-carrying `self`, cannot abort: `memory(none)`.
    Pure,
    /// Bounded only by `Alloc`, which is inaccessible memory.
    Allocating,
    /// Bounded by an observable effect: no memory attribute at all.
    Effectful,
}

/// What a backend may assume about a function.
///
/// Every field is *conservative* out of `lower`: owning every parameter,
/// `Effectful`, and abort-capable are all the answer that costs performance
/// and cannot be wrong. `middle::rc` (wave 1e) computes the ownership column
/// and the purity fixpoint; until it does, LLVM emits fewer attributes and
/// Cranelift emits more reference counting, which is the correct direction to
/// be wrong in.
///
/// `nounwind` is not a field. It is true of every function in the language —
/// there is no unwinding at all (SPEC 6.10) — and a constant stored per
/// function is a constant somebody eventually sets to `false`.
pub struct Facts {
    /// One per [`Sig::params`] entry.
    pub params: Vec<Ownership>,
    pub purity: Purity,
    /// Whether the function, or anything it calls, can reach `buri_abort`.
    pub can_abort: bool,
}

/// What a function *is*: blocks, or a symbol the runtime supplies.
///
/// The third case `monomorphize::FuncKind` has — `Unbuilt` — is not here.
/// Lowering turns one into a body that aborts, because reaching one at run
/// time is a compiler bug and an abort is what says so at the site rather than
/// at the far end of a `return 0`.
pub enum Body {
    Code(Code),
    /// An intrinsic key. The backend declares an import and defines nothing
    /// (VALUE-MODEL.md §10).
    Runtime(String),
}

/// The blocks of one function, and the type of every value in it.
pub struct Code {
    /// `blocks[0]` is the entry, and nothing branches to it: the entry's
    /// parameters are the function's parameters, and both backends forbid a
    /// branch to their entry block. A loop header is therefore always a second
    /// block (see `lower`'s tail-call loops).
    pub blocks: Vec<Block>,
    /// One row per value, by id. A value is defined exactly once, so this is
    /// the one place its type is written down.
    values: Vec<Type>,
}

impl Code {
    pub fn new() -> Code {
        Code { blocks: Vec::new(), values: Vec::new() }
    }

    /// The type of a value.
    ///
    /// Every id was minted by [`Code::value`] or [`Code::block`], both of
    /// which push a row, and nothing removes one — [`Code::retain_reachable`]
    /// drops blocks and leaves the value table alone precisely so that this
    /// stays true.
    pub fn ty_of(&self, v: ValueId) -> Type {
        *self.values.get(v.index()).or_ice("every ValueId was minted by `Code::value`")
    }

    pub fn values(&self) -> usize {
        self.values.len()
    }

    /// Mints a value of a type.
    pub fn value(&mut self, ty: Type) -> ValueId {
        let id = ValueId(self.values.len() as u32);
        self.values.push(ty);
        id
    }

    /// Appends a block taking these parameter types, and returns it. The
    /// parameters are minted here, so a block's parameters and their types
    /// cannot fall out of step.
    pub fn block(&mut self, params: &[Type]) -> BlockId {
        let ps: Vec<ValueId> = params.iter().map(|t| self.value(*t)).collect();
        let id = BlockId(self.blocks.len() as u32);
        self.blocks.push(Block { params: ps, insts: Vec::new(), term: Term::Unreachable });
        id
    }

    pub fn get(&self, b: BlockId) -> &Block {
        self.blocks.get(b.index()).or_ice("every BlockId was minted by `Code::block`")
    }

    pub fn get_mut(&mut self, b: BlockId) -> &mut Block {
        self.blocks.get_mut(b.index()).or_ice("every BlockId was minted by `Code::block`")
    }

    /// The predecessors of every block, in block order.
    pub fn preds(&self) -> Vec<Vec<BlockId>> {
        let mut preds = vec![Vec::new(); self.blocks.len()];
        for (i, b) in self.blocks.iter().enumerate() {
            for t in b.term.targets() {
                if let Some(p) = preds.get_mut(t.block.index()) {
                    p.push(BlockId(i as u32));
                }
            }
        }
        preds
    }

    /// Which blocks the entry reaches.
    pub fn reachable(&self) -> Vec<bool> {
        let mut seen = vec![false; self.blocks.len()];
        if self.blocks.is_empty() {
            return seen;
        }
        let mut stack = vec![BlockId(0)];
        if let Some(s) = seen.get_mut(0) {
            *s = true;
        }
        while let Some(b) = stack.pop() {
            for t in self.get(b).term.targets() {
                match seen.get_mut(t.block.index()) {
                    Some(s) if !*s => {
                        *s = true;
                        stack.push(t.block);
                    }
                    _ => {}
                }
            }
        }
        seen
    }

    /// Drops every block the entry does not reach, renumbering the rest in
    /// place.
    ///
    /// Lowering produces unreachable blocks routinely and on purpose: an
    /// expression after an `abort`, a `match` arm after one that diverges, and
    /// the continuation of a tail call that became a back edge are all lowered
    /// into a fresh block nothing jumps to, which is what lets an expression
    /// that does not return a value still *be* a value in the tree. Removing
    /// them here rather than teaching every producer to stop early keeps the
    /// producers straight-line, and it is why the printed CFG has no dead
    /// blocks in it.
    ///
    /// The value table is left alone. A value defined in a dropped block is
    /// used only in dropped blocks — that is what dominance means — so nothing
    /// dangles, and renumbering values would invalidate every id a caller
    /// holds for no gain.
    pub fn retain_reachable(&mut self) {
        let keep = self.reachable();
        if keep.iter().all(|k| *k) {
            return;
        }
        let mut renumber = vec![None; self.blocks.len()];
        let mut next = 0u32;
        for (i, k) in keep.iter().enumerate() {
            if *k {
                if let Some(slot) = renumber.get_mut(i) {
                    *slot = Some(BlockId(next));
                }
                next = next.saturating_add(1);
            }
        }
        let mut kept = keep.iter();
        self.blocks.retain(|_| kept.next().copied().unwrap_or(false));
        for b in &mut self.blocks {
            let retarget = |t: &mut Target| {
                t.block = renumber
                    .get(t.block.index())
                    .copied()
                    .flatten()
                    .or_ice("a reachable block's successors are reachable");
            };
            match &mut b.term {
                Term::Jump(t) => retarget(t),
                Term::Branch { then, else_, .. } => {
                    retarget(then);
                    retarget(else_);
                }
                Term::Switch { cases, default, .. } => {
                    for (_, t) in cases.iter_mut() {
                        retarget(t);
                    }
                    if let Some(t) = default {
                        retarget(t);
                    }
                }
                Term::Return(_) | Term::Unreachable => {}
            }
        }
    }
}

impl Default for Code {
    fn default() -> Code {
        Code::new()
    }
}

pub struct Func {
    /// The symbol the linker sees, from `monomorphize::Func::symbol`.
    pub symbol: String,
    /// `module:owner.name`, for a backtrace and for the printer.
    pub debug_name: String,
    pub sig: Sig,
    pub facts: Facts,
    /// The codegen unit this function belongs to: an index into
    /// [`Program::units`] (ARCHITECTURE.md §5).
    pub unit: u32,
    pub body: Body,
    pub span: Span,
}

impl Func {
    pub fn code(&self) -> Option<&Code> {
        match &self.body {
            Body::Code(c) => Some(c),
            Body::Runtime(_) => None,
        }
    }

    pub fn intrinsic_key(&self) -> Option<&str> {
        match &self.body {
            Body::Runtime(k) => Some(k),
            Body::Code(_) => None,
        }
    }
}

/// One program's worth of CFGs: what `middle::lower` produces and what both
/// native backends consume.
pub struct Program {
    /// One per `monomorphize::Func`, at the same index, so a `FuncIdx` in an
    /// [`Inst::Call`] means the same thing on both sides of the lowering.
    pub funcs: Vec<Func>,
    /// Codegen unit names, in first-appearance order: `core_list`, `main`.
    /// The object file for unit `u` is `units[u].o` (ARCHITECTURE.md §6.3).
    pub units: Vec<String>,
    /// Every source type the IR names, interned.
    pub types: Vec<TypeInfo>,
}

impl Program {
    pub fn type_info(&self, id: TypeId) -> &TypeInfo {
        self.types.get(id.index()).or_ice("every TypeId was minted by the lowering's interner")
    }

    pub fn unit_name(&self, unit: u32) -> &str {
        self.units.get(unit as usize).map(String::as_str).unwrap_or("?")
    }
}

// ---------------------------------------------------------------------------
// Verification
// ---------------------------------------------------------------------------

/// Every way one function can be malformed, as sentences.
///
/// This is not a debug assertion that fires in a developer's build and is
/// absent in a user's: it is a function the tests call on real lowered
/// programs, and a backend may call it behind `cfg!(debug_assertions)` the way
/// CODEGEN-CRANELIFT.md §4 sets Cranelift's own verifier.
///
/// What it checks, and why each one is a bug worth a check rather than a
/// convention worth a comment:
///
///  * **Every edge's arguments match the destination's parameters**, in count
///    and in type. This is the whole of the block-parameter contract, and it
///    is what LLVM's phi construction reads directly (CODEGEN-LLVM.md §2.1) —
///    a mismatch there is an `add_incoming` with the wrong arity, which LLVM
///    accepts and miscompiles.
///  * **Every value is defined before it is used**, in the dominance sense.
///    An SSA form where that does not hold is one where Cranelift's verifier
///    reports "instruction result used before definition" a wave later, with
///    no lowering site to point at.
///  * **Every value is defined exactly once.**
///  * **An abort ends its block.** `buri_abort` does not return, so anything
///    after it in the same block is unreachable code the backends would have
///    to invent a rule for.
///
/// Critical edges are *not* checked for, because the design does not forbid
/// them: a per-edge argument list is what makes them harmless
/// (CODEGEN-LLVM.md §2.1).
pub fn verify_func(program: &Program, func: &Func) -> Vec<String> {
    let mut errs = Vec::new();
    let Some(code) = func.code() else { return errs };
    let name = &func.debug_name;

    if code.blocks.is_empty() {
        errs.push(format!("{name}: a function with a body has no entry block"));
        return errs;
    }

    // Entry parameters are the signature's parameters.
    let entry = code.get(BlockId(0));
    if entry.params.len() != func.sig.params.len() {
        errs.push(format!(
            "{name}: the entry block takes {} parameters and the signature declares {}",
            entry.params.len(),
            func.sig.params.len()
        ));
    }
    for (i, (p, t)) in entry.params.iter().zip(func.sig.params.iter()).enumerate() {
        if code.ty_of(*p) != *t {
            errs.push(format!(
                "{name}: entry parameter {i} is {:?} and the signature says {:?}",
                code.ty_of(*p),
                t
            ));
        }
    }

    // One definition per value, and where.
    //
    // A position is `0` for a block parameter and `1 + index` for an
    // instruction result, so that "defined earlier in the same block" is a
    // comparison rather than two cases.
    let mut def: Vec<Option<(usize, usize)>> = vec![None; code.values()];
    let mut define = |v: ValueId, at: (usize, usize), errs: &mut Vec<String>| {
        match def.get_mut(v.index()) {
            Some(slot @ None) => *slot = Some(at),
            Some(Some(_)) => errs.push(format!("{name}: v{} is defined twice", v.0)),
            None => errs.push(format!("{name}: v{} has no type", v.0)),
        }
    };
    for (bi, b) in code.blocks.iter().enumerate() {
        for p in &b.params {
            define(*p, (bi, 0), &mut errs);
        }
        for (ii, inst) in b.insts.iter().enumerate() {
            for r in inst.results() {
                define(*r, (bi, ii.saturating_add(1)), &mut errs);
            }
        }
    }

    // Aborts end their block, and nothing else does; a direct call agrees
    // with the callee's signature.
    for (bi, b) in code.blocks.iter().enumerate() {
        for (ii, inst) in b.insts.iter().enumerate() {
            if let Inst::Call { dests, func: callee, args } = inst {
                match program.funcs.get(callee.index()) {
                    Some(c) => {
                        if args.len() != c.sig.params.len() {
                            errs.push(format!(
                                "{name}: b{bi} calls {} with {} arguments and it takes {}",
                                c.debug_name,
                                args.len(),
                                c.sig.params.len()
                            ));
                        }
                        if dests.len() != c.sig.rets.len() {
                            errs.push(format!(
                                "{name}: b{bi} takes {} results from {}, which returns {}",
                                dests.len(),
                                c.debug_name,
                                c.sig.rets.len()
                            ));
                        }
                    }
                    None => errs.push(format!("{name}: b{bi} calls f{}, which is not a function in this program", callee.0)),
                }
            }
            if matches!(inst, Inst::Abort { .. }) {
                if ii.saturating_add(1) != b.insts.len() {
                    errs.push(format!("{name}: b{bi} continues after an abort"));
                }
                if !matches!(b.term, Term::Unreachable) {
                    errs.push(format!("{name}: b{bi} aborts and does not end unreachable"));
                }
            }
        }
    }

    let dom = dominators(code);

    // Edges, and uses.
    let mut uses: Vec<ValueId> = Vec::new();
    for (bi, b) in code.blocks.iter().enumerate() {
        let check_use = |v: ValueId, at: (usize, usize), errs: &mut Vec<String>| {
            let Some(Some((db, dp))) = def.get(v.index()).copied() else {
                errs.push(format!("{name}: b{bi} uses v{}, which nothing defines", v.0));
                return;
            };
            let ok = if db == at.0 {
                dp < at.1
            } else {
                dom.get(at.0).is_some_and(|d| d.get(db).copied().unwrap_or(false))
            };
            if !ok {
                errs.push(format!(
                    "{name}: b{bi} uses v{}, defined in b{db}, which does not dominate it",
                    v.0
                ));
            }
        };

        for (ii, inst) in b.insts.iter().enumerate() {
            uses.clear();
            inst.operands(&mut uses);
            for v in &uses {
                check_use(*v, (bi, ii.saturating_add(1)), &mut errs);
            }
        }
        let end = b.insts.len().saturating_add(1);
        uses.clear();
        b.term.operands(&mut uses);
        for v in &uses {
            check_use(*v, (bi, end), &mut errs);
        }
        for t in b.term.targets() {
            for v in &t.args {
                check_use(*v, (bi, end), &mut errs);
            }
        }
    }

    for (bi, b) in code.blocks.iter().enumerate() {
        if let Term::Branch { cond, .. } = &b.term {
            if code.ty_of(*cond) != Type::I1 {
                errs.push(format!("{name}: b{bi} branches on a value that is not a Bool"));
            }
        }
        if let Term::Switch { on, cases, .. } = &b.term {
            if !code.ty_of(*on).is_integer() {
                errs.push(format!("{name}: b{bi} switches on a value that is not an integer"));
            }
            let mut seen: Vec<u64> = cases.iter().map(|(k, _)| *k).collect();
            seen.sort_unstable();
            let before = seen.len();
            seen.dedup();
            if seen.len() != before {
                errs.push(format!("{name}: b{bi} switches on a duplicated case value"));
            }
        }
        if let Term::Return(vs) = &b.term {
            if vs.len() != func.sig.rets.len() {
                errs.push(format!(
                    "{name}: b{bi} returns {} values and the signature declares {}",
                    vs.len(),
                    func.sig.rets.len()
                ));
            }
            for (v, t) in vs.iter().zip(func.sig.rets.iter()) {
                if code.ty_of(*v) != *t {
                    errs.push(format!("{name}: b{bi} returns a value of the wrong type"));
                }
            }
        }
        for t in b.term.targets() {
            let Some(dest) = code.blocks.get(t.block.index()) else {
                errs.push(format!("{name}: b{bi} jumps to b{}, which does not exist", t.block.0));
                continue;
            };
            if dest.params.len() != t.args.len() {
                errs.push(format!(
                    "{name}: b{bi} passes {} arguments to b{}, which takes {}",
                    t.args.len(),
                    t.block.0,
                    dest.params.len()
                ));
                continue;
            }
            for (a, p) in t.args.iter().zip(dest.params.iter()) {
                if code.ty_of(*a) != code.ty_of(*p) {
                    errs.push(format!(
                        "{name}: b{bi} passes v{} to b{}, whose parameter is a different type",
                        a.0, t.block.0
                    ));
                }
            }
        }
    }

    errs
}

/// Every problem in the program, function by function.
pub fn verify(program: &Program) -> Vec<String> {
    program.funcs.iter().flat_map(|f| verify_func(program, f)).collect()
}

/// The dominator sets, as one row of flags per block.
///
/// The textbook fixpoint rather than Lengauer-Tarjan: this runs over one
/// function's blocks, of which there are tens, and it is called from a
/// verifier rather than from a hot pass. A block the entry does not reach
/// dominates nothing and is dominated by nothing, which is the right answer
/// for a verifier that must not accuse dead code of anything.
fn dominators(code: &Code) -> Vec<Vec<bool>> {
    let n = code.blocks.len();
    let reachable = code.reachable();
    let preds = code.preds();
    let mut dom: Vec<Vec<bool>> = (0..n)
        .map(|i| {
            if i == 0 {
                let mut row = vec![false; n];
                if let Some(s) = row.get_mut(0) {
                    *s = true;
                }
                row
            } else if reachable.get(i).copied().unwrap_or(false) {
                vec![true; n]
            } else {
                vec![false; n]
            }
        })
        .collect();

    let mut changed = true;
    while changed {
        changed = false;
        for b in 1..n {
            if !reachable.get(b).copied().unwrap_or(false) {
                continue;
            }
            let mut next = vec![true; n];
            let mut any = false;
            for p in preds.get(b).map(Vec::as_slice).unwrap_or_default() {
                if !reachable.get(p.index()).copied().unwrap_or(false) {
                    continue;
                }
                let Some(row) = dom.get(p.index()) else { continue };
                any = true;
                for (slot, d) in next.iter_mut().zip(row.iter()) {
                    *slot = *slot && *d;
                }
            }
            if !any {
                next = vec![false; n];
            }
            if let Some(slot) = next.get_mut(b) {
                *slot = true;
            }
            if dom.get(b) != Some(&next) {
                if let Some(row) = dom.get_mut(b) {
                    *row = next;
                }
                changed = true;
            }
        }
    }
    dom
}

// ---------------------------------------------------------------------------
// Printing
// ---------------------------------------------------------------------------

/// The IR as text, which is the form a human reads and a cache key hashes.
///
/// Wave 2c's `codegen` key is `H(the unit's lowered IR)` (ARCHITECTURE.md
/// §6.2), and this rendering is a faithful, total and deterministic function
/// of the IR — no hash order anywhere, every name derived from the program —
/// so hashing these bytes per unit is a correct way to compute it and is the
/// one that can be inspected when a key changes and nobody knows why.
impl fmt::Display for Program {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for func in &self.funcs {
            write!(f, "{}", self.render_func(func))?;
        }
        Ok(())
    }
}

impl Program {
    /// One function, as text.
    pub fn render_func(&self, func: &Func) -> String {
        let mut out = String::new();
        let _ = writeln!(out, "; {} [unit {}]", func.debug_name, self.unit_name(func.unit));
        let params: Vec<String> = func.sig.params.iter().map(|t| self.ty(*t)).collect();
        let rets: Vec<String> = func.sig.rets.iter().map(|t| self.ty(*t)).collect();
        let ret = match rets.as_slice() {
            [] => String::new(),
            [one] => format!(" -> {one}"),
            many => format!(" -> ({})", many.join(", ")),
        };
        let code = match &func.body {
            Body::Runtime(key) => {
                let _ = writeln!(out, "fn {}({}){ret} = runtime {key:?}", func.symbol, params.join(", "));
                return out;
            }
            Body::Code(c) => c,
        };
        let _ = writeln!(out, "fn {}({}){ret} {{", func.symbol, params.join(", "));
        for (i, b) in code.blocks.iter().enumerate() {
            let ps: Vec<String> =
                b.params.iter().map(|p| format!("v{}: {}", p.0, self.ty(code.ty_of(*p)))).collect();
            // The parameter list is printed even when it is empty, so that a
            // block header and the edges naming it have the same shape.
            let _ = writeln!(out, "  b{i}({}):", ps.join(", "));
            for inst in &b.insts {
                let _ = writeln!(out, "    {}", self.inst(inst));
            }
            let _ = writeln!(out, "    {}", term(&b.term));
        }
        let _ = writeln!(out, "}}");
        out
    }

    fn ty(&self, t: Type) -> String {
        match t {
            Type::I1 => "i1".into(),
            Type::I8 => "i8".into(),
            Type::I16 => "i16".into(),
            Type::I32 => "i32".into(),
            Type::I64 => "i64".into(),
            Type::I128 => "i128".into(),
            Type::F32 => "f32".into(),
            Type::F64 => "f64".into(),
            Type::Ptr => "ptr".into(),
            Type::Unit => "unit".into(),
            Type::Agg(id) => self.type_info(id).name.clone(),
        }
    }

    fn inst(&self, inst: &Inst) -> String {
        let vals = |vs: &[ValueId]| {
            vs.iter().map(|v| format!("v{}", v.0)).collect::<Vec<String>>().join(", ")
        };
        let call = |dests: &[ValueId], text: String| match dests {
            [] => text,
            ds => format!("{} = {text}", vals(ds)),
        };
        match inst {
            Inst::Const { dest, value } => format!("v{} = const {}", dest.0, konst(value)),
            Inst::Unary { dest, op, prim, arg } => {
                format!("v{} = {}.{} v{}", dest.0, un_op(*op), prim.name(), arg.0)
            }
            Inst::Binary { dest, op, prim, lhs, rhs } => {
                format!("v{} = {}.{} v{}, v{}", dest.0, bin_op(*op), prim.name(), lhs.0, rhs.0)
            }
            Inst::MakeStruct { dest, fields } => {
                format!("v{} = make {}", dest.0, wrap(vals(fields)))
            }
            Inst::MakeEnum { dest, variant, fields } => {
                format!("v{} = make #{variant} {}", dest.0, wrap(vals(fields)))
            }
            Inst::MakeArray { dest, elems } => format!("v{} = array {}", dest.0, wrap(vals(elems))),
            Inst::MakeClosure { dest, func, env } => format!(
                "v{} = closure f{}, {}",
                dest.0,
                func.0,
                env.map(|e| format!("v{}", e.0)).unwrap_or_else(|| "null".into())
            ),
            Inst::GetField { dest, agg, index } => {
                format!("v{} = field.{index} v{}", dest.0, agg.0)
            }
            Inst::GetPayload { dest, agg, variant, index } => {
                format!("v{} = payload.#{variant}.{index} v{}", dest.0, agg.0)
            }
            Inst::GetTag { dest, agg } => format!("v{} = tag v{}", dest.0, agg.0),
            Inst::ArrayLen { dest, array } => format!("v{} = len v{}", dest.0, array.0),
            Inst::ArrayGet { dest, array, index } => {
                format!("v{} = elem v{}, v{}", dest.0, array.0, index.0)
            }
            Inst::ArraySlice { dest, array, from } => {
                format!("v{} = slice v{}, v{}", dest.0, array.0, from.0)
            }
            Inst::Call { dests, func, args } => {
                call(dests, format!("call f{}{}", func.0, wrap(vals(args))))
            }
            Inst::CallIndirect { dests, callee, args } => {
                call(dests, format!("call_indirect v{}{}", callee.0, wrap(vals(args))))
            }
            Inst::CallIntrinsic { dests, key, args } => {
                call(dests, format!("intrinsic {key:?}{}", wrap(vals(args))))
            }
            Inst::Structural { dest, op, ty, args } => format!(
                "v{} = structural.{} {}{}",
                dest.0,
                structural_op(*op),
                self.type_info(*ty).name,
                wrap(vals(args))
            ),
            Inst::IncRef { value } => format!("incref v{}", value.0),
            Inst::DecRef { value, drop } => match drop {
                Some(d) => format!("decref v{}, drop f{}", value.0, d.0),
                None => format!("decref v{}", value.0),
            },
            Inst::Abort { message } => format!("abort {message:?}"),
        }
    }
}

fn wrap(args: String) -> String {
    format!("({args})")
}

fn target(t: &Target) -> String {
    let args: Vec<String> = t.args.iter().map(|v| format!("v{}", v.0)).collect();
    format!("b{}{}", t.block.0, wrap(args.join(", ")))
}

fn term(t: &Term) -> String {
    match t {
        Term::Jump(to) => format!("jump {}", target(to)),
        Term::Branch { cond, then, else_ } => {
            format!("branch v{}, {}, {}", cond.0, target(then), target(else_))
        }
        Term::Switch { on, cases, default } => {
            let cs: Vec<String> =
                cases.iter().map(|(k, t)| format!("{k} -> {}", target(t))).collect();
            let d = match default {
                Some(t) => format!(", default {}", target(t)),
                None => String::new(),
            };
            format!("switch v{}, [{}]{d}", on.0, cs.join(", "))
        }
        Term::Return(vs) => {
            let args: Vec<String> = vs.iter().map(|v| format!("v{}", v.0)).collect();
            format!("return {}", args.join(", ")).trim_end().to_string()
        }
        Term::Unreachable => "unreachable".into(),
    }
}

fn konst(c: &Const) -> String {
    match c {
        Const::Unit => "()".into(),
        Const::Bool(b) => format!("{b}"),
        Const::Int { bits, negative } => {
            if *negative {
                format!("-{bits}")
            } else {
                format!("{bits}")
            }
        }
        Const::Float(v) => format!("{v:?}"),
        Const::Str(s) => format!("{s:?}"),
        Const::Char(c) => format!("{c:?}"),
        Const::Null => "null".into(),
        Const::Undef => "undef".into(),
    }
}

fn un_op(op: UnOp) -> &'static str {
    match op {
        UnOp::Neg => "neg",
        UnOp::Not => "not",
        UnOp::BitNot => "bitnot",
    }
}

fn bin_op(op: BinOp) -> &'static str {
    match op {
        BinOp::Add => "add",
        BinOp::Sub => "sub",
        BinOp::Mul => "mul",
        BinOp::Div => "div",
        BinOp::Rem => "rem",
        BinOp::BitAnd => "and",
        BinOp::BitOr => "or",
        BinOp::BitXor => "xor",
        BinOp::Eq => "eq",
        BinOp::Ne => "ne",
        BinOp::Lt => "lt",
        BinOp::Le => "le",
        BinOp::Gt => "gt",
        BinOp::Ge => "ge",
    }
}

fn structural_op(op: StructuralOp) -> &'static str {
    match op {
        StructuralOp::Eq => "eq",
        StructuralOp::Ne => "ne",
        StructuralOp::Cmp => "cmp",
        StructuralOp::Show => "show",
        StructuralOp::Hash => "hash",
        StructuralOp::ToJson => "toJson",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A function with one block that adds its two parameters, built by hand
    /// so that the printer and the verifier are tested without a front end in
    /// the way.
    fn adder() -> Program {
        let mut code = Code::new();
        let entry = code.block(&[Type::I64, Type::I64]);
        let (a, b) = {
            let ps = &code.get(entry).params;
            (
                *ps.first().or_ice("the entry was built with two parameters"),
                *ps.get(1).or_ice("the entry was built with two parameters"),
            )
        };
        let sum = code.value(Type::I64);
        code.get_mut(entry).insts.push(Inst::Binary {
            dest: sum,
            op: BinOp::Add,
            prim: Prim::I64,
            lhs: a,
            rhs: b,
        });
        code.get_mut(entry).term = Term::Return(vec![sum]);
        Program {
            funcs: vec![Func {
                symbol: "m$add".into(),
                debug_name: "m:add".into(),
                sig: Sig { params: vec![Type::I64, Type::I64], rets: vec![Type::I64] },
                facts: Facts {
                    params: vec![Ownership::Own, Ownership::Own],
                    purity: Purity::Effectful,
                    can_abort: true,
                },
                unit: 0,
                body: Body::Code(code),
                span: Span::NONE,
            }],
            units: vec!["m".into()],
            types: Vec::new(),
        }
    }

    #[test]
    fn a_well_formed_function_verifies_and_prints() {
        let p = adder();
        assert_eq!(verify(&p), Vec::<String>::new());
        assert_eq!(
            p.to_string(),
            "; m:add [unit m]\n\
             fn m$add(i64, i64) -> i64 {\n\
             \x20 b0(v0: i64, v1: i64):\n\
             \x20   v2 = add.I64 v0, v1\n\
             \x20   return v2\n\
             }\n"
        );
    }

    #[test]
    fn a_use_of_a_value_from_a_block_that_does_not_dominate_is_reported() {
        let mut p = adder();
        let Body::Code(code) = &mut p.funcs.first_mut().or_ice("one function").body else {
            return;
        };
        // b1 defines a value; b2 uses it; b0 branches to both, so b1 does not
        // dominate b2.
        let b1 = code.block(&[]);
        let b2 = code.block(&[]);
        let v = code.value(Type::I64);
        code.get_mut(b1).insts.push(Inst::Const {
            dest: v,
            value: Const::Int { bits: 1, negative: false },
        });
        code.get_mut(b1).term = Term::Return(vec![v]);
        code.get_mut(b2).term = Term::Return(vec![v]);
        let cond = code.value(Type::I1);
        code.get_mut(BlockId(0)).insts.push(Inst::Const { dest: cond, value: Const::Bool(true) });
        code.get_mut(BlockId(0)).term =
            Term::Branch { cond, then: Target::to(b1), else_: Target::to(b2) };
        let errs = verify(&p);
        assert_eq!(errs.len(), 1, "{errs:?}");
        assert!(errs.iter().any(|e| e.contains("does not dominate")), "{errs:?}");
    }

    #[test]
    fn an_edge_whose_arguments_do_not_match_its_destination_is_reported() {
        let mut p = adder();
        let Body::Code(code) = &mut p.funcs.first_mut().or_ice("one function").body else {
            return;
        };
        let b1 = code.block(&[Type::I64, Type::I64]);
        code.get_mut(b1).term = Term::Unreachable;
        let entry_param = *code.get(BlockId(0)).params.first().or_ice("two parameters");
        code.get_mut(BlockId(0)).term = Term::Jump(Target::new(b1, vec![entry_param]));
        let errs = verify(&p);
        assert!(errs.iter().any(|e| e.contains("passes 1 arguments")), "{errs:?}");
    }

    #[test]
    fn an_unreachable_block_is_dropped_and_the_rest_renumbered() {
        let mut code = Code::new();
        let entry = code.block(&[]);
        let dead = code.block(&[]);
        let live = code.block(&[]);
        code.get_mut(entry).term = Term::Jump(Target::to(live));
        code.get_mut(dead).term = Term::Jump(Target::to(live));
        code.get_mut(live).term = Term::Return(Vec::new());
        code.retain_reachable();
        assert_eq!(code.blocks.len(), 2);
        // `live` was b2 and is now b1, and the entry's jump names the new one.
        match &code.get(BlockId(0)).term {
            Term::Jump(t) => assert_eq!(t.block, BlockId(1)),
            other => panic!("expected a jump, got {other:?}"),
        }
    }

    #[test]
    fn a_block_that_continues_after_an_abort_is_reported() {
        let mut p = adder();
        let Body::Code(code) = &mut p.funcs.first_mut().or_ice("one function").body else {
            return;
        };
        let v = code.value(Type::I64);
        let entry = code.get_mut(BlockId(0));
        entry.insts.insert(0, Inst::Abort { message: "boom".into() });
        entry.insts.push(Inst::Const { dest: v, value: Const::Int { bits: 0, negative: false } });
        let errs = verify(&p);
        assert!(errs.iter().any(|e| e.contains("continues after an abort")), "{errs:?}");
        assert!(errs.iter().any(|e| e.contains("does not end unreachable")), "{errs:?}");
    }
}
