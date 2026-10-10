//! Loading a compilation: following imports from the entry point to every
//! module it reaches.
//!
//! Many modules naming one path is the common shape, so each claim here is
//! about a path that more than one import line names.
use crate::harness::*;

/// `//app`, whose `main.buri` imports `a.buri` and `b.buri`, which import the
/// same three paths: a library, the library's file spelled out, and `extra`.
fn app(extra: &str) -> Scratch {
    let scratch = Scratch::repo("loading");
    scratch.write("lib/shared/BUILD.buri", "library {\n    visibility: [\"//...\"]\n}\n");
    scratch.write("lib/shared/lib.buri", "export fn base(): Int {\n    40\n}\n");
    scratch.write(
        "app/BUILD.buri",
        "binary {\n    dependencies: [\"//lib/shared\"]\n    outputs: [{ platform: \"node\" }]\n}\n",
    );
    for name in ["a", "b"] {
        scratch.write(
            &format!("app/{name}.buri"),
            &format!(
                "from \"//lib/shared\" import {{ base }};\n\
                 from \"//lib/shared/lib.buri\" import {{ base as same }};\n\
                 {extra}\n\
                 export fn {name}(): Int {{\n    base() + same() - 39\n}}\n"
            ),
        );
    }
    scratch.write(
        "app/main.buri",
        "from \"node\" import { NodeHost };\n\
         from \"platform/effect\" import { Stdout };\n\
         from \"core/io\" import * as io;\n\
         from \"//app/a.buri\" import { a };\n\
         from \"//app/b.buri\" import { b };\n\n\
         export fn main(host: NodeHost): Result<(), Str> {\n    \
         let ctx = context { Stdout: host.stdout };\n    \
         let _ = io.println(ctx, \"${a() + b()}\").ignore();\n    \
         .Ok(())\n}\n",
    );
    scratch
}

/// Every import of a path that names nothing is reported where it's written,
/// however many modules write it.
#[test]
fn each_import_of_a_missing_module_is_reported_where_it_is_written() {
    for (path, problem) in [
        ("//lib/missing", "\"//lib/missing\" is in no package of this repository"),
        ("//lib/shared/gone.buri", "\"//lib/shared/gone.buri\" names no file (lib/shared/gone.buri)"),
    ] {
        let scratch = app(&format!("from \"{path}\" import {{ gone }};"));
        let run = scratch.run(&["build", "//app"]);
        run.exits(1);
        for file in ["app/a.buri:3:6", "app/b.buri:3:6"] {
            run.says(file);
        }
        let reported = strip_ansi(&run.all()).matches(&format!("{problem} [unknown-module]")).count();
        assert_eq!(reported, 2, "want one report per import of {path}:\n{}", indent(&run.all()));
    }
}

/// A module that several modules import, under two spellings, is one module:
/// the program builds and runs.
#[test]
fn a_module_imported_from_many_places_and_spellings_is_one_module() {
    let scratch = app("");
    scratch.run(&["build", "//app"]).ok();
    scratch.exec_js("app").ok().says("82");
}

/// `//wide`, whose `main.buri` imports `m0.buri` to `m7.buri` and prints the
/// sum of what each one's `f<i>()` returns. `module(i)` writes `m<i>.buri`.
/// A module's imports are read side by side, so these eight are.
fn wide(module: impl Fn(usize) -> Vec<u8>) -> Scratch {
    let scratch = Scratch::repo("loading-wide");
    scratch.write("wide/BUILD.buri", JS_BINARY);
    let mut imports = String::new();
    let mut calls = Vec::new();
    for i in 0..8 {
        std::fs::write(scratch.path(&format!("wide/m{i}.buri")), module(i)).unwrap();
        imports.push_str(&format!("from \"//wide/m{i}.buri\" import {{ f{i} }};\n"));
        calls.push(format!("f{i}()"));
    }
    scratch.write(
        "wide/main.buri",
        &format!(
            "from \"node\" import {{ NodeHost }};\n\
             from \"platform/effect\" import {{ Stdout }};\n\
             from \"core/io\" import * as io;\n{imports}\n\
             export fn main(host: NodeHost): Result<(), Str> {{\n    \
             let ctx = context {{ Stdout: host.stdout }};\n    \
             let _ = io.println(ctx, \"${{{}}}\").ignore();\n    \
             .Ok(())\n}}\n",
            calls.join(" + ")
        ),
    );
    scratch
}

fn returning(i: usize) -> Vec<u8> {
    format!("export fn f{i}(): Int {{\n    {i}\n}}\n").into_bytes()
}

/// **Modules read together load as they would one at a time**: the program
/// runs, a syntax error is reported in the file it is in, and a file that
/// can't be read is reported where it is imported.
#[test]
fn modules_read_side_by_side_load_as_they_would_one_at_a_time() {
    let scratch = wide(returning);
    scratch.run(&["build", "//wide"]).ok();
    scratch.exec_js("wide").ok().says("28");

    let broken = wide(|i| match i {
        2 => b"export fn f2(): Int {\n    let = 2;\n}\n".to_vec(),
        5 => b"export fn f5(: Int {\n    5\n}\n".to_vec(),
        _ => returning(i),
    });
    let run = broken.run(&["build", "//wide"]);
    run.exits(1);
    run.says("wide/m2.buri:2:9").says("wide/m5.buri:1:14");

    let unreadable = wide(|i| if i == 3 { b"export fn f3(): Int { \xff }\n".to_vec() } else { returning(i) });
    let run = unreadable.run(&["build", "//wide"]);
    run.exits(1);
    run.says("cannot read wide/m3.buri").says("wide/main.buri:7:6");
}

// ---------------------------------------------------------------------------
// What parsing and loading cost a line, on every pinned shape
// ---------------------------------------------------------------------------

/// Each pinned point, and the instructions a line `parser::parse` and a load
/// may retire on it in this test build. A load reads each file, resolves each
/// import and parses.
///
/// Counted rather than timed (`design/PERFORMANCE.md` §8), at 30,000 lines a
/// shape. Each bound is a tenth over what parsing measured, and a quarter over
/// what loading did, because the kernel's work opening files moves by a tenth
/// between runs. §6.83 has the release build's figures.
const BOUNDS: [(&str, u64, u64); 20] = [
    ("comment-free", 2340, 4490),
    ("comment-heavy", 1400, 2650),
    ("derive-heavy", 1880, 3760),
    ("enum-heavy", 1880, 3580),
    ("generic-blowup", 2160, 4130),
    ("generic-free", 1920, 3700),
    ("impl-heavy", 1760, 3500),
    ("list-heavy", 2010, 3820),
    ("long-bodies", 1900, 3800),
    ("long-idents", 1780, 3420),
    ("match-heavy", 1820, 3740),
    ("mixed", 1950, 3750),
    ("mixed-deep-graph", 1950, 3820),
    ("mixed-few-files", 1840, 2470),
    ("mixed-libs", 1960, 3890),
    ("mixed-many-files", 2300, 8580),
    ("mixed-wide-graph", 2010, 4140),
    ("string-heavy", 2140, 4140),
    ("struct-heavy", 1490, 3000),
    ("struct-light", 1970, 3770),
];

/// The pinned point `name`, regenerated from its manifest at 30,000 lines.
fn pinned_shape(name: &str) -> crate::generate::Program {
    let manifest = repo_root().join(format!("cli/benches/pinned/{name}-1M.txt"));
    let text = std::fs::read_to_string(&manifest).unwrap();
    let field = |key: &str| {
        let prefix = format!("{key} = ");
        text.lines().find_map(|l| l.strip_prefix(&prefix)).unwrap_or_default().to_string()
    };
    let (_, profile) = crate::generate::profile(&field("profile")).unwrap();
    let mut params = crate::generate::Params { shape: profile.shape, ..crate::generate::Params::default() };
    for pair in field("params").split_whitespace() {
        let (k, v) = pair.split_once('=').unwrap();
        params.set(k, v).unwrap();
    }
    params.set("lines", "30000").unwrap();
    crate::generate::program(&params)
}

/// The fewer instructions of two runs of `work`, by `counter`, or `None`
/// where the kernel counts none (Linux, and CI's virtual machines).
fn counted<T>(counter: fn() -> u64, mut work: impl FnMut() -> T) -> Option<u64> {
    let mut fewest = u64::MAX;
    for _ in 0..2 {
        let before = counter();
        std::hint::black_box(work());
        fewest = fewest.min(counter().saturating_sub(before));
    }
    (fewest > 0).then_some(fewest)
}

/// What `parser::parse` over every module, and a load of the whole program,
/// retire a line on the pinned point `name`.
fn costs(name: &str) -> Option<(u64, u64)> {
    use buri::diagnostics::{Diagnostics, FileId, Severity, SourceMap};
    let program = pinned_shape(name);
    let lines = program.lines() as u64;
    let parse = counted(buri::profile::thread_instructions, || {
        let parse = |(i, m): (usize, &crate::generate::Module)| buri::parsing::parser::parse(&m.text, FileId(i as u32));
        program.modules.iter().enumerate().map(parse).collect::<Vec<_>>()
    })?;
    let scratch = Scratch::repo(&format!("cost-{name}"));
    scratch.write("bench/BUILD.buri", JS_BINARY);
    for m in &program.modules {
        scratch.write(&format!("bench/{}", m.path.rsplit('/').next().unwrap()), &m.text);
    }
    let _ = buri::compiler::snapshot::of(buri::compiler::snapshot::Opening::Builtin, true);
    // A load reads and parses on other threads too, so the whole process counts.
    let load = counted(buri::profile::process_instructions, || {
        let mut map = SourceMap::new();
        let ws = buri::build::workspace::Workspace::load(&scratch.root, &mut map, &mut Diagnostics::new()).unwrap();
        let target = ws.targets().into_iter().find(|t| t.kind == buri::build::workspace::RuleKind::Binary).unwrap();
        let unit = buri::compiler::modules::Unit { target: Some(target), platform: None, entry: None, with_tests: false };
        let mut cache = buri::parsing::parser::Cache::new();
        let loading = buri::compiler::driver::load_all(Some(&ws), &mut map, &mut cache, &[unit]);
        assert!(!loading.reported().iter().any(|d| d.severity == Severity::Error), "{name} does not load");
        loading
    })?;
    Some((parse / lines, load / lines))
}

/// **Parsing and loading stay within their budgets on every pinned shape.**
/// Parsing's goal is 10 M lines a second on one core, and these bounds keep a
/// change from spending the margin over it unnoticed.
#[test]
fn parsing_and_loading_each_pinned_shape_stay_within_their_instruction_bounds() {
    let mut over = Vec::new();
    let mut table = String::new();
    for (name, parse_bound, load_bound) in BOUNDS {
        let Some((parse, load)) = costs(name) else { return };
        table.push_str(&format!("  {name:18} parse {parse:5} of {parse_bound:5}   load {load:5} of {load_bound:5}\n"));
        if parse > parse_bound || load > load_bound {
            over.push(name);
        }
    }
    assert!(over.is_empty(), "instructions a line over their bounds on {over:?}:\n{table}");
}
