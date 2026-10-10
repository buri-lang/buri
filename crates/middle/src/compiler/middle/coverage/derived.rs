//! A `derive`'s branches, under `buri test --coverage=mcdc`.
//!
//! A derived operation is a run-time walk on JavaScript and generated code
//! natively, so the two have no branch in common to probe. Instead, every call
//! of one on a type the user derives it for first calls a **shadow**: a plain
//! function that walks the value the way the derived code does, counts each
//! branch it takes, and answers nothing the program reads. The call itself is
//! untouched, so the program computes what it always did, and every backend runs
//! the same shadow.
//!
//! The branches, per operation on a type:
//!
//! - `Equal`, `Ordered` on a struct: after each field but the last, the walk
//!   went on to the next or stopped there;
//! - on an enum: which variant the first value is, whether the second is the
//!   same one, and then its fields, as a struct's;
//! - `Show`, `Hash`, `ToJson` on an enum: which variant.
//!
//! A field of a type the user derives the same operation for is walked by that
//! type's shadow. Anything else, a list of them included, is compared the way
//! the derived code compares it and not walked.

use crate::compiler::middle::monomorphize::{Desc, Func, FuncKind, Program};
use crate::compiler::semantics::name::Name;
use crate::compiler::semantics::typed::{self, Arm, Expr, ExprKind, FieldPat, Magnitude, PatKind, Pattern, PrimOp, Stmt};
use crate::compiler::semantics::types::{self, FuncIdx, LocalId, Tables, Ty, TyConId, TyDef, TyKind};
use crate::diagnostics::Span;
use std::collections::HashMap;

/// One decision of a derived operation: the `derive` it belongs to, what it
/// is, and its branches' names.
#[derive(Clone, Debug)]
pub struct Derived {
    pub span: Span,
    /// The operation and type, as in `Equal Point`.
    pub group: String,
    /// The decision's place in its operation's walk.
    pub order: usize,
    pub label: String,
    pub branches: Vec<String>,
}

/// The key for one branch of a derived decision.
pub fn derived_key(file: &str, (start, end): (u32, u32), label: &str, slot: usize) -> u64 {
    super::fnv(file, &format!("d:{start},{end}:{label}:{slot}"))
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Op {
    Equal,
    Ordered,
    Show,
    Hash,
    ToJson,
}

impl Op {
    fn of_trait(name: &str) -> Option<Op> {
        Some(match name {
            "Equal" => Op::Equal,
            "Ordered" => Op::Ordered,
            "Show" => Op::Show,
            "Hash" => Op::Hash,
            "ToJson" => Op::ToJson,
            _ => return None,
        })
    }

    fn of_intrinsic(name: &str) -> Option<Op> {
        Some(match name {
            "structuralEq" => Op::Equal,
            "structuralCompare" => Op::Ordered,
            "structuralShow" => Op::Show,
            "structuralHash" => Op::Hash,
            "structuralToJson" => Op::ToJson,
            _ => return None,
        })
    }

    fn name(self) -> &'static str {
        match self {
            Op::Equal => "Equal",
            Op::Ordered => "Ordered",
            Op::Show => "Show",
            Op::Hash => "Hash",
            Op::ToJson => "ToJson",
        }
    }

    /// How many values a call passes: two to compare, or one.
    fn values(self) -> usize {
        match self {
            Op::Equal | Op::Ordered => 2,
            Op::Show | Op::Hash | Op::ToJson => 1,
        }
    }
}

/// The operation a call is, and the type it's at.
fn call(e: &Expr) -> Option<(Op, Ty)> {
    match &e.kind {
        ExprKind::StructuralEq { args, .. } => Some((Op::Equal, args.first()?.ty)),
        ExprKind::Intrinsic { name, args, .. } => Some((Op::of_intrinsic(name)?, args.first()?.ty)),
        _ => None,
    }
}

/// Adds a shadow for every derived operation `program` calls on a type the
/// user's source derives it for, and a call to it before each such call.
/// Answers the decisions the shadows count.
pub fn run(
    program: &mut Program,
    tables: &Tables,
    file_of: &dyn Fn(Span) -> Option<String>,
    i64_ty: Ty,
    bool_ty: Ty,
) -> Vec<Derived> {
    let mut derives: HashMap<(TyConId, Op), Span> = HashMap::new();
    for ((trait_id, con), info) in tables.impls.iter() {
        if !info.is_derived() || file_of(info.span).is_none() {
            continue;
        }
        if let Some(op) = Op::of_trait(&tables.trait_(*trait_id).name) {
            derives.insert((*con, op), info.span);
        }
    }
    if derives.is_empty() {
        return Vec::new();
    }
    let mut order = None;
    let mut wanted: Vec<(Op, Ty)> = Vec::new();
    for f in &program.funcs {
        let Some(body) = f.body() else { continue };
        typed::walk(body, &mut |e| {
            if let ExprKind::Intrinsic { name, .. } = &e.kind {
                if name == "structuralCompare" {
                    order = Some(e.ty);
                }
            }
            if let Some((op, ty)) = call(e) {
                if ty.head().is_some_and(|c| derives.contains_key(&(c, op))) && !wanted.contains(&(op, ty)) {
                    wanted.push((op, ty));
                }
            }
        });
    }
    let original = program.funcs.len();
    let mut s = Shadows {
        tables,
        file_of,
        i64_ty,
        bool_ty,
        order,
        derives,
        desc_index: &program.desc_index,
        descriptors: &program.descriptors,
        base: original,
        made: HashMap::new(),
        queue: Vec::new(),
        funcs: Vec::new(),
        out: Vec::new(),
    };
    for (op, ty) in &wanted {
        s.request(*op, *ty);
    }
    s.drain();
    let (made, funcs, out) = (s.made, s.funcs, s.out);
    program.funcs.extend(funcs);
    for f in program.funcs.iter_mut().take(original) {
        if let FuncKind::Body(body) = &mut f.kind {
            precede(body, &made, &mut f.locals);
        }
    }
    out
}

/// Puts a call to the shadow before each derived call `made` has one for.
fn precede(e: &mut Expr, made: &HashMap<(Op, Ty), Option<(FuncIdx, Ty)>>, locals: &mut Vec<typed::Local>) {
    typed::children_mut(e, &mut |c| precede(c, made, locals));
    let Some((op, ty)) = call(e) else { return };
    let Some(Some((shadow, ret))) = made.get(&(op, ty)) else { return };
    let span = e.span;
    let args = match &mut e.kind {
        ExprKind::StructuralEq { args, .. } | ExprKind::Intrinsic { args, .. } => args,
        _ => return,
    };
    let mut stmts = Vec::new();
    let mut values = Vec::new();
    for arg in args.iter_mut().take(op.values()) {
        let id = LocalId(locals.len() as u32);
        locals.push(typed::Local { name: Name::new("coverage"), ty: arg.ty, span });
        let value = std::mem::replace(arg, Expr::new(ExprKind::Local(id), arg.ty, span));
        values.push(Expr::new(ExprKind::Local(id), value.ty, span));
        stmts.push(bind(id, value, span));
    }
    let shadowed = Expr::new(ExprKind::CallFn { func: typed::Callee::Func(*shadow), args: values }, *ret, span);
    stmts.push(Stmt::Let { pattern: Pattern { kind: PatKind::Wild, ty: *ret, span }, value: shadowed, span });
    let ty = e.ty;
    let inner = std::mem::replace(e, Expr::new(ExprKind::Unit, ty, span));
    *e = Expr::new(ExprKind::Block { stmts, tail: Some(Box::new(inner)) }, ty, span);
}

fn bind(local: LocalId, value: Expr, span: Span) -> Stmt {
    let pattern = Pattern { kind: PatKind::Bind { local, sub: None }, ty: value.ty, span };
    Stmt::Let { pattern, value, span }
}

struct Shadows<'a> {
    tables: &'a Tables,
    file_of: &'a dyn Fn(Span) -> Option<String>,
    i64_ty: Ty,
    bool_ty: Ty,
    /// `Order`, from a call that compares.
    order: Option<Ty>,
    derives: HashMap<(TyConId, Op), Span>,
    desc_index: &'a crate::hash::Map<Ty, usize>,
    descriptors: &'a [Desc],
    /// Where the shadows start in `Program::funcs`.
    base: usize,
    /// Each shadow and what it answers, or `None` for one that can't be built.
    made: HashMap<(Op, Ty), Option<(FuncIdx, Ty)>>,
    queue: Vec<(Op, Ty, usize)>,
    funcs: Vec<Func>,
    out: Vec<Derived>,
}

/// One shadow's body under construction.
struct Body {
    i64_ty: Ty,
    locals: Vec<typed::Local>,
    span: Span,
    file: String,
    prefix: String,
    decisions: Vec<Derived>,
}

impl Shadows<'_> {
    /// The shadow of `op` at `ty`, made on first request.
    fn request(&mut self, op: Op, ty: Ty) -> Option<(FuncIdx, Ty)> {
        if let Some(made) = self.made.get(&(op, ty)) {
            return *made;
        }
        let ret = match op {
            Op::Equal | Op::Ordered => self.bool_ty,
            Op::Show | Op::Hash | Op::ToJson => Ty::UNIT,
        };
        let slot = self.funcs.len();
        let idx = FuncIdx(self.base.saturating_add(slot) as u32);
        self.funcs.push(Func {
            symbol: format!("coverage$derived${slot}"),
            debug_name: format!("coverage: {} at {}", op.name(), types::show(self.tables, None, &[], &ty)),
            params: Vec::new(),
            locals: Vec::new(),
            kind: FuncKind::Unbuilt,
            ret,
            desc: None,
            span: Span::NONE,
        });
        self.made.insert((op, ty), Some((idx, ret)));
        self.queue.push((op, ty, slot));
        Some((idx, ret))
    }

    fn drain(&mut self) {
        while let Some((op, ty, slot)) = self.queue.pop() {
            if self.build(op, ty, slot).is_none() {
                // A shadow that can't be built is never called; it stays a
                // function that does nothing, and nobody counts its decisions.
                self.made.insert((op, ty), None);
                if let Some(f) = self.funcs.get_mut(slot) {
                    let unit = Expr::new(ExprKind::Unit, Ty::UNIT, Span::NONE);
                    let answer = if f.ret == Ty::UNIT { unit } else { Expr::new(ExprKind::Bool(true), f.ret, Span::NONE) };
                    f.params = Vec::new();
                    f.locals = Vec::new();
                    f.kind = FuncKind::Body(answer);
                }
            }
        }
        // A shadow that calls one that couldn't be built would count only half
        // a walk; there's none, because a failure fails its caller too.
    }

    fn build(&mut self, op: Op, ty: Ty, slot: usize) -> Option<()> {
        let con = ty.head()?;
        let span = *self.derives.get(&(con, op))?;
        let file = (self.file_of)(span)?;
        let prefix = format!("{} {}", op.name(), types::show(self.tables, None, &[], &ty));
        let mut body = Body { i64_ty: self.i64_ty, locals: Vec::new(), span, file, prefix, decisions: Vec::new() };
        let a = body.local(ty);
        let b = (op.values() == 2).then(|| body.local(ty));
        let targs: Vec<Ty> = match ty.kind() {
            TyKind::Con(_, args) => args.to_vec(),
            _ => Vec::new(),
        };
        let tycon = self.tables.tycon(con);
        let expr = match (&tycon.def, b) {
            (TyDef::Struct { fields, .. }, Some(b)) => {
                let names: Vec<String> = fields.iter().map(|f| f.name.clone()).collect();
                let types: Vec<Ty> = fields.iter().map(|f| types::substitute(&f.ty, &targs, None)).collect();
                let pairs: Vec<(Expr, Expr)> = types
                    .iter()
                    .enumerate()
                    .map(|(i, t)| (field(local(a, ty, span), i, *t, span), field(local(b, ty, span), i, *t, span)))
                    .collect();
                self.chain(op, &mut body, &names, &types, pairs, "")?
            }
            (TyDef::Struct { fields, .. }, None) => {
                let mut stmts = Vec::new();
                for (i, f) in fields.iter().enumerate() {
                    let t = types::substitute(&f.ty, &targs, None);
                    if let Some(call) = self.walk(op, t, field(local(a, ty, span), i, t, span), span) {
                        stmts.push(discard(call, span));
                    }
                }
                Expr::new(ExprKind::Block { stmts, tail: Some(Box::new(unit(span))) }, Ty::UNIT, span)
            }
            (TyDef::Enum { variants }, b) => {
                let names: Vec<String> = variants.iter().map(|v| v.name.clone()).collect();
                let which = body.decision("variant", names.clone());
                let mut arms = Vec::new();
                for (vi, v) in variants.iter().enumerate() {
                    let types: Vec<Ty> = v.fields.iter().map(|f| types::substitute(&f.ty, &targs, None)).collect();
                    let xs: Vec<LocalId> = types.iter().map(|t| body.local(*t)).collect();
                    let inner = match b {
                        Some(b) => {
                            let other = body.decision(&format!("{}, the other", v.name), vec!["same variant".into(), "another variant".into()]);
                            let ys: Vec<LocalId> = types.iter().map(|t| body.local(*t)).collect();
                            let pairs: Vec<(Expr, Expr)> = xs
                                .iter()
                                .zip(&ys)
                                .zip(&types)
                                .map(|((x, y), t)| (local(*x, *t, span), local(*y, *t, span)))
                                .collect();
                            let fnames: Vec<String> = v.fields.iter().map(|f| f.name.clone()).collect();
                            let same = self.chain(op, &mut body, &fnames, &types, pairs, &format!("{}.", v.name))?;
                            let same = body.counted(other, 0, same);
                            let differs = body.counted(other, 1, Expr::new(ExprKind::Bool(false), self.bool_ty, span));
                            let arms = vec![
                                arm(variant(ty, vi, &ys, &types, span), same),
                                arm(Pattern { kind: PatKind::Wild, ty, span }, differs),
                            ];
                            Expr::new(ExprKind::Match { scrutinee: Box::new(local(b, ty, span)), arms }, self.bool_ty, span)
                        }
                        None => {
                            let mut stmts = Vec::new();
                            for (x, t) in xs.iter().zip(&types) {
                                if let Some(call) = self.walk(op, *t, local(*x, *t, span), span) {
                                    stmts.push(discard(call, span));
                                }
                            }
                            Expr::new(ExprKind::Block { stmts, tail: Some(Box::new(unit(span))) }, Ty::UNIT, span)
                        }
                    };
                    arms.push(arm(variant(ty, vi, &xs, &types, span), body.counted(which, vi, inner)));
                }
                let ret = if b.is_some() { self.bool_ty } else { Ty::UNIT };
                Expr::new(ExprKind::Match { scrutinee: Box::new(local(a, ty, span)), arms }, ret, span)
            }
            (TyDef::Prim(_), _) => return None,
        };
        let params = match b {
            Some(b) => vec![a, b],
            None => vec![a],
        };
        let f = self.funcs.get_mut(slot)?;
        f.params = params;
        f.locals = body.locals;
        f.kind = FuncKind::Body(expr);
        f.span = span;
        self.out.extend(body.decisions);
        Some(())
    }

    /// `x.0 == y.0 && x.1 == y.1 && ...` for `Equal`, and the same over
    /// "compares equal" for `Ordered`, counting where it stopped.
    fn chain(&mut self, op: Op, body: &mut Body, names: &[String], types: &[Ty], pairs: Vec<(Expr, Expr)>, at: &str) -> Option<Expr> {
        let span = body.span;
        let outcomes: [&str; 2] = if op == Op::Equal { ["same", "differs"] } else { ["equal", "decides"] };
        let mut tests = Vec::new();
        for ((t, (x, y)), name) in types.iter().zip(pairs).zip(names) {
            tests.push((self.same(op, *t, x, y, span)?, name.clone()));
        }
        let Some((last, _)) = tests.pop() else {
            return Some(Expr::new(ExprKind::Bool(true), self.bool_ty, span));
        };
        let mut acc = last;
        let decisions: Vec<usize> =
            tests.iter().map(|(_, name)| body.decision(&format!("{at}{name}"), outcomes.iter().map(|s| s.to_string()).collect())).collect();
        for ((test, _), d) in tests.into_iter().zip(decisions).rev() {
            let go_on = body.counted(d, 0, acc);
            let stop = body.counted(d, 1, Expr::new(ExprKind::Bool(false), self.bool_ty, span));
            acc = Expr::new(
                ExprKind::If { cond: Box::new(test), then: Box::new(go_on), else_: Box::new(stop) },
                self.bool_ty,
                span,
            );
        }
        Some(acc)
    }

    /// Whether `x` and `y` of type `t` are equal, or compare equal: through
    /// `t`'s shadow where the user derives the operation for it, and otherwise
    /// the way the derived code asks.
    fn same(&mut self, op: Op, t: Ty, x: Expr, y: Expr, span: Span) -> Option<Expr> {
        if let Some(call) = self.walk(op, t, x.clone(), span).map(|c| (c, y.clone())) {
            let (call, y) = call;
            let ExprKind::CallFn { func, mut args } = call.kind else { return None };
            args.push(y);
            return Some(Expr::new(ExprKind::CallFn { func, args }, self.bool_ty, span));
        }
        let desc = *self.desc_index.get(&t)?;
        match (op, self.descriptors.get(desc)?) {
            (Op::Equal, Desc::Prim(p)) => {
                Some(Expr::new(ExprKind::Prim { op: PrimOp::Eq, prim: *p, args: vec![x, y] }, self.bool_ty, span))
            }
            (_, Desc::Unit) => Some(Expr::new(ExprKind::Bool(true), self.bool_ty, span)),
            (Op::Equal, _) => Some(Expr::new(ExprKind::StructuralEq { negate: false, args: vec![x, y] }, self.bool_ty, span)),
            (Op::Ordered, _) => {
                let order = self.order?;
                let con = order.head()?;
                let index = Expr::new(ExprKind::Int(Magnitude::new(desc as u128), false), Ty::ERROR, span);
                let compared = Expr::new(
                    ExprKind::Intrinsic { name: "structuralCompare".into(), targs: Vec::new(), args: vec![x, y, index] },
                    order,
                    span,
                );
                // `Order`'s variants in declaration order: `Less`, `Equal`, `Greater`.
                let equal = Pattern { kind: PatKind::Variant { con, variant: 1, fields: Vec::new() }, ty: order, span };
                let arms = vec![
                    arm(equal, Expr::new(ExprKind::Bool(true), self.bool_ty, span)),
                    arm(Pattern { kind: PatKind::Wild, ty: order, span }, Expr::new(ExprKind::Bool(false), self.bool_ty, span)),
                ];
                Some(Expr::new(ExprKind::Match { scrutinee: Box::new(compared), arms }, self.bool_ty, span))
            }
            _ => None,
        }
    }

    /// A call of `t`'s shadow on `x`, where the user derives `op` for `t`. A
    /// comparison's caller adds the second value.
    fn walk(&mut self, op: Op, t: Ty, x: Expr, span: Span) -> Option<Expr> {
        let con = t.head()?;
        if !self.derives.contains_key(&(con, op)) {
            return None;
        }
        let (func, ret) = self.request(op, t)?;
        Some(Expr::new(ExprKind::CallFn { func: typed::Callee::Func(func), args: vec![x] }, ret, span))
    }
}

impl Body {
    fn local(&mut self, ty: Ty) -> LocalId {
        let id = LocalId(self.locals.len() as u32);
        self.locals.push(typed::Local { name: Name::new("coverage"), ty, span: self.span });
        id
    }

    /// A new decision, named after what it decides. Answers its index.
    fn decision(&mut self, what: &str, branches: Vec<String>) -> usize {
        let order = self.decisions.len();
        let label = format!("{}, {what}", self.prefix);
        self.decisions.push(Derived { span: self.span, group: self.prefix.clone(), order, label, branches });
        self.decisions.len().saturating_sub(1)
    }

    /// `e`, counted as branch `slot` of decision `d` first.
    fn counted(&self, d: usize, slot: usize, e: Expr) -> Expr {
        let Some(decision) = self.decisions.get(d) else { return e };
        let key = derived_key(&self.file, (self.span.start, self.span.end), &decision.label, slot);
        let span = e.span;
        let ty = e.ty;
        let arg = Expr::new(ExprKind::Int(Magnitude::new(u128::from(key)), false), self.i64_ty, span);
        let hit = Expr::new(
            ExprKind::Intrinsic { name: super::HIT.to_string(), targs: Vec::new(), args: vec![arg] },
            Ty::UNIT,
            span,
        );
        Expr::new(ExprKind::Block { stmts: vec![Stmt::Expr(hit)], tail: Some(Box::new(e)) }, ty, span)
    }
}

fn local(id: LocalId, ty: Ty, span: Span) -> Expr {
    Expr::new(ExprKind::Local(id), ty, span)
}

fn field(base: Expr, index: usize, ty: Ty, span: Span) -> Expr {
    Expr::new(ExprKind::Field { base: Box::new(base), index }, ty, span)
}

fn unit(span: Span) -> Expr {
    Expr::new(ExprKind::Unit, Ty::UNIT, span)
}

fn discard(value: Expr, span: Span) -> Stmt {
    Stmt::Let { pattern: Pattern { kind: PatKind::Wild, ty: value.ty, span }, value, span }
}

fn arm(pattern: Pattern, body: Expr) -> Arm {
    let span = body.span;
    Arm { pattern, guard: None, body, span }
}

fn variant(ty: Ty, variant: usize, binds: &[LocalId], types: &[Ty], span: Span) -> Pattern {
    let fields = binds
        .iter()
        .zip(types)
        .enumerate()
        .map(|(index, (l, t))| FieldPat { index, pattern: Pattern { kind: PatKind::Bind { local: *l, sub: None }, ty: *t, span } })
        .collect();
    let con = ty.head().unwrap_or(TyConId(0));
    Pattern { kind: PatKind::Variant { con, variant, fields }, ty, span }
}
