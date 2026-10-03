//! `core/list`'s closure surface, open-coded as stencils.
//!
//! # Why the loop is emitted here and not called
//!
//! `cli/runtime/list.rs`'s header says why neither native backend has a
//! `buri_rt_list_map`: a Buri closure is `{ code, env }` where `code`'s
//! signature is the *flattened* one of the element type, so a C function
//! calling one would have to synthesize a parameter list that depends on `T`.
//! A backend already knows how, so the loop lives in the backend.
//!
//! The other road is one descriptor-driven runtime helper per operation,
//! reaching the step through the backend's generic closure call — the
//! callee's frame built with two `memcpy`s and an indirect call **per
//! element**. Three reports running named that boundary as the whole of the K4
//! gap: the L6→L12 ladder moves K4 by 6% while moving K1 by 2.2×,
//! because K4 never reaches the code generator at all. This file is the fix,
//! and it is the same shape `llvm/emit.rs::list_closure` has:
//!
//!   * the loop is emitted **at the call site**, out of ordinary stencils, so
//!     the `Body::Runtime` call disappears entirely;
//!   * the element is read with one indexed-copy stencil (`eload/{n}`) rather
//!     than an address computation and a `memcpy` call;
//!   * and when the step is a `MakeClosure` this function can see — which is
//!     every lambda written at the call site — the call is a **direct** `call`
//!     stencil, not a `calli`. That is `step_shape`'s `direct`, and it is what
//!     lets the callee's frame offsets be read out of `Jit::plan`
//!     instead of guessed from a source type.
//!
//! # The counts
//!
//! Two sentences of `middle/rc.rs`, which `llvm/emit.rs::list_closure` quotes
//! too, decide every reference operation here and they point in opposite
//! directions:
//!
//!  * *"A runtime intrinsic borrows its arguments and returns a fresh count."*
//!    The source list and the step arrive borrowed; nothing here releases one.
//!    A fold's initial accumulator is the exception: `middle::rc` hands it
//!    over (`rc::is_fold`), so it arrives owned and the first step consumes
//!    that count without a retain here.
//!  * *"A call through a function value owns its arguments."* So every other
//!    value handed to a step **through a function value** is retained first,
//!    the step consumes that count, and it answers a fresh one.
//!
//! The second rule is about a call through a function value. A **direct** call
//! obeys the callee's own `Facts::params` instead — and rather than reproduce
//! `helpers::thunk`'s release-where-the-callee-borrowed, this file does what
//! `direct_callee` does: it **refuses the direct call** whenever any parameter
//! is a borrowed, counted one, and takes the `calli` path with its retain. An
//! element type that holds nothing counted — `[Int]`, `[F64]`, a struct of
//! scalars, which is most of them — costs no reference instruction either way,
//! and that is the case every kernel in the corpus is.
//!
//! `filter` retains **twice** per kept element, exactly as
//! `llvm/emit.rs::list_closure`'s filter arm does: once because the predicate
//! is handed
//! a count it consumes, and once because the copy into the result block is a
//! second owner of what the element holds.
//!
//! # What "counted" means here
//!
//! Every backend asks `middle::rc`'s classifier rather than the layout table's,
//! because retaining what rc does not count adds one half of a pair nothing
//! completes. `emit.rs::rc_counted` is this backend's one classifier
//! and `counted` below is that same predicate — the *deep* question, not the
//! top-level repr: a struct of two `Str`s is counted and its layout is
//! `Aggregate`, so the shallow test skipped exactly the retains a step handed
//! one needs.

#![allow(
    clippy::arithmetic_side_effects,
    reason = "every sum here is a byte offset inside a frame `Jit::plan` has \
              already sized: a scratch word past `Fn2::scratch`, the second \
              word of a two-word list, or a step parameter past the callee's \
              frame base. `frame_sigs` laid all three out by accumulating the \
              same slot widths, so the frame that contains them exists before \
              this file adds anything to its base. The rest counts a step's \
              parameters, of which a signature has as many as it has"
)]

use super::jit::{Fn2, Jit, V};
use super::runtime;
use crate::compiler::backend::counts::Op;
use crate::compiler::middle::ir;
use crate::compiler::middle::layout::{EnumRepr, Layout, Repr};
use crate::compiler::semantics::types::Ty;

/// Where a thunk's frame puts its result, its environment and its arguments,
/// as offsets from the frame's base.
struct StepShape {
    /// Byte offset of the step's single result.
    ret: u32,
    /// Byte offset of the environment pointer.
    env: u32,
    /// Byte offsets of the value parameters, after the environment.
    params: Vec<u32>,
}

impl<'a> Jit<'a> {
    /// The retain a value crossing a function value needs: the whole walk, not
    /// one `incref`, because what is handed over may be a struct with a
    /// counted field rather than a bare pointer.
    fn retain_value(&mut self, st: &mut Fn2, ty: &Ty, at: u32) {
        if let Err(why) = self.walk_rc(st, ty, at, Op::Retain, 0) {
            self.unsupported(why);
        }
    }

    /// A `[T]`'s stride, element width and whether the element is counted.
    pub(crate) fn array_elem(&mut self, prog: &ir::Program, t: ir::Type) -> Option<(u32, u32, bool)> {
        let ir::Type::Agg(id) = t else { return None };
        let Ty::Array(elem) = prog.type_info(id).ty.clone() else { return None };
        let l = self.layouts_of((*elem).clone());
        let counted = self.rc_counted(&elem);
        Some((l.stride.max(1), l.size.max(1), counted))
    }

    /// The element type of a value whose IR type is a `[T]`.
    pub(crate) fn element_of(&mut self, prog: &ir::Program, t: ir::Type) -> Option<Ty> {
        let ir::Type::Agg(id) = t else { return None };
        match prog.type_info(id).ty.clone() {
            Ty::Array(elem) => Some(*elem),
            _ => None,
        }
    }

    /// The **thunk's** frame, reconstructed from the closure's *type*.
    ///
    /// What a `calli` enters is never the lifted lambda — it is the thunk
    /// `glue.rs` generates, whose frame is `[rets][env: 8][args...]` with the
    /// environment one word because it is the block's pointer. That is the same
    /// arithmetic `Jit::call_indirect` does at every indirect call site.
    fn step_shape_ty(
        &mut self,
        prog: &ir::Program,
        fty: ir::Type,
        want: usize,
    ) -> Option<StepShape> {
        let ir::Type::Agg(id) = fty else { return None };
        let Ty::Fn(ps, r) = prog.type_info(id).ty.clone() else { return None };
        if ps.len() != want {
            return None;
        }
        let slot = |s: &mut Self, t: &Ty| -> u32 {
            let n = s.layouts_of(t.clone()).size;
            ((n + 7) & !7).max(8)
        };
        let ret_size = slot(self, &r);
        let env = ret_size;
        let mut at = ret_size + 8;
        let mut params = Vec::new();
        for t in &ps {
            params.push(at);
            at += slot(self, t);
        }
        Some(StepShape { ret: 0, env, params })
    }

    /// `frame[dst] = *(base + i * stride)`, `bytes` wide.
    pub(crate) fn elem_load(&mut self, dst: u32, base: u32, i: u32, stride: u32, bytes: u32) {
        // An element narrower than a frame word has to arrive **zero-extended**
        // into a whole one; see `sources.rs`'s `eloadz` family for why.
        if bytes < 8 {
            let zk = if stride == bytes {
                format!("eloadz/{bytes}/s")
            } else {
                format!("eloadz/{bytes}")
            };
            if self.has(&zk) {
                self.emit(
                    &zk,
                    &[
                        ("JIT_D", V::I(u64::from(dst))),
                        ("JIT_A", V::I(u64::from(base))),
                        ("JIT_B", V::I(u64::from(i))),
                        ("JIT_P", V::I(u64::from(stride))),
                        ("JIT_CONT", V::Fall),
                    ],
                );
                return;
            }
            // 3, 5, 6, 7 bytes: no zero-extending twin, so the word is cleared
            // first and the bytes copied over its low half.
            self.imm_to(dst, 0);
        }
        let sk = format!("eload/{bytes}/s");
        let key = if stride == bytes && self.has(&sk) {
            sk
        } else {
            format!("eload/{bytes}")
        };
        if self.has(&key) {
            self.emit(
                &key,
                &[
                    ("JIT_D", V::I(u64::from(dst))),
                    ("JIT_A", V::I(u64::from(base))),
                    ("JIT_B", V::I(u64::from(i))),
                    ("JIT_P", V::I(u64::from(stride))),
                    ("JIT_CONT", V::Fall),
                ],
            );
            return;
        }
        self.emit(
            "eload/n",
            &[
                ("JIT_D", V::I(u64::from(dst))),
                ("JIT_A", V::I(u64::from(base))),
                ("JIT_B", V::I(u64::from(i))),
                ("JIT_P", V::I(u64::from(stride))),
                ("JIT_N", V::I(u64::from(bytes))),
                ("JIT_CONT", V::Fall),
            ],
        );
    }

    /// `*(base + i * stride) = frame[src]`, `bytes` wide.
    pub(crate) fn elem_store(&mut self, src: u32, base: u32, i: u32, stride: u32, bytes: u32) {
        let sk = format!("estore/{bytes}/s");
        let key = if stride == bytes && self.has(&sk) {
            sk
        } else {
            format!("estore/{bytes}")
        };
        if self.has(&key) {
            self.emit(
                &key,
                &[
                    ("JIT_D", V::I(u64::from(src))),
                    ("JIT_A", V::I(u64::from(base))),
                    ("JIT_B", V::I(u64::from(i))),
                    ("JIT_P", V::I(u64::from(stride))),
                    ("JIT_CONT", V::Fall),
                ],
            );
            return;
        }
        self.emit(
            "estore/n",
            &[
                ("JIT_D", V::I(u64::from(src))),
                ("JIT_A", V::I(u64::from(base))),
                ("JIT_B", V::I(u64::from(i))),
                ("JIT_P", V::I(u64::from(stride))),
                ("JIT_N", V::I(u64::from(bytes))),
                ("JIT_CONT", V::Fall),
            ],
        );
    }

    /// `frame[d] = frame[a] + k`.
    fn add_imm(&mut self, d: u32, a: u32, k: u64) {
        self.emit(
            "bin/add/u64/fi/f",
            &[
                ("JIT_D", V::I(u64::from(d))),
                ("JIT_A", V::I(u64::from(a))),
                ("JIT_K", V::I(k)),
                ("JIT_CONT", V::Fall),
            ],
        );
    }

    /// `if (frame[a] < frame[b]) goto tv; else goto fv`, unsigned.
    ///
    /// `fall` names the hole bound to [`V::Fall`], which is the one
    /// [`Jit::arm_key`] has to put on the stencil's tail for the branch to be
    /// dropped rather than patched.
    fn br_lt(&mut self, a: u32, b: u32, tv: V, fv: V, fall: Option<&str>) {
        let base = "brcmp/lt/u64/ff";
        let key = match fall {
            Some(arm) => self.arm_key(base, arm),
            None => base.to_string(),
        };
        self.emit(
            &key,
            &[
                ("JIT_A", V::I(u64::from(a))),
                ("JIT_B", V::I(u64::from(b))),
                ("JIT_T", tv),
                ("JIT_F", fv),
            ],
        );
    }

}

// ---------------------------------------------------------------------------
// The rest of the surface: an answer that is an enum, a second block, or a sort
// ---------------------------------------------------------------------------
//
// `map`, `filter`, `fold`, `any`, `all` and `count` carry one value through one
// walk, and `middle::lower` builds them as IR loops (`lower/lists.rs`). The six
// below are not that shape and each is not for its own reason: `find` and `foldResult` build
// an `Option` and a `Result` and leave early, `sortBy` is a stable bottom-up
// merge over two blocks, and `zip` and `flatten` read a *second* element layout
// that no runtime entry could be handed (`cli/runtime/list.rs`'s header).
// `deriveArrayEq`, `deriveArrayCompare` and `deriveArrayShow` are here rather
// than in `emit.rs` because they are the same loop over the same code pointer.
//
// Every one of them calls the step through the closure's `code` word — the
// thunk `glue.rs` generates — rather than directly.

/// Where this half's scratch words begin, as an offset **from `Fn2::scratch`**:
/// past the loops above (words 8–12) and past everything the emitter itself
/// claims, which is what [`rtcall::RESERVED_WORDS`](super::rtcall) answers.
///
/// **Derived rather than written.** It was `256` — word 32 — and the emitter's
/// own run ends at word 33, so the first two words of this half were the last
/// two of that one. What wrote them is a reference walk that goes out of line
/// (`emit::walk_field`, at `RAW_WORD + 3`), which is exactly what retaining an
/// element whose type holds an enum does — so `list.sortBy` lost its
/// destination pointer between reading an element and storing it, and answered
/// a block of zeros. Issue #41.
const LOOP_SCRATCH: u32 = super::rtcall::RESERVED_WORDS * 8;

/// Scratch word `k` of this half, still relative to `Fn2::scratch`.
fn t(k: u32) -> u32 {
    LOOP_SCRATCH + k * 8
}

/// Where a whole element is staged on its way between two blocks.
///
/// Past the twenty-four single words above, and the only part of a frame whose
/// size depends on a *type* — so it is the one a frame is measured for rather
/// than given a fixed index. `jit::SCRATCH_WORDS` is what makes the room every
/// frame gets for nothing, and [`BASE_STAGE_ROOM`] is what is left of it;
/// `jit::frame_sigs` adds whatever a function's own widest element needs past
/// that.
pub(crate) const STAGE: u32 = LOOP_SCRATCH + 24 * 8;

/// The staging room a frame has without being measured for one.
///
/// It is what is left after everything with a fixed index has taken its own, so
/// a word added anywhere above narrows it. Asserted rather than remembered: a
/// frame that no longer has the room says so at compile time, where the answer
/// is to raise `jit::SCRATCH_WORDS` beside it.
///
/// **A wider element is not refused for being wider than this.** It used to be,
/// and an `ast.Item` — 448 bytes, so every `[Item]` in `core/buri/ast` — was
/// what the refusal named (buri-lang/buri#48). `jit::frame_sigs` measures each
/// function against the elements it actually stages and buys the difference,
/// which leaves this as the floor rather than the ceiling: the helper bodies
/// `glue.rs` generates are sized from a constant and stage nothing wider than a
/// `Str`, and this is the room they have.
pub(crate) const BASE_STAGE_ROOM: u32 = super::jit::SCRATCH_WORDS as u32 * 8 - STAGE;
const _: () = assert!(BASE_STAGE_ROOM >= 320);

/// One list operation's operands, as frame offsets paired with their IR types.
///
/// The same indirection `LoopOps` is: written once, it serves the call site —
/// where the operands are `ValueId` slots — and the `Body::Runtime` body, where
/// they are the function's own parameters.
pub(crate) struct Operands {
    pub args: Vec<(u32, ir::Type)>,
    pub dest: (u32, ir::Type),
}

/// One step reached through its closure value, with every offset resolved
/// against the caller's frame.
struct Thunked {
    /// The callee frame's base, which is this function's own frame size.
    base: u32,
    /// The thunk's environment slot, inside that frame.
    env: u32,
    /// The answer.
    ret: u32,
    /// The value parameters, in declaration order.
    params: Vec<u32>,
    /// The closure `{ code, env }`, in *this* frame.
    fslot: u32,
    /// What the step answers, as a source type.
    ///
    /// Carried because a **narrow** answer is not a whole word: a step that
    /// answers an `Order` writes one byte into its return slot and leaves the
    /// other seven whatever they were, so a caller that compared the word
    /// against `Greater` compared seven bytes of the last thing in that slot.
    /// That was `sortBy` leaving its input untouched.
    ret_ty: Ty,
}

/// A `[T]` operand: where its two words are, and what its element is.
struct Block {
    /// The descriptor's frame offset: `ptr` at zero, `len` at eight.
    at: u32,
    elem: Ty,
    stride: u32,
    size: u32,
    counted: bool,
}

impl Jit<'_> {
    fn block_at(&mut self, prog: &ir::Program, at: u32, t: ir::Type) -> Option<Block> {
        let elem = self.element_of(prog, t)?;
        let l = self.layouts_of(elem.clone());
        let counted = self.rc_counted(&elem);
        Some(Block { at, elem, stride: l.stride.max(1), size: l.size.max(1), counted })
    }

    /// The frame a thunk expects — `[rets][env: 8][args...]` — with every
    /// offset made absolute.
    fn thunked(
        &mut self,
        prog: &ir::Program,
        st: &Fn2,
        fslot: u32,
        fty: ir::Type,
        want: usize,
    ) -> Option<Thunked> {
        let shape = self.step_shape_ty(prog, fty, want)?;
        let ir::Type::Agg(id) = fty else { return None };
        let Ty::Fn(_, ret_ty) = prog.type_info(id).ty.clone() else { return None };
        let base = st.frame.size;
        Some(Thunked {
            base,
            env: base + shape.env,
            ret: base + shape.ret,
            params: shape.params.iter().map(|p| base + p).collect(),
            fslot,
            ret_ty: *ret_ty,
        })
    }

    /// The call itself: the environment pointer into the thunk's frame, then
    /// `calli` through the closure's code word.
    fn thunk_call(&mut self, c: &Thunked) {
        self.mv(c.env, c.fslot + super::glue::ENV_WORD, 8);
        self.emit(
            "calli",
            &[
                ("JIT_A", V::I(u64::from(c.fslot))),
                ("JIT_N", V::I(u64::from(c.base))),
                ("JIT_P", V::I(u64::from(c.base))),
                ("JIT_CONT0", V::Fall),
            ],
        );
    }

    /// Where one whole element is staged on its way between two blocks, or a
    /// refusal when the element is wider than *this* frame keeps room for.
    ///
    /// The one part of a frame whose size depends on a *type*: everything else
    /// this file uses is a single word at a fixed index. So the room is read
    /// off the frame rather than off a constant — `jit::frame_sigs` sizes each
    /// frame from the elements its own function stages, and
    /// [`BASE_STAGE_ROOM`] is only the floor every frame gets for nothing.
    ///
    /// The refusal stays as the backstop it now is. A frame that was measured
    /// has the room by construction; one that was not — a helper `glue.rs`
    /// sized from a constant — says so here rather than writing an element past
    /// the end of itself and into the frame the callee is about to take.
    fn stage(&mut self, st: &Fn2, size: u32) -> Option<u32> {
        let room = st.frame.size.saturating_sub(st.scratch + STAGE);
        if size > room {
            self.unsupported(format!(
                "a `[T]` whose element is {size} bytes, past the {room} this frame \
                 stages one in"
            ));
            return None;
        }
        Some(st.scratch + STAGE)
    }

    /// A fresh `[T]` block of `n` elements, with the null-for-empty rule, and
    /// its descriptor written at `dest`.
    fn new_block(&mut self, ptr: u32, dest: u32, n: u32, stride: u32) {
        self.emit(
            "elemalloc",
            &[
                ("JIT_D", V::I(u64::from(ptr))),
                ("JIT_A", V::I(u64::from(n))),
                ("JIT_P", V::I(u64::from(stride))),
                ("JIT_CONT0", V::Fall),
            ],
        );
        self.mv(dest, ptr, 8);
        self.mv(dest + 8, n, 8);
    }

    /// One `list.*` or `deriveArray*` key whose answer is not a single carried
    /// value. Answers whether it was emitted.
    pub(crate) fn list_extra(
        &mut self,
        prog: &ir::Program,
        st: &mut Fn2,
        key: &str,
        o: &Operands,
    ) -> bool {
        match key {
            "list.sortBy" => self.list_sort(prog, st, o, 2),
            "list.zip" => self.list_zip(prog, st, o),
            "list.flatten" => self.list_flatten(prog, st, o),
            "deriveArrayEq" => self.derive_array_eq(prog, st, o),
            "deriveArrayCompare" => self.derive_array_compare(prog, st, o),
            "deriveArrayShow" => self.derive_array_show(prog, st, o),
            _ => false,
        }
    }

    /// `zip`: one block of pairs, as long as the shorter of the two.
    ///
    /// `runtime.js`'s `$list_zip` takes the minimum of the two lengths, so
    /// unequal inputs are not an error and the surplus is dropped — which is
    /// what makes the paired indexing below in bounds. Both sources are
    /// borrowed and both copies are a second owner, so each half of every pair
    /// is retained once against the result block's own element glue.
    fn list_zip(&mut self, prog: &ir::Program, st: &mut Fn2, o: &Operands) -> bool {
        let (Some(&(xs, xt)), Some(&(ys, yt))) = (o.args.first(), o.args.get(2)) else {
            return false;
        };
        let (Some(a), Some(b)) =
            (self.block_at(prog, xs, xt), self.block_at(prog, ys, yt))
        else {
            return false;
        };
        let Some(out_elem) = self.element_of(prog, o.dest.1) else { return false };
        let ol = self.layouts_of(out_elem);
        let out_stride = ol.stride.max(1);
        let first_at = ol.fields.first().copied().unwrap_or(0);
        let second_at = ol.fields.get(1).copied().unwrap_or(0);

        let n = st.scratch + t(0);
        let ptr = st.scratch + t(1);
        let i = st.scratch + t(2);
        self.mv(n, xs + 8, 8);
        let shorter = st.label();
        let brkey = self.arm_key("brcmp/lt/u64/ff", "JIT_T");
        self.emit(
            &brkey,
            &[
                ("JIT_A", V::I(u64::from(xs + 8))),
                ("JIT_B", V::I(u64::from(ys + 8))),
                ("JIT_T", V::Blk(shorter)),
                ("JIT_F", V::Fall),
            ],
        );
        self.mv(n, ys + 8, 8);
        st.place(shorter, self.region.code_addr());
        self.new_block(ptr, o.dest.0, n, out_stride);

        self.imm_to(i, 0);
        let head = st.label();
        let done = st.label();
        st.place(head, self.region.code_addr());
        self.br_lt(i, n, V::Fall, V::Blk(done), Some("JIT_T"));
        // The pair is built in scratch and stored whole, which is one indexed
        // store rather than an address computation per half.
        let Some(pair) = self.stage(st, ol.size.max(1)) else { return true };
        for (side, at_field) in [(&a, first_at), (&b, second_at)] {
            self.elem_load(pair + at_field, side.at, i, side.stride, side.size);
            if side.counted {
                let e = side.elem.clone();
                self.retain_value(st, &e, pair + at_field);
            }
        }
        self.elem_store(pair, ptr, i, out_stride, ol.size.max(1));
        self.add_imm(i, i, 1);
        self.emit("jump", &[("JIT_T", V::Blk(head))]);
        st.place(done, self.region.code_addr());
        true
    }

    /// `flatten`: one block holding every element of every inner block.
    ///
    /// Two passes, because the result's length is the sum of the inner lengths
    /// and a `[T]`'s element count is `cap / stride` (`glue.rs::elems_glue`) —
    /// an over-allocated block would have its uninitialised tail released when
    /// it died. The first pass reads `len` out of each descriptor and nothing
    /// else.
    ///
    /// The outer block and every inner one are borrowed, and each element
    /// copied out is a second owner, so one retain per element against the
    /// result block's own glue. The inner *descriptors* are not touched.
    fn list_flatten(&mut self, prog: &ir::Program, st: &mut Fn2, o: &Operands) -> bool {
        let Some(&(xs, xt)) = o.args.first() else { return false };
        let Some(outer) = self.block_at(prog, xs, xt) else { return false };
        let Some(out_elem) = self.element_of(prog, o.dest.1) else { return false };
        let ol = self.layouts_of(out_elem.clone());
        let (out_stride, out_size) = (ol.stride.max(1), ol.size.max(1));
        let out_counted = self.rc_counted(&out_elem);

        let sc = st.scratch;
        let (i, total, ptr, j, k, filled, inner, len) =
            (sc + t(0), sc + t(1), sc + t(2), sc + t(3), sc + t(4), sc + t(5), sc + t(6), sc + t(8));
        self.imm_to(i, 0);
        self.imm_to(total, 0);
        let count = st.label();
        let counted = st.label();
        st.place(count, self.region.code_addr());
        self.br_lt(i, xs + 8, V::Fall, V::Blk(counted), Some("JIT_T"));
        self.elem_load(inner, xs, i, outer.stride, outer.size);
        self.emit(
            "bin/add/u64/ff/f",
            &[
                ("JIT_D", V::I(u64::from(total))),
                ("JIT_A", V::I(u64::from(total))),
                ("JIT_B", V::I(u64::from(inner + 8))),
                ("JIT_CONT", V::Fall),
            ],
        );
        self.add_imm(i, i, 1);
        self.emit("jump", &[("JIT_T", V::Blk(count))]);
        st.place(counted, self.region.code_addr());
        self.new_block(ptr, o.dest.0, total, out_stride);

        self.imm_to(i, 0);
        self.imm_to(k, 0);
        let head = st.label();
        let done = st.label();
        st.place(head, self.region.code_addr());
        self.br_lt(i, xs + 8, V::Fall, V::Blk(done), Some("JIT_T"));
        self.elem_load(inner, xs, i, outer.stride, outer.size);
        self.mv(len, inner + 8, 8);
        self.imm_to(j, 0);
        let ihead = st.label();
        let idone = st.label();
        st.place(ihead, self.region.code_addr());
        self.br_lt(j, len, V::Fall, V::Blk(idone), Some("JIT_T"));
        let Some(staging) = self.stage(st, out_size) else { return true };
        self.elem_load(staging, inner, j, out_stride, out_size);
        if out_counted {
            let e = out_elem.clone();
            self.retain_value(st, &e, staging);
        }
        self.emit(
            "bin/add/u64/ff/f",
            &[
                ("JIT_D", V::I(u64::from(filled))),
                ("JIT_A", V::I(u64::from(k))),
                ("JIT_B", V::I(u64::from(j))),
                ("JIT_CONT", V::Fall),
            ],
        );
        self.elem_store(staging, ptr, filled, out_stride, out_size);
        self.add_imm(j, j, 1);
        self.emit("jump", &[("JIT_T", V::Blk(ihead))]);
        st.place(idone, self.region.code_addr());
        self.emit(
            "bin/add/u64/ff/f",
            &[
                ("JIT_D", V::I(u64::from(k))),
                ("JIT_A", V::I(u64::from(k))),
                ("JIT_B", V::I(u64::from(len))),
                ("JIT_CONT", V::Fall),
            ],
        );
        self.add_imm(i, i, 1);
        self.emit("jump", &[("JIT_T", V::Blk(head))]);
        st.place(done, self.region.code_addr());
        true
    }

    /// `sortBy`: a **stable bottom-up merge**, `llvm/emit.rs::list_sort` pass
    /// for pass.
    ///
    /// Bottom-up rather than recursive because the run width is a loop variable
    /// rather than a call depth, and stable because the merge takes the left
    /// run whenever the comparator does not say `Greater` — which is what makes
    /// `sortBy` a specified operation rather than an implementation detail.
    ///
    /// The source is copied into the result block and every element retained
    /// once; the merge then *moves* bytes between the two blocks, so the
    /// scratch goes back without being walked.
    fn list_sort(&mut self, prog: &ir::Program, st: &mut Fn2, o: &Operands, fi: usize) -> bool {
        let (Some(&(xs, xt)), Some(&(fslot, fty))) = (o.args.first(), o.args.get(fi)) else {
            return false;
        };
        let Some(src) = self.block_at(prog, xs, xt) else { return false };
        let Some(c) = self.thunked(prog, st, fslot, fty, 2) else { return false };
        let (stride, size) = (src.stride, src.size);

        let sc = st.scratch;
        let (n, dst, scratch, w, a, b, span) =
            (sc + t(0), sc + t(1), sc + t(2), sc + t(3), sc + t(4), sc + t(5), sc + t(6));
        let (lo, mid, hi, li, ri, out, i, one) =
            (sc + t(7), sc + t(8), sc + t(9), sc + t(10), sc + t(11), sc + t(12), sc + t(13), sc + t(14));
        self.mv(n, xs + 8, 8);
        self.new_block(dst, o.dest.0, n, stride);
        self.emit(
            "elemalloc",
            &[
                ("JIT_D", V::I(u64::from(scratch))),
                ("JIT_A", V::I(u64::from(n))),
                ("JIT_P", V::I(u64::from(stride))),
                ("JIT_CONT0", V::Fall),
            ],
        );

        // -- the source, copied in and retained once per element ------------
        let Some(staging) = self.stage(st, size) else { return true };
        self.imm_to(i, 0);
        let head = st.label();
        let filled = st.label();
        st.place(head, self.region.code_addr());
        self.br_lt(i, n, V::Fall, V::Blk(filled), Some("JIT_T"));
        self.elem_load(staging, xs, i, stride, size);
        if src.counted {
            let e = src.elem.clone();
            self.retain_value(st, &e, staging);
        }
        self.elem_store(staging, dst, i, stride, size);
        self.add_imm(i, i, 1);
        self.emit("jump", &[("JIT_T", V::Blk(head))]);
        st.place(filled, self.region.code_addr());

        // -- `w = 1, 2, 4, …`, with `a` and `b` swapping each pass ----------
        self.imm_to(w, 1);
        self.mv(a, dst, 8);
        self.mv(b, scratch, 8);
        let wide = st.label();
        let sorted = st.label();
        st.place(wide, self.region.code_addr());
        self.br_lt(w, n, V::Fall, V::Blk(sorted), Some("JIT_T"));
        self.emit(
            "bin/add/u64/ff/f",
            &[
                ("JIT_D", V::I(u64::from(span))),
                ("JIT_A", V::I(u64::from(w))),
                ("JIT_B", V::I(u64::from(w))),
                ("JIT_CONT", V::Fall),
            ],
        );

        // -- one pass: `lo = 0, 2w, 4w, …` ----------------------------------
        self.imm_to(lo, 0);
        let runs = st.label();
        let swap = st.label();
        st.place(runs, self.region.code_addr());
        self.br_lt(lo, n, V::Fall, V::Blk(swap), Some("JIT_T"));
        self.emit(
            "bin/add/u64/ff/f",
            &[
                ("JIT_D", V::I(u64::from(mid))),
                ("JIT_A", V::I(u64::from(lo))),
                ("JIT_B", V::I(u64::from(w))),
                ("JIT_CONT", V::Fall),
            ],
        );
        self.clamp(st, mid, n);
        self.emit(
            "bin/add/u64/ff/f",
            &[
                ("JIT_D", V::I(u64::from(hi))),
                ("JIT_A", V::I(u64::from(lo))),
                ("JIT_B", V::I(u64::from(span))),
                ("JIT_CONT", V::Fall),
            ],
        );
        self.clamp(st, hi, n);

        // -- one merge: `a[lo..mid)` and `a[mid..hi)` into `b[lo..hi)` ------
        self.mv(li, lo, 8);
        self.mv(ri, mid, 8);
        self.mv(out, lo, 8);
        let merge = st.label();
        let merged = st.label();
        let take_left = st.label();
        let take_right = st.label();
        let took = st.label();
        st.place(merge, self.region.code_addr());
        self.br_lt(out, hi, V::Fall, V::Blk(merged), Some("JIT_T"));
        self.br_lt(li, mid, V::Fall, V::Blk(take_right), Some("JIT_T"));
        self.br_lt(ri, hi, V::Fall, V::Blk(take_left), Some("JIT_T"));
        let Some(&p0) = c.params.first() else { return false };
        let Some(&p1) = c.params.get(1) else { return false };
        self.elem_load(p0, a, li, stride, size);
        self.elem_load(p1, a, ri, stride, size);
        if src.counted {
            let e = src.elem.clone();
            self.retain_value(st, &e, p0);
            self.retain_value(st, &e, p1);
        }
        self.thunk_call(&c);
        // `Greater` takes the right element; everything else takes the left,
        // which is what makes the merge stable. The answer is read at the
        // *tag's* width, because an `Order` is one byte in an eight-byte slot.
        let order = self.layouts_of(c.ret_ty.clone());
        let answer = sc + t(15);
        self.load_disc(&order, c.ret, answer);
        let brkey = self.arm_key("brcmp/eq/u64/fi", "JIT_T");
        self.emit(
            &brkey,
            &[
                ("JIT_A", V::I(u64::from(answer))),
                ("JIT_K", V::I(super::rtcall::GREATER)),
                ("JIT_T", V::Blk(take_right)),
                ("JIT_F", V::Fall),
            ],
        );
        st.place(take_left, self.region.code_addr());
        self.elem_load(staging, a, li, stride, size);
        self.add_imm(li, li, 1);
        self.emit("jump", &[("JIT_T", V::Blk(took))]);
        st.place(take_right, self.region.code_addr());
        self.elem_load(staging, a, ri, stride, size);
        self.add_imm(ri, ri, 1);
        st.place(took, self.region.code_addr());
        self.elem_store(staging, b, out, stride, size);
        self.add_imm(out, out, 1);
        self.emit("jump", &[("JIT_T", V::Blk(merge))]);

        st.place(merged, self.region.code_addr());
        self.emit(
            "bin/add/u64/ff/f",
            &[
                ("JIT_D", V::I(u64::from(lo))),
                ("JIT_A", V::I(u64::from(lo))),
                ("JIT_B", V::I(u64::from(span))),
                ("JIT_CONT", V::Fall),
            ],
        );
        self.emit("jump", &[("JIT_T", V::Blk(runs))]);

        st.place(swap, self.region.code_addr());
        self.mv(one, a, 8);
        self.mv(a, b, 8);
        self.mv(b, one, 8);
        self.mv(w, span, 8);
        self.emit("jump", &[("JIT_T", V::Blk(wide))]);

        // -- an odd number of passes ends in the scratch --------------------
        st.place(sorted, self.region.code_addr());
        let home = st.label();
        let brkey = self.arm_key("brcmp/eq/u64/ff", "JIT_T");
        self.emit(
            &brkey,
            &[
                ("JIT_A", V::I(u64::from(a))),
                ("JIT_B", V::I(u64::from(dst))),
                ("JIT_T", V::Blk(home)),
                ("JIT_F", V::Fall),
            ],
        );
        self.imm_to(i, 0);
        let back = st.label();
        st.place(back, self.region.code_addr());
        self.br_lt(i, n, V::Fall, V::Blk(home), Some("JIT_T"));
        self.elem_load(staging, a, i, stride, size);
        self.elem_store(staging, dst, i, stride, size);
        self.add_imm(i, i, 1);
        self.emit("jump", &[("JIT_T", V::Blk(back))]);
        st.place(home, self.region.code_addr());
        // The elements were *moved* into the result, so the scratch goes back
        // without being walked.
        self.emit(
            "decref/free",
            &[("JIT_A", V::I(u64::from(scratch))), ("JIT_CONT0", V::Fall)],
        );
        true
    }

    /// `frame[d] = min(frame[d], frame[n])`, unsigned.
    fn clamp(&mut self, st: &mut Fn2, d: u32, n: u32) {
        let ok = st.label();
        let brkey = self.arm_key("brcmp/lt/u64/ff", "JIT_T");
        self.emit(
            &brkey,
            &[
                ("JIT_A", V::I(u64::from(d))),
                ("JIT_B", V::I(u64::from(n))),
                ("JIT_T", V::Blk(ok)),
                ("JIT_F", V::Fall),
            ],
        );
        self.mv(d, n, 8);
        st.place(ok, self.region.code_addr());
    }

    /// `deriveArrayEq` — a derived `Equal` where the field is a `[T]`.
    ///
    /// `middle/derives.rs`'s header states the shape: `([T], [T], fn(T, T) ->
    /// Bool) -> Bool`, where the third argument is a code pointer to the
    /// element's generated function, "because a loop is not expressible in the
    /// layer-A tree and every backend has the loop already". Two lengths that
    /// differ answer `false` without calling it at all, which is `$eq`'s own
    /// first test and is what makes the paired indexing below in bounds.
    fn derive_array_eq(&mut self, prog: &ir::Program, st: &mut Fn2, o: &Operands) -> bool {
        let (Some(&(xs, xt)), Some(&(ys, _)), Some(&(fslot, fty))) =
            (o.args.first(), o.args.get(1), o.args.get(2))
        else {
            return false;
        };
        let Some(src) = self.block_at(prog, xs, xt) else { return false };
        let Some(c) = self.thunked(prog, st, fslot, fty, 2) else { return false };
        let d = o.dest.0;
        let i = st.scratch + t(0);
        let end = st.label();
        let head = st.label();
        let done = st.label();
        self.imm_to(d, 0);
        let same = st.label();
        let brkey = self.arm_key("brcmp/eq/u64/ff", "JIT_T");
        self.emit(
            &brkey,
            &[
                ("JIT_A", V::I(u64::from(xs + 8))),
                ("JIT_B", V::I(u64::from(ys + 8))),
                ("JIT_T", V::Blk(same)),
                ("JIT_F", V::Fall),
            ],
        );
        self.emit("jump", &[("JIT_T", V::Blk(end))]);
        st.place(same, self.region.code_addr());
        self.imm_to(d, 1);
        self.imm_to(i, 0);
        st.place(head, self.region.code_addr());
        self.br_lt(i, xs + 8, V::Fall, V::Blk(done), Some("JIT_T"));
        let Some(&p0) = c.params.first() else { return false };
        let Some(&p1) = c.params.get(1) else { return false };
        self.elem_load(p0, xs, i, src.stride, src.size);
        self.elem_load(p1, ys, i, src.stride, src.size);
        if src.counted {
            let e = src.elem.clone();
            self.retain_value(st, &e, p0);
            self.retain_value(st, &e, p1);
        }
        self.thunk_call(&c);
        let cont = st.label();
        let brkey = self.arm_key("br/f", "JIT_T");
        self.emit(
            &brkey,
            &[
                ("JIT_A", V::I(u64::from(c.ret))),
                ("JIT_T", V::Blk(cont)),
                ("JIT_F", V::Fall),
            ],
        );
        self.imm_to(d, 0);
        self.emit("jump", &[("JIT_T", V::Blk(end))]);
        st.place(cont, self.region.code_addr());
        self.add_imm(i, i, 1);
        self.emit("jump", &[("JIT_T", V::Blk(head))]);
        st.place(done, self.region.code_addr());
        st.place(end, self.region.code_addr());
        true
    }

    /// `deriveArrayCompare` — a derived `Ordered` where the field is a `[T]`.
    ///
    /// `middle/derives.rs`'s header states the shape: `([T], [T], fn(T, T) ->
    /// Order) -> Order`, the same code pointer [`Jit::derive_array_eq`] takes
    /// with a different carried answer.
    ///
    /// The order is lexicographic and it is `$cmp`'s array arm step for step
    /// (`backend/js/runtime.js`): the first `min(m, n)` elements decide it, and
    /// where every one of them is `Equal` the **lengths** do — so `[1]` is
    /// below `[1, 2]` and a prefix is below what extends it. That is the half
    /// `deriveArrayEq` has no equivalent of, and it is why the loop bound here
    /// is the shorter length rather than a refusal on unequal ones.
    ///
    /// The answer is read at the *tag's* width through [`Jit::load_disc`],
    /// because an `Order` is one byte in an eight-byte slot and the other seven
    /// are whatever the callee's return area last held — `list_sort` compares
    /// the same value the same way and for the same reason.
    fn derive_array_compare(&mut self, prog: &ir::Program, st: &mut Fn2, o: &Operands) -> bool {
        let (Some(&(xs, xt)), Some(&(ys, _)), Some(&(fslot, fty))) =
            (o.args.first(), o.args.get(1), o.args.get(2))
        else {
            return false;
        };
        let Some(src) = self.block_at(prog, xs, xt) else { return false };
        let Some(c) = self.thunked(prog, st, fslot, fty, 2) else { return false };
        let Some(order_ty) = super::rtcall::source_ty(prog, o.dest.1) else { return false };
        let order = self.layouts_of(order_ty);
        let d = o.dest.0;
        let sc = st.scratch;
        let (n, i, answer) = (sc + t(0), sc + t(1), sc + t(2));

        // The shorter of the two lengths, which is what makes the paired
        // indexing below in bounds.
        self.mv(n, xs + 8, 8);
        self.clamp(st, n, ys + 8);
        self.imm_to(i, 0);
        let head = st.label();
        let lengths = st.label();
        let next = st.label();
        let less = st.label();
        let greater = st.label();
        let end = st.label();
        st.place(head, self.region.code_addr());
        self.br_lt(i, n, V::Fall, V::Blk(lengths), Some("JIT_T"));
        let Some(&p0) = c.params.first() else { return false };
        let Some(&p1) = c.params.get(1) else { return false };
        self.elem_load(p0, xs, i, src.stride, src.size);
        self.elem_load(p1, ys, i, src.stride, src.size);
        if src.counted {
            let e = src.elem.clone();
            self.retain_value(st, &e, p0);
            self.retain_value(st, &e, p1);
        }
        self.thunk_call(&c);
        self.load_disc(&order, c.ret, answer);
        let brkey = self.arm_key("brcmp/eq/u64/fi", "JIT_T");
        self.emit(
            &brkey,
            &[
                ("JIT_A", V::I(u64::from(answer))),
                ("JIT_K", V::I(super::rtcall::EQUAL)),
                ("JIT_T", V::Blk(next)),
                ("JIT_F", V::Fall),
            ],
        );
        // Not `Equal`, so this element is the answer. The variant is stored
        // rather than the callee's return slot copied, because only the tag
        // byte of that slot is defined.
        let brkey = self.arm_key("brcmp/eq/u64/fi", "JIT_T");
        self.emit(
            &brkey,
            &[
                ("JIT_A", V::I(u64::from(answer))),
                ("JIT_K", V::I(super::rtcall::LESS)),
                ("JIT_T", V::Blk(less)),
                ("JIT_F", V::Fall),
            ],
        );
        self.emit("jump", &[("JIT_T", V::Blk(greater))]);
        st.place(next, self.region.code_addr());
        self.add_imm(i, i, 1);
        self.emit("jump", &[("JIT_T", V::Blk(head))]);

        // Every shared element compared `Equal`, so the lengths decide.
        st.place(lengths, self.region.code_addr());
        self.br_lt(xs + 8, ys + 8, V::Blk(less), V::Fall, Some("JIT_F"));
        self.br_lt(ys + 8, xs + 8, V::Blk(greater), V::Fall, Some("JIT_F"));
        self.store_disc(&order, d, super::rtcall::EQUAL as usize);
        self.emit("jump", &[("JIT_T", V::Blk(end))]);
        st.place(less, self.region.code_addr());
        self.store_disc(&order, d, super::rtcall::LESS as usize);
        self.emit("jump", &[("JIT_T", V::Blk(end))]);
        st.place(greater, self.region.code_addr());
        self.store_disc(&order, d, super::rtcall::GREATER as usize);
        st.place(end, self.region.code_addr());
        true
    }

    /// `deriveArrayShow` — a derived `Show` where the field is a `[T]`.
    ///
    /// `middle/derives.rs`'s header states the shape: `([T], fn(T) -> Str) ->
    /// Str`, rendering `[a, b]` with the separator included. So the answer is
    /// two halves: call the element's generated function once per element,
    /// which only a backend can do, and join the results with brackets and
    /// `", "`, which only the archive should do — `buri_rt_show_list` is that
    /// half.
    ///
    /// Each rendered `Str` arrives **owned** — the element's `show` is a
    /// function value and its answer is a fresh count — and the join *copies*
    /// bytes, so every one is released before the scratch block goes back.
    /// Without that loop a derived `show` of a `[Str]` would leak one block per
    /// element. The scratch block itself is freed without being walked, because
    /// the walk is what the release loop just did.
    fn derive_array_show(&mut self, prog: &ir::Program, st: &mut Fn2, o: &Operands) -> bool {
        let (Some(&(xs, xt)), Some(&(fslot, fty))) = (o.args.first(), o.args.get(1)) else {
            return false;
        };
        let Some(src) = self.block_at(prog, xs, xt) else { return false };
        let Some(c) = self.thunked(prog, st, fslot, fty, 1) else { return false };
        let Some(str_ty) = super::rtcall::source_ty(prog, o.dest.1) else { return false };
        let sl = self.layouts_of(str_ty.clone());
        let (out_stride, out_size) = (sl.stride.max(1), sl.size.max(1));

        let sc = st.scratch;
        let (n, ptr, i) = (sc + t(0), sc + t(1), sc + t(2));
        self.mv(n, xs + 8, 8);
        self.emit(
            "elemalloc",
            &[
                ("JIT_D", V::I(u64::from(ptr))),
                ("JIT_A", V::I(u64::from(n))),
                ("JIT_P", V::I(u64::from(out_stride))),
                ("JIT_CONT0", V::Fall),
            ],
        );
        self.imm_to(i, 0);
        let head = st.label();
        let done = st.label();
        st.place(head, self.region.code_addr());
        self.br_lt(i, n, V::Fall, V::Blk(done), Some("JIT_T"));
        let Some(&p0) = c.params.first() else { return false };
        self.elem_load(p0, xs, i, src.stride, src.size);
        if src.counted {
            let e = src.elem.clone();
            self.retain_value(st, &e, p0);
        }
        self.thunk_call(&c);
        self.elem_store(c.ret, ptr, i, out_stride, out_size);
        self.add_imm(i, i, 1);
        self.emit("jump", &[("JIT_T", V::Blk(head))]);
        st.place(done, self.region.code_addr());

        let args = [
            super::rtcall::Src::Word(ptr),
            super::rtcall::Src::Word(n),
            super::rtcall::Src::Addr(o.dest.0),
        ];
        if let Err(why) = self.c_call(runtime::SHOW_LIST, st, &args, &[], o.dest.0, "v") {
            self.unsupported(why);
        }

        // -- the rendered strings, released ---------------------------------
        let Some(staging) = self.stage(st, out_size) else { return true };
        self.imm_to(i, 0);
        let freeing = st.label();
        let freed = st.label();
        st.place(freeing, self.region.code_addr());
        self.br_lt(i, n, V::Fall, V::Blk(freed), Some("JIT_T"));
        self.elem_load(staging, ptr, i, out_stride, out_size);
        if let Err(why) = self.walk_rc(st, &str_ty, staging, Op::Release, 0) {
            self.unsupported(why);
        }
        self.add_imm(i, i, 1);
        self.emit("jump", &[("JIT_T", V::Blk(freeing))]);
        st.place(freed, self.region.code_addr());
        self.emit(
            "decref/free",
            &[("JIT_A", V::I(u64::from(ptr))), ("JIT_CONT0", V::Fall)],
        );
        true
    }
}

/// Byte offset of variant `v`'s first payload field.
///
/// A niche has no payload *area*: `.Some`'s payload is the whole value
/// (`middle/layout.rs`'s `build_enum`), so the offset is zero.
pub(crate) fn payload_at(l: &Layout, v: usize) -> u32 {
    match &l.repr {
        Repr::Enum { repr: EnumRepr::Niche { .. }, .. } => 0,
        Repr::Enum { variants, .. } => {
            variants.get(v).and_then(|f| f.first().copied()).unwrap_or(0)
        }
        _ => 0,
    }
}

impl Jit<'_> {
    /// Store variant `v`'s discriminant at `at`, leaving any payload alone.
    ///
    /// A niche's *empty* variant is a null pointer and its other variant is the
    /// payload itself, so only one of the two writes anything.
    pub(crate) fn store_disc(&mut self, l: &Layout, at: u32, v: usize) {
        match &l.repr {
            Repr::Enum { repr: EnumRepr::Bare { tag } | EnumRepr::Tagged { tag, .. }, .. } => {
                let w = tag.size();
                self.imm_w(at, w, v as u64);
            }
            Repr::Enum { repr: EnumRepr::Niche { null_at }, variants }
                if variants.get(v).is_some_and(|f| f.is_empty()) =>
            {
                self.imm_to(at + null_at, 0);
            }
            _ => {}
        }
    }

    /// The discriminant of an enum at `at`, into `dst` as a whole word.
    fn load_disc(&mut self, l: &Layout, at: u32, dst: u32) {
        match &l.repr {
            Repr::Enum { repr: EnumRepr::Bare { tag } | EnumRepr::Tagged { tag, .. }, .. } => {
                let w = tag.size();
                self.load_w(dst, at, w);
            }
            Repr::Enum { repr: EnumRepr::Niche { null_at }, variants } => {
                let null_variant = variants.iter().position(|v| v.is_empty()).unwrap_or(1) as u64;
                let other = u64::from(null_variant == 0);
                self.emit(
                    "niche_tag",
                    &[
                        ("JIT_D", V::I(u64::from(dst))),
                        ("JIT_A", V::I(u64::from(at + null_at))),
                        ("JIT_N", V::I(null_variant)),
                        ("JIT_P", V::I(other)),
                        ("JIT_CONT", V::Fall),
                    ],
                );
            }
            _ => self.imm_to(dst, 0),
        }
    }
}
