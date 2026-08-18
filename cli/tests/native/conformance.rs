//! The conformance corpus, compiled and run **natively**.
//!
//! `language/conformance.rs` drives the same corpus through `buri test`, which is the
//! JavaScript backend: it is the suite that says what the language does. This
//! file asks the other half of the question — whether the native backend
//! *agrees* — by taking the same `.buri` files, compiling each one through
//! `middle::run` → `middle::native` → `middle::lower` → `backend::cranelift`,
//! linking, running, and checking that every `test` block in it passed.
//!
//! # How a test file becomes a program
//!
//! It does not. The file is compiled exactly as it is, with
//! `monomorphize::Roots::Tests`, and the Cranelift backend emits a `main` that
//! calls every `test` block in order (`cranelift/mod.rs`'s
//! `test_entry_point`) — which is wave 3d's other half of this: without a
//! native test entry point there was nothing to run, and without
//! `core/testing/assert`'s three bodies there was nothing to assert.
//!
//! A failed assertion **ends the process**, because SPEC 6.10 says an abort is
//! a write and an `_exit` and there is nothing to catch. So the exit status is
//! the result: zero means every block in the file passed, and one means the
//! first failure printed `assert.<kind> failed` and stopped. That is a worse
//! *report* than `buri test` gives and it is not a different answer, which is
//! what this file is checking.
//!
//! # Which packages are in the native set, and which are not
//!
//! [`PACKAGES`] is the list, with the reason beside each exclusion. Three
//! things keep a file out, and they are worth separating because only the third
//! is about the backend:
//!
//!  1. **The harness cannot reach the package's library.** Eleven files import
//!     `//lib/<package>`, and this compiles one file at a time against the
//!     standard library with no repository, so the *front end* refuses them.
//!     That says nothing about the backend and is the first thing `buri test
//!     --backend=cranelift` would fix.
//!  2. **The testing context.** `captureOut`, `clockAt`, `envOf`, `randSeed`,
//!     `MemFs` and `TestStdin` have no native counterpart, and a file that
//!     builds a `Hermetic` context instantiates all of them whether it uses
//!     them or not. This is the single most common exclusion and it is what the
//!     mission expected: effects and the testing-context machinery are a wave
//!     of their own.
//!  3. **The closure surface.** Every `list.*` entry taking a function —
//!     `map`, `filter`, `fold`, `any`, `all`, `find`, `sortBy` and their `Ctx`
//!     variants — is a backend's loop to emit and `cli/runtime/list.rs`'s
//!     header says why. `core/map`, `core/queue`, `core/bitset`, `core/crypto`
//!     and `core/date` are all built on it, which is why **maps are deferred**:
//!     not for want of hashing, which landed this wave and agrees with `$hash`
//!     byte for byte, but for want of `list.fold`.
//!
//! Everything else — `json.*`, `core/proto`, `core/simd`, `core/math`,
//! `core/char`, `core/bytes` — is named against the file that needs it. Every
//! one is reported by `Backend::missing_intrinsics` before a byte of code is
//! generated, rather than discovered as a link error.
//!
//! The exclusions are checked as well as stated: [`the_excluded_packages_are_excluded_for_the_stated_reason`]
//! compiles each one and asserts that the backend does refuse it, so a package
//! that quietly becomes compilable is a failing test rather than a stale
//! comment.

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

use buri::build::buildfile::Platform;
use buri::compiler::backend::cranelift::Cranelift;
use buri::compiler::backend::runtime_native::{ARCHIVE, ARCHIVE_NAME, AVAILABLE};
use buri::compiler::backend::{Backend, Options, Profile, Target};
use buri::compiler::driver;
use buri::compiler::middle::{self, monomorphize};
use buri::compiler::modules::Role;
use buri::diagnostics::{Diagnostics, SourceMap};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

/// One conformance file, and whether the native backend is expected to
/// compile it.
struct Case {
    /// `lib/<package>/test/<file>.buri`, relative to the corpus root.
    path: &'static str,
    /// `None` when the file is in the native set; `Some(reason)` when it is
    /// not, and the reason is what a reader gets instead of a surprise.
    excluded: Option<&'static str>,
}

const fn included(path: &'static str) -> Case {
    Case { path, excluded: None }
}

const fn excluded(path: &'static str, why: &'static str) -> Case {
    Case { path, excluded: Some(why) }
}

/// Every file in `cli/tests/conformance/lib`, in or out, with the reason.
///
/// The list is exhaustive by construction:
/// [`every_conformance_file_is_accounted_for`] walks the corpus and fails on
/// a file that is in neither column, so a package added next door cannot be
/// silently skipped here.
const PACKAGES: &[Case] = &[
    // -- in the native set --------------------------------------------
    //
    // Five files, and between them they are `core/bits` entire,
    // `Checked`/`Wrapping`/`Saturating`/`Bounded` at every width including
    // 128, the bitwise and string codegen corpora, and `core/simd`.
    included("codegen/bitwise.buri"),
    included("numbers/bits.buri"),
    included("numbers/integers.buri"),
    // `core/simd` turned out to need no vector intrinsic at all: it is
    // written in Buri over fixed-size tuples, and the only entries it
    // reaches outside the language are `math.sqrt` and `math.absFloat`.
    included("vectors/simd.buri"),
    // The only file that builds a testing context and still compiles: the
    // one it builds is `alloc`, which is a no-op allocator natively and is
    // open-coded (`cranelift/emit.rs`). Every *stateful* part of the
    // testing context is still absent.
    included("codegen/strings.buri"),
    // `core/alloc`'s three allocators, and the numbers the cost model
    // defines. This one is the payoff of a model that is *defined* rather
    // than measured: the assertions are integers written into the file, and
    // the JavaScript suite next door runs the identical file and gets the
    // identical integers. A backend that disagreed would be wrong, not
    // merely different.
    included("memory/allocators.buri"),
    // -- out: the harness cannot reach the package's own library --------
    //
    // These import `//lib/<package>`, and this harness compiles one file at
    // a time against the standard library with no repository — so the front
    // end refuses them before the backend is asked anything. That is a
    // limit of the harness and says nothing about the backend; the fix is
    // the same one that deletes this file, which is `buri test` learning to
    // pick a native backend.
    excluded(
        "numbers/conversions.buri",
        "an *inexact* conversion answers `Result<T, E>` (SPEC 6.2.1), and \
             constructing that needs the error type `core/num` declares, which \
             the intrinsic table does not name. The exact ones — every \
             widening, and every `wrapTo*` — are compiled",
    ),
    excluded("codegen/equality.buri", "imports //lib/codegen"),
    excluded("codegen/tail_calls.buri", "imports //lib/codegen"),
    excluded("json/decoding.buri", "imports //lib/json"),
    excluded("json/encoding.buri", "imports //lib/json"),
    excluded("proto/binary.buri", "imports //lib/proto"),
    excluded("proto/failures.buri", "imports //lib/proto"),
    excluded("proto/json.buri", "imports //lib/proto"),
    excluded("semantics/effects.buri", "imports //lib/semantics"),
    excluded("semantics/evaluation.buri", "imports //lib/semantics"),
    excluded("semantics/generics.buri", "imports //lib/semantics"),
    excluded("semantics/traits.buri", "imports //lib/semantics"),
    // -- out: the backend has no body for what they reach ---------------
    //
    // Every one of these is reported by `Backend::missing_intrinsics`
    // before a byte of code is generated, which is what that hook is for,
    // and `the_excluded_packages_are_excluded_for_the_stated_reason`
    // checks that the reason is still true.
    excluded(
        "calendar/date.buri",
        "core/date sorts and maps over closures (`list.sortBy`, \
             `list.all`, `list.mapCtx`), and the file builds a `Hermetic` \
             testing context",
    ),
    excluded(
        "canary/canary.buri",
        "`list.fold`, which is the closure surface `cli/runtime/list.rs` \
             names as a backend's job",
    ),
    excluded(
        "collections/bitset.buri",
        "core/bitset folds and filters over closures, and the file builds \
             a `Hermetic` context",
    ),
    excluded(
        "collections/map.buri",
        "core/map is `list.fold`, `list.filterCtx` and `list.sortBy` over \
             closures. The hashing half is *done* — `deriveHash` at every \
             primitive landed in wave 3d and agrees with `$hash` byte for byte \
             (`native/cranelift.rs`) — so what defers maps is the closure walk \
             and nothing about hashing",
    ),
    excluded("collections/queue.buri", "as `collections/map.buri`"),
    excluded(
        "crypto/sha256.buri",
        "core/crypto is `list.map` and `list.flatten` over closures, plus \
             `bytes.toUtf8`",
    ),
    excluded(
        "data/lists.buri",
        "eighteen closure-taking `list.*` entries: this is the file the \
             closure surface exists for",
    ),
    excluded("data/optionresult.buri", "as `data/lists.buri`, plus a testing context"),
    excluded("data/patterns.buri", "as `data/lists.buri`"),
    excluded(
        "data/strings.buri",
        "core/char's eight *classification* entries — `isAlpha` is \
             `\\p{L}`, which is a General_Category table Rust does not expose — \
             and a testing context",
    ),
    excluded(
        "numbers/floats.buri",
        "core/math's thirteen *transcendentals*, whose answers IEEE 754 does \
             not fix — `cli/runtime/math.rs` says why implementing them with the \
             platform libm would be a divergence rather than a gap — and a \
             testing context",
    ),
    excluded("text/bytes.buri", "core/bytes, and the closure surface"),
    excluded("text/json.buri", "core/char's classifiers and the closure surface"),
];

fn supported() -> bool {
    AVAILABLE && cfg!(any(target_os = "macos", target_os = "linux"))
}

fn host_platform() -> Platform {
    if cfg!(target_os = "macos") {
        Platform::Macos
    } else {
        Platform::Linux
    }
}

fn corpus() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/conformance/lib")
}

/// A directory this *process* owns, per case.
///
/// The process id is in the name because two overlapping `cargo test` runs
/// otherwise share it, and the second overwrites the binary the first is
/// executing — which on macOS is a child that never returns rather than an
/// error.
fn workspace(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("native-conformance-{}", std::process::id()))
        .join(name.replace('/', "-"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// The runtime archive, written once for the process rather than once per
/// case, for the reason `native/cranelift.rs::archive` gives.
fn archive() -> &'static Path {
    static WRITTEN: OnceLock<PathBuf> = OnceLock::new();
    WRITTEN.get_or_init(|| {
        let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
            .join(format!("native-conformance-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(ARCHIVE_NAME);
        std::fs::write(&path, ARCHIVE).unwrap();
        path
    })
}

/// Compile one conformance file as a **test binary**, link it, run it.
///
/// Answers `(status, stdout, stderr, blocks)`, where `blocks` is how many
/// `test` declarations the file holds — the count this harness reports, and
/// the one that makes "it passed" mean something.
fn run(name: &str, source: &str) -> Option<(i32, String, String, usize)> {
    let mut map = SourceMap::new();
    let analysis = driver::analyze_snippet(&mut map, "main", source, Role::TestSource);
    if analysis.diags.has_errors() {
        return None;
    }
    let paths: Vec<String> = analysis.loaded.modules.iter().map(|m| m.path.clone()).collect();
    let mut diags = Diagnostics::new();
    let mut program =
        monomorphize::run(&analysis.checked, paths, &mut diags, monomorphize::Roots::Tests);
    assert!(!diags.has_errors(), "{name}: monomorphization failed");
    middle::run(&mut program, &middle::Options::default());
    middle::native(&mut program);
    let blocks = program.roots.tests().len();

    let target = Target { platform: host_platform(), arch: None };
    let opts = Options { profile: Profile::Debug, target, unit_prefix: "" };
    let mut backend = Cranelift;
    let missing = backend.missing_intrinsics(&program, &analysis.checked.tables);
    assert!(missing.is_empty(), "{name}: the backend is missing {missing:?}");
    let units = match backend.emit(&program, &analysis.checked.tables, &opts) {
        Ok(units) => units,
        Err(d) => panic!(
            "{name}: the backend refused the program: {:?}",
            d.items.iter().map(|i| i.message.clone()).collect::<Vec<_>>()
        ),
    };

    let dir = workspace(name);
    let mut objects = Vec::new();
    for unit in &units {
        let path = dir.join(&unit.name);
        std::fs::write(&path, &unit.bytes).unwrap();
        objects.push(path);
    }
    let binary = dir.join("program");
    let mut cc = Command::new(std::env::var("CC").unwrap_or_else(|_| "cc".to_string()));
    cc.arg("-o").arg(&binary);
    for o in &objects {
        cc.arg(o);
    }
    cc.arg(archive());
    if cfg!(target_os = "linux") {
        cc.args(["-lpthread", "-ldl", "-lm"]);
    }
    let built = cc.output().unwrap();
    assert!(
        built.status.success(),
        "{name}: the link failed:\n{}",
        String::from_utf8_lossy(&built.stderr)
    );
    let out = Command::new(&binary).output().unwrap();
    Some((
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
        blocks,
    ))
}

/// Whether the native backend can compile a source at all, without linking.
///
/// Used for the excluded set, where the expected answer is "no" and the
/// interesting output is *which* intrinsic is missing.
fn missing_for(source: &str) -> Result<Vec<String>, String> {
    let mut map = SourceMap::new();
    let analysis = driver::analyze_snippet(&mut map, "main", source, Role::TestSource);
    if analysis.diags.has_errors() {
        return Err(analysis
            .diags
            .items
            .iter()
            .take(2)
            .map(|d| d.message.clone())
            .collect::<Vec<_>>()
            .join("; "));
    }
    let paths: Vec<String> = analysis.loaded.modules.iter().map(|m| m.path.clone()).collect();
    let mut diags = Diagnostics::new();
    let mut program =
        monomorphize::run(&analysis.checked, paths, &mut diags, monomorphize::Roots::Tests);
    if diags.has_errors() {
        return Err(String::from("monomorphization failed"));
    }
    middle::run(&mut program, &middle::Options::default());
    middle::native(&mut program);
    Ok(Cranelift.missing_intrinsics(&program, &analysis.checked.tables))
}

/// `calendar/date.buri` names `lib/calendar/test/date.buri`: the corpus
/// puts a package's test sources under `test/`, and the key here drops that
/// because every file in the list is one.
fn read(case: &Case) -> String {
    let (package, file) = case.path.split_once('/').unwrap_or((case.path, ""));
    let path = corpus().join(package).join("test").join(file);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

// -----------------------------------------------------------------------

/// The list above covers the corpus, so a package added next door is a
/// failing test here rather than a silent omission.
#[test]
fn every_conformance_file_is_accounted_for() {
    let root = corpus();
    let mut found: Vec<String> = Vec::new();
    let mut packages: Vec<_> =
        std::fs::read_dir(&root).unwrap().filter_map(Result::ok).collect();
    packages.sort_by_key(std::fs::DirEntry::file_name);
    for package in packages {
        let tests = package.path().join("test");
        if !tests.is_dir() {
            continue;
        }
        let mut files: Vec<_> =
            std::fs::read_dir(&tests).unwrap().filter_map(Result::ok).collect();
        files.sort_by_key(std::fs::DirEntry::file_name);
        for file in files {
            found.push(format!(
                "{}/{}",
                package.file_name().to_string_lossy(),
                file.file_name().to_string_lossy()
            ));
        }
    }
    for path in &found {
        assert!(
            PACKAGES.iter().any(|c| c.path == path),
            "`{path}` is in the conformance corpus and in neither column of \
                 `PACKAGES`: put it in the native set, or exclude it with a reason"
        );
    }
    for case in PACKAGES {
        assert!(found.iter().any(|f| f == case.path), "`{}` no longer exists", case.path);
    }
}

/// The excluded set is excluded because the backend refuses it, not because
/// somebody wrote a comment.
///
/// This is the test that keeps the reasons above honest: if a package
/// becomes compilable — because a later wave landed `json.*` or the closure
/// surface — this fails and the reason has to be deleted rather than left
/// to rot.
#[test]
fn the_excluded_packages_are_excluded_for_the_stated_reason() {
    if !supported() {
        return;
    }
    for case in PACKAGES.iter().filter(|c| c.excluded.is_some()) {
        let source = read(case);
        match missing_for(&source) {
            // A front-end error means the corpus is mid-change, which is
            // not this file's business to fail over.
            Err(_) => continue,
            Ok(missing) => assert!(
                !missing.is_empty(),
                "`{}` is listed as excluded ({}), but the backend now \
                     compiles it — delete the exclusion",
                case.path,
                case.excluded.unwrap_or_default()
            ),
        }
    }
}

/// Every file in the native set compiles, links, runs, and every `test`
/// block in it passes.
///
/// The bar is the block count, not the exit status alone: a file that
/// compiled to no tests at all would exit zero and prove nothing, which is
/// the same reason `language/conformance.rs` counts its assertions.
#[test]
fn the_native_set_passes() {
    if !supported() {
        return;
    }
    let mut total = 0usize;
    let mut ran = 0usize;
    let mut skipped: Vec<String> = Vec::new();
    for case in PACKAGES.iter().filter(|c| c.excluded.is_none()) {
        let source = read(case);
        // A file the *front end* refuses is not this file's failure: the
        // corpus is shared with `language/conformance.rs` and may be mid-change.
        match missing_for(&source) {
            Err(e) => {
                skipped.push(format!("{} (front end: {e})", case.path));
                continue;
            }
            Ok(missing) if !missing.is_empty() => panic!(
                "`{}` is in the native set but the backend is missing {missing:?}",
                case.path
            ),
            Ok(_) => {}
        }
        let Some((status, out, err, blocks)) = run(case.path, &source) else {
            skipped.push(format!("{} (front end)", case.path));
            continue;
        };
        assert_eq!(status, 0, "`{}` failed:\nstdout:\n{out}\nstderr:\n{err}", case.path);
        assert!(blocks > 0, "`{}` holds no `test` blocks", case.path);
        total += blocks;
        ran += 1;
    }
    for s in &skipped {
        eprintln!("native conformance: skipped {s}");
    }
    eprintln!("native conformance: {ran} files, {total} test blocks, 0 failures");
    assert!(ran > 0, "no conformance file ran natively");
}

/// The harness has to be able to fail.
///
/// `language/conformance.rs` has the same test for the same reason: a suite that
/// cannot fail proves nothing. The canary package is not in the native set
/// — it uses `list.fold` — so this breaks an assertion in a file that is,
/// and checks that the native binary exits non-zero and says which
/// assertion it was.
#[test]
fn the_native_set_can_fail() {
    if !supported() {
        return;
    }
    let case = Case { path: "numbers/bits.buri", excluded: None };
    let source = read(&case);
    if missing_for(&source).is_err() {
        return;
    }
    // The value, not a name: renaming a constant and its use together would
    // leave the assertion true. `assert!` on the marker means a corpus that
    // stopped containing it fails here rather than passing vacuously.
    const MARKER: &str = "assert.eq(bits.shl(1, 10), 1024);"; 
    assert!(
        source.contains(MARKER),
        "`numbers/bits.buri` no longer contains the assertion this test edits"
    );
    let broken = source.replace(MARKER, "assert.eq(bits.shl(1, 10), 1025);");
    let Some((status, out, err, _)) = run("bits-broken", &broken) else {
        return;
    };
    assert_ne!(status, 0, "a broken assertion still passed:\n{out}\n{err}");
    assert!(
        err.contains("assert.eq failed"),
        "the failure did not name the assertion:\nstdout:\n{out}\nstderr:\n{err}"
    );
}
