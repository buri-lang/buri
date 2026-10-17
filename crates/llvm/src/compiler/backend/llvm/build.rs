//! A typed builder over llvm-sys, for the emitter.
//!
//! inkwell's `Builder` answers every instruction with a `Result`. It refuses
//! an instruction in two cases: the builder has no block, or a `memcpy`'s
//! alignment is not a power of two. Here both are types instead of checks:
//!
//!  * an [`At`] is the builder after it has been positioned, and the only way
//!    to get one is to position it ([`At::end_of`]) or to find it positioned
//!    ([`At::already`]). Nothing here clears the position, so every `At`
//!    stays good;
//!  * an [`Align`] is a power of two by construction.
//!
//! So each method answers the value it built, and there is no error to drop
//! or to turn into an internal error.
//!
//! Every `unsafe` block in the emitter's instruction building is in this file.
//! Each one hands LLVM operands that inkwell already typed, and wraps what
//! LLVM answered in the inkwell type of that instruction's result. A name is
//! set after the instruction is built ([`named`]), which is what LLVM's own
//! builder does with the name it is handed.

use std::ffi::c_char;
use std::ptr;

use inkwell::basic_block::BasicBlock;
use inkwell::builder::Builder;
use inkwell::context::Context;
use inkwell::llvm_sys::core as llvm;
use inkwell::llvm_sys::prelude::{LLVMBuilderRef, LLVMValueRef};
use inkwell::types::{AsTypeRef, BasicType, FunctionType, IntType};
use inkwell::values::{
    AsValueRef, BasicMetadataValueEnum, BasicValue, BasicValueEnum, CallSiteValue, FloatValue,
    FunctionValue, InstructionOpcode, InstructionValue, IntValue, MetadataValue, PhiValue,
    PointerValue,
};
use inkwell::{AtomicOrdering, FloatPredicate, IntPredicate};

/// An alignment in bytes, always a power of two.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Align(u32);

impl Align {
    /// The alignment an address that is a multiple of `bytes` has: `bytes`
    /// itself for a power of two, and its largest power-of-two factor
    /// otherwise. Zero is a multiple of everything, so it is 1.
    pub fn of(bytes: u32) -> Align {
        let bytes = bytes.max(1);
        Align(bytes & bytes.wrapping_neg())
    }

    pub fn bytes(self) -> u32 {
        self.0
    }
}

/// The emitter's builder, positioned in a block.
///
/// `Copy`, because the position belongs to the one builder and not to the
/// token: every `At` builds wherever the builder was last positioned.
#[derive(Clone, Copy)]
pub struct At<'ctx> {
    b: &'ctx Builder<'ctx>,
}

/// Sets `name` on `v` and answers `v`. LLVM drops a name on a constant, as
/// its builder does when it folds an instruction away.
fn named(v: LLVMValueRef, name: &str) -> LLVMValueRef {
    // SAFETY: `v` is a value LLVM just answered, and the name is read for
    // `name.len()` bytes with no terminator.
    unsafe { llvm::LLVMSetValueName2(v, name.as_ptr().cast::<c_char>(), name.len()) };
    v
}

/// The empty name an instruction is built with before [`named`] names it.
const NONE: *const c_char = c"".as_ptr();

fn int<'ctx>(v: LLVMValueRef, name: &str) -> IntValue<'ctx> {
    // SAFETY: `v` is an integer instruction LLVM just built, or the constant
    // it folded one to.
    unsafe { IntValue::new(named(v, name)) }
}

fn float<'ctx>(v: LLVMValueRef, name: &str) -> FloatValue<'ctx> {
    // SAFETY: `v` is a floating-point instruction LLVM just built, or the
    // constant it folded one to.
    unsafe { FloatValue::new(named(v, name)) }
}

fn instruction<'ctx>(v: LLVMValueRef) -> InstructionValue<'ctx> {
    // SAFETY: `v` is an instruction LLVM just built.
    unsafe { InstructionValue::new(v) }
}

impl<'ctx> At<'ctx> {
    /// Positions `b` at the end of `block`.
    pub fn end_of(b: &'ctx Builder<'ctx>, block: BasicBlock<'ctx>) -> At<'ctx> {
        b.position_at_end(block);
        At { b }
    }

    /// `b` where it already is, or `None` if nothing has positioned it.
    pub fn already(b: &'ctx Builder<'ctx>) -> Option<At<'ctx>> {
        b.get_insert_block().map(|_| At { b })
    }

    fn raw(self) -> LLVMBuilderRef {
        self.b.as_mut_ptr()
    }

    /// Moves to the end of `block`.
    pub fn to_end(self, block: BasicBlock<'ctx>) {
        self.b.position_at_end(block);
    }

    /// Moves to just before `block`'s terminator, or to its end while it has
    /// none.
    pub fn before_terminator(self, block: BasicBlock<'ctx>) {
        let block = block.as_mut_ptr();
        // SAFETY: `block` is a live block; a null instruction means its end.
        unsafe {
            llvm::LLVMPositionBuilder(self.raw(), block, llvm::LLVMGetBasicBlockTerminator(block))
        };
    }

    /// Moves to the top of `block`: before its first instruction, or to its
    /// end while it has none.
    pub fn top_of(self, block: BasicBlock<'ctx>) {
        let block = block.as_mut_ptr();
        // SAFETY: as `before_terminator`.
        unsafe {
            llvm::LLVMPositionBuilder(self.raw(), block, llvm::LLVMGetFirstInstruction(block))
        };
    }

    /// Moves to just after `def` and the phis that follow it, so that what is
    /// built next can use `def` and sits where a phi may not.
    pub fn after(self, def: InstructionValue<'ctx>) {
        let mut next = def.get_next_instruction();
        while let Some(phi) = next.filter(|i| i.get_opcode() == InstructionOpcode::Phi) {
            next = phi.get_next_instruction();
        }
        let next = next.map_or(ptr::null_mut(), |i| i.as_value_ref());
        // SAFETY: `def` is an instruction this builder built, so it is in a
        // block; a null instruction means that block's end.
        unsafe {
            let block = llvm::LLVMGetInstructionParent(def.as_value_ref());
            llvm::LLVMPositionBuilder(self.raw(), block, next)
        };
    }

    /// The block the builder is in.
    pub fn block(self) -> BasicBlock<'ctx> {
        // SAFETY: an `At` exists only once the builder is positioned, and
        // nothing clears the position, so the block is not null.
        unsafe { BasicBlock::new(llvm::LLVMGetInsertBlock(self.raw())).unwrap_unchecked() }
    }

    // -- integers -------------------------------------------------------------

    pub fn add(self, l: IntValue<'ctx>, r: IntValue<'ctx>, name: &str) -> IntValue<'ctx> {
        // SAFETY: `l` and `r` are integers of one type (this file's header).
        int(
            unsafe { llvm::LLVMBuildAdd(self.raw(), l.as_value_ref(), r.as_value_ref(), NONE) },
            name,
        )
    }

    pub fn sub(self, l: IntValue<'ctx>, r: IntValue<'ctx>, name: &str) -> IntValue<'ctx> {
        // SAFETY: as `add`.
        int(
            unsafe { llvm::LLVMBuildSub(self.raw(), l.as_value_ref(), r.as_value_ref(), NONE) },
            name,
        )
    }

    pub fn mul(self, l: IntValue<'ctx>, r: IntValue<'ctx>, name: &str) -> IntValue<'ctx> {
        // SAFETY: as `add`.
        int(
            unsafe { llvm::LLVMBuildMul(self.raw(), l.as_value_ref(), r.as_value_ref(), NONE) },
            name,
        )
    }

    pub fn and(self, l: IntValue<'ctx>, r: IntValue<'ctx>, name: &str) -> IntValue<'ctx> {
        // SAFETY: as `add`.
        int(
            unsafe { llvm::LLVMBuildAnd(self.raw(), l.as_value_ref(), r.as_value_ref(), NONE) },
            name,
        )
    }

    pub fn or(self, l: IntValue<'ctx>, r: IntValue<'ctx>, name: &str) -> IntValue<'ctx> {
        // SAFETY: as `add`.
        int(
            unsafe { llvm::LLVMBuildOr(self.raw(), l.as_value_ref(), r.as_value_ref(), NONE) },
            name,
        )
    }

    pub fn xor(self, l: IntValue<'ctx>, r: IntValue<'ctx>, name: &str) -> IntValue<'ctx> {
        // SAFETY: as `add`.
        int(
            unsafe { llvm::LLVMBuildXor(self.raw(), l.as_value_ref(), r.as_value_ref(), NONE) },
            name,
        )
    }

    pub fn shl(self, l: IntValue<'ctx>, r: IntValue<'ctx>, name: &str) -> IntValue<'ctx> {
        // SAFETY: as `add`.
        int(
            unsafe { llvm::LLVMBuildShl(self.raw(), l.as_value_ref(), r.as_value_ref(), NONE) },
            name,
        )
    }

    /// A logical (zero-filling) right shift.
    pub fn lshr(self, l: IntValue<'ctx>, r: IntValue<'ctx>, name: &str) -> IntValue<'ctx> {
        // SAFETY: as `add`.
        int(
            unsafe { llvm::LLVMBuildLShr(self.raw(), l.as_value_ref(), r.as_value_ref(), NONE) },
            name,
        )
    }

    pub fn sdiv(self, l: IntValue<'ctx>, r: IntValue<'ctx>, name: &str) -> IntValue<'ctx> {
        // SAFETY: as `add`.
        int(
            unsafe { llvm::LLVMBuildSDiv(self.raw(), l.as_value_ref(), r.as_value_ref(), NONE) },
            name,
        )
    }

    pub fn udiv(self, l: IntValue<'ctx>, r: IntValue<'ctx>, name: &str) -> IntValue<'ctx> {
        // SAFETY: as `add`.
        int(
            unsafe { llvm::LLVMBuildUDiv(self.raw(), l.as_value_ref(), r.as_value_ref(), NONE) },
            name,
        )
    }

    pub fn srem(self, l: IntValue<'ctx>, r: IntValue<'ctx>, name: &str) -> IntValue<'ctx> {
        // SAFETY: as `add`.
        int(
            unsafe { llvm::LLVMBuildSRem(self.raw(), l.as_value_ref(), r.as_value_ref(), NONE) },
            name,
        )
    }

    pub fn urem(self, l: IntValue<'ctx>, r: IntValue<'ctx>, name: &str) -> IntValue<'ctx> {
        // SAFETY: as `add`.
        int(
            unsafe { llvm::LLVMBuildURem(self.raw(), l.as_value_ref(), r.as_value_ref(), NONE) },
            name,
        )
    }

    pub fn neg(self, v: IntValue<'ctx>, name: &str) -> IntValue<'ctx> {
        // SAFETY: `v` is an integer.
        int(
            unsafe { llvm::LLVMBuildNeg(self.raw(), v.as_value_ref(), NONE) },
            name,
        )
    }

    pub fn not(self, v: IntValue<'ctx>, name: &str) -> IntValue<'ctx> {
        // SAFETY: `v` is an integer.
        int(
            unsafe { llvm::LLVMBuildNot(self.raw(), v.as_value_ref(), NONE) },
            name,
        )
    }

    pub fn icmp(
        self,
        op: IntPredicate,
        l: IntValue<'ctx>,
        r: IntValue<'ctx>,
        name: &str,
    ) -> IntValue<'ctx> {
        let (l, r) = (l.as_value_ref(), r.as_value_ref());
        // SAFETY: as `add`.
        int(
            unsafe { llvm::LLVMBuildICmp(self.raw(), op.into(), l, r, NONE) },
            name,
        )
    }

    pub fn zext(self, v: IntValue<'ctx>, to: IntType<'ctx>, name: &str) -> IntValue<'ctx> {
        // SAFETY: `v` is an integer no wider than `to`.
        int(
            unsafe { llvm::LLVMBuildZExt(self.raw(), v.as_value_ref(), to.as_type_ref(), NONE) },
            name,
        )
    }

    /// A `zext`, or nothing where `v` is already `to`.
    pub fn zext_or_bitcast(
        self,
        v: IntValue<'ctx>,
        to: IntType<'ctx>,
        name: &str,
    ) -> IntValue<'ctx> {
        let (v, to) = (v.as_value_ref(), to.as_type_ref());
        // SAFETY: as `zext`.
        int(
            unsafe { llvm::LLVMBuildZExtOrBitCast(self.raw(), v, to, NONE) },
            name,
        )
    }

    pub fn trunc(self, v: IntValue<'ctx>, to: IntType<'ctx>, name: &str) -> IntValue<'ctx> {
        // SAFETY: `v` is an integer no narrower than `to`.
        int(
            unsafe { llvm::LLVMBuildTrunc(self.raw(), v.as_value_ref(), to.as_type_ref(), NONE) },
            name,
        )
    }

    /// A `trunc`, or nothing where `v` is already `to`.
    pub fn trunc_or_bitcast(
        self,
        v: IntValue<'ctx>,
        to: IntType<'ctx>,
        name: &str,
    ) -> IntValue<'ctx> {
        let (v, to) = (v.as_value_ref(), to.as_type_ref());
        // SAFETY: as `trunc`.
        int(
            unsafe { llvm::LLVMBuildTruncOrBitCast(self.raw(), v, to, NONE) },
            name,
        )
    }

    // -- floats ---------------------------------------------------------------

    pub fn fadd(self, l: FloatValue<'ctx>, r: FloatValue<'ctx>, name: &str) -> FloatValue<'ctx> {
        // SAFETY: `l` and `r` are floats of one type.
        float(
            unsafe { llvm::LLVMBuildFAdd(self.raw(), l.as_value_ref(), r.as_value_ref(), NONE) },
            name,
        )
    }

    pub fn fsub(self, l: FloatValue<'ctx>, r: FloatValue<'ctx>, name: &str) -> FloatValue<'ctx> {
        // SAFETY: as `fadd`.
        float(
            unsafe { llvm::LLVMBuildFSub(self.raw(), l.as_value_ref(), r.as_value_ref(), NONE) },
            name,
        )
    }

    pub fn fmul(self, l: FloatValue<'ctx>, r: FloatValue<'ctx>, name: &str) -> FloatValue<'ctx> {
        // SAFETY: as `fadd`.
        float(
            unsafe { llvm::LLVMBuildFMul(self.raw(), l.as_value_ref(), r.as_value_ref(), NONE) },
            name,
        )
    }

    pub fn fdiv(self, l: FloatValue<'ctx>, r: FloatValue<'ctx>, name: &str) -> FloatValue<'ctx> {
        // SAFETY: as `fadd`.
        float(
            unsafe { llvm::LLVMBuildFDiv(self.raw(), l.as_value_ref(), r.as_value_ref(), NONE) },
            name,
        )
    }

    pub fn frem(self, l: FloatValue<'ctx>, r: FloatValue<'ctx>, name: &str) -> FloatValue<'ctx> {
        // SAFETY: as `fadd`.
        float(
            unsafe { llvm::LLVMBuildFRem(self.raw(), l.as_value_ref(), r.as_value_ref(), NONE) },
            name,
        )
    }

    pub fn fneg(self, v: FloatValue<'ctx>, name: &str) -> FloatValue<'ctx> {
        // SAFETY: `v` is a float.
        float(
            unsafe { llvm::LLVMBuildFNeg(self.raw(), v.as_value_ref(), NONE) },
            name,
        )
    }

    pub fn fcmp(
        self,
        op: FloatPredicate,
        l: FloatValue<'ctx>,
        r: FloatValue<'ctx>,
        name: &str,
    ) -> IntValue<'ctx> {
        let (l, r) = (l.as_value_ref(), r.as_value_ref());
        // SAFETY: as `fadd`.
        int(
            unsafe { llvm::LLVMBuildFCmp(self.raw(), op.into(), l, r, NONE) },
            name,
        )
    }

    // -- values -----------------------------------------------------------------

    /// `cond ? then : else_`, for two values of one type.
    pub fn select(
        self,
        cond: IntValue<'ctx>,
        then: impl BasicValue<'ctx>,
        else_: impl BasicValue<'ctx>,
        name: &str,
    ) -> BasicValueEnum<'ctx> {
        let (c, t, e) = (
            cond.as_value_ref(),
            then.as_value_ref(),
            else_.as_value_ref(),
        );
        // SAFETY: `cond` is an `i1` and the two arms share a type.
        unsafe {
            BasicValueEnum::new(named(
                llvm::LLVMBuildSelect(self.raw(), c, t, e, NONE),
                name,
            ))
        }
    }

    pub fn is_null(self, p: PointerValue<'ctx>, name: &str) -> IntValue<'ctx> {
        // SAFETY: `p` is a pointer.
        int(
            unsafe { llvm::LLVMBuildIsNull(self.raw(), p.as_value_ref(), NONE) },
            name,
        )
    }

    /// An empty phi of type `ty`, for the incoming edges to be added to.
    pub fn phi(self, ty: impl BasicType<'ctx>, name: &str) -> PhiValue<'ctx> {
        // SAFETY: a phi of a first-class type, at the top of its block.
        unsafe {
            PhiValue::new(named(
                llvm::LLVMBuildPhi(self.raw(), ty.as_type_ref(), NONE),
                name,
            ))
        }
    }

    // -- memory ---------------------------------------------------------------

    pub fn load(
        self,
        ty: impl BasicType<'ctx>,
        p: PointerValue<'ctx>,
        align: Align,
        name: &str,
    ) -> BasicValueEnum<'ctx> {
        // SAFETY: `p` is a pointer and `ty` a first-class type.
        unsafe {
            let v = llvm::LLVMBuildLoad2(self.raw(), ty.as_type_ref(), p.as_value_ref(), NONE);
            llvm::LLVMSetAlignment(v, align.bytes());
            BasicValueEnum::new(named(v, name))
        }
    }

    /// A `monotonic` atomic load: a plain one on every target this builds
    /// for, which no other thread's store tears.
    pub fn load_monotonic(
        self,
        ty: IntType<'ctx>,
        p: PointerValue<'ctx>,
        align: Align,
        name: &str,
    ) -> IntValue<'ctx> {
        // SAFETY: as `load`, of an integer, which an atomic load may be.
        unsafe {
            let v = llvm::LLVMBuildLoad2(self.raw(), ty.as_type_ref(), p.as_value_ref(), NONE);
            llvm::LLVMSetAlignment(v, align.bytes());
            llvm::LLVMSetOrdering(v, AtomicOrdering::Monotonic.into());
            int(v, name)
        }
    }

    /// [`At::load`] of an integer.
    pub fn load_int(
        self,
        ty: IntType<'ctx>,
        p: PointerValue<'ctx>,
        align: Align,
        name: &str,
    ) -> IntValue<'ctx> {
        // SAFETY: as `load`.
        unsafe {
            let v = llvm::LLVMBuildLoad2(self.raw(), ty.as_type_ref(), p.as_value_ref(), NONE);
            llvm::LLVMSetAlignment(v, align.bytes());
            int(v, name)
        }
    }

    pub fn store(self, p: PointerValue<'ctx>, v: impl BasicValue<'ctx>, align: Align) {
        // SAFETY: `p` is a pointer and `v` a first-class value.
        unsafe {
            let store = llvm::LLVMBuildStore(self.raw(), v.as_value_ref(), p.as_value_ref());
            llvm::LLVMSetAlignment(store, align.bytes());
        }
    }

    /// `count` of `ty` on the stack.
    pub fn array_alloca(
        self,
        ty: impl BasicType<'ctx>,
        count: IntValue<'ctx>,
        align: Align,
        name: &str,
    ) -> PointerValue<'ctx> {
        let (ty, count) = (ty.as_type_ref(), count.as_value_ref());
        // SAFETY: `ty` is sized and `count` an integer.
        unsafe {
            let v = llvm::LLVMBuildArrayAlloca(self.raw(), ty, count, NONE);
            llvm::LLVMSetAlignment(v, align.bytes());
            PointerValue::new(named(v, name))
        }
    }

    pub fn memcpy(
        self,
        dst: PointerValue<'ctx>,
        dst_align: Align,
        src: PointerValue<'ctx>,
        src_align: Align,
        bytes: IntValue<'ctx>,
    ) {
        let (d, s, n) = (dst.as_value_ref(), src.as_value_ref(), bytes.as_value_ref());
        // SAFETY: two pointers and an integer length; the alignments are
        // powers of two, which is all LLVM asserts.
        unsafe { llvm::LLVMBuildMemCpy(self.raw(), d, dst_align.bytes(), s, src_align.bytes(), n) };
    }

    pub fn memmove(
        self,
        dst: PointerValue<'ctx>,
        dst_align: Align,
        src: PointerValue<'ctx>,
        src_align: Align,
        bytes: IntValue<'ctx>,
    ) {
        let (d, s, n) = (dst.as_value_ref(), src.as_value_ref(), bytes.as_value_ref());
        // SAFETY: as `memcpy`.
        unsafe {
            llvm::LLVMBuildMemMove(self.raw(), d, dst_align.bytes(), s, src_align.bytes(), n)
        };
    }

    /// `bytes` bytes of `value`, an `i8`, at `dst`.
    pub fn memset(
        self,
        dst: PointerValue<'ctx>,
        align: Align,
        value: IntValue<'ctx>,
        bytes: IntValue<'ctx>,
    ) {
        let (d, v, n) = (
            dst.as_value_ref(),
            value.as_value_ref(),
            bytes.as_value_ref(),
        );
        // SAFETY: as `memcpy`.
        unsafe { llvm::LLVMBuildMemSet(self.raw(), d, v, n, align.bytes()) };
    }

    // -- control --------------------------------------------------------------

    pub fn br(self, to: BasicBlock<'ctx>) {
        // SAFETY: `to` is a block of the function being built.
        unsafe { llvm::LLVMBuildBr(self.raw(), to.as_mut_ptr()) };
    }

    /// A two-way branch, answered so that it can carry a weight.
    pub fn cond_br(
        self,
        cond: IntValue<'ctx>,
        then: BasicBlock<'ctx>,
        else_: BasicBlock<'ctx>,
    ) -> InstructionValue<'ctx> {
        let (c, t, e) = (cond.as_value_ref(), then.as_mut_ptr(), else_.as_mut_ptr());
        // SAFETY: `cond` is an `i1`; both are blocks of the function.
        instruction(unsafe { llvm::LLVMBuildCondBr(self.raw(), c, t, e) })
    }

    /// A `switch` on `on`, with `cases` keyed at `on`'s type.
    pub fn switch(
        self,
        on: IntValue<'ctx>,
        default: BasicBlock<'ctx>,
        cases: &[(IntValue<'ctx>, BasicBlock<'ctx>)],
    ) {
        // A hint for the table's capacity, which LLVM grows past.
        let n = cases.len() as u32;
        // SAFETY: `on` is an integer, every key a constant of its type, and
        // every target a block of the function.
        unsafe {
            let switch =
                llvm::LLVMBuildSwitch(self.raw(), on.as_value_ref(), default.as_mut_ptr(), n);
            for (key, to) in cases {
                llvm::LLVMAddCase(switch, key.as_value_ref(), to.as_mut_ptr());
            }
        }
    }

    pub fn ret(self, v: impl BasicValue<'ctx>) {
        // SAFETY: `v` is of the function's return type.
        unsafe { llvm::LLVMBuildRet(self.raw(), v.as_value_ref()) };
    }

    pub fn ret_void(self) {
        // SAFETY: the function returns `void`.
        unsafe { llvm::LLVMBuildRetVoid(self.raw()) };
    }

    pub fn unreachable(self) {
        // SAFETY: no operands.
        unsafe { llvm::LLVMBuildUnreachable(self.raw()) };
    }

    // -- calls ----------------------------------------------------------------

    /// A call of `f`, unnamed: a call answering `void` must be.
    pub fn call(
        self,
        f: FunctionValue<'ctx>,
        args: &[BasicMetadataValueEnum<'ctx>],
    ) -> CallSiteValue<'ctx> {
        self.call_named(f.get_type(), f.as_value_ref(), args, "")
    }

    /// A call of `f`, which answers a value, named `name`.
    pub fn call_answering(
        self,
        f: FunctionValue<'ctx>,
        args: &[BasicMetadataValueEnum<'ctx>],
        name: &str,
    ) -> CallSiteValue<'ctx> {
        self.call_named(f.get_type(), f.as_value_ref(), args, name)
    }

    /// An unnamed call through `target`, a function of type `ty`.
    pub fn indirect_call(
        self,
        ty: FunctionType<'ctx>,
        target: PointerValue<'ctx>,
        args: &[BasicMetadataValueEnum<'ctx>],
    ) -> CallSiteValue<'ctx> {
        self.call_named(ty, target.as_value_ref(), args, "")
    }

    fn call_named(
        self,
        ty: FunctionType<'ctx>,
        target: LLVMValueRef,
        args: &[BasicMetadataValueEnum<'ctx>],
        name: &str,
    ) -> CallSiteValue<'ctx> {
        let mut args: Vec<LLVMValueRef> = args.iter().map(|a| a.as_value_ref()).collect();
        // A call has the few arguments its signature declares.
        let n = args.len() as u32;
        // SAFETY: `args` match `ty`'s parameters, and `target` is of type `ty`.
        unsafe {
            let call = llvm::LLVMBuildCall2(
                self.raw(),
                ty.as_type_ref(),
                target,
                args.as_mut_ptr(),
                n,
                NONE,
            );
            CallSiteValue::new(named(call, name))
        }
    }

    /// What a call of a function answering a pointer answers.
    pub fn pointer(self, call: CallSiteValue<'ctx>) -> PointerValue<'ctx> {
        // SAFETY: the callee answers a pointer, so the call is one.
        unsafe { PointerValue::new(call.as_value_ref()) }
    }

    /// What a call answers, as a value.
    pub fn answer(self, call: CallSiteValue<'ctx>) -> BasicValueEnum<'ctx> {
        // SAFETY: the callee answers a first-class value, so the call is one.
        unsafe { BasicValueEnum::new(call.as_value_ref()) }
    }
}

/// Whether `ctx` keeps the names of the values built in it. Set before
/// anything is built: [`For::Linker`](super::For) drops them, as `clang` does
/// outside a debug build.
pub fn keep_names(ctx: &Context, keep: bool) {
    // SAFETY: `ctx` is a live context.
    unsafe { llvm::LLVMContextSetDiscardValueNames(ctx.raw(), i32::from(!keep)) };
}

/// Attaches metadata `node` of kind `kind` to `inst`.
pub fn set_metadata(inst: InstructionValue<'_>, node: MetadataValue<'_>, kind: u32) {
    // SAFETY: `node` is a metadata node, which every caller builds with
    // `Context::metadata_node`.
    unsafe { llvm::LLVMSetMetadata(inst.as_value_ref(), kind, node.as_value_ref()) };
}
