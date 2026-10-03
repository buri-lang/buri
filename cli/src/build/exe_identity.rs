//! Which build of `buri` is running, read off the executable's header.
//!
//! The cache key folds the toolchain's identity in ([`super::cache`]), and the
//! identity has to tell two builds of one version apart: a rebuilt `0.3.0` is a
//! different compiler. Hashing the whole binary does that, and it costs a
//! SHA-256 pass over tens of megabytes in every process — a fifth of a second
//! in a release build, a third in a development one, paid by every `buri` run
//! that opens a cache.
//!
//! The linker already wrote that fact down. ld64 puts an `LC_UUID` load command
//! in every Mach-O it links, and `--build-id` puts an `NT_GNU_BUILD_ID` note in
//! an ELF executable. Both are digests of the linked output, so a binary built
//! from different code carries a different one, and both sit in the first few
//! kilobytes of the file. `cli/build.rs` asks for the build id on Linux, where
//! not every linker writes one by default.
//!
//! Where neither is present — a fat Mach-O, a linker told to leave it out —
//! the identity is the SHA-256 of the file after all, remembered on disk under
//! the binary's path, size and modification time so that only the first
//! process after a rebuild pays for it.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use super::sha256::{hash_bytes, Sha256};

/// The most of a header this reads in one piece. A Mach-O's load commands
/// and an ELF's program headers and notes are a few kilobytes; a field that
/// claims more than this is not a header worth believing.
const MOST: u64 = 1 << 20;

const LC_UUID: u32 = 0x1b;
const PT_NOTE: u32 = 4;
const NT_GNU_BUILD_ID: u32 = 3;

/// The identity of the executable at `path`: its linker-written id where it
/// has one, the hash of its bytes otherwise. `None` only when the file cannot
/// be read at all.
pub fn of(path: &Path) -> Option<String> {
    linker_identity(path).or_else(|| hashed(path, cache_dir().as_deref()))
}

/// The id the linker wrote into the header: `macho-uuid:<hex>` or
/// `elf-build-id:<hex>`.
pub fn linker_identity(path: &Path) -> Option<String> {
    let mut file = File::open(path).ok()?;
    let mut head = [0u8; 64];
    read_at(&mut file, 0, &mut head)?;
    if let Some(uuid) = macho_uuid(&mut file, &head) {
        return Some(format!("macho-uuid:{}", hex(&uuid)));
    }
    elf_build_id(&mut file, &head).map(|id| format!("elf-build-id:{}", hex(&id)))
}

#[derive(Clone, Copy)]
enum Endian {
    Little,
    Big,
}

/// `len` bytes of `bytes` from `at`, or `None` past the end.
fn bytes_at(bytes: &[u8], at: usize, len: usize) -> Option<&[u8]> {
    bytes.get(at..at.checked_add(len)?)
}

fn uint_at(bytes: &[u8], at: usize, len: usize, endian: Endian) -> Option<u64> {
    let raw = bytes_at(bytes, at, len)?;
    let fold = |acc: u64, &b: &u8| acc.wrapping_shl(8) | u64::from(b);
    Some(match endian {
        Endian::Big => raw.iter().fold(0, fold),
        Endian::Little => raw.iter().rev().fold(0, fold),
    })
}

fn u32_at(bytes: &[u8], at: usize, endian: Endian) -> Option<u32> {
    u32::try_from(uint_at(bytes, at, 4, endian)?).ok()
}

/// Fills `out` from `offset`, or `None` if the file is shorter.
fn read_at(file: &mut File, offset: u64, out: &mut [u8]) -> Option<()> {
    file.seek(SeekFrom::Start(offset)).ok()?;
    file.read_exact(out).ok()
}

/// `len` bytes from `offset`, refusing a length no header has.
fn read_vec(file: &mut File, offset: u64, len: u64) -> Option<Vec<u8>> {
    if len > MOST {
        return None;
    }
    let mut out = vec![0; usize::try_from(len).ok()?];
    read_at(file, offset, &mut out)?;
    Some(out)
}

/// A thin Mach-O's `LC_UUID`. The header is followed directly by its load
/// commands, each led by its kind and its size.
fn macho_uuid(file: &mut File, head: &[u8]) -> Option<Vec<u8>> {
    let (endian, header) = match u32_at(head, 0, Endian::Little)? {
        0xfeed_facf => (Endian::Little, 32),
        0xfeed_face => (Endian::Little, 28),
        0xcffa_edfe => (Endian::Big, 32),
        0xcefa_edfe => (Endian::Big, 28),
        _ => return None,
    };
    let count = u32_at(head, 16, endian)?;
    let size = u32_at(head, 20, endian)?;
    let commands = read_vec(file, header, u64::from(size))?;
    let mut at = 0usize;
    for _ in 0..count {
        let kind = u32_at(&commands, at, endian)?;
        if kind == LC_UUID {
            return bytes_at(&commands, at.checked_add(8)?, 16).map(<[u8]>::to_vec);
        }
        let step = usize::try_from(u32_at(&commands, at.checked_add(4)?, endian)?).ok()?;
        if step == 0 {
            return None;
        }
        at = at.checked_add(step)?;
    }
    None
}

/// An ELF's `NT_GNU_BUILD_ID`, found through the program headers' `PT_NOTE`
/// segments, which is where a stripped executable still has it.
fn elf_build_id(file: &mut File, head: &[u8]) -> Option<Vec<u8>> {
    if bytes_at(head, 0, 4)? != b"\x7fELF" {
        return None;
    }
    let wide = match head.get(4)? {
        1 => false,
        2 => true,
        _ => return None,
    };
    let endian = match head.get(5)? {
        1 => Endian::Little,
        2 => Endian::Big,
        _ => return None,
    };
    let (table, entry, count) = if wide {
        (uint_at(head, 32, 8, endian)?, uint_at(head, 54, 2, endian)?, uint_at(head, 56, 2, endian)?)
    } else {
        (uint_at(head, 28, 4, endian)?, uint_at(head, 42, 2, endian)?, uint_at(head, 44, 2, endian)?)
    };
    let headers = read_vec(file, table, entry.checked_mul(count)?)?;
    for segment in headers.chunks_exact(usize::try_from(entry).ok()?.max(1)) {
        if u32_at(segment, 0, endian)? != PT_NOTE {
            continue;
        }
        let (offset, size, align) = if wide {
            (
                uint_at(segment, 8, 8, endian)?,
                uint_at(segment, 32, 8, endian)?,
                uint_at(segment, 48, 8, endian)?,
            )
        } else {
            (
                uint_at(segment, 4, 4, endian)?,
                uint_at(segment, 16, 4, endian)?,
                uint_at(segment, 28, 4, endian)?,
            )
        };
        let Some(notes) = read_vec(file, offset, size) else { continue };
        if let Some(id) = gnu_build_id(&notes, endian, if align == 8 { 8 } else { 4 }) {
            return Some(id.to_vec());
        }
    }
    None
}

/// Walks one note segment: each note is three words — the name's length, the
/// description's length, the kind — then the name and the description, each
/// padded to the segment's alignment.
fn gnu_build_id(notes: &[u8], endian: Endian, align: usize) -> Option<&[u8]> {
    let mask = align.saturating_sub(1);
    let pad = |n: usize| n.checked_add(mask).map(|n| n & !mask);
    let mut at = 0usize;
    while at < notes.len() {
        let name_len = usize::try_from(u32_at(notes, at, endian)?).ok()?;
        let desc_len = usize::try_from(u32_at(notes, at.checked_add(4)?, endian)?).ok()?;
        let kind = u32_at(notes, at.checked_add(8)?, endian)?;
        let name_at = at.checked_add(12)?;
        let desc_at = name_at.checked_add(pad(name_len)?)?;
        if kind == NT_GNU_BUILD_ID && bytes_at(notes, name_at, name_len)? == b"GNU\0" {
            return bytes_at(notes, desc_at, desc_len);
        }
        at = desc_at.checked_add(pad(desc_len)?)?;
    }
    None
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Where the fallback hashes are remembered: `$XDG_CACHE_HOME/buri`, or
/// `~/.cache/buri`. Under the user's own home rather than the shared temporary
/// directory, so that another account cannot plant an identity for this one.
fn cache_dir() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))?;
    Some(base.join("buri").join("exe-identity"))
}

/// `sha256:<hex>` of the file, through a record in `cache` keyed by the
/// file's path, size and modification time. A rebuild moves the time, so it
/// misses; a record that is not a well-formed hash is ignored and rewritten.
fn hashed(path: &Path, cache: Option<&Path>) -> Option<String> {
    let meta = std::fs::metadata(path).ok()?;
    let record = cache.and_then(|dir| {
        let stamp = meta.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?;
        let mut key = Sha256::new();
        key.text(&path.to_string_lossy());
        key.field(&meta.len().to_le_bytes());
        key.field(&stamp.as_nanos().to_le_bytes());
        Some(dir.join(key.finish()))
    });
    if let Some(found) = record.as_ref().and_then(|r| std::fs::read_to_string(r).ok()) {
        let digest = found.strip_prefix("sha256:").unwrap_or_default();
        if digest.len() == 64 && digest.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Some(found);
        }
    }
    let identity = format!("sha256:{}", hash_bytes(&std::fs::read(path).ok()?));
    if let Some(record) = record {
        // Renamed into place, so a concurrent reader sees the whole record or
        // none of it. Failing to remember is not failing: the next process
        // hashes again.
        if let Some(dir) = record.parent() {
            let _ = std::fs::create_dir_all(dir);
            let partial = dir.join(format!(".partial-{}", std::process::id()));
            if std::fs::write(&partial, &identity).is_ok() {
                let _ = std::fs::rename(&partial, &record);
            }
        }
    }
    Some(identity)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir()
            .join(format!("buri-exe-identity-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A 64-bit little-endian Mach-O header with one unrelated load command
    /// and then `LC_UUID`.
    fn macho(uuid: [u8; 16]) -> Vec<u8> {
        let mut commands = Vec::new();
        commands.extend(0x19u32.to_le_bytes()); // LC_SEGMENT_64, skipped
        commands.extend(16u32.to_le_bytes());
        commands.extend([0u8; 8]);
        commands.extend(LC_UUID.to_le_bytes());
        commands.extend(24u32.to_le_bytes());
        commands.extend(uuid);
        let mut file = Vec::new();
        file.extend(0xfeed_facfu32.to_le_bytes());
        file.extend([0u8; 12]);
        file.extend(2u32.to_le_bytes());
        file.extend(u32::try_from(commands.len()).unwrap().to_le_bytes());
        file.extend([0u8; 8]);
        file.extend(commands);
        file.extend(b"code that never moves the uuid");
        file
    }

    /// A 64-bit little-endian ELF with one `PT_NOTE` segment holding an
    /// unrelated note and then the GNU build id.
    fn elf(id: &[u8]) -> Vec<u8> {
        let mut notes = Vec::new();
        for (name, kind, desc) in [(&b"XYZ\0"[..], 1u32, &[7u8; 5][..]), (b"GNU\0", 3, id)] {
            notes.extend(u32::try_from(name.len()).unwrap().to_le_bytes());
            notes.extend(u32::try_from(desc.len()).unwrap().to_le_bytes());
            notes.extend(kind.to_le_bytes());
            notes.extend(name);
            notes.extend(desc);
            while !notes.len().is_multiple_of(4) {
                notes.push(0);
            }
        }
        let mut file = vec![0u8; 64];
        file[..4].copy_from_slice(b"\x7fELF");
        file[4] = 2;
        file[5] = 1;
        file[32..40].copy_from_slice(&64u64.to_le_bytes());
        file[54..56].copy_from_slice(&56u16.to_le_bytes());
        file[56..58].copy_from_slice(&1u16.to_le_bytes());
        let mut header = [0u8; 56];
        header[..4].copy_from_slice(&PT_NOTE.to_le_bytes());
        header[8..16].copy_from_slice(&120u64.to_le_bytes());
        header[32..40].copy_from_slice(&u64::try_from(notes.len()).unwrap().to_le_bytes());
        header[48..56].copy_from_slice(&4u64.to_le_bytes());
        file.extend(header);
        file.extend(notes);
        file
    }

    #[test]
    fn a_mach_o_is_known_by_its_uuid() {
        let dir = scratch("macho");
        let (a, b) = (dir.join("a"), dir.join("b"));
        std::fs::write(&a, macho([1; 16])).unwrap();
        std::fs::write(&b, macho([2; 16])).unwrap();
        assert_eq!(
            linker_identity(&a).as_deref(),
            Some("macho-uuid:01010101010101010101010101010101")
        );
        assert_eq!(linker_identity(&a), linker_identity(&a));
        assert_ne!(linker_identity(&a), linker_identity(&b));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_elf_is_known_by_its_build_id() {
        let dir = scratch("elf");
        let (a, b) = (dir.join("a"), dir.join("b"));
        std::fs::write(&a, elf(&[0xab; 20])).unwrap();
        std::fs::write(&b, elf(&[0xcd; 20])).unwrap();
        assert_eq!(linker_identity(&a).as_deref(), Some(&*format!("elf-build-id:{}", "ab".repeat(20))));
        assert_eq!(linker_identity(&a), linker_identity(&a));
        assert_ne!(linker_identity(&a), linker_identity(&b));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A file with no header the reader knows falls back to its hash, and the
    /// record remembered for it is served to the next ask — keyed by the
    /// file, so a different file is not served it.
    #[test]
    fn without_a_linker_id_the_bytes_are_hashed_once() {
        let dir = scratch("hashed");
        let cache = dir.join("cache");
        let (a, b) = (dir.join("a"), dir.join("b"));
        std::fs::write(&a, b"one program").unwrap();
        std::fs::write(&b, b"another program").unwrap();
        assert_eq!(linker_identity(&a), None);
        let first = hashed(&a, Some(&cache)).unwrap();
        assert_eq!(first, format!("sha256:{}", hash_bytes(b"one program")));
        assert_eq!(std::fs::read_dir(&cache).unwrap().count(), 1, "the hash was not remembered");
        assert_eq!(hashed(&a, Some(&cache)).unwrap(), first);
        assert_ne!(hashed(&b, Some(&cache)).unwrap(), first);

        // A damaged record is not believed.
        for entry in std::fs::read_dir(&cache).unwrap() {
            std::fs::write(entry.unwrap().path(), "sha256:not a hash").unwrap();
        }
        assert_eq!(hashed(&a, Some(&cache)).unwrap(), first);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_truncated_header_is_no_identity_rather_than_a_panic() {
        let dir = scratch("truncated");
        let whole = macho([3; 16]);
        for cut in [0, 4, 20, 31, 40, whole.len().saturating_sub(31)] {
            let path = dir.join(format!("cut-{cut}"));
            std::fs::write(&path, &whole[..cut]).unwrap();
            assert_eq!(linker_identity(&path), None, "cut at {cut}");
        }
        let whole = elf(&[9; 20]);
        for cut in [4, 63, 64, 100, whole.len().saturating_sub(1)] {
            let path = dir.join(format!("elf-cut-{cut}"));
            std::fs::write(&path, &whole[..cut]).unwrap();
            assert_eq!(linker_identity(&path), None, "elf cut at {cut}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
