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

use crate::hang::Capped;
use std::path::{Path, PathBuf};
use std::process::Command;

/// A program that prints one line and exits — the smallest whole program whose
/// output settles that it started, ran and returned cleanly.
const HELLO: &str = r#"from "platform/effect" import { Allocator, Stdout };
from "native" import { NativeHost };
from "core/io" import * as io;

export fn main(host: NativeHost): Result<(), Str> {
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
        .capped_output();
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
        b"binary {\n    outputs: [\n        { platform: \"native\", variant: \"linux-x86_64\" },\n    ]\n}\n",
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

    // This process's own, so two runs of the suite in one checkout don't
    // delete each other's repository mid-build. `sweep` takes old ones.
    let scratch = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("cross-e2e-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&scratch);
    let repo = scratch.join("repo");
    write_repo(&repo);
    // The home is kept from run to run and shared with every other test that
    // links for Linux, so the release build of the runtime for the target is
    // paid once rather than every run (`kept.rs`).
    let home = crate::sweep::kept::shared_cross_home();
    crate::sweep::kept::warm_cross_runtime();

    let out = buri(&repo, home)
        .args(["build", "//cmd/hello", "--output=native/linux-x86_64"])
        .capped_output()
        .expect("run buri build");
    assert!(
        out.status.success(),
        "the cross build failed:\n{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );

    let artifact = repo.join(".buri/out/native/linux-x86_64/cmd/hello/hello");
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

/// A generator writing the module `word`, which exports `word`: the text of
/// its one input.
const WORD_TOOL: &str = r#"from "core/buri/ast" import * as ast;
from "core/str" import * as str;
from "core/tool" import { Generated, GenerateRequest };
from "platform/effect" import { Allocator };

export fn generate<C: Allocator>(ctx: C, request: GenerateRequest<Str>): Generated {
    let text = request.inputs.get(0).map(fn(i) => i.value).withDefault("").trim();
    let source = str.format(ctx, "export let word: Str = \"${text}\";\n");
    let modules = match (ast.parse(ctx, "", source)) {
        .Ok(parsed) => [("word", parsed)],
        .Err(_) => [],
    };
    Generated { modules: modules, diagnostics: [], needs: [] }
}
"#;

/// A program printing the generated `word`.
const WORD_PROGRAM: &str = r#"from "platform/effect" import { Allocator, Stdout };
from "native" import { NativeHost };
from "core/io" import * as io;
from "//lib/wire" import { word };

export fn main(host: NativeHost): Result<(), Str> {
    let ctx = context {
        Allocator: host.alloc,
        Stdout: host.stdout,
    };
    io.println(ctx, word).mapErr(fn(_e) => "could not write to standard output")
}
"#;

#[test]
fn a_cross_build_runs_its_generator_on_the_host() {
    if !cross_ready() {
        return;
    }
    let scratch = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("cross-generator-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&scratch);
    let repo = scratch.join("repo");
    let files = [
        ("REPO.buri", ""),
        ("tools/gen/BUILD.buri", "tool {\n    generate {}\n}\n"),
        ("tools/gen/tool.buri", WORD_TOOL),
        (
            "lib/wire/BUILD.buri",
            "library {\n    generators: [\n        { tool: \"//tools/gen\", inputs: [\"word.txt\"] },\n    ]\n\n    visibility: [\"//visibility:public\"]\n}\n",
        ),
        ("lib/wire/word.txt", "generated\n"),
        ("lib/wire/lib.buri", "from \"//lib/wire/word\" export { word };\n"),
        (
            "cmd/hello/BUILD.buri",
            "binary {\n    dependencies: [\"//lib/wire\"]\n\n    outputs: [\n        { platform: \"native\", variant: \"linux-x86_64\" },\n    ]\n}\n",
        ),
        ("cmd/hello/main.buri", WORD_PROGRAM),
    ];
    for (path, text) in files {
        let path = repo.join(path);
        std::fs::create_dir_all(path.parent().expect("a parent")).expect("create a directory");
        std::fs::write(path, text).expect("write a file");
    }
    let home = crate::sweep::kept::shared_cross_home();
    crate::sweep::kept::warm_cross_runtime();

    let out = buri(&repo, home)
        .args(["build", "//cmd/hello", "--output=native/linux-x86_64"])
        .capped_output()
        .expect("run buri build");
    assert!(
        out.status.success(),
        "the cross build with a generator failed:\n{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let artifact = repo.join(".buri/out/native/linux-x86_64/cmd/hello/hello");
    let bytes = std::fs::read(&artifact).expect("the cross-built artifact");
    assert!(
        is_linux_x86_64_pie(&bytes),
        "the artifact is not a 64-bit x86-64 static-PIE ELF"
    );
    if let Some(ran) = run_in_container(&artifact) {
        assert_eq!(
            ran.trim(),
            "generated",
            "the generated module did not reach the binary"
        );
    }
    let _ = std::fs::remove_dir_all(&scratch);
}

/// A library whose function hands `map` a lambda over its argument.
const SHIFT_LIBRARY: &str = r#"from "platform/effect" import { Allocator };

export fn shift<C: Allocator>(ctx: C, xs: [Int], by: Int): [Int] {
    xs.map(ctx, fn(x) => x + by)
}
"#;

/// A program that hands `map` a lambda over a local too, and prints `33`.
const SHIFT_PROGRAM: &str = r#"from "platform/effect" import { Allocator, Stdout };
from "native" import { NativeHost };
from "core/io" import * as io;
from "//lib/shift" import { shift };

export fn main(host: NativeHost): Result<(), Str> {
    let ctx = context {
        Allocator: host.alloc,
        Stdout: host.stdout,
    };
    let k = shift(ctx, [1, 2, 3], 4).length();
    let ys = [5, 6].map(ctx, fn(x) => x * k);
    io.println(ctx, "${ys.sum()}").mapErr(fn(_e) => "could not write to standard output")
}
"#;

/// Lambdas over a value, handed on as values in two packages, cross-build to an
/// x86-64 Linux executable that prints what they compute.
#[test]
fn closures_in_two_packages_cross_build_for_linux_x86_64() {
    if !cross_ready() {
        return;
    }
    let scratch = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("cross-closures-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&scratch);
    let repo = scratch.join("repo");
    let files = [
        ("REPO.buri", ""),
        (
            "lib/shift/BUILD.buri",
            "library {\n    visibility: [\"//visibility:public\"]\n}\n",
        ),
        ("lib/shift/lib.buri", SHIFT_LIBRARY),
        (
            "cmd/hello/BUILD.buri",
            "binary {\n    dependencies: [\"//lib/shift\"]\n\n    outputs: [\n        { platform: \"native\", variant: \"linux-x86_64\" },\n    ]\n}\n",
        ),
        ("cmd/hello/main.buri", SHIFT_PROGRAM),
    ];
    for (path, text) in files {
        let path = repo.join(path);
        std::fs::create_dir_all(path.parent().expect("a parent")).expect("create a directory");
        std::fs::write(path, text).expect("write a file");
    }
    let home = crate::sweep::kept::shared_cross_home();
    crate::sweep::kept::warm_cross_runtime();

    let out = buri(&repo, home)
        .args(["build", "//cmd/hello", "--output=native/linux-x86_64"])
        .capped_output()
        .expect("run buri build");
    assert!(
        out.status.success(),
        "the cross build with closures failed:\n{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let artifact = repo.join(".buri/out/native/linux-x86_64/cmd/hello/hello");
    let bytes = std::fs::read(&artifact).expect("the cross-built artifact");
    assert!(
        is_linux_x86_64_pie(&bytes),
        "the artifact is not a 64-bit x86-64 static-PIE ELF"
    );
    if let Some(ran) = run_in_container(&artifact) {
        assert_eq!(ran.trim(), "33", "the closures computed the wrong thing");
    }
    let _ = std::fs::remove_dir_all(&scratch);
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
        .capped_output()
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
