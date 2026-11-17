//! The members of the baked archives a link needs, worked out from their
//! symbols (`design/PERFORMANCE.md` §6.146).
//!
//! mold reads every member of every archive it is given, whether the link
//! needs it or not. A Linux link is handed the runtime archive, musl's
//! `libc.a` and the baked `libunwind.a`, and a small program needs about a
//! fifth of their members. This file finds that fifth, so that `build::link`
//! can hand mold archives of only those members.
//!
//! Two halves share this file's reading of archives and objects:
//!
//! * `cli/build.rs` writes the index, once per runtime build. It reads the
//!   archives' symbol tables and writes which members each member needs, and
//!   which members define each symbol, laid out as [`Index`] says.
//! * [`Index::keep`] runs at link time. Given the symbols the program's own
//!   objects need, it walks that table to every member the linker could load.
//!
//! The answer is a superset of what the linker loads. Every member defining a
//! symbol that something kept needs is kept, so whichever one the linker
//! would pick is there, and the linked bytes are the same.
//!
//! Everything here reads ELF64 little-endian objects in GNU archives, which is
//! what a Linux runtime build writes. Anything else is `None`, and the link
//! hands over the whole archives as before.

#![allow(
    clippy::arithmetic_side_effects,
    reason = "offsets and counts in bytes already in memory: every read is \
              bounds-checked by `get` before it is used, and the hash wraps \
              on purpose"
)]

/// The archives the index covers, in the order its member numbers run.
pub const ARCHIVES: usize = 3;

/// The index's first word.
pub const MAGIC: u32 = u32::from_le_bytes(*b"BAM2");

/// How many of a hash's top bits pick its bucket in the index's last table.
pub const BUCKET_BITS: u32 = 12;

/// The bucket of a symbol's [`hash`]: its top [`BUCKET_BITS`] bits.
pub fn bucket(hash: u64) -> usize {
    usize::try_from(hash >> (64 - BUCKET_BITS)).unwrap_or(0)
}

/// The bytes at `at..at + len`, or `None` past the end.
fn span(bytes: &[u8], at: usize, len: usize) -> Option<&[u8]> {
    bytes.get(at..at.checked_add(len)?)
}

fn u16_at(bytes: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(span(bytes, at, 2)?.try_into().ok()?))
}

fn u32_at(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(span(bytes, at, 4)?.try_into().ok()?))
}

fn u64_at(bytes: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_le_bytes(span(bytes, at, 8)?.try_into().ok()?))
}

fn usize_at(bytes: &[u8], at: usize) -> Option<usize> {
    usize::try_from(u64_at(bytes, at)?).ok()
}

/// A symbol's name as the index stores it: FNV-1a, 64 bits.
///
/// Two names with one hash only make [`Index::keep`] keep more members, never
/// fewer, so a collision costs a little speed and never a link.
pub fn hash(name: &[u8]) -> u64 {
    name.iter().fold(0xcbf2_9ce4_8422_2325, |h, &b| {
        (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// A GNU archive's members, each as the offset of its contents in `archive`
/// and the contents. The symbol index and the long-name table are left out.
pub fn members(archive: &[u8]) -> Option<Vec<(usize, &[u8])>> {
    archive.starts_with(b"!<arch>\n").then_some(())?;
    let mut at = 8;
    let mut out = Vec::new();
    while at < archive.len() {
        let header = span(archive, at, 60)?;
        let size: usize = std::str::from_utf8(header.get(48..58)?)
            .ok()?
            .trim()
            .parse()
            .ok()?;
        let contents = at + 60;
        let body = span(archive, contents, size)?;
        let special = [&b"/ "[..], b"// ", b"/SYM64/"]
            .iter()
            .any(|name| header.starts_with(name));
        if !special {
            out.push((contents, body));
        }
        // Each member starts on an even offset.
        at = contents + size + (size & 1);
    }
    Some(out)
}

/// Calls `f(name, defined)` for each global and weak symbol of an ELF64
/// little-endian object, or answers `None` when `object` is not one.
pub fn symbols<'a>(object: &'a [u8], mut f: impl FnMut(&'a [u8], bool)) -> Option<()> {
    (object.get(..6)? == b"\x7fELF\x02\x01").then_some(())?;
    let sections = usize_at(object, 0x28)?;
    let size = usize::from(u16_at(object, 0x3a)?);
    let count = usize::from(u16_at(object, 0x3c)?);
    let section = |i: usize| span(object, sections.checked_add(i.checked_mul(size)?)?, 64);
    let contents = |header: &[u8]| span(object, usize_at(header, 0x18)?, usize_at(header, 0x20)?);
    for i in 0..count {
        let header = section(i)?;
        // `SHT_SYMTAB`.
        if u32_at(header, 4)? != 2 {
            continue;
        }
        let names = contents(section(usize::try_from(u32_at(header, 0x28)?).ok()?)?)?;
        for symbol in contents(header)?.as_chunks::<24>().0 {
            // `STB_GLOBAL`, `STB_WEAK` and `STB_GNU_UNIQUE`.
            let binding = symbol.get(4)? / 16;
            if !matches!(binding, 1 | 2 | 10) {
                continue;
            }
            // `SHN_UNDEF` is section 0.
            let defined = u16_at(symbol, 6)? != 0;
            let name = names.get(usize::try_from(u32_at(symbol, 0)?).ok()?..)?;
            let name = name.get(..name.iter().position(|&b| b == 0)?)?;
            f(name, defined);
        }
    }
    Some(())
}

/// Pushes the hash of each symbol `object` needs and doesn't define onto
/// `out`, or answers `None` when `object` is not an ELF64 little-endian
/// object.
pub fn undefined(object: &[u8], out: &mut Vec<u64>) -> Option<()> {
    symbols(object, |name, defined| {
        if !defined {
            out.push(hash(name));
        }
    })
}

/// The index `cli/build.rs` writes, read in place.
///
/// Laid out as little-endian `u32`s: [`MAGIC`], then each table as its count
/// and its rows. The members (archive, offset, length); where each member's
/// needs start in the next table, one more row than there are members; the
/// needs, as member numbers; the roots, which every link keeps; the defined
/// symbols (hash low, hash high, member), sorted; and where each [`bucket`]'s
/// rows of the defined symbols start, one more row than there are buckets.
///
/// A lookup searches only its bucket's rows, two or three of them, rather
/// than all nine thousand, which took a link about 85 k instructions.
pub struct Index<'a> {
    members: &'a [u8],
    starts: &'a [u8],
    needs: &'a [u8],
    roots: &'a [u8],
    defined: &'a [u8],
    buckets: &'a [u8],
}

impl<'a> Index<'a> {
    /// `None` for an empty index, which a toolchain with no musl sysroot has,
    /// and for one that isn't this shape.
    pub fn read(bytes: &'a [u8]) -> Option<Index<'a>> {
        (u32_at(bytes, 0)? == MAGIC).then_some(())?;
        let mut at = 4;
        let mut table = |width: usize| -> Option<&'a [u8]> {
            let count = usize::try_from(u32_at(bytes, at)?).ok()?;
            let rows = span(bytes, at + 4, count.checked_mul(width * 4)?)?;
            at += 4 + rows.len();
            Some(rows)
        };
        let index = Index {
            members: table(3)?,
            starts: table(1)?,
            needs: table(1)?,
            roots: table(1)?,
            defined: table(3)?,
            buckets: table(1)?,
        };
        (index.starts.len() / 4 == index.members() + 1).then_some(index)
    }

    /// How many members the archives hold between them.
    pub fn members(&self) -> usize {
        self.members.len() / 12
    }

    /// Member `m`'s archive, and the offset and length of its contents there.
    pub fn member(&self, m: usize) -> Option<(usize, usize, usize)> {
        let word = |i: usize| Some(u32_at(self.members, (m * 3 + i) * 4)? as usize);
        Some((word(0)?, word(1)?, word(2)?))
    }

    /// Every member the linker could load for a program whose own objects
    /// need the symbols hashed in `needs`, as one flag per member.
    pub fn keep(&self, needs: impl IntoIterator<Item = u64>) -> Vec<bool> {
        let mut kept = vec![false; self.members()];
        let mut todo: Vec<u32> = self
            .roots
            .as_chunks::<4>()
            .0
            .iter()
            .map(|w| u32::from_le_bytes(*w))
            .collect();
        let start = |b: usize| u32_at(self.buckets, b * 4).map_or(0, |r| r as usize);
        let hash_at = |i: usize| -> Option<u64> {
            let low = u32_at(self.defined, i * 12)?;
            let high = u32_at(self.defined, i * 12 + 4)?;
            Some(u64::from(low) | (u64::from(high) << 32))
        };
        for need in needs {
            // The first row of `need`'s bucket whose hash is not below it.
            let b = bucket(need);
            let (mut low, mut high) = (start(b), start(b + 1));
            while low < high {
                let mid = low + (high - low) / 2;
                if hash_at(mid).is_some_and(|h| h < need) {
                    low = mid + 1;
                } else {
                    high = mid;
                }
            }
            while hash_at(low) == Some(need) {
                todo.extend(u32_at(self.defined, low * 12 + 8));
                low += 1;
            }
        }
        while let Some(m) = todo.pop() {
            // A member already walked. The index names no member past the
            // end, and one would count as walked.
            let walked = kept
                .get_mut(m as usize)
                .is_none_or(|slot| std::mem::replace(slot, true));
            if walked {
                continue;
            }
            let start = u32_at(self.starts, m as usize * 4).unwrap_or(0) as usize;
            let end = u32_at(self.starts, m as usize * 4 + 4).unwrap_or(0) as usize;
            todo.extend((start..end).filter_map(|i| u32_at(self.needs, i * 4)));
        }
        kept
    }
}
