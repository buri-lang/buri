//! `core/list`'s closure surface, open-coded as stencils.
//!
//! # Why this is not `intrin.rs`
//!
//! `cli/runtime/list.rs`'s header says why neither native backend has a
//! `buri_rt_list_map`: a Buri closure is `{ code, env }` where `code`'s
//! signature is the *flattened* one of the element type, so a C function
//! calling one would have to synthesize a parameter list that depends on `T`.
//! A backend already knows how, so the loop lives in the backend.
//!
//! `intrin.rs` took the other road — one descriptor-driven helper, and
//! `call_closure` builds the callee's frame with two `memcpy`s and an indirect
//! call **per element**. Three reports running named that boundary as the whole
//! of the K4 gap: the L6→L12 ladder moves K4 by 6% while moving K1 by 2.2×,
//! because K4 never reaches the code generator at all. This file is the fix,
//! and it is the same shape `cranelift/emit.rs::list_closure` has:
//!
//!   * the loop is emitted **at the call site**, out of ordinary stencils, so
//!     the `Body::Runtime` call disappears entirely;
//!   * the element is read with one indexed-copy stencil (`eload/{n}`) rather
//!     than an address computation and a `memcpy` call;
//!   * and when the step is a `MakeClosure` this function can see — which is
//!     every lambda written at the call site — the call is a **direct** `call`
//!     stencil, not a `calli`. That is `cranelift/emit.rs::direct_callee`, and
//!     it is what lets the callee's frame offsets be read out of `Jit::plan`
//!     instead of guessed from a source type.
//!
//! # The counts
//!
//! Two sentences of `middle/rc.rs`, quoted by `cranelift/emit.rs::list_closure`,
//! decide every reference operation here and they point in opposite directions:
//!
//!  * *"A runtime intrinsic borrows its arguments and returns a fresh count."*
//!    The source list, the step and `fold`'s initial accumulator arrive
//!    borrowed; nothing here releases one.
//!  * *"A call through a function value owns its arguments."* So every value
//!    handed to a step **through a function value** is retained first, the step
//!    consumes that count, and it answers a fresh one.
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
//! `cranelift/emit.rs::list_filter` does: once because the predicate is handed
//! a count it consumes, and once because the copy into the result block is a
//! second owner of what the element holds.
//!
//! # What "counted" means here
//!
//! `cranelift` asks `middle::rc`'s oracle (`Cx::rc_counted`) rather than the
//! layout table's, because retaining what rc does not count adds one half of a
//! pair nothing completes. This prototype has only ever counted the top-level
//! pointer of a `Str` or a `[T]` (`emit.rs::rc`, and `stats.rc_skipped` says
//! how often it declined), so `counted` below is that same predicate and not a
//! wider one: the two oracles inside *this* compiler are the same one.

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
use crate::compiler::middle::ir;
use crate::compiler::middle::layout::Repr;
use crate::compiler::semantics::types::Ty;

/// Which loop. `cranelift/emit.rs::Step`, minus the three whose answer is an
/// enum this file does not build yet (§"what is not here").
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Step {
    Map,
    Filter,
    Fold,
    Any,
    All,
    Count,
}

/// One `core/list` key, with the argument positions the declaration fixes.
/// Receiver first, context second, everything else after (SPEC 10.7) — the
/// same table as `cranelift/emit.rs::list_call`, entry for entry.
pub struct ListCall {
    pub kind: Step,
    /// The context, where the *step* takes one. `map` and `mapCtx` both have a
    /// context argument — `Alloc`, for the block they build — and only the
    /// second passes it on.
    pub ctx: Option<usize>,
    pub func: usize,
    /// `fold`'s initial accumulator.
    pub init: Option<usize>,
}

pub fn list_call(key: &str) -> Option<ListCall> {
    let call = |kind, ctx, func, init| Some(ListCall { kind, ctx, func, init });
    match key {
        "list.map" => call(Step::Map, None, 2, None),
        "list.mapCtx" => call(Step::Map, Some(1), 2, None),
        "list.filter" => call(Step::Filter, None, 2, None),
        "list.filterCtx" => call(Step::Filter, Some(1), 2, None),
        "list.fold" => call(Step::Fold, None, 1, Some(2)),
        "list.foldCtx" => call(Step::Fold, Some(1), 2, Some(3)),
        "list.any" => call(Step::Any, None, 1, None),
        "list.all" => call(Step::All, None, 1, None),
        "list.count" => call(Step::Count, None, 1, None),
        _ => None,
    }
}

/// The scratch words this file uses, as offsets from `Fn2::scratch`.
///
/// `SCRATCH_WORDS` is 16 and everything else in the emitter uses words 0–5
/// (`st.scratch`, `+16`, `+24`, `+32`, `+40`), so words 8–15 are free and are
/// taken here. Nothing in a loop body reaches them: a step runs in its own
/// frame, which begins at `frame.size`, past every one of these.
const S_I: u32 = 64;
const S_LEN: u32 = 72;
const S_SRC: u32 = 80;
const S_DST: u32 = 88;
const S_K: u32 = 96;

/// Where a step's frame puts its result, its environment and its arguments —
/// read out of `Jit::plan`'s own table for the function the `MakeClosure`
/// names, so it is the callee's real layout rather than a reconstruction.
struct StepShape {
    /// The `FuncIdx` of the lifted lambda.
    func: u32,
    /// Byte offset of the step's single result inside its frame.
    ret: u32,
    /// Byte offset of the environment pointer: `middle::closures` puts it
    /// first, so it is parameter 0.
    env: u32,
    /// Byte offsets of the *value* parameters, after the environment.
    params: Vec<u32>,
    /// Whether the call may be a direct `call`. False when any parameter is a
    /// borrowed counted one — see the module header.
    direct: bool,
    /// The environment's real width. Zero for a lambda that captures nothing.
    env_w: u32,
}

/// The ablation switches this file answers to, so that each of its three
/// axes can be measured on its own the way every other axis in this project
/// was: `CPJIT_OFF=listloop` puts every `list.*` call back through
/// `intrin.rs`'s descriptor helper, `estride` gives up the stride-baked
/// load/store twins, `incbr` gives up the fused back edge, and `envskip` writes
/// the environment even when it weighs nothing.
fn off(name: &str) -> bool {
    std::env::var("CPJIT_OFF").unwrap_or_default().split(',').any(|x| x == name)
}


/// One resolved list operation: every operand as a **frame offset**, so that
/// the loop is written once and serves both the call site — where the operands
/// are `ValueId` slots — and the `Body::Runtime` body, where they are the
/// function's own parameters.
struct LoopOps {
    /// The `[T]`'s two words.
    xs: u32,
    /// The closure's `{ code, env }`.
    fslot: u32,
    /// Where the answer goes.
    dest: u32,
    /// The context the step is threaded: offset, slot width, counted.
    ctx: Option<(u32, u32, bool)>,
    /// `fold`'s initial accumulator.
    init: Option<u32>,
    acc_w: u32,
    acc_counted: bool,
    si: u32,
    elem_w: u32,
    elem_counted: bool,
    out_stride: u32,
    out_w: u32,
}

impl<'a> Jit<'a> {
    /// Is this type one `emit.rs::rc` would count?
    fn counted(&mut self, prog: &ir::Program, t: ir::Type) -> bool {
        let ir::Type::Agg(id) = t else { return false };
        matches!(self.layout_of(prog, id).repr, Repr::Str | Repr::List)
    }

    /// The stride of `[T]`'s element for a value of list type.
    pub(crate) fn elem_stride(&mut self, prog: &ir::Program, t: ir::Type) -> Option<u32> {
        let ir::Type::Agg(id) = t else { return None };
        let Ty::Array(elem) = prog.type_info(id).ty.clone() else { return None };
        Some(self.layouts_of(*elem).stride.max(1))
    }

    /// A `[T]`'s stride, element width and whether the element is counted.
    pub(crate) fn array_elem(&mut self, prog: &ir::Program, t: ir::Type) -> Option<(u32, u32, bool)> {
        let ir::Type::Agg(id) = t else { return None };
        let Ty::Array(elem) = prog.type_info(id).ty.clone() else { return None };
        let l = self.layouts_of(*elem);
        Some((l.stride.max(1), l.size.max(1), matches!(l.repr, Repr::Str | Repr::List)))
    }

    /// The step, resolved. `None` when the closure was not built by a
    /// `MakeClosure` this function can see — a step passed in from outside —
    /// in which case the caller falls back to the descriptor helper.
    fn step_shape(
        &mut self,
        prog: &ir::Program,
        st: &Fn2,
        f: ir::ValueId,
        want: usize,
    ) -> Option<StepShape> {
        let func = *st.closure_of.get(f.index())?.as_ref()?;
        let g = prog.funcs.get(func as usize)?;
        let fs = self.frame_sig_of(func as usize);
        // `[env] ++ values`: `middle::closures` puts the environment first,
        // whatever it weighs — `null` for a lambda that captures nothing, whose
        // parameter is then the unit type rather than a pointer. Requiring the
        // *count* to be one more than the step's value arguments is what makes
        // that assumption checkable rather than assumed: a signature this
        // function has misread cannot pass it.
        if fs.params.len() != want + 1 {
            return None;
        }
        let env = *fs.params.first()?;
        let params: Vec<u32> = fs.params.iter().skip(1).copied().collect();
        let ret = fs.ret.first().copied().unwrap_or(0);
        if fs.ret.len() > 1 {
            return None;
        }
        // `cranelift/emit.rs::direct_callee`: a borrowed counted parameter is
        // where the two calling conventions disagree, so the direct call is
        // refused there and the function-value one is taken instead.
        let mut direct = g.facts.params.len() == g.sig.params.len();
        for (t, own) in g.sig.params.iter().zip(g.facts.params.iter()) {
            if *own == ir::Ownership::Borrow && self.counted(prog, *t) {
                direct = false;
            }
        }
        let env_w = self.width_of(prog, *g.sig.params.first()?);
        Some(StepShape { func, ret, env, params, direct, env_w })
    }

    /// The step's frame, reconstructed from the closure's *type*.
    ///
    /// Used only where the step is not a `MakeClosure` this function can see,
    /// so `Jit::plan`'s table cannot be consulted: inside the `Body::Runtime`
    /// function, whose closure is a parameter. `middle::closures` lifts a
    /// lambda to `fn(env, args...)` and the environment is one word by value
    /// in this prototype (`emit.rs::rc` says why), so the callee's frame is
    /// `[ret][env: 8][args...]` — the same arithmetic `Jit::call_indirect`
    /// already does at every indirect call site, and the same one
    /// `Jit::frame_sig` would produce for the lifted function.
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
        Some(StepShape { func: 0, ret: 0, env, params, direct: false, env_w: 8 })
    }

    /// `frame[dst] = *(base + i * stride)`, `bytes` wide.
    pub(crate) fn elem_load(&mut self, dst: u32, base: u32, i: u32, stride: u32, bytes: u32) {
        // An element narrower than a frame word has to arrive **zero-extended**
        // into a whole one; see `gen.rs`'s `eloadz` family for why.
        if bytes < 8 {
            let zk = if stride == bytes && !off("estride") {
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
        let key = if stride == bytes && !off("estride") && self.has(&sk) {
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
    fn elem_store(&mut self, src: u32, base: u32, i: u32, stride: u32, bytes: u32) {
        let sk = format!("estore/{bytes}/s");
        let key = if stride == bytes && !off("estride") && self.has(&sk) {
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

    /// `frame[d] = frame[a] + k`, through the immediate form where the level
    /// has one and a materialised constant where it does not.
    fn add_imm(&mut self, d: u32, a: u32, k: u64, scratch: u32) {
        if self.has("bin/add/u64/fi/f") {
            self.emit(
                "bin/add/u64/fi/f",
                &[
                    ("JIT_D", V::I(u64::from(d))),
                    ("JIT_A", V::I(u64::from(a))),
                    ("JIT_K", V::I(k)),
                    ("JIT_CONT", V::Fall),
                ],
            );
            return;
        }
        self.imm_to(scratch, k);
        self.emit(
            "bin/add/u64/ff/f",
            &[
                ("JIT_D", V::I(u64::from(d))),
                ("JIT_A", V::I(u64::from(a))),
                ("JIT_B", V::I(u64::from(scratch))),
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
        if self.has(base) {
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
            return;
        }
        let s = a; // never taken: every level from L3 up has the fused form.
        let _ = s;
        self.emit(
            "bin/lt/u64/ff/f",
            &[
                ("JIT_D", V::I(u64::from(S_K))),
                ("JIT_A", V::I(u64::from(a))),
                ("JIT_B", V::I(u64::from(b))),
                ("JIT_CONT", V::Fall),
            ],
        );
        self.emit(
            "br/f",
            &[("JIT_A", V::I(u64::from(S_K))), ("JIT_T", tv), ("JIT_F", fv)],
        );
    }

    /// The retain a value crossing a function value needs, when the element
    /// type is one this compiler counts. `p` is the pointer's offset inside
    /// the element, which is 0 for both `Str` and `[T]`.
    fn retain_at(&mut self, off: u32) {
        if std::env::var("CPJIT_NOFREE").is_ok_and(|v| v == "1") {
            return;
        }
        self.emit("incref", &[("JIT_A", V::I(u64::from(off))), ("JIT_CONT", V::Fall)]);
    }
    /// A `list.*` call written in a function body. The step is a `MakeClosure`
    /// this function can see, so the call is direct and the callee's frame
    /// offsets come out of `Jit::plan`.
    ///
    /// Answers whether the call was open-coded; `false` leaves the caller to
    /// emit an ordinary call to the `Body::Runtime` function — whose body is
    /// now [`Jit::list_loop_rt`]'s loop, and `intrin.rs`'s descriptor helper
    /// only where that declines too.
    pub(crate) fn list_loop(
        &mut self,
        prog: &ir::Program,
        code: &ir::Code,
        st: &mut Fn2,
        dests: &[ir::ValueId],
        key: &str,
        args: &[ir::ValueId],
    ) -> bool {
        if off("listloop") {
            return false;
        }
        let Some(call) = list_call(key) else { return false };
        let (Some(xs), Some(f)) = (args.first().copied(), args.get(call.func).copied()) else {
            return false;
        };
        let Some(dest) = dests.first().copied() else { return false };
        let want = usize::from(call.ctx.is_some()) + usize::from(call.kind == Step::Fold) + 1;
        let Some(shape) = self.step_shape(prog, st, f, want) else { return false };
        let ctx = call.ctx.and_then(|i| args.get(i).copied()).map(|c| {
            let w = self.slot_bytes_of(prog, code.ty_of(c));
            (st.at(c), w, self.counted(prog, code.ty_of(c)))
        });
        if call.ctx.is_some() && ctx.is_none() {
            return false;
        }
        let init = match call.init {
            Some(i) => match args.get(i).copied() {
                Some(v) => Some(st.at(v)),
                None => return false,
            },
            None => None,
        };
        let Some(ops) = self.loop_ops(
            prog,
            call.kind,
            code.ty_of(xs),
            code.ty_of(dest),
            st.at(xs),
            st.at(f),
            st.at(dest),
            ctx,
            init,
        ) else {
            return false;
        };
        self.emit_list_loop(st, call.kind, &ops, &shape)
    }

    /// The same loop as the body of the `Body::Runtime` function itself, for
    /// the call sites [`Jit::list_loop`] declined — a step that arrives from
    /// somewhere else, so the call is through the closure's code pointer and
    /// the callee's frame has to be read off the closure's *type* rather than
    /// out of `Jit::plan`.
    ///
    /// Worth having for two reasons beyond the ones that decline. First,
    /// `reachable_dirty` walks the **IR's** call graph, so a `Body::Runtime`
    /// function left `unsupported` marks every test that mentions it
    /// unrunnable even where the call site was open-coded and the body is dead.
    /// Second, it is the whole of `foldCtx`, `filterCtx`, `any`, `all` and
    /// `count`, for which `intrin.rs` never had a helper at all.
    pub(crate) fn list_loop_rt(
        &mut self,
        prog: &ir::Program,
        fi: usize,
        key: &str,
        st: &mut Fn2,
    ) -> bool {
        if off("listloop") {
            return false;
        }
        let Some(call) = list_call(key) else { return false };
        let Some(f) = prog.funcs.get(fi) else { return false };
        let sig_params = f.sig.params.clone();
        let Some(&sig_ret) = f.sig.rets.first() else { return false };
        // The receiver, which SPEC 10.7 fixes as parameter 0 of every
        // `core/list` declaration this file open-codes.
        let Some(&xs_ty) = sig_params.first() else { return false };
        let fs = st.frame.clone();
        let (Some(&xs_off), Some(&f_off)) =
            (fs.params.first(), fs.params.get(call.func)) else { return false };
        let Some(&dest_off) = fs.ret.first() else { return false };
        let want = usize::from(call.ctx.is_some()) + usize::from(call.kind == Step::Fold) + 1;
        let Some(fty) = sig_params.get(call.func).copied() else { return false };
        let Some(shape) = self.step_shape_ty(prog, fty, want) else { return false };
        let ctx = match call.ctx {
            Some(i) => match (fs.params.get(i), sig_params.get(i)) {
                (Some(&o), Some(&t)) => {
                    let w = self.slot_bytes_of(prog, t);
                    Some((o, w, self.counted(prog, t)))
                }
                _ => return false,
            },
            None => None,
        };
        let init = match call.init {
            Some(i) => match fs.params.get(i) {
                Some(&o) => Some(o),
                None => return false,
            },
            None => None,
        };
        let Some(ops) = self.loop_ops(
            prog,
            call.kind,
            xs_ty,
            sig_ret,
            xs_off,
            f_off,
            dest_off,
            ctx,
            init,
        ) else {
            return false;
        };
        self.emit_list_loop(st, call.kind, &ops, &shape)
    }

    /// Everything the loop needs about the *types* involved, resolved once.
    #[allow(clippy::too_many_arguments)]
    fn loop_ops(
        &mut self,
        prog: &ir::Program,
        kind: Step,
        xs_ty: ir::Type,
        dest_ty: ir::Type,
        xs: u32,
        fslot: u32,
        dest: u32,
        ctx: Option<(u32, u32, bool)>,
        init: Option<u32>,
    ) -> Option<LoopOps> {
        let si = self.elem_stride(prog, xs_ty)?;
        // The element's *width* is what may be copied out of the block; the
        // stride is what separates two of them. They differ when an element's
        // alignment pads it, and copying the padding of the last element would
        // read past the allocation.
        let ir::Type::Agg(xid) = xs_ty else { return None };
        let Ty::Array(elem_ty) = prog.type_info(xid).ty.clone() else { return None };
        let elem_l = self.layouts_of(*elem_ty);
        let elem_w = elem_l.size.max(1);
        let elem_counted = matches!(elem_l.repr, Repr::Str | Repr::List);
        let (out_stride, out_w) = match kind {
            Step::Map => {
                let ir::Type::Agg(did) = dest_ty else { return None };
                let Ty::Array(oe) = prog.type_info(did).ty.clone() else { return None };
                (self.elem_stride(prog, dest_ty)?, self.layouts_of(*oe).size.max(1))
            }
            Step::Filter => (si, elem_w),
            _ => (0, 0),
        };
        if kind == Step::Fold && init.is_none() {
            return None;
        }
        Some(LoopOps {
            xs,
            fslot,
            dest,
            ctx,
            init,
            acc_w: self.slot_bytes_of(prog, dest_ty),
            acc_counted: self.counted(prog, dest_ty),
            si,
            elem_w,
            elem_counted,
            out_stride,
            out_w,
        })
    }

    /// The loop itself, in stencils. Every operand is a frame offset by now,
    /// so this is the one copy of `cranelift/emit.rs::list_map`,
    /// `list_filter`, `list_fold` and `list_test` that this backend has.
    fn emit_list_loop(
        &mut self,
        st: &mut Fn2,
        kind: Step,
        ops: &LoopOps,
        shape: &StepShape,
    ) -> bool {
        // Where the step's arguments go: the context first when the step takes
        // one, then the accumulator for a fold, then the element.
        let mut pi = 0usize;
        let mut ctx_off = None;
        if ops.ctx.is_some() {
            let Some(o) = shape.params.get(pi).copied() else { return false };
            ctx_off = Some(o);
            pi += 1;
        }
        let mut acc_off = None;
        if kind == Step::Fold {
            let Some(o) = shape.params.get(pi).copied() else { return false };
            acc_off = Some(o);
            pi += 1;
        }
        let Some(elem_off) = shape.params.get(pi).copied() else { return false };

        let scr = st.scratch;
        let (s_i, s_len, s_src, s_dst, s_k) =
            (scr + S_I, scr + S_LEN, scr + S_SRC, scr + S_DST, scr + S_K);
        let base = st.frame.size;
        let dslot = ops.dest;

        // -- the preamble ---------------------------------------------------
        self.mv(s_src, ops.xs, 8);
        self.mv(s_len, ops.xs + 8, 8);

        // The result block, where the step's answers go. `map` builds one of
        // its own element type; `filter` builds one of the source's, sized at
        // the source's length and used to its kept prefix — which is what
        // `intrin.rs::cpjit_list_filter` has always done here. The second,
        // exact block `cranelift/emit.rs::list_filter` copies into exists
        // because the *real* runtime releases a block by `cap / stride`, and
        // this prototype's `decref/free` never walks elements.
        if matches!(kind, Step::Map | Step::Filter) {
            self.emit(
                "elemalloc",
                &[
                    ("JIT_D", V::I(u64::from(s_dst))),
                    ("JIT_A", V::I(u64::from(s_len))),
                    ("JIT_P", V::I(u64::from(ops.out_stride))),
                    ("JIT_CONT0", V::Fall),
                ],
            );
        }

        // The carried value's initial state.
        match kind {
            Step::Map => {
                self.mv(dslot, s_dst, 8);
                self.mv(dslot + 8, s_len, 8);
            }
            Step::Filter => self.imm_to(s_k, 0),
            Step::Fold => {
                let init = ops.init.unwrap_or(dslot);
                self.mv(dslot, init, ops.acc_w);
                // "A runtime intrinsic borrows its arguments": the initial
                // accumulator arrives borrowed, and the first step consumes a
                // count, so one is taken here. Each step answers another, and
                // the last one is the result's — which is what makes a fold
                // balance (`cranelift/emit.rs::list_fold`).
                if ops.acc_counted {
                    self.retain_at(dslot);
                }
            }
            Step::Any | Step::Count => self.imm_to(dslot, 0),
            Step::All => self.imm_to(dslot, 1),
        }
        self.imm_to(s_i, 0);

        // -- the loop -------------------------------------------------------
        let l_body = st.label();
        let l_done = st.label();
        // The guard: an empty list runs no step at all. The back edge below is
        // the same test, so the body is the fallthrough on both.
        self.br_lt(s_i, s_len, V::Fall, V::Blk(l_done), Some("JIT_T"));
        st.place(l_body, self.region.code_addr());

        // The environment. One word, by value, at the step's parameter 0 —
        // and *nothing at all* when the lambda captures nothing, because
        // `middle::closures` then gives it the unit type and the callee has no
        // instruction that reads it. Every lambda in the four kernels is one of
        // those, so this is the common case and not a corner.
        if shape.env_w > 0 || off("envskip") {
            self.mv(base + shape.env, ops.fslot + 8, 8);
        }
        if let (Some(o), Some((c, w, counted))) = (ctx_off, ops.ctx) {
            self.mv(base + o, c, w);
            if counted {
                self.retain_at(base + o);
            }
        }
        if let Some(o) = acc_off {
            self.mv(base + o, dslot, ops.acc_w);
        }
        self.elem_load(base + elem_off, s_src, s_i, ops.si, ops.elem_w);
        if ops.elem_counted {
            self.retain_at(base + elem_off);
        }

        // The step.
        if shape.direct {
            self.emit(
                "call",
                &[
                    ("JIT_N", V::I(u64::from(base))),
                    ("JIT_P", V::I(u64::from(base))),
                    ("JIT_CALLEE", V::Fn(shape.func)),
                    ("JIT_CONT0", V::Fall),
                ],
            );
        } else {
            self.emit(
                "calli",
                &[
                    ("JIT_A", V::I(u64::from(ops.fslot))),
                    ("JIT_N", V::I(u64::from(base))),
                    ("JIT_P", V::I(u64::from(base))),
                    ("JIT_CONT0", V::Fall),
                ],
            );
        }
        let answer = base + shape.ret;

        // -- what each loop does with the answer ----------------------------
        let mut early: Option<u32> = None;
        match kind {
            Step::Map => self.elem_store(answer, s_dst, s_i, ops.out_stride, ops.out_w),
            Step::Filter => {
                let l_skip = st.label();
                // The predicate answers a whole zero-extended word.
                let brkey = self.arm_key("br/f", "JIT_T");
                self.emit(
                    &brkey,
                    &[
                        ("JIT_A", V::I(u64::from(answer))),
                        ("JIT_T", V::Fall),
                        ("JIT_F", V::Blk(l_skip)),
                    ],
                );
                // Re-read the element rather than trusting the step's own
                // parameter slot, which the callee's slot allocator may have
                // reused for one of its locals. The step's frame is dead here,
                // so `elem_off` is only a scratch address.
                self.elem_load(base + elem_off, s_src, s_i, ops.si, ops.elem_w);
                self.elem_store(base + elem_off, s_dst, s_k, ops.out_stride, ops.out_w);
                // The copy is a second owner of whatever the element holds;
                // the count the predicate was handed was its own and is gone.
                if ops.elem_counted {
                    self.retain_at(base + elem_off);
                }
                self.add_imm(s_k, s_k, 1, base + shape.env);
                st.place(l_skip, self.region.code_addr());
            }
            Step::Fold => self.mv(dslot, answer, ops.acc_w),
            Step::Count => self.emit(
                "bin/add/u64/ff/f",
                &[
                    ("JIT_D", V::I(u64::from(dslot))),
                    ("JIT_A", V::I(u64::from(dslot))),
                    ("JIT_B", V::I(u64::from(answer))),
                    ("JIT_CONT", V::Fall),
                ],
            ),
            // `any` and `all` leave at the first element that decides the
            // answer, which is what their `core/list` declarations promise by
            // taking no context: a step that cannot have an effect cannot
            // notice that it was not run.
            Step::Any | Step::All => {
                let l_exit = st.label();
                let want_true = kind == Step::Any;
                let brkey = self.arm_key("br/f", if want_true { "JIT_F" } else { "JIT_T" });
                let (tv, fv) = if want_true {
                    (V::Blk(l_exit), V::Fall)
                } else {
                    (V::Fall, V::Blk(l_exit))
                };
                self.emit(
                    &brkey,
                    &[("JIT_A", V::I(u64::from(answer))), ("JIT_T", tv), ("JIT_F", fv)],
                );
                early = Some(l_exit);
            }
        }

        if self.has("incbr/lt") && !off("incbr") {
            let key = self.arm_key("incbr/lt", "JIT_F");
            self.emit(
                &key,
                &[
                    ("JIT_D", V::I(u64::from(s_i))),
                    ("JIT_A", V::I(u64::from(s_i))),
                    ("JIT_N", V::I(1)),
                    ("JIT_B", V::I(u64::from(s_len))),
                    ("JIT_T", V::Blk(l_body)),
                    ("JIT_F", V::Fall),
                ],
            );
        } else {
            self.add_imm(s_i, s_i, 1, base + shape.env);
            self.br_lt(s_i, s_len, V::Blk(l_body), V::Fall, Some("JIT_F"));
        }

        if let Some(l_exit) = early {
            // The loop that *finished* keeps the answer it started with, so it
            // has to jump over the early-exit block rather than fall into it.
            // Falling into it was the first bug this file had, and it presented
            // exactly where it should have: `any` answered true on a list
            // nothing matched, and `all` answered false on one everything did.
            self.emit("jump", &[("JIT_T", V::Blk(l_done))]);
            st.place(l_exit, self.region.code_addr());
            self.imm_to(dslot, u64::from(kind == Step::Any));
        }

        st.place(l_done, self.region.code_addr());
        if kind == Step::Filter {
            self.mv(dslot, s_dst, 8);
            self.mv(dslot + 8, s_k, 8);
        }
        self.stats.list_loops += 1;
        if shape.direct {
            self.stats.list_direct += 1;
        }
        true
    }
}
