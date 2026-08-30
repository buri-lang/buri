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
//! cargo build --release --manifest-path cli/runtime/Cargo.toml \
//!             --target <host triple> --target-dir $OUT_DIR/rt
//! ```
//!
//! Three properties are worth naming, because each of them is a decision:
//!
//! * **No build dependency, and now a manifest with none either.** `cargo` and
//!   `rustc` are already required to build the toolchain, so driving the one
//!   that is already running adds no tool, nothing to the toolchain's
//!   lockfile, and nothing to `cargo install buri`. The obvious alternative —
//!   the `cc` crate and a runtime written in C — costs a build dependency *and*
//!   makes a C compiler a build-time requirement rather than a link-time one,
//!   which is heavier than the Rust dependency the toolchain already has
//!   (VALUE-MODEL.md §10).
//!
//!   What changed is only *who spells the flags*. This was a raw `rustc`
//!   command line over `cli/runtime/lib.rs`, because fifteen `.rs` files and no
//!   manifest was the smallest thing that could work. It is now a package —
//!   `cli/runtime/Cargo.toml`, **zero dependencies**, outside the workspace
//!   beside `editors/zed` — and the optimization flags live in its
//!   `[profile.release]` where a reader looks for them. The bar the archive is
//!   held to is *stricter* than the toolchain's, not looser: the archive is
//!   linked into every native binary this compiler produces, so a crate
//!   admitted there is a crate shipped in every user's program.
//!   `dependencies_stay_behind_the_bar` reads that manifest as well as
//!   `cli/Cargo.toml`, so the emptiness is a test rather than a habit.
//!
//!   One cost the manifest carries, stated where it will be read rather than
//!   discovered: `cargo package -p buri` **skips a directory that contains a
//!   `Cargo.toml` of its own**, unconditionally and with no `include` that can
//!   override it, so the fifteen runtime sources that used to ride inside the
//!   published `buri` crate no longer do. A checkout builds; a `cargo install
//!   buri` from a registry tarball would not. That is a packaging decision and
//!   not a compiler one, and it has the answer ARCHITECTURE.md §9 already
//!   writes for the same shape of problem — publish the runtime as its own
//!   crate, or ship prebuilt archives per triple — which is a change to make
//!   deliberately rather than as a side effect of this one.
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

/// The three flags that stay on the command line rather than moving into
/// `cli/runtime/Cargo.toml`, because Cargo has no profile key for any of them.
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
const RUNTIME_RUSTFLAGS: &str = "--remap-path-prefix==./runtime \
                                 -Cmetadata=buri_rt \
                                 -Cextra-filename=";

fn runtime_archive(manifest: &Path) {
    let runtime = manifest.join("runtime");
    let out_dir = PathBuf::from(env("OUT_DIR"));
    let out = out_dir.join("libburi_rt.a");

    // Cargo reruns this script when anything under `runtime/` changes, and
    // *only* then: without these lines it reruns on every change to any file in
    // the package, which would put a rustc invocation in front of every edit to
    // the compiler.
    println!("cargo:rerun-if-changed={}", runtime.display());
    println!("cargo:rerun-if-env-changed=BURI_RUNTIME_CARGO");
    println!("cargo:rerun-if-env-changed=BURI_RUNTIME_RUSTC");

    let target = env("TARGET");
    if !supported(&target) {
        // Not an error. The archive is only reachable through the native
        // backends, and a host with no runtime is a host that has no native
        // backend to reach it from.
        // An empty archive rather than a `--cfg`: the emptiness *is* the
        // signal, `runtime_native::AVAILABLE` reads it, and there is no
        // conditional compilation for a `check-cfg` list to have to know about.
        write_empty(&out);
        return;
    }

    // The cargo that is already running this script, unless something names
    // another. `CARGO` is cleared from the *child's* environment below, so it
    // is read here while it is still there.
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

    let target_dir = out_dir.join("rt");
    let mut command = Command::new(&cargo);

    // **Every `CARGO_*` variable but one, removed.** A build script runs inside
    // a cargo invocation and inherits its whole state: `CARGO_ENCODED_RUSTFLAGS`
    // would silently outrank the `RUSTFLAGS` set below (it is the one Cargo
    // reads first), `CARGO_MAKEFLAGS` hands over a jobserver whose tokens this
    // process is holding, and `CARGO_TARGET_DIR` would point the nested build
    // at the workspace's target directory — whose lock the outer cargo owns for
    // as long as this script runs, which is a deadlock rather than a slowdown.
    // `--target-dir` below says where instead, and `cli/runtime` is its own
    // workspace root, so there are two independent reasons the nested build
    // cannot reach the outer lock.
    //
    // `CARGO_HOME` is the exception, and it is deliberate: it is not build
    // state, it is *where cargo lives*. A sandboxed build — `nix build`'s
    // `buildRustPackage` is the one that matters here — points it at a writable
    // vendored directory precisely because `$HOME` is not writable, so clearing
    // it would turn a hermetic build into a failure. It cannot cause the
    // deadlock this loop exists to prevent: it names no target directory and
    // carries no jobserver.
    for (name, _) in std::env::vars() {
        if name.starts_with("CARGO_") && name != "CARGO_HOME" {
            command.env_remove(&name);
        }
    }
    command.env_remove("CARGO");

    let status = command
        .env("RUSTC", &rustc)
        // Set rather than appended: an archive built with a contributor's
        // ambient `RUSTFLAGS` would be a different archive on every machine,
        // and the raw `rustc` invocation this replaces never read them either.
        .env("RUSTFLAGS", RUNTIME_RUSTFLAGS)
        .arg("build")
        // `--release` is what selects `[profile.release]` in the runtime's
        // manifest: `lto = "fat"`, `panic = "abort"`, `codegen-units = 1`,
        // `debug = 0`. Each is argued where it is written.
        .arg("--release")
        .arg("--manifest-path")
        .arg(runtime.join("Cargo.toml"))
        .args(["--target", &target])
        .arg("--target-dir")
        .arg(&target_dir)
        .status();

    match status {
        Ok(s) if s.success() => {}
        Ok(s) => fail(&format!("{cargo} failed to build cli/runtime ({s})")),
        Err(e) => fail(&format!("could not run {cargo} to build cli/runtime: {e}")),
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
