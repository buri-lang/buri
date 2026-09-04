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

use crate::build::musl::{self, Libc};

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

/// The Cargo features `cli/build.rs` built the archive with, one per line.
///
/// Written beside the archive and its digest by one run of the build script, so
/// a stale `OUT_DIR` cannot pair one build's bytes with another's answer — the
/// same argument [`ARCHIVE_SHA256`] is written for. Empty when there is no
/// archive at all: a host the runtime is not built for, and a host whose
/// dependency tree would not resolve, both write nothing here.
const FEATURES: &str = include_str!(concat!(env!("OUT_DIR"), "/libburi_rt.a.features"));

/// Whether the archive was built with a named Cargo feature.
///
/// A file read rather than a `--cfg`, for the reason the emptiness of
/// [`ARCHIVE`] is the availability signal: conditional compilation would need a
/// `check-cfg` list to know about, and this way the fact travels with the bytes
/// it is a fact about.
///
/// Whole lines, because the next feature is `net-h3` and it contains `net`.
pub fn declares(feature: &str) -> bool {
    declares_in(FEATURES, feature)
}

/// The C library `cli/build.rs` built the archive against.
///
/// Written beside the archive, its digest and its feature list by one run of
/// the build script, for the reason [`FEATURES`] is: a stale `OUT_DIR` cannot
/// pair one build's bytes with another build's answer.
///
/// Three values, and the empty file is one of them — `musl`, `gnu`, or nothing
/// at all on a host with no archive to have a libc.
const LIBC: &str = include_str!(concat!(env!("OUT_DIR"), "/libburi_rt.a.libc"));

/// Which libc this toolchain links against, and whether it brought its own.
///
/// The sidecar answers the first half and `musl::BAKED` the second, which is
/// why this is a function over two facts rather than a fourth line in the
/// feature list. They are genuinely separate: `cli/build.rs` bakes the sysroot
/// from *this rustc's* self-contained directory whether or not the runtime's
/// dependency tree resolved, so "the archive is musl" and "the bytes to link it
/// with are in this binary" can differ, and [`Libc::MuslSystem`] is the case
/// where they do.
///
/// A file read rather than a `--cfg`, for the reason the emptiness of
/// [`ARCHIVE`] is the availability signal.
pub fn libc() -> Libc {
    libc_in(LIBC, musl::BAKED)
}

/// [`libc`], over a sidecar and a baked-ness the caller supplies.
///
/// The same seam [`declares_in`] is: both facts are baked into this binary, so
/// a test has no other way to ask what a differently-built toolchain would
/// answer, and all four states are reachable on some host but never on one.
fn libc_in(sidecar: &str, baked: bool) -> Libc {
    match sidecar.trim() {
        "musl" if baked => Libc::MuslBaked,
        "musl" => Libc::MuslSystem,
        "gnu" => Libc::Glibc,
        // Empty, which is the only other value `libc_beside` writes. Anything
        // else would be a build script and a reader that disagree, and reading
        // it as "no Linux libc here" is the answer that degrades rather than
        // the one that mislinks.
        _ => Libc::Absent,
    }
}

/// [`declares`], over a feature list the caller supplies.
///
/// [`FEATURES`] is baked into this binary, so a test has no way to ask what a
/// differently-built toolchain's file would answer. This is that seam.
fn declares_in(features: &str, feature: &str) -> bool {
    features.lines().any(|line| line == feature)
}

/// Whether this toolchain's runtime archive carries the networking crates.
///
/// The runtime's `net` feature is on by default, so this is true of every
/// ordinary toolchain. It is false in four ways, and the language-visible
/// consequence is the same in all four: `BURI_RUNTIME_NET=0` at build time, a
/// dependency tree that would not resolve, a host with no C compiler (`ring`,
/// the TLS provider, compiles C and assembly, so `cli/build.rs` probes and
/// falls back), and a host with no archive at all.
///
/// What reads it is [`super::networking_gap`], which turns it into a refusal
/// naming the operations rather than a link error naming a symbol. That
/// asymmetry is the workspace manifest's dependency bar: `backend-llvm` off
/// costs a code generator a contributor can do without, and `net` off costs a
/// *language capability*, which the user is owed a sentence about.
///
/// A function rather than a `const`, unlike [`AVAILABLE`] beside it: a `const`
/// would have to scan the file with indexing and subtraction, which this
/// repository's lints refuse, and nothing asks this question in a `const`
/// context.
pub fn net() -> bool {
    declares("net")
}

/// Whether this toolchain's runtime archive carries QUIC, and therefore
/// HTTP/3.
///
/// **False on every ordinary toolchain**, which is the difference between this
/// and [`net`] beside it: `net` is on unless something took it away, while
/// `net-h3` is off unless somebody asked for it with `BURI_RUNTIME_NET_H3=1`.
/// The concurrency note gated h3 behind configuration until the crate is
/// trusted, and a cargo feature that is not in `default` is what that gate is;
/// `cli/runtime/manifest.toml`'s feature block argues it in full.
///
/// **Nothing turns this into a compile-time refusal, and that is the
/// decision.** `net-h3` is deliberately absent from [`net_intrinsic`]'s family
/// below: the operations behind it are `serve`'s, and refusing every program
/// that mentions `serve` would refuse every server that was only ever going to
/// speak HTTP/1.1. What a toolchain without QUIC owes a program that asked for
/// `.Http3` is a run-time `.Err(Unsupported)` — `cli/runtime/net.rs`'s `serves`
/// is the one line F2's `serve` calls to produce it — which is the same
/// asymmetry `host.HostNet.fetch` and `https://` already carry.
///
/// So what reads this today is a **test**, and that is the honest description:
/// `cli/tests/native/runtime.rs` asserts it against the archive's own
/// `buri_rt_net_h3_available` door, which is the round trip from the cargo
/// feature through the compiled constant and back through the feature file. The
/// first non-test reader is F2.
pub fn h3() -> bool {
    declares("net-h3")
}

/// Whether this toolchain's runtime archive can answer `Entropy`.
///
/// The runtime's `crypto` feature is on by default, so this is true of every
/// ordinary toolchain — [`net`]'s shape rather than [`h3`]'s. It is false in
/// three ways: `BURI_RUNTIME_CRYPTO=0` at build time, a dependency tree that
/// would not resolve, and a host with no archive at all. There is no
/// C-compiler leg here, which is the one difference from `net`: `getrandom`
/// compiles no C.
///
/// What reads it is [`super::cryptography_gap`], which turns it into a refusal
/// naming the operation rather than a link error naming
/// `buri_rt_host_entropy_bytes`. **A weaker generator is not the alternative**
/// — the archive holds `rng.rs`'s xoshiro either way, and answering `Entropy`
/// from it would be undetectable by anything but an attacker, which is why the
/// feature's absence is a refusal rather than a fallback.
pub fn crypto() -> bool {
    declares("crypto")
}

/// Whether an intrinsic key is one only a `net` runtime answers.
///
/// The three host effects the networking archive carries — `Listen` accepts
/// connections, `Sockets` writes to open ones, `Tasks` runs Buri code on the
/// carrier pool the same reactor drives. Matched on the effect type rather than
/// on the whole key, so an operation added to one of them by a later slice is
/// covered the day it is added rather than the day somebody remembers this
/// list.
///
/// **`host.HostTasks.parallel` was the first of these keys a program reached**,
/// and it is answered by `cli/runtime/rt.rs` — behind `net`, beside the carrier
/// pool D4 fans it out onto — which is what keeps this rule honest for it.
/// `host.HostListen.*` is reachable now as well: the grant table gives `Listen`
/// `LINUX, MACOS`, `core/net/server` runs the accept loop over its four
/// operations, and `cli/runtime/net.rs` answers them from behind the same
/// feature, so a program that serves and a toolchain built without `net` meet
/// each other here rather than at the system linker. `Sockets` is the one still
/// waiting on a caller — it is granted with `Listen`, but nothing performs a
/// WebSocket upgrade, so no program can name a socket to write to — and it is
/// matched anyway, which costs nothing and is one fewer thing to remember on
/// the day an upgrade lands. A key this returns true for and the archive
/// answers anyway is not a problem either — the gap is only ever consulted when
/// [`net`] is false, and with `net` off the archive answers none of them.
///
/// `host.HostNet.fetch` is deliberately **not** here, and it stayed out when
/// `https://` landed. The earlier reason was that `cli/runtime/http.rs` reached
/// none of the crates; the reason now is stronger. `http.rs` writes the
/// cleartext client itself and only *wraps* the socket for `https://`, so a
/// `net`-off runtime answers `http://` exactly as it always did. Putting the
/// key here would refuse, at compile time, every program that mentions
/// `Net.fetch` — including every program that was only ever going to ask for
/// `http://`. What a `net`-off toolchain owes an `https://` URL is a run-time
/// `NetError::Transport` naming the feature, and `http.rs`'s `parse` is where
/// that sentence is written.
///
/// **This asks about `net`, and there is deliberately no second version of it
/// for `net-h3`.** [`h3`] is a feature of the same file with none of the same
/// consequences: the keys an HTTP/3 server is reached through are
/// `HostListen`'s, which are already covered here, and the protocol a server
/// asks for is a *field* of a value rather than an operation a key names — so
/// there is nothing for a key-shaped rule to match on. A toolchain without QUIC
/// compiles the program and `serve` answers `.Err(Unsupported)`, which is the
/// same choice as `HostNet.fetch` above and made for the same reason.
pub fn net_intrinsic(key: &str) -> bool {
    // `core/actor`'s nine. They are not a host effect and have no `host.`
    // prefix to strip — the authority is the `C: Tasks` bound in each
    // signature and the key is the module's, which is `core/list`'s shape —
    // but their bodies are in `cli/runtime/rt.rs` behind the same feature, and
    // one of them (`mailboxPush`) parks on the reactor. So the question this
    // function asks is about *where the body is*, not about what the key looks
    // like, and the family is matched by its own prefix.
    if key.starts_with("actor.") {
        return true;
    }
    let Some(rest) = key.strip_prefix("host.") else { return false };
    let Some((effect, _operation)) = rest.split_once('.') else { return false };
    matches!(effect, "HostListen" | "HostSockets" | "HostTasks")
}

/// Whether an intrinsic key is one only a `crypto` runtime answers.
///
/// One effect and one operation today, and matched on the effect for
/// [`net_intrinsic`]'s reason: a second operation on `Entropy` is covered the
/// day it is added rather than the day somebody remembers this line.
///
/// **`host_testing.TestEntropy.*` is deliberately not here.** The test
/// platform's `Entropy` is seeded, its body is in `cli/runtime/testing.rs`
/// beside `TestRand`'s and behind no feature, and it reaches no crate — so a
/// `crypto`-less toolchain runs a suite that binds `entropy()` exactly as it
/// always did. That is the same distinction `host.HostFs` and
/// `host_testing.TestFs` are on: two implementations of one effect, and only
/// one of them needs the world.
pub fn crypto_intrinsic(key: &str) -> bool {
    let Some(rest) = key.strip_prefix("host.") else { return false };
    let Some((effect, _operation)) = rest.split_once('.') else { return false };
    effect == "HostEntropy"
}

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
    // COMPILED OUT on a host no archive is built for, rather than ignored. The
    // two say the same thing about such a host and only one of them adds a
    // line to every summary that a reader has to learn to disregard — and a
    // summary with a skip in it is one nobody reads the skips of.
    // `.github/scripts/assert-no-skips.sh` is the other half of that decision.
    #[cfg(any(target_os = "macos", target_os = "linux"))]
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

    /// The features file is a fact about the archive beside it, so the two
    /// cannot disagree about whether there is one.
    ///
    /// One direction only, and deliberately: an archive with no `net` is an
    /// ordinary `BURI_RUNTIME_NET=0` toolchain, while `net` with no archive
    /// would mean the build script wrote a feature list for bytes it never
    /// produced.
    #[test]
    fn networking_implies_an_archive() {
        assert!(!net() || AVAILABLE, "the features file names `net` and there is no archive");
    }

    /// `net-h3` implies `net`, in the manifest and therefore in the file.
    ///
    /// QUIC carries TLS 1.3 inside the transport and runs on the same reactor,
    /// so `net-h3 = ["net", "dep:quinn"]`; a features file that named h3 alone
    /// would mean `cli/build.rs` had written a state Cargo cannot produce.
    /// One direction only, like its neighbour: `net` without h3 is the default
    /// toolchain.
    #[test]
    fn http3_implies_networking() {
        assert!(!h3() || net(), "the features file names `net-h3` without `net`");
        assert!(!h3() || AVAILABLE, "the features file names `net-h3` and there is no archive");
    }

    /// Whole lines, and nothing else: `net-h3` is the next feature this file
    /// will hold and it contains `net`.
    #[test]
    fn a_feature_is_a_whole_line() {
        assert!(declares_in("net", "net"));
        assert!(declares_in("net\nnet-h3", "net"));
        assert!(declares_in("net\nnet-h3", "net-h3"));
        assert!(!declares_in("net-h3", "net"), "a prefix of a feature is not that feature");
        assert!(!declares_in("supernet", "net"), "a suffix of a feature is not that feature");
        assert!(!declares_in("", "net"), "an archive with no features declares none");
        assert!(!declares_in("net", ""), "the empty string is not a feature");
        // And the toolchain's own file is read by the same rule.
        assert_eq!(net(), declares_in(FEATURES, "net"));
        assert_eq!(h3(), declares_in(FEATURES, "net-h3"));
        assert!(!declares("no-such-feature"));
    }

    /// The family `net` carries, and the near misses that are not it.
    /// The sidecar's three values against the four states they mean, and the
    /// join with `musl::BAKED` that makes four out of three. Written as a table
    /// rather than as assertions about *this* host because only one row of it is
    /// reachable from any given build — which is the reason `libc_in` is a seam
    /// at all, the same reason `declares_in` is one.
    #[test]
    fn a_libc_is_a_sidecar_and_a_sysroot() {
        assert_eq!(libc_in("musl", true), Libc::MuslBaked);
        assert_eq!(
            libc_in("musl", false),
            Libc::MuslSystem,
            "a musl archive with no baked sysroot links against the machine's musl"
        );
        assert_eq!(libc_in("gnu", true), Libc::Glibc, "a gnu archive is gnu whatever was baked");
        assert_eq!(libc_in("gnu", false), Libc::Glibc);
        assert_eq!(
            libc_in("", true),
            Libc::Absent,
            "an empty sidecar is no archive, and no archive has no libc — baking a sysroot \
             does not invent one"
        );
        assert_eq!(libc_in("", false), Libc::Absent);
        // Trailing whitespace is not a fourth value. `libc_beside` writes no
        // newline, but a file that acquired one must not read as `Absent` and
        // silently take a Linux toolchain out of the musl claim.
        assert_eq!(libc_in("musl\n", true), Libc::MuslBaked);
        assert_eq!(libc_in("gnu\n", false), Libc::Glibc);
        // And the accessor is the seam over this build's own two facts.
        assert_eq!(libc(), libc_in(LIBC, musl::BAKED));
    }

    /// What the host implies about the two, on the one host each test runs on.
    ///
    /// macOS is the whole of what can be asserted unconditionally: `cli/build.rs`
    /// writes the sidecar empty and bakes nothing there, so both halves are
    /// pinned. The Linux side is `.github/scripts/assert-runtime-archive.sh`'s,
    /// because the answer there depends on what the *builder* had installed and
    /// a unit test cannot tell a contributor's laptop from a release runner —
    /// which is exactly the distinction that script exists to draw.
    #[test]
    #[cfg(target_os = "macos")]
    fn macos_has_no_linux_libc() {
        assert_eq!(libc(), Libc::Absent);
        // `musl::BAKED` is a `const` the build script writes, so clippy reads
        // this as an assertion on a constant; that it is constant *per build*
        // is the point — the check is that this build is the macOS one.
        #[allow(clippy::assertions_on_constants)]
        {
            assert!(!musl::BAKED, "there is no Linux link to bake a sysroot for on macOS");
        }
        for (name, bytes) in musl::FILES {
            assert!(bytes.is_empty(), "{name} should be baked empty on macOS");
        }
    }

    /// The sysroot's digest is a digest, whether or not there is a sysroot.
    ///
    /// An empty sysroot still has a hash — of eleven empty members, which is a
    /// perfectly good sixty-four hex digits — so the shape of the answer does
    /// not depend on the host, and Package B's cache key does not have to ask.
    #[test]
    fn the_sysroot_hash_is_a_hash() {
        let hash = musl::sysroot_hash();
        assert_eq!(hash.len(), 64, "{hash} is not a SHA-256 in hex");
        assert!(
            hash.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
            "{hash} is not lowercase hex — no newline to trim, the convention ARCHIVE_SHA256 sets"
        );
    }

    #[test]
    fn the_networking_family_is_three_effects() {
        for key in [
            "host.HostListen.listen",
            "host.HostSockets.socketSendText",
            "host.HostTasks.parallel",
        ] {
            assert!(net_intrinsic(key), "{key} is not recognised as networking");
        }
        for key in [
            "host.HostFs.readFile",
            "host.HostClock.nowMillis",
            "host.HostNet.fetch",
            "list.map",
            "host.HostListen",
            "HostListen.listen",
            "host.HostListener.listen",
        ] {
            assert!(!net_intrinsic(key), "{key} is not networking and was claimed");
        }
    }

    /// The family obeys the symbol rule like every other key, so the refusal
    /// and the link name the same thing.
    #[test]
    fn a_networking_key_mangles_like_any_other() {
        assert_eq!(symbol_for("host.HostListen.listen"), "buri_rt_host_listen_listen");
        assert_eq!(symbol_for("host.HostTasks.parallel"), "buri_rt_host_tasks_parallel");
    }

    /// The cryptography family is one effect, and the doubles are not in it.
    ///
    /// The second half is the one worth asserting: `host_testing.TestEntropy.*`
    /// is a seeded generator in `cli/runtime/testing.rs` behind no feature at
    /// all, so a `crypto`-less toolchain still runs every suite that binds
    /// `entropy()`. Claiming it here would refuse those suites for want of a
    /// crate none of them reaches.
    #[test]
    fn the_cryptography_family_is_one_effect_and_excludes_the_double() {
        assert!(crypto_intrinsic("host.HostEntropy.bytes"));
        for key in [
            "host_testing.TestEntropy.bytes",
            "host_testing.entropy",
            "host.HostRand.nextInt",
            "host.HostListen.listen",
            "crypto.sha256",
            "host.HostEntropy",
            "HostEntropy.bytes",
        ] {
            assert!(!crypto_intrinsic(key), "{key} is not cryptography and was claimed");
        }
        // And the two families never claim each other's keys, which is what
        // lets `backend::split_cryptography` run over what
        // `backend::split_networking` left.
        assert!(!net_intrinsic("host.HostEntropy.bytes"));
        assert!(!crypto_intrinsic("host.HostTasks.parallel"));
    }

    /// The key mangles like any other, so the refusal and the symbol the linker
    /// would have looked for are the same thing said twice.
    #[test]
    fn the_entropy_key_mangles_like_any_other() {
        assert_eq!(symbol_for("host.HostEntropy.bytes"), "buri_rt_host_entropy_bytes");
        assert_eq!(
            symbol_for("host_testing.TestEntropy.bytes"),
            "buri_rt_host_testing_test_entropy_bytes"
        );
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
