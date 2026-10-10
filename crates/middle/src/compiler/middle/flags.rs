//! `derive Flags`: a struct of `Bool`s stored as one unsigned word.
//!
//! `Tables::flags_word` decides the word, and field `i` of `n` is bit
//! `n - 1 - i`, so comparing two words orders them as a plain struct's derived
//! `Ordered` does. Monomorphization rewrites every literal, update and field
//! read of such a struct into operations on the word, and `middle::derives`
//! renders one from it, both through [`Word`]. What remains for a backend is
//! the word's layout and the field tests a struct pattern makes.
//!
//! Every expression here keeps the struct's own type on the value it stands
//! for, so a descriptor, an instance key and a derived call still see the
//! struct. A backend asks `Tables::flags_word` where it needs the machine
//! width, and the operations name the word's primitive, which is all any of
//! them reads.

#![allow(
    clippy::arithmetic_side_effects,
    reason = "a field index is below `FLAGS_MAX`, so every shift and count \
              here fits a `u128`"
)]

use crate::compiler::semantics::typed::{Expr, ExprKind, Magnitude, PrimOp};
use crate::compiler::semantics::types::{Prim, Ty};
use crate::diagnostics::Span;

/// One `Flags` struct's word, and the types its operations are written at.
#[derive(Clone, Copy)]
pub struct Word {
    /// The unsigned primitive the struct is stored in.
    pub prim: Prim,
    /// How many fields, and so how many bits are in use.
    pub fields: usize,
    /// The struct itself.
    pub ty: Ty,
    /// `Bool`, which a field read answers.
    pub bool_ty: Ty,
    pub span: Span,
}

impl Word {
    /// Field `i`'s bit.
    pub fn mask(&self, i: usize) -> u128 {
        1 << (self.fields - 1 - i)
    }

    /// Every bit a field owns.
    pub fn all(&self) -> u128 {
        (1 << self.fields) - 1
    }

    /// A literal word, typed as the struct.
    pub fn lit(&self, v: u128) -> Expr {
        Expr::new(ExprKind::Int(Magnitude::new(v), false), self.ty, self.span)
    }

    /// An operation on the word, answering another word.
    pub fn op(&self, op: PrimOp, args: Vec<Expr>) -> Expr {
        Expr::new(ExprKind::Prim { op, prim: self.prim, args }, self.ty, self.span)
    }

    /// A comparison of two words.
    pub fn test(&self, op: PrimOp, a: Expr, b: Expr) -> Expr {
        Expr::new(ExprKind::Prim { op, prim: self.prim, args: vec![a, b] }, self.bool_ty, self.span)
    }

    /// `x.f`: whether field `i`'s bit is set.
    pub fn bit(&self, x: Expr, i: usize) -> Expr {
        let masked = self.op(PrimOp::BitAnd, vec![x, self.lit(self.mask(i))]);
        self.test(PrimOp::Ne, masked, self.lit(0))
    }

    /// `if (v) { mask } else { 0 }`, folded where `v` is a literal.
    fn place(&self, i: usize, v: Expr) -> Result<u128, Expr> {
        match v.kind {
            ExprKind::Bool(b) => Ok(if b { self.mask(i) } else { 0 }),
            _ => Err(Expr::new(
                ExprKind::If {
                    cond: Box::new(v),
                    then: Box::new(self.lit(self.mask(i))),
                    else_: Box::new(self.lit(0)),
                },
                self.ty,
                self.span,
            )),
        }
    }

    /// `base | v0 | v1 …`, the constant bits folded into one literal. The
    /// computed values stay in the order given, which is the order they run.
    fn join(&self, base: Option<Expr>, values: Vec<(usize, Expr)>) -> Expr {
        let mut constant = 0;
        let mut parts: Vec<Expr> = base.into_iter().collect();
        for (i, v) in values {
            match self.place(i, v) {
                Ok(bits) => constant |= bits,
                Err(e) => parts.push(e),
            }
        }
        if constant != 0 || parts.is_empty() {
            parts.push(self.lit(constant));
        }
        let mut parts = parts.into_iter();
        let first = parts.next().unwrap_or_else(|| self.lit(0));
        parts.fold(first, |acc, p| self.op(PrimOp::BitOr, vec![acc, p]))
    }

    /// A struct literal, its field values in declaration order.
    pub fn pack(&self, fields: Vec<Expr>) -> Expr {
        self.join(None, fields.into_iter().enumerate().collect())
    }

    /// `T { ..base, f: v, .. }`: the updated bits cleared, then set from the
    /// new values.
    pub fn update(&self, base: Expr, updates: Vec<(usize, Expr)>) -> Expr {
        let cleared = updates.iter().fold(0, |m, (i, _)| m | self.mask(*i));
        let kept = self.op(PrimOp::BitAnd, vec![base, self.lit(self.all() ^ cleared)]);
        self.join(Some(kept), updates)
    }
}
