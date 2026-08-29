//! The Cranelift backend.
//!
//! The dev backend: `buri build`, `buri run`, and every `buri test` on a native
//! platform. Chosen for `(Linux | Macos, Debug)` because compile time is the
//! only thing that matters in that quadrant.
//!
//! Behind the `backend-cranelift` feature, which is **on by default**:
//! Cranelift is pure Rust with no system dependency and cross-compiles anywhere
//! Rust does, so having it on costs a contributor nothing but compile time, and
//! having it off would mean the default toolchain cannot build a native
//! artifact at all.
//!
//! Design: `design/native/CODEGEN-CRANELIFT.md`, `BUILD-AND-WATCH.md` §2.
//!
//! ```text
//! cranelift/
//!   mod.rs      this file: settings, the ISA, one module per unit, the entry point
//!   abi.rs      the value model at the machine boundary, and where a wide result lives
//!   emit.rs     middle::ir into CLIF
//!   helpers.rs  the eight functions this backend generates for itself
//!   runtime.rs  which intrinsic keys have a `buri_rt_*` symbol, and its shape
//! ```
//!
//! # Where an intrinsic goes, and why
//!
//! Seven routes, tried in this order by `emit::Lower::intrinsic`, and the order
//! is the reasoning: everything that is *instructions* is asked before the
//! arguments are spread for a call that is not going to happen.
//!
//! | Route | What it covers | Why not a runtime call |
//! |---|---|---|
//! | `numeric` | `num.<T>.<op>`, including `Checked`, `Saturating`, `Wrapping` and `Bounded` | One conversion per source-and-target pair (SPEC 6.2.1), and generating two instructions beats calling a function that generates the same two |
//! | `bits` | all fourteen of `core/bits` | Each is one machine instruction behind `$shiftCount`'s range check |
//! | `prim_trait` | `Eq`/`Ord`/`Hash`/`Show` at `Bool` and `Char`, `str.show`, `Char::toU32` | A compare, a select, or the identity |
//! | `open_coded` | `str.concat`, `str.format`, `str.len`, `list.len`, `list.empty`, the three `core/testing/assert` bodies, the test allocator | A load, a copy, or an allocation and two copies |
//! | `list_closure` | `list.map`, `mapCtx`, `filter`, `filterCtx`, `fold`, `foldCtx`, `any`, `all`, `count` | The step is a closure whose signature is the element type flattened, so calling one from C would mean synthesizing a parameter list per `T` (`cli/runtime/list.rs`'s header) |
//! | `derived` | `derivePrimShow.<T>`, `derivePrimHash.<T>` | Dispatches on a type, then calls |
//! | *table* | everything in `runtime.rs`'s `ENTRIES` | The work is in `cli/runtime`, shared with the LLVM backend so the two cannot answer differently |
//!
//! Nothing falls through the table: a key it does not hold is a diagnostic
//! naming the operation, never a link error naming a symbol.
//!
//! # What `emit` is handed, and what it does with it
//!
//! [`Backend::emit`] takes the *layer A* program — `monomorphize::Program`,
//! after `middle::run` and `middle::native` — and lowers it here, by calling
//! `middle::lower::run` itself. The trait cannot hand over an `ir::Program`
//! because the JavaScript backend does not have one and `Backend` is what they
//! share; and the lowering is cheap, deterministic and takes the program by
//! reference, so doing it per backend costs nothing a shared one would save.
//!
//! One `ObjectModule` per codegen unit (§6), which is the shape
//! `rustc_codegen_cranelift` uses. `per_function_section(true)` puts each
//! function in its own `.text.<name>` so `--gc-sections` can drop what nothing
//! reaches — and it is the reason one object *per function* is not the answer:
//! that would make every intra-unit call an `Import`, which is non-colocated,
//! which turns a direct `call rel32` into a GOT-indirect call.
//!
//! # What is not here yet, named rather than left to be discovered
//!
//!  * **`.buri_symbols`** (§5). Frame-pointer backtraces need a
//!    `(address, name)` table the compiler emits and `buri_rt_abort` walks. The
//!    frame pointers are preserved — that half is done, and it is the half that
//!    has to be decided at codegen time — but the section and the runtime's
//!    walk are one wave's work together, and half of it is worse than none.
//!  * **The `Result`-returning host capabilities**: `readFile`, `writeFile`,
//!    `readDir`, `readLine`, `readBytes`, `variable`, `arguments`, `fetch`. The
//!    *shape* is solved — `runtime::Ret::Opt` turns a discriminant and an
//!    out-pointer into whatever `middle::layout` chose for an `Option`, and
//!    `str.toInt` and `list.get` go through it — but a `Result<T, IoError>` has
//!    an error *payload* to build as well as a tag, and `IoError` is a
//!    `core/effect` type the intrinsic table does not name. One `Ret::Result` and
//!    that type's layout away.
//!  * **The rest of the closure surface of `core/list`.** Ten of them are
//!    emitted now (`emit::Lower::list_closure`), which is what moved
//!    `canary/canary.buri` into `cli/tests/native/conformance.rs`'s native set
//!    and what makes the realistic half of `design/PERFORMANCE.md`'s native
//!    rows measurable at all. Nine are loops and the tenth, `sortBy`, is a
//!    stable bottom-up merge (`emit::Lower::list_sort`). What is left is the
//!    entries that are not one walk of one block: `find` and `findIndex`,
//!    which build an `Option` around the answer; `foldResult` and
//!    `foldResultCtx`, which build a `Result` and leave early; and `zip` and
//!    `flatten`, which need a second element layout
//!    (`cli/runtime/list.rs`'s header). `core/map` is held out by `find` and
//!    `flatten`, and by nothing else.
//!  * **`json.*`**, and with it `deriveArrayCompare`, `deriveArrayShow`,
//!    `deriveArrayJson` and `deriveArrayHash`, which are the same loop over a
//!    code pointer that the closure surface needs. `deriveArrayEq` is emitted
//!    (`emit::Lower::derive_array`); none of the five is reported by
//!    [`Cranelift::missing_intrinsics`], because a `deriveArray*` is an
//!    intrinsic *expression* inside a body `middle::derives` generated rather
//!    than a function the hook can see.
//!  * **`core/math`'s thirteen transcendentals** and **`core/char`'s eight
//!    classifiers**. Both are gaps for the same *kind* of reason and it is not
//!    effort: IEEE 754 does not fix `sin`, and `\p{L}` is a General_Category
//!    table Rust does not expose, so implementing either from what is to hand
//!    would put a divergence into the toolchain where a named gap is honest.
//!    `cli/runtime/math.rs` states it at length.
//!  * **`MemFs`'s four methods**, which is all that is left of
//!    `core/testing/context`. The handle table the stateful half wanted is
//!    `cli/runtime/testing.rs` now, and `captureOut`, `captureErr`,
//!    `TestClock`, `TestEnv`, `TestStdin` and `TestRand` are all in the
//!    archive; that file's header says why `readFile`, `writeFile`, `readDir`
//!    and `fileExists` stay out and why holding `fileExists` back with the
//!    other three is the honest choice rather than the cautious one.
//!  * **An inexact `to<T>`** (SPEC 6.2.1's `Result<T, _>` shape), for the same
//!    reason as the host capabilities: the error payload is a type the table
//!    does not name.
//!
//! Every one of them is reported by [`Cranelift::missing_intrinsics`], so a
//! program that uses one is told before a second is spent on it rather than
//! after — which is what asking the question up front was for.

pub mod abi;
pub mod emit;
pub mod helpers;
pub mod runtime;

use cranelift_codegen::isa::{self, TargetIsa};
use cranelift_codegen::ir::{types, AbiParam, InstBuilder, Signature};
use cranelift_codegen::settings::{self, Configurable};
use cranelift_module::{default_libcall_names, FuncId, Linkage, Module};
use cranelift_object::{ObjectBuilder, ObjectModule};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use crate::build::cache::ActionKey;
use crate::compiler::backend::cranelift::abi::{Abi, PTR};
use crate::compiler::backend::{Backend, Emitted, Options, Units};
use crate::compiler::middle::ir as mir;
use crate::compiler::middle::layout::{Cycles, EnumRepr, Repr};
use crate::compiler::middle::lower;
use crate::compiler::middle::rc;
use crate::compiler::middle::monomorphize::{Program, ProgramRoots};
use crate::compiler::semantics::types::Tables;
use crate::diagnostics::{Diagnostic, Diagnostics, Span};

/// The Cranelift backend.
#[derive(Default)]
pub struct Cranelift;

impl Backend for Cranelift {
    fn name(&self) -> &'static str {
        "cranelift"
    }

    /// The Cranelift version, and nothing else.
    ///
    /// It enters every `codegen` cache key (ARCHITECTURE.md §3, §6.2), which
    /// is why the dependency is pinned to an LTS line rather than tracked:
    /// a bump invalidates every cached object in every repository, and at the
    /// monthly cadence that is churn nobody asked for
    /// (CODEGEN-CRANELIFT.md §8).
    fn identity(&self) -> String {
        format!("cranelift {}", cranelift_codegen::VERSION)
    }

    /// Which intrinsics this backend has no body for.
    ///
    /// Asked of the program up front rather than accumulated as a side effect
    /// of a failed emission, which is the whole point of the signature: a
    /// program using an unimplemented intrinsic is told before the backend
    /// spends a second on it.
    fn missing_intrinsics(&self, program: &Program, _tables: &Tables) -> Vec<String> {
        let mut missing: Vec<String> = program
            .funcs
            .iter()
            .filter_map(|f| f.intrinsic_key())
            .filter(|k| !emit::implemented(k))
            .map(String::from)
            .collect();
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
        self.emit_units(program, tables, opts, Units::All)
    }

    /// Every step above the per-unit loop is whole-program and stays so: the
    /// lowering, the reference-counting classifier and the IR verifier all read the
    /// program, and a unit's object depends on the program it was lowered from
    /// rather than on the other units' objects. What `units` skips is
    /// `compile_unit`, which is where the time is.
    fn emit_units(
        &mut self,
        program: &Program,
        tables: &Tables,
        opts: &Options<'_>,
        units: Units<'_>,
    ) -> Result<Vec<Emitted>, Diagnostics> {
        let isa = match isa_for(opts) {
            Ok(isa) => isa,
            Err(message) => return Err(one(message)),
        };
        let lowered = lower::run(program, tables);
        // The classifier `middle::rc` used, rebuilt over the same program it ran
        // on, so the reference operations this backend adds around the calls
        // it invents are the ones rc would have added (`emit::Cx::rc_counted`).
        // One for the whole emission rather than one per unit: it memoises,
        // and building it is a walk of every body.
        let counted = RefCell::new(rc::Syntactic::new(program));
        // The verifier the design asks for is Cranelift's, over *our* output
        // (§4); this one is the middle end's, over its own. Both are on in a
        // toolchain built with assertions and off in a release one, for the
        // same reason: they check the compiler, not the program.
        if cfg!(debug_assertions) {
            let problems = mir::verify(&lowered);
            if !problems.is_empty() {
                return Err(one(format!(
                    "internal error: the lowered IR is malformed: {}",
                    problems.join("; ")
                )));
            }
        }
        let entry = match &program.roots {
            ProgramRoots::Main(idx) => Root::Main(idx.index()),
            ProgramRoots::Tests(tests) => {
                Root::Tests(tests.iter().map(|t| t.func.index()).collect())
            }
        };

        // The partition, once. Everything below reads its own row, so no step
        // in the loop is a function of the whole program's size — which is what
        // `design/PERFORMANCE.md` §6.4's first finding measured going wrong.
        let members = lowered.funcs_by_unit();
        let cycles = Rc::new(Cycles::new(tables));
        let empty: Vec<usize> = Vec::new();

        let mut out = Vec::new();
        let mut errors: Vec<String> = Vec::new();
        for (index, name) in lowered.units.iter().enumerate() {
            let unit = u32::try_from(index).unwrap_or(0);
            if !units.wants(unit) {
                continue;
            }
            let mine = members.get(index).unwrap_or(&empty);
            match compile_unit(&lowered, tables, opts, &isa, unit, name, mine, &cycles, &entry, &counted) {
                Ok(emitted) => out.push(emitted),
                Err(mut e) => errors.append(&mut e),
            }
        }
        if !errors.is_empty() {
            errors.sort();
            errors.dedup();
            let mut diags = Diagnostics::new();
            for e in errors {
                diags.push(Diagnostic::error(Span::NONE, e).with_fix(
                    "report it: a program the front end accepted is one this backend should compile",
                ));
            }
            return Err(diags);
        }
        Ok(out)
    }
}

fn one(message: String) -> Diagnostics {
    let mut diags = Diagnostics::new();
    diags.push(Diagnostic::error(Span::NONE, message));
    diags
}

/// The ISA, with the three settings whose defaults are wrong for this use.
///
/// | Flag | Default | Ours | Why |
/// |---|---|---|---|
/// | `opt_level` | `none` | `none` | Correct by accident: the middle end already optimized, and this is the backend compile time is for. |
/// | `enable_verifier` | `true` | `cfg!(debug_assertions)` | It checks *our* lowering, which is worth its cost with assertions on and is pure overhead without. |
/// | `is_pic` | `false` | `true` | Every artifact is PIE. |
/// | `preserve_frame_pointers` | `false` | `true` | Backtraces come from frame pointers, because there is no DWARF (§5). |
/// | `unwind_info` | `true` | `false` | There is nothing to unwind: an abort is a write and an `_exit` (SPEC 6.10). |
///
/// `regalloc_algorithm` is set and has no effect: the pinned Cranelift's enum
/// has only `backtracking` in it — 0.123 withdrew `single_pass` because it
/// cannot allocate for exception handling — so `set` rejects the value and the
/// line is a statement of intent for the version that has it back. The dial
/// that does move was measured rather than assumed: `opt_level = "speed"`
/// costs `enum-heavy/10k` 20 % (455 ms to 547 ms), because the egraph pass is
/// more than the register allocation it saves.
///
/// # The ISA need not be the host's
///
/// This function used to refuse any target platform that was not the host's,
/// with a message about the runtime archive. The reason was true and the place
/// was wrong: the archive is a **link** input, and that refusal already lives
/// where it belongs — [`crate::build::link::can_link`] checks the host platform
/// and architecture, and [`crate::build::actions::native_ready`] is what
/// `commands/build.rs`, `commands/run.rs`, `commands/test.rs` and `driver.rs`
/// all consult before a native build is attempted. Cross-*codegen* is fine and
/// cross-*linking* is not, so `buri build --output=linux/x86_64` on a mac still
/// fails with exactly the diagnostic it failed with before, from `native_ready`.
///
/// What removing the duplicate buys: `cranelift-codegen` is compiled with
/// `all-arch` (deliberately — see `cli/Cargo.toml`), so an ISA for any
/// supported triple costs nothing, and `cli/benches/compiler.rs` can measure
/// native lowering for both of the triples `design/PERFORMANCE.md` names on
/// whichever machine the suite is run on.
fn isa_for(opts: &Options<'_>) -> Result<Arc<dyn TargetIsa>, String> {
    let triple = triple_of(opts.target)?;

    let mut flags = settings::builder();
    let mut set = |name: &str, value: &str| {
        // A setting the pinned Cranelift does not have is a version skew, not
        // a program error: the build stays correct without it.
        let _ = flags.set(name, value);
    };
    set("opt_level", "none");
    set("enable_verifier", if cfg!(debug_assertions) { "true" } else { "false" });
    set("is_pic", "true");
    set("preserve_frame_pointers", "true");
    set("unwind_info", "false");
    set("regalloc_algorithm", "single_pass");
    // The x64 backend refuses `i128` arguments and returns without this.
    set("enable_llvm_abi_extensions", "true");
    let flags = settings::Flags::new(flags);

    // The host gets `cranelift_native`, which infers the running CPU's
    // features; any other triple gets the baseline ISA `all-arch` already
    // carries. That asymmetry is deliberate and it is the right way round: the
    // host build is the one that gets to use the machine it is running on, and
    // a cross build is the one that has to be reproducible on any machine.
    let builder = if triple == target_lexicon::Triple::host() {
        cranelift_native::builder().map_err(|e| format!("no ISA for this host: {e}"))?
    } else {
        isa::lookup(triple.clone())
            .map_err(|e| format!("no Cranelift backend for `{triple}`: {e}"))?
    };
    builder.finish(flags).map_err(|e| format!("cannot build an ISA for `{triple}`: {e}"))
}

/// The triple a [`Target`](crate::compiler::backend::Target) names, parsed.
///
/// The text comes from [`crate::compiler::backend::triple_text`], so this
/// backend and LLVM's cannot disagree about what an unqualified
/// `--output=linux` means.
fn triple_of(target: crate::compiler::backend::Target) -> Result<target_lexicon::Triple, String> {
    let Some(text) = crate::compiler::backend::triple_text(target) else {
        return Err(String::from("the Cranelift backend does not target JavaScript"));
    };
    text.parse().map_err(|e| format!("`{text}` is not a target triple: {e}"))
}

/// One codegen unit, from IR to object bytes.
#[allow(clippy::too_many_arguments, reason = "one unit's inputs, each of which it needs")]
fn compile_unit(
    program: &mir::Program,
    tables: &Tables,
    opts: &Options<'_>,
    isa: &Arc<dyn TargetIsa>,
    unit: u32,
    name: &str,
    members: &[usize],
    cycles: &Rc<Cycles>,
    entry: &Root,
    counted: &RefCell<rc::Syntactic>,
) -> Result<Emitted, Vec<String>> {
    let mut builder = match ObjectBuilder::new(isa.clone(), name, default_libcall_names()) {
        Ok(b) => b,
        Err(e) => return Err(vec![format!("cannot build an object for `{name}`: {e}")]),
    };
    // Fine-grained dead-code elimination at link time, without the codegen
    // regression a per-function *object* would cause (§6).
    builder.per_function_section(true);
    builder.per_data_object_section(true);
    let module = ObjectModule::new(builder);
    let abi = Abi::new(tables, isa.default_call_conv(), Rc::clone(cycles));
    let mut u = emit::Unit::new(module, abi, program, opts.profile, unit, members, counted);
    u.define_all();

    // The entry point goes in the unit that owns `main`, so that a program is
    // one `_start`-adjacent symbol and the other units are libraries. A test
    // binary has no `main` to own it, so it goes in the unit that owns the
    // *first* test — same rule, applied to the root that exists.
    match entry {
        Root::Main(idx) if u.owned.contains(idx) => entry_point(&mut u, *idx),
        Root::Tests(tests) => {
            if tests.first().is_some_and(|i| u.owned.contains(i)) {
                test_entry_point(&mut u, tests);
            }
        }
        Root::Main(_) => {}
    }

    if !u.errors.is_empty() {
        return Err(u.errors);
    }
    let product = u.module.finish();
    let bytes = match product.emit() {
        Ok(b) => b,
        Err(e) => return Err(vec![format!("cannot write the object for `{name}`: {e}")]),
    };
    // The `codegen` key is `H(the unit's lowered IR)` (ARCHITECTURE.md §6.2),
    // and `ir::Program`'s `Display` is a faithful, total and deterministic
    // function of the IR with no hash order in it — so hashing the text is a
    // correct way to compute it, and the one that can be *read* when a key
    // changes and nobody knows why.
    //
    // `members` is this unit's functions in ascending index — the same
    // functions in the same order the whole-program filter yielded
    // (`ir::Program::funcs_by_unit`), so the bytes hashed here are unchanged
    // and no cached object is invalidated.
    let mut text = String::new();
    for f in members.iter().filter_map(|i| program.funcs.get(*i)) {
        text.push_str(&program.render_func(f));
    }
    Ok(Emitted { name: format!("{name}.o"), key: ActionKey::of(text.as_bytes()), bytes })
}

/// Which root this program has, in the shape `compile_unit` needs.
///
/// `monomorphize::ProgramRoots` already says there are exactly two cases; this
/// is the same statement with the function indices resolved, so that the unit
/// loop does not carry a `&Program` it otherwise would not need.
enum Root {
    Main(usize),
    Tests(Vec<usize>),
}

/// The `main` of a **test binary**: every `test` block, in order, each behind
/// the runner's answer about whether this process is to run it.
///
/// A failed assertion is still an abort — SPEC 6.10 says this language has
/// nothing to catch — so one process reports at most one failure, and the
/// report `buri test` prints is assembled across as many processes as the suite
/// has failures: `buri_rt_test_enter(i)` answers 0 for a block before the one
/// the runner asked this process to start at, so a re-run skips what is already
/// reported without re-running it. `cli/runtime/testing.rs` states the protocol
/// and `commands/test.rs::run_native` drives it.
///
/// Sharding a suite across processes is allowed by construction: `commands/
/// test.rs`'s header is that a suite's result may not depend on the order its
/// blocks run in, and there is no mutable global state for one to leave behind
/// for the next.
///
/// A `test` body answers `()`, so there is nothing to inspect between calls;
/// which block a process was in when it aborted is what the runner needs, and
/// `enter` is where it learns it.
fn test_entry_point(u: &mut emit::Unit<'_>, tests: &[usize]) {
    let mut sig = Signature::new(u.abi.call_conv);
    sig.params.push(AbiParam::new(types::I32));
    sig.params.push(AbiParam::new(PTR));
    sig.returns.push(AbiParam::new(types::I32));
    let id: FuncId = match u.module.declare_function("main", Linkage::Export, &sig) {
        Ok(id) => id,
        Err(e) => {
            u.errors.push(format!("cannot declare the test entry point: {e}"));
            return;
        }
    };
    let tests = tests.to_vec();
    u.build_function(id, sig, "the test entry point", |unit, builder| {
        let mut cx = emit::Cx::new(unit, builder);
        let block = cx.builder.create_block();
        cx.builder.append_block_params_for_function_params(block);
        cx.builder.switch_to_block(block);
        let params = cx.builder.block_params(block).to_vec();
        let (Some(argc), Some(argv)) = (params.first().copied(), params.get(1).copied()) else {
            return;
        };
        let init = cx.runtime_ref("buri_rt_argv_init", &[types::I32, PTR], &[]);
        cx.builder.ins().call(init, &[argc, argv]);
        for (i, idx) in tests.iter().enumerate() {
            let Some(callee) = cx.func_ref(*idx) else { continue };
            let enter = cx.runtime_ref("buri_rt_test_enter", &[types::I64], &[types::I32]);
            let index = cx.iconst(types::I64, i as i64);
            let Some(run) = cx.call1(enter, &[index]) else { continue };
            let body = cx.builder.create_block();
            let next = cx.builder.create_block();
            cx.brif(run, body, &[], next, &[]);
            cx.builder.switch_to_block(body);
            cx.builder.ins().call(callee, &[]);
            cx.jump(next, &[]);
            cx.builder.switch_to_block(next);
        }
        let flush = cx.runtime_ref("buri_rt_flush", &[], &[]);
        cx.builder.ins().call(flush, &[]);
        let zero = cx.iconst(types::I32, 0);
        cx.builder.ins().return_(&[zero]);
    });
}

/// The `main` a C runtime calls.
///
/// `cli/runtime/lib.rs` §6 is the whole of the contract, and this is it:
///
/// ```c
/// int main(int argc, char** argv) {
///     buri_rt_argv_init(argc, argv);
///     ...
///     buri_rt_flush();
///     return 0;
/// }
/// ```
///
/// plus the exit convention the JavaScript backend already has
/// (`generate.rs:293`): `.Ok(())` exits 0, and `.Err(msg)` prints `msg` to
/// standard error and exits 1. One sentence in two backends, so that a program
/// that fails prints the same thing whichever one built it.
fn entry_point(u: &mut emit::Unit<'_>, idx: usize) {
    let mut sig = Signature::new(u.abi.call_conv);
    sig.params.push(AbiParam::new(types::I32));
    sig.params.push(AbiParam::new(PTR));
    sig.returns.push(AbiParam::new(types::I32));
    let id: FuncId = match u.module.declare_function("main", Linkage::Export, &sig) {
        Ok(id) => id,
        Err(e) => {
            u.errors.push(format!("cannot declare the entry point: {e}"));
            return;
        }
    };
    let Some(f) = u.program.funcs.get(idx) else { return };
    let ret = f.sig.rets.first().copied();
    let program = u.program;
    let layout = ret.map(|t| u.abi.layout(program, t));
    let leaves = ret.map(|t| u.abi.leaves(program, t)).unwrap_or_default();
    // `main` answers `Result<(), Str>` on every program that can fail, which is
    // three words and therefore the out-pointer form (`abi.rs`'s header). The
    // shim asks the same question the compiled call sites ask rather than
    // knowing the answer for that one type.
    let indirect = u.abi.rets_indirect(program, &f.sig.rets);

    u.build_function(id, sig, "the entry point", |unit, builder| {
        let mut cx = emit::Cx::new(unit, builder);
        let block = cx.builder.create_block();
        cx.builder.append_block_params_for_function_params(block);
        cx.builder.switch_to_block(block);
        let params = cx.builder.block_params(block).to_vec();
        let (Some(argc), Some(argv)) = (params.first().copied(), params.get(1).copied()) else {
            return;
        };
        let init = cx.runtime_ref("buri_rt_argv_init", &[types::I32, PTR], &[]);
        cx.builder.ins().call(init, &[argc, argv]);

        let flush = cx.runtime_ref("buri_rt_flush", &[], &[]);
        let Some(l) = layout.filter(|l| l.size > 0) else {
            if let Some(callee) = cx.func_ref(idx) {
                cx.builder.ins().call(callee, &[]);
            }
            cx.builder.ins().call(flush, &[]);
            let zero = cx.iconst(types::I32, 0);
            cx.builder.ins().return_(&[zero]);
            return;
        };
        let slot = cx.slot(l.size, l.align);
        let Some(callee) = cx.func_ref(idx) else { return };
        if indirect {
            cx.builder.ins().call(callee, &[slot]);
        } else {
            let call = cx.builder.ins().call(callee, &[]);
            let results = cx.builder.inst_results(call).to_vec();
            for (leaf, v) in leaves.iter().zip(results) {
                cx.store_at(slot, leaf.offset, v);
            }
        }
        // The tag, by the same rule `GetTag` uses. Anything that is not an
        // enum — a `main` returning `()` — is a success.
        let tag = match &l.repr {
            Repr::Enum { repr: EnumRepr::Bare { tag }, .. }
            | Repr::Enum { repr: EnumRepr::Tagged { tag, .. }, .. } => {
                let t = emit::scalar_clif(*tag);
                let raw = cx.load_at(t, slot, 0);
                if t == types::I32 {
                    raw
                } else {
                    cx.builder.ins().uextend(types::I32, raw)
                }
            }
            Repr::Enum { repr: EnumRepr::Niche { null_at }, .. } => {
                let p = cx.load_at(PTR, slot, *null_at);
                let is_null =
                    cx.builder.ins().icmp_imm(cranelift_codegen::ir::condcodes::IntCC::Equal, p, 0);
                cx.builder.ins().uextend(types::I32, is_null)
            }
            _ => cx.iconst(types::I32, 0),
        };

        let ok = cx.builder.create_block();
        let bad = cx.builder.create_block();
        let is_err =
            cx.builder.ins().icmp_imm(cranelift_codegen::ir::condcodes::IntCC::NotEqual, tag, 0);
        cx.brif(is_err, bad, &[], ok, &[]);

        cx.builder.switch_to_block(ok);
        cx.builder.ins().call(flush, &[]);
        let zero = cx.iconst(types::I32, 0);
        cx.builder.ins().return_(&[zero]);

        cx.builder.switch_to_block(bad);
        // `.Err(msg)`: the payload is a `Str` at the variant's first field, so
        // the three words the runtime's writer takes are right there.
        let payload = match &l.repr {
            Repr::Enum { variants, .. } => {
                variants.get(1).and_then(|v| v.first().copied()).unwrap_or(0)
            }
            _ => 0,
        };
        let base = cx.load_at(PTR, slot, payload);
        let ptr = cx.load_at(PTR, slot, payload.saturating_add(8));
        let len = cx.load_at(types::I64, slot, payload.saturating_add(16));
        let masked =
            cx.builder.ins().band_imm(len, crate::compiler::middle::layout::STR_LEN_MASK as i64);
        let write =
            cx.runtime_ref("buri_rt_host_stderr_eprintln", &[PTR, PTR, types::I64], &[]);
        cx.builder.ins().call(write, &[base, ptr, masked]);
        cx.builder.ins().call(flush, &[]);
        let one = cx.iconst(types::I32, 1);
        cx.builder.ins().return_(&[one]);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The identity is the Cranelift version, because that is what the bytes
    /// depend on and what a bump has to invalidate.
    #[test]
    fn the_identity_names_the_pinned_version() {
        let id = Cranelift.identity();
        assert!(id.starts_with("cranelift 0.123."), "{id}");
    }

    #[test]
    fn the_backend_is_named_in_every_key_it_produces() {
        assert_eq!(Cranelift.name(), "cranelift");
    }
}
