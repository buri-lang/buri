//! Checked-in benchmark corpora: the manifest, the digest, discovery, and
//! `--record`.
//!
//! `design/PERFORMANCE.md` §3.1 states the dual scheme this half implements.
//! The short version: a generated corpus buys scale and coverage and cannot
//! silently fall out of the language; a saved one buys **byte-stability over
//! time**, and that is the only thing it buys. Two runs a year apart compile
//! the same bytes, so a difference between them is a difference in the
//! compiler rather than a difference in `generate.rs`.
//!
//! Nothing in this file is timed and nothing in it touches the compiler, which
//! is why it is a separate module: `compiler.rs` stays the protocol and only
//! the protocol. [`load`] returns the **same [`generate::Program`]** a
//! generator returns, so `measure`, `validate` and every row type are
//! untouched and a saved corpus is not a second code path.
//!
//! # Layout
//!
//! ```text
//! cli/benches/corpora/
//!   README.md
//!   mixed-10k/
//!     manifest.txt
//!     src/m0000.buri
//!     src/m0001.buri
//!     src/main.buri
//! ```
//!
//! `src/` rather than files at the top so that `manifest.txt` can never be
//! confused for a module and the size cap is computed over one subtree.
//!
//! # Module order is load-bearing
//!
//! `Loader::load_source_in` returns early on a path it has already seen, so
//! imports must be loaded before importers. The manifest records nothing about
//! order: the loader relies on **filename** order, which the generator already
//! produces (`m0000` … `m9999`) and which [`record`] preserves, with
//! `main.buri` last by an explicit rule rather than by an accident of ASCII.

#![allow(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    reason = "benchmark harness; the same reasoning as `generate.rs`. Every \
              index here is into a slice this file just built."
)]

use std::path::{Path, PathBuf};

use crate::generate::{self, Family, Module, Params, Program};

/// Per-corpus size cap.
///
/// At roughly 35 bytes per non-blank line, this is a 15k-line corpus. Above it,
/// generate: the repository's whole history is 15 MB, and a 3.5 MB corpus
/// re-recorded once would be a quarter of that.
pub const MAX_CORPUS_BYTES: u64 = 512 * 1024;

/// Total cap over `cli/benches/corpora`. Enforced in `--record` and reported in
/// the `--validate` footer, so it is a number somebody watches rather than a
/// limit somebody discovers.
pub const MAX_TOTAL_BYTES: u64 = 2 * 1024 * 1024;

/// The provenance record beside a saved corpus.
///
/// Plain `key = value` lines, not textproto: coupling the benchmark's
/// provenance to a build-file format that may itself change is an avoidable
/// dependency between two things that have nothing to do with each other.
pub struct Manifest {
    pub name: String,
    /// The profile in `generate.rs` this was generated from.
    pub profile: String,
    /// [`generate::GENERATOR_REVISION`] at the time it was recorded. An older
    /// value is legal and expected — byte-stability across a generator change
    /// is the point — so it is a note, never an error.
    pub generator_revision: u32,
    /// Bumped on every re-record. `--json` carries it so a tracking script sees
    /// a break in the series rather than a step in it.
    pub revision: u32,
    pub recorded: String,
    /// Every non-default parameter, sorted. A profile is a small delta from the
    /// default, so the delta *is* the definition.
    pub params: String,
    pub lines: usize,
    pub bytes: usize,
    pub modules: usize,
    pub digest: String,
}

impl Manifest {
    pub fn family(&self) -> Family {
        generate::profile(&self.profile).map_or(Family::Stress, |(f, _)| f)
    }

    fn render(&self) -> String {
        format!(
            "# Provenance for a checked-in benchmark corpus. See\n\
             # cli/benches/corpora/README.md and design/PERFORMANCE.md §3.1.\n\
             name = {}\n\
             profile = {}\n\
             generator_revision = {}\n\
             revision = {}\n\
             recorded = {}\n\
             params = {}\n\
             lines = {}\n\
             bytes = {}\n\
             modules = {}\n\
             digest = {}\n",
            self.name,
            self.profile,
            self.generator_revision,
            self.revision,
            self.recorded,
            self.params,
            self.lines,
            self.bytes,
            self.modules,
            self.digest
        )
    }

    fn parse(text: &str, at: &Path) -> Result<Manifest, String> {
        let mut m = Manifest {
            name: String::new(),
            profile: String::new(),
            generator_revision: 0,
            revision: 0,
            recorded: String::new(),
            params: String::new(),
            lines: 0,
            bytes: 0,
            modules: 0,
            digest: String::new(),
        };
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                return Err(format!("{}: `{line}` is not `key = value`", at.display()));
            };
            let (key, value) = (key.trim(), value.trim());
            let num = |v: &str| v.parse::<u64>().unwrap_or(0);
            match key {
                "name" => m.name = value.to_string(),
                "profile" => m.profile = value.to_string(),
                "generator_revision" => m.generator_revision = num(value) as u32,
                "revision" => m.revision = num(value) as u32,
                "recorded" => m.recorded = value.to_string(),
                "params" => m.params = value.to_string(),
                "lines" => m.lines = num(value) as usize,
                "bytes" => m.bytes = num(value) as usize,
                "modules" => m.modules = num(value) as usize,
                "digest" => m.digest = value.to_string(),
                // An unknown key is a manifest written by a newer toolchain.
                // Ignoring it is what lets a field be added without every
                // older checkout failing to read a corpus it can still compile.
                _ => {}
            }
        }
        if m.name.is_empty() {
            return Err(format!("{}: no `name`", at.display()));
        }
        Ok(m)
    }
}

/// The root of the checked-in corpora, resolved at compile time so that the
/// benchmark finds them from any working directory.
pub fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("benches").join("corpora")
}

/// Every corpus directory under `root`, in name order.
pub fn discover(root: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(root) else { return Vec::new() };
    let mut out: Vec<PathBuf> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.join("manifest.txt").is_file())
        .collect();
    out.sort();
    out
}

/// A corpus's manifest, without reading its source.
pub fn manifest(dir: &Path) -> Result<Manifest, String> {
    let path = dir.join("manifest.txt");
    let text =
        std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    Manifest::parse(&text, &path)
}

/// Read a saved corpus into the same [`Program`] a generator returns.
///
/// Files are read here, at work-list construction — exactly where the harness
/// calls `generate::program` for the generated half — so that no file is read
/// inside a timer, which is what `design/PERFORMANCE.md` §2 requires.
pub fn load(dir: &Path) -> Result<(Manifest, Program), String> {
    let manifest = manifest(dir)?;

    let src = dir.join("src");
    let entries = std::fs::read_dir(&src).map_err(|e| format!("{}: {e}", src.display()))?;
    let mut files: Vec<PathBuf> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "buri"))
        .collect();
    // Filename order, with `main` last. Explicit rather than relying on `a`
    // sorting above the digits, because the whole program's loadability rests
    // on it.
    files.sort_by(|a, b| {
        let key = |p: &Path| {
            let stem = p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            (stem == "main", stem)
        };
        key(a).cmp(&key(b))
    });
    if files.is_empty() {
        return Err(format!("{}: no `.buri` files", src.display()));
    }

    let mut modules = Vec::new();
    for f in &files {
        let stem = f.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let text = std::fs::read_to_string(f).map_err(|e| format!("{}: {e}", f.display()))?;
        modules.push(Module { path: format!("//bench/{stem}"), text });
    }
    let program = Program { modules };

    let found = digest(&program);
    if !manifest.digest.is_empty() && found != manifest.digest {
        return Err(format!(
            "{}: the files do not match the manifest's digest ({} recorded, {found} on disk); \
             re-record with `--record={}` and bump `revision`",
            dir.display(),
            manifest.digest,
            manifest.name
        ));
    }
    Ok((manifest, program))
}

/// The corpus digest: `hash_bytes` over the module path, a NUL, the bytes, and
/// a NUL, for every module in sorted path order.
///
/// `buri::build::cache::hash_bytes` rather than a hash written here — it is the
/// same SHA-256 the build system trusts for every cache key, it is already
/// public, and a benchmark inventing its own digest would be a second answer to
/// a question the repository has already answered.
///
/// Over the *module* path rather than the repository-relative one, so that
/// moving `cli/benches/corpora` does not invalidate every manifest in it.
pub fn digest(program: &Program) -> String {
    let mut sorted: Vec<&Module> = program.modules.iter().collect();
    sorted.sort_by(|a, b| a.path.cmp(&b.path));
    let mut data: Vec<u8> = Vec::new();
    for m in sorted {
        data.extend_from_slice(m.path.as_bytes());
        data.push(0);
        data.extend_from_slice(m.text.as_bytes());
        data.push(0);
    }
    buri::build::cache::hash_bytes(&data)
}

/// Write a corpus and its manifest.
///
/// Refuses to overwrite an existing directory unless `BURI_BLESS=1` — the
/// convention `cli/tests/formatting.rs` already uses — and refuses if the write
/// would take `cli/benches/corpora` over either cap. Never runs a timer.
pub fn record(
    root: &Path,
    name: &str,
    profile: &str,
    params: &Params,
    program: &Program,
) -> Result<Manifest, String> {
    let dir = root.join(name);
    let bless = std::env::var("BURI_BLESS").is_ok_and(|v| v == "1");
    if dir.exists() && !bless {
        return Err(format!(
            "{} already exists; re-recording is a break in the series, so it is deliberate: \
             set BURI_BLESS=1 and bump `revision` in the manifest",
            dir.display()
        ));
    }

    let size = program.bytes() as u64;
    if size > MAX_CORPUS_BYTES {
        return Err(format!(
            "{name} is {} KiB, over the {} KiB per-corpus cap; generate it instead \
             (design/PERFORMANCE.md §3.1)",
            size / 1024,
            MAX_CORPUS_BYTES / 1024
        ));
    }
    let existing = total_bytes(root) - dir_bytes(&dir);
    if existing + size > MAX_TOTAL_BYTES {
        return Err(format!(
            "recording {name} ({} KiB) would take cli/benches/corpora to {} KiB, over the {} KiB \
             total cap; delete a corpus or generate this one",
            size / 1024,
            (existing + size) / 1024,
            MAX_TOTAL_BYTES / 1024
        ));
    }

    let previous = std::fs::read_to_string(dir.join("manifest.txt"))
        .ok()
        .and_then(|t| Manifest::parse(&t, &dir).ok())
        .map_or(0, |m| m.revision);

    let src = dir.join("src");
    if src.exists() {
        std::fs::remove_dir_all(&src).map_err(|e| format!("{}: {e}", src.display()))?;
    }
    std::fs::create_dir_all(&src).map_err(|e| format!("{}: {e}", src.display()))?;
    for m in &program.modules {
        let stem = m.path.rsplit('/').next().unwrap_or("module");
        let path = src.join(format!("{stem}.buri"));
        std::fs::write(&path, &m.text).map_err(|e| format!("{}: {e}", path.display()))?;
    }

    let manifest = Manifest {
        name: name.to_string(),
        profile: profile.to_string(),
        generator_revision: generate::GENERATOR_REVISION,
        revision: previous + 1,
        recorded: today(),
        params: params.delta(),
        lines: program.lines(),
        bytes: program.bytes(),
        modules: program.modules.len(),
        digest: digest(program),
    };
    let path = dir.join("manifest.txt");
    std::fs::write(&path, manifest.render()).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(manifest)
}

/// Bytes of `.buri` source under `root`, which is what the cap is stated over.
pub fn total_bytes(root: &Path) -> u64 {
    discover(root).iter().map(|d| dir_bytes(d)).sum()
}

fn dir_bytes(dir: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(dir.join("src")) else { return 0 };
    entries
        .filter_map(|e| e.ok())
        .filter_map(|e| e.metadata().ok())
        .filter(|m| m.is_file())
        .map(|m| m.len())
        .sum()
}

/// Today, as `YYYY-MM-DD`.
///
/// Hinnant's civil-from-days, which is fifteen lines and exact, rather than a
/// date crate: the dependency bar admits code generators and platform
/// interfaces, and a calendar is neither.
fn today() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let z = (secs / 86_400) as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}")
}
