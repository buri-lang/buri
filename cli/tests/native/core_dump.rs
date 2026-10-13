//! A crash's core dump holds what the program used, not the address space it
//! reserved.
//!
//! The runtime reserves 32 GiB of address space for its heap up front. Linux
//! writes every readable mapping into a core, and a `core_pattern` that pipes to
//! `systemd-coredump` ignores `ulimit -c`, so each crash streamed 32 GiB to disk.
//! A CI runner's 80 GB filled in seconds (run 38117702498). What the kernel will
//! dump is read off `/proc/<pid>/smaps`: every mapping not flagged `dd`.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Allocates, says so, and waits on standard input.
const WAITS: &str = r#"from "core/io" import * as io;
from "native" import { NativeHost };
from "platform/effect" import { Allocator, Stdin, Stdout };

export fn main(host: NativeHost): Result<(), Str> {
    let ctx = context { Allocator: host.alloc, Stdin: host.stdin, Stdout: host.stdout };
    let held = "x".repeat(ctx, 1000000);
    let _ready = io.println(ctx, "ready ${held.length()}").ignore();
    let _waited = io.readLine(ctx);
    .Ok(())
}
"#;

const GIB: u64 = 1 << 30;

fn host_variant() -> String {
    let arch = if cfg!(target_arch = "aarch64") {
        "arm64"
    } else {
        "x86_64"
    };
    format!("linux-{arch}")
}

fn write(root: &Path, rel: &str, text: &str) {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

/// [`WAITS`], built on each native backend: the backend and its binary.
fn built() -> Vec<(&'static str, PathBuf)> {
    let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("core-dump-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    write(&root, "REPO.buri", "");
    write(
        &root,
        "cmd/waits/BUILD.buri",
        &format!(
            "binary {{\n  outputs: [{{ platform: \"native\", variant: \"{}\" }}]\n}}\n",
            host_variant()
        ),
    );
    write(&root, "cmd/waits/main.buri", WAITS);
    let mut out = Vec::new();
    for (backend, mode) in crate::e2e::build_modes() {
        let ran = Command::new(env!("CARGO_BIN_EXE_buri"))
            .current_dir(&root)
            .args(["build", "//cmd/waits"])
            .args(mode)
            .output()
            .unwrap();
        assert!(
            ran.status.success(),
            "{backend}: {}",
            String::from_utf8_lossy(&ran.stderr)
        );
        // A move, not a copy: a child forked mid-copy would hold the file open
        // for writing, and Linux refuses to run it (`ETXTBSY`).
        let kept = root.join(backend);
        std::fs::rename(
            root.join(format!(
                ".buri/out/native/{}/cmd/waits/waits",
                host_variant()
            )),
            &kept,
        )
        .unwrap();
        out.push((backend, kept));
    }
    out
}

/// The bytes of every mapping a core of `pid` would hold, largest first.
fn dumpable(pid: u32) -> Vec<(u64, String)> {
    let smaps = std::fs::read_to_string(format!("/proc/{pid}/smaps")).expect("smaps");
    let mut out = Vec::new();
    let (mut name, mut size) = (String::new(), 0);
    for line in smaps.lines() {
        if let Some(kb) = line.strip_prefix("Size:") {
            size = kb
                .trim()
                .trim_end_matches(" kB")
                .parse::<u64>()
                .expect("a size in kB")
                * 1024;
        } else if let Some(flags) = line.strip_prefix("VmFlags:") {
            if !flags.split_whitespace().any(|f| f == "dd") {
                out.push((size, name.clone()));
            }
        } else if line
            .split_whitespace()
            .next()
            .is_some_and(|f| f.contains('-') && !f.ends_with(':'))
        {
            name = line.to_string();
        }
    }
    out.sort_by_key(|m| std::cmp::Reverse(m.0));
    out
}

#[test]
fn a_core_holds_what_the_program_used_and_not_what_it_reserved() {
    for (backend, binary) in built() {
        let mut child = crate::shared::started(
            Command::new(&binary)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null()),
        );
        let mut ready = String::new();
        BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut ready)
            .unwrap();
        assert_eq!(ready, "ready 1000000\n", "{backend}");
        let mappings = dumpable(child.id());
        child.stdin.take().unwrap().write_all(b"\n").unwrap();
        let status = crate::shared::waited(&mut child, crate::shared::SERVER_DEADLINE);
        assert!(status.success(), "{backend}: {status}");
        let total: u64 = mappings.iter().map(|m| m.0).sum();
        let largest: Vec<String> = mappings
            .iter()
            .take(5)
            .map(|(s, n)| format!("{} MiB {n}", s >> 20))
            .collect();
        assert!(
            total < GIB,
            "on {backend}, a core would hold {} MiB. The largest mappings:\n  {}",
            total >> 20,
            largest.join("\n  ")
        );
    }
}
