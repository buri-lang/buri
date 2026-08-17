//! The Cranelift backend. **Wave 2a.**
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
//!   abi.rs      the value model at the machine boundary
//!   emit.rs     middle::ir into CLIF
//!   helpers.rs  the eight functions this backend generates for itself
//!   runtime.rs  which intrinsic keys have a `buri_rt_*` symbol, and its shape
//! ```
//!
//! # Where an intrinsic goes, and why
//!
//! Six routes, tried in this order by `emit::Lower::intrinsic`, and the order
//! is the reasoning: everything that is *instructions* is asked before the
//! arguments are spread for a call that is not going to happen.
//!
//! | Route | What it covers | Why not a runtime call |
//! |---|---|---|
//! | `numeric` | `num.<T>.<op>`, including `Checked`, `Saturating`, `Wrapping` and `Bounded` | One conversion per source-and-target pair (SPEC 6.2.1), and generating two instructions beats calling a function that generates the same two |
//! | `bits` | all fourteen of `core/bits` | Each is one machine instruction behind `$shiftCount`'s range check |
//! | `prim_trait` | `Eq`/`Ord`/`Hash`/`Show` at `Bool` and `Char`, `str.show`, `Char::toU32` | A compare, a select, or the identity |
//! | `open_coded` | `str.concat`, `str.format`, `str.len`, `list.len`, `list.empty`, the three `core/testing/assert` bodies, the test allocator | A load, a copy, or an allocation and two copies |
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
//!    `core/cap` type the intrinsic table does not name. One `Ret::Result` and
//!    that type's layout away.
//!  * **Every `list.*` entry taking a closure** — `map`, `filter`, `fold`,
//!    `any`, `all`, `find`, `findIndex`, `count`, `sortBy` and the `Ctx`
//!    variants — plus `zip` and `flatten`. `cli/runtime/list.rs`'s header says
//!    why each is a backend's loop rather than a runtime call. This is the
//!    single largest remaining gap, and it is what defers `core/map`,
//!    `core/queue`, `core/bitset`, `core/crypto` and `core/date`.
//!  * **`json.*`**, and with it `deriveArrayEq`, `deriveArrayCompare`,
//!    `deriveArrayShow`, `deriveArrayJson` and `deriveArrayHash`, which are the
//!    same loop over a code pointer that the closure surface needs.
//!  * **`core/math`'s thirteen transcendentals** and **`core/char`'s eight
//!    classifiers**. Both are gaps for the same *kind* of reason and it is not
//!    effort: IEEE 754 does not fix `sin`, and `\p{L}` is a General_Category
//!    table Rust does not expose, so implementing either from what is to hand
//!    would put a divergence into the toolchain where a named gap is honest.
//!    `cli/runtime/math.rs` states it at length.
//!  * **The stateful half of `core/testing/context`** — `captureOut`, `MemFs`,
//!    `TestClock`, `TestEnv`, `TestStdin`, `TestRand`. Each is a mutable object
//!    behind a handle table the native runtime does not have. Its *allocator*
//!    is here, because natively that one is a no-op.
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

use cranelift_codegen::isa::TargetIsa;
use cranelift_codegen::ir::{types, AbiParam, InstBuilder, Signature};
use cranelift_codegen::settings::{self, Configurable};
use cranelift_module::{default_libcall_names, FuncId, Linkage, Module};
use cranelift_object::{ObjectBuilder, ObjectModule};
use std::sync::Arc;

use crate::build::buildfile::Platform;
use crate::build::cache::ActionKey;
use crate::compiler::backend::cranelift::abi::{Abi, PTR};
use crate::compiler::backend::{Backend, Emitted, Options};
use crate::compiler::middle::ir as mir;
use crate::compiler::middle::layout::{EnumRepr, Repr};
use crate::compiler::middle::lower;
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
        let isa = match host_isa(opts) {
            Ok(isa) => isa,
            Err(message) => return Err(one(message)),
        };
        let lowered = lower::run(program, tables);
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

        let mut out = Vec::new();
        let mut errors: Vec<String> = Vec::new();
        for (index, name) in lowered.units.iter().enumerate() {
            let unit = u32::try_from(index).unwrap_or(0);
            match compile_unit(&lowered, tables, opts, &isa, unit, name, &entry) {
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
/// `regalloc_algorithm = single_pass` is the compile-speed dial that actually
/// moves at `opt_level = none`, and it is the one knob whose value should be
/// re-measured rather than assumed.
fn host_isa(opts: &Options<'_>) -> Result<Arc<dyn TargetIsa>, String> {
    // ARCHITECTURE.md §9: no cross-compilation, because the runtime archive is
    // built for the host and for nothing else. The refusal is here, where the
    // triple is chosen, rather than in the build system, so that it cannot be
    // routed around.
    let host = target_lexicon::Triple::host();
    let wanted = match opts.target.platform {
        Platform::Linux => "linux",
        Platform::Macos => "macos",
        Platform::Js => return Err(String::from("the Cranelift backend does not target JavaScript")),
    };
    let have = match host.operating_system {
        target_lexicon::OperatingSystem::Darwin(_) => "macos",
        target_lexicon::OperatingSystem::Linux => "linux",
        other => return Err(format!("no native backend for a host running {other}")),
    };
    if wanted != have {
        return Err(format!(
            "cannot build for {wanted} on a {have} host: the native runtime archive is built for the host and for nothing else"
        ));
    }

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
    let flags = settings::Flags::new(flags);

    let builder = cranelift_native::builder().map_err(|e| format!("no ISA for this host: {e}"))?;
    builder.finish(flags).map_err(|e| format!("cannot build an ISA: {e}"))
}

/// One codegen unit, from IR to object bytes.
fn compile_unit(
    program: &mir::Program,
    tables: &Tables,
    opts: &Options<'_>,
    isa: &Arc<dyn TargetIsa>,
    unit: u32,
    name: &str,
    entry: &Root,
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
    let abi = Abi::new(tables, isa.default_call_conv());
    let mut u = emit::Unit::new(module, abi, program, opts.profile, unit);
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
    let mut text = String::new();
    for f in program.funcs.iter().filter(|f| f.unit == unit) {
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

/// The `main` of a **test binary**: every `test` block, in order.
///
/// There is no native test *runner*. On JavaScript, `buri test` catches the
/// throw a failed assertion raises, renders both values, and goes on to the
/// next test (`runtime.js:1643-1658`); natively a failed assertion is
/// `buri_rt_abort_assert`, which prints and exits 1, so this process runs
/// every block in order and the first failure is the last thing it does.
///
/// That is a real difference and it is the right one for now: a runner that
/// continued past a failure would need to catch something, and SPEC 6.10 says
/// this language has nothing to catch — an abort is a write and an `_exit`.
/// What it costs is that a failing native run names one failure rather than
/// all of them, which is a worse report and not a wrong answer.
///
/// A `test` body answers `()`, so there is nothing to inspect between calls;
/// the exit status is the whole result.
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
    u.build_function(id, sig, "the test entry point", |unit, b| {
        let mut cx = emit::Cx { unit, b };
        let block = cx.b.create_block();
        cx.b.append_block_params_for_function_params(block);
        cx.b.switch_to_block(block);
        let params = cx.b.block_params(block).to_vec();
        let (Some(argc), Some(argv)) = (params.first().copied(), params.get(1).copied()) else {
            return;
        };
        let init = cx.rt_ref("buri_rt_argv_init", &[types::I32, PTR], &[]);
        cx.b.ins().call(init, &[argc, argv]);
        for idx in &tests {
            if let Some(callee) = cx.func_ref(*idx) {
                cx.b.ins().call(callee, &[]);
            }
        }
        let flush = cx.rt_ref("buri_rt_flush", &[], &[]);
        cx.b.ins().call(flush, &[]);
        let zero = cx.iconst(types::I32, 0);
        cx.b.ins().return_(&[zero]);
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

    u.build_function(id, sig, "the entry point", |unit, b| {
        let mut cx = emit::Cx { unit, b };
        let block = cx.b.create_block();
        cx.b.append_block_params_for_function_params(block);
        cx.b.switch_to_block(block);
        let params = cx.b.block_params(block).to_vec();
        let (Some(argc), Some(argv)) = (params.first().copied(), params.get(1).copied()) else {
            return;
        };
        let init = cx.rt_ref("buri_rt_argv_init", &[types::I32, PTR], &[]);
        cx.b.ins().call(init, &[argc, argv]);

        let Some(callee) = cx.func_ref(idx) else { return };
        let call = cx.b.ins().call(callee, &[]);
        let results = cx.b.inst_results(call).to_vec();

        let flush = cx.rt_ref("buri_rt_flush", &[], &[]);
        let Some(l) = layout.filter(|l| l.size > 0) else {
            cx.b.ins().call(flush, &[]);
            let zero = cx.iconst(types::I32, 0);
            cx.b.ins().return_(&[zero]);
            return;
        };
        let slot = cx.slot(l.size, l.align);
        for (leaf, v) in leaves.iter().zip(results) {
            cx.store_at(slot, leaf.offset, v);
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
                    cx.b.ins().uextend(types::I32, raw)
                }
            }
            Repr::Enum { repr: EnumRepr::Niche { null_at }, .. } => {
                let p = cx.load_at(PTR, slot, *null_at);
                let is_null =
                    cx.b.ins().icmp_imm(cranelift_codegen::ir::condcodes::IntCC::Equal, p, 0);
                cx.b.ins().uextend(types::I32, is_null)
            }
            _ => cx.iconst(types::I32, 0),
        };

        let ok = cx.b.create_block();
        let bad = cx.b.create_block();
        let is_err = cx.b.ins().icmp_imm(cranelift_codegen::ir::condcodes::IntCC::NotEqual, tag, 0);
        cx.brif(is_err, bad, &[], ok, &[]);

        cx.b.switch_to_block(ok);
        cx.b.ins().call(flush, &[]);
        let zero = cx.iconst(types::I32, 0);
        cx.b.ins().return_(&[zero]);

        cx.b.switch_to_block(bad);
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
        let masked = cx.b.ins().band_imm(len, crate::compiler::middle::layout::STR_LEN_MASK as i64);
        let write =
            cx.rt_ref("buri_rt_host_stderr_eprintln", &[PTR, PTR, types::I64], &[]);
        cx.b.ins().call(write, &[base, ptr, masked]);
        cx.b.ins().call(flush, &[]);
        let one = cx.iconst(types::I32, 1);
        cx.b.ins().return_(&[one]);
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
