//! The interface between the middle end and a backend.
//!
//! What every backend has in common: the [`Backend`] trait, the [`Emitted`]
//! unit it trades in, and [`Profile`], which is a statement about programs
//! rather than about any one target. With them, what both native backends read
//! and neither owns.
//!
//! ```text
//! backend/
//!   mod.rs             this file
//!   runtime_native.rs  the runtime's symbol rule and ABI questions
//!   runtime_table.rs   which keys have a `buri_rt_*` symbol, and its shape
//!   task_thread.rs     the C signature a stencil door enters Buri through
//!   counts.rs          where the reference counts live inside a value
//!   linkage.rs         which functions another unit names
//! ```
//!
//! Each backend is its own crate: `buri-js`, `buri-stencil` and `buri-llvm`.
//! `buri`'s `compiler::backend::select` picks one.
//!
//! Design: `design/native/ARCHITECTURE.md` §3.

/// How an intrinsic key is classified, where every backend classifies it the
/// same way. `buri-middle`'s, because the middle end classifies keys too.
pub use buri_middle::compiler::backend::intrinsic_keys;

/// The one C signature by which something outside a Buri artifact enters Buri
/// code, and the two runtime entries a stencil door takes its stack from.
pub mod task_thread;

/// The native runtime's symbol rule and ABI questions. `buri` adds the archive
/// `cli/build.rs` builds. Its ABI contract is `cli/runtime/lib.rs`'s module
/// comment.
pub mod runtime_native;

/// Which `buri_rt_*` entry a key names, and what shape the call has.
pub mod runtime_table;

/// Where the reference counts live inside a value.
pub mod counts;

/// Which functions a unit other than their own names.
pub mod linkage;
use crate::build::buildfile::{Arch, Platform};
use crate::build::cache::ActionKey;
use crate::compiler::middle::ir;
use crate::compiler::middle::monomorphize::Program;
use crate::compiler::semantics::layered::{Dense, DenseId};
use crate::compiler::semantics::types::Tables;
use crate::diagnostics::{Diagnostic, Diagnostics, Span};

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

/// A link wants the same three answers an emission does, under the name a
/// link reads better with.
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
    match target.platform {
        Platform::Macos => Some(format!("{}-apple-darwin", arch_text(target.arch))),
        Platform::Linux => Some(linux_triple(target.arch)),
        Platform::Js | Platform::Web => None,
    }
}

/// [`triple_text`] of a Linux target on `arch`.
pub fn linux_triple(arch: Option<Arch>) -> String {
    format!("{}-unknown-linux-musl", arch_text(arch))
}

/// A triple's architecture: `arch`, or the host's when there is none.
fn arch_text(arch: Option<Arch>) -> &'static str {
    match arch {
        Some(Arch::X86_64) => "x86_64",
        Some(Arch::Arm64) => "aarch64",
        None if cfg!(target_arch = "aarch64") => "aarch64",
        None => "x86_64",
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
/// [`Native::emit_units`] answers exactly these units, one object each, in
/// unit order. So the caller pairs each unit it asked for with its object by
/// position, and never looks one up by name.
#[derive(Clone, Copy)]
pub enum Units<'a> {
    All,
    /// Unit indices into `ir::Program::units`, in ascending order.
    Only(&'a [usize]),
}

impl Units<'_> {
    pub fn wants(self, unit: usize) -> bool {
        match self {
            Units::All => true,
            Units::Only(only) => only.contains(&unit),
        }
    }
}

/// One codegen unit's output.
///
/// `key` is the cache key the unit was stored under. **It belongs to the
/// build system**: `build::actions::codegen_units_for` computes every unit's
/// key before it asks for any emission, and replaces whatever the backend put
/// here with it. A backend's statement about which of its own inputs the bytes
/// depend on — target triple, LLVM version — is `Backend::identity`, which is in
/// every key. So a backend may leave this `None`, and the stencil backend does:
/// rendering a unit's IR a second time to hash it was a fifth of its emission.
pub struct Emitted {
    /// Stable, deterministic, and a filename: `lib_money.o`, `main.mjs`.
    pub name: String,
    pub key: Option<ActionKey>,
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
    ///
    /// The objects' cache keys are the caller's, not this method's: see
    /// [`Emitted`].
    fn emit(
        &mut self,
        program: &Program,
        tables: &Tables,
        opts: &Options<'_>,
    ) -> Result<Vec<Emitted>, Diagnostics>;

    /// Whether every unit's object depends on
    /// [`runtime_table::shares_counts`]: this backend's reference-count fork
    /// reads `buri_rt_shared_mask` in a program that can fan out, in every
    /// unit, not only the one that fans out. The build folds the answer into
    /// every unit's cache key, so a library unit compiled for a program that
    /// couldn't fan out is never linked into one that can.
    ///
    /// No default, so a wrapper can't answer `false` for a backend that says
    /// `true`.
    fn forks_read_shared_mask(&self) -> bool;
}

/// A backend that emits from the lowered IR: `stencil` and `llvm`.
///
/// Emission is two steps. [`Native::resolve`] reads every intrinsic key the
/// lowered program names and answers the route this backend emits each by, or
/// every key it has none for; [`Native::emit_units`] then reads a key's route
/// by the id `middle::lower` gave it, which is total. So the keys the build
/// refuses are exactly the keys emission would have had no route for, and
/// emission has no "a key nobody checked" case to state.
///
/// A trait of its own rather than more of [`Backend`] because JavaScript emits
/// from the monomorphized tree and has no lowered program to resolve.
pub trait Native: Backend {
    /// What [`Native::resolve`] answers: a route per key, borrowing the keys
    /// from the program it read them in.
    type Routes<'p>;

    /// Every key `lowered` names, resolved — or every key this backend has no
    /// route for, sorted, each once.
    fn resolve<'p>(&self, lowered: &'p ir::Program) -> Result<Self::Routes<'p>, Vec<String>>;

    /// The units the caller still needs, from `lowered` — which is
    /// `lower::run(program, tables)` or the same lowering against the caller's
    /// own plan — and the routes [`Native::resolve`] answered for it.
    ///
    /// Restricted to `units` because `Func::unit` already partitions the
    /// program (ARCHITECTURE.md §5.1) and the build system already knows which
    /// units' keys missed, so invalidating one unit costs one unit's codegen
    /// rather than the whole program's.
    ///
    /// Answers one object for each unit `units` names and for no other, in
    /// unit order.
    fn emit_units(
        &mut self,
        program: &Program,
        tables: &Tables,
        opts: &Options<'_>,
        lowered: &ir::Program,
        routes: &Self::Routes<'_>,
        units: Units<'_>,
    ) -> Result<Vec<Emitted>, Diagnostics>;
}

/// The refusal for keys a backend has no route for: a toolchain bug, which is
/// what the fix says.
pub fn no_implementation(backend: &str, keys: &[String]) -> Diagnostic {
    Diagnostic::error(
        Span::NONE,
        format!(
            "the {backend} backend has no implementation of {}",
            keys.join(", ")
        ),
    )
    .with_fix("report it: this is a toolchain bug, not a problem with your program")
}

/// A lowered program's two key tables, each key resolved.
type Resolved<C, B> = (Dense<ir::IntrinsicId, C>, Dense<ir::RuntimeId, B>);

/// Both of a lowered program's key tables resolved, one key at a time — the
/// calls by `call` and the runtime bodies by `body` — or every key either has
/// no route for, sorted and each once.
pub fn resolve_program<'p, C, B>(
    lowered: &'p ir::Program,
    call: impl Fn(&'p str) -> Option<C>,
    body: impl Fn(&'p str) -> Option<B>,
) -> Result<Resolved<C, B>, Vec<String>> {
    let (calls, mut missing) = resolve_keys(&lowered.intrinsics, call);
    let (bodies, more) = resolve_keys(&lowered.runtimes, body);
    missing.extend(more);
    missing.sort();
    missing.dedup();
    calls.zip(bodies).ok_or(missing)
}

/// A key table resolved by `route`, and the keys it has none for. The table is
/// `None` exactly when a key is.
fn resolve_keys<'p, I: DenseId, R>(
    keys: &'p Dense<I, Box<str>>,
    route: impl Fn(&'p str) -> Option<R>,
) -> (Option<Dense<I, R>>, Vec<String>) {
    let routes: Vec<Option<R>> = keys.iter().map(|key| route(key)).collect();
    let missing = keys
        .iter()
        .zip(&routes)
        .filter(|(_, route)| route.is_none())
        .map(|(key, _)| key.to_string())
        .collect();
    (routes.into_iter().collect(), missing)
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

/// [`split_networking`], with the toolchain's answer as a parameter, for the
/// reason [`networking_gap_when`] takes one.
///
/// `pub` because the answer is per **target** now, not per toolchain: a cross
/// link is against an archive with fewer features than the host's, so
/// `build::actions` hands the target's own `net` here rather than the baked
/// constant `split_networking` reads.
pub fn split_networking_when(missing: &[String], net: bool) -> (Vec<String>, Vec<String>) {
    missing
        .iter()
        .cloned()
        .partition(|key| !net && runtime_native::net_intrinsic(key))
}

/// The refusal for operations this toolchain's runtime has no networking for.
///
/// Templated, because the wording is the page's: a reader who meets this has a
/// toolchain to replace rather than a program to fix, and "report it" — the
/// fix every other missing intrinsic carries — would be the wrong instruction.
pub fn no_networking(operations: &[String], span: Span) -> Diagnostic {
    Diagnostic::templated("networking-unavailable", span)
        .with_bind("operations", crate::diagnostics::names(operations))
}

/// [`split_cryptography`], with the toolchain's answer as a parameter.
///
/// `pub` for [`split_networking_when`]'s reason: `build::actions` supplies the
/// cross target's own `crypto`, which is off on the first cross target.
pub fn split_cryptography_when(missing: &[String], crypto: bool) -> (Vec<String>, Vec<String>) {
    missing
        .iter()
        .cloned()
        .partition(|key| !crypto && runtime_native::crypto_intrinsic(key))
}

/// The refusal for operations this toolchain's runtime has no cryptography for.
///
/// [`no_networking`]'s twin, and templated for its reason: a reader who meets
/// this has a toolchain to replace rather than a program to fix.
pub fn no_cryptography(operations: &[String], span: Span) -> Diagnostic {
    Diagnostic::templated("cryptography-unavailable", span)
        .with_bind("operations", crate::diagnostics::names(operations))
}
