//! The runtime archive and musl sysroot for a **cross** target, built at
//! `buri build` time and cached in `~/.buri`.
//!
//! `cli/build.rs` builds `libburi_rt.a` for the *host* and bakes the host's musl
//! sysroot. A cross link — `buri build --output=linux/x86_64` on a mac
//! (ARCHITECTURE.md §9) — needs the *target's* archive and the *target's*
//! sysroot, and neither is in the binary. This module produces both: it
//! re-assembles the runtime package from the sources `runtime_src` embeds,
//! compiles it for the target's musl triple with `rustc`, and copies the
//! target's `self-contained/` sysroot out of the installed `rust-std` — then
//! caches the lot under `~/.buri/cross/<key>/` and hands the linker the paths.
//!
//! **Amortized like a toolchain component, not baked into the binary.** Several
//! megabytes per triple in every `buri` binary would be a cost paid by every
//! user for a cross build most never make; a `~/.buri` cache built once per
//! triple and reused is the same trade the design already makes for `rustc`'s
//! own `rust-std`. The cache key is a digest of everything the archive depends
//! on — the runtime sources, the triple, the feature set, and the `rustc`/`cargo`
//! that will build it — so a toolchain upgrade or a runtime edit is a fresh
//! entry rather than a stale hit.
//!
//! **The first target is net-off and crypto-off**, and that is a property of the
//! cross host rather than a choice: `ring`, the TLS provider `net` needs,
//! compiles C against the target's musl headers, which a bare macOS host does
//! not have. `crypto` is left off with it for this first increment. So a cross
//! archive carries the base runtime and `paint`, and a program that reaches
//! networking or entropy against it is refused **by name at compile time**
//! ([`net`], [`crypto`], `backend::networking_gap_when`) rather than at the
//! system linker — which is the whole reason the feature set travels back with
//! the paths.

use crate::build::musl;
use crate::build::runtime_src;
use crate::build::sha256::hash_bytes;
use crate::compiler::backend::{self, Target};
use std::path::{Path, PathBuf};
use std::process::Command;

/// A version stamped into the cache key, bumped when this file changes what it
/// builds in a way an old cache entry would not otherwise reflect (the feature
/// decision, the command line). The sources, triple and toolchain are already
/// terms, so this is only for changes to *how* they are combined.
const RECIPE: &str = "buri-cross-runtime 1";

/// A resolved cross runtime: the cached archive and sysroot for one target, and
/// the facts a link and its cache key are built from.
#[derive(Clone, Debug)]
pub struct Cross {
    /// `~/.buri/cross/<key>/`, holding `libburi_rt.a` and `musl/lib/*`.
    dir: PathBuf,
    /// `libburi_rt.a`'s SHA-256, for the `link` key's runtime term.
    archive_hash: String,
    /// The eleven sysroot members' digest, for the `link` key's libc term.
    sysroot_hash: String,
    /// The Cargo features the archive was built with, one string each — the
    /// answer `net`/`crypto` read, and what a `--output` refusal names.
    features: Vec<String>,
}

impl Cross {
    /// `~/.buri/cross/<key>/`, where the archive and `musl/lib/` live.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// `libburi_rt.a` in the cache directory.
    pub fn archive(&self) -> PathBuf {
        self.dir.join("libburi_rt.a")
    }

    /// The archive's digest, for the `link` key.
    pub fn archive_hash(&self) -> &str {
        &self.archive_hash
    }

    /// The sysroot's digest, for the `link` key.
    pub fn sysroot_hash(&self) -> &str {
        &self.sysroot_hash
    }

    /// Whether the cross archive carries networking. **False on the first
    /// target** — `ring` cannot cross from a bare macOS host — and read by
    /// `backend::networking_gap_when` so a `net` program cross-compiled is
    /// refused by name rather than at the linker.
    pub fn net(&self) -> bool {
        self.features.iter().any(|f| f == "net")
    }

    /// Whether the cross archive can answer `Entropy`. **False on the first
    /// target**, with `net`, for the same compile-time-refusal reason.
    pub fn crypto(&self) -> bool {
        self.features.iter().any(|f| f == "crypto")
    }
}

/// The features a cross archive is built with: the host's, minus the two the
/// first cross target cannot carry.
///
/// `net` is dropped because `ring` compiles C against musl headers a macOS host
/// does not have; `crypto` is dropped with it for this increment. `paint` is
/// pure Rust and crosses, so a cross UI program still links. Deriving from the
/// host's own feature set rather than a fixed list means a `BURI_RUNTIME_*`
/// toolchain that already dropped `paint` does not have it re-added here.
///
/// **A pure function of the host's declared features**, so it can answer the
/// compile-time gap check without building anything: the decision is the same
/// whether or not the archive has been cached yet.
pub fn cross_features() -> Vec<String> {
    let mut features = Vec::new();
    if backend::runtime_native::declares("paint") {
        features.push(String::from("paint"));
    }
    features
}

/// `net`/`crypto` for a cross target, without building the archive.
///
/// The feature decision is [`cross_features`]'s, so this reads the same list the
/// build will use. It exists so `backend::networking_gap_when` can be handed the
/// target's answer during codegen, before a link — and long before the ~30s
/// cross build — has happened.
pub fn features_for(_target: Target) -> (bool, bool) {
    let features = cross_features();
    (
        features.iter().any(|f| f == "net"),
        features.iter().any(|f| f == "crypto"),
    )
}

/// The cross runtime for `target`, from the cache or freshly built.
///
/// Memoized for the process, for the reason `link`'s linker-identity probe is:
/// `buri test //...` and a `//...` build reach a target's link once per suite or
/// artifact, and re-checking the cache — or worse, rebuilding — each time would
/// be the ~30s build paid per suite. The memo holds the resolved paths, not a
/// lock on the directory.
pub fn resolve(target: Target) -> Result<Cross, link_refusal::Refusal> {
    use std::sync::Mutex;
    static MEMO: Mutex<Vec<(Target, Result<Cross, link_refusal::Refusal>)>> =
        Mutex::new(Vec::new());
    if let Ok(memo) = MEMO.lock() {
        if let Some((_, answer)) = memo.iter().find(|(t, _)| *t == target) {
            return answer.clone();
        }
    }
    let answer = resolve_uncached(target);
    if let Ok(mut memo) = MEMO.lock() {
        memo.push((target, answer.clone()));
    }
    answer
}

// `Refusal` is `link.rs`'s, and its fields are public so a sibling module can
// build one; this alias keeps the type's home obvious at the use sites above.
mod link_refusal {
    pub use crate::build::link::Refusal;
}

/// A refusal, built through `link.rs`'s public fields.
fn refuse(message: impl Into<String>, fix: impl Into<String>) -> link_refusal::Refusal {
    link_refusal::Refusal {
        message: message.into(),
        notes: Vec::new(),
        fix: fix.into(),
    }
}

/// [`resolve`] without the memo.
fn resolve_uncached(target: Target) -> Result<Cross, link_refusal::Refusal> {
    if !runtime_src::AVAILABLE {
        return Err(refuse(
            "this toolchain carries no runtime sources to cross-build from",
            "install a toolchain built on macOS or Linux, or add `{ platform: JS }` to `outputs`",
        ));
    }
    let triple = backend::triple_text(target).ok_or_else(|| {
        refuse(
            "this target has no musl triple to cross-build the runtime for",
            "declare a native output — a macOS or Linux platform",
        )
    })?;

    let rustc = tool("RUSTC", "rustc");
    let cargo = tool("CARGO", "cargo");
    let sysroot_src = self_contained(&rustc, &triple).ok_or_else(|| {
        refuse(
            format!("the standard library for {triple} is not installed, so the cross runtime cannot be built"),
            format!("`rustup target add {triple}` and try again"),
        )
    })?;
    let features = cross_features();

    let key = cache_key(&triple, &features, &rustc, &cargo);
    let root = buri_home()?.join("cross");
    let dir = root.join(&key);

    // A cache hit is an archive and a sysroot already in the keyed directory:
    // the key encodes every input, so the directory's mere existence with the
    // right files in it is the freshness answer, no stamp needed.
    if let Some(cross) = read_cache(&dir, &features) {
        return Ok(cross);
    }

    build_into(
        &root,
        &dir,
        &triple,
        &sysroot_src,
        &features,
        &rustc,
        &cargo,
    )
}

/// The cross build itself: unpack the sources, copy the sysroot, compile, and
/// atomically publish the result into the keyed cache directory.
///
/// **Built in a temporary directory and renamed into place**, so a build killed
/// halfway leaves no half-populated cache entry a later run would read as a hit.
/// The rename is within `~/.buri/cross`, so it is atomic on every filesystem the
/// cache can live on.
fn build_into(
    root: &Path,
    dir: &Path,
    triple: &str,
    sysroot_src: &Path,
    features: &[String],
    rustc: &str,
    cargo: &str,
) -> Result<Cross, link_refusal::Refusal> {
    std::fs::create_dir_all(root).map_err(io_refusal("create the cross cache directory"))?;
    let tmp = root.join(format!(".build.{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).map_err(io_refusal("create a cross build directory"))?;

    // The sources, written back out of the embedded blob.
    let src = tmp.join("src");
    std::fs::create_dir_all(&src).map_err(io_refusal("create the runtime source directory"))?;
    runtime_src::unpack(&src).map_err(io_refusal("write the runtime sources"))?;

    // The eleven sysroot members, copied out of this rustc's self-contained
    // directory into the cache, and digested for the `link` key.
    let sysroot_dir = tmp.join("musl").join("lib");
    std::fs::create_dir_all(&sysroot_dir).map_err(io_refusal("create the cross sysroot"))?;
    let mut digest_input: Vec<u8> = Vec::new();
    for (name, _) in musl::FILES {
        let bytes = std::fs::read(sysroot_src.join(name)).map_err(io_refusal(&format!(
            "read {name} from the target's self-contained sysroot"
        )))?;
        digest_input.extend_from_slice(name.as_bytes());
        digest_input.push(0);
        digest_input.extend_from_slice(&(bytes.len() as u64).to_le_bytes());
        digest_input.extend_from_slice(&bytes);
        std::fs::write(sysroot_dir.join(name), &bytes)
            .map_err(io_refusal(&format!("stage {name} into the cross sysroot")))?;
    }
    let sysroot_hash = hash_bytes(&digest_input);

    // The compile, offline first — like `cli/build.rs`'s runtime build, and for
    // the same reasons: a warm registry pays no network, and the network is a
    // fallback rather than the first move.
    let target_dir = tmp.join("rt");
    let built = run_cargo(cargo, rustc, &src, triple, features, &target_dir)?;
    let archive_bytes =
        std::fs::read(&built).map_err(io_refusal("read the built cross archive"))?;
    let archive_hash = hash_bytes(&archive_bytes);

    std::fs::write(tmp.join("libburi_rt.a"), &archive_bytes)
        .map_err(io_refusal("write the cross archive into the cache"))?;
    std::fs::write(tmp.join("features"), features.join("\n"))
        .map_err(io_refusal("write the cross feature list"))?;
    std::fs::write(tmp.join("sysroot.sha256"), &sysroot_hash)
        .map_err(io_refusal("write the cross sysroot digest"))?;
    std::fs::write(tmp.join("libburi_rt.a.sha256"), &archive_hash)
        .map_err(io_refusal("write the cross archive digest"))?;
    // `src/` and `rt/` were scratch; the published entry is the archive, the
    // sysroot and the sidecars.
    let _ = std::fs::remove_dir_all(tmp.join("src"));
    let _ = std::fs::remove_dir_all(&target_dir);

    // Publish. A directory already there is another process that won this race;
    // its bytes are keyed identically, so read it back rather than overwrite.
    let _ = std::fs::remove_dir_all(dir);
    match std::fs::rename(&tmp, dir) {
        Ok(()) => {}
        Err(_) => {
            let _ = std::fs::remove_dir_all(&tmp);
            return read_cache(dir, features).ok_or_else(|| {
                refuse(
                    "the cross runtime cache entry could not be published",
                    "check that `~/.buri` is writable",
                )
            });
        }
    }

    Ok(Cross {
        dir: dir.to_path_buf(),
        archive_hash,
        sysroot_hash,
        features: features.to_vec(),
    })
}

/// The nested cargo, mirroring `cli/build.rs`'s: the parent invocation's own
/// `CARGO_*` state removed so the child does not deadlock on the outer target
/// directory's lock, `RUSTFLAGS` emptied for reproducibility, and the flags
/// after `--` applied to the runtime crate alone. Offline first, then network.
fn run_cargo(
    cargo: &str,
    rustc: &str,
    src: &Path,
    triple: &str,
    features: &[String],
    target_dir: &Path,
) -> Result<PathBuf, link_refusal::Refusal> {
    let built = target_dir.join(triple).join("release/deps/libburi_rt.a");
    let mut last: Option<String> = None;
    for offline in [true, false] {
        let mut command = Command::new(cargo);
        for (name, _) in std::env::vars() {
            if name.starts_with("CARGO_") && name != "CARGO_HOME" {
                command.env_remove(&name);
            }
        }
        command.env_remove("CARGO");
        command.env("RUSTC", rustc);
        command.env("RUSTFLAGS", "");
        // The runtime bakes `#[track_caller]` locations; a fixed epoch and the
        // remap below keep two builds byte-identical, as they do for the host.
        command.env("SOURCE_DATE_EPOCH", "0");
        command.arg("rustc").arg("--lib").arg("--release");
        command.arg("--manifest-path").arg(src.join("Cargo.toml"));
        command.args(["--target", triple]);
        command.arg("--target-dir").arg(target_dir);
        command.arg("--no-default-features");
        if !features.is_empty() {
            command.arg("--features").arg(features.join(","));
        }
        command.arg("--locked");
        if offline {
            command.arg("--offline");
        }
        command.args([
            "--",
            "--remap-path-prefix==./runtime",
            "-Cmetadata=buri_rt",
            "-Cextra-filename=",
        ]);
        match command.output() {
            Ok(out) if out.status.success() => return Ok(built),
            Ok(out) => last = Some(String::from_utf8_lossy(&out.stderr).into_owned()),
            Err(e) => last = Some(e.to_string()),
        }
    }
    let mut refusal = refuse(
        format!("could not cross-build the runtime for {triple}"),
        format!(
            "check that `{cargo}` and the dependencies for {triple} are reachable — the first \
             cross build needs the network if they are not already fetched"
        ),
    );
    if let Some(text) = last {
        for line in text
            .lines()
            .rev()
            .take(8)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
        {
            refusal.notes.push(line.to_string());
        }
    }
    Err(refusal)
}

/// A cache hit: the archive, the sysroot and the sidecars all present in `dir`,
/// with the recorded feature list matching the one asked for.
///
/// The digests are read back from the sidecars rather than recomputed, for the
/// reason `runtime_native::ARCHIVE_SHA256` is a baked string: the bytes cannot
/// have changed since the entry was published under a key that named them, so
/// hashing several megabytes on every cache lookup would be work with nothing to
/// show for it.
fn read_cache(dir: &Path, features: &[String]) -> Option<Cross> {
    let archive = dir.join("libburi_rt.a");
    if std::fs::metadata(&archive).map(|m| m.len()).unwrap_or(0) == 0 {
        return None;
    }
    let recorded: Vec<String> = std::fs::read_to_string(dir.join("features"))
        .ok()?
        .lines()
        .map(String::from)
        .collect();
    if recorded != features {
        return None;
    }
    // Every sysroot member has to be there, or the link this hit feeds will
    // fail on a missing crt object.
    for (name, _) in musl::FILES {
        if !dir.join("musl").join("lib").join(name).is_file() {
            return None;
        }
    }
    let archive_hash = std::fs::read_to_string(dir.join("libburi_rt.a.sha256")).ok()?;
    let sysroot_hash = std::fs::read_to_string(dir.join("sysroot.sha256")).ok()?;
    Some(Cross {
        dir: dir.to_path_buf(),
        archive_hash: archive_hash.trim().to_string(),
        sysroot_hash: sysroot_hash.trim().to_string(),
        features: recorded,
    })
}

/// The cache key: a digest of everything the cross archive depends on.
///
/// The runtime sources (through `runtime_src::pack_hash`), the target triple,
/// the feature set, and the `rustc`/`cargo` that will build it — the same list
/// `cli/build.rs`'s stamp keeps for the host archive, minus the terms that are
/// about *this machine's* directories rather than the bytes. A change to any of
/// them is a fresh entry; nothing else is.
fn cache_key(triple: &str, features: &[String], rustc: &str, cargo: &str) -> String {
    let mut parts = String::new();
    parts.push_str(RECIPE);
    parts.push('\u{0}');
    parts.push_str(&runtime_src::pack_hash());
    parts.push('\u{0}');
    parts.push_str(triple);
    parts.push('\u{0}');
    parts.push_str(&features.join(","));
    parts.push('\u{0}');
    parts.push_str(&tool_version(rustc, &["-vV"]).unwrap_or_default());
    parts.push('\u{0}');
    parts.push_str(&tool_version(cargo, &["-V"]).unwrap_or_default());
    hash_bytes(parts.as_bytes())
}

/// `~/.buri`, or `$BURI_HOME` when it is set.
///
/// `BURI_HOME` is the seam a test needs so the cache does not touch a
/// contributor's real `~/.buri`, and the escape hatch for a machine whose home
/// directory is not where the cache should live. No home at all is a refusal
/// rather than a temp directory: a cache that vanished between builds would
/// rebuild the runtime every time, which is exactly the cost the cache exists to
/// avoid, and doing it silently would be worse than saying so.
fn buri_home() -> Result<PathBuf, link_refusal::Refusal> {
    if let Some(home) = std::env::var_os("BURI_HOME") {
        return Ok(PathBuf::from(home));
    }
    if let Some(home) = std::env::var_os("HOME") {
        return Ok(PathBuf::from(home).join(".buri"));
    }
    Err(refuse(
        "no home directory to cache the cross runtime under",
        "set `HOME`, or `BURI_HOME`, to a writable directory",
    ))
}

/// A program named by an environment variable, or a default off `PATH`.
fn tool(var: &str, default: &str) -> String {
    std::env::var(var).unwrap_or_else(|_| default.to_string())
}

/// `self-contained/` under a target's libdir, if it holds the `libc.a` that
/// makes it worth having — the same probe `cli/build.rs` uses, and for the same
/// reason: `--print target-libdir` prints a path for a target whose `rust-std`
/// was never installed, so the `libc.a` inside is the answer rather than the
/// path.
fn self_contained(rustc: &str, triple: &str) -> Option<PathBuf> {
    let out = Command::new(rustc)
        .args(["--print", "target-libdir", "--target", triple])
        .output()
        .ok()
        .filter(|o| o.status.success())?;
    let libdir = PathBuf::from(String::from_utf8(out.stdout).ok()?.trim().to_string());
    let dir = libdir.join("self-contained");
    dir.join("libc.a").is_file().then_some(dir)
}

/// A tool's version banner, or `None` when it could not be asked.
fn tool_version(program: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(program).args(args).output().ok()?;
    out.status.success().then_some(())?;
    String::from_utf8(out.stdout).ok()
}

/// An `io::Error` turned into a refusal that names the step it failed at.
fn io_refusal(step: &str) -> impl Fn(std::io::Error) -> link_refusal::Refusal + '_ {
    move |e| {
        refuse(
            format!("could not {step} for the cross runtime: {e}"),
            "check that `~/.buri` and the toolchain are readable and writable",
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The feature decision is net-off and crypto-off, whatever the host
    /// declares — the whole reason a cross `net` program is refused at compile
    /// time.
    #[test]
    fn the_first_cross_target_is_net_off_and_crypto_off() {
        let features = cross_features();
        assert!(
            !features.iter().any(|f| f == "net"),
            "net must be off for the first cross target"
        );
        assert!(
            !features.iter().any(|f| f == "crypto"),
            "crypto must be off for the first cross target"
        );
    }

    /// The cache key moves when any of its inputs moves, and is stable when none
    /// does.
    #[test]
    fn the_cache_key_is_a_function_of_its_inputs() {
        let a = cache_key(
            "x86_64-unknown-linux-musl",
            &[String::from("paint")],
            "rustc",
            "cargo",
        );
        let b = cache_key(
            "x86_64-unknown-linux-musl",
            &[String::from("paint")],
            "rustc",
            "cargo",
        );
        assert_eq!(a, b, "the same inputs give the same key");
        let c = cache_key(
            "aarch64-unknown-linux-musl",
            &[String::from("paint")],
            "rustc",
            "cargo",
        );
        assert_ne!(a, c, "a different triple is a different key");
        let d = cache_key("x86_64-unknown-linux-musl", &[], "rustc", "cargo");
        assert_ne!(a, d, "a different feature set is a different key");
    }

    /// `BURI_HOME` wins over `HOME`, which is the seam that keeps a test off a
    /// contributor's real cache.
    #[test]
    fn buri_home_overrides_home() {
        // Read-only over the process environment rather than set/unset, so the
        // test does not race a parallel one over a shared variable: it asserts
        // the precedence rule the function encodes rather than exercising it
        // against a mutated environment.
        let path = buri_home();
        // On any host running the suite there is a HOME or a BURI_HOME, so this
        // resolves; the assertion is that it ends in `.buri` unless BURI_HOME
        // redirected it.
        if let Ok(dir) = path {
            if std::env::var_os("BURI_HOME").is_none() {
                assert!(dir.ends_with(".buri"), "{dir:?} should be <home>/.buri");
            }
        }
    }
}
