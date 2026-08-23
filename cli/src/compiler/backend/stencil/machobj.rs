//! A Mach-O (arm64) object reader, enough to recover stencils and their holes.
//!
//! This is the step the paper calls "the stencil library builder": clang
//! compiles the stencil generators to an object file, and the builder "extracts
//! their binary code and the linker relocation records containing information
//! about the holes" (§5.3). On x86-64/ELF, as in the paper, a hole is a
//! `R_X86_64_64` or `PC32` field that a value drops straight into. On
//! arm64/Mach-O it is never one field — see `patch.rs` for what each relocation
//! kind costs us.
//!
//! Nothing here is general: it reads exactly the shapes `clang -c` emits for the
//! stencil sources, and returns an error rather than guessing on anything else.

#![allow(dead_code)]
#![allow(
    clippy::arithmetic_side_effects,
    reason = "every sum here is an offset into an object file this same build \
              script just had clang write: a structure's base plus a fixed \
              field offset from the Mach-O headers, or a table's base plus an \
              index bounded by the count the file itself declared. None of \
              them is ever the value returned — each one is handed straight to \
              a `.get`, so an offset the object put past its own end leaves \
              through the `truncated` error rather than reading a neighbour"
)]

pub const LC_SEGMENT_64: u32 = 0x19;
pub const LC_SYMTAB: u32 = 0x02;

// arm64 relocation types (mach-o/arm64/reloc.h).
pub const ARM64_RELOC_UNSIGNED: u8 = 0;
pub const ARM64_RELOC_SUBTRACTOR: u8 = 1;
pub const ARM64_RELOC_BRANCH26: u8 = 2;
pub const ARM64_RELOC_PAGE21: u8 = 3;
pub const ARM64_RELOC_PAGEOFF12: u8 = 4;
pub const ARM64_RELOC_GOT_LOAD_PAGE21: u8 = 5;
pub const ARM64_RELOC_GOT_LOAD_PAGEOFF12: u8 = 6;
pub const ARM64_RELOC_POINTER_TO_GOT: u8 = 7;
pub const ARM64_RELOC_ADDEND: u8 = 10;

#[derive(Clone, Debug)]
pub struct Reloc {
    /// Byte offset within the section.
    pub addr: u32,
    pub kind: u8,
    pub pcrel: bool,
    pub length: u8,
    pub external: bool,
    /// Index into the symbol table when `external`, else a section number.
    pub symbolnum: u32,
}

#[derive(Clone, Debug)]
pub struct Sym {
    pub name: String,
    /// Section number, 1-based; 0 for an undefined symbol.
    pub sect: u8,
    pub value: u64,
    pub defined: bool,
}

pub struct Obj {
    /// The bytes of `__TEXT,__text`.
    pub text: Vec<u8>,
    pub text_relocs: Vec<Reloc>,
    pub syms: Vec<Sym>,
    /// Names of any other section that carried bytes, for the error message a
    /// stencil that spilled a constant into `__const` deserves.
    pub other_sections: Vec<(String, u64)>,
}

/// What a field read past the end of the object reports. A structure whose
/// fields are not all inside the file is not one this reader can go on with,
/// and there is no partial answer worth returning.
fn truncated(o: usize) -> String {
    format!("Mach-O object ends before offset {o:#x}")
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
fn cstr16(b: &[u8], o: usize) -> Result<String, String> {
    let s = b.get(o..o + 16).ok_or_else(|| truncated(o))?;
    let n = s.iter().position(|c| *c == 0).unwrap_or(16);
    // `n` is either a position within `s` or `s`'s own length, so the prefix
    // exists; `s` itself is what the whole-field case would answer anyway.
    Ok(String::from_utf8_lossy(s.get(..n).unwrap_or(s)).into_owned())
}

pub fn read(bytes: &[u8]) -> Result<Obj, String> {
    if bytes.len() < 32 || u32le(bytes, 0)? != 0xfeed_facf {
        return Err("not a 64-bit little-endian Mach-O object".into());
    }
    let ncmds = u32le(bytes, 16)? as usize;
    let mut off = 32usize;
    let mut text: Vec<u8> = Vec::new();
    let mut text_relocs = Vec::new();
    let mut syms = Vec::new();
    let mut other_sections = Vec::new();
    let mut text_sect_no: u8 = 0;

    for _ in 0..ncmds {
        let cmd = u32le(bytes, off)?;
        let cmdsize = u32le(bytes, off + 4)? as usize;
        if cmd == LC_SEGMENT_64 {
            let nsects = u32le(bytes, off + 64)? as usize;
            let mut so = off + 72;
            for i in 0..nsects {
                let sectname = cstr16(bytes, so)?;
                let segname = cstr16(bytes, so + 16)?;
                let size = u64le(bytes, so + 40)?;
                let fileoff = u32le(bytes, so + 48)? as usize;
                let reloff = u32le(bytes, so + 56)? as usize;
                let nreloc = u32le(bytes, so + 60)? as usize;
                if segname == "__TEXT" && sectname == "__text" {
                    text_sect_no = (i + 1) as u8;
                    text = bytes
                        .get(fileoff..fileoff + size as usize)
                        .ok_or_else(|| truncated(fileoff))?
                        .to_vec();
                    for r in 0..nreloc {
                        let ro = reloff + r * 8;
                        let addr = u32le(bytes, ro)?;
                        let info = u32le(bytes, ro + 4)?;
                        text_relocs.push(Reloc {
                            addr,
                            symbolnum: info & 0x00ff_ffff,
                            pcrel: (info >> 24) & 1 == 1,
                            length: ((info >> 25) & 3) as u8,
                            external: (info >> 27) & 1 == 1,
                            kind: ((info >> 28) & 0xf) as u8,
                        });
                    }
                } else if size > 0 && sectname != "__compact_unwind" && sectname != "__eh_frame" {
                    other_sections.push((format!("{segname},{sectname}"), size));
                }
                so += 80;
            }
        } else if cmd == LC_SYMTAB {
            let symoff = u32le(bytes, off + 8)? as usize;
            let nsyms = u32le(bytes, off + 12)? as usize;
            let stroff = u32le(bytes, off + 16)? as usize;
            for i in 0..nsyms {
                let o = symoff + i * 16;
                let strx = u32le(bytes, o)? as usize;
                let n_type = *bytes.get(o + 4).ok_or_else(|| truncated(o + 4))?;
                let n_sect = *bytes.get(o + 5).ok_or_else(|| truncated(o + 5))?;
                let value = u64le(bytes, o + 8)?;
                let at = stroff + strx;
                let rest = bytes.get(at..).ok_or_else(|| truncated(at))?;
                let end = rest.iter().position(|c| *c == 0).unwrap_or(0);
                // `end` is a position within `rest`, so the prefix exists; the
                // empty default is the unterminated-string case, as before.
                let name =
                    String::from_utf8_lossy(rest.get(..end).unwrap_or_default()).into_owned();
                // N_TYPE mask 0x0e; N_SECT == 0x0e means "defined in a section".
                let defined = n_type & 0x0e == 0x0e;
                syms.push(Sym { name, sect: n_sect, value, defined });
            }
        }
        off += cmdsize;
    }
    // Drop symbols defined in a section other than __text: nothing should be.
    for s in &syms {
        if s.defined && s.sect != text_sect_no && text_sect_no != 0 {
            return Err(format!("symbol {} is defined outside __text", s.name));
        }
    }
    Ok(Obj { text, text_relocs, syms, other_sections })
}

/// The functions in `__text`, as `(name, start, end)` in section order.
///
/// Sizes come from the next symbol's start, which is what
/// `.subsections_via_symbols` makes true, with the section end for the last.
pub fn functions(o: &Obj) -> Vec<(String, usize, usize)> {
    let mut starts: Vec<(u64, String)> = o
        .syms
        .iter()
        .filter(|s| s.defined && !s.name.starts_with("ltmp") && !s.name.starts_with("L"))
        .map(|s| (s.value, s.name.clone()))
        .collect();
    starts.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
    // Two symbols may share an address when clang aliases identical bodies, so
    // the end of a function is the next *distinct* start, not the next entry.
    let mut out = Vec::new();
    for (i, (value, name)) in starts.iter().enumerate() {
        // `i` came from enumerating `starts`, so `i + 1 ..` is a suffix of it —
        // empty for the last entry, which is the case the section end covers.
        let end = starts
            .get(i + 1..)
            .unwrap_or_default()
            .iter()
            .find(|(v, _)| v != value)
            .map(|(v, _)| *v as usize)
            .unwrap_or(o.text.len());
        out.push((name.clone(), *value as usize, end));
    }
    out
}
