//! The copy-and-patch code generator.
//!
//! Paper §4: "for each node, look up the stencil corresponding to the node's
//! configuration from the stencil library. It then copies the stencil's binary
//! code into the output buffer and patches the holes." That is `emit` and
//! `patch` below, and there is no other pass: no instruction selection beyond
//! the stencil key, no register allocator beyond the CPS assignment in
//! `regalloc`, no scheduling, and no peephole other than the fallthrough
//! elision the paper's continuation-passing makes free.
//!
//! # The frame, and the call convention
//!
//! Every SSA value gets a byte range in a frame. An aggregate lives **flat** in
//! that range at its real `middle::layout` offsets, so `MakeStruct`,
//! `GetField`, `GetPayload` and `GetTag` are frame-to-frame moves and there is
//! no boxing anywhere. A frame is
//!
//! ```text
//!   fp + 0            return area   (the callee writes here, the caller reads)
//!   fp + ret_size     parameters    (the caller writes here before `bl`)
//!   ...               locals
//!   fp + frame_size   the callee's frame begins
//! ```
//!
//! so a call is: write the arguments where the callee will look, `bl`, read the
//! return area. There is no push and no stack pointer: the callee's frame
//! address is `fp + frame_size`, a constant this function knows, and it is one
//! of the holes in the `call` stencil.

#![allow(
    clippy::arithmetic_side_effects,
    reason = "three bounded quantities and nothing else. Frame offsets, which \
              accumulate the slot widths of one function's values and are \
              therefore bounded by the frame `frame_sigs` sized from the same \
              widths. Byte counts over the region — a stencil's length, a \
              branch displacement between two addresses inside it — which \
              cannot exceed code this emitter has already copied out. And \
              counters over tables it built itself: one entry per value or \
              block of the `ir::Code` in hand, so a use count is bounded by \
              the operands of a program already in memory. The one \
              subtraction, dropping a stencil's elided tail branch, runs only \
              where a tail branch was found — four bytes that exist on A64, \
              and the five of a `jmp rel32` on x86-64"
)]

use super::abi::{HoleName, Jump, Lit, Loc, Reg, Slot, StencilTarget, Word};
use super::library::{Hole, Library, Stencil};
use super::object::RelKind;
use super::region::{Region, Target};
use super::runtime;
use crate::compiler::backend::counts::{Counts, Site};
use crate::compiler::middle::ir;
use crate::compiler::middle::layout::{At, EnumLayout, EnumRepr, Layout, Layouts, Repr};
use crate::compiler::semantics::layered::Dense;
use crate::compiler::semantics::types::{variant_types, FuncIdx, Tables, Ty};
use crate::hash::Map as HashMap;
use std::borrow::Cow;

// ---------------------------------------------------------------------------
// Hole values
// ---------------------------------------------------------------------------

/// Where a [`Jump`] hole goes.
#[derive(Clone, Debug)]
pub enum V {
    /// The address of a basic block, or a synthetic label, in the body
    /// being emitted.
    Blk(Label),
    /// The entry of a function.
    Fn(FuncIdx),
    /// A C symbol in this process, through a veneer.
    Ext(&'static str),
    /// A symbol this unit defines for itself: one of `glue.rs`'s helpers.
    ///
    /// Separate from [`V::Ext`] only because the name is chosen at emission
    /// time and cannot be a `&'static str`; the two are patched identically.
    Sym(String),
    /// The stencil laid out immediately after this one — the paper's "control
    /// is passed directly to the next operation", which costs zero bytes
    /// because the branch is dropped rather than patched.
    Fall,
}

/// What [`Jit::emit`] binds one hole to: a value of the class the hole's
/// [`Slot`] is declared in, so a hole is only ever handed a value its
/// relocation can hold.
#[derive(Clone, Debug)]
pub enum Bind {
    Lit(Lit, u64),
    Word(Word, Datum),
    Jump(Jump, V),
}

/// A full-width datum: a [`Word`] hole's value.
#[derive(Clone, Debug)]
pub enum Datum {
    /// A literal: a constant, a mask, a pooled 64-bit datum.
    I(u64),
    /// A literal that **is an address inside the region** — a string literal's
    /// bytes, an abort message, a runtime helper's descriptor. Identical to
    /// `I` at emission time and distinguished from it for exactly one reason:
    /// the image cache has to know which pooled words move when the region is
    /// mapped somewhere else. See `cache.rs`.
    Ptr(u64),
    /// A C symbol in this process, as [`V::Ext`].
    Ext(&'static str),
    /// A symbol this unit defines for itself, as [`V::Sym`].
    Sym(String),
}

impl Lit {
    pub fn of(self, x: u64) -> Bind {
        Bind::Lit(self, x)
    }
}

impl Word {
    pub fn lit(self, x: u64) -> Bind {
        Bind::Word(self, Datum::I(x))
    }

    pub fn ptr(self, x: u64) -> Bind {
        Bind::Word(self, Datum::Ptr(x))
    }

    pub fn ext(self, n: &'static str) -> Bind {
        Bind::Word(self, Datum::Ext(n))
    }

    pub fn sym(self, n: String) -> Bind {
        Bind::Word(self, Datum::Sym(n))
    }
}

impl Jump {
    pub fn to(self, v: V) -> Bind {
        Bind::Jump(self, v)
    }
}

impl Bind {
    /// The hole this binds.
    fn slot(&self) -> Slot {
        match *self {
            Bind::Lit(l, _) => Slot::Lit(l),
            Bind::Word(w, _) => Slot::Word(w),
            Bind::Jump(j, _) => Slot::Jump(j),
        }
    }

    /// The literal, where this is one: what a folded twin's `imm12` takes.
    fn literal(&self) -> Option<u64> {
        match *self {
            Bind::Lit(_, x) | Bind::Word(_, Datum::I(x)) => Some(x),
            _ => None,
        }
    }
}

/// The bind for hole `h`, where the caller named it.
fn bind_of<'b>(binds: &'b [Bind], h: &Hole) -> Option<&'b Bind> {
    match h.name {
        HoleName::Slot(slot) => binds.iter().find(|b| b.slot() == slot),
        HoleName::Symbol(_) => None,
    }
}

/// A stencil key, built without allocating ([`key!`]).
///
/// The library's longest name is 31 bytes, its fold and swap suffixes
/// included, so every key fits inline. A longer spelling keeps its first
/// [`KEY_INLINE`] bytes, which is longer than any name
/// (`every_shipped_name_is_shorter_than_a_key`), so it names no stencil and
/// the lookup misses as it would have.
#[derive(Clone, Default)]
pub struct Key {
    len: usize,
    inline: [u8; KEY_INLINE],
}

pub(super) const KEY_INLINE: usize = 32;

impl Key {
    pub fn as_str(&self) -> &str {
        self.inline
            .get(..self.len)
            .and_then(|b| std::str::from_utf8(b).ok())
            .unwrap_or_default()
    }
}

impl Key {
    fn push(&mut self, s: &str) {
        let _ = std::fmt::Write::write_str(self, s);
    }

    /// `n` in decimal, as `format!` writes it.
    fn push_number(&mut self, mut n: u64) {
        let mut digits = [0u8; 20];
        let mut at = digits.len();
        for d in digits.iter_mut().rev() {
            *d = b'0' + (n % 10) as u8;
            at -= 1;
            n /= 10;
            if n == 0 {
                break;
            }
        }
        let s = digits
            .get(at..)
            .and_then(|d| std::str::from_utf8(d).ok())
            .unwrap_or_default();
        self.push(s);
    }
}

/// One piece of a [`key!`].
pub trait KeyPart {
    fn spell(self, key: &mut Key);
}

impl KeyPart for &str {
    fn spell(self, key: &mut Key) {
        key.push(self);
    }
}

impl KeyPart for &String {
    fn spell(self, key: &mut Key) {
        key.push(self);
    }
}

impl KeyPart for std::borrow::Cow<'_, str> {
    fn spell(self, key: &mut Key) {
        key.push(&self);
    }
}

impl KeyPart for &std::borrow::Cow<'_, str> {
    fn spell(self, key: &mut Key) {
        key.push(self);
    }
}

impl KeyPart for u32 {
    fn spell(self, key: &mut Key) {
        key.push_number(u64::from(self));
    }
}

impl KeyPart for u64 {
    fn spell(self, key: &mut Key) {
        key.push_number(self);
    }
}

impl KeyPart for usize {
    fn spell(self, key: &mut Key) {
        key.push_number(self as u64);
    }
}

impl KeyPart for u8 {
    fn spell(self, key: &mut Key) {
        key.push_number(u64::from(self));
    }
}

/// A register spells its number.
impl KeyPart for Reg {
    fn spell(self, key: &mut Key) {
        key.push_number(self as u64);
    }
}

impl std::fmt::Write for Key {
    fn write_str(&mut self, s: &str) -> std::fmt::Result {
        for (dst, b) in self.inline.iter_mut().skip(self.len).zip(s.as_bytes()) {
            *dst = *b;
        }
        self.len = (self.len + s.len()).min(KEY_INLINE);
        Ok(())
    }
}

impl std::ops::Deref for Key {
    type Target = str;
    fn deref(&self) -> &str {
        self.as_str()
    }
}

impl std::fmt::Display for Key {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A stencil key as [`Jit::emit`] takes it.
///
/// Most emitted stencils are named by a literal (`"ret"`, `"jump"`,
/// `"decref/free"`), and a literal's address never changes, so the worker's
/// [`Literals`] answers it by address instead of hashing it into the library's
/// index on every emit.
#[derive(Clone, Copy)]
pub(crate) enum KeyArg<'k> {
    Literal(&'static str),
    Spelled(&'k str),
    /// A stencil already found, by its index in the library.
    At(usize),
}

impl From<&'static str> for KeyArg<'static> {
    fn from(s: &'static str) -> KeyArg<'static> {
        KeyArg::Literal(s)
    }
}

impl<'k> From<&'k Key> for KeyArg<'k> {
    fn from(k: &'k Key) -> KeyArg<'k> {
        KeyArg::Spelled(k.as_str())
    }
}

/// Library indices of the literal keys a worker has emitted, by the literal's
/// address: direct-mapped, so a collision costs one more index lookup.
struct Literals {
    /// The library the indices are into.
    lib: *const Library,
    slots: [(usize, usize, u32); LITERAL_SLOTS],
}

const LITERAL_SLOTS: usize = 256;

impl Default for Literals {
    fn default() -> Literals {
        Literals {
            lib: std::ptr::null(),
            slots: [(0, 0, 0); LITERAL_SLOTS],
        }
    }
}

impl Literals {
    fn slot(key: &'static str) -> usize {
        let a = key.as_ptr() as usize;
        (a ^ (a >> 7) ^ (a >> 14)) % LITERAL_SLOTS
    }

    /// `key`'s index in `lib`, asking the library the first time.
    fn at(&mut self, lib: &Library, key: &'static str) -> Option<usize> {
        if !std::ptr::eq(self.lib, lib) {
            *self = Literals {
                lib,
                ..Literals::default()
            };
        }
        let id = (key.as_ptr() as usize, key.len());
        let slot = self.slots.get_mut(Literals::slot(key))?;
        if (slot.0, slot.1) == id {
            return Some(slot.2 as usize);
        }
        let (i, _) = lib.at(key)?;
        *slot = (id.0, id.1, i as u32);
        Some(i)
    }
}

#[derive(Clone, Copy)]
enum Fix {
    /// A `b`/`bl` whose target is a block of the function being emitted.
    Block { at: u64, blk: Label },
    /// The same, for a conditional branch whose `imm19` the cond fold made a
    /// hole. See `extract::fold_cond`.
    BlockCond { at: u64, blk: Label },
    /// A `b`/`bl` whose target is a function entry.
    Func { at: u64, f: FuncIdx },
}

/// One generated helper, as [`Jit::helper_symbols`] answers it.
pub struct HelperSymbol {
    pub name: String,
    /// Where its bytes start and end within the part.
    pub at: u64,
    pub end: u64,
    /// Whether `name` is program-wide ([`super::glue::shared_symbol`]).
    pub shared: bool,
}

pub struct Jit<'a> {
    pub(crate) lib: &'static Library,
    /// What each intrinsic key the program names is emitted as.
    pub(crate) routes: &'a super::route::Routes<'a>,
    /// Which machine's fields are being patched.
    ///
    /// The one axis this file has: a stencil's bytes are clang's, so *where* a
    /// hole is and *what* a patch writes are the instruction set's business
    /// and nothing else's. `abi::StencilTarget::is_arm64` is the question, and
    /// every place below that asks it says what the two answers are.
    pub(crate) target: StencilTarget,
    pub region: Region,
    layouts: Layouts<'a>,
    /// The type tables, for the questions a `Layout` does not answer — what a
    /// closure type's parameter and result types are (the open-coded `list.*`
    /// loops).
    pub(crate) tables: &'a Tables,
    /// Per function: (frame size, return offsets, parameter offsets).
    ///
    /// **Borrowed, not owned, and computed once for the whole emission.** It is
    /// a whole-*program* table — a call site needs the callee's frame whether or
    /// not this unit owns the callee — so computing it inside `plan` made the
    /// emitter O(units × program): at 104k lines and 367 units it walked a 121k
    /// line program 367 times, which was 2,378 ms of a 3,184 ms emission and
    /// most of `Layouts::compute`. `mod.rs::emit_units` computes it beside
    /// `lower::run`, which is where the other whole-program work already is.
    frames: &'a Dense<FuncIdx, FrameSig>,
    /// Section offset of each function of *this unit*; zero for a function the
    /// unit does not own. It is what the object's symbol table is written from
    /// — not what a call site reads, because every call is a relocation
    /// (see [`Jit::resolve`]).
    entries: Dense<FuncIdx, u64>,
    fixups: Vec<Fix>,
    /// Whether the region may be appended to right now. A conditional branch
    /// out of range needs a veneer, and a veneer may only be planted between
    /// functions — never in the middle of one, where it would land inside the
    /// fallthrough of the stencil being patched.
    veneer_ok: bool,
    /// The functions this unit generates for itself, in the order they were
    /// first asked for, and where each was laid out.
    ///
    /// A `Vec` and not a map because the order is the object's symbol order and
    /// `--check-reproducible` compares two builds byte for byte; the map beside
    /// it is only a lookup. `glue.rs` is the set, and its header is the
    /// argument.
    helpers: Vec<super::glue::Helper>,
    helper_ix: HashMap<super::glue::Helper, usize>,
    /// Each helper's symbol, and whether it is program-wide
    /// ([`super::glue::shared_symbol`]) rather than this part's own.
    helper_names: Vec<(String, bool)>,
    /// Program-wide symbols already registered, so two types with one glue key
    /// share one body.
    shared_ix: HashMap<String, usize>,
    /// Where each helper starts and ends within the part.
    helper_at: Vec<(u64, u64)>,
    /// Where each stencil's spilled constants were copied into this unit's
    /// pool, by stencil name. One copy per unit: the bytes are clang's
    /// `.rodata` and every copy of the stencil reads the same ones.
    spilled: HashMap<String, u64>,
    /// Where each type's counts live, memoised because the question is asked
    /// once per reference operation and answering it walks the type.
    pub(crate) counts: Counts,
    /// Which part of its unit this is, which is the namespace this part's
    /// generated helpers take their local symbols from
    /// ([`super::glue::symbol`]).
    part: usize,
    bufs: Bufs,
    /// Whether the program can fan out (`runtime::shares_counts`), so every
    /// count reads `buri_rt_shared_mask` (`emit.rs`'s `rc_key`).
    pub(crate) shares: bool,
}

/// The two memo tables a `Jit` is worth keeping across the parts one worker
/// emits.
///
/// Both are caches of a pure function of the type tables — a layout, and
/// where a type's counts live — so an answer computed for one part is
/// the answer for the next one, and neither is an answer that *accumulates*.
/// That is `parallel::map_with`'s scratch contract exactly, and it is why the
/// parts of a unit cost one memo per **worker** rather than one per part:
/// filling a `Layouts` is the largest thing in a `Jit` that is not the emitted
/// bytes, and a part is small enough that paying for it per part would be most
/// of what dividing the unit bought.
pub(crate) struct Scratch<'a> {
    layouts: Layouts<'a>,
    counts: Counts,
    /// Storage, not answers: every table in it is cleared before it is read.
    bufs: Bufs,
}

/// The per-function tables the analyses below fill, kept from one function to
/// the next so their storage is allocated once per worker rather than once per
/// function. [`ir::Code::per_value_in`] hands one out cleared and refilled.
#[derive(Default)]
struct Bufs {
    /// The last function's [`Fn2`], whose tables the next one refills.
    st: Fn2,
    order: Vec<ir::BlockId>,
    skip: Vec<bool>,
    pin: ir::PerValue<Option<u32>>,
    width: ir::PerValue<u32>,
    uf: ir::PerValue<ir::ValueId>,
    of_root: ir::PerValue<Option<u32>>,
    wide: ir::PerValue<u32>,
    is_param: ir::PerValue<bool>,
    def_block: ir::PerValue<Option<ir::BlockId>>,
    after_def: ir::PerValue<u32>,
    members: ir::PerValue<u32>,
    reads: ir::PerValue<BlockUses>,
    defs: ir::PerValue<Option<(ir::BlockId, usize)>>,
    kept: Vec<usize>,
    block_uses: ir::PerValue<BlockUses>,
    barrier: Vec<bool>,
    imm: ir::PerValue<u32>,
    seen: ir::PerBlock<bool>,
    stack: Vec<(ir::BlockId, usize)>,
    param: ir::PerValue<bool>,
    def_in: ir::PerValue<(u32, u32)>,
    aliased: ir::PerValue<bool>,
    fixed: ir::PerValue<bool>,
    last: ir::PerValue<(u32, u32)>,
    touched: Vec<Option<usize>>,
    opnd: Operands,
    literals: Literals,
    loops: Loops,
    pinned: ir::PerValue<bool>,
    grown: ir::PerValue<bool>,
}

/// What every instruction and terminator of one function reads, listed once.
///
/// Five analyses walk a function's operands, and `Inst::operands` is a match
/// and a push per operand each time. This is that walk done once, as one flat
/// list with a row per instruction and one per terminator. A terminator's row
/// is its own operands and then its edges' arguments.
#[derive(Default)]
struct Operands {
    /// Where each row starts in `ops`, and one past the last row.
    at: Vec<u32>,
    ops: Vec<ir::ValueId>,
    /// Each block's first row: its instructions', then its terminator's.
    first: ir::PerBlock<u32>,
}

impl Operands {
    fn fill(&mut self, code: &ir::Code) {
        let Operands { at, ops, first } = self;
        at.clear();
        ops.clear();
        *first = code.per_block_in(first, 0);
        for (id, row) in first.iter_mut() {
            let b = code.get(id);
            *row = at.len() as u32;
            for i in &b.insts {
                at.push(ops.len() as u32);
                i.operands(ops);
            }
            at.push(ops.len() as u32);
            b.term.operands(ops);
            for t in b.term.targets() {
                ops.extend_from_slice(&t.args);
            }
        }
        at.push(ops.len() as u32);
    }

    fn row(&self, r: usize) -> &[ir::ValueId] {
        let lo = ent(&self.at, r, 0) as usize;
        let hi = ent(&self.at, r + 1, 0) as usize;
        self.ops.get(lo..hi).unwrap_or_default()
    }

    /// What instruction `k` of block `b` reads.
    fn inst(&self, b: ir::BlockId, k: usize) -> &[ir::ValueId] {
        self.row(*self.first.get(b) as usize + k)
    }

    /// What block `b`'s terminator reads, its edges' arguments included.
    fn term(&self, code: &ir::Code, b: ir::BlockId) -> &[ir::ValueId] {
        self.row(*self.first.get(b) as usize + code.get(b).insts.len())
    }
}

impl<'a> Scratch<'a> {
    pub(crate) fn new(
        tables: &'a Tables,
        cycles: std::sync::Arc<crate::compiler::middle::layout::Cycles>,
    ) -> Scratch<'a> {
        Scratch {
            layouts: Layouts::with_cycles(tables, cycles),
            counts: Counts::default(),
            bufs: Bufs::default(),
        }
    }
}

/// Where a frame keeps its result: first, at `fp + 0` (this file's header).
pub(crate) const RET: u32 = 0;

#[derive(Clone, Default)]
pub(crate) struct FrameSig {
    pub ret_size: u32,
    pub params: Vec<u32>,
    pub param_end: u32,
    pub size: u32,
}

fn round8(n: u32) -> u32 {
    (n + 7) & !7
}

// ---------------------------------------------------------------------------
// The analyses' side tables
// ---------------------------------------------------------------------------
//
// Every table the analyses below build per value or per block is an
// `ir::PerValue` or an `ir::PerBlock` made from the `ir::Code` in hand, and
// every index into one is a `ValueId` or a `BlockId` of that same code. What is
// left indexed by position is per instruction of one block, or a row of
// [`Operands`], read through `ent`.

/// Entry `i` of a table, or `d` when the table has none.
fn ent<T: Copy>(t: &[T], i: usize, d: T) -> T {
    t.get(i).copied().unwrap_or(d)
}

/// A literal zero on the right of an integer `/` or `%`, which is the one
/// constant that must **not** become a stencil immediate.
///
/// `sources.rs` puts SPEC 6.2's abort in the stencil itself, as
/// `if ((B) == 0) buri_rt_abort_div_zero();`. In the immediate variant `B`
/// reads `(uintptr_t)_JIT_K`, and `_JIT_K` is an `extern char[]` — an address
/// the stencil's own compiler proves non-null, so it deletes the guard. Every
/// other variant reads a frame slot or a register and keeps it.
///
/// Refusing the fold leaves the zero in a frame slot, where the guard is a real
/// comparison. A non-zero divisor still folds: there the deleted guard is a
/// guard that could not have fired.
fn zero_divisor(name: &str, tag: &str, k: u64) -> bool {
    matches!(name, "div" | "rem") && !matches!(tag, "f32" | "f64") && k == 0
}

/// What a register of [`Jit::regalloc`]'s holds.
#[derive(Clone, Copy)]
enum Held {
    /// A loop variable [`Jit::promote`] gave it, which is not the local
    /// allocator's to free.
    Promoted,
    Value(ir::ValueId),
}

/// The uses of one value within one block, for [`Jit::regalloc`] and
/// [`Jit::pin_call_values`].
#[derive(Clone, Copy, Default)]
struct BlockUses {
    /// The block this entry describes; any other block reads it as unused.
    block: Option<ir::BlockId>,
    count: u32,
    /// Instruction indices, the terminator being one past the last.
    first: usize,
    last: usize,
}

/// Records a use of `o` at instruction `at` of block `block`. Uses arrive in
/// instruction order, so the latest is the last.
fn note_use(uses: &mut ir::PerValue<BlockUses>, block: ir::BlockId, o: ir::ValueId, at: usize) {
    let u = uses.get_mut(o);
    if u.block == Some(block) {
        u.count += 1;
        u.last = at;
    } else {
        *u = BlockUses {
            block: Some(block),
            count: 1,
            first: at,
            last: at,
        };
    }
}

/// The representative of `v`'s slot class.
///
/// `uf` starts as the identity and [`Jit::coalesce`] only ever points a value
/// at a *root*, so the walk ends at a value that is its own parent.
fn find(uf: &ir::PerValue<ir::ValueId>, mut v: ir::ValueId) -> ir::ValueId {
    loop {
        let parent = *uf.get(v);
        if parent == v {
            return v;
        }
        v = parent;
    }
}

// ---------------------------------------------------------------------------
// Construction
// ---------------------------------------------------------------------------

impl<'a> Jit<'a> {
    /// `cycles` is [`crate::compiler::middle::layout::Cycles`] over the same
    /// tables, computed **once for the emission** and handed to every unit.
    ///
    /// It used to be built here, which is `Layouts::new`, which is a walk of
    /// every constructor in the program followed by a strongly-connected-
    /// components pass — per unit. `layout.rs`'s own header says what that
    /// costs ("building one per unit made a native build quadratic in the
    /// number of units", `design/PERFORMANCE.md` §6.4) and names
    /// `Layouts::with_cycles` as the answer; the LLVM backend took it and this
    /// one had not.
    pub(crate) fn new(
        lib: &'static Library,
        routes: &'a super::route::Routes<'a>,
        tables: &'a Tables,
        frames: &'a Dense<FuncIdx, FrameSig>,
        target: StencilTarget,
        scratch: Scratch<'a>,
        part: usize,
    ) -> Jit<'a> {
        Jit {
            lib,
            routes,
            target,
            region: Region::new(),
            layouts: scratch.layouts,
            tables,
            frames,
            entries: Dense::new(),
            fixups: Vec::new(),
            veneer_ok: false,
            helpers: Vec::new(),
            helper_ix: HashMap::default(),
            helper_names: Vec::new(),
            shared_ix: HashMap::default(),
            helper_at: Vec::new(),
            spilled: HashMap::default(),
            counts: scratch.counts,
            bufs: scratch.bufs,
            part,
            shares: false,
        }
    }

    /// The two memo tables back, for the next part this worker emits.
    ///
    /// Everything else a `Jit` holds is *this part's* — a region, a fixup list,
    /// a helper table whose indices are symbol names, a map of where this
    /// part's pool put a stencil's spilled constants — and is dropped here
    /// rather than reset, so a field added later cannot leak into the next part
    /// by being forgotten.
    pub(crate) fn into_scratch(self) -> Scratch<'a> {
        Scratch {
            layouts: self.layouts,
            counts: self.counts,
            bufs: self.bufs,
        }
    }

    /// The symbol of a generated helper, registering it the first time it is
    /// asked for.
    ///
    /// Drop and copy glue gets its program-wide name
    /// ([`super::glue::shared_symbol`]); everything else is a local symbol of
    /// this part.
    pub(crate) fn helper(&mut self, h: super::glue::Helper) -> String {
        if let Some(i) = self.helper_ix.get(&h) {
            return self
                .helper_names
                .get(*i)
                .map(|(n, _)| n.clone())
                .unwrap_or_default();
        }
        let shared = super::glue::shared_symbol(&h, &mut self.layouts);
        if let Some(i) = shared.as_ref().and_then(|n| self.shared_ix.get(n)).copied() {
            self.helper_ix.insert(h, i);
            return shared.unwrap_or_default();
        }
        let i = self.helpers.len();
        let name = match shared {
            Some(n) => {
                self.shared_ix.insert(n.clone(), i);
                (n, true)
            }
            None => (super::glue::symbol(self.part, i), false),
        };
        self.helper_ix.insert(h.clone(), i);
        self.helpers.push(h);
        self.helper_names.push(name.clone());
        name.0
    }

    /// Every helper this part generated, in the order it was laid out.
    pub fn helper_symbols(&self) -> Vec<HelperSymbol> {
        self.helper_names
            .iter()
            .zip(&self.helper_at)
            .map(|((name, shared), (at, end))| HelperSymbol {
                name: name.clone(),
                at: *at,
                end: *end,
                shared: *shared,
            })
            .collect()
    }

    /// Where `key` is in the library.
    fn index_of(&mut self, key: KeyArg<'_>) -> Option<usize> {
        match key {
            KeyArg::Literal(k) => self.bufs.literals.at(self.lib, k),
            KeyArg::Spelled(k) => self.lib.at(k).map(|(i, _)| i),
            KeyArg::At(i) => Some(i),
        }
    }

    pub(crate) fn has(&self, key: &str) -> bool {
        self.lib.get(key).is_some()
    }

    /// [`Jit::has`], answered as the stencil's index.
    pub(crate) fn at(&self, key: &str) -> Option<usize> {
        self.lib.at(key).map(|(i, _)| i)
    }

    /// The name of the hole whose branch is the **last** instruction of the
    /// two-target stencil at `at` in the library — the only arm copy-and-patch
    /// can elide. Which one it is is clang's layout decision, not the
    /// emitter's, and it flips with the comparison. The `+fold` twin `emit`
    /// may pick ends in the same hole (`sources.rs` keeps no other), so the
    /// caller never has to know which one it gets.
    ///
    /// Borrowed out of the library rather than copied out of it, and the fold
    /// twin asked for by index: this is called twice per conditional branch
    /// the backend emits, and it used to allocate a `String` for the name, a
    /// second for the comparison and a third for `key+fold`.
    pub(crate) fn elidable_at(&self, at: usize) -> Option<Jump> {
        let s = self.lib.stencil(at)?;
        match s.holes.get(s.tail?)?.name {
            HoleName::Slot(Slot::Jump(j)) => Some(j),
            _ => None,
        }
    }

    /// `f`'s frame layout, borrowed from the program's table.
    pub(crate) fn frame_sig_of(&self, f: FuncIdx) -> &'a FrameSig {
        self.frames.get(f)
    }

    /// The layout of a source type directly, for the places the IR's `TypeId`
    /// is not the type wanted (a `[T]`'s element, a closure's return).
    pub(crate) fn layouts_of(&mut self, ty: Ty) -> std::rc::Rc<Layout> {
        self.layouts.shared(&ty)
    }

    /// The same answer, shared rather than copied — and without the `Ty` clone
    /// the owning form needs.
    ///
    /// `layout.rs`'s own note on [`Layouts::shared`] states the rule this
    /// exists for: "every caller in a loop over instructions must use this",
    /// because a `Layout` carries one `Vec<u32>` per variant and copying it
    /// per instruction is quadratic in the width of the widest enum a program
    /// touches. The reference-counting walk is that loop — it asks for a
    /// layout per field of per variant of per value it releases — and it was
    /// the largest single cost in emitting `core/orderedmap`.
    pub(crate) fn layout_shared(&mut self, ty: &Ty) -> std::rc::Rc<Layout> {
        self.layouts.shared(ty)
    }

    /// An interned enum's layout: its discriminant encoding and its variants.
    pub(crate) fn enum_layout(
        &mut self,
        prog: &ir::Program,
        e: ir::EnumTy,
    ) -> std::rc::Rc<EnumLayout> {
        self.layouts.of_enum(&prog.types.enum_head(e))
    }

    /// [`Jit::layout_of`], shared.
    pub(crate) fn layout_id_shared(
        &mut self,
        prog: &ir::Program,
        id: ir::TypeId,
    ) -> std::rc::Rc<Layout> {
        self.layouts.shared(&prog.type_info(id).ty)
    }

    /// Whether `middle::layout` put `field` behind a pointer inside `owner`,
    /// which it does for the field that would otherwise make the owner
    /// recursive.
    pub(crate) fn boxes(&self, owner: &Ty, field: &Ty) -> bool {
        self.layouts.boxes(owner, field)
    }

    /// Whether a source type owns a counted block anywhere inside it.
    pub(crate) fn rc_counted(&mut self, ty: &Ty) -> bool {
        self.counts.counted(self.tables, &mut self.layouts, ty)
    }

    /// Where a source type's counts live (`backend/counts.rs`).
    pub(crate) fn rc_sites(&mut self, ty: &Ty) -> std::rc::Rc<[Site]> {
        self.counts.sites(self.tables, &mut self.layouts, ty)
    }

    /// How big a walk of one value of this type is (`Counts::weight`).
    pub(crate) fn rc_weight(&mut self, ty: &Ty) -> u32 {
        self.counts.weight(self.tables, &mut self.layouts, ty)
    }

    pub(crate) fn layout_of(&mut self, prog: &ir::Program, id: ir::TypeId) -> std::rc::Rc<Layout> {
        self.layouts.shared(&prog.type_info(id).ty)
    }

    /// The same, for a type reached through another type's arguments rather
    /// than through the program's interner.
    pub(crate) fn layout_of_type(&mut self, ty: Ty) -> std::rc::Rc<Layout> {
        self.layouts.shared(&ty)
    }

    pub(crate) fn width_of(&mut self, prog: &ir::Program, t: ir::Type) -> u32 {
        self.width(prog, t)
    }

    pub(crate) fn slot_bytes_of(&mut self, prog: &ir::Program, t: ir::Type) -> u32 {
        self.slot_bytes(prog, t)
    }

    /// Bytes a value of this IR type occupies where it is *stored inside an
    /// aggregate* — its real width, not its frame slot's.
    fn width(&mut self, prog: &ir::Program, t: ir::Type) -> u32 {
        match t {
            ir::Type::I1 | ir::Type::I8 => 1,
            ir::Type::I16 => 2,
            ir::Type::I32 | ir::Type::F32 => 4,
            ir::Type::I64 | ir::Type::F64 | ir::Type::Ptr => 8,
            ir::Type::I128 => 16,
            ir::Type::Unit => 0,
            ir::Type::Agg(id) => self.layout_of(prog, id.id()).size,
        }
    }

    /// Bytes a value of this IR type occupies in a frame.
    fn slot_bytes(&mut self, prog: &ir::Program, t: ir::Type) -> u32 {
        round8(self.width(prog, t)).max(8)
    }

    /// The per-part tables, sized from the program.
    ///
    /// The frame layouts a call site needs are *not* computed here: they are a
    /// whole-program function of the program alone, so `emit_units` computes
    /// them once and every part borrows the same slice. What is left is the
    /// vector that is genuinely this part's — where each function was laid
    /// out — and it is a `memset`.
    pub fn plan(&mut self, prog: &ir::Program) {
        self.entries = prog.funcs.map(|_| 0);
    }
}

/// Every function's frame layout, from the program and the type tables alone.
///
/// A free function and not a method because it is **whole-program and computed
/// once per emission**, not once per unit: stencil's calling convention is
/// frame-threaded — a call site writes its arguments at
/// `fp + frame_size(caller) + param_off(callee)` — so a caller's bytes depend on
/// its *callee's* `FrameSig` whether or not the two share a unit, and the table
/// cannot be restricted to a unit's own members. `mod.rs::emit_units` calls it
/// beside `lower::run` and hands every `Jit` the same slice; calling it from
/// `Jit::plan` made emission quadratic in the program (see `Jit::frames`).
///
/// `cycles` is the emission's own, which [`Scratch::new`] takes too.
pub(crate) fn frame_sigs(
    prog: &ir::Program,
    tables: &Tables,
    cycles: std::sync::Arc<crate::compiler::middle::layout::Cycles>,
) -> Dense<FuncIdx, FrameSig> {
    let mut layouts = Layouts::with_cycles(tables, cycles);
    // Every value of every function is sized here, so an aggregate's size is
    // looked up by its `TypeId` rather than hashed into `layouts` each time.
    let mut sizes = prog.types.per_type(None);
    let mut width = |t: ir::Type| -> u32 {
        match t {
            ir::Type::I1 | ir::Type::I8 => 1,
            ir::Type::I16 => 2,
            ir::Type::I32 | ir::Type::F32 => 4,
            ir::Type::I64 | ir::Type::F64 | ir::Type::Ptr => 8,
            ir::Type::I128 => 16,
            ir::Type::Unit => 0,
            ir::Type::Agg(agg) => {
                if let Some(w) = *sizes.get(agg.id()) {
                    return w;
                }
                let w = layouts.shared(&prog.type_info(agg.id()).ty).size;
                *sizes.get_mut(agg.id()) = Some(w);
                w
            }
        }
    };
    let mut out = Dense::new();
    for f in prog.funcs.iter() {
        let mut fs = FrameSig::default();
        let mut at = RET + round8(width(f.sig.ret)).max(8);
        fs.ret_size = at;
        for t in &f.sig.params {
            fs.params.push(at);
            at += round8(width(*t)).max(8);
        }
        fs.param_end = at;
        if let ir::Body::Code(code) = &f.body {
            let entry_params = &code.entry().params;
            for v in code.value_ids() {
                if entry_params.contains(&v) {
                    continue;
                }
                at += round8(width(code.ty_of(v))).max(8);
            }
        }
        at += SCRATCH_WORDS as u32 * 8;
        fs.size = (at + 15) & !15;
        out.push(fs);
    }
    out
}

/// Words of scratch past the last local, inside every frame.
///
/// Sixteen for the emitter's own temporaries, then the C argument area a
/// runtime call marshals into and the words past it that the widening and
/// out-of-line-walk sequences take (`rtcall::CARG_WORD` through
/// `rtcall::RESERVED_WORDS`). The C argument area is not optional: a `crt`
/// stencil's arguments have to live **inside** this frame, because the first
/// byte past it is where a Buri callee's frame starts.
pub(crate) const SCRATCH_WORDS: usize = super::rtcall::RESERVED_WORDS as usize;

// ---------------------------------------------------------------------------
// Emission
// ---------------------------------------------------------------------------

/// A place in the body being emitted that a branch can name: a block of its
/// code, or a synthetic label [`Fn2::label`] handed out.
#[derive(Clone, Copy, Debug)]
pub struct Label(u32);

impl Label {
    /// The label of block `b`, which [`Labels::for_code`] gives a row.
    pub fn of(b: ir::BlockId) -> Label {
        Label(b.index() as u32)
    }
}

/// Where each [`Label`] of one body was laid out.
///
/// A row per block of the code first, from [`Labels::for_code`], then one per
/// label [`Labels::mint`] hands out. A body with no code (a runtime body, a
/// generated helper) names no block, so every label it reads is one it minted.
#[derive(Default)]
pub(crate) struct Labels(Vec<u64>);

#[expect(
    clippy::indexing_slicing,
    reason = "a label is a block of the code the table was sized for, or one `mint` pushed"
)]
impl Labels {
    /// A row per block of `code`, in `spare`'s storage.
    fn for_code(spare: &mut Labels, code: &ir::Code) -> Labels {
        let mut rows = std::mem::take(&mut spare.0);
        rows.clear();
        rows.resize(code.blocks().len(), 0);
        Labels(rows)
    }

    fn mint(&mut self) -> Label {
        self.0.push(0);
        Label((self.0.len() - 1) as u32)
    }

    fn place(&mut self, l: Label, at: u64) {
        self.0[l.0 as usize] = at;
    }

    fn at(&self, l: Label) -> u64 {
        self.0[l.0 as usize]
    }
}

/// Per-function emission state.
pub(crate) struct Fn2 {
    /// Byte offset of every value's frame slot.
    pub slot: ir::PerValue<u32>,
    /// The address each block and each synthetic label was laid out at.
    pub blk: Labels,
    pub frame: FrameSig,
    pub scratch: u32,
    /// The CPS register the level assigned a value, if any.
    pub reg: ir::PerValue<Option<Reg>>,
    /// Whether a value is a `Const` whose every use is an immediate operand,
    /// so that it is never materialised at all: it lives at [`Loc::Imm`].
    pub imm: ir::PerValue<bool>,
    /// For a value promoted across block boundaries: whether its frame slot has
    /// to be kept in step with its register, because something that cannot read
    /// a register reads it.
    pub wt: ir::PerValue<bool>,
    /// Which values [`Jit::promote`] put in a register. Their register holds
    /// the value inside [`Fn2::region`] and nowhere else.
    pub cross: ir::PerValue<bool>,
    /// The blocks of [`Jit::promote`]'s region.
    pub region: ir::PerBlock<bool>,
    /// The block being emitted.
    pub cur: ir::BlockId,
    /// The literal a value holds, when it is an `Inst::Const` a stencil can
    /// take as an immediate.
    pub constants: ir::PerValue<Option<u64>>,
    /// How many times each value is read, anywhere in the function.
    pub uses: ir::PerValue<u32>,
}

impl Default for Fn2 {
    fn default() -> Fn2 {
        Fn2::bare(FrameSig::default(), 0)
    }
}

impl Fn2 {
    /// The state of a body with no code: no values, no blocks, and `frame`
    /// with its scratch from `scratch`.
    pub fn bare(frame: FrameSig, scratch: u32) -> Fn2 {
        Fn2 {
            slot: ir::PerValue::default(),
            blk: Labels::default(),
            frame,
            scratch,
            reg: ir::PerValue::default(),
            imm: ir::PerValue::default(),
            wt: ir::PerValue::default(),
            cross: ir::PerValue::default(),
            region: ir::PerBlock::default(),
            cur: ir::BlockId::ENTRY,
            constants: ir::PerValue::default(),
            uses: ir::PerValue::default(),
        }
    }

    pub fn label(&mut self) -> Label {
        self.blk.mint()
    }

    pub fn place(&mut self, l: Label, at: u64) {
        self.blk.place(l, at);
    }

    /// The byte offset of `v`'s frame slot.
    pub fn at(&self, v: ir::ValueId) -> u32 {
        *self.slot.get(v)
    }
    /// Where a value **lives**: a CPS register if one was assigned, the frame
    /// otherwise. This is the paper's "whether it operates on constants,
    /// registers, or stack locations".
    ///
    /// The home is where a definition writes and where an edge copy lands, and
    /// it is the same answer everywhere in the function. Where a *read* takes
    /// the value from is [`Fn2::loc`], which is not.
    pub fn home(&self, v: ir::ValueId) -> Loc {
        if self.is_imm(v) {
            return Loc::Imm;
        }
        self.reg_of(v).map_or(Loc::Frame, Loc::Reg)
    }

    /// Where a read in the block being emitted takes a value from.
    pub fn loc(&self, v: ir::ValueId) -> Loc {
        if self.is_imm(v) {
            return Loc::Imm;
        }
        self.read_reg(v).map_or(Loc::Frame, Loc::Reg)
    }

    /// The register a read in the block being emitted takes a value from, or
    /// `None` for its frame slot. A read that has no immediate form, such as a
    /// branch's condition, asks this: [`Jit::constants`] makes a value an
    /// immediate only when every read of it has one.
    ///
    /// [`Jit::promote`]'s register is only the value inside its region: the
    /// region is barrier-free and enterable only at its header, and neither
    /// holds one block further out. So a read outside it takes the frame slot,
    /// which `wt` had the edge keep in step for exactly this — the other half
    /// of the promotion, and the half that was missing. Without it a
    /// comparison one block past the loop read a register a call had already
    /// overwritten, and a `Bool` walk that recursed from two places answered
    /// `true` for every input (buri-lang/buri#47).
    pub fn read_reg(&self, v: ir::ValueId) -> Option<Reg> {
        if *self.cross.get(v) && !*self.region.get(self.cur) {
            return None;
        }
        self.reg_of(v)
    }

    fn reg_of(&self, v: ir::ValueId) -> Option<Reg> {
        *self.reg.get(v)
    }

    fn is_imm(&self, v: ir::ValueId) -> bool {
        *self.imm.get(v)
    }
}

/// A node of [`Dominance`]'s graph: a block, or the virtual root that jumps
/// to every way in.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
enum Node {
    Root,
    Block(ir::BlockId),
}

/// A row per block of one code, and one for [`Node::Root`].
struct PerNode<T> {
    root: T,
    blocks: ir::PerBlock<T>,
}

impl<T: Default> Default for PerNode<T> {
    fn default() -> PerNode<T> {
        PerNode {
            root: T::default(),
            blocks: ir::PerBlock::default(),
        }
    }
}

impl<T: Clone> PerNode<T> {
    /// Every row `init`, a row per block of `code`, in this table's storage.
    fn refill(&mut self, code: &ir::Code, init: T) {
        self.blocks = code.per_block_in(&mut self.blocks, init.clone());
        self.root = init;
    }

    fn get(&self, n: Node) -> &T {
        match n {
            Node::Root => &self.root,
            Node::Block(b) => self.blocks.get(b),
        }
    }

    fn get_mut(&mut self, n: Node) -> &mut T {
        match n {
            Node::Root => &mut self.root,
            Node::Block(b) => self.blocks.get_mut(b),
        }
    }

    /// Every row, in [`Node`] order: the root's, then each block's.
    fn rows_mut(&mut self) -> impl Iterator<Item = &mut T> {
        std::iter::once(&mut self.root).chain(self.blocks.iter_mut().map(|(_, t)| t))
    }
}

/// One list per node, in two flat tables rather than a `Vec` each.
/// [`Jit::promote`] builds three of these per function with a loop in it, and
/// a `Vec` per block was most of its time.
struct Lists<I> {
    /// Where each node's list starts in `items`, and where it ends.
    span: PerNode<(u32, u32)>,
    items: Vec<I>,
    /// The pairs, sorted by node, while filling.
    pairs: Vec<(Node, I)>,
}

impl<I> Default for Lists<I> {
    fn default() -> Lists<I> {
        Lists {
            span: PerNode::default(),
            items: Vec::new(),
            pairs: Vec::new(),
        }
    }
}

impl<I: Copy> Lists<I> {
    /// A list per node of `code`, from `(node, item)` pairs, each list in the
    /// order its pairs come.
    fn fill(&mut self, code: &ir::Code, pairs: impl Iterator<Item = (Node, I)>) {
        self.pairs.clear();
        self.pairs.extend(pairs);
        // Stable, so each list keeps its pairs' order.
        self.pairs.sort_by_key(|(n, _)| *n);
        self.span.refill(code, (0, 0));
        for (n, _) in &self.pairs {
            self.span.get_mut(*n).1 += 1;
        }
        // `rows_mut` walks the nodes in the order the pairs are sorted in.
        let mut sum = 0u32;
        for s in self.span.rows_mut() {
            let n = s.1;
            *s = (sum, sum + n);
            sum += n;
        }
        self.items.clear();
        self.items.extend(self.pairs.iter().map(|(_, x)| *x));
    }

    fn of(&self, n: Node) -> &[I] {
        let (lo, hi) = *self.span.get(n);
        self.items.get(lo as usize..hi as usize).unwrap_or_default()
    }
}

/// What [`Jit::promote`] looks for its loop with, kept from one function to
/// the next.
#[derive(Default)]
struct Loops {
    preds: Lists<ir::BlockId>,
    dom: Dominance,
    barrier: ir::PerBlock<bool>,
    stamp: ir::PerBlock<u32>,
    members: Vec<ir::BlockId>,
    stack: Vec<ir::BlockId>,
    /// The innermost loop's blocks, once [`Loops::innermost`] has found one.
    best: Vec<ir::BlockId>,
}

impl Loops {
    /// The header of the innermost promotable loop, with its blocks in
    /// [`Loops::best`].
    fn innermost(&mut self, code: &ir::Code) -> Option<ir::BlockId> {
        let Loops {
            preds,
            dom,
            barrier,
            stamp,
            members,
            stack,
            best,
        } = self;
        *barrier = code.per_block_in(barrier, false);
        for (id, x) in barrier.iter_mut() {
            *x = code.get(id).insts.iter().any(is_barrier);
        }
        preds.fill(
            code,
            code.iter()
                .flat_map(|(id, b)| b.term.targets().map(move |t| (Node::Block(t.block), id))),
        );
        // The innermost promotable loop. A back edge is any edge whose target
        // is not after its source in layout order; the loop it makes is the
        // blocks that reach the source without passing the header, and it is a
        // real, *reducible* loop exactly when every predecessor of every block
        // in that set is in it too. That test is what makes the region safe
        // without a dominator tree: control cannot be inside the loop without
        // having come through the header, so a register filled at the header is
        // filled everywhere in it.
        //
        // The dominator tree is still worth having, as a filter: a candidate
        // whose header does not dominate its source is one the walk below
        // would abandon, and it would have walked back to the entry to find
        // that out. `lower` puts a `match`'s join block ahead of its arms, so
        // every arm of a wide `match` is such a candidate, and walking each of
        // them was quadratic in the width.
        dom.compute(code, preds);
        // One side table for every candidate, told apart by a stamp, and the
        // members listed beside it: a table per candidate was the other half
        // of the quadratic.
        *stamp = code.per_block_in(stamp, 0);
        let mut epoch = 0u32;
        let mut found: Option<ir::BlockId> = None;
        for (p, b) in code.iter() {
            for t in b.term.targets() {
                let h = t.block;
                if h > p {
                    continue;
                }
                if code.get(h).params.is_empty() {
                    continue;
                }
                if !dom.may_loop(h, p) {
                    continue;
                }
                // `h` dominates `p`, so the walk back from `p` meets the header
                // before it could meet the entry, and every predecessor of a
                // block it takes in is taken in too: the loop is closed by
                // construction. No block branches to itself (`ir::Code`), so
                // `p` is not `h`.
                epoch += 1;
                let inside = |stamp: &ir::PerBlock<u32>, x: ir::BlockId| *stamp.get(x) == epoch;
                members.clear();
                *stamp.get_mut(h) = epoch;
                *stamp.get_mut(p) = epoch;
                members.extend([h, p]);
                stack.clear();
                stack.push(p);
                while let Some(x) = stack.pop() {
                    for q in preds.of(Node::Block(x)).iter().copied() {
                        if !inside(stamp, q) {
                            *stamp.get_mut(q) = epoch;
                            members.push(q);
                            stack.push(q);
                        }
                    }
                }
                if members.iter().any(|x| *barrier.get(*x)) {
                    continue;
                }
                if found.is_none() || members.len() < best.len() {
                    best.clone_from(members);
                    found = Some(t.block);
                }
            }
        }
        found
    }
}

impl<'a> Jit<'a> {
    /// One **part** of a codegen unit: the members' bodies, back to back, the
    /// glue they asked for behind them, and the function-local branches
    /// resolved.
    ///
    /// `members` is a contiguous run of the unit's functions in ascending
    /// index, which is the order `ir::Program::funcs_by_unit` yields and
    /// therefore the order the object's symbols come out in. The whole unit is
    /// the parts' regions concatenated in part order
    /// ([`super::region::Emitted::append`]), so the symbols come out in the
    /// order a single-threaded emission would have produced. Deterministic
    /// emission is a cache requirement, not a nicety: `--check-reproducible`
    /// compares two builds byte for byte.
    ///
    /// **Nothing here reads the region's base**, which is what makes a part
    /// movable. A function's own branches are resolved before the next function
    /// is laid out and are relative; every other address this backend emits is
    /// a relocation.
    pub fn compile_part(&mut self, prog: &ir::Program, members: &[FuncIdx]) {
        self.plan(prog);
        for i in members {
            self.function(prog, *i);
        }
        // The helpers, after the bodies that asked for them. An index walk
        // rather than an iterator because emitting one may register another —
        // the drop glue of a `[[Str]]` block needs the drop glue of a `[Str]` —
        // and the list grows underneath the loop.
        let mut i = 0usize;
        while i < self.helpers.len() {
            let h = match self.helpers.get(i) {
                Some(h) => h.clone(),
                None => break,
            };
            let at = self.emit_helper(prog, &h);
            self.helper_at.push((at, self.region.code_addr()));
            i += 1;
        }
        self.resolve(prog);
    }

    fn function(&mut self, prog: &ir::Program, fi: FuncIdx) {
        self.region.align_code(4);
        let f = prog.funcs.get(fi);
        let entry = self.region.code_addr();
        *self.entries.get_mut(fi) = entry;
        let frame = self.frame_sig_of(fi);

        match &f.body {
            ir::Body::Code(code) => {
                // The last function's tables, refilled: every field below is
                // cleared and written before anything reads it.
                let mut st = std::mem::take(&mut self.bufs.st);
                st.frame.clone_from(frame);
                // How often each value is read, which five of the analyses
                // below ask and used to count for themselves.
                let mut opnd = std::mem::take(&mut self.bufs.opnd);
                opnd.fill(code);
                st.uses = count_uses(code, &opnd, &mut st.uses);
                let order = self.layout(code);
                st.scratch = self.slots(prog, code, frame, &st.uses, &opnd, &order, &mut st.slot);
                st.reg = code.per_value_in(&mut st.reg, None);
                st.wt = code.per_value_in(&mut st.wt, true);
                st.cross = code.per_value_in(&mut st.cross, false);
                st.region = code.per_block_in(&mut st.region, false);
                let taken =
                    self.promote(code, &mut st.reg, &mut st.wt, &mut st.cross, &mut st.region);
                self.regalloc(code, &mut st.reg, taken, &st.uses, &opnd);
                self.bufs.opnd = opnd;
                self.constants(code, &st.uses, &mut st.constants, &mut st.imm);
                st.blk = Labels::for_code(&mut st.blk, code);
                st.cur = ir::BlockId::ENTRY;
                let base = self.fixups.len();
                for (oi, id) in order.iter().copied().enumerate() {
                    let here = self.region.code_addr();
                    st.place(Label::of(id), here);
                    // Which side of the promotion's region every read in this
                    // block is on. See [`Fn2::loc`].
                    st.cur = id;
                    let block = code.get(id);
                    let plan = self.plan_block(code, &st, block);
                    // `Plan::skip` is built with one flag per instruction of
                    // this block, so the zip drops nothing.
                    for (inst, skip) in block.insts.iter().zip(plan.skip.iter()) {
                        if *skip {
                            continue;
                        }
                        self.inst(prog, code, &mut st, inst);
                    }
                    let next = order.get(oi + 1).copied();
                    // An `Abort` does not come back, so the terminator behind
                    // one is dead code.
                    if !matches!(block.insts.last(), Some(ir::Inst::Abort { .. })) {
                        self.term(prog, code, &mut st, next, &block.term, &plan);
                    }
                    self.bufs.skip = plan.skip;
                }
                self.resolve_blocks(base, &st.blk);
                self.bufs.order = order;
                self.bufs.st = st;
            }
            // A runtime-supplied body has no IR to walk, but its sequence may
            // still branch, which needs labels and the same function-local
            // branch resolution a real body gets. So it is given an empty
            // `Fn2` whose frame is its own signature's and whose scratch is the
            // area `Jit::plan` reserved past the parameters.
            ir::Body::Runtime(r) => {
                let mut st = Fn2::bare(frame.clone(), frame.param_end);
                let base = self.fixups.len();
                let route = *self.routes.bodies.get(r.key);
                self.runtime_body(prog, fi, (route, r.answer), &mut st);
                self.resolve_blocks(base, &st.blk);
            }
        }
    }

    /// Where the fixup list stands, so that a generated body can resolve its
    /// own labels without seeing the ones a member left behind.
    pub(crate) fn fixups_len(&self) -> usize {
        self.fixups.len()
    }

    /// [`Jit::resolve_blocks`] for one of `glue.rs`'s generated bodies.
    pub(crate) fn resolve_helper_blocks(&mut self, base: usize, st: &Fn2) {
        self.resolve_blocks(base, &st.blk);
    }

    /// Block references are function-local, so they are resolved as soon as
    /// the function is laid out — and only then may a veneer be planted, since
    /// one in the middle of a function would land inside the fallthrough of the
    /// stencil being patched.
    fn resolve_blocks(&mut self, base: usize, blk: &Labels) {
        self.veneer_ok = true;
        let mut i = base;
        while let Some(fix) = self.fixups.get(i).copied() {
            match fix {
                Fix::Block { at, blk: b } => {
                    self.patch_branch(at, blk.at(b));
                    self.fixups.swap_remove(i);
                }
                Fix::BlockCond { at, blk: b } => {
                    self.patch_cond(at, blk.at(b));
                    self.fixups.swap_remove(i);
                }
                Fix::Func { .. } => i += 1,
            }
        }
        self.veneer_ok = false;
    }

    // -- the stencil primitive ---------------------------------------------

    /// Copy, and patch. The whole of it.
    ///
    /// `self.lib` is a `&'a Library`, so the stencil reference this reads out
    /// lives independently of the `&mut self` the region needs: no stencil is
    /// ever copied out of the library, which matters because a clone per
    /// instruction would be most of this compiler's running time.
    pub(crate) fn emit<'k>(&mut self, key: impl Into<KeyArg<'k>>, binds: &[Bind]) {
        self.emit_key(key.into(), binds);
    }

    /// [`Jit::emit`], once for every spelling of a key.
    fn emit_key(&mut self, key: KeyArg<'_>, binds: &[Bind]) {
        let lib: &'static Library = self.lib;
        let found = self.index_of(key).and_then(|i| Some((i, lib.stencil(i)?)));
        let (at, mut s) = match found {
            Some(found) => found,
            None => crate::diagnostics::ice(&format!(
                "stencil: no stencil {} in {}",
                match key {
                    KeyArg::Literal(k) | KeyArg::Spelled(k) => k.to_string(),
                    KeyArg::At(i) => format!("#{i}"),
                },
                lib.config
            )),
        };
        let key = s.name.as_str();
        // The folded twins, most specific first: every offset or literal a twin
        // would put in an `imm12` field has to be a multiple of the field's
        // scale and inside its reach, and the two folds are independent, so a
        // stencil can have both, either or neither.
        //
        // Asked by index rather than by name: `Library::fold_twin` resolved
        // `key+ifold+fold`, `key+fold` and `key+ifold` once for the whole
        // library, and this is the loop that used to build those three names
        // with `format!` per emitted instruction.
        for k in 0..super::library::FOLD_SUFFIXES_LEN {
            let Some(f) = lib.fold_twin(at, k) else {
                continue;
            };
            let fits = f.holes.iter().all(|h| {
                h.lo12.is_empty()
                    || bind_of(binds, h).and_then(Bind::literal).is_some_and(|v| {
                        h.lo12
                            .iter()
                            .all(|(_, sc)| v % u64::from(*sc) == 0 && v / u64::from(*sc) < 4096)
                    })
            });
            if fits {
                s = f;
                break;
            }
        }
        // The hole on the stencil's last instruction, once the fold above has
        // settled which twin is being copied.
        let tail = s.tail.and_then(|t| s.holes.get(t));
        let mut len = s.code.len();
        // Fallthrough elision: the continuation is the next stencil, so the
        // trailing branch is not patched, it is dropped. One A64 instruction,
        // or the five bytes of an x86-64 `jmp rel32`.
        let tail_bytes = if self.target.is_arm64() { 4 } else { 5 };
        let mut elide = false;
        if let Some(Bind::Jump(_, V::Fall)) = tail.and_then(|h| bind_of(binds, h)) {
            elide = true;
            len = len.saturating_sub(tail_bytes);
        }
        let (bytes, _) = s.code.split_at(len);
        let at = self.region.put(bytes);
        let end = at + len as u64;
        // The read-only bytes clang spilled, which are not holes and take no
        // value: they go into this unit's pool and the references are aimed at
        // the copy. Only x86-64 has any (`library::ConstRef`).
        if !s.const_refs.is_empty() {
            let base = self.spilled_pool(s);
            for c in &s.const_refs {
                self.region.pool_ref_pc32(
                    at + u64::from(c.field),
                    at + u64::from(c.insn_end),
                    base + u64::from(c.at),
                );
            }
        }
        for (i, h) in s.holes.iter().enumerate() {
            if elide && s.tail == Some(i) {
                continue;
            }
            let Some(b) = bind_of(binds, h) else {
                match &h.name {
                    // A symbol the stencil body reached on its own — an abort,
                    // `buri_rt_free`, `memcpy` — becomes an import. The
                    // generated C declares nothing undefined but the `_JIT_*`
                    // holes and `cli/runtime/lib.rs`'s exports, so the name
                    // *is* the symbol; `EXTERNALS` records the set for a test
                    // to check rather than for this to consult.
                    HoleName::Symbol(sym) => self.import(at, h, sym),
                    // Which slots a stencil has is the library's, and which
                    // ones a call site binds is the emitter's: only a type per
                    // stencil would tie the two together.
                    HoleName::Slot(slot) => crate::diagnostics::ice(&format!(
                        "stencil: stencil {key} has an unbound hole {slot}"
                    )),
                }
                continue;
            };
            self.patch_hole(at, end, h, b);
        }
    }

    /// Where this stencil's spilled constants begin in the unit's pool,
    /// copying them the first time the stencil is emitted.
    fn spilled_pool(&mut self, s: &Stencil) -> u64 {
        if let Some(at) = self.spilled.get(&s.name) {
            return *at;
        }
        let at = self.region.pool_const(&s.consts, s.consts_align);
        self.spilled.insert(s.name.clone(), at);
        at
    }

    /// A hole naming a symbol outside this program: one relocation per site,
    /// and no instruction rewritten. A `bl` becomes a `BRANCH26`. Nothing
    /// takes such a symbol's address: `sources.rs` refuses a library that does.
    fn import(&mut self, at: u64, h: &Hole, sym: &'static str) {
        for off in &h.branches {
            self.branch_reloc(at + *off as u64, Target::Symbol(sym.into()));
        }
    }

    /// The relocation a call or jump to a symbol outside this unit takes.
    ///
    /// A `b`/`bl`'s 26-bit word displacement on A64; a `rel32` on x86-64,
    /// whose addend is `-4` because the field is the last four bytes of its
    /// instruction and the processor measures from the instruction's end.
    fn branch_reloc(&mut self, at: u64, target: Target) {
        if self.target.is_arm64() {
            self.region.reloc(at, RelKind::Branch26, target);
            return;
        }
        self.region.reloc_with(at, RelKind::Rel32, target, -4);
    }

    /// Patch hole `h` of the copy at `at` with `b`. `sources.rs` refused, when
    /// the library was built, any hole whose relocation is not the one its
    /// slot's class is declared with, so the bind's class is the hole's kind.
    fn patch_hole(&mut self, at: u64, end: u64, h: &Hole, b: &Bind) {
        match b {
            Bind::Jump(_, v) => match *v {
                // A call out of the program: `bl` a symbol the linker
                // resolves. The prototype planted a veneer holding the
                // address of a function in its own process; there is no
                // such address here, and a relocation is both simpler and
                // one instruction shorter.
                V::Ext(n) => {
                    for off in &h.branches {
                        self.branch_reloc(at + *off as u64, Target::Symbol(n.into()));
                    }
                }
                V::Sym(ref n) => {
                    for off in &h.branches {
                        self.branch_reloc(at + *off as u64, Target::Symbol(n.clone().into()));
                    }
                }
                // The stencil laid out immediately after this one, which
                // is the address one past this body — *not* one past this
                // branch, which for a two-target stencil is the other arm.
                V::Fall => {
                    for off in &h.branches {
                        self.patch_branch(at + *off as u64, end);
                    }
                    for off in &h.conds {
                        self.patch_cond(at + *off as u64, end);
                    }
                }
                V::Blk(b) => {
                    for off in &h.branches {
                        self.fixups.push(Fix::Block {
                            at: at + *off as u64,
                            blk: b,
                        });
                    }
                    for off in &h.conds {
                        self.fixups.push(Fix::BlockCond {
                            at: at + *off as u64,
                            blk: b,
                        });
                    }
                }
                V::Fn(f) => {
                    for off in &h.branches {
                        self.fixups.push(Fix::Func {
                            at: at + *off as u64,
                            f,
                        });
                    }
                }
            },
            Bind::Lit(_, x) => {
                let x = *x;
                if !self.target.is_arm64() {
                    // One instruction rewritten in place, where A64 rewrites a
                    // pair. `lo12` is an A64 fold's output and is always empty
                    // here (`x86.rs` §"why there are no folds here").
                    for (insn_end, field) in &h.pairs {
                        self.patch_pc32_imm(at + *field as u64, at + *insn_end as u64, x as u32);
                    }
                    return;
                }
                for (a, b) in &h.pairs {
                    self.patch_imm32(at + *a as u64, at + *b as u64, x as u32);
                }
                for (o, scale) in &h.lo12 {
                    self.patch_lo12(at + *o as u64, x as u32, *scale);
                }
            }
            Bind::Word(_, d) => {
                if !self.target.is_arm64() {
                    self.patch_imm64_x86_64(at, h, d);
                    return;
                }
                let slot = match *d {
                    Datum::I(x) => {
                        // The immediate fold may have taken this hole into an
                        // `imm12` field, in which case there is no pair left
                        // to relax. `emit` copies a folded twin only when its
                        // `imm12` holes are bound to literals.
                        for (o, scale) in &h.lo12 {
                            self.patch_lo12(at + *o as u64, x as u32, *scale);
                        }
                        if h.pairs.is_empty() {
                            return;
                        }
                        // A value that fits 32 bits does not need the pool at
                        // all: the GOT form is two instructions with one
                        // destination register, and so is `movz`/`movk`. This
                        // is the same relaxation a linker does, and it takes a
                        // load off every immediate operand.
                        // `sources::one_register_pairs` refused, when the
                        // library was built, any pair that names a second
                        // register.
                        if x < (1u64 << 32) {
                            for (a, b) in &h.pairs {
                                self.patch_imm32(at + *a as u64, at + *b as u64, x as u32);
                            }
                            return;
                        }
                        self.region.pool_u64(x)
                    }
                    _ => self.pool_datum(d),
                };
                for (a, b) in &h.pairs {
                    self.region.pool_ref(at + *a as u64, at + *b as u64, slot);
                }
            }
        }
    }

    /// [`Jit::patch_hole`]'s [`HoleKind::Imm64`] arm, for x86-64.
    ///
    /// A default-visibility hole compiles to `mov rD, sym@GOTPCREL(%rip)`, and
    /// the patch is to aim its `disp32` at this unit's constant pool — one
    /// relocation, where A64 needs the two halves of a GOT `adrp`/`ldr` pair.
    fn patch_imm64_x86_64(&mut self, at: u64, h: &Hole, d: &Datum) {
        // The same relaxation A64 takes, and for the same reason: a value that
        // fits 32 bits does not need the pool, and dropping the load takes an
        // L1 access off every immediate operand. It is only sound where the
        // instruction is a plain `mov rD, [rip+disp32]` — a float immediate
        // arrives as `movsd` and a small comparison as `cmpl $0, …`, and
        // neither may become a `mov` of a literal.
        if let Datum::I(x) = *d {
            if x < (1u64 << 32)
                && h.pairs
                    .iter()
                    .all(|(e, f)| self.is_plain_rip_mov(at + *f as u64, at + *e as u64))
            {
                for (insn_end, field) in &h.pairs {
                    self.patch_pc32_imm(at + *field as u64, at + *insn_end as u64, x as u32);
                }
                return;
            }
        }
        let slot = self.pool_datum(d);
        // On x86-64 a pair is `(instruction end, field)`.
        for (insn_end, field) in &h.pairs {
            self.region
                .pool_ref_pc32(at + u64::from(*field), at + u64::from(*insn_end), slot);
        }
    }

    /// The constant-pool slot holding `d`.
    fn pool_datum(&mut self, d: &Datum) -> u64 {
        match *d {
            Datum::I(x) => self.region.pool_u64(x),
            // A byte inside this section, whose base the linker picks.
            Datum::Ptr(x) => self.region.pool_target(Target::Here(x)),
            Datum::Ext(n) => self.region.pool_target(Target::Symbol(n.into())),
            Datum::Sym(ref n) => self.region.pool_target(Target::Symbol(n.clone().into())),
        }
    }

    /// Whether the seven bytes ending at `insn_end` are `REX.W 8b ModRM
    /// disp32` with the rip-relative addressing mode — the one shape
    /// [`Jit::patch_pc32_imm`] may rewrite into a `mov` of a literal.
    fn is_plain_rip_mov(&self, field: u64, insn_end: u64) -> bool {
        if field < 3 || insn_end != field + 4 {
            return false;
        }
        let start = field - 3;
        self.region.byte_at(start, 0) & 0xf8 == 0x48
            && self.region.byte_at(start, 1) == 0x8b
            && self.region.byte_at(start, 2) & 0xc7 == 0x05
    }

    /// `lea rD, [rip+disp32]` — or the `mov` above — into a `mov` of the
    /// literal, in the seven bytes it already occupies. See
    /// `library::HoleKind`.
    ///
    /// `REX.W C7 /0 id` is seven bytes and sign-extends, so it is the whole
    /// instruction for any value below `2^31`. Above that the value would come
    /// out negative, and the five-byte zero-extending `mov rD32, imm32` plus
    /// `nop` is written instead — still one instruction and no memory
    /// reference, which is the property the rewrite exists for.
    fn patch_pc32_imm(&mut self, field: u64, insn_end: u64, v: u32) {
        debug_assert_eq!(insn_end, field + 4);
        // Seven bytes end at `insn_end`, and every such hole of a shipped
        // library has them inside its stencil
        // (`the_shipped_patch_sites_are_inside_their_stencils`).
        let start = insn_end.saturating_sub(7);
        // `ModRM.reg` and `REX.R` are where the destination register is, in
        // both the `lea` and the `mov` this rewrites.
        let rex = self.region.byte_at(start, 0);
        let modrm = self.region.byte_at(start, 2);
        let rd = ((u32::from(rex) & 4) << 1) | ((u32::from(modrm) >> 3) & 7);
        let mut out: Vec<u8> = Vec::with_capacity(7);
        if v < 0x8000_0000 {
            out.push(0x48 | ((rd >> 3) & 1) as u8);
            out.push(0xc7);
            out.push(0xc0 | (rd & 7) as u8);
            out.extend_from_slice(&v.to_le_bytes());
        } else {
            if rd >= 8 {
                out.push(0x41);
            }
            out.push(0xb8 + (rd & 7) as u8);
            out.extend_from_slice(&v.to_le_bytes());
            // `66 90` and `90` are the canonical two- and one-byte nops.
            if out.len() == 5 {
                out.push(0x66);
            }
            out.push(0x90);
        }
        debug_assert_eq!(out.len(), 7);
        self.region.set_bytes(start, &out);
    }

    /// `b`/`bl`: a signed 26-bit word displacement. Everything generated lives
    /// in one region, so this always reaches. On x86-64 it is the `rel32` of a
    /// `jmp` or a `call`, which is [`Jit::patch_rel32`].
    fn patch_branch(&mut self, at: u64, target: u64) {
        if !self.target.is_arm64() {
            self.patch_rel32(at, target);
            return;
        }
        let d = target as i64 - at as i64;
        assert!(d % 4 == 0, "misaligned branch");
        let w = d >> 2;
        assert!(
            (-(1 << 25)..(1 << 25)).contains(&w),
            "branch out of range: {d}"
        );
        let old = self.region.word_at(at);
        self.region
            .set_word(at, (old & 0xfc00_0000) | (w as u32 & 0x03ff_ffff));
    }

    /// The `imm19` of a `b.cc`/`cbz`/`cbnz` the cond fold made a hole. ±1 MB,
    /// which is three orders of magnitude more than the largest function this
    /// JIT emits; the assertion says so rather than assuming it.
    fn patch_cond(&mut self, at: u64, target: u64) {
        // On x86-64 a conditional displacement is 32 bits, so it is the same
        // arithmetic as an unconditional one and there is nothing to veneer.
        if !self.target.is_arm64() {
            self.patch_rel32(at, target);
            return;
        }
        let mut target = target;
        if !(-(1 << 20)..(1 << 20)).contains(&(target as i64 - at as i64)) {
            assert!(
                self.veneer_ok,
                "a conditional-branch hole reached a target {} bytes away while the \
                 function was still being emitted; bind the far arm to the stencil's \
                 unconditional branch instead (see `Jit::arm_key`)",
                target as i64 - at as i64
            );
            // Out of the 19-bit field's ±1 MB: put a `b` to the real target at
            // the end of what has been emitted, which is inside the same
            // function's neighbourhood, and branch to that instead.
            self.region.align_code(4);
            let v = self.region.code_addr();
            self.region.put(&0x1400_0000u32.to_le_bytes());
            self.patch_branch(v, target);
            target = v;
        }
        let d = target as i64 - at as i64;
        debug_assert!(d % 4 == 0, "misaligned conditional branch");
        let w = d >> 2;
        assert!(
            (-(1 << 18)..(1 << 18)).contains(&w),
            "conditional branch out of imm19 range: {d} bytes"
        );
        let old = self.region.word_at(at);
        self.region
            .set_word(at, (old & 0xff00_001f) | (((w as u32) & 0x7ffff) << 5));
    }

    /// A `rel32`, measured from the end of its instruction — which is the four
    /// bytes of the field itself, for a `jmp`, a `call` and a `jcc` alike.
    ///
    /// The field cannot overflow for anything this emitter produces: a unit's
    /// code section is bounded by the program it was lowered from, and 2 GiB
    /// of it is not a section a linker would accept either.
    fn patch_rel32(&mut self, at: u64, target: u64) {
        let d = target as i64 - (at as i64 + 4);
        assert!(
            (i64::from(i32::MIN)..=i64::from(i32::MAX)).contains(&d),
            "rel32 out of range: {d} bytes"
        );
        self.region.set_word(at, d as i32 as u32);
    }

    /// `adrp Xd, sym@PAGE` + `add Xd, Xd, sym@PAGEOFF` → `movz Xd, #lo` +
    /// `movk Xd, #hi, lsl 16`. See `library::HoleKind`.
    fn patch_imm32(&mut self, adrp: u64, add: u64, v: u32) {
        let wa = self.region.word_at(adrp);
        let wb = self.region.word_at(add);
        let rd = wa & 0x1f;
        debug_assert_eq!(wb & 0x1f, rd);
        self.region
            .set_word(adrp, 0xd280_0000 | ((v & 0xffff) << 5) | rd);
        self.region
            .set_word(add, 0xf2a0_0000 | (((v >> 16) & 0xffff) << 5) | rd);
    }

    /// The `imm12` field of a load or store the library builder folded the hole
    /// into. See `extract::fold_addressing`.
    fn patch_lo12(&mut self, at: u64, v: u32, scale: u32) {
        debug_assert_eq!(v % scale, 0);
        let imm12 = v / scale;
        debug_assert!(imm12 < 4096);
        let w = self.region.word_at(at);
        self.region
            .set_word(at, (w & !(0xfff << 10)) | (imm12 << 10));
    }

    /// The cross-function sites, once the unit is laid out.
    ///
    /// **Every** call to a function is a relocation against its symbol, whether
    /// or not this unit owns the callee. `ir::Func::symbol` is the name both
    /// sides agree on — `ir.rs` §"a callee is named by its symbol" is the
    /// reason it exists.
    ///
    /// The intra-unit case is *not* an optimisation opportunity, and baking the
    /// displacement there is unsound. `object.rs` sets
    /// `MH_SUBSECTIONS_VIA_SYMBOLS`, which tells `ld64` that every symbol
    /// begins an independently movable atom, and `build/link.rs` passes
    /// `-Wl,-dead_strip` on every macOS link. A baked `bl` is not a reference,
    /// so nothing reaches the callee's atom, so the linker moves it and then
    /// deletes it and the branch lands on whatever took its place. This is what
    /// an assembler emits for a call to a symbol in the same file, and what
    /// any object writer emits for the same edge; the linker resolves an
    /// intra-section `BRANCH26` to the same instruction the bake would have
    /// produced, so it costs nothing and it keeps the atom alive.
    fn resolve(&mut self, prog: &ir::Program) {
        let fixups = std::mem::take(&mut self.fixups);
        for f in fixups {
            let (at, callee) = match f {
                Fix::Func { at, f } => (at, f),
                // Block fixups are resolved per function, in `resolve_blocks`.
                Fix::Block { .. } | Fix::BlockCond { .. } => continue,
            };
            let name = symbol_of(prog, callee);
            self.branch_reloc(at, Target::Symbol(String::from(name).into()));
        }
    }

    /// Where `f` was emitted inside this **part's** region. `plan` gives every
    /// function of the program an entry — zero for one this part does not own,
    /// which is also what a `FuncIdx` from outside the program would read.
    /// `mod.rs::assemble_unit` adds the part's base to make it the unit's.
    pub fn entry_of(&self, f: FuncIdx) -> u64 {
        *self.entries.get(f)
    }
}

// ---------------------------------------------------------------------------
// The three analyses a level's stencils are worth having
// ---------------------------------------------------------------------------

/// What a block's terminator may absorb from the block's tail.
pub(crate) struct Plan {
    pub skip: Vec<bool>,
    /// The comparison the branch will do itself: `(lhs, rhs, stencil)`, the
    /// stencil by its index in the library.
    pub cmpbr: Option<(ir::ValueId, ir::ValueId, usize)>,
    /// A `GetTag` the switch will do itself: the aggregate and its enum.
    pub tagsw: Option<(ir::ValueId, ir::EnumTy)>,
    /// The increment a jump into a loop's test does itself.
    pub incbr: Option<Incbr>,
}

/// A back edge that increments a loop's index and takes the loop's test, as
/// one `incbr/lt`: `into = from + by; if into < bound goto back else out`.
///
/// The shape is a block ending `t = add from, by; jump header(t, ..)` where
/// `t` already shares the parameter's slot, and a header that is nothing but
/// `branch (param >= bound) unsigned, out, back`. Unsigned and wrapping on
/// both sides, so the stencil's own `uint64_t` sum and comparison are the
/// same answer for every input, not only for a list's index.
#[derive(Clone, Copy)]
pub(crate) struct Incbr {
    pub from: ir::ValueId,
    pub by: u64,
    pub into: ir::ValueId,
    pub bound: ir::ValueId,
    pub back: ir::BlockId,
    pub out: ir::BlockId,
}

impl<'a> Jit<'a> {
    /// Where every value lives in the frame, and where the scratch begins.
    ///
    /// (i) **Slot coalescing.** `middle::lower` puts every loop variable in a
    /// block *parameter*, so the IR is a river of `p := a` copies: an edge's
    /// parallel copy, a `Return`'s move into the return area. Each one is a
    /// `mov` stencil — a load and a store — on top of the store the producer
    /// already did. Giving the producer the consumer's slot deletes both.
    ///
    /// This is the frame-slot half of what a register allocator's coalescing
    /// does, and it is the cheapest of the analyses here: one linear pass, a
    /// union-find, and a locality check. It is **not** a general one — it
    /// merges a single-use temporary into the class of the parameter it feeds,
    /// and never two parameters — because the safety argument then needs no
    /// liveness at all: the only place the merged slot can be read early is
    /// between the temporary's definition and the jump, which is a walk of the
    /// rest of one block.
    #[allow(clippy::too_many_arguments, reason = "the tables `emit` already built")]
    fn slots(
        &mut self,
        prog: &ir::Program,
        code: &ir::Code,
        frame: &FrameSig,
        uses: &ir::PerValue<u32>,
        opnd: &Operands,
        order: &[ir::BlockId],
        slot: &mut ir::PerValue<u32>,
    ) -> u32 {
        let mut pin = code.per_value_in(&mut self.bufs.pin, None);
        for (v, off) in code.entry().params.iter().zip(&frame.params) {
            *pin.get_mut(*v) = Some(*off);
        }
        let mut width = code.per_value_in(&mut self.bufs.width, 0);
        for (v, w) in width.iter_mut() {
            *w = self.slot_bytes(prog, code.ty_of(v));
        }
        let mut uf = code.per_value_from(&mut self.bufs.uf, |v| v);
        {
            self.coalesce(code, order, &mut uf, &mut pin, &width, uses, opnd);
        }
        self.pin_call_values(prog, code, &uf, &mut pin, frame.size, uses, opnd);
        // One slot per class: the pinned offset when the class holds a
        // parameter or a return value, a fresh one otherwise.
        let mut at = frame.param_end;
        let mut of_root = code.per_value_in(&mut self.bufs.of_root, None);
        for (v, pinned) in pin.iter() {
            let Some(p) = *pinned else { continue };
            // `coalesce` never merges two classes pinned at different offsets.
            *of_root.get_mut(find(&uf, v)) = Some(p);
        }
        let mut wide = code.per_value_in(&mut self.bufs.wide, 0);
        for (v, w) in width.iter() {
            let x = wide.get_mut(find(&uf, v));
            *x = (*x).max(*w);
        }
        *slot = code.per_value_in(slot, 0);
        for (v, s) in slot.iter_mut() {
            let r = find(&uf, v);
            let off = match *of_root.get(r) {
                Some(o) => o,
                None => {
                    let o = at;
                    at += *wide.get(r);
                    *of_root.get_mut(r) = Some(o);
                    o
                }
            };
            *s = off;
        }
        let parts = self.alias_parts(prog, code, &uf, &pin, slot, uses, opnd);
        self.forward_fields(prog, code, &uf, &pin, &width, slot, uses, opnd, parts, at);
        self.bufs.pin = pin;
        self.bufs.width = width;
        self.bufs.uf = uf;
        self.bufs.of_root = of_root;
        self.bufs.wide = wide;
        at
    }

    /// (i.e) A value that is **part of another** lives there. A list's length
    /// is the second word of the list, and a field built into a struct or an
    /// enum is the bytes of that field: the load that defines it writes them in
    /// place, and the move that would have copied it becomes the identity.
    ///
    /// Only where both sides keep still for as long as the part is read:
    ///
    ///  * a **length**, of a list that is a slot class of its own and never a
    ///    loop's parameter, so nothing writes its slot again — an instruction's
    ///    result, or one of the function's own parameters;
    ///  * a **field**, defined in the block that builds the aggregate and read
    ///    only there, by a load that writes nothing else and does not read the
    ///    aggregate's class, with nothing in between touching that class. The
    ///    field is whole frame words wide, because a slot write is a whole
    ///    word, and not behind a pointer.
    ///
    /// Answers whether it ran, which is whether `bufs.aliased` is this
    /// function's.
    #[allow(
        clippy::too_many_arguments,
        reason = "the tables `slots` already built"
    )]
    fn alias_parts(
        &mut self,
        prog: &ir::Program,
        code: &ir::Code,
        uf: &ir::PerValue<ir::ValueId>,
        pin: &ir::PerValue<Option<u32>>,
        slot: &mut ir::PerValue<u32>,
        uses: &ir::PerValue<u32>,
        opnd: &Operands,
    ) -> bool {
        let part = |i: &ir::Inst| {
            matches!(
                i,
                ir::Inst::ArrayLen { .. } | ir::Inst::MakeStruct { .. } | ir::Inst::MakeEnum { .. }
            )
        };
        if !code.blocks().iter().any(|b| b.insts.iter().any(part)) {
            return false;
        }
        let mut members = code.per_value_in(&mut self.bufs.members, 0);
        let mut param = code.per_value_in(&mut self.bufs.param, false);
        for v in code.value_ids() {
            *members.get_mut(find(uf, v)) += 1;
        }
        for b in code.blocks().iter().skip(1) {
            for p in &b.params {
                *param.get_mut(*p) = true;
            }
        }
        // Where each instruction result is defined: its block, and its index
        // there.
        let mut def_in = code.per_value_in(&mut self.bufs.def_in, (u32::MAX, 0));
        for (id, b) in code.iter() {
            for (k, i) in b.insts.iter().enumerate() {
                for d in i.results() {
                    *def_in.get_mut(*d) = (id.index() as u32, k as u32);
                }
            }
        }
        // `members` counts a class at its root, so one member means a class
        // of one.
        let alone = |v: ir::ValueId| *members.get(v) == 1 && pin.get(v).is_none();
        // The entry's parameters keep the slots the caller left them in.
        let entry: &[ir::ValueId] = &code.entry().params;
        let still = |v: ir::ValueId| {
            *members.get(v) == 1 && !*param.get(v) && (pin.get(v).is_none() || entry.contains(&v))
        };
        let mut aliased = code.per_value_in(&mut self.bufs.aliased, false);
        // Per class, the last instruction of the block at hand that touched
        // it, stamped with the block.
        let mut last = code.per_value_in(&mut self.bufs.last, (u32::MAX, 0));
        let mut touched_before = std::mem::take(&mut self.bufs.touched);
        for (bid, b) in code.iter() {
            if !b.insts.iter().any(part) {
                continue;
            }
            let stamp = bid.index() as u32;
            let def_at = |v: ir::ValueId| {
                Some(*def_in.get(v))
                    .filter(|(db, _)| *db == stamp)
                    .map(|(_, k)| k as usize)
            };
            // Per aggregate built here, the last instruction before it that
            // touches its class. Scanning back from each field instead is `n²`
            // in a variant's fields (PERFORMANCE.md §6.37).
            touched_before.clear();
            for (j, i) in b.insts.iter().enumerate() {
                let mut before = None;
                if let ir::Inst::MakeStruct { dest, .. } | ir::Inst::MakeEnum { dest, .. } = i {
                    before = Some(*last.get(find(uf, *dest)))
                        .filter(|(lb, _)| *lb == stamp)
                        .map(|(_, t)| t as usize);
                }
                touched_before.push(before);
                for o in opnd.inst(bid, j).iter().chain(i.results()) {
                    *last.get_mut(find(uf, *o)) = (stamp, j as u32);
                }
            }
            for ((j, i), before) in b.insts.iter().enumerate().zip(&touched_before) {
                if let ir::Inst::ArrayLen { dest, array } = i {
                    // The length is read where it is whatever its class: a
                    // read of it is a read of its own slot (`Fn2::at`), and a
                    // copy into a merged parameter's slot or a pinned one is a
                    // move from that slot like any other. `still` keeps the
                    // list's word where it is for as long as the length lives.
                    if still(*array) {
                        *slot.get_mut(*dest) = *slot.get(*array) + 8;
                        *aliased.get_mut(*dest) = true;
                    }
                    continue;
                }
                // The fields' declared types, which say which of them is boxed.
                let (dest, fields, id, variant, ftys) = match i {
                    ir::Inst::MakeStruct { dest, ty, fields } => (
                        *dest,
                        fields,
                        ty.id(),
                        None,
                        Cow::Borrowed(prog.types.fields(*ty).as_slice()),
                    ),
                    ir::Inst::MakeEnum {
                        dest,
                        ty,
                        variant,
                        fields,
                    } => {
                        let l = self.layout_of(prog, ty.id());
                        if matches!(
                            &l.repr,
                            Repr::Enum {
                                repr: EnumRepr::Bare { .. },
                                ..
                            }
                        ) {
                            continue;
                        }
                        let owner = prog.type_info(ty.id()).ty;
                        let ftys = variant_types(self.tables, &owner, variant.index());
                        (
                            *dest,
                            fields,
                            ty.id(),
                            Some((*ty, *variant)),
                            Cow::Owned(ftys),
                        )
                    }
                    _ => continue,
                };
                let (l, e);
                let owner = prog.type_info(id).ty;
                let offs: &[At] = match variant {
                    None => {
                        l = self.layout_of(prog, id);
                        l.fields.as_slice()
                    }
                    Some((ty, v)) => {
                        e = self.enum_layout(prog, ty);
                        e.variants.get(v).as_slice()
                    }
                };
                if pin.get(dest).is_some() {
                    continue;
                }
                let root = find(uf, dest);
                let in_class = |v: &ir::ValueId| find(uf, *v) == root;
                // A field already aliased is a length, which no load below
                // defines, or a field of an earlier aggregate, which is a
                // second use.
                for ((f, off), t) in fields.iter().zip(offs).zip(ftys.iter()) {
                    let w = self.width(prog, code.ty_of(*f));
                    if w == 0
                        || !w.is_multiple_of(8)
                        || !alone(*f)
                        || *uses.get(*f) != 1
                        || self.boxes(&owner, t)
                    {
                        continue;
                    }
                    let Some(k) = def_at(*f).filter(|k| *k < j) else {
                        continue;
                    };
                    let def = b.insts.get(k);
                    let loads = matches!(
                        def,
                        Some(
                            ir::Inst::ArrayGet { .. }
                                | ir::Inst::GetField { .. }
                                | ir::Inst::GetPayload { .. }
                                | ir::Inst::Binary { .. }
                                | ir::Inst::Unary { .. }
                        )
                    ) && def.is_some_and(keeps_callee_frame);
                    if !loads {
                        continue;
                    }
                    let touches = |x: &ir::Inst| {
                        opnd.inst(bid, k).iter().any(&in_class) || x.results().iter().any(&in_class)
                    };
                    if def.is_some_and(touches) || before.is_some_and(|t| t > k) {
                        continue;
                    }
                    *slot.get_mut(*f) = *slot.get(dest) + off.bytes();
                    *aliased.get_mut(*f) = true;
                }
            }
        }
        self.bufs.members = members;
        self.bufs.param = param;
        self.bufs.def_in = def_in;
        self.bufs.aliased = aliased;
        self.bufs.last = last;
        self.bufs.touched = touched_before;
        true
    }

    /// (i.f) A field read out of a struct is read **where it is**: the
    /// `GetField`'s result takes the bytes of the field inside its struct's
    /// slot, and the load and store that would have copied it out become the
    /// identity. `ctx.Random.0` inside an effect call was two such copies.
    ///
    /// Only where the struct's bytes keep still for as long as the field is
    /// read:
    ///
    ///  * a struct **written once**: a slot class of its own, never a block's
    ///    parameter, and not pinned past the frame — the field may then be read
    ///    anywhere, and so may a field forwarded out of it in turn;
    ///  * otherwise, a field read only by later instructions of its own block,
    ///    with nothing up to the last of them writing any of the field's
    ///    bytes. An edge writes a block parameter, so a terminator's read
    ///    never qualifies.
    ///
    /// The field is whole frame words wide and not boxed, the result is a
    /// class of its own, and the struct lives inside this frame, below `end`:
    /// past it is the callee's frame, which every call rewrites.
    #[allow(
        clippy::too_many_arguments,
        reason = "the tables `slots` already built"
    )]
    fn forward_fields(
        &mut self,
        prog: &ir::Program,
        code: &ir::Code,
        uf: &ir::PerValue<ir::ValueId>,
        pin: &ir::PerValue<Option<u32>>,
        width: &ir::PerValue<u32>,
        slot: &mut ir::PerValue<u32>,
        uses: &ir::PerValue<u32>,
        opnd: &Operands,
        parts: bool,
        end: u32,
    ) {
        // How far a read inside one block may be from the field it reads.
        // Bounds the scan below, so a long block stays linear.
        const WINDOW: usize = 32;
        let get = |i: &ir::Inst| matches!(i, ir::Inst::GetField { .. });
        if !code.blocks().iter().any(|b| b.insts.iter().any(get)) {
            return;
        }
        let mut members = code.per_value_in(&mut self.bufs.members, 0);
        let mut param = code.per_value_in(&mut self.bufs.param, false);
        for v in code.value_ids() {
            *members.get_mut(find(uf, v)) += 1;
        }
        for b in code.blocks().iter().skip(1) {
            for p in &b.params {
                *param.get_mut(*p) = true;
            }
        }
        let held_parts = std::mem::take(&mut self.bufs.aliased);
        // `alias_parts`' answer, where it ran for this function.
        let aliased: Option<&ir::PerValue<bool>> = if parts { Some(&held_parts) } else { None };
        let is_aliased = |v: ir::ValueId| aliased.is_some_and(|a| *a.get(v));
        let entry: &[ir::ValueId] = &code.entry().params;
        let alone = |v: ir::ValueId| *members.get(v) == 1 && pin.get(v).is_none() && !is_aliased(v);
        // Written once, by its definition or by the caller, and never again.
        let mut fixed = code.per_value_in(&mut self.bufs.fixed, false);
        for (v, f) in fixed.iter_mut() {
            *f = *members.get(v) == 1
                && !*param.get(v)
                && !is_aliased(v)
                && (pin.get(v).is_none() || entry.contains(&v));
        }
        for (bid, b) in code.iter() {
            for (j, i) in b.insts.iter().enumerate() {
                let ir::Inst::GetField {
                    dest,
                    agg,
                    ty,
                    field,
                } = i
                else {
                    continue;
                };
                let id = ty.id();
                let l = self.layout_of(prog, id);
                let off = l.fields.get(*field).bytes();
                let w = self.width(prog, code.ty_of(*dest));
                let owner = prog.type_info(id).ty;
                if w == 0
                    || !w.is_multiple_of(8)
                    || !alone(*dest)
                    || self.boxes(&owner, &prog.types.field(*ty, *field))
                {
                    continue;
                }
                let src = *slot.get(*agg) + off;
                if src + w > end {
                    continue;
                }
                let held = if *fixed.get(*agg) {
                    true
                } else {
                    // Every read is a later instruction of this block, and
                    // nothing up to the last one writes the field's bytes.
                    let mut seen = 0u32;
                    let mut last = j;
                    for k in j + 1..b.insts.len().min(j + 1 + WINDOW) {
                        let reads =
                            opnd.inst(bid, k).iter().filter(|o| **o == *dest).count() as u32;
                        if reads > 0 {
                            seen += reads;
                            last = k;
                        }
                    }
                    // Instructions are forwarded in order, so a later result
                    // still has a slot of its own here.
                    let writes = |k: usize| {
                        b.insts.get(k).is_some_and(|x| {
                            x.results().iter().any(|r| {
                                let at = *slot.get(*r);
                                let rw = *width.get(*r);
                                at < src + w && src < at + rw
                            })
                        })
                    };
                    seen == *uses.get(*dest) && seen > 0 && !(j + 1..=last).any(writes)
                };
                if !held {
                    continue;
                }
                *slot.get_mut(*dest) = src;
                let still = *fixed.get(*agg);
                *fixed.get_mut(*dest) = still;
            }
        }
        self.bufs.members = members;
        self.bufs.param = param;
        self.bufs.fixed = fixed;
        self.bufs.aliased = held_parts;
    }

    /// The merges themselves. See [`Jit::slots`].
    ///
    /// (i.d) A **latch** is the one exception to "never two parameters": a
    /// block whose parameter is only passed on, by its `Jump`, to the loop
    /// header's parameter. The paths that meet there each pass the header's
    /// own value or a temporary computed from it, so the latch's parameter
    /// can take the header parameter's slot and every copy on the way round
    /// the loop becomes the identity. It is safe for the same reason the rest
    /// is: [`Jit::merge_is_safe`] finds nothing in the latch reading the header
    /// parameter's class, so the only write that moves earlier is an edge copy
    /// at the end of a predecessor, and each temporary a predecessor passes the
    /// latch afterwards is checked against the header's class, not the
    /// latch's.
    #[allow(
        clippy::too_many_arguments,
        reason = "the tables `slots` already built"
    )]
    fn coalesce(
        &mut self,
        code: &ir::Code,
        order: &[ir::BlockId],
        uf: &mut ir::PerValue<ir::ValueId>,
        pin: &mut ir::PerValue<Option<u32>>,
        width: &ir::PerValue<u32>,
        uses: &ir::PerValue<u32>,
        opnd: &Operands,
    ) {
        let mut is_param = code.per_value_in(&mut self.bufs.is_param, false);
        let mut def_block = code.per_value_in(&mut self.bufs.def_block, None);
        // The first instruction after a value's definition: a parameter's
        // block is all after it.
        let mut after_def = code.per_value_in(&mut self.bufs.after_def, 0);
        for (id, b) in code.iter() {
            for p in &b.params {
                *is_param.get_mut(*p) = true;
                *def_block.get_mut(*p) = Some(id);
            }
            for (k, i) in b.insts.iter().enumerate() {
                for d in i.results() {
                    *def_block.get_mut(*d) = Some(id);
                    *after_def.get_mut(*d) = k as u32 + 1;
                }
            }
        }
        // Per class root: whether the class holds a pinned value, and whether
        // anything has merged into it. So far only the entry's parameters are
        // pinned: a return value pinned below is read by its `Return` alone.
        let mut pinned = code.per_value_in(&mut self.bufs.pinned, false);
        for ((_, x), (_, p)) in pinned.iter_mut().zip(pin.iter()) {
            *x = p.is_some();
        }
        let mut grown = code.per_value_in(&mut self.bufs.grown, false);
        let mut pairs: Vec<(ir::ValueId, ir::ValueId)> = Vec::new();
        // The entry first, whose parameters are where a tail call's loop
        // header lives, and then every block after the blocks it jumps to,
        // but for a loop's way back: a latch is met before the arms that pass
        // it anything. `order` is the layout's, which puts the entry first.
        let rest = order.iter().skip(1).rev();
        for id in order.iter().take(1).chain(rest) {
            let b = code.get(*id);
            // (i.a) An edge's block arguments.
            pairs.clear();
            for t in b.term.targets() {
                for (p, a) in code.get(t.block).params.iter().zip(t.args.iter()) {
                    pairs.push((*p, *a));
                }
            }
            let jumps = matches!(b.term, ir::Term::Jump(_));
            // An argument has its parameter's type (`ir::verify`), so the two
            // are as wide.
            for &(p, a) in &pairs {
                // No edge enters the entry (`ir::Code`), so `p` is not an
                // entry parameter, was never an argument merged here, and is
                // not `a`.
                let root = find(uf, p);
                // Only a value defined in this block and used exactly once is
                // a candidate. A temporary, or an entry parameter, because its
                // slot is where the caller left it and the class can simply
                // take that. Or a latch's parameter that nothing has merged
                // into yet — the order above meets a latch before its arms —
                // passed to a class that takes no caller's slot: a tail call's
                // loop header is pinned, as the entry's parameters merge into
                // it.
                let entry_ok = pin.get(a).is_some() && *id == ir::BlockId::ENTRY;
                let latch_ok = jumps && *width.get(a) == 8 && !*pinned.get(root) && !*grown.get(a);
                if !entry_ok && (*def_block.get(a) != Some(*id) || (*is_param.get(a) && !latch_ok))
                {
                    continue;
                }
                if *uses.get(a) != 1 {
                    continue;
                }
                // Its one read is this edge, so `a` is still a class of its
                // own, but for what a latch's predecessors passed it.
                // A second argument into a class this terminator already
                // merged into is a second read of it there, which
                // `merge_is_safe` refuses; that is also what keeps two entry
                // parameters, the only values pinned yet, out of one class.
                if !self.merge_is_safe(code, opnd, (*id, b), uf, root, a, *after_def.get(a)) {
                    continue;
                }
                *uf.get_mut(a) = root;
                let held = *pinned.get(root) | *pinned.get(a);
                *pinned.get_mut(root) = held;
                *grown.get_mut(root) = true;
            }
            // (i.b) A `Return`'s move into the return area, which is the same
            // copy with a fixed destination.
            if let ir::Term::Return(v) = &b.term {
                // Read by the `Return` alone, so nothing reads or writes its
                // class past its definition, and no edge merged it.
                if *width.get(*v) == 8
                    && !*is_param.get(*v)
                    && *def_block.get(*v) == Some(*id)
                    && *uses.get(*v) == 1
                {
                    // A class of one, pinned at the return area.
                    *pin.get_mut(*v) = Some(RET);
                }
            }
        }
        self.bufs.is_param = is_param;
        self.bufs.def_block = def_block;
        self.bufs.after_def = after_def;
        self.bufs.pinned = pinned;
        self.bufs.grown = grown;
    }

    /// (i.c) A value that only crosses a direct call lives **in the callee's
    /// frame**: an argument where the callee reads its parameter, a result
    /// where the callee left it. That deletes the copy between the value's own
    /// slot and the callee's, on each side of the call — the load and store a
    /// list loop otherwise pays per element for the element it hands its step
    /// and the answer it takes back.
    ///
    /// The callee's frame begins at `frame_size`, and nothing below it writes
    /// there; what does is a call of any kind, which lays its own frame out
    /// there. So a value is pinned only where every instruction across its
    /// life is one of [`keeps_callee_frame`]'s, and only where it is a slot
    /// class of its own, used in its defining block:
    ///
    ///  * a **result**, whose every use is in the block after the call;
    ///  * an **argument**, used once, by that call, and defined by an
    ///    instruction that writes nothing but its own slot. Its definition must
    ///    not land inside a pinned result's life, whose slot it could overlap.
    #[allow(
        clippy::too_many_arguments,
        reason = "the tables `slots` already built"
    )]
    fn pin_call_values(
        &mut self,
        prog: &ir::Program,
        code: &ir::Code,
        uf: &ir::PerValue<ir::ValueId>,
        pin: &mut ir::PerValue<Option<u32>>,
        frame_size: u32,
        uses: &ir::PerValue<u32>,
        opnd: &Operands,
    ) {
        let has_call = |b: &ir::Block| b.insts.iter().any(|i| matches!(i, ir::Inst::Call { .. }));
        if !code.blocks().iter().any(has_call) {
            return;
        }
        let mut members = code.per_value_in(&mut self.bufs.members, 0);
        for v in code.value_ids() {
            *members.get_mut(find(uf, v)) += 1;
        }
        let alone = |v: ir::ValueId, pin: &ir::PerValue<Option<u32>>| {
            *members.get(v) == 1 && pin.get(v).is_none()
        };
        let calls_code = |func: &FuncIdx| matches!(prog.funcs.get(*func).body, ir::Body::Code(_));
        // Where each value is read in the block at hand — an instruction's
        // index, or `last` for the terminator — and where it is defined. One
        // entry per value, stamped with the block it describes.
        let mut reads = code.per_value_in(&mut self.bufs.reads, BlockUses::default());
        let mut defs = code.per_value_in(&mut self.bufs.defs, None);
        let mut kept = std::mem::take(&mut self.bufs.kept);
        for (bid, b) in code.iter() {
            if !has_call(b) {
                continue;
            }
            let last = b.insts.len();
            // How many instructions before each index leave the callee's
            // frame alone, so a range is pure in one subtraction.
            kept.clear();
            let mut so_far = 0;
            kept.push(so_far);
            for (k, i) in b.insts.iter().enumerate() {
                for d in i.results() {
                    *defs.get_mut(*d) = Some((bid, k));
                }
                so_far += usize::from(keeps_callee_frame(i));
                kept.push(so_far);
            }
            for k in 0..b.insts.len() {
                for o in opnd.inst(bid, k) {
                    note_use(&mut reads, bid, *o, k);
                }
            }
            for o in opnd.term(code, bid) {
                note_use(&mut reads, bid, *o, last);
            }
            let read_at = |v: ir::ValueId| Some(*reads.get(v)).filter(|u| u.block == Some(bid));
            let def_at = |v: ir::ValueId| (*defs.get(v)).filter(|(b, _)| *b == bid).map(|(_, k)| k);
            let pure = |from: usize, to: usize| {
                to <= from || ent(&kept, to, 0) - ent(&kept, from, 0) == to - from
            };
            let mut results: Vec<(usize, usize)> = Vec::new();
            for (j, i) in b.insts.iter().enumerate() {
                let ir::Inst::Call { dest: d, func, .. } = *i else {
                    continue;
                };
                if !calls_code(&func) {
                    continue;
                }
                let Some(dr) = read_at(d) else { continue };
                let mut end = dr.last;
                // A value is read after its definition (`ir::verify`), so
                // `dr.first` is past `j`.
                if !alone(d, pin) || dr.count != *uses.get(d) {
                    continue;
                }
                // A `Bool` read only to be counted shares the slot it is
                // counted from: the conversion is a copy of the same word.
                let mut alias = None;
                if dr.count == 1 {
                    // Read once, by the instruction after it
                    // (`ir::UnOp::FromBool`), so it is a class of its own and
                    // nothing pinned it.
                    let counted = match b.insts.get(dr.first) {
                        Some(ir::Inst::Unary {
                            op: ir::UnOp::FromBool,
                            dest: z,
                            ..
                        }) => read_at(*z).map(|zr| (*z, zr.last)),
                        _ => None,
                    };
                    if let Some((z, z_last)) = counted {
                        end = end.max(z_last);
                        alias = Some(z);
                    }
                }
                if !pure(j + 1, (end + 1).min(last)) {
                    continue;
                }
                *pin.get_mut(d) = Some(frame_size + RET);
                if let Some(z) = alias {
                    *pin.get_mut(z) = Some(frame_size + RET);
                }
                results.push((j, end));
            }
            for (j, i) in b.insts.iter().enumerate() {
                let ir::Inst::Call { func, args, .. } = i else {
                    continue;
                };
                if !calls_code(func) {
                    continue;
                }
                let fs = self.frames.get(*func);
                for (a, off) in args.iter().zip(fs.params.iter()) {
                    let Some(k) = def_at(*a).filter(|k| *k < j) else {
                        continue;
                    };
                    let defines = matches!(
                        b.insts.get(k),
                        Some(
                            ir::Inst::ArrayGet { .. }
                                | ir::Inst::ArrayLen { .. }
                                | ir::Inst::Binary { .. }
                                | ir::Inst::Unary { .. }
                                | ir::Inst::GetField { .. }
                                | ir::Inst::GetPayload { .. }
                                | ir::Inst::GetTag { .. }
                        )
                    ) && b.insts.get(k).is_some_and(keeps_callee_frame);
                    // Read once, by this call, so no edge merged it, and
                    // defined by an instruction, so it is no entry parameter;
                    // the values pinned above are a call's result and a
                    // `FromBool` read before the next call.
                    if !defines
                        || *uses.get(*a) != 1
                        || !pure(k + 1, j)
                        || results.iter().any(|(rj, re)| *rj < k && k <= *re)
                    {
                        continue;
                    }
                    *pin.get_mut(*a) = Some(frame_size + off);
                }
            }
        }
        self.bufs.members = members;
        self.bufs.reads = reads;
        self.bufs.defs = defs;
        self.bufs.kept = kept;
    }

    /// Whether nothing in `root`'s class is read or written from instruction
    /// `from` of the block, the first after `a`'s definition, to its end.
    #[allow(clippy::too_many_arguments, reason = "one question about one block")]
    fn merge_is_safe(
        &self,
        code: &ir::Code,
        opnd: &Operands,
        (id, b): (ir::BlockId, &ir::Block),
        uf: &ir::PerValue<ir::ValueId>,
        root: ir::ValueId,
        a: ir::ValueId,
        from: u32,
    ) -> bool {
        // A write to the class is no separate question: every value merged
        // into it is a parameter or a temporary its own block's terminator
        // reads once, so one defined here is a second read below.
        for k in from as usize..b.insts.len() {
            if opnd.inst(id, k).iter().any(|o| find(uf, *o) == root) {
                return false;
            }
        }
        // The terminator reads `a` itself once; anything else in the class is
        // a conflict.
        opnd.term(code, id)
            .iter()
            .filter(|o| find(uf, **o) == root || **o == a)
            .count()
            <= 1
    }

    /// (k) The order the blocks are laid out in.
    ///
    /// Copy-and-patch's fallthrough elision only pays when the block a branch
    /// goes to is the block that comes next, and IR order is not that order:
    /// `middle::lower` emits a loop as header, exit, body, so the *taken* arm of
    /// every test is the one the loop takes every iteration. Reverse postorder
    /// — depth-first, `then` before `else`, reversed — puts the else-arm and
    /// the loop body immediately after the test, which is where the elision
    /// wants them.
    fn layout(&mut self, code: &ir::Code) -> Vec<ir::BlockId> {
        let mut seen = code.per_block_in(&mut self.bufs.seen, false);
        let mut post = std::mem::take(&mut self.bufs.order);
        post.clear();
        // An explicit stack, because a deeply nested function would blow a
        // recursive one and this runs on every function in the program.
        let mut stack = std::mem::take(&mut self.bufs.stack);
        stack.clear();
        *seen.get_mut(ir::BlockId::ENTRY) = true;
        stack.push((ir::BlockId::ENTRY, 0));
        while let Some((b, k)) = stack.pop() {
            // The `k`th successor alone: a switch's block is popped once per
            // case, and listing all of them each time was quadratic in its
            // width.
            match code.get(b).term.target(k).map(|t| t.block) {
                Some(s) => {
                    stack.push((b, k + 1));
                    if !*seen.get(s) {
                        *seen.get_mut(s) = true;
                        stack.push((s, 0));
                    }
                }
                None => post.push(b),
            }
        }
        // The entry reaches every block (`ir::Code`), so `post` lists them all.
        post.reverse();
        self.bufs.seen = seen;
        self.bufs.stack = stack;
        post
    }

    /// (j) `mem2reg`: the optimisation the paper names and does not implement.
    ///
    /// > "This mechanism can also be used to implement the `mem2reg`
    /// > optimization to keep hot local variables in registers as well."
    ///
    /// `middle::lower` puts every loop variable in a **block parameter**, so a
    /// loop's state is exactly the header block's parameter list, and the whole
    /// of `mem2reg` for this IR is: give those parameters CPS registers, fill
    /// them on the way into the loop, and update them on the back edge.
    ///
    /// The constraint that makes it sound without a liveness analysis is the
    /// **region**: the header and every block up to the furthest back edge, all
    /// of them free of any stencil with the zero-register prototype, and
    /// enterable only at the header. Inside that region a register cannot be
    /// clobbered by anything. Outside it, the value is read from its frame slot,
    /// which the edge keeps in step — unless every use is provably a register
    /// one, and then the slot is not written at all.
    ///
    /// Which is why the region comes back out in `region` and the promoted
    /// values in `cross`: "outside it, from the frame slot" is a rule about a
    /// *read*, and [`Fn2::loc`] is what states it. Leaving it unsaid was
    /// buri-lang/buri#47 — a promoted loop variable read by a comparison one
    /// block past the region, after a call in between had already taken the
    /// register.
    ///
    /// Answers how many integer and floating registers the promotion took, so
    /// that the paper's own expression-temporary allocator can have the rest.
    fn promote(
        &mut self,
        code: &ir::Code,
        out: &mut ir::PerValue<Option<Reg>>,
        wt: &mut ir::PerValue<bool>,
        cross: &mut ir::PerValue<bool>,
        region: &mut ir::PerBlock<bool>,
    ) -> (usize, usize) {
        // Every register but the last: one has to be left for the expression
        // temporaries the paper's own allocator keeps there, or the arithmetic
        // inside the loop loses more than the loop variable gains.
        const PROMOTABLE: [Reg; super::abi::CPS_REGISTER_COUNT - 1] = [Reg::R0, Reg::R1];
        // Only a back edge into a block with parameters can make a candidate
        // below, and most functions have none: they skip the tables.
        let back_edge = code.iter().any(|(p, b)| {
            b.term
                .targets()
                .any(|t| t.block <= p && !code.get(t.block).params.is_empty())
        });
        if !back_edge {
            return (0, 0);
        }
        let mut loops = std::mem::take(&mut self.bufs.loops);
        let found = loops.innermost(code);
        // `region` has a row per block, every one `false` (`Jit::function`).
        if found.is_some() {
            for x in &loops.best {
                *region.get_mut(*x) = true;
            }
        }
        self.bufs.loops = loops;
        let Some(header) = found else {
            return (0, 0);
        };
        let body: &ir::PerBlock<bool> = region;

        // Where every value is used, and whether every one of those uses can
        // read a register.
        let params = code.get(header).params.clone();
        let mut reg_ok = code.per_value(true);
        let mut uses = code.per_value(0u32);
        let mut ops = Vec::new();
        for (bi, b) in code.iter() {
            let inside = *body.get(bi);
            for i in &b.insts {
                ops.clear();
                i.operands(&mut ops);
                let ok = inside
                    && matches!(i, ir::Inst::Binary { .. } | ir::Inst::Unary { .. })
                    && !matches!(
                        i,
                        ir::Inst::Unary {
                            op: ir::UnOp::FromBool,
                            ..
                        }
                    );
                for o in &ops {
                    *uses.get_mut(*o) += 1;
                    if !ok {
                        *reg_ok.get_mut(*o) = false;
                    }
                }
            }
            ops.clear();
            b.term.operands(&mut ops);
            let ok = inside && matches!(b.term, ir::Term::Branch { .. });
            for o in &ops {
                *uses.get_mut(*o) += 1;
                if !ok {
                    *reg_ok.get_mut(*o) = false;
                }
            }
            // An edge argument is a register use only when it lands in the same
            // register it already sits in, which the edge emitter turns into
            // nothing at all.
            for t in b.term.targets() {
                for (p, a) in code.get(t.block).params.iter().zip(t.args.iter()) {
                    *uses.get_mut(*a) += 1;
                    if !(t.block == header && *p == *a) {
                        *reg_ok.get_mut(*a) = false;
                    }
                }
            }
        }

        let (mut ri, mut rf) = (0usize, 0usize);
        for p in &params {
            let ty = code.ty_of(*p);
            let float = ty == ir::Type::F64;
            let scalar = matches!(
                ty,
                ir::Type::I64
                    | ir::Type::Ptr
                    | ir::Type::I1
                    | ir::Type::I8
                    | ir::Type::I16
                    | ir::Type::I32
                    | ir::Type::F64
            );
            if !scalar || *uses.get(*p) == 0 {
                continue;
            }
            let k = if float { &mut rf } else { &mut ri };
            let Some(r) = PROMOTABLE.get(*k) else {
                continue;
            };
            *out.get_mut(*p) = Some(*r);
            *cross.get_mut(*p) = true;
            *wt.get_mut(*p) = !*reg_ok.get(*p);
            *k += 1;
        }
        // The value the back edge hands the parameter belongs in the parameter's
        // own register, or the loop-carried chain still goes through memory:
        // `add x8, x2, #2 ; str x8, [fp] ; ldr x2, [fp]` instead of
        // `add x2, x2, #2`. Measured, this is the whole of the difference —
        // without it the promotion is 36% *slower* than leaving the variable in
        // the frame, because it adds a reload to a chain that already had a
        // store-to-load forward in it.
        for (bi, b) in code.iter() {
            if !*body.get(bi) {
                continue;
            }
            for t in b.term.targets() {
                if t.block != header {
                    continue;
                }
                let ps = code.get(t.block).params.clone();
                for (p, a) in ps.iter().zip(t.args.iter()) {
                    let Some(k) = *out.get(*p) else {
                        continue;
                    };
                    if p == a || out.get(*a).is_some() || *uses.get(*a) != 1 {
                        continue;
                    }
                    // `a` has `p`'s type (`ir::verify`), so it reads the same
                    // register file.
                    // Defined in this block by something with a register
                    // result, and nothing after it may read the register the
                    // definition is about to overwrite.
                    let Some(di) = b.insts.iter().position(|i| i.results().contains(a)) else {
                        continue;
                    };
                    if !matches!(
                        b.insts.get(di),
                        Some(ir::Inst::Binary { .. } | ir::Inst::Unary { .. })
                    ) || matches!(
                        b.insts.get(di),
                        Some(ir::Inst::Unary {
                            op: ir::UnOp::FromBool,
                            ..
                        })
                    ) {
                        continue;
                    }
                    let mut ops = Vec::new();
                    let mut clash = false;
                    for i in b.insts.iter().skip(di + 1) {
                        ops.clear();
                        i.operands(&mut ops);
                        clash |= ops.iter().any(|o| *out.get(*o) == Some(k));
                    }
                    ops.clear();
                    b.term.operands(&mut ops);
                    for tt in b.term.targets() {
                        // The hand-over itself, `a` at `p` in `t`, reads no
                        // register: `a` has none yet.
                        ops.extend_from_slice(&tt.args);
                    }
                    clash |= ops.iter().any(|o| *out.get(*o) == Some(k));
                    if clash {
                        continue;
                    }
                    *out.get_mut(*a) = Some(k);
                    *cross.get_mut(*a) = true;
                    *wt.get_mut(*a) = false;
                }
            }
        }
        (ri, rf)
    }

    /// The CPS register assignment.
    ///
    /// The paper's Figure 8: a temporary can live in a register between the
    /// stencil that defines it and the one that consumes it, provided nothing
    /// in between clobbers it. Below [`Level::Reg`] there are no register
    /// stencils in the library, so every value is a frame slot.
    ///
    /// This is a *local* allocator on purpose. Copy-and-patch's claim is
    /// compile speed, and the paper says the same thing: "we only use registers
    /// to store temporary values while evaluating expression trees". A value
    /// that crosses a call or a block boundary stays in the frame.
    fn regalloc(
        &mut self,
        code: &ir::Code,
        out: &mut ir::PerValue<Option<Reg>>,
        taken: (usize, usize),
        total: &ir::PerValue<u32>,
        opnd: &Operands,
    ) {
        // Uses over the **whole function**, not just the defining block. A
        // value defined in one block is visible to every block it dominates,
        // and a register only survives to the end of its own block — so a value
        // with any out-of-block use has to stay in the frame. Counting uses
        // per block instead was a miscompile: the consumer read a frame slot
        // the producer had never written.
        // Where each value is used in the block being allocated: how often,
        // first and last. One entry per value of the function, stamped with
        // the block it describes, so moving to the next block resets nothing.
        let mut uses = code.per_value_in(&mut self.bufs.block_uses, BlockUses::default());
        let mut barrier = std::mem::take(&mut self.bufs.barrier);
        // The registers cross-block promotion took are not the local
        // allocator's to hand out.
        let register_count = super::abi::CPS_REGISTER_COUNT;
        let mut busy: Vec<Option<Held>> = vec![None; register_count];
        let mut busyf: Vec<Option<Held>> = vec![None; register_count];
        for (bi, block) in code.iter() {
            let n = block.insts.len();
            // Where the barriers are. A barrier is an instruction whose
            // stencil has the zero-register prototype and therefore clobbers
            // the file.
            barrier.clear();
            for (k, i) in block.insts.iter().enumerate() {
                for o in opnd.inst(bi, k) {
                    note_use(&mut uses, bi, *o, k);
                }
                barrier.push(is_barrier(i));
            }
            barrier.push(false);
            for o in opnd.term(code, bi) {
                note_use(&mut uses, bi, *o, n);
            }
            // The uses of `v` in this block, if it has any.
            let used = |v: ir::ValueId| Some(*uses.get(v)).filter(|u| u.block == Some(bi));

            for (r, b) in busy.iter_mut().enumerate() {
                *b = (r < taken.0).then_some(Held::Promoted);
            }
            for (r, b) in busyf.iter_mut().enumerate() {
                *b = (r < taken.1).then_some(Held::Promoted);
            }
            let (base, basef) = (taken.0, taken.1);
            for (k, i) in block.insts.iter().enumerate() {
                // Free every register whose value was last used here.
                for (slot, b) in busy.iter_mut().chain(busyf.iter_mut()).enumerate() {
                    let _ = slot;
                    if let Some(held) = *b {
                        let Held::Value(v) = held else {
                            continue; // a promoted register, not this pass's
                        };
                        // No entry means the value is not used in this block
                        // at all.
                        let live = used(v).is_some_and(|u| u.last > k);
                        if !live {
                            *b = None;
                        }
                    }
                }
                if ent(&barrier, k, false) {
                    // Nothing survives a zero-register stencil, except the
                    // promoted values, which are not the local allocator's and
                    // whose regions contain no barrier.
                    for (r, b) in busy.iter_mut().enumerate() {
                        *b = (r < base).then_some(Held::Promoted);
                    }
                    for (r, b) in busyf.iter_mut().enumerate() {
                        *b = (r < basef).then_some(Held::Promoted);
                    }
                    continue;
                }
                // A CPS register is one machine word, so a sixteen-byte
                // operand is never a candidate: every stencil at `I128` and
                // `U128` is frame-to-frame (`sources.rs::wide`), and promoting
                // one would be a value the consumer could not read.
                let wide = |p: &crate::compiler::semantics::types::Prim| {
                    matches!(
                        p,
                        crate::compiler::semantics::types::Prim::I128
                            | crate::compiler::semantics::types::Prim::U128
                    )
                };
                let (float, dest) = match i {
                    ir::Inst::Binary { dest, op, prim, .. } if !wide(prim) => {
                        let f = matches!(
                            prim,
                            crate::compiler::semantics::types::Prim::F32
                                | crate::compiler::semantics::types::Prim::F64
                        ) && !op.is_comparison();
                        (f, *dest)
                    }
                    ir::Inst::Unary { dest, prim, op, .. }
                        if !wide(&prim.prim()) && *op != ir::UnOp::FromBool =>
                    {
                        (prim.is_float(), *dest)
                    }
                    _ => continue,
                };
                if out.get(dest).is_some() {
                    continue; // already promoted across the loop
                }
                let Some(u) = used(dest) else {
                    continue;
                };
                if u.count != 1 || *total.get(dest) != 1 {
                    continue;
                }
                // A value is read after its definition (`ir::verify`), so its
                // first use is past `k`, and `barrier` runs from the block's
                // first instruction to one past its last.
                let at = u.first;
                if barrier.iter().take(at.min(n) + 1).skip(k).any(|b| *b) {
                    continue;
                }
                // The consumer has to be able to read a register operand.
                let consumes = if at == n {
                    // Only the branch's *condition* is read from a register; a
                    // value that reaches the terminator as a block argument is
                    // copied out of the frame, which a register never filled.
                    matches!(&block.term, ir::Term::Branch { cond, .. } if *cond == dest)
                } else {
                    matches!(
                        block.insts.get(at),
                        Some(ir::Inst::Binary { .. } | ir::Inst::Unary { .. })
                    ) && !matches!(
                        block.insts.get(at),
                        Some(ir::Inst::Unary {
                            op: ir::UnOp::FromBool,
                            ..
                        })
                    )
                };
                if !consumes {
                    continue;
                }
                let file = if float { &mut busyf } else { &mut busy };
                let _ = (base, basef);
                if let Some((r, b)) = Reg::ALL
                    .iter()
                    .zip(file.iter_mut())
                    .find(|(_, b)| b.is_none())
                {
                    *b = Some(Held::Value(dest));
                    *out.get_mut(dest) = Some(*r);
                }
            }
        }
        self.bufs.block_uses = uses;
        self.bufs.barrier = barrier;
    }

    /// Which `Inst::Const`s never need a frame slot, because every use of them
    /// is an immediate operand of a stencil that has an immediate variant.
    /// Each is marked in `immediate`, and lives at [`Loc::Imm`].
    ///
    /// [`zero_divisor`] is the one use that is *not* eligible however good the
    /// stencil is.
    fn constants(
        &mut self,
        code: &ir::Code,
        total: &ir::PerValue<u32>,
        constants: &mut ir::PerValue<Option<u64>>,
        immediate: &mut ir::PerValue<bool>,
    ) {
        *constants = code.per_value_in(constants, None);
        for block in code.blocks() {
            for i in &block.insts {
                if let ir::Inst::Const { dest, value } = i {
                    *constants.get_mut(*dest) = literal(value, code.ty_of(*dest));
                }
            }
        }
        // A use is immediate-eligible only as the right operand of a binary
        // operation whose immediate variant this level has.
        let mut imm = code.per_value_in(&mut self.bufs.imm, 0);
        for block in code.blocks() {
            for i in &block.insts {
                if let ir::Inst::Binary { op, prim, rhs, .. } = i {
                    // A `Str` comparison has no immediate form.
                    let Some(scalar) = prim.scalar() else {
                        continue;
                    };
                    if let Some(k) = *constants.get(*rhs) {
                        let (tag, _, _) = super::emit::prim_tag(scalar);
                        let name = op.name();
                        let key = key!["bin/", name, "/", tag, "/fi/f"];
                        if self.has(&key) && !zero_divisor(name, tag, k) {
                            *imm.get_mut(*rhs) += 1;
                        }
                    }
                }
            }
        }
        *immediate = code.per_value_in(immediate, false);
        for (v, i) in immediate.iter_mut() {
            let seen = *total.get(v);
            if constants.get(v).is_some() && seen > 0 && seen == *imm.get(v) {
                *i = true;
            }
        }
        self.bufs.imm = imm;
    }

    /// Fusions the terminator can absorb.
    pub(super) fn plan_block(&mut self, code: &ir::Code, st: &Fn2, block: &ir::Block) -> Plan {
        let n = block.insts.len();
        let mut skip = std::mem::take(&mut self.bufs.skip);
        skip.clear();
        skip.resize(n, false);
        let mut p = Plan {
            skip,
            cmpbr: None,
            tagsw: None,
            incbr: None,
        };
        // Every `Const` whose uses are all immediates disappears.
        for (i, skip) in block.insts.iter().zip(p.skip.iter_mut()) {
            if let ir::Inst::Const { dest, .. } = i {
                if matches!(st.home(*dest), Loc::Imm) {
                    *skip = true;
                }
            }
        }
        if n == 0 {
            return p;
        }
        // The instructions the fusions below absorb, skipped once they are all
        // known: the comparison check reads the `Const`s' flags alone.
        let (mut cmp_at, mut inc_at) = (None, None);
        // (d) The comparison immediately before a branch on its result.
        if let ir::Term::Branch { cond, .. } = &block.term {
            if let Some(k) = block.insts.iter().rposition(|i| i.results().contains(cond)) {
                // `k` came from a `rposition` over these same instructions and
                // `Plan::skip` has one flag per instruction, so a miss on
                // either would be this block disagreeing with itself; the
                // fusion is simply declined rather than guessed at.
                if let Some(ir::Inst::Binary {
                    op, prim, lhs, rhs, ..
                }) = block.insts.get(k)
                {
                    // Immediately before: the branch reads the operands where
                    // the comparison would have, and anything emitted between
                    // the two can reuse or clobber where that is (#297).
                    // Only a `Const` is skipped yet, so `k` is not.
                    if uses(st, *cond) == 1 && (k + 1..n).all(|j| ent(&p.skip, j, false)) {
                        let a = st.loc(*lhs).tag();
                        let b = st.loc(*rhs).tag();
                        // Fused where the library has the supernode: `sources`
                        // builds one for every operand shape of every
                        // comparison of every scalar type, and none for an
                        // operator that is not a comparison or for a `Str`.
                        let fused = prim.scalar().and_then(|s| {
                            let (tag, _, _) = super::emit::prim_tag(s);
                            self.at(&key!["brcmp/", op.name(), "/", tag, "/", a, b])
                        });
                        if let Some(at) = fused {
                            cmp_at = Some(k);
                            p.cmpbr = Some((*lhs, *rhs, at));
                        }
                    }
                }
            }
        }
        if let Some((k, incbr)) = self.incbr(code, st, block) {
            inc_at = Some(k);
            p.incbr = Some(incbr);
        }
        // (f) The tag load a switch discriminates on, folded into the first
        // comparison — the paper's `if (a[i] <op> b)` supernode, in the shape
        // this IR's `match` actually takes.
        //
        // `middle::lower` reads a `match`'s tag last thing before the switch,
        // and only the switch reads it. A merged tail-call group's dispatch
        // switch is the other kind, and it has a block with no instructions.
        // A switch with no tag load in front reads its discriminant's slot.
        let mut tag_at = None;
        if let ir::Term::Switch { .. } = &block.term {
            let k = n - 1;
            p.tagsw = match block.insts.get(k) {
                Some(ir::Inst::GetTag { agg, ty, .. }) => Some((*agg, *ty)),
                _ => None,
            };
            tag_at = p.tagsw.map(|_| k);
        }
        for (k, s) in p.skip.iter_mut().enumerate() {
            *s |= [cmp_at, inc_at, tag_at].contains(&Some(k));
        }
        p
    }
}

/// The loop test a jump to `t` can take in place of the jump: the header is
/// one instruction and a branch that passes nothing, and the jump copies
/// nothing, each argument being in its parameter's frame slot already.
pub(crate) fn copyless_test<'c>(
    code: &'c ir::Code,
    st: &Fn2,
    t: &ir::Target,
) -> Option<(&'c ir::Block, ir::ValueId, &'c ir::Target, &'c ir::Target)> {
    let header = code.get(t.block);
    let ir::Term::Branch { cond, then, else_ } = &header.term else {
        return None;
    };
    if header.insts.len() != 1 || !then.args.is_empty() || !else_.args.is_empty() {
        return None;
    }
    let frame = |v: ir::ValueId| matches!(st.home(v), Loc::Frame);
    let copies = header
        .params
        .iter()
        .zip(t.args.iter())
        .any(|(p, a)| !frame(*p) || !frame(*a) || st.at(*p) != st.at(*a));
    if copies {
        return None;
    }
    Some((header, *cond, then, else_))
}

impl Jit<'_> {
    /// The [`Incbr`] this block's jump is, and the index of the increment it
    /// absorbs.
    fn incbr(&mut self, code: &ir::Code, st: &Fn2, block: &ir::Block) -> Option<(usize, Incbr)> {
        use crate::compiler::semantics::types::Prim;
        let ir::Term::Jump(t) = &block.term else {
            return None;
        };
        // The header's own shape, and that the jump copies nothing into it,
        // is [`copyless_test`]'s, asked last: it is the dearer question.
        let header = code.get(t.block);
        let Some(ir::Inst::Binary {
            dest,
            op: ir::BinOp::Ge,
            prim: Prim::U64,
            lhs,
            rhs,
        }) = header.insts.first()
        else {
            return None;
        };
        let k = header.params.iter().position(|p| p == lhs)?;
        let m = block.insts.len().checked_sub(1)?;
        let Some(ir::Inst::Binary {
            dest: sum,
            op: ir::BinOp::Add,
            prim,
            lhs: from,
            rhs: by,
        }) = block.insts.get(m)
        else {
            return None;
        };
        // The stencil reads `from` and `bound` from their frame slots, and
        // writes the sum to its own, which the jump shares with `into`. A
        // folded constant has no slot: the step is the stencil's immediate,
        // and a bound must be a slot. Only `middle::lower`'s walks over a list
        // (`lower/lists.rs`) have this header, bounded by a length.
        let frame = |v: ir::ValueId| matches!(st.home(v), Loc::Frame);
        if !matches!(prim, Prim::I64 | Prim::U64)
            || t.args.get(k) != Some(sum)
            || !matches!(st.home(*by), Loc::Imm)
            || !frame(*from)
            || !frame(*rhs)
        {
            return None;
        }
        let (_, cond, then, else_) = copyless_test(code, st, t)?;
        if *dest != cond {
            return None;
        }
        let by = (*st.constants.get(*by))?;
        Some((
            m,
            Incbr {
                from: *from,
                by,
                into: *lhs,
                bound: *rhs,
                back: else_.block,
                out: then.block,
            },
        ))
    }
}

/// Which blocks dominate which, for [`Jit::promote`]'s candidate filter.
///
/// Rooted at a virtual block that jumps to the entry and to every block that
/// nothing jumps to, because those are what `promote`'s walk counts as a way
/// in. Built once per function, by Cooper, Harvey and Kennedy's iteration ("A
/// Simple, Fast Dominance Algorithm"), and asked in constant time.
#[derive(Default)]
struct Dominance {
    /// Each node's entry in a preorder walk of the dominator tree, and one
    /// past its last descendant's: `a` dominates `b` exactly when `b`'s entry
    /// is inside `a`'s range. `None` for a block no way in reaches.
    range: PerNode<Option<(u32, u32)>>,
    /// The walks' storage, kept from one function to the next.
    succ: Lists<Node>,
    children: Lists<Node>,
    number: PerNode<usize>,
    post: Vec<Node>,
    seen: PerNode<bool>,
    idom: PerNode<Option<Node>>,
    /// A node, the next child to visit, and the clock when it was entered.
    stack: Vec<(Node, usize, u32)>,
}

impl Dominance {
    fn compute(&mut self, code: &ir::Code, preds: &Lists<ir::BlockId>) {
        let Dominance {
            range,
            succ,
            children,
            number,
            post,
            seen,
            idom,
            stack,
        } = self;
        let root = Node::Root;
        let way_in = |x: Node| x == Node::Block(ir::BlockId::ENTRY) || preds.of(x).is_empty();
        succ.fill(
            code,
            code.iter()
                .flat_map(|(x, _)| {
                    preds
                        .of(Node::Block(x))
                        .iter()
                        .map(move |p| (Node::Block(*p), Node::Block(x)))
                })
                .chain(
                    code.iter()
                        .map(|(x, _)| Node::Block(x))
                        .filter(move |x| way_in(*x))
                        .map(move |x| (root, x)),
                ),
        );

        // Postorder from the root.
        number.refill(code, usize::MAX);
        post.clear();
        seen.refill(code, false);
        *seen.get_mut(root) = true;
        stack.clear();
        stack.push((root, 0, 0));
        while let Some((x, k, _)) = stack.pop() {
            match succ.of(x).get(k).copied() {
                Some(y) => {
                    stack.push((x, k + 1, 0));
                    if !*seen.get(y) {
                        *seen.get_mut(y) = true;
                        stack.push((y, 0, 0));
                    }
                }
                None => {
                    *number.get_mut(x) = post.len();
                    post.push(x);
                }
            }
        }

        idom.refill(code, None);
        *idom.get_mut(root) = Some(root);
        let number: &PerNode<usize> = number;
        // Every node `intersect` meets has its immediate dominator already:
        // `a` and `b` do, and so does every node above them.
        let up = |idom: &PerNode<Option<Node>>, x: Node| idom.get(x).unwrap_or(root);
        let intersect = |idom: &PerNode<Option<Node>>, mut a: Node, mut b: Node| {
            while a != b {
                while number.get(a) < number.get(b) {
                    a = up(idom, a);
                }
                while number.get(b) < number.get(a) {
                    b = up(idom, b);
                }
            }
            a
        };
        let mut changed = true;
        while changed {
            changed = false;
            // Reverse postorder, the root (which is last) left out.
            for b in post.iter().rev().skip(1).copied() {
                let from_root = way_in(b).then_some(root);
                let mut next = None;
                for p in preds.of(b).iter().map(|p| Node::Block(*p)).chain(from_root) {
                    if idom.get(p).is_none() {
                        continue;
                    }
                    next = if let Some(n) = next {
                        Some(intersect(idom, p, n))
                    } else {
                        Some(p)
                    };
                }
                if *idom.get(b) != next {
                    *idom.get_mut(b) = next;
                    changed = true;
                }
            }
        }

        // The tree, numbered in preorder. Every node of `post` is one the
        // root reaches, so it has its immediate dominator.
        let idom: &PerNode<Option<Node>> = idom;
        children.fill(code, post.iter().rev().skip(1).map(|b| (up(idom, *b), *b)));
        range.refill(code, None);
        let mut clock = 0u32;
        stack.clear();
        stack.push((root, 0, 0));
        while let Some((x, k, mut entered)) = stack.pop() {
            if k == 0 {
                entered = clock;
                clock += 1;
            }
            match children.of(x).get(k).copied() {
                Some(y) => {
                    stack.push((x, k + 1, entered));
                    stack.push((y, 0, 0));
                }
                None => *range.get_mut(x) = Some((entered, clock)),
            }
        }
    }

    /// Whether [`Jit::promote`]'s walk could accept the back edge from `p` to
    /// `h`. A `false` is certain; a `true` is for the walk to decide.
    ///
    /// The walk visits every predecessor of every block it reaches except the
    /// header, and gives up on reaching the entry or a block nothing jumps to.
    /// So it gives up exactly when one of those reaches `p` without passing
    /// `h`, which is when `h` does not dominate `p` from the virtual root. No
    /// edge enters the entry, and no block branches to itself (`ir::Code`), so
    /// `h` is neither the entry nor `p`.
    fn may_loop(&self, h: ir::BlockId, p: ir::BlockId) -> bool {
        match (
            *self.range.get(Node::Block(h)),
            *self.range.get(Node::Block(p)),
        ) {
            // No way in reaches `p`, so the walk cannot reach one either.
            (_, None) => true,
            (None, Some(_)) => false,
            (Some((lo, hi)), Some((at, _))) => lo <= at && at < hi,
        }
    }
}

/// How many times each value of `code` is read: as an instruction's operand, as
/// a terminator's, and as an edge's argument.
fn count_uses(
    code: &ir::Code,
    opnd: &Operands,
    spare: &mut ir::PerValue<u32>,
) -> ir::PerValue<u32> {
    let mut uses = code.per_value_in(spare, 0);
    for o in &opnd.ops {
        *uses.get_mut(*o) += 1;
    }
    uses
}

/// How many times `v` is read in the function `st` was built for. A value is
/// read only after the instruction that defines it, so this is every read of it
/// past that instruction.
fn uses(st: &Fn2, v: ir::ValueId) -> usize {
    *st.uses.get(v) as usize
}

/// Whether an instruction leaves the area past the frame alone — the callee's
/// frame, which [`Jit::pin_call_values`] keeps values in. Every call lays its
/// own frame out there, and so may anything that reaches a helper; what is
/// listed here writes only frame slots and scratch, or calls only C, which
/// runs on the machine stack.
fn keeps_callee_frame(i: &ir::Inst) -> bool {
    match i {
        ir::Inst::Binary { prim, .. } => !matches!(
            prim,
            crate::compiler::semantics::types::Prim::Str
                | crate::compiler::semantics::types::Prim::Template
        ),
        ir::Inst::Const { .. }
        | ir::Inst::Unary { .. }
        | ir::Inst::MakeStruct { .. }
        | ir::Inst::MakeEnum { .. }
        | ir::Inst::GetField { .. }
        | ir::Inst::GetPayload { .. }
        | ir::Inst::GetTag { .. }
        | ir::Inst::ArrayLen { .. }
        | ir::Inst::ArrayGet { .. }
        | ir::Inst::ArraySet { .. }
        | ir::Inst::ArrayPrefix { .. } => true,
        _ => false,
    }
}

fn is_barrier(i: &ir::Inst) -> bool {
    match i {
        ir::Inst::CallIntrinsic { .. } => true,
        // A comparison of two `Str`s is a *call* — `stencil_str_cmp` — and every
        // stencil that calls uses the zero-register prototype, so nothing may
        // be live in the CPS file across one. Missing this is not a slow
        // program, it is a wrong one: the register a loop variable was
        // promoted into comes back holding whatever the helper left.
        ir::Inst::Binary { prim, .. } => {
            matches!(
                prim,
                crate::compiler::semantics::types::Prim::Str
                    | crate::compiler::semantics::types::Prim::Template
            )
        }
        ir::Inst::Call { .. }
        | ir::Inst::Parallel(_)
        | ir::Inst::Handover(_)
        | ir::Inst::CallIndirect { .. }
        | ir::Inst::ShowPrim { .. }
        | ir::Inst::Abort { .. }
        | ir::Inst::MakeArray { .. }
        | ir::Inst::ArrayGet { .. }
        | ir::Inst::ArraySlice { .. }
        | ir::Inst::ArrayAlloc { .. }
        | ir::Inst::ArraySet { .. }
        | ir::Inst::ArrayPrefix { .. }
        | ir::Inst::DecRef { .. } => true,
        _ => false,
    }
}

fn literal(c: &ir::Const, ty: ir::Type) -> Option<u64> {
    Some(match c {
        ir::Const::Bool(b) => u64::from(*b),
        ir::Const::Char(ch) => u32::from(*ch) as u64,
        ir::Const::Int { bits, negative } => {
            let x = bits.get() as u64;
            if *negative {
                x.wrapping_neg()
            } else {
                x
            }
        }
        ir::Const::Float(f) => {
            if ty == ir::Type::F32 {
                (*f as f32).to_bits() as u64
            } else {
                f.to_bits()
            }
        }
        _ => return None,
    })
}

/// The symbols a stencil body may reach on its own, as opposed to through a
/// hole the emitter binds.
///
/// Every one is a symbol the linker resolves out of the runtime archive or the
/// C library — the boundary `runtime.rs` draws — so this list is checked
/// against `cli/runtime/lib.rs`'s exports by a test rather than left to a link
/// error to discover.
pub const EXTERNALS: [&str; 7] = [
    runtime::ABORT,
    runtime::ABORT_DIV_ZERO,
    runtime::ABORT_UNREACHABLE,
    runtime::ALLOC,
    runtime::FREE,
    runtime::I128_DIVMOD,
    "memcpy",
];

/// The symbol a function of this program is emitted under.
///
/// `ir::Func::symbol` and nothing else: `FuncIdx` is a whole-program index and
/// would put every unit's key in every other unit's, which is the reason
/// `ir.rs` §"a callee is named by its symbol" gives for the field existing.
pub fn symbol_of(prog: &ir::Program, f: FuncIdx) -> &str {
    &prog.funcs.get(f).symbol
}
