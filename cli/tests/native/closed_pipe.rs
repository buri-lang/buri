//! A compiled program whose reader goes away gets an `Err`, and decides what to
//! do with it, on every backend.
//!
//! A print to a closed pipe answers `.Err(.Other(_))` natively, as it does on
//! JavaScript, rather than ending the program by `SIGPIPE`. So does a write to
//! a socket whose peer has gone. A child the program spawns still starts with
//! `SIGPIPE`'s default, as every Unix tool expects.
//!
//! Each row builds one program per native backend this toolchain has, and once
//! for JavaScript, through `buri build`.

use std::io::{BufRead, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// One program, built for every backend: its name and how to start it.
struct Built {
    backend: &'static str,
    program: Vec<String>,
}

impl Built {
    fn command(&self) -> Command {
        let mut cmd = Command::new(&self.program[0]);
        cmd.args(&self.program[1..]);
        cmd
    }
}

fn host_variant() -> String {
    let os = if cfg!(target_os = "macos") { "macos" } else { "linux" };
    let arch = if cfg!(target_arch = "aarch64") { "arm64" } else { "x86_64" };
    format!("{os}-{arch}")
}

fn build(root: &Path, args: &[&str]) {
    let out = Command::new(env!("CARGO_BIN_EXE_buri")).current_dir(root).arg("build").args(args).output().unwrap();
    assert!(
        out.status.success(),
        "`buri build {}` failed:\n{}\n{}",
        args.join(" "),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

/// `source`, with `HOST_MODULE` and `HOST` naming the platform, built on every
/// native backend and on JavaScript.
fn built(name: &str, source: &str) -> Vec<Built> {
    let mut out = built_natively(name, source);
    match crate::shared::js_engine() {
        Some(engine) => {
            let root = scratch(name);
            write(&root, "cmd/js/BUILD.buri", "binary {\n  outputs: [{ platform: \"node\" }]\n}\n");
            write(&root, "cmd/js/main.buri", &source.replace("HOST_MODULE", "node").replace("HOST", "NodeHost"));
            build(&root, &["//cmd/js"]);
            let artifact = root.join(".buri/out/node/cmd/js/js.mjs");
            out.push(Built { backend: "javascript", program: vec![engine, artifact.display().to_string()] });
        }
        None => {
            crate::ci::skipped("closed pipes on JavaScript", "no JavaScript engine (`bun` or `node`) is on PATH");
        }
    }
    out
}

fn scratch(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("closed-pipe-{name}-{}", std::process::id()))
}

fn write(root: &Path, rel: &str, text: &str) {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

/// [`built`] on the native backends alone.
fn built_natively(name: &str, source: &str) -> Vec<Built> {
    let root = scratch(name);
    let _ = std::fs::remove_dir_all(&root);
    write(&root, "REPO.buri", "");
    write(
        &root,
        "cmd/native/BUILD.buri",
        &format!("binary {{\n  outputs: [{{ platform: \"native\", variant: \"{}\" }}]\n}}\n", host_variant()),
    );
    write(&root, "cmd/native/main.buri", &source.replace("HOST_MODULE", "native").replace("HOST", "NativeHost"));

    let mut out = Vec::new();
    for (backend, mode) in crate::e2e::build_modes() {
        let mut args = vec!["//cmd/native"];
        args.extend_from_slice(mode);
        build(&root, &args);
        // Both backends write the same path, so each keeps a copy of its own.
        let kept = root.join(backend);
        std::fs::copy(root.join(format!(".buri/out/native/{}/cmd/native/native", host_variant())), &kept).unwrap();
        crate::shared::admitted(&kept);
        out.push(Built { backend, program: vec![kept.display().to_string()] });
    }
    out
}

/// What a run ended with.
struct Ended {
    code: Option<i32>,
    signal: Option<i32>,
    stdout: String,
    stderr: String,
}

/// Runs `cmd` with stdout on a pipe whose read end goes to `reader`, which
/// drops it when it is done.
fn piped(mut cmd: Command, reader: impl FnOnce(std::io::PipeReader) -> String) -> Ended {
    use std::os::unix::process::ExitStatusExt;
    let (read, write) = std::io::pipe().expect("a pipe");
    let mut child = crate::shared::started(cmd.stdin(Stdio::null()).stdout(write).stderr(Stdio::piped()));
    // The child holds the write end; ours has to go, or the reader never sees
    // the end of the stream.
    drop(cmd);
    let stdout = reader(read);
    let status = crate::shared::waited(&mut child, crate::shared::SERVER_DEADLINE);
    let mut stderr = String::new();
    child.stderr.take().expect("a piped stderr").read_to_string(&mut stderr).expect("stderr");
    Ended { code: status.code(), signal: status.signal(), stdout, stderr }
}

/// A reader that is gone before the program writes anything.
fn gone(read: std::io::PipeReader) -> String {
    drop(read);
    String::new()
}

/// A reader that leaves after one line.
fn one_line(read: std::io::PipeReader) -> String {
    let mut first = String::new();
    std::io::BufReader::new(read).read_line(&mut first).expect("one line reads");
    first
}

/// A reader that stays to the end.
fn everything(mut read: std::io::PipeReader) -> String {
    let mut all = String::new();
    read.read_to_string(&mut all).expect("stdout reads");
    all
}

/// Prints until a print fails, then says which failure on standard error and
/// exits with a code of its own.
const STOPS: &str = r#"from "core/io" import * as io;
from "core/process" import * as process;
from "HOST_MODULE" import { HOST };
from "platform/effect" import { IoError, Process, Stderr, Stdout };

fn kind(e: IoError): Str {
    match (e) {
        .Other(_) => "Other",
        _ => "a classified error",
    }
}

fn lines<C: Process + Stderr + Stdout>(ctx: C, i: Int): () {
    if (i == 0) {
        ()
    } else {
        match (io.println(ctx, "line ${i}")) {
            .Ok(_) => lines(ctx, i - 1),
            .Err(e) => {
                let _said = io.eprintln(ctx, "stdout went away: ${kind(e)}").ignore();
                process.exit(ctx, 7)
            },
        }
    }
}

export fn main(host: HOST): Result<(), Str> {
    let ctx = context { Process: host.proc, Stderr: host.stderr, Stdout: host.stdout };
    let _ran = lines(ctx, 1000000);
    .Ok(())
}
"#;

#[test]
fn a_program_that_handles_a_failed_print_ends_its_own_way() {
    for program in built("stops", STOPS) {
        let backend = program.backend;
        for (reader, how) in [(gone as fn(_) -> _, "before it started"), (one_line, "after one line")] {
            let ended = piped(program.command(), reader);
            assert_eq!(
                (ended.code, ended.signal, ended.stderr.as_str()),
                (Some(7), None, "stdout went away: Other\n"),
                "on {backend}, with the reader gone {how}"
            );
        }
    }
}

/// Prints a bounded number of lines, ignoring every failure, then says on
/// standard error how many failed.
const KEEPS_GOING: &str = r#"from "core/io" import * as io;
from "HOST_MODULE" import { HOST };
from "platform/effect" import { Stderr, Stdout };

fn lines<C: Stdout>(ctx: C, i: Int, failed: Int): Int {
    if (i == 0) {
        failed
    } else {
        match (io.println(ctx, "line ${i}")) {
            .Ok(_) => lines(ctx, i - 1, failed),
            .Err(_) => lines(ctx, i - 1, failed + 1),
        }
    }
}

export fn main(host: HOST): Result<(), Str> {
    let ctx = context { Stderr: host.stderr, Stdout: host.stdout };
    let failed = lines(ctx, 200000, 0);
    let _said = io.eprintln(ctx, "finished, some failed: ${failed > 0}").ignore();
    .Ok(())
}
"#;

#[test]
fn a_program_that_ignores_failed_prints_runs_to_its_end() {
    for program in built("keeps-going", KEEPS_GOING) {
        let backend = program.backend;
        for (reader, how) in [(gone as fn(_) -> _, "before it started"), (one_line, "after one line")] {
            let mut cmd = program.command();
            crate::shared::heap_checked(&mut cmd);
            let ended = piped(cmd, reader);
            assert_eq!(
                (ended.code, ended.signal, ended.stderr.as_str()),
                (Some(0), None, "finished, some failed: true\n"),
                "on {backend}, with the reader gone {how}"
            );
        }
        // And a reader that stays sees every line, with nothing failed.
        let mut cmd = program.command();
        crate::shared::heap_checked(&mut cmd);
        let ended = piped(cmd, everything);
        assert_eq!((ended.code, ended.stderr.as_str()), (Some(0), "finished, some failed: false\n"), "on {backend}");
        assert_eq!(ended.stdout.lines().count(), 200_000, "on {backend}");
    }
}

/// Dials the port it is given and writes until a write fails. Native only:
/// `NodeHost` has no `Tcp`.
const SOCKET_WRITER: &str = r#"from "core/env" import * as env;
from "core/io" import * as io;
from "core/list" import * as list;
from "core/net/tcp" import * as tcp;
from "core/net/tcp" import { Stream };
from "core/process" import * as process;
from "HOST_MODULE" import { HOST };
from "platform/effect" import { Allocator, Environment, Process, Stderr, Tcp };

fn flood<C: Process + Stderr + Tcp>(ctx: C, stream: Stream, chunk: [U8], left: Int): () {
    if (left == 0) {
        io.eprintln(ctx, "every write succeeded").ignore()
    } else {
        match (stream.write(ctx, chunk)) {
            .Ok(_) => flood(ctx, stream, chunk, left - 1),
            .Err(_) => {
                let _said = io.eprintln(ctx, "a write failed").ignore();
                process.exit(ctx, 5)
            },
        }
    }
}

export fn main(host: HOST): Result<(), Str> {
    let ctx = context { Allocator: host.alloc, Environment: host.env, Process: host.proc, Stderr: host.stderr, Tcp: host.tcp };
    let port = env.arguments(ctx).first().andThen(fn(a) => a.toInt()).withDefault(0);
    match (tcp.connect(ctx, "127.0.0.1", port)) {
        .Err(_) => .Err("the dial failed"),
        .Ok(stream) => {
            let _ran = flood(ctx, stream, list.generate(ctx, 65536, fn(_i) => 120), 4096);
            .Ok(())
        },
    }
}
"#;

#[test]
fn a_write_to_a_socket_whose_peer_has_gone_answers_err() {
    for program in built_natively("socket-writer", SOCKET_WRITER) {
        let backend = program.backend;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("a loopback port");
        let port = listener.local_addr().expect("the bound address").port();
        // Accepts once and hangs up at once, without reading.
        let peer = std::thread::spawn(move || drop(listener.accept().expect("the program dials")));
        let mut cmd = program.command();
        cmd.arg(port.to_string());
        let ended = piped(cmd, everything);
        peer.join().expect("the peer thread");
        assert_eq!(
            (ended.code, ended.signal, ended.stderr.as_str()),
            (Some(5), None, "a write failed\n"),
            "on {backend}"
        );
    }
}

/// Runs a pipeline whose writer outlives its reader, and prints how the writer
/// ended.
const SPAWNS: &str = r#"from "core/bytes" import * as bytes;
from "core/io" import * as io;
from "core/process" import * as process;
from "core/process" import { Spawn };
from "HOST_MODULE" import { HOST };
from "platform/effect" import { Allocator, Stdout };

export fn main(host: HOST): Result<(), Str> {
    let ctx = context { Allocator: host.alloc, Spawn: host.spawn, Stdout: host.stdout };
    let pipeline = "(yes; echo \"yes ended $?\" >&2) | head -n 1 >/dev/null";
    let ran = process.run(ctx, process.command("/bin/sh", ["-c", pipeline])).mapErr(fn(_e) => "sh did not run")?;
    let said = bytes.fromUtf8(ctx, ran.stderr).withDefault("?");
    io.println(ctx, said.trim()).mapErr(fn(_e) => "no stdout")
}
"#;

#[test]
fn a_child_still_dies_by_sigpipe_when_its_reader_leaves() {
    for program in built("spawns", SPAWNS) {
        let backend = program.backend;
        let mut cmd = program.command();
        crate::shared::heap_checked(&mut cmd);
        let ended = piped(cmd, everything);
        // 141 is 128 + SIGPIPE, as the shell reports it.
        assert_eq!(
            (ended.code, ended.stdout.as_str(), ended.stderr.as_str()),
            (Some(0), "yes ended 141\n", ""),
            "on {backend}"
        );
    }
}
