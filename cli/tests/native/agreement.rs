//! The rows of `design/native/VALUE-MODEL.md` §12, as tests.
//!
//! §12 is a table of every way the JavaScript backend and a native backend
//! could differ, and every row is either **must agree** or is a **documented
//! divergence**. Until this file existed the table was a claim: nothing
//! compiled one program through both pipelines and compared the bytes, so a
//! row that quietly stopped being true stayed in the document as a sentence.
//! the design notes's native section said so in as many words — "that test does not
//! exist yet, and until it does the table is a claim rather than a check".
//!
//! It was not a check, and four of the rows were wrong. Each is a test below
//! rather than a paragraph:
//!
//!  * **Row 3 was false.** `wrappingMul` was not exact on JavaScript at any
//!    width where the product leaves 2^53 — the `BigInt` in `$wrapTo` wraps a
//!    double that has *already* been rounded, so `U32.wrappingMul(0xffffffff,
//!    0xffffffff)` answered 0 where the answer is 1. Not a precision ceiling: a
//!    wrong answer, at 32 bits, with exact operands and an exact answer, from
//!    the operation a checksum is written with. `$wrapOp` fixes it wherever the
//!    type's whole range is exact, which is every width up to 32 bits; above
//!    that the *operands* may already be rounded and there is nothing to
//!    recover, so the row's true statement is narrower than it was written and
//!    §12 now says which half is which. Pinned by
//!    [`row_03_wrapping_arithmetic_agrees`],
//!    [`row_03_wrapping_at_narrow_widths_agrees`] and
//!    [`row_03_wrapping_at_the_type_boundaries_agrees`].
//!  * **Row 5 was stale.** Both backends keep `.Some(.None)` distinct from
//!    `.None`; the row described a divergence the shipped toolchain does not
//!    have. §12 says so now and [`row_05_nested_option_is_distinct`] holds the
//!    agreement.
//!  * **Row 2 went the other way and came back.** Both native backends briefly
//!    narrowed `Checked` to `exact_int_range` so that `.None` above 2^53 was a
//!    property of the *language*; the ruling in
//!    `design/native/DECISIONS.md` is that `Checked` is bounded by the
//!    numbers the **backend** has, so a native `checkedAdd` reports
//!    two's-complement overflow and nothing else. The row is a listed
//!    divergence again, [`row_02_checked_above_the_exact_range`] pins both
//!    answers, and the band it covers came out of the shared conformance corpus
//!    to get here. `Saturating` was never bounded that way and
//!    [`row_02_saturating_is_bounded_by_the_type_on_both_backends`] says so.
//!  * **Two miscompiles**, both found by a row test refusing to build.
//!    `middle/lower.rs` interned `Str` and `Template` as two types, so a
//!    `match` whose arms are a literal and an interpolation did not verify;
//!    `middle/tail_calls.rs` labelled a merged group's forwarders `()`, so
//!    `even(3)` printed the empty string natively instead of `false` — and, one
//!    step on, panicked inside the debug backend of the day. Pinned by
//!    [`row_09_a_match_over_a_literal_and_an_interpolation`] and
//!    [`row_13_tail_calls_run_in_constant_stack`].
//!
//! # What a row test does
//!
//! One `.buri` source, compiled twice from one analysis:
//!
//! ```text
//! source -> analyze_snippet -> monomorphize -> prepare(Js)     -> select(Js)     -> main.mjs -> bun
//!                          \-> monomorphize -> prepare(native) -> select(native) -> objects  -> cc -> a.out
//! ```
//!
//! `actions::prepare` is the product's own seam — it is "the one place a
//! pipeline is chosen", and it is what decides that JavaScript does not run
//! `middle::native` — so the two halves here differ in exactly the way a real
//! build's two halves differ, and in nothing else. Then stdout is compared
//! **byte for byte**, and so are the exit status and, where a row is about one,
//! the abort message.
//!
//! Each row is its own `#[test]`, so a failure names the row rather than the
//! file.
//!
//! # Agreement is not the whole bar
//!
//! Two backends that agree on the wrong answer agree. So [`agree`] takes the
//! expected text as well and pins it: what is asserted is that JavaScript
//! prints it, that every native backend prints it, and that they are identical
//! — three claims, because the third alone would pass on a corpus that had
//! rotted on both sides at once.
//!
//! For the *divergent* rows [`diverge`] pins **both** documented behaviours
//! instead, and asserts that they still differ. A divergence that quietly
//! closed is a documentation bug in the other direction, and rows 2 and 5 are
//! what that looks like when nobody checks. [`diverge`] is also this file's
//! answer to "a suite that cannot fail proves nothing": it fails if the two
//! pipelines ever agree, so the comparison is demonstrably able to see a
//! difference.
//!
//! # Which backends
//!
//! Every row runs against every native backend this binary was built with, so a
//! failure says `stencil` or `llvm`. Both come from `backend::select` — stencil
//! at `Profile::Debug`, which is the selection a native debug build makes, and
//! LLVM at `Profile::Release` — so a row that cannot be built here is a row a
//! user could not build either.
//!
//! `cargo test -p buri --features backend-llvm --test native agreement::` is the
//! second half, and it runs: LLVM 21 compiles most of the rows and refuses the
//! rest for reasons of its own — `num.minValue`/`num.maxValue` have no body —
//! so it carries a
//! [`Native::partial`] note and a row it cannot compile is skipped with the
//! reason printed. Stencil carries no such note, so a refusal from it is a
//! failure — the note came off when it reached row parity, which is what made
//! it the debug backend a build is handed.
//! Where two backends compile a row they have never disagreed.
//!
//! A backend compiled in is not yet a backend this *host* can run: a host whose
//! `cc` built no stencil library, or one no library is built for at all (macOS
//! on x86-64), is left out by [`natives`] with a printed reason rather than
//! asked and failed. The question is the backend's own —
//! `stencil::unavailable_reason` — so these rows lit up with no edit here the
//! day x86-64 Linux got its entry point, and would again for a fourth target.
//!
//! With `--no-default-features` there is no native backend, and `main.rs`
//! does not declare this module at all. With one but no runtime archive, no
//! `cc`, no JavaScript engine, or no backend with a seat on this host, every
//! test returns early with a printed reason: `native_ready` is the same gate
//! `buri build` uses.
//!
//! # What is not here, and why
//!
//! * **The exhaustive float corpus.** `native/float_parity.rs` sweeps 3.8
//!   million doubles through `$f64` and `buri_rt_show_f64`; row 8 here is the
//!   cheap end-to-end variant — the same rendering reached through a whole
//!   compiled program rather than through a C driver — and repeating the sweep
//!   would add twenty minutes and no coverage.
//! * **What the native surface cannot reach.** `derive ToJson` and a `[T]`
//!   inside a derived `Show` are both refused by `missing_intrinsics`, and
//!   `Alloc` accounting exists on neither backend. Each is covered twice: an
//!   `#[ignore]`d agreement test that runs the day the gap closes, and a test
//!   asserting the gap is *still there*, so the ignore cannot rot into a lie.
//!   That is `native/conformance.rs`'s pattern, for its reason.
use buri::build::actions;
use buri::build::buildfile::{Arch, Platform};
use buri::compiler::backend::runtime_native::{ARCHIVE, ARCHIVE_NAME};
use buri::compiler::backend::{self, Backend, Options, Profile, Target};
use buri::compiler::driver;
use buri::compiler::middle::monomorphize;
use buri::compiler::modules::Role;
use buri::compiler::semantics::resolve::Checked;
use buri::diagnostics::{Diagnostics, SourceMap};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::OnceLock;

// -------------------------------------------------------------------
// The backends under test
// -------------------------------------------------------------------

/// One native backend, named.
///
/// A profile rather than a `Box<dyn Backend>`, because `backend::select`
/// takes one and this file goes through `select` wherever `select` can
/// answer: the rows are supposed to exercise the selection a build makes,
/// not a backend a test reached for.
#[derive(Clone, Copy)]
struct Native {
    name: &'static str,
    profile: Profile,
    /// `Some(why)` for a backend whose surface is admittedly narrower than
    /// the rows, in which case a row it cannot compile is **skipped with
    /// the reason printed** rather than failed.
    ///
    /// Only ever set for a backend that is not a candidate for the native
    /// *debug* seat, which is the selection a user gets by default: a refusal
    /// from a debug backend is this file's failure, and that is what keeps the
    /// tolerance from becoming a place for rows to go and die.
    partial: Option<&'static str>,
}

/// Never empty: the module is behind `any(backend-stencil, backend-llvm)`, so
/// at least one arm below is compiled in.
const NATIVES: &[Native] = &[
    // No `partial` note: this backend compiles every executable row, so a
    // refusal here is a failure rather than a skip.
    #[cfg(feature = "backend-stencil")]
    Native { name: "stencil", profile: Profile::Debug, partial: None },
    #[cfg(feature = "backend-llvm")]
    Native {
        name: "llvm",
        profile: Profile::Release,
        partial: Some(
            "the release backend, and its surface is narrower than the \
                 development backend's: \
                 `num.minValue`/`num.maxValue` have no body, so some rows \
                 cannot be asked of it. The `..rest` array pattern that used \
                 to be the other half of this sentence is emitted now \
                 (`Unit::array_slice`), which is what took \
                 `buri test --release` over the conformance corpus from 593 \
                 blocks to all 1111",
        ),
    },
];

impl Native {
    /// The backend, through `backend::select`.
    ///
    /// Every row here is the one `select` answers with for its profile, so a
    /// refusal is a failure and says which triple was refused. The release
    /// fallback below covers a toolchain whose `select` refuses `(native,
    /// Release)` while `backend-llvm` is compiled in.
    fn backend(self) -> Box<dyn Backend> {
        match backend::select(host_target(), self.profile) {
            Ok(b) => b,
            #[cfg(feature = "backend-llvm")]
            Err(_) if matches!(self.profile, Profile::Release) => {
                Box::new(backend::llvm::Llvm)
            }
            Err(message) => panic!("no `{}` backend: {message}", self.name),
        }
    }
}

fn host_target() -> Target {
    Target {
        platform: if cfg!(target_os = "macos") { Platform::Macos } else { Platform::Linux },
        arch: Some(if cfg!(target_arch = "aarch64") { Arch::Arm64 } else { Arch::X86_64 }),
    }
}

/// The JavaScript engine the rest of the suite runs, or `None`.
///
/// `BURI_JS` first, so this file answers the same question
/// `tests/harness/mod.rs` does and a machine that has configured one engine
/// does not silently get another.
fn engine() -> Option<String> {
    crate::shared::js_engine()
}

/// Why one backend cannot run a program on this host, or `None`.
///
/// The backend's own availability query rather than a `cfg!` here: stencil has
/// a library for x86-64 and no entry point to put in front of it, so it refuses
/// every row on that host, and asking it lets these rows light up with no edit
/// the day the entry point lands.
fn backend_unavailable(native: Native) -> Option<String> {
    #[cfg(feature = "backend-stencil")]
    if native.name == "stencil" {
        return backend::stencil::unavailable_reason();
    }
    let _ = native;
    None
}

/// The backends that can answer here, with a printed line naming each one that
/// cannot.
///
/// A backend with no host seat is left out *before* it is asked, because its
/// refusal is a fact about the host rather than about the row — and it is named
/// rather than dropped, so a column that stopped running says so.
fn natives(row: &str) -> Vec<Native> {
    let mut usable = Vec::new();
    for native in NATIVES {
        match backend_unavailable(*native) {
            Some(why) => {
                eprintln!("backend agreement: {row} not asked of `{}` ({why})", native.name)
            }
            None => usable.push(*native),
        }
    }
    usable
}

/// Why this host cannot answer a row, or `None`.
///
/// `native_ready` is the build system's own three questions — a backend
/// compiled in, a runtime archive built, a linker present — asked at
/// `Debug` because what it is really asking about is the host, and the
/// release arm is exactly the one `select` still refuses.
///
/// The last question is per backend: a binary whose every native backend is
/// unavailable here would compare JavaScript against nothing, which is the
/// silent pass this file exists to not be.
fn skip_reason() -> Option<String> {
    if !actions::native_ready(host_target(), Profile::Debug) {
        return Some(String::from("`native_ready` is false on this host"));
    }
    if engine().is_none() {
        return Some(String::from("no JavaScript engine on PATH"));
    }
    let unavailable: Vec<String> = NATIVES
        .iter()
        .filter_map(|n| backend_unavailable(*n).map(|why| format!("`{}`: {why}", n.name)))
        .collect();
    if unavailable.len() == NATIVES.len() {
        return Some(unavailable.join("; "));
    }
    None
}

/// The skip guard every row test opens with.
macro_rules! rows_or_skip {
    () => {
        if let Some(why) = skip_reason() {
            crate::ci::skipped("backend agreement", &why);
            return;
        }
    };
}

// -------------------------------------------------------------------
// Running one program through one pipeline
// -------------------------------------------------------------------

/// What one program printed, and how it ended.
struct Ran {
    status: i32,
    stdout: String,
    stderr: String,
}

impl Ran {
    /// The first line of standard error.
    ///
    /// The abort rows compare this rather than the whole stream, and the
    /// reason is a difference that is real and is *not* a §12 divergence:
    /// the JavaScript entry point catches the thrown abort and writes
    /// `e.stack` after the message (`generate.rs:302-308`), because on
    /// JavaScript there is a stack to write. Natively there is not —
    /// `cli/runtime/abort.rs` writes the message, a newline, and exits. §12
    /// rows 11 and 14 are about the message and the status, and both of
    /// those are on this line.
    fn first_error_line(&self) -> &str {
        self.stderr.lines().next().unwrap_or_default()
    }
}

/// A directory this *process* owns, per program and pipeline.
///
/// The process id is in the name because two overlapping `cargo test` runs
/// otherwise share it, and the second overwrites the binary the first is
/// executing — which on macOS is a child that never returns rather than an
/// error. The counter is because one row runs one source through two
/// pipelines or more.
fn workspace(name: &str) -> PathBuf {
    crate::sweep::once();
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("backend-agreement-{}", std::process::id()))
        .join(format!("{}-{n}", name.replace([' ', '(', ')', '.'], "-")));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// The runtime archive, written once for the process.
///
/// Six megabytes, and the same six megabytes for every row: a copy per
/// workspace is a third of a gigabyte written, linked against, and then left
/// behind under `CARGO_TARGET_TMPDIR`. Immutable once written and named by the
/// process id, so the concurrency `#[test]`s run under is fine and two
/// `cargo test` runs in two shells still do not share it —
/// `native/llvm.rs::archive` is the same lock for the same reason.
fn archive() -> &'static Path {
    static WRITTEN: OnceLock<PathBuf> = OnceLock::new();
    WRITTEN.get_or_init(|| {
        let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
            .join(format!("backend-agreement-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(ARCHIVE_NAME);
        std::fs::write(&path, ARCHIVE).unwrap();
        path
    })
}

/// The front end, once. Both pipelines are handed the same analysis, which
/// is what makes a difference between them a difference between backends.
fn analyze(row: &str, source: &str) -> (Checked, Vec<String>) {
    let mut map = SourceMap::new();
    let analysis = driver::analyze_snippet(&mut map, "main", source, Role::Entry);
    assert!(
        !analysis.diagnostics.has_errors(),
        "{row}: the program does not compile:\n{}",
        analysis.diagnostics.items.iter().map(|d| map.render(d, false)).collect::<Vec<_>>().join("\n")
    );
    let paths = analysis.loaded.modules.iter().map(|m| m.path.clone()).collect();
    (analysis.checked, paths)
}

/// Monomorphize, then the middle end composed for one target.
fn prepared(
    row: &str,
    checked: &Checked,
    paths: &[String],
    target: Target,
) -> monomorphize::Program {
    let entry = checked.entry.expect("the program exports `main`");
    let mut diagnostics = Diagnostics::new();
    let mut program = monomorphize::run(
        checked,
        paths.to_vec(),
        &mut diagnostics,
        monomorphize::Roots::Main(entry),
    );
    assert!(!diagnostics.has_errors(), "{row}: monomorphization failed");
    // The product's own seam: `middle::run` for everybody, `middle::native`
    // for the platforms that are not JavaScript.
    actions::prepare(&mut program, target);
    program
}

fn messages(diagnostics: &Diagnostics) -> String {
    diagnostics.items.iter().map(|d| d.message.clone()).collect::<Vec<_>>().join("; ")
}

/// Compile through the JavaScript backend and run the artifact.
fn run_js(row: &str, checked: &Checked, paths: &[String]) -> Ran {
    let target = Target { platform: Platform::Js, arch: None };
    let program = prepared(row, checked, paths, target);
    let opts = Options { profile: Profile::Debug, target, unit_prefix: "" };
    let mut backend = backend::select(target, Profile::Debug).expect("the JavaScript backend");
    let units = match backend.emit(&program, &checked.tables, &opts) {
        Ok(units) => units,
        Err(d) => {
            panic!("{row}: the JavaScript backend refused the program: {}", messages(&d))
        }
    };
    let dir = workspace(&format!("{row}-js"));
    let artifact = dir.join("main.mjs");
    std::fs::write(&artifact, &units.first().expect("one unit").bytes).unwrap();
    let engine = engine().expect("a JavaScript engine");
    let out = Command::new(&engine).arg(&artifact).output().unwrap();
    Ran {
        status: out.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&out.stdout).to_string(),
        stderr: String::from_utf8_lossy(&out.stderr).to_string(),
    }
}

/// Why one native backend will not compile a program, or the empty string
/// where it will.
///
/// Both halves, because a backend refuses in two places and the gap tests
/// care about either. `missing_intrinsics` answers *before* emission and is
/// where an unimplemented `FuncKind::Intrinsic` shows up — that is the hook
/// on the trait, and a key with no runtime row is what trips it. But a
/// structural operation is an `ir::Inst::Structural`, which exists only
/// after lowering and is therefore not in the program that hook is handed
/// (`llvm/mod.rs` says so where the hook is implemented), so a `deriveArray*`
/// can only be discovered by asking the backend to emit and reading the
/// diagnostic.
fn native_refusal(row: &str, native: Native, checked: &Checked, paths: &[String]) -> String {
    let program = prepared(row, checked, paths, host_target());
    let opts = Options { profile: native.profile, target: host_target(), unit_prefix: "" };
    let mut backend = native.backend();
    let missing = backend.missing_intrinsics(&program, &checked.tables);
    if !missing.is_empty() {
        return missing.join("; ");
    }
    match backend.emit(&program, &checked.tables, &opts) {
        Ok(_) => String::new(),
        Err(d) => messages(&d),
    }
}

/// Compile through one native backend, link, and run the executable.
///
/// `None` where a [`Native::partial`] backend refuses the program: the
/// reason is printed and the row is not asked of it. A backend with no
/// `partial` note refusing is a failure, which is what makes the tolerance
/// specific rather than general.
fn run_native(row: &str, native: Native, checked: &Checked, paths: &[String]) -> Option<Ran> {
    let target = host_target();
    let program = prepared(row, checked, paths, target);
    let opts = Options { profile: native.profile, target, unit_prefix: "" };
    let mut backend = native.backend();
    let missing = backend.missing_intrinsics(&program, &checked.tables);
    if !missing.is_empty() {
        let why = native.partial.unwrap_or_else(|| {
            panic!(
                "{row}: the `{}` backend is missing {missing:?} — if that is the gap \
                     the row is about, the row belongs with the gap tests rather than here",
                native.name
            )
        });
        eprintln!(
            "backend agreement: {row} not asked of `{}` (missing {missing:?}); it is {why}",
            native.name
        );
        return None;
    }
    let units = match backend.emit(&program, &checked.tables, &opts) {
        Ok(units) => units,
        Err(d) => {
            let why = native.partial.unwrap_or_else(|| {
                panic!(
                    "{row}: the `{}` backend refused the program: {}",
                    native.name,
                    messages(&d)
                )
            });
            eprintln!(
                "backend agreement: {row} not asked of `{}` ({}); it is {why}",
                native.name,
                messages(&d)
            );
            return None;
        }
    };
    assert!(!units.is_empty(), "{row}: the `{}` backend emitted no unit", native.name);

    let dir = workspace(&format!("{row}-{}", native.name));
    let mut objects = Vec::new();
    for unit in &units {
        let path = dir.join(&unit.name);
        std::fs::write(&path, &unit.bytes).unwrap();
        objects.push(path);
    }
    let binary = dir.join("program");
    // `build/link.rs`'s driver and its trailing arguments, rather than a list
    // spelled out again: on Linux those are now a whole static-PIE musl link
    // (`shared::product_cc`), and a harness that linked the old three `-l`s
    // would be asking the driver for a `libpthread.a` musl does not ship.
    let mut link = crate::shared::product_cc();
    link.arg("-o").arg(&binary);
    for object in &objects {
        link.arg(object);
    }
    link.arg(archive());
    link.args(crate::shared::product_link_args());
    let linked = link.output().unwrap();
    assert!(
        linked.status.success(),
        "{row}: the `{}` link failed:\n{}",
        native.name,
        String::from_utf8_lossy(&linked.stderr)
    );
    // **Under the heap check.** Every row here is a whole program run to
    // completion, which is exactly the population the runtime's exit audit is
    // a question about — so agreement about what was *printed* now travels
    // with agreement about what was *freed*, and the rows pay nothing for it
    // (`shared::ran_checked`, and `cli/runtime/memory.rs`'s heap-check
    // section).
    let ran = crate::shared::ran_checked(&binary);
    Some(Ran { status: ran.status, stdout: ran.stdout, stderr: ran.stderr })
}

// -------------------------------------------------------------------
// The four shapes a row can have
// -------------------------------------------------------------------

/// Both pipelines, run. JavaScript first, then one per native backend that
/// could compile the program.
fn both(row: &str, source: &str) -> (Ran, Vec<(&'static str, Ran)>) {
    let (checked, paths) = analyze(row, source);
    let js = run_js(row, &checked, &paths);
    let natives = natives(row)
        .into_iter()
        .filter_map(|n| run_native(row, n, &checked, &paths).map(|ran| (n.name, ran)))
        .collect();
    (js, natives)
}

/// A **must agree** row: every backend prints `expected`, exits zero, says
/// nothing on standard error, and gives back every block it took.
///
/// `expected` is asserted as well as agreement, because two backends that
/// agree on the wrong answer agree. The heap is asserted separately from the
/// status, so a leak is reported as a leak: the audit exits non-zero with its
/// own sentence on standard error, and "exited 134" is not a thing to read.
///
/// **There is no leaking variant of this function any more.** There was one,
/// `agree_leaking`, which pinned an exact block count for the two rows that
/// were known to leak — the projection off a generic call and the option
/// holding an array. Both were the same missing release and both are fixed, so
/// the permission went with them: a row that leaks now fails here, and a row
/// that leaks *on purpose* would have to say so in a function somebody writes
/// again and argues for.
fn agree(row: &str, source: &str, expected: &str) {
    let (js, natives) = both(row, source);
    assert_eq!(js.stderr, "", "{row}: JavaScript printed to standard error");
    assert_eq!(js.status, 0, "{row}: JavaScript exited {}", js.status);
    assert_eq!(js.stdout, expected, "{row}: JavaScript printed something else");
    for (name, ran) in &natives {
        if let Some(n) = crate::shared::leaked_blocks(ran.status, &ran.stderr) {
            panic!(
                "{row}: `{name}` leaked {n} block(s). \
                 `BURI_RT_HEAP_CHECK=trace` prints every surviving block."
            );
        }
        assert_eq!(ran.stderr, "", "{row}: `{name}` printed to standard error");
        assert_eq!(ran.status, 0, "{row}: `{name}` exited {}", ran.status);
        assert_eq!(
            ran.stdout, js.stdout,
            "{row}: `{name}` and JavaScript disagree.\n  javascript: {:?}\n  {name}: {:?}",
            js.stdout, ran.stdout
        );
    }
}

/// A **listed divergence** row: JavaScript prints one thing, every native
/// backend prints another, and both are pinned.
///
/// The inequality is asserted too, and that is what makes this file able to
/// fail: a comparison that could not tell the two pipelines apart would
/// fail here rather than passing everywhere.
fn diverge(row: &str, source: &str, javascript: &str, native: &str) {
    assert_ne!(
        javascript, native,
        "{row}: a divergence row whose two sides are equal is not a divergence"
    );
    let (js, natives) = both(row, source);
    assert_eq!(js.stdout, javascript, "{row}: JavaScript's documented answer moved");
    for (name, ran) in &natives {
        assert_eq!(
            ran.stdout, native,
            "{row}: `{name}`'s documented answer moved (JavaScript printed {:?})",
            js.stdout
        );
    }
}

/// An **abort** row: the same message on the same stream with the same
/// status, and the same output before it — the last thing the program
/// printed is flushed above the reason it stopped, on both backends.
fn abort_agrees(row: &str, source: &str, stdout: &str, message: &str) {
    let (js, natives) = both(row, source);
    assert_eq!(js.stdout, stdout, "{row}: JavaScript printed something else before aborting");
    assert_eq!(js.first_error_line(), message, "{row}: JavaScript's abort message moved");
    assert_eq!(js.status, 1, "{row}: JavaScript exited {}", js.status);
    for (name, ran) in &natives {
        assert_eq!(ran.stdout, js.stdout, "{row}: `{name}` printed something else");
        assert_eq!(
            ran.first_error_line(),
            js.first_error_line(),
            "{row}: `{name}` and JavaScript disagree about the abort message"
        );
        assert_eq!(ran.status, js.status, "{row}: `{name}` exited {}", ran.status);
    }
}

/// A **gap** row: JavaScript runs it, and every native backend says which
/// intrinsic it has no body for — before a byte of code is generated, which
/// is what that hook is for.
///
/// The assertion is two-sided on purpose. A gap that closed makes this fail
/// rather than leaving an `#[ignore]` next door describing a limitation
/// that no longer exists.
fn gap(row: &str, source: &str, wanted: &[&str]) {
    let (checked, paths) = analyze(row, source);
    let js = run_js(row, &checked, &paths);
    assert_eq!(js.status, 0, "{row}: JavaScript could not run it either: {}", js.stderr);
    for native in natives(row) {
        let refusal = native_refusal(row, native, &checked, &paths);
        // A [`Native::partial`] backend has its own reasons to refuse and
        // its own reasons not to, and neither is what a gap row is about — so
        // its answer is reported rather than asserted on. The two surfaces are
        // not the same set of keys, and a row's gap has to be the debug
        // backend's to be this file's.
        if let Some(why) = native.partial {
            eprintln!(
                "backend agreement: {row} on `{}` answered {refusal:?}; it is {why}",
                native.name
            );
            continue;
        }
        assert!(
            !refusal.is_empty(),
            "{row}: the `{}` backend now compiles this — delete the gap test \
                 and un-ignore the agreement test beside it",
            native.name
        );
        for key in wanted {
            assert!(
                refusal.contains(key),
                "{row}: the `{}` backend refused with {refusal:?}, which does not \
                     name `{key}` — the gap moved, so the reason has to move with it",
                native.name
            );
        }
    }
}

// -------------------------------------------------------------------
// Row 1 — `Int` overflow
// -------------------------------------------------------------------

/// Undefined on both, and the two implementations differ: a `BigInt` has no
/// width to overflow, so JavaScript answers the exact sum, and a native
/// backend wraps.
///
/// This is what is *left* of row 1 now that `I64` is a `BigInt`. The old
/// divergence was that neither answer was the sum; the remaining one is that
/// both are exact and only one of them is an `I64`.
///
/// Both answers are pinned rather than compared, which is §11.1's own
/// position — "descriptions of two implementations rather than a
/// specification of one".
#[test]
fn row_01_int_overflow() {
    rows_or_skip!();
    diverge(
        "row 1",
        r#"
from "core/host" import { stdout };
from "core/io" import * as io;
from "core/num" import * as num;

export fn main(): Result<(), Str> {
  let m = num.maxValue<Int>();
  let over = m + 1;
  let _ = io.println(stdout, "${over}").ignore();
  .Ok(())
}
"#,
        "9223372036854775808\n",
        "-9223372036854775808\n",
    );
}

/// The old ceiling seen through `show` rather than through arithmetic — and
/// it is gone: a `BigInt` names every `I64` and every `U64`, so the extremes
/// print the same digits on both backends.
#[test]
fn row_01_integer_show_at_the_64_bit_extremes() {
    rows_or_skip!();
    agree(
        "row 1 show",
        r#"
from "core/host" import { stdout };
from "core/io" import * as io;
from "core/num" import * as num;

export fn main(): Result<(), Str> {
  let a = num.minValue<I64>();
  let b = num.maxValue<I64>();
  let c = num.maxValue<U64>();
  let _ = io.println(stdout, "${a} ${b} ${c}").ignore();
  .Ok(())
}
"#,
        "-9223372036854775808 9223372036854775807 18446744073709551615\n",
    );
}

// -------------------------------------------------------------------
// Row 2 — `Checked` above the exact range
// -------------------------------------------------------------------

/// §12 row 2 and SPEC §6.2.2. `Checked` is bounded by the numbers the
/// **backend** has, and both backends now have the same ones: `.Some(v)`
/// means `v` is the exact true result, over the type's own range, on either.
///
/// The row was a band — above `2^53` and inside `I64`, where JavaScript said
/// `.None` because a `number` could not say which integer the answer was.
/// A `BigInt` says it, so the band is empty and the row is an agreement row.
/// Every case that used to sit in the band is asserted here: `1 << 60` plus
/// one, and `maxValue<I64>()` unchanged. Either side of the old band is
/// asserted too — `100 + 20` is `.Some`, a division by zero and
/// `maxValue<I64>() + 1` are `.None` — so a change that moved the bound in
/// one direction cannot pass by moving the whole row.
///
/// `conformance/lib/numbers/test/integers.buri` may now assert the band
/// itself, and does: `native/conformance.rs` runs that file natively, and
/// what both backends answer is what belongs there.
#[test]
fn row_02_checked_above_the_exact_range() {
    rows_or_skip!();
    agree(
        "row 2",
        r#"
from "core/bits" import * as bits;
from "core/host" import { stdout, alloc };
from "core/io" import * as io;
from "core/str" import * as str;

fn tell(x: Option<Int>): Str {
  match (x) { .Some(v) => str.format(alloc, "Some ${v}"), .None => "None" }
}

export fn main(): Result<(), Str> {
  let big = bits.shl(1, 60);
  // `maxValue<I64>()` as a literal: `num.minValue`/`num.maxValue` have no LLVM
  // body yet, and a row this one is about should not be skipped there.
  let top: Int = 9223372036854775807;
  let a = tell(big.checkedAdd(1));
  let b = tell(top.checkedAdd(0));
  let small: Int = 100;
  let c = tell(small.checkedAdd(20));
  let d = tell(small.checkedDiv(0));
  let e = tell(top.checkedAdd(1));
  let _ = io.println(stdout, "${a} ${b} ${c} ${d} ${e}").ignore();
  .Ok(())
}
"#,
        "Some 1152921504606846977 Some 9223372036854775807 Some 120 None None\n",
    );
}

/// The other half of row 2: `Saturating` has **no** second bound to lose,
/// and did not move when `Checked` did.
///
/// `$sat` clamps at `int_range` and both native backends clamp at
/// `int_range`, so the family was type-bounded everywhere before the ruling
/// and is type-bounded everywhere after it. It is asserted rather than
/// stated because "unaffected" is the claim a change like that quietly
/// breaks.
///
/// Every value here is inside 2^53 for the same reason it always was, which
/// is that the row is about the bound and not about the width.
#[test]
fn row_02_saturating_is_bounded_by_the_type_on_both_backends() {
    rows_or_skip!();
    agree(
        "row 2 saturating",
        r#"
from "core/host" import { stdout };
from "core/io" import * as io;

export fn main(): Result<(), Str> {
  let a: I32 = 2147483000;
  let b: U8 = 250;
  let c: I8 = 100;
  let d: I32 = 46341;
  let e: I32 = 0 - 2147483647;
  let _ = io.println(stdout, "${a.saturatingAdd(1000)} ${b.saturatingAdd(10)} ${b.saturatingSub(255)}").ignore();
  let _ = io.println(stdout, "${c.saturatingMul(2)} ${c.saturatingMul(0 - 2)} ${d.saturatingMul(d)}").ignore();
  let _ = io.println(stdout, "${e.saturatingSub(1000)}").ignore();
  .Ok(())
}
"#,
        "2147483647 255 0\n127 -128 2147483647\n-2147483648\n",
    );
}

// -------------------------------------------------------------------
// Row 3 — `Wrapping`
// -------------------------------------------------------------------

/// The vector table row 3 asks for, at 64 bits.
///
/// Every operand *and* every result here is an exact double, which is what
/// makes the row askable at all: on JavaScript a `U64` above 2^53 is not
/// the value the program wrote, so a vector built on one is testing the
/// literal rather than the operation.
///
/// Even so, agreement at 64 bits is narrower than the row claims, and this
/// is the honest boundary. `(2^62 + 1024).wrappingMul(4)` is 4096 natively
/// and 0 on JavaScript, with both operands exact and the answer exact,
/// because the *intermediate* 2^64 + 4096 rounds before the wrap — and the
/// repair, computing in `BigInt`, is not available here: it changes
/// `maxValue<U64>().wrappingAdd(1)` from 0 to 1, which is the case
/// `conformance/lib/numbers/test/integers.buri` pins and the case where the
/// *operand* is already 2^64. So at 64 bits and above, `Wrapping` agrees
/// where the intermediate stays inside 2^53 and is row 1 where it does not,
/// and this table is the first half.
#[test]
fn row_03_wrapping_arithmetic_agrees() {
    rows_or_skip!();
    agree(
        "row 3",
        r#"
from "core/host" import { stdout };
from "core/io" import * as io;
from "core/num" import * as num;

export fn main(): Result<(), Str> {
  // 2^32 * 2^32 = 2^64, which wraps to zero at 64 bits.
  let p: I64 = 4294967296;
  let a = p.wrappingMul(p);
  let c = num.minValue<I64>().wrappingAdd(num.minValue<I64>());
  let d = num.minValue<I64>().wrappingMul(2);
  let e: I64 = 3;
  let f = e.wrappingMul(5);
  let u: U64 = 9223372036854775808;
  let g = u.wrappingMul(2);
  let w: U64 = 18446744073709549568;
  let i = w.wrappingAdd(2048);
  let x: I64 = 0 - 7;
  let y = x.wrappingSub(9);
  let _ = io.println(stdout, "${a} ${c} ${d} ${f} ${g} ${i} ${y}").ignore();
  .Ok(())
}
"#,
        "0 0 0 15 0 0 -16\n",
    );
}

/// The boundary cases the conformance corpus pins, run through both
/// pipelines rather than only through `buri test`.
///
/// `maxValue<U64>() + 1 == 0` is true on JavaScript because the operand is
/// already 2^64 and the double sum rounds back to it — a compensating
/// error — and true natively because it is simply true. They agree, and the
/// row is pinned on the agreement rather than on the reason.
#[test]
fn row_03_wrapping_at_the_type_boundaries_agrees() {
    rows_or_skip!();
    agree(
        "row 3 boundaries",
        r#"
from "core/host" import { stdout };
from "core/io" import * as io;
from "core/num" import * as num;

export fn main(): Result<(), Str> {
  let a: U64 = 18446744073709551615;
  let b = a.wrappingAdd(1);
  let c: U64 = 0;
  // Printed as a verdict rather than as a number: the value is `maxValue<U64>`,
  // which is row 1's ceiling and renders differently on the two backends.
  let d = c.wrappingSub(1) == a;
  let e: U128 = 340282366920938463463374607431768211455;
  let f = e.wrappingAdd(1);
  let g: I64 = 9223372036854775807;
  let h = g.wrappingAdd(1) == num.minValue<I64>();
  let _ = io.println(stdout, "${b} ${d} ${f} ${h}").ignore();
  .Ok(())
}
"#,
        "0 true 0 true\n",
    );
}

/// The same, at the narrow widths — where the intermediate leaves 2^53 and
/// the answer never does, so there is no precision argument to hide behind.
///
/// `4294967295 * 4294967295` is 18446744065119617025 and its low 32 bits
/// are 1. The double is even, so wrapping *it* gives 0. This is the vector
/// that found the bug.
#[test]
fn row_03_wrapping_at_narrow_widths_agrees() {
    rows_or_skip!();
    agree(
        "row 3 narrow",
        r#"
from "core/host" import { stdout };
from "core/io" import * as io;
from "core/num" import * as num;

export fn main(): Result<(), Str> {
  let a: U32 = 4294967295;
  let b = a.wrappingMul(a);
  let c: U32 = 65536;
  let d = c.wrappingMul(c);
  let e = num.minValue<I32>();
  let f = e.wrappingMul(e);
  let g: U16 = 65535;
  let h = g.wrappingMul(g);
  let i: U8 = 255;
  let j = i.wrappingMul(i);
  let k: I8 = 127;
  let l = k.wrappingAdd(1);
  let _ = io.println(stdout, "${b} ${d} ${f} ${h} ${j} ${l}").ignore();
  .Ok(())
}
"#,
        "1 0 0 1 1 -128\n",
    );
}

// -------------------------------------------------------------------
// Row 4 — 128-bit arithmetic
// -------------------------------------------------------------------

/// ~~A listed divergence~~ — an agreement row. JavaScript had no 128-bit
/// integer to compute in and computed in a double instead, which is how
/// `1000000007` cubed came back as `1.0000000210000002e+27`.
#[test]
fn row_04_wide_integer_arithmetic() {
    rows_or_skip!();
    agree(
        "row 4",
        r#"
from "core/host" import { stdout };
from "core/io" import * as io;

export fn main(): Result<(), Str> {
  let a: I128 = 1000000007;
  let b = a * a * a;
  let _ = io.println(stdout, "${b}").ignore();
  .Ok(())
}
"#,
        "1000000021000000147000000343\n",
    );
}

/// `show` at the 128-bit extremes, which is the same row read off a constant
/// rather than out of a multiplication.
#[test]
fn row_04_integer_show_at_the_128_bit_extremes() {
    rows_or_skip!();
    agree(
        "row 4 show",
        r#"
from "core/host" import { stdout };
from "core/io" import * as io;
from "core/num" import * as num;

export fn main(): Result<(), Str> {
  let a = num.minValue<I128>();
  let b = num.maxValue<I128>();
  let c = num.maxValue<U128>();
  let _ = io.println(stdout, "${a} ${b} ${c}").ignore();
  .Ok(())
}
"#,
        "-170141183460469231731687303715884105728 170141183460469231731687303715884105727 \
             340282366920938463463374607431768211455\n",
    );
}

// -------------------------------------------------------------------
// Row 5 — `Option<Option<T>>`
// -------------------------------------------------------------------

/// §12 row 5 says `.Some(.None)` and `.None` are the same value on
/// JavaScript. **They are not**, and have not been since `$some`/`$val`
/// grew the `$n` depth counter (`runtime.js`): "the generated code knows
/// its types and wraps only there".
///
/// So this is a second stale divergence, and the row is an agreement row.
/// Three levels deep, through a `match`, through a derived `Show` and
/// through a derived `Eq` — because the collision the row is about is in
/// the *representation*, and each of those three reads it differently.
#[test]
fn row_05_nested_option_is_distinct() {
    rows_or_skip!();
    agree(
        "row 5",
        r#"
from "core/host" import { stdout, alloc };
from "core/io" import * as io;
from "core/str" import * as str;

export struct Box3 { v: Option<Option<Option<Int>>> }
derive Show, Eq for Box3;

fn tell(x: Option<Option<Int>>): Str {
  match (x) {
    .Some(inner) => match (inner) {
      .Some(v) => str.format(alloc, "some some ${v}"),
      .None => "some none",
    },
    .None => "none",
  }
}

export fn main(): Result<(), Str> {
  let a: Option<Option<Int>> = .Some(.None);
  let b: Option<Option<Int>> = .None;
  let c: Option<Option<Int>> = .Some(.Some(7));
  let same = a == b;
  let d = Box3 { v: .Some(.Some(.None)) };
  let e = Box3 { v: .Some(.None) };
  let f = Box3 { v: .None };
  let _ = io.println(stdout, "${tell(a)} | ${tell(b)} | ${tell(c)} | ${same}").ignore();
  let _ = io.println(stdout, "${d.show(alloc)} | ${e.show(alloc)} | ${f.show(alloc)} | ${d == e}").ignore();
  .Ok(())
}
"#,
        "some none | none | some some 7 | false\n\
             Box3 { v: .Some(.Some(.None)) } | Box3 { v: .Some(.None) } | \
             Box3 { v: .None } | false\n",
    );
}

// -------------------------------------------------------------------
// Rows 6 and 7 — `Str`
// -------------------------------------------------------------------

/// `len` is a scalar count on both, including on astral input — where the
/// JavaScript answer is *not* `String#length` — and a combining sequence is
/// two scalars rather than one grapheme.
#[test]
fn row_06_str_len_counts_scalars() {
    rows_or_skip!();
    agree(
        "row 6",
        r#"
from "core/host" import { stdout };
from "core/io" import * as io;

export fn main(): Result<(), Str> {
  let a = "abc".len();
  let b = "\u{1F600}".len();
  let c = "e\u{301}".len();
  let d = "".len();
  let e = "\u{1F600}\u{1F600}ab".len();
  let _ = io.println(stdout, "${a} ${b} ${c} ${d} ${e}").ignore();
  .Ok(())
}
"#,
        "3 1 2 0 4\n",
    );
}

/// Row 17: text orders by Unicode scalar value on both backends.
///
/// The row that has to be measured rather than reasoned about, because the two
/// backends reach it from opposite directions and each has a *cheaper* answer
/// that is the wrong one. A JavaScript string is UTF-16, so `<` on one orders
/// by code unit and puts every astral scalar below every scalar in
/// U+E000..U+FFFF — a surrogate pair begins at 0xD800. A native `Str` is UTF-8,
/// so a `memcmp` orders by scalar value and says the opposite. The language
/// says scalar value (`core/str`'s `compare`), so `$str_compare` spells that
/// order out instead of using `<` and `buri_rt_str_compare` is the `memcmp`.
///
/// Every input here straddles the boundary the two orders disagree on. `sort`
/// and `Char` are in the same program because they are the same conformance:
/// `[Str].sort` is `Ord`, `<` is `Ord`, and a `Char` is a one-character string
/// on JavaScript, so all three used to come out of `<`.
#[test]
fn row_17_text_orders_by_scalar_value() {
    rows_or_skip!();
    agree(
        "row 17",
        r#"
from "core/host" import { stdout, alloc };
from "core/io" import * as io;
from "core/list" import * as list;

export fn main(): Result<(), Str> {
  // U+E000 is private use and U+1F600 is an emoji: 57344 below 128512.
  // JavaScript's `<` answers the other way round.
  let a = "\u{e000}" < "\u{1f600}";
  // The pair `cli/runtime/text.rs` pins, from the other side of U+FFFF.
  let b = "\u{fffd}" < "\u{10000}";
  // A shared prefix, so the decision is made past it.
  let c = "x\u{e000}" < "x\u{1f600}";
  // A `Char` is a scalar, and orders like one.
  let d = '\u{e000}' < '\u{1f600}';
  // Sorting is the same conformance, so it moves with them.
  let e = ["\u{1f600}", "\u{e000}", "a"].sort(alloc) == ["a", "\u{e000}", "\u{1f600}"];
  let _ = io.println(stdout, "${a} ${b} ${c} ${d} ${e}").ignore();
  .Ok(())
}
"#,
        "true true true true true\n",
    );
}

/// Row 15: `char.toUpper` where the full case mapping is not one scalar.
///
/// `"ß".toUpperCase()` is `"SS"`, and JavaScript hands that back as a `Char` —
/// a value of two scalars, which the type does not have. Natively a `Char` is
/// one scalar and there is nothing equal to `"SS"` to answer, so the answer is
/// the **first** scalar of the full mapping, which is what `codePointAt(0)`
/// reads out of the JavaScript one.
///
/// So the two agree wherever the result is read as a scalar — `toU32`, `==`,
/// `compare` — and part company only where the whole `Char` is *rendered*,
/// which is the case this test pins. `cli/runtime/char.rs` §3 is the argument;
/// this is the measurement.
#[test]
fn row_15_char_case_of_a_multi_scalar_mapping() {
    rows_or_skip!();
    diverge(
        "row 15",
        r#"
from "core/host" import { stdout };
from "core/io" import * as io;

export fn main(): Result<(), Str> {
  let sharp = '\u{00df}'.toUpper();
  let scalar = sharp.toU32();
  let ligature = '\u{fb00}'.toUpper();
  let ordinary = 'a'.toUpper();
  let _ = io.println(stdout, "${sharp} ${scalar} ${ligature} ${ordinary}").ignore();
  .Ok(())
}
"#,
        "SS 83 FF A\n",
        "S 83 F A\n",
    );
}

/// `slice` clamps rather than aborting, at both boundaries and when the
/// range is inverted.
#[test]
fn row_07_str_slice_clamps() {
    rows_or_skip!();
    agree(
        "row 7",
        r#"
from "core/host" import { stdout };
from "core/io" import * as io;

export fn main(): Result<(), Str> {
  let s = "abcdef";
  let a = s.slice(0, 100);
  let b = s.slice(4, 2);
  let c = s.slice(10, 20);
  let d = s.slice(2, 4);
  let e = s.slice(6, 6);
  let _ = io.println(stdout, "${a}|${b}|${c}|${d}|${e}").ignore();
  .Ok(())
}
"#,
        "abcdef|||cd|\n",
    );
}

// -------------------------------------------------------------------
// Row 8 — floats
// -------------------------------------------------------------------

/// The end-to-end half of the float promise: the four presentation cases,
/// both boundaries of each, and the three values that are not numbers.
///
/// `native/float_parity.rs` is the corpus — 3.8 million doubles through
/// `$f64` and `buri_rt_show_f64`. This one asks the smaller question that
/// corpus cannot: whether a *compiled program* prints them, which puts the
/// whole pipeline between the constant and the characters.
#[test]
fn row_08_float_rendering() {
    rows_or_skip!();
    agree(
        "row 8",
        r#"
from "core/host" import { stdout };
from "core/io" import * as io;

export fn main(): Result<(), Str> {
  let a = 0.1 + 0.2;
  let b = 1.0;
  let c = 1e21;
  let d = 1e20;
  let e = 0.000001;
  let f = 0.0000001;
  let g = 0.0 - 0.0;
  let h = 1.0 / 0.0;
  let i = 0.0 / 0.0;
  let j = 1.0 / 3.0;
  let k = 0.0 - 1.5;
  let _ = io.println(stdout, "${a} ${b} ${c} ${d} ${e} ${f} ${g} ${h} ${i} ${j} ${k}").ignore();
  .Ok(())
}
"#,
        "0.30000000000000004 1.0 1e+21 100000000000000000000.0 0.000001 1e-7 0.0 \
             inf NaN 0.3333333333333333 -1.5\n",
    );
}

// -------------------------------------------------------------------
// Row 9 — derived `Show`
// -------------------------------------------------------------------

/// A struct, an enum with all three variant shapes, a nested struct, an
/// `Option` and a `Result` — field order, separators, and the quoting of a
/// `Str` and a `Char`, all of it byte for byte.
///
/// One backend walks a descriptor at run time and the other generated the
/// function at compile time (§9), which is what makes this the row that
/// would cost the most to lose: a `Show` that differed between backends
/// would make every golden test in every repository backend-specific.
#[test]
fn row_09_derived_show() {
    rows_or_skip!();
    agree(
        "row 9",
        r#"
from "core/host" import { stdout, alloc };
from "core/io" import * as io;

export struct Inner { n: I8, flag: Bool }
export enum Shape { Dot, Line(Int, Int), Named { label: Str, at: Inner } }
export struct Outer {
  id: Int,
  tag: Char,
  inner: Inner,
  shape: Shape,
  maybe: Option<Str>,
  res: Result<Int, Str>,
}
derive Show for Inner;
derive Show for Shape;
derive Show for Outer;

export fn main(): Result<(), Str> {
  let i = Inner { n: 0 - 3, flag: true };
  let o = Outer {
    id: 7,
    tag: 'q',
    inner: i,
    shape: .Named { label: "a\"b\\c\td", at: i },
    maybe: .Some("x"),
    res: .Err("bad"),
  };
  let none: Option<Str> = .None;
  let ok: Result<Int, Str> = .Ok(5);
  let p = Outer { id: 0, tag: 'z', inner: i, shape: .Dot, maybe: none, res: ok };
  let _ = io.println(stdout, o.show(alloc)).ignore();
  let _ = io.println(stdout, Shape.Dot.show(alloc)).ignore();
  let _ = io.println(stdout, Shape.Line(1, 0 - 2).show(alloc)).ignore();
  let _ = io.println(stdout, p.show(alloc)).ignore();
  .Ok(())
}
"#,
        "Outer { id: 7, tag: 'q', inner: Inner { n: -3, flag: true }, \
             shape: .Named { label: \"a\\\"b\\\\c\\td\", at: Inner { n: -3, flag: true } }, \
             maybe: .Some(\"x\"), res: .Err(\"bad\") }\n\
             .Dot\n\
             .Line(1, -2)\n\
             Outer { id: 0, tag: 'z', inner: Inner { n: -3, flag: true }, shape: .Dot, \
             maybe: .None, res: .Ok(5) }\n",
    );
}

/// Every integer width, at values a double holds exactly — which is every
/// value of every type up to 32 bits, and the exact range of the wider
/// ones. This is the part of "integer `show` at every width" that must
/// agree; [`row_01_integer_show_at_the_64_bit_extremes`] and
/// [`row_04_integer_show_at_the_128_bit_extremes`] are the part that
/// cannot.
#[test]
fn row_09_integer_show_at_every_width() {
    rows_or_skip!();
    agree(
        "row 9 integers",
        r#"
from "core/host" import { stdout };
from "core/io" import * as io;
from "core/num" import * as num;

export fn main(): Result<(), Str> {
  let a = num.minValue<I8>();
  let b = num.maxValue<I8>();
  let c = num.minValue<I16>();
  let d = num.maxValue<I16>();
  let e = num.minValue<I32>();
  let f = num.maxValue<I32>();
  let g = num.minValue<U8>();
  let h = num.maxValue<U8>();
  let i = num.maxValue<U16>();
  let j = num.maxValue<U32>();
  let k: I64 = 0 - 9007199254740991;
  let l: U64 = 9007199254740991;
  let m: Int = 1234567890123;
  let n: I128 = 0 - 9007199254740991;
  let o: U128 = 9007199254740991;
  let _ = io.println(stdout, "${a} ${b} ${c} ${d} ${e} ${f} ${g} ${h} ${i} ${j}").ignore();
  let _ = io.println(stdout, "${k} ${l} ${m} ${n} ${o}").ignore();
  .Ok(())
}
"#,
        "-128 127 -32768 32767 -2147483648 2147483647 0 255 65535 4294967295\n\
             -9007199254740991 9007199254740991 1234567890123 -9007199254740991 \
             9007199254740991\n",
    );
}

/// `Bool`, `Char` and `Str` under a derived `Show`, at every escape either
/// backend has an opinion about.
///
/// The two opinions are not the same opinion, and the backends agree about
/// that: a `Str` escapes `"`, `\`, tab, newline, carriage return and every
/// other control character as `\u00XX`; a `Char` is wrapped in single
/// quotes and escapes nothing at all, so `'\\'` prints as one backslash.
/// Whatever one thinks of that, it is one rendering rather than two, which
/// is what the row asks.
#[test]
fn row_09_bool_char_and_str_show() {
    rows_or_skip!();
    agree(
        "row 9 text",
        r#"
from "core/host" import { stdout, alloc };
from "core/io" import * as io;

export struct T { s: Str, c: Char, b: Bool }
derive Show for T;

export fn main(): Result<(), Str> {
  let a = T { s: "quote\" back\\ tab\t nl\n cr\r nul\u{0}", c: '"', b: true };
  let b = T { s: "\u{1F600} caf\u{e9}", c: '\u{1F600}', b: false };
  let d = T { s: "", c: '\\', b: true };
  let _ = io.println(stdout, a.show(alloc)).ignore();
  let _ = io.println(stdout, b.show(alloc)).ignore();
  let _ = io.println(stdout, d.show(alloc)).ignore();
  let _ = io.println(stdout, "${a.s.len()} ${b.s.len()} ${a.b} ${b.b}").ignore();
  .Ok(())
}
"#,
        "T { s: \"quote\\\" back\\\\ tab\\t nl\\n cr\\r nul\\u0000\", c: '\"', b: true }\n\
             T { s: \"\u{1F600} caf\u{e9}\", c: '\u{1F600}', b: false }\n\
             T { s: \"\", c: '\\', b: true }\n\
             30 6 true false\n",
    );
}

/// The miscompile a row test found by refusing to build.
///
/// `Str` widens to `Template` in argument position, and it does that by
/// *wrapping* the expression in a one-hole `Template`; lowering hands a
/// string hole's value straight back, typed `Str`. So a `match` whose type
/// is `Template` because one arm interpolates had arms producing `Str`, and
/// the native pipeline rejected its own IR — "b3 passes v8 to b1, whose
/// parameter is a different type" — on a program the JavaScript backend
/// compiles and runs. `lower.rs`'s interner now maps `Template` to `Str`,
/// which VALUE-MODEL.md §3.3 says they are.
///
/// Its own row because the shape is ordinary — a `match` producing a
/// message — and because a regression here is a *refusal to compile*
/// rather than a wrong answer, which no comparison of outputs would catch.
#[test]
fn row_09_a_match_over_a_literal_and_an_interpolation() {
    rows_or_skip!();
    agree(
        "row 9 template join",
        r#"
from "core/host" import { stdout };
from "core/io" import * as io;

export fn main(): Result<(), Str> {
  let o: Option<Int> = .Some(5);
  let n: Option<Int> = .None;
  let a = match (o) { .Some(v) => "some ${v}", .None => "none" };
  let b = match (n) { .Some(v) => "some ${v}", .None => "none" };
  let _ = io.println(stdout, a).ignore();
  let _ = io.println(stdout, b).ignore();
  .Ok(())
}
"#,
        "some 5\nnone\n",
    );
}

/// Derived `Eq` and `Ord`: the *verdicts*, over a struct compared
/// field-by-field and an enum compared by variant order and then payload.
#[test]
fn row_09_derived_eq_and_ord_verdicts() {
    rows_or_skip!();
    agree(
        "row 9 eq ord",
        r#"
from "core/host" import { stdout };
from "core/io" import * as io;
from "core/order" import { Order };

export struct P { a: Int, b: Str }
export enum E { A, B(Int), C { x: Int } }
derive Eq, Ord for P;
derive Eq, Ord for E;

fn name(o: Order): Str { match (o) { .Less => "lt", .Equal => "eq", .Greater => "gt" } }

export fn main(): Result<(), Str> {
  let p = P { a: 1, b: "m" };
  let q = P { a: 1, b: "n" };
  let r = P { a: 2, b: "a" };
  let x = name(p.compare(q));
  let y = name(r.compare(q));
  let z = name(p.compare(p));
  let eq = p == q;
  let ee = name(E.A.compare(E.B(1)));
  let ef = name(E.B(2).compare(E.B(1)));
  let eg = name(E.C { x: 1 }.compare(E.B(9)));
  let _ = io.println(stdout, "${x} ${y} ${z} ${eq} ${ee} ${ef} ${eg}").ignore();
  .Ok(())
}
"#,
        "lt gt eq false lt gt gt\n",
    );
}

/// Derived `Eq` over an `F64` field: the float facts SPEC 6.2 and 7.2 pin, on
/// every backend.
///
/// SPEC 6.2: "`==` on floats is an equivalence relation … `-0.0` equals `0.0`
/// and `NaN` equals `NaN`." SPEC 7.2: a derived `Eq` inherits that, so it is
/// reflexive at every value. The same rule is read here at four depths — the
/// bare primitive, two separately built aggregates, one aggregate against
/// itself, and the sign of zero the comparison must ignore — and the ordering
/// operators are read beside them because they did *not* move: `NaN < NaN` is
/// still false, which is what makes the ruling a change to `==` alone.
///
/// Every value below is built by a call so that two equal values are two
/// objects; written as literals the compiler may share one, and then the
/// comparison would be answered by identity and would prove nothing. That is
/// what `built` asks and `itself` does not: `itself` is the referential case, and
/// the two now agree because `==` is reflexive rather than because one
/// backend has objects and the other does not.
#[test]
fn row_09_derived_eq_on_a_float_field() {
    rows_or_skip!();
    agree(
        "row 9 float eq",
        r#"
from "core/host" import { stdout };
from "core/io" import * as io;

export struct F { x: Float }
derive Eq for F;

fn mk(x: Float): F { F { x: x } }
fn zeroF(): Float { 0.0 }
fn negZeroF(): Float { -0.0 }
fn notANumber(): Float { zeroF() / zeroF() }

export fn main(): Result<(), Str> {
  let n = notANumber();
  let bare = n == n;
  let built = mk(notANumber()) == mk(notANumber());
  let f = mk(notANumber());
  let itself = f == f;
  let mixed = mk(notANumber()) == mk(zeroF());
  let pz = mk(zeroF()) == mk(negZeroF());
  let nz = mk(negZeroF()) == mk(zeroF());
  let lt = n < n;
  let le = n <= n;
  let _ = io.println(stdout, "${bare} ${built} ${itself} ${mixed} ${pz} ${nz} ${lt} ${le}").ignore();
  .Ok(())
}
"#,
        "true true true false true true false false\n",
    );
}

/// `==` on floats in every position a comparison can be read from.
///
/// The row above reads the comparison as a **value**; this one reads the same
/// source comparison from the four places a *branch* consumes one. They are not
/// the same code path: a backend is free to fuse a comparison into the
/// terminator that tests it, and the stencil backend did — with C's `==` rather
/// than the language's equivalence. So `a == b` was true and `if (a == b)` took
/// the false edge in one program, and a `compare` written as a chain of `if`s
/// silently stopped being a total order (buri-lang/buri#40).
///
/// `lt` and `le` are here for the same reason they are in the row above: `<`
/// and friends stay IEEE-754, so a fix that reached too far would show up as
/// those two moving rather than as the five before them.
#[test]
fn row_09_float_equality_is_the_same_in_every_position() {
    rows_or_skip!();
    agree(
        "row 9 float eq positions",
        r#"
from "core/host" import { stdout };
from "core/io" import * as io;

fn zeroF(): Float { 0.0 }
fn notANumber(): Float { zeroF() / zeroF() }

fn asValue(a: Float, b: Float): Bool { a == b }
fn asCondition(a: Float, b: Float): Bool { if (a == b) { true } else { false } }
fn boundThenCondition(a: Float, b: Float): Bool {
  let equal = a == b;
  if (equal) { true } else { false }
}
fn asGuard(a: Float, b: Float): Bool {
  match (a) { _ if a == b => true, _ => false }
}
fn negated(a: Float, b: Float): Bool { if (!(a == b)) { false } else { true } }
fn ltCondition(a: Float, b: Float): Bool { if (a < b) { true } else { false } }
fn leCondition(a: Float, b: Float): Bool { if (a <= b) { true } else { false } }

export fn main(): Result<(), Str> {
  let n = notANumber();
  let v = asValue(n, n);
  let c = asCondition(n, n);
  let b = boundThenCondition(n, n);
  let g = asGuard(n, n);
  let x = negated(n, n);
  let lt = ltCondition(n, n);
  let le = leCondition(n, n);
  let ord = asCondition(1.5, 1.5);
  let neq = asCondition(1.5, 2.5);
  let _ = io.println(stdout, "${v} ${c} ${b} ${g} ${x} ${lt} ${le} ${ord} ${neq}").ignore();
  .Ok(())
}
"#,
        "true true true true true false false true false\n",
    );
}

/// Derived `Hash`: the *numbers*, not merely the verdicts.
///
/// A hash is the one derive whose output is a value a program can print, so
/// "agrees" means the same integer rather than the same partition.
/// `deriveHash` is emitted at every primitive and claims to match `$hash`
/// byte for byte; this pins that end to end, through a struct and an enum
/// rather than only at the primitives.
#[test]
fn row_09_derived_hash_values() {
    rows_or_skip!();
    agree(
        "row 9 hash",
        r#"
from "core/host" import { stdout };
from "core/io" import * as io;

export struct P { a: Int, b: Str }
export enum E { A, B(Int) }
derive Hash for P;
derive Hash for E;

export fn main(): Result<(), Str> {
  let p = P { a: 1, b: "m" };
  let h = p.hash();
  let i = E.A.hash();
  let j = E.B(7).hash();
  let k = (0 - 1).hash();
  let l = "".hash();
  let m = 'z'.hash();
  let n = false.hash();
  let _ = io.println(stdout, "${h} ${i} ${j} ${k} ${l} ${m} ${n}").ignore();
  .Ok(())
}
"#,
        "4152125875 3950255460 2709250641 4193493326 2166136261 4278997933 84696351\n",
    );
}

/// A `[T]` inside a derived `Show`, which used to be a named gap.
///
/// It was `row_09_derived_show_of_a_list_is_a_gap` beside an `#[ignore]`d
/// version of this test, and the ignore reason said to un-ignore this one and
/// delete that one together. `deriveArrayShow` landed — the element's generated
/// `show` called once per element into a scratch block of `Str`s, joined by
/// `buri_rt_show_list` — so that is what happened.
///
/// `Option` and `Result` inside one are covered by [`row_09_derived_show`]. A
/// bare `[T]`, `Option` or `Result` cannot be shown or interpolated at all —
/// the front end refuses it — so a derived `Show` over a field is the only way
/// to ask this question.
#[test]
fn row_09_derived_show_of_a_list() {
    rows_or_skip!();
    agree(
        "row 9 lists",
        SHOW_A_LIST,
        "Bag { xs: [1, 2, 3], ss: [\"a\", \"b\"], empty: [] }\n",
    );
}

/// `deriveArrayHash`, as a **named** gap rather than a panic.
///
/// `deriveArrayCompare` and `deriveArrayHash` are `deriveArrayEq`'s loop with a
/// different carried value. The first is emitted now
/// (`stencil/lists.rs::derive_array_compare`, buri-lang/buri#27) and
/// [`a_derived_ord_over_a_list`] is the agreement it bought; the second is
/// still the gap below. What this test is really
/// about is that a gap *reads* as one: an intrinsic with no body records an
/// error and binds nothing, and the debug backend of the day then unwrapped a
/// `None` on the next instruction rather than letting the recorded diagnostic
/// out. A `derive Hash` over a `[T]`
/// field crashed the toolchain until that bound a value.
///
/// This is not a §12 row and is not in the table: it is a claim about how a
/// missing intrinsic is *reported*, which every row already assumes.
#[test]
fn a_derived_hash_over_a_list_is_a_named_gap() {
    rows_or_skip!();
    gap("derived hash over a list", HASH_A_LIST, &["deriveArrayHash"]);
}

const HASH_A_LIST: &str = r#"
from "core/host" import { stdout };
from "core/io" import * as io;

export struct Bag { xs: [Int] }
derive Eq, Hash, Show for Bag;

export fn main(): Result<(), Str> {
  let a = Bag { xs: [1, 2] };
  let b = Bag { xs: [1, 2] };
  let _ = io.println(stdout, if (a.hash() == b.hash()) { "same" } else { "differ" }).ignore();
  .Ok(())
}
"#;

/// A `[T]` inside a derived `Ord`, which used to be a named gap of its own.
///
/// `derive Ord` on a type holding an array was a program the front end accepted
/// and the stencil backend refused by name — "cannot compile CallIntrinsic
/// deriveArrayCompare" — so a `Value` enum with a `Bytes([U8])` arm could not be
/// built for a native target at all (buri-lang/buri#27).
///
/// The order the row pins is the lexicographic one `$cmp`'s array arm gives:
/// the first `min(m, n)` elements decide it, and where they all agree the
/// lengths do. `nine` is the case that tells the two apart — `[9]` is one
/// element against `[1, 2]`'s two and still sorts *above* it — and `words`
/// carries the `Str` element, whose leaf is a runtime call rather than a
/// compare instruction, so the derived order and `Str.compare` are asserted to
/// be the same order.
#[test]
fn a_derived_ord_over_a_list() {
    rows_or_skip!();
    agree("derived ord over a list", ORD_A_LIST, "lt gt eq lt gt lt gt\n");
}

const ORD_A_LIST: &str = r#"
from "core/host" import { stdout };
from "core/io" import * as io;
from "core/order" import { Order };

export struct Bag { xs: [U8] }
derive Eq, Ord for Bag;

export struct Leaf { a: Int, b: Str }
derive Eq, Ord for Leaf;

export struct Deep { xs: [Leaf], ss: [Str] }
derive Eq, Ord for Deep;

fn name(o: Order): Str { match (o) { .Less => "lt", .Equal => "eq", .Greater => "gt" } }
fn bag(xs: [U8]): Bag { Bag { xs: xs } }
fn deep(xs: [Leaf], ss: [Str]): Deep { Deep { xs: xs, ss: ss } }
fn leaf(a: Int, b: Str): Leaf { Leaf { a: a, b: b } }

export fn main(): Result<(), Str> {
  let elem = name(bag([1]).compare(bag([2])));
  let nine = name(bag([9]).compare(bag([1, 2])));
  let same = name(bag([1, 2]).compare(bag([1, 2])));
  let prefix = name(bag([1]).compare(bag([1, 0])));
  let empty = name(bag([0]).compare(bag([])));
  let inner = name(deep([leaf(1, "x")], []).compare(deep([leaf(1, "y")], [])));
  let words = name(deep([], ["ab"]).compare(deep([], ["aa", "zz"])));
  let _ = io.println(stdout, "${elem} ${nine} ${same} ${prefix} ${empty} ${inner} ${words}").ignore();
  .Ok(())
}
"#;

/// A hand-written `impl Ord` on a field's type, and the derived `Ord` above it.
///
/// **The two backends agree, and the answer they agree on is the structural
/// one.** SPEC 5.12.3 says a `derive` "generates the trait's methods
/// structurally: struct fields in declaration order … recursing into field
/// types. It is a fold over one type definition — no search, no instances to
/// resolve", and the same section is where the language reasons that a
/// hand-written implementation "would be obeyed where the type is encoded on
/// its own and ignored where a type holding it is". `ToJson` and `FromJson` are
/// the two it settles by *rejecting* the `impl`; `Ord` is left half-obeyed, and
/// this row is where that shows.
///
/// So `direct` is the hand-written verdict and `derived` is the structural one,
/// and they differ: `Wrapper`'s own `compare` says the longer octets are
/// greater, while the fold under `Pair` walks straight past it into `[U8]`'s
/// lexicographic order and answers on the first element. buri-lang/buri#27's
/// second finding asks for `derived` to become `direct`; that is a change to
/// SPEC 5.12.3 and to both backends' walkers — `middle::derives` natively and
/// `$cmp` on JavaScript, which is handed no descriptor at all — rather than a
/// native-backend fix, and it is not what the array half of that issue was.
/// What this row is for until then is that the two backends cannot start
/// disagreeing about it quietly.
#[test]
fn a_derive_over_a_hand_written_impl_is_structural_on_both_backends() {
    rows_or_skip!();
    agree("derive over a hand-written impl", DERIVE_OVER_IMPL, "lt gt\n");
}

const DERIVE_OVER_IMPL: &str = r#"
from "core/host" import { stdout };
from "core/io" import * as io;
from "core/order" import { Order };

export struct Holder { octets: [U8] }
derive Eq, Ord for Holder;

export struct Wrapper(Holder);

impl Ord for Wrapper {
  fn compare(self, other: Wrapper): Order {
    if ((self.0).octets.len() < (other.0).octets.len()) { .Less } else { .Greater }
  }
}

export struct Pair { wrapped: Wrapper }
derive Ord for Pair;

fn name(o: Order): Str { match (o) { .Less => "lt", .Equal => "eq", .Greater => "gt" } }
fn wrap(octets: [U8]): Wrapper { Wrapper(Holder { octets: octets }) }
fn pair(octets: [U8]): Pair { Pair { wrapped: wrap(octets) } }

export fn main(): Result<(), Str> {
  // The hand-written ordering: fewer octets is `.Less`, whatever they hold.
  let direct = name(wrap([9]).compare(wrap([1, 2])));
  // The derived one over it: `[U8]`'s own order, which puts `[9]` above.
  let derived = name(pair([9]).compare(pair([1, 2])));
  let _ = io.println(stdout, "${direct} ${derived}").ignore();
  .Ok(())
}
"#;

const SHOW_A_LIST: &str = r#"
from "core/host" import { stdout, alloc };
from "core/io" import * as io;

export struct Bag { xs: [Int], ss: [Str], empty: [Int] }
derive Show for Bag;

export fn main(): Result<(), Str> {
  let b = Bag { xs: [1, 2, 3], ss: ["a", "b"], empty: [] };
  let _ = io.println(stdout, b.show(alloc)).ignore();
  .Ok(())
}
"#;

// -------------------------------------------------------------------
// Row 10 — derived `ToJson`
// -------------------------------------------------------------------

// `row_10_derived_tojson_is_a_gap` stood here and is gone, with the
// `#[ignore]` it was paired to: it pinned `derivePrimJson` having no native
// body at any primitive, and both native backends have one now
// (`stencil/emit.rs::json_prim`, `llvm/emit.rs::json_prim`). A gap test that
// outlives its gap fails — that is what it is for — so the pair was always
// going to be deleted and un-ignored in one commit, and this is it.
//
// The *other* half of the row's old sentence still stands and is why the
// program below walks the tree by hand: `json.stringify` is `list.mapCtx` and
// `str.chars` over closures, which is the surface `native/conformance.rs`
// names. A `match` over `.Object`/`.Array` needs no closure, which is what
// makes the row pinnable without the whole of `core/json`.

/// The agreement test `derivePrimJson` landing made runnable. It is a wire
/// format, so the bar is bytes.
///
/// `"a":3.0` and not `"a":3`, and the reason is the renderer rather than the
/// encoding: `a` is an `Int`, `derive ToJson` puts it in `.Num` — JSON has one
/// number type and it is a double, which is what `json.buri`'s header and
/// `$json_of`'s `Number(v)` both say — and the program below renders a `.Num`
/// with `"${x}"`, which is `Show` for a `Float` and spells a whole number with
/// its point. `json.stringify` is what would write `3`, and it is closures
/// (`native/conformance.rs`), so the row walks the tree by hand and gets
/// `Show`'s spelling. What the row is pinning is that **both** pipelines build
/// the same tree and print the same bytes off it.
#[test]
fn row_10_derived_tojson() {
    rows_or_skip!();
    agree(
        "row 10",
        TOJSON,
        "{\"a\":3.0,\"b\":\"hi\",\"c\":false,\"d\":1.5,\"e\":{\"flag\":true,\"note\":\"n\"}}\n",
    );
}

/// A `Json` rendered without `json.stringify`, because that is closures.
const TOJSON: &str = r#"
from "core/host" import { stdout, alloc };
from "core/io" import * as io;
from "core/json" import { Json, ToJson };
from "core/str" import * as str;

export struct Inner { flag: Bool, note: Str }
export struct P { a: Int, b: Str, c: Bool, d: Float, e: Inner }
derive ToJson for Inner;
derive ToJson for P;

fn render(j: Json): Str {
  match (j) {
    .Null => "null",
    .Bool(b) => str.format(alloc, "${b}"),
    .Num(x) => str.format(alloc, "${x}"),
    .Str(s) => str.format(alloc, "\"${s}\""),
    .Array(items) => str.format(alloc, "[${renderList(items)}]"),
    .Object(entries) => str.format(alloc, "{${renderEntries(entries)}}"),
  }
}

fn renderList(items: [Json]): Str {
  match (items) {
    [] => "",
    [h] => render(h),
    [h, ..t] => str.format(alloc, "${render(h)},${renderList(t)}"),
  }
}

fn renderEntries(entries: [(Str, Json)]): Str {
  match (entries) {
    [] => "",
    [h] => entryText(h),
    [h, ..t] => str.format(alloc, "${entryText(h)},${renderEntries(t)}"),
  }
}

fn entryText(e: (Str, Json)): Str {
  let (k, v) = e;
  str.format(alloc, "\"${k}\":${render(v)}")
}

export fn main(): Result<(), Str> {
  let p = P { a: 3, b: "hi", c: false, d: 1.5, e: Inner { flag: true, note: "n" } };
  let _ = io.println(stdout, render(p.toJson(alloc))).ignore();
  .Ok(())
}
"#;

/// Row 10 at every primitive the leaf has an arm for, which is what
/// `row_09_integer_show_at_every_width` is to row 9.
///
/// The row above pins four of them — `Bool`, `Str`, `Int`, `Float` — and four
/// is not the claim. `derivePrimJson` is one function per backend with a
/// three-way answer in it, and the arms that can differ are the ones the four
/// do not reach: a `Char`, whose JSON is a **string** and whose native answer
/// is a runtime call rather than a copy; and the narrow integers, which sit in
/// a frame slot zero-extended, so a signed one has to be widened by its own
/// signedness before it becomes a double. `-3` at `I8` arriving as `253.0` is
/// the bug this test is shaped to catch, and it is the same bug
/// `show_prim`'s `sext` comment describes at the other leaf.
///
/// An astral `Char` is here because `buri_rt_char_to_str` is the one arm that
/// encodes UTF-8 rather than moving bytes that were already encoded.
#[test]
fn row_10_derived_tojson_at_every_primitive() {
    rows_or_skip!();
    agree(
        "row 10 widths",
        TOJSON_WIDTHS,
        "{\"ch\":\"é\",\"em\":\"😀\",\"u8v\":255.0,\"i8v\":-3.0,\"u16v\":65535.0,\
         \"i16v\":-300.0,\"u32v\":4294967295.0,\"i32v\":-70000.0,\"u64v\":7.0,\
         \"f32v\":1.5}\n",
    );
}

const TOJSON_WIDTHS: &str = r#"
from "core/host" import { stdout, alloc };
from "core/io" import * as io;
from "core/json" import { Json, ToJson };
from "core/str" import * as str;

export struct W {
  ch: Char, em: Char,
  u8v: U8, i8v: I8, u16v: U16, i16v: I16, u32v: U32, i32v: I32, u64v: U64,
  f32v: F32,
}
derive ToJson for W;

fn render(j: Json): Str {
  match (j) {
    .Null => "null",
    .Bool(b) => str.format(alloc, "${b}"),
    .Num(x) => str.format(alloc, "${x}"),
    .Str(s) => str.format(alloc, "\"${s}\""),
    .Array(items) => str.format(alloc, "[${renderList(items)}]"),
    .Object(entries) => str.format(alloc, "{${renderEntries(entries)}}"),
  }
}

fn renderList(items: [Json]): Str {
  match (items) {
    [] => "",
    [h] => render(h),
    [h, ..t] => str.format(alloc, "${render(h)},${renderList(t)}"),
  }
}

fn renderEntries(entries: [(Str, Json)]): Str {
  match (entries) {
    [] => "",
    [h] => entryText(h),
    [h, ..t] => str.format(alloc, "${entryText(h)},${renderEntries(t)}"),
  }
}

fn entryText(e: (Str, Json)): Str {
  let (k, v) = e;
  str.format(alloc, "\"${k}\":${render(v)}")
}

export fn main(): Result<(), Str> {
  let w = W {
    ch: 'é', em: '😀',
    u8v: 255, i8v: -3, u16v: 65535, i16v: -300,
    u32v: 4294967295, i32v: -70000, u64v: 7,
    f32v: 1.5,
  };
  let _ = io.println(stdout, render(w.toJson(alloc))).ignore();
  .Ok(())
}
"#;

// -------------------------------------------------------------------
// Rows 11 and 14 — aborts
// -------------------------------------------------------------------

/// Division and remainder by zero: the same message, the same status, and
/// the same output before it.
///
/// The divisor is `"".len()` rather than a literal zero because a division
/// by a literal is decided at compile time and there is nothing left to
/// ask; `cli/tests/crash/` reaches for `env.args(ctx).len()` instead, which
/// is `host.HostEnv.args` and has no native body yet.
#[test]
fn row_11_division_by_zero() {
    rows_or_skip!();
    abort_agrees(
        "row 11",
        r#"
from "core/host" import { stdout };
from "core/io" import * as io;

fn ratio(a: Int, b: Int): Int { a / b }

export fn main(): Result<(), Str> {
  let zero = "".len();
  let _ = io.println(stdout, "before").ignore();
  let _ = io.println(stdout, "${ratio(10, zero)}").ignore();
  .Ok(())
}
"#,
        "before\n",
        "division by zero",
    );
    abort_agrees(
        "row 11 remainder",
        r#"
from "core/host" import { stdout };
from "core/io" import * as io;

fn rest(a: Int, b: Int): Int { a % b }

export fn main(): Result<(), Str> {
  let zero = "".len();
  let _ = io.println(stdout, "${rest(10, zero)}").ignore();
  .Ok(())
}
"#,
        "",
        "division by zero",
    );
}

/// A shift at or beyond the operand's width, which is `cli/tests/crash/`'s
/// other pinned message.
#[test]
fn row_14_shift_out_of_range() {
    rows_or_skip!();
    abort_agrees(
        "row 14 shift",
        r#"
from "core/bits" import * as bits;
from "core/host" import { stdout };
from "core/io" import * as io;

fn push(x: U8, n: Int): U8 { bits.shlU8(x, n) }

export fn main(): Result<(), Str> {
  let width = 8 + "".len();
  let _ = io.println(stdout, "${push(1, width)}").ignore();
  .Ok(())
}
"#,
        "",
        "shift out of range",
    );
}

/// The `.Err` path, which is not an abort: `main` returning `.Err(msg)`
/// writes `msg` to standard error and exits 1, and nothing was thrown — so
/// this is the one failure whose *whole* standard error agrees, not only
/// its first line.
#[test]
fn row_14_an_error_return() {
    rows_or_skip!();
    let (js, natives) = both(
        "row 14 err",
        r#"
from "core/host" import { stdout };
from "core/io" import * as io;

export fn main(): Result<(), Str> {
  let _ = io.println(stdout, "before").ignore();
  .Err("it did not work")
}
"#,
    );
    assert_eq!(js.stdout, "before\n");
    assert_eq!(js.stderr, "it did not work\n");
    assert_eq!(js.status, 1);
    for (name, ran) in &natives {
        assert_eq!(ran.stdout, js.stdout, "row 14: `{name}` printed something else");
        assert_eq!(ran.stderr, js.stderr, "row 14: `{name}`'s error text differs");
        assert_eq!(ran.status, js.status, "row 14: `{name}` exited {}", ran.status);
    }
}

// -------------------------------------------------------------------
// Row 12 — `Alloc` accounting
// -------------------------------------------------------------------

// `row_12_alloc_accounting_is_a_gap` stood here and is gone with the
// `#[ignore]` beside it, for the reason row 10's did: it pinned
// `host.HostAlloc.allocate` having no native body, and the debug backend has
// one now — `runtime_table.rs`'s row reaches
// `buri_rt_host_alloc_allocate`, which is the same archive body the release
// backend has always called.

/// MEMORY.md §7's model, on both backends, at the one row that charges its
/// own argument.
///
/// `HostAlloc` is zero-sized and unbounded (§7.2), so `allocate(64)` is
/// `Region(64)` and nothing accumulates *in the allocator* — the accounting a
/// program can read is `core/alloc`'s counters, which are a different four
/// keys and a different question. So the agreement this pins is the one §7.1
/// asks for: the charge is a function of the argument, defined rather than
/// measured, and therefore the same number on both pipelines.
#[test]
fn row_12_alloc_accounting() {
    rows_or_skip!();
    agree("row 12", ALLOCATE, "64\n");
}

const ALLOCATE: &str = r#"
from "core/alloc" import * as alloc;
from "core/effect" import { Alloc, Region };
from "core/host" import { alloc as platform, stdout };
from "core/io" import * as io;

export fn main(): Result<(), Str> {
  let r = alloc.allocate(platform, 64);
  let n = r.0;
  let _ = io.println(stdout, "${n}").ignore();
  .Ok(())
}
"#;

// -------------------------------------------------------------------
// Row 13 — tail calls
// -------------------------------------------------------------------

/// A self-recursive loop and a mutually recursive pair, each a million
/// deep: constant stack on both, or the process dies rather than answering.
///
/// This is the row that found the second miscompile. `tail_calls.rs` merges
/// a mutually recursive group into one function and leaves each member as a
/// forwarder, and it labelled the forwarders with `Func::ret` — which is
/// `()` for every function with a body. `lower::returns` reads a body's
/// type instead for exactly that reason, and its `Loop` arm saved the
/// merged function while a forwarder's `Continue` had no such arm. So
/// `even(1000001)` was lowered as returning nothing: natively it printed
/// the empty string rather than `false`, and one step on — the same value
/// used as a condition — it panicked inside the debug backend's frontend. The
/// JavaScript backend is untyped and never noticed.
#[test]
fn row_13_tail_calls_run_in_constant_stack() {
    rows_or_skip!();
    agree(
        "row 13",
        r#"
from "core/host" import { stdout };
from "core/io" import * as io;

fn count(n: Int, acc: Int): Int { if (n == 0) { acc } else { count(n - 1, acc + n) } }
fn even(n: Int): Bool { if (n == 0) { true } else { odd(n - 1) } }
fn odd(n: Int): Bool { if (n == 0) { false } else { even(n - 1) } }

export fn main(): Result<(), Str> {
  let a = count(1000000, 0);
  let b = even(1000001);
  let c = even(1000000);
  // The forwarder's result used as a condition rather than shown, which is
  // where a signature returning nothing stopped being a wrong answer and
  // started being a crash.
  let d = if (even(4)) { "yes" } else { "no" };
  let _ = io.println(stdout, "${a} ${b} ${c} ${d}").ignore();
  .Ok(())
}
"#,
        "500000500000 false true yes\n",
    );
}

// -------------------------------------------------------------------
// Not a row: a miscompile the rows did not reach
// -------------------------------------------------------------------

/// An aggregate holding two counted values, read back through its own
/// projections — the shape `middle::rc` dropped the base of one
/// instruction too early.
///
/// Not a §12 row: nothing in the table is about *when* a count goes down,
/// because JavaScript is garbage collected and the question does not arise
/// there. That is exactly why it belongs here anyway. The reference answer
/// is the one backend that cannot get a reference count wrong, so a native
/// backend that frees a pair while a `str.concat` chain is still reading
/// words out of it does not merely print something odd — it prints
/// something JavaScript does not, and this is the comparison that says so.
///
/// Every string here is a heap one. A literal's block is immortal, so the
/// same program over literals agreed all along, which is how the shape
/// survived a suite whose functions returned one value each.
#[test]
fn an_aggregate_of_two_counted_values_agrees_through_its_projections() {
    rows_or_skip!();
    agree(
        "aggregate projections",
        r#"
from "core/host" import { stdout, alloc };
from "core/io" import * as io;
from "core/str" import * as str;

struct Pair { a: Str, b: Str }

fn dupTuple(s: Str): (Str, Str) { (s, s) }
fn dupStruct(s: Str): Pair { Pair { a: s, b: s } }

fn spin(n: Int, p: (Str, Str)): Str {
  if (n == 0) { p.0 } else { spin(n - 1, (p.1, p.0)) }
}

export fn main(): Result<(), Str> {
  let heap = "ab".repeat(alloc, 3);
  let other = "cd".repeat(alloc, 2);
  let dup = dupTuple(heap);
  let rec = dupStruct(heap);
  let two = (heap, other);
  let xs = ["ef".repeat(alloc, 2)];
  let got = match (xs[0]) { .Some(v) => v, .None => "?" };
  let _ = io.println(stdout, "${dup.0}|${dup.1}").ignore();
  let _ = io.println(stdout, "${rec.a}|${rec.b}").ignore();
  let _ = io.println(stdout, "${two.0}|${two.1}").ignore();
  let _ = io.println(stdout, "${got}").ignore();
  let _ = io.println(stdout, "${spin(5, ("kl".repeat(alloc, 2), "mn".repeat(alloc, 2)))}").ignore();
  .Ok(())
}
"#,
        "ababab|ababab\nababab|ababab\nababab|cdcd\nefef\nmnmn\n",
    );
}

/// A `let` gives back the positions its pattern **skips**, on both natives.
///
/// `middle::rc` releases a value through a name — a local, or the node that
/// produced a temporary — and a destructuring `let` used to leave the
/// positions its pattern skipped with neither. `let (q, _) = nextToken(ctx, p)`
/// took the pair the call answered, handed element 0 to `q`, and element 1
/// went nowhere. That is the thirteen blocks `proto_schema/refusals.buri`
/// leaked, all of them one and two-byte tokens the `.proto` reader stepped
/// over, and `rc::name_discards` is the fix.
///
/// Not a §12 row, for [`an_aggregate_of_two_counted_values_agrees_through_its_projections`]'s
/// reason: nothing in the table is about when a count goes down, because
/// JavaScript is garbage collected and the question does not arise there. It
/// belongs here anyway, and here rather than only in the conformance corpus,
/// because the corpus is driven natively through the **stencil** backend and
/// the rule is one `middle::rc` states once for every backend. This is the
/// row that says the LLVM one obeys it: [`agree`] runs each native under the
/// heap check and reports a leak as a leak.
///
/// Every skipped position holds a heap string. A literal's block is immortal
/// (VALUE-MODEL.md §5.2), so the same program over literals leaked nothing
/// before the fix and would prove nothing after it. The last two lines are
/// the other direction: `held` is read again after being destructured, so a
/// second release of its element would be a use-after-free rather than a leak.
#[test]
fn a_let_gives_back_the_positions_its_pattern_skips() {
    rows_or_skip!();
    agree(
        "skipped let positions",
        r#"
from "core/host" import { stdout, alloc };
from "core/io" import * as io;

struct Pair { kept: Int, dropped: Str }

fn step(at: Int, word: Str): (Int, Str) { (at + 1, word) }

export fn main(): Result<(), Str> {
  let (at, _) = step(0, "ab".repeat(alloc, 2));
  let Pair { kept, dropped: _ } = Pair { kept: 2, dropped: "cd".repeat(alloc, 2) };
  let (n, (_, m)) = (3, ("ef".repeat(alloc, 2), 4));
  let held = step(5, "gh".repeat(alloc, 2));
  let (six, _) = held;
  let _ = io.println(stdout, "${at}|${kept}|${n}${m}|${six}|${held.1}").ignore();
  .Ok(())
}
"#,
        "1|2|34|6|ghgh\n",
    );
}

/// A struct holding `NaN` compared with **itself** — the case that used to
/// divide the backends, now the case that shows they no longer are divided.
///
/// This test was written with `diverge`, pinning `true` on JavaScript and
/// `false` natively, and its own doc said it "fails the day either side
/// moves — including the day the JavaScript side is corrected". The day came,
/// and the correction went the other way: SPEC 7.2 now rules `NaN == NaN`, so
/// the native side moved to JavaScript's answer rather than JavaScript to the
/// native one. Flipping `diverge` to `agree` is that mechanism firing exactly
/// as it was built to, and the expected text is the single answer both sides
/// now print.
///
/// Not a §12 row, and the absence is still the claim: there is nothing left
/// to list, because `==` at a float is one rule with one answer everywhere.
/// What made the old divergence possible is unchanged and worth keeping
/// written down — derived equality has **two** implementations,
/// `middle/derives.rs` natively and `js/generate.rs`'s `eq_decl` on
/// JavaScript, so §12 row 9's "because they are the same generator" is false
/// and this test is how the two are actually compared.
///
/// The referential fast path in `eq_decl` and in `runtime.js`'s `$eq` stays,
/// and is now sound rather than merely convenient: an equivalence relation is
/// reflexive, so two references to one value are equal without looking
/// inside. SPEC 7.2's rejection of referential equality was a rejection of it
/// as the *definition*; as a shortcut to an answer the walk would reach
/// anyway it decides nothing, which is why the native backends need no
/// identity notion to agree here.
#[test]
fn a_struct_holding_nan_compared_with_itself_agrees() {
    rows_or_skip!();
    agree(
        "nan self-identity",
        r#"
from "core/host" import { stdout };
from "core/io" import * as io;

export struct F { x: Float }
derive Eq for F;

fn mk(x: Float): F { F { x: x } }
fn zeroF(): Float { 0.0 }
fn notANumber(): Float { zeroF() / zeroF() }

export fn main(): Result<(), Str> {
  let f = mk(notANumber());
  let _ = io.println(stdout, "${f == f}").ignore();
  .Ok(())
}
"#,
        "true\n",
    );
}

// -------------------------------------------------------------------
// Row 16 — the NaN payload through `core/bytes`
// -------------------------------------------------------------------

/// A NaN payload does not survive `f64FromBytes`, on either backend.
///
/// It used to survive natively and not on JavaScript, where a `Float` is a
/// `number` and moving a NaN through one canonicalizes it — so the same
/// program computed different bytes on different backends, on a round trip
/// the module documents. SPEC §6.2 had already ruled that every NaN equals
/// every other "regardless of sign or payload", and `f64FromBytes` is the
/// only way to construct one, so native was the side that moved:
/// `cli/runtime/bytes.rs` canonicalizes on ingress.
///
/// The last line is the other half of the claim. Signed zero was never
/// affected and is pinned here so that a future canonicalization cannot
/// quietly widen: `-0.0` still round-trips to its own eight bytes.
#[test]
fn row_16_nan_payloads_canonicalize_on_every_backend() {
    rows_or_skip!();
    agree(
        "row 16",
        r#"
from "core/bytes" import * as bytes;
from "core/host" import { stdout, alloc };
from "core/io" import * as io;
from "core/str" import * as str;

fn ends(b: [U8]): Str {
  str.format(alloc, "${b[0].withDefault(0)} ${b[6].withDefault(0)} ${b[7].withDefault(0)}")
}

export fn main(): Result<(), Str> {
  let one = bytes.f64FromBytes([1, 0, 0, 0, 0, 0, 248, 127], 0).withDefault(0.0);
  let two = bytes.f64FromBytes([2, 0, 0, 0, 0, 0, 248, 127], 0).withDefault(0.0);
  let signalling = bytes.f64FromBytes([1, 0, 0, 0, 0, 0, 240, 255], 0).withDefault(0.0);
  let negativeZero = bytes.f64FromBytes([0, 0, 0, 0, 0, 0, 0, 128], 0).withDefault(1.0);
  let _ = io.println(stdout, ends(bytes.f64ToBytes(alloc, one))).ignore();
  let _ = io.println(stdout, ends(bytes.f64ToBytes(alloc, two))).ignore();
  let _ = io.println(stdout, ends(bytes.f64ToBytes(alloc, signalling))).ignore();
  let _ = io.println(stdout, "${one == two} ${one == signalling}").ignore();
  let _ = io.println(stdout, ends(bytes.f64ToBytes(alloc, negativeZero))).ignore();
  .Ok(())
}
"#,
        "0 248 127\n0 248 127\n0 248 127\ntrue true\n0 0 128\n",
    );
}

/// The **closure trampoline**: `list.mapCtxStep` answers what `list.mapCtx`
/// answers, on both native backends, and both answer what JavaScript does.
///
/// Not a §12 row, and the shape of the comparison is `middle/fuse.rs`'s. That
/// pass runs on the native branch only, and says why: "a differential test
/// whose two sides share the transformation under test proves nothing about
/// it", so JavaScript is left as the reference implementation. The same
/// discipline is what makes this test worth anything. `$list_mapCtxStep` in
/// `js/runtime.js` is the ordinary `mapCtx` loop — the *unfused* reference —
/// while natively the step is called by `cli/runtime/list.rs` through a
/// generated C-ABI entry thunk. The two sides share the program and nothing
/// else, which is the only way to find out whether the boundary is right.
///
/// Every element type here is one whose handling differs at the boundary:
///
///  * `Int -> Int` — the plain case, and the strides are equal.
///  * `Int -> Str` — the result is **counted** and wider than the source, so
///    the two strides differ and every element the step answers is a block the
///    result list now owns. A trampoline that lost that count prints garbage
///    or aborts; one that took an extra leaks, which `buri_rt_heap_stats`
///    catches in CI rather than here.
///  * `Str -> Str` — the *source* is counted too, so the retain the entry
///    thunk takes before entering Buri code is the thing under test. Without
///    it the step frees a block the list still holds.
///  * `Int -> (Int, Int)` — an aggregate result written through the
///    out-pointer at its own stride.
///  * the empty list — no element, no entry, and a `[B]` that allocates
///    nothing.
///
/// A `mapCtxStep` inside a `mapCtx` is there because the entry thunk works in
/// the frame the *call site* set aside, and a call site that is itself inside
/// a running step is where two of them would collide if that frame were
/// anything global.
#[test]
fn the_closure_trampoline_answers_what_the_open_coded_loop_does() {
    rows_or_skip!();
    agree(
        "closure trampoline",
        r#"
from "core/host" import { stdout, alloc };
from "core/io" import * as io;
from "core/list" import * as list;
from "core/str" import * as str;

fn show(xs: [Str]): Str { xs.join(alloc, ",") }

export fn main(): Result<(), Str> {
  let ns = [1, 2, 3, 4];
  let doubledStep = ns.mapCtxStep(alloc, fn(c, n) => n * 2);
  let doubledLoop = ns.mapCtx(alloc, fn(c, n) => n * 2);
  let _ = io.println(stdout, "${doubledStep.len()} ${doubledLoop.len()}").ignore();
  let _ = io.println(stdout, "${show(doubledStep.mapCtx(alloc, fn(c, n) => str.fromInt(c, n)))}").ignore();
  let _ = io.println(stdout, "${show(doubledLoop.mapCtx(alloc, fn(c, n) => str.fromInt(c, n)))}").ignore();

  // A counted result, at a stride the source does not have.
  let named = ns.mapCtxStep(alloc, fn(c, n) => "n".repeat(c, n));
  let _ = io.println(stdout, show(named)).ignore();

  // A counted source: the retain the entry thunk takes is what keeps `named`
  // alive while its elements are read.
  let louder = named.mapCtxStep(alloc, fn(c, s) => str.format(c, "<${s}>"));
  let _ = io.println(stdout, show(louder)).ignore();
  let _ = io.println(stdout, show(named)).ignore();

  // An aggregate result, through the out-pointer.
  let pairs = ns.mapCtxStep(alloc, fn(c, n) => (n, n * n));
  let _ = io.println(stdout, show(pairs.mapCtx(alloc, fn(c, p) => str.format(c, "${p.0}^${p.1}")))).ignore();

  // Nested: a step that is itself a call site.
  let nested = ns.mapCtx(alloc, fn(c, n) => [n, n].mapCtxStep(c, fn(d, m) => m + 1).len());
  let _ = io.println(stdout, "${nested.len()} ${nested[0].withDefault(0)}").ignore();

  let empty: [Int] = [];
  let _ = io.println(stdout, "${empty.mapCtxStep(alloc, fn(c, n) => n + 1).len()}").ignore();
  .Ok(())
}
"#,
        concat!(
            "4 4\n",
            "2,4,6,8\n",
            "2,4,6,8\n",
            "n,nn,nnn,nnnn\n",
            "<n>,<nn>,<nnn>,<nnnn>\n",
            "n,nn,nnn,nnnn\n",
            "1^1,2^4,3^9,4^16\n",
            "4 2\n",
            "0\n",
        ),
    );
}

/// `Tasks.parallel` answers the same list on all three backends, and it is the
/// same list in the same order.
///
/// This is the assertion half of `core/tasks`, and it is here rather than in
/// the conformance corpus for a reason that is a fact about the language: a
/// `test` block lives in a test source, a test source is not the module that
/// exports `main`, and `core/host` is importable only from that module. There
/// is no `Tasks` double yet either — `TestTasks` is a later slice — so the only
/// honest way to run `parallel` at all is a real program with a real granted
/// host, which is exactly what this file compiles.
///
/// **The two sides share the program and nothing else**, which is what makes
/// the comparison worth something. `$host_HostTasks_parallel` starts every task
/// before it awaits any of them and collects `Promise.all`'s array;
/// `buri_rt_host_tasks_parallel` walks the block in index order calling a
/// generated C-ABI entry thunk. Two implementations with nothing in common,
/// asked for one answer.
///
/// What each case is for:
///
///  * **the index** — the second closure parameter, which is neither in the
///    state record nor in the element and reaches the step in its own register.
///    Asserted as an *answer* rather than as a count, so a step told the wrong
///    index prints the wrong list rather than passing.
///  * **input order** — the answer is `[A]`'s order and not completion order.
///    On JavaScript the tasks are genuinely in flight together, so this is a
///    real promise being kept rather than an artefact of a sequential loop.
///  * **a counted result** at a wider stride — every element the step answers
///    is a block the new list owns.
///  * **a counted source** — the retain the entry thunk takes before entering
///    Buri code, read back afterwards, which a missing retain turns into a
///    use-after-free.
///  * **the empty list** — no task, no entry, and a `[B]` that allocates
///    nothing.
///  * **nested** — `parallel` inside `parallel`, because the entry thunk works
///    in the frame the call site reserved and a call site inside a running step
///    is where two of them would meet.
#[test]
fn the_task_scheduler_answers_in_input_order_on_every_backend() {
    rows_or_skip!();
    agree(
        "tasks.parallel",
        r#"
from "core/effect" import { Alloc, Stdout, Tasks };
from "core/host" import * as host;
from "core/io" import * as io;
from "core/list" import * as list;
from "core/str" import * as str;
from "core/tasks" import * as tasks;

fn show<C: Alloc>(ctx: C, xs: [Str]): Str { xs.join(ctx, ",") }

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout, Tasks: host.tasks };

  // The index is the item's own, and the answer is in the items' order.
  let ns = [10, 20, 30, 40];
  let indexed = tasks.parallel(ctx, ns, fn(c, i, n) => n + i);
  let _ = io.println(ctx, show(ctx, indexed.mapCtx(ctx, fn(c, n) => str.fromInt(c, n)))).ignore();

  // A counted answer, at a stride the source does not have.
  let named = tasks.parallel(ctx, ns, fn(c, i, n) => "n".repeat(c, i + 1));
  let _ = io.println(ctx, show(ctx, named)).ignore();

  // A counted source: the step is handed a count of its own, so the source is
  // still readable after the walk.
  let louder = tasks.parallel(ctx, named, fn(c, i, s) => str.format(c, "<${s}>"));
  let _ = io.println(ctx, show(ctx, louder)).ignore();
  let _ = io.println(ctx, show(ctx, named)).ignore();

  // An aggregate answer, through the out-pointer.
  let pairs = tasks.parallel(ctx, ns, fn(c, i, n) => (i, n));
  let _ = io.println(ctx, show(ctx, pairs.mapCtx(ctx, fn(c, p) => str.format(c, "${p.0}^${p.1}")))).ignore();

  // Nested: a task that itself runs tasks.
  let nested = tasks.parallel(ctx, ns, fn(c, i, n) =>
    tasks.parallel(c, [n, n, n], fn(d, j, m) => m + j).fold(fn(a, m) => a + m, 0));
  let _ = io.println(ctx, show(ctx, nested.mapCtx(ctx, fn(c, n) => str.fromInt(c, n)))).ignore();

  let empty: [Int] = [];
  let _ = io.println(ctx, "${tasks.parallel(ctx, empty, fn(c, i, n) => n + 1).len()}").ignore();
  .Ok(())
}
"#,
        concat!(
            "10,21,32,43\n",
            "n,nn,nnn,nnnn\n",
            "<n>,<nn>,<nnn>,<nnnn>\n",
            "n,nn,nnn,nnnn\n",
            "0^10,1^20,2^30,3^40\n",
            "33,63,93,123\n",
            "0\n",
        ),
    );
}

/// **`Tasks.parallel` over a list every step shares**, on all three backends.
///
/// G3's acceptance case, and the first program in this file whose answer
/// depends on the reference counts being right *under concurrency* rather than
/// merely right. The two tests above hand each step its own element; this one
/// hands every step the **same blocks**, three ways at once:
///
///  * **a captured list.** `shared` is one `[Str]` the closure's environment
///    owns, and every step reads the whole of it. Sixteen carriers therefore
///    `incref` and `decref` one list block and its four element blocks at the
///    same time. A count that lost an update frees a block another carrier is
///    still reading, and what that prints is not this file's business — it is
///    a crash, a repeated line or a line of rubbish, and any of the three
///    fails the comparison.
///  * **elements that alias.** `twice` is built out of one `Str` value placed
///    in four slots, so four steps that each look at "their own" element are
///    four carriers counting **one block**. This is the case a per-element
///    argument about ownership gets wrong.
///  * **a value read back afterwards.** `shared` is printed once more in
///    `main`'s own frame, so a step that over-released it shows up here as a
///    wrong answer rather than as a leak nobody looks at.
///
/// Sixteen steps rather than four, because the window is what makes carriers
/// overlap and four of them on a fast machine can finish one at a time by
/// accident. The number is a *likelihood* knob, and the assertion does not
/// depend on it: the answer is the same list either way.
///
/// **What makes this test able to fail is the marking latch.** Built against a
/// tree whose `buri_rt_values_may_cross_tasks` is a no-op — the `[G3-RED]`
/// experiment in `reports/wave8-g3.md` — the LLVM row of this case fails, and
/// the report records how. Without the fan-out it would be a sequential walk
/// and would pass for a reason that has nothing to do with counting.
#[test]
fn a_shared_list_is_counted_correctly_by_every_task() {
    rows_or_skip!();
    agree(
        "tasks.parallel shared",
        r#"
from "core/effect" import { Alloc, Clock, Stdout, Tasks };
from "core/host" import * as host;
from "core/io" import * as io;
from "core/list" import * as list;
from "core/str" import * as str;
from "core/tasks" import * as tasks;
from "core/time" import * as time;

export fn main(): Result<(), Str> {
  let ctx = context {
    Alloc: host.alloc, Stdout: host.stdout, Tasks: host.tasks, Clock: host.clock,
  };

  // One list, read whole by every step. The closure captures it, so the
  // environment owns the only reference and each carrier borrows it.
  let shared = ["al", "be", "ga", "de"].mapCtx(ctx, fn(c, s) => str.format(c, "<${s}>"));
  let ns = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15];
  let spin = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15];
  // The sleep is what makes the steps *overlap*, and the inner walk is what
  // gives them something to overlap on. Sixteen carriers are dispatched in
  // microseconds and then wait together, so they reach the shared list at the
  // same instant and each of them counts it sixteen times over. Without the
  // sleep a step finishes before the next is dispatched and gets handed the
  // same carrier back, which is a sequential walk with extra steps.
  let seen = tasks.parallel(ctx, ns, fn(c, i, n) => {
    let _ = time.sleepMs(c, 20);
    let each = spin.mapCtx(c, fn(d, k) => shared.join(d, ""));
    str.format(c, "${n}:${each.len()}:${each.join(c, "|").len()}")
  });
  let _ = io.println(ctx, seen.join(ctx, " ")).ignore();

  // Read back in the caller's own frame: the list survived the walk.
  let _ = io.println(ctx, shared.join(ctx, ",")).ignore();

  // One block in four slots: four steps counting the same allocation.
  let one = "z".repeat(ctx, 3);
  let twice = [one, one, one, one];
  let sized = tasks.parallel(ctx, twice, fn(c, i, s) => str.format(c, "${i}${s}"));
  let _ = io.println(ctx, sized.join(ctx, "|")).ignore();
  let _ = io.println(ctx, twice.join(ctx, "+")).ignore();
  .Ok(())
}
"#,
        concat!(
            "0:16:271 1:16:271 2:16:271 3:16:271 4:16:271 5:16:271 ",
            "6:16:271 7:16:271 8:16:271 9:16:271 10:16:271 11:16:271 ",
            "12:16:271 13:16:271 14:16:271 15:16:271\n",
            "<al>,<be>,<ga>,<de>\n",
            "0zzz|1zzz|2zzz|3zzz\n",
            "zzz+zzz+zzz+zzz\n",
        ),
    );
}

/// **The data-race fixture**: a value every step *appends to*, which is the
/// in-place write licence rather than the count.
///
/// The count is the half `a_shared_list_is_counted_correctly_by_every_task`
/// stresses. This is the other half, and it is the one that fails
/// **deterministically** without the mark rather than probabilistically:
///
///  * `seed` is a heap `Str` the closure's environment owns, with spare
///    capacity — `buri_rt_grown_capacity`'s floor is 64 bytes and this one is
///    four.
///  * Each step evaluates `seed.concat(c, ...)`. On an *unmarked* block that
///    reads `rc == 1`, takes MEMORY.md §5.3's in-place path, and writes the
///    suffix into the shared block's spare capacity — so all sixteen steps
///    write **the same bytes at the same offset**, and every step's answer is
///    a view over whichever suffix landed last. The failure is not a timing
///    accident: it is sixteen answers that are all the same string, where
///    sixteen different ones were asked for.
///  * On a **marked** block `buri_rt_unique_cap` answers `None`, the concat
///    allocates, and each step gets its own bytes.
///
/// So this is the case that says what the mark *buys*, in a program, in one
/// line of output. `[G3-RED]` in `reports/wave8-g3.md` is the same fixture on
/// a tree with the latch neutered.
#[test]
fn a_shared_buffer_is_never_appended_to_in_place_by_two_tasks() {
    rows_or_skip!();
    agree(
        "tasks.parallel in-place",
        r#"
from "core/effect" import { Alloc, Clock, Stdout, Tasks };
from "core/host" import * as host;
from "core/io" import * as io;
from "core/list" import * as list;
from "core/str" import * as str;
from "core/tasks" import * as tasks;
from "core/time" import * as time;

export fn main(): Result<(), Str> {
  let ctx = context {
    Alloc: host.alloc, Stdout: host.stdout, Tasks: host.tasks, Clock: host.clock,
  };

  // A heap Str with room to grow, owned by the closure's environment.
  let seed = "ab".repeat(ctx, 2);
  let ns = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15];
  // The sleep is what makes the steps *overlap*: sixteen carriers are
  // dispatched in microseconds, wait together, and reach the concat at the
  // same instant. Without it a step finishes before the next is dispatched and
  // its carrier is handed straight back, so a fan-out over trivial work is a
  // sequential walk with extra steps and would prove nothing about sharing.
  let grown = tasks.parallel(ctx, ns, fn(c, i, n) => {
    let _ = time.sleepMs(c, 40);
    seed.concat(c, str.fromInt(c, n))
  });
  let _ = io.println(ctx, grown.join(ctx, " ")).ignore();
  let _ = io.println(ctx, seed).ignore();
  .Ok(())
}
"#,
        concat!(
            "abab0 abab1 abab2 abab3 abab4 abab5 abab6 abab7 abab8 abab9 ",
            "abab10 abab11 abab12 abab13 abab14 abab15\n",
            "abab\n",
        ),
    );
}

/// A task is handed the **caller's context**, and reads a value out of it, on
/// all three backends.
///
/// The test above cannot see this and never could. Every implementation in
/// `core/host` is an empty struct, so a context built entirely out of them is
/// zero words wide — and so is any one of its bindings — which makes "the step
/// was handed the context" and "the step was handed the scheduler" the same
/// bytes. The step's first parameter was the second of those for as long as
/// `parallel` spelled it `Self`, and nothing above would have noticed.
///
/// So this program binds a `Clock` **it wrote itself**, carrying an `I64`. An
/// effect is an ordinary interface and anyone may write a type satisfying it
/// (SPEC 10.9), so a program may, and now the context is a word wide and the
/// scheduler is not: a step handed the wrong one answers `0` where it should
/// answer `7`, or reads a pointer that is not one.
///
/// Four claims, and each of them fails differently if the wrong value arrives:
///
///  * **one effect out of the context** — `time.now(c)` inside a step. The
///    reduced repro: `[7, 9]` where `[12, 14]` was promised.
///  * **two effects at once** — `str.format` needs the `Alloc` and reads the
///    `Clock`, so a step handed a value satisfying only `Tasks` could satisfy
///    neither.
///  * **nested** — the inner `parallel`'s receiver is the context the outer
///    step was handed, so the value has to survive going out to a step and
///    coming back in as a receiver.
///  * **the same context afterwards** — read once more in the caller's own
///    frame, so a step that consumed or moved what it was handed shows up
///    here rather than as a leak nobody looks at.
#[test]
fn a_task_is_handed_the_callers_context_on_every_backend() {
    rows_or_skip!();
    agree(
        "tasks.parallel context",
        r#"
from "core/effect" import { Alloc, Clock, Stdout, Tasks };
from "core/host" import * as host;
from "core/io" import * as io;
from "core/str" import * as str;
from "core/tasks" import * as tasks;
from "core/time" import * as time;

/// A `Clock` a program wrote, carrying a word — so the context that binds it is
/// a word wide and is not the same value as the scheduler beside it.
struct Ticker {
  at: I64,
}

impl Clock for Ticker {
  fn nowMillis(self): I64 { self.at }
  fn sleepMillis(self, millis: Int): () { () }
}

fn show<C: Alloc>(ctx: C, xs: [Str]): Str { xs.join(ctx, ",") }

export fn main(): Result<(), Str> {
  let ctx = context {
    Alloc: host.alloc,
    Clock: Ticker { at: 5 },
    Stdout: host.stdout,
    Tasks: host.tasks,
  };

  // One effect, read inside the step.
  let stamped = tasks.parallel(ctx, [7, 9], fn(c, i, x) => time.now(c).0 + x);
  let _ = io.println(ctx, show(ctx, stamped.mapCtx(ctx, fn(c, n) => str.fromInt(c, n)))).ignore();

  // Two effects in one expression: `Alloc` to build the string, `Clock` to
  // fill it.
  let both = tasks.parallel(ctx, [7, 9], fn(c, i, x) => str.format(c, "${time.now(c).0}:${i}:${x}"));
  let _ = io.println(ctx, show(ctx, both)).ignore();

  // The context handed out to a step and back in as a receiver.
  let nested = tasks.parallel(ctx, [7], fn(c, i, x) =>
    tasks.parallel(c, [x, x], fn(d, j, y) => time.now(d).0 + y + j).fold(fn(a, m) => a + m, 0));
  let _ = io.println(ctx, show(ctx, nested.mapCtx(ctx, fn(c, n) => str.fromInt(c, n)))).ignore();

  // And the caller's own copy still answers.
  let _ = io.println(ctx, "${time.now(ctx).0}").ignore();
  .Ok(())
}
"#,
        concat!("12,14\n", "5:0:7,5:1:9\n", "25\n", "5\n"),
    );
}

/// **A request handler entered through a wrapper is handed the wrapper.**
///
/// This row was written for a fault, and it outlived it. Wave-8 G4 found
/// `core/alloc`'s `Scoped<C>` faulting under the stencil backend with SIGBUS
/// before a line was flushed, because `Listen.listen` then took its request
/// handler as a `Self`-spelled callback and *invoked* it: `Self` is `Scoped<C>`
/// at the wrapper and the acceptor inside it, so the wrapper had to rebuild the
/// handler, and monomorphization used to rebuild it at the wrong type. Three
/// flips narrowed it — a zero-sized implementation passes, an acceptor that
/// refuses passes, and the same wrapper without the generic callback passes.
///
/// **The effect no longer carries a handler.** `core/net/server`'s `run` owns
/// the accept loop and calls `onRequest` itself, under the caller's own
/// context, so no `Self`-spelled callback is left in the standard library and
/// the compiler rule that fixed the fault — `middle/monomorphize.rs`'s
/// `rewrite_call_args`, and `implementing_ty` beside it — is guarded by their
/// own unit tests and by `semantics/expressions.rs`'s snippets, which declare
/// an effect of their own for the purpose.
///
/// What this row pins is the *value* the handler receives with the wrapper on
/// the path: a `Wrap<C>` forwarding an effect whose results are aggregates, a
/// handler written against a bound its own `C` declares and the acceptor does
/// not, and a generic `wrapped(ctx, body)` in front of the whole thing. `Plain`
/// still carries a word for the first flip's sake: a context whose bindings are
/// all zero-sized is zero words wide, and `Wrap<ctx>` and `Wrap<OneShot>` are
/// then the same bytes, which is what hid the original fault.
///
/// **The accept loop is back.** F3 put a worker per handler inside `run`, and
/// this program then faulted with no output at all under the LLVM backend —
/// while the frame-threaded backend and JavaScript both answered correctly, and
/// while a server built on the *real* acceptor
/// (`llvm::a_server_answers_a_request_on_a_socket` and
/// `llvm::fifty_requests_are_answered_at_once`) went on passing. For one wave
/// this row drove `server.bind` instead and entered the handler by hand, which
/// kept the wrapper claims and lost the loop.
///
/// It was **not** a backend fault, which is why it survived a wave of looking
/// at one. `middle/rc.rs` planned a `let` binding's drop *before* the `incref`
/// that paid for it, so a fresh value bound and not read was freed and then
/// written through — and the frame-threaded backend was emitting the same
/// use-after-free on the same program and getting away with it, because the
/// crash lands on an allocation made later.
/// `reports/llvm-parallel-listen-fix.md` is the bisect and the fix.
///
/// So `server.serve` is back, and the program below drives the whole
/// bind-accept-read-answer-close cycle through the wrapper, on a context that
/// grants `Tasks`. **`serve` is the only thing this `main` does**, and that is
/// load-bearing rather than tidy: a version of it that also called
/// `server.bind` — before the loop or after it — *passed* on the broken
/// toolchain, because a second call moves the heap under the first. The `bind`
/// half is the row below, for exactly that reason.
///
/// `OneShot` carries an `I64` and not the `Str` it used to, and that is the
/// stencil backend's own limit showing rather than a weakening of the case: its
/// `Tasks.parallel` refuses a step whose *context* owns a reference count
/// (`stencil/rtcall.rs`), so a context binding a `Str`-carrying acceptor would
/// be a build error on one of the three backends being compared. The claim was
/// never about the field's type — it is that the context is a word wide and is
/// not the scheduler — and `Plain` and the `I64` here both keep it.
#[test]
fn a_handler_a_wrapper_rebuilt_is_entered_on_every_backend() {
    rows_or_skip!();
    agree(
        "rebuilt handler",
        r#"
from "core/effect" import {
  Alloc, Header, IoError, Listen, Listener, Received, Region, Request, Response,
  Serve, ServeError, Stdout, Tasks,
};
from "core/host" import * as host;
from "core/io" import * as io;
from "core/net/server" import * as server;

/// An `Alloc` that is not zero-sized, so the context binding it is a word wide.
struct Plain {
  n: I64,
}

impl Alloc for Plain {
  fn allocate(self, bytes: Int): Region { Region(bytes + self.n) }
}

/// An acceptor that hands out one request naming the address it bound, takes
/// one answer, and closes.
struct OneShot {
  binds: I64,
}

impl Listen for OneShot {
  fn listenBind(
    self,
    address: Str,
    port: Int,
    plan: [Serve],
    requestLimit: Int,
    idleTimeoutMillis: Int,
  ): Result<Listener, ServeError> {
    match (self.binds) {
      0 => .Err(ServeError { cause: .PermissionDenied, detail: "" }),
      _ => .Ok(Listener { handle: 1, port: 8080, handlers: 1 }),
    }
  }

  fn listenAccept(self, handle: Int): Result<Int, ServeError> {
    match (handle) {
      1 => .Ok(7),
      _ => .Err(ServeError { cause: .Closed, detail: "" }),
    }
  }

  fn listenRequest(self, connection: Int): Result<Request, ServeError> {
    match (connection) {
      7 => .Ok(Request {
              method: .Get,
              url: "10.0.0.1",
              headers: [],
              body: [],
              timeoutMillis: 0,
          }),
      _ => .Err(ServeError { cause: .Closed, detail: "" }),
    }
  }

  fn listenRespond(
    self,
    connection: Int,
    status: Int,
    headers: [Header],
    body: [U8],
  ): Result<(), ServeError> {
    match (status) {
      200 => .Err(ServeError { cause: .Closed, detail: "" }),
      _ => .Err(ServeError { cause: .Transport, detail: "not 200" }),
    }
  }

  fn listenClose(self, handle: Int): () { () }

  fn listenUpgrade(self, connection: Int): Result<Int, ServeError> {
    .Err(ServeError { cause: .Unsupported, detail: "not an upgrade" })
  }

  fn listenReceive(self, socket: Int): Result<Received, ServeError> {
    .Err(ServeError { cause: .Closed, detail: "" })
  }
}

/// The wrapper: unbounded in `C`, exactly like `Scoped<C>`.
struct Wrap<C>(C, I64);

impl<C> Alloc for Wrap<C> {
  fn allocate(self, bytes: Int): Region { Region(bytes) }
}

impl<C: Stdout> Stdout for Wrap<C> {
  fn print(self, text: Template): Result<(), IoError> { self.0.print(text) }
  fn println(self, text: Template): Result<(), IoError> { self.0.println(text) }
  fn writeBytes(self, b: [U8]): Result<(), IoError> { self.0.writeBytes(b) }
}

impl<C: Tasks> Tasks for Wrap<C> {
  fn parallel<D, A, B>(self, ctx: D, items: [A], f: fn(D, Int, A) => B): [B] {
    self.0.parallel(ctx, items, f)
  }
}

impl<C: Listen> Listen for Wrap<C> {
  fn listenBind(
    self,
    address: Str,
    port: Int,
    plan: [Serve],
    requestLimit: Int,
    idleTimeoutMillis: Int,
  ): Result<Listener, ServeError> {
    self.0.listenBind(address, port, plan, requestLimit, idleTimeoutMillis)
  }

  fn listenAccept(self, handle: Int): Result<Int, ServeError> {
    self.0.listenAccept(handle)
  }

  fn listenRequest(self, connection: Int): Result<Request, ServeError> {
    self.0.listenRequest(connection)
  }

  fn listenRespond(
    self,
    connection: Int,
    status: Int,
    headers: [Header],
    body: [U8],
  ): Result<(), ServeError> {
    self.0.listenRespond(connection, status, headers, body)
  }

  fn listenClose(self, handle: Int): () { self.0.listenClose(handle) }

  fn listenUpgrade(self, connection: Int): Result<Int, ServeError> {
    self.0.listenUpgrade(connection)
  }

  fn listenReceive(self, socket: Int): Result<Received, ServeError> {
    self.0.listenReceive(socket)
  }
}

/// A handler written against a bound, which is what a request handler is.
///
/// **`serve` and nothing else**, and that is not laziness. The fault this row
/// exists to catch is a use-after-free whose crash lands on a *later*
/// allocation, so anything run beside `serve` in this `main` moves the heap
/// under it and can hide it: a version of this program that also called
/// `server.bind` — before the loop or after it — passed on the broken
/// toolchain while this one faulted. The `bind` half is a row of its own
/// below, for exactly that reason.
fn served<C: Alloc + Listen + Stdout + Tasks>(ctx: C): Int {
  let plan = server.Server {
    port: 0,
    address: .Some("10.0.0.1"),
    // The handler *uses* what it is handed, on a bound its own `C` declares
    // and the acceptor does not. A handler that ignored its first parameter
    // would pass whatever arrived.
    onRequest: fn(c, request) => {
      let _ = io.println(c, "handler on ${request.url}").ignore();
      Response { status: 200, headers: [], body: [] }
    },
  };
  match (server.serve(ctx, plan)) { .Ok(_ok) => 1, .Err(_e) => 0 }
}

/// The generic callback `scoped` is: it builds the wrapper and hands it over.
fn wrapped<C, T>(ctx: C, body: fn(Wrap<C>) => T): T {
  body(Wrap(ctx, 7))
}

export fn main(): Result<(), Str> {
  let ctx = context {
    Alloc: Plain { n: 0 },
    Stdout: host.stdout,
    Listen: OneShot { binds: 1 },
    Tasks: host.tasks,
  };
  let _ = io.println(host.stdout, "entering").ignore();
  let n = wrapped(ctx, fn(c) => served(c));
  let _ = io.println(host.stdout, "served ${n}").ignore();
  .Ok(())
}
"#,
        "entering\nhandler on 10.0.0.1\nserved 1\n",
    );
}

/// **A `Listener` a wrapper forwarded is read by the caller, on every backend.**
///
/// `bind` and `run` are `serve`'s two halves and are exported for one shape:
/// `port: 0`, where the operating system chooses the number and the program
/// has to publish it — write it to a file, print it, hand it to the task that
/// will connect — *between* the bind and the loop. `serve` never shows a caller
/// the `Listener` it made, so the row above cannot pin that shape, and this one
/// does: an aggregate `Result<Listener, ServeError>` out of a hand-written
/// acceptor, back through a generic wrapper's `Listen` forward, matched by the
/// caller, with two fields of the `.Ok` payload read off it.
///
/// It is a program of its own rather than three more lines in the row above,
/// and that is the lesson of `reports/llvm-parallel-listen-fix.md` written into
/// the file. The fault that row exists to catch is a use-after-free whose crash
/// lands on an allocation made *later*, so a second call in the same `main`
/// moves the heap under the first and hides it — measured, not feared: on the
/// broken toolchain the row above faulted on its own and *passed* with a
/// `server.bind` call added to the same `main`, before the loop or after it.
/// So the two claims are two programs, and this one is deliberately not a
/// paragraph in the other.
///
/// The error arm is asked for too, because an acceptor that always succeeds
/// makes `.Err`'s `Str`-carrying payload unreachable and leaves the enum's
/// second variant uncompiled.
#[test]
fn a_bound_listener_crosses_a_wrapper_on_every_backend() {
    rows_or_skip!();
    agree(
        "bound listener",
        r#"
from "core/effect" import {
  Alloc, Header, IoError, Listen, Listener, Received, Region, Request, Response,
  Serve, ServeError, Stdout,
};
from "core/host" import * as host;
from "core/io" import * as io;
from "core/net/server" import * as server;
from "core/str" import * as str;

/// An acceptor that binds one address and refuses every other, and that
/// accepts nothing at all: this program never reaches the loop.
struct Gate {
  opens: I64,
}

impl Listen for Gate {
  fn listenBind(
    self,
    address: Str,
    port: Int,
    plan: [Serve],
    requestLimit: Int,
    idleTimeoutMillis: Int,
  ): Result<Listener, ServeError> {
    match (self.opens) {
      0 => .Err(ServeError { cause: .AddressInUse, detail: "taken" }),
      _ => .Ok(Listener { handle: 3, port: 8080, handlers: 4 }),
    }
  }

  fn listenAccept(self, handle: Int): Result<Int, ServeError> {
    .Err(ServeError { cause: .Closed, detail: "" })
  }

  fn listenRequest(self, connection: Int): Result<Request, ServeError> {
    .Err(ServeError { cause: .Closed, detail: "" })
  }

  fn listenRespond(
    self,
    connection: Int,
    status: Int,
    headers: [Header],
    body: [U8],
  ): Result<(), ServeError> {
    .Err(ServeError { cause: .Closed, detail: "" })
  }

  fn listenClose(self, handle: Int): () { () }

  fn listenUpgrade(self, connection: Int): Result<Int, ServeError> {
    .Err(ServeError { cause: .Unsupported, detail: "not an upgrade" })
  }

  fn listenReceive(self, socket: Int): Result<Received, ServeError> {
    .Err(ServeError { cause: .Closed, detail: "" })
  }
}

/// The wrapper, unbounded in `C`, forwarding the effect whose results are
/// aggregates.
struct Wrap<C>(C, I64);

impl<C> Alloc for Wrap<C> {
  fn allocate(self, bytes: Int): Region { Region(bytes) }
}

impl<C: Stdout> Stdout for Wrap<C> {
  fn print(self, text: Template): Result<(), IoError> { self.0.print(text) }
  fn println(self, text: Template): Result<(), IoError> { self.0.println(text) }
  fn writeBytes(self, b: [U8]): Result<(), IoError> { self.0.writeBytes(b) }
}

impl<C: Listen> Listen for Wrap<C> {
  fn listenBind(
    self,
    address: Str,
    port: Int,
    plan: [Serve],
    requestLimit: Int,
    idleTimeoutMillis: Int,
  ): Result<Listener, ServeError> {
    self.0.listenBind(address, port, plan, requestLimit, idleTimeoutMillis)
  }

  fn listenAccept(self, handle: Int): Result<Int, ServeError> {
    self.0.listenAccept(handle)
  }

  fn listenRequest(self, connection: Int): Result<Request, ServeError> {
    self.0.listenRequest(connection)
  }

  fn listenRespond(
    self,
    connection: Int,
    status: Int,
    headers: [Header],
    body: [U8],
  ): Result<(), ServeError> {
    self.0.listenRespond(connection, status, headers, body)
  }

  fn listenClose(self, handle: Int): () { self.0.listenClose(handle) }

  fn listenUpgrade(self, connection: Int): Result<Int, ServeError> {
    self.0.listenUpgrade(connection)
  }

  fn listenReceive(self, socket: Int): Result<Received, ServeError> {
    self.0.listenReceive(socket)
  }
}

/// Binds, and answers what the acceptor said — the port it chose and the
/// number of handlers it will host, both read off the `.Ok` payload.
fn published<C: Alloc + Listen + Stdout>(ctx: C, opens: Bool): Str {
  let plan = server.Server {
    port: 0,
    address: .Some(if (opens) { "10.0.0.1" } else { "0.0.0.0" }),
    onRequest: fn(c, request) => Response { status: 200, headers: [], body: [] },
  };
  match (server.bind(ctx, plan)) {
    .Ok(listener) =>
      str.format(ctx, "port ${listener.port} on ${listener.handlers} handlers"),
    .Err(e) => str.format(ctx, "${server.errorText(e)}: ${e.detail}"),
  }
}

fn wrapped<C, T>(ctx: C, body: fn(Wrap<C>) => T): T {
  body(Wrap(ctx, 7))
}

export fn main(): Result<(), Str> {
  let open = context { Alloc: host.alloc, Stdout: host.stdout, Listen: Gate { opens: 1 } };
  let shut = context { Alloc: host.alloc, Stdout: host.stdout, Listen: Gate { opens: 0 } };
  let _ = io.println(host.stdout, wrapped(open, fn(c) => published(c, true))).ignore();
  let _ = io.println(host.stdout, wrapped(shut, fn(c) => published(c, false))).ignore();
  .Ok(())
}
"#,
        "port 8080 on 4 handlers\nthe address is already in use: taken\n",
    );
}

/// **A value leaves a scope alive, on every backend.**
///
/// `core/alloc::scoped` serves the body's blocks out of its own `mmap`s and
/// unmaps them when the body returns (G5), so the answer is deep-copied onto
/// the caller's allocator on the way out. This is that claim as a program: four
/// answers built inside scopes out of blocks the *program* allocated — a nested
/// `[[Str]]`, an enum variant carrying a list, a closure's captured
/// environment — read after the scopes have ended, and the first one read again
/// after two further scopes have mapped and released pages of their own.
///
/// **Every string in it is built rather than written.** A literal is `IMMORTAL`
/// and lives in the artifact's constant pool, so a version of this program that
/// answered literals would print the right thing whatever the copy glue did.
/// `repeat` allocates, which is what makes the last line a use-after-free
/// detector rather than a spelling check.
///
/// The conformance corpus has the same shapes with thirty cases
/// (`lib/memory/test/copyout.buri`), and `native::conformance` runs that file
/// through the frame-threaded backend. This is here because it is the one place
/// the **LLVM** backend's own copy glue — a different walk, in a different
/// file — is held to the same answer.
#[test]
fn a_value_leaves_a_scope_alive_on_every_backend() {
    rows_or_skip!();
    agree(
        "copy out of a scope",
        r#"
from "core/alloc" import * as alloc;
from "core/effect" import { Alloc, Stdout };
from "core/host" import * as host;
from "core/io" import * as io;
from "core/list" import * as list;

enum Answer {
  Nothing,
  Text(Str),
  Many([Str]),
}

/// A `Str` this program allocated, rather than one the compiler interned.
fn built<C: Alloc>(ctx: C, unit: Str, times: Int): Str {
  unit.repeat(ctx, times)
}

fn flatten<C: Alloc>(ctx: C, xss: [[Str]]): Str {
  xss.mapCtx(ctx, fn(c, xs) => xs.join(c, "+")).join(ctx, "|")
}

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout };

  let nested = alloc.scoped(ctx, fn(c) => [
    [built(c, "a", 2), built(c, "b", 3)],
    [built(c, "c", 1)],
  ]);
  let _ = io.println(ctx, flatten(ctx, nested)).ignore();

  let answer = alloc.scoped(ctx, fn(c) => Answer.Many([built(c, "m", 2), built(c, "n", 3)]));
  let _ = io.println(ctx, match (answer) {
    .Nothing => "none",
    .Text(t) => t,
    .Many(xs) => xs.join(ctx, ","),
  }).ignore();

  let f = alloc.scoped(ctx, fn(c) => {
    let captured = built(c, "z", 4);
    fn() => captured
  });
  let _ = io.println(ctx, f()).ignore();

  // Two more scopes, each mapping and releasing pages of its own. An address
  // that had escaped the first arena is one these are entitled to hand out
  // again — so the last line is the first line only if the answer was copied.
  let churn = alloc.scoped(ctx, fn(c) => built(c, "q", 4096));
  let more = alloc.scoped(ctx, fn(c) => [built(c, "r", 2048)]);
  let _ = io.println(ctx, "${churn.len()} ${more.len()}").ignore();
  let _ = io.println(ctx, flatten(ctx, nested)).ignore();
  .Ok(())
}
"#,
        concat!("aa+bbb|c\n", "mm,nnn\n", "zzzz\n", "4096 1\n", "aa+bbb|c\n"),
    );
}

/// **A scope per task, each on its own carrier.**
///
/// The arena a scope serves out of is a property of the **carrier**
/// (`memory::arena_slot_of_carrier`), not of the process — so sixteen steps of
/// one `Tasks.parallel` can each open a scope, allocate in it and answer out of
/// it at the same moment, and none of them can see another's arena or unmap
/// another's pages. That is the note's server workload — a scope per request —
/// with the server taken out of it, and it is the case that would fail if the
/// active arena were a global.
///
/// The scope is **inside** the step and not around the fan-out, which is
/// deliberate: `can_park` does not propagate through an indirect call today, so
/// a JavaScript caller of *any* generic wrapper whose callback parks does not
/// await it — reproduced with no `core/alloc` in the program at all, and
/// recorded in `reports/wave8-g5.md`. Putting the scope inside the step is the
/// shape a request handler has anyway.
#[test]
fn a_scope_per_task_answers_on_every_backend() {
    rows_or_skip!();
    agree(
        "a scope per task",
        r#"
from "core/alloc" import * as alloc;
from "core/effect" import { Alloc, Stdout, Tasks };
from "core/host" import * as host;
from "core/io" import * as io;
from "core/list" import * as list;
from "core/tasks" import * as tasks;

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout, Tasks: host.tasks };
  let ns = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15];
  let out = tasks.parallel(ctx, ns, fn(c, i, n) =>
    alloc.scoped(c, fn(d) => "-".repeat(d, n + 1)));
  let _ = io.println(ctx, out.join(ctx, ",")).ignore();
  let _ = io.println(ctx, "${out.len()}").ignore();
  .Ok(())
}
"#,
        concat!(
            "-,--,---,----,-----,------,-------,--------,---------,----------,",
            "-----------,------------,-------------,--------------,",
            "---------------,----------------\n16\n"
        ),
    );
}

/// A scope returns when its body and every task spawned into it have finished,
/// on every backend — and a task spawned *after* it returned still runs.
///
/// The whole of `core/tasks`'s background half in one program, and every line
/// of the answer is an order rather than a timing: one task per round, so the
/// output is the same whether the round ran on a carrier of its own, inside a
/// `Promise.all`, or one after another on the calling carrier. That is the
/// point — the scope's promise is what agrees across the three, and the overlap
/// is deliberately not asserted anywhere.
///
/// Three claims, in the order they are printed. `task` is after `before` and
/// before `after`, so the scope waited. `nested` is inside the same window, so
/// a task spawned by a task is waited for too. `late` is after `after`, spawned
/// through a `Scope` a lambda captured once the body had already returned —
/// which is the page's shape, written here because native is where the runtime
/// tables are, and the semantics are one rule on every platform.
#[test]
fn a_spawned_task_runs_before_its_scope_returns_on_every_backend() {
    rows_or_skip!();
    agree(
        "tasks.scope",
        r#"
from "core/effect" import { Alloc, Stdout, Tasks };
from "core/host" import * as host;
from "core/io" import * as io;
from "core/tasks" import * as tasks;

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout, Tasks: host.tasks };
  let _ = io.println(ctx, "before").ignore();
  let later = tasks.scope(ctx, fn(c, here) => {
    let _ = tasks.spawn(c, here, fn(c2) => {
      let _ = io.println(c2, "task").ignore();
      let _ = tasks.spawn(c2, here, fn(c3) => {
        let _ = io.println(c3, "nested").ignore();
        ()
      });
      ()
    });
    fn(c4) => {
      let _ = tasks.spawn(c4, here, fn(c5) => {
        let _ = io.println(c5, "late").ignore();
        ()
      });
      ()
    }
  });
  let _ = io.println(ctx, "after").ignore();
  let fire = later;
  let _ = fire(ctx);
  .Ok(())
}
"#,
        "before\ntask\nnested\nafter\nlate\n",
    );
}

/// **A task spawned inside an arena runs after that arena is gone.**
///
/// `core/tasks` says `copyAcross` deep-copies the task out of every arena
/// before it is queued, "because the runtime holds it past this call". This is
/// the program that reaches that: the scope is opened outside the arena, so the
/// body of `alloc.scoped` is not the drain and the task it spawns only runs
/// once the arena has been released — and the closure's captured string was
/// built in pages the arena is about to unmap.
///
/// `churn` is `cli/tests/conformance/lib/actor/test/scoped.buri`'s, and for the
/// reason its header gives: a released arena block of exactly one standard
/// block goes back to a pool rather than to the kernel, so the value that
/// crosses is 70 000 bytes and the eight small scopes are what hand the pooled
/// pages out again. Without both halves a dangling read is a gamble rather than
/// a fault.
///
/// On JavaScript there is no arena and no page to take away, and the row still
/// says what the answer is.
#[test]
fn a_task_spawned_inside_an_arena_keeps_what_it_captured_on_every_backend() {
    rows_or_skip!();
    agree(
        "tasks.spawn inside an arena",
        r#"
from "core/alloc" import * as alloc;
from "core/effect" import { Alloc, Stdout, Tasks };
from "core/host" import * as host;
from "core/io" import * as io;
from "core/tasks" import * as tasks;

/// Bigger than one standard arena block, so its mapping is unmapped rather than
/// pooled.
let LOOSE: Int = 70000;

/// Scopes that map and release pages of their own, so an arena the spawn left
/// behind is one this allocator is entitled to hand out again.
fn churn<C: Alloc>(ctx: C): Int {
  let small = [1, 2, 3, 4, 5, 6, 7, 8].mapCtx(ctx, fn(k, n) => {
    alloc.scoped(k, fn(c) => "z".repeat(c, 40 + n).len())
  });
  let large = alloc.scoped(ctx, fn(c) => "y".repeat(c, LOOSE).len());
  small.len() + large
}

export fn main(): Result<(), Str> {
  let ctx = context {
    Alloc: host.alloc,
    Stdout: host.stdout,
    Tasks: host.tasks,
  };
  let _ = tasks.scope(ctx, fn(c, here) => {
    // The scope's body is the drain, so this spawn queues rather than running:
    // the task is entered after `alloc.scoped` below has answered.
    let built = alloc.scoped(c, fn(d) => {
      let big = "s".repeat(d, LOOSE);
      let _ = tasks.spawn(d, here, fn(e) => {
        let _ = io.println(e, "the task read ${big.len()}").ignore();
        ()
      });
      big.len()
    });
    let _ = churn(c);
    io.println(c, "the arena built ${built}").ignore()
  });
  .Ok(())
}
"#,
        "the arena built 70000\nthe task read 70000\n",
    );
}

/// **The edges of a scope, on every backend**: a body that gives up, a scope
/// inside a scope, and an actor whose step spawns.
///
/// The row above says a scope waits for what was spawned into it. These are the
/// three shapes around that claim which a program actually writes, and none of
/// them is a timing:
///
///  * a body that answers `.Err` is still a body, so the scope waits for its
///    tasks and then hands the error back;
///  * an inner scope closes before the outer one, so its task has finished
///    before the outer body's next line;
///  * a `Scope` fits in a message, so an actor's step can start background work
///    — and the spawn lands in the drain the *sender* is inside, which is why
///    the job runs after the body rather than during the step.
///
/// `cli/tests/conformance/lib/tasks/test/background.buri` is the same claims as
/// a corpus, and its native run is the stencil backend alone.
#[test]
fn the_edges_of_a_scope_agree_on_every_backend() {
    rows_or_skip!();
    agree(
        "tasks.scope edges",
        r#"
from "core/actor" import * as actor;
from "core/actor" import { Actor, Stepped };
from "core/effect" import { Alloc, Stdout, Tasks };
from "core/host" import * as host;
from "core/io" import * as io;
from "core/tasks" import * as tasks;
from "core/tasks" import { Scope };

enum Job {
  Run(Scope),
}

enum Ran {
  Started(Int),
}

/// An actor that spawns rather than working. A `Scope` holds no context, so it
/// fits in a message the way an address fits in a state.
fn foreman<C: Alloc + Stdout + Tasks>(): Actor<C, Int, Job, Ran> {
  Actor {
    state: 0,
    step: fn(c, started, message) => {
      match (message) {
        .Run(here) => {
          let _ = tasks.spawn(c, here, fn(c2) => {
            let _ = io.println(c2, "the job ran").ignore();
            ()
          });
          Stepped { state: started + 1, answer: .Started(started + 1) }
        },
      }
    },
  }
}

export fn main(): Result<(), Str> {
  let ctx = context {
    Alloc: host.alloc,
    Stdout: host.stdout,
    Tasks: host.tasks,
  };

  // A body that gives up. The scope has no opinion about that and is still what
  // waits, so the task spawned before the error runs anyway.
  let refused: Result<Int, Str> = tasks.scope(ctx, fn(c, here) => {
    let _ = tasks.spawn(c, here, fn(c2) => {
      let _ = io.println(c2, "the task still ran").ignore();
      ()
    });
    .Err("the body gave up")
  });
  let said = match (refused) {
    .Ok(_n) => "ran",
    .Err(why) => why,
  };
  let _ = io.println(ctx, "body ${said}").ignore();

  // A scope inside a scope waits for its own tasks and for nobody else's.
  let _ = tasks.scope(ctx, fn(c, outer) => {
    let _ = tasks.spawn(c, outer, fn(c2) => {
      let _ = io.println(c2, "outer task").ignore();
      ()
    });
    let _ = tasks.scope(c, fn(d, inner) => {
      let _ = tasks.spawn(d, inner, fn(d2) => {
        let _ = io.println(d2, "inner task").ignore();
        ()
      });
      io.println(d, "inner body").ignore()
    });
    io.println(c, "outer body").ignore()
  });

  // A step that spawns. The body is still running, so the scope is still its
  // own drain and neither job starts until the body has finished.
  let boss = actor.start(ctx, foreman());
  let started = tasks.scope(ctx, fn(c, here) => {
    let first = match (boss.sendMessage(c, .Run(here))) {
      .Ok(.Started(n)) => n,
      _gone => -1,
    };
    let second = match (boss.sendMessage(c, .Run(here))) {
      .Ok(.Started(n)) => n,
      _gone => -1,
    };
    let _ = io.println(c, "asked twice").ignore();
    first + second
  });
  let _ = io.println(ctx, "started ${started}").ignore();
  let _ = boss.stop(ctx).ignore();
  .Ok(())
}
"#,
        "the task still ran\nbody the body gave up\ninner body\ninner task\nouter body\n\
         outer task\nasked twice\nthe job ran\nthe job ran\nstarted 3\n",
    );
}

/// **A timer is a task that sleeps**, and the sleep is a wait on every backend.
///
/// The row above proves the order; this one proves the waiting. `core/tasks`
/// ships no `Timer` and no `setTimeout` — the whole claim is that
/// `clock.sleepMillis` inside a spawned task is one — so a backend where the
/// sleep answered without waiting would pass every ordering assertion in this
/// file and still have no timers in it.
///
/// The clock is read on the calling task, before and after the scope, and what
/// is printed is a comparison rather than a duration: a program that says
/// `waited: true` says the same thing on a machine of any speed, and a sleep
/// that did nothing prints `waited: false` on all of them. Fifty milliseconds
/// three times over is the whole cost of the row.
#[test]
fn a_spawned_timer_waits_on_the_clock_on_every_backend() {
    rows_or_skip!();
    agree(
        "tasks.scope timer",
        r#"
from "core/effect" import { Alloc, Clock, Stdout, Tasks };
from "core/host" import * as host;
from "core/io" import * as io;
from "core/tasks" import * as tasks;
from "core/time" import * as time;

export fn main(): Result<(), Str> {
  let ctx = context {
    Alloc: host.alloc, Clock: host.clock, Stdout: host.stdout, Tasks: host.tasks,
  };
  let started = time.now(ctx).0;
  let _ = tasks.scope(ctx, fn(c, here) => {
    let _ = tasks.spawn(c, here, fn(c2) => {
      let _ = time.sleepMs(c2, 50);
      let _ = io.println(c2, "the timer fired").ignore();
      ()
    });
    ()
  });
  let waited = match (time.now(ctx).0 - started >= 50) {
    true => "true",
    false => "false",
  };
  let _ = io.println(ctx, "waited: ${waited}").ignore();
  .Ok(())
}
"#,
        "the timer fired\nwaited: true\n",
    );
}

/// **Stopping is cooperative**, and this is what that looks like on every
/// backend: a spawned loop asks an actor whether to carry on, and ends when it
/// is told not to.
///
/// `core/tasks` has no `cancel` and deliberately nothing to add one to, so the
/// module's answer to "how do I stop a background task" is this program. It is
/// also the one shape that puts the two concurrency modules inside each other —
/// a `sendMessage` on a task the scope's drain is running — and the pair have
/// to agree about it wherever they both exist.
#[test]
fn a_spawned_loop_stops_when_its_actor_says_so_on_every_backend() {
    rows_or_skip!();
    agree(
        "tasks.scope with an actor",
        r#"
from "core/actor" import * as actor;
from "core/actor" import { Actor, Address, Stepped };
from "core/effect" import { Alloc, Stdout, Tasks };
from "core/host" import * as host;
from "core/io" import * as io;
from "core/tasks" import * as tasks;

enum Ask {
  MayI,
}

enum Turn {
  Carry,
  Stop,
}

fn gate<C: Alloc + Tasks>(turns: Int): Actor<C, Int, Ask, Turn> {
  Actor {
    state: turns,
    step: fn(c, left, message) => {
      match (left > 0) {
        true => Stepped { state: left - 1, answer: .Carry },
        false => Stepped { state: left, answer: .Stop },
      }
    },
  }
}

fn frames<C: Alloc + Stdout + Tasks>(
  ctx: C,
  keeper: Address<C, Int, Ask, Turn>,
  n: Int,
): () {
  match (keeper.sendMessage(ctx, .MayI)) {
    .Ok(.Carry) => {
      let _ = io.println(ctx, "frame ${n}").ignore();
      frames(ctx, keeper, n + 1)
    },
    _stopped => {
      let _ = io.println(ctx, "the loop stopped").ignore();
      ()
    },
  }
}

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout, Tasks: host.tasks };
  let keeper = actor.start(ctx, gate(3));
  let _ = tasks.scope(ctx, fn(c, here) => {
    let _ = tasks.spawn(c, here, fn(c2) => frames(c2, keeper, 0));
    ()
  });
  let _ = keeper.stop(ctx).ignore();
  .Ok(())
}
"#,
        "frame 0\nframe 1\nframe 2\nthe loop stopped\n",
    );
}

/// **A `*Ctx` combinator waits for a step that waits**, on every backend, and
/// answers the same list either way.
///
/// `core/list`'s five context-carrying combinators hand the step the caller's
/// whole context, so the step may do anything the caller may. On the natives
/// that is a call and a return; on JavaScript a step that waits is an `async`
/// function, and a combinator that ran it without awaiting answered a list of
/// **promises** — `[object Promise]` where a number belonged, with the work
/// itself still queued when `main` returned. That is exactly the kind of
/// backend-shaped wrong answer this file exists to catch, and it is invisible
/// to a suite that runs one backend: the natives were right all along.
///
/// An actor is the instrument, for `an_actor_counts_the_same_on_every_backend`'s
/// reason and one more: `sendMessage` waits on the program rather than on the
/// world (`middle::rc::suspends`), so the row costs no wall-clock time, and the
/// recorder's state is a *value* — folded as `seen * 10 + n` — so each line
/// says both that the combinator's answer is complete and that every step
/// really ran, in order.
#[test]
fn a_waiting_step_runs_under_every_ctx_combinator_on_every_backend() {
    rows_or_skip!();
    agree(
        "list *Ctx with a waiting step",
        r#"
from "core/actor" import * as actor;
from "core/actor" import { Actor, Address, Stepped, Stopped };
from "core/effect" import { Alloc, Stdout, Tasks };
from "core/host" import * as host;
from "core/io" import * as io;
from "core/str" import * as str;

enum Note {
  Saw(Int),
  Read,
}

enum Heard {
  Noted(Int),
  Log(Int),
}

fn recorder<C: Alloc + Tasks>(): Actor<C, Int, Note, Heard> {
  Actor {
    state: 0,
    step: fn(c, seen, note) => {
      match (note) {
        .Saw(n) => Stepped { state: seen * 10 + n, answer: .Noted(n * 2) },
        .Read => Stepped { state: seen, answer: .Log(seen) },
      }
    },
  }
}

fn noted(r: Result<Heard, Stopped>): Int {
  match (r) {
    .Ok(.Noted(n)) => n,
    _otherwise => -1,
  }
}

fn heardSoFar(r: Result<Heard, Stopped>): Int {
  match (r) {
    .Ok(.Log(n)) => n,
    _otherwise => -1,
  }
}

fn shown<C: Alloc>(ctx: C, xs: [Int]): Str {
  xs.mapCtx(ctx, fn(c, v) => str.fromInt(c, v)).join(ctx, ",")
}

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout, Tasks: host.tasks };

  let mapping = actor.start(ctx, recorder());
  let mapped = [1, 2, 3].mapCtx(ctx, fn(c, x) => noted(mapping.sendMessage(c, .Saw(x))));
  let _ = io.println(
    ctx,
    "mapped ${shown(ctx, mapped)} heard ${heardSoFar(mapping.sendMessage(ctx, .Read))}",
  ).ignore();
  let _ = mapping.stop(ctx).ignore();

  let stepping = actor.start(ctx, recorder());
  let stepped = [4, 5, 6].mapCtxStep(ctx, fn(c, x) => noted(stepping.sendMessage(c, .Saw(x))));
  let _ = io.println(
    ctx,
    "stepped ${shown(ctx, stepped)} heard ${heardSoFar(stepping.sendMessage(ctx, .Read))}",
  ).ignore();
  let _ = stepping.stop(ctx).ignore();

  let filtering = actor.start(ctx, recorder());
  let kept = [1, 2, 3, 4].filterCtx(
    ctx,
    fn(c, x) => noted(filtering.sendMessage(c, .Saw(x))) % 4 == 0,
  );
  let _ = io.println(
    ctx,
    "kept ${shown(ctx, kept)} heard ${heardSoFar(filtering.sendMessage(ctx, .Read))}",
  ).ignore();
  let _ = filtering.stop(ctx).ignore();

  let folding = actor.start(ctx, recorder());
  let folded = [1, 2, 3].foldCtx(
    ctx,
    fn(c, acc: Int, x) => acc + noted(folding.sendMessage(c, .Saw(x))),
    0,
  );
  let _ = io.println(
    ctx,
    "folded ${folded} heard ${heardSoFar(folding.sendMessage(ctx, .Read))}",
  ).ignore();
  let _ = folding.stop(ctx).ignore();

  let tallying = actor.start(ctx, recorder());
  let tallied: Result<Int, Str> = [1, 2, 3].foldResultCtx(
    ctx,
    fn(c, acc: Int, x) => .Ok(acc + noted(tallying.sendMessage(c, .Saw(x)))),
    0,
  );
  let _ = io.println(
    ctx,
    "tallied ${tallied.withDefault(-1)} heard ${heardSoFar(tallying.sendMessage(ctx, .Read))}",
  ).ignore();
  let _ = tallying.stop(ctx).ignore();

  // The other half of the rule, on the same combinator: a step that never
  // waits leaves its combinator synchronous, and answers the same here.
  let _ = io.println(ctx, "plain ${shown(ctx, [1, 2, 3].mapCtx(ctx, fn(c, x) => x * 2))}").ignore();
  .Ok(())
}
"#,
        "mapped 2,4,6 heard 123\n\
         stepped 8,10,12 heard 456\n\
         kept 2,4 heard 1234\n\
         folded 12 heard 123\n\
         tallied 12 heard 123\n\
         plain 2,4,6\n",
    );
}

/// A task that aborts stops the program, with the same message and the same
/// status on every backend — and with what was printed before it flushed.
///
/// An abort is a write to standard error and an exit, never an unwind (SPEC
/// 6.9), so there is nothing for the trampoline to do about one and that is
/// precisely the claim: the entry thunk is a frame in the middle, and a frame
/// in the middle that had *anything* to do with an abort would be a frame that
/// could get it wrong. Natively the abort happens inside a call the runtime
/// made, several frames below Buri code, and `cli/runtime/abort.rs` exits from
/// there; on JavaScript it is a throw out of a promise inside a `Promise.all`,
/// which the entry epilogue catches.
///
/// The task that aborts is **not the first**, which is what makes the flushed
/// output above the message meaningful: the earlier line was printed by a task
/// that had already finished.
#[test]
fn an_abort_inside_a_task_stops_the_program_the_same_way() {
    rows_or_skip!();
    abort_agrees(
        "tasks.parallel abort",
        r#"
from "core/effect" import { Alloc, Stdout, Tasks };
from "core/host" import * as host;
from "core/io" import * as io;
from "core/list" import * as list;
from "core/tasks" import * as tasks;

fn ratio(a: Int, b: Int): Int { a / b }

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout, Tasks: host.tasks };
  let _ = io.println(ctx, "before").ignore();
  let answers = tasks.parallel(ctx, [4, 2, 0], fn(c, i, n) => ratio(8, n));
  let _ = io.println(ctx, "${answers.len()}").ignore();
  .Ok(())
}
"#,
        "before\n",
        "division by zero",
    );
}

/// A task somebody **spawned** ends the program the same way, on every backend.
///
/// The row above aborts inside `parallel`'s step. This one aborts inside a task
/// that crossed into the runtime as a value and came back out through
/// `scopeTaskAt`, which `core/tasks::running` enters — a second frame in the
/// middle, and the same claim about it: an abort is a write to standard error
/// and an exit, so there is nothing for a scope's drain to do about one.
/// `core/tasks` says stopping is cooperative and that an abort is "not
/// something a second task can survive", and this is the sentence as a program.
///
/// The body's line is above the message, so what the abort interrupted was a
/// program that was already running. `cli/tests/crash/spawned_task_aborts.buri`
/// is the same program wherever the JavaScript corpus runs.
#[test]
fn an_abort_inside_a_spawned_task_stops_the_program_the_same_way() {
    rows_or_skip!();
    abort_agrees(
        "tasks.spawn abort",
        r#"
from "core/effect" import { Alloc, Stdout, Tasks };
from "core/host" import * as host;
from "core/io" import * as io;
from "core/tasks" import * as tasks;

fn ratio(a: Int, b: Int): Int { a / b }

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdout: host.stdout, Tasks: host.tasks };
  let _ = tasks.scope(ctx, fn(c, here) => {
    let _ = tasks.spawn(c, here, fn(c2) => {
      let _ = io.println(c2, "${ratio(8, 0)}").ignore();
      ()
    });
    io.println(c, "before").ignore()
  });
  let _ = io.println(ctx, "after").ignore();
  .Ok(())
}
"#,
        "before\n",
        "division by zero",
    );
}

// -------------------------------------------------------------------
// The table itself
// -------------------------------------------------------------------

/// Every row of §12 names a test in this file, and that test exists.
///
/// The table is the document this file holds up, so a row added next door
/// with no test — or a test renamed out from under a row — is a failure
/// here rather than a "pinned by" column that has quietly stopped being
/// true. The same relationship `native/conformance.rs`'s
/// **The note's counter, on every backend** — `core/actor`'s acceptance case.
///
/// It is here rather than only in the conformance corpus because the corpus's
/// native run is the stencil backend alone, and the claim this row makes is
/// about the *boundary*: nine runtime entries that move a Buri block by its two
/// words and hand it back, with `middle::rc`'s `crosses_tasks` marking every
/// one of those blocks. Each backend decides for itself where a `[T]` sits in a
/// struct and how a niche `Option<[T]>` is spelled, so one green pipeline says
/// nothing about the other.
///
/// Five claims in eight lines of output: a send sees the state the sends before
/// it left; three sends arrive in the order they were sent, read out of an
/// answer that is a different number for a different order; sixty-five sends all
/// arrive and leave nothing for `stop` to discard, because every one of them ran
/// the mailbox down before it answered; `onStop` runs once with the final state,
/// and the three actors' hooks answer their own numbers; and every operation
/// after `stop` — a send and a second `stop` — is `.Err`.
#[test]
fn an_actor_counts_the_same_on_every_backend() {
    rows_or_skip!();
    agree(
        "actor counter",
        r#"
from "core/actor" import * as actor;
from "core/actor" import { Actor, Address, Stepped, Stopped };
from "core/effect" import { Alloc, Stdout, Tasks };
from "core/host" import * as host;
from "core/io" import * as io;

enum CounterMessage {
  Add(Int),
  Push(Int),
  Get,
}

enum CounterAnswer {
  Done,
  Count(Int),
}

fn counter<C: Alloc + Stdout + Tasks>(): Actor<C, Int, CounterMessage, CounterAnswer> {
  Actor {
    state: 0,
    step: fn(c, count, message) => {
      match (message) {
        .Add(n) => Stepped { state: count + n, answer: .Done },
        .Push(n) => Stepped { state: count * 10 + n, answer: .Done },
        .Get => Stepped { state: count, answer: .Count(count) },
      }
    },
    onStop: .Some(fn(c, last) => io.println(c, "stopped at ${last}").ignore()),
  }
}

fn pump<C: Alloc + Stdout + Tasks>(
  ctx: C,
  address: Address<C, Int, CounterMessage, CounterAnswer>,
  left: Int,
): () {
  match (left <= 0) {
    true => (),
    false => {
      let _ = address.sendMessage(ctx, .Add(1)).ignore();
      pump(ctx, address, left - 1)
    },
  }
}

fn total(r: Result<CounterAnswer, Stopped>): Int {
  match (r) {
    .Ok(.Count(n)) => n,
    .Ok(_other) => -1,
    .Err(_e) => -1,
  }
}

export fn main(): Result<(), Str> {
  let ctx = context {
    Alloc: host.alloc,
    Stdout: host.stdout,
    Tasks: host.tasks,
  };

  // Two sends and a third that reads: each one runs the mailbox down, so the
  // answer is the state the sends before it left.
  let counted = actor.start(ctx, counter());
  let _ = counted.sendMessage(ctx, .Add(1)).ignore();
  let _ = counted.sendMessage(ctx, .Add(2)).ignore();
  let _ = io.println(ctx, "total ${total(counted.sendMessage(ctx, .Get))}").ignore();

  // Three sends, and the answer is the order they were written in. `Push`
  // multiplies before it adds, so 123 and 321 are different numbers and the
  // order is what the answer reads out.
  let queued = actor.start(ctx, counter());
  let _ = queued.sendMessage(ctx, .Push(1)).ignore();
  let _ = queued.sendMessage(ctx, .Push(2)).ignore();
  let _ = queued.sendMessage(ctx, .Push(3)).ignore();
  let _ = io.println(ctx, "batched ${total(queued.sendMessage(ctx, .Get))}").ignore();

  // Sixty-five sends, past the mailbox's sixty-four: every one of them ran the
  // box down before it answered, so the stop finds nothing to discard.
  let filled = actor.start(ctx, counter());
  let _ = pump(ctx, filled, 65);
  let _ = filled.stop(ctx).ignore();

  let _ = counted.stop(ctx).ignore();
  let _ = queued.stop(ctx).ignore();

  // Everything after the stop is `.Err(.Stopped)`, including a second stop.
  let _ = io.println(ctx, "after ${gone2(counted.sendMessage(ctx, .Add(1)))}").ignore();
  let _ = io.println(ctx, "asked ${gone2(counted.sendMessage(ctx, .Get))}").ignore();
  let _ = io.println(ctx, "again ${gone(counted.stop(ctx))}").ignore();
  .Ok(())
}

fn gone(r: Result<(), Stopped>): Str {
  match (r) {
    .Ok(_ok) => "ran",
    .Err(_e) => "stopped",
  }
}

fn gone2(r: Result<CounterAnswer, Stopped>): Str {
  match (r) {
    .Ok(_ok) => "ran",
    .Err(_e) => "stopped",
  }
}
"#,
        "total 3\nbatched 123\nstopped at 65\nstopped at 3\nstopped at 123\n\
         after stopped\nasked stopped\nagain stopped\n",
    );
}

/// **A step that sends to the actor running it, on every backend.**
///
/// Taking the state is non-blocking, so a send made from inside a step finds
/// the state already out and answers `.Err(.Stopped)` rather than waiting for
/// itself. The message is posted all the same, and the loop that was already
/// running steps it — once — before it puts the state back.
///
/// That is the one arm of `sendMessage` a single-driver program reaches where
/// the post succeeds and the drive does not, and it is the arm that hands a
/// reply slot back unanswered: the sender takes the slot it opened, finds
/// nothing in it, and the runtime bumps that slot's generation so the answer
/// the step writes afterwards lands nowhere rather than in whoever holds the
/// index by then. Both runtimes reuse slots behind a generation and the two
/// halves are separate code, so this row is what says they agree.
///
/// It is also the only way a program fills the mailbox: nothing drains while a
/// step holds the state, so sixty-four posts from inside one is the bound, and
/// all sixty-four are stepped when the step returns.
///
/// The message carries the function rather than the address because a step
/// cannot name its own address — a state or a message that held one would be a
/// type defined in terms of itself — and `fn(C) => Int` names no actor.
#[test]
fn a_step_that_sends_to_its_own_actor_is_refused_on_every_backend() {
    rows_or_skip!();
    agree(
        "actor reentrancy",
        r#"
from "core/actor" import * as actor;
from "core/actor" import { Actor, Address, Stepped, Stopped };
from "core/effect" import { Alloc, Stdout, Tasks };
from "core/host" import * as host;
from "core/io" import * as io;

enum Reentrant<C> {
  Reenter(fn(C) => Int),
  Tick,
  Say(Str),
  Get,
}

enum Reentered {
  Ticked,
  Said(Str),
  Reentered(Int),
  Count(Int),
}

fn reentrant<C: Alloc + Tasks>(): Actor<C, Int, Reentrant<C>, Reentered> {
  Actor {
    state: 0,
    step: fn(c, count, message) => {
      match (message) {
        .Reenter(f) => {
          let call = f;
          Stepped { state: count, answer: .Reentered(call(c)) }
        },
        .Tick => Stepped { state: count + 1, answer: .Ticked },
        .Say(word) => Stepped { state: count + 1, answer: .Said(word.concat(c, "!")) },
        .Get => Stepped { state: count, answer: .Count(count) },
      }
    },
  }
}

fn ticks<C: Alloc + Tasks>(
  ctx: C,
  address: Address<C, Int, Reentrant<C>, Reentered>,
  left: Int,
): Int {
  match (left <= 0) {
    true => 0,
    false => {
      let stepped = match (address.sendMessage(ctx, .Tick)) {
        .Ok(_answered) => 1,
        .Err(_gone) => 0,
      };
      stepped + ticks(ctx, address, left - 1)
    },
  }
}

/// Posts `left` messages that answer with a **block** rather than a number, and
/// counts how many were answered on the spot, which is none of them.
fn says<C: Alloc + Tasks>(
  ctx: C,
  address: Address<C, Int, Reentrant<C>, Reentered>,
  left: Int,
): Int {
  match (left <= 0) {
    true => 0,
    false => {
      let stepped = match (address.sendMessage(ctx, .Say("late"))) {
        .Ok(_answered) => 1,
        .Err(_gone) => 0,
      };
      stepped + says(ctx, address, left - 1)
    },
  }
}

fn number(answered: Result<Reentered, Stopped>): Int {
  match (answered) {
    .Ok(.Reentered(n)) => n,
    .Ok(.Count(n)) => n,
    .Ok(.Ticked) => -2,
    .Ok(.Said(_word)) => -3,
    .Err(_gone) => -1,
  }
}

fn said(answered: Result<Reentered, Stopped>): Str {
  match (answered) {
    .Ok(.Said(word)) => word,
    _otherwise => "?",
  }
}

export fn main(): Result<(), Str> {
  let ctx = context {
    Alloc: host.alloc,
    Stdout: host.stdout,
    Tasks: host.tasks,
  };

  // One send from inside a step: refused, and stepped anyway.
  let counted = actor.start(ctx, reentrant());
  let refused = counted.sendMessage(ctx, .Reenter(fn(c) => ticks(c, counted, 1)));
  let _ = io.println(ctx, "reentered ${number(refused)}").ignore();
  let _ = io.println(ctx, "counted ${number(counted.sendMessage(ctx, .Get))}").ignore();
  let _ = counted.stop(ctx).ignore();

  // Sixty-four of them: the mailbox at its bound, and every one of them is
  // stepped when the step that posted them returns.
  let filled = actor.start(ctx, reentrant());
  let fanned = filled.sendMessage(ctx, .Reenter(fn(c) => ticks(c, filled, 64)));
  let _ = io.println(ctx, "fanned ${number(fanned)}").ignore();
  let _ = io.println(ctx, "filled ${number(filled.sendMessage(ctx, .Get))}").ignore();
  let _ = filled.stop(ctx).ignore();

  // The same refusal, with an answer that is a **block**. Each of the eight
  // sends opens a reply slot and takes it back with nothing in it, and the loop
  // that runs afterwards writes a string this program built into a slot nobody
  // is holding. A slot that stayed open would keep that string for the life of
  // the process, which the exit audit reads as a leak; a slot given back
  // without its generation moving would hand the string to whoever holds the
  // index next, which the ordinary send below is what asks about — it opens an
  // index one of the eight was named by, and has to come back with its own
  // answer.
  let saying = actor.start(ctx, reentrant());
  let gaveUp = saying.sendMessage(ctx, .Reenter(fn(c) => says(c, saying, 8)));
  let _ = io.println(ctx, "gave up ${number(gaveUp)}").ignore();
  let _ = io.println(ctx, "heard ${number(saying.sendMessage(ctx, .Get))}").ignore();
  let _ = io.println(ctx, "mine ${said(saying.sendMessage(ctx, .Say("mine")))}").ignore();
  let _ = saying.stop(ctx).ignore();
  .Ok(())
}
"#,
        "reentered 0\ncounted 1\nfanned 0\nfilled 64\n\
         gave up 0\nheard 8\nmine mine!\n",
    );
}

/// **What a message, a state and an answer may carry, on every backend.**
///
/// The rows above drive the protocol with `Int` states and `Int` answers, which
/// is the one shape where nothing about the crossing is interesting. This one
/// carries the shapes that are: a block whose element occupies **no** bytes, a
/// list, text outside the basic plane, a struct two deep, and one actor's
/// address as another actor's state.
///
/// Each backend generates the release and the copy walk for the concrete `T`
/// itself, and the nine runtime entries are type-erased — the type argument
/// written at the call is the only thing that says what the block holds — so a
/// wrong walk on one pipeline is a wrong answer here rather than a difference
/// nobody sees. `agree` fails a row that leaks, which is the other half: four
/// blocks cross per send and every one of them is the runtime's between two
/// calls.
///
/// `cli/tests/conformance/lib/actor/test/payloads.buri` is the same claims as a
/// corpus, and its native run is the stencil backend alone.
#[test]
fn what_an_actors_messages_carry_agrees_on_every_backend() {
    rows_or_skip!();
    agree(
        "actor payloads",
        r#"
from "core/actor" import * as actor;
from "core/actor" import { Actor, Address, Stepped, Stopped };
from "core/effect" import { Alloc, Stdout, Tasks };
from "core/host" import * as host;
from "core/io" import * as io;
from "core/str" import * as str;

// --- nothing at all ---------------------------------------------------------

enum Ping {
  Ping,
}

fn silent<C: Alloc + Stdout + Tasks>(): Actor<C, (), Ping, ()> {
  Actor {
    state: (),
    step: fn(c, held, message) => Stepped { state: (), answer: () },
    onStop: .Some(fn(c, last) => io.println(c, "quiet").ignore()),
  }
}

fn answered(r: Result<(), Stopped>): Str {
  match (r) {
    .Ok(_ok) => "yes",
    .Err(_e) => "no",
  }
}

// --- a list, and text nobody can spell in ASCII ------------------------------

enum Bag {
  Fill([Int]),
  Push(Int),
  Drain,
}

enum Bagged {
  Kept(Int),
  Held([Int]),
}

fn bag<C: Alloc + Tasks>(): Actor<C, [Int], Bag, Bagged> {
  Actor {
    state: [],
    step: fn(c, held, message) => {
      match (message) {
        .Fill(next) => Stepped { state: next, answer: .Kept(next.len()) },
        .Push(n) => {
          let grown = held.push(c, n);
          Stepped { state: grown, answer: .Kept(grown.len()) }
        },
        .Drain => Stepped { state: [], answer: .Held(held) },
      }
    },
  }
}

fn drained(r: Result<Bagged, Stopped>): Int {
  match (r) {
    .Ok(.Held(xs)) => xs.len(),
    .Ok(.Kept(n)) => n,
    .Err(_e) => -1,
  }
}

enum Say {
  Say(Str),
  Read,
}

enum Said {
  Scalars(Int),
  Text(Str),
}

fn scribe<C: Alloc + Tasks>(initial: Str): Actor<C, Str, Say, Said> {
  Actor {
    state: initial,
    step: fn(c, held, message) => {
      match (message) {
        .Say(next) => Stepped { state: next, answer: .Scalars(next.len()) },
        .Read => Stepped { state: held, answer: .Text(held) },
      }
    },
  }
}

fn told(r: Result<Said, Stopped>): Str {
  match (r) {
    .Ok(.Text(s)) => s,
    .Ok(.Scalars(n)) => "read back a count",
    .Err(_e) => "gone",
  }
}

fn counted(r: Result<Said, Stopped>): Int {
  match (r) {
    .Ok(.Scalars(n)) => n,
    .Ok(.Text(s)) => s.len(),
    .Err(_e) => -1,
  }
}

// --- a struct inside a struct ------------------------------------------------

struct Label {
  name: Str,
  tags: [Str],
}

struct Record {
  label: Label,
  counts: [Int],
  note: Option<Str>,
}

enum Filing {
  File(Record),
  Look,
}

enum Filed {
  Was(Record),
}

fn cabinet<C: Alloc + Tasks>(initial: Record): Actor<C, Record, Filing, Filed> {
  Actor {
    state: initial,
    step: fn(c, held, message) => {
      match (message) {
        .File(next) => Stepped { state: next, answer: .Was(held) },
        .Look => Stepped { state: held, answer: .Was(held) },
      }
    },
  }
}

fn record(name: Str, note: Option<Str>): Record {
  Record {
    label: Label { name: name, tags: ["one", "two"] },
    counts: [7, 8, 9],
    note: note,
  }
}

fn shown<C: Alloc>(ctx: C, r: Result<Filed, Stopped>): Str {
  match (r) {
    .Ok(.Was(rec)) => {
      let note = match (rec.note) {
        .None => "-",
        .Some(text) => text,
      };
      str.format(
        ctx,
        "${rec.label.name}/${rec.label.tags.len()}/${rec.counts.len()}/${note}",
      )
    },
    .Err(_e) => "gone",
  }
}

// --- one actor's address as another actor's state ----------------------------

enum Tally {
  Increment,
  Get,
}

enum Tallied {
  Done,
  Count(Int),
}

fn tally<C: Alloc + Tasks>(): Actor<C, Int, Tally, Tallied> {
  Actor {
    state: 0,
    step: fn(c, count, message) => {
      match (message) {
        .Increment => Stepped { state: count + 1, answer: .Done },
        .Get => Stepped { state: count, answer: .Count(count) },
      }
    },
  }
}

enum Desk {
  Bump,
  Total,
}

enum Desked {
  Noted,
  Total(Int),
  Gone,
}

fn desk<C: Alloc + Tasks>(
  behind: Address<C, Int, Tally, Tallied>,
): Actor<C, Address<C, Int, Tally, Tallied>, Desk, Desked> {
  Actor {
    state: behind,
    step: fn(c, back, message) => {
      match (message) {
        .Bump => {
          match (back.sendMessage(c, .Increment)) {
            .Ok(_done) => Stepped { state: back, answer: .Noted },
            .Err(_gone) => Stepped { state: back, answer: .Gone },
          }
        },
        .Total => {
          match (back.sendMessage(c, .Get)) {
            .Ok(.Count(n)) => Stepped { state: back, answer: .Total(n) },
            .Ok(_other) => Stepped { state: back, answer: .Gone },
            .Err(_gone) => Stepped { state: back, answer: .Gone },
          }
        },
      }
    },
  }
}

fn front(r: Result<Desked, Stopped>): Str {
  match (r) {
    .Ok(.Total(_n)) => "a total",
    .Ok(.Noted) => "noted",
    .Ok(.Gone) => "gone",
    .Err(_e) => "stopped",
  }
}

fn totalled(r: Result<Desked, Stopped>): Int {
  match (r) {
    .Ok(.Total(n)) => n,
    .Ok(_other) => -1,
    .Err(_e) => -1,
  }
}

export fn main(): Result<(), Str> {
  let ctx = context {
    Alloc: host.alloc,
    Stdout: host.stdout,
    Tasks: host.tasks,
  };

  // A state and an answer that occupy no bytes at all, which is the only way a
  // program reaches `Carried`'s pad.
  let quiet = actor.start(ctx, silent());
  let _ = io.println(ctx, "unit ${answered(quiet.sendMessage(ctx, .Ping))}").ignore();
  let _ = quiet.stop(ctx).ignore();

  // A list in the message, in the state, and in the answer.
  let held = actor.start(ctx, bag());
  let _ = held.sendMessage(ctx, .Fill([1, 2, 3])).ignore();
  let _ = held.sendMessage(ctx, .Push(4)).ignore();
  let _ = io.println(ctx, "list ${drained(held.sendMessage(ctx, .Drain))}").ignore();
  let _ = io.println(ctx, "empty ${drained(held.sendMessage(ctx, .Drain))}").ignore();
  let _ = held.stop(ctx).ignore();

  // Text in four scripts, one of them outside the basic plane, and the empty
  // string at the other end of the range.
  let text = actor.start(ctx, scribe("mix日😀éd"));
  let _ = io.println(ctx, "text ${told(text.sendMessage(ctx, .Read))}").ignore();
  let wider = text.sendMessage(ctx, .Say("héllo wörld 🌍"));
  let _ = io.println(ctx, "wide ${counted(wider)}").ignore();
  let _ = io.println(ctx, "back ${told(text.sendMessage(ctx, .Read))}").ignore();
  let _ = io.println(ctx, "none ${counted(text.sendMessage(ctx, .Say("")))}").ignore();
  let _ = text.stop(ctx).ignore();

  // A struct two deep, with a list at each level and an `Option` beside them.
  let filed = actor.start(ctx, cabinet(record("first", .None)));
  let put = filed.sendMessage(ctx, .File(record("second", .Some("kept"))));
  let _ = io.println(ctx, "was ${shown(ctx, put)}").ignore();
  let _ = io.println(ctx, "now ${shown(ctx, filed.sendMessage(ctx, .Look))}").ignore();
  let _ = filed.stop(ctx).ignore();

  // An address is inert, so it is another actor's whole state. The last send
  // is made from inside a step, to an actor that has stopped.
  let counted = actor.start(ctx, tally());
  let clerk = actor.start(ctx, desk(counted));
  let _ = clerk.sendMessage(ctx, .Bump).ignore();
  let _ = clerk.sendMessage(ctx, .Bump).ignore();
  let _ = io.println(ctx, "desk ${totalled(clerk.sendMessage(ctx, .Total))}").ignore();
  let _ = counted.stop(ctx).ignore();
  let _ = io.println(ctx, "behind ${front(clerk.sendMessage(ctx, .Bump))}").ignore();
  let _ = clerk.stop(ctx).ignore();
  .Ok(())
}
"#,
        "unit yes\nquiet\nlist 4\nempty 0\ntext mix日😀éd\nwide 13\nback héllo wörld 🌍\n\
         none 0\nwas first/2/3/-\nnow second/2/3/kept\ndesk 2\nbehind gone\n",
    );
}

/// **An actor driven from inside a scope, on every backend** — the four values
/// `core/actor` hands the runtime are copies made outside every arena.
///
/// `cli/tests/conformance/lib/actor/test/scoped.buri` is this claim as a corpus
/// and is the fuller statement of it, but the corpus's native run is the stencil
/// backend alone. The claim is about *pages*: `core/alloc`'s `copyAcross` leaves
/// the carrier's arena before it copies, and each backend generates the copy walk
/// for the concrete `T` itself (`stencil/glue.rs`'s `Helper::Copy`,
/// `llvm/emit.rs`'s `Job::Copy`), so one green pipeline says nothing about the
/// other. Before the copy existed this program read memory `munmap` had taken
/// back, and the native halves died on a signal while JavaScript — which has no
/// arena — printed the right answer.
///
/// The string that crosses is 70 000 bytes for a reason the corpus file's header
/// gives in full: a released arena block of exactly one standard block goes back
/// to a **pool** rather than to the kernel, so only a bigger one is unmapped
/// outright. The eight small scopes are the other half — they are what hands the
/// pooled pages out again.
#[test]
fn an_actor_driven_inside_a_scope_keeps_its_values_on_every_backend() {
    rows_or_skip!();
    agree(
        "actor in a scope",
        r#"
from "core/actor" import * as actor;
from "core/actor" import { Actor, Address, Stepped, Stopped };
from "core/alloc" import * as alloc;
from "core/alloc" import { Scoped };
from "core/effect" import { Alloc, Stdout, Tasks };
from "core/host" import * as host;
from "core/io" import * as io;

enum Keep {
  Put(Str),
  Get,
}

enum Kept {
  Stored,
  Held(Str),
}

fn keeper<C: Alloc + Tasks>(initial: Str): Actor<C, Str, Keep, Kept> {
  Actor {
    state: initial,
    step: fn(c, held, message) => {
      match (message) {
        .Put(next) => Stepped { state: next, answer: .Stored },
        .Get => Stepped { state: held, answer: .Held(held) },
      }
    },
  }
}

/// A scope's own context and an address minted inside it, carried out together.
/// A lambda may not capture a context, so this pair is the only way a program
/// reaches an actor that was started inside a scope.
struct Escaped<C> {
  scope: Scoped<C>,
  address: Address<Scoped<C>, Str, Keep, Kept>,
}

/// Bigger than one arena block, so its mapping is unmapped rather than pooled.
fn big<C: Alloc>(ctx: C, unit: Str): Str {
  unit.repeat(ctx, 70000)
}

fn same(got: Result<Kept, Stopped>, want: Str): Str {
  match (got) {
    .Err(_e) => "stopped",
    .Ok(.Stored) => "different",
    .Ok(.Held(s)) => {
      match (s == want) {
        true => "same",
        false => "different",
      }
    },
  }
}

fn ended(r: Result<(), Stopped>): Str {
  match (r) {
    .Ok(_ok) => "ok",
    .Err(_e) => "stopped",
  }
}

export fn main(): Result<(), Str> {
  let ctx = context {
    Alloc: host.alloc,
    Stdout: host.stdout,
    Tasks: host.tasks,
  };

  // Three crossings inside one scope: the state `start` moves in, the message
  // `sendMessage` posts, and the state `drive` puts back after the step ran.
  let out = alloc.scoped(ctx, fn(c) => {
    let address = actor.start(c, keeper(big(c, "s")));
    let _ = address.sendMessage(c, .Put(big(c, "m"))).ignore();
    let _ = address.sendMessage(c, .Get).ignore();
    Escaped { scope: c, address: address }
  });

  // Scopes that map and release pages of their own. Small ones first: those
  // are the ones that draw a pooled block.
  let churned = [1, 2, 3, 4, 5, 6, 7, 8].mapCtx(ctx, fn(k, n) => {
    alloc.scoped(k, fn(s) => "z".repeat(s, 40 + n).len())
  });
  let large = alloc.scoped(ctx, fn(s) => "y".repeat(s, 70000).len());
  let _ = io.println(ctx, "churned ${churned.len()} ${large}").ignore();

  let answered = out.address.sendMessage(out.scope, .Get);
  let want = big(ctx, "m");
  let _ = io.println(ctx, "state ${same(answered, want)}").ignore();
  let _ = io.println(ctx, "stop ${ended(out.address.stop(out.scope))}").ignore();
  .Ok(())
}
"#,
        "churned 8 70000\nstate same\nstop ok\n",
    );
}

// -------------------------------------------------------------------
// Not rows: the memory-corruption family, one test per report
// -------------------------------------------------------------------
//
// Five reports against the stencil backend, every one of them a program that
// runs on JavaScript and aborts or answers wrongly natively, and every one of
// them about a value that holds or reaches heap contents. None is a §12 row:
// the table is about what the *language* says at a type, and nothing in it is
// about when a reference count goes down or which frame word a loop keeps its
// destination in. They are here for the reason the aggregate-projection test
// above is — the reference answer is the backend that cannot get a count
// wrong, so "the two disagree" is the sharpest statement of each defect.

/// A field projected off a **generic call's** result, let-bound, where the
/// field holds a list of an enum type. Issue #33.
///
/// `middle::inline` replaces `identity(outer())` with the callee's body, and a
/// body is a `Block` whose tail is the block's own binding. `middle::rc`
/// scanned a projection's base as a **borrow** whatever shape it was, so the
/// block released that binding on its way out — between the call and the field
/// read, with the field read then copying words out of a freed block.
/// `Scan::children` had promoted a compound child to `Mode::Own` for exactly
/// this reason since the shape was first met; a projection was the one
/// construct with a child that had not.
///
/// Three spellings, because the report's own bisection is that the `let` is
/// what decides: the inline projection was fine, an `[Int]` was fine, and the
/// let-bound `[Leaf]` aborted.
///
/// **The promotion is only half of an owned value, and the other half took a
/// second pass.** Owning the base means the projection increfs the field it
/// hands on and releases the base there, so what comes out is an owned
/// reference with **no name** — a temporary, and somebody has to drop it.
/// `rc::fresh` is the function that says which values those are, and it did
/// not count this one: it read a projection as a temporary only when the
/// *base* was a construction or a call, and a base the inliner turned into a
/// `Block` is neither. So `identity(outer()).inner` was increfed and never
/// released, and this row leaked exactly one block — the `[Leaf]` the middle
/// field of the chain holds — from the day the heap check was switched on
/// until `fresh` learned that a tail-shaped base makes a temporary too.
#[test]
fn a_projection_off_a_generic_calls_result_agrees() {
    rows_or_skip!();
    agree(
        "projection off a generic call",
        r#"
from "core/host" import { stdout, alloc };
from "core/io" import * as io;

enum Leaf { One(Int), Two(Int) }
struct Inner { items: [Leaf] }
struct Plain { items: [Int] }
struct Outer { inner: Inner, plain: Plain }

fn outer(): Outer {
  Outer {
    inner: Inner { items: [1, 2].map(alloc, fn(n) => Leaf.One(n)) },
    plain: Plain { items: [1, 2].map(alloc, fn(n) => n + 1) },
  }
}

fn identity<T>(value: T): T { value }

export fn main(): Result<(), Str> {
  let a = identity(outer()).inner.items.len();
  let plain = identity(outer()).plain;
  let inner = identity(outer()).inner;
  let _ = io.println(stdout, "${a} ${plain.items.len()} ${inner.items.len()}").ignore();
  .Ok(())
}
"#,
        "2 2 2\n",
    );
}

/// A `match` arm's payload bindings, where the arm also reads a **sibling
/// field of the scrutinee's base**. Issue #39.
///
/// `match (s.outcome)` binds words copied out of `s`'s block, with no count of
/// their own; the arm then reads `s.manager`, which is `s`'s last use, so the
/// drop landed there — freeing the block the arm's `flush` still pointed into
/// and handing it straight back to the next allocation in the same expression.
/// It was a wrong answer rather than a crash: `flush` came back holding `id`'s
/// payload.
///
/// The fix is `Scan::children`'s deferral one construct over — the scrutinee's
/// root is held live across the arms and dropped after them — and the second
/// spelling here is the workaround the report found, which has to go on
/// answering the same thing.
#[test]
fn a_match_arms_bindings_survive_a_sibling_field_read() {
    rows_or_skip!();
    agree(
        "match arm bindings",
        r#"
from "core/host" import { stdout, alloc };
from "core/io" import * as io;
from "core/str" import * as str;

struct Msg { body: Str }
enum Outcome {
  Attached { id: Str, status: Int, reply: Msg, flush: [Msg] },
  Refused { reply: Msg },
}
struct Manager { held: Int }
struct Step { manager: Manager, outcome: Outcome }

fn step(): Step {
  Step {
    manager: Manager { held: 1 },
    outcome: .Attached {
      id: "se".repeat(alloc, 2),
      status: 0,
      reply: Msg { body: "re".repeat(alloc, 2) },
      flush: [Msg { body: "fl".repeat(alloc, 2) }],
    },
  }
}

fn sent(manager: Manager, messages: [Msg]): Str {
  let bodies = messages.map(alloc, fn(message) => message.body).join(alloc, ",");
  str.format(alloc, "${bodies}/${manager.held}")
}

fn insideArm(): Str {
  let s = step();
  match (s.outcome) {
    .Attached { id, reply, flush, .. } => {
      sent(s.manager, [Msg { body: id }].concat(alloc, [reply].concat(alloc, flush)))
    },
    .Refused { reply } => sent(s.manager, [reply]),
  }
}

fn beforeMatch(): Str {
  let s = step();
  let manager = s.manager;
  match (s.outcome) {
    .Attached { id, reply, flush, .. } => {
      sent(manager, [Msg { body: id }].concat(alloc, [reply].concat(alloc, flush)))
    },
    .Refused { reply } => sent(manager, [reply]),
  }
}

export fn main(): Result<(), Str> {
  let _ = io.println(stdout, insideArm()).ignore();
  let _ = io.println(stdout, beforeMatch()).ignore();
  .Ok(())
}
"#,
        "sese,rere,flfl/1\nsese,rere,flfl/1\n",
    );
}

/// A mutually tail-recursive group whose members' parameters **do not agree
/// position by position**. Issue #29.
///
/// `tail_calls::merge_group` gave the merged function one slot per parameter
/// *position*, typed by the first member that had one there, on the reasoning
/// that "nothing reads a slot at another member's type". Two members that
/// disagree are what makes that false: `walkFrom(octets, Int, Walk)` and
/// `walkOne(octets, Int, U8, Walk)` put a `Walk` and a `U8` in one slot, so the
/// merged function compared a pointer against 255 and released it as two
/// different types. `shared_slots` allocates by type instead, and every
/// member's arguments are passed in slot order.
///
/// The second half of the same report is the `Loop`: a merged group's entries
/// own **disjoint prefixes** of the parameter list, so an entry must not
/// release a slot beyond its own arity — `lower::pad` hands it `undef`.
#[test]
fn mutual_tail_recursion_with_unlike_parameters_agrees() {
    rows_or_skip!();
    agree(
        "mutual tail recursion",
        r#"
from "core/host" import { stdout, alloc };
from "core/io" import * as io;
from "core/list" import * as list;
from "core/str" import * as str;

enum Fault { Incomplete, Corrupt }
struct Walk { seen: [Int], total: Int }

fn walk(octets: [U8], at: Int): Result<Int, Fault> {
  let walked = walkFrom(octets, at, Walk { seen: list.empty<Int>(), total: 0 })?;
  .Ok(walked.total + walked.seen.len())
}

fn walkFrom(octets: [U8], at: Int, state: Walk): Result<Walk, Fault> {
  match (octets[at]) {
    .None => .Ok(state),
    .Some(octet) => walkOne(octets, at, octet, state),
  }
}

fn walkOne(octets: [U8], at: Int, octet: U8, state: Walk): Result<Walk, Fault> {
  if (octet == 255) {
    .Err(.Corrupt)
  } else {
    walkFrom(octets, at + 1, Walk {
      seen: state.seen.push(alloc, octet.toI64()),
      total: state.total + octet.toI64(),
    })
  }
}

fn shown(answer: Result<Int, Fault>): Str {
  match (answer) {
    .Ok(n) => str.format(alloc, "${n}"),
    .Err(.Corrupt) => "corrupt",
    .Err(.Incomplete) => "incomplete",
  }
}

export fn main(): Result<(), Str> {
  let _ = io.println(stdout, shown(walk([1, 2, 3], 0))).ignore();
  let _ = io.println(stdout, shown(walk([1, 255, 3], 0))).ignore();
  let _ = io.println(stdout, shown(walk(list.empty<U8>(), 0))).ignore();
  .Ok(())
}
"#,
        "9\ncorrupt\n0\n",
    );
}

/// An `Option` whose payload **holds an array**, read out through
/// `withDefault` and through a `match`. Issue #28.
///
/// The report was written against `??`, which the language has since dropped;
/// `x.withDefault(y)` is what replaced it and is what the shape is pinned at
/// here, beside the `match` the operator was defined as. The payload *holding*
/// an array rather than being one is the other half of the report — an
/// `Option<Wrapper>` where `Wrapper` holds a `[U8]` — because that is the case
/// where the count belongs to something the option does not name.
///
/// **This one passes on the compiler that had the defect**, and is here
/// anyway. The construct that aborted no longer exists to be tested, so what
/// is left of the report is the claim underneath it — an option's payload is
/// read out at every payload type, on every backend — and a shape nothing
/// pins is a shape the next rewrite of `withDefault` is free to break.
///
/// Keeping it is what caught a second defect. `wrapped` leaked one block per
/// `.Some` and it was never `withDefault`'s fault:
/// `held.withDefault(w).octets` is a projection off a call the inliner pasted
/// in, which is [`a_projection_off_a_generic_calls_result_agrees`] above, at a
/// different type and reached through the standard library rather than a
/// `let`. One change to `rc::fresh` moved both rows, and the `.None` line —
/// the same missing release, costing no block because an empty list is not
/// one — is still here to say the two backends agree about that too.
#[test]
fn an_option_whose_payload_holds_an_array_agrees() {
    rows_or_skip!();
    agree(
        "option holding an array",
        r#"
from "core/host" import { stdout, alloc };
from "core/io" import * as io;
from "core/list" import * as list;

struct Wrapper { octets: [U8] }

fn defaulted(held: Option<[U8]>): Int { held.withDefault(list.empty<U8>()).len() }

fn matched(held: Option<[U8]>): Int {
  match (held) { .None => 0, .Some(raw) => raw.len() }
}

fn wrapped(held: Option<Wrapper>): Int {
  held.withDefault(Wrapper { octets: list.empty<U8>() }).octets.len()
}

fn wrappedMatch(held: Option<Wrapper>): Int {
  match (held) { .None => 0, .Some(w) => w.octets.len() }
}

fn built(): [U8] { [1, 2, 3].map(alloc, fn(n) => n.wrapToU8()) }

export fn main(): Result<(), Str> {
  let a = defaulted(.Some(built()));
  let b = matched(.Some(built()));
  let c = wrapped(.Some(Wrapper { octets: built() }));
  let d = wrappedMatch(.Some(Wrapper { octets: built() }));
  let e = defaulted(.None);
  let f = wrapped(.None);
  let _ = io.println(stdout, "${a} ${b} ${c} ${d} ${e} ${f}").ignore();
  .Ok(())
}
"#,
        "3 3 3 3 0 0\n",
    );
}

/// `list.sortBy` over a list whose **element type holds an enum**. Issue #41.
///
/// The stencil backend open-codes the merge sort at the call site and keeps
/// its indices — including the destination block's pointer — in the frame's
/// scratch words. `lists.rs::LOOP_SCRATCH` said where those begin, as a
/// number, and the number was two words inside the run `rtcall.rs` reserves
/// for the emitter itself. The word that collided is the one
/// `emit::walk_deep` writes an address into when a value's reference walk goes
/// **out of line**, which is what retaining an element whose type holds an
/// enum does — so the sort lost its destination between reading an element and
/// storing it, and answered the block `elemalloc` had just handed it: zeros.
///
/// That is why the reported symptoms were a scan answering empty strings and
/// then `no arm of this match applied`: a zeroed block read at an enum type
/// has a tag nothing wrote. `LOOP_SCRATCH` is derived from
/// `rtcall::RESERVED_WORDS` now, so the two cannot drift apart again.
///
/// Sorted at three lengths, because the merge's passes alternate between the
/// result block and a scratch one: one element takes no pass at all, three
/// take an odd number of them and four an even.
#[test]
fn sorting_a_list_whose_element_holds_an_enum_agrees() {
    rows_or_skip!();
    agree(
        "sortBy over a counted element",
        r#"
from "core/host" import { stdout, alloc };
from "core/io" import * as io;
from "core/list" import * as list;
from "core/str" import * as str;

struct Body { text: Str }
enum Held { Text(Body), Number(Int) }
struct Cell { held: Held }
struct Row { name: Str, cell: Cell, rank: Int }

fn rows(count: Int): [Row] {
  list.range(alloc, 0, count).map(alloc, fn(n) => Row {
    name: "r".repeat(alloc, n + 1),
    cell: Cell { held: Held.Text(Body { text: "v".repeat(alloc, n + 1) }) },
    rank: count - n,
  })
}

fn label(row: Row): Str {
  let inner = match (row.cell.held) {
    .Text(body) => body.text,
    .Number(n) => str.format(alloc, "${n}"),
  };
  str.format(alloc, "${row.name}=${inner}:${row.rank}")
}

fn shown(xs: [Row]): Str { xs.map(alloc, fn(row) => label(row)).join(alloc, " ") }

fn byRank(xs: [Row]): [Row] { xs.sortBy(alloc, fn(a, b) => a.rank.compare(b.rank)) }

export fn main(): Result<(), Str> {
  let _ = io.println(stdout, shown(byRank(rows(1)))).ignore();
  let _ = io.println(stdout, shown(byRank(rows(3)))).ignore();
  let _ = io.println(stdout, shown(byRank(rows(4)))).ignore();
  let _ = io.println(stdout, shown(rows(3))).ignore();
  .Ok(())
}
"#,
        "r=v:1\nrrr=vvv:1 rr=vv:2 r=v:3\nrrrr=vvvv:1 rrr=vvv:2 rr=vv:3 r=v:4\nr=v:3 rr=vv:2 rrr=vvv:1\n",
    );
}

/// **A WebSocket client refuses a scheme it cannot speak, the same way on every
/// backend.**
///
/// The claim is about a `ServeError` **crossing**, and it is a different
/// crossing on each side. Natively the struct comes back through an
/// out-pointer of its own (`cli/runtime/lib.rs` §2.1's second shape) and each
/// backend decides where the `Str` sits inside it; on JavaScript it is an array
/// the runtime built. So one green pipeline says nothing at all about the
/// other, which is what this file is for.
///
/// A refusal rather than a socket, because a row here compiles and runs one
/// program under three pipelines with no network anywhere near it. A URL whose
/// scheme is not `ws://` or `wss://` is the one answer every implementation of
/// this effect can give without dialling anything, and it is the answer
/// `core/net/websocket` documents.
///
/// **The cause and the sentence, and deliberately not `detail`.** `errorText`
/// is a constant per variant, so it is the same string everywhere; `detail` is
/// the platform's own words about what it was handed, and the native client and
/// the browser's `WebSocket` are not obliged to phrase that alike. Asserting it
/// would be asserting that two platforms wrote the same sentence.
///
/// The two silent lines are half the row: neither hook prints, because a socket
/// that never opened runs none of them.
#[test]
fn a_websocket_client_refuses_a_scheme_it_cannot_speak_on_every_backend() {
    rows_or_skip!();
    agree(
        "websocket client refusal",
        r#"
from "core/effect" import {
  Alloc, ServeError, ServeFailure, Sockets, Stdout, WebSocketClient,
};
from "core/host" import * as host;
from "core/io" import * as io;
from "core/net/server" import { CloseReason };
from "core/net/websocket" import * as websocket;
from "core/net/websocket" import { Client };

/// Hooks that would announce themselves if they ran. None of them does.
fn dialling<C: Sockets + Stdout + WebSocketClient>(url: Str): Client<C, Int> {
  Client {
    url: url,
    onOpen: fn(c, _socket, _response) => {
      let _said = io.println(c, "opened").ignore();
      0
    },
    onMessage: fn(_c, _socket, seen, _message) => seen + 1,
    onClose: fn(c, _socket, _seen, _reason) => io.println(c, "closed").ignore(),
  }
}

fn sentence(r: Result<CloseReason, ServeError>): Str {
  match (r) {
    .Ok(_reason) => "a socket opened",
    .Err(e) => websocket.errorText(e),
  }
}

fn cause(r: Result<CloseReason, ServeError>): ServeFailure {
  match (r) {
    .Ok(_reason) => .Closed,
    .Err(e) => e.cause,
  }
}

export fn main(): Result<(), Str> {
  let ctx = context {
    Alloc: host.alloc,
    Sockets: host.sockets,
    Stdout: host.stdout,
    WebSocketClient: host.websocketClient,
  };
  let dialled = websocket.connect(ctx, dialling("http://example.test/socket"));
  let _ = io.println(ctx, "cause ${cause(dialled)}").ignore();
  let _ = io.println(ctx, "text ${sentence(dialled)}").ignore();
  .Ok(())
}
"#,
        "cause .Unsupported\ntext the protocol is not supported by this toolchain\n",
    );
}

/// **A closure does not spend what it captures, and four ordinary reads are
/// not second references — on every backend.**
///
/// Both halves of `middle::rc::sharing`, in one program, and the row is written
/// as a differential because the backend that had the bugs is the reference
/// one. A capture read as a last use let `xs.slice(c, 0, i)` truncate the
/// captured list **in place** on the first call, so a mapping that answers
/// `0, 1, 2` answered `0, 0, 0`; the natives never had it, because
/// `middle::closures` lifts a body into a function that reads a capture out of
/// an environment. The four reads below are the same shape the other way up —
/// each one used to be counted as a second reference, each one now is not, and
/// a rule that reaches one step too far writes through a list somebody still
/// holds. That is a wrong answer rather than a slow program, and it is a wrong
/// answer on JavaScript only.
///
/// `conformance/lib/memory/test/captures.buri` and `data/test/lists.buri`'s
/// "what is not a second reference" section are the same claims as a corpus,
/// and the corpus's native run is the stencil backend alone. This is where the
/// optimizing one answers.
///
/// Every list is grown by the program: a literal lives in the artifact's
/// constant pool (VALUE-MODEL.md §5.2) and can never be the unique owner an
/// in-place write is looking for.
#[test]
fn a_closure_does_not_spend_what_it_captures_on_every_backend() {
    rows_or_skip!();
    agree(
        "sharing rules",
        r#"
from "core/host" import { alloc, stdout };
from "core/io" import * as io;
from "core/list" import * as list;
from "core/str" import * as str;

struct Out { pieces: [Int], at: Int }
struct Swapped { at: Int, pieces: [Int] }
struct Held<C> { run: fn(C, Int) => Int }

fn grown(n: Int): [Int] {
  list.range(alloc, 0, n).foldCtx(alloc, fn(c, acc: [Int], i) => acc.push(c, i), list.empty())
}

/// One piece written: a push into a list beside a read of an `Int` out of the
/// same record.
fn wrote(out: Out, x: Int): Out {
  Out { ..out, pieces: out.pieces.push(alloc, x), at: out.at + 1 }
}

/// The same two fields, declared the other way round. Moving them past each
/// other used to decide whether the push copied.
fn wroteSwapped(out: Swapped, x: Int): Swapped {
  Swapped { ..out, at: out.at + 1, pieces: out.pieces.push(alloc, x) }
}

/// A walk whose **tail is a projection of a counted value**: the accumulator is
/// a `(Out, Bool)` and what comes back is its first element.
fn writing(i: Int, n: Int, acc: (Out, Bool)): Out {
  if (i >= n) { acc.0 } else { writing(i + 1, n, (wrote(acc.0, i), true)) }
}

fn shown(xs: [Int]): Str { str.format(alloc, "${xs.len()}:${xs.sum()}") }

export fn main(): Result<(), Str> {
  // One closure, called four times, over a list the program grew.
  let xs = grown(3);
  let sliced = list.range(alloc, 0, 4).mapCtx(alloc, fn(c, i) => xs.slice(c, 0, i).len());
  let kept = Held { run: fn(c, i) => xs.drop(c, i).len() };
  let call = kept.run;
  let _ = io.println(stdout, "captured ${shown(sliced)} ${call(alloc, 1)} ${shown(xs)}").ignore();

  // A push beside an uncounted field of the same record, both ways round.
  let base = wrote(Out { pieces: list.empty<Int>(), at: 0 }, 1);
  let one = wrote(base, 2);
  let two = wrote(base, 3);
  let other = wroteSwapped(Swapped { at: 0, pieces: list.empty<Int>() }, 1);
  let after = wroteSwapped(other, 2);
  let _ = io.println(
    stdout,
    "beside ${shown(one.pieces)} ${shown(two.pieces)} ${shown(base.pieces)} ${shown(after.pieces)} ${shown(other.pieces)}",
  ).ignore();

  // A counted tail projection, a projection out of a nameless temporary, and a
  // fold seed two folds are handed.
  let walked = writing(0, 4, (Out { pieces: list.empty<Int>(), at: 0 }, false));
  let nameless = list
    .range(alloc, 0, 3)
    .foldCtx(alloc, fn(c, acc: (Out, Bool), i) => (wrote(acc.0, i), true), (Out { pieces: list.empty<Int>(), at: 0 }, false))
    .0;
  let seed = grown(1);
  let left = list.range(alloc, 0, 2).foldCtx(alloc, fn(c, acc: [Int], i) => acc.push(c, i), seed);
  let right = list.range(alloc, 0, 3).foldCtx(alloc, fn(c, acc: [Int], i) => acc.push(c, i), seed);
  let _ = io.println(
    stdout,
    "tails ${shown(walked.pieces)} ${walked.at} ${shown(nameless.pieces)} ${shown(left)} ${shown(right)} ${shown(seed)}",
  ).ignore();
  .Ok(())
}
"#,
        "captured 4:6 2 3:3\nbeside 2:3 2:4 1:1 2:3 1:1\ntails 4:6 4 3:3 3:1 4:3 1:0\n",
    );
}

/// **A `let` gives back every position its pattern skips, on every backend.**
///
/// `middle::rc` releases a value through a *name*, and a destructuring `let`
/// used to leave the positions its pattern skipped with neither: `let (q, _) =
/// nextToken(ctx, p)` gave element 0 to `q` and element 1 to nobody. Thirteen
/// blocks of `proto_schema/refusals.buri` leaked for it.
///
/// Nothing about the *answers* moves either way, which is why this row is worth
/// having beyond `conformance/lib/memory/test/discards.buri`: what it adds is
/// the exit audit under a second native backend. Every row here runs under
/// `BURI_RT_HEAP_CHECK` (`agree`), so a skipped position nobody freed fails
/// here as a leak and an over-release fails as a use-after-free — and this is
/// the only place the optimizing backend is asked.
///
/// The four shapes are the whole surface: a `_` at depth, a field a `..` stands
/// for, a name nobody reads, and a `match` arm's payload — which goes back with
/// its scrutinee instead, and must not be named twice for it.
#[test]
fn a_let_gives_back_every_position_its_pattern_skips_on_every_backend() {
    rows_or_skip!();
    agree(
        "skipped pattern positions",
        r#"
from "core/host" import { alloc, stdout };
from "core/io" import * as io;
from "core/list" import * as list;

struct Nest { tag: Int, both: (Str, [Int]) }
enum Deep { Only(Nest) }
struct Pair { kept: Int, dropped: Str }

/// Grown rather than interned: a constant is in no block anybody gives back.
fn grown(word: Str): Str { word.concat(alloc, "!") }

fn deep(word: Str): Deep {
  .Only(Nest { tag: 1, both: (grown(word), list.range(alloc, 0, 2)) })
}

/// One round of every shape that skips a counted position.
fn skipping(i: Int, n: Int, acc: Int): Int {
  if (i >= n) {
    acc
  } else {
    // Three levels down, under two positions that do bind.
    let .Only(Nest { tag, both: (_, _) }) = deep("token");
    // A struct field written as `_`.
    let Pair { kept, dropped: _ } = Pair { kept: 2, dropped: grown("word") };
    // A struct field the pattern never wrote down at all.
    let Nest { tag: shallow, .. } = Nest { tag: 4, both: (grown("rest"), list.range(alloc, 0, 3)) };
    // A name nobody reads, which is the other half of the rule.
    let (at, _unread) = (8, grown("named"));
    skipping(i + 1, n, acc + tag + kept + shallow + at)
  }
}

/// A `match` arm's unbound payload, which goes back with the scrutinee.
fn arm(word: Str): Int {
  match (deep(word)) {
    .Only(Nest { tag, both: (_, _) }) => tag,
  }
}

export fn main(): Result<(), Str> {
  let _ = io.println(stdout, "skipped ${skipping(0, 8, 0)}").ignore();
  let _ = io.println(stdout, "arm ${arm("held")}").ignore();
  // And not twice: the destructuring is not the last read of `whole`.
  let whole = deep("kept");
  let .Only(Nest { tag, both: (_, rest) }) = whole;
  let again = match (whole) {
    .Only(nest) => nest.tag,
  };
  let _ = io.println(stdout, "twice ${tag} ${rest.len()} ${again}").ignore();
  .Ok(())
}
"#,
        "skipped 120\narm 1\ntwice 1 2 1\n",
    );
}

/// **A recursive type round-trips on every backend** — a boxed field and a
/// boxed variant payload, built, read back and released.
///
/// `middle::layout` puts a *pointer* where a field that would make its owner
/// recursive would be (VALUE-MODEL.md §5.2), so the field's slots are one
/// pointer and the value stored into it has its own. The optimizing backend
/// used to assemble a two-slot register out of that one pointer, which its own
/// IR verifier caught the first time a program built one —
/// `native::llvm::a_boxed_field_round_trips` is the smallest case, and this is
/// the same shape with the two things that make it a *memory* claim rather than
/// a layout one: counted leaves inside the box, and enough of them to be worth
/// counting.
///
/// Four shapes, because a box is reached four ways: a variant payload that is a
/// struct with a boxed field, a struct with **two** boxed fields, an `Option` of
/// a recursive type — the niche and the box in one value — and a tuple carrying
/// one that is read twice. Every one carries a `Str` or a `[Int]` under the
/// box, so a box the backend built and never released fails here as a leak
/// rather than in whatever program happens to build one next.
#[test]
fn a_recursive_type_round_trips_on_every_backend() {
    rows_or_skip!();
    agree(
        "boxed fields and payloads",
        r#"
from "core/host" import { alloc, stdout };
from "core/io" import * as io;
from "core/list" import * as list;
from "core/str" import * as str;

enum Chain { End, Link(Cell) }
struct Cell { next: Chain, value: Int, label: Str }

enum Tree { Tip, Branch(Fork) }
struct Fork { left: Tree, right: Tree, mark: [Int] }

fn total(c: Chain): Int {
  match (c) {
    .End => 0,
    .Link(cell) => cell.value + total(cell.next),
  }
}

fn labels(c: Chain, acc: Str): Str {
  match (c) {
    .End => acc,
    .Link(cell) => labels(cell.next, acc.concat(alloc, cell.label)),
  }
}

fn built(i: Int, n: Int, acc: Chain): Chain {
  if (i >= n) {
    acc
  } else {
    built(i + 1, n, .Link(Cell { next: acc, value: i, label: str.fromInt(alloc, i) }))
  }
}

fn size(t: Tree): Int {
  match (t) {
    .Tip => 0,
    .Branch(f) => 1 + size(f.left) + size(f.right) + f.mark.len(),
  }
}

fn tree(depth: Int): Tree {
  if (depth <= 0) {
    .Tip
  } else {
    .Branch(Fork { left: tree(depth - 1), right: tree(depth - 1), mark: list.range(alloc, 0, depth) })
  }
}

fn held(o: Option<Chain>): Int {
  match (o) {
    .Some(c) => total(c),
    .None => -1,
  }
}

export fn main(): Result<(), Str> {
  let chain = built(0, 8, .End);
  let written = labels(chain, "");
  let _ = io.println(stdout, "chain ${total(chain)} ${written}").ignore();
  let _ = io.println(stdout, "tree ${size(tree(3))}").ignore();
  let _ = io.println(stdout, "held ${held(.Some(built(0, 4, .End)))} ${held(.None)}").ignore();
  let pair = (built(0, 3, .End), 5);
  let _ = io.println(stdout, "pair ${total(pair.0)} ${total(pair.0)} ${pair.1}").ignore();
  .Ok(())
}
"#,
        "chain 28 76543210\ntree 18\nheld 6 -1\npair 3 3 5\n",
    );
}

/// `every_conformance_file_is_accounted_for` has to its own list, and it
/// needs no backend, so it runs on every host.
#[test]
fn every_row_of_the_table_names_a_test_that_exists() {
    let doc = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("the repository root is above `cli/`")
            .join("design/native/VALUE-MODEL.md"),
    )
    .expect("design/native/VALUE-MODEL.md");
    let me = include_str!("agreement.rs");

    let mut rows = 0usize;
    for line in doc.lines() {
        let cells: Vec<&str> = line.trim().trim_matches('|').split('|').collect();
        let [number, .., pinned] = cells.as_slice() else { continue };
        if number.trim().parse::<u32>().is_err() {
            continue;
        }
        rows += 1;
        let names: Vec<&str> = pinned
            .split(',')
            .map(|n| n.trim().trim_matches('`'))
            .filter(|n| !n.is_empty())
            .collect();
        assert!(!names.is_empty(), "§12 row {} names no test", number.trim());
        for name in names {
            assert!(
                me.contains(&format!("fn {name}(")),
                "§12 row {} is pinned by `{name}`, and there is no such test in \
                     `cli/tests/native/agreement.rs`",
                number.trim()
            );
        }
    }
    // A table this failed to find would "pass" having checked nothing,
    // which is the failure a self-checking document has.
    assert_eq!(rows, 17, "§12 has {rows} numbered rows rather than seventeen");
}
