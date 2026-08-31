//! What the backend emits, recorded.
//!
//! Every other suite asks whether a program *behaves*. This one asks what it
//! *compiles to*, because that is the only way an optimisation is visible: a
//! pass that removes an allocation or a call frame changes no answer anywhere,
//! so without a record of the output it lands unseen and regresses unnoticed.
//!
//! Each case in `tests/golden_javascript/` is one small program exercising one
//! construct, together with three recordings:
//!
//! * `expected.mjs` — the generated half of the debug artifact. The
//!   hand-written runtime is removed, because it is the same thousand lines in
//!   every case and it is not what any pass here changes.
//! * `expected.out` — what the program prints. A record of *output* with no
//!   record of *behaviour* would happily bless a miscompile, so every case runs,
//!   in both build modes, and the two must agree.
//! * `sizes.txt` — sizes for the whole corpus in one file, so the size effect
//!   of a change is a single reviewable diff rather than a number buried in
//!   each case. Two columns: the release artifact, which is what a user ships
//!   and is mostly runtime, and the generated code, which is what the backend
//!   emitted and where a pass either shows up or did not land.
//!
//! `BURI_BLESS=1 cargo test -p buri --test language golden_javascript::` records. Read the diff:
//! blessing without reading it is the one way this suite proves nothing.
use crate::harness::*;

/// The generated half of an artifact.
///
/// The runtime is spliced in one declaration at a time (`generate::generate`),
/// each verbatim in a debug build, so removing each declaration's own source
/// text leaves exactly what the backend produced. Dropping whichever
/// declarations dead-code elimination kept is order-independent and survives a
/// program reaching more or less of the runtime than its neighbours.
fn program_only(artifact: &str) -> String {
    let mut rest = artifact.to_string();
    for (_, src) in buri::compiler::backend::js::javascript::split_declarations(buri::compiler::backend::js::runtime_source()) {
        rest = rest.replacen(src.trim(), "", 1);
    }
    // The entry epilogue is the same fifteen lines of `try`/`catch` in every
    // case, modulo the name it calls, and nothing here changes it.
    let body: Vec<&str> = rest
        .lines()
        .filter(|l| !l.starts_with("try{const r="))
        .filter(|l| !l.starts_with("import{createRequire"))
        .filter(|l| !l.starts_with("const $require="))
        .collect();

    // Collapse the blank runs the removals left behind, so the record is the
    // program and nothing else.
    let mut out = String::new();
    let mut blank = true;
    for line in body {
        if line.trim().is_empty() {
            blank = true;
            continue;
        }
        if blank && !out.is_empty() {
            out.push('\n');
        }
        blank = false;
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// The stylesheet an artifact carries, as text.
///
/// The backend emits it as one assignment to the runtime's `$ui_sheet`, and the
/// printer escaped it into a string literal; this is the inverse. Reading it
/// back out of the artifact rather than asking the compiler for it is
/// deliberate: what is recorded here is what a page would actually load.
fn stylesheet_of(artifact: &str) -> String {
    let Some(rest) = artifact.split("$ui_sheet=").nth(1) else { return String::new() };
    let mut chars = rest.chars();
    let Some(quote) = chars.next() else { return String::new() };
    let mut out = String::new();
    while let Some(c) = chars.next() {
        if c == quote {
            break;
        }
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some(other) => out.push(other),
            None => break,
        }
    }
    out
}

/// V8 refuses to optimize a function above 61,440 bytecodes: past that it runs
/// in the interpreter forever, however hot it gets. Bytes of minified source
/// are not bytecodes, but they move together, and a whole-program compiler has
/// three ways to grow one function without anyone noticing — the inliner's
/// per-caller growth ceiling compounds over its rounds, a merged tail-call
/// group fuses a whole mutually recursive component into one function, and
/// `main` accumulates every single-use body inlined into it.
///
/// So the largest function is recorded, and a number that would put the limit
/// within reach fails rather than being blessed. The corpus is small; the point
/// is that the *trend* is visible in a diff.
const LARGEST_FUNCTION_LIMIT: usize = 32768;

/// The largest single declaration in an artifact, and what it is called.
fn largest_function(artifact: &str) -> (String, usize) {
    buri::compiler::backend::js::javascript::split_declarations(artifact)
        .into_iter()
        .map(|(name, src)| (name, src.len()))
        .max_by_key(|(_, n)| *n)
        .unwrap_or_else(|| (String::from("none"), 0))
}

/// Records what a program prints if nothing has been recorded yet, and
/// otherwise compares — whether or not this run is blessing.
///
/// The rest of this corpus is a record of *output* and is meant to move with
/// every pass. This is a record of *behaviour*, and a pass may not move it. A
/// `BURI_BLESS=1` that could rewrite it would turn the one check here that
/// catches a miscompile into a check that rubber-stamps one.
fn behaviour(g: &mut Golden, path: &std::path::Path, label: &str, actual: &str) {
    match std::fs::read_to_string(path) {
        Ok(recorded) if recorded == actual => {}
        Ok(recorded) => g.fail(format!(
            "{label}: the program prints something else now. This is a change in \
             behaviour, not in output, so it is never blessed — fix it, or delete \
             the file to re-record deliberately.\n  recorded:\n{}\n  printed:\n{}",
            indent(&recorded),
            indent(actual)
        )),
        Err(_) => std::fs::write(path, actual).unwrap(),
    }
}

#[test]
fn generated_javascript_matches_its_record() {
    let dir = tests_dir().join("golden_javascript");
    let cases = case_dirs(&dir, "main.buri", 15);

    let mut g = Golden::new();
    let mut sizes = String::new();
    let (mut total_release, mut total_generated) = (0usize, 0usize);
    let mut biggest = (String::new(), 0usize);

    for case in &cases {
        let name = case.file_name().unwrap().to_string_lossy().to_string();
        let source = std::fs::read_to_string(case.join("main.buri")).unwrap();

        let scratch = Scratch::repo(&format!("golden-js-{name}"));
        scratch.binary_package("cmd/x", &source);
        // A case that binds a UI effect declares `// PLATFORM: WEB`, and its
        // artifact lands under `.buri/out/web/`. Everything recorded below is
        // the same recording either way: the module a page loads and the module
        // a script loads are the same bytes, and it is the *host grant* the two
        // platforms differ in.
        let out_dir = output_dir_for(&source);

        // Debug first: it is what `expected.mjs` records, and unmangled names
        // are what make the diff readable.
        scratch.run(&["build", "//cmd/x", "--force"]).ok();
        let debug = std::fs::read_to_string(scratch.artifact_in(out_dir, "cmd/x")).unwrap();
        let generated = program_only(&debug);
        g.check(
            &case.join("expected.mjs"),
            &format!("golden_javascript/{name}/expected.mjs"),
            &generated,
        );
        // The stylesheet is the other half of what this backend emits for a
        // user interface, and it is a separate record because it is a separate
        // artifact in every way but where the bytes sit: it is CSS, it is read
        // by a browser rather than run, and a change to it is reviewed as text
        // rather than as code. A case with no styles records no file, and one
        // that *stopped* having styles fails rather than quietly losing them.
        let sheet = stylesheet_of(&debug);
        let css = case.join("expected.css");
        if sheet.is_empty() {
            if css.exists() {
                g.fail(format!(
                    "{name}: `expected.css` records a stylesheet, and the program \
                     no longer emits one. Delete the file to record that \
                     deliberately."
                ));
            }
        } else {
            g.check(&css, &format!("golden_javascript/{name}/expected.css"), &sheet);
        }

        let debug_out = scratch.exec_js_in(out_dir, "cmd/x");
        debug_out.ok();

        scratch.run(&["build", "//cmd/x", "--release", "--force"]).ok();
        let release = std::fs::read_to_string(scratch.artifact_in(out_dir, "cmd/x")).unwrap();
        let release_out = scratch.exec_js_in(out_dir, "cmd/x");
        release_out.ok();

        // Per case, what `release_and_debug_agree` asserts for the suite as a
        // whole. Here it is what stops a blessed `expected.mjs` from recording
        // a program that stopped working.
        if debug_out.stdout != release_out.stdout {
            g.fail(format!(
                "{name}: release and debug print differently.\n  debug:\n{}\n  release:\n{}",
                indent(&debug_out.stdout),
                indent(&release_out.stdout)
            ));
        }
        // Recorded once and never re-recorded. `expected.mjs` is a record of
        // *how* a program compiles and is meant to move; `expected.out` is a
        // claim about what it computes, and blessing one of those would launder
        // exactly the failure this corpus exists to catch. It caught a
        // tail-call rebinding that read a parameter after overwriting it, and
        // would have recorded `4950` for a sum of `5050` had it been writable.
        // To change one deliberately, delete it and re-record.
        behaviour(
            &mut g,
            &case.join("expected.out"),
            &format!("golden_javascript/{name}/expected.out"),
            &debug_out.stdout,
        );

        if release.len() >= debug.len() {
            g.fail(format!(
                "{name}: the release artifact ({} bytes) is not smaller than the debug one ({} bytes)",
                release.len(),
                debug.len()
            ));
        }
        // Two numbers, because they answer different questions. The artifact
        // is what a user ships; most of it is the runtime, which no pass here
        // changes, so it moves slowly. The generated column is what the
        // backend actually emitted, and it is where a pass either shows up or
        // did not land.
        sizes.push_str(&format!(
            "{name}: {} bytes artifact, {} generated\n",
            release.len(),
            generated.len()
        ));
        total_release += release.len();
        total_generated += generated.len();

        let (fname, fsize) = largest_function(&release);
        if fsize > biggest.1 {
            biggest = (format!("{name}/{fname}"), fsize);
        }
        if fsize > LARGEST_FUNCTION_LIMIT {
            g.fail(format!(
                "{name}: `{fname}` is {fsize} bytes of minified source, which puts it \
                 within reach of V8's 61,440-bytecode ceiling — past that a function is \
                 never optimized, however hot it gets. Look at what grew it: the \
                 inliner's per-caller ceiling compounds over its rounds, a merged \
                 tail-call group fuses a whole component, and `main` collects every \
                 single-use body inlined into it."
            ));
        }
    }
    sizes.push_str(&format!(
        "\ntotal: {total_release} bytes artifact, {total_generated} generated\n"
    ));
    // One number for the whole corpus, because what matters is the trend: a
    // pass that starts fusing functions together shows up here first.
    sizes.push_str(&format!("largest function: {} bytes, {}\n", biggest.1, biggest.0));

    g.check(&dir.join("sizes.txt"), "golden_javascript/sizes.txt", &sizes);
    g.finish("golden-js", cases.len());
}

/// `async` reaches exactly as far as `can_park` says, and no further.
///
/// The transform's whole claim is that the contagion has an edge: a function
/// that waits on the host is `async` and every call to it is awaited, and a
/// function that only computes is the same bytes it always was. A test that
/// checked one half would pass on a backend that printed `async` on
/// everything, which is the failure mode that costs a promise per call in
/// programs that touch no host at all.
///
/// So both directions are asserted, over one program holding both kinds:
/// `load` reaches `host.HostFs.readFile` and `count` reaches nothing. Both are
/// recursive, because an inlined function leaves no declaration to look at.
#[test]
fn only_the_functions_that_can_park_are_async() {
    let program = "\
from \"core/effect\" import { Alloc, Fs, Stdout };
from \"core/host\" import * as host;
from \"core/fs\" import * as fs;

// Reaches a host call that blocks, so this one waits.
fn load<C: Alloc + Fs>(ctx: C, path: Str, n: Int): Int {
  if (n <= 0) {
    0
  } else {
    let head = fs.readText(ctx, path).withDefault(\"\");
    head.len() + load(ctx, path, n - 1)
  }
}

// Reaches nothing but arithmetic.
fn count(n: Int, acc: Int): Int {
  if (n <= 0) { acc } else { count(n - 1, acc + 2) }
}

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Fs: host.fs, Stdout: host.stdout };
  let total = load(ctx, \"a.txt\", 3);
  let _ = ctx.println(\"${total} ${count(4, 0)}\");
  .Ok(())
}
";
    let scratch = Scratch::repo("async-reaches-only-what-parks");
    scratch.binary_package("cmd/x", program);
    scratch.run(&["build", "//cmd/x", "--force"]).ok();
    let artifact = std::fs::read_to_string(scratch.artifact("cmd/x")).unwrap();
    let generated = program_only(&artifact);

    let line = |needle: &str, what: &str| -> String {
        generated
            .lines()
            .find(|l| l.contains(needle))
            .unwrap_or_else(|| panic!("no {what} in\n{generated}"))
            .to_string()
    };

    let load = line("function __cmd_x_main_buri$load", "declaration of `load`");
    assert!(
        load.starts_with("async function "),
        "`load` reaches `host.HostFs.readFile`, so it waits:\n{load}\n\n{generated}"
    );
    let count = line("function __cmd_x_main_buri$count", "declaration of `count`");
    assert!(
        count.starts_with("function "),
        "`count` reaches nothing that waits and must not be `async`:\n{count}\n\n{generated}"
    );

    // And the call sites agree with the declarations: one awaited, one not.
    let call_load = line("=await __cmd_x_main_buri$load", "awaited call to `load`");
    assert!(call_load.contains("await "), "{call_load}");
    for l in generated.lines().filter(|l| l.contains("$count(")) {
        assert!(
            !l.contains("await "),
            "a call to a function that does not wait is not awaited:\n{l}\n\n{generated}"
        );
    }
    // The entry epilogue, which `program_only` filters out of the record, is
    // where the artifact leaves the synchronous world behind.
    assert!(
        artifact.contains("try{const r=await "),
        "`main` reaches `load`, so the epilogue awaits it\n\n{artifact}"
    );

    // And it runs, in both build modes. This is the only program in the suite
    // that is `async` at all, so it is the only one that says the minifier
    // carries `async`/`await` through its passes intact — and that the value
    // reaching `head.len()` is a string rather than the promise an unawaited
    // call would have left there. `data.txt` does not exist, so the read is
    // an `.Err` and `withDefault` answers `""`: two reads of nothing, and
    // `count(4, 0)`.
    let debug_out = scratch.exec_js("cmd/x");
    debug_out.ok();
    assert_eq!(debug_out.stdout.trim(), "0 8", "{}", debug_out.stdout);
    scratch.run(&["build", "//cmd/x", "--release", "--force"]).ok();
    let release_out = scratch.exec_js("cmd/x");
    release_out.ok();
    assert_eq!(release_out.stdout, debug_out.stdout, "release and debug disagree");
}

/// A sleeping program leaves the event loop free.
///
/// This is the whole of what the host bodies becoming asynchronous bought, and
/// it is the one claim that a test of *output* cannot make: the old
/// `sleepMillis` spun on `Date.now()` (or called `Bun.sleepSync`, which is the
/// same stall with the core given back), and a program that slept for a third
/// of a second printed exactly what one that waits for it prints.
///
/// So the measurement is of what else got to run *during* the sleep. The
/// artifact is an ES module, so a probe beside it can start a timer, import
/// it — which is when its top-level `await main()` runs — and count the
/// callbacks that landed. A blocking sleep lets none of them land.
///
/// **This is not the `Tasks.parallel` overlap test**, which is D3's: nothing
/// here runs two Buri tasks at once, and this asserts only that the runtime
/// gives the loop back, which is D3's precondition rather than its subject.
#[test]
fn a_sleeping_program_leaves_the_event_loop_free() {
    let program = "\
from \"core/effect\" import { Alloc, Clock, Stdout };
from \"core/host\" import * as host;

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Clock: host.clock, Stdout: host.stdout };
  let _ = ctx.sleepMillis(150);
  let _ = ctx.sleepMillis(150);
  let _ = ctx.println(\"slept\");
  .Ok(())
}
";
    let scratch = Scratch::repo("sleeping-frees-the-event-loop");
    scratch.binary_package("cmd/x", program);
    scratch.run(&["build", "//cmd/x", "--force"]).ok();

    let artifact = std::fs::read_to_string(scratch.artifact("cmd/x")).unwrap();
    assert!(
        artifact.contains("setTimeout"),
        "`sleepMillis` waits on a timer\n\n{artifact}"
    );
    assert!(
        !artifact.contains("sleepSync"),
        "the blocking sleep is gone\n\n{artifact}"
    );

    // Ten milliseconds apart over 300 of them is thirty callbacks if the loop
    // is free; the assertion asks for five, which no amount of scheduling
    // noise takes away and no blocking sleep ever reaches.
    scratch.write(
        ".buri/out/js/cmd/x/probe.mjs",
        "let ticks = 0;\n\
         const beat = setInterval(() => { ticks += 1; }, 10);\n\
         await import(\"./x.mjs\");\n\
         clearInterval(beat);\n\
         console.log(\"ticks \" + ticks);\n",
    );
    let probe = scratch.path(".buri/out/js/cmd/x/probe.mjs");
    let out = std::process::Command::new(js_runtime())
        .arg(&probe)
        .output()
        .expect("the javascript runtime runs");
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(text.contains("slept"), "the program ran:\n{text}");
    let ticks: u32 = text
        .lines()
        .find_map(|l| l.strip_prefix("ticks "))
        .and_then(|n| n.trim().parse().ok())
        .unwrap_or_else(|| {
            panic!("no tick count in\n{text}\nstderr:\n{}", String::from_utf8_lossy(&out.stderr))
        });
    assert!(
        ticks >= 5,
        "only {ticks} timer callbacks ran during 300ms of sleeping, so the sleep is \
         still blocking the event loop"
    );
}

/// Writing octets reaches a node module, and the prologue that fetches one is
/// guarded so that a page never resolves it.
///
/// Two facts in one program, because they are two halves of the same line.
///
/// `Stdout.writeBytes` is the one host operation left that must *not* wait —
/// a protocol answers before it reads again, and `process.stdout` is
/// asynchronous over a pipe on macOS — so it keeps `fs.writeSync`, and so it
/// keeps needing `require`. Before the readers below it moved off the
/// descriptor, `Stdin` asked for the same prologue and this program got one by
/// accident; asking for it by name is what makes a program that only writes
/// octets work at all.
///
/// And the prologue is a *dynamic* import behind `typeof process`, because
/// `Stdout` is granted on every platform, `WEB` included. A static
/// `import ... from "node:module"` is resolved before a line of the artifact
/// runs, so it would take a page down at load; this one is never reached
/// there, and `$writeRaw` refuses in the browser exactly as it did before.
#[test]
fn writing_octets_asks_for_the_prologue_and_a_page_never_resolves_it() {
    let program = "\
from \"core/effect\" import { Alloc, Stdout };
from \"core/host\" import * as host;
from \"core/io\" import * as io;

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout };
  let _ = io.writeBytes(ctx, [104, 105, 10]);
  .Ok(())
}
";
    let scratch = Scratch::repo("octets-ask-for-the-prologue");
    scratch.binary_package("cmd/x", program);
    scratch.run(&["build", "//cmd/x", "--force"]).ok();
    let artifact = std::fs::read_to_string(scratch.artifact("cmd/x")).unwrap();

    assert!(
        artifact.contains("const $require="),
        "a program that writes octets needs `require`\n\n{artifact}"
    );
    assert!(
        !artifact.contains("import{createRequire"),
        "the prologue is not a static import: a page resolves those at load\n\n{artifact}"
    );
    assert!(
        artifact.contains("typeof process===\"undefined\"?undefined"),
        "and it is guarded, because `Stdout` is granted on `WEB` too\n\n{artifact}"
    );

    // And it writes, which is the half that was broken: a program reaching
    // neither `Fs` nor `Stdin` used to get no prologue and abort inside
    // `$writeRaw` with "this platform grants no filesystem".
    let out = scratch.exec_js("cmd/x");
    out.ok();
    assert_eq!(out.stdout, "hi\n", "{}", out.stderr);
}

/// Two generics instantiated over different contexts get different symbols.
///
/// This is the invariant that decides how `monomorphize::name_of` may name things, and
/// it was worth a test the moment it nearly went the other way. The symbol's
/// hash is taken over the `Debug` form of the type arguments, which carries
/// type-table indices — so it moves if the compiler changes what it loads,
/// which is untidy. Hashing a *rendering* instead is the obvious repair and is
/// wrong: `types::show` prints every context type as `a context`, because a
/// context type is generated and has no name. The two instantiations below
/// would collide, one body would silently replace the other, and the program
/// would call the wrong implementation.
///
/// The two contexts here bind the same effect to different implementations —
/// exactly the case a name-based hash cannot tell apart.
#[test]
fn generics_over_different_contexts_do_not_share_a_symbol() {
    let program = "\
from \"core/effect\" import { Alloc, Stdout };
from \"core/host\" import * as host;

struct Loud(Str);
impl Stdout for Loud {
  fn print(self, text: Template): () { }
  fn println(self, text: Template): () { }
  fn writeBytes(self, b: [U8]): () { }
}

// Recursive, so the optimiser cannot inline it away — an inlined function
// leaves no symbol to compare.
fn shout<C: Stdout>(ctx: C, what: Str, n: Int): Int {
  if (n <= 0) {
    0
  } else {
    let _ = ctx.println(\"${what}\");
    shout(ctx, what, n - 1)
  }
}

export fn main(): Result<(), Str> {
  let real = context { Alloc: host.alloc, Stdout: host.stdout };
  let mine = context { Alloc: host.alloc, Stdout: Loud(\"x\") };
  let _ = shout(real, \"a\", 2);
  let _ = shout(mine, \"b\", 2);
  .Ok(())
}
";
    let scratch = Scratch::repo("symbol-context-identity");
    scratch.binary_package("cmd/x", program);
    scratch.run(&["build", "//cmd/x", "--force"]).ok();
    let generated =
        program_only(&std::fs::read_to_string(scratch.artifact("cmd/x")).unwrap());

    let mut symbols: Vec<String> = generated
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '$'))
        .filter(|p| p.contains("$shout$"))
        .map(String::from)
        .collect();
    symbols.sort();
    symbols.dedup();

    assert_eq!(
        symbols.len(),
        2,
        "expected one symbol per context, found {}: {symbols:?}\n\n{generated}",
        symbols.len()
    );
}

/// **A callback that parks, called through a function value, is awaited.**
///
/// The generic wrapper below is four lines and holds the whole defect class:
/// `body` is a `fn(C) => T`, so at `body(ctx)` the backend has no name to look
/// the callee's parkability up under. The set of `async` functions used to be
/// computed over direct edges only — an indirect call contributed nothing —
/// which is sound exactly while no function *value* in the program parks. A
/// lambda that takes a context parameter and sleeps on it is one, so the call
/// was emitted without `await`, `wrapped` was not `async`, and `n` was bound to
/// a `Promise`.
///
/// Two things fail when the `await` is missing and both are asserted, because
/// either alone can be got right by accident:
///
///  * **the value.** `n=[object Promise]` rather than `n=7`.
///  * **the order.** The line the callback prints lands *after* `main`'s,
///    because `main` runs to the end while the sleep is still pending. Order
///    is the assertion that says the caller waited rather than merely that
///    something later awaited the promise for it.
#[test]
fn a_parking_callback_called_through_a_function_value_is_awaited() {
    let program = "\
from \"core/effect\" import { Alloc, Clock, Stdout };
from \"core/host\" import * as host;

fn wrapped<C, T>(ctx: C, body: fn(C) => T): T {
  body(ctx)
}

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Clock: host.clock, Stdout: host.stdout };
  let n = wrapped(ctx, fn(c) => {
    let _ = c.sleepMillis(20);
    let _ = c.println(\"inside\");
    7
  });
  let _ = ctx.println(\"after ${n}\");
  .Ok(())
}
";
    let scratch = Scratch::repo("can-park-through-a-function-value");
    scratch.binary_package("cmd/x", program);
    scratch.run(&["build", "//cmd/x", "--force"]).ok();
    let out = scratch.exec_js("cmd/x");
    out.ok();
    assert_eq!(out.stdout, "inside\nafter 7\n", "{}", out.stderr);

    scratch.run(&["build", "//cmd/x", "--release", "--force"]).ok();
    let release = scratch.exec_js("cmd/x");
    release.ok();
    assert_eq!(release.stdout, out.stdout, "release and debug disagree");
}

/// The G5 reproduction, byte for byte: a wrapper whose callback runs tasks.
///
/// `tasks.parallel` is on `rc::suspends` for a reason of its own — it waits on
/// the program's own tasks rather than on the outside world — and it answers a
/// list. So a dropped `await` here does not merely print a promise, it *aborts*
/// on `xs.join is not a function`, which is how the gap was found (wave-8 G5
/// §6). No `core/alloc` and no scope in it: the wrapper is written in the file.
#[test]
fn the_wrapper_reproduction_from_g5_answers_a_list_rather_than_a_promise() {
    let program = "\
from \"core/effect\" import { Alloc, Clock, Stdout, Tasks };
from \"core/host\" import * as host;
from \"core/tasks\" import * as tasks;
from \"core/str\" import * as str;

fn wrapped<C, T>(ctx: C, body: fn(C) => T): T {
  body(ctx)
}

export fn main(): Result<(), Str> {
  let ctx = context {
    Alloc: host.alloc, Stdout: host.stdout, Tasks: host.tasks, Clock: host.clock,
  };
  let out = wrapped(ctx, fn(c) => tasks.parallel(c, [1, 2, 3], fn(d, i, n) => str.format(d, \"${n}\")));
  let _ = ctx.println(out.join(ctx, \",\"));
  .Ok(())
}
";
    let scratch = Scratch::repo("can-park-g5-wrapper");
    scratch.binary_package("cmd/x", program);
    scratch.run(&["build", "//cmd/x", "--force"]).ok();
    let out = scratch.exec_js("cmd/x");
    out.ok();
    assert_eq!(out.stdout, "1,2,3\n", "{}", out.stderr);
}

/// One function declaration out of the generated half, `async` and all.
///
/// `name` is a fragment of the symbol — `"$applyN"` — because a symbol carries
/// an instantiation hash a test has no business spelling. The `async` keyword
/// is ahead of the `function` this searches for, so it is read off the text
/// before the match rather than from inside the slice.
fn declaration(generated: &str, name: &str) -> String {
    let sym = generated
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '$'))
        .find(|p| p.contains(name))
        .unwrap_or_else(|| panic!("no `{name}` in\n\n{generated}"))
        .to_string();
    let at = generated
        .find(&format!("function {sym}("))
        .unwrap_or_else(|| panic!("no declaration of `{sym}` in\n\n{generated}"));
    let rest = generated.get(at..).unwrap_or_default();
    let end = rest[1..].find("\nfunction ").map_or(rest.len(), |i| i + 1);
    let asynced = generated.get(..at).unwrap_or_default().ends_with("async ");
    format!("{}{}", if asynced { "async " } else { "" }, &rest[..end])
}

/// And the `await` reaches only the indirect calls that need it.
///
/// The sound-but-blunt repair for the case above is to await *every* call
/// through a function value as soon as the program builds one that parks. It
/// would pass that test and cost a promise at every callback in the artifact —
/// and, worse, print `async` on functions whose value JavaScript itself calls:
/// a comparator, a `ui.each` row, the callback of `$list_mapCtx`.
///
/// So this program holds both kinds at once. `wrapped` is handed a callback
/// that sleeps and has to be `async`; `applyN` is handed one that adds and must
/// not be. Both take their callback as a parameter and call it through the
/// value, and neither is ever used as a value itself — which is exactly the
/// condition under which the argument at each call site is the whole story.
///
/// Both are recursive so that the inliner leaves a declaration to look at.
#[test]
fn a_callback_that_does_not_park_leaves_its_wrapper_synchronous() {
    let program = "\
from \"core/effect\" import { Alloc, Clock, Stdout };
from \"core/host\" import * as host;

fn sleepy<C: Clock>(ctx: C, n: Int, body: fn(C) => Int): Int {
  if (n <= 0) { 0 } else { body(ctx) + sleepy(ctx, n - 1, body) }
}

fn applyN(n: Int, x: Int, f: fn(Int) => Int): Int {
  if (n <= 0) { x } else { applyN(n - 1, f(x), f) }
}

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Clock: host.clock, Stdout: host.stdout };
  let slow = sleepy(ctx, 2, fn(c) => {
    let _ = c.sleepMillis(1);
    5
  });
  let fast = applyN(3, 1, fn(x) => x + 1);
  let _ = ctx.println(\"${slow} ${fast}\");
  .Ok(())
}
";
    let scratch = Scratch::repo("can-park-precision");
    scratch.binary_package("cmd/x", program);
    scratch.run(&["build", "//cmd/x", "--force"]).ok();
    let generated =
        program_only(&std::fs::read_to_string(scratch.artifact("cmd/x")).unwrap());

    let sleepy = declaration(&generated, "$sleepy");
    assert!(
        sleepy.starts_with("async ") && sleepy.contains("await"),
        "the wrapper whose callback sleeps is `async` and awaits it:\n\n{sleepy}"
    );
    let apply = declaration(&generated, "$applyN");
    assert!(
        !apply.starts_with("async ") && !apply.contains("await"),
        "the wrapper whose callback only adds is untouched:\n\n{apply}"
    );

    let out = scratch.exec_js("cmd/x");
    out.ok();
    assert_eq!(out.stdout, "10 4\n", "{}", out.stderr);
}

/// A callback the pass could not follow is answered by its **type**.
///
/// This is the case the first draft of the repair got wrong, and the monorepo's
/// page is what caught it: `Prop.read`'s third arm calls a function value out of
/// an enum payload — no name, no parameter, nothing to follow — while somewhere
/// else in the same program an `onPress` handler fetches. A program-wide
/// "something here parks" made `Prop.read` `async`, and `Prop.read` is reached
/// from a style thunk the *runtime* calls (`$tree_style_collect`,
/// `style[1](scope)`), which cannot await. The page died on `{} is not
/// iterable`: a promise where a list of styles belonged.
///
/// So the program below is that shape in miniature. `force` calls a
/// `fn(Int) => Int` it took out of an enum payload, and `wrapped` is handed a
/// `fn(C) => Int` that sleeps. Two function types that never meet, one of which
/// parks: `wrapped` must be `async` and `force` must not.
///
/// `the_monorepo_page_builds_as_a_web_artifact` is the same claim at full size
/// and would fail again first; this one names the reason.
#[test]
fn an_unfollowed_callback_is_answered_by_its_type() {
    let program = "\
from \"core/effect\" import { Alloc, Clock, Stdout };
from \"core/host\" import * as host;

enum Thunk {
  Const(Int),
  Computed(fn(Int) => Int),
}

fn force(t: Thunk, x: Int, n: Int): Int {
  if (n <= 0) {
    x
  } else {
    match (t) {
      .Const(k) => force(t, x + k, n - 1),
      .Computed(f) => force(t, f(x), n - 1),
    }
  }
}

fn wrapped<C: Clock>(ctx: C, n: Int, body: fn(C) => Int): Int {
  if (n <= 0) { 0 } else { body(ctx) + wrapped(ctx, n - 1, body) }
}

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Clock: host.clock, Stdout: host.stdout };
  let slow = wrapped(ctx, 2, fn(c) => {
    let _ = c.sleepMillis(1);
    3
  });
  let a = force(.Const(1), 10, 3);
  let b = force(.Computed(fn(x) => x + 5), 10, 3);
  let _ = ctx.println(\"${slow} ${a} ${b}\");
  .Ok(())
}
";
    let scratch = Scratch::repo("can-park-by-type");
    scratch.binary_package("cmd/x", program);
    scratch.run(&["build", "//cmd/x", "--force"]).ok();
    let generated =
        program_only(&std::fs::read_to_string(scratch.artifact("cmd/x")).unwrap());

    let force = declaration(&generated, "$force");
    assert!(
        !force.starts_with("async ") && !force.contains("await"),
        "a `fn(Int) => Int` out of an enum payload is not the `fn(C) => Int` that \
         sleeps, so nothing here waits:\n\n{force}"
    );
    let wrapped = declaration(&generated, "$wrapped");
    assert!(
        wrapped.starts_with("async ") && wrapped.contains("await"),
        "and the one that does still waits:\n\n{wrapped}"
    );

    let out = scratch.exec_js("cmd/x");
    out.ok();
    assert_eq!(out.stdout, "6 13 25\n", "{}", out.stderr);
}
