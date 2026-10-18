//! What this backend emits for each intrinsic key, decided once per program.
//!
//! `middle::lower` gives every key it writes an id ([`ir::IntrinsicId`] for an
//! `Inst::CallIntrinsic`, [`ir::RuntimeId`] for a `Body::Runtime`). This file
//! reads each key once and answers the sequence `emit.rs` writes for it, so a
//! key with no sequence is refused before emission starts, and emission reads
//! its route by id and never meets a key it has no answer for.

use super::emit::bound_pattern;
use super::runtime::{self, Entry};
use crate::compiler::backend::intrinsic_keys::{
    bits_op, conversion_target, derive_key, number_key, Leaf,
};
use crate::compiler::middle::ir;
use crate::compiler::semantics::layered::Dense;
use crate::compiler::semantics::types::{IntPrim, Prim};

/// Every key a lowered program names, resolved.
pub struct Routes<'p> {
    pub(crate) calls: Dense<ir::IntrinsicId, Call>,
    pub(crate) bodies: Dense<ir::RuntimeId, Body<'p>>,
}

impl<'p> Routes<'p> {
    /// Every key `lowered` names, resolved, or every key this backend has no
    /// sequence for.
    pub fn of(lowered: &'p ir::Program) -> Result<Routes<'p>, Vec<String>> {
        let (calls, bodies) = crate::compiler::backend::resolve_program(lowered, call, body)?;
        Ok(Routes { calls, bodies })
    }
}

/// What an `Inst::CallIntrinsic` is emitted as. `middle` writes these keys
/// itself: a template's `str.concat`, a derived leaf, a failed assertion's
/// shown report, and keys with a runtime row.
#[derive(Clone, Copy)]
pub(crate) enum Call {
    FailExpectedShown,
    ReportShown,
    Concat,
    Derive(Leaf, Prim),
    Row(&'static Entry),
}

/// What a `Body::Runtime` function's generated body is.
#[derive(Clone, Copy)]
pub(crate) enum Body<'p> {
    CopyOut,
    FailExpected,
    FailWith,
    /// `number.<T>.<op>`, open-coded.
    Number(Prim, Number<'p>),
    /// One call to the runtime archive.
    Row(&'static Entry),
    StrLength,
    Alloc,
    Allocate,
    ListLength,
    StrFormat,
    ListEmpty,
    /// The `Equal`/`Ordered`/`Hash`/`Show` leaves at `Bool` and `Char`,
    /// `Char::toU32` and `Str`'s `show`.
    PrimTrait(Prim, PrimOp),
    /// `core/bits`: the stencil's stem, the operand's width, and whether it
    /// takes a shift count.
    Bits {
        stem: &'p str,
        width: u32,
        counted: bool,
    },
    Concat,
}

/// One `number.<T>.<op>` operation.
#[derive(Clone, Copy)]
pub(crate) enum Number<'p> {
    Show,
    /// `minValue` or `maxValue`: the bound's bit pattern.
    Bound(u128),
    Convert(Prim),
    Hash,
    Checked(&'p str),
    Saturating(IntPrim, &'p str),
    Wrapping(&'p str),
    /// `abs` when true, `signum` when false.
    AbsSignum(bool),
    Compare,
    /// A binary operator, by its stencil's name.
    Bin(&'static str),
    Negate,
}

/// One [`Body::PrimTrait`] operation.
#[derive(Clone, Copy)]
pub(crate) enum PrimOp {
    Show,
    Equal,
    Compare,
    Hash,
    ToU32,
}

fn call(key: &str) -> Option<Call> {
    Some(match key {
        "testing_assert.failExpectedShown" => Call::FailExpectedShown,
        "testing_assert.reportShown" => Call::ReportShown,
        "str.concat" => Call::Concat,
        _ => match derive_key(key) {
            Some((leaf, prim)) => Call::Derive(leaf, prim),
            None => Call::Row(runtime::entry(key)?),
        },
    })
}

fn body(key: &str) -> Option<Body<'_>> {
    let fixed = match key {
        "alloc.copyOut" => Some(Body::CopyOut),
        "testing_assert.failExpected" => Some(Body::FailExpected),
        "testing_assert.failWith" => Some(Body::FailWith),
        _ => None,
    };
    let numeric = || {
        let (prim, op) = number_key(key)?;
        Some(Body::Number(prim, number(prim, op)?))
    };
    fixed
        .or_else(numeric)
        .or_else(|| runtime::entry(key).map(Body::Row))
        .or_else(|| open_coded(key))
}

/// A `number.<T>.<op>` operation.
///
/// A binary operator is named by its stencil, which is named after the **C
/// operator** it wraps — `bin/sub/i64/ff/f` is `a - b` — where `core/number`
/// names its methods after the word.
fn number(prim: Prim, op: &str) -> Option<Number<'_>> {
    Some(match op {
        "show" => Number::Show,
        "minValue" => Number::Bound(bound_pattern(prim, true)?),
        "maxValue" => Number::Bound(bound_pattern(prim, false)?),
        "toChar" => Number::Convert(Prim::Char),
        "hash" => Number::Hash,
        "abs" => Number::AbsSignum(true),
        "signum" => Number::AbsSignum(false),
        "compare" => Number::Compare,
        "add" => Number::Bin("add"),
        "subtract" => Number::Bin("sub"),
        "multiply" => Number::Bin("mul"),
        "divide" => Number::Bin("div"),
        "remainder" => Number::Bin("rem"),
        "equal" => Number::Bin("eq"),
        "negate" => Number::Negate,
        _ => return prefixed(prim, op),
    })
}

/// The `number.<T>.<op>` operations named by a prefix: a conversion by its
/// target (`toU8`, `wrapToI32`), and `checked`, `saturating` and `wrapping` by
/// the operation they guard. `Saturating` is declared on the integers only.
fn prefixed(prim: Prim, op: &str) -> Option<Number<'_>> {
    conversion_target(op)
        .map(Number::Convert)
        .or_else(|| op.strip_prefix("checked").map(Number::Checked))
        .or_else(|| {
            Some(Number::Saturating(
                prim.as_int()?,
                op.strip_prefix("saturating")?,
            ))
        })
        .or_else(|| op.strip_prefix("wrapping").map(Number::Wrapping))
}

/// The keys this backend writes as instructions rather than as a call.
fn open_coded(key: &str) -> Option<Body<'_>> {
    Some(match key {
        "str.length" => Body::StrLength,
        "host_testing.alloc" => Body::Alloc,
        "host_testing.TestAllocator.allocate" => Body::Allocate,
        "list.length" => Body::ListLength,
        "str.format" => Body::StrFormat,
        "list.empty" => Body::ListEmpty,
        "str.concat" => Body::Concat,
        _ => return prim_trait(key).or_else(|| bits(key)),
    })
}

fn prim_trait(key: &str) -> Option<Body<'static>> {
    let (prim, op) = match key.split_once('.')? {
        ("bool", op) => (Prim::Bool, op),
        ("character", op) => (Prim::Char, op),
        ("str", op) => (Prim::Str, op),
        _ => return None,
    };
    let op = match op {
        "show" => PrimOp::Show,
        "equal" => PrimOp::Equal,
        "compare" => PrimOp::Compare,
        "hash" => PrimOp::Hash,
        "toU32" => PrimOp::ToU32,
        _ => return None,
    };
    Some(Body::PrimTrait(prim, op))
}

/// `core/bits`. The operand's width comes from the key's suffix; the bare
/// names are `Int`, which is sixty-four bits. The stencil is named after the
/// machine's shift, `core/bits` after the word.
fn bits(key: &str) -> Option<Body<'_>> {
    let op = key.strip_prefix("bits.").filter(|_| bits_op(key))?;
    let width = match op {
        _ if op.ends_with("U8") => 8,
        _ if op.ends_with("U32") => 32,
        _ => 64,
    };
    let stem = op
        .trim_end_matches(|c: char| c.is_ascii_digit())
        .trim_end_matches('U');
    let (stem, counted) = match stem {
        "shiftLeft" => ("shl", true),
        "shiftRightArithmetic" => ("sar", true),
        "shiftRight" => ("shr", true),
        // Three counts and the byte reversal, none of which takes a shift
        // count and so none of which needs the range check.
        "popCount" | "leadingZeros" | "trailingZeros" | "byteSwap" => (stem, false),
        other => (other, true),
    };
    Some(Body::Bits {
        stem,
        width,
        counted,
    })
}
