//! Turning what clang emitted into stencils, and the four rewrites that make
//! them smaller.
//!
//! Paper §5.3: "the stencil library builder then extracts their binary code and
//! the linker relocation records". [`extract`] is that sentence — one stencil
//! per exported `st_*` function, one hole per undefined symbol, and the
//! relocation records grouped by the symbol they name.
//!
//! Everything below [`extract`] is this port's own, and none of it is in the
//! paper. AArch64 has no 32-bit immediate field, so clang cannot be asked for
//! the operand shapes the emitter wants; the four folds recover them by
//! rewriting the instructions clang *did* emit:
//!
//! | fold | what it recovers |
//! |---|---|
//! | [`fold_addressing`] | a frame offset in a load or store's own `imm12` field, instead of an address computed into a register |
//! | [`fold_imm`] | a literal in an `add`/`sub`/`cmp`'s `imm12` field |
//! | [`fold_cond`] | a two-way branch as `b.cc` + `b` rather than `b.cc` + `b` + `b`, by making the conditional's `imm19` a hole |
//! | [`swap_arms`] | the twin of a folded conditional with the arms exchanged, so the emitter can pick whichever one falls through |
//!
//! This file runs **once, in `cli/build.rs`**, and is not compiled into the
//! toolchain: a fold is a decision about the library, and the library is built
//! when the toolchain is.

#![allow(
    clippy::arithmetic_side_effects,
    reason = "the two quantities this file computes are a position in a \
              stencil body and a field of an A64 instruction. A body is a few \
              dozen words long and already in memory, so an offset scaled \
              between bytes and words, or stepped to the next instruction, \
              stays inside a `usize` by three orders of magnitude. A \
              displacement is widened to `i64` from a field of at most 26 \
              bits before anything is added to it, so the sum of one and a \
              word index has room to spare and its sign is the branch's own. \
              Every result that becomes an index is offered to `.get` rather \
              than trusted"
)]

use super::elfobj as elf;
use super::machobj as macho;
use super::library::{Hole, HoleKind, Site, SiteKind, Stencil};
use super::library::{refuse_hole_name, refuse_stencil_name};

const NOP: u32 = 0xd503_201f;

// ---------------------------------------------------------------------------
// The container-neutral hand-over
// ---------------------------------------------------------------------------
//
// Two object formats carry the same stencils. Mach-O and ELF disagree about
// how a relocation record is spelled, how a symbol is mangled and how a
// function's extent is known, and about nothing else: below this boundary the
// bytes are arm64 either way and every rewrite in this file reads them as
// arm64. So each reader answers a [`RawFn`] and the whole of the rest of the
// file is shared, which is the property that makes the linux-arm64 stencils the
// *same* stencils as the macos-arm64 ones rather than a second set to keep in
// step.

/// One relocation record, after the container's own vocabulary has been
/// translated into the one `library.rs` speaks.
pub struct RawSite {
    /// Byte offset within the body.
    pub off: u32,
    pub hole: HoleKind,
    pub site: SiteKind,
    /// The hole's name as the emitter spells it: `JIT_A`, or a plain C
    /// identifier for a runtime symbol.
    pub name: String,
}

/// One `st_*` function, with its body already cut to size and its relocation
/// records already rebased onto it.
pub struct RawFn {
    pub name: String,
    pub code: Vec<u8>,
    /// Sorted by `off`, which every rewrite below relies on.
    pub sites: Vec<RawSite>,
}

/// The instruction at byte offset `o`, or `None` when the body ends first.
fn word(b: &[u8], o: usize) -> Option<u32> {
    let mut a = [0u8; 4];
    a.copy_from_slice(b.get(o..o + 4)?);
    Some(u32::from_le_bytes(a))
}

/// A stencil body as the instruction words the rewrites below work on.
///
/// The tail of a body that is not a whole number of words is dropped, which is
/// what `words` counting by division already meant.
fn words_of(code: &[u8]) -> Option<Vec<u32>> {
    (0..code.len() / 4).map(|i| word(code, i * 4)).collect()
}

/// One flag per word of the body, set where a hole has a site.
///
/// Those fields are the JIT's to fill, so no rewrite below may read a value out
/// of one or re-displace it. `None` when a site names a word the body does not
/// have, which is a stencil no fold can reason about.
fn reloc_words(s: &Stencil, words: usize) -> Option<Vec<bool>> {
    let mut v = vec![false; words];
    for h in &s.holes {
        for site in &h.sites {
            *v.get_mut(site.off as usize / 4)? = true;
        }
    }
    Some(v)
}

/// Turns one clang-compiled **Mach-O** object into stencils, one per exported
/// function whose name starts with `st_`.
pub fn extract(obj: &macho::Obj) -> Result<Vec<Stencil>, String> {
    if let Some((n, sz)) = obj.other_sections.first() {
        return Err(format!(
            "a stencil spilled {sz} bytes into {n}; every constant must be a hole"
        ));
    }
    let mut raws = Vec::new();
    for (name, start, end) in macho::functions(obj) {
        let sname = name.strip_prefix('_').unwrap_or(&name);
        if !sname.starts_with("st_") {
            continue;
        }
        let Some(body) = obj.text.get(start..end) else {
            return Err(format!("{sname}: {start:#x}..{end:#x} is outside __text"));
        };
        let mut code = body.to_vec();
        // Trailing alignment padding, which `.p2align` fills with nops. Mach-O
        // has no `st_size`, so a body's extent is the next symbol's start and
        // the padding is inside it; the ELF reader is handed the exact length
        // and has nothing to trim.
        while code.len() >= 4
            && word(&code, code.len() - 4).is_some_and(|w| w == NOP || w == 0)
        {
            code.truncate(code.len() - 4);
        }
        let mut sites: Vec<RawSite> = Vec::new();
        for r in obj.text_relocs.iter().filter(|r| {
            (r.addr as usize) >= start && ((r.addr as usize) < start + code.len())
        }) {
            let sym = obj
                .syms
                .get(r.symbolnum as usize)
                .ok_or_else(|| format!("{sname}: relocation names symbol {}", r.symbolnum))?;
            let (hole, site) = match r.kind {
                macho::ARM64_RELOC_BRANCH26 => (HoleKind::Branch, SiteKind::Branch26),
                macho::ARM64_RELOC_PAGE21 => (HoleKind::Imm32, SiteKind::Adrp),
                macho::ARM64_RELOC_PAGEOFF12 => (HoleKind::Imm32, SiteKind::AddLo12),
                macho::ARM64_RELOC_GOT_LOAD_PAGE21 => (HoleKind::Imm64, SiteKind::GotAdrp),
                macho::ARM64_RELOC_GOT_LOAD_PAGEOFF12 => (HoleKind::Imm64, SiteKind::GotLdr),
                other => {
                    return Err(format!("{sname}: unhandled arm64 relocation type {other}"))
                }
            };
            sites.push(RawSite {
                off: r.addr - start as u32,
                hole,
                site,
                name: macho_hole_name(&sym.name),
            });
        }
        sites.sort_by_key(|s| s.off);
        raws.push(RawFn { name: sname.to_string(), code, sites });
    }
    finish_arm64(raws)
}

/// The emitter's spelling of a hole, from a Mach-O symbol name.
///
/// **One** underscore is Mach-O's C mangling, and what is under it is the C
/// name. A hole is declared `_JIT_A` in the generated C, so its C name has a
/// second one and the emitter's hole names have neither; every other symbol
/// here is a plain C identifier and keeps whatever underscores it was written
/// with. Stripping the run turned `___divti3` — compiler-rt's, which a 128-bit
/// divide used to reach — into `divti3`, a symbol nothing defines.
///
/// ELF does not mangle at all, which is why this is the Mach-O reader's
/// business and not the shared finisher's.
fn macho_hole_name(sym: &str) -> String {
    let c = sym.strip_prefix('_').unwrap_or(sym);
    match c.strip_prefix("_JIT_") {
        Some(rest) => format!("JIT_{rest}"),
        None => c.to_string(),
    }
}

/// Turns one clang-compiled **ELF/aarch64** object into stencils.
///
/// The relocation vocabulary is one-to-one with Mach-O's — the AArch64 ELF
/// psABI splits `BRANCH26` into `JUMP26` and `CALL26` by instruction, and
/// otherwise names the same five fields — so the stencils this answers are the
/// same stencils [`extract`] answers for the same C. `sources.rs`'s
/// `linux-arm64` library is verified against the `macos-arm64` one on exactly
/// that claim.
pub fn extract_elf_arm64(obj: &elf::Obj) -> Result<Vec<Stencil>, String> {
    if obj.machine != elf::EM_AARCH64 {
        return Err(format!("expected an aarch64 ELF object, got machine {}", obj.machine));
    }
    if let Some(other) = obj.other_sections.first() {
        return Err(format!(
            "a stencil spilled {} bytes into {}; every constant must be a hole",
            other.data.len(),
            other.name
        ));
    }
    let mut raws = Vec::new();
    for (sname, start, end) in elf::functions(obj) {
        if !sname.starts_with("st_") {
            continue;
        }
        let Some(body) = obj.text.get(start..end) else {
            return Err(format!("{sname}: {start:#x}..{end:#x} is outside .text"));
        };
        let code = body.to_vec();
        let mut sites: Vec<RawSite> = Vec::new();
        for r in obj
            .text_relocs
            .iter()
            .filter(|r| (r.addr as usize) >= start && ((r.addr as usize) < end))
        {
            let sym = obj
                .syms
                .get(r.symbolnum as usize)
                .ok_or_else(|| format!("{sname}: relocation names symbol {}", r.symbolnum))?;
            let (hole, site) = match r.kind {
                elf::R_AARCH64_JUMP26 | elf::R_AARCH64_CALL26 => {
                    (HoleKind::Branch, SiteKind::Branch26)
                }
                elf::R_AARCH64_ADR_PREL_PG_HI21 => (HoleKind::Imm32, SiteKind::Adrp),
                elf::R_AARCH64_ADD_ABS_LO12_NC => (HoleKind::Imm32, SiteKind::AddLo12),
                elf::R_AARCH64_ADR_GOT_PAGE => (HoleKind::Imm64, SiteKind::GotAdrp),
                elf::R_AARCH64_LD64_GOT_LO12_NC => (HoleKind::Imm64, SiteKind::GotLdr),
                other => {
                    return Err(format!("{sname}: unhandled aarch64 ELF relocation type {other}"))
                }
            };
            // Every one of these kinds takes its value from the symbol alone.
            // A non-zero addend would displace a field this file is about to
            // rewrite into a `movz`/`movk` pair that has nowhere to carry it,
            // so it is refused rather than dropped.
            if r.addend != 0 {
                return Err(format!(
                    "{sname}+{:#x}: relocation against {} has addend {}, which a hole cannot carry",
                    r.addr - start as u32,
                    sym.name,
                    r.addend
                ));
            }
            sites.push(RawSite {
                off: r.addr - start as u32,
                hole,
                site,
                name: sym.name.strip_prefix("_JIT_").map_or_else(
                    || sym.name.clone(),
                    |rest| format!("JIT_{rest}"),
                ),
            });
        }
        sites.sort_by_key(|s| s.off);
        raws.push(RawFn { name: sname.clone(), code, sites });
    }
    finish_arm64(raws)
}

/// Everything below the container: the arm64 half of extraction.
///
/// The dead-clear trim, the shape checks, the `adrp`/low-12 pairing, the tail
/// branch and `strip_dead_clears` all read the body as A64 instructions, so
/// they are shared by both readers and neither format appears below this line.
fn finish_arm64(raws: Vec<RawFn>) -> Result<Vec<Stencil>, String> {
    let mut out = Vec::new();
    for raw in raws {
        let sname = raw.name;
        let mut code = raw.code;
        let mut sites = raw.sites;

        // A function the compiler carved out of a stencil is a function too,
        // and on Mach-O it is one in `__text` named `_st_<parent>.cold.<n>`,
        // which the `st_` filter above lets straight through. Refused before
        // anything reads its bytes: half a function is not a stencil, whatever
        // its body decodes to. `library::refuse_stencil_name` says why.
        if let Some(e) = refuse_stencil_name(&sname) {
            return Err(e);
        }

        // Clang zeroes the scratch registers it used for `adrp` pairs just
        // before a tail call. They are caller-saved, unused by the
        // continuation's prototype, and provably dead; dropping the trailing
        // run is worth ~10% of a small stencil's instruction count.
        let mut trimmed = strip_dead_movs(&mut code, &mut sites);
        while trimmed {
            trimmed = strip_dead_movs(&mut code, &mut sites);
        }

        let mut holes: Vec<Hole> = Vec::new();
        for RawSite { off, hole: hk, site: sk, name: hname } in &sites {
            let (off, hk, sk, hname) = (*off, *hk, *sk, hname.clone());
            // A name check before the shape checks, because a hole whose target
            // the compiler invented is wrong however well-formed the
            // instruction under it is. `st_decref_drop.cold.1` is a perfectly
            // good `b`, and the function it reaches is one no stencil library
            // carries. On ELF the outlined half gets its own section and the
            // two readers above refuse it there; Mach-O has no section prefixes
            // and this is the only place it can be caught, so it is written
            // once, in `library.rs`, and applied by every finisher.
            if let Some(e) = refuse_hole_name(&sname, &hname) {
                return Err(e);
            }
            // Shape checks, so that a clang that stopped emitting the expected
            // pair fails here rather than at run time in patched machine code.
            let Some(w) = word(&code, off as usize) else {
                return Err(format!("{sname}+{off:#x}: relocation is outside the body"));
            };
            match sk {
                SiteKind::Adrp | SiteKind::GotAdrp => {
                    if w & 0x9f00_0000 != 0x9000_0000 {
                        return Err(format!("{sname}+{off:#x}: PAGE21 site is not adrp"));
                    }
                }
                SiteKind::AddLo12 => {
                    if w & 0xffc0_0000 != 0x9100_0000 {
                        return Err(format!("{sname}+{off:#x}: PAGEOFF12 site is not add"));
                    }
                    if (w & 0x1f) != ((w >> 5) & 0x1f) {
                        return Err(format!("{sname}+{off:#x}: add is not Xd,Xd,#imm"));
                    }
                }
                SiteKind::GotLdr => {
                    if w & 0xffc0_0000 != 0xf940_0000 {
                        return Err(format!("{sname}+{off:#x}: GOT PAGEOFF12 site is not ldr"));
                    }
                }
                SiteKind::Branch26 => {
                    if w & 0x7c00_0000 != 0x1400_0000 {
                        return Err(format!("{sname}+{off:#x}: BRANCH26 site is not b/bl"));
                    }
                }
                // No relocation type above maps to `Cond19`: the cond fold is
                // what makes one, and it runs long after this. The three
                // x86-64 kinds cannot reach here at all — `x86.rs` is the
                // other finisher — and saying so is cheaper than a wildcard
                // that would swallow a fourth arm added later.
                SiteKind::Cond19 => {
                    return Err(format!("{sname}+{off:#x}: a relocation named a Cond19 site"));
                }
                SiteKind::LeaPc32
                | SiteKind::GotPc32
                | SiteKind::Rel32
                | SiteKind::CondRel32 => {
                    return Err(format!(
                        "{sname}+{off:#x}: an x86-64 site reached the arm64 extractor"
                    ));
                }
            }
            match holes.iter_mut().find(|h| h.name == hname) {
                Some(h) => {
                    if h.kind != hk {
                        return Err(format!("{sname}: hole {hname} has two kinds"));
                    }
                    h.sites.push(Site { off, kind: sk });
                }
                None => holes.push(Hole {
                    name: hname,
                    kind: hk,
                    sites: vec![Site { off, kind: sk }],
                    pairs: Vec::new(),
                    branches: Vec::new(),
                    conds: Vec::new(),
                    lo12: Vec::new(),
                }),
            }
        }
        // Match each low-12 site to the `adrp` that produced its register. The
        // two are usually adjacent, but nothing in the ABI says they must be,
        // so the pairing is by destination register rather than by position.
        for h in &mut holes {
            let mut pending: Vec<(u32, u32)> = Vec::new(); // (offset, Rd)
            for s in &h.sites {
                let Some(w) = word(&code, s.off as usize) else {
                    return Err(format!("{sname}: site at {:#x} is outside the body", s.off));
                };
                match s.kind {
                    SiteKind::Adrp | SiteKind::GotAdrp => pending.push((s.off, w & 0x1f)),
                    SiteKind::AddLo12 | SiteKind::GotLdr => {
                        let rn = (w >> 5) & 0x1f;
                        let i = pending
                            .iter()
                            .rposition(|(o, rd)| *rd == rn && *o < s.off)
                            .ok_or_else(|| {
                                format!("{sname}: low-12 site at {:#x} has no adrp", s.off)
                            })?;
                        // `rposition` answers an index into `pending`, so the
                        // removal takes out the entry it just found.
                        let (adrp, _) = pending.remove(i);
                        h.pairs.push((adrp, s.off));
                    }
                    SiteKind::Branch26 => h.branches.push(s.off),
                    SiteKind::Cond19 => h.conds.push(s.off),
                    SiteKind::LeaPc32
                    | SiteKind::GotPc32
                    | SiteKind::Rel32
                    | SiteKind::CondRel32 => {
                        return Err(format!(
                            "{sname}: an x86-64 site reached the arm64 extractor"
                        ));
                    }
                }
            }
            if !pending.is_empty() {
                return Err(format!("{sname}: hole {} has an unpaired adrp", h.name));
            }
        }
        // The continuation branch, when it is the last instruction.
        let tail = if code.len() >= 4 {
            let last = (code.len() - 4) as u32;
            holes.iter().position(|h| {
                h.kind == HoleKind::Branch
                    && h.sites.len() == 1
                    && h.sites.first().is_some_and(|s| s.off == last)
                    && word(&code, last as usize)
                        .is_some_and(|w| w & 0xfc00_0000 == 0x1400_0000)
            })
        } else {
            None
        };
        let st = Stencil { name: sname.clone(), code, holes, tail, ..Stencil::unspilled() };
        out.push(strip_dead_clears(&st).unwrap_or(st));
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// The addressing-mode fold
// ---------------------------------------------------------------------------
//
// This is the one optimisation the port *has* to invent, because the paper gets
// it from x86-64 for free.
//
// On x86-64 a frame-slot hole is the 32-bit displacement of the load that uses
// it: `mov rax, [rbp + disp32]` is one instruction and the hole is a field in
// it. On AArch64 there is no such field to relocate, so clang materialises the
// offset into a register first — `adrp`/`add`/`ldr Xt,[Xn,Xm]` — and a
// two-operand arithmetic stencil comes out at **eleven** instructions where
// five would do.
//
// The unsigned-offset addressing form `ldr Xt,[Xn,#imm12]` *does* have a field,
// scaled by the access size and reaching 32 760 bytes for a 64-bit load, which
// is far more frame than any function here has. So the library builder rewrites
// the pair away: it deletes the `adrp` and the `add`, rewrites the load or
// store into unsigned-offset form, and records the hole as the load's `imm12`.
// Every PC-relative branch left in the body is re-displaced, because the body
// just got shorter.
//
// The result is a second copy of the stencil under `<key>+fold`, chosen at
// emission time when the offset is a multiple of the access size and fits the
// field. It is a stencil *variant* in exactly the paper's sense — an operand
// kind, "stack slot addressed by displacement" against "stack slot addressed by
// a computed index" — and the sweep measures it as one.

fn is_ls_reg_offset(w: u32) -> bool {
    w & 0x3b20_0c00 == 0x3820_0800
}
/// Any A64 control transfer: `b`/`bl`, `b.cond`, `cbz`/`cbnz`, `tbz`/`tbnz`,
/// and the register-indirect family `br`/`blr`/`ret`.
fn is_branch(w: u32) -> bool {
    w & 0x7c00_0000 == 0x1400_0000
        || w & 0xff00_0010 == 0x5400_0000
        || w & 0x7e00_0000 == 0x3400_0000
        || w & 0x7e00_0000 == 0x3600_0000
        || w & 0xfe1f_fc00 == 0xd61f_0000
}
/// Whether a register-offset access is a **scalar load into `rd`**, which is
/// the only form in this encoding that overwrites a general-purpose register.
///
/// `V` is bit 26 and `opc` is bits 23:22: a SIMD access names a vector register
/// in `Rt` and never touches an X register, and a scalar store — `opc == 00` —
/// reads `Rt` rather than writing it. Everything else here is a load.
fn writes_gpr(w: u32, rd: u32) -> bool {
    w & (1 << 26) == 0 && (w >> 22) & 3 != 0 && (w & 0x1f) == rd
}
/// The access size of a load or store, which is what its unsigned-offset field
/// is scaled by.
///
/// For the scalar forms that is `size`, bits 31:30. For the SIMD forms — and
/// clang reaches for `ldr q0` the moment a copy is sixteen bytes wide — the
/// size is `size` *extended by the top bit of `opc`*, so a 128-bit access has
/// `size == 00` and scale 16. Reading only `size` there gave scale 1 and a
/// sixteen-fold wrong offset, which is the one miscompile this port produced.
fn ls_scale(w: u32) -> u32 {
    let mut log = (w >> 30) & 3;
    if w & (1 << 26) != 0 {
        log |= ((w >> 23) & 1) << 2;
    }
    1 << log
}
fn to_unsigned_offset(w: u32, imm12: u32) -> u32 {
    (w & 0xffc0_0000) | (1 << 24) | (imm12 << 10) | (((w >> 5) & 0x1f) << 5) | (w & 0x1f)
}

/// Deletes instructions from a stencil body, re-displacing every PC-relative
/// branch that survives and moving every hole site to its new offset.
///
/// Relocation sites are never re-displaced: their fields are the JIT's to fill.
fn remove(s: &Stencil, dead: &[bool], code: &mut [u32], name: String) -> Option<Stencil> {
    let words = code.len();
    // One flag per word is what every caller builds and what the three tables
    // below are sized to; the whole rewrite indexes them interchangeably.
    if dead.len() != words {
        return None;
    }
    let reloc = reloc_words(s, words)?;
    let mut newpos = vec![usize::MAX; words];
    let mut n = 0;
    for (slot, is_dead) in newpos.iter_mut().zip(dead) {
        if !*is_dead {
            *slot = n;
            n += 1;
        }
    }
    for i in 0..words {
        let (Some(&is_dead), Some(&is_reloc), Some(&w)) =
            (dead.get(i), reloc.get(i), code.get(i))
        else {
            continue;
        };
        if is_dead || is_reloc {
            continue;
        }
        let (shift, bits) = if w & 0x7c00_0000 == 0x1400_0000 {
            (0u32, 26u32)
        } else if w & 0xff00_0010 == 0x5400_0000 || w & 0x7e00_0000 == 0x3400_0000 {
            (5, 19)
        } else if w & 0x7e00_0000 == 0x3600_0000 {
            (5, 14)
        } else {
            if w & 0x1f00_0000 == 0x1000_0000 || w & 0x3b00_0000 == 0x1800_0000 {
                // `adr` or a literal load: PC-relative in a way this rewrite
                // does not handle. Refuse rather than guess.
                return None;
            }
            continue;
        };
        let mask = (1i64 << bits) - 1;
        let raw = ((w >> shift) as i64) & mask;
        let d = if raw & (1 << (bits - 1)) != 0 { raw - (1 << bits) } else { raw };
        let target = i as i64 + d;
        if target < 0 {
            return None;
        }
        // A branch out of the body, or into a word that is being deleted, is
        // one this rewrite has nowhere to point: refuse the whole stencil.
        let (Some(&to), Some(&from)) = (newpos.get(target as usize), newpos.get(i)) else {
            return None;
        };
        if to == usize::MAX {
            return None;
        }
        let nd = to as i64 - from as i64;
        let slot = code.get_mut(i)?;
        *slot = (w & !((mask as u32) << shift)) | (((nd & mask) as u32) << shift);
    }
    let mut out =
        Stencil { name, code: Vec::with_capacity(n * 4), holes: s.holes.clone(), tail: None,
                  ..Stencil::unspilled() };
    // Where a recorded offset's word ended up. `usize::MAX` marks a word being
    // deleted, and no offset that reaches here names one: each fold clears the
    // hole records that pointed into the words it killed before calling in.
    let moved = |off: u32| newpos.get(off as usize / 4).map(|w| *w as u32 * 4);
    for h in &mut out.holes {
        for p in h.pairs.iter_mut() {
            p.0 = moved(p.0)?;
            p.1 = moved(p.1)?;
        }
        for b in h.branches.iter_mut() {
            *b = moved(*b)?;
        }
        for c in h.conds.iter_mut() {
            *c = moved(*c)?;
        }
        for l in h.lo12.iter_mut() {
            l.0 = moved(l.0)?;
        }
        // A site outside the body is not dead; the `moved` below is what
        // refuses it, so that one answer covers both loops.
        h.sites.retain(|st| !dead.get(st.off as usize / 4).copied().unwrap_or(false));
        for st in h.sites.iter_mut() {
            st.off = moved(st.off)?;
        }
    }
    for (w, is_dead) in code.iter().zip(dead) {
        if !*is_dead {
            out.code.extend_from_slice(&w.to_le_bytes());
        }
    }
    if out.code.len() >= 4 {
        let last = (out.code.len() - 4) as u32;
        out.tail = out.holes.iter().position(|h| {
            h.kind == HoleKind::Branch
                && h.branches.len() == 1
                && h.branches.first() == Some(&last)
        });
    }
    Some(out)
}

/// Clang zeroes the scratch registers it used for `adrp` pairs immediately
/// before every control transfer out of the stencil. They are caller-saved,
/// they are not in the continuation's prototype, and LLVM emitted them
/// precisely because it had already proved them dead. Dropping them is worth
/// three instructions in the inner loop of the prime kernel alone.
///
/// # The argument registers, on a stencil that ends in `_JIT_CONT0`
///
/// A stencil with the **zero-register** prototype passes its continuation the
/// frame pointer and nothing else, and clang clears the argument registers it
/// used — `x1`–`x7` — before the `b`, for the same reason it clears `x8`–`x17`:
/// they are not parameters of the callee and it has proved them dead. On a
/// runtime call that was **five instructions** after the `bl`, on every call,
/// and they are strippable for a reason this backend states rather than
/// inherits: `jit.rs::is_barrier` already treats every zero-register stencil as
/// clobbering the whole CPS register file, so nothing downstream may read
/// `x1`–`x7` across one. The zeroes are dead by the convention, not merely by
/// LLVM's analysis of one function.
///
/// The narrower rule for a stencil that continues to `_JIT_CONT` is kept, and
/// it has to be: there `x1`–`x3` *are* the continuation's `r0`–`r2`, so a
/// `movz x1, #0` can be a value the next stencil reads. Which prototype a
/// stencil has is read off the name of its tail hole, which is the one place
/// the two are distinguishable after extraction.
///
/// `x0` is never a candidate at either prototype: it is the frame pointer.
pub fn strip_dead_clears(s: &Stencil) -> Option<Stencil> {
    let words = s.code.len() / 4;
    let mut code = words_of(&s.code)?;
    let reloc = reloc_words(s, words)?;
    let zero_reg = s
        .tail
        .and_then(|t| s.holes.get(t))
        .is_some_and(|h| h.name == "JIT_CONT0");
    let lo = if zero_reg { 1 } else { 8 };
    let is_clear = |w: u32| w & 0xffff_ffe0 == 0xd280_0000 && (lo..=17).contains(&(w & 0x1f));
    let mut dead = vec![false; words];
    for i in 0..words {
        let (Some(&is_reloc), Some(&w)) = (reloc.get(i), code.get(i)) else { continue };
        if is_reloc || !is_clear(w) {
            continue;
        }
        // Only where the run it belongs to ends in a control transfer, which is
        // the shape LLVM emits and the only one that is provably dead.
        let mut j = i;
        // `reloc` is one flag per word of `code`, so the first test is what
        // bounds both: past the end there is no next word to run on to.
        while code.get(j + 1).copied().is_some_and(is_clear)
            && !reloc.get(j + 1).copied().unwrap_or(true)
        {
            j += 1;
        }
        let after = code.get(j + 1).copied().unwrap_or(0);
        let transfers = after & 0x7c00_0000 == 0x1400_0000 || after == 0xd65f_03c0;
        if transfers {
            for d in dead.iter_mut().take(j + 1).skip(i) {
                *d = true;
            }
        }
    }
    if !dead.iter().any(|d| *d) {
        return None;
    }
    remove(s, &dead, &mut code, s.name.clone())
}

/// Answers the folded twin of a stencil, when there is one to have.
pub fn fold_addressing(s: &Stencil) -> Option<Stencil> {
    let words = s.code.len() / 4;
    let reloc = reloc_words(s, words)?;
    let mut code = words_of(&s.code)?;
    let mut dead = vec![false; words];
    let mut folded: Vec<(usize, usize, u32, u32)> = Vec::new(); // hole, pair, ls index, scale
    for (hi, h) in s.holes.iter().enumerate() {
        if h.kind != HoleKind::Imm32 {
            continue;
        }
        for (pi, (a, b)) in h.pairs.iter().enumerate() {
            let (ai, bi) = (*a as usize / 4, *b as usize / 4);
            // Both offsets were recorded from this body's own relocations, so
            // both name words of it. A stencil whose records say otherwise is
            // one this fold declines rather than half-applies.
            let (Some(&adrp), Some(&a_dead), Some(&b_dead)) =
                (code.get(ai), dead.get(ai), dead.get(bi))
            else {
                return None;
            };
            let rd = adrp & 0x1f;
            // The one load or store that uses the materialised offset. A second
            // use, or a redefinition first, means this pair is left alone.
            let mut found: Option<usize> = None;
            let mut branched = false;
            for (i, &w) in code.iter().enumerate().skip(bi + 1) {
                // `reloc` is one flag per word of `code`, so `i` names one.
                let is_reloc = reloc.get(i).copied().unwrap_or(false);
                if is_reloc && (w & 0x9f00_0000 == 0x9000_0000) && (w & 0x1f) == rd {
                    break; // Rd redefined by the next hole's adrp.
                }
                branched |= is_branch(w);
                if is_ls_reg_offset(w) && ((w >> 16) & 0x1f) == rd {
                    if found.is_some() {
                        found = None;
                        break;
                    }
                    found = Some(i);
                    // A load into the very register it indexed by — `ldr x10,
                    // [x0, x10]`, which is what `AT(t, _JIT_X)` compiles to —
                    // ends the offset's life here: every later mention of the
                    // register is the value that was loaded, not the offset.
                    // So the scan stops rather than counting the loaded index
                    // as a second use and declining, which is why an `eload`'s
                    // index hole never folded. Straight-line only: past a
                    // branch the load may not have run, and the offset may
                    // still be live on the other arm.
                    if !branched && writes_gpr(w, rd) {
                        break;
                    }
                }
            }
            let Some(li) = found else { continue };
            if a_dead || b_dead {
                continue;
            }
            if let Some(slot) = dead.get_mut(ai) {
                *slot = true;
            }
            if let Some(slot) = dead.get_mut(bi) {
                *slot = true;
            }
            // `li` came out of enumerating `code`, so it names a word.
            let slot = code.get_mut(li)?;
            let scale = ls_scale(*slot);
            *slot = to_unsigned_offset(*slot, 0);
            folded.push((hi, pi, li as u32, scale));
        }
    }
    if folded.is_empty() {
        return None;
    }
    // Rewrite the holes so each folded pair becomes an `imm12` site, then let
    // `remove` delete the dead words and move everything that survives.
    let mut s2 = Stencil {
        name: s.name.clone(),
        code: s.code.clone(),
        holes: s.holes.clone(),
        tail: s.tail,
        ..Stencil::unspilled()
    };
    for (hi, h) in s.holes.iter().enumerate() {
        let mut pairs = Vec::new();
        let mut lo12 = h.lo12.clone();
        for (pi, p) in h.pairs.iter().enumerate() {
            match folded.iter().find(|(x, y, _, _)| *x == hi && *y == pi) {
                Some((_, _, li, scale)) => lo12.push((*li * 4, *scale)),
                None => pairs.push(*p),
            }
        }
        // `hi` enumerates `s.holes`, and `s2.holes` is a clone of it.
        let hole = s2.holes.get_mut(hi)?;
        hole.pairs = pairs;
        hole.lo12 = lo12;
    }
    remove(&s2, &dead, &mut code, format!("{}+fold", s.name))
}

/// Removes one trailing `movz Xn, #0` (n >= 8) sitting immediately before the
/// body's last instruction. Answers whether it removed one.
fn strip_dead_movs(code: &mut Vec<u8>, sites: &mut [RawSite]) -> bool {
    if code.len() < 8 {
        return false;
    }
    let cand = code.len() - 8;
    let Some(w) = word(code, cand) else { return false };
    // movz Xd, #0, lsl 0
    let is_zero_mov = w & 0xffff_ffe0 == 0xd280_0000 && (w & 0x1f) >= 8;
    if !is_zero_mov {
        return false;
    }
    if sites.iter().any(|s| s.off as usize == cand) {
        return false;
    }
    code.drain(cand..cand + 4);
    for s in sites.iter_mut() {
        if (s.off as usize) > cand {
            s.off -= 4;
        }
    }
    true
}


// ---------------------------------------------------------------------------
// The conditional-branch fold
// ---------------------------------------------------------------------------
//
// The second thing this port has to invent, and for the same reason as the
// first: clang cannot spell the instruction the JIT wants.
//
// A two-target stencil — `brcmp`, `br`, `tagbr` — asks C for
// `if (p) TAIL_T; else TAIL_F;`, and clang has no choice but to compile it as
//
// ```text
//   b.cc  L        ; the test
//   b     _JIT_X   ; one arm, a relocation
// L: b     _JIT_Y   ; the other arm, a relocation
// ```
//
// three instructions for a two-way branch, because a relocated `b` is the only
// form the Mach-O `BRANCH26` record can name. The machine has `b.cc <label>`,
// whose 19-bit displacement reaches ±1 MB — far more than any function this JIT
// emits. So the library builder inverts the condition, folds one arm *into* the
// conditional branch, and drops the now-unreachable `b`:
//
// ```text
//   b.!cc _JIT_X
//   b     _JIT_Y
// ```
//
// Two instructions, and when `_JIT_Y` is the stencil laid out next the paper's
// fallthrough elision takes it to **one**. Every compare-and-branch, every
// `match` arm and every loop back edge in the corpus pays this.
fn cond_invert(w: u32) -> Option<u32> {
    if w & 0xff00_0010 == 0x5400_0000 {
        // b.cond: the condition is bits 3:0, and inverting it is bit 0.
        Some(w ^ 1)
    } else if w & 0x7e00_0000 == 0x3400_0000 {
        // cbz / cbnz: bit 24 selects.
        Some(w ^ (1 << 24))
    } else {
        None
    }
}

fn cond_target(w: u32, at: usize) -> Option<usize> {
    if w & 0xff00_0010 == 0x5400_0000 || w & 0x7e00_0000 == 0x3400_0000 {
        let raw = ((w >> 5) & 0x7ffff) as i64;
        let d = if raw & (1 << 18) != 0 { raw - (1 << 19) } else { raw };
        Some((at as i64 + d) as usize)
    } else {
        None
    }
}

/// Answers the cond-folded form of a stencil, when it has the shape above.
pub fn fold_cond(s: &Stencil) -> Option<Stencil> {
    fold_cond_adjacent(s).or_else(|| fold_cond_last(s))
}

fn fold_cond_adjacent(s: &Stencil) -> Option<Stencil> {
    let words = s.code.len() / 4;
    if words < 3 {
        return None;
    }
    let code = words_of(&s.code)?;
    let (k, x, y) = (words - 3, words - 2, words - 1);
    // The last two instructions must be the two arms, each one relocation.
    let hx = s.holes.iter().position(|h| {
        h.kind == HoleKind::Branch
            && h.branches.len() == 1
            && h.branches.first().is_some_and(|b| *b as usize == x * 4)
    })?;
    let hy = s.holes.iter().position(|h| {
        h.kind == HoleKind::Branch
            && h.branches.len() == 1
            && h.branches.first().is_some_and(|b| *b as usize == y * 4)
    })?;
    if hx == hy {
        return None;
    }
    // `words` is at least three, so all three of these name words.
    let (Some(&wk), Some(&wx), Some(&wy)) = (code.get(k), code.get(x), code.get(y)) else {
        return None;
    };
    if wx & 0xfc00_0000 != 0x1400_0000 || wy & 0xfc00_0000 != 0x1400_0000 {
        return None;
    }
    // The test must be a conditional branch over the first arm, and must not
    // itself be a relocation site.
    if s.holes.iter().any(|h| h.sites.iter().any(|st| st.off as usize == k * 4)) {
        return None;
    }
    let inv = cond_invert(wk)?;
    if cond_target(wk, k)? != y {
        return None;
    }
    let mut s2 = Stencil {
        name: s.name.clone(),
        code: s.code.clone(),
        holes: s.holes.clone(),
        tail: s.tail,
        ..Stencil::unspilled()
    };
    let mut code = code;
    *code.get_mut(k)? = inv & 0xffff_001f; // clear imm19; the JIT fills it
    // `_JIT_X` moves from the `b` at x into the conditional at k. `hx` came
    // from `position` over `s.holes`, and `s2.holes` is a clone of it.
    let hole = s2.holes.get_mut(hx)?;
    hole.branches.clear();
    hole.conds.push((k * 4) as u32);
    hole.sites.retain(|st| st.off as usize != x * 4);
    hole.sites.push(Site { off: (k * 4) as u32, kind: SiteKind::Cond19 });
    let mut dead = vec![false; words];
    *dead.get_mut(x)? = true;
    remove(&s2, &dead, &mut code, s.name.clone())
}

/// The other shape the same fold takes, for the stencils where the two arms are
/// not adjacent.
///
/// LLVM's `propagateEquality` rewrites `r0` into the literal on the true edge of
/// `r0 == _JIT_K`, so the equality stencils come out as
///
/// ```text
///   cmp x1, x8 ; b.ne L ; adrp x1, K ; ldr x1, [x1] ; b _JIT_T ; L: b _JIT_F
/// ```
///
/// — the arm it rematerialises into is no longer next to the other one, and the
/// first form of the fold cannot delete it. This one deletes the **last**
/// instruction instead and puts its arm on the conditional branch, which is
/// sound whenever nothing falls into or branches at that last instruction.
fn fold_cond_last(s: &Stencil) -> Option<Stencil> {
    let words = s.code.len() / 4;
    if words < 3 {
        return None;
    }
    let code = words_of(&s.code)?;
    let y = words - 1;
    let hy = s.holes.iter().position(|h| {
        h.kind == HoleKind::Branch
            && h.branches.len() == 1
            && h.branches.first().is_some_and(|b| *b as usize == y * 4)
    })?;
    // Nothing may fall into the last instruction, and nothing else may branch
    // to it. `words` is at least three, so `y - 1` names a word.
    let &prev = code.get(y - 1)?;
    if prev & 0xfc00_0000 != 0x1400_0000 {
        return None;
    }
    let mut c = None;
    for (i, &w) in code.iter().enumerate().take(y) {
        if let Some(t) = cond_target(w, i) {
            if t == y {
                if c.is_some() {
                    return None;
                }
                c = Some(i);
            } else if t >= y {
                return None;
            }
        }
        if w & 0x7c00_0000 == 0x1400_0000 && !s.holes.iter().any(|h| {
            h.sites.iter().any(|st| st.off as usize == i * 4)
        }) {
            return None; // an internal unconditional branch; too clever to fold
        }
    }
    let c = c?;
    if s.holes.iter().any(|h| h.sites.iter().any(|st| st.off as usize == c * 4)) {
        return None;
    }
    let mut s2 = Stencil {
        name: s.name.clone(),
        code: s.code.clone(),
        holes: s.holes.clone(),
        tail: s.tail,
        ..Stencil::unspilled()
    };
    let mut code = code;
    // `c` came out of enumerating `code`, and `hy` from `position` over
    // `s.holes`, which `s2.holes` is a clone of.
    *code.get_mut(c)? &= 0xffff_001f;
    let hole = s2.holes.get_mut(hy)?;
    hole.branches.clear();
    hole.sites.retain(|st| st.off as usize != y * 4);
    hole.conds.push((c * 4) as u32);
    hole.sites.push(Site { off: (c * 4) as u32, kind: SiteKind::Cond19 });
    let mut dead = vec![false; words];
    *dead.get_mut(y)? = true;
    remove(&s2, &dead, &mut code, s.name.clone())
}

/// The same stencil with its two arms exchanged: the conditional branch's
/// condition inverted, and the arm that was on it moved onto the trailing `b`.
///
/// This exists because **which arm clang puts last is not the emitter's to
/// choose**, and it is the arm the fallthrough elision needs. LLVM canonicalises
/// `a <= b` into `!(a > b)` with the successors swapped, so asking for the
/// negated comparison gets the *same* layout back and buys nothing — measured,
/// and it is why `Level::Br` is a rewrite rather than a second family of C.
/// Once `fold_cond` has run the two arms are one conditional branch and one
/// unconditional one, and exchanging them is four bytes and a hole record.
pub fn swap_arms(s: &Stencil) -> Option<Stencil> {
    let words = s.code.len() / 4;
    if words < 2 {
        return None;
    }
    let (k, y) = (words - 2, words - 1);
    let hx = s.holes.iter().position(|h| {
        h.kind == HoleKind::Branch
            && h.branches.is_empty()
            && h.conds.len() == 1
            && h.conds.first().is_some_and(|c| *c as usize == k * 4)
    })?;
    let hy = s.holes.iter().position(|h| {
        h.kind == HoleKind::Branch
            && h.branches.len() == 1
            && h.branches.first().is_some_and(|b| *b as usize == y * 4)
    })?;
    if hx == hy {
        return None;
    }
    let inv = cond_invert(word(&s.code, k * 4)?)?;
    let mut out = s.clone();
    out.name = format!("{}+swap", s.name);
    // `k` is two words back from the end of a body of at least two, and both
    // hole indices came from `position` over the holes `out` was cloned from.
    out.code.get_mut(k * 4..k * 4 + 4)?.copy_from_slice(&inv.to_le_bytes());
    let hole = out.holes.get_mut(hx)?;
    hole.conds.clear();
    hole.sites.retain(|st| st.off as usize != k * 4);
    hole.branches.push((y * 4) as u32);
    hole.sites.push(Site { off: (y * 4) as u32, kind: SiteKind::Branch26 });
    let hole = out.holes.get_mut(hy)?;
    hole.branches.clear();
    hole.sites.retain(|st| st.off as usize != y * 4);
    hole.conds.push((k * 4) as u32);
    hole.sites.push(Site { off: (k * 4) as u32, kind: SiteKind::Cond19 });
    out.tail = Some(hx);
    Some(out)
}

// ---------------------------------------------------------------------------
// The immediate fold
// ---------------------------------------------------------------------------
//
// The addressing fold's twin, for [`crate::gen::Loc::Imm`] rather than
// `Loc::Frame`, and it exists for the same ISA reason. A literal hole is a
// symbol, so clang has to materialise it into a register — two instructions,
// `movz`/`movk` after the patcher has rewritten them — and then use the
// register:
//
// ```text
//   mov  x9, #0 ; movk x9, #0, lsl 16 ; add x8, x8, x9
//   mov  x8, #0 ; movk x8, #0, lsl 16 ; cmp x1, x8
// ```
//
// Three instructions for `add x8, x8, #2` and `cmp x1, #0`. AArch64's
// add/sub/cmp immediate forms have a 12-bit unsigned field, which holds every
// small constant this IR is made of, so the library builder deletes the pair
// and makes the hole that field. The emitter picks the folded twin when the
// literal is below 4096 and the generic one otherwise, which is the same
// mechanism the addressing fold uses and the same one the paper's `Loc::Imm`
// variants are.

/// Whether an instruction can possibly read `r`. Conservative: an encoding this
/// does not recognise answers `true`, so a fold is refused rather than guessed.
/// Every A64 register operand lives in one of four fixed 5-bit fields, so
/// checking those four is sound for an unknown encoding.
fn may_read(w: u32, r: u32) -> bool {
    // `b`: the whole low 26 bits are a displacement, and nothing is read.
    if w & 0xfc00_0000 == 0x1400_0000 {
        return false;
    }
    // `bl`: a call, so the argument registers are read by whatever it reaches.
    if w & 0xfc00_0000 == 0x9400_0000 {
        return r <= 7;
    }
    // b.cond: bits 23:5 displacement, 3:0 condition.
    if w & 0xff00_0010 == 0x5400_0000 {
        return false;
    }
    // nop and the rest of the hint space.
    if w & 0xffff_f01f == 0xd503_201f {
        return false;
    }
    let f = [w & 0x1f, (w >> 5) & 0x1f, (w >> 10) & 0x1f, (w >> 16) & 0x1f];
    f.contains(&r)
}

/// Whether an instruction certainly writes `r` and does not read it, so that
/// `r` is dead from there on. Only the encodings a stencil actually contains;
/// everything else answers `false` and the scan keeps going.
fn kills(w: u32, r: u32) -> bool {
    let rd = w & 0x1f;
    let rn = (w >> 5) & 0x1f;
    if rd != r {
        return false;
    }
    // adrp / adr.
    if w & 0x1f00_0000 == 0x1000_0000 {
        return true;
    }
    // movz / movn, but not movk, which reads its destination.
    if w & 0x1f80_0000 == 0x1280_0000 && (w >> 29) & 3 != 3 {
        return true;
    }
    // A load: unsigned-offset or register-offset, `L` set.
    let unsigned = w & 0x3b00_0000 == 0x3900_0000;
    let regoff = w & 0x3b20_0c00 == 0x3820_0800;
    if (unsigned || regoff) && w & (1 << 22) != 0 {
        let rm = (w >> 16) & 0x1f;
        return rn != r && (!regoff || rm != r);
    }
    false
}

/// The immediate form of an add/sub/cmp whose second source is `r`, with the
/// field left at zero for the patcher. `None` when there is no such form.
fn to_add_sub_imm(w: u32, r: u32) -> Option<u32> {
    // Add/sub (shifted register), shift == 0 and amount == 0.
    if w & 0x1f20_0000 != 0x0b00_0000 {
        return None;
    }
    if w & 0x00c0_fc00 != 0 {
        return None; // a real shift; the immediate form has none
    }
    let (rd, rn, rm) = (w & 0x1f, (w >> 5) & 0x1f, (w >> 16) & 0x1f);
    let sf_op_s = w & 0xe000_0000; // sf, op (sub), S (flags)
    let commutes = w & 0x4000_0000 == 0; // add/adds, not sub/subs
    let rn = if rm == r {
        rn
    } else if rn == r && commutes {
        rm
    } else {
        return None;
    };
    if rn == r {
        return None; // both operands are the hole
    }
    Some(sf_op_s | 0x1100_0000 | (rn << 5) | rd)
}

/// Whether `r` is dead from `from` to the end of the body: no instruction reads
/// it before one that provably overwrites it. A branch on the way makes the
/// "overwritten" half unsound — the kill might be on one path only — so after
/// one the scan only ever refuses.
fn dead_after(code: &[u32], dead: &[bool], from: usize, r: u32) -> bool {
    let mut saw_branch = false;
    // `dead` is one flag per word of `code`, so the zip is the whole of both.
    for (&w, &is_dead) in code.iter().zip(dead).skip(from) {
        if is_dead {
            continue;
        }
        // A `bl` is not a join: control comes back to the next instruction, so
        // an overwrite after one is still an overwrite on every path.
        let is_branch = w & 0xfc00_0000 == 0x1400_0000
            || w & 0xfe00_0000 == 0x5400_0000
            || w & 0x7e00_0000 == 0x3400_0000
            || w & 0x7e00_0000 == 0x3600_0000;
        if !saw_branch && kills(w, r) {
            return true;
        }
        if may_read(w, r) {
            return false;
        }
        saw_branch |= is_branch;
    }
    true
}

/// Answers the immediate-folded form of a stencil, when it has one.
pub fn fold_imm(s: &Stencil) -> Option<Stencil> {
    let words = s.code.len() / 4;
    let reloc = reloc_words(s, words)?;
    let mut code = words_of(&s.code)?;
    let mut dead = vec![false; words];
    let mut folded: Vec<(usize, usize, u32)> = Vec::new(); // hole, pair, consumer index
    for (hi, h) in s.holes.iter().enumerate() {
        if h.kind == HoleKind::Branch {
            continue;
        }
        for (pi, (a, b)) in h.pairs.iter().enumerate() {
            let (ai, bi) = (*a as usize / 4, *b as usize / 4);
            // Both offsets were recorded from this body's own relocations, so
            // both name words of it; a stencil whose records say otherwise is
            // one this fold declines rather than half-applies.
            let (Some(&a_dead), Some(&b_dead), Some(&adrp)) =
                (dead.get(ai), dead.get(bi), code.get(ai))
            else {
                return None;
            };
            if a_dead || b_dead {
                continue;
            }
            let rd = adrp & 0x1f;
            // The first instruction after the pair that can read the
            // materialised register has to be the one that folds it, and
            // nothing after that may read it again. `dead` and `reloc` are one
            // flag per word of `code`, so the range bounds all three.
            let Some(ci) = (bi + 1..words).find(|i| {
                !dead.get(*i).copied().unwrap_or(false)
                    && code.get(*i).copied().is_some_and(|w| may_read(w, rd))
            }) else {
                continue;
            };
            let (Some(&is_reloc), Some(&consumer)) = (reloc.get(ci), code.get(ci)) else {
                return None;
            };
            if is_reloc {
                continue;
            }
            let Some(imm) = to_add_sub_imm(consumer, rd) else { continue };
            if (consumer & 0x1f) != rd && !dead_after(&code, &dead, ci + 1, rd) {
                continue;
            }
            if let Some(slot) = dead.get_mut(ai) {
                *slot = true;
            }
            if let Some(slot) = dead.get_mut(bi) {
                *slot = true;
            }
            *code.get_mut(ci)? = imm;
            folded.push((hi, pi, ci as u32));
        }
    }
    if folded.is_empty() {
        return None;
    }
    let mut s2 = Stencil {
        name: s.name.clone(),
        code: s.code.clone(),
        holes: s.holes.clone(),
        tail: s.tail,
        ..Stencil::unspilled()
    };
    for (hi, pi, ci) in &folded {
        // The pair is gone; `remove` drops its sites with the words they sat
        // in, and the hole gains an `imm12` field. A hole materialised more
        // than once keeps the pairs that did not fold.
        // `hi` and `pi` enumerated `s.holes` and its pairs, and `s2` is a clone.
        let hole = s2.holes.get_mut(*hi)?;
        *hole.pairs.get_mut(*pi)? = (u32::MAX, u32::MAX);
        hole.lo12.push((*ci * 4, 1));
    }
    for h in s2.holes.iter_mut() {
        h.pairs.retain(|p| p.0 != u32::MAX);
    }
    remove(&s2, &dead, &mut code, format!("{}+ifold", s.name))
}
