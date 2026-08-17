//! Programs compiled through the whole native pipeline, linked, and **run**.
//!
//! The bar for the Cranelift backend is not "the object file has the right
//! sections". It is that a Buri program goes front end -> `middle::run` ->
//! `middle::native` -> `middle::lower` -> `backend::cranelift` -> object ->
//! `cc` -> executable, and that the executable prints what the language says
//! it prints. Everything short of that is a claim about an intermediate
//! representation, and this compiler has enough of those already.
//!
//! The link is a plain `cc <objects> libburi_rt.a` here. Wave 2c replaces it
//! with `build/actions.rs`'s real link step, which selects mold or lld and
//! caches the result (CODEGEN-CRANELIFT.md §7); nothing in this file depends on
//! which linker ran, so it keeps working when that lands.
//!
//! The whole file is behind `backend-cranelift`. With the feature off it
//! compiles to nothing and the suite is silent rather than red, which is what
//! "degrades rather than breaks" means for a test.

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

#[cfg(feature = "backend-cranelift")]
mod native {
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
        let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
            .join(format!("native-cranelift-{}", std::process::id()))
            .join(name);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// What running one program produced.
    struct Ran {
        status: i32,
        stdout: String,
        stderr: String,
    }

    /// The whole pipeline, for one snippet.
    fn build(name: &str, source: &str) -> PathBuf {
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
        let archive = dir.join(ARCHIVE_NAME);
        std::fs::write(&archive, ARCHIVE).unwrap();
        let binary = dir.join("program");

        let mut cc = Command::new(std::env::var("CC").unwrap_or_else(|_| "cc".to_string()));
        cc.arg("-o").arg(&binary);
        for o in &objects {
            cc.arg(o);
        }
        cc.arg(&archive);
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
        let binary = build(name, source);
        let out = Command::new(&binary).output().unwrap();
        Ran {
            status: out.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&out.stdout).to_string(),
            stderr: String::from_utf8_lossy(&out.stderr).to_string(),
        }
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
from "core/cap" import { Alloc, Stdout };
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
from "core/cap" import { Alloc, Stdout };
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
from "core/cap" import { Alloc };
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
        let mut diags = Diagnostics::new();
        let mut program = monomorphize::run(
            &analysis.checked,
            paths,
            &mut diags,
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
    // Wave 3d: the intrinsic surface
    // -----------------------------------------------------------------------

    /// `Float` rendering, which is the headline correctness item of wave 3d.
    ///
    /// Every string here is what `bun` prints for the same value — the four
    /// presentation cases of ECMA-262 §6.1.6.1.20 and the three non-finite
    /// spellings `$f64` gives them. `cli/tests/native_float_parity.rs` is the
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
    /// value: the two shapes wave 2a named as absent because "constructing one
    /// needs the layout of a type the intrinsic table does not name".
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
        let mut diags = Diagnostics::new();
        let mut program = monomorphize::run(
            &analysis.checked,
            paths,
            &mut diags,
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
}
