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

/// This machine's `variant` for a native output.
fn host_variant() -> String {
    let os = if cfg!(target_os = "macos") { "macos" } else { "linux" };
    let arch = if cfg!(target_arch = "aarch64") { "arm64" } else { "x86_64" };
    format!("{os}-{arch}")
}

/// `buri build` of one binary for `platform`, `native` or `node`, whose source
/// is `items` and a `main` running `body`. Answers `phase`'s instructions.
fn profiled(platform: &str, items: &str, body: &str, phase: &str) -> Option<f64> {
    let scratch = Scratch::repo("profile-large-shape");
    let output = match platform {
        "native" => format!("{{ platform: \"native\", variant: \"{}\" }}", host_variant()),
        _ => format!("{{ platform: \"{platform}\" }}"),
    };
    let host = if platform == "native" { "NativeHost" } else { "NodeHost" };
    scratch.write("app/BUILD.buri", &format!("binary {{\n    outputs: [\n        {output},\n    ]\n}}\n"));
    scratch.write(
        "app/main.buri",
        &format!(
            "from \"platform/effect\" import {{ Allocator, Stdout }};\n\
             from \"{platform}\" import {{ {host} }};\n\
             from \"core/io\" import * as io;\n\n{items}\n\
             export fn main(host: {host}): Result<(), Str> {{\n    \
             let ctx = context {{ Allocator: host.alloc, Stdout: host.stdout }};\n{body}    .Ok(())\n}}\n"
        ),
    );
    let run = scratch.run_with_env(&["build", "//app"], &[("BURI_PROFILE", "1")]);
    run.ok();
    phase_instructions(&run.all(), phase)
}

/// Asserts that `work(2n)` is at most 2.3 times `work(n)`. Where the platform
/// has no counter, there is nothing to compare and this asserts nothing.
fn grows_linearly(what: &str, work: impl Fn(usize) -> Option<f64>, n: usize) {
    let (Some(small), Some(large)) = (work(n), work(2 * n)) else {
        return;
    };
    assert!(
        large <= small * 2.3,
        "{what}: {large:.0} M instructions at {}, {:.2} times the {small:.0} M at {n}",
        2 * n,
        large / small
    );
}

/// Builds tuples of `n` elements, takes one apart and compares two.
fn long_tuple(n: usize) -> (String, String) {
    let types: Vec<&str> = (0..n).map(|i| if i % 2 == 0 { "Int" } else { "Str" }).collect();
    let values: Vec<String> =
        (0..n).map(|i| if i % 2 == 0 { format!("k + {i}") } else { String::from("s") }).collect();
    let names: Vec<String> = (0..n).map(|i| format!("a{i}")).collect();
    let summed: Vec<String> = (0..n)
        .map(|i| if i % 2 == 0 { format!("a{i}") } else { format!("a{i}.length()") })
        .collect();
    let items = format!(
        "fn make<C: Allocator>(ctx: C, k: Int): ({types}) {{\n    \
         let s = \"u\".repeat(ctx, 2);\n    ({values})\n}}\n\n\
         fn sum(t: ({types})): Int {{\n    let ({names}) = t;\n    {summed}\n}}\n",
        types = types.join(", "),
        values = values.join(", "),
        names = names.join(", "),
        summed = summed.join(" + "),
    );
    let body = String::from(
        "    let (a, b) = (make(ctx, 1), make(ctx, 2));\n    \
         let _ = io.println(ctx, \"${sum(a)} ${a == b} ${a < b}\").ignore();\n",
    );
    (items, body)
}

/// An enum of `n` variants, and a match of `n` arms over it, over an `Int` and
/// over a `Str`. With `pairs`, a match over a pair of them as well, whose arms
/// are the diagonal.
fn long_matches(n: usize, pairs: bool) -> (String, String) {
    let variants: String = (0..n).map(|i| format!("    V{i},\n")).collect();
    let from_int: String = (0..n - 1).map(|i| format!("        {i} => .V{i},\n")).collect();
    let to_int: String = (0..n).map(|i| format!("        .V{i} => {i},\n")).collect();
    let words: String = (0..n).map(|i| format!("        \"w{i}\" => {i},\n")).collect();
    let diagonal: String = (0..n).map(|i| format!("        (.V{i}, .V{i}) => {i},\n")).collect();
    let mut items = format!(
        "enum E {{\n{variants}}}\n\n\
         fn nth(i: Int): E {{\n    match (i) {{\n{from_int}        _ => .V{},\n    }}\n}}\n\n\
         fn code(e: E): Int {{\n    match (e) {{\n{to_int}    }}\n}}\n\n\
         fn word(w: Str): Int {{\n    match (w) {{\n{words}        _ => 0 - 1,\n    }}\n}}\n",
        n - 1
    );
    let mut body = String::from(
        "    let w = word(\"w2\");\n    let _ = io.println(ctx, \"${code(nth(3))} ${w}\").ignore();\n",
    );
    if pairs {
        items.push_str(&format!(
            "\nfn same(a: E, b: E): Int {{\n    match ((a, b)) {{\n{diagonal}        _ => 0 - 1,\n    }}\n}}\n"
        ));
        body.push_str("    let _ = io.println(ctx, \"${same(nth(1), nth(1))}\").ignore();\n");
    }
    (items, body)
}

/// **A debug build's emission is linear in the length of a function.** The
/// copy-and-patch backend asked, for every branch it might fuse with the
/// comparison before it, how often the compared value is used, and answered by
/// reading the whole function: a function of `n` instructions took `n²`. A
/// tuple of 400 elements built and taken apart took 1.24 G instructions to
/// emit, 3.5 times what 200 took. PERFORMANCE.md §6.32.
///
/// An instruction count rather than a time: one run reads to within about 1%
/// under any load (PERFORMANCE.md §8).
#[test]
fn a_debug_builds_emission_is_linear_in_a_functions_length() {
    if let Some(why) = ci::native_host_gap() {
        ci::skipped("build::profile", &why);
        return;
    }
    grows_linearly(
        "emitting a long tuple's functions",
        |n| {
            let (items, body) = long_tuple(n);
            profiled("native", &items, &body, "emit")
        },
        300,
    );
}

/// **JavaScript emission is linear in a match's arms.** A match is a chain of
/// tests, and the folder that turns each `if`/`else` of returns into a
/// conditional copied the rest of the chain at every arm.
#[test]
fn javascript_emission_is_linear_in_a_matchs_arms() {
    grows_linearly(
        "emitting long matches as JavaScript",
        |n| {
            let (items, body) = long_matches(n, false);
            profiled("node", &items, &body, "emit")
        },
        500,
    );
}

/// **Checking a match is linear in its arms.** Each arm of a match over a
/// pair is asked whether the arms before it already cover it.
#[test]
fn checking_a_match_over_pairs_is_linear_in_its_arms() {
    grows_linearly(
        "checking a match over pairs",
        |n| {
            let (items, body) = long_matches(n, true);
            profiled("node", &items, &body, "check")
        },
        500,
    );
}

/// **Checking a pattern is linear in the names it binds.** A `let` asked
/// whether the whole value could carry an effect once per name it binds, a
/// walk of the value's type each time: a tuple of 2,000 elements taken apart
/// took 1.2 G instructions to check.
#[test]
fn checking_a_pattern_is_linear_in_the_names_it_binds() {
    grows_linearly(
        "checking a long tuple's functions",
        |n| {
            let (items, body) = long_tuple(n);
            profiled("node", &items, &body, "check")
        },
        1000,
    );
}
