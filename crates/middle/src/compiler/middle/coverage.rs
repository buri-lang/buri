//! Line and branch coverage probes, for `buri test --coverage`
//! (`design/COVERAGE.md`).
//!
//! Not a pass the pipeline in `mod.rs` runs: the test runner calls
//! [`instrument`] and then [`branches`] between monomorphization and
//! `middle::run`, and only when coverage was asked for. So every backend sees
//! the same probes, inlining copies them with the code they sit in, and a
//! plain build never comes here.

use crate::compiler::middle::monomorphize::{FuncKind, Program};
use crate::compiler::semantics::name::Name;
use crate::compiler::semantics::typed::{self, Expr, ExprKind, Magnitude, PatKind, Pattern, Stmt};
use crate::compiler::semantics::types::{LocalId, Ty};
use crate::diagnostics::Span;

/// The inline intrinsic a probe is. Its one argument is the line's key.
pub const HIT: &str = "coverage.hit";

/// The largest key: 53 bits, so a JavaScript number holds every one exactly.
const KEY_MASK: u64 = 0x001F_FFFF_FFFF_FFFF;

/// The key for one line of one file. Two probes on one line share it.
pub fn key(file: &str, line: usize) -> u64 {
    fnv(file, &line.to_string())
}

/// The key for one probe of one decision. A line's key hashes digits after the
/// separator and this one a letter, so the two never hash the same input.
pub fn branch_key(file: &str, kind: Kind, (start, end): (u32, u32), slot: usize) -> u64 {
    fnv(file, &format!("b{}:{start},{end},{slot}", kind as u8))
}

/// FNV-1a, over the name, a separator no path holds, and `rest`.
fn fnv(file: &str, rest: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in file.bytes().chain(std::iter::once(0)).chain(rest.bytes()) {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h & KEY_MASK
}

/// What kind of choice a decision is. Its branches, in order:
///
/// - `If`: the `then` side, the `else` side;
/// - `Match`: each arm, in source order;
/// - `Guard`: true, false;
/// - `And`, `Or`: the right side ran, it didn't;
/// - `Try`: execution went on, `?` returned early.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Kind {
    If,
    Match,
    Guard,
    And,
    Or,
    Try,
}

/// A point in the user's source where control takes one of several paths.
#[derive(Clone, Copy, Debug)]
pub struct Decision {
    pub kind: Kind,
    /// With `kind`, what names the decision across suites and instantiations.
    pub span: Span,
    /// Where the choice is made, and so the line it's reported on: the `if`,
    /// the `match`, the guard, the right side of `&&` or `||`, the `?`.
    pub at: u32,
    pub branches: usize,
}

/// Calls `f` with every decision in `body`, lambdas' included.
///
/// The `match` that `semantics` writes for `a < b` isn't one: its arms carry
/// the operator's span, which an arm someone wrote can't.
pub fn decisions(e: &Expr, f: &mut impl FnMut(Decision)) {
    if let Some(d) = decision(e) {
        f(d);
    }
    if let ExprKind::Match { arms, .. } = &e.kind {
        if !desugared(e) {
            for g in arms.iter().filter_map(|a| a.guard.as_ref()) {
                f(Decision { kind: Kind::Guard, span: g.span, at: g.span.start, branches: 2 });
            }
        }
    }
    typed::children(e, &mut |c| decisions(c, f));
}

fn decision(e: &Expr) -> Option<Decision> {
    let (kind, at, branches) = match &e.kind {
        ExprKind::If { .. } => (Kind::If, e.span.start, 2),
        ExprKind::Match { arms, .. } if !desugared(e) => (Kind::Match, e.span.start, arms.len()),
        ExprKind::And { rhs, .. } => (Kind::And, rhs.span.start, 2),
        ExprKind::Or { rhs, .. } => (Kind::Or, rhs.span.start, 2),
        ExprKind::Try { .. } => (Kind::Try, e.span.end.saturating_sub(1), 2),
        _ => return None,
    };
    Some(Decision { kind, span: e.span, at, branches })
}

fn desugared(e: &Expr) -> bool {
    matches!(&e.kind, ExprKind::Match { arms, .. } if arms.first().is_some_and(|a| a.span == e.span))
}

/// Probes every branch of every decision [`decisions`] names whose span
/// `key_of` answers for. Runs after [`instrument`], so the line probes it adds
/// never see the `if`s it writes.
///
/// A probe per branch, except `?`, which has no place for one on its early
/// return: it counts how often its operand came back and how often execution
/// went on, and the report takes the difference.
pub fn branches(
    program: &mut Program,
    i64_ty: &Ty,
    bool_ty: &Ty,
    key_of: &dyn Fn(Kind, Span, usize) -> Option<u64>,
) {
    for f in &mut program.funcs {
        if let FuncKind::Body(body) = &mut f.kind {
            Branches { i64_ty, bool_ty, key_of, locals: &mut f.locals }.expr(body);
        }
    }
}

struct Branches<'a> {
    i64_ty: &'a Ty,
    bool_ty: &'a Ty,
    key_of: &'a dyn Fn(Kind, Span, usize) -> Option<u64>,
    locals: &'a mut Vec<typed::Local>,
}

impl Branches<'_> {
    fn expr(&mut self, e: &mut Expr) {
        typed::children_mut(e, &mut |c| self.expr(c));
        let Some(d) = decision(e) else {
            return;
        };
        let key_of = self.key_of;
        let key = |slot| key_of(d.kind, d.span, slot);
        // Every slot of a decision answers alike: the file decides.
        let (Some(first), Some(second)) = (key(0), key(1)) else {
            return;
        };
        let span = e.span;
        let ty = e.ty;
        if let ExprKind::Try { .. } = e.kind {
            // { let b = base; came back; let v = b?; went on; v }
            let mut tried = std::mem::replace(e, Expr::new(ExprKind::Local(LocalId(0)), ty, span));
            let ExprKind::Try { base, .. } = &mut tried.kind else { return };
            let b = self.local(base.ty, span);
            let operand_ty = base.ty;
            let operand = std::mem::replace(&mut **base, Expr::new(ExprKind::Local(b), operand_ty, span));
            let v = self.local(ty, span);
            let stmts = vec![
                bind(b, operand, span),
                Stmt::Expr(hit(second, self.i64_ty, span)),
                bind(v, tried, span),
                Stmt::Expr(hit(first, self.i64_ty, span)),
            ];
            let tail = Expr::new(ExprKind::Local(v), ty, span);
            *e = Expr::new(ExprKind::Block { stmts, tail: Some(Box::new(tail)) }, ty, span);
            return;
        }
        match &mut e.kind {
            ExprKind::If { then, else_, .. } => {
                hit_first(then, first, self.i64_ty);
                hit_first(else_, second, self.i64_ty);
            }
            ExprKind::Match { arms, .. } => {
                for (i, a) in arms.iter_mut().enumerate() {
                    if let Some(k) = key(i) {
                        hit_first(&mut a.body, k, self.i64_ty);
                    }
                    if let Some(g) = &mut a.guard {
                        let at = g.span;
                        if let (Some(yes), Some(no)) = (key_of(Kind::Guard, at, 0), key_of(Kind::Guard, at, 1)) {
                            let cond = std::mem::replace(g, self.bool(false, at));
                            *g = self.choose(at, cond, (yes, self.bool(true, at)), (no, self.bool(false, at)));
                        }
                    }
                }
            }
            ExprKind::And { lhs, rhs } => {
                let (lhs, rhs) = (take(lhs), take(rhs));
                *e = self.choose(span, lhs, (first, rhs), (second, self.bool(false, span)));
            }
            ExprKind::Or { lhs, rhs } => {
                let (lhs, rhs) = (take(lhs), take(rhs));
                *e = self.choose(span, lhs, (second, self.bool(true, span)), (first, rhs));
            }
            _ => {}
        }
    }

    /// `if (cond) { yes } else { no }`, each side counted first.
    fn choose(&self, span: Span, cond: Expr, yes: (u64, Expr), no: (u64, Expr)) -> Expr {
        let (mut then, mut else_) = (yes.1, no.1);
        hit_first(&mut then, yes.0, self.i64_ty);
        hit_first(&mut else_, no.0, self.i64_ty);
        Expr::new(
            ExprKind::If { cond: Box::new(cond), then: Box::new(then), else_: Box::new(else_) },
            *self.bool_ty,
            span,
        )
    }

    fn bool(&self, value: bool, span: Span) -> Expr {
        Expr::new(ExprKind::Bool(value), *self.bool_ty, span)
    }

    fn local(&mut self, ty: Ty, span: Span) -> LocalId {
        let id = LocalId(self.locals.len() as u32);
        self.locals.push(typed::Local { name: Name::new("coverage"), ty, span });
        id
    }
}

fn take(e: &mut Box<Expr>) -> Expr {
    let span = e.span;
    std::mem::replace(&mut **e, Expr::new(ExprKind::Unit, Ty::UNIT, span))
}

fn bind(local: LocalId, value: Expr, span: Span) -> Stmt {
    let pattern = Pattern { kind: PatKind::Bind { local, sub: None }, ty: value.ty, span };
    Stmt::Let { pattern, value, span }
}

/// Counts `key` before `e` runs.
fn hit_first(e: &mut Expr, key: u64, i64_ty: &Ty) {
    let span = e.span;
    let probe = Stmt::Expr(hit(key, i64_ty, span));
    if let ExprKind::Block { stmts, .. } = &mut e.kind {
        stmts.insert(0, probe);
        return;
    }
    let inner = std::mem::replace(e, Expr::new(ExprKind::Unit, Ty::UNIT, span));
    let ty = inner.ty;
    *e = Expr::new(ExprKind::Block { stmts: vec![probe], tail: Some(Box::new(inner)) }, ty, span);
}

fn hit(key: u64, i64_ty: &Ty, span: Span) -> Expr {
    let arg = Expr::new(ExprKind::Int(Magnitude::new(u128::from(key)), false), *i64_ty, span);
    Expr::new(
        ExprKind::Intrinsic { name: HIT.to_string(), targs: Vec::new(), args: vec![arg] },
        Ty::UNIT,
        span,
    )
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
        hit(key, self.i64_ty, span)
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
