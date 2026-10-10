//! Builds the copy-and-patch backend's stencil library into `OUT_DIR`, for
//! `backend::stencil` to `include_bytes!`. The runtime archive, the other
//! thing a native backend needs and cannot write itself, is `cli/build.rs`'s.
//!
//! A build script of this crate rather than of `buri` so the stencils build
//! beside the rest of the toolchain instead of in front of all of it.
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
// does once, and the generators and a Mach-O reader are not things `buri`
// should carry at run time. The four modules below are the halves of
// `backend/stencil` that only this script compiles, plus the two — `abi` and
// `library` — that both compile, which is what keeps the emitter and the
// library it reads from disagreeing. `super::` resolves the same way in both
// module trees, which is why the paths inside them are written that way.
//
// `dead_code` is allowed on the three the script does not use *all* of, and the
// allow is here rather than in the files because that is where the fact is:
// `library.rs`'s decoder and `abi.rs`'s register cap are the toolchain's half.
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
#[path = "src/compiler/backend/stencil/sources.rs"]
mod sources;
#[allow(dead_code, reason = "the halves of these files only the toolchain uses")]
#[path = "src/compiler/backend/stencil/library.rs"]
mod library;
// The toolchain's table hasher, which `library.rs` indexes the stencils with.
use buri_hash::hash;

// The toolchain's one hash. The library enters `Backend::identity` as its own
// digest, so the digest is taken here, where the bytes are written, rather than
// by every process that later reads them; `buri-hash`'s `sha256.rs` header is
// the argument.
use buri_hash::build::sha256;

fn main() {
    stencil_library(&PathBuf::from(env("CARGO_MANIFEST_DIR")));
}

/// Writes `bytes` to `path`, **only if what is there differs**: rustc records
/// an `include_bytes!` file as a dependency, so rewriting one with the bytes it
/// already holds would recompile this crate for nothing.
fn write_if_different(path: &Path, bytes: &[u8]) {
    if std::fs::read(path).is_ok_and(|existing| existing == bytes) {
        return;
    }
    if let Err(e) = std::fs::write(path, bytes) {
        fail(&format!("could not write {}: {e}", path.display()));
    }
}

/// Writes `<out>.sha256` beside a blob this script produced: sixty-four hex
/// digits and no newline, so that `include_str!` is the digest and not the
/// digest plus whitespace to trim.
fn digest_beside(out: &Path) {
    let bytes = match std::fs::read(out) {
        Ok(b) => b,
        Err(e) => fail(&format!("could not read back {} to hash it: {e}", out.display())),
    };
    let path = out.with_file_name(format!(
        "{}.sha256",
        out.file_name().and_then(|n| n.to_str()).unwrap_or_default()
    ));
    write_if_different(&path, sha256::hash_bytes(&bytes).as_bytes());
}
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
///   {aarch64,x86_64}-unknown-linux-musl` with clang's own headers and no
///   sysroot, which works because the generated C includes `<stdint.h>` and
///   declares the one libc function it uses (`sources::memcpy_decl`). A host
///   whose `cc` cannot do that gets empty blobs for those two and a full one
///   for its own, which is `can_build`'s whole job.
///
///   `-musl` and not `-gnu`, and the triple is `abi::StencilTarget::triple`'s
///   rather than this comment's: a stencil is bytes that get linked into an
///   artifact whose libc is musl, so naming gnu here would have been the one
///   place in the toolchain still describing a glibc Linux.
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
    // Which targets this `cc` can build, asked of all three at once: each probe
    // is a compile.
    let buildable: Vec<bool> = std::thread::scope(|scope| {
        let probes: Vec<_> = abi::StencilTarget::ALL
            .iter()
            .map(|t| {
                let (cc, scratch, target) = (&cc, &scratch, &target);
                scope.spawn(move || {
                    // The host library is only buildable on the host: `cc`
                    // without `-target` compiles for the machine it is on, and
                    // `sources.rs` does not pass one for `MacosArm64`.
                    let host_ok = *t != abi::StencilTarget::MacosArm64
                        || (target.contains("-apple-darwin") && target.starts_with("aarch64"));
                    host_ok && sources::can_build(cc, scratch, *t)
                })
            })
            .collect();
        probes.into_iter().map(|p| p.join().unwrap_or(false)).collect()
    });
    let wanted: Vec<abi::StencilTarget> = abi::StencilTarget::ALL
        .iter()
        .zip(&buildable)
        .filter(|(_, ok)| **ok)
        .map(|(t, _)| *t)
        .collect();
    let mut built = sources::build_all(&cc, &scratch, jobs, &wanted).into_iter();
    for (t, ok) in abi::StencilTarget::ALL.iter().zip(&buildable) {
        let out = blob(*t);
        if !*ok {
            write_empty(&out);
            continue;
        }
        match built.next() {
            // A failure *after* `cc` has been shown to compile this target's
            // prelude is a bug in the generators, not a missing tool, so it
            // fails the build rather than degrading: a toolchain that silently
            // shipped no stencils because a generator stopped compiling would
            // be a silent loss of a backend.
            Some(Err(e)) => fail(&format!("stencil library ({}): {e}", t.slug())),
            None => fail(&format!("stencil library ({}): no result", t.slug())),
            Some(Ok(lib)) => {
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
    write_if_different(out, &[]);
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
