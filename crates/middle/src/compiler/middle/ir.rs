//! Block-argument SSA, native backends only.
//!
//! A per-function control-flow graph of basic blocks with **block parameters**
//! and no phi instruction, every value defined once. It is the shape a
//! retargetable code generator wants, which is not a coincidence: it was chosen
//! so that lowering to a machine backend is a transliteration and lowering to
//! LLVM is a mechanical block-parameters-to-phis rewrite, rather than a
//! compromise between the two.
//!
//! The alternative — no shared CFG, and each native backend building its own
//! SSA — is rejected on `design/native/CODEGEN-LLVM.md` §0's second
//! instruction, "avoid `mem2reg`; generate optimized SSA form". A backend with
//! an SSA-building frontend would build SSA for us and LLVM would not, so one
//! backend's SSA would be real and the other's an artifact of `alloca`, which
//! is exactly the divergence that makes two backends disagree.
//!
//! The JavaScript backend does not consume this, deliberately: everything it
//! needs from the shared work is in layer A, and going from a CFG back to
//! structured JavaScript needs a relooper.
//!
//! The shape (`design/native/ARCHITECTURE.md` §2.1): `Func { sig,
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
//!    done without the layout table, and the interface [`super::layout`]
//!    presents answers sizes and field offsets rather than leaves. So
//!    this IR keeps a struct, list, closure or context as one SSA value of
//!    [`Type::Agg`], carrying the source `Ty` whose layout it has, and each
//!    backend flattens at the ABI boundary from `Layouts`. A lowering that
//!    flattened would be computing the value model a second time, in the one
//!    place the design says there must be exactly one.
//!
//! Zero-sized values are kept, not dropped: `()` and a context of empty
//! implementations are ordinary values of [`Type::Unit`] and [`Type::Agg`]
//! here, and a backend drops them where a *signature* is built, from the
//! layout table. Dropping them here would mean lowering
//! deciding what is zero-sized, which is the same second implementation of the
//! value model.
//!
//! # What is opaque on purpose
//!
//! [`Inst::IncRef`], [`Inst::DecRef`] and [`Inst::Structural`] carry no
//! meaning of their own here. `middle::rc` decides where the first two go, and
//! `middle::derives` replaces the third with a call to a generated function.
//! They are in the instruction set rather than in those passes' private
//! vocabularies so that the backends have one shape to lower and the passes
//! that fill them write to a fixed target rather than a negotiated one.

use std::fmt::{self, Write as _};

use crate::compiler::semantics::typed::Magnitude;
use crate::compiler::semantics::types::{FuncIdx, Prim, Ty};
use crate::hash::Map as HashMap;
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
    Int { bits: Magnitude, negative: bool },
    Float(f64),
    /// UTF-8 bytes. A literal is `IMMORTAL` with a null `base`
    /// (VALUE-MODEL.md §3), so it touches no allocator.
    Str(Box<str>),
    Char(char),
    /// A null pointer: a closure with no environment, a literal `Str`'s base.
    Null,
    /// A value nothing reads, at the type of its result.
    ///
    /// One producer: the padding of a merged tail-call group's argument list.
    /// A member with fewer parameters than the widest one has nothing to pass
    /// for the extra slots, and the entry it selects never reads them. LLVM
    /// spells it `poison`; a machine backend has no such value and a zero of
    /// each leaf type is the honest stand-in, since the claim is that nothing observes
    /// it.
    Undef,
}

/// A one-operand primitive operation, at the type in the instruction.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum UnOp {
    Neg,
    Not,
    BitNot,
    /// A `Bool` as the `I64` it counts as, `0` or `1`; `prim` is `Bool`. What
    /// `list.count` adds per element, so that the count needs no branch.
    FromBool,
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
/// the function that implements it. **`middle::derives` replaces every one of
/// these with an ordinary [`Inst::Call`]** (VALUE-MODEL.md §9), so a backend
/// that meets one has been handed a tree the native branch did not finish.
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
    /// tag.
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
    /// A fresh `[T]` of `len` elements that nothing has stored yet. A loop
    /// fills it with [`Inst::ArraySet`] before anything else reads it, and
    /// [`Inst::ArrayPrefix`] is how a loop that filled fewer says so. The
    /// three are what `lower`'s `core/list` loops build their answers from
    /// (`lower/lists.rs`).
    ///
    /// No elements is no allocation, as for [`Inst::MakeArray`].
    ArrayAlloc {
        dest: ValueId,
        len: ValueId,
    },
    /// Stores `value` as element `index` of a block an [`Inst::ArrayAlloc`]
    /// in this function made, which nothing else holds yet. The value's counts
    /// move into the block.
    ArraySet {
        array: ValueId,
        index: ValueId,
        value: ValueId,
    },
    /// The first `len` elements of such a block, which a loop stored, as the
    /// list. `array` is consumed. A backend may keep the block or copy the
    /// prefix into an exact one, and nothing reads the elements past `len`.
    ArrayPrefix {
        dest: ValueId,
        array: ValueId,
        len: ValueId,
    },

    // -- calls --------------------------------------------------------------
    /// A direct call. After monomorphization every call to a known function is
    /// one of these, because there is no dynamic dispatch in the language.
    Call {
        dest: ValueId,
        func: FuncIdx,
        args: Vec<ValueId>,
    },
    /// A call through a closure value: load `code` and `env`, then
    /// `call_indirect`.
    CallIndirect {
        dest: ValueId,
        callee: ValueId,
        args: Vec<ValueId>,
    },
    /// An operation the runtime supplies, by intrinsic key — `str.concat`,
    /// `host.HostFileSystem.readFile`. One symbol each (VALUE-MODEL.md §10).
    ///
    /// The key and the arguments are boxed slices rather than a `String` and
    /// a `Vec`, which keeps this variant inside the 40 bytes every other
    /// instruction fits in.
    CallIntrinsic {
        dest: ValueId,
        key: Box<str>,
        args: Box<[ValueId]>,
    },
    /// See [`StructuralOp`]: `middle::derives` turns this into a
    /// [`Inst::Call`].
    Structural {
        dest: ValueId,
        op: StructuralOp,
        ty: TypeId,
        args: Vec<ValueId>,
    },

    // -- reference counting -------------------------------------------------
    /// A saturating increment of the header count (MEMORY.md §5.1). Open-coded
    /// by both backends, never called. **Placed from `middle::rc`'s plan.**
    IncRef {
        value: ValueId,
    },
    /// A decrement, with the per-type `drop` to call on the cold path where
    /// the count reaches zero. **Placed from `middle::rc`'s plan.**
    DecRef {
        value: ValueId,
        drop: Option<FuncIdx>,
    },

    /// `buri_abort(msg)`, which does not return (SPEC 6.9). It is an
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
            | Inst::ArrayAlloc { dest, .. }
            | Inst::ArrayPrefix { dest, .. }
            | Inst::Structural { dest, .. }
            | Inst::Call { dest, .. }
            | Inst::CallIndirect { dest, .. }
            | Inst::CallIntrinsic { dest, .. } => std::slice::from_ref(dest),
            Inst::IncRef { .. }
            | Inst::DecRef { .. }
            | Inst::Abort { .. }
            | Inst::ArraySet { .. } => &[],
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
            | Inst::ArraySlice { array, from: other, .. }
            | Inst::ArrayPrefix { array, len: other, .. } => {
                out.push(*array);
                out.push(*other);
            }
            Inst::ArrayAlloc { len, .. } => out.push(*len),
            Inst::ArraySet { array, index, value } => {
                out.push(*array);
                out.push(*index);
                out.push(*value);
            }
            Inst::Call { args, .. } => out.extend_from_slice(args),
            Inst::CallIntrinsic { args, .. } => out.extend_from_slice(args),
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
    /// is what decides whether a backend emits an unreachable default anyway.
    Switch {
        on: ValueId,
        cases: Vec<(u64, Target)>,
        default: Option<Target>,
    },
    Return(Vec<ValueId>),
    Unreachable,
}

impl Term {
    /// Every edge out of the block, in order: a branch's `then` before its
    /// `else`, a switch's cases before its default.
    ///
    /// An iterator rather than a `Vec`, because every pass over a CFG asks
    /// this of every block, and a `Vec` per question was an allocation per
    /// block per pass.
    pub fn targets(&self) -> impl Iterator<Item = &Target> + Clone {
        // Up to two plain edges, then a switch's cases and its default.
        type Edges<'t> = ([Option<&'t Target>; 2], &'t [(u64, Target)], Option<&'t Target>);
        let (pair, cases, default): Edges<'_> =
            match self {
                Term::Jump(t) => ([Some(t), None], &[], None),
                Term::Branch { then, else_, .. } => ([Some(then), Some(else_)], &[], None),
                Term::Switch { cases, default, .. } => ([None, None], cases, default.as_ref()),
                Term::Return(_) | Term::Unreachable => ([None, None], &[], None),
            };
        pair.into_iter().flatten().chain(cases.iter().map(|(_, t)| t)).chain(default)
    }

    /// The `k`th of [`Term::targets`], without walking the ones before it.
    pub fn target(&self, k: usize) -> Option<&Target> {
        match self {
            Term::Jump(t) => (k == 0).then_some(t),
            Term::Branch { then, else_, .. } => match k {
                0 => Some(then),
                1 => Some(else_),
                _ => None,
            },
            Term::Switch { cases, default, .. } => match cases.get(k) {
                Some((_, t)) => Some(t),
                None if k == cases.len() => default.as_ref(),
                None => None,
            },
            Term::Return(_) | Term::Unreachable => None,
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
pub struct Signature {
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
    /// Bounded only by `Allocator`, which is inaccessible memory.
    Allocating,
    /// Bounded by an observable effect: no memory attribute at all.
    Effectful,
}

/// What a backend may assume about a function.
///
/// Every field is *conservative* out of `lower` alone: owning every parameter,
/// `Effectful` and abort-capable are all the answer that costs
/// performance and cannot be wrong. `middle::rc` computes the ownership column
/// and the two effect fixpoints, and `lower` copies them on from its plan;
/// where a field keeps the conservative answer, LLVM emits fewer attributes and
/// the debug backend emits more reference counting, which is the correct
/// direction to be wrong in.
///
/// `nounwind` is not a field. It is true of every function in the language —
/// there is no unwinding at all (SPEC 6.9) — and a constant stored per
/// function is a constant somebody eventually sets to `false`.
pub struct Facts {
    /// One per [`Signature::params`] entry.
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

    /// Rewrites every `TypeId` this code holds through `remap`, which maps a
    /// local interner's ids to a whole-program one's. `lower` builds each
    /// function against a private interner, across the cores, and folds the
    /// interners together afterwards; this is how a function's ids follow. The
    /// two places a `TypeId` reaches are a value's [`Type`] and a
    /// [`Inst::Structural`]'s type.
    pub fn remap_types(&mut self, remap: &[TypeId]) {
        for ty in &mut self.values {
            if let Type::Agg(id) = ty {
                if let Some(&g) = remap.get(id.index()) {
                    *id = g;
                }
            }
        }
        for block in &mut self.blocks {
            for inst in &mut block.insts {
                if let Inst::Structural { ty, .. } = inst {
                    if let Some(&g) = remap.get(ty.index()) {
                        *ty = g;
                    }
                }
            }
        }
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
    pub sig: Signature,
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
    /// Whether any value of this program can come to be reachable from a
    /// second thread — `middle::rc::crosses_tasks`, carried here because both
    /// native backends need it at their **entry points**, which is the one
    /// place in an artifact that is not a function of any one `Func`.
    ///
    /// True makes `main` call `buri_rt_values_may_cross_tasks`, which marks
    /// every block the program allocates and lets `Tasks.parallel` fan its
    /// steps out. False emits nothing, and a program that emits nothing is the
    /// program it was before track G — the *safe* answer either way, because
    /// the runtime's fan-out is gated on the same latch.
    pub crosses_tasks: bool,
    /// The generated `Equal` for each type a reactive **cell** holds, where
    /// `middle::derives` generated one.
    ///
    /// `ui/signal`'s rule is that writing a value equal to the one a cell holds
    /// re-runs nothing, and `==` is structural (SPEC 7.2) — so the runtime,
    /// which holds a cell as bytes, cannot decide it: two equal strings are two
    /// pointers. What the graph is handed instead is a comparison **at the
    /// type**, and this is where a backend finds the function to wrap in one
    /// (`cli/runtime/ui.rs`'s `Equal`, `runtime_table.rs`'s `Extra::Owned`).
    ///
    /// Keyed by the source type rather than by the call site, because the two
    /// keys that carry it — `signal` and `write` — name that type in a bare
    /// argument and there is one comparison per type however many cells hold
    /// one. Empty for every program with no signals, and for every program the
    /// JavaScript backend compiles: `middle::run` does not run `derives`.
    pub cell_equal: HashMap<Ty, FuncIdx>,
}

impl Program {
    pub fn type_info(&self, id: TypeId) -> &TypeInfo {
        self.types.get(id.index()).or_ice("every TypeId was minted by the lowering's interner")
    }

    pub fn unit_name(&self, unit: u32) -> &str {
        self.units.get(unit as usize).map(String::as_str).unwrap_or("?")
    }

    /// The index of every function, bucketed by the codegen unit that owns it.
    ///
    /// One row per entry in [`Program::units`], each row in ascending function
    /// index — which is exactly the order and exactly the membership that
    /// `funcs.iter().filter(|f| f.unit == u)` yields, so a caller that walked
    /// the whole program once per unit can walk `by_unit[u]` instead and see
    /// the same functions in the same order. That equivalence is load-bearing:
    /// both native backends derive a `codegen` cache key by concatenating the
    /// rendered text of a unit's functions, and a different order is a
    /// different key.
    ///
    /// It exists because the per-unit scan it replaces is Θ(units × functions),
    /// which is quadratic in a program that grows by adding modules — the
    /// first finding of `design/PERFORMANCE.md` §6.4. This is one pass, and each
    /// row
    /// costs what the unit itself contains.
    ///
    /// A function whose `unit` is out of range is dropped, which is the same
    /// answer the filter gave: no unit index in `0..units.len()` equals it.
    pub fn funcs_by_unit(&self) -> Vec<Vec<usize>> {
        let mut by_unit: Vec<Vec<usize>> = vec![Vec::new(); self.units.len()];
        for (i, f) in self.funcs.iter().enumerate() {
            if let Some(row) = by_unit.get_mut(f.unit as usize) {
                row.push(i);
            }
        }
        by_unit
    }
}

// ---------------------------------------------------------------------------
// Verification
// ---------------------------------------------------------------------------

/// Every way one function can be malformed, as sentences.
///
/// This is not a debug assertion that fires in a developer's build and is
/// absent in a user's: it is a function the tests call on real lowered
/// programs, and a backend calls it behind `cfg!(debug_assertions)`
/// (`stencil/mod.rs::emit_units`).
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
///    An SSA form where that does not hold is one where a backend's own
///    verifier reports "instruction result used before definition" a wave
///    later, with no lowering site to point at.
///  * **Every value is defined exactly once.**
///  * **An abort ends its block.** `buri_abort` does not return, so anything
///    after it in the same block is unreachable code the backends would have
///    to invent a rule for.
///
/// Critical edges are *not* checked for, because the design does not forbid
/// them: a per-edge argument list is what makes them harmless
/// (CODEGEN-LLVM.md §2.1).
fn verify_func(program: &Program, func: &Func) -> Vec<String> {
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
            if let Inst::Call { func: callee, args, .. } = inst {
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
                        if c.sig.rets.len() != 1 {
                            errs.push(format!(
                                "{name}: b{bi} takes one result from {}, which returns {}",
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
                dom.dominates(db, at.0)
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

/// Which blocks dominate which: the dominator tree, numbered in preorder and
/// postorder so that one question is two comparisons.
///
/// Cooper, Harvey and Kennedy's iteration over reverse postorder. A bitset per
/// block was `n²` in the blocks, and a long `match` is thousands of them.
struct Dominance {
    reachable: Vec<bool>,
    pre: Vec<usize>,
    post: Vec<usize>,
}

impl Dominance {
    /// Whether `a` dominates `b`. An unreachable block dominates nothing and
    /// is dominated by nothing.
    fn dominates(&self, a: usize, b: usize) -> bool {
        let reached = |i: usize| self.reachable.get(i).copied().unwrap_or(false);
        let at = |v: &Vec<usize>, i: usize| v.get(i).copied().unwrap_or(0);
        reached(a)
            && reached(b)
            && at(&self.pre, a) <= at(&self.pre, b)
            && at(&self.post, b) <= at(&self.post, a)
    }
}

fn dominators(code: &Code) -> Dominance {
    let n = code.blocks.len();
    let reachable = code.reachable();
    let preds = code.preds();
    // Postorder of the reachable blocks, by an explicit stack.
    let mut order: Vec<usize> = Vec::with_capacity(n);
    let mut number = vec![usize::MAX; n];
    if n > 0 {
        let mut seen = vec![false; n];
        let mut stack: Vec<(usize, usize)> = vec![(0, 0)];
        if let Some(s) = seen.get_mut(0) {
            *s = true;
        }
        while let Some((b, next)) = stack.last_mut().map(|(b, k)| (*b, k)) {
            if let Some(t) = code.blocks.get(b).and_then(|blk| blk.term.targets().nth(*next)) {
                *next = next.saturating_add(1);
                let t = t.block.index();
                if let Some(s) = seen.get_mut(t) {
                    if !*s {
                        *s = true;
                        stack.push((t, 0));
                    }
                }
            } else {
                stack.pop();
                if let Some(k) = number.get_mut(b) {
                    *k = order.len();
                }
                order.push(b);
            }
        }
    }
    let po = |b: usize| number.get(b).copied().unwrap_or(usize::MAX);
    let mut idom = vec![usize::MAX; n];
    if let Some(root) = idom.get_mut(0) {
        *root = 0;
    }
    let mut changed = true;
    while changed {
        changed = false;
        for &b in order.iter().rev().skip(1) {
            let mut new = usize::MAX;
            for p in preds.get(b).map(Vec::as_slice).unwrap_or_default() {
                let p = p.index();
                if idom.get(p).copied().unwrap_or(usize::MAX) == usize::MAX {
                    continue;
                }
                new = if new == usize::MAX {
                    p
                } else {
                    let (mut x, mut y) = (p, new);
                    while x != y {
                        while po(x) < po(y) {
                            x = idom.get(x).copied().unwrap_or(0);
                        }
                        while po(y) < po(x) {
                            y = idom.get(y).copied().unwrap_or(0);
                        }
                    }
                    x
                };
            }
            if let Some(slot) = idom.get_mut(b) {
                if *slot != new {
                    *slot = new;
                    changed = true;
                }
            }
        }
    }
    // Number the tree in preorder and postorder.
    let mut children: Vec<Vec<usize>> = vec![Vec::new(); n];
    for &b in order.iter().rev().skip(1) {
        if let Some(c) = idom.get(b).and_then(|&d| children.get_mut(d)) {
            c.push(b);
        }
    }
    let (mut pre, mut post) = (vec![0; n], vec![0; n]);
    let (mut next_pre, mut next_post) = (0usize, 0usize);
    if n > 0 {
        let mut stack: Vec<(usize, usize)> = vec![(0, 0)];
        if let Some(p) = pre.get_mut(0) {
            *p = next_pre;
        }
        next_pre = next_pre.saturating_add(1);
        while let Some((b, k)) = stack.last_mut().map(|(b, k)| (*b, k)) {
            if let Some(&c) = children.get(b).and_then(|cs| cs.get(*k)) {
                *k = k.saturating_add(1);
                if let Some(p) = pre.get_mut(c) {
                    *p = next_pre;
                }
                next_pre = next_pre.saturating_add(1);
                stack.push((c, 0));
            } else {
                stack.pop();
                if let Some(p) = post.get_mut(b) {
                    *p = next_post;
                }
                next_post = next_post.saturating_add(1);
            }
        }
    }
    Dominance { reachable, pre, post }
}

// ---------------------------------------------------------------------------
// Printing
// ---------------------------------------------------------------------------

/// The IR as text, which is the form a human reads and a cache key hashes.
///
/// The build's `codegen` key is `H(the unit's lowered IR)` (ARCHITECTURE.md
/// §6.2), and this rendering is a faithful, total and deterministic function
/// of the IR — no hash order anywhere, every name derived from the program —
/// so hashing these bytes per unit is a correct way to compute it and is the
/// one that can be inspected when a key changes and nobody knows why.
///
/// A callee is named by its **symbol**, never by its `FuncIdx`. `FuncIdx` is a
/// program-global index assigned in source order, so a declaration added
/// anywhere shifts every later index and rewrites the text — and therefore the
/// `codegen` key — of every unit that calls anything. The symbol is the name
/// the linker resolves the call by, which is exactly what the object file
/// depends on, and it is a property of the callee rather than of the program
/// it happens to be in. Symbols are unique per function by construction
/// (`monomorphize::Monomorphizer::name_of`), so the naming stays injective and
/// no two distinct callees can render alike.
impl fmt::Display for Program {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut text = String::new();
        for func in &self.funcs {
            text.clear();
            self.render_func_into(func, &mut text);
            f.write_str(&text)?;
        }
        Ok(())
    }
}

impl Program {
    /// One function, as text.
    pub fn render_func(&self, func: &Func) -> String {
        let mut out = String::new();
        self.render_func_into(func, &mut out);
        out
    }

    /// The same, appended to a buffer the caller owns.
    ///
    /// A `codegen` key is the hash of a whole unit's text, and a unit is a few
    /// hundred functions; building each of them as its own `String` only to
    /// copy it into the next one allocates the program twice over. The text is
    /// identical either way — [`render_func`](Program::render_func) is this
    /// function into an empty buffer — so the two cannot drift.
    ///
    /// Every piece is pushed straight onto `out`. The renderer used to build a
    /// `String` per operand with `format!` and join them, and that was most of
    /// what hashing a unit cost.
    pub fn render_func_into(&self, func: &Func, out: &mut String) {
        out.push_str("; ");
        out.push_str(&func.debug_name);
        out.push_str(" [unit ");
        out.push_str(self.unit_name(func.unit));
        out.push_str("]\nfn ");
        out.push_str(&func.symbol);
        out.push('(');
        self.types_into(&func.sig.params, out);
        out.push(')');
        match func.sig.rets.as_slice() {
            [] => {}
            [one] => {
                out.push_str(" -> ");
                self.ty_into(*one, out);
            }
            many => {
                out.push_str(" -> (");
                self.types_into(many, out);
                out.push(')');
            }
        }
        let code = match &func.body {
            Body::Runtime(key) => {
                let _ = writeln!(out, " = runtime {key:?}");
                return;
            }
            Body::Code(c) => c,
        };
        out.push_str(" {\n");
        for (i, b) in code.blocks.iter().enumerate() {
            // The parameter list is printed even when it is empty, so that a
            // block header and the edges naming it have the same shape.
            out.push_str("  b");
            num(out, i as u64);
            out.push('(');
            for (k, p) in b.params.iter().enumerate() {
                if k > 0 {
                    out.push_str(", ");
                }
                value(out, *p);
                out.push_str(": ");
                self.ty_into(code.ty_of(*p), out);
            }
            out.push_str("):\n");
            for inst in &b.insts {
                out.push_str("    ");
                self.inst_into(inst, out);
                out.push('\n');
            }
            out.push_str("    ");
            term_into(&b.term, out);
            out.push('\n');
        }
        out.push_str("}\n");
    }

    fn types_into(&self, ts: &[Type], out: &mut String) {
        for (k, t) in ts.iter().enumerate() {
            if k > 0 {
                out.push_str(", ");
            }
            self.ty_into(*t, out);
        }
    }

    fn ty_into(&self, t: Type, out: &mut String) {
        out.push_str(match t {
            Type::I1 => "i1",
            Type::I8 => "i8",
            Type::I16 => "i16",
            Type::I32 => "i32",
            Type::I64 => "i64",
            Type::I128 => "i128",
            Type::F32 => "f32",
            Type::F64 => "f64",
            Type::Ptr => "ptr",
            Type::Unit => "unit",
            Type::Agg(id) => &self.type_info(id).name,
        });
    }

    /// The symbol a callee is rendered by. A `FuncIdx` out of range renders as
    /// itself so that a malformed program still prints rather than panicking;
    /// `verify` is what reports it.
    fn sym_into(&self, func: FuncIdx, out: &mut String) {
        match self.funcs.get(func.index()) {
            Some(f) => out.push_str(&f.symbol),
            None => {
                out.push('f');
                num(out, u64::from(func.0));
            }
        }
    }

    fn inst_into(&self, inst: &Inst, out: &mut String) {
        // `vN = `, the head every instruction with one result starts with.
        let dest = |out: &mut String, d: &ValueId| {
            value(out, *d);
            out.push_str(" = ");
        };
        match inst {
            Inst::Const { dest: d, value: c } => {
                dest(out, d);
                out.push_str("const ");
                constant_into(c, out);
            }
            Inst::Unary { dest: d, op, prim, arg } => {
                dest(out, d);
                out.push_str(un_op(*op));
                out.push('.');
                out.push_str(prim.name());
                out.push(' ');
                value(out, *arg);
            }
            Inst::Binary { dest: d, op, prim, lhs, rhs } => {
                dest(out, d);
                out.push_str(bin_op(*op));
                out.push('.');
                out.push_str(prim.name());
                out.push(' ');
                pair(out, *lhs, *rhs);
            }
            Inst::MakeStruct { dest: d, fields } => {
                dest(out, d);
                out.push_str("make ");
                wrapped(out, fields);
            }
            Inst::MakeEnum { dest: d, variant, fields } => {
                dest(out, d);
                out.push_str("make #");
                num(out, u64::from(*variant));
                out.push(' ');
                wrapped(out, fields);
            }
            Inst::MakeArray { dest: d, elems } => {
                dest(out, d);
                out.push_str("array ");
                wrapped(out, elems);
            }
            Inst::MakeClosure { dest: d, func, env } => {
                dest(out, d);
                out.push_str("closure fn ");
                self.sym_into(*func, out);
                out.push_str(", ");
                match env {
                    Some(e) => value(out, *e),
                    None => out.push_str("null"),
                }
            }
            Inst::GetField { dest: d, agg, index } => {
                dest(out, d);
                out.push_str("field.");
                num(out, u64::from(*index));
                out.push(' ');
                value(out, *agg);
            }
            Inst::GetPayload { dest: d, agg, variant, index } => {
                dest(out, d);
                out.push_str("payload.#");
                num(out, u64::from(*variant));
                out.push('.');
                num(out, u64::from(*index));
                out.push(' ');
                value(out, *agg);
            }
            Inst::GetTag { dest: d, agg } => {
                dest(out, d);
                out.push_str("tag ");
                value(out, *agg);
            }
            Inst::ArrayLen { dest: d, array } => {
                dest(out, d);
                out.push_str("len ");
                value(out, *array);
            }
            Inst::ArrayGet { dest: d, array, index } => {
                dest(out, d);
                out.push_str("elem ");
                pair(out, *array, *index);
            }
            Inst::ArraySlice { dest: d, array, from } => {
                dest(out, d);
                out.push_str("slice ");
                pair(out, *array, *from);
            }
            Inst::ArrayAlloc { dest: d, len } => {
                dest(out, d);
                out.push_str("alloc ");
                value(out, *len);
            }
            Inst::ArraySet { array, index, value: v } => {
                out.push_str("set ");
                pair(out, *array, *index);
                out.push_str(", ");
                value(out, *v);
            }
            Inst::ArrayPrefix { dest: d, array, len } => {
                dest(out, d);
                out.push_str("prefix ");
                pair(out, *array, *len);
            }
            Inst::Call { dest: d, func, args } => {
                dest(out, d);
                out.push_str("call fn ");
                self.sym_into(*func, out);
                wrapped(out, args);
            }
            Inst::CallIndirect { dest: d, callee, args } => {
                dest(out, d);
                out.push_str("call_indirect ");
                value(out, *callee);
                wrapped(out, args);
            }
            Inst::CallIntrinsic { dest: d, key, args } => {
                dest(out, d);
                let _ = write!(out, "intrinsic {key:?}");
                wrapped(out, args);
            }
            Inst::Structural { dest: d, op, ty, args } => {
                dest(out, d);
                out.push_str("structural.");
                out.push_str(structural_op(*op));
                out.push(' ');
                out.push_str(&self.type_info(*ty).name);
                wrapped(out, args);
            }
            Inst::IncRef { value: v } => {
                out.push_str("incref ");
                value(out, *v);
            }
            Inst::DecRef { value: v, drop } => {
                out.push_str("decref ");
                value(out, *v);
                if let Some(d) = drop {
                    out.push_str(", drop fn ");
                    self.sym_into(*d, out);
                }
            }
            Inst::Abort { message } => {
                let _ = write!(out, "abort {message:?}");
            }
        }
    }
}

/// `n` in decimal, without going through `fmt`.
fn num(out: &mut String, mut n: u64) {
    let mut buf = [0u8; 20];
    let mut at = buf.len();
    loop {
        at = at.saturating_sub(1);
        if let Some(slot) = buf.get_mut(at) {
            *slot = b'0'.saturating_add((n % 10) as u8);
        }
        n /= 10;
        if n == 0 {
            break;
        }
    }
    for b in buf.get(at..).unwrap_or_default() {
        out.push(char::from(*b));
    }
}

fn value(out: &mut String, v: ValueId) {
    out.push('v');
    num(out, u64::from(v.0));
}

/// `va, vb`.
fn pair(out: &mut String, a: ValueId, b: ValueId) {
    value(out, a);
    out.push_str(", ");
    value(out, b);
}

/// `va, vb, vc`, or nothing for none.
fn values(out: &mut String, vs: &[ValueId]) {
    for (k, v) in vs.iter().enumerate() {
        if k > 0 {
            out.push_str(", ");
        }
        value(out, *v);
    }
}

/// `(va, vb)`.
fn wrapped(out: &mut String, vs: &[ValueId]) {
    out.push('(');
    values(out, vs);
    out.push(')');
}

fn target_into(t: &Target, out: &mut String) {
    out.push('b');
    num(out, u64::from(t.block.0));
    wrapped(out, &t.args);
}

fn term_into(t: &Term, out: &mut String) {
    match t {
        Term::Jump(to) => {
            out.push_str("jump ");
            target_into(to, out);
        }
        Term::Branch { cond, then, else_ } => {
            out.push_str("branch ");
            value(out, *cond);
            out.push_str(", ");
            target_into(then, out);
            out.push_str(", ");
            target_into(else_, out);
        }
        Term::Switch { on, cases, default } => {
            out.push_str("switch ");
            value(out, *on);
            out.push_str(", [");
            for (k, (key, t)) in cases.iter().enumerate() {
                if k > 0 {
                    out.push_str(", ");
                }
                num(out, *key);
                out.push_str(" -> ");
                target_into(t, out);
            }
            out.push(']');
            if let Some(t) = default {
                out.push_str(", default ");
                target_into(t, out);
            }
        }
        Term::Return(vs) => {
            out.push_str("return");
            if !vs.is_empty() {
                out.push(' ');
                values(out, vs);
            }
        }
        Term::Unreachable => out.push_str("unreachable"),
    }
}

fn constant_into(c: &Const, out: &mut String) {
    match c {
        Const::Unit => out.push_str("()"),
        Const::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Const::Int { bits, negative } => {
            if *negative {
                out.push('-');
            }
            let bits = bits.get();
            match u64::try_from(bits) {
                Ok(small) => num(out, small),
                Err(_) => {
                    let _ = write!(out, "{bits}");
                }
            }
        }
        Const::Float(v) => {
            let _ = write!(out, "{v:?}");
        }
        Const::Str(s) => {
            let _ = write!(out, "{s:?}");
        }
        Const::Char(c) => {
            let _ = write!(out, "{c:?}");
        }
        Const::Null => out.push_str("null"),
        Const::Undef => out.push_str("undef"),
    }
}

fn un_op(op: UnOp) -> &'static str {
    match op {
        UnOp::Neg => "neg",
        UnOp::Not => "not",
        UnOp::BitNot => "bitnot",
        UnOp::FromBool => "frombool",
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
                sig: Signature { params: vec![Type::I64, Type::I64], rets: vec![Type::I64] },
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
            crosses_tasks: false,
            cell_equal: HashMap::default(),
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
            value: Const::Int { bits: Magnitude::new(1), negative: false },
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
        entry.insts.push(Inst::Const { dest: v, value: Const::Int { bits: Magnitude::new(0), negative: false } });
        let errs = verify(&p);
        assert!(errs.iter().any(|e| e.contains("continues after an abort")), "{errs:?}");
        assert!(errs.iter().any(|e| e.contains("does not end unreachable")), "{errs:?}");
    }
}
