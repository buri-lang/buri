//! The value model, as a computed table.
//!
//! Sizes, alignments, strides, field offsets, discriminant encodings and the
//! two niches, computed once per type and memoised. It is in the middle end
//! rather than in a backend because both native backends must agree byte for
//! byte: an `[T]` whose element stride the two native backends disagree about is a
//! miscompile that only shows up between profiles.
//!
//! It is also the reason this directory is called `middle` and not `transform`.
//! A layout table is not a transformation of anything.
//!
//! # The interface, fixed before the content
//!
//! `middle::lower` depends on this module's *interface* and not its content —
//! the lowering asks for sizes and offsets and does not care how they were
//! computed. So the signature is stated here, and everything that reads a
//! layout is written against it (`design/native/BUILD-AND-WATCH.md` §5):
//!
//! ```text
//! pub struct Layout {
//!     pub size: u32,      // bytes the value occupies
//!     pub align: u32,     // power of two
//!     pub stride: u32,    // size rounded up to align; what `[T]` indexes by
//!     pub fields: Vec<u32>,  // byte offset per field, declaration order
//! }
//!
//! pub struct Layouts { .. }   // the memo table, one per compilation
//!
//! impl Layouts {
//!     pub fn new(tables: &Tables) -> Layouts;
//!     pub fn of(&mut self, ty: Ty) -> Layout;
//! }
//! ```
//!
//! `of` takes `&mut self` because it memoises, and it is infallible because
//! every type reaching it has been checked: a type the front end admitted has a
//! layout, and a recursive one is behind a pointer by VALUE-MODEL.md §4.
//!
//! [`Layout`] carries one field beyond the four agreed — [`Layout::repr`],
//! which says *what shape* the four numbers describe. `middle::lower` ignores
//! it; the backends cannot, because "24 bytes at alignment 8" does not say
//! which of the three words is the pointer a `decref` takes, and neither
//! backend should be rediscovering that from the `Ty`.
//!
//! # What is decided here, and where each rule comes from
//!
//! * **Scalars** (VALUE-MODEL.md §1). `Int` is `i64`, `Char` is `i32`, `Bool`
//!   is one byte in memory, `()` is nothing.
//! * **`Str`** (§3) is `{ base, ptr, len }`, 24 bytes, with the ASCII flag in
//!   bit 63 of `len` ([`STR_ASCII_FLAG`]). `Template` is `Str`.
//! * **`[T]`** (§4) is `{ ptr, len }`, 16 bytes, elements at `stride(T)`.
//! * **Tuples and structs** (§5) are declaration order, natural alignment, C
//!   rules, nothing reordered.
//! * **Enums** (§6) are `tag ++ payload`, with the two day-one niches: an enum
//!   whose payload area is empty is a bare integer, and `Option<T>` over a
//!   `T` carrying a known-non-null pointer is that `T` with null for `.None`.
//! * **Closures** (§7) are `{ code, env }`, 16 bytes.
//! * **Contexts** (§8) are a record of exactly the implementations that carry
//!   state, which for `core/host` is none of them — so the context is
//!   zero-sized and [`Layouts::zero_sized`] is what drops it from a signature.
//! * **The heap header** (MEMORY.md §2) is [`HEADER_BYTES`] immediately before
//!   every payload, which is why `[T]`'s `ptr` is a payload start and `Str`'s
//!   is not.
//! * **The `Alloc` cost model** (MEMORY.md §7.1) is [`Layouts::charge_list`]
//!   and friends: a *defined* charge computed from this table, so both
//!   backends charge the same number for the same program.
//!
//! Design: `design/native/VALUE-MODEL.md`, and the `Alloc` cost model in
//! `MEMORY.md` §7.1.

use crate::compiler::semantics::types::{self, Prim, Tables, Ty, TyConId, TyDef};
use crate::diagnostics::Invariant as _;
use crate::hash::Map;
use std::fmt::Write as _;
use std::rc::Rc;
use std::sync::Arc;

// ---------------------------------------------------------------------------
// The constants the model pins
// ---------------------------------------------------------------------------

/// The reference-count header, immediately before every heap payload
/// (MEMORY.md §2): `{ rc: u64, cap: u64 }`.
///
/// Sixteen and not eight so the payload stays 16-byte aligned and so `cap` is
/// there for the free path and for MEMORY.md §5.3's in-place reuse.
pub const HEADER_BYTES: u32 = 16;

/// Byte offset of `rc` from a payload pointer. Negative: the header is behind
/// the value, so that a `Str` view and a `[T]` payload are the same kind of
/// pointer.
pub const HEADER_RC_OFFSET: i32 = -16;

/// Byte offset of `cap` from a payload pointer.
///
/// The word holds the usable payload bytes in its low 63 bits and
/// [`CAP_SHARED_FLAG`] in bit 63, so every reader masks with [`CAP_MASK`].
pub const HEADER_CAP_OFFSET: i32 = -8;

/// Bit 63 of `cap`: **reserved** for the multi-threaded mark, and never set.
///
/// Set will mean "this block may be reached from more than one thread", which
/// is the question `incref`/`decref` will branch on to choose an atomic
/// count. Nothing in the tree sets it yet; the reservation is here so that the
/// readers already mask, and so the bit cannot be spent twice.
///
/// **Why `cap` and not `rc`.** A bit of the count would cost both of the two
/// properties the count has. `IMMORTAL` is `u64::MAX` and `incref` is a
/// *saturating* add precisely so the sentinel is a fixed point with no branch
/// on the hot side ([`IMMORTAL`]); a tag bit in the same word makes the
/// saturating add wrong. And MEMORY.md §5.3's whole licence for in-place reuse
/// is the literal test `rc == 1` (`memory.rs::buri_rt_unique_cap`), which a
/// tagged count fails for a block that is unique. `cap` has neither problem:
/// it is a byte count nobody does arithmetic on without knowing it is one, it
/// is read on cold paths (the free path, a growth decision, a drop glue's
/// element count), and a capacity is bounded by the address space long before
/// bit 63.
///
/// This follows [`STR_ASCII_FLAG`], which spends bit 63 of `Str::len` the same
/// way (VALUE-MODEL.md §3.1). `design/native/VALUE-MODEL.md` §2 records both.
pub const CAP_SHARED_FLAG: u64 = 1 << 63;

/// Bit 62 of `cap`: the block was served out of a `core/alloc::scoped` arena
/// and is not the platform allocator's to give back.
///
/// Set and read by the **runtime** alone
/// (`cli/runtime/memory.rs`'s `BURI_RT_CAP_ARENA`): a scope's `free` does the
/// accounting and returns, and the pages go back in one `munmap` when the scope
/// ends. Nothing a backend emits tests it. It is declared here because
/// [`CAP_MASK`] is, and every reader of a `cap` word in emitted code masks with
/// that — so the bit cannot be spent twice and an element count cannot pick it
/// up.
pub const CAP_ARENA_FLAG: u64 = 1 << 62;

/// The usable payload bytes of a block, once the flag bits are off.
pub const CAP_MASK: u64 = !(CAP_SHARED_FLAG | CAP_ARENA_FLAG);

/// `rc == IMMORTAL` is a value that is never counted and never freed: every
/// literal, every interned constant aggregate, every zero-sized value.
/// `incref` saturates, so `IMMORTAL` stays `IMMORTAL` without a branch.
pub const IMMORTAL: u64 = u64::MAX;

/// The smallest capacity a block a backend grows is given, in bytes.
///
/// MEMORY.md §5.3's growth policy is **doubling with a floor**, and this is the
/// floor: a block that is about to be appended to is allocated
/// `max(needed * 2, GROWTH_FLOOR)` bytes, so that a chain of concatenations
/// reallocates O(log n) times instead of once per step and the first few steps
/// do not reallocate at all.
///
/// `cli/runtime/memory.rs`'s `BURI_RT_GROWTH_FLOOR` is the same number for the
/// operations the runtime owns (`list.push`, `list.concat`). The two are
/// deliberately separate constants rather than a shared one — the compiler and
/// the runtime are two crates that never link against each other, which is the
/// same reason `BURI_OK` is spelled twice — and a disagreement between them
/// costs a reallocation, not an answer.
pub const GROWTH_FLOOR: u64 = 64;

/// Bit 63 of `Str::len`: set means every byte of the view is below `0x80`, so
/// the scalar count is the byte count and `str.len()` is a mask
/// (VALUE-MODEL.md §3.1).
pub const STR_ASCII_FLAG: u64 = 1 << 63;

/// The byte length of a `Str`, once the ASCII flag is off it.
pub const STR_LEN_MASK: u64 = !STR_ASCII_FLAG;

/// Field indices into a `Str`'s [`Layout::fields`].
pub const STR_BASE: usize = 0;
/// The bytes themselves. Non-null even for the empty string, which is what
/// makes it the niche `Option<Str>` uses.
pub const STR_PTR: usize = 1;
/// Scalar count, with [`STR_ASCII_FLAG`] in the top bit.
pub const STR_LEN: usize = 2;

/// Field indices into a `[T]`'s [`Layout::fields`].
pub const LIST_PTR: usize = 0;
/// Element count, exactly — no flag, because `list.len()` is always O(1).
pub const LIST_LEN: usize = 1;

/// Field indices into a closure's [`Layout::fields`].
pub const CLOSURE_CODE: usize = 0;
/// Null when the lambda captured nothing (VALUE-MODEL.md §7).
pub const CLOSURE_ENV: usize = 1;

/// A pointer, on every target this compiles for.
const POINTER: u32 = 8;

/// How deep inline nesting may go before this decides it is looking at a cycle
/// the recursion rule failed to cut.
///
/// It is a fuse and not a limit. [`Layouts::boxes`] cuts every cycle in the type
/// graph, so reaching this is an internal inconsistency rather than a deeply
/// nested program, and the number is set well above anything a program can
/// reach so that it never turns *legal* input into an error: the parser caps a
/// written type at `parser::MAX_DEPTH` (256), and a chain of declarations that
/// long would exhaust `main.rs`'s 256 MiB stack in `substitute` before it
/// reached here. Having the fuse means an inconsistency is a named internal
/// error rather than a compiler that does not terminate.
const MAX_NESTING: u32 = 4096;

// ---------------------------------------------------------------------------
// Layouts
// ---------------------------------------------------------------------------

/// One machine scalar: what a backend puts in a register.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Scalar {
    /// `i1` in a register, one byte in memory, values 0 and 1 only.
    Bool,
    I8,
    I16,
    I32,
    I64,
    I128,
    F32,
    F64,
    /// An address. Whether it may be null is the [`Repr`]'s business, not the
    /// scalar's.
    Ptr,
}

impl Scalar {
    pub fn size(self) -> u32 {
        match self {
            Scalar::Bool | Scalar::I8 => 1,
            Scalar::I16 => 2,
            Scalar::I32 | Scalar::F32 => 4,
            Scalar::I64 | Scalar::F64 | Scalar::Ptr => 8,
            Scalar::I128 => 16,
        }
    }

    /// Natural alignment, which for every scalar here is its size. `i128` is
    /// 16-aligned rather than 8-aligned: VALUE-MODEL.md §1 does not say, and
    /// this is what LLVM, clang and the SysV ABI all mean by `i128`, so
    /// choosing anything else would make `cli/runtime`'s `#[repr(C)]` types
    /// disagree with generated code at the one boundary that must not drift.
    pub fn align(self) -> u32 {
        self.size()
    }

    pub fn name(self) -> &'static str {
        match self {
            Scalar::Bool => "bool",
            Scalar::I8 => "i8",
            Scalar::I16 => "i16",
            Scalar::I32 => "i32",
            Scalar::I64 => "i64",
            Scalar::I128 => "i128",
            Scalar::F32 => "f32",
            Scalar::F64 => "f64",
            Scalar::Ptr => "ptr",
        }
    }
}

/// How an enum carries its discriminant.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum EnumRepr {
    /// The value *is* the tag: every variant's payload area is empty
    /// (VALUE-MODEL.md §6, first niche). Equality is an integer compare.
    Bare { tag: Scalar },
    /// `Option<T>` where `T` carries a pointer that is never null: the value
    /// is a `T`, and `.None` is that pointer set to null (§6, second niche).
    /// `null_at` is its byte offset.
    Niche { null_at: u32 },
    /// `tag ++ payload`. The tag is at offset 0 and holds the variant's index
    /// in declaration order; the payload area starts at `payload`.
    Tagged { tag: Scalar, payload: u32 },
}

/// What shape a [`Layout`]'s numbers describe.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Repr {
    /// Never passed, never stored, never loaded: `()`, an empty struct, a
    /// context of zero-sized implementations, an uninhabited enum.
    Zero,
    Scalar(Scalar),
    /// `{ base, ptr, len }` (VALUE-MODEL.md §3). `base` is null for a literal.
    Str,
    /// `{ ptr, len }` (§4). `ptr` is a payload start, so the header is at
    /// `ptr - 16`.
    List,
    /// `{ code, env }` (§7). `env` is null when nothing was captured.
    Closure,
    /// Fields at [`Layout::fields`], in declaration order.
    Aggregate,
    /// Variant field offsets are absolute — from the start of the enum value,
    /// not from the start of the payload area — so a projection needs no
    /// second addition.
    Enum { repr: EnumRepr, variants: Vec<Vec<u32>> },
}

/// What a type looks like in memory.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Layout {
    /// Bytes the value occupies.
    pub size: u32,
    /// Power of two, at least 1.
    pub align: u32,
    /// `size` rounded up to `align`; what `[T]` indexes by.
    pub stride: u32,
    /// Byte offset per field, in declaration order. Empty for a scalar and for
    /// an enum, whose offsets are per variant and live in [`Repr::Enum`].
    pub fields: Vec<u32>,
    pub repr: Repr,
}

impl Layout {
    /// Never passed, never stored, never loaded — and dropped from every
    /// signature it appears in (VALUE-MODEL.md §8).
    pub fn is_zero_sized(&self) -> bool {
        self.size == 0
    }

    /// The offset of one field. Out of range is an internal error rather than
    /// a panic: a backend asking for field 3 of a two-field struct has already
    /// gone wrong somewhere the index would not explain.
    pub fn field(&self, index: usize) -> u32 {
        *self.fields.get(index).or_ice("a field index came from the type this layout is of")
    }

    /// The offsets of one variant's fields, absolute.
    pub fn variant(&self, index: usize) -> &[u32] {
        match &self.repr {
            Repr::Enum { variants, .. } => variants
                .get(index)
                .map(Vec::as_slice)
                .or_ice("a variant index came from the enum this layout is of"),
            _ => &[],
        }
    }

    const fn zero() -> Layout {
        Layout { size: 0, align: 1, stride: 0, fields: Vec::new(), repr: Repr::Zero }
    }

    fn scalar(s: Scalar) -> Layout {
        Layout {
            size: s.size(),
            align: s.align(),
            stride: s.size(),
            fields: Vec::new(),
            repr: Repr::Scalar(s),
        }
    }

    /// The three fixed shapes: `Str`, `[T]` and a closure. Each is a record of
    /// pointer-sized words, so one constructor covers all three.
    fn words(count: u32, fields: Vec<u32>, repr: Repr) -> Layout {
        let size = POINTER.saturating_mul(count);
        Layout { size, align: POINTER, stride: size, fields, repr }
    }
}

/// Which type constructors are recursive together: a function of the checker's
/// tables and of nothing else.
///
/// Separate from [`Layouts`] because the two have different lifetimes. A memo
/// table is per compilation *unit* — it holds `Rc<Layout>` handles a unit's
/// emission owns — but this is a walk of every constructor in the program
/// followed by a strongly-connected-components pass, and building one per unit
/// made a native build quadratic in the number of units
/// (`design/PERFORMANCE.md` §6.4's first finding). Build one and hand it to
/// every
/// [`Layouts::with_cycles`].
pub struct Cycles {
    /// Recursion group per type constructor, by `TyConId` index: the strongly
    /// connected components of "mentions, in a position that would be stored
    /// inline". Two constructors share a group exactly when laying either out
    /// inline would need the other, and that is what [`Layouts::boxes`] tests.
    groups: Vec<usize>,
    /// Whether a group is a genuine cycle — more than one constructor, or one
    /// that mentions itself — by group index.
    ///
    /// Without this, `Option<Option<T>>` would box its own payload: the outer
    /// and the inner are the same constructor, so they share a group, and
    /// "shares a group with the owner" would be true of a type that is
    /// strictly smaller than the one holding it. `Option`'s declared field is
    /// its parameter, so it mentions nothing and its group is not a cycle.
    recursive: Vec<bool>,
}

impl Cycles {
    pub fn new(tables: &Tables) -> Cycles {
        let mut edges: Vec<Vec<usize>> = Vec::with_capacity(tables.tycons.len());
        for con in &tables.tycons {
            let mut mentioned = Vec::new();
            for field in con.fields() {
                inline_cons(&field.ty, &mut mentioned);
            }
            for variant in con.variants() {
                for field in &variant.fields {
                    inline_cons(&field.ty, &mut mentioned);
                }
            }
            let mut out: Vec<usize> = mentioned.into_iter().map(TyConId::index).collect();
            out.sort_unstable();
            out.dedup();
            edges.push(out);
        }
        let mut groups = vec![0usize; tables.tycons.len()];
        let mut recursive = Vec::new();
        for (id, group) in super::strongly_connected(&edges).into_iter().enumerate() {
            // One constructor that mentions itself is a cycle; one that does
            // not is a group of one and nothing more.
            let cycle = group.len() > 1
                || group
                    .first()
                    .is_some_and(|n| edges.get(*n).is_some_and(|out| out.contains(n)));
            recursive.push(cycle);
            for node in group {
                if let Some(slot) = groups.get_mut(node) {
                    *slot = id;
                }
            }
        }
        Cycles { groups, recursive }
    }
}

/// The memo table, one per compilation.
pub struct Layouts<'a> {
    tables: &'a Tables,
    memo: Map<Ty, usize>,
    /// The memoised layouts, behind a handle. An enum's [`Repr::Enum`] carries
    /// one `Vec<u32>` per variant, so a `Layout` is O(variants) to copy and a
    /// caller that asks per instruction would be quadratic in the width of the
    /// widest enum it touches; [`Layouts::shared`] is what makes an answer
    /// O(1) to hand out.
    table: Vec<Rc<Layout>>,
    /// See [`Cycles`]. Shared, because it is the same answer for every table
    /// built over one program's `Tables` and it is not cheap to derive.
    ///
    /// `Arc` and not `Rc` for the one reason an immutable analysis is ever
    /// atomic: the stencil backend compiles its codegen units on a thread each
    /// (`stencil/mod.rs::emit_units`), and one `Cycles` is what those threads
    /// share. Nothing else in a `Layouts` crosses a thread — the memo, the
    /// layout handles and the descriptions are one worker's own — so this is
    /// the only handle that pays for it.
    cycles: Arc<Cycles>,
    depth: u32,
    descriptions: Map<Ty, Rc<str>>,
}

impl<'a> Layouts<'a> {
    pub fn new(tables: &'a Tables) -> Layouts<'a> {
        Layouts::with_cycles(tables, Arc::new(Cycles::new(tables)))
    }

    /// A fresh memo table over a recursion analysis somebody else already paid
    /// for. What a per-unit backend wants.
    pub fn with_cycles(tables: &'a Tables, cycles: Arc<Cycles>) -> Layouts<'a> {
        Layouts {
            tables,
            memo: Map::default(),
            table: Vec::new(),
            cycles,
            depth: 0,
            descriptions: Map::default(),
        }
    }

    /// The layout of a type. Memoised; infallible.
    pub fn of(&mut self, ty: Ty) -> Layout {
        let id = self.compute(&ty);
        self.at(id).clone()
    }

    /// The same answer, shared rather than copied.
    ///
    /// Every caller in a loop over instructions must use this: copying a
    /// `Layout` copies one `Vec<u32>` per variant, and an enum with a thousand
    /// variants makes a per-instruction copy a thousand allocations.
    pub fn shared(&mut self, ty: &Ty) -> Rc<Layout> {
        let id = self.compute(ty);
        Rc::clone(self.table.get(id).or_ice("every layout id was minted by compute on this table"))
    }

    /// Whether a value of this type occupies nothing, and so is dropped from
    /// every signature it appears in.
    ///
    /// The rule VALUE-MODEL.md §8 is about: every implementation `core/host`
    /// exports is an empty struct, a context is a record of its
    /// implementations, and a record of nothing is nothing. So on the platform
    /// context `ctx` is not a parameter at all, and `list.map(ctx, f)` is
    /// `map(xs, f)`. A `FixedBuffer` holding a budget is not zero-sized, and a
    /// context binding one is passed — the cost is exactly the state a
    /// capability holds.
    pub fn zero_sized(&mut self, ty: &Ty) -> bool {
        let id = self.compute(ty);
        self.at(id).is_zero_sized()
    }

    /// Whether a field of type `field`, inside a value of type `owner`, is
    /// stored behind a pointer rather than inline.
    ///
    /// This is where the indirection in a recursive type is introduced, and
    /// it is the whole of it. `enum Tree<T> { Leaf, Node(Tree<T>, T, Tree<T>) }`
    /// is "boxed by the runtime" in SPEC.md's own words (SPEC 5.4); the
    /// question this answers is *which* slot the box goes in, and the answer
    /// has to be a property of the type rather than of the order layouts were
    /// asked for, or `Tree` would have two layouts depending on which of a
    /// mutually recursive pair was requested first.
    ///
    /// So: a field is boxed exactly when the owner's constructor is in a
    /// recursion group that is a genuine cycle, and the field's type mentions
    /// a constructor of that group in a position that would be stored inline.
    /// In a cycle `A -> B -> A` both edges are boxed rather than one, which
    /// costs an indirection a cleverer rule would not, and buys an answer that
    /// does not depend on where the walk started.
    ///
    /// The "genuine cycle" half is what keeps `Option<Option<T>>` inline: the
    /// two are the same constructor and so share a group, but `Option`'s
    /// declared payload is its own parameter, so nothing about `Option`
    /// recurses and the inner one is strictly smaller than the outer.
    ///
    /// `[T]` and a closure are not inline positions, which is why
    /// `enum Rose { Node([Rose]) }` needs no box at all: a list is already a
    /// pointer and a length, and laying one out never asks for its element's
    /// layout.
    pub fn boxes(&self, owner: &Ty, field: &Ty) -> bool {
        let Some(con) = owner.head() else { return false };
        let group = self.group(con);
        if !self.cycles.recursive.get(group).copied().unwrap_or(false) {
            return false;
        }
        let mut mentioned = Vec::new();
        inline_cons(field, &mut mentioned);
        mentioned.into_iter().any(|c| self.group(c) == group)
    }

    fn group(&self, con: TyConId) -> usize {
        *self
            .cycles
            .groups
            .get(con.index())
            .or_ice("every TyConId was minted by add_tycon on this table")
    }

    fn at(&self, id: usize) -> &Layout {
        self.table
            .get(id)
            .map(Rc::as_ref)
            .or_ice("every layout id was minted by compute on this table")
    }

    fn compute(&mut self, ty: &Ty) -> usize {
        if let Some(&id) = self.memo.get(ty) {
            return id;
        }
        self.depth = self.depth.saturating_add(1);
        if self.depth > MAX_NESTING {
            crate::diagnostics::ice(
                "a type nested deeper than the layout table's fuse: a recursive type reached \
                 middle::layout without an indirection to cut it",
            );
        }
        let layout = self.build(ty);
        self.depth = self.depth.saturating_sub(1);
        let id = self.table.len();
        self.table.push(Rc::new(layout));
        self.memo.insert(ty.clone(), id);
        id
    }

    fn build(&mut self, ty: &Ty) -> Layout {
        let tables = self.tables;
        match ty {
            // `[T]` and a closure are the two places the element type is not
            // consulted, which is exactly why they are where recursion stops.
            Ty::Array(_) => Layout::words(2, vec![0, POINTER], Repr::List),
            Ty::Fn(_, _) => Layout::words(2, vec![0, POINTER], Repr::Closure),
            Ty::Tuple(elements) => {
                let elements = elements.clone();
                let (size, align, fields) = self.record(None, &elements);
                // A tuple of nothing but zero-sized members is itself nothing,
                // and says so the same way an empty struct does — one
                // predicate drops both from a signature.
                let repr = if size == 0 { Repr::Zero } else { Repr::Aggregate };
                Layout { size, align, stride: size, fields, repr }
            }
            Ty::Ctx(id) => {
                let bindings: Vec<Ty> =
                    tables.ctx_type(*id).bindings.iter().map(|(_, t)| t.clone()).collect();
                let (size, align, fields) = self.record(None, &bindings);
                let repr = if size == 0 { Repr::Zero } else { Repr::Aggregate };
                Layout { size, align, stride: size, fields, repr }
            }
            Ty::Con(id, args) => match &tables.tycon(*id).def {
                TyDef::Prim(p) => match scalar_of(*p) {
                    Some(s) => Layout::scalar(s),
                    // `Str`, and `Template`, which is `Str` (§3.3).
                    None => Layout::words(3, vec![0, POINTER, 16], Repr::Str),
                },
                TyDef::Struct { fields, .. } => {
                    let tys: Vec<Ty> =
                        fields.iter().map(|f| types::substitute(&f.ty, args, None)).collect();
                    let (size, align, offsets) = self.record(Some(ty), &tys);
                    let repr = if size == 0 { Repr::Zero } else { Repr::Aggregate };
                    Layout { size, align, stride: size, fields: offsets, repr }
                }
                TyDef::Enum { .. } => self.build_enum(ty, *id, args),
            },
            // `Self` is not a type a monomorphized program contains: a trait's
            // signature is abstract, and an `impl`'s is elaborated against the
            // head's own type (`semantics::resolve`'s `self_scope`), so nothing
            // downstream is left holding a `Ty::SelfTy` to substitute. Zero was
            // the answer here once, and it made a written `Self` in an `impl`
            // method's parameter a silently zero-sized value rather than a
            // reported mistake — the failure this arm exists to make loud.
            Ty::SelfTy => crate::diagnostics::ice(
                "`Self` reached middle::layout: a signature left an `impl` with its receiver \
                 type unresolved",
            ),
            // `()` is nothing (§1). `Var`, `Param` and `Error` are not types a
            // monomorphized program contains either — a body reaching the
            // native branch has been checked and instantiated, and a build that
            // produced an `Error` stopped before it — but a defined empty
            // layout rather than an internal error is what keeps `of` total for
            // a caller asking about a half-checked program.
            Ty::Unit | Ty::Var(_) | Ty::Param(_) | Ty::Error => Layout::zero(),
        }
    }

    /// Fields in declaration order, at natural alignment, C rules, size
    /// rounded up to the alignment. Nothing is reordered (VALUE-MODEL.md §5).
    ///
    /// `owner` is `None` for a tuple and for a context, neither of which is a
    /// nominal type and so neither of which can be the head of a recursion
    /// group. A tuple inside a recursive type is boxed whole, by the field
    /// that holds it.
    fn record(&mut self, owner: Option<&Ty>, fields: &[Ty]) -> (u32, u32, Vec<u32>) {
        let mut end = 0u32;
        let mut align = 1u32;
        let mut offsets = Vec::with_capacity(fields.len());
        for field in fields {
            let (size, field_align) = if owner.is_some_and(|o| self.boxes(o, field)) {
                (POINTER, POINTER)
            } else {
                let id = self.compute(field);
                let l = self.at(id);
                (l.size, l.align)
            };
            let at = align_up(end, field_align);
            offsets.push(at);
            end = at.saturating_add(size);
            align = align.max(field_align);
        }
        (align_up(end, align), align, offsets)
    }

    fn build_enum(&mut self, ty: &Ty, con: TyConId, args: &[Ty]) -> Layout {
        let tables = self.tables;

        // The `Option<T>` niche first, because it is the case with no tag.
        // `Option<Option<T>>` is excluded by name rather than by looking for a
        // free bit pattern: the inner `Option` has already spent the pointer,
        // and reusing it is precisely the collision `runtime.js:24-27` records
        // on the JavaScript side. Natively `Some(None)` and `None` differ, and
        // this is where that is decided.
        if tables.is_option(con) {
            if let Some(payload) = args.first().filter(|p| !tables.is_option_ty(p)) {
                let payload = payload.clone();
                if let Some(null_at) = self.niche(&payload) {
                    let id = self.compute(&payload);
                    let inner = self.at(id);
                    return Layout {
                        size: inner.size,
                        align: inner.align,
                        stride: inner.stride,
                        fields: Vec::new(),
                        repr: Repr::Enum {
                            repr: EnumRepr::Niche { null_at },
                            // `.Some`'s payload is the whole value; `.None`
                            // has none.
                            variants: vec![vec![0], Vec::new()],
                        },
                    };
                }
            }
        }

        let variants = tables.tycon(con).variants().to_vec();
        if variants.is_empty() {
            // No variant, so no value: uninhabited types occupy nothing.
            return Layout::zero();
        }

        // Each variant is laid out independently, from the start of the
        // payload area (§6).
        let mut payload_size = 0u32;
        let mut payload_align = 1u32;
        let mut relative: Vec<Vec<u32>> = Vec::with_capacity(variants.len());
        for variant in &variants {
            let tys: Vec<Ty> =
                variant.fields.iter().map(|f| types::substitute(&f.ty, args, None)).collect();
            let (size, align, offsets) = self.record(Some(ty), &tys);
            payload_size = payload_size.max(size);
            payload_align = payload_align.max(align);
            relative.push(offsets);
        }

        // A payload area of no bytes is the first niche: the value is the tag,
        // and equality on it is `a == b`. VALUE-MODEL.md §6 says it in fields —
        // "a payload-free enum" — and this says it in bytes, which is the same
        // set plus `Option<()>`, whose one field occupies nothing.
        //
        // The variant offsets are kept rather than emptied: a zero-sized field
        // is still a field, and a `derive Show` folding over them needs the
        // arity even where every offset is 0.
        let tag = tag_scalar(variants.len());
        if payload_size == 0 {
            return Layout {
                size: tag.size(),
                align: tag.align(),
                stride: tag.size(),
                fields: Vec::new(),
                repr: Repr::Enum { repr: EnumRepr::Bare { tag }, variants: relative },
            };
        }

        let payload = align_up(tag.size(), payload_align);
        let align = tag.align().max(payload_align);
        let size = align_up(payload.saturating_add(payload_size), align);
        let absolute = relative
            .into_iter()
            .map(|offsets| offsets.into_iter().map(|o| o.saturating_add(payload)).collect())
            .collect();
        Layout {
            size,
            align,
            stride: size,
            fields: Vec::new(),
            repr: Repr::Enum { repr: EnumRepr::Tagged { tag, payload }, variants: absolute },
        }
    }

    /// The byte offset of a pointer inside `ty` that is never null, if there
    /// is one — the invariant the `Option<T>` niche spends.
    ///
    /// Two shapes carry one directly: a `Str`'s `ptr` (non-null even for the
    /// empty string, which points at a static) and a closure's `code`. `base`
    /// and `env` are nullable and so are not candidates. Structs and tuples
    /// are searched in declaration order, lowest offset wins, and a boxed
    /// field — the indirection in a recursive type — is a candidate too, which
    /// is what makes `Option<Box-shaped struct>` free.
    ///
    /// **A `[T]`'s `ptr` is not one, and the reason is a fact about the
    /// runtime rather than about the model.** An empty list has no payload to
    /// point at, and every producer of one says so with a null word:
    /// `list.empty` is two immediates in both backends, and
    /// `buri_rt_list_new(0, _)` answers a null block. So a `[T]` niche made
    /// `.Some(xs)` indistinguishable from `.None` whenever `xs` was empty —
    /// `Option<[Int]>` over `list.empty()` answered `.None`, and so did
    /// `queue.pop` at the moment it emptied a side, because a `Queue<T>` is
    /// two lists and the tuple it returns is niched on the first of them.
    /// `Str` is the shape that *does* hold: `BuriStr::empty` points at a
    /// one-byte static for exactly this reason, which is what the empty list
    /// has no counterpart of.
    ///
    /// An enum is not searched. Which of its pointers exist depends on the
    /// tag, so none of them is unconditionally there; general niche discovery
    /// is deferred (§6).
    fn niche(&mut self, ty: &Ty) -> Option<u32> {
        let tables = self.tables;
        match ty {
            Ty::Array(_) => None,
            Ty::Fn(_, _) => Some(offset_of(CLOSURE_CODE)),
            Ty::Tuple(elements) => {
                let elements = elements.clone();
                self.niche_in(ty, &elements)
            }
            Ty::Ctx(id) => {
                let bindings: Vec<Ty> =
                    tables.ctx_type(*id).bindings.iter().map(|(_, t)| t.clone()).collect();
                self.niche_in(ty, &bindings)
            }
            Ty::Con(id, args) => match &tables.tycon(*id).def {
                TyDef::Prim(Prim::Str | Prim::Template) => Some(offset_of(STR_PTR)),
                TyDef::Prim(_) | TyDef::Enum { .. } => None,
                TyDef::Struct { fields, .. } => {
                    let tys: Vec<Ty> =
                        fields.iter().map(|f| types::substitute(&f.ty, args, None)).collect();
                    self.niche_in(ty, &tys)
                }
            },
            Ty::Unit | Ty::Var(_) | Ty::Param(_) | Ty::SelfTy | Ty::Error => None,
        }
    }

    fn niche_in(&mut self, owner: &Ty, fields: &[Ty]) -> Option<u32> {
        let id = self.compute(owner);
        let offsets = self.at(id).fields.clone();
        for (field, offset) in fields.iter().zip(offsets) {
            if self.boxes(owner, field) {
                return Some(offset);
            }
            if let Some(inner) = self.niche(field) {
                return Some(offset.saturating_add(inner));
            }
        }
        None
    }

    // -----------------------------------------------------------------------
    // The `Alloc` cost model (MEMORY.md §7.1)
    // -----------------------------------------------------------------------
    //
    // A *defined* charge, computed from the types, so that both backends and
    // both platforms charge the same number for the same program and a test
    // asserting one is not flaky. The rows that are zero are zero because the
    // language says so: a `Str` view has no `Alloc` bound (`str.buri:26-45`),
    // and SPEC 10.5 says fixed-size construction never requires `Alloc`.
    //
    // **Nothing in the compiler calls these, and that is not a reason to
    // delete them.** MEMORY.md §7.1 names the table three times over — here,
    // in `core/effect`'s source above the `Alloc` declaration, and in
    // `core/alloc`'s `strBytes`, `listBytes` and `closureBytes` — and calls a
    // change to any row a breaking change to observable behaviour. The
    // charge a running program accounts for is `core/alloc`'s spelling; this
    // one is the model as the layout table computes it, and the tests below
    // are what would notice the two spellings parting company.

    /// `16 + n * stride(T)` for a `[T]` of `n` elements.
    pub fn charge_list(&mut self, element: &Ty, n: u64) -> u64 {
        let id = self.compute(element);
        let stride = u64::from(self.at(id).stride);
        u64::from(HEADER_BYTES).saturating_add(n.saturating_mul(stride))
    }

    /// `16 + size(record(F))` for a closure environment of captures `F`.
    /// The environment is an ordinary record of the captured locals (§7), so
    /// it is laid out by §5 like any other.
    pub fn charge_closure_env(&mut self, captures: &[Ty]) -> u64 {
        let (size, _, _) = self.record(None, captures);
        u64::from(HEADER_BYTES).saturating_add(u64::from(size))
    }

    // -----------------------------------------------------------------------
    // The debug renderer
    // -----------------------------------------------------------------------

    /// One type's layout as text, so a change to the model is a visible diff
    /// rather than a number that moved in a backend.
    ///
    /// A `*` before an offset means the slot holds a pointer to the value
    /// rather than the value.
    pub fn describe(&mut self, ty: &Ty) -> String {
        let mut out = String::new();
        self.write_description(&mut out, ty);
        out
    }

    /// The same text, memoised and shared.
    ///
    /// A backend that identifies a per-type helper by its layout description
    /// asks this once per reference operation, and the description of an enum
    /// is a line per variant over a cloned [`Layout`] — so building it each
    /// time is quadratic in the width of the widest enum in the program.
    pub fn description(&mut self, ty: &Ty) -> Rc<str> {
        if let Some(known) = self.descriptions.get(ty) {
            return Rc::clone(known);
        }
        let text: Rc<str> = Rc::from(self.describe(ty).as_str());
        self.descriptions.insert(ty.clone(), Rc::clone(&text));
        text
    }

    /// Several types, one block each, newline separated and with no trailing
    /// newline — the shape a golden test compares.
    #[cfg(test)]
    fn describe_all(&mut self, tys: &[Ty]) -> String {
        let mut out = String::new();
        for (i, ty) in tys.iter().enumerate() {
            if i > 0 {
                out.push('\n');
            }
            self.write_description(&mut out, ty);
        }
        out
    }

    fn write_description(&mut self, out: &mut String, ty: &Ty) {
        let layout = self.of(ty.clone());
        let name = self.name(ty);
        // Writing to a `String` cannot fail; the result is discarded rather
        // than unwrapped so that no path here can panic.
        let _ = writeln!(
            out,
            "{name}: size {}, align {}, stride {}",
            layout.size, layout.align, layout.stride
        );
        match &layout.repr {
            Repr::Zero => out.push_str("  zero-sized\n"),
            Repr::Scalar(s) => {
                let _ = writeln!(out, "  scalar {}", s.name());
            }
            Repr::Str | Repr::List | Repr::Closure | Repr::Aggregate => {
                let kind = match &layout.repr {
                    Repr::Str => "str",
                    Repr::List => "list",
                    Repr::Closure => "closure",
                    _ => "aggregate",
                };
                let names = self.field_names(ty);
                let slots = self.slots(ty);
                let mut body = String::new();
                for (i, offset) in layout.fields.iter().enumerate() {
                    if i > 0 {
                        body.push_str(", ");
                    }
                    let label = names.get(i).cloned().unwrap_or_else(|| i.to_string());
                    let boxed = if slots.get(i).copied().unwrap_or(false) { "*" } else { "" };
                    let _ = write!(body, "{label} {boxed}@{offset}");
                }
                let _ = writeln!(out, "  {kind} {{ {body} }}");
            }
            Repr::Enum { repr, variants } => {
                match repr {
                    EnumRepr::Bare { tag } => {
                        let _ = writeln!(out, "  enum bare tag {}", tag.name());
                    }
                    EnumRepr::Niche { null_at } => {
                        let _ = writeln!(out, "  enum niche, .None is a null pointer @{null_at}");
                    }
                    EnumRepr::Tagged { tag, payload } => {
                        let _ = writeln!(out, "  enum tag {} @0, payload @{payload}", tag.name());
                    }
                }
                let names = self.variant_names(ty);
                let boxed = self.variant_slots(ty);
                for (i, offsets) in variants.iter().enumerate() {
                    let label = names.get(i).cloned().unwrap_or_else(|| i.to_string());
                    let empty: Vec<bool> = Vec::new();
                    let slots = boxed.get(i).unwrap_or(&empty);
                    let mut body = String::new();
                    for (f, offset) in offsets.iter().enumerate() {
                        if f > 0 {
                            body.push_str(", ");
                        }
                        let star = if slots.get(f).copied().unwrap_or(false) { "*" } else { "" };
                        let _ = write!(body, "{star}@{offset}");
                    }
                    if offsets.is_empty() {
                        let _ = writeln!(out, "    .{label}");
                    } else {
                        let _ = writeln!(out, "    .{label}({body})");
                    }
                }
            }
        }
        // The block ends with the last line rather than with a blank one.
        out.truncate(out.trim_end_matches('\n').len());
    }

    /// How a program would write the type. `Ty::Ctx` has no spelling — it is
    /// never written down (SPEC 11.3) — so it is named by what it binds, which
    /// is what a reader of a layout diff needs anyway.
    fn name(&self, ty: &Ty) -> String {
        match ty {
            Ty::Ctx(id) => {
                let names: Vec<&str> = self
                    .tables
                    .ctx_type(*id)
                    .bindings
                    .iter()
                    .map(|(t, _)| self.tables.trait_(*t).name.as_str())
                    .collect();
                format!("context {{ {} }}", names.join(", "))
            }
            _ => types::show(self.tables, None, &[], ty),
        }
    }

    fn field_names(&self, ty: &Ty) -> Vec<String> {
        let tables = self.tables;
        match ty {
            Ty::Array(_) => vec!["ptr".into(), "len".into()],
            Ty::Fn(_, _) => vec!["code".into(), "env".into()],
            Ty::Tuple(elements) => (0..elements.len()).map(|i| i.to_string()).collect(),
            Ty::Ctx(id) => tables
                .ctx_type(*id)
                .bindings
                .iter()
                .map(|(t, _)| tables.trait_(*t).name.clone())
                .collect(),
            Ty::Con(id, _) => match &tables.tycon(*id).def {
                TyDef::Prim(Prim::Str | Prim::Template) => {
                    vec!["base".into(), "ptr".into(), "len".into()]
                }
                TyDef::Struct { fields, record } => fields
                    .iter()
                    .enumerate()
                    .map(|(i, f)| if *record { f.name.clone() } else { i.to_string() })
                    .collect(),
                TyDef::Prim(_) | TyDef::Enum { .. } => Vec::new(),
            },
            _ => Vec::new(),
        }
    }

    /// Which of a value's own fields are behind a pointer.
    fn slots(&self, ty: &Ty) -> Vec<bool> {
        let tables = self.tables;
        match ty {
            Ty::Con(id, args) => match &tables.tycon(*id).def {
                TyDef::Struct { fields, .. } => fields
                    .iter()
                    .map(|f| self.boxes(ty, &types::substitute(&f.ty, args, None)))
                    .collect(),
                _ => Vec::new(),
            },
            _ => Vec::new(),
        }
    }

    fn variant_names(&self, ty: &Ty) -> Vec<String> {
        match ty.head() {
            Some(con) => self.tables.tycon(con).variants().iter().map(|v| v.name.clone()).collect(),
            None => Vec::new(),
        }
    }

    fn variant_slots(&self, ty: &Ty) -> Vec<Vec<bool>> {
        let Ty::Con(id, args) = ty else { return Vec::new() };
        self.tables
            .tycon(*id)
            .variants()
            .iter()
            .map(|v| {
                v.fields
                    .iter()
                    .map(|f| self.boxes(ty, &types::substitute(&f.ty, args, None)))
                    .collect()
            })
            .collect()
    }
}

// ---------------------------------------------------------------------------
// The small arithmetic
// ---------------------------------------------------------------------------

/// `16 + n` for a `Str` of `n` UTF-8 bytes (MEMORY.md §7.1).
pub fn charge_str(bytes: u64) -> u64 {
    u64::from(HEADER_BYTES).saturating_add(bytes)
}

/// `allocate(ctx, n)` charges exactly `n`: the program asked for bytes and the
/// accounting is of bytes.
pub fn charge_allocate(n: u64) -> u64 {
    n
}

/// A `Str` view — `slice`, `trim`, `splitOnce` — charges nothing, because the
/// language says so: none of them carries an `Alloc` bound. The same zero is
/// the charge for a fixed-size construction, which SPEC 10.5 says never
/// requires `Alloc` even where the implementation allocates.
pub const CHARGE_VIEW: u64 = 0;

/// `offset` rounded up to `align`, which is a power of two and at least 1.
///
/// Saturating rather than wrapping: a type whose layout overflows a `u32` does
/// not exist, and if one ever did, a size that stops growing is a build that
/// fails somewhere legible rather than a struct that wraps to a small one.
fn align_up(offset: u32, align: u32) -> u32 {
    match align.checked_sub(1) {
        Some(mask) => offset.saturating_add(mask) & !mask,
        None => offset,
    }
}

/// The byte offset of a word-sized field, by index.
fn offset_of(index: usize) -> u32 {
    POINTER.saturating_mul(u32::try_from(index).unwrap_or(0))
}

/// The smallest of `i8`/`i16`/`i32` that holds a discriminant for `count`
/// variants (VALUE-MODEL.md §6). Tags are `0..count`, so 256 variants fit in a
/// byte.
fn tag_scalar(count: usize) -> Scalar {
    if count <= 256 {
        Scalar::I8
    } else if count <= 65536 {
        Scalar::I16
    } else {
        Scalar::I32
    }
}

fn scalar_of(p: Prim) -> Option<Scalar> {
    Some(match p {
        Prim::Bool => Scalar::Bool,
        Prim::I8 | Prim::U8 => Scalar::I8,
        Prim::I16 | Prim::U16 => Scalar::I16,
        // `Char` is a Unicode scalar value, not a code unit (§1).
        Prim::I32 | Prim::U32 | Prim::Char => Scalar::I32,
        Prim::I64 | Prim::U64 => Scalar::I64,
        Prim::I128 | Prim::U128 => Scalar::I128,
        Prim::F32 => Scalar::F32,
        Prim::F64 => Scalar::F64,
        Prim::Str | Prim::Template => return None,
    })
}

/// Every type constructor `ty` mentions in a position that would be stored
/// *inline*.
///
/// A generic argument counts, because a `Pair<Rose>` stores its `Rose`s
/// inline. `[T]` and `fn(..) => T` do not, because neither consults `T`'s
/// layout — which is exactly why they are the two places a recursive type
/// terminates without a box.
fn inline_cons(ty: &Ty, out: &mut Vec<TyConId>) {
    match ty {
        Ty::Con(id, args) => {
            out.push(*id);
            for arg in args {
                inline_cons(arg, out);
            }
        }
        Ty::Tuple(elements) => {
            for element in elements {
                inline_cons(element, out);
            }
        }
        Ty::Array(_) | Ty::Fn(_, _) => {}
        Ty::Unit | Ty::Var(_) | Ty::Param(_) | Ty::Ctx(_) | Ty::SelfTy | Ty::Error => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compiler::semantics::types::{
        CtxType, CtxTypeId, FieldInfo, GenericInfo, ModuleId, TraitId, TraitInfo, TyCon,
        VariantInfo,
    };
    use crate::diagnostics::Span;

    // -----------------------------------------------------------------------
    // Building tables by hand
    // -----------------------------------------------------------------------
    //
    // A `Tables` with the primitives registered and whatever declarations a
    // test needs. Building one by hand rather than compiling Buri source keeps
    // these tests about the layout rules: a shape that no program can spell —
    // an enum with 300 variants, a struct that recurses through a generic
    // wrapper — is one line here.

    fn tables() -> Tables {
        let mut t = Tables::default();
        for p in Prim::all() {
            let id = t.add_tycon(TyCon {
                name: p.name().to_string(),
                module: ModuleId(0),
                generics: Vec::new(),
                def: TyDef::Prim(*p),
                exported: true,
                span: Span::NONE,
            });
            t.register_prim(*p, id);
        }
        t
    }

    /// A primitive by name, without borrowing the tables.
    ///
    /// [`tables`] registers `Prim::all()` into an empty table in order, so the
    /// id is the position — which is what lets a declaration mention `Int`
    /// while the table it is being added to is mutably borrowed.
    /// `primitive_ids_are_where_this_says_they_are` is the check that keeps
    /// the two in step.
    fn p(prim: Prim) -> Ty {
        let index = Prim::all().iter().position(|x| *x == prim).unwrap_or(0);
        Ty::Con(TyConId(index as u32), Vec::new())
    }

    fn generics(names: &[&str]) -> Vec<GenericInfo> {
        names
            .iter()
            .map(|n| GenericInfo { name: (*n).into(), bounds: Vec::new(), span: Span::NONE })
            .collect()
    }

    fn field(name: &str, ty: &Ty) -> FieldInfo {
        FieldInfo { name: name.into(), ty: ty.clone(), exported: true, span: Span::NONE }
    }

    fn fields(named: &[(&str, Ty)]) -> Vec<FieldInfo> {
        named.iter().map(|(n, t)| field(n, t)).collect()
    }

    /// A tuple-like variant: fields named by position, the way the checker
    /// names them.
    fn variant(name: &str, tys: &[Ty]) -> VariantInfo {
        VariantInfo {
            name: name.into(),
            fields: tys.iter().enumerate().map(|(i, t)| field(&i.to_string(), t)).collect(),
            record: false,
            exported: true,
            span: Span::NONE,
        }
    }

    fn add_struct(
        t: &mut Tables,
        name: &str,
        generic_names: &[&str],
        named: &[(&str, Ty)],
    ) -> TyConId {
        t.add_tycon(TyCon {
            name: name.into(),
            module: ModuleId(0),
            generics: generics(generic_names),
            def: TyDef::Struct { fields: fields(named), record: true },
            exported: true,
            span: Span::NONE,
        })
    }

    fn add_enum(
        t: &mut Tables,
        name: &str,
        generic_names: &[&str],
        variants: Vec<VariantInfo>,
    ) -> TyConId {
        t.add_tycon(TyCon {
            name: name.into(),
            module: ModuleId(0),
            generics: generics(generic_names),
            def: TyDef::Enum { variants },
            exported: true,
            span: Span::NONE,
        })
    }

    /// A type constructor declared before its own definition, which is how a
    /// recursive one has to be built here: the definition mentions the id.
    fn declare(t: &mut Tables, name: &str, generic_names: &[&str]) -> TyConId {
        add_enum(t, name, generic_names, Vec::new())
    }

    fn define_enum(t: &mut Tables, con: TyConId, variants: Vec<VariantInfo>) {
        t.tycon_mut(con).def = TyDef::Enum { variants };
    }

    /// `Option<T>`, in exactly the shape `Tables::is_option` recognises.
    fn add_option(t: &mut Tables) -> TyConId {
        add_enum(t, "Option", &["T"], vec![variant("Some", &[Ty::Param(0)]), variant("None", &[])])
    }

    fn con(id: TyConId) -> Ty {
        Ty::Con(id, Vec::new())
    }

    fn at(id: TyConId, args: &[Ty]) -> Ty {
        Ty::Con(id, args.to_vec())
    }

    fn add_trait(t: &mut Tables, name: &str) -> TraitId {
        t.add_trait(TraitInfo {
            name: name.into(),
            module: ModuleId(0),
            generics: Vec::new(),
            methods: Vec::new(),
            is_effect: true,
            exported: true,
            span: Span::NONE,
        })
    }

    fn add_ctx(t: &mut Tables, bindings: Vec<(TraitId, Ty)>) -> CtxTypeId {
        t.add_ctx_type(CtxType { bindings })
    }

    #[test]
    fn primitive_ids_are_where_this_says_they_are() {
        let t = tables();
        for prim in Prim::all() {
            assert_eq!(p(*prim), t.prim(*prim), "{}", prim.name());
        }
    }

    // -----------------------------------------------------------------------
    // Scalars
    // -----------------------------------------------------------------------

    #[test]
    fn every_primitive_has_the_width_the_model_gives_it() {
        let t = tables();
        let mut l = Layouts::new(&t);
        let expected = [
            (Prim::Bool, 1, 1, Scalar::Bool),
            (Prim::I8, 1, 1, Scalar::I8),
            (Prim::U8, 1, 1, Scalar::I8),
            (Prim::I16, 2, 2, Scalar::I16),
            (Prim::U16, 2, 2, Scalar::I16),
            (Prim::I32, 4, 4, Scalar::I32),
            (Prim::U32, 4, 4, Scalar::I32),
            (Prim::I64, 8, 8, Scalar::I64),
            (Prim::U64, 8, 8, Scalar::I64),
            (Prim::I128, 16, 16, Scalar::I128),
            (Prim::U128, 16, 16, Scalar::I128),
            (Prim::F32, 4, 4, Scalar::F32),
            (Prim::F64, 8, 8, Scalar::F64),
            // A Unicode scalar value, not a code unit.
            (Prim::Char, 4, 4, Scalar::I32),
        ];
        for (prim, size, align, scalar) in expected {
            let layout = l.of(p(prim));
            assert_eq!(layout.size, size, "{}", prim.name());
            assert_eq!(layout.align, align, "{}", prim.name());
            assert_eq!(layout.stride, size, "{}", prim.name());
            assert_eq!(layout.repr, Repr::Scalar(scalar), "{}", prim.name());
        }
    }

    /// Unsigned is the same bits and different operations, so it is the same
    /// layout.
    #[test]
    fn signedness_is_not_a_layout_question() {
        let t = tables();
        let mut l = Layouts::new(&t);
        for (a, b) in [
            (Prim::I8, Prim::U8),
            (Prim::I16, Prim::U16),
            (Prim::I32, Prim::U32),
            (Prim::I64, Prim::U64),
            (Prim::I128, Prim::U128),
        ] {
            assert_eq!(l.of(p(a)), l.of(p(b)));
        }
    }

    #[test]
    fn unit_is_nothing() {
        let t = tables();
        let mut l = Layouts::new(&t);
        let layout = l.of(Ty::Unit);
        assert!(layout.is_zero_sized());
        assert_eq!(layout.align, 1);
        assert_eq!(layout.repr, Repr::Zero);
    }

    // -----------------------------------------------------------------------
    // Str and [T]
    // -----------------------------------------------------------------------

    #[test]
    fn a_string_is_a_base_a_pointer_and_a_length() {
        let t = tables();
        let mut l = Layouts::new(&t);
        let layout = l.of(p(Prim::Str));
        assert_eq!(layout.size, 24);
        assert_eq!(layout.align, 8);
        assert_eq!(layout.fields, vec![0, 8, 16]);
        assert_eq!(layout.repr, Repr::Str);
        assert_eq!(layout.field(STR_BASE), 0);
        assert_eq!(layout.field(STR_PTR), 8);
        assert_eq!(layout.field(STR_LEN), 16);
    }

    /// `Template` is `Str`; there is no `Template` value at run time on either
    /// backend (§3.3).
    #[test]
    fn a_template_is_a_string() {
        let t = tables();
        let mut l = Layouts::new(&t);
        assert_eq!(l.of(p(Prim::Template)), l.of(p(Prim::Str)));
    }

    #[test]
    fn the_ascii_flag_is_the_top_bit_of_the_length() {
        assert_eq!(STR_ASCII_FLAG, 1 << 63);
        assert_eq!(STR_LEN_MASK, u64::MAX >> 1);
        // Strings are capped at 2^63 - 1 bytes, which is not a cap.
        assert_eq!(STR_LEN_MASK, (1u64 << 63) - 1);
        assert_eq!(STR_ASCII_FLAG & STR_LEN_MASK, 0);
    }

    /// The reserved multi-threaded bit is the top bit of `cap`, and masking is
    /// the identity on every capacity a block can actually have.
    #[test]
    fn the_shared_flag_is_the_top_bit_of_the_capacity() {
        assert_eq!(CAP_SHARED_FLAG, 1 << 63);
        assert_eq!(CAP_ARENA_FLAG, 1 << 62);
        assert_eq!(CAP_MASK, u64::MAX >> 2);
        assert_eq!(CAP_SHARED_FLAG & CAP_MASK, 0);
        assert_eq!(CAP_ARENA_FLAG & CAP_MASK, 0);
        assert_eq!(CAP_SHARED_FLAG & CAP_ARENA_FLAG, 0);
        // The three boundaries a reader has to survive: an empty block, the
        // largest capacity the low 63 bits can spell, and the same capacity
        // with the flag on.
        for cap in [0u64, 1, GROWTH_FLOOR, CAP_MASK] {
            assert_eq!(cap & CAP_MASK, cap);
            assert_eq!((cap | CAP_SHARED_FLAG) & CAP_MASK, cap);
            assert_eq!((cap | CAP_ARENA_FLAG) & CAP_MASK, cap);
            assert_eq!((cap | CAP_SHARED_FLAG | CAP_ARENA_FLAG) & CAP_MASK, cap);
        }
        // It is a different bit from the one `Str::len` already spends, but the
        // same bit position — the precedent, not a collision: the two words are
        // eight bytes apart.
        assert_eq!(CAP_SHARED_FLAG, STR_ASCII_FLAG);
    }

    #[test]
    fn a_list_is_a_pointer_and_a_length_whatever_it_holds() {
        let t = tables();
        let mut l = Layouts::new(&t);
        for element in [Ty::Unit, p(Prim::I8), p(Prim::Str), p(Prim::I128)] {
            let layout = l.of(Ty::Array(Box::new(element)));
            assert_eq!(layout.size, 16);
            assert_eq!(layout.align, 8);
            assert_eq!(layout.fields, vec![0, 8]);
            assert_eq!(layout.field(LIST_PTR), 0);
            assert_eq!(layout.field(LIST_LEN), 8);
            assert_eq!(layout.repr, Repr::List);
        }
    }

    #[test]
    fn the_heap_header_is_sixteen_bytes_behind_the_payload() {
        assert_eq!(HEADER_BYTES, 16);
        assert_eq!(HEADER_RC_OFFSET, -16);
        assert_eq!(HEADER_CAP_OFFSET, -8);
        assert_eq!(IMMORTAL, u64::MAX);
        // Which is what keeps every payload 16-byte aligned, for `core/simd`.
        assert_eq!(HEADER_BYTES % 16, 0);
    }

    // -----------------------------------------------------------------------
    // Tuples and structs
    // -----------------------------------------------------------------------

    #[test]
    fn fields_are_in_declaration_order_with_c_padding() {
        let mut t = tables();
        let s = add_struct(
            &mut t,
            "Padded",
            &[],
            &[("a", p(Prim::Bool)), ("b", p(Prim::I64)), ("c", p(Prim::Bool))],
        );
        let mut l = Layouts::new(&t);
        let layout = l.of(con(s));
        // Nothing is reordered, so the two booleans do not share a word.
        assert_eq!(layout.fields, vec![0, 8, 16]);
        assert_eq!(layout.size, 24);
        assert_eq!(layout.align, 8);
        assert_eq!(layout.stride, 24);
    }

    #[test]
    fn a_nested_struct_pads_to_its_own_alignment() {
        let mut t = tables();
        let inner = add_struct(&mut t, "Inner", &[], &[("a", p(Prim::I16)), ("b", p(Prim::I8))]);
        let outer = add_struct(
            &mut t,
            "Outer",
            &[],
            &[("x", p(Prim::Bool)), ("y", con(inner)), ("z", p(Prim::I32))],
        );
        let mut l = Layouts::new(&t);
        let i = l.of(con(inner));
        assert_eq!(i.fields, vec![0, 2]);
        assert_eq!((i.size, i.align), (4, 2));
        let o = l.of(con(outer));
        assert_eq!(o.fields, vec![0, 2, 8]);
        assert_eq!((o.size, o.align), (12, 4));
    }

    #[test]
    fn a_struct_with_no_fields_is_zero_sized() {
        let mut t = tables();
        let host = add_struct(&mut t, "HostFs", &[], &[]);
        let mut l = Layouts::new(&t);
        assert!(l.zero_sized(&con(host)));
        assert_eq!(l.of(con(host)).repr, Repr::Zero);
    }

    #[test]
    fn a_zero_sized_field_takes_no_room_and_moves_nothing() {
        let mut t = tables();
        let empty = add_struct(&mut t, "Empty", &[], &[]);
        let s = add_struct(
            &mut t,
            "Holder",
            &[],
            &[("a", p(Prim::I32)), ("e", con(empty)), ("b", p(Prim::I32))],
        );
        let mut l = Layouts::new(&t);
        let layout = l.of(con(s));
        assert_eq!(layout.fields, vec![0, 4, 4]);
        assert_eq!(layout.size, 8);
    }

    #[test]
    fn a_tuple_is_a_struct_without_a_name() {
        let t = tables();
        let mut l = Layouts::new(&t);
        let layout = l.of(Ty::Tuple(vec![p(Prim::Str), p(Prim::Bool)]));
        assert_eq!(layout.fields, vec![0, 24]);
        assert_eq!((layout.size, layout.align), (32, 8));
    }

    /// Whatever it was built out of, a value of no bytes is a value of no
    /// bytes: one predicate drops all of them from a signature.
    #[test]
    fn everything_empty_is_the_same_kind_of_empty() {
        let mut t = tables();
        let empty = add_struct(&mut t, "Empty", &[], &[]);
        let mut l = Layouts::new(&t);
        for ty in [Ty::Unit, Ty::Tuple(Vec::new()), Ty::Tuple(vec![Ty::Unit]), con(empty)] {
            let layout = l.of(ty.clone());
            assert!(layout.is_zero_sized(), "{ty:?}");
            assert_eq!(layout.repr, Repr::Zero, "{ty:?}");
            assert_eq!(layout.align, 1, "{ty:?}");
        }
    }

    /// A generic struct is laid out at its arguments, not at its parameters.
    #[test]
    fn a_generic_struct_is_laid_out_after_substitution() {
        let mut t = tables();
        let pair = add_struct(&mut t, "Pair", &["T"], &[("a", Ty::Param(0)), ("b", Ty::Param(0))]);
        let mut l = Layouts::new(&t);
        assert_eq!(l.of(at(pair, &[p(Prim::I8)])).size, 2);
        assert_eq!(l.of(at(pair, &[p(Prim::I64)])).size, 16);
        assert_eq!(l.of(at(pair, &[p(Prim::Str)])).size, 48);
    }

    // -----------------------------------------------------------------------
    // The four enum shapes
    // -----------------------------------------------------------------------

    #[test]
    fn a_payload_free_enum_is_a_bare_integer() {
        let mut t = tables();
        let order = add_enum(
            &mut t,
            "Order",
            &[],
            vec![variant("Less", &[]), variant("Equal", &[]), variant("Greater", &[])],
        );
        let mut l = Layouts::new(&t);
        let layout = l.of(con(order));
        assert_eq!((layout.size, layout.align, layout.stride), (1, 1, 1));
        match layout.repr {
            Repr::Enum { repr: EnumRepr::Bare { tag }, ref variants } => {
                assert_eq!(tag, Scalar::I8);
                assert_eq!(variants.len(), 3);
                assert!(variants.iter().all(Vec::is_empty));
            }
            other => panic!("expected a bare tag, got {other:?}"),
        }
    }

    #[test]
    fn a_tagged_enum_puts_the_payload_after_the_tag() {
        let mut t = tables();
        let shape = add_enum(
            &mut t,
            "Shape",
            &[],
            vec![
                variant("Empty", &[]),
                variant("Circle", &[p(Prim::F64)]),
                variant("Rect", &[p(Prim::F64), p(Prim::F64)]),
            ],
        );
        let mut l = Layouts::new(&t);
        let layout = l.of(con(shape));
        assert_eq!((layout.size, layout.align), (24, 8));
        match layout.repr {
            Repr::Enum { repr: EnumRepr::Tagged { tag, payload }, ref variants } => {
                assert_eq!(tag, Scalar::I8);
                assert_eq!(payload, 8);
                // Absolute offsets, and each variant is laid out on its own.
                assert_eq!(variants, &vec![vec![], vec![8], vec![8, 16]]);
            }
            other => panic!("expected a tag, got {other:?}"),
        }
        assert_eq!(layout.variant(2), &[8, 16]);
    }

    /// The payload area is the union at the widest alignment, and a narrow
    /// variant does not shrink it.
    #[test]
    fn the_payload_area_is_the_widest_variant() {
        let mut t = tables();
        let e = add_enum(
            &mut t,
            "Mixed",
            &[],
            vec![
                variant("Small", &[p(Prim::I8)]),
                variant("Big", &[p(Prim::Str)]),
                variant("Wide", &[p(Prim::I128)]),
            ],
        );
        let mut l = Layouts::new(&t);
        let layout = l.of(con(e));
        // `I128` aligns the payload to 16, and `Str` is the widest at 24.
        assert_eq!((layout.size, layout.align), (48, 16));
        assert_eq!(layout.variant(0), &[16]);
        assert_eq!(layout.variant(1), &[16]);
        assert_eq!(layout.variant(2), &[16]);
    }

    #[test]
    fn the_tag_widens_with_the_variant_count() {
        assert_eq!(tag_scalar(1), Scalar::I8);
        assert_eq!(tag_scalar(256), Scalar::I8);
        assert_eq!(tag_scalar(257), Scalar::I16);
        assert_eq!(tag_scalar(65536), Scalar::I16);
        assert_eq!(tag_scalar(65537), Scalar::I32);
    }

    #[test]
    fn a_wide_enum_pays_for_its_tag_in_a_wider_word() {
        let mut t = tables();
        let variants: Vec<VariantInfo> =
            (0..300).map(|i| variant(&format!("V{i}"), &[p(Prim::I64)])).collect();
        let wide = add_enum(&mut t, "Wide", &[], variants);
        let mut l = Layouts::new(&t);
        match l.of(con(wide)).repr {
            Repr::Enum { repr: EnumRepr::Tagged { tag, payload }, .. } => {
                assert_eq!(tag, Scalar::I16);
                assert_eq!(payload, 8);
            }
            other => panic!("expected a tag, got {other:?}"),
        }
    }

    #[test]
    fn an_enum_with_no_variants_is_nothing() {
        let mut t = tables();
        let void = add_enum(&mut t, "Void", &[], Vec::new());
        let mut l = Layouts::new(&t);
        assert!(l.zero_sized(&con(void)));
    }

    #[test]
    fn option_of_a_string_is_the_string_with_a_null_pointer() {
        let mut t = tables();
        let option = add_option(&mut t);
        let mut l = Layouts::new(&t);
        let layout = l.of(at(option, &[p(Prim::Str)]));
        // Exactly a `Str`: the niche costs nothing.
        assert_eq!((layout.size, layout.align, layout.stride), (24, 8, 24));
        match layout.repr {
            // `base` is null for a literal, so the niche has to be `ptr`.
            Repr::Enum { repr: EnumRepr::Niche { null_at }, ref variants } => {
                assert_eq!(null_at, 8);
                assert_eq!(variants, &vec![vec![0], vec![]]);
            }
            other => panic!("expected a niche, got {other:?}"),
        }
    }

    /// A list pointer is null when the list is empty, so it is a tag and not a
    /// niche: `.Some(list.empty())` and `.None` would otherwise be the same
    /// two words.
    #[test]
    fn option_of_a_list_is_tagged_because_an_empty_list_is_a_null_pointer() {
        let mut t = tables();
        let option = add_option(&mut t);
        let mut l = Layouts::new(&t);
        let layout = l.of(at(option, &[Ty::Array(Box::new(p(Prim::I64)))]));
        assert!(matches!(layout.repr, Repr::Enum { repr: EnumRepr::Tagged { .. }, .. }));
    }

    /// And a type that *holds* a list niches past it, on the next pointer that
    /// really is unconditional — which is what keeps `Option<(Str, [Int])>`
    /// free.
    #[test]
    fn a_niche_search_steps_over_a_list_and_takes_the_string(){
        let mut t = tables();
        let option = add_option(&mut t);
        let mut l = Layouts::new(&t);
        let pair = Ty::Tuple(vec![Ty::Array(Box::new(p(Prim::I64))), p(Prim::Str)]);
        let layout = l.of(at(option, &[pair]));
        match layout.repr {
            Repr::Enum { repr: EnumRepr::Niche { null_at }, .. } => {
                // 16 bytes of `[Int]`, then the `Str`'s `ptr` at its offset 8.
                assert_eq!(null_at, 24);
            }
            other => panic!("expected a niche, got {other:?}"),
        }
    }

    #[test]
    fn option_of_a_closure_niches_on_the_code_pointer() {
        let mut t = tables();
        let option = add_option(&mut t);
        let mut l = Layouts::new(&t);
        let f = Ty::Fn(vec![p(Prim::I64)], Box::new(p(Prim::Bool)));
        let layout = l.of(at(option, &[f]));
        assert_eq!(layout.size, 16);
        // `env` is null when nothing was captured, so it cannot be the niche.
        assert!(matches!(layout.repr, Repr::Enum { repr: EnumRepr::Niche { null_at: 0 }, .. }));
    }

    #[test]
    fn option_of_a_box_shaped_struct_niches_on_the_pointer_inside_it() {
        let mut t = tables();
        let option = add_option(&mut t);
        let boxy =
            add_struct(&mut t, "Boxy", &[], &[("n", p(Prim::I64)), ("s", p(Prim::Str))]);
        let mut l = Layouts::new(&t);
        let layout = l.of(at(option, &[con(boxy)]));
        // The struct is `{ i64 @0, Str @8 }`, so the `Str`'s `ptr` is at 16.
        assert_eq!(layout.size, 32);
        assert!(matches!(layout.repr, Repr::Enum { repr: EnumRepr::Niche { null_at: 16 }, .. }));
    }

    #[test]
    fn option_of_something_with_no_pointer_gets_a_tag() {
        let mut t = tables();
        let option = add_option(&mut t);
        let mut l = Layouts::new(&t);
        let layout = l.of(at(option, &[p(Prim::I64)]));
        assert_eq!((layout.size, layout.align), (16, 8));
        match layout.repr {
            Repr::Enum { repr: EnumRepr::Tagged { tag, payload }, ref variants } => {
                assert_eq!(tag, Scalar::I8);
                assert_eq!(payload, 8);
                assert_eq!(variants, &vec![vec![8], vec![]]);
            }
            other => panic!("expected a tag, got {other:?}"),
        }
    }

    /// `Option<()>` has a field, so it is not payload-*free*, but its payload
    /// area is empty, which is the same statement in bytes.
    #[test]
    fn option_of_unit_is_a_bare_tag() {
        let mut t = tables();
        let option = add_option(&mut t);
        let mut l = Layouts::new(&t);
        let layout = l.of(at(option, &[Ty::Unit]));
        assert_eq!(layout.size, 1);
        assert!(matches!(layout.repr, Repr::Enum { repr: EnumRepr::Bare { .. }, .. }));
        // The field is still a field, at an offset that costs nothing.
        assert_eq!(layout.variant(0), &[0]);
        assert!(layout.variant(1).is_empty());
    }

    /// Row 5 of VALUE-MODEL.md §12: `Some(None)` and `None` collide on
    /// JavaScript and do not collide natively.
    #[test]
    fn option_of_option_gets_a_real_tag() {
        let mut t = tables();
        let option = add_option(&mut t);
        let mut l = Layouts::new(&t);
        let inner = at(option, &[p(Prim::Str)]);
        let outer = at(option, std::slice::from_ref(&inner));
        // The inner one still gets its niche.
        assert!(matches!(l.of(inner).repr, Repr::Enum { repr: EnumRepr::Niche { .. }, .. }));
        let layout = l.of(outer);
        assert_eq!((layout.size, layout.align), (32, 8));
        match layout.repr {
            Repr::Enum { repr: EnumRepr::Tagged { tag, payload }, ref variants } => {
                assert_eq!(tag, Scalar::I8);
                assert_eq!(payload, 8);
                assert_eq!(variants, &vec![vec![8], vec![]]);
            }
            other => panic!("expected a tag, got {other:?}"),
        }
    }

    #[test]
    fn a_result_gets_a_tag_because_both_arms_carry_a_payload() {
        let mut t = tables();
        let result = add_enum(
            &mut t,
            "Result",
            &["T", "E"],
            vec![variant("Ok", &[Ty::Param(0)]), variant("Err", &[Ty::Param(1)])],
        );
        let mut l = Layouts::new(&t);
        let layout = l.of(at(result, &[Ty::Unit, p(Prim::Str)]));
        assert_eq!((layout.size, layout.align), (32, 8));
        assert!(matches!(
            layout.repr,
            Repr::Enum { repr: EnumRepr::Tagged { payload: 8, .. }, .. }
        ));
    }

    // -----------------------------------------------------------------------
    // Closures
    // -----------------------------------------------------------------------

    #[test]
    fn a_closure_is_a_code_pointer_and_an_environment() {
        let t = tables();
        let mut l = Layouts::new(&t);
        let layout = l.of(Ty::Fn(vec![p(Prim::I64)], Box::new(p(Prim::Str))));
        assert_eq!((layout.size, layout.align), (16, 8));
        assert_eq!(layout.fields, vec![0, 8]);
        assert_eq!(layout.field(CLOSURE_CODE), 0);
        assert_eq!(layout.field(CLOSURE_ENV), 8);
        assert_eq!(layout.repr, Repr::Closure);
    }

    /// A closure is the second place a type stops recursing: the environment
    /// is a separate record, so the signature never reaches the layout.
    #[test]
    fn a_closure_does_not_consult_its_signature_for_a_layout() {
        let t = tables();
        let mut l = Layouts::new(&t);
        let a = l.of(Ty::Fn(Vec::new(), Box::new(Ty::Unit)));
        let b = l.of(Ty::Fn(vec![p(Prim::I128); 8], Box::new(p(Prim::Str))));
        assert_eq!(a, b);
    }

    /// The environment is an ordinary record of the captured locals, so it is
    /// laid out by §5 and charged by MEMORY.md §7.1.
    #[test]
    fn a_closure_environment_is_an_ordinary_record() {
        let t = tables();
        let mut l = Layouts::new(&t);
        let env = Ty::Tuple(vec![p(Prim::Bool), p(Prim::Str)]);
        let layout = l.of(env);
        assert_eq!(layout.fields, vec![0, 8]);
        assert_eq!(layout.size, 32);
    }

    // -----------------------------------------------------------------------
    // Contexts
    // -----------------------------------------------------------------------

    #[test]
    fn a_context_of_zero_sized_implementations_is_zero_sized() {
        let mut t = tables();
        let alloc = add_trait(&mut t, "Alloc");
        let stdout = add_trait(&mut t, "Stdout");
        let host_alloc = add_struct(&mut t, "HostAlloc", &[], &[]);
        let host_stdout = add_struct(&mut t, "HostStdout", &[], &[]);
        let ctx = add_ctx(&mut t, vec![(alloc, con(host_alloc)), (stdout, con(host_stdout))]);
        let mut l = Layouts::new(&t);
        let layout = l.of(Ty::Ctx(ctx));
        assert!(layout.is_zero_sized());
        assert_eq!(layout.repr, Repr::Zero);
        // Which is what drops `ctx` from every signature in the program.
        assert!(l.zero_sized(&Ty::Ctx(ctx)));
    }

    #[test]
    fn a_context_that_holds_state_is_a_record_of_exactly_that() {
        let mut t = tables();
        let alloc = add_trait(&mut t, "Alloc");
        let stdout = add_trait(&mut t, "Stdout");
        // A `FixedBuffer` holds a budget and a total; `HostStdout` holds
        // nothing, and costs nothing even beside one that does.
        let buffer = add_struct(
            &mut t,
            "FixedBuffer",
            &[],
            &[("budget", p(Prim::I64)), ("used", p(Prim::I64))],
        );
        let host_stdout = add_struct(&mut t, "HostStdout", &[], &[]);
        let ctx = add_ctx(&mut t, vec![(alloc, con(buffer)), (stdout, con(host_stdout))]);
        let mut l = Layouts::new(&t);
        let layout = l.of(Ty::Ctx(ctx));
        assert_eq!((layout.size, layout.align), (16, 8));
        // One offset per binding, in binding order, including the one that
        // occupies nothing.
        assert_eq!(layout.fields, vec![0, 16]);
        assert!(!l.zero_sized(&Ty::Ctx(ctx)));
    }

    // -----------------------------------------------------------------------
    // Recursive types
    // -----------------------------------------------------------------------

    /// `enum Tree<T> { Leaf, Node(Tree<T>, T, Tree<T>) }` — SPEC.md:553's own
    /// example, "boxed by the runtime".
    #[test]
    fn a_recursive_enum_is_cut_by_a_pointer() {
        let mut t = tables();
        let tree = declare(&mut t, "Tree", &["T"]);
        let self_ty = at(tree, &[Ty::Param(0)]);
        define_enum(
            &mut t,
            tree,
            vec![
                variant("Leaf", &[]),
                variant("Node", &[self_ty.clone(), Ty::Param(0), self_ty]),
            ],
        );
        let mut l = Layouts::new(&t);
        let ty = at(tree, &[p(Prim::I64)]);
        let layout = l.of(ty.clone());
        // tag @0, then a pointer, the payload, and a pointer.
        assert_eq!((layout.size, layout.align), (32, 8));
        assert_eq!(layout.variant(1), &[8, 16, 24]);
        assert!(l.boxes(&ty, &ty));
        assert!(!l.boxes(&ty, &p(Prim::I64)));
    }

    /// The answer does not depend on which of a mutually recursive pair was
    /// asked for first, which is the property that makes a memo table safe.
    #[test]
    fn the_boxes_do_not_move_with_the_order_of_the_questions() {
        fn build() -> (Tables, TyConId, TyConId) {
            let mut t = tables();
            let a = declare(&mut t, "A", &[]);
            let b = declare(&mut t, "B", &[]);
            define_enum(&mut t, a, vec![variant("A0", &[]), variant("A1", &[con(b)])]);
            define_enum(&mut t, b, vec![variant("B0", &[]), variant("B1", &[con(a)])]);
            (t, a, b)
        }
        let (t1, a1, b1) = build();
        let mut first = Layouts::new(&t1);
        let a_first = (first.of(con(a1)), first.of(con(b1)));
        let (t2, a2, b2) = build();
        let mut second = Layouts::new(&t2);
        // The other order, on a fresh table.
        let b_first = (second.of(con(b2)), second.of(con(a2)));
        assert_eq!(a_first.0, b_first.1);
        assert_eq!(a_first.1, b_first.0);
    }

    /// `enum Json { .., Array([Json]), Object([(Str, Json)]) }` —
    /// `core/json`'s own shape. A list is already an indirection, so nothing
    /// is boxed and the recursion still terminates.
    #[test]
    fn recursion_through_a_list_needs_no_box() {
        let mut t = tables();
        let json = declare(&mut t, "Json", &[]);
        let self_ty = con(json);
        define_enum(
            &mut t,
            json,
            vec![
                variant("Null", &[]),
                variant("Bool", &[p(Prim::Bool)]),
                variant("Num", &[p(Prim::F64)]),
                variant("Str", &[p(Prim::Str)]),
                variant("Array", &[Ty::Array(Box::new(self_ty.clone()))]),
                variant(
                    "Object",
                    &[Ty::Array(Box::new(Ty::Tuple(vec![p(Prim::Str), self_ty.clone()])))],
                ),
            ],
        );
        let mut l = Layouts::new(&t);
        let layout = l.of(con(json));
        // The widest payload is the 24-byte `Str`, at 8.
        assert_eq!((layout.size, layout.align), (32, 8));
        assert!(!l.boxes(&con(json), &Ty::Array(Box::new(con(json)))));
        // And the element of that list is an ordinary `Json`, inline inside
        // the array's one allocation.
        assert_eq!(l.of(Ty::Tuple(vec![p(Prim::Str), con(json)])).fields, vec![0, 24]);
    }

    #[test]
    fn mutual_recursion_boxes_both_edges() {
        let mut t = tables();
        let a = add_struct(&mut t, "A", &[], &[]);
        let b = add_enum(&mut t, "B", &[], vec![variant("Stop", &[]), variant("Go", &[con(a)])]);
        t.tycon_mut(a).def =
            TyDef::Struct { fields: fields(&[("n", p(Prim::I64)), ("b", con(b))]), record: true };
        let mut l = Layouts::new(&t);
        let la = l.of(con(a));
        assert_eq!(la.fields, vec![0, 8]);
        assert_eq!((la.size, la.align), (16, 8));
        assert!(l.boxes(&con(a), &con(b)));
        assert!(l.boxes(&con(b), &con(a)));
        // And `B` is a tag plus the pointer that cuts the other edge.
        let lb = l.of(con(b));
        assert_eq!((lb.size, lb.align), (16, 8));
        assert_eq!(lb.variant(1), &[8]);
    }

    /// A recursive type reached through a generic wrapper: the wrapper is not
    /// in the group, so the box goes on the field that names the cycle.
    #[test]
    fn recursion_through_a_generic_wrapper_terminates() {
        let mut t = tables();
        let pair = add_struct(&mut t, "Pair", &["T"], &[("a", Ty::Param(0)), ("b", Ty::Param(0))]);
        let node = declare(&mut t, "Node", &[]);
        define_enum(
            &mut t,
            node,
            vec![variant("Tip", &[]), variant("Fork", &[at(pair, &[con(node)])])],
        );
        let mut l = Layouts::new(&t);
        let layout = l.of(con(node));
        // tag, then a pointer to the pair.
        assert_eq!((layout.size, layout.align), (16, 8));
        assert_eq!(layout.variant(1), &[8]);
        // The pair itself holds two `Node`s inline, each of them 16 bytes.
        assert_eq!(l.of(at(pair, &[con(node)])).size, 32);
    }

    /// A recursive type through a tuple: the tuple is boxed whole, by the
    /// field that holds it.
    #[test]
    fn recursion_through_a_tuple_boxes_the_tuple() {
        let mut t = tables();
        let e = declare(&mut t, "E", &[]);
        define_enum(
            &mut t,
            e,
            vec![variant("Stop", &[]), variant("Go", &[Ty::Tuple(vec![p(Prim::I64), con(e)])])],
        );
        let mut l = Layouts::new(&t);
        let layout = l.of(con(e));
        assert_eq!((layout.size, layout.align), (16, 8));
        assert!(l.boxes(&con(e), &Ty::Tuple(vec![p(Prim::I64), con(e)])));
        // The tuple behind that pointer holds the `E` inline.
        assert_eq!(l.of(Ty::Tuple(vec![p(Prim::I64), con(e)])).fields, vec![0, 8]);
    }

    /// One constructor inside itself at smaller arguments is not a cycle, and
    /// boxing it would be a pointer for nothing.
    #[test]
    fn a_nested_instantiation_of_one_constructor_is_not_a_cycle() {
        let mut t = tables();
        let pair = add_struct(&mut t, "Pair", &["T"], &[("a", Ty::Param(0)), ("b", Ty::Param(0))]);
        let mut l = Layouts::new(&t);
        let inner = at(pair, &[p(Prim::I64)]);
        let outer = at(pair, std::slice::from_ref(&inner));
        assert!(!l.boxes(&outer, &inner));
        assert_eq!(l.of(outer).fields, vec![0, 16]);
    }

    /// A boxed field is a pointer that is never null, so an `Option` of a
    /// recursive struct still niches.
    #[test]
    fn option_of_a_struct_whose_field_is_boxed_niches_on_the_box() {
        let mut t = tables();
        let option = add_option(&mut t);
        let node = add_struct(&mut t, "Node", &[], &[]);
        t.tycon_mut(node).def =
            TyDef::Struct { fields: fields(&[("next", con(node)), ("n", p(Prim::I64))]), record: true };
        let mut l = Layouts::new(&t);
        assert_eq!(l.of(con(node)).fields, vec![0, 8]);
        let layout = l.of(at(option, &[con(node)]));
        assert_eq!(layout.size, 16);
        assert!(matches!(layout.repr, Repr::Enum { repr: EnumRepr::Niche { null_at: 0 }, .. }));
    }

    // -----------------------------------------------------------------------
    // Memoisation
    // -----------------------------------------------------------------------

    #[test]
    fn a_layout_is_computed_once() {
        let mut t = tables();
        let s = add_struct(&mut t, "S", &[], &[("a", p(Prim::I64)), ("b", p(Prim::Str))]);
        let mut l = Layouts::new(&t);
        let first = l.of(con(s));
        let entries = l.table.len();
        let second = l.of(con(s));
        assert_eq!(first, second);
        assert_eq!(l.table.len(), entries, "the second ask allocated a second layout");
        // And the members were interned on the way, rather than recomputed.
        assert!(l.memo.contains_key(&p(Prim::Str)));
    }

    // -----------------------------------------------------------------------
    // Properties
    // -----------------------------------------------------------------------

    /// A deterministic pool of types wide enough to reach every branch.
    fn pool(t: &mut Tables) -> Vec<Ty> {
        let option = add_option(t);
        let empty = add_struct(t, "Empty", &[], &[]);
        let mixed = add_struct(
            t,
            "Mixed",
            &[],
            &[("a", p(Prim::Bool)), ("b", p(Prim::I128)), ("c", p(Prim::Char))],
        );
        let flags = add_enum(t, "Flags", &[], vec![variant("On", &[]), variant("Off", &[])]);
        let shape = add_enum(
            t,
            "Shape",
            &[],
            vec![variant("Empty", &[]), variant("Circle", &[p(Prim::F64)])],
        );
        let mut out = vec![Ty::Unit, Ty::Array(Box::new(p(Prim::I64)))];
        out.push(Ty::Fn(vec![p(Prim::I64)], Box::new(Ty::Unit)));
        for prim in Prim::all() {
            out.push(p(*prim));
        }
        out.push(con(empty));
        out.push(con(mixed));
        out.push(con(flags));
        out.push(con(shape));
        out.push(at(option, &[p(Prim::Str)]));
        out.push(at(option, &[p(Prim::I64)]));
        out.push(at(option, &[at(option, &[p(Prim::Str)])]));
        out.push(Ty::Tuple(vec![p(Prim::Bool), con(mixed)]));
        out
    }

    /// A tiny linear congruential generator, so the property tests run the
    /// same cases on every machine and in every order.
    fn next(seed: &mut u64) -> u64 {
        *seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        *seed >> 33
    }

    #[test]
    fn every_offset_respects_its_own_alignment() {
        let mut t = tables();
        let members = pool(&mut t);
        let mut l = Layouts::new(&t);
        let mut seed = 0x5eed_u64;
        for _ in 0..2000 {
            let count = (next(&mut seed) % 6) as usize;
            let chosen: Vec<Ty> = (0..count)
                .map(|_| members[(next(&mut seed) as usize) % members.len()].clone())
                .collect();
            let layout = l.of(Ty::Tuple(chosen.clone()));
            assert!(layout.align.is_power_of_two(), "{layout:?}");
            assert_eq!(layout.size % layout.align, 0, "size is not rounded up: {layout:?}");
            assert_eq!(layout.stride, layout.size);
            let mut sum = 0u32;
            for (member, offset) in chosen.iter().zip(&layout.fields) {
                let m = l.of(member.clone());
                assert_eq!(offset % m.align, 0, "{member:?} at {offset} is misaligned");
                assert!(offset + m.size <= layout.size, "{member:?} runs past the end");
                assert!(m.align <= layout.align, "a member out-aligns its container");
                sum += m.size;
            }
            assert!(layout.size >= sum, "the total is smaller than its parts: {layout:?}");
        }
    }

    #[test]
    fn every_type_in_the_pool_is_self_consistent() {
        let mut t = tables();
        let members = pool(&mut t);
        let mut l = Layouts::new(&t);
        for ty in &members {
            let layout = l.of(ty.clone());
            assert!(layout.align.is_power_of_two(), "{ty:?}");
            assert_eq!(layout.stride, align_up(layout.size, layout.align), "{ty:?}");
            assert_eq!(layout.size % layout.align, 0, "{ty:?}");
            // A list of it is a pointer and a length, whatever it is.
            assert_eq!(l.of(Ty::Array(Box::new(ty.clone()))).size, 16);
        }
    }

    /// Deep is not the same as cyclic. A type nested as far as the parser
    /// would ever hand over is laid out, not fused.
    #[test]
    fn deep_nesting_is_not_mistaken_for_a_cycle() {
        let t = tables();
        let mut l = Layouts::new(&t);
        let mut ty = p(Prim::I64);
        for _ in 0..300 {
            ty = Ty::Tuple(vec![ty]);
        }
        let layout = l.of(ty);
        assert_eq!((layout.size, layout.align), (8, 8));
    }

    #[test]
    fn align_up_rounds_and_does_not_overflow() {
        assert_eq!(align_up(0, 8), 0);
        assert_eq!(align_up(1, 8), 8);
        assert_eq!(align_up(8, 8), 8);
        assert_eq!(align_up(9, 16), 16);
        assert_eq!(align_up(7, 1), 7);
        assert_eq!(align_up(u32::MAX, 16), 0xffff_fff0);
    }

    // -----------------------------------------------------------------------
    // The `Alloc` cost model
    // -----------------------------------------------------------------------

    #[test]
    fn the_charge_for_a_list_is_the_header_plus_the_elements() {
        let t = tables();
        let mut l = Layouts::new(&t);
        assert_eq!(l.charge_list(&p(Prim::I64), 10), 16 + 80);
        assert_eq!(l.charge_list(&p(Prim::Str), 3), 16 + 72);
        // `[(A, B)]` from `list.zip` is one block, at the pair's stride.
        let pairs = Ty::Tuple(vec![p(Prim::I64), p(Prim::Bool)]);
        assert_eq!(l.charge_list(&pairs, 4), 16 + 64);
        assert_eq!(l.charge_list(&Ty::Unit, 1000), 16);
    }

    #[test]
    fn the_charge_for_a_string_is_the_header_plus_its_bytes() {
        assert_eq!(charge_str(0), 16);
        assert_eq!(charge_str(7), 23);
        // A view charges nothing, because `slice` carries no `Alloc` bound.
        assert_eq!(CHARGE_VIEW, 0);
        assert_eq!(charge_allocate(4096), 4096);
    }

    #[test]
    fn the_charge_for_a_closure_environment_is_its_record() {
        let t = tables();
        let mut l = Layouts::new(&t);
        assert_eq!(l.charge_closure_env(&[]), 16);
        assert_eq!(l.charge_closure_env(&[p(Prim::Bool), p(Prim::I64)]), 16 + 16);
        assert_eq!(l.charge_closure_env(&[p(Prim::Str)]), 16 + 24);
    }

    // -----------------------------------------------------------------------
    // The golden rendering
    // -----------------------------------------------------------------------

    #[test]
    fn a_representative_set_renders_the_model() {
        let mut t = tables();
        let option = add_option(&mut t);
        let point = add_struct(&mut t, "Point", &[], &[("x", p(Prim::F64)), ("y", p(Prim::F64))]);
        let padded = add_struct(
            &mut t,
            "Padded",
            &[],
            &[("a", p(Prim::Bool)), ("b", p(Prim::I64)), ("c", p(Prim::Bool))],
        );
        let order = add_enum(
            &mut t,
            "Order",
            &[],
            vec![variant("Less", &[]), variant("Equal", &[]), variant("Greater", &[])],
        );
        let shape = add_enum(
            &mut t,
            "Shape",
            &[],
            vec![
                variant("Empty", &[]),
                variant("Circle", &[p(Prim::F64)]),
                variant("Rect", &[p(Prim::F64), p(Prim::F64)]),
            ],
        );
        let host = add_struct(&mut t, "HostFs", &[], &[]);
        let fs = add_trait(&mut t, "Fs");
        let free = add_ctx(&mut t, vec![(fs, con(host))]);
        let chain = declare(&mut t, "Chain", &[]);
        define_enum(
            &mut t,
            chain,
            vec![variant("Nil", &[]), variant("Cons", &[p(Prim::I64), con(chain)])],
        );

        let types = vec![
            Ty::Unit,
            p(Prim::Bool),
            p(Prim::I64),
            p(Prim::Char),
            p(Prim::I128),
            p(Prim::Str),
            Ty::Array(Box::new(p(Prim::I64))),
            Ty::Fn(vec![p(Prim::I64)], Box::new(p(Prim::Bool))),
            Ty::Tuple(vec![p(Prim::Str), p(Prim::Bool)]),
            con(point),
            con(padded),
            con(order),
            con(shape),
            at(option, &[p(Prim::Str)]),
            at(option, &[p(Prim::I64)]),
            at(option, &[at(option, &[p(Prim::Str)])]),
            con(host),
            Ty::Ctx(free),
            con(chain),
        ];
        let mut l = Layouts::new(&t);
        let rendered = l.describe_all(&types);
        assert_eq!(rendered, GOLDEN, "\n--- actual ---\n{rendered}\n");
    }

    /// The value model, printed. A change to any rule above moves a line here,
    /// which is the point: a layout that drifts is a diff and not a surprise
    /// in a backend.
    const GOLDEN: &str = "\
(): size 0, align 1, stride 0
  zero-sized
Bool: size 1, align 1, stride 1
  scalar bool
I64: size 8, align 8, stride 8
  scalar i64
Char: size 4, align 4, stride 4
  scalar i32
I128: size 16, align 16, stride 16
  scalar i128
Str: size 24, align 8, stride 24
  str { base @0, ptr @8, len @16 }
[I64]: size 16, align 8, stride 16
  list { ptr @0, len @8 }
fn(I64) => Bool: size 16, align 8, stride 16
  closure { code @0, env @8 }
(Str, Bool): size 32, align 8, stride 32
  aggregate { 0 @0, 1 @24 }
Point: size 16, align 8, stride 16
  aggregate { x @0, y @8 }
Padded: size 24, align 8, stride 24
  aggregate { a @0, b @8, c @16 }
Order: size 1, align 1, stride 1
  enum bare tag i8
    .Less
    .Equal
    .Greater
Shape: size 24, align 8, stride 24
  enum tag i8 @0, payload @8
    .Empty
    .Circle(@8)
    .Rect(@8, @16)
Option<Str>: size 24, align 8, stride 24
  enum niche, .None is a null pointer @8
    .Some(@0)
    .None
Option<I64>: size 16, align 8, stride 16
  enum tag i8 @0, payload @8
    .Some(@8)
    .None
Option<Option<Str>>: size 32, align 8, stride 32
  enum tag i8 @0, payload @8
    .Some(@8)
    .None
HostFs: size 0, align 1, stride 0
  zero-sized
context { Fs }: size 0, align 1, stride 0
  zero-sized
Chain: size 24, align 8, stride 24
  enum tag i8 @0, payload @8
    .Nil
    .Cons(@8, *@16)";
}
