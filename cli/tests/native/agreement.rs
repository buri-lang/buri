//! The rows of `design/native/VALUE-MODEL.md` §12, as tests.
//!
//! §12 is a table of every way the JavaScript backend and a native backend
//! could differ, and every row is either **must agree** or is a **documented
//! divergence**. Until this file existed the table was a claim: nothing
//! compiled one program through both pipelines and compared the bytes, so a
//! row that quietly stopped being true stayed in the document as a sentence.
//! `design/TODO.md`'s native section said so in as many words — "that test does not
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
use std::process::{Command, Stdio};
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
    let configured = std::env::var("BURI_JS").ok();
    let candidates: Vec<String> = match configured {
        Some(js) => vec![js],
        None => vec![String::from("bun"), String::from("node")],
    };
    candidates.into_iter().find(|candidate| {
        Command::new(candidate)
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    })
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

fn cc() -> String {
    std::env::var("CC").unwrap_or_else(|_| String::from("cc"))
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
    let mut link = Command::new(cc());
    link.arg("-o").arg(&binary);
    for object in &objects {
        link.arg(object);
    }
    link.arg(archive());
    if cfg!(target_os = "linux") {
        link.args(["-lpthread", "-ldl", "-lm"]);
    }
    let linked = link.output().unwrap();
    assert!(
        linked.status.success(),
        "{row}: the `{}` link failed:\n{}",
        native.name,
        String::from_utf8_lossy(&linked.stderr)
    );
    let out = Command::new(&binary).output().unwrap();
    Some(Ran {
        status: out.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&out.stdout).to_string(),
        stderr: String::from_utf8_lossy(&out.stderr).to_string(),
    })
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

/// A **must agree** row: every backend prints `expected`, exits zero, and
/// says nothing on standard error.
///
/// `expected` is asserted as well as agreement, because two backends that
/// agree on the wrong answer agree.
fn agree(row: &str, source: &str, expected: &str) {
    let (js, natives) = both(row, source);
    assert_eq!(js.stderr, "", "{row}: JavaScript printed to standard error");
    assert_eq!(js.status, 0, "{row}: JavaScript exited {}", js.status);
    assert_eq!(js.stdout, expected, "{row}: JavaScript printed something else");
    for (name, ran) in &natives {
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
/// different carried value, and neither is emitted. What this test is really
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
from "core/host" import { stdout, alloc };
from "core/io" import * as io;

export fn main(): Result<(), Str> {
  let r = alloc.allocate(64);
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
///  * **one effect out of the context** — `c.nowMillis()` inside a step. The
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

/// **A handler a wrapper rebuilt is entered, and is handed the wrapper.**
///
/// `core/alloc`'s `Scoped<C>` cannot forward `Listen` the way it forwards the
/// other twelve effects: `listen` hands its handler `Self`, which is
/// `Scoped<C>` at the wrapper and the acceptor inside it, so the wrapper has to
/// *rebuild* the handler — put the scope back around whatever the acceptor
/// hands out. Wave-8 G4 found that shape faulting under the stencil backend
/// with SIGBUS before a line was flushed, and narrowed it with three flips: a
/// zero-sized implementation passes, an acceptor that refuses passes, and the
/// same wrapper reached without the generic callback passes.
///
/// The program below is that repro with `core/alloc` taken out of it — every
/// implementation is written here, so what is being pinned is the language
/// shape and not one module's wrapper. `Plain` carries a word for the first
/// flip's sake: a context whose bindings are all zero-sized is zero words wide
/// and `Wrap<ctx>` and `Wrap<OneShot>` are then the same bytes, which is what
/// hid this.
#[test]
fn a_handler_a_wrapper_rebuilt_is_entered_on_every_backend() {
    rows_or_skip!();
    agree(
        "rebuilt handler",
        r#"
from "core/effect" import {
  Alloc, IoError, Listen, NetError, Region, Request, Response, Stdout,
};
from "core/host" import * as host;
from "core/net/server" import * as server;

/// An `Alloc` that is not zero-sized, so the context binding it is a word wide.
struct Plain {
  n: I64,
}

impl Alloc for Plain {
  fn allocate(self, bytes: Int): Region { Region(bytes + self.n) }
}

/// An acceptor that calls its handler once, with a request naming the address.
struct OneShot {
  bindsTo: Str,
}

impl Listen for OneShot {
  fn listen(
    self,
    address: Str,
    port: Int,
    onRequest: fn(OneShot, Request) => Response,
  ): Result<(), NetError> {
    match (self.bindsTo) {
      "" => .Err(.Refused),
      _ => {
        let reply = onRequest(self, Request {
          method: .Get,
          url: address,
          headers: [],
          body: [],
        });
        match (reply.status) { 200 => .Ok(()), _ => .Err(.Refused) }
      },
    }
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

impl<C: Listen> Listen for Wrap<C> {
  fn listen(
    self,
    address: Str,
    port: Int,
    onRequest: fn(Wrap<C>, Request) => Response,
  ): Result<(), NetError> {
    let tag = self.1;
    self.0.listen(address, port, fn(c, request) => onRequest(Wrap(c, tag), request))
  }
}

/// A handler written against a bound, which is what a request handler is.
fn served<C: Listen + Stdout>(ctx: C): Int {
  let answered = server.listen(ctx, "10.0.0.1", 0, fn(server, request) => {
    // The handler *uses* what it is handed, on a bound its own `C` declares
    // and the acceptor does not. A handler that ignored its first parameter
    // would pass whatever arrived.
    let _ = server.println("handler on ${request.url}");
    Response { status: 200, headers: [], body: [] }
  });
  match (answered) { .Ok(_) => 1, .Err(_) => 0 }
}

/// The generic callback `scoped` is: it builds the wrapper and hands it over.
fn wrapped<C, T>(ctx: C, body: fn(Wrap<C>) => T): T {
  body(Wrap(ctx, 7))
}

export fn main(): Result<(), Str> {
  let ctx = context {
    Alloc: Plain { n: 0 },
    Stdout: host.stdout,
    Listen: OneShot { bindsTo: "10.0.0.1" },
  };
  let _ = host.stdout.println("entering");
  let n = wrapped(ctx, fn(c) => served(c));
  let _ = host.stdout.println("served ${n}");
  .Ok(())
}
"#,
        "entering\nhandler on 10.0.0.1\nserved 1\n",
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

/// A task that aborts stops the program, with the same message and the same
/// status on every backend — and with what was printed before it flushed.
///
/// An abort is a write to standard error and an exit, never an unwind (SPEC
/// 6.10), so there is nothing for the trampoline to do about one and that is
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

// -------------------------------------------------------------------
// The table itself
// -------------------------------------------------------------------

/// Every row of §12 names a test in this file, and that test exists.
///
/// The table is the document this file holds up, so a row added next door
/// with no test — or a test renamed out from under a row — is a failure
/// here rather than a "pinned by" column that has quietly stopped being
/// true. The same relationship `native/conformance.rs`'s
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
    assert_eq!(rows, 16, "§12 has {rows} numbered rows rather than sixteen");
}
