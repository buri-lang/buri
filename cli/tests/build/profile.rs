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

/// A variant of `n` strings, two of them taken apart, and an `else if` chain
/// comparing them field by field.
fn long_branching(n: usize) -> (String, String) {
    let types = vec!["Str"; n].join(", ");
    let made = vec!["s"; n].join(", ");
    let left: Vec<String> = (0..n).map(|i| format!("a{i}")).collect();
    let right: Vec<String> = (0..n).map(|i| format!("b{i}")).collect();
    let chain: Vec<String> = (0..n).map(|i| format!("if (a{i} != b{i}) {{ {i} }}")).collect();
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
            profiled("native", &items, &body, "middle")
        },
        400,
    );
}

/// A struct of `n` fields, alternately `Int` and `Str`, that derives `Hash`
/// and is hashed.
fn wide_hashed_struct(n: usize) -> (String, String) {
    let types: Vec<&str> = (0..n).map(|i| if i % 2 == 0 { "Int" } else { "Str" }).collect();
    let values: Vec<String> =
        (0..n).map(|i| if i % 2 == 0 { format!("k + {i}") } else { String::from("s") }).collect();
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
            profiled("native", &items, &body, "middle")
        },
        400,
    );
}

/// A variant of `n` fields, alternately `Int` and `Str`, built and taken apart.
fn wide_variant(n: usize) -> (String, String) {
    let types: Vec<&str> = (0..n).map(|i| if i % 2 == 0 { "Int" } else { "Str" }).collect();
    let values: Vec<String> =
        (0..n).map(|i| if i % 2 == 0 { format!("k + {i}") } else { String::from("s") }).collect();
    let names: Vec<String> = (0..n).map(|i| format!("a{i}")).collect();
    let summed: Vec<String> = (0..n)
        .map(|i| if i % 2 == 0 { format!("a{i}") } else { format!("a{i}.length()") })
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
    grows_linearly(
        "emitting a wide variant's functions",
        |n| {
            let (items, body) = wide_variant(n);
            profiled("native", &items, &body, "emit")
        },
        1000,
    );
}

/// A body of `n` `let`s, each calling a function by a name no local has.
fn many_lets(n: usize) -> (String, String) {
    let lets: String = (0..n).map(|i| format!("    let t{} = step(t{i}, {i});\n", i + 1)).collect();
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
            profiled("node", &items, &body, "check")
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
            phase_instructions(&run.all(), "other")
        },
        2000,
    );
}

/// One block of `n` `let`s naming a field of a parameter, each read once.
fn field_lets(n: usize) -> (String, String) {
    let lets: String = (0..n)
        .map(|i| {
            let field = if i % 2 == 0 { "b" } else { "a" };
            format!("    let v{i} = p.{field};\n    let t{} = (t{i} + v{i} * {}) % 1013;\n", i + 1, i % 7 + 1)
        })
        .collect();
    let items = format!(
        "struct P {{\n    a: Int,\n    b: Int,\n}}\n\n\
         fn long(p: P): Int {{\n    let t0 = 0;\n{lets}    t{n}\n}}\n"
    );
    let body = String::from("    let _ = io.println(ctx, \"${long(P { a: 3, b: 4 })}\").ignore();\n");
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
            profiled("native", &items, &body, "middle")
        },
        1000,
    );
}

/// One function of `n` capturing lambdas, each called.
fn many_lambdas(n: usize) -> (String, String) {
    let lambdas: String = (0..n)
        .map(|i| format!("    let k{i} = x + {i};\n    let f{i} = fn(y: Int): Int => (y * k{i}) % 1013;\n"))
        .collect();
    let sums: String =
        (0..n).map(|i| format!("    let t{} = (t{i} + f{i}({i})) % 100003;\n", i + 1)).collect();
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
            profiled("native", &items, &body, "middle")
        },
        1000,
    );
}

/// `buri lint //...` over a repository `write` fills in, answering `phase`'s
/// instructions.
fn linted(name: &str, write: impl Fn(&Scratch), phase: &str) -> Option<f64> {
    let scratch = Scratch::repo(name);
    write(&scratch);
    let run = scratch.run_with_env(&["lint", "//..."], &[("BURI_PROFILE", "1")]);
    run.ok();
    phase_instructions(&run.all(), phase)
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
            None => (String::new(), String::from("export fn total(): Int {\n    1\n}\n")),
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
        |n| linted("profile-lint-chain", |s| library_chain(s, n), "check"),
        60,
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
    scratch.write("lib/big/BUILD.buri", "library {\n    visibility: [\"//visibility:public\"]\n}\n");
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
        |n| linted("profile-lint-long-file", |s| many_declarations(s, n), "other"),
        400,
    );
}
