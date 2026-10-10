//! The backends, and the choice between them.
//!
//! What every backend has in common — the [`Backend`] trait, [`Emitted`],
//! [`Profile`] — is `buri-backend`'s, re-exported here. Each backend is a crate
//! of its own:
//!
//! ```text
//! buri-js       always compiled in
//! buri-stencil  behind `backend-stencil`, on by default
//! buri-llvm     behind `backend-llvm`, off by default
//! ```
//!
//! What stays here needs all three or the runtime archive: [`select`], and the
//! capability gaps that read the archive's features.
//!
//! [`select`] answers `stencil` for every native debug build it can, and
//! refuses by triple where the stencil backend has no library or no entry
//! point. There is no third native backend and no crate behind the default
//! toolchain: `design/native/CODEGEN-STENCIL.md` is the whole of it.
//!
//! Design: `design/native/ARCHITECTURE.md` §3.

pub use buri_js::compiler::backend::js;

// What every backend shares is `buri-backend`'s. What is here names all three
// backends or the runtime archive `cli/build.rs` builds.
pub use buri_backend::compiler::backend::*;

/// The native runtime archive both native backends link against, built by
/// `cli/build.rs`, and `buri-backend`'s symbol rule.
pub mod runtime_native;

/// The LLVM backend, with the runtime archive's gaps folded in.
#[cfg(feature = "backend-llvm")]
pub mod llvm {
    pub use buri_llvm::compiler::backend::llvm::*;

    /// The backend [`super::select`] hands a native release build.
    pub type Llvm = super::WithRuntime<buri_llvm::compiler::backend::llvm::Llvm>;
}

/// The copy-and-patch backend, with the runtime archive's gaps folded in.
#[cfg(feature = "backend-stencil")]
pub mod stencil {
    pub use buri_stencil::compiler::backend::stencil::*;

    /// The backend [`super::select`] hands a native debug build.
    pub type Stencil = super::WithRuntime<buri_stencil::compiler::backend::stencil::Stencil>;
}

use crate::build::buildfile::Platform;
use crate::compiler::middle::monomorphize::Program;
use crate::compiler::semantics::types::Tables;
use crate::diagnostics::Diagnostics;

/// A native backend, with the keys this toolchain's runtime archive cannot
/// answer folded into [`Backend::missing_intrinsics`].
///
/// The backends sit below the archive, which `cli/build.rs` builds for this
/// crate, so they answer from their own surface and this adds
/// [`networking_gap`] and [`cryptography_gap`]: a program reaching networking
/// on a runtime that has none is refused before codegen rather than at `cc`
/// time with a symbol nobody outside this repository can read. Every other
/// method, and through `Deref` every inherent one, is the backend's own.
#[derive(Default)]
pub struct WithRuntime<B>(B);

impl<B> std::ops::Deref for WithRuntime<B> {
    type Target = B;

    fn deref(&self) -> &B {
        &self.0
    }
}

impl<B> std::ops::DerefMut for WithRuntime<B> {
    fn deref_mut(&mut self) -> &mut B {
        &mut self.0
    }
}

impl<B: Backend> Backend for WithRuntime<B> {
    fn name(&self) -> &'static str {
        self.0.name()
    }

    fn identity(&self) -> String {
        self.0.identity()
    }

    fn missing_intrinsics(&self, program: &Program, tables: &Tables) -> Vec<String> {
        let mut missing = self.0.missing_intrinsics(program, tables);
        missing.extend(networking_gap(program));
        missing.extend(cryptography_gap(program));
        missing.sort();
        missing.dedup();
        missing
    }

    fn emit(
        &mut self,
        program: &Program,
        tables: &Tables,
        opts: &Options<'_>,
    ) -> Result<Vec<Emitted>, Diagnostics> {
        self.0.emit(program, tables, opts)
    }

    fn emit_units(
        &mut self,
        program: &Program,
        tables: &Tables,
        opts: &Options<'_>,
        units: Units<'_>,
    ) -> Result<Vec<Emitted>, Diagnostics> {
        self.0.emit_units(program, tables, opts, units)
    }

    fn adopt_lowering(&mut self, lowered: crate::compiler::middle::ir::Program) {
        self.0.adopt_lowering(lowered);
    }

    fn forks_read_shared_mask(&self) -> bool {
        self.0.forks_read_shared_mask()
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

/// [`split_networking`] for the cryptography half.
///
/// Applied to what the networking split left rather than replacing it, so a
/// third cause is one more line at each site and not a wider tuple everywhere.
/// The two families are disjoint — `net_intrinsic` and `crypto_intrinsic` match
/// different effects — so the order the two splits run in cannot matter.
pub fn split_cryptography(missing: &[String]) -> (Vec<String>, Vec<String>) {
    split_cryptography_when(missing, runtime_native::crypto())
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
                Ok(_) => Ok(Box::new(stencil::Stencil::default())),
                Err(why) => Err(no_development_backend(target, &why)),
            }
        }
        // Gated the other way, not left as a fallback: with the feature on the
        // arm above is total for a native debug build, and an arm nothing can
        // reach is a warning rather than a safety net.
        #[cfg(not(feature = "backend-stencil"))]
        (_, Profile::Debug) => Err(no_development_code_generator()),
        #[cfg(feature = "backend-llvm")]
        (Platform::Linux | Platform::Macos, Profile::Release) => Ok(Box::new(llvm::Llvm::default())),
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
    use crate::build::buildfile::Arch;
    use crate::diagnostics::Span;

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
                ret: Ty::UNIT,
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
            cell_equal: Default::default(),
            ctx_layouts: Default::default(),
            shapes: Default::default(),
            stylesheet: String::new(),
            inline_styles: false,
            inline_animations: false,
            icons: false,
            tooltips: false,
            themes: false,
            chunks: Vec::new(),
            hosted: Default::default(),
            instances: Default::default(),
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
            "host.HostFileSystem.readFile",
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
        let program = program_using(&["host.HostListen.listen", "host.HostFileSystem.readFile"]);
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
        assert_eq!(refusal.code.as_deref(), Some("networking-unavailable"));
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
        assert_eq!(refusal.code.as_deref(), Some("cryptography-unavailable"));
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
    #[cfg(any(feature = "backend-stencil", feature = "backend-llvm"))]
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
            reports(stencil::Stencil::default().missing_intrinsics(&program, &tables)),
            "the stencil backend claimed a key no runtime answers"
        );
        #[cfg(feature = "backend-llvm")]
        assert!(
            reports(llvm::Llvm::default().missing_intrinsics(&program, &tables)),
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
