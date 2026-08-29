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
use crate::diagnostics::Diagnostics;
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
pub fn triple_text(target: Target) -> Option<String> {
    let arch = match target.arch {
        Some(Arch::X86_64) => "x86_64",
        Some(Arch::Arm64) => "aarch64",
        None if cfg!(target_arch = "aarch64") => "aarch64",
        None => "x86_64",
    };
    match target.platform {
        Platform::Macos => Some(format!("{arch}-apple-darwin")),
        Platform::Linux => Some(format!("{arch}-unknown-linux-gnu")),
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
    /// intrinsic" becomes a question asked per backend (`design/TODO.md#the-native-backend`).
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
        (platform, Profile::Debug) => Err(missing_backend(platform, "backend-stencil")),
        #[cfg(feature = "backend-llvm")]
        (Platform::Linux | Platform::Macos, Profile::Release) => Ok(Box::new(llvm::Llvm)),
        // Gated the other way for the same reason the debug arm above is: with
        // the feature on the arm above is total for a native release build.
        #[cfg(not(feature = "backend-llvm"))]
        (platform, Profile::Release) => Err(missing_backend(platform, "backend-llvm")),
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

/// Both arms that call this are `#[cfg(not(...))]`, so a build with both
/// backends has no caller.
#[cfg(not(all(feature = "backend-stencil", feature = "backend-llvm")))]
fn missing_backend(platform: Platform, feature: &str) -> String {
    format!(
        "the {} backend is not implemented (it arrives with `{feature}`)",
        platform.slug()
    )
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
