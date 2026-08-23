//! The fourteen rows of `design/native/VALUE-MODEL.md` §12, as tests.
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
//!    `design/native/OPEN-QUESTIONS.md` is that `Checked` is bounded by the
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
//!    step on, panicked inside Cranelift. Pinned by
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
//! failure says `cranelift` or `llvm`. Cranelift comes from `backend::select`
//! at `Profile::Debug`, which is the selection a native debug build makes. LLVM
//! is constructed directly, because `select` has no native `Profile::Release`
//! arm yet — the fallback in [`Native::backend`] is written so that the `Ok`
//! arm takes over the day that arm lands, rather than shadowing it.
//!
//! `cargo test -p buri --features backend-llvm --test native agreement::` is the
//! second half, and it runs: LLVM 21 compiles most of the rows and refuses the
//! rest for reasons of its own — `num.minValue`/`num.maxValue` have no body —
//! so it carries a
//! [`Native::partial`] note and a row it cannot compile is skipped with the
//! reason printed. Cranelift carries no such note, so a refusal there is a
//! failure. Where both compile a row they have never disagreed.
//!
//! With `--no-default-features` there is no native backend, and `main.rs`
//! does not declare this module at all. With one but no runtime archive,
//! no `cc`, or no
//! JavaScript engine, every test returns early with a printed reason:
//! `native_ready` is the same gate `buri build` uses.
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

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::string_slice,
    clippy::arithmetic_side_effects,
    clippy::print_stdout,
    clippy::print_stderr,
    reason = "test code, as in `tests/harness/mod.rs`: the lint set in \
              `Cargo.toml` pins a promise about the toolchain, and a harness \
              that drives the toolchain is not the toolchain."
)]

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
    /// Only ever set for a backend `backend::select` does not answer for a
    /// native *debug* build, which is the selection a user gets by default:
    /// a refusal from Cranelift is this file's failure, and that is what
    /// keeps the tolerance from becoming a place for rows to go and die.
    partial: Option<&'static str>,
}

/// Never empty: the module is behind `any(backend-cranelift,
/// backend-llvm)`, so at least one arm below is compiled in.
const NATIVES: &[Native] = &[
    #[cfg(feature = "backend-cranelift")]
    Native { name: "cranelift", profile: Profile::Debug, partial: None },
    #[cfg(feature = "backend-cpjit")]
    Native {
        name: "cpjit",
        profile: Profile::Debug,
        partial: Some(
            "wave 1 of the copy-and-patch productization, and its surface is \
             narrower than Cranelift's: drop glue, `Ret::Res`, `Ret::Tag` and \
             a `[T]` whose element carries a reference count are all refused \
             rather than emitted wrongly (`backend/cpjit/mod.rs`'s header). A \
             row it refuses is skipped here; the gate on taking Cranelift's \
             seat is that this list is empty",
        ),
    },
    #[cfg(feature = "backend-llvm")]
    Native {
        name: "llvm",
        profile: Profile::Release,
        partial: Some(
            "wave 2b, and its surface is narrower than Cranelift's: \
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
    /// The backend, through `backend::select` where `select` answers.
    ///
    /// It answers for `(native, Debug)` and refuses `(native, Release)`:
    /// the release arm still returns the "arrives with `backend-llvm`"
    /// diagnostic. So the release fallback is spelled out here, gated on
    /// the feature that carries the backend, and the `Ok` arm takes over
    /// the day `select` grows the arm.
    fn backend(self) -> Box<dyn Backend> {
        // `select` still sends every native debug build to Cranelift, which is
        // wave 1's deliberate non-change, so this backend is the one row here
        // that is named rather than selected.
        #[cfg(feature = "backend-cpjit")]
        if self.name == "cpjit" {
            return Box::new(backend::cpjit::Cpjit);
        }
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

/// Why this host cannot answer a row, or `None`.
///
/// `native_ready` is the build system's own three questions — a backend
/// compiled in, a runtime archive built, a linker present — asked at
/// `Debug` because what it is really asking about is the host, and the
/// release arm is exactly the one `select` still refuses.
fn skip_reason() -> Option<String> {
    if !actions::native_ready(host_target(), Profile::Debug) {
        return Some(String::from("`native_ready` is false on this host"));
    }
    if engine().is_none() {
        return Some(String::from("no JavaScript engine on PATH"));
    }
    None
}

/// The skip guard every row test opens with.
macro_rules! rows_or_skip {
    () => {
        if let Some(why) = skip_reason() {
            eprintln!("backend agreement: skipped ({why})");
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
        !analysis.diags.has_errors(),
        "{row}: the program does not compile:\n{}",
        analysis.diags.items.iter().map(|d| map.render(d, false)).collect::<Vec<_>>().join("\n")
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
    let mut diags = Diagnostics::new();
    let mut program =
        monomorphize::run(checked, paths.to_vec(), &mut diags, monomorphize::Roots::Main(entry));
    assert!(!diags.has_errors(), "{row}: monomorphization failed");
    // The product's own seam: `middle::run` for everybody, `middle::native`
    // for the platforms that are not JavaScript.
    actions::prepare(&mut program, target);
    program
}

fn cc() -> String {
    std::env::var("CC").unwrap_or_else(|_| String::from("cc"))
}

fn messages(diags: &Diagnostics) -> String {
    diags.items.iter().map(|d| d.message.clone()).collect::<Vec<_>>().join("; ")
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
/// on the trait and it is what `host.HostAlloc.allocate` trips. But a
/// structural operation is an `ir::Inst::Structural`, which exists only
/// after lowering and is therefore not in the program that hook is handed
/// (`llvm/mod.rs` says so where the hook is implemented), so
/// a `deriveArray*` and `derivePrimJson` can only be discovered by asking the
/// backend to emit and reading the diagnostic.
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
    let natives = NATIVES
        .iter()
        .filter_map(|n| run_native(row, *n, &checked, &paths).map(|ran| (n.name, ran)))
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
    for native in NATIVES {
        let refusal = native_refusal(row, *native, &checked, &paths);
        // A [`Native::partial`] backend has its own reasons to refuse and
        // its own reasons not to, and neither is what this row is about. It
        // is reported rather than asserted on — `llvm` compiles
        // `host.HostAlloc.allocate`, which `cranelift` does not, and that is
        // a fact about the two surfaces rather than about row 12.
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

/// Undefined on both, and the two implementations differ: JavaScript loses
/// precision above 2^53, a native backend wraps.
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
from "core/num" import * as num;

export fn main(): Result<(), Str> {
  let m = num.maxValue<Int>();
  let over = m + 1;
  let _ = stdout.println("${over}");
  .Ok(())
}
"#,
        "9223372036854776000\n",
        "-9223372036854775808\n",
    );
}

/// The same ceiling seen through `show` rather than through arithmetic: the
/// extremes of `I64` and `U64` are not integers a double can name.
#[test]
fn row_01_integer_show_at_the_64_bit_extremes() {
    rows_or_skip!();
    diverge(
        "row 1 show",
        r#"
from "core/host" import { stdout };
from "core/num" import * as num;

export fn main(): Result<(), Str> {
  let a = num.minValue<I64>();
  let b = num.maxValue<I64>();
  let c = num.maxValue<U64>();
  let _ = stdout.println("${a} ${b} ${c}");
  .Ok(())
}
"#,
        "-9223372036854776000 9223372036854776000 18446744073709552000\n",
        "-9223372036854775808 9223372036854775807 18446744073709551615\n",
    );
}

// -------------------------------------------------------------------
// Row 2 — `Checked` above the exact range
// -------------------------------------------------------------------

/// §12 row 2 and SPEC §6.2.2: `(1 << 60).checkedAdd(1)` is `.Some`
/// natively and `.None` on JavaScript, and this test pins both.
///
/// `Checked` is bounded by the numbers the **backend** has. `.Some(v)`
/// means `v` is the exact true result as that backend represents numbers,
/// so the JavaScript backend stops promising at `2^53 - 1` — past it a
/// `number` cannot say which integer it is — and a native backend stops at
/// the type's own range, where two's-complement overflow is the only thing
/// that could make the answer not the answer. Both keep the same promise;
/// they keep it over different numbers.
///
/// The band between the two bounds is what diverges, and it is the whole
/// row: `1 << 60` plus one, and `maxValue<I64>()` unchanged, are values an
/// `I64` holds exactly and a `number` cannot name. Either side of the band
/// agrees, and both sides are asserted here too — `100 + 20` is `.Some` on
/// both, a division by zero and `maxValue<I64>() + 1` are `.None` on both —
/// so a change that collapsed the divergence in *one* direction cannot pass
/// by moving the whole row.
///
/// The agreeing cases are also the only ones the shared conformance corpus
/// is allowed to hold: `conformance/lib/numbers/test/integers.buri` states
/// the rule and stays on one side of both bounds, because
/// `native/conformance.rs` runs that file natively and a divergent
/// assertion there would be asserting one backend against the other.
#[test]
fn row_02_checked_above_the_exact_range() {
    rows_or_skip!();
    diverge(
        "row 2",
        r#"
from "core/host" import { stdout, alloc };
from "core/bits" import * as bits;
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
  let _ = stdout.println("${a} ${b} ${c} ${d} ${e}");
  .Ok(())
}
"#,
        "None None Some 120 None None\n",
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
/// Every value here is inside 2^53 on purpose: `maxValue<U64>()` clamps to
/// the same *number* on both backends and then prints two different strings,
/// which is row 1 and not this row.
#[test]
fn row_02_saturating_is_bounded_by_the_type_on_both_backends() {
    rows_or_skip!();
    agree(
        "row 2 saturating",
        r#"
from "core/host" import { stdout };

export fn main(): Result<(), Str> {
  let a: I32 = 2147483000;
  let b: U8 = 250;
  let c: I8 = 100;
  let d: I32 = 46341;
  let e: I32 = 0 - 2147483647;
  let _ = stdout.println("${a.saturatingAdd(1000)} ${b.saturatingAdd(10)} ${b.saturatingSub(255)}");
  let _ = stdout.println("${c.saturatingMul(2)} ${c.saturatingMul(0 - 2)} ${d.saturatingMul(d)}");
  let _ = stdout.println("${e.saturatingSub(1000)}");
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
  let _ = stdout.println("${a} ${c} ${d} ${f} ${g} ${i} ${y}");
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
  let _ = stdout.println("${b} ${d} ${f} ${h}");
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
  let _ = stdout.println("${b} ${d} ${f} ${h} ${j} ${l}");
  .Ok(())
}
"#,
        "1 0 0 1 1 -128\n",
    );
}

// -------------------------------------------------------------------
// Row 4 — 128-bit arithmetic
// -------------------------------------------------------------------

/// A listed divergence, and the one where the native answer is simply the
/// right one: JavaScript has no 128-bit integer to compute in.
#[test]
fn row_04_wide_integer_arithmetic() {
    rows_or_skip!();
    diverge(
        "row 4",
        r#"
from "core/host" import { stdout };

export fn main(): Result<(), Str> {
  let a: I128 = 1000000007;
  let b = a * a * a;
  let _ = stdout.println("${b}");
  .Ok(())
}
"#,
        "1.0000000210000002e+27\n",
        "1000000021000000147000000343\n",
    );
}

/// `show` at the 128-bit extremes, which is the same divergence read off a
/// constant rather than out of a multiplication.
#[test]
fn row_04_integer_show_at_the_128_bit_extremes() {
    rows_or_skip!();
    diverge(
        "row 4 show",
        r#"
from "core/host" import { stdout };
from "core/num" import * as num;

export fn main(): Result<(), Str> {
  let a = num.minValue<I128>();
  let b = num.maxValue<I128>();
  let c = num.maxValue<U128>();
  let _ = stdout.println("${a} ${b} ${c}");
  .Ok(())
}
"#,
        "-1.7014118346046923e+38 1.7014118346046923e+38 3.402823669209385e+38\n",
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
  let _ = stdout.println("${tell(a)} | ${tell(b)} | ${tell(c)} | ${same}");
  let _ = stdout.println("${d.show(alloc)} | ${e.show(alloc)} | ${f.show(alloc)} | ${d == e}");
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

export fn main(): Result<(), Str> {
  let a = "abc".len();
  let b = "\u{1F600}".len();
  let c = "e\u{301}".len();
  let d = "".len();
  let e = "\u{1F600}\u{1F600}ab".len();
  let _ = stdout.println("${a} ${b} ${c} ${d} ${e}");
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

export fn main(): Result<(), Str> {
  let sharp = '\u{00df}'.toUpper();
  let scalar = sharp.toU32();
  let ligature = '\u{fb00}'.toUpper();
  let ordinary = 'a'.toUpper();
  let _ = stdout.println("${sharp} ${scalar} ${ligature} ${ordinary}");
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

export fn main(): Result<(), Str> {
  let s = "abcdef";
  let a = s.slice(0, 100);
  let b = s.slice(4, 2);
  let c = s.slice(10, 20);
  let d = s.slice(2, 4);
  let e = s.slice(6, 6);
  let _ = stdout.println("${a}|${b}|${c}|${d}|${e}");
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
  let _ = stdout.println("${a} ${b} ${c} ${d} ${e} ${f} ${g} ${h} ${i} ${j} ${k}");
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
  let _ = stdout.println(o.show(alloc));
  let _ = stdout.println(Shape.Dot.show(alloc));
  let _ = stdout.println(Shape.Line(1, 0 - 2).show(alloc));
  let _ = stdout.println(p.show(alloc));
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
  let _ = stdout.println("${a} ${b} ${c} ${d} ${e} ${f} ${g} ${h} ${i} ${j}");
  let _ = stdout.println("${k} ${l} ${m} ${n} ${o}");
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

export struct T { s: Str, c: Char, b: Bool }
derive Show for T;

export fn main(): Result<(), Str> {
  let a = T { s: "quote\" back\\ tab\t nl\n cr\r nul\u{0}", c: '"', b: true };
  let b = T { s: "\u{1F600} caf\u{e9}", c: '\u{1F600}', b: false };
  let d = T { s: "", c: '\\', b: true };
  let _ = stdout.println(a.show(alloc));
  let _ = stdout.println(b.show(alloc));
  let _ = stdout.println(d.show(alloc));
  let _ = stdout.println("${a.s.len()} ${b.s.len()} ${a.b} ${b.b}");
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

export fn main(): Result<(), Str> {
  let o: Option<Int> = .Some(5);
  let n: Option<Int> = .None;
  let a = match (o) { .Some(v) => "some ${v}", .None => "none" };
  let b = match (n) { .Some(v) => "some ${v}", .None => "none" };
  let _ = stdout.println(a);
  let _ = stdout.println(b);
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
  let _ = stdout.println("${x} ${y} ${z} ${eq} ${ee} ${ef} ${eg}");
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
  let _ = stdout.println("${bare} ${built} ${itself} ${mixed} ${pz} ${nz} ${lt} ${le}");
  .Ok(())
}
"#,
        "true true true false true true false false\n",
    );
}

/// Derived `Hash`: the *numbers*, not merely the verdicts.
///
/// A hash is the one derive whose output is a value a program can print, so
/// "agrees" means the same integer rather than the same partition. Wave 3d
/// landed `deriveHash` at every primitive and claimed it matched `$hash`
/// byte for byte; this pins that end to end, through a struct and an enum
/// rather than only at the primitives.
#[test]
fn row_09_derived_hash_values() {
    rows_or_skip!();
    agree(
        "row 9 hash",
        r#"
from "core/host" import { stdout };

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
  let _ = stdout.println("${h} ${i} ${j} ${k} ${l} ${m} ${n}");
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
/// error and binds nothing, and Cranelift's instruction selector then unwrapped
/// a `None` on the next instruction rather than letting the recorded diagnostic
/// out (`cranelift/emit.rs`'s `bind_absent`). A `derive Hash` over a `[T]`
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

export struct Bag { xs: [Int] }
derive Eq, Hash, Show for Bag;

export fn main(): Result<(), Str> {
  let a = Bag { xs: [1, 2] };
  let b = Bag { xs: [1, 2] };
  let _ = stdout.println(if (a.hash() == b.hash()) { "same" } else { "differ" });
  .Ok(())
}
"#;

const SHOW_A_LIST: &str = r#"
from "core/host" import { stdout, alloc };

export struct Bag { xs: [Int], ss: [Str], empty: [Int] }
derive Show for Bag;

export fn main(): Result<(), Str> {
  let b = Bag { xs: [1, 2, 3], ss: ["a", "b"], empty: [] };
  let _ = stdout.println(b.show(alloc));
  .Ok(())
}
"#;

// -------------------------------------------------------------------
// Row 10 — derived `ToJson`
// -------------------------------------------------------------------

/// `derive ToJson` has no native body at all: `derivePrimJson` is missing
/// at every primitive, so nothing downstream of it can be asked yet.
///
/// Rendering one is a second gap on top — `json.stringify` is `list.mapCtx`
/// and `str.chars` over closures, which is the surface
/// `native/conformance.rs` names against every package that has it — so
/// even a landed `derivePrimJson` would leave this row half-reachable. The
/// program below walks the `Json` tree by hand for that reason: `match`
/// over `.Object`/`.Array` needs no closure, so the day the derive lands
/// the row is pinnable without waiting for the whole of `core/json`.
#[test]
fn row_10_derived_tojson_is_a_gap() {
    rows_or_skip!();
    gap(
        "row 10",
        TOJSON,
        &[
            "derivePrimJson.Bool",
            "derivePrimJson.F64",
            "derivePrimJson.I64",
            "derivePrimJson.Str",
        ],
    );
}

/// The agreement test that runs the day `derivePrimJson` lands. It is a
/// wire format, so the bar is bytes.
#[test]
#[ignore = "`derivePrimJson` has no native body at any primitive, so \
                `derive ToJson` is refused by `missing_intrinsics`. Un-ignore \
                this and delete `row_10_derived_tojson_is_a_gap` together."]
fn row_10_derived_tojson() {
    rows_or_skip!();
    agree(
        "row 10",
        TOJSON,
        "{\"a\":3,\"b\":\"hi\",\"c\":false,\"d\":1.5,\"e\":{\"flag\":true,\"note\":\"n\"}}\n",
    );
}

/// A `Json` rendered without `json.stringify`, because that is closures.
const TOJSON: &str = r#"
from "core/host" import { stdout, alloc };
from "core/str" import * as str;
from "core/json" import { Json, ToJson };

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
  let _ = stdout.println(render(p.toJson(alloc)));
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
/// is `host.HostEnv.arguments` and has no native body yet.
#[test]
fn row_11_division_by_zero() {
    rows_or_skip!();
    abort_agrees(
        "row 11",
        r#"
from "core/host" import { stdout };

fn ratio(a: Int, b: Int): Int { a / b }

export fn main(): Result<(), Str> {
  let zero = "".len();
  let _ = stdout.println("before");
  let _ = stdout.println("${ratio(10, zero)}");
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

fn rest(a: Int, b: Int): Int { a % b }

export fn main(): Result<(), Str> {
  let zero = "".len();
  let _ = stdout.println("${rest(10, zero)}");
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
from "core/host" import { stdout };
from "core/bits" import * as bits;

fn push(x: U8, n: Int): U8 { bits.shlU8(x, n) }

export fn main(): Result<(), Str> {
  let width = 8 + "".len();
  let _ = stdout.println("${push(1, width)}");
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

export fn main(): Result<(), Str> {
  let _ = stdout.println("before");
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

/// Row 12 says the two "must agree once both exist", and neither does.
///
/// On JavaScript `$host_HostAlloc_allocate` answers `[Number(n)]` — the
/// byte count handed back as a `Region`, with nothing accumulated — and
/// natively `host.HostAlloc.allocate` has no body at all, so the backend
/// refuses the program. That is the honest state of the row: not a
/// disagreement, an absence on both sides, named on the side where it is
/// observable.
#[test]
fn row_12_alloc_accounting_is_a_gap() {
    rows_or_skip!();
    gap("row 12", ALLOCATE, &["host.HostAlloc.allocate"]);
}

/// What agreement will mean when MEMORY.md §7's model exists on both.
#[test]
#[ignore = "`Alloc` accounting is implemented on neither backend: \
                `host.HostAlloc.allocate` has no native body, and the \
                JavaScript one hands the byte count back without accumulating \
                anything (MEMORY.md §7 is the model). Un-ignore together with \
                `row_12_alloc_accounting_is_a_gap`."]
fn row_12_alloc_accounting() {
    rows_or_skip!();
    agree("row 12", ALLOCATE, "64\n");
}

const ALLOCATE: &str = r#"
from "core/cap" import { Alloc, Region };
from "core/host" import { stdout, alloc };

export fn main(): Result<(), Str> {
  let r = alloc.allocate(64);
  let n = r.0;
  let _ = stdout.println("${n}");
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
/// used as a condition — it panicked inside Cranelift's frontend. The
/// JavaScript backend is untyped and never noticed.
#[test]
fn row_13_tail_calls_run_in_constant_stack() {
    rows_or_skip!();
    agree(
        "row 13",
        r#"
from "core/host" import { stdout };

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
  let _ = stdout.println("${a} ${b} ${c} ${d}");
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
  let _ = stdout.println("${dup.0}|${dup.1}");
  let _ = stdout.println("${rec.a}|${rec.b}");
  let _ = stdout.println("${two.0}|${two.1}");
  let _ = stdout.println("${got}");
  let _ = stdout.println("${spin(5, ("kl".repeat(alloc, 2), "mn".repeat(alloc, 2)))}");
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

export struct F { x: Float }
derive Eq for F;

fn mk(x: Float): F { F { x: x } }
fn zeroF(): Float { 0.0 }
fn notANumber(): Float { zeroF() / zeroF() }

export fn main(): Result<(), Str> {
  let f = mk(notANumber());
  let _ = stdout.println("${f == f}");
  .Ok(())
}
"#,
        "true\n",
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
    assert_eq!(rows, 15, "§12 has {rows} numbered rows rather than fifteen");
}
