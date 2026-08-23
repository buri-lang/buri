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
//! rustc --crate-type=staticlib --edition 2024 -C opt-level=3 -C panic=abort \
//!       --target <host triple> -o $OUT_DIR/libburi_rt.a cli/runtime/lib.rs
//! ```
//!
//! Three properties are worth naming, because each of them is a decision:
//!
//! * **No build dependency.** `rustc` is already required to build the
//!   toolchain, so shelling out to it adds no tool, nothing to the lockfile,
//!   and nothing to `cargo install buri`. The obvious alternative — the `cc`
//!   crate and a runtime written in C — costs a build dependency *and* makes a
//!   C compiler a build-time requirement rather than a link-time one, which is
//!   heavier than the Rust dependency the toolchain already has
//!   (VALUE-MODEL.md §10).
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
// `backend/cpjit` that only this script compiles, plus the two — `abi` and
// `stencil` — that both compile, which is what keeps the emitter and the
// library it reads from disagreeing. `super::` resolves the same way in both
// module trees, which is why the paths inside them are written that way.
//
// `dead_code` is allowed on the four the script does not use *all* of, and the
// allow is here rather than in the files because that is where the fact is:
// `stencil.rs`'s decoder and `abi.rs`'s register cap are the toolchain's half,
// and `Level`'s lower rungs are the ladder the report measured along — the
// generators still read them, and a library is built at the top one.
#[allow(dead_code, reason = "the halves of these files only the toolchain uses")]
#[path = "src/compiler/backend/cpjit/abi.rs"]
mod abi;
#[path = "src/compiler/backend/cpjit/extract.rs"]
mod extract;
#[path = "src/compiler/backend/cpjit/machobj.rs"]
mod machobj;
#[allow(dead_code, reason = "the halves of these files only the toolchain uses")]
#[path = "src/compiler/backend/cpjit/sources.rs"]
mod sources;
#[allow(dead_code, reason = "the halves of these files only the toolchain uses")]
#[path = "src/compiler/backend/cpjit/stencil.rs"]
mod stencil;

fn main() {
    let manifest = PathBuf::from(env("CARGO_MANIFEST_DIR"));
    runtime_archive(&manifest);
    stencil_library(&manifest);
}

fn runtime_archive(manifest: &Path) {
    let runtime = manifest.join("runtime");
    let out = PathBuf::from(env("OUT_DIR")).join("libburi_rt.a");

    // Cargo reruns this script when anything under `runtime/` changes, and
    // *only* then: without these lines it reruns on every change to any file in
    // the package, which would put a rustc invocation in front of every edit to
    // the compiler.
    println!("cargo:rerun-if-changed={}", runtime.display());
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

    let rustc = std::env::var("BURI_RUNTIME_RUSTC")
        .or_else(|_| std::env::var("RUSTC"))
        .unwrap_or_else(|_| "rustc".to_string());

    let status = Command::new(&rustc)
        .arg("--crate-type=staticlib")
        .arg("--crate-name=buri_rt")
        .args(["--edition", "2024"])
        .args(["-C", "opt-level=3"])
        // No unwinder: an abort in this language is a write to standard error
        // and an exit, never an unwind (SPEC 6.10), so the tables would be dead
        // weight in every artifact.
        .args(["-C", "panic=abort"])
        .args(["-C", "debuginfo=0"])
        // Whole-program LTO over the runtime *and* the copy of `std` a
        // staticlib bundles. It is not an optimization decision — it is a size
        // one, and the size is embedded in every `buri` binary: measured on
        // aarch64-apple-darwin, 17.7 MB without it and 6.0 MB with, for 2.6
        // seconds of build time once per toolchain build.
        .args(["-C", "lto=fat"])
        // Three flags, all for one property: two builds of the same tree must
        // produce the same archive, because `--check-reproducible` compares
        // linked artifacts byte for byte (ARCHITECTURE.md §7) and the archive
        // is an input to every link.
        //
        // `-C metadata` and an empty `-C extra-filename` pin the symbol hash,
        // which otherwise varies with the output path — so an archive built in
        // `target/debug` and one built in `target/release` differed by a few
        // dozen bytes, and the difference was invisible until something hashed
        // it. `--remap-path-prefix` keeps the checkout's absolute path out.
        .args(["-C", "codegen-units=1"])
        .args(["-C", "metadata=buri_rt"])
        .args(["-C", "extra-filename="])
        .arg(format!("--remap-path-prefix={}=.", manifest.display()))
        .args(["--target", &target])
        .arg("-o")
        .arg(&out)
        .arg(runtime.join("lib.rs"))
        .status();

    match status {
        Ok(s) if s.success() => {}
        Ok(s) => fail(&format!("{rustc} failed to build cli/runtime ({s})")),
        Err(e) => fail(&format!("could not run {rustc} to build cli/runtime: {e}")),
    }

    if !out.exists() {
        fail("rustc reported success but produced no libburi_rt.a");
    }
}

// ---------------------------------------------------------------------------
// 2. The stencil library
// ---------------------------------------------------------------------------

/// Generates the copy-and-patch backend's stencils and writes the library into
/// `OUT_DIR`, for `backend::cpjit` to `include_bytes!`.
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
///   arm64, gets an **empty** library; `cpjit::AVAILABLE` reads the emptiness
///   and the backend reports itself unavailable, exactly as
///   `runtime_native::AVAILABLE` does for the archive. That is the third clause
///   of the dependency bar applied to a tool rather than to a crate, and it is
///   why this is a `return` and not a `fail`.
/// * **The stencils are arm64.** They are the bytes `cc` emitted for arm64
///   functions, so they are not portable in any sense — an x86-64 seat needs
///   its own generators, and `design/native/CODEGEN-CPJIT.md` says so. Hence
///   the architecture test rather than only the platform one.
fn stencil_library(manifest: &Path) {
    let dir = manifest.join("src/compiler/backend/cpjit");
    for file in ["abi.rs", "stencil.rs", "sources.rs", "extract.rs", "machobj.rs"] {
        println!("cargo:rerun-if-changed={}", dir.join(file).display());
    }
    println!("cargo:rerun-if-env-changed=CC");

    let out = PathBuf::from(env("OUT_DIR")).join("cpjit-stencils.bin");
    let target = env("TARGET");
    if !supported(&target) || !target.starts_with("aarch64") {
        write_empty(&out);
        return;
    }
    let cc = std::env::var("CC").unwrap_or_else(|_| String::from("cc"));
    if !can_compile(&cc) {
        write_empty(&out);
        return;
    }
    let scratch = PathBuf::from(env("OUT_DIR")).join("cpjit-stencils");
    let jobs: usize = std::env::var("NUM_JOBS").ok().and_then(|v| v.parse().ok()).unwrap_or(4);
    match sources::build(&cc, &scratch, jobs) {
        // A failure *after* `cc` has been shown to work is a bug in the
        // generators, not a missing tool, so it fails the build rather than
        // degrading: a toolchain that silently shipped no stencils because a
        // generator stopped compiling would be a silent loss of a backend.
        Err(e) => fail(&format!("cpjit stencil library: {e}")),
        Ok(lib) => {
            if let Err(e) = std::fs::write(&out, lib.encode()) {
                fail(&format!("could not write {}: {e}", out.display()));
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

fn write_empty(out: &Path) {
    if let Err(e) = std::fs::write(out, []) {
        fail(&format!("could not write {}: {e}", out.display()));
    }
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
