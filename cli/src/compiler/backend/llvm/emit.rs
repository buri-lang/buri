//! One codegen unit, as an LLVM module.
//!
//! CODEGEN-LLVM.md §2: direct SSA, and no `alloca`. Block parameters become
//! phis mechanically ([`Function::phis`]); an aggregate is built with
//! `insertvalue` and taken apart with `extractvalue`, both register
//! operations; and nothing here emits a slot for a local, a parameter, a
//! temporary, a match binding or a loop variable.
//!
//! Two `alloca`s exist in the whole backend and both are §2.3's case — a
//! genuine aggregate in memory whose fields are accessed by `getelementptr` —
//! rather than memory standing in for an SSA value:
//!
//!  * the two out-parameter buffers `buri_rt_i128_divmod` writes through
//!    ([`Function::divmod_buffers`]), because the runtime's C ABI takes
//!    out-pointers (`cli/runtime/lib.rs` §2 rule 2) and a pointer needs
//!    something to point at;
//!  * the same shape, generalized, for every runtime entry whose result is an
//!    aggregate or a sum and for every generic argument that has to be spilled
//!    ([`Unit::scratch`], §2 rules 2, 3 and 4). Each is one entry-block
//!    `alloca` whose loads are in the same basic block as the call that filled
//!    it, so SROA takes them back to registers wherever the callee did not
//!    escape the pointer;
//!  * nothing else. The escape-analysed stack aggregate §2.3 is really about
//!    (MEMORY.md §5.2) is not emitted, because `middle::rc` does not compute
//!    the escape analysis yet — every aggregate that needs memory is a heap
//!    block from `buri_rt_alloc`, which is the conservative direction.
//!
//! # Closures, and the environment block
//!
//! VALUE-MODEL.md §7 is `{ code, env }`, and this keeps that with the two
//! additions the flattened calling convention forces, which every native
//! backend here shares so that one artifact's closures have one shape whichever
//! built them:
//!
//!  * **`code` is always a thunk.** A closure's callee cannot be the lifted
//!    lambda itself, because `middle::closures` gives the lambda its captures
//!    as an aggregate *first parameter* and an aggregate parameter is its
//!    **leaves** (§5.1) — which a call site holding only a pointer cannot
//!    produce. So `code` points at a generated function that takes the
//!    environment as a pointer, loads those leaves out of it, and forwards. The
//!    same shape covers a capture-free lambda, which `middle::closures` has
//!    already turned into a plain `FnRef`: it still gets a thunk, because which
//!    of the two a closure value holds is a run-time fact.
//!  * **The environment block leads with its own drop glue.** `Ty::Fn` records
//!    what a function takes and answers and not what it captured, so a `decref`
//!    of a closure has no type from which to derive the function that releases
//!    the environment's contents. The block holds that function pointer in its
//!    first word and the environment record at [`ENV_FIELDS`], and one
//!    universal glue reads it. Eight bytes per closure, against a closure that
//!    could not be freed.
//!
//! # Blocks are emitted in reverse postorder
//!
//! Not in `blocks` order. Dominance says a value's defining block dominates
//! every use, and a dominator precedes its dominatee in reverse postorder — so
//! RPO is the order in which "the operand is already an LLVM value" is a
//! theorem rather than an observation about what `lower` happened to emit.
//! Phis are exempt by construction: every one is created empty before any body
//! is filled, and the incoming values are added last.

use inkwell::basic_block::BasicBlock;
use inkwell::builder::Builder;
use inkwell::context::Context;
use inkwell::module::{Linkage, Module};
use inkwell::types::{BasicMetadataTypeEnum, BasicType, BasicTypeEnum, FunctionType, IntType};
use inkwell::values::{
    BasicMetadataValueEnum, BasicValue, BasicValueEnum, FloatValue, FunctionValue, InstructionValue,
    IntValue, PointerValue,
};
use inkwell::{FloatPredicate, IntPredicate};

use crate::compiler::backend::intrinsic_keys::{
    bits_op, derive_key, list_call, list_closure_key, step_call, Step,
};
use crate::compiler::backend::carrier;
use crate::compiler::backend::Profile;
use crate::compiler::middle::ir;
use crate::compiler::middle::rc::{self, Counted as _};
use crate::compiler::middle::layout::{
    self, EnumRepr, Repr as LayoutRepr, Scalar, CAP_MASK, CAP_SHARED_FLAG, HEADER_CAP_OFFSET,
    HEADER_RC_OFFSET, IMMORTAL, STR_ASCII_FLAG, STR_LEN_MASK,
};
use crate::compiler::semantics::builtins::conversion_is_exact;
use crate::compiler::semantics::types::{self as types, FuncIdx, Prim, Tables, Ty};
use crate::diagnostics::{Diagnostic, Diagnostics, Span};
use crate::hash::Map;

use super::attrs::{self, Observed};
use super::repr::{self, Counted, Glue, Reprs, Site, Slot, SlotTy};

/// One counted field of one enum variant: its type, its absolute offset, and
/// whether it is behind the pointer a recursive type's field gets
/// (VALUE-MODEL.md §5.2).
///
/// Named because [`Unit::tagged_rc`] groups these by variant and the nested
/// shape is past what a reader should have to parse in a `let`.
type VariantField = (Ty, u32, bool);
use super::runtime;

/// A heap payload is 16-byte aligned, because the header is 16 bytes and sits
/// immediately before it (VALUE-MODEL.md §2). This is the number `align` on a
/// pointer parameter and on a pointer return reports.
const HEAP_ALIGN: u32 = 16;

/// The environment record starts one word into its block; the word before it is
/// the block's own drop glue. See this file's header, "Closures".
const ENV_FIELDS: u32 = 8;

/// One module under construction.
pub struct Unit<'ctx, 'a> {
    ctx: &'ctx Context,
    pub module: Module<'ctx>,
    builder: Builder<'ctx>,
    program: &'a ir::Program,
    /// The checker's tables, kept beside [`Reprs`] rather than reached through
    /// it: a structural operation and a `derivePrim*` intrinsic both arrive
    /// carrying a type and nothing else, and `Tables::as_prim` is the only way
    /// to ask which primitive that type is.
    tables: &'a Tables,
    reprs: Reprs<'a>,
    profile: Profile,
    /// What emission will find in each function, computed once for the whole
    /// program — see [`observe`].
    ///
    /// Borrowed rather than owned, because "once for the whole program" is what
    /// it has to be: it is a fixpoint over every body, and computing it inside
    /// each unit made the emission quadratic (`design/PERFORMANCE.md` §6.4's
    /// first finding).
    observed: &'a [Observed],
    /// The LLVM function for each `FuncIdx`, declared lazily: a unit declares
    /// only what it calls, so a module is not the whole program's symbol table.
    ///
    /// Keyed rather than indexed for the same reason the table is lazy: a slot
    /// per function in the program is an allocation and a memset per unit for a
    /// row that holds the unit's own functions and its handful of imports.
    funcs: Map<usize, FunctionValue<'ctx>>,
    /// `buri_rt_*` declarations, by symbol.
    runtime: Map<String, FunctionValue<'ctx>>,
    /// Interned string literal bodies, so `"hello"` twice is one global.
    literals: Map<Vec<u8>, PointerValue<'ctx>>,
    /// The generated helpers, by what they are generated *for*. One function
    /// per key per unit: every one of them is reachable through a function
    /// pointer, so a fresh copy per call site would be `n` copies of the same
    /// three instructions and `n` symbols for the linker to keep.
    ///
    /// A helper is inserted here **before** its body is built, which is what
    /// makes the walk over a recursive type terminate: `Release` for a `Tree`
    /// asks for `Release` for a `Tree`, and the second ask finds the first.
    thunks: Map<(u32, bool), FunctionValue<'ctx>>,
    releases: Map<Ty, FunctionValue<'ctx>>,
    release_elems: Map<Ty, FunctionValue<'ctx>>,
    retains: Map<Ty, FunctionValue<'ctx>>,
    /// The C-ABI entry thunks of [`Job::Entry`], one per step signature.
    entries: Map<(Vec<Ty>, Ty, Option<usize>), FunctionValue<'ctx>>,
    env_glue: Option<FunctionValue<'ctx>>,
    /// Helper bodies still to be built. Drained by [`Unit::finish`] rather than
    /// built where they are asked for, because a helper is asked for in the
    /// middle of another function's body and the builder is positioned there.
    pending: Vec<Job<'ctx>>,
    helpers: usize,
    /// The classifier `middle::rc` decided its own operations with, where the
    /// caller had the program `rc::run` was handed — see [`Unit::rc_counted`].
    rc: Option<std::rc::Rc<std::cell::RefCell<rc::Syntactic>>>,
    pub diags: Diagnostics,
}

impl<'ctx, 'a> Unit<'ctx, 'a> {
    /// `observed` is [`observe`] over the same program and profile, and
    /// `cycles` is [`layout::Cycles`] over the same tables. Both are taken once
    /// for the whole emission and handed to every unit.
    pub fn new(
        ctx: &'ctx Context,
        program: &'a ir::Program,
        tables: &'a Tables,
        name: &str,
        profile: Profile,
        observed: &'a [Observed],
        cycles: std::rc::Rc<layout::Cycles>,
    ) -> Unit<'ctx, 'a> {
        Unit {
            ctx,
            module: ctx.create_module(name),
            builder: ctx.create_builder(),
            program,
            tables,
            reprs: Reprs::new(tables, cycles),
            profile,
            observed,
            funcs: Map::default(),
            runtime: Map::default(),
            literals: Map::default(),
            thunks: Map::default(),
            releases: Map::default(),
            release_elems: Map::default(),
            retains: Map::default(),
            entries: Map::default(),
            env_glue: None,
            pending: Vec::new(),
            helpers: 0,
            rc: None,
            diags: Diagnostics::new(),
        }
    }

    /// Hands this unit the classifier `middle::rc` ran with.
    pub fn use_rc_classifier(
        &mut self,
        classifier: std::rc::Rc<std::cell::RefCell<rc::Syntactic>>,
    ) {
        self.rc = Some(classifier);
    }

    /// Whether **rc** counts a type — not whether the layout table does.
    ///
    /// The two are different questions and the difference is a leak. `rc` runs
    /// on the program's own descriptors and bodies, so a `Str` is `Yes` only
    /// where some body writes a string literal, a template, or a primitive
    /// operation that names the type; everywhere else it is `Unknown`, which rc
    /// reads as "not counted" and for which it emits **nothing** — no `IncRef`
    /// at a call that owns its argument and no drop in the callee. Retaining on
    /// the layout's answer where rc retains on nothing adds one half of a pair
    /// nothing completes, which is one leaked block per element of a
    /// `list.map`. The same mistake in the debug backend of the day leaked 199
    /// of them over a 200-element `[Str]`.
    ///
    /// Without a classifier the layout answer stands in, and the direction is what
    /// makes that safe rather than merely convenient: rc's `Yes` is a subset of
    /// the layout's `counted_type` — every shape rc calls counted, the layout
    /// does — so the substitution can only ever retain *more*, in a program
    /// where rc already leaves the same blocks uncounted. It cannot release a
    /// count that was never taken. The one entry point without a classifier is
    /// [`super::emit_lowered`], which is handed an `ir::Program` and has no
    /// `monomorphize::Program` to build one from.
    ///
    /// The **thunk's** release does not ask this at all: `ir::Ownership::Borrow`
    /// is set only where rc's classifier answered `Yes` (`middle/rc.rs`'s
    /// `infer_ownership`, whose other arm is `Own` "by convention" for anything
    /// uncounted), so the ownership column already carries rc's answer exactly.
    fn rc_counted(&mut self, ty: &Ty) -> bool {
        if let Some(classifier) = self.rc.clone() {
            let answer = classifier.borrow_mut().counted(ty);
            return matches!(answer, rc::Answer::Yes);
        }
        self.reprs.counted_type(ty)
    }

    fn error(&mut self, span: Span, message: String, fix: &str) {
        self.diags.push(Diagnostic::error(span, message).with_fix(fix.to_string()));
    }

    // -----------------------------------------------------------------------
    // Signatures
    // -----------------------------------------------------------------------

    /// The flattened parameter and result slots of a signature.
    ///
    /// VALUE-MODEL.md §5.1: an aggregate parameter is its scalar leaves, in
    /// declaration order, and a zero-sized one is dropped entirely
    /// (VALUE-MODEL.md §8) — which is what makes `ctx` free on the platform
    /// context, whose every implementation is an empty struct.
    ///
    /// §5.1 also says "up to eight; beyond eight leaves the aggregate is passed
    /// by pointer". That cap is not applied. It describes a register budget,
    /// and LLVM already spills what does not fit; both sides of every Buri call
    /// are generated by this backend, into one artifact, so there is no second
    /// implementation for the choice to disagree with. Applying it would mean
    /// emitting the by-pointer path and the caller-owned buffer that goes with
    /// it, which is memory a value model that keeps values in registers does
    /// not otherwise need.
    fn slots_of_sig(&mut self, sig: &ir::Signature) -> (Vec<Slot>, Vec<Slot>) {
        let mut params = Vec::new();
        for t in &sig.params {
            params.extend(repr::ir_slots(&mut self.reprs, self.program, *t));
        }
        let mut rets = Vec::new();
        for t in &sig.rets {
            rets.extend(repr::ir_slots(&mut self.reprs, self.program, *t));
        }
        (params, rets)
    }

    fn fn_type(&self, params: &[Slot], rets: &[Slot]) -> FunctionType<'ctx> {
        let ps: Vec<BasicMetadataTypeEnum<'ctx>> =
            params.iter().map(|s| repr::slot_type(self.ctx, s.ty).into()).collect();
        match rets {
            [] => self.ctx.void_type().fn_type(&ps, false),
            _ => repr::register_type(self.ctx, rets).fn_type(&ps, false),
        }
    }

    /// Declares a Buri function, defining nothing.
    ///
    /// `fastcc` on both the function and every call site, set through
    /// [`attrs::set_convention`] so that the two cannot drift — a mismatch
    /// between them is a miscompile LLVM will not diagnose.
    pub fn declare(&mut self, idx: FuncIdx) -> Option<FunctionValue<'ctx>> {
        if let Some(f) = self.funcs.get(&idx.index()) {
            return Some(*f);
        }
        let func = self.program.funcs.get(idx.index())?;
        // An intrinsic is not a Buri function at all: the backend declares the
        // runtime import and defines nothing (VALUE-MODEL.md §10), so a call
        // to one is rewritten at the call site rather than routed through a
        // thunk.
        if func.intrinsic_key().is_some() {
            return None;
        }
        let (params, rets) = self.slots_of_sig(&func.sig);
        let ty = self.fn_type(&params, &rets);
        let f = self.module.add_function(&func.symbol, ty, None);
        attrs::set_convention(f, attrs::FAST);
        // The attributes a *declaration* carries are the ones a caller reasons
        // with, and they must be the same ones the definition carries — a
        // declaration in unit B claiming `memory(none)` for a function unit A
        // defined as allocating is a miscompile the linker will not notice.
        // [`observe`] is a whole-program answer for exactly that reason.
        let observed = self.observed.get(idx.index()).copied().unwrap_or_else(Observed::opaque);
        attrs::decorate(self.ctx, f, &func.facts, observed, &params, &rets, HEAP_ALIGN);
        self.funcs.insert(idx.index(), f);
        Some(f)
    }

    /// Declares a `buri_rt_*` entry with an explicit C signature.
    ///
    /// `ccc`, always: this is the single place in a Buri artifact where a
    /// platform ABI appears (`cli/runtime/lib.rs` §2). `nounwind` too — the
    /// runtime is Rust compiled with `panic = "abort"`, and it installs a panic
    /// hook that exits rather than unwinding.
    fn declare_rt(
        &mut self,
        symbol: &str,
        params: &[BasicMetadataTypeEnum<'ctx>],
        ret: Option<BasicTypeEnum<'ctx>>,
    ) -> FunctionValue<'ctx> {
        if let Some(f) = self.runtime.get(symbol) {
            return *f;
        }
        let ty = match ret {
            Some(t) => t.fn_type(params, false),
            None => self.ctx.void_type().fn_type(params, false),
        };
        let f = self.module.add_function(symbol, ty, Some(Linkage::External));
        self.platform_abi(f);
        self.runtime.insert(symbol.to_string(), f);
        f
    }

    /// Marks a function as carrying the **platform** calling convention:
    /// `ccc`, `nounwind`.
    ///
    /// Extracted from [`Unit::declare_rt`] rather than copied beside it,
    /// because that function's header is the claim *"this is the single place
    /// in a Buri artifact where a platform ABI appears"* and the carrier door
    /// ([`Unit::carrier_door`]) is the second thing that needs it. Two callers
    /// of one function keep the claim true; two copies of four lines would
    /// have made it false the moment one of them gained an attribute.
    ///
    /// `nounwind` on both sides for the same reason: the runtime is Rust
    /// compiled with `panic = "abort"` and it installs a hook that exits, and
    /// a Buri body has no unwinding of its own to let out.
    fn platform_abi(&self, f: FunctionValue<'ctx>) {
        attrs::set_convention(f, attrs::C);
        let kind = inkwell::attributes::Attribute::get_named_enum_kind_id("nounwind");
        if kind != 0 {
            f.add_attribute(
                inkwell::attributes::AttributeLoc::Function,
                self.ctx.create_enum_attribute(kind, 0),
            );
        }
    }

    /// The carrier entry signature as an LLVM function type, built from
    /// `backend/carrier.rs` rather than spelled here.
    ///
    /// Every word of that ABI is a pointer today; the `match` is what makes a
    /// widening a compile error in this function instead of a silently wrong
    /// type.
    fn carrier_fn_type(&self) -> inkwell::types::FunctionType<'ctx> {
        let params: Vec<BasicMetadataTypeEnum<'ctx>> = carrier::ENTRY
            .params
            .iter()
            .map(|w| match w {
                carrier::Word::Ptr => self.ptr_ty().into(),
            })
            .collect();
        match carrier::ENTRY.ret {
            None => self.ctx.void_type().fn_type(&params, false),
            Some(carrier::Word::Ptr) => self.ptr_ty().fn_type(&params, false),
        }
    }

    fn ptr_ty(&self) -> inkwell::types::PointerType<'ctx> {
        self.ctx.ptr_type(inkwell::AddressSpace::default())
    }

    fn rt_abort(&mut self) -> FunctionValue<'ctx> {
        let p = self.ptr_ty().into();
        let n = self.ctx.i64_type().into();
        let f = self.declare_rt(runtime::ABORT, &[p, n], None);
        attrs::mark_noreturn(self.ctx, f);
        f
    }

    fn rt_abort_named(&mut self, symbol: &str) -> FunctionValue<'ctx> {
        let f = self.declare_rt(symbol, &[], None);
        attrs::mark_noreturn(self.ctx, f);
        f
    }

    fn rt_alloc(&mut self) -> FunctionValue<'ctx> {
        let n = self.ctx.i64_type().into();
        let p = self.ptr_ty().as_basic_type_enum();
        self.declare_rt(runtime::ALLOC, &[n], Some(p))
    }

    fn rt_free(&mut self) -> FunctionValue<'ctx> {
        let p = self.ptr_ty().into();
        self.declare_rt(runtime::FREE, &[p], None)
    }

    // === G2 begin: the two shared arms =====================================

    fn rt_incref(&mut self) -> FunctionValue<'ctx> {
        let p = self.ptr_ty().into();
        self.declare_rt(runtime::INCREF, &[p], None)
    }

    fn rt_decref(&mut self) -> FunctionValue<'ctx> {
        let p = self.ptr_ty();
        self.declare_rt(runtime::DECREF, &[p.into(), p.into()], None)
    }

    // === G2 end ============================================================

    // -----------------------------------------------------------------------
    // Constants
    // -----------------------------------------------------------------------

    /// A private, unnamed-address, constant array of bytes, and a pointer to
    /// it. `unnamed_addr` is what lets the linker merge two literals with the
    /// same bytes across units.
    fn literal(&mut self, bytes: &[u8]) -> PointerValue<'ctx> {
        if let Some(p) = self.literals.get(bytes) {
            return *p;
        }
        let value = self.ctx.const_string(bytes, false);
        let global = self.module.add_global(
            value.get_type(),
            None,
            &format!("buri.str.{}", self.literals.len()),
        );
        global.set_initializer(&value);
        global.set_constant(true);
        global.set_unnamed_addr(true);
        global.set_linkage(Linkage::Private);
        global.set_alignment(1);
        let p = global.as_pointer_value();
        self.literals.insert(bytes.to_vec(), p);
        p
    }

    /// A `Str` literal's register value: `{ base, ptr, len }` with a **null
    /// base**, which is what makes it `IMMORTAL` and touches no allocator
    /// (VALUE-MODEL.md §3, `ir::Const::Str`).
    fn str_literal(&mut self, text: &str) -> BasicValueEnum<'ctx> {
        let bytes = text.as_bytes();
        let data = self.literal(bytes);
        let ascii = bytes.iter().all(|b| *b < 0x80);
        let len = (bytes.len() as u64) | if ascii { STR_ASCII_FLAG } else { 0 };
        let values = [
            self.ptr_ty().const_null().into(),
            data.into(),
            self.ctx.i64_type().const_int(len, false).into(),
        ];
        let slots = [
            Slot { offset: 0, ty: SlotTy::Scalar(Scalar::Ptr) },
            Slot { offset: 8, ty: SlotTy::Scalar(Scalar::Ptr) },
            Slot { offset: 16, ty: SlotTy::Scalar(Scalar::I64) },
        ];
        repr::assemble(self.ctx, &self.builder, &slots, &values)
    }
}

// ---------------------------------------------------------------------------
// One function
// ---------------------------------------------------------------------------

/// The state of one function's emission.
struct Function<'ctx> {
    value: FunctionValue<'ctx>,
    blocks: Vec<BasicBlock<'ctx>>,
    /// The LLVM block each IR block's *terminator* was emitted into.
    ///
    /// Not the same as `blocks[i]`: an instruction that branches — a division's
    /// zero test, a `decref`'s null and count tests, an `exitWith` — leaves the
    /// builder in a block it created, and the terminator lands there. A phi's
    /// incoming edge must name the block control actually came *from*, so this
    /// is what `fill_phis` reads. Naming `blocks[i]` instead is a "PHI node
    /// entries do not match predecessors" that the verifier catches and that
    /// nothing else would.
    ends: Vec<Option<BasicBlock<'ctx>>>,
    /// One entry per `ValueId`.
    values: Vec<Option<BasicValueEnum<'ctx>>>,
    /// The phi for each block parameter, so incoming edges can be added last.
    phis: Vec<Vec<inkwell::values::PhiValue<'ctx>>>,
    /// The entry block, for the two `alloca`s §2.3 allows.
    entry: BasicBlock<'ctx>,
    divmod: Option<(PointerValue<'ctx>, PointerValue<'ctx>)>,
    observed: Observed,
    /// [`argument_based`], once per body rather than once per `incref`.
    /// Empty for the generated helpers, which carry no attributes at all and
    /// so have nothing to be right or wrong about.
    based: Vec<bool>,
}

impl<'ctx, 'a> Unit<'ctx, 'a> {
    /// Defines one function's body.
    pub fn define(&mut self, idx: FuncIdx) {
        let Some(func) = self.program.funcs.get(idx.index()) else { return };
        let Some(code) = func.code() else { return };
        let Some(value) = self.declare(idx) else { return };
        value.set_linkage(Linkage::External);

        let entry = self.ctx.append_basic_block(value, "entry");
        let blocks: Vec<BasicBlock<'ctx>> = (0..code.blocks.len())
            .map(|i| self.ctx.append_basic_block(value, &format!("b{i}")))
            .collect();

        let mut state = Function {
            value,
            ends: (0..code.blocks.len()).map(|_| None).collect(),
            blocks,
            values: (0..code.values()).map(|_| None).collect(),
            phis: Vec::new(),
            entry,
            divmod: None,
            observed: Observed::clean(),
            based: argument_based(code),
        };

        // The entry block's parameters are the function's parameters
        // (`ir::Code`: "nothing branches to it"), so they are bound from the
        // LLVM arguments rather than from phis. Every other block's parameters
        // are phis, created empty here and filled once every predecessor's
        // terminator exists.
        self.builder.position_at_end(entry);
        self.bind_entry_params(&mut state, func, code);

        for (i, block) in code.blocks.iter().enumerate() {
            let mut row = Vec::new();
            if i > 0 {
                let Some(bb) = state.blocks.get(i) else { continue };
                self.builder.position_at_end(*bb);
                for p in &block.params {
                    let ty = repr::ir_type(self.ctx, &mut self.reprs, self.program, code.ty_of(*p));
                    if let Ok(phi) = self.builder.build_phi(ty, &format!("v{}", p.0)) {
                        if let Some(slot) = state.values.get_mut(p.index()) {
                            *slot = Some(phi.as_basic_value());
                        }
                        row.push(phi);
                    }
                }
            }
            state.phis.push(row);
        }

        // The entry `alloca`s go before the jump into b0, which is what makes
        // them eligible for SROA at all (§2.3).
        self.builder.position_at_end(entry);
        if let Some(b0) = state.blocks.first() {
            let _ = self.builder.build_unconditional_branch(*b0);
        }

        for i in reverse_postorder(code) {
            let Some(block) = code.blocks.get(i) else { continue };
            let Some(bb) = state.blocks.get(i).copied() else { continue };
            self.builder.position_at_end(bb);
            for inst in &block.insts {
                self.inst(&mut state, code, inst, func.span);
            }
            if let Some(slot) = state.ends.get_mut(i) {
                *slot = self.builder.get_insert_block();
            }
            self.terminator(&mut state, code, &block.term, func.span);
        }

        self.fill_phis(&mut state, code);

        // The attributes are rewritten from the same whole-program answer the
        // declaration used, so that a caller in another unit and the definition
        // in this one agree. `state.observed` is what this body actually
        // emitted and is asserted against the table rather than replacing it:
        // a body that found more than the pre-pass predicted is a bug in
        // `observe`, and the safe direction is to widen.
        let (params, rets) = self.slots_of_sig(&func.sig);
        let mut observed = self.observed.get(idx.index()).copied().unwrap_or_else(Observed::opaque);
        observed.join(state.observed);
        attrs::decorate(self.ctx, value, &func.facts, observed, &params, &rets, HEAP_ALIGN);
    }

    fn bind_entry_params(&mut self, state: &mut Function<'ctx>, func: &ir::Func, code: &ir::Code) {
        let Some(entry_block) = code.blocks.first() else { return };
        let mut next = 0u32;
        for p in &entry_block.params {
            let slots = repr::ir_slots(&mut self.reprs, self.program, code.ty_of(*p));
            let mut pieces = Vec::with_capacity(slots.len());
            for _ in &slots {
                if let Some(arg) = state.value.get_nth_param(next) {
                    pieces.push(arg);
                }
                next = next.saturating_add(1);
            }
            let value = repr::assemble(self.ctx, &self.builder, &slots, &pieces);
            if let Some(slot) = state.values.get_mut(p.index()) {
                *slot = Some(value);
            }
        }
        let _ = func;
    }

    fn fill_phis(&mut self, state: &mut Function<'ctx>, code: &ir::Code) {
        for (i, block) in code.blocks.iter().enumerate() {
            let Some(from) = state.ends.get(i).copied().flatten() else { continue };
            for target in block.term.targets() {
                let Some(row) = state.phis.get(target.block.index()) else { continue };
                for (phi, arg) in row.iter().zip(target.args.iter()) {
                    let Some(Some(value)) = state.values.get(arg.index()).copied() else { continue };
                    phi.add_incoming(&[(&value as &dyn BasicValue<'ctx>, from)]);
                }
            }
        }
    }

    fn get(&self, state: &Function<'ctx>, v: ir::ValueId) -> BasicValueEnum<'ctx> {
        match state.values.get(v.index()).copied().flatten() {
            Some(value) => value,
            // Unreachable in a verified program: `ir::verify_func` checks that
            // every use is dominated by its definition. A poison of the right
            // shape rather than a panic, because the lint set forbids one and
            // a value nothing reads is what an undominated use would have
            // been.
            None => self.ctx.i64_type().get_poison().into(),
        }
    }

    fn set(&self, state: &mut Function<'ctx>, v: ir::ValueId, value: BasicValueEnum<'ctx>) {
        if let Some(slot) = state.values.get_mut(v.index()) {
            *slot = Some(value);
        }
    }

    // -----------------------------------------------------------------------
    // Terminators — CODEGEN-LLVM.md §2.1's table
    // -----------------------------------------------------------------------

    fn terminator(
        &mut self,
        state: &mut Function<'ctx>,
        code: &ir::Code,
        term: &ir::Term,
        span: Span,
    ) {
        match term {
            ir::Term::Jump(t) => {
                if let Some(bb) = state.blocks.get(t.block.index()).copied() {
                    let _ = self.builder.build_unconditional_branch(bb);
                }
            }
            ir::Term::Branch { cond, then, else_ } => {
                let c = self.get(state, *cond);
                let (Some(t), Some(e)) = (
                    state.blocks.get(then.block.index()).copied(),
                    state.blocks.get(else_.block.index()).copied(),
                ) else {
                    return;
                };
                if let BasicValueEnum::IntValue(c) = c {
                    let _ = self.builder.build_conditional_branch(c, t, e);
                }
            }
            ir::Term::Switch { on, cases, default } => {
                self.switch(state, code, *on, cases, default.as_ref(), span)
            }
            ir::Term::Return(vs) => self.ret(state, code, vs),
            ir::Term::Unreachable => {
                let _ = self.builder.build_unreachable();
            }
        }
    }

    fn switch(
        &mut self,
        state: &mut Function<'ctx>,
        code: &ir::Code,
        on: ir::ValueId,
        cases: &[(u64, ir::Target)],
        default: Option<&ir::Target>,
        span: Span,
    ) {
        let BasicValueEnum::IntValue(subject) = self.get(state, on) else { return };
        let width = subject.get_type();
        let mut arms: Vec<(IntValue<'ctx>, BasicBlock<'ctx>)> = Vec::with_capacity(cases.len());
        for (key, target) in cases {
            if let Some(bb) = state.blocks.get(target.block.index()).copied() {
                arms.push((width.const_int(*key, false), bb));
            }
        }
        // `default` is `None` wherever the middle end proved the table total,
        // which for an enum is always. LLVM's `switch` still needs one, and
        // which one it gets is the whole of `Profile::defensive_aborts`: a
        // debug build keeps the backend's own belt to the checker's braces and
        // aborts, and a release build says `unreachable`, which is what lets
        // the jump table drop its bounds check.
        let fallback = match default {
            Some(t) => state.blocks.get(t.block.index()).copied(),
            None => Some(self.total_switch_default(state, span)),
        };
        let Some(fallback) = fallback else { return };
        let _ = self.builder.build_switch(subject, fallback, &arms);
        let _ = code;
    }

    fn total_switch_default(&mut self, state: &mut Function<'ctx>, span: Span) -> BasicBlock<'ctx> {
        let here = self.builder.get_insert_block();
        let bb = self.ctx.append_basic_block(state.value, "switch.total");
        self.builder.position_at_end(bb);
        if self.profile.defensive_aborts() {
            let f = self.rt_abort_named(runtime::ABORT_UNREACHABLE);
            if let Ok(call) = self.builder.build_call(f, &[], "") {
                attrs::noreturn_call(self.ctx, call);
            }
            state.observed.aborts = true;
        }
        let _ = self.builder.build_unreachable();
        if let Some(here) = here {
            self.builder.position_at_end(here);
        }
        let _ = span;
        bb
    }

    fn ret(&mut self, state: &mut Function<'ctx>, code: &ir::Code, vs: &[ir::ValueId]) {
        let mut slots = Vec::new();
        let mut pieces = Vec::new();
        for v in vs {
            let s = repr::ir_slots(&mut self.reprs, self.program, code.ty_of(*v));
            let value = self.get(state, *v);
            pieces.extend(repr::disassemble(&self.builder, &s, value));
            slots.extend(s);
        }
        match slots.len() {
            0 => {
                let _ = self.builder.build_return(None);
            }
            _ => {
                let value = repr::assemble(self.ctx, &self.builder, &slots, &pieces);
                let _ = self.builder.build_return(Some(&value as &dyn BasicValue<'ctx>));
            }
        }
    }

    // -----------------------------------------------------------------------
    // Instructions
    // -----------------------------------------------------------------------

    fn inst(&mut self, state: &mut Function<'ctx>, code: &ir::Code, inst: &ir::Inst, span: Span) {
        match inst {
            ir::Inst::Const { dest, value } => {
                let v = self.constant(code.ty_of(*dest), value);
                self.set(state, *dest, v);
            }
            ir::Inst::Unary { dest, op, prim, arg } => {
                let a = self.get(state, *arg);
                let v = self.unary(*op, *prim, a);
                self.set(state, *dest, v);
            }
            ir::Inst::Binary { dest, op, prim, lhs, rhs } => {
                let l = self.get(state, *lhs);
                let r = self.get(state, *rhs);
                let operand = code.ty_of(*lhs);
                let v = self.binary(state, *op, *prim, operand, l, r);
                self.set(state, *dest, v);
            }
            ir::Inst::MakeStruct { dest, fields } => {
                self.make_record(state, code, *dest, fields)
            }
            ir::Inst::MakeEnum { dest, variant, fields } => {
                self.make_enum(state, code, *dest, *variant, fields, span)
            }
            ir::Inst::MakeArray { dest, elems } => self.make_array(state, code, *dest, elems),
            ir::Inst::MakeClosure { dest, func, env } => {
                self.make_closure(state, code, *dest, *func, *env, span)
            }
            ir::Inst::GetField { dest, agg, index } => {
                self.get_field(state, code, *dest, *agg, *index as usize)
            }
            ir::Inst::GetPayload { dest, agg, variant, index } => {
                self.get_payload(state, code, *dest, *agg, *variant as usize, *index as usize)
            }
            ir::Inst::GetTag { dest, agg } => self.get_tag(state, code, *dest, *agg),
            ir::Inst::ArrayLen { dest, array } => {
                let list = self.get(state, *array);
                let slots = repr::ir_slots(&mut self.reprs, self.program, code.ty_of(*array));
                let pieces = repr::disassemble(&self.builder, &slots, list);
                if let Some(len) = pieces.get(layout::LIST_LEN) {
                    self.set(state, *dest, *len);
                }
            }
            ir::Inst::ArrayGet { dest, array, index } => {
                self.array_get(state, code, *dest, *array, *index)
            }
            ir::Inst::ArraySlice { dest, array, from } => {
                self.array_slice(state, code, *dest, *array, *from)
            }
            ir::Inst::Call { dests, func, args } => self.call(state, code, dests, *func, args, span),
            ir::Inst::CallIndirect { dests, callee, args } => {
                self.call_indirect(state, code, dests, *callee, args)
            }
            ir::Inst::CallIntrinsic { dests, key, args } => {
                self.call_intrinsic(state, code, dests, key, args, span)
            }
            ir::Inst::Structural { .. } => self.structural(state, code, inst, span),
            ir::Inst::IncRef { value } => self.incref(state, code, *value),
            ir::Inst::DecRef { value, drop } => self.decref(state, code, *value, *drop),
            ir::Inst::Abort { message } => {
                self.abort(state, message);
            }
        }
    }

    // -- constants ----------------------------------------------------------

    fn constant(&mut self, ty: ir::Type, value: &ir::Const) -> BasicValueEnum<'ctx> {
        let llvm = repr::ir_type(self.ctx, &mut self.reprs, self.program, ty);
        match value {
            ir::Const::Unit => llvm.const_zero(),
            ir::Const::Bool(b) => self.ctx.bool_type().const_int(u64::from(*b), false).into(),
            ir::Const::Int { bits, negative } => self.int_constant(llvm, *bits, *negative),
            ir::Const::Float(f) => match llvm {
                BasicTypeEnum::FloatType(t) => t.const_float(*f).into(),
                other => other.const_zero(),
            },
            ir::Const::Str(s) => self.str_literal(s),
            ir::Const::Char(c) => self.ctx.i32_type().const_int(u64::from(*c as u32), false).into(),
            ir::Const::Null => llvm.const_zero(),
            // "A value nothing reads, at the type of its result" — LLVM spells
            // it `poison`, which is exactly the claim: nothing observes it.
            ir::Const::Undef => match llvm {
                BasicTypeEnum::IntType(t) => t.get_poison().into(),
                BasicTypeEnum::FloatType(t) => t.get_poison().into(),
                BasicTypeEnum::PointerType(t) => t.get_poison().into(),
                BasicTypeEnum::StructType(t) => t.get_poison().into(),
                other => other.const_zero(),
            },
        }
    }

    /// An integer literal at the width the layout table chose.
    ///
    /// The IR carries a magnitude and a sign rather than a two's-complement
    /// pattern (`ir::Const::Int`), because choosing the width is the layout
    /// table's job — so the negation happens here, at the width, and wraps,
    /// which is the native answer for overflow (VALUE-MODEL.md §11).
    fn int_constant(
        &self,
        llvm: BasicTypeEnum<'ctx>,
        bits: u128,
        negative: bool,
    ) -> BasicValueEnum<'ctx> {
        let BasicTypeEnum::IntType(t) = llvm else { return llvm.const_zero() };
        let magnitude = if t.get_bit_width() > 64 {
            let words = [bits as u64, (bits >> 64) as u64];
            t.const_int_arbitrary_precision(&words)
        } else {
            t.const_int(bits as u64, false)
        };
        if negative {
            magnitude.const_neg().into()
        } else {
            magnitude.into()
        }
    }

    // -- arithmetic ---------------------------------------------------------

    fn unary(&self, op: ir::UnOp, prim: Prim, arg: BasicValueEnum<'ctx>) -> BasicValueEnum<'ctx> {
        let b = &self.builder;
        match (op, arg) {
            // No `nsw`/`nuw` on the negation either, for §3.4's reason: a
            // program that overflows is wrong, and "two's-complement wrap" is
            // a description it can be debugged against.
            (ir::UnOp::Neg, BasicValueEnum::IntValue(v)) => {
                b.build_int_neg(v, "neg").map(Into::into).unwrap_or(arg)
            }
            (ir::UnOp::Neg, BasicValueEnum::FloatValue(v)) => {
                b.build_float_neg(v, "fneg").map(Into::into).unwrap_or(arg)
            }
            (ir::UnOp::Not | ir::UnOp::BitNot, BasicValueEnum::IntValue(v)) => {
                b.build_not(v, "not").map(Into::into).unwrap_or(arg)
            }
            _ => {
                let _ = prim;
                arg
            }
        }
    }

    fn binary(
        &mut self,
        state: &mut Function<'ctx>,
        op: ir::BinOp,
        prim: Prim,
        operand: ir::Type,
        lhs: BasicValueEnum<'ctx>,
        rhs: BasicValueEnum<'ctx>,
    ) -> BasicValueEnum<'ctx> {
        if prim.is_float() {
            return self.binary_float(op, lhs, rhs);
        }
        // A `Str` is three words and not an integer, so there is no comparison
        // instruction for one. `middle::derives` emits exactly this — a derived
        // `Eq` or `Ord` over a type with a `Str` in it becomes
        // `ExprKind::Prim { op: Eq, prim: Str }` (`derives.rs`'s `fn eq`), which
        // lowers to an `Inst::Binary` at `Prim::Str` — so falling through to the
        // integer path would compare a struct against a struct and answer with
        // whatever the first operand happened to be.
        if matches!(prim, Prim::Str | Prim::Template) {
            return self.string_binary(state, op, operand, lhs, rhs);
        }
        let (BasicValueEnum::IntValue(l), BasicValueEnum::IntValue(r)) = (lhs, rhs) else {
            return lhs;
        };
        let signed = prim.is_signed();
        let b = &self.builder;
        let out = match op {
            // **No `nsw` and no `nuw`, anywhere.** SPEC 6.2 says overflow is
            // undefined and `nsw` is exactly how LLVM spells that, so setting
            // it would be *correct*. It is still not set: VALUE-MODEL.md §11
            // describes two backends whose overflow behaviour differs, and
            // "two's-complement wrap" is a description a program can be
            // debugged against while "whatever the optimizer inferred" is not.
            ir::BinOp::Add => b.build_int_add(l, r, "add").map(Into::into),
            ir::BinOp::Sub => b.build_int_sub(l, r, "sub").map(Into::into),
            ir::BinOp::Mul => b.build_int_mul(l, r, "mul").map(Into::into),
            ir::BinOp::BitAnd => b.build_and(l, r, "and").map(Into::into),
            ir::BinOp::BitOr => b.build_or(l, r, "or").map(Into::into),
            ir::BinOp::BitXor => b.build_xor(l, r, "xor").map(Into::into),
            ir::BinOp::Div | ir::BinOp::Rem => {
                return self.divide(state, op, prim, l, r);
            }
            _ => {
                let p = int_predicate(op, signed);
                b.build_int_compare(p, l, r, "cmp").map(Into::into)
            }
        };
        out.unwrap_or(lhs)
    }

    fn binary_float(
        &self,
        op: ir::BinOp,
        lhs: BasicValueEnum<'ctx>,
        rhs: BasicValueEnum<'ctx>,
    ) -> BasicValueEnum<'ctx> {
        let (BasicValueEnum::FloatValue(l), BasicValueEnum::FloatValue(r)) = (lhs, rhs) else {
            return lhs;
        };
        let b = &self.builder;
        let out = match op {
            ir::BinOp::Add => b.build_float_add(l, r, "fadd").map(Into::into),
            ir::BinOp::Sub => b.build_float_sub(l, r, "fsub").map(Into::into),
            ir::BinOp::Mul => b.build_float_mul(l, r, "fmul").map(Into::into),
            // Float division by zero is an infinity, not an abort: IEEE 754 is
            // the semantics, and SPEC 6.2's abort is about integers.
            ir::BinOp::Div => b.build_float_div(l, r, "fdiv").map(Into::into),
            ir::BinOp::Rem => b.build_float_rem(l, r, "frem").map(Into::into),
            ir::BinOp::Eq | ir::BinOp::Ne => return self.float_equality(op, l, r).unwrap_or(lhs),
            _ => {
                let p = float_predicate(op);
                b.build_float_compare(p, l, r, "fcmp").map(Into::into)
            }
        };
        out.unwrap_or(lhs)
    }

    /// `==` or `!=` at a float, where two `NaN`s are equal.
    ///
    /// SPEC 7.2 makes `==` an equivalence relation, so `NaN == NaN`; SPEC 6.2
    /// keeps `-0.0 == 0.0`, which `fcmp oeq` already answers. Together they
    /// are `a == b || (isnan(a) && isnan(b))`, and `fcmp uno x, x` is the
    /// `isnan` LLVM canonicalises to `llvm.is.fpclass`/a single `ucomisd`.
    ///
    /// Comparing bit patterns instead would be wrong twice over: it separates
    /// `-0.0` from `0.0`, and it separates two `NaN`s with different payloads.
    fn float_equality(
        &self,
        op: ir::BinOp,
        l: FloatValue<'ctx>,
        r: FloatValue<'ctx>,
    ) -> Option<BasicValueEnum<'ctx>> {
        let b = &self.builder;
        let equal = b.build_float_compare(FloatPredicate::OEQ, l, r, "feq").ok()?;
        let l_nan = b.build_float_compare(FloatPredicate::UNO, l, l, "feq.lnan").ok()?;
        let r_nan = b.build_float_compare(FloatPredicate::UNO, r, r, "feq.rnan").ok()?;
        let both = b.build_and(l_nan, r_nan, "feq.nan").ok()?;
        let same = b.build_or(equal, both, "feq.same").ok()?;
        if matches!(op, ir::BinOp::Ne) {
            let one = same.get_type().const_int(1, false);
            return Some(b.build_xor(same, one, "fne").ok()?.into());
        }
        Some(same.into())
    }

    /// Integer division and remainder, with SPEC 6.2's abort in front.
    ///
    /// The zero test is a branch into a **cold** block that calls
    /// `buri_rt_abort_div_zero` and is `unreachable` after it, which is what
    /// puts it out of the hot path in block placement (CODEGEN-LLVM.md §6).
    /// The message lives in the runtime rather than at the call site so both
    /// backends print one string.
    ///
    /// 128-bit division is a call to `buri_rt_i128_divmod` rather than an
    /// `sdiv i128`: it is a hundred instructions on both backends and neither
    /// should inline it (CODEGEN-STENCIL.md §5). Its operands are pairs of
    /// `u64` and its results come back through out-pointers, which is the one
    /// place in this backend an `alloca` appears for a reason that is not
    /// §2.3's aggregate — and it is still §2.3's case, because the buffer is a
    /// genuine object in memory that a C ABI writes through.
    fn divide(
        &mut self,
        state: &mut Function<'ctx>,
        op: ir::BinOp,
        prim: Prim,
        l: IntValue<'ctx>,
        r: IntValue<'ctx>,
    ) -> BasicValueEnum<'ctx> {
        state.observed.aborts = true;
        if prim.bits() == 128 {
            return self.divmod_128(state, op, prim, l, r);
        }
        let zero = r.get_type().const_zero();
        let Ok(is_zero) = self.builder.build_int_compare(IntPredicate::EQ, r, zero, "divzero")
        else {
            return l.into();
        };
        let abort = self.ctx.append_basic_block(state.value, "div.zero");
        let ok = self.ctx.append_basic_block(state.value, "div.ok");
        let _ = self.builder.build_conditional_branch(is_zero, abort, ok);

        self.builder.position_at_end(abort);
        let f = self.rt_abort_named(runtime::ABORT_DIV_ZERO);
        if let Ok(call) = self.builder.build_call(f, &[], "") {
            attrs::noreturn_call(self.ctx, call);
        }
        let _ = self.builder.build_unreachable();

        self.builder.position_at_end(ok);
        let signed = prim.is_signed();
        let out = match (op, signed) {
            (ir::BinOp::Div, true) => self.builder.build_int_signed_div(l, r, "sdiv"),
            (ir::BinOp::Div, false) => self.builder.build_int_unsigned_div(l, r, "udiv"),
            (_, true) => self.builder.build_int_signed_rem(l, r, "srem"),
            (_, false) => self.builder.build_int_unsigned_rem(l, r, "urem"),
        };
        out.map(Into::into).unwrap_or_else(|_| l.into())
    }

    fn divmod_buffers(&mut self, state: &mut Function<'ctx>) -> (PointerValue<'ctx>, PointerValue<'ctx>) {
        if let Some(pair) = state.divmod {
            return pair;
        }
        let here = self.builder.get_insert_block();
        // In the entry block, before the branch into b0, which is what makes
        // them eligible for SROA (§2.3).
        match state.entry.get_first_instruction() {
            Some(first) => self.builder.position_before(&first),
            None => self.builder.position_at_end(state.entry),
        }
        let word = self.ctx.i64_type();
        let quot = self
            .builder
            .build_array_alloca(word, word.const_int(2, false), "divmod.q")
            .unwrap_or_else(|_| self.ptr_ty().const_null());
        let rem = self
            .builder
            .build_array_alloca(word, word.const_int(2, false), "divmod.r")
            .unwrap_or_else(|_| self.ptr_ty().const_null());
        if let Some(here) = here {
            self.builder.position_at_end(here);
        }
        state.divmod = Some((quot, rem));
        (quot, rem)
    }

    fn divmod_128(
        &mut self,
        state: &mut Function<'ctx>,
        op: ir::BinOp,
        prim: Prim,
        l: IntValue<'ctx>,
        r: IntValue<'ctx>,
    ) -> BasicValueEnum<'ctx> {
        let (quot, rem) = self.divmod_buffers(state);
        let word = self.ctx.i64_type();
        let byte = self.ctx.i8_type();
        let ptr = self.ptr_ty();
        let f = self.declare_rt(
            runtime::I128_DIVMOD,
            &[
                word.into(),
                word.into(),
                word.into(),
                word.into(),
                byte.into(),
                ptr.into(),
                ptr.into(),
            ],
            None,
        );
        let halves = |b: &Builder<'ctx>, v: IntValue<'ctx>| {
            let lo = b.build_int_truncate(v, word, "lo").unwrap_or_else(|_| word.const_zero());
            let shifted = b
                .build_right_shift(v, v.get_type().const_int(64, false), false, "hi.sh")
                .unwrap_or(v);
            let hi =
                b.build_int_truncate(shifted, word, "hi").unwrap_or_else(|_| word.const_zero());
            (lo, hi)
        };
        let (a_lo, a_hi) = halves(&self.builder, l);
        let (b_lo, b_hi) = halves(&self.builder, r);
        let signed = byte.const_int(u64::from(prim.is_signed()), false);
        let args: [BasicMetadataValueEnum<'ctx>; 7] = [
            a_lo.into(),
            a_hi.into(),
            b_lo.into(),
            b_hi.into(),
            signed.into(),
            quot.into(),
            rem.into(),
        ];
        if let Ok(call) = self.builder.build_call(f, &args, "") {
            attrs::set_call_convention(call, attrs::C);
        }
        let out = if matches!(op, ir::BinOp::Div) { quot } else { rem };
        let i128 = self.ctx.i128_type();
        let lo = self
            .builder
            .build_load(word, out, "res.lo")
            .unwrap_or_else(|_| word.const_zero().into());
        let hi_ptr = repr::byte_offset(self.ctx, &self.builder, out, 8, "res.hi.p");
        let hi = self
            .builder
            .build_load(word, hi_ptr, "res.hi")
            .unwrap_or_else(|_| word.const_zero().into());
        let (BasicValueEnum::IntValue(lo), BasicValueEnum::IntValue(hi)) = (lo, hi) else {
            return i128.const_zero().into();
        };
        let lo = self
            .builder
            .build_int_z_extend(lo, i128, "lo.w")
            .unwrap_or_else(|_| i128.const_zero());
        let hi = self
            .builder
            .build_int_z_extend(hi, i128, "hi.w")
            .unwrap_or_else(|_| i128.const_zero());
        let hi = self
            .builder
            .build_left_shift(hi, i128.const_int(64, false), "hi.sh")
            .unwrap_or(hi);
        self.builder.build_or(lo, hi, "i128").map(Into::into).unwrap_or_else(|_| lo.into())
    }

    // -- aggregates ---------------------------------------------------------

    fn make_record(
        &mut self,
        state: &mut Function<'ctx>,
        code: &ir::Code,
        dest: ir::ValueId,
        fields: &[ir::ValueId],
    ) {
        let slots = repr::ir_slots(&mut self.reprs, self.program, code.ty_of(dest));
        let mut pieces = Vec::with_capacity(slots.len());
        for f in fields {
            let fs = repr::ir_slots(&mut self.reprs, self.program, code.ty_of(*f));
            let value = self.get(state, *f);
            pieces.extend(repr::disassemble(&self.builder, &fs, value));
        }
        let value = repr::assemble(self.ctx, &self.builder, &slots, &pieces);
        self.set(state, dest, value);
    }

    fn get_field(
        &mut self,
        state: &mut Function<'ctx>,
        code: &ir::Code,
        dest: ir::ValueId,
        agg: ir::ValueId,
        index: usize,
    ) {
        let ir::Type::Agg(id) = code.ty_of(agg) else { return };
        let (slots, range) = {
            let r = self.reprs.of(self.program, id);
            (r.slots.clone(), r.field_range(index))
        };
        let whole = self.get(state, agg);
        let pieces = repr::disassemble(&self.builder, &slots, whole);
        let (start, end) = range;
        let want = repr::ir_slots(&mut self.reprs, self.program, code.ty_of(dest));
        let taken: Vec<BasicValueEnum<'ctx>> =
            pieces.get(start..end).map(<[_]>::to_vec).unwrap_or_default();
        let value = repr::assemble(self.ctx, &self.builder, &want, &taken);
        self.set(state, dest, value);
    }

    fn get_tag(
        &mut self,
        state: &mut Function<'ctx>,
        code: &ir::Code,
        dest: ir::ValueId,
        agg: ir::ValueId,
    ) {
        let ir::Type::Agg(id) = code.ty_of(agg) else { return };
        // The niche spends one variant's *absence* of fields: `.None` is the
        // pointer set to null and `.Some` is everything else.
        let (some_variant, none_variant) = self.option_variants(id);
        let (slots, enum_repr) = {
            let r = self.reprs.of(self.program, id);
            (r.slots.clone(), r.enum_repr().cloned())
        };
        let Some(enum_repr) = enum_repr else { return };
        let whole = self.get(state, agg);
        let tag = self.tag_of(&slots, &enum_repr, none_variant, some_variant, whole);
        self.set(state, dest, tag);
    }

    /// Where `.Some` and `.None` sit, read off the layout's per-variant field
    /// lists rather than assumed to be 0 and 1: a declaration order of
    /// `{ None, Some(T) }` has to work as well as the other.
    fn option_variants(&mut self, id: ir::TypeId) -> (usize, usize) {
        let r = self.reprs.of(self.program, id);
        match &r.layout.repr {
            LayoutRepr::Enum { variants, .. } => (
                variants.iter().position(|v| !v.is_empty()).unwrap_or(0),
                variants.iter().position(Vec::is_empty).unwrap_or(1),
            ),
            _ => (0, 1),
        }
    }

    /// An enum's discriminant, from the value rather than from a `ValueId`.
    ///
    /// Split out of [`Unit::get_tag`] so that a `Result` this backend built for
    /// itself — the one a `foldResult` step answers — is read by the same three
    /// cases as one the IR named.
    fn tag_of(
        &mut self,
        slots: &[Slot],
        enum_repr: &EnumRepr,
        none_variant: usize,
        some_variant: usize,
        whole: BasicValueEnum<'ctx>,
    ) -> BasicValueEnum<'ctx> {
        let pieces = repr::disassemble(&self.builder, slots, whole);
        let i32t = self.ctx.i32_type();
        match *enum_repr {
            // The value *is* the tag; widen it to the `i32` the IR says a tag
            // is (`ir::Inst::GetTag`), and the width it is *stored* at stays
            // the layout table's answer.
            EnumRepr::Bare { .. } | EnumRepr::Tagged { .. } => match pieces.first() {
                Some(BasicValueEnum::IntValue(v)) => self
                    .builder
                    .build_int_z_extend_or_bit_cast(*v, i32t, "tag")
                    .map(Into::into)
                    .unwrap_or_else(|_| i32t.const_zero().into()),
                _ => i32t.const_zero().into(),
            },
            // The niche: `.None` is the pointer at `null_at` set to null, so
            // the tag is a null test and a `select` between the two variant
            // indices (VALUE-MODEL.md §6).
            EnumRepr::Niche { null_at } => {
                let at = slots.iter().position(|s| s.offset == null_at && s.ty.is_pointer());
                match at.and_then(|i| pieces.get(i).copied()) {
                    Some(BasicValueEnum::PointerValue(p)) => {
                        let is_null = self
                            .builder
                            .build_is_null(p, "isnone")
                            .unwrap_or_else(|_| self.ctx.bool_type().const_zero());
                        let some = i32t.const_int(some_variant as u64, false);
                        let none = i32t.const_int(none_variant as u64, false);
                        self.builder
                            .build_select(is_null, none, some, "tag")
                            .unwrap_or_else(|_| i32t.const_zero().into())
                    }
                    _ => i32t.const_zero().into(),
                }
            }
        }
    }

    fn make_enum(
        &mut self,
        state: &mut Function<'ctx>,
        code: &ir::Code,
        dest: ir::ValueId,
        variant: u32,
        fields: &[ir::ValueId],
        span: Span,
    ) {
        let ir::Type::Agg(id) = code.ty_of(dest) else { return };
        let (slots, enum_repr, offsets, size) = {
            let r = self.reprs.of(self.program, id);
            let offsets = r.layout.variant(variant as usize).to_vec();
            (r.slots.clone(), r.enum_repr().cloned(), offsets, r.layout.size)
        };
        let _ = (&slots, &enum_repr, &offsets, size);
        let parts: Vec<(Vec<Slot>, Vec<BasicValueEnum<'ctx>>)> = fields
            .iter()
            .map(|f| {
                let fs = repr::ir_slots(&mut self.reprs, self.program, code.ty_of(*f));
                let value = self.get(state, *f);
                let pieces = repr::disassemble(&self.builder, &fs, value);
                (fs, pieces)
            })
            .collect();
        let Some(value) = self.build_variant(id, variant as usize, &parts) else { return };
        let _ = span;
        self.set(state, dest, value);
    }

    /// One variant of an enum, from field pieces already in registers.
    ///
    /// The half of [`Unit::make_enum`] that does not need a `ValueId` for each
    /// field, so that a value this backend builds for itself — the `Option` a
    /// `find` answers, the `.Ok` a `foldResult` carries out — is built by the
    /// same three cases as one the IR named.
    fn build_variant(
        &mut self,
        id: ir::TypeId,
        variant: usize,
        fields: &[(Vec<Slot>, Vec<BasicValueEnum<'ctx>>)],
    ) -> Option<BasicValueEnum<'ctx>> {
        let (slots, enum_repr, offsets, size) = {
            let r = self.reprs.of(self.program, id);
            let offsets = r.layout.variant(variant).to_vec();
            (r.slots.clone(), r.enum_repr().cloned()?, offsets, r.layout.size)
        };
        Some(match enum_repr {
            EnumRepr::Bare { tag } => {
                let t = repr::slot_type(self.ctx, SlotTy::Scalar(tag));
                match t {
                    BasicTypeEnum::IntType(t) => t.const_int(variant as u64, false).into(),
                    other => other.const_zero(),
                }
            }
            // The payload *is* the value, and `.None` is it with the niche
            // pointer null. A zeroed register is the right `.None`: every
            // other slot of a `.None` is unobservable, and zero is the only
            // pattern that is not poison.
            EnumRepr::Niche { .. } => match fields.first() {
                Some((fs, pieces)) => repr::assemble(self.ctx, &self.builder, fs, pieces),
                None => repr::register_type(self.ctx, &slots).const_zero(),
            },
            EnumRepr::Tagged { tag, payload } => {
                let tag_value = match repr::slot_type(self.ctx, SlotTy::Scalar(tag)) {
                    BasicTypeEnum::IntType(t) => t.const_int(variant as u64, false).into(),
                    other => other.const_zero(),
                };
                let bytes = size.saturating_sub(payload);
                let blob_ty = repr::blob_type(self.ctx, bytes);
                let mut blob: IntValue<'ctx> = blob_ty.const_zero();
                for (i, (fs, pieces)) in fields.iter().enumerate() {
                    let Some(at) = offsets.get(i).copied() else { continue };
                    let within = at.saturating_sub(payload);
                    for (slot, piece) in fs.iter().zip(pieces) {
                        let bits = repr::slot_to_bits(self.ctx, &self.builder, *slot, *piece);
                        let widened = self
                            .builder
                            .build_int_z_extend_or_bit_cast(bits, blob_ty, "pay.w")
                            .unwrap_or_else(|_| blob_ty.const_zero());
                        let shift = u64::from(within.saturating_add(slot.offset))
                            .saturating_mul(8);
                        let placed = self
                            .builder
                            .build_left_shift(widened, blob_ty.const_int(shift, false), "pay.sh")
                            .unwrap_or(widened);
                        blob = self.builder.build_or(blob, placed, "pay").unwrap_or(blob);
                    }
                }
                let values = [tag_value, blob.into()];
                repr::assemble(self.ctx, &self.builder, &slots, &values)
            }
        })
    }

    fn get_payload(
        &mut self,
        state: &mut Function<'ctx>,
        code: &ir::Code,
        dest: ir::ValueId,
        agg: ir::ValueId,
        variant: usize,
        index: usize,
    ) {
        let ir::Type::Agg(id) = code.ty_of(agg) else { return };
        let (slots, enum_repr, offsets) = {
            let r = self.reprs.of(self.program, id);
            (r.slots.clone(), r.enum_repr().cloned(), r.layout.variant(variant).to_vec())
        };
        let Some(enum_repr) = enum_repr else { return };
        let whole = self.get(state, agg);
        let want = repr::ir_slots(&mut self.reprs, self.program, code.ty_of(dest));
        let value = self.payload_of(&slots, &enum_repr, &offsets, index, &want, whole);
        self.set(state, dest, value);
    }

    /// One variant field, from the value rather than from a `ValueId`.
    ///
    /// Split out of [`Unit::get_payload`] for [`Unit::tag_of`]'s reason: the
    /// `.Ok` a `foldResult` step answers is a value this backend built, and
    /// taking it apart is the same three cases.
    fn payload_of(
        &mut self,
        slots: &[Slot],
        enum_repr: &EnumRepr,
        offsets: &[u32],
        index: usize,
        want: &[Slot],
        whole: BasicValueEnum<'ctx>,
    ) -> BasicValueEnum<'ctx> {
        match *enum_repr {
            // A payload area of no bytes has no fields to project; the result
            // is zero-sized by construction.
            EnumRepr::Bare { .. } => repr::assemble(self.ctx, &self.builder, want, &[]),
            // `.Some`'s payload is the whole value (`layout::build_enum`).
            EnumRepr::Niche { .. } => whole,
            EnumRepr::Tagged { payload, .. } => {
                let pieces = repr::disassemble(&self.builder, slots, whole);
                let Some(BasicValueEnum::IntValue(blob)) = pieces.get(1).copied() else {
                    return repr::assemble(self.ctx, &self.builder, want, &[]);
                };
                let within = offsets.get(index).copied().unwrap_or(0).saturating_sub(payload);
                let mut taken = Vec::with_capacity(want.len());
                for slot in want {
                    let shift =
                        u64::from(within.saturating_add(slot.offset)).saturating_mul(8);
                    let moved = self
                        .builder
                        .build_right_shift(
                            blob,
                            blob.get_type().const_int(shift, false),
                            false,
                            "pay.sh",
                        )
                        .unwrap_or(blob);
                    let narrow = repr::blob_type(self.ctx, slot.ty.size());
                    let cut = self
                        .builder
                        .build_int_truncate_or_bit_cast(moved, narrow, "pay.cut")
                        .unwrap_or(moved);
                    taken.push(repr::slot_from_bits(self.ctx, &self.builder, *slot, cut));
                }
                repr::assemble(self.ctx, &self.builder, want, &taken)
            }
        }
    }

    /// `[T]` of exactly these elements: **one allocation** (VALUE-MODEL.md
    /// §4), `16 + n * stride(T)` charged, elements stored contiguously.
    fn make_array(
        &mut self,
        state: &mut Function<'ctx>,
        code: &ir::Code,
        dest: ir::ValueId,
        elems: &[ir::ValueId],
    ) {
        let ir::Type::Agg(id) = code.ty_of(dest) else { return };
        let (list_slots, element) = {
            let r = self.reprs.of(self.program, id);
            (r.slots.clone(), self.reprs.of(self.program, id).ty.clone())
        };
        let Some(element) = self.reprs.element(&element) else { return };
        let (stride, element_slots, element_align) = {
            let r = self.reprs.of_ty(&element);
            (r.layout.stride, r.slots.clone(), r.layout.align)
        };
        let bytes = u64::from(stride).saturating_mul(elems.len() as u64);
        let alloc = self.rt_alloc();
        state.observed.allocates = true;
        let size = self.ctx.i64_type().const_int(bytes, false);
        let block = match self.builder.build_call(alloc, &[size.into()], "list") {
            Ok(call) => {
                attrs::set_call_convention(call, attrs::C);
                call.try_as_basic_value()
                    .basic()
                    .and_then(|v| v.try_into().ok())
                    .unwrap_or_else(|| self.ptr_ty().const_null())
            }
            Err(_) => self.ptr_ty().const_null(),
        };
        for (i, e) in elems.iter().enumerate() {
            let at = u64::from(stride).saturating_mul(i as u64);
            let value = self.get(state, *e);
            let pieces = repr::disassemble(&self.builder, &element_slots, value);
            for (slot, piece) in element_slots.iter().zip(pieces) {
                let offset = at.saturating_add(u64::from(slot.offset));
                let p = repr::byte_offset(self.ctx, &self.builder, block, offset as i64, "elem");
                if let Ok(store) = self.builder.build_store(p, piece) {
                    let _ = store.set_alignment(repr::access_align(element_align, *slot));
                }
            }
        }
        let len = self.ctx.i64_type().const_int(elems.len() as u64, false);
        let values = [block.into(), len.into()];
        let value = repr::assemble(self.ctx, &self.builder, &list_slots, &values);
        self.set(state, dest, value);
    }

    /// `[a, b, ..rest]`'s `rest`: a fresh block holding the elements from
    /// `from` to the end.
    ///
    /// `middle::lower` emits this for a rest pattern and for nothing else, and
    /// the bounds are already decided: the arm this instruction is in was
    /// entered by a length test, so `from <= len` holds and the count cannot go
    /// negative. That is the same promise `Unit::array_get` runs on.
    ///
    /// **A copy and not a view.** `cli/runtime/value.rs` states the rule this
    /// rests on — a `[T]` is never a view, so `ptr` is always a payload start
    /// and the header is at `ptr - 16` — so a slice that pointed into the
    /// middle of the source's block would have no header at all, and the first
    /// `decref` of it would read sixteen bytes of somebody's elements as a
    /// reference count. The `Alloc` bound on every list producer in `list.buri`
    /// is the language saying the same thing.
    ///
    /// The elements are **retained**, because the copy is a second owner of
    /// whatever each of them holds. Both halves of that pair are this backend's
    /// own — this retain, and the release the result's own glue does — so the
    /// question asked is [`Reprs::counted_type`] and not `middle::rc`'s, which
    /// is `Unit::list_filter`'s distinction applied to a copy nothing filtered.
    fn array_slice(
        &mut self,
        state: &mut Function<'ctx>,
        code: &ir::Code,
        dest: ir::ValueId,
        array: ir::ValueId,
        from: ir::ValueId,
    ) {
        let word = self.ctx.i64_type();
        let list_ty = code.ty_of(array);
        let Some(element) = self.type_of(list_ty).and_then(|t| self.reprs.element(&t)) else {
            return;
        };
        let (stride, align) = {
            let r = self.reprs.of_ty(&element);
            (r.layout.stride, r.layout.align.max(1))
        };
        let slots = repr::ir_slots(&mut self.reprs, self.program, list_ty);
        let value = self.get(state, array);
        let pieces = repr::disassemble(&self.builder, &slots, value);
        let (Some(BasicValueEnum::PointerValue(base)), Some(BasicValueEnum::IntValue(len))) = (
            pieces.get(layout::LIST_PTR).copied(),
            pieces.get(layout::LIST_LEN).copied(),
        ) else {
            return;
        };
        let BasicValueEnum::IntValue(start) = self.get(state, from) else { return };
        let Ok(count) = self.builder.build_int_sub(len, start, "slice.n") else { return };
        let bytes = self
            .builder
            .build_int_mul(count, word.const_int(u64::from(stride), false), "slice.bytes")
            .unwrap_or(count);
        let block = self.heap(state, bytes, "slice");
        let src = self.elem_at(base, start, stride, "slice.from");
        let _ = self.builder.build_memcpy(block, align, src, align, bytes);
        if self.reprs.counted_type(&element) {
            self.each_element(state, block, count, stride, &element, true);
        }
        let out = repr::assemble(
            self.ctx,
            &self.builder,
            &slots,
            &[block.into(), count.into()],
        );
        self.set(state, dest, out);
    }

    /// An element, with the bounds check already done: every emission site is
    /// guarded by a comparison against `ArrayLen` in a dominating block, which
    /// is what `list.get`'s `Option` return means in the source.
    fn array_get(
        &mut self,
        state: &mut Function<'ctx>,
        code: &ir::Code,
        dest: ir::ValueId,
        array: ir::ValueId,
        index: ir::ValueId,
    ) {
        let Some(read) = self.element_at(state, code, array, index) else { return };
        let (element_slots, taken) = self.load_element(read.base, read.index, &read.element);
        let value = repr::assemble(self.ctx, &self.builder, &element_slots, &taken);
        self.set(state, dest, value);
    }

    /// The block, its length, the index and the element type of one `[T]` read.
    ///
    /// Split out of [`Unit::array_get`] so that [`Unit::list_get`] can settle
    /// all of them *before* it opens the blocks its `Option` needs.
    fn element_at(
        &mut self,
        state: &mut Function<'ctx>,
        code: &ir::Code,
        array: ir::ValueId,
        index: ir::ValueId,
    ) -> Option<ElementRead<'ctx>> {
        let ir::Type::Agg(id) = code.ty_of(array) else { return None };
        let (list_ty, list_slots) = {
            let r = self.reprs.of(self.program, id);
            (r.ty.clone(), r.slots.clone())
        };
        let element = self.reprs.element(&list_ty)?;
        let list = self.get(state, array);
        let pieces = repr::disassemble(&self.builder, &list_slots, list);
        let (
            Some(BasicValueEnum::PointerValue(base)),
            Some(BasicValueEnum::IntValue(len)),
        ) = (
            pieces.get(layout::LIST_PTR).copied(),
            pieces.get(layout::LIST_LEN).copied(),
        )
        else {
            return None;
        };
        let BasicValueEnum::IntValue(i) = self.get(state, index) else { return None };
        Some(ElementRead { base, len, index: i, element })
    }

    /// The element at `i`: the slots it is made of, and the pieces loaded out
    /// of the block. The load half of [`Unit::array_get`], shared with
    /// [`Unit::list_get`], and total — every precondition is its caller's.
    fn load_element(
        &mut self,
        base: PointerValue<'ctx>,
        i: IntValue<'ctx>,
        element: &Ty,
    ) -> (Vec<Slot>, Vec<BasicValueEnum<'ctx>>) {
        let (stride, element_slots, element_align) = {
            let r = self.reprs.of_ty(element);
            (r.layout.stride, r.slots.clone(), r.layout.align)
        };
        let at = self.elem_at(base, i, stride, "elem");
        let mut taken = Vec::with_capacity(element_slots.len());
        for slot in &element_slots {
            let p = repr::byte_offset(self.ctx, &self.builder, at, i64::from(slot.offset), "sl");
            let ty = repr::slot_type(self.ctx, slot.ty);
            match self.builder.build_load(ty, p, "load") {
                Ok(v) => {
                    if let Some(instr) = v.as_instruction_value() {
                        let _ = instr.set_alignment(repr::access_align(element_align, *slot));
                    }
                    taken.push(v);
                }
                Err(_) => taken.push(ty.const_zero()),
            }
        }
        (element_slots, taken)
    }

    /// `{ code, env }` (VALUE-MODEL.md §7), with the two additions the
    /// flattened calling convention forces — see this file's header.
    ///
    /// `code` is **always** a thunk, never the lifted lambda: the lambda takes
    /// its environment as an aggregate parameter, which under VALUE-MODEL.md
    /// §5.1 is that aggregate's *leaves*, and a call site holding a closure has
    /// only a pointer. The same shape covers a capture-free lambda, which
    /// `middle::closures` has already turned into a plain `FnRef` — it still
    /// gets a thunk, because the call site cannot know which of the two it is
    /// holding.
    ///
    /// The rejected alternative was to make `code` the lambda and pass the
    /// environment's leaves at the call site. That needs the *caller* to know
    /// the capture layout, which is exactly what `Ty::Fn` does not record.
    fn make_closure(
        &mut self,
        state: &mut Function<'ctx>,
        code: &ir::Code,
        dest: ir::ValueId,
        func: FuncIdx,
        env: Option<ir::ValueId>,
        span: Span,
    ) {
        let _ = span;
        let Some(thunk) = self.thunk(func, env.is_some()) else { return };
        let env_ptr = match env {
            None => self.ptr_ty().const_null(),
            Some(e) => self.build_env(state, code, e),
        };
        let slots = repr::ir_slots(&mut self.reprs, self.program, code.ty_of(dest));
        let values = [function_pointer(thunk).into(), env_ptr.into()];
        let value = repr::assemble(self.ctx, &self.builder, &slots, &values);
        self.set(state, dest, value);
    }

    /// The environment block: its own drop glue in the first word, the
    /// environment record at [`ENV_FIELDS`].
    ///
    /// `Ty::Fn` does not record what was captured, so a `decref` of a closure
    /// has no type from which to derive the function that releases the
    /// environment's contents. The block therefore carries that function
    /// pointer itself and one universal glue reads it ([`Unit::build_env_glue`]).
    /// Eight bytes per closure, against a closure that could not be freed.
    fn build_env(
        &mut self,
        state: &mut Function<'ctx>,
        code: &ir::Code,
        env: ir::ValueId,
    ) -> PointerValue<'ctx> {
        let (slots, size, align) = self.dest_shape(code, env);
        let bytes = u64::from(size.saturating_add(ENV_FIELDS));
        let alloc = self.rt_alloc();
        state.observed.allocates = true;
        let block = match self
            .builder
            .build_call(alloc, &[self.ctx.i64_type().const_int(bytes, false).into()], "env")
        {
            Ok(call) => {
                attrs::set_call_convention(call, attrs::C);
                call.try_as_basic_value()
                    .basic()
                    .and_then(|v| v.try_into().ok())
                    .unwrap_or_else(|| self.ptr_ty().const_null())
            }
            Err(_) => self.ptr_ty().const_null(),
        };
        let glue = self
            .type_of(code.ty_of(env))
            .and_then(|t| self.release_glue(&t))
            .map(function_pointer)
            .unwrap_or_else(|| self.ptr_ty().const_null());
        if let Ok(store) = self.builder.build_store(block, glue) {
            let _ = store.set_alignment(8);
        }
        let record =
            repr::byte_offset(self.ctx, &self.builder, block, i64::from(ENV_FIELDS), "env.rec");
        let value = self.get(state, env);
        let pieces = repr::disassemble(&self.builder, &slots, value);
        self.store_slots(record, &slots, align, &pieces);
        block
    }

    // -- calls --------------------------------------------------------------

    fn flatten_args(
        &mut self,
        state: &Function<'ctx>,
        code: &ir::Code,
        args: &[ir::ValueId],
    ) -> Vec<BasicMetadataValueEnum<'ctx>> {
        let mut out = Vec::new();
        for a in args {
            let slots = repr::ir_slots(&mut self.reprs, self.program, code.ty_of(*a));
            let value = self.get(state, *a);
            for piece in repr::disassemble(&self.builder, &slots, value) {
                out.push(piece.into());
            }
        }
        out
    }

    fn bind_results(
        &mut self,
        state: &mut Function<'ctx>,
        code: &ir::Code,
        dests: &[ir::ValueId],
        result: Option<BasicValueEnum<'ctx>>,
    ) {
        let mut all_slots = Vec::new();
        for d in dests {
            all_slots.extend(repr::ir_slots(&mut self.reprs, self.program, code.ty_of(*d)));
        }
        let pieces = match result {
            Some(v) => repr::disassemble(&self.builder, &all_slots, v),
            None => Vec::new(),
        };
        let mut at = 0usize;
        for d in dests {
            let slots = repr::ir_slots(&mut self.reprs, self.program, code.ty_of(*d));
            let end = at.saturating_add(slots.len());
            let taken: Vec<BasicValueEnum<'ctx>> =
                pieces.get(at..end).map(<[_]>::to_vec).unwrap_or_default();
            let value = repr::assemble(self.ctx, &self.builder, &slots, &taken);
            self.set(state, *d, value);
            at = end;
        }
    }

    fn call(
        &mut self,
        state: &mut Function<'ctx>,
        code: &ir::Code,
        dests: &[ir::ValueId],
        func: FuncIdx,
        args: &[ir::ValueId],
        span: Span,
    ) {
        // A direct call to a function whose body is a runtime key is a call
        // into `cli/runtime` (VALUE-MODEL.md §10), not a call to a Buri
        // function — the backend declares the import and defines nothing.
        if let Some(key) = self.program.funcs.get(func.index()).and_then(ir::Func::intrinsic_key) {
            let key = key.to_string();
            self.call_intrinsic(state, code, dests, &key, args, span);
            return;
        }
        let Some(callee) = self.declare(func) else { return };
        let argv = self.flatten_args(state, code, args);
        let Ok(call) = self.builder.build_call(callee, &argv, "") else { return };
        attrs::set_call_convention(call, attrs::FAST);
        if let Some(f) = self.program.funcs.get(func.index()) {
            if f.facts.can_abort {
                state.observed.aborts = true;
            }
            if matches!(f.facts.purity, ir::Purity::Effectful) {
                state.observed.opaque = true;
            }
            if matches!(f.facts.purity, ir::Purity::Allocating) {
                state.observed.allocates = true;
            }
        }
        self.bind_results(state, code, dests, call.try_as_basic_value().basic());
    }

    /// A call through a closure value: take `code`, call it with `env` first.
    ///
    /// The callee is a thunk ([`Unit::make_closure`]), so its signature is
    /// `(ptr env, args-leaves...) -> rets` and the environment is passed even
    /// where it is null — the two thunk shapes have to be call-compatible,
    /// because which one a value holds is a run-time fact.
    fn call_indirect(
        &mut self,
        state: &mut Function<'ctx>,
        code: &ir::Code,
        dests: &[ir::ValueId],
        callee: ir::ValueId,
        args: &[ir::ValueId],
    ) {
        let closure_slots = repr::ir_slots(&mut self.reprs, self.program, code.ty_of(callee));
        let value = self.get(state, callee);
        let pieces = repr::disassemble(&self.builder, &closure_slots, value);
        let (
            Some(BasicValueEnum::PointerValue(target)),
            Some(BasicValueEnum::PointerValue(env)),
        ) = (
            pieces.get(layout::CLOSURE_CODE).copied(),
            pieces.get(layout::CLOSURE_ENV).copied(),
        )
        else {
            return;
        };
        let mut param_slots = Vec::new();
        for a in args {
            param_slots.extend(repr::ir_slots(&mut self.reprs, self.program, code.ty_of(*a)));
        }
        let mut ret_slots = Vec::new();
        for d in dests {
            ret_slots.extend(repr::ir_slots(&mut self.reprs, self.program, code.ty_of(*d)));
        }
        let ty = self.closure_fn_type(&param_slots, &ret_slots);
        let mut argv: Vec<BasicMetadataValueEnum<'ctx>> = vec![env.into()];
        argv.extend(self.flatten_args(state, code, args));
        let Ok(call) = self.builder.build_indirect_call(ty, target, &argv, "") else { return };
        attrs::set_call_convention(call, attrs::FAST);
        // An indirect callee's effects are not known here, so the caller keeps
        // the default `memory(readwrite)` rather than a claim it cannot back.
        state.observed.opaque = true;
        self.bind_results(state, code, dests, call.try_as_basic_value().basic());
    }

    /// One intrinsic, by the four routes it can take.
    ///
    /// The order is the order of decreasing knowledge, which is also increasing
    /// cost: an operation this backend can *generate* is generated, an
    /// operation the archive has a body for is called, and only a key that is
    /// neither is a diagnostic. [`Unit::numeric`] and [`Unit::open_coded`]
    /// answer `false` for a key they do not claim, so adding a symbol to
    /// [`runtime::ENTRIES`] later does not mean unpicking one of them.
    fn call_intrinsic(
        &mut self,
        state: &mut Function<'ctx>,
        code: &ir::Code,
        dests: &[ir::ValueId],
        key: &str,
        args: &[ir::ValueId],
        span: Span,
    ) {
        if self.numeric(state, code, dests, key, args, span)
            || self.open_coded(state, code, dests, key, args)
            || self.list_closure(state, code, dests, key, args)
            || self.derive_array(state, code, dests, key, args)
            || self.derived(state, code, dests, key, args, span)
        {
            return;
        }
        let Some(entry) = runtime::entry(key) else {
            self.error(
                span,
                format!("the native runtime has no implementation of `{key}`"),
                "report it: this is a toolchain bug, not a problem with your program",
            );
            return;
        };
        let elements = Elements {
            source: self.generic_element(code, dests, entry, args),
            answer: self.answer_element(code, dests),
        };
        let Some(argv) = self.entry_args(state, code, entry, args, elements, span) else {
            return;
        };
        self.call_entry(state, code, dests, entry, argv, span);
    }

    /// `T`, for an entry whose C signature carries a stride and a retain
    /// (`cli/runtime/lib.rs` §2 rule 4).
    ///
    /// The first `[T]` argument answers it for `get`, `concat`, `push`,
    /// `reverse` and `slice`; the **destination** answers it for `repeat`,
    /// whose only mention of `T` is a value whose lowered type is a bare
    /// register shape with no `Ty` behind it. Reading it off the item instead
    /// would work for `[Str]` and fail for `[Int]`, which is the worst place
    /// for a rule to be nearly right.
    fn generic_element(
        &mut self,
        code: &ir::Code,
        dests: &[ir::ValueId],
        entry: &runtime::Entry,
        args: &[ir::ValueId],
    ) -> Option<Ty> {
        let mut cursor = 0usize;
        for mode in entry.args {
            if !mode.consumes() {
                continue;
            }
            let at = cursor;
            cursor = cursor.saturating_add(1);
            if !matches!(mode, runtime::Arg::Elems) {
                continue;
            }
            let list = args.get(at).copied().and_then(|a| self.type_of(code.ty_of(a)));
            if let Some(elem) = list.and_then(|t| self.reprs.element(&t)) {
                return Some(elem);
            }
        }
        let out = dests.first().copied().and_then(|d| self.type_of(code.ty_of(d)))?;
        self.reprs.element(&out)
    }

    /// The element type of an entry's `[T]` **result**, where it has one.
    ///
    /// [`Unit::generic_element`] answers with the destination's element only
    /// when no argument had one, because a `stride` describes the list the
    /// runtime *walks*. A step's two lists are different types, so its result's
    /// element is a second question and is asked separately rather than by
    /// changing what the first one means.
    fn answer_element(&mut self, code: &ir::Code, dests: &[ir::ValueId]) -> Option<Ty> {
        let out = dests.first().copied().and_then(|d| self.type_of(code.ty_of(d)))?;
        self.reprs.element(&out)
    }

    /// The C parameter list of one runtime entry, from the Buri argument list.
    ///
    /// `cli/runtime/lib.rs` §2 rule 1: every parameter is a scalar leaf,
    /// flattened in declaration order, and a zero-sized one is dropped. The
    /// shapes are checked against what the IR actually holds, so that a table
    /// entry that has drifted from the contract is a diagnostic here rather
    /// than a wrong answer at run time.
    ///
    /// The walk is over the *C* list with a cursor into the Buri one, because
    /// §2 rule 4's `stride` and `retain` sit inside the C list at positions
    /// that no Buri argument corresponds to.
    fn entry_args(
        &mut self,
        state: &mut Function<'ctx>,
        code: &ir::Code,
        entry: &runtime::Entry,
        args: &[ir::ValueId],
        elements: Elements,
        span: Span,
    ) -> Option<Vec<BasicMetadataValueEnum<'ctx>>> {
        let element = elements.source.clone();
        let key = entry.key;
        let mut argv: Vec<BasicMetadataValueEnum<'ctx>> = Vec::new();
        let mut cursor = 0usize;
        for mode in entry.args {
            if !mode.consumes() {
                let Some(elem) = element.clone() else {
                    self.error(
                        span,
                        format!(
                            "internal error: `{key}` asks for a stride before naming an \
                             element type"
                        ),
                        "this is a toolchain bug; report it",
                    );
                    return None;
                };
                match mode {
                    runtime::Arg::Stride => {
                        let stride = self.reprs.of_ty(&elem).layout.stride;
                        argv.push(self.ctx.i64_type().const_int(u64::from(stride), false).into());
                    }
                    _ => {
                        let glue = self
                            .retain_glue(&elem)
                            .map(function_pointer)
                            .unwrap_or_else(|| self.ptr_ty().const_null());
                        argv.push(glue.into());
                    }
                }
                continue;
            }
            let Some(a) = args.get(cursor).copied() else {
                self.error(
                    span,
                    format!(
                        "internal error: `{key}` takes {} arguments and the call has {}",
                        entry.args.iter().filter(|m| m.consumes()).count(),
                        args.len()
                    ),
                    "this is a toolchain bug; report it",
                );
                return None;
            };
            cursor = cursor.saturating_add(1);
            let ir_ty = code.ty_of(a);
            let slots = repr::ir_slots(&mut self.reprs, self.program, ir_ty);
            let value = self.get(state, a);
            let pieces = repr::disassemble(&self.builder, &slots, value);
            match mode {
                // **Dropped whatever it weighs.** The obvious reading of
                // VALUE-MODEL.md §8 — "a context of zero-sized implementations
                // is dropped" — makes zero-sizedness the reason, and it is not:
                // a context is dropped here because the *runtime* has no use for
                // one, allocating through `buri_rt_alloc` and reading no
                // capability. Every context built from `core/host` happens to be
                // empty structs, so the two readings agree until a program
                // builds one from `core/testing/context`, whose `TestAlloc` is
                // `struct TestAlloc(I64)` and carries a handle. Then a check on
                // the leaf count refuses a valid program, and a *spread* on the
                // leaf count would put one extra word into a C signature that
                // has no parameter for it and shift every argument after it into
                // the wrong register — which links, runs, and answers garbage.
                runtime::Arg::Dropped => {}
                runtime::Arg::Bytes => {
                    // `ptr`, `len` — the byte range without the owning block.
                    for piece in pieces.into_iter().skip(1) {
                        argv.push(piece.into());
                    }
                }
                // The element crosses as a pointer to a stack copy (§2 rule 4):
                // the runtime cannot name `T`'s leaves, so there is nothing to
                // flatten it into. The copy is at `T`'s *memory* layout, which
                // is what `stride` describes and what the runtime's
                // `copy_nonoverlapping` reads.
                // The trampoline's four words. `{ code, env }` does not cross:
                // it goes into a two-word `alloca` the entry thunk reads, so
                // what the runtime holds is a C function pointer and an opaque
                // record. The two strides are here rather than in an
                // `Arg::Stride` because a step reads one element type and
                // writes another, and `element` is only the first.
                runtime::Arg::Step => {
                    let step = self.type_of(ir_ty);
                    let Some(Ty::Fn(ps, r)) = step else {
                        self.error(
                            span,
                            format!("internal error: `{key}`'s step is not a function"),
                            "this is a toolchain bug; report it",
                        );
                        return None;
                    };
                    let step_index = step_call(key).and_then(|c| c.index);
                    // A context that owns a reference count would need a
                    // retain per element rather than a copy, and none does:
                    // `core/host`'s allocators are empty structs and
                    // `core/testing/context`'s carry a handle. Refused here,
                    // where there is a span to hang it on.
                    if ps
                        .iter()
                        .enumerate()
                        .take(ps.len().saturating_sub(1))
                        .any(|(i, t)| step_index != Some(i) && self.rc_counted(t))
                    {
                        self.error(
                            span,
                            format!("`{key}` was given a step whose context owns a count"),
                            "this is a toolchain bug; report it",
                        );
                        return None;
                    }
                    let bytes = self.step_state_bytes(&ps, step_index);
                    let record = self.scratch(state, bytes, 8);
                    self.store_slots(record, &slots, 8, &pieces);
                    // The contexts, beside the closure. They are Buri arguments
                    // this row `Arg::Dropped`s out of the C signature, and this
                    // is where they go instead — read back by the entry thunk,
                    // which is the only thing that wants them.
                    let ctx_at = self.step_ctx_offsets(&ps, step_index);
                    let from: Vec<ir::ValueId> = step_call(key)
                        .and_then(|c| c.ctx)
                        .into_iter()
                        .filter_map(|i| args.get(i).copied())
                        .collect();
                    if from.len() != ctx_at.len() {
                        self.error(
                            span,
                            format!(
                                "internal error: `{key}`'s step takes {} contexts and the                                  call names {}",
                                ctx_at.len(),
                                from.len()
                            ),
                            "this is a toolchain bug; report it",
                        );
                        return None;
                    }
                    for (at, arg) in ctx_at.iter().copied().zip(from) {
                        let cty = code.ty_of(arg);
                        let cslots = repr::ir_slots(&mut self.reprs, self.program, cty);
                        if cslots.is_empty() {
                            continue;
                        }
                        let value = self.get(state, arg);
                        let cpieces = repr::disassemble(&self.builder, &cslots, value);
                        let align = match cty {
                            ir::Type::Agg(id) => self.reprs.of(self.program, id).layout.align,
                            _ => 8,
                        };
                        let into = repr::byte_offset(
                            self.ctx,
                            &self.builder,
                            record,
                            i64::from(at),
                            "step.ctx.p",
                        );
                        self.store_slots(into, &cslots, align, &cpieces);
                    }
                    let thunk = self.entry_thunk(&ps, &r, step_index);
                    let (Some(inn), Some(outn)) = (element.clone(), elements.answer.clone())
                    else {
                        self.error(
                            span,
                            format!("internal error: `{key}` has a step and no element types"),
                            "this is a toolchain bug; report it",
                        );
                        return None;
                    };
                    let (a, b) = (self.reprs.stride_of(&inn), self.reprs.stride_of(&outn));
                    let word = self.ctx.i64_type();
                    argv.push(function_pointer(thunk).into());
                    argv.push(record.into());
                    argv.push(word.const_int(u64::from(a), false).into());
                    argv.push(word.const_int(u64::from(b), false).into());
                }
                runtime::Arg::Spilled => {
                    let (size, align) = match &element {
                        Some(t) => {
                            let r = self.reprs.of_ty(t);
                            (r.layout.size, r.layout.align)
                        }
                        None => (0, 1),
                    };
                    let buf = self.scratch(state, size, align);
                    self.store_slots(buf, &slots, align, &pieces);
                    argv.push(buf.into());
                }
                other => {
                    if pieces.len() != other.leaves() {
                        self.error(
                            span,
                            format!(
                                "internal error: `{key}` argument {cursor} is {} words and \
                                 the runtime contract says {}",
                                pieces.len(),
                                other.leaves()
                            ),
                            "this is a toolchain bug; report it",
                        );
                        return None;
                    }
                    for piece in pieces {
                        argv.push(piece.into());
                    }
                }
            }
        }
        Some(argv)
    }

    /// The call itself, and the four shapes a result comes back in.
    fn call_entry(
        &mut self,
        state: &mut Function<'ctx>,
        code: &ir::Code,
        dests: &[ir::ValueId],
        entry: &runtime::Entry,
        mut argv: Vec<BasicMetadataValueEnum<'ctx>>,
        span: Span,
    ) {
        // The two out-pointer shapes append a buffer to the argument list and
        // then take the result out of it, so they are handled before the
        // declaration is written — the buffer is a parameter.
        match entry.ret {
            runtime::Ret::Out => {
                let Some(dest) = dests.first().copied() else { return };
                self.call_out(state, code, dest, entry.symbol, &mut argv);
                return;
            }
            runtime::Ret::Sum => {
                let Some(dest) = dests.first().copied() else { return };
                self.call_sum(state, code, dest, entry, argv, span);
                return;
            }
            runtime::Ret::Res => {
                let Some(dest) = dests.first().copied() else { return };
                self.call_result(state, code, dest, entry, argv, span);
                return;
            }
            _ => {}
        }
        let param_types: Vec<BasicMetadataTypeEnum<'ctx>> =
            argv.iter().map(|a| metadata_type_of(self.ctx, *a)).collect();
        let ret_type = match entry.ret {
            runtime::Ret::Void
            | runtime::Ret::NoReturn
            | runtime::Ret::Out
            | runtime::Ret::Sum
            | runtime::Ret::Res => None,
            runtime::Ret::Scalar => dests
                .first()
                .map(|d| repr::ir_type(self.ctx, &mut self.reprs, self.program, code.ty_of(*d))),
            runtime::Ret::Int(bits) => Some(self.int_of_width(bits).as_basic_type_enum()),
        };
        let f = self.declare_rt(entry.symbol, &param_types, ret_type);
        if matches!(entry.ret, runtime::Ret::NoReturn) {
            attrs::mark_noreturn(self.ctx, f);
        }
        let Ok(call) = self.builder.build_call(f, &argv, "") else { return };
        attrs::set_call_convention(call, attrs::C);
        // A host capability is the world: the caller keeps `memory(readwrite)`.
        state.observed.opaque = true;
        if matches!(entry.ret, runtime::Ret::NoReturn) {
            attrs::noreturn_call(self.ctx, call);
            let _ = self.builder.build_unreachable();
            let after = self.ctx.append_basic_block(state.value, "after.exit");
            self.builder.position_at_end(after);
            return;
        }
        // `Ret::Int` is the one shape where the C result is not the dest's
        // register shape: `u8` for a `Bool`, `i32` for a bare enum tag. The
        // narrowing happens here rather than in the declaration, because the
        // declaration is what has to agree with the archive.
        if let runtime::Ret::Int(_) = entry.ret {
            let Some(dest) = dests.first().copied() else { return };
            let want = repr::ir_type(self.ctx, &mut self.reprs, self.program, code.ty_of(dest));
            let value = call
                .try_as_basic_value()
                .basic()
                .unwrap_or_else(|| self.ctx.i32_type().const_zero().into());
            let narrowed = self.narrow_int(value, want);
            self.set(state, dest, narrowed);
            return;
        }
        self.bind_results(state, code, dests, call.try_as_basic_value().basic());
    }

    fn abort(&mut self, state: &mut Function<'ctx>, message: &str) {
        let text = self.literal(message.as_bytes());
        let len = self.ctx.i64_type().const_int(message.len() as u64, false);
        let f = self.rt_abort();
        if let Ok(call) = self.builder.build_call(f, &[text.into(), len.into()], "") {
            attrs::set_call_convention(call, attrs::C);
            attrs::noreturn_call(self.ctx, call);
        }
        state.observed.aborts = true;
    }

    /// `buri_rt_abort_assert(kind)`, from a `Str` argument.
    fn abort_assert(&mut self, state: &mut Function<'ctx>, code: &ir::Code, kind: ir::ValueId) {
        self.abort_str(state, code, kind, runtime::ABORT_ASSERT);
    }

    /// Call a `(ptr, len) -> !` runtime entry with a borrowed `Str`'s bytes.
    ///
    /// Ends the current block in `unreachable`, and leaves the builder there.
    /// Where emission continues is the caller's: `report` continues in the block
    /// it branched around this one from, and `failWith` continues in a fresh
    /// one. A block appended here and left without a terminator is malformed IR
    /// that nothing in this pipeline catches before a pass dereferences the
    /// terminator it does not have.
    ///
    /// The stored length is masked: bit 63 is the ASCII flag and not part of
    /// the byte count (VALUE-MODEL.md §3).
    fn abort_str(
        &mut self,
        state: &mut Function<'ctx>,
        code: &ir::Code,
        message: ir::ValueId,
        symbol: &str,
    ) {
        self.abort_strs(state, code, &[message], symbol);
    }

    /// The same, for an entry taking more than one `Str`: the test runner's two
    /// reporting entries take the assertion's kind and the values beside it.
    /// One function rather than three, because the flattening is `lib.rs` §2
    /// rule 1 and there is one of it.
    fn abort_strs(
        &mut self,
        state: &mut Function<'ctx>,
        code: &ir::Code,
        texts: &[ir::ValueId],
        symbol: &str,
    ) {
        self.abort_strs_continuing(state, code, texts, symbol);
        let _ = self.builder.build_unreachable();
    }

    /// The same call, leaving the block open for the terminator the IR says
    /// follows.
    ///
    /// Which of the two a caller wants is a question about the *block*, not
    /// about the callee: a block the IR continues out of has a terminator of
    /// its own, and a second one is malformed IR. `noreturn` on the call is
    /// what tells LLVM the code after it is dead.
    fn abort_strs_continuing(
        &mut self,
        state: &mut Function<'ctx>,
        code: &ir::Code,
        texts: &[ir::ValueId],
        symbol: &str,
    ) {
        let word = self.ctx.i64_type();
        let mut params: Vec<inkwell::types::BasicMetadataTypeEnum<'ctx>> = Vec::new();
        let mut call_args: Vec<inkwell::values::BasicMetadataValueEnum<'ctx>> = Vec::new();
        for text in texts {
            let slots = repr::ir_slots(&mut self.reprs, self.program, code.ty_of(*text));
            let value = self.get(state, *text);
            let pieces = repr::disassemble(&self.builder, &slots, value);
            let (
                Some(BasicValueEnum::PointerValue(ptr)),
                Some(BasicValueEnum::IntValue(raw)),
            ) = (pieces.get(layout::STR_PTR).copied(), pieces.get(layout::STR_LEN).copied())
            else {
                return;
            };
            let len = self
                .builder
                .build_and(raw, word.const_int(STR_LEN_MASK, false), "assert.bytes")
                .unwrap_or(raw);
            params.push(self.ptr_ty().into());
            params.push(word.into());
            call_args.push(ptr.into());
            call_args.push(len.into());
        }
        let f = self.declare_rt(symbol, &params, None);
        attrs::mark_noreturn(self.ctx, f);
        if let Ok(call) = self.builder.build_call(f, &call_args, "") {
            attrs::set_call_convention(call, attrs::C);
            attrs::noreturn_call(self.ctx, call);
        }
        state.observed.aborts = true;
    }

    /// A fresh block after a call that does not return, for the instructions the
    /// IR still names.
    fn after_abort(&mut self, state: &mut Function<'ctx>) {
        let after = self.ctx.append_basic_block(state.value, "assert.after");
        self.builder.position_at_end(after);
    }

    // -----------------------------------------------------------------------
    // Reference counting — MEMORY.md §5.1, open-coded
    // -----------------------------------------------------------------------

    /// `incref` and `decref` of an SSA value, over every count it owns.
    ///
    /// Driven by [`repr::Reprs::sites`] rather than by the slot list. The two
    /// agree about a `Str`, a `[T]` and a struct of those, and they disagree
    /// about exactly the two shapes a slot cannot express: a tagged enum's
    /// payload is one opaque `Blob` and a boxed field is a pointer whose
    /// pointee has a type of its own. A slot-driven walk therefore skipped
    /// every count inside a `Result<Str, E>` — silently, because a slot marked
    /// "not counted" reads the same as a slot that has nothing in it.
    ///
    /// `Inst::DecRef`'s own `drop` field is ignored: it is `None` at every
    /// construction site in the landed IR (`lower.rs`), and the glue is
    /// derivable here from the type, which is where `middle::layout` already
    /// put the answer.
    fn incref(&mut self, state: &mut Function<'ctx>, code: &ir::Code, v: ir::ValueId) {
        self.rc(state, code, v, true);
    }

    fn decref(
        &mut self,
        state: &mut Function<'ctx>,
        code: &ir::Code,
        v: ir::ValueId,
        drop: Option<FuncIdx>,
    ) {
        let _ = drop;
        self.rc(state, code, v, false);
    }

    fn rc(
        &mut self,
        state: &mut Function<'ctx>,
        code: &ir::Code,
        v: ir::ValueId,
        retain: bool,
    ) {
        let Some(ty) = self.type_of(code.ty_of(v)) else { return };
        if !self.reprs.counted_type(&ty) {
            return;
        }
        let slots = repr::ir_slots(&mut self.reprs, self.program, code.ty_of(v));
        let value = self.get(state, v);
        let pieces = repr::disassemble(&self.builder, &slots, value);
        let place = Place::Registers { slots, pieces };
        self.walk_rc(state, &ty, &place, 0, retain, 0);
        // The same split `observe::local` makes from the IR, made again here
        // through the same function so that the two cannot answer it
        // differently: a count in a parameter's block is `argmem`, and a count
        // anywhere else is the default location.
        if state.based.get(v.index()).copied().unwrap_or(false) {
            state.observed.writes_args = true;
        } else {
            state.observed.reads_far = true;
            state.observed.writes_far = true;
        }
        if !retain {
            state.observed.opaque = true;
        }
    }

    /// The walk itself, over a value wherever it is.
    ///
    /// `base` is a byte offset into `place`, added to every site's own. That is
    /// what lets one walk serve both forms: a nested field is *not* rebased to
    /// a new address, because an SSA value has no address to rebase
    /// (CODEGEN-LLVM.md §2.2) — so the offset travels instead of the pointer,
    /// and the memory form gets the same treatment rather than a second
    /// spelling.
    fn walk_rc(
        &mut self,
        state: &mut Function<'ctx>,
        ty: &Ty,
        place: &Place<'ctx>,
        base: u32,
        retain: bool,
        depth: u32,
    ) {
        if depth > repr::RC_DEPTH || !self.reprs.counted_type(ty) {
            return;
        }
        for site in self.reprs.sites(ty) {
            match site {
                Site::Block { offset, glue, counted } => {
                    let at = base.saturating_add(offset);
                    let Some(p) = self.place_pointer(place, at) else { continue };
                    if retain {
                        self.incref_pointer(state, p, counted);
                    } else {
                        let g = self.glue_pointer(&glue);
                        self.decref_pointer(state, p, counted, g);
                    }
                }
                Site::Nested { offset, ty } => {
                    let at = base.saturating_add(offset);
                    self.walk_rc(state, &ty, place, at, retain, depth.saturating_add(1));
                }
                Site::Boxed { offset, ty } => {
                    let at = base.saturating_add(offset);
                    let Some(p) = self.place_pointer(place, at) else { continue };
                    if retain {
                        self.incref_pointer(state, p, Counted::NonNull);
                    } else {
                        let g = self.release_glue(&ty).map(function_pointer);
                        self.decref_pointer(state, p, Counted::NonNull, g);
                    }
                }
                site @ Site::Tagged { .. } => {
                    self.tagged_rc(state, place, base, &site, retain, depth);
                }
                // The niche: `.None` is the payload's own pointer set to null
                // (VALUE-MODEL.md §6), so the payload is walked only where it
                // is not — and the *test* is the discriminant, so there is no
                // tag to read.
                Site::Guarded { null_at, ty } => {
                    let at = base.saturating_add(null_at);
                    let Some(p) = self.place_pointer(place, at) else { continue };
                    let live = self.ctx.append_basic_block(state.value, "rc.some");
                    let done = self.ctx.append_basic_block(state.value, "rc.done");
                    let Ok(is_null) = self.builder.build_is_null(p, "rc.isnone") else { continue };
                    let _ = self.builder.build_conditional_branch(is_null, done, live);
                    self.builder.position_at_end(live);
                    self.walk_rc(state, &ty, place, base, retain, depth.saturating_add(1));
                    let _ = self.builder.build_unconditional_branch(done);
                    self.builder.position_at_end(done);
                }
            }
        }
    }

    /// A tagged enum: one `switch` on the discriminant, and one arm per variant
    /// that has anything to release.
    ///
    /// One block per *variant* rather than per field, so a variant carrying two
    /// `Str`s is one case and not two; and no arm at all for a variant carrying
    /// nothing, which the `switch`'s default covers.
    fn tagged_rc(
        &mut self,
        state: &mut Function<'ctx>,
        place: &Place<'ctx>,
        base: u32,
        site: &Site,
        retain: bool,
        depth: u32,
    ) {
        let Site::Tagged { tag, variants } = site else { return };
        let Some(raw) = self.place_int(place, base, *tag) else { return };
        let i32t = self.ctx.i32_type();
        let key = self
            .builder
            .build_int_z_extend_or_bit_cast(raw, i32t, "rc.tag")
            .unwrap_or_else(|_| i32t.const_zero());
        let done = self.ctx.append_basic_block(state.value, "rc.join");
        let mut by_variant: Vec<(u32, Vec<VariantField>)> = Vec::new();
        for (v, ty, offset, boxed) in variants {
            match by_variant.iter_mut().find(|(k, _)| *k == *v) {
                Some((_, fields)) => fields.push((ty.clone(), *offset, *boxed)),
                None => by_variant.push((*v, vec![(ty.clone(), *offset, *boxed)])),
            }
        }
        let mut arms = Vec::with_capacity(by_variant.len());
        for (v, fields) in &by_variant {
            let bb = self.ctx.append_basic_block(state.value, "rc.arm");
            arms.push((i32t.const_int(u64::from(*v), false), bb, fields.clone()));
        }
        // The default is the join and not an `unreachable`: a variant with
        // nothing counted in it has no arm, and reaching the default is the
        // ordinary way that happens.
        let table: Vec<(IntValue<'ctx>, BasicBlock<'ctx>)> =
            arms.iter().map(|(k, bb, _)| (*k, *bb)).collect();
        let _ = self.builder.build_switch(key, done, &table);
        for (_, bb, fields) in arms {
            self.builder.position_at_end(bb);
            for (ty, offset, boxed) in fields {
                let at = base.saturating_add(offset);
                if boxed {
                    // The field *is* the pointer, so this is one reference
                    // operation on it and no descent — `Site::Boxed`'s arm,
                    // reached through a tag test.
                    if let Some(p) = self.place_pointer(place, at) {
                        if retain {
                            self.incref_pointer(state, p, Counted::NonNull);
                        } else {
                            let g = self.release_glue(&ty).map(function_pointer);
                            self.decref_pointer(state, p, Counted::NonNull, g);
                        }
                    }
                    continue;
                }
                self.walk_rc(state, &ty, place, at, retain, depth.saturating_add(1));
            }
            let _ = self.builder.build_unconditional_branch(done);
        }
        self.builder.position_at_end(done);
    }

    /// One piece of a value at a byte offset, however the value is held.
    ///
    /// The register form answers from the slot list where it can, which keeps a
    /// pointer typed as a pointer; where it cannot, the byte is inside a tagged
    /// enum's payload blob and is shifted out, which is the same
    /// `ptrtoint`/`inttoptr` round trip `get_payload` already pays for that
    /// shape (see `repr.rs`'s header).
    fn place_piece(
        &self,
        place: &Place<'ctx>,
        at: u32,
        want: SlotTy,
    ) -> Option<BasicValueEnum<'ctx>> {
        match place {
            Place::Memory { base, align } => {
                let p = repr::byte_offset(self.ctx, &self.builder, *base, i64::from(at), "rc.p");
                let ty = repr::slot_type(self.ctx, want);
                let v = self.builder.build_load(ty, p, "rc.v").ok()?;
                if let Some(instr) = v.as_instruction_value() {
                    let _ =
                        instr.set_alignment(repr::access_align(*align, Slot { offset: at, ty: want }));
                }
                Some(v)
            }
            Place::Registers { slots, pieces } => {
                for (i, slot) in slots.iter().enumerate() {
                    if slot.offset == at && slot.ty == want {
                        return pieces.get(i).copied();
                    }
                }
                for (i, slot) in slots.iter().enumerate() {
                    let end = slot.offset.saturating_add(slot.ty.size());
                    if !matches!(slot.ty, SlotTy::Blob(_)) || at < slot.offset || at >= end {
                        continue;
                    }
                    let Some(BasicValueEnum::IntValue(blob)) = pieces.get(i).copied() else {
                        continue;
                    };
                    let shift = u64::from(at.saturating_sub(slot.offset)).saturating_mul(8);
                    let moved = self
                        .builder
                        .build_right_shift(
                            blob,
                            blob.get_type().const_int(shift, false),
                            false,
                            "rc.sh",
                        )
                        .ok()?;
                    let narrow = repr::blob_type(self.ctx, want.size());
                    let cut =
                        self.builder.build_int_truncate_or_bit_cast(moved, narrow, "rc.cut").ok()?;
                    return Some(repr::slot_from_bits(
                        self.ctx,
                        &self.builder,
                        Slot { offset: at, ty: want },
                        cut,
                    ));
                }
                None
            }
        }
    }

    fn place_pointer(&self, place: &Place<'ctx>, at: u32) -> Option<PointerValue<'ctx>> {
        match self.place_piece(place, at, SlotTy::Scalar(Scalar::Ptr))? {
            BasicValueEnum::PointerValue(p) => Some(p),
            _ => None,
        }
    }

    fn place_int(&self, place: &Place<'ctx>, at: u32, scalar: Scalar) -> Option<IntValue<'ctx>> {
        match self.place_piece(place, at, SlotTy::Scalar(scalar))? {
            BasicValueEnum::IntValue(v) => Some(v),
            _ => None,
        }
    }

    /// `incref`: a **saturating** increment of the header count.
    ///
    /// MEMORY.md §5.1's sequence exactly — a load, a saturating add, a store,
    /// branchless after the null test. `IMMORTAL` is `u64::MAX` and saturation
    /// keeps it there without a branch, which is what makes a literal `Str` and
    /// an interned constant free.
    fn incref_pointer(
        &mut self,
        state: &mut Function<'ctx>,
        p: PointerValue<'ctx>,
        kind: Counted,
    ) {
        let (body, join) = match kind {
            // Null checks are eliminated wherever the layout says the pointer
            // is non-null, which is only the indirection a recursive type's
            // field is behind (MEMORY.md §5.1).
            Counted::NonNull => (None, None),
            Counted::Nullable => {
                let body = self.ctx.append_basic_block(state.value, "inc.some");
                let join = self.ctx.append_basic_block(state.value, "inc.done");
                let Ok(is_null) = self.builder.build_is_null(p, "isnull") else { return };
                let _ = self.builder.build_conditional_branch(is_null, join, body);
                self.builder.position_at_end(body);
                (Some(body), Some(join))
            }
        };
        let word = self.ctx.i64_type();
        let header =
            repr::byte_offset(self.ctx, &self.builder, p, i64::from(HEADER_RC_OFFSET), "rc.p");

        // === G2 begin: the shared fork (MEMORY.md §5.1, VALUE-MODEL.md §2.1) =
        // `cap`'s bit 63 says the block may be reached from more than one
        // thread, and it chooses which of the two counts below runs. It is
        // never set today, so the whole of what the fork costs a shipping
        // program is itself: one load, from the same 16-byte header line the
        // count is already in, and one bit test whose answer never changes.
        let shared = self.fork_on_shared(state, p, "inc");
        let meet = self.ctx.append_basic_block(state.value, "inc.meet");
        // === G2 end =========================================================

        let Ok(BasicValueEnum::IntValue(rc)) = self.builder.build_load(word, header, "rc") else {
            let _ = self.builder.build_unconditional_branch(meet);
            self.builder.position_at_end(meet);
            return;
        };
        if let Some(instr) = rc.as_instruction() {
            let _ = instr.set_alignment(8);
        }
        let one = word.const_int(1, false);
        let bumped = self.builder.build_int_add(rc, one, "rc.inc").unwrap_or(rc);
        let immortal = word.const_int(IMMORTAL, false);
        // The saturation: `rc == IMMORTAL ? IMMORTAL : rc + 1`. One `cmov` on
        // x86-64, one `csinv` on aarch64, and `IMMORTAL` stays `IMMORTAL`.
        let is_immortal = self
            .builder
            .build_int_compare(IntPredicate::EQ, rc, immortal, "rc.imm")
            .unwrap_or_else(|_| self.ctx.bool_type().const_zero());
        let next = self
            .builder
            .build_select(is_immortal, immortal, bumped, "rc.sat")
            .unwrap_or_else(|_| bumped.into());
        if let Ok(store) = self.builder.build_store(header, next) {
            let _ = store.set_alignment(8);
        }
        let _ = self.builder.build_unconditional_branch(meet);

        // === G2 begin: the atomic increment =================================
        // A **call**, where the unshared arm is open-coded, and the asymmetry
        // is the point. `buri_rt_incref` forks on the same bit and takes the
        // same atomic arm, so there is one atomic sequence in the tree per
        // backend rather than two — and the sequence is cold by construction,
        // because nothing sets the bit.
        //
        // It is also **half the price**. Open-coding the saturating
        // `atomicrmw` here instead puts it in front of `opt` at every
        // reference operation in the program, and that was measured: the
        // open-coded form cost a median **+46 %** of native release lowering
        // where this one costs **+21 %** (design/PERFORMANCE.md §6.6). The
        // machine code on the arm every program actually takes is the same two
        // instructions either way.
        self.builder.position_at_end(shared);
        let incref = self.rt_incref();
        if let Ok(call) = self.builder.build_call(incref, &[p.into()], "") {
            attrs::set_call_convention(call, attrs::C);
            attrs::cold_call(self.ctx, call);
        }
        let _ = self.builder.build_unconditional_branch(meet);
        // === G2 end =========================================================

        self.builder.position_at_end(meet);
        if let (Some(_), Some(join)) = (body, join) {
            let _ = self.builder.build_unconditional_branch(join);
            self.builder.position_at_end(join);
        }
    }

    // === G2 begin: the three pieces both counts share =======================

    /// The fork on `cap`'s bit 63: emit the test, and leave the builder in the
    /// **unshared** arm.
    ///
    /// Returns the block the caller fills with the atomic form; where the two
    /// arms rejoin is the caller's business, because the increment's arms meet
    /// and the decrement's do not (its two both run into `dec.free`).
    ///
    /// The load is of `cap` at `p - 8`, which shares a cache line with the
    /// count at `p - 16` by construction (the header is 16 bytes and the
    /// payload is 16-byte aligned, VALUE-MODEL.md §2), so on a block whose
    /// count is about to be touched anyway the fork's memory cost is nothing.
    /// The test is a sign test on a 64-bit word: `tbz`/`tbnz` on aarch64, a
    /// `js`/`jns` on x86-64.
    ///
    /// The unshared arm is the *false* arm, so it is the fallthrough, and the
    /// branch carries a weight saying so. Today that is exactly right — G1
    /// reserved the bit and nothing sets it — and after G3 it is still right:
    /// a value that crosses a task boundary is the exception.
    fn fork_on_shared(
        &mut self,
        state: &mut Function<'ctx>,
        p: PointerValue<'ctx>,
        tag: &str,
    ) -> BasicBlock<'ctx> {
        let word = self.ctx.i64_type();
        let plain = self.ctx.append_basic_block(state.value, &format!("{tag}.plain"));
        let shared = self.ctx.append_basic_block(state.value, &format!("{tag}.shared"));
        let capp = repr::byte_offset(
            self.ctx,
            &self.builder,
            p,
            i64::from(HEADER_CAP_OFFSET),
            &format!("{tag}.capp"),
        );
        let is_shared = match self.builder.build_load(word, capp, &format!("{tag}.cap")) {
            Ok(BasicValueEnum::IntValue(cap)) => {
                if let Some(instr) = cap.as_instruction() {
                    let _ = instr.set_alignment(8);
                }
                let bit = self
                    .builder
                    .build_and(cap, word.const_int(CAP_SHARED_FLAG, false), &format!("{tag}.mt"))
                    .unwrap_or_else(|_| word.const_zero());
                self.builder
                    .build_int_compare(
                        IntPredicate::NE,
                        bit,
                        word.const_zero(),
                        &format!("{tag}.isshared"),
                    )
                    .unwrap_or_else(|_| self.ctx.bool_type().const_zero())
            }
            // A load that cannot be built leaves the fork unanswerable, and
            // the unshared arm is the answer that matches every block the
            // program can produce today.
            _ => self.ctx.bool_type().const_zero(),
        };
        if let Ok(br) = self.builder.build_conditional_branch(is_shared, shared, plain) {
            self.unlikely(br);
        }
        self.builder.position_at_end(plain);
        shared
    }

    /// Say that a two-way branch takes its *false* arm.
    ///
    /// `!prof` branch weights, which is how to tell LLVM which arm is hot
    /// without an `llvm.expect` call it then has to fold away. The numbers are
    /// the ones clang emits for `__builtin_expect`.
    fn unlikely(&self, br: InstructionValue<'ctx>) {
        let word = self.ctx.i32_type();
        let node = self.ctx.metadata_node(&[
            self.ctx.metadata_string("branch_weights").into(),
            word.const_int(1, false).into(),
            word.const_int(2000, false).into(),
        ]);
        let _ = br.set_metadata(node, self.ctx.get_kind_id("prof"));
    }

    // === G2 end ============================================================

    /// `decref`: the decrement, with the free on the cold path.
    ///
    /// MEMORY.md §5.1:
    ///
    /// ```text
    ///   if p == null: return
    ///   rc = load p[-16]
    ///   if rc == IMMORTAL: return
    ///   if rc == 1: drop_T(p); free(p); return
    ///   store p[-16] = rc - 1
    /// ```
    ///
    /// `drop_T` is `glue`, and it is what makes the free complete: the block
    /// goes back to the allocator *after* whatever it held has been released,
    /// so a `[Str]` that dies takes its strings with it. `None` is still the
    /// common answer — a `Str`'s bytes, an `[Int]`, a struct of scalars hold
    /// nothing — and it costs no call.
    ///
    /// The free path gets `cold` on the call, which is the highest-value item
    /// on CODEGEN-LLVM.md §6's list: reference counting puts a rarely-taken
    /// branch next to *every* value that dies.
    fn decref_pointer(
        &mut self,
        state: &mut Function<'ctx>,
        p: PointerValue<'ctx>,
        kind: Counted,
        glue: Option<PointerValue<'ctx>>,
    ) {
        let join = self.ctx.append_basic_block(state.value, "dec.done");
        if matches!(kind, Counted::Nullable) {
            let live = self.ctx.append_basic_block(state.value, "dec.some");
            let Ok(is_null) = self.builder.build_is_null(p, "isnull") else { return };
            let _ = self.builder.build_conditional_branch(is_null, join, live);
            self.builder.position_at_end(live);
        }
        let word = self.ctx.i64_type();
        let header =
            repr::byte_offset(self.ctx, &self.builder, p, i64::from(HEADER_RC_OFFSET), "rc.p");

        // === G2 begin: the shared fork ======================================
        // The same fork the increment takes, and for the same reason. The two
        // arms rejoin at `dec.free`, not at `dec.meet`: whichever count found
        // the block dead, it dies exactly once and through one copy of the
        // glue call and one copy of `buri_rt_free`.
        let shared = self.fork_on_shared(state, p, "dec");
        // === G2 end =========================================================

        let Ok(BasicValueEnum::IntValue(rc)) = self.builder.build_load(word, header, "rc") else {
            let _ = self.builder.build_unconditional_branch(join);
            self.builder.position_at_end(join);
            return;
        };
        if let Some(instr) = rc.as_instruction() {
            let _ = instr.set_alignment(8);
        }
        let immortal = word.const_int(IMMORTAL, false);
        let counted_block = self.ctx.append_basic_block(state.value, "dec.counted");
        let is_immortal = self
            .builder
            .build_int_compare(IntPredicate::EQ, rc, immortal, "rc.imm")
            .unwrap_or_else(|_| self.ctx.bool_type().const_zero());
        let _ = self.builder.build_conditional_branch(is_immortal, join, counted_block);

        self.builder.position_at_end(counted_block);
        let one = word.const_int(1, false);
        let last = self
            .builder
            .build_int_compare(IntPredicate::EQ, rc, one, "rc.last")
            .unwrap_or_else(|_| self.ctx.bool_type().const_zero());
        let free_block = self.ctx.append_basic_block(state.value, "dec.free");
        let live_block = self.ctx.append_basic_block(state.value, "dec.live");
        let _ = self.builder.build_conditional_branch(last, free_block, live_block);

        // === G2 begin: the atomic decrement =================================
        // A cold call to `buri_rt_decref`, for `incref`'s reasons, and it
        // carries the glue pointer because the runtime owns the whole of the
        // dying arm on that side: the `atomicrmw sub` whose *returned* value
        // decides who frees, the glue call, and the free. Two threads that
        // each read `1` from a separate load would each free the block, which
        // is why that decision cannot be split across a call boundary and the
        // free path here is not reused.
        //
        // `glue` is null for a type holding no counted references, which is
        // the same null `buri_rt_decref` already documents.
        self.builder.position_at_end(shared);
        let decref = self.rt_decref();
        let glue_arg = glue.unwrap_or_else(|| self.ptr_ty().const_null());
        if let Ok(call) = self.builder.build_call(decref, &[p.into(), glue_arg.into()], "") {
            attrs::set_call_convention(call, attrs::C);
            attrs::cold_call(self.ctx, call);
        }
        let _ = self.builder.build_unconditional_branch(join);
        // === G2 end =========================================================

        self.builder.position_at_end(free_block);
        if let Some(glue) = glue {
            let ty = self.ctx.void_type().fn_type(&[self.ptr_ty().into()], false);
            if let Ok(call) = self.builder.build_indirect_call(ty, glue, &[p.into()], "") {
                attrs::set_call_convention(call, attrs::C);
                attrs::cold_call(self.ctx, call);
            }
        }
        let free = self.rt_free();
        if let Ok(call) = self.builder.build_call(free, &[p.into()], "") {
            attrs::set_call_convention(call, attrs::C);
            attrs::cold_call(self.ctx, call);
        }
        let _ = self.builder.build_unconditional_branch(join);

        self.builder.position_at_end(live_block);
        let next = self.builder.build_int_sub(rc, one, "rc.dec").unwrap_or(rc);
        if let Ok(store) = self.builder.build_store(header, next) {
            let _ = store.set_alignment(8);
        }
        let _ = self.builder.build_unconditional_branch(join);

        self.builder.position_at_end(join);
        state.observed.opaque = true;
    }
}

// ---------------------------------------------------------------------------
// The generated helpers — closures, and the drop glue
// ---------------------------------------------------------------------------

/// Where a value's bytes are, for the purpose of finding the counts in them.
enum Place<'ctx> {
    /// An SSA value: its slots, and the piece for each. There is no address
    /// (CODEGEN-LLVM.md §2.2), so a pointer at a byte offset is found by
    /// matching the offset against a slot rather than by loading one.
    Registers { slots: Vec<Slot>, pieces: Vec<BasicValueEnum<'ctx>> },
    /// A heap block or a stack aggregate, at the alignment its layout claims.
    Memory { base: PointerValue<'ctx>, align: u32 },
}

/// One `Checked` or `Saturating` operation, bundled.
///
/// A struct rather than five parameters because the lint set caps a signature
/// at seven, and the cap is right: `checked` and `saturating` differ in *how*
/// they treat the same four things, so the four travelling together is what the
/// pair is actually about.
#[derive(Clone, Copy)]
struct Wide<'o, 'ctx> {
    op: &'o str,
    prim: Prim,
    x: IntValue<'ctx>,
    y: IntValue<'ctx>,
}

/// A helper whose declaration exists and whose body does not yet.
enum Job<'ctx> {
    /// A closure's `code`: converts an environment *pointer* into the
    /// environment *leaves* the lifted lambda declares.
    Thunk { value: FunctionValue<'ctx>, func: FuncIdx, env: bool },
    /// Release the contents of one value of this type.
    Release { value: FunctionValue<'ctx>, ty: Ty },
    /// Release every element of a `[T]` block.
    ReleaseElems { value: FunctionValue<'ctx>, elem: Ty },
    /// Take a reference on everything *one* element of a `[T]` holds.
    RetainElem { value: FunctionValue<'ctx>, elem: Ty },
    /// Read a function pointer out of a block's first word and call it on the
    /// rest: the glue every closure environment shares.
    EnvGlue { value: FunctionValue<'ctx> },
    /// The C-ABI entry thunk a **runtime-driven step** is reached through:
    /// `void(state, index, arg, out)`, which runs one closure once on one
    /// element. `params` and `ret` are that closure's own signature, and
    /// `index` is which of `params` the runtime's loop counter goes into.
    Entry { value: FunctionValue<'ctx>, params: Vec<Ty>, ret: Ty, index: Option<usize> },
}

/// The element types one runtime entry's generic parameters are read at.
///
/// `source` is the `[T]` the runtime walks — `cli/runtime/lib.rs` §2 rule 4's
/// stride — and `answer` is the one it builds. They are the same type at every
/// entry but a **step**, which reads a `[A]` and writes a `[B]`.
#[derive(Clone, Default)]
struct Elements {
    source: Option<Ty>,
    answer: Option<Ty>,
}

/// Where a step's context arguments begin inside its **state record**.
///
/// The record is `{ code, env, ctx... }` — the closure's two words, in
/// `middle::layout`'s order, and then whatever the step's context arguments
/// weigh. It is written by [`Unit::entry_args`] and read by
/// [`Unit::build_entry`], and by nothing between them: the runtime is handed
/// the address and hands it back.
///
/// The frame-threaded backend's record (`stencil/glue.rs`) has a third word
/// this one does not — the Buri frame the thunk is to work in — because a frame
/// there is a byte offset the call site reserves and here it is the machine's.
/// Two shapes for one idea is right: nothing reads both.
const STEP_CTX: u32 = 16;

fn function_pointer<'ctx>(f: FunctionValue<'ctx>) -> PointerValue<'ctx> {
    f.as_global_value().as_pointer_value()
}

impl<'ctx, 'a> Unit<'ctx, 'a> {
    /// Builds every helper body that has been asked for, and every one those
    /// ask for in turn.
    ///
    /// Called once, after the unit's functions are emitted. A helper cannot be
    /// built where it is asked for, because it is asked for in the middle of
    /// another function's body and there is one builder; and it cannot be
    /// skipped, because a declared function with no body is a link error rather
    /// than a wrong answer.
    pub fn finish(&mut self) {
        while let Some(job) = self.pending.pop() {
            self.define_helper(job);
        }
    }

    /// One `void(ptr)` helper, declared and registered but not yet built.
    ///
    /// `Private` linkage, so two units that both need the glue for `[Str]` get
    /// a copy each and no symbol collides — duplication rather than a shared
    /// unit, because a shared unit would be a link-order dependency for a few
    /// hundred bytes.
    fn glue_function(&mut self, what: &str) -> FunctionValue<'ctx> {
        let ty = self.ctx.void_type().fn_type(&[self.ptr_ty().into()], false);
        let name = format!("buri.{what}.{}", self.helpers);
        self.helpers = self.helpers.saturating_add(1);
        let f = self.module.add_function(&name, ty, Some(Linkage::Private));
        // `ccc`: `cli/runtime/list.rs` takes the retain glue as a plain C
        // function pointer, and a closure environment's glue is reached through
        // an indirect call whose signature is written at the call site. One
        // convention for all of them, so no call site has to know which.
        attrs::set_convention(f, attrs::C);
        f
    }

    /// The function that releases the contents of a block holding one value of
    /// this type, or `None` where there is nothing to release.
    fn release_glue(&mut self, ty: &Ty) -> Option<FunctionValue<'ctx>> {
        if !self.reprs.counted_type(ty) {
            return None;
        }
        if let Some(f) = self.releases.get(ty) {
            return Some(*f);
        }
        let f = self.glue_function("release");
        self.releases.insert(ty.clone(), f);
        self.pending.push(Job::Release { value: f, ty: ty.clone() });
        Some(f)
    }

    /// The same for a `[T]` block, whose element count is `cap / stride`.
    fn release_elems_glue(&mut self, elem: &Ty) -> Option<FunctionValue<'ctx>> {
        if !self.reprs.counted_type(elem) {
            return None;
        }
        if let Some(f) = self.release_elems.get(elem) {
            return Some(*f);
        }
        let f = self.glue_function("release_elems");
        self.release_elems.insert(elem.clone(), f);
        self.pending.push(Job::ReleaseElems { value: f, elem: elem.clone() });
        Some(f)
    }

    /// The mirror: the function that takes a reference on everything one
    /// element holds, or `None` where it holds nothing counted.
    ///
    /// `None` is the answer for `[Int]`, `[U8]` and every struct of scalars,
    /// which is most of them — and it reaches `cli/runtime/list.rs` as a null
    /// pointer, so the common case costs a copy and no per-element call at all.
    fn retain_glue(&mut self, elem: &Ty) -> Option<FunctionValue<'ctx>> {
        if !self.reprs.counted_type(elem) {
            return None;
        }
        if let Some(f) = self.retains.get(elem) {
            return Some(*f);
        }
        let f = self.glue_function("retain_elem");
        self.retains.insert(elem.clone(), f);
        self.pending.push(Job::RetainElem { value: f, elem: elem.clone() });
        Some(f)
    }

    /// The C-ABI entry thunk for one step signature.
    ///
    /// `void(ptr state, i64 index, ptr arg, ptr out)` at `ccc` — the shape
    /// `cli/runtime/list.rs`'s `StepEntry` declares, and the same four
    /// arguments the frame-threaded backend's `Helper::Entry` takes. One per
    /// `(params, ret, index)`, because that is what the body depends on and a
    /// second call site at the same element types wants the same function. The
    /// index *position* is in the key rather than derived from the types: two
    /// steps can take the same types and mean different things by the second
    /// parameter.
    fn entry_thunk(
        &mut self,
        params: &[Ty],
        ret: &Ty,
        index: Option<usize>,
    ) -> FunctionValue<'ctx> {
        let key = (params.to_vec(), ret.clone(), index);
        if let Some(f) = self.entries.get(&key) {
            return *f;
        }
        let p = self.ptr_ty();
        let word = self.ctx.i64_type();
        let ty = self.ctx.void_type().fn_type(&[p.into(), word.into(), p.into(), p.into()], false);
        let name = format!("buri.entry.{}", self.helpers);
        self.helpers = self.helpers.saturating_add(1);
        let f = self.module.add_function(&name, ty, Some(Linkage::Private));
        // `ccc`: the runtime holds this as a plain C function pointer. It is
        // the *only* thing in a Buri artifact the runtime calls into, and it is
        // one convention for the same reason the glue above is.
        attrs::set_convention(f, attrs::C);
        self.entries.insert(key, f);
        self.pending.push(Job::Entry {
            value: f,
            params: params.to_vec(),
            ret: ret.clone(),
            index,
        });
        f
    }

    /// [`Unit::entry_thunk`]'s body: three pointers and a counter in, one Buri
    /// call.
    ///
    /// This is [`Unit::build_thunk`]'s problem from the other side. A thunk
    /// converts a closure's environment for a Buri caller; this converts *C
    /// pointers* for one, and it is the only place a Buri closure is entered
    /// from outside the artifact.
    ///
    /// The element is loaded, **retained**, and passed. The retain is
    /// `middle/rc.rs`'s "a call through a function value owns its arguments":
    /// the runtime lends the element and keeps its own count, so the step's is
    /// taken here — exactly as [`Unit::list_closure`] takes it before its own
    /// indirect call.
    ///
    /// The **contexts** come out of the record rather than out of `arg`: they
    /// are the same value at every element, and a C signature has no parameter
    /// for one. A zero-sized context is no leaves at all; `TestAlloc`'s handle
    /// is one, and [`STEP_CTX`] is where it was put.
    ///
    /// The **index** comes out of neither. It is the runtime's loop counter,
    /// which changes per element and which nothing on this side of the boundary
    /// can derive, so it is its own C argument and is copied straight into the
    /// parameter `index` names. A step whose closure does not take one — the
    /// `list.mapCtxStep` pilot — leaves the register unread.
    fn build_entry(
        &mut self,
        state: &mut Function<'ctx>,
        params: &[Ty],
        ret: &Ty,
        index: Option<usize>,
    ) {
        let ptr = self.ptr_ty();
        let (Some(record), Some(arg), Some(out)) = (
            state.value.get_nth_param(0).and_then(|p| p.try_into().ok()),
            state.value.get_nth_param(2).and_then(|p| p.try_into().ok()),
            state.value.get_nth_param(3).and_then(|p| p.try_into().ok()),
        ) else {
            let _ = self.builder.build_unreachable();
            return;
        };
        let counter = state.value.get_nth_param(1);
        let record: PointerValue<'ctx> = record;
        let arg: PointerValue<'ctx> = arg;
        let out: PointerValue<'ctx> = out;
        // `{ code, env }`, which is the closure's own two words in the order
        // `middle::layout` gives them (`CLOSURE_CODE`, `CLOSURE_ENV`).
        let code = match self.builder.build_load(ptr, record, "step.code") {
            Ok(BasicValueEnum::PointerValue(v)) => v,
            _ => ptr.const_null(),
        };
        let env_at = repr::byte_offset(self.ctx, &self.builder, record, 8, "step.env.p");
        let env = match self.builder.build_load(ptr, env_at, "step.env") {
            Ok(BasicValueEnum::PointerValue(v)) => v,
            _ => ptr.const_null(),
        };

        let mut param_slots: Vec<Slot> = Vec::new();
        for t in params {
            param_slots.extend(self.reprs.of_ty(t).slots.clone());
        }
        let ret_slots: Vec<Slot> = self.reprs.of_ty(ret).slots.clone();
        let fty = self.closure_fn_type(&param_slots, &ret_slots);

        let mut argv: Vec<BasicMetadataValueEnum<'ctx>> = vec![env.into()];
        let ctx_at = self.step_ctx_offsets(params, index);
        // `ctx_at` is parallel to the parameters that are in the record, which
        // is every one but the element and the index; the walk pairs them up
        // rather than assuming the two lists line up position for position.
        let record_params: Vec<usize> = (0..params.len().saturating_sub(1))
            .filter(|i| index != Some(*i))
            .collect();
        let mut from_record = record_params.into_iter().zip(ctx_at);
        for (i, t) in params.iter().enumerate().take(params.len().saturating_sub(1)) {
            if index == Some(i) {
                // The counter, at its declared width. `Int` is `i64` and the
                // C argument already is one, so this is a move.
                if let Some(v) = counter {
                    argv.push(v.into());
                }
                continue;
            }
            let Some((_, off)) = from_record.next() else { continue };
            let r = self.reprs.of_ty(t);
            let (slots, align) = (r.slots.clone(), r.layout.align);
            if slots.is_empty() {
                continue;
            }
            let at = repr::byte_offset(self.ctx, &self.builder, record, i64::from(off), "step.ctx");
            for piece in self.load_slots(at, &slots, align) {
                argv.push(piece.into());
            }
        }
        if let Some(elem) = params.last() {
            let r = self.reprs.of_ty(elem);
            let (slots, align) = (r.slots.clone(), r.layout.align);
            let pieces = self.load_slots(arg, &slots, align);
            if self.rc_counted(elem) {
                let place = Place::Registers { slots, pieces: pieces.clone() };
                self.walk_rc(state, elem, &place, 0, true, 0);
            }
            argv.extend(pieces.into_iter().map(BasicMetadataValueEnum::from));
        }
        let Ok(call) = self.builder.build_indirect_call(fty, code, &argv, "") else {
            let _ = self.builder.build_unreachable();
            return;
        };
        attrs::set_call_convention(call, attrs::FAST);
        state.observed.opaque = true;
        if let Some(answer) = call.try_as_basic_value().basic() {
            let align = self.reprs.of_ty(ret).layout.align;
            let pieces = repr::disassemble(&self.builder, &ret_slots, answer);
            self.store_slots(out, &ret_slots, align, &pieces);
        }
        let _ = self.builder.build_return(None);
    }

    /// Where each of a step's context arguments sits inside the state record,
    /// and how big the record is.
    ///
    /// One answer, asked twice: by the call site that writes the record and by
    /// the entry thunk that reads it. Two computations of one layout is one of
    /// them being wrong, and nothing would diagnose it — the record is opaque
    /// to everything between the two.
    ///
    /// The last parameter is the element, which travels through `arg`;
    /// `index`, where there is one, travels in its own C argument. Neither is
    /// in the record, so neither takes room in the answer.
    fn step_ctx_offsets(&mut self, params: &[Ty], index: Option<usize>) -> Vec<u32> {
        let mut at = STEP_CTX;
        let mut out = Vec::new();
        for (i, t) in params.iter().enumerate().take(params.len().saturating_sub(1)) {
            if index == Some(i) {
                continue;
            }
            out.push(at);
            let size = self.reprs.of_ty(t).layout.size;
            at = at.saturating_add(size.next_multiple_of(8));
        }
        out
    }

    /// The size of a step's state record: `{ code, env, ctx... }`.
    fn step_state_bytes(&mut self, params: &[Ty], index: Option<usize>) -> u32 {
        let mut at = STEP_CTX;
        for (i, t) in params.iter().enumerate().take(params.len().saturating_sub(1)) {
            if index == Some(i) {
                continue;
            }
            let size = self.reprs.of_ty(t).layout.size;
            at = at.saturating_add(size.next_multiple_of(8));
        }
        at
    }

    fn env_glue(&mut self) -> FunctionValue<'ctx> {
        if let Some(f) = self.env_glue {
            return f;
        }
        let f = self.glue_function("env_glue");
        self.env_glue = Some(f);
        self.pending.push(Job::EnvGlue { value: f });
        f
    }

    /// What `decref` calls before the block goes back.
    fn glue_pointer(&mut self, glue: &Glue) -> Option<PointerValue<'ctx>> {
        match glue {
            Glue::None => None,
            Glue::Env => Some(function_pointer(self.env_glue())),
            Glue::Elems(t) => self.release_elems_glue(t).map(function_pointer),
        }
    }

    /// A closure's `code` pointer: the thunk over `func`.
    fn thunk(&mut self, func: FuncIdx, env: bool) -> Option<FunctionValue<'ctx>> {
        let key = (func.0, env);
        if let Some(f) = self.thunks.get(&key) {
            return Some(*f);
        }
        let f = self.program.funcs.get(func.index())?;
        let (sig_params, sig_rets) = (f.sig.params.clone(), f.sig.rets.clone());
        let mut params: Vec<BasicMetadataTypeEnum<'ctx>> = vec![self.ptr_ty().into()];
        for t in sig_params.iter().skip(usize::from(env)) {
            for slot in repr::ir_slots(&mut self.reprs, self.program, *t) {
                params.push(repr::slot_type(self.ctx, slot.ty).into());
            }
        }
        let mut rets = Vec::new();
        for t in &sig_rets {
            rets.extend(repr::ir_slots(&mut self.reprs, self.program, *t));
        }
        let ty = match rets.as_slice() {
            [] => self.ctx.void_type().fn_type(&params, false),
            slots => repr::register_type(self.ctx, slots).fn_type(&params, false),
        };
        let name = format!("buri.thunk.{}", self.helpers);
        self.helpers = self.helpers.saturating_add(1);
        let f = self.module.add_function(&name, ty, Some(Linkage::Private));
        // `fastcc`, unlike the `void(ptr)` glue: this one is called by
        // `call_indirect`, which is a Buri-to-Buri call and uses the Buri
        // convention on both sides (`attrs::set_call_convention`).
        attrs::set_convention(f, attrs::FAST);
        self.thunks.insert(key, f);
        self.pending.push(Job::Thunk { value: f, func, env });
        Some(f)
    }

    /// The type of the indirect call a closure value stands for.
    ///
    /// `(ptr env, args-leaves...) -> rets`, which is the thunk's signature and
    /// not the lifted lambda's — see this file's header. The environment is
    /// passed even where it is null, because the callee is chosen at run time
    /// and the two thunk shapes must be call-compatible.
    fn closure_fn_type(&self, params: &[Slot], rets: &[Slot]) -> FunctionType<'ctx> {
        let mut ps: Vec<BasicMetadataTypeEnum<'ctx>> = vec![self.ptr_ty().into()];
        ps.extend(params.iter().map(|s| -> BasicMetadataTypeEnum<'ctx> {
            repr::slot_type(self.ctx, s.ty).into()
        }));
        match rets {
            [] => self.ctx.void_type().fn_type(&ps, false),
            _ => repr::register_type(self.ctx, rets).fn_type(&ps, false),
        }
    }

    fn define_helper(&mut self, job: Job<'ctx>) {
        let value = match &job {
            Job::Thunk { value, .. }
            | Job::Release { value, .. }
            | Job::ReleaseElems { value, .. }
            | Job::RetainElem { value, .. }
            | Job::EnvGlue { value }
            | Job::Entry { value, .. } => *value,
        };
        let entry = self.ctx.append_basic_block(value, "entry");
        self.builder.position_at_end(entry);
        let mut state = Function {
            value,
            blocks: Vec::new(),
            ends: Vec::new(),
            values: Vec::new(),
            phis: Vec::new(),
            entry,
            divmod: None,
            observed: Observed::clean(),
            based: Vec::new(),
        };
        let first: PointerValue<'ctx> = value
            .get_nth_param(0)
            .and_then(|p| p.try_into().ok())
            .unwrap_or_else(|| self.ptr_ty().const_null());
        match job {
            Job::Thunk { func, env, .. } => {
                self.build_thunk(&mut state, func, env, first);
                return;
            }
            Job::Entry { params, ret, index, .. } => {
                self.build_entry(&mut state, &params, &ret, index);
                return;
            }
            Job::Release { ty, .. } => {
                let align = self.reprs.of_ty(&ty).layout.align;
                let place = Place::Memory { base: first, align };
                self.walk_rc(&mut state, &ty, &place, 0, false, 0);
            }
            Job::RetainElem { elem, .. } => {
                let align = self.reprs.of_ty(&elem).layout.align;
                let place = Place::Memory { base: first, align };
                self.walk_rc(&mut state, &elem, &place, 0, true, 0);
            }
            // The count is `cap / stride`, and `cap` is the second header word
            // (VALUE-MODEL.md §2) — which is what makes a drop glue taking only
            // a pointer enough for a list. Bit 63 of the word is the reserved
            // multi-threaded mark (`layout::CAP_SHARED_FLAG`), so it is masked
            // off before the divide: a set bit would make this walk 2^60
            // elements of a block that holds a handful.
            Job::ReleaseElems { elem, .. } => {
                let stride = self.reprs.stride_of(&elem);
                let word = self.ctx.i64_type();
                let cap_at = repr::byte_offset(
                    self.ctx,
                    &self.builder,
                    first,
                    i64::from(HEADER_CAP_OFFSET),
                    "cap.p",
                );
                let raw = match self.builder.build_load(word, cap_at, "cap.raw") {
                    Ok(BasicValueEnum::IntValue(v)) => v,
                    _ => word.const_zero(),
                };
                let cap = self
                    .builder
                    .build_and(raw, word.const_int(CAP_MASK, false), "cap")
                    .unwrap_or(raw);
                let count = self
                    .builder
                    .build_int_unsigned_div(cap, word.const_int(u64::from(stride), false), "count")
                    .unwrap_or(cap);
                self.each_element(&mut state, first, count, stride, &elem, false);
            }
            Job::EnvGlue { .. } => self.build_env_glue(&mut state, first),
        }
        let _ = self.builder.build_return(None);
    }

    /// `code(env, args...)`, forwarding to the function the closure names.
    ///
    /// With an environment, the record's leaves are loaded out of the block at
    /// [`ENV_FIELDS`] and passed first, which is exactly the aggregate
    /// parameter the lifted lambda declares. Without one, the environment is
    /// ignored: a capture-free lambda is an ordinary `FnRef` by the time it
    /// reaches here (`middle::closures`), and its callee has no environment
    /// parameter at all — but it still needs a thunk, because the *call site*
    /// cannot know which of the two it is holding.
    fn build_thunk(
        &mut self,
        state: &mut Function<'ctx>,
        func: FuncIdx,
        env: bool,
        env_ptr: PointerValue<'ctx>,
    ) {
        let Some(callee) = self.declare(func) else {
            let _ = self.builder.build_unreachable();
            return;
        };
        let Some(first_param) =
            self.program.funcs.get(func.index()).map(|f| f.sig.params.first().copied())
        else {
            let _ = self.builder.build_unreachable();
            return;
        };
        let (sig, own) = match self.program.funcs.get(func.index()) {
            Some(f) => (f.sig.params.clone(), f.facts.params.clone()),
            None => (Vec::new(), Vec::new()),
        };
        let mut argv: Vec<BasicMetadataValueEnum<'ctx>> = Vec::new();
        if env {
            if let Some(first) = first_param {
                let slots = repr::ir_slots(&mut self.reprs, self.program, first);
                let align = match first {
                    ir::Type::Agg(id) => self.reprs.of(self.program, id).layout.align,
                    _ => 8,
                };
                let record = repr::byte_offset(
                    self.ctx,
                    &self.builder,
                    env_ptr,
                    i64::from(ENV_FIELDS),
                    "env.rec",
                );
                // The callee *owns* its environment record, so it drops what
                // the record holds — and the block still holds it too, because
                // the block's own glue releases the record when the closure
                // dies. One of the two counts has to be taken here, or a
                // closure that captured a `Str` and is called once frees it
                // twice.
                if own.first() == Some(&ir::Ownership::Own) {
                    if let Some(ty) = self.type_of(first) {
                        if self.rc_counted(&ty) {
                            let place = Place::Memory { base: record, align };
                            self.walk_rc(state, &ty, &place, 0, true, 0);
                        }
                    }
                }
                for piece in self.load_slots(record, &slots, align) {
                    argv.push(piece.into());
                }
            }
        }
        // An argument the callee only borrows, kept until after the call.
        let mut borrowed: Vec<(Ty, Vec<Slot>, Vec<BasicValueEnum<'ctx>>)> = Vec::new();
        let mut at = 1u32;
        for (j, p) in sig.iter().enumerate().skip(usize::from(env)) {
            let slots = repr::ir_slots(&mut self.reprs, self.program, *p);
            let mut taken = Vec::with_capacity(slots.len());
            for _ in 0..slots.len() {
                if let Some(v) = state.value.get_nth_param(at) {
                    taken.push(v);
                }
                at = at.saturating_add(1);
            }
            argv.extend(taken.iter().map(|v| BasicMetadataValueEnum::from(*v)));
            if own.get(j) != Some(&ir::Ownership::Borrow) {
                continue;
            }
            if let Some(ty) = self.type_of(*p) {
                borrowed.push((ty, slots, taken));
            }
        }
        // Anything the signature did not account for, forwarded verbatim.
        for i in at..state.value.count_params() {
            if let Some(p) = state.value.get_nth_param(i) {
                argv.push(p.into());
            }
        }
        let Ok(call) = self.builder.build_call(callee, &argv, "") else {
            let _ = self.builder.build_unreachable();
            return;
        };
        attrs::set_call_convention(call, attrs::FAST);
        let answer = call.try_as_basic_value().basic();
        // The two ownership conventions meet here and nowhere else. A call
        // through a function value **owns** its arguments — `middle/rc.rs`, and
        // the reason is that a code pointer cannot carry a per-callee
        // convention — so the caller took a count. The callee's own column says
        // whether it takes one too, and where it does not, this releases the
        // caller's. Without it a `list.map` over a `[Str]` leaks one block per
        // element, and so does any ordinary call through a lambda that only
        // reads its parameter.
        for (ty, slots, pieces) in borrowed {
            let place = Place::Registers { slots, pieces };
            self.walk_rc(state, &ty, &place, 0, false, 0);
        }
        match answer {
            Some(v) => {
                let _ = self.builder.build_return(Some(&v as &dyn BasicValue<'ctx>));
            }
            None => {
                let _ = self.builder.build_return(None);
            }
        }
    }

    /// The drop glue every closure environment shares: the block's first word
    /// is the type-specific release function, and the record follows it.
    fn build_env_glue(&mut self, state: &mut Function<'ctx>, block: PointerValue<'ctx>) {
        let Ok(BasicValueEnum::PointerValue(f)) =
            self.builder.build_load(self.ptr_ty(), block, "env.glue")
        else {
            return;
        };
        let live = self.ctx.append_basic_block(state.value, "env.some");
        let done = self.ctx.append_basic_block(state.value, "env.done");
        let Ok(none) = self.builder.build_is_null(f, "env.none") else { return };
        let _ = self.builder.build_conditional_branch(none, done, live);
        self.builder.position_at_end(live);
        let record =
            repr::byte_offset(self.ctx, &self.builder, block, i64::from(ENV_FIELDS), "env.rec");
        let ty = self.ctx.void_type().fn_type(&[self.ptr_ty().into()], false);
        if let Ok(call) = self.builder.build_indirect_call(ty, f, &[record.into()], "") {
            attrs::set_call_convention(call, attrs::C);
        }
        let _ = self.builder.build_unconditional_branch(done);
        self.builder.position_at_end(done);
    }

    /// A counted loop over a block's elements, walking each in place.
    fn each_element(
        &mut self,
        state: &mut Function<'ctx>,
        base: PointerValue<'ctx>,
        count: IntValue<'ctx>,
        stride: u32,
        elem: &Ty,
        retain: bool,
    ) {
        let word = self.ctx.i64_type();
        let Some(pre) = self.builder.get_insert_block() else { return };
        let header = self.ctx.append_basic_block(state.value, "elem.head");
        let body = self.ctx.append_basic_block(state.value, "elem.body");
        let done = self.ctx.append_basic_block(state.value, "elem.done");
        let _ = self.builder.build_unconditional_branch(header);

        self.builder.position_at_end(header);
        let Ok(phi) = self.builder.build_phi(word, "i") else { return };
        let Ok(index) = TryInto::<IntValue<'ctx>>::try_into(phi.as_basic_value()) else { return };
        let more = self
            .builder
            .build_int_compare(IntPredicate::ULT, index, count, "elem.more")
            .unwrap_or_else(|_| self.ctx.bool_type().const_zero());
        let _ = self.builder.build_conditional_branch(more, body, done);

        self.builder.position_at_end(body);
        let scaled = self
            .builder
            .build_int_mul(index, word.const_int(u64::from(stride), false), "elem.off")
            .unwrap_or(index);
        // SAFETY: inkwell marks `build_in_bounds_gep` unsafe because it cannot
        // check the index; the loop bound is `cap / stride`, so every offset is
        // inside the block this pointer names.
        let at = unsafe {
            self.builder
                .build_in_bounds_gep(self.ctx.i8_type(), base, &[scaled], "elem.at")
                .unwrap_or(base)
        };
        let align = self.reprs.of_ty(elem).layout.align;
        let place = Place::Memory { base: at, align };
        self.walk_rc(state, elem, &place, 0, retain, 0);
        let next = self
            .builder
            .build_int_add(index, word.const_int(1, false), "elem.next")
            .unwrap_or(index);
        // The back edge names the block the walk *ended* in, not `body`: a
        // tagged enum or a niche inside the element leaves the builder in a
        // join block it created, and a phi whose incoming block is not a real
        // predecessor is the one error LLVM's verifier catches here.
        let latch = self.builder.get_insert_block().unwrap_or(body);
        let _ = self.builder.build_unconditional_branch(header);
        phi.add_incoming(&[
            (&word.const_zero() as &dyn BasicValue<'ctx>, pre),
            (&next as &dyn BasicValue<'ctx>, latch),
        ]);

        self.builder.position_at_end(done);
    }
}

// ---------------------------------------------------------------------------
// The out-pointer boundary — `cli/runtime/lib.rs` §2 rules 2, 3 and 4
// ---------------------------------------------------------------------------

impl<'ctx, 'a> Unit<'ctx, 'a> {
    /// One entry-block buffer for a runtime entry that writes through an
    /// out-pointer.
    ///
    /// This is the third `alloca` shape in the backend and it is
    /// CODEGEN-LLVM.md §2.3's case for the same reason the divmod pair is: a C
    /// ABI writes through a pointer, and a pointer needs something to point at.
    /// It is not memory standing in for an SSA value — every load out of it is
    /// in the same basic block as the call that filled it, so SROA turns the
    /// pair back into registers wherever the callee did not escape it.
    ///
    /// In the entry block, before the branch into `b0`: `alloca` in LLVM is not
    /// scoped to a block, so a buffer emitted inside a loop body would grow the
    /// frame once per iteration. One per call site rather than one per
    /// `(size, align)`, because a shared buffer would need a claim about which
    /// two call sites are never live at once and there is nothing here that
    /// proves one.
    fn scratch(&mut self, state: &mut Function<'ctx>, size: u32, align: u32) -> PointerValue<'ctx> {
        let here = self.builder.get_insert_block();
        match state.entry.get_first_instruction() {
            Some(first) => self.builder.position_before(&first),
            None => self.builder.position_at_end(state.entry),
        }
        let byte = self.ctx.i8_type();
        // A zero-byte buffer is legal and useless; one byte keeps the pointer
        // distinct from every other object, which is what the C side assumes.
        let count = self.ctx.i64_type().const_int(u64::from(size.max(1)), false);
        let buf = self
            .builder
            .build_array_alloca(byte, count, "out")
            .unwrap_or_else(|_| self.ptr_ty().const_null());
        if let Some(instr) = buf.as_instruction() {
            let _ = instr.set_alignment(align.max(1));
        }
        if let Some(here) = here {
            self.builder.position_at_end(here);
        }
        buf
    }

    /// The slots of an aggregate, out of the memory form (`repr.rs`'s header):
    /// one `load` per slot at the alignment [`repr::access_align`] allows,
    /// rather than one load of a padded struct type.
    fn load_slots(
        &mut self,
        buf: PointerValue<'ctx>,
        slots: &[Slot],
        align: u32,
    ) -> Vec<BasicValueEnum<'ctx>> {
        let mut out = Vec::with_capacity(slots.len());
        for slot in slots {
            let p = repr::byte_offset(self.ctx, &self.builder, buf, i64::from(slot.offset), "out.p");
            let ty = repr::slot_type(self.ctx, slot.ty);
            match self.builder.build_load(ty, p, "out.v") {
                Ok(v) => {
                    if let Some(instr) = v.as_instruction_value() {
                        let _ = instr.set_alignment(repr::access_align(align, *slot));
                    }
                    out.push(v);
                }
                Err(_) => out.push(ty.const_zero()),
            }
        }
        out
    }

    /// The inverse of [`Unit::load_slots`], for spilling a generic argument.
    fn store_slots(
        &mut self,
        buf: PointerValue<'ctx>,
        slots: &[Slot],
        align: u32,
        pieces: &[BasicValueEnum<'ctx>],
    ) {
        for (slot, piece) in slots.iter().zip(pieces) {
            let p = repr::byte_offset(self.ctx, &self.builder, buf, i64::from(slot.offset), "in.p");
            if let Ok(store) = self.builder.build_store(p, *piece) {
                let _ = store.set_alignment(repr::access_align(align, *slot));
            }
        }
    }

    /// The slots, size and alignment of a destination.
    ///
    /// An [`ir::Type::Agg`] has all three from the layout table. A scalar has
    /// no `Layout` of its own, so the answer is derived from the one slot it
    /// is — which is exact, because a scalar's size is its alignment.
    fn dest_shape(&mut self, code: &ir::Code, dest: ir::ValueId) -> (Vec<Slot>, u32, u32) {
        match code.ty_of(dest) {
            ir::Type::Agg(id) => {
                let r = self.reprs.of(self.program, id);
                (r.slots.clone(), r.layout.size, r.layout.align)
            }
            other => {
                let slots = repr::ir_slots(&mut self.reprs, self.program, other);
                let size = slots
                    .iter()
                    .map(|s| s.offset.saturating_add(s.ty.size()))
                    .max()
                    .unwrap_or(0);
                let align = slots.iter().map(|s| s.ty.size()).max().unwrap_or(1);
                (slots, size, align)
            }
        }
    }

    /// A call whose aggregate result comes back through a trailing
    /// out-pointer (§2 rule 2), bound to `dest`.
    ///
    /// The buffer is at the *dest's own* layout: `BuriStr` and `BuriList` are
    /// `#[repr(C)]` records of exactly the words `middle::layout` gives `Str`
    /// and `[T]`, so there is one description of those bytes rather than two.
    fn call_out(
        &mut self,
        state: &mut Function<'ctx>,
        code: &ir::Code,
        dest: ir::ValueId,
        symbol: &str,
        argv: &mut Vec<BasicMetadataValueEnum<'ctx>>,
    ) {
        let (slots, size, align) = self.dest_shape(code, dest);
        let buf = self.scratch(state, size, align);
        argv.push(buf.into());
        let param_types: Vec<BasicMetadataTypeEnum<'ctx>> =
            argv.iter().map(|a| metadata_type_of(self.ctx, *a)).collect();
        let f = self.declare_rt(symbol, &param_types, None);
        if let Ok(call) = self.builder.build_call(f, argv, "") {
            attrs::set_call_convention(call, attrs::C);
        }
        // Every one of these produces an owned block (`cli/runtime/lib.rs` §3),
        // so the caller is not `memory(none)` and does allocate.
        state.observed.allocates = true;
        state.observed.opaque = true;
        let pieces = self.load_slots(buf, &slots, align);
        let value = repr::assemble(self.ctx, &self.builder, &slots, &pieces);
        self.set(state, dest, value);
    }

    /// A call whose result is a discriminant plus a payload through a trailing
    /// out-pointer (§2 rule 3), bound to `dest` as whatever `middle::layout`
    /// chose for the enum.
    ///
    /// The out-pointer is `buf + offset of .Some's payload`, so the runtime
    /// writes the payload **in place** and the two arms only have to settle the
    /// discriminant. That is what makes the niche free: `EnumRepr::Niche`'s
    /// `.Some` *is* the payload, and its non-null pointer is one the runtime
    /// guarantees (`value.rs`'s `EMPTY`, which exists so that the empty string
    /// does not read back as `.None`).
    fn call_sum(
        &mut self,
        state: &mut Function<'ctx>,
        code: &ir::Code,
        dest: ir::ValueId,
        entry: &runtime::Entry,
        mut argv: Vec<BasicMetadataValueEnum<'ctx>>,
        span: Span,
    ) {
        let key = entry.key;
        let ir::Type::Agg(id) = code.ty_of(dest) else {
            self.error(
                span,
                format!("internal error: `{key}` returns a sum and its destination is not one"),
                "this is a toolchain bug; report it",
            );
            return;
        };
        let (some_at, none_at) = self.option_variants(id);
        let (slots, enum_repr, size, align, payload_at) = {
            let r = self.reprs.of(self.program, id);
            (
                r.slots.clone(),
                r.enum_repr().cloned(),
                r.layout.size,
                r.layout.align,
                r.layout.variant(some_at).first().copied().unwrap_or(0),
            )
        };
        let Some(enum_repr) = enum_repr else {
            self.error(
                span,
                format!("internal error: `{key}`'s destination has no enum representation"),
                "this is a toolchain bug; report it",
            );
            return;
        };
        let buf = self.scratch(state, size, align);
        let out = repr::byte_offset(self.ctx, &self.builder, buf, i64::from(payload_at), "sum.p");
        argv.push(out.into());
        let param_types: Vec<BasicMetadataTypeEnum<'ctx>> =
            argv.iter().map(|a| metadata_type_of(self.ctx, *a)).collect();
        let i32t = self.ctx.i32_type();
        let f = self.declare_rt(entry.symbol, &param_types, Some(i32t.as_basic_type_enum()));
        let Ok(call) = self.builder.build_call(f, &argv, "") else { return };
        attrs::set_call_convention(call, attrs::C);
        state.observed.allocates = true;
        state.observed.opaque = true;
        let disc: IntValue<'ctx> = call
            .try_as_basic_value()
            .basic()
            .and_then(|v| v.try_into().ok())
            .unwrap_or_else(|| i32t.const_zero());
        // `BURI_OK` is `-1`, sign-extended into the `i32` the C side returns.
        let ok = i32t.const_int(runtime::BURI_OK as u64, true);
        let is_ok = self
            .builder
            .build_int_compare(IntPredicate::EQ, disc, ok, "sum.ok")
            .unwrap_or_else(|_| self.ctx.bool_type().const_zero());
        let some_bb = self.ctx.append_basic_block(state.value, "sum.some");
        let none_bb = self.ctx.append_basic_block(state.value, "sum.none");
        let join = self.ctx.append_basic_block(state.value, "sum.done");
        let _ = self.builder.build_conditional_branch(is_ok, some_bb, none_bb);

        self.builder.position_at_end(some_bb);
        let some_value = self.sum_arm(buf, &slots, &enum_repr, align, some_at, true);
        let _ = self.builder.build_unconditional_branch(join);

        self.builder.position_at_end(none_bb);
        let none_value = self.sum_arm(buf, &slots, &enum_repr, align, none_at, false);
        let _ = self.builder.build_unconditional_branch(join);

        self.builder.position_at_end(join);
        let ty = repr::register_type(self.ctx, &slots);
        if let Ok(phi) = self.builder.build_phi(ty, "sum") {
            phi.add_incoming(&[
                (&some_value as &dyn BasicValue<'ctx>, some_bb),
                (&none_value as &dyn BasicValue<'ctx>, none_bb),
            ]);
            self.set(state, dest, phi.as_basic_value());
        }
    }

    /// One arm of a [`Unit::call_sum`]: the enum value for `variant`, reading
    /// the payload out of `buf` where there is one to read.
    ///
    /// `.None` never reads the buffer, because nothing wrote it — a runtime
    /// entry that answers `0` leaves the out-pointer untouched
    /// (`cli/runtime/lib.rs` §2 rule 3), so the register constant is the only
    /// defined answer.
    fn sum_arm(
        &mut self,
        buf: PointerValue<'ctx>,
        slots: &[Slot],
        enum_repr: &EnumRepr,
        align: u32,
        variant: usize,
        loaded: bool,
    ) -> BasicValueEnum<'ctx> {
        let tag_const = |ctx: &'ctx Context, tag: Scalar| match repr::slot_type(
            ctx,
            SlotTy::Scalar(tag),
        ) {
            BasicTypeEnum::IntType(t) => t.const_int(variant as u64, false).as_basic_value_enum(),
            other => other.const_zero(),
        };
        match enum_repr {
            EnumRepr::Bare { tag } => tag_const(self.ctx, *tag),
            EnumRepr::Niche { .. } => {
                if loaded {
                    let pieces = self.load_slots(buf, slots, align);
                    repr::assemble(self.ctx, &self.builder, slots, &pieces)
                } else {
                    // A zeroed register is the right `.None`, exactly as
                    // `make_enum` builds one: the niche pointer is null and
                    // every other slot of a `.None` is unobservable.
                    repr::register_type(self.ctx, slots).const_zero()
                }
            }
            EnumRepr::Tagged { tag, .. } => {
                let tag_value = tag_const(self.ctx, *tag);
                let mut values = vec![tag_value];
                if let Some(slot) = slots.get(1).copied() {
                    let blob = if loaded {
                        self.load_slots(buf, &[slot], align)
                            .first()
                            .copied()
                            .unwrap_or_else(|| repr::slot_type(self.ctx, slot.ty).const_zero())
                    } else {
                        repr::slot_type(self.ctx, slot.ty).const_zero()
                    };
                    values.push(blob);
                }
                repr::assemble(self.ctx, &self.builder, slots, &values)
            }
        }
    }

    /// A call answering a `Result<T, E>` (`cli/runtime/lib.rs` §2.1), bound to
    /// `dest`.
    ///
    /// [`Unit::call_sum`]'s shape with the failure side carrying a number, and
    /// **assembled out of the buffer rather than out of two phis**. That is the
    /// one structural difference and it is worth the sentence: `call_sum`'s
    /// arms each produce a whole enum register, which works because `.None` is
    /// a constant. A `.Err(e)` is not — `e` is a tag *inside* the payload area,
    /// which for a `Tagged` destination is a single opaque blob slot
    /// (`repr::SlotTy::Blob`), and there is no register operation that puts one
    /// integer at a byte offset inside a blob. In memory there is: it is a
    /// store. So both arms write into the buffer the runtime already wrote
    /// `.Ok`'s payload into, and the join loads it once.
    ///
    /// The zeroing of `E`'s payload area on the failure path is §2.1's
    /// restriction made safe rather than merely stated, for the reason
    /// `stencil/rtcall.rs::store_result_tag` gives: the index is a register, so
    /// "the variant it names carries no fields" cannot be checked here, and a
    /// broken entry must produce a value `middle::rc` can walk rather than a
    /// count on whatever the stack held.
    fn call_result(
        &mut self,
        state: &mut Function<'ctx>,
        code: &ir::Code,
        dest: ir::ValueId,
        entry: &runtime::Entry,
        mut argv: Vec<BasicMetadataValueEnum<'ctx>>,
        span: Span,
    ) {
        let key = entry.key;
        let Some((ok, err, ok_bytes, err_ty)) = self.result_shape(code, dest) else {
            self.error(
                span,
                format!("internal error: `{key}` answers a `Result` and its destination is not one"),
                "this is a toolchain bug; report it",
            );
            return;
        };
        let (slots, size, align) = self.dest_shape(code, dest);
        let (enum_repr, ok_at, err_at) = {
            let ir::Type::Agg(id) = code.ty_of(dest) else { return };
            let r = self.reprs.of(self.program, id);
            (
                r.enum_repr().cloned(),
                r.layout.variant(ok).first().copied().unwrap_or(0),
                r.layout.variant(err).first().copied().unwrap_or(0),
            )
        };
        let Some(enum_repr) = enum_repr else {
            self.error(
                span,
                format!("internal error: `{key}`'s destination has no enum representation"),
                "this is a toolchain bug; report it",
            );
            return;
        };
        let err_layout = self.reprs.of_ty(&err_ty).layout.clone();

        let buf = self.scratch(state, size, align);
        if ok_bytes > 0 {
            let out = repr::byte_offset(self.ctx, &self.builder, buf, i64::from(ok_at), "res.ok");
            argv.push(out.into());
        }
        // `E`'s own shape, not a table column (`cli/runtime/lib.rs` §2.1): an
        // enum error is named by an index, and anything else — `Utf8Error(Int)`
        // — crosses through a second out-pointer.
        let err_is_enum = matches!(err_layout.repr, LayoutRepr::Enum { .. });
        if !err_is_enum && err_layout.size > 0 {
            let out = repr::byte_offset(self.ctx, &self.builder, buf, i64::from(err_at), "res.err");
            argv.push(out.into());
        }
        let param_types: Vec<BasicMetadataTypeEnum<'ctx>> =
            argv.iter().map(|a| metadata_type_of(self.ctx, *a)).collect();
        let i32t = self.ctx.i32_type();
        let f = self.declare_rt(entry.symbol, &param_types, Some(i32t.as_basic_type_enum()));
        let Ok(call) = self.builder.build_call(f, &argv, "") else { return };
        attrs::set_call_convention(call, attrs::C);
        state.observed.allocates = true;
        state.observed.opaque = true;
        let disc: IntValue<'ctx> = call
            .try_as_basic_value()
            .basic()
            .and_then(|v| v.try_into().ok())
            .unwrap_or_else(|| i32t.const_zero());
        let is_ok = self
            .builder
            .build_int_compare(
                IntPredicate::EQ,
                disc,
                i32t.const_int(runtime::BURI_OK as u64, true),
                "res.ok?",
            )
            .unwrap_or_else(|_| self.ctx.bool_type().const_zero());
        let good = self.ctx.append_basic_block(state.value, "res.good");
        let bad = self.ctx.append_basic_block(state.value, "res.bad");
        let join = self.ctx.append_basic_block(state.value, "res.done");
        let _ = self.builder.build_conditional_branch(is_ok, good, bad);

        self.builder.position_at_end(good);
        self.store_tag(buf, &enum_repr, align, ok);
        let _ = self.builder.build_unconditional_branch(join);

        self.builder.position_at_end(bad);
        self.store_tag(buf, &enum_repr, align, err);
        if err_is_enum {
            self.store_variant_index(buf, err_at, &err_layout, align, disc);
        }
        let _ = self.builder.build_unconditional_branch(join);

        self.builder.position_at_end(join);
        let pieces = self.load_slots(buf, &slots, align);
        let value = repr::assemble(self.ctx, &self.builder, &slots, &pieces);
        self.set(state, dest, value);
    }

    /// `.Ok`'s and `.Err`'s variant indices, `.Ok`'s payload size, and `E`, for
    /// a `Result<T, E>` destination.
    ///
    /// By **name**, for [`Unit::call_sum`]'s reason: `core/result` declares `Ok`
    /// first and `middle::layout` numbers in declaration order, so the answer is
    /// `(0, 1)` today, and a declaration this did not expect would otherwise be
    /// a wrong answer with no symptom.
    ///
    /// The payload's *size* comes from `T` and not from the variant's field
    /// list, because `Layout::variant` records where a field is and never how
    /// big it is: `Result<(), E>` has a field of no bytes, and whether the C
    /// signature carries an out-pointer turns on exactly that.
    fn result_shape(
        &mut self,
        code: &ir::Code,
        dest: ir::ValueId,
    ) -> Option<(usize, usize, u32, Ty)> {
        let source = self.type_of(code.ty_of(dest))?;
        let (ok, err, err_ty) = types::result_shape(self.tables, &source)?;
        let Ty::Con(_, args) = &source else { return None };
        let ok_bytes = self.reprs.of_ty(args.get(ok)?).layout.size;
        Some((ok, err, ok_bytes, err_ty))
    }

    /// One constant discriminant, written into a buffer whose payload the
    /// caller has already settled.
    ///
    /// The niche writes nothing for a variant that has a payload, exactly as
    /// [`Unit::sum_arm`] builds one: the payload *is* the discriminant there. A
    /// `Result` never gets a niche — `middle/layout.rs`'s `build_enum` gives one
    /// only to `Option<T>` — so in practice this is the tag store, and the niche
    /// arm is here so the function is about enums rather than about `Result`.
    fn store_tag(
        &mut self,
        buf: PointerValue<'ctx>,
        enum_repr: &EnumRepr,
        align: u32,
        variant: usize,
    ) {
        match enum_repr {
            EnumRepr::Bare { tag } | EnumRepr::Tagged { tag, .. } => {
                let slot = Slot { offset: 0, ty: SlotTy::Scalar(*tag) };
                let value = match repr::slot_type(self.ctx, slot.ty) {
                    BasicTypeEnum::IntType(t) => {
                        t.const_int(variant as u64, false).as_basic_value_enum()
                    }
                    other => other.const_zero(),
                };
                self.store_slots(buf, &[slot], align, &[value]);
            }
            EnumRepr::Niche { null_at } => {
                let slot = Slot { offset: *null_at, ty: SlotTy::Scalar(Scalar::Ptr) };
                let zero = self.ctx.ptr_type(inkwell::AddressSpace::default()).const_null();
                self.store_slots(buf, &[slot], align, &[zero.as_basic_value_enum()]);
            }
        }
    }

    /// An enum at `buf + at`, labelled with a variant index that is a
    /// **register**, and its payload area zeroed.
    ///
    /// [`Unit::store_tag`]'s counterpart for the one place the index is not
    /// known until the call comes back. The zeroing is why the payload area is
    /// walked here rather than left alone; `Unit::call_result`'s comment is the
    /// argument for it.
    fn store_variant_index(
        &mut self,
        buf: PointerValue<'ctx>,
        at: u32,
        l: &layout::Layout,
        align: u32,
        index: IntValue<'ctx>,
    ) {
        let LayoutRepr::Enum {
            repr: EnumRepr::Bare { tag } | EnumRepr::Tagged { tag, .. },
            ..
        } = &l.repr
        else {
            return;
        };
        let slot = Slot { offset: at, ty: SlotTy::Scalar(*tag) };
        let want = match repr::slot_type(self.ctx, slot.ty) {
            BasicTypeEnum::IntType(t) => t,
            _ => return,
        };
        let narrowed = self.narrow_int(index.as_basic_value_enum(), want.as_basic_type_enum());
        self.store_slots(buf, &[slot], align, &[narrowed]);
        if let LayoutRepr::Enum { repr: EnumRepr::Tagged { payload, .. }, .. } = &l.repr {
            let mut off = *payload;
            let word = self.ctx.i64_type();
            while off.saturating_add(8) <= l.size {
                let slot = Slot { offset: at.saturating_add(off), ty: SlotTy::Scalar(Scalar::I64) };
                self.store_slots(buf, &[slot], align, &[word.const_zero().as_basic_value_enum()]);
                off = off.saturating_add(8);
            }
            let byte = self.ctx.i8_type();
            while off < l.size {
                let slot = Slot { offset: at.saturating_add(off), ty: SlotTy::Scalar(Scalar::I8) };
                self.store_slots(buf, &[slot], align, &[byte.const_zero().as_basic_value_enum()]);
                off = off.saturating_add(1);
            }
        }
    }

    /// Whether an argument is a context, and therefore not the runtime's
    /// business (VALUE-MODEL.md §8).
    ///
    /// `Ty::Ctx` and not "spreads to no leaves": `core/testing/context`'s
    /// implementations carry an `I64` handle each, because Buri has no mutation
    /// and a captured stdout's state lives on the runner's side.
    fn is_context(&self, ty: ir::Type) -> bool {
        matches!(self.type_of(ty), Some(Ty::Ctx(_)))
    }

    /// The source type behind an [`ir::Type`], where there is one.
    ///
    /// Only an aggregate has one: `middle::lower` interns a `Ty` exactly when
    /// the type has no register shape (`lower.rs`'s `Types::of`), so a `Char`
    /// and a `U32` are both `ir::Type::I32` and nothing here can tell them
    /// apart. That is the whole of why [`Unit::derived`] refuses some of its
    /// arms, and the sentence is here so the refusal reads as a consequence.
    fn type_of(&self, ty: ir::Type) -> Option<Ty> {
        match ty {
            ir::Type::Agg(id) => Some(self.program.type_info(id).ty.clone()),
            _ => None,
        }
    }

    fn int_of_width(&self, bits: u32) -> IntType<'ctx> {
        match bits {
            1 => self.ctx.bool_type(),
            8 => self.ctx.i8_type(),
            16 => self.ctx.i16_type(),
            32 => self.ctx.i32_type(),
            128 => self.ctx.i128_type(),
            _ => self.ctx.i64_type(),
        }
    }

    /// An integer the C side returned, at the width the IR wants it.
    ///
    /// Always a truncation or a zero extension and never a sign extension: the
    /// two values that cross this way are a `u8` that is `0` or `1` and an
    /// `Order` tag that is `0`, `1` or `2`, both of which are non-negative by
    /// construction.
    fn narrow_int(
        &self,
        value: BasicValueEnum<'ctx>,
        want: BasicTypeEnum<'ctx>,
    ) -> BasicValueEnum<'ctx> {
        let (BasicValueEnum::IntValue(v), BasicTypeEnum::IntType(t)) = (value, want) else {
            return value;
        };
        if v.get_type().get_bit_width() == t.get_bit_width() {
            return value;
        }
        if t.get_bit_width() < v.get_type().get_bit_width() {
            self.builder
                .build_int_truncate(v, t, "narrow")
                .map(Into::into)
                .unwrap_or(value)
        } else {
            self.builder
                .build_int_z_extend(v, t, "widen")
                .map(Into::into)
                .unwrap_or(value)
        }
    }
}

// ---------------------------------------------------------------------------
// The generated half of the intrinsic surface
// ---------------------------------------------------------------------------

impl<'ctx, 'a> Unit<'ctx, 'a> {
    /// `==`, `<` and the rest at a `Str`, which has no comparison instruction.
    ///
    /// `buri_rt_str_eq` is a byte compare with a length test in front and
    /// `buri_rt_str_compare` is a lexicographic one answering `Order`'s own
    /// numbering — `Less = 0`, `Equal = 1`, `Greater = 2`, in declaration order
    /// in `core/order`. Every relational operator is therefore that number
    /// against `Equal`: `<` is `< 1`, `<=` is `<= 1`, `>` is `> 1`. One call
    /// and one comparison, rather than five entry points.
    fn string_binary(
        &mut self,
        state: &mut Function<'ctx>,
        op: ir::BinOp,
        operand: ir::Type,
        lhs: BasicValueEnum<'ctx>,
        rhs: BasicValueEnum<'ctx>,
    ) -> BasicValueEnum<'ctx> {
        let slots = repr::ir_slots(&mut self.reprs, self.program, operand);
        let mut argv: Vec<BasicMetadataValueEnum<'ctx>> = Vec::new();
        for value in [lhs, rhs] {
            for piece in repr::disassemble(&self.builder, &slots, value) {
                argv.push(piece.into());
            }
        }
        let equality = matches!(op, ir::BinOp::Eq | ir::BinOp::Ne);
        let (symbol, width) = if equality {
            (runtime::entry("str.eq").map_or("buri_rt_str_eq", |e| e.symbol), 8)
        } else {
            (runtime::entry("str.compare").map_or("buri_rt_str_compare", |e| e.symbol), 32)
        };
        let ret = self.int_of_width(width);
        let param_types: Vec<BasicMetadataTypeEnum<'ctx>> =
            argv.iter().map(|a| metadata_type_of(self.ctx, *a)).collect();
        let f = self.declare_rt(symbol, &param_types, Some(ret.as_basic_type_enum()));
        let Ok(call) = self.builder.build_call(f, &argv, "") else { return lhs };
        attrs::set_call_convention(call, attrs::C);
        state.observed.opaque = true;
        let answer: IntValue<'ctx> = call
            .try_as_basic_value()
            .basic()
            .and_then(|v| v.try_into().ok())
            .unwrap_or_else(|| ret.const_zero());
        let (predicate, against) = if equality {
            let p = if matches!(op, ir::BinOp::Eq) { IntPredicate::NE } else { IntPredicate::EQ };
            (p, ret.const_zero())
        } else {
            // `Order::Equal` is `1`, so the signed comparison of the tag
            // against it is the operator itself.
            (int_predicate(op, true), ret.const_int(1, false))
        };
        self.builder
            .build_int_compare(predicate, answer, against, "str.cmp")
            .map(Into::into)
            .unwrap_or(lhs)
    }

    /// A structural operation the derive pass left behind.
    ///
    /// `middle::derives` replaces every one it can with a call to a function it
    /// generated. What reaches here is a **template hole**, which
    /// `middle::lower` emits directly and after that pass has run. So the arms
    /// below are exactly the primitive arms of `derivePrimShow`
    /// (`middle/derives.rs`'s header) *without* its quoting, and anything else
    /// is a gap named rather than miscompiled.
    ///
    /// Unlike [`Unit::derived`] this has the source type in hand — an
    /// `Inst::Structural` carries an interned `TypeId` — so every primitive is
    /// reachable here, including the ones a bare register shape cannot name.
    fn structural(
        &mut self,
        state: &mut Function<'ctx>,
        code: &ir::Code,
        inst: &ir::Inst,
        span: Span,
    ) {
        let ir::Inst::Structural { dest, op, ty, args } = inst else { return };
        let (dest, op, ty) = (*dest, *op, *ty);
        let (source, name) = {
            let info = self.program.type_info(ty);
            (info.ty.clone(), info.name.clone())
        };
        if !matches!(op, ir::StructuralOp::Show) {
            self.error(
                span,
                format!(
                    "internal error: a structural `{op:?}` on `{name}` reached the LLVM \
                     backend, so `middle::native` did not replace it"
                ),
                "this is a toolchain bug; report it",
            );
            return;
        }
        let Some(arg) = args.first().copied() else { return };
        let prim = self.tables.as_prim(&source);
        if prim.is_some_and(|p| self.show_prim(state, code, dest, p, arg, false)) {
            return;
        }
        self.error(
            span,
            format!("the LLVM backend cannot render a `{name}` in a template yet"),
            "interpolate a primitive, or build with `--output=js`",
        );
    }

    /// One primitive as a `Str`, for a template hole (`quoted == false`) or for
    /// `derivePrimShow` (`quoted == true`).
    ///
    /// The two differ in exactly two arms, which is the whole reason they share
    /// a body: `$str` of a `Str` is the string and `$show` of one is
    /// `JSON.stringify`, and `$str` of a `Char` is the character and `$show` of
    /// one is `'c'` (`middle/derives.rs`'s table). Every numeric arm is
    /// identical, and duplicating them would be two places for the float
    /// formatter to be chosen from.
    fn show_prim(
        &mut self,
        state: &mut Function<'ctx>,
        code: &ir::Code,
        dest: ir::ValueId,
        prim: Prim,
        arg: ir::ValueId,
        quoted: bool,
    ) -> bool {
        let value = self.get(state, arg);
        match prim {
            Prim::Str | Prim::Template => {
                if !quoted {
                    // The value itself. Not a copy: a `Str` is three words in a
                    // register and `{ base, ptr, len }` aliased is the same
                    // three words, which is what a `memcpy` of a stack slot
                    // amounts to in a backend that keeps one.
                    self.set(state, dest, value);
                    return true;
                }
                let slots = repr::ir_slots(&mut self.reprs, self.program, code.ty_of(arg));
                let pieces = repr::disassemble(&self.builder, &slots, value);
                // `ptr`, `len` and no `base`: the renderer reads the bytes and
                // keeps nothing, which is `Arg::Bytes`'s shape.
                let (Some(ptr), Some(BasicValueEnum::IntValue(raw))) =
                    (pieces.get(layout::STR_PTR).copied(), pieces.get(layout::STR_LEN).copied())
                else {
                    return false;
                };
                // **Masked**, unlike every `Arg::Str` argument. `text.rs`'s
                // entries take the stored word and clear bit 63 themselves;
                // `buri_rt_show_str` documents `len` readable bytes and reads
                // exactly that many, so handing it the ASCII flag would ask for
                // a slice of `2^63 + n`. The two conventions are the runtime's,
                // and the call site is where they meet.
                let word = self.ctx.i64_type();
                let len = self
                    .builder
                    .build_and(raw, word.const_int(STR_LEN_MASK, false), "show.bytes")
                    .unwrap_or(raw);
                let mut argv: Vec<BasicMetadataValueEnum<'ctx>> = vec![ptr.into(), len.into()];
                self.call_out(state, code, dest, runtime::SHOW_STR, &mut argv);
                true
            }
            // Two literals and a `select`. The debug backend generates a
            // helper; here the two `Str`s are three constants each and LLVM
            // folds the selects into one, so a call would be the more expensive
            // of the two spellings.
            Prim::Bool => {
                let BasicValueEnum::IntValue(cond) = value else { return false };
                let yes = self.str_literal("true");
                let no = self.str_literal("false");
                let chosen =
                    self.builder.build_select(cond, yes, no, "show.bool").unwrap_or(no);
                self.set(state, dest, chosen);
                true
            }
            Prim::Char => {
                let symbol = if quoted { runtime::SHOW_CHAR } else { runtime::CHAR_TO_STR };
                let mut argv = vec![value.into()];
                self.call_out(state, code, dest, symbol, &mut argv);
                true
            }
            Prim::F32 | Prim::F64 => {
                let symbol =
                    if matches!(prim, Prim::F32) { runtime::SHOW_F32 } else { runtime::SHOW_F64 };
                let mut argv = vec![value.into()];
                self.call_out(state, code, dest, symbol, &mut argv);
                true
            }
            // A pair of `u64`s, low half first — `buri_rt_i128_divmod`'s shape,
            // for its reason: a 128-bit value is not a scalar leaf, and passing
            // it as one would mean agreeing with the platform ABI about how it
            // is classified.
            Prim::I128 | Prim::U128 => {
                let BasicValueEnum::IntValue(v) = value else { return false };
                let (lo, hi) = self.halves(v);
                let symbol =
                    if matches!(prim, Prim::I128) { runtime::SHOW_I128 } else { runtime::SHOW_U128 };
                let mut argv = vec![lo.into(), hi.into()];
                self.call_out(state, code, dest, symbol, &mut argv);
                true
            }
            // A `U64` is the one integer whose value does not fit the `i64`
            // [`runtime::SHOW_INT`] takes, and widening cannot help: every bit
            // is already in use. It is rendered as the 128-bit unsigned value
            // whose high half is zero, which `buri_rt_show_u128` already
            // renders — so `U64.maxValue` prints `18446744073709551615` rather
            // than `-1`. The debug backend answers the same digits by a
            // different route: `stencil/rtcall.rs::show_prim` picks the
            // renderer per signedness, so it has an unsigned division to hand
            // and this backend does not.
            Prim::U64 => {
                let BasicValueEnum::IntValue(v) = value else { return false };
                let word = self.ctx.i64_type();
                let mut argv = vec![v.into(), word.const_zero().into()];
                self.call_out(state, code, dest, runtime::SHOW_U128, &mut argv);
                true
            }
            p if p.is_integer() => {
                let BasicValueEnum::IntValue(v) = value else { return false };
                // Widened by the **source's** signedness: `U8` to `I64` is a
                // zero extension and `I8` to `I64` is a sign extension, and
                // getting that backwards presents as `255` printing as `-1`.
                let word = self.ctx.i64_type();
                let wide = if p.is_signed() {
                    self.builder.build_int_s_extend_or_bit_cast(v, word, "show.w")
                } else {
                    self.builder.build_int_z_extend_or_bit_cast(v, word, "show.w")
                }
                .unwrap_or_else(|_| word.const_zero());
                let mut argv = vec![wide.into()];
                self.call_out(state, code, dest, runtime::SHOW_INT, &mut argv);
                true
            }
            _ => false,
        }
    }

    /// A 128-bit value as `(lo, hi)`, low half first.
    fn halves(&self, v: IntValue<'ctx>) -> (IntValue<'ctx>, IntValue<'ctx>) {
        let word = self.ctx.i64_type();
        let lo = self
            .builder
            .build_int_truncate_or_bit_cast(v, word, "lo")
            .unwrap_or_else(|_| word.const_zero());
        let shifted = self
            .builder
            .build_right_shift(v, v.get_type().const_int(64, false), false, "hi.sh")
            .unwrap_or(v);
        let hi = self
            .builder
            .build_int_truncate_or_bit_cast(shifted, word, "hi")
            .unwrap_or_else(|_| word.const_zero());
        (lo, hi)
    }

    /// `derivePrimShow.<T>` and `derivePrimHash.<T>`, the two type-directed
    /// leaves `middle::derives` bottoms out at.
    ///
    /// The primitive is in the **key**, not in the IR type of the argument.
    /// `middle::lower`'s `qualified_key` appends it for exactly the reason a
    /// backend needs it: the lowered IR records `I64` for `I64` and `U64`
    /// alike, and `show` of `255` at `U8` is `255` while the same byte at `I8`
    /// is `-1`. So every arm is reachable here, and the unqualified spelling —
    /// which `lower` produces only for a `derivePrim*` at something that is not
    /// a primitive, a bug in `derives.rs` — is deliberately *not* claimed, so
    /// that `missing_intrinsics` names it.
    fn derived(
        &mut self,
        state: &mut Function<'ctx>,
        code: &ir::Code,
        dests: &[ir::ValueId],
        key: &str,
        args: &[ir::ValueId],
        span: Span,
    ) -> bool {
        let Some((name, prim)) = derive_key(key) else { return false };
        let Some(dest) = dests.first().copied() else { return true };
        let done = match name {
            // Quoted: `$show`'s primitive arm, where a `Str` is
            // `JSON.stringify` and a `Char` is `'c'` (`runtime.js:200-210`).
            "derivePrimShow" => args
                .first()
                .copied()
                .is_some_and(|arg| self.show_prim(state, code, dest, prim, arg, true)),
            _ => {
                let (Some(h), Some(x)) = (args.first().copied(), args.get(1).copied()) else {
                    return true;
                };
                let acc = self.get(state, h);
                self.hash_prim(state, code, dest, prim, acc, x)
            }
        };
        if !done {
            self.error(
                span,
                format!("the LLVM backend cannot compile `{key}` yet"),
                "report it: this is a toolchain bug, not a problem with your program",
            );
        }
        true
    }

    /// `derivePrimHash.<T>(h, x)`.
    ///
    /// `cli/runtime/hash.rs`'s table, which is `$hashInto`'s three shapes: an
    /// integer or a `Bool` mixes `ToUint32(x)`, a float goes through
    /// `ToUint32(Math.trunc(x))`, and a `Char` or a `Str` mixes one **UTF-16
    /// code unit** at a time. The last is the one that cannot be guessed — an
    /// astral scalar is two mixes of its surrogate halves — and is why hashing
    /// is a shared runtime body rather than open-coded here.
    ///
    /// The extension to 32 bits takes the **source's** signedness, because
    /// `ToUint32` of a negative number is its two's-complement pattern: an
    /// `I8` holding `-1` mixes `0xffffffff` and a `U8` holding `255` mixes
    /// `0xff`, and the two are the same byte.
    fn hash_prim(
        &mut self,
        state: &mut Function<'ctx>,
        code: &ir::Code,
        dest: ir::ValueId,
        prim: Prim,
        acc: BasicValueEnum<'ctx>,
        arg: ir::ValueId,
    ) -> bool {
        let value = self.get(state, arg);
        let word = self.ctx.i64_type();
        let i32t = self.ctx.i32_type();
        let f64t = self.ctx.f64_type();
        let (symbol, tail): (&str, Vec<BasicMetadataValueEnum<'ctx>>) = match prim {
            Prim::Str | Prim::Template => {
                let slots = repr::ir_slots(&mut self.reprs, self.program, code.ty_of(arg));
                // `base`, `ptr`, `len` — the whole `Str`, because `hash.rs`
                // takes the block even though it keeps nothing.
                (
                    runtime::HASH_STR,
                    repr::disassemble(&self.builder, &slots, value)
                        .into_iter()
                        .map(Into::into)
                        .collect(),
                )
            }
            Prim::Char => (runtime::HASH_CHAR, vec![value.into()]),
            Prim::F32 => {
                let BasicValueEnum::FloatValue(v) = value else { return false };
                // Promoted, not reinterpreted: on JavaScript there is one
                // number type, so `1.5f32` and `1.5f64` must hash alike.
                let wide = self
                    .builder
                    .build_float_ext(v, f64t, "hash.w")
                    .unwrap_or_else(|_| f64t.const_zero());
                (runtime::HASH_F64, vec![wide.into()])
            }
            Prim::F64 => (runtime::HASH_F64, vec![value.into()]),
            p if p.is_integer() || matches!(p, Prim::Bool) => {
                let BasicValueEnum::IntValue(v) = value else { return false };
                let bits = v.get_type().get_bit_width();
                let narrowed = if bits == 32 {
                    v
                } else if bits > 32 {
                    self.builder.build_int_truncate(v, i32t, "hash.n").unwrap_or(v)
                } else if p.is_signed() {
                    self.builder.build_int_s_extend(v, i32t, "hash.s").unwrap_or(v)
                } else {
                    self.builder.build_int_z_extend(v, i32t, "hash.z").unwrap_or(v)
                };
                (runtime::MIX, vec![narrowed.into()])
            }
            _ => return false,
        };
        let mut argv: Vec<BasicMetadataValueEnum<'ctx>> = vec![acc.into()];
        argv.extend(tail);
        let param_types: Vec<BasicMetadataTypeEnum<'ctx>> =
            argv.iter().map(|a| metadata_type_of(self.ctx, *a)).collect();
        let f = self.declare_rt(symbol, &param_types, Some(word.as_basic_type_enum()));
        let Ok(call) = self.builder.build_call(f, &argv, "") else { return true };
        attrs::set_call_convention(call, attrs::C);
        let out = call
            .try_as_basic_value()
            .basic()
            .unwrap_or_else(|| word.const_zero().as_basic_value_enum());
        self.set(state, dest, out);
        true
    }

    /// The handful of `str.*` and `list.*` entries that are a load or a copy.
    ///
    /// Each is here because calling a runtime function for it would be a call
    /// to fetch a word this backend already has the address of, or an
    /// allocation and two `memcpy`s wrapped in a `ccc` call. Answers `false`
    /// for a key it does not claim, so the caller falls through to the table.
    fn open_coded(
        &mut self,
        state: &mut Function<'ctx>,
        code: &ir::Code,
        dests: &[ir::ValueId],
        key: &str,
        args: &[ir::ValueId],
    ) -> bool {
        let Some(dest) = dests.first().copied() else { return false };
        // `str.show`, `char.eq`, `bool.compare` and their siblings: the same
        // three operations `Unit::numeric` emits, at the three primitives whose
        // defining module is not `core/num` and whose keys are therefore two
        // segments rather than three.
        if let Some((prim, op)) = prim_leaf(key) {
            let Some(x) = args.first().copied() else { return false };
            let y = args.get(1).copied();
            return match op {
                "show" => self.show_prim(state, code, dest, prim, x, false),
                "hash" => {
                    let seed = self.ctx.i64_type().const_int(runtime::HASH_SEED, false);
                    self.hash_prim(state, code, dest, prim, seed.into(), x)
                }
                "eq" => {
                    let Some(y) = y else { return false };
                    let (l, r) = (self.get(state, x), self.get(state, y));
                    let out = self.binary(state, ir::BinOp::Eq, prim, code.ty_of(x), l, r);
                    self.set(state, dest, out);
                    true
                }
                _ => {
                    let Some(y) = y else { return false };
                    let (l, r) = (self.get(state, x), self.get(state, y));
                    let want =
                        repr::ir_type(self.ctx, &mut self.reprs, self.program, code.ty_of(dest));
                    let BasicTypeEnum::IntType(tag) = want else { return false };
                    let out = self.order(l, r, false, false, tag);
                    self.set(state, dest, out);
                    true
                }
            };
        }
        match key {
            // `char.toU32()` is the identity: both sides are an `i32`.
            "char.toU32" => {
                let Some(a) = args.first().copied() else { return false };
                let value = self.get(state, a);
                self.set(state, dest, value);
                true
            }
            // `list.len()` is the element count, exactly, and always O(1)
            // (VALUE-MODEL.md §4).
            "list.len" => {
                let Some(a) = args.first().copied() else { return false };
                let slots = repr::ir_slots(&mut self.reprs, self.program, code.ty_of(a));
                let value = self.get(state, a);
                let pieces = repr::disassemble(&self.builder, &slots, value);
                let Some(len) = pieces.get(layout::LIST_LEN).copied() else { return false };
                self.set(state, dest, len);
                true
            }
            // `str.len()` is the number of Unicode *scalars* (§3.1). Bit 63 of
            // the stored length answers what that costs: set means every byte
            // is below 0x80, so the count is the byte count and this is a
            // mask; clear means the runtime counts continuation bytes.
            "str.len" => self.str_len(state, code, dest, args),
            // `str.format(ctx, template)` is the identity: a `Template` *is* a
            // `Str` (§3.3), and `middle::lower` has already turned the holes
            // into a `str.concat` chain. The context is zero-sized and has
            // already been dropped from the argument list.
            //
            // The identity on the *bytes*, not on the count. `middle/rc.rs`'s
            // stated convention is that an intrinsic borrows its arguments and
            // returns a fresh count, and copying three words produces a second
            // name for one block rather than a second reference to it — so the
            // count is taken here. Without it the argument and the result are
            // one block with one count and two owners, and the second drop is
            // a double free.
            "str.format" => {
                let Some(src) = args.last().copied() else { return false };
                let value = self.get(state, src);
                self.set(state, dest, value);
                self.incref(state, code, src);
                true
            }
            "str.concat" => self.concat(state, code, dest, args),
            "list.get" => self.list_get(state, code, dest, args),
            "list.zip" => self.list_zip(state, code, dests, key, args),
            "list.flatten" => self.list_flatten(state, code, dests, key, args),
            _ if key.starts_with("bits.") => {
                let op = key.split_once('.').map_or("", |(_, o)| o);
                self.bits(state, dest, op, args)
            }
            // The two parts of `core/testing/context` that have a native
            // counterpart at all. Every other part is stateful — a captured
            // stdout, an in-memory filesystem — and its state lives on the test
            // runner's side, which a compiled artifact does not have; those stay
            // named gaps.
            //
            // `alloc()` is a fresh `TestAlloc(handle)`, and the handle names an
            // arena the runner reclaims. Natively there is no runner and one
            // allocator, so the handle names nothing and zero is as good a name
            // as any.
            "testing_context.alloc" | "host_testing.alloc" => {
                let slots = repr::ir_slots(&mut self.reprs, self.program, code.ty_of(dest));
                let zero = self.ctx.i64_type().const_zero();
                let values = [zero.into()];
                let value = repr::assemble(self.ctx, &self.builder, &slots, &values);
                self.set(state, dest, value);
                true
            }
            // The identity on the byte count, which is exactly what
            // `buri_rt_host_alloc_allocate` is: MEMORY.md §7 makes the charge a
            // function of the *types*, computed by `middle::layout`, so
            // `allocate` returns what it was asked for and the accounting is the
            // caller's.
            "testing_context.TestAlloc.allocate" | "host_testing.TestAlloc.allocate" => {
                let Some(bytes) = args.get(1).copied() else { return false };
                let value = self.get(state, bytes);
                self.set(state, dest, value);
                true
            }
            // `core/testing/assert`'s three bodies. On JavaScript they are the
            // *runner*'s — `$testing_assert_report` throws and `buri test`
            // catches it (`runtime.js:1728`) — and a compiled test binary has no
            // runner, so a failure ends the process. That difference is stated
            // in `cli/runtime/abort.rs`: one process runs every `test` block in
            // order and the first failure is the last thing it does.
            //
            // The two `Shown` keys are `middle::derives`'s substitution for the
            // descriptor a native artifact never receives: both values arrive
            // already rendered by the `Show` generated at their type, so one
            // `commands/test.rs::report_failure` states the failure format for
            // every backend. `stencil/rtcall.rs` lowers the same two keys to
            // the same two entries.
            "testing_assert.reportShown" => {
                let (Some(kind), Some(actual), Some(expected)) =
                    (args.first().copied(), args.get(1).copied(), args.get(2).copied())
                else {
                    return false;
                };
                // No `unreachable` and no fresh block: this sits in the else of
                // an `if` the derive pass generated, and the block it is in is
                // the one the merge's phi names as a predecessor. Ending it
                // here would leave the branch to the merge in a block the phi
                // has no entry for, which is IR the verifier rejects. The call
                // is marked `noreturn`, which is the whole of what LLVM needs
                // to know.
                self.abort_strs_continuing(
                    state,
                    code,
                    &[kind, actual, expected],
                    runtime::TEST_FAIL_COMPARED,
                );
                // `report` answers `()`, and the branch this is the else of
                // merges into a block whose parameter is that `()`. An
                // unbound destination is an edge the phi has no entry for,
                // which the verifier reads before it learns the edge is dead.
                let want =
                    repr::ir_type(self.ctx, &mut self.reprs, self.program, code.ty_of(dest));
                self.set(state, dest, want.const_zero());
                true
            }
            "testing_assert.failExpectedShown" => {
                let (Some(kind), Some(shown)) = (args.first().copied(), args.get(1).copied())
                else {
                    return false;
                };
                self.abort_strs(state, code, &[kind, shown], runtime::TEST_FAIL_EXPECTED);
                self.after_abort(state);
                let want =
                    repr::ir_type(self.ctx, &mut self.reprs, self.program, code.ty_of(dest));
                self.set(state, dest, want.const_zero());
                true
            }
            // Reached where the derive pass declined to generate a `Show` at
            // the type — an opaque one — so there is nothing to render with and
            // the kind is what makes the failure attributable.
            "testing_assert.report" => {
                let (Some(passed), Some(kind)) = (args.first().copied(), args.get(1).copied())
                else {
                    return false;
                };
                let BasicValueEnum::IntValue(cond) = self.get(state, passed) else {
                    return false;
                };
                let ok = self.ctx.append_basic_block(state.value, "assert.ok");
                let bad = self.ctx.append_basic_block(state.value, "assert.bad");
                let _ = self.builder.build_conditional_branch(cond, ok, bad);
                self.builder.position_at_end(bad);
                self.abort_assert(state, code, kind);
                self.builder.position_at_end(ok);
                true
            }
            "testing_assert.failWith" => {
                let Some(message) = args.first().copied() else { return false };
                self.abort_str(state, code, message, runtime::ABORT);
                self.after_abort(state);
                true
            }
            // `failExpected<T, R>(kind, got): R` answers the bottom type. The
            // call above it does not return, so the destination is bound in a
            // block nothing reaches — but it is bound, because the IR that
            // follows names it and LLVM's verifier reads that IR before it
            // learns the block is dead.
            "testing_assert.failExpected" => {
                let Some(kind) = args.first().copied() else { return false };
                self.abort_assert(state, code, kind);
                self.after_abort(state);
                let want =
                    repr::ir_type(self.ctx, &mut self.reprs, self.program, code.ty_of(dest));
                self.set(state, dest, want.const_zero());
                true
            }
            // `list.empty()` is `Inst::MakeArray` with no elements, spelled
            // out: a block of no bytes, so that `ptr` is a real payload start
            // and the count behind it is the one `incref` and `decref` expect.
            // **Not** the null descriptor `cli/runtime/list.rs`'s `block`
            // answers — `repr.rs` marks a list's `ptr` `Counted::NonNull`, and
            // a null one would make the first `decref` read a header at `-16`.
            "list.empty" => {
                let alloc = self.rt_alloc();
                state.observed.allocates = true;
                let size = self.ctx.i64_type().const_zero();
                let block = match self.builder.build_call(alloc, &[size.into()], "empty") {
                    Ok(call) => {
                        attrs::set_call_convention(call, attrs::C);
                        call.try_as_basic_value()
                            .basic()
                            .and_then(|v| v.try_into().ok())
                            .unwrap_or_else(|| self.ptr_ty().const_null())
                    }
                    Err(_) => self.ptr_ty().const_null(),
                };
                let slots = repr::ir_slots(&mut self.reprs, self.program, code.ty_of(dest));
                let values = [block.into(), self.ctx.i64_type().const_zero().into()];
                let value = repr::assemble(self.ctx, &self.builder, &slots, &values);
                self.set(state, dest, value);
                true
            }
            _ => false,
        }
    }

    /// `str.len`, with the ASCII flag's fast path.
    fn str_len(
        &mut self,
        state: &mut Function<'ctx>,
        code: &ir::Code,
        dest: ir::ValueId,
        args: &[ir::ValueId],
    ) -> bool {
        let Some(a) = args.first().copied() else { return false };
        let slots = repr::ir_slots(&mut self.reprs, self.program, code.ty_of(a));
        let value = self.get(state, a);
        let pieces = repr::disassemble(&self.builder, &slots, value);
        let (
            Some(BasicValueEnum::PointerValue(ptr)),
            Some(BasicValueEnum::IntValue(raw)),
        ) = (pieces.get(layout::STR_PTR).copied(), pieces.get(layout::STR_LEN).copied())
        else {
            return false;
        };
        let word = self.ctx.i64_type();
        let bytes = self
            .builder
            .build_and(raw, word.const_int(STR_LEN_MASK, false), "str.bytes")
            .unwrap_or(raw);
        let flag = self
            .builder
            .build_and(raw, word.const_int(STR_ASCII_FLAG, false), "str.ascii")
            .unwrap_or(raw);
        let is_ascii = self
            .builder
            .build_int_compare(IntPredicate::NE, flag, word.const_zero(), "str.isascii")
            .unwrap_or_else(|_| self.ctx.bool_type().const_zero());
        let slow = self.ctx.append_basic_block(state.value, "len.scan");
        let join = self.ctx.append_basic_block(state.value, "len.done");
        let Some(fast) = self.builder.get_insert_block() else { return false };
        let _ = self.builder.build_conditional_branch(is_ascii, join, slow);

        self.builder.position_at_end(slow);
        let f = self.declare_rt(
            runtime::STR_SCALAR_LEN,
            &[self.ptr_ty().into(), word.into()],
            Some(word.as_basic_type_enum()),
        );
        let scanned = match self.builder.build_call(f, &[ptr.into(), bytes.into()], "scan") {
            Ok(call) => {
                attrs::set_call_convention(call, attrs::C);
                call.try_as_basic_value()
                    .basic()
                    .and_then(|v| v.try_into().ok())
                    .unwrap_or(bytes)
            }
            Err(_) => bytes,
        };
        let Some(slow_end) = self.builder.get_insert_block() else { return false };
        let _ = self.builder.build_unconditional_branch(join);

        self.builder.position_at_end(join);
        match self.builder.build_phi(word, "str.len") {
            Ok(phi) => {
                phi.add_incoming(&[
                    (&bytes as &dyn BasicValue<'ctx>, fast),
                    (&scanned as &dyn BasicValue<'ctx>, slow_end),
                ]);
                self.set(state, dest, phi.as_basic_value());
            }
            Err(_) => self.set(state, dest, bytes.into()),
        }
        true
    }

    /// `core/bits`, open-coded: one instruction behind a range check.
    ///
    /// Every one of the fourteen is a machine operation, and the interesting
    /// part is the operand widths, which are not the same as the *declared*
    /// ones. `bits.shr(x: Int, n)` is a **logical** right shift — `runtime.js`
    /// reinterprets the pattern as unsigned, shifts, and narrows back — while
    /// `bits.sar` is the arithmetic one; that they differ is the whole reason
    /// `core/bits` names both. The `U8`, `U32` and `U64` families operate at
    /// their own width, and the shift count is an `Int` at every one of them, so
    /// it is truncated after the range check rather than before.
    ///
    /// The range check aborts (`runtime.js:925`) rather than masking. Masking is
    /// what the hardware does and it is *not* what the language says, and a
    /// shift by 64 that silently answered `x` would be the kind of divergence
    /// VALUE-MODEL.md §12 exists to rule out.
    fn bits(
        &mut self,
        state: &mut Function<'ctx>,
        dest: ir::ValueId,
        op: &str,
        args: &[ir::ValueId],
    ) -> bool {
        let Some(BasicValueEnum::IntValue(x)) = args.first().copied().map(|v| self.get(state, v))
        else {
            return false;
        };
        // `llvm.ctpop`, `llvm.ctlz` and `llvm.cttz` answer at the operand's own
        // width, which here is `Int`'s, so nothing is converted. `false` for
        // the zero-is-poison flag: `$bits_leadingZeros(0)` is `64` and
        // `$bits_trailingZeros(0)` is `64`, so zero has an answer.
        let zero_defined = self.ctx.bool_type().const_zero().into();
        let counted = match op {
            "popCount" => Some(("llvm.ctpop", vec![x.into()])),
            "leadingZeros" => Some(("llvm.ctlz", vec![x.into(), zero_defined])),
            "trailingZeros" => Some(("llvm.cttz", vec![x.into(), zero_defined])),
            _ => None,
        };
        if let Some((name, argv)) = counted {
            let ty = x.get_type().as_basic_type_enum();
            let Some(v) = self.llvm_intrinsic(name, &[ty], &argv) else { return false };
            self.set(state, dest, v);
            return true;
        }
        let Some(BasicValueEnum::IntValue(n)) = args.get(1).copied().map(|v| self.get(state, v))
        else {
            return false;
        };
        let bits = match op {
            "shlU8" | "shrU8" => 8,
            "shlU32" | "shrU32" => 32,
            _ => 64,
        };
        self.shift_guard(state, n, bits);
        let want = x.get_type();
        let count = self
            .builder
            .build_int_truncate_or_bit_cast(n, want, "sh.n")
            .unwrap_or(n);
        let value = match op {
            "shl" | "shlU8" | "shlU32" | "shlU64" => {
                self.builder.build_left_shift(x, count, "sh").map(Into::into)
            }
            // Logical, at every width: `shr` reinterprets as unsigned and the
            // `U*` families are unsigned already.
            "shr" | "shrU8" | "shrU32" | "shrU64" => {
                self.builder.build_right_shift(x, count, false, "sh").map(Into::into)
            }
            "sar" => self.builder.build_right_shift(x, count, true, "sar").map(Into::into),
            // `llvm.fshl(x, x, n)` *is* a rotate, and it is defined for every
            // count — unlike `(x << n) | (x >> (w - n))`, whose second shift is
            // poison at `n == 0`. The range check has already ruled out `n >= w`.
            "rotateLeft" | "rotateRight" => {
                let name =
                    if op == "rotateLeft" { "llvm.fshl" } else { "llvm.fshr" };
                let ty = want.as_basic_type_enum();
                let Some(v) =
                    self.llvm_intrinsic(name, &[ty], &[x.into(), x.into(), count.into()])
                else {
                    return false;
                };
                self.set(state, dest, v);
                return true;
            }
            _ => return false,
        };
        let Ok(value) = value else { return false };
        self.set(state, dest, value);
        true
    }

    /// `0 <= n < bits`, or `buri_rt_abort_shift`.
    fn shift_guard(&mut self, state: &mut Function<'ctx>, n: IntValue<'ctx>, bits: u64) {
        let t = n.get_type();
        let bool_ty = self.ctx.bool_type();
        let below = self
            .builder
            .build_int_compare(IntPredicate::SLT, n, t.const_zero(), "sh.lo")
            .unwrap_or_else(|_| bool_ty.const_zero());
        let above = self
            .builder
            .build_int_compare(IntPredicate::SGE, n, t.const_int(bits, false), "sh.hi")
            .unwrap_or_else(|_| bool_ty.const_zero());
        let bad = self.builder.build_or(below, above, "sh.bad").unwrap_or(above);
        let abort = self.ctx.append_basic_block(state.value, "sh.abort");
        let ok = self.ctx.append_basic_block(state.value, "sh.ok");
        let _ = self.builder.build_conditional_branch(bad, abort, ok);
        self.builder.position_at_end(abort);
        let f = self.rt_abort_named(runtime::ABORT_SHIFT);
        if let Ok(call) = self.builder.build_call(f, &[], "") {
            attrs::noreturn_call(self.ctx, call);
        }
        let _ = self.builder.build_unreachable();
        state.observed.aborts = true;
        self.builder.position_at_end(ok);
    }

    /// One LLVM intrinsic, by name and overload types.
    fn llvm_intrinsic(
        &mut self,
        name: &str,
        overloads: &[BasicTypeEnum<'ctx>],
        args: &[BasicMetadataValueEnum<'ctx>],
    ) -> Option<BasicValueEnum<'ctx>> {
        let intrinsic = inkwell::intrinsics::Intrinsic::find(name)?;
        let f = intrinsic.get_declaration(&self.module, overloads)?;
        self.builder.build_call(f, args, "int").ok()?.try_as_basic_value().basic()
    }

    /// `str.concat`, with MEMORY.md §5.3's in-place growth.
    ///
    /// Generated rather than called: the sequence is short enough that a `ccc`
    /// call would cost more than it saves.
    ///
    /// The ASCII flag is the **conjunction** of the two operands': a
    /// concatenation is all-ASCII exactly when both halves are, and the bit is
    /// a single one so an `and` of the two raw lengths carries it.
    ///
    /// # The three paths
    ///
    /// The same three `cli/runtime/text.rs`'s `buri_rt_str_concat` takes for the
    /// copy-and-patch backend, which calls rather than open-codes them. Its
    /// comment is the argument for why the first one is unobservable — a count
    /// of one
    /// means one live `Str` value, every alias of it carries the same `ptr` and
    /// `len`, and the write here starts at `ptr + len`:
    ///
    ///  1. **In place** — the left operand's block is uniquely owned and has
    ///     the room. Nothing is allocated and nothing is copied but the right
    ///     operand's bytes; the result is the same `base` and `ptr` with a
    ///     longer length, and takes a reference of its own.
    ///  2. **Grown** — uniquely owned but out of room: `max(n * 2,
    ///     GROWTH_FLOOR)` bytes, so the next step of a chain takes path 1.
    ///  3. **Exact** — shared, immortal or a literal: exactly `n` bytes, which
    ///     is what this emitted unconditionally before.
    ///
    /// A `memmove` on path 1 rather than a `memcpy`: where the right operand is
    /// a second view into the same block the two ranges can touch, and the
    /// weaker instruction removes the case from the argument rather than
    /// adding a test to it.
    fn concat(
        &mut self,
        state: &mut Function<'ctx>,
        code: &ir::Code,
        dest: ir::ValueId,
        args: &[ir::ValueId],
    ) -> bool {
        // Two shapes reach this key: `str.concat(self, ctx, other)` from a
        // method call, whose context is zero-sized and contributes no leaf, and
        // `str.concat(a, b)` from `lower::template`, which never had one. Both
        // flatten to the same six words, so the flattening is the check.
        let mut pieces: Vec<BasicValueEnum<'ctx>> = Vec::new();
        for a in args {
            // A context contributes nothing, whatever it weighs — the same rule
            // `entry_args` applies to `Arg::Dropped`, and for the same reason.
            // Here it is read off the *source* type rather than from a table,
            // because this key arrives in two shapes: `str.concat(self, ctx,
            // other)` from a method call and `str.concat(a, b)` from
            // `lower::template`, which never had one.
            if self.is_context(code.ty_of(*a)) {
                continue;
            }
            let slots = repr::ir_slots(&mut self.reprs, self.program, code.ty_of(*a));
            let value = self.get(state, *a);
            pieces.extend(repr::disassemble(&self.builder, &slots, value));
        }
        let (
            Some(BasicValueEnum::PointerValue(a_base)),
            Some(BasicValueEnum::PointerValue(a_ptr)),
            Some(BasicValueEnum::IntValue(a_raw)),
            Some(BasicValueEnum::PointerValue(b_ptr)),
            Some(BasicValueEnum::IntValue(b_raw)),
        ) = (
            pieces.get(layout::STR_BASE).copied(),
            pieces.get(layout::STR_PTR).copied(),
            pieces.get(layout::STR_LEN).copied(),
            pieces.get(3usize.saturating_add(layout::STR_PTR)).copied(),
            pieces.get(3usize.saturating_add(layout::STR_LEN)).copied(),
        )
        else {
            return false;
        };
        let word = self.ctx.i64_type();
        let mask = word.const_int(STR_LEN_MASK, false);
        let a_len = self.builder.build_and(a_raw, mask, "cat.alen").unwrap_or(a_raw);
        let b_len = self.builder.build_and(b_raw, mask, "cat.blen").unwrap_or(b_raw);
        let total = self.builder.build_int_add(a_len, b_len, "cat.len").unwrap_or(a_len);

        let probe = self.ctx.append_basic_block(state.value, "cat.probe");
        let check = self.ctx.append_basic_block(state.value, "cat.check");
        let inplace = self.ctx.append_basic_block(state.value, "cat.inplace");
        let fresh = self.ctx.append_basic_block(state.value, "cat.fresh");
        let join = self.ctx.append_basic_block(state.value, "cat.join");

        // The header load stays behind the null test: a literal and a static
        // have no base, and `base == 0` is how a `Str` says so.
        let Some(entry) = self.builder.get_insert_block() else { return false };
        let Ok(no_base) = self.builder.build_is_null(a_base, "cat.nobase") else { return false };
        let _ = self.builder.build_conditional_branch(no_base, check, probe);

        self.builder.position_at_end(probe);
        let rc_at =
            repr::byte_offset(self.ctx, &self.builder, a_base, i64::from(HEADER_RC_OFFSET), "cat.rcp");
        let cap_at = repr::byte_offset(
            self.ctx,
            &self.builder,
            a_base,
            i64::from(HEADER_CAP_OFFSET),
            "cat.capp",
        );
        let rc = match self.builder.build_load(word, rc_at, "cat.rc") {
            Ok(BasicValueEnum::IntValue(v)) => v,
            _ => word.const_zero(),
        };
        // The reserved multi-threaded bit lives in the top of the same word
        // (`layout::CAP_SHARED_FLAG`), and this is a room-for-the-result test:
        // an unmasked read would report the whole address space as headroom and
        // write past the block.
        let cap_raw = match self.builder.build_load(word, cap_at, "cat.cap.raw") {
            Ok(BasicValueEnum::IntValue(v)) => v,
            _ => word.const_zero(),
        };
        let cap = self
            .builder
            .build_and(cap_raw, word.const_int(CAP_MASK, false), "cat.cap")
            .unwrap_or(cap_raw);
        // `IMMORTAL` is `u64::MAX`, so a literal or an interned constant fails
        // this test by construction and never reaches either fast path.
        let is_one = self
            .builder
            .build_int_compare(IntPredicate::EQ, rc, word.const_int(1, false), "cat.one")
            .unwrap_or_else(|_| self.ctx.bool_type().const_zero());
        let _ = self.builder.build_unconditional_branch(check);

        self.builder.position_at_end(check);
        let bool_ty = self.ctx.bool_type();
        let unique = match self.builder.build_phi(bool_ty, "cat.uniq") {
            Ok(phi) => {
                phi.add_incoming(&[(&bool_ty.const_zero(), entry), (&is_one, probe)]);
                phi.as_basic_value().into_int_value()
            }
            Err(_) => bool_ty.const_zero(),
        };
        let room = match self.builder.build_phi(word, "cat.room") {
            Ok(phi) => {
                phi.add_incoming(&[(&word.const_zero(), entry), (&cap, probe)]);
                phi.as_basic_value().into_int_value()
            }
            Err(_) => word.const_zero(),
        };
        // What has to fit is the offset of the view inside the block plus the
        // whole result, because a `Str` may start in the middle of one.
        let a_at = self.builder.build_ptr_to_int(a_ptr, word, "cat.at").unwrap_or(total);
        let base_at = self.builder.build_ptr_to_int(a_base, word, "cat.base").unwrap_or(total);
        let offset = self.builder.build_int_sub(a_at, base_at, "cat.off").unwrap_or(total);
        let end = self.builder.build_int_add(offset, total, "cat.end").unwrap_or(total);
        let fits = self
            .builder
            .build_int_compare(IntPredicate::ULE, end, room, "cat.fits")
            .unwrap_or_else(|_| bool_ty.const_zero());
        let take = self.builder.build_and(unique, fits, "cat.take").unwrap_or(fits);
        let _ = self.builder.build_conditional_branch(take, inplace, fresh);

        self.builder.position_at_end(inplace);
        // SAFETY: `build_in_bounds_gep` is `unsafe` in inkwell because it
        // cannot check the index; the branch above established that
        // `offset + a_len + b_len` is within the block's capacity.
        let at = unsafe {
            self.builder
                .build_in_bounds_gep(self.ctx.i8_type(), a_ptr, &[a_len], "cat.end")
                .unwrap_or(a_ptr)
        };
        let _ = self.builder.build_memmove(at, 1, b_ptr, 1, b_len);
        self.incref_pointer(state, a_base, Counted::NonNull);
        // Both halves of MEMORY.md §5.3 write through `a_base`, which is
        // routinely a parameter: the `memmove` above and the count `incref`
        // just stored. `readonly` is a promise about bytes and this breaks it,
        // whatever a Buri caller can observe — see `attrs.rs`'s header.
        //
        // The pre-pass already reached the same answer by a shorter route:
        // `str.concat` is an `ir::Body::Runtime` callee, so `observe` seeded
        // every caller of it from `Observed::opaque`, which carries both bits.
        // This is the local statement of the same fact, so that the reason is
        // where the store is.
        state.observed.writes_args = true;
        state.observed.writes_far = true;
        let from_inplace = self.builder.get_insert_block().unwrap_or(inplace);
        let _ = self.builder.build_unconditional_branch(join);

        self.builder.position_at_end(fresh);
        let doubled = self.builder.build_int_add(total, total, "cat.x2").unwrap_or(total);
        let floor = word.const_int(layout::GROWTH_FLOOR, false);
        let over = self
            .builder
            .build_int_compare(IntPredicate::UGT, doubled, floor, "cat.big")
            .unwrap_or_else(|_| bool_ty.const_zero());
        let wanted = self
            .builder
            .build_select(over, doubled, floor, "cat.want")
            .map(BasicValueEnum::into_int_value)
            .unwrap_or(doubled);
        let size = self
            .builder
            .build_select(unique, wanted, total, "cat.size")
            .map(BasicValueEnum::into_int_value)
            .unwrap_or(total);
        let alloc = self.rt_alloc();
        state.observed.allocates = true;
        let block = match self.builder.build_call(alloc, &[size.into()], "cat") {
            Ok(call) => {
                attrs::set_call_convention(call, attrs::C);
                call.try_as_basic_value()
                    .basic()
                    .and_then(|v| v.try_into().ok())
                    .unwrap_or_else(|| self.ptr_ty().const_null())
            }
            Err(_) => self.ptr_ty().const_null(),
        };
        // One byte of claimed alignment on both sides: a `Str`'s `ptr` may be a
        // view into the middle of somebody else's allocation (VALUE-MODEL.md
        // §3), so nothing here knows more than that.
        let _ = self.builder.build_memcpy(block, 1, a_ptr, 1, a_len);
        // SAFETY: as above; `a_len` is inside a block of at least
        // `a_len + b_len` bytes this call just allocated.
        let second = unsafe {
            self.builder
                .build_in_bounds_gep(self.ctx.i8_type(), block, &[a_len], "cat.at")
                .unwrap_or(block)
        };
        let _ = self.builder.build_memcpy(second, 1, b_ptr, 1, b_len);
        let from_fresh = self.builder.get_insert_block().unwrap_or(fresh);
        let _ = self.builder.build_unconditional_branch(join);

        self.builder.position_at_end(join);
        let ptr_ty = self.ptr_ty();
        let base = match self.builder.build_phi(ptr_ty, "cat.rbase") {
            Ok(phi) => {
                phi.add_incoming(&[(&a_base, from_inplace), (&block, from_fresh)]);
                phi.as_basic_value().into_pointer_value()
            }
            Err(_) => block,
        };
        let start = match self.builder.build_phi(ptr_ty, "cat.rptr") {
            Ok(phi) => {
                phi.add_incoming(&[(&a_ptr, from_inplace), (&block, from_fresh)]);
                phi.as_basic_value().into_pointer_value()
            }
            Err(_) => block,
        };

        let both = self.builder.build_and(a_raw, b_raw, "cat.both").unwrap_or(a_raw);
        let ascii = self
            .builder
            .build_and(both, word.const_int(STR_ASCII_FLAG, false), "cat.ascii")
            .unwrap_or(both);
        let stored = self.builder.build_or(total, ascii, "cat.raw").unwrap_or(total);

        let slots = repr::ir_slots(&mut self.reprs, self.program, code.ty_of(dest));
        let values = [base.into(), start.into(), stored.into()];
        let value = repr::assemble(self.ctx, &self.builder, &slots, &values);
        self.set(state, dest, value);
        true
    }
}

// ---------------------------------------------------------------------------
// The closure surface of `core/list`
// ---------------------------------------------------------------------------

/// The `[T]` a `list.*` loop walks: where the elements are, how far apart, and
/// what one of them is.
struct Source<'ctx> {
    /// The block's payload pointer. Nothing is loaded out of it outside the
    /// bound test, because `cli/runtime/list.rs` answers a null one for a list
    /// of no elements.
    base: PointerValue<'ctx>,
    len: IntValue<'ctx>,
    stride: u32,
    size: u32,
    align: u32,
    elem: Ty,
    slots: Vec<Slot>,
}

/// The closure a `list.*` loop steps with, taken apart once before the loop.
struct StepFn<'ctx> {
    target: PointerValue<'ctx>,
    env: PointerValue<'ctx>,
    /// The step's own result slots, read off the closure's `Ty::Fn` rather than
    /// off the destination: `count`'s predicate answers `Bool` and its
    /// destination is an `Int`, and `filter`'s answers `Bool` and its
    /// destination is a `[T]`.
    rets: Vec<Slot>,
}

/// `Order.Greater`'s tag.
///
/// `core/order` declares `Less`, `Equal`, `Greater` in that order and
/// `middle::layout` gives a three-variant enum a bare tag that *is* the variant
/// index. The same three integers are what `buri_rt_str_compare` returns and
/// what `runtime.js`'s `$list_sortBy` reads, so this is one spelling of a
/// number three files already agree on rather than a new convention.
const GREATER: u64 = 2;

/// One resolved call: the block to walk, the closure to step with, and the two
/// operands only some of the loops have.
///
/// A struct rather than four more parameters, for the reason [`Wide`] is one:
/// the lint set caps a signature at seven, and these four travel together
/// through every loop below.
struct ListArgs<'ctx> {
    src: Source<'ctx>,
    step: StepFn<'ctx>,
    ctx: Option<ir::ValueId>,
    init: Option<ir::ValueId>,
}

/// One pass of [`Unit::list_sort`]: the run width, twice it, and the two blocks
/// the pass reads from and writes to.
struct Runs<'ctx> {
    width: IntValue<'ctx>,
    span: IntValue<'ctx>,
    read_from: PointerValue<'ctx>,
    write_to: PointerValue<'ctx>,
}

/// One `[T]` read, settled: the block, its length, the index and the element
/// type. What [`Unit::element_at`] answers and [`Unit::load_element`] consumes.
struct ElementRead<'ctx> {
    base: PointerValue<'ctx>,
    len: IntValue<'ctx>,
    index: IntValue<'ctx>,
    element: Ty,
}

/// One counted loop under construction: the blocks, the index, and the value
/// carried across iterations.
struct Loop<'ctx> {
    pre: BasicBlock<'ctx>,
    header: BasicBlock<'ctx>,
    done: BasicBlock<'ctx>,
    index: inkwell::values::PhiValue<'ctx>,
    i: IntValue<'ctx>,
    /// The phi and the value it starts from, where the loop carries one.
    carried: Option<(inkwell::values::PhiValue<'ctx>, BasicValueEnum<'ctx>)>,
}

impl<'ctx, 'a> Unit<'ctx, 'a> {
    /// The `list.*` entries that take a function, as a loop over the block.
    ///
    /// `cli/runtime/list.rs`'s header says why none of these is a runtime call:
    /// a Buri closure is `{ code, env }` where `code`'s signature is the
    /// *flattened* one of the element type (VALUE-MODEL.md §5.1), so a C
    /// function calling one would have to synthesize a parameter list that
    /// depends on `T`. A backend already knows how — it is an indirect call and
    /// nothing else — so the loop lives here. `stencil/lists.rs::list_call`
    /// open-codes the same nine; the two agree on the conventions below because
    /// a closure built by one is called by the other in the same artifact's
    /// tests.
    ///
    /// # The counts, which is the whole of the difficulty
    ///
    /// Two sentences from `middle/rc.rs` decide every reference operation here,
    /// and they point in opposite directions:
    ///
    ///  * **"A runtime intrinsic borrows its arguments and returns a fresh
    ///    count."** So the source list, the closure and `fold`'s initial
    ///    accumulator arrive borrowed: nothing here releases one, and the
    ///    result leaves owning what it holds.
    ///  * **"A call through a function value owns its arguments."** So every
    ///    value handed to a step is retained first — the element, the threaded
    ///    context, and `fold`'s accumulator on the way in — the step consumes
    ///    that count and answers a fresh one, and [`Unit::build_thunk`]
    ///    releases it again where the callee only borrowed it.
    ///
    /// The retain asks [`Unit::rc_counted`] and not the layout table, for the
    /// reason that function states: retaining what rc does not count is one
    /// half of a pair nothing completes, which is a leak per element. An
    /// element type holding nothing counted — `[Int]`, `[U8]`, a struct of
    /// scalars, which is most of them — costs no instruction at all.
    ///
    /// # The whole family, and where each half lives
    ///
    /// Every entry of `core/list` that takes a function is here. `find`,
    /// `findIndex` and `foldResult` build an `Option` or a `Result` around
    /// their answer, through the same [`Unit::build_variant`] that
    /// `ir::Inst::MakeEnum` goes through. `zip` and `flatten` take no function
    /// at all and are [`Unit::list_zip`] and [`Unit::list_flatten`], for the
    /// reason `cli/runtime/list.rs`'s header gives: each needs a *second*
    /// layout that no C signature carries.
    fn list_closure(
        &mut self,
        state: &mut Function<'ctx>,
        code: &ir::Code,
        dests: &[ir::ValueId],
        key: &str,
        args: &[ir::ValueId],
    ) -> bool {
        let Some(call) = list_call(key) else { return false };
        let (Some(xs), Some(f), Some(dest)) =
            (args.first().copied(), args.get(call.func).copied(), dests.first().copied())
        else {
            return false;
        };
        let ctx = call.ctx.and_then(|i| args.get(i).copied());
        let init = call.init.and_then(|i| args.get(i).copied());
        let (Some(src), Some(step)) =
            (self.list_source(state, code, xs), self.step_fn(state, code, f))
        else {
            return false;
        };
        let a = ListArgs { src, step, ctx, init };
        match call.kind {
            Step::Map => self.list_map(state, code, dest, &a),
            Step::Filter => self.list_filter(state, code, dest, &a),
            Step::Fold => self.list_fold(state, code, dest, &a),
            Step::FoldResult => self.list_fold_result(state, code, dest, &a),
            Step::Sort => self.list_sort(state, code, dest, &a),
            Step::Find | Step::FindIndex => {
                self.list_find(state, code, dest, &a, call.kind);
            }
            kind => self.list_test(state, code, dest, &a, kind),
        }
        true
    }

    /// The three things every loop over a `[T]` starts from.
    fn list_source(
        &mut self,
        state: &mut Function<'ctx>,
        code: &ir::Code,
        xs: ir::ValueId,
    ) -> Option<Source<'ctx>> {
        let ir_ty = code.ty_of(xs);
        let elem = self.type_of(ir_ty).and_then(|t| self.reprs.element(&t))?;
        let slots = repr::ir_slots(&mut self.reprs, self.program, ir_ty);
        let value = self.get(state, xs);
        let pieces = repr::disassemble(&self.builder, &slots, value);
        let (
            Some(BasicValueEnum::PointerValue(base)),
            Some(BasicValueEnum::IntValue(len)),
        ) = (pieces.get(layout::LIST_PTR).copied(), pieces.get(layout::LIST_LEN).copied())
        else {
            return None;
        };
        let r = self.reprs.of_ty(&elem);
        let (stride, size, align, slots) =
            (r.layout.stride, r.layout.size, r.layout.align, r.slots.clone());
        Some(Source { base, len, stride, size, align, elem, slots })
    }

    /// The closure, taken apart once in the block before the loop so that the
    /// two words dominate every iteration.
    fn step_fn(
        &mut self,
        state: &mut Function<'ctx>,
        code: &ir::Code,
        f: ir::ValueId,
    ) -> Option<StepFn<'ctx>> {
        let ir_ty = code.ty_of(f);
        let slots = repr::ir_slots(&mut self.reprs, self.program, ir_ty);
        let value = self.get(state, f);
        let pieces = repr::disassemble(&self.builder, &slots, value);
        let (
            Some(BasicValueEnum::PointerValue(target)),
            Some(BasicValueEnum::PointerValue(env)),
        ) = (
            pieces.get(layout::CLOSURE_CODE).copied(),
            pieces.get(layout::CLOSURE_ENV).copied(),
        )
        else {
            return None;
        };
        let Some(Ty::Fn(_, ret)) = self.type_of(ir_ty) else { return None };
        let rets = self.reprs.of_ty(&ret).slots.clone();
        Some(StepFn { target, env, rets })
    }

    /// One step, through the closure's own two words: the same indirect call
    /// [`Unit::call_indirect`] emits, with the environment first.
    fn call_step(
        &mut self,
        state: &mut Function<'ctx>,
        step: &StepFn<'ctx>,
        params: &[Slot],
        args: &[BasicMetadataValueEnum<'ctx>],
    ) -> Option<BasicValueEnum<'ctx>> {
        let ty = self.closure_fn_type(params, &step.rets);
        let mut argv: Vec<BasicMetadataValueEnum<'ctx>> = vec![step.env.into()];
        argv.extend_from_slice(args);
        let call = self.builder.build_indirect_call(ty, step.target, &argv, "").ok()?;
        attrs::set_call_convention(call, attrs::FAST);
        // An indirect callee's effects are not known here.
        state.observed.opaque = true;
        call.try_as_basic_value().basic()
    }

    /// `base + i * stride`.
    fn elem_at(
        &self,
        base: PointerValue<'ctx>,
        i: IntValue<'ctx>,
        stride: u32,
        name: &str,
    ) -> PointerValue<'ctx> {
        let word = self.ctx.i64_type();
        let scaled = self
            .builder
            .build_int_mul(i, word.const_int(u64::from(stride), false), "list.off")
            .unwrap_or(i);
        // SAFETY: inkwell marks `build_in_bounds_gep` unsafe because it cannot
        // check the index; every one here is below the block's element count.
        unsafe {
            self.builder
                .build_in_bounds_gep(self.ctx.i8_type(), base, &[scaled], name)
                .unwrap_or(base)
        }
    }

    /// A fresh block of `bytes` payload bytes, whose `cap` header is exactly
    /// what was asked for — which is what makes `cap / stride` the element
    /// count the drop glue walks (VALUE-MODEL.md §4).
    fn heap(
        &mut self,
        state: &mut Function<'ctx>,
        bytes: IntValue<'ctx>,
        name: &str,
    ) -> PointerValue<'ctx> {
        let alloc = self.rt_alloc();
        state.observed.allocates = true;
        match self.builder.build_call(alloc, &[bytes.into()], name) {
            Ok(call) => {
                attrs::set_call_convention(call, attrs::C);
                call.try_as_basic_value()
                    .basic()
                    .and_then(|v| v.try_into().ok())
                    .unwrap_or_else(|| self.ptr_ty().const_null())
            }
            Err(_) => self.ptr_ty().const_null(),
        }
    }

    /// The context a `*Ctx` step is threaded, retained and flattened.
    fn pass_ctx(
        &mut self,
        state: &mut Function<'ctx>,
        code: &ir::Code,
        ctx: Option<ir::ValueId>,
        params: &mut Vec<Slot>,
        argv: &mut Vec<BasicMetadataValueEnum<'ctx>>,
    ) {
        let Some(c) = ctx else { return };
        let ir_ty = code.ty_of(c);
        let slots = repr::ir_slots(&mut self.reprs, self.program, ir_ty);
        let value = self.get(state, c);
        let pieces = repr::disassemble(&self.builder, &slots, value);
        if let Some(ty) = self.type_of(ir_ty) {
            if self.rc_counted(&ty) {
                let place =
                    Place::Registers { slots: slots.clone(), pieces: pieces.clone() };
                self.walk_rc(state, &ty, &place, 0, true, 0);
            }
        }
        params.extend_from_slice(&slots);
        argv.extend(pieces.into_iter().map(BasicMetadataValueEnum::from));
    }

    /// One element, retained and flattened.
    fn pass_elem(
        &mut self,
        state: &mut Function<'ctx>,
        src: &Source<'ctx>,
        at: PointerValue<'ctx>,
        params: &mut Vec<Slot>,
        argv: &mut Vec<BasicMetadataValueEnum<'ctx>>,
    ) {
        if self.rc_counted(&src.elem) {
            let elem = src.elem.clone();
            let place = Place::Memory { base: at, align: src.align };
            self.walk_rc(state, &elem, &place, 0, true, 0);
        }
        for piece in self.load_slots(at, &src.slots, src.align) {
            argv.push(piece.into());
        }
        params.extend_from_slice(&src.slots);
    }

    /// An aggregate already in registers, flattened into a step's arguments.
    fn pass_value(
        &mut self,
        value: BasicValueEnum<'ctx>,
        slots: &[Slot],
        params: &mut Vec<Slot>,
        argv: &mut Vec<BasicMetadataValueEnum<'ctx>>,
    ) {
        for piece in repr::disassemble(&self.builder, slots, value) {
            argv.push(piece.into());
        }
        params.extend_from_slice(slots);
    }

    /// Opens `for i in 0..len`, leaving the builder in the body.
    ///
    /// The index is a phi and not memory, which is CODEGEN-LLVM.md §2 applied
    /// to a loop this backend generates rather than one it lowers.
    fn open_loop(
        &mut self,
        state: &mut Function<'ctx>,
        len: IntValue<'ctx>,
        carried: Option<BasicValueEnum<'ctx>>,
    ) -> Option<Loop<'ctx>> {
        let word = self.ctx.i64_type();
        let pre = self.builder.get_insert_block()?;
        let header = self.ctx.append_basic_block(state.value, "list.head");
        let body = self.ctx.append_basic_block(state.value, "list.body");
        let done = self.ctx.append_basic_block(state.value, "list.done");
        self.builder.build_unconditional_branch(header).ok()?;

        self.builder.position_at_end(header);
        let index = self.builder.build_phi(word, "list.i").ok()?;
        let carried = match carried {
            Some(v) => Some((self.builder.build_phi(v.get_type(), "list.acc").ok()?, v)),
            None => None,
        };
        let i: IntValue<'ctx> = index.as_basic_value().try_into().ok()?;
        let more = self.builder.build_int_compare(IntPredicate::ULT, i, len, "list.more").ok()?;
        self.builder.build_conditional_branch(more, body, done).ok()?;

        self.builder.position_at_end(body);
        Some(Loop { pre, header, done, index, i, carried })
    }

    /// Closes the loop and leaves the builder in `done`.
    ///
    /// The back edge names the block the body *ended* in, not the block it
    /// began in: a retain over a tagged enum or a niche leaves the builder in a
    /// join block it created, and a phi whose incoming block is not a real
    /// predecessor is the one error LLVM's verifier catches here.
    fn close_loop(&mut self, l: &Loop<'ctx>, next: Option<BasicValueEnum<'ctx>>) {
        let word = self.ctx.i64_type();
        let bumped = self
            .builder
            .build_int_add(l.i, word.const_int(1, false), "list.next")
            .unwrap_or(l.i);
        let latch = self.builder.get_insert_block().unwrap_or(l.header);
        let _ = self.builder.build_unconditional_branch(l.header);
        l.index.add_incoming(&[
            (&word.const_zero() as &dyn BasicValue<'ctx>, l.pre),
            (&bumped as &dyn BasicValue<'ctx>, latch),
        ]);
        if let Some((phi, start)) = &l.carried {
            let stepped = next.unwrap_or(*start);
            phi.add_incoming(&[
                (start as &dyn BasicValue<'ctx>, l.pre),
                (&stepped as &dyn BasicValue<'ctx>, latch),
            ]);
        }
        self.builder.position_at_end(l.done);
    }

    /// `map` and `mapCtx`: one fresh block of the same length, filled in place
    /// by the step.
    fn list_map(
        &mut self,
        state: &mut Function<'ctx>,
        code: &ir::Code,
        dest: ir::ValueId,
        a: &ListArgs<'ctx>,
    ) {
        let (src, step, ctx) = (&a.src, &a.step, a.ctx);
        let ir_dest = code.ty_of(dest);
        let Some(out_elem) = self.type_of(ir_dest).and_then(|t| self.reprs.element(&t)) else {
            return;
        };
        let (out_stride, out_align, out_slots) = {
            let r = self.reprs.of_ty(&out_elem);
            (r.layout.stride, r.layout.align, r.slots.clone())
        };
        let word = self.ctx.i64_type();
        let bytes = self
            .builder
            .build_int_mul(src.len, word.const_int(u64::from(out_stride), false), "map.bytes")
            .unwrap_or(src.len);
        let dst = self.heap(state, bytes, "map");
        let Some(l) = self.open_loop(state, src.len, None) else { return };

        let at = self.elem_at(src.base, l.i, src.stride, "map.at");
        let mut params = Vec::new();
        let mut argv = Vec::new();
        self.pass_ctx(state, code, ctx, &mut params, &mut argv);
        self.pass_elem(state, src, at, &mut params, &mut argv);
        let answer = self.call_step(state, step, &params, &argv);
        // The step's answer is a fresh count and the block is where it lives
        // from here: it moves in, so there is nothing to retain.
        let into = self.elem_at(dst, l.i, out_stride, "map.into");
        if let Some(v) = answer {
            let pieces = repr::disassemble(&self.builder, &out_slots, v);
            self.store_slots(into, &out_slots, out_align, &pieces);
        }
        self.close_loop(&l, None);

        let slots = repr::ir_slots(&mut self.reprs, self.program, ir_dest);
        let value =
            repr::assemble(self.ctx, &self.builder, &slots, &[dst.into(), src.len.into()]);
        self.set(state, dest, value);
    }

    /// `filter` and `filterCtx`: kept elements into a scratch block, then one
    /// exact block copied out of its used prefix.
    ///
    /// Two allocations rather than one, and the second is not avoidable: a
    /// `[T]`'s element count is `cap / stride` ([`Job::ReleaseElems`]), so an
    /// over-allocated block whose length is shorter than its capacity would
    /// have its *uninitialised* tail released when it died, and shrinking `cap`
    /// would lie to the allocator about which size class the block came from.
    /// The predicate runs exactly once per element, which is what rules out
    /// counting in a first pass — a `filterCtx` predicate may have effects.
    fn list_filter(
        &mut self,
        state: &mut Function<'ctx>,
        code: &ir::Code,
        dest: ir::ValueId,
        a: &ListArgs<'ctx>,
    ) {
        let (src, step, ctx) = (&a.src, &a.step, a.ctx);
        let word = self.ctx.i64_type();
        let stride = word.const_int(u64::from(src.stride), false);
        let bytes = self.builder.build_int_mul(src.len, stride, "filter.bytes").unwrap_or(src.len);
        let scratch = self.heap(state, bytes, "filter.buf");
        let zero = word.const_zero();
        let Some(l) = self.open_loop(state, src.len, Some(zero.into())) else { return };
        let kept: IntValue<'ctx> = match l.carried.map(|(p, _)| p.as_basic_value()) {
            Some(BasicValueEnum::IntValue(v)) => v,
            _ => zero,
        };

        let at = self.elem_at(src.base, l.i, src.stride, "filter.at");
        let mut params = Vec::new();
        let mut argv = Vec::new();
        self.pass_ctx(state, code, ctx, &mut params, &mut argv);
        self.pass_elem(state, src, at, &mut params, &mut argv);
        let answer = self.call_step(state, step, &params, &argv);
        let next = match answer {
            Some(BasicValueEnum::IntValue(b)) => {
                let keep = self.ctx.append_basic_block(state.value, "filter.keep");
                let skip = self.ctx.append_basic_block(state.value, "filter.skip");
                let cont = self.ctx.append_basic_block(state.value, "filter.cont");
                let _ = self.builder.build_conditional_branch(b, keep, skip);

                self.builder.position_at_end(keep);
                let into = self.elem_at(scratch, kept, src.stride, "filter.into");
                let align = src.align.max(1);
                let _ = self.builder.build_memcpy(
                    into,
                    align,
                    at,
                    align,
                    word.const_int(u64::from(src.size), false),
                );
                // The copy is a second owner of whatever the element holds, and
                // the count the predicate was handed was its own and is gone.
                // Both halves of *this* pair are the backend's own — the retain
                // here and the release the result list's glue does — so it asks
                // the layout table rather than rc.
                let elem = src.elem.clone();
                let place = Place::Memory { base: into, align };
                self.walk_rc(state, &elem, &place, 0, true, 0);
                let more = self
                    .builder
                    .build_int_add(kept, word.const_int(1, false), "filter.kept")
                    .unwrap_or(kept);
                let from_keep = self.builder.get_insert_block().unwrap_or(keep);
                let _ = self.builder.build_unconditional_branch(cont);

                self.builder.position_at_end(skip);
                let _ = self.builder.build_unconditional_branch(cont);

                self.builder.position_at_end(cont);
                match self.builder.build_phi(word, "filter.n") {
                    Ok(phi) => {
                        phi.add_incoming(&[
                            (&more as &dyn BasicValue<'ctx>, from_keep),
                            (&kept as &dyn BasicValue<'ctx>, skip),
                        ]);
                        phi.as_basic_value()
                    }
                    Err(_) => kept.into(),
                }
            }
            _ => kept.into(),
        };
        self.close_loop(&l, Some(next));

        let out_bytes = self.builder.build_int_mul(kept, stride, "filter.out").unwrap_or(kept);
        let out = self.heap(state, out_bytes, "filter");
        let align = src.align.max(1);
        let _ = self.builder.build_memcpy(out, align, scratch, align, out_bytes);
        // The elements were *moved*, so the scratch block goes back without its
        // contents being walked: its count is the one the allocation gave it.
        let free = self.rt_free();
        if let Ok(call) = self.builder.build_call(free, &[scratch.into()], "") {
            attrs::set_call_convention(call, attrs::C);
        }
        let slots = repr::ir_slots(&mut self.reprs, self.program, code.ty_of(dest));
        let value = repr::assemble(self.ctx, &self.builder, &slots, &[out.into(), kept.into()]);
        self.set(state, dest, value);
    }

    /// `find` and `findIndex`: the first element the predicate keeps, as an
    /// `Option`.
    ///
    /// `runtime.js`'s `$list_find` and `$list_findIndex` are the same walk and
    /// the same early exit, and the exit is legal here for [`Unit::list_test`]'s
    /// reason: the predicate is `fn(T) => Bool` (`list.buri`), which names no
    /// context, so SPEC 10.2 leaves it no effect with which to notice how many
    /// times it was called.
    ///
    /// # The counts
    ///
    /// One retain, on the element the answer carries, and it asks **rc's**
    /// question rather than the layout table's. The pair is not this backend's:
    /// the `Option` leaves owned and `middle::rc` is what releases it, so
    /// retaining where rc counts nothing would be one half of a pair nothing
    /// completes. [`Unit::list_filter`]'s retain asks the layout table instead,
    /// and correctly — there both halves are the backend's own, the second
    /// being the result block's element glue.
    ///
    /// `findIndex`'s payload is an `Int` and holds nothing counted, so its
    /// answer costs no reference operation at all.
    fn list_find(
        &mut self,
        state: &mut Function<'ctx>,
        code: &ir::Code,
        dest: ir::ValueId,
        a: &ListArgs<'ctx>,
        kind: Step,
    ) {
        let (src, step) = (&a.src, &a.step);
        let ir::Type::Agg(id) = code.ty_of(dest) else { return };
        let join = self.ctx.append_basic_block(state.value, "find.join");
        let Some(l) = self.open_loop(state, src.len, None) else { return };
        let at = self.elem_at(src.base, l.i, src.stride, "find.at");
        let mut params = Vec::new();
        let mut argv = Vec::new();
        self.pass_elem(state, src, at, &mut params, &mut argv);
        let hit = match self.call_step(state, step, &params, &argv) {
            Some(BasicValueEnum::IntValue(b)) => {
                let found = self.ctx.append_basic_block(state.value, "find.hit");
                let cont = self.ctx.append_basic_block(state.value, "find.cont");
                let _ = self.builder.build_conditional_branch(b, found, cont);

                self.builder.position_at_end(found);
                let parts = if kind == Step::FindIndex {
                    // `Option<Int>`'s payload is one 64-bit slot, which is the
                    // index the loop already carries.
                    let slot = Slot { offset: 0, ty: SlotTy::Scalar(Scalar::I64) };
                    vec![(vec![slot], vec![l.i.into()])]
                } else {
                    let pieces = self.load_slots(at, &src.slots, src.align);
                    if self.rc_counted(&src.elem) {
                        let elem = src.elem.clone();
                        let place = Place::Memory { base: at, align: src.align };
                        self.walk_rc(state, &elem, &place, 0, true, 0);
                    }
                    vec![(src.slots.clone(), pieces)]
                };
                let some = self.build_variant(id, 0, &parts);
                let from = self.builder.get_insert_block().unwrap_or(found);
                let _ = self.builder.build_unconditional_branch(join);
                self.builder.position_at_end(cont);
                some.map(|v| (v, from))
            }
            _ => None,
        };
        self.close_loop(&l, None);

        // Exhausted. `.None` is variant 1: `core/option` declares `Some` first.
        let none = self.build_variant(id, 1, &[]);
        let missed = self.builder.get_insert_block().unwrap_or(l.done);
        let _ = self.builder.build_unconditional_branch(join);
        self.builder.position_at_end(join);
        let Some(none) = none else { return };
        let value = match hit {
            Some((some, from)) => match self.builder.build_phi(none.get_type(), "find.answer") {
                Ok(phi) => {
                    phi.add_incoming(&[
                        (&some as &dyn BasicValue<'ctx>, from),
                        (&none as &dyn BasicValue<'ctx>, missed),
                    ]);
                    phi.as_basic_value()
                }
                Err(_) => none,
            },
            None => none,
        };
        self.set(state, dest, value);
    }

    /// `foldResult` and `foldResultCtx`: a fold that stops at the first `.Err`.
    ///
    /// `runtime.js`'s `$list_foldResult` is transcribed rather than
    /// approximated, because the shape of its short circuit is what the answer
    /// is:
    ///
    /// ```js
    /// let cur = [0, acc];
    /// for (…) { cur = f(cur[1], xs[i]); if (cur[0] !== 0) return cur; }
    /// return cur;
    /// ```
    ///
    /// So an empty list answers `.Ok(init)` — a `Result` nothing built — and a
    /// step that answers `.Err` is handed back **exactly as it came**, payload
    /// and all, without the remaining elements being visited. `.Ok`'s
    /// discriminant is `0` because `core/result` declares `Ok` first, which is
    /// the same sentence `$list_foldResult`'s `!== 0` rests on.
    ///
    /// # The counts
    ///
    /// One retain, on the way in, for [`Unit::list_fold`]'s reason: a call
    /// through a function value owns its arguments and the initial accumulator
    /// arrives borrowed. After that each step consumes the count it is handed
    /// and answers another inside its `.Ok`, which is what the next step is
    /// handed. The early exit therefore leaks nothing — the step that answered
    /// `.Err` consumed the accumulator's count on the way in.
    fn list_fold_result(
        &mut self,
        state: &mut Function<'ctx>,
        code: &ir::Code,
        dest: ir::ValueId,
        a: &ListArgs<'ctx>,
    ) {
        let (src, step, ctx) = (&a.src, &a.step, a.ctx);
        let (Some(init), ir::Type::Agg(id)) = (a.init, code.ty_of(dest)) else { return };
        let (slots, enum_repr, ok_offsets, none_variant, some_variant) = {
            let r = self.reprs.of(self.program, id);
            let (none_at, some_at) = match &r.layout.repr {
                LayoutRepr::Enum { variants, .. } => (
                    variants.iter().position(Vec::is_empty).unwrap_or(1),
                    variants.iter().position(|v| !v.is_empty()).unwrap_or(0),
                ),
                _ => (1, 0),
            };
            (r.slots.clone(), r.enum_repr().cloned(), r.layout.variant(0).to_vec(), none_at, some_at)
        };
        let Some(enum_repr) = enum_repr else { return };
        let acc_slots = repr::ir_slots(&mut self.reprs, self.program, code.ty_of(init));
        let start = self.get(state, init);
        if let Some(ty) = self.type_of(code.ty_of(init)) {
            if self.rc_counted(&ty) {
                let pieces = repr::disassemble(&self.builder, &acc_slots, start);
                let place = Place::Registers { slots: acc_slots.clone(), pieces };
                self.walk_rc(state, &ty, &place, 0, true, 0);
            }
        }
        let join = self.ctx.append_basic_block(state.value, "fr.join");
        let Some(l) = self.open_loop(state, src.len, Some(start)) else { return };
        let acc = l.carried.map_or(start, |(p, _)| p.as_basic_value());

        let at = self.elem_at(src.base, l.i, src.stride, "fr.at");
        let mut params = Vec::new();
        let mut argv = Vec::new();
        self.pass_ctx(state, code, ctx, &mut params, &mut argv);
        self.pass_value(acc, &acc_slots, &mut params, &mut argv);
        self.pass_elem(state, src, at, &mut params, &mut argv);
        let Some(stepped) = self.call_step(state, step, &params, &argv) else { return };
        let tag = self.tag_of(&slots, &enum_repr, none_variant, some_variant, stepped);
        let BasicValueEnum::IntValue(tag) = tag else { return };
        let failed = self
            .builder
            .build_int_compare(IntPredicate::NE, tag, tag.get_type().const_zero(), "fr.err")
            .unwrap_or_else(|_| self.ctx.bool_type().const_zero());
        let bad = self.ctx.append_basic_block(state.value, "fr.bad");
        let carry = self.ctx.append_basic_block(state.value, "fr.carry");
        let _ = self.builder.build_conditional_branch(failed, bad, carry);
        self.builder.position_at_end(bad);
        let _ = self.builder.build_unconditional_branch(join);

        self.builder.position_at_end(carry);
        let next = self.payload_of(&slots, &enum_repr, &ok_offsets, 0, &acc_slots, stepped);
        self.close_loop(&l, Some(next));

        // Exhausted: `.Ok(acc)`, which is the `cur` a loop that never ran also
        // answers.
        let pieces = repr::disassemble(&self.builder, &acc_slots, acc);
        let ok = self.build_variant(id, 0, &[(acc_slots.clone(), pieces)]);
        let done = self.builder.get_insert_block().unwrap_or(l.done);
        let _ = self.builder.build_unconditional_branch(join);
        self.builder.position_at_end(join);
        let Some(ok) = ok else { return };
        let value = match self.builder.build_phi(ok.get_type(), "fr.answer") {
            Ok(phi) => {
                phi.add_incoming(&[
                    (&stepped as &dyn BasicValue<'ctx>, bad),
                    (&ok as &dyn BasicValue<'ctx>, done),
                ]);
                phi.as_basic_value()
            }
            Err(_) => ok,
        };
        self.set(state, dest, value);
    }

    /// `list.get(self, index) -> Option<T>`, open-coded: the unsigned bounds
    /// compare and the load `xs[i]` already lowers to, in place of a call whose
    /// stride is a parameter and whose copy is therefore a `memcpy`.
    ///
    /// `middle::lower::index` open-codes the `xs[i]` sugar "so that the
    /// optimizer can see it: a bound it can prove is a bound it can delete",
    /// and `list.buri` says the two spellings are one operation — but only the
    /// sugar reached [`Unit::array_get`]. This is the other spelling reaching
    /// the same three lines.
    ///
    /// **A counted element declines**, exactly as the stencil backend's
    /// `list_get` does: `buri_rt_list_get` is handed a retain glue and performs
    /// it, this sequence does not, and `middle::rc` plans over the tree that
    /// distinguishes them. The fallback is the table entry, which is where the
    /// key went before.
    fn list_get(
        &mut self,
        state: &mut Function<'ctx>,
        code: &ir::Code,
        dest: ir::ValueId,
        args: &[ir::ValueId],
    ) -> bool {
        let (Some(xs), Some(index)) = (args.first().copied(), args.get(1).copied()) else {
            return false;
        };
        let (ir::Type::Agg(list_id), ir::Type::Agg(opt_id)) = (code.ty_of(xs), code.ty_of(dest))
        else {
            return false;
        };
        let list_ty = self.reprs.of(self.program, list_id).ty.clone();
        let Some(element) = self.reprs.element(&list_ty) else { return false };
        if self.reprs.counted_type(&element) {
            return false;
        }
        let (opt_slots, has_repr) = {
            let r = self.reprs.of(self.program, opt_id);
            (r.slots.clone(), r.enum_repr().is_some())
        };
        if !has_repr {
            return false;
        }
        let (some_at, none_at) = self.option_variants(opt_id);
        // The read is settled before the first block is opened, so that nothing
        // below can decline half-emitted.
        let Some(read) = self.element_at(state, code, xs, index) else { return false };
        // One unsigned compare covers both bounds: an `Int` is a whole 64-bit
        // word, so a negative index reads as a very large unsigned one and
        // fails `index < len` exactly as `buri_rt_list_get`'s `index < 0` does.
        // A null block has length zero, so the same compare answers for that
        // entry's `ptr.is_null()` too.
        let in_bounds = self
            .builder
            .build_int_compare(IntPredicate::ULT, read.index, read.len, "get.in")
            .unwrap_or_else(|_| self.ctx.bool_type().const_zero());
        let some_bb = self.ctx.append_basic_block(state.value, "get.some");
        let none_bb = self.ctx.append_basic_block(state.value, "get.none");
        let join = self.ctx.append_basic_block(state.value, "get.done");
        let _ = self.builder.build_conditional_branch(in_bounds, some_bb, none_bb);

        let ty = repr::register_type(self.ctx, &opt_slots);
        self.builder.position_at_end(some_bb);
        let loaded = self.load_element(read.base, read.index, &read.element);
        let some_value =
            self.build_variant(opt_id, some_at, &[loaded]).unwrap_or_else(|| ty.const_zero());
        let _ = self.builder.build_unconditional_branch(join);

        self.builder.position_at_end(none_bb);
        let none_value =
            self.build_variant(opt_id, none_at, &[]).unwrap_or_else(|| ty.const_zero());
        let _ = self.builder.build_unconditional_branch(join);

        self.builder.position_at_end(join);
        if let Ok(phi) = self.builder.build_phi(ty, "get") {
            phi.add_incoming(&[
                (&some_value as &dyn BasicValue<'ctx>, some_bb),
                (&none_value as &dyn BasicValue<'ctx>, none_bb),
            ]);
            self.set(state, dest, phi.as_basic_value());
        }
        true
    }

    /// `zip`: one block of pairs, as long as the shorter of the two.
    ///
    /// `runtime.js`'s `$list_zip` takes `Math.min` of the two lengths, so
    /// unequal inputs are not an error and the surplus is dropped — which is
    /// what makes the paired indexing below in bounds.
    ///
    /// `cli/runtime/list.rs`'s header says why this is not a runtime call: the
    /// element layout of a `[(A, B)]` is `middle::layout`'s answer — the second
    /// field's offset after alignment — and not a function of the two strides a
    /// C entry could be handed.
    ///
    /// # The counts
    ///
    /// Both sources are borrowed and both copies are a second owner, so each
    /// half of every pair is retained once, against the result block's own
    /// element glue — [`Unit::list_filter`]'s pair, twice.
    fn list_zip(
        &mut self,
        state: &mut Function<'ctx>,
        code: &ir::Code,
        dests: &[ir::ValueId],
        key: &str,
        args: &[ir::ValueId],
    ) -> bool {
        if key != "list.zip" {
            return false;
        }
        let (Some(xs), Some(ys), Some(dest)) =
            (args.first().copied(), args.get(2).copied(), dests.first().copied())
        else {
            return false;
        };
        let (Some(a), Some(b)) =
            (self.list_source(state, code, xs), self.list_source(state, code, ys))
        else {
            return false;
        };
        let ir_dest = code.ty_of(dest);
        let Some(out_elem) = self.type_of(ir_dest).and_then(|t| self.reprs.element(&t)) else {
            return false;
        };
        let (out_stride, out_align, fields) = {
            let r = self.reprs.of_ty(&out_elem);
            (r.layout.stride, r.layout.align, r.layout.fields.clone())
        };
        let word = self.ctx.i64_type();
        let shorter = self
            .builder
            .build_int_compare(IntPredicate::ULT, a.len, b.len, "zip.shorter")
            .unwrap_or_else(|_| self.ctx.bool_type().const_zero());
        let n: IntValue<'ctx> = match self.builder.build_select(shorter, a.len, b.len, "zip.n") {
            Ok(BasicValueEnum::IntValue(v)) => v,
            _ => a.len,
        };
        let bytes = self
            .builder
            .build_int_mul(n, word.const_int(u64::from(out_stride), false), "zip.bytes")
            .unwrap_or(n);
        let dst = self.heap(state, bytes, "zip");
        let Some(l) = self.open_loop(state, n, None) else { return true };
        let into = self.elem_at(dst, l.i, out_stride, "zip.into");
        let sides = [(&a, fields.first().copied().unwrap_or(0)), (&b, fields.get(1).copied().unwrap_or(0))];
        for (side, field_at) in sides {
            let from = self.elem_at(side.base, l.i, side.stride, "zip.from");
            let target =
                repr::byte_offset(self.ctx, &self.builder, into, i64::from(field_at), "zip.field");
            let align = side.align.max(1).min(out_align.max(1));
            let _ = self.builder.build_memcpy(
                target,
                align,
                from,
                side.align.max(1),
                word.const_int(u64::from(side.size), false),
            );
            let elem = side.elem.clone();
            let place = Place::Memory { base: target, align };
            self.walk_rc(state, &elem, &place, 0, true, 0);
        }
        self.close_loop(&l, None);
        let slots = repr::ir_slots(&mut self.reprs, self.program, ir_dest);
        let value = repr::assemble(self.ctx, &self.builder, &slots, &[dst.into(), n.into()]);
        self.set(state, dest, value);
        true
    }

    /// `flatten`: one block holding every element of every inner block.
    ///
    /// Two passes, because the result's length is the sum of the inner lengths
    /// and a `[T]`'s element count is `cap / stride` ([`Job::ReleaseElems`]) —
    /// an over-allocated block would have its uninitialised tail released when
    /// it died. The first pass reads `len` out of each descriptor and nothing
    /// else, which is a load per inner list and no call.
    ///
    /// `cli/runtime/list.rs`'s header says why this is not a runtime call: the
    /// elements of a `[[T]]` are descriptors pointing at blocks whose stride
    /// the entry does not carry.
    ///
    /// # The counts
    ///
    /// The outer block and every inner one are borrowed, and each element
    /// copied out is a second owner — one retain per element, against the
    /// result block's own glue, exactly as [`Unit::list_filter`]'s is. The
    /// inner *descriptors* are not touched: a `[[T]]` that dies releases its
    /// own elements, and this walks past them into the blocks they name.
    fn list_flatten(
        &mut self,
        state: &mut Function<'ctx>,
        code: &ir::Code,
        dests: &[ir::ValueId],
        key: &str,
        args: &[ir::ValueId],
    ) -> bool {
        if key != "list.flatten" {
            return false;
        }
        let (Some(xs), Some(dest)) = (args.first().copied(), dests.first().copied()) else {
            return false;
        };
        let Some(src) = self.list_source(state, code, xs) else { return false };
        let ir_dest = code.ty_of(dest);
        let Some(out_elem) = self.type_of(ir_dest).and_then(|t| self.reprs.element(&t)) else {
            return false;
        };
        let (out_stride, out_size, out_align) = {
            let r = self.reprs.of_ty(&out_elem);
            (r.layout.stride, r.layout.size, r.layout.align)
        };
        let word = self.ctx.i64_type();
        let zero = word.const_zero();

        // -- pass one: the total ---------------------------------------------
        let Some(counting) = self.open_loop(state, src.len, Some(zero.into())) else {
            return true;
        };
        let total: IntValue<'ctx> = match counting.carried.map(|(p, _)| p.as_basic_value()) {
            Some(BasicValueEnum::IntValue(v)) => v,
            _ => zero,
        };
        let at = self.elem_at(src.base, counting.i, src.stride, "flat.count.at");
        let pieces = self.load_slots(at, &src.slots, src.align);
        let inner = match pieces.get(layout::LIST_LEN).copied() {
            Some(BasicValueEnum::IntValue(v)) => v,
            _ => zero,
        };
        let grown = self.builder.build_int_add(total, inner, "flat.total").unwrap_or(total);
        self.close_loop(&counting, Some(grown.into()));

        let bytes = self
            .builder
            .build_int_mul(total, word.const_int(u64::from(out_stride), false), "flat.bytes")
            .unwrap_or(total);
        let dst = self.heap(state, bytes, "flat");

        // -- pass two: the elements ------------------------------------------
        let Some(outer) = self.open_loop(state, src.len, Some(zero.into())) else { return true };
        let filled: IntValue<'ctx> = match outer.carried.map(|(p, _)| p.as_basic_value()) {
            Some(BasicValueEnum::IntValue(v)) => v,
            _ => zero,
        };
        let at = self.elem_at(src.base, outer.i, src.stride, "flat.at");
        let pieces = self.load_slots(at, &src.slots, src.align);
        let (
            Some(BasicValueEnum::PointerValue(base)),
            Some(BasicValueEnum::IntValue(len)),
        ) = (pieces.get(layout::LIST_PTR).copied(), pieces.get(layout::LIST_LEN).copied())
        else {
            return true;
        };
        let Some(one) = self.open_loop(state, len, None) else { return true };
        let from = self.elem_at(base, one.i, out_stride, "flat.from");
        let k = self.builder.build_int_add(filled, one.i, "flat.k").unwrap_or(one.i);
        let into = self.elem_at(dst, k, out_stride, "flat.into");
        let align = out_align.max(1);
        let _ = self.builder.build_memcpy(
            into,
            align,
            from,
            align,
            word.const_int(u64::from(out_size), false),
        );
        let elem = out_elem.clone();
        let place = Place::Memory { base: into, align };
        self.walk_rc(state, &elem, &place, 0, true, 0);
        self.close_loop(&one, None);
        let after = self.builder.build_int_add(filled, len, "flat.after").unwrap_or(filled);
        self.close_loop(&outer, Some(after.into()));

        let slots = repr::ir_slots(&mut self.reprs, self.program, ir_dest);
        let value = repr::assemble(self.ctx, &self.builder, &slots, &[dst.into(), total.into()]);
        self.set(state, dest, value);
        true
    }

    /// `deriveArrayEq` — a derived `Eq` where the field is a `[T]`.
    ///
    /// `middle/derives.rs`'s header states the shape: `([T], [T], fn(T, T) ->
    /// Bool) -> Bool`, where the third argument is a **code pointer to the
    /// element's generated function**, "because a loop is not expressible in
    /// the layer-A tree and every backend has the loop already". This is that
    /// loop — `stencil/lists.rs::derive_array_eq` is the same one — and it is
    /// `list_test`'s `all` walking two blocks instead of one.
    ///
    /// Two lengths that differ answer `false` without calling the element's
    /// function at all, which is `$eq`'s own first test and is what makes the
    /// paired indexing below in bounds. The counts are
    /// [`Unit::list_closure`]'s, unchanged.
    ///
    /// `deriveArrayShow` is [`Unit::derive_array_show`]. The three that remain
    /// are not here, and none of the five is reported by `missing_intrinsics`:
    /// a `deriveArray*` is an `ExprKind::Intrinsic` inside a body
    /// `middle::derives` generated rather than a `FuncKind::Intrinsic` the hook
    /// can see, which is why `native/agreement.rs`'s `native_refusal` asks the
    /// backend to emit as well as asking the hook.
    fn derive_array(
        &mut self,
        state: &mut Function<'ctx>,
        code: &ir::Code,
        dests: &[ir::ValueId],
        key: &str,
        args: &[ir::ValueId],
    ) -> bool {
        if key == "deriveArrayShow" {
            return self.derive_array_show(state, code, dests, args);
        }
        if key != "deriveArrayEq" {
            return false;
        }
        let (Some(xs), Some(ys), Some(f), Some(dest)) = (
            args.first().copied(),
            args.get(1).copied(),
            args.get(2).copied(),
            dests.first().copied(),
        ) else {
            return false;
        };
        let (Some(a), Some(b), Some(step)) = (
            self.list_source(state, code, xs),
            self.list_source(state, code, ys),
            self.step_fn(state, code, f),
        ) else {
            return false;
        };
        let want = repr::ir_type(self.ctx, &mut self.reprs, self.program, code.ty_of(dest));
        let BasicTypeEnum::IntType(int) = want else { return false };

        let paired = self.ctx.append_basic_block(state.value, "eq.paired");
        let short = self.ctx.append_basic_block(state.value, "eq.short");
        let Ok(same) =
            self.builder.build_int_compare(IntPredicate::EQ, a.len, b.len, "eq.same")
        else {
            return false;
        };
        let _ = self.builder.build_conditional_branch(same, paired, short);
        self.builder.position_at_end(short);
        let differ = self.ctx.append_basic_block(state.value, "eq.differ");
        let _ = self.builder.build_unconditional_branch(differ);

        self.builder.position_at_end(paired);
        let Some(l) = self.open_loop(state, a.len, None) else { return false };
        let at_a = self.elem_at(a.base, l.i, a.stride, "eq.a");
        let at_b = self.elem_at(b.base, l.i, b.stride, "eq.b");
        let mut params = Vec::new();
        let mut argv = Vec::new();
        self.pass_elem(state, &a, at_a, &mut params, &mut argv);
        self.pass_elem(state, &b, at_b, &mut params, &mut argv);
        let early = match self.call_step(state, &step, &params, &argv) {
            Some(BasicValueEnum::IntValue(equal)) => {
                let cont = self.ctx.append_basic_block(state.value, "eq.cont");
                let from = self.builder.get_insert_block().unwrap_or(cont);
                let _ = self.builder.build_conditional_branch(equal, cont, differ);
                self.builder.position_at_end(cont);
                Some(from)
            }
            _ => None,
        };
        self.close_loop(&l, None);
        let exhausted = self.builder.get_insert_block().unwrap_or(l.done);
        let _ = self.builder.build_unconditional_branch(differ);

        // `differ` is reached from the length test, from the element that said
        // `false`, and from the loop running out — the last of which is the
        // only `true`. The phi is built over exactly the edges that exist,
        // because one naming a block that is not a predecessor is the error
        // LLVM's verifier catches here.
        self.builder.position_at_end(differ);
        let (no, yes) = (int.const_zero(), int.const_int(1, false));
        let mut incoming: Vec<(&dyn BasicValue<'ctx>, BasicBlock<'ctx>)> =
            vec![(&no, short), (&yes, exhausted)];
        if let Some(from) = early.as_ref() {
            incoming.push((&no, *from));
        }
        let value = match self.builder.build_phi(int, "eq.answer") {
            Ok(phi) => {
                phi.add_incoming(&incoming);
                phi.as_basic_value()
            }
            Err(_) => no.into(),
        };
        self.set(state, dest, value);
        true
    }

    /// `deriveArrayShow` — a derived `Show` where the field is a `[T]`.
    ///
    /// `middle/derives.rs`'s header states the shape: `([T], fn(T) -> Str) ->
    /// Str`, rendering `[a, b]` with the separator included. So the answer is
    /// two halves: **call the element's generated function once per element**,
    /// which only a backend can do (`cli/runtime/list.rs`'s header), and
    /// **join the results with brackets and `", "`**, which only the archive
    /// should do — `buri_rt_show_list` is that half, and
    /// `stencil/lists.rs::derive_array_show` calls the same symbol with the same
    /// three arguments.
    ///
    /// # The counts
    ///
    /// Each rendered `Str` arrives **owned** — the element's `show` is a
    /// function value and its answer is a fresh count — and the join *copies*
    /// bytes rather than taking a reference, so every one is released before
    /// the scratch block goes back. Without that loop a derived `show` of a
    /// `[Str]` would leak one block per element.
    fn derive_array_show(
        &mut self,
        state: &mut Function<'ctx>,
        code: &ir::Code,
        dests: &[ir::ValueId],
        args: &[ir::ValueId],
    ) -> bool {
        let (Some(xs), Some(f), Some(dest)) =
            (args.first().copied(), args.get(1).copied(), dests.first().copied())
        else {
            return false;
        };
        let (Some(src), Some(step)) =
            (self.list_source(state, code, xs), self.step_fn(state, code, f))
        else {
            return false;
        };
        let ir_dest = code.ty_of(dest);
        let Some(str_ty) = self.type_of(ir_dest) else { return false };
        let (stride, size, align, slots) = {
            let r = self.reprs.of_ty(&str_ty);
            (r.layout.stride.max(1), r.layout.size, r.layout.align.max(1), r.slots.clone())
        };
        let word = self.ctx.i64_type();
        let bytes = self
            .builder
            .build_int_mul(src.len, word.const_int(u64::from(stride), false), "show.bytes")
            .unwrap_or(src.len);
        let scratch = self.heap(state, bytes, "show.buf");

        // -- one rendered `Str` per element ----------------------------------
        let Some(l) = self.open_loop(state, src.len, None) else { return true };
        let at = self.elem_at(src.base, l.i, src.stride, "show.at");
        let mut params = Vec::new();
        let mut argv = Vec::new();
        self.pass_elem(state, &src, at, &mut params, &mut argv);
        let answer = self.call_step(state, &step, &params, &argv);
        let into = self.elem_at(scratch, l.i, stride, "show.into");
        if let Some(v) = answer {
            let pieces = repr::disassemble(&self.builder, &slots, v);
            self.store_slots(into, &slots, align, &pieces);
        }
        self.close_loop(&l, None);

        let join = self.declare_rt(
            runtime::SHOW_LIST,
            &[self.ptr_ty().into(), word.into(), self.ptr_ty().into()],
            None,
        );
        let out = self.scratch(state, size, align);
        if let Ok(call) =
            self.builder.build_call(join, &[scratch.into(), src.len.into(), out.into()], "")
        {
            attrs::set_call_convention(call, attrs::C);
        }
        let joined = self.load_slots(out, &slots, align);
        let value = repr::assemble(self.ctx, &self.builder, &slots, &joined);

        // -- the rendered strings, released ----------------------------------
        let Some(freeing) = self.open_loop(state, src.len, None) else { return true };
        let rendered = self.elem_at(scratch, freeing.i, stride, "show.free");
        let place = Place::Memory { base: rendered, align };
        let str_ty = str_ty.clone();
        self.walk_rc(state, &str_ty, &place, 0, false, 0);
        self.close_loop(&freeing, None);
        let free = self.rt_free();
        if let Ok(call) = self.builder.build_call(free, &[scratch.into()], "") {
            attrs::set_call_convention(call, attrs::C);
        }
        self.set(state, dest, value);
        true
    }

    /// `sortBy`: a **stable** sort of a copy of the block.
    ///
    /// # Why a merge, and why stable
    ///
    /// `runtime.js`'s `$list_sortBy` pairs each element with its index and
    /// breaks a tie the comparator answered `.Equal` by that index, which is a
    /// stable sort spelled out. A bottom-up merge is the same order for the
    /// same reason: a merge takes from the left run unless the comparator says
    /// the left element is strictly `.Greater`, so equal elements keep the
    /// order the source had. `data/lists.buri`'s "sortBy is stable" and "sortBy
    /// is stable when every key is equal" are the two that pin it, and
    /// `stencil/lists.rs::list_sort` is the same merge for the same reason —
    /// the two backends have to agree on the *answer*, and they do because the
    /// algorithm is the one the specification of stability names.
    ///
    /// The *sequence of comparisons* is neither backend's business and cannot
    /// be observed as anything else: `sortBy`'s comparator is `fn(T, T) =>
    /// Order` (`list.buri`), which names no context, so SPEC 10.2 leaves it no
    /// effect to notice a call with. `list_test`'s early exit rests on the same
    /// sentence.
    ///
    /// # The counts
    ///
    /// Every element move below is a `memcpy` of a value that is *moving*, so
    /// nothing is retained or released for it: after the last pass each element
    /// exists exactly once in the result, and the scratch block goes back
    /// without being walked, exactly as [`Unit::list_filter`]'s does. The one
    /// retain per element is the copy of the borrowed source into the result;
    /// the one pair per comparison is [`Unit::pass_elem`]'s, which the
    /// convention in [`Unit::list_closure`]'s header requires of any call
    /// through a function value.
    fn list_sort(
        &mut self,
        state: &mut Function<'ctx>,
        code: &ir::Code,
        dest: ir::ValueId,
        a: &ListArgs<'ctx>,
    ) {
        let src = &a.src;
        let word = self.ctx.i64_type();
        let align = src.align.max(1);
        let size = word.const_int(u64::from(src.size), false);
        let one = word.const_int(1, false);
        let stride = word.const_int(u64::from(src.stride), false);
        let bytes = self.builder.build_int_mul(src.len, stride, "sort.bytes").unwrap_or(src.len);
        let dst = self.heap(state, bytes, "sort");
        let scratch = self.heap(state, bytes, "sort.buf");

        // -- the source, copied in and retained once per element ------------
        let Some(l) = self.open_loop(state, src.len, None) else { return };
        let from = self.elem_at(src.base, l.i, src.stride, "sort.from");
        let into = self.elem_at(dst, l.i, src.stride, "sort.into");
        let _ = self.builder.build_memcpy(into, align, from, align, size);
        let elem = src.elem.clone();
        let place = Place::Memory { base: into, align };
        self.walk_rc(state, &elem, &place, 0, true, 0);
        self.close_loop(&l, None);

        // -- width 1, 2, 4, …, with the two buffers swapping each pass ------
        let Some(entry) = self.builder.get_insert_block() else { return };
        let wide = self.ctx.append_basic_block(state.value, "sort.wide");
        let pass = self.ctx.append_basic_block(state.value, "sort.pass");
        let sorted = self.ctx.append_basic_block(state.value, "sort.sorted");
        let _ = self.builder.build_unconditional_branch(wide);
        self.builder.position_at_end(wide);
        let (Ok(wp), Ok(ap), Ok(bp)) = (
            self.builder.build_phi(word, "sort.w"),
            self.builder.build_phi(self.ptr_ty(), "sort.a"),
            self.builder.build_phi(self.ptr_ty(), "sort.b"),
        ) else {
            return;
        };
        let width: IntValue<'ctx> = wp.as_basic_value().try_into().unwrap_or(one);
        let read_from: PointerValue<'ctx> = ap.as_basic_value().try_into().unwrap_or(dst);
        let write_to: PointerValue<'ctx> = bp.as_basic_value().try_into().unwrap_or(scratch);
        let unsorted = self
            .builder
            .build_int_compare(IntPredicate::ULT, width, src.len, "sort.unsorted")
            .unwrap_or_else(|_| self.ctx.bool_type().const_zero());
        let _ = self.builder.build_conditional_branch(unsorted, pass, sorted);

        self.builder.position_at_end(pass);
        let span = self
            .builder
            .build_int_mul(width, word.const_int(2, false), "sort.span")
            .unwrap_or(width);
        let swap = self.merge_pass(state, a, &Runs { width, span, read_from, write_to });
        self.builder.position_at_end(swap);
        let doubled = self
            .builder
            .build_int_mul(width, word.const_int(2, false), "sort.w2")
            .unwrap_or(width);
        let _ = self.builder.build_unconditional_branch(wide);
        wp.add_incoming(&[
            (&one as &dyn BasicValue<'ctx>, entry),
            (&doubled as &dyn BasicValue<'ctx>, swap),
        ]);
        ap.add_incoming(&[
            (&dst as &dyn BasicValue<'ctx>, entry),
            (&write_to as &dyn BasicValue<'ctx>, swap),
        ]);
        bp.add_incoming(&[
            (&scratch as &dyn BasicValue<'ctx>, entry),
            (&read_from as &dyn BasicValue<'ctx>, swap),
        ]);

        // -- an odd number of passes ends in the scratch ---------------------
        self.builder.position_at_end(sorted);
        let home = self.ctx.append_basic_block(state.value, "sort.home");
        let end = self.ctx.append_basic_block(state.value, "sort.end");
        let in_place = self
            .builder
            .build_int_compare(IntPredicate::EQ, read_from, dst, "sort.inplace")
            .unwrap_or_else(|_| self.ctx.bool_type().const_zero());
        let _ = self.builder.build_conditional_branch(in_place, end, home);
        self.builder.position_at_end(home);
        let _ = self.builder.build_memcpy(dst, align, read_from, align, bytes);
        let _ = self.builder.build_unconditional_branch(end);
        self.builder.position_at_end(end);
        let free = self.rt_free();
        if let Ok(call) = self.builder.build_call(free, &[scratch.into()], "") {
            attrs::set_call_convention(call, attrs::C);
        }
        let slots = repr::ir_slots(&mut self.reprs, self.program, code.ty_of(dest));
        let value =
            repr::assemble(self.ctx, &self.builder, &slots, &[dst.into(), src.len.into()]);
        self.set(state, dest, value);
    }

    /// One pass of [`Unit::list_sort`]: every pair of `width`-wide runs in `a`,
    /// merged into `b`. Answers the block the pass ends in.
    fn merge_pass(
        &mut self,
        state: &mut Function<'ctx>,
        a: &ListArgs<'ctx>,
        r: &Runs<'ctx>,
    ) -> BasicBlock<'ctx> {
        let (src, step) = (&a.src, &a.step);
        let word = self.ctx.i64_type();
        let align = src.align.max(1);
        let size = word.const_int(u64::from(src.size), false);
        let one = word.const_int(1, false);
        let zero = word.const_zero();
        let Some(opened) = self.builder.get_insert_block() else {
            return self.ctx.append_basic_block(state.value, "sort.swap");
        };
        let runs = self.ctx.append_basic_block(state.value, "sort.runs");
        let run = self.ctx.append_basic_block(state.value, "sort.run");
        let swap = self.ctx.append_basic_block(state.value, "sort.swap");
        let _ = self.builder.build_unconditional_branch(runs);

        self.builder.position_at_end(runs);
        let Ok(lop) = self.builder.build_phi(word, "sort.lo") else { return swap };
        let lo: IntValue<'ctx> = lop.as_basic_value().try_into().unwrap_or(zero);
        let left = self
            .builder
            .build_int_compare(IntPredicate::ULT, lo, src.len, "sort.left")
            .unwrap_or_else(|_| self.ctx.bool_type().const_zero());
        let _ = self.builder.build_conditional_branch(left, run, swap);

        self.builder.position_at_end(run);
        let mid =
            self.clamp(self.builder.build_int_add(lo, r.width, "sort.mid").unwrap_or(lo), src.len);
        let hi =
            self.clamp(self.builder.build_int_add(lo, r.span, "sort.hi").unwrap_or(lo), src.len);
        let merge = self.ctx.append_basic_block(state.value, "sort.merge");
        let body = self.ctx.append_basic_block(state.value, "sort.body");
        let merged = self.ctx.append_basic_block(state.value, "sort.merged");
        let _ = self.builder.build_unconditional_branch(merge);

        self.builder.position_at_end(merge);
        let (Ok(lip), Ok(rip), Ok(kp)) = (
            self.builder.build_phi(word, "sort.li"),
            self.builder.build_phi(word, "sort.ri"),
            self.builder.build_phi(word, "sort.k"),
        ) else {
            return swap;
        };
        let li: IntValue<'ctx> = lip.as_basic_value().try_into().unwrap_or(zero);
        let ri: IntValue<'ctx> = rip.as_basic_value().try_into().unwrap_or(zero);
        let k: IntValue<'ctx> = kp.as_basic_value().try_into().unwrap_or(zero);
        let filling = self
            .builder
            .build_int_compare(IntPredicate::ULT, k, hi, "sort.filling")
            .unwrap_or_else(|_| self.ctx.bool_type().const_zero());
        let _ = self.builder.build_conditional_branch(filling, body, merged);

        // -- which run the next element comes from ---------------------------
        self.builder.position_at_end(body);
        let both = self.ctx.append_basic_block(state.value, "sort.both");
        let compare = self.ctx.append_basic_block(state.value, "sort.compare");
        let take_left = self.ctx.append_basic_block(state.value, "sort.left");
        let take_right = self.ctx.append_basic_block(state.value, "sort.right");
        let open = self
            .builder
            .build_int_compare(IntPredicate::ULT, li, mid, "sort.lopen")
            .unwrap_or_else(|_| self.ctx.bool_type().const_zero());
        let _ = self.builder.build_conditional_branch(open, both, take_right);
        self.builder.position_at_end(both);
        let open = self
            .builder
            .build_int_compare(IntPredicate::ULT, ri, hi, "sort.ropen")
            .unwrap_or_else(|_| self.ctx.bool_type().const_zero());
        let _ = self.builder.build_conditional_branch(open, compare, take_left);

        self.builder.position_at_end(compare);
        let left_at = self.elem_at(r.read_from, li, src.stride, "sort.lat");
        let right_at = self.elem_at(r.read_from, ri, src.stride, "sort.rat");
        let mut params = Vec::new();
        let mut argv = Vec::new();
        self.pass_elem(state, src, left_at, &mut params, &mut argv);
        self.pass_elem(state, src, right_at, &mut params, &mut argv);
        match self.call_step(state, step, &params, &argv) {
            Some(BasicValueEnum::IntValue(order)) => {
                let after = self
                    .builder
                    .build_int_compare(
                        IntPredicate::EQ,
                        order,
                        order.get_type().const_int(GREATER, false),
                        "sort.after",
                    )
                    .unwrap_or_else(|_| self.ctx.bool_type().const_zero());
                let _ = self.builder.build_conditional_branch(after, take_right, take_left);
            }
            _ => {
                let _ = self.builder.build_unconditional_branch(take_left);
            }
        }

        // -- the two moves, which are byte copies and nothing else -----------
        let mut edges: Vec<(IntValue<'ctx>, IntValue<'ctx>, IntValue<'ctx>, BasicBlock<'ctx>)> =
            Vec::new();
        for (block, right) in [(take_left, false), (take_right, true)] {
            self.builder.position_at_end(block);
            let at = if right { ri } else { li };
            let from = self.elem_at(r.read_from, at, src.stride, "sort.take");
            let into = self.elem_at(r.write_to, k, src.stride, "sort.put");
            let _ = self.builder.build_memcpy(into, align, from, align, size);
            let taken = self.builder.build_int_add(at, one, "sort.taken").unwrap_or(at);
            let filled = self.builder.build_int_add(k, one, "sort.filled").unwrap_or(k);
            let _ = self.builder.build_unconditional_branch(merge);
            let (nl, nr) = if right { (li, taken) } else { (taken, ri) };
            edges.push((nl, nr, filled, block));
        }
        for (value, phi) in [(lo, lip), (mid, rip), (lo, kp)] {
            phi.add_incoming(&[(&value as &dyn BasicValue<'ctx>, run)]);
        }
        for (nl, nr, filled, block) in &edges {
            lip.add_incoming(&[(nl as &dyn BasicValue<'ctx>, *block)]);
            rip.add_incoming(&[(nr as &dyn BasicValue<'ctx>, *block)]);
            kp.add_incoming(&[(filled as &dyn BasicValue<'ctx>, *block)]);
        }

        self.builder.position_at_end(merged);
        let next = self.builder.build_int_add(lo, r.span, "sort.nextlo").unwrap_or(lo);
        let _ = self.builder.build_unconditional_branch(runs);
        lop.add_incoming(&[
            (&zero as &dyn BasicValue<'ctx>, opened),
            (&next as &dyn BasicValue<'ctx>, merged),
        ]);
        swap
    }

    /// `min(v, n)`, as a `select`: a run's end is its start plus the width, or
    /// the block's length where the last run is short.
    fn clamp(&self, v: IntValue<'ctx>, n: IntValue<'ctx>) -> IntValue<'ctx> {
        let Ok(over) = self.builder.build_int_compare(IntPredicate::ULT, v, n, "sort.fits") else {
            return v;
        };
        match self.builder.build_select(over, v, n, "sort.end") {
            Ok(BasicValueEnum::IntValue(x)) => x,
            _ => v,
        }
    }

    /// `fold` and `foldCtx`: the accumulator, threaded.
    ///
    /// One path and not two: an accumulator of any width is a phi, because
    /// LLVM's phi takes a first-class aggregate. The counts balance by the
    /// retain below — the initial accumulator arrives borrowed and the first
    /// step owns it — after which each step consumes the count it is given and
    /// answers another, and the last one is the result's.
    fn list_fold(
        &mut self,
        state: &mut Function<'ctx>,
        code: &ir::Code,
        dest: ir::ValueId,
        a: &ListArgs<'ctx>,
    ) {
        let (src, step, ctx) = (&a.src, &a.step, a.ctx);
        let Some(init) = a.init else { return };
        let ir_dest = code.ty_of(dest);
        let slots = repr::ir_slots(&mut self.reprs, self.program, ir_dest);
        let start = self.get(state, init);
        if let Some(ty) = self.type_of(ir_dest) {
            if self.rc_counted(&ty) {
                let pieces = repr::disassemble(&self.builder, &slots, start);
                let place = Place::Registers { slots: slots.clone(), pieces };
                self.walk_rc(state, &ty, &place, 0, true, 0);
            }
        }
        let Some(l) = self.open_loop(state, src.len, Some(start)) else { return };
        let acc = l.carried.map_or(start, |(p, _)| p.as_basic_value());

        let at = self.elem_at(src.base, l.i, src.stride, "fold.at");
        let mut params = Vec::new();
        let mut argv = Vec::new();
        self.pass_ctx(state, code, ctx, &mut params, &mut argv);
        self.pass_value(acc, &slots, &mut params, &mut argv);
        self.pass_elem(state, src, at, &mut params, &mut argv);
        let stepped = self.call_step(state, step, &params, &argv).unwrap_or(acc);
        self.close_loop(&l, Some(stepped));

        // The header's phi, which dominates `done`: the accumulator as it stood
        // when the bound test last failed.
        self.set(state, dest, acc);
    }

    /// `any`, `all` and `count`.
    ///
    /// `any` and `all` leave the loop at the first element that decides the
    /// answer, which is what their `core/list` declarations promise by taking
    /// no context: a step that cannot have an effect cannot notice.
    fn list_test(
        &mut self,
        state: &mut Function<'ctx>,
        code: &ir::Code,
        dest: ir::ValueId,
        a: &ListArgs<'ctx>,
        kind: Step,
    ) {
        let (src, step) = (&a.src, &a.step);
        let want = repr::ir_type(self.ctx, &mut self.reprs, self.program, code.ty_of(dest));
        let BasicTypeEnum::IntType(int) = want else { return };
        if kind == Step::Count {
            let Some(l) = self.open_loop(state, src.len, Some(int.const_zero().into())) else {
                return;
            };
            let total: IntValue<'ctx> = match l.carried.map(|(p, _)| p.as_basic_value()) {
                Some(BasicValueEnum::IntValue(v)) => v,
                _ => int.const_zero(),
            };
            let at = self.elem_at(src.base, l.i, src.stride, "count.at");
            let mut params = Vec::new();
            let mut argv = Vec::new();
            self.pass_elem(state, src, at, &mut params, &mut argv);
            let next = match self.call_step(state, step, &params, &argv) {
                Some(BasicValueEnum::IntValue(b)) => {
                    let one = self
                        .builder
                        .build_int_z_extend_or_bit_cast(b, int, "count.one")
                        .unwrap_or_else(|_| int.const_zero());
                    self.builder.build_int_add(total, one, "count.n").unwrap_or(total)
                }
                _ => total,
            };
            self.close_loop(&l, Some(next.into()));
            self.set(state, dest, total.into());
            return;
        }

        let Some(l) = self.open_loop(state, src.len, None) else { return };
        let at = self.elem_at(src.base, l.i, src.stride, "test.at");
        let mut params = Vec::new();
        let mut argv = Vec::new();
        self.pass_elem(state, src, at, &mut params, &mut argv);
        let answer = self.call_step(state, step, &params, &argv);
        let early = match answer {
            Some(BasicValueEnum::IntValue(b)) => {
                let cont = self.ctx.append_basic_block(state.value, "test.cont");
                let from = self.builder.get_insert_block().unwrap_or(cont);
                let _ = match kind {
                    Step::Any => self.builder.build_conditional_branch(b, l.done, cont),
                    _ => self.builder.build_conditional_branch(b, cont, l.done),
                };
                self.builder.position_at_end(cont);
                Some(from)
            }
            _ => None,
        };
        self.close_loop(&l, None);

        // `any` falls out of the loop having found nothing and leaves it having
        // found something; `all` is the same statement with the constants
        // swapped. The phi is the whole of the answer, so neither carries a
        // value across the iterations.
        let exhausted = int.const_int(u64::from(kind == Step::All), false);
        let decided = int.const_int(u64::from(kind == Step::Any), false);
        // The phi is built only where the early exit was, because `done` then
        // has one predecessor and a phi with one incoming edge naming two would
        // be the verifier error this whole comment block exists to avoid.
        let value = match early {
            Some(from) => match self.builder.build_phi(int, "test.answer") {
                Ok(phi) => {
                    phi.add_incoming(&[
                        (&exhausted as &dyn BasicValue<'ctx>, l.header),
                        (&decided as &dyn BasicValue<'ctx>, from),
                    ]);
                    phi.as_basic_value()
                }
                Err(_) => exhausted.into(),
            },
            None => exhausted.into(),
        };
        self.set(state, dest, value);
    }
}

// ---------------------------------------------------------------------------
// `num.<T>.<op>` — the numeric surface `core/num` declares without a body
// ---------------------------------------------------------------------------

impl<'ctx, 'a> Unit<'ctx, 'a> {
    /// Emitted inline rather than called, for the same reason the JavaScript
    /// backend emits it inline (`js/intrinsics.rs`): there is one conversion
    /// per source-and-target pair (SPEC 6.2.1) and generating two instructions
    /// beats calling a runtime function that generates the same two.
    ///
    /// Answers `false` for an operation it does not implement — `checked*`,
    /// `saturating*`, `compare`, `minValue`, `maxValue` — so that the caller
    /// reports a missing intrinsic instead of emitting a call to a symbol that
    /// does not exist.
    fn numeric(
        &mut self,
        state: &mut Function<'ctx>,
        code: &ir::Code,
        dests: &[ir::ValueId],
        key: &str,
        args: &[ir::ValueId],
        span: Span,
    ) -> bool {
        let parts: Vec<&str> = key.split('.').collect();
        let (Some(&"num"), Some(name), Some(op), 3) =
            (parts.first(), parts.get(1), parts.get(2), parts.len())
        else {
            return false;
        };
        let Some(from) = Prim::all().iter().copied().find(|p| p.name() == *name) else {
            return false;
        };
        let Some(dest) = dests.first().copied() else { return false };
        let want = repr::ir_type(self.ctx, &mut self.reprs, self.program, code.ty_of(dest));
        let a = args.first().copied().map(|v| self.get(state, v));
        let b = args.get(1).copied().map(|v| self.get(state, v));

        // A conversion is named by its target: `toI32`, `wrapToU8`. Both
        // spellings are the same instruction sequence — widen, narrow, or
        // cross between integer and float — chosen from the two types.
        //
        // Only the ones whose result is the target type itself. SPEC 6.2.1
        // gives three shapes and the third is `Result<T, RangeError>`: a
        // narrowing `to` answers one, and so does `U32.toChar`, because not
        // every `U32` is a Unicode scalar. Building that `Result` means
        // building a `RangeError`, whose fields are two `Str`s naming the
        // value and the target — a rendering, not a conversion — so those keys
        // are left to the table and to `missing_intrinsics`.
        if let Some((to, exact)) = conversion_target(from, op) {
            let Some(v) = a else { return false };
            if !exact {
                return false;
            }
            let out = self.cast(v, from, to, want);
            self.set(state, dest, out);
            return true;
        }

        // `Bounded`'s two, which take no `self` and whose type is in the key
        // rather than in an argument — `middle::lower`'s `bounded_key` puts it
        // there for the reason `qualified_key` puts a `derivePrim*`'s there:
        // the *return* type is a bare register shape, and `I64` and `U64` are
        // the same one.
        if matches!(*op, "minValue" | "maxValue") {
            let Some(value) = self.bounds(from, *op == "minValue", want) else { return false };
            self.set(state, dest, value);
            return true;
        }

        let float = from.is_float();
        let signed = from.is_signed();
        let value = match (*op, a, b) {
            // `wrappingAdd` and `add` are the same instruction here: §3.4
            // declines to set `nsw`/`nuw`, so the plain operation already wraps
            // and the `Wrapping` trait's promise is the one this backend keeps
            // by default. The keys stay separate because the *checker* uses
            // them to say whether a program meant it.
            (
                "add" | "sub" | "mul" | "div" | "rem" | "wrappingAdd" | "wrappingSub"
                | "wrappingMul",
                Some(x),
                Some(y),
            ) => {
                let binop = match *op {
                    "add" | "wrappingAdd" => ir::BinOp::Add,
                    "sub" | "wrappingSub" => ir::BinOp::Sub,
                    "mul" | "wrappingMul" => ir::BinOp::Mul,
                    "div" => ir::BinOp::Div,
                    _ => ir::BinOp::Rem,
                };
                // Straight through `binary`, so that SPEC 6.2's division abort
                // and the 128-bit `buri_rt_i128_divmod` call are emitted from
                // one place rather than from two that have to agree.
                self.binary(state, binop, from, code.ty_of(dest), x, y)
            }
            ("neg", Some(x), _) => self.unary(ir::UnOp::Neg, from, x),
            ("abs", Some(BasicValueEnum::FloatValue(x)), _) => self.fabs(x),
            // `abs` of a signed minimum overflows, and overflow is undefined
            // (SPEC 6.2), so there is nothing to check.
            ("abs", Some(BasicValueEnum::IntValue(x)), _) if signed => {
                let zero = x.get_type().const_zero();
                let flipped = self.builder.build_int_neg(x, "abs.neg").unwrap_or(x);
                let below = self
                    .builder
                    .build_int_compare(IntPredicate::SLT, x, zero, "abs.lt")
                    .unwrap_or_else(|_| self.ctx.bool_type().const_zero());
                self.builder.build_select(below, flipped, x, "abs").unwrap_or_else(|_| x.into())
            }
            // An unsigned value is its own magnitude.
            ("abs", Some(x), _) => x,
            ("signum", Some(x), _) => self.signum(x, float, signed),
            // The three structural leaves declared on every primitive
            // (`semantics/builtins.rs`'s `numeric_methods`). Unlike
            // `derivePrimShow` and `derivePrimHash`, the *key* says which
            // primitive this is, so there is no erasure to work around and
            // every arm is reachable. `hash` is the fourth and is not here:
            // `$hash` starts from the FNV-1a seed, which is a Rust `const` in
            // `cli/runtime/hash.rs` rather than an exported symbol, and
            // copying `0x811c9dc5` into a backend is the one thing
            // VALUE-MODEL.md §12 most wants stated once.
            ("eq", Some(x), Some(y)) => {
                let operand = args.first().copied().map_or(ir::Type::I64, |v| code.ty_of(v));
                self.binary(state, ir::BinOp::Eq, from, operand, x, y)
            }
            ("compare", Some(x), Some(y)) => {
                let BasicTypeEnum::IntType(tag) = want else { return false };
                self.order(x, y, float, signed, tag)
            }
            // `x.show(ctx)` is `$str(x)` and **not** `$show(x)`
            // (`js/intrinsics.rs:58`): a `Str` renders as itself and a `Char`
            // as its own character, unquoted. The quoting is `derivePrimShow`'s
            // and belongs to `derive Show`, not to the method.
            ("show", _, _) => {
                let Some(arg) = args.first().copied() else { return false };
                return self.show_prim(state, code, dest, from, arg, false);
            }
            // `Checked` and `Saturating`, which are integer traits: exact
            // arithmetic in 128 bits, then a range test or a clamp.
            (
                "checkedAdd" | "checkedSub" | "checkedMul" | "checkedDiv",
                Some(BasicValueEnum::IntValue(x)),
                Some(BasicValueEnum::IntValue(y)),
            ) => {
                let task = Wide { op, prim: from, x, y };
                return self.checked(state, code, dest, &task, span);
            }
            (
                "saturatingAdd" | "saturatingSub" | "saturatingMul",
                Some(BasicValueEnum::IntValue(x)),
                Some(BasicValueEnum::IntValue(y)),
            ) => {
                let task = Wide { op, prim: from, x, y };
                return self.saturating(state, code, dest, &task);
            }
            // `x.hash()` is `$hashInto(SEED, x)`: the same body a derived
            // `Hash` reaches, started rather than continued.
            ("hash", _, _) => {
                let Some(arg) = args.first().copied() else { return false };
                let seed = self.ctx.i64_type().const_int(runtime::HASH_SEED, false);
                return self.hash_prim(state, code, dest, from, seed.into(), arg);
            }
            _ => return false,
        };
        self.set(state, dest, value);
        true
    }

    /// `checkedAdd`, `checkedSub`, `checkedMul`, `checkedDiv` — `Option<T>`.
    ///
    /// **The bound is the type's own range**, which is where this parts company
    /// with the JavaScript backend: there `js/intrinsics.rs` tests
    /// `exact_int_range`, because past `2^53` a double cannot say which integer
    /// it is and `.None` is the only honest answer it has. `Checked` is bounded
    /// by the numbers the *platform* has, and `.Some(v)` promises that `v` is
    /// the true result as this backend represents numbers — a promise both
    /// backends keep, at different widths (SPEC 6.2.2,
    /// `design/native/VALUE-MODEL.md` §12 row 2). The band between `2^53` and
    /// the type's own maximum is a documented divergence and
    /// `cli/tests/native/agreement.rs`'s row 2 pins both answers.
    ///
    /// The arithmetic is done in 128 bits rather than with
    /// `llvm.*.with.overflow`, which is both simpler and *more* correct here:
    /// a 64-bit sum, difference or product is exact in 128 bits, so the range
    /// test over the widened value answers "did it overflow the type" directly,
    /// with no per-operation overflow flag to interpret. `i64::MIN / -1` is
    /// `2^63`, which the widening holds and the range test then rejects — which
    /// is `.None`, and is two's-complement overflow rather than a special case.
    ///
    /// The division is guarded rather than branched around: an `sdiv` by zero is
    /// immediate undefined behaviour in LLVM even on a path whose result is
    /// discarded, so the divisor is *replaced* by one and the instruction never
    /// sees it.
    fn checked(
        &mut self,
        state: &mut Function<'ctx>,
        code: &ir::Code,
        dest: ir::ValueId,
        task: &Wide<'_, 'ctx>,
        span: Span,
    ) -> bool {
        let Wide { op, prim, x, y } = *task;
        if prim.bits() == 128 {
            return self.checked_128(state, code, dest, task, span);
        }
        let Some((lo, hi)) = prim.int_range() else { return false };
        let wide = self.ctx.i128_type();
        let signed = prim.is_signed();
        let a = self.widen(x, wide, signed);
        let b = self.widen(y, wide, signed);
        let bool_ty = self.ctx.bool_type();
        let mut ok = bool_ty.const_int(1, false);
        let value = match op {
            "checkedAdd" => self.builder.build_int_add(a, b, "ck.add").unwrap_or(a),
            "checkedSub" => self.builder.build_int_sub(a, b, "ck.sub").unwrap_or(a),
            "checkedMul" => self.builder.build_int_mul(a, b, "ck.mul").unwrap_or(a),
            _ => {
                // A checked division by zero is `.None`, not SPEC 6.2's abort.
                // The divisor is *replaced* by one where it is zero rather than
                // branched around: an `sdiv` by zero is immediate undefined
                // behaviour in LLVM even on a path whose result is discarded,
                // so the instruction must never see it.
                let nonzero = self
                    .builder
                    .build_int_compare(IntPredicate::NE, b, wide.const_zero(), "ck.nz")
                    .unwrap_or_else(|_| bool_ty.const_zero());
                ok = nonzero;
                let safe: IntValue<'ctx> = self
                    .builder
                    .build_select(nonzero, b, wide.const_int(1, false), "ck.safe")
                    .ok()
                    .and_then(|v| v.try_into().ok())
                    .unwrap_or(b);
                if signed {
                    self.builder.build_int_signed_div(a, safe, "ck.div").unwrap_or(a)
                } else {
                    self.builder.build_int_unsigned_div(a, safe, "ck.div").unwrap_or(a)
                }
            }
        };
        let low = self.int_constant(wide.as_basic_type_enum(), lo.unsigned_abs(), lo < 0);
        let high = self.int_constant(wide.as_basic_type_enum(), hi, false);
        // Signed comparisons at 128 bits for a signed *and* an unsigned type:
        // every bound of every type of 64 bits or fewer is exactly an `i128`.
        for (predicate, bound) in
            [(IntPredicate::SGE, low), (IntPredicate::SLE, high)]
        {
            let BasicValueEnum::IntValue(bound) = bound else { continue };
            let inside = self
                .builder
                .build_int_compare(predicate, value, bound, "ck.in")
                .unwrap_or_else(|_| bool_ty.const_zero());
            ok = self.builder.build_and(ok, inside, "ck.ok").unwrap_or(ok);
        }
        let narrow = self
            .builder
            .build_int_truncate_or_bit_cast(value, x.get_type(), "ck.v")
            .unwrap_or(x);
        self.option_value(code, dest, ok, narrow).is_some_and(|v| {
            self.set(state, dest, v);
            true
        })
    }

    fn checked_128(
        &mut self,
        state: &mut Function<'ctx>,
        code: &ir::Code,
        dest: ir::ValueId,
        task: &Wide<'_, 'ctx>,
        span: Span,
    ) -> bool {
        let argv = self.wide_op_args(task);
        self.call_sum(state, code, dest, &runtime::I128_CHECKED_ENTRY, argv, span);
        true
    }

    /// `saturatingAdd`, `saturatingSub`, `saturatingMul` — clamped to the
    /// **type's own** bounds, which is `$sat`'s rule on the other backend too:
    /// `Saturating` promises a value in range and says nothing about whether a
    /// double could name it, so it is the one family of the three that never
    /// had a second bound to lose.
    fn saturating(
        &mut self,
        state: &mut Function<'ctx>,
        code: &ir::Code,
        dest: ir::ValueId,
        task: &Wide<'_, 'ctx>,
    ) -> bool {
        let Wide { op, prim, x, y } = *task;
        if prim.bits() == 128 {
            let mut argv = self.wide_op_args(task);
            self.call_out(state, code, dest, runtime::I128_SATURATING, &mut argv);
            return true;
        }
        let Some((lo, hi)) = prim.int_range() else { return false };
        let wide = self.ctx.i128_type();
        let signed = prim.is_signed();
        let a = self.widen(x, wide, signed);
        let b = self.widen(y, wide, signed);
        let value = match op {
            "saturatingAdd" => self.builder.build_int_add(a, b, "sat.add").unwrap_or(a),
            "saturatingSub" => self.builder.build_int_sub(a, b, "sat.sub").unwrap_or(a),
            _ => self.builder.build_int_mul(a, b, "sat.mul").unwrap_or(a),
        };
        let low = self.int_constant(wide.as_basic_type_enum(), lo.unsigned_abs(), lo < 0);
        let high = self.int_constant(wide.as_basic_type_enum(), hi, false);
        let mut clamped = value;
        for (predicate, bound) in [(IntPredicate::SLT, low), (IntPredicate::SGT, high)] {
            let BasicValueEnum::IntValue(bound) = bound else { continue };
            let outside = self
                .builder
                .build_int_compare(predicate, clamped, bound, "sat.out")
                .unwrap_or_else(|_| self.ctx.bool_type().const_zero());
            clamped = self
                .builder
                .build_select(outside, bound, clamped, "sat.c")
                .ok()
                .and_then(|v| v.try_into().ok())
                .unwrap_or(clamped);
        }
        let narrow = self
            .builder
            .build_int_truncate_or_bit_cast(clamped, x.get_type(), "sat.v")
            .unwrap_or(x);
        self.set(state, dest, narrow.into());
        true
    }

    /// `(op, a_lo, a_hi, b_lo, b_hi, signed)` — the argument list both 128-bit
    /// entries take, with the operation as an immediate rather than as four
    /// symbols. `0` add, `1` sub, `2` mul, `3` div.
    fn wide_op_args(&mut self, task: &Wide<'_, 'ctx>) -> Vec<BasicMetadataValueEnum<'ctx>> {
        let Wide { op, prim, x, y } = *task;
        let byte = self.ctx.i8_type();
        let code = match op {
            "checkedAdd" | "saturatingAdd" => 0,
            "checkedSub" | "saturatingSub" => 1,
            "checkedMul" | "saturatingMul" => 2,
            _ => 3,
        };
        let (a_lo, a_hi) = self.halves(x);
        let (b_lo, b_hi) = self.halves(y);
        vec![
            byte.const_int(code, false).into(),
            a_lo.into(),
            a_hi.into(),
            b_lo.into(),
            b_hi.into(),
            byte.const_int(u64::from(prim.is_signed()), false).into(),
        ]
    }

    /// One integer at a wider one, by the source's signedness.
    fn widen(&self, v: IntValue<'ctx>, want: IntType<'ctx>, signed: bool) -> IntValue<'ctx> {
        if signed {
            self.builder.build_int_s_extend_or_bit_cast(v, want, "w")
        } else {
            self.builder.build_int_z_extend_or_bit_cast(v, want, "w")
        }
        .unwrap_or(v)
    }

    /// `ok ? .Some(payload) : .None`, at whatever `middle::layout` chose for
    /// the destination `Option`.
    ///
    /// A `select` between two register values rather than a branch and a phi:
    /// both arms are constants-and-shifts over values that already dominate, so
    /// there is nothing to make cold and nothing to skip.
    fn option_value(
        &mut self,
        code: &ir::Code,
        dest: ir::ValueId,
        ok: IntValue<'ctx>,
        payload: IntValue<'ctx>,
    ) -> Option<BasicValueEnum<'ctx>> {
        let ir::Type::Agg(id) = code.ty_of(dest) else { return None };
        let (slots, enum_repr, offsets, size) = {
            let r = self.reprs.of(self.program, id);
            let (none_at, some_at) = match &r.layout.repr {
                LayoutRepr::Enum { variants, .. } => (
                    variants.iter().position(Vec::is_empty).unwrap_or(1),
                    variants.iter().position(|v| !v.is_empty()).unwrap_or(0),
                ),
                _ => (1, 0),
            };
            (
                r.slots.clone(),
                r.enum_repr().cloned()?,
                (some_at, none_at, r.layout.variant(some_at).first().copied().unwrap_or(0)),
                r.layout.size,
            )
        };
        let (some_at, none_at, payload_at) = offsets;
        let tag_const = |ctx: &'ctx Context, tag: Scalar, v: usize| {
            match repr::slot_type(ctx, SlotTy::Scalar(tag)) {
                BasicTypeEnum::IntType(t) => t.const_int(v as u64, false).as_basic_value_enum(),
                other => other.const_zero(),
            }
        };
        let (some, none) = match enum_repr {
            EnumRepr::Bare { tag } => {
                (tag_const(self.ctx, tag, some_at), tag_const(self.ctx, tag, none_at))
            }
            // No `Option` of an integer takes the niche — it needs a pointer to
            // spend — but the arm is written rather than refused, because which
            // encoding a type got is `middle::layout`'s answer and not this
            // function's assumption.
            EnumRepr::Niche { .. } => (
                payload.as_basic_value_enum(),
                repr::register_type(self.ctx, &slots).const_zero(),
            ),
            EnumRepr::Tagged { tag, payload: payload_start } => {
                let bytes = size.saturating_sub(payload_start);
                let blob_ty = repr::blob_type(self.ctx, bytes);
                let width = payload.get_type().get_bit_width().saturating_div(8);
                let bits = repr::slot_to_bits(
                    self.ctx,
                    &self.builder,
                    Slot { offset: 0, ty: SlotTy::Blob(width) },
                    payload.as_basic_value_enum(),
                );
                let widened = self
                    .builder
                    .build_int_z_extend_or_bit_cast(bits, blob_ty, "opt.w")
                    .unwrap_or_else(|_| blob_ty.const_zero());
                let shift =
                    u64::from(payload_at.saturating_sub(payload_start)).saturating_mul(8);
                let placed = self
                    .builder
                    .build_left_shift(widened, blob_ty.const_int(shift, false), "opt.sh")
                    .unwrap_or(widened);
                let some = [tag_const(self.ctx, tag, some_at), placed.into()];
                let none = [tag_const(self.ctx, tag, none_at), blob_ty.const_zero().into()];
                (
                    repr::assemble(self.ctx, &self.builder, &slots, &some),
                    repr::assemble(self.ctx, &self.builder, &slots, &none),
                )
            }
        };
        self.builder.build_select(ok, some, none, "opt").ok()
    }

    /// `Bounded::minValue` and `Bounded::maxValue`, as constants.
    ///
    /// The bounds are the **type's**, not JavaScript's exactly-representable
    /// ones: `js/intrinsics.rs` uses `int_range` here too. `exact_int_range` is
    /// the JavaScript backend's business alone — it is the range a double still
    /// names, and natively nothing has to survive being one.
    ///
    /// A float's bounds are the largest finite magnitude at both signs, which
    /// is what `Bounded` is about. Not `MIN_POSITIVE`, which is the smallest
    /// *positive* one and would make `minValue<F64>()` a number above zero.
    fn bounds(&self, prim: Prim, low: bool, want: BasicTypeEnum<'ctx>) -> Option<BasicValueEnum<'ctx>> {
        if prim.is_float() {
            let BasicTypeEnum::FloatType(t) = want else { return None };
            let v = match (prim, low) {
                (Prim::F32, true) => f64::from(-f32::MAX),
                (Prim::F32, false) => f64::from(f32::MAX),
                (_, true) => f64::MIN,
                (_, false) => f64::MAX,
            };
            return Some(t.const_float(v).into());
        }
        let (lo, hi) = prim.int_range()?;
        if low {
            Some(self.int_constant(want, lo.unsigned_abs(), lo < 0))
        } else {
            Some(self.int_constant(want, hi, false))
        }
    }

    /// `|x|` for a float, as a mask rather than a comparison.
    ///
    /// Clearing the sign bit is what `Math.abs` is: it answers `+0` for `-0`
    /// and `NaN` for `NaN`, both of which a `x < 0 ? -x : x` gets wrong — the
    /// first because `-0 < 0` is false, and the second because every comparison
    /// with `NaN` is false and the sign would survive.
    fn fabs(&self, x: FloatValue<'ctx>) -> BasicValueEnum<'ctx> {
        let bits = if x.get_type() == self.ctx.f32_type() { 32 } else { 64 };
        let int = self.int_of_width(bits);
        let Ok(as_int) = self.builder.build_bit_cast(x, int, "abs.i") else { return x.into() };
        let BasicValueEnum::IntValue(as_int) = as_int else { return x.into() };
        // `0x7fff...`: every bit but the sign.
        let mask = int.const_all_ones();
        let mask = self
            .builder
            .build_right_shift(mask, int.const_int(1, false), false, "abs.m")
            .unwrap_or(mask);
        let cleared = self.builder.build_and(as_int, mask, "abs.b").unwrap_or(as_int);
        self.builder
            .build_bit_cast(cleared, x.get_type(), "abs.f")
            .unwrap_or_else(|_| x.into())
    }

    /// `(x > y, x < y)`, at whichever of the two comparison instructions the
    /// operand shape asks for.
    ///
    /// The ordered float predicates (`OGT`, `OLT`) rather than the unordered
    /// ones, so that every comparison with a `NaN` is false — which is what
    /// makes `signum(NaN)` fall through to zero and `compare(NaN, x)` to
    /// `Equal`, both of which are `js/intrinsics.rs`'s answers.
    fn cmp_pair(
        &self,
        x: BasicValueEnum<'ctx>,
        y: BasicValueEnum<'ctx>,
        float: bool,
        signed: bool,
    ) -> Option<(IntValue<'ctx>, IntValue<'ctx>)> {
        let (above, below) = match (x, y) {
            (BasicValueEnum::FloatValue(l), BasicValueEnum::FloatValue(r)) if float => (
                self.builder.build_float_compare(FloatPredicate::OGT, l, r, "gt"),
                self.builder.build_float_compare(FloatPredicate::OLT, l, r, "lt"),
            ),
            (BasicValueEnum::IntValue(l), BasicValueEnum::IntValue(r)) => {
                let (gt, lt) = if signed {
                    (IntPredicate::SGT, IntPredicate::SLT)
                } else {
                    (IntPredicate::UGT, IntPredicate::ULT)
                };
                (
                    self.builder.build_int_compare(gt, l, r, "gt"),
                    self.builder.build_int_compare(lt, l, r, "lt"),
                )
            }
            _ => return None,
        };
        match (above, below) {
            (Ok(above), Ok(below)) => Some((above, below)),
            _ => None,
        }
    }

    /// `x < 0 ? -1 : (x > 0 ? 1 : 0)`, which is `js/intrinsics.rs`'s spelling
    /// and therefore also `NaN`'s answer: both comparisons are false, so a
    /// `NaN` signs as `0` rather than as itself.
    fn signum(&self, x: BasicValueEnum<'ctx>, float: bool, signed: bool) -> BasicValueEnum<'ctx> {
        let origin = match x {
            BasicValueEnum::FloatValue(v) => v.get_type().const_zero().as_basic_value_enum(),
            BasicValueEnum::IntValue(v) => v.get_type().const_zero().as_basic_value_enum(),
            other => return other,
        };
        let Some((above, below)) = self.cmp_pair(x, origin, float, signed) else { return x };
        let (zero, one, minus) = match x {
            BasicValueEnum::FloatValue(v) => {
                let t = v.get_type();
                (
                    t.const_zero().as_basic_value_enum(),
                    t.const_float(1.0).as_basic_value_enum(),
                    t.const_float(-1.0).as_basic_value_enum(),
                )
            }
            BasicValueEnum::IntValue(v) => {
                let t = v.get_type();
                (
                    t.const_zero().as_basic_value_enum(),
                    t.const_int(1, false).as_basic_value_enum(),
                    t.const_all_ones().as_basic_value_enum(),
                )
            }
            other => return other,
        };
        let lower = self.builder.build_select(below, minus, zero, "sgn.lo").unwrap_or(zero);
        self.builder.build_select(above, one, lower, "sgn").unwrap_or(lower)
    }

    /// `compare(x, y)` as an `Order`.
    ///
    /// `core/order` declares `{ Less, Equal, Greater }` with no payload, so
    /// `middle::layout` gives it `EnumRepr::Bare` and the value *is* the tag —
    /// `0`, `1`, `2` in declaration order, which is the numbering
    /// `buri_rt_str_compare` answers with too. Two `select`s and no branch.
    fn order(
        &self,
        x: BasicValueEnum<'ctx>,
        y: BasicValueEnum<'ctx>,
        float: bool,
        signed: bool,
        tag: IntType<'ctx>,
    ) -> BasicValueEnum<'ctx> {
        let Some((above, below)) = self.cmp_pair(x, y, float, signed) else { return x };
        let less = tag.const_zero().as_basic_value_enum();
        let equal = tag.const_int(1, false).as_basic_value_enum();
        let greater = tag.const_int(2, false).as_basic_value_enum();
        let lower = self.builder.build_select(below, less, equal, "ord.lo").unwrap_or(equal);
        self.builder.build_select(above, greater, lower, "ord").unwrap_or(lower)
    }

    /// One numeric value at one register shape, at another.
    ///
    /// Widening takes its signedness from the **source**, which is the whole of
    /// the integer rule: `U8` to `I64` is a zero extension and `I8` to `I64` is
    /// a sign extension, and getting that backwards is the classic conversion
    /// bug. A float-to-integer conversion takes its signedness from the
    /// **target** instead, because that is what decides which end of the range
    /// it saturates against — and it saturates rather than trapping, because a
    /// trap here would be a run-time failure the language does not have.
    fn cast(
        &mut self,
        v: BasicValueEnum<'ctx>,
        from: Prim,
        to: Prim,
        want: BasicTypeEnum<'ctx>,
    ) -> BasicValueEnum<'ctx> {
        match (v, want) {
            (BasicValueEnum::FloatValue(x), BasicTypeEnum::FloatType(t)) => {
                if x.get_type() == t {
                    return v;
                }
                if to.bits() > from.bits() {
                    self.builder.build_float_ext(x, t, "fpext").map(Into::into).unwrap_or(v)
                } else {
                    self.builder.build_float_trunc(x, t, "fptrunc").map(Into::into).unwrap_or(v)
                }
            }
            (BasicValueEnum::FloatValue(x), BasicTypeEnum::IntType(t)) => {
                self.float_to_int(x, t, to.is_signed())
            }
            (BasicValueEnum::IntValue(x), BasicTypeEnum::FloatType(t)) => {
                if from.is_signed() {
                    self.builder.build_signed_int_to_float(x, t, "sitofp").map(Into::into)
                } else {
                    self.builder.build_unsigned_int_to_float(x, t, "uitofp").map(Into::into)
                }
                .unwrap_or(v)
            }
            (BasicValueEnum::IntValue(x), BasicTypeEnum::IntType(t)) => {
                let have = x.get_type().get_bit_width();
                if have == t.get_bit_width() {
                    return v;
                }
                if t.get_bit_width() > have {
                    if from.is_signed() {
                        self.builder.build_int_s_extend(x, t, "sext").map(Into::into)
                    } else {
                        self.builder.build_int_z_extend(x, t, "zext").map(Into::into)
                    }
                } else {
                    self.builder.build_int_truncate(x, t, "trunc").map(Into::into)
                }
                .unwrap_or(v)
            }
            _ => v,
        }
    }

    /// A float to an integer, **saturating**.
    ///
    /// `llvm.fptosi.sat` / `llvm.fptoui.sat` rather than a plain `fptosi`: a
    /// plain one is `poison` outside the target's range, and `poison` reaching
    /// a value a program prints is undefined behaviour where SPEC has a defined
    /// answer. The saturating form clamps to the endpoints and answers `0` for
    /// `NaN`, which is the same clamp the debug backend performs — so the two
    /// backends agree without either of them writing the clamp out.
    fn float_to_int(
        &mut self,
        x: FloatValue<'ctx>,
        want: IntType<'ctx>,
        signed: bool,
    ) -> BasicValueEnum<'ctx> {
        let name = if signed { "llvm.fptosi.sat" } else { "llvm.fptoui.sat" };
        let zero = want.const_zero().as_basic_value_enum();
        let Some(intrinsic) = inkwell::intrinsics::Intrinsic::find(name) else { return zero };
        let overloads = [want.as_basic_type_enum(), x.get_type().as_basic_type_enum()];
        let Some(f) = intrinsic.get_declaration(&self.module, &overloads) else { return zero };
        match self.builder.build_call(f, &[x.into()], "sat") {
            Ok(call) => call.try_as_basic_value().basic().unwrap_or(zero),
            Err(_) => zero,
        }
    }
}

// ---------------------------------------------------------------------------
// What this backend claims, asked before emission
// ---------------------------------------------------------------------------

/// Whether this backend has *something* for a key — a table entry, an inline
/// sequence, or a generated body.
///
/// [`super::Llvm::missing_intrinsics`] is asked this before a second is spent
/// in LLVM, which is the whole reason that hook is on the trait
/// (`backend/mod.rs`). It is therefore a claim about the **key** and not about
/// one call site: `derivePrimShow` is claimed here and still refuses the arms
/// whose primitive the lowered IR does not determine ([`Unit::derived`]), and
/// that late diagnostic names the two types it could not tell apart, which is
/// more than a key could have said.
pub fn implemented(key: &str) -> bool {
    bits_op(key)
        || open_coded_key(key)
        || list_closure_key(key)
        || key == "deriveArrayEq"
        || key == "deriveArrayShow"
        || derive_key(key).is_some()
        || runtime::entry(key).is_some()
        || numeric_op(key)
        || prim_leaf(key).is_some()
}

/// `str.show`, `char.eq`, `bool.compare` and their six siblings.
///
/// `semantics/builtins.rs` declares `eq`, `compare`, `show` and `hash` on
/// **every** primitive, and `monomorphize::intrinsic_key` names each after the
/// type's own module — so `Str`'s live under `str.`, `Char`'s under `char.` and
/// `Bool`'s under `bool.`, while the numeric ones are three segments under
/// `num.` because `core/num` defines a dozen types. One rule, two spellings,
/// and this is the half of it `numeric_op` does not cover.
///
/// `str.eq`, `str.compare` and `str.hash` are absent because the archive has
/// bodies for all three and [`runtime::ENTRIES`] is where a body goes.
fn prim_leaf(key: &str) -> Option<(Prim, &str)> {
    let (module, op) = key.split_once('.')?;
    let prim = match module {
        "str" => Prim::Str,
        "char" => Prim::Char,
        "bool" => Prim::Bool,
        _ => return None,
    };
    match (prim, op) {
        (_, "show") | (Prim::Char | Prim::Bool, "eq" | "compare" | "hash") => Some((prim, op)),
        _ => None,
    }
}

/// `toI64`, `wrapToU8`: the target primitive, and whether the conversion's
/// result is that primitive rather than a `Result<T, RangeError>`.
///
/// `Char`, `Str`, `Bool` and `Template` are excluded as targets even though
/// `Prim::all` lists them: `U32.toChar` answers a `Result` because not every
/// `U32` is a Unicode scalar, and `conversion_is_exact` — which classifies by
/// `is_integer`/`is_float` — would call it exact by falling into its
/// integer-to-float arm.
fn conversion_target(from: Prim, op: &str) -> Option<(Prim, bool)> {
    let numeric = |name: &str| {
        Prim::all().iter().copied().find(|p| p.name() == name && (p.is_integer() || p.is_float()))
    };
    if let Some(target) = op.strip_prefix("wrapTo") {
        // The modular form always answers the target type. Its float source is
        // the one place this backend and JavaScript differ: `$wrapTo` truncates
        // and then takes the low bits through a `BigInt`, and this saturates,
        // because `llvm.fptosi.sat` is the only float-to-integer conversion
        // that is not `poison` out of range. VALUE-MODEL.md §11 and its
        // divergence table's row 1 put overflow outside what the two backends
        // promise each other, which is the ground this stands on.
        return numeric(target).map(|to| (to, true));
    }
    let to = numeric(op.strip_prefix("to")?)?;
    Some((to, conversion_is_exact(from, to)))
}

/// The intrinsics this backend emits as instructions rather than as a call.
///
/// The same list [`Unit::open_coded`] matches on, asked ahead of time. Each is
/// here because a call would fetch a word the backend already has the address
/// of; `str.concat` is the one that is a real body, and it is generated because
/// it is one allocation and two `memcpy`s and a `ccc` call per interpolation
/// would cost more than it saves.
fn open_coded_key(key: &str) -> bool {
    matches!(
        key,
        "str.concat"
            | "str.format"
            | "str.len"
            | "list.len"
            | "list.empty"
            | "testing_context.alloc"
            | "testing_context.TestAlloc.allocate"
            | "host_testing.alloc"
            | "host_testing.TestAlloc.allocate"
            | "list.zip"
            | "list.flatten"
            // Claimed for the shape, not for every call site: a counted
            // element declines and falls through to the table entry.
            | "list.get"
            | "testing_assert.report"
            | "testing_assert.failWith"
            | "testing_assert.failExpected"
            | "testing_assert.reportShown"
            | "testing_assert.failExpectedShown"
            // A `Char` **is** a `U32` (`char.buri`: "Exact: every `Char` is a
            // `U32`"), so this is the identity on the register. The debug
            // backend has had it since its list was written; here it was
            // absent, and `core/char`'s eight arriving in the runtime table
            // is what made the absence reachable — `data/strings.buri` and
            // `text/json.buri` both call it right beside a classifier.
            | "char.toU32"
    )
}

/// The `num.<T>.<op>` operations [`Unit::numeric`] emits, asked before
/// emission rather than during it.
///
/// Four families are deliberately absent, and each for its own reason:
///
///  * **`checked*`** answers an `Option<T>`, which needs the overflow test
///    (`llvm.*.with.overflow`) *and* the enum construction; the second half is
///    [`Unit::call_sum`]'s machinery driven by something that is not a call.
///  * **`saturating*`** is the same test with a clamp instead of an `Option`.
///  * **`wrapping*`** is the plain operation — every one of `add`, `sub` and
///    `mul` already wraps here, because §3.4 declines to set `nsw`/`nuw` — but
///    claiming the key without emitting it would be a silent miscompile if that
///    ever changed, and emitting it is one line that has not been asked for.
///  * **`hash`** is `$hashInto` from the FNV-1a **seed**, and the seed is a
///    Rust `const` in `cli/runtime/hash.rs` rather than an exported symbol —
///    so claiming it would mean writing `0x811c9dc5` into the backend, which is
///    the one number VALUE-MODEL.md §12 most wants stated once.
pub fn numeric_op(key: &str) -> bool {
    // `missing_intrinsics` is asked of the *monomorphized* program, before
    // `middle::lower` runs — so `Bounded` is still two segments there and three
    // by the time `Unit::numeric` sees it (`lower.rs`'s `bounded_key`). Both
    // spellings answer yes, because both describe an operation this backend
    // compiles; `stencil/emit.rs::numeric_key` has said so since the change
    // that found it, and this table had drifted from it.
    if key == "num.minValue" || key == "num.maxValue" {
        return true;
    }
    let parts: Vec<&str> = key.split('.').collect();
    let (Some(&"num"), Some(name), Some(op), 3) =
        (parts.first(), parts.get(1), parts.get(2), parts.len())
    else {
        return false;
    };
    let Some(prim) = Prim::all().iter().copied().find(|p| p.name() == *name) else {
        return false;
    };
    if matches!(
        *op,
        "add"
            | "sub"
            | "mul"
            | "div"
            | "rem"
            | "neg"
            | "abs"
            | "signum"
            | "eq"
            | "compare"
            | "show"
            | "hash"
            | "wrappingAdd"
            | "wrappingSub"
            | "wrappingMul"
            | "minValue"
            | "maxValue"
    ) {
        return true;
    }
    // `Checked`, `Wrapping` and `Saturating` are declared on the integer types
    // only (`semantics/builtins.rs`), so a float spelling of one is a key that
    // does not exist rather than one this backend declines.
    if matches!(
        *op,
        "checkedAdd"
            | "checkedSub"
            | "checkedMul"
            | "checkedDiv"
            | "saturatingAdd"
            | "saturatingSub"
            | "saturatingMul"
    ) {
        return prim.is_integer();
    }
    // A conversion is claimed only where its result is the target type;
    // [`conversion_target`] is the same question, asked of the same two types.
    conversion_target(prim, op).is_some_and(|(_, exact)| exact)
}

// ---------------------------------------------------------------------------
// The emitted entry point
// ---------------------------------------------------------------------------

/// The carrier entry signature **as this backend actually emits it**, read
/// back out of an LLVM module.
///
/// Not the shared constant restated: a throwaway context and module are made,
/// a door type is built the way [`Unit::carrier_fn_type`] builds one, and its
/// LLVM type is *printed* and translated into `backend/carrier.rs`'s canonical
/// spelling. So the bytes this answers come from LLVM's own idea of the
/// function type — which is what a linker and a caller will see — rather than
/// from the table the type was meant to be built from.
///
/// `stencil::asm::carrier_signature` is the other side, and
/// `the_two_carrier_doors_have_one_signature` diffs the two.
pub fn carrier_signature() -> Vec<u8> {
    let ctx = inkwell::context::Context::create();
    let module = ctx.create_module("carrier.signature");
    let ptr = ctx.ptr_type(inkwell::AddressSpace::default());
    let params: Vec<BasicMetadataTypeEnum<'_>> = carrier::ENTRY
        .params
        .iter()
        .map(|w| match w {
            carrier::Word::Ptr => ptr.into(),
        })
        .collect();
    let ty = match carrier::ENTRY.ret {
        None => ctx.void_type().fn_type(&params, false),
        Some(carrier::Word::Ptr) => ptr.fn_type(&params, false),
    };
    let f = module.add_function(carrier::MAIN_ENTRY, ty, Some(Linkage::External));
    // `void (ptr, ptr)`, as LLVM prints it. The translation to the canonical
    // spelling is whitespace and nothing else, which is the point: a third
    // parameter or an `i32` return would come through this untouched and would
    // not match.
    let printed = f.get_type().print_to_string().to_string();
    printed.split_whitespace().collect::<String>().into_bytes()
}

impl<'ctx, 'a> Unit<'ctx, 'a> {
    /// `buri_rt_frames_are_per_carrier()`, in both emitted entry points.
    ///
    /// A Buri frame here is a machine frame — `middle::layout`'s locals area
    /// becomes `alloca`s in an ordinary LLVM function — so a carrier that
    /// enters Buri code brings its own, and `Tasks.parallel` may run two steps
    /// at once. The frame-threaded backend emits no such call and must not
    /// until each carrier owns a Buri stack: `runtime::FRAMES_PER_CARRIER` is
    /// where the difference is argued, and `cli/runtime/lib.rs` §6 is the
    /// contract.
    ///
    /// It is the one call in §6's list that is *optional*, so an entry point
    /// that lost it would be a program whose tasks stopped overlapping — slow,
    /// never wrong.
    fn declare_frames_are_per_carrier(&mut self) {
        let f = self.declare_rt(runtime::FRAMES_PER_CARRIER, &[], None);
        if let Ok(call) = self.builder.build_call(f, &[], "") {
            attrs::set_call_convention(call, attrs::C);
        }
    }

    /// `int main(int, char**)`, the calls `cli/runtime/lib.rs` §6 names, and
    /// SPEC's `Result<(), Str>` contract.
    ///
    /// ```c
    /// int main(int argc, char** argv) {
    ///     buri_rt_argv_init(argc, argv);
    ///     buri_rt_frames_are_per_carrier();
    ///     r = buri_main();
    ///     buri_rt_flush();
    ///     if (tag(r) != Ok) { eprintln(payload(r)); buri_rt_flush(); return 1; }
    ///     return 0;
    /// }
    /// ```
    ///
    /// `.Ok(())` exits 0; `.Err(msg)` prints `msg` to standard error and exits
    /// 1 — byte for byte what the JavaScript backend does
    /// (`js/generate.rs:300-310`), because a program's exit status must not
    /// depend on which backend built it.
    /// **`void buri$carrier$main(void *state, void *out)`** — the C door into
    /// one root, for a caller that is not this artifact's own `main`.
    ///
    /// `backend/carrier.rs` is the signature and the argument order; this is
    /// the LLVM half of it, and it is a `ccc` wrapper in front of the `fastcc`
    /// body and nothing else. What the stencil door spends nine instructions
    /// on — asking `buri_rt_stack_acquire` for a Buri data stack no kernel
    /// guards — has no counterpart here, and should not: a frame in this
    /// backend *is* the machine's, and a carrier thread's machine stack is the
    /// OS's and already guarded on the side it grows towards.
    ///
    /// That asymmetry is the reason the signature is written down in a third
    /// file. The two doors are two different programs behind one C type, and
    /// the only thing that keeps them substitutable is that neither one spells
    /// the type itself.
    ///
    /// **External linkage**, unlike [`Unit::entry_thunk`]'s private one: this
    /// is a name something outside the artifact looks up. `-dead_strip` deletes
    /// it from a Mach-O program that never references it, so a single-threaded
    /// artifact pays nothing for it there. On ELF that depends on the door
    /// having a section of its own to be collected — `--gc-sections` collects
    /// sections, not symbols — and neither backend asks for one today, which
    /// `tests/native/stencil.rs`'s carrier-door test measures at 84 and 128
    /// bytes for the stencil emitter rather than assuming either way.
    ///
    /// `state` is passed and not read — `carrier.rs` says why the parameter is
    /// fixed before the caller that fills it exists — and a root with
    /// parameters gets no door at all, for the reason `stencil/mod.rs`'s
    /// `carrier_door` gives: the record's layout is the *call site's*
    /// decision, and there is no call site yet to make it.
    pub fn carrier_door(&mut self, root: FuncIdx, symbol: &str) {
        let Some(func) = self.program.funcs.get(root.index()) else { return };
        if !func.sig.params.is_empty() {
            return;
        }
        let sig = ir::Signature { params: Vec::new(), rets: func.sig.rets.clone() };
        let Some(callee) = self.declare(root) else { return };
        let door = self.module.add_function(symbol, self.carrier_fn_type(), Some(Linkage::External));
        self.platform_abi(door);
        let block = self.ctx.append_basic_block(door, "entry");
        self.builder.position_at_end(block);

        let Ok(call) = self.builder.build_call(callee, &[], "r") else {
            let _ = self.builder.build_return(None);
            return;
        };
        attrs::set_call_convention(call, attrs::FAST);

        // The return area goes through `out`, in the slots this backend
        // returns it in — the same `store_slots` the step entry thunk writes
        // its answer with, because it is the same question.
        let (_, rets) = self.slots_of_sig(&sig);
        let out = door.get_nth_param(carrier::OUT as u32).and_then(|p| p.try_into().ok());
        if let (Some(out), Some(answer)) = (out, call.try_as_basic_value().basic()) {
            let out: PointerValue<'ctx> = out;
            if !rets.is_empty() {
                let align = self.ret_align(&sig);
                let pieces = repr::disassemble(&self.builder, &rets, answer);
                self.store_slots(out, &rets, align, &pieces);
            }
        }
        let _ = self.builder.build_return(None);
    }

    /// The alignment a signature's return area wants, for the one store that
    /// needs to know.
    ///
    /// An aggregate's is `middle::layout`'s; anything else is a word, which is
    /// the widest a scalar slot in a return position is.
    fn ret_align(&mut self, sig: &ir::Signature) -> u32 {
        match sig.rets.first().copied() {
            Some(ir::Type::Agg(id)) => self.reprs.of(self.program, id).layout.align,
            _ => 8,
        }
    }

    pub fn entry_point(&mut self, main: FuncIdx) {
        let Some(func) = self.program.funcs.get(main.index()) else { return };
        let Some(callee) = self.declare(main) else { return };
        let i32t = self.ctx.i32_type();
        let ty = i32t.fn_type(&[i32t.into(), self.ptr_ty().into()], false);
        let shim = self.module.add_function("main", ty, Some(Linkage::External));
        attrs::set_convention(shim, attrs::C);
        let block = self.ctx.append_basic_block(shim, "entry");
        self.builder.position_at_end(block);

        let argv_init =
            self.declare_rt(runtime::ARGV_INIT, &[i32t.into(), self.ptr_ty().into()], None);
        if let (Some(argc), Some(argv)) = (shim.get_nth_param(0), shim.get_nth_param(1)) {
            if let Ok(call) = self.builder.build_call(argv_init, &[argc.into(), argv.into()], "") {
                attrs::set_call_convention(call, attrs::C);
            }
        }
        self.declare_frames_are_per_carrier();

        let Ok(call) = self.builder.build_call(callee, &[], "r") else { return };
        attrs::set_call_convention(call, attrs::FAST);
        let result = call.try_as_basic_value().basic();

        let flush = self.declare_rt(runtime::FLUSH, &[], None);
        if let Ok(call) = self.builder.build_call(flush, &[], "") {
            attrs::set_call_convention(call, attrs::C);
        }

        // The result type is `Result<(), Str>`. Its shape comes from the
        // layout table like every other enum's, so a change to the encoding
        // moves this with it.
        let Some(ir::Type::Agg(id)) = func.sig.rets.first().copied() else {
            let _ = self.builder.build_return(Some(&i32t.const_zero()));
            return;
        };
        let (slots, enum_repr, err_offsets, payload_at, size) = {
            let r = self.reprs.of(self.program, id);
            let offsets = r.layout.variant(1).to_vec();
            let (at, size) = match r.enum_repr() {
                Some(EnumRepr::Tagged { payload, .. }) => (*payload, r.layout.size),
                _ => (0, r.layout.size),
            };
            (r.slots.clone(), r.enum_repr().cloned(), offsets, at, size)
        };
        let Some(result) = result else {
            let _ = self.builder.build_return(Some(&i32t.const_zero()));
            return;
        };
        let pieces = repr::disassemble(&self.builder, &slots, result);
        let ok = self.ctx.append_basic_block(shim, "ok");
        let err = self.ctx.append_basic_block(shim, "err");
        let is_ok = match (&enum_repr, pieces.first().copied()) {
            (Some(EnumRepr::Bare { .. } | EnumRepr::Tagged { .. }), Some(BasicValueEnum::IntValue(tag))) => {
                self.builder
                    .build_int_compare(IntPredicate::EQ, tag, tag.get_type().const_zero(), "isok")
                    .ok()
            }
            _ => None,
        };
        match is_ok {
            Some(c) => {
                let _ = self.builder.build_conditional_branch(c, ok, err);
            }
            None => {
                let _ = self.builder.build_unconditional_branch(ok);
            }
        }

        self.builder.position_at_end(err);
        if let (Some(EnumRepr::Tagged { .. }), Some(BasicValueEnum::IntValue(blob))) =
            (&enum_repr, pieces.get(1).copied())
        {
            let within = err_offsets.first().copied().unwrap_or(0).saturating_sub(payload_at);
            let str_slots = [
                Slot { offset: 0, ty: SlotTy::Scalar(Scalar::Ptr) },
                Slot { offset: 8, ty: SlotTy::Scalar(Scalar::Ptr) },
                Slot { offset: 16, ty: SlotTy::Scalar(Scalar::I64) },
            ];
            let mut argv: Vec<BasicMetadataValueEnum<'ctx>> = Vec::new();
            for slot in &str_slots {
                let shift = u64::from(within.saturating_add(slot.offset)).saturating_mul(8);
                let moved = self
                    .builder
                    .build_right_shift(blob, blob.get_type().const_int(shift, false), false, "e.sh")
                    .unwrap_or(blob);
                let narrow = repr::blob_type(self.ctx, slot.ty.size());
                let cut = self
                    .builder
                    .build_int_truncate_or_bit_cast(moved, narrow, "e.cut")
                    .unwrap_or(moved);
                argv.push(repr::slot_from_bits(self.ctx, &self.builder, *slot, cut).into());
            }
            let types: Vec<BasicMetadataTypeEnum<'ctx>> =
                argv.iter().map(|a| metadata_type_of(self.ctx, *a)).collect();
            let eprintln = self.declare_rt("buri_rt_host_stderr_eprintln", &types, None);
            if let Ok(call) = self.builder.build_call(eprintln, &argv, "") {
                attrs::set_call_convention(call, attrs::C);
            }
            if let Ok(call) = self.builder.build_call(flush, &[], "") {
                attrs::set_call_convention(call, attrs::C);
            }
        }
        let _ = size;
        let _ = self.builder.build_return(Some(&i32t.const_int(1, false)));

        self.builder.position_at_end(ok);
        let _ = self.builder.build_return(Some(&i32t.const_zero()));
    }

    /// The `main` of a **test binary**: every `test` block, in order.
    ///
    /// `stencil/asm.rs::test_entry` is the same statements for the same
    /// reason: a failed assertion is an abort (SPEC 6.10 leaves nothing to
    /// catch), so one process reports at most one failure and `buri test`
    /// assembles a suite's report across as many processes as it has failures.
    /// `buri_rt_test_enter(i)` answers 0 for a block a previous process already
    /// reported, which is what makes the re-run cheap and what tells the
    /// runtime which block an abort belongs to.
    ///
    /// A `test` body answers `()`, so there is nothing to inspect between calls;
    /// `cli/runtime/testing.rs` states the protocol and
    /// `commands/test.rs::run_native` drives it.
    pub fn test_entry_point(&mut self, tests: &[FuncIdx]) {
        let i32t = self.ctx.i32_type();
        let ty = i32t.fn_type(&[i32t.into(), self.ptr_ty().into()], false);
        let shim = self.module.add_function("main", ty, Some(Linkage::External));
        attrs::set_convention(shim, attrs::C);
        let block = self.ctx.append_basic_block(shim, "entry");
        self.builder.position_at_end(block);

        let argv_init =
            self.declare_rt(runtime::ARGV_INIT, &[i32t.into(), self.ptr_ty().into()], None);
        if let (Some(argc), Some(argv)) = (shim.get_nth_param(0), shim.get_nth_param(1)) {
            if let Ok(call) = self.builder.build_call(argv_init, &[argc.into(), argv.into()], "") {
                attrs::set_call_convention(call, attrs::C);
            }
        }
        self.declare_frames_are_per_carrier();
        let word = self.ctx.i64_type();
        for (i, test) in tests.iter().enumerate() {
            let Some(callee) = self.declare(*test) else { continue };
            let enter = self.declare_rt(runtime::TEST_ENTER, &[word.into()], Some(i32t.into()));
            let index = word.const_int(i as u64, false);
            let Ok(answer) = self.builder.build_call(enter, &[index.into()], "enter") else {
                continue;
            };
            attrs::set_call_convention(answer, attrs::C);
            let Some(BasicValueEnum::IntValue(run)) = answer.try_as_basic_value().basic() else {
                continue;
            };
            let body = self.ctx.append_basic_block(shim, "test.run");
            let next = self.ctx.append_basic_block(shim, "test.next");
            let Ok(cond) =
                self.builder.build_int_compare(IntPredicate::NE, run, i32t.const_zero(), "asked")
            else {
                continue;
            };
            let _ = self.builder.build_conditional_branch(cond, body, next);
            self.builder.position_at_end(body);
            if let Ok(call) = self.builder.build_call(callee, &[], "") {
                attrs::set_call_convention(call, attrs::FAST);
            }
            let _ = self.builder.build_unconditional_branch(next);
            self.builder.position_at_end(next);
        }
        let flush = self.declare_rt(runtime::FLUSH, &[], None);
        if let Ok(call) = self.builder.build_call(flush, &[], "") {
            attrs::set_call_convention(call, attrs::C);
        }
        let _ = self.builder.build_return(Some(&i32t.const_zero()));
    }
}

// ---------------------------------------------------------------------------
// What emission will find, for the whole program at once
// ---------------------------------------------------------------------------

/// [`Observed`] for every function, as a fixpoint over the call graph.
///
/// Two things make this necessary rather than a refinement.
///
/// **Soundness across units.** A function is *declared* in every unit that
/// calls it and *defined* in one, and LLVM reasons about a call from the
/// declaration. If the two carried different `memory(...)` bits, a caller in
/// unit B would optimize against a promise unit A's definition does not keep,
/// and no linker would notice. So the answer has to be a property of the
/// program rather than of whichever unit is being emitted.
///
/// **Soundness across calls.** `rc.rs`'s purity fixpoint misses two things
/// (`attrs.rs`'s header has them): only `Array` and `Template` raise
/// `Allocating`, so a struct literal that allocates leaves a function `Pure`;
/// and an inline `ExprKind::Intrinsic` raises purity but never `aborts`. Those
/// have to be found *and then propagated*, because a caller of a function that
/// allocates allocates too — a local scan would let `f` claim `memory(none)`
/// while the `g` it calls builds a list.
///
/// The lattice is three booleans that only ever go from `false` to `true`, so
/// the loop terminates in at most one pass per call-graph level.
pub fn observe(program: &ir::Program, profile: Profile) -> Vec<Observed> {
    let mut out: Vec<Observed> = program
        .funcs
        .iter()
        .map(|f| match &f.body {
            // A function the backend does not define is a runtime import: the
            // world, as far as this compilation can tell.
            ir::Body::Runtime(_) => Observed::opaque(),
            ir::Body::Code(code) => local(code, profile),
        })
        .collect();

    // The one bit whose seed is a property of the graph rather than of a body.
    for (o, cycles) in out.iter_mut().zip(reaches_a_cycle(program)) {
        o.may_diverge |= cycles;
    }

    let mut changed = true;
    while changed {
        changed = false;
        for (i, f) in program.funcs.iter().enumerate() {
            let Some(code) = f.code() else { continue };
            let mut here = out.get(i).copied().unwrap_or_else(Observed::opaque);
            let before = key(here);
            for block in &code.blocks {
                for inst in &block.insts {
                    let callee = match inst {
                        ir::Inst::Call { func, .. } => Some(func.index()),
                        _ => None,
                    };
                    let Some(callee) = callee else { continue };
                    // An unknown callee is the conservative answer, which is
                    // the direction that costs performance and cannot be wrong.
                    let theirs = out.get(callee).copied().unwrap_or_else(Observed::opaque);
                    here.join(theirs);
                }
            }
            if before != key(here) {
                changed = true;
            }
            if let Some(slot) = out.get_mut(i) {
                *slot = here;
            }
        }
    }
    out
}

/// The seven bits, as a tuple, so the fixpoint's "did anything change" is one
/// comparison that a new bit cannot be left out of by accident.
fn key(o: Observed) -> (bool, bool, bool, bool, bool, bool, bool) {
    (
        o.allocates,
        o.aborts,
        o.opaque,
        o.writes_args,
        o.reads_far,
        o.writes_far,
        o.may_diverge,
    )
}

/// The nodes of a directed graph that can reach a **cycle** — every member of
/// one, and everything with a path into one.
///
/// A peel, which is Kahn's algorithm read from the far end: a node every one of
/// whose successors has already been peeled off cannot reach a cycle, so peel it
/// and repeat. What is left when nothing more will peel is exactly the set that
/// can reach one — a self-edge keeps a node in its own count so it never peels,
/// the two members of a two-cycle keep each other in, and every predecessor of
/// either keeps a count above zero. Each edge is walked once.
///
/// Repeated edges are one edge: the peel counts *distinct* successors, because
/// the same edge twice would need two peels and the second never comes. A
/// successor this graph has no node for is dropped, and both callers say why
/// that is their conservative direction.
fn reaches_a_cycle_in(successors: &[Vec<usize>]) -> Vec<bool> {
    let n = successors.len();
    let mut out_edges: Vec<Vec<usize>> = Vec::with_capacity(n);
    let mut preds: Vec<Vec<usize>> = vec![Vec::new(); n];
    for (i, succ) in successors.iter().enumerate() {
        let mut mine: Vec<usize> = succ.iter().copied().filter(|s| *s < n).collect();
        mine.sort_unstable();
        mine.dedup();
        for s in &mine {
            if let Some(row) = preds.get_mut(*s) {
                row.push(i);
            }
        }
        out_edges.push(mine);
    }

    let mut remaining: Vec<usize> = out_edges.iter().map(Vec::len).collect();
    let mut peeled = vec![false; n];
    let mut work: Vec<usize> = Vec::new();
    for (i, left) in remaining.iter().enumerate() {
        if *left == 0 {
            if let Some(slot) = peeled.get_mut(i) {
                *slot = true;
            }
            work.push(i);
        }
    }
    while let Some(i) = work.pop() {
        let Some(predecessors) = preds.get(i) else { continue };
        for &p in predecessors {
            if peeled.get(p).copied() == Some(true) {
                continue;
            }
            let Some(left) = remaining.get_mut(p) else { continue };
            *left = left.saturating_sub(1);
            if *left == 0 {
                if let Some(slot) = peeled.get_mut(p) {
                    *slot = true;
                }
                work.push(p);
            }
        }
    }
    peeled.iter().map(|p| !*p).collect()
}

/// Which functions can reach a cycle in the **call graph** — the seed of
/// [`Observed::may_diverge`].
///
/// [`observe`]'s fixpoint cannot answer this and must not be asked to. It starts
/// each function at what a local scan found and joins its callees' answers
/// upwards, which is a *least* fixpoint: a self-recursive function whose local
/// scan found nothing would have nothing joined into it and would converge on
/// "proven to return", which is the miscompile [`Observed::may_diverge`] exists
/// to stop. So the seeds have to be right before that loop runs, and this is
/// what makes them right.
///
/// The edges are `Inst::Call` — the fixpoint's own edges — plus `DecRef`'s
/// `drop`, which is a call this backend emits and the fixpoint does not follow.
/// It does not have to: a `DecRef` sets `opaque`, which already disqualifies the
/// function. The edge is here so that what the peel walks is the call graph
/// rather than most of it. An index this program has no function for is dropped,
/// which costs nothing: `observe` answers `Observed::opaque` for one, and that
/// carries `may_diverge` anyway.
fn reaches_a_cycle(program: &ir::Program) -> Vec<bool> {
    let callees: Vec<Vec<usize>> = program
        .funcs
        .iter()
        .map(|f| {
            let mut mine: Vec<usize> = Vec::new();
            let Some(code) = f.code() else { return mine };
            for block in &code.blocks {
                for inst in &block.insts {
                    match inst {
                        ir::Inst::Call { func, .. } => mine.push(func.index()),
                        ir::Inst::DecRef { drop: Some(func), .. } => mine.push(func.index()),
                        _ => {}
                    }
                }
            }
            mine
        })
        .collect();
    reaches_a_cycle_in(&callees)
}

/// Which values are *based on* one of this function's parameters, in the sense
/// LangRef's **Pointer Aliasing Rules** define — which is the sense `argmem`
/// is defined in, so it is the one that decides whether an `incref`'s store at
/// `p - 16` is a write to argument memory.
///
/// LangRef's relation is `getelementptr`, `bitcast` and `inttoptr`, closed
/// transitively. Two things follow that are not obvious from the source:
///
///  * **A register projection keeps it.** A `Str` parameter is three LLVM
///    parameters (VALUE-MODEL.md §5.1); `bind_entry_params` binds them, and
///    `GetField`/`GetPayload` disassemble and reassemble the same SSA values
///    without touching memory. So a field of a parameter *is* a parameter, and
///    a pointer shifted out of a tagged enum's payload blob comes back through
///    an `inttoptr`, which LangRef makes based on everything that fed it.
///  * **A load loses it.** `ArrayGet` reads an element out of a block; the
///    result points at a *different* object, and no rule in LangRef's list
///    reaches it from the array. Counting it is a write to the default
///    location, not to `argmem` — which is exactly what `opt
///    -passes=function-attrs` answers for the same shape.
///
/// A block parameter takes the **conjunction** of its incoming arguments, which
/// is a meet rather than a join, so this fixpoint descends from an optimistic
/// start instead of ascending from a pessimistic one. That matters for every
/// counted function in the language and not as a corner case:
/// `middle::tail_calls` rewrites self-recursion into a loop, so a parameter a
/// leaf function counts arrives at the `incref` as a *loop header's* parameter
/// whose incoming values are the entry parameter and itself.
fn argument_based(code: &ir::Code) -> Vec<bool> {
    let mut based = vec![true; code.values()];
    let set = |based: &mut Vec<bool>, v: ir::ValueId, to: bool, changed: &mut bool| {
        if let Some(slot) = based.get_mut(v.index()) {
            if *slot && !to {
                *slot = false;
                *changed = true;
            }
        }
    };
    let mut operands: Vec<ir::ValueId> = Vec::new();
    let mut changed = true;
    while changed {
        changed = false;
        for block in &code.blocks {
            for inst in &block.insts {
                match inst {
                    // A block this function allocated, or one a callee handed
                    // back. Both are ordinary program memory the caller cannot
                    // name, which is the *default* location and not `argmem`.
                    ir::Inst::MakeArray { dest, .. } | ir::Inst::MakeClosure { dest, .. } => {
                        set(&mut based, *dest, false, &mut changed);
                    }
                    // A load: see the header.
                    ir::Inst::ArrayGet { dest, .. } => {
                        set(&mut based, *dest, false, &mut changed);
                    }
                    ir::Inst::Structural { dest, .. } => {
                        set(&mut based, *dest, false, &mut changed);
                    }
                    ir::Inst::Call { dests, .. }
                    | ir::Inst::CallIndirect { dests, .. }
                    | ir::Inst::CallIntrinsic { dests, .. } => {
                        for d in dests {
                            set(&mut based, *d, false, &mut changed);
                        }
                    }
                    // Everything else either produces a scalar — which is
                    // never counted, so its verdict is read by nobody — or
                    // assembles registers out of values already classified
                    // here. `Inst::MakeStruct`, `Inst::MakeEnum`,
                    // `Inst::GetField`, `Inst::GetPayload` and
                    // `Inst::ArraySlice` are that second group: the
                    // conjunction below is what carries the verdict through
                    // them, because a struct built from a parameter's pieces
                    // holds a parameter's pointers.
                    _ => {
                        operands.clear();
                        inst.operands(&mut operands);
                        let all = operands
                            .iter()
                            .all(|u| based.get(u.index()).copied().unwrap_or(false));
                        if !all {
                            for d in inst.results() {
                                set(&mut based, *d, false, &mut changed);
                            }
                        }
                    }
                }
            }
        }
        // The entry block's parameters are the function's, and nothing branches
        // to the entry (`ir::Code`), so they are the fixed point's floor.
        for (i, block) in code.blocks.iter().enumerate() {
            if i == 0 {
                continue;
            }
            for (k, p) in block.params.iter().enumerate() {
                let mut all = true;
                for src in &code.blocks {
                    for t in src.term.targets() {
                        if t.block.index() != i {
                            continue;
                        }
                        if let Some(a) = t.args.get(k) {
                            all &= based.get(a.index()).copied().unwrap_or(false);
                        }
                    }
                }
                set(&mut based, *p, all, &mut changed);
            }
        }
    }
    based
}

/// What one body holds, before anything is propagated into it.
fn local(code: &ir::Code, profile: Profile) -> Observed {
    let mut o = Observed::clean();
    let based = argument_based(code);
    let from_args = |v: &ir::ValueId| based.get(v.index()).copied().unwrap_or(false);
    for block in &code.blocks {
        for inst in &block.insts {
            match inst {
                // One allocation, from `buri_rt_alloc` — which is inaccessible
                // memory (CODEGEN-LLVM.md §3.1's `Alloc`-bounded row).
                ir::Inst::MakeArray { .. } => o.allocates = true,
                ir::Inst::MakeClosure { env: Some(_), .. } => o.allocates = true,
                ir::Inst::Abort { .. } => o.aborts = true,
                // SPEC 6.2: integer division by zero aborts. Float division is
                // an infinity and does not.
                ir::Inst::Binary { op: ir::BinOp::Div | ir::BinOp::Rem, prim, .. } => {
                    if !prim.is_float() {
                        o.aborts = true;
                    }
                }
                // A host capability, and a `decref` whose free path calls the
                // allocator, are both the world.
                ir::Inst::CallIntrinsic { .. } | ir::Inst::CallIndirect { .. } => o.opaque = true,
                // A count is memory, and *which* memory decides the attribute
                // — see [`argument_based`]. `decref` keeps `opaque` on top of
                // that for its free path, which goes back to the allocator
                // through glue this scan cannot see.
                ir::Inst::IncRef { value } => {
                    if from_args(value) {
                        o.writes_args = true;
                    } else {
                        o.reads_far = true;
                        o.writes_far = true;
                    }
                }
                ir::Inst::DecRef { value, .. } => {
                    o.opaque = true;
                    if from_args(value) {
                        o.writes_args = true;
                    } else {
                        o.reads_far = true;
                        o.writes_far = true;
                    }
                }
                // An element load out of a block this function allocated, or
                // out of one a callee returned, reads the default location.
                // Out of a parameter's block it is `argmem`, which
                // `memory(argmem: read)` already covers.
                ir::Inst::ArrayGet { array, .. } => {
                    if !from_args(array) {
                        o.reads_far = true;
                    }
                }
                _ => {}
            }
        }
        // `Profile::defensive_aborts` puts an abort behind a total switch.
        if profile.defensive_aborts() {
            if let ir::Term::Switch { default: None, .. } = &block.term {
                o.aborts = true;
            }
        }
    }
    // A cycle in the control-flow graph is a loop, and nothing in the IR bounds
    // a loop's trip count — see [`Observed::may_diverge`]. Asked as a *cycle*
    // and not as a back edge: `lower` emits blocks in whatever order it built
    // them, and "an edge that points earlier in the list" calls a nested `if`'s
    // join block a loop — which costs the attribute on every branchy function
    // in the program.
    let successors: Vec<Vec<usize>> = code
        .blocks
        .iter()
        .map(|b| b.term.targets().iter().map(|t| t.block.index()).collect())
        .collect();
    if reaches_a_cycle_in(&successors).iter().any(|c| *c) {
        o.may_diverge = true;
    }
    o
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Reverse postorder from the entry — see the module header.
fn reverse_postorder(code: &ir::Code) -> Vec<usize> {
    let n = code.blocks.len();
    let mut seen = vec![false; n];
    let mut post = Vec::with_capacity(n);
    // (block, how many successors have been taken), iterative so a deep CFG
    // cannot exhaust the stack.
    let mut work: Vec<(usize, usize)> = Vec::new();
    if n > 0 {
        work.push((0, 0));
        if let Some(s) = seen.get_mut(0) {
            *s = true;
        }
    }
    while let Some((b, taken)) = work.pop() {
        let Some(block) = code.blocks.get(b) else { continue };
        let targets = block.term.targets();
        match targets.get(taken) {
            Some(t) => {
                work.push((b, taken.saturating_add(1)));
                let next = t.block.index();
                if seen.get(next).copied() == Some(false) {
                    if let Some(s) = seen.get_mut(next) {
                        *s = true;
                    }
                    work.push((next, 0));
                }
            }
            None => post.push(b),
        }
    }
    post.reverse();
    // A block the entry does not reach is still emitted, so that every
    // `BasicBlock` this backend appended ends in a terminator — LLVM's
    // verifier rejects one that does not, and `lower` leaves no unreachable
    // block behind (`Code::retain_reachable`), so this is belt to that brace.
    for (i, s) in seen.iter().enumerate() {
        if !*s {
            post.push(i);
        }
    }
    post
}

/// The LLVM type of an argument already lowered to a metadata value.
///
/// `BasicMetadataValueEnum` has no `get_type`, and a runtime declaration needs
/// one per argument. Every argument this backend passes to a `buri_rt_*` entry
/// is a scalar leaf (`cli/runtime/lib.rs` §2 rule 1), so the three cases below
/// are the whole of it and the fallthrough answers `i64` rather than panicking.
fn metadata_type_of<'ctx>(
    ctx: &'ctx Context,
    v: BasicMetadataValueEnum<'ctx>,
) -> BasicMetadataTypeEnum<'ctx> {
    match v {
        BasicMetadataValueEnum::IntValue(i) => i.get_type().into(),
        BasicMetadataValueEnum::FloatValue(f) => f.get_type().into(),
        BasicMetadataValueEnum::PointerValue(p) => p.get_type().into(),
        BasicMetadataValueEnum::StructValue(s) => s.get_type().into(),
        _ => ctx.i64_type().into(),
    }
}

fn int_predicate(op: ir::BinOp, signed: bool) -> IntPredicate {
    match (op, signed) {
        (ir::BinOp::Eq, _) => IntPredicate::EQ,
        (ir::BinOp::Ne, _) => IntPredicate::NE,
        (ir::BinOp::Lt, true) => IntPredicate::SLT,
        (ir::BinOp::Lt, false) => IntPredicate::ULT,
        (ir::BinOp::Le, true) => IntPredicate::SLE,
        (ir::BinOp::Le, false) => IntPredicate::ULE,
        (ir::BinOp::Gt, true) => IntPredicate::SGT,
        (ir::BinOp::Gt, false) => IntPredicate::UGT,
        (_, true) => IntPredicate::SGE,
        (_, false) => IntPredicate::UGE,
    }
}

/// The **ordering** comparisons: a `NaN` operand answers `false` for every
/// relation, which is IEEE 754 and is what the JavaScript backend's `<` does
/// too. Equality is not here — SPEC 7.2 rules `NaN == NaN`, so `==` and `!=`
/// are [`Unit::float_equality`] and never a single predicate.
fn float_predicate(op: ir::BinOp) -> FloatPredicate {
    match op {
        ir::BinOp::Lt => FloatPredicate::OLT,
        ir::BinOp::Le => FloatPredicate::OLE,
        ir::BinOp::Gt => FloatPredicate::OGT,
        _ => FloatPredicate::OGE,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **The two carrier doors have one signature, byte for byte.**
    ///
    /// The acceptance test of slices B7 and B8 together. Two backends emit a
    /// function under the same name for the same caller to call, and nothing
    /// links them: `stencil/asm.rs` writes machine code and this file writes
    /// LLVM IR, in two modules that never meet. A disagreement would not be a
    /// compile error or a link error — it would be a `ccc` call made with the
    /// wrong number of arguments at run time, in whichever profile the user
    /// happened to build, which is `cli/runtime/lib.rs` §1's *"a wrong answer
    /// at run time"* exactly.
    ///
    /// The two sides are derived rather than restated. The stencil side is the
    /// table its emitter reads its argument registers out of; this side is an
    /// LLVM `FunctionType`, built and then **printed**, so what is compared is
    /// LLVM's own idea of the type a caller will see.
    #[cfg(feature = "backend-stencil")]
    #[test]
    fn the_two_carrier_doors_have_one_signature() {
        let stencil = crate::compiler::backend::stencil::asm::carrier_signature();
        let llvm = carrier_signature();
        assert_eq!(
            String::from_utf8_lossy(&stencil),
            String::from_utf8_lossy(&llvm),
            "the two backends' carrier doors do not have the same C signature"
        );
        assert_eq!(stencil, llvm);
        assert_eq!(llvm, b"void(ptr,ptr)".to_vec());
    }

    /// The LLVM side is read out of a module and not out of the constant, so a
    /// type built with a different arity or a different answer is a different
    /// string. This is what makes the comparison above worth making.
    #[test]
    fn the_printed_signature_is_llvms_own() {
        let ctx = inkwell::context::Context::create();
        let module = ctx.create_module("carrier.signature.control");
        let ptr = ctx.ptr_type(inkwell::AddressSpace::default());
        let render = |f: FunctionValue<'_>| {
            f.get_type().print_to_string().to_string().split_whitespace().collect::<String>()
        };
        let three = module.add_function(
            "three",
            ctx.void_type().fn_type(&[ptr.into(), ptr.into(), ptr.into()], false),
            None,
        );
        let answering =
            module.add_function("answering", ptr.fn_type(&[ptr.into(), ptr.into()], false), None);
        assert_eq!(render(three), "void(ptr,ptr,ptr)");
        assert_eq!(render(answering), "ptr(ptr,ptr)");
        assert_ne!(render(three).into_bytes(), carrier_signature());
        assert_ne!(render(answering).into_bytes(), carrier_signature());
    }

    /// The signature is the shared table's, not a copy of it: the type is
    /// built by walking `carrier::ENTRY`, so an entry added there appears here
    /// without this file being edited.
    #[test]
    fn the_door_type_is_built_from_the_shared_table() {
        assert_eq!(carrier_signature(), carrier::ENTRY.render());
    }
}

#[cfg(test)]
mod cycles {
    use super::*;
    use crate::compiler::semantics::types::FuncIdx;
    use crate::diagnostics::Span;

    /// A program of `edges.len()` functions in which function `i` calls every
    /// index in `edges[i]` from its one block and does nothing else.
    /// [`reaches_a_cycle`] reads calls and nothing else, so this is the whole
    /// of what it needs.
    fn call_graph(edges: &[&[usize]]) -> ir::Program {
        let funcs = edges
            .iter()
            .enumerate()
            .map(|(i, callees)| {
                let mut code = ir::Code::new();
                code.blocks.push(ir::Block {
                    params: Vec::new(),
                    insts: callees
                        .iter()
                        .map(|c| ir::Inst::Call {
                            dests: Vec::new(),
                            func: FuncIdx(*c as u32),
                            args: Vec::new(),
                        })
                        .collect(),
                    term: ir::Term::Return(Vec::new()),
                });
                ir::Func {
                    symbol: format!("f{i}"),
                    debug_name: format!("f{i}"),
                    sig: ir::Signature { params: Vec::new(), rets: Vec::new() },
                    facts: ir::Facts {
                        params: Vec::new(),
                        purity: ir::Purity::Pure,
                        can_abort: false,
                        can_park: false,
                    },
                    unit: 0,
                    body: ir::Body::Code(code),
                    span: Span::default(),
                }
            })
            .collect();
        ir::Program { funcs, units: vec![String::from("main")], types: Vec::new() }
    }

    /// A chain ends. A function that calls itself does not, and neither does
    /// anything that can reach one that does.
    #[test]
    fn a_chain_peels_and_a_cycle_does_not() {
        assert_eq!(reaches_a_cycle(&call_graph(&[&[1], &[2], &[]])), vec![false, false, false]);
        assert_eq!(reaches_a_cycle(&call_graph(&[&[0]])), vec![true]);
        // `0` and `1` are the cycle, `2` calls into it, `3` is off to the side.
        assert_eq!(
            reaches_a_cycle(&call_graph(&[&[1], &[0], &[1], &[]])),
            vec![true, true, true, false]
        );
    }

    /// The same edge twice is one edge. Without the dedup, `0` would need two
    /// peels of `1` and only ever get one, so a straight-line program would
    /// report a cycle and lose `willreturn` everywhere.
    #[test]
    fn a_repeated_call_is_one_edge() {
        assert_eq!(reaches_a_cycle(&call_graph(&[&[1, 1, 1], &[]])), vec![false, false]);
    }

    /// The two shapes a **back-edge** rule gets wrong, which is why this is a
    /// peel and not a comparison of indices. Both are what a control-flow
    /// graph looks like: [`local`] asks the same question of one, and `lower`
    /// numbers blocks in the order it built them rather than in any order a
    /// reader could rely on.
    #[test]
    fn an_edge_that_points_backwards_is_not_a_cycle() {
        // `0 -> 2 -> 1`, and nothing returns. A rule that called every edge
        // into a lower index a loop would call `0 -> 2`'s sibling one.
        assert_eq!(reaches_a_cycle_in(&[vec![2], vec![], vec![1]]), vec![false, false, false]);
        // A diamond — a nested `if` and its join — with the join *before* one
        // of its predecessors.
        assert_eq!(
            reaches_a_cycle_in(&[vec![2, 3], vec![], vec![1], vec![1]]),
            vec![false, false, false, false]
        );
    }
}
