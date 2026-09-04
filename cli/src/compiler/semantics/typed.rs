//! The typed intermediate representation.
//!
//! Checking produces this; monomorphization rewrites it; the JS backend reads
//! it. Everything the surface syntax leaves to name resolution is settled
//! here: a `.` is already a field, a tuple index, or a call; an operator is
//! already a primitive operation or a trait method; a `match` still has
//! patterns but every path in one is resolved.

use crate::compiler::semantics::types::{FnId, FuncIdx, LocalId, Prim, TraitId, Ty, TyConId};
use crate::diagnostics::Span;

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

/// One piece of an interpolation: literal text, or a hole to render.
///
/// Mirrors `tree::TemplatePart`. A part is one or the other and never both or
/// neither, so it is an enum rather than two `Option`s — a part that carried
/// both would be rendered twice by the backend.
#[derive(Clone, Debug)]
pub enum TemplatePart {
    Text(String),
    Hole(Expr),
}

/// Which of `Option` and `Result` a `?` is working on. They are
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

/// Who a direct call, or a function-valued reference, names.
///
/// Before monomorphization a call names a *declaration* together with the type
/// arguments it is instantiated at. After it, every call names one concrete
/// function — an index into `Program::funcs`. Both used to be spelled `FnId`
/// with a `Vec<Ty>` beside it that had to be empty afterwards, so the two index
/// spaces were interchangeable to the compiler and the rule lived in a doc
/// comment in another module.
#[derive(Clone, Debug)]
pub enum Callee {
    /// A declaration and the type arguments it is called at.
    Decl { id: FnId, targs: Vec<Ty> },
    /// One concrete function, after monomorphization. Carries no type
    /// arguments, because there is nothing left to instantiate.
    Func(FuncIdx),
}

impl Callee {
    /// The declaration this names, before monomorphization has run.
    pub fn decl(&self) -> Option<FnId> {
        match self {
            Callee::Decl { id, .. } => Some(*id),
            Callee::Func(_) => None,
        }
    }

    /// The concrete function this names, after monomorphization has run.
    pub fn func(&self) -> Option<FuncIdx> {
        match self {
            Callee::Decl { .. } => None,
            Callee::Func(i) => Some(*i),
        }
    }
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
    /// A module-level `let`, inlined by the backend.
    Const(crate::compiler::semantics::types::ConstId),
    /// A top-level function used as a value.
    FnRef(Callee),

    /// A call through a value of function type.
    CallValue { callee: Box<Expr>, args: Vec<Expr> },
    /// A direct call to a known function. After monomorphization every generic
    /// call is one of these.
    CallFn { func: Callee, args: Vec<Expr> },
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
    /// Postfix `?`, the only early exit in the language.
    Try { base: Box<Expr>, kind: OptionOrResult },

    /// A loop, and the only expression here that cannot be written down in the
    /// source language: `middle::tail_calls` produces it and nothing else
    /// does.
    ///
    /// It is always a whole function body, and the values it rebinds are that
    /// function's parameters — [`ExprKind::Continue`] assigns them and
    /// re-enters. There is one entry per member of the tail-recursive group
    /// the loop was made from, which for a function that only tail-calls
    /// itself is one; a merged group is entered from outside, so which entry
    /// runs first is the caller's to choose.
    Loop { entries: Vec<Expr> },

    /// A tail call, after elimination: rebind and jump.
    ///
    /// `func` is `None` for a jump back to the top of the enclosing
    /// [`ExprKind::Loop`], and `Some(f)` for the call that enters the function
    /// a merged group became. The entry that call selects is a dispatch
    /// parameter the backend materialises: it is a control index rather than a
    /// value of any Buri type, so it is spelled here as a number rather than
    /// smuggled in as an extra argument at a type the middle end would have
    /// had to invent.
    Continue { func: Option<FuncIdx>, entry: usize, args: Vec<Expr> },

    /// A closure: a lifted function, and the environment it reads its captures
    /// out of. Produced by `middle::closures` in place of a [`ExprKind::Lambda`],
    /// on the native branch only.
    ///
    /// The type is still the `Ty::Fn` the lambda had — VALUE-MODEL.md §7 makes
    /// `{ code, env }` the representation of *every* function value, so a
    /// closure is not a different type from a function, only a different way of
    /// filling one in. A lambda that captures nothing does not become one of
    /// these at all: it becomes an [`ExprKind::FnRef`], which is already the
    /// null-environment case.
    Closure { func: FuncIdx, env: Vec<Expr> },

    /// A primitive operation at a known primitive type. The type is not
    /// optional: every operation here is chosen *because* the operand type
    /// resolved to a primitive, and a missing one used to default to `I64` in
    /// the backend — which is integer division semantics for a float divide.
    Prim { op: PrimOp, prim: Prim, args: Vec<Expr> },
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
    CtxCall { decl: crate::compiler::semantics::types::ContextDeclId },

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

/// The tail of an array pattern. Three states, spelled once: this was
/// `Option<Option<LocalId>>`, and every reader had to remember which nesting
/// level meant "no `..`" and which meant "`..` without a name".
#[derive(Clone, Copy, Debug)]
pub enum ArrayRest {
    /// No `..`, so the pattern matches an array of exactly `elems.len()`.
    None,
    /// `..` with no name: the tail is matched but discarded.
    Ignored,
    /// `..name`, binding the tail.
    Bound(LocalId),
}

impl ArrayRest {
    /// Whether the pattern matches arrays longer than its listed elements.
    pub fn is_open(self) -> bool {
        !matches!(self, ArrayRest::None)
    }
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
    Array { elems: Vec<Pattern>, rest: ArrayRest },
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
                if let ArrayRest::Bound(l) = rest {
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

    /// The locals this pattern binds to a **freshly allocated** value rather
    /// than to a projection into the scrutinee.
    ///
    /// Only `..rest` is one. Every other binding names a piece of the value
    /// being matched and borrows the scrutinee's allocation; a rest binding
    /// names a tail that both native backends build with a fresh block and a
    /// copy (VALUE-MODEL.md §4.4), so its owner is the arm, not the scrutinee.
    pub fn fresh_binds(&self, out: &mut Vec<LocalId>) {
        match &self.kind {
            PatKind::Bind { sub: Some(s), .. } => s.fresh_binds(out),
            PatKind::Tuple(ps) => ps.iter().for_each(|p| p.fresh_binds(out)),
            PatKind::Struct { fields, .. } | PatKind::Variant { fields, .. } => {
                fields.iter().for_each(|f| f.pattern.fresh_binds(out))
            }
            PatKind::Array { elems, rest } => {
                elems.iter().for_each(|p| p.fresh_binds(out));
                if let ArrayRest::Bound(l) = rest {
                    out.push(*l);
                }
            }
            PatKind::Or(alts) => {
                if let Some(f) = alts.first() {
                    f.fresh_binds(out)
                }
            }
            _ => {}
        }
    }

    /// Whether this pattern matches every value of its type.
    pub fn is_irrefutable(&self, tables: &crate::compiler::semantics::types::Tables) -> bool {
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
                elems.is_empty() && rest.is_open()
            }
            PatKind::Or(alts) => alts.iter().any(|p| p.is_irrefutable(tables)),
            _ => false,
        }
    }
}

// ---------------------------------------------------------------------------
// Visiting
// ---------------------------------------------------------------------------

/// Every **direct** sub-expression, in evaluation order for every construct
/// that has one.
///
/// The one description of what an [`ExprKind`]'s children are. Every walk over
/// a body in the front end and the middle end is this function plus what it
/// does at each node — [`walk`] here, `middle::rc`'s node numbering, and the
/// four middle-end passes that rewrite through [`children_mut`]. It is written
/// out with no `_` arm, so a new `ExprKind` is a compile error here rather
/// than a silently unvisited subtree; the two bugs the comments below record
/// are what a `_` arm and a second copy cost.
///
/// A callback rather than a `Vec<&Expr>`: a vector here is a heap allocation
/// at every node of every body, paid again by every pass that walks the tree.
pub fn children<'a>(e: &'a Expr, f: &mut impl FnMut(&'a Expr)) {
    match &e.kind {
        ExprKind::CallValue { callee, args } => {
            f(callee);
            args.iter().for_each(f);
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
        | ExprKind::Intrinsic { args, .. }
        // `Continue`'s arguments and `Loop`'s entries are children like any
        // other. A walk that left them out reported a whole loop as one node,
        // and left a `structuralEq` inside one for `lower` to meet as an
        // `Inst::Structural`; both were real bugs, in two separate copies of
        // this match.
        | ExprKind::Continue { args, .. }
        | ExprKind::Closure { env: args, .. }
        | ExprKind::Loop { entries: args } => args.iter().for_each(f),
        ExprKind::StructUpdate { base, updates, .. } => {
            f(base);
            updates.iter().for_each(|(_, e)| f(e));
        }
        ExprKind::Field { base, .. }
        | ExprKind::TupleIndex { base, .. }
        | ExprKind::CtxGet { base, .. }
        | ExprKind::Try { base, .. } => f(base),
        ExprKind::Index { base, index, .. } => {
            f(base);
            f(index);
        }
        ExprKind::Block { stmts, tail } => {
            for s in stmts {
                match s {
                    Stmt::Let { value, .. } => f(value),
                    Stmt::Expr(e) => f(e),
                }
            }
            if let Some(t) = tail {
                f(t);
            }
        }
        ExprKind::If { cond, then, else_ } => {
            f(cond);
            f(then);
            f(else_);
        }
        ExprKind::Match { scrutinee, arms } => {
            f(scrutinee);
            for a in arms {
                if let Some(g) = &a.guard {
                    f(g);
                }
                f(&a.body);
            }
        }
        ExprKind::Lambda { body, .. } => f(body),
        ExprKind::And { lhs, rhs } | ExprKind::Or { lhs, rhs } => {
            f(lhs);
            f(rhs);
        }
        ExprKind::Template { parts } => {
            for p in parts {
                if let TemplatePart::Hole(h) = p {
                    f(h);
                }
            }
        }
        ExprKind::CtxLit { bindings } => bindings.iter().for_each(|(_, e)| f(e)),
        ExprKind::Int(..)
        | ExprKind::Float(_)
        | ExprKind::Str(_)
        | ExprKind::Char(_)
        | ExprKind::Bool(_)
        | ExprKind::Unit
        | ExprKind::Local(_)
        | ExprKind::Const(_)
        | ExprKind::FnRef(_)
        | ExprKind::CtxCall { .. }
        | ExprKind::Error => {}
    }
}

/// [`children`], by mutable reference, for a pass that rewrites what it
/// visits.
///
/// The same arms, and it has to be a second function rather than a generic
/// one: Rust has no way to be generic over the mutability of a reference, and
/// the alternative — a pass reading the tree and then writing it — is two
/// walks where one will do.
pub fn children_mut(e: &mut Expr, f: &mut impl FnMut(&mut Expr)) {
    match &mut e.kind {
        ExprKind::CallValue { callee, args } => {
            f(callee);
            args.iter_mut().for_each(f);
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
        | ExprKind::Intrinsic { args, .. }
        // `Continue`'s arguments and `Loop`'s entries are children like any
        // other. A walk that left them out reported a whole loop as one node,
        // and left a `structuralEq` inside one for `lower` to meet as an
        // `Inst::Structural`; both were real bugs, in two separate copies of
        // this match.
        | ExprKind::Continue { args, .. }
        | ExprKind::Closure { env: args, .. }
        | ExprKind::Loop { entries: args } => args.iter_mut().for_each(f),
        ExprKind::StructUpdate { base, updates, .. } => {
            f(base);
            updates.iter_mut().for_each(|(_, e)| f(e));
        }
        ExprKind::Field { base, .. }
        | ExprKind::TupleIndex { base, .. }
        | ExprKind::CtxGet { base, .. }
        | ExprKind::Try { base, .. } => f(base),
        ExprKind::Index { base, index, .. } => {
            f(base);
            f(index);
        }
        ExprKind::Block { stmts, tail } => {
            for s in stmts {
                match s {
                    Stmt::Let { value, .. } => f(value),
                    Stmt::Expr(e) => f(e),
                }
            }
            if let Some(t) = tail {
                f(t);
            }
        }
        ExprKind::If { cond, then, else_ } => {
            f(cond);
            f(then);
            f(else_);
        }
        ExprKind::Match { scrutinee, arms } => {
            f(scrutinee);
            for a in arms {
                if let Some(g) = &mut a.guard {
                    f(g);
                }
                f(&mut a.body);
            }
        }
        ExprKind::Lambda { body, .. } => f(body),
        ExprKind::And { lhs, rhs } | ExprKind::Or { lhs, rhs } => {
            f(lhs);
            f(rhs);
        }
        ExprKind::Template { parts } => {
            for p in parts {
                if let TemplatePart::Hole(h) = p {
                    f(h);
                }
            }
        }
        ExprKind::CtxLit { bindings } => bindings.iter_mut().for_each(|(_, e)| f(e)),
        ExprKind::Int(..)
        | ExprKind::Float(_)
        | ExprKind::Str(_)
        | ExprKind::Char(_)
        | ExprKind::Bool(_)
        | ExprKind::Unit
        | ExprKind::Local(_)
        | ExprKind::Const(_)
        | ExprKind::FnRef(_)
        | ExprKind::CtxCall { .. }
        | ExprKind::Error => {}
    }
}

/// Walks every sub-expression, outermost first.
pub fn walk<'a>(e: &'a Expr, f: &mut impl FnMut(&'a Expr)) {
    f(e);
    children(e, &mut |c| walk(c, f));
}
