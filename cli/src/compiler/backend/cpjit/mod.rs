//! The copy-and-patch backend. **Wave 1 of the productization.**
//!
//! A third native backend, behind `backend-cpjit`, which is **on by default**
//! and off on a host with no C compiler. It is not wired into
//! [`select`](super::select) yet: this wave moves it into the tree and holds it
//! to the same seams the other two use, and the seat it is meant to take is
//! gated on correctness parity with Cranelift and on a re-benchmark that has
//! not been run.
//!
//! Design: `design/native/CODEGEN-CPJIT.md`. The technique is Xu and
//! Kjolstad's *Copy-and-Patch Compilation* (OOPSLA 2021).
//!
//! ```text
//! cpjit/
//!   mod.rs      this file: the backend, and one object per codegen unit
//!   abi.rs      the two facts the library builder and the emitter must share
//!   asm.rs      the two hand-written shims: `main`, for a program and for tests
//!   emit.rs     middle::ir into stencil keys
//!   jit.rs      copy, patch, and the three analyses a stencil key needs
//!   lists.rs    `core/list`'s closure surface, open-coded
//!   object.rs   a Mach-O relocatable writer
//!   region.rs   the buffer a unit is copied into, and what leaves it
//!   rtcall.rs   one call into `libburi_rt.a`
//!   runtime.rs  which intrinsic keys have a `buri_rt_*` symbol, and its shape
//!   stencil.rs  a stencil, a hole, and the library's serialized form
//!
//!   sources.rs  the stencil generators — compiled by `cli/build.rs` only
//!   extract.rs  clang's object into stencils, and the four folds — likewise
//!   machobj.rs  a Mach-O reader, for `extract` — likewise
//! ```
//!
//! # What copy-and-patch is, in one paragraph
//!
//! A stencil is the machine code of one C function compiled ahead of time, with
//! its literals, frame offsets and jump targets left as **undefined symbols**.
//! Code generation is then a `memcpy` of the stencil's bytes followed by a
//! store into each hole — no instruction selection, no register allocation, no
//! scheduling. `cli/build.rs` generates the C, compiles it once when the
//! toolchain is built, and embeds the extracted library; `jit.rs::emit` is the
//! copy and the patch, and it is the whole of the code generator.
//!
//! # The frame-threaded convention, and what it costs
//!
//! A stencil is a C function, so it can only take what C can pass. Every one
//! here takes the **frame pointer** in `x0` and three CPS registers, and a call
//! is `fp + frame_size`: the caller writes the callee's arguments where the
//! callee will look for them and branches. There is no machine stack use in
//! generated code at all, and the Buri stack is [`asm::STACK_SYMBOL`].
//!
//! That is not the C convention, so two things have to bridge it, and both are
//! hand-written rather than emitted from stencils:
//!
//!  * **`main`** — [`asm::program_entry`] and [`asm::test_entry`], which are
//!    `cranelift/mod.rs`'s two shims with the same behaviour;
//!  * **a runtime call** — `rtcall.rs`, which places the flattened arguments in
//!    a scratch area and lets one `crt` stencil load them into `x0`–`x7`.
//!
//! # What is not here yet, named rather than left to be discovered
//!
//!  * **Drop glue.** `middle::rc`'s plan is consumed for `IncRef` and for the
//!    inline half of `DecRef`, and the cold half calls `buri_rt_decref` with a
//!    **null** glue pointer. That is right for a value that owns no counted
//!    block and wrong for one that does, so every shape that would need glue is
//!    refused rather than leaked: a `[T]` with a counted element at a runtime
//!    boundary, and `Inst::DecRef` of an aggregate that owns one.
//!  * **`Ret::Res` and `Ret::Tag`** in `runtime.rs`'s table, which is
//!    `bytes.fromUtf8`, `MemFs`'s three, and `str.compare` reached as an
//!    intrinsic rather than as a `Binary`.
//!  * **`Linux`**, and **x86-64**. The stencils are the bytes clang emitted for
//!    arm64 functions and `object.rs` writes Mach-O; both halves need a second
//!    one, and `Cpjit::AVAILABLE` is false where they are missing.
//!  * **Debug information.** Neither DWARF nor `.buri_symbols`, which is the
//!    same gap `cranelift/mod.rs` records for itself.

pub mod abi;
pub mod asm;
pub mod emit;
pub mod jit;
pub mod lists;
pub mod object;
pub mod region;
pub mod rtcall;
pub mod runtime;
pub mod stencil;

use crate::build::buildfile::{Arch, Platform};
use crate::build::cache::{hash_bytes, ActionKey};
use crate::compiler::backend::{Backend, Emitted, Options, Profile, Units};
use crate::compiler::middle::layout::{EnumRepr, Layouts, Repr};
use crate::compiler::middle::monomorphize::{Program, ProgramRoots};
use crate::compiler::middle::{ir, lower};
use crate::compiler::semantics::types::Tables;
use crate::diagnostics::{Diagnostic, Diagnostics, Span};
use std::sync::OnceLock;

/// The stencil library, built by `cli/build.rs`.
///
/// **Empty** on a host that has no C compiler, or that is not arm64. The
/// emptiness *is* the signal, exactly as it is for `runtime_native::ARCHIVE`:
/// there is no conditional compilation for a `check-cfg` list to know about,
/// and [`AVAILABLE`] is the question to ask.
const LIBRARY_BYTES: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/cpjit-stencils.bin"));

/// Whether this toolchain has stencils to emit from.
pub const AVAILABLE: bool = !LIBRARY_BYTES.is_empty();

/// The decoded library, once per process.
///
/// Decoding twenty-three thousand stencils is tens of milliseconds and the
/// whole claim of this backend is compile time, so it is paid once for a build
/// rather than once per codegen unit — a `buri build` at a hundred thousand
/// lines emits several hundred units in one process.
fn library() -> Result<&'static stencil::Library, String> {
    static LIB: OnceLock<Result<stencil::Library, String>> = OnceLock::new();
    match LIB.get_or_init(|| stencil::Library::decode(LIBRARY_BYTES)) {
        Ok(l) => Ok(l),
        Err(e) => Err(e.clone()),
    }
}

/// The copy-and-patch backend.
#[derive(Default)]
pub struct Cpjit;

impl Backend for Cpjit {
    fn name(&self) -> &'static str {
        "cpjit"
    }

    /// The stencil library's own hash.
    ///
    /// Not a version string, because there is no version to name: the library
    /// is generated by this repository and compiled by whatever `cc` was on the
    /// host when the toolchain was built, and *both* halves change the bytes
    /// this backend emits. Hashing the library covers the generators, the
    /// folds, the register width and the C compiler in one number, and a
    /// toolchain built against a different `cc` therefore shares no cached
    /// object with one built against this — which is the property
    /// `Backend::identity` exists for.
    fn identity(&self) -> String {
        format!("cpjit {}", hash_bytes(LIBRARY_BYTES))
    }

    /// Which intrinsics this backend has no body for, asked of the program up
    /// front so that a program using one is told before a second is spent on
    /// it.
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

    /// One object per codegen unit, which is the granularity
    /// `build::actions::codegen_units` caches at.
    ///
    /// Everything above the loop is whole-program and stays so — the lowering
    /// reads the program, and a unit's object depends on the program it was
    /// lowered from rather than on the other units' objects — which is the same
    /// shape `cranelift/mod.rs::emit_units` has.
    fn emit_units(
        &mut self,
        program: &Program,
        tables: &Tables,
        opts: &Options<'_>,
        units: Units<'_>,
    ) -> Result<Vec<Emitted>, Diagnostics> {
        if let Err(e) = supported(opts) {
            return Err(one(e));
        }
        let lib = match library() {
            Ok(l) => l,
            Err(e) => return Err(one(e)),
        };
        let lowered = lower::run(program, tables);
        if cfg!(debug_assertions) {
            let problems = ir::verify(&lowered);
            if !problems.is_empty() {
                return Err(one(format!(
                    "internal error: the lowered IR is malformed: {}",
                    problems.join("; ")
                )));
            }
        }
        let root = match &program.roots {
            ProgramRoots::Main(idx) => Root::Main(idx.index()),
            ProgramRoots::Tests(tests) => {
                Root::Tests(tests.iter().map(|t| t.func.index()).collect())
            }
        };
        let members = lowered.funcs_by_unit();
        let empty: Vec<usize> = Vec::new();

        let mut out = Vec::new();
        let mut errors: Vec<String> = Vec::new();
        for (index, name) in lowered.units.iter().enumerate() {
            let unit = u32::try_from(index).unwrap_or(0);
            if !units.wants(unit) {
                continue;
            }
            let mine = members.get(index).unwrap_or(&empty);
            match compile_unit(lib, &lowered, tables, unit, name, mine, &root) {
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

/// Which root a program has, with the indices resolved.
enum Root {
    Main(usize),
    Tests(Vec<usize>),
}

fn one(message: String) -> Diagnostics {
    let mut diags = Diagnostics::new();
    diags.push(Diagnostic::error(Span::NONE, message));
    diags
}

/// The one target this backend has, stated as a refusal rather than as a
/// silent wrong answer.
///
/// A stencil is the bytes `cc` emitted for an arm64 function, so there is no
/// sense in which this backend can target another architecture, and
/// `object.rs` writes Mach-O. Both halves are named here so that
/// `--output=linux/arm64` fails with a sentence rather than with a link error.
fn supported(opts: &Options<'_>) -> Result<(), String> {
    if !AVAILABLE {
        return Err(String::from(
            "this toolchain was built without a stencil library, so the cpjit \
             backend is not available (it needs a C compiler and an arm64 host)",
        ));
    }
    match opts.target.platform {
        Platform::Macos => {}
        p => {
            return Err(format!(
                "the cpjit backend emits Mach-O and has no {} object writer yet",
                p.slug()
            ))
        }
    }
    match opts.target.arch {
        Some(Arch::Arm64) | None if cfg!(target_arch = "aarch64") => Ok(()),
        _ => Err(String::from("the cpjit backend's stencils are arm64 and there is no other set")),
    }
}

/// One codegen unit, from IR to object bytes.
fn compile_unit(
    lib: &stencil::Library,
    program: &ir::Program,
    tables: &Tables,
    unit: u32,
    name: &str,
    members: &[usize],
    root: &Root,
) -> Result<Emitted, Vec<String>> {
    let mut j = jit::Jit::new(lib, tables, unit);
    j.compile_unit(program, members);

    // A refused IR shape is a diagnostic naming the shape, never an artifact
    // that aborts when it reaches it. The emission is finished first so that
    // one build reports every refusal rather than the first.
    let refused = j.reasons();
    if !refused.is_empty() {
        return Err(refused
            .iter()
            .map(|r| format!("the cpjit backend cannot compile {r} yet"))
            .collect());
    }

    let entries: Vec<u64> = members.iter().map(|i| j.entry_of(*i)).collect();
    let emitted = std::mem::take(&mut j.region).finish();
    let mut code = emitted.code;

    // The entry point goes in the unit that owns `main`, so that a program is
    // one `_start`-adjacent symbol and the other units are libraries. A test
    // binary has no `main` to own it, so it goes in the unit that owns the
    // *first* test — the same rule `cranelift/mod.rs` applies to the root that
    // exists.
    let mut shim: Option<asm::Asm> = None;
    match root {
        Root::Main(idx) if members.contains(idx) => {
            let sym = jit::symbol_of(program, u32::try_from(*idx).unwrap_or(0));
            shim = Some(asm::program_entry(&sym, main_result(program, tables, *idx)));
        }
        Root::Tests(tests) if tests.first().is_some_and(|i| members.contains(i)) => {
            let names: Vec<String> = tests
                .iter()
                .map(|i| jit::symbol_of(program, u32::try_from(*i).unwrap_or(0)))
                .collect();
            shim = Some(asm::test_entry(&names));
        }
        _ => {}
    }

    let mut symbols: Vec<object::Symbol> = Vec::new();
    let mut index: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    let want = |symbols: &mut Vec<object::Symbol>,
                    index: &mut std::collections::HashMap<String, usize>,
                    name: &str|
     -> usize {
        if let Some(i) = index.get(name) {
            return *i;
        }
        let i = symbols.len();
        symbols.push(object::Symbol {
            name: String::from(name),
            defined: None,
            global: true,
        });
        index.insert(String::from(name), i);
        i
    };

    // Every member of this unit is defined here and visible outside it: a call
    // from another unit is a relocation against this name.
    for (i, entry) in members.iter().zip(&entries) {
        let sym = jit::symbol_of(program, u32::try_from(*i).unwrap_or(0));
        let at = want(&mut symbols, &mut index, &sym);
        if let Some(s) = symbols.get_mut(at) {
            s.defined = Some(object::Definition { section: 0, offset: *entry });
        }
    }

    let mut out: Vec<object::Reloc> = Vec::new();
    let name_of = |target: &region::Target| -> String {
        match target {
            region::Target::Func(f) => jit::symbol_of(program, *f),
            region::Target::Symbol(s) => s.clone(),
            // The pool's own base, under a local name no Buri symbol can
            // collide with: Mach-O has no "this section" relocation that is
            // not scattered, so an offset into the pool is a symbol plus an
            // addend.
            region::Target::Here(_) | region::Target::Pool => String::from(POOL_ANCHOR),
        }
    };
    for (section, r) in emitted
        .code_relocs
        .iter()
        .map(|r| (CODE, r))
        .chain(emitted.pool_relocs.iter().map(|r| (POOL, r)))
    {
        let sym = want(&mut symbols, &mut index, &name_of(&r.target));
        out.push(object::Reloc {
            section,
            offset: r.at,
            kind: match r.kind {
                region::RelocKind::Branch26 => object::RelKind::Branch26,
                region::RelocKind::Abs64 => object::RelKind::Abs64,
                region::RelocKind::Page21 => object::RelKind::Page21,
                region::RelocKind::PageOff12 => object::RelKind::PageOff12,
            },
            symbol: sym,
            addend: r.addend,
        });
    }
    let anchor = want(&mut symbols, &mut index, POOL_ANCHOR);
    if let Some(s) = symbols.get_mut(anchor) {
        s.defined = Some(object::Definition { section: POOL, offset: 0 });
        s.global = false;
    }

    let mut sections = vec![
        object::Section {
            name: "__text",
            segment: "__TEXT",
            align: region::CODE_ALIGN,
            attributes: object::CODE_ATTRIBUTES,
            zerofill: 0,
            data: Vec::new(),
        },
        object::Section {
            name: "__const",
            segment: "__DATA_CONST",
            align: region::POOL_ALIGN,
            attributes: 0,
            zerofill: 0,
            data: emitted.pool,
        },
    ];

    if let Some(shim) = shim {
        // The shim goes after the bodies rather than before them, so that the
        // offsets `resolve` already patched stay where they are.
        while !code.len().is_multiple_of(16) {
            code.push(0);
        }
        let at = code.len() as u64;
        let (bytes, srelocs) = shim.finish();
        code.extend_from_slice(&bytes);
        let main = want(&mut symbols, &mut index, "main");
        if let Some(s) = symbols.get_mut(main) {
            s.defined = Some(object::Definition { section: 0, offset: at });
        }
        for (off, kind, target) in srelocs {
            let sym = want(&mut symbols, &mut index, &name_of(&target));
            out.push(object::Reloc {
                section: CODE,
                offset: at.saturating_add(off),
                kind,
                symbol: sym,
                addend: 0,
            });
        }
        // The Buri stack, in its own zero-filled section so that it costs no
        // bytes in the object or in the artifact.
        sections.push(object::Section {
            name: "__bss",
            segment: "__DATA",
            align: asm::STACK_ALIGN,
            attributes: 0,
            zerofill: asm::STACK_BYTES,
            data: Vec::new(),
        });
        let stack = want(&mut symbols, &mut index, asm::STACK_SYMBOL);
        if let Some(s) = symbols.get_mut(stack) {
            s.defined = Some(object::Definition { section: STACK, offset: 0 });
        }
    }

    if let Some(s) = sections.first_mut() {
        s.data = code;
    }
    let bytes = object::write(&sections, &symbols, &out).map_err(|e| vec![e])?;

    // The `codegen` key is `H(the unit's lowered IR)` (ARCHITECTURE.md §6.2),
    // and `ir::Program`'s `Display` is a faithful, total and deterministic
    // function of the IR with no hash order in it — the same key
    // `cranelift/mod.rs` computes, from the same text.
    let mut text = String::new();
    for f in members.iter().filter_map(|i| program.funcs.get(*i)) {
        text.push_str(&program.render_func(f));
    }
    Ok(Emitted { name: format!("{name}.o"), key: ActionKey::of(text.as_bytes()), bytes })
}

/// The local symbol every pool offset is measured from.
///
/// `$` cannot appear in a Buri path, so no `ir::Func::symbol` can collide with
/// it (`monomorphize::Func::symbol`).
const POOL_ANCHOR: &str = "buri$cpjit$pool";

/// The sections a unit's object has, in the order `object::write` takes them:
/// the code, the constant pool, and — in the unit that owns `main` — the Buri
/// stack, which has to be last because it is the zero-filled one.
const CODE: usize = 0;
const POOL: usize = 1;
const STACK: usize = 2;

/// Where a `main` returning `Result<(), Str>` keeps its answer.
///
/// Read off `middle::layout` here so that `asm.rs` never learns a layout rule,
/// and `None` where there is nothing to inspect — a `main` answering `()` is a
/// success unconditionally, which is `cranelift/mod.rs`'s rule too.
fn main_result(program: &ir::Program, tables: &Tables, idx: usize) -> Option<asm::MainResult> {
    let f = program.funcs.get(idx)?;
    let ir::Type::Agg(id) = f.sig.rets.first().copied()? else { return None };
    let mut layouts = Layouts::new(tables);
    let l = layouts.of(program.type_info(id).ty.clone());
    if l.size == 0 {
        return None;
    }
    let Repr::Enum { repr, variants } = &l.repr else { return None };
    // `.Err(msg)`'s payload is a `Str` at the variant's first field.
    let payload = variants.get(1).and_then(|v| v.first()).copied().unwrap_or(0);
    let message = (payload, payload.saturating_add(8), payload.saturating_add(16));
    Some(match repr {
        EnumRepr::Bare { tag } | EnumRepr::Tagged { tag, .. } => {
            asm::MainResult { tag: (0, tag.size()), niche: None, message }
        }
        EnumRepr::Niche { null_at } => {
            asm::MainResult { tag: (*null_at, 8), niche: Some(*null_at), message }
        }
    })
}

/// A debug build is the only profile this backend is for.
///
/// Stated as a function rather than as an assertion because nothing calls it
/// yet: `select` still sends every native build to Cranelift or LLVM, and this
/// wave deliberately does not change that.
pub fn profile_is_debug(profile: Profile) -> bool {
    profile == Profile::Debug
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_backend_is_named_in_every_key_it_produces() {
        assert_eq!(Cpjit.name(), "cpjit");
    }

    /// The identity has to move when the stencils do, because the stencils are
    /// most of what the emitted bytes are.
    #[test]
    fn the_identity_is_the_librarys_hash() {
        let id = Cpjit.identity();
        assert!(id.starts_with("cpjit "), "{id}");
        assert_eq!(id.len(), "cpjit ".len() + 64);
    }

    /// A host with a library must have one that decodes; a host without must
    /// say so rather than fail to build.
    #[test]
    fn the_library_matches_its_availability() {
        assert_eq!(AVAILABLE, library().is_ok());
    }
}
