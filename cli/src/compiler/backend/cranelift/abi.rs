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

use cranelift_codegen::ir::{types, AbiParam, Signature, Type as ClifType};
use cranelift_codegen::isa::CallConv;

use crate::compiler::middle::ir;
use crate::compiler::middle::layout::{Layout, Layouts, Repr, Scalar};
use crate::compiler::semantics::types::{Tables, Ty};

/// A pointer, on both targets this compiles for (ARCHITECTURE.md §9).
pub const PTR: ClifType = types::I64;

/// One scalar in an aggregate's flattened form: what to load, and from where.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Leaf {
    pub offset: u32,
    pub ty: ClifType,
}

/// The layout table, plus the questions a backend asks it.
pub struct Abi<'a> {
    pub layouts: Layouts<'a>,
    pub tables: &'a Tables,
    pub call_conv: CallConv,
}

impl<'a> Abi<'a> {
    pub fn new(tables: &'a Tables, call_conv: CallConv) -> Abi<'a> {
        Abi { layouts: Layouts::new(tables), tables, call_conv }
    }

    /// The source type an [`ir::Type::Agg`] names, or `None` for a scalar.
    pub fn source_ty(&self, program: &ir::Program, t: ir::Type) -> Option<Ty> {
        match t {
            ir::Type::Agg(id) => Some(program.type_info(id).ty.clone()),
            _ => None,
        }
    }

    /// The layout of anything an IR value can have.
    pub fn layout(&mut self, program: &ir::Program, t: ir::Type) -> Layout {
        match self.source_ty(program, t) {
            Some(ty) => self.layouts.of(ty),
            None => scalar_layout(t),
        }
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

    /// Whether a value of this type is carried as a pointer to its bytes.
    pub fn is_indirect(&mut self, program: &ir::Program, t: ir::Type) -> bool {
        matches!(t, ir::Type::Agg(_)) && self.layout(program, t).size > 0
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
    pub fn leaves(&mut self, program: &ir::Program, t: ir::Type) -> Vec<Leaf> {
        match Abi::register(t) {
            Some(r) => vec![Leaf { offset: 0, ty: r }],
            None => {
                let size = self.size(program, t);
                chunks(size)
            }
        }
    }

    /// The machine signature of an IR signature: every parameter's leaves, in
    /// order, then every result's.
    pub fn signature(&mut self, program: &ir::Program, sig: &ir::Sig) -> Signature {
        let mut s = Signature::new(self.call_conv);
        for p in &sig.params {
            for leaf in self.leaves(program, *p) {
                s.params.push(AbiParam::new(leaf.ty));
            }
        }
        for r in &sig.rets {
            for leaf in self.leaves(program, *r) {
                s.returns.push(AbiParam::new(leaf.ty));
            }
        }
        s
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

/// A scalar IR type's layout, so that [`Abi::layout`] is total.
fn scalar_layout(t: ir::Type) -> Layout {
    let s = match t {
        ir::Type::I1 => Scalar::Bool,
        ir::Type::I8 => Scalar::I8,
        ir::Type::I16 => Scalar::I16,
        ir::Type::I32 => Scalar::I32,
        ir::Type::I64 => Scalar::I64,
        ir::Type::I128 => Scalar::I128,
        ir::Type::F32 => Scalar::F32,
        ir::Type::F64 => Scalar::F64,
        ir::Type::Ptr => Scalar::Ptr,
        ir::Type::Unit | ir::Type::Agg(_) => {
            return Layout {
                size: 0,
                align: 1,
                stride: 0,
                fields: Vec::new(),
                repr: Repr::Zero,
            }
        }
    };
    Layout {
        size: s.size(),
        align: s.align(),
        stride: s.size(),
        fields: Vec::new(),
        repr: Repr::Scalar(s),
    }
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

/// The fields of a struct, tuple or context, as types.
///
/// The same walk `layout::build` does, which is why the offsets in
/// [`Layout::fields`] line up with this list index for index. It exists
/// because a [`Layout`] answers *where* a field is and not *what* it is, and
/// the reference-counting walk needs both.
pub fn field_types(tables: &Tables, ty: &Ty) -> Vec<Ty> {
    use crate::compiler::semantics::types::{self, TyDef};
    match ty {
        Ty::Tuple(elements) => elements.clone(),
        Ty::Ctx(id) => tables.ctx_type(*id).bindings.iter().map(|(_, t)| t.clone()).collect(),
        Ty::Con(id, args) => match &tables.tycon(*id).def {
            TyDef::Struct { fields, .. } => {
                fields.iter().map(|f| types::substitute(&f.ty, args, None)).collect()
            }
            TyDef::Prim(_) | TyDef::Enum { .. } => Vec::new(),
        },
        _ => Vec::new(),
    }
}

/// One variant's fields, as types, in declaration order.
pub fn variant_types(tables: &Tables, ty: &Ty, variant: usize) -> Vec<Ty> {
    use crate::compiler::semantics::types::{self, TyDef};
    let Ty::Con(id, args) = ty else { return Vec::new() };
    match &tables.tycon(*id).def {
        TyDef::Enum { .. } => match tables.tycon(*id).variants().get(variant) {
            Some(v) => v.fields.iter().map(|f| types::substitute(&f.ty, args, None)).collect(),
            None => Vec::new(),
        },
        TyDef::Prim(_) | TyDef::Struct { .. } => Vec::new(),
    }
}

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

    #[test]
    fn an_alignment_is_its_own_logarithm() {
        assert_eq!(align_shift(1), 0);
        assert_eq!(align_shift(8), 3);
        assert_eq!(align_shift(16), 4);
    }
}
