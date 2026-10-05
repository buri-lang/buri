//! The native runtime's symbol rule and the questions about its ABI a backend
//! asks: which symbol a key names, where an error's message goes, and which
//! keys only a `net` or `crypto` runtime answers.
//!
//! The archive itself is `buri`'s `compiler::backend::runtime_native`, beside
//! the `cli/build.rs` that builds it; that module re-exports this one.

/// The prefix on every symbol the runtime exports.
///
/// One prefix and one rule, so that "is this symbol the runtime's" is a string
/// comparison and not a table. Both native backends import through it.
pub const SYMBOL_PREFIX: &str = "buri_rt_";

/// The symbol `cli/runtime/lib.rs` §1's rule names for an intrinsic key.
///
/// This names the symbol of a `runtime_table.rs` row (`Entry::symbol`); the
/// table, not this rule, decides which keys exist. A key the rule names is not
/// thereby a key the table has a row for: `str.concat` mangles to
/// `buri_rt_str_concat`, which the archive does export, and `runtime_table.rs`
/// still has no row for it and says why.
///
/// The rule is "[`SYMBOL_PREFIX`] followed by `snake_case`", plus one thing the
/// contract states by example rather than in words: `host.HostStdout.println`
/// is `buri_rt_host_stdout_println` and **not** `buri_rt_host_host_stdout_println`.
/// The effect type repeats its module in its name, and the symbol does not
/// repeat it twice — so a snake-cased segment that begins with the previous
/// segment drops that prefix. `host.HostAllocator.allocate` is
/// `buri_rt_host_allocator_allocate`, which is the same rule and keeps the
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

/// `HostFileSystem` -> `host_file_system`, `readFile` -> `read_file`,
/// `nowMilliseconds` -> `now_milliseconds`. An underscore before an upper-case letter that follows a
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

use crate::compiler::middle::layout::{EnumRepr, Layout, Repr};

/// The byte offset, inside an enum error `E`, of the `Str` its
/// message-carrying variants hold, or `None` for an `E` with nowhere to put one.
///
/// This is `cli/runtime/lib.rs` §2.1's **message** shape: *where* the message
/// goes. *Whether* an entry writes one is a column of the runtime table
/// (`Ret::ResMsg`), because two entries answering one `Result<Str, IoError>`
/// can differ: one meets an `EISDIR`, the other is a map in memory.
///
/// `E` qualifies when:
///
/// * it is a **tagged** enum, since a bare one has no payload area;
/// * every variant carries either nothing or **one field at the payload
///   area's start**, and at least one carries that field;
/// * the payload area is 24 bytes, which is a `Str` (VALUE-MODEL.md §3).
///
/// So every payload-carrying variant keeps its message at the same offset,
/// and the address does not depend on which variant the discriminant names.
/// `IoError` (`Other(Str)`) and `NetError` (`BadUrl(Str)`, `Transport(Str)`)
/// both answer `Some(8)`.
///
/// The caller zeroes the area before the call, the entry writes the message
/// where it has one, and on failure the caller zeroes only the bytes in front
/// of it. A fieldless variant therefore keeps an empty `Str` it never reads.
/// `cli/runtime/host.rs`'s `fail` is the runtime's side.
pub fn error_message_offset(err: &Layout) -> Option<u32> {
    let Repr::Enum { repr: EnumRepr::Tagged { payload, .. }, variants } = &err.repr else {
        return None;
    };
    let carries = |fields: &Vec<u32>| fields.len() == 1 && fields.first() == Some(payload);
    if !variants.iter().all(|fields| fields.is_empty() || carries(fields)) {
        return None;
    }
    if !variants.iter().any(carries) {
        return None;
    }
    if err.size.checked_sub(*payload) != Some(STR_BYTES) {
        return None;
    }
    Some(*payload)
}

/// `{ base, ptr, len }` — VALUE-MODEL.md §3, restated here because
/// [`error_message_offset`] recognises a `Str` by the bytes it occupies.
const STR_BYTES: u32 = 24;

/// Whether an intrinsic key is one only a `net` runtime answers.
///
/// The three host effects the networking archive carries — `Listen` accepts
/// connections, `Sockets` writes to open ones, `Tasks` runs Buri code on the
/// thread pool the same reactor drives. Matched on the effect type rather than
/// on the whole key, so an operation added to one of them by a later slice is
/// covered the day it is added rather than the day somebody remembers this
/// list.
///
/// **`host.HostTasks.parallel` was the first of these keys a program reached**,
/// and it is answered by `cli/runtime/rt.rs` — behind `net`, beside the thread
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
/// `runtime_native::net` is false, and with `net` off the archive answers none of them.
///
/// `host.HostNetwork.fetch` is deliberately **not** here, and it stayed out when
/// `https://` landed. The earlier reason was that `cli/runtime/http.rs` reached
/// none of the crates; the reason now is stronger. `http.rs` writes the
/// cleartext client itself and only *wraps* the socket for `https://`, so a
/// `net`-off runtime answers `http://` exactly as it always did. Putting the
/// key here would refuse, at compile time, every program that mentions
/// `Network.fetch` — including every program that was only ever going to ask for
/// `http://`. What a `net`-off toolchain owes an `https://` URL is a run-time
/// `NetError::Transport` naming the feature, and `http.rs`'s `parse` is where
/// that sentence is written.
///
/// **This asks about `net`, and there is deliberately no second version of it
/// for `net-h3`.** `runtime_native::h3` is a feature of the same file with none of the same
/// consequences: the keys an HTTP/3 server is reached through are
/// `HostListen`'s, which are already covered here, and the protocol a server
/// asks for is a *field* of a value rather than an operation a key names — so
/// there is nothing for a key-shaped rule to match on. A toolchain without QUIC
/// compiles the program and `serve` answers `.Err(Unsupported)`, which is the
/// same choice as `HostNetwork.fetch` above and made for the same reason.
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
    // `core/tasks`'s scope entries, for the same reason and in the same file.
    // One of them (`scopeClaim`) parks too, and all of them live in `rt.rs`,
    // which is behind the feature in full.
    if key.starts_with("tasks.scope") {
        return true;
    }
    // Its timers, likewise: the table and the waits that fire them are
    // `rt.rs`'s.
    if key.starts_with("tasks.timer") {
        return true;
    }
    let Some(rest) = key.strip_prefix("host.") else { return false };
    let Some((effect, _operation)) = rest.split_once('.') else { return false };
    matches!(effect, "HostListen" | "HostSockets" | "HostTasks" | "HostWebSocketClient")
}

/// Whether an intrinsic key is one only a `crypto` runtime answers.
///
/// `Entropy`, matched on the effect for [`net_intrinsic`]'s reason, and
/// `core/crypto`'s five `ring` entries by name.
///
/// **`host_testing.TestEntropy.*` is deliberately not here.** The test
/// platform's `Entropy` is seeded, its body is in `cli/runtime/testing.rs`
/// beside `TestRandom`'s and behind no feature, and it reaches no crate — so a
/// `crypto`-less toolchain runs a suite that binds `entropy()` exactly as it
/// always did. That is the same distinction `host.HostFileSystem` and
/// `host_testing.TestFileSystem` are on: two implementations of one effect, and only
/// one of them needs the world.
pub fn crypto_intrinsic(key: &str) -> bool {
    // `core/crypto`'s five `ring` entries (`cli/runtime/crypto.rs`). Named one
    // by one, because the rest of the module is Buri and reaches no feature.
    if matches!(
        key,
        "crypto.chacha20Poly1305Seal"
            | "crypto.chacha20Poly1305Open"
            | "crypto.ecdsaP256Sha256Verify"
            | "crypto.ed25519Verify"
            | "crypto.rsaPkcs1Sha256Verify"
    ) {
        return true;
    }
    let Some(rest) = key.strip_prefix("host.") else { return false };
    let Some((effect, _operation)) = rest.split_once('.') else { return false };
    effect == "HostEntropy"
}

/// The filename to write the archive under. Named, rather than spelled at each
/// use, because the linker command line names it too.
pub const ARCHIVE_NAME: &str = "libburi_rt.a";

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn the_networking_family_is_four_effects() {
        for key in [
            "host.HostListen.listen",
            "host.HostSockets.socketSendText",
            "host.HostTasks.parallel",
            "host.HostWebSocketClient.connectSocket",
        ] {
            assert!(net_intrinsic(key), "{key} is not recognised as networking");
        }
        for key in [
            "host.HostFileSystem.readFile",
            "host.HostClock.nowMilliseconds",
            "host.HostNetwork.fetch",
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
        assert!(crypto_intrinsic("crypto.chacha20Poly1305Seal"));
        assert!(crypto_intrinsic("crypto.ed25519Verify"));
        assert!(crypto_intrinsic("crypto.rsaPkcs1Sha256Verify"));
        for key in [
            "host_testing.TestEntropy.bytes",
            "host_testing.entropy",
            "host.HostRandom.nextInt",
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

    // -- §2.1's message shape ------------------------------------------------
    //
    // Over layouts built by hand rather than over `IoError`'s, and deliberately:
    // what is under test is the *rule*, and every one of the four ways to fail
    // it has to be reachable from somewhere. `middle/layout.rs` produces the
    // first of them from `platform/effect`'s own declaration, and
    // `the_message_shape_is_the_one_io_error_has` in `runtime_table.rs` is what
    // ties the rule to that type.

    use crate::compiler::middle::layout::Scalar;

    /// A tagged enum: a one-byte tag at zero, a payload area at eight, and one
    /// field list per variant.
    fn tagged(size: u32, variants: Vec<Vec<u32>>) -> Layout {
        Layout {
            size,
            align: 8,
            stride: size,
            fields: Vec::new(),
            repr: Repr::Enum {
                repr: EnumRepr::Tagged { tag: Scalar::I8, payload: 8 },
                variants,
            },
        }
    }

    /// `IoError`: six variants carrying nothing and a seventh carrying a `Str`.
    #[test]
    fn one_trailing_str_variant_is_the_message_shape() {
        let io_error = tagged(32, vec![vec![], vec![], vec![], vec![], vec![], vec![], vec![8]]);
        assert_eq!(error_message_offset(&io_error), Some(8));
    }

    /// `NetError`: two variants carry a `Str`, both at the payload area's start,
    /// so one address serves whichever the discriminant names.
    #[test]
    fn every_payload_variant_holding_one_str_is_the_message_shape() {
        let net_error = tagged(32, vec![vec![], vec![], vec![8], vec![8], vec![]]);
        assert_eq!(error_message_offset(&net_error), Some(8));
    }

    /// A variant with two fields has no single place for a message.
    #[test]
    fn a_variant_with_two_fields_is_not_the_message_shape() {
        let two = tagged(32, vec![vec![], vec![8], vec![8, 16]]);
        assert_eq!(error_message_offset(&two), None);
    }

    /// A payload that is not 24 bytes is not a `Str`, whatever else it is —
    /// one `Int` here, which is the shape a `.Code(Int)` variant would have.
    #[test]
    fn a_payload_that_is_not_a_str_is_not_the_message_shape() {
        let int_payload = tagged(16, vec![vec![], vec![8]]);
        assert_eq!(error_message_offset(&int_payload), None);
    }

    /// A bare enum has no payload area at all, which is every error the shape
    /// was restricted to before it existed.
    #[test]
    fn a_bare_enum_has_no_message() {
        let bare = Layout {
            size: 1,
            align: 1,
            stride: 1,
            fields: Vec::new(),
            repr: Repr::Enum {
                repr: EnumRepr::Bare { tag: Scalar::I8 },
                variants: vec![vec![], vec![], vec![]],
            },
        };
        assert_eq!(error_message_offset(&bare), None);
        // And neither does an error that is not an enum: `Utf8Error(Int)` is a
        // struct, and §2.1's *other* shape — a second out-pointer — is what
        // carries it.
        let utf8_error = Layout {
            size: 8,
            align: 8,
            stride: 8,
            fields: vec![0],
            repr: Repr::Aggregate,
        };
        assert_eq!(error_message_offset(&utf8_error), None);
    }
}
