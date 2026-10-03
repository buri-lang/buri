//! The value model, in LLVM types.
//!
//! `middle::ir` hands a backend a struct, list, `Str`, closure, context or enum
//! as **one SSA value** of [`ir::Type::Agg`] naming the source type whose
//! layout it has (`ir.rs`'s module header, "Aggregates are values"). LLVM
//! cannot hold that: it needs a type. So this file is the flattening
//! VALUE-MODEL.md §5.1 describes, computed from [`middle::layout`] and from
//! nothing else, so that the one place the value model is decided stays the one
//! place.
//!
//! # Two forms, and why there are two
//!
//! **The register form** is what an SSA value *is*: a sequence of [`Slot`]s
//! with no padding between them, held as an LLVM literal struct (or, for one
//! slot, as the bare scalar). Nothing about it is a statement about bytes —
//! padding does not exist in a register, and an SSA value never has an address.
//!
//! **The memory form** is what a heap block or a stack aggregate holds: the
//! same slots at the byte offsets `Layout` gives them, reached by `getelementptr
//! inbounds` ([`byte_offset`]) and moved by one `load`/`store` per slot at the
//! alignment [`access_align`] allows. Loading an aggregate is a load per slot rather than one
//! load of a padded struct type, because a padded LLVM struct type would be a
//! second spelling of the layout table — and two spellings of a layout are how
//! two backends come to disagree about a byte.
//!
//! The pair is what lets CODEGEN-LLVM.md §2.2 hold. There is no `alloca` for an
//! SSA value anywhere, because an SSA value is never in memory: an aggregate is
//! built with `insertvalue` and taken apart with `extractvalue`, both of which
//! are register operations.
//!
//! # The one place bytes are opaque: a tagged enum's payload
//!
//! A tagged enum's payload area is a **union** — variant 0 may put a pointer at
//! offset 8 and variant 1 an `F64` — so there is no one typed decomposition of
//! it, and any attempt to find one has to answer "what is the type of the slot
//! two variants disagree about". [`SlotTy::Blob`] declines the question: the
//! payload area is one `iN` of exactly its bytes, a variant's fields are shifted
//! into and out of it, and the only variant whose fields are ever read is the
//! one a `Switch` on the tag has just established (`ir.rs`, [`ir::Inst::GetPayload`]).
//!
//! What that costs is alias information *inside* a tagged enum: a `Str` in a
//! `Result`'s payload round-trips through `ptrtoint`/`inttoptr`, so LLVM will
//! not reason about it as a pointer while it is in there. What it buys is that
//! `Option<Str>` — the case that matters, and the case VALUE-MODEL.md §6 gave a
//! niche precisely because it matters — is *not* a tagged enum, so it keeps
//! typed pointer slots. The growth path, if a profile ever asks for it, is a
//! per-offset slot union with a canonical type and `bitcast`s at the
//! disagreements; it is a change to this file and to nothing else.

use inkwell::context::Context;
use inkwell::types::BasicTypeEnum;
use inkwell::values::{BasicValue, BasicValueEnum, IntValue, PointerValue};


use crate::compiler::backend::counts::{Counted, Counts, Site};
use crate::compiler::middle::ir;
use crate::compiler::middle::layout::{
    self, Cycles, EnumRepr, Layout, Layouts, Repr as LayoutRepr, Scalar,
};
use crate::compiler::semantics::types::{Tables, Ty};
use crate::hash::Map;

/// What one machine-sized piece of an aggregate is.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SlotTy {
    Scalar(Scalar),
    /// A tagged enum's payload area, as an integer of exactly its bytes.
    Blob(u32),
}

impl SlotTy {
    pub fn size(self) -> u32 {
        match self {
            SlotTy::Scalar(s) => s.size(),
            SlotTy::Blob(bytes) => bytes,
        }
    }

    /// The alignment a `load` or `store` of this slot may claim.
    ///
    /// A blob is aligned to what its enum is aligned to, which the caller
    /// knows and this does not; the conservative answer here is one byte, and
    /// [`access_align`] raises it from the layout.
    fn align(self) -> u32 {
        match self {
            SlotTy::Scalar(s) => s.align(),
            SlotTy::Blob(_) => 1,
        }
    }

    pub fn is_pointer(self) -> bool {
        matches!(self, SlotTy::Scalar(Scalar::Ptr))
    }
}

/// One piece of an aggregate: what it is, and where it is in the bytes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Slot {
    pub offset: u32,
    pub ty: SlotTy,
}

/// One aggregate type's flattening: the whole answer, computed once.
pub struct Repr {
    pub layout: Layout,
    pub slots: Vec<Slot>,
    /// The slot range `[start, end)` of each field, in declaration order.
    /// Empty for an enum and for a scalar.
    pub fields: Vec<(usize, usize)>,
    /// For each slot, whether a reference count lives behind it.
    pub counted: Vec<Option<Counted>>,
    /// The source type, kept so that an enum's variants and a list's element
    /// can be asked about without a second lookup.
    pub ty: Ty,
}

impl Repr {
    /// The enum discriminant encoding, where this is an enum.
    pub fn enum_repr(&self) -> Option<&EnumRepr> {
        match &self.layout.repr {
            LayoutRepr::Enum { repr, .. } => Some(repr),
            _ => None,
        }
    }

    pub fn field_range(&self, index: usize) -> (usize, usize) {
        self.fields.get(index).copied().unwrap_or((0, 0))
    }

}

/// The flattening table, one per unit. Memoised on the interned
/// [`ir::TypeId`], because a unit names the same twenty types thousands of
/// times.
pub struct Reprs<'a> {
    tables: &'a Tables,
    layouts: Layouts<'a>,
    /// Keyed on the id rather than indexed by it: a row per type the *program*
    /// interned, built fresh per unit, is a large allocation and a large memset
    /// for the twenty entries a unit fills (`design/PERFORMANCE.md` §6.4's
    /// first finding).
    memo: Map<usize, Repr>,
    by_ty: Map<Ty, usize>,
    side: Vec<Repr>,
    /// The answer for a lookup that cannot happen: every id was minted by this
    /// table, so a miss is an internal inconsistency. A zero-sized repr rather
    /// than a panic, because the lint set forbids one and "this value occupies
    /// nothing" degrades rather than crashes.
    empty: Repr,
    counts: Counts,
}

impl<'a> Reprs<'a> {
    /// Whether `owner` keeps `member` behind an indirection — the box
    /// `middle::layout` puts where a recursive field would be
    /// (VALUE-MODEL.md §5.2).
    ///
    /// [`Reprs::place`] already asks it while flattening, and a backend has to
    /// ask it again for a different reason: a boxed field's *slots* are one
    /// pointer, and the value being stored into it has the field's own. So
    /// building one is an allocation and reading one is a load, and this is
    /// the question that says which fields those are.
    pub fn boxes(&mut self, owner: &Ty, member: &Ty) -> bool {
        self.layouts.boxes(owner, member)
    }

    /// `cycles` is the recursion analysis of these same `tables`, taken once
    /// for the emission rather than once per unit: see [`Cycles`].
    pub fn new(tables: &'a Tables, cycles: std::sync::Arc<Cycles>) -> Reprs<'a> {
        Reprs {
            tables,
            layouts: Layouts::with_cycles(tables, cycles),
            memo: Map::default(),
            by_ty: Map::default(),
            side: Vec::new(),
            empty: Repr {
                layout: Layout {
                    size: 0,
                    align: 1,
                    stride: 0,
                    fields: Vec::new(),
                    repr: LayoutRepr::Zero,
                },
                slots: Vec::new(),
                fields: Vec::new(),
                counted: Vec::new(),
                ty: Ty::Unit,
            },
            counts: Counts::default(),
        }
    }

    /// The flattening of an interned IR type.
    pub fn of(&mut self, program: &ir::Program, id: ir::TypeId) -> &Repr {
        if !self.memo.contains_key(&id.index()) {
            let ty = program.type_info(id).ty.clone();
            let repr = self.build(&ty);
            self.memo.insert(id.index(), repr);
        }
        self.memo.get(&id.index()).unwrap_or(&self.empty)
    }

    /// The flattening of a type that is not interned in the IR — an enum's
    /// variant field, a list's element, a boxed payload.
    pub fn of_ty(&mut self, ty: &Ty) -> &Repr {
        if let Some(&at) = self.by_ty.get(ty) {
            return self.side.get(at).unwrap_or(&self.empty);
        }
        let repr = self.build(ty);
        let at = self.side.len();
        self.side.push(repr);
        self.by_ty.insert(ty.clone(), at);
        self.side.get(at).unwrap_or(&self.empty)
    }

    fn build(&mut self, ty: &Ty) -> Repr {
        let layout = self.layouts.of(ty.clone());
        let mut slots = Vec::new();
        let mut counted = Vec::new();
        let mut fields = Vec::new();
        match &layout.repr {
            LayoutRepr::Zero => {}
            LayoutRepr::Scalar(s) => {
                slots.push(Slot { offset: 0, ty: SlotTy::Scalar(*s) });
                counted.push(None);
            }
            // `{ base, ptr, len }`. `base` is the count — `ptr` is a view into
            // it and is not a payload start (VALUE-MODEL.md §3) — and `base` is
            // null for a literal, which is why it is `Nullable`.
            LayoutRepr::Str => {
                slots.push(Slot { offset: layout.field(layout::STR_BASE), ty: ptr() });
                slots.push(Slot { offset: layout.field(layout::STR_PTR), ty: ptr() });
                slots.push(Slot {
                    offset: layout.field(layout::STR_LEN),
                    ty: SlotTy::Scalar(Scalar::I64),
                });
                counted.extend([Some(Counted::Nullable), None, None]);
                fields.extend([(0, 1), (1, 2), (2, 3)]);
            }
            // `{ ptr, len }`. A list is never a view, so `ptr` is a payload
            // start and the header is at `ptr - 16` (§4).
            //
            // **`Nullable`, not `NonNull`.** VALUE-MODEL.md §4 says a list is
            // one block, so `NonNull` describes every *non-empty* list. It
            // does not describe every list that reaches this backend:
            // `cli/runtime/list.rs`'s `block` answers a null `ptr` for a list
            // of no elements — "an empty `[T]` allocates nothing, which is what
            // makes `list.empty` free" — so `xs.slice(ctx, 2, 2)` comes back
            // with one, and so does `emit.rs`'s own `empty_list`.
            // A `NonNull` claim on that value is a `load` at `null - 16` in the
            // first `incref` or `decref` that touches it, which is a segfault
            // rather than a wrong answer.
            //
            // The alternative, rejected: keep `NonNull` and have every
            // list-producing entry in `runtime::ENTRIES` normalize a null `ptr`
            // to a real zero-byte block. That is a branch and possibly an
            // allocation per call, at every one of them, to save a null test
            // that only the ones that can be empty ever fail — and it would
            // have to be repeated in the debug backend, where the same runtime
            // answers the same null.
            LayoutRepr::List => {
                slots.push(Slot { offset: layout.field(layout::LIST_PTR), ty: ptr() });
                slots.push(Slot {
                    offset: layout.field(layout::LIST_LEN),
                    ty: SlotTy::Scalar(Scalar::I64),
                });
                counted.extend([Some(Counted::Nullable), None]);
                fields.extend([(0, 1), (1, 2)]);
            }
            // `{ code, env }`. `code` is a function pointer and never counted;
            // `env` is null when nothing was captured (§7).
            LayoutRepr::Closure => {
                slots.push(Slot { offset: layout.field(layout::CLOSURE_CODE), ty: ptr() });
                slots.push(Slot { offset: layout.field(layout::CLOSURE_ENV), ty: ptr() });
                counted.extend([None, Some(Counted::Nullable)]);
                fields.extend([(0, 1), (1, 2)]);
            }
            LayoutRepr::Aggregate => {
                let members = self.members(ty);
                for (index, member) in members.iter().enumerate() {
                    let at = layout.field(index);
                    let start = slots.len();
                    self.place(ty, member, at, &mut slots, &mut counted);
                    fields.push((start, slots.len()));
                }
            }
            LayoutRepr::Enum { repr, .. } => match repr {
                // The value *is* the tag, so there is nothing to flatten and
                // nothing to count (§6, first niche).
                EnumRepr::Bare { tag } => {
                    slots.push(Slot { offset: 0, ty: SlotTy::Scalar(*tag) });
                    counted.push(None);
                }
                // The value *is* the payload, with null for `.None` (§6,
                // second niche), so its slots are the payload's — and every
                // count in them becomes nullable, because `.None` is the null
                // this niche spends.
                EnumRepr::Niche { .. } => {
                    let payload = self.option_payload(ty);
                    if let Some(payload) = payload {
                        self.place(ty, &payload, 0, &mut slots, &mut counted);
                        for c in &mut counted {
                            if c.is_some() {
                                *c = Some(Counted::Nullable);
                            }
                        }
                    }
                }
                // `tag ++ payload`, with the payload area opaque — see the
                // module header. Counts inside it are reached by the generated
                // drop glue, which switches on the tag first, and not by a
                // slot walk, which is why every slot here is `None`.
                EnumRepr::Tagged { tag, payload } => {
                    slots.push(Slot { offset: 0, ty: SlotTy::Scalar(*tag) });
                    counted.push(None);
                    let bytes = layout.size.saturating_sub(*payload);
                    if bytes > 0 {
                        slots.push(Slot { offset: *payload, ty: SlotTy::Blob(bytes) });
                        counted.push(None);
                    }
                }
            },
        }
        Repr { layout, slots, fields, counted, ty: ty.clone() }
    }

    /// Places one member's slots inside its owner, at `at`.
    ///
    /// A member the layout table boxes — the indirection a recursive type gets
    /// (VALUE-MODEL.md §5.2) — is one non-null pointer slot and the recursion
    /// stops there, which is the same place it stops in `layout::record` and is
    /// what makes this terminate for `enum Tree { Node(Tree, Tree) }`.
    fn place(
        &mut self,
        owner: &Ty,
        member: &Ty,
        at: u32,
        slots: &mut Vec<Slot>,
        counted: &mut Vec<Option<Counted>>,
    ) {
        if self.layouts.boxes(owner, member) {
            slots.push(Slot { offset: at, ty: ptr() });
            counted.push(Some(Counted::NonNull));
            return;
        }
        let inner = self.of_ty(member);
        let inner_slots = inner.slots.clone();
        let inner_counted = inner.counted.clone();
        for (slot, count) in inner_slots.into_iter().zip(inner_counted) {
            slots.push(Slot { offset: at.saturating_add(slot.offset), ty: slot.ty });
            counted.push(count);
        }
    }

    /// The declared members of a record-shaped type, substituted.
    ///
    /// The same walk `layout::build` does, and it has to be: a member list that
    /// disagreed with the one the offsets were computed from is a value whose
    /// fields are read from the wrong words.
    fn members(&self, ty: &Ty) -> Vec<Ty> {
        field_types(self.tables, ty)
    }

    /// The stride `middle::layout` gives an element type, never zero.
    ///
    /// A zero stride would make a `[T]`'s element count unrecoverable from
    /// `cap` and would make the release loop over its elements not terminate;
    /// a zero-sized element has no counts to release either way, so one is the
    /// harmless floor.
    pub fn stride_of(&mut self, ty: &Ty) -> u32 {
        self.of_ty(ty).layout.stride.max(1)
    }

    /// See `Layouts::glue_key`.
    pub fn glue_key(&mut self, ty: &Ty) -> std::rc::Rc<str> {
        self.layouts.glue_key(ty)
    }

    /// Whether a value of this type owns any reference count at all.
    pub fn counted_type(&mut self, ty: &Ty) -> bool {
        self.counts.counted(self.tables, &mut self.layouts, ty)
    }

    /// Every place a reference count lives inside one value of this type.
    pub fn sites(&mut self, ty: &Ty) -> std::rc::Rc<[Site]> {
        self.counts.sites(self.tables, &mut self.layouts, ty)
    }

    /// `T` of an `Option<T>` that took the niche.
    fn option_payload(&self, ty: &Ty) -> Option<Ty> {
        match ty {
            Ty::Con(_, args) => args.first().cloned(),
            _ => None,
        }
    }

    /// The element type of a `[T]`.
    pub fn element(&self, ty: &Ty) -> Option<Ty> {
        match ty {
            Ty::Array(t) => Some((**t).clone()),
            _ => None,
        }
    }
}

fn ptr() -> SlotTy {
    SlotTy::Scalar(Scalar::Ptr)
}

/// How deep a reference-count walk descends before concluding the type graph
/// has a cycle the boxing rule failed to cut. A fuse, not a limit:
/// `Layouts::boxes` cuts every cycle, so reaching it is an inconsistency and
/// stopping is the conservative answer — a leak rather than a stack overflow in
/// the compiler.
pub const RC_DEPTH: u32 = 64;

/// What is inside a struct, a tuple, a context or one enum variant.
///
/// `semantics::types` owns the walk; it is re-exported here because `repr::`
/// is where this file's callers already look for it.
pub use crate::compiler::semantics::types::{field_types, variant_types};

// ---------------------------------------------------------------------------
// Slots as LLVM types
// ---------------------------------------------------------------------------

/// The LLVM type of one slot.
pub fn slot_type<'ctx>(ctx: &'ctx Context, ty: SlotTy) -> BasicTypeEnum<'ctx> {
    match ty {
        SlotTy::Scalar(Scalar::Bool) => ctx.bool_type().into(),
        SlotTy::Scalar(Scalar::I8) => ctx.i8_type().into(),
        SlotTy::Scalar(Scalar::I16) => ctx.i16_type().into(),
        SlotTy::Scalar(Scalar::I32) => ctx.i32_type().into(),
        SlotTy::Scalar(Scalar::I64) => ctx.i64_type().into(),
        SlotTy::Scalar(Scalar::I128) => ctx.i128_type().into(),
        SlotTy::Scalar(Scalar::F32) => ctx.f32_type().into(),
        SlotTy::Scalar(Scalar::F64) => ctx.f64_type().into(),
        SlotTy::Scalar(Scalar::Ptr) => ctx.ptr_type(inkwell::AddressSpace::default()).into(),
        SlotTy::Blob(bytes) => blob_type(ctx, bytes).into(),
    }
}

/// `iN` for a payload area of `bytes` bytes.
///
/// `NonZeroU32::new` is checked rather than asserted because the lint set
/// forbids a panic; a zero-byte blob is not produced (a payload area of no
/// bytes is the bare-integer niche) and `i8` is the harmless answer if one
/// ever were.
pub fn blob_type(ctx: &Context, bytes: u32) -> inkwell::types::IntType<'_> {
    match std::num::NonZeroU32::new(bytes.saturating_mul(8)) {
        Some(bits) => ctx.custom_width_int_type(bits).unwrap_or_else(|_| ctx.i8_type()),
        None => ctx.i8_type(),
    }
}

/// The register form of a list of slots.
///
/// One slot is the bare scalar rather than a struct of one, because a `Str`
/// length that arrived as `{ i64 }` would need an `extractvalue` in front of
/// every arithmetic operation on it and would read that way in the dump.
pub fn register_type<'ctx>(ctx: &'ctx Context, slots: &[Slot]) -> BasicTypeEnum<'ctx> {
    match slots {
        [] => ctx.struct_type(&[], false).into(),
        [one] => slot_type(ctx, one.ty),
        many => {
            let tys: Vec<BasicTypeEnum<'ctx>> =
                many.iter().map(|s| slot_type(ctx, s.ty)).collect();
            ctx.struct_type(&tys, false).into()
        }
    }
}

/// The register form of an [`ir::Type`].
pub fn ir_type<'ctx>(
    ctx: &'ctx Context,
    reprs: &mut Reprs<'_>,
    program: &ir::Program,
    ty: ir::Type,
) -> BasicTypeEnum<'ctx> {
    match ty {
        ir::Type::I1 => ctx.bool_type().into(),
        ir::Type::I8 => ctx.i8_type().into(),
        ir::Type::I16 => ctx.i16_type().into(),
        ir::Type::I32 => ctx.i32_type().into(),
        ir::Type::I64 => ctx.i64_type().into(),
        ir::Type::I128 => ctx.i128_type().into(),
        ir::Type::F32 => ctx.f32_type().into(),
        ir::Type::F64 => ctx.f64_type().into(),
        ir::Type::Ptr => ctx.ptr_type(inkwell::AddressSpace::default()).into(),
        ir::Type::Unit => ctx.struct_type(&[], false).into(),
        ir::Type::Agg(id) => {
            let slots = reprs.of(program, id).slots.clone();
            register_type(ctx, &slots)
        }
    }
}

/// The slots of an [`ir::Type`]: one for a scalar, none for `()`, the
/// aggregate's own for an [`ir::Type::Agg`].
pub fn ir_slots(reprs: &mut Reprs<'_>, program: &ir::Program, ty: ir::Type) -> Vec<Slot> {
    let scalar = |s: Scalar| vec![Slot { offset: 0, ty: SlotTy::Scalar(s) }];
    match ty {
        ir::Type::I1 => scalar(Scalar::Bool),
        ir::Type::I8 => scalar(Scalar::I8),
        ir::Type::I16 => scalar(Scalar::I16),
        ir::Type::I32 => scalar(Scalar::I32),
        ir::Type::I64 => scalar(Scalar::I64),
        ir::Type::I128 => scalar(Scalar::I128),
        ir::Type::F32 => scalar(Scalar::F32),
        ir::Type::F64 => scalar(Scalar::F64),
        ir::Type::Ptr => scalar(Scalar::Ptr),
        ir::Type::Unit => Vec::new(),
        ir::Type::Agg(id) => reprs.of(program, id).slots.clone(),
    }
}

// ---------------------------------------------------------------------------
// Moving a value between the register form and memory
// ---------------------------------------------------------------------------

/// The alignment a slot's access may claim: the natural alignment of the slot,
/// capped by what the containing value is aligned to and by where in it the
/// slot sits. A slot at an odd offset inside an 8-aligned value is 1-aligned,
/// and claiming more would be a lie LLVM is entitled to act on.
pub fn access_align(container_align: u32, slot: Slot) -> u32 {
    let from_offset = if slot.offset == 0 { container_align } else { 1 << slot.offset.trailing_zeros() };
    slot.ty.align().min(container_align).min(from_offset).max(1)
}

/// A `getelementptr inbounds i8, ptr %base, i64 offset`.
///
/// `inbounds` without exception, per CODEGEN-LLVM.md §3.4: every projection in
/// this language is a field of a known layout or an index a bounds check has
/// already turned into an `Option`, so the premise is enforced by the type
/// system rather than assumed away.
pub fn byte_offset<'ctx>(
    ctx: &'ctx Context,
    builder: &inkwell::builder::Builder<'ctx>,
    base: PointerValue<'ctx>,
    offset: i64,
    name: &str,
) -> PointerValue<'ctx> {
    if offset == 0 {
        return base;
    }
    let index = ctx.i64_type().const_int(offset as u64, true);
    // SAFETY: `build_in_bounds_gep` is `unsafe` in inkwell because it cannot
    // check the index against the pointee type. The offset here comes from the
    // layout table for the very type this pointer points at.
    unsafe {
        builder
            .build_in_bounds_gep(ctx.i8_type(), base, &[index], name)
            .unwrap_or(base)
    }
}

/// Assembles a register value from its slot values.
pub fn assemble<'ctx>(
    ctx: &'ctx Context,
    builder: &inkwell::builder::Builder<'ctx>,
    slots: &[Slot],
    values: &[BasicValueEnum<'ctx>],
) -> BasicValueEnum<'ctx> {
    match (slots, values) {
        ([], _) => ctx.struct_type(&[], false).const_zero().into(),
        ([_], [one]) => *one,
        (many, vals) => {
            let ty = register_type(ctx, many);
            let mut acc: BasicValueEnum<'ctx> = match ty {
                BasicTypeEnum::StructType(s) => s.get_poison().into(),
                other => other.const_zero(),
            };
            for (i, v) in vals.iter().enumerate() {
                if let BasicValueEnum::StructValue(s) = acc {
                    if let Ok(next) = builder.build_insert_value(s, *v, i as u32, "agg") {
                        acc = next.as_basic_value_enum();
                    }
                }
            }
            acc
        }
    }
}

/// Takes a register value apart into its slot values.
pub fn disassemble<'ctx>(
    builder: &inkwell::builder::Builder<'ctx>,
    slots: &[Slot],
    value: BasicValueEnum<'ctx>,
) -> Vec<BasicValueEnum<'ctx>> {
    match slots.len() {
        0 => Vec::new(),
        1 => vec![value],
        n => {
            let mut out = Vec::with_capacity(n);
            for i in 0..n {
                let piece = match value {
                    BasicValueEnum::StructValue(s) => {
                        builder.build_extract_value(s, i as u32, "slot").unwrap_or(value)
                    }
                    other => other,
                };
                out.push(piece);
            }
            out
        }
    }
}

/// A slot's value as an integer of its own width, for packing into a blob.
pub fn slot_to_bits<'ctx>(
    ctx: &'ctx Context,
    builder: &inkwell::builder::Builder<'ctx>,
    slot: Slot,
    value: BasicValueEnum<'ctx>,
) -> IntValue<'ctx> {
    let bits = slot.ty.size().saturating_mul(8);
    let int = blob_type(ctx, slot.ty.size());
    match value {
        BasicValueEnum::PointerValue(p) => {
            builder.build_ptr_to_int(p, ctx.i64_type(), "p2i").unwrap_or_else(|_| int.const_zero())
        }
        BasicValueEnum::FloatValue(f) => {
            let as_int = builder.build_bit_cast(f, int, "f2i").unwrap_or_else(|_| int.const_zero().into());
            as_int.try_into().unwrap_or_else(|_| int.const_zero())
        }
        BasicValueEnum::IntValue(i) => {
            if i.get_type().get_bit_width() == bits {
                i
            } else {
                builder
                    .build_int_z_extend_or_bit_cast(i, int, "widen")
                    .unwrap_or_else(|_| int.const_zero())
            }
        }
        other => {
            let _ = other;
            int.const_zero()
        }
    }
}

/// The inverse of [`slot_to_bits`].
pub fn slot_from_bits<'ctx>(
    ctx: &'ctx Context,
    builder: &inkwell::builder::Builder<'ctx>,
    slot: Slot,
    bits: IntValue<'ctx>,
) -> BasicValueEnum<'ctx> {
    let want = slot_type(ctx, slot.ty);
    match want {
        BasicTypeEnum::PointerType(p) => builder
            .build_int_to_ptr(bits, p, "i2p")
            .map(|v| v.as_basic_value_enum())
            .unwrap_or_else(|_| p.const_null().into()),
        BasicTypeEnum::FloatType(f) => builder
            .build_bit_cast(bits, f, "i2f")
            .unwrap_or_else(|_| f.const_zero().into()),
        BasicTypeEnum::IntType(i) => {
            if bits.get_type().get_bit_width() == i.get_bit_width() {
                bits.into()
            } else {
                builder
                    .build_int_truncate_or_bit_cast(bits, i, "narrow")
                    .map(|v| v.as_basic_value_enum())
                    .unwrap_or_else(|_| i.const_zero().into())
            }
        }
        other => other.const_zero(),
    }
}
