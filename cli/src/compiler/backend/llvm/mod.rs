//! The LLVM backend. **Wave 2b.**
//!
//! The release backend: chosen for `(Linux | Macos, Release)`. Through
//! `inkwell`, against LLVM 21.
//!
//! Behind the `backend-llvm` feature, which is **off by default**: it needs
//! LLVM 21 installed and `LLVM_SYS_211_PREFIX` set, and `cargo install buri`
//! must not require that.
//!
//! A toolchain built without this feature refuses a native `--release` build
//! with a diagnostic naming the feature. It does not fall back to Cranelift —
//! a `--release` build that silently produced different code depending on how
//! the compiler happened to be installed is the same class of bug as an
//! unpinned toolchain. The hazard that would normally follow, two `buri`
//! binaries with identical sources and different capabilities, is already
//! closed by `build/toolchain.rs`: it hashes the running executable, and a
//! binary built with `backend-llvm` is a different executable with a different
//! hash.
//!
//! ```text
//! llvm/
//!   mod.rs        this file: the `Backend` impl, the unit loop, the keys
//!   repr.rs       the value model in LLVM types (VALUE-MODEL.md §5.1)
//!   attrs.rs      the attribute discipline (CODEGEN-LLVM.md §3)
//!   emit.rs       one module: blocks, phis, instructions (§2)
//!   runtime.rs    the `buri_rt_*` boundary (`cli/runtime/lib.rs`)
//!   target.rs     the triple, the machine, `default<O2>` (§4)
//! ```
//!
//! # Where this backend sits in the pipeline, and the one seam it cannot close
//!
//! [`Backend::emit`] is handed the **layer-A** program — `monomorphize::Program`
//! after `middle::run` — and takes it by shared reference. `middle::native`
//! (`derives`, `closures`, `rc`) needs `&mut`, so this backend cannot run it,
//! and lowering a tree that has not been through it produces
//! `ir::Inst::Structural` placeholders and unlifted lambdas. A caller that has
//! not run `middle::native` therefore gets a diagnostic naming the pass rather
//! than an object file that is quietly wrong. Wiring `middle::native` into
//! `actions::emit` beside the `middle::run` already there is wave 2c's, and
//! [`emit_lowered`] is the entry point that takes an already-lowered
//! `ir::Program` for anything that has done it.
//!
//! # Two things this backend deliberately does not emit
//!
//!  * **`.buri_symbols`.** CODEGEN-CRANELIFT.md §5 has the debug backend emit a
//!    sorted `(address, name)` array so `buri_rt_abort` can walk the
//!    frame-pointer chain and name the frames. It is a *debug* feature — the
//!    same section says the escape hatch for release is DWARF, which
//!    CODEGEN-LLVM.md §7 gives this backend from wave 3 — and this backend is
//!    only ever selected for `--release`. There is also nothing to be in parity
//!    with yet: wave 2a's Cranelift backend lists the section in its header and
//!    does not write one.
//!  * **Debug info.** CODEGEN-LLVM.md §7, from wave 3 onward.
//!
//! Design: `design/native/CODEGEN-LLVM.md`, `BUILD-AND-WATCH.md` §2, §2.1.

pub mod attrs;
pub mod emit;
pub mod repr;
pub mod runtime;
pub mod target;

use inkwell::context::Context;

use crate::build::cache::{hash_bytes, ActionKey};
use crate::compiler::backend::{Backend, Emitted, Options};
use crate::compiler::middle::{ir, lower, monomorphize};
use crate::compiler::semantics::types::{FuncIdx, Tables};
use crate::diagnostics::{Diagnostic, Diagnostics, Span};

/// The release backend.
///
/// A plain owned object: an LLVM `Context` is not `Sync` and owns everything
/// built inside it, so one is created per [`emit_lowered`] call and dropped
/// with the modules it produced. Holding a `Context` in this struct would tie
/// the backend's lifetime to a context's and make `Backend` object-unsafe for
/// the one implementor that most wants a plain object.
#[derive(Default)]
pub struct Llvm;

impl Backend for Llvm {
    fn name(&self) -> &'static str {
        "llvm"
    }

    /// The LLVM version this binary is linked against, and the inkwell version
    /// it speaks through. Both enter every cache key.
    ///
    /// A constant would be a lie here, and this is the backend the trait's
    /// documentation names when it says so: `llvm-sys` links against whatever
    /// `llvm-config` found at build time, so two `buri` binaries with identical
    /// Rust source can have different LLVM underneath and produce different
    /// objects. `support::get_llvm_version` asks the library rather than the
    /// feature flag, so a `strict-versioning` mismatch that slipped through
    /// still moves the key.
    fn identity(&self) -> String {
        let (major, minor, patch) = inkwell::support::get_llvm_version();
        format!("llvm {major}.{minor}.{patch} inkwell 0.10")
    }

    /// Which intrinsic keys this backend has nothing for, asked of the program
    /// rather than accumulated as a side effect of a failed emission.
    ///
    /// [`emit::implemented`] is the single predicate: a key is answered by a
    /// [`runtime::ENTRIES`] row, by an inline sequence, or by a generated body,
    /// and everything else — `json.*`, every `list.*` entry taking a closure,
    /// `checked*` and `saturating*` — is named here rather than discovered as a
    /// link error or, worse, as a wrong answer. So a program that reaches
    /// outside this backend's surface is told so *before* LLVM is started,
    /// which is the whole reason this hook is on the trait.
    ///
    /// Two things it cannot answer, both stated so the absence reads as a
    /// consequence rather than an oversight. `derivePrimShow` and
    /// `derivePrimHash` are claimed at the key and still refuse the arms whose
    /// primitive `middle::lower` erased (`emit::Unit::derived`); and a
    /// structural operation on a type `middle::derives` did not reach is an
    /// `ir::Inst::Structural`, which exists only after lowering and is
    /// therefore not in this program at all.
    fn missing_intrinsics(&self, program: &monomorphize::Program, _tables: &Tables) -> Vec<String> {
        let mut missing: Vec<String> = program
            .funcs
            .iter()
            .filter_map(|f| match &f.kind {
                monomorphize::FuncKind::Intrinsic(key) => Some(key.clone()),
                _ => None,
            })
            .filter(|key| !emit::implemented(key))
            .collect();
        // `str.concat` is emitted by `lower::template` at every interpolation
        // and never appears as a `FuncKind::Intrinsic`, so scanning the
        // function list alone would miss the single most common way a program
        // could leave this backend's surface. It is open-coded now, so this
        // adds nothing — the test is kept rather than deleted so that removing
        // the open-coding restores the diagnostic instead of silently removing
        // it.
        if program.funcs.iter().any(uses_template) && !emit::implemented("str.concat") {
            missing.push(String::from("str.concat"));
        }
        missing.sort();
        missing.dedup();
        missing
    }

    fn emit(
        &mut self,
        program: &monomorphize::Program,
        tables: &Tables,
        opts: &Options<'_>,
    ) -> Result<Vec<Emitted>, Diagnostics> {
        let missing = self.missing_intrinsics(program, tables);
        if !missing.is_empty() {
            let mut diags = Diagnostics::new();
            diags.push(
                Diagnostic::error(
                    Span::NONE,
                    format!("the native runtime has no implementation of {}", missing.join(", ")),
                )
                .with_fix("report it: this is a toolchain bug, not a problem with your program"),
            );
            return Err(diags);
        }
        let entry = match program.roots {
            monomorphize::ProgramRoots::Main(idx) => Some(idx),
            monomorphize::ProgramRoots::Tests(_) => None,
        };
        let lowered = lower::run(program, tables);
        emit_lowered(&lowered, tables, opts, entry)
    }
}

fn uses_template(f: &monomorphize::Func) -> bool {
    let Some(body) = f.body() else { return false };
    let mut found = false;
    crate::compiler::semantics::typed::walk(body, &mut |e| {
        if matches!(e.kind, crate::compiler::semantics::typed::ExprKind::Template { .. }) {
            found = true;
        }
    });
    found
}

/// One object file per codegen unit, from an already-lowered program.
///
/// This is the real entry point: [`Backend::emit`] is a thin wrapper that
/// lowers first, and everything that already holds an `ir::Program` — the
/// tests, and wave 2c's action graph once `middle::native` is wired in — calls
/// this.
///
/// The partition is the middle end's: a codegen unit is the set of
/// monomorphized functions whose declaration came from one source module
/// (ARCHITECTURE.md §5.1), so functions that call each other land in one
/// `.text` section next to each other. Emission order within a unit is the
/// middle end's function order, which is the monomorphization worklist's —
/// deterministic, and derived from the reachability walk out of the entry point
/// rather than from a hash order, which is a free first approximation of a
/// call-order layout (CODEGEN-LLVM.md §6).
pub fn emit_lowered(
    program: &ir::Program,
    tables: &Tables,
    opts: &Options<'_>,
    entry: Option<FuncIdx>,
) -> Result<Vec<Emitted>, Diagnostics> {
    let mut diags = Diagnostics::new();
    let triple = match target::triple(opts.target) {
        Ok(t) => t,
        Err(message) => {
            diags.push(Diagnostic::error(Span::NONE, message));
            return Err(diags);
        }
    };
    let machine = match target::machine(&triple, opts.profile) {
        Ok(m) => m,
        Err(message) => {
            diags.push(Diagnostic::error(Span::NONE, message).with_fix(
                "install LLVM 21 and rebuild the toolchain, or build with `--output=js`",
            ));
            return Err(diags);
        }
    };
    let data_layout = machine.get_target_data().get_data_layout();

    let identity = Llvm.identity();
    let mut out = Vec::with_capacity(program.units.len());
    for (index, unit_name) in program.units.iter().enumerate() {
        let unit = index as u32;
        let members: Vec<usize> = program
            .funcs
            .iter()
            .enumerate()
            .filter(|(_, f)| f.unit == unit && f.code().is_some())
            .map(|(i, _)| i)
            .collect();
        let owns_entry = entry.is_some_and(|e| {
            program.funcs.get(e.index()).is_some_and(|f| f.unit == unit && f.code().is_some())
        });
        if members.is_empty() && !owns_entry {
            continue;
        }

        let ctx = Context::create();
        let module_name = format!("{}{unit_name}", opts.unit_prefix);
        let mut emitter = emit::Unit::new(&ctx, program, tables, &module_name, opts.profile);
        emitter.module.set_triple(&inkwell::targets::TargetTriple::create(&triple));
        emitter.module.set_data_layout(&data_layout);
        for member in &members {
            emitter.define(FuncIdx(*member as u32));
        }
        if let (Some(e), true) = (entry, owns_entry) {
            emitter.entry_point(e);
        }
        // The generated helpers — a closure's thunk, the per-type drop glue —
        // are asked for from inside a function body and built here, after every
        // body is complete: there is one builder, and a declared function with
        // no body is a link error rather than a wrong answer.
        emitter.finish();
        if emitter.diags.has_errors() {
            diags.extend(emitter.diags.items);
            return Err(diags);
        }
        // The verifier checks *our* IR rather than LLVM's, which is the same
        // rule Cranelift's verifier is set by (CODEGEN-CRANELIFT.md §4): a
        // developer build pays for the check and a user's build does not.
        if cfg!(debug_assertions) {
            if let Err(message) = emitter.module.verify() {
                diags.push(
                    Diagnostic::error(
                        Span::NONE,
                        format!(
                            "internal error: the LLVM backend emitted invalid IR for unit \
                             `{unit_name}`: {message}"
                        ),
                    )
                    .with_fix("this is a toolchain bug; report it"),
                );
                return Err(diags);
            }
        }
        if let Err(message) = target::optimize(&emitter.module, &machine, opts.profile) {
            diags.push(Diagnostic::error(Span::NONE, message));
            return Err(diags);
        }
        let bytes = match target::object(&emitter.module, &machine) {
            Ok(b) => b,
            Err(message) => {
                diags.push(Diagnostic::error(Span::NONE, message));
                return Err(diags);
            }
        };
        out.push(Emitted {
            name: format!("{unit_name}.o"),
            key: codegen_key(program, unit, &identity, &triple, opts),
            bytes,
        });
    }
    Ok(out)
}

/// The optimized IR of one unit, as text. What a FileCheck-style assertion
/// reads, and what `--explain` would print.
///
/// It runs the same emitter and the same pipeline as [`emit_lowered`] and
/// stops one step earlier, so an assertion about the IR is an assertion about
/// the object rather than about a second code path that resembles it.
pub fn emit_ir_text(
    program: &ir::Program,
    tables: &Tables,
    opts: &Options<'_>,
    entry: Option<FuncIdx>,
    unit: u32,
) -> Result<String, Diagnostics> {
    let mut diags = Diagnostics::new();
    let triple = target::triple(opts.target).map_err(|m| {
        let mut d = Diagnostics::new();
        d.push(Diagnostic::error(Span::NONE, m));
        d
    })?;
    let machine = target::machine(&triple, opts.profile).map_err(|m| {
        let mut d = Diagnostics::new();
        d.push(Diagnostic::error(Span::NONE, m));
        d
    })?;
    let ctx = Context::create();
    let name = program.unit_name(unit).to_string();
    let mut emitter = emit::Unit::new(&ctx, program, tables, &name, opts.profile);
    emitter.module.set_triple(&inkwell::targets::TargetTriple::create(&triple));
    emitter.module.set_data_layout(&machine.get_target_data().get_data_layout());
    for (i, f) in program.funcs.iter().enumerate() {
        if f.unit == unit && f.code().is_some() {
            emitter.define(FuncIdx(i as u32));
        }
    }
    if let Some(e) = entry.filter(|e| {
        program.funcs.get(e.index()).is_some_and(|f| f.unit == unit && f.code().is_some())
    }) {
        emitter.entry_point(e);
    }
    emitter.finish();
    if emitter.diags.has_errors() {
        diags.extend(emitter.diags.items);
        return Err(diags);
    }
    // The same verification `emit_lowered` does, for the same reason: an
    // assertion about the IR is only an assertion about the object if the two
    // came out of the same checks.
    if let Err(message) = emitter.module.verify() {
        diags.push(Diagnostic::error(
            Span::NONE,
            format!("internal error: the LLVM backend emitted invalid IR: {message}"),
        ));
        return Err(diags);
    }
    if let Err(message) = target::optimize(&emitter.module, &machine, opts.profile) {
        diags.push(Diagnostic::error(Span::NONE, message));
        return Err(diags);
    }
    Ok(emitter.module.to_string())
}

/// `codegen_key(unit) = H(backend, identity, triple, profile, the unit's IR)`.
///
/// Content-addressed **on the IR**, which is the convention `actions::codegen_key`
/// states and the decision the whole incremental story rests on: keying a unit
/// on the sources of the module it came from is unsound, because a
/// monomorphized unit contains instantiations requested by other modules.
///
/// `ir::Program`'s `Display` is a faithful, total and deterministic function of
/// the IR — no hash order anywhere, every name derived from the program — which
/// is what makes hashing its bytes per unit a correct way to compute this and
/// the one that can be inspected when a key changes and nobody knows why
/// (`ir.rs`, "Printing").
///
/// The build system's own `actions::codegen_key` wraps this with the toolchain
/// hash and the platform; both must move when either input does, which is why
/// the backend's identity is in *this* half rather than only in that one.
fn codegen_key(
    program: &ir::Program,
    unit: u32,
    identity: &str,
    triple: &str,
    opts: &Options<'_>,
) -> ActionKey {
    let mut text = String::new();
    text.push_str("llvm\n");
    text.push_str(identity);
    text.push('\n');
    text.push_str(triple);
    text.push('\n');
    text.push_str(opts.profile.name());
    text.push('\n');
    for f in program.funcs.iter().filter(|f| f.unit == unit) {
        text.push_str(&program.render_func(f));
    }
    ActionKey::of(hash_bytes(text.as_bytes()).as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The identity names the library rather than the feature flag, so a
    /// toolchain built against a different LLVM produces a different key.
    #[test]
    fn the_identity_names_the_linked_llvm() {
        let id = Llvm.identity();
        assert!(id.starts_with("llvm 21."), "{id}");
        assert!(id.contains("inkwell"), "{id}");
    }

    /// Four targets, and the one that is not this backend's.
    #[test]
    fn the_triples_are_the_four_supported_targets() {
        use crate::build::buildfile::{Arch, Platform};
        use crate::compiler::backend::Target;
        let t = |platform, arch| target::triple(Target { platform, arch });
        assert_eq!(t(Platform::Macos, Some(Arch::Arm64)).as_deref(), Ok("aarch64-apple-darwin"));
        assert_eq!(t(Platform::Macos, Some(Arch::X86_64)).as_deref(), Ok("x86_64-apple-darwin"));
        assert_eq!(
            t(Platform::Linux, Some(Arch::Arm64)).as_deref(),
            Ok("aarch64-unknown-linux-gnu")
        );
        assert_eq!(
            t(Platform::Linux, Some(Arch::X86_64)).as_deref(),
            Ok("x86_64-unknown-linux-gnu")
        );
        assert!(t(Platform::Js, None).is_err());
    }
}
