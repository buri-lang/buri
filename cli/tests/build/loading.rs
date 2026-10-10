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
    ("comment-free", 2890, 4990),
    ("comment-heavy", 1720, 2950),
    ("derive-heavy", 2310, 4110),
    ("enum-heavy", 2390, 4070),
    ("generic-blowup", 2620, 4410),
    ("generic-free", 2370, 4120),
    ("impl-heavy", 2200, 3810),
    ("list-heavy", 2500, 4280),
    ("long-bodies", 2490, 4280),
    ("long-idents", 2190, 3800),
    ("match-heavy", 2250, 3990),
    ("mixed", 2410, 4170),
    ("mixed-deep-graph", 2410, 4150),
    ("mixed-few-files", 2330, 3040),
    ("mixed-libs", 2420, 4200),
    ("mixed-many-files", 2700, 8510),
    ("mixed-wide-graph", 2480, 4570),
    ("string-heavy", 2640, 4510),
    ("struct-heavy", 1830, 3320),
    ("struct-light", 2450, 4210),
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

/// The fewer instructions of two runs of `work`, or `None` where the kernel
/// counts none (Linux, and CI's virtual machines).
fn counted<T>(mut work: impl FnMut() -> T) -> Option<u64> {
    let mut fewest = u64::MAX;
    for _ in 0..2 {
        let before = buri::profile::thread_instructions();
        std::hint::black_box(work());
        fewest = fewest.min(buri::profile::thread_instructions().saturating_sub(before));
    }
    (fewest > 0).then_some(fewest)
}

/// What `parser::parse` over every module, and a load of the whole program,
/// retire a line on the pinned point `name`.
fn costs(name: &str) -> Option<(u64, u64)> {
    use buri::diagnostics::{Diagnostics, FileId, Severity, SourceMap};
    let program = pinned_shape(name);
    let lines = program.lines() as u64;
    let parse = counted(|| {
        let parse = |(i, m): (usize, &crate::generate::Module)| buri::parsing::parser::parse(&m.text, FileId(i as u32));
        program.modules.iter().enumerate().map(parse).collect::<Vec<_>>()
    })?;
    let scratch = Scratch::repo(&format!("cost-{name}"));
    scratch.write("bench/BUILD.buri", JS_BINARY);
    for m in &program.modules {
        scratch.write(&format!("bench/{}", m.path.rsplit('/').next().unwrap()), &m.text);
    }
    let _ = buri::compiler::snapshot::of(buri::compiler::snapshot::Opening::Builtin, true);
    let load = counted(|| {
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
