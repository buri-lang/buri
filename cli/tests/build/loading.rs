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
    scratch.write(
        "lib/shared/BUILD.buri",
        "library {\n    visibility: [\"//...\"]\n}\n",
    );
    scratch.write(
        "lib/shared/lib.buri",
        "export fn base(): Int {\n    40\n}\n",
    );
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
        (
            "//lib/missing",
            "\"//lib/missing\" is in no package of this repository",
        ),
        (
            "//lib/shared/gone.buri",
            "\"//lib/shared/gone.buri\" names no file (lib/shared/gone.buri)",
        ),
    ] {
        let scratch = app(&format!("from \"{path}\" import {{ gone }};"));
        let run = scratch.run(&["build", "//app"]);
        run.exits(1);
        for file in ["app/a.buri:3:6", "app/b.buri:3:6"] {
            run.says(file);
        }
        let reported = strip_ansi(&run.all())
            .matches(&format!("{problem} [unknown-module]"))
            .count();
        assert_eq!(
            reported,
            2,
            "want one report per import of {path}:\n{}",
            indent(&run.all())
        );
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

    let unreadable = wide(|i| {
        if i == 3 {
            b"export fn f3(): Int { \xff }\n".to_vec()
        } else {
            returning(i)
        }
    });
    let run = unreadable.run(&["build", "//wide"]);
    run.exits(1);
    run.says("cannot read wide/m3.buri")
        .says("wide/main.buri:7:6");
}

// ---------------------------------------------------------------------------
// What parsing and loading cost a line, on every pinned shape
// ---------------------------------------------------------------------------

/// Each pinned point, and the instructions a line `parser::parse` and a load
/// may retire on it in this test build. The load is of the program's text, as
/// the benchmark's is: it resolves each import and parses, and reads no file.
///
/// Counted rather than timed (`design/PERFORMANCE.md` §8), at 30,000 lines a
/// shape, on this thread only, one module at a time: each module's fewest of
/// five rounds after one that warms up ([`buri::profile::fewest`]). A load
/// from disk isn't counted: opening files and reading ahead on other threads
/// is kernel work, and under load it moved a whole-process count by a quarter.
/// Each bound is a tenth over what was measured. §6.83 has the release build's
/// figures.
const BOUNDS: [(&str, u64, u64); 20] = [
    ("comment-free", 2210, 2540),
    ("comment-heavy", 1320, 1580),
    ("derive-heavy", 1760, 2120),
    ("enum-heavy", 1770, 2070),
    ("generic-blowup", 2130, 2440),
    ("generic-free", 1800, 2100),
    ("impl-heavy", 1650, 1940),
    ("list-heavy", 1910, 2210),
    ("long-bodies", 1810, 2110),
    ("long-idents", 1680, 1970),
    ("match-heavy", 1720, 2020),
    ("mixed", 1840, 2140),
    ("mixed-deep-graph", 1840, 2130),
    ("mixed-few-files", 1750, 1960),
    ("mixed-libs", 1850, 2140),
    ("mixed-many-files", 2240, 3020),
    ("mixed-wide-graph", 1900, 2320),
    ("string-heavy", 2030, 2340),
    ("struct-heavy", 1410, 1690),
    ("struct-light", 1870, 2170),
];

/// The pinned point `name`, regenerated from its manifest at 30,000 lines.
fn pinned_shape(name: &str) -> crate::generate::Program {
    let manifest = repo_root().join(format!("cli/benches/pinned/{name}-1M.txt"));
    let text = std::fs::read_to_string(&manifest).unwrap();
    let field = |key: &str| {
        let prefix = format!("{key} = ");
        text.lines()
            .find_map(|l| l.strip_prefix(&prefix))
            .unwrap_or_default()
            .to_string()
    };
    let (_, profile) = crate::generate::profile(&field("profile")).unwrap();
    let mut params = crate::generate::Params {
        shape: profile.shape,
        ..crate::generate::Params::default()
    };
    for pair in field("params").split_whitespace() {
        let (k, v) = pair.split_once('=').unwrap();
        params.set(k, v).unwrap();
    }
    params.set("lines", "30000").unwrap();
    crate::generate::program(&params)
}

/// What `parser::parse` over every module, and a load of the whole program,
/// retire a line on each shape in [`BOUNDS`], in its order. A round takes
/// every shape in turn, so one shape's rounds are 0.35 s apart, longer than a
/// burst of the kernel's work (`design/PERFORMANCE.md` §8). `None` where the
/// kernel won't say.
fn costs() -> Option<Vec<(u64, u64)>> {
    use buri::compiler::modules::{Loader, Role};
    use buri::compiler::snapshot::{self, Opening};
    use buri::diagnostics::{Diagnostics, FileId, Severity, SourceMap};
    let programs: Vec<_> = BOUNDS
        .iter()
        .map(|&(name, ..)| pinned_shape(name))
        .collect();
    let snapshot = snapshot::of(Opening::Builtin, true);
    let fewest = buri::profile::fewest(5, |steps| {
        for (program, (name, ..)) in programs.iter().zip(BOUNDS) {
            for (i, m) in program.modules.iter().enumerate() {
                steps.step(|| drop(buri::parsing::parser::parse(&m.text, FileId(i as u32))));
            }
            let (mut map, mut diagnostics) = (SourceMap::new(), Diagnostics::new());
            let mut cache = buri::parsing::parser::Cache::new();
            let mut loader = steps.step(|| {
                let mut loader =
                    Loader::seeded(None, &mut map, &mut diagnostics, &mut cache, snapshot);
                loader.load_builtin_modules();
                loader
            });
            let last = program.modules.len().saturating_sub(1);
            for (i, m) in program.modules.iter().enumerate() {
                let role = if i == last { Role::Entry } else { Role::Source };
                steps.step(|| loader.load_source_in(&m.path, role, m.text.clone(), None));
            }
            steps.step(|| drop(loader.finish()));
            assert!(
                !diagnostics
                    .items
                    .iter()
                    .any(|d| d.severity == Severity::Error),
                "{name} does not load"
            );
        }
    });
    if fewest.iter().all(|&n| n == 0) {
        return None;
    }
    // Each shape's steps: a parse of each module, then its load's.
    let mut rest = &fewest[..];
    let mut take = |n: usize| {
        let (these, after) = rest.split_at(n);
        rest = after;
        these.iter().sum::<u64>()
    };
    let costs = programs.iter().map(|program| {
        let (modules, lines) = (program.modules.len(), program.lines() as u64);
        (take(modules) / lines, take(modules + 2) / lines)
    });
    Some(costs.collect())
}

/// **Parsing and loading stay within their budgets on every pinned shape.**
/// Parsing's goal is 10 M lines a second on one core, and these bounds keep a
/// change from spending the margin over it unnoticed.
#[test]
fn parsing_and_loading_each_pinned_shape_stay_within_their_instruction_bounds() {
    let mut over = Vec::new();
    let mut table = String::new();
    let Some(costs) = costs() else {
        return;
    };
    for ((name, parse_bound, load_bound), (parse, load)) in BOUNDS.into_iter().zip(costs) {
        table.push_str(&format!(
            "  {name:18} parse {parse:5} of {parse_bound:5}   load {load:5} of {load_bound:5}\n"
        ));
        if parse > parse_bound || load > load_bound {
            over.push(name);
        }
    }
    eprintln!("{table}");
    assert!(
        over.is_empty(),
        "instructions a line over their bounds on {over:?}:\n{table}"
    );
}

// ---------------------------------------------------------------------------
// What a workspace's bookkeeping costs a module
// ---------------------------------------------------------------------------

/// How many libraries [`chain`] writes.
const CHAIN: usize = 120;

/// `//app`, which prints what the last of [`CHAIN`] libraries returns:
/// `//lib/l<i>` depends on `//lib/l<i-1>` and returns one more than it does.
fn chain() -> Scratch {
    let scratch = Scratch::repo("loading-chain");
    for i in 0..CHAIN {
        let (deps, import, body) = match i.checked_sub(1) {
            None => (String::new(), String::new(), "1".to_string()),
            Some(p) => (
                format!("    dependencies: [\"//lib/l{p}\"]\n"),
                format!("from \"//lib/l{p}\" import {{ v{p} }};\n\n"),
                format!("v{p}() + 1"),
            ),
        };
        scratch.write(
            &format!("lib/l{i}/BUILD.buri"),
            &format!("library {{\n    visibility: [\"//...\"]\n{deps}}}\n"),
        );
        scratch.write(
            &format!("lib/l{i}/lib.buri"),
            &format!("{import}export fn v{i}(): Int {{\n    {body}\n}}\n"),
        );
    }
    let last = CHAIN - 1;
    scratch.write(
        "app/BUILD.buri",
        &format!(
            "binary {{\n    dependencies: [\"//lib/l{last}\"]\n    outputs: [{{ platform: \"node\" }}]\n}}\n"
        ),
    );
    scratch.write(
        "app/main.buri",
        &format!(
            "from \"node\" import {{ NodeHost }};\n\
             from \"platform/effect\" import {{ Stdout }};\n\
             from \"core/io\" import * as io;\n\
             from \"//lib/l{last}\" import {{ v{last} }};\n\n\
             export fn main(host: NodeHost): Result<(), Str> {{\n    \
             let ctx = context {{ Stdout: host.stdout }};\n    \
             let _ = io.println(ctx, \"${{v{last}()}}\").ignore();\n    \
             .Ok(())\n}}\n"
        ),
    );
    scratch
}

/// **A long chain of libraries builds**, and the binary at its end runs.
#[test]
fn a_long_chain_of_libraries_builds_and_runs() {
    let scratch = chain();
    scratch.run(&["build", "//..."]).ok();
    scratch.exec_js("app").ok().says(&CHAIN.to_string());
}

/// The instructions a module that loading every target of [`chain`] at once
/// may retire, on this thread, with every file already read and parsed: what
/// is left is the loader's bookkeeping. Finding where the build files name a
/// library walks every target's dependencies, and doing that for each library
/// each target reaches grows with the cube of the chain: done every time, it
/// cost 2.7 M a module here, and done only for a library that is missing its
/// root, 0.23 M (`design/PERFORMANCE.md` §6.99). Asking each unit whether
/// every library it reaches has its root grew with the square: 0.23 M here and
/// 0.18 M at half the chain. Asked once a library, it is 0.11 M at both
/// (§6.101). The bound is twice that: a load still asks the disk whether each
/// file is there, and that kernel work moved the count by an eighth while the
/// whole suite ran.
const BOOKKEEPING_BOUND: u64 = 225_000;

/// **A workspace's bookkeeping stays within its bound a module**, however
/// many targets reach a library.
#[test]
fn loading_every_target_of_a_long_chain_stays_within_its_instruction_bound() {
    use buri::build::session;
    use buri::commands::arguments::Flags;
    use buri::compiler::driver;
    use buri::compiler::modules::Unit;
    let scratch = chain();
    let mut open = session::open_at(&scratch.path(""), &Flags::default()).unwrap();
    assert!(!open.diagnostics.has_errors(), "the chain does not open");
    let units: Vec<Unit> = open
        .workspace
        .targets()
        .into_iter()
        .map(|target| Unit {
            target: Some(target),
            platform: None,
            entry: None,
            with_tests: true,
        })
        .collect();
    let ws = Some(&*open.workspace);
    let analysis = driver::analyze_all(ws, &mut open.map, &mut open.parsed, &units);
    assert!(
        !analysis.diagnostics.has_errors(),
        "the chain does not load"
    );
    let fewest = buri::profile::fewest(5, |steps| {
        steps.step(|| {
            drop(driver::load_all(
                ws,
                &mut open.map,
                &mut open.parsed,
                &units,
            ))
        });
    });
    if fewest == [0] {
        return;
    }
    let a_module = fewest[0] / (CHAIN as u64 + 1);
    eprintln!("bookkeeping {a_module} instructions a module, of {BOOKKEEPING_BOUND}");
    assert!(
        a_module <= BOOKKEEPING_BOUND,
        "bookkeeping {a_module} instructions a module, over {BOOKKEEPING_BOUND}"
    );
}

/// `//lib/card`, whose tests are `tests`: each one a file under `test/`.
///
/// `platform/effect/testing` re-exports the headless renderer from a module
/// that loads only when an import names part of it, so these tests name it
/// every way a program can.
pub(crate) fn card(tests: &[(&str, &str)]) -> Scratch {
    let scratch = Scratch::repo("loading-renderer");
    let sources: Vec<String> = (tests.iter())
        .map(|(name, _)| format!("\"test/{name}.buri\""))
        .collect();
    scratch.write(
        "lib/card/BUILD.buri",
        &format!(
            "library {{\n    test {{\n        sources: [{}]\n    }}\n}}\n",
            sources.join(", ")
        ),
    );
    scratch.write(
        "lib/card/lib.buri",
        "from \"ui/node\" import * as ui;\n\
         from \"ui/node\" import { Node };\n\n\
         export fn card<C>(children: [Node<C>]): Node<C> {\n    \
         ui.stack({ styles: [.MinWidth(.Px(285))], children: children })\n}\n",
    );
    for (name, source) in tests {
        scratch.write(&format!("lib/card/test/{name}.buri"), source);
    }
    scratch
}

/// A test that renders, importing `render` by name.
pub(crate) const RENDERS: &str = "from \"core/testing/assert\" import * as assert;\n\
     from \"platform/effect\" import { Ui };\n\
     from \"platform/effect/testing\" import { headless, render };\n\
     from \"//lib/card\" import { card };\n\n\
     test \"a card renders\" {\n    \
     let ctx = context {\n        Ui: headless(),\n    };\n    \
     assert.equal(render(ctx, card([])).markup(), \"e 0 class:minw-285\");\n}\n";

/// A test that takes an allocator from the same module and renders nothing.
pub(crate) const ALLOCATES: &str = "from \"core/testing/assert\" import * as assert;\n\
     from \"platform/effect\" import { Allocator };\n\
     from \"platform/effect/testing\" import { alloc };\n\n\
     test \"a number in a list\" {\n    \
     let ctx = context {\n        Allocator: alloc(),\n    };\n    \
     assert.equal([42].map(ctx, fn(n) => n + 1), [43]);\n}\n";

#[test]
fn a_test_renders_through_a_named_import_of_render() {
    let run = card(&[("renders", RENDERS)]).run(&["test", "//..."]);
    run.ok();
    assert_eq!(run.tests_passed(), 1, "{}", indent(&run.all()));
}

#[test]
fn a_test_renders_through_a_namespace_import_of_the_testing_module() {
    let source = "from \"core/testing/assert\" import * as assert;\n\
         from \"platform/effect\" import { Ui };\n\
         from \"platform/effect/testing\" import * as testing;\n\
         from \"//lib/card\" import { card };\n\n\
         test \"a card renders\" {\n    \
         let ctx = context {\n        Ui: testing.headless(),\n    };\n    \
         let page: testing.Rendered = testing.render(ctx, card([]));\n    \
         assert.equal(page.markup(), \"e 0 class:minw-285\");\n    \
         assert.isTrue(testing.stylesheet().contains(\".minw-285{min-width:285px}\"));\n}\n";
    let run = card(&[("renders", source)]).run(&["test", "//..."]);
    run.ok();
    assert_eq!(run.tests_passed(), 1, "{}", indent(&run.all()));
}

#[test]
fn a_test_that_renders_and_one_that_does_not_pass_in_one_run() {
    let run = card(&[("renders", RENDERS), ("allocates", ALLOCATES)]).run(&["test", "//..."]);
    run.ok();
    assert_eq!(run.tests_passed(), 2, "{}", indent(&run.all()));
}

#[test]
fn a_misspelt_renderer_name_is_reported_with_the_name_it_meant() {
    let source = RENDERS
        .replace("{ headless, render }", "{ headless, rendr }")
        .replace("render(ctx", "rendr(ctx");
    let run = card(&[("renders", &source)]).run(&["test", "//..."]);
    run.exits(1)
        .says("\"platform/effect/testing\" does not export `rendr`")
        .says("`render`");
}

/// The page for `platform/effect/testing` lists the renderer and the methods
/// of the tree it returns, though both are declared in a module of their own.
#[test]
fn the_testing_module_page_lists_the_renderer_and_its_methods() {
    let run = Scratch::repo("loading-renderer-docs").run(&["docs", "platform/effect/testing"]);
    run.ok()
        .says("fn render<C: Ui>(ctx: C, root: Node<C>): Rendered")
        .says("fn markup(self): Str")
        .says("fn snapshot<");
}

/// `source: N lines of code lexed` from a `BURI_PROFILE=1` report.
fn lines_lexed(all: &str) -> Option<u64> {
    let line = all.lines().find(|l| l.starts_with("source: "))?;
    line.trim_start_matches("source: ")
        .split_whitespace()
        .next()?
        .parse()
        .ok()
}

/// **A test that renders nothing doesn't load the renderer.** Every test that
/// takes an allocator imports `platform/effect/testing`, and its renderer
/// brought `ui/node`, `ui/style` and `ui/theme` into every one: a quarter of
/// `test/one-edit`'s instructions (design/PERFORMANCE.md §6.122). The
/// renderer and `ui/` are well over 3,000 lines between them.
#[test]
fn a_test_that_renders_nothing_lexes_none_of_the_renderer() {
    let lexed = |source: &str| {
        let scratch = Scratch::repo("loading-renderer-lexed");
        scratch.write(
            "lib/a/BUILD.buri",
            "library {\n    test {\n        sources: [\"test/a.buri\"]\n    }\n}\n",
        );
        scratch.write("lib/a/lib.buri", "export fn answer(): Int {\n    42\n}\n");
        scratch.write("lib/a/test/a.buri", source);
        let run = scratch.run_with_env(&["test", "//..."], &[("BURI_PROFILE", "1")]);
        run.ok();
        assert_eq!(run.tests_passed(), 1, "{}", indent(&run.all()));
        lines_lexed(&run.all()).unwrap_or_else(|| panic!("no source line:\n{}", indent(&run.all())))
    };
    let renders = "from \"core/testing/assert\" import * as assert;\n\
         from \"platform/effect\" import { Ui };\n\
         from \"platform/effect/testing\" import { headless, render };\n\
         from \"ui/node\" import * as ui;\n\n\
         test \"a stack renders\" {\n    \
         let ctx = context {\n        Ui: headless(),\n    };\n    \
         let page = render(ctx, ui.stack({ styles: [], children: [] }));\n    \
         assert.equal(page.count(\"nothing\"), 0);\n}\n";
    let (allocates, renders) = (lexed(ALLOCATES), lexed(renders));
    assert!(
        allocates + 3_000 < renders,
        "a test that renders nothing lexed {allocates} lines, one that renders {renders}"
    );
}
