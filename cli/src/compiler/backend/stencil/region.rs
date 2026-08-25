//! The buffer one codegen unit's code and constants are copied into, and the
//! relocations that leave it.
//!
//! The prototype this backend grew out of wrote into an `mmap`ed,
//! `PROT_EXEC`able region and patched absolute addresses into it, because it
//! executed what it emitted in its own process. A backend does not: it hands
//! object files to the system linker like the other two do
//! (`design/native/ARCHITECTURE.md` §3), so the same emitter writes into a plain
//! `Vec<u8>` whose addresses are **section offsets** and records what it could
//! not resolve as a relocation.
//!
//! Nothing about the copy-and-patch emitter had to change for that, and the
//! reason is worth stating because it is the property that made this backend
//! possible at all: **every stencil is position-independent**. A stencil's bytes
//! are whatever clang emitted for a leaf C function; the only addresses in them
//! are the holes, and a hole is either a literal, a pc-relative branch, or a
//! pc-relative load of the constant pool. Emitting at a virtual base of zero and
//! letting the linker choose the real one is therefore the *same* patching, and
//! the two addresses a hole cannot know — a symbol in another object and the
//! runtime's — are exactly the two the relocation format exists for.
//!
//! # Code and pool in two sections
//!
//! [`HoleKind::Imm64`](super::library::HoleKind) is an `adrp`/`ldr` pair aimed
//! at a constant-pool slot, and a pool slot may hold an **address** — a
//! function's, or a string literal's bytes. An eight-byte absolute address is
//! what a relocation is for, and `ld` refuses one inside a code section outright
//! ("Absolute addressing not allowed in arm64 code"), so the pool is its own
//! `__DATA_CONST,__const` and not a tail on `__text`.
//!
//! That in turn means the `adrp`/`ldr` pair crosses a section boundary, whose
//! distance is the linker's choice — so the pair is not patched at all. It is
//! left exactly as clang emitted it, with both immediate fields zero, and given
//! an `ARM64_RELOC_PAGE21`/`ARM64_RELOC_PAGEOFF12` pair naming the pool slot.
//! Which is what clang's own GOT form was before the prototype retargeted it:
//! the port comes back to the relocation it started from once there is a linker
//! to honour it.

#![allow(
    clippy::arithmetic_side_effects,
    reason = "the two operations here are a padding count and a pool slot's \
              end. The first is `len % to` against an alignment this emitter \
              passes as a literal 4, never zero; the second is one slot's \
              eight bytes past an offset the pool itself handed out, so both \
              operands are bounded by a buffer that is already in memory"
)]

use std::collections::HashMap;

/// log2 of the code section's alignment. Instructions are four bytes and
/// nothing here asks for more.
pub const CODE_ALIGN: u32 = 2;

/// log2 of the constant pool's. Eight, because the `ldr` a pool reference
/// becomes scales its twelve-bit offset by eight and every slot is a word.
pub const POOL_ALIGN: u32 = 3;

/// Where a relocation's value comes from.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Target {
    /// A function of this program, by `FuncIdx`. Resolved inside the object
    /// when the callee is in this unit and left to the linker when it is not.
    Func(u32),
    /// A symbol outside this program: the runtime archive's, or the C
    /// library's.
    Symbol(String),
    /// A byte offset within the constant pool — a string literal's bytes. The
    /// linker still has to see it, because the pool's base is its choice.
    Here(u64),
    /// The constant pool itself, at offset zero.
    Pool,
}

/// What the linker is asked to do at one place in the section.
#[derive(Clone, Debug)]
pub struct Reloc {
    /// Byte offset within the section.
    pub at: u64,
    pub kind: RelocKind,
    pub target: Target,
    /// Added to the target's address before the field is formed.
    pub addend: i64,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RelocKind {
    /// The 26-bit word displacement of a `b`/`bl`.
    Branch26,
    /// An eight-byte absolute address in the constant pool.
    Abs64,
    /// The page half of an `adrp`.
    Page21,
    /// The offset half of the `ldr` behind one.
    PageOff12,
}

/// One unit's emitted bytes.
///
/// `base` is zero and stays zero: the emitter's "addresses" are offsets into
/// this section, and the linker supplies the rest.
pub struct Region {
    /// The instruction stream.
    pub bytes: Vec<u8>,
    /// The constant pool, laid out after the code by [`Region::finish`].
    pool: Vec<u8>,

    /// Pool slots holding an address, by pool offset.
    pool_targets: Vec<(u64, Target)>,
    /// Deduplicates the pool: one slot per distinct 64-bit datum. A literal is
    /// pooled once per function in the prototype and once per *unit* here, and
    /// the constants a lowered program repeats are the small integers every
    /// loop bound is.
    pool_index: HashMap<u64, u64>,
    /// Relocations against the code stream.
    pub relocs: Vec<Reloc>,
    /// Relocations against the constant pool.
    pool_relocs: Vec<Reloc>,
}

/// A pool handle is not an address, and the emitter passes both around as
/// `u64`. The tag is what tells them apart: it is far above any offset a unit
/// can reach, so a handle used where an address belongs is a wild pointer that
/// faults rather than a plausible one that does not.
const POOL_TAG: u64 = 1 << 40;

/// Whether a `u64` the emitter is carrying is a pool handle.
pub fn is_pool_handle(v: u64) -> bool {
    v & POOL_TAG != 0
}

impl Default for Region {
    fn default() -> Region {
        Region::new()
    }
}

impl Region {
    pub fn new() -> Region {
        Region {
            bytes: Vec::new(),
            pool: Vec::new(),
            pool_targets: Vec::new(),
            pool_index: HashMap::new(),
            relocs: Vec::new(),
            pool_relocs: Vec::new(),
        }
    }

    /// The offset the next byte of code goes to, which is the address the
    /// emitter uses for everything inside the section.
    pub fn code_addr(&self) -> u64 {
        self.bytes.len() as u64
    }

    pub fn put(&mut self, bytes: &[u8]) -> u64 {
        let at = self.code_addr();
        self.bytes.extend_from_slice(bytes);
        at
    }

    pub fn align_code(&mut self, to: usize) {
        while !self.bytes.len().is_multiple_of(to) {
            self.bytes.push(0);
        }
    }

    /// A pool slot holding `v`, by pool offset.
    ///
    /// Eight-aligned, because the `ldr` a constant-pool hole becomes scales its
    /// 12-bit offset by eight: a misaligned slot is not slow, it is a different
    /// address.
    pub fn pool_u64(&mut self, v: u64) -> u64 {
        if let Some(at) = self.pool_index.get(&v) {
            return POOL_TAG | *at;
        }
        let at = self.align_pool();
        self.pool.extend_from_slice(&v.to_le_bytes());
        self.pool_index.insert(v, at);
        POOL_TAG | at
    }

    /// A pool slot whose contents the linker fills in.
    pub fn pool_target(&mut self, t: Target) -> u64 {
        // A `Here` arrives carrying the tag every pool handle has; what goes
        // into the relocation is the offset.
        let t = match t {
            Target::Here(off) => Target::Here(off & !POOL_TAG),
            other => other,
        };
        let at = self.align_pool();
        self.pool.extend_from_slice(&0u64.to_le_bytes());
        self.pool_targets.push((at, t));
        POOL_TAG | at
    }

    /// Bytes in the pool — a string literal's payload, an abort message.
    pub fn pool_bytes(&mut self, b: &[u8]) -> u64 {
        let at = self.align_pool();
        self.pool.extend_from_slice(b);
        POOL_TAG | at
    }

    fn align_pool(&mut self) -> u64 {
        while !self.pool.len().is_multiple_of(8) {
            self.pool.push(0);
        }
        self.pool.len() as u64
    }

    /// An `adrp`/`ldr` pair aimed at a pool slot, as the relocation pair the
    /// linker resolves. Neither instruction is rewritten: clang emitted them
    /// with zero immediates and the linker fills both fields.
    pub fn pool_ref(&mut self, adrp: u64, ldr: u64, slot: u64) {
        let addend = (slot & !POOL_TAG) as i64;
        self.relocs.push(Reloc {
            at: adrp,
            kind: RelocKind::Page21,
            target: Target::Pool,
            addend,
        });
        self.relocs.push(Reloc {
            at: ldr,
            kind: RelocKind::PageOff12,
            target: Target::Pool,
            addend,
        });
    }

    pub fn reloc(&mut self, at: u64, kind: RelocKind, target: Target) {
        self.relocs.push(Reloc { at, kind, target, addend: 0 });
    }

    pub fn word_at(&self, addr: u64) -> u32 {
        let o = addr as usize;
        let b = self.bytes.get(o..o.saturating_add(4)).unwrap_or(&[0, 0, 0, 0]);
        u32::from_le_bytes([
            *b.first().unwrap_or(&0),
            *b.get(1).unwrap_or(&0),
            *b.get(2).unwrap_or(&0),
            *b.get(3).unwrap_or(&0),
        ])
    }

    pub fn set_word(&mut self, addr: u64, w: u32) {
        let o = addr as usize;
        if let Some(slot) = self.bytes.get_mut(o..o.saturating_add(4)) {
            slot.copy_from_slice(&w.to_le_bytes());
        }
    }

    /// The two sections and the relocations each still needs.
    pub fn finish(mut self) -> Emitted {
        for (at, target) in std::mem::take(&mut self.pool_targets) {
            // A pool word naming a byte of the pool carries the byte's offset
            // as the relocation's addend — the field, not the slot's bytes:
            // ELF `RELA` resolves to `S + r_addend` and ignores the section's
            // contents, while `object.rs` folds the field into the bytes where
            // Mach-O's `UNSIGNED` reads it.
            let (target, addend) = match target {
                Target::Here(off) => (Target::Pool, off as i64),
                other => (other, 0),
            };
            self.pool_relocs.push(Reloc { at, kind: RelocKind::Abs64, target, addend });
        }
        Emitted {
            code: self.bytes,
            code_relocs: self.relocs,
            pool: self.pool,
            pool_relocs: self.pool_relocs,
        }
    }
}

/// What one unit's emission comes to.
pub struct Emitted {
    pub code: Vec<u8>,
    pub code_relocs: Vec<Reloc>,
    pub pool: Vec<u8>,
    pub pool_relocs: Vec<Reloc>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_datum_is_pooled_once() {
        let mut r = Region::new();
        let a = r.pool_u64(7);
        let b = r.pool_u64(7);
        let c = r.pool_u64(8);
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    /// The pool is its own section, and the bytes of a literal are at the
    /// offset the handle named.
    #[test]
    fn the_pool_is_its_own_section() {
        let mut r = Region::new();
        r.put(&[0xaa; 6]);
        let s = r.pool_bytes(b"hi");
        let out = r.finish();
        assert!(out.pool_relocs.is_empty());
        assert_eq!(out.code.len(), 6);
        assert_eq!(out.pool.get(0..2), Some(&b"hi"[..]));
        assert_eq!(s & !(1 << 40), 0);
    }

    /// A pool word that names a symbol becomes a relocation and stays zero in
    /// the bytes; one that names this section carries the offset as the
    /// relocation's addend, and its bytes also stay zero.
    #[test]
    fn pool_targets_become_relocations() {
        let mut r = Region::new();
        r.put(&[0; 8]);
        r.pool_bytes(b"leading bytes");
        let here = r.pool_bytes(b"abcd");
        r.pool_target(Target::Symbol(String::from("buri_rt_flush")));
        r.pool_target(Target::Here(here & !(1 << 40)));
        let out = r.finish();
        assert_eq!(out.pool_relocs.len(), 2);
        assert_eq!(out.pool_relocs.first().map(|x| x.kind), Some(RelocKind::Abs64));
        assert_eq!(out.pool_relocs.first().map(|x| x.addend), Some(0));
        assert_ne!(here & !(1 << 40), 0);
        assert_eq!(out.pool_relocs.get(1).map(|x| x.addend), Some((here & !(1 << 40)) as i64));
        let slot = out.pool_relocs.get(1).map(|x| x.at).unwrap_or(0) as usize;
        let w = out.pool.get(slot..slot + 8).map(|b| {
            let mut a = [0u8; 8];
            a.copy_from_slice(b);
            u64::from_le_bytes(a)
        });
        assert_eq!(w, Some(0));
    }
}
