//! A typed builder over llvm-sys, for the emitter.
//!
//! inkwell's `Builder` answers every instruction with a `Result`. It refuses
//! an instruction in two cases: the builder has no block, or a `memcpy`'s
//! alignment is not a power of two. Here both are types instead of checks:
//!
//!  * an [`At`] is the builder after it has been positioned, and the only way
//!    to get one is to position it ([`At::end_of`]). Nothing here clears the
//!    position, so every `At` stays good;
//!  * an [`Align`] is a power of two by construction.
//!
//! So each method answers the value it built, and there is no error to drop
//! or to turn into an internal error.
//!
//! Every `unsafe` block in the LLVM emitter is in this file. Each one hands
//! LLVM operands that inkwell already typed, and wraps what
//! LLVM answered in the inkwell type of that instruction's result. A name is
//! set after the instruction is built ([`named`]), which is what LLVM's own
//! builder does with the name it is handed.

use std::ffi::c_char;
use std::ptr;

use inkwell::basic_block::BasicBlock;
use inkwell::builder::Builder;
use inkwell::context::Context;
use inkwell::llvm_sys::core as llvm;
use inkwell::llvm_sys::prelude::{LLVMBuilderRef, LLVMTypeRef, LLVMValueRef};
use inkwell::llvm_sys::LLVMOpcode;
use inkwell::module::{Linkage, Module};
use inkwell::types::{
    AsTypeRef, BasicMetadataTypeEnum, BasicType, FloatType, FunctionType, IntType, PointerType,
};
use inkwell::values::{
    AggregateValue, ArrayValue, AsValueRef, BasicMetadataValueEnum, BasicValue, BasicValueEnum,
    CallSiteValue, FloatValue, FunctionValue, InstructionOpcode, InstructionValue, IntValue,
    MetadataValue, PhiValue, PointerValue, StructValue,
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

/// Sets `name` on `v` and answers `v`, unless LLVM folded the instruction to
/// a constant: its own builder names only what it inserts, and a constant
/// can be a function whose name is its symbol.
fn named(v: LLVMValueRef, name: &str) -> LLVMValueRef {
    // SAFETY: `v` is a value LLVM just answered, and the name is read for
    // `name.len()` bytes with no terminator.
    unsafe {
        if llvm::LLVMIsAConstant(v).is_null() {
            llvm::LLVMSetValueName2(v, name.as_ptr().cast::<c_char>(), name.len());
        }
    }
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

    /// An arithmetic (sign-filling) right shift.
    pub fn ashr(self, l: IntValue<'ctx>, r: IntValue<'ctx>, name: &str) -> IntValue<'ctx> {
        // SAFETY: as `add`.
        int(
            unsafe { llvm::LLVMBuildAShr(self.raw(), l.as_value_ref(), r.as_value_ref(), NONE) },
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

    /// A cast of `v` to `to`, or `v` itself where it already is one, as
    /// LLVM's own builder answers: then nothing is built and nothing renamed.
    fn cast(self, op: LLVMOpcode, v: LLVMValueRef, to: LLVMTypeRef, name: &str) -> LLVMValueRef {
        // SAFETY: `v` is a value and `to` a type, and each caller pairs them
        // as `op` requires.
        unsafe {
            if llvm::LLVMTypeOf(v) == to {
                return v;
            }
            named(llvm::LLVMBuildCast(self.raw(), op, v, to, NONE), name)
        }
    }

    /// A zero extension, or nothing where `v` is already `to`.
    pub fn zext(self, v: IntValue<'ctx>, to: IntType<'ctx>, name: &str) -> IntValue<'ctx> {
        let v = self.cast(
            LLVMOpcode::LLVMZExt,
            v.as_value_ref(),
            to.as_type_ref(),
            name,
        );
        // SAFETY: an integer cast to an integer type.
        unsafe { IntValue::new(v) }
    }

    /// A sign extension, or nothing where `v` is already `to`.
    pub fn sext(self, v: IntValue<'ctx>, to: IntType<'ctx>, name: &str) -> IntValue<'ctx> {
        let v = self.cast(
            LLVMOpcode::LLVMSExt,
            v.as_value_ref(),
            to.as_type_ref(),
            name,
        );
        // SAFETY: as `zext`.
        unsafe { IntValue::new(v) }
    }

    /// A truncation, or nothing where `v` is already `to`.
    pub fn trunc(self, v: IntValue<'ctx>, to: IntType<'ctx>, name: &str) -> IntValue<'ctx> {
        let v = self.cast(
            LLVMOpcode::LLVMTrunc,
            v.as_value_ref(),
            to.as_type_ref(),
            name,
        );
        // SAFETY: as `zext`.
        unsafe { IntValue::new(v) }
    }

    pub fn ptr_to_int(
        self,
        p: PointerValue<'ctx>,
        to: IntType<'ctx>,
        name: &str,
    ) -> IntValue<'ctx> {
        let v = self.cast(
            LLVMOpcode::LLVMPtrToInt,
            p.as_value_ref(),
            to.as_type_ref(),
            name,
        );
        // SAFETY: a pointer cast to an integer type.
        unsafe { IntValue::new(v) }
    }

    pub fn int_to_ptr(
        self,
        v: IntValue<'ctx>,
        to: PointerType<'ctx>,
        name: &str,
    ) -> PointerValue<'ctx> {
        let v = self.cast(
            LLVMOpcode::LLVMIntToPtr,
            v.as_value_ref(),
            to.as_type_ref(),
            name,
        );
        // SAFETY: an integer cast to a pointer type.
        unsafe { PointerValue::new(v) }
    }

    /// The same bits as another type of the same width.
    pub fn bitcast(
        self,
        v: impl BasicValue<'ctx>,
        to: impl BasicType<'ctx>,
        name: &str,
    ) -> BasicValueEnum<'ctx> {
        let v = self.cast(
            LLVMOpcode::LLVMBitCast,
            v.as_value_ref(),
            to.as_type_ref(),
            name,
        );
        // SAFETY: a first-class value cast to a first-class type.
        unsafe { BasicValueEnum::new(v) }
    }

    pub fn fpext(self, v: FloatValue<'ctx>, to: FloatType<'ctx>, name: &str) -> FloatValue<'ctx> {
        let v = self.cast(
            LLVMOpcode::LLVMFPExt,
            v.as_value_ref(),
            to.as_type_ref(),
            name,
        );
        // SAFETY: a float cast to a float type.
        unsafe { FloatValue::new(v) }
    }

    pub fn fptrunc(self, v: FloatValue<'ctx>, to: FloatType<'ctx>, name: &str) -> FloatValue<'ctx> {
        let v = self.cast(
            LLVMOpcode::LLVMFPTrunc,
            v.as_value_ref(),
            to.as_type_ref(),
            name,
        );
        // SAFETY: as `fpext`.
        unsafe { FloatValue::new(v) }
    }

    /// An integer as the float nearest it, read as signed.
    pub fn sitofp(self, v: IntValue<'ctx>, to: FloatType<'ctx>, name: &str) -> FloatValue<'ctx> {
        let v = self.cast(
            LLVMOpcode::LLVMSIToFP,
            v.as_value_ref(),
            to.as_type_ref(),
            name,
        );
        // SAFETY: an integer cast to a float type.
        unsafe { FloatValue::new(v) }
    }

    /// An integer as the float nearest it, read as unsigned.
    pub fn uitofp(self, v: IntValue<'ctx>, to: FloatType<'ctx>, name: &str) -> FloatValue<'ctx> {
        let v = self.cast(
            LLVMOpcode::LLVMUIToFP,
            v.as_value_ref(),
            to.as_type_ref(),
            name,
        );
        // SAFETY: as `sitofp`.
        unsafe { FloatValue::new(v) }
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

    /// Field `index` of a struct or element `index` of an array.
    pub fn extract(
        self,
        agg: impl AggregateValue<'ctx>,
        index: u32,
        name: &str,
    ) -> BasicValueEnum<'ctx> {
        // SAFETY: `index` is inside the aggregate's type, which the slot
        // table it was built from says.
        unsafe {
            BasicValueEnum::new(named(
                llvm::LLVMBuildExtractValue(self.raw(), agg.as_value_ref(), index, NONE),
                name,
            ))
        }
    }

    /// `agg` with field `index` replaced by `v`.
    pub fn insert_field(
        self,
        agg: StructValue<'ctx>,
        v: impl BasicValue<'ctx>,
        index: u32,
        name: &str,
    ) -> StructValue<'ctx> {
        let (a, v) = (agg.as_value_ref(), v.as_value_ref());
        // SAFETY: as `extract`, and `v` is of that field's type.
        unsafe {
            StructValue::new(named(
                llvm::LLVMBuildInsertValue(self.raw(), a, v, index, NONE),
                name,
            ))
        }
    }

    /// `agg` with element `index` replaced by `v`.
    pub fn insert_element(
        self,
        agg: ArrayValue<'ctx>,
        v: impl BasicValue<'ctx>,
        index: u32,
        name: &str,
    ) -> ArrayValue<'ctx> {
        let (a, v) = (agg.as_value_ref(), v.as_value_ref());
        // SAFETY: as `insert_field`.
        unsafe {
            ArrayValue::new(named(
                llvm::LLVMBuildInsertValue(self.raw(), a, v, index, NONE),
                name,
            ))
        }
    }

    // -- memory ---------------------------------------------------------------

    /// `base + bytes`, a `getelementptr inbounds i8`.
    ///
    /// `inbounds` without exception, per CODEGEN-LLVM.md §3.4: every
    /// projection in this language is a field of a known layout or an index a
    /// bounds check has already turned into an `Option`, so the premise is
    /// enforced by the type system rather than assumed away.
    pub fn offset(
        self,
        base: PointerValue<'ctx>,
        bytes: IntValue<'ctx>,
        name: &str,
    ) -> PointerValue<'ctx> {
        let i8t = base.get_type().get_context().i8_type();
        let mut index = [bytes.as_value_ref()];
        // SAFETY: an `i8` element type takes any integer index; whether the
        // address is in bounds is the claim above, which LLVM does not check.
        unsafe {
            PointerValue::new(named(
                llvm::LLVMBuildInBoundsGEP2(
                    self.raw(),
                    i8t.as_type_ref(),
                    base.as_value_ref(),
                    index.as_mut_ptr(),
                    1,
                    NONE,
                ),
                name,
            ))
        }
    }

    /// [`At::offset`] by a constant, and `base` itself for none.
    pub fn byte_offset(
        self,
        base: PointerValue<'ctx>,
        bytes: i64,
        name: &str,
    ) -> PointerValue<'ctx> {
        if bytes == 0 {
            return base;
        }
        let i64t = base.get_type().get_context().i64_type();
        self.offset(base, i64t.const_int(bytes as u64, true), name)
    }

    /// `bytes` bytes on the stack, at the top of the entry block of the
    /// function the builder is in, leaving the builder at the end of the
    /// block it was in.
    ///
    /// The entry block, because `alloca` in LLVM is not scoped to a block: a
    /// buffer emitted inside a loop body would grow the frame once per
    /// iteration.
    pub fn entry_buffer(self, bytes: u32, align: Align) -> PointerValue<'ctx> {
        let here = self.block();
        // SAFETY: a positioned builder's block is in a function, and a
        // function with a block has an entry block.
        let entry = unsafe {
            let f = llvm::LLVMGetBasicBlockParent(here.as_mut_ptr());
            BasicBlock::new(llvm::LLVMGetEntryBasicBlock(f)).unwrap_unchecked()
        };
        self.top_of(entry);
        let ctx = entry.get_context();
        // A zero-byte buffer is legal and useless; one byte keeps the pointer
        // distinct from every other object, which is what the C side assumes.
        let count = ctx.i64_type().const_int(u64::from(bytes.max(1)), false);
        let buf = self.array_alloca(ctx.i8_type(), count, align, "out");
        self.to_end(here);
        buf
    }

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

    /// [`At::load`] of a pointer.
    pub fn load_ptr(
        self,
        ty: PointerType<'ctx>,
        p: PointerValue<'ctx>,
        align: Align,
        name: &str,
    ) -> PointerValue<'ctx> {
        // SAFETY: as `load`.
        unsafe {
            let v = llvm::LLVMBuildLoad2(self.raw(), ty.as_type_ref(), p.as_value_ref(), NONE);
            llvm::LLVMSetAlignment(v, align.bytes());
            PointerValue::new(named(v, name))
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

/// A function added with exactly `N` parameters, so each one is there to
/// read: [`Declared::params`] answers all `N` with no question of whether one
/// is missing.
#[derive(Clone, Copy)]
pub struct Declared<'ctx, const N: usize> {
    value: FunctionValue<'ctx>,
}

impl<'ctx, const N: usize> Declared<'ctx, N> {
    /// Adds `name` to `module`, taking `params`. `ty` makes the function type
    /// from them: `void_type().fn_type` or a value type's `fn_type`.
    pub fn add(
        module: &Module<'ctx>,
        name: &str,
        params: [BasicMetadataTypeEnum<'ctx>; N],
        ty: impl FnOnce(&[BasicMetadataTypeEnum<'ctx>]) -> FunctionType<'ctx>,
        linkage: Linkage,
    ) -> Self {
        Declared {
            value: module.add_function(name, ty(&params), Some(linkage)),
        }
    }

    pub fn value(self) -> FunctionValue<'ctx> {
        self.value
    }

    /// The function's parameters, in order.
    pub fn params(self) -> [BasicValueEnum<'ctx>; N] {
        let f = self.value.as_value_ref();
        std::array::from_fn(|i| {
            // SAFETY: the function was added with `N` parameters and `i` is
            // below `N`, so `LLVMGetParam` answers one of them.
            unsafe { BasicValueEnum::new(llvm::LLVMGetParam(f, i as u32)) }
        })
    }
}

/// Whether `ctx` keeps the names of the values built in it. Set before
/// anything is built: [`For::Linker`](super::For) drops them, as `clang` does
/// outside a debug build.
pub fn keep_names(ctx: &Context, keep: bool) {
    // SAFETY: `ctx` is a live context.
    unsafe { llvm::LLVMContextSetDiscardValueNames(ctx.raw(), i32::from(!keep)) };
}

/// Deletes `f` from its module if nothing in the module uses it, and answers
/// whether it did.
///
/// The caller must not hold `f` anywhere it will read it again: deleting it
/// leaves every copy of it dangling. `Unit::prune` clears its own table first.
pub fn delete_if_unused(f: FunctionValue<'_>) -> bool {
    let unused = f
        .as_global_value()
        .as_pointer_value()
        .get_first_use()
        .is_none();
    if unused {
        // SAFETY: nothing in the module uses `f`, and the caller holds no
        // copy of it.
        unsafe { f.delete() };
    }
    unused
}

/// Attaches metadata `node` of kind `kind` to `inst`.
pub fn set_metadata(inst: InstructionValue<'_>, node: MetadataValue<'_>, kind: u32) {
    // SAFETY: `node` is a metadata node, which every caller builds with
    // `Context::metadata_node`.
    unsafe { llvm::LLVMSetMetadata(inst.as_value_ref(), kind, node.as_value_ref()) };
}
