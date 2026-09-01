//! The LLVM backend, driven the way a build drives it.
//!
//! Every test here goes through the **whole real pipeline** — parse, check,
//! monomorphize, `middle::run`, `middle::native`, `middle::lower`, then
//! `backend::llvm` — links the resulting object with `cc` against the embedded
//! runtime archive, runs the binary, and asserts on what it printed. Nothing is
//! stubbed and no intermediate form is compared to a golden file, because the
//! claim being tested is that a Buri program *executes* natively and produces
//! the answers the JavaScript backend produces.
//!
//! Three tests are not end-to-end, deliberately:
//!
//!  * [`the_attribute_discipline_reaches_the_optimized_ir`] reads the optimized
//!    LLVM IR text and asserts on it, FileCheck-style. An attribute that is
//!    emitted and then dropped by `default<O2>` is an attribute the object does
//!    not have, so the assertion is made *after* the pipeline.
//!  * [`a_hot_function_has_no_allocas`] is CODEGEN-LLVM.md §2.2's claim as a
//!    test rather than as a sentence.
//!  * [`an_unimplemented_intrinsic_is_reported_before_llvm_runs`] is the
//!    `missing_intrinsics` hook, which exists to answer before a second is
//!    spent in LLVM.
//!
//! # What is skipped, and when
//!
//! `main.rs` declares this module behind `backend-llvm`, which is off by
//! default and so this module is usually not compiled at all. With the
//! feature on, LLVM is linked into this binary, so "is LLVM installed" is not a
//! question — but "can this host link and run what we emit" still is, and
//! [`can_execute`] answers it: a runtime archive must have been built
//! (`runtime_native::AVAILABLE`), `cc` must be on the path, and the target
//! machine for this host's triple must be constructible. A host that fails any
//! of those skips with a message rather than failing.
use buri::build::buildfile::{Arch, Platform};
use buri::compiler::backend::runtime_native::{ARCHIVE, ARCHIVE_NAME, AVAILABLE};
use buri::compiler::backend::{llvm, Options, Profile, Target};
use buri::compiler::driver;
use buri::compiler::middle;
use buri::compiler::modules::Role;
use buri::compiler::semantics::types::{FuncIdx, Tables};
use buri::diagnostics::{Diagnostics, SourceMap};
use std::path::{Path, PathBuf};
use std::process::Command;

// ---------------------------------------------------------------------------
// The pipeline
// ---------------------------------------------------------------------------

/// One compiled program: the lowered IR, the tables it was lowered against,
/// and where its entry point is.
struct Lowered {
    ir: middle::ir::Program,
    tables: Tables,
    entry: FuncIdx,
}

/// Source text through the whole middle end.
///
/// This is `actions::prepare`'s path, spelled out: `middle::run` and then
/// `middle::native`. `Backend::emit` takes the program by shared reference and
/// `middle::native` needs `&mut`, so the backend cannot run it — the build
/// composes the two, and so does this.
fn lower(source: &str) -> Lowered {
    let mut map = SourceMap::new();
    let mut cache = buri::parsing::parser::Cache::new();
    let analysis =
        driver::analyze_snippet_in(None, &mut map, &mut cache, "main.buri", source, Role::Entry);
    assert!(!analysis.diagnostics.has_errors(), "{}", render(&analysis.diagnostics, &map));
    let entry = analysis.checked.entry.expect("the program exports no `main`");
    let module_paths: Vec<String> =
        analysis.loaded.modules.iter().map(|m| m.path.clone()).collect();

    let mut diagnostics = Diagnostics::new();
    let mut program = middle::monomorphize::run(
        &analysis.checked,
        module_paths,
        &mut diagnostics,
        middle::monomorphize::Roots::Main(entry),
    );
    assert!(!diagnostics.has_errors(), "{}", render(&diagnostics, &map));

    middle::run(&mut program, &middle::Options::default());
    middle::native(&mut program);
    let ir = middle::lower::run(&program, &analysis.checked.tables);

    // The IR the backend is handed is verified first, so a backend failure is
    // never a lowering failure wearing a backend's clothes.
    let errs = middle::ir::verify(&ir);
    assert!(errs.is_empty(), "the lowered IR does not verify: {errs:#?}");

    // `checked.entry` is the front end's `FnId`; the *monomorphized* index is
    // what the backend needs, and `Program::roots` is where monomorphization
    // records it — the same place `js/generate.rs` reads it from.
    let middle::monomorphize::ProgramRoots::Main(entry) = program.roots else {
        panic!("the program has no `main` root")
    };
    Lowered { ir, tables: analysis.checked.tables, entry }
}

/// Diagnostics are not `Debug`, and a failed emission should print what it
/// said rather than "called `unwrap` on an `Err`".
fn expect<T>(what: Result<T, Diagnostics>) -> T {
    match what {
        Ok(value) => value,
        Err(diagnostics) => panic!(
            "the LLVM backend refused the program: {}",
            diagnostics.items.iter().map(|d| d.message.clone()).collect::<Vec<_>>().join("; ")
        ),
    }
}

fn render(diagnostics: &Diagnostics, map: &SourceMap) -> String {
    diagnostics.items.iter().map(|d| map.render(d, false)).collect::<Vec<_>>().join("\n")
}

fn host_target() -> Target {
    Target {
        platform: if cfg!(target_os = "macos") { Platform::Macos } else { Platform::Linux },
        arch: Some(if cfg!(target_arch = "aarch64") { Arch::Arm64 } else { Arch::X86_64 }),
    }
}

fn options(profile: Profile) -> Options<'static> {
    Options { profile, target: host_target(), unit_prefix: "" }
}

// ---------------------------------------------------------------------------
// Linking and running
// ---------------------------------------------------------------------------

/// A per-run directory under `CARGO_TARGET_TMPDIR`, so nothing is written
/// inside a checked-in tree, and named by the process id so that two
/// `cargo test` runs in two shells do not share it — the same rule
/// `tests/native/runtime.rs` follows, for the reason written there.
fn workspace() -> PathBuf {
    crate::sweep::once();
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("native-llvm-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// The runtime archive, written once and reused.
fn archive() -> &'static Path {
    static WRITTEN: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    WRITTEN.get_or_init(|| {
        let path = workspace().join(ARCHIVE_NAME);
        std::fs::write(&path, ARCHIVE).unwrap();
        path
    })
}

/// Whether this host can link and run what the backend emits.
///
/// `cc` is not a new requirement: the link step already drives the platform C
/// compiler (CODEGEN-STENCIL.md §12), so a machine that can build a Buri
/// artifact can run these.
fn can_execute() -> Option<&'static str> {
    if !AVAILABLE {
        return Some("no runtime archive was built for this host");
    }
    if Command::new(cc()).arg("--version").output().is_err() {
        return Some("`cc` is not on the path");
    }
    let target = host_target();
    match llvm::target::triple(target).and_then(|t| llvm::target::machine(&t, Profile::Release)) {
        Ok(_) => None,
        Err(_) => Some("LLVM has no backend for this host's triple"),
    }
}

fn cc() -> String {
    std::env::var("CC").unwrap_or_else(|_| String::from("cc"))
}

/// Compile, link, run, and answer `(stdout, stderr, exit status)`.
///
/// The link is a plain `cc` over the emitted objects and the runtime archive,
/// which is exactly the command `native/runtime.rs`'s module comment writes
/// out. `build/link.rs` owns the real linker selection; this is the same
/// command line spelled in the test that needs it.
fn build_and_run(name: &str, source: &str) -> (String, String, Option<i32>) {
    build_and_run_with(name, source, None)
}

/// A C shim linked beside the program, whose destructor prints the runtime's
/// live-block count once `main` has returned.
///
/// `buri_rt_live_blocks` is not reachable from Buri — it has no intrinsic key,
/// and it should not have one — so a leak has to be observed from outside the
/// program. A destructor rather than a wrapper around `main`: the emitted entry
/// point is the one `cli/runtime/lib.rs` §6 describes and calls `buri_rt_flush`
/// on the way out, and replacing it would be testing a different program.
const LIVE_PROBE: &str = r#"
#include <stdio.h>
extern unsigned long long buri_rt_live_blocks(void);
__attribute__((destructor)) static void buri_probe(void) {
  fprintf(stderr, "live=%llu\n", buri_rt_live_blocks());
}
"#;

/// The live-block count a [`LIVE_PROBE`]-linked run reported.
fn live_blocks(stderr: &str) -> u64 {
    let line = stderr
        .lines()
        .find_map(|l| l.strip_prefix("live="))
        .unwrap_or_else(|| panic!("the probe printed nothing: {stderr:?}"));
    line.trim().parse().unwrap()
}

fn build_and_run_with(
    name: &str,
    source: &str,
    probe: Option<&str>,
) -> (String, String, Option<i32>) {
    build_and_run_at(name, source, probe, Profile::Release)
}

/// The same, at a chosen profile — which is a chosen *pipeline*:
/// `Profile::Release` is `default<O2>` and `Profile::Debug` is `default<O0>`
/// (`llvm/target.rs`). Running one program through both is how a claim about
/// an attribute becomes a claim about a program: an optimizer that exploited a
/// false `memory(...)` would make the two disagree.
fn build_and_run_at(
    name: &str,
    source: &str,
    probe: Option<&str>,
    profile: Profile,
) -> (String, String, Option<i32>) {
    let binary = build_at(name, source, probe, profile);
    let run = Command::new(&binary).output().unwrap();
    (
        String::from_utf8_lossy(&run.stdout).to_string(),
        String::from_utf8_lossy(&run.stderr).to_string(),
        run.status.code(),
    )
}

/// [`build_and_run_at`]'s first half, for the one row that has to have the
/// binary in hand before it runs: a server has to be running *while* something
/// else talks to it, so the run cannot be folded into the build.
fn build_at(name: &str, source: &str, probe: Option<&str>, profile: Profile) -> PathBuf {
    let lowered = lower(source);
    let opts = options(profile);
    let units =
        expect(llvm::emit_lowered(&lowered.ir, &lowered.tables, &opts, Some(lowered.entry)));
    assert!(!units.is_empty(), "the backend emitted no codegen unit");

    let dir = workspace().join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
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
        let built = Command::new(cc()).arg("-c").arg(&c).arg("-o").arg(&o).output().unwrap();
        assert!(
            built.status.success(),
            "the probe did not compile:\n{}",
            String::from_utf8_lossy(&built.stderr)
        );
        objects.push(o);
    }

    let binary = dir.join("program");
    let mut link = Command::new(cc());
    link.arg("-o").arg(&binary);
    for object in &objects {
        link.arg(object);
    }
    link.arg(archive());
    if cfg!(target_os = "linux") {
        // Harmless where glibc has folded them in, and required where it has
        // not: `std` in the runtime archive reaches for all three.
        link.args(["-lpthread", "-ldl", "-lm"]);
    }
    let linked = link.output().unwrap();
    assert!(
        linked.status.success(),
        "linking failed:\n{}",
        String::from_utf8_lossy(&linked.stderr)
    );
    binary
}

/// The prelude every program below shares: the two capabilities a program that
/// prints needs, and nothing else — so the context is a record of two empty
/// implementations and is therefore zero-sized (VALUE-MODEL.md §8).
const PRELUDE: &str = r#"
from "core/effect" import { Alloc, Stdout };
from "core/host" import * as host;
from "core/io" import * as io;
"#;

fn program(body: &str) -> String {
    format!("{PRELUDE}\n{body}")
}

macro_rules! skip_unless_executable {
    () => {
        if let Some(why) = can_execute() {
            crate::ci::skipped("llvm", &why);
            return;
        }
    };
}

/// A probe that **enters Buri code from a second carrier** through the door
/// `carrier.rs` names, and says so.
///
/// The counterpart of `stencil.rs`'s `CARRIER_PROBE`, minus its sentinel: this
/// backend has no Buri data stack to keep two carriers off each other's frames
/// — a frame here is the machine's, and a thread's machine stack is the OS's.
/// So what is left to check is the half that *is* shared, and it is the half
/// the two slices exist to make identical: that a `void(void *, void *)`
/// declared once in C reaches a root emitted by either backend.
///
/// A **constructor** rather than a wrapper around `main`, for [`ALLOC_PROBE`]'s
/// reason. It runs before `main`, and `main` then runs the same root on the
/// process's own thread — which is what makes the expected output two lines.
const CARRIER_PROBE: &str = r#"
#include <pthread.h>
#include <stdio.h>

extern void buri$carrier$main(void *state, void *out);

static unsigned char answer[4096];

static void *carrier(void *unused) {
  (void)unused;
  buri$carrier$main(0, answer);
  return 0;
}

__attribute__((constructor)) static void buri_carrier_probe(void) {
  /* 64 MiB, which is `asm::STACK_USABLE`: on this backend a Buri frame *is* a
     machine frame, so the depth a carrier can recurse to is the depth its
     thread stack allows, and the default one is far under what the stencil
     backend's own block gives. See the test's header. */
  pthread_attr_t a;
  pthread_attr_init(&a);
  pthread_attr_setstacksize(&a, 64u * 1024u * 1024u);
  pthread_t t;
  if (pthread_create(&t, &a, carrier, 0) != 0) { fprintf(stderr, "carrier: no thread\n"); return; }
  pthread_join(t, 0);
  pthread_attr_destroy(&a);
  fprintf(stderr, "carrier: entered\n");
}
"#;

/// **A second carrier enters Buri code through the door, at both profiles.**
///
/// Slice B8. The door is a `ccc` wrapper in front of the `fastcc` body, so the
/// two things it can get wrong are a convention mismatch — which shows up as a
/// crash or a wrong answer rather than as a diagnostic — and being optimized
/// away, which `default<O2>` would do to a function with external linkage that
/// nothing in the module calls if the linkage were wrong.
///
/// Ten thousand non-tail frames, the same depth `stencil.rs` recurses on its
/// own stack, so the two halves of the pair are asking one question of one
/// program — but **not** on the same stack, and the probe has to say so.
///
/// A Buri frame here *is* a machine frame, so a carrier's depth is its
/// thread's stack and nothing else. The default for a non-main thread is well
/// under a megabyte on both platforms, and ten thousand of these frames do not
/// fit in it: the first run of this test was killed by a signal. The probe
/// therefore asks for 64 MiB, which is `asm::STACK_USABLE` — the number the
/// *other* backend gives a carrier — so that the pair is comparable.
///
/// That is a real asymmetry rather than a detail of this test, and it is
/// written down here because this is where it was found: `cli/runtime/rt.rs`
/// sizes a pool carrier's machine stack at 512 KiB, which is right for a
/// backend whose frames are on a separate 64 MiB block and is a much shallower
/// recursion limit for the one whose frames are not. Slice B9 replaces the
/// carrier thread with a stack switch and is where the two become one number.
///
/// **Both pipelines**, because that is what `release_and_debug_agree` asserts
/// for the suite and this file asserts per case: `default<O2>` and
/// `default<O0>` have to print the same thing and exit the same way, and an
/// entry point one of them deleted would not.
#[test]
fn a_second_carrier_enters_through_the_door() {
    skip_unless_executable!();
    // `f` prints at the base case, and that is not decoration. A *pure*
    // self-recursive function is `memory(none) willreturn` here, and
    // `default<O2>` speculates such a call to flatten the branch — which turns
    // `if (i <= 0) { 0 } else { 1 + f(i - 1) }` into an unconditional
    // recursion that never returns. That is a pre-existing hazard of this
    // backend's attributes and not of the door (the same program crashes with
    // no probe linked at all, at `-O2`, and runs at `-O0`); it is recorded in
    // this slice's report. One side effect at the bottom takes `f` out of
    // `memory(none)` and leaves the depth this test is about intact.
    let source = r#"
from "core/host" import { stdout };
from "core/io" import * as io;
fn f(i: Int): Int { if (i <= 0) { let _ = io.println(stdout, "bottom").ignore(); 0 } else { 1 + f(i - 1) } }
export fn main(): Result<(), Str> {
  let _ = io.println(stdout, "depth ${f(10000)}").ignore();
  .Ok(())
}
"#
    .to_string();
    let fast = build_and_run_at("carrier-door-o2", &source, Some(CARRIER_PROBE), Profile::Release);
    let plain = build_and_run_at("carrier-door-o0", &source, Some(CARRIER_PROBE), Profile::Debug);

    for (what, ran) in [("default<O2>", &fast), ("default<O0>", &plain)] {
        assert_eq!(ran.2, Some(0), "{what} exited {:?}: {}", ran.2, ran.1);
        assert!(
            ran.1.contains("carrier: entered"),
            "{what}: the probe never came back from the door: {:?}",
            ran.1
        );
        assert_eq!(
            ran.0, "bottom\ndepth 10000\nbottom\ndepth 10000\n",
            "{what}: the root ran {} time(s), not twice",
            ran.0.lines().count() / 2
        );
    }
    assert_eq!(fast.0, plain.0, "the two pipelines printed different things");
    assert_eq!(fast.2, plain.2, "the two pipelines exited differently");
}

/// **The door is emitted `ccc`, external, in front of a `fastcc` body.**
///
/// Read out of the IR before `default<O2>`, which is where a claim about what
/// this backend *emits* can be made ([`emitted_ir`]). Three things, and each
/// one is what a caller outside the artifact depends on:
///
///  * the name is `carrier.rs`'s, so a C declaration finds it;
///  * it takes two pointers and answers nothing, which
///    `the_two_carrier_doors_have_one_signature` pins byte for byte against
///    the other backend;
///  * the call inside it is `fastcc`, because the body is — the door is the
///    one place the two conventions meet, and a `ccc` call to a `fastcc`
///    definition is a miscompile no linker sees.
#[test]
fn the_carrier_door_is_a_ccc_wrapper_over_a_fastcc_body() {
    let source = program(
        r#"
export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout };
  let _ = io.println(ctx, "hi").ignore();
  .Ok(())
}
"#,
    );
    let ir = emitted_ir(&source);
    let name = buri::compiler::backend::carrier::MAIN_ENTRY;
    let line = ir
        .lines()
        .find(|l| l.starts_with("define") && l.contains(name))
        .unwrap_or_else(|| panic!("no definition of {name} in:\n{ir}"));
    assert!(line.contains("void"), "the door answers something: {line}");
    assert_eq!(line.matches("ptr ").count(), 2, "the door is not two pointers: {line}");
    assert!(!line.contains("fastcc"), "the door is not at the platform ABI: {line}");
    assert!(!line.contains("internal"), "the door is not externally visible: {line}");
    // The body it wraps is `fastcc`, and the call is where the two meet.
    let body = ir
        .lines()
        .skip_while(|l| !(l.starts_with("define") && l.contains(name)))
        .take_while(|l| *l != "}")
        .find(|l| l.contains("call") && l.contains("fastcc"));
    assert!(body.is_some(), "the door does not call its body at `fastcc`:\n{ir}");
}

// ---------------------------------------------------------------------------
// End to end, from hello world upwards
// ---------------------------------------------------------------------------

/// The first thing that has to work: a program that prints a literal and exits
/// zero. It exercises the whole spine — the emitted `main`, `buri_rt_argv_init`,
/// a zero-sized context, a literal `Str` with a null `base`, a `buri_rt_*` call
/// with a flattened three-word argument, `buri_rt_flush`, and the
/// `Result<(), Str>` contract on the way out.
#[test]
fn hello_world_compiles_links_and_runs() {
    skip_unless_executable!();
    let (out, err, code) = build_and_run(
        "hello",
        &program(
            r#"
export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout };
  let _ = io.println(ctx, "hello, world").ignore();
  .Ok(())
}
"#,
        ),
    );
    assert_eq!(out, "hello, world\n", "stderr was: {err}");
    assert_eq!(code, Some(0));
}

/// This backend's half of the archive decision: its objects reference the
/// runtime too, so `build/link.rs` names `libburi_rt.a` on the command line for
/// a release build exactly as it does for a debug one.
///
/// The stencil suite carries the measurement
/// (`stencil.rs::hello_world_still_links_the_runtime_archive`); what this adds
/// is that the answer is not a property of one code generator. A backend whose
/// entry point stopped calling `buri_rt_argv_init` would flip here first.
#[test]
fn hello_world_references_the_runtime_archive() {
    if !buri::compiler::backend::runtime_native::AVAILABLE {
        crate::ci::skipped(
            "llvm",
            "this toolchain carries no runtime archive, so there is nothing for the entry point \
             to reference",
        );
        return;
    }
    let lowered = lower(&program(
        r#"
export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout };
  let _ = io.println(ctx, "hello, world").ignore();
  .Ok(())
}
"#,
    ));
    let opts = options(Profile::Release);
    let units =
        expect(llvm::emit_lowered(&lowered.ir, &lowered.tables, &opts, Some(lowered.entry)));
    assert_eq!(
        buri::build::link::runtime_archive_for(&units),
        buri::build::link::RuntimeArchive::Linked,
        "an LLVM artifact was judged not to reference the runtime"
    );
}

/// Arithmetic, comparison and a branch, across a function call the inliner
/// leaves alone. `divide` is the interesting one: SPEC 6.2 says division by
/// zero aborts, so the emitted code carries a zero test and a cold call to
/// `buri_rt_abort_div_zero` that this program does not take.
#[test]
fn arithmetic_and_branches_run() {
    skip_unless_executable!();
    let (out, err, code) = build_and_run(
        "arith",
        &program(
            r#"
fn sum(a: Int, b: Int): Int { a + b }
fn quotient(a: Int, b: Int): Int { a / b }
fn remainder(a: Int, b: Int): Int { a % b }

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout };
  let n = sum(40, 2);
  let _ = if (n == 42) { io.println(ctx, "sum ok").ignore() } else { io.println(ctx, "sum bad").ignore() };
  let _ = if (quotient(n, 5) == 8) { io.println(ctx, "div ok").ignore() } else { io.println(ctx, "div bad").ignore() };
  let _ = if (remainder(n, 5) == 2) { io.println(ctx, "rem ok").ignore() } else { io.println(ctx, "rem bad").ignore() };
  let _ = if (0 - n < 0) { io.println(ctx, "neg ok").ignore() } else { io.println(ctx, "neg bad").ignore() };
  .Ok(())
}
"#,
        ),
    );
    assert_eq!(out, "sum ok\ndiv ok\nrem ok\nneg ok\n", "stderr was: {err}");
    assert_eq!(code, Some(0));
}

/// A division by zero really does abort, with the runtime's message and a
/// non-zero status. This is the cold path the previous test only compiles.
#[test]
fn division_by_zero_aborts_with_the_runtimes_message() {
    skip_unless_executable!();
    let (out, err, code) = build_and_run(
        "divzero",
        &program(
            r#"
fn quotient(a: Int, b: Int): Int { a / b }

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout };
  let _ = io.println(ctx, "before").ignore();
  let _ = if (quotient(1, 0) == 0) { io.println(ctx, "no").ignore() } else { io.println(ctx, "no").ignore() };
  .Ok(())
}
"#,
        ),
    );
    // The abort flushes what was printed before it (`cli/runtime/lib.rs` §6),
    // which is the property `an_abort_flushes_what_was_printed` pins on the
    // runtime side and this pins through generated code.
    assert_eq!(out, "before\n");
    assert!(err.contains("divide by zero") || err.contains("zero"), "stderr was: {err:?}");
    assert_ne!(code, Some(0));
}

/// A struct built, passed, projected and rebuilt. Every value here is in
/// registers: the struct never reaches memory, which is what
/// [`a_hot_function_has_no_allocas`] asserts on the IR.
#[test]
fn structs_run() {
    skip_unless_executable!();
    let (out, err, code) = build_and_run(
        "structs",
        &program(
            r#"
struct Point { x: Int, y: Int }

fn shifted(p: Point, d: Int): Point { Point { x: p.x + d, y: p.y + d } }
fn magnitude(p: Point): Int { p.x * p.x + p.y * p.y }

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout };
  let p = shifted(Point { x: 1, y: 2 }, 3);
  let _ = if (p.x == 4) { io.println(ctx, "x ok").ignore() } else { io.println(ctx, "x bad").ignore() };
  let _ = if (magnitude(p) == 41) { io.println(ctx, "mag ok").ignore() } else { io.println(ctx, "mag bad").ignore() };
  .Ok(())
}
"#,
        ),
    );
    assert_eq!(out, "x ok\nmag ok\n", "stderr was: {err}");
    assert_eq!(code, Some(0));
}

/// An enum with payloads: `MakeEnum` packs a variant's fields into the payload
/// blob, `GetTag` reads the discriminant, the decision tree becomes one
/// `switch`, and `GetPayload` reads a field back out of the blob under a tag
/// the switch has just established.
#[test]
fn enums_and_matches_run() {
    skip_unless_executable!();
    let (out, err, code) = build_and_run(
        "enums",
        &program(
            r#"
enum Shape { Circle(Int), Rect(Int, Int), Empty }

fn area(s: Shape): Int {
  match (s) {
    .Circle(r) => 3 * r * r,
    .Rect(w, h) => w * h,
    .Empty => 0,
  }
}

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout };
  let total = area(Shape.Circle(2)) + area(Shape.Rect(3, 4)) + area(Shape.Empty);
  let _ = if (total == 24) { io.println(ctx, "area ok").ignore() } else { io.println(ctx, "area bad").ignore() };
  let _ = match (Shape.Rect(2, 5)) {
    .Circle(_) => io.println(ctx, "circle").ignore(),
    .Rect(w, h) => if (w * h == 10) { io.println(ctx, "rect ok").ignore() } else { io.println(ctx, "rect bad").ignore() },
    .Empty => io.println(ctx, "empty").ignore(),
  };
  .Ok(())
}
"#,
        ),
    );
    assert_eq!(out, "area ok\nrect ok\n", "stderr was: {err}");
    assert_eq!(code, Some(0));
}

/// `Option<Str>` takes VALUE-MODEL.md §6's second niche: there is no tag, the
/// value *is* the `Str`, and `.None` is its `ptr` set to null. So this exercises
/// the one enum shape whose pointer slots stay pointer-typed.
#[test]
fn the_option_niche_runs() {
    skip_unless_executable!();
    let (out, err, code) = build_and_run(
        "option",
        &program(
            r#"
fn pick(yes: Bool): Option<Str> { if (yes) { .Some("some") } else { .None } }

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout };
  let _ = match (pick(true)) { .Some(s) => io.println(ctx, s).ignore(), .None => io.println(ctx, "none").ignore() };
  let _ = match (pick(false)) { .Some(s) => io.println(ctx, s).ignore(), .None => io.println(ctx, "none").ignore() };
  .Ok(())
}
"#,
        ),
    );
    assert_eq!(out, "some\nnone\n", "stderr was: {err}");
    assert_eq!(code, Some(0));
}

/// A self-recursive tail call, which `middle::tail_calls` has already turned
/// into a loop — so what reaches LLVM is a CFG loop with phis and no mutable
/// slot anywhere (CODEGEN-LLVM.md §2.4). Ten thousand iterations, so a
/// backend that had emitted a real recursion would overflow the stack rather
/// than answer.
#[test]
fn a_tail_recursive_loop_runs_without_growing_the_stack() {
    skip_unless_executable!();
    let (out, err, code) = build_and_run(
        "loop",
        &program(
            r#"
fn total(n: Int, acc: Int): Int {
  if (n <= 0) { acc } else { total(n - 1, acc + n) }
}

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout };
  let _ = if (total(10000, 0) == 50005000) { io.println(ctx, "loop ok").ignore() } else { io.println(ctx, "loop bad").ignore() };
  .Ok(())
}
"#,
        ),
    );
    assert_eq!(out, "loop ok\n", "stderr was: {err}");
    assert_eq!(code, Some(0));
}

/// A **non-tail** recursion: the recursive call's result is added to, so the
/// frame has to survive the call and `middle::tail_calls` has nothing to
/// rewrite. Ten thousand real frames, at both profiles.
///
/// **This is the shape `default<O2>` miscompiled.** `attrs::decorate` claimed
/// `willreturn` — and, for an all-scalar signature, `memory(none)` and
/// `speculatable` — of every function the effect system called pure and
/// non-aborting, recursive or not. A call LLVM believes returns and touches
/// nothing may be **hoisted above the branch that guards it**, and that is
/// what happened: the emitted `main_buri$depth` became an unconditional `bl
/// main_buri$depth` with the base-case test folded into a `csinc` *after* it,
/// so the program recursed until the machine stack ran out. `Profile::Debug`
/// printed the right answer all along, which is the signature of an attribute
/// that is not true rather than of a body that is wrong.
///
/// The bound is a **runtime** value: `repeat` and `len` are runtime calls this
/// compilation cannot see through, so no amount of constant folding can
/// pre-compute the recursion away and the base case is reached only by
/// actually recursing. Both profiles run here, which is
/// `language::conformance::release_and_debug_agree`'s assertion made for the
/// one shape that used to break it.
#[test]
fn a_pure_non_tail_recursion_returns_at_both_profiles() {
    skip_unless_executable!();
    let source = program(
        r#"
fn depth(i: Int): Int { if (i <= 0) { 0 } else { 1 + depth(i - 1) } }

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout };
  let n = "ab".repeat(ctx, 5000).len();
  let _ = io.println(ctx, "depth ${depth(n)}").ignore();
  .Ok(())
}
"#,
    );
    for profile in [Profile::Release, Profile::Debug] {
        let name = format!("nontail-{}", profile.name());
        let (out, err, code) = build_and_run_at(&name, &source, None, profile);
        assert_eq!(out, "depth 10000\n", "at {}, stderr was: {err}", profile.name());
        assert_eq!(code, Some(0), "at {}, stderr was: {err}", profile.name());
    }
}

/// Every integer width, including `I128` — whose division and remainder are a
/// call to `buri_rt_i128_divmod` with the operands split into pairs of `u64`
/// and the results read back through out-pointers (`cli/runtime/lib.rs`).
#[test]
fn every_integer_width_runs_including_i128() {
    skip_unless_executable!();
    let (out, err, code) = build_and_run(
        "widths",
        &program(
            r#"
fn wide(a: I128, b: I128): I128 { a / b }
fn narrow(a: I32, b: I32): I32 { a * b }
fn tiny(a: U8, b: U8): U8 { a + b }

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout };
  let big: I128 = 1000000000000000000;
  let _ = if (wide(big, 1000) == 1000000000000000) { io.println(ctx, "i128 ok").ignore() } else { io.println(ctx, "i128 bad").ignore() };
  let _ = if (narrow(1000, 1000) == 1000000) { io.println(ctx, "i32 ok").ignore() } else { io.println(ctx, "i32 bad").ignore() };
  let _ = if (tiny(200, 55) == 255) { io.println(ctx, "u8 ok").ignore() } else { io.println(ctx, "u8 bad").ignore() };
  .Ok(())
}
"#,
        ),
    );
    assert_eq!(out, "i128 ok\ni32 ok\nu8 ok\n", "stderr was: {err}");
    assert_eq!(code, Some(0));
}

/// `F32` and `F64`. `F32` is a distinct register shape rather than a rounded
/// `F64`, so an `F32` multiply that a double would get right and a single
/// would not is the thing worth asserting.
#[test]
fn both_float_widths_run() {
    skip_unless_executable!();
    let (out, err, code) = build_and_run(
        "floats",
        &program(
            r#"
fn scale32(x: F32, by: F32): F32 { x * by }
fn scale64(x: Float, by: Float): Float { x * by }

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout };
  let a: F32 = 0.5;
  let b: F32 = 4.0;
  let _ = if (scale32(a, b) == 2.0) { io.println(ctx, "f32 ok").ignore() } else { io.println(ctx, "f32 bad").ignore() };
  let _ = if (scale64(0.25, 8.0) == 2.0) { io.println(ctx, "f64 ok").ignore() } else { io.println(ctx, "f64 bad").ignore() };
  // `+` on floats is an `ir::BinOp` like any other; `/` and `<` on a `Float`
  // are `num.F64.div` and `num.F64.compare`, which are stdlib intrinsics the
  // native runtime has no body for yet and which `missing_intrinsics` reports.
  let _ = if (scale64(1.5, 2.0) + 1.0 == 4.0) { io.println(ctx, "add ok").ignore() } else { io.println(ctx, "add bad").ignore() };
  .Ok(())
}
"#,
        ),
    );
    assert_eq!(out, "f32 ok\nf64 ok\nadd ok\n", "stderr was: {err}");
    assert_eq!(code, Some(0));
}

/// `.Err(msg)` prints `msg` to standard error and exits 1, which is byte for
/// byte what the JavaScript backend does (`js/generate.rs:300-310`): a
/// program's exit status must not depend on which backend built it.
#[test]
fn an_err_result_exits_one_and_prints_to_stderr() {
    skip_unless_executable!();
    let (out, err, code) = build_and_run(
        "err",
        &program(
            r#"
export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout };
  let _ = io.println(ctx, "working").ignore();
  .Err("it went wrong")
}
"#,
        ),
    );
    assert_eq!(out, "working\n");
    assert_eq!(err, "it went wrong\n");
    assert_eq!(code, Some(1));
}

/// Every value in a program at once, so that a shape that only breaks in
/// combination — a struct holding an enum holding a `Str`, matched inside a
/// loop — is covered by something.
#[test]
fn a_larger_program_runs() {
    skip_unless_executable!();
    let (out, err, code) = build_and_run(
        "larger",
        &program(
            r#"
enum Token { Number(Int), Word(Str), End }
struct State { seen: Int, total: Int }

fn step(s: State, t: Token): State {
  match (t) {
    .Number(n) => State { seen: s.seen + 1, total: s.total + n },
    .Word(_) => State { seen: s.seen + 1, total: s.total },
    .End => s,
  }
}

fn label(t: Token): Str {
  match (t) { .Number(_) => "number", .Word(w) => w, .End => "end" }
}

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout };
  let a = step(State { seen: 0, total: 0 }, Token.Number(10));
  let b = step(a, Token.Word("hello"));
  let c = step(b, Token.Number(32));
  let d = step(c, Token.End);
  let _ = if (d.seen == 3) { io.println(ctx, "seen ok").ignore() } else { io.println(ctx, "seen bad").ignore() };
  let _ = if (d.total == 42) { io.println(ctx, "total ok").ignore() } else { io.println(ctx, "total bad").ignore() };
  let _ = io.println(ctx, label(Token.Word("word"))).ignore();
  let _ = io.println(ctx, label(Token.Number(1))).ignore();
  let _ = io.println(ctx, label(Token.End)).ignore();
  .Ok(())
}
"#,
        ),
    );
    assert_eq!(out, "seen ok\ntotal ok\nword\nnumber\nend\n", "stderr was: {err}");
    assert_eq!(code, Some(0));
}

// ---------------------------------------------------------------------------
// The IR itself
// ---------------------------------------------------------------------------

fn optimized_ir(source: &str) -> String {
    ir_at(source, Profile::Release)
}

/// The IR *before* `default<O2>`, which is the only place a claim about what
/// this backend *emits* can be made. At O2 LLVM adds its own facts — it
/// infers `nuw` on a shift it proved non-negative, and it closed-form-solves
/// a loop out of existence — and an assertion made after that is an assertion
/// about LLVM rather than about the emitter.
fn emitted_ir(source: &str) -> String {
    ir_at(source, Profile::Debug)
}

/// Set `BURI_LLVM_DUMP=1` to see the IR every assertion below is made
/// against. A backend test that fails on an attribute is a test whose next
/// question is always "what did it emit", and answering it should not need an
/// edit.
fn ir_at(source: &str, profile: Profile) -> String {
    let text = build_ir(source, profile);
    if std::env::var("BURI_LLVM_DUMP").is_ok() {
        println!("---- {} ----\n{text}", profile.name());
    }
    text
}

fn build_ir(source: &str, profile: Profile) -> String {
    let lowered = lower(source);
    let opts = options(profile);
    let unit = lowered
        .ir
        .funcs
        .get(lowered.entry.index())
        .map(|f| f.unit)
        .expect("the entry point is one of the functions");
    llvm::emit_ir_text(&lowered.ir, &lowered.tables, &opts, Some(lowered.entry), unit)
        .unwrap_or_else(|d| {
            panic!(
                "{}",
                d.items.iter().map(|x| x.message.clone()).collect::<Vec<_>>().join("; ")
            )
        })
}

/// Takes the body of one function out of an IR dump, so an assertion is about
/// that function rather than about the module.
fn function_body<'a>(ir: &'a str, name: &str) -> &'a str {
    let at = ir
        .find(name)
        .unwrap_or_else(|| panic!("no function matching `{name}` in:\n{ir}"));
    let rest = &ir[at..];
    let end = rest.find("\n}").map(|e| e + 2).unwrap_or(rest.len());
    &rest[..end]
}

/// CODEGEN-LLVM.md §3, on the IR that becomes the object.
///
/// An attribute that is emitted and then dropped by `default<O2>` is an
/// attribute the object does not have, so every assertion here is made *after*
/// the pipeline has run.
#[test]
fn the_attribute_discipline_reaches_the_optimized_ir() {
    skip_unless_executable!();
    let ir = optimized_ir(&program(
        r#"
fn magnitude(x: Int, y: Int): Int { x * x + y * y }

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout };
  let _ = if (magnitude(3, 4) == 25) { io.println(ctx, "ok").ignore() } else { io.println(ctx, "no").ignore() };
  .Ok(())
}
"#,
    ));

    // `nounwind` on every function, on every backend: the language has no
    // `throw`, no unwinding `panic` and no `catch` (SPEC 6.10). Checked on the
    // emitted `main`, which is the one function that is always there.
    assert!(ir.contains("nounwind"), "no `nounwind` anywhere in:\n{ir}");

    // The runtime declarations keep the C convention and the Buri functions
    // keep `fastcc` — a mismatch between a function and its call sites is a
    // miscompile LLVM will not diagnose, so both halves are asserted.
    assert!(ir.contains("declare void @buri_rt_flush"), "no flush declaration in:\n{ir}");
    assert!(ir.contains("buri_rt_argv_init"), "no argv_init in:\n{ir}");

    // `cli/runtime/lib.rs` §6's optional call, which **this** backend makes and
    // the frame-threaded one must not. It survives `default<O2>` because the
    // callee is an opaque declaration; a pipeline that learned to drop it would
    // be a `Tasks.parallel` that silently stopped overlapping its waits, which
    // is why the assertion is here rather than on the unoptimized module.
    assert!(
        ir.contains("buri_rt_frames_are_per_carrier"),
        "no frames_are_per_carrier in:\n{ir}"
    );

    // The emitted `main` is `ccc`, because the platform starts it, while every
    // Buri function is `fastcc`. A mismatch between a function and its call
    // sites is a miscompile LLVM will not diagnose, so both halves are here.
    let shim = function_body(&ir, "i32 @main(");
    assert!(!shim.contains("fastcc"), "the emitted `main` must keep the C convention:\n{shim}");
    assert!(
        ir.contains("define fastcc") && ir.contains("tail call fastcc") || ir.contains("define fastcc"),
        "Buri functions must be `fastcc`:\n{ir}"
    );
}

/// **The marking statement is emitted for a program that has tasks in it, and
/// for no other program.**
///
/// `cli/runtime/lib.rs` §6's second optional call. Two directions, because
/// only the pair says anything: a backend that always made it would put every
/// program on atomic reference counting, and one that never made it would let
/// `Tasks.parallel` fan out over blocks nobody marked — the silent aliasing
/// `design/native/MEMORY.md` §5.5 names and the reason the whole slice exists.
///
/// The negative half is the one worth having. `middle::rc::crosses_tasks` asks
/// the whole post-monomorphization program, and "the whole program" is exactly
/// the kind of question that answers `true` by accident as soon as something
/// unrelated links `core/tasks` in. This asserts that a program that prints a
/// string does not pay for a scheduler it never mentions.
///
/// Asserted on the **optimized** IR, like its `frames_are_per_carrier`
/// neighbour: a call `default<O2>` decided to drop is a call the object does
/// not make.
#[test]
fn the_marking_statement_is_emitted_only_for_a_program_with_tasks() {
    skip_unless_executable!();
    let plain = optimized_ir(&program(
        r#"
export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout };
  let _ = io.println(ctx, "no tasks here").ignore();
  .Ok(())
}
"#,
    ));
    assert!(
        !plain.contains("buri_rt_values_may_cross_tasks"),
        "a program with no tasks in it marked every block it allocates:\n{plain}"
    );
    // …and it is not that this program has no entry point to put a call in.
    assert!(plain.contains("buri_rt_argv_init"), "no entry point in:\n{plain}");

    let tasks = optimized_ir(&format!(
        "{}\n{}",
        r#"
from "core/effect" import { Alloc, Stdout, Tasks };
from "core/host" import * as host;
from "core/io" import * as io;
from "core/tasks" import * as tasks;
"#,
        r#"
export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout, Tasks: host.tasks };
  let doubled = tasks.parallel(ctx, [1, 2, 3], fn(c, i, n) => n * 2);
  let _ = io.println(ctx, "${doubled.len()}").ignore();
  .Ok(())
}
"#,
    ));
    assert!(
        tasks.contains("buri_rt_values_may_cross_tasks"),
        "a program that fans out did not mark its blocks:\n{tasks}"
    );
    // The order is the contract (`lib.rs` §6): the latch decides the header of
    // every block allocated *after* it, so a call that landed after the first
    // allocation would mark a program's heap only from the middle. `main`'s
    // body is where both calls are, and `argv_init` — which allocates no Buri
    // block — is the only thing before it.
    let shim = function_body(&tasks, "i32 @main(");
    let init = shim.find("buri_rt_argv_init").expect("no argv_init in the emitted main");
    let mark = shim.find("buri_rt_values_may_cross_tasks").expect("no mark in the emitted main");
    assert!(init < mark, "the mark was made before the runtime was initialised:\n{shim}");
}

/// `memory(none)` on a function the effect system proved pure, and the
/// correction this backend makes to CODEGEN-LLVM.md §3.1: a *pure* function
/// that can abort does not get it.
#[test]
fn purity_becomes_a_memory_attribute_and_an_abort_takes_it_away() {
    skip_unless_executable!();
    // `collatz` cannot abort — no division, no context, no call it cannot see
    // — and takes no pointer, so there is no argument memory to read and
    // `memory(none)` is the answer. It is recursive so `middle::inline` leaves
    // it alone and it survives to be inspected.
    //
    // `risky` divides, which `rc.rs` records as `can_abort` while leaving
    // `purity` at `Pure`. That pair is the correction this backend makes: it
    // must **not** become `memory(none)`.
    let ir = optimized_ir(&program(
        r#"
// `n % 2` by a literal is a mask rather than a remainder, so this cannot
// abort and stays `Purity::Pure` with `can_abort: false`.
fn steps(n: Int, so_far: Int): Int {
  if (n <= 1) { so_far } else {
    if (n % 2 == 0) { steps(n - 2, so_far + 1) } else { steps(n - 1, so_far + 1) }
  }
}

fn risky(x: Int, y: Int): Int { if (x == 0) { 0 } else { x / y + risky(x - 1, y) } }

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout };
  let a = steps(27, 0);
  let b = risky(3, 2);
  let _ = if (a + b >= 0) { io.println(ctx, "ok").ignore() } else { io.println(ctx, "no").ignore() };
  .Ok(())
}
"#,
    ));

    let pure = function_body(&ir, "@\"main_buri$steps\"");
    assert!(
        pure.contains("memory(none)") || ir.contains("memory(none)"),
        "a pure, non-aborting, pointer-free function must carry `memory(none)`:\n{ir}"
    );
    // The abort-capable one carries the `inaccessiblemem: write` form instead:
    // an abort writes to stderr and exits, which is an observable effect the
    // caller can only detect by not being returned to.
    assert!(
        ir.contains("inaccessiblemem: write"),
        "an abort-capable function must add `inaccessiblemem: write`:\n{ir}"
    );
    assert!(
        ir.contains("noreturn") && ir.contains("cold"),
        "the abort path must be `noreturn` and `cold`:\n{ir}"
    );
}

/// `willreturn` is a promise that the function **returns**, and the backend
/// has to be able to point at the proof.
///
/// CODEGEN-LLVM.md §3.1's row used to read "a function `middle` proved
/// terminates". `middle` proves no such thing — it has no termination analysis
/// at all — so what this backend proves, and what §3.6 now states, is the
/// weaker fact it can check on the IR it is holding: **no cycle**. A function with no loop in its control-flow graph
/// that can reach no cycle in the call graph runs a bounded number of
/// instructions and returns. Anything else is not proven, and an unproven
/// `willreturn` is the licence LLVM used to speculate a recursive call
/// (`a_pure_non_tail_recursion_returns_at_both_profiles`).
///
/// Eight functions, one per row of that rule, asserted on the IR this backend
/// **emits** — `default<O2>` infers its own `willreturn` where it can prove
/// one, and an assertion made after it would be an assertion about LLVM.
#[test]
fn willreturn_is_claimed_only_where_nothing_can_cycle() {
    skip_unless_executable!();
    let ir = emitted_ir(&program(
        r#"
// No call, no loop: the proof is that the body is straight-line. Called
// twice so `middle::inline` leaves it where it can be read (a single-use
// callee is moved into its caller).
fn leaf(x: Int, y: Int): Int { x * x + y * y }

// A leaf's caller is proven too: the callee returns and this adds to it.
// Two calls and a body above `middle::inline`'s trivial size, or it would be
// moved into `main` and there would be nothing here to read.
fn viaLeaf(x: Int): Int { leaf(x, x) + leaf(x, 1) }

// Branches, and still no cycle. This is the row that decided how the
// question is asked: `lower` numbers blocks in the order it built them, and
// a nested `if`'s join block comes *before* one of its predecessors — so
// "an edge that points earlier in the list" reads this function as a loop
// and takes the attribute off every branchy function in the program. The
// answer is a real cycle search, and this is what says so.
fn classify(i: Int): Int { if (i <= 0) { 0 } else { if (i < 10) { 1 } else { 2 } } }

// Self-recursive and *not* a tail call — the shape that was miscompiled.
fn down(i: Int): Int { if (i <= 0) { 0 } else { 1 + down(i - 1) } }

// The same cycle, two functions long. Neither is self-recursive, and a
// per-function look at either one sees only a call to a function that is
// pure and does not abort.
fn pingA(i: Int): Int { if (i <= 0) { 0 } else { 1 + pingB(i - 1) } }
fn pingB(i: Int): Int { if (i <= 0) { 0 } else { 1 + pingA(i - 1) } }

// Tail-recursive, so `middle::tail_calls` has already made it a CFG loop
// (CODEGEN-LLVM.md §2.4). A loop whose trip count nothing bounded is not a
// proof of termination either — and `mustprogress`, which is the other half
// of what this rule emits, would let LLVM *delete* a loop that diverges.
// SPEC 10.4: an implementation may drop a pure call only where it can also
// show the call returns.
fn total(n: Int, acc: Int): Int { if (n <= 0) { acc } else { total(n - 1, acc + n) } }

// A caller of an unproven function is unproven: the promise covers the
// callees too. Two calls and two of them in the body, for the same reason.
fn viaDown(i: Int): Int { down(i) + down(i + 1) }

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout };
  let _ = io.println(ctx, "${viaLeaf(3)} ${viaLeaf(4)} ${classify(3)} ${classify(30)}").ignore();
  let _ = io.println(ctx, "${down(3)} ${pingA(3)} ${total(3, 0)} ${viaDown(2)} ${viaDown(3)}").ignore();
  .Ok(())
}
"#,
    ));

    let attrs_of = |name: &str| -> String {
        let full = format!("main_buri${name}");
        let (head, _) = definitions(&ir)
            .into_iter()
            .find(|(head, _)| head.contains(&full))
            .unwrap_or_else(|| panic!("no `{full}` was emitted; the IR is:\n{ir}"));
        attribute_group(&ir, head).to_string()
    };

    for (name, why) in [
        ("down", "it calls itself, and the call is not a tail call"),
        ("pingA", "it is one half of a two-function cycle"),
        ("pingB", "it is the other half of that cycle"),
        ("total", "its body is a loop with no bound on the trip count"),
        ("viaDown", "it calls a function that is not proven to return"),
    ] {
        let attrs = attrs_of(name);
        assert!(
            !attrs.contains("willreturn"),
            "`{name}` claims `willreturn` and must not — {why}:\n{attrs}"
        );
        assert!(
            !attrs.contains("mustprogress"),
            "`{name}` claims `mustprogress` and must not — {why}:\n{attrs}"
        );
        // The attribute the hoist was actually made of. `speculatable` is
        // only emitted where `willreturn` is, so this is a second reading of
        // the same rule rather than a second rule.
        assert!(
            !attrs.contains("speculatable"),
            "`{name}` claims `speculatable` and must not — {why}:\n{attrs}"
        );
    }

    // The other direction, which is what keeps the conservatism from being a
    // blanket: where the proof exists the attribute is still emitted, and with
    // it `memory(none)` makes the call speculatable.
    for name in ["leaf", "viaLeaf", "classify"] {
        let attrs = attrs_of(name);
        assert!(
            attrs.contains("willreturn") && attrs.contains("mustprogress"),
            "`{name}` reaches no cycle and must keep `willreturn`:\n{attrs}"
        );
        assert!(
            attrs.contains("speculatable"),
            "`{name}` is `memory(none)` and proven to return, so it is speculatable:\n{attrs}"
        );
    }
}

/// `nsw`/`nuw` are never set by this backend — VALUE-MODEL.md §11.1's
/// debuggability argument, which CODEGEN-LLVM.md §3.4 turns into a rule.
///
/// Asserted on the IR this backend *emits*, because `default<O2>` infers its
/// own no-wrap flags from range facts and an assertion after it would be an
/// assertion about LLVM.
#[test]
fn no_wrap_flags_are_never_emitted() {
    skip_unless_executable!();
    let ir = emitted_ir(&program(
        r#"
fn arithmetic(a: Int, b: Int): Int {
  (a + b) * (a - b) + (0 - a)
}

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout };
  let _ = if (arithmetic(3, 2) == 2) { io.println(ctx, "ok").ignore() } else { io.println(ctx, "no").ignore() };
  .Ok(())
}
"#,
    ));
    assert!(!ir.contains(" nsw "), "`nsw` must never be emitted:\n{ir}");
    assert!(!ir.contains(" nuw "), "`nuw` must never be emitted:\n{ir}");
    // `inbounds` on the other hand is set everywhere it applies, because the
    // premise is enforced by the type system rather than assumed away.
    assert!(!ir.contains("getelementptr ") || ir.contains("inbounds"), "a bare gep:\n{ir}");
}

/// `readonly` on a pointer parameter, and the condition CODEGEN-LLVM.md §3.2
/// now attaches to it: the function must not adjust a reference count in that
/// parameter's block.
///
/// `firstOrElse` is the shape that keeps it. It reads two `Str`s and returns
/// one of them, and `middle::rc` gives it no plan at all — the caller owns
/// both — so nothing in it writes through a parameter and the whole of the
/// §3.2 row still applies.
#[test]
fn a_borrowing_function_keeps_readonly_and_argmem_read() {
    skip_unless_executable!();
    let ir = optimized_ir(&program(
        r#"
fn firstOrElse(s: Option<Str>, fallback: Str): Str {
  match (s) { .Some(t) => t, .None => fallback }
}

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout };
  let _ = io.println(ctx, firstOrElse(.Some("yes"), "no")).ignore();
  let _ = io.println(ctx, firstOrElse(.None, "fallback")).ignore();
  .Ok(())
}
"#,
    ));
    // `align 16` is unconditional — every heap pointer is 16-byte aligned
    // because the header is 16 bytes and sits immediately before the payload —
    // so a pointer parameter with neither attribute is a bug either way.
    for line in ir.lines().filter(|l| l.starts_with("define")) {
        // The emitted `main` is the one function whose pointer parameter is
        // not a Buri value: `argv` comes from the platform, and claiming
        // anything about it would be a claim about the C runtime.
        if line.contains("@main(") {
            continue;
        }
        for piece in line.split(',') {
            if piece.contains("ptr ") && piece.contains('%') && !piece.contains("captures") {
                assert!(
                    piece.contains("readonly") || piece.contains("align"),
                    "a pointer parameter with neither `readonly` nor `align`: {piece}\nin:\n{ir}"
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// The reference count is memory — CODEGEN-LLVM.md §3.2, the condition
// ---------------------------------------------------------------------------

/// One `define` line and the body under it.
fn definitions(ir: &str) -> Vec<(&str, &str)> {
    let mut out = Vec::new();
    for chunk in ir.split("\ndefine ").skip(1) {
        let Some((head, rest)) = chunk.split_once('\n') else { continue };
        let body = rest.split("\n}").next().unwrap_or(rest);
        out.push((head, body));
    }
    out
}

/// The `%N` names on a `define` line, and whether each is `readonly`.
fn parameters(head: &str) -> Vec<(String, bool)> {
    let Some(open) = head.find('(') else { return Vec::new() };
    let Some(close) = head.rfind(')') else { return Vec::new() };
    let Some(list) = head.get(open + 1..close) else { return Vec::new() };
    list.split(',')
        .filter_map(|piece| {
            let name = piece.split_whitespace().find(|w| w.starts_with('%'))?;
            Some((name.to_string(), piece.contains("readonly")))
        })
        .collect()
}

/// The attribute group `#N` a `define` line names, resolved through the
/// module's `attributes #N = { ... }` lines.
fn attribute_group<'a>(ir: &'a str, head: &str) -> &'a str {
    let Some(hash) = head.rsplit('#').next().and_then(|t| t.split_whitespace().next()) else {
        return "";
    };
    let key = format!("\nattributes #{hash} = ");
    let Some(at) = ir.find(&key) else { return "" };
    let rest = &ir[at + key.len()..];
    rest.split('\n').next().unwrap_or("")
}

/// **The differential test for the whole miscompile class.**
///
/// Not one golden assertion about one function: a scan of every function the
/// backend emitted for the *pattern* that was undefined behaviour, which is a
/// store to `p - 16` for a `p` LangRef's Pointer Aliasing Rules call *based
/// on* a parameter — `getelementptr` from the parameter, transitively — under
/// a `readonly` on that parameter or a `memory(...)` whose `argmem` is not
/// writable.
///
/// Both halves of that pair are undefined behaviour on their own:
///
///  * `readonly`: "If a function writes to a readonly pointer argument, the
///    behavior is undefined."
///  * `memory(argmem: read)`: "The location is only read. Writing to the
///    location is immediate undefined behavior. This includes the case where
///    the location is read from and then the same value is written back."
///
/// The second sentence is why "the count is not part of the value" is not a
/// defence. LLVM does not have to *exploit* it for the IR to be wrong, and
/// this asserts the IR rather than the exploit — `opt -passes=function-attrs`
/// on the same shape infers `memory(argmem: readwrite)` and no `readonly`,
/// which is the independent check that the corrected answer is the right one.
///
/// Asserted after `default<O2>`, where the `getelementptr` names the parameter
/// directly, and with a count of matches so that a future lowering that stops
/// producing the pattern fails here instead of passing vacuously.
#[test]
fn no_reference_count_store_sits_under_a_read_only_claim() {
    skip_unless_executable!();
    let ir = optimized_ir(&program(
        r#"
// Recursive, so `middle::inline` leaves it alone and it survives to be
// inspected. `s` is used twice in the result, which is what `middle::rc`
// plans an `incref` for — of a value the tail-call loop turns into a block
// parameter whose incoming values are the entry parameter and itself.
fn twice(s: Str, n: Int): (Str, Str) {
  if (n <= 0) { (s, s) } else { twice(s, n - 1) }
}

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout };
  let both = twice("ab".repeat(ctx, 2), 3);
  let _ = io.println(ctx, "${both.0}/${both.1}").ignore();
  .Ok(())
}
"#,
    ));

    let mut found = 0usize;
    for (head, body) in definitions(&ir) {
        let params = parameters(head);
        let attrs = attribute_group(&ir, head);
        for line in body.lines() {
            // `%rc.p = getelementptr inbounds i8, ptr %0, i64 -16`
            let Some((dest, gep)) = line.split_once(" = getelementptr") else { continue };
            if !gep.contains("i64 -16") {
                continue;
            }
            let dest = dest.trim();
            let Some(base) = gep
                .split_whitespace()
                .map(|w| w.trim_end_matches(','))
                .find(|w| w.starts_with('%'))
            else {
                continue;
            };
            let Some((_, readonly)) = params.iter().find(|(n, _)| n == base) else { continue };
            let target = format!(", ptr {dest},");
            let stored = body
                .lines()
                .any(|l| l.trim_start().starts_with("store ") && l.contains(&target));
            if !stored {
                continue;
            }
            found += 1;
            assert!(
                !readonly,
                "a reference count is stored through `{base}`, which is marked \
                 `readonly`:\ndefine {head}\n{body}"
            );
            assert!(
                !attrs.contains("memory(none)"),
                "a function that stores a reference count claims `memory(none)`:\n\
                 define {head}\nattributes: {attrs}"
            );
            assert!(
                !attrs.contains("memory(") || attrs.contains("argmem: readwrite"),
                "a reference count is stored through the parameter `{base}` under \
                 `{attrs}`, which does not make `argmem` writable:\ndefine {head}"
            );
        }
    }
    assert!(
        found > 0,
        "no `p - 16` count store survived to be checked, so this test proved \
         nothing:\n{ir}"
    );
}

/// The other half: what the corrected discipline *says*, rather than what it
/// no longer says.
///
/// `memory(argmem: readwrite)` is the honest floor for a function that counts
/// a parameter, and it is strictly better than the `memory(readwrite)` a
/// backend that gave up would emit — LLVM still knows the function reaches no
/// global and no allocator. And the purity theorem survives untouched where it
/// is true: a function of scalars that counts nothing is still `memory(none)`.
#[test]
fn counting_a_parameter_is_argmem_readwrite_and_purity_survives() {
    skip_unless_executable!();
    let ir = optimized_ir(&program(
        r#"
fn twice(s: Str, n: Int): (Str, Str) {
  if (n <= 0) { (s, s) } else { twice(s, n - 1) }
}

// No pointer, no allocation, no abort — `n % 2` by a literal is a mask — so
// nothing here has changed and the theorem is still an attribute.
fn steps(n: Int, so_far: Int): Int {
  if (n <= 1) { so_far } else {
    if (n % 2 == 0) { steps(n - 2, so_far + 1) } else { steps(n - 1, so_far + 1) }
  }
}

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout };
  let both = twice("ab".repeat(ctx, 2), 3);
  let _ = io.println(ctx, "${both.0}/${both.1} ${steps(27, 0)}").ignore();
  .Ok(())
}
"#,
    ));
    let counting = function_body(&ir, "@\"main_buri$twice\"");
    assert!(
        !counting.lines().next().unwrap_or_default().contains("readonly"),
        "a function that counts its parameter must not call it `readonly`:\n{counting}"
    );
    assert!(
        ir.contains("memory(argmem: readwrite)"),
        "a function that counts a parameter must say `argmem: readwrite`:\n{ir}"
    );
    assert!(
        ir.contains("memory(none)"),
        "a pure, pointer-free, non-aborting function must still be `memory(none)`:\n{ir}"
    );
}

/// The behaviour the attribute was lying about, pinned end to end — and as a
/// **differential between the two pipelines**, which is the shape a
/// miscompile of this class has.
///
/// A false `memory(argmem: read)` or `readonly` is only a wrong *answer* once
/// something exploits it, and the things that exploit memory attributes —
/// GVN, LICM, DSE, load forwarding across a call — run in `default<O2>` and
/// not in `default<O0>`. So the same source through both pipelines has to
/// agree on all three observations, and the third is the one a reference count
/// cannot hide from: `buri_rt_live_blocks` after `main` has returned.
///
/// The program is built out of the one shape that exercises the count and
/// nothing else: `keep` is a tail-recursive loop over a `Str` parameter, which
/// is what `middle::rc` inserts its back-edge `incref`/`decref` pair around,
/// and the `Str` is a *heap* one — `repeat` allocates, so its `base` is not
/// the null a literal carries and its count is a real one. Both the argument
/// and the result are live at the `println`, so a count that came back wrong
/// frees the block while it is still being read.
#[test]
fn the_two_pipelines_agree_about_a_shared_heap_strings_count() {
    skip_unless_executable!();
    let source = program(
        r#"
fn keep(s: Str, n: Int): Str { if (n <= 0) { s } else { keep(s, n - 1) } }

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout };
  let owned = "ab".repeat(ctx, 3);
  let echoed = keep(owned, 4);
  let again = keep(owned, 2);
  let _ = io.println(ctx, "${owned}|${echoed}|${again}").ignore();
  .Ok(())
}
"#,
    );
    let fast = build_and_run_at("rc-attrs-o2", &source, Some(LIVE_PROBE), Profile::Release);
    let plain = build_and_run_at("rc-attrs-o0", &source, Some(LIVE_PROBE), Profile::Debug);

    assert_eq!(fast.0, "ababab|ababab|ababab\n", "stderr was: {}", fast.1);
    assert_eq!(fast.2, Some(0), "stderr was: {}", fast.1);
    assert_eq!(
        fast.0, plain.0,
        "`default<O2>` and `default<O0>` printed different things, which is what \
         an optimizer exploiting a false memory attribute looks like:\n{:?}\n{:?}",
        fast.1, plain.1
    );
    assert_eq!(fast.2, plain.2, "the two pipelines exited differently");
    assert_eq!(
        live_blocks(&fast.1),
        live_blocks(&plain.1),
        "`default<O2>` left a different number of blocks live than `default<O0>`: \
         {:?} against {:?}",
        fast.1,
        plain.1
    );
}

/// `derivePrimJson.Str` takes a count of its own, and it balances.
///
/// The twin of `stencil.rs::nothing_is_leaked`'s fourth shape, and it is here
/// rather than folded into a wider test because the count is *this backend's*
/// decision: `middle::rc`'s plan releases the argument at its last use, because
/// its contract is that a runtime intrinsic borrows (`rc.rs`'s header), and
/// `emit::json_prim` is what makes the `Json` that kept the block own one. Miss
/// the `incref` and this is a double free of a block `noteText` still reads;
/// add one with no matching release and the probe reports a leak. One test
/// sees both, because both move `live` off zero.
///
/// The `Str` is `repeat`'s and not a literal, for the reason
/// `the_two_pipelines_agree_about_a_shared_heap_strings_count` says: a
/// literal's `base` is null and its count is nobody's, so a literal here would
/// pass with no `incref` at all.
#[test]
fn a_json_string_leaf_balances_its_own_count() {
    skip_unless_executable!();
    let (out, err, code) = build_and_run_with(
        "tojson-count",
        &program(
            r#"
from "core/io" import * as io;
from "core/json" import { Json, ToJson };

export struct Note { text: Str }
derive ToJson for Note;

fn noteText(j: Json): Str {
  match (j) {
    .Object(entries) => match (entries) {
      [] => "none",
      [first, ..] => {
        let (_k, v) = first;
        match (v) { .Str(s) => s, _ => "not a string" }
      },
    },
    _ => "not an object",
  }
}

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout };
  let encoded = Note { text: "ab".repeat(ctx, 3) }.toJson(ctx);
  let _ = io.println(ctx, noteText(encoded)).ignore();
  .Ok(())
}
"#,
        ),
        Some(LIVE_PROBE),
    );
    assert_eq!(out, "ababab\n", "stderr was: {err}");
    assert_eq!(code, Some(0), "stderr was: {err}");
    assert_eq!(live_blocks(&err), 0, "a `Json` holding a heap `Str` leaked: {err:?}");
}

/// G2: a reference operation is two counts behind one branch on `cap`'s bit 63.
///
/// The assertion is on the *emitted* IR rather than the optimized IR, because
/// this is a claim about what the emitter writes: `default<O2>` is entitled to
/// notice that nothing in a single translation unit sets the bit and delete
/// the atomic arm, and a test that let it would be asserting nothing.
///
/// Four things, and the last two are the ones that could quietly go wrong:
///
///  * the fork exists on both operations;
///  * the shared arms are a **cold call** into the runtime, which owns the one
///    atomic sequence — open-coding it here instead cost a median +46 % of
///    native release lowering against this form's +21 %, because it is a
///    saturating `atomicrmw` in front of `opt` at every reference operation in
///    the program (`design/PERFORMANCE.md` §6.6);
///  * the **unshared arm is unchanged** — the same load, saturating `select`
///    and store MEMORY.md §5.1 specifies, not an atomic in disguise;
///  * the branch says which arm is hot, so the unshared one stays the
///    fallthrough.
#[test]
fn a_reference_operation_forks_on_the_multi_threaded_bit() {
    skip_unless_executable!();
    let ir = emitted_ir(&program(
        r#"
fn keep(s: Str, n: Int): Str { if (n <= 0) { s } else { keep(s, n - 1) } }

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout };
  let owned = "ab".repeat(ctx, 3);
  let echoed = keep(owned, 4);
  let _ = io.println(ctx, "${owned}|${echoed}").ignore();
  .Ok(())
}
"#,
    ));

    assert!(ir.contains("inc.shared"), "no shared arm on the increment in:\n{ir}");
    assert!(ir.contains("dec.shared"), "no shared arm on the decrement in:\n{ir}");
    assert!(
        ir.contains("call void @buri_rt_incref"),
        "the increment's shared arm does not reach the runtime in:\n{ir}"
    );
    assert!(
        ir.contains("call void @buri_rt_decref"),
        "the decrement's shared arm does not reach the runtime in:\n{ir}"
    );
    assert!(
        ir.contains("declare void @buri_rt_incref") && ir.contains("declare void @buri_rt_decref"),
        "the two shared arms are not declared in:\n{ir}"
    );

    // The unshared arm, unchanged: MEMORY.md §5.1's load, saturating select
    // and store. A `%rc.sat` that stopped being stored would mean the fork
    // replaced the fast path rather than standing beside it.
    assert!(ir.contains("%rc.sat"), "the saturating select is gone from:\n{ir}");
    assert!(ir.contains("%rc.inc"), "the non-atomic increment is gone from:\n{ir}");
    assert!(ir.contains("%rc.dec"), "the non-atomic decrement is gone from:\n{ir}");

    // And the emitter says which arm is hot.
    assert!(
        ir.contains("!prof") && ir.contains("branch_weights"),
        "the fork carries no branch weight in:\n{ir}"
    );
}

/// CODEGEN-LLVM.md §2.2, as a test: nothing in the lowering emits an `alloca`
/// for a local, a parameter, a temporary, a match binding or a loop variable,
/// so a function built out of exactly those has none.
///
/// `walk` is that function and the claim is read out of **its** body, not out
/// of the module. `main` is not built out of exactly those: a print answers a
/// `Result` now, and §2.3's out-pointer boundary gives every runtime entry that
/// does one entry-block buffer (`emit.rs`'s `scratch`, reached from
/// `call_result`) — an `alloca` the section allows and this test is not about.
/// The buffer never escapes, so `default<O2>` promotes it, which is why the
/// *whole-module* claim is still the one made after the pipeline.
#[test]
fn a_hot_function_has_no_allocas() {
    skip_unless_executable!();
    let source = program(
        r#"
struct Point { x: Int, y: Int }
enum Step { Move(Point), Stop }

fn walk(n: Int, at: Point): Point {
  if (n <= 0) { at } else {
    match (Step.Move(Point { x: at.x + 1, y: at.y + 2 })) {
      .Move(p) => walk(n - 1, p),
      .Stop => at,
    }
  }
}

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout };
  let end = walk(100, Point { x: 0, y: 0 });
  let _ = if (end.x == 100) { io.println(ctx, "ok").ignore() } else { io.println(ctx, "no").ignore() };
  .Ok(())
}
"#,
    );
    let optimized = optimized_ir(&source);
    assert!(
        !optimized.contains("alloca"),
        "a function of locals, a struct, an enum and a loop must reach no memory:\n{optimized}"
    );
    // The loop is a CFG loop with phis, because `middle::tail_calls` already
    // rewrote the self-recursive tail call into one (§2.4) and nothing here
    // emulates a mutable slot. Asserted on the emitted IR: at O2 LLVM solves
    // this particular loop into a closed form, which is the optimization doing
    // its job and would leave nothing to look at.
    let emitted = emitted_ir(&source);
    let walk = definitions(&emitted)
        .into_iter()
        .find(|(head, _)| head.contains("main_buri$walk"))
        .map(|(_, body)| body)
        .unwrap_or_else(|| panic!("no `main_buri$walk` was emitted; the IR is:\n{emitted}"));
    assert!(!walk.contains("alloca"), "no `alloca` before the pipeline either:\n{walk}");
    assert!(walk.contains("phi "), "the loop must carry phis:\n{walk}");
}

/// The pipeline is what it says it is, and the module carries the triple and
/// the data layout the machine chose — without which LLVM would size a pointer
/// from a default that is not this target's.
#[test]
fn the_module_carries_the_target() {
    skip_unless_executable!();
    let ir = optimized_ir(&program(
        r#"
export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout };
  let _ = io.println(ctx, "x").ignore();
  .Ok(())
}
"#,
    ));
    assert!(ir.contains("target datalayout"), "no data layout in:\n{ir}");
    assert!(ir.contains("target triple"), "no triple in:\n{ir}");
    let expected = llvm::target::triple(host_target()).unwrap();
    assert!(ir.contains(&expected), "the module names a different triple than the machine");
}

// ---------------------------------------------------------------------------
// The seams, asserted rather than assumed
// ---------------------------------------------------------------------------

/// `missing_intrinsics` answers *before* LLVM is started, which is the whole
/// reason the hook is on the trait rather than accumulated during emission.
///
/// A string interpolation is the case that matters: `lower::template` emits
/// `str.concat` at every join and there is no `buri_rt_str_concat` — the
/// archive is deliberately only allocation, aborts and host capabilities
/// (`cli/runtime/lib.rs` §0).
#[test]
fn an_unimplemented_intrinsic_is_reported_before_llvm_runs() {
    use buri::compiler::backend::Backend as _;
    let mut map = SourceMap::new();
    let mut cache = buri::parsing::parser::Cache::new();
    let source = program(
        r#"
export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout };
  let n = 7;
  let _ = io.println(ctx, "n is ${n}").ignore();
  .Ok(())
}
"#,
    );
    let analysis =
        driver::analyze_snippet_in(None, &mut map, &mut cache, "main.buri", &source, Role::Entry);
    assert!(!analysis.diagnostics.has_errors(), "{}", render(&analysis.diagnostics, &map));
    let entry = analysis.checked.entry.unwrap();
    let module_paths: Vec<String> =
        analysis.loaded.modules.iter().map(|m| m.path.clone()).collect();
    let mut diagnostics = Diagnostics::new();
    let mut mono = middle::monomorphize::run(
        &analysis.checked,
        module_paths,
        &mut diagnostics,
        middle::monomorphize::Roots::Main(entry),
    );
    middle::run(&mut mono, &middle::Options::default());

    let missing = llvm::Llvm.missing_intrinsics(&mono, &analysis.checked.tables);
    // An interpolation is `str.concat` plus a template hole, and both are
    // emitted now — so the hook's job here is to say *nothing*, which is the
    // half of it that is easiest to break by accident.
    assert!(missing.is_empty(), "an interpolation is implemented now, got {missing:?}");

    // `core/list` has no gap left: every key in `list.buri` is either a row in
    // `llvm/runtime.rs`'s table or a loop in `emit::Unit`, and `map`, `sortBy`,
    // `zip` and `flatten` were the last of it to land. So the example
    // this test is built around moved out of `core/list`, and then twice more:
    // to `char.isDigit`, to `bytes.toUtf8`, and now past both. `core/char` and
    // `core/bytes` have archive bodies (`cli/runtime/char.rs`,
    // `cli/runtime/bytes.rs`) and `data/strings.buri`, `text/bytes.buri`,
    // `crypto/sha256.buri` and two of the three `//lib/proto` files are in the
    // native conformance set because of them.
    //
    // `math.sin` is the example now, and it is a different *kind* of gap on
    // purpose: it is refused rather than unwritten. `cli/runtime/math.rs`
    // argues that IEEE 754 does not fix a transcendental's answer, so V8's
    // fdlibm port and the platform libm differ in the last bit and a rendered
    // `Float` shows seventeen digits of it. That makes it the one example that
    // will not stop being true next wave, which is what this assertion wants:
    // the *mechanism* is what is under test, and it has been re-pointed three
    // times at things that were merely early.
    let with_closure = program(
        r#"
from "core/bytes" import * as bytes;
from "core/io" import * as io;
from "core/list" import * as list;
from "core/math" import * as math;
from "core/str" import * as str;

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout };
  let sorted = list.range(ctx, 0, 3).sortBy(ctx, fn(a, b) => a.compare(b));
  let paired = sorted.zip(ctx, list.range(ctx, 0, 3));
  let nested = [paired.mapCtx(ctx, fn(c, p) => p.0)].flatten(ctx);
  let digits = "a1".chars(ctx).count(fn(c) => c.isDigit());
  let raw = bytes.toUtf8(ctx, "hi");
  let wave = math.sin(1.0);
  let _ = io.println(ctx, "${nested.len()}${digits}${raw.len()}${wave}").ignore();
  .Ok(())
}
"#,
    );
    let analysis = driver::analyze_snippet_in(
        None,
        &mut map,
        &mut cache,
        "closure.buri",
        &with_closure,
        Role::Entry,
    );
    assert!(!analysis.diagnostics.has_errors(), "{}", render(&analysis.diagnostics, &map));
    let entry = analysis.checked.entry.unwrap();
    let module_paths: Vec<String> =
        analysis.loaded.modules.iter().map(|m| m.path.clone()).collect();
    let mut diagnostics = Diagnostics::new();
    let mut mono = middle::monomorphize::run(
        &analysis.checked,
        module_paths,
        &mut diagnostics,
        middle::monomorphize::Roots::Main(entry),
    );
    middle::run(&mut mono, &middle::Options::default());
    let missing = llvm::Llvm.missing_intrinsics(&mono, &analysis.checked.tables);
    assert!(
        missing.iter().any(|m| m == "math.sin"),
        "`math.sin` is refused on purpose and must be reported, got {missing:?}"
    );
    assert!(
        !missing.iter().any(|m| m == "char.isDigit"),
        "`char.isDigit` is a row in the runtime table now and must not be \
         reported, got {missing:?}"
    );
    assert!(
        !missing.iter().any(|m| m == "bytes.toUtf8"),
        "`bytes.toUtf8` is a row in the runtime table now and must not be \
         reported, got {missing:?}"
    );
    assert!(
        !missing.iter().any(|m| m == "list.map"),
        "`list.map` is emitted as a loop now and must not be reported, got {missing:?}"
    );
    assert!(
        !missing.iter().any(|m| m == "list.sortBy"),
        "`list.sortBy` is emitted as a merge sort now and must not be reported, \
         got {missing:?}"
    );
    assert!(
        !missing.iter().any(|m| m == "list.zip" || m == "list.flatten"),
        "`zip` and `flatten` are emitted as loops now and must not be reported, \
         got {missing:?}"
    );
}

/// The identity is what invalidates a cached object when the toolchain's LLVM
/// moves, and the trait's documentation names this backend as the one for
/// which a constant would be a lie.
#[test]
fn the_identity_names_the_linked_llvm_and_inkwell() {
    use buri::compiler::backend::Backend as _;
    let id = llvm::Llvm.identity();
    assert!(id.starts_with("llvm 21."), "{id}");
    assert!(id.contains("inkwell 0.10"), "{id}");
    assert_eq!(llvm::Llvm.name(), "llvm");
}

/// Two runs of the same program produce the same bytes, which is the claim
/// `--check-reproducible` exists to make about the whole build and which has
/// to hold of one unit first.
#[test]
fn emission_is_deterministic() {
    skip_unless_executable!();
    let source = program(
        r#"
enum Shape { Circle(Int), Square(Int) }
fn area(s: Shape): Int { match (s) { .Circle(r) => 3 * r * r, .Square(w) => w * w } }

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout };
  let _ = if (area(Shape.Circle(2)) == 12) { io.println(ctx, "ok").ignore() } else { io.println(ctx, "no").ignore() };
  .Ok(())
}
"#,
    );
    let once = {
        let l = lower(&source);
        expect(llvm::emit_lowered(&l.ir, &l.tables, &options(Profile::Release), Some(l.entry)))
    };
    let twice = {
        let l = lower(&source);
        expect(llvm::emit_lowered(&l.ir, &l.tables, &options(Profile::Release), Some(l.entry)))
    };
    assert_eq!(once.len(), twice.len());
    for (a, b) in once.iter().zip(twice.iter()) {
        assert_eq!(a.name, b.name);
        assert_eq!(a.key.as_str(), b.key.as_str(), "the codegen key moved between two runs");
        assert_eq!(a.bytes, b.bytes, "unit `{}` is not byte-identical", a.name);
    }
}

/// The codegen key is content-addressed on the IR: a program whose IR differs
/// gets a different key, and one whose IR does not does not.
#[test]
fn the_codegen_key_follows_the_ir() {
    skip_unless_executable!();
    let one = program(
        r#"
export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout };
  let _ = io.println(ctx, "a").ignore();
  .Ok(())
}
"#,
    );
    // The same program with a comment added: different bytes, identical IR.
    let same = program(
        r#"
// a comment changes no instruction
export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout };
  let _ = io.println(ctx, "a").ignore();
  .Ok(())
}
"#,
    );
    let other = program(
        r#"
export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout };
  let _ = io.println(ctx, "b").ignore();
  .Ok(())
}
"#,
    );
    let keys = |source: &str| {
        let l = lower(source);
        expect(llvm::emit_lowered(&l.ir, &l.tables, &options(Profile::Release), Some(l.entry)))
            .into_iter()
            .map(|u| u.key.as_str().to_string())
            .collect::<Vec<_>>()
    };
    assert_eq!(keys(&one), keys(&same), "reformatting must not move a codegen key");
    assert_ne!(keys(&one), keys(&other), "a changed literal must move a codegen key");
}

// ---------------------------------------------------------------------------
// The intrinsic surface
// ---------------------------------------------------------------------------

/// A template hole at every primitive `ir::Inst::Structural { op: Show }` can
/// carry.
///
/// This is the shape that used to be a flat refusal, and it is the one every
/// program hits: `middle::lower` emits a hole as a `Structural` at the hole's
/// *interned* type, so unlike `derivePrimShow` every primitive is reachable
/// here. The bytes are the JavaScript backend's — `runtime.js`'s `$str` — so
/// what is asserted is the rendering and not the call.
#[test]
fn a_template_hole_renders_every_primitive() {
    skip_unless_executable!();
    let (out, err, code) = build_and_run(
        "template",
        &program(
            r#"
export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout };
  let n = 42;
  let neg: I32 = -7;
  let small: U8 = 255;
  let big: I128 = 170141183460469231731687303715884105727;
  let f = 1.5;
  let single: F32 = 0.25;
  let yes = true;
  let c = 'q';
  let s = "inner";
  let _ = io.println(ctx, "int ${n}").ignore();
  let _ = io.println(ctx, "neg ${neg}").ignore();
  let _ = io.println(ctx, "u8 ${small}").ignore();
  let _ = io.println(ctx, "i128 ${big}").ignore();
  let _ = io.println(ctx, "f64 ${f}").ignore();
  let _ = io.println(ctx, "f32 ${single}").ignore();
  let _ = io.println(ctx, "bool ${yes} ${!yes}").ignore();
  let _ = io.println(ctx, "char ${c}").ignore();
  let _ = io.println(ctx, "str ${s}").ignore();
  .Ok(())
}
"#,
        ),
    );
    assert_eq!(
        out,
        "int 42\nneg -7\nu8 255\ni128 170141183460469231731687303715884105727\nf64 1.5\n\
         f32 0.25\nbool true false\nchar q\nstr inner\n",
        "stderr was: {err}"
    );
    assert_eq!(code, Some(0));
}

/// The `num.<T>.<op>` surface, emitted inline.
///
/// `abs` at `-0.0` and `signum` at `NaN` are the two that a comparison-shaped
/// implementation gets wrong, so both are here: `js/intrinsics.rs` uses
/// `Math.abs` (which normalizes `-0`) and `x < 0 ? -1 : (x > 0 ? 1 : 0)`
/// (which answers `0` for `NaN`).
#[test]
fn the_numeric_surface_runs() {
    skip_unless_executable!();
    let (out, err, code) = build_and_run(
        "numeric",
        &program(
            r#"
from "core/io" import * as io;
from "core/num" import * as num;

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout };
  let a = -9;
  let _ = io.println(ctx, "abs ${a.abs()}").ignore();
  let _ = io.println(ctx, "sign ${a.signum()} ${0.signum()} ${9.signum()}").ignore();
  let _ = io.println(ctx, "min ${num.min(a, 3)} ${num.max(a, 3)}").ignore();
  let x = -0.0;
  let _ = io.println(ctx, "fabs ${x.abs()}").ignore();
  let big = 3.9;
  let _ = io.println(ctx, "wrap ${big.wrapToI64()}").ignore();
  let n: I32 = 300;
  let _ = io.println(ctx, "wrapToU8 ${n.wrapToU8()}").ignore();
  let _ = io.println(ctx, "toF64 ${n.toF64()}").ignore();
  let u: U8 = 200;
  let _ = io.println(ctx, "widen ${u.toI64()}").ignore();
  let s: I8 = -1;
  let _ = io.println(ctx, "sext ${s.toI64()}").ignore();
  let _ = io.println(ctx, "show ${(7).show(ctx)} ${(0.5).show(ctx)}").ignore();
  let _ = io.println(ctx, "eq ${(7).eq(7)} ${'a'.eq('b')} ${true.eq(true)}").ignore();
  .Ok(())
}
"#,
        ),
    );
    assert_eq!(
        out,
        "abs 9\nsign -1 0 1\nmin -9 3\nfabs 0.0\nwrap 3\nwrapToU8 44\ntoF64 300.0\n\
         widen 200\nsext -1\nshow 7 0.5\neq true false true\n",
        "stderr was: {err}"
    );
    assert_eq!(code, Some(0));
}

/// A float-to-integer conversion **saturates** rather than trapping.
///
/// A plain `fptosi` is `poison` outside the target's range, and `poison`
/// reaching a value a program prints is undefined behaviour where SPEC has a
/// defined answer — so this is `llvm.fptosi.sat`'s clamp, which is also the
/// clamp `fcvt_to_sint_sat` performs on the other backend.
#[test]
fn float_to_integer_conversions_saturate() {
    skip_unless_executable!();
    let (out, err, code) = build_and_run(
        "saturate",
        &program(
            r#"
fn narrow(x: Float): I32 { x.wrapToI32() }

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout };
  let _ = io.println(ctx, "hi ${narrow(1.0e30)}").ignore();
  let _ = io.println(ctx, "lo ${narrow(-1.0e30)}").ignore();
  let _ = io.println(ctx, "mid ${narrow(-3.7)}").ignore();
  .Ok(())
}
"#,
        ),
    );
    assert_eq!(out, "hi 2147483647\nlo -2147483648\nmid -3\n", "stderr was: {err}");
    assert_eq!(code, Some(0));
}

/// `str.len` is the number of Unicode **scalars** and not of bytes, with the
/// ASCII flag deciding whether that costs a mask or a scan.
///
/// Both halves are exercised: an ASCII literal takes the flag's fast path and
/// a literal with an astral character takes `buri_rt_str_scalar_len`.
#[test]
fn str_len_counts_scalars_and_takes_both_paths() {
    skip_unless_executable!();
    let (out, err, code) = build_and_run(
        "strlen",
        &program(
            r#"
export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout };
  let _ = io.println(ctx, "ascii ${"hello".len()}").ignore();
  let _ = io.println(ctx, "wide ${"héllo".len()}").ignore();
  let _ = io.println(ctx, "astral ${"a😀b".len()}").ignore();
  let _ = io.println(ctx, "empty ${"".len()}").ignore();
  .Ok(())
}
"#,
        ),
    );
    assert_eq!(out, "ascii 5\nwide 5\nastral 3\nempty 0\n", "stderr was: {err}");
    assert_eq!(code, Some(0));
}

/// `str.concat`, open-coded: one allocation and two `memcpy`s, with the ASCII
/// flag the conjunction of the two operands'.
///
/// The flag is what the third line pins: a concatenation of an ASCII half and
/// a non-ASCII one is not ASCII, and a `len()` that trusted a wrongly-set flag
/// would answer the byte count instead of the scalar count.
#[test]
fn str_concat_is_open_coded_and_keeps_the_ascii_flag() {
    skip_unless_executable!();
    let (out, err, code) = build_and_run(
        "concat",
        &program(
            r#"
from "core/io" import * as io;
from "core/str" import * as str;

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout };
  let joined = "abc".concat(ctx, "def");
  let _ = io.println(ctx, joined).ignore();
  let _ = io.println(ctx, "${joined.len()}").ignore();
  let mixed = "ab".concat(ctx, "😀");
  let _ = io.println(ctx, mixed).ignore();
  let _ = io.println(ctx, "${mixed.len()}").ignore();
  let _ = io.println(ctx, "${"".concat(ctx, "x")}").ignore();
  .Ok(())
}
"#,
        ),
    );
    assert_eq!(out, "abcdef\n6\nab😀\n3\nx\n", "stderr was: {err}");
    assert_eq!(code, Some(0));
}

use crate::shared::{probed, ALLOC_PROBE};

/// MEMORY.md §5.3, pinned by allocation count on this backend too.
///
/// `str.concat` is open-coded here rather than called, so this backend's
/// `concat` carries its own copy of the three paths and needs its own
/// assertion that they are the paths taken. A chain of a thousand
/// concatenations onto a uniquely-owned string reallocates O(log n) times;
/// without the fast path it allocates once per step.
#[test]
fn a_unique_concat_loop_allocates_logarithmically() {
    skip_unless_executable!();
    let (out, err, _) = build_and_run_with(
        "concat-growth",
        &program(
            r#"
from "core/io" import * as io;
from "core/str" import * as str;

export fn build(s: Str, i: Int): Str {
  if (i == 0) { s } else { build(s.concat(host.alloc, "xy"), i - 1) }
}

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout };
  let s = build("", 1000);
  let _ = io.println(ctx, "${s.len()}").ignore();
  let _ = io.println(ctx, s.slice(0, 4)).ignore();
  .Ok(())
}
"#,
        ),
        Some(ALLOC_PROBE),
    );
    assert_eq!(out, "2000\nxyxy\n", "stderr was: {err}");
    let (blocks, _) = probed(&err);
    assert!(
        blocks < 50,
        "a thousand concatenations allocated {blocks} blocks: the fast path did not fire"
    );
}

/// The observable-semantics guard: a string a second binding still holds has
/// a count above one, so concatenating onto it must copy.
///
/// A fast path that fired on a shared string would print the wrong answer, not
/// merely allocate less — which is why this is an output assertion.
#[test]
fn a_shared_concat_does_not_mutate_what_is_shared() {
    skip_unless_executable!();
    let (out, err, code) = build_and_run(
        "concat-shared",
        &program(
            r#"
from "core/io" import * as io;
from "core/str" import * as str;

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout };
  let base = "ab".concat(ctx, "cd");
  let a = base.concat(ctx, "-one");
  let b = base.concat(ctx, "-two");
  let _ = io.println(ctx, base).ignore();
  let _ = io.println(ctx, a).ignore();
  let _ = io.println(ctx, b).ignore();
  let _ = io.println(ctx, base).ignore();
  .Ok(())
}
"#,
        ),
    );
    assert_eq!(out, "abcd\nabcd-one\nabcd-two\nabcd\n", "stderr was: {err}");
    assert_eq!(code, Some(0));
}

/// A borrowed local handed to a construct **beside** a sibling that holds its
/// last mention — `middle::rc`'s `children`, and the reason the deferral there
/// exists.
///
/// `"${s} ${x} ${y} ${s.len()}"` is the shape: the first hole is `s` itself, so
/// the concatenation chain is holding three uncounted words out of `s` while
/// the last hole computes `s.len()`. A drop after the rightmost mention frees
/// the block those words point into, and the failure is a wrong answer rather
/// than a crash. It is a middle-end fact, so both backends show it and both
/// pin it.
#[test]
fn a_borrowed_local_survives_a_sibling_that_holds_its_last_mention() {
    skip_unless_executable!();
    let (out, err, code) = build_and_run(
        "borrow-across-siblings",
        &program(
            r#"
from "core/io" import * as io;
from "core/str" import * as str;

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout };
  let base = "ab".concat(ctx, "cd");
  let a = base.concat(ctx, "-one");
  let b = base.concat(ctx, "-two");
  let _ = io.println(ctx, "${base} ${a} ${b} ${base.len()}").ignore();
  let _ = io.println(ctx, "${b} ${b.len()}").ignore();
  .Ok(())
}
"#,
        ),
    );
    assert_eq!(out, "abcd abcd-one abcd-two 4\nabcd-two 8\n", "stderr was: {err}");
    assert_eq!(code, Some(0));
}

/// A `Str` view whose block someone else still holds is not a place to write
/// either: two slices of one allocation, and appending to the first must leave
/// the second and the whole alone.
#[test]
fn appending_to_a_view_does_not_disturb_its_siblings() {
    skip_unless_executable!();
    let (out, err, code) = build_and_run(
        "concat-view",
        &program(
            r#"
from "core/io" import * as io;
from "core/str" import * as str;

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout };
  let whole = "left".concat(ctx, ",right");
  let head = whole.slice(0, 4);
  let tail = whole.slice(5, 10);
  let grown = head.concat(ctx, "!!");
  let _ = io.println(ctx, head).ignore();
  let _ = io.println(ctx, tail).ignore();
  let _ = io.println(ctx, grown).ignore();
  let _ = io.println(ctx, whole).ignore();
  .Ok(())
}
"#,
        ),
    );
    assert_eq!(out, "left\nright\nleft!!\nleft,right\n", "stderr was: {err}");
    assert_eq!(code, Some(0));
}

/// The list half of MEMORY.md §5.3, which lives in `cli/runtime/list.rs` and
/// is therefore the same code on both backends — but the *call* is this
/// backend's, so the count is asserted here as well.
#[test]
fn a_unique_push_loop_allocates_logarithmically() {
    skip_unless_executable!();
    let (out, err, _) = build_and_run_with(
        "push-growth",
        &program(
            r#"
from "core/io" import * as io;
from "core/list" import * as list;

export fn build(xs: [Int], i: Int): [Int] {
  if (i == 0) { xs } else { build(xs.push(host.alloc, i), i - 1) }
}

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout };
  let xs = build([], 2000);
  let _ = io.println(ctx, "${xs.len()}").ignore();
  .Ok(())
}
"#,
        ),
        Some(ALLOC_PROBE),
    );
    assert_eq!(out, "2000\n", "stderr was: {err}");
    let (blocks, _) = probed(&err);
    assert!(
        blocks < 50,
        "two thousand pushes allocated {blocks} blocks: the uniqueness fast path did not fire"
    );
}

/// The same guard for the list half: two pushes onto a list a binding still
/// holds must answer two distinct lists and leave the original alone.
#[test]
fn a_shared_push_does_not_mutate_what_is_shared() {
    skip_unless_executable!();
    let (out, err, code) = build_and_run(
        "push-shared",
        &program(
            r#"
from "core/io" import * as io;
from "core/list" import * as list;

export fn total(xs: [Int], i: Int, acc: Int): Int {
  if (i == xs.len()) { acc } else {
    match (xs.get(i)) {
      .Some(v) => total(xs, i + 1, acc + v),
      .None => acc,
    }
  }
}

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout };
  let xs = [1, 2, 3].push(host.alloc, 4);
  let a = xs.push(host.alloc, 100);
  let b = xs.push(host.alloc, 200);
  let _ = io.println(ctx, "${xs.len()} ${total(xs, 0, 0)}").ignore();
  let _ = io.println(ctx, "${a.len()} ${total(a, 0, 0)}").ignore();
  let _ = io.println(ctx, "${b.len()} ${total(b, 0, 0)}").ignore();
  .Ok(())
}
"#,
        ),
    );
    assert_eq!(out, "4 10\n5 110\n5 210\n", "stderr was: {err}");
    assert_eq!(code, Some(0));
}

/// The `str.*` entries that write a `Str` or a `[Str]` through a trailing
/// out-pointer (`cli/runtime/lib.rs` §2 rule 2).
#[test]
fn the_string_surface_runs() {
    skip_unless_executable!();
    let (out, err, code) = build_and_run(
        "strings",
        &program(
            r#"
from "core/io" import * as io;
from "core/list" import * as list;

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout };
  let _ = io.println(ctx, "trim [${"  hi  ".trim()}]").ignore();
  let _ = io.println(ctx, "ends [${"  hi  ".trimStart()}][${"  hi  ".trimEnd()}]").ignore();
  let _ = io.println(ctx, "slice ${"abcdef".slice(1, 4)}").ignore();
  let _ = io.println(ctx, "upper ${"abc".toUpper(ctx)} lower ${"ABC".toLower(ctx)}").ignore();
  let _ = io.println(ctx, "repeat ${"ab".repeat(ctx, 3)}").ignore();
  let _ = io.println(ctx, "replace ${"a-b-c".replace(ctx, "-", "+")}").ignore();
  let _ = io.println(ctx, "starts ${"abc".startsWith("ab")} ${"abc".endsWith("bc")}").ignore();
  let _ = io.println(ctx, "contains ${"abc".contains("b")} ${"abc".contains("z")}").ignore();
  let parts = "a,b,c".split(ctx, ",");
  let _ = io.println(ctx, "split ${parts.len()} ${parts.join(ctx, "|")}").ignore();
  let ls = "one\ntwo".lines(ctx);
  let _ = io.println(ctx, "lines ${ls.len()} ${ls.join(ctx, "/")}").ignore();
  .Ok(())
}
"#,
        ),
    );
    assert_eq!(
        out,
        "trim [hi]\nends [hi  ][  hi]\nslice bcd\nupper ABC lower abc\nrepeat ababab\n\
         replace a+b+c\nstarts true true\ncontains true false\nsplit 3 a|b|c\n\
         lines 2 one/two\n",
        "stderr was: {err}"
    );
    assert_eq!(code, Some(0));
}

/// `cli/runtime/lib.rs` §2 rule 3: an `i32` discriminant and a payload through
/// an out-pointer, translated into whatever `middle::layout` chose.
///
/// All three enum encodings are here. `Option<Str>` and `Option<(Str, Str)>`
/// take the **niche**, so `.Some` is the payload with a non-null `ptr` and
/// there is no tag to write; `Option<Char>` and `Option<Int>` are **tagged**,
/// so the arm has to supply the discriminant itself. Both arms of each are
/// taken, because a translation that got `.None` wrong would still print the
/// right thing for `.Some`.
#[test]
fn an_option_returning_entry_takes_both_arms() {
    skip_unless_executable!();
    let (out, err, code) = build_and_run(
        "sums",
        &program(
            r#"
export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout };
  let _ = match ("abc".charAt(1)) { .Some(c) => io.println(ctx, "at ${c}").ignore(), .None => io.println(ctx, "at none").ignore() };
  let _ = match ("abc".charAt(9)) { .Some(c) => io.println(ctx, "at ${c}").ignore(), .None => io.println(ctx, "at none").ignore() };
  let _ = match ("abc".indexOf("c")) { .Some(i) => io.println(ctx, "idx ${i}").ignore(), .None => io.println(ctx, "idx none").ignore() };
  let _ = match ("abc".indexOf("z")) { .Some(i) => io.println(ctx, "idx ${i}").ignore(), .None => io.println(ctx, "idx none").ignore() };
  let _ = match ("42".toInt()) { .Some(n) => io.println(ctx, "int ${n}").ignore(), .None => io.println(ctx, "int none").ignore() };
  let _ = match ("4x".toInt()) { .Some(n) => io.println(ctx, "int ${n}").ignore(), .None => io.println(ctx, "int none").ignore() };
  let _ = match ("1.5".toFloat()) { .Some(x) => io.println(ctx, "flt ${x}").ignore(), .None => io.println(ctx, "flt none").ignore() };
  let _ = match ("a=b".splitOnce("=")) {
    .Some(pair) => io.println(ctx, "pair ${pair.0}|${pair.1}").ignore(),
    .None => io.println(ctx, "pair none").ignore(),
  };
  let _ = match ("ab".splitOnce("=")) {
    .Some(pair) => io.println(ctx, "pair ${pair.0}|${pair.1}").ignore(),
    .None => io.println(ctx, "pair none").ignore(),
  };
  .Ok(())
}
"#,
        ),
    );
    assert_eq!(
        out,
        "at b\nat none\nidx 2\nidx none\nint 42\nint none\nflt 1.5\npair a|b\npair none\n",
        "stderr was: {err}"
    );
    assert_eq!(code, Some(0));
}

/// The `list.*` entries that are a block copy, at an element type with **no**
/// counted pointer — so `retain` is null, which is the common case
/// (`cli/runtime/list.rs`'s header).
#[test]
fn the_list_surface_runs_over_plain_elements() {
    skip_unless_executable!();
    let (out, err, code) = build_and_run(
        "lists",
        &program(
            r#"
from "core/io" import * as io;
from "core/list" import * as list;

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout };
  let xs = list.range(ctx, 0, 4);
  let _ = io.println(ctx, "range ${xs.len()}").ignore();
  let _ = match (xs.get(2)) { .Some(v) => io.println(ctx, "get ${v}").ignore(), .None => io.println(ctx, "get none").ignore() };
  let _ = match (xs.get(9)) { .Some(v) => io.println(ctx, "get ${v}").ignore(), .None => io.println(ctx, "get none").ignore() };
  let more = xs.push(ctx, 9);
  let _ = match (more.get(4)) { .Some(v) => io.println(ctx, "push ${v}").ignore(), .None => io.println(ctx, "push none").ignore() };
  let both = xs.concat(ctx, more);
  let _ = io.println(ctx, "concat ${both.len()}").ignore();
  let back = xs.reverse(ctx);
  let _ = match (back.get(0)) { .Some(v) => io.println(ctx, "rev ${v}").ignore(), .None => io.println(ctx, "rev none").ignore() };
  let mid = xs.slice(ctx, 1, 3);
  let _ = io.println(ctx, "slice ${mid.len()}").ignore();
  let rep = list.repeat(ctx, 7, 3);
  let _ = io.println(ctx, "repeat ${rep.len()}").ignore();
  let _ = match (rep.get(2)) { .Some(v) => io.println(ctx, "rep ${v}").ignore(), .None => io.println(ctx, "rep none").ignore() };
  let none: [Int] = list.empty<Int>();
  let _ = io.println(ctx, "empty ${none.len()}").ignore();
  .Ok(())
}
"#,
        ),
    );
    assert_eq!(
        out,
        "range 4\nget 2\nget none\npush 9\nconcat 9\nrev 3\nslice 2\nrepeat 3\nrep 7\nempty 0\n",
        "stderr was: {err}"
    );
    assert_eq!(code, Some(0));
}

/// A `[Str]` copied by a runtime entry, with the **generated retain glue**
/// taking a reference on every element.
///
/// This is the test the glue exists for. `cli/runtime/list.rs`'s header:
/// `middle::rc` assumes "a runtime intrinsic borrows its arguments and returns
/// a fresh count" (`rc.rs:98`), so a `[Str]` built by copying another one's
/// bytes has `n` new references and something has to take them. A null
/// `retain` here would be a use-after-free the moment the source went out of
/// scope, and the shape of that bug is a printed string that is not the one
/// that was stored — so the assertion is on the bytes after the source is
/// gone.
#[test]
fn a_list_of_strings_is_retained_by_the_generated_glue() {
    skip_unless_executable!();
    let (out, err, code) = build_and_run(
        "retain",
        &program(
            r#"
from "core/io" import * as io;
from "core/list" import * as list;
from "core/str" import * as str;

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout };
  let kept = {
    let base = ["alpha".concat(ctx, "!"), "beta".concat(ctx, "!")];
    base.push(ctx, "gamma".concat(ctx, "!")).reverse(ctx)
  };
  let _ = io.println(ctx, kept.join(ctx, ",")).ignore();
  let _ = io.println(ctx, "${kept.len()}").ignore();
  .Ok(())
}
"#,
        ),
    );
    assert_eq!(out, "gamma!,beta!,alpha!\n3\n", "stderr was: {err}");
    assert_eq!(code, Some(0));
}

/// `==` and `<` at a `Str`, which has no comparison instruction.
///
/// `middle::derives` lowers a derived `Eq` over a type with a `Str` in it to
/// an `ir::Inst::Binary` at `Prim::Str` (`derives.rs`'s `fn eq`), so the
/// integer path would compare two three-word structs and answer with whichever
/// operand it happened to keep. Both the direct comparison and the derived one
/// are here, because they arrive by different routes.
#[test]
fn strings_compare_and_order() {
    skip_unless_executable!();
    let (out, err, code) = build_and_run(
        "strcmp",
        &program(
            r#"
from "core/io" import * as io;
from "core/order" import { Order };

struct Named { name: Str, rank: Int }
derive Eq for Named;

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout };
  let _ = io.println(ctx, "eq ${"abc" == "abc"} ${"abc" == "abd"}").ignore();
  // The declared method, which is the `Ret::Int(32)` path: `buri_rt_str_compare`
  // answers an `i32` and `Order` is a bare tag of whatever width the layout
  // table chose, so the narrowing is the whole of what is being pinned.
  let _ = io.println(ctx, "cmp ${"a".compare("b") == Order.Less} ${"b".compare("b") == Order.Equal} ${"c".compare("b") == Order.Greater}").ignore();
  let _ = io.println(ctx, "ord ${"abc" < "abd"} ${"b" > "a"} ${"a" >= "a"}").ignore();
  let one = Named { name: "x", rank: 1 };
  let same = Named { name: "x", rank: 1 };
  let other = Named { name: "y", rank: 1 };
  let _ = io.println(ctx, "derived ${one == same} ${one == other}").ignore();
  .Ok(())
}
"#,
        ),
    );
    assert_eq!(
        out,
        "eq true false\ncmp true true true\nord true true true\nderived true false\n",
        "stderr was: {err}"
    );
    assert_eq!(code, Some(0));
}

/// `derive Show`, whose primitive leaves are `derivePrimShow.<T>`.
///
/// The primitive is in the **key** — `middle::lower`'s `qualified_key` appends
/// it, because the lowered IR records `I64` for `I64` and `U64` alike — so
/// every arm is reachable and the quoting is the one thing that separates this
/// from a template hole: `$show` of a `Str` is `JSON.stringify` and of a `Char`
/// is `'c'`, while `$str` of either is the value itself.
#[test]
fn a_derived_show_renders_every_primitive_leaf() {
    skip_unless_executable!();
    let (out, err, code) = build_and_run(
        "derive",
        &program(
            r#"
struct Tagged { label: Str, live: Bool, ratio: Float, n: Int, small: U8, c: Char }
derive Show for Tagged;

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout };
  let t = Tagged { label: "hi", live: true, ratio: 0.5, n: -3, small: 255, c: 'q' };
  let _ = io.println(ctx, t.show(ctx)).ignore();
  .Ok(())
}
"#,
        ),
    );
    assert_eq!(
        out,
        "Tagged { label: \"hi\", live: true, ratio: 0.5, n: -3, small: 255, c: 'q' }\n",
        "stderr was: {err}"
    );
    assert_eq!(code, Some(0));
}

/// A `U8` and an `I8` holding the same byte render differently, which is the
/// whole reason `qualified_key` exists — and the assertion that this backend
/// reads the key rather than the register shape.
#[test]
fn a_derived_show_distinguishes_a_signed_byte_from_an_unsigned_one() {
    skip_unless_executable!();
    let (out, err, code) = build_and_run(
        "derive-sign",
        &program(
            r#"
struct Bytes { signed: I8, unsigned: U8 }
derive Show for Bytes;

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout };
  let _ = io.println(ctx, Bytes { signed: -1, unsigned: 255 }.show(ctx)).ignore();
  .Ok(())
}
"#,
        ),
    );
    assert_eq!(out, "Bytes { signed: -1, unsigned: 255 }\n", "stderr was: {err}");
    assert_eq!(code, Some(0));
}

/// The rest of the `str.*` surface, including the two `Char`-taking entries.
///
/// `padStart` repeats the *whole* fill for `width - len(self)` iterations, so a
/// multi-scalar fill overshoots — a peculiarity of `$str_padStart` rather than
/// a design, reproduced exactly because the conformance suite pins the
/// JavaScript answer (`cli/runtime/text.rs`).
#[test]
fn the_rest_of_the_string_surface_runs() {
    skip_unless_executable!();
    let (out, err, code) = build_and_run(
        "strings2",
        &program(
            r#"
from "core/io" import * as io;
from "core/list" import * as list;
from "core/str" import * as str;

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout };
  let _ = io.println(ctx, "pad [${"abc".padStart(ctx, 5, '.')}][${"abc".padEnd(ctx, 5, '.')}]").ignore();
  let _ = io.println(ctx, "nopad [${"abc".padStart(ctx, 2, '.')}]").ignore();
  let cs = "abc".chars(ctx);
  let _ = io.println(ctx, "chars ${cs.len()}").ignore();
  let _ = io.println(ctx, "from ${str.fromChars(ctx, cs)}").ignore();
  let _ = io.println(ctx, "fromInt ${str.fromInt(ctx, -12)} fromFloat ${str.fromFloat(ctx, 1.25)}").ignore();
  let any = "a1b2c".splitAny(ctx, "12");
  let _ = io.println(ctx, "splitAny ${any.len()} ${any.join(ctx, "-")}").ignore();
  let _ = io.println(ctx, "hashEq ${"ab".hash() == "ab".hash()} ${"ab".hash() == "ba".hash()}").ignore();
  .Ok(())
}
"#,
        ),
    );
    assert_eq!(
        out,
        "pad [..abc][abc..]\nnopad [abc]\nchars 3\nfrom abc\nfromInt -12 fromFloat 1.25\n\
         splitAny 3 a-b-c\nhashEq true false\n",
        "stderr was: {err}"
    );
    assert_eq!(code, Some(0));
}

/// `char.show`, `bool.show` and their `eq`/`compare` siblings.
///
/// `semantics/builtins.rs` declares these on every primitive and
/// `monomorphize::intrinsic_key` names each after the type's own module, so
/// they are `char.` and `bool.` keys rather than the three-segment `num.` ones
/// — one rule, two spellings, and this is the half `numeric_op` does not cover.
/// `show` here is `$str` and not `$show`: a `Char` renders as itself, unquoted.
#[test]
fn the_char_and_bool_leaves_run() {
    skip_unless_executable!();
    let (out, err, code) = build_and_run(
        "leaves",
        &program(
            r#"
from "core/io" import * as io;
from "core/order" import { Order };

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout };
  let _ = io.println(ctx, "show ${'q'.show(ctx)} ${true.show(ctx)} ${false.show(ctx)}").ignore();
  let _ = io.println(ctx, "show ${"raw".show(ctx)}").ignore();
  let _ = io.println(ctx, "eq ${'a'.eq('a')} ${'a'.eq('b')} ${true.eq(false)}").ignore();
  let _ = io.println(ctx, "cmp ${'a'.compare('b') == Order.Less} ${'b'.compare('b') == Order.Equal}").ignore();
  // `$hashInto(SEED, x)`, at every shape `cli/runtime/hash.rs` has an arm for.
  // The numbers are the runtime's, so what is asserted is that equal values
  // agree and unequal ones do not — and that a `Char` and the `U32` with its
  // scalar value do *not*, which is the surrogate-pair arm.
  let _ = io.println(ctx, "hash ${'a'.hash() == 'a'.hash()} ${'a'.hash() == 'b'.hash()}").ignore();
  let _ = io.println(ctx, "hash ${true.hash() == true.hash()} ${(7).hash() == (7).hash()}").ignore();
  let _ = io.println(ctx, "hash ${(1.5).hash() == (1.5).hash()} ${(1.5).hash() == (2.5).hash()}").ignore();
  let _ = io.println(ctx, "wrap ${(1).wrappingAdd(2)} ${(3).wrappingMul(4)} ${(9).wrappingSub(1)}").ignore();
  .Ok(())
}
"#,
        ),
    );
    assert_eq!(
        out,
        "show q true false\nshow raw\neq true false false\ncmp true true\n\
         hash true false\nhash true true\nhash true false\nwrap 3 12 8\n",
        "stderr was: {err}"
    );
    assert_eq!(code, Some(0));
}

/// Every producer that can answer an **empty** result.
///
/// `cli/runtime/list.rs`'s `block` answers a **null** `ptr` for a list of no
/// elements, and `repr.rs` marks a list's `ptr` `Counted::NonNull` — so an
/// empty result from a runtime entry is the one shape where a reference-count
/// operation would read a header at `null - 16`. This is that shape, at every
/// entry that can produce it, and it runs to completion or it segfaults.
#[test]
fn an_empty_result_from_a_runtime_entry_survives_its_counts() {
    skip_unless_executable!();
    let (out, err, code) = build_and_run(
        "empties",
        &program(
            r#"
from "core/io" import * as io;
from "core/list" import * as list;

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout };
  let xs = list.range(ctx, 0, 4);
  let _ = io.println(ctx, "slice ${xs.slice(ctx, 2, 2).len()}").ignore();
  let _ = io.println(ctx, "range ${list.range(ctx, 5, 1).len()}").ignore();
  let _ = io.println(ctx, "repeat ${list.repeat(ctx, 1, 0).len()}").ignore();
  let _ = io.println(ctx, "reverse ${list.range(ctx, 0, 0).reverse(ctx).len()}").ignore();
  let _ = io.println(ctx, "chars ${"".chars(ctx).len()}").ignore();
  let _ = io.println(ctx, "trim [${"   ".trim()}]").ignore();
  let _ = io.println(ctx, "repeat0 [${"ab".repeat(ctx, 0)}]").ignore();
  let _ = io.println(ctx, "slice0 [${"abc".slice(2, 2)}]").ignore();
  .Ok(())
}
"#,
        ),
    );
    assert_eq!(
        out,
        "slice 0\nrange 0\nrepeat 0\nreverse 0\nchars 0\ntrim []\nrepeat0 []\nslice0 []\n",
        "stderr was: {err}"
    );
    assert_eq!(code, Some(0));
}

/// `list.get` at a `[Str]`: the `Ret::Sum` translation and the retain glue at
/// once, since `Option<Str>` takes the niche and its `.Some` is the payload
/// with a non-null `ptr`.
#[test]
fn getting_a_string_out_of_a_list_takes_a_reference() {
    skip_unless_executable!();
    let (out, err, code) = build_and_run(
        "getstr",
        &program(
            r#"
from "core/io" import * as io;
from "core/str" import * as str;

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout };
  let xs = ["a".concat(ctx, "1"), "b".concat(ctx, "2")];
  let _ = match (xs.get(1)) { .Some(s) => io.println(ctx, "got ${s}").ignore(), .None => io.println(ctx, "got none").ignore() };
  let _ = match (xs.get(7)) { .Some(s) => io.println(ctx, "got ${s}").ignore(), .None => io.println(ctx, "got none").ignore() };
  .Ok(())
}
"#,
        ),
    );
    assert_eq!(out, "got b2\ngot none\n", "stderr was: {err}");
    assert_eq!(code, Some(0));
}

/// `derive Hash`, whose primitive leaves are `derivePrimHash.<T>` — the
/// accumulator-taking form, unlike `x.hash()`'s.
///
/// `cli/runtime/hash.rs` is FNV-1a over **UTF-16 code units**, which is the one
/// thing about hashing that cannot be open-coded: `$hashInto` walks a string
/// with `charCodeAt`, so an astral character is two mixes of its surrogate
/// halves and a native hasher that mixed scalars would agree on every ASCII
/// string and differ on every emoji. What is asserted is the algebra — equal
/// values agree, unequal ones do not, and the fields all participate — because
/// the numbers themselves are pinned on the runtime's own side.
#[test]
fn a_derived_hash_reaches_every_primitive_leaf() {
    skip_unless_executable!();
    let (out, err, code) = build_and_run(
        "derive-hash",
        &program(
            r#"
struct Key { label: Str, n: Int, live: Bool, ratio: Float, c: Char, small: U8 }
derive Hash for Key;

fn base(): Key { Key { label: "k", n: 1, live: true, ratio: 0.5, c: 'z', small: 3 } }

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout };
  let a = base();
  let _ = io.println(ctx, "same ${a.hash() == base().hash()}").ignore();
  let _ = io.println(ctx, "str ${a.hash() == Key { label: "j", n: 1, live: true, ratio: 0.5, c: 'z', small: 3 }.hash()}").ignore();
  let _ = io.println(ctx, "int ${a.hash() == Key { label: "k", n: 2, live: true, ratio: 0.5, c: 'z', small: 3 }.hash()}").ignore();
  let _ = io.println(ctx, "bool ${a.hash() == Key { label: "k", n: 1, live: false, ratio: 0.5, c: 'z', small: 3 }.hash()}").ignore();
  let _ = io.println(ctx, "float ${a.hash() == Key { label: "k", n: 1, live: true, ratio: 1.5, c: 'z', small: 3 }.hash()}").ignore();
  let _ = io.println(ctx, "char ${a.hash() == Key { label: "k", n: 1, live: true, ratio: 0.5, c: 'y', small: 3 }.hash()}").ignore();
  let _ = io.println(ctx, "u8 ${a.hash() == Key { label: "k", n: 1, live: true, ratio: 0.5, c: 'z', small: 4 }.hash()}").ignore();
  .Ok(())
}
"#,
        ),
    );
    assert_eq!(
        out,
        "same true\nstr false\nint false\nbool false\nfloat false\nchar false\nu8 false\n",
        "stderr was: {err}"
    );
    assert_eq!(code, Some(0));
}

// ---------------------------------------------------------------------------
// Closures, and the drop glue
// ---------------------------------------------------------------------------

/// A lambda that captures a value, called through the closure it became.
///
/// This is the shape this backend used to refuse. `middle::closures` lifts the
/// lambda to a top-level function whose *first* parameter is a tuple of the
/// captures, and under VALUE-MODEL.md §5.1 an aggregate parameter is its
/// **leaves** — which a call site holding `{ code, env }` cannot produce. So
/// `code` is a generated thunk that takes the environment as a pointer and
/// loads those leaves out of it, which is the convention every native backend
/// here follows.
///
/// Both shapes are here: a capture-free lambda is an `FnRef` by the time it
/// reaches the backend and still gets a thunk, because the call site cannot
/// know which of the two it is holding.
#[test]
fn a_capturing_closure_runs() {
    skip_unless_executable!();
    let (out, err, code) = build_and_run(
        "closure",
        &program(
            r#"
fn apply(f: fn(Int) => Int, x: Int): Int { f(x) }
fn twice(f: fn(Int) => Int, x: Int): Int { f(f(x)) }

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout };
  let n = 10;
  let add = fn(x: Int) => x + n;
  let _ = io.println(ctx, "add ${apply(add, 5)}").ignore();
  let _ = io.println(ctx, "twice ${twice(add, 1)}").ignore();
  // Nothing captured: an `FnRef`, and a null environment.
  let double = fn(x: Int) => x * 2;
  let _ = io.println(ctx, "free ${apply(double, 21)}").ignore();
  // Two captures, so the environment is a two-field record read back as
  // leaves rather than as one word.
  let a = 3;
  let b = 4;
  let both = fn(x: Int) => x * a + b;
  let _ = io.println(ctx, "both ${apply(both, 5)}").ignore();
  .Ok(())
}
"#,
        ),
    );
    assert_eq!(out, "add 15\ntwice 21\nfree 42\nboth 19\n", "stderr was: {err}");
    assert_eq!(code, Some(0));
}

/// A closure over a `Str`, which is the case the environment block's leading
/// glue word exists for.
///
/// `Ty::Fn` records what a function takes and answers and **not** what it
/// captured, so a `decref` of a closure has no type from which to derive the
/// function that releases the environment's contents. The block therefore
/// carries that function pointer in its first word and one universal glue
/// reads it. A closure that captured a `Str` and could not release it is a leak
/// that no amount of type information at the call site would have caught.
#[test]
fn a_closure_capturing_a_string_runs_and_releases_it() {
    skip_unless_executable!();
    let (out, err, code) = build_and_run(
        "closure-str",
        &program(
            r#"
from "core/io" import * as io;
from "core/str" import * as str;

// SPEC 10.6 forbids a lambda from capturing a context, so the allocation
// happens outside it and the capture is the `Str` it produced.
fn run(f: fn(Int) => Str, n: Int): Str { f(n) }

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout };
  let prefix = "pre".concat(ctx, "-x");
  let tag = fn(_n: Int) => prefix;
  let _ = io.println(ctx, run(tag, 1)).ignore();
  let _ = io.println(ctx, run(tag, 2)).ignore();
  .Ok(())
}
"#,
        ),
    );
    assert_eq!(out, "pre-x\npre-x\n", "stderr was: {err}");
    assert_eq!(code, Some(0));
}

/// The drop glue is complete: a block goes back to the allocator only after
/// whatever it held has been released.
///
/// `middle::layout` is the source of truth and the walk is derived from it, so
/// the shapes here are exactly the ones a slot list cannot express — a `[Str]`
/// released element by element with the count recovered from `cap / stride`, a
/// struct holding one, and a **tagged enum** whose payload `repr.rs` keeps as
/// one opaque `Blob` because two variants disagree about what is in it.
///
/// The assertion is a **difference**, not a number: the same program with and
/// without the aggregate must leave the same number of blocks live, because
/// everything the aggregate allocated is everything it should have released.
/// A body that freed the `[Str]`'s block but not its strings fails by two.
///
/// Scoped and not looped, on purpose. `middle::rc` inserts no reference-count
/// operation at all inside a loop body — `churn(ctx, n - 1, ..)` over this same
/// `Row` lowers to a `b5` with three allocations and no `decref` — so a looped
/// version would be measuring that gap rather than this glue.
#[test]
fn a_nested_aggregate_drop_balances_its_counts() {
    skip_unless_executable!();
    let source = |built: &str| {
        program(&format!(
            r#"
from "core/io" import * as io;
from "core/str" import * as str;

struct Row {{ name: Str, tags: [Str] }}
enum Cell {{ Empty, Text(Str), Pair(Str, Str) }}

export fn main(): Result<(), Str> {{
  let ctx = context {{ Alloc: host.alloc, Stdout: host.stdout }};
{built}
  let _ = io.println(ctx, "done ${{n}}").ignore();
  .Ok(())
}}
"#
        ))
    };
    // The two `Str`s inside the list, the list's own block, the `Str` in the
    // struct's other field, and the two inside the enum's payload: six blocks
    // that only a layout-derived walk finds.
    let with = source(
        r#"  let row = Row {
    name: "row".concat(ctx, "!"),
    tags: ["a".concat(ctx, "!"), "b".concat(ctx, "!")],
  };
  let cell = Cell.Pair("p".concat(ctx, "!"), "q".concat(ctx, "!"));
  let held = match (cell) { .Empty => 0, .Text(_t) => 1, .Pair(_a, _b) => 2 };
  let n = row.tags.len() + held;"#,
    );
    let without = source(r#"  let n = 2 + 2;"#);
    let a = build_and_run_with("drop-with", &with, Some(LIVE_PROBE));
    let b = build_and_run_with("drop-without", &without, Some(LIVE_PROBE));
    assert_eq!(a.0, "done 4\n", "stderr was: {}", a.1);
    assert_eq!(b.0, "done 4\n", "stderr was: {}", b.1);
    let (held, base) = (live_blocks(&a.1), live_blocks(&b.1));
    assert_eq!(
        held, base,
        "the drop glue leaks {} block(s): a nested aggregate left {held} live where an \
         equivalent program with no aggregate left {base}",
        held.saturating_sub(base)
    );
}

/// `Bounded`, whose two methods take no `self` and whose type is in the key.
///
/// `middle::lower`'s `bounded_key` qualifies `num.minValue` into
/// `num.<Prim>.minValue` for the reason `qualified_key` qualifies a
/// `derivePrim*`: the *return* type is a bare register shape, and `I64` and
/// `U64` are the same one. The unsigned bounds are the assertion that the key
/// is what is read.
#[test]
fn the_bounded_methods_are_the_types_own_range() {
    skip_unless_executable!();
    let (out, err, code) = build_and_run(
        "bounded",
        &program(
            r#"
from "core/io" import * as io;
from "core/num" import * as num;

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout };
  let _ = io.println(ctx, "u8 ${num.minValue<U8>()} ${num.maxValue<U8>()}").ignore();
  let _ = io.println(ctx, "i8 ${num.minValue<I8>()} ${num.maxValue<I8>()}").ignore();
  let _ = io.println(ctx, "i32 ${num.minValue<I32>()} ${num.maxValue<I32>()}").ignore();
  let _ = io.println(ctx, "u32 ${num.maxValue<U32>()}").ignore();
  .Ok(())
}
"#,
        ),
    );
    assert_eq!(
        out,
        "u8 0 255\ni8 -128 127\ni32 -2147483648 2147483647\nu32 4294967295\n",
        "stderr was: {err}"
    );
    assert_eq!(code, Some(0));
}

/// A **stateful** context, which is what breaks a boundary that drops a
/// context by weighing it rather than by knowing what it is.
///
/// VALUE-MODEL.md §8's "a context of zero-sized implementations is dropped"
/// invites the reading that zero-sizedness is the reason. It is not: a context
/// is dropped at the `buri_rt_*` boundary because the runtime has no use for
/// one — it allocates through `buri_rt_alloc` and reads no capability. The two
/// readings agree for every context built from `core/host`, whose
/// implementations are all empty structs, and they part company at
/// `core/host/testing`, whose `TestAlloc` is `struct TestAlloc(I64)` and
/// carries a handle. Spreading that word into a C signature with no parameter
/// for it shifts every argument after it by one register, which links and runs
/// and answers garbage.
///
/// `padStart` is the entry that catches it: its context sits between `self` and
/// two arguments the answer depends on, so a one-word shift is visible in the
/// output rather than swallowed.
#[test]
fn a_stateful_context_is_dropped_at_the_runtime_boundary() {
    skip_unless_executable!();
    let (out, err, code) = build_and_run(
        "stateful-ctx",
        &program(
            r#"
from "core/effect" import { Region };
from "core/io" import * as io;
from "core/list" import * as list;

/// The shape `core/host/testing`'s `TestAlloc` has: a handle, because Buri
/// has no mutation and the state an allocator names lives elsewhere. Written
/// out here rather than imported, because `core/host/testing` is importable
/// only from a test source and the hazard is the *weight*, not the module.
struct Arena { handle: Int }
impl Alloc for Arena {
  fn allocate(self, bytes: Int): Region { Region(bytes) }
}

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: Arena { handle: 7 }, Stdout: host.stdout };
  let _ = io.println(ctx, "pad [${"abc".padStart(ctx, 6, '.')}]").ignore();
  let _ = io.println(ctx, "cat ${"ab".concat(ctx, "cd")}").ignore();
  let _ = io.println(ctx, "rep ${"xy".repeat(ctx, 3)}").ignore();
  let _ = io.println(ctx, "rep2 ${"a-b".replace(ctx, "-", "+")}").ignore();
  let parts = "1,2,3".split(ctx, ",");
  let _ = io.println(ctx, "split ${parts.len()} ${parts.join(ctx, "|")}").ignore();
  let xs = list.range(ctx, 0, 3).push(ctx, 9);
  let _ = io.println(ctx, "list ${xs.len()}").ignore();
  .Ok(())
}
"#,
        ),
    );
    assert_eq!(
        out,
        "pad [...abc]\ncat abcd\nrep xyxyxy\nrep2 a+b\nsplit 3 1|2|3\nlist 4\n",
        "stderr was: {err}"
    );
    assert_eq!(code, Some(0));
}

/// `core/math`'s nine, and the thirteen that are a named gap.
///
/// The transcendentals are absent on purpose: IEEE 754 fixes `sqrt` and the
/// rounding functions and does **not** fix `sin`, `exp` or `pow`, so V8's
/// fdlibm port and a platform libm differ in the last bit — and a rendered
/// `Float` shows seventeen digits of it. `missing_intrinsics` names them, which
/// is a diagnostic; a libm call would be a conformance failure nobody could
/// attribute to a backend.
#[test]
fn the_exact_half_of_core_math_runs() {
    skip_unless_executable!();
    let (out, err, code) = build_and_run(
        "math",
        &program(
            r#"
from "core/io" import * as io;
from "core/math" import * as math;

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout };
  let _ = io.println(ctx, "sqrt ${math.sqrt(9.0)} abs ${math.absFloat(0.0 - 2.5)}").ignore();
  let _ = io.println(ctx, "floor ${math.floor(1.7)} ceil ${math.ceil(1.2)}").ignore();
  let _ = io.println(ctx, "trunc ${math.trunc(0.0 - 1.7)} round ${math.round(2.5)}").ignore();
  let _ = io.println(ctx, "nan ${math.isNan(0.0)} inf ${math.isInfinite(1.0)} fin ${math.isFinite(1.0)}").ignore();
  .Ok(())
}
"#,
        ),
    );
    assert_eq!(
        out,
        "sqrt 3.0 abs 2.5\nfloor 1.0 ceil 2.0\ntrunc -1.0 round 3.0\n\
         nan false inf false fin true\n",
        "stderr was: {err}"
    );
    assert_eq!(code, Some(0));
}

/// `Checked` and `Saturating`, both bounded by the **type**.
///
/// `Checked` is bounded by the numbers the backend has (SPEC 6.2.2): natively
/// that is the type's own range, so `checkedAdd` reports two's-complement
/// overflow and nothing else, and `(1 << 53).checkedAdd(1)` is `.Some` even
/// though a JavaScript `number` could not name it — the JavaScript backend
/// answers `.None` there, and `cli/tests/native/agreement.rs`'s row 2 pins both.
/// The interesting native case is the one the machine, not the width, decides:
/// `minValue<I64>() / -1` is `2^63`, which has no two's-complement
/// representation, so it is `.None` alongside a zero divisor.
///
/// `saturating*` clamps at the type's own bounds on every backend, and always
/// answers a value.
///
/// The whole family is emitted by widening to 128 bits and range-testing there
/// rather than by `llvm.*.with.overflow`: a 64-bit sum, difference or product is
/// exact in 128 bits, so one widening answers the question directly. The
/// division is the one that needs care — an `sdiv` by zero is undefined
/// behaviour in LLVM even on a dead path, so the divisor is replaced by one
/// with a `select` rather than branched around.
#[test]
fn the_checked_and_saturating_families_are_bounded_by_the_type() {
    skip_unless_executable!();
    let (out, err, code) = build_and_run(
        "checked",
        &program(
            r#"
export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout };
  let say = fn(o: Option<Int>) => match (o) { .Some(v) => "some", .None => "none" };
  let top: Int = 9223372036854775807;
  let bot: Int = 0 - 9223372036854775807;
  let min = bot - 1;
  // The third column of each line is inside the type and above 2^53: `.Some`
  // here, `.None` on JavaScript, and that band is row 2's divergence.
  let _ = io.println(ctx, "add ${say((2).checkedAdd(3))} ${say(top.checkedAdd(1))} ${say((9007199254740991).checkedAdd(1))}").ignore();
  let _ = io.println(ctx, "sub ${say((5).checkedSub(3))} ${say(bot.checkedSub(2))} ${say(bot.checkedSub(1))}").ignore();
  let _ = io.println(ctx, "mul ${say((1000).checkedMul(1000))} ${say((4294967296).checkedMul(4294967296))} ${say((4503599627370496).checkedMul(4))}").ignore();
  // A checked division by zero is `.None`, not SPEC 6.2's abort — and so is
  // the one signed quotient the width cannot hold.
  let _ = io.println(ctx, "div ${say((7).checkedDiv(2))} ${say((7).checkedDiv(0))} ${say(min.checkedDiv(0 - 1))}").ignore();
  let _ = match ((2).checkedAdd(3)) { .Some(v) => io.println(ctx, "value ${v}").ignore(), .None => io.println(ctx, "value none").ignore() };
  // A narrow type is bounded by itself, and always was: at 32 bits and below
  // the type's range and a double's exact range are the same range.
  let small: U8 = 200;
  let _ = io.println(ctx, "u8 ${say2(small.checkedAdd(100))} ${say2(small.checkedAdd(55))}").ignore();
  // `saturating*` clamps at the type's own bounds and always answers a value.
  let _ = io.println(ctx, "sat ${small.saturatingAdd(100)} ${small.saturatingSub(255)}").ignore();
  let big: I8 = 100;
  let _ = io.println(ctx, "sat8 ${big.saturatingAdd(100)} ${big.saturatingMul(0 - 100)}").ignore();
  // 128 bits goes through `buri_rt_i128_checked` and
  // `buri_rt_i128_saturating`: the overflow test both backends use at 64 bits
  // is a widening multiply, which no backend here has at `i128`, so one
  // shared body rather than two hand-rolled ones. It is `i128::checked_mul`,
  // so it is bounded by the type there too.
  let w: I128 = 1000;
  let wide: I128 = 170141183460469231731687303715884105727;
  let _ = io.println(ctx, "i128 ${say3(w.checkedMul(9007199254740991))} ${say3(wide.checkedAdd(1))}").ignore();
  let _ = io.println(ctx, "i128sat ${w.saturatingAdd(1)}").ignore();
  .Ok(())
}

fn say2(o: Option<U8>): Str { match (o) { .Some(_v) => "some", .None => "none" } }
fn say3(o: Option<I128>): Str { match (o) { .Some(_v) => "some", .None => "none" } }
"#,
        ),
    );
    assert_eq!(
        out,
        "add some none some\nsub some none some\nmul some none some\n\
         div some none none\nvalue 5\n\
         u8 none some\nsat 255 0\nsat8 127 -128\ni128 some none\ni128sat 1001\n",
        "stderr was: {err}"
    );
    assert_eq!(code, Some(0));
}

/// `core/bits`, open-coded — fourteen machine operations behind a range check.
///
/// The interesting rows are the two right shifts: `bits.shr` is **logical** and
/// `bits.sar` is arithmetic, which is why `core/bits` names both, and `-8 >> 1`
/// is a very large positive number under one and `-4` under the other. The
/// rotates go through `llvm.fshl`/`llvm.fshr` rather than `(x << n) | (x >> (w
/// - n))`, whose second shift is poison at `n == 0`.
#[test]
fn the_bits_module_runs() {
    skip_unless_executable!();
    let (out, err, code) = build_and_run(
        "bits",
        &program(
            r#"
from "core/bits" import * as bits;
from "core/io" import * as io;

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout };
  let _ = io.println(ctx, "shl ${bits.shl(1, 10)} sar ${bits.sar(0 - 8, 1)}").ignore();
  let _ = io.println(ctx, "shr ${bits.shr(0 - 8, 1)}").ignore();
  let _ = io.println(ctx, "pop ${bits.popCount(255)} lz ${bits.leadingZeros(1)} tz ${bits.trailingZeros(8)}").ignore();
  let _ = io.println(ctx, "zeros ${bits.leadingZeros(0)} ${bits.trailingZeros(0)}").ignore();
  // A rotate by zero is the identity, which is the case a shift-pair spelling
  // gets wrong. `rotateRight(1, 1)` is the sign bit, and `Int` is signed, so it
  // renders negative — `BigInt.asIntN(64, ..)` on the other backend too.
  let _ = io.println(ctx, "rot ${bits.rotateLeft(1, 0)} ${bits.rotateLeft(1, 1)} ${bits.rotateRight(1, 1)}").ignore();
  let b: U8 = 0b1000_0001;
  let _ = io.println(ctx, "u8 ${bits.shlU8(b, 1)} ${bits.shrU8(b, 1)}").ignore();
  let w: U32 = 0x8000_0001;
  let _ = io.println(ctx, "u32 ${bits.shlU32(w, 1)} ${bits.shrU32(w, 31)}").ignore();
  let q: U64 = 3;
  let _ = io.println(ctx, "u64 ${bits.shlU64(q, 2)} ${bits.shrU64(q, 1)}").ignore();
  .Ok(())
}
"#,
        ),
    );
    assert_eq!(
        out,
        "shl 1024 sar -4\nshr 9223372036854775804\npop 8 lz 63 tz 3\nzeros 64 64\n\
         rot 1 2 -9223372036854775808\nu8 2 64\nu32 2 1\nu64 12 1\n",
        "stderr was: {err}"
    );
    assert_eq!(code, Some(0));
}

/// A shift count outside `0 ..< bits` aborts with the runtime's message, which
/// is `$shiftCount`'s (`runtime.js:925`) so that one string is pinned across
/// both backends and JavaScript.
#[test]
fn a_shift_out_of_range_aborts() {
    skip_unless_executable!();
    let (out, err, code) = build_and_run(
        "shift-range",
        &program(
            r#"
from "core/bits" import * as bits;
from "core/io" import * as io;

fn go(n: Int): Int { bits.shl(1, n) }

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout };
  let _ = io.println(ctx, "before").ignore();
  let _ = io.println(ctx, "${go(64)}").ignore();
  .Ok(())
}
"#,
        ),
    );
    assert_eq!(out, "before\n");
    assert!(err.contains("shift out of range"), "stderr was: {err:?}");
    assert_ne!(code, Some(0));
}

// ---------------------------------------------------------------------------
// A projection is not where its base dies
// ---------------------------------------------------------------------------

/// An aggregate holding two counted values, read through its own projections.
///
/// `middle::rc` placed the base's `decref` at the projection that was its last
/// *mention* — `p.b` in `"[${p.a}][${p.b}]"` — and a projection produces three
/// words copied out of the base with **no count of their own**. So the pair was
/// dropped, and with it the two string blocks its fields named, before the
/// `str.concat` chain that reads those words ever ran; `malloc` handed the
/// freed block straight back to the next concatenation, and the program printed
/// zeroed bytes. It is a middle-end fact and not a backend one — every backend
/// prints exactly the same wrong answer — but the whole point of a heap `Str`
/// is that its `base` is not the null a literal carries, so both are here.
///
/// The literal half is the control: a literal's block is immortal, so the same
/// program over `"..."` was *always* right and says nothing about the count.
/// Only the heap half fails, which is what made this shape survive an audit
/// whose differential used single-result functions.
///
/// Both profiles, because the audit reported it at `default<O0>` and
/// `default<O2>` alike, and the live-block count on the way out, because a
/// drop this early is a leak as well as a wrong answer: the incref'd block is
/// freed once by the pair and then never again by the value that still names
/// it.
#[test]
fn an_aggregate_of_counted_values_outlives_its_own_projections() {
    skip_unless_executable!();
    let source = program(
        r#"
from "core/io" import * as io;
from "core/str" import * as str;

struct Pair { a: Str, b: Str }

fn dupTuple(s: Str): (Str, Str) { (s, s) }
fn dupStruct(s: Str): Pair { Pair { a: s, b: s } }
fn twoTuple(a: Str, b: Str): (Str, Str) { (a, b) }

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout };
  let heap = "ab".repeat(ctx, 3);
  let other = "cd".repeat(ctx, 2);

  let dup = dupTuple(heap);
  let _ = io.println(ctx, "tuple [${dup.0}][${dup.1}]").ignore();

  let rec = dupStruct(heap);
  let _ = io.println(ctx, "struct [${rec.a}][${rec.b}]").ignore();

  let two = twoTuple(heap, other);
  let _ = io.println(ctx, "two [${two.0}][${two.1}]").ignore();

  let here = (heap, heap);
  let _ = io.println(ctx, "local [${here.0}][${here.1}]").ignore();

  let lit = dupTuple("zz");
  let _ = io.println(ctx, "literal [${lit.0}][${lit.1}]").ignore();

  let one = (heap, 1);
  let _ = io.println(ctx, "one [${one.0}]").ignore();
  .Ok(())
}
"#,
    );
    let expected = "tuple [ababab][ababab]\nstruct [ababab][ababab]\n\
                    two [ababab][cdcd]\nlocal [ababab][ababab]\n\
                    literal [zz][zz]\none [ababab]\n";

    let fast = build_and_run_at("aggregate-projection-o2", &source, Some(LIVE_PROBE), Profile::Release);
    let plain = build_and_run_at("aggregate-projection-o0", &source, Some(LIVE_PROBE), Profile::Debug);

    assert_eq!(fast.0, expected, "stderr was: {}", fast.1);
    assert_eq!(plain.0, expected, "stderr was: {}", plain.1);
    assert_eq!(fast.2, Some(0), "stderr was: {}", fast.1);
    assert_eq!(plain.2, Some(0), "stderr was: {}", plain.1);
    assert_eq!(live_blocks(&fast.1), 0, "`default<O2>` left blocks live: {:?}", fast.1);
    assert_eq!(live_blocks(&plain.1), 0, "`default<O0>` left blocks live: {:?}", plain.1);
}

/// The **class**, rather than the shape the report arrived in: every borrowing
/// projection, wherever the value it produces is read.
///
/// `Field`, `TupleIndex`, `CtxGet` and `Index` all read a base without taking
/// it, and all four had the same drop placement. `xs[i]` as a `match`
/// scrutinee was the same bug as `p.a` in a template, except that there it was
/// a segmentation fault rather than a wrong answer — the arms read a payload
/// out of a list block the scrutinee's own drop had already freed.
///
/// Four rows, and the fourth is a back edge: an aggregate of two counted values
/// carried around a tail-recursive loop, projected on every iteration, is where
/// a drop placed one instruction early would be a use-after-free a thousand
/// times over rather than once.
#[test]
fn a_borrowing_projection_does_not_end_its_bases_lifetime() {
    skip_unless_executable!();
    let source = program(
        r#"
from "core/io" import * as io;
from "core/list" import * as list;
from "core/str" import * as str;

struct Held { name: Str, tag: Str }

fn hold(a: Str, b: Str): Held { Held { name: a, tag: b } }

// A pair carried around a back edge, projected on every iteration.
fn spin(n: Int, p: (Str, Str)): Str {
  if (n == 0) { p.0 } else { spin(n - 1, (p.1, p.0)) }
}

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout };

  // 1. A field projection through a function's return value.
  let held = hold("ab".repeat(ctx, 2), "cd".repeat(ctx, 2));
  let _ = io.println(ctx, "field [${held.name}][${held.tag}]").ignore();

  // 2. An index whose base's last use it is, as a `match` scrutinee. This was
  //    a segmentation fault: the arms read a payload out of a freed block.
  let xs = ["ef".repeat(ctx, 2)];
  let got = match (xs[0]) { .Some(v) => v, .None => "?" };
  let _ = io.println(ctx, "index [${got}]").ignore();

  // 3. A tuple projection under `match`, where the scrutinee is the projection
  //    rather than a bare local.
  let pair = ("gh".repeat(ctx, 2), "ij".repeat(ctx, 2));
  let picked = match (pair.0.toInt()) { .Some(_n) => "number", .None => pair.1 };
  let _ = io.println(ctx, "match [${picked}]").ignore();

  // 4. A back edge.
  let _ = io.println(ctx, "loop [${spin(5, ("kl".repeat(ctx, 2), "mn".repeat(ctx, 2)))}]").ignore();
  .Ok(())
}
"#,
    );
    let expected =
        "field [abab][cdcd]\nindex [efef]\nmatch [ijij]\nloop [mnmn]\n";

    let fast = build_and_run_at("projection-class-o2", &source, Some(LIVE_PROBE), Profile::Release);
    let plain = build_and_run_at("projection-class-o0", &source, Some(LIVE_PROBE), Profile::Debug);

    assert_eq!(fast.0, expected, "stderr was: {}", fast.1);
    assert_eq!(plain.0, expected, "stderr was: {}", plain.1);
    assert_eq!(fast.2, Some(0), "stderr was: {}", fast.1);
    assert_eq!(live_blocks(&fast.1), 0, "`default<O2>` left blocks live: {:?}", fast.1);
    assert_eq!(live_blocks(&plain.1), 0, "`default<O0>` left blocks live: {:?}", plain.1);
}

/// The join `middle::lower::template` builds is nobody else's to drop.
///
/// Every `Show` result and every intermediate `str.concat` in an interpolation
/// is a value `lower` invents; `middle::rc` plans over the *tree* and has no
/// `NodeId` to name one with. So until `template` dropped them itself, a
/// program that interpolated in a loop grew the heap by a block an iteration
/// — forever, in a language whose whole memory story is that it does not — and
/// `Prim::Template` was `Answer::Unknown` in `rc`'s classifier, so the block the
/// chain *ended* holding leaked once per evaluation on top of that.
///
/// Counted rather than asserted against zero at one size: a per-iteration leak
/// is what this is about, so twenty iterations and two hundred have to leave
/// the same number of blocks live.
#[test]
fn interpolating_in_a_loop_leaks_nothing() {
    skip_unless_executable!();
    let source = |n: u32| {
        program(&format!(
            r#"
from "core/io" import * as io;
from "core/str" import * as str;

fn go(n: Int, acc: Int): Int {{
  if (n <= 0) {{ acc }} else {{
    let h = "ab".repeat(host.alloc, 3);
    let p = (h, h);
    let s = str.format(host.alloc, "[${{p.0}}][${{p.1}}]");
    let _ = io.println(host.stdout, "${{n}}").ignore();
    go(n - 1, acc + s.len())
  }}
}}

export fn main(): Result<(), Str> {{
  let ctx = context {{ Alloc: host.alloc, Stdout: host.stdout }};
  let _ = io.println(ctx, "total ${{go({n}, 0)}}").ignore();
  .Ok(())
}}
"#
        ))
    };
    let few = build_and_run_with("template-leak-few", &source(20), Some(LIVE_PROBE));
    let many = build_and_run_with("template-leak-many", &source(200), Some(LIVE_PROBE));
    assert!(few.0.ends_with("total 320\n"), "stdout was: {:?}", few.0);
    assert!(many.0.ends_with("total 3200\n"), "stdout was: {:?}", many.0);
    assert_eq!(
        live_blocks(&few.1),
        live_blocks(&many.1),
        "twenty interpolations left {} blocks live and two hundred left {}: \
         the join leaks per iteration",
        live_blocks(&few.1),
        live_blocks(&many.1)
    );
    assert_eq!(live_blocks(&many.1), 0, "the heap did not come back balanced: {:?}", many.1);
}

/// A `C: Alloc` parameter reached with a value that **implements** `Alloc`
/// rather than with a `context { … }` record — the shape SPEC 10.8's
/// attenuation is made of, and the one a native ABI rule used to get wrong.
///
/// `<C: Alloc>` and `<T: Ord>` are one feature (SPEC 10.1), so the argument at
/// `C` need not be a context, and `Tagged` here is a plain struct that forwards
/// `allocate` and carries a word of its own so that it is **not** zero-sized.
///
/// Both backends drop a runtime call's context argument, because `cli/runtime`
/// allocates through `buri_rt_alloc` and has no use for one. Which argument
/// that is was asked of the *value's type* — "is it a `Ty::Ctx`?" — which is the
/// same answer only while every `C` is instantiated at a context. A `Tagged`
/// spread to a leaf the C signature has no parameter for, and every argument
/// after it moved one register down.
///
/// Two operations, because the two backends got it wrong in two places:
///
/// * `list.push` is a [`runtime_table::ENTRIES`] row, and its context is named
///   by `Entry::ctx` / `Arg::Dropped` at index one. This is the one that
///   segfaulted in `memmove` under the stencil backend.
/// * `str.concat` has **no** row on either side, so its argument list is
///   narrowed at the call site instead — `emit.rs`'s `concat_ctx`. This one is
///   only reachable here: the conformance corpus covers the stencil backend
///   (`semantics/effects.buri`'s "a bare implementing value" blocks) and
///   `native/conformance.rs` drives that generator, so this file is where the
///   LLVM half of the same rule is exercised at all.
///
/// A wrong answer here is not a refusal — it links and runs — so the assertion
/// is on the two values, not on the exit code.
#[test]
fn a_value_that_implements_alloc_is_a_context_bounds_argument() {
    skip_unless_executable!();
    let (out, err, code) = build_and_run(
        "bare-implementor",
        &program(
            r#"
from "core/effect" import { Region };
from "core/io" import * as io;
from "core/list" import * as list;
from "core/str" import * as str;

/// Satisfies `Alloc` by forwarding, and is not a context. The `tag` is what
/// makes it eight bytes wide: a zero-sized implementor would spread to no
/// leaves and the two readings would agree by accident.
struct Tagged<C> {
  export inner: C,
  export tag: Int,
}

impl<C: Alloc> Alloc for Tagged<C> {
  fn allocate(self, bytes: Int): Region {
    self.inner.allocate(bytes)
  }
}

/// The context is argument **one**, after the receiver.
fn pushed<C: Alloc>(ctx: C, item: Int, into: [Int]): [Int] {
  into.push(ctx, item)
}

/// The context is argument **zero**, so both indices the rule can take are
/// reached from one program.
fn repeated<C: Alloc>(ctx: C, item: Int): [Int] {
  list.repeat(ctx, item, 2)
}

/// No table row on either backend.
fn joined<C: Alloc>(ctx: C, a: Str, b: Str): Str {
  a.concat(ctx, b)
}

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout };
  let bare = Tagged { inner: ctx, tag: 9 };
  let pushedOnto = pushed(bare, 7, [1]);
  let twice = repeated(bare, 5);
  let _ = io.println(ctx, str.format(
    ctx,
    "${pushedOnto[0] ?? 0} ${pushedOnto[1] ?? 0} ${twice[1] ?? 0} ${joined(bare, "ab", "cd")}",
  )).ignore();
  // And the context record built from the same implementations, which is what
  // every other program here passes: one answer, two argument types.
  let alsoPushed = pushed(ctx, 7, [1]);
  let _ = io.println(ctx, "${alsoPushed[1] ?? 0} ${joined(ctx, "ab", "cd")}").ignore();
  .Ok(())
}
"#,
        ),
    );
    assert_eq!(out, "1 7 5 abcd\n7 abcd\n", "stderr was: {err}");
    assert_eq!(code, Some(0));
}

/// **A Buri server answers a real request, through this pipeline too.**
///
/// `stencil.rs`'s row of the same name, one backend over — and the reason it is
/// written twice rather than shared is that the two pipelines lay the same
/// values out and the assertion is about the bytes. `effect Listen`'s four
/// entries cross the C ABI with a `Request` written by the runtime and a
/// `Response` read back out of one, and each backend decides for itself where
/// `.Ok`'s payload sits in a `Result` and where a `Str` sits in a struct. One
/// green row says nothing about the other.
///
/// Every wait is bounded and the client is joined —
/// `shared::request_when_ready` is where that is argued.
#[test]
fn a_server_answers_a_request_on_a_socket() {
    skip_unless_executable!();
    let source = crate::shared::one_shot_server();
    let binary = build_at("server-socket", &source, None, Profile::Release);
    let (out, reply) = crate::shared::served(&binary, "/ping?x=1");
    assert_eq!(out.status, 0, "stdout:\n{}\nstderr:\n{}", out.stdout, out.stderr);
    assert!(
        out.stdout.ends_with("served\n"),
        "the server did not run to its own end:\n{}",
        out.stdout
    );
    crate::shared::served_the_path(&reply, "/ping");
}

/// **A `Server`'s `tls` field reaches the acceptor, at this backend's layout.**
///
/// The stencil row's twin, and written twice for the reason every server row in
/// this repository is: each backend decides for itself where a payload sits
/// inside an enum and where a `Str` sits inside a struct, so one green row says
/// nothing about the other. What F4 added to the C ABI is a payload-carrying
/// enum in a list — `listenBind`'s plan is a `[Serve]` — and a backend that
/// disagreed about its 32-byte stride would hand the acceptor a certificate
/// path read out of the middle of another element.
///
/// The three claims and why each of them is one are in `shared::tls_server`.
#[test]
fn a_secured_server_opens_its_port_and_says_why_when_it_cannot() {
    skip_unless_executable!();
    let (certificate, key, absent) = crate::shared::tls_identity("llvm");
    let source = crate::shared::tls_server(&certificate, &key, &absent);
    let binary = build_at("server-tls", &source, None, Profile::Release);
    let out = crate::shared::ran(&binary);
    assert_eq!(out.status, 0, "stdout:\n{}\nstderr:\n{}", out.stdout, out.stderr);
    crate::shared::tls_bind_answers(&out.stdout, &absent);
}

/// **Fifty requests at once, against a handler that sleeps** — F3, end to end.
///
/// `run` fans its accept loop out with `Tasks.parallel`, one worker per handler
/// the acceptor says it will host, so the fifty exchanges below overlap. The
/// assertion is the clock and it is a wide one on purpose: fifty two-hundred
/// millisecond sleeps one after another is ten seconds, the acceptor's
/// sixty-four workers running them together is a little over two hundred
/// milliseconds, and four seconds is a line no loaded machine crosses from the
/// fast side and none stays under from the slow one.
///
/// **This row is the LLVM backend's alone**, and that is a fact about the
/// artifact rather than a gap. `rt.rs` fans a `parallel` out only where the
/// artifact called `buri_rt_frames_are_per_carrier`, which the frame-threaded
/// backend deliberately does not (`stencil/asm.rs` asserts it never emits the
/// call): a program built there has one Buri stack, so its workers run one after
/// another and fifty sleeps take fifty sleeps. The stencil row beside this one
/// asserts what *is* true there — every request answered, each with its own
/// path — and says nothing about the clock.
///
/// The pairing is asserted beside the timing, because a fast server that
/// answered the wrong client would be a worse failure than a slow one:
/// `each_answered_its_own` is the ordering-independence half.
#[test]
fn fifty_requests_are_answered_at_once() {
    skip_unless_executable!();
    const REQUESTS: usize = 50;
    const SLEEP_MILLIS: usize = 200;
    let source = crate::shared::concurrent_server(REQUESTS, SLEEP_MILLIS);
    let binary = build_at("server-concurrent", &source, None, Profile::Release);
    let (out, replies, elapsed) = crate::shared::served_many(&binary, REQUESTS);
    assert_eq!(out.status, 0, "stdout:\n{}\nstderr:\n{}", out.stdout, out.stderr);
    assert!(
        out.stdout.ends_with("served\n"),
        "the server did not run to its own end:\n{}",
        out.stdout
    );
    assert_eq!(replies.len(), REQUESTS);
    crate::shared::each_answered_its_own(&replies);
    assert!(
        elapsed < std::time::Duration::from_secs(4),
        "{REQUESTS} requests against a {SLEEP_MILLIS} ms handler took {elapsed:?}, which is \
         one at a time rather than {REQUESTS} at once"
    );
}
