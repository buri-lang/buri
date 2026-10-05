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

use super::abi::{Loc, StencilTarget};
use super::object::RelKind;
use super::region::{Region, Target};
use super::library::{Hole, HoleKind, Library, Stencil};
use super::runtime;
use crate::compiler::backend::counts::{Counts, Site};
use crate::compiler::middle::ir;
use crate::compiler::middle::layout::{EnumRepr, Layout, Layouts, Repr};
use crate::compiler::semantics::types::{Tables, Ty};
use crate::hash::Map as HashMap;

// ---------------------------------------------------------------------------
// Hole values
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub enum V {
    /// A literal: a frame offset, a size, a tag, a pooled 64-bit datum.
    I(u64),
    /// A literal that **is an address inside the region** — a string literal's
    /// bytes, an abort message, a runtime helper's descriptor. Identical to
    /// `I` at emission time and distinguished from it for exactly one reason:
    /// the image cache has to know which pooled words move when the region is
    /// mapped somewhere else. See `cache.rs`.
    Ptr(u64),
    /// The address of a basic block in the function being emitted.
    Blk(u32),
    /// The entry of a function, by `FuncIdx`.
    Fn(u32),
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

#[derive(Clone, Copy)]
enum Fix {
    /// A `b`/`bl` whose target is a block of the function being emitted.
    Block { at: u64, blk: u32 },
    /// The same, for a conditional branch whose `imm19` the cond fold made a
    /// hole. See `extract::fold_cond`.
    BlockCond { at: u64, blk: u32 },
    /// A `b`/`bl` whose target is a function entry.
    Func { at: u64, f: u32 },
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
    pub(crate) lib: &'a Library,
    /// Which machine's fields are being patched.
    ///
    /// The one axis this file has: a stencil's bytes are clang's, so *where* a
    /// hole is and *what* a patch writes are the instruction set's business
    /// and nothing else's. `abi::StencilTarget::is_arm64` is the question, and
    /// every place below that asks it says what the two answers are.
    pub(crate) target: StencilTarget,
    pub region: Region,
    layouts: Layouts<'a>,
    /// The type tables, for the questions a `Layout` does not answer — which
    /// primitive a `TypeId` is (`Inst::Structural`), and what a closure type's
    /// parameter and result types are (the open-coded `list.*` loops).
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
    frames: &'a [FrameSig],
    /// Section offset of each function of *this unit*; zero for a function the
    /// unit does not own. It is what the object's symbol table is written from
    /// — not what a call site reads, because every call is a relocation
    /// (see [`Jit::resolve`]).
    entries: Vec<u64>,
    fixups: Vec<Fix>,
    /// The IR shapes this emitter refused, in the order they were first met.
    /// A unit with any of them produces no object: a refusal is a diagnostic
    /// naming the shape, never an artifact that aborts when it reaches it.
    reasons: Vec<String>,
    /// Whether the region may be appended to right now. A conditional branch
    /// out of range needs a veneer, and a veneer may only be planted between
    /// functions — never in the middle of one, where it would land inside the
    /// fallthrough of the stencil being patched.
    veneer_ok: bool,
    /// Per function: did anything in it compile to an `unsupported` stencil.
    dirty: Vec<bool>,
    current: usize,
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
}

impl<'a> Scratch<'a> {
    pub(crate) fn new(
        tables: &'a Tables,
        cycles: std::sync::Arc<crate::compiler::middle::layout::Cycles>,
    ) -> Scratch<'a> {
        Scratch { layouts: Layouts::with_cycles(tables, cycles), counts: Counts::default() }
    }
}

#[derive(Clone, Default)]
pub(crate) struct FrameSig {
    pub ret: Vec<u32>,
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
// Every table the three analyses below build has exactly one entry per value or
// per block of the `ir::Code` in hand, and every index into one is a `ValueId`
// or a `BlockId` of that same `code`. An index past the end would mean the IR
// disagreed with itself, so the fallbacks these three take are values no read
// can actually produce: they are here so that consulting a table is not a
// panic, and each call site that leans on one says which way it leans.

/// Entry `i` of a side table, or `d` when the table has none.
fn ent<T: Copy>(t: &[T], i: usize, d: T) -> T {
    t.get(i).copied().unwrap_or(d)
}

/// Sets entry `i` of a side table, where the table has one.
fn put<T>(t: &mut [T], i: usize, x: T) {
    if let Some(e) = t.get_mut(i) {
        *e = x;
    }
}

/// One more use recorded against entry `i`.
fn bump(t: &mut [u32], i: usize) {
    if let Some(e) = t.get_mut(i) {
        *e += 1;
    }
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

/// The uses of one value within one block, for [`Jit::regalloc`] and
/// [`Jit::pin_call_values`].
#[derive(Clone, Copy)]
struct BlockUses {
    /// The block this entry describes; any other block reads it as unused.
    block: u32,
    count: u32,
    /// Instruction indices, the terminator being one past the last.
    first: usize,
    last: usize,
}

impl Default for BlockUses {
    fn default() -> BlockUses {
        BlockUses { block: u32::MAX, count: 0, first: 0, last: 0 }
    }
}

/// Records a use of `o` at instruction `at` of block `block`. Uses arrive in
/// instruction order, so the latest is the last.
fn note_use(uses: &mut [BlockUses], block: u32, o: &ir::ValueId, at: usize) {
    if let Some(u) = uses.get_mut(o.index()) {
        if u.block == block {
            u.count += 1;
            u.last = at;
        } else {
            *u = BlockUses { block, count: 1, first: at, last: at };
        }
    }
}

/// The representative of `v`'s slot class.
///
/// `uf` starts as the identity and [`Jit::coalesce`] only ever points an entry
/// at the index of a *root*, so the walk stays inside the vector and ends at a
/// value that is its own parent. An index the vector does not hold is in no
/// class and is its own root.
fn find(uf: &[u32], mut v: u32) -> u32 {
    while let Some(&parent) = uf.get(v as usize) {
        if parent == v {
            break;
        }
        v = parent;
    }
    v
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
        lib: &'a Library,
        tables: &'a Tables,
        frames: &'a [FrameSig],
        target: StencilTarget,
        scratch: Scratch<'a>,
        part: usize,
    ) -> Jit<'a> {
        Jit {
            lib,
            target,
            region: Region::new(),
            layouts: scratch.layouts,
            tables,
            frames,
            entries: Vec::new(),
            fixups: Vec::new(),
            reasons: Vec::new(),
            veneer_ok: false,
            dirty: Vec::new(),
            current: 0,
            helpers: Vec::new(),
            helper_ix: HashMap::default(),
            helper_names: Vec::new(),
            shared_ix: HashMap::default(),
            helper_at: Vec::new(),
            spilled: HashMap::default(),
            counts: scratch.counts,
            part,
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
        Scratch { layouts: self.layouts, counts: self.counts }
    }

    /// The symbol of a generated helper, registering it the first time it is
    /// asked for.
    ///
    /// Drop and copy glue gets its program-wide name
    /// ([`super::glue::shared_symbol`]); everything else is a local symbol of
    /// this part.
    pub(crate) fn helper(&mut self, h: super::glue::Helper) -> String {
        if let Some(i) = self.helper_ix.get(&h) {
            return self.helper_names.get(*i).map(|(n, _)| n.clone()).unwrap_or_default();
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
        self.helper_at.push((0, 0));
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

    pub(crate) fn has(&self, key: &str) -> bool {
        self.lib.get(key).is_some()
    }

    /// The name of the hole whose branch is a two-target stencil's **last**
    /// instruction — the only arm copy-and-patch can elide. Which one it is is
    /// clang's layout decision, not the emitter's, and it flips with the
    /// comparison; `None` when the two twins disagree, so that the caller never
    /// has to know which one `emit` will pick.
    ///
    /// Borrowed out of the library rather than copied out of it, and the fold
    /// twin asked for by index: this is called twice per conditional branch
    /// the backend emits, and it used to allocate a `String` for the name, a
    /// second for the comparison and a third for `key+fold`.
    pub(crate) fn elidable_arm(&self, key: &str) -> Option<&'a str> {
        let (at, _) = self.lib.at(key)?;
        self.elidable_at(at)
    }

    /// [`Jit::elidable_arm`], of the stencil at `at` in the library.
    pub(crate) fn elidable_at(&self, at: usize) -> Option<&'a str> {
        let s = self.lib.stencil(at)?;
        let n = s.holes.get(s.tail?)?.name.as_str();
        if let Some(f) = self.lib.fold_twin(at, super::library::FOLD_PLAIN) {
            if f.holes.get(f.tail?).map(|h| h.name.as_str()) != Some(n) {
                return None;
            }
        }
        Some(n)
    }

    pub(crate) fn push_reason(&mut self, why: String) -> u64 {
        if let Some(d) = self.dirty.get_mut(self.current) {
            *d = true;
        }
        if let Some(i) = self.reasons.iter().position(|r| *r == why) {
            return i as u64;
        }
        self.reasons.push(why);
        (self.reasons.len() - 1) as u64
    }

    /// `f`'s frame layout, borrowed from the program's table. [`frame_sigs`]
    /// fills it with one entry per function of the program, so the empty
    /// signature is what a `FuncIdx` from some other program would get and not
    /// one from this one.
    pub(crate) fn frame_sig_of(&self, f: usize) -> &'a FrameSig {
        static EMPTY: FrameSig =
            FrameSig { ret: Vec::new(), ret_size: 0, params: Vec::new(), param_end: 0, size: 0 };
        self.frames.get(f).unwrap_or(&EMPTY)
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
            ir::Type::Agg(id) => self.layout_of(prog, id).size,
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
    /// them once and every part borrows the same slice. What is left is the two
    /// vectors that are genuinely this part's — where each function was laid
    /// out, and whether it has been emitted — and both are a `memset`.
    pub fn plan(&mut self, prog: &ir::Program) {
        self.entries = vec![0; prog.funcs.len()];
        self.dirty = vec![false; prog.funcs.len()];
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
pub(crate) fn frame_sigs(prog: &ir::Program, tables: &Tables) -> Vec<FrameSig> {
    let mut layouts = Layouts::new(tables);
    let width = |l: &mut Layouts, t: ir::Type| -> u32 {
        match t {
            ir::Type::I1 | ir::Type::I8 => 1,
            ir::Type::I16 => 2,
            ir::Type::I32 | ir::Type::F32 => 4,
            ir::Type::I64 | ir::Type::F64 | ir::Type::Ptr => 8,
            ir::Type::I128 => 16,
            ir::Type::Unit => 0,
            ir::Type::Agg(id) => l.shared(&prog.type_info(id).ty).size,
        }
    };
    let mut out = Vec::with_capacity(prog.funcs.len());
    for f in &prog.funcs {
        let mut fs = FrameSig::default();
        let mut at = 0u32;
        for t in &f.sig.rets {
            fs.ret.push(at);
            at += round8(width(&mut layouts, *t)).max(8);
        }
        fs.ret_size = at;
        for t in &f.sig.params {
            fs.params.push(at);
            at += round8(width(&mut layouts, *t)).max(8);
        }
        fs.param_end = at;
        if let ir::Body::Code(code) = &f.body {
            let entry_params: Vec<u32> =
                code.get(ir::BlockId(0)).params.iter().map(|v| v.0).collect();
            for v in 0..code.values() {
                let t = code.ty_of(ir::ValueId(v as u32));
                if entry_params.contains(&(v as u32)) {
                    continue;
                }
                at += round8(width(&mut layouts, t)).max(8);
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

/// Per-function emission state.
pub(crate) struct Fn2 {
    /// Byte offset of every value's frame slot.
    pub slot: Vec<u32>,
    /// The address each block was laid out at. Entries past the IR's own
    /// blocks are the synthetic labels an edge's copies need.
    pub blk: Vec<u64>,
    pub frame: FrameSig,
    pub scratch: u32,
    /// CPS register assignment, when the level has one.
    pub reg: Vec<Option<Loc>>,
    /// For a value promoted across block boundaries: whether its frame slot has
    /// to be kept in step with its register, because something that cannot read
    /// a register reads it.
    pub wt: Vec<bool>,
    /// Which values [`Jit::promote`] put in a register. Their register holds
    /// the value inside [`Fn2::region`] and nowhere else.
    pub cross: Vec<bool>,
    /// The blocks of [`Jit::promote`]'s region, one entry each.
    pub region: Vec<bool>,
    /// The block being emitted, as an index into [`Fn2::region`].
    pub cur: usize,
    /// The literal a value holds, when it is an `Inst::Const` a stencil can
    /// take as an immediate.
    pub constants: Vec<Option<u64>>,
    /// Values whose every use is an immediate operand: the `Const` that
    /// defines them is never materialised into a frame slot at all.
    pub folded: Vec<bool>,
}

impl Fn2 {
    pub fn label(&mut self) -> u32 {
        self.blk.push(0);
        (self.blk.len() - 1) as u32
    }
    pub fn place(&mut self, l: u32, at: u64) {
        put(&mut self.blk, l as usize, at);
    }
    /// The byte offset of `v`'s frame slot.
    ///
    /// `slot` is `Jit::slots`' answer and therefore has one entry per value of
    /// the `code` `v` came from, so the fallback is an offset no call can
    /// actually ask for; it is here so that reading a slot cannot be a panic.
    /// Offset zero is the first word of the return area, which is inside every
    /// frame this backend builds.
    pub fn at(&self, v: ir::ValueId) -> u32 {
        self.slot.get(v.index()).copied().unwrap_or(0)
    }
    /// Where a value **lives**: a CPS register if one was assigned, the frame
    /// otherwise. This is the paper's "whether it operates on constants,
    /// registers, or stack locations".
    ///
    /// The home is where a definition writes and where an edge copy lands, and
    /// it is the same answer everywhere in the function. Where a *read* takes
    /// the value from is [`Fn2::loc`], which is not.
    pub fn home(&self, v: ir::ValueId) -> Loc {
        self.reg.get(v.index()).copied().flatten().unwrap_or(Loc::Frame)
    }

    /// Where a read in the block being emitted takes a value from.
    ///
    /// [`Jit::promote`]'s register is only the value inside its region: the
    /// region is barrier-free and enterable only at its header, and neither
    /// holds one block further out. So a read outside it takes the frame slot,
    /// which `wt` had the edge keep in step for exactly this — the other half
    /// of the promotion, and the half that was missing. Without it a
    /// comparison one block past the loop read a register a call had already
    /// overwritten, and a `Bool` walk that recursed from two places answered
    /// `true` for every input (buri-lang/buri#47).
    pub fn loc(&self, v: ir::ValueId) -> Loc {
        if self.cross.get(v.index()).copied().unwrap_or(false)
            && !self.region.get(self.cur).copied().unwrap_or(false)
        {
            return Loc::Frame;
        }
        self.home(v)
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
    pub fn compile_part(&mut self, prog: &ir::Program, members: &[usize]) {
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
            put(&mut self.helper_at, i, (at, self.region.code_addr()));
            i += 1;
        }
        self.resolve(prog);
    }

    fn function(&mut self, prog: &ir::Program, fi: usize) {
        self.current = fi;
        self.region.align_code(4);
        let Some(f) = prog.funcs.get(fi) else {
            crate::diagnostics::ice(&format!(
                "stencil: unit member {fi} is past the program's {} functions",
                prog.funcs.len()
            ));
        };
        // `plan` sized `entries` and `frames` from the same `prog.funcs`, so
        // the check above is what makes both of these present.
        let entry = self.region.code_addr();
        put(&mut self.entries, fi, entry);
        let frame = self.frames.get(fi).cloned().unwrap_or_default();

        match &f.body {
            ir::Body::Code(code) => {
                let (slot, scratch) = self.slots(prog, code, &frame);
                let mut reg = vec![None; code.values()];
                let mut wt = vec![true; code.values()];
                let mut cross = vec![false; code.values()];
                let mut promoted = Vec::new();
                let taken = self.promote(code, &mut reg, &mut wt, &mut cross, &mut promoted);
                self.regalloc(code, &mut reg, taken);
                let (constants, folded) = self.constants(code);
                let mut st = Fn2 {
                    slot,
                    blk: vec![0; code.blocks.len()],
                    frame,
                    scratch,
                    reg,
                    wt,
                    cross,
                    region: promoted,
                    cur: 0,
                    constants,
                    folded,
                };
                let base = self.fixups.len();
                let order = self.layout(code);
                for (oi, bi) in order.iter().copied().enumerate() {
                    let here = self.region.code_addr();
                    put(&mut st.blk, bi, here);
                    // Which side of the promotion's region every read in this
                    // block is on. See [`Fn2::loc`].
                    st.cur = bi;
                    let block = code.get(ir::BlockId(bi as u32));
                    let plan = self.plan_block(code, &st, block);
                    // `Plan::skip` is built with one flag per instruction of
                    // this block, so the zip drops nothing.
                    for (inst, skip) in block.insts.iter().zip(plan.skip.iter()) {
                        if *skip {
                            continue;
                        }
                        self.inst(prog, code, &mut st, inst);
                    }
                    let next = order.get(oi + 1).copied().unwrap_or(usize::MAX);
                    // An `Abort` does not come back, so the terminator behind
                    // one is dead code.
                    if !matches!(block.insts.last(), Some(ir::Inst::Abort { .. }))
                    {
                        self.term(prog, code, &mut st, next, &block.term, &plan);
                    }
                }
                self.resolve_blocks(base, &st.blk);
            }
            // A runtime-supplied body has no IR to walk, but its sequence may
            // still branch, which needs labels and the same function-local
            // branch resolution a real body gets. So it is given an empty
            // `Fn2` whose frame is its own signature's and whose scratch is the
            // area `Jit::plan` reserved past the parameters.
            ir::Body::Runtime(key) => {
                let mut st = Fn2 {
                    slot: Vec::new(),
                    blk: Vec::new(),
                    frame: frame.clone(),
                    scratch: frame.param_end,
                    reg: Vec::new(),
                    wt: Vec::new(),
                    cross: Vec::new(),
                    region: Vec::new(),
                    cur: 0,
                    constants: Vec::new(),
                    folded: Vec::new(),
                };
                let base = self.fixups.len();
                self.runtime_body(prog, fi, key.clone(), &mut st);
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
    fn resolve_blocks(&mut self, base: usize, blk: &[u64]) {
        self.veneer_ok = true;
        let mut i = base;
        while let Some(fix) = self.fixups.get(i).copied() {
            // `blk` holds every block of the function plus every synthetic
            // label `Fn2::label` handed out, and a block fixup names one of
            // those, so address zero is a target no fixup here can carry.
            match fix {
                Fix::Block { at, blk: b } => {
                    self.patch_branch(at, ent(blk, b as usize, 0));
                    self.fixups.swap_remove(i);
                }
                Fix::BlockCond { at, blk: b } => {
                    self.patch_cond(at, ent(blk, b as usize, 0));
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
    pub(crate) fn emit(&mut self, key: &str, binds: &[(&str, V)]) {
        let lib: &'a Library = self.lib;
        let (at, mut s) = match lib.at(key) {
            Some(found) => found,
            None => crate::diagnostics::ice(&format!(
                "stencil: no stencil {key} in {}",
                lib.config
            )),
        };
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
            let Some(f) = lib.fold_twin(at, k) else { continue };
            let fits = f.holes.iter().all(|h| {
                h.lo12.is_empty()
                    || match binds.iter().find(|(n, _)| *n == h.name.as_str()) {
                        Some((_, V::I(v))) => h
                            .lo12
                            .iter()
                            .all(|(_, sc)| v % u64::from(*sc) == 0 && v / u64::from(*sc) < 4096),
                        _ => false,
                    }
            });
            if fits {
                s = f;
                break;
            }
        }
        // The hole on the stencil's last instruction, once the fold above has
        // settled which twin is being copied.
        let tail_name: Option<&str> =
            s.tail.and_then(|t| s.holes.get(t)).map(|h| h.name.as_str());
        let mut len = s.code.len();
        // Fallthrough elision: the continuation is the next stencil, so the
        // trailing branch is not patched, it is dropped. One A64 instruction,
        // or the five bytes of an x86-64 `jmp rel32`.
        let tail_bytes = if self.target.is_arm64() { 4 } else { 5 };
        let mut elide = false;
        if let Some(name) = tail_name {
            if binds.iter().any(|(n, v)| *n == name && matches!(v, V::Fall)) {
                elide = true;
                len -= tail_bytes;
            }
        }
        let Some(bytes) = s.code.get(..len) else {
            crate::diagnostics::ice(&format!(
                "stencil: stencil {key} is {} bytes, shorter than the tail branch it names",
                s.code.len()
            ));
        };
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
        for h in &s.holes {
            if elide && tail_name == Some(h.name.as_str()) {
                continue;
            }
            let bound = binds.iter().find(|(n, _)| *n == h.name.as_str()).map(|(_, v)| v.clone());
            let Some(v) = bound else {
                // A hole the caller did not name is a symbol the stencil body
                // reached on its own — an abort, `buri_rt_free`, `memcpy` —
                // and becomes an import. The generated C declares nothing
                // undefined but the `_JIT_*` holes and `cli/runtime/lib.rs`'s
                // exports, so the name *is* the symbol; `EXTERNALS` records the
                // set for a test to check rather than for this to consult.
                if h.name.starts_with("JIT_") {
                    crate::diagnostics::ice(&format!(
                        "stencil: stencil {key} has an unbound hole {}",
                        h.name
                    ));
                }
                self.import(at, h);
                continue;
            };
            self.patch_hole(at, end, h, v);
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
    /// and no instruction rewritten. A `bl` becomes a `BRANCH26`; the address
    /// of one, materialised into the constant pool, becomes an `Abs64`.
    fn import(&mut self, at: u64, h: &Hole) {
        let name = h.name.clone();
        for off in &h.branches {
            self.branch_reloc(at + *off as u64, Target::Symbol(name.clone()));
        }
        if !h.pairs.is_empty() {
            let slot = self.region.pool_target(Target::Symbol(name));
            for (a, b) in &h.pairs {
                self.pool_ref(at, *a, *b, slot);
            }
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

    /// The reference to a constant-pool slot one of a hole's `pairs` becomes.
    ///
    /// `pairs` means `(adrp, add-or-ldr)` on A64 and `(instruction end,
    /// field)` on x86-64 — the two ISAs never share a patcher, and this is
    /// where the two meanings are consumed.
    fn pool_ref(&mut self, at: u64, a: u32, b: u32, slot: u64) {
        if self.target.is_arm64() {
            self.region.pool_ref(at + a as u64, at + b as u64, slot);
            return;
        }
        self.region.pool_ref_pc32(at + b as u64, at + a as u64, slot);
    }

    fn patch_hole(&mut self, at: u64, end: u64, h: &Hole, v: V) {
        match h.kind {
            HoleKind::Branch => {
                let target = match v {
                    V::I(x) | V::Ptr(x) => Some(x),
                    // A call out of the program: `bl` a symbol the linker
                    // resolves. The prototype planted a veneer holding the
                    // address of a function in its own process; there is no
                    // such address here, and a relocation is both simpler and
                    // one instruction shorter.
                    V::Ext(n) => {
                        for off in &h.branches {
                            self.branch_reloc(at + *off as u64, Target::Symbol(String::from(n)));
                        }
                        None
                    }
                    V::Sym(ref n) => {
                        for off in &h.branches {
                            self.branch_reloc(at + *off as u64, Target::Symbol(n.clone()));
                        }
                        None
                    }
                    // The stencil laid out immediately after this one, which
                    // is the address one past this body — *not* one past this
                    // branch, which for a two-target stencil is the other arm.
                    V::Fall => Some(end),
                    V::Blk(b) => {
                        for off in &h.branches {
                            self.fixups.push(Fix::Block { at: at + *off as u64, blk: b });
                        }
                        for off in &h.conds {
                            self.fixups.push(Fix::BlockCond { at: at + *off as u64, blk: b });
                        }
                        None
                    }
                    V::Fn(f) => {
                        for off in &h.branches {
                            self.fixups.push(Fix::Func { at: at + *off as u64, f });
                        }
                        None
                    }
                };
                if let Some(t) = target {
                    for off in &h.branches {
                        self.patch_branch(at + *off as u64, t);
                    }
                    for off in &h.conds {
                        self.patch_cond(at + *off as u64, t);
                    }
                }
            }
            HoleKind::Imm32 => {
                let V::I(x) = v else {
                    crate::diagnostics::ice(&format!("stencil: hole {} takes a literal", h.name));
                };
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
            HoleKind::Imm64 => {
                if !self.target.is_arm64() {
                    self.patch_imm64_x86_64(at, h, v);
                    return;
                }
                // The immediate fold may have taken this hole into an `imm12`
                // field, in which case there is no pair left to relax.
                if !h.lo12.is_empty() {
                    // The stencil builder only ever produces `lo12` for
                    // offset-shaped holes, so anything else here is a library
                    // that does not match this emitter.
                    let V::I(x) = v else {
                        crate::diagnostics::ice(&format!(
                            "stencil: hole {} was folded into an imm12 but is not an offset",
                            h.name
                        ));
                    };
                    for (o, scale) in &h.lo12 {
                        self.patch_lo12(at + *o as u64, x as u32, *scale);
                    }
                    if h.pairs.is_empty() {
                        return;
                    }
                }
                // A value that fits 32 bits does not need the pool at all: the
                // GOT form is two instructions with one destination register,
                // and so is `movz`/`movk`. This is the same relaxation a linker
                // does, and it takes a load off every immediate operand.
                if let V::I(x) = v {
                    if x < (1u64 << 32)
                        && h.pairs.iter().all(|(a, b)| {
                            let wa = self.region.word_at(at + *a as u64);
                            let wb = self.region.word_at(at + *b as u64);
                            (wa & 0x1f) == (wb & 0x1f) && (wa & 0x1f) == ((wb >> 5) & 0x1f)
                        })
                    {
                        for (a, b) in &h.pairs {
                            self.patch_imm32(at + *a as u64, at + *b as u64, x as u32);
                        }
                        return;
                    }
                }
                let slot = match v {
                    V::I(x) => self.region.pool_u64(x),
                    // A byte inside this section, whose base the linker picks.
                    V::Ptr(x) => self.region.pool_target(Target::Here(x)),
                    V::Fn(f) => self.region.pool_target(Target::Func(f)),
                    V::Ext(n) => self.region.pool_target(Target::Symbol(String::from(n))),
                    V::Sym(ref n) => self.region.pool_target(Target::Symbol(n.clone())),
                    other => crate::diagnostics::ice(&format!(
                        "stencil: hole {} takes a datum, got {other:?}",
                        h.name
                    )),
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
    fn patch_imm64_x86_64(&mut self, at: u64, h: &Hole, v: V) {
        // The same relaxation A64 takes, and for the same reason: a value that
        // fits 32 bits does not need the pool, and dropping the load takes an
        // L1 access off every immediate operand. It is only sound where the
        // instruction is a plain `mov rD, [rip+disp32]` — a float immediate
        // arrives as `movsd` and a small comparison as `cmpl $0, …`, and
        // neither may become a `mov` of a literal.
        if let V::I(x) = v {
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
        let slot = match v {
            V::I(x) => self.region.pool_u64(x),
            V::Ptr(x) => self.region.pool_target(Target::Here(x)),
            V::Fn(f) => self.region.pool_target(Target::Func(f)),
            V::Ext(n) => self.region.pool_target(Target::Symbol(String::from(n))),
            V::Sym(ref n) => self.region.pool_target(Target::Symbol(n.clone())),
            other => crate::diagnostics::ice(&format!(
                "stencil: hole {} takes a datum, got {other:?}",
                h.name
            )),
        };
        for (insn_end, field) in &h.pairs {
            self.pool_ref(at, *insn_end, *field, slot);
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
        if field < 3 {
            crate::diagnostics::ice("stencil: a pc-relative field with no instruction in front");
        }
        let start = field - 3;
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
        assert!((-(1 << 25)..(1 << 25)).contains(&w), "branch out of range: {d}");
        let old = self.region.word_at(at);
        self.region.set_word(at, (old & 0xfc00_0000) | (w as u32 & 0x03ff_ffff));
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
        self.region.set_word(at, (old & 0xff00_001f) | (((w as u32) & 0x7ffff) << 5));
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
        self.region.set_word(adrp, 0xd280_0000 | ((v & 0xffff) << 5) | rd);
        self.region.set_word(add, 0xf2a0_0000 | (((v >> 16) & 0xffff) << 5) | rd);
    }

    /// The `imm12` field of a load or store the library builder folded the hole
    /// into. See `extract::fold_addressing`.
    fn patch_lo12(&mut self, at: u64, v: u32, scale: u32) {
        debug_assert_eq!(v % scale, 0);
        let imm12 = v / scale;
        debug_assert!(imm12 < 4096);
        let w = self.region.word_at(at);
        self.region.set_word(at, (w & !(0xfff << 10)) | (imm12 << 10));
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
            self.branch_reloc(at, Target::Symbol(name));
        }
    }

    /// Whether a function, or anything reachable from it, contains an
    /// `unsupported` stencil — the honest predicate for "this test can be run".
    ///
    /// **This part's** answer, and a unit is emitted in parts (`mod.rs`), so a
    /// caller wanting the unit's would have to `or` the parts' vectors together
    /// before running the fixpoint. Nothing asks today: the emission path reads
    /// [`Jit::reasons`] instead, which `assemble_unit` does collect across the
    /// parts, and refuses the whole unit where any part refused anything.
    pub fn reachable_dirty(&self, prog: &ir::Program) -> Vec<bool> {
        let n = prog.funcs.len();
        let mut edges: Vec<Vec<u32>> = vec![Vec::new(); n];
        for (f, out) in prog.funcs.iter().zip(edges.iter_mut()) {
            let ir::Body::Code(code) = &f.body else { continue };
            for b in &code.blocks {
                for inst in &b.insts {
                    match inst {
                        ir::Inst::Call { func, .. } => out.push(func.0),
                        ir::Inst::MakeClosure { func, .. } => out.push(func.0),
                        ir::Inst::DecRef { drop: Some(g), .. } => out.push(g.0),
                        // An indirect call can reach anything a closure was
                        // made of, and `MakeClosure` already recorded those.
                        _ => {}
                    }
                }
            }
        }
        let mut bad = self.dirty.clone();
        let mut changed = true;
        while changed {
            changed = false;
            // `bad` is `self.dirty`, which `plan` sized from the same
            // `prog.funcs` this counted, and `edges` has one entry per
            // function too — so "not dirty" is what a missing entry means and
            // the fixpoint still terminates: `changed` is only set where a
            // flag was actually written.
            for i in 0..n {
                if ent(&bad, i, false) {
                    continue;
                }
                let reaches = edges
                    .get(i)
                    .is_some_and(|es| es.iter().any(|c| ent(&bad, *c as usize, false)));
                if reaches {
                    if let Some(b) = bad.get_mut(i) {
                        *b = true;
                        changed = true;
                    }
                }
            }
        }
        bad
    }

    /// Where `f` was emitted inside this **part's** region. `plan` gives every
    /// function of the program an entry — zero for one this part does not own,
    /// which is also what a `FuncIdx` from outside the program would read.
    /// `mod.rs::assemble_unit` adds the part's base to make it the unit's.
    pub fn entry_of(&self, f: usize) -> u64 {
        ent(&self.entries, f, 0)
    }
    pub fn reasons(&self) -> &[String] {
        &self.reasons
    }
}

// ---------------------------------------------------------------------------
// The three analyses a level's stencils are worth having
// ---------------------------------------------------------------------------

/// What a block's terminator may absorb from the block's tail.
pub(crate) struct Plan {
    pub skip: Vec<bool>,
    /// The comparison the branch will do itself: `(op, prim, lhs, rhs)`.
    pub cmpbr: Option<(ir::BinOp, crate::compiler::semantics::types::Prim, ir::ValueId, ir::ValueId)>,
    /// A `GetTag` the switch will do itself: `(aggregate value, byte offset,
    /// tag width)`.
    pub tagsw: Option<ir::ValueId>,
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
    fn slots(
        &mut self,
        prog: &ir::Program,
        code: &ir::Code,
        frame: &FrameSig,
    ) -> (Vec<u32>, u32) {
        let n = code.values();
        let entry: Vec<ir::ValueId> = code.get(ir::BlockId(0)).params.clone();
        let mut pin: Vec<Option<u32>> = vec![None; n];
        for (k, v) in entry.iter().enumerate() {
            put(&mut pin, v.index(), frame.params.get(k).copied());
        }
        let width: Vec<u32> =
            (0..n).map(|v| self.slot_bytes(prog, code.ty_of(ir::ValueId(v as u32)))).collect();
        let mut uf: Vec<u32> = (0..n as u32).collect();
        {
            self.coalesce(code, &mut uf, &mut pin, &width);
        }
        self.pin_call_values(prog, code, &uf, &mut pin, frame.size);
        // One slot per class: the pinned offset when the class holds a
        // parameter or a return value, a fresh one otherwise.
        let mut at = frame.param_end;
        let mut of_root: Vec<Option<u32>> = vec![None; n];
        for (v, pinned) in pin.iter().enumerate() {
            let Some(p) = *pinned else { continue };
            let r = find(&uf, v as u32) as usize;
            let Some(root) = of_root.get_mut(r) else { continue };
            // Two pinned offsets in one class would mean two values with
            // fixed, different homes had been merged, and the second would
            // silently win. `coalesce` refuses those merges; this says so
            // rather than trusting it.
            if root.is_some_and(|o| o != p) {
                crate::diagnostics::ice(&format!(
                    "stencil: slot class {r} is pinned at two offsets ({root:?} and {p})"
                ));
            }
            *root = Some(p);
        }
        let mut wide: Vec<u32> = vec![0; n];
        for (v, w) in width.iter().enumerate() {
            let r = find(&uf, v as u32) as usize;
            if let Some(x) = wide.get_mut(r) {
                *x = (*x).max(*w);
            }
        }
        let mut slot = vec![0u32; n];
        for (v, s) in slot.iter_mut().enumerate() {
            let r = find(&uf, v as u32) as usize;
            let off = match ent(&of_root, r, None) {
                Some(o) => o,
                None => {
                    let o = at;
                    at += ent(&wide, r, 0);
                    put(&mut of_root, r, Some(o));
                    o
                }
            };
            *s = off;
        }
        self.alias_parts(prog, code, &uf, &pin, &mut slot);
        (slot, at)
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
    fn alias_parts(
        &mut self,
        prog: &ir::Program,
        code: &ir::Code,
        uf: &[u32],
        pin: &[Option<u32>],
        slot: &mut [u32],
    ) {
        let part = |i: &ir::Inst| {
            matches!(i, ir::Inst::ArrayLen { .. } | ir::Inst::MakeStruct { .. } | ir::Inst::MakeEnum { .. })
        };
        if !code.blocks.iter().any(|b| b.insts.iter().any(part)) {
            return;
        }
        let n = code.values();
        let mut members = vec![0u32; n];
        let mut uses = vec![0u32; n];
        let mut param = vec![false; n];
        let mut ops = Vec::new();
        for v in 0..n {
            bump(&mut members, find(uf, v as u32) as usize);
        }
        for (bi, b) in code.blocks.iter().enumerate() {
            if bi != 0 {
                for p in &b.params {
                    put(&mut param, p.index(), true);
                }
            }
            for i in &b.insts {
                ops.clear();
                i.operands(&mut ops);
                for o in &ops {
                    bump(&mut uses, o.index());
                }
            }
            ops.clear();
            b.term.operands(&mut ops);
            for t in b.term.targets() {
                ops.extend_from_slice(&t.args);
            }
            for o in &ops {
                bump(&mut uses, o.index());
            }
        }
        let alone = |v: ir::ValueId| {
            find(uf, v.0) == v.0
                && ent(&members, v.index(), 0) == 1
                && ent(pin, v.index(), None).is_none()
        };
        // The entry's parameters keep the slots the caller left them in.
        let entry: Vec<ir::ValueId> = code.get(ir::BlockId(0)).params.clone();
        let still = |v: ir::ValueId| {
            find(uf, v.0) == v.0
                && ent(&members, v.index(), 0) == 1
                && !ent(&param, v.index(), true)
                && (ent(pin, v.index(), None).is_none() || entry.contains(&v))
        };
        let mut aliased = vec![false; n];
        for b in &code.blocks {
            if !b.insts.iter().any(part) {
                continue;
            }
            let def_at: HashMap<u32, usize> = b
                .insts
                .iter()
                .enumerate()
                .flat_map(|(k, i)| i.results().iter().map(move |d| (d.0, k)))
                .collect();
            for (j, i) in b.insts.iter().enumerate() {
                if let ir::Inst::ArrayLen { dest, array } = i {
                    if alone(*dest) && still(*array) {
                        put(slot, dest.index(), ent(slot, array.index(), 0) + 8);
                        put(&mut aliased, dest.index(), true);
                    }
                    continue;
                }
                let (dest, fields, offs, owner, ftys) = match i {
                    ir::Inst::MakeStruct { dest, fields } => {
                        let ir::Type::Agg(id) = code.ty_of(*dest) else { continue };
                        let owner = prog.type_info(id).ty;
                        let l = self.layout_of(prog, id);
                        let ftys = crate::compiler::semantics::types::field_types(self.tables, &owner);
                        (*dest, fields, l.fields.clone(), owner, ftys)
                    }
                    ir::Inst::MakeEnum { dest, variant, fields } => {
                        let ir::Type::Agg(id) = code.ty_of(*dest) else { continue };
                        let owner = prog.type_info(id).ty;
                        let l = self.layout_of(prog, id);
                        if matches!(&l.repr, Repr::Enum { repr: EnumRepr::Bare { .. }, .. }) {
                            continue;
                        }
                        let ftys = crate::compiler::semantics::types::variant_types(
                            self.tables,
                            &owner,
                            *variant as usize,
                        );
                        (*dest, fields, l.variant(*variant as usize).to_vec(), owner, ftys)
                    }
                    _ => continue,
                };
                if ent(pin, dest.index(), None).is_some() {
                    continue;
                }
                let root = find(uf, dest.0);
                let in_class = |v: &ir::ValueId| find(uf, v.0) == root;
                for (fi, f) in fields.iter().enumerate() {
                    let Some(&off) = offs.get(fi) else { continue };
                    let w = self.width(prog, code.ty_of(*f));
                    if w == 0
                        || !w.is_multiple_of(8)
                        || ftys.get(fi).is_some_and(|t| self.boxes(&owner, t))
                        || !alone(*f)
                        || ent(&aliased, f.index(), true)
                        || ent(&uses, f.index(), 0) != 1
                    {
                        continue;
                    }
                    let Some(&k) = def_at.get(&f.0).filter(|k| **k < j) else { continue };
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
                        let mut ops = Vec::new();
                        x.operands(&mut ops);
                        ops.iter().any(&in_class) || x.results().iter().any(&in_class)
                    };
                    if def.is_some_and(touches) || b.insts.iter().take(j).skip(k + 1).any(touches) {
                        continue;
                    }
                    put(slot, f.index(), ent(slot, dest.index(), 0) + off);
                    put(&mut aliased, f.index(), true);
                }
            }
        }
    }

    /// The merges themselves. See [`Jit::slots`].
    fn coalesce(
        &mut self,
        code: &ir::Code,
        uf: &mut [u32],
        pin: &mut [Option<u32>],
        width: &[u32],
    ) {
        let n = code.values();
        let mut uses = vec![0u32; n];
        let mut is_param = vec![false; n];
        let mut def_block = vec![u32::MAX; n];
        let mut def_idx = vec![u32::MAX; n];
        let mut ops = Vec::new();
        for (bi, b) in code.blocks.iter().enumerate() {
            for p in &b.params {
                put(&mut is_param, p.index(), true);
                put(&mut def_block, p.index(), bi as u32);
            }
            for (k, i) in b.insts.iter().enumerate() {
                ops.clear();
                i.operands(&mut ops);
                for o in &ops {
                    bump(&mut uses, o.index());
                }
                for d in i.results() {
                    put(&mut def_block, d.index(), bi as u32);
                    put(&mut def_idx, d.index(), k as u32);
                }
            }
            ops.clear();
            b.term.operands(&mut ops);
            for t in b.term.targets() {
                ops.extend_from_slice(&t.args);
            }
            for o in &ops {
                bump(&mut uses, o.index());
            }
        }
        let mut used_here: std::collections::HashSet<(u32, u32)> = std::collections::HashSet::new();
        for (bi, b) in code.blocks.iter().enumerate() {
            // (i.a) An edge's block arguments.
            let mut pairs: Vec<(ir::ValueId, ir::ValueId)> = Vec::new();
            for t in b.term.targets() {
                for (p, a) in code.get(t.block).params.iter().zip(t.args.iter()) {
                    pairs.push((*p, *a));
                }
            }
            // (i.b) A `Return`'s move into the return area, which is the same
            // copy with a fixed destination.
            let rets: Vec<(u32, ir::ValueId)> = match &b.term {
                ir::Term::Return(vs) => vs
                    .iter()
                    .enumerate()
                    .filter_map(|(i, v)| {
                        self.frames
                            .get(self.current)
                            .and_then(|fs| fs.ret.get(i))
                            .map(|o| (*o, *v))
                    })
                    .collect(),
                _ => Vec::new(),
            };
            // Every table below has one entry per value of `code`, and `pi`
            // and `ai` are values of `code`, so the fallbacks are what a value
            // from some other function would read: a width no slot has, a
            // definition in no block, and no uses — each of which declines the
            // merge rather than making one on a guess.
            for (p, a) in pairs {
                let (pi, ai) = (p.index(), a.index());
                if ent(width, ai, 0) != ent(width, pi, 0) {
                    continue;
                }
                // Only a temporary defined in this block, used exactly once,
                // and not already merged, is a candidate — an entry parameter
                // too, because its slot is where the caller left it and the
                // class can simply take that.
                let def_a = ent(&def_block, ai, u32::MAX);
                let entry_ok = ent(pin, ai, None).is_some() && def_a == 0 && bi == 0;
                if !entry_ok && (ent(&is_param, ai, true) || def_a != bi as u32) {
                    continue;
                }
                if ent(&uses, ai, 0) != 1 || find(uf, ai as u32) != ai as u32 {
                    continue;
                }
                let root = find(uf, pi as u32);
                if root == ai as u32 {
                    continue;
                }
                if ent(pin, ai, None).is_some()
                    && (0..n).any(|v| find(uf, v as u32) == root && ent(pin, v, None).is_some())
                {
                    continue; // two pinned slots cannot be one slot
                }
                if !used_here.insert((bi as u32, root)) {
                    continue;
                }
                if !self.merge_is_safe(code, b, uf, root, a, ent(&def_idx, ai, u32::MAX)) {
                    continue;
                }
                put(uf, ai, root);
            }
            for (off, v) in rets {
                let vi = v.index();
                if ent(width, vi, 0) != 8
                    || ent(&is_param, vi, true)
                    || ent(&def_block, vi, u32::MAX) != bi as u32
                    || ent(&uses, vi, 0) != 1
                    || find(uf, vi as u32) != vi as u32
                    || ent(pin, vi, None).is_some()
                {
                    continue;
                }
                if !self.merge_is_safe(code, b, uf, vi as u32, v, ent(&def_idx, vi, u32::MAX)) {
                    continue;
                }
                // A class of one, pinned at the return area.
                put(pin, vi, Some(off));
            }
        }
        self.coalesce_latches(code, uf, pin, width, &uses, &def_block, &def_idx);
    }

    /// (i.d) A **latch**: a block whose parameter is only passed on to the
    /// loop header's parameter. The paths that meet there each pass the
    /// header's own value or a temporary computed from it, so the latch's
    /// parameter can take the header parameter's slot and every copy on the
    /// way round the loop becomes the identity.
    ///
    /// The one exception to "never two parameters", and safe for the same
    /// reason the rest is: the latch reads nothing in the header parameter's
    /// class, so the only write that moves earlier is an edge copy at the end
    /// of a predecessor, after which nothing on that path reads the old value
    /// before the header takes the new one. Every temporary already merged into
    /// the latch's class is checked again against the header's.
    #[allow(clippy::too_many_arguments, reason = "the tables `coalesce` already built")]
    fn coalesce_latches(
        &mut self,
        code: &ir::Code,
        uf: &mut [u32],
        pin: &[Option<u32>],
        width: &[u32],
        uses: &[u32],
        def_block: &[u32],
        def_idx: &[u32],
    ) {
        let candidate = |b: &ir::Block| match &b.term {
            ir::Term::Jump(t) => b.params.iter().any(|p| t.args.contains(p)),
            _ => false,
        };
        if !code.blocks.iter().any(candidate) {
            return;
        }
        let n = code.values();
        // Each class's members and whether one is pinned, kept up to date as
        // classes merge, so a candidate costs its own class and not the code.
        let mut members: Vec<Vec<u32>> = vec![Vec::new(); n];
        let mut pinned = vec![false; n];
        for v in 0..n {
            let r = find(uf, v as u32) as usize;
            if let Some(m) = members.get_mut(r) {
                m.push(v as u32);
            }
            if ent(pin, v, None).is_some() {
                put(&mut pinned, r, true);
            }
        }
        for latch in &code.blocks {
            let ir::Term::Jump(t) = &latch.term else { continue };
            let header = code.get(t.block);
            for p2 in &latch.params {
                let Some(k) = t.args.iter().position(|a| a == p2) else { continue };
                let Some(&p1) = header.params.get(k) else { continue };
                if ent(uses, p2.index(), 0) != 1
                    || ent(width, p2.index(), 0) != 8
                    || ent(width, p1.index(), 0) != 8
                    || ent(pin, p2.index(), None).is_some()
                {
                    continue;
                }
                let (r1, r2) = (find(uf, p1.0), find(uf, p2.0));
                if r1 == r2 || ent(&pinned, r1 as usize, true) || ent(&pinned, r2 as usize, true) {
                    continue;
                }
                // Nothing in the latch reads the header parameter's class, and
                // the jump passes it nothing but this one value.
                let mut ops = Vec::new();
                for i in &latch.insts {
                    i.operands(&mut ops);
                }
                latch.term.operands(&mut ops);
                if ops.iter().any(|o| find(uf, o.0) == r1)
                    || t.args.iter().filter(|a| find(uf, a.0) == r1 || find(uf, a.0) == r2).count() != 1
                {
                    continue;
                }
                let class = members.get(r2 as usize).cloned().unwrap_or_default();
                let safe = class.iter().all(|m| {
                    if *m == p2.0 {
                        return true;
                    }
                    let b = ent(def_block, *m as usize, u32::MAX);
                    let Some(block) = code.blocks.get(b as usize) else { return false };
                    // A parameter of some other block in the class is not a
                    // shape this reasons about.
                    if block.params.iter().any(|p| p.0 == *m) {
                        return false;
                    }
                    // Its definition now writes the header parameter's slot,
                    // so the block must have no way out but to the latch.
                    if !matches!(&block.term, ir::Term::Jump(j) if std::ptr::eq(code.get(j.block), latch)) {
                        return false;
                    }
                    self.merge_is_safe(code, block, uf, r1, ir::ValueId(*m), ent(def_idx, *m as usize, u32::MAX))
                });
                if !safe {
                    continue;
                }
                put(uf, r2 as usize, r1);
                if let Some(m) = members.get_mut(r1 as usize) {
                    m.extend(class);
                }
            }
        }
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
    fn pin_call_values(
        &mut self,
        prog: &ir::Program,
        code: &ir::Code,
        uf: &[u32],
        pin: &mut [Option<u32>],
        frame_size: u32,
    ) {
        let has_call = |b: &ir::Block| b.insts.iter().any(|i| matches!(i, ir::Inst::Call { .. }));
        if !code.blocks.iter().any(has_call) {
            return;
        }
        let n = code.values();
        let mut members = vec![0u32; n];
        let mut uses = vec![0u32; n];
        let mut ops = Vec::new();
        for v in 0..n {
            bump(&mut members, find(uf, v as u32) as usize);
        }
        for b in &code.blocks {
            for i in &b.insts {
                ops.clear();
                i.operands(&mut ops);
                for o in &ops {
                    bump(&mut uses, o.index());
                }
            }
            ops.clear();
            b.term.operands(&mut ops);
            for t in b.term.targets() {
                ops.extend_from_slice(&t.args);
            }
            for o in &ops {
                bump(&mut uses, o.index());
            }
        }
        let alone = |v: ir::ValueId, pin: &[Option<u32>]| {
            find(uf, v.0) == v.0
                && ent(&members, v.index(), 0) == 1
                && ent(pin, v.index(), None).is_none()
        };
        let calls_code = |func: &crate::compiler::semantics::types::FuncIdx| {
            matches!(prog.funcs.get(func.index()).map(|f| &f.body), Some(ir::Body::Code(_)))
        };
        // Where each value is read in the block at hand — an instruction's
        // index, or `last` for the terminator — and where it is defined. One
        // entry per value, stamped with the block it describes.
        let mut reads = vec![BlockUses::default(); n];
        let mut defs: Vec<(u32, usize)> = vec![(u32::MAX, 0); n];
        let mut kept: Vec<usize> = Vec::new();
        for (bi, b) in code.blocks.iter().enumerate() {
            if !has_call(b) {
                continue;
            }
            let stamp = u32::try_from(bi).unwrap_or(u32::MAX);
            let last = b.insts.len();
            // How many instructions before each index leave the callee's
            // frame alone, so a range is pure in one subtraction.
            kept.clear();
            kept.resize(last + 1, 0);
            for (k, i) in b.insts.iter().enumerate() {
                for d in i.results() {
                    put(&mut defs, d.index(), (stamp, k));
                }
                let so_far = ent(&kept, k, 0) + usize::from(keeps_callee_frame(i));
                put(&mut kept, k + 1, so_far);
            }
            for (k, i) in b.insts.iter().enumerate() {
                ops.clear();
                i.operands(&mut ops);
                for o in &ops {
                    note_use(&mut reads, stamp, o, k);
                }
            }
            ops.clear();
            b.term.operands(&mut ops);
            for t in b.term.targets() {
                ops.extend_from_slice(&t.args);
            }
            for o in &ops {
                note_use(&mut reads, stamp, o, last);
            }
            let read_at = |v: ir::ValueId| reads.get(v.index()).copied().filter(|u| u.block == stamp);
            let def_at = |v: ir::ValueId| {
                defs.get(v.index()).filter(|(b, _)| *b == stamp).map(|(_, k)| *k)
            };
            let pure = |from: usize, to: usize| {
                to <= from || ent(&kept, to, 0) - ent(&kept, from, 0) == to - from
            };
            let mut results: Vec<(usize, usize)> = Vec::new();
            for (j, i) in b.insts.iter().enumerate() {
                let ir::Inst::Call { dests, func, .. } = i else { continue };
                if !calls_code(func) {
                    continue;
                }
                let Some(fs) = self.frames.get(func.index()) else { continue };
                let (Some(&d), Some(&off)) = (dests.first(), fs.ret.first()) else { continue };
                let Some(dr) = read_at(d) else { continue };
                let mut end = dr.last;
                if !alone(d, pin) || dr.count != ent(&uses, d.index(), 0) || dr.first <= j {
                    continue;
                }
                // A `Bool` read only to be counted shares the slot it is
                // counted from: the conversion is a copy of the same word.
                let mut alias = None;
                if dr.count == 1 {
                    let r = dr.first;
                    if let Some(ir::Inst::Unary { op: ir::UnOp::FromBool, dest: z, .. }) =
                        b.insts.get(r)
                    {
                        if let Some(zr) = read_at(*z) {
                            if alone(*z, pin)
                                && zr.count == ent(&uses, z.index(), 0)
                                && zr.first > r
                            {
                                end = end.max(zr.last);
                                alias = Some(*z);
                            }
                        }
                    }
                }
                if !pure(j + 1, (end + 1).min(last)) {
                    continue;
                }
                put(pin, d.index(), Some(frame_size + off));
                if let Some(z) = alias {
                    put(pin, z.index(), Some(frame_size + off));
                }
                results.push((j, end));
            }
            for (j, i) in b.insts.iter().enumerate() {
                let ir::Inst::Call { func, args, .. } = i else { continue };
                if !calls_code(func) {
                    continue;
                }
                let Some(fs) = self.frames.get(func.index()) else { continue };
                for (a, off) in args.iter().zip(fs.params.iter()) {
                    let Some(k) = def_at(*a).filter(|k| *k < j) else { continue };
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
                    if !defines
                        || !alone(*a, pin)
                        || ent(&uses, a.index(), 0) != 1
                        || !pure(k + 1, j)
                        || results.iter().any(|(rj, re)| *rj < k && k <= *re)
                    {
                        continue;
                    }
                    put(pin, a.index(), Some(frame_size + off));
                }
            }
        }
    }

    /// Whether nothing in `root`'s class is read or written between `a`'s
    /// definition and the end of the block.
    fn merge_is_safe(
        &self,
        code: &ir::Code,
        b: &ir::Block,
        uf: &[u32],
        root: u32,
        a: ir::ValueId,
        from: u32,
    ) -> bool {
        let _ = code;
        let mut ops = Vec::new();
        for i in b.insts.iter().skip(from as usize + 1) {
            ops.clear();
            i.operands(&mut ops);
            if ops.iter().any(|o| find(uf, o.0) == root) {
                return false;
            }
            if i.results().iter().any(|d| find(uf, d.0) == root) {
                return false;
            }
        }
        ops.clear();
        b.term.operands(&mut ops);
        for t in b.term.targets() {
            ops.extend_from_slice(&t.args);
        }
        // The terminator reads `a` itself once; anything else in the class is
        // a conflict.
        ops.iter().filter(|o| find(uf, o.0) == root || **o == a).count() <= 1
    }

    /// (k) The order the blocks are laid out in.
    ///
    /// Copy-and-patch's fallthrough elision only pays when the block a branch
    /// goes to is the block that comes next, and IR order is not that order:
    /// `middle::lower` emits a loop as header, exit, body, so the *taken* arm of
    /// every test is the one the loop takes every iteration. Reverse postorder
    /// — depth-first, `then` before `else`, reversed — puts the else-arm and
    /// the loop body immediately after the test, which is where the elision
    /// wants them. Blocks the walk never reaches keep IR order at the end.
    fn layout(&self, code: &ir::Code) -> Vec<usize> {
        let nb = code.blocks.len();
        let mut seen = vec![false; nb];
        let mut post = Vec::with_capacity(nb);
        // An explicit stack, because a deeply nested function would blow a
        // recursive one and this runs on every function in the program.
        let mut stack: Vec<(usize, usize)> = Vec::new();
        // Block zero is the entry, and a body with no blocks has nothing to
        // walk from.
        if let Some(s) = seen.first_mut() {
            *s = true;
            stack.push((0, 0));
        }
        while let Some((b, k)) = stack.pop() {
            // The `k`th successor alone: a switch's block is popped once per
            // case, and listing all of them each time was quadratic in its
            // width.
            let succ = code.blocks.get(b).and_then(|block| block.term.target(k));
            match succ.map(|t| t.block.index()) {
                Some(s) => {
                    stack.push((b, k + 1));
                    // A successor `seen` does not hold is a target outside
                    // this function's blocks, and treating it as already
                    // visited is what keeps the walk inside them.
                    if !ent(&seen, s, true) {
                        put(&mut seen, s, true);
                        stack.push((s, 0));
                    }
                }
                None => post.push(b),
            }
        }
        post.reverse();
        for (b, s) in seen.iter().enumerate() {
            if !*s {
                post.push(b);
            }
        }
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
        out: &mut [Option<Loc>],
        wt: &mut [bool],
        cross: &mut [bool],
        region: &mut Vec<bool>,
    ) -> (usize, usize) {
        let nb = code.blocks.len();
        let register_count = super::abi::CPS_REGISTER_COUNT;
        if register_count < 2 || nb == 0 {
            return (0, 0);
        }
        // Only a back edge into a block with parameters can make a candidate
        // below, and most functions have none: they skip the tables.
        let back_edge = code.blocks.iter().enumerate().any(|(p, b)| {
            b.term.targets().any(|t| t.block.index() <= p && !code.get(t.block).params.is_empty())
        });
        if !back_edge {
            return (0, 0);
        }
        let barrier: Vec<bool> =
            code.blocks.iter().map(|b| b.insts.iter().any(is_barrier)).collect();
        let mut preds: Vec<Vec<usize>> = vec![Vec::new(); nb];
        for (bi, b) in code.blocks.iter().enumerate() {
            for t in b.term.targets() {
                if let Some(ps) = preds.get_mut(t.block.index()) {
                    ps.push(bi);
                }
            }
        }
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
        let dom = Dominance::of(&preds);
        // One side table for every candidate, told apart by a stamp, and the
        // members listed beside it: a table per candidate was the other half
        // of the quadratic.
        let mut stamp: Vec<u32> = vec![0; nb];
        let mut epoch = 0u32;
        let mut members: Vec<usize> = Vec::new();
        let mut stack: Vec<usize> = Vec::new();
        let mut best: Option<Vec<usize>> = None;
        let mut best_h = 0usize;
        for (p, b) in code.blocks.iter().enumerate() {
            for t in b.term.targets() {
                let h = t.block.index();
                if h > p {
                    continue;
                }
                if code.get(ir::BlockId(h as u32)).params.is_empty() {
                    continue;
                }
                if !dom.may_loop(h, p) {
                    continue;
                }
                epoch += 1;
                let inside = |stamp: &[u32], x: usize| ent(stamp, x, epoch) == epoch;
                members.clear();
                put(&mut stamp, h, epoch);
                members.push(h);
                if p != h {
                    put(&mut stamp, p, epoch);
                    members.push(p);
                }
                stack.clear();
                stack.push(p);
                let mut ok = true;
                while let Some(x) = stack.pop() {
                    if x == h {
                        continue;
                    }
                    let Some(ps) = preds.get(x) else {
                        ok = false;
                        break;
                    };
                    if ps.is_empty() {
                        ok = false; // reached the entry: not a natural loop
                        break;
                    }
                    for q in ps {
                        if !inside(&stamp, *q) {
                            put(&mut stamp, *q, epoch);
                            members.push(*q);
                            stack.push(*q);
                        }
                    }
                }
                if !ok || inside(&stamp, 0) {
                    continue;
                }
                // Every way into the loop is through the header.
                let closed = members.iter().all(|x| {
                    *x == h
                        || preds.get(*x).is_some_and(|ps| ps.iter().all(|q| inside(&stamp, *q)))
                });
                if !closed {
                    continue;
                }
                if members.iter().any(|x| ent(&barrier, *x, false)) {
                    continue;
                }
                if best.as_ref().is_none_or(|bb| members.len() < bb.len()) {
                    best = Some(members.clone());
                    best_h = h;
                }
            }
        }
        let Some(best) = best else { return (0, 0) };
        let h = best_h;
        let mut body = vec![false; nb];
        for x in best {
            put(&mut body, x, true);
        }
        region.clone_from(&body);

        // Where every value is used, and whether every one of those uses can
        // read a register.
        let params = code.get(ir::BlockId(h as u32)).params.clone();
        let n = code.values();
        let mut reg_ok = vec![true; n];
        let mut uses = vec![0u32; n];
        let mut ops = Vec::new();
        for (bi, b) in code.blocks.iter().enumerate() {
            let inside = ent(&body, bi, false);
            for i in &b.insts {
                ops.clear();
                i.operands(&mut ops);
                let ok = inside
                    && matches!(i, ir::Inst::Binary { .. } | ir::Inst::Unary { .. })
                    && !matches!(i, ir::Inst::Unary { op: ir::UnOp::FromBool, .. });
                for o in &ops {
                    bump(&mut uses, o.index());
                    if !ok {
                        put(&mut reg_ok, o.index(), false);
                    }
                }
            }
            ops.clear();
            b.term.operands(&mut ops);
            let ok = inside && matches!(b.term, ir::Term::Branch { .. });
            for o in &ops {
                bump(&mut uses, o.index());
                if !ok {
                    put(&mut reg_ok, o.index(), false);
                }
            }
            // An edge argument is a register use only when it lands in the same
            // register it already sits in, which the edge emitter turns into
            // nothing at all.
            for t in b.term.targets() {
                for (p, a) in code.get(t.block).params.iter().zip(t.args.iter()) {
                    bump(&mut uses, a.index());
                    if !(t.block.index() == h && *p == *a) {
                        put(&mut reg_ok, a.index(), false);
                    }
                }
            }
        }

        let (mut ri, mut rf) = (0usize, 0usize);
        for p in &params {
            // One register has to be left for the expression temporaries the
            // paper's own allocator keeps there, or the arithmetic inside the
            // loop loses more than the loop variable gains.
            let ty = code.ty_of(*p);
            let float = ty == ir::Type::F64;
            let scalar = matches!(
                ty,
                ir::Type::I64 | ir::Type::Ptr | ir::Type::I1 | ir::Type::I8
                    | ir::Type::I16 | ir::Type::I32 | ir::Type::F64
            );
            if !scalar || ent(&uses, p.index(), 0) == 0 {
                continue;
            }
            let k = if float { &mut rf } else { &mut ri };
            if *k + 1 >= register_count {
                continue;
            }
            put(out, p.index(), Some(Loc::Reg(*k as u8)));
            put(cross, p.index(), true);
            // A value no `reg_ok` entry covers is one nothing in this function
            // reads, so writing its slot through is the conservative answer.
            put(wt, p.index(), !ent(&reg_ok, p.index(), false));
            *k += 1;
        }
        // The value the back edge hands the parameter belongs in the parameter's
        // own register, or the loop-carried chain still goes through memory:
        // `add x8, x2, #2 ; str x8, [fp] ; ldr x2, [fp]` instead of
        // `add x2, x2, #2`. Measured, this is the whole of the difference —
        // without it the promotion is 36% *slower* than leaving the variable in
        // the frame, because it adds a reload to a chain that already had a
        // store-to-load forward in it.
        for (bi, b) in code.blocks.iter().enumerate() {
            if !ent(&body, bi, false) {
                continue;
            }
            for t in b.term.targets() {
                if t.block.index() != h {
                    continue;
                }
                let ps = code.get(t.block).params.clone();
                for (p, a) in ps.iter().zip(t.args.iter()) {
                    let Some(Loc::Reg(k)) = ent(out, p.index(), None) else { continue };
                    if p == a
                        || ent(out, a.index(), None).is_some()
                        || ent(&uses, a.index(), 0) != 1
                    {
                        continue;
                    }
                    let float = code.ty_of(*a) == ir::Type::F64;
                    if float != (code.ty_of(*p) == ir::Type::F64) {
                        continue;
                    }
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
                        Some(ir::Inst::Unary { op: ir::UnOp::FromBool, .. })
                    ) {
                        continue;
                    }
                    let mut ops = Vec::new();
                    let mut clash = false;
                    for i in b.insts.iter().skip(di + 1) {
                        ops.clear();
                        i.operands(&mut ops);
                        clash |= ops.iter().any(|o| ent(out, o.index(), None) == Some(Loc::Reg(k)));
                    }
                    ops.clear();
                    b.term.operands(&mut ops);
                    for tt in b.term.targets() {
                        for (pp, aa) in code.get(tt.block).params.iter().zip(tt.args.iter()) {
                            // The one occurrence that *is* this hand-over is
                            // fine; any other read of the register is not.
                            if !(std::ptr::eq(tt, t) && pp == p && aa == a) {
                                ops.push(*aa);
                            }
                        }
                    }
                    clash |= ops.iter().any(|o| ent(out, o.index(), None) == Some(Loc::Reg(k)));
                    if clash {
                        continue;
                    }
                    put(out, a.index(), Some(Loc::Reg(k)));
                    put(cross, a.index(), true);
                    put(wt, a.index(), false);
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
    fn regalloc(&mut self, code: &ir::Code, out: &mut [Option<Loc>], taken: (usize, usize)) {
        // Uses over the **whole function**, not just the defining block. A
        // value defined in one block is visible to every block it dominates,
        // and a register only survives to the end of its own block — so a value
        // with any out-of-block use has to stay in the frame. Counting uses
        // per block instead was a miscompile: the consumer read a frame slot
        // the producer had never written.
        let mut total = vec![0u32; code.values()];
        let mut tmp = Vec::new();
        for b in &code.blocks {
            for i in &b.insts {
                tmp.clear();
                i.operands(&mut tmp);
                for o in &tmp {
                    bump(&mut total, o.index());
                }
            }
            tmp.clear();
            b.term.operands(&mut tmp);
            for t in b.term.targets() {
                tmp.extend_from_slice(&t.args);
            }
            for o in &tmp {
                bump(&mut total, o.index());
            }
        }
        // Where each value is used in the block being allocated: how often,
        // first and last. One entry per value of the function, stamped with
        // the block it describes, so moving to the next block resets nothing.
        let mut uses = vec![BlockUses::default(); code.values()];
        let mut barrier: Vec<bool> = Vec::new();
        let mut ops = Vec::new();
        // The registers cross-block promotion took are not the local
        // allocator's to hand out.
        let register_count = super::abi::CPS_REGISTER_COUNT;
        let mut busy: Vec<Option<u32>> = vec![None; register_count];
        let mut busyf: Vec<Option<u32>> = vec![None; register_count];
        for (bi, block) in code.blocks.iter().enumerate() {
            let n = block.insts.len();
            let stamp = u32::try_from(bi).unwrap_or(u32::MAX);
            // Where the barriers are. A barrier is an instruction whose
            // stencil has the zero-register prototype and therefore clobbers
            // the file.
            barrier.clear();
            barrier.resize(n + 1, false);
            for (k, i) in block.insts.iter().enumerate() {
                ops.clear();
                i.operands(&mut ops);
                for o in &ops {
                    note_use(&mut uses, stamp, o, k);
                }
                put(&mut barrier, k, is_barrier(i));
            }
            ops.clear();
            block.term.operands(&mut ops);
            for t in block.term.targets() {
                ops.extend_from_slice(&t.args);
            }
            for o in &ops {
                note_use(&mut uses, stamp, o, n);
            }
            // The uses of `v` in this block, if it has any.
            let used = |v: u32| uses.get(v as usize).copied().filter(|u| u.block == stamp);

            for (r, b) in busy.iter_mut().enumerate() {
                *b = (r < taken.0).then_some(u32::MAX);
            }
            for (r, b) in busyf.iter_mut().enumerate() {
                *b = (r < taken.1).then_some(u32::MAX);
            }
            let (base, basef) = (taken.0, taken.1);
            for (k, i) in block.insts.iter().enumerate() {
                // Free every register whose value was last used here.
                for (slot, b) in busy.iter_mut().chain(busyf.iter_mut()).enumerate() {
                    let _ = slot;
                    if let Some(v) = *b {
                        if v == u32::MAX {
                            continue; // a promoted register, not this pass's
                        }
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
                        *b = (r < base).then_some(u32::MAX);
                    }
                    for (r, b) in busyf.iter_mut().enumerate() {
                        *b = (r < basef).then_some(u32::MAX);
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
                        let f = matches!(prim, crate::compiler::semantics::types::Prim::F32 | crate::compiler::semantics::types::Prim::F64)
                            && !op.is_comparison();
                        (f, *dest)
                    }
                    ir::Inst::Unary { dest, prim, op, .. }
                        if !wide(prim) && *op != ir::UnOp::FromBool =>
                    (
                        matches!(prim, crate::compiler::semantics::types::Prim::F32 | crate::compiler::semantics::types::Prim::F64),
                        *dest,
                    ),
                    _ => continue,
                };
                if ent(out, dest.index(), None).is_some() {
                    continue; // already promoted across the loop
                }
                let Some(u) = used(dest.0) else { continue };
                if u.count != 1 || ent(&total, dest.index(), 0) != 1 {
                    continue;
                }
                let at = u.first;
                // `barrier` runs from the block's first instruction to one
                // past its last, so a use ahead of `k` always names a span
                // inside it. A use behind `k` names an empty range instead,
                // which is the same answer the `at <= k` test gives: no
                // register.
                let Some(span) = barrier.get(k..=at.min(n)) else { continue };
                if at <= k || span.iter().any(|b| *b) {
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
                        Some(ir::Inst::Unary { op: ir::UnOp::FromBool, .. })
                    )
                };
                if !consumes {
                    continue;
                }
                let file = if float { &mut busyf } else { &mut busy };
                if let Some(r) = file.iter().position(|b| b.is_none()) {
                    let _ = (base, basef);
                    put(file, r, Some(dest.0));
                    put(out, dest.index(), Some(Loc::Reg(r as u8)));
                }
            }
        }
    }

    /// Which `Inst::Const`s never need a frame slot, because every use of them
    /// is an immediate operand of a stencil that has an immediate variant.
    ///
    /// [`zero_divisor`] is the one use that is *not* eligible however good the
    /// stencil is.
    fn constants(&mut self, code: &ir::Code) -> (Vec<Option<u64>>, Vec<bool>) {
        let mut constants: Vec<Option<u64>> = vec![None; code.values()];
        let mut folded = vec![false; code.values()];
        for block in &code.blocks {
            for i in &block.insts {
                if let ir::Inst::Const { dest, value } = i {
                    put(&mut constants, dest.index(), literal(value, code.ty_of(*dest)));
                }
            }
        }
        // A use is immediate-eligible only as the right operand of a binary
        // operation whose immediate variant this level has.
        let mut total = vec![0u32; code.values()];
        let mut imm = vec![0u32; code.values()];
        let mut ops = Vec::new();
        for block in &code.blocks {
            for i in &block.insts {
                ops.clear();
                i.operands(&mut ops);
                for o in &ops {
                    bump(&mut total, o.index());
                }
                if let ir::Inst::Binary { op, prim, rhs, .. } = i {
                    if let Some(k) = ent(&constants, rhs.index(), None) {
                        if let Some((tag, _, _)) = super::emit::prim_tag(*prim) {
                            let name = super::emit::binop_name(*op);
                            let key = format!("bin/{name}/{tag}/fi/f");
                            if self.has(&key) && !zero_divisor(name, tag, k) {
                                bump(&mut imm, rhs.index());
                            }
                        }
                    }
                }
            }
            ops.clear();
            block.term.operands(&mut ops);
            for t in block.term.targets() {
                ops.extend_from_slice(&t.args);
            }
            for o in &ops {
                bump(&mut total, o.index());
            }
        }
        for (v, f) in folded.iter_mut().enumerate() {
            let seen = ent(&total, v, 0);
            *f = ent(&constants, v, None).is_some() && seen > 0 && seen == ent(&imm, v, 0);
        }
        (constants, folded)
    }

    /// Fusions the terminator can absorb.
    pub(super) fn plan_block(&mut self, code: &ir::Code, st: &Fn2, block: &ir::Block) -> Plan {
        let n = block.insts.len();
        let mut p = Plan { skip: vec![false; n], cmpbr: None, tagsw: None, incbr: None };
        // Every `Const` whose uses are all immediates disappears.
        for (i, skip) in block.insts.iter().zip(p.skip.iter_mut()) {
            if let ir::Inst::Const { dest, .. } = i {
                if ent(&st.folded, dest.index(), false) {
                    *skip = true;
                }
            }
        }
        if n == 0 {
            return p;
        }
        // (d) The comparison immediately before a branch on its result.
        if let ir::Term::Branch { cond, .. } = &block.term {
            if let Some(k) = block.insts.iter().rposition(|i| i.results().contains(cond)) {
                // `k` came from a `rposition` over these same instructions and
                // `Plan::skip` has one flag per instruction, so a miss on
                // either would be this block disagreeing with itself; the
                // fusion is simply declined rather than guessed at.
                if let Some(ir::Inst::Binary { op, prim, lhs, rhs, .. }) = block.insts.get(k) {
                    if op.is_comparison()
                        && uses_after(code, block, *cond, k) == 1
                        && !ent(&p.skip, k, true)
                    {
                        let a = st.loc(*lhs).tag();
                        let b = if ent(&st.folded, rhs.index(), false) {
                            "i".into()
                        } else {
                            st.loc(*rhs).tag()
                        };
                        if let Some((tag, _, _)) = super::emit::prim_tag(*prim) {
                            let key = format!(
                                "brcmp/{}/{tag}/{a}{b}",
                                super::emit::binop_name(*op)
                            );
                            if self.has(&key) {
                                put(&mut p.skip, k, true);
                                p.cmpbr = Some((*op, *prim, *lhs, *rhs));
                            }
                        }
                    }
                }
            }
        }
        if let Some((k, incbr)) = self.incbr(code, st, block) {
            put(&mut p.skip, k, true);
            p.incbr = Some(incbr);
        }
        // (f) The tag load a switch discriminates on, folded into the first
        // comparison — the paper's `if (a[i] <op> b)` supernode, in the shape
        // this IR's `match` actually takes.
        {
            if let ir::Term::Switch { on, .. } = &block.term {
                if let Some(k) = block.insts.iter().rposition(|i| i.results().contains(on)) {
                    if let Some(ir::Inst::GetTag { agg, .. }) = block.insts.get(k) {
                        if uses_after(code, block, *on, k) == 1 && !ent(&p.skip, k, true) {
                            put(&mut p.skip, k, true);
                            p.tagsw = Some(*agg);
                        }
                    }
                }
            }
        }
        p
    }
}

impl Jit<'_> {
    /// The [`Incbr`] this block's jump is, and the index of the increment it
    /// absorbs.
    fn incbr(&mut self, code: &ir::Code, st: &Fn2, block: &ir::Block) -> Option<(usize, Incbr)> {
        use crate::compiler::semantics::types::Prim;
        if !self.has("incbr/lt") {
            return None;
        }
        let ir::Term::Jump(t) = &block.term else { return None };
        let header = code.get(t.block);
        let ir::Term::Branch { cond, then, else_ } = &header.term else { return None };
        if header.insts.len() != 1 || !then.args.is_empty() || !else_.args.is_empty() {
            return None;
        }
        let Some(ir::Inst::Binary { dest, op: ir::BinOp::Ge, prim: Prim::U64, lhs, rhs }) =
            header.insts.first()
        else {
            return None;
        };
        if dest != cond {
            return None;
        }
        let k = header.params.iter().position(|p| p == lhs)?;
        let m = block.insts.len().checked_sub(1)?;
        let Some(ir::Inst::Binary { dest: sum, op: ir::BinOp::Add, prim, lhs: from, rhs: by }) =
            block.insts.get(m)
        else {
            return None;
        };
        let frame = |v: ir::ValueId| matches!(st.home(v), Loc::Frame);
        if !matches!(prim, Prim::I64 | Prim::U64)
            || t.args.get(k) != Some(sum)
            || !ent(&st.folded, by.index(), false)
            || ent(&st.folded, rhs.index(), false)
            || !frame(*from)
            || !frame(*rhs)
            || uses_after(code, block, *sum, m) != 1
            || header.params.iter().zip(t.args.iter()).any(|(p, a)| {
                !frame(*p) || !frame(*a) || st.at(*p) != st.at(*a)
            })
        {
            return None;
        }
        let by = st.constants.get(by.index()).copied().flatten()?;
        Some((
            m,
            Incbr { from: *from, by, into: *lhs, bound: *rhs, back: else_.block, out: then.block },
        ))
    }
}

/// Which blocks dominate which, for [`Jit::promote`]'s candidate filter.
///
/// Rooted at a virtual block that jumps to the entry and to every block that
/// nothing jumps to, because those are what `promote`'s walk counts as a way
/// in. Built once per function, by Cooper, Harvey and Kennedy's iteration ("A
/// Simple, Fast Dominance Algorithm"), and asked in constant time.
struct Dominance {
    /// Each block's entry in a preorder walk of the dominator tree, and one
    /// past its last descendant's: `a` dominates `b` exactly when `b`'s entry
    /// is inside `a`'s range. `None` for a block no way in reaches.
    range: Vec<Option<(u32, u32)>>,
}

impl Dominance {
    fn of(preds: &[Vec<usize>]) -> Dominance {
        let nb = preds.len();
        let root = nb;
        let way_in = |x: usize| x == 0 || preds.get(x).is_some_and(Vec::is_empty);
        let mut succ: Vec<Vec<usize>> = vec![Vec::new(); nb + 1];
        for (x, ps) in preds.iter().enumerate() {
            for p in ps {
                if let Some(s) = succ.get_mut(*p) {
                    s.push(x);
                }
            }
        }
        if let Some(s) = succ.get_mut(root) {
            s.extend((0..nb).filter(|x| way_in(*x)));
        }

        // Postorder from the root.
        const NONE: usize = usize::MAX;
        let mut number = vec![NONE; nb + 1];
        let mut post: Vec<usize> = Vec::with_capacity(nb + 1);
        let mut seen = vec![false; nb + 1];
        put(&mut seen, root, true);
        let mut stack: Vec<(usize, usize)> = vec![(root, 0)];
        while let Some((x, k)) = stack.pop() {
            match succ.get(x).and_then(|s| s.get(k)).copied() {
                Some(y) => {
                    stack.push((x, k + 1));
                    if !ent(&seen, y, true) {
                        put(&mut seen, y, true);
                        stack.push((y, 0));
                    }
                }
                None => {
                    put(&mut number, x, post.len());
                    post.push(x);
                }
            }
        }

        let mut idom = vec![NONE; nb + 1];
        put(&mut idom, root, root);
        let intersect = |idom: &[usize], mut a: usize, mut b: usize| {
            while a != b {
                while ent(&number, a, NONE) < ent(&number, b, NONE) {
                    a = ent(idom, a, root);
                }
                while ent(&number, b, NONE) < ent(&number, a, NONE) {
                    b = ent(idom, b, root);
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
                let ps = preds.get(b).map(Vec::as_slice).unwrap_or_default();
                let mut next = NONE;
                for p in ps.iter().copied().chain(from_root) {
                    if ent(&idom, p, NONE) == NONE {
                        continue;
                    }
                    next = if next == NONE { p } else { intersect(&idom, p, next) };
                }
                if ent(&idom, b, NONE) != next {
                    put(&mut idom, b, next);
                    changed = true;
                }
            }
        }

        // The tree, numbered in preorder.
        let mut children: Vec<Vec<usize>> = vec![Vec::new(); nb + 1];
        for b in post.iter().rev().skip(1).copied() {
            if let Some(c) = children.get_mut(ent(&idom, b, root)) {
                c.push(b);
            }
        }
        let mut range: Vec<Option<(u32, u32)>> = vec![None; nb + 1];
        let mut clock = 0u32;
        let mut stack: Vec<(usize, usize)> = vec![(root, 0)];
        while let Some((x, k)) = stack.pop() {
            if k == 0 {
                put(&mut range, x, Some((clock, clock)));
                clock += 1;
            }
            match children.get(x).and_then(|c| c.get(k)).copied() {
                Some(y) => {
                    stack.push((x, k + 1));
                    stack.push((y, 0));
                }
                None => {
                    if let Some(Some((_, end))) = range.get_mut(x) {
                        *end = clock;
                    }
                }
            }
        }
        range.truncate(nb);
        Dominance { range }
    }

    /// Whether [`Jit::promote`]'s walk could accept the back edge from `p` to
    /// `h`. A `false` is certain; a `true` is for the walk to decide.
    ///
    /// The walk visits every predecessor of every block it reaches except the
    /// header, and gives up on reaching the entry or a block nothing jumps to.
    /// So it gives up exactly when one of those reaches `p` without passing
    /// `h`, which is when `h` does not dominate `p` from the virtual root — and
    /// always when the header is the entry, which is inside every loop body.
    fn may_loop(&self, h: usize, p: usize) -> bool {
        if h == 0 {
            return false;
        }
        if p == h {
            return true;
        }
        match (self.range.get(h).copied().flatten(), self.range.get(p).copied().flatten()) {
            // No way in reaches `p`, so the walk cannot reach one either.
            (_, None) => true,
            (None, Some(_)) => false,
            (Some((lo, hi)), Some((at, _))) => lo <= at && at < hi,
        }
    }
}

fn uses_after(code: &ir::Code, block: &ir::Block, v: ir::ValueId, from: usize) -> usize {
    let mut n = 0;
    let mut ops = Vec::new();
    for i in block.insts.iter().skip(from + 1) {
        ops.clear();
        i.operands(&mut ops);
        n += ops.iter().filter(|o| **o == v).count();
    }
    ops.clear();
    block.term.operands(&mut ops);
    for t in block.term.targets() {
        ops.extend_from_slice(&t.args);
    }
    n += ops.iter().filter(|o| **o == v).count();
    // A value used in another block is not a candidate for fusion at all.
    for b in &code.blocks {
        if std::ptr::eq(b, block) {
            continue;
        }
        for i in &b.insts {
            ops.clear();
            i.operands(&mut ops);
            n += ops.iter().filter(|o| **o == v).count();
        }
        ops.clear();
        b.term.operands(&mut ops);
        for t in b.term.targets() {
            ops.extend_from_slice(&t.args);
        }
        n += ops.iter().filter(|o| **o == v).count();
    }
    n
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
        ir::Inst::CallIntrinsic { key, .. } => key != "testing_assert.report",
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
        | ir::Inst::CallIndirect { .. }
        | ir::Inst::Structural { .. }
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
            let x = *bits as u64;
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
pub fn symbol_of(prog: &ir::Program, f: u32) -> String {
    match prog.funcs.get(f as usize) {
        Some(func) => func.symbol.clone(),
        None => format!("buri$missing${f}"),
    }
}

