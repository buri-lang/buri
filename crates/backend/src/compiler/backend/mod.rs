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
use crate::build::buildfile::{Arch, Platform};
use crate::build::cache::ActionKey;
use crate::compiler::middle::monomorphize::Program;
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
    /// backend with one unit — JavaScript, whose artifact is one file — wants
    /// exactly that.
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

    /// The lowering the build already computed for this exact program, offered
    /// so that a native backend does not compute it a second time.
    ///
    /// It is a **hint and not a seam**: the emission entry point is still
    /// [`Backend::emit_units`] and it still takes a `Program`, so a backend
    /// that ignores this — JavaScript — is not a backend that works
    /// differently. What is offered is a value the caller is about to
    /// throw away and the callee is about to recompute:
    /// `build::actions::objects_named` lowers to hash the unit keys, and
    /// `middle::lower` is a pure function of the program, so the IR it holds is
    /// the IR the backend's own `lower::run` would produce. It was measured at
    /// one second of an eight-second `buri test //...` on a real repository —
    /// `middle::rc::analyze` twice and `middle::lower::run_with` twice — which
    /// is what makes a hint worth having.
    ///
    /// **The contract is that `lowered` is `lower::run(program, tables)` for
    /// the `program` the next emission is asked about.** An implementation must
    /// consume it at most once, so that a second emission of a *different*
    /// program cannot be served a stale lowering; the default ignores it, which
    /// satisfies that by construction.
    fn adopt_lowering(&mut self, lowered: crate::compiler::middle::ir::Program) {
        let _ = lowered;
    }

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
    missing.iter().cloned().partition(|key| !net && runtime_native::net_intrinsic(key))
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
    missing.iter().cloned().partition(|key| !crypto && runtime_native::crypto_intrinsic(key))
}

/// The refusal for operations this toolchain's runtime has no cryptography for.
///
/// [`no_networking`]'s twin, and templated for its reason: a reader who meets
/// this has a toolchain to replace rather than a program to fix.
pub fn no_cryptography(operations: &[String], span: Span) -> Diagnostic {
    Diagnostic::templated("cryptography-unavailable", span)
        .with_bind("operations", crate::diagnostics::names(operations))
}
