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
//! element** (`cranelift/emit.rs::call_closure`). Three reports running named that boundary as the whole
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
//! `cranelift` asks `middle::rc`'s classifier (`Cx::rc_counted`) rather than the
//! layout table's, because retaining what rc does not count adds one half of a
//! pair nothing completes. `emit.rs::rc_counted` is this backend's one classifier
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
use crate::compiler::middle::ir;
use crate::compiler::middle::layout::{EnumRepr, Layout, Repr};
use crate::compiler::semantics::types::Ty;

/// Which loop. The six whose answer is one carried value; the ones whose answer
/// is an enum, a second block or a sort are below, because they are not that
/// shape.
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
/// same rows as `backend/intrinsic_keys.rs`'s table, for the nine keys of it
/// this backend open-codes.
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
/// was: `STENCIL_OFF=listloop` puts every `list.*` call back through the
/// ordinary runtime call, `estride` gives up the stride-baked
/// load/store twins, `incbr` gives up the fused back edge, and `envskip` writes
/// the environment even when it weighs nothing.
fn off(name: &str) -> bool {
    std::env::var("STENCIL_OFF").unwrap_or_default().split(',').any(|x| x == name)
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
    /// The context the step is threaded: offset, slot width, and the type
    /// where it owns a count.
    ctx: Option<(u32, u32, Option<Ty>)>,
    /// `fold`'s initial accumulator.
    init: Option<u32>,
    acc_w: u32,
    acc_counted: Option<Ty>,
    si: u32,
    elem_w: u32,
    elem_counted: Option<Ty>,
    out_stride: u32,
    out_w: u32,
}

impl<'a> Jit<'a> {
    /// The source type behind an IR type, where it owns a counted block
    /// anywhere inside it, and `None` where it owns none.
    ///
    /// The *deep* question `middle::rc` asks itself, not the top-level repr:
    /// a struct of two `Str`s is counted and its layout is `Aggregate`, so the
    /// shallow test skipped exactly the retains a step handed such a value
    /// needs.
    fn counted(&mut self, prog: &ir::Program, t: ir::Type) -> Option<Ty> {
        let ty = super::rtcall::source_ty(prog, t)?;
        self.rc_counted(&ty).then_some(ty)
    }

    /// The retain a value crossing a function value needs: the whole walk, not
    /// one `incref`, because what is handed over may be a struct with a
    /// counted field rather than a bare pointer.
    fn retain_value(&mut self, st: &mut Fn2, ty: &Ty, at: u32) {
        if std::env::var("STENCIL_NOFREE").is_ok_and(|v| v == "1") {
            return;
        }
        if let Err(why) = self.walk_rc(st, ty, at, true, 0) {
            self.unsupported(why);
        }
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
            if *own == ir::Ownership::Borrow && self.counted(prog, *t).is_some() {
                direct = false;
            }
        }
        let env_w = self.width_of(prog, *g.sig.params.first()?);
        // A lambda that captured something takes its environment as a record
        // out of the heap block the closure carries (`glue.rs::build_env`), and
        // unpacking that at the call site would be the thunk written twice — so
        // the direct call is for the capture-free step, whose environment
        // parameter is the unit type and weighs nothing.
        if env_w > 0 {
            direct = false;
        }
        Some(StepShape { func, ret, env, params, direct, env_w })
    }

    /// The **thunk's** frame, reconstructed from the closure's *type*.
    ///
    /// What a `calli` enters is never the lifted lambda — it is the thunk
    /// `glue.rs` generates, whose frame is `[rets][env: 8][args...]` with the
    /// environment one word because it is the block's pointer. That is the same
    /// arithmetic `Jit::call_indirect` does at every indirect call site.
    ///
    /// Used wherever the call is not direct: inside the `Body::Runtime`
    /// function, whose closure is a parameter and where `Jit::plan`'s table
    /// cannot be consulted, and at a call site whose step captured something.
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
        // into a whole one; see `sources.rs`'s `eloadz` family for why.
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
    pub(crate) fn elem_store(&mut self, src: u32, base: u32, i: u32, stride: u32, bytes: u32) {
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

    /// A `list.*` call written in a function body. The step is a `MakeClosure`
    /// this function can see, so the call is direct and the callee's frame
    /// offsets come out of `Jit::plan`.
    ///
    /// Answers whether the call was open-coded; `false` leaves the caller to
    /// emit an ordinary call to the `Body::Runtime` function — whose body is
    /// now [`Jit::list_loop_rt`]'s loop, and an ordinary runtime call only
    /// where that declines too.
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
        // A step that cannot be called directly is called through the closure's
        // code word, which is `glue.rs`'s **thunk** — so the frame to fill is
        // the thunk's `[rets][env: 8][args]` and not the lifted lambda's, whose
        // first parameter is the environment *record*. Filling the lambda's
        // shape and then entering the thunk was a real miscompile: every
        // argument landed one slot away from where the callee read it.
        let shape = if shape.direct {
            shape
        } else {
            match self.step_shape_ty(prog, code.ty_of(f), want) {
                Some(s) => s,
                None => return false,
            }
        };
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
    /// `count`, for which `cli/runtime/list.rs` has no entry at all.
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
        ctx: Option<(u32, u32, Option<Ty>)>,
        init: Option<u32>,
    ) -> Option<LoopOps> {
        let si = self.elem_stride(prog, xs_ty)?;
        // The element's *width* is what may be copied out of the block; the
        // stride is what separates two of them. They differ when an element's
        // alignment pads it, and copying the padding of the last element would
        // read past the allocation.
        let ir::Type::Agg(xid) = xs_ty else { return None };
        let Ty::Array(elem_ty) = prog.type_info(xid).ty.clone() else { return None };
        let elem_l = self.layouts_of((*elem_ty).clone());
        let elem_w = elem_l.size.max(1);
        let elem_counted = self.rc_counted(&elem_ty).then(|| (*elem_ty).clone());
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
        // the source's length and used to its kept prefix.
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
                if let Some(t) = ops.acc_counted.clone() {
                    self.retain_value(st, &t, dslot);
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
        if let (Some(o), Some((c, w, counted))) = (ctx_off, ops.ctx.clone()) {
            self.mv(base + o, c, w);
            if let Some(t) = counted {
                self.retain_value(st, &t, base + o);
            }
        }
        if let Some(o) = acc_off {
            self.mv(base + o, dslot, ops.acc_w);
        }
        self.elem_load(base + elem_off, s_src, s_i, ops.si, ops.elem_w);
        if let Some(t) = ops.elem_counted.clone() {
            self.retain_value(st, &t, base + elem_off);
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
                if let Some(t) = ops.elem_counted.clone() {
                    self.retain_value(st, &t, base + elem_off);
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

// ---------------------------------------------------------------------------
// The rest of the surface: an answer that is an enum, a second block, or a sort
// ---------------------------------------------------------------------------
//
// `emit_list_loop` above carries one value through one walk, which is what
// `map`, `filter`, `fold`, `any`, `all` and `count` are. The six below are not
// that shape and each is not for its own reason: `find` and `foldResult` build
// an `Option` and a `Result` and leave early, `sortBy` is a stable bottom-up
// merge over two blocks, and `zip` and `flatten` read a *second* element layout
// that no runtime entry could be handed (`cli/runtime/list.rs`'s header).
// `deriveArrayEq` and `deriveArrayShow` are here rather than in `emit.rs`
// because they are the same loop over the same code pointer.
//
// Every one of them calls the step through the closure's `code` word — the
// thunk `glue.rs` generates — rather than directly. The direct call is
// `emit_list_loop`'s optimisation and it is not repeated here: what it buys is
// one indirection per element, and what it costs is a second copy of
// `glue.rs::thunk`'s ownership reconciliation at every one of these sites.

/// Where this half's scratch words begin, as an offset **from `Fn2::scratch`**:
/// past the loops above (words 8–12) and past `rtcall.rs`'s C argument area
/// (words 16–28).
const LOOP_SCRATCH: u32 = 256;

/// Scratch word `k` of this half, still relative to `Fn2::scratch`.
fn t(k: u32) -> u32 {
    LOOP_SCRATCH + k * 8
}

/// Where a whole element is staged on its way between two blocks.
///
/// Past the twenty-four single words above, and the only part of a frame whose
/// size depends on a *type* — so it is the one that has a bound and a refusal
/// rather than a fixed index. `jit::SCRATCH_WORDS` is what makes the room, and
/// [`STAGE_ROOM`] is what is left of it.
const STAGE: u32 = LOOP_SCRATCH + 24 * 8;
const STAGE_ROOM: u32 = super::jit::SCRATCH_WORDS as u32 * 8 - STAGE;

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

    /// `frame[d] = frame[a] + k`, with a scratch word for the level that has no
    /// immediate form.
    fn addk(&mut self, st: &Fn2, d: u32, a: u32, k: u64) {
        let scratch = st.scratch + t(23);
        self.add_imm(d, a, k, scratch);
    }

    /// Where one whole element is staged on its way between two blocks, or a
    /// refusal when the element is wider than the frame keeps room for.
    ///
    /// The one part of a frame whose size depends on a *type*, so it is the one
    /// with a bound: everything else this file uses is a single word at a fixed
    /// index. `jit::SCRATCH_WORDS` makes the room and [`STAGE_ROOM`] is what is
    /// left of it once the indices above have theirs.
    fn stage(&mut self, st: &Fn2, size: u32) -> Option<u32> {
        if size > STAGE_ROOM {
            self.unsupported(format!(
                "a `[T]` whose element is {size} bytes, past the {STAGE_ROOM} a frame \
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
        if off("listloop") {
            return false;
        }
        match key {
            "list.find" => self.list_find(prog, st, o, 1, false),
            "list.findIndex" => self.list_find(prog, st, o, 1, true),
            "list.sortBy" => self.list_sort(prog, st, o, 2),
            "list.foldResult" => self.list_fold_result(prog, st, o, None, 1, 2),
            "list.foldResultCtx" => self.list_fold_result(prog, st, o, Some(1), 2, 3),
            "list.zip" => self.list_zip(prog, st, o),
            "list.flatten" => self.list_flatten(prog, st, o),
            "deriveArrayEq" => self.derive_array_eq(prog, st, o),
            "deriveArrayShow" => self.derive_array_show(prog, st, o),
            _ => false,
        }
    }

    /// `find` and `findIndex`: the first element the predicate keeps, as an
    /// `Option`.
    ///
    /// The early exit is legal for `list_test`'s reason: the predicate is
    /// `fn(T) => Bool` (`list.buri`), which names no context, so SPEC 10.2
    /// leaves it no effect with which to notice how many times it was called.
    ///
    /// One retain, on the element the answer carries: the `Option` leaves here
    /// owned and `middle::rc` is what releases it.
    fn list_find(
        &mut self,
        prog: &ir::Program,
        st: &mut Fn2,
        o: &Operands,
        fi: usize,
        index: bool,
    ) -> bool {
        let (Some(&(xs, xt)), Some(&(fslot, fty))) = (o.args.first(), o.args.get(fi)) else {
            return false;
        };
        let Some(src) = self.block_at(prog, xs, xt) else { return false };
        let Some(c) = self.thunked(prog, st, fslot, fty, 1) else { return false };
        let ir::Type::Agg(id) = o.dest.1 else { return false };
        let l = self.layout_of(prog, id);
        let (d, pay) = (o.dest.0, payload_at(&l, 0));

        let i = st.scratch + t(0);
        self.imm_to(i, 0);
        let head = st.label();
        let done = st.label();
        let end = st.label();
        st.place(head, self.region.code_addr());
        self.br_lt(i, xs + 8, V::Fall, V::Blk(done), Some("JIT_T"));
        let Some(&p0) = c.params.first() else { return false };
        self.elem_load(p0, xs, i, src.stride, src.size);
        if src.counted {
            let e = src.elem.clone();
            self.retain_value(st, &e, p0);
        }
        self.thunk_call(&c);
        let cont = st.label();
        let brkey = self.arm_key("br/f", "JIT_T");
        self.emit(
            &brkey,
            &[
                ("JIT_A", V::I(u64::from(c.ret))),
                ("JIT_T", V::Fall),
                ("JIT_F", V::Blk(cont)),
            ],
        );
        if index {
            self.mv(d + pay, i, 8);
        } else {
            self.elem_load(d + pay, xs, i, src.stride, src.size);
            if src.counted {
                let e = src.elem.clone();
                self.retain_value(st, &e, d + pay);
            }
        }
        self.store_disc(&l, d, 0);
        self.emit("jump", &[("JIT_T", V::Blk(end))]);
        st.place(cont, self.region.code_addr());
        self.addk(st, i, i, 1);
        self.emit("jump", &[("JIT_T", V::Blk(head))]);
        st.place(done, self.region.code_addr());
        self.store_disc(&l, d, 1);
        st.place(end, self.region.code_addr());
        true
    }

    /// `foldResult` and `foldResultCtx`: a fold that stops at the first `.Err`.
    ///
    /// `runtime.js`'s `$list_foldResult` is transcribed rather than
    /// approximated, because the shape of its short circuit is what the answer
    /// is: an empty list answers `.Ok(init)` — a `Result` nothing built — and a
    /// step that answers `.Err` is handed back exactly as it came, without the
    /// remaining elements being visited. `.Ok` is variant 0 because
    /// `core/result` declares it first.
    ///
    /// One retain on the accumulator, on the way in, because a call through a
    /// function value owns its arguments and the initial accumulator arrives
    /// borrowed. After that each step consumes the count it is handed and
    /// answers another inside its `.Ok`, so the early exit leaks nothing.
    fn list_fold_result(
        &mut self,
        prog: &ir::Program,
        st: &mut Fn2,
        o: &Operands,
        ctx: Option<usize>,
        fi: usize,
        ii: usize,
    ) -> bool {
        let (Some(&(xs, xt)), Some(&(fslot, fty)), Some(&(init, it))) =
            (o.args.first(), o.args.get(fi), o.args.get(ii))
        else {
            return false;
        };
        let Some(src) = self.block_at(prog, xs, xt) else { return false };
        let want = 1 + usize::from(ctx.is_some()) + 1;
        let Some(c) = self.thunked(prog, st, fslot, fty, want) else { return false };
        let ir::Type::Agg(id) = o.dest.1 else { return false };
        let l = self.layout_of(prog, id);
        let (res, ok_at) = (o.dest.0, payload_at(&l, 0));
        let acc_w = self.slot_bytes_of(prog, it);
        let acc_ty = self.counted(prog, it);

        // The accumulator, in the staging area: it is read by the step, written
        // by the step's answer and read again by the exit, and one address for
        // all three keeps both paths out of it the same copy. It is the one
        // slot here whose size comes from a *type* and so the one with a bound.
        let Some(acc) = self.stage(st, acc_w) else { return true };
        self.mv(acc, init, acc_w);
        if let Some(a) = acc_ty.clone() {
            self.retain_value(st, &a, acc);
        }

        let i = st.scratch + t(8);
        self.imm_to(i, 0);
        let head = st.label();
        let done = st.label();
        let end = st.label();
        st.place(head, self.region.code_addr());
        self.br_lt(i, xs + 8, V::Fall, V::Blk(done), Some("JIT_T"));
        let mut pi = 0usize;
        if let Some(k) = ctx {
            let Some(&(cslot, cty)) = o.args.get(k) else { return false };
            let Some(&to) = c.params.get(pi) else { return false };
            let w = self.slot_bytes_of(prog, cty);
            self.mv(to, cslot, w);
            if let Some(ct) = self.counted(prog, cty) {
                self.retain_value(st, &ct, to);
            }
            pi += 1;
        }
        let Some(&to_acc) = c.params.get(pi) else { return false };
        self.mv(to_acc, acc, acc_w);
        pi += 1;
        let Some(&to_elem) = c.params.get(pi) else { return false };
        self.elem_load(to_elem, xs, i, src.stride, src.size);
        if src.counted {
            let e = src.elem.clone();
            self.retain_value(st, &e, to_elem);
        }
        self.thunk_call(&c);
        let rw = self.slot_bytes_of(prog, o.dest.1);
        self.mv(res, c.ret, rw);
        let carry = st.label();
        let tag = st.scratch + t(9);
        self.load_disc(&l, res, tag);
        let brkey = self.arm_key("brcmp/eq/u64/fi", "JIT_T");
        self.emit(
            &brkey,
            &[
                ("JIT_A", V::I(u64::from(tag))),
                ("JIT_K", V::I(0)),
                ("JIT_T", V::Blk(carry)),
                ("JIT_F", V::Fall),
            ],
        );
        self.emit("jump", &[("JIT_T", V::Blk(end))]);
        st.place(carry, self.region.code_addr());
        self.mv(acc, res + ok_at, acc_w);
        self.addk(st, i, i, 1);
        self.emit("jump", &[("JIT_T", V::Blk(head))]);
        st.place(done, self.region.code_addr());
        self.mv(res + ok_at, acc, acc_w);
        self.store_disc(&l, res, 0);
        st.place(end, self.region.code_addr());
        true
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
        self.addk(st, i, i, 1);
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
        self.addk(st, i, i, 1);
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
        self.addk(st, j, j, 1);
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
        self.addk(st, i, i, 1);
        self.emit("jump", &[("JIT_T", V::Blk(head))]);
        st.place(done, self.region.code_addr());
        true
    }

    /// `sortBy`: a **stable bottom-up merge**, `cranelift/emit.rs::list_sort`
    /// pass for pass.
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
        self.addk(st, i, i, 1);
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
        self.addk(st, li, li, 1);
        self.emit("jump", &[("JIT_T", V::Blk(took))]);
        st.place(take_right, self.region.code_addr());
        self.elem_load(staging, a, ri, stride, size);
        self.addk(st, ri, ri, 1);
        st.place(took, self.region.code_addr());
        self.elem_store(staging, b, out, stride, size);
        self.addk(st, out, out, 1);
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
        self.addk(st, i, i, 1);
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

    /// `deriveArrayEq` — a derived `Eq` where the field is a `[T]`.
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
        self.addk(st, i, i, 1);
        self.emit("jump", &[("JIT_T", V::Blk(head))]);
        st.place(done, self.region.code_addr());
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
        self.addk(st, i, i, 1);
        self.emit("jump", &[("JIT_T", V::Blk(head))]);
        st.place(done, self.region.code_addr());

        let args = [
            super::rtcall::Src::Word(ptr),
            super::rtcall::Src::Word(n),
            super::rtcall::Src::Addr(o.dest.0),
        ];
        if let Err(why) = self.c_call("buri_rt_show_list", st, &args, &[], o.dest.0, "v") {
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
        if let Err(why) = self.walk_rc(st, &str_ty, staging, false, 0) {
            self.unsupported(why);
        }
        self.addk(st, i, i, 1);
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
            Repr::Enum { repr: EnumRepr::Niche { null_at }, variants } => {
                if variants.get(v).is_some_and(|f| f.is_empty()) {
                    self.imm_to(at + null_at, 0);
                }
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
