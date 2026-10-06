//! Line coverage probes, for `buri test --coverage` (`design/COVERAGE.md`).
//!
//! Not a pass the pipeline in `mod.rs` runs: the test runner calls
//! [`instrument`] between monomorphization and `middle::run`, and only when
//! coverage was asked for. So every backend sees the same probes, inlining
//! copies them with the code they sit in, and a plain build never comes here.

use crate::compiler::middle::monomorphize::Program;
use crate::compiler::semantics::typed::{Expr, ExprKind, Magnitude, Stmt};
use crate::compiler::semantics::types::Ty;
use crate::diagnostics::Span;

/// The inline intrinsic a probe is. Its one argument is the line's key.
pub const HIT: &str = "coverage.hit";

/// The largest key: 53 bits, so a JavaScript number holds every one exactly.
const KEY_MASK: u64 = 0x001F_FFFF_FFFF_FFFF;

/// The key for one line of one file. Two probes on one line share it.
pub fn key(file: &str, line: usize) -> u64 {
    // FNV-1a, over the name, a separator no path holds, and the line.
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let line = line.to_string();
    for b in file.bytes().chain(std::iter::once(0)).chain(line.bytes()) {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h & KEY_MASK
}

/// Calls `f` with the span of every site in `body`: the body itself, each
/// statement and tail of a block, each branch of an `if`, each arm of a
/// `match`, and each lambda's body. A line counts when a site starts on it.
pub fn sites(body: &Expr, f: &mut impl FnMut(Span)) {
    f(body.span);
    within(body, f);
}

fn within(e: &Expr, f: &mut impl FnMut(Span)) {
    match &e.kind {
        ExprKind::Block { stmts, tail } => {
            for s in stmts {
                match s {
                    Stmt::Let { value, span, .. } => {
                        f(*span);
                        within(value, f);
                    }
                    Stmt::Expr(x) => sites(x, f),
                }
            }
            if let Some(t) = tail {
                sites(t, f);
            }
        }
        ExprKind::If { cond, then, else_ } => {
            within(cond, f);
            sites(then, f);
            sites(else_, f);
        }
        ExprKind::Match { scrutinee, arms } => {
            within(scrutinee, f);
            for a in arms {
                if let Some(g) = &a.guard {
                    within(g, f);
                }
                sites(&a.body, f);
            }
        }
        ExprKind::Lambda { body, .. } => sites(body, f),
        _ => crate::compiler::semantics::typed::children(e, &mut |c| within(c, f)),
    }
}

/// Puts a probe in front of every site [`sites`] names whose span `key_of`
/// answers for.
///
/// A site on the line of the site that has to run before it gets no probe of
/// its own: the line already counted. So a line's count is how many times
/// execution reached its first site, and `if (c) {` costs one probe, not two.
pub fn instrument(program: &mut Program, i64_ty: &Ty, key_of: &dyn Fn(Span) -> Option<u64>) {
    let probe = Probe { i64_ty, key_of };
    for f in &mut program.funcs {
        if let Some(body) = f.body_mut() {
            probe.site(body, None);
        }
    }
}

struct Probe<'a> {
    i64_ty: &'a Ty,
    key_of: &'a dyn Fn(Span) -> Option<u64>,
}

impl Probe<'_> {
    /// A site, run right after `before` (the key of the line that ran last).
    /// Answers the key its line leaves for whatever runs next.
    fn site(&self, e: &mut Expr, before: Option<u64>) -> Option<u64> {
        let here = (self.key_of)(e.span);
        let fresh = here.filter(|k| Some(*k) != before);
        let after = here.or(before);
        match &mut e.kind {
            ExprKind::Block { .. } => {
                self.block(e, after);
                if let (Some(k), ExprKind::Block { stmts, .. }) = (fresh, &mut e.kind) {
                    stmts.insert(0, Stmt::Expr(self.hit(k, e.span)));
                }
            }
            _ => {
                self.inside(e, after);
                if let Some(k) = fresh {
                    let span = e.span;
                    let ty = e.ty;
                    let inner = std::mem::replace(e, Expr::new(ExprKind::Unit, Ty::UNIT, span));
                    *e = Expr::new(
                        ExprKind::Block {
                            stmts: vec![Stmt::Expr(self.hit(k, span))],
                            tail: Some(Box::new(inner)),
                        },
                        ty,
                        span,
                    );
                }
            }
        }
        after
    }

    /// A block's statements and tail, each a site run after the one before.
    fn block(&self, e: &mut Expr, mut before: Option<u64>) {
        let ExprKind::Block { stmts, tail } = &mut e.kind else { return };
        let mut out = Vec::with_capacity(stmts.len());
        for mut s in std::mem::take(stmts) {
            match &mut s {
                Stmt::Let { value, span, .. } => {
                    let here = (self.key_of)(*span);
                    let fresh = here.filter(|k| Some(*k) != before);
                    before = here.or(before);
                    self.inside(value, before);
                    if let Some(k) = fresh {
                        out.push(Stmt::Expr(self.hit(k, *span)));
                    }
                }
                Stmt::Expr(x) => before = self.site(x, before),
            }
            out.push(s);
        }
        *stmts = out;
        if let Some(t) = tail {
            self.site(t, before);
        }
    }

    /// The sites inside an expression that is not one itself.
    fn inside(&self, e: &mut Expr, before: Option<u64>) {
        match &mut e.kind {
            ExprKind::If { cond, then, else_ } => {
                self.inside(cond, before);
                self.site(then, before);
                self.site(else_, before);
            }
            ExprKind::Match { scrutinee, arms } => {
                self.inside(scrutinee, before);
                for a in arms {
                    if let Some(g) = &mut a.guard {
                        self.inside(g, before);
                    }
                    self.site(&mut a.body, before);
                }
            }
            // A lambda's body runs when it is called, not where it is written.
            ExprKind::Lambda { body, .. } => {
                self.site(body, None);
            }
            ExprKind::Block { .. } => self.block(e, before),
            _ => crate::compiler::semantics::typed::children_mut(e, &mut |c| self.inside(c, before)),
        }
    }

    fn hit(&self, key: u64, span: Span) -> Expr {
        let arg = Expr::new(ExprKind::Int(Magnitude::new(u128::from(key)), false), *self.i64_ty, span);
        Expr::new(
            ExprKind::Intrinsic { name: HIT.to_string(), targs: Vec::new(), args: vec![arg] },
            Ty::UNIT,
            span,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::key;

    #[test]
    fn a_key_fits_a_javascript_number_and_names_its_line() {
        assert!(key("lib/a.buri", 7) <= super::KEY_MASK);
        assert_ne!(key("lib/a.buri", 7), key("lib/a.buri", 8));
        assert_ne!(key("lib/a.buri", 17), key("lib/a.buri1", 7));
    }
}
