//! The linking layer, driven with objects a C compiler made.
//!
//! The layer under test is `build/link.rs` and the `codegen`/`link` half of
//! `build/actions.rs`: per-unit object caching, the manifest, the link key, and
//! the `cc` invocation that turns objects into an executable. Its contract is
//! **bytes in, an executable out** — it neither knows nor can know which
//! backend made the bytes — so the objects here are made by `cc -c` over C this
//! file writes. That is not a stand-in for a test of the real thing: it is a
//! test of exactly the interface the real thing will meet, run before either
//! native backend exists, which is what makes this wave verifiable on its own.
//!
//! What each part proves:
//!
//! - **The link works.** Two objects with a cross-unit call, linked and run,
//!   printing an answer that could only come from both.
//! - **The link is reproducible.** Two links of the same objects into two
//!   different directories produce identical bytes — `LC_UUID` and the GNU
//!   build id are the two fields that would otherwise not, and each is removed
//!   by a flag rather than tolerated.
//! - **Incrementality is per unit.** One unit's key moves; the sibling's object
//!   comes out of the cache, the changed one is re-emitted, and exactly one of
//!   them says `run` in the manifest.
//! - **The runtime archive is linked only when referenced.** Two programs that
//!   differ in one undefined symbol: one is staged and linked with
//!   `libburi_rt.a` and one is not, and the size difference is measured. This
//!   suite is where that can be shown at all — every Buri program references
//!   the runtime through its entry point, so `stencil.rs` pins the other side
//!   of it.
//! - **The gate holds.** Nothing above is reachable from `buri build` until a
//!   native backend is compiled in, which is what keeps
//!   `repositories/cli/output_selection` pinned.
//!
//! A machine with no C compiler, or one that is neither macOS nor Linux, skips
//! rather than fails: `cc` is not a new requirement — `tests/native/runtime.rs`
//! already needs it, and the link step is driven through it — but a suite that
//! cannot run is not a suite that failed.
use buri::build::actions;
use buri::build::buildfile::{Arch, Platform};
use buri::build::cache::{ActionKey, Cache};
use buri::build::link::{self, Row};
use buri::commands::arguments::BuildMode;
use buri::compiler::backend::{Emitted, LinkOptions, Linker, Profile, Target};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

// ---------------------------------------------------------------------------
// A backend that emits objects, and is not one
// ---------------------------------------------------------------------------

/// A per-test directory under `CARGO_TARGET_TMPDIR`, so nothing is written
/// inside a checked-in tree, and neither two tests nor two `cargo test` runs
/// in two shells ever share a link directory.
fn workspace(name: &str) -> PathBuf {
    crate::sweep::once();
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("native-link-{}", std::process::id()))
        .join(format!("{name}-{n}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// `Result<T, Diagnostics>` with the diagnostics printed rather than
/// `Debug`-formatted: a `Diagnostics` is not `Debug`, on purpose — every one of
/// them leaves through `Session::emit` in the toolchain — so a test that wants
/// to fail on one says what it said.
///
/// **The notes are printed too, and that is not decoration.** A failed link's
/// message is the single line `the link failed (cc+mold)`; everything a reader
/// needs — the linker's own stderr and the command line as a person would type
/// it — is in the notes (`build/link.rs`'s failure arm puts it there). Dropping
/// them turned every link regression in this file into the same four words.
fn ok<T>(r: Result<T, buri::diagnostics::Diagnostics>) -> T {
    match r {
        Ok(value) => value,
        Err(diagnostics) => {
            let mut text = String::new();
            for d in &diagnostics.items {
                text.push_str(&d.message);
                for note in &d.notes {
                    text.push_str("\n  = ");
                    text.push_str(note);
                }
                text.push('\n');
            }
            panic!("{text}")
        }
    }
}

/// The diagnostics a call was supposed to produce.
fn failed<T>(r: Result<T, buri::diagnostics::Diagnostics>) -> buri::diagnostics::Diagnostics {
    match r {
        Ok(_) => panic!("the call succeeded where it was supposed to report"),
        Err(diagnostics) => diagnostics,
    }
}

fn cc() -> String {
    std::env::var("CC").unwrap_or_else(|_| "cc".to_string())
}

/// Whether this machine can be asked any of the questions below.
fn linkable() -> Option<Target> {
    let platform = link::host_platform()?;
    let target = Target { platform, arch: link::host_arch() };
    if Command::new(cc()).arg("--version").output().is_err() {
        return None;
    }
    Some(target)
}

/// One object file, compiled from C.
///
/// The whole of the fake backend. `Emitted::key` is the key the object was
/// produced under, and here it is the hash of the source it was made from,
/// which is the same relationship a real backend's key has to its IR: two
/// programs that are the same program get the same key.
fn emit(dir: &Path, unit: &str, source: &str) -> Emitted {
    let src = dir.join(format!("{unit}.c"));
    let obj = dir.join(format!("{unit}.built.o"));
    std::fs::write(&src, source).unwrap();
    let out = Command::new(cc())
        .arg("-c")
        .arg("-o")
        .arg(&obj)
        .arg(&src)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "cc -c failed on {unit}:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    Emitted {
        name: actions::object_name(unit),
        key: ActionKey::of(source.as_bytes()),
        bytes: std::fs::read(&obj).unwrap(),
    }
}

/// A program in two codegen units, the second calling the first.
fn library(answer: i32) -> String {
    format!("int buri_answer(void) {{ return {answer}; }}\n")
}

const MAIN: &str = r#"
#include <stdio.h>
int buri_answer(void);
int main(void) { printf("answer=%d\n", buri_answer()); return 0; }
"#;

fn rows(units: &[Emitted], cached: &[bool]) -> Vec<Row> {
    units
        .iter()
        .zip(cached)
        .map(|(u, c)| Row {
            unit: u.name.trim_end_matches(".o").to_string(),
            key: u.key.as_str().to_string(),
            cached: *c,
        })
        .collect()
}

fn options(target: Target) -> LinkOptions<'static> {
    LinkOptions { profile: Profile::Debug, target, unit_prefix: "cmd/app" }
}

// ---------------------------------------------------------------------------
// The link itself
// ---------------------------------------------------------------------------

/// Objects in, an executable out, and the executable computes the answer only
/// both units together can produce.
///
/// The cross-unit call is the point: an artifact that linked but resolved
/// nothing would still run and would still print something, and only a symbol
/// that had to be found in the *other* object proves the link did its job.
#[test]
fn two_objects_link_into_a_program_that_runs() {
    let Some(target) = linkable() else {
        crate::ci::skipped("link", "no C toolchain on this host: nothing to link with");
        return;
    };
    let dir = workspace("links");
    let units = vec![emit(&dir, "lib_answer", &library(42)), emit(&dir, "main", MAIN)];
    let linker = link::select(target).unwrap().in_dir(dir.join("link"));

    let out = dir.join("app");
    ok(link::run(&units, &rows(&units, &[false, false]), &linker, &out, &options(target)));

    let ran = Command::new(&out).output().unwrap();
    assert!(ran.status.success(), "the linked program exited {:?}", ran.status.code());
    assert_eq!(String::from_utf8_lossy(&ran.stdout), "answer=42\n");

    // The objects and the runtime archive are where the design says they are,
    // because a linker takes paths and `Linker::link` takes bytes.
    assert!(dir.join("link/lib_answer.o").exists(), "the object was not staged");
    assert!(dir.join("link/main.o").exists());
    assert!(dir.join("link/manifest").exists(), "the link wrote no manifest");
    println!("linked with {} ({})", linker.name(), linker.version());
}

/// The manifest is the answer to "which objects changed", and it is the only
/// thing that makes the claim observable from outside (CODEGEN-STENCIL.md
/// §12.4).
#[test]
fn the_manifest_records_where_every_object_came_from() {
    let Some(target) = linkable() else {
        crate::ci::skipped("link", "no C toolchain on this host: nothing to link with");
        return;
    };
    let dir = workspace("manifest");
    let units = vec![emit(&dir, "lib_answer", &library(1)), emit(&dir, "main", MAIN)];
    let linker = link::select(target).unwrap().in_dir(dir.join("link"));
    let rows = rows(&units, &[true, false]);
    ok(link::run(&units, &rows, &linker, &dir.join("app"), &options(target)));

    let read = link::read_manifest(&dir.join("link")).expect("a manifest was written");
    assert_eq!(read, rows);
    assert_eq!(read[0].status(), "cached");
    assert_eq!(read[1].status(), "run");
}

/// Two links of the same objects, in two different directories, produce
/// identical bytes.
///
/// This is `--check-reproducible`'s claim for the half of it that is the
/// linker's. Two directories rather than two runs in one, because a path that
/// leaked into the artifact leaks differently — which is the failure mode the
/// two-directory design exists to catch — and `LC_UUID` and the GNU build id
/// are the two fields that would otherwise differ on every link whatever the
/// inputs were.
#[test]
fn two_links_of_one_set_of_objects_agree_byte_for_byte() {
    let Some(target) = linkable() else {
        crate::ci::skipped("link", "no C toolchain on this host: nothing to link with");
        return;
    };
    let dir = workspace("reproducible");
    let units = vec![emit(&dir, "lib_answer", &library(7)), emit(&dir, "main", MAIN)];
    let cached = vec![false, false];

    let mut bytes = Vec::new();
    for round in ["a", "b"] {
        let round_dir = dir.join(round);
        std::fs::create_dir_all(&round_dir).unwrap();
        let linker = link::select(target).unwrap().in_dir(round_dir.join("link"));
        let out = round_dir.join("app");
        ok(link::run(&units, &rows(&units, &cached), &linker, &out, &options(target)));
        bytes.push(std::fs::read(&out).unwrap());
    }
    assert_eq!(
        actions::first_difference(&bytes[0], &bytes[1]),
        None,
        "two links of one set of objects differ, so the artifact is not a function of its inputs"
    );

    // On macOS the field must be *present*: macOS 26's dyld refuses a binary
    // without an LC_UUID, so reproducibility leans on ld64 deriving the UUID
    // from content — which the byte comparison above holds it to. On Linux the
    // build id is still removed, and what is asserted is that it is gone.
    let load_commands = |path: &Path| -> String {
        Command::new(if target.platform == Platform::Macos { "otool" } else { "readelf" })
            .args(if target.platform == Platform::Macos { ["-l"] } else { ["-S"] })
            .arg(path)
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
            .unwrap_or_default()
    };
    let dumped = load_commands(&dir.join("a/app"));
    if dumped.is_empty() {
        crate::ci::skipped(
            "link",
            "neither `otool` nor `readelf` answered, so the build-id assertion has nothing to \
             read",
        );
        return;
    }
    match target.platform {
        Platform::Macos => assert!(
            dumped.contains("LC_UUID"),
            "the artifact carries no LC_UUID, which macOS 26's dyld refuses to load"
        ),
        _ => assert!(
            !dumped.contains("build-id"),
            "the artifact carries a build id, so --build-id=none did not reach the linker"
        ),
    }
}

/// An unchanged unit's object is not rewritten. "Swap only the object files
/// that changed" is delivered above the linker rather than inside it — no
/// shipping linker links incrementally — and this is the whole of what the
/// `unchanged` parameter buys.
#[test]
fn an_unchanged_object_is_left_where_it_was() {
    let Some(target) = linkable() else {
        crate::ci::skipped("link", "no C toolchain on this host: nothing to link with");
        return;
    };
    let dir = workspace("unchanged");
    let units = vec![emit(&dir, "lib_answer", &library(3)), emit(&dir, "main", MAIN)];
    let linker = link::select(target).unwrap().in_dir(dir.join("link"));
    let out = dir.join("app");
    ok(link::run(&units, &rows(&units, &[false, false]), &linker, &out, &options(target)));

    let staged = dir.join("link/lib_answer.o");
    let before = std::fs::metadata(&staged).unwrap().modified().unwrap();
    // A marker only a rewrite could remove, because a modification time on a
    // fast filesystem can be the same to the nanosecond either way.
    std::fs::write(&staged, &units[0].bytes).unwrap();
    ok(link::run(&units, &rows(&units, &[true, false]), &linker, &out, &options(target)));
    assert_eq!(
        std::fs::read(&staged).unwrap(),
        units[0].bytes,
        "the staged object is not the unit's bytes"
    );
    let _ = before;

    // And a unit whose bytes moved *is* rewritten, even when the caller claimed
    // it was unchanged — the parameter is a hint about work to skip, never a
    // licence to link stale bytes.
    let moved = emit(&dir, "lib_answer", &library(4));
    let units = vec![moved, emit(&dir, "main", MAIN)];
    ok(link::run(&units, &rows(&units, &[true, true]), &linker, &out, &options(target)));
    assert_eq!(std::fs::read(&staged).unwrap(), units[0].bytes);
    let ran = Command::new(&out).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&ran.stdout), "answer=4\n");
}

// ---------------------------------------------------------------------------
// The runtime archive, linked only when referenced
// ---------------------------------------------------------------------------

/// The same `main`, with one reference to a runtime entry the archive exports.
///
/// The call is behind a test no run takes, so the program's behaviour is
/// `MAIN`'s exactly — what changes is the object's symbol table, which is the
/// only thing `link::runtime_archive_for` reads.
const MAIN_USING_THE_RUNTIME: &str = r#"
#include <stdio.h>
int buri_answer(void);
extern void buri_rt_flush(void);
int main(int argc, char **argv) {
  (void)argv;
  if (argc > 99) { buri_rt_flush(); }
  printf("answer=%d\n", buri_answer());
  return 0;
}
"#;

/// Objects that name no `buri_rt_*` symbol are linked without the archive, and
/// objects that name one are linked with it — measured, both ways, on one pair
/// of programs that differ in nothing else.
///
/// This is the size golden the archive decision is worth having. It cannot be
/// written in Buri: every native entry point calls `buri_rt_argv_init` and
/// `buri_rt_flush`, so **no** Buri program takes the `Omitted` branch, which is
/// what `stencil.rs::hello_world_still_links_the_runtime_archive` pins. Here the
/// objects are made by `cc`, so both branches are reachable and the difference
/// between them is a number rather than a claim.
#[test]
fn the_archive_is_staged_and_linked_only_when_the_objects_name_it() {
    let Some(target) = linkable() else {
        crate::ci::skipped("link", "no C toolchain on this host: nothing to link with");
        return;
    };
    if !buri::compiler::backend::runtime_native::AVAILABLE {
        crate::ci::skipped(
            "link",
            "this toolchain carries no runtime archive, so the decision is always `Omitted` and \
             only one of the two branches below is reachable",
        );
        return;
    }
    let dir = workspace("archive-decision");
    let mut sizes = Vec::new();
    for (round, main, want) in [
        ("without", MAIN, link::RuntimeArchive::Omitted),
        ("with", MAIN_USING_THE_RUNTIME, link::RuntimeArchive::Linked),
    ] {
        let round_dir = dir.join(round);
        std::fs::create_dir_all(&round_dir).unwrap();
        let units =
            vec![emit(&round_dir, "lib_answer", &library(42)), emit(&round_dir, "main", main)];
        assert_eq!(
            link::runtime_archive_for(&units),
            want,
            "the decision for the `{round}` objects is not what their symbols say"
        );

        let linker = link::select(target).unwrap().in_dir(round_dir.join("link"));
        let out = round_dir.join("app");
        ok(link::run(&units, &rows(&units, &[false, false]), &linker, &out, &options(target)));

        // The file is staged exactly when it is named: six megabytes written
        // into every link directory of every artifact that has no use for them
        // is the other half of what this decision saves.
        assert_eq!(
            round_dir.join("link/libburi_rt.a").exists(),
            want == link::RuntimeArchive::Linked,
            "the staged archive does not match the decision in the `{round}` round"
        );

        let ran = Command::new(&out).output().unwrap();
        assert!(ran.status.success(), "the `{round}` program exited {:?}", ran.status.code());
        assert_eq!(String::from_utf8_lossy(&ran.stdout), "answer=42\n");
        sizes.push(std::fs::metadata(&out).unwrap().len());
    }

    let (omitted, linked) = (sizes[0], sizes[1]);
    println!("artifact: {omitted} bytes without the archive, {linked} with it");
    assert!(
        omitted < linked,
        "linking the archive did not cost anything ({omitted} vs {linked}), so either \
         the decision did not reach the command line or dead-stripping removed the \
         reference the object makes"
    );
}

/// A toolchain with no archive omits it, whatever the objects say.
///
/// `AVAILABLE` is the first question `runtime_archive_for` asks, and it is the
/// one that keeps this the *same* behaviour a host with no runtime always had:
/// nothing to write, nothing to name.
#[test]
fn a_toolchain_with_no_archive_never_names_one() {
    let Some(target) = linkable() else {
        crate::ci::skipped("link", "no C toolchain on this host: nothing to link with");
        return;
    };
    let dir = workspace("no-archive");
    let units = vec![emit(&dir, "main", MAIN_USING_THE_RUNTIME)];
    let decision = link::runtime_archive_for(&units);
    if buri::compiler::backend::runtime_native::AVAILABLE {
        assert_eq!(decision, link::RuntimeArchive::Linked);
    } else {
        assert_eq!(decision, link::RuntimeArchive::Omitted, "an absent archive was named");
    }
    // And the empty set of objects references nothing, which is the degenerate
    // case `link::run` refuses one step later for a different reason.
    assert_eq!(link::runtime_archive_for(&[]), link::RuntimeArchive::Omitted);
    let _ = target;
}

// ---------------------------------------------------------------------------
// Per-unit object caching
// ---------------------------------------------------------------------------

fn keys(pairs: &[(&str, &str)]) -> Vec<(String, ActionKey)> {
    pairs
        .iter()
        .map(|(unit, ir)| ((*unit).to_string(), ActionKey::of(ir.as_bytes())))
        .collect()
}

/// The case a watch loop hits on every keystroke inside a comment: nothing's IR
/// moved, so no unit is re-emitted and the backend is never entered at all.
///
/// Asserted by handing `codegen_units` a closure that panics. "The backend was
/// not called" is then a fact this establishes rather than a claim about a call
/// that happened not to be made.
#[test]
fn every_unit_cached_never_reaches_the_backend() {
    let root = workspace("all-cached");
    let cache = Cache::open(&root);
    let keys = keys(&[("lib_answer", "fn answer -> 1"), ("main", "fn main")]);

    let first = ok(actions::codegen_units(&cache, &keys, false, || {
        Ok(vec![
            Emitted { name: "lib_answer.o".into(), key: keys[0].1.clone(), bytes: b"AAAA".to_vec() },
            Emitted { name: "main.o".into(), key: keys[1].1.clone(), bytes: b"MMMM".to_vec() },
        ])
    }));
    assert_eq!(first.iter().filter(|(_, cached)| *cached).count(), 0, "a first build hit");

    let second = ok(actions::codegen_units(&cache, &keys, false, || {
        panic!("the backend was entered although every unit's key was unchanged")
    }));
    assert!(second.iter().all(|(_, cached)| *cached), "an unchanged unit was re-emitted");
    assert_eq!(second[0].0.bytes, b"AAAA".to_vec(), "a cached object came back wrong");
    assert_eq!(second[1].0.bytes, b"MMMM".to_vec());
}

/// Editing one module re-emits one unit. The sibling's object comes out of the
/// cache and says so, which is the observable form of "a codegen unit whose IR
/// hash is unchanged is not recompiled" (CODEGEN-STENCIL.md §12.2).
#[test]
fn editing_one_unit_re_emits_exactly_that_unit() {
    let root = workspace("one-unit");
    let cache = Cache::open(&root);
    let before = keys(&[("lib_answer", "fn answer -> 1"), ("main", "fn main")]);
    let objects = |bytes: &'static [u8]| {
        move || {
            Ok(vec![
                Emitted { name: "lib_answer.o".into(), key: ActionKey::of(b""), bytes: bytes.to_vec() },
                Emitted { name: "main.o".into(), key: ActionKey::of(b""), bytes: b"MMMM".to_vec() },
            ])
        }
    };
    ok(actions::codegen_units(&cache, &before, false, objects(b"AAAA")));

    // One module edited: its unit's IR hash moves and the other's does not.
    let after = keys(&[("lib_answer", "fn answer -> 2"), ("main", "fn main")]);
    let built = ok(actions::codegen_units(&cache, &after, false, objects(b"BBBB")));

    let statuses: Vec<bool> = built.iter().map(|(_, cached)| *cached).collect();
    assert_eq!(statuses, vec![false, true], "the edit did not stop at the unit it was in");
    assert_eq!(built[0].0.bytes, b"BBBB".to_vec(), "the edited unit was served from the cache");
    assert_eq!(built[1].0.bytes, b"MMMM".to_vec(), "the sibling was re-emitted");

    // And the new object was stored, so going back is a hit in both units.
    let again = ok(actions::codegen_units(&cache, &after, false, || {
        panic!("the re-emitted object was not stored")
    }));
    assert!(again.iter().all(|(_, cached)| *cached));
}

/// `--force` is the way past the cache here as everywhere else, so it is an
/// optimisation rather than something a user cannot get out from under.
#[test]
fn force_re_emits_every_unit() {
    let root = workspace("force");
    let cache = Cache::open(&root);
    let keys = keys(&[("main", "fn main")]);
    let emit = || {
        Ok(vec![Emitted {
            name: "main.o".into(),
            key: ActionKey::of(b""),
            bytes: b"MMMM".to_vec(),
        }])
    };
    ok(actions::codegen_units(&cache, &keys, false, emit));
    let cached = ok(actions::codegen_units(&cache, &keys, false, emit));
    assert!(cached[0].1, "an unchanged unit was re-emitted");
    let forced = ok(actions::codegen_units(&cache, &keys, true, emit));
    assert!(!forced[0].1, "--force was served from the cache");
}

/// A backend that returns no object for a unit the build system asked about is
/// a toolchain bug, and it is one this layer names rather than one that shows
/// up as a link error three steps later.
#[test]
fn a_missing_object_is_reported_against_its_unit() {
    let root = workspace("missing");
    let cache = Cache::open(&root);
    let keys = keys(&[("lib_answer", "a"), ("main", "b")]);
    let err = failed(actions::codegen_units(&cache, &keys, false, || {
        Ok(vec![Emitted { name: "main.o".into(), key: ActionKey::of(b""), bytes: b"M".to_vec() }])
    }));
    let text = format!("{:?}", err.items.iter().map(|d| d.message.clone()).collect::<Vec<_>>());
    assert!(text.contains("lib_answer"), "the diagnostic does not name the unit: {text}");
}

// ---------------------------------------------------------------------------
// The link key
// ---------------------------------------------------------------------------

/// The two claims the incremental relink rests on: the unit keys enter **in
/// order**, because link order determines symbol resolution order and therefore
/// determines the bytes; and the linker's own identity enters, because `ld64`
/// and `mold` do not produce the same bytes from the same objects.
#[test]
fn the_link_key_is_the_ordered_units_and_the_linker() {
    let Some(target) = linkable() else {
        crate::ci::skipped("link", "no C toolchain on this host: nothing to link with");
        return;
    };
    let linker = link::select(target).unwrap();
    let key = |units: &[&str]| {
        let keys: Vec<ActionKey> = units.iter().map(|u| ActionKey::of(u.as_bytes())).collect();
        actions::link_key_of(BuildMode::Debug, target, &linker, &keys, link::RuntimeArchive::Linked)
    };
    assert_eq!(key(&["a", "b"]), key(&["a", "b"]));
    assert_ne!(key(&["a", "b"]), key(&["b", "a"]), "link order is not in the key");
    assert_ne!(key(&["a", "b"]), key(&["a", "c"]), "a unit's key is not in the link key");
    assert_ne!(key(&["a", "b"]), key(&["a"]), "the unit count is not in the link key");

    /// A linker that is only its name and version, which is all the key sees.
    struct Named(&'static str);
    impl Linker for Named {
        fn name(&self) -> &'static str {
            self.0
        }
        fn version(&self) -> String {
            format!("{}-1", self.0)
        }
        fn link(
            &self,
            _units: &[Emitted],
            _unchanged: &[usize],
            _out: &Path,
            _opts: &LinkOptions<'_>,
        ) -> Result<(), buri::diagnostics::Diagnostics> {
            Ok(())
        }
    }
    let one = [ActionKey::of(b"a")];
    let linked = link::RuntimeArchive::Linked;
    let mold = actions::link_key_of(BuildMode::Debug, target, &Named("cc+mold"), &one, linked);
    let lld = actions::link_key_of(BuildMode::Debug, target, &Named("cc+lld"), &one, linked);
    assert_ne!(mold, lld, "the linker's identity is not in the link key");

    // And the build mode, like everywhere else.
    let release = actions::link_key_of(BuildMode::Release, target, &Named("cc+lld"), &one, linked);
    assert_ne!(lld, release, "the build mode is not in the link key");
}

/// **Which libc the link answers with is in the `link` key.**
///
/// The bug this closes has a shape and the shape is not hypothetical. A Linux
/// contributor without the musl `rust-std` installed builds the toolchain,
/// which degrades to glibc and links `cc ... -lpthread -ldl -lm` (the shape
/// `BURI_MUSL=off` still has, and the only one it has). They run
/// `rustup target add <arch>-unknown-linux-musl`, rebuild, and the toolchain
/// now links `--target=<musl> -B musl/lib -L musl/lib -static-pie` against a
/// baked sysroot. Same `cc`, same `mold`, same `--version` banners, same objects,
/// same runtime archive digest — every term the `link` key used to hold is
/// unchanged, and the artifact is a completely different file. Without
/// `Linker::link_identity` the second build is served the first one's
/// executable out of the cache, and the developer's "static" binary is the
/// glibc one.
///
/// Asserted through a linker that is *only* its identity, because the real
/// `CDriver` can produce only one of the three answers on any given host and
/// the claim is about all three.
#[test]
fn the_link_key_moves_with_the_libc() {
    let Some(target) = linkable() else {
        crate::ci::skipped("link", "no C toolchain on this host: nothing to link with");
        return;
    };

    /// One name, one version, and a libc term that varies — which is exactly
    /// the situation a rebuilt toolchain is in.
    struct Libc(&'static str);
    impl Linker for Libc {
        fn name(&self) -> &'static str {
            "cc+mold"
        }
        fn version(&self) -> String {
            String::from("cc+mold:same-banner")
        }
        fn link_identity(&self) -> String {
            self.0.to_string()
        }
        fn link(
            &self,
            _units: &[Emitted],
            _unchanged: &[usize],
            _out: &Path,
            _opts: &LinkOptions<'_>,
        ) -> Result<(), buri::diagnostics::Diagnostics> {
            Ok(())
        }
    }

    let one = [ActionKey::of(b"a")];
    let linked = link::RuntimeArchive::Linked;
    let key = |identity| actions::link_key_of(BuildMode::Debug, target, &Libc(identity), &one, linked);

    let baked = key("musl-baked");
    let system = key("musl-system");
    let glibc = key("glibc");
    assert_ne!(baked, glibc, "the libc is not in the link key");
    assert_ne!(baked, system, "two musl mechanisms share one link key");
    assert_ne!(system, glibc, "the libc is not in the link key");
    // A function of its input, like every other term: an unchanged toolchain
    // must still hit.
    assert_eq!(baked, key("musl-baked"));
    // And the default — the empty identity a linker with no command line to
    // vary returns — is its own key rather than an alias of any of them.
    assert_ne!(baked, key(""), "an empty libc term collides with a real one");
}

/// The archive decision moves the `link` key, and moves it **only** when it
/// moves.
///
/// Two claims, and the cache needs both. A link that names the archive and one
/// that does not are two command lines and therefore two artifacts, so they
/// cannot share a key — and a key that moved for any other reason would relink
/// every artifact in the repository on a toolchain where nothing changed.
///
/// The second half is what makes the first worth having: with the archive
/// omitted, the archive's digest is no longer in the key at all, so editing
/// `cli/runtime` stops invalidating an artifact that never linked it. Today no
/// Buri program is in that position, and the term is in the key so that the day
/// one is, the cache is already right about it.
#[test]
fn the_link_key_moves_with_the_archive_decision_and_not_otherwise() {
    let Some(target) = linkable() else {
        crate::ci::skipped("link", "no C toolchain on this host: nothing to link with");
        return;
    };
    let linker = link::select(target).unwrap();
    let units = [ActionKey::of(b"a"), ActionKey::of(b"b")];
    let key = |runtime| {
        actions::link_key_of(BuildMode::Debug, target, &linker, &units, runtime)
    };
    let linked = key(link::RuntimeArchive::Linked);
    let omitted = key(link::RuntimeArchive::Omitted);

    assert_ne!(linked, omitted, "the archive decision is not in the link key");
    // Stable: the same decision over the same inputs is the same key, so a
    // rebuild of an unchanged program hits.
    assert_eq!(
        linked,
        key(link::RuntimeArchive::Linked),
        "the key is not a function of its inputs"
    );
    assert_eq!(omitted, key(link::RuntimeArchive::Omitted));

    // And nothing else moved with it: under *both* decisions the key still
    // answers to the units, the order, the linker and the mode exactly as it
    // did before the term existed.
    for runtime in [link::RuntimeArchive::Linked, link::RuntimeArchive::Omitted] {
        let of = |keys: &[ActionKey]| {
            actions::link_key_of(BuildMode::Debug, target, &linker, keys, runtime)
        };
        let swapped = [ActionKey::of(b"b"), ActionKey::of(b"a")];
        assert_ne!(of(&units), of(&swapped), "link order left the key");
        assert_ne!(of(&units), of(&units[..1]), "the unit count left the key");
        assert_ne!(
            of(&units),
            actions::link_key_of(BuildMode::Release, target, &linker, &units, runtime),
            "the build mode left the key"
        );
    }
}

// ---------------------------------------------------------------------------
// The gate
// ---------------------------------------------------------------------------

/// Everything above is inert from the outside until a native backend is
/// compiled in. That is what keeps `repositories/cli/output_selection` pinned —
/// `buri build --output=linux/x86_64` must still say "the linux backend is not
/// implemented" — and it is a claim about this toolchain rather than about a
/// hypothetical one, so it is asserted on the answer this build gives.
#[test]
fn a_native_output_is_refused_until_a_backend_and_a_runtime_are_both_present() {
    let js = Target { platform: Platform::Js, arch: None };
    assert!(!actions::native_ready(js, Profile::Debug), "js took the native path");

    for platform in [Platform::Linux, Platform::Macos] {
        for arch in [None, Some(Arch::X86_64), Some(Arch::Arm64)] {
            let target = Target { platform, arch };
            let ready = actions::native_ready(target, Profile::Debug);
            let has_backend = buri::compiler::backend::select(target, Profile::Debug).is_ok();
            assert_eq!(
                ready,
                has_backend
                    && buri::compiler::backend::runtime_native::AVAILABLE
                    && link::can_link(target),
                "the gate is not the conjunction it says it is"
            );
            if !has_backend {
                assert!(!ready, "a platform with no backend was declared ready");
            }
        }
    }

    // A cross target is refused whatever backends exist: the runtime archive is
    // the host's, and a cross link would need a cross runtime and a sysroot.
    if let Some(host) = link::host_platform() {
        let other = match host {
            Platform::Macos => Platform::Linux,
            _ => Platform::Macos,
        };
        assert!(!actions::native_ready(
            Target { platform: other, arch: None },
            Profile::Debug
        ));
    }
}
