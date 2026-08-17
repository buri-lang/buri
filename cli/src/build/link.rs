//! Turning codegen units into an executable.
//!
//! This is the implementation of [`Linker`] for the native platforms, and the
//! directory layout the link step works in. It lives under `build/` rather than
//! under `backend/` on purpose: `cranelift` and `llvm` both hand their objects
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
//! invoking `ld` directly (CODEGEN-CRANELIFT.md §7.3). The driver is what knows
//! where `crt1.o`, `libc`, and `libSystem.tbd` live, which SDK is selected, and
//! what a `-syslibroot` should be; reimplementing that is reimplementing the
//! part of a toolchain that changes with every OS release. Which *linker* the
//! driver then runs is chosen with `-fuse-ld=`:
//!
//! ```text
//! linux   cc -fuse-ld=mold  -o A u0.o u1.o libburi_rt.a -Wl,--build-id=none -Wl,--gc-sections
//!         cc -fuse-ld=lld   ...                          (mold absent)
//!         cc                ...                          (neither present)
//! macos   cc -fuse-ld=lld   -o A u0.o u1.o libburi_rt.a -Wl,-no_uuid -Wl,-dead_strip
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
//! system linker does the job; CODEGEN-CRANELIFT.md §7.3 prefers the system
//! linker for the opposite reason (it is what every macOS SDK assumption is
//! built around), and either way the fallback is not a failure mode: **there is
//! no case in which a build fails for want of a particular linker**, and no
//! flag that has to be set to get a working one.
//!
//! `BURI_LINKER` forces the choice — `mold`, `lld`, or `cc` — and is the escape
//! hatch for a `cc` too old to understand `-fuse-ld=mold`. It is not a hole in
//! hermeticity: the linker's name and version are in the `link` key
//! ([`Cc::version`]), so two choices are two cache entries rather than one entry
//! that might hold either's bytes.
//!
//! # Reproducibility
//!
//! ARCHITECTURE.md §7 names three sources of nondeterminism, and the two that
//! belong to the linker are closed here rather than compared for:
//!
//! - **`LC_UUID`** is removed with `-no_uuid` on macOS. A content-derived UUID
//!   would be reproducible; Apple's is not, and the flag removes the question.
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
//! .buri/link/<link-key>/libburi_rt.a    the embedded runtime archive
//! ```
//!
//! It exists because a linker takes paths and [`Linker::link`] takes bytes, and
//! because the manifest is what makes "which objects changed" answerable from
//! outside (CODEGEN-CRANELIFT.md §7.4).

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
pub fn manifest_text(rows: &[Row]) -> String {
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
/// One type for all three flavours rather than a `Lld`, a `Mold` and a `Cc`
/// that would differ by a single argument: what varies is one flag and one
/// probe, and three structs whose `link` bodies were copies of each other would
/// be three places for a platform flag to be fixed in two of them.
pub struct Cc {
    /// `.buri/link/<link-key>`: where the objects and the archive are written.
    dir: PathBuf,
    /// The driver: `$CC`, or `cc`.
    driver: PathBuf,
    flavour: Flavour,
    target: Target,
    /// `<flavour>:<sha256 of the driver's and the linker's `--version`>`.
    /// Computed once, in [`select`], because [`Linker::version`] is called per
    /// key and shelling out per key would be a process per build.
    version: String,
}

/// The driver and the linker for one target, probing `PATH`.
///
/// The link directory is *not* a parameter, because it is named by the `link`
/// key and the `link` key is built from [`Linker::version`] — so the linker has
/// to exist before the directory it will work in has a name.
/// [`Cc::in_dir`] closes the loop.
///
/// `Err` names the thing that is missing, in the form a diagnostic's message
/// wants. The only two ways to get one are a target this host cannot link and a
/// `cc` that is not installed — and the second is not a case the fallback story
/// covers, because there is no link at all without a C driver.
pub fn select(target: Target) -> Result<Cc, String> {
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
    let mut identity = String::new();
    identity.push_str(&version_of(&driver));
    if let Some(program) = flavour.program(Some(target.platform)) {
        if let Some(path) = spawn::resolve(program) {
            identity.push_str(&version_of(&path));
        }
    }
    let version = format!("{}:{}", flavour.name(), hash_bytes(identity.as_bytes()));
    Ok(Cc { dir: PathBuf::new(), driver, flavour, target, version })
}

/// The flavour for a platform, honouring `BURI_LINKER` and otherwise probing.
fn choose(platform: Platform) -> Flavour {
    let forced = std::env::var("BURI_LINKER").unwrap_or_default();
    let candidates: &[Flavour] = match platform {
        // mold is ELF-only and refuses macOS by name; `sold`, the Mach-O fork,
        // was archived in November 2024 with its author recommending Apple's
        // linker instead.
        Platform::Macos => &[Flavour::Lld],
        _ => &[Flavour::Mold, Flavour::Lld],
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

impl Cc {
    /// The directory the objects are written into — `.buri/link/<link-key>`,
    /// which is only nameable once this linker's version has entered that key.
    pub fn in_dir(mut self, dir: PathBuf) -> Cc {
        self.dir = dir;
        self
    }

    /// The directory the objects are written into.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn flavour(&self) -> Flavour {
        self.flavour
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
                // LC_UUID is the one field that would differ on every link.
                flags.push("-Wl,-no_uuid".into());
                flags.push("-Wl,-dead_strip".into());
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
                flags.push("-Wl,--gc-sections".into());
                // What `std` reaches for. Harmless where glibc has folded them
                // in, and required where it has not — the same three
                // `tests/runtime_native.rs` passes for the same archive.
                flags.push("-lpthread".into());
                flags.push("-ldl".into());
                flags.push("-lm".into());
            }
        }
        flags
    }
}

impl Linker for Cc {
    fn name(&self) -> &'static str {
        self.flavour.name()
    }

    fn version(&self) -> String {
        self.version.clone()
    }

    /// Writes the objects and the runtime archive into the link directory and
    /// runs the driver over them.
    ///
    /// `unchanged` is used for exactly what it can honestly buy. No shipping
    /// linker links incrementally — mold rejected it as irreproducible, LLD
    /// calls it a non-goal, and Zig's Mach-O in-place patcher is unfinished
    /// (CODEGEN-CRANELIFT.md §7.1) — so "swap only the files that changed"
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
        let mut diags = Diagnostics::new();
        if units.is_empty() {
            diags.push(Diagnostic::error(
                Span::NONE,
                String::from("internal error: the backend emitted no codegen unit to link"),
            ));
            return Err(diags);
        }
        if let Err(e) = std::fs::create_dir_all(&self.dir) {
            diags.push(
                Diagnostic::error(
                    Span::NONE,
                    format!("cannot create {}: {e}", self.dir.display()),
                )
                .with_fix("check the directory exists and is writable"),
            );
            return Err(diags);
        }

        let mut objects: Vec<PathBuf> = Vec::with_capacity(units.len());
        for (i, unit) in units.iter().enumerate() {
            let Some(path) = self.object_path(&unit.name) else {
                diags.push(Diagnostic::error(
                    Span::NONE,
                    format!("internal error: {:?} is not a codegen unit filename", unit.name),
                ));
                return Err(diags);
            };
            // An unchanged unit's bytes came from the cache, so the file on
            // disk — if there is one — already holds them. Everything else is
            // written unconditionally.
            let already = unchanged.contains(&i)
                && std::fs::read(&path).is_ok_and(|on_disk| on_disk == unit.bytes);
            if !already {
                if let Err(e) = std::fs::write(&path, &unit.bytes) {
                    diags.push(Diagnostic::error(
                        Span::NONE,
                        format!("cannot write {}: {e}", path.display()),
                    ));
                    return Err(diags);
                }
            }
            objects.push(path);
        }

        let archive = self.dir.join(runtime_native::ARCHIVE_NAME);
        if runtime_native::AVAILABLE {
            let stale = std::fs::metadata(&archive).map(|m| m.len()).ok()
                != Some(runtime_native::ARCHIVE.len() as u64);
            if stale {
                if let Err(e) = std::fs::write(&archive, runtime_native::ARCHIVE) {
                    diags.push(Diagnostic::error(
                        Span::NONE,
                        format!("cannot write {}: {e}", archive.display()),
                    ));
                    return Err(diags);
                }
            }
        }

        if let Some(parent) = out.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let mut cmd = Command::new(&self.driver);
        // The parent's environment, unlike every other action this toolchain
        // spawns (`build/spawn.rs` clears it). A C driver finds its assembler,
        // its own linker and its SDK through `PATH`, `DEVELOPER_DIR` and
        // `SDKROOT`, so clearing them would not make the link deterministic —
        // it would make it fail. What is pinned instead is everything that
        // could reach the *bytes*.
        cmd.env("SOURCE_DATE_EPOCH", spawn::SOURCE_DATE_EPOCH);
        cmd.env("TZ", "UTC");
        cmd.env("LC_ALL", "C");
        cmd.env("ZERO_AR_DATE", "1");
        if let Some(flag) = self.flavour.driver_flag() {
            cmd.arg(flag);
        }
        cmd.arg("-o").arg(out);
        cmd.args(&objects);
        if runtime_native::AVAILABLE {
            cmd.arg(&archive);
        }
        cmd.args(self.platform_flags());

        let spelled = spell(&cmd);
        match cmd.output() {
            Ok(out) if out.status.success() => Ok(()),
            Ok(out) => {
                let text = String::from_utf8_lossy(&out.stderr);
                let mut d = Diagnostic::error(
                    Span::NONE,
                    format!("the link failed ({})", self.flavour.name()),
                );
                for line in text.lines().take(20) {
                    d = d.with_note(line.to_string());
                }
                diags.push(d.with_note(spelled).with_fix(
                    "if the C compiler does not understand the chosen linker, set \
                     `BURI_LINKER=cc` to use the system default",
                ));
                Err(diags)
            }
            Err(e) => {
                diags.push(
                    Diagnostic::error(
                        Span::NONE,
                        format!("cannot run {}: {e}", self.driver.display()),
                    )
                    .with_fix("install a C toolchain, or set `CC` to one"),
                );
                Err(diags)
            }
        }
    }
}

/// The command as a person would type it, for a failure to quote back.
fn spell(cmd: &Command) -> String {
    let mut out = cmd.get_program().to_string_lossy().into_owned();
    for arg in cmd.get_args() {
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
    linker: &Cc,
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
        assert!(!can_link(Target { platform: Platform::Js, arch: None }));
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
