//! The functions one unit generates for itself.
//!
//! Every register-machine backend here generates the same set under the same
//! argument, and the reason each exists is:
//!
//! | Helper | Why it is generated rather than called |
//! |---|---|
//! | [`Helper::Thunk`] | A closure's `code` takes its environment as a **pointer**; a lifted lambda takes it as an aggregate parameter laid out flat in its frame. Something has to convert, and it is also the one place the indirect-call ownership convention meets the callee's own. |
//! | [`Helper::Walk`] | The per-type reference-count walk, as a C function `fn(*mut u8)`: the drop glue [`buri_rt_decref`](cli/runtime/memory.rs) calls, and the per-element retain `cli/runtime/list.rs` is handed. |
//! | [`Helper::Elems`] | The same for a whole `[T]` block, whose element count is `cap / stride`. |
//! | [`Helper::Copy`] | The per-type **copy** walk, the same recursion with allocation where [`Helper::Walk`] has release: what `core/alloc::copyOut` is compiled into, so that a value leaving a scope shares no block with the one it left behind. |
//! | [`Helper::CopyElems`] | The same for a whole `[T]` block, [`Helper::Elems`]'s twin. |
//! | [`Helper::EnvGlue`] | The one indirection that lets a closure environment carry its own drop glue: `Ty::Fn` does not record what was captured, so the block holds the release function in its first word. |
//! | [`Helper::EnvCopy`] | The same indirection for the copy, out of the block's **second** word. `Ty::Fn` is as silent about a copy as it is about a release, and one word is what that silence costs — see [`ENV_FIELDS`]. |
//! | [`Helper::Entry`] | The other direction through the C boundary: a `void(state, index, in, out)` the **runtime** calls to run one Buri step. A closure's `code` has a parameter list that depends on the element type, so the runtime cannot call it; this is generated where that type is known and is the only thing that does. |
//!
//! Every one is a **local** symbol of the unit that needed it, so two units
//! that both drop a `[Str]` get a copy each and neither collides.
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
//! discipline, and it is the word `Helper::Entry`'s stub reads before it does
//! anything else. When the runtime grows real per-task stacks (`design`'s B7)
//! this is the word that changes and nothing else here does.
//!
//! The walk itself reads the value out of a *copy* in that frame rather than
//! through the pointer. That is what lets `Lower::walk_rc` — which addresses
//! everything as a frame offset — serve both an `Inst::DecRef` and a glue
//! function with no second implementation of the walk.

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
use super::jit::{Fn2, FrameSig, Jit, V};
use crate::compiler::middle::ir;
use crate::compiler::middle::layout::{CAP_MASK, CLOSURE_ENV};
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
    /// The counted-pointer walk over one value of a type, as `fn(*mut u8)`.
    Walk { ty: Ty, retain: bool },
    /// The same over every element of a `[T]` block.
    Elems { ty: Ty },
    /// The **copy** walk over one value of a type, as `fn(*mut u8)`: every
    /// counted pointer inside the value it is handed is replaced, in place, by
    /// a pointer to a fresh block holding a copy of the same thing.
    ///
    /// It is [`Helper::Walk`]'s recursion exactly — the same `Repr` arms, the
    /// same tag dispatch, the same depth bound and the same going out of line
    /// when a field is compound and the walk is already deep — with one
    /// substitution: where the walk emits a `decref`, this emits
    /// `buri_rt_copy_block` and stores what it answered. There is no `retain`
    /// column, because a copy has only one direction.
    Copy { ty: Ty },
    /// The same over every element of a `[T]` block, [`Helper::Elems`]'s twin.
    CopyElems { ty: Ty },
    /// Read a release function out of a block's first word and call it on the
    /// rest.
    EnvGlue,
    /// Read a **copy** function out of a block's second word and call it on the
    /// rest.
    EnvCopy,
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
}

/// The symbol a helper is emitted under.
///
/// `$` cannot appear in a Buri path, so no `ir::Func::symbol` can collide with
/// one — the same guarantee `mod.rs`'s pool anchor rests on. The index is the
/// order the unit first asked for it, which is emission order and therefore
/// reproducible.
pub fn symbol(i: usize) -> String {
    format!("buri$stencil$h{i}")
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
/// the two, and it keeps [`Helper::EnvGlue`] the five instructions it was.
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
/// unshifted. A type whose walk needs more than this is refused with a sentence
/// rather than given a shifted encoding nothing else in this file would use.
///
/// x86-64's `sub rsp, imm32` has no such limit and is held to the same number
/// anyway: a refusal that depended on the machine would mean a program compiled
/// for one target and refused for another with nothing a user could act on.
const MAX_GLUE_FRAME: u32 = 4080;

fn round8(n: u32) -> u32 {
    (n + 7) & !7
}

fn round16(n: u32) -> u32 {
    (n + 15) & !15
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
/// not capture one (SPEC 10.6). `core/host`'s allocators are empty structs and
/// would need no room at all; `core/host/testing`'s `TestAlloc` is
/// `struct TestAlloc(I64)` and carries a handle, so a record with nowhere to
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
            Helper::Walk { ty, retain } => self.walk_glue(ty.clone(), *retain),
            Helper::Elems { ty } => self.elems_glue(ty.clone()),
            Helper::Copy { ty } => self.copy_glue(ty.clone()),
            Helper::CopyElems { ty } => self.copy_elems_glue(ty.clone()),
            Helper::EnvGlue => self.env_glue(),
            Helper::EnvCopy => self.env_copy_glue(),
            Helper::Entry { params, ret, index } => {
                self.entry_thunk(params.clone(), ret.clone(), *index)
            }
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
        let glue = counted.clone().map(|t| self.helper(Helper::Walk { ty: t, retain: false }));
        let copy = counted.map(|t| self.helper(Helper::Copy { ty: t }));
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
    /// closure: the block's first word is the release function of whatever was
    /// captured, and the record follows it.
    ///
    /// Hand-assembled rather than emitted from stencils because the call it
    /// makes is an indirect **tail** call — there is nothing to do after it —
    /// and no stencil in the library has that shape.
    fn env_glue(&mut self) {
        if !self.target.is_arm64() {
            let mut a = X86::new();
            a.ldr(RSI, RDI, 0);
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
        a.ldr(1, 0, 0);
        let done = a.cbz_x(1);
        a.add_imm(0, 0, ENV_FIELDS);
        a.br_reg(1);
        a.here(done);
        a.ret();
        let (bytes, _) = a.finish();
        self.region.put(&bytes);
    }

    /// [`Jit::env_glue`]'s twin for the copy, reading the **second** word of
    /// the block instead of the first.
    ///
    /// The same five instructions, and hand-assembled for the same reason: the
    /// call it makes is an indirect tail call and no stencil has that shape.
    fn env_copy_glue(&mut self) {
        if !self.target.is_arm64() {
            let mut a = X86::new();
            a.ldr(RSI, RDI, ENV_COPY_WORD);
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
        a.ldr(1, 0, ENV_COPY_WORD);
        let done = a.cbz_x(1);
        a.add_imm(0, 0, ENV_FIELDS);
        a.br_reg(1);
        a.here(done);
        a.ret();
        let (bytes, _) = a.finish();
        self.region.put(&bytes);
    }

    /// `fn(*mut u8)` over one value of `ty`: the **copy** glue, which replaces
    /// every counted pointer inside the value it is handed with a pointer to a
    /// fresh block of its own.
    ///
    /// [`Jit::walk_glue`]'s shape exactly, and deliberately so: the two are the
    /// same recursion over the same `Repr` arms, and keeping them the same
    /// shape is what makes a new layout case a two-line change in each rather
    /// than a second traversal to keep in step. The one addition is the store
    /// at the end — a walk leaves nothing behind and a copy is all
    /// replacement, so the value has to go back through the pointer it came
    /// in on.
    fn copy_glue(&mut self, ty: Ty) {
        let size = self.layouts_of(ty.clone()).size.max(8);
        let frame = round16(G_VALUE + round8(size) + SCRATCH_BYTES);
        if !self.glue_stub(frame) {
            return;
        }
        let mut st = self.glue_frame(frame, G_VALUE + round8(size));
        self.imm_to(G_INDEX, 0);
        self.elem_load(G_VALUE, G_PTR, G_INDEX, 8, size);
        let base = self.fixups_len();
        if let Err(why) = self.copy_rc(&mut st, &ty, G_VALUE, 0) {
            self.unsupported(why);
        }
        self.imm_to(G_INDEX, 0);
        self.elem_store(G_VALUE, G_PTR, G_INDEX, 8, size);
        self.emit("ret", &[]);
        self.resolve_helper_blocks(base, &st);
    }

    /// [`Jit::elems_glue`] for the copy: every element of a `[T]` block,
    /// replaced in place.
    fn copy_elems_glue(&mut self, ty: Ty) {
        let l = self.layouts_of(ty.clone());
        let (size, stride) = (l.size.max(1), l.stride.max(1));
        let frame = round16(G_VALUE + round8(size) + SCRATCH_BYTES);
        if !self.glue_stub(frame) {
            return;
        }
        let mut st = self.glue_frame(frame, G_VALUE + round8(size));
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
        self.glue_loop_test(G_INDEX, G_COUNT, V::Fall, V::Blk(done), "JIT_T");
        let here = self.region.code_addr();
        st.place(body, here);
        self.elem_load(G_VALUE, G_PTR, G_INDEX, stride, size);
        if let Err(why) = self.copy_rc(&mut st, &ty, G_VALUE, 0) {
            self.unsupported(why);
        }
        self.elem_store(G_VALUE, G_PTR, G_INDEX, stride, size);
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

    /// `fn(*mut u8)` over one value of `ty`: the drop glue, or the per-element
    /// retain `cli/runtime/list.rs` takes.
    fn walk_glue(&mut self, ty: Ty, retain: bool) {
        let size = self.layouts_of(ty.clone()).size.max(8);
        let frame = round16(G_VALUE + round8(size) + SCRATCH_BYTES);
        if !self.glue_stub(frame) {
            return;
        }
        let mut st = self.glue_frame(frame, G_VALUE + round8(size));
        self.imm_to(G_INDEX, 0);
        self.elem_load(G_VALUE, G_PTR, G_INDEX, 8, size);
        let base = self.fixups_len();
        if let Err(why) = self.walk_rc(&mut st, &ty, G_VALUE, retain, 0) {
            self.unsupported(why);
        }
        self.emit("ret", &[]);
        self.resolve_helper_blocks(base, &st);
    }

    /// `fn(*mut u8)` over every element of a `[T]` block.
    ///
    /// The count is `cap / stride`, and `cap` is the second header word
    /// (VALUE-MODEL.md §2) — which is what makes a drop glue taking only a
    /// pointer enough for a whole list. `llvm/emit.rs::release_elems_glue`
    /// reads the same word and divides by the same stride.
    ///
    /// Bit 63 of the word is the reserved multi-threaded mark
    /// (`layout::CAP_SHARED_FLAG`), so the load is masked with [`CAP_MASK`]
    /// before the divide — a set bit would turn this loop into a walk over
    /// 2^60 elements of a block that holds a handful.
    fn elems_glue(&mut self, ty: Ty) {
        let l = self.layouts_of(ty.clone());
        let (size, stride) = (l.size.max(1), l.stride.max(1));
        let frame = round16(G_VALUE + round8(size) + SCRATCH_BYTES);
        if !self.glue_stub(frame) {
            return;
        }
        let mut st = self.glue_frame(frame, G_VALUE + round8(size));
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
        self.glue_loop_test(G_INDEX, G_COUNT, V::Fall, V::Blk(done), "JIT_T");
        let here = self.region.code_addr();
        st.place(body, here);
        self.elem_load(G_VALUE, G_PTR, G_INDEX, stride, size);
        if let Err(why) = self.walk_rc(&mut st, &ty, G_VALUE, false, 0) {
            self.unsupported(why);
        }
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

    fn glue_loop_test(&mut self, i: u32, n: u32, tv: V, fv: V, fall: &str) {
        let key = self.arm_key("brcmp/lt/u64/ff", fall);
        self.emit(
            &key,
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
            constants: Vec::new(),
            folded: Vec::new(),
            closure_of: Vec::new(),
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
    /// difference between `core/host`'s allocators and
    /// `core/host/testing`'s.
    fn entry_thunk(&mut self, params: Vec<Ty>, ret: Ty, index: Option<usize>) {
        let Some(elem) = params.last().cloned() else {
            self.unsupported(String::from("a runtime-driven step taking no argument"));
            self.emit("ret", &[]);
            return;
        };
        let widths: Vec<u32> =
            params.iter().map(|t| self.layouts_of(t.clone()).size).collect();
        let (ctx_at, _) = state_shape(&widths, index);
        let elem_l = self.layouts_of(elem.clone());
        let (elem_size, elem_slot) = (elem_l.size, round8(elem_l.size).max(8));
        let ret_l = self.layouts_of(ret.clone());
        let (ret_size, ret_slot) = (ret_l.size, round8(ret_l.size).max(8));

        let scratch = E_ELEM + elem_slot;
        let frame = round16(scratch + SCRATCH_BYTES);
        self.entry_stub();
        let mut st = self.glue_frame(frame, scratch);
        let base = self.fixups_len();

        // The step's own frame, `[ret][env: 8][params...]`, laid out exactly as
        // `lists.rs::step_shape_ty` lays it out: what a `calli` enters is the
        // thunk, and this is the thunk's frame.
        let mut at = frame + ret_slot + 8;
        let mut param_at: Vec<u32> = Vec::new();
        for t in &params {
            param_at.push(at);
            at += round8(self.layouts_of(t.clone()).size).max(8);
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
            // so the step's is taken here — the same retain `lists.rs` emits
            // before its own `calli`, and for the same sentence.
            if self.rc_counted(&elem) {
                if let Err(why) = self.walk_rc(&mut st, &elem, E_ELEM, true, 0) {
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

    /// The instructions in front of an entry thunk's body: the four C
    /// arguments into the frame the state record names, and a call into the
    /// frame-threaded code that follows.
    ///
    /// Unlike [`Jit::glue_stub`] this one makes **no machine-stack frame** and
    /// so has no width to refuse: the Buri frame it works in already exists —
    /// the call site set it aside past its own — and the only thing the machine
    /// stack holds is the return address, which the stencil chain below would
    /// otherwise lose.
    fn entry_stub(&mut self) {
        if !self.target.is_arm64() {
            let mut a = X86::new();
            // `rsp % 16` is 8 on entry and the `call` below wants 0; the push
            // is the whole of the correction, exactly as in `glue_stub`.
            a.push_rbp();
            a.ldr(RAX, RDI, E_FRAME);
            a.str_off(RDI, RAX, E_STATE);
            a.str_off(RSI, RAX, E_INDEX);
            a.str_off(RDX, RAX, E_ARG);
            a.str_off(RCX, RAX, E_OUT);
            a.mov_reg(RDI, RAX);
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
        // `x4` rather than `x3` for the frame pointer: `x3` is the fourth C
        // argument now, and reading the record into it would lose `out`.
        a.ldr(4, 0, E_FRAME);
        a.str_off(0, 4, E_STATE);
        a.str_off(1, 4, E_INDEX);
        a.str_off(2, 4, E_ARG);
        a.str_off(3, 4, E_OUT);
        a.add_imm(0, 4, 0);
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
    /// what `Lower::call_indirect` writes and what `lists.rs::step_shape_ty`
    /// reconstructs from the closure's type — and the callee wants the
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
                        if let Err(why) = self.walk_rc(&mut st, &ty, to, true, 0) {
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
            if let Err(why) = self.walk_rc(&mut st, &ty, at, false, 0) {
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
