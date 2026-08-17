//! Builds `cli/runtime` into `libburi_rt.a` for the host, so that
//! `backend::runtime_native` can `include_bytes!` it.
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

fn main() {
    let manifest = PathBuf::from(env("CARGO_MANIFEST_DIR"));
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
