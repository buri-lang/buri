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

use super::abi::{Jump, Lit, Word};
use super::jit::{Jit, V};
use crate::compiler::middle::ir;

impl Jit<'_> {
    /// A `[T]`'s stride, element width and whether the element is counted,
    /// for a list the instruction names.
    pub(crate) fn list_elem(&mut self, prog: &ir::Program, l: ir::ListTy) -> (u32, u32, bool) {
        let elem = prog.types.elem(l);
        let l = self.layouts_of(elem);
        let counted = self.rc_counted(&elem);
        (l.stride.max(1), l.size.max(1), counted)
    }
}

impl Jit<'_> {
    /// `frame[dst] = *(base + i * stride)`, `bytes` wide.
    ///
    /// `dst` may share bytes with `base` or `i`: each stencil reads both
    /// before it writes, and nothing here writes `dst` before that.
    pub(crate) fn elem_load(&mut self, dst: u32, base: u32, i: u32, stride: u32, bytes: u32) {
        // An element narrower than a frame word has to arrive **zero-extended**
        // into a whole one; see `sources.rs`'s `eloadz` family for why.
        if bytes < 8 {
            let zk = if stride == bytes {
                key!["eloadz/", bytes, "/s"]
            } else {
                key!["eloadz/", bytes]
            };
            if self.has(&zk) {
                self.emit(
                    &zk,
                    &[
                        Lit::D.of(u64::from(dst)),
                        Lit::A.of(u64::from(base)),
                        Lit::B.of(u64::from(i)),
                        Lit::P.of(u64::from(stride)),
                        Jump::Cont.to(V::Fall),
                    ],
                );
                return;
            }
            // 3, 5, 6, 7 bytes: no zero-extending twin, so the bytes land in
            // the word's low half and the rest is masked off after. Clearing
            // the word first would clear a `base` or `i` that shares it.
            self.elem_copy(dst, base, i, stride, bytes);
            self.emit(
                "bin/and/u64/fi/f",
                &[
                    Lit::D.of(u64::from(dst)),
                    Lit::A.of(u64::from(dst)),
                    Word::K.lit((1u64 << (8 * bytes)) - 1),
                    Jump::Cont.to(V::Fall),
                ],
            );
            return;
        }
        self.elem_copy(dst, base, i, stride, bytes);
    }

    /// `bytes` bytes of `*(base + i * stride)` into `frame[dst]`, and nothing
    /// past them.
    fn elem_copy(&mut self, dst: u32, base: u32, i: u32, stride: u32, bytes: u32) {
        let sk = key!["eload/", bytes, "/s"];
        let key = if stride == bytes && self.has(&sk) {
            sk
        } else {
            key!["eload/", bytes]
        };
        if self.has(&key) {
            self.emit(
                &key,
                &[
                    Lit::D.of(u64::from(dst)),
                    Lit::A.of(u64::from(base)),
                    Lit::B.of(u64::from(i)),
                    Lit::P.of(u64::from(stride)),
                    Jump::Cont.to(V::Fall),
                ],
            );
            return;
        }
        self.emit(
            "eload/n",
            &[
                Lit::D.of(u64::from(dst)),
                Lit::A.of(u64::from(base)),
                Lit::B.of(u64::from(i)),
                Lit::P.of(u64::from(stride)),
                Lit::N.of(u64::from(bytes)),
                Jump::Cont.to(V::Fall),
            ],
        );
    }

    /// `*(base + i * stride) = frame[src]`, `bytes` wide.
    pub(crate) fn elem_store(&mut self, src: u32, base: u32, i: u32, stride: u32, bytes: u32) {
        let sk = key!["estore/", bytes, "/s"];
        let key = if stride == bytes && self.has(&sk) {
            sk
        } else {
            key!["estore/", bytes]
        };
        if self.has(&key) {
            self.emit(
                &key,
                &[
                    Lit::D.of(u64::from(src)),
                    Lit::A.of(u64::from(base)),
                    Lit::B.of(u64::from(i)),
                    Lit::P.of(u64::from(stride)),
                    Jump::Cont.to(V::Fall),
                ],
            );
            return;
        }
        self.emit(
            "estore/n",
            &[
                Lit::D.of(u64::from(src)),
                Lit::A.of(u64::from(base)),
                Lit::B.of(u64::from(i)),
                Lit::P.of(u64::from(stride)),
                Lit::N.of(u64::from(bytes)),
                Jump::Cont.to(V::Fall),
            ],
        );
    }
}
