//! `middle::ir` into CLIF, one codegen unit at a time.
//!
//! Everything the design's §2 and §3 name lives here: the `FunctionBuilder`
//! discipline with no `Variable` anywhere, the `Switch`/`br_table` split, the
//! open-coded reference counts with a cold slow path, `call_indirect` through a
//! closure, the aborts, and the generated helpers a template and a derived
//! `Show` need.
//!
//! # The `FunctionBuilder` discipline, and where the design is wrong about it
//!
//! CODEGEN-CRANELIFT.md §2.1's substance holds and is what this does: every IR
//! block is created up front, every block's parameters are appended at
//! creation, and **no `declare_var`/`def_var`/`use_var` is ever called**. The
//! middle IR is already SSA — that is what it was for — so the Braun et al.
//! machinery inside `cranelift-frontend` has nothing to do, and
//! `append_block_param`'s own warning ("has to be called at the creation of the
//! `Block` before adding instructions to it") is satisfied by construction.
//!
//! Its step 3 is wrong, and it is worth saying why rather than quietly doing
//! something else. The design has every block **sealed** after creation and
//! before any body is filled, on the reasoning that once the whole CFG exists
//! the predecessors are known. They are known — but `SSABuilder` does not learn
//! them at `create_block`, it learns them at each branch, and
//! `declare_block_predecessor` asserts `!is_sealed(block)`. Sealing a block and
//! then branching to it trips that assertion on every jump in the function.
//!
//! So sealing happens in exactly one place, `Unit::build_function`, after the
//! body is complete, via `seal_all_blocks`. Nothing is lost, and that is the
//! point of the other half of the design's decision: sealing exists only to
//! drive the variable-to-phi construction this backend does not use, so *when*
//! it happens is invisible. There is still no sealing discipline to get wrong,
//! which is what §2.1 was buying.
//!
//! Blocks are also created *during* a body, for the open-coded reference
//! counts and for a switch's trampolines. Those are not IR blocks, and they are
//! swept up by the same `seal_all_blocks`.
//!
//! # Closures, and the environment block
//!
//! VALUE-MODEL.md §7 is `{ code, env }`, and this keeps that with two additions
//! the flattened calling convention forces:
//!
//!  * **`code` is always a thunk.** A closure's callee cannot be the lifted
//!    lambda itself, because the lambda takes its environment as an aggregate
//!    parameter and an aggregate parameter is its *leaves* — which a call site
//!    holding only a pointer cannot produce. So `code` points at a generated
//!    two-line function that takes the environment as a pointer, and the same
//!    shape covers a lifted lambda and a plain `FnRef` (`middle::closures`
//!    turns a capture-free lambda into the second).
//!  * **The environment block leads with its own drop glue.** `Ty::Fn` does not
//!    record what was captured, so a `decref` of a closure has no type from
//!    which to derive the function that releases the environment's contents.
//!    The block therefore holds that function pointer in its first word and the
//!    environment record at offset 8, and one universal glue reads it. Eight
//!    bytes per closure, against a closure that could not be freed.

use std::collections::BTreeMap;

use cranelift_codegen::ir::condcodes::{FloatCC, IntCC};
use cranelift_codegen::ir::{
    types, AbiParam, Block, BlockArg, FuncRef, Function, InstBuilder, MemFlags, SigRef, Signature,
    StackSlot, StackSlotData, StackSlotKind, TrapCode, Type as ClifType, Value,
};
use cranelift_codegen::Context;
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext, Switch};
use cranelift_module::{DataDescription, DataId, FuncId, Linkage, Module};
use cranelift_object::ObjectModule;

use crate::compiler::backend::cranelift::abi::{self, Abi, PTR};
use crate::compiler::backend::cranelift::runtime;
use crate::compiler::backend::Profile;
use crate::compiler::middle::ir::{
    self as mir, BinOp, Body, Const, Inst, StructuralOp, Target, Term, Type, UnOp, ValueId,
};
use crate::compiler::middle::layout::{
    self, EnumRepr, Repr, Scalar, CLOSURE_ENV, HEADER_RC_OFFSET, IMMORTAL, LIST_LEN, LIST_PTR,
    STR_ASCII_FLAG, STR_BASE, STR_LEN, STR_LEN_MASK, STR_PTR,
};
use crate::compiler::semantics::types::{Prim, Tables, Ty, TyDef};

/// The trap a block that cannot be reached ends with. Cranelift has no
/// `noreturn` attribute, so a call to `buri_rt_abort` is followed by one of
/// these and the block terminates (CODEGEN-CRANELIFT.md §3.7).
pub const UNREACHABLE: TrapCode = TrapCode::unwrap_user(1);

/// The environment record starts one word into its block; the word before it
/// is the block's own drop glue. See this file's header.
pub const ENV_FIELDS: u32 = 8;

/// Loads and stores this backend emits never trap and never claim an
/// alignment: an aggregate covered by machine words (`abi.rs`) is read at
/// offsets its own alignment does not guarantee.
///
/// **`readonly` is deliberately not set, and this is the one constructor that
/// could set it.** Every load and store in this backend goes through here,
/// including the reference-count load and store at `p - 16` that `Emit::rc`
/// walks to —
/// so a `with_readonly()` added for the payload would be added for the count
/// as well, and Cranelift's redundant-load elimination would be entitled to
/// forward a count across an intervening `incref`. This is the same hazard
/// `backend/llvm/attrs.rs` §"The reference count is memory" documents on the
/// LLVM side, where it *was* a live miscompile; here it is not, because CLIF
/// has no function-level memory-effects attribute at all, `mir::Func::facts`
/// is never read on this path, and `opt_level = none` (`mod.rs`) leaves the
/// aegraph mid-end — GVN, LICM, alias analysis — switched off entirely. Both
/// of those would have to change before the flag could be considered, and
/// then it would have to be per-access rather than shared.
pub fn mem() -> MemFlags {
    MemFlags::new().with_notrap()
}

// ---------------------------------------------------------------------------
// Generated helpers
// ---------------------------------------------------------------------------

/// A function this unit generates for itself.
///
/// Every one is `Linkage::Local`, so two units that both need `str.concat` get
/// a copy each and no symbol collides. Duplication rather than a shared unit,
/// because a shared unit would be a link-order dependency for a few hundred
/// bytes.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Helper {
    /// The `code` pointer of a closure over `func`. `env` says whether the
    /// callee's first parameter is the environment record.
    Thunk { func: u32, env: bool },
    /// `str.concat`, open-coded.
    Concat,
    /// An integer, in decimal.
    ShowInt { signed: bool },
    /// `true` or `false`.
    ShowBool,
    /// Release the contents of one value of this type.
    Release { name: String },
    /// Release every element of a `[T]` block.
    ReleaseElems { name: String },
    /// Take a reference on everything *one* element of a `[T]` holds.
    ///
    /// The mirror of `Release`, and the only new helper wave 3d needs. It
    /// exists because `cli/runtime/list.rs` copies element bytes and cannot
    /// know what is counted inside them: `lib.rs` §3 says a result is owned, so
    /// something has to take the `n` new references a copied `[Str]` now holds,
    /// and the runtime is handed this function to do it with. Null where the
    /// element type holds no counted pointer, which is most of them.
    RetainElem { name: String },
    /// Read a function pointer out of a block's first word and call it on the
    /// rest: the glue every closure environment shares.
    EnvGlue,
}

/// A declared helper waiting for a body.
pub struct Pending {
    pub key: Helper,
    pub id: FuncId,
    /// The type a `Release` or `ReleaseElems` walks, carried beside its name so
    /// the body does not have to parse one back.
    pub ty: Option<Ty>,
}

/// What drops the contents of the block a counted pointer names.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Glue {
    /// Bytes, with nothing inside them to release: a `Str`'s allocation.
    None,
    /// A closure environment, which carries its own.
    Env,
    /// A `[T]` block, element by element.
    Elems(Ty),
}

// ---------------------------------------------------------------------------
// The unit
// ---------------------------------------------------------------------------

/// One codegen unit: one `ObjectModule`, and everything declared into it.
pub struct Unit<'a> {
    pub module: ObjectModule,
    pub abi: Abi<'a>,
    pub program: &'a mir::Program,
    pub profile: Profile,
    /// The `FuncId` of every function in the program, by index. A function
    /// this unit defines is `Hidden`; one another unit defines is an `Import`.
    funcs: Vec<Option<FuncId>>,
    /// Which of the program's functions this unit defines.
    pub owned: Vec<usize>,
    imports: BTreeMap<String, FuncId>,
    strings: BTreeMap<String, DataId>,
    helpers: BTreeMap<Helper, FuncId>,
    pending: Vec<Pending>,
    /// What this unit cannot compile, as sentences. Collected rather than
    /// raised at the first, so a program is told everything at once.
    pub errors: Vec<String>,
    ctx: Context,
}

impl<'a> Unit<'a> {
    pub fn new(
        module: ObjectModule,
        abi: Abi<'a>,
        program: &'a mir::Program,
        profile: Profile,
        unit: u32,
    ) -> Unit<'a> {
        let owned = program
            .funcs
            .iter()
            .enumerate()
            .filter(|(_, f)| f.unit == unit && f.code().is_some())
            .map(|(i, _)| i)
            .collect();
        Unit {
            module,
            abi,
            program,
            profile,
            funcs: vec![None; program.funcs.len()],
            owned,
            imports: BTreeMap::new(),
            strings: BTreeMap::new(),
            helpers: BTreeMap::new(),
            pending: Vec::new(),
            errors: Vec::new(),
            ctx: Context::new(),
        }
    }

    /// The `FuncId` for one of the program's functions, declared on demand.
    ///
    /// The linkage table of CODEGEN-CRANELIFT.md §6. `Hidden` rather than
    /// `Export` for a cross-unit callee is the load-bearing choice:
    /// `Linkage::is_final()` is true for it, which is what drives `colocated`
    /// and therefore a direct rather than a GOT-indirect call.
    pub fn func(&mut self, idx: usize) -> Option<FuncId> {
        if let Some(Some(id)) = self.funcs.get(idx) {
            return Some(*id);
        }
        let f = self.program.funcs.get(idx)?;
        let id = match &f.body {
            Body::Runtime(key) => {
                // Only reached where a runtime entry is used as a *value* — a
                // `FnRef` to an intrinsic — because `Lower::call` routes every
                // ordinary call through `intrinsic`. The Buri signature is the
                // right one only for the entries whose C signature *is* the
                // flattened Buri one; anything with an out-pointer has a
                // different arity, and taking its address would be a call
                // shape nothing agrees on.
                let entry = runtime::entry(key)?;
                if !matches!(entry.ret, runtime::Ret::Void | runtime::Ret::Scalar | runtime::Ret::NoReturn)
                {
                    self.errors.push(format!(
                        "the Cranelift backend cannot take the address of `{key}`: its runtime \
                         entry answers through an out-pointer"
                    ));
                    return None;
                }
                let sig = self.abi.signature(self.program, &f.sig);
                self.import(entry.symbol, sig)
            }
            Body::Code(_) => {
                let sig = self.abi.signature(self.program, &f.sig);
                let linkage =
                    if self.owned.contains(&idx) { Linkage::Hidden } else { Linkage::Import };
                match self.module.declare_function(&f.symbol, linkage, &sig) {
                    Ok(id) => id,
                    Err(e) => {
                        self.errors.push(format!("cannot declare `{}`: {e}", f.symbol));
                        return None;
                    }
                }
            }
        };
        if let Some(slot) = self.funcs.get_mut(idx) {
            *slot = Some(id);
        }
        Some(id)
    }

    fn import(&mut self, symbol: &str, sig: Signature) -> FuncId {
        if let Some(id) = self.imports.get(symbol) {
            return *id;
        }
        match self.module.declare_function(symbol, Linkage::Import, &sig) {
            Ok(id) => {
                self.imports.insert(symbol.to_string(), id);
                id
            }
            Err(e) => {
                self.errors.push(format!("cannot import `{symbol}`: {e}"));
                FuncId::from_u32(0)
            }
        }
    }

    /// A `buri_rt_*` entry, with a signature spelled at the call site rather
    /// than derived: the runtime's ABI is C and the program's is not
    /// (`cli/runtime/lib.rs` §2).
    pub fn rt(&mut self, symbol: &str, params: &[ClifType], rets: &[ClifType]) -> FuncId {
        let mut sig = Signature::new(self.abi.call_conv);
        for p in params {
            sig.params.push(AbiParam::new(*p));
        }
        for r in rets {
            sig.returns.push(AbiParam::new(*r));
        }
        self.import(symbol, sig)
    }

    /// A read-only data object holding these bytes, interned by content.
    pub fn bytes(&mut self, text: &str) -> Option<DataId> {
        if let Some(id) = self.strings.get(text) {
            return Some(*id);
        }
        // The symbol is an index and not the content: a name derived from the
        // bytes would put a program's own text in a symbol table.
        let name = format!("buri$s{}", self.strings.len());
        let id = self.module.declare_data(&name, Linkage::Local, false, false).ok()?;
        let mut data = DataDescription::new();
        // One trailing NUL, so that even the empty string has a non-null
        // address — which is the invariant the `Option<Str>` niche spends
        // (VALUE-MODEL.md §6).
        let mut bs = text.as_bytes().to_vec();
        bs.push(0);
        data.define(bs.into_boxed_slice());
        self.module.define_data(id, &data).ok()?;
        self.strings.insert(text.to_string(), id);
        Some(id)
    }

    /// A generated helper: declared now, queued for a body.
    pub fn helper(&mut self, key: Helper, ty: Option<Ty>) -> Option<FuncId> {
        if let Some(id) = self.helpers.get(&key) {
            return Some(*id);
        }
        let sig = self.helper_signature(&key)?;
        let name = helper_name(&key, self.helpers.len());
        let id = self.module.declare_function(&name, Linkage::Local, &sig).ok()?;
        self.helpers.insert(key.clone(), id);
        self.pending.push(Pending { key, id, ty });
        Some(id)
    }

    pub fn helper_signature(&mut self, key: &Helper) -> Option<Signature> {
        let mut sig = Signature::new(self.abi.call_conv);
        match key {
            Helper::Thunk { func, env } => {
                let f = self.program.funcs.get(*func as usize)?;
                sig.params.push(AbiParam::new(PTR));
                for p in f.sig.params.iter().skip(usize::from(*env)) {
                    for leaf in self.abi.leaves(self.program, *p) {
                        sig.params.push(AbiParam::new(leaf.ty));
                    }
                }
                for r in &f.sig.rets {
                    for leaf in self.abi.leaves(self.program, *r) {
                        sig.returns.push(AbiParam::new(leaf.ty));
                    }
                }
            }
            Helper::Concat => {
                for _ in 0..6 {
                    sig.params.push(AbiParam::new(types::I64));
                }
                for _ in 0..3 {
                    sig.returns.push(AbiParam::new(types::I64));
                }
            }
            Helper::ShowInt { .. } => {
                sig.params.push(AbiParam::new(types::I64));
                for _ in 0..3 {
                    sig.returns.push(AbiParam::new(types::I64));
                }
            }
            Helper::ShowBool => {
                sig.params.push(AbiParam::new(types::I8));
                for _ in 0..3 {
                    sig.returns.push(AbiParam::new(types::I64));
                }
            }
            Helper::Release { .. }
            | Helper::ReleaseElems { .. }
            | Helper::RetainElem { .. }
            | Helper::EnvGlue => {
                sig.params.push(AbiParam::new(PTR));
            }
        }
        Some(sig)
    }

    /// Defines every function this unit owns, then every helper they asked
    /// for, until nothing is pending.
    pub fn define_all(&mut self) {
        for idx in self.owned.clone() {
            self.define_func(idx);
        }
        // A helper may ask for another, so the queue is drained rather than
        // iterated: the second one would otherwise be declared and never
        // defined, which is a link error rather than a wrong answer.
        while let Some(job) = self.pending.pop() {
            super::helpers::define(self, job);
        }
    }

    /// Hands a fresh `FunctionBuilder` to `build`, then defines the function.
    ///
    /// One entry point for a body, so that the signature, the finalisation and
    /// the context reset cannot be forgotten in one of six places.
    pub fn build_function<F>(&mut self, id: FuncId, sig: Signature, what: &str, build: F)
    where
        F: FnOnce(&mut Unit<'a>, &mut FunctionBuilder<'_>),
    {
        self.ctx.func.signature = sig;
        {
            let mut fbctx = FunctionBuilderContext::new();
            let mut func = Function::new();
            std::mem::swap(&mut func, &mut self.ctx.func);
            let mut builder = FunctionBuilder::new(&mut func, &mut fbctx);
            build(self, &mut builder);
            // Sealing is here and nowhere else. `declare_block_predecessor`
            // asserts a destination is *not* sealed, so a block may only be
            // sealed once every branch to it exists — which for a whole CFG is
            // the end. Nothing is lost: sealing only drives the SSA
            // construction this backend does not use (`mod.rs`'s header).
            builder.seal_all_blocks();
            builder.finalize();
            std::mem::swap(&mut func, &mut self.ctx.func);
        }
        if let Err(e) = self.module.define_function(id, &mut self.ctx) {
            self.errors.push(format!("cranelift rejected {what}: {e:?}"));
        }
        self.module.clear_context(&mut self.ctx);
    }

    fn define_func(&mut self, idx: usize) {
        let Some(id) = self.func(idx) else { return };
        let Some(f) = self.program.funcs.get(idx) else { return };
        let Some(code) = f.code() else { return };
        let sig = self.abi.signature(self.program, &f.sig);
        let what = f.debug_name.clone();
        self.build_function(id, sig, &what, |unit, b| {
            let mut lower = Lower::new(Cx { unit, b }, code);
            lower.run();
        });
    }
}

// ---------------------------------------------------------------------------
// The primitives every body is built from
// ---------------------------------------------------------------------------

/// A unit and a builder, together.
///
/// Everything that emits CLIF takes one of these: the IR lowering, the drop
/// glue, `str.concat`, and the entry point. It is the reason the
/// reference-counting walk can be written once and used from a function body
/// and from a generated `release` alike.
pub struct Cx<'u, 'a, 'b> {
    pub unit: &'u mut Unit<'a>,
    pub b: &'u mut FunctionBuilder<'b>,
}

impl<'u, 'a, 'b> Cx<'u, 'a, 'b> {
    pub fn tables(&self) -> &'a Tables {
        self.unit.abi.tables
    }

    /// An integer constant, at any integer width.
    ///
    /// Two things `InstBuilder::iconst` will not do, and both of them are
    /// reachable from ordinary Buri source:
    ///
    /// * **A negative immediate at a narrow type.** Cranelift's verifier
    ///   requires the immediate to be the *zero-extended* value, so
    ///   `iconst.i8 -128` — which is what `num.minValue<I8>()` asks for — is
    ///   rejected. The bit pattern is what is meant, so it is masked to the
    ///   width.
    /// * **`i128` at all.** There is no `iconst.i128`; a 128-bit constant is two
    ///   halves and an `iconcat`, which is what `konst` already does for a
    ///   literal and what every constant-producing path needs.
    ///
    /// Both were `unreachable!()` inside `verifier::iconst_bounds` — a panic in
    /// a dependency rather than a diagnostic, which is the worst shape a bug can
    /// have, and the reason this is one function rather than a rule each call
    /// site remembers.
    pub fn iconst(&mut self, ty: ClifType, v: i64) -> Value {
        if ty == types::I128 {
            let lo = self.b.ins().iconst(types::I64, v);
            // Sign-extended: the callers that pass a negative mean a negative,
            // and the ones that pass a bit pattern pass a non-negative one.
            let hi = self.b.ins().iconst(types::I64, if v < 0 { -1 } else { 0 });
            return self.b.ins().iconcat(lo, hi);
        }
        let masked = if ty.bits() >= 64 {
            v
        } else {
            let bits = u32::min(ty.bits(), 63);
            let mask = (1i64 << bits).wrapping_sub(1);
            v & mask
        };
        self.b.ins().iconst(ty, masked)
    }

    pub fn load_at(&mut self, ty: ClifType, addr: Value, offset: u32) -> Value {
        let off = i32::try_from(offset).unwrap_or(0);
        self.b.ins().load(ty, mem(), addr, off)
    }

    pub fn store_at(&mut self, addr: Value, offset: u32, v: Value) {
        let off = i32::try_from(offset).unwrap_or(0);
        self.b.ins().store(mem(), v, addr, off);
    }

    pub fn offset(&mut self, addr: Value, by: u32) -> Value {
        if by == 0 {
            return addr;
        }
        self.b.ins().iadd_imm(addr, i64::from(by))
    }

    pub fn slot(&mut self, size: u32, align: u32) -> Value {
        let s: StackSlot = self.b.create_sized_stack_slot(StackSlotData::new(
            StackSlotKind::ExplicitSlot,
            size.max(1),
            abi::align_shift(align),
        ));
        self.b.ins().stack_addr(PTR, s, 0)
    }

    pub fn copy(&mut self, dest: Value, src: Value, size: u32, align: u32) {
        if size == 0 {
            return;
        }
        let cfg = self.unit.module.isa().frontend_config();
        let a = u8::try_from(align.min(16)).unwrap_or(1);
        self.b.emit_small_memory_copy(cfg, dest, src, u64::from(size), a, a, true, mem());
    }

    pub fn jump(&mut self, block: Block, args: &[Value]) {
        let args: Vec<BlockArg> = args.iter().map(|v| BlockArg::from(*v)).collect();
        self.b.ins().jump(block, &args);
    }

    pub fn brif(&mut self, c: Value, t: Block, ta: &[Value], f: Block, fa: &[Value]) {
        let ta: Vec<BlockArg> = ta.iter().map(|v| BlockArg::from(*v)).collect();
        let fa: Vec<BlockArg> = fa.iter().map(|v| BlockArg::from(*v)).collect();
        self.b.ins().brif(c, t, &ta, f, &fa);
    }

    pub fn rt_ref(&mut self, symbol: &str, params: &[ClifType], rets: &[ClifType]) -> FuncRef {
        let id = self.unit.rt(symbol, params, rets);
        self.unit.module.declare_func_in_func(id, self.b.func)
    }

    pub fn helper_ref(&mut self, key: Helper, ty: Option<Ty>) -> Option<FuncRef> {
        let id = self.unit.helper(key, ty)?;
        Some(self.unit.module.declare_func_in_func(id, self.b.func))
    }

    pub fn func_ref(&mut self, idx: usize) -> Option<FuncRef> {
        let id = self.unit.func(idx)?;
        Some(self.unit.module.declare_func_in_func(id, self.b.func))
    }

    pub fn call1(&mut self, r: FuncRef, args: &[Value]) -> Option<Value> {
        let inst = self.b.ins().call(r, args);
        self.b.inst_results(inst).first().copied()
    }

    pub fn alloc(&mut self, bytes: Value) -> Value {
        let f = self.rt_ref("buri_rt_alloc", &[types::I64], &[PTR]);
        self.call1(f, &[bytes]).unwrap_or(bytes)
    }

    /// The abort, and the trap behind it (§3.7).
    pub fn abort_with(&mut self, message: &str) {
        let Some(data) = self.unit.bytes(message) else { return };
        let gv = self.unit.module.declare_data_in_func(data, self.b.func);
        let ptr = self.b.ins().symbol_value(PTR, gv);
        let len = self.iconst(types::I64, message.len() as i64);
        let f = self.rt_ref("buri_rt_abort", &[PTR, types::I64], &[]);
        self.b.ins().call(f, &[ptr, len]);
        self.b.ins().trap(UNREACHABLE);
    }

    // -- reference counting -------------------------------------------------

    /// Whether a value of this type owns any counted block.
    pub fn counted(&mut self, ty: &Ty) -> bool {
        counted_ty(self.unit.abi.tables, &mut self.unit.abi.layouts, ty, 0)
    }

    /// The open-coded increment and decrement of MEMORY.md §5.1, over every
    /// counted block a value of this type owns.
    ///
    /// The null test in front of both is not conditional on a niche: a `Str`
    /// literal *is* a null `base` (VALUE-MODEL.md §3), so the test is the
    /// common case rather than the exception, and it is one compare.
    pub fn walk_rc(&mut self, ty: &Ty, addr: Value, retain: bool, depth: u32) {
        if depth > RC_DEPTH || !self.counted(ty) {
            return;
        }
        for site in sites_of(self.unit, ty) {
            match site {
                Site::Block { offset, glue } => {
                    let p = self.load_at(PTR, addr, offset);
                    if retain {
                        self.incref(p);
                    } else {
                        let g = self.glue_ref(&glue);
                        self.decref(p, g);
                    }
                }
                Site::Nested { offset, ty } => {
                    let p = self.offset(addr, offset);
                    self.walk_rc(&ty, p, retain, depth.saturating_add(1));
                }
                Site::Boxed { offset, ty } => {
                    let p = self.load_at(PTR, addr, offset);
                    if retain {
                        self.incref(p);
                    } else {
                        let g = self.release_glue(&ty);
                        self.decref(p, g);
                    }
                }
                Site::Tagged { tag, variants } => {
                    self.tagged_rc(addr, tag, &variants, retain, depth);
                }
                Site::Guarded { null_at, ty } => {
                    let p = self.load_at(PTR, addr, null_at);
                    let live = self.b.create_block();
                    let done = self.b.create_block();
                    let is_null = self.b.ins().icmp_imm(IntCC::Equal, p, 0);
                    self.brif(is_null, done, &[], live, &[]);
                    self.b.switch_to_block(live);
                    self.walk_rc(&ty, addr, retain, depth.saturating_add(1));
                    self.jump(done, &[]);
                    self.b.switch_to_block(done);
                }
            }
        }
    }

    fn tagged_rc(
        &mut self,
        addr: Value,
        tag: Scalar,
        variants: &[(u32, Ty, u32)],
        retain: bool,
        depth: u32,
    ) {
        let t = scalar_clif(tag);
        let raw = self.load_at(t, addr, 0);
        let key = if t == types::I32 { raw } else { self.b.ins().uextend(types::I32, raw) };
        let done = self.b.create_block();
        let mut sw = Switch::new();
        let mut arms = Vec::new();
        // One arm per variant that has anything to release, and one block per
        // variant rather than per field, so a variant with two `Str`s is one
        // case and not two.
        let mut by_variant: BTreeMap<u32, Vec<(Ty, u32)>> = BTreeMap::new();
        for (v, ty, offset) in variants {
            by_variant.entry(*v).or_default().push((ty.clone(), *offset));
        }
        for (v, fields) in by_variant {
            let bb = self.b.create_block();
            sw.set_entry(u128::from(v), bb);
            arms.push((bb, fields));
        }
        let otherwise = self.b.create_block();
        sw.emit(self.b, key, otherwise);
        self.b.switch_to_block(otherwise);
        self.jump(done, &[]);
        for (bb, fields) in arms {
            self.b.switch_to_block(bb);
            for (ty, offset) in fields {
                let p = self.offset(addr, offset);
                self.walk_rc(&ty, p, retain, depth.saturating_add(1));
            }
            self.jump(done, &[]);
        }
        self.b.switch_to_block(done);
    }

    /// The saturating increment of MEMORY.md §5.1, branchless.
    ///
    /// ```text
    /// v_rc  = load.i64  v_p-16
    /// v_sum = iadd_imm  v_rc, 1
    /// v_n   = select    (v_rc == IMMORTAL), v_rc, v_sum
    /// store v_n, v_p-16
    /// ```
    ///
    /// CODEGEN-CRANELIFT.md §3.5 writes this as one `uadd_sat`, and that is
    /// the one factual error in the design this backend found: `uadd_sat` in
    /// Cranelift 0.123 is **vector-only** — its controlling type set has
    /// `lanes >= 2`, and the verifier rejects `uadd_sat.i64` outright. A
    /// compare and a `select` are the scalar spelling, they are still
    /// branchless, and `IMMORTAL` still needs no cold path.
    pub fn incref(&mut self, p: Value) {
        let live = self.b.create_block();
        let done = self.b.create_block();
        let is_null = self.b.ins().icmp_imm(IntCC::Equal, p, 0);
        self.brif(is_null, done, &[], live, &[]);
        self.b.switch_to_block(live);
        let rc = self.b.ins().load(types::I64, mem(), p, HEADER_RC_OFFSET);
        let sum = self.b.ins().iadd_imm(rc, 1);
        let immortal = self.b.ins().icmp_imm(IntCC::Equal, rc, IMMORTAL as i64);
        let n = self.b.ins().select(immortal, rc, sum);
        self.b.ins().store(mem(), n, p, HEADER_RC_OFFSET);
        self.jump(done, &[]);
        self.b.switch_to_block(done);
    }

    /// The decrement, with the free on a cold path.
    ///
    /// The inline code is the fast path and nothing else: a count above one and
    /// below `IMMORTAL` is stored back decremented. The sentinel and the count
    /// that reached zero are `buri_rt_decref`, in a block marked cold so the
    /// final layout moves it away from the hot one (CODEGEN-CRANELIFT.md §3.5).
    pub fn decref(&mut self, p: Value, glue: Option<FuncRef>) {
        let live = self.b.create_block();
        let fast = self.b.create_block();
        let slow = self.b.create_block();
        let done = self.b.create_block();
        let is_null = self.b.ins().icmp_imm(IntCC::Equal, p, 0);
        self.brif(is_null, done, &[], live, &[]);
        self.b.switch_to_block(live);
        let rc = self.b.ins().load(types::I64, mem(), p, HEADER_RC_OFFSET);
        let above_one = self.b.ins().icmp_imm(IntCC::UnsignedGreaterThan, rc, 1);
        let immortal = self.iconst(types::I64, IMMORTAL as i64);
        let mortal = self.b.ins().icmp(IntCC::NotEqual, rc, immortal);
        let ordinary = self.b.ins().band(above_one, mortal);
        self.brif(ordinary, fast, &[], slow, &[]);
        self.b.set_cold_block(slow);

        self.b.switch_to_block(fast);
        let n = self.b.ins().iadd_imm(rc, -1);
        self.b.ins().store(mem(), n, p, HEADER_RC_OFFSET);
        self.jump(done, &[]);

        self.b.switch_to_block(slow);
        let g = match glue {
            Some(r) => self.b.ins().func_addr(PTR, r),
            None => self.iconst(PTR, 0),
        };
        let f = self.rt_ref("buri_rt_decref", &[PTR, PTR], &[]);
        self.b.ins().call(f, &[p, g]);
        self.jump(done, &[]);

        self.b.switch_to_block(done);
    }

    fn glue_ref(&mut self, glue: &Glue) -> Option<FuncRef> {
        match glue {
            Glue::None => None,
            Glue::Env => self.helper_ref(Helper::EnvGlue, None),
            Glue::Elems(t) => {
                if !self.counted(t) {
                    return None;
                }
                let name = self.unit.abi.layouts.describe(t);
                self.helper_ref(Helper::ReleaseElems { name }, Some(t.clone()))
            }
        }
    }

    /// The function that releases the contents of a block holding one value of
    /// this type, or `None` where there is nothing to release.
    pub fn release_glue(&mut self, ty: &Ty) -> Option<FuncRef> {
        if !self.counted(ty) {
            return None;
        }
        let name = self.unit.abi.layouts.describe(ty);
        self.helper_ref(Helper::Release { name }, Some(ty.clone()))
    }

    /// The mirror: the function that takes a reference on everything one value
    /// of this type holds, or `None` where it holds nothing counted.
    ///
    /// `None` is the answer for `[Int]`, `[U8]` and every struct of scalars,
    /// which is most of them — and it is passed to the runtime as a null
    /// pointer, so the common case costs a copy and no per-element call at all
    /// (`cli/runtime/list.rs`'s header).
    pub fn retain_glue(&mut self, ty: &Ty) -> Option<FuncRef> {
        if !self.counted(ty) {
            return None;
        }
        let name = self.unit.abi.layouts.describe(ty);
        self.helper_ref(Helper::RetainElem { name }, Some(ty.clone()))
    }

    /// A counted loop over a block's elements.
    pub fn each_element(&mut self, base: Value, count: Value, stride: u32, elem: &Ty, retain: bool) {
        let header = self.b.create_block();
        self.b.append_block_param(header, types::I64);
        let body = self.b.create_block();
        let done = self.b.create_block();
        let zero = self.iconst(types::I64, 0);
        self.jump(header, &[zero]);
        self.b.switch_to_block(header);
        let i = self.b.block_params(header).first().copied().unwrap_or(zero);
        let more = self.b.ins().icmp(IntCC::UnsignedLessThan, i, count);
        self.brif(more, body, &[], done, &[]);
        self.b.switch_to_block(body);
        let scaled = self.b.ins().imul_imm(i, i64::from(stride));
        let p = self.b.ins().iadd(base, scaled);
        self.walk_rc(elem, p, retain, 0);
        let next = self.b.ins().iadd_imm(i, 1);
        self.jump(header, &[next]);
        self.b.switch_to_block(done);
    }
}

// ---------------------------------------------------------------------------
// One function's body
// ---------------------------------------------------------------------------

struct Lower<'u, 'a, 'b> {
    cx: Cx<'u, 'a, 'b>,
    code: &'a mir::Code,
    /// The CLIF value of every IR value, or `None` where it occupies no bytes.
    vals: Vec<Option<Value>>,
    slots: Vec<Option<StackSlot>>,
    blocks: Vec<Block>,
    /// Whether the current block has already been terminated by an
    /// [`Inst::Abort`]. `verify_func` guarantees the IR block ends
    /// `unreachable` after one, and emitting that trap on top of the abort's
    /// own would be a second terminator in a filled block.
    filled: bool,
    /// Interned signatures for `call_indirect` (§3.2), by the shape they
    /// describe. Keyed on the rendered shape because `ir::Type` is not `Ord`
    /// and a cache nothing iterates does not need it to be.
    sigs: BTreeMap<String, SigRef>,
}

impl<'u, 'a, 'b> Lower<'u, 'a, 'b> {
    fn new(cx: Cx<'u, 'a, 'b>, code: &'a mir::Code) -> Lower<'u, 'a, 'b> {
        Lower {
            cx,
            code,
            vals: vec![None; code.values()],
            slots: vec![None; code.values()],
            blocks: Vec::new(),
            filled: false,
            sigs: BTreeMap::new(),
        }
    }

    fn program(&self) -> &'a mir::Program {
        self.cx.unit.program
    }

    fn leaves(&mut self, t: Type) -> Vec<abi::Leaf> {
        let program = self.program();
        self.cx.unit.abi.leaves(program, t)
    }

    fn layout(&mut self, t: Type) -> layout::Layout {
        let program = self.program();
        self.cx.unit.abi.layout(program, t)
    }

    fn source_ty(&self, t: Type) -> Option<Ty> {
        self.cx.unit.abi.source_ty(self.program(), t)
    }

    // -- the discipline -----------------------------------------------------

    fn run(&mut self) {
        for (i, block) in self.code.blocks.iter().enumerate() {
            let cb = self.cx.b.create_block();
            if i == 0 {
                self.cx.b.append_block_params_for_function_params(cb);
            } else {
                for p in &block.params {
                    let ty = self.code.ty_of(*p);
                    for leaf in self.leaves(ty) {
                        self.cx.b.append_block_param(cb, leaf.ty);
                    }
                }
            }
            self.blocks.push(cb);
        }
        for i in 0..self.code.blocks.len() {
            let Some(cb) = self.blocks.get(i).copied() else { continue };
            self.cx.b.switch_to_block(cb);
            self.filled = false;
            self.bind_params(i, cb);
            let Some(block) = self.code.blocks.get(i) else { continue };
            for inst in &block.insts {
                self.inst(inst);
            }
            self.term(&block.term);
        }
    }

    /// Reconstitutes a block's parameters from the machine parameters.
    ///
    /// A scalar *is* the machine parameter. An aggregate is stored into a slot
    /// of its own, which is what makes a block parameter a value rather than a
    /// pointer somebody else may overwrite (`abi.rs`).
    fn bind_params(&mut self, index: usize, cb: Block) {
        let Some(block) = self.code.blocks.get(index) else { return };
        let params: Vec<Value> = self.cx.b.block_params(cb).to_vec();
        let mut at = 0usize;
        for p in &block.params {
            let ty = self.code.ty_of(*p);
            let leaves = self.leaves(ty);
            let taken: Vec<Value> =
                params.get(at..at.saturating_add(leaves.len())).unwrap_or_default().to_vec();
            at = at.saturating_add(leaves.len());
            match Abi::register(ty) {
                Some(_) => self.set(*p, taken.first().copied()),
                None if leaves.is_empty() => self.set(*p, None),
                None => {
                    let addr = self.alloc_slot(*p, ty);
                    for (leaf, v) in leaves.iter().zip(taken) {
                        self.cx.store_at(addr, leaf.offset, v);
                    }
                }
            }
        }
    }

    // -- values -------------------------------------------------------------

    fn set(&mut self, v: ValueId, val: Option<Value>) {
        if let Some(slot) = self.vals.get_mut(v.index()) {
            *slot = val;
        }
    }

    fn get(&self, v: ValueId) -> Option<Value> {
        self.vals.get(v.index()).copied().flatten()
    }

    fn alloc_slot(&mut self, v: ValueId, ty: Type) -> Value {
        if let Some(Some(slot)) = self.slots.get(v.index()).copied() {
            let addr = self.cx.b.ins().stack_addr(PTR, slot, 0);
            self.set(v, Some(addr));
            return addr;
        }
        let l = self.layout(ty);
        let slot = self.cx.b.create_sized_stack_slot(StackSlotData::new(
            StackSlotKind::ExplicitSlot,
            l.size.max(1),
            abi::align_shift(l.align),
        ));
        if let Some(s) = self.slots.get_mut(v.index()) {
            *s = Some(slot);
        }
        let addr = self.cx.b.ins().stack_addr(PTR, slot, 0);
        self.set(v, Some(addr));
        addr
    }

    /// The leaves of an IR value, ready to be passed.
    fn spread(&mut self, v: ValueId, out: &mut Vec<Value>) {
        let ty = self.code.ty_of(v);
        match Abi::register(ty) {
            Some(_) => {
                if let Some(val) = self.get(v) {
                    out.push(val);
                }
            }
            None => {
                let leaves = self.leaves(ty);
                match self.get(v) {
                    Some(addr) => {
                        for leaf in leaves {
                            let val = self.cx.load_at(leaf.ty, addr, leaf.offset);
                            out.push(val);
                        }
                    }
                    // A value with bytes and no address is one whose defining
                    // instruction already recorded an error. Emission fails on
                    // that error, but not before this function is verified, so
                    // the arity has to be right anyway: a short argument list
                    // reports "mismatched argument count" instead of the thing
                    // that actually went wrong.
                    None => {
                        for leaf in leaves {
                            let zero = self.cx.iconst(leaf.ty, 0);
                            out.push(zero);
                        }
                    }
                }
            }
        }
    }

    /// The inverse: takes results off a call and binds them to IR values.
    fn gather(&mut self, dests: &[ValueId], results: &[Value]) {
        let mut at = 0usize;
        for d in dests {
            let ty = self.code.ty_of(*d);
            let leaves = self.leaves(ty);
            let taken: Vec<Value> =
                results.get(at..at.saturating_add(leaves.len())).unwrap_or_default().to_vec();
            at = at.saturating_add(leaves.len());
            match Abi::register(ty) {
                Some(_) => self.set(*d, taken.first().copied()),
                None if leaves.is_empty() => self.set(*d, None),
                None => {
                    let addr = self.alloc_slot(*d, ty);
                    for (leaf, v) in leaves.iter().zip(taken) {
                        self.cx.store_at(addr, leaf.offset, v);
                    }
                }
            }
        }
    }

    // -- instructions -------------------------------------------------------

    fn inst(&mut self, inst: &Inst) {
        match inst {
            Inst::Const { dest, value } => self.konst(*dest, value),
            Inst::Unary { dest, op, prim, arg } => self.unary(*dest, *op, *prim, *arg),
            Inst::Binary { dest, op, prim, lhs, rhs } => {
                self.binary(*dest, *op, *prim, *lhs, *rhs)
            }
            Inst::MakeStruct { dest, fields } => self.make_struct(*dest, fields),
            Inst::MakeEnum { dest, variant, fields } => self.make_enum(*dest, *variant, fields),
            Inst::MakeArray { dest, elems } => self.make_array(*dest, elems),
            Inst::MakeClosure { dest, func, env } => self.make_closure(*dest, func.index(), *env),
            Inst::GetField { dest, agg, index } => self.get_field(*dest, *agg, *index as usize),
            Inst::GetPayload { dest, agg, variant, index } => {
                self.get_payload(*dest, *agg, *variant as usize, *index as usize)
            }
            Inst::GetTag { dest, agg } => self.get_tag(*dest, *agg),
            Inst::ArrayLen { dest, array } => self.array_len(*dest, *array),
            Inst::ArrayGet { dest, array, index } => self.array_get(*dest, *array, *index),
            Inst::ArraySlice { dest, array, from } => self.array_slice(*dest, *array, *from),
            Inst::Call { dests, func, args } => self.call(dests, func.index(), args),
            Inst::CallIndirect { dests, callee, args } => {
                self.call_indirect(dests, *callee, args)
            }
            Inst::CallIntrinsic { dests, key, args } => self.intrinsic(dests, key, args),
            Inst::Structural { dest, op, ty, args } => self.structural(*dest, *op, *ty, args),
            Inst::IncRef { value } => self.rc(*value, true),
            Inst::DecRef { value, .. } => self.rc(*value, false),
            Inst::Abort { message } => {
                self.cx.abort_with(message);
                self.filled = true;
            }
        }
    }

    fn rc(&mut self, value: ValueId, retain: bool) {
        let ty = self.code.ty_of(value);
        let (Some(source), Some(addr)) = (self.source_ty(ty), self.get(value)) else { return };
        self.cx.walk_rc(&source, addr, retain, 0);
    }

    fn konst(&mut self, dest: ValueId, value: &Const) {
        let ty = self.code.ty_of(dest);
        match value {
            Const::Unit => self.set(dest, None),
            Const::Bool(b) => {
                let v = self.cx.iconst(types::I8, i64::from(*b));
                self.set(dest, Some(v));
            }
            Const::Char(c) => {
                let v = self.cx.iconst(types::I32, i64::from(u32::from(*c)));
                self.set(dest, Some(v));
            }
            Const::Null => {
                let v = self.cx.iconst(PTR, 0);
                self.set(dest, Some(v));
            }
            Const::Int { bits, negative } => {
                let raw = if *negative { bits.wrapping_neg() } else { *bits };
                let v = match Abi::register(ty) {
                    Some(t) if t == types::I128 => {
                        let lo = self.cx.iconst(types::I64, raw as i64);
                        let hi = self.cx.iconst(types::I64, (raw >> 64) as i64);
                        self.cx.b.ins().iconcat(lo, hi)
                    }
                    Some(t) => self.cx.iconst(t, raw as i64),
                    None => {
                        self.set(dest, None);
                        return;
                    }
                };
                self.set(dest, Some(v));
            }
            Const::Float(f) => {
                let v = match Abi::register(ty) {
                    Some(t) if t == types::F32 => self.cx.b.ins().f32const(*f as f32),
                    _ => self.cx.b.ins().f64const(*f),
                };
                self.set(dest, Some(v));
            }
            Const::Str(s) => self.string(dest, s),
            // "A value nothing reads, at the type of its result." Cranelift has
            // no `poison`; a zero of each leaf is the honest stand-in, since
            // the claim is that nothing observes it.
            Const::Undef => match Abi::register(ty) {
                Some(t) if t == types::F32 => {
                    let v = self.cx.b.ins().f32const(0.0);
                    self.set(dest, Some(v));
                }
                Some(t) if t == types::F64 => {
                    let v = self.cx.b.ins().f64const(0.0);
                    self.set(dest, Some(v));
                }
                Some(t) if t == types::I128 => {
                    let lo = self.cx.iconst(types::I64, 0);
                    let hi = self.cx.iconst(types::I64, 0);
                    let v = self.cx.b.ins().iconcat(lo, hi);
                    self.set(dest, Some(v));
                }
                Some(t) => {
                    let v = self.cx.iconst(t, 0);
                    self.set(dest, Some(v));
                }
                None => {
                    if self.layout(ty).size == 0 {
                        self.set(dest, None);
                    } else {
                        self.alloc_slot(dest, ty);
                    }
                }
            },
        }
    }

    /// A literal `Str`: `base` is null and the count is `IMMORTAL`, so it
    /// touches no allocator (VALUE-MODEL.md §3).
    fn string(&mut self, dest: ValueId, s: &str) {
        let ty = self.code.ty_of(dest);
        let addr = self.alloc_slot(dest, ty);
        let Some(data) = self.cx.unit.bytes(s) else {
            self.cx.unit.errors.push(String::from("cannot emit a string literal"));
            return;
        };
        let gv = self.cx.unit.module.declare_data_in_func(data, self.cx.b.func);
        let ptr = self.cx.b.ins().symbol_value(PTR, gv);
        let zero = self.cx.iconst(PTR, 0);
        let ascii = if s.is_ascii() { STR_ASCII_FLAG } else { 0 };
        let len = self.cx.iconst(types::I64, (s.len() as u64 | ascii) as i64);
        self.cx.store_at(addr, word(STR_BASE), zero);
        self.cx.store_at(addr, word(STR_PTR), ptr);
        self.cx.store_at(addr, word(STR_LEN), len);
    }

    fn unary(&mut self, dest: ValueId, op: UnOp, prim: Prim, arg: ValueId) {
        let Some(a) = self.get(arg) else {
            self.set(dest, None);
            return;
        };
        let v = match op {
            UnOp::Neg if matches!(prim, Prim::F32 | Prim::F64) => self.cx.b.ins().fneg(a),
            UnOp::Neg => self.cx.b.ins().ineg(a),
            // `Bool` is 0 or 1, so `not` is `x ^ 1` and stays in that set.
            UnOp::Not => self.cx.b.ins().bxor_imm(a, 1),
            UnOp::BitNot => self.cx.b.ins().bnot(a),
        };
        self.set(dest, Some(v));
    }

    fn binary(&mut self, dest: ValueId, op: BinOp, prim: Prim, lhs: ValueId, rhs: ValueId) {
        let (Some(x), Some(y)) = (self.get(lhs), self.get(rhs)) else {
            self.set(dest, None);
            return;
        };
        // A `Str` is an aggregate, so `self.get` answered the *address of its
        // slot* — and comparing two of those with `icmp` compares stack slots,
        // which is a wrong answer rather than a missing one. `middle::derives`
        // lowers a derived `Eq` or `Ord` over anything containing a `Str` to an
        // `ExprKind::Prim` at `Prim::Str`, so this path is reached by every
        // `derive Eq for` on a struct with a string field.
        if matches!(prim, Prim::Str | Prim::Template) {
            self.string_binary(dest, op, x, y);
            return;
        }
        let float = matches!(prim, Prim::F32 | Prim::F64);
        let signed = signed_prim(prim);
        if op.is_comparison() {
            let v = if float {
                let cc = match op {
                    BinOp::Eq => FloatCC::Equal,
                    BinOp::Ne => FloatCC::NotEqual,
                    BinOp::Lt => FloatCC::LessThan,
                    BinOp::Le => FloatCC::LessThanOrEqual,
                    BinOp::Gt => FloatCC::GreaterThan,
                    _ => FloatCC::GreaterThanOrEqual,
                };
                self.cx.b.ins().fcmp(cc, x, y)
            } else {
                let cc = match (op, signed) {
                    (BinOp::Eq, _) => IntCC::Equal,
                    (BinOp::Ne, _) => IntCC::NotEqual,
                    (BinOp::Lt, true) => IntCC::SignedLessThan,
                    (BinOp::Lt, false) => IntCC::UnsignedLessThan,
                    (BinOp::Le, true) => IntCC::SignedLessThanOrEqual,
                    (BinOp::Le, false) => IntCC::UnsignedLessThanOrEqual,
                    (BinOp::Gt, true) => IntCC::SignedGreaterThan,
                    (BinOp::Gt, false) => IntCC::UnsignedGreaterThan,
                    (_, true) => IntCC::SignedGreaterThanOrEqual,
                    (_, false) => IntCC::UnsignedGreaterThanOrEqual,
                };
                self.cx.b.ins().icmp(cc, x, y)
            };
            self.set(dest, Some(v));
            return;
        }
        if float {
            let v = match op {
                BinOp::Add => self.cx.b.ins().fadd(x, y),
                BinOp::Sub => self.cx.b.ins().fsub(x, y),
                BinOp::Mul => self.cx.b.ins().fmul(x, y),
                BinOp::Div => self.cx.b.ins().fdiv(x, y),
                _ => {
                    self.cx
                        .unit
                        .errors
                        .push(format!("the Cranelift backend cannot compile {op:?} on a float"));
                    x
                }
            };
            self.set(dest, Some(v));
            return;
        }
        if matches!(op, BinOp::Div | BinOp::Rem) {
            let v = self.divide(op, prim, x, y);
            self.set(dest, Some(v));
            return;
        }
        let v = match op {
            BinOp::Add => self.cx.b.ins().iadd(x, y),
            BinOp::Sub => self.cx.b.ins().isub(x, y),
            BinOp::Mul => self.cx.b.ins().imul(x, y),
            BinOp::BitAnd => self.cx.b.ins().band(x, y),
            BinOp::BitOr => self.cx.b.ins().bor(x, y),
            BinOp::BitXor => self.cx.b.ins().bxor(x, y),
            _ => x,
        };
        self.set(dest, Some(v));
    }

    /// Division, with the zero test in front of it.
    ///
    /// SPEC 6.2 says division by zero aborts, and the message lives in the
    /// runtime so that `cli/tests/crash/` pins one string for both backends
    /// (CODEGEN-CRANELIFT.md §3.7). The abort block is cold.
    fn divide(&mut self, op: BinOp, prim: Prim, x: Value, y: Value) -> Value {
        let ty = self.cx.b.func.dfg.value_type(y);
        let zero = if ty == types::I128 {
            let lo = self.cx.iconst(types::I64, 0);
            let hi = self.cx.iconst(types::I64, 0);
            self.cx.b.ins().iconcat(lo, hi)
        } else {
            self.cx.iconst(ty, 0)
        };
        let is_zero = self.cx.b.ins().icmp(IntCC::Equal, y, zero);
        let bad = self.cx.b.create_block();
        let ok = self.cx.b.create_block();
        self.cx.brif(is_zero, bad, &[], ok, &[]);
        self.cx.b.set_cold_block(bad);
        self.cx.b.switch_to_block(bad);
        let abort = self.cx.rt_ref("buri_rt_abort_div_zero", &[], &[]);
        self.cx.b.ins().call(abort, &[]);
        self.cx.b.ins().trap(UNREACHABLE);
        self.cx.b.switch_to_block(ok);
        if ty == types::I128 {
            return self.i128_divmod(op, prim, x, y);
        }
        match (op, signed_prim(prim)) {
            (BinOp::Div, true) => self.cx.b.ins().sdiv(x, y),
            (BinOp::Div, false) => self.cx.b.ins().udiv(x, y),
            (_, true) => self.cx.b.ins().srem(x, y),
            (_, false) => self.cx.b.ins().urem(x, y),
        }
    }

    /// 128-bit division and remainder go to the runtime on both backends,
    /// because that is a hundred instructions nobody should inline
    /// (CODEGEN-CRANELIFT.md §3.6). Operands cross as pairs of `i64`s, low half
    /// first, so neither backend has to agree with the platform ABI about how
    /// a 128-bit integer is classified.
    fn i128_divmod(&mut self, op: BinOp, prim: Prim, x: Value, y: Value) -> Value {
        let (a_lo, a_hi) = self.cx.b.ins().isplit(x);
        let (b_lo, b_hi) = self.cx.b.ins().isplit(y);
        let signed = self.cx.iconst(types::I8, i64::from(signed_prim(prim)));
        let quot = self.cx.slot(16, 8);
        let rem = self.cx.slot(16, 8);
        let f = self.cx.rt_ref(
            "buri_rt_i128_divmod",
            &[types::I64, types::I64, types::I64, types::I64, types::I8, PTR, PTR],
            &[],
        );
        self.cx.b.ins().call(f, &[a_lo, a_hi, b_lo, b_hi, signed, quot, rem]);
        let out = if matches!(op, BinOp::Div) { quot } else { rem };
        let lo = self.cx.load_at(types::I64, out, 0);
        let hi = self.cx.load_at(types::I64, out, 8);
        self.cx.b.ins().iconcat(lo, hi)
    }

    // -- aggregates ---------------------------------------------------------

    fn make_struct(&mut self, dest: ValueId, fields: &[ValueId]) {
        let ty = self.code.ty_of(dest);
        let l = self.layout(ty);
        if l.size == 0 {
            self.set(dest, None);
            return;
        }
        let addr = self.alloc_slot(dest, ty);
        let Some(source) = self.source_ty(ty) else { return };
        let field_tys = abi::field_types(self.cx.tables(), &source);
        for (i, f) in fields.iter().enumerate() {
            let offset = l.fields.get(i).copied().unwrap_or(0);
            match field_tys.get(i) {
                Some(ft) if self.cx.unit.abi.layouts.boxes(&source, ft) => {
                    let ft = ft.clone();
                    let p = self.box_value(*f, &ft);
                    self.cx.store_at(addr, offset, p);
                }
                _ => self.write_value(addr, offset, *f),
            }
        }
    }

    /// The indirection a recursive type's field gets (VALUE-MODEL.md §5.2): a
    /// heap block holding the field's bytes.
    fn box_value(&mut self, v: ValueId, ty: &Ty) -> Value {
        let l = self.cx.unit.abi.layouts.of(ty.clone());
        let size = self.cx.iconst(types::I64, i64::from(l.size.max(1)));
        let p = self.cx.alloc(size);
        self.write_value(p, 0, v);
        p
    }

    /// Writes an IR value's bytes at `addr + offset`.
    fn write_value(&mut self, addr: Value, offset: u32, v: ValueId) {
        let ty = self.code.ty_of(v);
        match Abi::register(ty) {
            Some(_) => {
                if let Some(val) = self.get(v) {
                    self.cx.store_at(addr, offset, val);
                }
            }
            None => {
                let l = self.layout(ty);
                let Some(src) = self.get(v).filter(|_| l.size > 0) else { return };
                let dest = self.cx.offset(addr, offset);
                self.cx.copy(dest, src, l.size, l.align);
            }
        }
    }

    fn make_enum(&mut self, dest: ValueId, variant: u32, fields: &[ValueId]) {
        let ty = self.code.ty_of(dest);
        let l = self.layout(ty);
        if l.size == 0 {
            self.set(dest, None);
            return;
        }
        let addr = self.alloc_slot(dest, ty);
        let Repr::Enum { repr, .. } = l.repr.clone() else {
            self.cx.unit.errors.push(String::from("an enum value whose layout is not an enum"));
            return;
        };
        match repr {
            EnumRepr::Bare { tag } => {
                let v = self.cx.iconst(scalar_clif(tag), i64::from(variant));
                self.cx.store_at(addr, 0, v);
            }
            // Variant 0 is `.Some` and its payload is the whole value; variant
            // 1 is `.None`, which is that one word set to null (§6).
            EnumRepr::Niche { null_at } => {
                if variant == 0 {
                    if let Some(f) = fields.first() {
                        self.write_value(addr, 0, *f);
                    }
                } else {
                    let zero = self.cx.iconst(PTR, 0);
                    self.cx.store_at(addr, null_at, zero);
                }
            }
            EnumRepr::Tagged { tag, .. } => {
                let v = self.cx.iconst(scalar_clif(tag), i64::from(variant));
                self.cx.store_at(addr, 0, v);
                let offsets = l.variant(variant as usize).to_vec();
                let Some(source) = self.source_ty(ty) else { return };
                let field_tys = abi::variant_types(self.cx.tables(), &source, variant as usize);
                for (i, f) in fields.iter().enumerate() {
                    let Some(offset) = offsets.get(i).copied() else { continue };
                    match field_tys.get(i) {
                        Some(ft) if self.cx.unit.abi.layouts.boxes(&source, ft) => {
                            let ft = ft.clone();
                            let p = self.box_value(*f, &ft);
                            self.cx.store_at(addr, offset, p);
                        }
                        _ => self.write_value(addr, offset, *f),
                    }
                }
            }
        }
    }

    /// Reads a value of `dest`'s type out of `addr + offset`.
    ///
    /// An aggregate is *copied* into its own slot rather than aliased, which is
    /// the rule `abi.rs`'s header argues for.
    fn read_into(&mut self, dest: ValueId, addr: Value, offset: u32, boxed: bool) {
        let ty = self.code.ty_of(dest);
        if boxed {
            let p = self.cx.load_at(PTR, addr, offset);
            let l = self.layout(ty);
            if l.size == 0 {
                self.set(dest, None);
                return;
            }
            let d = self.alloc_slot(dest, ty);
            self.cx.copy(d, p, l.size, l.align);
            return;
        }
        match Abi::register(ty) {
            Some(t) => {
                let v = self.cx.load_at(t, addr, offset);
                self.set(dest, Some(v));
            }
            None => {
                let l = self.layout(ty);
                if l.size == 0 {
                    self.set(dest, None);
                    return;
                }
                let d = self.alloc_slot(dest, ty);
                let src = self.cx.offset(addr, offset);
                self.cx.copy(d, src, l.size, l.align);
            }
        }
    }

    fn boxed_field(&mut self, owner: Type, index: usize, variant: Option<usize>) -> bool {
        let Some(source) = self.source_ty(owner) else { return false };
        let fields = match variant {
            Some(v) => abi::variant_types(self.cx.tables(), &source, v),
            None => abi::field_types(self.cx.tables(), &source),
        };
        match fields.get(index) {
            Some(f) => self.cx.unit.abi.layouts.boxes(&source, f),
            None => false,
        }
    }

    fn get_field(&mut self, dest: ValueId, agg: ValueId, index: usize) {
        let ty = self.code.ty_of(agg);
        let l = self.layout(ty);
        let Some(addr) = self.get(agg) else {
            self.set(dest, None);
            return;
        };
        let offset = l.fields.get(index).copied().unwrap_or(0);
        let boxed = self.boxed_field(ty, index, None);
        self.read_into(dest, addr, offset, boxed);
    }

    fn get_payload(&mut self, dest: ValueId, agg: ValueId, variant: usize, index: usize) {
        let ty = self.code.ty_of(agg);
        let l = self.layout(ty);
        let Some(addr) = self.get(agg) else {
            self.set(dest, None);
            return;
        };
        let offset = l.variant(variant).get(index).copied().unwrap_or(0);
        let boxed = self.boxed_field(ty, index, Some(variant));
        self.read_into(dest, addr, offset, boxed);
    }

    fn get_tag(&mut self, dest: ValueId, agg: ValueId) {
        let ty = self.code.ty_of(agg);
        let l = self.layout(ty);
        let addr = self.get(agg);
        let tag = match (&l.repr, addr) {
            (Repr::Enum { repr: EnumRepr::Bare { tag }, .. }, Some(a))
            | (Repr::Enum { repr: EnumRepr::Tagged { tag, .. }, .. }, Some(a)) => {
                let t = scalar_clif(*tag);
                let raw = self.cx.load_at(t, a, 0);
                if t == types::I32 {
                    raw
                } else {
                    self.cx.b.ins().uextend(types::I32, raw)
                }
            }
            // `.None` is the null, and the variant it stands for is 1 —
            // `Some` is declared first (`core/option`).
            (Repr::Enum { repr: EnumRepr::Niche { null_at }, .. }, Some(a)) => {
                let at = *null_at;
                let p = self.cx.load_at(PTR, a, at);
                let is_null = self.cx.b.ins().icmp_imm(IntCC::Equal, p, 0);
                self.cx.b.ins().uextend(types::I32, is_null)
            }
            // A zero-sized or uninhabited enum: there is one variant and it is
            // variant zero.
            _ => self.cx.iconst(types::I32, 0),
        };
        self.set(dest, Some(tag));
    }

    fn array_len(&mut self, dest: ValueId, array: ValueId) {
        let v = match self.get(array) {
            Some(addr) => self.cx.load_at(types::I64, addr, word(LIST_LEN)),
            None => self.cx.iconst(types::I64, 0),
        };
        self.set(dest, Some(v));
    }

    /// The stride a `[T]` indexes by, and the element type.
    fn element(&mut self, list: Type) -> (u32, Option<Ty>) {
        let Some(source) = self.source_ty(list) else { return (1, None) };
        let Some(elem) = abi::element_type(&source) else { return (1, None) };
        let l = self.cx.unit.abi.layouts.of(elem.clone());
        (l.stride.max(1), Some(elem))
    }

    fn array_get(&mut self, dest: ValueId, array: ValueId, index: ValueId) {
        let (stride, _) = self.element(self.code.ty_of(array));
        let (Some(addr), Some(i)) = (self.get(array), self.get(index)) else {
            self.set(dest, None);
            return;
        };
        let base = self.cx.load_at(PTR, addr, word(LIST_PTR));
        let scaled = self.cx.b.ins().imul_imm(i, i64::from(stride));
        let p = self.cx.b.ins().iadd(base, scaled);
        self.read_into(dest, p, 0, false);
    }

    /// `xs[from..]`, as a fresh list.
    ///
    /// A **copy**, not a view. VALUE-MODEL.md §4 is explicit that a `[T]` is
    /// never a view — its header is at `ptr - 16` — so handing back an interior
    /// pointer would make the next `decref` of that value read a header that is
    /// not one. Every element is retained on the way out, so the counts balance
    /// against the copy's own eventual release.
    fn array_slice(&mut self, dest: ValueId, array: ValueId, from: ValueId) {
        let list_ty = self.code.ty_of(array);
        let (stride, elem) = self.element(list_ty);
        let (Some(addr), Some(start)) = (self.get(array), self.get(from)) else {
            self.set(dest, None);
            return;
        };
        let base = self.cx.load_at(PTR, addr, word(LIST_PTR));
        let len = self.cx.load_at(types::I64, addr, word(LIST_LEN));
        let count = self.cx.b.ins().isub(len, start);
        let out = self.alloc_slot(dest, self.code.ty_of(dest));
        let stride_v = self.cx.iconst(types::I64, i64::from(stride));
        let new = self.cx.rt_ref("buri_rt_list_new", &[types::I64, types::I64, PTR], &[PTR]);
        let fresh = self.cx.call1(new, &[count, stride_v, out]).unwrap_or(base);
        let scaled = self.cx.b.ins().imul_imm(start, i64::from(stride));
        let src = self.cx.b.ins().iadd(base, scaled);
        let bytes = self.cx.b.ins().imul_imm(count, i64::from(stride));
        let cfg = self.cx.unit.module.isa().frontend_config();
        self.cx.b.call_memcpy(cfg, fresh, src, bytes);
        if let Some(elem) = elem {
            if self.cx.counted(&elem) {
                self.cx.each_element(fresh, count, stride, &elem, true);
            }
        }
    }

    fn make_array(&mut self, dest: ValueId, elems: &[ValueId]) {
        let ty = self.code.ty_of(dest);
        let (stride, _) = self.element(ty);
        let out = self.alloc_slot(dest, ty);
        let count = self.cx.iconst(types::I64, elems.len() as i64);
        let stride_v = self.cx.iconst(types::I64, i64::from(stride));
        let new = self.cx.rt_ref("buri_rt_list_new", &[types::I64, types::I64, PTR], &[PTR]);
        let p = self.cx.call1(new, &[count, stride_v, out]).unwrap_or(out);
        for (i, e) in elems.iter().enumerate() {
            let offset = u32::try_from(i).unwrap_or(0).saturating_mul(stride);
            self.write_value(p, offset, *e);
        }
    }

    fn make_closure(&mut self, dest: ValueId, func: usize, env: Option<ValueId>) {
        let ty = self.code.ty_of(dest);
        let addr = self.alloc_slot(dest, ty);
        let key = Helper::Thunk { func: u32::try_from(func).unwrap_or(0), env: env.is_some() };
        let Some(thunk) = self.cx.helper_ref(key, None) else { return };
        let code = self.cx.b.ins().func_addr(PTR, thunk);
        let env_ptr = match env {
            None => self.cx.iconst(PTR, 0),
            Some(e) => self.build_env(e),
        };
        self.cx.store_at(addr, 0, code);
        self.cx.store_at(addr, word(CLOSURE_ENV), env_ptr);
    }

    /// The environment block: its own drop glue in the first word, the
    /// environment record at [`ENV_FIELDS`]. See this file's header.
    fn build_env(&mut self, env: ValueId) -> Value {
        let ty = self.code.ty_of(env);
        let l = self.layout(ty);
        let size = self.cx.iconst(types::I64, i64::from(l.size.saturating_add(ENV_FIELDS)));
        let p = self.cx.alloc(size);
        let glue = match self.source_ty(ty) {
            Some(source) => match self.cx.release_glue(&source) {
                Some(r) => self.cx.b.ins().func_addr(PTR, r),
                None => self.cx.iconst(PTR, 0),
            },
            None => self.cx.iconst(PTR, 0),
        };
        self.cx.store_at(p, 0, glue);
        let fields = self.cx.offset(p, ENV_FIELDS);
        self.write_value(fields, 0, env);
        p
    }

    // -- calls --------------------------------------------------------------

    fn call(&mut self, dests: &[ValueId], func: usize, args: &[ValueId]) {
        // The arguments are spread *after* the intrinsic tables have had their
        // chance, so that an operation this backend open-codes does not first
        // emit a load of every leaf it is not going to pass.
        if let Some(f) = self.program().funcs.get(func) {
            if let Body::Runtime(key) = &f.body {
                // The same key reaches this backend two ways: as an
                // `Inst::CallIntrinsic` where the front end spelled it inline,
                // and as an `Inst::Call` to a `Body::Runtime` function where it
                // was a method. Both go through `intrinsic`, and nothing falls
                // through to the ordinary call path below — because that path
                // declares the import with the *Buri* signature, and an entry
                // that writes its result through an out-pointer
                // (`cli/runtime/lib.rs` §2 rule 2) does not have one.
                let key = key.clone();
                self.intrinsic(dests, &key, args);
                return;
            }
        }
        let mut vals = Vec::new();
        for a in args {
            self.spread(*a, &mut vals);
        }
        let Some(r) = self.cx.func_ref(func) else { return };
        let inst = self.cx.b.ins().call(r, &vals);
        let results = self.cx.b.inst_results(inst).to_vec();
        self.gather(dests, &results);
    }

    /// A call through a closure value (§3.2): load `code` and `env`, then
    /// `call_indirect` on a signature interned per shape.
    fn call_indirect(&mut self, dests: &[ValueId], callee: ValueId, args: &[ValueId]) {
        let Some(clo) = self.get(callee) else { return };
        let code = self.cx.load_at(PTR, clo, 0);
        let env = self.cx.load_at(PTR, clo, word(CLOSURE_ENV));
        let mut vals = vec![env];
        let mut params = vec![PTR];
        for a in args {
            let before = vals.len();
            self.spread(*a, &mut vals);
            for v in vals.get(before..).unwrap_or_default().to_vec() {
                params.push(self.cx.b.func.dfg.value_type(v));
            }
        }
        let mut rets = Vec::new();
        for d in dests {
            let ty = self.code.ty_of(*d);
            for leaf in self.leaves(ty) {
                rets.push(leaf.ty);
            }
        }
        let sig = self.indirect_sig(&params, &rets);
        let inst = self.cx.b.ins().call_indirect(sig, code, &vals);
        let results = self.cx.b.inst_results(inst).to_vec();
        self.gather(dests, &results);
    }

    fn indirect_sig(&mut self, params: &[ClifType], rets: &[ClifType]) -> SigRef {
        let key = format!("{params:?}->{rets:?}");
        if let Some(s) = self.sigs.get(&key) {
            return *s;
        }
        let mut s = Signature::new(self.cx.unit.abi.call_conv);
        for t in params {
            s.params.push(AbiParam::new(*t));
        }
        for t in rets {
            s.returns.push(AbiParam::new(*t));
        }
        let r = self.cx.b.import_signature(s);
        self.sigs.insert(key, r);
        r
    }

    /// One intrinsic, through whichever of the four routes owns it.
    ///
    /// The order is not arbitrary. `numeric` and `open_coded` come first
    /// because they emit *instructions* and never a call, and asking them
    /// first means the arguments are never spread for a call that is not going
    /// to happen. `derived` comes next because `derivePrimShow` dispatches on
    /// the argument's type rather than on the key. Only what is left is a
    /// table lookup.
    fn intrinsic(&mut self, dests: &[ValueId], key: &str, args: &[ValueId]) {
        if self.numeric(dests, key, args)
            || self.bits(dests, key, args)
            || self.prim_trait(dests, key, args)
            || self.open_coded(dests, key, args)
            || self.derived(dests, key, args)
        {
            return;
        }
        let Some(entry) = runtime::entry(key) else {
            self.cx
                .unit
                .errors
                .push(format!("the native runtime has no implementation of `{key}`"));
            return;
        };
        self.runtime_call(dests, entry, args);
    }

    /// The one emission rule `cranelift/runtime.rs`'s header states: the
    /// flattened Buri arguments, then the element pair, then the out-pointer.
    fn runtime_call(&mut self, dests: &[ValueId], entry: &'static runtime::Entry, args: &[ValueId]) {
        let mut vals: Vec<Value> = Vec::new();
        let mut params: Vec<ClifType> = Vec::new();
        for (i, a) in args.iter().enumerate() {
            // A **context argument is dropped**, whatever it weighs.
            //
            // VALUE-MODEL.md §8 drops a context of zero-sized implementations
            // from every signature, and every `<C: Alloc>` bound in `core/str`
            // and `core/list` is one *in a program using `core/host`* — so
            // relying on "it spreads to no leaves" worked until a program built
            // `context { Alloc: alloc() }` from `core/testing/context`, whose
            // `TestAlloc` carries an `I64`. Then the context spread one extra
            // argument into a C call that has no parameter for it, and the
            // arguments after it landed in the wrong registers.
            //
            // The runtime allocates through `buri_rt_alloc` and has no use for
            // a context at all, so dropping it here is right for every entry
            // rather than a special case for some.
            if matches!(self.source_ty(self.code.ty_of(*a)), Some(Ty::Ctx(_))) {
                continue;
            }
            if entry.by_ref == Some(i) {
                let addr = self.address_of(*a);
                vals.push(addr);
                params.push(PTR);
                continue;
            }
            let before = vals.len();
            self.spread(*a, &mut vals);
            for v in vals.get(before..).unwrap_or_default().to_vec() {
                params.push(self.cx.b.func.dfg.value_type(v));
            }
        }
        if entry.extra == runtime::Extra::Element {
            let elem = self.element_ty(dests, args);
            let stride = match &elem {
                Some(t) => self.cx.unit.abi.layouts.of(t.clone()).stride.max(1),
                None => 1,
            };
            let s = self.cx.iconst(types::I64, i64::from(stride));
            vals.push(s);
            params.push(types::I64);
            let glue = match elem.as_ref().and_then(|t| self.cx.retain_glue(t)) {
                Some(r) => self.cx.b.ins().func_addr(PTR, r),
                None => self.cx.iconst(PTR, 0),
            };
            vals.push(glue);
            params.push(PTR);
        }

        match entry.ret {
            runtime::Ret::Void | runtime::Ret::NoReturn => {
                let r = self.cx.rt_ref(entry.symbol, &params, &[]);
                self.cx.b.ins().call(r, &vals);
            }
            runtime::Ret::Scalar => {
                let mut rets = Vec::new();
                for d in dests {
                    let ty = self.code.ty_of(*d);
                    for leaf in self.leaves(ty) {
                        rets.push(leaf.ty);
                    }
                }
                let r = self.cx.rt_ref(entry.symbol, &params, &rets);
                let inst = self.cx.b.ins().call(r, &vals);
                let results = self.cx.b.inst_results(inst).to_vec();
                self.gather(dests, &results);
            }
            runtime::Ret::Tag => {
                let Some(dest) = dests.first().copied() else { return };
                let r = self.cx.rt_ref(entry.symbol, &params, &[types::I32]);
                let raw = self.cx.call1(r, &vals);
                let Some(raw) = raw else { return };
                let want = self.tag_ty(dest).unwrap_or(types::I32);
                let narrowed = if want == types::I32 {
                    raw
                } else if want.bits() < 32 {
                    self.cx.b.ins().ireduce(want, raw)
                } else {
                    self.cx.b.ins().uextend(want, raw)
                };
                self.bind_tag(dest, narrowed);
            }
            runtime::Ret::Out => {
                let Some(dest) = dests.first().copied() else { return };
                let dty = self.code.ty_of(dest);
                if self.layout(dty).size == 0 {
                    return;
                }
                let out = self.alloc_slot(dest, dty);
                vals.push(out);
                params.push(PTR);
                let r = self.cx.rt_ref(entry.symbol, &params, &[]);
                self.cx.b.ins().call(r, &vals);
            }
            runtime::Ret::Opt => {
                let Some(dest) = dests.first().copied() else { return };
                self.option_call(dest, entry.symbol, &mut vals, &mut params);
            }
        }
    }

    /// A runtime entry answering an `Option<T>` (`cli/runtime/lib.rs` §2
    /// rule 3), turned into whatever `middle::layout` chose for the enum.
    ///
    /// The out-pointer is the destination slot **already offset to `.Some`'s
    /// payload**, so the runtime writes the value where it belongs and there is
    /// no copy after the call. That works for both representations for the same
    /// reason: a niche-encoded `Option` puts the payload at offset zero and
    /// spends one of its own pointers as the discriminant, and a tagged one
    /// records the payload's offset in `variants[0]`.
    ///
    /// On the absent path a tagged enum gets its tag and a niche gets a null
    /// pointer; neither writes the payload, and nothing reads it, because
    /// `GetPayload` and the reference-count walk both test the discriminant
    /// first.
    fn option_call(
        &mut self,
        dest: ValueId,
        symbol: &str,
        vals: &mut Vec<Value>,
        params: &mut Vec<ClifType>,
    ) {
        let dty = self.code.ty_of(dest);
        let l = self.layout(dty);
        let slot = self.alloc_slot(dest, dty);
        let payload_at = match &l.repr {
            // `.Some`'s payload *is* the whole value (`middle/layout.rs`'s
            // `build_enum`), so the offset is zero.
            Repr::Enum { repr: EnumRepr::Niche { .. }, .. } => 0,
            Repr::Enum { variants, .. } => {
                variants.first().and_then(|v| v.first().copied()).unwrap_or(0)
            }
            _ => 0,
        };
        let out = self.cx.offset(slot, payload_at);
        vals.push(out);
        params.push(PTR);
        let r = self.cx.rt_ref(symbol, params, &[types::I32]);
        let Some(disc) = self.cx.call1(r, vals) else { return };

        let present = self.cx.b.create_block();
        let absent = self.cx.b.create_block();
        let done = self.cx.b.create_block();
        // `BURI_OK` is `-1` rather than `0` so that an error variant's index is
        // its index; here there is one error arm, and it is `.None`.
        let is_ok = self.cx.b.ins().icmp_imm(IntCC::Equal, disc, i64::from(runtime::BURI_OK));
        self.cx.brif(is_ok, present, &[], absent, &[]);

        self.cx.b.switch_to_block(present);
        match &l.repr {
            Repr::Enum { repr: EnumRepr::Bare { tag }, .. }
            | Repr::Enum { repr: EnumRepr::Tagged { tag, .. }, .. } => {
                let t = scalar_clif(*tag);
                let zero = self.cx.iconst(t, 0);
                self.cx.store_at(slot, 0, zero);
            }
            // Nothing: the payload the runtime just wrote carries a non-null
            // pointer at `null_at`, which *is* `.Some`. `BuriStr::empty` in the
            // runtime points at a one-byte static for exactly this reason.
            _ => {}
        }
        self.cx.jump(done, &[]);

        self.cx.b.switch_to_block(absent);
        match &l.repr {
            Repr::Enum { repr: EnumRepr::Bare { tag }, .. }
            | Repr::Enum { repr: EnumRepr::Tagged { tag, .. }, .. } => {
                let t = scalar_clif(*tag);
                let one = self.cx.iconst(t, 1);
                self.cx.store_at(slot, 0, one);
            }
            Repr::Enum { repr: EnumRepr::Niche { null_at }, .. } => {
                let null = self.cx.iconst(PTR, 0);
                let at = *null_at;
                self.cx.store_at(slot, at, null);
            }
            _ => {}
        }
        self.cx.jump(done, &[]);
        self.cx.b.switch_to_block(done);
    }

    /// The address of a value, spilling a register-shaped one to get it.
    ///
    /// `lib.rs` §2 rule 4's by-address argument. An aggregate already lives in
    /// a slot, so this is a load of the pointer it is; a scalar is stored into
    /// a fresh one, which is the single store the rule costs.
    fn address_of(&mut self, v: ValueId) -> Value {
        let ty = self.code.ty_of(v);
        let l = self.layout(ty);
        match Abi::register(ty) {
            Some(_) => {
                let slot = self.cx.slot(l.size.max(1), l.align.max(1));
                if let Some(val) = self.get(v) {
                    self.cx.store_at(slot, 0, val);
                }
                slot
            }
            None => match self.get(v) {
                Some(addr) => addr,
                None => self.cx.iconst(PTR, 0),
            },
        }
    }

    /// The element type of the `[T]` a `list.*` entry operates on.
    ///
    /// The argument's, where there is a `[T]` argument; the *result's*
    /// otherwise, which is what covers `list.repeat` and `list.empty` — the two
    /// whose only mention of `T` is in the return type.
    fn element_ty(&mut self, dests: &[ValueId], args: &[ValueId]) -> Option<Ty> {
        for a in args {
            let ty = self.code.ty_of(*a);
            if let Some(Ty::Array(e)) = self.source_ty(ty) {
                return Some((*e).clone());
            }
        }
        let dest = dests.first().copied()?;
        match self.source_ty(self.code.ty_of(dest))? {
            Ty::Array(e) => Some((*e).clone()),
            // `list.get` answers `Option<T>`, whose payload is the element.
            Ty::Con(id, targs) if self.cx.tables().is_option(id) => targs.first().cloned(),
            _ => None,
        }
    }

    /// `derivePrimShow` and `derivePrimHash`, the two type-directed intrinsics
    /// `middle::derives` leaves for a backend to bottom out.
    ///
    /// Neither carries its type argument by the time it is here — `lower.rs`
    /// keeps the key and drops `targs` — so the primitive is recovered from the
    /// IR type of the operand, which is the same fact by a shorter route.
    fn derived(&mut self, dests: &[ValueId], key: &str, args: &[ValueId]) -> bool {
        let Some(dest) = dests.first().copied() else { return false };
        let Some((name, prim)) = derive_key(key) else { return false };
        match name {
            "derivePrimShow" => {
                let Some(arg) = args.first().copied() else { return false };
                self.show_prim(dest, arg, prim, true);
                true
            }
            "derivePrimHash" => {
                // `(U64, T) -> U64`: the accumulator, then the value.
                let (Some(acc), Some(arg)) = (args.first().copied(), args.get(1).copied()) else {
                    return false;
                };
                let Some(h) = self.get(acc) else { return false };
                self.hash_prim(dest, arg, prim, h);
                true
            }
            _ => false,
        }
    }

    /// One primitive rendered into a fresh `Str`.
    ///
    /// `quoted` is the whole difference between the two callers, and it is a
    /// real difference in only two of the arms: `$str` of a `Str` is the string
    /// and `$show`'s is `JSON.stringify` of it; `$str` of a `Char` is the
    /// character and `$show`'s wraps it in single quotes. Everything numeric
    /// renders the same either way.
    fn show_prim(&mut self, dest: ValueId, arg: ValueId, prim: Prim, quoted: bool) {
        let dty = self.code.ty_of(dest);
        match prim {
            Prim::Str | Prim::Template if !quoted => {
                let l = self.layout(dty);
                let Some(src) = self.get(arg).filter(|_| l.size > 0) else { return };
                let d = self.alloc_slot(dest, dty);
                self.cx.copy(d, src, l.size, l.align);
            }
            Prim::Str | Prim::Template => {
                let Some(src) = self.get(arg) else { return };
                let ptr = self.cx.load_at(PTR, src, word(STR_PTR));
                let raw = self.cx.load_at(types::I64, src, word(STR_LEN));
                let len = self.cx.b.ins().band_imm(raw, STR_LEN_MASK as i64);
                self.out_call(dest, runtime::show::STR_QUOTED, &[ptr, len], &[PTR, types::I64]);
            }
            Prim::Char => {
                let Some(v) = self.get(arg) else { return };
                let symbol = if quoted { runtime::show::CHAR_QUOTED } else { runtime::show::CHAR };
                self.out_call(dest, symbol, &[v], &[types::I32]);
            }
            Prim::Bool => {
                let Some(v) = self.get(arg) else { return };
                let Some(r) = self.cx.helper_ref(Helper::ShowBool, None) else { return };
                let inst = self.cx.b.ins().call(r, &[v]);
                let results = self.cx.b.inst_results(inst).to_vec();
                self.gather(&[dest], &results);
            }
            Prim::F32 => {
                let Some(v) = self.get(arg) else { return };
                self.out_call(dest, runtime::show::F32, &[v], &[types::F32]);
            }
            Prim::F64 => {
                let Some(v) = self.get(arg) else { return };
                self.out_call(dest, runtime::show::F64, &[v], &[types::F64]);
            }
            // A pair of `u64`s, low half first: `lib.rs` §2's first rule says a
            // parameter is a scalar leaf, and a 128-bit value is not one.
            Prim::I128 | Prim::U128 => {
                let Some(v) = self.get(arg) else { return };
                let (lo, hi) = self.cx.b.ins().isplit(v);
                let symbol = if prim == Prim::I128 {
                    runtime::show::I128
                } else {
                    runtime::show::U128
                };
                self.out_call(dest, symbol, &[lo, hi], &[types::I64, types::I64]);
            }
            // Generated rather than called, and this is the one arm where that
            // is worth the code: `buri_rt_str_from_int` would build a Rust
            // `String` and copy it, where `Helper::ShowInt` writes the digits
            // straight into the block it allocates. `str.fromInt` still goes to
            // the runtime, because there the call *is* the operation.
            p if is_integer(p) => {
                let Some(v) = self.get(arg) else { return };
                let wide = self.widen(v, p);
                let key = Helper::ShowInt { signed: signed_prim(p) };
                let Some(r) = self.cx.helper_ref(key, None) else { return };
                let inst = self.cx.b.ins().call(r, &[wide]);
                let results = self.cx.b.inst_results(inst).to_vec();
                self.gather(&[dest], &results);
            }
            _ => self.cx.unit.errors.push(format!(
                "the Cranelift backend cannot render a `{}` yet",
                prim.name()
            )),
        }
    }

    /// A runtime call whose only result is an aggregate through an
    /// out-pointer — the shape every renderer has.
    fn out_call(&mut self, dest: ValueId, symbol: &str, args: &[Value], tys: &[ClifType]) {
        let dty = self.code.ty_of(dest);
        if self.layout(dty).size == 0 {
            return;
        }
        let out = self.alloc_slot(dest, dty);
        let mut vals = args.to_vec();
        vals.push(out);
        let mut params = tys.to_vec();
        params.push(PTR);
        let r = self.cx.rt_ref(symbol, &params, &[]);
        self.cx.b.ins().call(r, &vals);
    }

    /// One primitive mixed into a hash accumulator — `$hashInto`'s three arms
    /// (`cli/runtime/hash.rs`'s table).
    fn hash_prim(&mut self, dest: ValueId, arg: ValueId, prim: Prim, acc: Value) {
        let Some(v) = self.get(arg) else { return };
        let out = match prim {
            Prim::Str | Prim::Template => {
                let base = self.cx.load_at(PTR, v, word(STR_BASE));
                let ptr = self.cx.load_at(PTR, v, word(STR_PTR));
                let raw = self.cx.load_at(types::I64, v, word(STR_LEN));
                let r = self.cx.rt_ref(
                    runtime::hash::STR,
                    &[types::I64, PTR, PTR, types::I64],
                    &[types::I64],
                );
                self.cx.call1(r, &[acc, base, ptr, raw])
            }
            // A `Char` is a one-character *string* on JavaScript, so it takes
            // the string arm and an astral scalar is two mixes.
            Prim::Char => {
                let r = self.cx.rt_ref(runtime::hash::CHAR, &[types::I64, types::I32], &[types::I64]);
                self.cx.call1(r, &[acc, v])
            }
            Prim::F32 | Prim::F64 => {
                let wide = if prim == Prim::F32 {
                    self.cx.b.ins().fpromote(types::F64, v)
                } else {
                    v
                };
                let r = self.cx.rt_ref(runtime::hash::F64, &[types::I64, types::F64], &[types::I64]);
                self.cx.call1(r, &[acc, wide])
            }
            _ => {
                let low = self.low_u32(v);
                let r = self.cx.rt_ref(runtime::hash::MIX, &[types::I64, types::I32], &[types::I64]);
                self.cx.call1(r, &[acc, low])
            }
        };
        self.set(dest, out);
    }

    /// The low 32 bits of an integer of any width — `ToUint32` on a value that
    /// is already an integer, which is what `$mix` is handed.
    fn low_u32(&mut self, v: Value) -> Value {
        let have = self.cx.b.func.dfg.value_type(v);
        if have == types::I32 {
            return v;
        }
        if have.bits() > 32 {
            self.cx.b.ins().ireduce(types::I32, v)
        } else {
            self.cx.b.ins().uextend(types::I32, v)
        }
    }

    /// `num.<T>.<op>`: the numeric surface `core/num` declares without a body.
    ///
    /// Emitted inline rather than called, for the same reason the JavaScript
    /// backend emits it inline (`js/intrinsics.rs`): there is one conversion
    /// per source-and-target pair (SPEC 6.2.1) and generating two instructions
    /// beats calling a runtime function that generates the same two.
    ///
    /// Answers `false` for an operation it does not implement — `checked*`,
    /// `saturating*` and `compare`, which return an `Option` or an `Order` —
    /// so that the caller reports a missing intrinsic instead of emitting a
    /// call to a symbol that does not exist. [`numeric_op`] is the same list,
    /// asked ahead of time.
    fn numeric(&mut self, dests: &[ValueId], key: &str, args: &[ValueId]) -> bool {
        let parts: Vec<&str> = key.split('.').collect();
        let Some(dest) = dests.first().copied() else { return false };
        // `Bounded`'s two methods take no `self`, so `num.minValue<U8>()`
        // reaches its type through the *return* type. `middle::lower`'s
        // `bounded_key` has already moved that type into the key, so what is
        // left in the two-segment form is a `Bounded` at a type that is not a
        // primitive — which no backend implements.
        if parts.first() == Some(&"num") && parts.len() == 2 {
            return false;
        }
        let (Some(&"num"), Some(name), Some(op), 3) =
            (parts.first(), parts.get(1), parts.get(2), parts.len())
        else {
            return false;
        };
        let Some(from) = Prim::all().iter().copied().find(|p| p.name() == *name) else {
            return false;
        };
        // Two operations answer something that is not a register — a `Str` and
        // an `Option<T>` — so they are taken before the register shape is asked
        // for, which would otherwise refuse them.
        if matches!(*op, "minValue" | "maxValue") {
            return self.bounds(dest, from, op);
        }
        if *op == "show" {
            let Some(arg) = args.first().copied() else { return false };
            self.show_prim(dest, arg, from, false);
            return true;
        }
        if let Some(kind) = op.strip_prefix("checked") {
            return self.checked(dest, from, kind, args);
        }
        // `Order` is an enum, and `middle::layout` gives even a payload-less one
        // an aggregate IR type — so `Abi::register` refuses it and `compare`
        // has to be taken before that guard, like `show`.
        if *op == "compare" {
            return self.compare(dest, from, args);
        }
        let dty = self.code.ty_of(dest);
        let Some(want) = Abi::register(dty) else { return false };
        let a = args.first().copied().and_then(|v| self.get(v));
        let b = args.get(1).copied().and_then(|v| self.get(v));
        let float = from.is_float();
        let signed = from.is_signed() || float;

        // A conversion is named by its target: `toI32`, `wrapToU8`, `toChar`.
        // Every one of them is the same instruction sequence — widen, narrow,
        // or cross between integer and float — chosen from the two types.
        if *op == "toChar" {
            let Some(v) = a else { return false };
            let out = self.cast(v, from, types::I32, false);
            self.set(dest, Some(out));
            return true;
        }
        for prefix in ["wrapTo", "to"] {
            let Some(target) = op.strip_prefix(prefix) else { continue };
            let Some(to) = Prim::all().iter().copied().find(|p| p.name() == target) else {
                continue;
            };
            // An inexact `toX` answers a `Result` (SPEC 6.2.1) and is refused
            // here so that `numeric_op` and this function agree — see the note
            // there. Casting anyway would silently drop the failure case.
            if prefix == "to"
                && !crate::compiler::semantics::builtins::conversion_is_exact(from, to)
            {
                return false;
            }
            let Some(v) = a else { return false };
            let out = self.cast(v, from, want, signed);
            self.set(dest, Some(out));
            return true;
        }

        let v = match (*op, a, b) {
            ("add", Some(x), Some(y)) if float => self.cx.b.ins().fadd(x, y),
            ("sub", Some(x), Some(y)) if float => self.cx.b.ins().fsub(x, y),
            ("mul", Some(x), Some(y)) if float => self.cx.b.ins().fmul(x, y),
            ("div", Some(x), Some(y)) if float => self.cx.b.ins().fdiv(x, y),
            ("add", Some(x), Some(y)) => self.cx.b.ins().iadd(x, y),
            ("sub", Some(x), Some(y)) => self.cx.b.ins().isub(x, y),
            ("mul", Some(x), Some(y)) => self.cx.b.ins().imul(x, y),
            ("div", Some(x), Some(y)) => self.divide(BinOp::Div, from, x, y),
            ("rem", Some(x), Some(y)) => self.divide(BinOp::Rem, from, x, y),
            ("neg", Some(x), _) if float => self.cx.b.ins().fneg(x),
            ("neg", Some(x), _) => self.cx.b.ins().ineg(x),
            ("abs", Some(x), _) if float => self.cx.b.ins().fabs(x),
            // `abs` of a signed minimum overflows, and overflow is undefined
            // (SPEC 6.2), so there is nothing to check.
            ("abs", Some(x), _) if signed => {
                let flipped = self.cx.b.ins().ineg(x);
                let neg = self.cx.b.ins().icmp_imm(IntCC::SignedLessThan, x, 0);
                self.cx.b.ins().select(neg, flipped, x)
            }
            ("abs", Some(x), _) => x,
            ("signum", Some(x), _) if float => {
                let zero = self.cx.b.ins().f64const(0.0);
                let zero = if want == types::F32 {
                    self.cx.b.ins().fdemote(types::F32, zero)
                } else {
                    zero
                };
                let one = self.cx.b.ins().f64const(1.0);
                let one =
                    if want == types::F32 { self.cx.b.ins().fdemote(types::F32, one) } else { one };
                let minus = self.cx.b.ins().fneg(one);
                let above = self.cx.b.ins().fcmp(FloatCC::GreaterThan, x, zero);
                let below = self.cx.b.ins().fcmp(FloatCC::LessThan, x, zero);
                let lower = self.cx.b.ins().select(below, minus, zero);
                self.cx.b.ins().select(above, one, lower)
            }
            ("signum", Some(x), _) => {
                let zero = self.cx.iconst(want, 0);
                let one = self.cx.iconst(want, 1);
                let minus = self.cx.iconst(want, -1);
                let gt = if signed { IntCC::SignedGreaterThan } else { IntCC::UnsignedGreaterThan };
                let lt = if signed { IntCC::SignedLessThan } else { IntCC::UnsignedLessThan };
                let above = self.cx.b.ins().icmp(gt, x, zero);
                let below = self.cx.b.ins().icmp(lt, x, zero);
                let lower = self.cx.b.ins().select(below, minus, zero);
                self.cx.b.ins().select(above, one, lower)
            }
            // `Eq` on a float is `===`, which is *unordered*: `NaN != NaN`, and
            // an ordered compare is what says so.
            ("eq", Some(x), Some(y)) if float => self.cx.b.ins().fcmp(FloatCC::Equal, x, y),
            ("eq", Some(x), Some(y)) => self.cx.b.ins().icmp(IntCC::Equal, x, y),
            // `Hash::hash` takes no accumulator, so it is the seeded form
            // (`cli/runtime/hash.rs`).
            ("hash", Some(x), _) => {
                let seed = self.cx.iconst(types::I64, runtime::hash::SEED);
                if float {
                    let wide = if from == Prim::F32 {
                        self.cx.b.ins().fpromote(types::F64, x)
                    } else {
                        x
                    };
                    let r = self.cx.rt_ref(
                        runtime::hash::F64,
                        &[types::I64, types::F64],
                        &[types::I64],
                    );
                    match self.cx.call1(r, &[seed, wide]) {
                        Some(v) => v,
                        None => return false,
                    }
                } else {
                    let low = self.low_u32(x);
                    let r = self.cx.rt_ref(
                        runtime::hash::MIX,
                        &[types::I64, types::I32],
                        &[types::I64],
                    );
                    match self.cx.call1(r, &[seed, low]) {
                        Some(v) => v,
                        None => return false,
                    }
                }
            }
            // Wrapping *is* the machine operation: two's complement at the
            // operand's own width is what `iadd` on an `i8` already does. The
            // JavaScript backend has to say `$wrapTo` because a double does
            // not wrap; here the name is the only difference.
            ("wrappingAdd", Some(x), Some(y)) => self.cx.b.ins().iadd(x, y),
            ("wrappingSub", Some(x), Some(y)) => self.cx.b.ins().isub(x, y),
            ("wrappingMul", Some(x), Some(y)) => self.cx.b.ins().imul(x, y),
            ("saturatingAdd", Some(x), Some(y))
            | ("saturatingSub", Some(x), Some(y))
            | ("saturatingMul", Some(x), Some(y)) => {
                let Some(v) = self.saturating(op, from, want, x, y) else { return false };
                v
            }
            ("min", Some(x), Some(y)) | ("max", Some(x), Some(y)) => {
                let want_min = *op == "min";
                let c = if float {
                    let cc = if want_min { FloatCC::LessThan } else { FloatCC::GreaterThan };
                    self.cx.b.ins().fcmp(cc, x, y)
                } else {
                    let cc = match (want_min, signed) {
                        (true, true) => IntCC::SignedLessThan,
                        (true, false) => IntCC::UnsignedLessThan,
                        (false, true) => IntCC::SignedGreaterThan,
                        (false, false) => IntCC::UnsignedGreaterThan,
                    };
                    self.cx.b.ins().icmp(cc, x, y)
                };
                self.cx.b.ins().select(c, x, y)
            }
            _ => return false,
        };
        self.set(dest, Some(v));
        true
    }

    /// `Eq`, `Ord`, `Hash` and `Show` at `Bool`, `Char` and `Str`.
    ///
    /// `core/num`'s primitives reach these through `num.<T>.<op>`; `Bool`,
    /// `Char` and `Str` get their own impls from `semantics::builtins`, and so
    /// their keys are `bool.eq`, `char.compare`, `str.show` and the rest. Only
    /// the first two modules are here — `str.eq`, `str.compare` and `str.hash`
    /// are runtime entries and are in the table.
    ///
    /// The `Char` arms are the ones with a rule in them. A `Char` is a
    /// one-character *string* on JavaScript, so `<` and `charCodeAt` both speak
    /// UTF-16: ordering and hashing go through the runtime rather than through
    /// the scalar value, and `cli/runtime/hash.rs` says why.
    fn prim_trait(&mut self, dests: &[ValueId], key: &str, args: &[ValueId]) -> bool {
        let Some((module, op)) = key.split_once('.') else { return false };
        let prim = match module {
            "bool" => Prim::Bool,
            "char" => Prim::Char,
            "str" => Prim::Str,
            _ => return false,
        };
        let Some(dest) = dests.first().copied() else { return false };
        match op {
            // `show` is `$str`, not `$show`: the trait method renders the value
            // and the *derived* one quotes it (`js/intrinsics.rs`'s `"show"`
            // arm against `$show`'s `"s"` and `"c"` arms).
            "show" => {
                let Some(arg) = args.first().copied() else { return false };
                self.show_prim(dest, arg, prim, false);
                true
            }
            "eq" if prim != Prim::Str => {
                let (Some(x), Some(y)) = (
                    args.first().copied().and_then(|v| self.get(v)),
                    args.get(1).copied().and_then(|v| self.get(v)),
                ) else {
                    return false;
                };
                let v = self.cx.b.ins().icmp(IntCC::Equal, x, y);
                self.set(dest, Some(v));
                true
            }
            // `false` sorts before `true`, which is what `a < b` says of two
            // booleans on JavaScript and what an unsigned compare says here.
            "compare" if prim == Prim::Bool => self.compare(dest, Prim::U8, args),
            "compare" if prim == Prim::Char => {
                let (Some(x), Some(y)) = (
                    args.first().copied().and_then(|v| self.get(v)),
                    args.get(1).copied().and_then(|v| self.get(v)),
                ) else {
                    return false;
                };
                let r = self.cx.rt_ref(
                    "buri_rt_char_compare",
                    &[types::I32, types::I32],
                    &[types::I32],
                );
                let Some(order) = self.cx.call1(r, &[x, y]) else { return false };
                let Some(tag) = self.tag_ty(dest) else { return false };
                let narrowed = if tag == types::I32 {
                    order
                } else if tag.bits() < 32 {
                    self.cx.b.ins().ireduce(tag, order)
                } else {
                    self.cx.b.ins().uextend(tag, order)
                };
                self.bind_tag(dest, narrowed);
                true
            }
            "hash" if prim != Prim::Str => {
                let Some(x) = args.first().copied().and_then(|v| self.get(v)) else {
                    return false;
                };
                let seed = self.cx.iconst(types::I64, runtime::hash::SEED);
                let out = if prim == Prim::Char {
                    let r = self.cx.rt_ref(
                        runtime::hash::CHAR,
                        &[types::I64, types::I32],
                        &[types::I64],
                    );
                    self.cx.call1(r, &[seed, x])
                } else {
                    let low = self.low_u32(x);
                    let r = self.cx.rt_ref(
                        runtime::hash::MIX,
                        &[types::I64, types::I32],
                        &[types::I64],
                    );
                    self.cx.call1(r, &[seed, low])
                };
                self.set(dest, out);
                true
            }
            // `Char::toU32` is the representation: a `Char` *is* an `i32`
            // holding a Unicode scalar value (`middle/layout.rs` §1).
            "toU32" if prim == Prim::Char => {
                let Some(x) = args.first().copied().and_then(|v| self.get(v)) else {
                    return false;
                };
                self.set(dest, Some(x));
                true
            }
            _ => false,
        }
    }

    /// `core/bits`, open-coded.
    ///
    /// Every one is a single machine instruction behind a range check, which is
    /// why none of them is a runtime call: `$bits_shl` has to route through a
    /// `BigInt` because a double is not 64 bits, and here the operand *is* the
    /// register.
    ///
    /// The range check is the whole of what is not the instruction. `$shiftCount`
    /// aborts on a count that is negative or at or beyond the operand's width
    /// (`runtime.js:923-928`), and `cli/tests/crash/shift_*` pins the message —
    /// so the check is unconditional and the abort is the runtime's shared one.
    /// Cranelift's own shifts mask the count instead, which is a *different*
    /// answer rather than an undefined one, and would be silently wrong.
    fn bits(&mut self, dests: &[ValueId], key: &str, args: &[ValueId]) -> bool {
        let Some(op) = key.strip_prefix("bits.") else { return false };
        let Some(dest) = dests.first().copied() else { return false };
        let Some(x) = args.first().copied().and_then(|v| self.get(v)) else { return false };
        let width = self.cx.b.func.dfg.value_type(x);
        let bits = i64::from(width.bits());

        // The unary three take no count and cannot be out of range. Each
        // answers an `Int`, so the count is widened back to 64 bits.
        let unary = match op {
            "popCount" => Some(self.cx.b.ins().popcnt(x)),
            "leadingZeros" => Some(self.cx.b.ins().clz(x)),
            "trailingZeros" => Some(self.cx.b.ins().ctz(x)),
            _ => None,
        };
        if let Some(v) = unary {
            let wide = if width == types::I64 {
                v
            } else {
                self.cx.b.ins().uextend(types::I64, v)
            };
            self.set(dest, Some(wide));
            return true;
        }

        let Some(count) = args.get(1).copied().and_then(|v| self.get(v)) else { return false };
        // The count arrives as an `Int` — 64 bits — whatever the operand's
        // width is, so the check is at 64 bits and the shift takes the
        // narrowed value.
        let low = self.cx.b.ins().icmp_imm(IntCC::SignedLessThan, count, 0);
        let high = self.cx.b.ins().icmp_imm(IntCC::SignedGreaterThanOrEqual, count, bits);
        let bad = self.cx.b.ins().bor(low, high);
        let out_of_range = self.cx.b.create_block();
        let ok = self.cx.b.create_block();
        self.cx.brif(bad, out_of_range, &[], ok, &[]);
        self.cx.b.switch_to_block(out_of_range);
        self.cx.b.set_cold_block(out_of_range);
        let abort = self.cx.rt_ref("buri_rt_abort_shift", &[], &[]);
        self.cx.b.ins().call(abort, &[]);
        self.cx.b.ins().trap(UNREACHABLE);
        self.cx.b.switch_to_block(ok);

        let v = match op {
            // `shl` at every width, and `shr` at an unsigned one, are the plain
            // machine shifts: `ushr` is the logical one and `sshr` the
            // arithmetic one, which is exactly the `shr`/`sar` split
            // `core/bits` draws at `Int`.
            "shl" | "shlU8" | "shlU32" | "shlU64" => self.cx.b.ins().ishl(x, count),
            "shr" | "shrU8" | "shrU32" | "shrU64" => self.cx.b.ins().ushr(x, count),
            "sar" => self.cx.b.ins().sshr(x, count),
            "rotateLeft" => self.cx.b.ins().rotl(x, count),
            "rotateRight" => self.cx.b.ins().rotr(x, count),
            _ => return false,
        };
        self.set(dest, Some(v));
        true
    }

    /// `Bounded::minValue` and `Bounded::maxValue`.
    ///
    /// The type comes from the key — `middle::lower`'s `bounded_key` puts it
    /// there, because `Bounded`'s methods take no argument and the IR type of
    /// the result has lost the signedness that separates `0` from `-128`.
    ///
    /// The bounds are the **type's**, not JavaScript's exactly-representable
    /// ones: `js/intrinsics.rs` uses `int_range` here too. `exact_int_range` is
    /// the JavaScript backend's business alone — it is the range a double still
    /// names, and natively nothing has to survive being one.
    fn bounds(&mut self, dest: ValueId, prim: Prim, op: &str) -> bool {
        if !matches!(op, "minValue" | "maxValue") {
            return false;
        }
        let dty = self.code.ty_of(dest);
        let Some(want) = Abi::register(dty) else { return false };
        let low = op == "minValue";
        if prim.is_float() {
            // `F32`'s bounds are `f32::MAX` at both signs and `F64`'s are
            // `f64::MIN`/`f64::MAX`, which is the same statement: the largest
            // finite magnitude, signed. Not the smallest *positive* one — that
            // is `MIN_POSITIVE`, and `Bounded` is about the range.
            let v = match (prim, low) {
                (Prim::F32, true) => f64::from(-f32::MAX),
                (Prim::F32, false) => f64::from(f32::MAX),
                (_, true) => f64::MIN,
                (_, false) => f64::MAX,
            };
            let c = self.cx.b.ins().f64const(v);
            let c = if want == types::F32 { self.cx.b.ins().fdemote(types::F32, c) } else { c };
            self.set(dest, Some(c));
            return true;
        }
        let Some((lo, hi)) = prim.int_range() else { return false };
        // The bit pattern, not the number: `u64::MAX` is not an `i64`, and an
        // `iconst` takes the pattern the width holds.
        let bits = if low { lo as u128 } else { hi };
        let c = if want == types::I128 {
            let lo64 = self.cx.iconst(types::I64, bits as u64 as i64);
            let hi64 = self.cx.iconst(types::I64, (bits >> 64) as u64 as i64);
            self.cx.b.ins().iconcat(lo64, hi64)
        } else {
            self.cx.iconst(want, bits as u64 as i64)
        };
        self.set(dest, Some(c));
        true
    }

    /// `Ord::compare`, answering an `Order` tag.
    ///
    /// `Less = 0`, `Equal = 1`, `Greater = 2` in declaration order
    /// (`order.buri:10-14`), and `$cmp`'s `a < b ? 0 : a > b ? 2 : 1` puts an
    /// unordered pair — a `NaN` on either side — at `Equal`. Two *ordered*
    /// compares reproduce that exactly, and an unordered one would not.
    fn compare(&mut self, dest: ValueId, from: Prim, args: &[ValueId]) -> bool {
        let (Some(x), Some(y)) = (
            args.first().copied().and_then(|v| self.get(v)),
            args.get(1).copied().and_then(|v| self.get(v)),
        ) else {
            return false;
        };
        let Some(tag) = self.tag_ty(dest) else { return false };
        let (lt, gt) = if from.is_float() {
            (
                self.cx.b.ins().fcmp(FloatCC::LessThan, x, y),
                self.cx.b.ins().fcmp(FloatCC::GreaterThan, x, y),
            )
        } else if from.is_signed() {
            (
                self.cx.b.ins().icmp(IntCC::SignedLessThan, x, y),
                self.cx.b.ins().icmp(IntCC::SignedGreaterThan, x, y),
            )
        } else {
            (
                self.cx.b.ins().icmp(IntCC::UnsignedLessThan, x, y),
                self.cx.b.ins().icmp(IntCC::UnsignedGreaterThan, x, y),
            )
        };
        let less = self.cx.iconst(tag, 0);
        let equal = self.cx.iconst(tag, 1);
        let greater = self.cx.iconst(tag, 2);
        let upper = self.cx.b.ins().select(gt, greater, equal);
        let v = self.cx.b.ins().select(lt, less, upper);
        self.bind_tag(dest, v);
        true
    }

    /// The machine type of a payload-less enum's tag.
    ///
    /// `Order` occupies one byte and `middle::layout` still calls it an
    /// aggregate, so `Abi::register` refuses it and the answer is its single
    /// leaf instead.
    fn tag_ty(&mut self, dest: ValueId) -> Option<ClifType> {
        let dty = self.code.ty_of(dest);
        match Abi::register(dty) {
            Some(t) => Some(t),
            None => self.leaves(dty).first().map(|l| l.ty),
        }
    }

    /// Bind a computed tag to a destination, as a value or as a store.
    ///
    /// The distinction is not cosmetic: an aggregate destination is *addressed*
    /// by everything downstream — `GetTag` loads from it — so binding the tag as
    /// a value would leave a one-byte integer being used as a pointer, which is
    /// a verifier error rather than a wrong answer, but only because Cranelift
    /// happens to check.
    fn bind_tag(&mut self, dest: ValueId, v: Value) {
        let dty = self.code.ty_of(dest);
        match Abi::register(dty) {
            Some(_) => self.set(dest, Some(v)),
            None => {
                let slot = self.alloc_slot(dest, dty);
                self.cx.store_at(slot, 0, v);
            }
        }
    }

    /// `checkedAdd`, `checkedSub`, `checkedMul`, `checkedDiv` — an `Option<T>`.
    ///
    /// The bound checked is the **type's own range**, which is where this
    /// deliberately parts company with the JavaScript backend: there,
    /// `$checkedIn` tests `exact_int_range`, because past `2^53` a double can no
    /// longer say which integer it is, so `.None` is the only honest answer it
    /// has. Natively an `i64` addition either overflows or does not, and
    /// reporting `.None` for a value the machine represents exactly would be
    /// inventing a failure. `Checked` is bounded by the numbers the *platform*
    /// has, and `.Some(v)` promises that `v` is the true result as this backend
    /// represents numbers — which is a promise both backends keep, at different
    /// widths (SPEC 6.2.2, `design/native/VALUE-MODEL.md` §12 row 2).
    ///
    /// The two agree on every operand inside `±2^53` and on every overflow of
    /// the type itself; the band between them is a documented divergence, and
    /// `cli/tests/native/agreement.rs`'s row 2 pins both answers. The shared
    /// conformance corpus deliberately stays out of that band.
    ///
    /// `Div` is where "the type's own range" is not the same statement as "the
    /// machine did not wrap": `MIN / -1` is `2^63`, which the width cannot
    /// hold, so [`Lower::overflowing`] reports it alongside a zero divisor.
    ///
    /// 128-bit goes to the runtime, because the overflow test below is
    /// `smulhi`/`umulhi` and Cranelift does not define either at `i128`
    /// ([`Lower::wide_checked`]).
    fn checked(&mut self, dest: ValueId, from: Prim, kind: &str, args: &[ValueId]) -> bool {
        if !from.is_integer() {
            return false;
        }
        if from.bits() == 128 {
            return self.wide_checked(dest, from, kind, args);
        }
        let (Some(x), Some(y)) = (
            args.first().copied().and_then(|v| self.get(v)),
            args.get(1).copied().and_then(|v| self.get(v)),
        ) else {
            return false;
        };
        let signed = from.is_signed();
        let width = self.cx.b.func.dfg.value_type(x);
        let Some((value, overflowed)) = self.overflowing(kind, signed, width, x, y) else {
            return false;
        };
        self.build_option(dest, value, overflowed);
        true
    }

    /// `(result, did it overflow)` for one of the four checked operations.
    ///
    /// Every test is the textbook one, and each is chosen because it needs no
    /// wider type than the operands: a wider one does not exist at 64 bits.
    fn overflowing(
        &mut self,
        kind: &str,
        signed: bool,
        width: ClifType,
        x: Value,
        y: Value,
    ) -> Option<(Value, Value)> {
        let top = i64::from(width.bits().saturating_sub(1));
        match kind {
            "Add" => {
                let r = self.cx.b.ins().iadd(x, y);
                let bad = if signed {
                    // Overflow iff both operands differ in sign from the sum.
                    let a = self.cx.b.ins().bxor(x, r);
                    let b = self.cx.b.ins().bxor(y, r);
                    let both = self.cx.b.ins().band(a, b);
                    self.cx.b.ins().icmp_imm(IntCC::SignedLessThan, both, 0)
                } else {
                    self.cx.b.ins().icmp(IntCC::UnsignedLessThan, r, x)
                };
                Some((r, bad))
            }
            "Sub" => {
                let r = self.cx.b.ins().isub(x, y);
                let bad = if signed {
                    let a = self.cx.b.ins().bxor(x, y);
                    let b = self.cx.b.ins().bxor(x, r);
                    let both = self.cx.b.ins().band(a, b);
                    self.cx.b.ins().icmp_imm(IntCC::SignedLessThan, both, 0)
                } else {
                    self.cx.b.ins().icmp(IntCC::UnsignedLessThan, x, y)
                };
                Some((r, bad))
            }
            "Mul" => {
                let r = self.cx.b.ins().imul(x, y);
                let bad = if signed {
                    // The high half of the exact product must be the sign
                    // extension of the low half, or bits were lost.
                    let hi = self.cx.b.ins().smulhi(x, y);
                    let sign = self.cx.b.ins().sshr_imm(r, top);
                    self.cx.b.ins().icmp(IntCC::NotEqual, hi, sign)
                } else {
                    let hi = self.cx.b.ins().umulhi(x, y);
                    self.cx.b.ins().icmp_imm(IntCC::NotEqual, hi, 0)
                };
                Some((r, bad))
            }
            "Div" => {
                // Two ways to fail, and the divide must not be reached on
                // either: a zero divisor traps, and `MIN / -1` is the one
                // signed quotient with no representation. The divisor is
                // replaced with `1` on both, so the instruction is always
                // well-defined and the answer is discarded.
                let zero = self.cx.b.ins().icmp_imm(IntCC::Equal, y, 0);
                let bad = if signed {
                    // The type's minimum, as the bit pattern the width holds:
                    // all-ones above the sign bit, which `iconst` masks down.
                    let min = self.cx.iconst(width, i64::MIN >> (63i64.saturating_sub(top)));
                    let at_min = self.cx.b.ins().icmp(IntCC::Equal, x, min);
                    let minus_one = self.cx.b.ins().icmp_imm(IntCC::Equal, y, -1);
                    let both = self.cx.b.ins().band(at_min, minus_one);
                    self.cx.b.ins().bor(zero, both)
                } else {
                    zero
                };
                let one = self.cx.iconst(width, 1);
                let safe = self.cx.b.ins().select(bad, one, y);
                let r = if signed {
                    self.cx.b.ins().sdiv(x, safe)
                } else {
                    self.cx.b.ins().udiv(x, safe)
                };
                Some((r, bad))
            }
            _ => None,
        }
    }

    /// `saturatingAdd`, `saturatingSub`, `saturatingMul`.
    ///
    /// `$sat(v, lo, hi)` clamps an exact double; here the overflow is detected
    /// and the *end* it ran off is chosen, which is the same answer without a
    /// wider type. Which end is a property of the operands' signs, not of the
    /// wrapped result — the wrapped result is precisely the thing that is
    /// wrong.
    fn saturating(
        &mut self,
        op: &str,
        from: Prim,
        want: ClifType,
        x: Value,
        y: Value,
    ) -> Option<Value> {
        if !from.is_integer() {
            return None;
        }
        if from.bits() == 128 {
            let kind = op.strip_prefix("saturating")?;
            return self.wide_saturating(from, kind, x, y);
        }
        let signed = from.is_signed();
        let kind = op.strip_prefix("saturating")?;
        let (r, bad) = self.overflowing(kind, signed, want, x, y)?;
        let (lo, hi) = from.int_range()?;
        let low = self.cx.iconst(want, lo as u64 as i64);
        let high = self.cx.iconst(want, hi as u64 as i64);
        let end = if !signed {
            // Unsigned: an addition or a multiplication can only run off the
            // top, and a subtraction only off the bottom.
            if kind == "Sub" {
                low
            } else {
                high
            }
        } else {
            let negative = match kind {
                // The sum of two operands that overflowed is negative exactly
                // when they were both positive, so the sign of `x` decides.
                "Add" | "Sub" => self.cx.b.ins().icmp_imm(IntCC::SignedLessThan, x, 0),
                _ => {
                    let xn = self.cx.b.ins().icmp_imm(IntCC::SignedLessThan, x, 0);
                    let yn = self.cx.b.ins().icmp_imm(IntCC::SignedLessThan, y, 0);
                    self.cx.b.ins().bxor(xn, yn)
                }
            };
            // For `Sub` the same test is right for the same reason: an
            // overflowing `x - y` runs off the bottom exactly when `x` is
            // negative, because the only way to underflow is a negative `x`
            // against a positive `y`.
            self.cx.b.ins().select(negative, low, high)
        };
        Some(self.cx.b.ins().select(bad, end, r))
    }

    /// The 128-bit forms of `checked*`, through `buri_rt_i128_checked`.
    ///
    /// One call for four operations, selected by an immediate, rather than four
    /// symbols: the runtime body is a `match` either way, and one entry is one
    /// row in the contract instead of four.
    fn wide_checked(&mut self, dest: ValueId, from: Prim, kind: &str, args: &[ValueId]) -> bool {
        let Some(op) = wide_op(kind) else { return false };
        let (Some(x), Some(y)) = (
            args.first().copied().and_then(|v| self.get(v)),
            args.get(1).copied().and_then(|v| self.get(v)),
        ) else {
            return false;
        };
        let dty = self.code.ty_of(dest);
        let l = self.layout(dty);
        let slot = self.alloc_slot(dest, dty);
        let payload_at = match &l.repr {
            Repr::Enum { repr: EnumRepr::Niche { .. }, .. } => 0,
            Repr::Enum { variants, .. } => {
                variants.first().and_then(|v| v.first().copied()).unwrap_or(0)
            }
            _ => 0,
        };
        let out = self.cx.offset(slot, payload_at);
        let (a_lo, a_hi) = self.cx.b.ins().isplit(x);
        let (b_lo, b_hi) = self.cx.b.ins().isplit(y);
        let code = self.cx.iconst(types::I8, i64::from(op));
        let signed = self.cx.iconst(types::I8, i64::from(from.is_signed()));
        let r = self.cx.rt_ref(
            "buri_rt_i128_checked",
            &[types::I8, types::I64, types::I64, types::I64, types::I64, types::I8, PTR],
            &[types::I32],
        );
        let Some(disc) = self.cx.call1(r, &[code, a_lo, a_hi, b_lo, b_hi, signed, out]) else {
            return false;
        };
        let bad = self.cx.b.ins().icmp_imm(IntCC::NotEqual, disc, i64::from(runtime::BURI_OK));
        // The payload is already where it belongs, so all that is left is the
        // discriminant — which is what `build_option` writes. Passing the
        // payload back through it would store it a second time, so a zero of
        // the tag's own width stands in and is overwritten by nothing.
        self.write_option_tag(slot, &l, bad);
        true
    }

    /// The 128-bit forms of `saturating*`, through `buri_rt_i128_saturating`.
    fn wide_saturating(&mut self, from: Prim, kind: &str, x: Value, y: Value) -> Option<Value> {
        let op = wide_op(kind)?;
        // Division cannot saturate, and the runtime entry has no arm for it.
        if op == 3 {
            return None;
        }
        let slot = self.cx.slot(16, 16);
        let (a_lo, a_hi) = self.cx.b.ins().isplit(x);
        let (b_lo, b_hi) = self.cx.b.ins().isplit(y);
        let code = self.cx.iconst(types::I8, i64::from(op));
        let signed = self.cx.iconst(types::I8, i64::from(from.is_signed()));
        let r = self.cx.rt_ref(
            "buri_rt_i128_saturating",
            &[types::I8, types::I64, types::I64, types::I64, types::I64, types::I8, PTR],
            &[],
        );
        self.cx.b.ins().call(r, &[code, a_lo, a_hi, b_lo, b_hi, signed, slot]);
        let lo = self.cx.load_at(types::I64, slot, 0);
        let hi = self.cx.load_at(types::I64, slot, 8);
        Some(self.cx.b.ins().iconcat(lo, hi))
    }

    /// The discriminant half of an `Option<T>` whose payload is already in
    /// place: a tag, or a null pointer in the niche.
    fn write_option_tag(&mut self, slot: Value, l: &layout::Layout, bad: Value) {
        let some = self.cx.b.create_block();
        let none = self.cx.b.create_block();
        let done = self.cx.b.create_block();
        self.cx.brif(bad, none, &[], some, &[]);
        for (block, index) in [(some, 0i64), (none, 1i64)] {
            self.cx.b.switch_to_block(block);
            match &l.repr {
                Repr::Enum { repr: EnumRepr::Bare { tag }, .. }
                | Repr::Enum { repr: EnumRepr::Tagged { tag, .. }, .. } => {
                    let t = scalar_clif(*tag);
                    let v = self.cx.iconst(t, index);
                    self.cx.store_at(slot, 0, v);
                }
                Repr::Enum { repr: EnumRepr::Niche { null_at }, .. } if index == 1 => {
                    let null = self.cx.iconst(PTR, 0);
                    let at = *null_at;
                    self.cx.store_at(slot, at, null);
                }
                _ => {}
            }
            self.cx.jump(done, &[]);
        }
        self.cx.b.switch_to_block(done);
    }

    /// Build an `Option<T>` in the destination from a scalar payload and a
    /// "this failed" flag — the inline counterpart of [`Lower::option_call`].
    fn build_option(&mut self, dest: ValueId, value: Value, bad: Value) {
        let dty = self.code.ty_of(dest);
        let l = self.layout(dty);
        let slot = self.alloc_slot(dest, dty);
        let payload_at = match &l.repr {
            Repr::Enum { repr: EnumRepr::Niche { .. }, .. } => 0,
            Repr::Enum { variants, .. } => {
                variants.first().and_then(|v| v.first().copied()).unwrap_or(0)
            }
            _ => 0,
        };
        let some = self.cx.b.create_block();
        let none = self.cx.b.create_block();
        let done = self.cx.b.create_block();
        self.cx.brif(bad, none, &[], some, &[]);

        self.cx.b.switch_to_block(some);
        self.cx.store_at(slot, payload_at, value);
        match &l.repr {
            Repr::Enum { repr: EnumRepr::Bare { tag }, .. }
            | Repr::Enum { repr: EnumRepr::Tagged { tag, .. }, .. } => {
                let t = scalar_clif(*tag);
                let zero = self.cx.iconst(t, 0);
                self.cx.store_at(slot, 0, zero);
            }
            _ => {}
        }
        self.cx.jump(done, &[]);

        self.cx.b.switch_to_block(none);
        match &l.repr {
            Repr::Enum { repr: EnumRepr::Bare { tag }, .. }
            | Repr::Enum { repr: EnumRepr::Tagged { tag, .. }, .. } => {
                let t = scalar_clif(*tag);
                let one = self.cx.iconst(t, 1);
                self.cx.store_at(slot, 0, one);
            }
            Repr::Enum { repr: EnumRepr::Niche { null_at }, .. } => {
                let null = self.cx.iconst(PTR, 0);
                let at = *null_at;
                self.cx.store_at(slot, at, null);
            }
            _ => {}
        }
        self.cx.jump(done, &[]);
        self.cx.b.switch_to_block(done);
    }

    /// `==`, `!=` and the four orderings at `Str`.
    ///
    /// Equality is bytes, which needs no decoding — identical strings have
    /// identical UTF-8. Ordering is `buri_rt_str_compare`, which is UTF-16
    /// code-unit order because `$str_compare` is JavaScript's `<`
    /// (`cli/runtime/text.rs`); the `Order` tag it answers is turned back into
    /// the boolean the operator wants, since `Less = 0`, `Equal = 1`,
    /// `Greater = 2`.
    fn string_binary(&mut self, dest: ValueId, op: BinOp, x: Value, y: Value) {
        let (xb, xp, xl) = self.str_parts(x);
        let (yb, yp, yl) = self.str_parts(y);
        let args = [xb, xp, xl, yb, yp, yl];
        let params = [PTR, PTR, types::I64, PTR, PTR, types::I64];
        if matches!(op, BinOp::Eq | BinOp::Ne) {
            let f = self.cx.rt_ref("buri_rt_str_eq", &params, &[types::I8]);
            let Some(same) = self.cx.call1(f, &args) else { return };
            let v = if op == BinOp::Eq {
                same
            } else {
                self.cx.b.ins().icmp_imm(IntCC::Equal, same, 0)
            };
            self.set(dest, Some(v));
            return;
        }
        let f = self.cx.rt_ref("buri_rt_str_compare", &params, &[types::I32]);
        let Some(order) = self.cx.call1(f, &args) else { return };
        let cc = match op {
            BinOp::Lt => IntCC::SignedLessThan,
            BinOp::Le => IntCC::SignedLessThanOrEqual,
            BinOp::Gt => IntCC::SignedGreaterThan,
            BinOp::Ge => IntCC::SignedGreaterThanOrEqual,
            // Arithmetic on a `Str` does not exist: `+` on two strings is
            // `str.concat` and the front end says so, so anything else here is
            // a middle-end bug rather than a program error.
            _ => {
                self.cx
                    .unit
                    .errors
                    .push(format!("internal error: `{op:?}` on a `Str`"));
                return;
            }
        };
        // Against `Equal`, which is the middle tag: `a < b` is `order < 1`.
        let v = self.cx.b.ins().icmp_imm(cc, order, 1);
        self.set(dest, Some(v));
    }

    /// The three words a borrowed `Str` argument crosses the C boundary as
    /// (`cli/runtime/lib.rs` §2 rule 1), from the address of its slot.
    fn str_parts(&mut self, addr: Value) -> (Value, Value, Value) {
        let base = self.cx.load_at(PTR, addr, word(STR_BASE));
        let ptr = self.cx.load_at(PTR, addr, word(STR_PTR));
        let len = self.cx.load_at(types::I64, addr, word(STR_LEN));
        (base, ptr, len)
    }

    /// One numeric value at one register shape, at another.
    ///
    /// Widening takes its signedness from the *source*, which is the whole of
    /// the rule: `U8` to `I64` is a zero extension and `I8` to `I64` is a sign
    /// extension, and getting that backwards is the classic conversion bug.
    /// Float-to-integer saturates rather than trapping, because a trap here
    /// would be a run-time failure the language does not have.
    fn cast(&mut self, v: Value, from: Prim, want: ClifType, signed: bool) -> Value {
        let have = self.cx.b.func.dfg.value_type(v);
        if have == want {
            return v;
        }
        match (have.is_float(), want.is_float()) {
            (true, true) => {
                if want.bits() > have.bits() {
                    self.cx.b.ins().fpromote(want, v)
                } else {
                    self.cx.b.ins().fdemote(want, v)
                }
            }
            (true, false) => {
                if signed {
                    self.cx.b.ins().fcvt_to_sint_sat(want, v)
                } else {
                    self.cx.b.ins().fcvt_to_uint_sat(want, v)
                }
            }
            (false, true) => {
                if from.is_signed() {
                    self.cx.b.ins().fcvt_from_sint(want, v)
                } else {
                    self.cx.b.ins().fcvt_from_uint(want, v)
                }
            }
            (false, false) => {
                if want.bits() > have.bits() {
                    if from.is_signed() {
                        self.cx.b.ins().sextend(want, v)
                    } else {
                        self.cx.b.ins().uextend(want, v)
                    }
                } else {
                    self.cx.b.ins().ireduce(want, v)
                }
            }
        }
    }

    /// The handful of `str.*` and `list.*` entries that are a load or a copy.
    ///
    /// Each is here because calling a runtime function for it would be a call
    /// to fetch a word this backend already has the address of. The rest of
    /// that surface — `split`, `replace`, `toUpper`, every `list` producer — is
    /// a later wave's work in `cli/runtime`, and
    /// [`super::Cranelift::missing_intrinsics`] is what says so up front.
    fn open_coded(&mut self, dests: &[ValueId], key: &str, args: &[ValueId]) -> bool {
        let Some(dest) = dests.first().copied() else { return false };
        match key {
            // `list.len()` is the element count, exactly, and always O(1)
            // (VALUE-MODEL.md §4).
            "list.len" => {
                let Some(v) = args.first().copied() else { return false };
                self.array_len(dest, v);
                true
            }
            // `str.len()` is the number of Unicode *scalars* (§3.1). Bit 63 of
            // the stored length answers what that costs: set means every byte
            // is below 0x80, so the count is the byte count and this is a
            // mask; clear means the runtime counts continuation bytes.
            "str.len" => {
                let Some(a) = args.first().copied().and_then(|v| self.get(v)) else {
                    return false;
                };
                let ptr = self.cx.load_at(PTR, a, word(STR_PTR));
                let raw = self.cx.load_at(types::I64, a, word(STR_LEN));
                let bytes = self.cx.b.ins().band_imm(raw, STR_LEN_MASK as i64);
                let ascii = self.cx.b.ins().band_imm(raw, STR_ASCII_FLAG as i64);
                let fast = self.cx.b.create_block();
                let slow = self.cx.b.create_block();
                let done = self.cx.b.create_block();
                self.cx.b.append_block_param(done, types::I64);
                let is_ascii = self.cx.b.ins().icmp_imm(IntCC::NotEqual, ascii, 0);
                self.cx.brif(is_ascii, fast, &[], slow, &[]);
                self.cx.b.switch_to_block(fast);
                self.cx.jump(done, &[bytes]);
                self.cx.b.switch_to_block(slow);
                let f = self.cx.rt_ref(
                    "buri_rt_str_scalar_len",
                    &[PTR, types::I64],
                    &[types::I64],
                );
                let n = self.cx.call1(f, &[ptr, bytes]).unwrap_or(bytes);
                self.cx.jump(done, &[n]);
                self.cx.b.switch_to_block(done);
                let out = self.cx.b.block_params(done).first().copied().unwrap_or(bytes);
                self.set(dest, Some(out));
                true
            }
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
                let Some(arg) = args.last().copied() else { return false };
                let Some(src) = self.get(arg) else { return false };
                let dty = self.code.ty_of(dest);
                let l = self.layout(dty);
                if l.size == 0 {
                    return false;
                }
                let d = self.alloc_slot(dest, dty);
                self.cx.copy(d, src, l.size, l.align);
                self.rc(arg, true);
                true
            }
            // `core/testing/context`'s allocator, and only its allocator.
            //
            // `TestAlloc` is `struct TestAlloc(I64)` holding a handle into the
            // JavaScript runtime's side table (`$handle`, `runtime.js:1452`),
            // and natively there is no side table — but there is also nothing
            // in it: `$testing_context_TestAlloc_allocate` returns the byte
            // count it was given, exactly as `buri_rt_host_alloc_allocate`
            // does. So the value is never read and a zero stands for it.
            //
            // The *stateful* halves of the testing context — `captureOut`,
            // `MemFs`, `TestClock`, `TestEnv`, `TestStdin`, `TestRand` — are
            // deliberately not here. Each of them is a mutable object the
            // handle table exists for, and giving them a native counterpart is
            // its own wave; `missing_intrinsics` names every one.
            "testing_context.alloc" => {
                let dty = self.code.ty_of(dest);
                if self.layout(dty).size == 0 {
                    return true;
                }
                let slot = self.alloc_slot(dest, dty);
                let zero = self.cx.iconst(types::I64, 0);
                self.cx.store_at(slot, 0, zero);
                true
            }
            "testing_context.TestAlloc.allocate" => {
                let Some(bytes) = args.get(1).copied().and_then(|v| self.get(v)) else {
                    return false;
                };
                self.set(dest, Some(bytes));
                true
            }
            // `core/testing/assert`'s three bodies. They are the *runner*'s on
            // JavaScript — `$testing_assert_report` throws and `buri test`
            // catches it (`runtime.js:1643-1658`) — and a native test binary
            // has no runner, so a failure ends the process. That is a real
            // difference in behaviour and it is stated in
            // `cli/runtime/abort.rs`: one process runs every `test` block in
            // order, and the first failure is the last thing it does.
            //
            // `actual` and `expected` are ignored, at every type: rendering
            // them is what a runner does with the values it caught, and this
            // has nowhere to put them. The kind is what makes a failing run
            // attributable.
            "testing_assert.report" => {
                let (Some(passed), Some(kind)) = (args.first().copied(), args.get(1).copied())
                else {
                    return false;
                };
                let Some(cond) = self.get(passed) else { return false };
                let ok = self.cx.b.create_block();
                let bad = self.cx.b.create_block();
                self.cx.brif(cond, ok, &[], bad, &[]);
                self.cx.b.switch_to_block(bad);
                self.abort_assert(kind);
                self.cx.jump(ok, &[]);
                self.cx.b.switch_to_block(ok);
                true
            }
            "testing_assert.failWith" => {
                let Some(message) = args.first().copied() else { return false };
                self.abort_str(message, runtime::ABORT);
                true
            }
            // `failExpected<T, R>(kind, got): R` answers the bottom type. The
            // destination is bound to zeros and is unreachable: the call above
            // it does not return, and Cranelift has no way to be told so
            // (§3.7), so the alternative is a terminator in the middle of a
            // block that the IR says continues.
            "testing_assert.failExpected" => {
                let Some(kind) = args.first().copied() else { return false };
                self.abort_assert(kind);
                let dty = self.code.ty_of(dest);
                match Abi::register(dty) {
                    Some(t) => {
                        let zero = if t.is_float() {
                            let z = self.cx.b.ins().f64const(0.0);
                            if t == types::F32 {
                                self.cx.b.ins().fdemote(types::F32, z)
                            } else {
                                z
                            }
                        } else {
                            self.cx.iconst(t, 0)
                        };
                        self.set(dest, Some(zero));
                    }
                    None => {
                        if self.layout(dty).size > 0 {
                            self.alloc_slot(dest, dty);
                        }
                    }
                }
                true
            }
            // `list.empty()` is two immediates: a null block pointer and a
            // zero count (VALUE-MODEL.md §4). A runtime call for it would be a
            // call that allocates nothing and returns two constants.
            "list.empty" => {
                let dty = self.code.ty_of(dest);
                if self.layout(dty).size == 0 {
                    return true;
                }
                let slot = self.alloc_slot(dest, dty);
                let null = self.cx.iconst(PTR, 0);
                let zero = self.cx.iconst(types::I64, 0);
                self.cx.store_at(slot, word(LIST_PTR), null);
                self.cx.store_at(slot, word(LIST_LEN), zero);
                true
            }
            // `str.concat` is generated rather than called: there is no
            // `buri_rt_*` entry for it, and the sequence is one allocation and
            // two copies (`helpers.rs`).
            "str.concat" => {
                let mut vals = Vec::new();
                for a in args {
                    self.spread(*a, &mut vals);
                }
                let Some(r) = self.cx.helper_ref(Helper::Concat, None) else { return false };
                let inst = self.cx.b.ins().call(r, &vals);
                let results = self.cx.b.inst_results(inst).to_vec();
                self.gather(dests, &results);
                true
            }
            _ => false,
        }
    }

    /// `buri_rt_abort_assert(kind)`, from a `Str` argument.
    fn abort_assert(&mut self, kind: ValueId) {
        self.abort_str(kind, "buri_rt_abort_assert");
    }

    /// Call a `(ptr, len) -> !` runtime entry with a borrowed `Str`'s bytes.
    ///
    /// No trap after it. `buri_rt_abort` never returns, and Cranelift has no
    /// `noreturn` attribute to be told that (CODEGEN-CRANELIFT.md §3.7) — but a
    /// trap here would terminate a block the IR says continues, which is a
    /// verifier error. A call that happens not to return is valid CLIF; the
    /// code after it is dead at run time and well-formed at compile time.
    fn abort_str(&mut self, message: ValueId, symbol: &str) {
        let Some(addr) = self.get(message) else { return };
        let ptr = self.cx.load_at(PTR, addr, word(STR_PTR));
        let raw = self.cx.load_at(types::I64, addr, word(STR_LEN));
        let len = self.cx.b.ins().band_imm(raw, STR_LEN_MASK as i64);
        let r = self.cx.rt_ref(symbol, &[PTR, types::I64], &[]);
        self.cx.b.ins().call(r, &[ptr, len]);
    }

    /// A structural operation the derive pass left behind.
    ///
    /// `middle::derives` replaces every one it can with a call to a function it
    /// generated. What reaches here is a **template hole**, which
    /// `middle::lower` emits directly and after that pass has run. So the cases
    /// below are exactly the primitive arms of `derivePrimShow`
    /// (`middle/derives.rs`'s header), and anything else is a gap named rather
    /// than miscompiled.
    fn structural(&mut self, dest: ValueId, op: StructuralOp, ty: mir::TypeId, args: &[ValueId]) {
        let source = self.program().type_info(ty).ty.clone();
        let name = self.program().type_info(ty).name.clone();
        if !matches!(op, StructuralOp::Show) {
            self.cx.unit.errors.push(format!(
                "the Cranelift backend cannot compile a derived `{op:?}` on `{name}` yet"
            ));
            return;
        }
        let Some(arg) = args.first().copied() else { return };
        let Some(prim) = self.cx.tables().as_prim(&source) else {
            self.cx.unit.errors.push(format!(
                "the Cranelift backend cannot render a `{name}` in a template yet"
            ));
            return;
        };
        // A template hole is `$str` and not `$show` (`runtime.js:72-82` against
        // `runtime.js:200-210`): a `Str` renders as itself and a `Char` without
        // quotes. That is the whole of what `quoted = false` selects, and the
        // numeric arms are the same either way.
        self.show_prim(dest, arg, prim, false);
    }

    /// One integer at its own width, at `i64`, by the *source*'s signedness.
    ///
    /// `U8` to `I64` is a zero extension and `I8` to `I64` is a sign
    /// extension; getting that backwards is the classic conversion bug, and it
    /// would present as `255` printing as `-1`.
    fn widen(&mut self, v: Value, p: Prim) -> Value {
        let t = self.cx.b.func.dfg.value_type(v);
        if t == types::I64 || t == types::I128 {
            return v;
        }
        if signed_prim(p) {
            self.cx.b.ins().sextend(types::I64, v)
        } else {
            self.cx.b.ins().uextend(types::I64, v)
        }
    }

    // -- terminators --------------------------------------------------------

    fn edge_args(&mut self, t: &Target) -> Vec<Value> {
        let mut out = Vec::new();
        for a in &t.args {
            self.spread(*a, &mut out);
        }
        out
    }

    fn block_of(&self, t: &Target) -> Option<Block> {
        self.blocks.get(t.block.index()).copied()
    }

    fn term(&mut self, term: &Term) {
        if self.filled {
            return;
        }
        match term {
            Term::Jump(t) => {
                let args = self.edge_args(t);
                if let Some(dest) = self.block_of(t) {
                    self.cx.jump(dest, &args);
                }
            }
            Term::Branch { cond, then, else_ } => {
                let Some(c) = self.get(*cond) else { return };
                let a = self.edge_args(then);
                let e = self.edge_args(else_);
                if let (Some(x), Some(y)) = (self.block_of(then), self.block_of(else_)) {
                    self.cx.brif(c, x, &a, y, &e);
                }
            }
            Term::Switch { on, cases, default } => self.switch(*on, cases, default.as_ref()),
            Term::Return(vs) => {
                let mut out = Vec::new();
                for v in vs {
                    self.spread(*v, &mut out);
                }
                self.cx.b.ins().return_(&out);
            }
            Term::Unreachable => {
                self.cx.b.ins().trap(UNREACHABLE);
            }
        }
    }

    /// A discriminant switch.
    ///
    /// `cranelift-frontend`'s own `Switch` does the partitioning the design
    /// describes — `br_table` over a dense range, a balanced comparison tree
    /// over a sparse one — so it is used rather than reimplemented (§3.1). It
    /// takes argument-less blocks, so each case gets a trampoline carrying the
    /// edge's arguments.
    fn switch(&mut self, on: ValueId, cases: &[(u64, Target)], default: Option<&Target>) {
        let Some(v) = self.get(on) else { return };
        let mut sw = Switch::new();
        let mut trampolines = Vec::new();
        for (key, target) in cases {
            let tb = self.cx.b.create_block();
            sw.set_entry(u128::from(*key), tb);
            trampolines.push((tb, target.clone()));
        }
        let otherwise = self.cx.b.create_block();
        sw.emit(self.cx.b, v, otherwise);
        self.cx.b.switch_to_block(otherwise);
        match default {
            Some(t) => {
                let args = self.edge_args(t);
                if let Some(dest) = self.block_of(t) {
                    self.cx.jump(dest, &args);
                }
            }
            None => {
                // Exhaustiveness is proved (`exhaustiveness.rs`), so this arm
                // cannot run. The debug profile calls the runtime's message
                // anyway, because this is the backend that wears a belt and
                // the belt is cheap (§3.1).
                if self.cx.unit.profile.defensive_aborts() {
                    let f = self.cx.rt_ref("buri_rt_abort_unreachable", &[], &[]);
                    self.cx.b.ins().call(f, &[]);
                }
                self.cx.b.ins().trap(UNREACHABLE);
            }
        }
        for (tb, target) in trampolines {
            self.cx.b.switch_to_block(tb);
            let args = self.edge_args(&target);
            if let Some(dest) = self.block_of(&target) {
                self.cx.jump(dest, &args);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Where the counts are
// ---------------------------------------------------------------------------

/// One place a reference count lives inside a value.
enum Site {
    /// A counted block pointer at this byte offset.
    Block { offset: u32, glue: Glue },
    /// A field that is itself an aggregate with counts inside it.
    Nested { offset: u32, ty: Ty },
    /// The pointer a recursive type's field is behind (VALUE-MODEL.md §5.2).
    Boxed { offset: u32, ty: Ty },
    /// An enum: switch on the tag, then walk the variant that is live.
    Tagged { tag: Scalar, variants: Vec<(u32, Ty, u32)> },
    /// A niche-encoded `Option`: walk the payload only where it is `.Some`.
    Guarded { null_at: u32, ty: Ty },
}

/// How deep the counted-pointer walk descends before concluding the type graph
/// has a cycle the boxing rule failed to cut. A fuse, not a limit:
/// `Layouts::boxes` cuts every cycle, so reaching it is an inconsistency.
const RC_DEPTH: u32 = 64;

fn counted_ty(
    tables: &Tables,
    layouts: &mut layout::Layouts<'_>,
    ty: &Ty,
    depth: u32,
) -> bool {
    if depth > RC_DEPTH {
        return false;
    }
    let next = depth.saturating_add(1);
    let any = |fields: Vec<Ty>, layouts: &mut layout::Layouts<'_>| {
        fields.iter().any(|f| layouts.boxes(ty, f) || counted_ty(tables, layouts, f, next))
    };
    match ty {
        Ty::Array(_) | Ty::Fn(_, _) => true,
        Ty::Tuple(_) | Ty::Ctx(_) => any(abi::field_types(tables, ty), layouts),
        Ty::Con(id, _) => match &tables.tycon(*id).def {
            TyDef::Prim(Prim::Str | Prim::Template) => true,
            TyDef::Prim(_) => false,
            TyDef::Struct { .. } => any(abi::field_types(tables, ty), layouts),
            TyDef::Enum { .. } => (0..tables.tycon(*id).variants().len())
                .any(|v| any(abi::variant_types(tables, ty, v), layouts)),
        },
        _ => false,
    }
}

fn sites_of(unit: &mut Unit<'_>, ty: &Ty) -> Vec<Site> {
    let tables = unit.abi.tables;
    match ty {
        Ty::Array(elem) => vec![Site::Block {
            offset: word(LIST_PTR),
            glue: Glue::Elems((**elem).clone()),
        }],
        Ty::Fn(_, _) => vec![Site::Block { offset: word(CLOSURE_ENV), glue: Glue::Env }],
        Ty::Tuple(_) | Ty::Ctx(_) => record_sites(unit, ty, &abi::field_types(tables, ty)),
        Ty::Con(id, _) => match &tables.tycon(*id).def {
            TyDef::Prim(Prim::Str | Prim::Template) => {
                vec![Site::Block { offset: word(STR_BASE), glue: Glue::None }]
            }
            TyDef::Prim(_) => Vec::new(),
            TyDef::Struct { .. } => record_sites(unit, ty, &abi::field_types(tables, ty)),
            TyDef::Enum { .. } => enum_sites(unit, ty),
        },
        _ => Vec::new(),
    }
}

fn record_sites(unit: &mut Unit<'_>, owner: &Ty, fields: &[Ty]) -> Vec<Site> {
    let l = unit.abi.layouts.of(owner.clone());
    let mut out = Vec::new();
    for (i, f) in fields.iter().enumerate() {
        let offset = l.fields.get(i).copied().unwrap_or(0);
        if unit.abi.layouts.boxes(owner, f) {
            out.push(Site::Boxed { offset, ty: f.clone() });
        } else if counted_ty(unit.abi.tables, &mut unit.abi.layouts, f, 0) {
            out.push(Site::Nested { offset, ty: f.clone() });
        }
    }
    out
}

fn enum_sites(unit: &mut Unit<'_>, owner: &Ty) -> Vec<Site> {
    let l = unit.abi.layouts.of(owner.clone());
    let Repr::Enum { repr, .. } = l.repr.clone() else { return Vec::new() };
    match repr {
        EnumRepr::Bare { .. } => Vec::new(),
        EnumRepr::Niche { null_at } => {
            let Ty::Con(_, args) = owner else { return Vec::new() };
            let Some(payload) = args.first().cloned() else { return Vec::new() };
            if counted_ty(unit.abi.tables, &mut unit.abi.layouts, &payload, 0) {
                vec![Site::Guarded { null_at, ty: payload }]
            } else {
                Vec::new()
            }
        }
        EnumRepr::Tagged { tag, .. } => {
            let Ty::Con(id, _) = owner else { return Vec::new() };
            let count = unit.abi.tables.tycon(*id).variants().len();
            let mut variants = Vec::new();
            for v in 0..count {
                let fields = abi::variant_types(unit.abi.tables, owner, v);
                let offsets = l.variant(v).to_vec();
                for (i, f) in fields.iter().enumerate() {
                    let offset = offsets.get(i).copied().unwrap_or(0);
                    let counts = unit.abi.layouts.boxes(owner, f)
                        || counted_ty(unit.abi.tables, &mut unit.abi.layouts, f, 0);
                    if counts {
                        variants.push((u32::try_from(v).unwrap_or(0), f.clone(), offset));
                    }
                }
            }
            if variants.is_empty() {
                Vec::new()
            } else {
                vec![Site::Tagged { tag, variants }]
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Small helpers
// ---------------------------------------------------------------------------

/// A field index in the layout table's word-indexed constants, as a byte
/// offset. `STR_LEN` is word 2 and byte 16.
pub fn word(index: usize) -> u32 {
    u32::try_from(index.saturating_mul(8)).unwrap_or(0)
}

pub fn scalar_clif(s: Scalar) -> ClifType {
    match s {
        Scalar::Bool | Scalar::I8 => types::I8,
        Scalar::I16 => types::I16,
        Scalar::I32 => types::I32,
        Scalar::I64 => types::I64,
        Scalar::I128 => types::I128,
        Scalar::F32 => types::F32,
        Scalar::F64 => types::F64,
        Scalar::Ptr => PTR,
    }
}

fn signed_prim(p: Prim) -> bool {
    matches!(p, Prim::I8 | Prim::I16 | Prim::I32 | Prim::I64 | Prim::I128 | Prim::F32 | Prim::F64)
}

fn is_integer(p: Prim) -> bool {
    matches!(
        p,
        Prim::I8
            | Prim::I16
            | Prim::I32
            | Prim::I64
            | Prim::I128
            | Prim::U8
            | Prim::U16
            | Prim::U32
            | Prim::U64
            | Prim::U128
    )
}

/// Whether this backend can compile a call to this intrinsic at all.
///
/// Four routes, and this is the same four `Lower::intrinsic` tries, asked
/// ahead of emission rather than during it — which is what makes
/// [`super::Cranelift::missing_intrinsics`] an answer a program can be given
/// before a second is spent on it.
pub fn implemented(key: &str) -> bool {
    bits_op(key)
        || prim_trait_op(key)
        || open_coded_key(key)
        || derive_key(key).is_some()
        || runtime::entry(key).is_some()
        || numeric_op(key)
}

/// `derivePrimShow.U8` into `("derivePrimShow", Prim::U8)`.
///
/// `middle::lower`'s `qualified_key` puts the operand's primitive in the key,
/// because the IR type has lost it — see that function for why. `None` for
/// anything that is not one of the two operations implemented here, including
/// `derivePrimJson`, which answers a `core/json` value this backend cannot yet
/// construct.
fn derive_key(key: &str) -> Option<(&str, Prim)> {
    let (name, ty) = key.split_once('.')?;
    if !matches!(name, "derivePrimShow" | "derivePrimHash") {
        return None;
    }
    let prim = Prim::all().iter().copied().find(|p| p.name() == ty)?;
    Some((name, prim))
}

/// `Add` into `0`, `Sub` into `1`, `Mul` into `2`, `Div` into `3` — the
/// selector `buri_rt_i128_checked` and `buri_rt_i128_saturating` take.
fn wide_op(kind: &str) -> Option<u8> {
    match kind {
        "Add" => Some(0),
        "Sub" => Some(1),
        "Mul" => Some(2),
        "Div" => Some(3),
        _ => None,
    }
}

/// The `Eq`/`Ord`/`Hash`/`Show` leaves at `Bool` and `Char`, plus
/// `Char::toU32` — `Lower::prim_trait`'s list, asked ahead of emission.
///
/// `str.*` is absent because every one of those is a runtime entry and the
/// table already claims it; `str.show` is the one exception, because it is the
/// identity rather than a call.
pub fn prim_trait_op(key: &str) -> bool {
    matches!(
        key,
        "bool.eq"
            | "bool.compare"
            | "bool.hash"
            | "bool.show"
            | "char.eq"
            | "char.compare"
            | "char.hash"
            | "char.show"
            | "char.toU32"
            | "str.show"
    )
}

/// The `core/bits` operations `Lower::bits` emits, asked ahead of emission.
///
/// The unsigned-width family is spelled out rather than derived from a suffix,
/// because `core/bits` declares exactly these six (`bits.buri:24-29`) and a
/// rule that accepted `shlU16` would claim something that does not exist.
pub fn bits_op(key: &str) -> bool {
    matches!(
        key,
        "bits.shl"
            | "bits.shr"
            | "bits.sar"
            | "bits.popCount"
            | "bits.leadingZeros"
            | "bits.trailingZeros"
            | "bits.rotateLeft"
            | "bits.rotateRight"
            | "bits.shlU8"
            | "bits.shrU8"
            | "bits.shlU32"
            | "bits.shrU32"
            | "bits.shlU64"
            | "bits.shrU64"
    )
}

/// The intrinsics this backend emits as instructions rather than as a call.
///
/// Each is here because a call would fetch a word the backend already has the
/// address of, or would be a runtime function generating the same two
/// instructions. `str.concat` is the one that is a real body, and it is
/// generated because it is one allocation and two copies and a `buri_rt_*`
/// entry for it would be a call per interpolation.
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
            | "testing_assert.report"
            | "testing_assert.failWith"
            | "testing_assert.failExpected"
    )
}

/// The `num.<T>.<op>` and `num.<op>` operations `Lower::numeric` emits.
///
/// The same list, asked before emission rather than during it. `toJson` is
/// deliberately absent: it answers a `core/json` value, and constructing one
/// needs that module's type constructors, which the intrinsic table does not
/// name.
pub fn numeric_op(key: &str) -> bool {
    // `missing_intrinsics` is asked of the *monomorphized* program, before
    // `middle::lower` runs — so `Bounded` is still two segments there and three
    // by the time `Lower::numeric` sees it (`lower.rs`'s `bounded_key`). Both
    // spellings answer yes, because both describe an operation this backend
    // compiles.
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
            | "min"
            | "max"
            | "toChar"
            | "minValue"
            | "maxValue"
            | "eq"
            | "compare"
            | "hash"
            | "show"
            | "wrappingAdd"
            | "wrappingSub"
            | "wrappingMul"
    ) {
        return true;
    }
    // The overflow tests are `smulhi`/`umulhi`, which Cranelift does not define
    // at `i128`, so the 128-bit forms are a named gap rather than a wrong
    // answer.
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
    // A conversion is a plain cast only where it cannot fail. `SPEC 6.2.1`
    // gives every *inexact* one a `Result`, and constructing that needs the
    // error type `core/num` declares — which the intrinsic table does not name,
    // and which is a thing to build rather than to guess at. So `toI8` from an
    // `I64` is a named gap and `toI64` from an `I8` is a `sextend`.
    //
    // `wrapTo*` is always exact by construction: wrapping *is* the answer.
    if let Some(target) = op.strip_prefix("wrapTo") {
        return Prim::all().iter().any(|p| p.name() == target);
    }
    if let Some(target) = op.strip_prefix("to") {
        if let Some(to) = Prim::all().iter().copied().find(|p| p.name() == target) {
            return crate::compiler::semantics::builtins::conversion_is_exact(prim, to);
        }
    }
    false
}

fn helper_name(key: &Helper, n: usize) -> String {
    match key {
        Helper::Thunk { func, .. } => format!("buri$thunk{func}"),
        Helper::Concat => String::from("buri$concat"),
        Helper::ShowInt { signed } => {
            format!("buri$show_{}", if *signed { "int" } else { "uint" })
        }
        Helper::ShowBool => String::from("buri$show_bool"),
        Helper::EnvGlue => String::from("buri$env_glue"),
        Helper::Release { .. } => format!("buri$release{n}"),
        Helper::ReleaseElems { .. } => format!("buri$release_elems{n}"),
        Helper::RetainElem { .. } => format!("buri$retain{n}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A key the runtime has no body for answers `None` rather than a symbol
    /// that would fail to link — the table's whole purpose (`runtime.rs`'s
    /// header). The symbol *rule* is checked in `runtime.rs`; this checks that
    /// the two predicates a program is judged by agree with it.
    #[test]
    fn an_unimplemented_intrinsic_is_not_invented() {
        assert!(runtime::entry("host.HostFs.readFile").is_none());
        assert!(runtime::entry("list.map").is_none());
        assert!(!implemented("list.map"));
        assert!(!implemented("json.decode"));
        assert!(implemented("str.concat"));
        assert!(implemented("str.split"));
        // Qualified with the operand's primitive by `middle::lower`, because
        // the IR type has lost the signedness that separates `255` from `-1`.
        assert!(implemented("derivePrimShow.U8"));
        assert!(implemented("derivePrimHash.Str"));
        assert!(!implemented("derivePrimShow"));
        assert!(!implemented("derivePrimJson.I64"));
        assert!(implemented("host.HostStdout.println"));
    }

    /// The numeric surface, at the edges that moved: `Bounded` is two segments
    /// rather than three, and the 128-bit checked forms are a named gap.
    #[test]
    fn the_numeric_surface_is_the_one_lower_emits() {
        // `middle::lower`'s `bounded_key` turns `num.minValue` into the
        // three-segment form before a backend sees it, because `Bounded` takes
        // no argument and the IR type has lost the signedness.
        assert!(numeric_op("num.U8.minValue"));
        assert!(numeric_op("num.F32.maxValue"));
        // The two-segment form is what `missing_intrinsics` sees, before
        // `middle::lower` runs; both spellings describe an operation this
        // backend compiles, so both answer yes.
        assert!(numeric_op("num.minValue"));
        assert!(numeric_op("num.I64.compare"));
        assert!(numeric_op("num.F64.eq"));
        assert!(numeric_op("num.U8.checkedAdd"));
        assert!(numeric_op("num.I64.saturatingMul"));
        assert!(numeric_op("num.I128.checkedAdd"));
        assert!(!numeric_op("num.I64.toJson"));
        assert!(!numeric_op("num.Nope.add"));
    }

    #[test]
    fn a_word_index_is_a_byte_offset() {
        assert_eq!(word(STR_BASE), 0);
        assert_eq!(word(STR_PTR), 8);
        assert_eq!(word(STR_LEN), 16);
    }
}
