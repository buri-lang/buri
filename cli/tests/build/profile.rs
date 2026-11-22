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

/// **A one-test run reads a few hundred of the stencil library's twenty-five
/// thousand stencils.** The library is read in place, so a run pays for the
/// stencils it emits and nothing for the rest (design/PERFORMANCE.md §6.124).
#[test]
fn a_one_test_run_reads_only_the_stencils_it_emits() {
    let run = repo().run_with_env(&["test", "//..."], &[("BURI_PROFILE", "1")]);
    run.ok();
    assert_eq!(run.tests_passed(), 1, "{}", indent(&run.all()));
    let read = work_count(&run.all(), "stencils read").unwrap_or_default();
    assert!(
        (1.0..1_000.0).contains(&read),
        "a one-test run read {read} stencils:\n{}",
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

/// **The whole process's instructions read in billions, as `B`.** Only macOS
/// counts the whole process.
#[test]
fn a_profiled_run_counts_the_process_in_billions() {
    let scratch = repo();
    let run = scratch.run_with_env(&["test", "//..."], &[("BURI_PROFILE", "1")]);
    run.ok();
    let report = run.all();
    let Some(line) = report.lines().find(|l| l.starts_with("process: ")) else {
        assert_ne!(
            std::env::consts::OS,
            "macos",
            "no process line in\n{}",
            indent(&report)
        );
        return;
    };
    let words: Vec<&str> = line.split_whitespace().collect();
    assert!(
        words[1].parse::<f64>().is_ok() && words[2..4] == ["B", "instructions,"],
        "{line}"
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
        ci::missing("build::profile", &why);
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
        ci::missing("build::profile", &why);
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
        ci::missing("build::profile", &why);
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

/// `n` recursive functions, which nothing inlines, and a `main` printing the
/// sum of each at one: `n * (n + 1) / 2`.
fn many_recursive(n: usize) -> (String, String) {
    let items: String = (0..n)
        .map(|i| format!("fn f{i}(k: Int): Int {{\n    if (k <= 0) {{ {i} }} else {{ f{i}(k - 1) + 1 }}\n}}\n\n"))
        .collect();
    let calls: Vec<String> = (0..n).map(|i| format!("f{i}(1)")).collect();
    let body = format!(
        "    let _ = io.println(ctx, \"${{{}}}\").ignore();\n",
        calls.join(" + ")
    );
    (items, body)
}

/// **A unit is joined from its parts without copying the first.** A debug
/// build emits a unit's functions in parts of up to 512, and each part's code
/// was copied into a buffer of the unit's: a second copy of every unit's code,
/// which most units, being one part, never needed. Now a unit of one part is
/// that part, so a small program copies no code to join its units, and a unit
/// of more parts copies only the parts after its first. Both programs run.
#[test]
fn a_unit_is_joined_without_copying_its_first_part() {
    if let Some(why) = ci::native_host_gap() {
        ci::missing("build::profile", &why);
        return;
    }
    let copied = |n: usize| {
        let (items, body) = many_recursive(n);
        let scratch = large_shape("native", &items, &body);
        let run = scratch.run_with_env(&["build", "//app"], &[("BURI_PROFILE", "1")]);
        run.ok();
        let binary = scratch.path(&format!(".buri/out/native/{}/app/app", host_variant()));
        let ran = std::process::Command::new(binary).output().unwrap();
        assert_eq!(
            String::from_utf8_lossy(&ran.stdout),
            format!("{}\n", n * (n + 1) / 2)
        );
        work_count(&run.all(), "unit code bytes copied").unwrap()
    };
    assert_eq!(copied(3), 0.0, "one part a unit, and nothing copied");
    assert!(copied(600) > 0.0, "a unit of two parts joins them");
}

/// **The table of instantiated functions moves none of them as it fills.** It
/// started empty and doubled, and each doubling copied every function it held
/// to fresh memory, the dearest copy in a debug build. Now it starts with room
/// for every declared function, which a program's instantiations rarely
/// outnumber. Doubling moved 1,020 functions for the 604 this build makes,
/// more than one for each, so a quarter of one for each is a bound four times
/// clear of it. The program runs.
#[test]
fn the_function_table_moves_no_function_as_it_fills() {
    if let Some(why) = ci::native_host_gap() {
        ci::missing("build::profile", &why);
        return;
    }
    let (items, body) = many_recursive(600);
    let scratch = large_shape("native", &items, &body);
    let run = scratch.run_with_env(&["build", "//app"], &[("BURI_PROFILE", "1")]);
    run.ok();
    let binary = scratch.path(&format!(".buri/out/native/{}/app/app", host_variant()));
    let ran = std::process::Command::new(binary).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&ran.stdout), "180300\n");
    let report = run.all();
    let moved = work_count(&report, "function rows moved").unwrap();
    let made = work_count(&report, "functions instantiated").unwrap();
    assert!(
        moved * 4.0 < made,
        "{moved} functions moved for {made} made"
    );
}

/// **On one thread, each lowered function is folded in as it is made.** The
/// lowering kept every function's result until the last was made, which
/// wrote each to memory nothing had touched and read it back from there for
/// the fold. On two threads the results still wait for one another, so the
/// count there is every function, and on one it is none. The program runs
/// either way.
#[test]
fn a_build_on_one_thread_folds_each_function_as_it_is_lowered() {
    if let Some(why) = ci::native_host_gap() {
        ci::missing("build::profile", &why);
        return;
    }
    let (items, body) = many_recursive(300);
    let held = |threads: &str| {
        let scratch = large_shape("native", &items, &body);
        let run = scratch.run_with_env(
            &["build", "//app"],
            &[("BURI_PROFILE", "1"), ("BURI_THREADS", threads)],
        );
        run.ok();
        let binary = scratch.path(&format!(".buri/out/native/{}/app/app", host_variant()));
        let ran = std::process::Command::new(binary).output().unwrap();
        assert_eq!(String::from_utf8_lossy(&ran.stdout), "45150\n");
        work_count(&run.all(), "results held for a fold").unwrap()
    };
    assert_eq!(held("1"), 0.0, "one thread holds nothing");
    assert!(held("2") > 300.0, "two threads hold every function");
}

/// **Lint looks each identifier a module writes up once.** It looked each up
/// in two tables, one for the names written outside an import and one for
/// the names counted as undeclared. A function of [`many_recursive`] writes
/// seven identifiers, so 200 more of them are 1,400 more identifiers, and the
/// two tables looked up 2,800 more names. The bound is 2,100, halfway. The
/// program runs.
#[test]
fn lint_looks_each_identifier_up_once() {
    if let Some(why) = ci::native_host_gap() {
        ci::missing("build::profile", &why);
        return;
    }
    let looked_up = |n: usize| {
        let (items, body) = many_recursive(n);
        let scratch = large_shape("native", &items, &body);
        let run = scratch.run_with_env(&["build", "//app"], &[("BURI_PROFILE", "1")]);
        run.ok();
        let binary = scratch.path(&format!(".buri/out/native/{}/app/app", host_variant()));
        let ran = std::process::Command::new(binary).output().unwrap();
        assert_eq!(
            String::from_utf8_lossy(&ran.stdout),
            format!("{}\n", n * (n + 1) / 2)
        );
        work_count(&run.all(), "names looked up for lint").unwrap()
    };
    let more = looked_up(400) - looked_up(200);
    assert!(
        more <= 2100.0,
        "{more} more names looked up for 1,400 more identifiers"
    );
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
/// tuple of 400 elements built and taken apart took 1.24 B instructions to
/// emit, 3.5 times what 200 took. PERFORMANCE.md §6.32.
///
/// Counted in operand lists read and dominator steps, which no load can move.
#[test]
fn a_debug_builds_emission_is_linear_in_a_functions_length() {
    if let Some(why) = ci::native_host_gap() {
        ci::missing("build::profile", &why);
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
/// took 1.2 B instructions to check.
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
/// 0.16 B instructions at 800 fields, 3.5 times what 400 took.
/// PERFORMANCE.md §6.32.
///
/// Counted in the live set's entries written, walked and copied.
#[test]
fn reference_counting_is_linear_in_a_long_branching_expression() {
    if let Some(why) = ci::native_host_gap() {
        ci::missing("build::profile", &why);
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
/// accumulator built so far: 0.55 B instructions in `middle` at 800 fields,
/// 3.8 times what 400 took. PERFORMANCE.md §6.36.
///
/// Counted in the expression nodes deriving built, copies included.
#[test]
fn deriving_hash_is_linear_in_a_structs_fields() {
    if let Some(why) = ci::native_host_gap() {
        ci::missing("build::profile", &why);
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
        ci::missing("build::profile", &why);
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
/// variants took 2.7 B instructions at 1,600 fields, 2.9 times what 800 took.
/// PERFORMANCE.md §6.37.
#[test]
fn a_debug_builds_emission_is_linear_in_a_variants_fields() {
    if let Some(why) = ci::native_host_gap() {
        ci::missing("build::profile", &why);
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
/// block walked its whole subtree again: 12 B instructions in `middle` at
/// 8,000 `let`s, 3.96 times what 4,000 took. PERFORMANCE.md §6.42.
#[test]
fn forwarding_field_reads_is_linear_in_a_bodys_lets() {
    if let Some(why) = ci::native_host_gap() {
        ci::missing("build::profile", &why);
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
/// pass sized itself by it: 2.3 B instructions in `middle` at 4,000 lambdas,
/// 3 times what 2,000 took. PERFORMANCE.md §6.42.
#[test]
fn closure_conversion_is_linear_in_a_functions_lambdas() {
    if let Some(why) = ci::native_host_gap() {
        ci::missing("build::profile", &why);
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

/// `n` functions, each folding two wide literals into its argument and
/// calling the next: stencils whose holes are mostly literals and calls, so
/// most of the emitter's work is patching them. Answers the source and what
/// `main` prints.
fn many_patched_holes(n: usize) -> (String, String, i64) {
    let mul = |i: usize| 1_099_511_627_000 + i as i64;
    let add = |i: usize| 281_474_976_710_000 + i as i64;
    let items: String = (0..n)
        .map(|i| {
            let next = if i + 1 < n {
                format!("w{}(r + {i})", i + 1)
            } else {
                String::from("r")
            };
            format!(
                "fn w{i}(x: Int): Int {{\n    let r = (x * {} + {}) % 1000003;\n    {next}\n}}\n",
                mul(i),
                add(i)
            )
        })
        .collect();
    let body = String::from("    let _ = io.println(ctx, \"${w0(7)}\").ignore();\n");
    let mut x = 7_i64;
    for i in 0..n {
        let r = (x * mul(i) + add(i)) % 1_000_003;
        x = if i + 1 < n { r + i as i64 } else { r };
    }
    (items, body, x)
}

/// **A program whose stencils are mostly holes runs right, and patches each
/// hole once.** A stencil's holes are patched in place as it is copied; the
/// count is of holes, which no load moves, so a hole patched twice shows here.
#[test]
fn a_program_of_many_patched_holes_runs_and_patches_linearly() {
    if let Some(why) = ci::native_host_gap() {
        ci::missing("build::profile", &why);
        return;
    }
    let patched = |n: usize| {
        let (items, body, printed) = many_patched_holes(n);
        let scratch = large_shape("native", &items, &body);
        let run = scratch.run_with_env(
            &["build", "//app"],
            &[("BURI_PROFILE", "1"), ("BURI_THREADS", "1")],
        );
        run.ok();
        scratch
            .run(&["run", "//app"])
            .ok()
            .says(&format!("{printed}\n"));
        let holes = work_count(&run.all(), "holes patched").unwrap_or_default();
        assert!(
            (2.0 * n as f64..=12.0 * n as f64).contains(&holes),
            "{n} functions of wide literals patched {holes} holes:\n{}",
            indent(&run.all())
        );
        Some(holes)
    };
    grows_linearly("patching many holes", patched, 200);
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

/// **Lint finds a test's assertion without walking what it compares.** Each
/// test asserted on `chain()`, and lint, looking for an assertion the test
/// reaches, walked every function `chain()` calls before the `assert.equal`
/// beside it: `n` tests over a chain of `n` walked the square of `n` bodies.
#[test]
fn lint_finds_a_tests_assertion_without_walking_what_it_compares() {
    grows_linearly(
        "tests asserting on a chain of calls, walking bodies for assertions",
        |n| {
            let scratch = Scratch::repo("profile-assert-chain");
            let mut lib = String::from("export fn f0(): Int {\n    1\n}\n");
            for i in 1..n {
                lib.push_str(&format!(
                    "\nexport fn f{i}(): Int {{\n    f{}() + 1\n}}\n",
                    i - 1
                ));
            }
            scratch.write(
                "lib/chain/BUILD.buri",
                "library {\n    test {\n        sources: [\"test/chain.buri\"]\n    }\n}\n",
            );
            scratch.write("lib/chain/lib.buri", &lib);
            let mut tests = format!(
                "from \"core/testing/assert\" import * as assert;\n\
                 from \"//lib/chain\" import {{ f{} }};\n",
                n - 1
            );
            for i in 0..n {
                tests.push_str(&format!(
                    "\ntest \"chain {i}\" {{\n    assert.equal(f{}(), {n});\n}}\n",
                    n - 1
                ));
            }
            scratch.write("lib/chain/test/chain.buri", &tests);
            let run = scratch.run_with_env(&["lint", "//..."], &[("BURI_PROFILE", "1")]);
            run.ok();
            assert!(
                !run.all().contains("test-missing-assertion"),
                "{}",
                indent(&run.all())
            );
            work_count(&run.all(), "bodies walked for assertions")
        },
        12,
    );
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

/// [`library_chain`], with a suite in each library.
fn tested_chain(n: usize) -> Scratch {
    let scratch = Scratch::repo("profile-tested-chain");
    library_chain(&scratch, n);
    for i in 0..n {
        let build = scratch.read(&format!("lib/p{i}/BUILD.buri"));
        let build = build.replacen(
            "library {\n",
            "library {\n    test {\n        sources: [\"test/total.buri\"]\n    }\n",
            1,
        );
        scratch.write(&format!("lib/p{i}/BUILD.buri"), &build);
        scratch.write(
            &format!("lib/p{i}/test/total.buri"),
            &format!(
                "from \"core/testing/assert\" import * as assert;\n\n\
                 test \"p{i}\" {{\n    assert.equal({i}, {i});\n}}\n"
            ),
        );
    }
    scratch
}

/// **A test run hashes each source once for its keys.** Every suite's two
/// keys, its verdict's and its build's, hashed the bytes of every source in
/// its closure, so a chain of libraries each with a suite hashed the square of
/// its length. A key now takes each source's digest, which a run works out
/// once.
#[test]
fn a_test_run_hashes_each_source_once_for_its_keys() {
    grows_linearly(
        "a chain of tested libraries, hashing sources for keys",
        |n| {
            let run = tested_chain(n).run_with_env(
                &["test", "//..."],
                &[("BURI_PROFILE", "1"), ("BURI_THREADS", "1")],
            );
            run.ok();
            assert_eq!(run.tests_passed(), n, "{}", indent(&run.all()));
            work_count(&run.all(), "sources hashed for keys")
        },
        8,
    );
}

/// **A test run keys each rule once.** Every suite's two keys listed the
/// files of every rule in its closure and folded in each one's digest, so a
/// chain of libraries each with a suite did the square of its length in
/// lookups. Each rule's share of a key is now worked out once a run.
#[test]
fn a_test_run_keys_each_rule_once() {
    grows_linearly(
        "a chain of tested libraries, keying rules",
        |n| {
            let run = tested_chain(n).run_with_env(
                &["test", "//..."],
                &[("BURI_PROFILE", "1"), ("BURI_THREADS", "1")],
            );
            run.ok();
            assert_eq!(run.tests_passed(), n, "{}", indent(&run.all()));
            work_count(&run.all(), "rules keyed")
        },
        8,
    );
}

/// `n` libraries with a suite each, every one of them dropping a record of
/// lists that `//lib/row` declares.
fn rows_dropped(n: usize) -> Scratch {
    let scratch = Scratch::repo("profile-rows-dropped");
    scratch.write(
        "lib/row/BUILD.buri",
        "library {\n    visibility: [\"//visibility:public\"]\n}\n",
    );
    scratch.write(
        "lib/row/lib.buri",
        "export struct Row {\n    export names: [Str],\n    export groups: [[Str]],\n}\n",
    );
    for i in 0..n {
        scratch.write(
            &format!("lib/p{i}/BUILD.buri"),
            "library {\n    dependencies: [\"//lib/row\"]\n    \
             test {\n        sources: [\"test/width.buri\"]\n    }\n}\n",
        );
        scratch.write(
            &format!("lib/p{i}/lib.buri"),
            &format!(
                "from \"//lib/row\" import {{ Row }};\n\n\
                 export fn width(row: Row): Int {{\n    \
                 row.names.length() + row.groups.length() + {i}\n}}\n"
            ),
        );
        scratch.write(
            &format!("lib/p{i}/test/width.buri"),
            &format!(
                "from \"//lib/p{i}\" import * as p;\n\
                 from \"//lib/row\" import {{ Row }};\n\
                 from \"core/testing/assert\" import * as assert;\n\n\
                 test \"p{i}\" {{\n    \
                 let row = Row {{ names: [\"a\", \"b\"], groups: [[\"c\"], [\"d\", \"e\"]] }};\n    \
                 assert.equal(p.width(row), {});\n}}\n",
                4 + i
            ),
        );
    }
    scratch
}

/// **The glue every unit drops a type with is emitted once a run.** Drop and
/// copy glue is a weak symbol every unit that needs it defines, and each unit
/// emitted it again: a test run of `n` suites over one record emitted its glue
/// in every unit of every suite. The run now emits each body once and lays it
/// out again wherever it's needed, so twice the suites emit no more glue.
#[test]
fn a_test_run_emits_the_glue_its_units_share_once() {
    if let Some(why) = ci::native_host_gap() {
        ci::missing("build::profile", &why);
        return;
    }
    let glue = |n: usize| {
        let run = rows_dropped(n).run_with_env(
            &["test", "//..."],
            &[("BURI_PROFILE", "1"), ("BURI_THREADS", "1")],
        );
        run.ok();
        assert_eq!(run.tests_passed(), n, "{}", indent(&run.all()));
        work_count(&run.all(), "glue bodies emitted").unwrap()
    };
    let (few, many) = (glue(4), glue(8));
    assert!(few > 0.0, "4 suites dropping a record emitted no glue");
    assert_eq!(
        many, few,
        "8 suites dropping one record emitted {many} glue bodies, against {few} for 4"
    );
}

/// `n` programs, every one of them handing a context to `//lib/walk`'s helper,
/// which only passes it on.
fn contexts_handed_on(n: usize) -> Scratch {
    let scratch = Scratch::repo("profile-contexts-handed-on");
    scratch.write(
        "lib/walk/BUILD.buri",
        "library {\n    visibility: [\"//visibility:public\"]\n}\n",
    );
    scratch.write(
        "lib/walk/lib.buri",
        "from \"platform/effect\" import { Allocator };\n\n\
         export fn doubled<C: Allocator>(ctx: C, xs: [Int]): [Int] {\n    \
         xs.map(ctx, fn(x) => x * 2)\n}\n",
    );
    for i in 0..n {
        scratch.write(
            &format!("app{i}/BUILD.buri"),
            "binary {\n    dependencies: [\"//lib/walk\"]\n    \
             outputs: [{ platform: \"node\" }]\n}\n",
        );
        scratch.write(
            &format!("app{i}/main.buri"),
            &format!(
                "from \"platform/effect\" import {{ Allocator, Stdout }};\n\
                 from \"node\" import {{ NodeHost }};\n\
                 from \"core/io\" import * as io;\n\
                 from \"//lib/walk\" import * as walk;\n\n\
                 export fn main(host: NodeHost): Result<(), Str> {{\n    \
                 let ctx = context {{ Allocator: host.alloc, Stdout: host.stdout }};\n    \
                 let _ = io.println(ctx, \"${{walk.doubled(ctx, [{i}]).length()}}\").ignore();\n    \
                 .Ok(())\n}}\n"
            ),
        );
    }
    scratch
}

/// **A build reads each function for erasable type parameters once.** Every
/// program read the body of every generic function it reached for which type
/// parameters it only hands on (`erase.rs`), so `n` programs over one library
/// read its helper, and the standard library's, `n` times. The programs built
/// from one compilation now share what a body says, so twice the programs
/// read no more.
#[test]
fn a_build_reads_each_function_for_erasure_once() {
    let read = |n: usize| {
        let run = contexts_handed_on(n).run_with_env(
            &["build", "//..."],
            &[("BURI_PROFILE", "1"), ("BURI_THREADS", "1")],
        );
        run.ok();
        work_count(&run.all(), "functions read for erasure").unwrap()
    };
    let (few, many) = (read(2), read(4));
    assert!(
        few > 0.0,
        "2 programs handing on a context read no function"
    );
    assert_eq!(
        many, few,
        "4 programs handing on one context read {many} functions, against {few} for 2"
    );
}

/// **A function's runtime key is spelled once, not per instance.** Every
/// instance of a function spelled its runtime key, two allocations, to see
/// whether a JavaScript engine answers it, so `n` instances of one generic
/// function spelled it `n` times. The answer is now kept per declaration.
#[test]
fn a_runtime_key_is_spelled_once_per_function() {
    let spelled = |n: usize| {
        let items: String = (0..n)
            .map(|i| format!("struct S{i} {{\n    v: Int,\n}}\n\n"))
            .chain(std::iter::once(String::from(
                "fn id<T>(x: T): T {\n    x\n}\n",
            )))
            .collect();
        let body: String = (0..n)
            .map(|i| format!("    let _ = id(S{i} {{ v: {i} }});\n"))
            .collect();
        counted_work("node", &items, &body, "runtime keys spelled").unwrap()
    };
    let (few, many) = (spelled(8), spelled(16));
    assert_eq!(
        many, few,
        "16 instances of one function spelled {many} runtime keys, against {few} for 8"
    );
}

/// **Linting checks each library once.** `buri lint` analysed every target on
/// its own, so a library was checked again for every target that depends on
/// it: a chain of 200 libraries took 24.8 B instructions to lint, against 2.4 B
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
/// libraries spent 11.7 B instructions outside the compiler to lint, against
/// 1.8 B at 200. PERFORMANCE.md §6.43.
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
/// 1,000 types took 1.5 B instructions to lint, three times what 500 took.
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
/// never read their `ctx` took 6.5 B instructions to lint. PERFORMANCE.md §6.71.
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
/// compared with every finding before it: a file with 4,000 took 26 B
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
/// 129 B instructions to check, against 0.2 B now.
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
        ci::missing("build::profile", &why);
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
        ci::missing("build::profile", &why);
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
        ci::missing("build::profile", &why);
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
        ci::missing("build::profile", &why);
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

/// `n` helpers, each called once by `total` and so inlined there, which leaves
/// every one of them reached by nothing. `main` calls `total` twice, so
/// `total` is not inlined, and `main`'s size does not depend on `n`.
fn inlined_helpers(n: usize) -> (String, String) {
    let mut items: String = (0..n)
        .map(|i| format!("fn helper{i}(x: Int): Int {{\n    x * 3 + {i}\n}}\n\n"))
        .collect();
    let sum: Vec<String> = (0..n).map(|i| format!("helper{i}(x + {i})")).collect();
    items.push_str(&format!(
        "fn total(x: Int): Int {{\n    {}\n}}\n",
        sum.join(" + ")
    ));
    let body = "    let _ = io.println(ctx, \"${total(1) - total(2)}\").ignore();\n".to_string();
    (items, body)
}

/// `work` in `buri build` of [`inlined_helpers`], at `n` and at twice `n`.
fn with_helpers(platform: &str, work: &str, n: usize) -> (f64, f64) {
    let count = |n| {
        let (items, body) = inlined_helpers(n);
        counted_work(platform, &items, &body, work).unwrap()
    };
    (count(n), count(2 * n))
}

/// **A native build compiles nothing for a function nothing reaches.**
/// Inlining a helper at its one call site leaves the helper reached by
/// nothing, and each one was still compiled to an abort and linked. A count
/// of the functions compiled, which no load can move.
#[test]
fn a_native_build_compiles_nothing_for_an_inlined_helper() {
    if let Some(why) = ci::native_host_gap() {
        ci::missing("build::profile", &why);
        return;
    }
    let (few, many) = with_helpers("native", "functions compiled", 20);
    assert_eq!(
        few, many,
        "{few} functions compiled with 20 inlined helpers, {many} with 40"
    );
}

/// **A JavaScript build declares nothing for a function nothing reaches.**
/// The minifier dropped such a function, but only after every one had been
/// generated and folded. A count of the functions declared, which no load can
/// move.
#[test]
fn a_javascript_build_declares_nothing_for_an_inlined_helper() {
    let (few, many) = with_helpers("node", "javascript functions declared", 20);
    assert_eq!(
        few, many,
        "{few} functions declared with 20 inlined helpers, {many} with 40"
    );
}

/// `n` helpers with one body, each called twice by `total` so that none is
/// inlined. Every one is its own function to the middle end, and the same
/// JavaScript.
fn twin_helpers(n: usize) -> (String, String) {
    let mut items: String = (0..n)
        .map(|i| {
            format!(
                "fn helper{i}(x: Int): Int {{\n    let a = x * 3 + 1;\n    \
                 let b = a * a % 1009;\n    let c = b + a * 7;\n    \
                 let d = c % 101 + b;\n    d * 2 + c\n}}\n\n"
            )
        })
        .collect();
    let sum: Vec<String> = (0..n)
        .map(|i| format!("helper{i}(x) + helper{i}(x + 1)"))
        .collect();
    items.push_str(&format!(
        "fn total(x: Int): Int {{\n    {}\n}}\n",
        sum.join(" + ")
    ));
    let body =
        "    let _ = io.println(ctx, \"total ${total(1) + total(2)}\").ignore();\n".to_string();
    (items, body)
}

/// What [`twin_helpers`]'s program prints, worked out here.
fn twin_total(n: i64) -> i64 {
    let helper = |x: i64| {
        let a = x * 3 + 1;
        let b = a * a % 1009;
        let c = b + a * 7;
        let d = c % 101 + b;
        d * 2 + c
    };
    let total = |x| n * (helper(x) + helper(x + 1));
    total(1) + total(2)
}

/// **A JavaScript build folds one body once.** Functions that print the same
/// are merged into one, and the minifier did that only after folding every
/// one of them. A count of the functions folded, which no load can move, and
/// the program still prints what its source says.
#[test]
fn a_javascript_build_folds_one_body_once() {
    let folded = |n| {
        let (items, body) = twin_helpers(n);
        counted_work("node", &items, &body, "javascript functions folded").unwrap()
    };
    let (few, many) = (folded(20), folded(40));
    assert_eq!(
        few, many,
        "{few} functions folded with 20 helpers of one body, {many} with 40"
    );
    let (items, body) = twin_helpers(40);
    large_shape("node", &items, &body)
        .run(&["run", "//app"])
        .ok()
        .says(&format!("total {}", twin_total(40)));
}

/// A record of `n` `Int` fields that `steps` recursive functions each take
/// unchanged three calls deep and read one field of, and the sum `main`
/// prints of them.
fn wide_record_passed(n: usize, steps: usize) -> (String, String, usize) {
    let fields: Vec<String> = (0..n).map(|i| format!("    a{i}: Int,")).collect();
    let made: Vec<String> = (0..n).map(|i| format!("a{i}: {i}")).collect();
    let field = |s: usize| s * (n - 1) / steps.max(2);
    let bumps: String = (0..steps)
        .map(|s| {
            let f = field(s);
            format!(
                "fn bump{s}(r: R, k: Int): Int {{\n    \
                 if (k <= 0) {{ r.a{f} }} else {{ bump{s}(r, k - 1) + 1 }}\n}}\n"
            )
        })
        .collect();
    let items = format!(
        "struct R {{\n{fields}\n}}\n\n{bumps}",
        fields = fields.join("\n")
    );
    let sum: Vec<String> = (0..steps).map(|s| format!("bump{s}(r, 3)")).collect();
    let body = format!(
        "    let r = R {{ {made} }};\n    let t = {sum};\n    \
         let _ = io.println(ctx, \"${{t}}\").ignore();\n",
        made = made.join(", "),
        sum = sum.join(" + "),
    );
    (items, body, (0..steps).map(|s| field(s) + 3).sum())
}

/// **A wide record passed between functions runs right, and a copy of it is a
/// few holes however wide it is.** Each copy was a stencil per thirty-two
/// bytes, so a record of a few hundred words passed to a call was hundreds of
/// stencils laid out and patched one by one. PERFORMANCE.md §6.152.
#[test]
fn a_wide_record_passed_between_functions_runs_and_copies_in_a_few_holes() {
    if let Some(why) = ci::native_host_gap() {
        ci::missing("build::profile", &why);
        return;
    }
    let holes = |n: usize| {
        let (items, body, printed) = wide_record_passed(n, 12);
        let scratch = large_shape("native", &items, &body);
        let run = scratch.run_with_env(
            &["build", "//app"],
            &[("BURI_PROFILE", "1"), ("BURI_THREADS", "1")],
        );
        run.ok();
        scratch
            .run(&["run", "//app"])
            .ok()
            .says(&format!("{printed}\n"));
        work_count(&run.all(), "holes patched").unwrap()
    };
    // 134 fields is 1072 bytes, wide enough for the loop, and leaves a tail
    // after the looped runs. The record's literal alone costs about six holes
    // a field; copies by the run of thirty-two bytes cost four times that.
    let (narrow, wide) = (holes(134), holes(402));
    let per_field = (wide - narrow) / 268.0;
    assert!(
        per_field <= 12.0,
        "a record 268 fields wider patched {per_field:.1} more holes per field \
         ({narrow} at 134 fields, {wide} at 402)"
    );
}

/// **A record too narrow for the copy loop is copied a hundred and
/// twenty-eight bytes to a stencil.** Copies under the loop went thirty-two
/// bytes to a stencil, so a record of thirty-one words was eight stencils on
/// every call it was passed to; it is three now. PERFORMANCE.md §6.154.
#[test]
fn a_mid_sized_record_passed_between_functions_copies_in_few_holes() {
    if let Some(why) = ci::native_host_gap() {
        ci::missing("build::profile", &why);
        return;
    }
    let holes = |n: usize| {
        let (items, body, printed) = wide_record_passed(n, 12);
        let scratch = large_shape("native", &items, &body);
        let run = scratch.run_with_env(
            &["build", "//app"],
            &[("BURI_PROFILE", "1"), ("BURI_THREADS", "1")],
        );
        run.ok();
        scratch
            .run(&["run", "//app"])
            .ok()
            .says(&format!("{printed}\n"));
        work_count(&run.all(), "holes patched").unwrap()
    };
    // Eight fields is 64 bytes and thirty-one is 248, just under the loop's
    // 256. Thirty-two-byte copies cost about 24 holes a field between them,
    // 128-byte copies about 12.
    let (narrow, wide) = (holes(8), holes(31));
    let per_field = (wide - narrow) / 23.0;
    assert!(
        per_field <= 16.0,
        "a record 23 fields wider patched {per_field:.1} more holes per field \
         ({narrow} at 8 fields, {wide} at 31)"
    );
}

/// `n` functions returning a record each arm of an `if` builds, and `n`
/// returning a number each arm computes, with a `main` that prints the sum of
/// their answers. Answers the items, the body and what it prints.
fn returned_from_arms(n: usize) -> (String, String, usize) {
    let items: String = (0..n)
        .map(|i| {
            format!(
                "fn make{i}(a: Int): P {{\n    \
                 if (a > {i}) {{ P {{ x: a, y: a + {i}, z: a * 2 }} }} \
                 else {{ P {{ x: {i}, y: a, z: 0 }} }}\n}}\n\n\
                 fn pick{i}(a: Int): Int {{\n    \
                 if (a > {i}) {{ a * 3 }} else {{ a + {i} }}\n}}\n\n"
            )
        })
        .collect();
    let sum: Vec<String> = (0..n)
        .map(|i| format!("make{i}({}).y + pick{i}({i})", i * 2))
        .collect();
    let body = format!(
        "    let t = {};\n    let _ = io.println(ctx, \"${{t}}\").ignore();\n",
        sum.join(" + ")
    );
    let items = format!("struct P {{\n    x: Int,\n    y: Int,\n    z: Int,\n}}\n\n{items}");
    (items, body, 5 * n * (n - 1) / 2)
}

/// **A value each arm of an `if` computes is returned where it is built.**
/// Both arms jumped to a block that copied its parameter into the return
/// area, and a record built in the returning block was copied there too, so
/// every such function paid a copy on every return. Now each arm returns, and
/// the record's fields are written into the return area.
#[test]
fn values_returned_from_arms_are_built_in_the_return_area() {
    if let Some(why) = ci::native_host_gap() {
        ci::missing("build::profile", &why);
        return;
    }
    let copies = |n: usize| {
        let (items, body, printed) = returned_from_arms(n);
        let scratch = large_shape("native", &items, &body);
        let run = scratch.run_with_env(
            &["build", "//app"],
            &[("BURI_PROFILE", "1"), ("BURI_THREADS", "1")],
        );
        run.ok();
        scratch
            .run(&["run", "//app"])
            .ok()
            .says(&format!("{printed}\n"));
        work_count(&run.all(), "holes copying returns").unwrap()
    };
    // Each function copied its answer into the return area: a record of three
    // words in two stencils, and a word in one, each of three holes.
    let (few, many) = (copies(8), copies(24));
    let per_pair = (many - few) / 16.0;
    assert!(
        per_pair < 1.0,
        "16 more pairs of functions patched {per_pair:.1} more holes each copying \
         returns ({few} at 8, {many} at 24)"
    );
}

/// `n` functions each called with two string literals around a call made for
/// the same call's arguments, with a `main` that prints the sum of their
/// answers. Answers the items, the body and what it prints.
fn literals_around_calls(n: usize) -> (String, String, usize) {
    let mut items = String::from(
        "fn down(n: Int): Int {\n    if (n <= 0) { 0 } else { 1 + down(n - 1) }\n}\n\n",
    );
    for i in 0..n {
        items.push_str(&format!(
            "fn tag{i}(a: Str, k: Int, b: Str): Int {{\n    \
             if (a == b) {{ down(k) }} else {{ k + {i} }}\n}}\n\n"
        ));
    }
    let sum: Vec<String> = (0..n)
        .map(|i| format!("tag{i}(\"a{i}\", down({i}), \"b{i}\")"))
        .collect();
    let body = format!(
        "    let t = {};\n    let _ = io.println(ctx, \"${{t}}\").ignore();\n",
        sum.join(" + ")
    );
    (items, body, n * (n - 1))
}

/// **A constant argument is written where the callee reads it, and a literal
/// takes no counts.** Each literal was stored in the caller's frame, copied
/// into the callee's, and released after the call, though a literal string
/// is immortal. The constant handed to `down` was copied too. Now the call
/// writes each constant into the callee's frame itself, and nothing releases
/// a literal.
#[test]
fn constant_arguments_are_written_where_the_callee_reads_them() {
    if let Some(why) = ci::native_host_gap() {
        ci::missing("build::profile", &why);
        return;
    }
    let holes = |n: usize| {
        let (items, body, printed) = literals_around_calls(n);
        let scratch = large_shape("native", &items, &body);
        let run = scratch.run_with_env(
            &["build", "//app"],
            &[("BURI_PROFILE", "1"), ("BURI_THREADS", "1")],
        );
        run.ok();
        scratch
            .run(&["run", "//app"])
            .ok()
            .says(&format!("{printed}\n"));
        let all = run.all();
        (
            work_count(&all, "holes patched").unwrap(),
            work_count(&all, "holes copying arguments").unwrap(),
        )
    };
    let ((few, few_copied), (many, many_copied)) = (holes(8), holes(24));
    // Each call's copies were six holes. The three left copy `down`'s answer,
    // which is no constant.
    let copied = (many_copied - few_copied) / 16.0;
    assert!(
        copied <= 3.0,
        "16 more calls patched {copied:.1} more holes each copying arguments \
         ({few_copied} at 8, {many_copied} at 24)"
    );
    // Each function and its call were 70.4 holes, 6 of them releasing the two
    // literals.
    let patched = (many - few) / 16.0;
    assert!(
        patched < 62.0,
        "16 more calls patched {patched:.1} more holes each ({few} at 8, {many} at 24)"
    );
}

/// `n` functions each called with a struct and an enum built in the call,
/// and once more with constant ones, with a `main` that prints the sum of
/// their answers. Answers the items, the body and what it prints.
fn records_built_for_calls(n: usize) -> (String, String, usize) {
    let mut items = String::from(
        "fn down(n: Int): Int {\n    if (n <= 0) { 0 } else { 1 + down(n - 1) }\n}\n\n\
         struct Pair {\n    a: Int,\n    b: Int,\n}\n\n\
         enum Step {\n    By(Int),\n    Stay,\n}\n\n",
    );
    for i in 0..n {
        items.push_str(&format!(
            "fn take{i}(p: Pair, s: Step): Int {{\n    match (s) {{\n        \
             .By(k) => down(p.a) - p.b + k,\n        .Stay => {i},\n    }}\n}}\n\n"
        ));
    }
    let sum: Vec<String> = (0..n)
        .map(|i| {
            format!(
                "take{i}(Pair {{ a: down({i}) + 1, b: down(1) }}, Step.By({i})) \
                 + take{i}(Pair {{ a: 1, b: 1 }}, Step.Stay)"
            )
        })
        .collect();
    let body = format!(
        "    let t = {};\n    let _ = io.println(ctx, \"${{t}}\").ignore();\n",
        sum.join(" + ")
    );
    (items, body, n * (n - 1) + n * (n - 1) / 2)
}

/// **A record built for a call is built in the callee's frame.** A struct or
/// an enum made only to be passed was built in the caller's frame and copied
/// into the callee's. Now it is built where the callee reads it.
#[test]
fn records_built_for_calls_are_built_where_the_callee_reads_them() {
    if let Some(why) = ci::native_host_gap() {
        ci::missing("build::profile", &why);
        return;
    }
    let holes = |n: usize| {
        let (items, body, printed) = records_built_for_calls(n);
        let scratch = large_shape("native", &items, &body);
        let run = scratch.run_with_env(
            &["build", "//app"],
            &[("BURI_PROFILE", "1"), ("BURI_THREADS", "1")],
        );
        run.ok();
        scratch
            .run(&["run", "//app"])
            .ok()
            .says(&format!("{printed}\n"));
        let all = run.all();
        (
            work_count(&all, "holes patched").unwrap(),
            work_count(&all, "holes copying arguments").unwrap(),
        )
    };
    let ((few, few_copied), (many, many_copied)) = (holes(8), holes(24));
    // Each function's two calls copied 12 holes of arguments.
    let copied = (many_copied - few_copied) / 16.0;
    assert!(
        copied <= 3.0,
        "16 more functions patched {copied:.1} more holes each copying arguments \
         ({few_copied} at 8, {many_copied} at 24)"
    );
    // Each function and its two calls were 97 holes.
    let patched = (many - few) / 16.0;
    assert!(
        patched < 90.0,
        "16 more functions patched {patched:.1} more holes each ({few} at 8, {many} at 24)"
    );
}

/// `n` structs of their own type, each pair of them mapped through a closure
/// that reads both fields, with a `main` that prints the sum of the mapped
/// lists' lengths. Answers the items, the body and what it prints.
fn items_mapped_by_closures(n: usize) -> (String, String, usize) {
    let items: String = (0..n)
        .map(|i| format!("struct Item{i} {{\n    a: Int,\n    b: Str,\n}}\n\n"))
        .collect();
    let sum: Vec<String> = (0..n)
        .map(|i| {
            format!(
                "[Item{i} {{ a: t, b: \"x{i}\" }}, Item{i} {{ a: {i}, b: \"y\" }}]\
                 .map(ctx, fn(s) => s.a + s.b.length()).length()"
            )
        })
        .collect();
    let body = format!(
        "    let t = 3;\n    let u = {};\n    let _ = io.println(ctx, \"${{u}}\").ignore();\n",
        sum.join(" + ")
    );
    (items, body, 2 * n)
}

/// **A closure's argument and answer stay in its frame.** Each list item
/// `map` hands its closure was loaded into the caller's frame and copied into
/// the closure's, and the closure's answer was copied back out before it was
/// stored. Now the item is loaded where the closure reads it, and the answer
/// stored from where the closure wrote it.
#[test]
fn closure_arguments_and_answers_stay_in_the_closures_frame() {
    if let Some(why) = ci::native_host_gap() {
        ci::missing("build::profile", &why);
        return;
    }
    let holes = |n: usize| {
        let (items, body, printed) = items_mapped_by_closures(n);
        let scratch = large_shape("native", &items, &body);
        let run = scratch.run_with_env(
            &["build", "//app"],
            &[("BURI_PROFILE", "1"), ("BURI_THREADS", "1")],
        );
        run.ok();
        scratch
            .run(&["run", "//app"])
            .ok()
            .says(&format!("{printed}\n"));
        let all = run.all();
        (
            work_count(&all, "holes copying arguments").unwrap(),
            work_count(&all, "holes copying results").unwrap(),
        )
    };
    let ((few_args, few_results), (many_args, many_results)) = (holes(8), holes(24));
    // Each type's `map` copied 6 holes of arguments: 3 were the item.
    let args = (many_args - few_args) / 16.0;
    assert!(
        args <= 3.0,
        "16 more types copied {args:.1} more holes each of arguments \
         ({few_args} at 8, {many_args} at 24)"
    );
    // And 3 of results, the closure's answer.
    let results = (many_results - few_results) / 16.0;
    assert!(
        results < 1.0,
        "16 more types copied {results:.1} more holes each of results \
         ({few_results} at 8, {many_results} at 24)"
    );
}

/// A library whose eight tests each call one function generic over a
/// `Random` context: from eight contexts that bind `Random` among other
/// effects, in other orders, or from eight of the one that binds all three.
fn random_from_contexts(shapes: bool) -> Scratch {
    let scratch = Scratch::repo("profile-context-shapes");
    scratch.write(
        "lib/spin/BUILD.buri",
        "library {\n  test { sources: [\"test/spin.buri\"] }\n}\n",
    );
    scratch.write(
        "lib/spin/lib.buri",
        "from \"core/random\" import * as random;\n\
         from \"platform/effect\" import { Random };\n\n\
         export struct Mul(Int);\n\n\
         export fn mul(k: Int): Mul {\n    Mul(k)\n}\n\n\
         impl Random for Mul {\n    \
         fn nextInt(self, low: Int, high: Int): Int {\n        (low * self.0 + high) % 1000003\n    }\n\n    \
         fn nextFloat(self): Float {\n        0.5\n    }\n}\n\n\
         fn step<C: Random>(ctx: C, n: Int): Int {\n    \
         if (n == 0) { 0 } else { random.int(ctx, n, 1) + step(ctx, n - 1) }\n}\n\n\
         export fn twice<C: Random>(ctx: C): Int {\n    step(ctx, 3) + random.int(ctx, 1, 0)\n}\n",
    );
    // `R` is where `Random` goes.
    let shaped = [
        "R",
        "R, Allocator: alloc()",
        "Allocator: alloc(), R",
        "R, Stdout: stdout()",
        "Stdout: stdout(), R",
        "Allocator: alloc(), Stdout: stdout(), R",
        "R, Allocator: alloc(), Stdout: stdout()",
        "Stdout: stdout(), R, Allocator: alloc()",
    ];
    let mut tests = "from \"core/testing/assert\" import * as assert;\n\
         from \"platform/effect\" import { Allocator, Random, Stdout };\n\
         from \"platform/effect/testing\" import { alloc, stdout };\n\
         from \"//lib/spin\" import { mul, twice };\n"
        .to_string();
    for k in 1..=8 {
        let shape = shaped[if shapes { k - 1 } else { 6 }];
        let fields = shape.replace('R', &format!("Random: mul({k})"));
        // Seven times `k`, plus three.
        tests.push_str(&format!(
            "\ntest \"shape {k}\" {{\n    assert.equal(twice(context {{ {fields} }}), {});\n}}\n",
            7 * k + 3
        ));
    }
    scratch.write("lib/spin/test/spin.buri", &tests);
    scratch
}

/// **A function generic over its caller's context has one body for every
/// context that binds the same implementations of the effects it names.**
/// Each shape of context a test held made a copy of the function and of all
/// it called with that context: tests holding eight shapes made eight copies
/// of a library function. A count of the functions instantiated, which no
/// load can move. PERFORMANCE.md §6.153.
#[test]
fn a_function_generic_over_its_context_has_one_body_for_many_context_shapes() {
    if let Some(why) = ci::native_host_gap() {
        ci::missing("build::profile", &why);
        return;
    }
    let instantiated = |shapes: bool| {
        let run = random_from_contexts(shapes).run_with_env(
            &["test", "//..."],
            &[("BURI_PROFILE", "1"), ("BURI_THREADS", "1")],
        );
        run.ok().says("8 passed, 0 failed");
        work_count(&run.all(), "functions instantiated").unwrap()
    };
    let (many, one) = (instantiated(true), instantiated(false));
    assert!(
        many <= one,
        "eight shapes of context instantiated {many} functions, against {one} for one shape"
    );
}

/// `n` functions each handed another function's answer straight, a record and
/// an `Int`, twice, with a `main` that prints the sum of their answers. Every
/// body is too large and called too often to be inlined. Answers the items,
/// the body and what it prints.
fn answers_handed_to_calls(n: usize) -> (String, String, usize) {
    let mut items = String::from(
        "fn down(n: Int): Int {\n    if (n <= 0) { 0 } else { 1 + down(n - 1) }\n}\n\n\
         struct Pair {\n    a: Int,\n    b: Int,\n}\n\n",
    );
    for i in 0..n {
        items.push_str(&format!(
            "fn make{i}(k: Int): Pair {{\n    Pair {{ a: down(k) + down(k + 1), b: down({i}) }}\n}}\n\n\
             fn count{i}(k: Int): Int {{\n    down(k) * down(k + {i}) + 1\n}}\n\n\
             fn take{i}(n: Int, p: Pair): Int {{\n    down(n) + p.a - p.b\n}}\n\n"
        ));
    }
    let sum: Vec<String> = (0..n)
        .map(|i| format!("take{i}(count{i}(1), make{i}(t)) + take{i}(count{i}(2), make{i}(1))"))
        .collect();
    let body = format!(
        "    let t = down(2);\n    let u = {};\n    let _ = io.println(ctx, \"${{u}}\").ignore();\n",
        sum.join(" + ")
    );
    // The first `take` is (i + 2) + 5 - i, and the second (2i + 5) + 3 - i.
    let printed = (0..n).map(|i| 7 + (8 + i)).sum();
    (items, body, printed)
}

/// **A call's answer handed straight to another call is written where that
/// call reads it.** `take(count(1), make(t))` copied each answer out of its
/// callee's frame and then into `take`'s. Now `make` lays its frame out at
/// `take`'s parameter and writes its answer there.
#[test]
fn answers_handed_to_calls_are_written_where_the_callee_reads_them() {
    if let Some(why) = ci::native_host_gap() {
        ci::missing("build::profile", &why);
        return;
    }
    let holes = |n: usize| {
        let (items, body, printed) = answers_handed_to_calls(n);
        let scratch = large_shape("native", &items, &body);
        let run = scratch.run_with_env(
            &["build", "//app"],
            &[("BURI_PROFILE", "1"), ("BURI_THREADS", "1")],
        );
        run.ok();
        scratch
            .run(&["run", "//app"])
            .ok()
            .says(&format!("{printed}\n"));
        let all = run.all();
        work_count(&all, "holes copying arguments").unwrap()
            + work_count(&all, "holes copying results").unwrap()
    };
    let (few, many) = (holes(8), holes(24));
    // Each index copied 42 holes, 12 of them `make`'s two answers going out
    // of its frame and into `take`'s. The rest are answers read by `+`, and
    // `count`'s, which `make`'s call comes between.
    let copied = (many - few) / 16.0;
    assert!(
        copied <= 30.0,
        "16 more indices copied {copied:.1} more holes each of arguments and results \
         ({few} at 8, {many} at 24)"
    );
}

/// `n` functions over `Int`s alone, each called twice and too large to inline,
/// beside one that compares a derived record and keeps a `Str`. Answers the
/// items, the body and what it prints.
fn plain_functions(n: usize) -> (String, String, String) {
    let mut items = String::from(
        "fn down(n: Int): Int {\n    if (n <= 0) { 0 } else { 1 + down(n - 1) }\n}\n\n\
         struct Tag(Int, Str);\n\nderive Equal for Tag;\n\n\
         fn same(t: Tag, s: Str): Bool {\n    t == Tag(down(1), s)\n}\n\n",
    );
    for i in 0..n {
        items.push_str(&format!(
            "fn step{i}(k: Int): Int {{\n    down(k) * down(k + {i}) + 1\n}}\n\n"
        ));
    }
    let sum: Vec<String> = (0..n).map(|i| format!("step{i}(1) + step{i}(2)")).collect();
    let body = format!(
        "    let u = {};\n    let s = \"tag\".repeat(ctx, 2);\n    \
         let _ = io.println(ctx, \"${{u}} ${{same(Tag(1, s), s)}} ${{same(Tag(2, s), s)}}\").ignore();\n",
        sum.join(" + ")
    );
    // step(1) is (i + 1) + 1, and step(2) is 2(i + 2) + 1.
    let printed: usize = (0..n).map(|i| (i + 2) + (2 * i + 5)).sum();
    (items, body, format!("{printed} true false\n"))
}

/// **A function with no structural call, no match and nothing borrowed costs
/// the native middle passes no walk, and `rc` reads its calls and effects in
/// one.** `derives` walked every body to replace structural calls,
/// `decision` every body to rewrite its matches, and `rc` every body for the
/// parameters it consumes. Now each walks only the bodies with something to
/// find. `rc` also walked each body once for its calls and
/// again, per round of a fixpoint, for its effects, and `derives` walked each
/// once for its structural calls and again for the types it writes.
#[test]
fn functions_with_nothing_to_derive_or_borrow_are_not_walked_for_it() {
    if let Some(why) = ci::native_host_gap() {
        ci::missing("build::profile", &why);
        return;
    }
    let counts = |n: usize| {
        let (items, body, printed) = plain_functions(n);
        let scratch = large_shape("native", &items, &body);
        let run = scratch.run_with_env(
            &["build", "//app"],
            &[("BURI_PROFILE", "1"), ("BURI_THREADS", "1")],
        );
        run.ok();
        scratch.run(&["run", "//app"]).ok().says(&printed);
        let all = run.all();
        [
            work_count(&all, "bodies rewritten for derives").unwrap(),
            work_count(&all, "bodies walked for consuming uses").unwrap(),
            work_count(&all, "bodies walked for decisions").unwrap(),
            work_count(&all, "bodies walked for calls and effects").unwrap(),
            work_count(&all, "bodies walked for derives").unwrap(),
        ]
    };
    let (few, many) = (counts(8), counts(24));
    assert!(
        many[..3] == few[..3],
        "16 more `Int` functions walked more bodies: {few:?} at 8, {many:?} at 24"
    );
    // About one walk each (17 for the 16). Walking for effects apart from
    // calls, or for types apart from structural calls, made it at least two.
    for (k, what) in [(3, "its calls and effects"), (4, "derives")] {
        let walked = (many[k] - few[k]) / 16.0;
        assert!(
            walked <= 1.5,
            "each of 16 more `Int` functions was walked {walked:.1} times for {what}"
        );
    }
}

/// A function whose first statement calls a one-line function, followed by
/// `n` statements that call nothing small enough to paste.
fn early_call(n: usize) -> (String, String, String) {
    let lets: String = (1..=n)
        .map(|i| format!("    let t{i} = down(t{}) + {i};\n", i - 1))
        .collect();
    let items = format!(
        "fn down(n: Int): Int {{\n    if (n <= 0) {{ 0 }} else {{ 1 + down(n - 1) }}\n}}\n\n\
         fn twice(k: Int): Int {{\n    k * 2\n}}\n\n\
         fn long(k: Int): Int {{\n    let t0 = twice(k);\n{lets}    t{n}\n}}\n"
    );
    // Called twice, so `long` is never pasted into `main` itself.
    let body = "    let _ = io.println(ctx, \"${long(3) + long(4)}\").ignore();\n".to_string();
    // `down` of a positive number is that number, so each statement adds `i`.
    let printed = 14 + n * (n + 1);
    (items, body, format!("{printed}\n"))
}

/// **The inliner stops walking a body after the last call it could paste.**
/// A pasted body is not walked again in the same round, so nothing after the
/// last such call can change, and its statements are not walked at all.
#[test]
fn a_body_is_walked_for_inlining_only_up_to_its_last_small_call() {
    let walked = |n: usize| {
        let (items, body, printed) = early_call(n);
        let scratch = large_shape("node", &items, &body);
        let run = scratch.run_with_env(
            &["build", "//app"],
            &[("BURI_PROFILE", "1"), ("BURI_THREADS", "1")],
        );
        run.ok();
        scratch.run(&["run", "//app"]).ok().says(&printed);
        work_count(&run.all(), "nodes walked for inlining").unwrap()
    };
    let (few, many) = (walked(8), walked(40));
    assert!(
        many == few,
        "32 more statements after the last small call were walked: {few} at 8, {many} at 40"
    );
}

/// `n` structs deriving `Equal`, each compared by its own generic function: `n`
/// calls of `==` on a type parameter, and `n` more `impl`s of `Equal` they
/// may reach.
fn generic_equals(n: usize) -> (String, String) {
    let items: String = (0..n)
        .map(|i| {
            format!(
                "struct S{i}(Int);\n\nderive Equal for S{i};\n\n\
                 fn same{i}<T: Equal>(a: T, b: T): Int {{\n    \
                 if (a == b) {{ {i} }} else {{ 0 }}\n}}\n\n"
            )
        })
        .collect();
    let calls: Vec<String> = (0..n)
        .map(|i| format!("same{i}(S{i}(k), S{i}(k))"))
        .collect();
    let body = format!(
        "    let k = 3;\n    let _ = io.println(ctx, \"${{{}}}\").ignore();\n",
        calls.join(" + ")
    );
    (items, body)
}

/// **The search for endless instantiations lists a method's `impl`s once.**
/// At each call of a trait's method on a type parameter it looked through
/// every `impl` of the trait, derived ones included, for those with the
/// method: a program with `n` such calls and `n` derived `impl`s looked
/// through `n` squared. On a 48-library workspace that was a sixteenth of
/// checking. Doubling both now about doubles the `impl`s looked through.
#[test]
fn endless_search_lists_a_methods_impls_once() {
    if let Some(why) = ci::native_host_gap() {
        ci::missing("build::profile", &why);
        return;
    }
    each_grows_linearly(
        "generic `==` on derived structs",
        &["impls sought for endless calls"],
        "native",
        generic_equals,
        32,
    );
    let (items, body) = generic_equals(4);
    let scratch = large_shape("native", &items, &body);
    scratch.run(&["run", "//app"]).ok().says("6");
}

/// A generic function comparing its own `T` once, and `n` structs of fixed
/// types, each with itself.
fn fixed_equals(n: usize) -> (String, String) {
    let structs: String = (0..n)
        .map(|i| format!("struct S{i}(Int);\n\nderive Equal for S{i};\n\n"))
        .collect();
    let checks: String = (0..n)
        .map(|i| format!("    let c{i} = if (S{i}(k) == S{i}(k)) {{ k }} else {{ 0 }};\n"))
        .collect();
    let sum: Vec<String> = (0..n).map(|i| format!("c{i}")).collect();
    let items = format!(
        "{structs}fn count<T: Equal>(a: T, b: T, k: Int): Int {{\n{checks}    \
         let t = if (a == b) {{ 1 }} else {{ 0 }};\n    {} + t\n}}\n",
        sum.join(" + ")
    );
    let body = "    let _ = io.println(ctx, \"${count(2, 2, 3)}\").ignore();\n".to_string();
    (items, body)
}

/// **The search for endless instantiations follows only trait calls that
/// mention a type parameter.** A call at fixed types is an edge from none of
/// the caller's parameters, whichever `impl` it runs, yet each was taken apart
/// into the `impl`s it could reach. However many fixed comparisons the generic
/// function makes, the search follows its one call at `T`.
#[test]
fn endless_search_follows_only_trait_calls_on_type_parameters() {
    if let Some(why) = ci::native_host_gap() {
        ci::missing("build::profile", &why);
        return;
    }
    let (items, body) = fixed_equals(16);
    let followed = counted_work(
        "native",
        &items,
        &body,
        "trait calls followed for endless calls",
    );
    assert_eq!(followed, Some(1.0), "16 fixed comparisons and one at `T`");
    let scratch = large_shape("native", &items, &body);
    scratch.run(&["run", "//app"]).ok().says("49");
}
