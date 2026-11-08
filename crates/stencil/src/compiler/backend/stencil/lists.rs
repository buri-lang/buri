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
use super::jit::{Jit, Key, V};
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
    /// `frame[dst] = *(frame[ptr] + off)`, `bytes` wide, zero-extended into a
    /// whole word below eight bytes. `dst` may share bytes with `ptr`: the
    /// pointer is read before anything is written.
    pub(crate) fn ptr_load(&mut self, dst: u32, ptr: u32, off: u32, bytes: u32) {
        let fixed_key = key!["pload/", bytes];
        let fixed = self.has(&fixed_key);
        let key = if fixed { fixed_key } else { key!["pload/n"] };
        self.emit(
            &key,
            &[
                Lit::D.of(u64::from(dst)),
                Lit::A.of(u64::from(ptr)),
                Lit::N.of(u64::from(off)),
                Lit::P.of(u64::from(bytes)),
                Jump::Cont.to(V::Fall),
            ],
        );
        // 3, 5, 6 and 7 bytes land in the word's low half, and the rest is
        // masked off after, as `elem_load` does.
        if !fixed && bytes < 8 {
            self.emit(
                "bin/and/u64/fi/f",
                &[
                    Lit::D.of(u64::from(dst)),
                    Lit::A.of(u64::from(dst)),
                    Word::K.lit((1u64 << (8 * bytes)) - 1),
                    Jump::Cont.to(V::Fall),
                ],
            );
        }
    }

    /// `*(frame[ptr] + off) = frame[src]`, `bytes` wide.
    pub(crate) fn ptr_store(&mut self, src: u32, ptr: u32, off: u32, bytes: u32) {
        let fixed_key = key!["pput/", bytes];
        let fixed = self.has(&fixed_key);
        let key = if fixed { fixed_key } else { key!["pput/n"] };
        self.emit(
            &key,
            &[
                Lit::D.of(u64::from(src)),
                Lit::A.of(u64::from(ptr)),
                Lit::N.of(u64::from(off)),
                Lit::P.of(u64::from(bytes)),
                Jump::Cont.to(V::Fall),
            ],
        );
    }

    /// `frame[dst] = *(base + i * bytes)`, `bytes` wide: an element, whose
    /// stride is its size.
    ///
    /// `dst` may share bytes with `base` or `i`: each stencil reads both
    /// before it writes, and nothing here writes `dst` before that.
    pub(crate) fn elem_load(&mut self, dst: u32, base: u32, i: u32, bytes: u32) {
        // An element narrower than a frame word has to arrive **zero-extended**
        // into a whole one; see `sources.rs`'s `eloadz` family for why.
        let zk = key!["eloadz/", bytes];
        if self.has(&zk) {
            self.elem_op(zk, dst, base, i, bytes);
            return;
        }
        self.elem_op(self.elem_key("eload/", bytes), dst, base, i, bytes);
        // 3, 5, 6, 7 bytes: no zero-extending twin, so the bytes land in the
        // word's low half and the rest is masked off after. Clearing the word
        // first would clear a `base` or `i` that shares it.
        if bytes < 8 {
            self.emit(
                "bin/and/u64/fi/f",
                &[
                    Lit::D.of(u64::from(dst)),
                    Lit::A.of(u64::from(dst)),
                    Word::K.lit((1u64 << (8 * bytes)) - 1),
                    Jump::Cont.to(V::Fall),
                ],
            );
        }
    }

    /// `*(base + i * bytes) = frame[src]`, `bytes` wide.
    pub(crate) fn elem_store(&mut self, src: u32, base: u32, i: u32, bytes: u32) {
        self.elem_op(self.elem_key("estore/", bytes), src, base, i, bytes);
    }

    /// `{family}{bytes}`, or `{family}n` for a width with no fixed stencil.
    fn elem_key(&self, family: &str, bytes: u32) -> Key {
        let fixed = key![family, bytes];
        if self.has(&fixed) {
            fixed
        } else {
            key![family, "n"]
        }
    }

    fn elem_op(&mut self, key: Key, at: u32, base: u32, i: u32, bytes: u32) {
        self.emit(
            &key,
            &[
                Lit::D.of(u64::from(at)),
                Lit::A.of(u64::from(base)),
                Lit::B.of(u64::from(i)),
                Lit::N.of(u64::from(bytes)),
                Jump::Cont.to(V::Fall),
            ],
        );
    }
}
