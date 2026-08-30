//! Turning codegen units into an executable.
//!
//! This is the implementation of [`Linker`] for the native platforms, and the
//! directory layout the link step works in. It lives under `build/` rather than
//! under `backend/` on purpose: `stencil` and `llvm` both hand their objects
//! to the same linker, so a linker is not a property of a backend — and what is
//! in here is *not* code generation. It is process invocation, `PATH` probing,
//! a manifest file, and a cache key, which is the build system's subject
//! matter. `backend/mod.rs` keeps the trait; `backend/js/mod.rs` keeps
//! `Concatenate`, which is the JavaScript artifact's whole link step and shares
//! nothing with this.
//!
//! # The invocation
//!
//! **The link is always driven through the platform C compiler**, never by
//! invoking `ld` directly. The driver is what knows
//! where `crt1.o`, `libc`, and `libSystem.tbd` live, which SDK is selected, and
//! what a `-syslibroot` should be; reimplementing that is reimplementing the
//! part of a toolchain that changes with every OS release. Which *linker* the
//! driver then runs is chosen with `-fuse-ld=`:
//!
//! ```text
//! linux   cc -fuse-ld=mold  -o A u0.o u1.o libburi_rt.a -Wl,--build-id=none -Wl,--gc-sections
//!         cc -fuse-ld=lld   ...                          (mold absent)
//!         cc                ...                          (neither present)
//! macos   cc -fuse-ld=lld   -o A u0.o u1.o libburi_rt.a -Wl,-dead_strip
//!         cc                ...                          (ld64.lld absent)
//! ```
//!
//! mold first on Linux because it is 3-10x faster than lld on its own
//! benchmarks; mold is **not** an option on macOS (it is ELF-only, and its
//! Mach-O fork `sold` was archived in November 2024), so there the choice is
//! `ld64.lld` or Apple's own `ld`. `ld64.lld` is preferred when it is on `PATH`
//! because the toolchain's devShell puts it there, and a linker that came with
//! the pinned toolchain is a linker that is the same on every machine — which
//! is the property `--check-reproducible` is about. When it is absent the
//! system linker does the job — it is what every macOS SDK assumption is built
//! around — and either way the fallback is not a failure mode: **there is
//! no case in which a build fails for want of a particular linker**, and no
//! flag that has to be set to get a working one.
//!
//! `BURI_LINKER` forces the choice — `mold`, `lld`, or `cc` — and is the escape
//! hatch for a `cc` too old to understand `-fuse-ld=mold`. It is not a hole in
//! hermeticity: the linker's name and version are in the `link` key
//! ([`CDriver::version`]), so two choices are two cache entries rather than one
//! entry that might hold either's bytes.
//!
//! # Reproducibility
//!
//! ARCHITECTURE.md §7 names three sources of nondeterminism, and the two that
//! belong to the linker are closed here rather than compared for:
//!
//! - **`LC_UUID`** stays: ld64's UUID is a content digest, so two identical
//!   links carry one UUID. It used to be removed with `-no_uuid`, until macOS
//!   26's dyld (under Xcode 26 tooling) started rejecting binaries without one.
//! - **The GNU build id** is removed with `--build-id=none` on Linux, for the
//!   same reason and one more: it is a hash of content that is about to be
//!   compared byte for byte, so it can only ever restate the answer.
//! - **Timestamps** never enter: `SOURCE_DATE_EPOCH=0` and `ZERO_AR_DATE=1` are
//!   in the linker's environment, the runtime archive is embedded bytes rather
//!   than a freshly-`ar`'d one, and nothing here stamps a file.
//!
//! The third of §7's sources — absolute paths in debug info — is the backend's,
//! through `Options::unit_prefix`.
//!
//! # The directory
//!
//! ```text
//! .buri/link/<link-key>/manifest        unit name -> codegen key -> cached|run
//! .buri/link/<link-key>/<unit>.o        the object, from the cache
//! .buri/link/<link-key>/libburi_rt.a    the embedded runtime archive, when
//!                                       these objects reference it
//! ```
//!
//! The archive's last line is a decision rather than a constant:
//! [`runtime_archive_for`] asks the objects whether any of them names a
//! `buri_rt_*` symbol, and the answer both gates the file and enters the `link`
//! key. On this toolchain it is `Linked` for every Buri program — see that
//! function for why, and for what would have to change.
//!
//! It exists because a linker takes paths and [`Linker::link`] takes bytes, and
//! because the manifest is what makes "which objects changed" answerable from
//! outside.

use crate::build::buildfile::{Arch, Platform};
use crate::build::cache::hash_bytes;
use crate::build::spawn;
use crate::compiler::backend::runtime_native;
use crate::compiler::backend::{Emitted, LinkOptions, Linker, Target};
use crate::diagnostics::{Diagnostic, Diagnostics, Span};
use std::path::{Path, PathBuf};
use std::process::Command;

// ---------------------------------------------------------------------------
// What the host can link
// ---------------------------------------------------------------------------

/// The architecture this toolchain is running on, or `None` on one no `Arch`
/// names.
pub fn host_arch() -> Option<Arch> {
    if cfg!(target_arch = "x86_64") {
        Some(Arch::X86_64)
    } else if cfg!(target_arch = "aarch64") {
        Some(Arch::Arm64)
    } else {
        None
    }
}

/// The platform this toolchain is running on, or `None` where there is no
/// native backend and no runtime archive anyway.
pub fn host_platform() -> Option<Platform> {
    if cfg!(target_os = "macos") {
        Some(Platform::Macos)
    } else if cfg!(target_os = "linux") {
        Some(Platform::Linux)
    } else {
        None
    }
}

/// Whether this machine can link an artifact for `target`.
///
/// Host only, and that is a decision rather than an omission
/// (ARCHITECTURE.md §9): the runtime archive is built for the host by
/// `cli/build.rs`, and a cross link would need a cross runtime, a cross libc
/// and a sysroot. A declared `linux/x86_64` output on an arm64 mac is refused
/// with the same diagnostic as a missing backend, which is the honest answer.
pub fn can_link(target: Target) -> bool {
    host_platform() == Some(target.platform)
        && target.arch.is_none_or(|a| host_arch() == Some(a))
}

// ---------------------------------------------------------------------------
// Whether the runtime archive is linked at all
// ---------------------------------------------------------------------------

/// Whether a link puts `libburi_rt.a` on the driver's command line.
///
/// It used to be one question — "does this toolchain *have* an archive" — and
/// the answer to "does this program *need* one" was left to `-dead_strip` and
/// `--gc-sections`. That still works, and it is still what decides the
/// artifact's size — per target rather than by BUILD-AND-WATCH.md §2.2's single
/// number: hello world is 356 KB with its debug information removed against a
/// 6.05 MB archive on arm64 macOS, and 401 KB against 8.73 MB on x86-64 Linux.
/// The *linked* file on Linux is 2.19 MB, and the difference is not the
/// stripping: an ELF link copies the archive members' DWARF into the executable
/// where `ld64` leaves it in the members. The numbers, and how each was taken,
/// are in `tests/native/stencil.rs::hello_world_still_links_the_runtime_archive`.
/// What it could not do is keep the archive out of the `link` **key**: an
/// artifact that names no runtime symbol was still relinked whenever the
/// runtime changed, because the key held the archive's digest unconditionally.
///
/// So the answer is a value rather than a `bool`, and it travels to both places
/// that need it: [`CDriver::link`]'s command line, and
/// `build::actions::link_key_of`'s `runtime` term. Those two must agree —
/// disagreeing would key one artifact under another's inputs — and they agree
/// because both ask [`runtime_archive_for`] about the same objects.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RuntimeArchive {
    /// The archive is staged beside the objects and named on the command line.
    Linked,
    /// It is neither written nor named: nothing in these objects refers to it,
    /// or this toolchain has none.
    Omitted,
}

impl RuntimeArchive {
    /// Whether the archive reaches the command line.
    pub fn is_linked(self) -> bool {
        self == RuntimeArchive::Linked
    }
}

/// Whether these objects reference the runtime archive.
///
/// This is [`crate::compiler::backend::networking_gap`]'s question asked one
/// level lower down, and it is asked of the **objects** rather than of
/// `program.funcs` — which is a correction to the shape the design proposed,
/// and the reason for it is measurable rather than stylistic. A program's
/// intrinsic keys are not the whole of what it asks the archive for: both
/// native entry points call `buri_rt_argv_init` and `buri_rt_flush`
/// unconditionally (`stencil/asm.rs`, `llvm/emit.rs::entry_point`), reference
/// counting reaches `buri_rt_alloc`/`buri_rt_free`, a division emits
/// `buri_rt_abort_div_zero`, `==` on `Str` emits `buri_rt_str_eq`, and none of
/// those is a `FuncKind::Intrinsic` anybody could have walked for. A query over
/// the program would have had to restate every emitter's structural calls, in a
/// third place, and would have been wrong the first time one of them moved —
/// and being wrong here is an unresolved symbol at `cc` time.
///
/// So the question is asked of what the backend actually produced, and the
/// prefix is the one rule `runtime_native::SYMBOL_PREFIX` already states: an
/// object that refers to a runtime entry carries that entry's name in its
/// symbol table, as ASCII, whatever the object format. Conservative in the one
/// direction that is safe — a string constant that happens to spell
/// `buri_rt_` links an archive nothing needs, which costs a staged file and a
/// dead-stripped nothing, where the opposite mistake costs a failed link.
///
/// **On this toolchain the answer is [`RuntimeArchive::Linked`] for every Buri
/// program**, because of the two entry-point calls above. That is not a
/// disappointment hidden in a comment: it is the finding, it is pinned by
/// `tests/native/stencil.rs::hello_world_still_links_the_runtime_archive`, and
/// the `Omitted` branch is reachable today only for objects that were not made
/// from Buri source and for a toolchain with no archive at all. It becomes
/// interesting the day an entry point stops needing the runtime.
///
/// One pass over the objects, comparing only at the bytes that could begin the
/// prefix — the link is about to read every one of them off disk anyway.
pub fn runtime_archive_for(units: &[Emitted]) -> RuntimeArchive {
    if !runtime_native::AVAILABLE {
        return RuntimeArchive::Omitted;
    }
    let needle = runtime_native::SYMBOL_PREFIX.as_bytes();
    if units.iter().any(|unit| mentions(&unit.bytes, needle)) {
        RuntimeArchive::Linked
    } else {
        RuntimeArchive::Omitted
    }
}

/// Whether `haystack` contains `needle`.
///
/// `windows(n).any(..)` would say the same thing and compare at every offset;
/// this compares only where the first byte matches, which over a few megabytes
/// of object is the difference between a scan and a memory-bandwidth-bound
/// one. `get(i..)` rather than `[i..]` because this repository's lints deny
/// indexing, and the slice cannot be empty here in any case.
fn mentions(haystack: &[u8], needle: &[u8]) -> bool {
    let Some(first) = needle.first() else { return true };
    haystack
        .iter()
        .enumerate()
        .filter(|(_, byte)| *byte == first)
        .any(|(i, _)| haystack.get(i..).is_some_and(|rest| rest.starts_with(needle)))
}

// ---------------------------------------------------------------------------
// The manifest
// ---------------------------------------------------------------------------

/// One unit's row in `.buri/link/<link-key>/manifest`.
///
/// The key is the hex text rather than an `ActionKey`, so that writing the file
/// and reading it back are the same shape: a key that has been through a file
/// is a string, and re-wrapping it in a type whose only constructor hashes
/// would be inventing a hash of a hash.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Row {
    /// The codegen unit's name, without the `.o`: `core_list`, `main`.
    pub unit: String,
    /// Its `codegen` key, in full.
    pub key: String,
    /// Whether the object came from the cache rather than from this build.
    pub cached: bool,
}

impl Row {
    pub fn status(&self) -> &'static str {
        if self.cached {
            "cached"
        } else {
            "run"
        }
    }
}

/// The manifest's text: one line per unit, columns aligned to the longest name.
///
/// ```text
/// core_list  3f9a1c2b8d4e...  cached
/// main       8b2e01f4c7a9...  run
/// ```
fn manifest_text(rows: &[Row]) -> String {
    let width = rows.iter().map(|r| r.unit.len()).max().unwrap_or(0);
    let mut out = String::new();
    for row in rows {
        let pad = width.saturating_sub(row.unit.len());
        out.push_str(&row.unit);
        for _ in 0..pad {
            out.push(' ');
        }
        out.push(' ');
        out.push_str(&row.key);
        out.push(' ');
        out.push_str(row.status());
        out.push('\n');
    }
    out
}

/// The manifest as rows, or `None` when there is no readable one.
///
/// A line that is not three fields is skipped rather than refused: the manifest
/// is a record of a build, not an input to one, and a truncated one is a reason
/// to know less rather than a reason to fail.
pub fn read_manifest(dir: &Path) -> Option<Vec<Row>> {
    let text = std::fs::read_to_string(dir.join("manifest")).ok()?;
    let mut rows = Vec::new();
    for line in text.lines() {
        let fields: Vec<&str> = line.split_whitespace().collect();
        let [unit, key, status] = fields.as_slice() else { continue };
        rows.push(Row {
            unit: (*unit).to_string(),
            key: (*key).to_string(),
            cached: *status == "cached",
        });
    }
    Some(rows)
}

/// `.buri/link/<link-key>`, the directory one link runs in.
pub fn dir(root: &Path, link_key: &str) -> PathBuf {
    root.join(".buri/link").join(link_key)
}

// ---------------------------------------------------------------------------
// Which linker
// ---------------------------------------------------------------------------

/// Which linker the C driver is told to run.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Flavour {
    /// `-fuse-ld=mold`. Linux only.
    Mold,
    /// `-fuse-ld=lld`: `ld.lld` on Linux, `ld64.lld` on macOS.
    Lld,
    /// Whatever the driver defaults to.
    System,
}

impl Flavour {
    pub fn name(self) -> &'static str {
        match self {
            Flavour::Mold => "cc+mold",
            Flavour::Lld => "cc+lld",
            Flavour::System => "cc",
        }
    }

    /// The program whose presence on `PATH` this flavour needs, and whose
    /// `--version` enters the key.
    fn program(self, platform: Option<Platform>) -> Option<&'static str> {
        match (self, platform) {
            (Flavour::Mold, _) => Some("mold"),
            (Flavour::Lld, Some(Platform::Macos)) => Some("ld64.lld"),
            (Flavour::Lld, _) => Some("ld.lld"),
            (Flavour::System, _) => None,
        }
    }

    fn driver_flag(self) -> Option<&'static str> {
        match self {
            Flavour::Mold => Some("-fuse-ld=mold"),
            Flavour::Lld => Some("-fuse-ld=lld"),
            Flavour::System => None,
        }
    }
}

/// The C driver, and the linker it is told to run.
///
/// One type for all three flavours rather than a `Lld`, a `Mold` and a
/// `CDriver` that would differ by a single argument: what varies is one flag
/// and one probe, and three structs whose `link` bodies were copies of each
/// other would be three places for a platform flag to be fixed in two of them.
pub struct CDriver {
    /// `.buri/link/<link-key>`: where the objects and the archive are written.
    dir: PathBuf,
    /// The driver: `$CC`, or `cc`.
    driver: PathBuf,
    flavour: Flavour,
    target: Target,
    /// Shared with every other `CDriver` this process selected for the same
    /// driver and flavour, so the two `--version` spawns happen once (see
    /// [`PROBED`]).
    version: std::sync::Arc<Identity>,
}

/// `<flavour>:<sha256 of the driver's and the linker's `--version`>`, probed on
/// a thread of its own.
///
/// Asking two programs for their version is two process spawns, and where the
/// driver is a wrapper script it is two shells as well: 100 ms on this
/// toolchain. Nothing between [`select`] and
/// [`crate::build::actions::link_key`] reads the answer, and what lies between
/// them is the entire front and middle end — so the probe runs beside that work
/// rather than in front of it. It is the same two commands producing the same
/// string; only the thread it happens on is new.
///
/// Probed once, because [`Linker::version`] is called per key and shelling out
/// per key would be a process per key. **Once per process**, not once per
/// [`select`]: `buri test //...` selects a linker per suite, and on a repository
/// of five small suites the same two `--version` banners were the largest single
/// item in the run — 100 ms each time, against a front end that finishes in one.
/// The registry below is what makes the count one, and the term it produces is
/// the same string it always was.
struct Identity {
    flavour: &'static str,
    /// The two programs the probe asks, kept so that the answer can always be
    /// computed here as well.
    programs: Vec<PathBuf>,
    ready: std::sync::OnceLock<String>,
    probe: std::sync::Mutex<Option<std::thread::JoinHandle<String>>>,
}

impl Identity {
    /// The finished identity, waiting for the probe if it is still running.
    ///
    /// **A probe that did not run is asked here instead of being skipped.** The
    /// hash of the two `--version` banners is the term that keeps two different
    /// linkers out of each other's cache entries, so an empty one would be a
    /// key that merges them — the exact shape of a stale artifact served for a
    /// linker that never produced it. The thread is a way of paying for this
    /// term earlier, never a way of doing without it.
    fn get(&self) -> String {
        if let Some(ready) = self.ready.get() {
            return ready.clone();
        }
        let handle = self.probe.lock().ok().and_then(|mut slot| slot.take());
        let hashed = handle
            .and_then(|handle| handle.join().ok())
            .unwrap_or_else(|| probe(&self.programs));
        let value = format!("{}:{hashed}", self.flavour);
        let _ = self.ready.set(value.clone());
        self.ready.get().cloned().unwrap_or(value)
    }
}

/// The hash of every named program's `--version`, in order.
///
/// One thread per program, joined in order, so the string is the concatenation
/// it has always been while the spawns overlap: on a toolchain whose `cc` is a
/// wrapper script the driver's banner costs twice what the linker's does, and
/// asking them one after the other pays for both.
fn probe(programs: &[PathBuf]) -> String {
    let mut identity = String::new();
    let mut started = Vec::with_capacity(programs.len());
    for program in programs {
        let owned = program.clone();
        started.push(
            std::thread::Builder::new()
                .name("buri-linker-version".into())
                .spawn(move || version_of(&owned))
                .ok(),
        );
    }
    // In order, and a thread that could not be started or did not return is
    // asked for here rather than left out: a missing banner would shorten the
    // term that keeps two linkers out of each other's cache entries.
    for (program, handle) in programs.iter().zip(started) {
        let banner = handle
            .and_then(|handle| handle.join().ok())
            .unwrap_or_else(|| version_of(program));
        identity.push_str(&banner);
    }
    hash_bytes(identity.as_bytes())
}

/// The identity probes this process has already started, by the programs they
/// ask.
///
/// Process-global because the answer is: two `--version` banners are a fact
/// about the machine, and a `buri test //...` that selects a linker per suite
/// was asking the same two programs the same question once per suite. The key is
/// the flavour and the resolved program paths, which is everything [`probe`]
/// reads — so two entries differ exactly when the string they produce could.
///
/// The scope is the process, which is the scope a `--watch` loop's passes share.
/// A linker replaced underneath a running `buri test --watch` keeps the banner
/// it had when the loop started; that is the same window `Identity`'s own
/// `OnceLock` has always had within one build, widened to the process that a
/// user would have to restart anyway to pick up a new toolchain.
static PROBED: std::sync::Mutex<Vec<(String, std::sync::Arc<Identity>)>> =
    std::sync::Mutex::new(Vec::new());

/// The shared identity for one flavour and one set of programs, starting the
/// probe the first time it is asked for.
fn identity_of(flavour: Flavour, programs: Vec<PathBuf>) -> std::sync::Arc<Identity> {
    let mut name = String::from(flavour.name());
    for program in &programs {
        name.push('\u{0}');
        name.push_str(&program.to_string_lossy());
    }
    let fresh = || {
        let probed = programs.clone();
        let started = std::thread::Builder::new()
            .name("buri-linker-version".into())
            .spawn(move || probe(&probed))
            .ok();
        std::sync::Arc::new(Identity {
            flavour: flavour.name(),
            programs,
            ready: std::sync::OnceLock::new(),
            probe: std::sync::Mutex::new(started),
        })
    };
    // A poisoned registry is a reason to probe again, never a reason to fail: it
    // is a memo, and the answer it holds is one this can always compute.
    let Ok(mut table) = PROBED.lock() else { return fresh() };
    if let Some((_, ready)) = table.iter().find(|(key, _)| *key == name) {
        return std::sync::Arc::clone(ready);
    }
    let ready = fresh();
    table.push((name, std::sync::Arc::clone(&ready)));
    ready
}

/// Starts the linker-identity probe for `target` without needing a link.
///
/// The probe runs on a thread and is joined by the first `link` key that is
/// built. On a repository whose suites compile in a millisecond there is nothing
/// between the two for it to hide behind, so `buri test` asks for it once before
/// the first suite instead — the thread then runs beside the front end of the
/// whole pass rather than beside one suite's.
///
/// A target this host cannot link has no probe to start, and saying so is not
/// this function's job: the run reaches [`select`] and is refused there.
pub fn warm(target: Target) {
    let _ = select(target);
}

/// The driver and the linker for one target, probing `PATH`.
///
/// The link directory is *not* a parameter, because it is named by the `link`
/// key and the `link` key is built from [`Linker::version`] — so the linker has
/// to exist before the directory it will work in has a name.
/// [`CDriver::in_dir`] closes the loop.
///
/// `Err` names the thing that is missing, in the form a diagnostic's message
/// wants. The only two ways to get one are a target this host cannot link and a
/// `cc` that is not installed — and the second is not a case the fallback story
/// covers, because there is no link at all without a C driver.
pub fn select(target: Target) -> Result<CDriver, String> {
    if !can_link(target) {
        return Err(format!(
            "this toolchain cannot link {} artifacts on this machine",
            target.platform.slug()
        ));
    }
    let name = std::env::var("CC").unwrap_or_else(|_| String::from("cc"));
    let Some(driver) = spawn::resolve(&name) else {
        return Err(format!("no C compiler on PATH to drive the link (looked for `{name}`)"));
    };
    let flavour = choose(target.platform);
    let mut programs = vec![driver.clone()];
    programs.extend(flavour.program(Some(target.platform)).and_then(spawn::resolve));
    let version = identity_of(flavour, programs);
    Ok(CDriver { dir: PathBuf::new(), driver, flavour, target, version })
}

/// The flavour for a platform, honouring `BURI_LINKER` and otherwise probing.
fn choose(platform: Platform) -> Flavour {
    let forced = std::env::var("BURI_LINKER").unwrap_or_default();
    let candidates: &[Flavour] = match platform {
        // mold is ELF-only and refuses macOS by name; `sold`, the Mach-O fork,
        // was archived in November 2024 with its author recommending Apple's
        // linker instead.
        Platform::Macos => &[Flavour::Lld],
        Platform::Linux => &[Flavour::Mold, Flavour::Lld],
        // Exhaustive rather than a catch-all, which would have handed a WEB
        // target a native linker to probe for. A JavaScript artifact is not
        // linked; nothing reaches this with one, and now nothing can.
        Platform::Js | Platform::Web => &[],
    };
    match forced.as_str() {
        "cc" | "system" => return Flavour::System,
        "mold" => return Flavour::Mold,
        "lld" => return Flavour::Lld,
        _ => {}
    }
    for flavour in candidates {
        let present = flavour
            .program(Some(platform))
            .is_some_and(|p| spawn::resolve(p).is_some());
        if present {
            return *flavour;
        }
    }
    Flavour::System
}

/// A program's `--version`, hashed into the linker's identity.
///
/// The bytes rather than a parsed version number: what has to be true is that
/// two different linkers produce two different strings, and a version banner
/// already does that without this having to know the shape of anyone's.
/// Unreadable — a driver that does not answer `--version` — contributes the
/// program's own path, so the field is never silently empty.
fn version_of(program: &Path) -> String {
    let out = Command::new(program).arg("--version").output();
    match out {
        Ok(out) if out.status.success() => String::from_utf8_lossy(&out.stdout).into_owned(),
        _ => program.display().to_string(),
    }
}

// ---------------------------------------------------------------------------
// The link
// ---------------------------------------------------------------------------

impl CDriver {
    /// The directory the objects are written into — `.buri/link/<link-key>`,
    /// which is only nameable once this linker's version has entered that key.
    pub fn in_dir(mut self, dir: PathBuf) -> CDriver {
        self.dir = dir;
        self
    }

    /// The directory the objects are written into.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The object's filename for one unit, rejecting anything that is not one.
    ///
    /// `Emitted::name` is documented as "stable, deterministic, and a
    /// filename", and this is where that is enforced rather than trusted: a
    /// name carrying a separator would write outside the link directory.
    fn object_path(&self, name: &str) -> Option<PathBuf> {
        let file = Path::new(name).file_name()?;
        (file == Path::new(name).as_os_str()).then(|| self.dir.join(file))
    }

    /// The flags that are the platform's, in the order the design states them.
    fn platform_flags(&self) -> Vec<String> {
        let mut flags: Vec<String> = Vec::new();
        match self.target.platform {
            Platform::Macos => {
                // No `-no_uuid`: macOS 26's dyld refuses a binary without an
                // LC_UUID, and ld64's UUID is a content digest, so keeping it
                // costs no reproducibility.
                flags.push("-Wl,-dead_strip".into());
                // ARCHITECTURE.md §7's third source of nondeterminism, on the
                // linker's side of it. `ld64` records an `N_OSO` stab naming
                // every input that carries debug information, as an absolute
                // path — and the runtime archive carries debug information, so
                // the artifact held `<repository>/.buri/link/<key>/libburi_rt.a`
                // and therefore depended on where the repository was checked out
                // and on which cache key this link had. `-oso_prefix` strips a
                // prefix from those names, and the link runs *in* the link
                // directory (see `Linker::link`), so `.` strips all of it: the
                // recorded name is `libburi_rt.a(...)`, which is a fact about
                // the link rather than about the machine.
                flags.push("-Wl,-oso_prefix,.".into());
                // `-platform_version` is deliberately *not* passed. The driver
                // computes it from the selected SDK and passes its own, and a
                // second one is an error rather than an override; pinning it
                // here would also mean pinning a deployment target the runtime
                // archive was not built for.
            }
            _ => {
                // A build id is a hash of content that is about to be compared
                // byte for byte.
                flags.push("-Wl,--build-id=none".into());
                // `-dead_strip`'s counterpart: the artifact keeps the part of
                // the archive it reaches. Measured by relinking the x86-64
                // Linux CI job's own objects and archive with and without it,
                // debug information removed from both: 373 936 bytes against
                // 673 440. It does not touch `.debug_*` — the linked file is
                // 2.19 MB either way — which is why the test that polices this
                // measures the stripped size.
                flags.push("-Wl,--gc-sections".into());
                // What `std` reaches for. Harmless where glibc has folded them
                // in, and required where it has not — the same three
                // `tests/native/runtime.rs` passes for the same archive.
                flags.push("-lpthread".into());
                flags.push("-ldl".into());
                flags.push("-lm".into());
            }
        }
        flags
    }
}

impl Linker for CDriver {
    fn name(&self) -> &'static str {
        self.flavour.name()
    }

    fn version(&self) -> String {
        self.version.get()
    }

    /// Writes the objects and the runtime archive into the link directory and
    /// runs the driver over them.
    ///
    /// `unchanged` is used for exactly what it can honestly buy. No shipping
    /// linker links incrementally — mold rejected it as irreproducible, LLD
    /// calls it a non-goal, and Zig's Mach-O in-place patcher is unfinished —
    /// so "swap only the files that changed"
    /// happens *above* the linker: an object whose bytes are already on disk is
    /// not rewritten, and the link itself is always full. The saving that
    /// matters is upstream of here, in the codegen units that were never
    /// re-emitted.
    fn link(
        &self,
        units: &[Emitted],
        unchanged: &[usize],
        out: &Path,
        _opts: &LinkOptions<'_>,
    ) -> Result<(), Diagnostics> {
        let mut diagnostics = Diagnostics::new();
        if units.is_empty() {
            diagnostics.push(Diagnostic::error(
                Span::NONE,
                String::from("internal error: the backend emitted no codegen unit to link"),
            ));
            return Err(diagnostics);
        }
        if let Err(e) = std::fs::create_dir_all(&self.dir) {
            diagnostics.push(
                Diagnostic::error(
                    Span::NONE,
                    format!("cannot create {}: {e}", self.dir.display()),
                )
                .with_fix("check the directory exists and is writable"),
            );
            return Err(diagnostics);
        }

        // One unit per worker. Every unit writes its own file and reads nothing
        // any other unit writes, so the only thing the division decides is
        // which core does the `read`, and the order of `objects` — which the
        // command line depends on, and which `parallel::map` preserves. A
        // program of several hundred units is several hundred `open`/`read`
        // round trips over eight megabytes.
        let skip: std::collections::HashSet<usize> = unchanged.iter().copied().collect();
        let staged: Vec<Result<PathBuf, Diagnostic>> = crate::parallel::map(units.len(), |i| {
            let Some(unit) = units.get(i) else {
                return Err(Diagnostic::error(
                    Span::NONE,
                    String::from("internal error: the unit list changed while it was being staged"),
                ));
            };
            let Some(path) = self.object_path(&unit.name) else {
                return Err(Diagnostic::error(
                    Span::NONE,
                    format!("internal error: {:?} is not a codegen unit filename", unit.name),
                ));
            };
            // An unchanged unit's bytes came from the cache, so the file on
            // disk — if there is one — already holds them. Everything else is
            // written unconditionally.
            //
            // `unchanged` is a hint about work to skip, so the bytes are
            // checked rather than taken on the caller's word.
            let already = skip.contains(&i) && already_holds(&path, &unit.bytes);
            if !already {
                if let Err(e) = std::fs::write(&path, &unit.bytes) {
                    return Err(Diagnostic::error(
                        Span::NONE,
                        format!("cannot write {}: {e}", path.display()),
                    ));
                }
            }
            Ok(path)
        });
        let mut objects: Vec<PathBuf> = Vec::with_capacity(units.len());
        for one in staged {
            match one {
                Ok(path) => objects.push(path),
                Err(d) => {
                    diagnostics.push(d);
                    return Err(diagnostics);
                }
            }
        }

        // The decision, taken once and used twice below — the staged file and
        // the command line. `build::actions` asks the same function about the
        // same objects to build the `link` key, which is what makes the key a
        // fact about the link that actually ran.
        let runtime = runtime_archive_for(units);
        if runtime.is_linked() {
            let archive = self.dir.join(runtime_native::ARCHIVE_NAME);
            let stale = std::fs::metadata(&archive).map(|m| m.len()).ok()
                != Some(runtime_native::ARCHIVE.len() as u64);
            if stale {
                if let Err(e) = std::fs::write(&archive, runtime_native::ARCHIVE) {
                    diagnostics.push(Diagnostic::error(
                        Span::NONE,
                        format!("cannot write {}: {e}", archive.display()),
                    ));
                    return Err(diagnostics);
                }
            }
        }

        if let Some(parent) = out.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        // The driver writes inside the link directory, and the bytes are then
        // placed at `out` through the file that is already there.
        //
        // macOS charges about 200 ms the first time a *newly created* file is
        // executed, and the charge is on the file's identity rather than on its
        // contents: a 20 KB executable pays the same as an 8 MB one, and a file
        // that has already been executed once pays nothing when it is truncated
        // and rewritten with different bytes. A linker given the artifact's own
        // path unlinks it and creates a new one, so every rebuild produced a
        // first execution. Writing over the existing file keeps the identity.
        //
        // A `link` cache hit has always reached the artifact this way
        // (`actions::write_executable`), so this makes the two paths agree
        // rather than introducing a way of writing an artifact that did not
        // already exist.
        //
        // The driver is *run in* the link directory and every path it is given
        // is relative to it, which is the other half of ARCHITECTURE.md §7's
        // third source of nondeterminism. `ld64` records an input's path in an
        // `N_OSO` stab for every input that carries debug information, and the
        // runtime archive does: an artifact linked from
        // `.buri/link/<key>/libburi_rt.a` carried that absolute path, so the
        // same objects linked in two directories — which is exactly what
        // `--check-reproducible` arranges — produced two different executables,
        // and two checkouts at two paths produced two different executables for
        // the same reason. Relative names make the recorded path
        // `libburi_rt.a(...)`, which is a fact about the link and not about the
        // machine.
        let staged = self.dir.join("artifact");
        let mut command = Command::new(&self.driver);
        command.current_dir(&self.dir);
        // The parent's environment, unlike every other action this toolchain
        // spawns (`build/spawn.rs` clears it). A C driver finds its assembler,
        // its own linker and its SDK through `PATH`, `DEVELOPER_DIR` and
        // `SDKROOT`, so clearing them would not make the link deterministic —
        // it would make it fail. What is pinned instead is everything that
        // could reach the *bytes*.
        command.env("SOURCE_DATE_EPOCH", spawn::SOURCE_DATE_EPOCH);
        command.env("TZ", "UTC");
        command.env("LC_ALL", "C");
        command.env("ZERO_AR_DATE", "1");
        if let Some(flag) = self.flavour.driver_flag() {
            command.arg(flag);
        }
        command.arg("-o").arg("artifact");
        for path in &objects {
            command.arg(path.file_name().unwrap_or(path.as_os_str()));
        }
        if runtime.is_linked() {
            command.arg(runtime_native::ARCHIVE_NAME);
        }
        command.args(self.platform_flags());

        let spelled = format!("cd {} && {}", self.dir.display(), command_line(&command));
        match command.output() {
            Ok(status) if status.status.success() => {
                let bytes = match std::fs::read(&staged) {
                    Ok(bytes) => bytes,
                    Err(e) => {
                        diagnostics.push(Diagnostic::error(
                            Span::NONE,
                            format!("the link produced no {}: {e}", staged.display()),
                        ));
                        return Err(diagnostics);
                    }
                };
                let _ = std::fs::remove_file(&staged);
                if let Err(e) = place(out, &bytes) {
                    diagnostics.push(
                        Diagnostic::error(
                            Span::NONE,
                            format!("cannot write {}: {e}", out.display()),
                        )
                        .with_fix("check the directory exists and is writable"),
                    );
                    return Err(diagnostics);
                }
                Ok(())
            }
            Ok(out) => {
                let text = String::from_utf8_lossy(&out.stderr);
                let mut d = Diagnostic::error(
                    Span::NONE,
                    format!("the link failed ({})", self.flavour.name()),
                );
                for line in text.lines().take(20) {
                    d = d.with_note(line.to_string());
                }
                diagnostics.push(d.with_note(spelled).with_fix(
                    "if the C compiler does not understand the chosen linker, set \
                     `BURI_LINKER=cc` to use the system default",
                ));
                Err(diagnostics)
            }
            Err(e) => {
                diagnostics.push(
                    Diagnostic::error(
                        Span::NONE,
                        format!("cannot run {}: {e}", self.driver.display()),
                    )
                    .with_fix("install a C toolchain, or set `CC` to one"),
                );
                Err(diagnostics)
            }
        }
    }
}

/// Whether the file already holds exactly these bytes.
///
/// The length is checked first, so the common case — a file whose contents
/// genuinely moved — is not read back before being overwritten. The *bytes*
/// are then compared rather than the length alone: skipping a write is a
/// saving and never a licence to leave stale bytes, which is what
/// `an_unchanged_object_is_left_where_it_was` holds this to.
fn already_holds(path: &Path, bytes: &[u8]) -> bool {
    std::fs::metadata(path).is_ok_and(|m| m.len() == bytes.len() as u64)
        && std::fs::read(path).is_ok_and(|on_disk| on_disk == bytes)
}

/// Writes an executable over whatever is at `path`, keeping that file's
/// identity, and makes it executable.
///
/// `File::create` truncates rather than replaces, which is the whole point: see
/// the note in [`Linker::link`] about what macOS charges for a file that has
/// never been executed before.
pub fn place(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    // A write the file does not need is a write worth not doing. The charge
    // above is on the first execution *after a write*, so an artifact whose
    // bytes did not move — every rebuild whose edit the optimiser removed, and
    // every rebuild of a target the edit did not reach — runs for nothing.
    let settled = already_holds(path, bytes);
    if !settled {
        std::fs::write(path, bytes)?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(path).map(|m| m.permissions().mode() & 0o777).unwrap_or(0);
        if mode != 0o755 {
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))?;
        }
    }
    Ok(())
}

/// The command as a person would type it, for a failure to quote back.
fn command_line(command: &Command) -> String {
    let mut out = command.get_program().to_string_lossy().into_owned();
    for arg in command.get_args() {
        out.push(' ');
        out.push_str(&arg.to_string_lossy());
    }
    out
}

/// The whole link step: the manifest, then the linker.
///
/// Separate from [`Linker::link`] because the manifest is the build system's
/// record and not the linker's input — a linker that ignored `unchanged`
/// entirely would still owe the reader a manifest, and `--explain`'s per-unit
/// lines come from these rows rather than from anything a linker returns.
pub fn run(
    units: &[Emitted],
    rows: &[Row],
    linker: &CDriver,
    out: &Path,
    opts: &LinkOptions<'_>,
) -> Result<(), Diagnostics> {
    let _ = std::fs::create_dir_all(linker.dir());
    let _ = std::fs::write(linker.dir().join("manifest"), manifest_text(rows));
    let unchanged: Vec<usize> =
        rows.iter().enumerate().filter(|(_, r)| r.cached).map(|(i, _)| i).collect();
    linker.link(units, &unchanged, out, opts)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows() -> Vec<Row> {
        vec![
            Row { unit: "core_list".into(), key: "3f9a1c2b8d4e".into(), cached: true },
            Row { unit: "main".into(), key: "8b2e01f4c7a9".into(), cached: false },
        ]
    }

    /// The manifest is the answer to "which objects changed", so it has to
    /// survive a round trip through a file. Alignment is cosmetic and the
    /// reader must not depend on it, which is what splitting on whitespace
    /// means.
    #[test]
    fn a_manifest_round_trips() {
        let text = manifest_text(&rows());
        assert_eq!(text, "core_list 3f9a1c2b8d4e cached\nmain      8b2e01f4c7a9 run\n");
        let dir = std::env::temp_dir().join(format!("buri-manifest-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("a temporary directory");
        std::fs::write(dir.join("manifest"), &text).expect("writing the manifest");
        assert_eq!(read_manifest(&dir).as_deref(), Some(rows().as_slice()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The substring search the archive decision rests on, at the offsets that
    /// break a hand-rolled one.
    ///
    /// `mentions` compares only where the first byte matches, so the cases that
    /// matter are a match at the very end, a run of first-byte matches that go
    /// nowhere, and a needle longer than what is left.
    #[test]
    fn a_runtime_symbol_is_found_wherever_it_sits() {
        let needle = b"buri_rt_";
        assert!(mentions(b"buri_rt_flush", needle), "a match at offset zero");
        assert!(mentions(b"\0\0_buri_rt_alloc\0", needle), "a match in the middle");
        assert!(mentions(b"xxxburi_rt_", needle), "a match that ends the haystack");
        assert!(!mentions(b"buri_rt", needle), "a prefix of the needle is not the needle");
        assert!(!mentions(b"", needle), "an empty object mentions nothing");
        assert!(!mentions(b"bbbbbbbb", needle), "first-byte matches that go nowhere");
        assert!(!mentions(b"buri_rtX", needle), "one byte short");
        assert!(mentions(b"", b""), "an empty needle is everywhere");
    }

    /// The decision itself, over objects this test writes: the answer is the
    /// symbol prefix and nothing else about the bytes.
    ///
    /// Gated on `AVAILABLE`, because a toolchain with no archive answers
    /// `Omitted` to everything — which is the first clause of the function and
    /// is asserted on its own below.
    #[test]
    fn the_decision_is_whether_an_object_names_a_runtime_symbol() {
        let unit = |bytes: &[u8]| Emitted {
            name: String::from("main.o"),
            key: crate::build::cache::ActionKey::of(b""),
            bytes: bytes.to_vec(),
        };
        assert_eq!(runtime_archive_for(&[]), RuntimeArchive::Omitted);
        if !runtime_native::AVAILABLE {
            assert_eq!(runtime_archive_for(&[unit(b"buri_rt_flush")]), RuntimeArchive::Omitted);
            return;
        }
        assert_eq!(runtime_archive_for(&[unit(b"printf\0main\0")]), RuntimeArchive::Omitted);
        assert_eq!(runtime_archive_for(&[unit(b"\0buri_rt_flush\0")]), RuntimeArchive::Linked);
        // Any one unit is enough: the archive is one file on one command line.
        assert_eq!(
            runtime_archive_for(&[unit(b"printf"), unit(b"buri_rt_alloc")]),
            RuntimeArchive::Linked
        );
    }

    /// A missing manifest is `None` rather than an empty build.
    #[test]
    fn no_manifest_is_not_an_empty_one() {
        let dir = std::env::temp_dir().join(format!("buri-no-manifest-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(read_manifest(&dir), None);
    }

    /// A unit name is a filename. Anything else would write outside the link
    /// directory, and the name comes from a backend rather than from a
    /// constant.
    #[test]
    fn a_unit_name_that_is_a_path_is_refused() {
        let Ok(cc) = select(Target {
            platform: host_platform().unwrap_or(Platform::Linux),
            arch: None,
        })
        .map(|cc| cc.in_dir(PathBuf::from("/tmp/buri-link-test")))
        else {
            // No C compiler on this machine: there is nothing to link with and
            // nothing this test could be about.
            return;
        };
        assert!(cc.object_path("main.o").is_some());
        assert!(cc.object_path("../escape.o").is_none());
        assert!(cc.object_path("a/b.o").is_none());
        assert!(cc.object_path("").is_none());
    }

    /// Two selections in one process ask the linker its version once, and get
    /// the same term.
    ///
    /// Two claims, and both matter. **The same string**, because it is a field
    /// of every `link` key and a run whose second suite keyed on a different
    /// term would store its executable somewhere the first could not find it.
    /// **The same probe**, because that is the whole saving: `buri test //...`
    /// selects a linker per suite, and two `--version` spawns per suite were the
    /// largest single item in a five-suite run.
    #[test]
    fn a_linker_is_asked_its_version_once_per_process() {
        let Some(host) = host_platform() else { return };
        let target = Target { platform: host, arch: None };
        let (Ok(first), Ok(second)) = (select(target), select(target)) else {
            // No C compiler on this machine: there is no linker to have an
            // identity, and `select` says so rather than inventing one.
            return;
        };
        assert!(std::sync::Arc::ptr_eq(&first.version, &second.version));
        let term = first.version();
        assert_eq!(term, second.version());
        assert!(term.starts_with(first.name()), "the flavour is part of the term: {term}");
        // A term that is only the flavour is a term that does not tell two
        // linkers apart, which is the thing it exists to do.
        assert!(term.len() > first.name().len() + 1, "no banner hash in {term}");
    }

    /// mold is ELF-only and refuses macOS by name, so it must never be
    /// offered there whatever is on `PATH`.
    #[test]
    fn mold_is_never_chosen_on_macos() {
        if std::env::var_os("BURI_LINKER").is_some() {
            return;
        }
        assert_ne!(choose(Platform::Macos), Flavour::Mold);
    }

    /// The host is the only thing this can link for, and saying so is what
    /// keeps a declared cross output producing a diagnostic rather than an
    /// object file for the wrong machine.
    #[test]
    fn only_the_host_is_linkable() {
        // Neither JavaScript platform is linked at all, and `Web` is the one
        // that could plausibly have been mistaken for a native target by a
        // predicate spelled `!= Js`.
        assert!(!can_link(Target { platform: Platform::Js, arch: None }));
        assert!(!can_link(Target { platform: Platform::Web, arch: None }));
        let Some(host) = host_platform() else { return };
        assert!(can_link(Target { platform: host, arch: None }));
        assert!(can_link(Target { platform: host, arch: host_arch() }));
        let other = match host {
            Platform::Macos => Platform::Linux,
            _ => Platform::Macos,
        };
        assert!(!can_link(Target { platform: other, arch: None }));
        let wrong = match host_arch() {
            Some(Arch::X86_64) => Arch::Arm64,
            _ => Arch::X86_64,
        };
        assert!(!can_link(Target { platform: host, arch: Some(wrong) }));
    }
}
