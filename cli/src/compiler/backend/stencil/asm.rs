//! The two functions in a copy-and-patch program that no stencil can supply:
//! `main`, hand-assembled.
//!
//! Everything else this backend emits is copied from a stencil, which is a leaf
//! C function clang compiled, and therefore obeys AAPCS64 in the ordinary way.
//! An emitted *Buri* function does not. Its whole calling convention is one
//! register and one contiguous block:
//!
//!  * `x0` is a **frame pointer** into the Buri stack, and the only argument.
//!  * The frame is `[fp .. fp + ret_size)`, the return area the callee writes
//!    and the caller reads, then the parameters, then the locals.
//!  * A call writes its arguments at `fp + frame_size(caller) + param_off(callee)`
//!    and enters the callee with `x0 = fp + frame_size(caller)`.
//!  * The callee answers its caller's `fp` — the value it was handed — in `x0`.
//!  * Nothing is pushed. Generated code does not touch the machine stack at
//!    all, which is what makes a continuation call a `b` rather than a frame
//!    teardown and a `ret`.
//!
//! A C runtime cannot call that, and `crt1.o` calls `main` and not something
//! else, so exactly one function per program has to bridge the two conventions.
//! It is written here as instruction words rather than built out of stencils
//! because a stencil is a *C* function: the bridge is precisely the code that
//! cannot be expressed in the language the stencils are written in.
//!
//! # The shim is a copy of the debug backend's, not a second design
//!
//! `cli/runtime/lib.rs` §6 is the contract — `buri_rt_argv_init` first,
//! `buri_rt_flush` before every return — and the backend this one replaced
//! emitted the same two shims. Every behavioural decision below was read off
//! them rather than made again, including which way round the niche test goes
//! ([`MainResult::niche`]), because a program that fails must print the same
//! thing and exit the same way whichever backend built it.
//!
//! # Two machines, one shim
//!
//! Everything above is stated in A64's vocabulary because that is the machine
//! this file was written for first. The **SysV x86-64** half below is the same
//! two shims for the other instruction set, and the convention it bridges to is
//! the same one: `rdi` is the frame pointer where `x0` was, and an emitted
//! function answers the frame pointer it was handed — in `rax`, because that is
//! where a C function's return value lives on this machine and every stencil is
//! `uint64_t *st_…(uint64_t *fp, …) { … return fp; }`.
//!
//! The two differences that show up in every line of it:
//!
//!  * **the machine stack has to stay sixteen-aligned.** A64 needs no such
//!    thing from a leaf that touches `sp` in pairs; SysV requires `rsp % 16 ==
//!    0` at the point of a `call`, and `main` is entered with the return
//!    address already pushed. One `push rbp` fixes that for the whole shim, and
//!    nothing below it moves `rsp` again.
//!  * **an address is one instruction.** `adrp`/`add` against a symbol becomes
//!    `lea rD, [rip+sym]` and one `R_X86_64_PC32`, and `bl` becomes `call
//!    rel32` and one `R_X86_64_PLT32`. Both fields are the last four bytes of
//!    their instruction, so both relocations carry an addend of `-4`.
//!
//! # Relocation vocabulary
//!
//! [`region::Target`] says *what* an address is; the kind comes from
//! [`object::RelKind`] rather than [`region::RelocKind`]. The emitter proper
//! never needs an `adrp`/`add` pair against a symbol — its only page-relative
//! addressing is at the constant pool, which shares a section with the code and
//! so needs no relocation at all (`region.rs`'s header) — but this file does,
//! to name the Buri stack. `object::RelKind` already spells every kind the
//! object writer accepts, so the shim speaks that and nothing has to translate.

#![allow(
    clippy::arithmetic_side_effects,
    reason = "this file's arithmetic is instruction encoding and byte offsets \
              into a buffer it is itself appending to. Every operand is a \
              register number masked to three or five bits, an immediate \
              masked to its field width, a shift by a constant, or a position \
              already reached in a `Vec` held in memory; every result is one \
              such field or the next such position. The two signed operations \
              are branch displacements, each a difference of two offsets into \
              that same buffer — divided by the four bytes an A64 instruction \
              occupies, or measured from the end of an x86-64 one"
)]

use super::abi::StencilTarget;
use super::object::RelKind;
use super::region::Target;

/// Whether a `linux-x86_64` build can be made at all.
///
/// **It can.** This file writes a SysV `main` ([`program_entry`],
/// [`test_entry`]), `jit.rs` patches `rel32` and rip-relative `disp32` fields
/// where it patches A64 ones, `glue.rs` writes the SysV stub in front of a
/// generated body, and `region.rs`/`elf.rs` carry the two relocation kinds that
/// needs. `design/native/CODEGEN-STENCIL.md` §10.3 is the list this was written
/// against, and its last part is what CI confirms.
///
/// It stays a constant rather than becoming nothing, because it is the one
/// place a target's *entry point* is either present or named as missing, and a
/// fourth target would be added here the same way.
pub const AVAILABLE_X86_64: bool = true;

/// The symbol the program's Buri stack is emitted under.
///
/// A `$` because no Buri identifier can contain one, so the name cannot collide
/// with a mangled user symbol.
pub const STACK_SYMBOL: &str = "buri$stencil$stack";

/// How much Buri stack a program may actually use: 64 MiB.
///
/// The block is zero-filled and the object emits it into a `__bss`-style
/// zerofill section, so it costs no bytes in the executable and no work at
/// startup — only address space, which is why the number can be generous
/// without being paid for.
///
/// 64 MiB rather than the 8 MiB macOS and Linux give a main thread, because a
/// Buri frame is the function's *entire* locals area laid out by
/// `middle::layout` rather than the register allocator's spill set: a frame
/// here is larger than the machine frame the same function compiles to under a
/// register-allocating backend, so equal depth costs more bytes.
pub const STACK_USABLE: u64 = 64 * 1024 * 1024;

/// The guard region above the usable stack, which [`install_guard`] turns into
/// `PROT_NONE` at startup: 1 MiB.
///
/// **Why it is above and not below.** A frame here is entered at
/// `fp + frame_size(caller)`, so this stack grows *upward* from
/// [`STACK_SYMBOL`] and the address a runaway recursion reaches first is the
/// top of the block. That is the opposite direction from the machine stack, and
/// therefore the opposite side from where the kernel puts a thread's own guard.
///
/// **Why a megabyte and not a page.** A guard smaller than the largest frame
/// can be *stepped over*: a callee whose frame is wider than the guard writes
/// past it without ever touching it, which is the same hazard native code
/// answers with stack probes. A Buri frame is a `middle::layout` locals area
/// and 1 MiB is far past any this compiler emits, at a cost of address space
/// alone — the pages are zero-fill and never faulted in. It is not a *proof*,
/// and neither is the machine stack's: no backend here emits stack probes, so a
/// machine frame past the OS guard has exactly this exposure. The two are level
/// here rather than one being sound.
pub const GUARD_BYTES: u64 = 1024 * 1024;

/// The whole zero-filled block: the usable stack, then the guard.
///
/// One block and one symbol rather than two, because
/// `MH_SUBSECTIONS_VIA_SYMBOLS` makes every symbol the start of an
/// independently movable atom — a second symbol at the guard's own address
/// would let `ld64` place the guard somewhere other than immediately above the
/// stack, which is the one property the whole mechanism rests on.
pub const STACK_BYTES: u64 = STACK_USABLE + GUARD_BYTES;

/// log2 of the alignment the stack block must be emitted with: 16 KiB.
///
/// Two requirements, and the second is the larger. Every load and store into a
/// frame is at a `middle::layout` offset computed from a base of zero, so the
/// block's own address has to satisfy the widest alignment a layout can ask for
/// — sixteen bytes, which covers `i128` and `f64`. And [`install_guard`]
/// `mprotect`s the top [`GUARD_BYTES`] of the block, which the kernel will only
/// do on a page boundary: arm64 macOS pages are 16 KiB, and both
/// [`STACK_USABLE`] and [`GUARD_BYTES`] are whole multiples of one, so a block
/// aligned to 16 KiB puts the guard's base on a page. The widest page any
/// supported target has, so x86-64 Linux's 4 KiB is covered by the same number.
pub const STACK_ALIGN: u32 = 14;

/// A place in the instruction stream whose branch target is not known yet.
///
/// Held by value and consumed by [`Asm::here`], so a forward branch that is
/// never resolved is an unused-variable warning rather than a branch to itself.
#[must_use]
pub struct Patch {
    at: u64,
    kind: PatchKind,
}

enum PatchKind {
    /// The 19-bit displacement of a `cbz`/`cbnz`.
    Cond19,
    /// The 26-bit displacement of a `b`.
    Uncond26,
}

/// One relocation a hand-written block leaves behind: where, what kind, what
/// it names, and what to add to the target before the field is formed.
pub type ShimReloc = (u64, RelKind, Target, i64);

/// A finished shim: the bytes, and one relocation per site.
///
/// The addend travels with the kind because the two x86-64 kinds need one — a
/// `rel32` and a rip-relative `disp32` are both measured from the end of their
/// instruction, and the psABI computes from the start of the field — while
/// every A64 kind here takes zero.
pub struct Shim {
    pub bytes: Vec<u8>,
    pub relocs: Vec<ShimReloc>,
}

/// A block of hand-written code, with the relocations it needs.
pub struct Asm {
    bytes: Vec<u8>,
    relocs: Vec<(u64, RelKind, Target)>,
}

// Opcode templates, with every variable field zero. `Rd` is bits 0..5 and `Rn`
// is bits 5..10 throughout, so the emitters below share one shape.
const MOVZ_X: u32 = 0xd280_0000;
const MOVK_X: u32 = 0xf280_0000;
const ORR_X_SHIFTED: u32 = 0xaa00_0000;
const AND_X_SHIFTED: u32 = 0x8a00_0000;
const ADD_X_IMM: u32 = 0x9100_0000;
const ADD_X_SHIFTED: u32 = 0x8b00_0000;
const SUB_X_IMM: u32 = 0xd100_0000;
/// `str xt, [xn, #off]`, unsigned offset scaled by eight.
const STR_X_UIMM: u32 = 0xf900_0000;
/// `str xt, [xn, #imm9]!` — pre-index, unscaled.
const STR_X_PRE: u32 = 0xf800_0c00;
/// `ldr xt, [xn], #imm9` — post-index, unscaled.
const LDR_X_POST: u32 = 0xf840_0400;
const LDR_X_UIMM: u32 = 0xf940_0000;
const LDR_W_UIMM: u32 = 0xb940_0000;
const LDRH_W_UIMM: u32 = 0x7940_0000;
const LDRB_W_UIMM: u32 = 0x3940_0000;
const CBZ_W: u32 = 0x3400_0000;
const CBZ_X: u32 = 0xb400_0000;
const CBNZ_W: u32 = 0x3500_0000;
const CBNZ_X: u32 = 0xb500_0000;
const B: u32 = 0x1400_0000;
const BL: u32 = 0x9400_0000;
const ADRP: u32 = 0x9000_0000;
const RET_X30: u32 = 0xd65f_03c0;
/// `stp x29, x30, [sp, #-16]!` — the pre-index immediate is `-16/8 = -2`.
const STP_FP_LR_PRE: u32 = 0xa9bf_7bfd;
/// `ldp x29, x30, [sp], #16`.
const LDP_FP_LR_POST: u32 = 0xa8c1_7bfd;
/// The stack pointer and the zero register share encoding 31; which one an
/// instruction means is fixed by the instruction.
pub const SP: u32 = 31;
const ZR: u32 = 31;

impl Default for Asm {
    fn default() -> Asm {
        Asm::new()
    }
}

impl Asm {
    pub fn new() -> Asm {
        Asm { bytes: Vec::new(), relocs: Vec::new() }
    }

    /// The offset the next instruction will occupy.
    fn at(&self) -> u64 {
        self.bytes.len() as u64
    }

    fn word(&mut self, w: u32) {
        self.bytes.extend_from_slice(&w.to_le_bytes());
    }

    /// `movz`/`movk`, low halfword first, skipping the halfwords that are zero.
    ///
    /// Up to four instructions, and exactly one for every value below `2^16` —
    /// which is every value either shim actually forms except the `Str` length
    /// mask.
    pub fn mov_imm(&mut self, rd: u32, value: u64) {
        let mut started = false;
        for hw in 0..4u32 {
            let part = ((value >> (hw * 16)) & 0xffff) as u32;
            if part == 0 {
                continue;
            }
            let op = if started { MOVK_X } else { MOVZ_X };
            self.word(op | (hw << 21) | (part << 5) | (rd & 31));
            started = true;
        }
        // Zero has no non-zero halfword to start the sequence, so it is the one
        // value that needs a `movz #0` written unconditionally.
        if !started {
            self.word(MOVZ_X | (rd & 31));
        }
    }

    /// `mov xd, xn`, which arm64 spells `orr xd, xzr, xn`.
    pub fn mov_reg(&mut self, rd: u32, rn: u32) {
        self.word(ORR_X_SHIFTED | ((rn & 31) << 16) | (ZR << 5) | (rd & 31));
    }

    /// `and xd, xn, xm`. The register form rather than arm64's bitmask
    /// immediate, because the one mask this file applies comes from
    /// `middle::layout` and a mask that is not an encodable bitmask pattern
    /// must not become a silently different instruction.
    pub fn and_reg(&mut self, rd: u32, rn: u32, rm: u32) {
        self.word(AND_X_SHIFTED | ((rm & 31) << 16) | ((rn & 31) << 5) | (rd & 31));
    }

    /// `add xd, xn, #imm`, unshifted, so `imm` must fit twelve bits.
    pub fn add_imm(&mut self, rd: u32, rn: u32, imm: u32) {
        self.word(ADD_X_IMM | ((imm & 0xfff) << 10) | ((rn & 31) << 5) | (rd & 31));
    }

    /// `add xd, xn, xm`. The register form, for the one addend this file has
    /// that does not fit an `imm12`: the distance from the stack's base to its
    /// guard ([`install_guard`]).
    pub fn add_reg(&mut self, rd: u32, rn: u32, rm: u32) {
        self.word(ADD_X_SHIFTED | ((rm & 31) << 16) | ((rn & 31) << 5) | (rd & 31));
    }

    /// `sub xd, xn, #imm`, unshifted, so `imm` must fit twelve bits.
    pub fn sub_imm(&mut self, rd: u32, rn: u32, imm: u32) {
        self.word(SUB_X_IMM | ((imm & 0xfff) << 10) | ((rn & 31) << 5) | (rd & 31));
    }

    /// `str xt, [xn, #off]`, scaled by eight.
    pub fn str_off(&mut self, rt: u32, rn: u32, off: u32) {
        self.word(STR_X_UIMM | (((off / 8) & 0xfff) << 10) | ((rn & 31) << 5) | (rt & 31));
    }

    /// `str xt, [xn, #-16]!` and `ldr xt, [xn], #16` — the one-register frame
    /// a generated glue function needs to keep its return address across the
    /// stencil chain it calls.
    pub fn str_pre16(&mut self, rt: u32, rn: u32) {
        self.word(STR_X_PRE | ((0x1f0u32 & 0x1ff) << 12) | ((rn & 31) << 5) | (rt & 31));
    }

    pub fn ldr_post16(&mut self, rt: u32, rn: u32) {
        self.word(LDR_X_POST | ((16u32 & 0x1ff) << 12) | ((rn & 31) << 5) | (rt & 31));
    }

    /// `bl .+4*words`, with the displacement written here rather than left to a
    /// relocation: the target is inside this same block.
    pub fn bl_words(&mut self, words: u32) {
        self.word(BL | (words & 0x03ff_ffff));
    }

    /// `br xn` — an indirect tail call, which leaves the link register alone.
    pub fn br_reg(&mut self, rn: u32) {
        self.word(0xd61f_0000 | ((rn & 31) << 5));
    }

    /// `ldr xt, [xn, #off]`. The unsigned-offset form scales by eight, so `off`
    /// must be eight-aligned; a layout offset for a 64-bit scalar always is.
    pub fn ldr(&mut self, rt: u32, rn: u32, off: u32) {
        self.word(LDR_X_UIMM | (((off / 8) & 0xfff) << 10) | ((rn & 31) << 5) | (rt & 31));
    }

    /// `ldr wt, [xn, #off]`, scaled by four. Zero-extends into the whole of
    /// `xt`, as every write to a `W` register does.
    pub fn ldr_w(&mut self, rt: u32, rn: u32, off: u32) {
        self.word(LDR_W_UIMM | (((off / 4) & 0xfff) << 10) | ((rn & 31) << 5) | (rt & 31));
    }

    /// `ldrh wt, [xn, #off]`, scaled by two.
    pub fn ldrh(&mut self, rt: u32, rn: u32, off: u32) {
        self.word(LDRH_W_UIMM | (((off / 2) & 0xfff) << 10) | ((rn & 31) << 5) | (rt & 31));
    }

    /// `ldrb wt, [xn, #off]`, unscaled.
    pub fn ldrb(&mut self, rt: u32, rn: u32, off: u32) {
        self.word(LDRB_W_UIMM | ((off & 0xfff) << 10) | ((rn & 31) << 5) | (rt & 31));
    }

    /// `cbz wt, <later>` — the 32-bit form, for a value whose upper half is not
    /// known to be zero, which is what a C function returning `int` leaves in
    /// `x0`.
    pub fn cbz(&mut self, rt: u32) -> Patch {
        self.cond(CBZ_W, rt)
    }

    /// `cbz xt, <later>`, for a pointer or a zero-extending load.
    pub fn cbz_x(&mut self, rt: u32) -> Patch {
        self.cond(CBZ_X, rt)
    }

    pub fn cbnz(&mut self, rt: u32) -> Patch {
        self.cond(CBNZ_W, rt)
    }

    pub fn cbnz_x(&mut self, rt: u32) -> Patch {
        self.cond(CBNZ_X, rt)
    }

    fn cond(&mut self, op: u32, rt: u32) -> Patch {
        let at = self.at();
        self.word(op | (rt & 31));
        Patch { at, kind: PatchKind::Cond19 }
    }

    /// `b <later>`.
    pub fn b(&mut self) -> Patch {
        let at = self.at();
        self.word(B);
        Patch { at, kind: PatchKind::Uncond26 }
    }

    /// Resolves a forward branch to the instruction that comes next.
    pub fn here(&mut self, p: Patch) {
        let delta = (self.at() as i64 - p.at as i64) / 4;
        let w = self.word_at(p.at);
        let patched = match p.kind {
            PatchKind::Cond19 => (w & !(0x7_ffff << 5)) | ((delta as u32 & 0x7_ffff) << 5),
            PatchKind::Uncond26 => (w & !0x3ff_ffff) | (delta as u32 & 0x3ff_ffff),
        };
        self.set_word(p.at, patched);
    }

    /// `bl name`, with the displacement left to the linker.
    pub fn bl_symbol(&mut self, name: &str) {
        let at = self.at();
        self.word(BL);
        self.relocs.push((at, RelKind::Branch26, Target::Symbol(String::from(name))));
    }

    /// `adrp xd, name` + `add xd, xd, #:lo12:name` — the address of a symbol in
    /// a register, in the two instructions and one relocation pair arm64 uses
    /// for it.
    ///
    /// The two relocations must stay adjacent and in this order: `PAGEOFF12`
    /// has no meaning on its own, and the linker reads the pair as one.
    pub fn adrp_add_symbol(&mut self, rd: u32, name: &str) {
        let page = self.at();
        self.word(ADRP | (rd & 31));
        let off = self.at();
        self.word(ADD_X_IMM | ((rd & 31) << 5) | (rd & 31));
        self.relocs.push((page, RelKind::Page21, Target::Symbol(String::from(name))));
        self.relocs.push((off, RelKind::PageOff12, Target::Symbol(String::from(name))));
    }

    pub fn ret(&mut self) {
        self.word(RET_X30);
    }

    /// `stp x29, x30, [sp, #-16]!` then `mov x29, sp`.
    ///
    /// Writes `x29`, `x30` and `sp` and nothing else, which is what lets it
    /// stand in front of a `bl` that still needs `main`'s own `w0` and `x1`.
    pub fn stp_fp_lr(&mut self) {
        self.word(STP_FP_LR_PRE);
        self.add_imm(29, SP, 0);
    }

    /// `ldp x29, x30, [sp], #16`.
    pub fn ldp_fp_lr(&mut self) {
        self.word(LDP_FP_LR_POST);
    }

    fn word_at(&self, at: u64) -> u32 {
        let o = at as usize;
        let b = self.bytes.get(o..o.saturating_add(4)).unwrap_or(&[0, 0, 0, 0]);
        u32::from_le_bytes([
            *b.first().unwrap_or(&0),
            *b.get(1).unwrap_or(&0),
            *b.get(2).unwrap_or(&0),
            *b.get(3).unwrap_or(&0),
        ])
    }

    fn set_word(&mut self, at: u64, w: u32) {
        let o = at as usize;
        if let Some(slot) = self.bytes.get_mut(o..o.saturating_add(4)) {
            slot.copy_from_slice(&w.to_le_bytes());
        }
    }

    pub fn finish(self) -> (Vec<u8>, Vec<(u64, RelKind, Target)>) {
        (self.bytes, self.relocs)
    }

    /// The same block as a [`Shim`]. Every A64 relocation here takes a zero
    /// addend: the field is the whole of what the linker writes.
    fn into_shim(self) -> Shim {
        Shim {
            bytes: self.bytes,
            relocs: self.relocs.into_iter().map(|(at, k, t)| (at, k, t, 0)).collect(),
        }
    }
}

// ---------------------------------------------------------------------------
// SysV x86-64
// ---------------------------------------------------------------------------

/// A place in an x86-64 instruction stream whose `rel32` is not known yet.
///
/// `at` is the **field**, not the instruction: a displacement is measured from
/// the instruction's end, which is four bytes past it.
#[must_use]
pub struct Patch32 {
    at: u64,
}

/// A block of hand-written x86-64 code, with the relocations it needs.
///
/// [`Asm`]'s twin, and deliberately a separate type rather than a mode: there
/// is no instruction the two share, no field width they agree on, and a shim
/// that could hold either machine's bytes is one that could put the wrong
/// machine's in an object.
pub struct X86 {
    bytes: Vec<u8>,
    relocs: Vec<ShimReloc>,
}

/// The integer registers either shim names, in encoding order.
pub const RAX: u32 = 0;
pub const RDX: u32 = 2;
pub const RSP: u32 = 4;
pub const RSI: u32 = 6;
pub const RDI: u32 = 7;

/// A `REX` prefix. `W` is the 64-bit operand size, `r` extends the `ModRM.reg`
/// field and `b` extends `ModRM.rm`; no instruction here has an index register.
fn rex(w: bool, r: u32, b: u32) -> u8 {
    0x40 | (u8::from(w) << 3) | (((r >> 3) & 1) as u8) << 2 | (((b >> 3) & 1) as u8)
}

/// A register-direct `ModRM`: `mod = 11`.
fn modrm_rr(reg: u32, rm: u32) -> u8 {
    0xc0 | (((reg & 7) as u8) << 3) | ((rm & 7) as u8)
}

impl Default for X86 {
    fn default() -> X86 {
        X86::new()
    }
}

impl X86 {
    pub fn new() -> X86 {
        X86 { bytes: Vec::new(), relocs: Vec::new() }
    }

    fn at(&self) -> u64 {
        self.bytes.len() as u64
    }

    fn put(&mut self, b: &[u8]) {
        self.bytes.extend_from_slice(b);
    }

    /// The one machine-stack pair either shim uses.
    ///
    /// `main` is entered with the return address pushed, so `rsp % 16 == 8`;
    /// one push makes it zero and every `call` below is then aligned. `rbp` is
    /// callee-saved and is what a frame pointer would have been, so this is
    /// also the prologue a debugger expects — and nothing else here writes it.
    pub fn push_rbp(&mut self) {
        self.put(&[0x55]);
    }

    pub fn pop_rbp(&mut self) {
        self.put(&[0x5d]);
    }

    pub fn ret(&mut self) {
        self.put(&[0xc3]);
    }

    /// `mov rd, #imm`.
    ///
    /// Five bytes for anything below `2^32`, because a write to a 32-bit
    /// register zero-extends into the whole of the 64-bit one; ten for the one
    /// wider value either shim forms, which is the `Str` length mask.
    pub fn mov_imm(&mut self, rd: u32, value: u64) {
        if value < (1u64 << 32) {
            if rd >= 8 {
                self.put(&[rex(false, 0, rd)]);
            }
            self.put(&[0xb8 + (rd & 7) as u8]);
            self.put(&(value as u32).to_le_bytes());
            return;
        }
        self.put(&[rex(true, 0, rd), 0xb8 + (rd & 7) as u8]);
        self.put(&value.to_le_bytes());
    }

    /// `mov rd, rs`.
    pub fn mov_reg(&mut self, rd: u32, rs: u32) {
        self.put(&[rex(true, rs, rd), 0x89, modrm_rr(rs, rd)]);
    }

    /// `and rd, rs`. The register form, because the one mask this file applies
    /// is 64 bits wide and no x86-64 ALU immediate is.
    pub fn and_reg(&mut self, rd: u32, rs: u32) {
        self.put(&[rex(true, rs, rd), 0x21, modrm_rr(rs, rd)]);
    }

    /// `add rd, #imm32` and `sub rd, #imm32`, both in the `81 /digit` form so
    /// that one encoding serves every immediate a shim forms.
    pub fn add_imm(&mut self, rd: u32, imm: u32) {
        self.alu_imm(0, rd, imm);
    }

    pub fn sub_imm(&mut self, rd: u32, imm: u32) {
        self.alu_imm(5, rd, imm);
    }

    fn alu_imm(&mut self, digit: u32, rd: u32, imm: u32) {
        self.put(&[rex(true, digit, rd), 0x81, modrm_rr(digit, rd)]);
        self.put(&imm.to_le_bytes());
    }

    /// `mov rt, [rn + off]`, and below it the three narrower loads. Every one
    /// of the four leaves the whole of `rt` defined: a 32-bit destination
    /// zero-extends, and `movzx` is zero-extending by name.
    pub fn ldr(&mut self, rt: u32, rn: u32, off: u32) {
        self.load(&[0x8b], true, rt, rn, off);
    }

    pub fn ldr_w(&mut self, rt: u32, rn: u32, off: u32) {
        self.load(&[0x8b], false, rt, rn, off);
    }

    pub fn ldrh(&mut self, rt: u32, rn: u32, off: u32) {
        self.load(&[0x0f, 0xb7], false, rt, rn, off);
    }

    pub fn ldrb(&mut self, rt: u32, rn: u32, off: u32) {
        self.load(&[0x0f, 0xb6], false, rt, rn, off);
    }

    /// `mov [rn + off], rs`.
    pub fn str_off(&mut self, rs: u32, rn: u32, off: u32) {
        self.mem(&[0x89], true, rs, rn, off);
    }

    /// `lea rd, [rn + off]`.
    pub fn lea(&mut self, rd: u32, rn: u32, off: u32) {
        self.mem(&[0x8d], true, rd, rn, off);
    }

    fn load(&mut self, op: &[u8], wide: bool, rt: u32, rn: u32, off: u32) {
        self.mem(op, wide, rt, rn, off);
    }

    /// One memory form for every load and store here: `mod = 10`, a `disp32`,
    /// and a `SIB` byte where the base register's encoding is the one that
    /// demands one.
    ///
    /// A single shape rather than the shortest per offset, because a frame
    /// offset is not bounded by a signed byte and an encoder that sometimes
    /// emits a `disp8` is one whose instruction lengths a reader has to
    /// compute rather than read.
    fn mem(&mut self, op: &[u8], wide: bool, reg: u32, base: u32, off: u32) {
        let r = rex(wide, reg, base);
        if r != 0x40 {
            self.put(&[r]);
        }
        self.put(op);
        self.put(&[0x80 | (((reg & 7) as u8) << 3) | ((base & 7) as u8)]);
        // `rm = 100` is not a register: it says a `SIB` byte follows. `rsp`
        // and `r12` encode as 100, so both need one, and `0x24` is the byte
        // that means "this base, no index".
        if base & 7 == 4 {
            self.put(&[0x24]);
        }
        self.put(&off.to_le_bytes());
    }

    /// `test wt, wt` then `jz <later>` — the 32-bit form, for a value whose
    /// upper half is not known to be zero, which is what a C function
    /// returning `int` leaves in `rax`.
    pub fn cbz(&mut self, rt: u32) -> Patch32 {
        self.test(false, rt);
        self.jcc(0x84)
    }

    /// `test rt, rt` then `jz`/`jnz`, for a pointer or a zero-extending load.
    pub fn cbz_x(&mut self, rt: u32) -> Patch32 {
        self.test(true, rt);
        self.jcc(0x84)
    }

    pub fn cbnz_x(&mut self, rt: u32) -> Patch32 {
        self.test(true, rt);
        self.jcc(0x85)
    }

    fn test(&mut self, wide: bool, rt: u32) {
        let r = rex(wide, rt, rt);
        if r != 0x40 {
            self.put(&[r]);
        }
        self.put(&[0x85, modrm_rr(rt, rt)]);
    }

    /// A `jcc rel32`, whose field is filled in by [`X86::here`]. Always the
    /// 32-bit form: this machine has one, so a shim never has to know how far
    /// its own arms are.
    fn jcc(&mut self, cc: u8) -> Patch32 {
        self.put(&[0x0f, cc]);
        let at = self.at();
        self.put(&[0, 0, 0, 0]);
        Patch32 { at }
    }

    /// Resolves a forward branch to the instruction that comes next.
    pub fn here(&mut self, p: Patch32) {
        // Measured from the end of the instruction, which is the four bytes
        // of the field itself.
        let delta = self.at() as i64 - (p.at as i64 + 4);
        let o = p.at as usize;
        if let Some(slot) = self.bytes.get_mut(o..o.saturating_add(4)) {
            slot.copy_from_slice(&(delta as i32).to_le_bytes());
        }
    }

    /// `call name`, with the displacement left to the linker.
    pub fn call_symbol(&mut self, name: &str) {
        self.put(&[0xe8]);
        self.pcrel(RelKind::Rel32, name);
    }

    /// `lea rd, [rip + name]` — the address of a symbol in one instruction and
    /// one relocation, where A64 needs an `adrp`/`add` pair and two.
    pub fn lea_symbol(&mut self, rd: u32, name: &str) {
        self.put(&[rex(true, rd, 0), 0x8d, (((rd & 7) as u8) << 3) | 0x05]);
        self.pcrel(RelKind::Pc32, name);
    }

    /// The four zero bytes of a pc-relative field, and the relocation that
    /// fills them. The addend is `-4` because the field is the last thing in
    /// the instruction and the processor measures from the instruction's end.
    fn pcrel(&mut self, kind: RelKind, name: &str) {
        let at = self.at();
        self.put(&[0, 0, 0, 0]);
        self.relocs.push((at, kind, Target::Symbol(String::from(name)), -4));
    }

    /// `call .+bytes` — a call to a place this many bytes past the end of this
    /// instruction, with the displacement written here rather than left to a
    /// relocation: the target is inside this same block.
    pub fn call_ahead(&mut self, bytes: i32) {
        self.put(&[0xe8]);
        self.put(&bytes.to_le_bytes());
    }

    /// `jmp rn` — an indirect tail call, which pushes nothing.
    pub fn jmp_reg(&mut self, rn: u32) {
        if rn >= 8 {
            self.put(&[rex(false, 0, rn)]);
        }
        self.put(&[0xff, 0xe0 | ((rn & 7) as u8)]);
    }

    pub fn finish(self) -> (Vec<u8>, Vec<ShimReloc>) {
        (self.bytes, self.relocs)
    }

    fn into_shim(self) -> Shim {
        Shim { bytes: self.bytes, relocs: self.relocs }
    }
}

/// Where a `main` returning `Result<(), Str>` keeps its answer.
///
/// Read off `middle::layout` by the caller and passed in, so that this file
/// never learns a layout rule: it is told an offset and a width and it emits
/// the load. `None` in place of the whole struct is a zero-sized return or a
/// return that is not an enum — a `main` answering `()` — which is a success
/// unconditionally.
#[derive(Clone)]
pub struct MainResult {
    /// Byte offset of the tag within the return area, and its width in bytes.
    pub tag: (u32, u32),
    /// `Some(offset)` when the layout is a niche, so there is no tag word: the
    /// value *is* the payload and the discriminant is a pointer at this offset
    /// being null.
    ///
    /// A null pointer is the **error** arm, not the success one. `layout.rs`'s
    /// `EnumRepr::Niche` is `Option<T>` with `.Some` at variant 0 and `.None`
    /// at variant 1, and null means `.None` — so `is_null` *is* the variant
    /// index, and a non-zero index is the failure arm by the same rule the tag
    /// case uses. Every backend's entry point computes exactly this — the tag
    /// is the pointer compared against null — and they must agree.
    pub niche: Option<u32>,
    /// Byte offsets of the `Str`'s three words — `base`, `ptr`, `len` — within
    /// the return area, for the error arm.
    pub message: (u32, u32, u32),
}

// The registers the shims use. `x0`–`x2` carry arguments and `x9`–`x12` are
// AAPCS64 temporaries, so nothing here has to be saved across the `bl`s.
const X0: u32 = 0;
const X1: u32 = 1;
const X2: u32 = 2;
const X9: u32 = 9;
const X10: u32 = 10;

/// Turns the top [`GUARD_BYTES`] of the Buri stack into an unmapped hole, so
/// that a runaway recursion **faults** where it would otherwise have kept
/// writing.
///
/// Nine instructions, once per process, and the whole of the answer to
/// `design/native/CODEGEN-STENCIL.md` §8. What it does is what the kernel does
/// for a thread stack — a `PROT_NONE` page on the side the stack grows towards
/// — done by the program because this stack is a `__bss` block the program
/// owns rather than a mapping the kernel made.
///
/// **What each backend guards, and where.** A stencil artifact runs on two
/// stacks. Generated Buri code uses only this one, and it is the one with no
/// kernel guard, which is this function. The *machine* stack is used by the
/// `crt` stencils' own prologues and by `glue.rs`'s `extern "C"` stubs — drop
/// glue recurses, `[[Str]]` releasing `[Str]` releasing `Str` — and that stack
/// is the OS's, already guarded, on both backends and in every mode. There is
/// no in-process JIT mode left to guard separately: this backend writes objects
/// and the linker makes the artifact (`region.rs`'s header), so `main` is the
/// only place a stack is established at all.
///
/// **On failure the program stops.** `mprotect` over a page-aligned range of a
/// mapping the process owns does not fail in practice, but a program running
/// without the guard it believes it has is the exact condition this function
/// exists to remove, so a non-zero answer goes to `abort` rather than being
/// ignored. The result is a `SIGABRT` at startup instead of a silent corruption
/// later.
fn install_guard(a: &mut Asm) {
    // The guard has no symbol of its own — see [`STACK_BYTES`] — so its address
    // is the stack's plus a constant, and the constant is far past an `imm12`.
    a.adrp_add_symbol(X0, STACK_SYMBOL);
    a.mov_imm(X9, STACK_USABLE);
    a.add_reg(X0, X0, X9);
    a.mov_imm(X1, GUARD_BYTES);
    a.mov_imm(X2, PROT_NONE);
    a.bl_symbol("mprotect");
    // `mprotect` answers an `int`, so the test is the 32-bit one: `-1` is
    // `0xffff_ffff` in `w0` and the upper half of `x0` is not the ABI's to
    // promise.
    let ok = a.cbz(X0);
    a.bl_symbol("abort");
    a.here(ok);
}

/// `PROT_NONE`, which is zero on every platform that has the call and is
/// spelled here because a bare `0` in an argument register says nothing.
const PROT_NONE: u64 = 0;

/// `int main(int argc, char **argv)` for a program whose root is `main`.
///
/// `llvm/emit.rs::entry_point` behaviour for behaviour: `buri_rt_argv_init`
/// first, the root, then the exit convention the JavaScript backend already
/// has — `.Ok(())` flushes and exits 0, `.Err(msg)` writes `msg` to standard
/// error, flushes and exits 1.
///
/// The target picks the machine and nothing else: the two bodies below are the
/// same program in two instruction sets, and a difference between them that is
/// not forced by the ISA is a bug in one of them.
pub fn program_entry(target: StencilTarget, callee: &str, result: Option<MainResult>) -> Shim {
    if target.is_arm64() {
        return program_entry_arm64(callee, result).into_shim();
    }
    program_entry_x86_64(callee, result).into_shim()
}

fn program_entry_arm64(callee: &str, result: Option<MainResult>) -> Asm {
    let mut a = Asm::new();
    a.stp_fp_lr();
    a.bl_symbol("buri_rt_argv_init");
    install_guard(&mut a);

    // The root's frame is the bottom of the Buri stack, so its return area —
    // which begins at offset 0 of the frame — is at the stack base itself. The
    // callee answers the `fp` it was handed, so `x0` still holds that base on
    // return and the loads below need no second `adrp` pair.
    a.adrp_add_symbol(X0, STACK_SYMBOL);
    a.bl_symbol(callee);

    let Some(r) = result else {
        a.bl_symbol("buri_rt_flush");
        a.mov_imm(X0, 0);
        a.ldp_fp_lr();
        a.ret();
        return a;
    };

    let ok = match r.niche {
        Some(null_at) => {
            a.ldr(X9, X0, null_at);
            a.cbnz_x(X9)
        }
        None => {
            let (off, width) = r.tag;
            load_tag(&mut a, X9, X0, off, width);
            a.cbz_x(X9)
        }
    };

    // The error arm falls through, so the success arm is the branch target and
    // both ends in a `ret` — there is no join and therefore no `b`.
    let (base, ptr, len) = r.message;
    a.ldr(X9, X0, base);
    a.ldr(X1, X0, ptr);
    a.ldr(X2, X0, len);
    a.mov_imm(X10, crate::compiler::middle::layout::STR_LEN_MASK);
    a.and_reg(X2, X2, X10);
    a.mov_reg(X0, X9);
    a.bl_symbol("buri_rt_host_stderr_eprintln");
    a.bl_symbol("buri_rt_flush");
    a.mov_imm(X0, 1);
    a.ldp_fp_lr();
    a.ret();

    a.here(ok);
    a.bl_symbol("buri_rt_flush");
    a.mov_imm(X0, 0);
    a.ldp_fp_lr();
    a.ret();
    a
}

/// The tag, zero-extended into the whole of `rt` so that one 64-bit `cbz`
/// decides every width.
///
/// A width this does not recognise loads eight bytes, which is the widest read
/// the return area can support and so cannot read outside it.
fn load_tag(a: &mut Asm, rt: u32, rn: u32, off: u32, width: u32) {
    match width {
        1 => a.ldrb(rt, rn, off),
        2 => a.ldrh(rt, rn, off),
        4 => a.ldr_w(rt, rn, off),
        _ => a.ldr(rt, rn, off),
    }
}

/// `int main(int argc, char **argv)` for a test binary.
///
/// `llvm/emit.rs::test_entry_point` behaviour for behaviour: every `test`
/// block in order, each behind `buri_rt_test_enter`'s answer about whether this
/// process is to run it. A failed assertion is an abort (SPEC 6.10), so one
/// process reports at most one failure and the runner re-runs the binary with
/// `enter` answering 0 for everything already reported — which is why the skip
/// is a branch around the call and not a call that returns early.
///
/// `argc` and `argv` arrive in `w0` and `x1` and have to reach
/// `buri_rt_argv_init` unchanged, so the prologue is the one instruction pair
/// that touches neither and the `bl` comes before anything else at all.
pub fn test_entry(target: StencilTarget, tests: &[String]) -> Shim {
    if target.is_arm64() {
        return test_entry_arm64(tests).into_shim();
    }
    test_entry_x86_64(tests).into_shim()
}

fn test_entry_arm64(tests: &[String]) -> Asm {
    let mut a = Asm::new();
    a.stp_fp_lr();
    a.bl_symbol("buri_rt_argv_init");
    install_guard(&mut a);
    for (i, sym) in tests.iter().enumerate() {
        a.mov_imm(X0, i as u64);
        a.bl_symbol("buri_rt_test_enter");
        let next = a.cbz(X0);
        a.adrp_add_symbol(X0, STACK_SYMBOL);
        a.bl_symbol(sym);
        a.here(next);
    }
    a.bl_symbol("buri_rt_flush");
    a.mov_imm(X0, 0);
    a.ldp_fp_lr();
    a.ret();
    a
}

// ---------------------------------------------------------------------------
// The same two shims, for SysV x86-64
// ---------------------------------------------------------------------------

/// [`install_guard`] for the other machine.
///
/// Six instructions rather than nine, and the saving is the whole of the
/// difference between the two address forms: `lea` names the stack in one
/// instruction where `adrp`/`add` needs two, and `add rdi, #imm32` reaches the
/// guard where A64 had to materialise the distance into a register first.
fn install_guard_x86_64(a: &mut X86) {
    a.lea_symbol(RDI, STACK_SYMBOL);
    a.add_imm(RDI, STACK_USABLE as u32);
    a.mov_imm(RSI, GUARD_BYTES);
    a.mov_imm(RDX, PROT_NONE);
    a.call_symbol("mprotect");
    // `mprotect` answers an `int`, so the test is the 32-bit one.
    let ok = a.cbz(RAX);
    a.call_symbol("abort");
    a.here(ok);
}

/// [`program_entry`] for SysV x86-64.
///
/// The one structural difference from the A64 body: an emitted function answers
/// the frame pointer in `rax` rather than in the register it was handed, so the
/// return area is read off `rax` and the message's three words go straight into
/// the argument registers instead of through a temporary.
fn program_entry_x86_64(callee: &str, result: Option<MainResult>) -> X86 {
    let mut a = X86::new();
    a.push_rbp();
    // `argc` and `argv` are already in `edi` and `rsi`, and `push` writes
    // neither, so this call comes before anything else at all.
    a.call_symbol("buri_rt_argv_init");
    install_guard_x86_64(&mut a);

    a.lea_symbol(RDI, STACK_SYMBOL);
    a.call_symbol(callee);

    let Some(r) = result else {
        a.call_symbol("buri_rt_flush");
        a.mov_imm(RAX, 0);
        a.pop_rbp();
        a.ret();
        return a;
    };

    let ok = match r.niche {
        Some(null_at) => {
            a.ldr(RSI, RAX, null_at);
            a.cbnz_x(RSI)
        }
        None => {
            let (off, width) = r.tag;
            load_tag_x86_64(&mut a, RSI, RAX, off, width);
            a.cbz_x(RSI)
        }
    };

    // The error arm falls through, exactly as it does on A64.
    let (base, ptr, len) = r.message;
    a.ldr(RDX, RAX, len);
    a.ldr(RSI, RAX, ptr);
    a.ldr(RDI, RAX, base);
    a.mov_imm(RAX, crate::compiler::middle::layout::STR_LEN_MASK);
    a.and_reg(RDX, RAX);
    a.call_symbol("buri_rt_host_stderr_eprintln");
    a.call_symbol("buri_rt_flush");
    a.mov_imm(RAX, 1);
    a.pop_rbp();
    a.ret();

    a.here(ok);
    a.call_symbol("buri_rt_flush");
    a.mov_imm(RAX, 0);
    a.pop_rbp();
    a.ret();
    a
}

/// [`load_tag`] for the other machine. Every width zero-extends into the whole
/// of `rt`, so the one 64-bit `test` after it decides all four.
fn load_tag_x86_64(a: &mut X86, rt: u32, rn: u32, off: u32, width: u32) {
    match width {
        1 => a.ldrb(rt, rn, off),
        2 => a.ldrh(rt, rn, off),
        4 => a.ldr_w(rt, rn, off),
        _ => a.ldr(rt, rn, off),
    }
}

/// [`test_entry`] for SysV x86-64.
fn test_entry_x86_64(tests: &[String]) -> X86 {
    let mut a = X86::new();
    a.push_rbp();
    a.call_symbol("buri_rt_argv_init");
    install_guard_x86_64(&mut a);
    for (i, sym) in tests.iter().enumerate() {
        a.mov_imm(RDI, i as u64);
        a.call_symbol("buri_rt_test_enter");
        let next = a.cbz(RAX);
        a.lea_symbol(RDI, STACK_SYMBOL);
        a.call_symbol(sym);
        a.here(next);
    }
    a.call_symbol("buri_rt_flush");
    a.mov_imm(RAX, 0);
    a.pop_rbp();
    a.ret();
    a
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(a: Asm) -> Vec<u32> {
        let (bytes, _) = a.finish();
        bytes
            .chunks_exact(4)
            .map(|c| {
                u32::from_le_bytes([
                    *c.first().unwrap_or(&0),
                    *c.get(1).unwrap_or(&0),
                    *c.get(2).unwrap_or(&0),
                    *c.get(3).unwrap_or(&0),
                ])
            })
            .collect()
    }

    /// `movz x0, #5`, one instruction: every halfword above the first is zero
    /// and contributes nothing.
    #[test]
    fn a_small_immediate_is_one_movz() {
        let mut a = Asm::new();
        a.mov_imm(0, 5);
        assert_eq!(words(a), vec![0xd280_00a0]);
    }

    /// Zero is the value with no non-zero halfword, and it still has to move.
    #[test]
    fn zero_is_a_movz_of_zero() {
        let mut a = Asm::new();
        a.mov_imm(0, 0);
        assert_eq!(words(a), vec![0xd280_0000]);
    }

    /// `STR_LEN_MASK` into `x3`: `movz` then three `movk`s, low halfword first.
    #[test]
    fn a_wide_immediate_is_a_movz_and_three_movks() {
        let mut a = Asm::new();
        a.mov_imm(3, 0x7fff_ffff_ffff_ffff);
        assert_eq!(
            words(a),
            vec![0xd29f_ffe3, 0xf2bf_ffe3, 0xf2df_ffe3, 0xf2ef_ffe3]
        );
    }

    #[test]
    fn the_fixed_instructions_are_the_bytes_they_have_to_be() {
        let mut a = Asm::new();
        a.ret();
        a.stp_fp_lr();
        a.ldp_fp_lr();
        a.mov_reg(0, 9);
        assert_eq!(
            words(a),
            vec![0xd65f_03c0, 0xa9bf_7bfd, 0x9100_03fd, 0xa8c1_7bfd, 0xaa09_03e0]
        );
    }

    /// Three instructions are skipped, so the displacement is four words and
    /// lands in `imm19` at bits 5..24.
    #[test]
    fn a_patched_cbz_carries_the_word_displacement() {
        let mut a = Asm::new();
        let p = a.cbz(0);
        a.ret();
        a.ret();
        a.ret();
        a.here(p);
        let got = words(a);
        assert_eq!(got.first().copied(), Some(CBZ_W | (4 << 5)));
        assert_eq!(got.first().copied().map(|w| (w >> 5) & 0x7_ffff), Some(4));
    }

    /// A `b` with nothing between it and its target is `b .+4`, which proves
    /// the 26-bit field is the one being written.
    #[test]
    fn a_patched_b_carries_the_word_displacement() {
        let mut a = Asm::new();
        let p = a.b();
        a.here(p);
        assert_eq!(words(a), vec![B | 1]);
    }

    /// `RelKind` is not `Debug`, and this file does not get to add a derive to
    /// `object.rs` for a test's benefit, so a kind is compared by the name the
    /// assertion would print anyway.
    fn kind(k: RelKind) -> &'static str {
        match k {
            RelKind::Branch26 => "Branch26",
            RelKind::Abs64 => "Abs64",
            RelKind::Page21 => "Page21",
            RelKind::PageOff12 => "PageOff12",
            RelKind::Rel32 => "Rel32",
            RelKind::Pc32 => "Pc32",
        }
    }

    fn names(s: Shim) -> Vec<(&'static str, String)> {
        s.relocs
            .into_iter()
            .map(|(_, k, t, _)| {
                let n = match t {
                    Target::Symbol(s) => s,
                    other => format!("{other:?}"),
                };
                (kind(k), n)
            })
            .collect()
    }

    /// Every shim is asked for by target, and the arm64 one is the twin the
    /// x86-64 one was read off.
    const ARM64: StencilTarget = StencilTarget::MacosArm64;
    const X86_64: StencilTarget = StencilTarget::LinuxX86_64;

    /// The order is the contract: `argv_init` before anything writes `x0`, the
    /// `PAGE21`/`PAGEOFF12` pair adjacent and in that order, and the test's own
    /// symbol after the pair that forms its argument.
    #[test]
    fn the_test_shim_relocates_what_it_calls_in_order() {
        let a = test_entry(ARM64, &[String::from("t0"), String::from("t1")]);
        assert_eq!(
            names(a),
            vec![
                ("Branch26", String::from("buri_rt_argv_init")),
                ("Page21", String::from(STACK_SYMBOL)),
                ("PageOff12", String::from(STACK_SYMBOL)),
                ("Branch26", String::from("mprotect")),
                ("Branch26", String::from("abort")),
                ("Branch26", String::from("buri_rt_test_enter")),
                ("Page21", String::from(STACK_SYMBOL)),
                ("PageOff12", String::from(STACK_SYMBOL)),
                ("Branch26", String::from("t0")),
                ("Branch26", String::from("buri_rt_test_enter")),
                ("Page21", String::from(STACK_SYMBOL)),
                ("PageOff12", String::from(STACK_SYMBOL)),
                ("Branch26", String::from("t1")),
                ("Branch26", String::from("buri_rt_flush")),
            ]
        );
    }

    /// A shim for a suite with no blocks is still a valid `main`: it initialises
    /// the runtime, flushes it and answers 0.
    #[test]
    fn an_empty_suite_is_still_a_main() {
        let a = test_entry(ARM64, &[]);
        assert_eq!(
            names(a),
            vec![
                ("Branch26", String::from("buri_rt_argv_init")),
                ("Page21", String::from(STACK_SYMBOL)),
                ("PageOff12", String::from(STACK_SYMBOL)),
                ("Branch26", String::from("mprotect")),
                ("Branch26", String::from("abort")),
                ("Branch26", String::from("buri_rt_flush")),
            ]
        );
    }

    /// A `main` answering `()` never inspects the return area, so it calls the
    /// writer for no message and exits 0 on every path.
    #[test]
    fn a_main_without_a_result_never_reads_the_return_area() {
        let a = program_entry(ARM64, "buri$main", None);
        assert_eq!(
            names(a),
            vec![
                ("Branch26", String::from("buri_rt_argv_init")),
                ("Page21", String::from(STACK_SYMBOL)),
                ("PageOff12", String::from(STACK_SYMBOL)),
                ("Branch26", String::from("mprotect")),
                ("Branch26", String::from("abort")),
                ("Page21", String::from(STACK_SYMBOL)),
                ("PageOff12", String::from(STACK_SYMBOL)),
                ("Branch26", String::from("buri$main")),
                ("Branch26", String::from("buri_rt_flush")),
            ]
        );
    }

    /// The error arm falls through and the success arm is the branch target, so
    /// `flush` appears twice and the writer only on the first of them.
    #[test]
    fn a_failing_main_writes_the_message_before_it_flushes() {
        let a = program_entry(
            ARM64,
            "buri$main",
            Some(MainResult { tag: (0, 4), niche: None, message: (8, 16, 24) }),
        );
        assert_eq!(
            names(a),
            vec![
                ("Branch26", String::from("buri_rt_argv_init")),
                ("Page21", String::from(STACK_SYMBOL)),
                ("PageOff12", String::from(STACK_SYMBOL)),
                ("Branch26", String::from("mprotect")),
                ("Branch26", String::from("abort")),
                ("Page21", String::from(STACK_SYMBOL)),
                ("PageOff12", String::from(STACK_SYMBOL)),
                ("Branch26", String::from("buri$main")),
                ("Branch26", String::from("buri_rt_host_stderr_eprintln")),
                ("Branch26", String::from("buri_rt_flush")),
                ("Branch26", String::from("buri_rt_flush")),
            ]
        );
    }

    /// The niche test is `cbnz`: a **non-null** pointer is `.Some`, variant 0,
    /// the success arm. The tag test is `cbz` on the same register. Getting
    /// these the same way round would make every failing program exit 0.
    #[test]
    fn the_niche_test_is_the_opposite_of_the_tag_test() {
        let niche = program_entry_arm64(
            "m",
            Some(MainResult { tag: (0, 8), niche: Some(8), message: (0, 8, 16) }),
        );
        let tagged = program_entry_arm64(
            "m",
            Some(MainResult { tag: (0, 8), niche: None, message: (8, 16, 24) }),
        );
        // The only compare-and-branch in either shim is the one being asked
        // about, so the first word with a `cbz`/`cbnz` opcode is it.
        let branch = |a: Asm| {
            words(a)
                .into_iter()
                .map(|w| w & 0xff00_0000)
                .find(|op| *op == CBZ_X || *op == CBNZ_X)
        };
        assert_eq!(branch(niche), Some(CBNZ_X));
        assert_eq!(branch(tagged), Some(CBZ_X));
    }

    /// Every tag width loads zero-extended into the whole register, so the one
    /// 64-bit `cbz` after it is correct for all four.
    #[test]
    fn each_tag_width_has_its_own_load() {
        let of = |w: u32| {
            let mut a = Asm::new();
            load_tag(&mut a, 9, 0, 8, w);
            words(a).first().copied().unwrap_or(0) & 0xffc0_0000
        };
        assert_eq!(of(1), LDRB_W_UIMM);
        assert_eq!(of(2), LDRH_W_UIMM);
        assert_eq!(of(4), LDR_W_UIMM);
        assert_eq!(of(8), LDR_X_UIMM);
    }

    /// The five instructions `glue.rs`'s C-ABI stub is made of, as the bytes
    /// they have to be.
    ///
    /// A glue function is entered by the *runtime* and its body is
    /// frame-threaded, so the stub is the whole of the bridge: keep the return
    /// address, take a frame off the machine stack, put the argument in its
    /// first slot, and hand the frame pointer over. Getting the pre- and
    /// post-index forms the wrong way round is a stack that grows every call.
    #[test]
    fn the_glue_stubs_bridge_is_the_bytes_it_has_to_be() {
        let mut a = Asm::new();
        a.str_pre16(30, SP);
        a.sub_imm(SP, SP, 512);
        a.str_off(0, SP, 0);
        a.add_imm(0, SP, 0);
        a.bl_words(4);
        a.add_imm(SP, SP, 512);
        a.ldr_post16(30, SP);
        a.br_reg(1);
        assert_eq!(
            words(a),
            vec![
                0xf81f_0ffe, // str  x30, [sp, #-16]!
                0xd108_03ff, // sub  sp, sp, #512
                0xf900_03e0, // str  x0, [sp]
                0x9100_03e0, // mov  x0, sp
                0x9400_0004, // bl   .+16
                0x9108_03ff, // add  sp, sp, #512
                0xf841_07fe, // ldr  x30, [sp], #16
                0xd61f_0020, // br   x1
            ]
        );
    }

    /// The stack has to be at least as aligned as the widest scalar a frame can
    /// hold, or every layout offset in the program is wrong by the same amount —
    /// and at least as aligned as a page, or the guard's `mprotect` is a call
    /// the kernel refuses.
    #[test]
    fn the_stack_is_aligned_for_the_widest_scalar_and_for_a_page() {
        // Sixteen bytes covers `i128` and `f64`; sixteen kilobytes is the page
        // the guard's `mprotect` needs, and it is the binding one.
        assert_eq!(1u64 << STACK_ALIGN, 16 * 1024);
        assert_eq!(STACK_BYTES % (1u64 << STACK_ALIGN), 0);
    }

    /// The guard is *inside* the block and above the usable stack, so that
    /// nothing but this program's own address space is ever `mprotect`ed and
    /// the guard's base is on a page whatever the block's address.
    #[test]
    fn the_guard_is_a_whole_number_of_pages_at_the_top_of_the_block() {
        assert_eq!(STACK_BYTES, STACK_USABLE + GUARD_BYTES);
        assert_eq!(STACK_USABLE % (1u64 << STACK_ALIGN), 0);
        assert_eq!(GUARD_BYTES % (1u64 << STACK_ALIGN), 0);
    }

    // -- SysV x86-64 -------------------------------------------------------
    //
    // Every byte sequence asserted below was disassembled with `llvm-mc
    // -triple=x86_64-unknown-linux-gnu` and read back against what it is
    // named for. The comments are that listing.

    /// The encoders, as the bytes they have to be. One instruction of every
    /// form either shim uses, including the two the `rsp` base makes special.
    #[test]
    fn the_x86_64_encoders_are_the_bytes_they_have_to_be() {
        let mut a = X86::new();
        a.push_rbp();
        a.sub_imm(RSP, 512);
        a.str_off(RDI, RSP, 8);
        a.mov_reg(RDI, RSP);
        a.ldr(RSI, RAX, 3);
        a.and_reg(RDX, RAX);
        a.add_imm(RSP, 512);
        a.pop_rbp();
        a.jmp_reg(RSI);
        a.ret();
        let (bytes, _) = a.finish();
        assert_eq!(
            bytes,
            vec![
                0x55, // push  %rbp
                0x48, 0x81, 0xec, 0x00, 0x02, 0x00, 0x00, // sub   $512, %rsp
                0x48, 0x89, 0xbc, 0x24, 0x08, 0x00, 0x00, 0x00, // mov  %rdi, 8(%rsp)
                0x48, 0x89, 0xe7, // mov   %rsp, %rdi
                0x48, 0x8b, 0xb0, 0x03, 0x00, 0x00, 0x00, // mov  3(%rax), %rsi
                0x48, 0x21, 0xc2, // and   %rax, %rdx
                0x48, 0x81, 0xc4, 0x00, 0x02, 0x00, 0x00, // add   $512, %rsp
                0x5d, // pop   %rbp
                0xff, 0xe6, // jmp   *%rsi
                0xc3, // ret
            ]
        );
    }

    /// A base register whose encoding is `100` is not a register: it says a
    /// `SIB` byte follows. `rsp` is that encoding, so a store through it is one
    /// byte longer than the same store through `rax`, and an encoder that
    /// forgot would address `[rsp*1 + disp]` — which is not a thing.
    #[test]
    fn a_stack_pointer_base_takes_a_sib_byte() {
        let mut a = X86::new();
        a.str_off(RDI, RSP, 8);
        a.str_off(RDI, RAX, 8);
        let (bytes, _) = a.finish();
        assert_eq!(bytes.get(0..4), Some(&[0x48, 0x89, 0xbc, 0x24][..]));
        assert_eq!(bytes.get(8..11), Some(&[0x48, 0x89, 0xb8][..]));
        assert_eq!(bytes.len(), 8 + 7);
    }

    /// A value below `2^32` moves in five bytes and zero-extends; the `Str`
    /// length mask is the one value either shim forms that needs ten.
    #[test]
    fn an_immediate_is_five_bytes_until_it_needs_ten() {
        let mut a = X86::new();
        a.mov_imm(RAX, 1);
        a.mov_imm(RAX, 0x7fff_ffff_ffff_ffff);
        let (bytes, _) = a.finish();
        assert_eq!(bytes.get(0..5), Some(&[0xb8, 0x01, 0x00, 0x00, 0x00][..]));
        assert_eq!(bytes.get(5..7), Some(&[0x48, 0xb8][..]));
        assert_eq!(bytes.len(), 5 + 10);
    }

    /// The order is the same contract the A64 shim has, in the same words:
    /// `argv_init` before anything writes an argument register, the guard, then
    /// each test's own symbol behind the address of the stack.
    #[test]
    fn the_x86_64_test_shim_relocates_what_it_calls_in_order() {
        let a = test_entry(X86_64, &[String::from("t0"), String::from("t1")]);
        assert_eq!(
            names(a),
            vec![
                ("Rel32", String::from("buri_rt_argv_init")),
                ("Pc32", String::from(STACK_SYMBOL)),
                ("Rel32", String::from("mprotect")),
                ("Rel32", String::from("abort")),
                ("Rel32", String::from("buri_rt_test_enter")),
                ("Pc32", String::from(STACK_SYMBOL)),
                ("Rel32", String::from("t0")),
                ("Rel32", String::from("buri_rt_test_enter")),
                ("Pc32", String::from(STACK_SYMBOL)),
                ("Rel32", String::from("t1")),
                ("Rel32", String::from("buri_rt_flush")),
            ]
        );
    }

    /// The x86-64 shim calls exactly what the A64 one calls, in the same
    /// order. The kinds differ because the machines do; the *names* may not,
    /// because a program that fails must print the same thing whichever machine
    /// built it.
    #[test]
    fn the_two_machines_shims_call_the_same_symbols_in_the_same_order() {
        for r in [
            None,
            Some(MainResult { tag: (0, 4), niche: None, message: (8, 16, 24) }),
            Some(MainResult { tag: (0, 8), niche: Some(8), message: (0, 8, 16) }),
        ] {
            let arm = names(program_entry(ARM64, "m", r.clone()));
            let x86 = names(program_entry(X86_64, "m", r));
            // A64 spells an address as an adjacent `Page21`/`PageOff12` pair
            // and x86-64 as one `Pc32`, so the pairs collapse before the two
            // lists can be compared at all.
            let arm: Vec<String> = arm
                .into_iter()
                .filter(|(k, _)| *k != "PageOff12")
                .map(|(_, n)| n)
                .collect();
            let x86: Vec<String> = x86.into_iter().map(|(_, n)| n).collect();
            assert_eq!(arm, x86);
        }
    }

    /// Every x86-64 relocation names a four-byte field that **ends** its
    /// instruction, which is what an addend of `-4` says. A site with any other
    /// addend would be aimed past its target by the difference.
    #[test]
    fn every_x86_64_relocation_is_a_field_at_the_end_of_its_instruction() {
        let s = test_entry(X86_64, &[String::from("t0")]);
        assert!(!s.relocs.is_empty());
        for (at, _, _, addend) in &s.relocs {
            assert_eq!(*addend, -4);
            assert!(at + 4 <= s.bytes.len() as u64, "a field at {at} runs past the shim");
            // The four bytes of a field are left zero: the linker writes them.
            let field = s.bytes.get(*at as usize..*at as usize + 4);
            assert_eq!(field, Some(&[0u8, 0, 0, 0][..]));
        }
    }

    /// **The machine stack is sixteen-aligned at every `call`.**
    ///
    /// `main` is entered with the return address pushed, so `rsp % 16 == 8`;
    /// the single `push rbp` makes it zero and nothing else in either shim
    /// writes `rsp`. The check is structural because it has to be: the property
    /// is about every path, and a shim has three.
    #[test]
    fn the_x86_64_shim_keeps_the_machine_stack_aligned() {
        for bytes in [
            program_entry(X86_64, "m", None).bytes,
            program_entry(
                X86_64,
                "m",
                Some(MainResult { tag: (0, 4), niche: None, message: (8, 16, 24) }),
            )
            .bytes,
            test_entry(X86_64, &[String::from("t0")]).bytes,
        ] {
            assert_eq!(bytes.first(), Some(&0x55), "the shim does not open with `push %rbp`");
            // `48 81 ec` and `48 81 c4` are `sub`/`add` on `rsp`; neither may
            // appear, because either would put the alignment back where the
            // entry found it.
            for w in bytes.windows(3) {
                assert_ne!(w, [0x48, 0x81, 0xec], "the shim moves rsp");
                assert_ne!(w, [0x48, 0x81, 0xc4], "the shim moves rsp");
            }
            // Every `ret` is preceded by the one `pop %rbp` that balances the
            // push, so no path leaves the machine stack where it found it.
            for (i, b) in bytes.iter().enumerate() {
                if *b == 0xc3 && i > 0 {
                    assert_eq!(bytes.get(i - 1), Some(&0x5d), "a `ret` with no `pop %rbp`");
                }
            }
        }
    }

    /// The niche test is `jne` and the tag test is `je`, on the same register:
    /// the same way round as A64's `cbnz`/`cbz`, for the reason
    /// [`MainResult::niche`] gives. Getting these the same way round would make
    /// every failing program exit 0.
    #[test]
    fn the_x86_64_niche_test_is_the_opposite_of_the_tag_test() {
        // `48 85 f6` is `test %rsi, %rsi`, the only 64-bit test either shim
        // emits — the guard's is `85 c0`, on `eax` — and the two bytes after
        // it are the condition: `0f 84` is `je`, `0f 85` is `jne`.
        let branch = |s: Shim| {
            s.bytes
                .windows(5)
                .find(|w| w.get(0..3) == Some(&[0x48, 0x85, 0xf6][..]))
                .and_then(|w| w.get(3..5).map(<[u8]>::to_vec))
        };
        let niche = program_entry(
            X86_64,
            "m",
            Some(MainResult { tag: (0, 8), niche: Some(8), message: (0, 8, 16) }),
        );
        let tagged = program_entry(
            X86_64,
            "m",
            Some(MainResult { tag: (0, 8), niche: None, message: (8, 16, 24) }),
        );
        assert_eq!(branch(niche), Some(vec![0x0f, 0x85]));
        assert_eq!(branch(tagged), Some(vec![0x0f, 0x84]));
    }

    /// Every tag width loads zero-extended into the whole register, so the one
    /// 64-bit `test` after it is correct for all four.
    #[test]
    fn each_x86_64_tag_width_has_its_own_load() {
        let of = |w: u32| {
            let mut a = X86::new();
            load_tag_x86_64(&mut a, RSI, RAX, 8, w);
            let (b, _) = a.finish();
            b.get(0..2).map(<[u8]>::to_vec)
        };
        assert_eq!(of(1), Some(vec![0x0f, 0xb6])); // movzbl
        assert_eq!(of(2), Some(vec![0x0f, 0xb7])); // movzwl
        assert_eq!(of(4), Some(vec![0x8b, 0xb0])); // mov r32
        assert_eq!(of(8), Some(vec![0x48, 0x8b])); // mov r64
    }
}

