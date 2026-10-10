//! The functions one unit generates for itself.
//!
//! Every register-machine backend here generates the same set under the same
//! argument, and the reason each exists is:
//!
//! | Helper | Why it is generated rather than called |
//! |---|---|
//! | [`Helper::Thunk`] | A closure's `code` takes its environment as a **pointer**; a lifted lambda takes it as an aggregate parameter laid out flat in its frame. Something has to convert, and it is also the one place the indirect-call ownership convention meets the callee's own. |
//! | [`Helper::Walk`] | The per-type reference-count walk, as a C function `fn(*mut u8)`: the drop glue [`buri_rt_decref`](cli/runtime/memory.rs) calls, the per-element retain `cli/runtime/list.rs` is handed, and the **copy** `core/alloc::copyOut` is compiled into, so that a value leaving a scope shares no block with the one it left behind. |
//! | [`Helper::Elems`] | The same for a whole `[T]` block, whose element count is `cap / stride`. |
//! | [`Helper::Env`] | The one indirection that lets a closure environment carry its own glue: `Ty::Fn` does not record what was captured, so the block holds the release function in its first word and the copy in its second — see [`ENV_FIELDS`]. |
//! | [`Helper::Entry`] | The other direction through the C boundary: a `void(state, index, in, out)` the **runtime** calls to run one Buri step. A closure's `code` has a parameter list that depends on the element type, so the runtime cannot call it; this is generated where that type is known and is the only thing that does. |
//! | [`Helper::Equal`] | The same direction and the same reason, one shape smaller: a `void(frame, a, b, out)` the reactive graph compares a write through. `==` is structural, so a cell holding a `Str` cannot answer "is this the value already there" from its bytes — and the comparison's parameter list depends on the type, so the runtime cannot make the call either. |
//!
//! The drop and copy glue ([`Helper::Walk`] through [`Helper::Env`]) is
//! named by what it does and by the type's glue key
//! (`layout::Layouts::glue_key`), and defined weak ([`shared_symbol`]): every
//! unit that drops a `[Str]` emits the same symbol and the linker keeps one.
//! The rest name a function of this program, so each is a **local** symbol of
//! the part that needed it.
//!
//! # Two calling conventions, and the bridge between them
//!
//! A thunk is entered by the `calli` stencil, so it is an ordinary
//! frame-threaded body and needs no bridge: `x0` is a frame pointer on the way
//! in and on the way out.
//!
//! A glue function is entered from outside the frame-threaded world — the
//! `decref` stencil's dying arm, `buri_rt_decref(p, glue)`, and
//! `cli/runtime/list.rs`'s `retain` — so it is `extern "C" fn(*mut u8)` and
//! every one of them is a hand-written eight-instruction stub in front of a
//! frame-threaded body. The stub's whole job is to make a frame: it takes the
//! **machine** stack for it rather than the Buri stack, because drop glue
//! recurses (a `[[Str]]` releases a `[Str]` releases a `Str`) and a fixed
//! scratch frame would be re-entered by its own callee.
//!
//! An **entry thunk** is entered from outside as well, and by something with no
//! frame at all to lend it: `cli/runtime/list.rs` is C, and the Buri stack is
//! not a thing C has a pointer into. So the frame it works in is one the *call
//! site* set aside — the first byte past its own frame, which is where a Buri
//! callee's frame begins anyway — and the address of it travels in the third
//! word of the state record. That is one word of ABI rather than a stack
//! discipline. A step running on a task can't use it, because its siblings
//! run beside it and it may park, so `Helper::Entry`'s stub first asks the
//! runtime for a stack of the task's own and falls back to the word only
//! outside a task.
//!
//! The walk itself reads the value out of a *copy* in that frame rather than
//! through the pointer. That is what lets `Jit::walk_rc` — which addresses
//! everything as a frame offset — serve both an `Inst::DecRef` and a glue
//! function with no second implementation of the walk.
//!
//! Only a value one fixed-width load copies is copied whole: 64 bytes or
//! fewer, at a width the library has an `eload` for. Anything else would be a
//! `memcpy` call before the first count, so it is walked **in place**: each
//! counted field is loaded on its own, walked, and for a copy written back,
//! and a field no one load covers goes to its own type's glue through a
//! pointer. A list's element glue does the same per element. So a glue frame
//! holds one field rather than the value, and no value is too wide for one
//! (`Jit::walk_in_place`, `Jit::elems_glue`, buri-lang/buri#255).

#![allow(
    clippy::arithmetic_side_effects,
    reason = "the sums here are byte offsets inside a frame this file lays out \
              immediately above, from slot widths `middle::layout` computed for \
              types already in memory, plus the scratch words `SCRATCH_BYTES` \
              names. `frame_bytes` is the one that could overflow a machine \
              instruction's field and it is checked against `MAX_GLUE_FRAME` \
              before anything is emitted"
)]

use super::asm::{Asm, RAX, RCX, RDI, RDX, RSI, RSP, SP, X86};
use super::emit::RC_DEPTH;
use super::jit::{Fn2, FrameSig, Jit, V};
use super::rtcall::Src;
use crate::compiler::backend::counts::{Field, Op, Site};
use crate::compiler::backend::task_thread;
use crate::compiler::middle::ir;
use crate::compiler::middle::layout::{Layouts, CAP_MASK, CLOSURE_ENV};
use crate::compiler::semantics::types::Ty;

/// One generated function.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Helper {
    /// `code(fp)` for a closure over `func`.
    ///
    /// `args` is how many parameters the closure's *type* declares, which is
    /// what separates a leading environment parameter from a value one: a
    /// capture-free lambda still has the first, of the unit type, and a plain
    /// `FnRef` has none at all. `boxed` says whether the `env` word holds a
    /// block to read the record out of.
    Thunk { func: u32, args: u32, boxed: bool },
    /// `op` over one value of a type, as `fn(*mut u8)`. A copy replaces every
    /// counted pointer in the value, in place, by a pointer to a fresh block.
    Walk { ty: Ty, op: Op },
    /// The same over every element of a `[T]` block.
    Elems { ty: Ty, op: Op },
    /// Read a release (or, for [`Op::Copy`], a copy) function out of a
    /// block's first (second) word and call it on the rest.
    Env { op: Op },
    /// The C-ABI **entry thunk** a runtime-driven step is reached through:
    /// `extern "C" fn(state, index, arg, out)`, which runs the closure in
    /// `state` once on the element at `arg` and writes its answer through `out`
    /// (`cli/runtime/list.rs`'s `StepEntry`).
    ///
    /// `params` and `ret` are the closure's own signature, which is what makes
    /// one of these per step shape rather than per key: everything the runtime
    /// cannot say about the element types is said here, at the call site, where
    /// they are known.
    ///
    /// `index` is which of `params` receives the runtime's loop counter, and it
    /// is part of the *key* rather than derived from the signature: two steps
    /// can take the same types and mean different things by the second one.
    /// `None` is a step that is not told where it is; the register still
    /// arrives and the body ignores it.
    Entry { params: Vec<Ty>, ret: Ty, index: Option<usize> },
    /// The C-ABI **equality thunk** the reactive graph compares a write
    /// through: `extern "C" fn(frame, a, b, out)`, which runs
    /// `middle::derives`'s generated `Equal` for `ty` on the two values it is
    /// handed and writes the answer through `out` as a byte.
    ///
    /// [`Helper::Entry`]'s smaller sibling, and `frame` is the word that makes
    /// them siblings: the comparison is Buri code, so it needs a Buri frame,
    /// and the runtime acquires one and passes its address exactly as an entry
    /// thunk reads one out of the state record. There is no record here to put
    /// it in, so it is the first C argument — and the LLVM backend, which works
    /// on the machine stack, ignores it.
    Equal { ty: Ty, func: u32 },
}

/// The program-wide symbol of a drop or copy glue function, or `None` for a
/// helper that stays local to its part.
///
/// The glue key covers everything the glue's body reads, so two helpers with
/// one name have one body, whichever unit or program emitted it. That is what
/// makes it safe to define them weak: on Mach-O a weak private external, which
/// `ld64` coalesces, and on ELF a COMDAT group per function (`mod.rs`).
pub fn shared_symbol(h: &Helper, layouts: &mut Layouts<'_>) -> Option<String> {
    let (what, key) = match h {
        Helper::Walk { ty, op: Op::Release } => ("release", layouts.glue_key(ty)),
        Helper::Walk { ty, op: Op::Retain } => ("retain", layouts.glue_key(ty)),
        Helper::Walk { ty, op: Op::Copy } => ("copy", layouts.glue_key(ty)),
        Helper::Elems { ty, op: Op::Release } => ("elems", layouts.glue_key(ty)),
        Helper::Elems { ty, op: Op::Retain } => ("retainelems", layouts.glue_key(ty)),
        Helper::Elems { ty, op: Op::Copy } => ("copyelems", layouts.glue_key(ty)),
        Helper::Env { op: Op::Copy } => return Some(String::from("buri$stencil$glue$envcopy")),
        Helper::Env { .. } => return Some(String::from("buri$stencil$glue$env")),
        Helper::Thunk { .. } | Helper::Entry { .. } | Helper::Equal { .. } => return None,
    };
    Some(format!("buri$stencil$glue${what}${key}"))
}

/// The symbol a part-local helper is emitted under.
///
/// `$` cannot appear in a Buri path, so no `ir::Func::symbol` can collide with
/// one — the same guarantee `mod.rs`'s pool anchor rests on. The index is the
/// order the *part* first asked for it, which is emission order and therefore
/// reproducible.
///
/// **Two numbers and not one**, because a unit is emitted in parts and a part
/// does not know what the parts beside it asked for. `part` is the part's index
/// within its unit — a function of the member count alone
/// (`mod.rs::PART_MEMBERS`) — so the pair is unique inside the object however
/// the work divided, and it is the same pair on every machine. Two parts
/// needing the same thunk get a copy each; they are local symbols and
/// `-dead_strip` keeps whichever is reached.
pub fn symbol(part: usize, i: usize) -> String {
    format!("buri$stencil$p{part}h{i}")
}

/// The environment record starts **two** words into its block: the first word
/// is the block's own release function and the second is its copy function.
/// Every backend writes the same sixteen bytes, and they must agree because
/// they write the same shape.
///
/// # Why two words and not one
///
/// `Ty::Fn` does not record what a closure captured, so neither of the two
/// operations a generic path performs on an environment — release it, copy
/// it — can be derived from the type at the site that performs it. The release
/// half has carried its answer in the block since closures were first counted;
/// G5 needs the other half for the same reason and gets it the same way.
///
/// The alternative considered was one word pointing at a static pair, which
/// costs the same eight bytes per *type* rather than per closure but puts a
/// second load in front of every drop of every closure in the language. Eight
/// bytes on a block that already carries sixteen of header is the cheaper of
/// the two, and it keeps [`Helper::Env`] the five instructions it was.
pub const ENV_FIELDS: u32 = 16;

/// Where a block's copy function sits inside its environment header.
pub const ENV_COPY_WORD: u32 = 8;

/// Where the closure's environment pointer sits inside `{ code, env }`.
pub const ENV_WORD: u32 = (CLOSURE_ENV as u32) * 8;

/// Scratch past the last named slot of a generated frame, in bytes.
///
/// The same words `jit::SCRATCH_WORDS` gives every emitted body, and the same
/// constant, so that a sequence which fits one frame fits the other.
const SCRATCH_BYTES: u32 = super::jit::SCRATCH_WORDS as u32 * 8;

/// The widest frame a glue stub can make.
///
/// The stub forms it with `sub sp, sp, #imm`, whose immediate is twelve bits
/// unshifted. No glue asks for more: a glue frame holds at most 64 bytes of
/// value, because anything wider is walked in place (`Jit::walk_in_place`).
/// The stub still refuses a wider frame rather than emit an immediate that
/// does not fit.
///
/// x86-64's `sub rsp, imm32` has no such limit and is held to the same number
/// anyway, so both targets walk the same values the same way. Keeping every
/// glue frame under a page is also what lets the stub go without a stack
/// probe: one `sub` this size cannot step over a guard page.
const MAX_GLUE_FRAME: u32 = 4080;

fn round8(n: u32) -> u32 {
    (n + 7) & !7
}

fn round16(n: u32) -> u32 {
    (n + 15) & !15
}

/// The frame a glue body needs to hold `bytes` of value.
fn frame_for(bytes: u32) -> u32 {
    round16(G_VALUE + round8(bytes) + SCRATCH_BYTES)
}

/// The fixed slots a glue frame opens with: the pointer it was handed, a loop
/// index, an element count, and one spare word.
const G_PTR: u32 = 0;
const G_INDEX: u32 = 8;
const G_COUNT: u32 = 16;
const G_SPARE: u32 = 24;
const G_VALUE: u32 = 32;

/// The fixed slots an **entry thunk**'s frame opens with: its four C
/// arguments, a zero to index them by, a pointer into the state record, the
/// closure copied out of that record, and the element copied out of `arg`.
const E_STATE: u32 = 0;
const E_INDEX: u32 = 8;
const E_ARG: u32 = 16;
const E_OUT: u32 = 24;
const E_ZERO: u32 = 32;
const E_CTXP: u32 = 40;
const E_CLOS: u32 = 48;
const E_ELEM: u32 = 64;

/// The fixed slots an **equality thunk**'s frame opens with: the two values it
/// was pointed at, where the answer goes, a zero to index by, and then the two
/// values themselves copied out.
const Q_A: u32 = 0;
const Q_B: u32 = 8;
const Q_OUT: u32 = 16;
const Q_ZERO: u32 = 24;
const Q_VALUE: u32 = 32;

/// The **state record** a runtime-driven step crosses the C boundary inside.
///
/// ```text
///   0   code     the closure's two words, in `middle::layout`'s order,
///   8   env      so that one load copies both
///   16  frame    the Buri frame the entry thunk is to work in
///   24  ctx...   the step's context arguments, each rounded up to a word
/// ```
///
/// It is written by `rtcall.rs` and read by [`Jit::entry_thunk`], and by
/// nothing else — the runtime is handed the address and passes it back
/// untouched. That is what makes the shape this backend's business rather than
/// part of the runtime contract, and it is why the LLVM backend's record is a
/// different one (it needs no `frame` word: there, a frame is the machine's).
///
/// # Why the context is in here
///
/// A runtime entry **drops** its context: the runtime allocates through
/// `buri_rt_alloc` and has no use for one (`rtcall.rs`). A *step* does not — it
/// is a Buri closure whose signature names the context, because a lambda may
/// not capture one (SPEC 10.6). `platform/host`'s allocators are empty structs and
/// would need no room at all; `platform/effect/testing`'s `TestAllocator` is
/// `struct TestAllocator(I64)` and carries a handle, so a record with nowhere to
/// put one would refuse every file in the conformance corpus.
pub const E_FRAME: u32 = 16;
const E_CTX: u32 = 24;

/// Where each context argument sits inside the record, and how big the record
/// is — which is what the call site puts the entry thunk's frame past.
///
/// `widths` is every step parameter's width in signature order. **Two of them
/// are not in the record**: the last, which is the element and travels through
/// `arg`; and `index`, which travels in its own register because the runtime is
/// the side that knows it. What is left is the contexts, which are the same
/// value at every element and so are written once, here.
///
/// The answer is a vector of offsets *parallel to the contexts*, not to
/// `widths` — a caller zips it against the arguments it supplies, and the two
/// backends check the lengths agree.
pub fn state_shape(widths: &[u32], index: Option<usize>) -> (Vec<u32>, u32) {
    let mut at = E_CTX;
    let mut out = Vec::new();
    for (i, w) in widths.iter().enumerate().take(widths.len().saturating_sub(1)) {
        if index == Some(i) {
            continue;
        }
        out.push(at);
        at += round8(*w);
    }
    (out, round16(at))
}

impl Jit<'_> {
    /// One helper, emitted at the end of the unit. Answers where it starts.
    pub(crate) fn emit_helper(&mut self, prog: &ir::Program, h: &Helper) -> u64 {
        self.region.align_code(4);
        let at = self.region.code_addr();
        match h {
            Helper::Thunk { func, args, boxed } => self.thunk(prog, *func, *args, *boxed),
            Helper::Walk { ty, op } => self.walk_glue(*ty, *op),
            Helper::Elems { ty, op } => self.elems_glue(*ty, *op),
            Helper::Env { op } => {
                self.env_glue(if *op == Op::Copy { ENV_COPY_WORD } else { 0 })
            }
            Helper::Entry { params, ret, index } => {
                self.entry_thunk(params.clone(), *ret, *index)
            }
            Helper::Equal { ty, func } => self.equal_thunk(prog, *ty, *func),
        }
        at
    }

    /// The environment block: its own release function in the first word, the
    /// captured record at [`ENV_FIELDS`].
    ///
    /// `llvm/emit.rs::build_env` allocates the same shape for the same reason — `Ty::Fn` does not record what was captured, so a `decref` of a
    /// closure has no type to derive the release from and the block has to
    /// carry it. Eight bytes per closure, against a closure that could not be
    /// freed.
    pub(crate) fn build_env(
        &mut self,
        prog: &ir::Program,
        code: &ir::Code,
        st: &mut Fn2,
        dest: u32,
        env: ir::ValueId,
    ) {
        let ty = code.ty_of(env);
        let size = self.width_of(prog, ty);
        let (block, one, word) = (st.scratch, st.scratch + 8, st.scratch + 16);
        self.imm_to(one, 1);
        self.emit(
            "elemalloc",
            &[
                ("JIT_D", V::I(u64::from(block))),
                ("JIT_A", V::I(u64::from(one))),
                ("JIT_P", V::I(u64::from(size + ENV_FIELDS))),
                ("JIT_CONT0", V::Fall),
            ],
        );
        let counted = source_ty(prog, ty).filter(|t| self.rc_counted(t));
        let glue = counted.map(|t| self.helper(Helper::Walk { ty: t, op: Op::Release }));
        let copy = counted.map(|t| self.helper(Helper::Walk { ty: t, op: Op::Copy }));
        for (name, at) in [(glue, 0u32), (copy, ENV_COPY_WORD)] {
            match name {
                Some(sym) => self.emit(
                    "imm/64",
                    &[
                        ("JIT_D", V::I(u64::from(word))),
                        ("JIT_M", V::Sym(sym)),
                        ("JIT_CONT", V::Fall),
                    ],
                ),
                None => self.imm_to(word, 0),
            }
            self.emit(
                "pstore/8",
                &[
                    ("JIT_A", V::I(u64::from(block))),
                    ("JIT_B", V::I(u64::from(word))),
                    ("JIT_N", V::I(u64::from(at))),
                    ("JIT_CONT", V::Fall),
                ],
            );
        }
        if size > 0 {
            self.elem_store(st.at(env), block, one, ENV_FIELDS, size);
        }
        self.mv(dest, block, 8);
    }

    /// The environment glue, which is the same five instructions for every
    /// closure: the block's word at `word` is the release (or copy) function of
    /// whatever was captured, and the record follows it.
    ///
    /// Hand-assembled rather than emitted from stencils because the call it
    /// makes is an indirect **tail** call — there is nothing to do after it —
    /// and no stencil in the library has that shape.
    fn env_glue(&mut self, word: u32) {
        if !self.target.is_arm64() {
            let mut a = X86::new();
            a.ldr(RSI, RDI, word);
            let done = a.cbz_x(RSI);
            a.add_imm(RDI, ENV_FIELDS);
            a.jmp_reg(RSI);
            a.here(done);
            a.ret();
            let (bytes, _) = a.finish();
            self.region.put(&bytes);
            return;
        }
        let mut a = Asm::new();
        a.ldr(1, 0, word);
        let done = a.cbz_x(1);
        a.add_imm(0, 0, ENV_FIELDS);
        a.br_reg(1);
        a.here(done);
        a.ret();
        let (bytes, _) = a.finish();
        self.region.put(&bytes);
    }

    /// `fn(*mut u8)` over one value of `ty`: the drop glue, the per-element
    /// retain `cli/runtime/list.rs` takes, or the copy glue. A copy is all
    /// replacement, so the value goes back through the pointer it came in on.
    ///
    /// A value no one load copies is walked in place instead
    /// ([`Jit::walk_in_place`]).
    fn walk_glue(&mut self, ty: Ty, op: Op) {
        let size = self.layouts_of(ty).size.max(8);
        if !self.one_load(size) {
            return self.walk_glue_in_place(ty, op);
        }
        let frame = frame_for(size);
        if !self.glue_stub(frame) {
            return;
        }
        let mut st = self.glue_frame(frame, G_VALUE + round8(size));
        self.imm_to(G_INDEX, 0);
        self.elem_load(G_VALUE, G_PTR, G_INDEX, 8, size);
        let base = self.fixups_len();
        if let Err(why) = self.walk_rc(&mut st, &ty, G_VALUE, op, 0) {
            self.unsupported(why);
        }
        if op == Op::Copy {
            self.imm_to(G_INDEX, 0);
            self.elem_store(G_VALUE, G_PTR, G_INDEX, 8, size);
        }
        self.emit("ret", &[]);
        self.resolve_helper_blocks(base, &st);
    }

    /// [`Jit::walk_glue`] for a value no one load copies.
    ///
    /// A retain or a release reads one word per pointer and tag
    /// ([`Jit::walk_through`]), so its frame holds one word. A copy loads
    /// whole fields ([`Jit::walk_in_place`]), so its frame holds the widest.
    fn walk_glue_in_place(&mut self, ty: Ty, op: Op) {
        let window = if op == Op::Copy { self.in_place_window(&ty) } else { 8 };
        let frame = frame_for(window);
        if !self.glue_stub(frame) {
            return;
        }
        let mut st = self.glue_frame(frame, G_VALUE + window);
        // One, so that an indexed load at stride `offset` reads `G_PTR + offset`.
        self.imm_to(G_INDEX, 1);
        let base = self.fixups_len();
        let walked = if op == Op::Copy {
            self.walk_in_place(&mut st, &ty, op)
        } else {
            self.walk_through(&mut st, &ty, 0, op, 0)
        };
        if let Err(why) = walked {
            self.unsupported(why);
        }
        self.emit("ret", &[]);
        self.resolve_helper_blocks(base, &st);
    }

    /// A retain or a release over the value at `G_PTR + at`, reading only the
    /// words it counts or tests: each block pointer, tag and niche word is
    /// loaded on its own ([`Jit::load_through`]) and nothing else is.
    ///
    /// The same walk as `Jit::walk_rc`, site for site and depth for depth, with
    /// offsets relative to the pointer instead of the frame.
    fn walk_through(&mut self, st: &mut Fn2, ty: &Ty, at: u32, op: Op, depth: u32) -> Result<(), String> {
        if depth > RC_DEPTH {
            return Err(String::from("a reference-counted type nested past the walk's depth"));
        }
        let sites = self.rc_sites(ty);
        for site in sites.iter() {
            match site {
                Site::Block { offset, glue } => {
                    self.load_through(G_VALUE, at + offset, 8)?;
                    self.block_op(st, G_VALUE, glue, op)?;
                }
                Site::Field(f) => self.field_through(st, f, at, op, depth)?,
                Site::Tagged { tag, arms } => {
                    self.load_through(G_SPARE, at, tag.size())?;
                    let done = st.label();
                    for (i, arm) in arms.iter().enumerate() {
                        let next = st.label();
                        let key = self.arm_key("brcmp/eq/u64/fi", "JIT_T");
                        self.emit(
                            key,
                            &[
                                ("JIT_A", V::I(u64::from(G_SPARE))),
                                ("JIT_K", V::I(u64::from(arm.variant))),
                                ("JIT_T", V::Fall),
                                ("JIT_F", V::Blk(next)),
                            ],
                        );
                        for f in arm.fields.iter() {
                            self.field_through(st, f, at, op, depth)?;
                        }
                        // The last arm's `next` is `done`, so it falls there.
                        if i + 1 < arms.len() {
                            self.emit("jump", &[("JIT_T", V::Blk(done))]);
                        }
                        let here = self.region.code_addr();
                        st.place(next, here);
                    }
                    let here = self.region.code_addr();
                    st.place(done, here);
                }
                Site::Guarded { null_at, ty } => {
                    self.load_through(G_SPARE, at + null_at, 8)?;
                    let skip = st.label();
                    let key = self.arm_key("brcmp/eq/u64/fi", "JIT_F");
                    self.emit(
                        key,
                        &[
                            ("JIT_A", V::I(u64::from(G_SPARE))),
                            ("JIT_K", V::I(0)),
                            ("JIT_T", V::Blk(skip)),
                            ("JIT_F", V::Fall),
                        ],
                    );
                    self.walk_through(st, ty, at, op, depth + 1)?;
                    let here = self.region.code_addr();
                    st.place(skip, here);
                }
            }
        }
        Ok(())
    }

    /// One field of [`Jit::walk_through`]'s value, by `Jit::walk_field`'s
    /// rules: a box is one pointer, a heavy compound field deep in the walk
    /// goes to its type's glue by address, and anything else is walked inline.
    fn field_through(&mut self, st: &mut Fn2, f: &Field, at: u32, op: Op, depth: u32) -> Result<(), String> {
        let at = at + f.offset;
        if f.boxed {
            self.load_through(G_VALUE, at, 8)?;
            let here = Field { offset: 0, ..f.clone() };
            return self.walk_field(st, &here, G_VALUE, op, depth);
        }
        if self.field_out_of_line(&f.ty, depth) {
            let addr = self.address_of(at);
            let sym = self.helper(Helper::Walk { ty: f.ty, op });
            return self.c_call_sym(sym, st, &[Src::Word(addr)], &[], 0, "v");
        }
        self.walk_through(st, &f.ty, at, op, depth + 1)
    }

    /// `frame[dst] = *(G_PTR + offset)`, `bytes` wide and zero-extended to a
    /// word, as three instructions rather than an indexed-load stencil.
    ///
    /// `x9`–`x11` on arm64 and `rax`, `rdx` on x86-64 are as free between a
    /// glue body's stencils as [`Jit::first_nonzero_word`]'s register.
    fn load_through(&mut self, dst: u32, offset: u32, bytes: u32) -> Result<(), String> {
        if !matches!(bytes, 1 | 2 | 4 | 8) {
            return Err(format!("a {bytes}-byte tag read through a pointer"));
        }
        if !self.target.is_arm64() {
            let mut a = X86::new();
            a.ldr(RDX, RDI, G_PTR);
            match bytes {
                1 => a.ldrb(RAX, RDX, offset),
                2 => a.ldrh(RAX, RDX, offset),
                4 => a.ldr_w(RAX, RDX, offset),
                _ => a.ldr(RAX, RDX, offset),
            }
            a.str_off(RAX, RDI, dst);
            let (code, _) = a.finish();
            self.region.put(&code);
            return Ok(());
        }
        let mut a = Asm::new();
        a.ldr(10, 0, G_PTR);
        // The unsigned-offset forms scale by the width and take twelve bits.
        let off = if offset.is_multiple_of(bytes) && offset / bytes <= 0xfff {
            offset
        } else {
            a.mov_imm(11, u64::from(offset));
            a.add_reg(10, 10, 11);
            0
        };
        match bytes {
            1 => a.ldrb(9, 10, off),
            2 => a.ldrh(9, 10, off),
            4 => a.ldr_w(9, 10, off),
            _ => a.ldr(9, 10, off),
        }
        a.str_off(9, 0, dst);
        let (code, _) = a.finish();
        self.region.put(&code);
        Ok(())
    }

    /// The widest stretch [`Jit::walk_in_place`] copies into the frame for a
    /// value of `ty`, rounded to a word: at least one, for a tag or a niche's
    /// pointer.
    fn in_place_window(&mut self, ty: &Ty) -> u32 {
        let mut widest = 8;
        for site in self.rc_sites(ty).iter() {
            let fields: Vec<Field> = match site {
                Site::Field(f) => vec![f.clone()],
                Site::Tagged { arms, .. } => {
                    arms.iter().flat_map(|arm| arm.fields.iter().cloned()).collect()
                }
                Site::Guarded { ty, .. } => vec![Field { offset: 0, boxed: false, ty: *ty }],
                Site::Block { .. } => Vec::new(),
            };
            for f in fields {
                let width = self.field_width(&f);
                if self.one_load(width) {
                    widest = widest.max(round8(width));
                }
            }
        }
        widest
    }

    /// How many bytes of its owner a counted field occupies: a word when it is
    /// boxed, and its type's size otherwise.
    fn field_width(&mut self, f: &Field) -> u32 {
        if f.boxed {
            8
        } else {
            self.layouts_of(f.ty).size
        }
    }

    /// Whether one fixed-width `eload` copies `bytes`. Any other width is
    /// `eload/n`, a `memcpy` call.
    fn one_load(&self, bytes: u32) -> bool {
        self.has(&key!["eload/", bytes])
    }

    /// `op` over the value at `G_PTR`, read through the pointer rather than
    /// out of a copy of the whole value.
    ///
    /// Each counted field is copied into the frame on its own, walked there by
    /// the ordinary `walk_field`, and for a copy written back. A tag or a
    /// niche's pointer is read the same way and tested in the frame. A field no
    /// one load copies goes to its own type's glue, which walks it in place in
    /// turn.
    ///
    /// Only the value's own sites are read through the pointer, so every
    /// offset here is relative to `G_PTR`: anything deeper is inside a field
    /// that was copied in or handed on.
    fn walk_in_place(&mut self, st: &mut Fn2, ty: &Ty, op: Op) -> Result<(), String> {
        let sites = self.rc_sites(ty);
        for site in sites.iter() {
            match site {
                Site::Field(f) => self.field_in_place(st, f, op)?,
                Site::Tagged { tag, arms } => {
                    self.load_at(G_SPARE, 0, tag.size());
                    let done = st.label();
                    for (i, arm) in arms.iter().enumerate() {
                        let next = st.label();
                        let key = self.arm_key("brcmp/eq/u64/fi", "JIT_T");
                        self.emit(
                            key,
                            &[
                                ("JIT_A", V::I(u64::from(G_SPARE))),
                                ("JIT_K", V::I(u64::from(arm.variant))),
                                ("JIT_T", V::Fall),
                                ("JIT_F", V::Blk(next)),
                            ],
                        );
                        for f in arm.fields.iter() {
                            self.field_in_place(st, f, op)?;
                        }
                        // The last arm's `next` is `done`, so it falls there.
                        if i + 1 < arms.len() {
                            self.emit("jump", &[("JIT_T", V::Blk(done))]);
                        }
                        let here = self.region.code_addr();
                        st.place(next, here);
                    }
                    let here = self.region.code_addr();
                    st.place(done, here);
                }
                Site::Guarded { null_at, ty } => {
                    self.load_at(G_SPARE, *null_at, 8);
                    let skip = st.label();
                    let key = self.arm_key("brcmp/eq/u64/fi", "JIT_F");
                    self.emit(
                        key,
                        &[
                            ("JIT_A", V::I(u64::from(G_SPARE))),
                            ("JIT_K", V::I(0)),
                            ("JIT_T", V::Blk(skip)),
                            ("JIT_F", V::Fall),
                        ],
                    );
                    let payload = Field { offset: 0, boxed: false, ty: *ty };
                    self.field_in_place(st, &payload, op)?;
                    let here = self.region.code_addr();
                    st.place(skip, here);
                }
                // A `Str`, a `[T]` and a closure are a few words, so a value
                // with one of these at its top is never this wide.
                Site::Block { .. } => {
                    return Err(String::from("a counted block too wide for a glue frame"));
                }
            }
        }
        Ok(())
    }

    /// One counted field of the value at `G_PTR`: loaded into the frame and
    /// walked there when one load copies it, and handed to its type's glue
    /// when none does.
    fn field_in_place(&mut self, st: &mut Fn2, f: &Field, op: Op) -> Result<(), String> {
        let width = self.field_width(f);
        if !self.one_load(width) {
            let at = self.address_of(f.offset);
            let sym = self.helper(Helper::Walk { ty: f.ty, op });
            return self.c_call_sym(sym, st, &[Src::Word(at)], &[], 0, "v");
        }
        self.load_at(G_VALUE, f.offset, width);
        let here = Field { offset: 0, ..f.clone() };
        self.walk_field(st, &here, G_VALUE, op, 0)?;
        if op == Op::Copy {
            self.elem_store(G_VALUE, G_PTR, G_INDEX, f.offset, width);
        }
        Ok(())
    }

    /// The frame word holding `G_PTR + offset`: `G_PTR` itself at zero, and
    /// otherwise `G_COUNT`, which a walk of one value has no count for.
    fn address_of(&mut self, offset: u32) -> u32 {
        if offset == 0 {
            return G_PTR;
        }
        self.emit(
            "bin/add/u64/fi/f",
            &[
                ("JIT_D", V::I(u64::from(G_COUNT))),
                ("JIT_A", V::I(u64::from(G_PTR))),
                ("JIT_K", V::I(u64::from(offset))),
                ("JIT_CONT", V::Fall),
            ],
        );
        G_COUNT
    }

    /// `frame[dst] = *(G_PTR + offset)`, `bytes` wide, in one stencil: `G_INDEX`
    /// holds one throughout an in-place walk, so `offset` is the stride.
    fn load_at(&mut self, dst: u32, offset: u32, bytes: u32) {
        self.elem_load(dst, G_PTR, G_INDEX, offset, bytes);
    }

    /// `fn(*mut u8)` over every element of a `[T]` block.
    ///
    /// The count is `cap / stride`, and `cap` is the second header word
    /// (VALUE-MODEL.md §2) — which is what makes a drop glue taking only a
    /// pointer enough for a whole list. `llvm/emit.rs::elems_glue`
    /// reads the same word and divides by the same stride. Headroom past the
    /// last element is zeroed and skipped ([`Jit::unless_spare`]).
    ///
    /// Bit 63 of the word is the reserved multi-threaded mark
    /// (`layout::CAP_SHARED_FLAG`), so the load is masked with [`CAP_MASK`]
    /// before the divide — a set bit would turn this loop into a walk over
    /// 2^60 elements of a block that holds a handful.
    ///
    /// A copy stores each element back once it is replaced.
    ///
    /// An element no one load copies is tested where it is
    /// ([`Jit::unless_spare_in_place`]) and handed to its type's glue through a
    /// pointer, which walks it in place.
    fn elems_glue(&mut self, ty: Ty, op: Op) {
        let l = self.layouts_of(ty);
        let (size, stride) = (l.size.max(1), l.stride.max(1));
        let wide = !self.one_load(stride);
        // The frame holds a whole *stride*, padding and all, because
        // [`Jit::unless_spare`] reads every byte of the slot.
        let window = if wide { 8 } else { round8(stride) };
        let frame = frame_for(window);
        if !self.glue_stub(frame) {
            return;
        }
        let mut st = self.glue_frame(frame, G_VALUE + window);
        // `cap` lives eight bytes below the payload pointer, so the header
        // address is formed first and read as an ordinary indexed load.
        self.emit(
            "bin/sub/u64/fi/f",
            &[
                ("JIT_D", V::I(u64::from(G_SPARE))),
                ("JIT_A", V::I(u64::from(G_PTR))),
                ("JIT_K", V::I(8)),
                ("JIT_CONT", V::Fall),
            ],
        );
        self.imm_to(G_INDEX, 0);
        self.elem_load(G_COUNT, G_SPARE, G_INDEX, 8, 8);
        self.emit(
            "bin/and/u64/fi/f",
            &[
                ("JIT_D", V::I(u64::from(G_COUNT))),
                ("JIT_A", V::I(u64::from(G_COUNT))),
                ("JIT_K", V::I(CAP_MASK)),
                ("JIT_CONT", V::Fall),
            ],
        );
        self.emit(
            "bin/div/u64/fi/f",
            &[
                ("JIT_D", V::I(u64::from(G_COUNT))),
                ("JIT_A", V::I(u64::from(G_COUNT))),
                ("JIT_K", V::I(u64::from(stride))),
                ("JIT_CONT", V::Fall),
            ],
        );
        let base = self.fixups_len();
        let body = st.label();
        let done = st.label();
        let next = st.label();
        self.glue_loop_test(G_INDEX, G_COUNT, V::Fall, V::Blk(done), "JIT_T");
        let here = self.region.code_addr();
        st.place(body, here);
        if wide {
            self.unless_spare_in_place(stride, next);
            let sym = self.helper(Helper::Walk { ty, op });
            if let Err(why) = self.c_call_sym(sym, &st, &[Src::Word(G_SPARE)], &[], 0, "v") {
                self.unsupported(why);
            }
        } else {
            self.unless_spare(stride, next);
            if let Err(why) = self.walk_rc(&mut st, &ty, G_VALUE, op, 0) {
                self.unsupported(why);
            }
            if op == Op::Copy {
                self.elem_store(G_VALUE, G_PTR, G_INDEX, stride, size);
            }
        }
        let here = self.region.code_addr();
        st.place(next, here);
        self.emit(
            "bin/add/u64/fi/f",
            &[
                ("JIT_D", V::I(u64::from(G_INDEX))),
                ("JIT_A", V::I(u64::from(G_INDEX))),
                ("JIT_K", V::I(1)),
                ("JIT_CONT", V::Fall),
            ],
        );
        self.glue_loop_test(G_INDEX, G_COUNT, V::Blk(body), V::Fall, "JIT_F");
        let here = self.region.code_addr();
        st.place(done, here);
        self.emit("ret", &[]);
        self.resolve_helper_blocks(base, &st);
    }

    /// Loads element `G_INDEX` of the block into the frame, **all `stride`
    /// bytes** of it, and branches to `skip` when every one of them is zero.
    ///
    /// A `[T]` block's walks run to `cap / stride`, and `cli/runtime/list.rs`'s
    /// `append_dest` grows a block of counted elements with **zeroed headroom**
    /// past the last element, so these loops meet slots nothing was written
    /// to. Skipping them is exact rather than a guess: a reference is a
    /// non-null pointer, so an all-zero element holds none, whether it is
    /// headroom or a value such as `.None` that really was stored. Walking one
    /// instead is not harmless: a boxed field is released without a null test,
    /// and a zero discriminant names a variant that may have one.
    /// `llvm/emit.rs`'s `unless_spare` is the same test.
    ///
    /// The whole stride rather than the value's `size`, so the last frame word
    /// holds the slot's own bytes rather than whatever the frame held before.
    fn unless_spare(&mut self, stride: u32, skip: u32) {
        let words = round8(stride) / 8;
        if !stride.is_multiple_of(8) {
            // The load fills the low bytes of the last word and no more.
            self.imm_to(G_VALUE + (words - 1) * 8, 0);
        }
        self.elem_load(G_VALUE, G_PTR, G_INDEX, stride, stride);
        let any = if words > 1 {
            self.first_nonzero_word(words);
            G_SPARE
        } else {
            G_VALUE
        };
        let key = self.arm_key("brcmp/eq/u64/fi", "JIT_F");
        self.emit(
            key,
            &[
                ("JIT_A", V::I(u64::from(any))),
                ("JIT_K", V::I(0)),
                ("JIT_T", V::Blk(skip)),
                ("JIT_F", V::Fall),
            ],
        );
    }

    /// [`Jit::unless_spare`] for an element no one load copies, read where it
    /// is rather than copied in: `G_SPARE` is set to the element's address,
    /// and the first non-zero word of it is written to `G_VALUE` and tested.
    ///
    /// Only the stride's whole words are read, so the last element's read
    /// stays inside the block. A counted element is word-aligned, so its
    /// stride is whole words anyway.
    fn unless_spare_in_place(&mut self, stride: u32, skip: u32) {
        self.emit(
            "bin/mul/u64/fi/f",
            &[
                ("JIT_D", V::I(u64::from(G_SPARE))),
                ("JIT_A", V::I(u64::from(G_INDEX))),
                ("JIT_K", V::I(u64::from(stride))),
                ("JIT_CONT", V::Fall),
            ],
        );
        self.emit(
            "bin/add/u64/ff/f",
            &[
                ("JIT_D", V::I(u64::from(G_SPARE))),
                ("JIT_A", V::I(u64::from(G_SPARE))),
                ("JIT_B", V::I(u64::from(G_PTR))),
                ("JIT_CONT", V::Fall),
            ],
        );
        self.first_nonzero_word_at(stride / 8);
        let key = self.arm_key("brcmp/eq/u64/fi", "JIT_F");
        self.emit(
            key,
            &[
                ("JIT_A", V::I(u64::from(G_VALUE))),
                ("JIT_K", V::I(0)),
                ("JIT_T", V::Blk(skip)),
                ("JIT_F", V::Fall),
            ],
        );
    }

    /// [`Jit::first_nonzero_word`] over the `words` words at the address in
    /// `G_SPARE`, answered into `G_VALUE`.
    ///
    /// The address goes in a second scratch register, `x10` or `rdx`, which
    /// is as free as the first between a glue body's stencils. arm64's `ldr`
    /// reaches 32760 bytes past its base, so the base steps forward every
    /// 4088 bytes, which one `add` immediate names.
    fn first_nonzero_word_at(&mut self, words: u32) {
        if !self.target.is_arm64() {
            let mut a = X86::new();
            a.ldr(RDX, RDI, G_SPARE);
            let mut found = Vec::new();
            for w in 0..words {
                a.ldr(RAX, RDX, w * 8);
                found.push(a.cbnz_x(RAX));
            }
            for p in found {
                a.here(p);
            }
            a.str_off(RAX, RDI, G_VALUE);
            let (bytes, _) = a.finish();
            self.region.put(&bytes);
            return;
        }
        const STEP: u32 = 4088;
        let mut a = Asm::new();
        a.ldr(10, 0, G_SPARE);
        let mut found = Vec::new();
        let mut base = 0;
        for w in 0..words {
            if w * 8 - base >= STEP {
                a.add_imm(10, 10, STEP);
                base += STEP;
            }
            a.ldr(9, 10, w * 8 - base);
            found.push(a.cbnz_x(9));
        }
        for p in found {
            a.here(p);
        }
        a.str_off(9, 0, G_VALUE);
        let (bytes, _) = a.finish();
        self.region.put(&bytes);
    }

    /// Writes the first non-zero word of the `words` frame words at `G_VALUE`
    /// into `G_SPARE`, or zero when every one of them is zero.
    ///
    /// Hand-assembled, a load and a branch per word, because the stencil
    /// spelling of the same test was an `or` per word through a frame slot:
    /// four instructions a word, each waiting on the store before it. A
    /// 792-byte element is ninety-nine words, and in a release glue walking a
    /// list of them that chain was most of the glue's time. Here a live element
    /// usually stops at its first word.
    ///
    /// The one scratch register is `x9` on arm64 and `rax` on x86-64, neither
    /// of which is a CPS register (`abi::CPS_REGISTER_COUNT`), and nothing
    /// lives in a register across a glue body's stencils anyway.
    fn first_nonzero_word(&mut self, words: u32) {
        if !self.target.is_arm64() {
            let mut a = X86::new();
            let mut found = Vec::new();
            for w in 0..words {
                a.ldr(RAX, RDI, G_VALUE + w * 8);
                found.push(a.cbnz_x(RAX));
            }
            for p in found {
                a.here(p);
            }
            a.str_off(RAX, RDI, G_SPARE);
            let (bytes, _) = a.finish();
            self.region.put(&bytes);
            return;
        }
        let mut a = Asm::new();
        let mut found = Vec::new();
        for w in 0..words {
            a.ldr(9, 0, G_VALUE + w * 8);
            found.push(a.cbnz_x(9));
        }
        for p in found {
            a.here(p);
        }
        a.str_off(9, 0, G_SPARE);
        let (bytes, _) = a.finish();
        self.region.put(&bytes);
    }

    fn glue_loop_test(&mut self, i: u32, n: u32, tv: V, fv: V, fall: &str) {
        let key = self.arm_key("brcmp/lt/u64/ff", fall);
        self.emit(
            key,
            &[
                ("JIT_A", V::I(u64::from(i))),
                ("JIT_B", V::I(u64::from(n))),
                ("JIT_T", tv),
                ("JIT_F", fv),
            ],
        );
    }

    /// The eight instructions in front of a glue body: a machine-stack frame,
    /// the C argument stored into its first slot, and a `bl` into the
    /// frame-threaded code that follows.
    ///
    /// Answers `false` when the frame is wider than one `sub sp` immediate can
    /// name, in which case nothing has been emitted and the caller has already
    /// recorded a refusal.
    fn glue_stub(&mut self, frame: u32) -> bool {
        if frame > MAX_GLUE_FRAME {
            self.unsupported(format!(
                "a value needing {frame} bytes of drop glue frame, past what one \
                 `sub sp` immediate names"
            ));
            // The refusal is the whole answer: a unit with one emits no object.
            self.emit("ret", &[]);
            return false;
        }
        if !self.target.is_arm64() {
            self.glue_stub_x86_64(frame);
            return true;
        }
        let mut a = Asm::new();
        a.str_pre16(30, SP);
        a.sub_imm(SP, SP, frame);
        a.str_off(0, SP, G_PTR);
        a.add_imm(0, SP, 0);
        // Three instructions stand between this one and the body.
        a.bl_words(4);
        a.add_imm(SP, SP, frame);
        a.ldr_post16(30, SP);
        a.ret();
        let (bytes, _) = a.finish();
        self.region.put(&bytes);
        true
    }

    /// [`Jit::glue_stub`] for SysV x86-64.
    ///
    /// The return address is already on the machine stack — `call` put it
    /// there — so nothing has to be saved the way `x30` does, and the one
    /// `push` here is alignment: `rsp % 16` is 8 on entry, and SysV wants 0 at
    /// the `call` below. `frame` is a multiple of sixteen, so the push is the
    /// whole of the correction and the frame base stays sixteen-aligned, which
    /// is what every `middle::layout` offset in the body is computed against.
    fn glue_stub_x86_64(&mut self, frame: u32) {
        let mut a = X86::new();
        a.push_rbp();
        a.sub_imm(RSP, frame);
        a.str_off(RDI, RSP, G_PTR);
        a.mov_reg(RDI, RSP);
        // `add rsp` is seven bytes and `pop`/`ret` one each: nine bytes stand
        // between the end of this call and the body.
        a.call_ahead(9);
        a.add_imm(RSP, frame);
        a.pop_rbp();
        a.ret();
        let (bytes, _) = a.finish();
        self.region.put(&bytes);
    }

    /// The per-function state a generated body needs: no values, no registers,
    /// and a scratch area past the named slots.
    fn glue_frame(&mut self, frame: u32, scratch: u32) -> Fn2 {
        Fn2 {
            slot: Vec::new(),
            blk: Vec::new(),
            frame: FrameSig {
                ret: Vec::new(),
                ret_size: 0,
                params: Vec::new(),
                param_end: scratch,
                size: frame,
            },
            scratch,
            reg: Vec::new(),
            wt: Vec::new(),
            cross: Vec::new(),
            region: Vec::new(),
            cur: 0,
            constants: Vec::new(),
            folded: Vec::new(),
            uses: Vec::new(),
        }
    }

    /// `extern "C" fn(state, index, arg, out)` — one step of a runtime-driven
    /// call.
    ///
    /// This is [`Jit::thunk`]'s problem from the other side. A thunk converts a
    /// closure's environment for a Buri caller; an entry thunk converts *three
    /// C pointers and a counter* for one, and it is the only thing that ever
    /// calls a Buri closure from outside the frame-threaded world.
    ///
    /// The sequence is six moves and a call:
    ///
    /// ```text
    ///   the closure `{ code, env }`, out of the state record
    ///   the index, out of its own C argument, where the key names one
    ///   the element, out of `arg` and into the step's parameter slot
    ///   a retain on it — the step owns what it is handed (`middle/rc.rs`)
    ///   `calli` through `code`, into the ordinary thunk
    ///   the answer, out of the step's frame and through `out`
    /// ```
    ///
    /// The step's **context** arguments come out of the record rather than out
    /// of `arg`: they are the same value at every element, and a C signature
    /// has no parameter for one. The *index* is the opposite case and so is
    /// neither — it changes at every element and nothing here can derive it —
    /// so it has a C argument of its own. A zero-sized context costs nothing here and a
    /// context carrying a handle costs a copy, which is the whole of the
    /// difference between `platform/host`'s allocators and
    /// `platform/effect/testing`'s.
    fn entry_thunk(&mut self, params: Vec<Ty>, ret: Ty, index: Option<usize>) {
        let Some(elem) = params.last().cloned() else {
            self.unsupported(String::from("a runtime-driven step taking no argument"));
            self.emit("ret", &[]);
            return;
        };
        let widths: Vec<u32> =
            params.iter().map(|t| self.layouts_of(*t).size).collect();
        let (ctx_at, _) = state_shape(&widths, index);
        let elem_l = self.layouts_of(elem);
        let (elem_size, elem_slot) = (elem_l.size, round8(elem_l.size).max(8));
        let ret_l = self.layouts_of(ret);
        let (ret_size, ret_slot) = (ret_l.size, round8(ret_l.size).max(8));

        let scratch = E_ELEM + elem_slot;
        let frame = round16(scratch + SCRATCH_BYTES);
        self.entry_stub();
        let mut st = self.glue_frame(frame, scratch);
        let base = self.fixups_len();

        // The step's own frame, `[ret][env: 8][params...]`, laid out exactly as
        // `Jit::call_indirect` lays it out: what a `calli` enters is the
        // thunk, and this is the thunk's frame.
        let mut at = frame + ret_slot + 8;
        let mut param_at: Vec<u32> = Vec::new();
        for t in &params {
            param_at.push(at);
            at += round8(self.layouts_of(*t).size).max(8);
        }

        self.imm_to(E_ZERO, 0);
        self.elem_load(E_CLOS, E_STATE, E_ZERO, 8, 16);
        // The step's index, straight from the C argument into the parameter the
        // key names. It is the one parameter that is neither in the record nor
        // in `arg`, because it is the one thing about this call that only the
        // runtime knows.
        if let Some(to) = index.and_then(|i| param_at.get(i).copied()) {
            self.mv(to, E_INDEX, 8);
        }
        // `ctx_at` is parallel to the *contexts*, and `param_at` to the
        // parameters, so the two are walked together rather than by one index:
        // an index parameter sits between them and is in neither.
        let ctx_params: Vec<usize> = (0..params.len().saturating_sub(1))
            .filter(|i| index != Some(*i))
            .collect();
        for (off, i) in ctx_at.iter().copied().zip(ctx_params) {
            let w = widths.get(i).copied().unwrap_or(0);
            let Some(to) = param_at.get(i).copied().filter(|_| w > 0) else { continue };
            self.emit(
                "bin/add/u64/fi/f",
                &[
                    ("JIT_D", V::I(u64::from(E_CTXP))),
                    ("JIT_A", V::I(u64::from(E_STATE))),
                    ("JIT_K", V::I(u64::from(off))),
                    ("JIT_CONT", V::Fall),
                ],
            );
            self.elem_load(to, E_CTXP, E_ZERO, 8, w);
        }
        if elem_size > 0 {
            self.elem_load(E_ELEM, E_ARG, E_ZERO, 8, elem_size);
            // `middle/rc.rs`: a call through a function value owns its
            // arguments. The runtime lends the element and keeps its own count,
            // so the step's is taken here — the same retain `middle::lower`'s
            // list loops place before a call through a closure.
            if self.rc_counted(&elem) {
                if let Err(why) = self.walk_rc(&mut st, &elem, E_ELEM, Op::Retain, 0) {
                    self.unsupported(why);
                }
            }
            if let Some(to) = param_at.last().copied() {
                self.mv(to, E_ELEM, elem_slot);
            }
        }
        self.mv(frame + ret_slot, E_CLOS + ENV_WORD, 8);
        self.emit(
            "calli",
            &[
                ("JIT_A", V::I(u64::from(E_CLOS))),
                ("JIT_N", V::I(u64::from(frame))),
                ("JIT_P", V::I(u64::from(frame))),
                ("JIT_CONT0", V::Fall),
            ],
        );
        if ret_size > 0 {
            self.elem_store(frame, E_OUT, E_ZERO, 8, ret_size);
        }
        self.emit("ret", &[]);
        self.resolve_helper_blocks(base, &st);
    }

    /// The instructions in front of an entry thunk's body: pick the Buri frame
    /// to work in, put the four C arguments into it, and call the
    /// frame-threaded code that follows.
    ///
    /// The frame is a stack of the running task's own
    /// (`task_thread::STEP_STACK_ACQUIRE`), given back after the body. Outside a
    /// task the runtime answers null and the frame is the one the state record
    /// names, which the call site set aside past its own. A step on a task runs
    /// beside its siblings and may park, so it can't share that one.
    ///
    /// The machine stack holds the return address, the four arguments across
    /// the acquire, and the acquired stack across the body.
    fn entry_stub(&mut self) {
        const ARGS: u32 = 0;
        const STACK: u32 = 32;
        const FRAME: u32 = 48;
        if !self.target.is_arm64() {
            // The epilogue after the body's call, emitted twice: once here to
            // measure how far ahead the body starts.
            let epilogue = |a: &mut X86| {
                a.ldr(RDI, RSP, STACK);
                a.call_symbol(task_thread::STEP_STACK_RELEASE);
                a.add_imm(RSP, FRAME);
                a.pop_rbp();
                a.ret();
            };
            let mut measure = X86::new();
            epilogue(&mut measure);
            let ahead = measure.finish().0.len() as i32;

            let mut a = X86::new();
            // `rsp % 16` is 8 on entry; the push makes it 0 and `FRAME` keeps
            // it, so every `call` below is made on a sixteen-aligned stack.
            a.push_rbp();
            a.sub_imm(RSP, FRAME);
            for (i, r) in [RDI, RSI, RDX, RCX].into_iter().enumerate() {
                a.str_off(r, RSP, ARGS + 8 * i as u32);
            }
            a.call_symbol(task_thread::STEP_STACK_ACQUIRE);
            a.str_off(RAX, RSP, STACK);
            a.ldr(RDI, RSP, ARGS);
            let have = a.cbnz_x(RAX);
            a.ldr(RAX, RDI, E_FRAME);
            a.here(have);
            a.ldr(RSI, RSP, ARGS + 8);
            a.ldr(RDX, RSP, ARGS + 16);
            a.ldr(RCX, RSP, ARGS + 24);
            a.str_off(RDI, RAX, E_STATE);
            a.str_off(RSI, RAX, E_INDEX);
            a.str_off(RDX, RAX, E_ARG);
            a.str_off(RCX, RAX, E_OUT);
            a.mov_reg(RDI, RAX);
            a.call_ahead(ahead);
            epilogue(&mut a);
            let (bytes, relocs) = a.finish();
            let at = self.region.put(&bytes);
            for (off, kind, target, addend) in relocs {
                self.region.reloc_with(at + off, kind, target, addend);
            }
            return;
        }
        let mut a = Asm::new();
        a.stp_fp_lr();
        a.sub_imm(SP, SP, FRAME);
        for r in 0..4 {
            a.str_off(r, SP, ARGS + 8 * r);
        }
        a.bl_symbol(task_thread::STEP_STACK_ACQUIRE);
        a.str_off(0, SP, STACK);
        // `x4` for the frame: `x0`..`x3` are the four C arguments again below.
        a.mov_reg(4, 0);
        a.ldr(0, SP, ARGS);
        let have = a.cbnz_x(4);
        a.ldr(4, 0, E_FRAME);
        a.here(have);
        a.ldr(1, SP, ARGS + 8);
        a.ldr(2, SP, ARGS + 16);
        a.ldr(3, SP, ARGS + 24);
        a.str_off(0, 4, E_STATE);
        a.str_off(1, 4, E_INDEX);
        a.str_off(2, 4, E_ARG);
        a.str_off(3, 4, E_OUT);
        a.add_imm(0, 4, 0);
        // Five instructions stand between this one and the body.
        a.bl_words(6);
        a.ldr(0, SP, STACK);
        a.bl_symbol(task_thread::STEP_STACK_RELEASE);
        a.add_imm(SP, SP, FRAME);
        a.ldp_fp_lr();
        a.ret();
        let (bytes, relocs) = a.finish();
        let at = self.region.put(&bytes);
        for (off, kind, target) in relocs {
            self.region.reloc(at + off, kind, target);
        }
    }

    /// `extern "C" fn(frame, a, b, out)` — whether two values of one type are
    /// the same value.
    ///
    /// [`Jit::entry_thunk`]'s shape with the state record taken out of it. What
    /// it calls is `middle::derives`'s generated `Equal` for the type, which is
    /// an ordinary Buri function, so the body is: copy each value out of the
    /// pointer it was handed into the callee's parameter slot, call, and store
    /// the `Bool` through `out`.
    ///
    /// **The values are borrowed.** A write asks whether what it is about to
    /// store is the value already there, and neither side of that question
    /// changes hands — so a count is taken here only where the callee's own
    /// ownership column says it consumes its parameter, which is
    /// [`Jit::thunk`]'s reconciliation at a call with exactly one caller.
    fn equal_thunk(&mut self, prog: &ir::Program, ty: Ty, func: u32) {
        let Some(f) = prog.funcs.get(func as usize) else {
            self.unsupported(format!(
                "an equality over function {func}, which is not in the program"
            ));
            self.emit("ret", &[]);
            return;
        };
        let facts = f.facts.params.clone();
        let ret_ty = f.sig.rets.first().copied();
        let callee = self.frame_sig_of(func as usize);

        let size = self.layouts_of(ty).size;
        let slot = round8(size).max(8);
        let ret_size = ret_ty.map(|t| self.width_of(prog, t)).unwrap_or(0);
        let scratch = Q_VALUE + slot * 2;
        let frame = round16(scratch + SCRATCH_BYTES);
        let cbase = frame;

        self.equal_stub();
        let mut st = self.glue_frame(frame, scratch);
        let base = self.fixups_len();

        self.imm_to(Q_ZERO, 0);
        let counted = self.rc_counted(&ty);
        for (i, (from, at)) in [(Q_A, Q_VALUE), (Q_B, Q_VALUE + slot)].into_iter().enumerate() {
            if size > 0 {
                self.elem_load(at, from, Q_ZERO, 8, size);
            }
            if counted && facts.get(i) == Some(&ir::Ownership::Own) {
                if let Err(why) = self.walk_rc(&mut st, &ty, at, Op::Retain, 0) {
                    self.unsupported(why);
                }
            }
            if let Some(to) = callee.params.get(i).copied() {
                self.mv(cbase + to, at, slot);
            }
        }
        self.emit(
            "call",
            &[
                ("JIT_N", V::I(u64::from(cbase))),
                ("JIT_P", V::I(u64::from(cbase))),
                ("JIT_CALLEE", V::Fn(func)),
                ("JIT_CONT0", V::Fall),
            ],
        );
        if let Some(from) = callee.ret.first().copied().filter(|_| ret_size > 0) {
            self.elem_store(cbase + from, Q_OUT, Q_ZERO, 8, ret_size);
        }
        self.emit("ret", &[]);
        self.resolve_helper_blocks(base, &st);
    }

    /// The instructions in front of an equality thunk's body: its three
    /// pointers into the frame the runtime handed over, and a call into the
    /// frame-threaded code that follows.
    ///
    /// [`Jit::entry_stub`] with one register fewer to shuffle. The frame is
    /// already the first C argument, so nothing has to be read out of a record
    /// to find it, and — as there — no machine-stack frame is made: the Buri
    /// frame exists, and the only thing the machine stack holds is the return
    /// address the stencil chain below would otherwise lose.
    fn equal_stub(&mut self) {
        if !self.target.is_arm64() {
            let mut a = X86::new();
            // `rsp % 16` is 8 on entry and the `call` below wants 0; the push
            // is the whole of the correction, exactly as in `entry_stub`.
            a.push_rbp();
            a.str_off(RSI, RDI, Q_A);
            a.str_off(RDX, RDI, Q_B);
            a.str_off(RCX, RDI, Q_OUT);
            // `pop` and `ret` are one byte each: two bytes stand between the
            // end of this call and the body.
            a.call_ahead(2);
            a.pop_rbp();
            a.ret();
            let (bytes, _) = a.finish();
            self.region.put(&bytes);
            return;
        }
        let mut a = Asm::new();
        a.str_pre16(30, SP);
        a.str_off(1, 0, Q_A);
        a.str_off(2, 0, Q_B);
        a.str_off(3, 0, Q_OUT);
        // Two instructions stand between this one and the body.
        a.bl_words(3);
        a.ldr_post16(30, SP);
        a.ret();
        let (bytes, _) = a.finish();
        self.region.put(&bytes);
    }

    /// `code(fp)` for a closure over `func`.
    ///
    /// The caller laid the frame out as `[rets][env: 8][args...]` — which is
    /// what `Lower::call_indirect` writes — and the callee wants the
    /// environment *record* flat in its own frame. So the body is: copy the
    /// record out of the block, copy the arguments across, call, copy the
    /// results back.
    ///
    /// # Where the two ownership conventions meet
    ///
    /// `middle/rc.rs` states one of them: *"a call through a function value
    /// **owns** its arguments, because a code pointer cannot carry a per-callee
    /// convention"*. The callee has the other — `ir::Facts`'s ownership column,
    /// where a `Str` a lambda only reads is `Borrow` and is never released by
    /// the body. A thunk is the only thing a code pointer ever points at, so it
    /// is the only place the two can be reconciled, and
    /// every backend's thunk reconciles them the same two ways:
    ///
    ///  * an argument the callee **borrows** is released here, after the call;
    ///  * the environment record is **retained** where the callee owns it,
    ///    because those bytes belong to the closure's block.
    fn thunk(&mut self, prog: &ir::Program, func: u32, args: u32, boxed: bool) {
        let Some(f) = prog.funcs.get(func as usize) else {
            self.unsupported(format!("a closure over function {func}, which is not in the program"));
            self.emit("ret", &[]);
            return;
        };
        let sig_params = f.sig.params.clone();
        let sig_rets = f.sig.rets.clone();
        let facts = f.facts.params.clone();
        let skip = sig_params.len().saturating_sub(args as usize);
        let env = skip > 0;

        let mut at = 0u32;
        let mut rets: Vec<u32> = Vec::new();
        for t in &sig_rets {
            rets.push(at);
            at += self.slot_bytes_of(prog, *t);
        }
        let env_at = at;
        at += 8;
        let mut params: Vec<u32> = Vec::new();
        for t in sig_params.iter().skip(skip) {
            params.push(at);
            at += self.slot_bytes_of(prog, *t);
        }
        let scratch = at;
        let frame = round16(at + SCRATCH_BYTES);
        let mut st = self.glue_frame(frame, scratch);
        let base = self.fixups_len();
        let callee = self.frame_sig_of(func as usize);
        let cbase = frame;

        // The environment record, out of the block and into the callee's first
        // parameter. `middle::closures` gives a capture-free lambda no
        // environment parameter at all, which is the `env == false` case and
        // where the block pointer is null.
        if env {
            let w = sig_params.first().map(|t| self.width_of(prog, *t)).unwrap_or(0);
            let to = cbase + callee.params.first().copied().unwrap_or(0);
            if w > 0 && boxed {
                self.imm_to(scratch, 1);
                self.elem_load(to, env_at, scratch, ENV_FIELDS, w);
            }
            let owns = boxed && facts.first() == Some(&ir::Ownership::Own);
            if owns {
                if let Some(ty) = sig_params.first().and_then(|t| source_ty(prog, *t)) {
                    if self.rc_counted(&ty) {
                        if let Err(why) = self.walk_rc(&mut st, &ty, to, Op::Retain, 0) {
                            self.unsupported(why);
                        }
                    }
                }
            }
        }

        // The arguments, and the ones the callee borrows, kept for the release
        // after the call.
        let mut borrowed: Vec<(u32, Ty)> = Vec::new();
        for (j, t) in sig_params.iter().enumerate().skip(skip) {
            let Some(from) = params.get(j - skip).copied() else { continue };
            let Some(to) = callee.params.get(j).copied() else { continue };
            let n = self.slot_bytes_of(prog, *t);
            self.mv(cbase + to, from, n);
            if facts.get(j) != Some(&ir::Ownership::Borrow) {
                continue;
            }
            if let Some(ty) = source_ty(prog, *t) {
                if self.rc_counted(&ty) {
                    borrowed.push((from, ty));
                }
            }
        }

        self.emit(
            "call",
            &[
                ("JIT_N", V::I(u64::from(cbase))),
                ("JIT_P", V::I(u64::from(cbase))),
                ("JIT_CALLEE", V::Fn(func)),
                ("JIT_CONT0", V::Fall),
            ],
        );
        for (i, t) in sig_rets.iter().enumerate() {
            let (Some(to), Some(from)) = (rets.get(i).copied(), callee.ret.get(i).copied()) else {
                continue;
            };
            let n = self.slot_bytes_of(prog, *t);
            self.mv(to, cbase + from, n);
        }
        // The caller handed over a count the callee did not consume. Without
        // this every step of a `list.map` over a `[Str]` leaks one block per
        // element, which is exactly what it did.
        for (at, ty) in borrowed {
            if let Err(why) = self.walk_rc(&mut st, &ty, at, Op::Release, 0) {
                self.unsupported(why);
            }
        }
        self.emit("ret", &[]);
        self.resolve_helper_blocks(base, &st);
    }
}

/// The source type an IR type stands for, where it has one.
fn source_ty(prog: &ir::Program, t: ir::Type) -> Option<Ty> {
    super::rtcall::source_ty(prog, t)
}
