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

use super::abi::Loc;
use crate::compiler::backend::intrinsic_keys::{
    bits_op, json_arm, json_variant, prim_trait_op, JsonArm,
};
use crate::compiler::semantics::types::{field_types, variant_types};

/// Where an edge's register half reads from.
#[derive(Clone, Copy, PartialEq, Eq)]
enum RSrc {
    Reg(u8),
    Slot(u32),
}
use super::jit::{Fn2, Jit, Plan, V};
use super::rtcall::{Src, EQUAL, GREATER, LESS};
use crate::compiler::middle::ir::{self, BinOp, Const, Inst, Target, Term, UnOp};
use crate::compiler::middle::layout::{EnumRepr, Repr, Scalar};
use crate::compiler::semantics::types::{Prim, Ty};

/// How deep the reference-count walk goes before it refuses.
///
/// A recursive type reaches itself through a **box**, and a box is refused by
/// the walk anyway, so this bounds the walk rather than the type — it is a
/// second belt on a case the first one already stops.
const RC_DEPTH: u32 = 8;

/// How many levels of a compound type's counted-pointer walk are emitted
/// inline before the rest goes through the type's own glue. See
/// [`Jit::walk_deep`].
const RC_INLINE: u32 = 2;

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

pub fn binop_name(op: BinOp) -> &'static str {
    match op {
        BinOp::Add => "add",
        BinOp::Sub => "sub",
        BinOp::Mul => "mul",
        BinOp::Div => "div",
        BinOp::Rem => "rem",
        BinOp::BitAnd => "and",
        BinOp::BitOr => "or",
        BinOp::BitXor => "xor",
        BinOp::Eq => "eq",
        BinOp::Ne => "ne",
        BinOp::Lt => "lt",
        BinOp::Le => "le",
        BinOp::Gt => "gt",
        BinOp::Ge => "ge",
    }
}

// `Fn2::folded`, `Fn2::constants` and `Fn2::wt` are `Jit::plan`'s side tables
// and carry one entry per value of the `ir::Code` being emitted, exactly as
// `Fn2::slot` — which is why `Fn2::at` reads its own with the same fallback.
// An index past the end would mean the IR disagreed with itself, so each of
// these three answers what a value the three analyses never saw would mean
// rather than panicking: not folded away, holding no immediate, and promoted
// into a register that nothing reads out of the frame.

/// Whether every use of `v` is an immediate operand, so that the `Const`
/// defining it was never materialised.
fn folded(st: &Fn2, v: ir::ValueId) -> bool {
    st.folded.get(v.index()).copied().unwrap_or(false)
}

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
            let n = [32u32, 24, 16, 8, 4, 2, 1].into_iter().find(|n| *n <= left).unwrap_or(1);
            self.emit(
                &format!("mov/{n}"),
                &[("JIT_D", V::I(d as u64)), ("JIT_A", V::I(s as u64)), ("JIT_CONT", V::Fall)],
            );
            d += n;
            s += n;
            left -= n;
        }
    }

    pub(crate) fn imm_to(&mut self, dst: u32, v: u64) {
        if v == 0 {
            self.emit("imm/z", &[("JIT_D", V::I(dst as u64)), ("JIT_CONT", V::Fall)]);
        } else if v < (1u64 << 32) {
            self.emit(
                "imm/32",
                &[("JIT_D", V::I(dst as u64)), ("JIT_N", V::I(v)), ("JIT_CONT", V::Fall)],
            );
        } else {
            self.emit(
                "imm/64",
                &[("JIT_D", V::I(dst as u64)), ("JIT_M", V::I(v)), ("JIT_CONT", V::Fall)],
            );
        }
    }

    /// [`Jit::imm_to`] for a value that is an address inside the region. Always
    /// the 64-bit form: a region address never fits 32 bits, and the pooled
    /// word it produces is what `cache.rs` relocates.
    pub(crate) fn imm_to_ptr(&mut self, dst: u32, v: u64) {
        debug_assert!(super::region::is_pool_handle(v), "not a constant-pool handle");
        self.emit(
            "imm/64",
            &[("JIT_D", V::I(dst as u64)), ("JIT_M", V::Ptr(v)), ("JIT_CONT", V::Fall)],
        );
    }

    /// Stores a frame word's low `w` bytes into a narrower field.
    pub(crate) fn store_w(&mut self, dst: u32, src: u32, w: u32) {
        match w {
            0 => {}
            1 | 2 | 4 => self.emit(
                &format!("store/{w}"),
                &[("JIT_D", V::I(dst as u64)), ("JIT_A", V::I(src as u64)), ("JIT_CONT", V::Fall)],
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
                &format!("loadu/{w}"),
                &[("JIT_D", V::I(dst as u64)), ("JIT_A", V::I(src as u64)), ("JIT_CONT", V::Fall)],
            ),
            _ => self.mv(dst, src, w),
        }
    }

    pub(crate) fn unsupported(&mut self, why: String) {
        let id = self.push_reason(why);
        self.emit("unsupported", &[("JIT_N", V::I(id))]);
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
                        let w = self.width_of(prog, ty);
                        // A literal wider than a word is two stores: truncating
                        // it to one would silently drop the top half of every
                        // `I128` constant in the program.
                        if w > 8 {
                            let x = if *negative { bits.wrapping_neg() } else { *bits };
                            self.imm_to(off, x as u64);
                            self.imm_to(off + 8, (x >> 64) as u64);
                            return;
                        }
                        let x = *bits as u64;
                        let x = if *negative { x.wrapping_neg() } else { x };
                        let x = if w >= 8 { x } else { x & ((1u64 << (w * 8)) - 1) };
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
            Inst::Unary { dest, op, prim, arg } => {
                self.unary(prog, code, st, *dest, *op, *prim, *arg)
            }
            Inst::Binary { dest, op, prim, lhs, rhs } => {
                self.binary(st, *dest, *op, *prim, *lhs, *rhs)
            }
            Inst::Call { dests, func, args } => self.call(prog, code, st, dests, func.0, args),
            Inst::CallIndirect { dests, callee, args } => {
                self.call_indirect(prog, code, st, dests, *callee, args)
            }
            Inst::CallIntrinsic { dests, key, args } => {
                self.intrinsic(prog, code, st, dests, key, args)
            }
            Inst::MakeStruct { dest, fields } => {
                let d = st.at(*dest);
                let (l, owner) = match code.ty_of(*dest) {
                    ir::Type::Agg(id) => {
                        (self.layout_of(prog, id), prog.type_info(id).ty.clone())
                    }
                    _ => return self.unsupported("MakeStruct of a non-aggregate".into()),
                };
                let ftys = field_types(self.tables, &owner);
                for (i, f) in fields.iter().enumerate() {
                    let w = self.width_of(prog, code.ty_of(*f));
                    if w == 0 || i >= l.fields.len() {
                        continue;
                    }
                    let off = l.field(i);
                    if ftys.get(i).is_some_and(|t| self.boxes(&owner, t)) {
                        self.box_into(st, d + off, st.at(*f), w);
                        continue;
                    }
                    self.store_w(d + off, st.at(*f), w);
                }
            }
            Inst::GetField { dest, agg, index } => {
                let (l, owner) = match code.ty_of(*agg) {
                    ir::Type::Agg(id) => {
                        (self.layout_of(prog, id), prog.type_info(id).ty.clone())
                    }
                    _ => return self.unsupported("GetField of a non-aggregate".into()),
                };
                let w = self.width_of(prog, code.ty_of(*dest));
                let ftys = field_types(self.tables, &owner);
                if (*index as usize) < l.fields.len() {
                    let off = l.field(*index as usize);
                    let src = st.at(*agg) + off;
                    if ftys.get(*index as usize).is_some_and(|t| self.boxes(&owner, t)) {
                        self.unbox_from(st, st.at(*dest), src, w);
                        return;
                    }
                    self.load_w(st.at(*dest), src, w);
                }
            }
            Inst::MakeEnum { dest, variant, fields } => {
                self.make_enum(prog, code, st, *dest, *variant, fields)
            }
            Inst::GetPayload { dest, agg, variant, index } => {
                let (l, owner) = match code.ty_of(*agg) {
                    ir::Type::Agg(id) => {
                        (self.layout_of(prog, id), prog.type_info(id).ty.clone())
                    }
                    _ => return self.unsupported("GetPayload of a non-aggregate".into()),
                };
                let offs = l.variant(*variant as usize).to_vec();
                let w = self.width_of(prog, code.ty_of(*dest));
                let ftys = variant_types(self.tables, &owner, *variant as usize);
                match offs.get(*index as usize) {
                    Some(o) => {
                        let src = st.at(*agg) + *o;
                        if ftys.get(*index as usize).is_some_and(|t| self.boxes(&owner, t)) {
                            self.unbox_from(st, st.at(*dest), src, w);
                            return;
                        }
                        self.load_w(st.at(*dest), src, w)
                    }
                    None => self.unsupported("GetPayload of an absent field".into()),
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
                // unit type, and a plain `FnRef` has none at all.
                let want = match code.ty_of(*dest) {
                    ir::Type::Agg(id) => match &prog.type_info(id).ty {
                        Ty::Fn(ps, _) => Some(ps.len()),
                        _ => None,
                    },
                    _ => None,
                };
                let Some(args) = want else {
                    return self.unsupported("MakeClosure of a value that is not a function".into());
                };
                let thunk = self.helper(super::glue::Helper::Thunk {
                    func: func.0,
                    args: args as u32,
                    boxed: env.is_some(),
                });
                self.emit(
                    "imm/64",
                    &[("JIT_D", V::I(d as u64)), ("JIT_M", V::Sym(thunk)), ("JIT_CONT", V::Fall)],
                );
                match env {
                    Some(e) => self.build_env(prog, code, st, d + super::glue::ENV_WORD, *e),
                    None => self.imm_to(d + super::glue::ENV_WORD, 0),
                }
            }
            Inst::IncRef { value } => self.rc(prog, code, st, *value, true),
            Inst::DecRef { value, .. } => self.rc(prog, code, st, *value, false),
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
                self.emit(
                    "abort",
                    &[("JIT_M", V::Ptr(p)), ("JIT_N", V::I(b.len() as u64))],
                );
            }
            Inst::MakeArray { dest, elems } => {
                // One allocation and `n` stores (VALUE-MODEL.md §4).
                let d0 = st.at(*dest);
                let Some(stride) = self.array_stride(prog, code.ty_of(*dest)) else {
                    return self.unsupported("MakeArray of a non-list".into());
                };
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
                        ("JIT_D", V::I(d0 as u64)),
                        ("JIT_A", V::I(n as u64)),
                        ("JIT_P", V::I(stride)),
                        ("JIT_CONT0", V::Fall),
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
                        let n = [8u32, 4, 2, 1].into_iter().find(|n| *n <= w - done).unwrap_or(1);
                        self.emit(
                            &format!("pstore/{n}"),
                            &[
                                ("JIT_A", V::I(d0 as u64)),
                                ("JIT_B", V::I((src + done) as u64)),
                                ("JIT_N", V::I(base + k)),
                                ("JIT_CONT", V::Fall),
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
                let Some((stride, w, _)) = self.array_elem(prog, code.ty_of(*array)) else {
                    return self.unsupported("ArrayGet of a non-array".into());
                };
                let s0 = st.scratch + 48;
                self.mv(s0, st.at(*array), 8);
                let d = st.at(*dest);
                let i = st.at(*index);
                self.elem_load(d, s0, i, stride, w);
            }
            Inst::ArrayLen { dest, array } => {
                // `[T]` is `{ ptr, len }`, and the length is O(1) by §4.
                self.mv(st.at(*dest), st.at(*array) + 8, 8)
            }
            // `xs[from..]`, for the `..rest` of an array pattern. A **copy**,
            // not a view: VALUE-MODEL.md §4 is explicit that a `[T]` is never
            // one — its header is at `ptr - 16` — so handing back an interior
            // pointer would make the next `decref` read a header that is not
            // one. Every element is retained on the way out, so the counts
            // balance against the copy's own eventual release.
            Inst::ArraySlice { dest, array, from } => {
                let Some((stride, _, _)) = self.array_elem(prog, code.ty_of(*array)) else {
                    return self.unsupported("ArraySlice of a non-array".into());
                };
                let glue = match self.element_of(prog, code.ty_of(*array)) {
                    Some(elem) => self.element_glue(elem),
                    None => None,
                };
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
                if let Err(why) = self.c_call("buri_rt_list_slice", st, &args, &[], dst, "v") {
                    self.unsupported(why);
                }
            }
            Inst::Structural { dest, op, ty, args } => {
                self.structural(prog, code, st, *dest, *op, *ty, args)
            }
        }
    }

    /// `Inst::Structural` — the placeholder `middle::derives` leaves where it
    /// declined to generate a body.
    ///
    /// `llvm/emit.rs::structural` is the twin, and it is narrower than the name
    /// suggests: **only `Show` reaches a backend**, only at a *primitive*
    /// type, and only from a template hole — every structural `Eq`, `Cmp`,
    /// `Hash` and `ToJson` on a compound type has already become an
    /// `Inst::Call` to a generated function by the time lowering runs. So this
    /// is `show_prim(quoted = false)`, and "unquoted" is the whole of the
    /// difference from `derivePrimShow`: `runtime.js`'s `$str` hole renders a
    /// `Str` as itself and a `Char` without the `'`s, where `$show` quotes both.
    #[allow(
        clippy::too_many_arguments,
        reason = "one instruction's operands, each of which it needs: the two \
                  programs it is read against, the frame it is emitted into, \
                  and the four fields `Inst::Structural` has"
    )]
    fn structural(
        &mut self,
        prog: &ir::Program,
        code: &ir::Code,
        st: &mut Fn2,
        dest: ir::ValueId,
        op: ir::StructuralOp,
        ty: ir::TypeId,
        args: &[ir::ValueId],
    ) {
        if !matches!(op, ir::StructuralOp::Show) {
            let name = prog.type_info(ty).name.clone();
            return self.unsupported(format!("Structural::{op:?} on `{name}`"));
        }
        let Some(arg) = args.first().copied() else { return };
        let source = prog.type_info(ty).ty.clone();
        let Some(prim) = self.tables.as_prim(&source) else {
            let name = prog.type_info(ty).name.clone();
            return self.unsupported(format!("Structural::Show of a non-primitive `{name}`"));
        };
        let d = st.at(dest);
        let a = st.at(arg);
        // A `Str` in a template *is* its own rendering: three words, copied.
        // No allocation, no call, and no count — `middle::rc` has already
        // decided who owns the bytes.
        if matches!(prim, Prim::Str | Prim::Template) {
            let n = self.width_of(prog, code.ty_of(dest));
            if n > 0 {
                self.mv(d, a, n);
            }
            return;
        }
        // `quoted = false`: a `Char` renders as its own bytes.
        if let Err(why) = self.show_prim(st, prim, a, d, false) {
            self.unsupported(why);
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
        let (l, owner) = match code.ty_of(dest) {
            ir::Type::Agg(id) => (self.layout_of(prog, id), prog.type_info(id).ty.clone()),
            _ => return self.unsupported("MakeEnum of a non-aggregate".into()),
        };
        let ftys = variant_types(self.tables, &owner, variant as usize);
        let d = st.at(dest);
        let Repr::Enum { repr, variants } = l.repr.clone() else {
            return self.unsupported("MakeEnum of a non-enum layout".into());
        };
        match repr {
            EnumRepr::Bare { tag } => self.imm_w(d, tag.size(), variant as u64),
            EnumRepr::Tagged { tag, .. } => {
                self.imm_w(d, tag.size(), variant as u64);
                let offs = variants.get(variant as usize).cloned().unwrap_or_default();
                self.store_variant(prog, code, st, d, fields, &offs, &owner, &ftys);
            }
            EnumRepr::Niche { null_at } => {
                let offs = variants.get(variant as usize).cloned().unwrap_or_default();
                if offs.is_empty() {
                    // The null variant: VALUE-MODEL.md §6's second niche.
                    self.imm_to(d + null_at, 0);
                } else {
                    self.store_variant(prog, code, st, d, fields, &offs, &owner, &ftys);
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
        for (i, f) in fields.iter().enumerate() {
            let w = self.width_of(prog, code.ty_of(*f));
            let Some(o) = offs.get(i).copied() else { continue };
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
    /// double free, and it is the whole walk rather than one `incref` for the
    /// same reason `lists.rs::retain_value` is.
    fn json_prim(
        &mut self,
        prog: &ir::Program,
        code: &ir::Code,
        st: &mut Fn2,
        prim: Prim,
        arg: ir::ValueId,
        dest: ir::ValueId,
    ) -> Result<(), String> {
        let ir::Type::Agg(id) = code.ty_of(dest) else {
            return Err(String::from("a `derive ToJson` whose result is not an aggregate"));
        };
        let owner = prog.type_info(id).ty.clone();
        let arm = json_arm(prim);
        let Some(variant) = json_variant(self.tables, &owner, arm) else {
            return Err(format!(
                "a `derive ToJson` answering `{}`, which is not `core/json`'s `Json`",
                prog.type_info(id).name
            ));
        };
        let l = self.layout_of(prog, id);
        let Repr::Enum { repr: EnumRepr::Tagged { tag, .. }, variants } = l.repr.clone() else {
            return Err(String::from("a `Json` whose layout is not a tagged enum"));
        };
        let Some(off) = variants.get(variant).and_then(|v| v.first()).copied() else {
            return Err(String::from("a `Json` arm that carries no payload"));
        };
        let (d, a) = (st.at(dest), st.at(arg));
        self.imm_w(d, tag.size(), variant as u64);
        match arm {
            JsonArm::Bool => {
                let w = self.width_of(prog, code.ty_of(arg));
                self.store_w(d + off, a, w);
                Ok(())
            }
            // JSON's one number type is a double, so every numeric primitive
            // widens to `F64` — the same narrowing `$json_of` does with
            // `Number(v)`, and lossy in the same place. A 128-bit value is
            // refused here rather than rounded, because `convert` has no
            // sixteen-byte-to-float step and inventing one at the call site is
            // how two backends stop agreeing.
            JsonArm::Num => {
                let scratch = st.scratch;
                self.convert(st, prim, Prim::F64, a, scratch)?;
                self.store_w(d + off, scratch, 8);
                Ok(())
            }
            JsonArm::Str => match prim {
                Prim::Str | Prim::Template => {
                    self.rc(prog, code, st, arg, true);
                    let w = self.width_of(prog, code.ty_of(arg));
                    self.mv(d + off, a, w);
                    Ok(())
                }
                // A `Char` is a one-scalar string, and the runtime already
                // builds it: `buri_rt_char_to_str` is the unquoted half of
                // `show_prim`'s `Char` arm, and the block it answers is fresh,
                // so this arm needs no retain.
                _ => self.c_call(
                    "buri_rt_char_to_str",
                    st,
                    &[Src::Word(a), Src::Addr(d + off)],
                    &[],
                    d + off,
                    "v",
                ),
            },
        }
    }

    /// A **boxed** field: `middle::layout` puts a pointer where the field would
    /// be, for the field that would otherwise make the owner recursive
    /// (`Layouts::boxes`). So writing one is an allocation, a copy and a store
    /// of the pointer — the shape `llvm/repr.rs`'s `Site::Boxed` releases.
    fn box_into(&mut self, st: &mut Fn2, dest: u32, src: u32, size: u32) {
        let (ptr, one) = (st.scratch, st.scratch + 8);
        self.imm_to(one, 1);
        self.emit(
            "elemalloc",
            &[
                ("JIT_D", V::I(u64::from(ptr))),
                ("JIT_A", V::I(u64::from(one))),
                ("JIT_P", V::I(u64::from(size))),
                ("JIT_CONT0", V::Fall),
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
                &format!("immw/{w}"),
                &[("JIT_D", V::I(dst as u64)), ("JIT_N", V::I(v)), ("JIT_CONT", V::Fall)],
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
        let l = match code.ty_of(agg) {
            ir::Type::Agg(id) => self.layout_of(prog, id),
            _ => return self.unsupported("GetTag of a non-aggregate".into()),
        };
        let d = st.at(dest);
        let a = st.at(agg);
        let Repr::Enum { repr, variants } = l.repr.clone() else {
            return self.unsupported("GetTag of a non-enum layout".into());
        };
        match repr {
            EnumRepr::Bare { tag } | EnumRepr::Tagged { tag, .. } => {
                self.load_w(d, a, tag.size())
            }
            EnumRepr::Niche { null_at } => {
                let null_variant =
                    variants.iter().position(|v| v.is_empty()).unwrap_or(1) as u64;
                let other = u64::from(null_variant == 0);
                self.emit(
                    "niche_tag",
                    &[
                        ("JIT_D", V::I(d as u64)),
                        ("JIT_A", V::I((a + null_at) as u64)),
                        ("JIT_N", V::I(null_variant)),
                        ("JIT_P", V::I(other)),
                        ("JIT_CONT", V::Fall),
                    ],
                );
            }
        }
    }

    /// `Inst::IncRef` and `Inst::DecRef`: MEMORY.md §5.1's saturating increment
    /// and its decrement, over **every** counted block the value owns.
    ///
    /// The walk covers all five site kinds `llvm/repr.rs`'s `Site` names. A
    /// missing release is a leak, and a leak that compiles is a wrong program
    /// that passes its tests: `cli/tests/native/runtime.rs` holds the toolchain
    /// to "every allocation is freed at exit", and a backend that quietly did
    /// not would be reported by that test rather than by this one.
    fn rc(
        &mut self,
        prog: &ir::Program,
        code: &ir::Code,
        st: &mut Fn2,
        value: ir::ValueId,
        retain: bool,
    ) {
        let ir::Type::Agg(id) = code.ty_of(value) else { return };
        let ty = prog.type_info(id).ty.clone();
        let at = st.at(value);
        if let Err(why) = self.walk_rc(st, &ty, at, retain, 0) {
            self.unsupported(why);
        }
    }

    /// One value's counted blocks, in the order `middle::rc`'s classifier names
    /// them.
    ///
    /// `depth` bounds the walk the same way `llvm/emit.rs` bounds its own,
    /// and [`Jit::walk_deep`] is what keeps it from being reached: a recursive
    /// type reaches itself through a **box**, which is a leaf here, and a deep
    /// non-recursive one goes out of line into its own glue.
    pub(crate) fn walk_rc(
        &mut self,
        st: &mut Fn2,
        ty: &Ty,
        at: u32,
        retain: bool,
        depth: u32,
    ) -> Result<(), String> {
        if depth > RC_DEPTH {
            return Err(String::from("a reference-counted type nested past the walk's depth"));
        }
        let l = self.layouts_of(ty.clone());
        match l.repr.clone() {
            Repr::Str => {
                self.rc_block(at, retain, None);
                Ok(())
            }
            Repr::List => {
                // The block itself is counted; its **elements** are released by
                // the glue the free path is handed, which is what makes one
                // pointer enough to drop a whole `[T]`.
                let Ty::Array(elem) = ty else {
                    return Err(String::from("a list layout on a type that is not one"));
                };
                let glue = (!retain && self.rc_counted(elem))
                    .then(|| self.helper(super::glue::Helper::Elems { ty: (**elem).clone() }));
                self.rc_block(at, retain, glue);
                Ok(())
            }
            // A closure's environment is a heap block holding its own release
            // function in the first word (`glue.rs`), which is what `Ty::Fn`
            // not recording what was captured forces:
            // `llvm/emit.rs::build_env` allocates the same shape and counts the
            // same word.
            Repr::Closure => {
                let glue = (!retain).then(|| self.helper(super::glue::Helper::EnvGlue));
                self.rc_block(at + super::glue::ENV_WORD, retain, glue);
                Ok(())
            }
            Repr::Zero | Repr::Scalar(_) => Ok(()),
            Repr::Aggregate => {
                for (i, f) in field_types(self.tables, ty).iter().enumerate() {
                    let off = l.fields.get(i).copied().unwrap_or(0);
                    if self.boxes(ty, f) {
                        self.rc_box(at + off, f, retain);
                        continue;
                    }
                    if !self.rc_counted(f) {
                        continue;
                    }
                    self.walk_deep(st, f, at + off, retain, depth)?;
                }
                Ok(())
            }
            Repr::Enum { repr, variants } => {
                let tag = match repr {
                    EnumRepr::Tagged { tag, .. } => tag,
                    // A bare tag carries no payload at all.
                    EnumRepr::Bare { .. } => return Ok(()),
                    // A niche is a pointer whose null *is* the discriminant, so
                    // the walk is the payload's behind that same null test.
                    EnumRepr::Niche { null_at } => {
                        return self.niche_rc(st, ty, at, null_at, retain, depth)
                    }
                };
                self.tagged_rc(st, ty, at, &variants, tag, retain, depth)
            }
        }
    }

    /// A niche `Option<T>`: the payload **is** the value, and the pointer the
    /// niche spends is null exactly when the value is `.None`.
    ///
    /// The walk is therefore behind that null test, and the test is not
    /// belt-and-braces: `.None` is written by storing null at `null_at` and
    /// nothing else (`Lower::store_disc`, `rtcall::store_option_tag`), so every
    /// other byte of the payload area is whatever the frame last held. Walking
    /// it unguarded decremented a reference count at an address that was never
    /// a pointer — `llvm/repr.rs`'s `Site::Guarded` is the same test for the
    /// same reason.
    fn niche_rc(
        &mut self,
        st: &mut Fn2,
        ty: &Ty,
        at: u32,
        null_at: u32,
        retain: bool,
        depth: u32,
    ) -> Result<(), String> {
        let Ty::Con(_, args) = ty else { return Ok(()) };
        let Some(payload) = args.first().cloned() else { return Ok(()) };
        if !self.rc_counted(&payload) {
            return Ok(());
        }
        let skip = st.label();
        let key = self.arm_key("brcmp/eq/u64/fi", "JIT_T");
        self.emit(
            &key,
            &[
                ("JIT_A", V::I(u64::from(at + null_at))),
                ("JIT_K", V::I(0)),
                ("JIT_T", V::Blk(skip)),
                ("JIT_F", V::Fall),
            ],
        );
        self.walk_rc(st, &payload, at, retain, depth + 1)?;
        let here = self.region.code_addr();
        st.place(skip, here);
        Ok(())
    }

    /// A tagged enum: one arm per variant that owns something, dispatched on
    /// the tag the same way a `match` is.
    #[allow(
        clippy::too_many_arguments,
        reason = "one variant walk's inputs, each of which it needs"
    )]
    fn tagged_rc(
        &mut self,
        st: &mut Fn2,
        ty: &Ty,
        at: u32,
        variants: &[Vec<u32>],
        tag: Scalar,
        retain: bool,
        depth: u32,
    ) -> Result<(), String> {
        let done = st.label();
        for (v, offsets) in variants.iter().enumerate() {
            let fields = variant_types(self.tables, ty, v);
            let owned: Vec<(u32, Ty, bool)> = fields
                .iter()
                .enumerate()
                .filter_map(|(i, f)| {
                    let boxed = self.boxes(ty, f);
                    (boxed || self.rc_counted(f))
                        .then(|| (offsets.get(i).copied().unwrap_or(0), f.clone(), boxed))
                })
                .collect();
            if owned.is_empty() {
                continue;
            }
            let next = st.label();
            let scr = st.scratch + super::rtcall::SPARE_WORD * 8;
            self.load_w(scr, at, tag.size());
            let key = self.arm_key("brcmp/eq/u64/fi", "JIT_T");
            self.emit(
                &key,
                &[
                    ("JIT_A", V::I(u64::from(scr))),
                    ("JIT_K", V::I(v as u64)),
                    ("JIT_T", V::Fall),
                    ("JIT_F", V::Blk(next)),
                ],
            );
            for (off, f, boxed) in owned {
                if boxed {
                    // The field *is* the pointer, so this is one reference
                    // operation on it and no descent.
                    self.rc_box(at + off, &f, retain);
                    continue;
                }
                self.walk_deep(st, &f, at + off, retain, depth)?;
            }
            self.emit("jump", &[("JIT_T", V::Blk(done))]);
            let here = self.region.code_addr();
            st.place(next, here);
        }
        let here = self.region.code_addr();
        st.place(done, here);
        Ok(())
    }

    /// One step down inside [`Jit::walk_rc`]: the field's own glue where that
    /// field is a compound one and the walk is already deep, and the walk
    /// inline where it is not.
    ///
    /// The threshold has to apply at **every** level and not only at the top.
    /// A type graph is a DAG whose nodes are revisited along every path, so an
    /// inline walk of a record of records of records expands once per path
    /// rather than once per type — which is what
    /// `conformance/lib/semantics/test/generics.buri` is
    /// (`Tree`, `Pair`, `Either`, `Slot`, `Boxed`, each over the others) and
    /// what made the walk run out of depth there. Going through here, an
    /// emitted body holds at most [`RC_INLINE`] levels of its own plus one call
    /// per deeper field, so the code is linear in the *distinct* types a
    /// program holds. `llvm/emit.rs`'s glue threshold is the same one for the
    /// same reason.
    ///
    /// A `Str`, a `[T]` and a closure are one reference operation whatever the
    /// depth, so they never go out of line: the call would cost more than the
    /// instruction it replaced.
    fn walk_deep(
        &mut self,
        st: &mut Fn2,
        ty: &Ty,
        at: u32,
        retain: bool,
        depth: u32,
    ) -> Result<(), String> {
        let compound = matches!(
            self.layouts_of(ty.clone()).repr,
            Repr::Aggregate | Repr::Enum { .. }
        );
        if compound && depth >= RC_INLINE {
            let sym = self.helper(super::glue::Helper::Walk { ty: ty.clone(), retain });
            let addr = st.scratch + (super::rtcall::RAW_WORD + 3) * 8;
            self.emit(
                "lea",
                &[
                    ("JIT_D", V::I(u64::from(addr))),
                    ("JIT_A", V::I(u64::from(at))),
                    ("JIT_CONT", V::Fall),
                ],
            );
            return self.c_call_sym(sym, st, &[Src::Word(addr)], &[], 0, "v");
        }
        self.walk_rc(st, ty, at, retain, depth + 1)
    }

    /// The reference operation on a **boxed** field: the field is the block's
    /// pointer, so there is no descent — whatever is inside it is the block's
    /// own drop glue's business.
    fn rc_box(&mut self, at: u32, ty: &Ty, retain: bool) {
        let glue = (!retain && self.rc_counted(ty))
            .then(|| self.helper(super::glue::Helper::Walk { ty: ty.clone(), retain: false }));
        self.rc_block(at, retain, glue);
    }

    /// The increment or the decrement of one block, at a frame offset holding
    /// its pointer.
    ///
    /// `glue` is what releases the block's *contents* once its count reaches
    /// zero, and is `None` for a block that holds only bytes — a `Str`'s
    /// allocation, a `[Int]`. A retain never needs one: taking a reference on a
    /// block says nothing about what is inside it.
    fn rc_block(&mut self, at: u32, retain: bool, glue: Option<String>) {
        if retain {
            self.emit("incref", &[("JIT_A", V::I(u64::from(at))), ("JIT_CONT", V::Fall)]);
            return;
        }
        match glue {
            Some(g) => self.emit(
                "decref/drop",
                &[
                    ("JIT_A", V::I(u64::from(at))),
                    ("JIT_M", V::Sym(g)),
                    ("JIT_CONT0", V::Fall),
                ],
            ),
            None => {
                self.emit("decref/free", &[("JIT_A", V::I(u64::from(at))), ("JIT_CONT0", V::Fall)])
            }
        }
    }

    fn variant_count(&self, ty: &Ty) -> usize {
        let Ty::Con(id, _) = ty else { return 0 };
        self.tables.tycon(*id).variants().len()
    }

    /// Whether a source type owns a counted block anywhere inside it, which is
    /// the same question `middle::rc`'s classifier asks.
    /// The answer is **memoised, and recorded before the descent**, which is
    /// what makes this terminate and what makes it linear in the *distinct*
    /// types a program holds rather than in the paths through them.
    /// `llvm/emit.rs`'s classifier is the same two lines for the same two
    /// reasons: a type graph is a DAG whose nodes are revisited along every
    /// path, and a recursive type reaches itself.
    pub(crate) fn rc_counted(&mut self, ty: &Ty) -> bool {
        if let Some(known) = self.counted_memo.get(ty) {
            return *known;
        }
        self.counted_memo.insert(ty.clone(), false);
        let answer = self.counted_ty(ty, 0);
        self.counted_memo.insert(ty.clone(), answer);
        answer
    }

    fn counted_ty(&mut self, ty: &Ty, depth: u32) -> bool {
        if depth > RC_DEPTH {
            return false;
        }
        match self.layouts_of(ty.clone()).repr {
            // The closure is here because its environment is a heap block this
            // backend allocates and counts (`glue.rs`), which is the same
            // answer `llvm/repr.rs`'s site walk gives `Ty::Fn`.
            Repr::Str | Repr::List | Repr::Closure => true,
            Repr::Aggregate => {
                let fields = field_types(self.tables, ty);
                self.any_counted(ty, &fields, depth)
            }
            Repr::Enum { .. } => {
                let n = self.variant_count(ty);
                (0..n).any(|v| {
                    let fields = variant_types(self.tables, ty, v);
                    self.any_counted(ty, &fields, depth)
                })
            }
            _ => false,
        }
    }

    /// Whether any of `fields` carries a count, where a **boxed** field always
    /// does: the box is a heap block of its own, whoever else owns what is
    /// inside it.
    fn any_counted(&mut self, owner: &Ty, fields: &[Ty], depth: u32) -> bool {
        for f in fields {
            if self.boxes(owner, f) {
                return true;
            }
            if let Some(known) = self.counted_memo.get(f) {
                if *known {
                    return true;
                }
                continue;
            }
            let answer = self.counted_ty(f, depth + 1);
            self.counted_memo.insert(f.clone(), answer);
            if answer {
                return true;
            }
        }
        false
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
        // `buri_rt_str_eq` answers the equality directly; every ordering is a
        // test of `buri_rt_str_compare`'s three-way answer, whose variants are
        // `Less`, `Equal`, `Greater` in that order (`core/order`).
        let (symbol, want): (&'static str, &[u64]) = match op {
            BinOp::Eq => ("buri_rt_str_eq", &[]),
            BinOp::Ne => ("buri_rt_str_eq", &[]),
            BinOp::Lt => ("buri_rt_str_compare", &[LESS]),
            BinOp::Le => ("buri_rt_str_compare", &[LESS, EQUAL]),
            BinOp::Gt => ("buri_rt_str_compare", &[GREATER]),
            BinOp::Ge => ("buri_rt_str_compare", &[GREATER, EQUAL]),
            other => return self.unsupported(format!("Binary {other:?} at Str")),
        };
        if let Err(why) = self.str_compare(st, symbol, a, b, raw) {
            return self.unsupported(why);
        }
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
        // `buri_rt_str_eq` and `buri_rt_str_compare` are on the native side.
        if matches!(prim, Prim::Str | Prim::Template) {
            return self.str_binary(st, dest, op, lhs, rhs);
        }
        let Some((tag, bits, signed)) = prim_tag(prim) else {
            return self.unsupported(format!("Binary at {prim:?}"));
        };
        let name = binop_name(op);
        let d = st.at(dest);
        let a = st.at(lhs);
        let b = st.at(rhs);
        // The paper's stencil configuration: which of constant, register and
        // stack each operand and the result is.
        let la = st.loc(lhs);
        // `folded` and `constants` are `Jit::plan`'s side tables and have one
        // entry per value of the `code` these operands came from, so the
        // defaults are answers no operand here can actually ask for:
        // "materialised" and "not an immediate", which is what a value with no
        // `Const` definition is.
        let lb = if folded(st, rhs) { Loc::Imm } else { st.loc(rhs) };
        let ld = st.loc(dest);
        let key = format!("bin/{name}/{tag}/{}{}/{}", la.tag(), lb.tag(), ld.tag());
        if self.has(&key) {
            let k = constant(st, rhs);
            self.emit(
                &key,
                &[
                    ("JIT_A", V::I(a as u64)),
                    ("JIT_B", V::I(b as u64)),
                    ("JIT_D", V::I(d as u64)),
                    ("JIT_K", V::I(k)),
                    ("JIT_CONT", V::Fall),
                    // A 128-bit divide is a *call* — the runtime owns it
                    // (`sources.rs::wide`) — so its stencil has the
                    // zero-register prototype and names its continuation
                    // `JIT_CONT0`. Binding both costs nothing: `emit` looks a
                    // hole up by name and a stencil that has neither ignores
                    // the other.
                    ("JIT_CONT0", V::Fall),
                ],
            );
            return;
        }
        // Falling back to the frame-only form is only sound when nothing this
        // instruction touches lives in a register.
        if la != Loc::Frame || lb != Loc::Frame || ld != Loc::Frame {
            return self.unsupported(format!("Binary {name} at {tag} with no register variant"));
        }
        let key = format!("bin/{name}/{tag}/ff/f");
        if self.has(&key) {
            self.emit(
                &key,
                &[
                    ("JIT_A", V::I(a as u64)),
                    ("JIT_B", V::I(b as u64)),
                    ("JIT_D", V::I(d as u64)),
                    ("JIT_CONT", V::Fall),
                    ("JIT_CONT0", V::Fall),
                ],
            );
            return;
        }
        // The Base level has only the 64-bit and float stencils, so a narrow
        // operation is an extend, a 64-bit operation and a truncate.
        let (s0, s1, s2) = (st.scratch, st.scratch + 8, st.scratch + 16);
        let ext = if signed { "sext" } else { "zext" };
        self.emit(
            &format!("{ext}/{bits}"),
            &[("JIT_D", V::I(s0 as u64)), ("JIT_A", V::I(a as u64)), ("JIT_CONT", V::Fall)],
        );
        self.emit(
            &format!("{ext}/{bits}"),
            &[("JIT_D", V::I(s1 as u64)), ("JIT_A", V::I(b as u64)), ("JIT_CONT", V::Fall)],
        );
        let wide = if signed { "i64" } else { "u64" };
        let cmp = op.is_comparison();
        let out = if cmp { d } else { s2 };
        self.emit(
            &format!("bin/{name}/{wide}/ff/f"),
            &[
                ("JIT_A", V::I(s0 as u64)),
                ("JIT_B", V::I(s1 as u64)),
                ("JIT_D", V::I(out as u64)),
                ("JIT_CONT", V::Fall),
            ],
        );
        if !cmp {
            self.emit(
                &format!("zext/{bits}"),
                &[("JIT_D", V::I(d as u64)), ("JIT_A", V::I(s2 as u64)), ("JIT_CONT", V::Fall)],
            );
        }
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
        if op == UnOp::Not {
            // The operand and the result may be in CPS registers, exactly as
            // for any other unary operation. Spelling this `f/f` was a real
            // miscompile: the stencil wrote the frame while the consumer read
            // the register.
            let key =
                format!("un/lnot/b/{}/{}", st.loc(arg).tag(), st.loc(dest).tag());
            return self.emit(
                &key,
                &[("JIT_A", V::I(a as u64)), ("JIT_D", V::I(d as u64)), ("JIT_CONT", V::Fall)],
            );
        }
        let Some((tag, bits, signed)) = prim_tag(prim) else {
            return self.unsupported(format!("Unary at {prim:?}"));
        };
        let name = if op == UnOp::Neg { "neg" } else { "bnot" };
        let key = format!("un/{name}/{tag}/{}/{}", st.loc(arg).tag(), st.loc(dest).tag());
        if self.has(&key) {
            return self.emit(
                &key,
                &[("JIT_A", V::I(a as u64)), ("JIT_D", V::I(d as u64)), ("JIT_CONT", V::Fall)],
            );
        }
        if st.loc(arg) != Loc::Frame || st.loc(dest) != Loc::Frame {
            return self.unsupported(format!("Unary {name} at {tag} with no register variant"));
        }
        let key = format!("un/{name}/{tag}/f/f");
        if self.has(&key) {
            return self.emit(
                &key,
                &[("JIT_A", V::I(a as u64)), ("JIT_D", V::I(d as u64)), ("JIT_CONT", V::Fall)],
            );
        }
        let (s0, s2) = (st.scratch, st.scratch + 16);
        let ext = if signed { "sext" } else { "zext" };
        self.emit(
            &format!("{ext}/{bits}"),
            &[("JIT_D", V::I(s0 as u64)), ("JIT_A", V::I(a as u64)), ("JIT_CONT", V::Fall)],
        );
        let wide = if signed { "i64" } else { "u64" };
        self.emit(
            &format!("un/{name}/{wide}/f/f"),
            &[("JIT_A", V::I(s0 as u64)), ("JIT_D", V::I(s2 as u64)), ("JIT_CONT", V::Fall)],
        );
        self.emit(
            &format!("zext/{bits}"),
            &[("JIT_D", V::I(d as u64)), ("JIT_A", V::I(s2 as u64)), ("JIT_CONT", V::Fall)],
        );
        let _ = (prog, code);
    }

    // -- calls -------------------------------------------------------------

    fn call(
        &mut self,
        prog: &ir::Program,
        code: &ir::Code,
        st: &mut Fn2,
        dests: &[ir::ValueId],
        func: u32,
        args: &[ir::ValueId],
    ) {
        // Every backend's `call` does this first, for the same reason: the same
        // `list.*` key reaches a backend two ways — as an `Inst::CallIntrinsic`
        // where the front end spelled it inline, and as an `Inst::Call` to a
        // `Body::Runtime` function where it was a method — and the loop belongs
        // at the call site, where the step is a `MakeClosure` this function can
        // see. `lists.rs` says why. A `false` here falls through to the
        // ordinary call, whose callee's body `list_loop_rt` open-codes
        // in turn.
        if let Some(ir::Body::Runtime(key)) = prog.funcs.get(func as usize).map(|f| &f.body) {
            let key = key.clone();
            if self.list_loop(prog, code, st, dests, &key, args) {
                return;
            }
            if let Some(o) = self.operands(prog, code, st, dests, args) {
                if self.list_extra(prog, st, &key, &o) {
                    return;
                }
            }
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
            // parameter offsets — so a shape refused here is refused there too,
            // and the refusal is the same sentence.
            if inline_runtime_key(&key) {
                if let Some(entry) = super::runtime::entry(&key) {
                    let list: Vec<(u32, ir::Type)> =
                        args.iter().map(|a| (st.at(*a), code.ty_of(*a))).collect();
                    let dest = dests.first().map(|d| (st.at(*d), code.ty_of(*d)));
                    if let Err(why) = self.rt_call(prog, st, entry, dest, &list) {
                        self.unsupported(why);
                    }
                    return;
                }
            }
        }
        let base = st.frame.size;
        let callee = self.frame_sig_of(func as usize);
        for (i, a) in args.iter().enumerate() {
            let Some(off) = callee.params.get(i) else { continue };
            let n = self.slot_bytes_of(prog, code.ty_of(*a));
            self.mv(base + *off, st.at(*a), n);
        }
        self.emit(
            "call",
            &[
                ("JIT_N", V::I(base as u64)),
                ("JIT_P", V::I(base as u64)),
                ("JIT_CALLEE", V::Fn(func)),
                ("JIT_CONT0", V::Fall),
            ],
        );
        for (i, d) in dests.iter().enumerate() {
            let Some(off) = callee.ret.get(i) else { continue };
            let n = self.slot_bytes_of(prog, code.ty_of(*d));
            self.mv(st.at(*d), base + *off, n);
        }
    }

    fn call_indirect(
        &mut self,
        prog: &ir::Program,
        code: &ir::Code,
        st: &mut Fn2,
        dests: &[ir::ValueId],
        callee: ir::ValueId,
        args: &[ir::ValueId],
    ) {
        // The environment is prepended to the argument list as a pointer, so
        // the callee's frame is laid out for `[Ptr] ++ args`.
        let base = st.frame.size;
        let mut at = 0u32;
        let mut rets = Vec::new();
        for d in dests {
            rets.push(at);
            at += self.slot_bytes_of(prog, code.ty_of(*d));
        }
        let env_at = at;
        at += 8;
        let mut params = Vec::new();
        for a in args {
            params.push(at);
            at += self.slot_bytes_of(prog, code.ty_of(*a));
        }
        let c = st.at(callee);
        self.mv(base + env_at, c + 8, 8);
        // `params` and `rets` were pushed one entry per argument and one per
        // destination just above, so zipping them back is the same pairing an
        // index would give.
        for (a, off) in args.iter().zip(&params) {
            let n = self.slot_bytes_of(prog, code.ty_of(*a));
            self.mv(base + *off, st.at(*a), n);
        }
        self.emit(
            "calli",
            &[
                ("JIT_A", V::I(c as u64)),
                ("JIT_N", V::I(base as u64)),
                ("JIT_P", V::I(base as u64)),
                ("JIT_CONT0", V::Fall),
            ],
        );
        for (d, off) in dests.iter().zip(&rets) {
            let n = self.slot_bytes_of(prog, code.ty_of(*d));
            self.mv(st.at(*d), base + *off, n);
        }
    }

    fn intrinsic(
        &mut self,
        prog: &ir::Program,
        code: &ir::Code,
        st: &mut Fn2,
        dests: &[ir::ValueId],
        key: &str,
        args: &[ir::ValueId],
    ) {
        let scr = st.scratch;
        let arg = |st: &Fn2, i: usize| -> u32 {
            args.get(i).map(|v| st.at(*v)).unwrap_or(0)
        };
        match key {
            // A failed assertion is an abort, and the kind is what makes it
            // attributable (`cli/runtime/testing.rs`). Every one of these is
            // the shape `llvm/emit.rs` emits, symbol for symbol, because what
            // `buri test` parses out of the process's output is the runtime's
            // writing and not the backend's.
            "testing_assert.report" => {
                let ok = st.label();
                let brkey = self.arm_key("br/f", "JIT_F");
                self.emit(
                    &brkey,
                    &[
                        ("JIT_A", V::I(arg(st, 0) as u64)),
                        ("JIT_T", V::Blk(ok)),
                        ("JIT_F", V::Fall),
                    ],
                );
                let (p, l) = self.str_arg(arg(st, 1), scr);
                if let Err(why) = self.c_call("buri_rt_abort_assert", st, &[p, l], &[], 0, "v") {
                    self.unsupported(why);
                }
                let here = self.region.code_addr();
                st.place(ok, here);
            }
            "testing_assert.failWith" => {
                let (p, l) = self.str_arg(arg(st, 0), scr);
                if let Err(why) = self.c_call("buri_rt_abort", st, &[p, l], &[], 0, "v") {
                    self.unsupported(why);
                }
            }
            // `failExpected<T, R>(kind, got): R` answers the bottom type: the
            // call does not come back, and the destination the IR gives it is
            // unreachable.
            "testing_assert.failExpectedShown" => {
                let (kp, kl) = self.str_arg(arg(st, 0), scr);
                let (vp, vl) = self.str_arg(arg(st, 1), scr + 8);
                if let Err(why) =
                    self.c_call("buri_rt_test_fail_expected", st, &[kp, kl, vp, vl], &[], 0, "v")
                {
                    self.unsupported(why);
                }
            }
            // `failExpected<T, R>(kind, got): R` where the derive pass declined
            // to generate a `Show` at `T` — an opaque type — so there is
            // nothing to render `got` with and the kind is the whole of what
            // makes the failure attributable. `llvm/emit.rs`'s arm for the key
            // is the same abort, and `middle/derives.rs` is what decides which
            // of the two keys a `.Some`/`.Ok` assertion lowers to.
            //
            // The destination is not written, for the reason
            // `failExpectedShown`'s is not: `buri_rt_abort_assert` does not
            // return, so every instruction the emitter lays down after this one
            // is unreachable. A backend over a verified block-parameter IR
            // binds zeros there; a frame slot needs no such thing.
            "testing_assert.failExpected" => {
                let (kp, kl) = self.str_arg(arg(st, 0), scr);
                if let Err(why) = self.c_call("buri_rt_abort_assert", st, &[kp, kl], &[], 0, "v") {
                    self.unsupported(why);
                }
            }
            "testing_assert.reportShown" => {
                let (kp, kl) = self.str_arg(arg(st, 0), scr);
                let (ap, al) = self.str_arg(arg(st, 1), scr + 8);
                let (ep, el) = self.str_arg(arg(st, 2), scr + 16);
                if let Err(why) = self.c_call(
                    "buri_rt_test_fail_compared",
                    st,
                    &[kp, kl, ap, al, ep, el],
                    &[],
                    0,
                    "v",
                ) {
                    self.unsupported(why);
                }
            }
            // `list.empty()` is two immediates: a null block and a zero count
            // (VALUE-MODEL.md §4). A runtime call for it would allocate
            // nothing and answer two constants.
            "list.empty" => {
                let Some(d) = dests.first().map(|v| st.at(*v)) else { return };
                self.imm_to(d, 0);
                self.imm_to(d + 8, 0);
            }
            // The test allocator is a **handle**, and `TestAlloc.allocate`
            // answers the byte count it was asked for. Both are
            // `llvm/emit.rs`'s arms exactly: `core/testing/context`'s
            // `alloc` reads no state, so the handle is zero and the allocation
            // is the request.
            "testing_context.alloc" | "host_testing.alloc" => {
                let Some(d) = dests.first().map(|v| st.at(*v)) else { return };
                self.imm_to(d, 0);
            }
            "testing_context.TestAlloc.allocate" | "host_testing.TestAlloc.allocate" => {
                let (Some(d), Some(n)) = (
                    dests.first().map(|v| st.at(*v)),
                    args.get(1).map(|v| st.at(*v)),
                ) else {
                    return;
                };
                self.mv(d, n, 8);
            }
            // `str.concat` is a runtime call with no table row, so its
            // arguments are narrowed here rather than flattened: the two
            // `Str`s and nothing else (`rtcall.rs`'s `str_concat`).
            "str.concat" => {
                let Some(d) = dests.first().map(|v| st.at(*v)) else { return };
                let drop = Self::concat_ctx(args.len());
                let mut list: Vec<u32> = Vec::new();
                for (i, a) in args.iter().enumerate() {
                    if drop == Some(i) {
                        continue;
                    }
                    list.push(st.at(*a));
                }
                if let Err(why) = self.str_concat(st, &list, d) {
                    self.unsupported(why);
                }
            }
            _ => {
                if self.list_loop(prog, code, st, dests, key, args) {
                    return;
                }
                if let Some(o) = self.operands(prog, code, st, dests, args) {
                    if self.list_extra(prog, st, key, &o) {
                        return;
                    }
                }
                if self.prim_trait(prog, code, st, dests, key, args) {
                    return;
                }
                if self.bits(st, dests, key, args) {
                    return;
                }
                if let Some(t) = key.strip_prefix("derivePrimHash.") {
                    let Some(d) = dests.first().map(|v| st.at(*v)) else { return };
                    let Some(prim) = prim_of_name(t) else {
                        return self.unsupported(format!("derivePrimHash.{t}"));
                    };
                    return self.hash_prim(st, prim, arg(st, 0), arg(st, 1), d);
                }
                if let Some(t) = key.strip_prefix("derivePrimShow.") {
                    let Some(d) = dests.first().map(|v| st.at(*v)) else { return };
                    let Some(prim) = prim_of_name(t) else {
                        return self.unsupported(format!("derivePrimShow.{t}"));
                    };
                    if let Err(why) = self.show_prim(st, prim, arg(st, 0), d, true) {
                        self.unsupported(why);
                    }
                    return;
                }
                if let Some(t) = key.strip_prefix("derivePrimJson.") {
                    let (Some(dest), Some(a)) =
                        (dests.first().copied(), args.first().copied())
                    else {
                        return;
                    };
                    let Some(prim) = prim_of_name(t) else {
                        return self.unsupported(format!("derivePrimJson.{t}"));
                    };
                    if let Err(why) = self.json_prim(prog, code, st, prim, a, dest) {
                        self.unsupported(why);
                    }
                    return;
                }
                let Some(entry) = super::runtime::entry(key) else {
                    return self.unsupported(format!("CallIntrinsic {key}"));
                };
                let list: Vec<(u32, ir::Type)> =
                    args.iter().map(|a| (st.at(*a), code.ty_of(*a))).collect();
                let dest = dests.first().map(|d| (st.at(*d), code.ty_of(*d)));
                if let Err(why) = self.rt_call(prog, st, entry, dest, &list) {
                    self.unsupported(why);
                }
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
                if self.jump_fused(prog, code, st, x, fall) {
                    return;
                }
                self.edge(prog, code, st, x);
                if fall {
                    self.emit("jump", &[("JIT_T", V::Fall)]);
                } else {
                    self.emit("jump", &[("JIT_T", V::Blk(x.block.0))]);
                }
            }
            Term::Branch { cond, then, else_ } => {
                // (g) Branch mechanics. Two instructions are on the table: the
                // arm that falls through should be the stencil's elidable one,
                // and the second arm's `jump` should fall through to the block
                // laid out next.
                let newbr = true;
                if then.args.is_empty() && else_.args.is_empty() {
                    let ft = newbr && then.block.index() == next;
                    let fe = newbr && else_.block.index() == next;
                    let tv = if ft { V::Fall } else { V::Blk(then.block.0) };
                    let fv = if fe { V::Fall } else { V::Blk(else_.block.0) };
                    let fall = if ft {
                        Some("JIT_T")
                    } else if fe {
                        Some("JIT_F")
                    } else {
                        None
                    };
                    self.cond(st, *cond, plan, tv, fv, fall);
                    return;
                }
                // With edge copies, one arm's copies fall through and the
                // other's sit behind a label; the label goes to the arm whose
                // own target is the next block, so that its jump goes too.
                let y_is_then = newbr && then.block.index() == next;
                let (x, y) = if y_is_then { (else_, then) } else { (then, else_) };
                let lf = st.label();
                if newbr {
                    let (tv, fv) = if y_is_then {
                        (V::Blk(lf), V::Fall)
                    } else {
                        (V::Fall, V::Blk(lf))
                    };
                    let fall = if y_is_then { "JIT_F" } else { "JIT_T" };
                    self.cond(st, *cond, plan, tv, fv, Some(fall));
                } else {
                    self.cond(st, *cond, plan, V::Fall, V::Blk(lf), None);
                }
                self.edge(prog, code, st, x);
                self.emit("jump", &[("JIT_T", V::Blk(x.block.0))]);
                st.place(lf, self.region.code_addr());
                self.edge(prog, code, st, y);
                if newbr && y.block.index() == next {
                    self.emit("jump", &[("JIT_T", V::Fall)]);
                } else {
                    self.emit("jump", &[("JIT_T", V::Blk(y.block.0))]);
                }
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
                                ("JIT_D", V::I(s0 as u64)),
                                ("JIT_A", V::I(off as u64)),
                                ("JIT_N", V::I(w as u64)),
                                ("JIT_P", V::I(u64::from(w == 0))),
                                ("JIT_CONT", V::Fall),
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
                let newbr = true;
                // (l) The total chain, ported from the removed backend's
                // `compare_chain`.
                // `middle::exhaustiveness` has proved the match total and this
                // IR hands every `Switch` over with `default: None`, so once
                // every other arm has been refused the tag *is* the last one:
                // its test, and the `unreachable` behind the chain, are both
                // dead. On the two-arm `Option` match that is 58% of the
                // corpus's switches this halves the dispatch. The belt a
                // defensive-abort profile would keep is `STENCIL_OFF=tag` here.
                let total = default.is_none() && cases.len() > 1;
                let last = cases.len() - 1;
                for (ci, (k, tgt)) in cases.iter().enumerate() {
                    if total && ci == last {
                        self.edge(prog, code, st, tgt);
                        let to =
                            if tgt.block.index() == next { V::Fall } else { V::Blk(tgt.block.0) };
                        self.emit("jump", &[("JIT_T", to)]);
                        return;
                    }
                    let lnext = st.label();
                    let s0 = st.scratch + 32;
                    // (g) The case body falls through, so the test's `JIT_T`
                    // arm is the one that has to be the elidable one.
                    if newbr {
                        let base = match fusedw {
                            Some(f) => f.to_string(),
                            None => "brcmp/eq/u64/fi".to_string(),
                        };
                        let key = self.arm_key(&base, "JIT_T");
                        #[allow(unused_mut)]
                        let mut binds: Vec<(&str, V)> = vec![
                            ("JIT_A", V::I(src as u64)),
                            ("JIT_T", V::Fall),
                            ("JIT_F", V::Blk(lnext)),
                        ];
                        if fusedw.is_some() {
                            binds.push(("JIT_N", V::I(*k)));
                        } else {
                            binds.push(("JIT_K", V::I(*k)));
                        }
                        // (2) The census says `tagbr + jump` is the commonest
                        // adjacent pair in the corpus after the constant
                        // stores: every `match` arm is a tag test followed by a
                        // jump to the arm's block. When the edge carries no
                        // copies the test can name the block itself.
                        let direct = code.get(tgt.block).params.is_empty();
                        if direct {
                            for b in binds.iter_mut() {
                                if b.0 == "JIT_T" {
                                    b.1 = V::Blk(tgt.block.0);
                                }
                            }
                        }
                        self.emit(&key, &binds);
                        if !direct {
                            self.edge(prog, code, st, tgt);
                            self.emit("jump", &[("JIT_T", V::Blk(tgt.block.0))]);
                        }
                        st.place(lnext, self.region.code_addr());
                        continue;
                    }
                    if let Some(fused) = fusedw {
                        self.emit(
                            fused,
                            &[
                                ("JIT_A", V::I(src as u64)),
                                ("JIT_N", V::I(*k)),
                                ("JIT_T", V::Fall),
                                ("JIT_F", V::Blk(lnext)),
                            ],
                        );
                    } else if self.has("brcmp/eq/u64/fi") {
                        self.emit(
                            "brcmp/eq/u64/fi",
                            &[
                                ("JIT_A", V::I(src as u64)),
                                ("JIT_K", V::I(*k)),
                                ("JIT_T", V::Fall),
                                ("JIT_F", V::Blk(lnext)),
                            ],
                        );
                    } else {
                        if self.has("bin/eq/u64/fi/f") {
                            self.emit(
                                "bin/eq/u64/fi/f",
                                &[
                                    ("JIT_A", V::I(src as u64)),
                                    ("JIT_K", V::I(*k)),
                                    ("JIT_D", V::I(s0 as u64)),
                                    ("JIT_CONT", V::Fall),
                                ],
                            );
                        } else {
                            let s1 = st.scratch + 40;
                            self.imm_to(s1, *k);
                            self.emit(
                                "bin/eq/u64/ff/f",
                                &[
                                    ("JIT_A", V::I(src as u64)),
                                    ("JIT_B", V::I(s1 as u64)),
                                    ("JIT_D", V::I(s0 as u64)),
                                    ("JIT_CONT", V::Fall),
                                ],
                            );
                        }
                        self.emit(
                            "br/f",
                            &[
                                ("JIT_A", V::I(s0 as u64)),
                                ("JIT_T", V::Fall),
                                ("JIT_F", V::Blk(lnext)),
                            ],
                        );
                    }
                    self.edge(prog, code, st, tgt);
                    self.emit("jump", &[("JIT_T", V::Blk(tgt.block.0))]);
                    st.place(lnext, self.region.code_addr());
                }
                match default {
                    Some(tgt) => {
                        self.edge(prog, code, st, tgt);
                        self.emit("jump", &[("JIT_T", V::Blk(tgt.block.0))]);
                    }
                    None => self.emit("unreachable", &[]),
                }
            }
            Term::Return(vs) => {
                for (i, v) in vs.iter().enumerate() {
                    let Some(off) = st.frame.ret.get(i).copied() else { continue };
                    let n = self.slot_bytes_of(prog, code.ty_of(*v));
                    self.mv(off, st.at(*v), n);
                }
                self.emit("ret", &[]);
            }
            Term::Unreachable => self.emit("unreachable", &[]),
        }
    }

    /// A branch on a condition: either the `br` stencil reading a materialised
    /// boolean, or — from [`Level::CmpBr`] — the comparison and the branch as
    /// one stencil, which is the paper's Figure 11b supernode.
    /// The stencil key this terminator's test wants.
    fn cond_key(&self, st: &Fn2, cond: ir::ValueId, plan: &Plan) -> Option<String> {
        if let Some((op, prim, lhs, rhs)) = plan.cmpbr {
            let (tag, _, _) = prim_tag(prim)?;
            let la = st.loc(lhs).tag();
            let lb = if folded(st, rhs) { "i".to_string() } else { st.loc(rhs).tag() };
            let k = format!("brcmp/{}/{tag}/{la}{lb}", binop_name(op));
            return self.has(&k).then_some(k);
        }
        let k = format!("br/{}", st.loc(cond).tag());
        if self.has(&k) {
            return Some(k);
        }
        (st.loc(cond) == Loc::Frame && self.has("br/f")).then(|| "br/f".to_string())
    }

    /// The twin of a two-target stencil whose **last** branch is the arm named
    /// by `fall` — the only arm copy-and-patch can elide. See
    /// `extract::swap_arms` for why this is a choice the library has to offer
    /// rather than one the emitter can make by negating the test.
    pub(crate) fn arm_key(&self, base: &str, fall: &str) -> String {
        if self.elidable_arm(base).as_deref() == Some(fall) {
            return base.to_string();
        }
        let sw = format!("{base}+swap");
        if self.elidable_arm(&sw).as_deref() == Some(fall) {
            return sw;
        }
        base.to_string()
    }

    fn cond(&mut self, st: &Fn2, cond: ir::ValueId, plan: &Plan, tv: V, fv: V, fall: Option<&str>) {
        // `cond_key`'s last resort is `br/f`, which every other branch site in
        // this backend emits without asking — `rtcall.rs`, `lists.rs` and the
        // `Switch` chain below all name it directly. A library without it
        // cannot compile a conditional at all, so this is a broken stencil
        // library rather than a program the level declines, and it stops here
        // instead of emitting a fall-through that would run the wrong arm.
        let Some(key) = self.cond_key(st, cond, plan) else {
            crate::diagnostics::ice("stencil: the level has no branch stencil")
        };
        let key = match fall {
            Some(f) => self.arm_key(&key, f),
            None => key,
        };
        if let Some((_, _, lhs, rhs)) = plan.cmpbr {
            let k = constant(st, rhs);
            self.emit(
                &key,
                &[
                    ("JIT_A", V::I(st.at(lhs) as u64)),
                    ("JIT_B", V::I(st.at(rhs) as u64)),
                    ("JIT_K", V::I(k)),
                    ("JIT_T", tv),
                    ("JIT_F", fv),
                ],
            );
            return;
        }
        self.emit(
            &key,
            &[("JIT_A", V::I(st.at(cond) as u64)), ("JIT_T", tv), ("JIT_F", fv)],
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
        if let ir::Type::Agg(id) = code.ty_of(agg) {
            let l = self.layout_of(prog, id);
            if let Repr::Enum { repr, variants } = &l.repr {
                return match repr {
                    EnumRepr::Bare { tag } | EnumRepr::Tagged { tag, .. } => {
                        (base, tag.size(), false)
                    }
                    EnumRepr::Niche { null_at } => (
                        base + null_at,
                        variants.iter().position(|v| v.is_empty()).unwrap_or(1) as u32,
                        true,
                    ),
                };
            }
        }
        (base, 8, false)
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
        if params.iter().any(|p| matches!(st.loc(*p), Loc::Reg(_))) {
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
        let key = format!("movjump/{n}");
        if !self.has(&key) {
            return false;
        }
        let src = ["JIT_A", "JIT_B", "JIT_C", "JIT_E"];
        let dst = ["JIT_D", "JIT_N", "JIT_P", "JIT_Q"];
        let mut binds: Vec<(&str, V)> = Vec::new();
        // `movjump/{n}` has `n` source holes and `n` destination holes, and
        // `n` is `pairs.len()`, which the test above holds to four — so the
        // zip runs out with the pairs and not with the hole names.
        for ((d, s), (sh, dh)) in pairs.iter().zip(src.into_iter().zip(dst)) {
            binds.push((sh, V::I(*s as u64)));
            binds.push((dh, V::I(*d as u64)));
        }
        binds.push(("JIT_T", V::Blk(t.block.0)));
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
            let Loc::Reg(k) = st.loc(*p) else { continue };
            let src = match st.loc(*a) {
                Loc::Reg(j) if j == k => {
                    // The definition wrote the register in place; the frame
                    // slot still needs it when anything reads the slot.
                    if write_through(st, *p) {
                        self.emit(
                            &format!(
                                "{}/r{k}",
                                if code.ty_of(*p) == ir::Type::F64 { "stwg" } else { "stw" }
                            ),
                            &[
                                ("JIT_D", V::I(st.at(*p) as u64)),
                                ("JIT_CONT", V::Fall),
                            ],
                        );
                    }
                    continue;
                }
                Loc::Reg(j) => RSrc::Reg(j),
                _ => RSrc::Slot(st.at(*a)),
            };
            pend.push((k, code.ty_of(*p) == ir::Type::F64, src));
        }
        let mut tmp = st.scratch + 56;
        while !pend.is_empty() {
            let ready = pend
                .iter()
                .position(|(k, _, _)| !pend.iter().any(|(_, _, s)| *s == RSrc::Reg(*k)));
            match ready {
                Some(i) => {
                    let (k, f, src) = pend.remove(i);
                    self.reg_move(k, f, src);
                }
                None => {
                    // A permutation of the register file: break it through a
                    // scratch slot, exactly as the frame half does.
                    //
                    // The loop's own test says `pend` is not empty. `ready` is
                    // `None` only when every entry's register is some entry's
                    // source; the registers are distinct, so covering all of
                    // them takes as many register sources as there are entries
                    // and no entry's source can be a slot. A slot here would
                    // mean the search above missed a move that was ready, and
                    // emitting the permutation anyway would drop it.
                    let Some(&(_, f, src)) = pend.first() else { break };
                    let RSrc::Reg(j) = src else {
                        crate::diagnostics::ice(
                            "stencil: an edge's register copies deadlocked on a slot source",
                        )
                    };
                    self.emit(
                        &format!("{}/r{j}", if f { "stwg" } else { "stw" }),
                        &[("JIT_D", V::I(tmp as u64)), ("JIT_CONT", V::Fall)],
                    );
                    for p in pend.iter_mut() {
                        if p.2 == RSrc::Reg(j) {
                            p.2 = RSrc::Slot(tmp);
                        }
                    }
                    tmp += 8;
                }
            }
        }
        let mut pending: Vec<(u32, u32, u32)> = Vec::new();
        for (p, a) in params.iter().zip(t.args.iter()) {
            if let Loc::Reg(k) = st.loc(*p) {
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
        while !pending.is_empty() {
            match pending.iter().position(|(d, _, _)| !pending.iter().any(|(_, s, _)| s == d)) {
                Some(i) => {
                    let (d, s, n) = pending.remove(i);
                    self.mv(d, s, n);
                }
                None => {
                    // A cycle: break it through a scratch slot. The loop's own
                    // test says there is a first entry to break it at.
                    let Some(&(_, s, n)) = pending.first() else { break };
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
                &format!("{}/r{k}/r{j}", if float { "mvg" } else { "mvr" }),
                &[("JIT_CONT", V::Fall)],
            ),
            RSrc::Slot(o) => self.emit(
                &format!("{}/r{k}", if float { "ldg" } else { "ld" }),
                &[("JIT_A", V::I(o as u64)), ("JIT_CONT", V::Fall)],
            ),
        }
    }

    // -- runtime-supplied bodies -------------------------------------------

    pub(crate) fn runtime_body(
        &mut self,
        prog: &ir::Program,
        fi: usize,
        key: String,
        st: &mut Fn2,
    ) {
        let fs = self.frame_sig_of(fi);
        // `fi` is a member of the unit `Jit::compile_unit` is walking, which
        // `ir::Program::funcs_by_unit` drew from this same table; a function
        // with no signature returns nothing, which is what the arms below that
        // read `nrets` already treat as "not the two-operand shape".
        let nrets = prog.funcs.get(fi).map_or(0, |f| f.sig.rets.len());
        let ret0 = fs.ret.first().copied().unwrap_or(0);
        let p = |i: usize| fs.params.get(i).copied().unwrap_or(0);

        if key == "testing_assert.report" {
            let ok = st.label();
            let brkey = self.arm_key("br/f", "JIT_F");
            self.emit(
                &brkey,
                &[("JIT_A", V::I(p(0) as u64)), ("JIT_T", V::Blk(ok)), ("JIT_F", V::Fall)],
            );
            let (kp, kl) = self.str_arg(p(1), fs.param_end);
            if let Err(why) = self.c_call("buri_rt_abort_assert", st, &[kp, kl], &[], 0, "v") {
                self.unsupported(why);
            }
            let here = self.region.code_addr();
            st.place(ok, here);
            self.emit("ret", &[]);
            return;
        }
        // `failExpected(kind, got)` is the one of this family whose parameters
        // are **not** all `Str`: `got` is the opaque `T` there was no `Show`
        // for, and the abort names the kind alone. It cannot join the loop
        // below, which flattens every parameter as a string.
        if key == "testing_assert.failExpected" {
            let (kp, kl) = self.str_arg(p(0), fs.param_end);
            if let Err(why) = self.c_call("buri_rt_abort_assert", st, &[kp, kl], &[], 0, "v") {
                self.unsupported(why);
            }
            self.emit("ret", &[]);
            return;
        }
        if key == "testing_assert.failWith" || key == "testing_assert.failExpectedShown" {
            let mut args = Vec::new();
            for i in 0..fs.params.len() {
                let (p, l) = self.str_arg(p(i), fs.param_end + i as u32 * 8);
                args.push(p);
                args.push(l);
            }
            let symbol = if key == "testing_assert.failWith" {
                "buri_rt_abort"
            } else {
                "buri_rt_test_fail_expected"
            };
            if let Err(why) = self.c_call(symbol, st, &args, &[], 0, "v") {
                self.unsupported(why);
            }
            self.emit("ret", &[]);
            return;
        }
        if key == "testing_assert.reportShown" {
            let (kp, kl) = self.str_arg(p(0), fs.param_end);
            let (ap, al) = self.str_arg(p(1), fs.param_end + 8);
            let (ep, el) = self.str_arg(p(2), fs.param_end + 16);
            if let Err(why) = self.c_call(
                "buri_rt_test_fail_compared",
                st,
                &[kp, kl, ap, al, ep, el],
                &[],
                0,
                "v",
            ) {
                self.unsupported(why);
            }
            self.emit("ret", &[]);
            return;
        }
        if let Some(t) = key.strip_prefix("num.").and_then(|k| k.strip_suffix(".show")) {
            match prim_of_name(t).ok_or_else(|| format!("num.{t}.show"))
                .and_then(|prim| self.show_prim(st, prim, p(0), ret0, true))
            {
                Ok(()) => self.emit("ret", &[]),
                Err(why) => self.unsupported(why),
            }
            return;
        }
        // `num.<T>.<op>`, which the native backends open-code.
        let parts: Vec<&str> = key.split('.').collect();
        if let ["num", tname, op] = *parts.as_slice() {
            if let Some(prim) = prim_of_name(tname) {
                // `Bounded::minValue` and `Bounded::maxValue`. The type comes
                // from the key — `middle::lower`'s `bounded_key` puts it there,
                // because the methods take no argument and the IR type of the
                // result has lost the signedness that separates `0` from
                // `-128`. The bounds are the **type's**, not JavaScript's
                // exactly-representable ones: `exact_int_range` is the
                // JavaScript backend's business alone.
                if matches!(op, "minValue" | "maxValue") {
                    let low = op == "minValue";
                    match bound_pattern(prim, low) {
                        Some(bits) => {
                            self.imm_num(ret0, prim, bits);
                            self.emit("ret", &[]);
                        }
                        None => self.unsupported(format!("Body::Runtime {key}")),
                    }
                    return;
                }
                // A conversion is named by its target: `toU8`, `wrapToI32`,
                // `toChar`. Between two integers it is a truncation and a
                // re-widening, because a frame slot holds an integer
                // zero-extended at its own width — so the source is
                // sign-extended first where it is signed, and masked to the
                // target's width after.
                if let Some(to) = conversion_target(op) {
                    // An **aggregate** result is the whole of the test, and it
                    // is the same one `llvm/emit.rs::numeric` makes before its
                    // `toChar` arm: an exact
                    // conversion answers the target type and an inexact one
                    // answers `Result<T, RangeError>` (SPEC 6.2.1), so the
                    // result's *shape* says which this is without a second
                    // table. `U32.toChar` is the case that needs it — not every
                    // `U32` is a scalar value — and casting anyway would have
                    // written a `Char` into a `Result` and called it success.
                    let wide = matches!(
                        prog.funcs.get(fi).and_then(|f| f.sig.rets.first()),
                        Some(ir::Type::Agg(_))
                    );
                    let exact = !wide
                        && (op.starts_with("wrapTo")
                            || crate::compiler::semantics::builtins::conversion_is_exact(prim, to)
                            || op == "toChar");
                    // An inexact `toX` answers a `Result` (SPEC 6.2.1), so it
                    // is a range test and two arms rather than a cast.
                    let done = if exact {
                        self.convert(st, prim, to, p(0), ret0)
                    } else {
                        self.convert_checked(prog, fi, st, prim, to, p(0), ret0)
                    };
                    match done {
                        Err(why) => self.unsupported(why),
                        Ok(()) => self.emit("ret", &[]),
                    }
                    return;
                }
                // `Hash::hash` takes no accumulator, so it is the seeded form
                // (`cli/runtime/hash.rs`), and a float hashes through the
                // double it widens to.
                if op == "hash" {
                    let seed = fs.param_end;
                    self.imm_to(seed, HASH_SEED);
                    let (symbol, ints, floats): (&str, Vec<Src>, Vec<Src>) = if prim.is_float() {
                        ("buri_rt_hash_f64", vec![Src::Word(seed)], vec![Src::Word(p(0))])
                    } else {
                        ("buri_rt_mix", vec![Src::Word(seed), Src::Word(p(0))], Vec::new())
                    };
                    if prim == Prim::F32 {
                        self.unsupported(format!("Body::Runtime {key}"));
                        return;
                    }
                    match self.c_call(symbol, st, &ints, &floats, ret0, "i") {
                        Ok(()) => self.emit("ret", &[]),
                        Err(why) => self.unsupported(why),
                    }
                    return;
                }
                // `Checked`, `Saturating`, `Wrapping`, `abs` and `signum`,
                // which `core/num` declares without a body and every backend
                // open-codes. `llvm/emit.rs::numeric` is the twin, and the
                // bound each one checks is the **type's own range** — SPEC
                // 6.2.2 and VALUE-MODEL.md §12 row 2.
                if let Some(kind) = op.strip_prefix("checked") {
                    let ok = self.checked(prog, fi, st, prim, kind, p(0), p(1), ret0);
                    if !ok {
                        self.unsupported(format!("Body::Runtime {key}"));
                    }
                    self.emit("ret", &[]);
                    return;
                }
                if let Some(kind) = op.strip_prefix("saturating") {
                    if !self.saturating(st, prim, kind, p(0), p(1), ret0) {
                        self.unsupported(format!("Body::Runtime {key}"));
                    }
                    self.emit("ret", &[]);
                    return;
                }
                // Wrapping **is** the machine operation: two's complement at
                // the operand's own width is what an `i8` addition already
                // does, and a frame slot holds the answer re-truncated.
                if let Some(kind) = op.strip_prefix("wrapping") {
                    if !self.wrapping(st, prim, kind, p(0), p(1), ret0) {
                        self.unsupported(format!("Body::Runtime {key}"));
                    }
                    self.emit("ret", &[]);
                    return;
                }
                if matches!(op, "abs" | "signum") {
                    if !self.abs_signum(st, prim, op == "abs", p(0), ret0) {
                        self.unsupported(format!("Body::Runtime {key}"));
                    }
                    self.emit("ret", &[]);
                    return;
                }
                // `Ord::compare` answers an `Order`, which is an enum and not a
                // register, so it is taken before the binary-operator table
                // below could refuse it for having no `bin/compare` stencil.
                if op == "compare" {
                    match self.ret_tag(prog, fi) {
                        Some(w) => {
                            self.compare_prim(st, prim, p(0), p(1), ret0, w);
                            self.emit("ret", &[]);
                        }
                        None => self.unsupported(format!("Body::Runtime {key}")),
                    }
                    return;
                }
                if let Some((tag, _, _)) = prim_tag(prim) {
                    let binkey = format!("bin/{}/{tag}/ff/f", op);
                    if nrets == 1 && self.has(&binkey) && fs.params.len() == 2 {
                        self.emit(
                            &binkey,
                            &[
                                ("JIT_A", V::I(p(0) as u64)),
                                ("JIT_B", V::I(p(1) as u64)),
                                ("JIT_D", V::I(ret0 as u64)),
                                ("JIT_CONT", V::Fall),
                                ("JIT_CONT0", V::Fall),
                            ],
                        );
                        self.emit("ret", &[]);
                        return;
                    }
                    if op == "neg" && fs.params.len() == 1 {
                        self.emit(
                            &format!("un/neg/{tag}/f/f"),
                            &[
                                ("JIT_A", V::I(p(0) as u64)),
                                ("JIT_D", V::I(ret0 as u64)),
                                ("JIT_CONT", V::Fall),
                            ],
                        );
                        self.emit("ret", &[]);
                        return;
                    }
                    if op == "toF64" && fs.params.len() == 1 {
                        // `F32::toF64` is a widening, not an integer
                        // conversion, and `F64::toF64` is a copy.
                        if prim == Prim::F64 {
                            self.mv(ret0, p(0), 8);
                            self.emit("ret", &[]);
                            return;
                        }
                        let k = match prim {
                            Prim::F32 => "cvt/f322f",
                            Prim::U8 | Prim::U16 | Prim::U32 | Prim::U64 => "cvt/u2f",
                            _ => "cvt/i2f",
                        };
                        self.emit(
                            k,
                            &[
                                ("JIT_A", V::I(p(0) as u64)),
                                ("JIT_D", V::I(ret0 as u64)),
                                ("JIT_CONT", V::Fall),
                            ],
                        );
                        self.emit("ret", &[]);
                        return;
                    }
                }
            }
        }
        // The open-coded loop first: the runtime call is the fallback, not the
        // other way round. `lists.rs` says why.
        if self.list_loop_rt(prog, fi, &key, st) {
            self.emit("ret", &[]);
            return;
        }
        if let Some(o) = self.rt_operands(prog, fi, &fs) {
            if self.list_extra(prog, st, &key, &o) {
                self.emit("ret", &[]);
                return;
            }
        }
        if self.runtime_intrinsic(prog, fi, &key, &fs, st) {
            self.emit("ret", &[]);
            return;
        }
        if key == "str.eq" || key == "str.compare" {
            let symbol =
                if key == "str.eq" { "buri_rt_str_eq" } else { "buri_rt_str_compare" };
            match self.str_compare(st, symbol, p(0), p(1), ret0) {
                Ok(()) => self.emit("ret", &[]),
                Err(why) => self.unsupported(why),
            }
            return;
        }
        // `str.len` is the number of Unicode scalar values (VALUE-MODEL.md
        // §3.1), which `cli/runtime/text.rs` answers for both the ASCII and the
        // multibyte case. Not in the table because it has no `Ret` shape of its
        // own: the count comes straight back in a register.
        if key == "str.len" {
            let (sp, sl) = self.str_arg(p(0), fs.param_end);
            match self.c_call("buri_rt_str_scalar_len", st, &[sp, sl], &[], ret0, "i") {
                Ok(()) => self.emit("ret", &[]),
                Err(why) => self.unsupported(why),
            }
            return;
        }
        // The test allocator, as a runtime-supplied body rather than as an
        // intrinsic call: `core/testing/context`'s `alloc` reads no state, so
        // the handle is zero and `allocate` answers the request.
        if key == "testing_context.alloc" || key == "host_testing.alloc" {
            self.imm_to(ret0, 0);
            self.emit("ret", &[]);
            return;
        }
        if key == "testing_context.TestAlloc.allocate"
            || key == "host_testing.TestAlloc.allocate"
        {
            self.mv(ret0, p(1), 8);
            self.emit("ret", &[]);
            return;
        }
        if key == "list.len" {
            self.mv(ret0, p(0) + 8, 8);
            self.emit("ret", &[]);
            return;
        }
        // `str.format(ctx, s)` answers its last argument — the template has
        // already been built by `str.concat` — and is `llvm/emit.rs`'s arm
        // exactly: three words copied, and **a count taken**, because the
        // copy is a second name for one block rather than a second reference
        // to it, and without the retain the second drop is a double free.
        if key == "str.format" {
            let Some(last) = fs.params.last().copied() else {
                self.unsupported("Body::Runtime str.format with no argument".into());
                self.emit("ret", &[]);
                return;
            };
            self.mv(ret0, last, 24);
            if !std::env::var("STENCIL_NOFREE").is_ok_and(|v| v == "1") {
                self.emit("incref", &[("JIT_A", V::I(u64::from(ret0))), ("JIT_CONT", V::Fall)]);
            }
            self.emit("ret", &[]);
            return;
        }
        if key == "list.empty" {
            self.imm_to(ret0, 0);
            self.imm_to(ret0 + 8, 0);
            self.emit("ret", &[]);
            return;
        }
        // The three surfaces `Lower::intrinsic` open-codes at a call site,
        // reached here because the same key arrives two ways: spelled inline it
        // is an `Inst::CallIntrinsic`, and spelled as a method it is a call to
        // the `Body::Runtime` function whose body this is. Answering only the
        // first left `char.eq`, `bits.shl` and `str.concat` refused in exactly
        // the files that write them as methods.
        let ret_tag = self.ret_tag(prog, fi);
        if self.prim_trait_at(st, &key, ret0, p(0), p(1), Some(ret_tag)) {
            self.emit("ret", &[]);
            return;
        }
        if self.bits_at(st, &key, ret0, p(0), fs.params.get(1).copied()) {
            self.emit("ret", &[]);
            return;
        }
        if key == "str.concat" {
            let params = prog.funcs.get(fi).map(|f| f.sig.params.clone()).unwrap_or_default();
            let drop = Self::concat_ctx(params.len());
            let mut list: Vec<u32> = Vec::new();
            for i in 0..params.len() {
                if drop == Some(i) {
                    continue;
                }
                list.push(p(i));
            }
            match self.str_concat(st, &list, ret0) {
                Ok(()) => self.emit("ret", &[]),
                Err(why) => self.unsupported(why),
            }
            return;
        }
        self.unsupported(format!("Body::Runtime {key}"));
        self.emit("ret", &[]);
    }
}

impl<'a> Jit<'a> {
    /// A `Body::Runtime` whose key `runtime.rs`'s table has a row for.
    ///
    /// `middle::lower` leaves `core/str`, `core/list`, `core/char`, `core/math`
    /// and the host capabilities as bodyless functions carrying an intrinsic
    /// key, exactly as it does for the other two backends, and the body is one
    /// call to the archive. There is no second implementation of any of them
    /// here: that was the prototype's shape, and it was `libburi_rt.a` written
    /// twice.
    ///
    /// The parameters are already in the frame at the offsets `frame_sigs`
    /// gave them, so the argument list is the signature's types paired with
    /// those offsets — the same list `Lower::intrinsic` builds from an
    /// `Inst::CallIntrinsic`'s values.
    fn runtime_intrinsic(
        &mut self,
        prog: &ir::Program,
        fi: usize,
        key: &str,
        fs: &super::jit::FrameSig,
        st: &mut Fn2,
    ) -> bool {
        let Some(entry) = super::runtime::entry(key) else { return false };
        let Some(f) = prog.funcs.get(fi) else { return false };
        let args: Vec<(u32, ir::Type)> = f
            .sig
            .params
            .iter()
            .enumerate()
            .map(|(i, t)| (fs.params.get(i).copied().unwrap_or(0), *t))
            .collect();
        let dest = f.sig.rets.first().map(|t| (fs.ret.first().copied().unwrap_or(0), *t));
        match self.rt_call(prog, st, entry, dest, &args) {
            Ok(()) => true,
            Err(why) => {
                self.unsupported(why);
                true
            }
        }
    }

    /// One instruction's operands as `lists.rs` wants them: a frame offset and
    /// an IR type apiece, so that the loops are written once and serve both a
    /// call site and a `Body::Runtime` body.
    fn operands(
        &mut self,
        prog: &ir::Program,
        code: &ir::Code,
        st: &Fn2,
        dests: &[ir::ValueId],
        args: &[ir::ValueId],
    ) -> Option<super::lists::Operands> {
        let dest = dests.first()?;
        let _ = prog;
        Some(super::lists::Operands {
            args: args.iter().map(|a| (st.at(*a), code.ty_of(*a))).collect(),
            dest: (st.at(*dest), code.ty_of(*dest)),
        })
    }

    /// The same, for a `Body::Runtime` function whose operands are its own
    /// parameters.
    fn rt_operands(
        &mut self,
        prog: &ir::Program,
        fi: usize,
        fs: &super::jit::FrameSig,
    ) -> Option<super::lists::Operands> {
        let f = prog.funcs.get(fi)?;
        let dest = (fs.ret.first().copied()?, f.sig.rets.first().copied()?);
        let args = f
            .sig
            .params
            .iter()
            .enumerate()
            .map(|(i, t)| (fs.params.get(i).copied().unwrap_or(0), *t))
            .collect();
        Some(super::lists::Operands { args, dest })
    }

    /// The stride of `[T]`'s element, for an IR type that is a `[T]`.
    pub(crate) fn array_stride(&mut self, prog: &ir::Program, t: ir::Type) -> Option<u64> {
        let ir::Type::Agg(id) = t else { return None };
        let ty = prog.type_info(id).ty.clone();
        let crate::compiler::semantics::types::Ty::Array(elem) = ty else { return None };
        Some(u64::from(self.layouts_of(*elem).stride.max(1)))
    }
}

fn prim_of_name(s: &str) -> Option<Prim> {
    Some(match s {
        "Bool" => Prim::Bool,
        "I8" => Prim::I8,
        "I16" => Prim::I16,
        "I32" => Prim::I32,
        "I64" => Prim::I64,
        "U8" => Prim::U8,
        "U16" => Prim::U16,
        "U32" => Prim::U32,
        "U64" => Prim::U64,
        "F32" => Prim::F32,
        "F64" => Prim::F64,
        "Char" => Prim::Char,
        "I128" => Prim::I128,
        "U128" => Prim::U128,
        "Str" => Prim::Str,
        "Template" => Prim::Template,
        _ => return None,
    })
}

impl Jit<'_> {
    /// The `Eq`/`Ord`/`Hash`/`Show` leaves at `Bool` and `Char`, plus
    /// `Char::toU32`. Answers whether it handled the key.
    ///
    /// `llvm/emit.rs` emits the same arms for the same keys — these are four
    /// *language* answers and two backends must not give different ones.
    fn prim_trait(
        &mut self,
        prog: &ir::Program,
        code: &ir::Code,
        st: &mut Fn2,
        dests: &[ir::ValueId],
        key: &str,
        args: &[ir::ValueId],
    ) -> bool {
        let Some(d) = dests.first().map(|v| st.at(*v)) else { return false };
        let a = args.first().map(|v| st.at(*v)).unwrap_or(0);
        let b = args.get(1).map(|v| st.at(*v)).unwrap_or(0);
        let tag = dests.first().and_then(|v| match code.ty_of(*v) {
            ir::Type::Agg(id) => Some(self.tag_width(prog, id)),
            _ => None,
        });
        self.prim_trait_at(st, key, d, a, b, tag)
    }

    /// [`Jit::prim_trait`] with its operands as frame offsets.
    ///
    /// `tag` is the destination's tag width where the destination is an enum —
    /// `compare` answers an `Order` — `Some(None)` where it is an aggregate
    /// that is not one, and `None` where the caller has no aggregate
    /// destination at all.
    pub(crate) fn prim_trait_at(
        &mut self,
        st: &mut Fn2,
        key: &str,
        d: u32,
        a: u32,
        b: u32,
        tag: Option<Option<u32>>,
    ) -> bool {
        let Some((module, op)) = key.split_once('.') else { return false };
        let prim = match module {
            "bool" => Prim::Bool,
            "char" => Prim::Char,
            "str" => Prim::Str,
            _ => return false,
        };
        match op {
            // `show` is `$str`, not `$show`: the trait method renders the value
            // and the *derived* one quotes it.
            "show" => {
                if let Err(why) = self.show_prim(st, prim, a, d, false) {
                    self.unsupported(why);
                }
                true
            }
            // A frame slot holds a `Bool` and a `Char` zero-extended, so both
            // compare at sixty-four bits whatever their own width is, and
            // `false` sorting before `true` is what an unsigned compare says.
            "eq" if prim != Prim::Str => {
                self.emit(
                    "bin/eq/u64/ff/f",
                    &[
                        ("JIT_D", V::I(u64::from(d))),
                        ("JIT_A", V::I(u64::from(a))),
                        ("JIT_B", V::I(u64::from(b))),
                        ("JIT_CONT", V::Fall),
                    ],
                );
                true
            }
            "compare" if prim != Prim::Str => {
                let raw = st.scratch + super::rtcall::RAW_WORD * 8;
                let entry: &[(&str, u64)] =
                    &[("bin/lt/u64/ff/f", LESS), ("bin/gt/u64/ff/f", GREATER)];
                // `Less`, `Equal`, `Greater` in declaration order, which is
                // what `middle::layout` gives a three-variant enum as a bare
                // tag: `Equal` unless one of the two tests says otherwise.
                self.imm_to(raw, EQUAL);
                for (k, v) in entry {
                    let scr = st.scratch + super::rtcall::SPARE_WORD * 8;
                    self.emit(
                        k,
                        &[
                            ("JIT_D", V::I(u64::from(scr))),
                            ("JIT_A", V::I(u64::from(a))),
                            ("JIT_B", V::I(u64::from(b))),
                            ("JIT_CONT", V::Fall),
                        ],
                    );
                    let skip = st.label();
                    let brkey = self.arm_key("br/f", "JIT_T");
                    self.emit(
                        &brkey,
                        &[
                            ("JIT_A", V::I(u64::from(scr))),
                            ("JIT_T", V::Fall),
                            ("JIT_F", V::Blk(skip)),
                        ],
                    );
                    self.imm_to(raw, *v);
                    let here = self.region.code_addr();
                    st.place(skip, here);
                }
                match tag.unwrap_or(Some(8)) {
                    Some(w) => self.store_w(d, raw, w),
                    None => self.unsupported(format!("{key} into a destination that is not a tag")),
                }
                true
            }
            // `$hashInto` at a `Bool` or a `Char`, from `core/order`'s offset
            // basis. A `Char` is one *string* of one character, so an astral
            // scalar is two mixes and the runtime owns that; a `Bool` is one
            // mix of its low word.
            "hash" if prim != Prim::Str => {
                let symbol =
                    if prim == Prim::Char { "buri_rt_hash_char" } else { "buri_rt_mix" };
                let seed = st.scratch + super::rtcall::SPARE_WORD * 8;
                self.imm_to(seed, HASH_SEED);
                let args = [Src::Word(seed), Src::Word(a)];
                if let Err(why) = self.c_call(symbol, st, &args, &[], d, "i") {
                    self.unsupported(why);
                }
                true
            }
            // `Char::toU32` is the representation: a `Char` **is** a `U32`
            // holding a Unicode scalar value.
            "toU32" if prim == Prim::Char => {
                self.mv(d, a, 8);
                true
            }
            _ => false,
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
        (st.scratch + super::rtcall::RAW_WORD * 8, st.scratch + super::rtcall::SPARE_WORD * 8)
    }

    /// `chk/<op>/<tag>` — the result and the overflow flag. Answers `false`
    /// for an operation or a width there is no stencil for.
    fn overflowing(&mut self, st: &Fn2, prim: Prim, kind: &str, a: u32, b: u32) -> bool {
        let Some((tag, _, _)) = prim_tag(prim) else { return false };
        if !prim.is_integer() {
            return false;
        }
        let name = match kind {
            "Add" => "add",
            "Sub" => "sub",
            "Mul" => "mul",
            "Div" => "div",
            _ => return false,
        };
        let key = format!("chk/{name}/{tag}");
        if !self.has(&key) {
            return false;
        }
        let (res, flag) = Self::overflow_slots(st);
        self.emit(
            &key,
            &[
                ("JIT_A", V::I(u64::from(a))),
                ("JIT_B", V::I(u64::from(b))),
                ("JIT_D", V::I(u64::from(res))),
                ("JIT_N", V::I(u64::from(flag))),
                ("JIT_CONT", V::Fall),
                ("JIT_CONT0", V::Fall),
            ],
        );
        true
    }

    /// `checkedAdd`, `checkedSub`, `checkedMul`, `checkedDiv` — an `Option<T>`.
    ///
    /// `Div` is where "the type's own range" is not the same statement as "the
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
        fi: usize,
        st: &mut Fn2,
        prim: Prim,
        kind: &str,
        a: u32,
        b: u32,
        dest: u32,
    ) -> bool {
        let Some((_, bits, _)) = prim_tag(prim) else { return false };
        let Some(ir::Type::Agg(id)) = prog.funcs.get(fi).and_then(|f| f.sig.rets.first().copied())
        else {
            return false;
        };
        let l = self.layout_of(prog, id);
        if !self.overflowing(st, prim, kind, a, b) {
            return false;
        }
        let (res, flag) = Self::overflow_slots(st);
        let pay = super::lists::payload_at(&l, 0);
        let some = st.label();
        let done = st.label();
        let key = self.arm_key("brcmp/eq/u64/fi", "JIT_T");
        self.emit(
            &key,
            &[
                ("JIT_A", V::I(u64::from(flag))),
                ("JIT_K", V::I(0)),
                ("JIT_T", V::Blk(some)),
                ("JIT_F", V::Fall),
            ],
        );
        self.store_disc(&l, dest, 1);
        self.emit("jump", &[("JIT_T", V::Blk(done))]);
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
        fi: usize,
        st: &mut Fn2,
        from: Prim,
        to: Prim,
        src: u32,
        dest: u32,
    ) -> Result<(), String> {
        let refuse = || format!("Body::Runtime num.{}.to{}", from.name(), to.name());
        // Integers only. A float source has `NaN` and the infinities to answer
        // for, and `Char` is a set of scalar values rather than a range.
        if !from.is_integer() || !to.is_integer() {
            return Err(refuse());
        }
        let (Some((tag, _, _)), Some((from_lo, from_hi)), Some((to_lo, to_hi))) =
            (prim_tag(from), from.int_range(), to.int_range())
        else {
            return Err(refuse());
        };
        let Some(ir::Type::Agg(id)) = prog.funcs.get(fi).and_then(|f| f.sig.rets.first().copied())
        else {
            return Err(refuse());
        };
        let ty = prog.type_info(id).ty.clone();
        let Ty::Con(_, arguments) = &ty else { return Err(refuse()) };
        let Some(error_ty) = arguments.get(1).cloned() else { return Err(refuse()) };
        let result = self.layout_of_type(ty.clone());
        // A niche layout puts both payloads at the same offset, and this writes
        // one of two payloads — so it is refused rather than guessed at.
        if !matches!(
            result.repr,
            Repr::Enum { repr: EnumRepr::Bare { .. } | EnumRepr::Tagged { .. }, .. }
        ) {
            return Err(refuse());
        }
        let error = self.layout_of_type(error_ty);
        if error.fields.len() != 2 {
            return Err(refuse());
        }
        let ok_at = super::lists::payload_at(&result, 0);
        let Some(err_at) = result.variant(1).first().copied() else { return Err(refuse()) };

        let bound = st.scratch + super::rtcall::SPARE_WORD * 8;
        let verdict = st.scratch;
        let err = st.label();
        let done = st.label();
        for (op, needed, pattern) in
            [("lt", to_lo > from_lo, to_lo as u128), ("gt", to_hi < from_hi, to_hi)]
        {
            if !needed {
                continue;
            }
            self.imm_num(bound, from, pattern);
            self.emit(
                &format!("bin/{op}/{tag}/ff/f"),
                &[
                    ("JIT_D", V::I(u64::from(verdict))),
                    ("JIT_A", V::I(u64::from(src))),
                    ("JIT_B", V::I(u64::from(bound))),
                    ("JIT_CONT", V::Fall),
                ],
            );
            let brkey = self.arm_key("br/f", "JIT_F");
            self.emit(
                &brkey,
                &[
                    ("JIT_A", V::I(u64::from(verdict))),
                    ("JIT_T", V::Blk(err)),
                    ("JIT_F", V::Fall),
                ],
            );
        }
        self.convert(st, from, to, src, dest + ok_at)?;
        self.store_disc(&result, dest, 0);
        self.emit("jump", &[("JIT_T", V::Blk(done))]);

        let here = self.region.code_addr();
        st.place(err, here);
        self.show_prim(st, from, src, dest + err_at + error.field(0), false)?;
        self.str_literal(dest + err_at + error.field(1), to.name().as_bytes());
        self.store_disc(&result, dest, 1);
        let here = self.region.code_addr();
        st.place(done, here);
        Ok(())
    }

    /// `saturatingAdd`, `saturatingSub`, `saturatingMul`.
    ///
    /// The overflow is detected and the **end** it ran off is chosen, which is
    /// the same answer a wider type would give without there being one. Which
    /// end is a property of the operands' signs, not of the wrapped result —
    /// the wrapped result is precisely the thing that is wrong.
    fn saturating(
        &mut self,
        st: &mut Fn2,
        prim: Prim,
        kind: &str,
        a: u32,
        b: u32,
        dest: u32,
    ) -> bool {
        let Some((tag, _, signed)) = prim_tag(prim) else { return false };
        // Division cannot saturate: its only failures are a zero divisor and
        // `MIN / -1`, and neither has an end to run off.
        if kind == "Div" || !self.overflowing(st, prim, kind, a, b) {
            return false;
        }
        let (res, flag) = Self::overflow_slots(st);
        let (Some(lo), Some(hi)) = (bound_pattern(prim, true), bound_pattern(prim, false)) else {
            return false;
        };
        let ok = st.label();
        let done = st.label();
        let key = self.arm_key("brcmp/eq/u64/fi", "JIT_T");
        self.emit(
            &key,
            &[
                ("JIT_A", V::I(u64::from(flag))),
                ("JIT_K", V::I(0)),
                ("JIT_T", V::Blk(ok)),
                ("JIT_F", V::Fall),
            ],
        );
        if !signed {
            // Unsigned: an addition or a multiplication can only run off the
            // top, and a subtraction only off the bottom.
            self.imm_num(dest, prim, if kind == "Sub" { lo } else { hi });
        } else {
            // The sum of two operands that overflowed is negative exactly when
            // they were both positive, so the sign of `x` decides — and for
            // `Sub` the same test is right for the same reason, because the
            // only way to underflow is a negative `x` against a positive `y`.
            // A product runs off the bottom when the signs differ.
            // Past `overflow_slots`' two words, which hold the result and the
            // flag this branch was reached on.
            let scr = st.scratch + (super::rtcall::RAW_WORD + 1) * 8;
            self.lt_zero(scr, a, tag);
            if kind == "Mul" {
                let other = scr + 8;
                self.lt_zero(other, b, tag);
                self.emit(
                    "bin/xor/u64/ff/f",
                    &[
                        ("JIT_D", V::I(u64::from(scr))),
                        ("JIT_A", V::I(u64::from(scr))),
                        ("JIT_B", V::I(u64::from(other))),
                        ("JIT_CONT", V::Fall),
                    ],
                );
            }
            let top = st.label();
            let brkey = self.arm_key("br/f", "JIT_T");
            self.emit(
                &brkey,
                &[
                    ("JIT_A", V::I(u64::from(scr))),
                    ("JIT_T", V::Fall),
                    ("JIT_F", V::Blk(top)),
                ],
            );
            self.imm_num(dest, prim, lo);
            self.emit("jump", &[("JIT_T", V::Blk(done))]);
            let here = self.region.code_addr();
            st.place(top, here);
            self.imm_num(dest, prim, hi);
        }
        self.emit("jump", &[("JIT_T", V::Blk(done))]);
        let here = self.region.code_addr();
        st.place(ok, here);
        self.mv(dest, res, if prim.bits() > 64 { 16 } else { 8 });
        let here = self.region.code_addr();
        st.place(done, here);
        true
    }

    /// `frame[d] = frame[a] < 0`, at the operand's own signedness.
    ///
    /// At sixteen bytes there is no immediate stencil — `_JIT_K` is one word —
    /// so the sign test is a signed compare of the **high** half, which is the
    /// same question one instruction narrower.
    fn lt_zero(&mut self, d: u32, a: u32, tag: &str) {
        let (key, at) = if tag == "i128" || tag == "u128" {
            (String::from("bin/lt/i64/fi/f"), a + 8)
        } else {
            (format!("bin/lt/{tag}/fi/f"), a)
        };
        self.emit(
            &key,
            &[
                ("JIT_D", V::I(u64::from(d))),
                ("JIT_A", V::I(u64::from(at))),
                ("JIT_K", V::I(0)),
                ("JIT_CONT", V::Fall),
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

    /// `wrappingAdd`, `wrappingSub`, `wrappingMul` — the machine operation at
    /// the operand's own width, which is what the ordinary stencil already is.
    fn wrapping(
        &mut self,
        st: &mut Fn2,
        prim: Prim,
        kind: &str,
        a: u32,
        b: u32,
        dest: u32,
    ) -> bool {
        let Some((tag, _, _)) = prim_tag(prim) else { return false };
        let name = match kind {
            "Add" => "add",
            "Sub" => "sub",
            "Mul" => "mul",
            _ => return false,
        };
        let key = format!("bin/{name}/{tag}/ff/f");
        if !self.has(&key) {
            return false;
        }
        let _ = st;
        self.emit(
            &key,
            &[
                ("JIT_A", V::I(u64::from(a))),
                ("JIT_B", V::I(u64::from(b))),
                ("JIT_D", V::I(u64::from(dest))),
                ("JIT_CONT", V::Fall),
            ],
        );
        true
    }

    /// `abs` and `signum`.
    ///
    /// `abs` of a signed minimum overflows, and overflow is undefined
    /// (SPEC 6.2), so there is nothing to check. A float's is the sign bit
    /// cleared, which is exact for every value including the infinities and
    /// leaves a NaN a NaN.
    fn abs_signum(&mut self, st: &mut Fn2, prim: Prim, abs: bool, a: u32, dest: u32) -> bool {
        let Some((tag, bits, signed)) = prim_tag(prim) else { return false };
        let mask = if bits >= 64 { u64::MAX } else { (1u64 << bits) - 1 };
        // A sign test at sixteen bytes reads the high half, which the `signum`
        // comparisons below cannot: they are `bin/{lt,gt}/{tag}/fi/f` and there
        // is no immediate form that wide. So it goes through the same wide
        // compare against a materialised zero.
        if abs {
            if prim.is_float() {
                let sign = if prim == Prim::F32 { 0x7fff_ffffu64 } else { !(1u64 << 63) };
                self.emit(
                    "bin/and/u64/fi/f",
                    &[
                        ("JIT_D", V::I(u64::from(dest))),
                        ("JIT_A", V::I(u64::from(a))),
                        ("JIT_K", V::I(sign)),
                        ("JIT_CONT", V::Fall),
                    ],
                );
                return true;
            }
            self.mv(dest, a, if bits > 64 { 16 } else { 8 });
            if !signed {
                return true;
            }
            let scr = st.scratch + super::rtcall::SPARE_WORD * 8;
            self.lt_zero(scr, a, tag);
            let skip = st.label();
            let brkey = self.arm_key("br/f", "JIT_T");
            self.emit(
                &brkey,
                &[
                    ("JIT_A", V::I(u64::from(scr))),
                    ("JIT_T", V::Fall),
                    ("JIT_F", V::Blk(skip)),
                ],
            );
            self.emit(
                &format!("un/neg/{tag}/f/f"),
                &[
                    ("JIT_A", V::I(u64::from(a))),
                    ("JIT_D", V::I(u64::from(dest))),
                    ("JIT_CONT", V::Fall),
                ],
            );
            let here = self.region.code_addr();
            st.place(skip, here);
            return true;
        }
        // `signum`: zero, then one test per end. A `Float`'s answers are
        // `1.0`, `-1.0` and `0.0`, so a NaN — which is neither above nor
        // below — answers zero, exactly as the two `select`s `llvm/emit.rs`
        // chains do.
        let (zero, one, minus) = if prim == Prim::F64 {
            (0u128, u128::from(1.0f64.to_bits()), u128::from((-1.0f64).to_bits()))
        } else if prim == Prim::F32 {
            (0, u128::from(1.0f32.to_bits()), u128::from((-1.0f32).to_bits()))
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
            let key = format!("bin/{op}/{tag}/{}/f", if wide { "ff" } else { "fi" });
            self.emit(
                &key,
                &[
                    ("JIT_D", V::I(u64::from(scr))),
                    ("JIT_A", V::I(u64::from(a))),
                    ("JIT_B", V::I(u64::from(zslot))),
                    ("JIT_K", V::I(0)),
                    ("JIT_CONT", V::Fall),
                ],
            );
            let skip = st.label();
            let brkey = self.arm_key("br/f", "JIT_T");
            self.emit(
                &brkey,
                &[
                    ("JIT_A", V::I(u64::from(scr))),
                    ("JIT_T", V::Fall),
                    ("JIT_F", V::Blk(skip)),
                ],
            );
            self.imm_num(dest, prim, v);
            let here = self.region.code_addr();
            st.place(skip, here);
        }
        true
    }

    /// `Ord::compare` at a primitive: `Less`, `Equal`, `Greater` in
    /// declaration order, which is what `middle::layout` gives `core/order`'s
    /// three-variant enum as a bare tag.
    ///
    /// Two tests of the operands rather than one three-way instruction, because
    /// `Equal` is the answer neither test claims — and the tests are at the
    /// operand's *own* type, so a signed compare is signed and a `Float`'s is
    /// `fcmp`.
    fn compare_prim(&mut self, st: &mut Fn2, prim: Prim, a: u32, b: u32, dest: u32, w: u32) {
        let Some((tag, _, _)) = prim_tag(prim) else {
            return self.unsupported(format!("Ord::compare at `{}`", prim.name()));
        };
        let raw = st.scratch + super::rtcall::RAW_WORD * 8;
        self.imm_to(raw, EQUAL);
        for (op, v) in [("lt", LESS), ("gt", GREATER)] {
            let scr = st.scratch + super::rtcall::SPARE_WORD * 8;
            self.emit(
                &format!("bin/{op}/{tag}/ff/f"),
                &[
                    ("JIT_D", V::I(u64::from(scr))),
                    ("JIT_A", V::I(u64::from(a))),
                    ("JIT_B", V::I(u64::from(b))),
                    ("JIT_CONT", V::Fall),
                ],
            );
            let skip = st.label();
            let brkey = self.arm_key("br/f", "JIT_T");
            self.emit(
                &brkey,
                &[
                    ("JIT_A", V::I(u64::from(scr))),
                    ("JIT_T", V::Fall),
                    ("JIT_F", V::Blk(skip)),
                ],
            );
            self.imm_to(raw, v);
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
        let r = match prim {
            P::Str | P::Template => {
                let raw = st.scratch + super::rtcall::SPARE_WORD * 8;
                let _ = raw;
                self.c_call(
                    "buri_rt_hash_str",
                    st,
                    &[Src::Word(acc), Src::Word(v), Src::Word(v + 8), Src::Word(v + 16)],
                    &[],
                    dest,
                    "i",
                )
            }
            P::Char => self.c_call(
                "buri_rt_hash_char",
                st,
                &[Src::Word(acc), Src::Word(v)],
                &[],
                dest,
                "i",
            ),
            P::F32 | P::F64 => {
                let wide = if prim == P::F32 {
                    let scr = st.scratch + super::rtcall::SPARE_WORD * 8;
                    self.emit(
                        "cvt/f322f",
                        &[
                            ("JIT_D", V::I(u64::from(scr))),
                            ("JIT_A", V::I(u64::from(v))),
                            ("JIT_CONT", V::Fall),
                        ],
                    );
                    scr
                } else {
                    v
                };
                self.c_call(
                    "buri_rt_hash_f64",
                    st,
                    &[Src::Word(acc)],
                    &[Src::Word(wide)],
                    dest,
                    "i",
                )
            }
            // `$mix` is handed `ToUint32` of a value that is already an
            // integer, and a frame slot holds one zero-extended at its own
            // width — so the low word is already what the `u32` parameter
            // reads out of `w1`.
            _ => self.c_call(
                "buri_rt_mix",
                st,
                &[Src::Word(acc), Src::Word(v)],
                &[],
                dest,
                "i",
            ),
        };
        if let Err(why) = r {
            self.unsupported(why);
        }
    }

    /// One numeric conversion, between two types a frame slot can hold.
    ///
    /// Integer to integer is the interesting case and it is two steps: widen
    /// the *source* if it is signed, because a slot holds it zero-extended and
    /// the bits above its own width are zero rather than its sign; then narrow
    /// to the target, which is the same zero-extension at the new width. A
    /// 128-bit type on either side is refused, because it does not fit a slot.
    fn convert(
        &mut self,
        st: &Fn2,
        from: Prim,
        to: Prim,
        src: u32,
        dest: u32,
    ) -> Result<(), String> {
        let (Some((ftag, fw, fsigned)), Some((ttag, tw, _))) = (prim_tag(from), prim_tag(to))
        else {
            return Err(format!("a conversion at `{}` or `{}`", from.name(), to.name()));
        };
        // The sixteen-byte types, first, because everything below assumes a
        // value that fits one frame word.
        if fw == 128 || tw == 128 {
            if fw == 128 && tw == 128 {
                self.mv(dest, src, 16);
                return Ok(());
            }
            if tw == 128 {
                if from.is_float() {
                    return Err(format!("a conversion from `{}`", from.name()));
                }
                let wide = st.scratch + super::rtcall::SPARE_WORD * 8;
                let at = self.widen(src, wide, fw, fsigned);
                self.emit(
                    &format!("cvt/to/{ttag}/{}", if fsigned { "i" } else { "u" }),
                    &[
                        ("JIT_D", V::I(u64::from(dest))),
                        ("JIT_A", V::I(u64::from(at))),
                        ("JIT_CONT", V::Fall),
                    ],
                );
                return Ok(());
            }
            let key = if to.is_float() {
                format!("cvt/from/{ftag}/f")
            } else {
                format!("cvt/from/{ftag}/{tw}")
            };
            self.emit(
                &key,
                &[
                    ("JIT_D", V::I(u64::from(dest))),
                    ("JIT_A", V::I(u64::from(src))),
                    ("JIT_CONT", V::Fall),
                ],
            );
            if to == Prim::F32 {
                self.emit(
                    "cvt/f2f32",
                    &[
                        ("JIT_D", V::I(u64::from(dest))),
                        ("JIT_A", V::I(u64::from(dest))),
                        ("JIT_CONT", V::Fall),
                    ],
                );
            }
            return Ok(());
        }
        match (from.is_float(), to.is_float()) {
            (true, true) => {
                let key = if to == Prim::F32 { "cvt/f2f32" } else { "cvt/f322f" };
                if from == to {
                    self.mv(dest, src, 8);
                    return Ok(());
                }
                self.emit(
                    key,
                    &[
                        ("JIT_D", V::I(u64::from(dest))),
                        ("JIT_A", V::I(u64::from(src))),
                        ("JIT_CONT", V::Fall),
                    ],
                );
                Ok(())
            }
            (false, true) => {
                let wide = st.scratch + super::rtcall::SPARE_WORD * 8;
                let at = self.widen(src, wide, fw, fsigned);
                let key = if fsigned { "cvt/i2f" } else { "cvt/u2f" };
                self.emit(
                    key,
                    &[
                        ("JIT_D", V::I(u64::from(dest))),
                        ("JIT_A", V::I(u64::from(at))),
                        ("JIT_CONT", V::Fall),
                    ],
                );
                if to == Prim::F32 {
                    self.emit(
                        "cvt/f2f32",
                        &[
                            ("JIT_D", V::I(u64::from(dest))),
                            ("JIT_A", V::I(u64::from(dest))),
                            ("JIT_CONT", V::Fall),
                        ],
                    );
                }
                Ok(())
            }
            // Float to integer rounds toward zero, and out of range is
            // undefined in C but not in this language: the shape needs the
            // clamp `llvm/emit.rs::float_to_int` emits, and it is not here.
            (true, false) => Err(format!("a conversion from `{}`", from.name())),
            (false, false) => {
                let wide = st.scratch + super::rtcall::SPARE_WORD * 8;
                let at = self.widen(src, wide, fw, fsigned);
                if tw >= 64 {
                    self.mv(dest, at, 8);
                } else {
                    self.emit(
                        &format!("zext/{tw}"),
                        &[
                            ("JIT_D", V::I(u64::from(dest))),
                            ("JIT_A", V::I(u64::from(at))),
                            ("JIT_CONT", V::Fall),
                        ],
                    );
                }
                Ok(())
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
            &format!("sext/{bits}"),
            &[
                ("JIT_D", V::I(u64::from(scratch))),
                ("JIT_A", V::I(u64::from(src))),
                ("JIT_CONT", V::Fall),
            ],
        );
        scratch
    }

    /// The tag width of a function's first result, where that result is an
    /// enum. `None` covers both "not an aggregate" and "an aggregate that is
    /// not an enum", which is what the caller declines on either way.
    fn ret_tag(&mut self, prog: &ir::Program, fi: usize) -> Option<u32> {
        match prog.funcs.get(fi).and_then(|f| f.sig.rets.first().copied()) {
            Some(ir::Type::Agg(id)) => self.tag_width(prog, id),
            _ => None,
        }
    }

    /// The tag width of an enum destination, or `None` when it is not one.
    fn tag_width(&mut self, prog: &ir::Program, id: ir::TypeId) -> Option<u32> {
        match &self.layout_of(prog, id).repr {
            Repr::Enum { repr: EnumRepr::Bare { tag }, .. }
            | Repr::Enum { repr: EnumRepr::Tagged { tag, .. }, .. } => Some(tag.size()),
            _ => None,
        }
    }

    /// `core/bits`, open-coded.
    ///
    /// Every one is a single machine instruction behind a range check, which is
    /// why none is a runtime call. The check is the whole of what is not the
    /// instruction: `$shiftCount` aborts on a count that is negative or at or
    /// beyond the operand's width, and `cli/tests/crash/shift_*` pins the
    /// message — so the check is unconditional and the abort is the runtime's
    /// shared one. A machine shift *masks* the count instead, which is a
    /// different answer rather than an undefined one and would be silently
    /// wrong.
    fn bits(
        &mut self,
        st: &mut Fn2,
        dests: &[ir::ValueId],
        key: &str,
        args: &[ir::ValueId],
    ) -> bool {
        let Some(d) = dests.first().map(|v| st.at(*v)) else { return false };
        let a = args.first().map(|v| st.at(*v)).unwrap_or(0);
        let b = args.get(1).map(|v| st.at(*v));
        self.bits_at(st, key, d, a, b)
    }

    /// [`Jit::bits`] with its operands as frame offsets, so that the same
    /// sequence serves an `Inst::CallIntrinsic` and the `Body::Runtime`
    /// function `middle::lower` leaves when the same key was spelled as a
    /// method call.
    pub(crate) fn bits_at(
        &mut self,
        st: &mut Fn2,
        key: &str,
        d: u32,
        a: u32,
        count: Option<u32>,
    ) -> bool {
        let Some(op) = key.strip_prefix("bits.") else { return false };
        // The operand's width comes from the key's suffix; the bare names are
        // `Int`, which is sixty-four bits.
        let (width, unsigned) = match op {
            _ if op.ends_with("U8") => (8u32, true),
            _ if op.ends_with("U32") => (32, true),
            _ if op.ends_with("U64") => (64, true),
            _ => (64, false),
        };
        let stem = op.trim_end_matches(|c: char| c.is_ascii_digit()).trim_end_matches('U');
        if matches!(stem, "popCount" | "leadingZeros" | "trailingZeros") {
            let k = format!("bits/{stem}/{width}");
            if !self.has(&k) {
                self.unsupported(format!("CallIntrinsic {key}"));
                return true;
            }
            self.emit(
                &k,
                &[
                    ("JIT_D", V::I(u64::from(d))),
                    ("JIT_A", V::I(u64::from(a))),
                    ("JIT_CONT", V::Fall),
                ],
            );
            return true;
        }
        let Some(count) = count else { return false };
        self.shift_count_check(st, count, width);
        let key2 = format!("bits/{stem}/{width}");
        if !self.has(&key2) {
            self.unsupported(format!("CallIntrinsic {key}"));
            return true;
        }
        let _ = unsigned;
        self.emit(
            &key2,
            &[
                ("JIT_D", V::I(u64::from(d))),
                ("JIT_A", V::I(u64::from(a))),
                ("JIT_B", V::I(u64::from(count))),
                ("JIT_CONT", V::Fall),
            ],
        );
        true
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
                ("JIT_D", V::I(u64::from(scr))),
                ("JIT_A", V::I(u64::from(count))),
                ("JIT_K", V::I(u64::from(width))),
                ("JIT_CONT", V::Fall),
            ],
        );
        let brkey = self.arm_key("br/f", "JIT_T");
        self.emit(
            &brkey,
            &[
                ("JIT_A", V::I(u64::from(scr))),
                ("JIT_T", V::Blk(ok)),
                ("JIT_F", V::Fall),
            ],
        );
        if let Err(why) = self.c_call("buri_rt_abort_shift", st, &[], &[], 0, "v") {
            self.unsupported(why);
        }
        let here = self.region.code_addr();
        st.place(ok, here);
    }
}

/// The primitive a conversion key names, or `None` when the key is not one.
fn conversion_target(op: &str) -> Option<Prim> {
    if op == "toChar" {
        return Some(Prim::Char);
    }
    for prefix in ["wrapTo", "to"] {
        let Some(name) = op.strip_prefix(prefix) else { continue };
        if let Some(p) = prim_of_name(name) {
            return Some(p);
        }
    }
    None
}

/// The bit pattern of a type's lowest or highest value, at the width a frame
/// slot holds it in.
///
/// The pattern, not the number: `u64::MAX` is not an `i64`, and a slot holds an
/// integer zero-extended at its own width. `None` for the two 128-bit types,
/// which do not fit a slot at all.
fn bound_pattern(prim: Prim, low: bool) -> Option<u128> {
    if prim.is_float() {
        return bound_bits(prim, low).map(u128::from);
    }
    let (lo, hi) = prim.int_range()?;
    let (_, bits, _) = prim_tag(prim)?;
    let pattern = if low { lo as u128 } else { hi };
    if bits >= 128 {
        return Some(pattern);
    }
    let mask = if bits == 64 { u128::from(u64::MAX) } else { (1u128 << bits) - 1 };
    Some(pattern & mask)
}

fn bound_bits(prim: Prim, low: bool) -> Option<u64> {
    if prim.is_float() {
        // The largest finite magnitude, signed — not the smallest *positive*
        // one, which is `MIN_POSITIVE`; `Bounded` is about the range. An `F32`
        // is stored as its own thirty-two bits in the low half of the slot.
        return Some(match (prim, low) {
            (Prim::F32, true) => u64::from((-f32::MAX).to_bits()),
            (Prim::F32, false) => u64::from(f32::MAX.to_bits()),
            (_, true) => f64::MIN.to_bits(),
            (_, false) => f64::MAX.to_bits(),
        });
    }
    let (lo, hi) = prim.int_range()?;
    let (_, bits, _) = prim_tag(prim)?;
    let pattern = if low { lo as u128 } else { hi };
    if bits > 64 {
        return None;
    }
    let mask = if bits == 64 { u64::MAX } else { (1u64 << bits) - 1 };
    Some((pattern as u64) & mask)
}

/// `core/order`'s FNV-1a offset basis, which `order.buri:34` states and
/// `llvm/runtime.rs::HASH_SEED` restates.
const HASH_SEED: u64 = 0x811c_9dc5;

/// Which intrinsic keys this backend has a body for, asked ahead of emission.
///
/// `Backend::missing_intrinsics` is the *up-front* answer, so this is
/// deliberately the set of keys `Lower::intrinsic` and `Lower::runtime_body`
/// dispatch on, and not a claim that every program using one compiles: a shape
/// they decline — an argument list wider than a `crt` stencil, an element that
/// needs retain glue — is refused during emission with a sentence naming it.
/// The two answers are different questions and this is the cheaper one.
pub fn implemented(key: &str) -> bool {
    super::runtime::entry(key).is_some()
        || open_coded_key(key)
        || list_closure_key(key)
        || bits_op(key)
        || prim_trait_op(key)
        || key.strip_prefix("derivePrimShow.").is_some_and(|t| prim_of_name(t).is_some())
        || key.strip_prefix("derivePrimHash.").is_some_and(|t| prim_of_name(t).is_some())
        || key.strip_prefix("derivePrimJson.").is_some_and(|t| prim_of_name(t).is_some())
        || numeric_key(key)
}

/// The keys that are instructions rather than calls.
fn open_coded_key(key: &str) -> bool {
    matches!(
        key,
        "list.len"
            | "list.empty"
            | "str.concat"
            | "testing_context.alloc"
            | "testing_context.TestAlloc.allocate"
            | "host_testing.alloc"
            | "host_testing.TestAlloc.allocate"
            | "str.len"
            | "str.format"
            | "str.eq"
            | "str.compare"
            | "str.show"
            | "testing_assert.report"
            | "testing_assert.reportShown"
            | "testing_assert.failWith"
            | "testing_assert.failExpected"
            | "testing_assert.failExpectedShown"
    )
}

/// `core/list`'s closure surface, which `lists.rs` open-codes as a loop
/// because the step's signature is the element type flattened
/// (`cli/runtime/list.rs`'s header).
fn list_closure_key(key: &str) -> bool {
    matches!(
        key,
        "list.map"
            | "list.mapCtx"
            | "list.filter"
            | "list.filterCtx"
            | "list.fold"
            | "list.foldCtx"
            | "list.any"
            | "list.all"
            | "list.count"
            | "list.find"
            | "list.findIndex"
            | "list.sortBy"
            | "list.foldResult"
            | "list.foldResultCtx"
            // The two that build a block without taking a function at all:
            // what keeps them out of `cli/runtime/list.rs` is a second
            // *layout* rather than a closure, and that file's header says
            // which one each needs.
            | "list.zip"
            | "list.flatten"
    )
}

/// Whether a call to a `Body::Runtime` function is **emitted into its caller**
/// instead of being made.
///
/// The rule is one line — a key `runtime.rs`'s table has a row for — and the
/// two exclusions are not exceptions to it but the same fallback the loops have
/// always had:
///
/// * [`list_closure_key`] and the two `deriveArray*` derives are open-coded as
///   a **loop** whose step this function has to be able to see as a
///   `MakeClosure` (`lists.rs`). Where it cannot, the designed answer is a call
///   to the `Body::Runtime` function, whose body reaches the same loop through
///   the closure's thunk — so inlining those would replace a working fallback
///   with a refusal.
/// * A key with no row is `str.len`, `num.<T>.<op>` and the rest, whose bodies
///   `runtime_body` generates from the signature; those keys reach a backend
///   only as a method, never as an `Inst::CallIntrinsic`, so there is no
///   call-site emitter for them to be inlined by.
fn inline_runtime_key(key: &str) -> bool {
    super::runtime::entry(key).is_some()
        && !list_closure_key(key)
        && !matches!(key, "deriveArrayEq" | "deriveArrayShow")
}

/// `num.<T>.<op>`, for the operations `Lower::runtime_body` turns into an
/// arithmetic stencil or an immediate.
///
/// `missing_intrinsics` is asked of the *monomorphized* program, before
/// `middle::lower` runs — so `Bounded` is still two segments there and three by
/// the time the body is emitted. Both spellings answer yes, because both
/// describe an operation this backend compiles.
///
/// The list is what `runtime_body` actually dispatches on and not `num.*`:
/// claiming a key with no body would turn a diagnostic that names the operation
/// into one that names an IR shape. `toJson` is the operation `core/num`
/// declares that this does not answer.
fn numeric_key(key: &str) -> bool {
    if key == "num.minValue" || key == "num.maxValue" {
        return true;
    }
    let mut parts = key.split('.');
    if parts.next() != Some("num") {
        return false;
    }
    let (Some(t), Some(op)) = (parts.next(), parts.next()) else { return false };
    if parts.next().is_some() || prim_of_name(t).is_none() {
        return false;
    }
    matches!(
        op,
        "add"
            | "sub"
            | "mul"
            | "div"
            | "rem"
            | "neg"
            | "min"
            | "max"
            | "eq"
            | "compare"
            | "show"
            | "minValue"
            | "maxValue"
            | "toF64"
            | "toChar"
            | "hash"
            | "abs"
            | "signum"
    ) || conversion_target(op).is_some()
        || ["checked", "saturating", "wrapping"]
            .iter()
            .any(|p| op.strip_prefix(p).is_some_and(|k| matches!(k, "Add" | "Sub" | "Mul" | "Div")))
}
