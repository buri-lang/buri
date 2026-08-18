//! The interface between the middle end and a backend.
//!
//! One directory per backend, and this file holds only what they have in
//! common: the [`Backend`] and [`Linker`] traits, the [`Emitted`] unit they
//! trade in, and [`Profile`], which is a statement about programs rather than
//! about any one target.
//!
//! ```text
//! backend/
//!   mod.rs        this file
//!   js/           always compiled in
//!   cranelift/    behind `backend-cranelift`, on by default   (wave 2a)
//!   llvm/         behind `backend-llvm`, off by default       (wave 2b)
//! ```
//!
//! Design: `design/native/ARCHITECTURE.md` §3.

pub mod js;

/// The native runtime archive both native backends link against, built by
/// `cli/build.rs`. Its ABI contract is `cli/runtime/lib.rs`'s module comment.
pub mod runtime_native;

#[cfg(feature = "backend-cranelift")]
pub mod cranelift;

#[cfg(feature = "backend-llvm")]
pub mod llvm;

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

pub struct LinkOptions<'a> {
    pub profile: Profile,
    pub target: Target,
    pub unit_prefix: &'a str,
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
    /// `js`, `cranelift`, `llvm`. Enters every cache key this backend
    /// produces.
    fn name(&self) -> &'static str;

    /// The identity of everything outside the program that the bytes depend
    /// on: the LLVM version, the Cranelift version, the runtime's own hash.
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
}

/// Combining units into the final artifact.
///
/// Separate from [`Backend`] because the two vary independently: `cranelift`
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
/// (Linux | Macos,  Debug)    -> cranelift
/// (Linux | Macos,  Release)  -> llvm
/// ```
///
/// The second row arrives with wave 2a and is gated on the feature that
/// carries it, so a toolchain built `--no-default-features` still answers the
/// diagnostic rather than failing to compile. The third arrives with 2b.
///
/// A toolchain built without `backend-llvm` refuses a native release build with
/// a diagnostic naming the feature rather than silently falling back to
/// Cranelift: `--release` producing different code depending on how the
/// compiler was installed is the same class of bug as an unpinned toolchain.
pub fn select(target: Target, profile: Profile) -> Result<Box<dyn Backend>, String> {
    match (target.platform, profile) {
        (Platform::Js, _) => Ok(Box::new(js::Js)),
        #[cfg(feature = "backend-cranelift")]
        (Platform::Linux | Platform::Macos, Profile::Debug) => {
            Ok(Box::new(cranelift::Cranelift))
        }
        // Gated the other way, not left as a fallback: with the feature on the
        // arm above is total for a native debug build, and an arm nothing can
        // reach is a warning rather than a safety net.
        #[cfg(not(feature = "backend-cranelift"))]
        (platform, Profile::Debug) => Err(missing_backend(platform, "backend-cranelift")),
        (platform, Profile::Release) => Err(missing_backend(platform, "backend-llvm")),
    }
}

fn missing_backend(platform: Platform, feature: &str) -> String {
    format!(
        "the {} backend is not implemented (it arrives with `{feature}`)",
        platform.slug()
    )
}
