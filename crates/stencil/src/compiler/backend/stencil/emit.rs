//! Stencil selection: one IR instruction to one or more stencils.
//!
//! This is the whole of the paper's §4 "code generation" for this IR. Each arm
//! either names a stencil key directly or falls back to a shorter sequence of
//! more general ones — which is what makes a configuration *level* meaningful:
//! a level that lacks `bin/add/i32/ff/f` gets `sext` + `bin/add/i64/ff/f` +
//! `zext`, and the sweep measures exactly that difference.

#![allow(
    clippy::arithmetic_side_effects,
    reason = "the sums are addresses in a frame and positions inside a value \
              that has already been laid out. A frame offset comes from \
              `Fn2::at`, `Fn2::scratch` or a `FrameSig`, and what is added to \
              it is a field offset, a scratch word or a callee's frame base — \
              all of them inside the frame `jit::frame_sigs` sized from the \
              same slot widths, so the address exists before this file names \
              it. A field offset and the width beside it come from one \
              `middle::layout` entry for the aggregate being read, which is \
              why `field_room` can subtract them: layout put the later field \
              after the earlier one. The rest counts the IR's own vectors — a \
              variant's fields, a call's arguments, a switch's cases — and is \
              bounded by a program already in memory"
)]

use super::abi::{Jump, Lit, Loc, Word};
use super::jit::KeyArg;
use crate::compiler::backend::intrinsic_keys::{
    checked_kind, json_arm, json_variant, native_body, CheckedKind, JsonArm, Leaf,
};
use crate::compiler::semantics::types::{field_types, variant_types};

/// Where an edge's register half reads from.
#[derive(Clone, Copy, PartialEq, Eq)]
enum RSrc {
    Reg(u8),
    Slot(u32),
}
use super::glue::Helper;
use super::jit::{agg_of, copyless_test, enum_ty, fn_ty, Bind, Fn2, Jit, Plan, RET, V};
use super::route;
use super::rtcall::{source_ty, Src, EQUAL, GREATER, LESS};
use super::runtime;
use crate::compiler::backend::counts::{Field, Glue, Op, Site};
use crate::compiler::middle::ir::{self, BinOp, Const, Inst, Target, Term, UnOp};
use crate::compiler::middle::layout::{EnumRepr, Repr};
use crate::compiler::semantics::types::{FuncIdx, IntPrim, Prim, Ty, TyKind};
use crate::diagnostics::Invariant;

/// How deep an inline reference-count walk goes.
///
/// [`Jit::field_out_of_line`] sends every compound field two levels short of
/// this into its type's glue, so a walk ends here at the latest: one level for
/// a niche `Option`'s payload and one for that payload's `Str`, `[T]` or
/// closure fields.
pub(crate) const RC_DEPTH: u32 = 8;

/// How many levels of a compound type's counted-pointer walk are emitted
/// inline before the rest goes through the type's own glue. See
/// [`Jit::walk_field`].
const RC_INLINE: u32 = 2;

/// The heaviest field, by `Counts::weight`, that stays inline past
/// [`RC_INLINE`]. Its whole walk is at most this many tests and pointer
/// operations, so inlining it cannot multiply the way a deep record can, and
/// the call it replaces — a frame, a copy of the value into it and back out —
/// cost more than the walk.
const RC_LIGHT: u32 = 4;

/// The heaviest value, by `Counts::weight`, whose reference operation is
/// written out where it happens. A heavier one calls its type's glue: a walk
/// is a test per counted variant and an operation per counted field, and a
/// function makes a reference operation per arm or per field it reads, so
/// writing each one out made a match over `n` counted variants `n²` code.
const RC_OUT_OF_LINE: u32 = 16;

/// [`prim_tag`] where the operation makes the type numeric: `core/number` and
/// `Ordered` at a primitive, never at `Str`.
pub fn num_tag(p: Prim) -> (&'static str, u32, bool) {
    prim_tag(p).or_ice("a numeric operation is at a type with a stencil tag")
}

/// The aggregate a function answers, for a `Body::Runtime` whose signature
/// makes it one.
fn ret_agg(prog: &ir::Program, fi: FuncIdx) -> ir::TypeId {
    agg_of(prog.funcs.get(fi).sig.ret)
}

pub fn prim_tag(p: Prim) -> Option<(&'static str, u32, bool)> {
    Some(match p {
        Prim::Bool => ("u64", 64, false),
        Prim::I8 => ("i8", 8, true),
        Prim::I16 => ("i16", 16, true),
        Prim::I32 => ("i32", 32, true),
        Prim::I64 => ("i64", 64, true),
        Prim::U8 => ("u8", 8, false),
        Prim::U16 => ("u16", 16, false),
        Prim::U32 => ("u32", 32, false),
        Prim::U64 => ("u64", 64, false),
        Prim::Char => ("u32", 32, false),
        Prim::F32 => ("f32", 32, true),
        Prim::F64 => ("f64", 64, true),
        // Sixteen bytes, and the register file is eight: every stencil at
        // these two widths is frame-to-frame (`sources.rs::wide`), which is
        // what the `Loc` tests at each call site already fall back to.
        Prim::I128 => ("i128", 128, true),
        Prim::U128 => ("u128", 128, false),
        Prim::Str | Prim::Template => return None,
    })
}

/// The runtime hash of an integer or a `Bool` in the frame slot at `v`, and
/// the words it is handed. Up to 32 bits, `$mix` reads the low word, which a
/// slot holds zero-extended at its own width; wider, every word goes across.
// `Fn2::constants` and `Fn2::wt` are `Jit::plan`'s side tables and carry one
// entry per value of the `ir::Code` being emitted, exactly as `Fn2::slot` —
// which is why `Fn2::at` reads its own with the same fallback. An index past
// the end would mean the IR disagreed with itself, so each of these answers
// what a value the analyses never saw would mean rather than panicking:
// holding no immediate, and promoted into a register that nothing reads out of
// the frame.

/// The literal `v` holds when a stencil can take it as an immediate, or zero —
/// which is the same zero the immediate hole is ignored at when it cannot.
fn constant(st: &Fn2, v: ir::ValueId) -> u64 {
    st.constants.get(v.index()).copied().flatten().unwrap_or(0)
}

/// Whether a register-promoted `v` also needs its frame slot kept in step.
fn write_through(st: &Fn2, v: ir::ValueId) -> bool {
    st.wt.get(v.index()).copied().unwrap_or(false)
}

impl<'a> Jit<'a> {
    // -- primitives --------------------------------------------------------

    pub(crate) fn mv(&mut self, dst: u32, src: u32, bytes: u32) {
        if bytes == 0 || dst == src {
            return;
        }
        // Decomposed into fixed-width copies rather than a `memcpy` call: a
        // stencil that calls anything has to preserve the CPS register file
        // across it, and the widest of these is two `ldp`/`stp` pairs.
        let (mut d, mut s, mut left) = (dst, src, bytes);
        while left > 0 {
            let (n, key) = [
                (32u32, "mov/32"),
                (24, "mov/24"),
                (16, "mov/16"),
                (8, "mov/8"),
                (4, "mov/4"),
                (2, "mov/2"),
            ]
            .into_iter()
            .find(|(n, _)| *n <= left)
            .unwrap_or((1, "mov/1"));
            self.emit(
                key,
                &[
                    Lit::D.of(d as u64),
                    Lit::A.of(s as u64),
                    Jump::Cont.to(V::Fall),
                ],
            );
            d += n;
            s += n;
            left -= n;
        }
    }

    pub(crate) fn imm_to(&mut self, dst: u32, v: u64) {
        if v == 0 {
            self.emit("imm/z", &[Lit::D.of(dst as u64), Jump::Cont.to(V::Fall)]);
        } else if v < (1u64 << 32) {
            self.emit(
                "imm/32",
                &[Lit::D.of(dst as u64), Lit::N.of(v), Jump::Cont.to(V::Fall)],
            );
        } else {
            self.emit(
                "imm/64",
                &[
                    Lit::D.of(dst as u64),
                    Word::M.lit(v),
                    Jump::Cont.to(V::Fall),
                ],
            );
        }
    }

    /// [`Jit::imm_to`] for a value that is an address inside the region. Always
    /// the 64-bit form: a region address never fits 32 bits, and the pooled
    /// word it produces is what `cache.rs` relocates.
    pub(crate) fn imm_to_ptr(&mut self, dst: u32, v: u64) {
        debug_assert!(
            super::region::is_pool_handle(v),
            "not a constant-pool handle"
        );
        self.emit(
            "imm/64",
            &[
                Lit::D.of(dst as u64),
                Word::M.ptr(v),
                Jump::Cont.to(V::Fall),
            ],
        );
    }

    /// Stores a frame word's low `w` bytes into a narrower field.
    pub(crate) fn store_w(&mut self, dst: u32, src: u32, w: u32) {
        match w {
            0 => {}
            1 | 2 | 4 => self.emit(
                match w {
                    1 => "store/1",
                    2 => "store/2",
                    _ => "store/4",
                },
                &[
                    Lit::D.of(dst as u64),
                    Lit::A.of(src as u64),
                    Jump::Cont.to(V::Fall),
                ],
            ),
            _ => self.mv(dst, src, w),
        }
    }

    /// Loads a narrow field into a whole frame word, zero-extended — the
    /// invariant every frame slot holds.
    pub(crate) fn load_w(&mut self, dst: u32, src: u32, w: u32) {
        match w {
            0 => {}
            1 | 2 | 4 => self.emit(
                match w {
                    1 => "loadu/1",
                    2 => "loadu/2",
                    _ => "loadu/4",
                },
                &[
                    Lit::D.of(dst as u64),
                    Lit::A.of(src as u64),
                    Jump::Cont.to(V::Fall),
                ],
            ),
            _ => self.mv(dst, src, w),
        }
    }

    // -- instructions ------------------------------------------------------

    pub(crate) fn inst(&mut self, prog: &ir::Program, code: &ir::Code, st: &mut Fn2, i: &Inst) {
        match i {
            Inst::Const { dest, value } => {
                let off = st.at(*dest);
                let ty = code.ty_of(*dest);
                match value {
                    Const::Unit | Const::Undef | Const::Null => self.imm_to(off, 0),
                    Const::Bool(b) => self.imm_to(off, u64::from(*b)),
                    Const::Char(c) => self.imm_to(off, u32::from(*c) as u64),
                    Const::Int { bits, negative } => {
                        let bits = bits.get();
                        let w = self.width_of(prog, ty);
                        // A literal wider than a word is two stores: truncating
                        // it to one would silently drop the top half of every
                        // `I128` constant in the program.
                        if w > 8 {
                            let x = if *negative { bits.wrapping_neg() } else { bits };
                            self.imm_to(off, x as u64);
                            self.imm_to(off + 8, (x >> 64) as u64);
                            return;
                        }
                        let x = bits as u64;
                        let x = if *negative { x.wrapping_neg() } else { x };
                        let x = if w >= 8 {
                            x
                        } else {
                            x & ((1u64 << (w * 8)) - 1)
                        };
                        self.imm_to(off, x);
                    }
                    Const::Float(f) => {
                        let bits = if ty == ir::Type::F32 {
                            (*f as f32).to_bits() as u64
                        } else {
                            f.to_bits()
                        };
                        self.imm_to(off, bits);
                    }
                    Const::Str(s) => {
                        // VALUE-MODEL.md §3: a literal is `{ base: null, ptr,
                        // len }` and IMMORTAL, so it touches no allocator; bit
                        // 63 of `len` is the ASCII flag (§3.1).
                        let bytes = s.as_bytes();
                        let ptr = self.region.pool_bytes(bytes);
                        let ascii = bytes.iter().all(|b| *b < 0x80);
                        let len = bytes.len() as u64 | (u64::from(ascii) << 63);
                        self.imm_to(off, 0);
                        self.imm_to_ptr(off + 8, ptr);
                        self.imm_to(off + 16, len);
                    }
                }
            }
            Inst::Unary {
                dest,
                op,
                prim,
                arg,
            } => self.unary(prog, code, st, *dest, *op, *prim, *arg),
            Inst::Binary {
                dest,
                op,
                prim,
                lhs,
                rhs,
            } => self.binary(st, *dest, *op, *prim, *lhs, *rhs),
            Inst::Call { dest, func, args } => self.call(prog, code, st, *dest, *func, args),
            Inst::CallIndirect { dest, callee, args } => {
                self.call_indirect(prog, code, st, *dest, *callee, args)
            }
            Inst::CallIntrinsic { dest, key, args } => {
                let route = *self.routes.calls.get(*key);
                self.intrinsic(prog, code, st, *dest, route, args)
            }
            // The row's three arguments, and the step after them.
            Inst::Parallel(p) => {
                let tail = self.step_words(st, p);
                let list: Vec<(u32, ir::Type)> = [p.sched, p.ctx, p.items]
                    .iter()
                    .map(|a| (st.at(*a), code.ty_of(*a)))
                    .collect();
                let dest = (st.at(p.dest), code.ty_of(p.dest));
                self.rt_call(prog, st, runtime::parallel(p.tasks), dest, &list, tail);
            }
            Inst::MakeStruct { dest, fields } => {
                let d = st.at(*dest);
                let (l, owner) = match code.ty_of(*dest) {
                    ir::Type::Agg(id) => (
                        self.layout_id_shared(prog, id.id()),
                        &prog.type_info(id.id()).ty,
                    ),
                    _ => crate::diagnostics::ice("stencil: MakeStruct of a non-aggregate"),
                };
                let ftys = field_types(self.tables, owner);
                for (i, f) in fields.iter().enumerate() {
                    let w = self.width_of(prog, code.ty_of(*f));
                    if w == 0 {
                        continue;
                    }
                    let off = l.field(i);
                    if ftys.get(i).is_some_and(|t| self.boxes(owner, t)) {
                        self.box_into(st, d + off, st.at(*f), w);
                        continue;
                    }
                    self.store_w(d + off, st.at(*f), w);
                }
            }
            Inst::GetField { dest, agg, index } => {
                let (l, owner) = match code.ty_of(*agg) {
                    ir::Type::Agg(id) => (
                        self.layout_id_shared(prog, id.id()),
                        &prog.type_info(id.id()).ty,
                    ),
                    _ => crate::diagnostics::ice("stencil: GetField of a non-aggregate"),
                };
                let w = self.width_of(prog, code.ty_of(*dest));
                let off = l.field(*index as usize);
                let src = st.at(*agg) + off;
                let fty = self.field_ty(owner, None, *index as usize);
                if fty.is_some_and(|t| self.boxes(owner, &t)) {
                    self.unbox_from(st, st.at(*dest), src, w);
                    return;
                }
                self.load_w(st.at(*dest), src, w);
            }
            Inst::MakeEnum {
                dest,
                variant,
                fields,
            } => self.make_enum(prog, code, st, *dest, *variant, fields),
            Inst::GetPayload {
                dest,
                agg,
                variant,
                index,
            } => {
                let (l, owner) = match code.ty_of(*agg) {
                    ir::Type::Agg(id) => (
                        self.layout_id_shared(prog, id.id()),
                        &prog.type_info(id.id()).ty,
                    ),
                    _ => crate::diagnostics::ice("stencil: GetPayload of a non-aggregate"),
                };
                let w = self.width_of(prog, code.ty_of(*dest));
                let off = l.variant(*variant as usize).get(*index as usize).copied();
                match off {
                    Some(o) => {
                        let src = st.at(*agg) + o;
                        let fty = self.field_ty(owner, Some(*variant as usize), *index as usize);
                        if fty.is_some_and(|t| self.boxes(owner, &t)) {
                            self.unbox_from(st, st.at(*dest), src, w);
                            return;
                        }
                        self.load_w(st.at(*dest), src, w)
                    }
                    None => crate::diagnostics::ice("stencil: GetPayload of an absent field"),
                }
            }
            Inst::GetTag { dest, agg } => self.get_tag(prog, code, st, *dest, *agg),
            Inst::MakeClosure { dest, func, env } => {
                // `{ code, env }` has one shape whatever the target is, and a
                // **thunk** is what makes that true: a lambda lifted by
                // `middle::closures` takes the environment as its first
                // parameter, and a named function referenced as a value
                // (`let f = identity<Int>`) does not.
                // `llvm/emit.rs::make_closure` builds the same two words.
                let d = st.at(*dest);
                // How many parameters the closure's *type* declares is what
                // separates the environment parameter from a value one: a
                // capture-free lambda still has a leading parameter, of the
                // unit type, and a plain `FuncRef` has none at all.
                let args = prog.types.sig(fn_ty(code.ty_of(*dest))).params.len();
                let thunk = self.helper(super::glue::Helper::Thunk {
                    func: *func,
                    args: args as u32,
                    boxed: env.is_some(),
                });
                self.emit(
                    "imm/64",
                    &[
                        Lit::D.of(d as u64),
                        Word::M.sym(thunk),
                        Jump::Cont.to(V::Fall),
                    ],
                );
                match env {
                    Some(e) => self.build_env(prog, code, st, d + super::glue::ENV_WORD, *e),
                    None => self.imm_to(d + super::glue::ENV_WORD, 0),
                }
            }
            Inst::IncRef { value } => self.rc(prog, code, st, *value, Op::Retain),
            Inst::DecRef { value, .. } => self.rc(prog, code, st, *value, Op::Release),
            Inst::Abort { message } => {
                // `buri_rt_abort` is `noreturn`, so whatever the block's
                // terminator was is dead. The census counts 6,870 `abort +
                // unreachable` pairs at 104k lines; this is the other half of
                // the same observation.
                //
                // It takes the bytes and a count rather than a NUL-terminated
                // string, because that is the shape `cli/runtime/lib.rs` §2
                // rule 1 gives a borrowed `Str` and the abort message the other
                // two backends pass is one.
                let b = message.as_bytes();
                let p = self.region.pool_bytes(b);
                self.emit("abort", &[Word::M.ptr(p), Lit::N.of(b.len() as u64)]);
            }
            Inst::MakeArray { dest, elems } => {
                // One allocation and `n` stores (VALUE-MODEL.md §4).
                let d0 = st.at(*dest);
                let stride = self.array_stride(prog, code.ty_of(*dest));
                // One block of `n * stride` bytes with VALUE-MODEL.md §2's
                // header, and the count beside it; the elements are stored
                // below. `elemalloc` is `buri_rt_alloc` behind the
                // null-for-empty rule, which is the same block
                // `buri_rt_list_new` would have answered.
                let n = st.scratch;
                self.imm_to(n, elems.len() as u64);
                self.emit(
                    "elemalloc",
                    &[
                        Lit::D.of(d0 as u64),
                        Lit::A.of(n as u64),
                        Lit::P.of(stride),
                        Jump::Cont0.to(V::Fall),
                    ],
                );
                self.imm_to(d0 + 8, elems.len() as u64);
                for (i, e) in elems.iter().enumerate() {
                    let w = self.width_of(prog, code.ty_of(*e)).min(stride as u32);
                    let src = st.at(*e);
                    let base = i as u64 * stride;
                    // An element wider than a word is copied a word at a time,
                    // which is what the aggregate's flat frame layout allows.
                    let (mut done, mut k) = (0u32, 0u64);
                    while done < w {
                        let (n, key) = [(8u32, "pstore/8"), (4, "pstore/4"), (2, "pstore/2")]
                            .into_iter()
                            .find(|(n, _)| *n <= w - done)
                            .unwrap_or((1, "pstore/1"));
                        self.emit(
                            key,
                            &[
                                Lit::A.of(d0 as u64),
                                Lit::B.of((src + done) as u64),
                                Lit::N.of(base + k),
                                Jump::Cont.to(V::Fall),
                            ],
                        );
                        done += n;
                        k += u64::from(n);
                    }
                }
            }
            // "An element, with the bounds check already done" — every
            // emission site is guarded by a comparison against `ArrayLen` in a
            // dominating block (`middle/ir.rs`), so this is one indexed copy
            // and nothing else. A register-machine backend spells the same
            // three pieces; `eload` is the three of them as one stencil.
            Inst::ArrayGet { dest, array, index } => {
                let (stride, w, _) = self.array_elem(prog, code.ty_of(*array));
                // The pointer is read where it lives, unless the destination
                // shares its slot: a narrow element clears its destination
                // first, which would clear the pointer.
                let (d, a, i) = (st.at(*dest), st.at(*array), st.at(*index));
                let base = if d < a + 8 && a < d + w.max(8) {
                    let s0 = st.scratch + 48;
                    self.mv(s0, a, 8);
                    s0
                } else {
                    a
                };
                self.elem_load(d, base, i, stride, w);
            }
            Inst::ArrayLen { dest, array } => {
                // `[T]` is `{ ptr, len }`, and the length is O(1) by §4.
                self.mv(st.at(*dest), st.at(*array) + 8, 8)
            }
            // The count goes through scratch first, because the destination's
            // slot may be the one the count dies in.
            Inst::ArrayAlloc { dest, len } => {
                let stride = self.array_stride(prog, code.ty_of(*dest));
                let (d, n) = (st.at(*dest), st.scratch);
                self.mv(n, st.at(*len), 8);
                self.emit(
                    "elemalloc",
                    &[
                        Lit::D.of(u64::from(d)),
                        Lit::A.of(u64::from(n)),
                        Lit::P.of(stride),
                        Jump::Cont0.to(V::Fall),
                    ],
                );
                self.mv(d + 8, n, 8);
            }
            Inst::ArraySet {
                array,
                index,
                value,
            } => {
                let (stride, w, _) = self.array_elem(prog, code.ty_of(*array));
                self.elem_store(st.at(*value), st.at(*array), st.at(*index), stride, w);
            }
            // `elemalloc` zeroes the block, so the elements past `len` are the
            // spare slots the drop glue skips, and the block is kept.
            Inst::ArrayPrefix { dest, array, len } => {
                let (d, n) = (st.at(*dest), st.scratch);
                self.mv(n, st.at(*len), 8);
                self.mv(d, st.at(*array), 8);
                self.mv(d + 8, n, 8);
            }
            // `xs[from..]`, for the `..rest` of an array pattern. A **copy**,
            // not a view: VALUE-MODEL.md §4 is explicit that a `[T]` is never
            // one — its header is at `ptr - 16` — so handing back an interior
            // pointer would make the next `decref` read a header that is not
            // one. Every element is retained on the way out, so the counts
            // balance against the copy's own eventual release.
            Inst::ArraySlice { dest, array, from } => {
                let (stride, _, _) = self.array_elem(prog, code.ty_of(*array));
                let glue = self.element_glue(super::lists::element_of(prog, code.ty_of(*array)));
                let src = st.at(*array);
                let dst = st.at(*dest);
                // `buri_rt_list_slice(ptr, len, start, end, stride, retain,
                // out)`: **seven** parameters, and `end` is the length because
                // `ArraySlice` is `xs[from..]`. A `[T]` is two words
                // (VALUE-MODEL.md §4), not the three a `Str` is, so reading a
                // third slid every argument along and handed the entry a null
                // out-pointer.
                let args = [
                    Src::Word(src),
                    Src::Word(src + 8),
                    Src::Word(st.at(*from)),
                    Src::Word(src + 8),
                    Src::Imm(u64::from(stride)),
                    match glue {
                        Some(name) => Src::Sym(name),
                        None => Src::Imm(0),
                    },
                    Src::Addr(dst),
                ];
                self.c_call("buri_rt_list_slice", st, &args, &[], dst, "v");
            }
            Inst::ShowPrim { dest, prim, value } => {
                let (d, a) = (st.at(*dest), st.at(*value));
                // `quoted = false`: a `Char` renders as its own bytes.
                self.show_prim(st, *prim, a, d, false);
            }
        }
    }

    fn make_enum(
        &mut self,
        prog: &ir::Program,
        code: &ir::Code,
        st: &mut Fn2,
        dest: ir::ValueId,
        variant: u32,
        fields: &[ir::ValueId],
    ) {
        let e = enum_ty(code.ty_of(dest));
        let owner = &prog.type_info(e.id()).ty;
        let l = self.enum_layout(prog, e);
        let ftys = variant_types(self.tables, owner, variant as usize);
        let d = st.at(dest);
        let repr = &l.repr;
        let offs: &[u32] = l.variants.get(variant as usize).map_or(&[], Vec::as_slice);
        match repr {
            EnumRepr::Bare { tag } => self.imm_w(d, tag.size(), variant as u64),
            EnumRepr::Tagged { tag, .. } => {
                self.imm_w(d, tag.size(), variant as u64);
                self.store_variant(prog, code, st, d, fields, offs, owner, &ftys);
            }
            EnumRepr::Niche { null_at } => {
                if offs.is_empty() {
                    // The null variant: VALUE-MODEL.md §6's second niche.
                    self.imm_to(d + null_at, 0);
                } else {
                    self.store_variant(prog, code, st, d, fields, offs, owner, &ftys);
                }
            }
        }
    }

    /// One variant's payload fields, at the offsets the layout gave them.
    #[allow(
        clippy::too_many_arguments,
        reason = "one variant's fields, and the four tables that say where each \
                  goes and whether it is behind a pointer"
    )]
    fn store_variant(
        &mut self,
        prog: &ir::Program,
        code: &ir::Code,
        st: &mut Fn2,
        d: u32,
        fields: &[ir::ValueId],
        offs: &[u32],
        owner: &Ty,
        ftys: &[Ty],
    ) {
        for (i, (f, o)) in fields.iter().zip(offs.iter().copied()).enumerate() {
            let w = self.width_of(prog, code.ty_of(*f));
            if ftys.get(i).is_some_and(|t| self.boxes(owner, t)) {
                self.box_into(st, d + o, st.at(*f), w);
                continue;
            }
            self.store_w(d + o, st.at(*f), w);
        }
    }

    /// `derivePrimJson.<T>` — one primitive as a `Json`, which is
    /// `middle::derives`' third type-directed leaf.
    ///
    /// `llvm/emit.rs::json_prim` is the twin, and the two agree because the
    /// three decisions are made in one place each: [`json_arm`] says which arm
    /// a primitive encodes to, [`json_variant`] says where that arm sits in
    /// `core/json`'s declaration, and `middle::layout` says where its payload
    /// sits in the value. Nothing here reads a variant number or an offset it
    /// wrote down itself.
    ///
    /// The `Str` arm is the one with a count in it. `middle::rc`'s contract is
    /// that a runtime intrinsic **borrows** its arguments and returns a fresh
    /// count (`rc.rs`'s header), and this one's result *keeps* the argument's
    /// block — so the retain is the difference between that contract and a
    /// double free, and it is the whole walk rather than one `incref` because
    /// what is retained may be a struct with a counted field rather than a bare
    /// pointer.
    fn json_prim(
        &mut self,
        prog: &ir::Program,
        code: &ir::Code,
        st: &mut Fn2,
        prim: Prim,
        arg: ir::ValueId,
        dest: ir::ValueId,
    ) {
        let e = enum_ty(code.ty_of(dest));
        let owner = prog.type_info(e.id()).ty;
        let arm = json_arm(prim);
        let variant = json_variant(self.tables, &owner, arm)
            .or_ice("a `derive ToJson` answers `core/json`'s `Json`");
        let l = self.layout_id_shared(prog, e.id());
        let tag = match self.enum_layout(prog, e).repr {
            EnumRepr::Tagged { tag, .. } => Some(tag),
            _ => None,
        }
        .or_ice("`Json` is a tagged enum");
        let off = *l
            .variant(variant)
            .first()
            .or_ice("each `Json` arm a primitive takes carries a payload");
        let (d, a) = (st.at(dest), st.at(arg));
        self.imm_w(d, tag.size(), variant as u64);
        match arm {
            JsonArm::Bool => {
                let w = self.width_of(prog, code.ty_of(arg));
                self.store_w(d + off, a, w);
            }
            // JSON's one number type is a double, so every numeric primitive
            // widens to `F64` — the same narrowing `$json_of` does with
            // `Number(v)`, and lossy in the same place.
            JsonArm::Num => {
                let scratch = st.scratch;
                self.convert(st, prim, Prim::F64, a, scratch);
                self.store_w(d + off, scratch, 8);
            }
            JsonArm::Str => match prim {
                Prim::Str | Prim::Template => {
                    self.rc(prog, code, st, arg, Op::Retain);
                    let w = self.width_of(prog, code.ty_of(arg));
                    self.mv(d + off, a, w);
                }
                // A `Char` is a one-scalar string, and the runtime already
                // builds it: `buri_rt_char_to_str` is the unquoted half of
                // `show_prim`'s `Char` arm, and the block it answers is fresh,
                // so this arm needs no retain.
                _ => {
                    self.c_call(
                        runtime::CHAR_TO_STR,
                        st,
                        &[Src::Word(a), Src::Addr(d + off)],
                        &[],
                        d + off,
                        "v",
                    );
                }
            },
        }
    }

    /// A **boxed** field: `middle::layout` puts a pointer where the field would
    /// be, for the field that would otherwise make the owner recursive
    /// (`Layouts::boxes`). So writing one is an allocation, a copy and a store
    /// of the pointer — the shape a boxed `counts::Field` releases.
    fn box_into(&mut self, st: &mut Fn2, dest: u32, src: u32, size: u32) {
        let (ptr, one) = (st.scratch, st.scratch + 8);
        self.imm_to(one, 1);
        self.emit(
            "elemalloc",
            &[
                Lit::D.of(u64::from(ptr)),
                Lit::A.of(u64::from(one)),
                Lit::P.of(u64::from(size)),
                Jump::Cont0.to(V::Fall),
            ],
        );
        self.imm_to(one, 0);
        self.elem_store(src, ptr, one, 8, size);
        self.mv(dest, ptr, 8);
    }

    /// Reading one back: the field holds the block's pointer, and the value is
    /// the bytes it names.
    fn unbox_from(&mut self, st: &mut Fn2, dest: u32, at: u32, size: u32) {
        let zero = st.scratch + 8;
        self.imm_to(zero, 0);
        self.elem_load(dest, at, zero, 8, size);
    }

    pub(crate) fn imm_w(&mut self, dst: u32, w: u32, v: u64) {
        match w {
            1 | 2 | 4 => self.emit(
                match w {
                    1 => "immw/1",
                    2 => "immw/2",
                    _ => "immw/4",
                },
                &[Lit::D.of(dst as u64), Lit::N.of(v), Jump::Cont.to(V::Fall)],
            ),
            _ => self.imm_to(dst, v),
        }
    }

    fn get_tag(
        &mut self,
        prog: &ir::Program,
        code: &ir::Code,
        st: &mut Fn2,
        dest: ir::ValueId,
        agg: ir::ValueId,
    ) {
        let l = self.enum_layout(prog, enum_ty(code.ty_of(agg)));
        let d = st.at(dest);
        let a = st.at(agg);
        let (repr, variants) = (&l.repr, &l.variants);
        match repr {
            EnumRepr::Bare { tag } | EnumRepr::Tagged { tag, .. } => self.load_w(d, a, tag.size()),
            EnumRepr::Niche { null_at } => {
                let null_variant = variants.iter().position(|v| v.is_empty()).unwrap_or(1) as u64;
                let other = u64::from(null_variant == 0);
                self.emit(
                    "niche_tag",
                    &[
                        Lit::D.of(d as u64),
                        Lit::A.of((a + null_at) as u64),
                        Lit::N.of(null_variant),
                        Lit::P.of(other),
                        Jump::Cont.to(V::Fall),
                    ],
                );
            }
        }
    }

    /// `Inst::IncRef` and `Inst::DecRef`: MEMORY.md §5.1's saturating increment
    /// and its decrement, over **every** counted block the value owns.
    ///
    /// A missing release is a leak, and a leak that compiles is a wrong program
    /// that passes its tests: `cli/tests/native/runtime.rs` holds the toolchain
    /// to "every allocation is freed at exit".
    fn rc(
        &mut self,
        prog: &ir::Program,
        code: &ir::Code,
        st: &mut Fn2,
        value: ir::ValueId,
        op: Op,
    ) {
        // A scalar has no counts.
        let ir::Type::Agg(agg) = code.ty_of(value) else {
            return;
        };
        let id = agg.id();
        // Borrowed, not cloned: a `Ty` is a tree and this is asked once per
        // reference operation in the program, which is the most common
        // instruction `middle::rc` emits.
        let ty = &prog.type_info(id).ty;
        let at = st.at(value);
        // A heavy walk goes through the type's glue, so that a function
        // holding `n` reference operations on an enum of `n` counted variants
        // is not `n²` tests (PERFORMANCE.md §6.32).
        if self.rc_weight(ty) > RC_OUT_OF_LINE {
            self.walk_out_of_line(st, *ty, at, op);
        } else {
            self.walk_rc(st, ty, at, op, 0);
        }
    }

    /// `op` over a value's counted blocks through its type's glue,
    /// `fn(*mut u8)`, handed the value's address.
    fn walk_out_of_line(&mut self, st: &mut Fn2, ty: Ty, at: u32, op: Op) {
        let sym = self.helper(Helper::Walk { ty, op });
        let addr = st.scratch + (super::rtcall::RAW_WORD + 3) * 8;
        self.emit(
            "lea",
            &[
                Lit::D.of(u64::from(addr)),
                Lit::A.of(u64::from(at)),
                Jump::Cont.to(V::Fall),
            ],
        );
        self.c_call_sym(sym, st, &[Src::Word(addr)]);
    }

    /// `op` over one value's counted blocks, at frame offset `at`
    /// (`backend/counts.rs`).
    ///
    /// [`Op::Copy`] replaces each block with `buri_rt_copy_block`'s answer and
    /// stores it back where the pointer was. Nothing in a copy increments a
    /// count: *a copy is not a share*.
    ///
    /// `depth` is how far into the walk this is, and never passes
    /// [`RC_DEPTH`]: a recursive type reaches itself through a **box**, which is
    /// a leaf here, and a deep non-recursive one goes out of line into its own
    /// glue.
    pub(crate) fn walk_rc(&mut self, st: &mut Fn2, ty: &Ty, at: u32, op: Op, depth: u32) {
        let sites = self.rc_sites(ty);
        for site in sites.iter() {
            match site {
                Site::Block { offset, glue } => self.block_op(st, at + offset, glue, op),
                Site::Field(f) => self.walk_field(st, f, at, op, depth),
                // One arm per variant that owns something, dispatched on the
                // tag the same way a `match` is.
                //
                // The tag is read in place by the fused `tagbr` test a `match`
                // uses, where its width has one: loading it into a scratch slot
                // and comparing that was a load, a store and a reload per arm,
                // and an enum's walk runs on every retain and release of it.
                Site::Tagged { tag, arms } => {
                    let done = st.label();
                    let fused = match tag.size() {
                        1 => Some("tagbr/eq8"),
                        4 => Some("tagbr/eq32"),
                        8 => Some("tagbr/eq"),
                        _ => None,
                    }
                    .filter(|k| self.has(k));
                    for (i, arm) in arms.iter().enumerate() {
                        let next = st.label();
                        if let Some(base) = fused {
                            let key = self.arm_key(base, Jump::T);
                            self.emit(
                                key,
                                &[
                                    Lit::A.of(u64::from(at)),
                                    Lit::N.of(u64::from(arm.variant)),
                                    Jump::T.to(V::Fall),
                                    Jump::F.to(V::Blk(next)),
                                ],
                            );
                        } else {
                            let scr = st.scratch + super::rtcall::SPARE_WORD * 8;
                            self.load_w(scr, at, tag.size());
                            let key = self.arm_key("brcmp/eq/u64/fi", Jump::T);
                            self.emit(
                                key,
                                &[
                                    Lit::A.of(u64::from(scr)),
                                    Word::K.lit(u64::from(arm.variant)),
                                    Jump::T.to(V::Fall),
                                    Jump::F.to(V::Blk(next)),
                                ],
                            );
                        }
                        for f in arm.fields.iter() {
                            self.walk_field(st, f, at, op, depth);
                        }
                        // The last arm's `next` is `done`, so it falls there.
                        if i + 1 < arms.len() {
                            self.emit("jump", &[Jump::T.to(V::Blk(done))]);
                        }
                        let here = self.region.code_addr();
                        st.place(next, here);
                    }
                    let here = self.region.code_addr();
                    st.place(done, here);
                }
                // A niche `Option<T>`: `.None` is written by storing null at
                // `null_at` and nothing else (`Lower::store_disc`,
                // `rtcall::store_option_tag`), so the rest of the payload is
                // whatever the frame last held and the walk is behind the test.
                Site::Guarded { null_at, ty } => {
                    let skip = st.label();
                    let key = self.arm_key("brcmp/eq/u64/fi", Jump::T);
                    self.emit(
                        key,
                        &[
                            Lit::A.of(u64::from(at + null_at)),
                            Word::K.lit(0),
                            Jump::T.to(V::Blk(skip)),
                            Jump::F.to(V::Fall),
                        ],
                    );
                    self.walk_rc(st, ty, at, op, depth + 1);
                    let here = self.region.code_addr();
                    st.place(skip, here);
                }
            }
        }
    }

    /// One field with counts in it, inside a value at `at`.
    ///
    /// A **boxed** field is the block's pointer, so it is one operation and no
    /// descent: whatever is inside is the block's own glue's business.
    ///
    /// A compound field deep in the walk goes through its own glue rather than
    /// inline, at **every** level. A type graph is a DAG whose nodes are
    /// revisited along every path, so an inline walk of a record of records
    /// expands once per path rather than once per type
    /// (`conformance/lib/semantics/test/generics.buri`). Going through here, an
    /// emitted body holds at most [`RC_INLINE`] levels plus one call per deeper
    /// field. A `Str`, a `[T]` and a closure never go out of line: the call
    /// would cost more than the instruction it replaced.
    pub(crate) fn walk_field(&mut self, st: &mut Fn2, f: &Field, at: u32, op: Op, depth: u32) {
        let at = at + f.offset;
        if f.boxed {
            let glue = (op != Op::Retain && self.rc_counted(&f.ty))
                .then(|| self.helper(Helper::Walk { ty: f.ty, op }));
            return self.count_block(st, at, op, glue);
        }
        if self.field_out_of_line(&f.ty, depth) {
            return self.walk_out_of_line(st, f.ty, at, op);
        }
        self.walk_rc(st, &f.ty, at, op, depth + 1)
    }

    /// Whether an unboxed field of `ty`, `depth` levels into a walk, goes
    /// through its type's glue rather than inline ([`Jit::walk_field`]).
    pub(crate) fn field_out_of_line(&mut self, ty: &Ty, depth: u32) -> bool {
        let compound = matches!(
            self.layout_shared(ty).repr,
            Repr::Aggregate | Repr::Enum { .. }
        );
        // A light field stays inline however deep, until the walk nears
        // [`RC_DEPTH`]; the glue starts its own count from zero.
        let light = depth + 2 < RC_DEPTH && self.rc_weight(ty) <= RC_LIGHT;
        compound && depth >= RC_INLINE && !light
    }

    /// `op` on one nullable block pointer, with the glue its contents need.
    pub(crate) fn block_op(&mut self, st: &mut Fn2, at: u32, glue: &Glue, op: Op) {
        let glue = match glue {
            // A `Str` is `{ base, ptr, len }` and `ptr` points *into* `base`,
            // so a copy rebases as well as replaces (`buri_rt_copy_str`).
            Glue::Str if op == Op::Copy => {
                self.c_call(runtime::COPY_STR, st, &[Src::Addr(at)], &[], 0, "v");
                return;
            }
            Glue::Str => None,
            // A closure's environment carries its own release in its first
            // word and its copy in its second (`glue.rs`), because `Ty::Fn`
            // does not record what was captured.
            Glue::Env => (op != Op::Retain).then(|| self.helper(Helper::Env { op })),
            Glue::Elems(elem) => (op != Op::Retain && self.rc_counted(elem))
                .then(|| self.helper(Helper::Elems { ty: *elem, op })),
        };
        self.count_block(st, at, op, glue)
    }

    /// `op` on the block whose pointer is at frame offset `at`.
    ///
    /// `glue` releases or copies the block's *contents*, and is `None` for a
    /// block of bytes — a `Str`'s allocation, a `[Int]`. A retain never needs
    /// one. A null pointer copies to a null pointer, so a niche and a static
    /// `Str`'s absent base need no test.
    fn count_block(&mut self, st: &mut Fn2, at: u32, op: Op, glue: Option<String>) {
        match (op, glue) {
            (Op::Retain, _) => self.incref(at),
            (Op::Release, Some(g)) => {
                let (key, mask) = self.rc_key("decref/drop", "decref/drop/shared");
                self.emit(
                    key,
                    &[
                        Lit::A.of(u64::from(at)),
                        Jump::Callee.to(V::Sym(g)),
                        mask,
                        Jump::Cont0.to(V::Fall),
                    ],
                )
            }
            (Op::Release, None) => {
                let (key, mask) = self.rc_key("decref/free", "decref/free/shared");
                self.emit(
                    key,
                    &[Lit::A.of(u64::from(at)), mask, Jump::Cont0.to(V::Fall)],
                )
            }
            (Op::Copy, glue) => {
                let g = glue.map_or(Src::Imm(0), Src::Sym);
                self.c_call(runtime::COPY_BLOCK, st, &[Src::Word(at), g], &[], at, "i");
            }
        }
    }

    /// A retain of the block whose pointer is at frame offset `at`.
    pub(crate) fn incref(&mut self, at: u32) {
        let (key, mask) = self.rc_key("incref", "incref/shared");
        self.emit(
            key,
            &[Lit::A.of(u64::from(at)), mask, Jump::Cont.to(V::Fall)],
        );
    }

    /// The count stencil to use, and the binding for its mask hole.
    ///
    /// A program that can fan out ([`Jit::shares`]) ORs `buri_rt_shared_mask`
    /// into the bit its fork tests, so a block allocated before the first
    /// fan-out is counted atomically after it (buri-lang/buri#243). Any other
    /// program keeps the plain stencil; the plain one has no `R` hole, so the
    /// binding is never read.
    fn rc_key(&self, plain: &'static str, shared: &'static str) -> (&'static str, Bind) {
        if self.shares {
            (shared, Word::R.ext(runtime::SHARED_MASK))
        } else {
            (plain, Word::R.lit(0))
        }
    }

    /// A comparison of two `Str`s, through one helper. The six orderings are
    /// one three-way compare and a test of its answer, so the helper takes the
    /// operator rather than there being six of them.
    fn str_binary(
        &mut self,
        st: &mut Fn2,
        dest: ir::ValueId,
        op: BinOp,
        lhs: ir::ValueId,
        rhs: ir::ValueId,
    ) {
        let d = st.at(dest);
        let (a, b) = (st.at(lhs), st.at(rhs));
        let raw = st.scratch + super::rtcall::RAW_WORD * 8;
        // `buri_rt_str_equal` answers the equality directly; every ordering is a
        // test of `buri_rt_str_compare`'s three-way answer, whose variants are
        // `Less`, `Equal`, `Greater` in that order (`core/order`).
        let (symbol, want): (&'static str, &[u64]) = match op {
            BinOp::Eq => ("buri_rt_str_equal", &[]),
            BinOp::Ne => ("buri_rt_str_equal", &[]),
            BinOp::Lt => ("buri_rt_str_compare", &[LESS]),
            BinOp::Le => ("buri_rt_str_compare", &[LESS, EQUAL]),
            BinOp::Gt => ("buri_rt_str_compare", &[GREATER]),
            BinOp::Ge => ("buri_rt_str_compare", &[GREATER, EQUAL]),
            other => crate::diagnostics::ice(&format!("stencil: Binary {other:?} at Str")),
        };
        self.str_compare(st, symbol, a, b, raw);
        self.order_test(st, raw, d, op, want);
    }

    // -- arithmetic --------------------------------------------------------

    fn binary(
        &mut self,
        st: &mut Fn2,
        dest: ir::ValueId,
        op: BinOp,
        prim: Prim,
        lhs: ir::ValueId,
        rhs: ir::ValueId,
    ) {
        // `Str` has no arithmetic and no `bin/*` stencil: a comparison of two
        // is a length-and-bytes question the runtime answers, which is what
        // `buri_rt_str_equal` and `buri_rt_str_compare` are on the native side.
        if matches!(prim, Prim::Str | Prim::Template) {
            return self.str_binary(st, dest, op, lhs, rhs);
        }
        let (tag, _, _) = num_tag(prim);
        let name = op.name();
        let d = st.at(dest);
        let a = st.at(lhs);
        let b = st.at(rhs);
        // The paper's stencil configuration: which of constant, register and
        // stack each operand and the result is.
        let la = st.loc(lhs);
        // A `Const` every use of which is an immediate lives at `Loc::Imm`.
        let lb = st.loc(rhs);
        let ld = st.home(dest);
        let key = key![
            "bin/",
            name,
            "/",
            tag,
            "/",
            la.tag(),
            lb.tag(),
            "/",
            ld.tag()
        ];
        // `sources.rs` generates every operation at every type and location
        // the IR can ask for, so the key is always in the library.
        let k = constant(st, rhs);
        self.emit(
            &key,
            &[
                Lit::A.of(a as u64),
                Lit::B.of(b as u64),
                Lit::D.of(d as u64),
                Word::K.lit(k),
                Jump::Cont.to(V::Fall),
                // A 128-bit divide is a *call* — the runtime owns it
                // (`sources.rs::wide`) — so its stencil has the
                // zero-register prototype and names its continuation
                // `JIT_CONT0`. Binding both costs nothing: `emit` looks a
                // hole up by name and a stencil that has neither ignores
                // the other.
                Jump::Cont0.to(V::Fall),
            ],
        );
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "one instruction's operands, as `Lower::binary` above"
    )]
    fn unary(
        &mut self,
        prog: &ir::Program,
        code: &ir::Code,
        st: &mut Fn2,
        dest: ir::ValueId,
        op: UnOp,
        prim: Prim,
        arg: ir::ValueId,
    ) {
        let d = st.at(dest);
        let a = st.at(arg);
        // A `Bool`'s frame word is already the whole zero-extended integer, so
        // the conversion is the copy. `Jit::regalloc` leaves both in the frame.
        if op == UnOp::FromBool {
            return self.mv(d, a, 8);
        }
        if op == UnOp::Not {
            // The operand and the result may be in CPS registers, exactly as
            // for any other unary operation. Spelling this `f/f` was a real
            // miscompile: the stencil wrote the frame while the consumer read
            // the register.
            let key = key!["un/lnot/b/", st.loc(arg).tag(), "/", st.home(dest).tag()];
            return self.emit(
                &key,
                &[
                    Lit::A.of(a as u64),
                    Lit::D.of(d as u64),
                    Jump::Cont.to(V::Fall),
                ],
            );
        }
        let (tag, _, _) = num_tag(prim);
        let name = if op == UnOp::Neg { "neg" } else { "bnot" };
        let key = key![
            "un/",
            name,
            "/",
            tag,
            "/",
            st.loc(arg).tag(),
            "/",
            st.home(dest).tag()
        ];
        self.emit(
            &key,
            &[
                Lit::A.of(a as u64),
                Lit::D.of(d as u64),
                Jump::Cont.to(V::Fall),
            ],
        );
        let _ = (prog, code);
    }

    // -- calls -------------------------------------------------------------

    fn call(
        &mut self,
        prog: &ir::Program,
        code: &ir::Code,
        st: &mut Fn2,
        dest: ir::ValueId,
        func: FuncIdx,
        args: &[ir::ValueId],
    ) {
        if let ir::Body::Runtime(id) = &prog.funcs.get(func).body {
            // The archive boundary, **emitted here rather than called**, which
            // is every backend's first act at a `Body::Runtime` call.
            //
            // Called, `a.get(i)` costs two frames: the caller copies the `[T]`
            // and the index into the callee's parameter slots and branches; the
            // generated `core/list$get` body then copies the same words again
            // into its C argument area and branches into `libburi_rt.a`. The
            // second copy is the whole of the difference — the marshalling the
            // generated body does is exactly the marshalling the caller could
            // have done, from the operands it already has in its own frame.
            // Measured on a matrix multiply whose inner loop is two `list.get`
            // per element, that was ~40 instructions against the incumbent's
            // ten, thirty-two million times.
            //
            // Sound because the *shape* of a marshalled call is a function of
            // the key and of the operand and result IR types alone, and those
            // are the same at the two sites: the `Body::Runtime` function's
            // signature **is** the caller's argument and destination types.
            // `rt_call` is the one implementation of `cli/runtime/lib.rs` §2's
            // rule, and `runtime_body` hands it the same list from the callee's
            // parameter offsets — so the two sites marshal alike.
            //
            // A key with no row in `runtime.rs`'s table is `str.length`,
            // `number.<T>.<op>` and the rest, whose bodies `runtime_body`
            // generates from the signature; those are called.
            if let route::Body::Row(entry) = *self.routes.bodies.get(*id) {
                let list: Vec<(u32, ir::Type)> =
                    args.iter().map(|a| (st.at(*a), code.ty_of(*a))).collect();
                let dest = (st.at(dest), code.ty_of(dest));
                self.rt_call(prog, st, entry, dest, &list, Vec::new());
                return;
            }
        }
        let base = st.frame.size;
        let callee = self.frame_sig_of(func);
        for (a, off) in args.iter().zip(&callee.params) {
            // A zero-sized argument — a lifted lambda's empty environment —
            // has no bytes for the callee to read.
            if self.width_of(prog, code.ty_of(*a)) == 0 {
                continue;
            }
            let n = self.slot_bytes_of(prog, code.ty_of(*a));
            self.mv(base + *off, st.at(*a), n);
        }
        self.emit(
            "call",
            &[
                Lit::N.of(base as u64),
                Lit::P.of(base as u64),
                Jump::Callee.to(V::Fn(func)),
                Jump::Cont0.to(V::Fall),
            ],
        );
        let n = self.slot_bytes_of(prog, code.ty_of(dest));
        self.mv(st.at(dest), base + RET, n);
    }

    fn call_indirect(
        &mut self,
        prog: &ir::Program,
        code: &ir::Code,
        st: &mut Fn2,
        dest: ir::ValueId,
        callee: ir::ValueId,
        args: &[ir::ValueId],
    ) {
        // The environment is prepended to the argument list as a pointer, so
        // the callee's frame is laid out for `[Ptr] ++ args`.
        let base = st.frame.size;
        let ret_n = self.slot_bytes_of(prog, code.ty_of(dest));
        let mut at = RET + ret_n;
        let env_at = at;
        at += 8;
        let mut params = Vec::new();
        for a in args {
            params.push(at);
            at += self.slot_bytes_of(prog, code.ty_of(*a));
        }
        let c = st.at(callee);
        self.mv(base + env_at, c + 8, 8);
        // `params` was pushed one entry per argument just above, so zipping it
        // back is the same pairing an index would give.
        for (a, off) in args.iter().zip(&params) {
            let n = self.slot_bytes_of(prog, code.ty_of(*a));
            self.mv(base + *off, st.at(*a), n);
        }
        self.emit(
            "calli",
            &[
                Lit::A.of(c as u64),
                Lit::N.of(base as u64),
                Lit::P.of(base as u64),
                Jump::Cont0.to(V::Fall),
            ],
        );
        self.mv(st.at(dest), base + RET, ret_n);
    }

    /// One `Inst::CallIntrinsic`. `middle` emits one for the keys it writes
    /// itself — a template's `str.concat`, a derived leaf, the test runner's
    /// hooks and a failed assertion's shown report — and every other key reaches
    /// here as a call to its `Body::Runtime` function instead.
    fn intrinsic(
        &mut self,
        prog: &ir::Program,
        code: &ir::Code,
        st: &mut Fn2,
        dest: ir::ValueId,
        route: route::Call,
        args: &[ir::ValueId],
    ) {
        let scr = st.scratch;
        let d = st.at(dest);
        let arg = |st: &Fn2, i: usize| -> u32 { args.get(i).map(|v| st.at(*v)).unwrap_or(0) };
        match route {
            // A failed assertion is an abort, and the kind is what makes it
            // attributable (`cli/runtime/testing.rs`). Both are the shape
            // `llvm/emit.rs` emits, symbol for symbol, because what `buri test`
            // parses out of the process's output is the runtime's writing and
            // not the backend's.
            //
            // `failExpected<T, R>(kind, got): R` answers the bottom type: the
            // call does not come back, and the destination the IR gives it is
            // unreachable.
            route::Call::FailExpectedShown => {
                let (kp, kl) = self.str_arg(arg(st, 0), scr);
                let (vp, vl) = self.str_arg(arg(st, 1), scr + 8);
                self.c_call(
                    runtime::TEST_FAIL_EXPECTED,
                    st,
                    &[kp, kl, vp, vl],
                    &[],
                    0,
                    "v",
                );
            }
            route::Call::ReportShown => {
                let (kp, kl) = self.str_arg(arg(st, 0), scr);
                let (ap, al) = self.str_arg(arg(st, 1), scr + 8);
                let (ep, el) = self.str_arg(arg(st, 2), scr + 16);
                self.c_call(
                    runtime::TEST_FAIL_COMPARED,
                    st,
                    &[kp, kl, ap, al, ep, el],
                    &[],
                    0,
                    "v",
                );
            }
            // `str.concat` is a runtime call with no table row, so its
            // arguments are narrowed here rather than flattened: the two
            // `Str`s and nothing else (`rtcall.rs`'s `str_concat`).
            route::Call::Concat => {
                let strs = Self::concat_strs(args.len()).map(|i| arg(st, i));
                self.str_concat(st, strs, d);
            }
            // `middle::derives` names each leaf by its primitive's type.
            route::Call::Derive(Leaf::Hash, prim) => {
                self.hash_prim(st, prim, arg(st, 0), arg(st, 1), d);
            }
            route::Call::Derive(Leaf::Show, prim) => self.show_prim(st, prim, arg(st, 0), d, true),
            route::Call::Derive(Leaf::Json, prim) => {
                let value = args.first().copied().unwrap_or(dest);
                self.json_prim(prog, code, st, prim, value, dest)
            }
            // `middle` writes every other key itself, and each has a row.
            route::Call::Row(entry) => {
                let list: Vec<(u32, ir::Type)> =
                    args.iter().map(|a| (st.at(*a), code.ty_of(*a))).collect();
                self.rt_call(prog, st, entry, (d, code.ty_of(dest)), &list, Vec::new());
            }
        }
    }

    // -- terminators -------------------------------------------------------

    pub(crate) fn term(
        &mut self,
        prog: &ir::Program,
        code: &ir::Code,
        st: &mut Fn2,
        next: usize,
        t: &Term,
        plan: &Plan,
    ) {
        match t {
            Term::Jump(x) => {
                let fall = x.block.index() == next;
                if let Some(b) = plan.incbr {
                    let out = if b.out.index() == next {
                        V::Fall
                    } else {
                        V::Blk(b.out.index() as u32)
                    };
                    let key = self.arm_key("incbr/lt", Jump::F);
                    self.emit(
                        key,
                        &[
                            Lit::D.of(u64::from(st.at(b.into))),
                            Lit::A.of(u64::from(st.at(b.from))),
                            Lit::N.of(b.by),
                            Lit::B.of(u64::from(st.at(b.bound))),
                            Jump::T.to(V::Blk(b.back.index() as u32)),
                            Jump::F.to(out),
                        ],
                    );
                    return;
                }
                if !fall && self.jump_into_test(code, st, x) {
                    return;
                }
                if self.jump_fused(prog, code, st, x, fall) {
                    return;
                }
                self.edge(prog, code, st, x);
                if fall {
                    self.emit("jump", &[Jump::T.to(V::Fall)]);
                } else {
                    self.emit("jump", &[Jump::T.to(V::Blk(x.block.index() as u32))]);
                }
            }
            Term::Branch { cond, then, else_ } => {
                // (g) Branch mechanics. Two instructions are on the table: the
                // arm that falls through should be the stencil's elidable one,
                // and the second arm's `jump` should fall through to the block
                // laid out next.
                //
                // One arm is always next. `middle::lower` gives every branch a
                // target only that branch enters, and the layout's depth-first
                // walk reaches it from the branch and places one of the two
                // targets right after it.
                if then.args.is_empty() && else_.args.is_empty() {
                    let ft = then.block.index() == next;
                    let fe = else_.block.index() == next;
                    let tv = if ft {
                        V::Fall
                    } else {
                        V::Blk(then.block.index() as u32)
                    };
                    let fv = if fe {
                        V::Fall
                    } else {
                        V::Blk(else_.block.index() as u32)
                    };
                    let fall = if ft { Jump::T } else { Jump::F };
                    self.cond(st, *cond, plan, tv, fv, fall);
                    return;
                }
                // With edge copies, one arm's copies fall through and the
                // other's sit behind a label; the label goes to the arm whose
                // own target is the next block, so that its jump goes too.
                let y_is_then = then.block.index() == next;
                let (x, y) = if y_is_then {
                    (else_, then)
                } else {
                    (then, else_)
                };
                let lf = st.label();
                let (tv, fv) = if y_is_then {
                    (V::Blk(lf), V::Fall)
                } else {
                    (V::Fall, V::Blk(lf))
                };
                let fall = if y_is_then { Jump::F } else { Jump::T };
                self.cond(st, *cond, plan, tv, fv, fall);
                self.edge(prog, code, st, x);
                self.emit("jump", &[Jump::T.to(V::Blk(x.block.index() as u32))]);
                st.place(lf, self.region.code_addr());
                // `y` is the arm laid out next.
                self.edge(prog, code, st, y);
                self.emit("jump", &[Jump::T.to(V::Fall)]);
            }
            Term::Switch { on, cases, default } => {
                // What the comparison reads: the discriminant's frame slot, or
                // — when the level has the supernode — the enum's tag field
                // directly, so that `GetTag` never lands in a slot at all.
                // Where the comparison reads its discriminant, and whether the
                // fused `tagbr` stencil can read it in place. A niche-encoded
                // tag is a *test* rather than a field, so it is materialised
                // into scratch and the fusion does not apply.
                let mut src = st.at(*on);
                let mut fusedw: Option<&'static str> = None;
                if let Some(agg) = plan.tagsw {
                    let (off, w, niche) = self.tag_field(prog, code, st, agg);
                    let s0 = st.scratch + 24;
                    if niche {
                        self.emit(
                            "niche_tag",
                            &[
                                Lit::D.of(s0 as u64),
                                Lit::A.of(off as u64),
                                Lit::N.of(w as u64),
                                Lit::P.of(u64::from(w == 0)),
                                Jump::Cont.to(V::Fall),
                            ],
                        );
                        src = s0;
                    } else {
                        match w {
                            1 => fusedw = Some("tagbr/eq8"),
                            4 => fusedw = Some("tagbr/eq32"),
                            8 => fusedw = Some("tagbr/eq"),
                            _ => {}
                        }
                        match fusedw {
                            Some(_) => src = off,
                            None => {
                                self.load_w(s0, off, w);
                                src = s0;
                            }
                        }
                    }
                }
                // (l) The total chain, ported from the removed backend's
                // `compare_chain`.
                // `middle::exhaustiveness` has proved the match total and this
                // IR hands every `Switch` over with `default: None`, so once
                // every other arm has been refused the tag *is* the last one:
                // its test, and the `unreachable` behind the chain, are both
                // dead. On the two-arm `Option` match that is 58% of the
                // corpus's switches this halves the dispatch.
                let total = default.is_none() && cases.len() > 1;
                let last = cases.len() - 1;
                for (ci, (k, tgt)) in cases.iter().enumerate() {
                    // The last case is laid out next: every case is a block
                    // only this switch enters, and the layout's depth-first
                    // walk visits them in order.
                    if total && ci == last {
                        self.edge(prog, code, st, tgt);
                        self.emit("jump", &[Jump::T.to(V::Fall)]);
                        return;
                    }
                    let lnext = st.label();
                    // (g) The case body falls through, so the test's `JIT_T`
                    // arm is the one that has to be the elidable one.
                    let key = self.arm_key(fusedw.unwrap_or("brcmp/eq/u64/fi"), Jump::T);
                    // (2) The census says `tagbr + jump` is the commonest
                    // adjacent pair in the corpus after the constant
                    // stores: every `match` arm is a tag test followed by a
                    // jump to the arm's block. A case's edge carries no copies
                    // (`middle::lower` gives every case a block of its own with
                    // no parameters), so the test names the block itself.
                    let mut binds: Vec<Bind> = vec![
                        Lit::A.of(src as u64),
                        Jump::T.to(V::Blk(tgt.block.index() as u32)),
                        Jump::F.to(V::Blk(lnext)),
                    ];
                    if fusedw.is_some() {
                        binds.push(Lit::N.of(*k));
                    } else {
                        binds.push(Word::K.lit(*k));
                    }
                    self.emit(key, &binds);
                    st.place(lnext, self.region.code_addr());
                }
                match default {
                    Some(tgt) => {
                        self.edge(prog, code, st, tgt);
                        self.emit("jump", &[Jump::T.to(V::Blk(tgt.block.index() as u32))]);
                    }
                    None => self.emit("unreachable", &[]),
                }
            }
            Term::Return(v) => {
                let n = self.slot_bytes_of(prog, code.ty_of(*v));
                self.mv(RET, st.at(*v), n);
                self.emit("ret", &[]);
            }
            Term::Unreachable => self.emit("unreachable", &[]),
        }
    }

    /// A branch on a condition: either the `br` stencil reading a materialised
    /// boolean, or — from [`Level::CmpBr`] — the comparison and the branch as
    /// one stencil, which is the paper's Figure 11b supernode.
    /// The stencil key this terminator's test wants.
    fn cond_key(&self, st: &Fn2, cond: ir::ValueId, plan: &Plan) -> Option<super::jit::Key> {
        if let Some((op, prim, lhs, rhs)) = plan.cmpbr {
            let (tag, _, _) = prim_tag(prim)?;
            let la = st.loc(lhs).tag();
            let lb = st.loc(rhs).tag();
            let k = key!["brcmp/", op.name(), "/", tag, "/", la, lb];
            return self.has(&k).then_some(k);
        }
        let k = key!["br/", st.loc(cond).tag()];
        self.has(&k).then_some(k)
    }

    /// The twin of a two-target stencil whose **last** branch is the arm named
    /// by `fall` — the only arm copy-and-patch can elide. See
    /// `extract::swap_arms` for why this is a choice the library has to offer
    /// rather than one the emitter can make by negating the test.
    ///
    /// Answered as an index into the library, which `emit` then copies
    /// without looking the name up again: this is asked once per conditional
    /// branch, and the twin is found by index rather than by building its
    /// name.
    pub(crate) fn arm_key<'k>(&mut self, base: impl Into<KeyArg<'k>>, fall: Jump) -> KeyArg<'k> {
        self.arm_key_of(base.into(), fall)
    }

    /// [`Jit::arm_key`], once for every spelling of a key.
    fn arm_key_of<'k>(&mut self, base: KeyArg<'k>, fall: Jump) -> KeyArg<'k> {
        let at = self.index_of(base).or_ice("an arm key names a stencil");
        if self.elidable_at(at) == Some(fall) {
            return KeyArg::At(at);
        }
        // The twin has the arms the other way round, so its last branch is
        // the other hole: the one this base does not end in.
        match self.lib.swap_twin(at) {
            Some((sw, _)) => KeyArg::At(sw),
            None => KeyArg::At(at),
        }
    }

    fn cond(&mut self, st: &Fn2, cond: ir::ValueId, plan: &Plan, tv: V, fv: V, fall: Jump) {
        // `rtcall.rs` names `br/f` without asking. A library without
        // a branch stencil is broken, so this stops here rather than emitting
        // a fall-through that would run the wrong arm.
        let key = self
            .cond_key(st, cond, plan)
            .or_ice("stencil: the level has a branch stencil");
        let key = self.arm_key(&key, fall);
        if let Some((_, _, lhs, rhs)) = plan.cmpbr {
            let k = constant(st, rhs);
            self.emit(
                key,
                &[
                    Lit::A.of(st.at(lhs) as u64),
                    Lit::B.of(st.at(rhs) as u64),
                    Word::K.lit(k),
                    Jump::T.to(tv),
                    Jump::F.to(fv),
                ],
            );
            return;
        }
        self.emit(
            key,
            &[
                Lit::A.of(st.at(cond) as u64),
                Jump::T.to(tv),
                Jump::F.to(fv),
            ],
        );
    }

    /// `(byte offset, width-or-null-variant, is a niche)` for an enum's
    /// discriminant. For a niche the second field is the *variant index* the
    /// null pointer stands for, because there is no tag field to read.
    fn tag_field(
        &mut self,
        prog: &ir::Program,
        code: &ir::Code,
        st: &Fn2,
        agg: ir::ValueId,
    ) -> (u32, u32, bool) {
        let base = st.at(agg);
        let l = self.enum_layout(prog, enum_ty(code.ty_of(agg)));
        let (repr, variants) = (&l.repr, &l.variants);
        match repr {
            EnumRepr::Bare { tag } | EnumRepr::Tagged { tag, .. } => (base, tag.size(), false),
            EnumRepr::Niche { null_at } => (
                base + null_at,
                variants.iter().position(|v| v.is_empty()).unwrap_or(1) as u32,
                true,
            ),
        }
    }

    /// A jump to a block that is nothing but a fused compare-and-branch — a
    /// loop's header — takes that branch itself, so a back edge costs one
    /// conditional branch rather than a jump and then the test.
    ///
    /// Only where the edge copies nothing, which slot coalescing makes the
    /// common case for a loop's index, and where nothing the test reads lives
    /// in a register: the register half of an edge is [`Jit::edge`]'s, and a
    /// read here is from the jumping block's side of a promotion's region.
    fn jump_into_test(&mut self, code: &ir::Code, st: &mut Fn2, t: &Target) -> bool {
        let Some((header, cond, then, else_)) = copyless_test(code, st, t) else {
            return false;
        };
        let frame = |v: ir::ValueId| matches!(st.home(v), Loc::Frame);
        let plan = self.plan_block(code, st, header);
        let Some((_, _, lhs, rhs)) = plan.cmpbr else {
            return false;
        };
        if !frame(lhs) || !frame(rhs) {
            return false;
        }
        // The loop's way back is the `else` arm (`lower/lists.rs`), so the
        // `then` arm is the one left for the trailing jump.
        self.cond(
            st,
            cond,
            &plan,
            V::Blk(then.block.index() as u32),
            V::Blk(else_.block.index() as u32),
            Jump::T,
        );
        true
    }

    /// (e) The edge copies and the jump as one stencil. Every loop back edge in
    /// this IR is a run of moves followed by a jump, and at Base each move is
    /// its own stencil.
    fn jump_fused(
        &mut self,
        prog: &ir::Program,
        code: &ir::Code,
        st: &mut Fn2,
        t: &Target,
        fall: bool,
    ) -> bool {
        if fall {
            return false;
        }
        let params = code.get(t.block).params.clone();
        if params.iter().any(|p| matches!(st.home(*p), Loc::Reg(_))) {
            return false;
        }
        let mut pairs: Vec<(u32, u32)> = Vec::new();
        for (p, a) in params.iter().zip(t.args.iter()) {
            if self.slot_bytes_of(prog, code.ty_of(*p)) != 8 {
                return false;
            }
            let (d, s) = (st.at(*p), st.at(*a));
            if d != s {
                pairs.push((d, s));
            }
        }
        let n = pairs.len();
        if n == 0 || n > 4 {
            return false;
        }
        // `sources.rs` builds `movjump/1` to `movjump/4`.
        let key = key!["movjump/", n];
        let src = [Lit::A, Lit::B, Lit::C, Lit::E];
        let dst = [Lit::D, Lit::N, Lit::P, Lit::Q];
        let mut binds: Vec<Bind> = Vec::new();
        // `movjump/{n}` has `n` source holes and `n` destination holes, and
        // `n` is `pairs.len()`, which the test above holds to four — so the
        // zip runs out with the pairs and not with the hole names.
        for ((d, s), (sh, dh)) in pairs.iter().zip(src.into_iter().zip(dst)) {
            binds.push(sh.of(*s as u64));
            binds.push(dh.of(*d as u64));
        }
        binds.push(Jump::T.to(V::Blk(t.block.index() as u32)));
        self.emit(&key, &binds);
        true
    }

    /// The parallel copy an edge's block arguments are.
    ///
    /// (j) With cross-block promotion there are two halves: the CPS registers,
    /// which are done first because the frame half can overwrite the slots they
    /// read, and the frame slots, which a promoted parameter only takes part in
    /// when something that cannot read a register reads it.
    fn edge(&mut self, prog: &ir::Program, code: &ir::Code, st: &mut Fn2, t: &Target) {
        let params = code.get(t.block).params.clone();
        let mut pend: Vec<(u8, bool, RSrc)> = Vec::new();
        for (p, a) in params.iter().zip(t.args.iter()) {
            // The *home* of the target's parameter: the edge lands where the
            // parameter lives, and the block it lives in is the region's
            // header however far outside the region this edge starts.
            let Loc::Reg(k) = st.home(*p) else { continue };
            let src = match st.loc(*a) {
                Loc::Reg(j) if j == k => {
                    // The definition wrote the register in place; the frame
                    // slot still needs it when anything reads the slot.
                    if write_through(st, *p) {
                        self.emit(
                            &key![
                                if code.ty_of(*p) == ir::Type::F64 {
                                    "stwg"
                                } else {
                                    "stw"
                                },
                                "/r",
                                k
                            ],
                            &[Lit::D.of(st.at(*p) as u64), Jump::Cont.to(V::Fall)],
                        );
                    }
                    continue;
                }
                Loc::Reg(j) => RSrc::Reg(j),
                _ => RSrc::Slot(st.at(*a)),
            };
            pend.push((k, code.ty_of(*p) == ir::Type::F64, src));
        }
        // Integers and floats each number their registers from zero, so a
        // register is its number and its class.
        let mut tmp = st.scratch + 56;
        while let Some(&(_, f, src)) = pend.first() {
            let ready = pend
                .iter()
                .position(|(k, f, _)| !pend.iter().any(|(_, g, s)| g == f && *s == RSrc::Reg(*k)));
            match ready {
                Some(i) => {
                    let (k, f, src) = pend.remove(i);
                    self.reg_move(k, f, src);
                }
                None => {
                    // A permutation of the register file: break it through a
                    // scratch slot, exactly as the frame half does.
                    //
                    // `ready` is `None` only when every entry's register is
                    // some entry's source; the registers are distinct, so covering all of
                    // them takes as many register sources as there are entries
                    // and no entry's source can be a slot. A slot here would
                    // mean the search above missed a move that was ready, and
                    // emitting the permutation anyway would drop it.
                    let j = match src {
                        RSrc::Reg(j) => Some(j),
                        RSrc::Slot(_) => None,
                    }
                    .or_ice("stencil: an edge's register copies deadlock only on register sources");
                    self.emit(
                        &key![if f { "stwg" } else { "stw" }, "/r", j],
                        &[Lit::D.of(tmp as u64), Jump::Cont.to(V::Fall)],
                    );
                    for p in pend.iter_mut() {
                        if p.1 == f && p.2 == RSrc::Reg(j) {
                            p.2 = RSrc::Slot(tmp);
                        }
                    }
                    tmp += 8;
                }
            }
        }
        let mut pending: Vec<(u32, u32, u32)> = Vec::new();
        for (p, a) in params.iter().zip(t.args.iter()) {
            if let Loc::Reg(k) = st.home(*p) {
                // Written back above, or not written at all.
                if !write_through(st, *p) || st.loc(*a) == Loc::Reg(k) {
                    continue;
                }
            }
            let n = self.slot_bytes_of(prog, code.ty_of(*p));
            let (d, s) = (st.at(*p), st.at(*a));
            if d != s {
                pending.push((d, s, n));
            }
        }
        let mut tmp = st.scratch + 48;
        while let Some(&(_, s, n)) = pending.first() {
            match pending
                .iter()
                .position(|(d, _, _)| !pending.iter().any(|(_, s, _)| s == d))
            {
                Some(i) => {
                    let (d, s, n) = pending.remove(i);
                    self.mv(d, s, n);
                }
                None => {
                    // A cycle: break it through a scratch slot, at the first
                    // entry.
                    self.mv(tmp, s, n);
                    for p in pending.iter_mut() {
                        if p.1 == s {
                            p.1 = tmp;
                        }
                    }
                    tmp += 8;
                }
            }
        }
    }

    fn reg_move(&mut self, k: u8, float: bool, src: RSrc) {
        match src {
            RSrc::Reg(j) => self.emit(
                &key![if float { "mvg" } else { "mvr" }, "/r", k, "/r", j],
                &[Jump::Cont.to(V::Fall)],
            ),
            RSrc::Slot(o) => self.emit(
                &key![if float { "ldg" } else { "ld" }, "/r", k],
                &[Lit::A.of(o as u64), Jump::Cont.to(V::Fall)],
            ),
        }
    }

    // -- runtime-supplied bodies -------------------------------------------

    pub(crate) fn runtime_body(
        &mut self,
        prog: &ir::Program,
        fi: FuncIdx,
        route: route::Body<'_>,
        st: &mut Fn2,
    ) {
        let fs = self.frame_sig_of(fi);
        let ret0 = RET;
        let p = |i: usize| fs.params.get(i).copied().unwrap_or(0);

        match route {
            // `core/alloc`'s `copyOut(value)`: the value, and then every
            // counted block inside it, copied. The type is the signature's —
            // this function is one of `middle::monomorphize`'s instantiations,
            // so the erasure the key suffers is repaired by the *body* being
            // generated here, where the instantiation is known
            // (`GENERIC_INTRINSICS`).
            route::Body::CopyOut => {
                let arg = prog
                    .funcs
                    .get(fi)
                    .sig
                    .params
                    .first()
                    .copied()
                    .and_then(|t| source_ty(prog, t));
                match arg {
                    Some(ty) => {
                        let size = self.layouts_of(ty).size;
                        self.mv(ret0, p(0), size);
                        if self.rc_counted(&ty) {
                            self.walk_rc(st, &ty, ret0, Op::Copy, 0);
                        }
                    }
                    // A scalar or a unit: the move above is the whole copy,
                    // and there is no source type to walk.
                    None => {
                        let size = prog
                            .funcs
                            .get(fi)
                            .sig
                            .params
                            .first()
                            .map_or(0, |t| self.width_of(prog, *t));
                        self.mv(ret0, p(0), size);
                    }
                }
            }
            // `failExpected(kind, got)` where `got` has no `Show`: the abort
            // names the kind alone. Where it has one, `hand_written.rs` writes
            // a body that reaches `failExpectedShown` as an
            // `Inst::CallIntrinsic`, as `report`'s reaches `reportShown`.
            route::Body::FailExpected => {
                let (kp, kl) = self.str_arg(p(0), fs.param_end);
                self.c_call(runtime::ABORT_ASSERT, st, &[kp, kl], &[], 0, "v");
            }
            route::Body::FailWith => {
                let (kp, kl) = self.str_arg(p(0), fs.param_end);
                self.c_call(runtime::ABORT, st, &[kp, kl], &[], 0, "v");
            }
            // `number.<T>.<op>`, which the native backends open-code.
            route::Body::Number(prim, op) => self.number_body(prog, fi, st, prim, op),
            route::Body::Row(entry) => self.runtime_intrinsic(prog, fi, entry, fs, st),
            // `str.length` is the number of Unicode scalar values
            // (VALUE-MODEL.md §3.1), which `cli/runtime/text.rs` answers for
            // both the ASCII and the multibyte case. Not in the table because
            // it has no `Ret` shape of its own: the count comes straight back
            // in a register.
            route::Body::StrLength => {
                let (sp, sl) = self.str_arg(p(0), fs.param_end);
                self.c_call(runtime::STR_SCALAR_LEN, st, &[sp, sl], &[], ret0, "i");
            }
            // The test allocator, as a runtime-supplied body rather than as an
            // intrinsic call: `alloc` reads no state, so the handle is zero and
            // `allocate` answers the request.
            route::Body::Alloc => self.imm_to(ret0, 0),
            route::Body::Allocate => self.mv(ret0, p(1), 8),
            route::Body::ListLength => self.mv(ret0, p(0) + 8, 8),
            // `str.format(ctx, s)` answers its last argument — the template
            // has already been built by `str.concat` — and is `llvm/emit.rs`'s
            // arm exactly: three words copied, and **a count taken**, because
            // the copy is a second name for one block rather than a second
            // reference to it, and without the retain the second drop is a
            // double free.
            route::Body::StrFormat => {
                let last = fs.params.last().copied().unwrap_or(0);
                self.mv(ret0, last, 24);
                self.incref(ret0);
            }
            route::Body::ListEmpty => {
                self.imm_to(ret0, 0);
                self.imm_to(ret0 + 8, 0);
            }
            // The surfaces this backend open-codes, which reach it as a call
            // to the `Body::Runtime` function whose body this is.
            route::Body::PrimTrait(prim, op) => {
                let ret_tag = self.ret_tag(prog, fi);
                self.prim_trait_at(st, prim, op, ret0, p(0), p(1), ret_tag);
            }
            route::Body::Bits {
                stem,
                width,
                counted,
            } => self.bits_at(st, stem, width, counted, ret0, p(0), p(1)),
            route::Body::Concat => {
                let argc = prog.funcs.get(fi).sig.params.len();
                self.str_concat(st, Self::concat_strs(argc).map(p), ret0);
            }
        }
        self.emit("ret", &[]);
    }

    /// A `number.<T>.<op>` body, short of its `ret`.
    fn number_body(
        &mut self,
        prog: &ir::Program,
        fi: FuncIdx,
        st: &mut Fn2,
        prim: Prim,
        op: route::Number<'_>,
    ) {
        let fs = self.frame_sig_of(fi);
        let ret0 = RET;
        let p = |i: usize| fs.params.get(i).copied().unwrap_or(0);
        match op {
            route::Number::Show => self.show_prim(st, prim, p(0), ret0, true),
            // `Bounded::minValue` and `Bounded::maxValue`. The type comes from
            // the key — `middle::lower`'s `bounded_key` puts it there, because
            // the methods take no argument and the IR type of the result has
            // lost the signedness that separates `0` from `-128`. The bounds
            // are the **type's**, not JavaScript's exactly-representable ones:
            // `exact_int_range` is the JavaScript backend's business alone.
            route::Number::Bound(bits) => self.imm_num(ret0, prim, bits),
            // A conversion is named by its target: `toU8`, `wrapToI32`,
            // `toChar`. Between two integers it is a truncation and a
            // re-widening, because a frame slot holds an integer zero-extended
            // at its own width — so the source is sign-extended first where it
            // is signed, and masked to the target's width after.
            route::Number::Convert(to) => {
                // An **aggregate** result is the whole of the test, and it is
                // the same one `llvm/emit.rs::numeric` makes before its
                // `toChar` arm: an exact conversion, a `wrapTo` among them,
                // answers the target type and an inexact one answers
                // `Result<T, RangeError>` (SPEC 6.2.1), so the result's
                // *shape* says which this is without a second table.
                // `U32.toChar` is the case that needs it — not every `U32` is
                // a scalar value — and casting anyway would have written a
                // `Char` into a `Result` and called it success. An inexact one
                // is a range test and two arms rather than a cast.
                let exact = !matches!(prog.funcs.get(fi).sig.ret, ir::Type::Agg(_));
                if exact {
                    self.convert(st, prim, to, p(0), ret0);
                } else {
                    self.convert_checked(prog, fi, st, prim, to, p(0), ret0);
                }
            }
            // `Hash::hash` takes no accumulator, so it is the seeded form
            // (`cli/runtime/hash.rs`), and a float hashes through the double
            // it widens to.
            route::Number::Hash => {
                let seed = fs.param_end;
                self.imm_to(seed, runtime::HASH_SEED);
                self.hash_prim(st, prim, seed, p(0), ret0);
            }
            // `Checked`, `Saturating`, `Wrapping`, `abs` and `signum`, which
            // `core/number` declares without a body and every backend
            // open-codes. `llvm/emit.rs::numeric` is the twin, and the bound
            // each one checks is the **type's own range** — SPEC 6.2.2 and
            // VALUE-MODEL.md §12 row 2.
            route::Number::Checked(kind) => {
                self.checked(prog, fi, st, prim, kind, p(0), p(1), ret0);
            }
            route::Number::Saturating(int, kind) => {
                self.saturating(st, int, kind, p(0), p(1), ret0);
            }
            // Wrapping **is** the machine operation: two's complement at the
            // operand's own width is what an `i8` addition already does, and a
            // frame slot holds the answer re-truncated.
            route::Number::Wrapping(kind) => self.wrapping(prim, kind, p(0), p(1), ret0),
            route::Number::AbsSignum(abs) => self.abs_signum(st, prim, abs, p(0), ret0),
            // `Ordered::compare` answers an `Order`, which is an enum and not a
            // register.
            route::Number::Compare => match self.ret_tag(prog, fi) {
                Some(w) => self.compare_prim(st, prim, p(0), p(1), ret0, w),
                None => crate::diagnostics::ice(&format!(
                    "stencil: number.{}.compare into a destination that is not a tag",
                    prim.name()
                )),
            },
            // A number operator answers `Self`, one result.
            route::Number::Bin(name) => {
                let (tag, _, _) = num_tag(prim);
                self.emit(
                    &key!["bin/", name, "/", tag, "/ff/f"],
                    &[
                        Lit::A.of(p(0) as u64),
                        Lit::B.of(p(1) as u64),
                        Lit::D.of(ret0 as u64),
                        Jump::Cont.to(V::Fall),
                        Jump::Cont0.to(V::Fall),
                    ],
                );
            }
            // `Negate::negate(self)` takes its one operand.
            route::Number::Negate => {
                let (tag, _, _) = num_tag(prim);
                self.emit(
                    &key!["un/neg/", tag, "/f/f"],
                    &[
                        Lit::A.of(p(0) as u64),
                        Lit::D.of(ret0 as u64),
                        Jump::Cont.to(V::Fall),
                    ],
                );
            }
        }
    }
}

impl<'a> Jit<'a> {
    /// A `Body::Runtime` whose key `runtime.rs`'s table has a row for.
    ///
    /// `middle::lower` leaves `core/str`, `core/list`, `core/character`,
    /// `core/math` and the host capabilities as bodyless functions carrying an
    /// intrinsic key, exactly as it does for the other two backends, and the
    /// body is one call to the archive. There is no second implementation of
    /// any of them here: that was the prototype's shape, and it was
    /// `libburi_rt.a` written twice.
    ///
    /// The parameters are already in the frame at the offsets `frame_sigs`
    /// gave them, so the argument list is the signature's types paired with
    /// those offsets — the same list `Lower::intrinsic` builds from an
    /// `Inst::CallIntrinsic`'s values.
    fn runtime_intrinsic(
        &mut self,
        prog: &ir::Program,
        fi: FuncIdx,
        entry: &'static runtime::Entry,
        fs: &super::jit::FrameSig,
        st: &mut Fn2,
    ) {
        let f = prog.funcs.get(fi);
        let args: Vec<(u32, ir::Type)> = f
            .sig
            .params
            .iter()
            .enumerate()
            .map(|(i, t)| (fs.params.get(i).copied().unwrap_or(0), *t))
            .collect();
        let dest = (RET, f.sig.ret);
        self.rt_call(prog, st, entry, dest, &args, Vec::new());
    }

    /// The stride of `[T]`'s element, for an IR type that is a `[T]`.
    pub(crate) fn array_stride(&mut self, prog: &ir::Program, t: ir::Type) -> u64 {
        let elem = super::lists::element_of(prog, t);
        u64::from(self.layouts_of(elem).stride.max(1))
    }
}

impl Jit<'_> {
    /// The `Equal`/`Ordered`/`Hash`/`Show` leaves at `Bool` and `Char`, plus
    /// `Char::toU32`, with their operands as frame offsets.
    ///
    /// `tag` is the destination's tag width where the destination is an enum,
    /// as `compare`'s `Order` is.
    #[allow(
        clippy::too_many_arguments,
        reason = "the primitive and the operation are the route, and the rest \
                  are the frame offsets the sequence reads and writes"
    )]
    pub(crate) fn prim_trait_at(
        &mut self,
        st: &mut Fn2,
        prim: Prim,
        op: route::PrimOp,
        d: u32,
        a: u32,
        b: u32,
        tag: Option<u32>,
    ) {
        // `str`'s `equal`, `compare` and `hash` are runtime rows, which
        // resolve before this, and only a `Char` has a `toU32`.
        match op {
            // `show` is `$str`, not `$show`: the trait method renders the value
            // and the *derived* one quotes it.
            route::PrimOp::Show => self.show_prim(st, prim, a, d, false),
            // A frame slot holds a `Bool` and a `Char` zero-extended, so both
            // compare at sixty-four bits whatever their own width is, and
            // `false` sorting before `true` is what an unsigned compare says.
            route::PrimOp::Equal => {
                self.emit(
                    "bin/eq/u64/ff/f",
                    &[
                        Lit::D.of(u64::from(d)),
                        Lit::A.of(u64::from(a)),
                        Lit::B.of(u64::from(b)),
                        Jump::Cont.to(V::Fall),
                    ],
                );
            }
            route::PrimOp::Compare => {
                let raw = st.scratch + super::rtcall::RAW_WORD * 8;
                let entry: &[(&'static str, u64)] =
                    &[("bin/lt/u64/ff/f", LESS), ("bin/gt/u64/ff/f", GREATER)];
                // `Less`, `Equal`, `Greater` in declaration order, which is
                // what `middle::layout` gives a three-variant enum as a bare
                // tag: `Equal` unless one of the two tests says otherwise.
                self.imm_to(raw, EQUAL);
                for (k, v) in entry {
                    let scr = st.scratch + super::rtcall::SPARE_WORD * 8;
                    self.emit(
                        *k,
                        &[
                            Lit::D.of(u64::from(scr)),
                            Lit::A.of(u64::from(a)),
                            Lit::B.of(u64::from(b)),
                            Jump::Cont.to(V::Fall),
                        ],
                    );
                    let skip = st.label();
                    let brkey = self.arm_key("br/f", Jump::T);
                    self.emit(
                        brkey,
                        &[
                            Lit::A.of(u64::from(scr)),
                            Jump::T.to(V::Fall),
                            Jump::F.to(V::Blk(skip)),
                        ],
                    );
                    self.imm_to(raw, *v);
                    let here = self.region.code_addr();
                    st.place(skip, here);
                }
                match tag {
                    Some(w) => self.store_w(d, raw, w),
                    None => crate::diagnostics::ice(&format!(
                        "stencil: {}.compare into a destination that is not a tag",
                        prim.name()
                    )),
                }
            }
            // `$hashInto` at a `Bool` or a `Char`, from `core/order`'s offset
            // basis. A `Char` is one *string* of one character, so an astral
            // scalar is two mixes and the runtime owns that; a `Bool` is one
            // mix of its low word.
            route::PrimOp::Hash => {
                let symbol = if prim == Prim::Char {
                    runtime::HASH_CHAR
                } else {
                    runtime::MIX
                };
                let seed = st.scratch + super::rtcall::SPARE_WORD * 8;
                self.imm_to(seed, runtime::HASH_SEED);
                let args = [Src::Word(seed), Src::Word(a)];
                self.c_call(symbol, st, &args, &[], d, "i");
            }
            // `Char::toU32` and a `charOf` are the representation: a `Char`
            // **is** a `U32` holding a Unicode scalar value.
            route::PrimOp::Same => self.mv(d, a, 8),
        }
    }

    // -- the numeric surface ------------------------------------------------
    //
    // `Checked` and `Saturating` are one stencil and a branch apiece: the
    // stencil answers `(result, did it overflow)` at the operand's own width
    // (`sources.rs::checks`) and the branch turns that pair into the `Option`
    // one wants and the clamp the other does. Splitting it that way is what
    // keeps the *test* — which is different at every width — out of this file.

    /// The two scratch words the overflow pair lands in: the result, and the
    /// flag beside it.
    fn overflow_slots(st: &Fn2) -> (u32, u32) {
        (
            st.scratch + super::rtcall::RAW_WORD * 8,
            st.scratch + super::rtcall::SPARE_WORD * 8,
        )
    }

    /// `chk/<op>/<tag>` — the result and the overflow flag. Answers `false`
    /// for an operation or a width there is no stencil for.
    fn overflowing(&mut self, st: &Fn2, prim: Prim, kind: &str, a: u32, b: u32) {
        let (tag, _, _) = num_tag(prim);
        let name = match kind {
            "Add" => Some("add"),
            "Subtract" => Some("sub"),
            "Multiply" => Some("mul"),
            "Divide" => Some("div"),
            "Remainder" => Some("rem"),
            "Negate" => Some("neg"),
            "Power" => Some("pow"),
            _ => None,
        }
        .or_ice("`core/number` declares these checked operations and no others");
        let key = key!["chk/", name, "/", tag];
        let (res, flag) = Self::overflow_slots(st);
        self.emit(
            &key,
            &[
                Lit::A.of(u64::from(a)),
                Lit::B.of(u64::from(b)),
                Lit::D.of(u64::from(res)),
                Lit::N.of(u64::from(flag)),
                Jump::Cont.to(V::Fall),
                Jump::Cont0.to(V::Fall),
            ],
        );
    }

    /// `checkedAdd`, `checkedSubtract`, `checkedMultiply`, `checkedDivide` — an `Option<T>`.
    ///
    /// `Divide` is where "the type's own range" is not the same statement as "the
    /// machine did not wrap": `MIN / -1` is `2^63`, which the width cannot
    /// hold, so the stencil reports it alongside a zero divisor.
    #[allow(
        clippy::too_many_arguments,
        reason = "one operation's operands and the two programs it is read \
                  against, which is what naming the destination's layout needs"
    )]
    fn checked(
        &mut self,
        prog: &ir::Program,
        fi: FuncIdx,
        st: &mut Fn2,
        prim: Prim,
        kind: &str,
        a: u32,
        b: u32,
        dest: u32,
    ) -> bool {
        let (_, bits, _) = num_tag(prim);
        let id = ret_agg(prog, fi);
        let l = self.layout_id_shared(prog, id);
        self.overflowing(st, prim, kind, a, b);
        let (res, flag) = Self::overflow_slots(st);
        let pay = super::lists::payload_at(&l, 0);
        let some = st.label();
        let done = st.label();
        let key = self.arm_key("brcmp/eq/u64/fi", Jump::T);
        self.emit(
            key,
            &[
                Lit::A.of(u64::from(flag)),
                Word::K.lit(0),
                Jump::T.to(V::Blk(some)),
                Jump::F.to(V::Fall),
            ],
        );
        self.store_disc(&l, dest, 1);
        self.emit("jump", &[Jump::T.to(V::Blk(done))]);
        let here = self.region.code_addr();
        st.place(some, here);
        self.store_w(dest + pay, res, bits / 8);
        self.store_disc(&l, dest, 0);
        let here = self.region.code_addr();
        st.place(done, here);
        true
    }

    /// `x.toT()` where not every `x` fits, which answers
    /// `Result<T, RangeError>` (SPEC 6.2.1).
    ///
    /// The test is at the **source's** width against the **target's** own
    /// range, because "does not fit `T`" is a fact about `T`. A bound the
    /// source cannot reach is not tested at all: `I64 -> U64` cannot exceed
    /// `U64`'s maximum, and `U64 -> I64` cannot fall below `I64`'s minimum, so
    /// each of those is one comparison rather than two.
    ///
    /// The `.Err` arm renders the value it was handed, so a value that had
    /// already lost digits would say so rather than hide behind the message.
    ///
    /// # The four shapes
    ///
    /// [`Checked`] names them. Three of the four have a *source* the machine
    /// can compare directly against the target's ends; the fourth, `F64 -> F32`,
    /// has no integer range at all and asks a different question — whether the
    /// value survived as a finite one.
    #[allow(
        clippy::too_many_arguments,
        reason = "the program and the function index locate the `Result`'s layout, \
                  the two primitives are the conversion, and the two offsets are \
                  where the value comes from and where the answer goes; none is \
                  derivable from another"
    )]
    fn convert_checked(
        &mut self,
        prog: &ir::Program,
        fi: FuncIdx,
        st: &mut Fn2,
        from: Prim,
        to: Prim,
        src: u32,
        dest: u32,
    ) {
        let kind = checked_kind(from, to);
        let ty = prog.type_info(ret_agg(prog, fi)).ty;
        let error_ty = match ty.kind() {
            TyKind::Con(_, arguments) => arguments.get(1).copied(),
            _ => None,
        }
        .or_ice("a checked conversion answers `Result<T, ConversionError>`");
        // `middle::layout` gives a niche only to an `Option`, so the result
        // keeps a tag and each payload at its own offset. The error is
        // `core/number`'s `RangeError`: its value, then its target.
        let result = self.layout_of_type(ty);
        let error = self.layout_of_type(error_ty);
        let ok_at = super::lists::payload_at(&result, 0);
        let err_at = result
            .variant(1)
            .first()
            .copied()
            .or_ice("`.Err` carries its error");

        let bound = st.scratch + super::rtcall::SPARE_WORD * 8;
        let verdict = st.scratch;
        let err = st.label();
        let done = st.label();
        match kind {
            // Integer to integer. The test is at the **source's** width, and a
            // bound the source cannot reach is not tested at all.
            CheckedKind::Ints(source, target) => {
                let (tag, _, _) = num_tag(from);
                let ((from_lo, from_hi), (to_lo, to_hi)) = (source.range(), target.range());
                for (op, needed, pattern) in [
                    ("lt", to_lo > from_lo, to_lo as u128),
                    ("gt", to_hi < from_hi, to_hi),
                ] {
                    if !needed {
                        continue;
                    }
                    self.imm_num(bound, from, pattern);
                    self.cmp_to(tag, op, src, bound, verdict, err);
                }
                self.convert(st, from, to, src, dest + ok_at);
            }
            // A float source, which has three ways not to fit where an integer
            // one has two: it can be fractional, it can be `NaN`, and it can be
            // infinite. JavaScript answers all three with the same `.Err`
            // (`runtime.js`'s `$convChecked` tests `Number.isInteger` first and
            // the range second), and so does this.
            //
            // **The bounds are written one past the end.** `v > I64::MAX` is
            // not a comparison a double can make — `I64::MAX` has no double —
            // and rounding it up would accept `2^63`, which JavaScript's
            // `BigInt` comparison rejects. Every `hi + 1` is a power of two and
            // every `lo` is zero or a negative one, so both bounds are exact
            // doubles and `v >= hi + 1` is the same question with an answer.
            CheckedKind::FloatToInt(target) => {
                let v = self.as_f64(st, from, src);
                let (to_lo, to_hi) = target.range();
                let (ttag, tw, _) = num_tag(to);
                #[allow(
                    clippy::cast_precision_loss,
                    reason = "every integer type's lower bound is zero or a negative power \
                              of two, which a double holds exactly"
                )]
                let lo = to_lo as f64;
                #[allow(
                    clippy::cast_precision_loss,
                    reason = "every `hi + 1` here is a power of two at most 2^128, which a \
                              double holds exactly, and `U128`'s saturated `hi` rounds up to \
                              it; that is the property the bound relies on"
                )]
                let over = to_hi.saturating_add(1) as f64;
                self.imm_num(bound, Prim::F64, u128::from(lo.to_bits()));
                self.cmp_to("f64", "lt", v, bound, verdict, err);
                self.imm_num(bound, Prim::F64, u128::from(over.to_bits()));
                self.cmp_to("f64", "ge", v, bound, verdict, err);
                // In range, so the truncation is exact and the round trip back
                // through a double says whether the value was an integer at
                // all. `NaN` never gets this far — it fails both comparisons
                // above the way JavaScript's `isInteger` fails it.
                let back = st.scratch + (super::rtcall::RAW_WORD + 2) * 8;
                if tw == 128 {
                    // Two words, so the value goes straight into the `.Ok`.
                    let ok = dest + ok_at;
                    self.cvt(&key!["cvt/to/", ttag, "/f"], ok, v);
                    self.cvt(&key!["cvt/from/", ttag, "/f"], back, ok);
                    self.cmp_to("f64", "ne", back, v, verdict, err);
                } else {
                    let raw = st.scratch + (super::rtcall::RAW_WORD + 1) * 8;
                    let unsigned = !to.is_signed();
                    self.cvt(if unsigned { "cvt/f2u" } else { "cvt/f2i" }, raw, v);
                    self.cvt(if unsigned { "cvt/u2f" } else { "cvt/i2f" }, back, raw);
                    self.cmp_to("f64", "ne", back, v, verdict, err);
                    self.narrow(raw, dest + ok_at, tw);
                }
            }
            // `U32 -> Char`, where the target is a set of scalar values rather
            // than a range: everything above U+10FFFF is out, and so is the
            // surrogate block in the middle of it (`runtime.js`'s `$toChar`).
            CheckedKind::ToChar => {
                let (tag, _, _) = num_tag(from);
                let scalar = st.label();
                self.imm_num(bound, from, 0x0010_ffff);
                self.cmp_to(tag, "gt", src, bound, verdict, err);
                self.imm_num(bound, from, 0xd800);
                self.cmp_to(tag, "lt", src, bound, verdict, scalar);
                self.imm_num(bound, from, 0xdfff);
                self.cmp_to(tag, "gt", src, bound, verdict, scalar);
                self.emit("jump", &[Jump::T.to(V::Blk(err))]);
                let here = self.region.code_addr();
                st.place(scalar, here);
                self.convert(st, from, to, src, dest + ok_at);
            }
            // `F64 -> F32`, which fails only where the value does not survive
            // as a finite binary32. Asked of the answer rather than of the
            // input: the rounded value is infinite exactly when the input
            // overflowed — or was infinite already, and an infinity converts to
            // an infinity rather than failing.
            CheckedKind::ToF32 => {
                let back = st.scratch + super::rtcall::RAW_WORD * 8;
                self.cvt("cvt/f2f32", dest + ok_at, src);
                self.cvt("cvt/f322f", back, dest + ok_at);
                for infinite in [f64::INFINITY, f64::NEG_INFINITY] {
                    let survived = st.label();
                    self.imm_num(bound, Prim::F64, u128::from(infinite.to_bits()));
                    self.cmp_to("f64", "ne", back, bound, verdict, survived);
                    self.cmp_to("f64", "ne", src, bound, verdict, err);
                    let here = self.region.code_addr();
                    st.place(survived, here);
                }
            }
        }
        self.store_disc(&result, dest, 0);
        self.emit("jump", &[Jump::T.to(V::Blk(done))]);

        let here = self.region.code_addr();
        st.place(err, here);
        self.show_prim(st, from, src, dest + err_at + error.field(0), false);
        self.str_literal(dest + err_at + error.field(1), to.name().as_bytes());
        self.store_disc(&result, dest, 1);
        let here = self.region.code_addr();
        st.place(done, here);
    }

    /// `frame[verdict] = frame[a] <op> frame[b]`, and a branch to `target`
    /// when the answer is true.
    #[allow(
        clippy::too_many_arguments,
        reason = "a comparison and its branch: the two operands, where the answer \
                  lands, the width to compare at, and where a true answer goes"
    )]
    fn cmp_to(&mut self, tag: &str, op: &str, a: u32, b: u32, verdict: u32, target: u32) {
        self.emit(
            &key!["bin/", op, "/", tag, "/ff/f"],
            &[
                Lit::D.of(u64::from(verdict)),
                Lit::A.of(u64::from(a)),
                Lit::B.of(u64::from(b)),
                Jump::Cont.to(V::Fall),
            ],
        );
        let brkey = self.arm_key("br/f", Jump::F);
        self.emit(
            brkey,
            &[
                Lit::A.of(u64::from(verdict)),
                Jump::T.to(V::Blk(target)),
                Jump::F.to(V::Fall),
            ],
        );
    }

    /// One `cvt/*` stencil, which every conversion here is a sequence of.
    fn cvt<'k>(&mut self, key: impl Into<KeyArg<'k>>, dest: u32, src: u32) {
        self.emit(
            key,
            &[
                Lit::D.of(u64::from(dest)),
                Lit::A.of(u64::from(src)),
                Jump::Cont.to(V::Fall),
            ],
        );
    }

    /// A whole word as the target's own width, which is how a frame slot holds
    /// an integer: zero-extended at that width, whatever its signedness
    /// (`sources.rs::write`).
    fn narrow(&mut self, src: u32, dest: u32, bits: u32) {
        if bits >= 64 {
            self.mv(dest, src, 8);
        } else {
            self.emit(
                &key!["zext/", bits],
                &[
                    Lit::D.of(u64::from(dest)),
                    Lit::A.of(u64::from(src)),
                    Jump::Cont.to(V::Fall),
                ],
            );
        }
    }

    /// A float operand as a **double**, wherever it started.
    ///
    /// An `F32` sits in its frame slot as its own thirty-two bits, and every
    /// sequence below reads a slot as a double — so the widening happens once,
    /// here, and it is exact. It is also what JavaScript compares: an `F32`
    /// there is a `number`, which is the double this produces.
    fn as_f64(&mut self, st: &Fn2, from: Prim, src: u32) -> u32 {
        if from == Prim::F32 {
            let wide = st.scratch + super::rtcall::RAW_WORD * 8;
            self.cvt("cvt/f322f", wide, src);
            return wide;
        }
        src
    }

    /// `saturatingAdd`, `saturatingSubtract`, `saturatingMultiply`.
    ///
    /// The overflow is detected and the **end** it ran off is chosen, which is
    /// the same answer a wider type would give without there being one. Which
    /// end is a property of the operands' signs, not of the wrapped result —
    /// the wrapped result is precisely the thing that is wrong.
    fn saturating(
        &mut self,
        st: &mut Fn2,
        int: IntPrim,
        kind: &str,
        a: u32,
        b: u32,
        dest: u32,
    ) -> bool {
        let prim = int.prim();
        let (tag, _, signed) = num_tag(prim);
        // `core/number` declares no saturating division: it has no end to run off.
        self.overflowing(st, prim, kind, a, b);
        let (res, flag) = Self::overflow_slots(st);
        let (lo, hi) = (int_pattern(int, true), int_pattern(int, false));
        let ok = st.label();
        let done = st.label();
        let key = self.arm_key("brcmp/eq/u64/fi", Jump::T);
        self.emit(
            key,
            &[
                Lit::A.of(u64::from(flag)),
                Word::K.lit(0),
                Jump::T.to(V::Blk(ok)),
                Jump::F.to(V::Fall),
            ],
        );
        if !signed {
            // Unsigned: an addition or a multiplication can only run off the
            // top, and a subtraction only off the bottom.
            self.imm_num(dest, prim, if kind == "Subtract" { lo } else { hi });
        } else {
            // The sum of two operands that overflowed is negative exactly when
            // they were both positive, so the sign of `x` decides — and for
            // `Subtract` the same test is right for the same reason, because the
            // only way to underflow is a negative `x` against a positive `y`.
            // A product runs off the bottom when the signs differ.
            // Past `overflow_slots`' two words, which hold the result and the
            // flag this branch was reached on.
            let scr = st.scratch + (super::rtcall::RAW_WORD + 1) * 8;
            self.lt_zero(scr, a, tag);
            if kind == "Multiply" {
                let other = scr + 8;
                self.lt_zero(other, b, tag);
                self.emit(
                    "bin/xor/u64/ff/f",
                    &[
                        Lit::D.of(u64::from(scr)),
                        Lit::A.of(u64::from(scr)),
                        Lit::B.of(u64::from(other)),
                        Jump::Cont.to(V::Fall),
                    ],
                );
            }
            let top = st.label();
            let brkey = self.arm_key("br/f", Jump::T);
            self.emit(
                brkey,
                &[
                    Lit::A.of(u64::from(scr)),
                    Jump::T.to(V::Fall),
                    Jump::F.to(V::Blk(top)),
                ],
            );
            self.imm_num(dest, prim, lo);
            self.emit("jump", &[Jump::T.to(V::Blk(done))]);
            let here = self.region.code_addr();
            st.place(top, here);
            self.imm_num(dest, prim, hi);
        }
        self.emit("jump", &[Jump::T.to(V::Blk(done))]);
        let here = self.region.code_addr();
        st.place(ok, here);
        self.mv(dest, res, if prim.bits() > 64 { 16 } else { 8 });
        let here = self.region.code_addr();
        st.place(done, here);
        true
    }

    /// `frame[d] = frame[a] < 0` for a signed operand.
    ///
    /// At sixteen bytes there is no immediate stencil — `_JIT_K` is one word —
    /// so the sign test is a signed compare of the **high** half, which is the
    /// same question one instruction narrower.
    fn lt_zero(&mut self, d: u32, a: u32, tag: &str) {
        let (key, at) = if tag == "i128" {
            (key!["bin/lt/i64/fi/f"], a + 8)
        } else {
            (key!["bin/lt/", tag, "/fi/f"], a)
        };
        self.emit(
            &key,
            &[
                Lit::D.of(u64::from(d)),
                Lit::A.of(u64::from(at)),
                Word::K.lit(0),
                Jump::Cont.to(V::Fall),
            ],
        );
    }

    /// A literal into a destination at the type's own width: one store, or two
    /// where the type is sixteen bytes.
    fn imm_num(&mut self, dest: u32, prim: Prim, v: u128) {
        if prim.bits() > 64 {
            self.imm_to(dest, v as u64);
            self.imm_to(dest + 8, (v >> 64) as u64);
            return;
        }
        self.imm_to(dest, v as u64);
    }

    /// `wrappingAdd`, `wrappingSubtract`, `wrappingMultiply` — the machine operation at
    /// the operand's own width, which is what the ordinary stencil already is.
    fn wrapping(&mut self, prim: Prim, kind: &str, a: u32, b: u32, dest: u32) {
        let (tag, _, _) = num_tag(prim);
        let name = match kind {
            "Add" => Some("add"),
            "Subtract" => Some("sub"),
            "Multiply" => Some("mul"),
            _ => None,
        }
        .or_ice("`core/number` declares these wrapping operations and no others");
        let key = key!["bin/", name, "/", tag, "/ff/f"];
        self.emit(
            &key,
            &[
                Lit::A.of(u64::from(a)),
                Lit::B.of(u64::from(b)),
                Lit::D.of(u64::from(dest)),
                Jump::Cont.to(V::Fall),
            ],
        );
    }

    /// `abs` and `signum`.
    ///
    /// `abs` of a signed minimum overflows, and overflow is undefined
    /// (SPEC 6.2), so there is nothing to check. A float's is the sign bit
    /// cleared, which is exact for every value including the infinities and
    /// leaves a NaN a NaN.
    fn abs_signum(&mut self, st: &mut Fn2, prim: Prim, abs: bool, a: u32, dest: u32) {
        let (tag, bits, _) = num_tag(prim);
        let mask = if bits >= 64 {
            u64::MAX
        } else {
            (1u64 << bits) - 1
        };
        // A sign test at sixteen bytes reads the high half, which the `signum`
        // comparisons below cannot: they are `bin/{lt,gt}/{tag}/fi/f` and there
        // is no immediate form that wide. So it goes through the same wide
        // compare against a materialised zero.
        if abs {
            if prim.is_float() {
                let sign = if prim == Prim::F32 {
                    0x7fff_ffffu64
                } else {
                    !(1u64 << 63)
                };
                self.emit(
                    "bin/and/u64/fi/f",
                    &[
                        Lit::D.of(u64::from(dest)),
                        Lit::A.of(u64::from(a)),
                        Word::K.lit(sign),
                        Jump::Cont.to(V::Fall),
                    ],
                );
                return;
            }
            // The checker declares `abs` on signed integers and floats only.
            self.mv(dest, a, if bits > 64 { 16 } else { 8 });
            let scr = st.scratch + super::rtcall::SPARE_WORD * 8;
            self.lt_zero(scr, a, tag);
            let skip = st.label();
            let brkey = self.arm_key("br/f", Jump::T);
            self.emit(
                brkey,
                &[
                    Lit::A.of(u64::from(scr)),
                    Jump::T.to(V::Fall),
                    Jump::F.to(V::Blk(skip)),
                ],
            );
            self.emit(
                &key!["un/neg/", tag, "/f/f"],
                &[
                    Lit::A.of(u64::from(a)),
                    Lit::D.of(u64::from(dest)),
                    Jump::Cont.to(V::Fall),
                ],
            );
            let here = self.region.code_addr();
            st.place(skip, here);
            return;
        }
        // `signum`: zero, then one test per end. A `Float`'s answers are
        // `1.0`, `-1.0` and `0.0`, so a NaN — which is neither above nor
        // below — answers zero, exactly as the two `select`s `llvm/emit.rs`
        // chains do.
        let (zero, one, minus) = if prim == Prim::F64 {
            (
                0u128,
                u128::from(1.0f64.to_bits()),
                u128::from((-1.0f64).to_bits()),
            )
        } else if prim == Prim::F32 {
            (
                0,
                u128::from(1.0f32.to_bits()),
                u128::from((-1.0f32).to_bits()),
            )
        } else if bits > 64 {
            (0, 1, u128::MAX)
        } else {
            (0, 1, u128::from(mask))
        };
        self.imm_num(dest, prim, zero);
        // At sixteen bytes there is no immediate operand — `_JIT_K` is one
        // word — so the zero the two tests compare against is materialised.
        let wide = bits > 64;
        let zslot = st.scratch + (super::rtcall::RAW_WORD + 1) * 8;
        if wide {
            self.imm_num(zslot, prim, 0);
        }
        for (op, v) in [("gt", one), ("lt", minus)] {
            let scr = st.scratch + super::rtcall::SPARE_WORD * 8;
            let key = key![
                "bin/",
                op,
                "/",
                tag,
                "/",
                if wide { "ff" } else { "fi" },
                "/f"
            ];
            self.emit(
                &key,
                &[
                    Lit::D.of(u64::from(scr)),
                    Lit::A.of(u64::from(a)),
                    Lit::B.of(u64::from(zslot)),
                    Word::K.lit(0),
                    Jump::Cont.to(V::Fall),
                ],
            );
            let skip = st.label();
            let brkey = self.arm_key("br/f", Jump::T);
            self.emit(
                brkey,
                &[
                    Lit::A.of(u64::from(scr)),
                    Jump::T.to(V::Fall),
                    Jump::F.to(V::Blk(skip)),
                ],
            );
            self.imm_num(dest, prim, v);
            let here = self.region.code_addr();
            st.place(skip, here);
        }
    }

    /// `Ordered::compare` at a primitive: `Less`, `Equal`, `Greater` in
    /// declaration order, which is what `middle::layout` gives `core/order`'s
    /// three-variant enum as a bare tag.
    ///
    /// Two tests of the operands rather than one three-way instruction, because
    /// `Equal` is the answer neither test claims — and the tests are at the
    /// operand's *own* type, so a signed compare is signed and a `Float`'s is
    /// `fcmp`.
    fn compare_prim(&mut self, st: &mut Fn2, prim: Prim, a: u32, b: u32, dest: u32, w: u32) {
        let (tag, _, _) = num_tag(prim);
        if prim.is_float() {
            return self.compare_float(st, prim, a, b, dest, w);
        }
        let raw = st.scratch + super::rtcall::RAW_WORD * 8;
        self.imm_to(raw, EQUAL);
        for (op, v) in [("lt", LESS), ("gt", GREATER)] {
            let scr = st.scratch + super::rtcall::SPARE_WORD * 8;
            self.emit(
                &key!["bin/", op, "/", tag, "/ff/f"],
                &[
                    Lit::D.of(u64::from(scr)),
                    Lit::A.of(u64::from(a)),
                    Lit::B.of(u64::from(b)),
                    Jump::Cont.to(V::Fall),
                ],
            );
            let skip = st.label();
            let brkey = self.arm_key("br/f", Jump::T);
            self.emit(
                brkey,
                &[
                    Lit::A.of(u64::from(scr)),
                    Jump::T.to(V::Fall),
                    Jump::F.to(V::Blk(skip)),
                ],
            );
            self.imm_to(raw, v);
            let here = self.region.code_addr();
            st.place(skip, here);
        }
        self.store_w(dest, raw, w);
    }

    /// [`Self::compare_prim`] at a float: `-inf < … < -0.0 < 0.0 < … < inf <
    /// NaN`, every NaN equal to every other. `llvm/emit.rs`'s `float_order` is
    /// the twin.
    ///
    /// A run of guarded stores into the answer, each later one overriding the
    /// earlier. `eq` at a float is SPEC 7.2's, where every NaN equals every
    /// other, so `x == NaN` is how a NaN is found, and `1 / x` is how a zero's
    /// sign is read.
    fn compare_float(&mut self, st: &mut Fn2, prim: Prim, a: u32, b: u32, dest: u32, w: u32) {
        let (tag, _, _) = num_tag(prim);
        let word = |k: u32| st.scratch + (super::rtcall::RAW_WORD + k) * 8;
        let (raw, constant, ia, ib) = (word(0), word(1), word(2), word(3));
        let (one, nan) = if prim == Prim::F32 {
            (u64::from(1.0f32.to_bits()), u64::from(f32::NAN.to_bits()))
        } else {
            (1.0f64.to_bits(), f64::NAN.to_bits())
        };
        let bin = |op: &str| key!["bin/", op, "/", tag, "/ff/f"];
        self.imm_to(constant, one);
        for (from, to) in [(a, ia), (b, ib)] {
            self.emit(
                &bin("div"),
                &[
                    Lit::D.of(u64::from(to)),
                    Lit::A.of(u64::from(constant)),
                    Lit::B.of(u64::from(from)),
                    Jump::Cont.to(V::Fall),
                ],
            );
        }
        self.imm_to(constant, nan);
        self.imm_to(raw, EQUAL);
        // Each row's tests, all of which must hold, then the answer it stores.
        type Test<'t> = (&'t str, u32, u32);
        let rows: [(&[Test<'_>], u64); 7] = [
            (&[("lt", a, b)], LESS),
            (&[("gt", a, b)], GREATER),
            // Equal by `==`: a zero against a zero is told apart by its sign.
            (&[("eq", a, b), ("lt", ia, ib)], LESS),
            (&[("eq", a, b), ("gt", ia, ib)], GREATER),
            (&[("eq", b, constant)], LESS),
            (&[("eq", a, constant)], GREATER),
            (&[("eq", a, constant), ("eq", b, constant)], EQUAL),
        ];
        for (tests, value) in rows {
            let skip = st.label();
            for &(op, x, y) in tests {
                let scr = st.scratch + super::rtcall::SPARE_WORD * 8;
                self.emit(
                    &bin(op),
                    &[
                        Lit::D.of(u64::from(scr)),
                        Lit::A.of(u64::from(x)),
                        Lit::B.of(u64::from(y)),
                        Jump::Cont.to(V::Fall),
                    ],
                );
                let brkey = self.arm_key("br/f", Jump::T);
                self.emit(
                    brkey,
                    &[
                        Lit::A.of(u64::from(scr)),
                        Jump::T.to(V::Fall),
                        Jump::F.to(V::Blk(skip)),
                    ],
                );
            }
            self.imm_to(raw, value);
            let here = self.region.code_addr();
            st.place(skip, here);
        }
        self.store_w(dest, raw, w);
    }

    /// `derivePrimHash.<T>` — `(U64, T) -> U64`, the accumulator then the
    /// value.
    ///
    /// `llvm/emit.rs::hash_prim` is the twin and the symbols are the same
    /// ones, because what a hash has to agree with is `runtime.js`'s: a `Char`
    /// is a one-character *string* on JavaScript, so an astral scalar is two
    /// mixes and the runtime owns that, and a `Float` hashes through the double
    /// it widens to.
    fn hash_prim(&mut self, st: &mut Fn2, prim: Prim, acc: u32, v: u32, dest: u32) {
        use crate::compiler::semantics::types::Prim as P;
        let (a, w) = (Src::Word(acc), Src::Word(v));
        let (w8, w16) = (Src::Word(v + 8), Src::Word(v + 16));
        match prim {
            P::Str | P::Template => {
                self.c_call(runtime::HASH_STR, st, &[a, w, w8, w16], &[], dest, "i")
            }
            P::Char => self.c_call(runtime::HASH_CHAR, st, &[a, w], &[], dest, "i"),
            P::F32 | P::F64 => {
                let wide = if prim == P::F32 {
                    let scr = st.scratch + super::rtcall::SPARE_WORD * 8;
                    self.emit(
                        "cvt/f322f",
                        &[
                            Lit::D.of(u64::from(scr)),
                            Lit::A.of(u64::from(v)),
                            Jump::Cont.to(V::Fall),
                        ],
                    );
                    scr
                } else {
                    v
                };
                self.c_call(runtime::HASH_F64, st, &[a], &[Src::Word(wide)], dest, "i");
            }
            P::I128 => self.c_call(runtime::HASH_I128, st, &[a, w, w8], &[], dest, "i"),
            P::U128 => self.c_call(runtime::HASH_U128, st, &[a, w, w8], &[], dest, "i"),
            P::I64 => self.c_call(runtime::HASH_I64, st, &[a, w], &[], dest, "i"),
            P::U64 => self.c_call(runtime::HASH_U64, st, &[a, w], &[], dest, "i"),
            _ => self.c_call(runtime::MIX, st, &[a, w], &[], dest, "i"),
        }
    }

    /// One numeric conversion, between two types a frame slot can hold.
    ///
    /// Integer to integer is the interesting case and it is two steps: widen
    /// the *source* if it is signed, because a slot holds it zero-extended and
    /// the bits above its own width are zero rather than its sign; then narrow
    /// to the target, which is the same zero-extension at the new width. A
    /// 128-bit type on either side has stencils of its own, because it is two
    /// words rather than one.
    fn convert(&mut self, st: &Fn2, from: Prim, to: Prim, src: u32, dest: u32) {
        let (ftag, fw, fsigned) = num_tag(from);
        let (ttag, tw, _) = num_tag(to);
        // The sixteen-byte types, first, because everything below assumes a
        // value that fits one frame word.
        if fw == 128 || tw == 128 {
            if fw == 128 && tw == 128 {
                self.mv(dest, src, 16);
                return;
            }
            if tw == 128 {
                if from.is_float() {
                    let wide = self.as_f64(st, from, src);
                    self.cvt(&key!["cvt/to/", ttag, "/f"], dest, wide);
                    return;
                }
                let wide = st.scratch + super::rtcall::SPARE_WORD * 8;
                let at = self.widen(src, wide, fw, fsigned);
                self.emit(
                    &key!["cvt/to/", ttag, "/", if fsigned { "i" } else { "u" }],
                    &[
                        Lit::D.of(u64::from(dest)),
                        Lit::A.of(u64::from(at)),
                        Jump::Cont.to(V::Fall),
                    ],
                );
                return;
            }
            let key = match to {
                Prim::F32 => key!["cvt/from/", ftag, "/f32"],
                Prim::F64 => key!["cvt/from/", ftag, "/f"],
                _ => key!["cvt/from/", ftag, "/", tw],
            };
            self.emit(
                &key,
                &[
                    Lit::D.of(u64::from(dest)),
                    Lit::A.of(u64::from(src)),
                    Jump::Cont.to(V::Fall),
                ],
            );
            return;
        }
        match (from.is_float(), to.is_float()) {
            (true, true) => {
                let key = if to == Prim::F32 {
                    "cvt/f2f32"
                } else {
                    "cvt/f322f"
                };
                if from == to {
                    self.mv(dest, src, 8);
                    return;
                }
                self.emit(
                    key,
                    &[
                        Lit::D.of(u64::from(dest)),
                        Lit::A.of(u64::from(src)),
                        Jump::Cont.to(V::Fall),
                    ],
                );
            }
            (false, true) => {
                let wide = st.scratch + super::rtcall::SPARE_WORD * 8;
                let at = self.widen(src, wide, fw, fsigned);
                let key = match (fsigned, to == Prim::F32) {
                    (true, true) => "cvt/i2f32",
                    (true, false) => "cvt/i2f",
                    (false, true) => "cvt/u2f32",
                    (false, false) => "cvt/u2f",
                };
                self.cvt(key, dest, at);
            }
            // Float to integer truncates toward zero and keeps the low bits,
            // with `NaN` and the infinities at zero — `sources.rs`'s `cvt/f2i`,
            // and `llvm/emit.rs::float_to_int` on the other backend.
            //
            // **Signed, whatever the target is.** This arm is the *wrapping*
            // family's and a checked conversion's `.Ok` both, and the wrapping
            // one takes the low bits of a two's-complement truncation: JavaScript
            // reads `(-1.5).wrapToU8()` as `asUintN(8, -1n)`, which is `255`, and
            // a truncation toward zero followed by the target's own width is that
            // number. `convert_checked` is the one caller that wants the other
            // direction — a `U64` above `2^63` — and it emits its own conversion
            // for that reason.
            (true, false) => {
                let wide = self.as_f64(st, from, src);
                let raw = st.scratch + super::rtcall::RAW_WORD * 8 + 8;
                self.cvt("cvt/f2i", raw, wide);
                self.narrow(raw, dest, tw);
            }
            (false, false) => {
                let wide = st.scratch + super::rtcall::SPARE_WORD * 8;
                let at = self.widen(src, wide, fw, fsigned);
                if tw >= 64 {
                    self.mv(dest, at, 8);
                } else {
                    self.emit(
                        &key!["zext/", tw],
                        &[
                            Lit::D.of(u64::from(dest)),
                            Lit::A.of(u64::from(at)),
                            Jump::Cont.to(V::Fall),
                        ],
                    );
                }
            }
        }
    }

    /// A source value as a whole sixty-four-bit word, sign-extended where its
    /// type is signed. Answers where the widened value is, which is the source
    /// itself when nothing had to be done.
    fn widen(&mut self, src: u32, scratch: u32, bits: u32, signed: bool) -> u32 {
        if !signed || bits >= 64 {
            return src;
        }
        self.emit(
            &key!["sext/", bits],
            &[
                Lit::D.of(u64::from(scratch)),
                Lit::A.of(u64::from(src)),
                Jump::Cont.to(V::Fall),
            ],
        );
        scratch
    }

    /// The tag width of a function's first result, where that result is an
    /// enum. `None` covers both "not an aggregate" and "an aggregate that is
    /// not an enum", which is what the caller declines on either way.
    fn ret_tag(&mut self, prog: &ir::Program, fi: FuncIdx) -> Option<u32> {
        match prog.funcs.get(fi).sig.ret {
            ir::Type::Agg(id) => self.tag_width(prog, id.id()),
            _ => None,
        }
    }

    /// The tag width of an enum destination, or `None` when it is not one.
    fn tag_width(&mut self, prog: &ir::Program, id: ir::TypeId) -> Option<u32> {
        match &self.layout_id_shared(prog, id).repr {
            Repr::Enum {
                repr: EnumRepr::Bare { tag },
                ..
            }
            | Repr::Enum {
                repr: EnumRepr::Tagged { tag, .. },
                ..
            } => Some(tag.size()),
            _ => None,
        }
    }

    /// `core/bits`, open-coded, for the `Body::Runtime` function
    /// `middle::lower` leaves.
    ///
    /// Every one is a single machine instruction behind a range check, which is
    /// why none is a runtime call. The check is the whole of what is not the
    /// instruction: `$shiftCount` aborts on a count that is negative or at or
    /// beyond the operand's width, and `cli/tests/crash/shift_*` pins the
    /// message — so the check is unconditional and the abort is the runtime's
    /// shared one. A machine shift *masks* the count instead, which is a
    /// different answer rather than an undefined one and would be silently
    /// wrong.
    #[allow(
        clippy::too_many_arguments,
        reason = "the stem, width and count are the route, and the rest are \
                  the frame offsets the sequence reads and writes"
    )]
    pub(crate) fn bits_at(
        &mut self,
        st: &mut Fn2,
        stem: &str,
        width: u32,
        counted: bool,
        d: u32,
        a: u32,
        count: u32,
    ) {
        // The one-operand family: three counts and the byte reversal, none of
        // which takes a shift count and so none of which needs the range check.
        if !counted {
            // `sources.rs` generates every width `core/bits` declares.
            let k = key!["bits/", stem, "/", width];
            self.emit(
                &k,
                &[
                    Lit::D.of(u64::from(d)),
                    Lit::A.of(u64::from(a)),
                    Jump::Cont.to(V::Fall),
                ],
            );
            return;
        }
        self.shift_count_check(st, count, width);
        let key2 = key!["bits/", stem, "/", width];
        self.emit(
            &key2,
            &[
                Lit::D.of(u64::from(d)),
                Lit::A.of(u64::from(a)),
                Lit::B.of(u64::from(count)),
                Jump::Cont.to(V::Fall),
            ],
        );
    }

    /// `$shiftCount`'s range check: negative, or at or beyond the operand's
    /// width, aborts through the runtime's shared message.
    ///
    /// The count arrives as an `Int` — sixty-four bits — whatever the operand's
    /// width is, so the check is at sixty-four bits and the shift takes the
    /// value unchanged.
    fn shift_count_check(&mut self, st: &mut Fn2, count: u32, width: u32) {
        let ok = st.label();
        let scr = st.scratch + super::rtcall::SPARE_WORD * 8;
        self.emit(
            "bin/lt/u64/fi/f",
            &[
                Lit::D.of(u64::from(scr)),
                Lit::A.of(u64::from(count)),
                Word::K.lit(u64::from(width)),
                Jump::Cont.to(V::Fall),
            ],
        );
        let brkey = self.arm_key("br/f", Jump::T);
        self.emit(
            brkey,
            &[
                Lit::A.of(u64::from(scr)),
                Jump::T.to(V::Blk(ok)),
                Jump::F.to(V::Fall),
            ],
        );
        self.c_call(runtime::ABORT_SHIFT, st, &[], &[], 0, "v");
        let here = self.region.code_addr();
        st.place(ok, here);
    }
}

/// The bit pattern of a type's lowest or highest value, at the width a frame
/// slot holds it in.
///
/// The pattern, not the number: `u64::MAX` is not an `i64`, and a slot holds an
/// integer zero-extended at its own width. A float's bounds are its largest
/// finite magnitude, signed, and an `F32` is its own thirty-two bits.
pub(crate) fn bound_pattern(prim: Prim, low: bool) -> Option<u128> {
    if prim.is_float() {
        return Some(match (prim, low) {
            (Prim::F32, true) => u128::from((-f32::MAX).to_bits()),
            (Prim::F32, false) => u128::from(f32::MAX.to_bits()),
            (_, true) => u128::from(f64::MIN.to_bits()),
            (_, false) => u128::from(f64::MAX.to_bits()),
        });
    }
    Some(int_pattern(prim.as_int()?, low))
}

/// [`bound_pattern`] for an integer, which always has one.
fn int_pattern(int: IntPrim, low: bool) -> u128 {
    let (lo, hi) = int.range();
    let bits = int.bits();
    let pattern = if low { lo as u128 } else { hi };
    if bits >= 128 {
        return pattern;
    }
    let mask = if bits == 64 {
        u128::from(u64::MAX)
    } else {
        (1u128 << bits) - 1
    };
    pattern & mask
}

/// Which intrinsic keys this backend has a body for, asked ahead of emission.
///
/// `Backend::missing_intrinsics` is the *up-front* answer, so this is
/// deliberately the set of keys `Lower::intrinsic` and `Lower::runtime_body`
/// dispatch on, and not a claim that every program using one compiles: a shape
/// they decline — an argument list wider than a `crt` stencil, an element that
/// needs retain glue — is refused during emission with a sentence naming it.
/// The two answers are different questions and this is the cheaper one.
pub fn implemented(key: &str) -> bool {
    super::runtime::entry(key).is_some() || open_coded_key(key) || native_body(key)
}

/// The keys that are instructions rather than calls.
fn open_coded_key(key: &str) -> bool {
    matches!(
        key,
        "list.length"
            | "list.empty"
            | "str.concat"
            | "host_testing.alloc"
            | "host_testing.TestAllocator.allocate"
            | "str.length"
            | "str.format"
            | "str.equal"
            | "str.compare"
            | "str.show"
            | "testing_assert.report"
            | "testing_assert.reportShown"
            | "testing_assert.failWith"
            | "testing_assert.failExpected"
            | "testing_assert.failExpectedShown"
            // `core/alloc`'s copy-out: a move and the per-type copy glue, so
            // it is emitted into its own generated body and reaches no runtime
            // table row (G5, `glue.rs`'s `Helper::Walk`).
            | "alloc.copyOut"
    )
}
