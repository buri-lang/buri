//! The value model at the machine boundary: leaves, signatures, and where a
//! value's bytes live.
//!
//! `middle::layout` decides the bytes (VALUE-MODEL.md §1–§8) and this file
//! decides how those bytes are *carried* — in a register, in a stack slot,
//! across a call, across a control-flow edge. Nothing here recomputes a size or
//! an offset; every number comes out of [`Layouts`].
//!
//! # One IR value is one CLIF value
//!
//! A scalar is its register. An aggregate is **a pointer to its bytes**, and
//! every aggregate SSA value owns a stack slot written by the instruction that
//! defines it. A zero-sized value is *no* CLIF value at all, which is the same
//! rule VALUE-MODEL.md §8 states for signatures, applied one level down so that
//! `()` and a `core/host` context need no special case anywhere else.
//!
//! Owning a slot per value rather than handing out interior pointers is what
//! makes the representation sound. A value has exactly one definition, so the
//! most recent execution of that definition is always the binding a dominated
//! use wants; a pointer *into* another value's slot has no such guarantee,
//! because the slot it points into can be rewritten by a re-entered block
//! without the pointer's own definition re-executing. So a field projection
//! copies rather than aliases, and the copy is the price of not having to
//! reason about that.
//!
//! # Aggregates cross every boundary as scalar leaves
//!
//! VALUE-MODEL.md §5.1: a parameter is a scalar leaf, so a `Str` argument is
//! three `i64`s and nothing in a signature is an aggregate. This file computes
//! that decomposition once, in [`Abi::leaves`], and it is used for three things
//! that have to agree: a call's arguments, a function's returns, and a **block
//! parameter**. The third is the one the design does not name, and it is not
//! optional: passing a pointer along an edge would reintroduce exactly the
//! aliasing the paragraph above rules out.
//!
//! ## Why the decomposition is machine words and not fields
//!
//! An aggregate is covered greedily with `i64`/`i32`/`i16`/`i8` chunks rather
//! than walked field by field. Two reasons, and the first is decisive:
//!
//!  * **An enum's payload is a union.** There is no field list to walk — which
//!    of the variants' fields is live depends on the tag — so a field-wise
//!    decomposition is not defined for the one shape that most needs one.
//!  * It agrees with the runtime's C ABI everywhere the runtime is called.
//!    Every `buri_rt_*` aggregate parameter is a record of pointer-sized words
//!    (`Str` is `{base, ptr, len}`, `[T]` is `{ptr, len}`), and a greedy `i64`
//!    cover of a 24- or 16-byte value is exactly those words
//!    (`cli/runtime/lib.rs` §2, rule 1).
//!
//! What it costs is that padding bytes ride along uninitialised. Nothing reads
//! them: they are stored back into a slot at the far end and the only loads
//! from that slot are at offsets the layout named.
//!
//! # A result that does not fit in the return registers travels in memory
//!
//! The decomposition above is the whole story for *parameters*: a target that
//! runs out of argument registers puts the rest on the stack, and Cranelift
//! does that itself. Results have no such fallback. SysV x86_64 returns in
//! `rax`/`rdx` — two — so a `Str`, which is three words, is not a signature
//! that architecture has, and Cranelift says so rather than inventing one:
//! *"Too many return values to fit in registers. Use a StructReturn argument
//! instead."* AArch64 has eight and hid it.
//!
//! So [`MAX_RET_LEAVES`] results come back in registers and anything wider
//! comes back through a pointer the caller passes and the callee writes — the
//! same **out-pointer** `cli/runtime/lib.rs` §2 rule 2 already states for every
//! runtime entry that answers an aggregate, appended last for the same reason.
//! One convention now covers the runtime's C functions, the functions this
//! backend compiles and the ones it generates.
//!
//! The threshold is a constant and not a question asked of the ISA. Two is the
//! smallest any supported target has, so the rule is the same everywhere —
//! which means the memory path is the path *every* test on *every* host walks,
//! rather than one that is only ever exercised by a target the test suite
//! cannot run.

use std::rc::Rc;

use cranelift_codegen::ir::{types, AbiParam, Signature, Type as ClifType};
use cranelift_codegen::isa::CallConv;

use crate::compiler::middle::ir;
use crate::compiler::middle::layout::{Cycles, Layout, Layouts, Repr, Scalar};
use crate::compiler::semantics::types::{Tables, Ty};

/// A pointer, on both targets this compiles for (ARCHITECTURE.md §9).
pub const PTR: ClifType = types::I64;

/// How many flattened results come back in registers; see this file's header.
const MAX_RET_LEAVES: usize = 2;

/// Whether these flattened results travel through an out-pointer.
///
/// An `i128` counts as two, because that is how many registers it takes on
/// every target that has it.
pub fn returns_indirectly(rets: &[ClifType]) -> bool {
    let slots: usize =
        rets.iter().map(|t| if *t == types::I128 { 2 } else { 1 }).sum();
    slots > MAX_RET_LEAVES
}

/// A signature from flattened parameters and flattened results.
///
/// The one place the out-pointer rule is applied, so that a definition, a
/// call, a generated helper and a call through a closure cannot disagree about
/// where a wide result lives.
pub fn signature_of(
    call_conv: CallConv,
    params: &[ClifType],
    rets: &[ClifType],
) -> Signature {
    let mut s = Signature::new(call_conv);
    for p in params {
        s.params.push(AbiParam::new(*p));
    }
    if returns_indirectly(rets) {
        s.params.push(AbiParam::new(PTR));
    } else {
        for r in rets {
            s.returns.push(AbiParam::new(*r));
        }
    }
    s
}

/// One scalar in an aggregate's flattened form: what to load, and from where.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Leaf {
    pub offset: u32,
    pub ty: ClifType,
}

/// The layout table, plus the questions a backend asks it.
///
/// The two caches are not an optimisation of a correct answer, they are what
/// makes the answer affordable: [`Abi::layout`] is asked once per instruction
/// operand, and both the `Ty` it would clone to ask and the `Layout` it would
/// copy back are O(the type's width). One `Abi` serves one [`ir::Program`] —
/// `compile_unit` builds them together — which is what lets a [`ir::TypeId`]
/// be an index here.
pub struct Abi<'a> {
    pub layouts: Layouts<'a>,
    pub tables: &'a Tables,
    pub call_conv: CallConv,
    /// By [`ir::TypeId`] index.
    aggs: Vec<Option<Rc<Layout>>>,
    /// By [`ir::TypeId`] index; the flattened form of the same type.
    agg_leaves: Vec<Option<Rc<[Leaf]>>>,
    /// The flattened form of every scalar register shape, by [`scalar_slot`],
    /// and of anything that has none.
    scalar_leaves: Vec<Rc<[Leaf]>>,
    empty_leaves: Rc<[Leaf]>,
}

impl<'a> Abi<'a> {
    /// `cycles` is the recursion analysis of these same `tables`, taken once
    /// for the emission: see [`Cycles`].
    pub fn new(tables: &'a Tables, call_conv: CallConv, cycles: Rc<Cycles>) -> Abi<'a> {
        Abi {
            layouts: Layouts::with_cycles(tables, cycles),
            tables,
            call_conv,
            aggs: Vec::new(),
            agg_leaves: Vec::new(),
            scalar_leaves: SCALARS
                .iter()
                .map(|t| Rc::from(vec![Leaf { offset: 0, ty: *t }].as_slice()))
                .collect(),
            empty_leaves: Rc::from(Vec::new().as_slice()),
        }
    }

    /// The source type an [`ir::Type::Agg`] names, or `None` for a scalar.
    pub fn source_ty(&self, program: &ir::Program, t: ir::Type) -> Option<Ty> {
        match t {
            ir::Type::Agg(id) => Some(program.type_info(id).ty.clone()),
            _ => None,
        }
    }

    /// The layout of anything an IR value can have.
    pub fn layout(&mut self, program: &ir::Program, t: ir::Type) -> Rc<Layout> {
        let ir::Type::Agg(id) = t else { return scalar_layout(t) };
        let i = id.index();
        if let Some(Some(l)) = self.aggs.get(i) {
            return Rc::clone(l);
        }
        let l = self.layouts.shared(&program.type_info(id).ty);
        if self.aggs.len() <= i {
            self.aggs.resize(i.saturating_add(1), None);
        }
        if let Some(slot) = self.aggs.get_mut(i) {
            *slot = Some(Rc::clone(&l));
        }
        l
    }

    /// The register shape of a scalar type. `None` for `Unit` and for an
    /// aggregate, which are the two things that are not one.
    pub fn register(t: ir::Type) -> Option<ClifType> {
        Some(match t {
            // `Bool` is `i1` in the value model and `i8` in a register:
            // Cranelift has no `b1`, and an `icmp` already answers `i8`.
            ir::Type::I1 | ir::Type::I8 => types::I8,
            ir::Type::I16 => types::I16,
            ir::Type::I32 => types::I32,
            ir::Type::I64 => types::I64,
            ir::Type::I128 => types::I128,
            ir::Type::F32 => types::F32,
            ir::Type::F64 => types::F64,
            ir::Type::Ptr => PTR,
            ir::Type::Unit | ir::Type::Agg(_) => return None,
        })
    }

    /// How many bytes a value of this type occupies.
    pub fn size(&mut self, program: &ir::Program, t: ir::Type) -> u32 {
        self.layout(program, t).size
    }

    /// The flattened form: what a call, a return and an edge carry.
    ///
    /// Empty for `Unit` and for anything zero-sized, which is what drops a
    /// `core/host` context from every signature it appears in without a rule
    /// about contexts anywhere (VALUE-MODEL.md §8).
    pub fn leaves(&mut self, program: &ir::Program, t: ir::Type) -> Rc<[Leaf]> {
        let ir::Type::Agg(id) = t else {
            return match scalar_slot(t) {
                Some(s) => self
                    .scalar_leaves
                    .get(s)
                    .map_or_else(|| Rc::clone(&self.empty_leaves), Rc::clone),
                None => Rc::clone(&self.empty_leaves),
            };
        };
        let i = id.index();
        if let Some(Some(l)) = self.agg_leaves.get(i) {
            return Rc::clone(l);
        }
        let cover: Rc<[Leaf]> = Rc::from(chunks(self.size(program, t)).as_slice());
        if self.agg_leaves.len() <= i {
            self.agg_leaves.resize(i.saturating_add(1), None);
        }
        if let Some(slot) = self.agg_leaves.get_mut(i) {
            *slot = Some(Rc::clone(&cover));
        }
        cover
    }

    /// The flattened form of a **source** type, for a value the IR never named.
    ///
    /// `leaves` answers this question about an [`ir::Type`], which is what a
    /// signature is built from. The closure surface of `core/list` needs the
    /// same answer about a `[T]`'s element — a type the loop it emits knows
    /// only as the layout table's `T` — and the two must agree leaf for leaf,
    /// because one of them is the callee's signature and the other is the call.
    /// They do: an [`ir::Type`] is a scalar exactly where the layout is
    /// [`Repr::Scalar`], and everything else is covered by [`chunks`] on both
    /// sides.
    pub fn ty_leaves(&mut self, ty: &Ty) -> Vec<Leaf> {
        let l = self.layouts.shared(ty);
        match l.repr {
            Repr::Zero => Vec::new(),
            Repr::Scalar(s) => {
                vec![Leaf { offset: 0, ty: super::emit::scalar_clif(s) }]
            }
            _ => chunks(l.size),
        }
    }

    /// The flattened form of a list of types, as register shapes.
    pub fn leaf_types(&mut self, program: &ir::Program, ts: &[ir::Type]) -> Vec<ClifType> {
        let mut out = Vec::new();
        for t in ts {
            for leaf in self.leaves(program, *t).iter() {
                out.push(leaf.ty);
            }
        }
        out
    }

    /// The machine signature of an IR signature: every parameter's leaves, in
    /// order, then every result's — or, where the results are too wide for the
    /// return registers, an out-pointer after the parameters.
    pub fn signature(&mut self, program: &ir::Program, sig: &ir::Sig) -> Signature {
        let params = self.leaf_types(program, &sig.params);
        let rets = self.leaf_types(program, &sig.rets);
        signature_of(self.call_conv, &params, &rets)
    }

    /// Whether a function with these results answers through an out-pointer.
    pub fn rets_indirect(&mut self, program: &ir::Program, rets: &[ir::Type]) -> bool {
        let leaves = self.leaf_types(program, rets);
        returns_indirectly(&leaves)
    }
}

/// A greedy cover of `size` bytes by machine words, largest first.
///
/// `24 -> [i64@0, i64@8, i64@16]`, which is a `Str`; `16 -> [i64@0, i64@8]`,
/// which is a `[T]` and a closure; `3 -> [i16@0, i8@2]`. Every chunk is inside
/// `0..size`, so a load or a store of one is in bounds for a slot of exactly
/// that many bytes.
fn chunks(size: u32) -> Vec<Leaf> {
    let mut out = Vec::new();
    let mut at = 0u32;
    while at < size {
        let left = size.saturating_sub(at);
        let (width, ty) = if left >= 8 {
            (8, types::I64)
        } else if left >= 4 {
            (4, types::I32)
        } else if left >= 2 {
            (2, types::I16)
        } else {
            (1, types::I8)
        };
        out.push(Leaf { offset: at, ty });
        at = at.saturating_add(width);
    }
    out
}

/// The register shapes, in [`scalar_slot`] order.
const SCALARS: [ClifType; 9] = [
    types::I8,
    types::I8,
    types::I16,
    types::I32,
    types::I64,
    types::I128,
    types::F32,
    types::F64,
    PTR,
];

/// Which of [`SCALARS`] an IR type is, or `None` for `Unit` and an aggregate.
fn scalar_slot(t: ir::Type) -> Option<usize> {
    Some(match t {
        ir::Type::I1 => 0,
        ir::Type::I8 => 1,
        ir::Type::I16 => 2,
        ir::Type::I32 => 3,
        ir::Type::I64 => 4,
        ir::Type::I128 => 5,
        ir::Type::F32 => 6,
        ir::Type::F64 => 7,
        ir::Type::Ptr => 8,
        ir::Type::Unit | ir::Type::Agg(_) => return None,
    })
}

/// A scalar IR type's layout, so that [`Abi::layout`] is total.
///
/// Built once per process rather than per call: a `Layout` for a scalar owns
/// no heap, but the `Rc` every caller now takes would still be an allocation
/// each time.
fn scalar_layout(t: ir::Type) -> Rc<Layout> {
    thread_local! {
        static TABLE: Vec<Rc<Layout>> = build_scalar_layouts();
    }
    TABLE.with(|table| {
        let i = scalar_slot(t).map_or(SCALARS.len(), |s| s);
        table.get(i).map_or_else(
            || {
                Rc::new(Layout {
                    size: 0,
                    align: 1,
                    stride: 0,
                    fields: Vec::new(),
                    repr: Repr::Zero,
                })
            },
            Rc::clone,
        )
    })
}

/// One entry per [`scalar_slot`], then the zero-sized layout last.
fn build_scalar_layouts() -> Vec<Rc<Layout>> {
    let scalars = [
        Scalar::Bool,
        Scalar::I8,
        Scalar::I16,
        Scalar::I32,
        Scalar::I64,
        Scalar::I128,
        Scalar::F32,
        Scalar::F64,
        Scalar::Ptr,
    ];
    let mut out: Vec<Rc<Layout>> = scalars
        .into_iter()
        .map(|s| {
            Rc::new(Layout {
                size: s.size(),
                align: s.align(),
                stride: s.size(),
                fields: Vec::new(),
                repr: Repr::Scalar(s),
            })
        })
        .collect();
    out.push(Rc::new(Layout {
        size: 0,
        align: 1,
        stride: 0,
        fields: Vec::new(),
        repr: Repr::Zero,
    }));
    out
}

/// `log2` of an alignment, which is how a `StackSlotData` spells one.
pub fn align_shift(align: u32) -> u8 {
    let mut shift = 0u8;
    let mut a = align.max(1);
    while a > 1 {
        a >>= 1;
        shift = shift.saturating_add(1);
    }
    shift
}

/// What is inside a struct, a tuple, a context or one enum variant.
///
/// `semantics::types` owns the walk; it is re-exported here because `abi::`
/// is where this file's callers already look for it.
pub use crate::compiler::semantics::types::{field_types, variant_types};

/// The element type of a `[T]`.
pub fn element_type(ty: &Ty) -> Option<Ty> {
    match ty {
        Ty::Array(t) => Some((**t).clone()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_str_covers_as_three_words() {
        assert_eq!(
            chunks(24),
            vec![
                Leaf { offset: 0, ty: types::I64 },
                Leaf { offset: 8, ty: types::I64 },
                Leaf { offset: 16, ty: types::I64 },
            ]
        );
    }

    #[test]
    fn a_ragged_size_covers_without_leaving_the_value() {
        let cover = chunks(3);
        assert_eq!(cover.len(), 2);
        let end = cover.last().map(|l| l.offset + 1).unwrap_or(0);
        assert_eq!(end, 3);
    }

    #[test]
    fn nothing_covers_nothing() {
        assert!(chunks(0).is_empty());
    }

    /// Two words come back in registers and three do not, on every target,
    /// because the threshold is the smallest any of them has.
    #[test]
    fn a_str_is_wider_than_the_return_registers() {
        assert!(!returns_indirectly(&[]));
        assert!(!returns_indirectly(&[types::I64]));
        assert!(!returns_indirectly(&[types::I64, types::F64]));
        assert!(returns_indirectly(&[types::I64, types::I64, types::I64]));
        // One `i128` is two registers, so a pair of them is four.
        assert!(!returns_indirectly(&[types::I128]));
        assert!(returns_indirectly(&[types::I128, types::I64]));
    }

    /// The out-pointer is a *parameter*, appended last, and the returns are
    /// then empty — which is the shape `cli/runtime/lib.rs` §2 rule 2 states.
    #[test]
    fn a_wide_result_becomes_a_trailing_pointer() {
        let wide = signature_of(
            CallConv::SystemV,
            &[types::I64],
            &[types::I64, types::I64, types::I64],
        );
        assert_eq!(wide.params.len(), 2);
        assert_eq!(wide.params.last().map(|p| p.value_type), Some(PTR));
        assert!(wide.returns.is_empty());

        let narrow = signature_of(CallConv::SystemV, &[types::I64], &[types::I64]);
        assert_eq!(narrow.params.len(), 1);
        assert_eq!(narrow.returns.len(), 1);
    }

    #[test]
    fn an_alignment_is_its_own_logarithm() {
        assert_eq!(align_shift(1), 0);
        assert_eq!(align_shift(8), 3);
        assert_eq!(align_shift(16), 4);
    }
}
