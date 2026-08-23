//! The copy-and-patch backend, through the whole native pipeline, linked, and
//! **run**.
//!
//! `cranelift.rs`'s bar, applied to the third backend: a Buri program goes
//! front end -> `middle::run` -> `middle::native` -> `middle::lower` ->
//! `backend::cpjit` -> object -> `cc` -> executable, and the executable prints
//! what the language says it prints. Nothing short of that is evidence about a
//! backend whose whole output is bytes.
//!
//! This file is deliberately *narrower* than `cranelift.rs`: it holds the
//! programs that exercise the pieces this backend has that no other does — the
//! frame-threaded convention, the hand-written `main`, the `crt` marshalling,
//! the constant pool as its own relocated section — plus the shapes wave 1
//! closed. The whole-language question is `agreement.rs`'s and the conformance
//! corpus's, where cpjit is a column beside the other two rather than a suite
//! of its own.
//!
//! Every test here starts with the same guard: a host with no stencil library
//! (no C compiler, or not arm64) has no backend to ask, and skips rather than
//! fails. That is the "degrades rather than breaks" clause of the dependency
//! bar applied to the suite.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::string_slice,
    clippy::arithmetic_side_effects,
    clippy::print_stdout,
    clippy::print_stderr,
    reason = "test code, as in `tests/harness/mod.rs`: the lint set in \
              `Cargo.toml` pins a promise about the toolchain, and a harness \
              that drives the toolchain is not the toolchain."
)]

use buri::build::buildfile::Platform;
use buri::build::workspace::Workspace;
use buri::compiler::backend::runtime_native::{ARCHIVE, ARCHIVE_NAME, AVAILABLE};
use buri::compiler::backend::{Backend, Options, Profile, Target};
use buri::compiler::backend::cpjit::{Cpjit, AVAILABLE as STENCILS};
use buri::compiler::driver;
use buri::compiler::middle::{self, monomorphize};
use buri::compiler::modules::Role;
use buri::diagnostics::{Diagnostics, SourceMap};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

/// Whether this host can build and run a cpjit artifact at all.
///
/// Three conditions, and each is a real one: no runtime archive means nothing
/// to link against, no stencil library means no code generator, and this
/// backend's stencils are arm64 Mach-O and there is no second set.
fn supported() -> bool {
    AVAILABLE && STENCILS && cfg!(target_os = "macos") && cfg!(target_arch = "aarch64")
}

fn host_platform() -> Platform {
    if cfg!(target_os = "macos") {
        Platform::Macos
    } else {
        Platform::Linux
    }
}

/// A directory this *process* owns.
///
/// The process id is in the name because two overlapping `cargo test` runs
/// otherwise share `native-cranelift/<name>`, and the second overwrites the
/// binary the first is executing — which on macOS is a child that never
/// returns rather than an error, and a full-suite run that never completes.
fn workspace(name: &str) -> PathBuf {
    crate::sweep::once();
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("native-cpjit-{}", std::process::id()))
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// The runtime archive, written once for the process rather than once per
/// test.
///
/// It is six megabytes and it is the same six megabytes every time, so a copy
/// per workspace is a quarter of a gigabyte written and then left behind under
/// `CARGO_TARGET_TMPDIR`. Immutable once written and named by the process id,
/// so `#[test]`s running concurrently share it safely and two `cargo test`
/// runs in two shells still do not — `native/llvm.rs::archive` is the same
/// lock for the same reason.
fn archive() -> &'static Path {
    static WRITTEN: OnceLock<PathBuf> = OnceLock::new();
    WRITTEN.get_or_init(|| {
        let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
            .join(format!("native-cpjit-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(ARCHIVE_NAME);
        std::fs::write(&path, ARCHIVE).unwrap();
        path
    })
}

/// What running one program produced.
struct Ran {
    status: i32,
    stdout: String,
    stderr: String,
}

/// A C shim linked beside the program, whose destructor reports the
/// runtime's allocation counters once `main` has returned.
///
/// `buri_rt_heap_stats` is not reachable from Buri and should not be, so
/// an assertion about *how many times a program allocated* has to be made
/// from outside it. A destructor rather than a wrapper around `main`: the
/// emitted entry point is the one `cli/runtime/lib.rs` §6 describes, and
/// replacing it would be measuring a different program.
const ALLOC_PROBE: &str = r#"
#include <stdio.h>
#include <stdint.h>
typedef struct { uint64_t live_blocks, live_bytes, total_blocks, total_bytes; } Stats;
extern void buri_rt_heap_stats(Stats *out);
__attribute__((destructor)) static void buri_probe(void) {
  Stats s; buri_rt_heap_stats(&s);
  fprintf(stderr, "blocks=%llu live=%llu\n",
          (unsigned long long)s.total_blocks, (unsigned long long)s.live_blocks);
}
"#;

/// `(total_blocks, live_blocks)` from an [`ALLOC_PROBE`]-linked run.
fn probed(stderr: &str) -> (u64, u64) {
    let line = stderr
        .lines()
        .find_map(|l| l.strip_prefix("blocks="))
        .unwrap_or_else(|| panic!("the probe printed nothing: {stderr:?}"));
    let (total, rest) = line.split_once(" live=").unwrap();
    (total.trim().parse().unwrap(), rest.trim().parse().unwrap())
}

/// The whole pipeline, for one snippet, with an optional C probe linked
/// beside it.
fn lowered(source: &str) -> (monomorphize::Program, buri::compiler::semantics::types::Tables) {
    let mut map = SourceMap::new();
    let analysis = driver::analyze_snippet(&mut map, "main", source, Role::Entry);
    assert!(
        !analysis.diags.has_errors(),
        "the snippet did not compile: {:?}",
        analysis.diags.items.iter().map(|d| d.message.clone()).collect::<Vec<_>>()
    );
    let entry = analysis.checked.entry.expect("the snippet exports `main`");
    let paths: Vec<String> = analysis.loaded.modules.iter().map(|m| m.path.clone()).collect();
    let mut diags = Diagnostics::new();
    let mut program = monomorphize::run(
        &analysis.checked,
        paths,
        &mut diags,
        monomorphize::Roots::Main(entry),
    );
    assert!(!diags.has_errors(), "monomorphization failed");
    middle::run(&mut program, &middle::Options::default());
    // The native branch: derives, closure conversion, reference counting.
    // Wave 2c calls this from `build/actions.rs`; here the test does, and
    // the backend is handed exactly what it will be handed there.
    middle::native(&mut program);
    (program, analysis.checked.tables)
}

/// The whole pipeline, for one snippet, with an optional C probe linked
/// beside it.
fn build_with(name: &str, source: &str, probe: Option<&str>) -> PathBuf {
    let (program, tables) = lowered(source);
    let target = Target { platform: host_platform(), arch: None };
    let opts = Options { profile: Profile::Debug, target, unit_prefix: "" };
    let mut backend = Cpjit;
    let units = match backend.emit(&program, &tables, &opts) {
        Ok(units) => units,
        Err(d) => panic!(
            "the backend refused the program: {:?}",
            d.items.iter().map(|i| i.message.clone()).collect::<Vec<_>>()
        ),
    };
    assert!(!units.is_empty(), "no codegen units were emitted");

    let dir = workspace(name);
    let mut objects = Vec::new();
    for unit in &units {
        let path = dir.join(&unit.name);
        std::fs::write(&path, &unit.bytes).unwrap();
        objects.push(path);
    }
    if let Some(text) = probe {
        let c = dir.join("probe.c");
        std::fs::write(&c, text).unwrap();
        let o = dir.join("probe.o");
        let built = Command::new(std::env::var("CC").unwrap_or_else(|_| "cc".to_string()))
            .arg("-c")
            .arg(&c)
            .arg("-o")
            .arg(&o)
            .output()
            .unwrap();
        assert!(
            built.status.success(),
            "the probe did not compile:\n{}",
            String::from_utf8_lossy(&built.stderr)
        );
        objects.push(o);
    }
    let binary = dir.join("program");

    let mut cc = Command::new(std::env::var("CC").unwrap_or_else(|_| "cc".to_string()));
    cc.arg("-o").arg(&binary);
    for o in &objects {
        cc.arg(o);
    }
    cc.arg(archive());
    if cfg!(target_os = "linux") {
        cc.args(["-lpthread", "-ldl", "-lm"]);
    }
    let out = cc.output().unwrap();
    assert!(
        out.status.success(),
        "the link failed:\n{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    binary
}

/// The pipeline up to the objects, for the questions that are about bytes
/// rather than about behaviour.
fn emitted(name: &str, source: &str) -> Vec<(String, Vec<u8>)> {
    let _ = name;
    let (program, tables) = lowered(source);
    let target = Target { platform: host_platform(), arch: None };
    let opts = Options { profile: Profile::Debug, target, unit_prefix: "" };
    let units = Cpjit
        .emit(&program, &tables, &opts)
        .unwrap_or_else(|d| panic!("refused: {:?}", messages(&d)));
    units.into_iter().map(|u| (u.name, u.bytes)).collect()
}

/// The diagnostics a program this backend cannot compile produces.
fn refusal(name: &str, source: &str) -> Vec<String> {
    let _ = name;
    let (program, tables) = lowered(source);
    let target = Target { platform: host_platform(), arch: None };
    let opts = Options { profile: Profile::Debug, target, unit_prefix: "" };
    let mut missing = Cpjit.missing_intrinsics(&program, &tables);
    match Cpjit.emit(&program, &tables, &opts) {
        Ok(_) if missing.is_empty() => panic!("the backend compiled a program it should refuse"),
        Ok(_) => missing,
        Err(d) => {
            missing.append(&mut messages(&d));
            missing
        }
    }
}

fn messages(d: &Diagnostics) -> Vec<String> {
    d.items.iter().map(|i| i.message.clone()).collect()
}

fn run(name: &str, source: &str) -> Ran {
    run_with(name, source, None)
}

fn run_with(name: &str, source: &str, probe: Option<&str>) -> Ran {
    let binary = build_with(name, source, probe);
    let out = Command::new(&binary).output().unwrap();
    Ran {
        status: out.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&out.stdout).to_string(),
        stderr: String::from_utf8_lossy(&out.stderr).to_string(),
    }
}


/// The first program: a hand-written `main` sets up the Buri stack, calls a
/// frame-threaded body, and the runtime prints. Every other claim in this file
/// rests on this one working.
#[test]
fn hello_world_prints_and_exits_zero() {
    if !supported() {
        return;
    }
    let ran = run(
        "hello",
        r#"
from "core/host" import { stdout };
export fn main(): Result<(), Str> {
  let _ = stdout.println("hello, world");
  .Ok(())
}
"#,
    );
    assert_eq!(ran.stdout, "hello, world\n");
    assert_eq!(ran.status, 0, "{}", ran.stderr);
}

/// The exit convention of `cli/runtime/lib.rs` §6, which the hand-written
/// `main` implements rather than the emitter: `.Err(msg)` writes `msg` to
/// standard error and exits 1.
#[test]
fn an_error_return_prints_and_exits_one() {
    if !supported() {
        return;
    }
    let ran = run(
        "err",
        r#"
export fn main(): Result<(), Str> { .Err("nope") }
"#,
    );
    assert_eq!(ran.stderr, "nope\n");
    assert_eq!(ran.status, 1);
}

/// A call, a branch and arithmetic: the frame-threaded convention end to end,
/// with a callee's frame at `fp + frame_size` and no machine stack use at all.
#[test]
fn calls_and_branches_thread_one_frame() {
    if !supported() {
        return;
    }
    let ran = run(
        "frames",
        r#"
from "core/host" import { stdout };
fn fib(n: Int): Int { if (n < 2) { n } else { fib(n - 1) + fib(n - 2) } }
export fn main(): Result<(), Str> {
  let _ = stdout.println("${fib(20)}");
  .Ok(())
}
"#,
    );
    assert_eq!(ran.stdout, "6765\n");
    assert_eq!(ran.status, 0, "{}", ran.stderr);
}

/// A narrow **signed** integer crossing to the runtime.
///
/// A frame slot holds every integer zero-extended (`sources.rs::write`), so an
/// `I8` of `-3` is `0xfd` in its slot and a renderer handed that word prints
/// `253`. `rtcall::show_prim` widens it; this is what says so.
#[test]
fn a_narrow_signed_integer_renders_signed() {
    if !supported() {
        return;
    }
    let ran = run(
        "narrow",
        r#"
from "core/host" import { stdout };
export struct S { a: I8, b: I16, c: I32 }
export fn main(): Result<(), Str> {
  let s = S { a: -3, b: -300, c: -70000 };
  let _ = stdout.println("${s.a} ${s.b} ${s.c}");
  .Ok(())
}
"#,
    );
    assert_eq!(ran.stdout, "-3 -300 -70000\n");
    assert_eq!(ran.status, 0, "{}", ran.stderr);
}

/// A string literal is a pointer into the **constant pool**, which is its own
/// relocated section: `ld` refuses an absolute address inside `__text`, and
/// this is the program that says the pool is not there.
#[test]
fn string_literals_come_out_of_the_constant_pool() {
    if !supported() {
        return;
    }
    let ran = run(
        "literals",
        r#"
from "core/host" import { stdout };
export fn main(): Result<(), Str> {
  let a = "one";
  let b = "two";
  let _ = stdout.println("${a}-${b}");
  .Ok(())
}
"#,
    );
    assert_eq!(ran.stdout, "one-two\n");
    assert_eq!(ran.status, 0, "{}", ran.stderr);
}

/// `str.concat` is generated rather than called: one allocation and two
/// copies, with the ASCII flag the `and` of both operands'.
#[test]
fn concatenation_keeps_the_ascii_flag() {
    if !supported() {
        return;
    }
    let ran = run(
        "concat",
        r#"
from "core/host" import { stdout };
from "core/alloc" import * as alloc;
from "core/cap" import { Alloc };
from "core/str" import * as str;
export fn main(): Result<(), Str> {
  let ctx = context { Alloc: alloc.generalPurpose() };
  let a = "ab";
  let b = "cd";
  let c = "é";
  let ascii = str.format(ctx, "${a}${b}");
  let wide = str.format(ctx, "${a}${c}");
  let _ = stdout.println("${ascii} ${wide} ${ascii.len()} ${wide.len()}");
  .Ok(())
}
"#,
    );
    assert_eq!(ran.stdout, "abcd abé 4 3\n");
    assert_eq!(ran.status, 0, "{}", ran.stderr);
}

/// Two runtime entries whose shapes are the two that are not a plain scalar: a
/// `Ret::Out` writing three words through a trailing out-pointer, and a
/// `Ret::Opt` whose discriminant the emitter turns into whichever tag
/// `middle::layout` chose.
#[test]
fn runtime_entries_answer_through_an_out_pointer() {
    if !supported() {
        return;
    }
    let ran = run(
        "rtshapes",
        r#"
from "core/host" import { stdout };
from "core/alloc" import * as alloc;
from "core/cap" import { Alloc };
from "core/str" import * as str;
export fn main(): Result<(), Str> {
  let ctx = context { Alloc: alloc.generalPurpose() };
  let s = "  Hello  ";
  let t = s.trim();
  let n = "41".toInt();
  let bad = "x".toInt();
  let shown = match (n) { .Some(v) => str.format(ctx, "${v}"), .None => "none" };
  let missing = match (bad) { .Some(v) => str.format(ctx, "${v}"), .None => "none" };
  let _ = stdout.println("[${t}] ${shown} ${missing}");
  .Ok(())
}
"#,
    );
    assert_eq!(ran.stdout, "[Hello] 41 none\n");
    assert_eq!(ran.status, 0, "{}", ran.stderr);
}

/// A refusal is a **diagnostic naming the shape**, never an object that aborts
/// when it reaches it.
///
/// Wave 1 leaves shapes unimplemented on purpose (`backend/cpjit/mod.rs`'s
/// header), and what has to be true of every one of them is that the build
/// stops with a sentence. `deriveArrayHash` is the one used here because it is
/// stable: it is a gap `cranelift/mod.rs` records for itself too.
#[test]
fn a_refused_shape_is_a_diagnostic_and_not_an_object() {
    if !supported() {
        return;
    }
    let messages = refusal(
        "refusal",
        r#"
from "core/host" import { stdout };
from "core/alloc" import * as alloc;
from "core/cap" import { Alloc };
export fn main(): Result<(), Str> {
  let ctx = context { Alloc: alloc.generalPurpose() };
  let xs: [Str] = ["a", "b"];
  let ys = xs.reverse(ctx);
  let _ = stdout.println("${ys.len()}");
  .Ok(())
}
"#,
    );
    assert!(
        messages.iter().any(|m| m.contains("cannot compile")),
        "a refusal has to name the shape: {messages:?}"
    );
}

/// Two emissions of one program are byte-identical.
///
/// `--check-reproducible` compares linked artifacts byte for byte
/// (ARCHITECTURE.md §7) and the object cache is keyed on the unit's IR, so a
/// backend whose output moved between runs would serve a stale object and
/// produce a different artifact from the same source. Nothing in this emitter
/// may iterate a hash map into its output, and this is what says so.
#[test]
fn emission_is_deterministic() {
    if !supported() {
        return;
    }
    let source = r#"
from "core/host" import { stdout };
export fn main(): Result<(), Str> {
  let a = "a";
  let _ = stdout.println("${a}b ${1 + 2}");
  .Ok(())
}
"#;
    let first = emitted("determinism-1", source);
    let second = emitted("determinism-2", source);
    assert_eq!(first.len(), second.len(), "a different number of units");
    for (a, b) in first.iter().zip(&second) {
        assert_eq!(a.0, b.0, "unit names differ");
        assert_eq!(a.1, b.1, "unit `{}` is not byte-identical between two emissions", a.0);
    }

    // And **across processes**, which is the property `--check-reproducible`
    // actually needs and the one an in-process comparison can miss: a
    // `HashMap`'s iteration order is stable within a run and not between two.
    // Re-running this same binary is the cheapest way to get a second process
    // with a different hash seed.
    let mine: Vec<u64> = first.iter().map(|(_, bytes)| fnv(bytes)).collect();
    if std::env::var(DIGEST).is_ok() {
        // The child: print the digests and say nothing else.
        for h in &mine {
            println!("{DIGEST}={h}");
        }
        return;
    }
    let out = Command::new(std::env::current_exe().unwrap())
        .arg("cpjit::emission_is_deterministic")
        .args(["--exact", "--nocapture"])
        .env(DIGEST, "1")
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    let theirs: Vec<u64> = text
        .lines()
        .filter_map(|l| l.strip_prefix(DIGEST).and_then(|r| r.strip_prefix('=')))
        .filter_map(|n| n.parse().ok())
        .collect();
    assert!(!theirs.is_empty(), "the second process printed no digest:\n{text}");
    assert_eq!(mine, theirs, "the objects differ between two processes:\n{text}");
}

/// The environment variable that turns this test into its own child.
const DIGEST: &str = "BURI_CPJIT_DIGEST";

/// FNV-1a over the object's bytes. Any function of all of them would do; this
/// one is four lines and needs nothing from the toolchain.
fn fnv(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h = (h ^ u64::from(*b)).wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// The identity carries the stencil library, because the library is most of
/// what the emitted bytes *are*.
#[test]
fn the_identity_moves_with_the_library() {
    let id = Cpjit.identity();
    assert!(id.starts_with("cpjit "), "{id}");
}

/// `str.compare`, and the derived `Ord` that reaches it.
///
/// Two strings compared is a **call**, and every stencil that calls uses the
/// zero-register prototype — so nothing may be live in the CPS register file
/// across one. `jit::is_barrier` is what says so, and this is the program that
/// finds out when it stops being true.
#[test]
fn string_ordering_is_a_call_and_a_register_barrier() {
    if !supported() {
        return;
    }
    let ran = run(
        "strord",
        r#"
from "core/host" import { stdout };
from "core/order" import { Order };
export struct P { a: Int, b: Str }
derive Eq, Ord for P;
fn name(o: Order): Str { match (o) { .Less => "lt", .Equal => "eq", .Greater => "gt" } }
export fn main(): Result<(), Str> {
  let p = P { a: 1, b: "m" };
  let q = P { a: 1, b: "n" };
  let _ = stdout.println("${name(p.compare(q))} ${name(q.compare(p))} ${name(p.compare(p))} ${p == q}");
  .Ok(())
}
"#,
    );
    assert_eq!(ran.stdout, "lt gt eq false\n", "{}", ran.stderr);
    assert_eq!(ran.status, 0, "{}", ran.stderr);
}

/// Every allocation a program makes is freed before it exits.
///
/// This is the bar `cli/tests/native/runtime.rs` holds the toolchain to, asked
/// of the backend that consumes `middle::rc`'s plan: a missing release is a
/// leak, and a leak that compiles is a wrong program that passes its own
/// tests. `emit::Lower::walk_rc` refuses every shape it cannot release rather
/// than emitting one, so what this asserts is that the shapes it *does* emit
/// balance — a `Str` in a struct, a `Str` in an enum payload, and a `Str`
/// built by concatenation.
#[test]
fn nothing_is_leaked() {
    if !supported() {
        return;
    }
    let ran = run_with(
        "leaks",
        r#"
from "core/host" import { stdout };
from "core/alloc" import * as alloc;
from "core/cap" import { Alloc };
from "core/str" import * as str;

export struct Boxed { label: Str, n: Int }
export enum Held { Empty, Full(Str) }

fn hold(s: Str): Held { .Full(s) }

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: alloc.generalPurpose() };
  let a = str.format(ctx, "one ${1}");
  let b = Boxed { label: str.format(ctx, "two ${2}"), n: 2 };
  let c = hold(str.format(ctx, "three ${3}"));
  let shown = match (c) { .Full(s) => s, .Empty => "none" };
  let _ = stdout.println("${a} ${b.label} ${shown}");
  .Ok(())
}
"#,
        Some(ALLOC_PROBE),
    );
    assert_eq!(ran.status, 0, "{}", ran.stderr);
    let (total, live) = probed(&ran.stderr);
    assert!(total > 0, "the program allocated nothing, so this asserts nothing");
    assert_eq!(live, 0, "{total} blocks allocated and {live} still live at exit");
}

// -----------------------------------------------------------------------
// The conformance corpus, as a census
// -----------------------------------------------------------------------

/// The corpus `language/conformance.rs` and `native/conformance.rs` both run.
fn corpus() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/conformance/lib")
}

/// The corpus as the repository it is, opened once per process. Eleven files
/// import `//lib/<package>`, so a harness that compiled each as a bare snippet
/// would be refused by the *front end* and would say nothing about a backend.
fn repository() -> Option<&'static Workspace> {
    static WS: OnceLock<Option<Workspace>> = OnceLock::new();
    WS.get_or_init(|| {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/conformance");
        let mut map = SourceMap::new();
        let mut diags = Diagnostics::new();
        let ws = Workspace::load(&root, &mut map, &mut diags).ok()?;
        if diags.has_errors() {
            return None;
        }
        Some(ws)
    })
    .as_ref()
}

/// Every `<package>/<file>.buri` in the corpus, in a stable order.
fn corpus_files() -> Vec<String> {
    let mut out = Vec::new();
    let mut packages: Vec<_> =
        std::fs::read_dir(corpus()).unwrap().filter_map(Result::ok).collect();
    packages.sort_by_key(std::fs::DirEntry::file_name);
    for package in packages {
        let tests = package.path().join("test");
        if !tests.is_dir() {
            continue;
        }
        let mut files: Vec<_> = std::fs::read_dir(&tests).unwrap().filter_map(Result::ok).collect();
        files.sort_by_key(std::fs::DirEntry::file_name);
        for file in files {
            out.push(format!(
                "{}/{}",
                package.file_name().to_string_lossy(),
                file.file_name().to_string_lossy()
            ));
        }
    }
    out
}

/// Why this backend refuses one corpus file, or the empty string when it does
/// not. `Err` is a front-end failure, which is the corpus mid-change and not
/// this file's business.
fn corpus_refusal(path: &str) -> Result<String, String> {
    let (package, file) = path.split_once('/').unwrap_or((path, ""));
    let full = corpus().join(package).join("test").join(file);
    let source = std::fs::read_to_string(&full).map_err(|e| e.to_string())?;
    let mut map = SourceMap::new();
    let ws = repository();
    let pkg = ws.and_then(|w| w.pkg_by_path(&format!("lib/{package}")));
    let mut cache = buri::parsing::parser::Cache::new();
    let analysis =
        driver::analyze_snippet_as(ws, pkg, &mut map, &mut cache, "main", &source, Role::TestSource);
    if analysis.diags.has_errors() {
        return Err(String::from("the front end refused it"));
    }
    let paths: Vec<String> = analysis.loaded.modules.iter().map(|m| m.path.clone()).collect();
    let mut diags = Diagnostics::new();
    let mut program =
        monomorphize::run(&analysis.checked, paths, &mut diags, monomorphize::Roots::Tests);
    if diags.has_errors() {
        return Err(String::from("monomorphization failed"));
    }
    middle::run(&mut program, &middle::Options::default());
    middle::native(&mut program);
    let missing = Cpjit.missing_intrinsics(&program, &analysis.checked.tables);
    if !missing.is_empty() {
        return Ok(format!("missing {}", missing.join(", ")));
    }
    let target = Target { platform: host_platform(), arch: None };
    let opts = Options { profile: Profile::Debug, target, unit_prefix: "" };
    match Cpjit.emit(&program, &analysis.checked.tables, &opts) {
        Ok(_) => Ok(String::new()),
        Err(d) => Ok(messages(&d).join("; ")),
    }
}

/// The conformance files this backend compiles today.
///
/// A **ratchet**, in both directions. A file that stops compiling fails here,
/// and so does one that starts: the second is the case that matters, because
/// the whole gate on this backend taking Cranelift's seat is that this list
/// grows to the whole corpus, and a list nobody updates is a list nobody
/// believes. `cargo test -p buri --test native cpjit::the_corpus -- --nocapture`
/// prints the refusal for every file that is not here.
const CORPUS_COMPILES: &[&str] = &[
    "canary/canary.buri",
    "codegen/bitwise.buri",
    "codegen/strings.buri",
    "codegen/tail_calls.buri",
    "memory/allocators.buri",
    "vectors/simd.buri",
];

#[test]
fn the_corpus_census_is_a_ratchet() {
    if !supported() {
        return;
    }
    let mut compiles: Vec<String> = Vec::new();
    let mut refused = 0usize;
    let mut front = 0usize;
    for path in corpus_files() {
        match corpus_refusal(&path) {
            Err(_) => front += 1,
            Ok(why) if why.is_empty() => compiles.push(path),
            Ok(why) => {
                refused += 1;
                println!("cpjit refuses {path}: {why}");
            }
        }
    }
    println!(
        "cpjit compiles {} of {} conformance files ({refused} refused, {front} not asked)",
        compiles.len(),
        corpus_files().len()
    );
    for path in CORPUS_COMPILES {
        assert!(
            compiles.iter().any(|c| c == path),
            "`{path}` used to compile under cpjit and no longer does"
        );
    }
    let unlisted: Vec<&str> =
        compiles.iter().map(String::as_str).filter(|p| !CORPUS_COMPILES.contains(p)).collect();
    assert!(
        unlisted.is_empty(),
        "{unlisted:?} compile under cpjit now — add them to `CORPUS_COMPILES`, which is \
         the list the seat this backend is meant to take is gated on"
    );
}

/// Every conformance file this backend compiles also **runs**, and every
/// `test` block in it passes.
///
/// Compiling is not the bar: a backend that emitted an object for a file and
/// got the answers wrong would pass the census next door. A failed assertion
/// ends the process (SPEC 6.10), so the exit status is the result — the same
/// bar `native/conformance.rs` holds Cranelift to, and the same reason the
/// block count is asserted beside it.
#[test]
fn the_corpus_files_it_compiles_pass() {
    if !supported() {
        return;
    }
    for path in CORPUS_COMPILES {
        let (package, file) = path.split_once('/').unwrap_or((path, ""));
        let full = corpus().join(package).join("test").join(file);
        let source = std::fs::read_to_string(&full).unwrap();
        let blocks = source.matches("\ntest \"").count();
        assert!(blocks > 0, "`{path}` has no test blocks, so running it proves nothing");
        let ran = run_corpus(path, &source);
        assert_eq!(ran.status, 0, "`{path}` failed:\n{}\n{}", ran.stdout, ran.stderr);
    }
}

/// One corpus file, compiled as the test source of its own package, linked and
/// run. The entry point is `asm::test_entry`, which calls every `test` block in
/// order behind `buri_rt_test_enter`.
fn run_corpus(path: &str, source: &str) -> Ran {
    let (package, _) = path.split_once('/').unwrap_or((path, ""));
    let mut map = SourceMap::new();
    let ws = repository();
    let pkg = ws.and_then(|w| w.pkg_by_path(&format!("lib/{package}")));
    let mut cache = buri::parsing::parser::Cache::new();
    let analysis =
        driver::analyze_snippet_as(ws, pkg, &mut map, &mut cache, "main", source, Role::TestSource);
    assert!(!analysis.diags.has_errors(), "`{path}`: the front end refused it");
    let paths: Vec<String> = analysis.loaded.modules.iter().map(|m| m.path.clone()).collect();
    let mut diags = Diagnostics::new();
    let mut program =
        monomorphize::run(&analysis.checked, paths, &mut diags, monomorphize::Roots::Tests);
    assert!(!diags.has_errors(), "`{path}`: monomorphization failed");
    middle::run(&mut program, &middle::Options::default());
    middle::native(&mut program);

    let target = Target { platform: host_platform(), arch: None };
    let opts = Options { profile: Profile::Debug, target, unit_prefix: "" };
    let units = Cpjit
        .emit(&program, &analysis.checked.tables, &opts)
        .unwrap_or_else(|d| panic!("`{path}`: {:?}", messages(&d)));
    let dir = workspace(&path.replace('/', "-"));
    let mut objects = Vec::new();
    for unit in &units {
        let at = dir.join(&unit.name);
        std::fs::write(&at, &unit.bytes).unwrap();
        objects.push(at);
    }
    let binary = dir.join("program");
    let mut cc = Command::new(std::env::var("CC").unwrap_or_else(|_| "cc".to_string()));
    cc.arg("-o").arg(&binary);
    for o in &objects {
        cc.arg(o);
    }
    cc.arg(archive());
    let out = cc.output().unwrap();
    assert!(out.status.success(), "`{path}`: the link failed:\n{}", String::from_utf8_lossy(&out.stderr));
    let out = Command::new(&binary).output().unwrap();
    Ran {
        status: out.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&out.stdout).to_string(),
        stderr: String::from_utf8_lossy(&out.stderr).to_string(),
    }
}

/// The test-binary protocol, which is what makes a native failure report
/// attributable.
///
/// `commands/test.rs::run_native` runs the binary from block zero and, on an
/// abort, runs it again from the block after the one that aborted — so a suite
/// with *n* failures is *n+1* processes and every block gets a verdict
/// (`cli/runtime/testing.rs`'s "the runner's side"). The whole of what a
/// backend owes that is: call `buri_rt_test_enter(i)` before block *i*, in
/// order, and run the block only when it answers non-zero. `asm::test_entry`
/// is that, and this is the program that says so — three blocks, the second
/// failing, run twice.
///
/// `select` still sends `buri test` to Cranelift, so this protocol is not
/// *reached* through the command yet; it is asserted here rather than left to
/// be discovered on the day it is.
#[test]
fn the_test_binary_resumes_where_the_runner_asks() {
    if !supported() {
        return;
    }
    let source = r#"
from "core/testing/assert" import * as assert;
test "first" { assert.eq(1, 1); }
test "second" { assert.eq(1, 2); }
test "third" { assert.eq(3, 3); }
"#;
    let binary = build_tests("resume", source);

    // From the top: the second block aborts, and the third never runs.
    let whole = Command::new(&binary).env("BURI_TEST_FROM", "0").output().unwrap();
    assert_ne!(whole.status.code(), Some(0), "a failing block must end the process");
    let report = String::from_utf8_lossy(&whole.stderr).to_string();
    assert!(report.contains("assert.eq failed"), "the abort is the assertion's: {report}");

    // Started *after* the failure but before the last block: the runner's
    // resume, and the proof that `buri_rt_test_enter` is consulted per block
    // rather than once.
    let after = Command::new(&binary).env("BURI_TEST_FROM", "2").output().unwrap();
    assert_eq!(
        after.status.code(),
        Some(0),
        "resuming past the failure must run the rest cleanly:\n{}",
        String::from_utf8_lossy(&after.stderr)
    );
    // And one *before* it still fails, which is what says the blocks before
    // the resume point were skipped rather than the whole suite.
    let middle = Command::new(&binary).env("BURI_TEST_FROM", "1").output().unwrap();
    assert_ne!(middle.status.code(), Some(0));
}

/// A `.buri` snippet with `test` blocks, compiled as a test binary and linked.
fn build_tests(name: &str, source: &str) -> PathBuf {
    let mut map = SourceMap::new();
    let analysis = driver::analyze_snippet(&mut map, "main", source, Role::TestSource);
    assert!(
        !analysis.diags.has_errors(),
        "the snippet did not compile: {:?}",
        analysis.diags.items.iter().map(|d| d.message.clone()).collect::<Vec<_>>()
    );
    let paths: Vec<String> = analysis.loaded.modules.iter().map(|m| m.path.clone()).collect();
    let mut diags = Diagnostics::new();
    let mut program =
        monomorphize::run(&analysis.checked, paths, &mut diags, monomorphize::Roots::Tests);
    assert!(!diags.has_errors(), "monomorphization failed");
    middle::run(&mut program, &middle::Options::default());
    middle::native(&mut program);

    let target = Target { platform: host_platform(), arch: None };
    let opts = Options { profile: Profile::Debug, target, unit_prefix: "" };
    let units = Cpjit
        .emit(&program, &analysis.checked.tables, &opts)
        .unwrap_or_else(|d| panic!("refused: {:?}", messages(&d)));
    let dir = workspace(name);
    let mut objects = Vec::new();
    for unit in &units {
        let at = dir.join(&unit.name);
        std::fs::write(&at, &unit.bytes).unwrap();
        objects.push(at);
    }
    let binary = dir.join("program");
    let mut cc = Command::new(std::env::var("CC").unwrap_or_else(|_| "cc".to_string()));
    cc.arg("-o").arg(&binary);
    for o in &objects {
        cc.arg(o);
    }
    cc.arg(archive());
    let out = cc.output().unwrap();
    assert!(out.status.success(), "the link failed:\n{}", String::from_utf8_lossy(&out.stderr));
    binary
}
