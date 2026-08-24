//! The target machine and the pass pipeline. **Wave 2b.**
//!
//! CODEGEN-LLVM.md §4. Four targets: `x86_64` and `aarch64`, Darwin and Linux,
//! selected by `Target::from_triple` from the `Options::target` the build
//! system already carries as a `(Platform, Option<Arch>)` pair.

use inkwell::module::Module;
use inkwell::passes::PassBuilderOptions;
use inkwell::targets::{
    CodeModel, FileType, InitializationConfig, RelocMode, Target as LlvmTarget, TargetMachine,
    TargetTriple,
};
use inkwell::OptimizationLevel;

use crate::build::buildfile::{Arch, Platform};
use crate::compiler::backend::{Profile, Target};

/// The LLVM triple for a build target.
///
/// `Arch` is `None` where the build did not name one, and the host's is the
/// answer then — the same rule `cli/build.rs` uses to build the runtime
/// archive, which is what the objects are linked against. Naming a different
/// architecture than the archive was built for is a link error rather than a
/// miscompile, which is the right failure.
pub fn triple(target: Target) -> Result<String, String> {
    let arch = match target.arch {
        Some(Arch::X86_64) => "x86_64",
        Some(Arch::Arm64) => "aarch64",
        None if cfg!(target_arch = "aarch64") => "aarch64",
        None => "x86_64",
    };
    match target.platform {
        // Apple spells the vendor, and `ld64` wants the whole triple. No
        // deployment version: the object the linker is handed carries what
        // `cc` puts on the command line, and pinning one here would make a
        // toolchain refuse to link on a newer SDK.
        Platform::Macos => Ok(format!("{arch}-apple-darwin")),
        Platform::Linux => Ok(format!("{arch}-unknown-linux-gnu")),
        Platform::Js | Platform::Web => Err(String::from(
            "the LLVM backend does not emit JavaScript; that is the `js` backend",
        )),
    }
}

/// Initializes exactly the two target families this compiler emits for.
///
/// `initialize_all` would pull every backend LLVM was built with into the
/// process — thirty of them on a distribution build — for no gain: a triple
/// naming anything else is refused by [`triple`] before this is reached.
/// It also happens exactly once per process. LLVM's target registry is
/// process-global mutable state with no lock of its own, so two threads
/// registering at the same time is a data race that shows up as a `SIGABRT`
/// inside LLVM with no stack to read — which is what a test binary running its
/// cases in parallel does immediately.
pub fn initialize() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let config = InitializationConfig::default();
        LlvmTarget::initialize_x86(&config);
        LlvmTarget::initialize_aarch64(&config);
    });
}

/// The machine to optimize and emit against.
///
/// `Reloc::PIC` on both platforms: macOS has required position-independent
/// executables since 10.7, and every mainstream Linux distribution defaults
/// `cc` to `-pie`. Emitting non-PIC objects would work until the linker was
/// asked for a PIE, and then fail with a relocation error naming a section
/// rather than a decision.
///
/// **No `-mcpu=native`.** The generic CPU for the triple, always. A compiler
/// whose central claim is byte-identical output cannot let the machine it ran
/// on enter the bytes, and `Backend::identity` has no way to report a host
/// feature string that `llvm-config` did not choose.
pub fn machine(triple_text: &str, profile: Profile) -> Result<TargetMachine, String> {
    initialize();
    let triple = TargetTriple::create(triple_text);
    let target = LlvmTarget::from_triple(&triple)
        .map_err(|e| format!("LLVM has no backend for `{triple_text}`: {e}"))?;
    target
        .create_target_machine(
            &triple,
            "generic",
            "",
            optimization(profile),
            RelocMode::PIC,
            CodeModel::Default,
        )
        .ok_or_else(|| format!("LLVM could not create a target machine for `{triple_text}`"))
}

/// `-O0` in debug and `-O2` in release, for the *machine*'s own choices
/// (register allocation, scheduling). The IR-level pipeline is [`optimize`].
fn optimization(profile: Profile) -> OptimizationLevel {
    match profile {
        Profile::Debug => OptimizationLevel::None,
        Profile::Release => OptimizationLevel::Default,
    }
}

/// The pass pipeline: `default<O2>`, as a pipeline string.
///
/// **`default<O2>`, not `default<O3>`.** O3 differs mainly in unroll and
/// vectorize aggressiveness and in inline thresholds; it costs code size and
/// compile time for a gain that is workload-dependent and usually small, and O2
/// is what production toolchains ship by default for the same reason.
///
/// **A pipeline string, not a hand-assembled pass list.** A custom pipeline is
/// a list that has to be re-derived and re-tuned at every LLVM bump, against a
/// pass manager whose pass names and orderings are internal. `default<O2>` is
/// the pipeline LLVM's own developers regression-test.
///
/// The legacy `PassManager` is `#[llvm_versions(..=16)]` in inkwell and does
/// not exist at all on LLVM 17 and above, so there is no choice to make and no
/// migration to plan. `Module::run_passes` requires a `&TargetMachine`, which
/// is why the target is initialized before any optimization even in a
/// hypothetical IR-only run.
///
/// `FunctionValue::run_passes` exists on LLVM 20+ and is not used: the unit is
/// the module (ARCHITECTURE.md §5), and a per-function pipeline would lose the
/// module-scoped passes the unit boundary is already narrow enough to cost.
pub fn optimize(
    module: &Module<'_>,
    machine: &TargetMachine,
    profile: Profile,
) -> Result<(), String> {
    let options = PassBuilderOptions::create();
    // Monomorphization produces structurally identical functions at different
    // types — `Option<A>` and `Option<B>` where both are pointer-sized, every
    // `[T]` helper at two pointer-shaped `T`s. Merging them is free size.
    options.set_merge_functions(true);
    // Loop unrolling and loop vectorization are left at their defaults, which
    // is on: the point of `--release` on an `[Int]` fold is the vectorizer.
    //
    // `verify_each` checks *our* IR rather than LLVM's, which is the same rule
    // Cranelift's verifier is set by (CODEGEN-CRANELIFT.md §4): a developer
    // build pays for the check and a user's build does not.
    options.set_verify_each(cfg!(debug_assertions));
    let pipeline = match profile {
        Profile::Release => "default<O2>",
        // A debug build still runs a pipeline, because this backend is only
        // ever selected for `--release` (`backend::select`) — the string is
        // here so that a debug run of the emitter for a *test* produces IR a
        // human can read against the source.
        Profile::Debug => "default<O0>",
    };
    module
        .run_passes(pipeline, machine, options)
        .map_err(|e| format!("the `{pipeline}` pipeline failed: {e}"))
}

/// One optimized module as an object file.
pub fn object(module: &Module<'_>, machine: &TargetMachine) -> Result<Vec<u8>, String> {
    machine
        .write_to_memory_buffer(module, FileType::Object)
        .map(|buffer| buffer.as_slice().to_vec())
        .map_err(|e| format!("LLVM could not emit an object file: {e}"))
}
