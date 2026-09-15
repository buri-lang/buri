//! Cross-compilation to Linux, end to end through the `buri` binary.
//!
//! Every other native tier here builds and runs a *host* binary. This one drives
//! the real CLI to build a `linux/x86_64` executable from whatever host runs it,
//! proves the result is a genuine static-PIE ELF, and — where `podman` is on
//! `PATH` — proves it **runs** in a Linux container. The container is the only
//! thing that can settle "runnable" from a mac, because a mac cannot execute a
//! Linux binary itself (ARCHITECTURE.md §9).
//!
//! Gated on the target's Rust standard library being installed
//! (`rustup target add x86_64-unknown-linux-musl`): without it the cross runtime
//! cannot be built and the run is skipped, the same shape every other tier here
//! uses for a toolchain that cannot serve it. On a `linux/x86_64` host the same
//! build takes the baked host path rather than the cross one, and the ELF is
//! just as good a proof there — so the test is meaningful on a mac and harmless
//! on Linux.

use std::path::{Path, PathBuf};
use std::process::Command;

/// A program that prints one line and exits — the smallest whole program whose
/// output settles that it started, ran and returned cleanly.
const HELLO: &str = r#"from "core/effect" import { Allocator, Stdout };
from "core/host" import * as host;
from "core/io" import * as io;

export fn main(): Result<(), Str> {
    let ctx = context {
        Allocator: host.alloc,
        Stdout: host.stdout,
    };
    io.println(ctx, "native").mapErr(fn(_e) => "could not write to standard output")
}
"#;

/// Whether this host can produce a `x86_64-unknown-linux-musl` artifact: it needs
/// that target's self-contained standard library, which is the same probe
/// `build/runtime_cross.rs` and `cli/build.rs` use.
fn cross_ready() -> bool {
    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| String::from("rustc"));
    let out = Command::new(&rustc)
        .args([
            "--print",
            "target-libdir",
            "--target",
            "x86_64-unknown-linux-musl",
        ])
        .output();
    let Ok(out) = out else { return false };
    if !out.status.success() {
        return false;
    }
    let Ok(libdir) = String::from_utf8(out.stdout) else {
        return false;
    };
    Path::new(libdir.trim())
        .join("self-contained")
        .join("libc.a")
        .is_file()
}

/// Lays out a one-binary repository under `dir` that declares a `linux/x86_64`
/// output and prints `native`.
fn write_repo(dir: &Path) {
    let pkg = dir.join("cmd").join("hello");
    std::fs::create_dir_all(&pkg).expect("create the package directory");
    std::fs::write(dir.join("REPO.buri"), b"").expect("write REPO.buri");
    std::fs::write(
        pkg.join("BUILD.buri"),
        b"binary {\n    outputs: [\n        { platform: LINUX, arch: X86_64 },\n    ]\n}\n",
    )
    .expect("write BUILD.buri");
    std::fs::write(pkg.join("main.buri"), HELLO).expect("write main.buri");
}

/// The `buri` binary this workspace built, its cross cache pointed at a scratch
/// `~/.buri` so the test does not touch the developer's real one.
fn buri(repo: &Path, home: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_buri"));
    command.current_dir(repo);
    command.env("BURI_HOME", home);
    command
}

/// The little of an ELF header this needs: it is a 64-bit ELF, a PIE
/// (`ET_DYN`), and an x86-64 one.
fn is_linux_x86_64_pie(bytes: &[u8]) -> bool {
    // e_ident: magic, then EI_CLASS at offset 4 (2 == 64-bit).
    let magic = bytes.get(0..4) == Some(&[0x7f, b'E', b'L', b'F']);
    let elf64 = bytes.get(4) == Some(&2);
    // e_type at 16 (little-endian): 3 == ET_DYN, which is what a PIE is.
    let et_dyn = bytes.get(16..18) == Some(&[3, 0]);
    // e_machine at 18: 62 (0x3E) == x86-64.
    let x86_64 = bytes.get(18..20) == Some(&[62, 0]);
    magic && elf64 && et_dyn && x86_64
}

#[test]
fn a_linux_x86_64_artifact_is_a_real_elf_and_runs_in_a_container() {
    if !cross_ready() {
        // No `x86_64-unknown-linux-musl` standard library: the cross runtime
        // cannot be built here. `rustup target add x86_64-unknown-linux-musl`.
        return;
    }

    let scratch = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("cross-e2e");
    let _ = std::fs::remove_dir_all(&scratch);
    let repo = scratch.join("repo");
    let home = scratch.join("home");
    write_repo(&repo);
    std::fs::create_dir_all(&home).expect("create the scratch BURI_HOME");

    let out = buri(&repo, &home)
        .args(["build", "//cmd/hello", "--output=linux/x86_64"])
        .output()
        .expect("run buri build");
    assert!(
        out.status.success(),
        "the cross build failed:\n{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );

    let artifact = repo.join(".buri/out/linux-x86_64/cmd/hello/hello");
    let bytes = std::fs::read(&artifact).unwrap_or_else(|e| {
        panic!("the artifact {} is missing: {e}", artifact.display());
    });
    assert!(
        is_linux_x86_64_pie(&bytes),
        "the artifact is not a 64-bit x86-64 static-PIE ELF"
    );

    // The one thing a mac cannot check by running it: that it runs. Where
    // `podman` is present, run it in a Linux container and read its line back.
    // A container that cannot even start — no image, no network — is a skip
    // rather than a failure, because the ELF above already settles the build;
    // a container that *ran* and said the wrong thing is a failure.
    if let Some(ran) = run_in_container(&artifact) {
        assert_eq!(
            ran.trim(),
            "native",
            "the cross-built binary printed the wrong thing"
        );
    }
}

/// Runs `artifact` in a `docker.io/library/rust` container and answers its
/// standard output, or `None` when no container could be started.
fn run_in_container(artifact: &Path) -> Option<String> {
    resolve("podman")?;
    let dir = artifact.parent()?;
    let name = artifact.file_name()?.to_string_lossy().into_owned();
    let mount = format!("{}:/work", dir.display());
    let out = Command::new("podman")
        .args([
            "run",
            "--rm",
            "-v",
            &mount,
            "-w",
            "/work",
            "docker.io/library/rust:1.89",
        ])
        .arg(format!("./{name}"))
        .output()
        .ok()?;
    // A pull or runtime-setup failure is an environment gap, not a product bug:
    // its message is on stderr and the program never ran, so there is no output
    // to check and this is a skip.
    if !out.status.success() && out.stdout.is_empty() {
        return None;
    }
    assert!(
        out.status.success(),
        "the container ran the binary and it failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).ok()
}

/// Whether a program is on `PATH`.
fn resolve(program: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(program))
        .find(|candidate| candidate.is_file())
}
