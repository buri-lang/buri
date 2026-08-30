//! The copy-and-patch backend.
//!
//! The native **development** backend, behind `backend-stencil`, which is **on
//! by default** and off on a host with no C compiler. Every native debug build
//! comes through here: [`select`](super::select) answers `stencil` for a target
//! [`supported`] has a library for, and refuses by name for the one it does
//! not.
//!
//! It compiles, links, runs and passes the **same 997 native conformance tests
//! through `buri test`** the backend it replaced did, refuses the same six
//! packages for the same three reasons, and leaves the same blocks live at exit
//! on every one.
//!
//! What the benchmark says, so that nothing here reads as a claim it does not
//! make. Against the removed backend at `opt_level=none` on macOS/arm64
//! (`design/native/CODEGEN-STENCIL.md` §13, `design/PERFORMANCE.md` §6):
//!
//!  * **the compile side is this backend's** — emission of a 121k-line program
//!    in 367 units is about 0.43× the incumbent's, re-taken at 0.47×, and a
//!    cold `buri build` about 0.65×;
//!  * **the run side is still the incumbent's, and by much less than it was** —
//!    the four kernels were 1.38×, from 1.86× before the slots-only `crt`
//!    family (`design/native/CODEGEN-STENCIL.md` §5.1), and a fresh six-kernel
//!    series after the removal reads 1.26× (`design/PERFORMANCE.md` §6.2). What
//!    is left is one kernel — `core/list`'s closure surface — rather than the
//!    runtime boundary;
//!  * **it is three targets and the one it replaced was four** — `macos-arm64`,
//!    `linux-arm64` and `linux-x86_64` all emit, link and run; the fourth,
//!    `macos-x86_64`, is a combination this repository builds no library for
//!    and does not intend to.
//!
//! Design: `design/native/CODEGEN-STENCIL.md`. The technique is Xu and
//! Kjolstad's *Copy-and-Patch Compilation* (OOPSLA 2021).
//!
//! ```text
//! stencil/
//!   mod.rs      this file: the backend, and one object per codegen unit
//!   abi.rs      the two facts the library builder and the emitter must share
//!   asm.rs      the hand-written shims: `main`, for a program and for tests,
//!               and the carrier door in front of each root (`carrier.rs`)
//!   emit.rs     middle::ir into stencil keys
//!   glue.rs     the functions a unit generates for itself: thunks, drop glue
//!   jit.rs      copy, patch, and the three analyses a stencil key needs
//!   lists.rs    `core/list`'s closure surface, open-coded
//!   elf.rs      an ELF64 relocatable writer, for the two Linux targets
//!   object.rs   a Mach-O relocatable writer
//!   region.rs   the buffer a unit is copied into, and what leaves it
//!   rtcall.rs   one call into `libburi_rt.a`
//!   runtime.rs  which intrinsic keys have a `buri_rt_*` symbol, and its shape
//!   library.rs  a stencil, a hole, and the library's serialized form
//!
//!   sources.rs  the stencil generators — compiled by `cli/build.rs` only
//!   extract.rs  clang's object into stencils, and the four folds — likewise
//!   machobj.rs  a Mach-O reader, for `extract` — likewise
//!   elfobj.rs   an ELF reader, for `extract` — likewise, and by `elf.rs`
//!               under `cfg(test)`, which is what checks the writer
//!   x86.rs      the x86-64 half of `extract` — likewise
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
//! # Three targets, one generator
//!
//! `macos-arm64`, `linux-arm64` and `linux-x86_64` — [`abi::StencilTarget`]. A
//! stencil is the bytes clang emitted for a C function, so it belongs to an
//! instruction set *and* a container, and a toolchain bakes one library per
//! pair. The two Linux
//! libraries are cross-compiled by the same `cc`, which works because the
//! generated C includes `<stdint.h>` and nothing else; `CODEGEN-STENCIL.md` §3.2
//! is the whole of it.
//!
//! The split of labour is the ISA on one axis and the container on the other,
//! and every file below is on exactly one of them: `extract.rs` and `x86.rs`
//! read instructions, `machobj.rs`/`elfobj.rs` and `object.rs`/`elf.rs` read and
//! write containers, and neither pair knows about the other's axis.
//!
//! # The frame-threaded convention, and what it costs
//!
//! A stencil is a C function, so it can only take what C can pass. Every one
//! here takes the **frame pointer** in `x0` — `rdi` on x86-64 — and three CPS
//! registers, and a call is `fp + frame_size`: the caller writes the callee's
//! arguments where the callee will look for them and branches. There is no
//! machine stack use in generated code at all, and the Buri stack is
//! [`asm::STACK_SYMBOL`].
//!
//! That is not the C convention, so two things have to bridge it, and both are
//! hand-written rather than emitted from stencils:
//!
//!  * **`main`** — [`asm::program_entry`] and [`asm::test_entry`], the two
//!    shims the removed backend emitted, hand-written here;
//!  * **a runtime call** — `rtcall.rs`, whose `crt` stencil loads the flattened
//!    arguments into `x0`–`x7`, each from its own frame-offset hole.
//!
//! Generated code touches the machine stack nowhere, so the Buri stack is a
//! `__bss` block with a `PROT_NONE` guard `main` installs above it
//! ([`asm::install_guard`]): a runaway recursion faults at the boundary rather
//! than writing past it.
//!
//! # What is not here, named rather than left to be discovered
//!
//! `design/native/CODEGEN-STENCIL.md` §9 is the list and separates the two kinds.
//! What is **this backend's**:
//!
//!  * **macOS on x86-64.** No stencil library is built for it and none is
//!    intended: those stencils would be x86-64 instructions in a Mach-O, and
//!    nothing this repository runs on or ships to is that. [`supported`]
//!    refuses it by name.
//!  * **Linux execution, from a macOS host.** Both Linux targets emit objects a
//!    real linker accepts and fully resolves, and nothing on a macOS machine
//!    can run one: `link::can_link` refuses a cross target and
//!    `runtime_native::ARCHIVE` is the host's alone.
//!    `design/native/CODEGEN-STENCIL.md` §10 is what was checked, what was not,
//!    and what the Linux CI legs confirm.
//!  * **Debug information.** Neither DWARF nor `.buri_symbols`, which is the
//!    same gap `llvm/mod.rs` records for itself.
//!
//! What is refused by **every** backend — a conversion out of a float or into
//! a `Char`, `json.*`, and `core/math`'s thirteen transcendentals — is refused
//! here for the reasons `native/conformance.rs`'s `PACKAGES` gives.

pub mod abi;
pub mod asm;
pub mod elf;
pub mod emit;
pub mod glue;
pub mod jit;
pub mod library;
pub mod lists;
pub mod object;
pub mod region;
pub mod rtcall;
pub mod runtime;

use crate::build::buildfile::{Arch, Platform};
use crate::build::cache::ActionKey;
use crate::compiler::backend::carrier;
use crate::compiler::backend::{Backend, Emitted, Options, Target, Units};
use crate::compiler::middle::layout::{EnumRepr, Layouts, Repr};
use crate::compiler::middle::monomorphize::{Program, ProgramRoots};
use crate::compiler::middle::{ir, lower};
use crate::compiler::semantics::types::Tables;
use crate::diagnostics::{Diagnostic, Diagnostics, Span};
use std::sync::OnceLock;

/// The stencil libraries, built by `cli/build.rs` — one per
/// [`abi::StencilTarget`].
///
/// **Empty** where the toolchain's `cc` could not produce that target's
/// objects: the host's own on a machine with no C compiler, and the two cross
/// ones on a machine whose clang cannot be pointed at another triple. The
/// emptiness *is* the signal, exactly as it is for `runtime_native::ARCHIVE`:
/// there is no conditional compilation for a `check-cfg` list to know about,
/// and [`available_for`] is the question to ask.
///
/// Three constants rather than an array built from
/// [`abi::StencilTarget::ALL`], because `include_bytes!` takes a literal path
/// and there is no way to spell the loop.
/// [`blob`] is the one place the three are matched to their targets.
const MACOS_ARM64_BYTES: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/stencils-macos-arm64.bin"));
const LINUX_ARM64_BYTES: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/stencils-linux-arm64.bin"));
const LINUX_X86_64_BYTES: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/stencils-linux-x86_64.bin"));

/// Each library's SHA-256, **computed when the toolchain was built** and
/// written beside its blob by `cli/build.rs`.
///
/// The same sixty-four hex digits `build::cache::hash_bytes` would produce from
/// the bytes, because the script and that function are the same source file
/// (`cli/src/build/sha256.rs`, which the script `#[path]`-includes). One
/// implementation is the only way to be sure the two agree, and they must:
/// [`Stencil::identity`] is a **cache key**, so a digest that differed from the
/// one an earlier toolchain computed would invalidate every cached object
/// without anything having changed.
///
/// Each exists on every host, including one with no `cc` and three empty
/// libraries — the script writes a digest on every path — so this compiles
/// where the backend is unavailable.
const MACOS_ARM64_SHA256: &str =
    include_str!(concat!(env!("OUT_DIR"), "/stencils-macos-arm64.bin.sha256"));
const LINUX_ARM64_SHA256: &str =
    include_str!(concat!(env!("OUT_DIR"), "/stencils-linux-arm64.bin.sha256"));
const LINUX_X86_64_SHA256: &str =
    include_str!(concat!(env!("OUT_DIR"), "/stencils-linux-x86_64.bin.sha256"));

/// One target's blob and its baked digest.
fn blob(t: abi::StencilTarget) -> (&'static [u8], &'static str) {
    match t {
        abi::StencilTarget::MacosArm64 => (MACOS_ARM64_BYTES, MACOS_ARM64_SHA256),
        abi::StencilTarget::LinuxArm64 => (LINUX_ARM64_BYTES, LINUX_ARM64_SHA256),
        abi::StencilTarget::LinuxX86_64 => (LINUX_X86_64_BYTES, LINUX_X86_64_SHA256),
    }
}

/// Whether this toolchain has stencils for one target.
pub fn available_for(t: abi::StencilTarget) -> bool {
    !blob(t).0.is_empty()
}

/// Whether this toolchain can build a **runnable** stencil program on the machine
/// it is running on.
///
/// The question every caller outside this module was asking when there was one
/// library, and the one the tests still ask; a *cross* target is
/// [`available_for`]'s business and this is deliberately not it.
///
/// Two things are needed and both are named: the host's stencil library has to
/// be non-empty, and — on x86-64 — `asm.rs` has to have an entry point to put in
/// front of it. Folding the second in here is what lets
/// `tests/native/stencil.rs` ask one question instead of spelling out which hosts
/// are which, and it is why that suite's guard is not a `cfg!(target_os =
/// "macos")`. [`unavailable_reason`] is the same question with the reason
/// attached, which is what a skipping suite prints.
pub const AVAILABLE: bool = !MACOS_ARM64_BYTES.is_empty()
    && cfg!(all(target_os = "macos", target_arch = "aarch64"))
    || !LINUX_ARM64_BYTES.is_empty() && cfg!(all(target_os = "linux", target_arch = "aarch64"))
    || !LINUX_X86_64_BYTES.is_empty()
        && cfg!(all(target_os = "linux", target_arch = "x86_64"))
        && asm::AVAILABLE_X86_64;

/// The stencil target this host *is*, or `None` where no library is built for
/// one — x86-64 macOS is the only such host.
pub fn host_stencil_target() -> Option<abi::StencilTarget> {
    match (cfg!(target_os = "macos"), cfg!(target_arch = "aarch64")) {
        (true, true) => Some(abi::StencilTarget::MacosArm64),
        (true, false) => None,
        (false, true) => Some(abi::StencilTarget::LinuxArm64),
        (false, false) => Some(abi::StencilTarget::LinuxX86_64),
    }
}

/// Why this host cannot build a runnable stencil program, or `None` where it
/// can.
///
/// [`AVAILABLE`] as a sentence, and the one a test suite prints when it skips.
/// A suite that asked `cfg!(target_arch)` for itself would have to be edited
/// the day the missing half lands; one that asks here lights up instead.
pub fn unavailable_reason() -> Option<String> {
    if AVAILABLE {
        return None;
    }
    let Some(host) = host_stencil_target() else {
        return Some(String::from("stencil has no x86_64-apple-darwin backend yet"));
    };
    if !available_for(host) {
        return Some(format!(
            "this toolchain was built without {} stencils, so there is no stencil library \
             for the host to run",
            host.slug()
        ));
    }
    // The library is there and the entry point is not. No machine is in that
    // position today; it is the shape a fourth target would arrive in.
    Some(format!("stencil has no {} backend yet", host.triple()))
}

/// The decoded library for one target, once per process.
///
/// Decoding twenty-three thousand stencils is tens of milliseconds and the
/// whole claim of this backend is compile time, so it is paid once for a build
/// rather than once per codegen unit — a `buri build` at a hundred thousand
/// lines emits several hundred units in one process. It is also paid **per
/// target reached**, and a build reaches one, so a three-target toolchain
/// decodes no more than a one-target toolchain did.
fn load(t: abi::StencilTarget) -> Result<&'static library::Library, String> {
    static LIBS: [OnceLock<Result<library::Library, String>>; abi::StencilTarget::ALL.len()] =
        [OnceLock::new(), OnceLock::new(), OnceLock::new()];
    // `StencilTarget::ALL` is the order, and `blob` is matched to it by the
    // assertion in `the_libraries_are_indexed_by_their_own_order`.
    let i = abi::StencilTarget::ALL.iter().position(|x| *x == t).unwrap_or(0);
    let slot = LIBS.get(i).ok_or("stencil: a target with no library slot")?;
    match slot.get_or_init(|| library::Library::decode(blob(t).0)) {
        Ok(l) => Ok(l),
        Err(e) => Err(e.clone()),
    }
}

/// The copy-and-patch backend.
#[derive(Default)]
pub struct Stencil;

impl Backend for Stencil {
    fn name(&self) -> &'static str {
        "stencil"
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
    ///
    /// **Not computed here.** Hashing four megabytes at run time cost about
    /// 22 ms of *every* `buri` invocation that reached this backend, and
    /// memoising it removed only the repeats — a `buri` invocation is a
    /// process, so the first one is not a repeat. `cli/build.rs` now writes the
    /// digest beside the blob when the toolchain is built and
    /// the baked digests are those strings, which are the same strings this
    /// used to compute (the script and `build::cache::hash_bytes` are one
    /// source file) and therefore invalidate no cache.
    ///
    /// That 22 ms was the whole of the remaining compile-side gap against the
    /// backend this one replaced: it is 22 ms of a 25 ms no-op build, so it
    /// decided the no-op, the incremental and the live-edit cells on its own.
    ///
    /// **All three digests, not the one this build will use.** `identity` is
    /// not told the target — `Backend::identity(&self)` takes none — so the
    /// only honest answer is the whole toolchain's stencil identity. It costs
    /// a conservative invalidation: adding or rebuilding *any* target's library
    /// invalidates every cached object rather than that target's. That is the
    /// right way round. The alternative, naming only the host's, would let a
    /// toolchain whose `linux-arm64` stencils had changed serve a cached
    /// `linux-arm64` object built from the old ones, which is a wrong artifact
    /// rather than a slow build.
    fn identity(&self) -> String {
        let mut id = String::from("stencil");
        for t in abi::StencilTarget::ALL {
            id.push(' ');
            id.push_str(blob(t).1);
        }
        id
    }

    /// Which intrinsics this backend has no body for, asked of the program up
    /// front so that a program using one is told before a second is spent on
    /// it.
    ///
    /// Two sources, not one. This backend's own surface, and — since the
    /// runtime archive grew a `net` feature — the keys *no* backend can answer
    /// on a toolchain whose archive was built without it
    /// ([`super::networking_gap`]). The second is empty on an ordinary
    /// toolchain and it is not this backend's business what the sentence is:
    /// `super::gap_refusals` sorts the two apart where the diagnostic is built.
    fn missing_intrinsics(&self, program: &Program, _tables: &Tables) -> Vec<String> {
        let mut missing: Vec<String> = program
            .funcs
            .iter()
            .filter_map(|f| f.intrinsic_key())
            .filter(|k| !emit::implemented(k))
            .map(String::from)
            .collect();
        missing.extend(super::networking_gap(program));
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
    /// shape `llvm/mod.rs::emit_units` has.
    fn emit_units(
        &mut self,
        program: &Program,
        tables: &Tables,
        opts: &Options<'_>,
        units: Units<'_>,
    ) -> Result<Vec<Emitted>, Diagnostics> {
        let target = match supported(opts.target) {
            Ok(t) => t,
            Err(e) => return Err(one(e)),
        };
        let lib = match load(target) {
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

        // Whole-program, and therefore computed here rather than inside the
        // loop. A frame-threaded call site needs its *callee's* frame layout,
        // and a callee is as often in another unit as in this one, so the table
        // is the program's and not a unit's; computing it per unit made the
        // emitter O(units × program) — 2,378 ms of a 3,184 ms emission at 104k
        // lines and 367 units. This is the same place `lower::run` above is,
        // for the same reason.
        let frames = jit::frame_sigs(&lowered, tables);

        let whole = Whole {
            lib,
            program: &lowered,
            tables,
            frames: &frames,
            root: &root,
            target,
        };
        let mut out = Vec::new();
        let mut errors: Vec<String> = Vec::new();
        for (index, name) in lowered.units.iter().enumerate() {
            let unit = u32::try_from(index).unwrap_or(0);
            if !units.wants(unit) {
                continue;
            }
            let mine = members.get(index).unwrap_or(&empty);
            match compile_unit(&whole, name, mine) {
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

/// The stencil library a [`Target`] resolves to, or the sentence saying why
/// there is none.
///
/// Three things can be missing and each gets its own sentence, because a user
/// who asked for a Linux artifact on a machine whose clang cannot cross-compile
/// is in a different position from one who asked for a target this backend has
/// not finished.
///
/// Public because `backend::select` asks it before answering: this is the one
/// place the per-target answer is derived from `available_for` and
/// `asm::AVAILABLE_X86_64`, so a target that lights up here lights up in
/// selection with no second list to edit.
pub fn supported(target: Target) -> Result<abi::StencilTarget, String> {
    let arch = target.arch.unwrap_or(if cfg!(target_arch = "aarch64") {
        Arch::Arm64
    } else {
        Arch::X86_64
    });
    let stencils = match (target.platform, arch) {
        (Platform::Macos, Arch::Arm64) => abi::StencilTarget::MacosArm64,
        (Platform::Linux, Arch::Arm64) => abi::StencilTarget::LinuxArm64,
        (Platform::Linux, Arch::X86_64) => abi::StencilTarget::LinuxX86_64,
        // x86-64 macOS is the one combination no library is built for: the
        // stencils would be x86-64 in a Mach-O, and nothing this repository
        // runs on or ships to is that. `design/native/CODEGEN-STENCIL.md` §9.
        (p, a) => {
            return Err(format!(
                "the stencil backend has no stencil library for {}-{}",
                p.slug(),
                a.slug()
            ))
        }
    };
    if !available_for(stencils) {
        return Err(format!(
            "this toolchain was built without {} stencils, so the stencil backend cannot \
             emit for that target (it needs a C compiler that can produce {} objects)",
            stencils.slug(),
            stencils.triple()
        ));
    }
    // A target whose stencils exist but whose `main` does not can emit unit
    // objects and not a program, which is a different thing to be told than a
    // missing library. No target is in that position today —
    // [`asm::AVAILABLE_X86_64`] — and the sentence stays because the condition
    // is what a fourth target would arrive in.
    if !stencils.is_arm64() && !asm::AVAILABLE_X86_64 {
        return Err(format!(
            "the stencil backend has {} stencils but no hand-written entry point for that \
             machine, so it can emit unit objects for the target but not a program \
             (design/native/CODEGEN-STENCIL.md §10.3)",
            stencils.slug()
        ));
    }
    Ok(stencils)
}

/// The symbol a C runtime starts a program at, and the one the two `asm.rs`
/// shims are emitted under.
///
/// Named here rather than spelled at the one use because a *second* named shim
/// arrived in the same list ([`carrier::MAIN_ENTRY`]) and a list with one
/// literal and one constant in it reads as though the two were different
/// kinds of thing.
const ENTRY_SYMBOL: &str = "main";

/// The C door into one root, or `None` for a root this backend has no record
/// shape for yet.
///
/// **Nullary only, deliberately.** `carrier.rs`'s `state` is passed and not
/// read, so a root that took arguments would have to read them out of a record
/// whose layout nothing has decided — and deciding it here, with no call site
/// to fill it, is exactly the guess `cli/runtime/rt.rs` §1 refuses to make
/// about the task table. Every Buri root *is* nullary (`main(): Result<(),
/// Str>` and a `test` block both), so the `None` arm is unreachable today and
/// is here so that the day it is reachable is a missing symbol rather than a
/// door that reads uninitialised bytes.
fn carrier_door(
    target: abi::StencilTarget,
    frames: &[jit::FrameSig],
    idx: usize,
    sym: &str,
) -> Option<asm::Shim> {
    let frame = frames.get(idx)?;
    if !frame.params.is_empty() {
        return None;
    }
    Some(asm::carrier_entry(target, sym, frame.ret_size))
}

/// One codegen unit, from IR to object bytes.
/// Everything one emission's units share, computed once above the loop.
///
/// A struct rather than six more parameters, and the grouping is the real one:
/// every field here is a property of the *program and the target*, and none is
/// a property of the unit. `name` and `members` are the unit's, and they stay
/// arguments.
struct Whole<'a> {
    lib: &'a library::Library,
    program: &'a ir::Program,
    tables: &'a Tables,
    frames: &'a [jit::FrameSig],
    root: &'a Root,
    target: abi::StencilTarget,
}

fn compile_unit(
    w: &Whole<'_>,
    name: &str,
    members: &[usize],
) -> Result<Emitted, Vec<String>> {
    let Whole { lib, program, tables, frames, root, target } = *w;
    let mut j = jit::Jit::new(lib, tables, frames, target);
    j.compile_unit(program, members);

    // A refused IR shape is a diagnostic naming the shape, never an artifact
    // that aborts when it reaches it. The emission is finished first so that
    // one build reports every refusal rather than the first.
    let refused = j.reasons();
    if !refused.is_empty() {
        return Err(refused
            .iter()
            .map(|r| format!("the stencil backend cannot compile {r} yet"))
            .collect());
    }

    let entries: Vec<u64> = members.iter().map(|i| j.entry_of(*i)).collect();
    let emitted = std::mem::take(&mut j.region).finish();
    let mut code = emitted.code;

    // The entry point goes in the unit that owns `main`, so that a program is
    // one `_start`-adjacent symbol and the other units are libraries. A test
    // binary has no `main` to own it, so it goes in the unit that owns the
    // *first* test — the same rule `llvm/mod.rs` applies to the root that
    // exists.
    //
    // **The carrier doors ride with it**, in the same unit and for the same
    // reason: `main` and a door are the two ways into this program's Buri
    // code, they name the same roots, and a unit that has one and not the
    // other would leave a symbol nothing defines. `carrier.rs` is the
    // signature; `asm::carrier_entry` is this backend's half of it.
    let mut shims: Vec<(String, asm::Shim)> = Vec::new();
    match root {
        Root::Main(idx) if members.contains(idx) => {
            let sym = jit::symbol_of(program, u32::try_from(*idx).unwrap_or(0));
            shims.push((
                String::from(ENTRY_SYMBOL),
                asm::program_entry(target, &sym, main_result(program, tables, *idx)),
            ));
            if let Some(door) = carrier_door(target, frames, *idx, &sym) {
                shims.push((String::from(carrier::MAIN_ENTRY), door));
            }
        }
        Root::Tests(tests) if tests.first().is_some_and(|i| members.contains(i)) => {
            let names: Vec<String> = tests
                .iter()
                .map(|i| jit::symbol_of(program, u32::try_from(*i).unwrap_or(0)))
                .collect();
            shims.push((String::from(ENTRY_SYMBOL), asm::test_entry(target, &names)));
            for (i, (t, sym)) in tests.iter().zip(&names).enumerate() {
                if let Some(door) = carrier_door(target, frames, *t, sym) {
                    shims.push((carrier::test_entry(i), door));
                }
            }
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

    // The functions the unit generated for itself (`glue.rs`), under **local**
    // names: a unit that drops a `[Str]` has its own glue and no two units
    // collide, which is what a local symbol is for.
    for (name, at) in j.helper_symbols() {
        let ix = want(&mut symbols, &mut index, &name);
        if let Some(s) = symbols.get_mut(ix) {
            s.defined = Some(object::Definition { section: 0, offset: at });
            s.global = false;
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
                region::RelocKind::Rel32 => object::RelKind::Rel32,
                region::RelocKind::Pc32 => object::RelKind::Pc32,
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

    // The three sections a unit has, under whichever container's names. Only
    // the spelling differs: a Mach-O section is `segment,section` and an ELF
    // one is a name and a flag word, and `elf.rs` reads the flags off
    // `attributes` and `zerofill` exactly as `object.rs` does.
    let (text, text_seg) = if target.is_elf() { (".text", "") } else { ("__text", "__TEXT") };
    let (const_, const_seg) =
        if target.is_elf() { (".rodata", "") } else { ("__const", "__DATA_CONST") };
    let (bss, bss_seg) = if target.is_elf() { (".bss", "") } else { ("__bss", "__DATA") };
    let mut sections = vec![
        object::Section {
            name: text,
            segment: text_seg,
            align: region::CODE_ALIGN,
            attributes: object::CODE_ATTRIBUTES,
            zerofill: 0,
            data: Vec::new(),
        },
        object::Section {
            name: const_,
            segment: const_seg,
            align: if target.is_arm64() {
                region::POOL_ALIGN
            } else {
                region::POOL_ALIGN_X86_64
            },
            attributes: 0,
            zerofill: 0,
            data: emitted.pool,
        },
    ];

    if !shims.is_empty() {
        // The shims go after the bodies rather than before them, so that the
        // offsets `resolve` already patched stay where they are.
        for (name, shim) in shims {
            while !code.len().is_multiple_of(16) {
                code.push(0);
            }
            let at = code.len() as u64;
            code.extend_from_slice(&shim.bytes);
            let ix = want(&mut symbols, &mut index, &name);
            if let Some(s) = symbols.get_mut(ix) {
                s.defined = Some(object::Definition { section: 0, offset: at });
            }
            for (off, kind, target, addend) in shim.relocs {
                let sym = want(&mut symbols, &mut index, &name_of(&target));
                out.push(object::Reloc {
                    section: CODE,
                    offset: at.saturating_add(off),
                    kind,
                    symbol: sym,
                    addend,
                });
            }
        }
        // The Buri stack, in its own zero-filled section so that it costs no
        // bytes in the object or in the artifact.
        sections.push(object::Section {
            name: bss,
            segment: bss_seg,
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
    let bytes = if target.is_elf() {
        elf::write(target, &sections, &symbols, &out)
    } else {
        object::write(&sections, &symbols, &out)
    }
    .map_err(|e| vec![e])?;

    // The `codegen` key is `H(the unit's lowered IR)` (ARCHITECTURE.md §6.2),
    // and `ir::Program`'s `Display` is a faithful, total and deterministic
    // function of the IR with no hash order in it — the same key every backend
    // computes, from the same text.
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
const POOL_ANCHOR: &str = "buri$stencil$pool";

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
/// success unconditionally, which is `llvm/emit.rs::entry_point`'s rule too.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_backend_is_named_in_every_key_it_produces() {
        assert_eq!(Stencil.name(), "stencil");
    }

    /// The identity has to move when the stencils do, because the stencils are
    /// most of what the emitted bytes are.
    #[test]
    fn the_identity_is_the_librarys_hash() {
        let id = Stencil.identity();
        assert!(id.starts_with("stencil "), "{id}");
        // One digest per target, space-separated after the name.
        assert_eq!(id.len(), "stencil".len() + abi::StencilTarget::ALL.len() * (1 + 64));
    }

    /// `library` indexes `LIBS` by a target's position in
    /// `StencilTarget::ALL`, so the two must not drift: a slot that held another
    /// target's library would serve arm64 stencils to an x86-64 build.
    #[test]
    fn the_libraries_are_indexed_by_their_own_order() {
        for (i, t) in abi::StencilTarget::ALL.iter().enumerate() {
            assert_eq!(abi::StencilTarget::ALL.iter().position(|x| x == t), Some(i));
        }
        // And no two targets share a blob.
        for a in abi::StencilTarget::ALL {
            for b in abi::StencilTarget::ALL {
                if a != b && available_for(a) {
                    assert_ne!(blob(a).1, blob(b).1, "{} and {}", a.slug(), b.slug());
                }
            }
        }
    }

    /// A host with a library must have one that decodes; a host without must
    /// say so rather than fail to build.
    #[test]
    fn the_library_matches_its_availability() {
        for t in abi::StencilTarget::ALL {
            assert_eq!(available_for(t), load(t).is_ok(), "{}", t.slug());
        }
    }

    /// `AVAILABLE` is "this host can build a runnable program", and the host's
    /// own library existing is necessary but not sufficient: an entry point for
    /// the machine is the other half, which is what the assertion below reads.
    #[test]
    fn availability_on_this_host_implies_a_library_and_an_entry_point() {
        if AVAILABLE {
            let host = host_stencil_target().expect("an available host has a stencil target");
            assert!(available_for(host), "{} is available with no library", host.slug());
            assert!(host.is_arm64() || asm::AVAILABLE_X86_64);
        }
    }

    /// The sentence and the flag are one question, and a skip that printed
    /// nothing would be the silent pass the test suites are guarding against.
    #[test]
    fn the_unavailable_reason_is_present_exactly_when_the_backend_is_not() {
        assert_eq!(AVAILABLE, unavailable_reason().is_none());
        if let Some(why) = unavailable_reason() {
            assert!(!why.is_empty());
        }
    }

    /// **Every slots-only runtime call has to arrive folded, or it is a
    /// pessimisation rather than an optimisation.**
    ///
    /// `rtcall::c_call_to` chooses the `crts` family on the belief that each of
    /// its argument holes becomes the `imm12` of one load. Unfolded, the same
    /// stencil materialises an offset into a register per argument and the call
    /// is *longer* than the array-passing form it replaced — and nothing at run
    /// time would say so, because both compute the same answer. `Jit::emit`
    /// silently takes the unfolded twin when there is no folded one, so the
    /// check belongs here, against the library the toolchain actually built.
    #[test]
    fn every_slots_runtime_call_has_a_folded_twin_that_is_shorter() {
        let Ok(lib) = load(abi::StencilTarget::MacosArm64) else { return };
        let mut checked = 0;
        for ni in 0..=abi::MAX_INT_ARGS {
            for nf in 0..=abi::MAX_FLOAT_ARGS {
                for ret in ["v", "i", "w", "d", "b", "h", "u"] {
                    let key = format!("crts/{ni}/{nf}/{ret}");
                    let plain = lib.get(&key).unwrap_or_else(|| panic!("no {key}"));
                    checked += 1;
                    // The one shape with nothing to fold: no arguments and no
                    // result, so no offset appears in it at all.
                    let holes = ni + nf + usize::from(ret != "v");
                    if holes == 0 {
                        continue;
                    }
                    let folded = lib
                        .get(&format!("{key}+fold"))
                        .unwrap_or_else(|| panic!("{key} has no folded twin"));
                    assert!(
                        folded.code.len() < plain.code.len(),
                        "{key}: folded {} is not shorter than unfolded {}",
                        folded.code.len(),
                        plain.code.len()
                    );
                    // One `imm12` site per argument, plus the destination where
                    // the shape has one: a hole left with an `adrp`/`add` pair
                    // is an argument the fold did not reach.
                    let sites: usize = folded.holes.iter().map(|h| h.lo12.len()).sum();
                    let pairs: usize = folded.holes.iter().map(|h| h.pairs.len()).sum();
                    assert_eq!(
                        (sites, pairs),
                        (holes, 0),
                        "{key}: {ni} + {nf} arguments, {sites} folded and {pairs} not"
                    );
                }
            }
        }
        assert_eq!(checked, (abi::MAX_INT_ARGS + 1) * (abi::MAX_FLOAT_ARGS + 1) * 7);
    }
    /// A stencil key without its fold suffixes: the operation and operand
    /// shape the emitter looks up, before `Jit::emit` narrows to whichever
    /// twin's fields the operands fit.
    #[cfg(test)]
    fn base_keys(lib: &library::Library) -> std::collections::BTreeSet<String> {
        lib.index.keys().map(|k| k.split('+').next().unwrap_or(k).to_string()).collect()
    }

    /// **The two arm64 libraries must cover exactly the same operations.**
    ///
    /// This is the check that makes `linux-arm64` a container port rather than
    /// a second backend. The *bytes* are not the same and are not expected to
    /// be — Darwin's arm64 ABI mandates a frame record, so a stencil that makes
    /// a call opens with `stp x29, x30, [sp, #-16]!` where Linux's opens with
    /// `str x30, [sp, #-16]!`, and the two drivers schedule differently — but
    /// every key the emitter can ask for has to exist on both, or a program
    /// that compiles for macOS would be refused for Linux with no reason a user
    /// could act on.
    ///
    /// Fold *twins* are deliberately not compared: whether `+ifold` applies is
    /// a property of the instructions clang chose, so the two libraries differ
    /// by a few dozen twins in both directions and `Jit::emit` already falls
    /// back to the unfolded key when a twin is absent.
    #[test]
    fn the_two_arm64_libraries_cover_the_same_operations() {
        let (Ok(m), Ok(l)) =
            (load(abi::StencilTarget::MacosArm64), load(abi::StencilTarget::LinuxArm64))
        else {
            return;
        };
        let (bm, bl) = (base_keys(m), base_keys(l));
        let only_macos: Vec<&String> = bm.difference(&bl).collect();
        let only_linux: Vec<&String> = bl.difference(&bm).collect();
        assert!(
            only_macos.is_empty() && only_linux.is_empty(),
            "only macos-arm64: {only_macos:?}; only linux-arm64: {only_linux:?}"
        );
        assert!(bm.len() > 10_000, "{} base keys is not a library", bm.len());
    }

    /// **The three libraries cover exactly the same operations.**
    ///
    /// x86-64 used to drop thirty keys of thirteen thousand nine hundred, in
    /// three families where AArch64 has an instruction and x86-64 has a
    /// constant clang spilled into `.rodata`:
    ///
    ///  * `un/neg/f32`, `un/neg/f64` — `fneg` against an `xorps` with a sign
    ///    mask;
    ///  * `cvt/u2f` — `ucvtf` against the two-bias `unsigned long long` to
    ///    `double` sequence;
    ///  * `chk/div/i128` — the 128-bit divide's zero check, which spills its
    ///    comparison constant.
    ///
    /// The spilled bytes now travel with the stencil and the emitter copies
    /// them into the unit's own constant pool (`library::ConstRef`), so the
    /// list is empty and this asserts that it stays empty. A drop that
    /// reappeared would be a silent loss of coverage on the one target that has
    /// no second backend behind it.
    #[test]
    fn the_x86_64_library_covers_what_the_arm64_ones_do() {
        let (Ok(m), Ok(x)) =
            (load(abi::StencilTarget::MacosArm64), load(abi::StencilTarget::LinuxX86_64))
        else {
            return;
        };
        let missing: Vec<String> = base_keys(m).difference(&base_keys(x)).cloned().collect();
        assert!(missing.is_empty(), "x86-64 has no {missing:?}");
    }

    /// And the spilled constants are actually carried, rather than the three
    /// families having quietly stopped needing them: a `cvt/u2f` with no
    /// `ConstRef` would be a stencil reading `.rodata` that is not there.
    #[test]
    fn the_spilled_constant_families_carry_their_bytes() {
        let Ok(x) = load(abi::StencilTarget::LinuxX86_64) else { return };
        let s = x.get("cvt/u2f").unwrap_or_else(|| panic!("no cvt/u2f"));
        assert!(!s.const_refs.is_empty(), "cvt/u2f reads no spilled constant");
        let carriers = x.stencils.iter().filter(|s| !s.const_refs.is_empty()).count();
        assert!(carriers >= 4, "only {carriers} stencils carry spilled constants");
        for s in &x.stencils {
            assert_eq!(s.consts.is_empty(), s.const_refs.is_empty(), "{}", s.name);
            assert!(s.consts_align <= 16, "{} asks for {} alignment", s.name, s.consts_align);
            for c in &s.const_refs {
                assert!(c.insn_end > c.field, "{}: a field past its instruction", s.name);
                assert!(c.insn_end - c.field <= 8, "{}: an eight-byte trailer", s.name);
                assert!((c.at as usize) < s.consts.len(), "{}: a reference past the bytes", s.name);
                assert!((c.insn_end as usize) <= s.code.len(), "{}: a field outside the body", s.name);
            }
        }
        // No AArch64 stencil has any: `extract.rs` refuses an object that
        // spilled anything at all.
        let Ok(m) = load(abi::StencilTarget::MacosArm64) else { return };
        assert!(m.stencils.iter().all(|s| s.const_refs.is_empty() && s.consts.is_empty()));
    }

    /// Every key in the x86-64 library has exactly one entry — no fold twins —
    /// which is `x86.rs`'s claim that three of the four AArch64 folds are
    /// answered by the instruction set. A `+fold` key here would mean the
    /// arm64 rewrites had been let loose on x86-64 bytes.
    #[test]
    fn the_x86_64_library_has_no_folded_twins() {
        let Ok(x) = load(abi::StencilTarget::LinuxX86_64) else { return };
        assert!(
            !x.index.keys().any(|k| k.contains('+')),
            "the x86-64 library has fold twins, which nothing produces"
        );
        assert_eq!(x.index.len(), base_keys(x).len());
    }
}
