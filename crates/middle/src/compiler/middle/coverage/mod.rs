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
use crate::compiler::semantics::typed::{self, Expr, ExprKind, Magnitude, PatKind, Pattern, PrimOp, Stmt};
use crate::compiler::semantics::types::{LocalId, Prim, Ty};
use std::collections::{HashMap, HashSet};

pub mod derived;
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
/// `instance` names a generic instantiation, under MC/DC only.
pub fn branch_key(file: &str, kind: Kind, (start, end): (u32, u32), slot: usize, instance: Option<&str>) -> u64 {
    let at = instance.map_or(String::new(), |i| format!("@{i}"));
    fnv(file, &format!("b{}:{start},{end},{slot}{at}", kind as u8))
}

/// The first of an MC/DC decision's keys: path `p` counts at this key plus
/// `p`. 52 bits, so the sum stays inside the 53 a JavaScript number holds.
pub fn mcdc_key(file: &str, (start, end): (u32, u32), instance: Option<&str>) -> u64 {
    let at = instance.map_or(String::new(), |i| format!("@{i}"));
    fnv(file, &format!("m:{start},{end}{at}")) & (KEY_MASK >> 1)
}

/// The most paths an MC/DC decision may have, about 30 conditions' worth. One
/// with more keeps its branches and isn't counted for MC/DC.
pub const MAX_PATHS: u64 = 1 << 32;

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
/// - `Try`: execution went on, `?` returned early;
/// - `Trap`, under MC/DC only: an integer `/` or `%`, or a `core/bits` shift,
///   went on, or aborted;
/// - `Derived`, under MC/DC only: a decision of a derived operation, whose
///   branches are named ([`derived`]).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Kind {
    If,
    Match,
    Guard,
    And,
    Or,
    Try,
    Trap,
    Derived,
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

/// Calls `f` with every decision in `body`, lambdas' included. With `traps`,
/// which says whether a call can abort, the abort points too.
///
/// The `match` that `semantics` writes for `a < b` isn't one: its arms carry
/// the operator's span, which an arm someone wrote can't.
pub fn decisions(e: &Expr, traps: Option<&dyn Fn(&Expr) -> bool>, f: &mut impl FnMut(Decision)) {
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
    if traps.is_some_and(|call| traps_here(e, call)) {
        f(Decision { kind: Kind::Trap, span: e.span, at: e.span.start, branches: 2 });
    }
    typed::children(e, &mut |c| decisions(c, traps, f));
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

/// Whether `e` is an operation that can abort: an integer `/` or `%`, or a
/// call `call` says can.
fn traps_here(e: &Expr, call: &dyn Fn(&Expr) -> bool) -> bool {
    match &e.kind {
        ExprKind::Prim { op: PrimOp::Div | PrimOp::Rem, prim, .. } => prim.is_integer(),
        ExprKind::CallFn { .. } => call(e),
        _ => false,
    }
}

/// An MC/DC decision: `&&`, `||` and `!` over conditions, with at least two
/// conditions. A condition is any other boolean expression, `!c` included.
#[derive(Clone, Debug)]
pub enum Tree {
    Condition(Span),
    And(Box<Tree>, Box<Tree>, Span),
    Or(Box<Tree>, Box<Tree>, Span),
    Not(Box<Tree>),
}

/// One path through a [`Tree`].
pub struct Path {
    /// Each condition's value, in source order; `None` where short-circuiting
    /// skipped it.
    pub values: Vec<Option<bool>>,
    pub outcome: bool,
    /// Each `&&` and `||` the path reached, by span, and whether its right side
    /// ran.
    pub sides: Vec<(Span, bool)>,
}

impl Tree {
    /// The decision `e` is the whole of, if any.
    pub fn of(e: &Expr) -> Option<Tree> {
        logical(e).then(|| Tree::build(e))
    }

    fn build(e: &Expr) -> Tree {
        match &e.kind {
            ExprKind::And { lhs, rhs } => Tree::And(Box::new(Tree::build(lhs)), Box::new(Tree::build(rhs)), e.span),
            ExprKind::Or { lhs, rhs } => Tree::Or(Box::new(Tree::build(lhs)), Box::new(Tree::build(rhs)), e.span),
            ExprKind::Prim { op: PrimOp::Not, args, .. } if logical(e) => {
                Tree::Not(Box::new(args.first().map_or(Tree::Condition(e.span), Tree::build)))
            }
            _ => Tree::Condition(e.span),
        }
    }

    /// How many paths end true, and how many end false.
    pub fn paths(&self) -> (u64, u64) {
        match self {
            Tree::Condition(_) => (1, 1),
            Tree::And(l, r, _) => {
                let ((tl, fl), (tr, fr)) = (l.paths(), r.paths());
                (tl.saturating_mul(tr), fl.saturating_add(tl.saturating_mul(fr)))
            }
            Tree::Or(l, r, _) => {
                let ((tl, fl), (tr, fr)) = (l.paths(), r.paths());
                (tl.saturating_add(fl.saturating_mul(tr)), fl.saturating_mul(fr))
            }
            Tree::Not(x) => {
                let (t, f) = x.paths();
                (f, t)
            }
        }
    }

    /// Whether MC/DC counts it: few enough paths to number.
    pub fn counted(&self) -> bool {
        let (t, f) = self.paths();
        t.saturating_add(f) <= MAX_PATHS
    }

    /// Every condition's span, in source order.
    pub fn conditions(&self, out: &mut Vec<Span>) {
        match self {
            Tree::Condition(s) => out.push(*s),
            Tree::And(l, r, _) | Tree::Or(l, r, _) => {
                l.conditions(out);
                r.conditions(out);
            }
            Tree::Not(x) => x.conditions(out),
        }
    }

    fn count(&self) -> usize {
        match self {
            Tree::Condition(_) => 1,
            Tree::And(l, r, _) | Tree::Or(l, r, _) => l.count().saturating_add(r.count()),
            Tree::Not(x) => x.count(),
        }
    }

    /// The path numbered `index`, the inverse of the numbering
    /// [`branches`] computes at run time.
    pub fn decode(&self, index: u64) -> Path {
        let mut path = Path { values: vec![None; self.count()], outcome: false, sides: Vec::new() };
        path.outcome = self.walk(index, 0, &mut path);
        path
    }

    #[allow(clippy::arithmetic_side_effects, reason = "an index below `paths()`, which `counted` keeps far from overflow")]
    fn walk(&self, i: u64, first: usize, path: &mut Path) -> bool {
        match self {
            Tree::Condition(_) => {
                if let Some(v) = path.values.get_mut(first) {
                    *v = Some(i == 0);
                }
                i == 0
            }
            Tree::And(l, r, span) => {
                let ((tl, fl), (tr, fr)) = (l.paths(), r.paths());
                let next = first + l.count();
                if i < tl * tr {
                    l.walk(i / tr, first, path);
                    r.walk(i % tr, next, path);
                    path.sides.push((*span, true));
                    true
                } else if i < tl * tr + fl {
                    l.walk(tl + (i - tl * tr), first, path);
                    path.sides.push((*span, false));
                    false
                } else {
                    let j = i - tl * tr - fl;
                    l.walk(j / fr, first, path);
                    r.walk(tr + j % fr, next, path);
                    path.sides.push((*span, true));
                    false
                }
            }
            Tree::Or(l, r, span) => {
                let ((tl, fl), (tr, fr)) = (l.paths(), r.paths());
                let next = first + l.count();
                let true_paths = tl + fl * tr;
                if i < tl {
                    l.walk(i, first, path);
                    path.sides.push((*span, false));
                    true
                } else if i < true_paths {
                    let j = i - tl;
                    l.walk(tl + j / tr, first, path);
                    r.walk(j % tr, next, path);
                    path.sides.push((*span, true));
                    true
                } else {
                    let j = i - true_paths;
                    l.walk(tl + j / fr, first, path);
                    r.walk(tr + j % fr, next, path);
                    path.sides.push((*span, true));
                    false
                }
            }
            Tree::Not(x) => {
                let (tx, fx) = x.paths();
                if i < fx { !x.walk(tx + i, first, path) } else { !x.walk(i - fx, first, path) }
            }
        }
    }
}

/// Whether `e` is `&&`, `||`, or `!` over one of those.
fn logical(e: &Expr) -> bool {
    match &e.kind {
        ExprKind::And { .. } | ExprKind::Or { .. } => true,
        ExprKind::Prim { op: PrimOp::Not, args, .. } => args.first().is_some_and(logical),
        _ => false,
    }
}

/// Calls `f` with every MC/DC decision in `body`, lambdas' included.
pub fn trees(e: &Expr, f: &mut impl FnMut(Tree)) {
    trees_at(e, true, f);
}

/// [`trees`], knowing whether `e` is in tail position.
fn trees_at(e: &Expr, tail: bool, f: &mut impl FnMut(Tree)) {
    if let Some(tree) = Tree::of(e) {
        let counted = tree.counted() && !(tail && ends_in_tail_call(e));
        if counted {
            f(tree);
        }
        conditions(e, counted || !tail, &mut |c, t| trees_at(c, t, f));
        return;
    }
    each_child(e, tail, &mut |c, t| trees_at(c, t, f));
}

/// Whether the decision `e` ends on a condition that makes a call in tail
/// position. Recording that condition's value would cost the call the
/// constant stack a tail call promises, so MC/DC leaves the decision to its
/// branches.
fn ends_in_tail_call(e: &Expr) -> bool {
    match &e.kind {
        ExprKind::And { rhs, .. } | ExprKind::Or { rhs, .. } => ends_in_tail_call(rhs),
        _ if logical(e) => false,
        _ => calls_last(e),
    }
}

/// Whether `e` can end in a call whose answer is `e`'s.
fn calls_last(e: &Expr) -> bool {
    match &e.kind {
        ExprKind::CallFn { .. } | ExprKind::CallValue { .. } | ExprKind::CallTrait { .. } => true,
        ExprKind::Block { tail: Some(t), .. } => calls_last(t),
        ExprKind::If { then, else_, .. } => calls_last(then) || calls_last(else_),
        ExprKind::Match { arms, .. } => arms.iter().any(|a| calls_last(&a.body)),
        ExprKind::And { rhs, .. } | ExprKind::Or { rhs, .. } => calls_last(rhs),
        _ => false,
    }
}

/// Calls `f` with each child of `e` and whether it's in tail position, given
/// whether `e` is.
fn each_child(e: &Expr, tail: bool, f: &mut impl FnMut(&Expr, bool)) {
    match &e.kind {
        ExprKind::Block { stmts, tail: last } => {
            for s in stmts {
                match s {
                    Stmt::Let { value, .. } => f(value, false),
                    Stmt::Expr(x) => f(x, false),
                }
            }
            if let Some(t) = last {
                f(t, tail);
            }
        }
        ExprKind::If { cond, then, else_ } => {
            f(cond, false);
            f(then, tail);
            f(else_, tail);
        }
        ExprKind::Match { scrutinee, arms } => {
            f(scrutinee, false);
            for a in arms {
                if let Some(g) = &a.guard {
                    f(g, false);
                }
                f(&a.body, tail);
            }
        }
        ExprKind::Lambda { body, .. } => f(body, true),
        ExprKind::And { lhs, rhs } | ExprKind::Or { lhs, rhs } => {
            f(lhs, false);
            f(rhs, tail);
        }
        _ => typed::children(e, &mut |c| f(c, false)),
    }
}

/// [`each_child`], mutably.
fn each_child_mut(e: &mut Expr, tail: bool, f: &mut impl FnMut(&mut Expr, bool)) {
    match &mut e.kind {
        ExprKind::Block { stmts, tail: last } => {
            for s in stmts {
                match s {
                    Stmt::Let { value, .. } => f(value, false),
                    Stmt::Expr(x) => f(x, false),
                }
            }
            if let Some(t) = last {
                f(t, tail);
            }
        }
        ExprKind::If { cond, then, else_ } => {
            f(cond, false);
            f(then, tail);
            f(else_, tail);
        }
        ExprKind::Match { scrutinee, arms } => {
            f(scrutinee, false);
            for a in arms {
                if let Some(g) = &mut a.guard {
                    f(g, false);
                }
                f(&mut a.body, tail);
            }
        }
        ExprKind::Lambda { body, .. } => f(body, true),
        ExprKind::And { lhs, rhs } | ExprKind::Or { lhs, rhs } => {
            f(lhs, false);
            f(rhs, tail);
        }
        _ => typed::children_mut(e, &mut |c| f(c, false)),
    }
}

/// Calls `f` with each condition of the decision `e`, and whether it's in
/// tail position: only the last can be, and only if `e` is and the decision
/// isn't counted, which reads its value.
fn conditions<'a>(e: &'a Expr, settled: bool, f: &mut impl FnMut(&'a Expr, bool)) {
    match &e.kind {
        ExprKind::And { lhs, rhs } | ExprKind::Or { lhs, rhs } => {
            conditions(lhs, true, f);
            conditions(rhs, settled, f);
        }
        ExprKind::Prim { op: PrimOp::Not, args, .. } if logical(e) => args.iter().for_each(|a| conditions(a, true, f)),
        _ => f(e, !settled),
    }
}

/// [`conditions`], mutably.
fn conditions_mut(e: &mut Expr, settled: bool, f: &mut impl FnMut(&mut Expr, bool)) {
    if !logical(e) {
        f(e, !settled);
        return;
    }
    match &mut e.kind {
        ExprKind::And { lhs, rhs } | ExprKind::Or { lhs, rhs } => {
            conditions_mut(lhs, true, f);
            conditions_mut(rhs, settled, f);
        }
        ExprKind::Prim { args, .. } => args.iter_mut().for_each(|a| conditions_mut(a, true, f)),
        _ => {}
    }
}

/// How [`branches`] probes.
pub struct Options<'a> {
    pub i64_ty: Ty,
    pub bool_ty: Ty,
    /// The user file a span is in, or `None` outside the user's source.
    pub file_of: &'a dyn Fn(Span) -> Option<String>,
    /// `Some` under MC/DC: each generic instantiation's name, by slot.
    pub mcdc: Option<&'a HashMap<usize, String>>,
    /// Under MC/DC, for the `derive`s in the user's source.
    pub tables: Option<&'a crate::compiler::semantics::types::Tables>,
}

/// Probes every branch of every decision [`decisions`] names in the user's
/// source. Runs after [`instrument`], so the line probes it adds never see the
/// `if`s it writes.
///
/// A probe per branch, except `?` and an abort, which have no place for one on
/// the way out: each counts how often it was reached and how often execution
/// went on, and the report takes the difference.
///
/// Under MC/DC an `&&`/`||` decision counts its path instead: each condition
/// adds to a number only that path reaches, and the report reads both the
/// conditions and the right sides from the paths that ran.
pub fn branches(program: &mut Program, options: &Options) -> Vec<derived::Derived> {
    let traps: HashSet<usize> = match options.mcdc {
        Some(_) => program
            .funcs
            .iter()
            .enumerate()
            .filter(|(_, f)| (options.file_of)(f.span).is_none() && aborts(f))
            .map(|(i, _)| i)
            .collect(),
        None => HashSet::default(),
    };
    for (slot, f) in program.funcs.iter_mut().enumerate() {
        if let FuncKind::Body(body) = &mut f.kind {
            let instance = options.mcdc.and_then(|names| names.get(&slot)).map(String::as_str);
            Branches { options, instance, traps: &traps, locals: &mut f.locals }.expr(body, true);
        }
    }
    // Last, so the shadows' own branches aren't taken for the user's.
    match (options.mcdc, options.tables) {
        (Some(_), Some(tables)) => derived::run(program, tables, options.file_of, options.i64_ty, options.bool_ty),
        _ => Vec::new(),
    }
}

/// Whether a function outside the user's source is one a call to can abort: a
/// `core/bits` shift, or a body that is an integer `/` or `%`, as a generic
/// `a / b` reaches at an integer type.
pub fn aborts(f: &crate::compiler::middle::monomorphize::Func) -> bool {
    match &f.kind {
        FuncKind::Intrinsic(key) => key.starts_with("bits.shift"),
        FuncKind::Body(body) => matches!(
            &body.kind,
            ExprKind::Prim { op: PrimOp::Div | PrimOp::Rem, prim, .. } if prim.is_integer()
        ),
        FuncKind::Unbuilt => false,
    }
}

struct Branches<'a> {
    options: &'a Options<'a>,
    instance: Option<&'a str>,
    traps: &'a HashSet<usize>,
    locals: &'a mut Vec<typed::Local>,
}

impl Branches<'_> {
    fn key(&self, kind: Kind, span: Span, slot: usize) -> Option<u64> {
        let file = (self.options.file_of)(span)?;
        Some(branch_key(&file, kind, (span.start, span.end), slot, self.instance))
    }

    /// Probes `e`, given whether it's in tail position.
    fn expr(&mut self, e: &mut Expr, tail: bool) {
        if self.options.mcdc.is_some() {
            if let Some(tree) = Tree::of(e) {
                let counted = tree.counted() && !(tail && ends_in_tail_call(e));
                conditions_mut(e, counted || !tail, &mut |c, t| self.expr(c, t));
                if counted {
                    self.decision(e);
                    return;
                }
                // Too many paths to number, or a tail call to keep: the right
                // sides get probes of their own, as without MC/DC.
                self.sides(e);
                return;
            }
        }
        each_child_mut(e, tail, &mut |c, t| self.expr(c, t));
        if self.options.mcdc.is_some() && traps_here(e, &|call| self.calls_trap(call)) {
            self.trap(e);
            return;
        }
        let Some(d) = decision(e) else {
            return;
        };
        let (Some(first), Some(second)) = (self.key(d.kind, d.span, 0), self.key(d.kind, d.span, 1)) else {
            return;
        };
        let span = e.span;
        let ty = e.ty;
        match &mut e.kind {
            ExprKind::Try { base, .. } => {
                // { let b = base; came back; let v = b?; went on; v }
                let operand_ty = base.ty;
                let b = self.local(operand_ty, span);
                let ExprKind::Try { base, .. } = &mut e.kind else { return };
                let operand = std::mem::replace(&mut **base, Expr::new(ExprKind::Local(b), operand_ty, span));
                let tried = std::mem::replace(e, Expr::new(ExprKind::Unit, ty, span));
                *e = self.reached(vec![bind(b, operand, span)], tried, second, first);
            }
            ExprKind::If { then, else_, .. } => {
                hit_first(then, first, &self.options.i64_ty);
                hit_first(else_, second, &self.options.i64_ty);
            }
            ExprKind::Match { arms, .. } => {
                for (i, a) in arms.iter_mut().enumerate() {
                    if let Some(k) = self.key(Kind::Match, span, i) {
                        hit_first(&mut a.body, k, &self.options.i64_ty);
                    }
                    let Some(g) = &mut a.guard else { continue };
                    let at = g.span;
                    if let (Some(yes), Some(no)) = (self.key(Kind::Guard, at, 0), self.key(Kind::Guard, at, 1)) {
                        let cond = std::mem::replace(g, self.bool(false, at));
                        *g = self.choose(at, cond, (yes, self.bool(true, at)), (no, self.bool(false, at)));
                    }
                }
            }
            ExprKind::And { .. } | ExprKind::Or { .. } => self.side(e, first, second),
            _ => {}
        }
    }

    /// Probes each `&&` and `||` in the decision `e`, as without MC/DC.
    fn sides(&mut self, e: &mut Expr) {
        match &mut e.kind {
            ExprKind::And { lhs, rhs } | ExprKind::Or { lhs, rhs } => {
                self.sides(lhs);
                self.sides(rhs);
            }
            ExprKind::Prim { op: PrimOp::Not, args, .. } => args.iter_mut().for_each(|a| self.sides(a)),
            _ => return,
        }
        let Some(d) = decision(e) else { return };
        if let (Some(first), Some(second)) = (self.key(d.kind, d.span, 0), self.key(d.kind, d.span, 1)) {
            self.side(e, first, second);
        }
    }

    /// `a && b` as `if (a) { ran; b } else { skipped; false }`, and `||` alike.
    fn side(&mut self, e: &mut Expr, ran: u64, skipped: u64) {
        let span = e.span;
        let (t, f) = (self.bool(true, span), self.bool(false, span));
        *e = match std::mem::replace(&mut e.kind, ExprKind::Unit) {
            ExprKind::And { lhs, rhs } => self.choose(span, *lhs, (ran, *rhs), (skipped, f)),
            ExprKind::Or { lhs, rhs } => self.choose(span, *lhs, (skipped, t), (ran, *rhs)),
            kind => Expr::new(kind, e.ty, span),
        };
    }

    /// The decision `e` as `{ let p = <its path>; count p; p < true paths }`.
    fn decision(&mut self, e: &mut Expr) {
        let span = e.span;
        let Some(file) = (self.options.file_of)(span) else { return };
        let base = mcdc_key(&file, (span.start, span.end), self.instance);
        let whole = std::mem::replace(e, Expr::new(ExprKind::Unit, self.options.bool_ty, span));
        let (index, trues, _) = self.path(whole);
        let p = self.local(self.options.i64_ty, span);
        let key = self.prim(PrimOp::Add, vec![self.int(base, span), self.get(p, span)], span);
        let hit = Expr::new(
            ExprKind::Intrinsic { name: HIT.to_string(), targs: Vec::new(), args: vec![key] },
            Ty::UNIT,
            span,
        );
        let outcome = self.prim(PrimOp::Lt, vec![self.get(p, span), self.int(trues, span)], span);
        let stmts = vec![bind(p, index, span), Stmt::Expr(hit)];
        *e = Expr::new(ExprKind::Block { stmts, tail: Some(Box::new(outcome)) }, self.options.bool_ty, span);
    }

    /// An `I64` expression for the path `e` takes, numbered as [`Tree::decode`]
    /// reads it, with how many paths end true and how many false. True paths
    /// come first.
    #[allow(clippy::arithmetic_side_effects, reason = "path counts of a decision `Tree::counted` admits, far below overflow")]
    fn path(&mut self, e: Expr) -> (Expr, u64, u64) {
        let span = e.span;
        let not_operand = match &e.kind {
            ExprKind::Prim { op: PrimOp::Not, args, .. } if logical(&e) => !args.is_empty(),
            _ => false,
        };
        match e.kind {
            ExprKind::And { lhs, rhs } => {
                let (la, tl, fl) = self.path(*lhs);
                let (rb, tr, fr) = self.path(*rhs);
                let (a, b) = (self.local(self.options.i64_ty, span), self.local(self.options.i64_ty, span));
                // r true: a*tr + b. r false: tl*tr + fl + a*fr + (b - tr).
                // l false: tl*tr + (a - tl).
                let both = self.affine(a, tr, Some((b, 0)), 0, span);
                let r_false = self.affine(a, fr, Some((b, tr)), tl * tr + fl, span);
                let l_false = self.offset(a, tl, tl * tr, span);
                let inner = self.split(b, tr, both, r_false, span);
                let inner = self.with(b, rb, inner, span);
                let body = self.split(a, tl, inner, l_false, span);
                (self.with(a, la, body, span), tl * tr, fl + tl * fr)
            }
            ExprKind::Or { lhs, rhs } => {
                let (la, tl, fl) = self.path(*lhs);
                let (rb, tr, fr) = self.path(*rhs);
                let (a, b) = (self.local(self.options.i64_ty, span), self.local(self.options.i64_ty, span));
                let trues = tl + fl * tr;
                // l true: a. r true: tl + (a - tl)*tr + b. Both false:
                // trues + (a - tl)*fr + (b - tr).
                let l_true = self.get(a, span);
                let shifted = self.offset(a, tl, 0, span);
                let r_true = self.combine(shifted.clone(), tr, b, 0, tl, span);
                let r_false = self.combine(shifted, fr, b, tr, trues, span);
                let inner = self.split(b, tr, r_true, r_false, span);
                let inner = self.with(b, rb, inner, span);
                let body = self.split(a, tl, l_true, inner, span);
                (self.with(a, la, body, span), trues, fl * fr)
            }
            ExprKind::Prim { op: PrimOp::Not, args, .. } if not_operand => {
                let operand = args.into_iter().next().unwrap_or_else(|| Expr::new(ExprKind::Unit, Ty::UNIT, span));
                let (xa, tx, fx) = self.path(operand);
                let a = self.local(self.options.i64_ty, span);
                // x true: fx + a. x false: a - tx.
                let x_true = self.offset(a, 0, fx, span);
                let x_false = self.offset(a, tx, 0, span);
                let body = self.split(a, tx, x_true, x_false, span);
                (self.with(a, xa, body, span), fx, tx)
            }
            kind => {
                let cond = Expr::new(kind, self.options.bool_ty, span);
                let pick = Expr::new(
                    ExprKind::If {
                        cond: Box::new(cond),
                        then: Box::new(self.int(0, span)),
                        else_: Box::new(self.int(1, span)),
                    },
                    self.options.i64_ty,
                    span,
                );
                (pick, 1, 1)
            }
        }
    }

    /// `{ let a = value; body }`.
    fn with(&self, a: LocalId, value: Expr, body: Expr, span: Span) -> Expr {
        Expr::new(ExprKind::Block { stmts: vec![bind(a, value, span)], tail: Some(Box::new(body)) }, self.options.i64_ty, span)
    }

    /// `if (a < bound) { below } else { above }`.
    fn split(&self, a: LocalId, bound: u64, below: Expr, above: Expr, span: Span) -> Expr {
        let cond = self.prim(PrimOp::Lt, vec![self.get(a, span), self.int(bound, span)], span);
        Expr::new(
            ExprKind::If { cond: Box::new(cond), then: Box::new(below), else_: Box::new(above) },
            self.options.i64_ty,
            span,
        )
    }

    /// `a - minus + plus`, written without the zeros.
    #[allow(clippy::arithmetic_side_effects, reason = "each subtraction is guarded by the comparison before it")]
    fn offset(&self, a: LocalId, minus: u64, plus: u64, span: Span) -> Expr {
        let mut x = self.get(a, span);
        if minus > plus {
            x = self.prim(PrimOp::Sub, vec![x, self.int(minus - plus, span)], span);
        } else if plus > minus {
            x = self.prim(PrimOp::Add, vec![x, self.int(plus - minus, span)], span);
        }
        x
    }

    /// `a*scale + (b - minus) + plus`.
    fn affine(&self, a: LocalId, scale: u64, b: Option<(LocalId, u64)>, plus: u64, span: Span) -> Expr {
        let x = self.get(a, span);
        match b {
            Some((b, minus)) => self.combine(x, scale, b, minus, plus, span),
            None => x,
        }
    }

    /// `x*scale + (b - minus) + plus`.
    fn combine(&self, x: Expr, scale: u64, b: LocalId, minus: u64, plus: u64, span: Span) -> Expr {
        let scaled = if scale == 1 { x } else { self.prim(PrimOp::Mul, vec![x, self.int(scale, span)], span) };
        self.prim(PrimOp::Add, vec![scaled, self.offset(b, minus, plus, span)], span)
    }

    fn int(&self, n: u64, span: Span) -> Expr {
        Expr::new(ExprKind::Int(Magnitude::new(u128::from(n)), false), self.options.i64_ty, span)
    }

    fn get(&self, a: LocalId, span: Span) -> Expr {
        Expr::new(ExprKind::Local(a), self.options.i64_ty, span)
    }

    fn prim(&self, op: PrimOp, args: Vec<Expr>, span: Span) -> Expr {
        let ty = if matches!(op, PrimOp::Lt) { self.options.bool_ty } else { self.options.i64_ty };
        Expr::new(ExprKind::Prim { op, prim: Prim::I64, args }, ty, span)
    }

    fn calls_trap(&self, call: &Expr) -> bool {
        matches!(&call.kind, ExprKind::CallFn { func: typed::Callee::Func(f), .. } if self.traps.contains(&(f.0 as usize)))
    }

    /// An abort point as `{ let a = x; ...; reached; let v = op(a, ...); went on; v }`.
    fn trap(&mut self, e: &mut Expr) {
        let span = e.span;
        let (Some(went_on), Some(reached)) = (self.key(Kind::Trap, span, 0), self.key(Kind::Trap, span, 1)) else {
            return;
        };
        let mut op = std::mem::replace(e, Expr::new(ExprKind::Unit, Ty::UNIT, span));
        let args = match &mut op.kind {
            ExprKind::Prim { args, .. } | ExprKind::CallFn { args, .. } => args,
            _ => return,
        };
        let mut stmts = Vec::with_capacity(args.len());
        for arg in args.iter_mut() {
            let ty = arg.ty;
            let a = self.local(ty, span);
            let value = std::mem::replace(arg, Expr::new(ExprKind::Local(a), ty, span));
            stmts.push(bind(a, value, span));
        }
        *e = self.reached(stmts, op, reached, went_on);
    }

    /// `{ stmts; reached; let v = op; went on; v }`.
    fn reached(&mut self, mut stmts: Vec<Stmt>, op: Expr, reached: u64, went_on: u64) -> Expr {
        let (span, ty) = (op.span, op.ty);
        let v = self.local(ty, span);
        stmts.push(Stmt::Expr(hit(reached, &self.options.i64_ty, span)));
        stmts.push(bind(v, op, span));
        stmts.push(Stmt::Expr(hit(went_on, &self.options.i64_ty, span)));
        let tail = Expr::new(ExprKind::Local(v), ty, span);
        Expr::new(ExprKind::Block { stmts, tail: Some(Box::new(tail)) }, ty, span)
    }

    /// `if (cond) { yes } else { no }`, each side counted first.
    fn choose(&self, span: Span, cond: Expr, yes: (u64, Expr), no: (u64, Expr)) -> Expr {
        let (mut then, mut else_) = (yes.1, no.1);
        hit_first(&mut then, yes.0, &self.options.i64_ty);
        hit_first(&mut else_, no.0, &self.options.i64_ty);
        Expr::new(
            ExprKind::If { cond: Box::new(cond), then: Box::new(then), else_: Box::new(else_) },
            self.options.bool_ty,
            span,
        )
    }

    fn bool(&self, value: bool, span: Span) -> Expr {
        Expr::new(ExprKind::Bool(value), self.options.bool_ty, span)
    }

    fn local(&mut self, ty: Ty, span: Span) -> LocalId {
        let id = LocalId(self.locals.len() as u32);
        self.locals.push(typed::Local { name: Name::new("coverage"), ty, span });
        id
    }
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
