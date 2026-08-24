//! The functions one unit generates for itself.
//!
//! `cranelift/helpers.rs` is the same set under the same argument, and the
//! reason each exists is the reason it gives:
//!
//! | Helper | Why it is generated rather than called |
//! |---|---|
//! | [`Helper::Thunk`] | A closure's `code` takes its environment as a **pointer**; a lifted lambda takes it as an aggregate parameter laid out flat in its frame. Something has to convert, and it is also the one place the indirect-call ownership convention meets the callee's own. |
//! | [`Helper::Walk`] | The per-type reference-count walk, as a C function `fn(*mut u8)`: the drop glue [`buri_rt_decref`](cli/runtime/memory.rs) calls, and the per-element retain `cli/runtime/list.rs` is handed. |
//! | [`Helper::Elems`] | The same for a whole `[T]` block, whose element count is `cap / stride`. |
//! | [`Helper::EnvGlue`] | The one indirection that lets a closure environment carry its own drop glue: `Ty::Fn` does not record what was captured, so the block holds the release function in its first word. |
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
//! A glue function is entered by the *runtime* — `buri_rt_decref(p, glue)` and
//! `cli/runtime/list.rs`'s `retain` — so it is `extern "C" fn(*mut u8)` and
//! every one of them is a hand-written eight-instruction stub in front of a
//! frame-threaded body. The stub's whole job is to make a frame: it takes the
//! **machine** stack for it rather than the Buri stack, because drop glue
//! recurses (a `[[Str]]` releases a `[Str]` releases a `Str`) and a fixed
//! scratch frame would be re-entered by its own callee.
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

use super::asm::{Asm, SP};
use super::jit::{Fn2, FrameSig, Jit, V};
use crate::compiler::middle::ir;
use crate::compiler::middle::layout::CLOSURE_ENV;
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
    /// Read a release function out of a block's first word and call it on the
    /// rest.
    EnvGlue,
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

/// The environment record starts one word into its block; the word before it is
/// the block's own release function. `cranelift/emit.rs::ENV_FIELDS` is the
/// same eight bytes, and the two must agree because both write the same shape.
pub const ENV_FIELDS: u32 = 8;

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

impl Jit<'_> {
    /// One helper, emitted at the end of the unit. Answers where it starts.
    pub(crate) fn emit_helper(&mut self, prog: &ir::Program, h: &Helper) -> u64 {
        self.region.align_code(4);
        let at = self.region.code_addr();
        match h {
            Helper::Thunk { func, args, boxed } => self.thunk(prog, *func, *args, *boxed),
            Helper::Walk { ty, retain } => self.walk_glue(ty.clone(), *retain),
            Helper::Elems { ty } => self.elems_glue(ty.clone()),
            Helper::EnvGlue => self.env_glue(),
        }
        at
    }

    /// The environment block: its own release function in the first word, the
    /// captured record at [`ENV_FIELDS`].
    ///
    /// `cranelift/emit.rs::build_env` allocates the same shape for the same
    /// reason — `Ty::Fn` does not record what was captured, so a `decref` of a
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
        let glue = source_ty(prog, ty)
            .filter(|t| self.rc_counted(t))
            .map(|t| self.helper(Helper::Walk { ty: t, retain: false }));
        match glue {
            Some(name) => self.emit(
                "imm/64",
                &[
                    ("JIT_D", V::I(u64::from(word))),
                    ("JIT_M", V::Sym(name)),
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
                ("JIT_N", V::I(0)),
                ("JIT_CONT", V::Fall),
            ],
        );
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
    /// pointer enough for a whole list. `cranelift/helpers.rs::release_elems`
    /// reads the same word and divides by the same stride.
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
    /// `cranelift/helpers.rs::thunk` reconciles them the same two ways:
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
