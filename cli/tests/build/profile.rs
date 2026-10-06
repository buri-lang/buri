//! `BURI_PROFILE=1`: one run says where its work went, by phase.
use crate::harness::*;

fn repo() -> Scratch {
    let scratch = Scratch::repo("profile");
    scratch.write("lib/a/BUILD.buri", "library {\n  test { sources: [\"test/a.buri\"] }\n}\n");
    scratch.write("lib/a/lib.buri", "export fn answer(): I64 { 42 }\n");
    scratch.write(
        "lib/a/test/a.buri",
        "from \"//lib/a\" import { answer };\n\
         from \"core/testing/assert\" import * as assert;\n\
         \ntest \"a answers\" {\n  assert.equal(answer(), 42);\n}\n",
    );
    scratch
}

/// A native `buri test` passes through every phase, and the report names each.
#[test]
fn a_profiled_test_run_names_every_phase_it_went_through() {
    let scratch = repo();
    let run = scratch.run_with_env(&["test", "//..."], &[("BURI_PROFILE", "1")]);
    run.ok().says("buri profile");
    for phase in ["lex+parse", "check", "monomorphize", "middle", "emit", "link", "run", "all phases"] {
        run.says(&format!("\n  {phase} "));
    }
    assert_eq!(run.tests_passed(), 1, "the suite did not run:\n{}", indent(&run.all()));
}

#[test]
fn an_unprofiled_run_prints_no_profile() {
    let scratch = repo();
    scratch.run(&["test", "//..."]).ok().silent_about("buri profile");
    scratch.run_with_env(&["test", "//..."], &[("BURI_PROFILE", "0")]).ok().silent_about("buri profile");
}

/// The instructions one phase took, in millions, off a `BURI_PROFILE=1`
/// report, or `None` where the platform has no per-thread counter (Linux, and
/// CI's macOS virtual machines, read `-` or 0).
fn phase_instructions(report: &str, phase: &str) -> Option<f64> {
    let line = report.lines().find(|l| l.trim_start().starts_with(&format!("{phase} ")))?;
    let count = line.split_whitespace().nth(1)?.parse::<f64>().ok()?;
    (count > 0.0).then_some(count)
}

/// One binary whose `main` builds tuples of `n` elements, takes one apart and
/// compares two.
fn long_function(n: usize) -> Scratch {
    let scratch = Scratch::repo("profile-long-function");
    let types: Vec<&str> = (0..n).map(|i| if i % 2 == 0 { "Int" } else { "Str" }).collect();
    let values: Vec<String> =
        (0..n).map(|i| if i % 2 == 0 { format!("k + {i}") } else { String::from("s") }).collect();
    let names: Vec<String> = (0..n).map(|i| format!("a{i}")).collect();
    let summed: Vec<String> = (0..n)
        .map(|i| if i % 2 == 0 { format!("a{i}") } else { format!("a{i}.length()") })
        .collect();
    scratch.write(
        "app/BUILD.buri",
        &format!(
            "binary {{\n    outputs: [\n        {{ platform: \"native\", variant: \"{}\" }},\n    ]\n}}\n",
            host_variant()
        ),
    );
    scratch.write(
        "app/main.buri",
        &format!(
            "from \"platform/effect\" import {{ Allocator, Stdout }};\n\
             from \"native\" import {{ NativeHost }};\n\
             from \"core/io\" import * as io;\n\n\
             fn make<C: Allocator>(ctx: C, k: Int): ({types}) {{\n    \
             let s = \"u\".repeat(ctx, 2);\n    ({values})\n}}\n\n\
             fn sum(t: ({types})): Int {{\n    let ({names}) = t;\n    {summed}\n}}\n\n\
             export fn main(host: NativeHost): Result<(), Str> {{\n    \
             let ctx = context {{ Allocator: host.alloc, Stdout: host.stdout }};\n    \
             let (a, b) = (make(ctx, 1), make(ctx, 2));\n    \
             let _ = io.println(ctx, \"${{sum(a)}} ${{a == b}} ${{a < b}}\").ignore();\n    .Ok(())\n}}\n",
            types = types.join(", "),
            values = values.join(", "),
            names = names.join(", "),
            summed = summed.join(" + "),
        ),
    );
    scratch
}

/// This machine's `variant` for a native output.
fn host_variant() -> String {
    let os = if cfg!(target_os = "macos") { "macos" } else { "linux" };
    let arch = if cfg!(target_arch = "aarch64") { "arm64" } else { "x86_64" };
    format!("{os}-{arch}")
}

/// **A debug build's emission is linear in the length of a function.** The
/// copy-and-patch backend asked, for every branch it might fuse with the
/// comparison before it, how often the compared value is used, and answered by
/// reading the whole function: a function of `n` instructions took `n²`. A
/// tuple of 400 elements built and taken apart took 1.24 G instructions to
/// emit, 3.5 times what 200 took. PERFORMANCE.md §6.32.
///
/// An instruction count rather than a time: one run reads to within about 1%
/// under any load (PERFORMANCE.md §8). Where the platform has no counter, the
/// report has no number and this asserts nothing.
#[test]
fn a_debug_builds_emission_is_linear_in_a_functions_length() {
    if let Some(why) = ci::native_host_gap() {
        ci::skipped("build::profile", &why);
        return;
    }
    let emitted = |n: usize| {
        let run = long_function(n).run_with_env(&["build", "//app"], &[("BURI_PROFILE", "1")]);
        run.ok();
        phase_instructions(&run.all(), "emit")
    };
    let (Some(small), Some(large)) = (emitted(300), emitted(600)) else {
        return;
    };
    assert!(
        large <= small * 2.3,
        "emitting a tuple of 600 elements took {large:.0} M instructions, {:.2} times the \
         {small:.0} M that 300 took",
        large / small
    );
}
