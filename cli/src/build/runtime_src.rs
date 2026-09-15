//! The runtime's sources, embedded so a cross build can re-assemble them.
//!
//! `cli/build.rs` assembles the runtime package in `OUT_DIR` and bakes only the
//! *host* `libburi_rt.a` (`runtime_native::ARCHIVE`). A cross link needs the
//! *target's* archive, and rebuilding it means compiling the runtime for a
//! foreign triple — which needs the sources, not the compiled bytes. So the
//! build script packs those sources into one blob ([`pack_runtime_src`] there)
//! and this module is the reading end: `include_bytes!` of the same shape
//! `runtime_native::ARCHIVE` uses, and an [`unpack`] that writes the package
//! back out for `build::runtime_cross` to build.
//!
//! **The bytes travel with the toolchain rather than being found on the
//! machine**, for the reason the runtime archive and the musl sysroot do: a
//! toolchain that went looking for its own sources on disk would make a cross
//! build depend on where — and whether — the checkout still is. A `cargo install
//! buri` has no checkout at all.
//!
//! Empty on a host `cli/build.rs` packed nothing for — the same host that gets
//! an empty archive — which [`AVAILABLE`] reports, so a caller asks before it
//! tries to unpack nothing.

use std::path::Path;

/// The packed runtime sources, or empty on a host with none.
///
/// The format is `build.rs`'s: a length-prefixed, path-sorted concatenation of
/// `[u32 name_len][name][u64 data_len][data]`. Empty is the signal, exactly as
/// it is for `runtime_native::ARCHIVE`.
pub const PACK: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/runtime-src.pack"));

/// Whether this toolchain carries the runtime sources to cross-build from.
pub const AVAILABLE: bool = !PACK.is_empty();

/// One member's `(relative path, bytes)`, decoded from the blob.
struct Member<'a> {
    path: &'a str,
    bytes: &'a [u8],
}

/// Every member of the packed blob, in the order it was written (sorted by
/// path), or `None` if the blob is malformed.
///
/// Malformed is `None` rather than a partial list because the blob is a constant
/// of this binary written by one run of the build script: a truncated one is a
/// packaging bug to surface, not a subset to compile. The only way it is empty
/// is the host that packed nothing, and [`AVAILABLE`] answers that first.
fn members(pack: &[u8]) -> Option<Vec<Member<'_>>> {
    let mut out = Vec::new();
    let mut at = 0usize;
    // Every advance is checked: a length in the blob is not to be trusted into
    // an index. A slice that runs off the end, or an addition that would wrap,
    // is `None` — a malformed blob, which the caller surfaces rather than reads
    // past. (The lints forbid bare `+` on an index for exactly this reason.)
    while at < pack.len() {
        let after_len = at.checked_add(4)?;
        let name_len = u32::from_le_bytes(pack.get(at..after_len)?.try_into().ok()?) as usize;
        let name_end = after_len.checked_add(name_len)?;
        let name = std::str::from_utf8(pack.get(after_len..name_end)?).ok()?;
        let after_data_len = name_end.checked_add(8)?;
        let data_len =
            u64::from_le_bytes(pack.get(name_end..after_data_len)?.try_into().ok()?) as usize;
        let data_end = after_data_len.checked_add(data_len)?;
        let bytes = pack.get(after_data_len..data_end)?;
        out.push(Member { path: name, bytes });
        at = data_end;
    }
    Some(out)
}

/// Writes the packed sources into `dir` as an assembled cargo package —
/// `Cargo.toml`, `Cargo.lock`, the `.rs`/`.s` files and `fonts/` — the way
/// `cli/build.rs`'s `assemble` leaves them in `OUT_DIR`.
///
/// **Only when the bytes differ**, member by member, so a warm cache directory
/// a second `buri build` reuses is not rewritten and its mtimes do not move: the
/// nested cargo `build::runtime_cross` runs treats an unchanged source as fresh,
/// and rewriting a file with the bytes it already holds would defeat that
/// exactly as it does in the toolchain's own build.
///
/// A path with a separator (`fonts/…`) has its parent created; a path that tries
/// to escape `dir` is refused, because the blob is trusted but the code that
/// reads it should not be the thing that trusts it.
pub fn unpack(dir: &Path) -> std::io::Result<()> {
    let members = members(PACK).ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "the runtime source blob is malformed",
        )
    })?;
    for member in members {
        // No absolute paths, no `..`: the members are `build.rs`'s own file
        // names and `fonts/<name>`, and anything else is a blob this code was
        // not asked to trust.
        let relative = Path::new(member.path);
        if relative.is_absolute() || relative.components().any(|c| c.as_os_str() == "..") {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "the runtime source blob names an unsafe path: {}",
                    member.path
                ),
            ));
        }
        let path = dir.join(relative);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        if std::fs::read(&path).is_ok_and(|existing| existing == member.bytes) {
            continue;
        }
        std::fs::write(&path, member.bytes)?;
    }
    Ok(())
}

/// The blob's SHA-256, for the cross build's cache key.
///
/// The cross archive depends on the sources exactly as the host archive does, so
/// a toolchain whose runtime changed must not reuse a cross archive built from
/// the old one. Computed once for the process: the blob is a constant of this
/// binary, so its digest cannot move after it is linked.
pub fn pack_hash() -> String {
    use std::sync::OnceLock;
    static HASH: OnceLock<String> = OnceLock::new();
    HASH.get_or_init(|| crate::build::cache::hash_bytes(PACK))
        .clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A round trip through the format the build script writes.
    #[test]
    fn a_packed_blob_unpacks_to_the_files_it_held() {
        // Hand-built rather than over `PACK`, so the format is under test on
        // every host — including the ones that packed nothing.
        let mut blob = Vec::new();
        for (name, data) in [
            ("Cargo.toml", &b"[package]"[..]),
            ("fonts/a.ttf", &b"\x00\x01"[..]),
        ] {
            blob.extend_from_slice(&(name.len() as u32).to_le_bytes());
            blob.extend_from_slice(name.as_bytes());
            blob.extend_from_slice(&(data.len() as u64).to_le_bytes());
            blob.extend_from_slice(data);
        }
        let parsed = members(&blob).expect("well-formed");
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].path, "Cargo.toml");
        assert_eq!(parsed[0].bytes, b"[package]");
        assert_eq!(parsed[1].path, "fonts/a.ttf");
        assert_eq!(parsed[1].bytes, b"\x00\x01");
    }

    /// A truncated blob is `None`, not a partial list.
    #[test]
    fn a_truncated_blob_is_rejected() {
        let mut blob = Vec::new();
        blob.extend_from_slice(&(4u32).to_le_bytes());
        blob.extend_from_slice(b"main"); // names a member but carries no length
        assert!(members(&blob).is_none());
    }

    /// The empty blob is a valid, empty member list — the host that packed
    /// nothing, which is what `AVAILABLE` reports.
    #[test]
    fn the_empty_blob_is_no_members() {
        assert_eq!(members(&[]).map(|m| m.len()), Some(0));
        assert_eq!(AVAILABLE, !PACK.is_empty());
    }
}
