//! `buri test --release`: Buri test suites built by LLVM through the `buri`
//! binary, under the heap check.
//!
//! `release/` is a repository of suites, and one program, written for what the
//! release backend emits: 128-bit arithmetic, conversions at every width, values
//! too wide for registers. The conformance corpus covers the rest, and the files
//! of it that `coverage_agrees` leaves on JavaScript run here.

use std::path::{Path, PathBuf};
use std::process::Command;

fn copy(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        if entry.file_name() == ".buri" {
            continue;
        }
        let dest = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy(&entry.path(), &dest);
        } else {
            std::fs::copy(entry.path(), dest).unwrap();
        }
    }
}

/// A fresh directory for `name`, under this process's own.
fn scratch(name: &str) -> PathBuf {
    crate::sweep::once();
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("native-release-{}", std::process::id()))
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// `buri test --release` over `targets` in `repo`, which must pass whole.
fn passes(repo: &Path, targets: &[&str]) -> String {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_buri"));
    cmd.current_dir(repo)
        .args(["test", "--release"])
        .args(targets);
    let ran = crate::shared::ran_command(crate::shared::heap_checked(&mut cmd));
    assert!(
        ran.status == 0 && ran.stdout.contains(" passed, 0 failed, 0 skipped ("),
        "buri test --release {targets:?}:\n{}\n{}",
        ran.stdout,
        ran.stderr
    );
    ran.stdout
}

macro_rules! skip_unless_executable {
    () => {
        if let Some(why) = crate::llvm::can_execute() {
            crate::ci::skipped("llvm", why);
            return;
        }
    };
}

/// **Every suite in `release/` passes.**
#[test]
fn the_release_suites_pass() {
    skip_unless_executable!();
    let repo = scratch("suites");
    copy(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/native/release"),
        &repo,
    );
    passes(&repo, &["//lib/..."]);
}

/// **A program fans wide values out over the host's threads.** The host's
/// implementations hold no bytes, so each task's context is nothing at all.
#[test]
fn a_program_fans_out_wide_values_on_the_host() {
    skip_unless_executable!();
    let repo = scratch("fanout");
    copy(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/native/release"),
        &repo,
    );
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_buri"));
    cmd.current_dir(&repo)
        .args(["run", "--release", "//cmd/fanout"]);
    let ran = crate::shared::ran_command(crate::shared::heap_checked(&mut cmd));
    assert_eq!(
        (ran.status, ran.stdout.as_str()),
        (0, "a2 b5 c8\n"),
        "{}",
        ran.stderr
    );
}

/// **A program fans a context that owns a count out over the host's
/// threads**, and every task's count on it is given back.
#[test]
fn a_program_fans_out_a_counted_context_on_the_host() {
    skip_unless_executable!();
    let repo = scratch("counted");
    copy(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/native/release"),
        &repo,
    );
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_buri"));
    cmd.current_dir(&repo)
        .args(["run", "--release", "//cmd/counted"]);
    let ran = crate::shared::ran_command(crate::shared::heap_checked(&mut cmd));
    assert_eq!(
        (ran.status, ran.stdout.as_str()),
        (0, "64 .Some(5)\n"),
        "{}",
        ran.stderr
    );
}

/// **The native set's files in a package that also holds a JavaScript-only
/// one pass under `--release`.** `coverage_agrees` moves a corpus package to
/// the native backends whole or not at all, so `numbers/integers.buri` stays
/// on JavaScript there for `numbers/floats.buri`'s sake. Here each such
/// package keeps only its native-set files and moves.
#[cfg(feature = "backend-stencil")]
#[test]
fn the_native_files_of_a_mixed_package_pass_under_release() {
    skip_unless_executable!();
    let corpus = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/conformance");
    let repo = scratch("mixed");
    std::fs::create_dir_all(repo.join("lib")).unwrap();
    std::fs::copy(corpus.join("REPO.buri"), repo.join("REPO.buri")).unwrap();
    let mut targets = Vec::new();
    for package in crate::conformance::javascript_only_packages() {
        let mut kept = Vec::new();
        for file in std::fs::read_dir(corpus.join("lib").join(package).join("test")).unwrap() {
            let name = file.unwrap().file_name().to_string_lossy().to_string();
            if crate::conformance::excluded_because(&format!("{package}/{name}")).is_none() {
                kept.push(format!("\"test/{name}\""));
            }
        }
        if kept.is_empty() {
            continue;
        }
        kept.sort();
        let at = repo.join("lib").join(package);
        copy(&corpus.join("lib").join(package), &at);
        let build = std::fs::read_to_string(at.join("BUILD.buri")).unwrap();
        // The test block's own `sources`, the second list in the file.
        let (head, rest) = build.split_at(build.find("test {").expect("a test block"));
        let open = rest.find("sources: [").unwrap() + "sources: [".len();
        let close = open + rest[open..].find(']').unwrap();
        let rest = format!("{}{}{}", &rest[..open], kept.join(", "), &rest[close..]);
        let build = format!("{head}{rest}").replace("backends: [JS]", "backends: [NATIVE]");
        std::fs::write(at.join("BUILD.buri"), build).unwrap();
        targets.push(format!("//lib/{package}"));
    }
    assert!(
        targets.len() >= 3,
        "fewer mixed packages than expected: {targets:?}"
    );
    let targets: Vec<&str> = targets.iter().map(String::as_str).collect();
    passes(&repo, &targets);
}

/// **A value nested seventy structs deep, with a `Str` at the bottom, keeps its
/// count.** Generated, because the source is the same line seventy times.
#[test]
fn a_value_nested_seventy_structs_deep_keeps_its_count() {
    skip_unless_executable!();
    const DEPTH: usize = 70;
    let repo = scratch("deep");
    let at = repo.join("lib/deep");
    std::fs::create_dir_all(at.join("test")).unwrap();
    let release = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/native/release");
    std::fs::copy(release.join("REPO.buri"), repo.join("REPO.buri")).unwrap();
    std::fs::copy(release.join("lib/wide/BUILD.buri"), at.join("BUILD.buri")).unwrap();
    std::fs::write(at.join("lib.buri"), "").unwrap();
    let mut source = String::from(
        "from \"core/str\" import * as str;\n\
         from \"core/testing/assert\" import * as assert;\n\
         from \"platform/effect\" import { Allocator };\n\
         from \"platform/effect/testing\" import { alloc };\n\n\
         struct S0 { s: Str }\n",
    );
    let mut value = "S0 { s: str.format(ctx, \"${7}\") }".to_string();
    for i in 1..=DEPTH {
        source.push_str(&format!("struct S{i} {{ a: S{} }}\n", i - 1));
        value = format!("S{i} {{ a: {value} }}");
    }
    source.push_str(&format!(
        "\ntest \"deep\" {{\n    let ctx = context {{ Allocator: alloc() }};\n    \
         let d = {value};\n    assert.equal(d{}.s, \"7\");\n}}\n",
        ".a".repeat(DEPTH)
    ));
    std::fs::write(at.join("test/wide.buri"), source).unwrap();
    passes(&repo, &["//lib/deep"]);
}

/// The library every module of [`shared_rows`] builds rows with: a record of a
/// dozen fields, each an enum holding strings, so releasing one is a long walk.
const SHAPE: &str = r#"from "core/str" import * as str;
from "platform/effect" import { Allocator };

export enum Leaf {
    Word(Str),
    Pair(Str, Str),
    Many([Str]),
    Nothing,
}

export struct Row {
    a: Leaf,
    b: Leaf,
    export c: Leaf,
    d: Leaf,
    e: Leaf,
    f: Leaf,
    g: Leaf,
    h: Leaf,
    i: Leaf,
    j: Leaf,
    k: Leaf,
    l: Leaf,
FIELDS}

export struct Extra {
    rows: [[Row]],
    export name: Str,
}

fn word<C: Allocator>(ctx: C, n: Int): Leaf {
    .Word(str.format(ctx, "w${n}"))
}

fn pair<C: Allocator>(ctx: C, n: Int): Leaf {
    .Pair(str.format(ctx, "p${n}"), str.format(ctx, "q${n}"))
}

fn many<C: Allocator>(ctx: C, n: Int): Leaf {
    .Many([str.format(ctx, "m${n}"), str.format(ctx, "n${n}")])
}

export fn row<C: Allocator>(ctx: C, n: Int): Row {
    Row {
        a: word(ctx, n),
        b: pair(ctx, n),
        c: many(ctx, n),
        d: .Nothing,
        e: word(ctx, n + 1),
        f: pair(ctx, n + 1),
        g: many(ctx, n + 1),
        h: .Nothing,
        i: word(ctx, n + 2),
        j: pair(ctx, n + 2),
        k: many(ctx, n + 2),
        l: .Nothing,
VALUES    }
}

export fn extra<C: Allocator>(ctx: C, n: Int): Extra {
    Extra { rows: [[row(ctx, n)], [row(ctx, n + 1)]], name: str.format(ctx, "x") }
}

export fn width(leaf: Leaf): Int {
    match (leaf) {
        .Word(w) => w.length(),
        .Pair(p, q) => p.length() + q.length(),
        .Many(m) => m.length(),
        .Nothing => 0,
    }
}
"#;

/// How many more fields [`SHAPE`]'s `Row` has, each of its own enum type. The
/// `j`th holds `j + 1` strings, so no two types release alike.
const MORE_FIELDS: usize = 24;

/// [`SHAPE`], with its `Row`'s [`MORE_FIELDS`] filled in.
fn shape_source() -> String {
    let fields: String = (0..MORE_FIELDS)
        .map(|j| format!("    x{j}: L{j},\n"))
        .collect();
    let values: String = (0..MORE_FIELDS)
        .map(|j| format!("        x{j}: .Nothing,\n"))
        .collect();
    let types: String = (0..MORE_FIELDS)
        .map(|j| {
            format!(
                "\nenum L{j} {{\n    Words({}),\n    Many([Str]),\n    Nothing,\n}}\n",
                vec!["Str"; j + 1].join(", ")
            )
        })
        .collect();
    format!(
        "{}{types}",
        SHAPE.replace("FIELDS", &fields).replace("VALUES", &values)
    )
}

/// Module `i`'s function: it builds rows and drops them, and answers 4. With
/// `extra`, it also drops an `Extra`, which no other module does, and
/// answers 5.
fn count_fn(i: usize, extra: bool) -> String {
    let more = if extra {
        "    let more = shape.extra(ctx, n);\n    let n = n + more.name.length();\n"
    } else {
        "    let n = n + 0;\n"
    };
    format!(
        "export fn count{i}<C: Allocator>(ctx: C, n: Int): Int {{\n{more}    \
         let rows = [shape.row(ctx, n), shape.row(ctx, n + {i})];\n    \
         rows.length() + shape.width(shape.row(ctx, n).c) + n - {i}\n}}\n"
    )
}

/// A repository whose binary `//cmd/app` calls `count{i}` for each of
/// `modules`, a list of the functions each library module holds, by `i` and
/// whether it drops an `Extra`. It prints their sum.
fn shared_rows(name: &str, modules: &[Vec<(usize, bool)>]) -> PathBuf {
    let repo = scratch(name);
    let release = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/native/release");
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::copy(release.join("REPO.buri"), repo.join("REPO.buri")).unwrap();
    let public = "library {\n    visibility: [\"//visibility:public\"]\n";
    std::fs::create_dir_all(repo.join("lib/shape")).unwrap();
    std::fs::write(repo.join("lib/shape/BUILD.buri"), format!("{public}}}\n")).unwrap();
    std::fs::write(repo.join("lib/shape/lib.buri"), shape_source()).unwrap();
    write_counts(&repo, modules);
    let deps: Vec<String> = (0..modules.len())
        .map(|m| format!("\"//lib/m{m}\""))
        .collect();
    let imports: String = (0..modules.len())
        .map(|m| format!("from \"//lib/m{m}\" import * as m{m};\n"))
        .collect();
    let calls: String = modules
        .iter()
        .enumerate()
        .flat_map(|(m, fns)| {
            fns.iter()
                .map(move |(i, _)| format!(" + m{m}.count{i}(host.alloc, {i})"))
        })
        .collect();
    std::fs::create_dir_all(repo.join("cmd/app")).unwrap();
    std::fs::write(
        repo.join("cmd/app/BUILD.buri"),
        format!(
            "binary {{\n    dependencies: [{}]\n    outputs: [\n        {{ platform: \"native\" }},\n    ]\n}}\n",
            deps.join(", ")
        ),
    )
    .unwrap();
    std::fs::write(
        repo.join("cmd/app/main.buri"),
        format!(
            "from \"native\" import {{ NativeHost }};\nfrom \"core/io\" import * as io;\n{imports}\n\
             export fn main(host: NativeHost): Result<(), Str> {{\n    let total = 0{calls};\n    \
             let _ = io.println(host.stdout, \"${{total}}\").ignore();\n    .Ok(())\n}}\n"
        ),
    )
    .unwrap();
    repo
}

/// [`shared_rows`]'s library modules, written over whatever is there.
fn write_counts(repo: &Path, modules: &[Vec<(usize, bool)>]) {
    let public = "library {\n    dependencies: [\"//lib/shape\"]\n    visibility: [\"//visibility:public\"]\n}\n";
    for (m, fns) in modules.iter().enumerate() {
        let at = repo.join(format!("lib/m{m}"));
        std::fs::create_dir_all(&at).unwrap();
        std::fs::write(at.join("BUILD.buri"), public).unwrap();
        let body: String = fns.iter().map(|&(i, extra)| count_fn(i, extra)).collect();
        std::fs::write(
            at.join("lib.buri"),
            format!(
                "from \"//lib/shape\" import * as shape;\nfrom \"platform/effect\" import {{ Allocator }};\n\n{body}"
            ),
        )
        .unwrap();
    }
}

/// `buri run --release //cmd/app` in `repo`, under the heap check: what it
/// printed.
fn run_app(repo: &Path) -> String {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_buri"));
    cmd.current_dir(repo)
        .args(["run", "--release", "//cmd/app"]);
    let ran = crate::shared::ran_command(crate::shared::heap_checked(&mut cmd));
    assert_eq!(ran.status, 0, "{}\n{}", ran.stdout, ran.stderr);
    ran.stdout
}

/// `K` modules, one function each, none dropping an `Extra`.
fn one_each(k: usize) -> Vec<Vec<(usize, bool)>> {
    (0..k).map(|i| vec![(i, false)]).collect()
}

/// **Modules that each release the same records give every block back.**
/// Each of eight modules builds lists of one library's records and drops
/// them, so all eight release the same types.
#[test]
fn modules_releasing_one_libraries_records_give_every_block_back() {
    skip_unless_executable!();
    let repo = shared_rows("shared-rows", &one_each(8));
    assert_eq!(run_app(&repo), "32\n");
}

/// **A rebuild after a module starts or stops releasing a type no other
/// module does still runs.** The other modules' objects come from the cache.
#[test]
fn a_module_that_starts_and_stops_releasing_a_type_rebuilds_and_runs() {
    skip_unless_executable!();
    let repo = shared_rows("shared-rows-edit", &one_each(4));
    assert_eq!(run_app(&repo), "16\n");
    let mut edited = one_each(4);
    edited[2] = vec![(2, true)];
    write_counts(&repo, &edited);
    assert_eq!(run_app(&repo), "17\n");
    write_counts(&repo, &one_each(4));
    assert_eq!(run_app(&repo), "16\n");
}

/// The `emit` phase's instructions, in millions, off a `BURI_PROFILE=1`
/// report, or `None` where the platform has no per-thread counter.
fn emit_instructions(repo: &Path) -> Option<f64> {
    let out = Command::new(env!("CARGO_BIN_EXE_buri"))
        .current_dir(repo)
        .args(["build", "--release", "//cmd/app"])
        .env("BURI_PROFILE", "1")
        .env("BURI_THREADS", "1")
        .output()
        .unwrap();
    let all = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(out.status.success(), "{all}");
    let line = all.lines().find(|l| l.trim_start().starts_with("emit "))?;
    let count = line.split_whitespace().nth(1)?.parse::<f64>().ok()?;
    (count > 0.0).then_some(count)
}

/// **The code that releases a type is compiled once a program, however many
/// modules release it.** Eight functions that drop the same records cost about
/// as much to emit spread over eight modules as all in one, where every module
/// compiled its own copy of the release code before. Counted in instructions,
/// on one thread.
#[test]
fn spreading_functions_over_modules_compiles_their_release_code_once() {
    skip_unless_executable!();
    let spread = shared_rows("glue-spread", &one_each(8));
    let joined = shared_rows("glue-joined", &[(0..8).map(|i| (i, false)).collect()]);
    let (Some(spread), Some(joined)) = (emit_instructions(&spread), emit_instructions(&joined))
    else {
        return;
    };
    assert!(
        spread <= joined * 1.28,
        "eight modules took {spread:.0} M instructions to emit, {:.2} times the {joined:.0} M \
         that one module with the same functions took",
        spread / joined
    );
}
