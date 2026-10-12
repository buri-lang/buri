//! A debug test suite's objects, linked by `buri` into an image the test host
//! maps (`design/PERFORMANCE.md` §6.85).
//!
//! The image is the link `ld64` would do for a runner, less what the runtime
//! is: the host already holds all of `libburi_rt.a`. Objects are laid out in
//! link order, code then constants then data and the stack, and every branch
//! and page reference inside the image is patched here. What's left is a
//! fixup for the host to apply once it knows where things are:
//!
//! ```text
//! header   "BURIIMG\0", version u32, flags u32, the host's key (64 hex digits),
//!          text, const_at, consts, data_at, data, total, entry, stubs: u64,
//!          imports u32, fixups u32
//! imports  kind u32 (0 host offset, 1 C library name), name length u32, offset u64, name
//! fixups   at u64, kind u32 (0 address word, 1 b/bl, 2 adrp), import u32 (MAX: the image), addend u64
//! bytes    text (its last 16 × imports bytes are the stubs), constants, data
//! ```
//!
//! `cli/runtime/loader.rs` reads it. Both sides change together, and a host's
//! key holds the runtime archive's hash, so a host and the `buri` that built
//! it always agree.
//!
//! macOS arm64 only: the objects are the four relocation kinds
//! `stencil/object.rs` writes. Anything else is an `Err`, and the suite links a
//! runner as before.

#![allow(
    clippy::arithmetic_side_effects,
    reason = "offsets and sizes in objects already in memory: each section's \
              bytes are bounds-checked against its object before they are \
              copied, positions are sums of section sizes that fit in memory, \
              and relocation arithmetic wraps where the range check that \
              follows catches the result"
)]

use crate::hash::Map as HashMap;

pub const MAGIC: &[u8; 8] = b"BURIIMG\0";
pub const VERSION: u32 = 1;

const ABS64: u32 = 0;
const BRANCH26: u32 = 1;
const PAGE21: u32 = 2;
const HOST: u32 = 0;
const NAMED: u32 = 1;
const SELF: u32 = u32::MAX;

const PAGE: u64 = 16 * 1024;
const STUB_BYTES: u64 = 16;

// Mach-O.
const MH_MAGIC_64: u32 = 0xfeed_facf;
const MH_OBJECT: u32 = 1;
const LC_SEGMENT_64: u32 = 0x19;
const LC_SYMTAB: u32 = 0x2;
const N_STAB: u8 = 0xe0;
const N_TYPE: u8 = 0x0e;
const N_SECT: u8 = 0x0e;
const N_EXT: u8 = 0x01;
const N_WEAK_DEF: u16 = 0x0080;
const S_ZEROFILL: u32 = 0x1;
const ARM64_RELOC_UNSIGNED: u32 = 0;
const ARM64_RELOC_BRANCH26: u32 = 2;
const ARM64_RELOC_PAGE21: u32 = 3;
const ARM64_RELOC_PAGEOFF12: u32 = 4;
const ARM64_RELOC_ADDEND: u32 = 10;

/// How many bytes [`host_of`] reads.
pub const HEAD: usize = 80;

/// The key of the host an image was linked against, or `None` when the bytes
/// begin no image.
pub fn host_of(head: &[u8]) -> Option<&str> {
    if !head.starts_with(MAGIC) {
        return None;
    }
    std::str::from_utf8(head.get(16..HEAD)?).ok()
}

/// The host's symbols, by Mach-O name, as offsets from its Mach header.
pub struct HostSymbols(HashMap<String, u64>);

impl HostSymbols {
    /// The external symbols an executable defines.
    pub fn of_executable(bytes: &[u8]) -> Result<HostSymbols, String> {
        let file = MachO::parse(bytes, None)?;
        let text = file.segments.iter().find(|s| s.name == "__TEXT").map(|s| s.vmaddr);
        let text = text.ok_or("the host has no __TEXT segment")?;
        let mut map = HashMap::default();
        for sym in &file.symbols {
            if sym.n_type & N_STAB == 0 && sym.n_type & N_TYPE == N_SECT && sym.n_type & N_EXT != 0 {
                map.insert(sym.name.to_string(), sym.n_value.wrapping_sub(text));
            }
        }
        Ok(HostSymbols(map))
    }

    /// One `name offset` line each, sorted, as kept beside the host.
    pub fn to_text(&self) -> String {
        let mut lines: Vec<String> = self.0.iter().map(|(name, at)| format!("{name} {at:x}\n")).collect();
        lines.sort();
        lines.concat()
    }

    pub fn from_text(text: &str) -> Option<HostSymbols> {
        let mut map = HashMap::default();
        for line in text.lines() {
            let (name, at) = line.rsplit_once(' ')?;
            map.insert(name.to_string(), u64::from_str_radix(at, 16).ok()?);
        }
        (!map.is_empty()).then_some(HostSymbols(map))
    }

    fn get(&self, name: &str) -> Option<u64> {
        self.0.get(name).copied()
    }
}

/// Where a section's bytes go.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Area {
    Text,
    Const,
    Data,
    Zero,
}

struct Section {
    addr: u64,
    size: u64,
    offset: u32,
    align: u32,
    reloff: u32,
    nreloc: u32,
    area: Area,
    /// Its offset in the image, once laid out.
    placed: u64,
}

struct Segment<'a> {
    name: &'a str,
    vmaddr: u64,
}

struct Symbol<'a> {
    name: &'a str,
    n_type: u8,
    n_sect: u8,
    n_desc: u16,
    n_value: u64,
}

/// The parts of a Mach-O file this reads.
struct MachO<'a> {
    bytes: &'a [u8],
    sections: Vec<Section>,
    segments: Vec<Segment<'a>>,
    symbols: Vec<Symbol<'a>>,
}

fn u32_at(bytes: &[u8], at: usize) -> Result<u32, String> {
    let b = at.checked_add(4).and_then(|end| bytes.get(at..end)).ok_or("a Mach-O file is cut short")?;
    let mut w = [0u8; 4];
    w.copy_from_slice(b);
    Ok(u32::from_le_bytes(w))
}

fn u64_at(bytes: &[u8], at: usize) -> Result<u64, String> {
    let b = at.checked_add(8).and_then(|end| bytes.get(at..end)).ok_or("a Mach-O file is cut short")?;
    let mut w = [0u8; 8];
    w.copy_from_slice(b);
    Ok(u64::from_le_bytes(w))
}

fn name16(bytes: &[u8], at: usize) -> Result<&str, String> {
    let b = bytes.get(at..at + 16).ok_or("a Mach-O file is cut short")?;
    let end = b.iter().position(|&c| c == 0).unwrap_or(16);
    std::str::from_utf8(b.get(..end).unwrap_or_default()).map_err(|_| "a section name isn't text".to_string())
}

impl<'a> MachO<'a> {
    /// `filetype`, when given, is what the file must be.
    fn parse(bytes: &'a [u8], filetype: Option<u32>) -> Result<MachO<'a>, String> {
        if u32_at(bytes, 0)? != MH_MAGIC_64 {
            return Err("not a 64-bit Mach-O file".into());
        }
        if filetype.is_some_and(|t| u32_at(bytes, 12).ok() != Some(t)) {
            return Err("not a Mach-O object".into());
        }
        let ncmds = u32_at(bytes, 16)?;
        let mut at = 32usize;
        let mut file = MachO { bytes, sections: Vec::new(), segments: Vec::new(), symbols: Vec::new() };
        for _ in 0..ncmds {
            let (cmd, size) = (u32_at(bytes, at)?, u32_at(bytes, at + 4)? as usize);
            if cmd == LC_SEGMENT_64 {
                file.segments.push(Segment { name: name16(bytes, at + 8)?, vmaddr: u64_at(bytes, at + 24)? });
                for i in 0..u32_at(bytes, at + 64)? as usize {
                    let s = at + 72 + 80 * i;
                    let (sectname, segname) = (name16(bytes, s)?, name16(bytes, s + 16)?);
                    let flags = u32_at(bytes, s + 64)?;
                    let area = if flags & 0xff == S_ZEROFILL {
                        Area::Zero
                    } else {
                        match segname {
                            "__TEXT" => Area::Text,
                            "__DATA_CONST" => Area::Const,
                            "__DATA" => Area::Data,
                            other => return Err(format!("a section {other},{sectname}")),
                        }
                    };
                    file.sections.push(Section {
                        addr: u64_at(bytes, s + 32)?,
                        size: u64_at(bytes, s + 40)?,
                        offset: u32_at(bytes, s + 48)?,
                        align: u32_at(bytes, s + 52)?,
                        reloff: u32_at(bytes, s + 56)?,
                        nreloc: u32_at(bytes, s + 60)?,
                        area,
                        placed: 0,
                    });
                }
            } else if cmd == LC_SYMTAB {
                let (symoff, nsyms, stroff) =
                    (u32_at(bytes, at + 8)? as usize, u32_at(bytes, at + 12)? as usize, u32_at(bytes, at + 16)? as usize);
                for i in 0..nsyms {
                    let n = symoff + 16 * i;
                    let strx = stroff + u32_at(bytes, n)? as usize;
                    let tail = bytes.get(strx..).ok_or("a symbol's name is outside the file")?;
                    let end = tail.iter().position(|&c| c == 0).ok_or("a symbol's name never ends")?;
                    let name = std::str::from_utf8(tail.get(..end).unwrap_or_default())
                        .map_err(|_| "a symbol's name isn't text".to_string())?;
                    let head = bytes.get(n + 4..n + 8).ok_or("the symbol table is cut short")?;
                    let (n_type, n_sect) = (head.first().copied().unwrap_or(0), head.get(1).copied().unwrap_or(0));
                    let n_desc = u16::from_le_bytes([head.get(2).copied().unwrap_or(0), head.get(3).copied().unwrap_or(0)]);
                    file.symbols.push(Symbol { name, n_type, n_sect, n_desc, n_value: u64_at(bytes, n + 8)? });
                }
            }
            at += size;
        }
        Ok(file)
    }
}

/// What a symbol resolves to.
#[derive(Clone, Copy)]
enum Resolved {
    /// An offset in the image.
    Image(u64),
    /// An import, by index.
    Import(u32),
}

/// One of the image's imports.
struct Import {
    name: String,
    host: Option<u64>,
}

/// The image `objects` link into, against the symbols of the host whose key
/// is `host_key`.
pub fn link(objects: &[&[u8]], host: &HostSymbols, host_key: &str) -> Result<Vec<u8>, String> {
    if host_key.len() != HEAD - 16 {
        return Err("a host key that isn't a digest".into());
    }
    let mut files: Vec<MachO<'_>> =
        objects.iter().map(|bytes| MachO::parse(bytes, Some(MH_OBJECT))).collect::<Result<_, _>>()?;

    // Who defines each global. A weak definition gives way to a strong one,
    // and the first of several weak ones stands, as `ld` keeps it.
    let mut globals: HashMap<&str, (usize, usize, bool)> = HashMap::default();
    for (o, file) in files.iter().enumerate() {
        for (i, sym) in file.symbols.iter().enumerate() {
            if sym.n_type & N_STAB != 0 || sym.n_type & N_TYPE != N_SECT || sym.n_type & N_EXT == 0 {
                continue;
            }
            let weak = sym.n_desc & N_WEAK_DEF != 0;
            match globals.get(sym.name) {
                None => {
                    globals.insert(sym.name, (o, i, weak));
                }
                Some(&(_, _, true)) if !weak => {
                    globals.insert(sym.name, (o, i, weak));
                }
                Some(&(_, _, false)) if !weak => return Err(format!("{} is defined twice", sym.name)),
                Some(_) => {}
            }
        }
    }

    // The imports, in the order the relocations first name them.
    let mut imports: Vec<Import> = Vec::new();
    let mut import_of: HashMap<&str, u32> = HashMap::default();
    for file in &files {
        for section in &file.sections {
            for r in relocations(file.bytes, section)? {
                let Some(sym) = file.symbols.get(r.symbol) else { continue };
                if r.kind == ARM64_RELOC_ADDEND || sym.n_type & N_TYPE == N_SECT || globals.contains_key(sym.name) {
                    continue;
                }
                if !import_of.contains_key(sym.name) {
                    let index = u32::try_from(imports.len()).map_err(|_| "too many imports")?;
                    import_of.insert(sym.name, index);
                    let name = sym.name.strip_prefix('_').unwrap_or(sym.name).to_string();
                    imports.push(Import { name, host: host.get(sym.name) });
                }
            }
        }
    }

    // Layout: code and the stubs, then constants, then data and the stack,
    // each starting on a page.
    let mut cursor = 0u64;
    place(&mut files, Area::Text, &mut cursor)?;
    let stubs = cursor.next_multiple_of(STUB_BYTES);
    let text = stubs + STUB_BYTES * imports.len() as u64;
    let const_at = text.next_multiple_of(PAGE);
    cursor = const_at;
    place(&mut files, Area::Const, &mut cursor)?;
    let consts = cursor - const_at;
    let data_at = cursor.next_multiple_of(PAGE);
    cursor = data_at;
    place(&mut files, Area::Data, &mut cursor)?;
    let data = cursor - data_at;
    place(&mut files, Area::Zero, &mut cursor)?;
    let total = cursor.next_multiple_of(PAGE).max(data_at + PAGE);

    let mut bytes = vec![0u8; usize::try_from(data_at + data).map_err(|_| "the image is too large")?];
    for file in &files {
        for s in file.sections.iter().filter(|s| s.area != Area::Zero) {
            let from = s.offset as usize;
            let src = from.checked_add(s.size as usize).and_then(|end| file.bytes.get(from..end));
            let src = src.ok_or("a section's bytes are outside its object")?;
            image_slice(&mut bytes, s.placed, src.len())?.copy_from_slice(src);
        }
    }

    let address = |o: usize, sym: &Symbol<'_>| -> Option<u64> {
        let s = files.get(o)?.sections.get(usize::from(sym.n_sect).checked_sub(1)?)?;
        Some(s.placed + sym.n_value.wrapping_sub(s.addr))
    };
    let resolve = |o: usize, index: usize| -> Result<Resolved, String> {
        let sym = files.get(o).and_then(|f| f.symbols.get(index)).ok_or("a relocation names no symbol")?;
        let defined_here = sym.n_type & N_TYPE == N_SECT;
        if let Some(&(g, i, _)) = globals.get(sym.name).filter(|_| sym.n_type & N_EXT != 0 || !defined_here) {
            let def = files.get(g).and_then(|f| f.symbols.get(i)).ok_or("a global names no symbol")?;
            return address(g, def).map(Resolved::Image).ok_or_else(|| format!("{} is in no section", sym.name));
        }
        if defined_here {
            return address(o, sym).map(Resolved::Image).ok_or_else(|| format!("{} is in no section", sym.name));
        }
        import_of.get(sym.name).map(|&i| Resolved::Import(i)).ok_or_else(|| format!("{} is never defined", sym.name))
    };

    let mut fixups: Vec<(u64, u32, u32, i64)> = Vec::new();
    for (o, file) in files.iter().enumerate() {
        for s in &file.sections {
            let mut addend = 0i64;
            for r in relocations(file.bytes, s)? {
                if r.kind == ARM64_RELOC_ADDEND {
                    // A signed 24-bit addend, in place of a symbol.
                    addend = (((r.symbol as u32) << 8).cast_signed() >> 8).into();
                    continue;
                }
                let a = std::mem::take(&mut addend);
                if !r.external {
                    return Err("a relocation against a section rather than a symbol".into());
                }
                let place = s.placed + u64::from(r.address);
                let target = resolve(o, r.symbol)?;
                match (r.kind, target) {
                    (ARM64_RELOC_UNSIGNED, _) if r.length == 3 => {
                        let implicit = u64::from_le_bytes(word8(&bytes, place)?).cast_signed();
                        image_slice(&mut bytes, place, 8)?.fill(0);
                        match target {
                            Resolved::Image(t) => fixups.push((place, ABS64, SELF, t.cast_signed() + implicit + a)),
                            Resolved::Import(i) => fixups.push((place, ABS64, i, implicit + a)),
                        }
                    }
                    (ARM64_RELOC_BRANCH26, Resolved::Image(t)) => {
                        let delta = t.wrapping_add_signed(a).wrapping_sub(place).cast_signed();
                        if !(-(1i64 << 27)..(1i64 << 27)).contains(&delta) {
                            return Err("a branch inside the image is out of reach".into());
                        }
                        let words = (delta >> 2).cast_unsigned();
                        patch(&mut bytes, place, |insn| (insn & 0xfc00_0000) | (words as u32 & 0x03ff_ffff))?;
                    }
                    (ARM64_RELOC_BRANCH26, Resolved::Import(i)) => fixups.push((place, BRANCH26, i, a)),
                    (ARM64_RELOC_PAGE21, Resolved::Image(t)) => {
                        let pages = (t.wrapping_add_signed(a) >> 12).wrapping_sub(place >> 12).cast_signed();
                        if !(-(1i64 << 20)..(1i64 << 20)).contains(&pages) {
                            return Err("a page reference inside the image is out of reach".into());
                        }
                        let p = pages.cast_unsigned() as u32;
                        patch(&mut bytes, place, |insn| (insn & 0x9f00_001f) | ((p & 3) << 29) | (((p >> 2) & 0x7_ffff) << 5))?;
                    }
                    (ARM64_RELOC_PAGE21, Resolved::Import(i)) if imports.get(i as usize).is_some_and(|m| m.host.is_some()) => {
                        fixups.push((place, PAGE21, i, a));
                    }
                    (ARM64_RELOC_PAGEOFF12, _) => {
                        // The host's Mach header is page-aligned, so the low
                        // twelve bits of a host address are its offset's.
                        let low = match target {
                            Resolved::Image(t) => t.wrapping_add_signed(a),
                            Resolved::Import(i) => imports
                                .get(i as usize)
                                .and_then(|m| m.host)
                                .ok_or("a page offset into the C library")?
                                .wrapping_add_signed(a),
                        } & 0xfff;
                        let mut failed = false;
                        patch(&mut bytes, place, |insn| {
                            pageoff12(insn, low as u32).unwrap_or_else(|| {
                                failed = true;
                                insn
                            })
                        })?;
                        if failed {
                            return Err("a misaligned page offset".into());
                        }
                    }
                    (kind, _) => return Err(format!("a relocation of type {kind}")),
                }
            }
        }
    }

    let entry = globals
        .get("_main")
        .and_then(|&(g, i, _)| address(g, files.get(g)?.symbols.get(i)?))
        .filter(|&e| e < text)
        .ok_or("the objects define no main")?;

    let mut out = Vec::with_capacity(bytes.len() + 64 * fixups.len());
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&VERSION.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(host_key.as_bytes());
    for v in [text, const_at, consts, data_at, data, total, entry, stubs] {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out.extend_from_slice(&(imports.len() as u32).to_le_bytes());
    out.extend_from_slice(&u32::try_from(fixups.len()).map_err(|_| "too many fixups")?.to_le_bytes());
    for import in &imports {
        out.extend_from_slice(&(if import.host.is_some() { HOST } else { NAMED }).to_le_bytes());
        out.extend_from_slice(&(import.name.len() as u32).to_le_bytes());
        out.extend_from_slice(&import.host.unwrap_or(0).to_le_bytes());
        out.extend_from_slice(import.name.as_bytes());
    }
    for (at, kind, import, addend) in &fixups {
        out.extend_from_slice(&at.to_le_bytes());
        out.extend_from_slice(&kind.to_le_bytes());
        out.extend_from_slice(&import.to_le_bytes());
        out.extend_from_slice(&addend.to_le_bytes());
    }
    let (code, rest) = bytes.split_at(usize::try_from(text).map_err(|_| "the image is too large")?);
    out.extend_from_slice(code);
    out.extend_from_slice(rest.get((const_at - text) as usize..(const_at - text + consts) as usize).unwrap_or_default());
    out.extend_from_slice(rest.get((data_at - text) as usize..).unwrap_or_default());
    Ok(out)
}

/// Places every section of `area`, in object order, from `cursor` on.
fn place(files: &mut [MachO<'_>], area: Area, cursor: &mut u64) -> Result<(), String> {
    for file in files {
        for s in file.sections.iter_mut().filter(|s| s.area == area) {
            if s.align > 15 {
                return Err("a section asks for more than a page's alignment".into());
            }
            *cursor = cursor.next_multiple_of(1 << s.align);
            s.placed = *cursor;
            *cursor += s.size;
        }
    }
    Ok(())
}

fn image_slice(bytes: &mut [u8], at: u64, len: usize) -> Result<&mut [u8], String> {
    let at = usize::try_from(at).map_err(|_| "an offset past the image")?;
    at.checked_add(len).and_then(|end| bytes.get_mut(at..end)).ok_or_else(|| "an offset past the image".to_string())
}

fn word8(bytes: &[u8], at: u64) -> Result<[u8; 8], String> {
    let at = usize::try_from(at).map_err(|_| "an offset past the image")?;
    let mut w = [0u8; 8];
    w.copy_from_slice(at.checked_add(8).and_then(|end| bytes.get(at..end)).ok_or("an offset past the image")?);
    Ok(w)
}

/// Rewrites the instruction at `at` with `f` of what it was.
fn patch(bytes: &mut [u8], at: u64, f: impl FnOnce(u32) -> u32) -> Result<(), String> {
    let field = image_slice(bytes, at, 4)?;
    let mut w = [0u8; 4];
    w.copy_from_slice(field);
    field.copy_from_slice(&f(u32::from_le_bytes(w)).to_le_bytes());
    Ok(())
}

/// An `add` or a load or store's twelve-bit offset, scaled by its access size.
/// `None` when the offset isn't a multiple of it.
fn pageoff12(insn: u32, low: u32) -> Option<u32> {
    let imm = if insn & 0x3b00_0000 == 0x3900_0000 {
        // A load or store with an unsigned offset: scaled, and a 128-bit
        // vector one by sixteen.
        let scale = if insn & 0x0480_0000 == 0x0480_0000 { 4 } else { insn >> 30 };
        low.is_multiple_of(1 << scale).then_some(low >> scale)?
    } else {
        low
    };
    Some((insn & !(0xfff << 10)) | (imm << 10))
}

/// One relocation record.
struct Relocation {
    address: u32,
    symbol: usize,
    length: u32,
    external: bool,
    kind: u32,
}

fn relocations(bytes: &[u8], s: &Section) -> Result<Vec<Relocation>, String> {
    let mut out = Vec::with_capacity(s.nreloc as usize);
    for i in 0..s.nreloc as usize {
        let at = s.reloff as usize + 8 * i;
        let (address, info) = (u32_at(bytes, at)?, u32_at(bytes, at + 4)?);
        out.push(Relocation {
            address,
            symbol: (info & 0x00ff_ffff) as usize,
            length: (info >> 25) & 3,
            external: (info >> 27) & 1 == 1,
            kind: info >> 28,
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_page_offset_is_scaled_by_its_access() {
        // `add x0, x0, #0` takes the offset as it is.
        assert_eq!(pageoff12(0x9100_0000, 0x123), Some(0x9100_0000 | (0x123 << 10)));
        // `ldr x0, [x0]` takes it in eight-byte steps.
        assert_eq!(pageoff12(0xf940_0000, 0x128), Some(0xf940_0000 | (0x25 << 10)));
        assert_eq!(pageoff12(0xf940_0000, 0x124), None);
    }

    #[test]
    fn host_symbols_survive_their_text() {
        let mut map = HashMap::default();
        map.insert("_buri_rt_alloc".to_string(), 0x4000u64);
        map.insert("_buri_rt_shared_mask".to_string(), 0x8_0010u64);
        let symbols = HostSymbols(map);
        let back = HostSymbols::from_text(&symbols.to_text()).unwrap();
        assert_eq!(back.get("_buri_rt_alloc"), Some(0x4000));
        assert_eq!(back.get("_buri_rt_shared_mask"), Some(0x8_0010));
    }
}
