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
//! `buri` binary self-contained, which is what `build/cache.rs` already
//! assumes when it folds the toolchain version into every action key: a
//! runtime that lived beside the binary would be an unpinned input to every
//! artifact that no key names.

/// The prefix on every symbol the runtime exports.
///
/// One prefix and one rule, so that "is this symbol the runtime's" is a string
/// comparison and not a table. Both native backends import through it.
pub const SYMBOL_PREFIX: &str = "buri_rt_";

/// The symbol `cli/runtime/lib.rs` §1's rule names for an intrinsic key.
///
/// Used to *check* a backend's runtime table rather than to drive emission —
/// each table spells its own symbol, and this is what says the spelling obeys
/// the contract. A key the rule names is not thereby a key a table has a row
/// for: `str.concat` mangles to `buri_rt_str_concat`, which the archive does
/// export, and `runtime_table.rs` still has no row for it and says why.
///
/// The rule is "[`SYMBOL_PREFIX`] followed by `snake_case`", plus one thing the
/// contract states by example rather than in words: `host.HostStdout.println`
/// is `buri_rt_host_stdout_println` and **not** `buri_rt_host_host_stdout_println`.
/// The effect type repeats its module in its name, and the symbol does not
/// repeat it twice — so a snake-cased segment that begins with the previous
/// segment drops that prefix. `host.HostAlloc.allocate` is
/// `buri_rt_host_alloc_allocate`, which is the same rule and keeps the
/// non-redundant repetition it happens to have.
///
/// One copy for every backend, because the rule is the runtime's and not any
/// one code generator's: two manglers that drifted would disagree about which
/// table is wrong.
pub fn symbol_for(key: &str) -> String {
    let mut out = String::from(SYMBOL_PREFIX);
    let mut previous = String::new();
    for (i, segment) in key.split('.').enumerate() {
        let mut piece = String::new();
        snake_into(segment, &mut piece);
        if !previous.is_empty() {
            if let Some(rest) = piece.strip_prefix(&format!("{previous}_")) {
                piece = rest.to_string();
            }
        }
        if i > 0 {
            out.push('_');
        }
        previous.clone_from(&piece);
        out.push_str(&piece);
    }
    out
}

/// `HostFs` -> `host_fs`, `readFile` -> `read_file`, `nowMillis` ->
/// `now_millis`. An underscore before an upper-case letter that follows a
/// lower-case one or a digit; runs of capitals are not split, because no key
/// has one.
fn snake_into(segment: &str, out: &mut String) {
    let mut previous_lower = false;
    for c in segment.chars() {
        if c.is_ascii_uppercase() {
            if previous_lower {
                out.push('_');
            }
            out.push(c.to_ascii_lowercase());
            previous_lower = false;
        } else {
            out.push(c);
            previous_lower = c.is_ascii_lowercase() || c.is_ascii_digit();
        }
    }
}

/// `libburi_rt.a` for the host.
///
/// Empty on a host `cli/build.rs` builds no runtime for — anything that is not
/// macOS or Linux — which is the same set of hosts that has no native backend
/// to link it into. [`AVAILABLE`] is the question to ask.
///
/// Since the runtime's `net` feature there is a second way to be empty, and it
/// is not a property of the host: a build that could not **resolve** the
/// runtime's dependency tree writes an empty archive too, with a
/// `cargo:warning` naming which of the three causes it was (no network and a
/// cold registry, a sandbox that vendored only the toolchain's lockfile, or a
/// `cli/runtime/manifest.lock` the manifest has outgrown). That is the
/// "degrades rather than breaks" clause reaching one level further down, and
/// the reason it is not silent in CI is
/// `.github/scripts/assert-runtime-archive.sh`.
pub const ARCHIVE: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/libburi_rt.a"));

/// Whether this toolchain has a runtime to link against.
pub const AVAILABLE: bool = !ARCHIVE.is_empty();

/// The archive's SHA-256, **taken by `cli/build.rs` when the bytes were
/// written**.
///
/// BUILD-AND-WATCH.md §2.2: editing the runtime relinks every artifact and
/// recompiles none, which is right, because the runtime is linked against and
/// not compiled against. This is what makes that true — without it, a changed
/// runtime would be invisible to a cache that only hashes source.
///
/// The digest is the same string it always was; what moved is *when*. [`ARCHIVE`]
/// is a constant of this binary, so its hash is fixed the moment the binary is
/// linked, and computing it at run time was six and a quarter megabytes of
/// SHA-256 in front of the first `link` key of every process that builds
/// anything native — about thirty-five milliseconds, before any cache lookup
/// could be made, and therefore paid in full by a **no-op** build.
/// `build::actions::runtime_archive_hash`'s `OnceLock` removed the repeats; a
/// `buri` invocation is a process, so only the build script can remove the
/// first one.
///
/// `the_hash_is_of_the_bytes` is what keeps this honest, and it is the reason
/// `cli/build.rs` `#[path]`-includes `build::sha256` rather than restating the
/// algorithm: the assertion is `archive_hash() == hash_bytes(ARCHIVE)` — the
/// baked answer against the computed one, over the very bytes that were baked.
pub fn archive_hash() -> String {
    ARCHIVE_SHA256.to_string()
}

/// `libburi_rt.a`'s digest, written beside it in `OUT_DIR` by `cli/build.rs`.
///
/// Sixty-four hex digits and nothing else — no newline to trim — so that this
/// is the string [`hash_bytes`] would have answered and not a rendering of it.
/// It sits beside the `include_bytes!` above deliberately: one `OUT_DIR`, two
/// files written by one run of the build script, so there is no way to pair
/// this digest with some other build's archive.
const ARCHIVE_SHA256: &str = include_str!(concat!(env!("OUT_DIR"), "/libburi_rt.a.sha256"));

/// The filename to write the archive under. Named, rather than spelled at each
/// use, because the linker command line names it too.
pub const ARCHIVE_NAME: &str = "libburi_rt.a";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build::cache::hash_bytes;

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
    ///
    /// Since the digest is baked by `cli/build.rs`, this is also the join
    /// between the two copies of SHA-256 that exist at different times: the
    /// build script's, over the bytes as it wrote them, and the toolchain's,
    /// over the bytes as it embedded them. They are one file
    /// (`build/sha256.rs`, `#[path]`-included by the script), and this is the
    /// assertion that says so — a fork of that file, or a stale `OUT_DIR`
    /// pairing one build's archive with another's digest, fails here and moves
    /// no cache key silently.
    #[test]
    fn the_hash_is_of_the_bytes() {
        assert_eq!(archive_hash(), hash_bytes(ARCHIVE));
        assert_eq!(archive_hash().len(), 64);
    }
}
