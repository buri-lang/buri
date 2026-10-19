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
//!   mod.rs        this file: the `Backend` impl, the unit loop
//!   repr.rs       the value model in LLVM types (VALUE-MODEL.md §5.1)
//!   attrs.rs      the attribute discipline (CODEGEN-LLVM.md §3)
//!   emit.rs       one module: blocks, phis, instructions (§2)
//!   target.rs     the triple, the machine, `default<O2>` (§4)
//! ```
//!
//! # Where this backend sits in the pipeline, and the one seam it cannot close
//!
//! [`Backend::emit`] is handed the **layer-A** program — `monomorphize::Program`
//! after `middle::run` — and takes it by shared reference. `middle::native`
//! (`derives`, `closures`, `rc`) needs `&mut`, so this backend cannot run it,
//! and lowering a tree that has not been through it meets structural
//! operations and unlifted lambdas. A caller that has not run `middle::native`
//! therefore gets a diagnostic naming the pass rather than an object file that
//! is quietly wrong. `actions::prepare` is where the
//! composition lives — `middle::run`, then `middle::native` on a native
//! target — and [`Backend::adopt_lowering`] hands this backend the
//! `ir::Program` the build already lowered from it.
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
mod build;
pub mod emit;
pub mod repr;
pub mod route;
pub mod target;

use inkwell::context::Context;
use std::cell::RefCell;
use std::rc::Rc;

use crate::build::buildfile::{Arch, Platform};
use crate::compiler::backend::task_thread;
use crate::compiler::backend::{
    no_implementation, triple_text, Backend, Emitted, Native, Options, Target, Units,
};
use crate::compiler::middle::{ir, layout, lower, monomorphize, rc};
use crate::compiler::semantics::layered::Dense;
use crate::compiler::semantics::types::{FuncIdx, Tables};
use crate::diagnostics::{Diagnostic, Diagnostics, Invariant, Span};

/// The release backend.
///
/// A plain owned object: an LLVM `Context` is not `Sync` and owns everything
/// built inside it, so one is created per unit and dropped
/// with the modules it produced. Holding a `Context` in this struct would tie
/// the backend's lifetime to a context's and make `Backend` object-unsafe for
/// the one implementor that most wants a plain object.
///
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
    /// same string under both spellings, and `build::actions::codegen_key`
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
    /// states the platform and the arch beside it, and those two pick *the*
    /// triple out of the rendering. This backend computes no key of its own:
    /// `Emitted::key` belongs to the build system.
    fn identity(&self) -> String {
        let (major, minor, patch) = inkwell::support::get_llvm_version();
        let mut id = format!("llvm {major}.{minor}.{patch} inkwell 0.10");
        // Derived from the platform list rather than written out beside it, so
        // that a fifth platform cannot leave this naming four. The
        // JavaScript ones render no triple and drop out here, which is the
        // same `None` `select` refuses them by.
        for platform in Platform::ALL {
            for arch in [Arch::Arm64, Arch::X86_64] {
                if let Some(triple) = triple_text(Target {
                    platform,
                    arch: Some(arch),
                }) {
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
    /// [`ENTRIES`](crate::compiler::backend::runtime_table::ENTRIES) row, by an inline sequence, or by a generated body,
    /// and everything else — `json.*`, every `list.*` entry taking a closure,
    /// `checked*` and `saturating*` — is named here rather than discovered as a
    /// link error or, worse, as a wrong answer. So a program that reaches
    /// outside this backend's surface is told so *before* LLVM is started,
    /// which is the whole reason this hook is on the trait.
    ///
    /// The keys the **runtime archive** carries only behind its `net` or
    /// `crypto` feature are not this backend's to name: `buri`'s
    /// `backend::WithRuntime` folds them in, because the archive is `buri`'s. A
    /// toolchain built without one would otherwise meet it as an unresolved
    /// `buri_rt_*` symbol at `cc` time, which is the one failure mode this hook
    /// exists to replace.
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
        let lowered = lower::run(program, tables);
        let routes = resolved(self, &lowered)?;
        self.emit_units(program, tables, opts, &lowered, &routes, Units::All)
    }

    fn forks_read_shared_mask(&self) -> bool {
        true
    }
}

impl Native for Llvm {
    type Routes<'p> = route::Routes<'p>;

    fn resolve<'p>(&self, lowered: &'p ir::Program) -> Result<route::Routes<'p>, Vec<String>> {
        route::Routes::of(lowered)
    }

    /// The unit loop is already per unit; what `units` adds is the parameter
    /// that lets the build system say which of them it still needs. Everything
    /// above the loop — the triple, the machine — is whole-program and is done
    /// once either way.
    fn emit_units(
        &mut self,
        program: &monomorphize::Program,
        tables: &Tables,
        opts: &Options<'_>,
        lowered: &ir::Program,
        routes: &route::Routes<'_>,
        units: Units<'_>,
    ) -> Result<Vec<Emitted>, Diagnostics> {
        let counted = classifier(program);
        let root = root_of(lowered);
        emit_selected(
            lowered,
            routes,
            tables,
            opts,
            root,
            units,
            &counted,
            For::linker(lowered),
            &target::object,
        )
    }
}

/// `lowered`'s routes, or the refusal naming every key this backend has none
/// for.
fn resolved<'p>(llvm: &Llvm, lowered: &'p ir::Program) -> Result<route::Routes<'p>, Diagnostics> {
    llvm.resolve(lowered).map_err(|keys| {
        let mut diags = Diagnostics::new();
        diags.push(no_implementation(llvm.name(), &keys));
        diags
    })
}

impl Llvm {
    /// The optimized IR of one unit, as text. What a FileCheck-style assertion
    /// reads.
    ///
    /// It runs the same emitter and the same pipeline as
    /// [`Backend::emit_units`] and stops one step earlier, so an assertion about
    /// the IR is an assertion about the object rather than about a second code
    /// path that resembles it.
    pub fn emit_ir_text(
        &mut self,
        program: &monomorphize::Program,
        tables: &Tables,
        opts: &Options<'_>,
        unit: u32,
    ) -> Result<String, Diagnostics> {
        let lowered = lower::run(program, tables);
        let routes = resolved(self, &lowered)?;
        let counted = classifier(program);
        let text = |module: &inkwell::module::Module<'_>, _: &inkwell::targets::TargetMachine| {
            Ok(module.to_string().into_bytes())
        };
        let only = [unit as usize];
        let emitted = emit_selected(
            &lowered,
            &routes,
            tables,
            opts,
            root_of(&lowered),
            Units::Only(&only),
            &counted,
            For::reader(&lowered),
            &text,
        )?;
        let bytes = emitted
            .into_iter()
            .next()
            .map(|e| e.bytes)
            .unwrap_or_default();
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    }
}

/// The root a lowered program has, in the shape the unit loop needs.
fn root_of(lowered: &ir::Program) -> Option<Root> {
    Some(match &lowered.roots {
        ir::Roots::Main(main) => Root::Main(*main),
        ir::Roots::Tests(tests) => Root::Tests(tests.clone()),
    })
}

/// Which root this program has, with the function indices resolved.
///
/// `ir::Roots` already says there are exactly two cases, and
/// `stencil/mod.rs` names the same two for the same reason: a binary's `main`
/// and a test binary's list of `test` blocks are two different entry points and
/// only one of them exists in any program.
enum Root {
    Main(ir::Main),
    Tests(Vec<ir::Root>),
}

/// Whether `unit` is the one the entry point belongs in.
fn owns(program: &ir::Program, root: &Root, unit: u32) -> bool {
    let here = |f: FuncIdx| {
        let x = program.funcs.get(f);
        x.unit == unit && x.code().is_some()
    };
    match root {
        Root::Main(e) => here(e.root.func()),
        Root::Tests(tests) => tests.first().is_some_and(|t| here(t.func())),
    }
}

/// The classifier `middle::rc` decided its own operations with, rebuilt over the
/// same program it ran on so that the reference operations this backend adds
/// around the calls it *invents* — a runtime-driven step, a thunk — are the
/// ones rc would have added (`emit::Unit::rc_counted`).
///
/// Built once for the whole emission, because building it is a walk of every
/// body, and copied into each thread that emits units, because it memoises.
fn classifier(program: &monomorphize::Program) -> rc::Syntactic {
    rc::Syntactic::new(program)
}

/// Who a unit is for: a reader of [`Llvm::emit_ir_text`], or the linker.
///
/// An object leaves out what only a reader would look at:
///
/// - **Value and block names**, as `clang` does outside a debug build. Every
///   name is a string LLVM uniques into a symbol table, and every pass that
///   makes an instruction names it too.
/// - **Functions nothing will call.** One nothing names isn't emitted, and one
///   no other unit names is dropped once `opt` leaves no use of it
///   (`Unit::prune`). The linker would strip both.
///
/// The functions left are optimized exactly as the reader sees them.
struct For {
    /// Whether values and blocks keep their names.
    names: bool,
    /// Whether another unit or the entry point names each function, so that
    /// its unit keeps it (`Unit::prune`).
    kept: Dense<FuncIdx, bool>,
    /// Whether anything names each function. One nothing names is not
    /// emitted at all.
    named: Dense<FuncIdx, bool>,
}

impl For {
    fn linker(program: &ir::Program) -> For {
        For {
            names: false,
            kept: crate::compiler::backend::linkage::named_elsewhere(program),
            named: crate::compiler::backend::linkage::named_anywhere(program),
        }
    }

    /// A reader sees every function, by name.
    fn reader(program: &ir::Program) -> For {
        let every = program.funcs.map(|_| true);
        For {
            names: true,
            kept: every.clone(),
            named: every,
        }
    }
}

/// What one thread emitting units keeps between them: the machine, which
/// LLVM does not share between threads, and the thread's own copy of the
/// classifier.
struct Worker {
    machine: inkwell::targets::TargetMachine,
    data_layout: inkwell::data_layout::DataLayout,
    counted: Rc<RefCell<rc::Syntactic>>,
}

/// What every unit's emission reads and none of them changes.
struct Shared<'p> {
    program: &'p ir::Program,
    routes: &'p route::Routes<'p>,
    tables: &'p Tables,
    opts: &'p Options<'p>,
    root: Option<Root>,
    triple: String,
    cycles: std::sync::Arc<layout::Cycles>,
    observed: Dense<FuncIdx, attrs::Observed>,
    /// `runtime_table::shares_counts`: whether every count's fork reads
    /// `buri_rt_shared_mask`.
    shares: bool,
    /// Who the units are for, with the roots kept and named.
    audience: For,
}

/// One object per codegen unit, for a chosen subset of the units, from an
/// already-lowered program.
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
/// The objects it returns are the ones asked for, in unit order, and each is
/// byte-identical to the one a whole-program emission would have produced for
/// it: a unit's module is built from the program and from that unit's members,
/// and nothing carries state from one unit to the next.
///
/// **The units are emitted side by side**, one per core, because they are
/// independent: each has its own LLVM context, and every thread has its own
/// machine. A test batch's program is hundreds of units, and one at a time it
/// was one core running for many minutes while the rest of the machine waited.
/// The largest units start first, so that the last one to finish is a small
/// one; the order the objects are returned in does not change.
///
/// `render` turns each optimized module into the bytes returned for it: an
/// object file for the build, the IR text for [`Llvm::emit_ir_text`].
#[allow(
    clippy::too_many_arguments,
    reason = "the whole-program answers every unit reads"
)]
fn emit_selected(
    program: &ir::Program,
    routes: &route::Routes<'_>,
    tables: &Tables,
    opts: &Options<'_>,
    root: Option<Root>,
    units: Units<'_>,
    counted: &rc::Syntactic,
    audience: For,
    render: Render<'_>,
) -> Result<Vec<Emitted>, Diagnostics> {
    let triple = match target::triple(opts.target) {
        Ok(t) => t,
        Err(message) => {
            let mut diags = Diagnostics::new();
            diags.push(Diagnostic::error(Span::NONE, message));
            return Err(diags);
        }
    };
    // Asked once here, so that a toolchain whose LLVM cannot make a machine
    // gets one diagnostic rather than one per thread. Each thread then makes
    // its own.
    machine_for(&triple, opts)?;

    // These are functions of the whole program and of nothing a unit varies,
    // so they are taken once. Computing them per unit is what
    // `design/PERFORMANCE.md` §6.4's first finding measured on the native side:
    // work proportional to units × program, in a program that grows by adding
    // units.
    let cycles = std::sync::Arc::new(layout::Cycles::new(tables));
    let observed = {
        let layouts = layout::Layouts::with_cycles(tables, cycles.clone());
        emit::observe(program, &emit::Boxes::new(program, tables, &layouts))
    };
    let roots: Vec<FuncIdx> = match &root {
        Some(Root::Main(e)) => vec![e.root.func()],
        Some(Root::Tests(tests)) => tests.iter().map(|t| t.func()).collect(),
        None => Vec::new(),
    };
    let with_roots = |mut named: Dense<FuncIdx, bool>| {
        for r in &roots {
            *named.get_mut(*r) = true;
        }
        named
    };
    let audience = For {
        names: audience.names,
        kept: with_roots(audience.kept),
        named: with_roots(audience.named),
    };
    let shared = Shared {
        program,
        routes,
        tables,
        opts,
        root,
        triple,
        cycles,
        observed,
        shares: crate::compiler::backend::runtime_table::shares_counts(program),
        audience,
    };

    let mut wanted: Vec<(usize, &ir::Unit)> = program
        .units
        .iter()
        .enumerate()
        .filter(|(i, _)| units.wants(*i))
        .collect();
    // Largest first, by member count, with the unit index breaking ties so that
    // the order is a function of the program alone.
    wanted.sort_by_key(|(i, u)| (std::cmp::Reverse(u.funcs.len()), *i));
    let emitted = crate::parallel::map_items_with(
        &wanted,
        || None,
        |worker: &mut Option<Worker>, _, &(index, unit)| {
            let worker = match worker {
                Some(w) => w,
                None => {
                    let machine = machine_for(&shared.triple, opts)?;
                    let data_layout = machine.get_target_data().get_data_layout();
                    let counted = Rc::new(RefCell::new(counted.clone()));
                    worker.insert(Worker {
                        machine,
                        data_layout,
                        counted,
                    })
                }
            };
            emit_unit(&shared, worker, index, unit, render)
        },
    );

    // In unit order, so a failure is the first failing unit's: the one a loop
    // over the units would have stopped at.
    let mut out: Vec<(usize, Result<Emitted, Diagnostics>)> =
        wanted.iter().map(|(i, _)| *i).zip(emitted).collect();
    out.sort_by_key(|(index, _)| *index);
    out.into_iter().map(|(_, e)| e).collect()
}

/// The machine to optimize and emit against, or the diagnostic for a toolchain
/// whose LLVM cannot make one.
fn machine_for(
    triple: &str,
    opts: &Options<'_>,
) -> Result<inkwell::targets::TargetMachine, Diagnostics> {
    target::machine(triple, opts.profile).map_err(|message| {
        let mut diags = Diagnostics::new();
        diags
            .push(Diagnostic::error(Span::NONE, message).with_fix(
                "install LLVM 21 and rebuild the toolchain, or build with `--output=js`",
            ));
        diags
    })
}

/// What turns an optimized module into the bytes returned for it. Dynamic, so
/// the unit loop is one function whichever renders.
type Render<'r> = &'r (dyn Fn(
    &inkwell::module::Module<'_>,
    &inkwell::targets::TargetMachine,
) -> Result<Vec<u8>, String>
         + Sync);

/// One unit's object: its module emitted, verified, optimized and rendered.
fn emit_unit(
    shared: &Shared<'_>,
    worker: &Worker,
    index: usize,
    this: &ir::Unit,
    render: Render<'_>,
) -> Result<Emitted, Diagnostics> {
    let program = shared.program;
    let opts = shared.opts;
    let mut diags = Diagnostics::new();
    let unit = index as u32;
    let unit_name = this.name.as_str();
    let audience = &shared.audience;
    // This unit's functions, ascending — the same list, in the same order,
    // that a filter over the whole program yielded. An object leaves out the
    // ones nothing names (`For`).
    let members: Vec<(FuncIdx, &ir::Code)> = this
        .funcs
        .iter()
        .filter(|&&i| *audience.named.get(i))
        .filter_map(|&i| Some((i, program.funcs.get(i).code()?)))
        .collect();
    // The entry point goes in the unit that owns `main`, so a program is
    // one `_start`-adjacent symbol and the other units are libraries. A test
    // binary has no `main` to own it, so it goes in the unit that owns the
    // *first* test — the same rule, applied to the root that exists.
    let root = shared.root.as_ref();
    let owns_entry = root.is_some_and(|r| owns(program, r, unit));
    // One object per selected unit, including a unit with nothing in it:
    // `actions::objects_of` pairs the objects with `unit_hashes`, which has
    // a row per unit unconditionally, and a unit that was asked for and not
    // returned is reported there as "the backend emitted no object for unit
    // `x`". The debug backend's loop is over the same list for the same
    // reason.
    let ctx = Context::create();
    build::keep_names(&ctx, audience.names);
    let builder = ctx.create_builder();
    let module_name = format!("{}{unit_name}", opts.unit_prefix);
    let mut emitter = emit::Unit::new(
        &ctx,
        &builder,
        program,
        shared.routes,
        shared.tables,
        &module_name,
        &shared.observed,
        shared.shares,
        std::sync::Arc::clone(&shared.cycles),
        Rc::clone(&worker.counted),
    );
    emitter
        .module
        .set_triple(&inkwell::targets::TargetTriple::create(&shared.triple));
    emitter.module.set_data_layout(&worker.data_layout);
    for &(member, code) in &members {
        emitter.define(member, code);
    }
    if owns_entry {
        match root {
            Some(Root::Main(e)) => {
                emitter.entry_point(*e);
                // The thread door rides with `main`, in the same unit and
                // for the same reason the stencil backend puts it there:
                // they are the two ways into this program's Buri code and
                // they name the same root.
                emitter.thread_door(e.root, task_thread::MAIN_ENTRY);
            }
            Some(Root::Tests(tests)) => {
                emitter.test_entry_point(tests);
                for (i, t) in tests.iter().enumerate() {
                    emitter.thread_door(*t, &task_thread::test_entry(i));
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
    attrs::unwind_tables(&emitter.module);
    optimize(&emitter.module, &worker.machine, opts.profile);
    emitter.prune(members.iter().map(|&(f, _)| f), &audience.kept);
    let bytes = match render(&emitter.module, &worker.machine) {
        Ok(b) => b,
        Err(message) => {
            diags.push(Diagnostic::error(Span::NONE, message));
            return Err(diags);
        }
    };
    Ok(Emitted {
        name: format!("{unit_name}.o"),
        key: None,
        bytes,
    })
}

/// One unit's module, verified in a developer build and then optimized.
///
/// The verifier checks *our* IR rather than LLVM's, under this repository's
/// rule for a verifier: a developer build pays for the check and a user's
/// build does not. A user's build still gets `finish`'s check that every block
/// ends in a terminator. It runs once, here, rather than after every pass
/// (`target::optimize`).
fn optimize(
    module: &inkwell::module::Module<'_>,
    machine: &inkwell::targets::TargetMachine,
    profile: crate::compiler::backend::Profile,
) {
    // `optimize` fails only on a pipeline string it was not built for, and
    // both strings are constants, so the one check covers the pair.
    let verified = if cfg!(debug_assertions) {
        module.verify().map_err(|e| e.to_string())
    } else {
        Ok(())
    };
    verified
        .and_then(|()| target::optimize(module, machine, profile))
        .or_ice("the LLVM backend emits valid IR, and LLVM runs its own pipeline");
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
            assert!(
                id.contains(triple),
                "the identity does not name {triple}: {id}"
            );
        }
        assert!(
            !id.contains("linux-gnu"),
            "a glibc triple is in the identity: {id}"
        );
    }

    /// Four targets, and the one that is not this backend's.
    #[test]
    fn the_triples_are_the_four_supported_targets() {
        use crate::build::buildfile::{Arch, Platform};
        use crate::compiler::backend::Target;
        let t = |platform, arch| target::triple(Target { platform, arch });
        assert_eq!(
            t(Platform::Macos, Some(Arch::Arm64)).as_deref(),
            Ok("aarch64-apple-darwin")
        );
        assert_eq!(
            t(Platform::Macos, Some(Arch::X86_64)).as_deref(),
            Ok("x86_64-apple-darwin")
        );
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
