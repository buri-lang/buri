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
              where a tail branch was found, which is four bytes that exist"
)]

use super::abi::Loc;
use super::region::{Region, RelocKind, Target};
use super::library::{Hole, HoleKind, Library};
use crate::compiler::middle::ir;
use crate::compiler::middle::layout::{Layout, Layouts};
use crate::compiler::semantics::types::{Tables, Ty};
use std::collections::HashMap;

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

#[derive(Default)]
pub struct Stats {
    pub funcs: usize,
    pub funcs_clean: usize,
    pub blocks: usize,
    pub insts: usize,
    pub stencils: usize,
    pub bytes: usize,
    pub elided: usize,
    pub unsupported: usize,
    pub rc_skipped: usize,
    pub reasons: HashMap<String, usize>,
    pub regs_assigned: usize,
    pub fused: usize,
    pub folded: usize,
    pub imm_relaxed: usize,
    pub max_frame: u32,
    pub coalesced: usize,
    pub cross_regs: usize,
    /// How many `list.*` closure calls were open-coded as a loop rather than
    /// left to `intrin.rs`'s descriptor helper, and how many of those got a
    /// **direct** call to the step (`lists.rs`).
    pub list_loops: usize,
    pub list_direct: usize,
    /// (2) The profile the supernode question needs: how often each stencil is
    /// copied, and how often each *adjacent pair* of them is, which is exactly
    /// the shape a two-operation supernode would fuse.
    pub keys: HashMap<String, usize>,
    pub pairs: HashMap<(String, String), usize>,
    pub last_key: String,
}

pub struct Jit<'a> {
    pub(crate) lib: &'a Library,
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
    pub stats: Stats,
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
    /// it is only a lookup. `cranelift/helpers.rs` is the same set of helpers
    /// under the same argument.
    helpers: Vec<super::glue::Helper>,
    helper_ix: HashMap<super::glue::Helper, usize>,
    helper_at: Vec<u64>,
    /// Whether a value of a type owns a counted block anywhere inside it, by
    /// type. Memoised because the question is asked once per reference
    /// operation and answering it walks the type; see `Lower::rc_counted`.
    pub(crate) counted_memo: HashMap<Ty, bool>,
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
    pub(crate) fn new(lib: &'a Library, tables: &'a Tables, frames: &'a [FrameSig]) -> Jit<'a> {
        Jit {
            lib,
            region: Region::new(),
            layouts: Layouts::new(tables),
            tables,
            frames,
            entries: Vec::new(),
            fixups: Vec::new(),
            stats: Stats::default(),
            reasons: Vec::new(),
            veneer_ok: false,
            dirty: Vec::new(),
            current: 0,
            helpers: Vec::new(),
            helper_ix: HashMap::new(),
            helper_at: Vec::new(),
            counted_memo: HashMap::new(),
        }
    }

    /// The symbol of a generated helper, registering it the first time it is
    /// asked for.
    ///
    /// A **local** symbol of this unit, so two units that both need the drop
    /// glue for `[Str]` get a copy each and neither collides —
    /// `cranelift/helpers.rs`'s `Linkage::Local` for the same reason.
    pub(crate) fn helper(&mut self, h: super::glue::Helper) -> String {
        if let Some(i) = self.helper_ix.get(&h) {
            return super::glue::symbol(*i);
        }
        let i = self.helpers.len();
        self.helper_ix.insert(h.clone(), i);
        self.helpers.push(h);
        self.helper_at.push(0);
        super::glue::symbol(i)
    }

    /// Every helper this unit generated, as `(symbol, section offset)`.
    pub fn helper_symbols(&self) -> Vec<(String, u64)> {
        self.helper_at
            .iter()
            .enumerate()
            .map(|(i, at)| (super::glue::symbol(i), *at))
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
    pub(crate) fn elidable_arm(&self, key: &str) -> Option<String> {
        let s = self.lib.get(key)?;
        let n = s.holes.get(s.tail?)?.name.clone();
        if let Some(f) = self.lib.get(&format!("{key}+fold")) {
            if f.holes.get(f.tail?).map(|h| h.name.clone()) != Some(n.clone()) {
                return None;
            }
        }
        Some(n)
    }

    pub(crate) fn push_reason(&mut self, why: String) -> u64 {
        self.stats.unsupported += 1;
        if let Some(d) = self.dirty.get_mut(self.current) {
            *d = true;
        }
        *self.stats.reasons.entry(why.clone()).or_default() += 1;
        if let Some(i) = self.reasons.iter().position(|r| *r == why) {
            return i as u64;
        }
        self.reasons.push(why);
        (self.reasons.len() - 1) as u64
    }

    /// `f`'s frame layout. `Jit::plan` fills `frames` with one entry per
    /// function of the program, so the empty signature is what a `FuncIdx` from
    /// some other program would get and not one from this one.
    pub(crate) fn frame_sig_of(&self, f: usize) -> FrameSig {
        self.frames.get(f).cloned().unwrap_or_default()
    }

    /// The layout of a source type directly, for the places the IR's `TypeId`
    /// is not the type wanted (a `[T]`'s element, a closure's return).
    pub(crate) fn layouts_of(&mut self, ty: Ty) -> Layout {
        self.layouts.of(ty)
    }

    /// Whether `middle::layout` put `field` behind a pointer inside `owner`,
    /// which it does for the field that would otherwise make the owner
    /// recursive.
    pub(crate) fn boxes(&self, owner: &Ty, field: &Ty) -> bool {
        self.layouts.boxes(owner, field)
    }

    pub(crate) fn layout_of(&mut self, prog: &ir::Program, id: ir::TypeId) -> Layout {
        let ty: Ty = prog.type_info(id).ty.clone();
        self.layouts.of(ty)
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

    /// The per-unit tables, sized from the program.
    ///
    /// The frame layouts a call site needs are *not* computed here: they are a
    /// whole-program function of the program alone, so `emit_units` computes
    /// them once and every unit borrows the same slice. What is left is the two
    /// vectors that are genuinely this unit's — where each function was laid
    /// out, and whether it has been emitted — and both are a `memset`.
    pub fn plan(&mut self, prog: &ir::Program) {
        self.entries = vec![0; prog.funcs.len()];
        self.dirty = vec![false; prog.funcs.len()];
        for fs in self.frames {
            self.stats.max_frame = self.stats.max_frame.max(fs.size);
        }
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
            ir::Type::Agg(id) => l.of(prog.type_info(id).ty.clone()).size,
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
                if entry_params.contains(&(v as u32)) {
                    continue;
                }
                at += round8(width(&mut layouts, code.ty_of(ir::ValueId(v as u32)))).max(8);
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
/// Sixteen for the emitter's own temporaries and the open-coded list loops'
/// (`lists.rs` §"the scratch words"), sixteen more for the C argument area a
/// runtime call marshals into (`rtcall::CARG_WORD`), and twenty-four for the
/// loops whose state does not fit two words — the merge sort's seven indices,
/// `flatten`'s two passes (`lists.rs::LOOP_SCRATCH`). The C argument area is
/// not optional: a `crt` stencil's arguments have to live **inside** this
/// frame, and the first byte past it is where a Buri callee's frame starts —
/// which `lists.rs` writes into before it calls the step.
pub(crate) const SCRATCH_WORDS: usize = 96;

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
    /// The literal a value holds, when it is an `Inst::Const` a stencil can
    /// take as an immediate.
    pub konst: Vec<Option<u64>>,
    /// Values whose every use is an immediate operand: the `Const` that
    /// defines them is never materialised into a frame slot at all.
    pub folded: Vec<bool>,
    /// For a value defined by an `Inst::MakeClosure` in *this* function, the
    /// `FuncIdx` of the lifted lambda. `cranelift/emit.rs` keeps the same map
    /// (`Lower::closures`) and for the same reason: it is what lets a step be
    /// called by name rather than through its code pointer.
    pub closure_of: Vec<Option<u32>>,
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
    /// Where a value is: a CPS register if one was assigned, the frame
    /// otherwise. This is the paper's "whether it operates on constants,
    /// registers, or stack locations".
    pub fn loc(&self, v: ir::ValueId) -> Loc {
        self.reg.get(v.index()).copied().flatten().unwrap_or(Loc::Frame)
    }
}

impl<'a> Jit<'a> {
    /// One codegen unit: the members' bodies, back to back, and the
    /// function-local branches resolved.
    ///
    /// `members` is the unit's functions in ascending index, which is the order
    /// `ir::Program::funcs_by_unit` yields and therefore the order the object's
    /// symbols come out in. Deterministic emission is a cache requirement, not
    /// a nicety: `--check-reproducible` compares two builds byte for byte.
    pub fn compile_unit(&mut self, prog: &ir::Program, members: &[usize]) {
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
            put(&mut self.helper_at, i, at);
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
        self.stats.funcs += 1;
        let before_unsupported = self.stats.unsupported;

        match &f.body {
            ir::Body::Code(code) => {
                let (slot, scratch) = self.slots(prog, code, &frame);
                let mut reg = vec![None; code.values()];
                let mut wt = vec![true; code.values()];
                let taken = self.promote(code, &mut reg, &mut wt);
                self.regalloc(code, &mut reg, taken);
                let (konst, folded) = self.constants(code);
                let mut closure_of: Vec<Option<u32>> = vec![None; code.values()];
                for b in &code.blocks {
                    for i in &b.insts {
                        if let ir::Inst::MakeClosure { dest, func, .. } = i {
                            put(&mut closure_of, dest.index(), Some(func.0));
                        }
                    }
                }
                let mut st = Fn2 {
                    slot,
                    blk: vec![0; code.blocks.len()],
                    frame,
                    scratch,
                    reg,
                    wt,
                    konst,
                    folded,
                    closure_of,
                };
                let base = self.fixups.len();
                let order = self.layout(code);
                for (oi, bi) in order.iter().copied().enumerate() {
                    self.stats.last_key.clear();
                    let here = self.region.code_addr();
                    put(&mut st.blk, bi, here);
                    self.stats.blocks += 1;
                    let block = code.get(ir::BlockId(bi as u32));
                    let plan = self.plan_block(code, &st, block);
                    // `Plan::skip` is built with one flag per instruction of
                    // this block, so the zip drops nothing.
                    for (inst, skip) in block.insts.iter().zip(plan.skip.iter()) {
                        self.stats.insts += 1;
                        if *skip {
                            self.stats.fused += 1;
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
            // A runtime-supplied body has no IR to walk, but it may still emit
            // a **loop** (`lists.rs`), which needs labels and the same
            // function-local branch resolution a real body gets. So it is given
            // an empty `Fn2` whose frame is its own signature's and whose
            // scratch is the area `Jit::plan` reserved past the parameters.
            ir::Body::Runtime(key) => {
                let mut st = Fn2 {
                    slot: Vec::new(),
                    blk: Vec::new(),
                    frame: frame.clone(),
                    scratch: frame.param_end,
                    reg: Vec::new(),
                    wt: Vec::new(),
                    konst: Vec::new(),
                    folded: Vec::new(),
                    closure_of: Vec::new(),
                };
                let base = self.fixups.len();
                self.runtime_body(prog, fi, key.clone(), &mut st);
                self.resolve_blocks(base, &st.blk);
            }
        }
        if self.stats.unsupported == before_unsupported {
            self.stats.funcs_clean += 1;
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
        let mut s = match lib.get(key) {
            Some(s) => s,
            None => crate::diagnostics::ice(&format!(
                "stencil: no stencil {key} in {}",
                lib.config
            )),
        };
        // The folded twins, most specific first: every offset or literal a twin
        // would put in an `imm12` field has to be a multiple of the field's
        // scale and inside its reach, and the two folds are independent, so a
        // stencil can have both, either or neither.
        for suffix in ["+ifold+fold", "+fold", "+ifold"] {
            let Some(f) = lib.get(&format!("{key}{suffix}")) else { continue };
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
                self.stats.folded += 1;
                break;
            }
        }
        // The hole on the stencil's last instruction, once the fold above has
        // settled which twin is being copied.
        let tail_name: Option<&str> =
            s.tail.and_then(|t| s.holes.get(t)).map(|h| h.name.as_str());
        let mut len = s.code.len();
        // Fallthrough elision: the continuation is the next stencil, so the
        // trailing `b` is not patched, it is dropped.
        let mut elide = false;
        if let Some(name) = tail_name {
            if binds.iter().any(|(n, v)| *n == name && matches!(v, V::Fall)) {
                elide = true;
                len -= 4;
                self.stats.elided += 1;
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
        self.stats.stencils += 1;
        self.stats.bytes += len;
        for h in &s.holes {
            if elide && tail_name == Some(h.name.as_str()) {
                continue;
            }
            let bound = binds.iter().find(|(n, _)| *n == h.name.as_str()).map(|(_, v)| v.clone());
            let Some(v) = bound else {
                // A hole the caller did not name is a symbol the stencil body
                // reached on its own — an abort, `buri_rt_decref`, `memcpy` —
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

    /// A hole naming a symbol outside this program: one relocation per site,
    /// and no instruction rewritten. A `bl` becomes a `BRANCH26`; the address
    /// of one, materialised into the constant pool, becomes an `Abs64`.
    fn import(&mut self, at: u64, h: &Hole) {
        let name = h.name.clone();
        for off in &h.branches {
            self.region.reloc(
                at + *off as u64,
                RelocKind::Branch26,
                Target::Symbol(name.clone()),
            );
        }
        if !h.pairs.is_empty() {
            let slot = self.region.pool_target(Target::Symbol(name));
            for (a, b) in &h.pairs {
                self.region.pool_ref(at + *a as u64, at + *b as u64, slot);
            }
        }
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
                            self.region.reloc(
                                at + *off as u64,
                                RelocKind::Branch26,
                                Target::Symbol(String::from(n)),
                            );
                        }
                        None
                    }
                    V::Sym(ref n) => {
                        for off in &h.branches {
                            self.region.reloc(
                                at + *off as u64,
                                RelocKind::Branch26,
                                Target::Symbol(n.clone()),
                            );
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
                for (a, b) in &h.pairs {
                    self.patch_imm32(at + *a as u64, at + *b as u64, x as u32);
                }
                for (o, scale) in &h.lo12 {
                    self.patch_lo12(at + *o as u64, x as u32, *scale);
                }
            }
            HoleKind::Imm64 => {
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
                        self.stats.imm_relaxed += 1;
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

    /// `b`/`bl`: a signed 26-bit word displacement. Everything generated lives
    /// in one region, so this always reaches.
    fn patch_branch(&mut self, at: u64, target: u64) {
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
    /// `cranelift-object` emits for the same edge; the linker resolves an
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
            self.region.reloc(at, RelocKind::Branch26, Target::Symbol(name));
        }
    }

    /// Whether a function, or anything reachable from it, contains an
    /// `unsupported` stencil — the honest predicate for "this test can be run".
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

    /// Where `f` was emitted inside this unit's section. `plan` gives every
    /// function of the program an entry — zero for one this unit does not own,
    /// which is also what a `FuncIdx` from outside the program would read.
    pub fn entry_of(&self, f: usize) -> u64 {
        ent(&self.entries, f, 0)
    }
    /// Every function's entry, for `cache::Image::capture`.
    pub fn dirty_raw(&self) -> &[bool] {
        &self.dirty
    }

    pub fn entries_raw(&self) -> &[u64] {
        &self.entries
    }
    /// `f`'s frame size, from the same table [`Jit::frame_sig_of`] reads.
    pub fn frame_of(&self, f: usize) -> u32 {
        self.frames.get(f).map_or(0, |fs| fs.size)
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
        (slot, at)
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
        let mut merged = 0usize;
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
                if ent(width, ai, 0) != 8 || ent(width, pi, 0) != 8 {
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
                merged += 1;
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
                merged += 1;
            }
        }
        self.stats.coalesced += merged;
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
            let succ: Vec<usize> = match code.blocks.get(b) {
                Some(block) => block.term.targets().iter().map(|t| t.block.index()).collect(),
                None => Vec::new(),
            };
            match succ.get(k).copied() {
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
    /// Answers how many integer and floating registers the promotion took, so
    /// that the paper's own expression-temporary allocator can have the rest.
    fn promote(
        &mut self,
        code: &ir::Code,
        out: &mut [Option<Loc>],
        wt: &mut [bool],
    ) -> (usize, usize) {
        let nb = code.blocks.len();
        let nregs = super::abi::NREGS;
        if nregs < 2 || nb == 0 {
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
        let mut best: Option<Vec<bool>> = None;
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
                // `body`, `preds` and `barrier` all have one entry per block
                // of `code` and every index into them is a block of `code`,
                // so "outside the loop" is what a missing entry reads as —
                // which abandons the candidate rather than widening it.
                let mut body = vec![false; nb];
                put(&mut body, h, true);
                put(&mut body, p, true);
                let mut stack = vec![p];
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
                        if !ent(&body, *q, true) {
                            put(&mut body, *q, true);
                            stack.push(*q);
                        }
                    }
                }
                if !ok || ent(&body, 0, true) {
                    continue;
                }
                // Every way into the loop is through the header.
                let closed = body.iter().enumerate().all(|(x, inside)| {
                    !*inside
                        || x == h
                        || preds.get(x).is_some_and(|ps| ps.iter().all(|q| ent(&body, *q, false)))
                });
                if !closed {
                    continue;
                }
                if body.iter().zip(barrier.iter()).any(|(x, br)| *x && *br) {
                    continue;
                }
                let size = body.iter().filter(|x| **x).count();
                if best.as_ref().map(|bb| size < bb.iter().filter(|x| **x).count()).unwrap_or(true) {
                    best = Some(body);
                    best_h = h;
                }
            }
        }
        let Some(body) = best else { return (0, 0) };
        let h = best_h;

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
                let ok = inside && matches!(i, ir::Inst::Binary { .. } | ir::Inst::Unary { .. });
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
            if *k + 1 >= nregs {
                continue;
            }
            put(out, p.index(), Some(Loc::Reg(*k as u8)));
            // A value no `reg_ok` entry covers is one nothing in this function
            // reads, so writing its slot through is the conservative answer.
            put(wt, p.index(), !ent(&reg_ok, p.index(), false));
            *k += 1;
            self.stats.cross_regs += 1;
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
                    put(wt, a.index(), false);
                    self.stats.cross_regs += 1;
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
        for block in &code.blocks {
            let n = block.insts.len();
            // Where each value defined in this block is used, and where the
            // barriers are. A barrier is an instruction whose stencil has the
            // zero-register prototype and therefore clobbers the file.
            let mut def_at: HashMap<u32, usize> = HashMap::new();
            let mut use_at: HashMap<u32, Vec<usize>> = HashMap::new();
            let mut barrier = vec![false; n + 1];
            let mut ops = Vec::new();
            for (k, i) in block.insts.iter().enumerate() {
                ops.clear();
                i.operands(&mut ops);
                for o in &ops {
                    use_at.entry(o.0).or_default().push(k);
                }
                for d in i.results() {
                    def_at.insert(d.0, k);
                }
                put(&mut barrier, k, is_barrier(i));
            }
            ops.clear();
            block.term.operands(&mut ops);
            for t in block.term.targets() {
                ops.extend_from_slice(&t.args);
            }
            for o in &ops {
                use_at.entry(o.0).or_default().push(n);
            }

            // The registers cross-block promotion took are not the local
            // allocator's to hand out.
            let nr = super::abi::NREGS;
            let mut busy: Vec<Option<u32>> =
                (0..nr).map(|k| (k < taken.0).then_some(u32::MAX)).collect();
            let mut busyf: Vec<Option<u32>> =
                (0..nr).map(|k| (k < taken.1).then_some(u32::MAX)).collect();
            let (base, basef) = (taken.0, taken.1);
            for (k, i) in block.insts.iter().enumerate() {
                // Free every register whose value was last used here.
                for (slot, b) in busy.iter_mut().chain(busyf.iter_mut()).enumerate() {
                    let _ = slot;
                    if let Some(v) = *b {
                        if v == u32::MAX {
                            continue; // a promoted register, not this pass's
                        }
                        // Every entry of `use_at` was created by pushing to
                        // it, so none of them is empty and "no last use" here
                        // means the value is not used in this block at all.
                        let live = use_at
                            .get(&v)
                            .is_some_and(|u| u.last().is_some_and(|last| *last > k));
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
                    ir::Inst::Unary { dest, prim, .. } if !wide(prim) => (
                        matches!(prim, crate::compiler::semantics::types::Prim::F32 | crate::compiler::semantics::types::Prim::F64),
                        *dest,
                    ),
                    _ => continue,
                };
                if ent(out, dest.index(), None).is_some() {
                    continue; // already promoted across the loop
                }
                let Some(u) = use_at.get(&dest.0) else { continue };
                if u.len() != 1 || ent(&total, dest.index(), 0) != 1 {
                    continue;
                }
                let Some(at) = u.first().copied() else { continue };
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
                    self.stats.regs_assigned += 1;
                }
            }
        }
    }

    /// Which `Inst::Const`s never need a frame slot, because every use of them
    /// is an immediate operand of a stencil that has an immediate variant.
    fn constants(&mut self, code: &ir::Code) -> (Vec<Option<u64>>, Vec<bool>) {
        let mut konst: Vec<Option<u64>> = vec![None; code.values()];
        let mut folded = vec![false; code.values()];
        for block in &code.blocks {
            for i in &block.insts {
                if let ir::Inst::Const { dest, value } = i {
                    put(&mut konst, dest.index(), literal(value, code.ty_of(*dest)));
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
                    if ent(&konst, rhs.index(), None).is_some() {
                        if let Some((tag, _, _)) = super::emit::prim_tag(*prim) {
                            let k = format!(
                                "bin/{}/{tag}/fi/f",
                                super::emit::binop_name(*op)
                            );
                            if self.has(&k) {
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
            *f = ent(&konst, v, None).is_some() && seen > 0 && seen == ent(&imm, v, 0);
        }
        (konst, folded)
    }

    /// Fusions the terminator can absorb.
    fn plan_block(&mut self, code: &ir::Code, st: &Fn2, block: &ir::Block) -> Plan {
        let n = block.insts.len();
        let mut p = Plan { skip: vec![false; n], cmpbr: None, tagsw: None };
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
/// C library — the same boundary `cranelift/runtime.rs` draws — so this list is
/// checked against `cli/runtime/lib.rs`'s exports by a test rather than left to
/// a link error to discover.
pub const EXTERNALS: [&str; 7] = [
    "buri_rt_abort",
    "buri_rt_abort_div_zero",
    "buri_rt_abort_unreachable",
    "buri_rt_alloc",
    "buri_rt_decref",
    "buri_rt_i128_divmod",
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

