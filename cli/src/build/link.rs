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
//! linux   cc -fuse-ld=mold  -o A u0.o u1.o libburi_rt.a -Wl,--build-id=none -Wl,--gc-sections \
//!            --target=aarch64-unknown-linux-musl -B musl/lib -L musl/lib -static-pie \
//!            -unwindlib=none -lunwind
//!         cc -fuse-ld=lld   ...                          (mold absent)
//!         cc                ...                          (neither present)
//! macos   cc -fuse-ld=lld   -o A u0.o u1.o libburi_rt.a -Wl,-dead_strip
//!         cc                ...                          (ld64.lld absent)
//! ```
//!
//! **That rule is narrowed on Linux and not reversed.** The extra flags answer
//! one question — *which libc* — with bytes this binary carries
//! ([`crate::build::musl`]) rather than with whatever is in `/usr/lib`, and
//! they answer it by putting one directory in front of the driver's own search
//! path. **Which files a link needs, and in what order, stays entirely the
//! driver's**: clang is what knows that `-static-pie` selects `rcrt1.o` over
//! `crt1.o` and `crtbeginS.o` over `crtbegin.o`, and where `crti.o … crtn.o`
//! bracket the inputs. `-nostdlib` with a hand-assembled crt sequence was the
//! alternative and was rejected on exactly that: five objects in a fixed order
//! is the part of a link that changes with a libc release, and it is already
//! inside the driver.
//!
//! The one library the driver must **not** choose is the unwinder. Its default
//! is glibc's `libgcc_eh.a`, which is not libc-neutral: a link that keeps it
//! ends at `undefined symbol: _dl_find_object`, glibc's unwinder reaching for a
//! glibc-only loader entry point from inside a binary that has just left glibc
//! behind. `-unwindlib=none` drops it and `-lunwind` names the baked
//! `libunwind.a`. `-lgcc` and `-lc` stay the driver's: `-lc` finds the staged
//! `libc.a` because `-L musl/lib` is searched first, and `-lgcc` is compiler
//! builtins — `__addtf3` and its family, arithmetic that knows nothing about a
//! libc, and which **musl's own `libc.a` needs** in order to print a
//! long double.
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
//! # Which libc, on Linux
//!
//! **A Linux artifact is a static-PIE musl executable, or it is a refusal.**
//! ARCHITECTURE.md §9 makes the output's portability a property of the
//! toolchain rather than of the machine that ran it: the executable
//! `buri build` produces has to run on a Linux that is not this one, and a
//! glibc link cannot promise that. A statically linked glibc still dlopens a
//! matching `libnss_*.so` from inside `getaddrinfo`, so the "static" binary
//! resolves no hostnames on the older machine the staticness was for. musl was
//! designed for this link.
//!
//! Three tiers, in order, and `BURI_MUSL` (`baked` | `system` | `off`, and
//! nothing else — `forced_libc` refuses a fourth spelling rather than ignoring
//! it) forces the choice the way `BURI_LINKER` forces the flavour:
//!
//! 1. **Baked** — `cli/build.rs` copied musl's `libc.a`, `libunwind.a` and the
//!    nine crt objects out of this rustc's self-contained directory, and
//!    [`stage_sysroot`] writes them into `<link dir>/musl/lib` for
//!    `-B musl/lib -L musl/lib` to find. Needs a driver that
//!    understands `--target=<musl triple>`, which is [`accepts_target`]'s one
//!    memoized probe.
//! 2. **System** — the host is itself musl (Alpine: `cc -dumpmachine` says so)
//!    or carries a `musl-clang`/`musl-gcc` wrapper. The wrapper becomes the
//!    driver, `-static-pie` still goes on the line, and nothing points it
//!    anywhere: the machine's musl is already the driver's own libc.
//! 3. **Neither** — [`select`] refuses. There is no silent glibc fallback,
//!    because the failure it would hide is invisible until the artifact is
//!    copied to another machine, which is after every test in this repository
//!    has passed.
//!
//! `BURI_MUSL=off` is the escape hatch, and it is the only path on which
//! `-lpthread -ldl -lm` are still passed. They are dropped everywhere else, and
//! that is a hard requirement rather than tidiness: musl folds all three into
//! `libc.a` and ships no `libpthread.a` stub, so `-lpthread` against the baked
//! sysroot is `cannot find -lpthread` and the link ends there.
//!
//! **`-static-pie` and not `-static`**, matching the Linux link line
//! CODEGEN-STENCIL.md states. The stencil and LLVM backends emit
//! position-independent code — the LLVM one through `RelocMode::PIC`, the
//! stencil one through the `.data.rel.ro` constant pool `stencil/mod.rs`
//! argues for — musl ships the `rcrt1.o` that self-relocates a static PIE
//! before `main`, and both mold and ld.lld implement the mode. The same flag
//! in `debug` and in `release`: a dynamic musl executable needs a
//! `ld-musl-*.so.1` that a glibc distribution does not have, so the
//! profile-dependent variant would be a development build that runs in fewer
//! places than the release one.
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
//! .buri/link/<link-key>/musl/lib/*      the baked musl sysroot, on the Linux
//!                                       path that uses it
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
use crate::build::musl::{self, Libc};
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

/// Why this machine has no linker for a target, in the shape a diagnostic
/// wants.
///
/// A `String` was enough while the only two refusals were "wrong platform" and
/// "no `cc`", both of which are one sentence with one remedy. The musl refusal
/// is not: a reader who meets it needs to be told what the rule is
/// (ARCHITECTURE.md §9), which half of the toolchain broke it, and that
/// `BURI_MUSL=off` exists — and folding that into one `message` would put four
/// sentences on the `error:` line, which is the shape this repository's
/// diagnostics are built to avoid.
///
/// One `fix` and not three, because [`Diagnostic::fix`] is one field and "every
/// diagnostic has one": the alternatives are joined into it with "or", and the
/// background lives in `notes`.
///
/// [`Diagnostic::fix`]: crate::diagnostics::Diagnostic::fix
#[derive(Clone, Debug)]
pub struct Refusal {
    /// The `error:` line.
    pub message: String,
    /// The `= ...` lines: what the rule is, and what broke it.
    pub notes: Vec<String>,
    /// The edit that resolves it.
    pub fix: String,
}

impl Refusal {
    fn new(message: impl Into<String>, fix: impl Into<String>) -> Refusal {
        Refusal { message: message.into(), notes: Vec::new(), fix: fix.into() }
    }

    fn with_note(mut self, note: impl Into<String>) -> Refusal {
        self.notes.push(note.into());
        self
    }

    /// The whole refusal as one block of text, for a caller with no
    /// `Diagnostic` to hand — `commands/build.rs`'s `--check-reproducible`
    /// path prints to stderr directly.
    pub fn text(&self) -> String {
        let mut out = format!("error: {}", self.message);
        for note in &self.notes {
            out.push_str("\n  note: ");
            out.push_str(note);
        }
        out.push_str("\n  fix: ");
        out.push_str(&self.fix);
        out
    }
}


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
    /// Which libc this link answers with, decided once in [`select`] and read
    /// in three places: the flags, the staging, and the `link` key. One field
    /// rather than three calls to [`libc_for`], so that a key can never be
    /// built from a different answer than the command line was.
    libc: LibcMode,
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
/// `Err` names the thing that is missing, in the form a diagnostic wants. There
/// are three ways to get one: a target this host cannot link, a `cc` that is
/// not installed — which no fallback story covers, because there is no link at
/// all without a C driver — and a Linux host that cannot produce a hermetic
/// executable ([`libc_for`]).
pub fn select(target: Target) -> Result<CDriver, Refusal> {
    if !can_link(target) {
        return Err(Refusal::new(
            format!("this toolchain cannot link {} artifacts on this machine", target.platform.slug()),
            "build for this machine's platform, or run the build on one of the target's",
        ));
    }
    let name = std::env::var("CC").unwrap_or_else(|_| String::from("cc"));
    let Some(driver) = spawn::resolve(&name) else {
        return Err(Refusal::new(
            format!("no C compiler on PATH to drive the link (looked for `{name}`)"),
            "install a C toolchain — the link is driven through `cc`, which is what knows where \
             this platform's libc and startup files live",
        ));
    };
    // The libc question can replace the driver (the `musl-clang` tier), so it
    // is answered before the identity probe: the term in the `link` key has to
    // be the version of the program that will actually run.
    let (driver, libc) = libc_for(target, driver)?;
    let flavour = choose(target.platform);
    let mut programs = vec![driver.clone()];
    programs.extend(flavour.program(Some(target.platform)).and_then(spawn::resolve));
    let version = identity_of(flavour, programs);
    Ok(CDriver { dir: PathBuf::new(), driver, flavour, target, libc, version })
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
// Which libc
// ---------------------------------------------------------------------------

/// How one link answers "which libc", which on Linux is the whole of whether
/// the artifact is hermetic.
///
/// Not [`musl::Libc`] reused: that enum says what `cli/build.rs` built the
/// *runtime archive* against, which is a fact about this binary, and this says
/// what the *link* is about to do, which is a fact about a command line. The
/// two are compared in [`libc_for`] and disagreeing is the refusal — a
/// comparison that could not be written if one type were doing both jobs.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LibcMode {
    /// musl, from the sysroot baked into this binary and staged into the link
    /// directory. The intended state of every Linux toolchain.
    MuslBaked,
    /// musl, from the machine: an Alpine host, or a `musl-clang`/`musl-gcc`
    /// wrapper that is now the driver. Still `-static-pie`, no sysroot flags.
    MuslSystem,
    /// The host's glibc, and therefore an artifact that runs where that glibc
    /// does. `BURI_MUSL=off` and nothing else reaches this.
    Glibc,
    /// Not a Linux link. macOS, where libSystem is the only answer and there
    /// was never a question.
    NotLinux,
}

impl LibcMode {
    /// The term this mode contributes to the `link` key.
    ///
    /// Spelled out rather than `{:?}`: a `Debug` rendering is a convenience
    /// that a `derive` may change, and this string is a cache key, so two
    /// toolchains that disagree about it would serve each other's artifacts.
    fn key(self) -> &'static str {
        match self {
            LibcMode::MuslBaked => "musl-baked",
            LibcMode::MuslSystem => "musl-system",
            LibcMode::Glibc => "glibc",
            LibcMode::NotLinux => "not-linux",
        }
    }

    /// Whether this link is hermetic, which is the question every caller
    /// outside this module is actually asking.
    pub fn is_musl(self) -> bool {
        matches!(self, LibcMode::MuslBaked | LibcMode::MuslSystem)
    }
}

/// The libc for one link, and the driver that will produce it.
///
/// The driver comes back because tier 2 can replace it: `musl-clang` is a
/// wrapper around the host clang that puts musl's headers and libraries in
/// front of glibc's, and using it means spawning it rather than passing a flag.
///
/// `BURI_MUSL` narrows the candidate list; it does **not** skip the probes, and
/// that is the first of two ways it differs from `BURI_LINKER`.
/// `BURI_LINKER=mold` on a machine with no mold is a link that fails with a
/// message naming mold, which is a fine escape hatch for an escape hatch.
/// `BURI_MUSL=baked` on a toolchain with no baked sysroot would instead be a
/// link that succeeds against the host's glibc — a silent loss of the property
/// the flag was asking for — so the forced choice is checked and refused rather
/// than taken on trust. The second way is [`forced_libc`]: a value that is not
/// one of the three is refused too, for the same reason and against the same
/// mistake made one letter earlier.
fn libc_for(target: Target, driver: PathBuf) -> Result<(PathBuf, LibcMode), Refusal> {
    if target.platform != Platform::Linux {
        // Before the variable is read, and deliberately: `BURI_MUSL` is a
        // question about a Linux link, and a macOS build that refused because
        // of a stale export in someone's shell profile would be refusing a
        // build the variable has nothing to say about.
        return Ok((driver, LibcMode::NotLinux));
    }
    let forced = forced_libc(&std::env::var("BURI_MUSL").unwrap_or_default())?;
    if forced == Forced::Off {
        // The only path that keeps `-lpthread -ldl -lm`, and the only one that
        // is not checked against the runtime archive's own libc: a toolchain
        // whose archive is musl and whose link is glibc will fail in the
        // linker, loudly, which is the answer this flag asked for.
        return Ok((driver, LibcMode::Glibc));
    }
    let triple = crate::compiler::backend::triple_text(target)
        .unwrap_or_else(|| String::from("unknown-unknown-linux-musl"));

    // Tier 1. Bytes first, because the probe costs a process and the bytes
    // cost a `is_empty`.
    if forced != Forced::System && musl::BAKED && accepts_target(&driver, &triple) {
        return agrees(driver, LibcMode::MuslBaked, &triple);
    }
    // Tier 2.
    if forced != Forced::Baked {
        if let Some(system) = system_musl(&driver) {
            return agrees(system, LibcMode::MuslSystem, &triple);
        }
    }
    Err(refusal(&triple).with_note(match (forced, musl::BAKED) {
        (Forced::Baked | Forced::Unset, false) => {
            "this toolchain baked no musl sysroot, so it has nothing to link against"
        }
        (Forced::Baked, true) => {
            "this toolchain baked a musl sysroot, but its C driver does not understand \
             `--target=` and so cannot be pointed at one"
        }
        (Forced::System, _) => "`BURI_MUSL=system` was set and no musl driver is on PATH",
        // `Forced::Off` returned above, and the remaining case is an unset
        // variable on a toolchain that baked a sysroot its driver refused.
        _ => "no musl driver is on PATH either",
    }))
}

/// What `BURI_MUSL` was set to, once, in a form the tiers below can be matched
/// against.
///
/// An enum rather than the string compared four times: "not one of the three"
/// is then a case the compiler makes [`forced_libc`] answer, instead of a
/// spelling that matches no comparison and lands on the unset behaviour.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Forced {
    /// Unset. The three tiers are tried in order and the first that answers
    /// wins.
    Unset,
    /// `baked`. Tier 1 only.
    Baked,
    /// `system`. Tier 2 only.
    System,
    /// `off`. The glibc escape hatch, which is not a tier at all.
    Off,
}

/// `BURI_MUSL`'s value, and a refusal for anything that is not one of the
/// three.
///
/// **A value this does not know is a hard error**, which is [`libc_for`]'s own
/// argument applied to the flag rather than to the toolchain. That function's
/// doc says the forced choice is *checked* and not taken on trust, because
/// `BURI_MUSL=baked` on a toolchain with no baked sysroot would otherwise be a
/// link that quietly succeeded against the host's glibc — a silent loss of the
/// property the flag was asking for. `BURI_MUSL=bakde` is that case with two
/// letters transposed: it matches nothing, every comparison in `libc_for`
/// falls through, and the build takes the unset path while the person who
/// typed it believes they pinned the opposite. A typo'd force is exactly the
/// failure the checking was for.
///
/// **`BURI_LINKER` is left as it is, and the asymmetry is on purpose.**
/// [`choose`]'s `match` ends in `_ => {}`, so an unknown flavour is probed for
/// as if the variable were unset — right there and wrong here, because the two
/// mistakes are not the same size. A wrong linker is *loudly* wrong: the
/// flavour enters the `link` key and the link either runs the linker that was
/// named or fails naming it, so nothing about the artifact is in question. A
/// wrong libc is *silently* wrong: the link succeeds, every test on this
/// machine passes, and what was lost — an executable that runs on a Linux that
/// is not this one — is not observable until it is copied to one.
///
/// The unset case is `Ok(Forced::Unset)` and not an error, because an unset
/// variable is how nearly every build reaches here.
fn forced_libc(value: &str) -> Result<Forced, Refusal> {
    match value {
        "" => Ok(Forced::Unset),
        "baked" => Ok(Forced::Baked),
        "system" => Ok(Forced::System),
        "off" => Ok(Forced::Off),
        other => Err(Refusal::new(
            format!("`BURI_MUSL={other}` is not a libc this toolchain can be forced to"),
            "set `BURI_MUSL` to `baked`, `system` or `off`, or unset it and let the \
             toolchain choose",
        )
        .with_note(
            "`baked` is the musl sysroot this binary carries, `system` is a musl driver on \
             PATH, and `off` links against this machine's glibc",
        )
        .with_note(
            "the value is refused rather than ignored: a misspelled force would silently \
             take the path it was set to avoid",
        )),
    }
}

/// [`libc_for`]'s last step: the link's answer must be the archive's answer.
///
/// The runtime archive is half of every Buri executable, and `cli/build.rs`
/// records which libc it was compiled against
/// ([`runtime_native::libc`]). A glibc archive dropped onto a musl command line
/// is not a portability question — it is `undefined reference to
/// `__libc_start_main`` at best and a binary that segfaults in `getaddrinfo` at
/// worst — so the disagreement is refused here rather than discovered by
/// whoever runs the artifact.
///
/// [`Libc::Absent`] agrees with everything, because it is the answer of a
/// toolchain with no archive at all: there is nothing for the link to
/// contradict.
fn agrees(
    driver: PathBuf,
    mode: LibcMode,
    triple: &str,
) -> Result<(PathBuf, LibcMode), Refusal> {
    match runtime_native::libc() {
        Libc::Glibc => Err(refusal(triple)
            .with_note("this toolchain's runtime archive was built against glibc")),
        Libc::MuslBaked | Libc::MuslSystem | Libc::Absent => Ok((driver, mode)),
    }
}

/// The shared skeleton of the hermetic-link refusal.
///
/// One wording with one `note` slot, because there is one rule being broken and
/// several ways to break it: a reader who typed `buri build` cares first that
/// the artifact would not have been portable, and only then which half of the
/// toolchain is why.
fn refusal(triple: &str) -> Refusal {
    Refusal::new(
        "this toolchain cannot produce a hermetic Linux executable",
        format!(
            "`rustup target add {triple}` and rebuild the toolchain, or install `musl-tools` \
             (Debian/Ubuntu) for a system musl driver"
        ),
    )
    .with_note(
        "Linux artifacts are statically linked against musl so that they run in any Linux \
         environment (design/native/ARCHITECTURE.md §9)",
    )
    .with_note(
        "`BURI_MUSL=off` links against this machine's glibc instead, and the executable then \
         runs only where that glibc does",
    )
}

/// Whether the driver can be pointed at a triple.
///
/// The question is asked by **compiling** a translation unit for it rather than
/// by matching the driver's name or its `--version` banner. `--target=` is
/// clang's spelling and gcc rejects it outright, but "is this clang" is not the
/// property that matters — the property is that this driver will accept the
/// flag and produce code for that triple, and a `cc` that is a wrapper script,
/// a ccache shim, or a cross toolchain with its own opinions can answer that
/// differently from what its name suggests.
///
/// The source includes nothing, so no sysroot and no headers are needed: the
/// probe is about the driver's own target support and must not fail merely
/// because musl's `stdio.h` is not installed. `-c -o /dev/null` because the
/// object is not wanted and a temporary file would be one more thing to clean
/// up.
///
/// Memoized for the process, for [`PROBED`]'s reason: `buri test //...` selects
/// a linker per suite, and this would otherwise be one `clang -c` per suite.
fn accepts_target(driver: &Path, triple: &str) -> bool {
    static ASKED: std::sync::Mutex<Vec<(String, bool)>> = std::sync::Mutex::new(Vec::new());
    let name = format!("{}\u{0}{triple}", driver.display());
    if let Ok(table) = ASKED.lock() {
        if let Some((_, answer)) = table.iter().find(|(key, _)| *key == name) {
            return *answer;
        }
    }
    let answer = probe_target(driver, triple);
    if let Ok(mut table) = ASKED.lock() {
        table.push((name, answer));
    }
    answer
}

/// [`accepts_target`] without the memo.
fn probe_target(driver: &Path, triple: &str) -> bool {
    use std::io::Write as _;
    let spawned = Command::new(driver)
        .arg(format!("--target={triple}"))
        .args(["-c", "-x", "c", "-", "-o", "/dev/null"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
    let Ok(mut child) = spawned else { return false };
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(b"int buri_musl_probe(void) { return 0; }\n");
    }
    child.wait().is_ok_and(|status| status.success())
}

/// The machine's own musl driver, if it has one.
///
/// Two shapes, and the first is not a program at all. On Alpine the *host* is
/// musl: `cc` is already a musl compiler, `cc -dumpmachine` says
/// `<arch>-alpine-linux-musl`, and there is nothing to install and nothing to
/// swap. Everywhere else the wrapper is a separate program, and
/// `musl-clang` is preferred over `musl-gcc` because `-static-pie` under gcc
/// wants a `libgcc` built for musl and Debian's is not.
///
/// The `-dumpmachine` spawn is only reached when tier 1 has already failed,
/// which on a correctly built Linux toolchain is never — so it is not on the
/// path any normal build takes and does not need the memo [`accepts_target`]
/// has.
fn system_musl(driver: &Path) -> Option<PathBuf> {
    let host = Command::new(driver).arg("-dumpmachine").output().ok();
    let musl_host = host.is_some_and(|out| {
        out.status.success() && String::from_utf8_lossy(&out.stdout).contains("musl")
    });
    if musl_host {
        return Some(driver.to_path_buf());
    }
    ["musl-clang", "musl-gcc"].iter().find_map(|name| spawn::resolve(name))
}

/// Writes the baked musl sysroot into `<dir>/musl/lib`.
///
/// The names are musl's own and [`musl::FILES`] carries them, because a driver
/// given `-B <dir>` looks for `rcrt1.o` and its siblings by exactly those
/// names.
///
/// The "write only if the size differs" guard is the one [`Linker::link`] uses
/// for the runtime archive, and it is worth more here: this is eleven files and
/// about 6.6 MB, written into a directory that a `--watch` loop reuses on every
/// pass. The *size* rather than the bytes, unlike [`already_holds`], because
/// these are constants of this binary — a file in the link directory whose
/// length matches came from this same `include_bytes!` and cannot be a
/// different sysroot that happens to weigh the same.
pub fn stage_sysroot(dir: &Path) -> std::io::Result<()> {
    let lib = dir.join("musl").join("lib");
    std::fs::create_dir_all(&lib)?;
    for (name, bytes) in musl::FILES {
        let path = lib.join(name);
        if std::fs::metadata(&path).map(|m| m.len()).ok() != Some(bytes.len() as u64) {
            std::fs::write(&path, bytes)?;
        }
    }
    Ok(())
}

/// The driver a harness should spawn to link the way the product does, and the
/// arguments it should pass.
///
/// **A harness that links more permissively than the product is a harness that
/// cannot see the product's bugs.** That sentence is already in
/// `tests/native/stencil.rs`, written the day a hand-rolled `cc -o out *.o
/// libburi_rt.a` passed every program in the file while `buri test` failed 977
/// of 997 conformance tests on the same emitter — the harness had left off
/// `-dead_strip`. Nine harnesses across five test files were each spelling the
/// product's flags out again, which means nine places for the next flag to be
/// added in eight of them; this is the one place.
///
/// [`product_link_args`] **stages** whatever the link needs into `dir` — today
/// the musl sysroot — and returns the non-object arguments in the order the
/// product passes them, which is *after* the objects and the archive. The paths
/// in them are relative, exactly as they are in the product, so the command
/// must be run with `current_dir(dir)`; that is not a quirk of the helper but
/// the reproducibility discipline of [`Linker::link`] itself
/// (ARCHITECTURE.md §7), and a harness that ran the driver somewhere else would
/// again be linking differently from the product.
///
/// An empty list on a host that cannot link at all — no `cc`, or the hermetic
/// refusal. That is not permissiveness: the product cannot link there either,
/// and the harness's own driver invocation is about to fail for the same
/// reason.
pub fn product_link_args(dir: &Path) -> Vec<String> {
    let Some(platform) = host_platform() else { return Vec::new() };
    let target = Target { platform, arch: host_arch() };
    let Ok(driver) = select(target) else { return Vec::new() };
    if driver.libc == LibcMode::MuslBaked {
        let _ = stage_sysroot(dir);
    }
    driver.platform_flags()
}

/// The driver [`product_link_args`]'s arguments are for.
///
/// A separate function and not a tuple, because most callers want only the
/// arguments: `$CC` is the driver on every host but one. The exception is the
/// `musl-clang` tier, where the *program* is the answer to "which libc" and a
/// harness that kept spawning `$CC` would link against glibc while the product
/// linked against musl — the divergence this pair exists to close.
pub fn product_link_driver() -> Option<PathBuf> {
    let platform = host_platform()?;
    select(Target { platform, arch: host_arch() }).ok().map(|cc| cc.driver)
}

/// The arguments a **compile** needs in order to agree with the link
/// [`product_link_args`] describes.
///
/// The pair above answers "how does the product link"; a harness that also
/// *compiles* — the C probe `tests/native/llvm.rs` and `tests/native/stencil.rs`
/// link beside the emitted objects — has a second question, and until this
/// function it answered it by accident. The probe was compiled by the product's
/// driver for the driver's **own default target**, which on a Debian host is
/// glibc, and then linked against the baked musl sysroot. It works, and it
/// works for a reason that does not generalize: those probes are a dozen lines
/// that call `printf` and touch no type whose layout a libc chooses. The first
/// probe to name a `struct stat`, a `pthread_mutex_t` or an `off_t` would be
/// compiled against one libc's idea of it and linked against another's, and the
/// symptom would be a corrupt field rather than an error.
///
/// **Only the baked tier has anything to say.** There the driver is the host's
/// own `cc` and `--target=` is the entire mechanism by which it is pointed at
/// musl (`CDriver::libc_flags`), so a compile that omits it is compiling for
/// a different target from the one being linked for. On the `musl-clang` tier
/// the driver *is* the musl compiler — that is what tier 2 selects it for — and
/// there is nothing to point. On `BURI_MUSL=off`, and on macOS, the compile and
/// the link already name one target and a flag here would be the divergence
/// rather than the fix.
///
/// Empty on a host that cannot link at all, for [`product_link_args`]'s reason:
/// the compile that follows is about to fail for the same cause.
pub fn product_compile_args() -> Vec<String> {
    let Some(platform) = host_platform() else { return Vec::new() };
    let target = Target { platform, arch: host_arch() };
    let Ok(driver) = select(target) else { return Vec::new() };
    if driver.libc != LibcMode::MuslBaked {
        return Vec::new();
    }
    // The same rendering the link's `--target=` is built from, and taken from
    // the same function: two spellings of one triple would be the divergence
    // this exists to close.
    crate::compiler::backend::triple_text(target)
        .map(|triple| vec![format!("--target={triple}")])
        .unwrap_or_default()
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

    /// Where a finished link's bytes are kept until something takes them.
    ///
    /// The driver writes `artifact`, under a name every process linking this
    /// key shares (the directory is the key), and a successful link
    /// immediately renames that to this — a name carrying *this* process's id,
    /// so the file the caller is then handed cannot be truncated out from
    /// under it by a second build of the same key.
    ///
    /// Not the driver's `-o` argument, deliberately: the driver is given the
    /// relative name `artifact` because a name that varies is a name that
    /// could vary the bytes, and ARCHITECTURE.md §7's byte-for-byte rebuild
    /// check is a promise this file does not test its luck against.
    fn claimed(&self) -> PathBuf {
        self.dir.join(format!("artifact.{}", std::process::id()))
    }

    /// Which libc this link will answer with.
    ///
    /// Public because it is what a test asks to know whether it is looking at a
    /// hermetic link, and because a harness that reproduces the product's
    /// command line has to be able to say which one it reproduced.
    pub fn libc(&self) -> LibcMode {
        self.libc
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
                flags.extend(self.libc_flags());
            }
        }
        flags
    }

    /// The half of [`platform_flags`] that answers "which libc", which on
    /// Linux is the half that decides whether the artifact is hermetic.
    ///
    /// [`platform_flags`]: CDriver::platform_flags
    fn libc_flags(&self) -> Vec<String> {
        let mut flags: Vec<String> = Vec::new();
        match self.libc {
            LibcMode::MuslBaked => {
                // `--target=` is what makes clang select musl's crt sequence
                // — `rcrt1.o crti.o crtbeginS.o … crtendS.o crtn.o`, in that
                // order — rather than the host glibc's; `-B` and `-L` are what
                // make it find them in the copy this binary carries instead of
                // in `/usr/lib`. Both prefixes are searched *before* the
                // driver's own directories, which is the whole mechanism.
                //
                // **No `--sysroot=musl`, and that is a measurement rather than
                // an omission.** It was the obvious flag and it makes the link
                // fail: clang locates its GCC installation relative to the
                // sysroot, a staged directory has none, and the link ended at
                // `mold: fatal: cannot open crtbeginS.o` — then, once those
                // were baked too, at `mold: fatal: library not found: gcc`.
                // The flag buys nothing the two prefixes do not already buy,
                // because a link reads no headers.
                //
                // Every path is **relative**, and the driver is run with
                // `current_dir` set to the link directory (`Linker::link`).
                // That is the same reproducibility discipline the object names
                // are under (ARCHITECTURE.md §7): an absolute `-L` would put
                // the checkout's path on the command line, and a command line
                // is what `--check-reproducible` compares two of.
                if let Some(triple) = crate::compiler::backend::triple_text(self.target) {
                    flags.push(format!("--target={triple}"));
                }
                flags.push("-B".into());
                flags.push("musl/lib".into());
                flags.push("-L".into());
                flags.push("musl/lib".into());
                flags.push("-static-pie".into());
                // **The unwinder is the one library the driver must not
                // choose**, and it is the narrowest place the rule in this
                // file's header could be narrowed to. clang's default for a
                // Linux target is glibc's `libgcc_eh.a`, and that archive is
                // not libc-neutral the way the rest of `libgcc` is: linking it
                // gave `undefined symbol: _dl_find_object, referenced by
                // libgcc_eh.a(unwind-dw2-fde-dip.o)` — glibc's unwinder
                // reaching for a glibc-only loader entry point from inside a
                // binary that has just left glibc behind. `-unwindlib=none`
                // drops it; `-lunwind` names the `libunwind.a` this toolchain
                // baked (`musl::LIBUNWIND_A`, staged beside `libc.a` and found
                // through the `-L` above).
                //
                // `-lgcc` and `-lc` are left to the driver on purpose. `-lc`
                // resolves to the staged `libc.a`, because `-L musl/lib` is
                // searched first. `-lgcc` resolves to the host's, and that is
                // correct rather than tolerated: it is the compiler builtins —
                // `__addtf3`, `__floatsitf`, `__fixunstfsi` — which are
                // arithmetic and know nothing about a libc.
                //
                // **`-nodefaultlibs`, with the builtins taken from
                // `libburi_rt.a`'s own `compiler_builtins`, was tried and is
                // wrong: musl's `libc.a` needs builtins too.** Its
                // `vfprintf.lo` reaches for `__addtf3`, `__netf2`,
                // `__floatsitf` and `__fixunstfsi` to format a long double, and
                // libc is scanned *after* the archive — so a link with no
                // runtime archive on the line, which is every link in
                // `tests/native/link.rs`, ended at four undefined symbols
                // inside libc itself. (`-rtlib=compiler-rt` is not the escape
                // either: Debian ships no `libclang_rt.builtins.a` for a musl
                // triple.)
                //
                // `-lunwind` comes after `libburi_rt.a` on the command line,
                // because `platform_flags` is appended after the inputs and an
                // ELF linker resolves left to right.
                flags.push("-unwindlib=none".into());
                flags.push("-lunwind".into());
            }
            // The machine's musl is already the driver's own libc, so there is
            // nothing to point it at — only the mode to ask for.
            LibcMode::MuslSystem => flags.push("-static-pie".into()),
            LibcMode::Glibc => {
                // **The `BURI_MUSL=off` path, and the only one that keeps
                // these three.** What `std` and the runtime archive reach for,
                // and what macOS gets for free because libSystem is all of
                // them at once. Harmless where glibc has folded them in, and
                // required where it has not — the same three
                // `tests/native/runtime.rs` passes for the same archive.
                //
                // The reason `-lm` was load-bearing survives the flag's
                // removal from every other path: `tokio`'s multi-thread worker
                // calls libm's `pow` (its mean-poll-time estimator), and since
                // the carrier pool made `rt::Launch::launch` reachable,
                // `libburi_rt.a` carries that worker in every Buri program. On
                // glibc that call needs `-lm` and a link without it ends at
                // `undefined reference to 'pow'`. On musl it needs nothing:
                // `pow` is *in* `libc.a`, along with everything `-lpthread`
                // and `-ldl` used to name, and musl ships no `libpthread.a`
                // stub for `-lpthread` to find — so passing them against the
                // baked sysroot is not a harmless extra but `cannot find
                // -lpthread`, a hard error before a single symbol is resolved.
                //
                // They come *after* the archive on the command line
                // (`Linker::link` below), which is the half that matters to a
                // left-to-right ELF linker: `-lm` ahead of `libburi_rt.a`
                // resolves nothing.
                flags.push("-lpthread".into());
                flags.push("-ldl".into());
                flags.push("-lm".into());
            }
            // Unreachable from the `_` arm this is called under, which is
            // Linux; spelled out rather than caught by a wildcard so that a
            // fourth platform cannot silently inherit a Linux libc.
            LibcMode::NotLinux => {}
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

    /// The libc, the sysroot's digest, and the flags — hashed, because the
    /// flags are a list and a key term is a string.
    ///
    /// All three and not one of them. The **mode** is what a reader would
    /// think of first and is the weakest of the three on its own: two
    /// toolchains can both say `musl-baked`. The **sysroot digest** is what
    /// separates them, and it belongs here for the same reason
    /// `runtime_archive_hash` is in this key — a link's output depends on
    /// musl's `libc.a` exactly as it depends on `libburi_rt.a`, so a toolchain
    /// built against a different musl must miss. The **flags** are the
    /// backstop: they are the thing that actually reaches the driver, and a
    /// future flag added on one path and not another would move the bytes
    /// without moving either of the other two terms. `musl::sysroot_hash` is
    /// valid on a toolchain that baked nothing, so no branch is needed for the
    /// empty case.
    fn link_identity(&self) -> String {
        let mut text = String::from(self.libc.key());
        text.push('\u{0}');
        text.push_str(&musl::sysroot_hash());
        for flag in self.platform_flags() {
            text.push('\u{0}');
            text.push_str(&flag);
        }
        hash_bytes(text.as_bytes())
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

        // The libc's own staged files, on the one path that has any. Eleven
        // files under `musl/lib/`, named on the command line as
        // `-B musl/lib -L musl/lib` — see `libc_flags`.
        if self.libc == LibcMode::MuslBaked {
            if let Err(e) = stage_sysroot(&self.dir) {
                diagnostics.push(Diagnostic::error(
                    Span::NONE,
                    format!("cannot write the musl sysroot into {}: {e}", self.dir.display()),
                ));
                return Err(diagnostics);
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
        // What the driver wrote is *claimed* rather than read into memory:
        // renamed to a name this process owns, copied from there to `out`, and
        // handed back to the caller as a [`Staged`] for the cache to move in.
        // See [`Staged`] for why the linked bytes reach the cache by a rename
        // rather than by a second write of the same hundred megabytes.
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
                // Claimed before anything reads it. Two builds of one key share
                // this directory — the directory *is* the key — so `artifact`
                // is a name a second process's driver may truncate at any
                // moment. Renaming it into a name this process owns closes that
                // window to the length of a rename, where reading a hundred
                // megabytes back held it open for the length of the read.
                let claimed = self.claimed();
                if let Err(e) = std::fs::rename(&staged, &claimed) {
                    diagnostics.push(Diagnostic::error(
                        Span::NONE,
                        format!("the link produced no {}: {e}", staged.display()),
                    ));
                    return Err(diagnostics);
                }
                if let Err(e) = place_from(&claimed, out) {
                    // The claim is dropped here rather than left for a
                    // `Staged` the caller will never be given: a failed link
                    // owes the link directory no hundred-megabyte file.
                    let _ = std::fs::remove_file(&claimed);
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
///
/// This is the *object* staging half of that rule, where the bytes are in
/// memory already because the backend just emitted them. The artifact half is
/// [`place_from`], where they are not.
fn already_holds(path: &Path, bytes: &[u8]) -> bool {
    std::fs::metadata(path).is_ok_and(|m| m.len() == bytes.len() as u64)
        && std::fs::read(path).is_ok_and(|on_disk| on_disk == bytes)
}

/// A linked artifact on disk, owned by this process until something takes it.
///
/// Returned by [`run`] so that the caller can hand the bytes to the cache **by
/// moving the file** rather than by writing them a second time. A debug test
/// binary for a real repository is about a hundred megabytes, and it used to
/// reach disk three times per link: once from the driver, once at the output
/// path, and once more as a cache entry copied out of a full read of the
/// output. The third write and the read that fed it are both this type: a
/// `rename` inside `.buri/` costs one directory entry.
///
/// The `Drop` is what makes that safe to hand out. A caller with no use for
/// the file — `--check-reproducible`, which compares two links and keeps
/// neither — drops it and the file goes, so "the cache took it" and "nobody
/// wanted it" are the same code path from the link's point of view. Once
/// [`crate::build::cache::Cache::put_file`] has moved it the removal fails and
/// is ignored, which is the whole of the interaction between the two.
pub struct Staged {
    path: PathBuf,
}

impl Staged {
    /// The file, for the one caller that moves it into the cache.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for Staged {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// A megabyte, which is the unit both halves of [`place_from`] work in.
///
/// Big enough that a hundred-megabyte artifact is a hundred reads rather than
/// a hundred thousand, and small enough that comparing two of them is a buffer
/// and not a copy of the artifact. What it replaces is a `Vec<u8>` per file:
/// the old path held the whole artifact in memory twice — once out of the
/// cache and once out of the file it was being compared with — for the length
/// of the comparison.
const CHUNK: u64 = 1 << 20;

/// Whether two open files hold the same bytes, read a chunk at a time.
///
/// Both files are read from wherever they are positioned, and the caller is
/// what has positioned them. A difference ends the loop at the chunk it is in,
/// so the common failure — an artifact whose first section moved — is not a
/// full read of either file.
fn same_contents(a: &mut std::fs::File, b: &mut std::fs::File) -> std::io::Result<bool> {
    use std::io::Read;
    let mut left = Vec::new();
    let mut right = Vec::new();
    loop {
        left.clear();
        right.clear();
        a.by_ref().take(CHUNK).read_to_end(&mut left)?;
        b.by_ref().take(CHUNK).read_to_end(&mut right)?;
        if left != right {
            return Ok(false);
        }
        if left.is_empty() {
            return Ok(true);
        }
    }
}

/// Copies `src` over whatever is at `dest`, keeping that file's identity, and
/// makes it executable. Answers how many bytes it is.
///
/// `File::create` truncates rather than replaces, which is the whole point: see
/// the note in [`Linker::link`] about what macOS charges for a file that has
/// never been executed before. That is also why this is a copy and not a
/// `hard_link` or an APFS `clonefile` out of the cache, both of which would be
/// free: either one replaces the output's inode, which is the charge this
/// avoids, and a hard link would additionally make the file being executed and
/// the cache entry describing it *the same bytes* — a cache whose entries can
/// be mutated by anything that opens the artifact is not a content-addressed
/// store any more. The output and the entry are separate inodes on every path
/// through this file, and the saving is taken on the write that produced the
/// entry instead ([`Staged`]).
///
/// A write the file does not need is a write worth not doing. The charge above
/// is on the first execution *after a write*, so an artifact whose bytes did
/// not move — every rebuild whose edit the optimiser removed, and every
/// rebuild of a target the edit did not reach — runs for nothing. The length
/// is checked first, so the common case is not compared byte by byte; the
/// *bytes* are then compared rather than the length alone, because skipping a
/// write is a saving and never a licence to leave stale bytes.
pub fn place_from(src: &Path, dest: &Path) -> std::io::Result<u64> {
    let mut source = std::fs::File::open(src)?;
    let len = source.metadata()?.len();
    let settled = match std::fs::File::open(dest) {
        Ok(mut existing) => {
            existing.metadata().is_ok_and(|m| m.len() == len)
                // An error mid-comparison is answered as "not settled", which
                // spends a write rather than trusting a read that did not
                // finish.
                && same_contents(&mut source, &mut existing).unwrap_or(false)
        }
        Err(_) => false,
    };
    if !settled {
        use std::io::Seek;
        // The comparison above left the source wherever it stopped.
        source.rewind()?;
        let mut file = std::fs::File::create(dest)?;
        // `io::copy` over two files is the kernel's copy on both platforms this
        // links for — `fcopyfile` on macOS, `copy_file_range` on Linux — so the
        // bytes do not travel through this process at all.
        std::io::copy(&mut source, &mut file)?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(dest).map(|m| m.permissions().mode() & 0o777).unwrap_or(0);
        if mode != 0o755 {
            std::fs::set_permissions(dest, std::fs::Permissions::from_mode(0o755))?;
        }
    }
    Ok(len)
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
) -> Result<Staged, Diagnostics> {
    let _ = std::fs::create_dir_all(linker.dir());
    let _ = std::fs::write(linker.dir().join("manifest"), manifest_text(rows));
    let unchanged: Vec<usize> =
        rows.iter().enumerate().filter(|(_, r)| r.cached).map(|(i, _)| i).collect();
    linker.link(units, &unchanged, out, opts)?;
    // The bytes are at `out` and they are also still in the link directory,
    // where the caller can move them from rather than write them again. A
    // caller that does not is a caller that drops this, and dropping it is
    // what removes the file — see [`Staged`].
    //
    // The trait method keeps its `()` because the other implementation of it
    // (`backend::js::Concatenate`) has no staged file to hand back: it writes
    // bytes it computed itself, and there is nothing on disk between the
    // computation and the output.
    Ok(Staged { path: linker.claimed() })
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

    /// The libc a link answers with, and the flags that answer it.
    ///
    /// Three claims, and each of them is a bug this repository has already had
    /// or is one flag away from having.
    ///
    /// **macOS asks no libc question.** `Libc::Absent` is a fourth variant
    /// precisely so that "not applicable" is not reported as glibc, and a macOS
    /// link that grew a `-static-pie` or a `--sysroot` would fail on every
    /// developer machine in the repository.
    ///
    /// **`-lpthread` never appears beside a musl sysroot.** musl ships no
    /// `libpthread.a` stub, so it is not a harmless extra: it is
    /// `cannot find -lpthread` before a symbol is resolved.
    ///
    /// **`-static-pie` appears exactly on the musl paths.** It is the whole of
    /// what makes the artifact run on a Linux that is not this one.
    #[test]
    fn the_libc_flags_are_the_libc_the_link_chose() {
        let Some(host) = host_platform() else { return };
        let Ok(cc) = select(Target { platform: host, arch: host_arch() }) else {
            // No C compiler, or a Linux host that cannot link hermetically.
            // Both are refusals rather than links, and both are asserted
            // elsewhere; there is no command line here to be about.
            return;
        };
        let flags = cc.platform_flags();
        let has = |flag: &str| flags.iter().any(|f| f == flag);
        if host == Platform::Macos {
            assert_eq!(cc.libc(), LibcMode::NotLinux, "macOS has no Linux libc question");
            assert!(has("-Wl,-dead_strip"), "the macOS link lost -dead_strip: {flags:?}");
            assert!(!has("-static-pie"), "a Mach-O link asked for -static-pie: {flags:?}");
            assert!(!has("-lunwind"), "a Mach-O link named musl's unwinder: {flags:?}");
            return;
        }
        assert!(has("-Wl,--gc-sections"), "the Linux link lost --gc-sections: {flags:?}");
        if cc.libc().is_musl() {
            assert!(has("-static-pie"), "a musl link is not static-pie: {flags:?}");
            for absent in ["-lpthread", "-ldl", "-lm"] {
                assert!(!has(absent), "{absent} is on a musl command line: {flags:?}");
            }
        }
        if cc.libc() == LibcMode::MuslBaked {
            assert!(has("musl/lib"), "-B/-L do not point at the staged sysroot: {flags:?}");
            assert!(has("-unwindlib=none"), "glibc's unwinder was left on the line: {flags:?}");
            assert!(has("-lunwind"), "the baked unwinder is not named: {flags:?}");
            assert!(
                flags.iter().any(|f| f.starts_with("--target=") && f.ends_with("-linux-musl")),
                "the driver is not pointed at a musl triple: {flags:?}"
            );
        }
        if cc.libc() == LibcMode::Glibc {
            // `BURI_MUSL=off`, the one path that keeps them. `-lm` is
            // load-bearing there: `tokio`'s worker calls `pow`.
            for present in ["-lpthread", "-ldl", "-lm"] {
                assert!(has(present), "{present} left the glibc path: {flags:?}");
            }
        }
    }

    /// `BURI_MUSL` has three values, and a fourth is a refusal rather than a
    /// silent unset.
    ///
    /// **Asked of [`forced_libc`] and not of the environment.** A `set_var`
    /// here would be a data race against every other test in this binary —
    /// they share one process, they run in parallel, and several of them
    /// select a linker — and the parse is the whole of what this claim is
    /// about. What the force then *does* is the test above's subject, and it
    /// reads the variable the way a build does.
    ///
    /// The refusal has to name the offending value, because a typo is only
    /// obvious once it is quoted back, and the three that would have worked,
    /// because a reader who misspelled one does not know which three they were
    /// choosing between.
    #[test]
    fn an_unknown_buri_musl_is_refused_rather_than_ignored() {
        assert_eq!(forced_libc("").unwrap(), Forced::Unset);
        assert_eq!(forced_libc("baked").unwrap(), Forced::Baked);
        assert_eq!(forced_libc("system").unwrap(), Forced::System);
        assert_eq!(forced_libc("off").unwrap(), Forced::Off);
        // The near-misses, which are the values this exists for: a transposed
        // `baked`, a plausible synonym, and the case-shifted spelling of a
        // value that is otherwise right.
        for typo in ["bakde", "musl", "none", "OFF", "off "] {
            let refused = forced_libc(typo).expect_err("a value that is not one of the three");
            let text = refused.text();
            assert!(text.contains(typo), "the refusal does not quote `{typo}`:\n{text}");
            for valid in ["baked", "system", "off"] {
                assert!(text.contains(valid), "the refusal does not name `{valid}`:\n{text}");
            }
        }
    }

    /// The staged sysroot is the bytes this binary carries, under the names
    /// musl's own crt objects have — which is what `-B musl/lib` looks for.
    ///
    /// Skipped where there is nothing baked, because the empty case is the one
    /// [`select`] refuses rather than the one this describes.
    #[test]
    fn the_sysroot_is_staged_under_musls_own_names() {
        if !musl::BAKED {
            return;
        }
        let dir = std::env::temp_dir().join(format!("buri-sysroot-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        stage_sysroot(&dir).expect("staging the sysroot");
        for (name, bytes) in musl::FILES {
            let path = dir.join("musl").join("lib").join(name);
            let on_disk = std::fs::read(&path).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert_eq!(on_disk.len(), bytes.len(), "{name} was staged at the wrong length");
        }
        // Twice is the same eleven files: a `--watch` pass must not rewrite
        // 6.6 MB it already wrote.
        stage_sysroot(&dir).expect("staging the sysroot again");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The harness helper is the product's own command line, not a copy of it.
    ///
    /// A harness that links more permissively than the product is a harness
    /// that cannot see the product's bugs, so what this asserts is identity
    /// with `platform_flags` rather than the presence of any particular flag —
    /// the flags themselves are the test above's subject.
    #[test]
    fn the_harness_helper_is_the_products_own_flags() {
        let Some(host) = host_platform() else { return };
        let Ok(cc) = select(Target { platform: host, arch: host_arch() }) else { return };
        let dir = std::env::temp_dir().join(format!("buri-helper-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a temporary directory");
        assert_eq!(product_link_args(&dir), cc.platform_flags());
        // And it staged what those flags name, so a caller that runs the
        // driver in `dir` finds them.
        if cc.libc() == LibcMode::MuslBaked {
            assert!(dir.join("musl/lib/rcrt1.o").exists(), "the static-PIE crt was not staged");
            assert!(dir.join("musl/lib/libc.a").exists(), "musl's libc.a was not staged");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A harness compiles for the target it links for.
    ///
    /// Only the baked tier has anything to say, and there it says the whole
    /// thing: the driver is the host's own `cc` and the triple is a flag, so a
    /// probe compiled without it is an object for a different target from the
    /// one on the link line. Asserted as *agreement with*
    /// [`CDriver::platform_flags`] rather than against a spelled-out triple,
    /// for the reason the test above gives — two spellings of one triple is the
    /// divergence, not the check.
    #[test]
    fn the_harness_compiles_for_the_target_it_links_for() {
        let Some(host) = host_platform() else { return };
        let Ok(cc) = select(Target { platform: host, arch: host_arch() }) else { return };
        let compile = product_compile_args();
        let named = |flags: &[String]| {
            flags.iter().find(|f| f.starts_with("--target=")).cloned()
        };
        if cc.libc() != LibcMode::MuslBaked {
            // macOS, the `musl-clang` tier and `BURI_MUSL=off` all compile and
            // link with one driver for one target already, and a flag here
            // would be the divergence rather than the fix.
            assert!(compile.is_empty(), "a compile flag escaped the baked tier: {compile:?}");
            return;
        }
        assert_eq!(
            named(&compile),
            named(&cc.platform_flags()),
            "the compile and the link name two different targets"
        );
        assert!(named(&compile).is_some(), "the baked tier named no target at all");
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
