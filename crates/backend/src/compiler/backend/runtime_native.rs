//! The native runtime's symbol rule and the questions about its ABI a backend
//! asks: which symbol a key names, where an error's message goes, and which
//! keys only a `net` or `crypto` runtime answers.
//!
//! The archive itself is `buri`'s `compiler::backend::runtime_native`, beside
//! the `cli/build.rs` that builds it; that module re-exports this one.

/// The symbol rule, which `buri-middle` holds because the runtime table it
/// reads names each row's symbol with it.
pub use buri_middle::compiler::backend::runtime_symbol::{symbol_for, SYMBOL_PREFIX};

use crate::compiler::middle::layout::{At, EnumRepr, Layout, Repr};
use crate::diagnostics::invariant;

/// The byte offset, inside a tagged enum error `E`, of the `Str` its
/// message-carrying variants hold, or `None` for an `E` that is not one.
///
/// This is `cli/runtime/lib.rs` §2.1's **message** shape: *where* the message
/// goes. *Whether* an entry writes one is a column of the runtime table
/// (`Ret::ResMsg`), because two entries answering one `Result<Str, IoError>`
/// can differ: one meets an `EISDIR`, the other is a map in memory.
///
/// A bare enum has no payload area. Every tagged error the runtime table
/// answers has this shape, and a tagged `E` without it is a bug in the table:
///
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
    let Repr::Enum {
        repr: EnumRepr::Tagged { payload, .. },
        variants,
    } = &err.repr
    else {
        return None;
    };
    let carries =
        |fields: &Vec<At>| fields.len() == 1 && fields.first().map(|f| f.bytes()) == Some(*payload);
    let shaped = "a tagged error the runtime answers keeps one `Str` at its payload's start";
    invariant(
        variants
            .iter()
            .all(|fields| fields.is_empty() || carries(fields)),
        shaped,
    );
    invariant(variants.iter().any(carries), shaped);
    invariant(err.size.checked_sub(*payload) == Some(STR_BYTES), shaped);
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
    // A host effect's operations, `host.<Effect>.<operation>`.
    [
        "host.HostListen.",
        "host.HostSockets.",
        "host.HostTasks.",
        "host.HostWebSocketClient.",
    ]
    .iter()
    .any(|effect| key.starts_with(effect))
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
    key.starts_with("host.HostEntropy.")
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
            assert!(
                !net_intrinsic(key),
                "{key} is not networking and was claimed"
            );
        }
    }

    /// The family obeys the symbol rule like every other key, so the refusal
    /// and the link name the same thing.
    #[test]
    fn a_networking_key_mangles_like_any_other() {
        assert_eq!(
            symbol_for("host.HostListen.listen"),
            "buri_rt_host_listen_listen"
        );
        assert_eq!(
            symbol_for("host.HostTasks.parallel"),
            "buri_rt_host_tasks_parallel"
        );
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
            assert!(
                !crypto_intrinsic(key),
                "{key} is not cryptography and was claimed"
            );
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
        assert_eq!(
            symbol_for("host.HostEntropy.bytes"),
            "buri_rt_host_entropy_bytes"
        );
        assert_eq!(
            symbol_for("host_testing.TestEntropy.bytes"),
            "buri_rt_host_testing_test_entropy_bytes"
        );
    }

    // -- §2.1's message shape ------------------------------------------------
    //
    // Over layouts built by hand rather than over `IoError`'s, and deliberately:
    // what is under test is the *rule*. A tagged error outside it is a bug in
    // the runtime table and stops the build, and
    // `the_message_shape_is_the_one_io_error_has` in `runtime_table.rs` is what
    // ties the rule to that type.

    use crate::compiler::middle::layout::{Scalar, Words};
    use crate::compiler::semantics::layered::Dense;

    /// The payload area's start, a word in.
    const WORD: At = At::Word(Words::nth(1));

    /// A tagged enum: a one-byte tag at zero, a payload area at eight, and one
    /// field list per variant.
    fn tagged(size: u32, variants: Vec<Vec<At>>) -> Layout {
        Layout {
            size,
            align: 8,
            stride: size,
            fields: Dense::new(),
            repr: Repr::Enum {
                repr: EnumRepr::Tagged {
                    tag: Scalar::I8,
                    payload: 8,
                },
                variants,
            },
        }
    }

    /// `IoError`: six variants carrying nothing and a seventh carrying a `Str`.
    #[test]
    fn one_trailing_str_variant_is_the_message_shape() {
        let io_error = tagged(
            32,
            vec![vec![], vec![], vec![], vec![], vec![], vec![], vec![WORD]],
        );
        assert_eq!(error_message_offset(&io_error), Some(8));
    }

    /// `NetError`: two variants carry a `Str`, both at the payload area's start,
    /// so one address serves whichever the discriminant names.
    #[test]
    fn every_payload_variant_holding_one_str_is_the_message_shape() {
        let net_error = tagged(32, vec![vec![], vec![], vec![WORD], vec![WORD], vec![]]);
        assert_eq!(error_message_offset(&net_error), Some(8));
    }

    /// A bare enum has no payload area at all, which is every error the shape
    /// was restricted to before it existed.
    #[test]
    fn a_bare_enum_has_no_message() {
        let bare = Layout {
            size: 1,
            align: 1,
            stride: 1,
            fields: Dense::new(),
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
            fields: [At::ZERO].into_iter().collect(),
            repr: Repr::Aggregate,
        };
        assert_eq!(error_message_offset(&utf8_error), None);
    }
}
