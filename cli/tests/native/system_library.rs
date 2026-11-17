//! A macOS link is handed a `libSystem.tbd` of only the symbols its program
//! needs (`design/PERFORMANCE.md` §6.151).
//!
//! lld parsed the SDK's whole `libSystem.tbd`, every symbol of libSystem and
//! the libraries it re-exports, on every link. Each program here is built on
//! every native backend, run, and its `BURI_PROFILE=1` count of the symbols
//! handed over is held to a bound. The count is of what the linker was
//! handed, not a timing.

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

/// The most libSystem symbols a program that only prints may be handed. The
/// SDK's `libSystem.tbd` names about ten thousand. Such a program was handed
/// 197 when this was written, most of them for the runtime archive.
const HELLO_MOST: u64 = 300;

fn write(root: &Path, rel: &str, text: &str) {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

/// `source` as `//cmd/app` in a repository of its own, built on `backend`
/// with `mode`: what the binary printed, and the `BURI_PROFILE=1` count of
/// libSystem symbols the build's link was handed.
fn built(name: &str, source: &str, backend: &str, mode: &[&str]) -> (String, u64) {
    let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("system-library-{}", std::process::id()))
        .join(format!("{name}-{backend}"));
    let _ = std::fs::remove_dir_all(&root);
    write(&root, "REPO.buri", "");
    write(
        &root,
        "cmd/app/BUILD.buri",
        "binary {\n  outputs: [{ platform: \"native\" }]\n}\n",
    );
    write(&root, "cmd/app/main.buri", source);
    let out = Command::new(env!("CARGO_BIN_EXE_buri"))
        .current_dir(&root)
        .args(["build", "//cmd/app"])
        .args(mode)
        .env("BURI_PROFILE", "1")
        .capped_output()
        .unwrap();
    let all = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    if !out.status.success() {
        crate::ci::native_program_failed("system_library", "the build failed", &all);
    }
    let symbols = all
        .lines()
        .find_map(|l| l.strip_prefix("system library symbols linked "))
        .and_then(|n| n.trim().parse().ok())
        .unwrap_or_else(|| panic!("no count of libSystem symbols:\n{all}"));
    let binary = root.join(".buri/out/native/macos-arm64/cmd/app/app");
    let ran = Command::new(&binary).capped_output().unwrap();
    assert!(ran.status.success(), "{}: {ran:?}", binary.display());
    let _ = std::fs::remove_dir_all(&root);
    (String::from_utf8_lossy(&ran.stdout).into_owned(), symbols)
}

/// **A program that only prints is handed a fraction of libSystem, and
/// runs.** A count of zero is a link handed the whole `libSystem.tbd`.
#[test]
#[cfg_attr(
    not(all(target_os = "macos", target_arch = "aarch64")),
    ignore = "not run here: needs an arm64 macOS host"
)]
fn a_program_that_only_prints_is_handed_a_fraction_of_libsystem() {
    for (backend, mode) in crate::e2e::build_modes() {
        let (printed, symbols) = built("hello", HELLO, backend, mode);
        assert_eq!(printed, "hello\n", "{backend}");
        assert!(
            symbols > 0 && symbols <= HELLO_MOST,
            "on {backend}, a program that only prints was handed {symbols} libSystem symbols, \
             and at most {HELLO_MOST} were expected"
        );
    }
}

/// **A program that serves links against the cut too, and runs.** Serving
/// needs sockets, which a program that only prints doesn't. A link the cut
/// broke would have run again with the whole `libSystem.tbd` and counted none.
#[test]
#[cfg_attr(
    not(all(target_os = "macos", target_arch = "aarch64")),
    ignore = "not run here: needs an arm64 macOS host"
)]
fn a_program_that_serves_links_against_the_cut_libsystem() {
    for (backend, mode) in crate::e2e::build_modes() {
        let (printed, symbols) = built("serves", SERVES, backend, mode);
        assert_eq!(printed, "bound true\n", "{backend}");
        assert!(
            symbols > 0,
            "on {backend}, a program that serves was linked against the whole libSystem"
        );
    }
}
