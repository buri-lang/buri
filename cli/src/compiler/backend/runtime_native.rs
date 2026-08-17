//! The native runtime archive, and where it enters a build.
//!
//! `cli/runtime` is a Rust static library with a C ABI (VALUE-MODEL.md §10),
//! built for the host by `cli/build.rs` (BUILD-AND-WATCH.md §2.2) and embedded
//! here. `cli/runtime/lib.rs`'s module comment is the `buri_rt_*` ABI contract;
//! this file is only the delivery.
//!
//! The link step writes [`ARCHIVE`] beside the objects and passes it to `cc`:
//!
//! ```text
//! cc -o <artifact> <units...> libburi_rt.a ...
//! ```
//!
//! Embedding rather than shipping a file next to the binary is what makes a
//! `buri` binary self-contained, which is what `build/toolchain.rs` already
//! assumes when it pins the toolchain by hashing the running executable: a
//! runtime that lived beside the binary would be an unpinned input to every
//! artifact.

use crate::build::cache::hash_bytes;

/// The prefix on every symbol the runtime exports.
///
/// One prefix and one rule, so that "is this symbol the runtime's" is a string
/// comparison and not a table. Both native backends import through it.
pub const SYMBOL_PREFIX: &str = "buri_rt_";

/// `libburi_rt.a` for the host.
///
/// Empty on a host `cli/build.rs` builds no runtime for — anything that is not
/// macOS or Linux — which is the same set of hosts that has no native backend
/// to link it into. [`AVAILABLE`] is the question to ask.
pub const ARCHIVE: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/libburi_rt.a"));

/// Whether this toolchain has a runtime to link against.
pub const AVAILABLE: bool = !ARCHIVE.is_empty();

/// The archive's SHA-256, for the `link` cache key.
///
/// BUILD-AND-WATCH.md §2.2: editing the runtime relinks every artifact and
/// recompiles none, which is right, because the runtime is linked against and
/// not compiled against. This is what makes that true — without it, a changed
/// runtime would be invisible to a cache that only hashes source.
pub fn archive_hash() -> String {
    hash_bytes(ARCHIVE)
}

/// The filename to write the archive under. Named, rather than spelled at each
/// use, because the linker command line names it too.
pub const ARCHIVE_NAME: &str = "libburi_rt.a";

#[cfg(test)]
mod tests {
    use super::*;

    /// A host we build a runtime for must have produced one, and it must be an
    /// archive rather than whatever else ended up at that path. `!<arch>\n` is
    /// the magic every `ar` format shares, including Apple's and GNU's.
    #[test]
    #[cfg_attr(
        not(any(target_os = "macos", target_os = "linux")),
        ignore = "no runtime archive is built for this host"
    )]
    fn the_archive_is_an_archive() {
        assert_eq!(
            ARCHIVE.get(..8),
            Some(&b"!<arch>\n"[..]),
            "the embedded bytes are not an `ar` archive, or there are none"
        );
        // Which is the same fact `AVAILABLE` reports, and the reason the
        // assertion above is on the bytes: a host with no archive gets an empty
        // one rather than a build failure.
        assert_eq!(AVAILABLE, !ARCHIVE.is_empty());
    }

    /// The hash is what the `link` key is built from, so it has to be a hash of
    /// the bytes and not of something incidental.
    #[test]
    fn the_hash_is_of_the_bytes() {
        assert_eq!(archive_hash(), hash_bytes(ARCHIVE));
        assert_eq!(archive_hash().len(), 64);
    }
}
