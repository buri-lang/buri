//! A Linux link hands mold only the members of the baked archives a program
//! can need (`design/PERFORMANCE.md` §6.146).
//!
//! mold reads every member of every archive it is handed, so a link handed the
//! whole runtime archive, `libc.a` and `libunwind.a` paid for all of them.
//! Each program here is built on every native backend, run, and its
//! `BURI_PROFILE=1` count of the members handed over is held to a bound. The
//! count is of what the linker was handed, not a timing.

use crate::hang::Capped;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Prints one line.
const HELLO: &str = r#"from "core/io" import * as io;
from "native" import { NativeHost };
from "platform/effect" import { Stdout };

export fn main(host: NativeHost): Result<(), Str> {
    let ctx = context { Stdout: host.stdout };
    let _said = io.println(ctx, "hello").ignore();
    .Ok(())
}
"#;

/// Binds a server on a port of its own, says which, and stops.
const SERVES: &str = r#"from "core/io" import * as io;
from "core/net/http" import * as http;
from "core/net/server" import * as server;
from "native" import { NativeHost };
from "platform/effect" import { Allocator, Clock, Listen, Stdout, Tasks };

export fn main(host: NativeHost): Result<(), Str> {
    let ctx = context {
        Allocator: host.alloc,
        Clock: host.clock,
        Listen: host.listen,
        Stdout: host.stdout,
        Tasks: host.tasks,
    };
    let plan = server.Server {
        port: 0,
        onRequest: fn(c, request) => http.text(c, request.path()),
        drain: .None,
    };
    match (server.bind(ctx, plan)) {
        .Err(e) => .Err(server.errorText(e)),
        .Ok(listener) => {
            let _said = io.println(ctx, "bound ${listener.port > 0}").ignore();
            .Ok(())
        }
    }
}
"#;

/// The most members a program that only prints may be handed. A whole link
/// hands about 1,900; such a program needed 362 when this was written.
const HELLO_MOST: u64 = 600;

fn write(root: &Path, rel: &str, text: &str) {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

/// `source` as `//cmd/app` in a repository of its own, built on `backend`
/// with `mode`, with `~/.buri` at `home` if one is given: its binary, and the
/// `BURI_PROFILE=1` counts the build printed.
fn built(
    name: &str,
    source: &str,
    backend: &str,
    mode: &[&str],
    home: Option<&Path>,
) -> (PathBuf, String) {
    let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("archive-members-{}", std::process::id()))
        .join(format!("{name}-{backend}"));
    let _ = std::fs::remove_dir_all(&root);
    write(&root, "REPO.buri", "");
    let variant = format!(
        "linux-{}",
        if cfg!(target_arch = "aarch64") {
            "arm64"
        } else {
            "x86_64"
        }
    );
    write(
        &root,
        "cmd/app/BUILD.buri",
        &format!(
            "binary {{\n  outputs: [{{ platform: \"native\", variant: \"{variant}\" }}]\n}}\n"
        ),
    );
    write(&root, "cmd/app/main.buri", source);
    let mut command = Command::new(env!("CARGO_BIN_EXE_buri"));
    command
        .current_dir(&root)
        .args(["build", "//cmd/app"])
        .args(mode)
        .env("BURI_PROFILE", "1");
    if let Some(home) = home {
        command.env("BURI_HOME", home);
    }
    let out = command.capped_output().unwrap();
    let all = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    if !out.status.success() {
        crate::ci::native_program_failed("archive_members", "the build failed", &all);
    }
    (
        root.join(format!(".buri/out/native/{variant}/cmd/app/app")),
        all,
    )
}

/// The count `counts`, a build's `BURI_PROFILE=1` output, gives for `name`.
fn count(counts: &str, name: &str) -> u64 {
    counts
        .lines()
        .find_map(|l| l.strip_prefix(name)?.strip_prefix(' '))
        .and_then(|n| n.trim().parse().ok())
        .unwrap_or_else(|| panic!("no count of {name}:\n{counts}"))
}

/// The archive members a build's link was handed.
fn members(counts: &str) -> u64 {
    count(counts, "archive members linked")
}

/// What `binary` printed, after it exited cleanly.
fn printed(binary: &Path) -> String {
    let out = Command::new(binary).capped_output().unwrap();
    assert!(out.status.success(), "{}: {:?}", binary.display(), out);
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// mold, which is the only linker that reads every member it is handed.
fn mold_or_missing() -> bool {
    let found = std::env::var_os("PATH")
        .is_some_and(|path| std::env::split_paths(&path).any(|d| d.join("ld.mold").is_file()));
    found || crate::ci::missing("archive_members", "`mold` is not on PATH")
}

/// **A program that only prints is handed a fraction of the archives, and
/// runs.** A count of zero is a link handed the whole archives.
#[test]
#[cfg_attr(not(target_os = "linux"), ignore = "not run here: needs a Linux host")]
fn a_program_that_only_prints_is_handed_a_fraction_of_the_archives() {
    if !mold_or_missing() {
        return;
    }
    for (backend, mode) in crate::e2e::build_modes() {
        let (binary, counts) = built("hello", HELLO, backend, mode, None);
        let members = members(&counts);
        assert_eq!(printed(&binary), "hello\n", "{backend}");
        assert!(
            members > 0 && members <= HELLO_MOST,
            "on {backend}, a program that only prints was handed {members} archive members, \
             and at most {HELLO_MOST} were expected"
        );
    }
}

/// **A program that serves is handed the members serving needs, and runs.**
/// More than a program that only prints, so the count follows the program.
#[test]
#[cfg_attr(not(target_os = "linux"), ignore = "not run here: needs a Linux host")]
fn a_program_that_serves_is_handed_what_serving_needs() {
    if !mold_or_missing() {
        return;
    }
    for (backend, mode) in crate::e2e::build_modes() {
        let hello = members(&built("hello-beside", HELLO, backend, mode, None).1);
        let (binary, counts) = built("serves", SERVES, backend, mode, None);
        let serves = members(&counts);
        assert_eq!(printed(&binary), "bound true\n", "{backend}");
        assert!(
            serves > hello,
            "on {backend}, a program that serves was handed {serves} archive members and one \
             that only prints {hello}"
        );
    }
}

/// **The toolchain's `libgcc.a` is read for the first link and not again.**
/// What it needs is kept under `~/.buri`, so the next link, in a process of
/// its own, is handed the same members without reading the file.
#[test]
#[cfg_attr(not(target_os = "linux"), ignore = "not run here: needs a Linux host")]
fn the_toolchains_libgcc_is_read_once() {
    if !mold_or_missing() {
        return;
    }
    let read = "toolchain archive bytes read";
    for (backend, mode) in crate::e2e::build_modes() {
        let home = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
            .join(format!("archive-members-{}", std::process::id()))
            .join(format!("home-{backend}"));
        let _ = std::fs::remove_dir_all(&home);
        let (_, first) = built("libgcc-first", HELLO, backend, mode, Some(&home));
        let (binary, second) = built("libgcc-second", HELLO, backend, mode, Some(&home));
        assert_eq!(printed(&binary), "hello\n", "{backend}");
        assert!(
            count(&first, read) > 0,
            "on {backend}, the first link read none of libgcc.a:\n{first}"
        );
        assert_eq!(
            count(&second, read),
            0,
            "on {backend}, the second link read libgcc.a"
        );
        assert_eq!(
            members(&second),
            members(&first),
            "on {backend}, the second link was handed other members"
        );
        let _ = std::fs::remove_dir_all(&home);
    }
}
