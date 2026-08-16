//! The toolchain pin: which compiler `REPO.buri` says must build this
//! repository, and what happens when it is not the one running.
//!
//! REPO-CONFIG.md is unambiguous about why this exists — "an exact version,
//! never a range … the CLI refuses to run if the toolchain it resolved does not
//! hash to it, which is the difference between pinning a version and pinning a
//! compiler." Two decisions were left to the implementation, and both are here.
//!
//! **What is hashed.** The running executable. There is no release archive on
//! this machine to hash — the archive is what a downloader would verify before
//! unpacking it, and there is no downloader — so the pin is checked against the
//! artifact the archive would have contained. That is the strictly stronger
//! check of the two: it also catches an executable that was replaced after it
//! was unpacked.
//!
//! **What an unpinned repository looks like.** A `sha256` of nothing but zeros
//! (`""`, `"00"`, sixty-four of them) is the sentinel for *unpinned*: a
//! repository whose toolchain has no published release to name yet, which is
//! every repository in this toolchain's own test suite and this toolchain's own
//! development checkout. An unpinned pin verifies nothing. It still enters
//! every cache key, because two toolchains that disagree still produce
//! different artifacts whether or not anybody wrote the hash down.
//!
//! The escape hatch is the sentinel and nothing else. There is no flag and no
//! environment variable, because a pin you can turn off from the command line
//! is a pin that gets turned off in the one script that matters.

use crate::build::buildfile::Toolchain;
use crate::build::cache::hash_bytes;
use crate::commands::arguments::VERSION;

/// Whether a `sha256` field is a pin at all.
///
/// Hex, and not all zeros. Anything else — a truncated hash, a placeholder, a
/// sentence — is a repository that has not pinned, rather than a repository
/// that has pinned wrongly, because guessing which of the two somebody meant is
/// how a typo becomes an unchecked build.
pub fn is_pinned(sha256: &str) -> bool {
    !sha256.is_empty()
        && sha256.chars().all(|c| c.is_ascii_hexdigit())
        && sha256.chars().any(|c| c != '0')
}

/// The SHA-256 of the running executable.
pub fn running_sha256() -> Option<String> {
    let exe = std::env::current_exe().ok()?;
    Some(hash_bytes(&std::fs::read(exe).ok()?))
}

/// Refuses the invocation when the repository pins a toolchain this is not.
///
/// The message is the whole product of this function, so it says both halves —
/// what was asked for and what is here — rather than reporting that something
/// did not match.
pub fn verify(t: &Toolchain) -> Result<(), String> {
    if !t.version.is_empty() && t.version != VERSION {
        return Err(format!(
            "this repository pins toolchain {} but this toolchain is {VERSION}\n  \
             = an exact version, never a range: two checkouts of the same commit must not \
             build with two different compilers\n  \
             = install toolchain {}, or change `toolchain.version` in REPO.buri",
            t.version, t.version
        ));
    }
    if !is_pinned(&t.sha256) {
        return Ok(());
    }
    let Some(actual) = running_sha256() else {
        return Err(
            "REPO.buri pins a toolchain by sha256 and this executable cannot be read to check it\n  \
             = a pin that cannot be checked is not a pin; make the executable readable, or write \
             a sha256 of zeros to say the toolchain is unpinned"
                .to_string(),
        );
    };
    if actual == t.sha256 {
        return Ok(());
    }
    Err(format!(
        "this repository pins a toolchain whose sha256 is {} but this one hashes to {actual}\n  \
         = the version matches and the executable does not, so this is a different build of \
         {VERSION} than the one this repository was pinned to\n  \
         = install the pinned toolchain, or update `toolchain.sha256` in REPO.buri",
        t.sha256
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostics::Span;

    fn toolchain(version: &str, sha256: &str) -> Toolchain {
        Toolchain { version: version.into(), sha256: sha256.into(), span: Span::NONE }
    }

    #[test]
    fn zeros_are_the_unpinned_sentinel() {
        assert!(!is_pinned(""));
        assert!(!is_pinned("00"));
        assert!(!is_pinned(&"0".repeat(64)));
        assert!(is_pinned("01"));
        assert!(is_pinned(&"a".repeat(64)));
        // Not hex is not a pin: it cannot be the hash of anything.
        assert!(!is_pinned("unpinned"));
    }

    #[test]
    fn an_unpinned_repository_verifies_nothing() {
        assert!(verify(&toolchain(VERSION, "00")).is_ok());
        assert!(verify(&toolchain("", "")).is_ok());
    }

    #[test]
    fn a_version_that_is_not_this_one_is_refused() {
        let e = verify(&toolchain("0.0.1", "00")).unwrap_err();
        assert!(e.contains("pins toolchain 0.0.1"), "{e}");
        assert!(e.contains(VERSION), "{e}");
        // Every refusal says what to do about it, the way a diagnostic does.
        assert!(e.contains("REPO.buri"), "{e}");
    }

    #[test]
    fn a_sha256_that_is_not_this_executable_is_refused() {
        let e = verify(&toolchain(VERSION, &"a".repeat(64))).unwrap_err();
        assert!(e.contains(&"a".repeat(64)), "{e}");
        assert!(e.contains("REPO.buri"), "{e}");
    }

    /// The positive half, and the only one that proves what is being hashed:
    /// the pin this executable satisfies is its own hash.
    #[test]
    fn the_running_executable_satisfies_a_pin_on_itself() {
        let sha = running_sha256().expect("the running executable is readable");
        assert!(is_pinned(&sha), "the executable hashed to a sentinel: {sha}");
        assert!(verify(&toolchain(VERSION, &sha)).is_ok());
    }
}
