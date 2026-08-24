//! An abstract interpreter over the typed tree.
//!
//! It answers one question — *what value does this expression denote, if the
//! answer does not depend on anything that happens at run time* — and it is
//! allowed to give up. `None` is not an error anywhere in this file: the one
//! caller, [`crate::compiler::semantics::styles`], treats "cannot be folded" as
//! "belongs to the runtime tier", which is the degradation
//! `design/ui-reactivity.md` §Styling makes the documented default.
//!
//! ## Why an interpreter rather than a table of literals
//!
//! The overwhelmingly common style literal is written inline —
//! `ui.row([.PaddingX(.Rem(0.5)), .Radius(.Px(6))], [...])` — so the extractor
//! is a walker over constructor applications and array literals whatever else
//! it does. Once that exists, folding a *pure call* is an increment on it:
//! inline the callee's body, bind its parameters, and keep evaluating. That is
//! what makes a design token's `Token.Surface.color()` extractable, and it is
//! also what makes an ordinary helper — `fn cardPadding(): Style` — extractable,
//! which a generated token module would not have been.
//!
//! ## What makes inlining a call sound
//!
//! Purity is read off the signature, exactly as SPEC 10.4 defines it: a
//! function is pure when it takes no `ctx`, carries no effect on `self`, and
//! asks for no allocator. Those are three questions about a [`FnInfo`], and all
//! three are settled before any body is checked. A pure function's result
//! depends on nothing but its arguments, so evaluating it early cannot change
//! what the program does.
//!
//! ## What it deliberately does not do
//!
//! No loops, because there are none in the language; no recursion into the
//! function currently being folded, so a recursive helper gives up rather than
//! running forever; no intrinsics, no trait-method dispatch, no effect of any
//! kind. Two counters bound it anyway — a step budget and a call depth — so a
//! deeply nested constant costs a bounded amount of compile time rather than an
//! unbounded one.

use crate::compiler::semantics::typed::{self, ExprKind, PatKind, PrimOp, Stmt};
use crate::compiler::semantics::types::{
    ConstId, FnId, LocalId, ParamRole, Prim, Tables,
};
use crate::hash::Map as HashMap;

/// A value the interpreter reached.
///
/// Positional throughout, and carrying no type: the caller knows what it asked
/// to evaluate, so a `Length` is three variants and a payload rather than a
/// name it would have to look up again. Nothing outside this module and
/// `styles` reads one.
#[derive(Clone, Debug)]
pub enum Value {
    Int(i128),
    Float(f64),
    Str(String),
    Char(char),
    Bool(bool),
    Unit,
    Tuple(Vec<Value>),
    Array(Vec<Value>),
    /// A struct, by field order.
    Struct(Vec<Value>),
    /// An enum value: which variant, and its payload.
    Variant { variant: usize, args: Vec<Value> },
}

impl Value {
    pub fn as_int(&self) -> Option<i128> {
        match self {
            Value::Int(v) => Some(*v),
            _ => None,
        }
    }

    /// A float, accepting an integer where a float is wanted — which is what a
    /// numeric literal that defaulted the other way looks like here.
    pub fn as_float(&self) -> Option<f64> {
        match self {
            Value::Float(v) => Some(*v),
            Value::Int(v) => Some(*v as f64),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Bool(b) => Some(*b),
            _ => None,
        }
    }

    pub fn as_variant(&self) -> Option<(usize, &[Value])> {
        match self {
            Value::Variant { variant, args } => Some((*variant, args)),
            _ => None,
        }
    }
}

/// How many expression nodes one fold may visit before it gives up.
///
/// A budget rather than a proof of termination: the language has no loops, so
/// the only way to spend an unbounded number of steps is recursion through
/// calls, which [`Folder::depth`] already bounds. This is the belt to that
/// pair of braces — a program with a thousand nested constants folds, and one
/// that would cost a second of compile time does not.
const STEP_BUDGET: u32 = 100_000;

/// How deep a chain of folded calls may go.
const CALL_DEPTH: u32 = 32;

pub struct Folder<'a> {
    tables: &'a Tables,
    /// Bodies as they were *before* extraction rewrote any of them. Folding
    /// against a rewritten body would fold a call into an already-extracted
    /// style, which the flattener cannot read back.
    bodies: &'a HashMap<FnId, typed::Body>,
    consts: &'a HashMap<ConstId, typed::Expr>,
    steps: u32,
    depth: u32,
    /// Constants currently being evaluated, so a cyclic initializer gives up
    /// instead of recursing. Cycles are diagnosed elsewhere; this only has to
    /// not hang.
    open_consts: Vec<ConstId>,
}

/// One frame of locals. A fold never sees an outer frame: a lambda is not
/// folded, and a call's body reads only its own parameters.
#[derive(Default)]
pub struct Env {
    locals: HashMap<LocalId, Value>,
}

impl<'a> Folder<'a> {
    pub fn new(
        tables: &'a Tables,
        bodies: &'a HashMap<FnId, typed::Body>,
        consts: &'a HashMap<ConstId, typed::Expr>,
    ) -> Folder<'a> {
        Folder { tables, bodies, consts, steps: 0, depth: 0, open_consts: Vec::new() }
    }

    /// Whether a function may be inlined by the folder: pure by SPEC 10.4, and
    /// with a body to inline.
    ///
    /// The `Alloc` question is asked of the bounds rather than of the body,
    /// because that is where the answer is: a function that allocates says so
    /// in its signature, and one that does not cannot start.
    fn is_foldable_fn(&self, id: FnId) -> bool {
        let info = self.tables.fun(id);
        if info.intrinsic {
            return false;
        }
        if info.params.iter().any(|p| p.role == ParamRole::Ctx) {
            return false;
        }
        // An effect-carrying `self` is the other way a function is impure, and
        // it is the one a method could smuggle in.
        if info
            .params
            .iter()
            .any(|p| p.role == ParamRole::SelfParam && self.tables.is_effect_carrying(&p.ty, &[]))
        {
            return false;
        }
        if info
            .generics
            .iter()
            .any(|g| g.bounds.iter().any(|t| self.tables.trait_(*t).is_effect))
        {
            return false;
        }
        self.bodies.contains_key(&id)
    }

    /// The value an expression denotes, or `None` when it denotes something
    /// only the run time knows.
    pub fn eval(&mut self, e: &typed::Expr, env: &Env) -> Option<Value> {
        self.steps = self.steps.saturating_add(1);
        if self.steps > STEP_BUDGET {
            return None;
        }
        match &e.kind {
            ExprKind::Int(v, neg) => {
                let magnitude = i128::try_from(*v).ok()?;
                Some(Value::Int(if *neg { magnitude.checked_neg()? } else { magnitude }))
            }
            ExprKind::Float(v) => {
                // An `F32` literal denotes the binary32 value, the same
                // narrowing the backend applies.
                match self.tables.as_prim(&e.ty) {
                    Some(Prim::F32) => Some(Value::Float(*v as f32 as f64)),
                    _ => Some(Value::Float(*v)),
                }
            }
            ExprKind::Str(s) => Some(Value::Str(s.clone())),
            ExprKind::Char(c) => Some(Value::Char(*c)),
            ExprKind::Bool(b) => Some(Value::Bool(*b)),
            ExprKind::Unit => Some(Value::Unit),

            ExprKind::Local(l) => env.locals.get(l).cloned(),
            ExprKind::Const(id) => self.eval_const(*id),

            ExprKind::Tuple(xs) => Some(Value::Tuple(self.eval_all(xs, env)?)),
            ExprKind::Array(xs) => Some(Value::Array(self.eval_all(xs, env)?)),
            ExprKind::StructLit { fields, .. } => {
                Some(Value::Struct(self.eval_all(fields, env)?))
            }
            ExprKind::EnumLit { variant, args, .. } => {
                Some(Value::Variant { variant: *variant, args: self.eval_all(args, env)? })
            }
            ExprKind::StructUpdate { base, updates, .. } => {
                let Value::Struct(mut fields) = self.eval(base, env)? else { return None };
                for (index, value) in updates {
                    *fields.get_mut(*index)? = self.eval(value, env)?;
                }
                Some(Value::Struct(fields))
            }

            ExprKind::Field { base, index } => match self.eval(base, env)? {
                Value::Struct(fields) => fields.get(*index).cloned(),
                _ => None,
            },
            ExprKind::TupleIndex { base, index } => match self.eval(base, env)? {
                Value::Tuple(items) => items.get(*index).cloned(),
                _ => None,
            },
            ExprKind::Index { base, index, .. } => {
                let Value::Array(items) = self.eval(base, env)? else { return None };
                let at = self.eval(index, env)?.as_int()?;
                // Indexing answers an `Option`, and both variants are ordinary
                // values here: out of bounds is `None`, not a failure to fold.
                let found = usize::try_from(at).ok().and_then(|i| items.get(i).cloned());
                Some(match found {
                    Some(v) => Value::Variant { variant: OPTION_SOME, args: vec![v] },
                    None => Value::Variant { variant: OPTION_NONE, args: Vec::new() },
                })
            }

            ExprKind::Block { stmts, tail } => {
                let mut inner = Env { locals: env.locals.clone() };
                for stmt in stmts {
                    match stmt {
                        Stmt::Let { pattern, value, .. } => {
                            let v = self.eval(value, &inner)?;
                            bind(pattern, &v, &mut inner)?;
                        }
                        // An expression statement is legal only at `()`, so it
                        // has no value to carry and nothing it could do that
                        // this interpreter models.
                        Stmt::Expr(_) => return None,
                    }
                }
                match tail {
                    Some(t) => self.eval(t, &inner),
                    None => Some(Value::Unit),
                }
            }
            ExprKind::If { cond, then, else_ } => {
                if self.eval(cond, env)?.as_bool()? {
                    self.eval(then, env)
                } else {
                    self.eval(else_, env)
                }
            }
            ExprKind::Match { scrutinee, arms } => {
                let value = self.eval(scrutinee, env)?;
                for arm in arms {
                    let mut inner = Env { locals: env.locals.clone() };
                    if !matches_pattern(&arm.pattern, &value, &mut inner) {
                        continue;
                    }
                    if let Some(guard) = &arm.guard {
                        if !self.eval(guard, &inner)?.as_bool()? {
                            continue;
                        }
                    }
                    return self.eval(&arm.body, &inner);
                }
                None
            }
            ExprKind::And { lhs, rhs } => {
                if self.eval(lhs, env)?.as_bool()? {
                    self.eval(rhs, env)
                } else {
                    Some(Value::Bool(false))
                }
            }
            ExprKind::Or { lhs, rhs } => {
                if self.eval(lhs, env)?.as_bool()? {
                    Some(Value::Bool(true))
                } else {
                    self.eval(rhs, env)
                }
            }

            ExprKind::Prim { op, prim, args } => {
                let args = self.eval_all(args, env)?;
                prim_op(*op, *prim, &args)
            }
            ExprKind::Template { parts } => {
                let mut out = String::new();
                for part in parts {
                    match part {
                        typed::TemplatePart::Text(t) => out.push_str(t),
                        // Only a string hole renders without calling `show`,
                        // and `show` is a trait method this does not dispatch.
                        typed::TemplatePart::Hole(h) => {
                            out.push_str(self.eval(h, env)?.as_str()?)
                        }
                    }
                }
                Some(Value::Str(out))
            }

            ExprKind::CallFn { func, args } => {
                let id = func.decl()?;
                if !self.is_foldable_fn(id) {
                    return None;
                }
                let args = self.eval_all(args, env)?;
                self.call(id, args)
            }

            // Everything below denotes something the run time decides, or
            // something this interpreter has no model of. Each is a `None`
            // rather than an omission, so adding a case is a deliberate act.
            ExprKind::FnRef(_)
            | ExprKind::CallValue { .. }
            | ExprKind::CallTrait { .. }
            | ExprKind::Lambda { .. }
            | ExprKind::Closure { .. }
            | ExprKind::Coalesce { .. }
            | ExprKind::Try { .. }
            | ExprKind::Loop { .. }
            | ExprKind::Continue { .. }
            | ExprKind::StructuralEq { .. }
            | ExprKind::StructuralCmp { .. }
            | ExprKind::CtxLit { .. }
            | ExprKind::CtxGet { .. }
            | ExprKind::CtxCall { .. }
            | ExprKind::Intrinsic { .. }
            | ExprKind::Error => None,
        }
    }

    fn eval_all(&mut self, xs: &[typed::Expr], env: &Env) -> Option<Vec<Value>> {
        let mut out = Vec::with_capacity(xs.len());
        for x in xs {
            out.push(self.eval(x, env)?);
        }
        Some(out)
    }

    fn eval_const(&mut self, id: ConstId) -> Option<Value> {
        if self.open_consts.contains(&id) {
            return None;
        }
        let init = self.consts.get(&id)?;
        // Cloned so that the borrow of `self.consts` ends before the recursive
        // call, which needs `&mut self`. Constant initializers are small and
        // are folded once per use site at most.
        let init = init.clone();
        self.open_consts.push(id);
        let value = self.eval(&init, &Env::default());
        self.open_consts.pop();
        value
    }

    /// Inlines one pure call: bind the parameters, evaluate the body.
    fn call(&mut self, id: FnId, args: Vec<Value>) -> Option<Value> {
        if self.depth >= CALL_DEPTH {
            return None;
        }
        let body = self.bodies.get(&id)?.clone();
        if body.params.len() != args.len() {
            return None;
        }
        let mut env = Env::default();
        for (local, value) in body.params.iter().zip(args) {
            env.locals.insert(*local, value);
        }
        self.depth = self.depth.saturating_add(1);
        let out = self.eval(&body.expr, &env);
        self.depth = self.depth.saturating_sub(1);
        out
    }
}

/// `Option`'s variant indices, which `core/option` fixes and the backend
/// already depends on.
const OPTION_SOME: usize = 0;
const OPTION_NONE: usize = 1;

/// Binds an irrefutable pattern. `None` when the pattern is one this does not
/// model, which makes the whole fold give up rather than bind wrongly.
fn bind(p: &typed::Pattern, v: &Value, env: &mut Env) -> Option<()> {
    matches_pattern(p, v, env).then_some(())
}

/// Whether a pattern matches a value, binding as it goes.
///
/// A pattern this does not model answers `false`, which is safe in the one
/// place that matters: [`Folder::eval`]'s `Match` runs out of arms and gives
/// up, rather than taking a later arm the program would not have taken.
fn matches_pattern(p: &typed::Pattern, v: &Value, env: &mut Env) -> bool {
    match (&p.kind, v) {
        (PatKind::Wild, _) => true,
        (PatKind::Bind { local, sub }, _) => {
            env.locals.insert(*local, v.clone());
            match sub {
                Some(inner) => matches_pattern(inner, v, env),
                None => true,
            }
        }
        (PatKind::Int(magnitude, neg), Value::Int(n)) => i128::try_from(*magnitude)
            .ok()
            .and_then(|m| if *neg { m.checked_neg() } else { Some(m) })
            .is_some_and(|m| m == *n),
        (PatKind::Float(f), Value::Float(g)) => f == g,
        (PatKind::Str(s), Value::Str(t)) => s == t,
        (PatKind::Char(c), Value::Char(d)) => c == d,
        (PatKind::Bool(b), Value::Bool(c)) => b == c,
        (PatKind::Unit, Value::Unit) => true,
        (PatKind::Tuple(ps), Value::Tuple(vs)) => {
            ps.len() == vs.len()
                && ps.iter().zip(vs).all(|(q, w)| matches_pattern(q, w, env))
        }
        (PatKind::Struct { fields, .. }, Value::Struct(vs)) => fields
            .iter()
            .all(|f| vs.get(f.index).is_some_and(|w| matches_pattern(&f.pattern, w, env))),
        (PatKind::Variant { variant, fields, .. }, Value::Variant { variant: got, args }) => {
            *variant == *got
                && fields.iter().all(|f| {
                    args.get(f.index).is_some_and(|w| matches_pattern(&f.pattern, w, env))
                })
        }
        (PatKind::Array { elems, rest }, Value::Array(vs)) => {
            let long_enough =
                if rest.is_open() { vs.len() >= elems.len() } else { vs.len() == elems.len() };
            if !long_enough {
                return false;
            }
            if !elems.iter().zip(vs).all(|(q, w)| matches_pattern(q, w, env)) {
                return false;
            }
            match rest {
                typed::ArrayRest::Bound(local) => {
                    let tail = vs.get(elems.len()..).unwrap_or(&[]).to_vec();
                    env.locals.insert(*local, Value::Array(tail));
                    true
                }
                _ => true,
            }
        }
        (PatKind::Or(alts), _) => alts.iter().any(|q| matches_pattern(q, v, env)),
        _ => false,
    }
}

/// One primitive operation on values that are already known.
///
/// Integer overflow is undefined in Buri, so a fold that would overflow gives
/// up rather than picking one of the two answers a backend might produce. That
/// is the only way an interpreter and a target can be made to agree about an
/// operation neither is required to define.
fn prim_op(op: PrimOp, prim: Prim, args: &[Value]) -> Option<Value> {
    let float = matches!(prim, Prim::F32 | Prim::F64);
    match (op, args) {
        (PrimOp::Not, [Value::Bool(b)]) => Some(Value::Bool(!b)),
        (PrimOp::Neg, [a]) if float => narrow(prim, -a.as_float()?),
        (PrimOp::Neg, [Value::Int(a)]) => a.checked_neg().map(Value::Int),
        (PrimOp::BitNot, [Value::Int(a)]) => Some(Value::Int(!a)),

        (_, [a, b]) if float => {
            let (x, y) = (a.as_float()?, b.as_float()?);
            match op {
                PrimOp::Add => narrow(prim, x + y),
                PrimOp::Sub => narrow(prim, x - y),
                PrimOp::Mul => narrow(prim, x * y),
                PrimOp::Div => narrow(prim, x / y),
                PrimOp::Rem => narrow(prim, x % y),
                _ => compare(op, &x, &y),
            }
        }
        (_, [Value::Str(x), Value::Str(y)]) => match op {
            PrimOp::Add => Some(Value::Str(format!("{x}{y}"))),
            _ => compare(op, x, y),
        },
        (_, [Value::Bool(x), Value::Bool(y)]) => match op {
            PrimOp::Eq => Some(Value::Bool(x == y)),
            PrimOp::Ne => Some(Value::Bool(x != y)),
            _ => None,
        },
        (_, [Value::Int(x), Value::Int(y)]) => match op {
            PrimOp::Add => x.checked_add(*y).map(Value::Int),
            PrimOp::Sub => x.checked_sub(*y).map(Value::Int),
            PrimOp::Mul => x.checked_mul(*y).map(Value::Int),
            PrimOp::Div => x.checked_div(*y).map(Value::Int),
            PrimOp::Rem => x.checked_rem(*y).map(Value::Int),
            PrimOp::BitAnd => Some(Value::Int(x & y)),
            PrimOp::BitOr => Some(Value::Int(x | y)),
            PrimOp::BitXor => Some(Value::Int(x ^ y)),
            _ => compare(op, x, y),
        },
        _ => None,
    }
}

/// The six comparisons, at whichever type the operands are.
///
/// `PartialOrd` rather than `Ord` because `F64` is one of the three callers
/// and NaN is the reason the distinction exists; `<` on `f64` is what `Lt`
/// means, and this is that operator and not a re-derivation of it. Anything
/// that is not a comparison answers `None`, which is the arm each caller had
/// written out below its own arithmetic.
fn compare<T: PartialOrd + ?Sized>(op: PrimOp, x: &T, y: &T) -> Option<Value> {
    Some(Value::Bool(match op {
        PrimOp::Eq => x == y,
        PrimOp::Ne => x != y,
        PrimOp::Lt => x < y,
        PrimOp::Le => x <= y,
        PrimOp::Gt => x > y,
        PrimOp::Ge => x >= y,
        _ => return None,
    }))
}

/// A float result, at the width the operation was performed at.
fn narrow(prim: Prim, v: f64) -> Option<Value> {
    Some(Value::Float(if matches!(prim, Prim::F32) { v as f32 as f64 } else { v }))
}
