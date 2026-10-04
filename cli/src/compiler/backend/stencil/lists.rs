//! A `[T]`'s elements, read and written in place, and the two facts about an
//! enum that a native `Option` or `Result` answer needs.
//!
//! `middle::lower` builds every `core/list` loop as IR (`lower/lists.rs`), so
//! what is left here is what [`ir::Inst::ArrayGet`], [`ir::Inst::ArraySet`] and
//! a runtime call answering an enum are made of: the indexed copy between a
//! block and a frame slot, and where a variant's payload and tag are.

#![allow(
    clippy::arithmetic_side_effects,
    reason = "every sum here is a byte offset inside a frame `Jit::plan` has \
              already sized, or an offset inside one value's layout"
)]

use super::jit::{Jit, V};
use crate::compiler::middle::ir;
use crate::compiler::middle::layout::{EnumRepr, Layout, Repr};
use crate::compiler::semantics::types::Ty;

impl Jit<'_> {
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
}
