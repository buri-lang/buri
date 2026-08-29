//! The copy-and-patch backend, through the whole native pipeline, linked, and
//! **run**.
//!
//! `cranelift.rs`'s bar, applied to the third backend: a Buri program goes
//! front end -> `middle::run` -> `middle::native` -> `middle::lower` ->
//! `backend::stencil` -> object -> `cc` -> executable, and the executable prints
//! what the language says it prints. Nothing short of that is evidence about a
//! backend whose whole output is bytes.
//!
//! This file is deliberately *narrower* than `cranelift.rs`: it holds the
//! programs that exercise the pieces this backend has that no other does — the
//! frame-threaded convention, the hand-written `main`, the `crt` marshalling,
//! the constant pool as its own relocated section, the functions a unit
//! generates for itself — plus the two questions no `test` block inside the
//! language can ask: what is still live at exit, and whether two emissions are
//! the same bytes. The whole-language question is `agreement.rs`'s and the
//! conformance corpus's, where stencil is a column beside the other two rather
//! than a suite of its own.
//!
//! Every test here starts with the same guard: a host with no stencil library
//! (no C compiler, or not arm64) has no backend to ask, and skips rather than
//! fails. That is the "degrades rather than breaks" clause of the dependency
//! bar applied to the suite.
use buri::build::buildfile::{Arch, Platform};
use buri::build::link::{self, Row};
use buri::build::workspace::Workspace;
use buri::compiler::backend::{LinkOptions, Linker};
use buri::compiler::backend::runtime_native::{ARCHIVE, ARCHIVE_NAME, AVAILABLE};
use buri::compiler::backend::{Backend, Options, Profile, Target};
use buri::compiler::backend::stencil::{abi as stencil_abi, Stencil, AVAILABLE as STENCILS};
use buri::compiler::driver;
use buri::compiler::middle::{self, monomorphize};
use buri::compiler::modules::Role;
use buri::diagnostics::{Diagnostics, SourceMap};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

/// Whether this host can build and run a stencil artifact at all.
///
/// Two conditions, and each is a real one: no runtime archive means nothing to
/// link against, and `stencil::AVAILABLE` is "this host has a stencil library
/// *and* an entry point to put in front of it" — which is false on x86-64,
/// where `asm.rs` has no SysV shim, and false wherever `cc` could not build the
/// host's library.
///
/// It used to be four, with `cfg!(target_os = "macos")` and
/// `cfg!(target_arch = "aarch64")` spelled out here. They are gone deliberately:
/// with three libraries the host question belongs to `stencil::AVAILABLE`, and a
/// suite that answered it for itself would have to be edited again the first
/// time this backend runs on a Linux runner. Everything below then runs
/// unchanged wherever the backend does.
fn supported() -> bool {
    AVAILABLE && STENCILS
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
        .join(format!("native-stencil-{}", std::process::id()))
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
            .join(format!("native-stencil-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(ARCHIVE_NAME);
        std::fs::write(&path, ARCHIVE).unwrap();
        path
    })
}

/// A C shim linked beside the program, whose destructor reports the
/// runtime's allocation counters once `main` has returned.
///
use crate::shared::{probed, Ran, ALLOC_PROBE};

/// The whole pipeline, for one snippet, with an optional C probe linked
/// beside it.
fn lowered(source: &str) -> (monomorphize::Program, buri::compiler::semantics::types::Tables) {
    let mut map = SourceMap::new();
    let analysis = driver::analyze_snippet(&mut map, "main", source, Role::Entry);
    assert!(
        !analysis.diagnostics.has_errors(),
        "the snippet did not compile: {:?}",
        analysis.diagnostics.items.iter().map(|d| d.message.clone()).collect::<Vec<_>>()
    );
    let entry = analysis.checked.entry.expect("the snippet exports `main`");
    let paths: Vec<String> = analysis.loaded.modules.iter().map(|m| m.path.clone()).collect();
    let mut diagnostics = Diagnostics::new();
    let mut program = monomorphize::run(
        &analysis.checked,
        paths,
        &mut diagnostics,
        monomorphize::Roots::Main(entry),
    );
    assert!(!diagnostics.has_errors(), "monomorphization failed");
    middle::run(&mut program, &middle::Options::default());
    // The native branch: derives, closure conversion, reference counting.
    // A real build calls this from `build/actions.rs`; here the test does, and
    // the backend is handed exactly what it is handed there.
    middle::native(&mut program);
    (program, analysis.checked.tables)
}

/// The whole pipeline, for one snippet, with an optional C probe linked
/// beside it.
fn build_with(name: &str, source: &str, probe: Option<&str>) -> PathBuf {
    let (program, tables) = lowered(source);
    let target = Target { platform: host_platform(), arch: None };
    let opts = Options { profile: Profile::Debug, target, unit_prefix: "" };
    let mut backend = Stencil;
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
    // **The product's link flags, not a bare one.** `build/link.rs` passes
    // `-dead_strip` on every macOS link, and `object.rs` sets
    // `MH_SUBSECTIONS_VIA_SYMBOLS` — which together mean a function nothing
    // *relocates to* is moved and then deleted. Linking these programs without
    // it measured a backend the build system does not produce: every program
    // here passed while `buri test` failed 977 of 997 conformance tests on the
    // same emitter. A harness that links more permissively than the product is
    // a harness that cannot see the product's bugs.
    if cfg!(target_os = "macos") {
        cc.args(["-Wl,-dead_strip", "-Wl,-oso_prefix,."]);
    }
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
    let units = Stencil
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
    let mut missing = Stencil.missing_intrinsics(&program, &tables);
    match Stencil.emit(&program, &tables, &opts) {
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
    crate::shared::ran(&build_with(name, source, probe))
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
from "core/effect" import { Alloc };
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
from "core/effect" import { Alloc };
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
/// Shapes stay unimplemented on purpose (`backend/stencil/mod.rs`'s header), and
/// what has to be true of every one of them is that the build stops with a
/// sentence. An **inexact** conversion is the one used here because it is
/// stable: `x.toI64()` where not every `Float` fits answers
/// `Result<Int, RangeError>` (SPEC 6.2.1), and `RangeError` is a struct of two
/// `Str`s the backend would have to build — a gap `cranelift/mod.rs` records
/// for itself too.
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
from "core/effect" import { Alloc };
export fn main(): Result<(), Str> {
  let ctx = context { Alloc: alloc.generalPurpose() };
  let x: F64 = 2.5;
  let n = x.toI64();
  let _ = stdout.println("${n ?? 0}");
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
        .arg("stencil::emission_is_deterministic")
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
const DIGEST: &str = "BURI_STENCIL_DIGEST";

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
    let id = Stencil.identity();
    assert!(id.starts_with("stencil "), "{id}");
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
from "core/effect" import { Alloc };
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

/// The five shapes `glue.rs` added, each of which is a **pair** that has to
/// balance.
///
/// The conformance corpus is the coverage — twenty-six files and 1,187 blocks
/// — and this is the *leak* half of it, which no `test` block can make an
/// assertion about from inside the language: `buri_rt_heap_stats` is not
/// reachable from Buri and should not be. One program, so that a count of
/// **zero** live blocks covers all five at once:
///
///  * a **closure environment**, which is a heap block carrying its own release
///    function in its first word (`glue.rs::build_env`);
///  * a `[Str]`, whose elements are released by the block's own glue and not by
///    the walk at the drop site (`glue.rs::elems_glue`);
///  * a `[T]` **element retain** across a runtime entry — `push` copies bytes
///    and `cli/runtime/list.rs` calls the glue once per element;
///  * a **derived `show` over a `[Str]`**, where every rendered `Str` arrives
///    owned and the join copies rather than referring;
///  * a **boxed field**, where the field is a block of its own and what is
///    inside it is that block's glue's business.
#[test]
fn the_glue_balances() {
    if !supported() {
        return;
    }
    let ran = run_with(
        "glue",
        r#"
from "core/host" import { stdout };
from "core/alloc" import * as alloc;
from "core/effect" import { Alloc };
from "core/str" import * as str;

export struct Row { names: [Str] }
derive Show for Row;

export enum Tree { Leaf, Node(Tree, Str) }

fn depth(t: Tree): Int {
  match (t) { .Leaf => 0, .Node(inner, _) => 1 + depth(inner) }
}

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: alloc.generalPurpose() };
  let tag = str.format(ctx, "t${1}");
  let names = ["a", "b", "c"].mapCtx(ctx, fn(c, s) => tag.concat(c, s));
  let more = names.push(ctx, str.format(ctx, "d${4}"));
  let row = Row { names: more };
  let t = Tree.Node(Tree.Node(Tree.Leaf, str.format(ctx, "x${1}")), "y");
  let _ = stdout.println("${row.show(ctx)} ${depth(t)} ${row.names.len()}");
  .Ok(())
}
"#,
        Some(ALLOC_PROBE),
    );
    assert_eq!(ran.status, 0, "{}", ran.stderr);
    assert_eq!(
        ran.stdout,
        "Row { names: [\"t1a\", \"t1b\", \"t1c\", \"d4\"] } 2 4\n",
        "{}",
        ran.stderr
    );
    let (total, live) = probed(&ran.stderr);
    assert!(total > 5, "the program allocated {total} blocks, so this asserts little");
    assert_eq!(live, 0, "{total} blocks allocated and {live} still live at exit");
}

/// `Checked`, `Saturating` and `Wrapping` answer the **type's own range**, at
/// every width including 128.
///
/// SPEC 6.2.2 and VALUE-MODEL.md §12 row 2: `Checked` is bounded by the numbers
/// the *platform* has, not by the ones a double can name, so `.Some(v)` here
/// promises `v` is the true result. The 128-bit column is the one with no wider
/// type to do the arithmetic in, which is why it is in the same test as the
/// eight-bit one rather than left to the corpus. `?? 7` stands for `.None`,
/// which none of these operations can otherwise produce.
#[test]
fn the_numeric_surface_answers_at_its_own_width() {
    if !supported() {
        return;
    }
    let ran = run(
        "numeric",
        r#"
from "core/host" import { stdout };
from "core/num" import * as num;
export fn main(): Result<(), Str> {
  let a: U8 = 200;
  let b: I8 = -128;
  let c: I64 = num.maxValue<I64>();
  let d: I128 = num.maxValue<I128>();
  let e: U128 = 340282366920938463463374607431768211455;
  let f: I64 = -5;
  let _ = stdout.println("${a.checkedAdd(100) ?? 7} ${a.saturatingAdd(100)} ${a.wrappingAdd(100)}");
  let _ = stdout.println("${b.checkedSub(1) ?? 7} ${b.saturatingSub(1)} ${b.wrappingSub(1)}");
  let _ = stdout.println("${c.checkedMul(2) ?? 7} ${c.saturatingMul(2)} ${c.wrappingMul(2)}");
  let _ = stdout.println("${d.checkedAdd(1) ?? 7} ${d.saturatingAdd(1)} ${d.wrappingAdd(1)}");
  let _ = stdout.println("${e} ${e.checkedAdd(1) ?? 7} ${f.abs()} ${f.signum()}");
  let _ = stdout.println("${c.checkedDiv(0) ?? 7} ${num.minValue<I8>().checkedDiv(-1) ?? 7}");
  .Ok(())
}
"#,
    );
    assert_eq!(ran.status, 0, "{}", ran.stderr);
    assert_eq!(
        ran.stdout,
        "7 255 44\n\
         7 -128 127\n\
         7 9223372036854775807 -2\n\
         7 170141183460469231731687303715884105727 -170141183460469231731687303715884105728\n\
         340282366920938463463374607431768211455 7 5 -1\n\
         7 7\n",
        "{}",
        ran.stderr
    );
}

/// `sortBy` is **stable**, and `find`, `zip` and `flatten` answer what
/// `runtime.js` answers.
///
/// Stability is the property a merge has and a quicksort does not, and it is
/// observable: two records that compare equal come back in the order they went
/// in. `lists.rs::list_sort` takes the left run whenever the comparator does
/// not say `Greater`, and this is the program that says so.
#[test]
fn the_list_surface_is_the_one_the_language_specifies() {
    if !supported() {
        return;
    }
    let ran = run(
        "lists",
        r#"
from "core/host" import { stdout };
from "core/alloc" import * as alloc;
from "core/effect" import { Alloc };
from "core/str" import * as str;

export struct Row { key: Int, tag: Str }

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: alloc.generalPurpose() };
  let rows = [
    Row { key: 1, tag: "a" }, Row { key: 0, tag: "b" }, Row { key: 1, tag: "c" },
    Row { key: 0, tag: "d" }, Row { key: 1, tag: "e" },
  ];
  let sorted = rows.sortBy(ctx, fn(x, y) => x.key.compare(y.key));
  let tags = sorted.map(ctx, fn(r) => r.tag).join(ctx, "");
  let found = rows.find(fn(r) => r.key == 1).map(fn(r) => r.tag) ?? "?";
  let at = rows.findIndex(fn(r) => r.tag == "d") ?? -1;
  let pairs = [1, 2, 3].zip(ctx, ["x", "y"]);
  let flat = [["p", "q"], [], ["r"]].flatten(ctx).join(ctx, "");
  let sum = [1, 2, 3].foldResult(fn(acc, x) => .Ok(acc + x), 0) ?? -1;
  let stop = [1, 9, 3].foldResult(fn(acc, x) => if (x == 9) { .Err(-2) } else { .Ok(acc + x) }, 0) ?? -1;
  let _ = stdout.println("${tags} ${found} ${at} ${pairs.len()} ${flat} ${sum} ${stop}");
  .Ok(())
}
"#,
    );
    assert_eq!(ran.status, 0, "{}", ran.stderr);
    assert_eq!(ran.stdout, "bdace a 3 2 pqr 6 -1\n", "{}", ran.stderr);
}

/// A `.None` whose payload area was never written is not walked.
///
/// The niche writes **one** word — null at the pointer the discriminant is —
/// and leaves the rest of the payload whatever the frame last held, so a
/// reference-count walk that descended unguarded decremented a count at an
/// address that was never a pointer. It is a crash rather than a wrong answer,
/// and it is exactly the shape `cranelift/emit.rs`'s `Site::Guarded` exists
/// for; this is a `.None` produced in a loop whose frame is still holding the
/// previous iteration's live `Str`.
#[test]
fn a_none_with_a_niche_is_not_walked() {
    if !supported() {
        return;
    }
    let ran = run_with(
        "niche",
        r#"
from "core/host" import { stdout };
from "core/alloc" import * as alloc;
from "core/effect" import { Alloc };
from "core/list" import * as list;
from "core/str" import * as str;

fn pick(xs: [Str], i: Int): Option<Str> {
  if (i == 2) { .None } else { xs.get(i) }
}

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: alloc.generalPurpose() };
  let xs = [str.format(ctx, "a${1}"), str.format(ctx, "b${2}"), str.format(ctx, "c${3}")];
  let seen = list.range(ctx, 0, 4).map(ctx, fn(i) => pick(xs, i) ?? "-").join(ctx, "");
  let _ = stdout.println(seen);
  .Ok(())
}
"#,
        Some(ALLOC_PROBE),
    );
    assert_eq!(ran.status, 0, "{}", ran.stderr);
    assert_eq!(ran.stdout, "a1b2--\n", "{}", ran.stderr);
    let (total, live) = probed(&ran.stderr);
    assert_eq!(live, 0, "{total} blocks allocated and {live} still live at exit");
}

// -----------------------------------------------------------------------
// MEMORY.md §5.3: uniqueness, in-place growth, and what must not move
// -----------------------------------------------------------------------

/// MEMORY.md §5.3, pinned by allocation count rather than by reading the
/// emitted code.
///
/// This backend reaches the three paths through
/// `cli/runtime/text.rs`'s `buri_rt_str_concat` rather than open-coding them
/// (`rtcall.rs`'s `str_concat` says why), so what this asserts is that the
/// call is emitted where the other backends emit instructions — the answer
/// is the same either way, and the *count* is what used to differ.
#[test]
fn a_unique_concat_loop_allocates_logarithmically() {
    if !supported() {
        return;
    }
    let r = run_with(
        "concat-loop",
        r#"
from "core/host" import { stdout, alloc };
from "core/str" import * as str;

export fn build(s: Str, i: Int): Str {
  if (i == 0) { s } else { build(s.concat(alloc, "xy"), i - 1) }
}

export fn main(): Result<(), Str> {
  let s = build("", 1000);
  let _ = stdout.println("${s.len()} ${s.slice(0, 4)}");
  .Ok(())
}
"#,
        Some(ALLOC_PROBE),
    );
    assert_eq!(r.stdout, "2000 xyxy\n", "stderr: {}", r.stderr);
    let (blocks, _) = probed(&r.stderr);
    assert!(
        blocks < 50,
        "a thousand concatenations allocated {blocks} blocks: the fast path did not fire"
    );
}

/// The observable-semantics guard: a string a second binding still holds
/// has a count above one, so concatenating onto it must copy.
///
/// A fast path that fired on a shared string would print the wrong answer,
/// not merely allocate less — which is why this is an output assertion.
#[test]
fn a_shared_concat_does_not_mutate_what_is_shared() {
    if !supported() {
        return;
    }
    let r = run(
        "concat-shared",
        r#"
from "core/host" import { stdout, alloc };
from "core/str" import * as str;

export fn main(): Result<(), Str> {
  let base = "ab".concat(alloc, "cd");
  let a = base.concat(alloc, "-one");
  let b = base.concat(alloc, "-two");
  let _ = stdout.println(base);
  let _ = stdout.println(a);
  let _ = stdout.println(b);
  let _ = stdout.println(base);
  .Ok(())
}
"#,
    );
    assert_eq!(r.stdout, "abcd\nabcd-one\nabcd-two\nabcd\n", "stderr: {}", r.stderr);
    assert_eq!(r.status, 0);
}

/// A `Str` view whose block someone else still holds is not a place to
/// write either: two slices of one allocation, and appending to the first
/// must leave the second and the whole alone.
#[test]
fn appending_to_a_view_does_not_disturb_its_siblings() {
    if !supported() {
        return;
    }
    let r = run(
        "concat-view",
        r#"
from "core/host" import { stdout, alloc };
from "core/str" import * as str;

export fn main(): Result<(), Str> {
  let whole = "left".concat(alloc, ",right");
  let head = whole.slice(0, 4);
  let tail = whole.slice(5, 10);
  let grown = head.concat(alloc, "!!");
  let _ = stdout.println(head);
  let _ = stdout.println(tail);
  let _ = stdout.println(grown);
  let _ = stdout.println(whole);
  .Ok(())
}
"#,
    );
    assert_eq!(r.stdout, "left\nright\nleft!!\nleft,right\n", "stderr: {}", r.stderr);
    assert_eq!(r.status, 0);
}

/// A borrowed local handed to a construct **beside** a sibling that holds
/// its last mention — `middle::rc`'s `children`, and a middle-end fact both
/// backends show.
///
/// It is here as well as in `cranelift.rs` because the failure it guards
/// against is a *wrong answer* rather than a leak: the concatenation chain
/// holds three uncounted words out of `base` while the last hole computes
/// `base.len()`, and an in-place append into a freed block prints rubbish.
#[test]
fn a_borrowed_local_survives_a_sibling_that_holds_its_last_mention() {
    if !supported() {
        return;
    }
    let r = run(
        "borrow-across-siblings",
        r#"
from "core/host" import { stdout, alloc };
from "core/str" import * as str;

export fn main(): Result<(), Str> {
  let base = "ab".concat(alloc, "cd");
  let a = base.concat(alloc, "-one");
  let b = base.concat(alloc, "-two");
  let _ = stdout.println("${base} ${a} ${b} ${base.len()}");
  let _ = stdout.println("${b} ${b.len()}");
  .Ok(())
}
"#,
    );
    assert_eq!(r.stdout, "abcd abcd-one abcd-two 4\nabcd-two 8\n", "stderr: {}", r.stderr);
    assert_eq!(r.status, 0);
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
    crate::shared::conformance_repository()
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
    let repository = repository();
    let package = repository.and_then(|w| w.package_by_path(&format!("lib/{package}")));
    let mut cache = buri::parsing::parser::Cache::new();
    let analysis = driver::analyze_snippet_as(
        repository,
        package,
        &mut map,
        &mut cache,
        "main",
        &source,
        Role::TestSource,
    );
    if analysis.diagnostics.has_errors() {
        return Err(String::from("the front end refused it"));
    }
    let paths: Vec<String> = analysis.loaded.modules.iter().map(|m| m.path.clone()).collect();
    let mut diagnostics = Diagnostics::new();
    let mut program =
        monomorphize::run(&analysis.checked, paths, &mut diagnostics, monomorphize::Roots::Tests);
    if diagnostics.has_errors() {
        return Err(String::from("monomorphization failed"));
    }
    middle::run(&mut program, &middle::Options::default());
    middle::native(&mut program);
    let missing = Stencil.missing_intrinsics(&program, &analysis.checked.tables);
    if !missing.is_empty() {
        return Ok(format!("missing {}", missing.join(", ")));
    }
    let target = Target { platform: host_platform(), arch: None };
    let opts = Options { profile: Profile::Debug, target, unit_prefix: "" };
    match Stencil.emit(&program, &analysis.checked.tables, &opts) {
        Ok(_) => Ok(String::new()),
        Err(d) => Ok(messages(&d).join("; ")),
    }
}

/// The conformance files this backend compiles.
///
/// A **ratchet**, in both directions. A file that stops compiling fails here,
/// and so does one that starts: the second is the case that matters, because a
/// list nobody updates is a list nobody believes.
/// `cargo test -p buri --test native stencil::the_corpus -- --nocapture` prints
/// the refusal for every file that is not here.
///
/// It is **`native/conformance.rs`'s `PACKAGES`**, entry for entry: the
/// twenty-five Cranelift runs in its suite plus `proto/binary.buri`, which
/// Cranelift compiles and passes and excludes only for what the *middle end*
/// costs on it. The six that are not here are the six Cranelift refuses, for
/// the three reasons that file records.
const CORPUS_COMPILES: &[&str] = &[
    "calendar/date.buri",
    "canary/canary.buri",
    "codegen/bitwise.buri",
    "codegen/equality.buri",
    "codegen/strings.buri",
    "codegen/tail_calls.buri",
    "collections/bitset.buri",
    "collections/map.buri",
    "collections/queue.buri",
    "crypto/sha256.buri",
    "data/lists.buri",
    "data/optionresult.buri",
    "data/patterns.buri",
    "data/strings.buri",
    "memory/allocators.buri",
    "numbers/bits.buri",
    "numbers/integers.buri",
    "proto/binary.buri",
    "proto/failures.buri",
    "semantics/effects.buri",
    "semantics/evaluation.buri",
    "semantics/generics.buri",
    "semantics/traits.buri",
    "semantics/variance.buri",
    "text/bytes.buri",
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
                println!("stencil refuses {path}: {why}");
            }
        }
    }
    println!(
        "stencil compiles {} of {} conformance files ({refused} refused, {front} not asked)",
        compiles.len(),
        corpus_files().len()
    );
    for path in CORPUS_COMPILES {
        assert!(
            compiles.iter().any(|c| c == path),
            "`{path}` used to compile under stencil and no longer does"
        );
    }
    let unlisted: Vec<&str> =
        compiles.iter().map(String::as_str).filter(|p| !CORPUS_COMPILES.contains(p)).collect();
    assert!(
        unlisted.is_empty(),
        "{unlisted:?} compile under stencil now — add them to `CORPUS_COMPILES`, which is \
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
    let mut failures: Vec<String> = Vec::new();
    for path in CORPUS_COMPILES {
        let (package, file) = path.split_once('/').unwrap_or((path, ""));
        let full = corpus().join(package).join("test").join(file);
        let source = std::fs::read_to_string(&full).unwrap();
        let blocks = source.matches("\ntest \"").count();
        assert!(blocks > 0, "`{path}` has no test blocks, so running it proves nothing");
        let ran = run_corpus(path, &source);
        if ran.status != 0 {
            failures.push(format!(
                "`{path}` exited {}:\nstdout:\n{}\nstderr:\n{}",
                ran.status, ran.stdout, ran.stderr
            ));
        }
    }
    // Every failing file, not the first: two platforms failing on two
    // different files is one report here and two runs otherwise.
    assert!(failures.is_empty(), "{} corpus files failed:\n{}", failures.len(), failures.join("\n"));
}

/// One corpus file, compiled as the test source of its own package, linked and
/// run. The entry point is `asm::test_entry`, which calls every `test` block in
/// order behind `buri_rt_test_enter`.
fn run_corpus(path: &str, source: &str) -> Ran {
    let (package, _) = path.split_once('/').unwrap_or((path, ""));
    let mut map = SourceMap::new();
    let repository = repository();
    let package = repository.and_then(|w| w.package_by_path(&format!("lib/{package}")));
    let mut cache = buri::parsing::parser::Cache::new();
    let analysis = driver::analyze_snippet_as(
        repository,
        package,
        &mut map,
        &mut cache,
        "main",
        source,
        Role::TestSource,
    );
    assert!(!analysis.diagnostics.has_errors(), "`{path}`: the front end refused it");
    let paths: Vec<String> = analysis.loaded.modules.iter().map(|m| m.path.clone()).collect();
    let mut diagnostics = Diagnostics::new();
    let mut program =
        monomorphize::run(&analysis.checked, paths, &mut diagnostics, monomorphize::Roots::Tests);
    assert!(!diagnostics.has_errors(), "`{path}`: monomorphization failed");
    middle::run(&mut program, &middle::Options::default());
    middle::native(&mut program);

    let target = Target { platform: host_platform(), arch: None };
    let opts = Options { profile: Profile::Debug, target, unit_prefix: "" };
    let units = Stencil
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

// ---------------------------------------------------------------------------
// The product's own link
// ---------------------------------------------------------------------------

/// A stencil program linked by **`build/link.rs`**, not by a hand-written `cc`
/// line, and run.
///
/// This exists because the difference between the two was once a silent
/// miscompile. `build/link.rs` passes `-Wl,-dead_strip` on every macOS link and
/// `object.rs` sets `MH_SUBSECTIONS_VIA_SYMBOLS`, which tells `ld64` that every
/// symbol begins an independently movable atom; a call emitted as a **baked
/// displacement** rather than as a relocation is therefore not a reference, so
/// the callee's atom was moved and then deleted and the branch landed on
/// whatever followed. Every program in this file passed under a bare
/// `cc -o out *.o libburi_rt.a` while `buri test` failed 977 of 997 native
/// conformance tests on the same emitter.
///
/// The program is chosen to be the minimal shape that reproduced it: a
/// self-recursive function in the unit that owns `main`, reached only through
/// calls the emitter resolves itself. `link::run` is the same entry
/// `build/actions.rs` calls, so the flags, the archive, the manifest and the
/// staging are the product's and cannot drift from it.
#[test]
fn the_products_own_link_produces_a_program_that_runs() {
    if !supported() {
        return;
    }
    let Some(platform) = link::host_platform() else {
        eprintln!("no linkable host platform");
        return;
    };
    let target = Target { platform, arch: link::host_arch() };
    let source = r#"
from "core/host" import { stdout };
fn sumTo(n: Int, acc: Int): Int { if (n <= 0) { acc } else { sumTo(n - 1, acc + n) } }
export fn main(): Result<(), Str> {
  let _ = stdout.println("${sumTo(1000, 0)}");
  .Ok(())
}
"#;
    let (program, tables) = lowered(source);
    let opts = Options { profile: Profile::Debug, target, unit_prefix: "cmd/app" };
    let units = Stencil
        .emit(&program, &tables, &opts)
        .unwrap_or_else(|d| panic!("the backend refused the program: {:?}", messages(&d)));
    assert!(!units.is_empty(), "no codegen units were emitted");

    let dir = workspace("product-link");
    let Ok(linker) = link::select(target) else {
        eprintln!("no linker driver on this host");
        return;
    };
    let linker = linker.in_dir(dir.join("link"));
    let rows: Vec<Row> = units
        .iter()
        .map(|u| Row {
            unit: u.name.trim_end_matches(".o").to_string(),
            key: u.key.as_str().to_string(),
            cached: false,
        })
        .collect();
    let out = dir.join("app");
    let options = LinkOptions { profile: Profile::Debug, target, unit_prefix: "cmd/app" };
    if let Err(d) = link::run(&units, &rows, &linker, &out, &options) {
        panic!("the product's link failed: {:?}", messages(&d));
    }

    let ran = Command::new(&out).output().unwrap();
    assert_eq!(
        ran.status.code(),
        Some(0),
        "the artifact `buri build` would produce exited {:?}:\n{}",
        ran.status.code(),
        String::from_utf8_lossy(&ran.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&ran.stdout), "500500\n");
    // A unit reaches the link under the key the action cache stored it by; an
    // empty one would mean the link was fed something that was never cached.
    assert!(
        units.iter().all(|u| !u.key.as_str().is_empty()),
        "a unit reached the link with no key"
    );
    println!("linked with {} ({})", linker.name(), linker.version());
}

/// One non-tail recursion, at a depth the caller chooses.
///
/// Non-tail on purpose: SPEC §8.3 requires a *tail* call to run in constant
/// stack space, so a tail-recursive probe would say nothing about a stack at
/// all. The `1 +` after the call is what makes each level a frame.
fn recursion(depth: u64) -> String {
    format!(
        r#"
fn f(i: Int): Int {{ if (i <= 0) {{ 0 }} else {{ 1 + f(i - 1) }} }}
export fn main(): Result<(), Str> {{
  if (f({depth}) == {depth}) {{ .Ok(()) }} else {{ .Err("wrong") }}
}}
"#
    )
}

/// **A runaway recursion faults rather than writing past the stack** — design
/// §8.
///
/// The Buri stack is a `__bss` block and nothing about `__bss` stops a program
/// walking off the end of it: before `asm::install_guard` the same program ran
/// thirteen kilobytes past the block before it happened to reach an unmapped
/// page, and *what it wrote into on the way is whatever the linker placed
/// there*. The guard makes the first byte past the usable stack unmapped, so
/// the fault is at the boundary and there is no "on the way".
///
/// What is asserted is that the process is **killed by a signal**, which is the
/// same thing a Cranelift-compiled program does when it exhausts the machine
/// stack (measured: `SIGSEGV` there, `SIGBUS` here, neither with a message).
/// It is not asserted that a particular signal arrives: which one the kernel
/// raises for a `PROT_NONE` page inside a mapping is the kernel's business.
#[test]
fn a_runaway_recursion_faults_at_the_guard() {
    if !supported() {
        return;
    }
    use std::os::unix::process::ExitStatusExt;
    let binary = build_with("guard-runaway", &recursion(5_000_000), None);
    let out = Command::new(&binary).output().unwrap();
    assert!(
        out.status.signal().is_some(),
        "a five-million-deep recursion exited {:?} instead of faulting: {}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    );
}

/// The guard is **above** the stack a program may use, not carved out of it.
///
/// The counterpart to the test above, and the one that would catch the guard
/// being put at the wrong end or being made out of the program's own room: a
/// recursion that fits has to be unaffected. Twenty thousand levels is about a
/// quarter of what the block holds for this function, which leaves margin for a
/// frame that grows without leaving the test asserting nothing.
#[test]
fn a_deep_recursion_inside_the_stack_still_answers() {
    if !supported() {
        return;
    }
    let ran = run("guard-fits", &recursion(20_000));
    assert_eq!(ran.status, 0, "{}", ran.stderr);
}

/// A `.buri` snippet with `test` blocks, compiled as a test binary and linked.
fn build_tests(name: &str, source: &str) -> PathBuf {
    let mut map = SourceMap::new();
    let analysis = driver::analyze_snippet(&mut map, "main", source, Role::TestSource);
    assert!(
        !analysis.diagnostics.has_errors(),
        "the snippet did not compile: {:?}",
        analysis.diagnostics.items.iter().map(|d| d.message.clone()).collect::<Vec<_>>()
    );
    let paths: Vec<String> = analysis.loaded.modules.iter().map(|m| m.path.clone()).collect();
    let mut diagnostics = Diagnostics::new();
    let mut program =
        monomorphize::run(&analysis.checked, paths, &mut diagnostics, monomorphize::Roots::Tests);
    assert!(!diagnostics.has_errors(), "monomorphization failed");
    middle::run(&mut program, &middle::Options::default());
    middle::native(&mut program);

    let target = Target { platform: host_platform(), arch: None };
    let opts = Options { profile: Profile::Debug, target, unit_prefix: "" };
    let units = Stencil
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
    // `build/link.rs`'s macOS flags, for `build_with`'s reason.
    if cfg!(target_os = "macos") {
        cc.args(["-Wl,-dead_strip", "-Wl,-oso_prefix,."]);
    }
    let out = cc.output().unwrap();
    assert!(out.status.success(), "the link failed:\n{}", String::from_utf8_lossy(&out.stderr));
    binary
}

// ---------------------------------------------------------------------------
// The cross targets
// ---------------------------------------------------------------------------
//
// **Nothing below runs a Buri program, and nothing below can.** This host is
// macOS/arm64; `link::can_link` refuses a Linux target on it and
// `runtime_native::ARCHIVE` is the host's alone, so the product's own path
// stops at object bytes for `linux-arm64` exactly as `cli/benches/compiler.rs`'s
// `lower+linux-*` rows stop there for Cranelift.
//
// What is available is everything up to execution, and it is worth having:
// these tests emit real objects for a cross target, hand them to the real
// linker the product would use, and read the result back. The four questions
// they answer are the four a container port can get wrong —
//
//  1. does a system linker *accept* what `elf.rs` writes;
//  2. does every relocation **resolve**, with nothing left over in the image;
//  3. is the resolved code still disassemblable arm64, or did a relocation
//     land in the middle of an instruction;
//  4. are two emissions the same bytes.
//
// — and none of them is answered by the macOS suite above, because none of them
// is about the emitter. What is left unanswered is whether the *program* is
// right, and no test on this machine can say so; `design/native/CODEGEN-STENCIL.md`
// under "the Linux checklist" is the list a Linux CI run must confirm.

/// Whether a cross emission can be attempted at all.
///
/// The stencils have to exist — a toolchain built where `cc` could not
/// cross-compile has empty ones — and the tools this file drives have to be on
/// `PATH`. Missing tools skip rather than fail, which is the same bar the rest
/// of the file applies to a missing C compiler.
fn cross_tools() -> Option<()> {
    for t in ["ld.lld", "llvm-nm", "llvm-objdump", "llvm-readelf"] {
        Command::new(t).arg("--version").output().ok().filter(|o| o.status.success())?;
    }
    Some(())
}

/// One cross target's unit objects for a snippet.
fn cross_units(source: &str, arch: Arch) -> Option<Vec<buri::compiler::backend::Emitted>> {
    let stencil_target = match arch {
        Arch::Arm64 => stencil_abi::StencilTarget::LinuxArm64,
        Arch::X86_64 => stencil_abi::StencilTarget::LinuxX86_64,
    };
    if !buri::compiler::backend::stencil::available_for(stencil_target) {
        eprintln!("this toolchain has no {} stencils", stencil_target.slug());
        return None;
    }
    let (program, tables) = lowered(source);
    let target = Target { platform: Platform::Linux, arch: Some(arch) };
    let opts = Options { profile: Profile::Debug, target, unit_prefix: "cmd/app" };
    match Stencil.emit(&program, &tables, &opts) {
        Ok(units) => Some(units),
        Err(d) => {
            // A refusal is an answer too, and for x86-64 it is the *expected*
            // one until `asm.rs` has a SysV entry point. The caller decides.
            eprintln!("{} refused: {:?}", stencil_target.slug(), messages(&d));
            None
        }
    }
}

/// A program with enough shapes in it to reach every relocation kind
/// `elf.rs` can write: a call between units' functions (`Branch26`), a string
/// literal in the pool (`Abs64` and the `Page21`/`PageOff12` pair), the Buri
/// stack the shim names (a `Page21`/`PageOff12` against a `__bss` symbol), and
/// a runtime call.
const CROSS_PROGRAM: &str = r#"
from "core/host" import { stdout };
fn sumTo(n: Int, acc: Int): Int { if (n <= 0) { acc } else { sumTo(n - 1, acc + n) } }
export fn main(): Result<(), Str> {
  let _ = stdout.println("sum=${sumTo(1000, 0)}");
  .Ok(())
}
"#;

/// **`linux-arm64` objects link, and every relocation in them resolves.**
///
/// The link is a real `ld.lld` static link, not a `-r` merge: a merge would
/// accept a relocation that no symbol satisfies, and the whole question here is
/// whether the emitter named things the linker can find. The runtime is a
/// generated stub rather than `libburi_rt.a`, because there is no Linux
/// `libburi_rt.a` on this host — `cli/build.rs` builds the archive for the host
/// triple and one triple only. That is the honest boundary of this test: it
/// proves the *shape* of every reference and nothing about what the referent
/// does.
#[test]
fn linux_arm64_objects_link_and_every_relocation_resolves() {
    if cross_tools().is_none() {
        eprintln!("no cross tool-chain on PATH");
        return;
    }
    let Some(units) = cross_units(CROSS_PROGRAM, Arch::Arm64) else { return };
    assert!(!units.is_empty(), "no codegen units were emitted");
    let dir = workspace("cross-linux-arm64");
    let objects = write_objects(&dir, &units);

    // Every object is an ELF for the right machine, with the sections a unit
    // has. `llvm-readelf` rather than this crate's own reader, deliberately:
    // a reader written beside a writer agrees with it by construction.
    for o in &objects {
        let h = tool("llvm-readelf", &["-h", &o.display().to_string()]);
        assert!(h.contains("AArch64"), "{}: not an aarch64 ELF:\n{h}", o.display());
        assert!(h.contains("REL (Relocatable file)"), "{}: not relocatable", o.display());
    }

    let exe = dir.join("app");
    link_with_stub(&dir, &objects, &exe, "aarch64-unknown-linux-gnu", "aarch64linux");

    // (2) Nothing is left over. A fully static link resolves every relocation;
    // one that survived would mean the linker had deferred a reference it could
    // not compute, which for a program that loads nothing is a reference to
    // something that will never exist.
    let rel = tool("llvm-readelf", &["-r", &exe.display().to_string()]);
    assert!(
        rel.trim().is_empty() || rel.contains("There are no relocations"),
        "the linked image still has relocations:\n{rel}"
    );

    // (3) The resolved code is still arm64. `llvm-objdump` prints `<unknown>`
    // for a word it cannot decode, and a relocation applied to the wrong offset
    // — the failure a container port makes — turns the instruction it landed in
    // into exactly that.
    let dis = tool("llvm-objdump", &["-d", &exe.display().to_string()]);
    assert!(!dis.contains("<unknown>"), "the linked image does not disassemble cleanly");
    assert!(dis.contains("<main>"), "the linked image has no main:\n{}", &dis[..dis.len().min(400)]);
}

/// **Two emissions of one program are the same bytes, for every target.**
///
/// `--check-reproducible` compares linked artifacts byte for byte
/// (ARCHITECTURE.md §7), and it can only do that if the objects underneath are
/// stable. `elf.rs` builds two string tables and sorts a symbol table, which
/// are exactly the places a `HashMap` iteration would leak in.
#[test]
fn a_cross_emission_is_reproducible() {
    for arch in [Arch::Arm64, Arch::X86_64] {
        let (Some(a), Some(b)) = (cross_units(CROSS_PROGRAM, arch), cross_units(CROSS_PROGRAM, arch))
        else {
            continue;
        };
        assert_eq!(a.len(), b.len());
        for (x, y) in a.iter().zip(&b) {
            assert_eq!(x.name, y.name);
            assert_eq!(x.bytes, y.bytes, "{} is not reproducible for {arch:?}", x.name);
        }
    }
}

/// **A target this toolchain cannot emit for says which one and why.**
///
/// The two ways it can happen are different and a user can act on only one of
/// them: a missing library is a toolchain that was built without a
/// cross-compiler, and a missing entry point is work this backend has not done.
#[test]
fn an_unsupported_cross_target_is_refused_with_a_reason() {
    let (program, tables) = lowered("export fn main(): Result<(), Str> { .Ok(()) }");
    let opts = |arch| Options {
        profile: Profile::Debug,
        target: Target { platform: Platform::Linux, arch: Some(arch) },
        unit_prefix: "",
    };
    // x86-64 has stencils and no `main`; the sentence has to say so rather than
    // producing an object with arm64 bytes in its entry point.
    if buri::compiler::backend::stencil::available_for(stencil_abi::StencilTarget::LinuxX86_64) {
        let d = Stencil.emit(&program, &tables, &opts(Arch::X86_64)).err();
        let msgs = d.map(|d| messages(&d)).unwrap_or_default();
        assert!(
            msgs.iter().any(|m| m.contains("entry point") && m.contains("x86-64")),
            "x86-64 was not refused with the reason: {msgs:?}"
        );
    }
    // And macOS x86-64 is a combination no library is built for at all.
    let mac = Options {
        profile: Profile::Debug,
        target: Target { platform: Platform::Macos, arch: Some(Arch::X86_64) },
        unit_prefix: "",
    };
    let msgs = Stencil.emit(&program, &tables, &mac).err().map(|d| messages(&d)).unwrap_or_default();
    assert!(
        msgs.iter().any(|m| m.contains("macos-x86_64")),
        "macos-x86_64 was not refused by name: {msgs:?}"
    );
}

/// Writes a unit list into a directory and answers the paths, in order.
fn write_objects(dir: &Path, units: &[buri::compiler::backend::Emitted]) -> Vec<PathBuf> {
    units
        .iter()
        .map(|u| {
            let p = dir.join(&u.name);
            std::fs::write(&p, &u.bytes).unwrap();
            p
        })
        .collect()
}

/// Runs a tool that must succeed, and answers its standard output.
fn tool(name: &str, args: &[&str]) -> String {
    let out = Command::new(name).args(args).output().unwrap();
    assert!(
        out.status.success(),
        "{name} {args:?} failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// Links a set of cross objects with a **generated stub** standing in for
/// `libburi_rt.a`.
///
/// The stub is derived from the objects themselves: every symbol they leave
/// undefined gets an empty definition. That is what makes this a test of the
/// emitter's *references* rather than of a hand-maintained list — a new runtime
/// entry added to `runtime.rs` cannot make this test stale, and a reference the
/// emitter invents cannot be silently satisfied by something that was already
/// in the list.
fn link_with_stub(dir: &Path, objects: &[PathBuf], out: &Path, triple: &str, emulation: &str) {
    let mut undefined: Vec<String> = Vec::new();
    for o in objects {
        for line in tool("llvm-nm", &["--undefined-only", &o.display().to_string()]).lines() {
            if let Some(name) = line.split_whitespace().last() {
                if !undefined.contains(&name.to_string()) {
                    undefined.push(name.to_string());
                }
            }
        }
    }
    undefined.sort();
    assert!(!undefined.is_empty(), "a program with no external references is not this one");
    let mut c = String::from("// generated: the shape of every symbol these objects need\n");
    for name in &undefined {
        // Everything a stencil unit imports is a function: the runtime archive's
        // entries and `memcpy`. A data symbol would need a size this stub
        // cannot know, and its appearance here should be noticed.
        c.push_str(&format!("void {name}(void) {{}}\n"));
    }
    let stub_c = dir.join("stub.c");
    let stub_o = dir.join("stub.o");
    std::fs::write(&stub_c, &c).unwrap();
    let cc = std::env::var("CC").unwrap_or_else(|_| "cc".to_string());
    let res = Command::new(&cc)
        .arg(format!("--target={triple}"))
        .args(["-c", "-O0", "-ffreestanding", "-fno-asynchronous-unwind-tables"])
        .arg(&stub_c)
        .arg("-o")
        .arg(&stub_o)
        .output()
        .unwrap();
    assert!(
        res.status.success(),
        "the stub did not cross-compile:\n{}",
        String::from_utf8_lossy(&res.stderr)
    );

    let mut ld = Command::new("ld.lld");
    ld.args(["-m", emulation, "-static", "--entry=main", "--gc-sections", "-o"]);
    ld.arg(out);
    for o in objects {
        ld.arg(o);
    }
    ld.arg(&stub_o);
    let res = ld.output().unwrap();
    assert!(
        res.status.success(),
        "ld.lld refused the cross objects:\n{}",
        String::from_utf8_lossy(&res.stderr)
    );
}

/// **Emission throughput, cross-triple, against Cranelift.**
///
/// The one measurement a machine that cannot *run* a Linux artifact can still
/// make honestly, and the one the benchmark's `lower+linux-*` rows already make
/// for Cranelift: how long it takes to turn a lowered program into object
/// bytes for a target that is not this one. Nothing is linked and nothing is
/// run, here or there.
///
/// It prints rather than only asserting, because the number is the point and a
/// pass/fail hides it. The assertion is deliberately loose — a wall clock on a
/// laptop under `cargo test` is not a benchmark harness — and exists only to
/// catch the direction reversing: this backend's whole compile-side claim is
/// that copying and patching is faster than instruction selection, and a run
/// where it is *slower* is a fact worth failing on even at this precision.
#[test]
fn cross_emission_throughput_against_cranelift() {
    use buri::compiler::backend::cranelift::Cranelift;
    use std::time::Instant;

    if !buri::compiler::backend::stencil::available_for(stencil_abi::StencilTarget::LinuxArm64) {
        eprintln!("no linux-arm64 stencils");
        return;
    }
    // A program with enough functions to measure: one emission of a
    // three-function snippet is dominated by process noise.
    let mut src = String::from("from \"core/host\" import { stdout };\n");
    for i in 0..300 {
        src.push_str(&format!(
            "fn f{i}(n: Int, m: Int): Int {{ let a = n * {i} + m; \
             if (a > 100) {{ a - m }} else {{ a + n }} }}\n"
        ));
    }
    src.push_str("export fn main(): Result<(), Str> {\n  let t0 = 0;\n");
    for i in 0..300 {
        src.push_str(&format!("  let t{} = t{i} + f{i}(t{i}, {i});\n", i + 1));
    }
    src.push_str("  let _ = stdout.println(\"${t300}\");\n  .Ok(())\n}\n");

    let (program, tables) = lowered(&src);
    let reps = 5;
    println!("target          stencil ms   cranelift ms   ratio   units");
    for (name, target) in [
        ("macos-arm64", Target { platform: Platform::Macos, arch: Some(Arch::Arm64) }),
        ("linux-arm64", Target { platform: Platform::Linux, arch: Some(Arch::Arm64) }),
    ] {
        let opts = Options { profile: Profile::Debug, target, unit_prefix: "cmd/app" };
        // One untimed emission each, so neither pays for the stencil library's
        // decode or Cranelift's ISA construction inside the measurement.
        let units = match Stencil.emit(&program, &tables, &opts) {
            Ok(u) => u,
            Err(d) => {
                eprintln!("{name}: stencil refused: {:?}", messages(&d));
                continue;
            }
        };
        let n_units = units.len();
        if Cranelift.emit(&program, &tables, &opts).is_err() {
            eprintln!("{name}: cranelift refused, so there is nothing to compare against");
            continue;
        }
        let t0 = Instant::now();
        for _ in 0..reps {
            let _ = Stencil.emit(&program, &tables, &opts);
        }
        let cp = t0.elapsed().as_secs_f64() * 1000.0 / f64::from(reps);
        let t1 = Instant::now();
        for _ in 0..reps {
            let _ = Cranelift.emit(&program, &tables, &opts);
        }
        let cl = t1.elapsed().as_secs_f64() * 1000.0 / f64::from(reps);
        println!("{name:<15} {cp:>8.1}   {cl:>12.1}   {:>5.2}   {n_units}", cp / cl);
        assert!(
            cp < cl * 2.0,
            "{name}: stencil emitted in {cp:.1} ms against Cranelift's {cl:.1}; this backend's \
             compile-side claim is the other way round"
        );
    }
}
