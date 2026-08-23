//! A Mach-O (arm64) relocatable object *writer*.
//!
//! `macho.rs` reads what `clang -c` produced; this writes what `ld` will
//! accept. It is the last step of the copy-and-patch backend when the output is
//! a program rather than a mapping in this process: the same machine code the
//! JIT would patch in memory is instead handed to the system linker, which
//! resolves the runtime's symbols and builds the executable.
//!
//! Nothing here is a general Mach-O emitter. It writes one `LC_SEGMENT_64`
//! holding every section, `LC_SYMTAB`, `LC_DYSYMTAB` and `LC_BUILD_VERSION`,
//! and it writes exactly the four relocation kinds an arm64 code generator
//! needs. Anything it cannot express is an `Err`, never a guess.
//!
//! # Why each of the four load commands is there
//!
//! An `MH_OBJECT` puts all of its sections in a single unnamed segment — the
//! per-section `segname` is what says `__TEXT` or `__DATA`, and the segment's
//! own name is empty. That is one command rather than three.
//!
//! `LC_DYSYMTAB` is not optional even though nothing here is dynamic. `ld`
//! reads the symbol table through it: the table must be sorted into local,
//! defined-external and undefined runs, and `LC_DYSYMTAB` is what states where
//! each run begins. So this file sorts the symbols itself and rewrites the
//! caller's relocation indices, which are indices into the slice the caller
//! passed and mean nothing after the sort.
//!
//! `LC_BUILD_VERSION` earns its place by silencing a linker warning: without a
//! platform, `ld` says "object file was built for newer macOS version than
//! being linked" or, on newer tools, that the object has no platform at all.
//!
//! `MH_SUBSECTIONS_VIA_SYMBOLS` promises the linker that every symbol starts an
//! independently movable block, which is what lets dead-strip work. It holds
//! here because the caller emits one symbol per function and never lets code
//! fall through from one symbol's range into the next.
//!
//! # Addends
//!
//! Mach-O has no relocation field for an addend, so the addend lives in one of
//! two places depending on the kind. For `ARM64_RELOC_UNSIGNED` it is simply
//! part of the eight bytes being relocated, so this file adds it into the
//! section's own bytes and the linker adds the symbol address on top. For the
//! pc-relative instruction kinds there is nowhere in the instruction to put it —
//! a `bl`'s 26 bits are the whole displacement — so Apple's assembler emits a
//! separate `ARM64_RELOC_ADDEND` record immediately *before* the relocation it
//! modifies, carrying the value in `r_symbolnum` with `r_extern = 0`. That is a
//! 24-bit unsigned field, which is why a negative or oversized addend on those
//! kinds is refused rather than truncated.
//!
//! # Determinism
//!
//! The same inputs must produce the same bytes: an object that differs run to
//! run defeats the build cache that decides whether a link is needed at all. So
//! every ordering in the output path comes from an index or a stable sort, and
//! no map is iterated anywhere.

#![allow(
    clippy::arithmetic_side_effects,
    reason = "this file's arithmetic is file-offset and address bookkeeping \
              over a buffer it is itself building: every operand is a length \
              or an offset already reached in a `Vec` held in memory, and \
              every result is the next such position. The header sizes are \
              constants, the section count is bounded by the caller's slice, \
              and the two places a value could come from outside — a caller's \
              relocation offset and a caller's addend — are range-checked \
              against the section length and the 24-bit field before any \
              arithmetic touches them"
)]

// mach_header_64.
const MH_MAGIC_64: u32 = 0xfeed_facf;
const CPU_TYPE_ARM64: u32 = 0x0100_000c;
const CPU_SUBTYPE_ARM64_ALL: u32 = 0;
const MH_OBJECT: u32 = 1;

/// The section-type nibble that says "a size, and no bytes on disk".
const S_ZEROFILL: u32 = 0x1;

/// `S_ATTR_PURE_INSTRUCTIONS | S_ATTR_SOME_INSTRUCTIONS`, for a `__text`.
pub const CODE_ATTRIBUTES: u32 = 0x8000_0000 | 0x0000_0400;
const MH_SUBSECTIONS_VIA_SYMBOLS: u32 = 0x2000;

// Load commands.
const LC_SYMTAB: u32 = 0x02;
const LC_DYSYMTAB: u32 = 0x0b;
const LC_SEGMENT_64: u32 = 0x19;
const LC_BUILD_VERSION: u32 = 0x32;

const PLATFORM_MACOS: u32 = 1;
/// 11.0.0, packed `xxxx.yy.zz` as in `mach-o/loader.h`.
const VERSION_11_0: u32 = 11 << 16;

// Structure sizes, in bytes.
const HEADER_SIZE: u64 = 32;
const SEGMENT_CMD_SIZE: u64 = 72;
const SECTION_SIZE: u64 = 80;
const BUILD_VERSION_CMD_SIZE: u64 = 24;
const SYMTAB_CMD_SIZE: u64 = 24;
const DYSYMTAB_CMD_SIZE: u64 = 80;
const NLIST_SIZE: u64 = 16;
const RELOC_SIZE: u64 = 8;

// nlist_64 `n_type` bits (`mach-o/nlist.h`).
const N_EXT: u8 = 0x01;
const N_SECT: u8 = 0x0e;

// arm64 relocation types (`mach-o/arm64/reloc.h`).
const ARM64_RELOC_UNSIGNED: u32 = 0;
const ARM64_RELOC_BRANCH26: u32 = 2;
const ARM64_RELOC_PAGE21: u32 = 3;
const ARM64_RELOC_PAGEOFF12: u32 = 4;
const ARM64_RELOC_ADDEND: u32 = 10;

/// `r_symbolnum` is 24 bits, and an `ARM64_RELOC_ADDEND` puts the addend there.
const MAX_ADDEND: i64 = 0x00ff_ffff;

/// The widest alignment a section may ask for. `2^15` is far past anything a
/// code generator needs and keeps the shift below `u64`'s width.
const MAX_ALIGN_LOG2: u32 = 15;

/// One section of the object.
pub struct Section {
    /// `__text`, `__const`, `__data`.
    pub name: &'static str,
    /// `__TEXT`, `__DATA_CONST`, `__DATA`.
    pub segment: &'static str,
    /// log2 of the required alignment.
    pub align: u32,
    /// How many bytes of zeros the section is, when it is a zero-fill one.
    ///
    /// A zero-fill section has a size and no file bytes: `data` must be empty
    /// and `S_ZEROFILL` goes in the flags' type nibble. It is what a `__bss`
    /// is, and it is how a sixty-four-megabyte block costs nothing in the
    /// object or in the artifact. Mach-O requires every zero-fill section to
    /// come **after** every other one in the segment, which `write` checks.
    pub zerofill: u64,
    /// Section attributes (`S_ATTR_PURE_INSTRUCTIONS | S_ATTR_SOME_INSTRUCTIONS`
    /// for code, 0 for data).
    pub attributes: u32,
    pub data: Vec<u8>,
}

/// A symbol this object defines or references.
pub struct Symbol {
    /// Already mangled, *without* the leading underscore — this file adds it.
    pub name: String,
    /// `None` for an undefined (imported) symbol.
    pub defined: Option<Definition>,
    /// Whether the symbol is visible outside the object (`N_EXT`).
    pub global: bool,
}

pub struct Definition {
    pub section: usize,
    pub offset: u64,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum RelKind {
    /// `ARM64_RELOC_BRANCH26` on a `b`/`bl`: the low 26 bits are a signed word
    /// displacement to the symbol.
    Branch26,
    /// `ARM64_RELOC_UNSIGNED`, `r_length = 3`: an eight-byte absolute address.
    Abs64,
    /// `ARM64_RELOC_PAGE21` on an `adrp`.
    Page21,
    /// `ARM64_RELOC_PAGEOFF12` on the `add`/`ldr` that follows a `Page21`.
    PageOff12,
}

impl RelKind {
    fn r_type(self) -> u32 {
        match self {
            Self::Branch26 => ARM64_RELOC_BRANCH26,
            Self::Abs64 => ARM64_RELOC_UNSIGNED,
            Self::Page21 => ARM64_RELOC_PAGE21,
            Self::PageOff12 => ARM64_RELOC_PAGEOFF12,
        }
    }

    /// `adrp` and `b`/`bl` are relative to the instruction; the `add` of a
    /// `PAGEOFF12` pair and an absolute pointer are not.
    fn pcrel(self) -> bool {
        matches!(self, Self::Branch26 | Self::Page21)
    }

    /// log2 of the field width: one instruction word, or one pointer.
    fn length(self) -> u32 {
        match self {
            Self::Abs64 => 3,
            _ => 2,
        }
    }

    /// The width of the relocated field, for bounds-checking the offset.
    fn width(self) -> u64 {
        1 << self.length()
    }
}

pub struct Reloc {
    pub section: usize,
    /// Byte offset of the instruction (or word) within its section.
    pub offset: u64,
    pub kind: RelKind,
    /// Index into the symbol table passed to `write`.
    pub symbol: usize,
    /// Added to the symbol's address before the field is formed. Emitted as an
    /// `ARM64_RELOC_ADDEND` record in front of the relocation for the two
    /// pc-relative kinds, and folded into the section bytes for `Abs64`.
    pub addend: i64,
}

/// A section's placement, computed once and read by both the header writer and
/// the body writer so that the two cannot disagree.
struct Placed {
    addr: u64,
    file_offset: u64,
    reloc_offset: u64,
    reloc_count: u32,
}

/// A relocation as it appears on disk, after the addend records have been
/// interleaved and the symbol indices remapped.
struct RawReloc {
    address: u32,
    symbolnum: u32,
    pcrel: bool,
    length: u32,
    external: bool,
    r_type: u32,
}

impl RawReloc {
    fn write(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.address.to_le_bytes());
        let info = (self.symbolnum & 0x00ff_ffff)
            | (u32::from(self.pcrel) << 24)
            | ((self.length & 3) << 25)
            | (u32::from(self.external) << 27)
            | ((self.r_type & 0xf) << 28);
        out.extend_from_slice(&info.to_le_bytes());
    }
}

/// The object's bytes.
pub fn write(
    sections: &[Section],
    symbols: &[Symbol],
    relocs: &[Reloc],
) -> Result<Vec<u8>, String> {
    if sections.is_empty() {
        return Err("a Mach-O object needs at least one section".into());
    }
    if sections.len() > u32::MAX as usize {
        return Err("too many sections".into());
    }
    for s in sections {
        if s.align > MAX_ALIGN_LOG2 {
            return Err(format!(
                "section {} asks for 2^{} alignment; the maximum is 2^{MAX_ALIGN_LOG2}",
                s.name, s.align
            ));
        }
        if s.name.len() > 16 || s.segment.len() > 16 {
            return Err(format!(
                "section name {},{} does not fit Mach-O's 16-byte fields",
                s.segment, s.name
            ));
        }
    }

    // The addend of an `Abs64` is written into the bytes being relocated, so
    // the caller's data is copied before anything else touches it.
    let mut seen_zerofill = false;
    for s in sections {
        if s.zerofill > 0 {
            seen_zerofill = true;
            if !s.data.is_empty() {
                return Err(format!("zero-fill section {} also carries bytes", s.name));
            }
        } else if seen_zerofill {
            return Err(format!(
                "section {} follows a zero-fill one; Mach-O requires zero-fill sections last",
                s.name
            ));
        }
    }

    let mut bodies: Vec<Vec<u8>> =
        sections.iter().map(|s| s.data.clone()).collect();
    let order = sort_symbols(symbols);
    apply_abs64_addends(&mut bodies, sections, relocs)?;
    let per_section = group_relocs(sections, symbols, relocs, &order)?;

    // Addresses are a single ascending run with each section's own alignment
    // honoured; file offsets are that run shifted by one aligned base, so a
    // section's offset is congruent to its address and `ld`'s own alignment
    // check passes.
    let mut addrs = Vec::with_capacity(sections.len());
    let mut addr = 0u64;
    let mut max_align = 0u32;
    for (i, s) in sections.iter().enumerate() {
        addr = align_up(addr, s.align);
        addrs.push(addr);
        let len = if s.zerofill > 0 {
            s.zerofill
        } else {
            bodies.get(i).map_or(0, Vec::len) as u64
        };
        addr += len;
        max_align = max_align.max(s.align);
    }
    // The file image stops at the first zero-fill section: everything past it
    // has a size and an address but no bytes.
    let span = addr;
    let file_span = sections
        .iter()
        .enumerate()
        .find(|(_, s)| s.zerofill > 0)
        .and_then(|(i, _)| addrs.get(i).copied())
        .unwrap_or(span);

    let sizeofcmds = SEGMENT_CMD_SIZE
        + SECTION_SIZE * sections.len() as u64
        + BUILD_VERSION_CMD_SIZE
        + SYMTAB_CMD_SIZE
        + DYSYMTAB_CMD_SIZE;
    let data_start = align_up(HEADER_SIZE + sizeofcmds, max_align.max(3));

    let mut reloc_cursor = align_up(data_start + file_span, 2);
    let reloc_start = reloc_cursor;
    let mut placed = Vec::with_capacity(sections.len());
    for (i, a) in addrs.iter().enumerate() {
        let entries = per_section.get(i).map_or(0, Vec::len) as u64;
        let empty = bodies.get(i).is_none_or(Vec::is_empty);
        placed.push(Placed {
            addr: *a,
            // A zero-length section has no bytes on disk, and `ld` reads a
            // non-zero offset on one as a file-range error.
            file_offset: if empty { 0 } else { data_start + *a },
            reloc_offset: if entries == 0 { 0 } else { reloc_cursor },
            reloc_count: entries as u32,
        });
        reloc_cursor += entries * RELOC_SIZE;
    }

    let symtab_off = align_up(reloc_cursor, 3);
    let strtab_off = symtab_off + NLIST_SIZE * order.len() as u64;
    let (strings, str_indices) = string_table(symbols, &order);

    let (nlocal, nextdef, nundef) = symbol_groups(symbols, &order);

    let mut out: Vec<u8> = Vec::with_capacity(strtab_off as usize + strings.len());

    // -- header ---------------------------------------------------------------
    out.extend_from_slice(&MH_MAGIC_64.to_le_bytes());
    out.extend_from_slice(&CPU_TYPE_ARM64.to_le_bytes());
    out.extend_from_slice(&CPU_SUBTYPE_ARM64_ALL.to_le_bytes());
    out.extend_from_slice(&MH_OBJECT.to_le_bytes());
    out.extend_from_slice(&4u32.to_le_bytes());
    out.extend_from_slice(&(sizeofcmds as u32).to_le_bytes());
    out.extend_from_slice(&MH_SUBSECTIONS_VIA_SYMBOLS.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());

    // -- LC_SEGMENT_64 --------------------------------------------------------
    out.extend_from_slice(&LC_SEGMENT_64.to_le_bytes());
    out.extend_from_slice(
        &((SEGMENT_CMD_SIZE + SECTION_SIZE * sections.len() as u64) as u32)
            .to_le_bytes(),
    );
    // An object's one segment is unnamed; `segname` on each section is what
    // carries `__TEXT` and friends.
    out.extend_from_slice(&[0u8; 16]);
    out.extend_from_slice(&0u64.to_le_bytes());
    out.extend_from_slice(&span.to_le_bytes());
    out.extend_from_slice(&data_start.to_le_bytes());
    out.extend_from_slice(&file_span.to_le_bytes());
    // VM_PROT_READ | WRITE | EXECUTE, both max and initial: an object is not
    // mapped, and `ld` expects the permissive value here.
    out.extend_from_slice(&7u32.to_le_bytes());
    out.extend_from_slice(&7u32.to_le_bytes());
    out.extend_from_slice(&(sections.len() as u32).to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());

    for (i, s) in sections.iter().enumerate() {
        let p = placed
            .get(i)
            .ok_or_else(|| "section placement is missing".to_string())?;
        out.extend_from_slice(&name16(s.name));
        out.extend_from_slice(&name16(s.segment));
        out.extend_from_slice(&p.addr.to_le_bytes());
        let size = if s.zerofill > 0 {
            s.zerofill
        } else {
            bodies.get(i).map_or(0, Vec::len) as u64
        };
        out.extend_from_slice(&size.to_le_bytes());
        out.extend_from_slice(&(p.file_offset as u32).to_le_bytes());
        out.extend_from_slice(&s.align.to_le_bytes());
        out.extend_from_slice(&(p.reloc_offset as u32).to_le_bytes());
        out.extend_from_slice(&p.reloc_count.to_le_bytes());
        // S_REGULAR is zero, so the section type contributes nothing to an
        // ordinary section and the flags word is the caller's attributes.
        let flags = s.attributes | if s.zerofill > 0 { S_ZEROFILL } else { 0 };
        out.extend_from_slice(&flags.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
    }

    // -- LC_BUILD_VERSION -----------------------------------------------------
    out.extend_from_slice(&LC_BUILD_VERSION.to_le_bytes());
    out.extend_from_slice(&(BUILD_VERSION_CMD_SIZE as u32).to_le_bytes());
    out.extend_from_slice(&PLATFORM_MACOS.to_le_bytes());
    out.extend_from_slice(&VERSION_11_0.to_le_bytes());
    out.extend_from_slice(&VERSION_11_0.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());

    // -- LC_SYMTAB ------------------------------------------------------------
    out.extend_from_slice(&LC_SYMTAB.to_le_bytes());
    out.extend_from_slice(&(SYMTAB_CMD_SIZE as u32).to_le_bytes());
    out.extend_from_slice(&(symtab_off as u32).to_le_bytes());
    out.extend_from_slice(&(order.len() as u32).to_le_bytes());
    out.extend_from_slice(&(strtab_off as u32).to_le_bytes());
    out.extend_from_slice(&(strings.len() as u32).to_le_bytes());

    // -- LC_DYSYMTAB ----------------------------------------------------------
    out.extend_from_slice(&LC_DYSYMTAB.to_le_bytes());
    out.extend_from_slice(&(DYSYMTAB_CMD_SIZE as u32).to_le_bytes());
    for v in [0, nlocal, nlocal, nextdef, nlocal + nextdef, nundef] {
        out.extend_from_slice(&v.to_le_bytes());
    }
    // No table of contents, module table, reference table, indirect symbols or
    // external/local relocation streams: an object keeps its relocations in the
    // section headers, which is where `reloff`/`nreloc` above point.
    for _ in 0..12 {
        out.extend_from_slice(&0u32.to_le_bytes());
    }

    // -- section bodies -------------------------------------------------------
    pad_to(&mut out, data_start);
    for (i, body) in bodies.iter().enumerate() {
        if body.is_empty() {
            continue;
        }
        let p = placed
            .get(i)
            .ok_or_else(|| "section placement is missing".to_string())?;
        pad_to(&mut out, p.file_offset);
        out.extend_from_slice(body);
    }

    // -- relocations ----------------------------------------------------------
    pad_to(&mut out, reloc_start);
    for entries in &per_section {
        for r in entries {
            r.write(&mut out);
        }
    }

    // -- symbol table ---------------------------------------------------------
    pad_to(&mut out, symtab_off);
    for (slot, sym_index) in order.iter().enumerate() {
        let sym = symbols
            .get(*sym_index)
            .ok_or_else(|| "symbol index is out of range".to_string())?;
        let strx = str_indices
            .get(slot)
            .copied()
            .ok_or_else(|| "string index is missing".to_string())?;
        out.extend_from_slice(&strx.to_le_bytes());
        match &sym.defined {
            Some(d) => {
                let n_type = N_SECT | if sym.global { N_EXT } else { 0 };
                let addr = placed
                    .get(d.section)
                    .ok_or_else(|| {
                        format!("symbol {} names section {}", sym.name, d.section)
                    })?
                    .addr;
                out.push(n_type);
                // `n_sect` is 1-based, and the range check above is what makes
                // the increment meaningful.
                out.push((d.section + 1) as u8);
                out.extend_from_slice(&0u16.to_le_bytes());
                out.extend_from_slice(&(addr + d.offset).to_le_bytes());
            }
            None => {
                // An undefined symbol is external whatever the caller said:
                // `N_UNDF` with no `N_EXT` is a common symbol, which is a
                // different thing entirely.
                out.push(N_EXT);
                out.push(0);
                out.extend_from_slice(&0u16.to_le_bytes());
                out.extend_from_slice(&0u64.to_le_bytes());
            }
        }
    }

    // -- string table ---------------------------------------------------------
    pad_to(&mut out, strtab_off);
    out.extend_from_slice(&strings);

    Ok(out)
}

/// The symbol table's order: locals, then defined externals, then undefined.
///
/// `ld` reads the three runs out of `LC_DYSYMTAB` rather than scanning, so this
/// order is a format requirement and not a convenience. The sort is stable, so
/// symbols within a run keep the caller's order and the output stays
/// reproducible.
fn sort_symbols(symbols: &[Symbol]) -> Vec<usize> {
    let mut order: Vec<usize> = (0..symbols.len()).collect();
    order.sort_by_key(|i| match symbols.get(*i) {
        Some(s) if s.defined.is_none() => 2u8,
        Some(s) if s.global => 1,
        Some(_) => 0,
        // Unreachable through `write`, which builds the range from the slice;
        // sorting such an entry last keeps this total without a panic.
        None => 3,
    });
    order
}

fn symbol_groups(symbols: &[Symbol], order: &[usize]) -> (u32, u32, u32) {
    let mut counts = (0u32, 0u32, 0u32);
    for i in order {
        match symbols.get(*i) {
            Some(s) if s.defined.is_none() => counts.2 += 1,
            Some(s) if s.global => counts.1 += 1,
            _ => counts.0 += 1,
        }
    }
    counts
}

/// The string table, and each symbol slot's index into it.
///
/// Index 0 is a lone NUL so that a symbol with no name can point at it, which
/// is the convention every Mach-O consumer assumes.
fn string_table(symbols: &[Symbol], order: &[usize]) -> (Vec<u8>, Vec<u32>) {
    let mut strings = vec![0u8];
    let mut indices = Vec::with_capacity(order.len());
    for i in order {
        indices.push(strings.len() as u32);
        if let Some(s) = symbols.get(*i) {
            // C linkage on Darwin prefixes every symbol with an underscore, and
            // the caller's names are the unprefixed ones.
            strings.push(b'_');
            strings.extend_from_slice(s.name.as_bytes());
        }
        strings.push(0);
    }
    (strings, indices)
}

/// Folds every `Abs64` addend into the bytes it applies to.
///
/// `ARM64_RELOC_UNSIGNED` has the linker *add* the symbol's address to the
/// field's existing contents, so an addend needs no record of its own — but it
/// does need the caller's bytes to be modified, which is why `write` copies
/// them first.
fn apply_abs64_addends(
    bodies: &mut [Vec<u8>],
    sections: &[Section],
    relocs: &[Reloc],
) -> Result<(), String> {
    for r in relocs {
        if r.kind != RelKind::Abs64 || r.addend == 0 {
            continue;
        }
        let name = sections.get(r.section).map_or("?", |s| s.name);
        let body = bodies
            .get_mut(r.section)
            .ok_or_else(|| format!("relocation names section {}", r.section))?;
        let start = r.offset as usize;
        let field = body
            .get_mut(start..start + 8)
            .ok_or_else(|| {
                format!("Abs64 at {:#x} runs past section {name}", r.offset)
            })?;
        let mut bytes = [0u8; 8];
        bytes.copy_from_slice(field);
        let sum = u64::from_le_bytes(bytes).wrapping_add(r.addend as u64);
        field.copy_from_slice(&sum.to_le_bytes());
    }
    Ok(())
}

/// The on-disk relocation entries, per section, in ascending address order.
///
/// Sorting by address is what an assembler produces and what a reader of the
/// stream expects; the `ARM64_RELOC_ADDEND` records are interleaved afterwards
/// so that each stays immediately before the relocation it modifies.
fn group_relocs(
    sections: &[Section],
    symbols: &[Symbol],
    relocs: &[Reloc],
    order: &[usize],
) -> Result<Vec<Vec<RawReloc>>, String> {
    let mut slot = vec![0u32; symbols.len()];
    for (new, old) in order.iter().enumerate() {
        if let Some(s) = slot.get_mut(*old) {
            *s = new as u32;
        }
    }

    let mut sorted: Vec<usize> = (0..relocs.len()).collect();
    sorted.sort_by_key(|i| relocs.get(*i).map(|r| (r.section, r.offset)));

    let mut out: Vec<Vec<RawReloc>> =
        (0..sections.len()).map(|_| Vec::new()).collect();
    for i in sorted {
        let r = relocs
            .get(i)
            .ok_or_else(|| "relocation index is out of range".to_string())?;
        let section = sections
            .get(r.section)
            .ok_or_else(|| format!("relocation names section {}", r.section))?;
        let end = r.offset + r.kind.width();
        if end > section.data.len() as u64 {
            return Err(format!(
                "relocation at {:#x} runs past section {}",
                r.offset, section.name
            ));
        }
        if r.offset > u64::from(u32::MAX) {
            return Err(format!("relocation offset {:#x} exceeds 32 bits", r.offset));
        }
        let symbolnum = *slot
            .get(r.symbol)
            .ok_or_else(|| format!("relocation names symbol {}", r.symbol))?;

        let entries = out
            .get_mut(r.section)
            .ok_or_else(|| format!("relocation names section {}", r.section))?;

        // Every instruction kind takes its addend in a separate record — a
        // `PageOff12` is not pc-relative but has no more room in its immediate
        // than an `adrp` does — and only `Abs64` carries it in the bytes.
        if r.kind != RelKind::Abs64 && r.addend != 0 {
            if r.addend < 0 || r.addend > MAX_ADDEND {
                return Err(format!(
                    "a relocation in {} has addend {}; ARM64_RELOC_ADDEND \
                     carries an unsigned 24-bit value and nothing wider",
                    section.name, r.addend
                ));
            }
            entries.push(RawReloc {
                address: r.offset as u32,
                symbolnum: r.addend as u32,
                pcrel: false,
                length: 2,
                // The field holds a value, not a symbol index, so `r_extern`
                // must be 0 or `ld` reads the addend as a symbol number.
                external: false,
                r_type: ARM64_RELOC_ADDEND,
            });
        }

        entries.push(RawReloc {
            address: r.offset as u32,
            symbolnum,
            pcrel: r.kind.pcrel(),
            length: r.kind.length(),
            external: true,
            r_type: r.kind.r_type(),
        });
    }
    Ok(out)
}

fn align_up(value: u64, align_log2: u32) -> u64 {
    let mask = (1u64 << align_log2) - 1;
    (value + mask) & !mask
}

fn pad_to(out: &mut Vec<u8>, offset: u64) {
    while (out.len() as u64) < offset {
        out.push(0);
    }
}

/// A name in a fixed 16-byte field, NUL-padded and not NUL-terminated when it
/// fills the field exactly — `__DATA_CONST` is all sixteen bytes.
fn name16(name: &str) -> [u8; 16] {
    let mut field = [0u8; 16];
    for (slot, byte) in field.iter_mut().zip(name.as_bytes()) {
        *slot = *byte;
    }
    field
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `ret`.
    const RET: [u8; 4] = [0xc0, 0x03, 0x5f, 0xd6];
    /// `bl #0`, with the displacement left for the linker.
    const BL: [u8; 4] = [0x00, 0x00, 0x00, 0x94];

    const S_ATTR_PURE_INSTRUCTIONS: u32 = 0x8000_0000;
    const S_ATTR_SOME_INSTRUCTIONS: u32 = 0x0000_0400;

    fn text(data: Vec<u8>) -> Section {
        Section {
            name: "__text",
            segment: "__TEXT",
            align: 2,
            attributes: S_ATTR_PURE_INSTRUCTIONS | S_ATTR_SOME_INSTRUCTIONS,
            zerofill: 0,
            data,
        }
    }

    /// One function that calls an imported one and returns.
    fn tiny() -> (Vec<Section>, Vec<Symbol>, Vec<Reloc>) {
        let mut code = Vec::new();
        code.extend_from_slice(&BL);
        code.extend_from_slice(&RET);
        let sections = vec![text(code)];
        let symbols = vec![
            Symbol {
                name: "callee".into(),
                defined: None,
                global: true,
            },
            Symbol {
                name: "main_entry".into(),
                defined: Some(Definition { section: 0, offset: 0 }),
                global: true,
            },
        ];
        let relocs = vec![Reloc {
            section: 0,
            offset: 0,
            kind: RelKind::Branch26,
            symbol: 0,
            addend: 0,
        }];
        (sections, symbols, relocs)
    }

    fn u32at(bytes: &[u8], at: usize) -> u32 {
        u32::from_le_bytes([
            bytes[at],
            bytes[at + 1],
            bytes[at + 2],
            bytes[at + 3],
        ])
    }

    #[test]
    fn header_names_an_arm64_object() {
        let (s, y, r) = tiny();
        let o = write(&s, &y, &r).unwrap();
        assert_eq!(u32at(&o, 0), MH_MAGIC_64);
        assert_eq!(u32at(&o, 4), CPU_TYPE_ARM64);
        assert_eq!(u32at(&o, 12), MH_OBJECT);
        assert_eq!(u32at(&o, 16), 4, "four load commands");
        assert_eq!(u32at(&o, 24), MH_SUBSECTIONS_VIA_SYMBOLS);
        // The segment command's section count, at header + 64.
        assert_eq!(u32at(&o, 32 + 64), 1);
    }

    #[test]
    fn defined_symbols_precede_undefined_ones() {
        let (s, y, r) = tiny();
        let o = write(&s, &y, &r).unwrap();
        // LC_SYMTAB is the third command: segment, build version, symtab.
        let lc = 32 + 72 + 80 + 24;
        assert_eq!(u32at(&o, lc), LC_SYMTAB);
        let symoff = u32at(&o, lc + 8) as usize;
        let nsyms = u32at(&o, lc + 12) as usize;
        let stroff = u32at(&o, lc + 16) as usize;
        assert_eq!(nsyms, 2);

        let name_at = |i: usize| {
            let strx = u32at(&o, symoff + i * 16) as usize;
            let start = stroff + strx;
            let len = o[start..].iter().position(|b| *b == 0).unwrap();
            String::from_utf8(o[start..start + len].to_vec()).unwrap()
        };
        // The caller listed the undefined symbol first; the sort moved it last.
        assert_eq!(name_at(0), "_main_entry");
        assert_eq!(o[symoff + 4], N_SECT | N_EXT);
        assert_eq!(o[symoff + 5], 1, "n_sect is 1-based");
        assert_eq!(name_at(1), "_callee");
        assert_eq!(o[symoff + 16 + 4], N_EXT);
        assert_eq!(o[symoff + 16 + 5], 0);

        assert_eq!(o[stroff], 0, "the string table starts with a NUL");

        // LC_DYSYMTAB follows LC_SYMTAB and must describe the same split.
        let dy = lc + 24;
        assert_eq!(u32at(&o, dy), LC_DYSYMTAB);
        assert_eq!(u32at(&o, dy + 8), 0, "ilocalsym");
        assert_eq!(u32at(&o, dy + 12), 0, "nlocalsym");
        assert_eq!(u32at(&o, dy + 16), 0, "iextdefsym");
        assert_eq!(u32at(&o, dy + 20), 1, "nextdefsym");
        assert_eq!(u32at(&o, dy + 24), 1, "iundefsym");
        assert_eq!(u32at(&o, dy + 28), 1, "nundefsym");
    }

    #[test]
    fn a_branch_relocation_names_the_remapped_symbol() {
        let (s, y, r) = tiny();
        let o = write(&s, &y, &r).unwrap();
        let sect = 32 + 72;
        let reloff = u32at(&o, sect + 56) as usize;
        assert_eq!(u32at(&o, sect + 60), 1, "one relocation");
        assert_eq!(u32at(&o, reloff), 0, "r_address");
        let info = u32at(&o, reloff + 4);
        // The undefined symbol was index 0 to the caller and is index 1 after
        // the sort, which is what the relocation must carry.
        assert_eq!(info & 0x00ff_ffff, 1);
        assert_eq!((info >> 24) & 1, 1, "r_pcrel");
        assert_eq!((info >> 25) & 3, 2, "r_length");
        assert_eq!((info >> 27) & 1, 1, "r_extern");
        assert_eq!(info >> 28, ARM64_RELOC_BRANCH26, "r_type");
    }

    #[test]
    fn an_addend_record_precedes_its_relocation() {
        let sections = vec![text(vec![0; 8])];
        let symbols = vec![Symbol {
            name: "table".into(),
            defined: Some(Definition { section: 0, offset: 0 }),
            global: false,
        }];
        let relocs = vec![
            Reloc {
                section: 0,
                offset: 0,
                kind: RelKind::Page21,
                symbol: 0,
                addend: 0x30,
            },
            Reloc {
                section: 0,
                offset: 4,
                kind: RelKind::PageOff12,
                symbol: 0,
                addend: 0x30,
            },
        ];
        let o = write(&sections, &symbols, &relocs).unwrap();
        let sect = 32 + 72;
        let reloff = u32at(&o, sect + 56) as usize;
        assert_eq!(u32at(&o, sect + 60), 4, "two pairs");
        let types: Vec<u32> =
            (0..4).map(|i| u32at(&o, reloff + i * 8 + 4) >> 28).collect();
        assert_eq!(
            types,
            vec![
                ARM64_RELOC_ADDEND,
                ARM64_RELOC_PAGE21,
                ARM64_RELOC_ADDEND,
                ARM64_RELOC_PAGEOFF12
            ]
        );
        let addend = u32at(&o, reloff + 4);
        assert_eq!(addend & 0x00ff_ffff, 0x30);
        assert_eq!((addend >> 27) & 1, 0, "an addend record is not external");
    }

    #[test]
    fn an_abs64_addend_lands_in_the_section_bytes() {
        let sections = vec![Section {
            name: "__const",
            segment: "__DATA_CONST",
            align: 3,
            attributes: 0,
            zerofill: 0,
            data: vec![0; 8],
        }];
        let symbols = vec![Symbol {
            name: "base".into(),
            defined: Some(Definition { section: 0, offset: 0 }),
            global: true,
        }];
        let relocs = vec![Reloc {
            section: 0,
            offset: 0,
            kind: RelKind::Abs64,
            symbol: 0,
            addend: 0x2a,
        }];
        let o = write(&sections, &symbols, &relocs).unwrap();
        let sect = 32 + 72;
        let off = u32at(&o, sect + 48) as usize;
        assert_eq!(u32at(&o, off), 0x2a);
        let reloff = u32at(&o, sect + 56) as usize;
        assert_eq!(u32at(&o, sect + 60), 1, "no addend record for Abs64");
        let info = u32at(&o, reloff + 4);
        assert_eq!((info >> 25) & 3, 3, "r_length is 3 for eight bytes");
        assert_eq!((info >> 24) & 1, 0, "r_pcrel");
        assert_eq!(info >> 28, ARM64_RELOC_UNSIGNED);
    }

    #[test]
    fn a_negative_pc_relative_addend_is_refused() {
        let sections = vec![text(vec![0; 4])];
        let symbols = vec![Symbol {
            name: "t".into(),
            defined: Some(Definition { section: 0, offset: 0 }),
            global: true,
        }];
        let relocs = vec![Reloc {
            section: 0,
            offset: 0,
            kind: RelKind::Page21,
            symbol: 0,
            addend: -1,
        }];
        assert!(write(&sections, &symbols, &relocs).is_err());
    }

    #[test]
    fn two_writes_agree_byte_for_byte() {
        let (s, y, r) = tiny();
        assert_eq!(write(&s, &y, &r).unwrap(), write(&s, &y, &r).unwrap());
    }
}
