//! `BURI_PROFILE=1`: one run says where its work went, by phase.
use crate::harness::*;

fn repo() -> Scratch {
    let scratch = Scratch::repo("profile");
    scratch.write(
        "lib/a/BUILD.buri",
        "library {\n  test { sources: [\"test/a.buri\"] }\n}\n",
    );
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
    for phase in [
        "lex+parse",
        "check",
        "monomorphize",
        "middle",
        "emit",
        "link",
        "run",
        "all phases",
    ] {
        run.says(&format!("\n  {phase} "));
    }
    assert_eq!(
        run.tests_passed(),
        1,
        "the suite did not run:\n{}",
        indent(&run.all())
    );
}

/// **A phase that waited for no child reads `-` for child instructions**,
/// rather than a 0 that reads as a child that ran none. Only macOS counts
/// children at all.
#[test]
fn a_phase_that_waited_for_no_child_counts_no_child_instructions() {
    let scratch = repo();
    let run = scratch.run_with_env(&["test", "//..."], &[("BURI_PROFILE", "1")]);
    run.ok();
    let report = run.all();
    if !report.contains("child instructions") {
        return;
    }
    let row = report
        .lines()
        .find(|l| l.trim_start().starts_with("check "))
        .unwrap_or_else(|| panic!("no `check` row in\n{}", indent(&report)));
    let words: Vec<&str> = row.split_whitespace().collect();
    assert_eq!(
        words[words.len() - 3..],
        ["-", "0.000", "s"],
        "{}",
        indent(&report)
    );
}

#[test]
fn an_unprofiled_run_prints_no_profile() {
    let scratch = repo();
    scratch
        .run(&["test", "//..."])
        .ok()
        .silent_about("buri profile")
        .silent_about("suites built");
    scratch
        .run_with_env(&["test", "//..."], &[("BURI_PROFILE", "0")])
        .ok()
        .silent_about("buri profile");
}

/// **A toolchain built with `--features alloc-counter` counts each phase's
/// allocations**, in a column of its own, and one built without it has no such
/// column. The coverage job's release `buri` is the first kind and the suite's
/// the second.
#[test]
fn an_allocation_counting_toolchain_reports_its_allocations() {
    let scratch = repo();
    let run = scratch.run_with_env(&["test", "//..."], &[("BURI_PROFILE", "1")]);
    run.ok().says("buri profile");
    let report = run.all();
    let header = report
        .lines()
        .find(|l| l.starts_with("buri profile"))
        .unwrap();
    if !cfg!(feature = "alloc-counter") {
        assert!(!header.contains("allocations"), "{}", indent(&report));
        return;
    }
    assert!(header.contains("allocations"), "{}", indent(&report));
    for phase in ["check", "all phases"] {
        let row = report
            .lines()
            .find(|l| l.trim_start().starts_with(&format!("{phase} ")))
            .unwrap();
        // After the name: instructions (`-` or `<n> M`), cpu `<n> s`, busy
        // `<n> s`, then the allocations.
        let words: Vec<&str> = row[row.find(phase).unwrap() + phase.len()..]
            .split_whitespace()
            .collect();
        let busy = words.iter().enumerate().filter(|(_, w)| **w == "s").nth(1);
        let allocations: u64 = words[busy.unwrap().0 + 1].parse().unwrap();
        assert!(
            allocations > 0,
            "{phase} allocated nothing:\n{}",
            indent(&report)
        );
    }
}

/// This machine's `variant` for a native output.
fn host_variant() -> String {
    let os = if cfg!(target_os = "macos") {
        "macos"
    } else {
        "linux"
    };
    let arch = if cfg!(target_arch = "aarch64") {
        "arm64"
    } else {
        "x86_64"
    };
    format!("{os}-{arch}")
}

/// A repository with one binary for `platform`, `native` or `node`, whose
/// source is `items` and a `main` running `body`.
fn large_shape(platform: &str, items: &str, body: &str) -> Scratch {
    let scratch = Scratch::repo("profile-large-shape");
    let output = match platform {
        "native" => format!(
            "{{ platform: \"native\", variant: \"{}\" }}",
            host_variant()
        ),
        _ => format!("{{ platform: \"{platform}\" }}"),
    };
    let host = if platform == "native" {
        "NativeHost"
    } else {
        "NodeHost"
    };
    scratch.write(
        "app/BUILD.buri",
        &format!("binary {{\n    outputs: [\n        {output},\n    ]\n}}\n"),
    );
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
    scratch
}

/// `buri build` of [`large_shape`]. Answers the profile's count of `work`, a
/// line `<work> <n>`, which no load can move.
fn counted_work(platform: &str, items: &str, body: &str, work: &str) -> Option<f64> {
    work_count(&counted(platform, items, body), work)
}

/// The `BURI_PROFILE=1` report of a `buri build` of [`large_shape`], on one
/// thread.
fn counted(platform: &str, items: &str, body: &str) -> String {
    let run = large_shape(platform, items, body).run_with_env(
        &["build", "//app"],
        &[("BURI_PROFILE", "1"), ("BURI_THREADS", "1")],
    );
    run.ok();
    run.all()
}

/// [`grows_linearly`] for each of `works`, off one build of each size of
/// `shape`'s program.
fn each_grows_linearly(
    what: &str,
    works: &[&str],
    platform: &str,
    shape: impl Fn(usize) -> (String, String),
    n: usize,
) {
    let report = |n| {
        let (items, body) = shape(n);
        counted(platform, &items, &body)
    };
    let (small, large) = (report(n), report(2 * n));
    for work in works {
        let count = |m| work_count(if m == n { &small } else { &large }, work);
        grows_linearly(&format!("{what}, {work}"), count, n);
    }
}

/// The count on a `BURI_PROFILE=1` report's `<work> <n>` line.
fn work_count(report: &str, work: &str) -> Option<f64> {
    let count = report
        .lines()
        .find_map(|l| l.strip_prefix(work)?.strip_prefix(' '));
    let count = count.unwrap_or_else(|| panic!("no `{work}` in:\n{}", indent(report)));
    Some(count.trim().parse::<f64>().unwrap())
}

/// **A build on one thread writes the binary a build on every core does.**
/// `BURI_THREADS=1` is how [`counted`] builds on one thread.
#[test]
fn a_build_on_one_thread_writes_the_same_binary() {
    if let Some(why) = ci::native_host_gap() {
        ci::skipped("build::profile", &why);
        return;
    }
    let (items, body) = many_lambdas(50);
    let scratch = large_shape("native", &items, &body);
    let built = |env: &[(&str, &str)]| {
        scratch
            .run_with_env(&["build", "//app", "--force"], env)
            .ok();
        std::fs::read(scratch.path(&format!(".buri/out/native/{}/app/app", host_variant())))
            .unwrap()
    };
    let (every_core, one_thread) = (built(&[]), built(&[("BURI_THREADS", "1")]));
    assert!(!every_core.is_empty());
    assert!(
        every_core == one_thread,
        "one thread built a different binary from every core"
    );
}

/// `n` structs deriving `Equal` and `Hash`, each with a function that maps and
/// compares a list of it: a few source lines that monomorphize into many
/// functions to lower and key.
fn many_instantiated_types(n: usize) -> (String, String) {
    let items: String = (0..n)
        .map(|i| {
            format!(
                "struct S{i}(Int, Str);\n\nderive Equal, Hash for S{i};\n\n\
                 fn g{i}<C: Allocator>(ctx: C, k: Int): Int {{\n    \
                 let xs = [S{i}(k, \"a\"), S{i}(k + {i}, \"b\")];\n    \
                 let ys = xs.map(ctx, fn(s) => s);\n    \
                 if (ys == xs) {{ ys.length() }} else {{ 0 }}\n}}\n\n"
            )
        })
        .collect();
    let calls: Vec<String> = (0..n).map(|i| format!("g{i}(ctx, k)")).collect();
    let body = format!(
        "    let k = 3;\n    let _ = io.println(ctx, \"${{{}}}\").ignore();\n",
        calls.join(" + ")
    );
    (items, body)
}

/// Each phase's instructions, in millions, off a `BURI_PROFILE=1` report, by
/// the phase's name: `None` where the platform has no per-thread counter.
fn phase_table(report: &str) -> Option<std::collections::HashMap<String, f64>> {
    let mut table = std::collections::HashMap::new();
    let header = report.lines().position(|l| l.starts_with("buri profile"))?;
    for line in report.lines().skip(header + 1) {
        let Some(row) = line.strip_prefix("  ") else {
            break;
        };
        let at = row.find(|c: char| c.is_ascii_digit() || c == '-')?;
        let count = row[at..].split_whitespace().next()?.parse::<f64>().ok()?;
        table.insert(row[..at].trim().to_string(), count);
    }
    (table.get("all phases").copied().unwrap_or(0.0) > 0.0).then_some(table)
}

/// **Lowering and keying a program's units are billed to codegen.** A build
/// lowered each program and rendered and hashed each unit for its cache key
/// outside any phase, so the report put 0.8 s of a 7 s build under `other`:
/// a quarter of what a program's own functions cost went there. Doubling the
/// functions a program builds now grows `other` by under a seventh of what
/// it grows the whole build by. The middle end and emission take the rest.
///
/// Instruction counts rather than times, read where the platform counts them;
/// the doubling cancels what every build pays alike.
#[test]
fn lowering_and_unit_keys_are_billed_to_codegen_not_other() {
    if let Some(why) = ci::native_host_gap() {
        ci::skipped("build::profile", &why);
        return;
    }
    let table = |n: usize| {
        let (items, body) = many_instantiated_types(n);
        let scratch = large_shape("native", &items, &body);
        let run = scratch.run_with_env(
            &["build", "//app"],
            &[("BURI_PROFILE", "1"), ("BURI_THREADS", "1")],
        );
        run.ok();
        for phase in ["middle", "emit", "other"] {
            run.says(&format!("\n  {phase} "));
        }
        phase_table(&run.all())
    };
    let (Some(small), Some(large)) = (table(200), table(400)) else {
        return;
    };
    let grew = |phase: &str| large[phase] - small[phase];
    let (other, whole) = (grew("other"), grew("all phases"));
    assert!(
        other <= whole / 7.0,
        "doubling the program grew `other` by {other:.1} M instructions, \
         {:.0}% of the build's {whole:.1} M",
        100.0 * other / whole
    );
}

/// **A release build builds each LLVM module once.** Every unit's module was
/// built, thrown away to learn which glue it asks for, and built again: 1.6%
/// of a release build. Now a cold build builds one module per object it
/// writes, the glue unit's included. A toolchain without the optimizing
/// backend refuses the build.
#[test]
fn a_release_build_builds_each_llvm_module_once() {
    if let Some(why) = ci::native_host_gap() {
        ci::skipped("build::profile", &why);
        return;
    }
    let (items, body) = many_instantiated_types(3);
    let scratch = large_shape("native", &items, &body);
    let run = scratch.run_with_env(&["build", "--release", "//app"], &[("BURI_PROFILE", "1")]);
    if run.stderr.contains("built without `backend-llvm`") {
        run.exits(1);
        return;
    }
    run.ok();
    let count = |name: &str| -> u64 {
        let all = run.all();
        let line = all
            .lines()
            .find_map(|l| l.strip_prefix(&format!("{name} ")))
            .unwrap_or_else(|| panic!("no `{name}` count in\n{}", indent(&all)));
        line.parse().unwrap()
    };
    let objects = count("objects compiled");
    assert!(objects > 2, "{objects} objects:\n{}", indent(&run.all()));
    assert_eq!(
        count("llvm modules built"),
        objects,
        "a module built for each object, once:\n{}",
        indent(&run.all())
    );
    let binary = scratch.path(&format!(".buri/out/native/{}/app/app", host_variant()));
    let ran = std::process::Command::new(binary).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&ran.stdout), "6\n");
}

/// Asserts that `work(2n)` is at most 2.3 times `work(n)`. Where the platform
/// has no counter, there is nothing to compare and this asserts nothing.
///
/// For a count the compiler keeps, which no load can move: the kernel's work
/// on a thread only ever adds to its instructions, and under load once added
/// more than half to one build's, which a growth bound can't tell from a
/// quadratic.
fn grows_linearly(what: &str, work: impl Fn(usize) -> Option<f64>, n: usize) {
    let (Some(small), Some(large)) = (work(n), work(2 * n)) else {
        return;
    };
    assert!(
        large <= small * 2.3,
        "{what}: {large:.0} at {}, {:.2} times the {small:.0} at {n}",
        2 * n,
        large / small
    );
}

/// Builds tuples of `n` elements, takes one apart and compares two.
fn long_tuple(n: usize) -> (String, String) {
    let types: Vec<&str> = (0..n)
        .map(|i| if i % 2 == 0 { "Int" } else { "Str" })
        .collect();
    let values: Vec<String> = (0..n)
        .map(|i| {
            if i % 2 == 0 {
                format!("k + {i}")
            } else {
                String::from("s")
            }
        })
        .collect();
    let names: Vec<String> = (0..n).map(|i| format!("a{i}")).collect();
    let summed: Vec<String> = (0..n)
        .map(|i| {
            if i % 2 == 0 {
                format!("a{i}")
            } else {
                format!("a{i}.length()")
            }
        })
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
    let from_int: String = (0..n - 1)
        .map(|i| format!("        {i} => .V{i},\n"))
        .collect();
    let to_int: String = (0..n).map(|i| format!("        .V{i} => {i},\n")).collect();
    let words: String = (0..n)
        .map(|i| format!("        \"w{i}\" => {i},\n"))
        .collect();
    let diagonal: String = (0..n)
        .map(|i| format!("        (.V{i}, .V{i}) => {i},\n"))
        .collect();
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
/// Counted in operand lists read and dominator steps, which no load can move.
#[test]
fn a_debug_builds_emission_is_linear_in_a_functions_length() {
    if let Some(why) = ci::native_host_gap() {
        ci::skipped("build::profile", &why);
        return;
    }
    each_grows_linearly(
        "emitting a long tuple's functions",
        &["ir operand lists read", "dominator steps"],
        "native",
        long_tuple,
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
            counted_work("node", &items, &body, "javascript nodes copied")
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
            counted_work("node", &items, &body, "pattern rows specialized")
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
            counted_work(
                "node",
                &items,
                &body,
                "types asked whether they carry an effect",
            )
        },
        1000,
    );
}

/// A variant of `n` strings, two of them taken apart, and an `else if` chain
/// comparing them field by field.
fn long_branching(n: usize) -> (String, String) {
    let types = vec!["Str"; n].join(", ");
    let made = vec!["s"; n].join(", ");
    let left: Vec<String> = (0..n).map(|i| format!("a{i}")).collect();
    let right: Vec<String> = (0..n).map(|i| format!("b{i}")).collect();
    let chain: Vec<String> = (0..n)
        .map(|i| format!("if (a{i} != b{i}) {{ {i} }}"))
        .collect();
    let items = format!(
        "enum W {{\n    T({types}),\n    Empty,\n}}\n\n\
         fn make<C: Allocator>(ctx: C, k: Int): W {{\n    \
         let s = \"w\".repeat(ctx, k);\n    .T({made})\n}}\n\n\
         fn differ(a: W, b: W): Int {{\n    match ((a, b)) {{\n        \
         (.T({left}), .T({right})) => {chain} else {{ 0 - 1 }},\n        \
         _ => 0 - 2,\n    }}\n}}\n",
        left = left.join(", "),
        right = right.join(", "),
        chain = chain.join(" else "),
    );
    let body = String::from(
        "    let _ = io.println(ctx, \"${differ(make(ctx, 1), make(ctx, 2))}\").ignore();\n",
    );
    (items, body)
}

/// **Reference counting is linear in the length of a branching expression.**
/// The `rc` pass copied its live set at every node and at every branch, so an
/// `else if` chain over `n` live fields copied `n` sets of up to `n` names:
/// 0.16 G instructions at 800 fields, 3.5 times what 400 took.
/// PERFORMANCE.md §6.32.
///
/// Counted in the live set's entries written, walked and copied.
#[test]
fn reference_counting_is_linear_in_a_long_branching_expression() {
    if let Some(why) = ci::native_host_gap() {
        ci::skipped("build::profile", &why);
        return;
    }
    grows_linearly(
        "planning the reference counts of a long `else if` chain",
        |n| {
            let (items, body) = long_branching(n);
            counted_work("native", &items, &body, "rc live entries touched")
        },
        400,
    );
}

/// A struct of `n` fields, alternately `Int` and `Str`, that derives `Hash`
/// and is hashed.
fn wide_hashed_struct(n: usize) -> (String, String) {
    let types: Vec<&str> = (0..n)
        .map(|i| if i % 2 == 0 { "Int" } else { "Str" })
        .collect();
    let values: Vec<String> = (0..n)
        .map(|i| {
            if i % 2 == 0 {
                format!("k + {i}")
            } else {
                String::from("s")
            }
        })
        .collect();
    let items = format!(
        "struct W({types});\n\nderive Hash for W;\n\n\
         fn make<C: Allocator>(ctx: C, k: Int): W {{\n    \
         let s = \"w\".repeat(ctx, k);\n    W({values})\n}}\n",
        types = types.join(", "),
        values = values.join(", "),
    );
    let body = String::from(
        "    let _ = io.println(ctx, \"${make(ctx, 1).hash() == make(ctx, 1).hash()}\").ignore();\n",
    );
    (items, body)
}

/// **Deriving `Hash` is linear in a struct's fields.** The hash threads one
/// accumulator through every field, and inlining a field's hash copied the
/// accumulator built so far: 0.55 G instructions in `middle` at 800 fields,
/// 3.8 times what 400 took. PERFORMANCE.md §6.36.
///
/// Counted in the expression nodes deriving built, copies included.
#[test]
fn deriving_hash_is_linear_in_a_structs_fields() {
    if let Some(why) = ci::native_host_gap() {
        ci::skipped("build::profile", &why);
        return;
    }
    grows_linearly(
        "deriving a wide struct's hash",
        |n| {
            let (items, body) = wide_hashed_struct(n);
            counted_work("native", &items, &body, "derive nodes built")
        },
        400,
    );
}

/// A struct of `n` fields, alternately `Int` and `Str`, that derives
/// `FromJson` and is decoded from an object with every member.
fn wide_decoded_struct(n: usize) -> (String, String) {
    let fields: String = (0..n)
        .map(|i| format!("    f{i}: {},\n", if i % 2 == 0 { "Int" } else { "Str" }))
        .collect();
    let members: Vec<String> = (0..n)
        .map(|i| {
            if i % 2 == 0 {
                format!("\\\"f{i}\\\":{i}")
            } else {
                format!("\\\"f{i}\\\":\\\"s\\\"")
            }
        })
        .collect();
    let items = format!(
        "from \"core/json\" import * as json;\n\
         from \"core/json\" import {{ DecodeError, FromJson, Json }};\n\n\
         derive FromJson for W;\nstruct W {{\n{fields}}}\n",
    );
    let body = format!(
        "    let doc = json.parse(ctx, \"{{{}}}\").withDefault(Json.Null);\n    \
         let w: Result<W, DecodeError> = json.decode(ctx, doc);\n    \
         let got = match (w) {{\n        .Ok(v) => v.f{last},\n        .Err(_) => 0 - 1,\n    }};\n    \
         let _ = io.println(ctx, \"${{got}}\").ignore();\n",
        members.join(","),
        last = (n - 1) / 2 * 2,
    );
    (items, body)
}

/// **Deriving `FromJson` is linear in a struct's fields.** Each field is one
/// member lookup and one decode, through helpers built once.
///
/// Counted in the expression nodes deriving built, helpers included.
#[test]
fn deriving_from_json_is_linear_in_a_structs_fields() {
    if let Some(why) = ci::native_host_gap() {
        ci::skipped("build::profile", &why);
        return;
    }
    grows_linearly(
        "deriving a wide struct's decoder",
        |n| {
            let (items, body) = wide_decoded_struct(n);
            counted_work("native", &items, &body, "derive nodes built")
        },
        200,
    );
}

/// A variant of `n` fields, alternately `Int` and `Str`, built and taken apart.
fn wide_variant(n: usize) -> (String, String) {
    let types: Vec<&str> = (0..n)
        .map(|i| if i % 2 == 0 { "Int" } else { "Str" })
        .collect();
    let values: Vec<String> = (0..n)
        .map(|i| {
            if i % 2 == 0 {
                format!("k + {i}")
            } else {
                String::from("s")
            }
        })
        .collect();
    let names: Vec<String> = (0..n).map(|i| format!("a{i}")).collect();
    let summed: Vec<String> = (0..n)
        .map(|i| {
            if i % 2 == 0 {
                format!("a{i}")
            } else {
                format!("a{i}.length()")
            }
        })
        .collect();
    let items = format!(
        "enum W {{\n    T({types}),\n    Empty,\n}}\n\n\
         derive Equal, Hash, Show, Ordered for W;\n\n\
         fn make<C: Allocator>(ctx: C, k: Int): W {{\n    \
         let s = \"w\".repeat(ctx, k);\n    .T({values})\n}}\n\n\
         fn sum(w: W): Int {{\n    match (w) {{\n        \
         .T({names}) => {summed},\n        .Empty => 0 - 1,\n    }}\n}}\n",
        types = types.join(", "),
        values = values.join(", "),
        names = names.join(", "),
        summed = summed.join(" + "),
    );
    let body = String::from(
        "    let (a, b) = (make(ctx, 1), make(ctx, 2));\n    \
         let _ = io.println(ctx, \"${sum(a)} ${a == b} ${a < b} ${a.hash() == b.hash()} ${a}\").ignore();\n",
    );
    (items, body)
}

/// **A debug build's emission is linear in a variant's fields.** Reading one
/// payload field typed every field in the variant, and building one scanned
/// back from each field for writes to its slot, so both were `n²`. §6.32's wide
/// variants took 2.7 G instructions at 1,600 fields, 2.9 times what 800 took.
/// PERFORMANCE.md §6.37.
#[test]
fn a_debug_builds_emission_is_linear_in_a_variants_fields() {
    if let Some(why) = ci::native_host_gap() {
        ci::skipped("build::profile", &why);
        return;
    }
    each_grows_linearly(
        "emitting a wide variant's functions",
        &["field types worked out", "ir operand lists read"],
        "native",
        wide_variant,
        1000,
    );
}

/// A body of `n` `let`s, each calling a function by a name no local has.
fn many_lets(n: usize) -> (String, String) {
    let lets: String = (0..n)
        .map(|i| format!("    let t{} = step(t{i}, {i});\n", i + 1))
        .collect();
    let items = format!(
        "fn step(a: Int, b: Int): Int {{\n    (a + b) % 1013\n}}\n\n\
         fn long(x: Int): Int {{\n    let t0 = x;\n{lets}    t{n}\n}}\n"
    );
    let body = String::from("    let _ = io.println(ctx, \"${long(2)}\").ignore();\n");
    (items, body)
}

/// **Checking a body is linear in its `let`s.** Looking a name up walked
/// every local in scope, and a name that is not a local, such as a function's,
/// walked all of them every time. PERFORMANCE.md §6.41.
#[test]
fn checking_is_linear_in_a_bodys_lets() {
    grows_linearly(
        "checking a long body",
        |n| {
            let (items, body) = many_lets(n);
            counted_work("node", &items, &body, "locals compared in a lookup")
        },
        4000,
    );
}

/// A file of `n` functions, each under a comment and holding two, unformatted.
fn commented_file(n: usize) -> String {
    (0..n)
        .map(|i| format!("// f{i} adds.\nfn f{i}(a:Int):Int{{\n  // inside {i}\n  a+{i} // beside {i}\n}}\n\n"))
        .collect()
}

/// **Formatting is linear in a file's comments.** Every lookup of the comments
/// above a token scanned the file's whole list, so a file of a few thousand
/// lines spent most of its formatting there. PERFORMANCE.md §6.41.
#[test]
fn formatting_is_linear_in_a_files_comments() {
    grows_linearly(
        "formatting a long commented file",
        |n| {
            let scratch = Scratch::repo("profile-format");
            scratch.write("app/main.buri", &commented_file(n));
            let run = scratch.run_with_env(&["format", "app/main.buri"], &[("BURI_PROFILE", "1")]);
            run.ok();
            work_count(&run.all(), "comment entries examined")
        },
        2000,
    );
}

/// One block of `n` `let`s naming a field of a parameter, each read once.
fn field_lets(n: usize) -> (String, String) {
    let lets: String = (0..n)
        .map(|i| {
            let field = if i % 2 == 0 { "b" } else { "a" };
            format!(
                "    let v{i} = p.{field};\n    let t{} = (t{i} + v{i} * {}) % 1013;\n",
                i + 1,
                i % 7 + 1
            )
        })
        .collect();
    let items = format!(
        "struct P {{\n    a: Int,\n    b: Int,\n}}\n\n\
         fn long(p: P): Int {{\n    let t0 = 0;\n{lets}    t{n}\n}}\n"
    );
    let body =
        String::from("    let _ = io.println(ctx, \"${long(P { a: 3, b: 4 })}\").ignore();\n");
    (items, body)
}

/// **Forwarding field reads is linear in a body's `let`s.** Each forwarded
/// path was substituted by a walk of every later statement, and every nested
/// block walked its whole subtree again: 12 G instructions in `middle` at
/// 8,000 `let`s, 3.96 times what 4,000 took. PERFORMANCE.md §6.42.
#[test]
fn forwarding_field_reads_is_linear_in_a_bodys_lets() {
    if let Some(why) = ci::native_host_gap() {
        ci::skipped("build::profile", &why);
        return;
    }
    grows_linearly(
        "forwarding a long body's field reads",
        |n| {
            let (items, body) = field_lets(n);
            counted_work("native", &items, &body, "forward nodes visited")
        },
        1000,
    );
}

/// One function of `n` capturing lambdas, each called.
fn many_lambdas(n: usize) -> (String, String) {
    let lambdas: String = (0..n)
        .map(|i| {
            format!(
                "    let k{i} = x + {i};\n    let f{i} = fn(y: Int): Int => (y * k{i}) % 1013;\n"
            )
        })
        .collect();
    let sums: String = (0..n)
        .map(|i| format!("    let t{} = (t{i} + f{i}({i})) % 100003;\n", i + 1))
        .collect();
    let items = format!("fn many(x: Int): Int {{\n{lambdas}    let t0 = 0;\n{sums}    t{n}\n}}\n");
    let body = String::from("    let _ = io.println(ctx, \"${many(2)}\").ignore();\n");
    (items, body)
}

/// **Closure conversion is linear in a function's lambdas.** Every lifted
/// lambda took a copy of its parent's whole table of locals, and every later
/// pass sized itself by it: 2.3 G instructions in `middle` at 4,000 lambdas,
/// 3 times what 2,000 took. PERFORMANCE.md §6.42.
#[test]
fn closure_conversion_is_linear_in_a_functions_lambdas() {
    if let Some(why) = ci::native_host_gap() {
        ci::skipped("build::profile", &why);
        return;
    }
    grows_linearly(
        "lifting many lambdas out of one function",
        |n| {
            let (items, body) = many_lambdas(n);
            counted_work("native", &items, &body, "locals after closure conversion")
        },
        1000,
    );
}

/// `buri lint //...` over a repository `write` fills in, answering the
/// profile's count of `work`.
fn linted(name: &str, write: impl Fn(&Scratch), work: &str) -> Option<f64> {
    let scratch = Scratch::repo(name);
    write(&scratch);
    let run = scratch.run_with_env(&["lint", "//..."], &[("BURI_PROFILE", "1")]);
    run.ok();
    work_count(&run.all(), work)
}

/// A chain of `n` libraries, each calling the one before it.
fn library_chain(scratch: &Scratch, n: usize) {
    for i in 0..n {
        let (deps, source) = match i.checked_sub(1) {
            Some(before) => (
                format!("    dependencies: [\"//lib/p{before}\"]\n"),
                format!(
                    "from \"//lib/p{before}\" import * as before;\n\n\
                     export fn total(): Int {{\n    before.total() + 1\n}}\n"
                ),
            ),
            None => (
                String::new(),
                String::from("export fn total(): Int {\n    1\n}\n"),
            ),
        };
        scratch.write(
            &format!("lib/p{i}/BUILD.buri"),
            &format!("library {{\n{deps}    visibility: [\"//visibility:public\"]\n}}\n"),
        );
        scratch.write(&format!("lib/p{i}/lib.buri"), &source);
    }
}

/// **Linting checks each library once.** `buri lint` analysed every target on
/// its own, so a library was checked again for every target that depends on
/// it: a chain of 200 libraries took 24.8 G instructions to lint, against 2.4 G
/// to build. PERFORMANCE.md §6.47.
#[test]
fn linting_checks_each_library_once() {
    grows_linearly(
        "checking a chain of libraries for lint",
        |n| {
            linted(
                "profile-lint-chain",
                |s| library_chain(s, n),
                "bodies checked",
            )
        },
        60,
    );
}

/// `buri <args>` over a chain of `n` libraries, cold and then warm: how much
/// of the build graph the two runs walked (`graph work` in the profile).
///
/// A count rather than instructions: instructions outside the compiler's
/// phases are mostly the kernel opening files, and under load they swung by a
/// third from run to run, which a growth bound can't tell from a quadratic.
fn graph_work(name: &str, n: usize, args: &[&str]) -> Option<f64> {
    let scratch = Scratch::repo(name);
    library_chain(&scratch, n);
    let mut total = 0.0;
    for _ in 0..2 {
        let run = scratch.run_with_env(args, &[("BURI_PROFILE", "1")]);
        run.ok();
        let all = run.all();
        let line = all.lines().find(|l| l.starts_with("graph work "))?;
        total += line
            .trim_start_matches("graph work ")
            .trim()
            .parse::<f64>()
            .ok()?;
    }
    (total > 0.0).then_some(total)
}

/// **A command walks the build graph once.** Each target walked its closure
/// from scratch, each library's check key folded every library below it, and
/// `buri lint` re-read dependency edges per target, so a chain of 800
/// libraries spent 11.7 G instructions outside the compiler to lint, against
/// 1.8 G at 200. PERFORMANCE.md §6.43.
#[test]
fn linting_a_chain_of_libraries_walks_the_graph_once() {
    grows_linearly(
        "the build graph a lint of a chain of libraries walks",
        |n| graph_work("profile-lint-graph", n, &["lint", "//..."]),
        150,
    );
}

/// The same for `buri build`, which keys every library's check.
#[test]
fn building_a_chain_of_libraries_walks_the_graph_once() {
    grows_linearly(
        "the build graph a build of a chain of libraries walks",
        |n| graph_work("profile-build-graph", n, &["build", "//..."]),
        150,
    );
}

/// **A cold build loads and checks each library once.** `buri build //...`
/// checked every library over its whole closure, so a chain loaded and checked
/// its first library once per library after it: 3.6 times the work per
/// doubling at 100 libraries. PERFORMANCE.md §6.47.
///
/// Counted in modules loaded and in bodies checked, which no load can move.
#[test]
fn a_cold_build_of_a_chain_of_libraries_checks_each_once() {
    let cold = |n: usize| {
        let scratch = Scratch::repo("profile-build-chain");
        library_chain(&scratch, n);
        let run = scratch.run_with_env(&["build", "//..."], &[("BURI_PROFILE", "1")]);
        run.ok();
        run.all()
    };
    let loaded = |all: &str| {
        let line = all.lines().find(|l| l.starts_with("modules loaded "))?;
        line.trim_start_matches("modules loaded ")
            .trim()
            .parse::<f64>()
            .ok()
    };
    grows_linearly(
        "a cold build of a chain of libraries, loading",
        |n| loaded(&cold(n)),
        100,
    );
    grows_linearly(
        "a cold build of a chain of libraries, checking",
        |n| work_count(&cold(n), "bodies checked"),
        100,
    );
}

/// **A build that lints loads each module once.** Under `check_during_build`,
/// the catalogue loaded and checked every library the build had just checked a
/// second time. A cold build of libraries with test suites loads each of their
/// modules, and each test source, exactly once. PERFORMANCE.md §6.92.
#[test]
fn a_build_that_lints_loads_each_module_once() {
    let scratch = Scratch::repo("profile-build-lints-once");
    scratch.write("REPO.buri", "lint {\n    check_during_build: true\n}\n");
    library_chain(&scratch, 4);
    for i in 0..4_usize {
        scratch.write(
            &format!("lib/p{i}/BUILD.buri"),
            &format!(
                "library {{\n{}    visibility: [\"//visibility:public\"]\n\n    \
                 test {{\n        sources: [\"test/p{i}.buri\"]\n    }}\n}}\n",
                i.checked_sub(1)
                    .map(|b| format!("    dependencies: [\"//lib/p{b}\"]\n"))
                    .unwrap_or_default()
            ),
        );
        scratch.write(
            &format!("lib/p{i}/test/p{i}.buri"),
            &format!(
                "from \"core/testing/assert\" import * as assert;\n\
                 from \"//lib/p{i}\" import {{ total }};\n\n\
                 test \"counts\" {{\n    assert.equal(total(), {});\n}}\n",
                i + 1
            ),
        );
    }
    let run = scratch.run_with_env(&["build", "//..."], &[("BURI_PROFILE", "1")]);
    run.ok();
    assert_eq!(
        modules_loaded(&run.all()),
        Some(8),
        "{}",
        indent(&run.all())
    );
}

/// A library of `n` types, each with a method and a function that builds one.
fn many_declarations(scratch: &Scratch, n: usize) {
    let items: String = (0..n)
        .map(|i| {
            format!(
                "struct R{i} {{\n    a: Int,\n}}\n\n\
                 impl R{i} {{\n    fn get(self): Int {{\n        self.a\n    }}\n}}\n\n\
                 export fn use{i}(x: Int): Int {{\n    R{i} {{ a: x }}.get()\n}}\n\n"
            )
        })
        .collect();
    scratch.write(
        "lib/big/BUILD.buri",
        "library {\n    visibility: [\"//visibility:public\"]\n}\n",
    );
    scratch.write("lib/big/lib.buri", &items);
}

/// **The lint rules are linear in the length of a file.** The type census
/// asked every type a module declares about every identifier in it: a file of
/// 1,000 types took 1.5 G instructions to lint, three times what 500 took.
/// PERFORMANCE.md §6.47.
#[test]
fn the_lint_rules_are_linear_in_a_files_length() {
    grows_linearly(
        "the lint rules over one long file",
        |n| {
            linted(
                "profile-lint-long-file",
                |s| many_declarations(s, n),
                "declarations compared with an identifier",
            )
        },
        400,
    );
}

/// **`unused-context` is linear in its findings.** Each finding's fix walked
/// every body in the package for call sites: a file of 4,000 functions that
/// never read their `ctx` took 6.5 G instructions to lint. PERFORMANCE.md §6.71.
#[test]
fn unused_context_is_linear_in_its_findings() {
    let linted = |n: usize| {
        let scratch = Scratch::repo("profile-lint-unused-contexts");
        let items: String = (0..n)
            .map(|i| format!("fn p{i}<C: Allocator>(ctx: C, x: Int): Int {{\n    x * {i}\n}}\n\n"))
            .collect();
        let calls: Vec<String> = (0..n).map(|i| format!("p{i}(ctx, {i})")).collect();
        scratch.write(
            "app/BUILD.buri",
            "binary {\n    outputs: [\n        { platform: \"node\" },\n    ]\n}\n",
        );
        scratch.write(
            "app/main.buri",
            &format!(
                "from \"platform/effect\" import {{ Allocator, Stdout }};\n\
                 from \"node\" import {{ NodeHost }};\n\
                 from \"core/io\" import * as io;\n\n{items}\
                 fn total<C: Allocator>(ctx: C): Int {{\n    {}\n}}\n\n\
                 export fn main(host: NodeHost): Result<(), Str> {{\n    \
                 let ctx = context {{ Allocator: host.alloc, Stdout: host.stdout }};\n    \
                 io.println(ctx, \"${{total(ctx)}}\").mapErr(fn(_e) => \"no stdout\")\n}}\n",
                calls.join(" +\n        ")
            ),
        );
        let run = scratch.run_with_env(&["lint", "//..."], &[("BURI_PROFILE", "1")]);
        run.exits(1);
        assert_eq!(
            run.all().matches("[unused-context]").count(),
            n,
            "{}",
            indent(&run.all())
        );
        work_count(&run.all(), "nodes walked for call sites")
    };
    grows_linearly("unused-context over one file", linted, 500);
}

/// A library of `n` functions, each with a context bound its body never uses.
fn unused_bounds(scratch: &Scratch, n: usize) {
    let items: String = (0..n)
        .map(|i| format!("export fn p{i}<C: Allocator>(ctx: C, x: Int): Int {{\n    let _ = ctx;\n    x * {i}\n}}\n\n"))
        .collect();
    scratch.write(
        "lib/big/BUILD.buri",
        "library {\n    visibility: [\"//visibility:public\"]\n}\n",
    );
    scratch.write(
        "lib/big/lib.buri",
        &format!("from \"platform/effect\" import {{ Allocator }};\n\n{items}"),
    );
}

/// **The language server publishes a file's findings in time linear in how
/// many it has.** Each finding's range was counted from the top of the file and
/// compared with every finding before it: a file with 4,000 took 26 G
/// instructions a keystroke. PERFORMANCE.md §6.70.
#[test]
fn publishing_a_files_findings_is_linear_in_how_many_it_has() {
    let published = |n: usize| {
        let scratch = Scratch::repo("profile-lsp-findings");
        unused_bounds(&scratch, n);
        let uri = format!("file://{}", scratch.path("lib/big/lib.buri").display());
        let root = format!("file://{}", scratch.root.display());
        let session: String = [
            format!(r#"{{"jsonrpc":"2.0","id":1,"method":"initialize","params":{{"rootUri":"{root}","capabilities":{{}}}}}}"#),
            r#"{"jsonrpc":"2.0","method":"initialized","params":{}}"#.to_string(),
            format!(r#"{{"jsonrpc":"2.0","id":2,"method":"textDocument/diagnostic","params":{{"textDocument":{{"uri":"{uri}"}}}}}}"#),
            r#"{"jsonrpc":"2.0","id":3,"method":"shutdown"}"#.to_string(),
            r#"{"jsonrpc":"2.0","method":"exit"}"#.to_string(),
        ]
        .iter()
        .map(|m| format!("Content-Length: {}\r\n\r\n{m}", m.len()))
        .collect();
        let run =
            scratch.run_with_stdin_and_env(&["lsp"], session.as_bytes(), &[("BURI_PROFILE", "1")]);
        assert_eq!(
            run.stdout
                .matches("\"code\":\"unused-context-bound\"")
                .count(),
            n,
            "{}",
            indent(&run.stdout)
        );
        run.stderr
    };
    let (small, large) = (published(200), published(400));
    for work in ["bytes walked to a position", "findings keyed"] {
        let count = |n| work_count(if n == 200 { &small } else { &large }, work);
        grows_linearly(
            &format!("publishing one file's findings, {work}"),
            count,
            200,
        );
    }
}

/// **A `format --check` of an unchanged tree formats nothing.** It formatted
/// every file on one core every run: 1.3–1.5 s for #259's 8 MB. Each file's
/// answer is kept under its bytes. PERFORMANCE.md §6.55.
#[test]
fn a_second_format_check_formats_no_file() {
    let scratch = Scratch::repo("profile-format-check");
    library_chain(&scratch, 20);
    scratch.write(
        "README.md",
        "# Notes\n\n```buri\nfn three(): Int {\n    3\n}\n```\n",
    );
    let formatted = |all: &str| -> Option<u64> {
        let line = all.lines().find(|l| l.starts_with("files formatted "))?;
        line.trim_start_matches("files formatted ")
            .trim()
            .parse()
            .ok()
    };
    let check = || scratch.run_with_env(&["format", "--check"], &[("BURI_PROFILE", "1")]);
    let cold = check();
    cold.ok();
    assert_eq!(formatted(&cold.all()), Some(42), "{}", indent(&cold.all()));
    let warm = check();
    warm.ok();
    assert_eq!(formatted(&warm.all()), Some(0), "{}", indent(&warm.all()));
}

/// The modules a `BURI_PROFILE=1` run loaded (`modules loaded` in the report).
fn modules_loaded(all: &str) -> Option<u64> {
    let line = all.lines().find(|l| l.starts_with("modules loaded "))?;
    line.trim_start_matches("modules loaded ")
        .trim()
        .parse()
        .ok()
}

/// **A native build with nothing to do loads nothing.** It used to check,
/// monomorphize, lower and emit the whole program before finding every unit
/// and the link cached: 0.42 s for #253's 160 libraries, against 0.03 s for
/// the same program on node. PERFORMANCE.md §6.43.
#[test]
fn a_warm_native_build_loads_no_module() {
    let scratch = Scratch::repo("profile-warm-native");
    library_chain(&scratch, 3);
    scratch.write(
        "app/BUILD.buri",
        &format!(
            "binary {{\n    dependencies: [\"//lib/p2\"]\n    outputs: [{{ platform: \"native\", variant: \"{}\" }}]\n}}\n",
            host_variant()
        ),
    );
    scratch.write(
        "app/main.buri",
        "from \"core/io\" import * as io;\n\
         from \"native\" import { NativeHost };\n\
         from \"platform/effect\" import { Allocator, Stdout };\n\
         from \"//lib/p2\" import { total };\n\n\
         export fn main(host: NativeHost): Result<(), Str> {\n    \
         let ctx = context { Allocator: host.alloc, Stdout: host.stdout };\n    \
         io.println(ctx, \"${total()}\").mapErr(fn(_e) => \"stdout\")\n}\n",
    );
    let build = || scratch.run_with_env(&["build", "//app"], &[("BURI_PROFILE", "1")]);
    let cold = build();
    cold.ok();
    assert!(
        modules_loaded(&cold.all()).is_some_and(|n| n > 0),
        "{}",
        indent(&cold.all())
    );
    let warm = build();
    warm.ok().says("cached");
    assert_eq!(
        modules_loaded(&warm.all()),
        Some(0),
        "{}",
        indent(&warm.all())
    );
}

/// The build files a `BURI_PROFILE=1` run of `buri gen` worked out rather than
/// recalled (`build files worked out` in the report).
fn build_files_worked_out(all: &str) -> Option<u64> {
    let line = all
        .lines()
        .find(|l| l.starts_with("build files worked out "))?;
    line.trim_start_matches("build files worked out ")
        .trim()
        .parse()
        .ok()
}

/// **A `gen --check` of an unchanged tree works out no build file.** It
/// checked every package again, on one core, every run: 7–9 s for #260's
/// 8 MB. Each package's answer is kept under what it read. PERFORMANCE.md
/// §6.59.
#[test]
fn a_second_gen_check_works_out_no_build_file() {
    let scratch = Scratch::repo("profile-gen-check");
    library_chain(&scratch, 20);
    let check = || scratch.run_with_env(&["gen", "--check"], &[("BURI_PROFILE", "1")]);
    let cold = check();
    cold.ok();
    assert_eq!(
        build_files_worked_out(&cold.all()),
        Some(20),
        "{}",
        indent(&cold.all())
    );
    assert!(
        modules_loaded(&cold.all()).is_some_and(|n| n > 0),
        "{}",
        indent(&cold.all())
    );
    let warm = check();
    warm.ok();
    assert_eq!(
        build_files_worked_out(&warm.all()),
        Some(0),
        "{}",
        indent(&warm.all())
    );
    assert_eq!(
        modules_loaded(&warm.all()),
        Some(0),
        "{}",
        indent(&warm.all())
    );
}

/// The targets a `BURI_PROFILE=1` run of `buri lint` analysed rather than
/// recalled (`targets linted` in the report).
fn targets_linted(all: &str) -> Option<u64> {
    let line = all.lines().find(|l| l.starts_with("targets linted "))?;
    line.trim_start_matches("targets linted ")
        .trim()
        .parse()
        .ok()
}

/// **A second lint analyses no target, and neither does a stray file.**
/// `unused-source` reads a package's directory, so its answer is asked again on
/// every run rather than remembered. That is a walk of each package, and
/// nothing is loaded or analysed for a file appearing or going away
/// (buri-lang/buri#281).
#[test]
fn a_second_lint_analyses_no_target_and_a_stray_file_none_either() {
    let scratch = Scratch::repo("profile-lint-warm");
    library_chain(&scratch, 20);
    scratch.write(
        "lib/shapes/BUILD.buri",
        "library {\n    generators: [\n        { tool: \"json\", inputs: [\"square.schema.json\"] },\n    ]\n}\n",
    );
    scratch.write(
        "lib/shapes/square.schema.json",
        "{\n    \"$schema\": \"https://json-schema.org/draft/2020-12/schema\",\n    \"title\": \"Square\",\n    \"type\": \"object\"\n}\n",
    );
    scratch.write(
        "lib/shapes/lib.buri",
        "from \"//lib/shapes/square.schema.json\" export { Square };\n",
    );
    let lint = || scratch.run_with_env(&["lint", "//..."], &[("BURI_PROFILE", "1")]);
    let counts = |all: &str| (targets_linted(all), modules_loaded(all));

    let cold = lint();
    cold.ok();
    assert_eq!(
        targets_linted(&cold.all()),
        Some(21),
        "{}",
        indent(&cold.all())
    );
    let warm = lint();
    warm.ok();
    assert_eq!(
        counts(&warm.all()),
        (Some(0), Some(0)),
        "{}",
        indent(&warm.all())
    );

    scratch.write("lib/shapes/stray.json", "{}\n");
    let stray = lint();
    stray
        .exits(1)
        .says("lib/shapes/stray.json belongs to no library or binary");
    assert_eq!(
        counts(&stray.all()),
        (Some(0), Some(0)),
        "{}",
        indent(&stray.all())
    );

    let _ = std::fs::remove_file(scratch.path("lib/shapes/stray.json"));
    let gone = lint();
    gone.ok();
    assert_eq!(
        counts(&gone.all()),
        (Some(0), Some(0)),
        "{}",
        indent(&gone.all())
    );
}

/// The same with a generator: a warm check runs no tool and works out
/// nothing, where a generator's key in a record once spoiled it.
#[test]
fn a_second_gen_check_runs_no_generator() {
    let source = tests_dir().join("repositories/cli/gen_check_remembers/repo");
    let scratch = Scratch::copy_of("profile-gen-check-generator", &source);
    let check = || scratch.run_with_env(&["gen", "--check"], &[("BURI_PROFILE", "1")]);
    let cold = check();
    cold.ok();
    assert_eq!(
        build_files_worked_out(&cold.all()),
        Some(5),
        "{}",
        indent(&cold.all())
    );
    let warm = check();
    warm.ok();
    // Only where the platform counts a process's children.
    if let Some(children) = warm
        .all()
        .lines()
        .find(|l| l.starts_with("child processes: "))
    {
        assert_eq!(
            children,
            "child processes: 0.000 s cpu",
            "{}",
            indent(&warm.all())
        );
    }
    assert_eq!(
        build_files_worked_out(&warm.all()),
        Some(0),
        "{}",
        indent(&warm.all())
    );
    assert_eq!(
        modules_loaded(&warm.all()),
        Some(0),
        "{}",
        indent(&warm.all())
    );
}

/// **A cold `gen --check` loads each library once.** It analysed every
/// package over its whole closure, so a chain loaded its first library once
/// per library after it. PERFORMANCE.md §6.59.
#[test]
fn a_cold_gen_check_of_a_chain_of_libraries_loads_each_once() {
    let loaded = |n: usize| {
        let scratch = Scratch::repo("profile-gen-chain");
        library_chain(&scratch, n);
        let run = scratch.run_with_env(&["gen", "--check"], &[("BURI_PROFILE", "1")]);
        run.ok();
        modules_loaded(&run.all()).map(|m| m as f64)
    };
    grows_linearly(
        "a cold gen --check of a chain of libraries, loading",
        loaded,
        100,
    );
}

/// The graphs of what reads what a `BURI_PROFILE=1` run drew to extract
/// styles (`style graphs drawn` in the report).
fn style_graphs_drawn(all: &str) -> Option<u64> {
    let line = all.lines().find(|l| l.starts_with("style graphs drawn "))?;
    line.trim_start_matches("style graphs drawn ")
        .trim()
        .parse()
        .ok()
}

/// **A test build that imports nothing from `ui/` draws no style graph.**
/// A test that takes an allocator from `platform/effect/testing` loads
/// `ui/style` through it, and style extraction drew its graph over every
/// declaration the build reads, the standard library's included: 12 M of
/// `test/one-edit`'s instructions (design/PERFORMANCE.md §6.87). No style such
/// a program holds can reach a node.
#[test]
fn a_test_build_that_imports_nothing_from_ui_draws_no_style_graph() {
    let scratch = repo();
    scratch.write(
        "lib/a/test/a.buri",
        "from \"//lib/a\" import { answer };\n\
         from \"core/testing/assert\" import * as assert;\n\
         from \"platform/effect\" import { Allocator };\n\
         from \"platform/effect/testing\" import { alloc };\n\n\
         test \"a answers in a list\" {\n    \
         let ctx = context {\n        Allocator: alloc(),\n    };\n    \
         assert.equal([answer()].map(ctx, fn(n) => n + 1), [43]);\n}\n",
    );
    let run = scratch.run_with_env(&["test", "//..."], &[("BURI_PROFILE", "1")]);
    run.ok();
    assert_eq!(run.tests_passed(), 1, "{}", indent(&run.all()));
    assert_eq!(
        style_graphs_drawn(&run.all()),
        Some(0),
        "{}",
        indent(&run.all())
    );
}

/// **A test build that gives a node a style draws the graph once, and the
/// style is its class.** The other side of
/// [`a_test_build_that_imports_nothing_from_ui_draws_no_style_graph`]: a
/// program that imports from `ui/` still has its styles extracted, and one
/// with no generic parameter a node reads past its result draws the graph
/// only once.
#[test]
fn a_test_build_that_styles_a_node_draws_the_style_graph_once() {
    let scratch = Scratch::repo("profile-styled");
    scratch.write(
        "lib/card/BUILD.buri",
        "library {\n    test {\n        sources: [\"test/card.buri\"]\n    }\n}\n",
    );
    scratch.write(
        "lib/card/lib.buri",
        "from \"ui/node\" import * as ui;\n\
         from \"ui/node\" import { Node };\n\n\
         export fn card<C>(children: [Node<C>]): Node<C> {\n    \
         ui.stack({ styles: [.MinWidth(.Px(285))], children: children })\n}\n",
    );
    scratch.write(
        "lib/card/test/card.buri",
        "from \"core/testing/assert\" import * as assert;\n\
         from \"platform/effect\" import { Ui };\n\
         from \"platform/effect/testing\" import { headless, render, stylesheet };\n\
         from \"//lib/card\" import { card };\n\n\
         test \"a card's style is its class\" {\n    \
         let ctx = context {\n        Ui: headless(),\n    };\n    \
         let page = render(ctx, card([]));\n    \
         assert.equal(page.markup(), \"e 0 class:minw-285\");\n    \
         assert.isTrue(stylesheet().contains(\".minw-285{min-width:285px}\"));\n}\n",
    );
    let run = scratch.run_with_env(&["test", "//..."], &[("BURI_PROFILE", "1")]);
    run.ok();
    assert_eq!(run.tests_passed(), 1, "{}", indent(&run.all()));
    assert_eq!(
        style_graphs_drawn(&run.all()),
        Some(1),
        "{}",
        indent(&run.all())
    );
}

/// **A test build that hands a node its styles through a generic struct draws
/// the style graph once.** A generic parameter a node reads through a field
/// stands for every caller's argument, and finding one drew the graph of what
/// reads what again, over every declaration, once per round that found more.
/// Only the declarations that call such a function read differently.
#[test]
fn a_style_handed_through_a_generic_struct_draws_the_style_graph_once() {
    let scratch = Scratch::repo("profile-styled-generic");
    scratch.write(
        "lib/card/BUILD.buri",
        "library {\n    test {\n        sources: [\"test/card.buri\"]\n    }\n}\n",
    );
    scratch.write(
        "lib/card/lib.buri",
        "from \"ui/node\" import * as ui;\n\
         from \"ui/node\" import { Node };\n\
         from \"ui/style\" import { Style };\n\n\
         export struct Props<C> {\n    export children: [Node<C>],\n    \
         export styles: [Style],\n}\n\n\
         export fn card<C>(props: Props<C>): Node<C> {\n    \
         ui.stack({ styles: [.Group(props.styles)], children: props.children })\n}\n\n\
         export fn framed<C>(props: Props<C>): Node<C> {\n    \
         card(Props { children: [card(props)], styles: [.MinWidth(.Px(9))] })\n}\n",
    );
    scratch.write(
        "lib/card/test/card.buri",
        "from \"core/testing/assert\" import * as assert;\n\
         from \"platform/effect\" import { Ui };\n\
         from \"platform/effect/testing\" import { headless, render, stylesheet };\n\
         from \"//lib/card\" import { framed, Props };\n\n\
         test \"a framed card's styles are its classes\" {\n    \
         let ctx = context {\n        Ui: headless(),\n    };\n    \
         let page = render(ctx, framed(Props { children: [], styles: [.MinWidth(.Px(286))] }));\n    \
         assert.equal(page.markup(), \"e 0 class:minw-9\\ne 1 class:minw-286\");\n    \
         assert.isTrue(stylesheet().contains(\".minw-286{min-width:286px}\"));\n}\n",
    );
    let run = scratch.run_with_env(&["test", "//..."], &[("BURI_PROFILE", "1")]);
    run.ok();
    assert_eq!(run.tests_passed(), 1, "{}", indent(&run.all()));
    assert_eq!(
        style_graphs_drawn(&run.all()),
        Some(1),
        "{}",
        indent(&run.all())
    );
}

/// The times a `BURI_PROFILE=1` run searched a type for a value (`type
/// values searched` in the report).
fn type_values_searched(all: &str) -> Option<u64> {
    let line = all
        .lines()
        .find(|l| l.starts_with("type values searched "))?;
    line.trim_start_matches("type values searched ")
        .trim()
        .parse()
        .ok()
}

/// **Finding that each of a chain of structs has a value searches each
/// once.** Each struct holding the last inline was searched for a value from
/// scratch, through every struct below it: a chain of five thousand took
/// 129 G instructions to check, against 0.2 G now.
#[test]
fn a_chain_of_structs_each_holding_the_last_is_searched_for_values_once() {
    let searched = |n: usize| {
        let scratch = Scratch::repo("profile-struct-chain");
        scratch.write("lib/chain/BUILD.buri", "library {}\n");
        let mut source = String::from("export struct S0 {\n    export v: Int,\n}\n");
        for i in 1..n {
            let j = i - 1;
            source.push_str(&format!(
                "\nexport struct S{i} {{\n    export inner: S{j},\n}}\n"
            ));
        }
        scratch.write("lib/chain/lib.buri", &source);
        let run = scratch.run_with_env(&["build", "//..."], &[("BURI_PROFILE", "1")]);
        run.ok();
        type_values_searched(&run.all()).map(|n| n as f64)
    };
    grows_linearly("a chain of structs, searching for values", searched, 300);
}

/// A suite that writes, appends to, reads, renames, lists and removes `n`
/// files in one directory of the testing filesystem.
fn many_files_suite(n: usize) -> Scratch {
    let scratch = Scratch::repo("profile-many-files");
    scratch.write(
        "lib/files/BUILD.buri",
        "library {\n    test {\n        sources: [\"test/files.buri\"]\n    }\n}\n",
    );
    scratch.write("lib/files/lib.buri", "export fn unused(): Int { 0 }\n");
    scratch.write(
        "lib/files/test/files.buri",
        &format!(
            "from \"core/fs\" import * as fs;\n\
             from \"core/fs\" import {{ FileSystemRead, FileSystemWrite }};\n\
             from \"core/path\" import * as path;\n\
             from \"core/str\" import * as str;\n\
             from \"core/testing/assert\" import * as assert;\n\
             from \"platform/effect\" import {{ Allocator }};\n\
             from \"platform/effect/testing\" import {{ alloc, fs as disk }};\n\n\
             fn fill<C: Allocator + FileSystemRead + FileSystemWrite>(ctx: C, i: Int, n: Int): Int {{\n    \
             if (i == n) {{\n        0\n    }} else {{\n        \
             let at = path.of(ctx, str.format(ctx, \"d/f${{i}}\"));\n        \
             assert.ok(fs.writeText(ctx, at, \"x\"));\n        \
             assert.ok(fs.append(ctx, at, [33]));\n        \
             assert.equal(assert.ok(fs.readText(ctx, at)), \"x!\");\n        \
             assert.ok(fs.rename(ctx, at, path.of(ctx, str.format(ctx, \"d/g${{i}}\"))));\n        \
             fill(ctx, i + 1, n) + 1\n    }}\n}}\n\n\
             fn empty<C: Allocator + FileSystemWrite>(ctx: C, i: Int, n: Int): Int {{\n    \
             if (i == n) {{\n        0\n    }} else {{\n        \
             assert.ok(fs.remove(ctx, path.of(ctx, str.format(ctx, \"d/g${{i}}\"))));\n        \
             empty(ctx, i + 1, n) + 1\n    }}\n}}\n\n\
             test \"{n} files come and go\" {{\n    \
             let files = disk();\n    \
             let ctx = context {{\n        Allocator: alloc(),\n        \
             FileSystemRead: files,\n        FileSystemWrite: files,\n    }};\n    \
             assert.ok(fs.makeDir(ctx, path.of(ctx, \"d\")));\n    \
             assert.equal(fill(ctx, 0, {n}), {n});\n    \
             assert.equal(assert.ok(fs.listDir(ctx, path.of(ctx, \"d\"))).length(), {n});\n    \
             assert.equal(empty(ctx, 0, {n}), {n});\n    \
             assert.isEmpty(assert.ok(fs.listDir(ctx, path.of(ctx, \"d\"))));\n}}\n"
        ),
    );
    scratch
}

/// **The testing filesystem is linear in the files it holds.** Every read,
/// write and parent check scanned a list of every file, and a listing
/// deduplicated against a list of every name, so a suite's filesystem work
/// grew with the square of its files.
#[test]
fn the_test_filesystem_is_linear_in_its_files() {
    grows_linearly(
        "a suite over many files",
        |n| {
            let run =
                many_files_suite(n).run_with_env(&["test", "//..."], &[("BURI_PROFILE", "1")]);
            run.ok();
            assert_eq!(run.tests_passed(), 1, "{}", indent(&run.all()));
            work_count(&run.all(), "testing filesystem keys walked")
        },
        2000,
    );
}

/// The `source:` line's count of lines of code.
fn source_lines(report: &str) -> u64 {
    let line = report
        .lines()
        .find_map(|l| l.strip_prefix("source: "))
        .unwrap_or_else(|| panic!("no `source:` line in\n{}", indent(report)));
    line.split_whitespace().next().unwrap().parse().unwrap()
}

/// A cold profiled `buri build` of `target`, and its count of lines of code.
fn lines_built(scratch: &Scratch, target: &str) -> u64 {
    scratch.run(&["clean"]).ok();
    let run = scratch.run_with_env(&["build", target], &[("BURI_PROFILE", "1")]);
    run.ok();
    source_lines(&run.all())
}

/// A native program at `dir` printing `word`: eight lines of code.
fn printing(scratch: &Scratch, dir: &str, word: &str) {
    scratch.write(
        &format!("{dir}/BUILD.buri"),
        &format!(
            "binary {{\n    outputs: [\n        {{ platform: \"native\", variant: \"{}\" }},\n    ]\n}}\n",
            host_variant()
        ),
    );
    scratch.write(
        &format!("{dir}/main.buri"),
        &format!(
            "from \"platform/effect\" import {{ Allocator, Stdout }};\n\
             from \"native\" import {{ NativeHost }};\n\
             from \"core/io\" import * as io;\n\
             export fn main(host: NativeHost): Result<(), Str> {{\n    \
             let ctx = context {{ Allocator: host.alloc, Stdout: host.stdout }};\n    \
             let _ = io.println(ctx, \"{word}\").ignore();\n    \
             .Ok(())\n}}\n"
        ),
    );
}

/// **The report's lines of code leave out blank lines and comments.** The
/// compile goals are rates per line of code, so a commented file is no
/// faster to compile than the same code bare.
#[test]
fn the_line_count_leaves_out_blank_lines_and_comments() {
    if let Some(why) = ci::native_host_gap() {
        ci::skipped("build::profile", &why);
        return;
    }
    let scratch = Scratch::repo("profile-lines");
    printing(&scratch, "app", "hi");
    let bare = lines_built(&scratch, "//app");
    scratch.edit(
        "app/main.buri",
        "export fn main",
        "\n\n// A plain comment.\n\n/// The program.\n/// It prints.\nexport fn main",
    );
    scratch.edit(
        "app/main.buri",
        "    .Ok(())",
        "\n    // Done.\n    .Ok(())",
    );
    assert_eq!(
        lines_built(&scratch, "//app"),
        bare,
        "blank or comment lines were counted as code"
    );
    scratch.edit(
        "app/main.buri",
        "    .Ok(())",
        "    let _ = io.println(ctx, \"again\").ignore();\n    .Ok(())",
    );
    assert_eq!(lines_built(&scratch, "//app"), bare + 1);
}

/// **Two programs that share the standard library count it once.** Building
/// both together counts the first's lines and the second's own source, and
/// not the library again.
#[test]
fn the_line_count_counts_the_standard_library_once() {
    if let Some(why) = ci::native_host_gap() {
        ci::skipped("build::profile", &why);
        return;
    }
    let scratch = Scratch::repo("profile-lines-once");
    printing(&scratch, "p0", "zero");
    printing(&scratch, "p1", "one");
    let first = lines_built(&scratch, "//p0");
    let both = lines_built(&scratch, "//...");
    assert!(
        first > 1000,
        "the standard library went uncounted: {first} lines"
    );
    // `p1`'s build file is `p0`'s, word for word, so only its main is new.
    assert_eq!(both, first + 8, "the standard library was counted twice");
}

/// **A build told to count one phase for callgrind builds as usual outside
/// valgrind**, whatever the group: `BURI_PROFILE_COLLECT`'s requests do
/// nothing there, and a group it doesn't know is ignored.
#[test]
fn a_build_counting_one_phase_for_callgrind_builds_as_usual() {
    if let Some(why) = ci::native_host_gap() {
        ci::skipped("build::profile", &why);
        return;
    }
    let scratch = Scratch::repo("profile-collect");
    printing(&scratch, "app", "hi");
    for group in ["parse", "check", "other", "nonsense"] {
        scratch.run(&["clean"]).ok();
        let run = scratch.run_with_env(
            &["build", "//app"],
            &[("BURI_PROFILE", "1"), ("BURI_PROFILE_COLLECT", group)],
        );
        run.ok().says("\n  lex+parse ").says("\n  check ");
    }
}

/// **A build callgrind measures counts none of the profile's own work**, so
/// the goals count what a build does without the profile: under any
/// `BURI_PROFILE_COLLECT` group, the report prints no work counts, and
/// without one it does.
#[test]
fn a_build_callgrind_measures_counts_no_work_of_the_profile() {
    if let Some(why) = ci::native_host_gap() {
        ci::skipped("build::profile", &why);
        return;
    }
    let scratch = Scratch::repo("profile-collect-work");
    printing(&scratch, "app", "hi");
    scratch.run(&["clean"]).ok();
    scratch
        .run_with_env(&["build", "//app"], &[("BURI_PROFILE", "1")])
        .ok()
        .says("\nbodies checked ")
        .says("\nlocals compared in a lookup ");
    for group in ["check", "whole"] {
        scratch.run(&["clean"]).ok();
        scratch
            .run_with_env(
                &["build", "//app"],
                &[("BURI_PROFILE", "1"), ("BURI_PROFILE_COLLECT", group)],
            )
            .ok()
            .says("\n  check ")
            .silent_about("bodies checked")
            .silent_about("locals compared in a lookup");
    }
}

/// `buri build //...` of a repository holding a page, a node program of `n`
/// small functions that imports nothing from `ui/`, or both. Answers the
/// functions monomorphization walked for styles.
fn page_and_program(page: bool, program: Option<usize>) -> Option<f64> {
    let scratch = Scratch::repo("profile-page-and-program");
    if page {
        scratch.write(
            "cmd/page/BUILD.buri",
            "binary {\n    outputs: [{ platform: \"web\" }]\n}\n",
        );
        scratch.write(
            "cmd/page/main.buri",
            "from \"core/io\" import * as io;\n\
             from \"platform/effect\" import { Allocator, Stdout, Ui, Watch };\n\
             from \"ui/node\" import * as ui;\n\
             from \"web\" import { WebHost };\n\n\
             export fn main(host: WebHost): Result<(), Str> {\n    \
             let ctx = context { Allocator: host.alloc, Stdout: host.stdout, Ui: host.ui, Watch: host.watch };\n    \
             let _ = io.println(ctx, \"page\").mapErr(fn(_e) => \"could not write\")?;\n    \
             let app = ui.text({ content: .Const(\"app\") });\n    \
             ui.mount(ctx, ui.stack({ styles: [.MinWidth(.Px(285))], children: [app] }), [])\n}\n",
        );
    }
    if let Some(n) = program {
        let steps: String = (0..n)
            .map(|i| {
                format!(
                    "fn step{i}(x: Int): Int {{\n    let a = (x * {i} + x * 3 - x / 7 + (x % 11) * (x + 5) - (x * x) % 13) % 1013;\n    \
                     let b = if (a > 500) {{ a - 7 }} else {{ a + 11 }};\n    \
                     [a, b, x].length() + b\n}}\n\n"
                )
            })
            .collect();
        let calls: String = (0..n)
            .map(|i| format!("    let t{} = (t{i} + step{i}(t{i})) % 100003;\n", i + 1))
            .collect();
        scratch.write(
            "cmd/tool/BUILD.buri",
            "binary {\n    outputs: [{ platform: \"node\" }]\n}\n",
        );
        scratch.write(
            "cmd/tool/main.buri",
            &format!(
                "from \"core/io\" import * as io;\n\
                 from \"platform/effect\" import {{ Allocator, Stdout }};\n\
                 from \"node\" import {{ NodeHost }};\n\n{steps}\
                 fn many(x: Int): Int {{\n    let t0 = x;\n{calls}    t{n}\n}}\n\n\
                 export fn main(host: NodeHost): Result<(), Str> {{\n    \
                 let ctx = context {{ Allocator: host.alloc, Stdout: host.stdout }};\n    \
                 io.println(ctx, \"${{many(2)}}\").mapErr(fn(_e) => \"could not write\")\n}}\n"
            ),
        );
    }
    let run = scratch.run_with_env(&["build", "//...", "--force"], &[("BURI_PROFILE", "1")]);
    run.ok();
    work_count(&run.all(), "functions surveyed for styles")
}

/// **A program with no styles pays nothing for a page built beside it.** One
/// compilation checks both and holds `ui/style` for the page, and every
/// function of the program was walked for styles none of its values can hold:
/// monomorphizing the two together took 1.12 times what the two took apart, at
/// 400 functions. A count of the functions walked for styles, which no load
/// can move.
#[test]
fn a_program_with_no_styles_pays_nothing_for_a_page_beside_it() {
    let n = 400;
    let (Some(both), Some(page), Some(program)) = (
        page_and_program(true, Some(n)),
        page_and_program(true, None),
        page_and_program(false, Some(n)),
    ) else {
        return;
    };
    assert!(
        both <= page + program,
        "{both} functions walked for styles building a page and a program together, \
         against {page} and {program} apart"
    );
}
