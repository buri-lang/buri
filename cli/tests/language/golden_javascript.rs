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
from \"core/effect/lib.buri\" import { Alloc, Fs, Stdout };
from \"core/host/lib.buri\" import * as host;
from \"core/fs/lib.buri\" import * as fs;

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
from \"core/effect/lib.buri\" import { Alloc, Stdout };
from \"core/host/lib.buri\" import * as host;

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
