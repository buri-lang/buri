//! Programs compiled through the whole native pipeline, linked, and **run**.
//!
//! The bar for the Cranelift backend is not "the object file has the right
//! sections". It is that a Buri program goes front end -> `middle::run` ->
//! `middle::native` -> `middle::lower` -> `backend::cranelift` -> object ->
//! `cc` -> executable, and that the executable prints what the language says
//! it prints. Everything short of that is a claim about an intermediate
//! representation, and this compiler has enough of those already.
//!
//! The link is a plain `cc <objects> libburi_rt.a` here rather than
//! `build/link.rs`'s real link step, which selects mold or lld and caches the
//! result (CODEGEN-CRANELIFT.md §7); nothing in this file depends on which
//! linker ran, so the rows say the same thing under either.
//!
//! `main.rs` declares this module behind `backend-cranelift`. With the
//! feature off there is no module and the suite is silent rather than red,
//! which is what "degrades rather than breaks" means for a test.
use buri::build::buildfile::Platform;
use buri::compiler::backend::runtime_native::{ARCHIVE, ARCHIVE_NAME, AVAILABLE};
use buri::compiler::backend::{Backend, Options, Profile, Target};
use buri::compiler::backend::cranelift::Cranelift;
use buri::compiler::driver;
use buri::compiler::middle::{self, monomorphize};
use buri::compiler::modules::Role;
use buri::diagnostics::{Diagnostics, SourceMap};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

/// Whether this host can build and run a native artifact at all.
///
/// `AVAILABLE` is false where `cli/build.rs` built no runtime, which is the
/// same set of hosts that has no native backend to link one into. A test
/// that skips there is the "degrades rather than breaks" clause of the
/// dependency bar applied to the suite.
fn supported() -> bool {
    AVAILABLE && cfg!(any(target_os = "macos", target_os = "linux"))
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
        .join(format!("native-cranelift-{}", std::process::id()))
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
            .join(format!("native-cranelift-{}", std::process::id()));
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
fn build_with(name: &str, source: &str, probe: Option<&str>) -> PathBuf {
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

    let target = Target { platform: host_platform(), arch: None };
    let opts = Options { profile: Profile::Debug, target, unit_prefix: "" };
    let mut backend = Cranelift;
    let units = match backend.emit(&program, &analysis.checked.tables, &opts) {
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

fn run(name: &str, source: &str) -> Ran {
    run_with(name, source, None)
}

fn run_with(name: &str, source: &str, probe: Option<&str>) -> Ran {
    crate::shared::ran(&build_with(name, source, probe))
}

/// The first program. It prints, it exits zero, and every claim this
/// backend makes rests on this one working.
#[test]
fn hello_world_prints_and_exits_zero() {
    if !supported() {
        return;
    }
    let r = run(
        "hello",
        r#"
from "core/host" import { stdout };

export fn main(): Result<(), Str> {
  let _ = stdout.println("hello, world");
  .Ok(())
}
"#,
    );
    assert_eq!(r.stdout, "hello, world\n", "stderr: {}", r.stderr);
    assert_eq!(r.status, 0);
}

/// Arithmetic, a template hole at `Int`, and the generated `show`.
#[test]
fn arithmetic_and_interpolation() {
    if !supported() {
        return;
    }
    let r = run(
        "arithmetic",
        r#"
from "core/host" import { stdout };

export fn add(a: Int, b: Int): Int { a + b }
export fn triple(a: Int): Int { a * 3 }

export fn main(): Result<(), Str> {
  let s = add(20, 22);
  let t = triple(s);
  let d = t / 7;
  let m = t % 5;
  let n = 0 - 17;
  let _ = stdout.println("${s} ${t} ${d} ${m} ${n}");
  .Ok(())
}
"#,
    );
    assert_eq!(r.stdout, "42 126 18 1 -17\n", "stderr: {}", r.stderr);
    assert_eq!(r.status, 0);
}

/// A branch, a comparison, and `Bool` rendering.
#[test]
fn branches_and_booleans() {
    if !supported() {
        return;
    }
    let r = run(
        "branches",
        r#"
from "core/host" import { stdout };

export fn bigger(a: Int, b: Int): Int { if (a > b) { a } else { b } }

export fn main(): Result<(), Str> {
  let a = bigger(3, 9);
  let b = bigger(9, 3);
  let c = a == b;
  let _ = stdout.println("${a} ${b} ${c}");
  .Ok(())
}
"#,
    );
    assert_eq!(r.stdout, "9 9 true\n", "stderr: {}", r.stderr);
}

/// An enum with payloads, a `match`, and the `Switch` the design's §3.1
/// describes — including the projections that follow a tag test.
#[test]
fn enums_and_matches() {
    if !supported() {
        return;
    }
    let r = run(
        "enums",
        r#"
from "core/host" import { stdout };

export enum Shape {
  Circle(Int),
  Rect(Int, Int),
  Point,
}

export fn area(s: Shape): Int {
  match (s) {
    .Circle(r) => r * r * 3,
    .Rect(w, h) => w * h,
    .Point => 0,
  }
}

export fn main(): Result<(), Str> {
  let a = area(.Circle(4));
  let b = area(.Rect(3, 5));
  let c = area(.Point);
  let _ = stdout.println("${a} ${b} ${c}");
  .Ok(())
}
"#,
    );
    assert_eq!(r.stdout, "48 15 0\n", "stderr: {}", r.stderr);
}

/// A struct built, stored and projected.
#[test]
fn structs_are_built_and_projected() {
    if !supported() {
        return;
    }
    let r = run(
        "structs",
        r#"
from "core/host" import { stdout };

export struct Point { x: Int, y: Int }

export fn shift(p: Point, by: Int): Point {
  Point { x: p.x + by, y: p.y + by }
}

export fn main(): Result<(), Str> {
  let p = shift(Point { x: 1, y: 2 }, 10);
  let _ = stdout.println("${p.x} ${p.y}");
  .Ok(())
}
"#,
    );
    assert_eq!(r.stdout, "11 12\n", "stderr: {}", r.stderr);
}

/// A list literal, `len`, indexing through a match, and the tail-recursive
/// fold the middle end has already turned into a loop.
///
/// SPEC 8.3's constant stack is delivered by `middle::tail_calls`, not by
/// this backend (§3.3): what arrives here is a `Loop` with a back edge, and
/// the depth below is what would blow a stack if it were not one.
#[test]
fn lists_and_tail_recursion() {
    if !supported() {
        return;
    }
    let r = run(
        "lists",
        r#"
from "core/host" import { stdout };

export fn total(xs: [Int], acc: Int): Int {
  match (xs) {
    [] => acc,
    [h, ..t] => total(t, acc + h),
  }
}

export fn count(n: Int, acc: Int): Int {
  if (n == 0) { acc } else { count(n - 1, acc + n) }
}

export fn main(): Result<(), Str> {
  let xs = [1, 2, 3, 4, 5];
  let s = total(xs, 0);
  let deep = count(50000, 0);
  let _ = stdout.println("${s} ${deep}");
  .Ok(())
}
"#,
    );
    assert_eq!(r.stdout, "15 1250025000\n", "stderr: {}", r.stderr);
    assert_eq!(r.status, 0);
}

/// A closure over a captured local, called through its value: the
/// `call_indirect` of §3.2, and the environment block of `emit.rs`'s
/// header.
#[test]
fn closures_capture_and_are_called_indirectly() {
    if !supported() {
        return;
    }
    let r = run(
        "closures",
        r#"
from "core/host" import { stdout };

export fn apply(f: fn(Int) => Int, v: Int): Int { f(v) }
export fn twice(f: fn(Int) => Int, v: Int): Int { apply(f, apply(f, v)) }

export fn main(): Result<(), Str> {
  let n = 7;
  let a = apply(fn(v) => v + n, 10);
  let b = twice(fn(v) => v * 2, 3);
  let _ = stdout.println("${a} ${b}");
  .Ok(())
}
"#,
    );
    assert_eq!(r.stdout, "17 12\n", "stderr: {}", r.stderr);
}

/// String concatenation, which the backend generates rather than calls
/// (`helpers.rs`), and a literal `Str`, which touches no allocator.
#[test]
fn strings_concatenate() {
    if !supported() {
        return;
    }
    let r = run(
        "strings",
        r#"
from "core/host" import { stdout };

export fn main(): Result<(), Str> {
  let who = "world";
  let empty = "";
  let _ = stdout.println("hello, ${who}!");
  let _ = stdout.println("hello, ${empty}!");
  .Ok(())
}
"#,
    );
    assert_eq!(r.stdout, "hello, world!\nhello, !\n", "stderr: {}", r.stderr);
}

/// `.Err(msg)` prints to standard error and exits 1, which is the exit
/// convention the JavaScript backend already has (`generate.rs:293`). One
/// sentence in two backends.
#[test]
fn an_error_return_prints_and_exits_one() {
    if !supported() {
        return;
    }
    let r = run(
        "error",
        r#"
from "core/host" import { stdout };

export fn main(): Result<(), Str> {
  let _ = stdout.println("before");
  .Err("it did not work")
}
"#,
    );
    assert_eq!(r.stdout, "before\n");
    assert_eq!(r.stderr, "it did not work\n");
    assert_eq!(r.status, 1);
}

/// Division by zero aborts with the runtime's message, which is where it
/// lives so that `cli/tests/crash/` pins one string for both backends
/// (CODEGEN-CRANELIFT.md §3.7).
#[test]
fn dividing_by_zero_aborts_with_the_runtime_message() {
    if !supported() {
        return;
    }
    let r = run(
        "divzero",
        r#"
from "core/host" import { stdout };

export fn divide(a: Int, b: Int): Int { a / b }

export fn main(): Result<(), Str> {
  let zero = 0;
  let _ = stdout.println("${divide(1, zero)}");
  .Ok(())
}
"#,
    );
    assert_ne!(r.status, 0, "a division by zero must not exit cleanly");
    assert!(
        r.stderr.contains("division by zero"),
        "stderr was {:?}",
        r.stderr
    );
}

/// `core/alloc`'s three allocators, natively, printing the numbers the cost
/// model defines.
///
/// The numbers are the assertion and they are written out rather than
/// computed, because they are the same numbers the JavaScript backend
/// prints for this program: `cli/tests/conformance/lib/memory/` runs the
/// identical arithmetic on both, and MEMORY.md §7.1 is why that is a
/// theorem rather than a coincidence — the charge is a function of the
/// types, not a measurement of an allocator.
///
/// It also exercises the shape this wave needed to work: three *non*
/// zero-sized implementations of `Alloc`, each in its own context, each
/// carrying a handle into `cli/runtime/memory.rs`'s counters.
#[test]
fn the_three_allocators_count_the_defined_charges() {
    if !supported() {
        return;
    }
    let r = run(
        "allocators",
        r#"
from "core/alloc" import * as alloc;
from "core/effect" import { Alloc, Stdout };
from "core/host" import * as host;

export fn main(): Result<(), Str> {
  let gp = alloc.generalPurpose();
  let scratch = alloc.arena();
  let buffer = alloc.fixedBuffer(64);
  let ctx = context { Alloc: gp, Stdout: host.stdout };
  let inArena = context { Alloc: scratch };
  let inBuffer = context { Alloc: buffer };
  let _ = ctx.allocate(64);
  let _ = ctx.allocate(alloc.strBytes(5));
  let _ = inArena.allocate(alloc.listBytes(4, 8));
  let _ = inBuffer.allocate(24);
  let g = gp.stats();
  let a = scratch.stats();
  let b = buffer.stats();
  let _ = ctx.println("gp ${g.allocations} ${g.bytes}");
  let _ = ctx.println("arena ${a.allocations} ${a.bytes}");
  let _ = ctx.println("buffer ${b.allocations} ${b.bytes} ${buffer.remaining()} ${buffer.budget}");
  .Ok(())
}
"#,
    );
    assert_eq!(
        r.stdout, "gp 2 85\narena 1 48\nbuffer 1 24 40 64\n",
        "stderr: {}",
        r.stderr
    );
    assert_eq!(r.status, 0);
}

/// A `FixedBuffer` overrun ends the process with the budget and the request
/// in the message.
///
/// The same program is `cli/tests/crash/alloc_budget_exhausted.buri` on the
/// JavaScript backend, and the message is one string in
/// `cli/runtime/abort.rs` and one in `runtime.js` that copies it — so this
/// test and that corpus pin the same sentence, which is what makes a budget
/// a portable assertion rather than a native one.
#[test]
fn a_fixed_buffer_overrun_aborts_with_the_budget_and_the_request() {
    if !supported() {
        return;
    }
    let r = run(
        "allocbudget",
        r#"
from "core/alloc" import * as alloc;
from "core/effect" import { Alloc, Stdout };
from "core/host" import * as host;

export fn main(): Result<(), Str> {
  let buffer = alloc.fixedBuffer(64);
  let ctx = context { Alloc: buffer, Stdout: host.stdout };
  let _ = ctx.allocate(32);
  let _ = ctx.println("${buffer.remaining()} bytes left");
  let _ = ctx.allocate(40);
  let _ = ctx.println("unreachable");
  .Ok(())
}
"#,
    );
    assert_ne!(r.status, 0, "an exhausted budget must not exit cleanly");
    assert_eq!(r.stdout, "32 bytes left\n");
    assert!(
        r.stderr
            .contains("allocation budget exhausted: 40 bytes requested against a budget of 64"),
        "stderr was {:?}",
        r.stderr
    );
}

/// Two units, and a call across the boundary between them.
///
/// `core/host`'s functions are in `core_host` and the program's are in
/// `main`, so any program that prints already crosses a unit — but the
/// callee there is a runtime `Import`. This one crosses to a *defined*
/// symbol, which is the `Hidden` linkage of §6 and the thing that would
/// break if it were `Local`.
const CROSS_UNIT: &str = r#"
from "core/host" import { stdout };

export fn main(): Result<(), Str> {
  let some: Option<Int> = .Some(3);
  let none: Option<Int> = .None;
  let a = some.isSome();
  let b = none.isSome();
  let _ = stdout.println("${a} ${b}");
  .Ok(())
}
"#;

#[test]
fn a_call_crosses_a_codegen_unit() {
    if !supported() {
        return;
    }
    // More than one unit, and the artifact links: a symbol another unit
    // defines has to be reachable from this one, which is what `Hidden`
    // buys and what `Local` would break at the link rather than at run
    // time.
    let units = emit_units(CROSS_UNIT);
    let names: Vec<String> = units.iter().map(|u| u.0.clone()).collect();
    assert!(names.len() > 1, "expected more than one codegen unit, got {names:?}");
    assert!(names.contains(&String::from("main.o")), "{names:?}");
    let r = run("cross_unit", CROSS_UNIT);
    assert_eq!(r.stdout, "true false\n", "stderr: {}", r.stderr);
}

/// The niche-encoded `Option<Str>` (VALUE-MODEL.md §6): `.None` is the
/// `ptr` word set to null, so the value is 24 bytes and testing it is one
/// compare against zero. Nothing else in the pipeline knows that, which is
/// why it is worth running rather than asserting about a layout.
#[test]
fn a_niche_encoded_option_round_trips() {
    if !supported() {
        return;
    }
    let r = run(
        "niche",
        r#"
from "core/host" import { stdout };

export fn describe(x: Option<Str>): Str {
  match (x) {
    .Some(s) => s,
    .None => "nothing",
  }
}

export fn main(): Result<(), Str> {
  let there: Option<Str> = .Some("here");
  let gone: Option<Str> = .None;
  let _ = stdout.println(describe(there));
  let _ = stdout.println(describe(gone));
  .Ok(())
}
"#,
    );
    assert_eq!(r.stdout, "here\nnothing\n", "stderr: {}", r.stderr);
}

/// A list whose elements are counted, walked and rebuilt: the drop glue of
/// `helpers.rs` and the element retain `ArraySlice` emits.
#[test]
fn a_list_of_strings_is_walked() {
    if !supported() {
        return;
    }
    let r = run(
        "list_of_str",
        r#"
from "core/effect" import { Alloc };
from "core/host" import { stdout };
from "core/host" import * as host;

export fn join<C: Alloc>(ctx: C, xs: [Str], acc: Str): Str {
  match (xs) {
    [] => acc,
    [h, ..t] => join(ctx, t, acc.concat(ctx, h)),
  }
}

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc };
  let xs = ["ab", "c", "de"];
  let joined = join(ctx, xs, "");
  let n = xs.len();
  let wide = joined.len();
  let _ = stdout.println(joined);
  let _ = stdout.println("${n} ${wide}");
  .Ok(())
}
"#,
    );
    assert_eq!(r.stdout, "abcde\n3 5\n", "stderr: {}", r.stderr);
    assert_eq!(r.status, 0);
}

/// A derived `Eq` over a struct: `middle::derives` generated the function
/// and this backend compiled it like any other, which is the whole of
/// VALUE-MODEL.md §9's claim that no descriptor reaches a native artifact.
#[test]
fn a_derived_equality_runs() {
    if !supported() {
        return;
    }
    let r = run(
        "derive_eq",
        r#"
from "core/host" import { stdout };

export struct Point { x: Int, y: Int }
derive Eq for Point;

export fn main(): Result<(), Str> {
  let a = Point { x: 1, y: 2 };
  let b = Point { x: 1, y: 2 };
  let c = Point { x: 1, y: 3 };
  let same = a == b;
  let other = a == c;
  let _ = stdout.println("${same} ${other}");
  .Ok(())
}
"#,
    );
    assert_eq!(r.stdout, "true false\n", "stderr: {}", r.stderr);
}

/// The unit names and keys the build system will index by.
///
/// The key is `H(the unit's lowered IR)` (ARCHITECTURE.md §6.2), so two
/// emissions of one program agree and the object bytes are reproducible —
/// which is what `--check-reproducible` compares.
#[test]
fn two_emissions_of_one_program_agree() {
    if !supported() {
        return;
    }
    let source = r#"
from "core/host" import { stdout };
export fn main(): Result<(), Str> {
  let _ = stdout.println("stable");
  .Ok(())
}
"#;
    let first = emit_units(source);
    let second = emit_units(source);
    assert_eq!(first.len(), second.len());
    for (a, b) in first.iter().zip(second.iter()) {
        assert_eq!(a.0, b.0, "unit names differ");
        assert_eq!(a.1, b.1, "unit keys differ for {}", a.0);
        assert_eq!(a.2, b.2, "object bytes differ for {}", a.0);
    }
    assert!(first.iter().any(|u| u.0 == "main.o"), "{:?}", first);
}

fn emit_units(source: &str) -> Vec<(String, String, Vec<u8>)> {
    let mut map = SourceMap::new();
    let analysis = driver::analyze_snippet(&mut map, "main", source, Role::Entry);
    let entry = analysis.checked.entry.expect("the snippet exports `main`");
    let paths: Vec<String> = analysis.loaded.modules.iter().map(|m| m.path.clone()).collect();
    let mut diagnostics = Diagnostics::new();
    let mut program = monomorphize::run(
        &analysis.checked,
        paths,
        &mut diagnostics,
        monomorphize::Roots::Main(entry),
    );
    middle::run(&mut program, &middle::Options::default());
    middle::native(&mut program);
    let target = Target { platform: host_platform(), arch: None };
    let opts = Options { profile: Profile::Debug, target, unit_prefix: "" };
    let mut backend = Cranelift;
    let units = match backend.emit(&program, &analysis.checked.tables, &opts) {
        Ok(units) => units,
        Err(d) => panic!(
            "the backend refused the program: {:?}",
            d.items.iter().map(|i| i.message.clone()).collect::<Vec<_>>()
        ),
    };
    units.into_iter().map(|u| (u.name, u.key.as_str().to_string(), u.bytes)).collect()
}

// -----------------------------------------------------------------------
// The intrinsic surface
// -----------------------------------------------------------------------

/// `Float` rendering, which is the headline correctness item of that surface.
///
/// Every string here is what `bun` prints for the same value — the four
/// presentation cases of ECMA-262 §6.1.6.1.20 and the three non-finite
/// spellings `$f64` gives them. `cli/tests/native/float_parity.rs` is the
/// four-million-value version; this is the one that fails first and reads
/// like a specification.
#[test]
fn floats_render_exactly_as_javascript_does() {
    if !supported() {
        return;
    }
    let r = run(
        "floats",
        r#"
from "core/host" import { stdout };

export fn main(): Result<(), Str> {
  let a = 0.1;
  let b = 1.0;
  let c = 1.0 / 3.0;
  let _ = stdout.println("${a} ${b} ${c}");
  .Ok(())
}
"#,
    );
    assert_eq!(r.stdout, "0.1 1.0 0.3333333333333333\n", "stderr: {}", r.stderr);
}

/// A `Str` sliced, trimmed and searched — the pure half of `core/str`,
/// which answers views into the receiver rather than copies.
#[test]
fn the_pure_string_surface_answers_views() {
    if !supported() {
        return;
    }
    let r = run(
        "str_pure",
        r#"
from "core/host" import { stdout };

export fn main(): Result<(), Str> {
  let s = "  hello, world  ";
  let t = s.trim();
  let head = t.slice(0, 5);
  let has = t.contains("world");
  let starts = t.startsWith("hello");
  let n = t.len();
  let _ = stdout.println("[${t}] [${head}] ${has} ${starts} ${n}");
  .Ok(())
}
"#,
    );
    assert_eq!(
        r.stdout,
        "[hello, world] [hello] true true 12\n",
        "stderr: {}",
        r.stderr
    );
}

/// `str.len` counts Unicode scalars, not bytes, and the ASCII flag is what
/// makes the common case a mask rather than a scan (VALUE-MODEL.md §3.1).
/// A non-ASCII string takes the other path, and both have to agree with the
/// JavaScript backend's `$str_len`.
#[test]
fn a_scalar_index_is_not_a_byte_offset() {
    if !supported() {
        return;
    }
    let r = run(
        "str_utf8",
        r#"
from "core/host" import { stdout };

export fn main(): Result<(), Str> {
  let s = "aébc漢";
  let n = s.len();
  let mid = s.slice(1, 3);
  let _ = stdout.println("${n} [${mid}]");
  .Ok(())
}
"#,
    );
    assert_eq!(r.stdout, "5 [éb]\n", "stderr: {}", r.stderr);
}

/// An `Option` coming back from the runtime: `lib.rs` §2 rule 3's
/// discriminant, turned into whatever `middle::layout` chose for the enum.
/// `toInt` is the tagged case and `splitOnce` the niche one, so both
/// translations are exercised.
#[test]
fn an_option_crosses_the_c_boundary() {
    if !supported() {
        return;
    }
    let r = run(
        "str_option",
        r#"
from "core/host" import { stdout, alloc };
from "core/str" import * as str;

export fn main(): Result<(), Str> {
  let good = match ("42".toInt()) { .Some(n) => n, .None => 0 - 1 };
  let bad = match ("4x".toInt()) { .Some(n) => n, .None => 0 - 1 };
  let at = match ("a,b".indexOf(",")) { .Some(i) => i, .None => 0 - 1 };
  let halves = match (",b".splitOnce(",")) {
    .Some(pair) => str.format(alloc, "[${pair.0}][${pair.1}]"),
    .None => "none",
  };
  let _ = stdout.println("${good} ${bad} ${at} ${halves}");
  .Ok(())
}
"#,
    );
    // The empty first half is the case a null `ptr` would misreport as
    // `.None`, which is why `BuriStr::empty` has an address.
    assert_eq!(r.stdout, "42 -1 1 [][b]\n", "stderr: {}", r.stderr);
}

/// The `Alloc`-bounded half of `core/str`: every one of these builds a
/// fresh block, and `split` builds a `[Str]` whose elements are views that
/// each hold a count on the receiver's block.
#[test]
fn the_allocating_string_surface_builds_blocks() {
    if !supported() {
        return;
    }
    let r = run(
        "str_alloc",
        r#"
from "core/host" import { stdout, alloc };
from "core/list" import * as list;

export fn main(): Result<(), Str> {
  let parts = "a,b,c".split(alloc, ",");
  let joined = parts.join(alloc, "-");
  let up = "hi".toUpper(alloc);
  let rep = "ab".repeat(alloc, 3);
  let sub = "banana".replace(alloc, "na", "NA");
  let pad = "7".padStart(alloc, 3, '0');
  let _ = stdout.println("${joined} ${up} ${rep} ${sub} ${pad} ${parts.len()}");
  .Ok(())
}
"#,
    );
    assert_eq!(r.stdout, "a-b-c HI ababab baNANA 007 3\n", "stderr: {}", r.stderr);
}

/// The block-copying half of `core/list`, including the retain glue: a
/// `[Str]` that is concatenated holds new counts on the same string blocks,
/// and freeing either list must not free the strings the other still names.
#[test]
fn a_list_of_strings_copies_with_its_counts() {
    if !supported() {
        return;
    }
    let r = run(
        "list_copy",
        r#"
from "core/host" import { stdout, alloc };
from "core/list" import * as list;

export fn main(): Result<(), Str> {
  let a = ["x", "y"];
  let b = a.concat(alloc, ["z"]);
  let c = b.reverse(alloc);
  let d = c.push(alloc, "w");
  let e = d.slice(alloc, 1, 3);
  let f = list.range(alloc, 2, 5);
  let g = match (d.get(0)) { .Some(s) => s, .None => "?" };
  let h = d.take(alloc, 2);
  let i = d.drop(alloc, 2);
  let _ = stdout.println("${b.join(alloc, "")} ${c.join(alloc, "")} ${e.join(alloc, "")} ${f.len()} ${g} ${h.join(alloc, "")} ${i.join(alloc, "")}");
  .Ok(())
}
"#,
    );
    assert_eq!(r.stdout, "xyz zyx yx 3 z zy xw\n", "stderr: {}", r.stderr);
}

/// `Eq`, `Ord` and `Hash` at a primitive, and the two `Bounded` methods.
/// The hash numbers are `$hash`'s, because VALUE-MODEL.md §12 says a
/// program printing `x.hash()` prints the same number on both backends.
#[test]
fn the_structural_traits_agree_with_javascript() {
    if !supported() {
        return;
    }
    let r = run(
        "traits",
        r#"
from "core/host" import { stdout };
from "core/order" import { Order };

fn name(o: Order): Str {
  match (o) { .Less => "lt", .Equal => "eq", .Greater => "gt" }
}

export fn main(): Result<(), Str> {
  let a = name((1).compare(2));
  let b = name("b".compare("a"));
  let h = (7).hash();
  let s = "ab".hash();
  // `Bool` and `Char` get their own impls from `semantics::builtins`, so their
  // keys are `bool.*` and `char.*` rather than `num.<T>.*`.
  let c = name(false.compare(true));
  let d = name('a'.compare('b'));
  let e = true.hash();
  let f = 'a'.hash();
  let _ = stdout.println("${a} ${b} ${h} ${s} ${c} ${d} ${e} ${f}");
  .Ok(())
}
"#,
    );
    // `$hash(7)` and `$hash("ab")` under the JavaScript runtime, computed
    // from `$mix`/`$hashInto` rather than recorded from a native run.
    assert_eq!(
        r.stdout,
        "lt gt 34363494 1294271946 lt lt 67918732 3826002220\n",
        "stderr: {}",
        r.stderr
    );
}

/// `checked*` and `saturating*`, which answer an `Option<T>` and a clamped
/// value: the two shapes this backend once listed as absent because
/// "constructing one needs the layout of a type the intrinsic table does not
/// name", and emits now.
#[test]
fn checked_and_saturating_arithmetic() {
    if !supported() {
        return;
    }
    let r = run(
        "checked",
        r#"
from "core/host" import { stdout };
from "core/num" import * as num;

export fn main(): Result<(), Str> {
  let big: I8 = 100;
  let small: I8 = 0 - 100;
  let ok = match (big.checkedAdd(20)) { .Some(v) => v, .None => 0 - 1 };
  let over = match (big.checkedAdd(100)) { .Some(v) => v, .None => 0 - 1 };
  let dz = match (big.checkedDiv(0)) { .Some(v) => v, .None => 0 - 1 };
  let sat = big.saturatingAdd(100);
  let low = small.saturatingSub(100);
  let _ = stdout.println("${ok} ${over} ${dz} ${sat} ${low}");
  .Ok(())
}
"#,
    );
    assert_eq!(r.stdout, "120 -1 -1 127 -128\n", "stderr: {}", r.stderr);
}

/// A derived `Show`, which is where `derivePrimShow` differs from a
/// template hole: a `Str` field is quoted and escaped and a `Char` is in
/// single quotes, exactly as `$show`'s primitive arm renders them.
#[test]
fn a_derived_show_quotes_what_javascript_quotes() {
    if !supported() {
        return;
    }
    let r = run(
        "derive_show",
        r#"
from "core/host" import { stdout, alloc };

export struct Point { x: Int, label: Str }

derive Show for Point;

export fn main(): Result<(), Str> {
  let p = Point { x: 3, label: "a\"b" };
  let _ = stdout.println(p.show(alloc));
  .Ok(())
}
"#,
    );
    assert_eq!(r.stdout, "Point { x: 3, label: \"a\\\"b\" }\n", "stderr: {}", r.stderr);
}

/// `core/bits`, which is open-coded: every entry is one machine
/// instruction behind the range check `$shiftCount` performs
/// (`runtime.js:923-928`).
#[test]
fn the_bit_operations_are_instructions() {
    if !supported() {
        return;
    }
    let r = run("bits", r#"
from "core/host" import { stdout };
from "core/bits" import * as bits;

export fn main(): Result<(), Str> {
  let a = bits.shl(1, 4);
  let b = bits.shr(256, 4);
  let c = bits.sar(0 - 256, 4);
  let d = bits.popCount(255);
  let e = bits.leadingZeros(1);
  let f = bits.rotateLeft(1, 1);
  let _ = stdout.println("${a} ${b} ${c} ${d} ${e} ${f}");
  .Ok(())
}
"#);
    assert_eq!(r.stdout, "16 16 -16 8 63 2\n", "stderr: {}", r.stderr);
}

/// 128-bit `Checked`, `Saturating` and `Bounded`.
///
/// These are the arms that go to the runtime rather than being open-coded:
/// the 64-bit overflow test is `smulhi`/`umulhi` and Cranelift defines
/// neither at `i128`, so `buri_rt_i128_checked` is one call for four
/// operations (`cli/runtime/lib.rs`).
#[test]
fn wide_integers_are_checked_saturated_and_bounded() {
    if !supported() {
        return;
    }
    let r = run("i128", r#"
from "core/host" import { stdout };
from "core/num" import * as num;

export fn main(): Result<(), Str> {
  let a: I128 = 100;
  let ok = match (a.checkedAdd(20)) { .Some(v) => v, .None => 0 - 1 };
  let sat = a.saturatingMul(3);
  let mx = num.maxValue<I128>();
  let _ = stdout.println("${ok} ${sat} ${mx}");
  .Ok(())
}
"#);
    assert_eq!(r.stdout, "120 300 170141183460469231731687303715884105727\n", "stderr: {}", r.stderr);
}

/// The half of `core/math` whose answer IEEE 754 fixes.
///
/// `round` is the one that is not `f64::round`: `Math.round` breaks a tie
/// toward positive infinity, so `-1.5` is `-1` and not `-2`, and `-0.4` is
/// `-0` and not `0` — which print differently.
#[test]
fn the_specified_half_of_math_agrees_with_javascript() {
    if !supported() {
        return;
    }
    let r = run(
        "math",
        r#"
from "core/host" import { stdout };
from "core/math" import * as math;

export fn main(): Result<(), Str> {
  let a = math.sqrt(2.0);
  let b = math.round(0.0 - 1.5);
  let c = math.round(0.0 - 0.4);
  let d = math.floor(0.0 - 1.5);
  let e = math.ceil(1.2);
  let f = math.absFloat(0.0 - 3.5);
  let g = math.isNan(0.0 / 0.0);
  let _ = stdout.println("${a} ${b} ${c} ${d} ${e} ${f} ${g}");
  .Ok(())
}
"#,
    );
    assert_eq!(
        r.stdout,
        "1.4142135623730951 -1.0 -0.0 -2.0 2.0 3.5 true\n",
        "stderr: {}",
        r.stderr
    );
}

/// UTF-8 slicing where the ASCII fast path does not apply, through a
/// context whose allocator **carries state**.
///
/// The second half is the load-bearing one. Every `<C: Alloc>` bound is a
/// zero-sized context in a program using `core/host`, so a runtime entry
/// could get away with letting it spread to no leaves — until a test builds
/// `context { Alloc: alloc() }` from `core/testing/context`, whose
/// `TestAlloc` carries an `I64`. Then the context spread an extra argument
/// into a C call with no parameter for it. `codegen/strings.buri` is where
/// that was found, and this is the small version of it.
#[test]
fn a_stateful_context_is_still_dropped_at_the_c_boundary() {
    if !supported() {
        return;
    }
    let r = run("astral", r#"
from "core/host" import { stdout, alloc };
from "core/str" import * as str;
from "core/list" import * as list;

export fn main(): Result<(), Str> {
  let s = "a\u{1F600}b";
  let n = s.len();
  let c = match (s.charAt(1)) { .Some(c) => c, .None => '?' };
  let sl = s.slice(0, 2);
  let i = match (s.indexOf("b")) { .Some(i) => i, .None => 0 - 1 };
  let p = "abc".padStart(alloc, 5, '.');
  let cs = s.chars(alloc);
  let _ = stdout.println("${n} ${c} ${sl} ${i} ${p} ${cs.len()}");
  .Ok(())
}
"#);
    assert_eq!(r.stdout, "3 \u{1F600} a\u{1F600} 2 ..abc 3\n", "stderr: {}", r.stderr);
}

/// The row `backend::select` gained: a native debug build is Cranelift.
///
/// It is asserted here rather than left to the build system, because the
/// table in `backend/mod.rs` is a claim about which backend compiles which
/// quadrant (ARCHITECTURE.md §4), and a claim nothing checks is one that
/// silently becomes "the JavaScript backend, for everything".
#[test]
fn a_native_debug_build_selects_this_backend() {
    use buri::compiler::backend::select;
    for platform in [Platform::Linux, Platform::Macos] {
        let target = Target { platform, arch: None };
        let backend = select(target, Profile::Debug).expect("a debug backend");
        assert_eq!(backend.name(), "cranelift");
    }
    // JavaScript is unaffected, and a native `--release` still names the
    // feature that is missing rather than falling back to this one.
    let js = select(Target { platform: Platform::Js, arch: None }, Profile::Debug)
        .expect("the JavaScript backend");
    assert_eq!(js.name(), "js");
}

/// An intrinsic the native runtime has no entry for is reported before the
/// backend spends anything on the program, which is what the signature of
/// `missing_intrinsics` is for.
#[test]
fn an_unimplemented_intrinsic_is_reported_up_front() {
    let source = r#"
from "core/host" import { stdout, fs };

export fn main(): Result<(), Str> {
  match (fs.readFile("x")) {
    .Ok(t) => stdout.println(t),
    .Err(_e) => stdout.println("no"),
  }
  .Ok(())
}
"#;
    let mut map = SourceMap::new();
    let analysis = driver::analyze_snippet(&mut map, "main", source, Role::Entry);
    let Some(entry) = analysis.checked.entry else { return };
    let paths: Vec<String> = analysis.loaded.modules.iter().map(|m| m.path.clone()).collect();
    let mut diagnostics = Diagnostics::new();
    let mut program = monomorphize::run(
        &analysis.checked,
        paths,
        &mut diagnostics,
        monomorphize::Roots::Main(entry),
    );
    middle::run(&mut program, &middle::Options::default());
    middle::native(&mut program);
    let missing = Cranelift.missing_intrinsics(&program, &analysis.checked.tables);
    assert!(
        missing.iter().any(|m| m == "host.HostFs.readFile"),
        "expected `readFile` to be reported, got {missing:?}"
    );
}

// -----------------------------------------------------------------------
// MEMORY.md §5.3: uniqueness, in-place growth, and what must not move
// -----------------------------------------------------------------------

/// MEMORY.md §5.3, pinned by allocation count rather than by reading the
/// emitted code.
///
/// `str.concat` is open-coded rather than called, so this backend's fast
/// path lives in `cranelift/helpers.rs`'s `concat` and not in the runtime —
/// it is written twice across the two backends and therefore has to be
/// pinned twice. A chain of a thousand concatenations onto a uniquely-owned
/// string reallocates O(log n) times; without the fast path it allocates
/// once per step.
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

/// A borrowed local handed to a construct **beside** a sibling that holds
/// its last mention — `middle::rc`'s `children`, and the reason the
/// deferral there exists.
///
/// `"${base} ${a} ${b} ${base.len()}"` is the shape: the first hole is
/// `base` itself, so the concatenation chain is holding three uncounted
/// words out of it while the last hole computes `base.len()`. A drop after
/// the rightmost mention frees the block those words point into, and the
/// failure is a wrong answer rather than a crash. It is a middle-end fact,
/// so both backends show it and both pin it.
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
    assert_eq!(
        r.stdout,
        "abcd abcd-one abcd-two 4\nabcd-two 8\n",
        "stderr: {}",
        r.stderr
    );
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

/// The list half of MEMORY.md §5.3, which lives in `cli/runtime/list.rs`
/// and is therefore the same code on both backends — but the *call* is
/// this backend's, so the count is asserted here as well.
#[test]
fn a_unique_push_loop_allocates_logarithmically() {
    if !supported() {
        return;
    }
    let many = run_with(
        "push-loop",
        r#"
from "core/host" import { stdout, alloc };
from "core/list" import * as list;

export fn build(xs: [Int], i: Int): [Int] {
  if (i == 0) { xs } else { build(xs.push(alloc, i), i - 1) }
}

export fn main(): Result<(), Str> {
  let xs = build([], 2000);
  let _ = stdout.println("${xs.len()}");
  .Ok(())
}
"#,
        Some(ALLOC_PROBE),
    );
    assert_eq!(many.stdout, "2000\n", "stderr: {}", many.stderr);
    let base = run_with(
        "push-loop-base",
        r#"
from "core/host" import { stdout, alloc };
from "core/list" import * as list;

export fn main(): Result<(), Str> {
  let xs = [1, 2, 3];
  let _ = stdout.println("${xs.len()}");
  .Ok(())
}
"#,
        Some(ALLOC_PROBE),
    );
    let (grown, live_grown) = probed(&many.stderr);
    let (flat, live_flat) = probed(&base.stderr);
    assert!(
        grown.saturating_sub(flat) < 50,
        "two thousand pushes allocated {grown} blocks against a {flat}-block baseline: \
             the uniqueness fast path did not fire"
    );
    // The other half of the claim, and the one the *grown* path can break
    // on its own: it allocates a bigger block and leaves the old one to the
    // `decref` the caller was going to emit anyway. If that hand-off is
    // wrong, the count above still passes and the heap still grows.
    assert_eq!(
        live_grown, live_flat,
        "two thousand pushes left {live_grown} blocks live against a {live_flat}-block \
             baseline: the growth path leaks the block it grew out of"
    );
}

/// The same guard for the list half: two pushes onto a list a binding
/// still holds must answer two distinct lists and leave the original
/// alone.
#[test]
fn a_shared_push_does_not_mutate_what_is_shared() {
    if !supported() {
        return;
    }
    let r = run(
        "push-shared",
        r#"
from "core/host" import { stdout, alloc };
from "core/list" import * as list;

export fn total(xs: [Int], acc: Int): Int {
  match (xs) {
    [] => acc,
    [h, ..t] => total(t, acc + h),
  }
}

export fn main(): Result<(), Str> {
  let xs = [1, 2, 3].push(alloc, 4);
  let a = xs.push(alloc, 100);
  let b = xs.push(alloc, 200);
  let _ = stdout.println("${xs.len()} ${total(xs, 0)}");
  let _ = stdout.println("${a.len()} ${total(a, 0)}");
  let _ = stdout.println("${b.len()} ${total(b, 0)}");
  .Ok(())
}
"#,
    );
    assert_eq!(r.stdout, "4 10\n5 110\n5 210\n", "stderr: {}", r.stderr);
    assert_eq!(r.status, 0);
}

/// `FuncPlan::reuse` is analysis only, and this is why it can be: a
/// functional record update **does not allocate**, so there is no cell for
/// in-place reuse to write into (`middle/rc.rs`, "Analysis only, and
/// deliberately so").
///
/// A thousand updates allocate exactly as many blocks as ten, because both
/// allocate none: a struct is a register or stack value on both backends.
#[test]
fn a_struct_update_loop_allocates_nothing_per_iteration() {
    if !supported() {
        return;
    }
    let source = |n: u32| {
        format!(
            r#"
from "core/host" import {{ stdout, alloc }};

struct Point {{ x: Int, y: Int }}

export fn step(p: Point, i: Int): Point {{
  if (i == 0) {{ p }} else {{ step(Point {{ ..p, x: p.x + i }}, i - 1) }}
}}

export fn main(): Result<(), Str> {{
  let p = step(Point {{ x: 0, y: 7 }}, {n});
  let _ = stdout.println("${{p.x}} ${{p.y}}");
  .Ok(())
}}
"#
        )
    };
    let many = run_with("struct-update-many", &source(1000), Some(ALLOC_PROBE));
    let few = run_with("struct-update-few", &source(10), Some(ALLOC_PROBE));
    assert_eq!(many.stdout, "500500 7\n", "stderr: {}", many.stderr);
    assert_eq!(few.stdout, "55 7\n", "stderr: {}", few.stderr);
    let (a, _) = probed(&many.stderr);
    let (b, _) = probed(&few.stderr);
    assert_eq!(
        a, b,
        "a hundredfold more struct updates allocated {} more block(s): an aggregate is on \
             the heap now, and `middle::rc`'s reuse plan has something to do",
        a.saturating_sub(b)
    );
}

/// The counted-element half of `list.push`: the elements a grown block
/// carries keep exactly the counts they had, so a push loop over `[Str]`
/// leaves nothing behind.
///
/// Measured as a differential rather than against zero, because the
/// interesting failure is *per push*: forty pushes and ten leaving the
/// same number of blocks live is the claim, and one leaked count per push
/// would make them differ by thirty.
#[test]
fn pushing_a_counted_element_type_is_unchanged() {
    if !supported() {
        return;
    }
    let source = |n: u32| {
        format!(
            r#"
from "core/host" import {{ stdout, alloc }};
from "core/list" import * as list;
from "core/str" import * as str;

export fn build(xs: [Str], i: Int): [Str] {{
  if (i == 0) {{ xs }} else {{ build(xs.push(alloc, "x".concat(alloc, str.fromInt(alloc, i))), i - 1) }}
}}

export fn main(): Result<(), Str> {{
  let xs = build([], {n});
  let joined = xs.join(alloc, "");
  let _ = stdout.println("${{xs.len()}} ${{joined.slice(0, 9)}}");
  .Ok(())
}}
"#
        )
    };
    let many = run_with("push-counted", &source(40), Some(ALLOC_PROBE));
    let few = run_with("push-counted-few", &source(10), Some(ALLOC_PROBE));
    assert_eq!(many.stdout, "40 x40x39x38\n", "stderr: {}", many.stderr);
    assert_eq!(few.stdout, "10 x10x9x8x7\n", "stderr: {}", few.stderr);
    let (blocks, live_many) = probed(&many.stderr);
    let (_, live_few) = probed(&few.stderr);
    assert_eq!(
        live_many, live_few,
        "forty pushes left {live_many} blocks live and ten left {live_few}: \
             a counted element type leaks per push"
    );
    // The exclusion, pinned from the other side. `append_dest` takes
    // neither the in-place path nor the over-allocation when `retain` is
    // non-null, so a `[Str]` still allocates once per push; a change that
    // lifted the guard without also giving this ABI a per-element release
    // glue would pass every assertion above and leak counts instead.
    assert!(
        blocks >= 40,
        "forty pushes over a counted element type allocated {blocks} blocks: the fast path \
             fired on a `[Str]`, and `cli/runtime/list.rs`'s `append_dest` says why it must not"
    );
}

// -----------------------------------------------------------------------
// A projection is not where its base dies
// -----------------------------------------------------------------------

/// An aggregate holding two counted values, read through its own
/// projections.
///
/// `middle::rc` placed the base's `decref` at the projection that was its
/// last *mention* — `p.b` in `"[${p.a}][${p.b}]"` — and a projection
/// produces three words copied out of the base with **no count of their
/// own**. So the pair was dropped, and with it the two string blocks its
/// fields named, before the `str.concat` chain that reads those words ever
/// ran, and the program printed zeroed bytes.
///
/// It is a middle-end fact, so this backend and LLVM printed the same wrong
/// answer and both pin it. The literal half is the control: a literal's
/// block is immortal, so the same program over `"..."` was always right and
/// says nothing about the count — which is how a shape this common survived
/// an audit whose differential used single-result functions.
#[test]
fn an_aggregate_of_counted_values_outlives_its_own_projections() {
    if !supported() {
        return;
    }
    let r = run_with(
        "aggregate-projection",
        r#"
from "core/host" import { stdout, alloc };
from "core/str" import * as str;

struct Pair { a: Str, b: Str }

fn dupTuple(s: Str): (Str, Str) { (s, s) }
fn dupStruct(s: Str): Pair { Pair { a: s, b: s } }
fn twoTuple(a: Str, b: Str): (Str, Str) { (a, b) }

export fn main(): Result<(), Str> {
  let heap = "ab".repeat(alloc, 3);
  let other = "cd".repeat(alloc, 2);

  let dup = dupTuple(heap);
  let _ = stdout.println("tuple [${dup.0}][${dup.1}]");

  let rec = dupStruct(heap);
  let _ = stdout.println("struct [${rec.a}][${rec.b}]");

  let two = twoTuple(heap, other);
  let _ = stdout.println("two [${two.0}][${two.1}]");

  let here = (heap, heap);
  let _ = stdout.println("local [${here.0}][${here.1}]");

  let lit = dupTuple("zz");
  let _ = stdout.println("literal [${lit.0}][${lit.1}]");

  let one = (heap, 1);
  let _ = stdout.println("one [${one.0}]");
  .Ok(())
}
"#,
        Some(ALLOC_PROBE),
    );
    assert_eq!(
        r.stdout,
        "tuple [ababab][ababab]\nstruct [ababab][ababab]\ntwo [ababab][cdcd]\n\
             local [ababab][ababab]\nliteral [zz][zz]\none [ababab]\n",
        "stderr: {}",
        r.stderr
    );
    assert_eq!(r.status, 0);
    let (_, live) = probed(&r.stderr);
    assert_eq!(live, 0, "the heap did not come back balanced: {}", r.stderr);
}

/// The **class**, rather than the shape the report arrived in: every
/// borrowing projection, wherever the value it produces is read.
///
/// `Field`, `TupleIndex`, `CtxGet` and `Index` all read a base without
/// taking it, and all four had the same drop placement. `xs[i]` as a
/// `match` scrutinee was the same bug as `p.a` in a template, except that
/// there it was a segmentation fault rather than a wrong answer — the arms
/// read a payload out of a list block the scrutinee's own drop had already
/// freed.
///
/// The fourth row is a back edge: an aggregate of two counted values
/// carried around a tail-recursive loop and projected on every iteration,
/// where a drop placed one instruction early is a use-after-free per
/// iteration rather than once.
#[test]
fn a_borrowing_projection_does_not_end_its_bases_lifetime() {
    if !supported() {
        return;
    }
    let r = run_with(
        "projection-class",
        r#"
from "core/host" import { stdout, alloc };
from "core/str" import * as str;
from "core/list" import * as list;

struct Held { name: Str, tag: Str }

fn hold(a: Str, b: Str): Held { Held { name: a, tag: b } }

fn spin(n: Int, p: (Str, Str)): Str {
  if (n == 0) { p.0 } else { spin(n - 1, (p.1, p.0)) }
}

export fn main(): Result<(), Str> {
  let held = hold("ab".repeat(alloc, 2), "cd".repeat(alloc, 2));
  let _ = stdout.println("field [${held.name}][${held.tag}]");

  let xs = ["ef".repeat(alloc, 2)];
  let got = match (xs[0]) { .Some(v) => v, .None => "?" };
  let _ = stdout.println("index [${got}]");

  let pair = ("gh".repeat(alloc, 2), "ij".repeat(alloc, 2));
  let picked = match (pair.0.toInt()) { .Some(_n) => "number", .None => pair.1 };
  let _ = stdout.println("match [${picked}]");

  let _ = stdout.println("loop [${spin(5, ("kl".repeat(alloc, 2), "mn".repeat(alloc, 2)))}]");
  .Ok(())
}
"#,
        Some(ALLOC_PROBE),
    );
    assert_eq!(
        r.stdout,
        "field [abab][cdcd]\nindex [efef]\nmatch [ijij]\nloop [mnmn]\n",
        "stderr: {}",
        r.stderr
    );
    assert_eq!(r.status, 0);
    let (_, live) = probed(&r.stderr);
    assert_eq!(live, 0, "the heap did not come back balanced: {}", r.stderr);
}

/// The join `middle::lower::template` builds is nobody else's to drop.
///
/// Every `Show` result and every intermediate `str.concat` in an
/// interpolation is a value `lower` invents; `middle::rc` plans over the
/// *tree* and has no `NodeId` to name one with. So until `template`
/// dropped them itself, a program that interpolated in a loop grew the
/// heap by a block an iteration — and `Prim::Template` was
/// `Answer::Unknown` in `rc`'s oracle, so the block the chain *ended*
/// holding leaked once per evaluation on top of that.
///
/// Counted rather than asserted against zero at one size: a per-iteration
/// leak is what this is about, so twenty iterations and two hundred have to
/// leave the same number of blocks live.
#[test]
fn interpolating_in_a_loop_leaks_nothing() {
    if !supported() {
        return;
    }
    let source = |n: u32| {
        format!(
            r#"
from "core/host" import {{ stdout, alloc }};
from "core/str" import * as str;

fn go(n: Int, acc: Int): Int {{
  if (n <= 0) {{ acc }} else {{
    let h = "ab".repeat(alloc, 3);
    let p = (h, h);
    let s = str.format(alloc, "[${{p.0}}][${{p.1}}]");
    let _ = stdout.println("${{n}}");
    go(n - 1, acc + s.len())
  }}
}}

export fn main(): Result<(), Str> {{
  let _ = stdout.println("total ${{go({n}, 0)}}");
  .Ok(())
}}
"#
        )
    };
    let few = run_with("template-leak-few", &source(20), Some(ALLOC_PROBE));
    let many = run_with("template-leak-many", &source(200), Some(ALLOC_PROBE));
    assert!(few.stdout.ends_with("total 320\n"), "stdout was: {:?}", few.stdout);
    assert!(many.stdout.ends_with("total 3200\n"), "stdout was: {:?}", many.stdout);
    let (_, live_few) = probed(&few.stderr);
    let (_, live_many) = probed(&many.stderr);
    assert_eq!(
        live_few, live_many,
        "twenty interpolations left {live_few} blocks live and two hundred left \
             {live_many}: the join leaks per iteration"
    );
    assert_eq!(live_many, 0, "the heap did not come back balanced: {}", many.stderr);
}
