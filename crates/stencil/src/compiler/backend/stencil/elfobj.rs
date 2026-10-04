//! An ELF64 (little-endian) object reader, enough to recover stencils and their
//! holes — `machobj.rs`'s sibling for the two Linux targets.
//!
//! It reads what `clang -target {aarch64,x86_64}-unknown-linux-musl -c` emits for
//! the same generated C `machobj.rs` reads out of a Mach-O, and it answers the
//! same four questions: the bytes of the code section, the relocation records
//! against it, the symbol table, and whether anything spilled into a section
//! that is not the code.
//!
//! Three differences from the Mach-O reader are worth naming, because each one
//! makes this file *simpler* rather than harder and the simplification is load
//! bearing downstream:
//!
//! * **`RELA`, not `REL`.** Both Linux psABIs use explicit addends, so there is
//!   no `ARM64_RELOC_ADDEND` prefix record to interleave and no addend hidden in
//!   the instruction stream. A relocation is one 24-byte record and says
//!   everything.
//! * **`st_size` is real.** A Mach-O function's extent has to be inferred from
//!   the next symbol's start, which is what `.subsections_via_symbols` makes
//!   true and what forces `machobj::functions` to sort and de-duplicate. ELF
//!   records the size on the symbol, so a body is exact and there is no trailing
//!   alignment padding inside it to trim. That matters on x86-64, where the
//!   padding is a multi-byte `nopw` rather than a recognisable word.
//! * **A relocation names a symbol index directly**, with no `external` bit: a
//!   relocation against a section is a relocation against that section's symbol,
//!   which for these objects never happens in `.text`.
//!
//! Nothing here is general. It reads exactly the shapes `clang -c` emits for the
//! stencil sources, and returns an error rather than guessing on anything else.
//! Like `machobj.rs` it is compiled by `cli/build.rs`, and additionally by
//! `elf.rs` under `cfg(test)`, so that this crate's ELF *writer* is checked
//! against the same reader.

#![allow(dead_code)]
#![allow(
    clippy::arithmetic_side_effects,
    reason = "every sum here is an offset into an object file this same build \
              script just had clang write: a table's base plus a fixed field \
              offset from the ELF64 headers, or a table's base plus an index \
              times an entry size the file itself declared. None of them is \
              ever the value returned — each one is handed straight to a \
              `.get`, so an offset the object put past its own end leaves \
              through the `truncated` error rather than reading a neighbour"
)]

// Section header types.
const SHT_PROGBITS: u32 = 1;
const SHT_SYMTAB: u32 = 2;
const SHT_STRTAB: u32 = 3;
const SHT_RELA: u32 = 4;
const SHT_NOBITS: u32 = 8;

/// `SHF_ALLOC`. A section without it is not part of the image — `.comment`,
/// `.llvm_addrsig`, the debug sections — so nothing in it can be a constant a
/// stencil spilled.
const SHF_ALLOC: u64 = 0x2;

/// `SHF_EXECINSTR`. A section with it holds instructions, and the only
/// instructions a stencil may consist of are `.text`'s: a second executable
/// section is a piece of a function a compiler carved out — `.text.unlikely.`,
/// `.text.split.` — never a constant. Carried so `x86.rs` can refuse it while
/// still accepting the read-only sections a spilled SSE constant lives in.
const SHF_EXECINSTR: u64 = 0x4;

/// `EM_X86_64` and `EM_AARCH64`, the only two `e_machine` values this reader
/// accepts. The machine decides how a relocation type number is read, so it is
/// returned rather than checked and forgotten.
pub const EM_X86_64: u16 = 62;
pub const EM_AARCH64: u16 = 183;

// aarch64 relocation types (the AArch64 ELF psABI, §4.6). These are the exact
// counterparts of the Mach-O kinds `machobj.rs` names, and the mapping is
// one-to-one but for the branch, which ELF splits by instruction.
pub const R_AARCH64_ABS64: u32 = 257;
pub const R_AARCH64_CALL26: u32 = 283;
pub const R_AARCH64_JUMP26: u32 = 282;
pub const R_AARCH64_ADR_PREL_PG_HI21: u32 = 275;
pub const R_AARCH64_ADD_ABS_LO12_NC: u32 = 277;
pub const R_AARCH64_LDST64_ABS_LO12_NC: u32 = 286;
pub const R_AARCH64_ADR_GOT_PAGE: u32 = 311;
pub const R_AARCH64_LD64_GOT_LO12_NC: u32 = 312;

// x86-64 relocation types (the x86-64 psABI, §4.4.1).
pub const R_X86_64_64: u32 = 1;
pub const R_X86_64_PC32: u32 = 2;
pub const R_X86_64_GOTPCREL: u32 = 9;
pub const R_X86_64_32: u32 = 10;
pub const R_X86_64_32S: u32 = 11;
pub const R_X86_64_PLT32: u32 = 4;
/// `R_X86_64_GOTPCREL` with a promise that the instruction is one of the forms
/// a linker may relax. Clang emits it for `mov sym@GOTPCREL(%rip), %reg`, which
/// is what a default-visibility hole compiles to; it is the same *field* as
/// `R_X86_64_GOTPCREL` and is read the same way here.
pub const R_X86_64_GOTPCRELX: u32 = 41;
pub const R_X86_64_REX_GOTPCRELX: u32 = 42;

#[derive(Clone, Debug)]
pub struct Reloc {
    /// Byte offset within the code section.
    pub addr: u32,
    /// The psABI relocation type; how to read it depends on [`Obj::machine`].
    pub kind: u32,
    /// Index into [`Obj::syms`].
    pub symbolnum: u32,
    /// The explicit addend. For every pc-relative x86-64 kind clang emits it is
    /// `-4` — the distance from the field to the end of the instruction — and
    /// for every aarch64 kind here it is `0`. Carried rather than assumed so
    /// that a shape this reader did not expect is a mismatch the caller can
    /// name.
    pub addend: i64,
}

#[derive(Clone, Debug)]
pub struct Sym {
    pub name: String,
    /// Section index; `SHN_UNDEF` (0) for an undefined symbol.
    pub sect: u16,
    pub value: u64,
    /// `st_size`, which for a `FUNC` in these objects is the body's exact
    /// length.
    pub size: u64,
    pub defined: bool,
}

/// An allocatable section of the object that is not `.text`.
///
/// On AArch64 there is never one and its presence is an error. On x86-64 it is
/// where clang puts a constant no instruction can hold — an SSE sign mask, the
/// two biases of a `u64`-to-`double` conversion — so the **bytes** are carried
/// and not only measured: `x86.rs` copies them into the stencil, and the
/// emitter copies them into the unit's own constant pool.
#[derive(Clone, Debug)]
pub struct Other {
    pub name: String,
    /// Section index, which is what a defined symbol's `st_shndx` names.
    pub index: u16,
    /// `sh_addralign`. An SSE constant asks for sixteen and reading it at a
    /// lesser alignment is a fault, not a slowdown.
    pub align: u64,
    /// The section's bytes; empty for a `NOBITS` section, which has none.
    pub data: Vec<u8>,
    /// Whether the section holds instructions ([`SHF_EXECINSTR`]). `x86.rs`
    /// refuses one that does; the arm64 reader refuses every section here and
    /// never looks.
    pub exec: bool,
}

pub struct Obj {
    /// `EM_AARCH64` or `EM_X86_64`.
    pub machine: u16,
    /// The bytes of `.text`.
    pub text: Vec<u8>,
    pub text_relocs: Vec<Reloc>,
    pub syms: Vec<Sym>,
    /// Every other allocatable section that carried bytes.
    pub other_sections: Vec<Other>,
}

/// What a field read past the end of the object reports. A structure whose
/// fields are not all inside the file is not one this reader can go on with,
/// and there is no partial answer worth returning.
fn truncated(o: usize) -> String {
    format!("ELF object ends before offset {o:#x}")
}

fn u16le(b: &[u8], o: usize) -> Result<u16, String> {
    let mut a = [0u8; 2];
    a.copy_from_slice(b.get(o..o + 2).ok_or_else(|| truncated(o))?);
    Ok(u16::from_le_bytes(a))
}
fn u32le(b: &[u8], o: usize) -> Result<u32, String> {
    let mut a = [0u8; 4];
    a.copy_from_slice(b.get(o..o + 4).ok_or_else(|| truncated(o))?);
    Ok(u32::from_le_bytes(a))
}
fn u64le(b: &[u8], o: usize) -> Result<u64, String> {
    let mut a = [0u8; 8];
    a.copy_from_slice(b.get(o..o + 8).ok_or_else(|| truncated(o))?);
    Ok(u64::from_le_bytes(a))
}

/// A NUL-terminated name at `at` in a string table.
fn cstr(b: &[u8], at: usize) -> Result<String, String> {
    let rest = b.get(at..).ok_or_else(|| truncated(at))?;
    let end = rest.iter().position(|c| *c == 0).unwrap_or(0);
    // `end` is a position within `rest`, so the prefix exists; the empty
    // default is the unterminated-string case, which is a name no caller
    // matches on.
    Ok(String::from_utf8_lossy(rest.get(..end).unwrap_or_default()).into_owned())
}

/// One section header, as the four fields anything below needs.
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

const EHDR_SIZE: usize = 64;
const SHDR_SIZE: usize = 64;
const SYM_SIZE: usize = 24;
const RELA_SIZE: usize = 24;

pub fn read(bytes: &[u8]) -> Result<Obj, String> {
    // ELFCLASS64 (2) and ELFDATA2LSB (1) in `e_ident`, after `\x7fELF`.
    if bytes.len() < EHDR_SIZE
        || bytes.get(..4) != Some(b"\x7fELF")
        || bytes.get(4) != Some(&2)
        || bytes.get(5) != Some(&1)
    {
        return Err("not a 64-bit little-endian ELF object".into());
    }
    let machine = u16le(bytes, 18)?;
    if machine != EM_AARCH64 && machine != EM_X86_64 {
        return Err(format!("ELF object is for machine {machine}, not aarch64 or x86-64"));
    }
    let shoff = u64le(bytes, 40)? as usize;
    let shentsize = u16le(bytes, 58)? as usize;
    let shnum = u16le(bytes, 60)? as usize;
    let shstrndx = u16le(bytes, 62)? as usize;
    if shentsize != SHDR_SIZE {
        return Err(format!("ELF section headers are {shentsize} bytes, not {SHDR_SIZE}"));
    }

    let mut shdrs = Vec::with_capacity(shnum);
    for i in 0..shnum {
        let o = shoff + i * SHDR_SIZE;
        shdrs.push(Shdr {
            name: u32le(bytes, o)?,
            kind: u32le(bytes, o + 4)?,
            flags: u64le(bytes, o + 8)?,
            offset: u64le(bytes, o + 24)?,
            size: u64le(bytes, o + 32)?,
            link: u32le(bytes, o + 40)?,
            info: u32le(bytes, o + 44)?,
            align: u64le(bytes, o + 48)?,
            entsize: u64le(bytes, o + 56)?,
        });
    }

    // The section-name string table, needed before any section can be named.
    let shstr = {
        let sh = shdrs.get(shstrndx).ok_or("ELF object names a section-name table it lacks")?;
        let at = sh.offset as usize;
        bytes.get(at..at + sh.size as usize).ok_or_else(|| truncated(at))?
    };
    let name_of = |sh: &Shdr| cstr(shstr, sh.name as usize);

    // `.text`, and everything allocatable that is not it.
    let mut text: Vec<u8> = Vec::new();
    let mut text_index: u16 = 0;
    let mut other_sections = Vec::new();
    for (i, sh) in shdrs.iter().enumerate() {
        let name = name_of(sh)?;
        if name == ".text" {
            text_index = i as u16;
            let at = sh.offset as usize;
            text = bytes.get(at..at + sh.size as usize).ok_or_else(|| truncated(at))?.to_vec();
        } else if sh.flags & SHF_ALLOC != 0
            && sh.size > 0
            && (sh.kind == SHT_PROGBITS || sh.kind == SHT_NOBITS)
            // `.eh_frame` is unwind metadata about the bytes, not bytes a
            // stencil reads; the generated C is compiled with
            // `-fno-asynchronous-unwind-tables` so it should not be here at
            // all, and a clang that emits it anyway is not a spill.
            && !name.starts_with(".eh_frame")
        {
            let at = sh.offset as usize;
            let data = if sh.kind == SHT_NOBITS {
                Vec::new()
            } else {
                bytes.get(at..at + sh.size as usize).ok_or_else(|| truncated(at))?.to_vec()
            };
            other_sections.push(Other {
                name,
                index: i as u16,
                align: sh.align,
                data,
                exec: sh.flags & SHF_EXECINSTR != 0,
            });
        }
    }
    if text.is_empty() && text_index == 0 {
        return Err("ELF object has no .text".into());
    }

    // The symbol table. There is exactly one `SHT_SYMTAB` in a relocatable
    // object, and its `sh_link` names the string table its names live in.
    let mut syms = Vec::new();
    for sh in shdrs.iter().filter(|s| s.kind == SHT_SYMTAB) {
        if sh.entsize as usize != SYM_SIZE {
            return Err(format!("ELF symbols are {} bytes, not {SYM_SIZE}", sh.entsize));
        }
        let strtab = {
            let s = shdrs
                .get(sh.link as usize)
                .filter(|s| s.kind == SHT_STRTAB)
                .ok_or("the ELF symbol table names no string table")?;
            let at = s.offset as usize;
            bytes.get(at..at + s.size as usize).ok_or_else(|| truncated(at))?
        };
        let n = (sh.size / SYM_SIZE as u64) as usize;
        for i in 0..n {
            let o = sh.offset as usize + i * SYM_SIZE;
            let strx = u32le(bytes, o)? as usize;
            let shndx = u16le(bytes, o + 6)?;
            syms.push(Sym {
                name: cstr(strtab, strx)?,
                sect: shndx,
                value: u64le(bytes, o + 8)?,
                size: u64le(bytes, o + 16)?,
                defined: shndx != 0,
            });
        }
    }

    // The relocations against `.text`, which is the one section this reader
    // relocates: the pool a stencil would need does not exist, and a
    // relocation anywhere else belongs to a section `other_sections` already
    // reported.
    let mut text_relocs = Vec::new();
    for sh in shdrs.iter().filter(|s| s.kind == SHT_RELA && s.info as u16 == text_index) {
        if sh.entsize as usize != RELA_SIZE {
            return Err(format!("ELF relocations are {} bytes, not {RELA_SIZE}", sh.entsize));
        }
        let n = (sh.size / RELA_SIZE as u64) as usize;
        for i in 0..n {
            let o = sh.offset as usize + i * RELA_SIZE;
            let info = u64le(bytes, o + 8)?;
            text_relocs.push(Reloc {
                addr: u64le(bytes, o)? as u32,
                kind: (info & 0xffff_ffff) as u32,
                symbolnum: (info >> 32) as u32,
                addend: u64le(bytes, o + 16)? as i64,
            });
        }
    }

    // The same guard `machobj::read` ends on: a symbol defined outside the code
    // section is a constant that escaped, and the caller decides what to do
    // about it from `other_sections`. Local label symbols (`.L…`) in a spilled
    // constant pool are exactly that case, so they are left in the table for
    // the caller to trace and not refused here.
    Ok(Obj { machine, text, text_relocs, syms, other_sections })
}

/// The functions in `.text`, as `(name, start, end)` in section order.
///
/// `st_size` rather than the next symbol's start, so a body is exact and
/// carries none of the inter-function alignment padding. That is why nothing
/// downstream has to recognise a multi-byte `nopw`.
///
/// What is returned is every *sized* defined symbol that is not a `.L` local
/// label, which for these objects is exactly the functions: the section symbol,
/// the `FILE` entry and a spilled constant's label all have `st_size == 0`.
/// Filtering on `STT_FUNC` instead would be the same set and would additionally
/// have to trust `st_info`, which nothing else here reads.
pub fn functions(o: &Obj) -> Vec<(String, usize, usize)> {
    let text_len = o.text.len();
    let mut out: Vec<(String, usize, usize)> = o
        .syms
        .iter()
        .filter(|s| s.defined && s.size > 0 && !s.name.is_empty() && !s.name.starts_with(".L"))
        .map(|s| {
            let start = s.value as usize;
            // `st_size` is what clang wrote; clamping to the section keeps a
            // malformed object out of the slice below rather than trusting it.
            let end = start.saturating_add(s.size as usize).min(text_len);
            (s.name.clone(), start, end)
        })
        .collect();
    out.sort_by(|a, b| a.1.cmp(&b.1).then(a.0.cmp(&b.0)));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn foreign_bytes_are_refused() {
        assert!(read(b"").is_err());
        assert!(read(&[0u8; 64]).is_err());
        // A 32-bit ELF, which this reader has no business guessing at.
        let mut e = vec![0u8; 64];
        e.splice(..4, *b"\x7fELF");
        e[4] = 1;
        e[5] = 1;
        assert!(read(&e).is_err());
    }
}
