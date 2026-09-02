//! An ELF64 relocatable object *writer* — `object.rs`'s sibling for Linux.
//!
//! `object.rs` writes what `ld64` will accept; this writes what `mold`, `ld.lld`
//! and `ld.bfd` will. It takes the **same three slices** `object::write` takes,
//! and that is the point: a codegen unit is described once — sections, symbols,
//! relocations — and the container is a choice made at the end of `mod.rs`
//! rather than a second emitter. `elfobj.rs` is the reader that closes the
//! circle, and it is the build script's.
//!
//! Nothing here is a general ELF emitter. It writes one relocatable object with
//! the sections a codegen unit has, a `RELA` section for each one that needs it,
//! a symbol table and two string tables, and it writes exactly the four
//! relocation kinds an arm64 code generator needs. Anything it cannot express is
//! an `Err`, never a guess.
//!
//! # Why RELA is simpler than what Mach-O needed, in three places
//!
//! * **Addends are a field.** `object.rs` has a long header about the two places
//!   a Mach-O addend has to live — inside the relocated bytes for `UNSIGNED`,
//!   and in a separate `ARM64_RELOC_ADDEND` record squeezed into 24 bits for the
//!   pc-relative kinds. `Elf64_Rela` has a signed 64-bit `r_addend`, so every
//!   addend goes in it, the section bytes are left alone, and there is no
//!   24-bit ceiling to refuse anything against.
//! * **There is no `LC_DYSYMTAB`.** ELF states where the local symbols end in
//!   the symbol table's own `sh_info`, so the sort this file does is one field
//!   rather than a load command. The sort itself is still required — ELF
//!   mandates every `STB_LOCAL` before every other symbol — which is why the
//!   caller's relocation indices are remapped here exactly as they are there.
//! * **`.subsections_via_symbols` has no counterpart and needs none.**
//!   `build/link.rs` passes `-Wl,--gc-sections` on Linux, which collects whole
//!   *sections*; a unit emits one `.text`, so nothing inside it can be
//!   collected out from under a branch that was baked rather than relocated.
//!   The Mach-O side has to relocate every intra-unit call for precisely that
//!   reason (`jit.rs::resolve`), and it still does — this file is not what makes
//!   that safe, and nothing here relies on it being unnecessary.
//!
//! # What the relocation kinds cost
//!
//! Mach-O names one `ARM64_RELOC_PAGEOFF12` for the low half of any `adrp` pair
//! and lets the linker read the instruction to decide what to do with it. The
//! AArch64 ELF psABI splits it by *instruction*: `ADD_ABS_LO12_NC` for an `add`
//! and `LDST64_ABS_LO12_NC` for a 64-bit `ldr`, and the two are not
//! interchangeable — the second is scaled by eight and the first is not. So this
//! file decodes the instruction at the site, which is the one place it looks at
//! the bytes it is relocating. The same split applies to `BRANCH26`, which ELF
//! spells `JUMP26` for a `b` and `CALL26` for a `bl`.
//!
//! # Determinism
//!
//! The same inputs must produce the same bytes: an object that differs run to
//! run defeats the build cache that decides whether a link is needed at all. So
//! every ordering in the output path comes from an index or a stable sort, no
//! map is iterated anywhere, and — unlike `ar` and unlike Mach-O's `LC_UUID` —
//! ELF has no timestamp or random field for `build/link.rs` to have to suppress.

#![allow(
    clippy::arithmetic_side_effects,
    reason = "this file's arithmetic is file-offset and table bookkeeping over \
              a buffer it is itself building: every operand is a length or an \
              offset already reached in a `Vec` held in memory, and every \
              result is the next such position. The header sizes are \
              constants, the section and symbol counts are bounded by the \
              caller's slices, and the one value that could come from outside \
              — a caller's relocation offset — is range-checked against the \
              section length before any arithmetic touches it"
)]

use super::abi::StencilTarget;
use super::object::{RelKind, Reloc, Section, Symbol};

// The ELF *reader* is `cli/build.rs`'s — the toolchain has no use for one and
// should not carry three hundred lines of it — but the strongest check
// available on a host that cannot run an ELF is that this file's output is
// something that reader accepts. So it is compiled here, under `cfg(test)`,
// and nowhere else in the toolchain's module tree.
#[cfg(test)]
#[path = "elfobj.rs"]
mod elfobj;

// Elf64_Ehdr / Elf64_Shdr / Elf64_Sym / Elf64_Rela sizes.
const EHDR: u64 = 64;
const SHDR: u64 = 64;
const SYM: u64 = 24;
const RELA: u64 = 24;

const ET_REL: u16 = 1;
const EM_AARCH64: u16 = 183;
const EM_X86_64: u16 = 62;

const SHT_PROGBITS: u32 = 1;
const SHT_SYMTAB: u32 = 2;
const SHT_STRTAB: u32 = 3;
const SHT_RELA: u32 = 4;
const SHT_NOBITS: u32 = 8;

const SHF_WRITE: u64 = 0x1;
const SHF_ALLOC: u64 = 0x2;
const SHF_EXECINSTR: u64 = 0x4;
const SHF_INFO_LINK: u64 = 0x40;

const STB_LOCAL: u8 = 0;
const STB_GLOBAL: u8 = 1;
const STT_NOTYPE: u8 = 0;
const STT_OBJECT: u8 = 1;
const STT_FUNC: u8 = 2;

// AArch64 ELF psABI §4.6.
const R_AARCH64_ABS64: u32 = 257;
const R_AARCH64_ADR_PREL_PG_HI21: u32 = 275;
const R_AARCH64_ADD_ABS_LO12_NC: u32 = 277;
const R_AARCH64_JUMP26: u32 = 282;
const R_AARCH64_CALL26: u32 = 283;
const R_AARCH64_LDST64_ABS_LO12_NC: u32 = 286;

// x86-64 psABI §4.4.1. Three kinds, which is the whole vocabulary an emitted
// unit uses: an absolute pool word, a branch, and a rip-relative reference.
const R_X86_64_64: u32 = 1;
const R_X86_64_PC32: u32 = 2;
const R_X86_64_PLT32: u32 = 4;

/// The widest alignment a section may ask for. `2^15` is far past anything a
/// code generator needs and keeps the shift below `u64`'s width.
const MAX_ALIGN_LOG2: u32 = 15;

/// A string table being built, with the empty name at offset zero.
///
/// Names are appended in the order they are asked for and never deduplicated
/// beyond an exact repeat, which is what keeps the output a function of the
/// input order alone.
#[derive(Default)]
struct Strtab {
    bytes: Vec<u8>,
    seen: Vec<(String, u32)>,
}

impl Strtab {
    fn new() -> Strtab {
        Strtab { bytes: vec![0], seen: Vec::new() }
    }
    fn add(&mut self, s: &str) -> u32 {
        if s.is_empty() {
            return 0;
        }
        if let Some((_, at)) = self.seen.iter().find(|(n, _)| n == s) {
            return *at;
        }
        let at = self.bytes.len() as u32;
        self.bytes.extend_from_slice(s.as_bytes());
        self.bytes.push(0);
        self.seen.push((String::from(s), at));
        at
    }
}

/// One section header, filled in as the layout is decided and written once.
struct Shdr {
    name: u32,
    kind: u32,
    flags: u64,
    offset: u64,
    size: u64,
    link: u32,
    info: u32,
    align: u64,
    entsize: u64,
}

impl Shdr {
    fn write(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.name.to_le_bytes());
        out.extend_from_slice(&self.kind.to_le_bytes());
        out.extend_from_slice(&self.flags.to_le_bytes());
        // sh_addr: zero in a relocatable object; the linker chooses it.
        out.extend_from_slice(&0u64.to_le_bytes());
        out.extend_from_slice(&self.offset.to_le_bytes());
        out.extend_from_slice(&self.size.to_le_bytes());
        out.extend_from_slice(&self.link.to_le_bytes());
        out.extend_from_slice(&self.info.to_le_bytes());
        out.extend_from_slice(&self.align.to_le_bytes());
        out.extend_from_slice(&self.entsize.to_le_bytes());
    }
}

/// The relocation type for one kind, decided from the instruction where the
/// psABI splits a Mach-O kind in two.
///
/// `w` is the four bytes at the site, which for the two split kinds is the
/// instruction whose field is being relocated. A kind the target has no
/// counterpart for is an `Err` naming both, rather than the nearest number.
fn r_type(target: StencilTarget, kind: RelKind, w: u32, where_: &str) -> Result<u32, String> {
    if target.is_arm64() {
        return Ok(match kind {
            RelKind::Abs64 => R_AARCH64_ABS64,
            RelKind::Page21 => R_AARCH64_ADR_PREL_PG_HI21,
            // `bl` is `0x94000000`, `b` is `0x14000000`: bit 31 is the only
            // difference, and it is what ELF splits on.
            RelKind::Branch26 => {
                if w & 0x8000_0000 != 0 {
                    R_AARCH64_CALL26
                } else {
                    R_AARCH64_JUMP26
                }
            }
            // `add Xd, Xn, #imm12` is `0x91……`; `ldr Xt, [Xn, #imm12]` is
            // `0xf94…….` The two low-12 relocations are *not* interchangeable:
            // the load's field is scaled by eight and the add's is not, so a
            // wrong choice here is a pool slot read from the wrong address.
            RelKind::PageOff12 => {
                if w & 0xffc0_0000 == 0x9100_0000 {
                    R_AARCH64_ADD_ABS_LO12_NC
                } else if w & 0xffc0_0000 == 0xf940_0000 {
                    R_AARCH64_LDST64_ABS_LO12_NC
                } else {
                    return Err(format!(
                        "{where_}: a low-12 relocation names {w:#010x}, which is neither \
                         `add Xd, Xn, #imm12` nor `ldr Xt, [Xn, #imm12]`"
                    ));
                }
            }
            // A64 has no `rel32` field of any width, so neither x86-64 kind
            // can be spelled for this machine.
            RelKind::Rel32 | RelKind::Pc32 => {
                return Err(format!("{where_}: {} has no aarch64 relocation type", x86_name(kind)))
            }
        });
    }
    match kind {
        RelKind::Abs64 => Ok(R_X86_64_64),
        RelKind::Rel32 => Ok(R_X86_64_PLT32),
        RelKind::Pc32 => Ok(R_X86_64_PC32),
        // The three A64 kinds have no x86-64 counterpart: a rip-relative
        // reference is one `disp32` field, so an x86-64 emission produces one
        // relocation where A64 produces two, and a 26-bit word displacement is
        // not a field this machine has.
        k => Err(format!(
            "{where_}: {} has no x86-64 relocation type",
            match k {
                RelKind::Page21 => "the page half of an adrp pair",
                RelKind::Branch26 => "the 26-bit displacement of a b/bl",
                _ => "the offset half of an adrp pair",
            }
        )),
    }
}

/// What an x86-64-only kind is called, for the message that says A64 has no
/// counterpart for it.
fn x86_name(kind: RelKind) -> &'static str {
    match kind {
        RelKind::Pc32 => "a rip-relative disp32",
        _ => "the rel32 of a jmp/call/jcc",
    }
}

/// The object's bytes.
///
/// Takes exactly what `object::write` takes, plus the target, which decides the
/// machine and the relocation vocabulary.
pub fn write(
    target: StencilTarget,
    sections: &[Section],
    symbols: &[Symbol],
    relocs: &[Reloc],
) -> Result<Vec<u8>, String> {
    if sections.is_empty() {
        return Err("an ELF object needs at least one section".into());
    }
    for s in sections {
        if s.align > MAX_ALIGN_LOG2 {
            return Err(format!(
                "section {} asks for 2^{} alignment; the maximum is 2^{MAX_ALIGN_LOG2}",
                s.name, s.align
            ));
        }
        if s.zerofill > 0 && !s.data.is_empty() {
            return Err(format!("zero-fill section {} also carries bytes", s.name));
        }
    }

    // ELF requires every `STB_LOCAL` symbol to precede every other one, and
    // `sh_info` to say where the boundary is. The caller's indices mean nothing
    // after that sort, so they are remapped here — the same rewrite
    // `object.rs` does for `LC_DYSYMTAB`, for the same reason.
    //
    // A *stable* partition rather than a sort, so that two symbols of the same
    // binding keep the order the caller pushed them in and the output stays a
    // function of the input.
    let mut order: Vec<usize> = (0..symbols.len()).filter(|i| !is_global(symbols, *i)).collect();
    let nlocal = order.len() + 1; // +1 for the mandatory null symbol at index 0.
    order.extend((0..symbols.len()).filter(|i| is_global(symbols, *i)));
    let mut slot = vec![0usize; symbols.len()];
    for (new, old) in order.iter().enumerate() {
        // `order` is a permutation of `0..symbols.len()`, so every index in it
        // names a slot, and `new + 1` leaves room for the null symbol.
        if let Some(s) = slot.get_mut(*old) {
            *s = new + 1;
        }
    }

    // Section numbering: 0 is the mandatory null section, then the caller's, in
    // their order, so `Definition::section` and `Reloc::section` need no
    // translation beyond the +1.
    let out_section = |i: usize| (i + 1) as u16;

    // Group the relocations by the section they apply to, keeping the caller's
    // order within each group.
    let mut per_section: Vec<Vec<&Reloc>> = sections.iter().map(|_| Vec::new()).collect();
    for r in relocs {
        let sec = sections
            .get(r.section)
            .ok_or_else(|| format!("a relocation names section {}, which does not exist", r.section))?;
        let width = if r.kind == RelKind::Abs64 { 8 } else { 4 };
        if r.offset + width > sec.data.len() as u64 {
            return Err(format!(
                "a relocation at {:#x} is outside section {} ({} bytes)",
                r.offset,
                sec.name,
                sec.data.len()
            ));
        }
        if slot.get(r.symbol).is_none() {
            return Err(format!("a relocation names symbol {}, which does not exist", r.symbol));
        }
        // `r.section` was just shown to name a section, and `per_section` has
        // one slot per section.
        if let Some(g) = per_section.get_mut(r.section) {
            g.push(r);
        }
    }

    // ---- names -----------------------------------------------------------
    let mut shstr = Strtab::new();
    let mut strtab = Strtab::new();
    // Laid out in the order the headers are written, so that the string table
    // is a function of the section list alone.
    let sec_name: Vec<u32> = sections.iter().map(|s| shstr.add(s.name)).collect();
    let rela_name: Vec<u32> = sections
        .iter()
        .enumerate()
        .map(|(i, s)| {
            if per_section.get(i).is_some_and(|g| !g.is_empty()) {
                shstr.add(&format!(".rela{}", s.name))
            } else {
                0
            }
        })
        .collect();
    let gnu_stack_name = shstr.add(".note.GNU-stack");
    let symtab_name = shstr.add(".symtab");
    let strtab_name = shstr.add(".strtab");
    let shstrtab_name = shstr.add(".shstrtab");

    // ---- the symbol table ------------------------------------------------
    let mut symtab: Vec<u8> = vec![0; SYM as usize]; // the null symbol
    for old in &order {
        let s = symbols.get(*old).ok_or("a symbol vanished from the table")?;
        let name = strtab.add(&s.name);
        let (shndx, value, kind) = match &s.defined {
            None => (0u16, 0u64, STT_NOTYPE),
            Some(d) => {
                let sec = sections
                    .get(d.section)
                    .ok_or_else(|| format!("symbol {} is defined in a section that does not exist", s.name))?;
                // A symbol in an executable section is a function; one in a
                // data or zero-fill section is an object. `--gc-sections`
                // reads neither, but `nm`, a profiler and a backtrace all do.
                let kind = if sec.attributes & super::object::CODE_ATTRIBUTES != 0 {
                    STT_FUNC
                } else {
                    STT_OBJECT
                };
                (out_section(d.section), d.offset, kind)
            }
        };
        let bind = if s.global { STB_GLOBAL } else { STB_LOCAL };
        symtab.extend_from_slice(&name.to_le_bytes());
        symtab.push((bind << 4) | kind);
        symtab.push(0); // st_other: default visibility.
        symtab.extend_from_slice(&shndx.to_le_bytes());
        symtab.extend_from_slice(&value.to_le_bytes());
        // st_size. Zero rather than a guess: this emitter knows a symbol's
        // start and not its end, the Mach-O side records none either, and a
        // wrong size is worse for a debugger than no size.
        symtab.extend_from_slice(&0u64.to_le_bytes());
    }

    // ---- the relocation tables ------------------------------------------
    let mut relatabs: Vec<Vec<u8>> = Vec::with_capacity(sections.len());
    for (i, group) in per_section.iter().enumerate() {
        let mut t = Vec::with_capacity(group.len() * RELA as usize);
        for r in group {
            let sec = sections.get(i).ok_or("a relocation group lost its section")?;
            let at = r.offset as usize;
            let w = u32::from_le_bytes([
                sec.data.get(at).copied().unwrap_or(0),
                sec.data.get(at + 1).copied().unwrap_or(0),
                sec.data.get(at + 2).copied().unwrap_or(0),
                sec.data.get(at + 3).copied().unwrap_or(0),
            ]);
            let ty = r_type(target, r.kind, w, &format!("{}+{:#x}", sec.name, r.offset))?;
            let sym = slot.get(r.symbol).copied().unwrap_or(0) as u64;
            t.extend_from_slice(&r.offset.to_le_bytes());
            t.extend_from_slice(&((sym << 32) | ty as u64).to_le_bytes());
            t.extend_from_slice(&r.addend.to_le_bytes());
        }
        relatabs.push(t);
    }

    // ---- layout ----------------------------------------------------------
    //
    // Section headers last, so that every body's offset is known before any of
    // them is written. Everything is eight-aligned in the file, which is the
    // widest `entsize` here and costs at most seven bytes a section.
    let mut body: Vec<u8> = Vec::new();
    let mut at = EHDR;
    let mut shdrs: Vec<Shdr> = Vec::new();
    // Index 0: the null section header, all zeros.
    shdrs.push(Shdr {
        name: 0,
        kind: 0,
        flags: 0,
        offset: 0,
        size: 0,
        link: 0,
        info: 0,
        align: 0,
        entsize: 0,
    });

    let place = |body: &mut Vec<u8>, at: &mut u64, data: &[u8], align: u64| -> u64 {
        let a = align.max(1);
        while !(*at).is_multiple_of(a) {
            body.push(0);
            *at += 1;
        }
        let off = *at;
        body.extend_from_slice(data);
        *at += data.len() as u64;
        off
    };

    for (i, s) in sections.iter().enumerate() {
        let align = 1u64 << s.align;
        let (kind, size, offset) = if s.zerofill > 0 {
            // A `NOBITS` section occupies no file bytes; its `sh_offset` is
            // conventionally where it *would* have gone, which is what makes a
            // hex dump of the file readable and costs nothing.
            (SHT_NOBITS, s.zerofill, at)
        } else {
            (SHT_PROGBITS, s.data.len() as u64, place(&mut body, &mut at, &s.data, align))
        };
        let exec = s.attributes & super::object::CODE_ATTRIBUTES != 0;
        // Code, or data that is written to. There is no third kind here: the
        // two non-code sections this emitter produces are the Buri stack
        // (`.bss`, zero-filled and written by the program) and the constant
        // pool (`.data.rel.ro`, written *once* by the dynamic relocations a
        // static PIE applies before `main` — see `mod.rs` for why the pool is
        // not `.rodata`). A read-only alloc section that carries an `Abs64`
        // relocation is one a `-static-pie` link refuses outright, so this is
        // the flag that decides whether a Linux artifact links at all rather
        // than a hardening detail. `PT_GNU_RELRO` is what gives the pool its
        // read-onlyness back, and the linker builds that from the section
        // *name*.
        let flags = SHF_ALLOC | if exec { SHF_EXECINSTR } else { SHF_WRITE };
        shdrs.push(Shdr {
            name: sec_name.get(i).copied().unwrap_or(0),
            kind,
            flags,
            offset,
            size,
            link: 0,
            info: 0,
            align,
            entsize: 0,
        });
    }

    // The `.rela` sections, each pointing at the section it relocates. Written
    // after every `PROGBITS` so that the section indices above are contiguous
    // and `out_section` stays the simple `+1`.
    let symtab_index = (1 + sections.len() + relatabs.iter().filter(|t| !t.is_empty()).count() + 1)
        as u32;
    for (i, t) in relatabs.iter().enumerate() {
        if t.is_empty() {
            continue;
        }
        let off = place(&mut body, &mut at, t, 8);
        shdrs.push(Shdr {
            name: rela_name.get(i).copied().unwrap_or(0),
            kind: SHT_RELA,
            flags: SHF_INFO_LINK,
            offset: off,
            size: t.len() as u64,
            link: symtab_index,
            info: u32::from(out_section(i)),
            align: 8,
            entsize: RELA,
        });
    }

    // An empty, non-allocatable `.note.GNU-stack`. Its *absence* is what makes
    // a Linux linker mark the stack executable and warn about it, and it is
    // zero bytes.
    shdrs.push(Shdr {
        name: gnu_stack_name,
        kind: SHT_PROGBITS,
        flags: 0,
        offset: at,
        size: 0,
        link: 0,
        info: 0,
        align: 1,
        entsize: 0,
    });

    let symtab_off = place(&mut body, &mut at, &symtab, 8);
    let strtab_off = place(&mut body, &mut at, &strtab.bytes, 1);
    let shstrtab_off = place(&mut body, &mut at, &shstr.bytes, 1);
    // The string table `.symtab` names is the one written immediately after it.
    let strtab_index = symtab_index + 1;
    shdrs.push(Shdr {
        name: symtab_name,
        kind: SHT_SYMTAB,
        flags: 0,
        offset: symtab_off,
        size: symtab.len() as u64,
        link: strtab_index,
        info: nlocal as u32,
        align: 8,
        entsize: SYM,
    });
    shdrs.push(Shdr {
        name: strtab_name,
        kind: SHT_STRTAB,
        flags: 0,
        offset: strtab_off,
        size: strtab.bytes.len() as u64,
        link: 0,
        info: 0,
        align: 1,
        entsize: 0,
    });
    shdrs.push(Shdr {
        name: shstrtab_name,
        kind: SHT_STRTAB,
        flags: 0,
        offset: shstrtab_off,
        size: shstr.bytes.len() as u64,
        link: 0,
        info: 0,
        align: 1,
        entsize: 0,
    });

    // The header index this file computed for `.symtab` above has to be the one
    // it actually landed at, or every relocation names the wrong table.
    let landed = shdrs.iter().position(|h| h.kind == SHT_SYMTAB).unwrap_or(0) as u32;
    if landed != symtab_index {
        return Err(format!(
            "internal error: .symtab was predicted at section {symtab_index} and written at {landed}"
        ));
    }

    // Section headers are eight-aligned, which `place` has already left `at`
    // ready for only by accident, so it is done explicitly.
    while !at.is_multiple_of(8) {
        body.push(0);
        at += 1;
    }
    let shoff = at;

    let mut out = Vec::with_capacity((EHDR + body.len() as u64 + shdrs.len() as u64 * SHDR) as usize);
    out.extend_from_slice(b"\x7fELF");
    out.push(2); // ELFCLASS64
    out.push(1); // ELFDATA2LSB
    out.push(1); // EV_CURRENT
    out.push(0); // ELFOSABI_SYSV
    out.push(0); // ABI version
    out.extend_from_slice(&[0u8; 7]); // e_ident padding
    out.extend_from_slice(&ET_REL.to_le_bytes());
    out.extend_from_slice(&if target.is_arm64() { EM_AARCH64 } else { EM_X86_64 }.to_le_bytes());
    out.extend_from_slice(&1u32.to_le_bytes()); // e_version
    out.extend_from_slice(&0u64.to_le_bytes()); // e_entry
    out.extend_from_slice(&0u64.to_le_bytes()); // e_phoff
    out.extend_from_slice(&shoff.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes()); // e_flags
    out.extend_from_slice(&(EHDR as u16).to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes()); // e_phentsize
    out.extend_from_slice(&0u16.to_le_bytes()); // e_phnum
    out.extend_from_slice(&(SHDR as u16).to_le_bytes());
    out.extend_from_slice(&(shdrs.len() as u16).to_le_bytes());
    out.extend_from_slice(&((shdrs.len() - 1) as u16).to_le_bytes()); // e_shstrndx
    out.extend_from_slice(&body);
    for h in &shdrs {
        h.write(&mut out);
    }
    Ok(out)
}

/// Whether the caller's symbol at `i` is a global one, with a missing index
/// answering "local" so that the partition is total.
fn is_global(symbols: &[Symbol], i: usize) -> bool {
    symbols.get(i).is_some_and(|s| s.global)
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::object::{Definition, CODE_ATTRIBUTES};

    use super::elfobj;

    fn text(data: Vec<u8>) -> Section {
        Section {
            name: ".text",
            segment: "",
            align: 2,
            zerofill: 0,
            attributes: CODE_ATTRIBUTES,
            data,
        }
    }

    /// `b` and `bl` are one Mach-O kind and two ELF ones, and getting it wrong
    /// is a call that returns into the wrong place.
    #[test]
    fn a_branch_is_split_by_its_instruction() {
        let jump = r_type(StencilTarget::LinuxArm64, RelKind::Branch26, 0x1400_0000, "t").unwrap();
        let call = r_type(StencilTarget::LinuxArm64, RelKind::Branch26, 0x9400_0000, "t").unwrap();
        assert_eq!((jump, call), (R_AARCH64_JUMP26, R_AARCH64_CALL26));
    }

    /// The low-12 split is the one that is silently wrong rather than loudly:
    /// both types exist, both apply to the same field, and only one scales.
    #[test]
    fn a_low_twelve_is_split_by_its_instruction() {
        // `add x8, x8, #0`
        let add = r_type(StencilTarget::LinuxArm64, RelKind::PageOff12, 0x9100_0108, "t").unwrap();
        // `ldr x8, [x8]`
        let ldr = r_type(StencilTarget::LinuxArm64, RelKind::PageOff12, 0xf940_0108, "t").unwrap();
        assert_eq!((add, ldr), (R_AARCH64_ADD_ABS_LO12_NC, R_AARCH64_LDST64_ABS_LO12_NC));
        // A `str`, which no pool reference is, is refused rather than guessed.
        assert!(r_type(StencilTarget::LinuxArm64, RelKind::PageOff12, 0xf900_0108, "t").is_err());
    }

    /// **Neither machine's kinds may be spelled for the other**, and the refusal
    /// goes both ways rather than only the way that used to be missing. A
    /// nearest-number guess here is a relocation the linker applies to the
    /// wrong field, which is not something a test downstream would catch.
    #[test]
    fn a_kind_the_machine_does_not_have_is_refused_by_name() {
        for k in [RelKind::Rel32, RelKind::Pc32] {
            assert!(r_type(StencilTarget::LinuxArm64, k, 0, "t").is_err());
        }
        for k in [RelKind::Branch26, RelKind::Page21, RelKind::PageOff12] {
            assert!(r_type(StencilTarget::LinuxX86_64, k, 0, "t").is_err());
        }
        // And the three x86-64 emission uses are the three psABI numbers.
        let of = |k| r_type(StencilTarget::LinuxX86_64, k, 0, "t").unwrap();
        assert_eq!(of(RelKind::Abs64), R_X86_64_64);
        assert_eq!(of(RelKind::Rel32), R_X86_64_PLT32);
        assert_eq!(of(RelKind::Pc32), R_X86_64_PC32);
    }

    /// The header this file writes has to be one a reader accepts, and there is
    /// a reader in this repository: `elfobj.rs`, which the build script uses on
    /// clang's objects. Round-tripping through it is the strongest check
    /// available on a host that cannot run the result.
    #[test]
    fn what_is_written_is_what_the_reader_reads() {
        // `b .` and `bl .`, so both branch types appear.
        let mut code = Vec::new();
        code.extend_from_slice(&0x1400_0000u32.to_le_bytes());
        code.extend_from_slice(&0x9400_0000u32.to_le_bytes());
        let sections = vec![
            text(code),
            Section {
                name: ".rodata",
                segment: "",
                align: 3,
                zerofill: 0,
                attributes: 0,
                data: vec![0; 8],
            },
        ];
        let symbols = vec![
            Symbol {
                name: String::from("f"),
                defined: Some(Definition { section: 0, offset: 0 }),
                global: true,
            },
            Symbol { name: String::from("buri_rt_abort"), defined: None, global: true },
            Symbol {
                name: String::from("pool"),
                defined: Some(Definition { section: 1, offset: 0 }),
                global: false,
            },
        ];
        let relocs = vec![
            Reloc { section: 0, offset: 0, kind: RelKind::Branch26, symbol: 2, addend: 0 },
            Reloc { section: 0, offset: 4, kind: RelKind::Branch26, symbol: 1, addend: 0 },
            Reloc { section: 1, offset: 0, kind: RelKind::Abs64, symbol: 0, addend: 3 },
        ];
        let bytes = write(StencilTarget::LinuxArm64, &sections, &symbols, &relocs).unwrap();
        let obj = elfobj::read(&bytes).expect("the writer's output must read back");
        assert_eq!(obj.machine, elfobj::EM_AARCH64);
        assert_eq!(obj.text.len(), 8);
        assert_eq!(obj.text_relocs.len(), 2);
        // The local symbol was sorted in front of the two globals, and the
        // relocations were remapped with it.
        let jump = obj.text_relocs.first().expect("a first relocation");
        assert_eq!(jump.kind, elfobj::R_AARCH64_JUMP26);
        assert_eq!(
            obj.syms.get(jump.symbolnum as usize).map(|s| s.name.as_str()),
            Some("pool")
        );
        let call = obj.text_relocs.get(1).expect("a second relocation");
        assert_eq!(call.kind, elfobj::R_AARCH64_CALL26);
        assert_eq!(
            obj.syms.get(call.symbolnum as usize).map(|s| s.name.as_str()),
            Some("buri_rt_abort")
        );
        // And `f` is a function of the code section, which is what a linker
        // and a profiler read.
        assert!(obj.syms.iter().any(|s| s.name == "f" && s.defined));
    }

    /// Two encodes of the same unit must be the same bytes, or the link cache
    /// relinks on every build.
    #[test]
    fn encoding_is_deterministic() {
        let sections = vec![text(vec![0; 16])];
        let symbols = vec![
            Symbol {
                name: String::from("a"),
                defined: Some(Definition { section: 0, offset: 0 }),
                global: true,
            },
            Symbol { name: String::from("b"), defined: None, global: true },
        ];
        let a = write(StencilTarget::LinuxArm64, &sections, &symbols, &[]).unwrap();
        let b = write(StencilTarget::LinuxArm64, &sections, &symbols, &[]).unwrap();
        assert_eq!(a, b);
    }
}
