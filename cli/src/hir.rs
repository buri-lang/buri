//! The typed intermediate representation.
//!
//! Checking produces this; monomorphization rewrites it; the JS backend reads
//! it. Everything the surface syntax leaves to name resolution is settled
//! here: a `.` is already a field, a tuple index, or a call; an operator is
//! already a primitive operation or a trait method; a `match` still has
//! patterns but every path in one is resolved.

use crate::diag::Span;
use crate::types::{FnId, LocalId, Prim, TraitId, Ty, TyConId};

#[derive(Clone, Debug)]
pub struct Local {
    pub name: String,
    pub ty: Ty,
    pub span: Span,
}

/// The body of a function, once checked.
#[derive(Clone, Debug)]
pub struct Body {
    pub locals: Vec<Local>,
    /// Locals holding the parameters, in declaration order.
    pub params: Vec<LocalId>,
    pub expr: Expr,
}

#[derive(Clone, Debug)]
pub struct Expr {
    pub kind: ExprKind,
    pub ty: Ty,
    pub span: Span,
}

impl Expr {
    pub fn new(kind: ExprKind, ty: Ty, span: Span) -> Expr {
        Expr { kind, ty, span }
    }
}

#[derive(Clone, Debug)]
pub enum Stmt {
    /// `let` with an irrefutable pattern. The pattern is kept rather than
    /// flattened so the backend can emit destructuring directly.
    Let { pattern: Pattern, value: Expr, span: Span },
    /// An expression statement, legal only in a test source and only at type
    /// `()`.
    Expr(Expr),
}

#[derive(Clone, Debug)]
pub struct Arm {
    pub pattern: Pattern,
    pub guard: Option<Expr>,
    pub body: Expr,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct TemplatePart {
    pub text: Option<String>,
    pub hole: Option<Expr>,
}

/// Which of `Option` and `Result` a `?` or `??` is working on. They are
/// ordinary enums, but the backend can emit better code knowing which.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum OptionOrResult {
    Option,
    Result,
}

/// Operations the backend implements directly rather than through a trait.
/// Every one of these is defined on two operands of the *same* type: there is
/// no implicit promotion of any kind.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PrimOp {
    /// Overflow and underflow of an integer operation are undefined; the
    /// backend emits the operation and nothing else.
    Add,
    Sub,
    Mul,
    /// Integer `/` truncates toward zero. Division by zero aborts.
    Div,
    /// `%` takes the sign of the dividend.
    Rem,
    Neg,
    BitAnd,
    BitOr,
    BitXor,
    BitNot,
    Not,
    /// Structural equality, compiled per type.
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

#[derive(Clone, Debug)]
pub enum ExprKind {
    /// The literal's value, already checked to be representable in `ty`.
    Int(u128, bool),
    Float(f64),
    Str(String),
    Char(char),
    Bool(bool),
    Unit,

    Local(LocalId),
    /// A `const`, inlined by the backend.
    Const(crate::types::ConstId),
    /// A top-level function used as a value.
    FnRef(FnId, Vec<Ty>),

    /// A call through a value of function type.
    CallValue { callee: Box<Expr>, args: Vec<Expr> },
    /// A direct call to a known function. After monomorphization every generic
    /// call is one of these.
    CallFn { func: FnId, targs: Vec<Ty>, args: Vec<Expr> },
    /// A trait or effect method whose receiver type is not yet concrete.
    /// Monomorphization turns each of these into a `CallFn`.
    CallTrait {
        trait_id: TraitId,
        method: usize,
        /// The type the method is being called on.
        recv: Ty,
        targs: Vec<Ty>,
        args: Vec<Expr>,
    },

    StructLit { con: TyConId, targs: Vec<Ty>, fields: Vec<Expr> },
    /// `S { ..base, f: v }`. Never names the hidden fields, which is why it
    /// works outside the declaring module.
    StructUpdate { con: TyConId, base: Box<Expr>, updates: Vec<(usize, Expr)> },
    EnumLit { con: TyConId, targs: Vec<Ty>, variant: usize, args: Vec<Expr> },
    Tuple(Vec<Expr>),
    Array(Vec<Expr>),

    Field { base: Box<Expr>, index: usize },
    TupleIndex { base: Box<Expr>, index: usize },
    /// Indexing yields `Option<T>`. There is no way to index out of bounds.
    Index { base: Box<Expr>, index: Box<Expr>, elem: Ty },

    Block { stmts: Vec<Stmt>, tail: Option<Box<Expr>> },
    If { cond: Box<Expr>, then: Box<Expr>, else_: Box<Expr> },
    Match { scrutinee: Box<Expr>, arms: Vec<Arm> },

    /// Lambdas capture by value. Since values are immutable, capture is
    /// unobservable — and a lambda may not capture an effect-carrying value,
    /// which is what makes the purity theorem hold structurally.
    Lambda { params: Vec<LocalId>, body: Box<Expr>, captures: Vec<LocalId> },

    /// `&&` and `||`, which short-circuit.
    And { lhs: Box<Expr>, rhs: Box<Expr> },
    Or { lhs: Box<Expr>, rhs: Box<Expr> },
    /// `??`. The right operand is evaluated only when the left is `None`/`Err`.
    Coalesce { lhs: Box<Expr>, rhs: Box<Expr>, kind: OptionOrResult },
    /// Postfix `?`, the only early exit in the language.
    Try { base: Box<Expr>, kind: OptionOrResult },

    Prim { op: PrimOp, prim: Option<Prim>, args: Vec<Expr> },
    /// Structural equality or ordering on a compound type, compiled per type.
    StructuralEq { negate: bool, args: Vec<Expr> },
    StructuralCmp { op: PrimOp, args: Vec<Expr> },

    Template { parts: Vec<TemplatePart> },

    /// `context { ... }`, the only construct in which more than one
    /// effect-carrying value may appear.
    CtxLit { bindings: Vec<(TraitId, Expr)> },
    /// Reading one effect implementation out of a context value.
    CtxGet { base: Box<Expr>, trait_id: TraitId },
    /// A call to a named context declaration, which builds a fresh one.
    CtxCall { decl: crate::types::ContextDeclId },

    /// An operation the runtime supplies. Only the embedded standard library
    /// produces these.
    Intrinsic { name: String, targs: Vec<Ty>, args: Vec<Expr> },

    /// Poison. Produced only where a diagnostic was already reported.
    Error,
}

// ---------------------------------------------------------------------------
// Patterns
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct Pattern {
    pub kind: PatKind,
    pub ty: Ty,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct FieldPat {
    pub index: usize,
    pub pattern: Pattern,
}

#[derive(Clone, Debug)]
pub enum PatKind {
    Wild,
    Bind { local: LocalId, sub: Option<Box<Pattern>> },
    Int(u128, bool),
    Float(f64),
    Str(String),
    Char(char),
    Bool(bool),
    Unit,
    Tuple(Vec<Pattern>),
    Struct { con: TyConId, fields: Vec<FieldPat> },
    Variant { con: TyConId, variant: usize, fields: Vec<FieldPat> },
    /// `[a, b, ..rest]`. Rest patterns bind only at the end.
    Array { elems: Vec<Pattern>, rest: Option<Option<LocalId>> },
    Or(Vec<Pattern>),
    Error,
}

impl Pattern {
    /// Every local this pattern binds.
    pub fn binds(&self, out: &mut Vec<LocalId>) {
        match &self.kind {
            PatKind::Bind { local, sub } => {
                out.push(*local);
                if let Some(s) = sub {
                    s.binds(out);
                }
            }
            PatKind::Tuple(ps) => ps.iter().for_each(|p| p.binds(out)),
            PatKind::Struct { fields, .. } | PatKind::Variant { fields, .. } => {
                fields.iter().for_each(|f| f.pattern.binds(out))
            }
            PatKind::Array { elems, rest } => {
                elems.iter().for_each(|p| p.binds(out));
                if let Some(Some(l)) = rest {
                    out.push(*l);
                }
            }
            // Alternatives bind identical names at identical types, so the
            // first is representative.
            PatKind::Or(alts) => {
                if let Some(f) = alts.first() {
                    f.binds(out)
                }
            }
            _ => {}
        }
    }

    /// Whether this pattern matches every value of its type.
    pub fn is_irrefutable(&self, tables: &crate::types::Tables) -> bool {
        match &self.kind {
            PatKind::Wild | PatKind::Unit | PatKind::Error => true,
            PatKind::Bind { sub, .. } => {
                sub.as_ref().map(|s| s.is_irrefutable(tables)).unwrap_or(true)
            }
            PatKind::Tuple(ps) => ps.iter().all(|p| p.is_irrefutable(tables)),
            PatKind::Struct { fields, .. } => {
                fields.iter().all(|f| f.pattern.is_irrefutable(tables))
            }
            // A single-variant enum has nothing to fall through to.
            PatKind::Variant { con, fields, .. } => {
                tables.tycon(*con).variants().len() == 1
                    && fields.iter().all(|f| f.pattern.is_irrefutable(tables))
            }
            PatKind::Array { elems, rest } => {
                elems.is_empty() && rest.is_some()
            }
            PatKind::Or(alts) => alts.iter().any(|p| p.is_irrefutable(tables)),
            _ => false,
        }
    }
}

// ---------------------------------------------------------------------------
// Visiting
// ---------------------------------------------------------------------------

/// Walks every sub-expression, outermost first.
pub fn walk(e: &Expr, f: &mut impl FnMut(&Expr)) {
    f(e);
    let mut go = |e: &Expr| walk(e, f);
    match &e.kind {
        ExprKind::CallValue { callee, args } => {
            go(callee);
            args.iter().for_each(go);
        }
        ExprKind::CallFn { args, .. }
        | ExprKind::CallTrait { args, .. }
        | ExprKind::StructLit { fields: args, .. }
        | ExprKind::EnumLit { args, .. }
        | ExprKind::Tuple(args)
        | ExprKind::Array(args)
        | ExprKind::Prim { args, .. }
        | ExprKind::StructuralEq { args, .. }
        | ExprKind::StructuralCmp { args, .. }
        | ExprKind::Intrinsic { args, .. } => args.iter().for_each(go),
        ExprKind::StructUpdate { base, updates, .. } => {
            go(base);
            updates.iter().for_each(|(_, e)| go(e));
        }
        ExprKind::Field { base, .. }
        | ExprKind::TupleIndex { base, .. }
        | ExprKind::CtxGet { base, .. }
        | ExprKind::Try { base, .. } => go(base),
        ExprKind::Index { base, index, .. } => {
            go(base);
            go(index);
        }
        ExprKind::Block { stmts, tail } => {
            for s in stmts {
                match s {
                    Stmt::Let { value, .. } => go(value),
                    Stmt::Expr(e) => go(e),
                }
            }
            if let Some(t) = tail {
                go(t);
            }
        }
        ExprKind::If { cond, then, else_ } => {
            go(cond);
            go(then);
            go(else_);
        }
        ExprKind::Match { scrutinee, arms } => {
            go(scrutinee);
            for a in arms {
                if let Some(g) = &a.guard {
                    go(g);
                }
                go(&a.body);
            }
        }
        ExprKind::Lambda { body, .. } => go(body),
        ExprKind::And { lhs, rhs } | ExprKind::Or { lhs, rhs } | ExprKind::Coalesce { lhs, rhs, .. } => {
            go(lhs);
            go(rhs);
        }
        ExprKind::Template { parts } => {
            for p in parts {
                if let Some(h) = &p.hole {
                    go(h);
                }
            }
        }
        ExprKind::CtxLit { bindings } => bindings.iter().for_each(|(_, e)| go(e)),
        _ => {}
    }
}
