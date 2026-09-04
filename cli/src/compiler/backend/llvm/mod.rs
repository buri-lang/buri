//! The LLVM backend.
//!
//! The release backend: chosen for `(Linux | Macos, Release)`. Through
//! `inkwell`, against LLVM 21.
//!
//! Behind the `backend-llvm` feature, which is **off by default**: it needs
//! LLVM 21 installed and `LLVM_SYS_211_PREFIX` set, and `cargo install buri`
//! must not require that.
//!
//! A toolchain built without this feature refuses a native `--release` build
//! with a diagnostic naming the feature. It does not fall back to the debug
//! backend —
//! a `--release` build that silently produced different code depending on how
//! the compiler happened to be installed is the same class of bug as an
//! unpinned toolchain. The hazard that would normally follow, two `buri`
//! binaries with identical sources and different capabilities, is already
//! closed by `build/cache.rs`: every action key names the backend and its
//! `Backend::identity`, which here is the LLVM version this binary links
//! against, so two `buri` binaries with different LLVM underneath cannot
//! share a cache entry.
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
//! than an object file that is quietly wrong. `actions::prepare` is where the
//! composition lives — `middle::run`, then `middle::native` on a native
//! target — and [`emit_lowered`] is the entry point that takes an
//! already-lowered `ir::Program` for anything that has done it.
//!
//! # Two things this backend deliberately does not emit
//!
//!  * **`.buri_symbols`.** CODEGEN-STENCIL.md §11 wants a sorted
//!    `(address, name)` array so `buri_rt_abort` can walk the
//!    frame-pointer chain and name the frames. It is a *debug* feature — the
//!    same section says the escape hatch for release is DWARF, which
//!    CODEGEN-LLVM.md §7 specifies for this backend — and this backend is
//!    only ever selected for `--release`. There is also nothing to be in parity
//!    with: the debug backend lists the section as a gap of its own and does not
//!    write one either.
//!  * **Debug info.** CODEGEN-LLVM.md §7 specifies it and none is emitted yet,
//!    which is the same gap `stencil/mod.rs` records for itself.
//!
//! Design: `design/native/CODEGEN-LLVM.md`, `BUILD-AND-WATCH.md` §2, §2.1.

pub mod attrs;
pub mod emit;
pub mod repr;
pub mod runtime;
pub mod target;

use inkwell::context::Context;
use std::cell::RefCell;
use std::rc::Rc;

use crate::build::buildfile::{Arch, Platform};
use crate::build::cache::{hash_bytes, ActionKey};
use crate::compiler::backend::carrier;
use crate::compiler::backend::{triple_text, Backend, Emitted, Options, Target, Units};
use crate::compiler::middle::{ir, layout, lower, monomorphize, rc};
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

    /// The LLVM version this binary is linked against, the inkwell version it
    /// speaks through, and every triple this toolchain renders. All of them
    /// enter every cache key.
    ///
    /// A constant would be a lie here, and this is the backend the trait's
    /// documentation names when it says so: `llvm-sys` links against whatever
    /// `llvm-config` found at build time, so two `buri` binaries with identical
    /// Rust source can have different LLVM underneath and produce different
    /// objects. `support::get_llvm_version` asks the library rather than the
    /// feature flag, so a `strict-versioning` mismatch that slipped through
    /// still moves the key.
    ///
    /// # Why the triples are in it
    ///
    /// **To close an asymmetry with the other native backend.** `Stencil`'s
    /// identity is the digests of the libraries it bakes, so *how a target is
    /// named* reaches its key through the bytes that name buys: when the Linux
    /// triple went from `-gnu` to `-musl` those digests were rebuilt from an
    /// empty scratch directory and measured identical
    /// (`stencil::abi::StencilTarget::triple`), and a rename that had changed
    /// one byte of one shard would have moved every stencil key by itself.
    /// This backend had no such term. `llvm <version> inkwell 0.10` is the
    /// same string under both spellings, and [`crate::build::actions::codegen_key`]
    /// folds in the platform and the arch but not the triple — so a `.buri`
    /// cache written by a gnu-era toolchain would serve its `linux/arm64`
    /// objects to a musl one under a key that never moved.
    ///
    /// That today's emission is byte-identical across the rename is not a
    /// property a cache key may rest on. The relocation model, the TLS model,
    /// the stack-protector default and the unwinder are all derived from the
    /// triple inside LLVM, and two triples agreeing on all of them is a fact
    /// about one LLVM version rather than a rule.
    ///
    /// **Every triple, not this build's.** `Backend::identity(&self)` is told
    /// no target — `Stencil::identity` makes the same observation about its own
    /// digests and answers with all three libraries for it — so the honest
    /// answer here is the whole rendering. The term then moves when
    /// [`triple_text`] changes for *any* target rather than only for the one
    /// being built, which costs one conservative invalidation and buys the
    /// property: the alternative moves nothing and serves an object emitted for
    /// another triple.
    ///
    /// **It lands once.** `actions::codegen_key` folds this string in and
    /// states the platform and the arch beside it; this backend's own
    /// `codegen_key` folds it in beside *the* triple, which is what tells its
    /// per-unit keys for two targets apart. Neither key grows a second copy of
    /// the rendering.
    fn identity(&self) -> String {
        let (major, minor, patch) = inkwell::support::get_llvm_version();
        let mut id = format!("llvm {major}.{minor}.{patch} inkwell 0.10");
        // Derived from the platform list rather than written out beside it, so
        // that a fifth platform cannot leave this naming four. The
        // JavaScript ones render no triple and drop out here, which is the
        // same `None` `select` refuses them by.
        for platform in Platform::ALL {
            for arch in [Arch::Arm64, Arch::X86_64] {
                if let Some(triple) = triple_text(Target { platform, arch: Some(arch) }) {
                    id.push(' ');
                    id.push_str(&triple);
                }
            }
        }
        id
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
    /// One key it names that is not about this backend at all: an operation the
    /// **runtime archive** carries only behind its `net` feature
    /// ([`super::networking_gap`]). A toolchain built without it would
    /// otherwise meet networking as an unresolved `buri_rt_*` symbol at `cc`
    /// time, which is the one failure mode this hook exists to replace.
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
        // The second source: the keys *no* backend can answer on a toolchain
        // whose runtime archive was built without `net`, and the keys it cannot
        // answer without `crypto`. Both empty on an ordinary toolchain, and the
        // sentences they earn are `super::gap_refusals`'s rather than this
        // backend's.
        missing.extend(super::networking_gap(program));
        missing.extend(super::cryptography_gap(program));
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
        self.emit_units(program, tables, opts, Units::All)
    }

    /// The unit loop is already per unit; what `units` adds is the parameter
    /// that lets the build system say which of them it still needs. Everything
    /// above the loop — the triple, the machine, the lowering — is
    /// whole-program and is done once either way.
    fn emit_units(
        &mut self,
        program: &monomorphize::Program,
        tables: &Tables,
        opts: &Options<'_>,
        units: Units<'_>,
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
        let root = root_of(program);
        let lowered = lower::run(program, tables);
        emit_selected(&lowered, tables, opts, root, units, Some(&classifier(program)))
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
/// lowers first, and the native tests, which already hold an `ir::Program`,
/// call this directly. It is deliberately not on `Backend`, so the build's
/// action graph reaches it through [`Backend::emit_units`] rather than around
/// it (`build/actions.rs`).
///
/// The partition is the middle end's: a codegen unit is the set of
/// monomorphized functions whose declaration came from one source module
/// (ARCHITECTURE.md §5.1), so functions that call each other land in one
/// `.text` section next to each other. Emission order within a unit is the
/// middle end's function order, which is the monomorphization worklist's —
/// deterministic, and derived from the reachability walk out of the entry point
/// rather than from a hash order, which is a free first approximation of a
/// call-order layout (CODEGEN-LLVM.md §6).
///
/// It passes **no** reference-counting classifier, and cannot: [`classifier`] is
/// built
/// from the `monomorphize::Program` `rc::run` was handed, and this signature
/// has only the lowered one. `emit::Unit::rc_counted` states what stands in and
/// why the substitution is leak-safe rather than unsound.
pub fn emit_lowered(
    program: &ir::Program,
    tables: &Tables,
    opts: &Options<'_>,
    entry: Option<FuncIdx>,
) -> Result<Vec<Emitted>, Diagnostics> {
    emit_selected(program, tables, opts, entry.map(Root::Main), Units::All, None)
}

/// The root a monomorphized program has, in the shape the unit loop needs.
fn root_of(program: &monomorphize::Program) -> Option<Root> {
    Some(match &program.roots {
        monomorphize::ProgramRoots::Main(idx) => Root::Main(*idx),
        monomorphize::ProgramRoots::Tests(tests) => {
            Root::Tests(tests.iter().map(|t| t.func).collect())
        }
    })
}

/// Which root this program has, with the function indices resolved.
///
/// `monomorphize::ProgramRoots` already says there are exactly two cases, and
/// `stencil/mod.rs` names the same two for the same reason: a binary's `main`
/// and a test binary's list of `test` blocks are two different entry points and
/// only one of them exists in any program.
enum Root {
    Main(FuncIdx),
    Tests(Vec<FuncIdx>),
}

/// Whether `unit` is the one the entry point belongs in.
fn owns(program: &ir::Program, root: &Root, unit: u32) -> bool {
    let here = |f: FuncIdx| {
        program.funcs.get(f.index()).is_some_and(|x| x.unit == unit && x.code().is_some())
    };
    match root {
        Root::Main(e) => here(*e),
        Root::Tests(tests) => tests.first().copied().is_some_and(here),
    }
}

/// The classifier `middle::rc` decided its own operations with, rebuilt over the
/// same program it ran on so that the reference operations this backend adds
/// around the calls it *invents* — the loops of `emit::Unit::list_closure` —
/// are the ones rc would have added (`emit::Unit::rc_counted`).
///
/// One for the whole emission rather than one per unit: it memoises, and
/// building it is a walk of every body.
fn classifier(program: &monomorphize::Program) -> Rc<RefCell<rc::Syntactic>> {
    Rc::new(RefCell::new(rc::Syntactic::new(program)))
}

/// [`emit_lowered`], for a chosen subset of the units.
///
/// The objects it returns are the ones asked for, in unit order, and each is
/// byte-identical to the one a whole-program emission would have produced for
/// it: a unit's module is built from the program and from that unit's members,
/// and nothing in the loop carries state from one iteration to the next.
fn emit_selected(
    program: &ir::Program,
    tables: &Tables,
    opts: &Options<'_>,
    root: Option<Root>,
    units: Units<'_>,
    counted: Option<&Rc<RefCell<rc::Syntactic>>>,
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
    // Both of these are functions of the whole program and of nothing the loop
    // varies, so they are taken once. Computing them per unit is what
    // `design/PERFORMANCE.md` §6.4's first finding measured on the native side:
    // work proportional to units × program, in a program that grows by adding
    // units.
    let by_unit = program.funcs_by_unit();
    let observed = emit::observe(program, opts.profile);
    let cycles = std::sync::Arc::new(layout::Cycles::new(tables));
    let no_members: Vec<usize> = Vec::new();

    let mut out = Vec::with_capacity(program.units.len());
    for (index, unit_name) in program.units.iter().enumerate() {
        let unit = index as u32;
        if !units.wants(unit) {
            continue;
        }
        // This unit's functions, ascending — the same list, in the same order,
        // that a filter over the whole program yielded.
        let all = by_unit.get(index).unwrap_or(&no_members);
        let members: Vec<usize> = all
            .iter()
            .copied()
            .filter(|i| program.funcs.get(*i).is_some_and(|f| f.code().is_some()))
            .collect();
        // The entry point goes in the unit that owns `main`, so a program is
        // one `_start`-adjacent symbol and the other units are libraries. A test
        // binary has no `main` to own it, so it goes in the unit that owns the
        // *first* test — the same rule, applied to the root that exists.
        let owns_entry = root.as_ref().is_some_and(|r| owns(program, r, unit));
        // One object per selected unit, including a unit with nothing in it:
        // `actions::objects_of` pairs this vector with `unit_hashes`, which has
        // a row per unit unconditionally, and a unit that was asked for and not
        // returned is reported there as "the backend emitted no object for unit
        // `x`". The debug backend's loop is over the same list for the same
        // reason.
        let ctx = Context::create();
        let module_name = format!("{}{unit_name}", opts.unit_prefix);
        let mut emitter = emit::Unit::new(
            &ctx,
            program,
            tables,
            &module_name,
            opts.profile,
            &observed,
            Rc::clone(&cycles),
        );
        if let Some(counted) = counted {
            emitter.use_rc_classifier(Rc::clone(counted));
        }
        emitter.module.set_triple(&inkwell::targets::TargetTriple::create(&triple));
        emitter.module.set_data_layout(&data_layout);
        for member in &members {
            emitter.define(FuncIdx(*member as u32));
        }
        if owns_entry {
            match &root {
                Some(Root::Main(e)) => {
                    emitter.entry_point(*e);
                    // The carrier door rides with `main`, in the same unit and
                    // for the same reason the stencil backend puts it there:
                    // they are the two ways into this program's Buri code and
                    // they name the same root.
                    emitter.carrier_door(*e, carrier::MAIN_ENTRY);
                }
                Some(Root::Tests(tests)) => {
                    emitter.test_entry_point(tests);
                    for (i, t) in tests.iter().enumerate() {
                        emitter.carrier_door(*t, &carrier::test_entry(i));
                    }
                }
                None => {}
            }
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
        // The verifier checks *our* IR rather than LLVM's, under this
        // repository's rule for a verifier: a developer build pays for the check
        // and a user's build does not.
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
            key: codegen_key(program, all, &identity, &triple, opts),
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
    let observed = emit::observe(program, opts.profile);
    let cycles = std::sync::Arc::new(layout::Cycles::new(tables));
    let mut emitter =
        emit::Unit::new(&ctx, program, tables, &name, opts.profile, &observed, cycles);
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
        emitter.carrier_door(e, carrier::MAIN_ENTRY);
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

/// `codegen_key(unit) = H(backend, identity, triple, profile, prefix, the unit's IR)`.
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
/// `members` is the unit's function indices in ascending order, which is the
/// order the whole-program filter this used to run yielded them in.
///
/// `unit_prefix` is in it because this backend puts it in the module name, and
/// on every ELF target LLVM emits the module's source-file name as a `.file`
/// directive — an `STT_FILE` symbol in the object. The measurement and the
/// consequence are in `actions::codegen_key`'s note; the term is here so that
/// neither half of the key can be sound while the other is not.
fn codegen_key(
    program: &ir::Program,
    members: &[usize],
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
    text.push_str(opts.unit_prefix);
    text.push('\n');
    for f in members.iter().filter_map(|i| program.funcs.get(*i)) {
        text.push_str(&program.render_func(f));
    }
    ActionKey::of(hash_bytes(text.as_bytes()).as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The identity names the library rather than the feature flag, so a
    /// toolchain built against a different LLVM produces a different key.
    ///
    /// And it names every triple this toolchain renders, so that a rename of
    /// one — `-gnu` to `-musl` was the one that happened — moves the key even
    /// though the platform and the arch beside it in
    /// `actions::codegen_key` did not. The Linux assertion is the case that
    /// motivated the term: a cache written before the rename must not serve a
    /// musl toolchain.
    #[test]
    fn the_identity_names_the_linked_llvm_and_its_triples() {
        let id = Llvm.identity();
        assert!(id.starts_with("llvm 21."), "{id}");
        assert!(id.contains("inkwell"), "{id}");
        for triple in [
            "aarch64-apple-darwin",
            "x86_64-apple-darwin",
            "aarch64-unknown-linux-musl",
            "x86_64-unknown-linux-musl",
        ] {
            assert!(id.contains(triple), "the identity does not name {triple}: {id}");
        }
        assert!(!id.contains("linux-gnu"), "a glibc triple is in the identity: {id}");
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
            Ok("aarch64-unknown-linux-musl")
        );
        assert_eq!(
            t(Platform::Linux, Some(Arch::X86_64)).as_deref(),
            Ok("x86_64-unknown-linux-musl")
        );
        assert!(t(Platform::Js, None).is_err());
    }
}
