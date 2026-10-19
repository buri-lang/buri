//! The typed intermediate representation.
//!
//! Checking produces this; monomorphization rewrites it; the JS backend reads
//! it. Everything the surface syntax leaves to name resolution is settled
//! here: a `.` is already a field, a tuple index, or a call; an operator is
//! already a primitive operation or a trait method; a `match` still has
//! patterns but every path in one is resolved.

use crate::compiler::semantics::layered::Dense;
use crate::compiler::semantics::name::Name;
use crate::compiler::semantics::types::{
    CtxTypeId, FieldId, FnId, FuncIdx, LocalId, MethodId, Prim, Scalar, TraitId, Ty, TyConId,
    VariantId,
};
use crate::diagnostics::Span;

/// An integer literal's magnitude, kept as two 64-bit halves.
///
/// A `u128` field makes its whole node 16-byte aligned, and an IR instruction
/// or an expression holding one rounds up around it. Two halves hold the same
/// value at 8-byte alignment.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Magnitude([u64; 2]);

impl Magnitude {
    pub const fn new(value: u128) -> Magnitude {
        Magnitude([value as u64, (value >> 64) as u64])
    }

    pub const fn get(self) -> u128 {
        ((self.0[1] as u128) << 64) | self.0[0] as u128
    }
}

impl std::fmt::Debug for Magnitude {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.get().fmt(f)
    }
}

#[derive(Clone, Debug)]
pub struct Local {
    pub name: Name,
    pub ty: Ty,
    pub span: Span,
}

/// A body's locals, which mint its `LocalId`s.
pub type Locals = Dense<LocalId, Local>;

/// The body of a function, once checked.
#[derive(Clone, Debug)]
pub struct Body {
    pub locals: Locals,
    /// Locals holding the parameters, in declaration order.
    pub params: Vec<LocalId>,
    pub expr: Expr,
    /// Every bound of the signature's own type parameters that checking this
    /// body relied on, as `(parameter index, trait)`, sorted and deduplicated.
    /// It includes a bound reached through an `impl`'s own bounds, as in
    /// `impl<C: Clock> Allocator for Wrap<C>`.
    pub bounds_used: Vec<(u32, TraitId)>,
    /// The span of each bound the declaration's own type parameters were
    /// written with, by parameter and then in order. A method's own parameters
    /// come after its `impl`'s in its generics, and those are not here.
    pub bounds_written: Vec<Vec<Span>>,
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

    /// What a struct literal or update wrote for one field, through an
    /// update's base when the update leaves it alone.
    pub fn written_field(&self, index: FieldId) -> Option<&Expr> {
        match &self.kind {
            ExprKind::StructLit { fields, .. } => fields.get(index.index()),
            ExprKind::StructUpdate { base, updates, .. } => updates
                .iter()
                .find(|(i, _)| *i == index)
                .map(|(_, e)| e)
                .or_else(|| base.written_field(index)),
            _ => None,
        }
    }
}

#[derive(Clone, Debug)]
pub enum Stmt {
    /// `let` with an irrefutable pattern. The pattern is kept rather than
    /// flattened so the backend can emit destructuring directly.
    Let {
        pattern: Pattern,
        value: Expr,
        span: Span,
    },
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
///
/// `monomorphize` settles how each hole renders. After it, a `Hole` is a
/// string already, and a `Shown` is a primitive of another kind, which the
/// backend renders from `prim`. A hole of a compound type is a `structuralShow`
/// call by then, so no hole is left that needs a `show` the backend can't see.
#[derive(Clone, Debug)]
pub enum TemplatePart {
    Text(String),
    Hole(Expr),
    Shown { prim: Prim, value: Expr },
}

/// Which of `Option` and `Result` a `?` is working on. They are
/// ordinary enums, but the backend can emit better code knowing which.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum OptionOrResult {
    Option,
    Result,
}

/// Binary operations the backend implements directly rather than through a
/// trait. Every one of these is defined on two operands of the *same* type:
/// there is no implicit promotion of any kind.
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
    BitAnd,
    BitOr,
    BitXor,
    /// Structural equality, compiled per type.
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

/// Unary operations the backend implements directly, on one operand of the
/// result's own type.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum UnaryOp {
    Neg,
    BitNot,
    Not,
}

/// The declaration a checked body's direct call, or function-valued
/// reference, names, and the type arguments it is instantiated at.
///
/// Monomorphization replaces each with one concrete function: a
/// [`ExprKind::CallFn`] becomes a [`ExprKind::CallFunc`] and a
/// [`ExprKind::FnRef`] a [`ExprKind::FuncRef`].
#[derive(Clone, Debug)]
pub struct Callee {
    pub id: FnId,
    pub targs: Vec<Ty>,
}

#[derive(Clone, Debug)]
pub enum ExprKind {
    /// The literal's value, already checked to be representable in `ty`.
    Int(Magnitude, bool),
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
    /// [`ExprKind::FnRef`] after monomorphization: one concrete function.
    FuncRef(FuncIdx),

    /// A call through a value of function type.
    CallValue {
        callee: Box<Expr>,
        args: Vec<Expr>,
    },
    /// A direct call to a known function. After monomorphization every generic
    /// call is one of these.
    CallFn {
        func: Callee,
        args: Vec<Expr>,
    },
    /// [`ExprKind::CallFn`] after monomorphization: one concrete function.
    CallFunc {
        func: FuncIdx,
        args: Vec<Expr>,
    },
    /// A trait or effect method whose receiver type is not yet concrete.
    /// Monomorphization turns each of these into a `CallFunc`.
    CallTrait {
        trait_id: TraitId,
        method: MethodId,
        /// The type the method is being called on.
        recv: Ty,
        targs: Vec<Ty>,
        args: Vec<Expr>,
    },

    StructLit {
        con: TyConId,
        targs: Vec<Ty>,
        fields: Vec<Expr>,
    },
    /// `S { ..base, f: v }`. Never names the hidden fields, which is why it
    /// works outside the declaring module.
    StructUpdate {
        con: TyConId,
        base: Box<Expr>,
        updates: Vec<(FieldId, Expr)>,
    },
    EnumLit {
        con: TyConId,
        targs: Vec<Ty>,
        variant: VariantId,
        args: Vec<Expr>,
    },
    Tuple(Vec<Expr>),
    /// `[a, b]`, whose elements are `elem`s.
    Array {
        elem: Ty,
        elems: Vec<Expr>,
    },

    Field {
        base: Box<Expr>,
        index: FieldId,
    },
    TupleIndex {
        base: Box<Expr>,
        index: FieldId,
    },
    /// Indexing yields `Option<T>`. There is no way to index out of bounds.
    Index {
        base: Box<Expr>,
        index: Box<Expr>,
        elem: Ty,
    },

    Block {
        stmts: Vec<Stmt>,
        tail: Option<Box<Expr>>,
    },
    If {
        cond: Box<Expr>,
        then: Box<Expr>,
        else_: Box<Expr>,
    },
    Match {
        scrutinee: Box<Expr>,
        arms: Vec<Arm>,
    },

    /// Lambdas capture by value. Since values are immutable, capture is
    /// unobservable — and a lambda may not capture an effect-carrying value,
    /// which is what makes the purity theorem hold structurally.
    Lambda {
        params: Vec<LocalId>,
        body: Box<Expr>,
        captures: Vec<LocalId>,
    },

    /// `&&` and `||`, which short-circuit.
    And {
        lhs: Box<Expr>,
        rhs: Box<Expr>,
    },
    Or {
        lhs: Box<Expr>,
        rhs: Box<Expr>,
    },
    /// Postfix `?`, the only early exit in the language.
    Try {
        base: Box<Expr>,
        kind: OptionOrResult,
    },

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
    Loop {
        entries: Vec<Expr>,
    },

    /// A tail call, after elimination: rebind and jump.
    ///
    /// `func` is `None` for a jump back to the top of the enclosing
    /// [`ExprKind::Loop`], and `Some(f)` for the call that enters the function
    /// a merged group became. The entry that call selects is a dispatch
    /// parameter the backend materialises: it is a control index rather than a
    /// value of any Buri type, so it is spelled here as a number rather than
    /// smuggled in as an extra argument at a type the middle end would have
    /// had to invent.
    Continue {
        func: Option<FuncIdx>,
        entry: usize,
        args: Vec<Expr>,
    },

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
    Closure {
        func: FuncIdx,
        env: Vec<Expr>,
    },

    /// A primitive operation at a known primitive type. The type is not
    /// optional: every operation here is chosen *because* the operand type
    /// resolved to a primitive, and a missing one used to default to `I64` in
    /// the backend — which is integer division semantics for a float divide.
    Prim {
        op: PrimOp,
        prim: Prim,
        args: Box<[Expr; 2]>,
    },
    /// [`ExprKind::Prim`]'s one-operand operations, none of which is at a
    /// string.
    Unary {
        op: UnaryOp,
        prim: Scalar,
        arg: Box<Expr>,
    },

    Template {
        parts: Vec<TemplatePart>,
    },

    /// `context { ... }`, the only construct in which more than one
    /// effect-carrying value may appear.
    CtxLit {
        bindings: Vec<(TraitId, Expr)>,
        /// The context type the literal builds, which is also its `ty`.
        ctx: CtxTypeId,
    },
    /// Reading one effect implementation out of a context value.
    CtxGet {
        base: Box<Expr>,
        trait_id: TraitId,
    },
    /// A call to a named context declaration, which builds a fresh one.
    CtxCall {
        decl: crate::compiler::semantics::types::ContextDeclId,
    },

    /// An operation the runtime supplies. Only the embedded standard library
    /// produces these.
    Intrinsic {
        name: String,
        targs: Vec<Ty>,
        args: Vec<Expr>,
    },

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
    pub index: FieldId,
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
    Bind {
        local: LocalId,
        sub: Option<Box<Pattern>>,
    },
    Int(Magnitude, bool),
    Float(f64),
    /// An `Int` or a `Float` once `monomorphize` has settled the primitive
    /// it compares at, which before it can be a type parameter's.
    Number {
        prim: Prim,
        value: NumberLit,
    },
    Str(String),
    Char(char),
    Bool(bool),
    Unit,
    Tuple(Vec<Pattern>),
    Struct {
        con: TyConId,
        fields: Vec<FieldPat>,
    },
    Variant {
        con: TyConId,
        variant: VariantId,
        fields: Vec<FieldPat>,
    },
    /// `[a, b, ..rest]`, of `elem`s. Rest patterns bind only at the end.
    Array {
        elem: Ty,
        elems: Vec<Pattern>,
        rest: ArrayRest,
    },
    Or(Vec<Pattern>),
    Error,
}

/// A number literal pattern's value: [`PatKind::Int`]'s, or [`PatKind::Float`]'s.
#[derive(Clone, Copy, Debug)]
pub enum NumberLit {
    Int(Magnitude, bool),
    Float(f64),
}

impl Pattern {
    /// Every local this pattern binds.
    pub fn binds(&self, out: &mut Vec<LocalId>) {
        self.each_bind(&mut |l| out.push(l));
    }

    /// Calls `f` with every local this pattern binds, in the order
    /// [`Pattern::binds`] lists them, for a caller with no use for the list.
    pub fn each_bind(&self, f: &mut impl FnMut(LocalId)) {
        match &self.kind {
            PatKind::Bind { local, sub } => {
                f(*local);
                if let Some(s) = sub {
                    s.each_bind(f);
                }
            }
            PatKind::Tuple(ps) => ps.iter().for_each(|p| p.each_bind(f)),
            PatKind::Struct { fields, .. } | PatKind::Variant { fields, .. } => {
                fields.iter().for_each(|p| p.pattern.each_bind(f))
            }
            PatKind::Array { elems, rest, .. } => {
                elems.iter().for_each(|p| p.each_bind(f));
                if let ArrayRest::Bound(l) = rest {
                    f(*l);
                }
            }
            // Alternatives bind identical names at identical types, so the
            // first is representative.
            PatKind::Or(alts) => alts.iter().take(1).for_each(|first| first.each_bind(f)),
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
            PatKind::Array { elems, rest, .. } => {
                elems.iter().for_each(|p| p.fresh_binds(out));
                if let ArrayRest::Bound(l) = rest {
                    out.push(*l);
                }
            }
            PatKind::Or(alts) => alts.iter().take(1).for_each(|first| first.fresh_binds(out)),
            _ => {}
        }
    }

    /// Whether this pattern matches every value of its type.
    pub fn is_irrefutable(&self, tables: &crate::compiler::semantics::types::Tables) -> bool {
        match &self.kind {
            PatKind::Wild | PatKind::Unit | PatKind::Error => true,
            PatKind::Bind { sub, .. } => sub
                .as_ref()
                .map(|s| s.is_irrefutable(tables))
                .unwrap_or(true),
            PatKind::Tuple(ps) => ps.iter().all(|p| p.is_irrefutable(tables)),
            PatKind::Struct { fields, .. } => {
                fields.iter().all(|f| f.pattern.is_irrefutable(tables))
            }
            // A single-variant enum has nothing to fall through to.
            PatKind::Variant { con, fields, .. } => {
                tables.tycon(*con).variants().len() == 1
                    && fields.iter().all(|f| f.pattern.is_irrefutable(tables))
            }
            PatKind::Array { elems, rest, .. } => elems.is_empty() && rest.is_open(),
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
        | ExprKind::CallFunc { args, .. }
        | ExprKind::CallTrait { args, .. }
        | ExprKind::StructLit { fields: args, .. }
        | ExprKind::EnumLit { args, .. }
        | ExprKind::Tuple(args)
        | ExprKind::Array { elems: args, .. }
        | ExprKind::Intrinsic { args, .. }
        // `Continue`'s arguments and `Loop`'s entries are children like any
        // other. A walk that left them out reported a whole loop as one node,
        // and left a `structuralEq` inside one for `lower` to meet; both were
        // real bugs, in two separate copies of this match.
        | ExprKind::Continue { args, .. }
        | ExprKind::Closure { env: args, .. }
        | ExprKind::Loop { entries: args } => args.iter().for_each(f),
        ExprKind::Prim { args, .. } => args.iter().for_each(f),
        ExprKind::StructUpdate { base, updates, .. } => {
            f(base);
            updates.iter().for_each(|(_, e)| f(e));
        }
        ExprKind::Field { base, .. }
        | ExprKind::TupleIndex { base, .. }
        | ExprKind::CtxGet { base, .. }
        | ExprKind::Unary { arg: base, .. }
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
                if let TemplatePart::Hole(h) | TemplatePart::Shown { value: h, .. } = p {
                    f(h);
                }
            }
        }
        ExprKind::CtxLit { bindings, .. } => bindings.iter().for_each(|(_, e)| f(e)),
        ExprKind::Int(..)
        | ExprKind::Float(_)
        | ExprKind::Str(_)
        | ExprKind::Char(_)
        | ExprKind::Bool(_)
        | ExprKind::Unit
        | ExprKind::Local(_)
        | ExprKind::Const(_)
        | ExprKind::FnRef(_)
        | ExprKind::FuncRef(_)
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
pub fn children_mut<'a>(e: &'a mut Expr, f: &mut impl FnMut(&'a mut Expr)) {
    match &mut e.kind {
        ExprKind::CallValue { callee, args } => {
            f(callee);
            args.iter_mut().for_each(f);
        }
        ExprKind::CallFn { args, .. }
        | ExprKind::CallFunc { args, .. }
        | ExprKind::CallTrait { args, .. }
        | ExprKind::StructLit { fields: args, .. }
        | ExprKind::EnumLit { args, .. }
        | ExprKind::Tuple(args)
        | ExprKind::Array { elems: args, .. }
        | ExprKind::Intrinsic { args, .. }
        // `Continue`'s arguments and `Loop`'s entries are children like any
        // other. A walk that left them out reported a whole loop as one node,
        // and left a `structuralEq` inside one for `lower` to meet; both were
        // real bugs, in two separate copies of this match.
        | ExprKind::Continue { args, .. }
        | ExprKind::Closure { env: args, .. }
        | ExprKind::Loop { entries: args } => args.iter_mut().for_each(f),
        ExprKind::Prim { args, .. } => args.iter_mut().for_each(f),
        ExprKind::StructUpdate { base, updates, .. } => {
            f(base);
            updates.iter_mut().for_each(|(_, e)| f(e));
        }
        ExprKind::Field { base, .. }
        | ExprKind::TupleIndex { base, .. }
        | ExprKind::CtxGet { base, .. }
        | ExprKind::Unary { arg: base, .. }
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
            for Arm { guard, body, .. } in arms {
                if let Some(g) = guard {
                    f(g);
                }
                f(body);
            }
        }
        ExprKind::Lambda { body, .. } => f(body),
        ExprKind::And { lhs, rhs } | ExprKind::Or { lhs, rhs } => {
            f(lhs);
            f(rhs);
        }
        ExprKind::Template { parts } => {
            for p in parts {
                if let TemplatePart::Hole(h) | TemplatePart::Shown { value: h, .. } = p {
                    f(h);
                }
            }
        }
        ExprKind::CtxLit { bindings, .. } => bindings.iter_mut().for_each(|(_, e)| f(e)),
        ExprKind::Int(..)
        | ExprKind::Float(_)
        | ExprKind::Str(_)
        | ExprKind::Char(_)
        | ExprKind::Bool(_)
        | ExprKind::Unit
        | ExprKind::Local(_)
        | ExprKind::Const(_)
        | ExprKind::FnRef(_)
        | ExprKind::FuncRef(_)
        | ExprKind::CtxCall { .. }
        | ExprKind::Error => {}
    }
}

/// Walks every sub-expression, outermost first.
pub fn walk<'a>(e: &'a Expr, f: &mut impl FnMut(&'a Expr)) {
    f(e);
    children(e, &mut |c| walk(c, f));
}
