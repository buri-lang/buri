//! Builds the two things a native backend needs and cannot write itself: the
//! runtime archive, and the copy-and-patch backend's stencil library.
//!
//! # 1. `libburi_rt.a`
//!
//! `cli/runtime` for the host, so that `backend::runtime_native` can
//! `include_bytes!` it.
//!
//! BUILD-AND-WATCH.md §2.2 settles the shape and this file is it:
//!
//! ```text
//! <assemble $OUT_DIR/rt-pkg from cli/runtime/>
//! cargo fetch --locked --manifest-path $OUT_DIR/rt-pkg/Cargo.toml   # offline first
//! cargo rustc --release --lib --manifest-path $OUT_DIR/rt-pkg/Cargo.toml \
//!             --target <host triple> --target-dir $OUT_DIR/rt \
//!             -- --remap-path-prefix==./runtime -Cmetadata=buri_rt -Cextra-filename=
//! ```
//!
//! Five properties are worth naming, because each of them is a decision:
//!
//! * **No build dependency for the *toolchain*.** `cargo` and `rustc` are
//!   already required to build it, so driving the one that is already running
//!   adds no tool and nothing to the toolchain's own lockfile. The obvious
//!   alternative — the `cc` crate and a runtime written in C — costs a build
//!   dependency *and* makes a C compiler a build-time requirement rather than a
//!   link-time one, which is heavier than the Rust dependency the toolchain
//!   already has (VALUE-MODEL.md §10).
//!
//!   The *runtime* is a different set and a different bar, and since the `net`
//!   feature it is no longer empty: `tokio`, `hyper`, `rustls` and
//!   `tungstenite`, closed by an exact list. The root `Cargo.toml` states both
//!   halves of the bar, `cli/runtime/manifest.toml` argues each entry, and
//!   `dependencies_stay_behind_the_bar` asserts the equality — so a fifth crate
//!   is a failing test rather than a review comment. What the toolchain's build
//!   inherits from that set is one thing only: the nested `cargo` now has a
//!   dependency tree to resolve, which is the degradation path below.
//!
//! * **The package is assembled in `OUT_DIR`, not checked in.** `cli/runtime/`
//!   holds the sources, `manifest.toml` and `manifest.lock`; this script copies
//!   them to `$OUT_DIR/rt-pkg/` as `Cargo.toml`, `Cargo.lock` and the same
//!   `.rs` files, and builds there.
//!
//!   The reason is packaging, and it is the regression this repairs. `cargo
//!   package` **skips any subdirectory containing a `Cargo.toml`**,
//!   unconditionally and before `include`/`exclude` are consulted, in both its
//!   git-driven and its filesystem-driven listers — verified, not assumed, and
//!   an explicit `include = ["runtime/**"]` does not override it. A manifest in
//!   `cli/runtime/` therefore deleted the entire directory from the published
//!   `buri` crate: a checkout built, and a `cargo install buri` from a registry
//!   tarball failed here with no runtime to compile. With the manifest under a
//!   name Cargo does not recognise, `cli/runtime/` is an ordinary directory of
//!   ordinary files and ships whole. `.github/scripts/assert-package-ships-
//!   runtime.sh` is the assertion; `cli/tests/language/corpus.rs` holds the
//!   invariant it rests on — no second `Cargo.toml` anywhere under `cli/`.
//!
//!   Nothing about the compile changes: Cargo still runs `rustc` with the
//!   package root as its working directory and the crate root as the relative
//!   path `lib.rs`, which is exactly what `RUNTIME_RUSTC_ARGS`'s empty
//!   `--remap-path-prefix` prefix depends on. Copies are written **only when
//!   the contents differ**, so a rerun of this script for an unrelated reason
//!   does not move an mtime and does not rebuild the runtime.
//!
//!   The lockfile is regenerated with `BURI_RUNTIME_RELOCK=1 cargo build -p
//!   buri`, which drops `--locked`, lets Cargo resolve, and copies the result
//!   back over `cli/runtime/manifest.lock`. That is the only path on which this
//!   script writes into the source tree, and it is opt-in for exactly that
//!   reason.
//!
//! * **Precompiled, not compiled on demand.** The archive is produced once when
//!   the toolchain is built, not once per `buri build`. A compile-on-demand
//!   runtime would put a `rustc` invocation inside the build loop the rest of
//!   this design spends its effort shortening, and would make the archive's
//!   hash — which enters the `link` cache key — depend on whatever `rustc` was
//!   on `PATH` at *use* time rather than at *install* time.
//!
//! * **The host triple and nothing else.** `--target <host>` names the one
//!   triple this build supports, which is why cross-compilation is refused
//!   rather than half-working (ARCHITECTURE.md §9). On a host with no runtime —
//!   anything that is not macOS or Linux — the archive is written empty and
//!   `runtime_native::AVAILABLE` is false, so the toolchain still builds and
//!   still runs the JavaScript backend, which is the "degrades rather than
//!   breaks" clause of the dependency bar applied to the runtime itself.
//!
//! * **A dependency tree that cannot be resolved degrades; one that cannot be
//!   compiled does not.** The second half of the same clause, and the two
//!   halves are deliberately not the same answer. A host that cannot *reach*
//!   the four crates — no network and a cold registry, a `nix` sandbox whose
//!   vendoring covers the toolchain's lockfile and not the runtime's, a
//!   lockfile this manifest has outgrown — is a host with no archive: empty,
//!   `AVAILABLE == false`, a `cargo:warning` naming which of the three it was,
//!   and a toolchain that still builds and still runs the JavaScript backend. A
//!   host that resolved the tree and then failed to *compile* it has a broken
//!   runtime, not a missing one, and that fails the build.
//!
//!   The probe is `cargo fetch --locked`, run `--offline` first so that the
//!   warm case costs no network and the sandboxed case is answered without one.
//!   It is deliberately a **separate command** from the build: a single `cargo
//!   rustc` cannot tell the two failures apart, and "the archive quietly went
//!   empty because a crate failed to compile" is precisely the silent green
//!   that `.github/scripts/assert-runtime-archive.sh` exists to refuse.
//!
//! * **The archive says which features it was built with.** `libburi_rt.a.features`
//!   is written beside the archive and beside its digest, holding the feature
//!   names one to a line — `net`, today — and empty when there is no archive at
//!   all. `runtime_native::net()` reads it, and `Backend::missing_intrinsics`
//!   turns it into a diagnostic naming the effect rather than a link error
//!   naming a symbol. A file rather than a `cargo:rustc-env` for the reason
//!   `digest_beside` already gives: the bytes, their digest and their feature
//!   list are written by one run of this script into one `OUT_DIR`, so a stale
//!   directory cannot pair one build's archive with another's answer.

#![allow(
    clippy::print_stderr,
    reason = "a build script's standard error *is* its diagnostic channel: \
              cargo prints it when the script fails, and there is no \
              `Session::emit` here to route it through."
)]

use std::path::{Path, PathBuf};
use std::process::Command;

// The stencil library's *builder* is compiled into this script rather than into
// the toolchain: generating C and running a C compiler is something a build
// does once, and a `Level` ladder and a Mach-O reader are not things `buri`
// should carry at run time. The four modules below are the halves of
// `backend/stencil` that only this script compiles, plus the two — `abi` and
// `library` — that both compile, which is what keeps the emitter and the
// library it reads from disagreeing. `super::` resolves the same way in both
// module trees, which is why the paths inside them are written that way.
//
// `dead_code` is allowed on the four the script does not use *all* of, and the
// allow is here rather than in the files because that is where the fact is:
// `library.rs`'s decoder and `abi.rs`'s register cap are the toolchain's half,
// and `Level`'s lower rungs are the ladder the report measured along — the
// generators still read them, and a library is built at the top one.
#[allow(dead_code, reason = "the halves of these files only the toolchain uses")]
#[path = "src/compiler/backend/stencil/abi.rs"]
mod abi;
#[path = "src/compiler/backend/stencil/elfobj.rs"]
mod elfobj;
#[path = "src/compiler/backend/stencil/extract.rs"]
mod extract;
#[path = "src/compiler/backend/stencil/machobj.rs"]
mod machobj;
#[allow(dead_code, reason = "the halves of these files only the toolchain uses")]
#[path = "src/compiler/backend/stencil/x86.rs"]
mod x86;
#[allow(dead_code, reason = "the halves of these files only the toolchain uses")]
#[path = "src/compiler/backend/stencil/sources.rs"]
mod sources;
#[allow(dead_code, reason = "the halves of these files only the toolchain uses")]
#[path = "src/compiler/backend/stencil/library.rs"]
mod library;

// The toolchain's one hash, shared the same way and for a reason of the same
// shape. Both blobs written below enter a cache key **as their own digest** —
// the archive through `link_key`'s runtime term, the stencil library through
// `Backend::identity` — and a digest of bytes that cannot change after this
// script has written them has no business being recomputed by every process
// that later reads them. Ten megabytes of SHA-256 is about fifty-five
// milliseconds, paid once per `buri` invocation that reaches a native backend
// and paid *before* any cache lookup, so it lands on the no-op build as
// squarely as on the cold one.
//
// Shared rather than restated: `hash_bytes` here and `hash_bytes` in the
// toolchain must produce the same string, and the only way to be sure of that
// is for there to be one of them. `src/build/sha256.rs`'s header is the whole
// argument; `runtime_native::the_hash_is_of_the_bytes` is the assertion.
#[allow(dead_code, reason = "the streaming half of this file is the toolchain's")]
#[path = "src/build/sha256.rs"]
mod sha256;

fn main() {
    let manifest = PathBuf::from(env("CARGO_MANIFEST_DIR"));
    // Not in the rerun set by default: this script names its inputs, so
    // without this line an edit to the hash would leave a digest baked by the
    // old one beside bytes the new one reads differently.
    println!("cargo:rerun-if-changed={}", manifest.join("src/build/sha256.rs").display());
    runtime_archive(&manifest);
    stencil_library(&manifest);
}

/// Writes `<out>.sha256` beside a blob this script produced.
///
/// A file rather than a `cargo:rustc-env`: the bytes are reached with
/// `include_bytes!(concat!(env!("OUT_DIR"), …))` and the digest is reached with
/// `include_str!` of the same shape, so the two travel together and a stale
/// `OUT_DIR` cannot pair one with the other's digest.
///
/// Sixty-four hex digits and no newline, so that `include_str!` is the digest
/// and not the digest plus whitespace to trim.
fn digest_beside(out: &Path) {
    let bytes = match std::fs::read(out) {
        Ok(b) => b,
        Err(e) => fail(&format!("could not read back {} to hash it: {e}", out.display())),
    };
    let path = out.with_file_name(format!(
        "{}.sha256",
        out.file_name().and_then(|n| n.to_str()).unwrap_or_default()
    ));
    if let Err(e) = std::fs::write(&path, sha256::hash_bytes(&bytes)) {
        fail(&format!("could not write {}: {e}", path.display()));
    }
}

/// Writes `<out>.features` beside the runtime archive: the Cargo features it
/// was built with, one per line, and empty when there is no archive.
///
/// The toolchain reads it with `include_str!` of the same shape
/// [`digest_beside`]'s output is read with, and for the same reason — one
/// `OUT_DIR`, one run of this script, so the answer cannot be paired with
/// another build's bytes.
///
/// A list rather than a bool because `net-h3` is already planned: a second
/// feature is a second line here and a second `runtime_native::declares` call
/// there, and nothing about the file's shape has to change.
fn features_beside(out: &Path, features: &[&str]) {
    let path = out.with_file_name(format!(
        "{}.features",
        out.file_name().and_then(|n| n.to_str()).unwrap_or_default()
    ));
    if let Err(e) = std::fs::write(&path, features.join("\n")) {
        fail(&format!("could not write {}: {e}", path.display()));
    }
}

/// The three flags that reach `rustc` on the command line rather than through
/// `cli/runtime/manifest.toml`, because Cargo has no profile key for any of
/// them.
///
/// They are all for one property: two builds of the same tree must produce the
/// same archive, because `--check-reproducible` compares linked artifacts byte
/// for byte (ARCHITECTURE.md §7) and the archive is an input to every link.
///
/// * `--remap-path-prefix` keeps the checkout's absolute path out of the
///   panic locations the runtime's own `#[track_caller]` sites bake in. The
///   empty *from* prefix is not a typo and it is not a trick: Cargo runs
///   `rustc` with the package root as its working directory and passes the
///   crate root as the **relative** path `lib.rs`, so there is no absolute
///   prefix left to strip. An empty prefix matches every relative path and
///   nothing else — `Path::strip_prefix("")` fails on an absolute one — so
///   `abort.rs` becomes `./runtime/abort.rs`, which is exactly the string the
///   raw-`rustc` invocation produced, and the sysroot's own already-remapped
///   `/rustc/<hash>/library/...` paths are untouched.
/// * `-C metadata` and an empty `-C extra-filename` pin *our* half of the
///   crate disambiguator and the output file name. Cargo prepends a
///   disambiguator of its own — `-C metadata` is a list rustc hashes whole, so
///   ours does not replace it — and that one is derived from the package's
///   name, version and profile rather than from where the tree is checked out,
///   so the archive is still the same bytes at any path. The empty
///   `extra-filename` is what keeps the artifact at a name this script can
///   name: `<target-dir>/<triple>/release/deps/libburi_rt.a`, with no hash in
///   it, and with the archive's own member names free of one too.
///
/// **They are passed through `cargo rustc --`, which applies them to the
/// runtime crate alone, and not through `RUSTFLAGS`, which would apply them to
/// every crate in the tree.** That distinction was free while the tree was one
/// crate and is not free now: an empty `-C extra-filename` is a *collision* the
/// moment two versions of one crate appear, and two do — the runtime's lockfile
/// carries `getrandom` at 0.2 and 0.3 — so both would compile to
/// `deps/libgetrandom.rlib`.
const RUNTIME_RUSTC_ARGS: &[&str] =
    &["--remap-path-prefix==./runtime", "-Cmetadata=buri_rt", "-Cextra-filename="];

/// Copies `from` over `to`, **only if the bytes differ**.
///
/// Cargo's fingerprints are mtimes, so an unconditional copy would rebuild the
/// runtime — now a dependency tree rather than one crate — every time this
/// script reran for an unrelated reason, of which there are several: an edit to
/// `sha256.rs`, an edit to any stencil generator, a change to `CC`.
fn copy_if_different(from: &Path, to: &Path) {
    let bytes = match std::fs::read(from) {
        Ok(b) => b,
        Err(e) => fail(&format!("could not read {}: {e}", from.display())),
    };
    if std::fs::read(to).is_ok_and(|existing| existing == bytes) {
        return;
    }
    if let Err(e) = std::fs::write(to, &bytes) {
        fail(&format!("could not write {}: {e}", to.display()));
    }
}

/// Assembles the runtime's cargo package in `OUT_DIR` from the plain directory
/// of files `cli/runtime/` is, and answers where it put it.
///
/// The header's second bullet is the whole argument for why the package is
/// assembled rather than checked in. Mechanically it is three things: the
/// manifest and the lockfile under the names Cargo insists on, the `.rs` files
/// beside them, and the removal of anything left over from a previous build —
/// a source file deleted from `cli/runtime/` must not go on being compiled out
/// of a stale `OUT_DIR`.
fn assemble(runtime: &Path, out_dir: &Path) -> PathBuf {
    let pkg = out_dir.join("rt-pkg");
    if let Err(e) = std::fs::create_dir_all(&pkg) {
        fail(&format!("could not create {}: {e}", pkg.display()));
    }

    let mut wanted = vec![String::from("Cargo.toml"), String::from("Cargo.lock")];
    copy_if_different(&runtime.join("manifest.toml"), &pkg.join("Cargo.toml"));
    copy_if_different(&runtime.join("manifest.lock"), &pkg.join("Cargo.lock"));

    let entries = match std::fs::read_dir(runtime) {
        Ok(e) => e,
        Err(e) => fail(&format!("could not read {}: {e}", runtime.display())),
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_some_and(|e| e == "rs") {
            let name = entry.file_name().to_string_lossy().to_string();
            copy_if_different(&path, &pkg.join(&name));
            wanted.push(name);
        }
    }

    if let Ok(existing) = std::fs::read_dir(&pkg) {
        for entry in existing.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if entry.path().is_file() && !wanted.contains(&name) {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
    pkg
}

/// A `cargo` for the assembled package, with the parent invocation's state
/// taken out of its environment.
///
/// **Every `CARGO_*` variable but one, removed.** A build script runs inside a
/// cargo invocation and inherits its whole state: `CARGO_ENCODED_RUSTFLAGS`
/// would silently outrank `RUSTFLAGS` (it is the one Cargo reads first),
/// `CARGO_MAKEFLAGS` hands over a jobserver whose tokens this process is
/// holding, and `CARGO_TARGET_DIR` would point the nested build at the
/// workspace's target directory — whose lock the outer cargo owns for as long
/// as this script runs, which is a deadlock rather than a slowdown.
/// `--target-dir` says where instead, and the assembled package is its own
/// workspace root, so there are two independent reasons the nested build cannot
/// reach the outer lock.
///
/// `CARGO_HOME` is the exception, and it is deliberate: it is not build state,
/// it is *where cargo lives*. A sandboxed build — `nix build`'s
/// `buildRustPackage` is the one that matters here — points it at a writable
/// vendored directory precisely because `$HOME` is not writable, so clearing it
/// would turn a hermetic build into a failure. It cannot cause the deadlock
/// this loop exists to prevent: it names no target directory and carries no
/// jobserver.
///
/// `RUSTFLAGS` is **emptied** rather than left alone. A contributor's ambient
/// flags would make the archive a different archive on every machine, and the
/// flags this build does need go to the runtime crate alone
/// ([`RUNTIME_RUSTC_ARGS`]).
fn nested(cargo: &str, rustc: &str) -> Command {
    let mut command = Command::new(cargo);
    for (name, _) in std::env::vars() {
        if name.starts_with("CARGO_") && name != "CARGO_HOME" {
            command.env_remove(&name);
        }
    }
    command.env_remove("CARGO");
    command.env("RUSTC", rustc);
    command.env("RUSTFLAGS", "");
    command
}

/// Whether the runtime's dependency tree can be reached, and whether reaching
/// it needed the network.
///
/// `Some(true)` means the lockfile resolved with no network at all, which is
/// the warm case and the sandboxed one; `Some(false)` means it took a fetch.
/// `None` is the degradation: the tree is out of reach, and the caller writes
/// an empty archive rather than failing the toolchain's build.
///
/// Offline first, deliberately. `cargo fetch` without `--offline` updates the
/// registry index, which is a network round trip on every cold build script
/// even when every crate is already in `CARGO_HOME` — and the answer it would
/// give is the one `--offline` already gave.
///
/// `--locked` on both, because a lockfile the manifest has outgrown is not a
/// thing to resolve around silently: the archive's contents would stop being a
/// function of the tree. Regenerating it is `BURI_RUNTIME_RELOCK=1`.
fn resolves(cargo: &str, rustc: &str, pkg: &Path, target: &str) -> Option<bool> {
    for offline in [true, false] {
        let mut command = nested(cargo, rustc);
        command.arg("fetch").arg("--locked").arg("--manifest-path").arg(pkg.join("Cargo.toml"));
        command.args(["--target", target]);
        if offline {
            command.arg("--offline");
        }
        if command.status().is_ok_and(|s| s.success()) {
            return Some(offline);
        }
    }
    None
}

fn runtime_archive(manifest: &Path) {
    let runtime = manifest.join("runtime");
    let out_dir = PathBuf::from(env("OUT_DIR"));
    let out = out_dir.join("libburi_rt.a");

    // Cargo reruns this script when anything under `runtime/` changes, and
    // *only* then: without these lines it reruns on every change to any file in
    // the package, which would put a rustc invocation in front of every edit to
    // the compiler.
    println!("cargo:rerun-if-changed={}", runtime.display());
    for name in
        ["BURI_RUNTIME_CARGO", "BURI_RUNTIME_RUSTC", "BURI_RUNTIME_NET", "BURI_RUNTIME_RELOCK"]
    {
        println!("cargo:rerun-if-env-changed={name}");
    }

    let target = env("TARGET");
    if !supported(&target) {
        // Not an error. The archive is only reachable through the native
        // backends, and a host with no runtime is a host that has no native
        // backend to reach it from.
        // An empty archive rather than a `--cfg`: the emptiness *is* the
        // signal, `runtime_native::AVAILABLE` reads it, and there is no
        // conditional compilation for a `check-cfg` list to have to know about.
        write_empty(&out);
        // No archive is no features, and the file exists on every path because
        // the toolchain `include_str!`s it unconditionally.
        features_beside(&out, &[]);
        return;
    }

    // The cargo that is already running this script, unless something names
    // another. `CARGO` is cleared from the *child's* environment, so it is read
    // here while it is still there.
    let cargo = std::env::var("BURI_RUNTIME_CARGO")
        .or_else(|_| std::env::var("CARGO"))
        .unwrap_or_else(|_| "cargo".to_string());
    // Likewise the compiler: `RUSTC` is how Cargo is told which one to use, and
    // the toolchain's own is the right answer, so a nested build that picked a
    // different `rustc` off `PATH` would produce an archive built by a compiler
    // the rest of the binary was not.
    let rustc = std::env::var("BURI_RUNTIME_RUSTC")
        .or_else(|_| std::env::var("RUSTC"))
        .unwrap_or_else(|_| "rustc".to_string());

    let pkg = assemble(&runtime, &out_dir);
    let target_dir = out_dir.join("rt");
    // `BURI_RUNTIME_NET=0` is the runtime's `net` feature off: no dependency
    // tree, no resolution to fail, and the archive this repository had before
    // the feature existed. It is how the twenty-four-byte figure in
    // `manifest.toml` was measured and what a host with an unreachable registry
    // can fall back to by hand.
    let net = !matches!(std::env::var("BURI_RUNTIME_NET").as_deref(), Ok("0"));
    // The opt-in write-back path. Without `--locked` Cargo resolves and updates
    // the assembled package's `Cargo.lock`; this is what carries the result back
    // to the file a reviewer reads.
    let relock = std::env::var_os("BURI_RUNTIME_RELOCK").is_some();

    let offline = match (net, relock) {
        (false, _) => Some(true),
        (true, true) => Some(false),
        (true, false) => match resolves(&cargo, &rustc, &pkg, &target) {
            Some(offline) => Some(offline),
            None => {
                // The degradation the header's fifth bullet argues for. A
                // `cargo:warning` rather than silence, because the consequence
                // — no native backend — is one a contributor will otherwise
                // meet as a test that skipped.
                println!(
                    "cargo:warning=the runtime's dependency tree could not be resolved, so this \
                     toolchain has no native runtime archive and no native backend. Either the \
                     registry is unreachable and CARGO_HOME holds none of the crates, or \
                     cli/runtime/manifest.lock no longer matches manifest.toml — \
                     `BURI_RUNTIME_RELOCK=1 cargo build -p buri` fixes the second. \
                     `BURI_RUNTIME_NET=0` builds the runtime without the networking crates."
                );
                write_empty(&out);
                features_beside(&out, &[]);
                return;
            }
        },
    };

    let mut command = nested(&cargo, &rustc);
    // `cargo rustc`, not `cargo build`: the flags after `--` are for the crate
    // being built and not for its dependencies. See `RUNTIME_RUSTC_ARGS`.
    command.arg("rustc").arg("--lib");
    // `--release` is what selects `[profile.release]` in the runtime's
    // manifest: `lto = "fat"`, `panic = "abort"`, `codegen-units = 1`,
    // `debug = 0`. Each is argued where it is written.
    command.arg("--release");
    command.arg("--manifest-path").arg(pkg.join("Cargo.toml"));
    command.args(["--target", &target]);
    command.arg("--target-dir").arg(&target_dir);
    if !relock {
        command.arg("--locked");
    }
    if offline == Some(true) {
        command.arg("--offline");
    }
    if !net {
        command.arg("--no-default-features");
    }
    command.arg("--").args(RUNTIME_RUSTC_ARGS);

    match command.status() {
        Ok(s) if s.success() => {}
        // A tree that resolved and then failed to compile is a broken runtime
        // rather than a missing one, so this is a failure and not a degrade.
        Ok(s) => fail(&format!("{cargo} failed to build the runtime ({s})")),
        Err(e) => fail(&format!("could not run {cargo} to build the runtime: {e}")),
    }

    if relock {
        copy_if_different(&pkg.join("Cargo.lock"), &runtime.join("manifest.lock"));
    }

    // `deps/` rather than the profile directory above it: Cargo hard-links an
    // artifact up one level only when the copy in `deps/` has a different name,
    // and an empty `-C extra-filename` makes the two names the same, so the
    // uplift does not happen and `deps/` is where the archive actually is.
    let built = target_dir.join(&target).join("release/deps/libburi_rt.a");
    if let Err(e) = std::fs::copy(&built, &out) {
        fail(&format!(
            "cargo reported success but {} could not be copied to {}: {e}",
            built.display(),
            out.display()
        ));
    }
    digest_beside(&out);
    // What the build asked for and got: `--no-default-features` above is the
    // only way `net` is off, and a tree that resolved and compiled is one whose
    // features are the ones named on the command line.
    let features: &[&str] = if net { &["net"] } else { &[] };
    features_beside(&out, features);
}

// ---------------------------------------------------------------------------
// 2. The stencil library
// ---------------------------------------------------------------------------

/// Generates the copy-and-patch backend's stencils and writes the library into
/// `OUT_DIR`, for `backend::stencil` to `include_bytes!`.
///
/// This is the paper's §5.3 "stencil library builder", and it is here for the
/// same reason the runtime archive is: it is an **install-time** cost paid once
/// when the toolchain is built, not a cost inside the build loop the rest of
/// this design spends its effort shortening. Twenty-three thousand C functions
/// are about a second of `cc` across twelve shards; paying that per `buri
/// build` would be paying for a C compiler in order to avoid one.
///
/// Three properties, each a decision:
///
/// * **A host C compiler, not a crate.** `cc` is a platform interface in
///   exactly the sense the dependency bar in the workspace manifest means, and
///   it is not a Cargo dependency: nothing is added to the lockfile and nothing
///   is added to `cargo install buri` beyond a tool every machine that can link
///   a native artifact already has — `build/link.rs` shells out to the same one
///   to produce the artifact itself.
/// * **Degrades rather than breaks.** A host with no `cc`, or one that is not
///   arm64, gets an **empty** library; `stencil::AVAILABLE` reads the emptiness
///   and the backend reports itself unavailable, exactly as
///   `runtime_native::AVAILABLE` does for the archive. That is the third clause
///   of the dependency bar applied to a tool rather than to a crate, and it is
///   why this is a `return` and not a `fail`.
/// * **One library per target.** A stencil is the bytes `cc` emitted for a
///   function of a particular instruction set, in a particular container, so
///   it is not portable in any sense. Three are built —
///   [`abi::StencilTarget::ALL`] — and each is a separate blob with its own
///   baked digest, so a toolchain can have the host's and not the cross ones,
///   or all three, and `Stencil::identity` names whichever it has.
///
///   The two Linux libraries are **cross-compiled**: `clang -target
///   {aarch64,x86_64}-unknown-linux-gnu` with clang's own headers and no
///   sysroot, which works because the generated C includes `<stdint.h>` and
///   declares the one libc function it uses (`sources::memcpy_decl`). A host
///   whose `cc` cannot do that gets empty blobs for those two and a full one
///   for its own, which is `can_build`'s whole job.
fn stencil_library(manifest: &Path) {
    let dir = manifest.join("src/compiler/backend/stencil");
    for file in
        ["abi.rs", "library.rs", "sources.rs", "extract.rs", "machobj.rs", "elfobj.rs", "x86.rs"]
    {
        println!("cargo:rerun-if-changed={}", dir.join(file).display());
    }
    println!("cargo:rerun-if-env-changed=CC");

    let out_dir = PathBuf::from(env("OUT_DIR"));
    let blob = |t: abi::StencilTarget| out_dir.join(format!("stencils-{}.bin", t.slug()));
    let target = env("TARGET");
    let cc = std::env::var("CC").unwrap_or_else(|_| String::from("cc"));

    // A host with no C compiler, or one that is not a platform this toolchain
    // has a runtime for, has no stencil library of any kind. Every blob is
    // still written, because the emitter `include_bytes!`es all three by name.
    if !supported(&target) || !can_compile(&cc) {
        for t in abi::StencilTarget::ALL {
            write_empty(&blob(t));
        }
        return;
    }
    let scratch = out_dir.join("stencils");
    let jobs: usize = std::env::var("NUM_JOBS").ok().and_then(|v| v.parse().ok()).unwrap_or(4);
    for t in abi::StencilTarget::ALL {
        let out = blob(t);
        // The host library is only buildable on the host: `cc` without
        // `-target` compiles for the machine it is on, and `sources.rs` does
        // not pass one for `MacosArm64`.
        let host_ok = t != abi::StencilTarget::MacosArm64
            || (target.contains("-apple-darwin") && target.starts_with("aarch64"));
        if !host_ok || !sources::can_build(&cc, &scratch, t) {
            write_empty(&out);
            continue;
        }
        match sources::build(&cc, &scratch, jobs, t) {
            // A failure *after* `cc` has been shown to compile this target's
            // prelude is a bug in the generators, not a missing tool, so it
            // fails the build rather than degrading: a toolchain that silently
            // shipped no stencils because a generator stopped compiling would
            // be a silent loss of a backend.
            Err(e) => fail(&format!("stencil library ({}): {e}", t.slug())),
            Ok(lib) => {
                if let Err(e) = std::fs::write(&out, lib.encode()) {
                    fail(&format!("could not write {}: {e}", out.display()));
                }
                digest_beside(&out);
            }
        }
    }
}

/// Whether `cc` exists and can produce an object at all.
///
/// A version probe rather than a `which`: `cc` on a machine with the Xcode
/// command-line tools missing exists, is on `PATH`, and fails with a dialog.
fn can_compile(cc: &str) -> bool {
    Command::new(cc).arg("--version").output().map(|o| o.status.success()).unwrap_or(false)
}

/// macOS and Linux. The runtime is `std` over `cfg(unix)`, and no other host
/// has a native backend to link it into.
fn supported(target: &str) -> bool {
    target.contains("-apple-darwin") || target.contains("-linux-")
}

/// The blob a host with no runtime, no `cc` or no arm64 gets. The emptiness
/// *is* the signal (see both headers above), and it gets a digest too: the
/// digest of no bytes is a perfectly good identity for no bytes, and a missing
/// file would be an `include_str!` that does not compile on exactly the hosts
/// this branch exists to keep building.
fn write_empty(out: &Path) {
    if let Err(e) = std::fs::write(out, []) {
        fail(&format!("could not write {}: {e}", out.display()));
    }
    digest_beside(out);
}

fn env(name: &str) -> String {
    match std::env::var(name) {
        Ok(v) => v,
        Err(_) => fail(&format!("cargo did not set {name}")),
    }
}

fn fail(message: &str) -> ! {
    eprintln!("error: {message}");
    std::process::exit(1)
}
