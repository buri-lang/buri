//! The interface between the middle end and a backend.
//!
//! One directory per backend, and this file holds only what they have in
//! common: the [`Backend`] and [`Linker`] traits, the [`Emitted`] unit they
//! trade in, and [`Profile`], which is a statement about programs rather than
//! about any one target.
//!
//! ```text
//! backend/
//!   mod.rs             this file
//!   intrinsic_keys.rs  how an intrinsic key is classified, shared
//!   runtime_native.rs  the `libburi_rt.a` archive, and the symbol rule
//!   runtime_table.rs   which keys have a `buri_rt_*` symbol, and its shape
//!   js/                always compiled in
//!   llvm/              behind `backend-llvm`, off by default
//!   stencil/           behind `backend-stencil`, on by default
//! ```
//!
//! [`select`] answers `stencil` for every native debug build it can, and
//! refuses by triple where the stencil backend has no library or no entry
//! point. There is no third native backend and no crate behind the default
//! toolchain: `design/native/CODEGEN-STENCIL.md` is the whole of it.
//!
//! Design: `design/native/ARCHITECTURE.md` §3.

pub mod js;

/// How an intrinsic key is classified, where every backend classifies it the
/// same way.
pub mod intrinsic_keys;

/// The one C signature by which something outside a Buri artifact enters Buri
/// code, and the two runtime entries a stencil door takes its stack from.
///
/// Ungated for [`intrinsic_keys`]'s reason: both native backends read it, and
/// a table two backends share cannot live inside either of them.
pub mod carrier;

/// The native runtime archive both native backends link against, built by
/// `cli/build.rs`. Its ABI contract is `cli/runtime/lib.rs`'s module comment.
pub mod runtime_native;

/// Which `buri_rt_*` entry a key names, and what shape the call has.
#[cfg(feature = "backend-stencil")]
pub mod runtime_table;

#[cfg(feature = "backend-llvm")]
pub mod llvm;

#[cfg(feature = "backend-stencil")]
pub mod stencil;

use crate::build::buildfile::{Arch, Platform};
use crate::build::cache::ActionKey;
use crate::compiler::middle::monomorphize::Program;
use crate::compiler::semantics::types::Tables;
use crate::diagnostics::{Diagnostic, Diagnostics, Span};
use std::path::Path;

/// Which build this run is for.
///
/// This was three independent `bool`s — `pretty`, `debug_names` and
/// `defensive_aborts` — all set from one `!release` at the single construction
/// site: eight combinations representable, two ever produced. One axis, and the
/// knobs are derived from it.
///
/// It lives here rather than in the JavaScript backend because
/// [`Profile::defensive_aborts`] is a statement about programs and not about
/// JavaScript, and because a native backend needs the same two-valued answer.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Profile {
    Debug,
    Release,
}

impl Profile {
    /// Debug builds stay readable: the names are what make a stack trace
    /// useful, and `--release` is where size matters.
    pub fn pretty(self) -> bool {
        self == Profile::Debug
    }

    /// Whether a match keeps a test on its last arm, and an abort behind it,
    /// even though `exhaustiveness.rs` has already proved one of the arms runs.
    ///
    /// On in debug, off in release. It is the backend's own belt to the
    /// checker's braces, and `release_and_debug_agree` is what says the two
    /// still compute the same answers.
    pub fn defensive_aborts(self) -> bool {
        self == Profile::Debug
    }

    pub fn name(self) -> &'static str {
        match self {
            Profile::Debug => "debug",
            Profile::Release => "release",
        }
    }
}

/// What a backend is emitting for.
///
/// Platform and architecture together, because a backend needs both and the
/// build system already carries them as a pair on every `Output`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Target {
    pub platform: Platform,
    pub arch: Option<Arch>,
}

/// What an emission or a link is for.
pub struct Options<'a> {
    pub profile: Profile,
    pub target: Target,
    /// Repository-relative, for the paths a debug section records.
    ///
    /// Relative rather than absolute for the same reason `action_key` hashes
    /// relative paths: two checkouts in different directories must produce
    /// identical bytes, and an absolute `DW_AT_comp_dir` is precisely the
    /// failure `--check-reproducible`'s two-directory design exists to catch.
    pub unit_prefix: &'a str,
}

/// A link wants the same three answers an emission does, under the name the
/// [`Linker`] trait reads better with.
pub type LinkOptions<'a> = Options<'a>;

/// The target triple a [`Target`] names, as text, or `None` for a platform no
/// native backend emits for.
///
/// `arch: None` means the host's architecture — the same rule `cli/build.rs`
/// uses to build the runtime archive the objects are linked against. One rule
/// here rather than one per backend, so that an unqualified `--output=linux`
/// cannot mean two things; the refusal is each backend's own sentence, which
/// is why this answers `None` rather than an error.
///
/// No macOS deployment version: the object the linker is handed carries what
/// `cc` puts on the command line, and pinning one here would make a toolchain
/// refuse to link on a newer SDK.
///
/// **Linux is `-musl`, not `-gnu`.** A Linux executable this toolchain
/// produces is a static PIE linked against the musl libc it ships, so it runs
/// on any Linux of that architecture with no loader and no `libc.so` to find;
/// naming glibc in the triple while linking musl would be a claim the
/// toolchain has stopped being able to make. It costs nothing in bytes: on
/// both architectures emitted for, the two spellings share a psABI, a data
/// layout and a relocation vocabulary, so what an object holds is unchanged
/// and only the name on it moves.
pub fn triple_text(target: Target) -> Option<String> {
    let arch = match target.arch {
        Some(Arch::X86_64) => "x86_64",
        Some(Arch::Arm64) => "aarch64",
        None if cfg!(target_arch = "aarch64") => "aarch64",
        None => "x86_64",
    };
    match target.platform {
        Platform::Macos => Some(format!("{arch}-apple-darwin")),
        Platform::Linux => Some(format!("{arch}-unknown-linux-musl")),
        Platform::Js | Platform::Web => None,
    }
}

/// Which codegen units an emission is for.
///
/// The build system keys one cache entry per unit (ARCHITECTURE.md §6.2) and
/// serves every hit from the cache, so the units it still needs after a
/// one-line edit are usually one of several hundred. Without this the whole
/// program is re-emitted whenever any single key misses, which is the same
/// work `--force` does: at 118k lines that was 1680 ms of a 2622 ms rebuild,
/// spent producing objects that were then thrown away.
///
/// A backend may emit *more* than it was asked for — the caller selects the
/// objects it wanted by name — but never fewer, which is what makes
/// [`Backend::emit_units`]'s default implementation correct for a backend that
/// has no per-unit path.
#[derive(Clone, Copy)]
pub enum Units<'a> {
    All,
    /// Unit indices into `ir::Program::units`, which is the order
    /// `Backend::emit` returns its objects in.
    Only(&'a [u32]),
}

impl Units<'_> {
    pub fn wants(self, unit: u32) -> bool {
        match self {
            Units::All => true,
            Units::Only(only) => only.contains(&unit),
        }
    }
}

/// One codegen unit's output.
///
/// `key` is the cache key the unit was produced under, and it is computed by
/// the backend rather than by the build system: only the backend knows which of
/// its own inputs — target triple, LLVM version, pass pipeline — the bytes
/// depend on.
pub struct Emitted {
    /// Stable, deterministic, and a filename: `lib_money.o`, `main.mjs`.
    pub name: String,
    pub key: ActionKey,
    pub bytes: Vec<u8>,
}

/// What every backend can be asked.
///
/// The signature `design/native/ARCHITECTURE.md` §3 proposed was
/// `emit(&Program, &Tables, &Options) -> Result<Vec<u8>, Diagnostics>`, and it
/// is amended in one place: `Vec<u8>` is one artifact, and the whole of the
/// incremental-link plan is that a build emits *many* object files and relinks
/// only the ones that moved. A trait that can only return one blob makes the
/// feature unrepresentable, and the shape of it would have to be smuggled
/// through `Options` or through the filesystem.
pub trait Backend {
    /// `js`, `stencil`, `llvm`. Enters every cache key this backend
    /// produces.
    fn name(&self) -> &'static str;

    /// The identity of everything outside the program that the bytes depend
    /// on: the LLVM version, the stencil libraries' hash, the runtime's own hash.
    /// Enters every cache key.
    ///
    /// A backend that returns a constant here is claiming its output cannot
    /// change without the toolchain hash changing, which is true of `js` and of
    /// nothing else: `llvm-sys` links against whatever `llvm-config` found at
    /// build time, so two `buri` binaries with identical Rust source can have
    /// different LLVM underneath. The build system has no way to ask, so the
    /// backend answers.
    fn identity(&self) -> String;

    /// Intrinsic keys this backend has no implementation of, so "missing
    /// intrinsic" becomes a question asked per backend .
    ///
    /// Taking the program rather than a list of strings is the point: the list
    /// was accumulated as a side effect of emission, so a program could only be
    /// told what it was missing *after* a failed one. Asking up front means
    /// `buri build --output=linux/arm64` on a program using an unimplemented
    /// intrinsic reports it before spending a second in LLVM.
    fn missing_intrinsics(&self, program: &Program, tables: &Tables) -> Vec<String>;

    /// `&mut self` because an LLVM `Context` is not `Sync` and owns everything
    /// built inside it; a `&self` signature would force interior mutability on
    /// the one backend that most wants a plain owned object.
    fn emit(
        &mut self,
        program: &Program,
        tables: &Tables,
        opts: &Options<'_>,
    ) -> Result<Vec<Emitted>, Diagnostics>;

    /// [`Backend::emit`], restricted to the units the caller still needs.
    ///
    /// This is the parameter the incremental-link plan was missing: `Func::unit`
    /// already partitions the program (ARCHITECTURE.md §5.1) and the build
    /// system already knows which units' keys missed, so invalidating one unit
    /// should cost one unit's codegen rather than the whole program's.
    ///
    /// The default emits everything and is correct rather than fast, because a
    /// superset satisfies every caller: `build::actions::codegen_units` takes
    /// the objects it asked for by name and serves the rest from the cache. A
    /// backend with one unit — JavaScript, whose artifact is one file its
    /// `Concatenate` linker takes element zero of — wants exactly that.
    fn emit_units(
        &mut self,
        program: &Program,
        tables: &Tables,
        opts: &Options<'_>,
        units: Units<'_>,
    ) -> Result<Vec<Emitted>, Diagnostics> {
        let _ = units;
        self.emit(program, tables, opts)
    }
}

/// The intrinsic keys this toolchain cannot answer because its runtime archive
/// was built without networking.
///
/// A second source of "missing intrinsic", beside the one every backend already
/// answers from its own surface, and a different sentence: a key the backend has
/// no body for is a toolchain bug to report, and a key the *archive* has no
/// symbol for is a toolchain built without a capability. Both native backends
/// fold this into [`Backend::missing_intrinsics`], so a program reaching
/// networking on a runtime that has none is refused before codegen rather than
/// at `cc` time with a symbol nobody outside this repository can read.
///
/// It answers nothing on an ordinary toolchain: `net` is the runtime's default
/// feature.
pub fn networking_gap(program: &Program) -> Vec<String> {
    networking_gap_when(program, runtime_native::net())
}

/// [`networking_gap`], with the toolchain's answer as a parameter.
///
/// [`runtime_native::net()`] is read from a file baked into this binary, so a
/// test on a toolchain that *has* networking has no way to ask what one without
/// it would say. This is that seam, and `networking_gap` is the one line that
/// binds it to the constant.
///
/// **`pub` because the seam is reached from outside this crate too.** The unit
/// rows below drive it over hand-built [`Program`]s;
/// `cli/tests/native/e2e.rs`'s `the_refusal_a_toolchain_without_networking_names`
/// drives it over a `Program` the real front end built out of a real
/// `server.serve` source, which is the closest a toolchain that *has*
/// networking can stand to the refusal a toolchain without it prints.
pub fn networking_gap_when(program: &Program, net: bool) -> Vec<String> {
    if net {
        return Vec::new();
    }
    let mut keys: Vec<String> = program
        .funcs
        .iter()
        .filter_map(|f| f.intrinsic_key())
        .filter(|key| runtime_native::net_intrinsic(key))
        .map(String::from)
        .collect();
    keys.sort();
    keys.dedup();
    keys
}

/// The intrinsic keys this toolchain cannot answer because its runtime archive
/// was built without cryptography.
///
/// [`networking_gap`]'s twin, and a separate function rather than a parameter
/// because the two say different sentences and name different features. It
/// answers nothing on an ordinary toolchain: `crypto` is one of the runtime's
/// default features.
pub fn cryptography_gap(program: &Program) -> Vec<String> {
    cryptography_gap_when(program, runtime_native::crypto())
}

/// [`cryptography_gap`], with the toolchain's answer as a parameter, for the
/// reason [`networking_gap_when`] takes one: `runtime_native::crypto()` is read
/// from a file baked into this binary, so a toolchain that *has* cryptography
/// has no other way to ask what one without it would say.
pub fn cryptography_gap_when(program: &Program, crypto: bool) -> Vec<String> {
    if crypto {
        return Vec::new();
    }
    let mut keys: Vec<String> = program
        .funcs
        .iter()
        .filter_map(|f| f.intrinsic_key())
        .filter(|key| runtime_native::crypto_intrinsic(key))
        .map(String::from)
        .collect();
    keys.sort();
    keys.dedup();
    keys
}

/// What [`Backend::missing_intrinsics`] answered, split by what to say about it.
///
/// Two causes, and they ask the reader for different things. The first list is
/// the operations *no* backend can answer because this toolchain's runtime
/// archive was built without networking — [`no_networking`] is their sentence,
/// and the way out of it is a different toolchain. The second is everything
/// else: a key the backend has no body for, which is a toolchain bug, and which
/// each emission site already has its own sentence for.
///
/// Splitting here rather than at each site is what keeps the two sites from
/// disagreeing about which half a key is in.
pub fn split_networking(missing: &[String]) -> (Vec<String>, Vec<String>) {
    split_networking_when(missing, runtime_native::net())
}

/// [`split_networking`], with the toolchain's answer as a parameter, for the
/// reason [`networking_gap_when`] takes one.
fn split_networking_when(missing: &[String], net: bool) -> (Vec<String>, Vec<String>) {
    missing.iter().cloned().partition(|key| !net && runtime_native::net_intrinsic(key))
}

/// The refusal for operations this toolchain's runtime has no networking for.
///
/// Templated, because the wording is the page's: a reader who meets this has a
/// toolchain to replace rather than a program to fix, and "report it" — the
/// fix every other missing intrinsic carries — would be the wrong instruction.
pub fn no_networking(operations: &[String], span: Span) -> Diagnostic {
    Diagnostic::templated("networking-not-available", span)
        .with_bind("operations", crate::diagnostics::names(operations))
}

/// [`split_networking`] for the cryptography half.
///
/// Applied to what the networking split left rather than replacing it, so a
/// third cause is one more line at each site and not a wider tuple everywhere.
/// The two families are disjoint — `net_intrinsic` and `crypto_intrinsic` match
/// different effects — so the order the two splits run in cannot matter.
pub fn split_cryptography(missing: &[String]) -> (Vec<String>, Vec<String>) {
    split_cryptography_when(missing, runtime_native::crypto())
}

/// [`split_cryptography`], with the toolchain's answer as a parameter.
fn split_cryptography_when(missing: &[String], crypto: bool) -> (Vec<String>, Vec<String>) {
    missing.iter().cloned().partition(|key| !crypto && runtime_native::crypto_intrinsic(key))
}

/// The refusal for operations this toolchain's runtime has no cryptography for.
///
/// [`no_networking`]'s twin, and templated for its reason: a reader who meets
/// this has a toolchain to replace rather than a program to fix.
pub fn no_cryptography(operations: &[String], span: Span) -> Diagnostic {
    Diagnostic::templated("cryptography-not-available", span)
        .with_bind("operations", crate::diagnostics::names(operations))
}

/// Combining units into the final artifact.
///
/// Separate from [`Backend`] because the two vary independently: `stencil`
/// and `llvm` both hand their objects to the platform linker, and the same
/// backend links differently on macOS and on Linux.
pub trait Linker {
    /// Enters the `link` key, with [`Linker::version`].
    fn name(&self) -> &'static str;

    /// The linker's own identity, for the same reason [`Backend::identity`]
    /// exists: `ld64` and `mold` do not produce the same bytes.
    fn version(&self) -> String;

    /// Everything *else* about the link that decides the bytes: the flags, and
    /// the libc they name.
    ///
    /// [`Linker::version`] is the linker's `--version` banner and nothing more,
    /// so for years the `link` key held *who* linked and not *how*. That was
    /// survivable while the command line was a function of the platform alone;
    /// it stopped being survivable when the Linux link gained a libc question
    /// with three answers. A toolchain rebuilt with a musl `rust-std` installed
    /// links `-static-pie` against a baked sysroot where the one before it
    /// linked against the host glibc — same `cc`, same `mold`, same objects,
    /// same banner, and an artifact that is a different file. Without this term
    /// the second build is served the first one's executable.
    ///
    /// Empty by default, and that is the honest answer for a linker whose
    /// command line has no variation in it: `js::Concatenate` writes bytes it
    /// computed itself, and a term that is always the same string is a term
    /// that says nothing.
    fn link_identity(&self) -> String {
        String::new()
    }

    /// Combines units into the final artifact at `out`. `unchanged` names the
    /// units whose bytes are byte-identical to the previous link, which a
    /// linker may use and may ignore.
    fn link(
        &self,
        units: &[Emitted],
        unchanged: &[usize],
        out: &Path,
        opts: &LinkOptions<'_>,
    ) -> Result<(), Diagnostics>;
}

/// The backend for one target and one profile.
///
/// ```text
/// (Js,             _)        -> js
/// (Linux | Macos,  Debug)    -> stencil, where stencil has that target
/// (Linux | Macos,  Release)  -> llvm
/// ```
///
/// The second and third rows are each gated on the feature that carries them,
/// so a toolchain built `--no-default-features` still answers the diagnostic
/// rather than failing to compile.
///
/// The debug row is the only one that asks about the *target* as well as the
/// platform, and it does not ask it here: [`stencil::supported`] is the one
/// place a target is matched to a stencil library. `linux-x86_64` became a
/// supported target with no edit in this file, and a fourth would arrive the
/// same way. Asking it at selection rather than inside `emit` is what makes
/// `build::actions::native_ready` honest — otherwise a host reports a backend
/// it has and then refuses every program deep inside an emission.
///
/// A toolchain built without `backend-llvm` refuses a native release build with
/// a diagnostic naming the feature rather than silently falling back to the
/// development backend: `--release` producing different code depending on how
/// the compiler was installed is the same class of bug as an unpinned
/// toolchain.
pub fn select(target: Target, profile: Profile) -> Result<Box<dyn Backend>, String> {
    match (target.platform, profile) {
        // `Web` joins `Js` here because the question this match asks is
        // "which backend emits this artifact", and a page is JavaScript.
        (Platform::Js | Platform::Web, _) => Ok(Box::new(js::Js)),
        #[cfg(feature = "backend-stencil")]
        (Platform::Linux | Platform::Macos, Profile::Debug) => {
            match stencil::supported(target) {
                Ok(_) => Ok(Box::new(stencil::Stencil)),
                Err(why) => Err(no_development_backend(target, &why)),
            }
        }
        // Gated the other way, not left as a fallback: with the feature on the
        // arm above is total for a native debug build, and an arm nothing can
        // reach is a warning rather than a safety net.
        #[cfg(not(feature = "backend-stencil"))]
        (_, Profile::Debug) => Err(no_development_code_generator()),
        #[cfg(feature = "backend-llvm")]
        (Platform::Linux | Platform::Macos, Profile::Release) => Ok(Box::new(llvm::Llvm)),
        // Gated the other way for the same reason the debug arm above is: with
        // the feature on the arm above is total for a native release build.
        #[cfg(not(feature = "backend-llvm"))]
        (_, Profile::Release) => Err(no_optimizing_backend()),
    }
}

/// A native debug build for a triple the development backend has not finished.
///
/// The triple leads because it is what the user chose and what would have to
/// change; the backend's own sentence follows, because it is the one that says
/// which of the three missing things is missing.
#[cfg(feature = "backend-stencil")]
fn no_development_backend(target: Target, why: &str) -> String {
    let triple = triple_text(target).unwrap_or_else(|| target.platform.slug().to_string());
    format!("no development backend for {triple}: {why}")
}

/// What `--release` is refused with on a toolchain built without the
/// optimizing backend.
///
/// It does **not** say "the macos backend is not implemented", which is what it
/// said for as long as the two sentences were one function, and which was false
/// twice over on every host that has a development backend: the platform is
/// implemented, the *profile* is not, and the debug build of the very same
/// output succeeds (buri-lang/buri#26). What is missing is a cargo feature of
/// this binary, so that is what the sentence names, and `build::actions`'s
/// `native_gap` is what pairs it with the fix — build without `--release`,
/// where the development backend has the target.
#[cfg(not(feature = "backend-llvm"))]
fn no_optimizing_backend() -> String {
    "`--release` needs the optimizing native backend, and this toolchain was \
     built without `backend-llvm`"
        .to_string()
}

/// The same sentence for the other half: a toolchain built
/// `--no-default-features` has no code generator for a native artifact at all.
#[cfg(not(feature = "backend-stencil"))]
fn no_development_code_generator() -> String {
    "this toolchain was built without a native code generator (`backend-stencil`)".to_string()
}

/// [`select`] over platform × profile × per-target availability.
///
/// The availability axis is real on every host: `macos-x86_64` is refused by a
/// constant this file does not own — no stencil library is built for it — and
/// the other three rows are checked against `stencil::supported` rather than
/// against a second list, which is the property that let `linux-x86_64` light
/// up here with no edit to this file at all.
#[cfg(test)]
mod tests {
    use super::*;

    fn at(platform: Platform, arch: Option<Arch>) -> Target {
        Target { platform, arch }
    }

    const NATIVE: [(Platform, Arch); 4] = [
        (Platform::Macos, Arch::Arm64),
        (Platform::Macos, Arch::X86_64),
        (Platform::Linux, Arch::Arm64),
        (Platform::Linux, Arch::X86_64),
    ];

    #[test]
    fn a_page_is_javascript_in_both_profiles() {
        for platform in [Platform::Js, Platform::Web] {
            for profile in [Profile::Debug, Profile::Release] {
                let backend = select(at(platform, None), profile)
                    .unwrap_or_else(|e| panic!("{platform:?}/{profile:?} refused: {e}"));
                assert_eq!(backend.name(), "js", "{platform:?}/{profile:?}");
            }
        }
    }

    #[cfg(feature = "backend-stencil")]
    #[test]
    fn a_native_debug_build_is_stencil_exactly_where_stencil_has_the_target() {
        for (platform, arch) in NATIVE {
            let target = at(platform, Some(arch));
            let selected = select(target, Profile::Debug);
            match stencil::supported(target) {
                Ok(_) => assert_eq!(
                    selected.map(|b| b.name()).unwrap_or("<refused>"),
                    "stencil",
                    "{platform:?}/{arch:?}"
                ),
                Err(_) => assert!(selected.is_err(), "{platform:?}/{arch:?} was selected anyway"),
            }
        }
    }

    /// The one native target selection refuses, and the other x86-64 row,
    /// which it must not.
    ///
    /// Both halves are here on purpose: a test that only checked the refusal
    /// would still pass if `linux-x86_64` had quietly stopped being selected,
    /// and that row is the one the arm64 host this usually runs on cannot
    /// otherwise vouch for.
    #[cfg(feature = "backend-stencil")]
    #[test]
    fn macos_x86_64_is_refused_by_triple_and_linux_x86_64_is_not() {
        let refused = at(Platform::Macos, Some(Arch::X86_64));
        let why = select(refused, Profile::Debug)
            .err()
            .expect("macos/x86_64 debug was not refused");
        assert!(why.starts_with("no development backend for "), "{why}");
        let triple = triple_text(refused).expect("a native target has a triple");
        assert!(why.contains(&triple), "the refusal names no triple: {why}");

        let supported = at(Platform::Linux, Some(Arch::X86_64));
        assert_eq!(
            select(supported, Profile::Debug).map(|b| b.name()).unwrap_or("<refused>"),
            "stencil"
        );
    }

    /// A refusal is a sentence, not a panic: the whole point of asking the
    /// target question in `select` is that the failure arrives before an
    /// emission starts.
    #[cfg(feature = "backend-stencil")]
    #[test]
    fn an_unsupported_target_refuses_rather_than_answering_a_backend() {
        let target = at(Platform::Macos, Some(Arch::X86_64));
        assert!(select(target, Profile::Debug).is_err());
        assert!(
            !crate::build::actions::native_ready(target, Profile::Debug),
            "native_ready answered true for a target select refuses"
        );
    }

    /// Where the host's own row is available, its unqualified target has to
    /// reach the same backend the qualified one does.
    #[cfg(feature = "backend-stencil")]
    #[test]
    fn an_unqualified_native_target_follows_the_host_architecture() {
        let platform = if cfg!(target_os = "macos") { Platform::Macos } else { Platform::Linux };
        let arch = if cfg!(target_arch = "aarch64") { Arch::Arm64 } else { Arch::X86_64 };
        let unqualified = select(at(platform, None), Profile::Debug);
        let qualified = select(at(platform, Some(arch)), Profile::Debug);
        assert_eq!(unqualified.is_ok(), qualified.is_ok());
        assert_eq!(
            unqualified.map(|b| b.name()).unwrap_or("<refused>"),
            qualified.map(|b| b.name()).unwrap_or("<refused>")
        );
    }

    // -- the networking gap -------------------------------------------------
    //
    // Two of the three key families below are reachable from ordinary source
    // now: `Tasks` is granted on the three non-page platforms and answered by
    // `cli/runtime/rt.rs`, and `Listen` is granted on `LINUX` and `MACOS`, run
    // from `core/net/server` and answered by `cli/runtime/net.rs`. Only
    // `host.HostSockets.*` is still without a caller, because nothing performs
    // a WebSocket upgrade and so no program can name a socket to write to.
    //
    // A hand-built program is still the honest seam, and now for a reason that
    // has nothing to do with what is grantable. What is under test is the
    // *refusal* — what a toolchain whose archive carries no `net` says about a
    // program reaching these keys — and a fixture compiling a real server would
    // exercise that only on a toolchain built without networking, which is not
    // the one this suite runs on. Naming the keys directly checks the rule
    // wherever the suite runs, against the toolchain that is actually there.

    /// A program holding one intrinsic key, and nothing else.
    fn program_using(keys: &[&str]) -> Program {
        use crate::compiler::middle::monomorphize::{Func, FuncKind, ProgramRoots};
        use crate::compiler::semantics::types::Ty;
        use crate::diagnostics::Span;
        let funcs = keys
            .iter()
            .map(|key| Func {
                symbol: key.to_string(),
                debug_name: key.to_string(),
                params: Vec::new(),
                locals: Vec::new(),
                kind: FuncKind::Intrinsic(key.to_string()),
                ret: Ty::Unit,
                desc: None,
                span: Span::default(),
            })
            .collect();
        Program {
            funcs,
            roots: ProgramRoots::Main(crate::compiler::semantics::types::FuncIdx(0)),
            descriptors: Vec::new(),
            desc_modules: Vec::new(),
            desc_index: Default::default(),
            ctx_layouts: Default::default(),
            shapes: Default::default(),
            stylesheet: String::new(),
            inline_styles: false,
            themes: false,
        }
    }

    /// A toolchain whose runtime has no networking cannot answer the family,
    /// whatever else is in the program.
    #[test]
    fn a_runtime_without_networking_reports_the_family() {
        let program = program_using(&[
            "host.HostListen.listen",
            "host.HostSockets.socketSendText",
            "host.HostTasks.parallel",
            "host.HostFs.readFile",
            "list.map",
        ]);
        assert_eq!(
            networking_gap_when(&program, false),
            vec![
                "host.HostListen.listen".to_string(),
                "host.HostSockets.socketSendText".to_string(),
                "host.HostTasks.parallel".to_string(),
            ],
            "the gap claimed something outside the family, or missed part of it"
        );
    }

    /// And a toolchain whose runtime has it reports nothing at all, so the
    /// ordinary build pays a walk of the function list and no diagnostic.
    #[test]
    fn a_runtime_with_networking_reports_nothing() {
        let program = program_using(&["host.HostListen.listen", "host.HostTasks.parallel"]);
        assert!(networking_gap_when(&program, true).is_empty());
    }

    /// The wiring: what the backends call is the parameterised answer at this
    /// toolchain's own feature state, and not a second reading of it.
    #[test]
    fn the_gap_is_the_toolchain_s_answer() {
        let program = program_using(&["host.HostListen.listen", "host.HostFs.readFile"]);
        assert_eq!(
            networking_gap(&program),
            networking_gap_when(&program, runtime_native::net())
        );
        assert_eq!(
            networking_gap(&program).is_empty(),
            runtime_native::net(),
            "an ordinary toolchain has networking and reports no gap"
        );
    }

    /// The two causes are two sentences, and the networking one does not say
    /// "report it": nothing about the program is wrong.
    #[test]
    fn a_networking_gap_is_its_own_refusal() {
        let missing = vec![
            "host.HostListen.listen".to_string(),
            "json.encode".to_string(),
            "host.HostTasks.parallel".to_string(),
        ];
        let (networking, rest) = split_networking_when(&missing, false);
        assert_eq!(networking, vec!["host.HostListen.listen", "host.HostTasks.parallel"]);
        assert_eq!(rest, vec!["json.encode"]);

        let refusal = no_networking(&networking, Span::NONE);
        assert_eq!(refusal.code.as_deref(), Some("networking-not-available"));
        assert!(
            refusal.message.contains("`host.HostListen.listen`")
                && refusal.message.contains("`host.HostTasks.parallel`"),
            "the refusal names neither operation: {}",
            refusal.message
        );
        assert!(refusal.message.contains("without networking"), "{}", refusal.message);
        let fix = refusal.fix.clone().expect("the page carries a fix");
        assert!(fix.contains("net"), "the fix does not name the feature: {fix}");
        assert!(!fix.contains("report it"), "a missing capability is not a bug report: {fix}");
    }

    /// With networking present, every missing key is the backend's own gap —
    /// the split is not a reclassification of keys a toolchain can answer.
    #[test]
    fn networking_present_leaves_every_key_where_it_was() {
        let missing = vec!["host.HostListen.listen".to_string(), "json.encode".to_string()];
        let (networking, rest) = split_networking_when(&missing, true);
        assert!(networking.is_empty());
        assert_eq!(rest, missing);
        assert_eq!(
            split_networking(&missing),
            split_networking_when(&missing, runtime_native::net())
        );
    }

    // -- the cryptography gap -----------------------------------------------
    //
    // The same seam, one feature over. `host.HostEntropy.bytes` is reachable
    // from ordinary source on every platform — `core/crypto`'s `randomBytes`
    // and `token` are its only callers — but what is under test here is again
    // the *refusal*, which only a toolchain built without `crypto` produces.
    // So the key is named directly, for `networking_gap`'s reason.

    /// A `crypto`-less toolchain names the operation; an ordinary one is
    /// silent.
    #[test]
    fn a_toolchain_without_cryptography_names_the_operation() {
        let program = program_using(&["host.HostEntropy.bytes", "json.encode"]);
        assert_eq!(
            cryptography_gap_when(&program, false),
            vec!["host.HostEntropy.bytes".to_string()],
            "the gap is the entropy key and nothing else"
        );
        assert!(
            cryptography_gap_when(&program, true).is_empty(),
            "a toolchain with cryptography reports no gap"
        );
        assert_eq!(
            cryptography_gap(&program),
            cryptography_gap_when(&program, runtime_native::crypto())
        );
        assert_eq!(
            cryptography_gap(&program).is_empty(),
            runtime_native::crypto(),
            "an ordinary toolchain has cryptography and reports no gap"
        );
    }

    /// The refusal is its own sentence, names the feature, and does not say
    /// "report it": nothing about the program is wrong.
    #[test]
    fn a_cryptography_gap_is_its_own_refusal() {
        let missing =
            vec!["host.HostEntropy.bytes".to_string(), "json.encode".to_string()];
        let (cryptography, rest) = split_cryptography_when(&missing, false);
        assert_eq!(cryptography, vec!["host.HostEntropy.bytes"]);
        assert_eq!(rest, vec!["json.encode"]);

        let refusal = no_cryptography(&cryptography, Span::NONE);
        assert_eq!(refusal.code.as_deref(), Some("cryptography-not-available"));
        assert!(
            refusal.message.contains("`host.HostEntropy.bytes`"),
            "the refusal names no operation: {}",
            refusal.message
        );
        assert!(refusal.message.contains("without cryptography"), "{}", refusal.message);
        let fix = refusal.fix.clone().expect("the page carries a fix");
        assert!(fix.contains("crypto"), "the fix does not name the feature: {fix}");
        assert!(!fix.contains("report it"), "a missing capability is not a bug report: {fix}");
    }

    /// The two gaps are disjoint, and a toolchain missing both says both
    /// sentences rather than one of them twice.
    #[test]
    fn the_two_capability_gaps_do_not_overlap() {
        let missing = vec![
            "host.HostEntropy.bytes".to_string(),
            "host.HostListen.listen".to_string(),
            "json.encode".to_string(),
        ];
        let (networking, rest) = split_networking_when(&missing, false);
        let (cryptography, rest) = split_cryptography_when(&rest, false);
        assert_eq!(networking, vec!["host.HostListen.listen"]);
        assert_eq!(cryptography, vec!["host.HostEntropy.bytes"]);
        assert_eq!(rest, vec!["json.encode"]);
        // And the same three keys sorted into one pile by a toolchain that has
        // both capabilities.
        let (networking, rest) = split_networking_when(&missing, true);
        let (cryptography, rest) = split_cryptography_when(&rest, true);
        assert!(networking.is_empty() && cryptography.is_empty());
        assert_eq!(rest, missing);
    }

    /// Both native backends consult the gaps, rather than one of them.
    ///
    /// Asserted through `Backend::missing_intrinsics` — the trait method the
    /// build system and `buri test` actually ask — so a backend that stopped
    /// folding a gap in fails here rather than at somebody's link step.
    ///
    /// **Neither key is one a runtime answers**, and that is deliberate for
    /// both. `host.HostListen.listen` is not an operation `Listen` declares and
    /// `host.HostEntropy.words` is not one `Entropy` declares; a key that *is*
    /// implemented would be reported by neither backend on the toolchain this
    /// suite runs on, because both features are on and the surface covers it.
    /// What is under test is that a key in each family reaches the caller as a
    /// missing one at all.
    #[test]
    fn both_native_backends_report_a_capability_key() {
        for key in ["host.HostListen.listen", "host.HostEntropy.words"] {
            both_native_backends_report(key);
        }
    }

    /// One key, asked of whichever native backends this toolchain has.
    #[cfg(any(feature = "backend-stencil", feature = "backend-llvm"))]
    fn both_native_backends_report(key: &str) {
        let program = program_using(&[key]);
        let tables = Tables::default();
        let reports = |missing: Vec<String>| missing.iter().any(|k| k == key);
        #[cfg(feature = "backend-stencil")]
        assert!(
            reports(stencil::Stencil.missing_intrinsics(&program, &tables)),
            "the stencil backend claimed a key no runtime answers"
        );
        #[cfg(feature = "backend-llvm")]
        assert!(
            reports(llvm::Llvm.missing_intrinsics(&program, &tables)),
            "the llvm backend claimed a key no runtime answers"
        );
        // A toolchain with neither native backend has nothing to ask, and the
        // bindings above are then unused rather than wrong.
        let _ = (&program, &tables, reports);
    }

    /// The release path is unchanged by the development backend's arrival: a
    /// toolchain without `backend-llvm` names the feature rather than handing
    /// `--release` to whatever emits debug builds.
    #[test]
    fn a_native_release_build_answers_llvm_or_names_the_feature() {
        for (platform, arch) in NATIVE {
            let selected = select(at(platform, Some(arch)), Profile::Release);
            if cfg!(feature = "backend-llvm") {
                assert_eq!(
                    selected.map(|b| b.name()).unwrap_or("<refused>"),
                    "llvm",
                    "{platform:?}/{arch:?}"
                );
            } else {
                let why = selected
                    .err()
                    .unwrap_or_else(|| panic!("{platform:?}/{arch:?} release was not refused"));
                assert!(why.contains("backend-llvm"), "{why}");
            }
        }
    }

    /// **The refusal a `--release` build gets, in full** — the sentence, not
    /// only the fact that there is one (buri-lang/buri#26).
    ///
    /// The row above says `select` names the feature. This one asks
    /// `build::actions::native_gap`, which is what the three refusal sites
    /// actually print, and checks the two things the issue was about: that the
    /// reason blames the **profile** rather than the platform, and that the fix
    /// says the development backend has this very target — because it does, and
    /// the sentence this replaced told a reader whose debug build had just
    /// succeeded that the toolchain emits only JavaScript.
    ///
    /// It lives here rather than beside `native_gap` because it reads
    /// `cfg!(feature = "backend-llvm")`, and `cli/tests/README.md`'s
    /// verification bar confines that feature to the files this one is in —
    /// `corpus::the_llvm_feature_is_confined_to_the_files_the_bar_names` is
    /// what enforces it.
    #[test]
    fn a_release_refusal_names_the_profile_rather_than_the_platform() {
        use crate::build::actions::native_gap;
        let (Some(platform), arch) = (crate::build::link::host_platform(), crate::build::link::host_arch())
        else {
            return;
        };
        let target = Target { platform, arch };
        // A host with no development backend for its own target — an Intel mac
        // — has a different gap, and it is the one the debug row reports.
        if native_gap(target, Profile::Debug).is_some() {
            return;
        }
        let gap = native_gap(target, Profile::Release);
        if cfg!(feature = "backend-llvm") {
            assert!(
                gap.is_none(),
                "a toolchain with the optimizing backend refused its own `--release` build"
            );
            return;
        }
        let gap = gap.expect("`--release` was not refused on a toolchain without `backend-llvm`");
        assert!(gap.reason.contains("`--release`"), "{}", gap.reason);
        assert!(gap.reason.contains("backend-llvm"), "{}", gap.reason);
        assert!(
            gap.fix.contains("without `--release`") && gap.fix.contains(&gap.output),
            "the fix does not say that the development backend has this target: {}",
            gap.fix
        );
        assert!(
            !gap.reason.contains("emits JavaScript") && !gap.fix.contains("--output=js"),
            "a toolchain that had just built this target natively claimed to emit only \
             JavaScript: {} / {}",
            gap.reason,
            gap.fix
        );
    }

    /// A `--no-default-features` toolchain answers the diagnostic rather than
    /// failing to compile, and the diagnostic names the feature that carries
    /// the backend.
    #[cfg(not(feature = "backend-stencil"))]
    #[test]
    fn a_toolchain_with_no_development_backend_names_the_feature() {
        for (platform, arch) in NATIVE {
            let why = select(at(platform, Some(arch)), Profile::Debug)
                .err()
                .unwrap_or_else(|| panic!("{platform:?}/{arch:?} debug was not refused"));
            assert!(why.contains("backend-stencil"), "{why}");
        }
    }
}
