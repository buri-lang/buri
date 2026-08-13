//! The abstract syntax tree.
//!
//! One node per production in `grammar.ebnf`, with the deliberate exception
//! named in SPEC 12.16: a method call has no node of its own. `sq.area()` is a
//! `Call` whose callee is a `Field`, and which of the four meanings that `.`
//! carries — field, tuple index, module member, method — is settled during
//! name resolution rather than during parsing.

use crate::diag::Span;

#[derive(Clone, Debug)]
pub struct Ident {
    pub name: String,
    pub span: Span,
}

impl Ident {
    pub fn new(name: impl Into<String>, span: Span) -> Ident {
        Ident { name: name.into(), span }
    }
}

// ---------------------------------------------------------------------------
// Compilation unit
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Default)]
pub struct Module {
    pub items: Vec<Item>,
}

#[derive(Clone, Debug)]
pub enum Item {
    Import(Import),
    ReExport(ReExport),
    Fn(FnDecl),
    Struct(StructDecl),
    Enum(EnumDecl),
    TypeAlias(TypeAliasDecl),
    Const(ConstDecl),
    Trait(TraitDecl),
    Impl(ImplDecl),
    Derive(DeriveDecl),
    Context(ContextDecl),
    Test(TestDecl),
}

impl Item {
    pub fn span(&self) -> Span {
        match self {
            Item::Import(i) => i.span,
            Item::ReExport(i) => i.span,
            Item::Fn(i) => i.span,
            Item::Struct(i) => i.span,
            Item::Enum(i) => i.span,
            Item::TypeAlias(i) => i.span,
            Item::Const(i) => i.span,
            Item::Trait(i) => i.span,
            Item::Impl(i) => i.span,
            Item::Derive(i) => i.span,
            Item::Context(i) => i.span,
            Item::Test(i) => i.span,
        }
    }

    /// The name a declaration introduces, if it introduces one.
    pub fn declared_name(&self) -> Option<&Ident> {
        match self {
            Item::Fn(d) => Some(&d.name),
            Item::Struct(d) => Some(&d.name),
            Item::Enum(d) => Some(&d.name),
            Item::TypeAlias(d) => Some(&d.name),
            Item::Const(d) => Some(&d.name),
            Item::Trait(d) => Some(&d.name),
            Item::Context(d) => Some(&d.name),
            _ => None,
        }
    }

    pub fn is_exported(&self) -> bool {
        match self {
            Item::Fn(d) => d.exported,
            Item::Struct(d) => d.exported,
            Item::Enum(d) => d.exported,
            Item::TypeAlias(d) => d.exported,
            Item::Const(d) => d.exported,
            Item::Trait(d) => d.exported,
            Item::Context(d) => d.exported,
            _ => false,
        }
    }
}

// ---------------------------------------------------------------------------
// Imports
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct Import {
    pub path: String,
    pub path_span: Span,
    pub clause: ImportClause,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub enum ImportClause {
    /// `import { a, b as c }`
    Named(Vec<ImportSpec>),
    /// `import * as list`. A namespace import must be named; bare `import *`
    /// is not derivable from the grammar.
    Namespace(Ident),
}

#[derive(Clone, Debug)]
pub struct ImportSpec {
    pub name: Ident,
    pub alias: Option<Ident>,
    pub span: Span,
}

impl ImportSpec {
    /// The name this specifier binds locally.
    pub fn local(&self) -> &Ident {
        self.alias.as_ref().unwrap_or(&self.name)
    }
}

#[derive(Clone, Debug)]
pub struct ReExport {
    pub path: String,
    pub path_span: Span,
    pub specs: Vec<ImportSpec>,
    pub span: Span,
}

// ---------------------------------------------------------------------------
// Declarations
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct GenericParam {
    pub name: Ident,
    pub bounds: Vec<TypeExpr>,
    pub span: Span,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ParamKind {
    /// Written literally `self`. A function is a method if and only if its
    /// first parameter is this (SPEC 6.7.1).
    SelfParam,
    /// Written literally `ctx`, first or immediately after `self`.
    CtxParam,
    Normal,
}

#[derive(Clone, Debug)]
pub struct Param {
    pub kind: ParamKind,
    pub name: Ident,
    pub ty: TypeExpr,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct FnDecl {
    pub name: Ident,
    pub generics: Vec<GenericParam>,
    pub params: Vec<Param>,
    pub ret: TypeExpr,
    /// `None` for a trait or effect method signature, and for the
    /// signature-only declarations the embedded standard library uses for
    /// operations the backend supplies.
    pub body: Option<Block>,
    pub exported: bool,
    pub span: Span,
    pub docs: Vec<String>,
}

impl FnDecl {
    pub fn is_method(&self) -> bool {
        matches!(self.params.first().map(|p| p.kind), Some(ParamKind::SelfParam))
    }

    pub fn ctx_param(&self) -> Option<&Param> {
        self.params.iter().find(|p| p.kind == ParamKind::CtxParam)
    }
}

#[derive(Clone, Debug)]
pub struct StructDecl {
    pub name: Ident,
    pub generics: Vec<GenericParam>,
    pub body: StructBody,
    pub exported: bool,
    pub span: Span,
    pub docs: Vec<String>,
}

#[derive(Clone, Debug)]
pub enum StructBody {
    Record(Vec<FieldDecl>),
    Tuple(Vec<TupleField>),
}

#[derive(Clone, Debug)]
pub struct FieldDecl {
    pub exported: bool,
    pub name: Ident,
    pub ty: TypeExpr,
    pub span: Span,
    pub docs: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct TupleField {
    pub exported: bool,
    pub ty: TypeExpr,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct EnumDecl {
    pub name: Ident,
    pub generics: Vec<GenericParam>,
    pub variants: Vec<Variant>,
    pub exported: bool,
    pub span: Span,
    pub docs: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct Variant {
    pub exported: bool,
    pub name: Ident,
    pub payload: VariantPayload,
    pub span: Span,
    pub docs: Vec<String>,
}

#[derive(Clone, Debug)]
pub enum VariantPayload {
    None,
    Tuple(Vec<TypeExpr>),
    Record(Vec<FieldDecl>),
}

#[derive(Clone, Debug)]
pub struct TypeAliasDecl {
    pub name: Ident,
    pub generics: Vec<GenericParam>,
    pub ty: TypeExpr,
    pub exported: bool,
    pub span: Span,
    pub docs: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct ConstDecl {
    pub name: Ident,
    pub ty: TypeExpr,
    pub value: Expr,
    pub exported: bool,
    pub span: Span,
    pub docs: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct TraitDecl {
    pub name: Ident,
    pub generics: Vec<GenericParam>,
    pub methods: Vec<FnDecl>,
    /// Declared with `effect` rather than `trait`. The only difference is that
    /// implementors are effect-carrying (SPEC 10.1).
    pub is_effect: bool,
    pub exported: bool,
    pub span: Span,
    pub docs: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct ImplDecl {
    pub generics: Vec<GenericParam>,
    pub trait_ty: TypeExpr,
    pub self_ty: TypeExpr,
    pub methods: Vec<FnDecl>,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct DeriveDecl {
    pub traits: Vec<TypeExpr>,
    pub self_ty: TypeExpr,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct ContextDecl {
    pub name: Ident,
    pub body: ContextBody,
    pub exported: bool,
    pub span: Span,
    pub docs: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct ContextBody {
    /// `..Hermetic()` — takes every binding from another context, which the
    /// bindings that follow replace rather than duplicate.
    pub spread: Option<Box<Expr>>,
    pub bindings: Vec<ContextBinding>,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct ContextBinding {
    /// A `NamedType` because it names an effect, possibly qualified
    /// (`cap.Alloc`).
    pub effect: TypeExpr,
    pub value: Expr,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct TestDecl {
    pub name: String,
    pub name_span: Span,
    pub body: Block,
    pub span: Span,
    pub docs: Vec<String>,
}

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub enum TypeExpr {
    Named { path: Vec<Ident>, args: Vec<TypeExpr>, span: Span },
    SelfType { span: Span },
    Unit { span: Span },
    Tuple { elems: Vec<TypeExpr>, span: Span },
    Array { elem: Box<TypeExpr>, span: Span },
    Fn { params: Vec<TypeExpr>, ret: Box<TypeExpr>, span: Span },
}

impl TypeExpr {
    pub fn span(&self) -> Span {
        match self {
            TypeExpr::Named { span, .. }
            | TypeExpr::SelfType { span }
            | TypeExpr::Unit { span }
            | TypeExpr::Tuple { span, .. }
            | TypeExpr::Array { span, .. }
            | TypeExpr::Fn { span, .. } => *span,
        }
    }

    /// The trailing segment of a named type's path — the name itself.
    pub fn head_name(&self) -> Option<&str> {
        match self {
            TypeExpr::Named { path, .. } => path.last().map(|i| i.name.as_str()),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// Expressions
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BinOp {
    Or,
    Coalesce,
    And,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    BitOr,
    BitXor,
    BitAnd,
    Add,
    Sub,
    Mul,
    Div,
    Rem,
}

impl BinOp {
    pub fn text(self) -> &'static str {
        match self {
            BinOp::Or => "||",
            BinOp::Coalesce => "??",
            BinOp::And => "&&",
            BinOp::Eq => "==",
            BinOp::Ne => "!=",
            BinOp::Lt => "<",
            BinOp::Le => "<=",
            BinOp::Gt => ">",
            BinOp::Ge => ">=",
            BinOp::BitOr => "|",
            BinOp::BitXor => "^",
            BinOp::BitAnd => "&",
            BinOp::Add => "+",
            BinOp::Sub => "-",
            BinOp::Mul => "*",
            BinOp::Div => "/",
            BinOp::Rem => "%",
        }
    }

    /// The trait method this operator desugars to (SPEC 5.12.4).
    pub fn trait_method(self) -> Option<(&'static str, &'static str)> {
        Some(match self {
            BinOp::Add => ("Add", "add"),
            BinOp::Sub => ("Sub", "sub"),
            BinOp::Mul => ("Mul", "mul"),
            BinOp::Div => ("Div", "div"),
            BinOp::Rem => ("Rem", "rem"),
            BinOp::Eq | BinOp::Ne => ("Eq", "eq"),
            BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => ("Ord", "compare"),
            _ => return None,
        })
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum UnOp {
    Neg,
    Not,
    BitNot,
}

impl UnOp {
    pub fn text(self) -> &'static str {
        match self {
            UnOp::Neg => "-",
            UnOp::Not => "!",
            UnOp::BitNot => "~",
        }
    }
}

#[derive(Clone, Debug)]
pub enum TemplatePart {
    Text(String),
    Hole(Expr),
}

#[derive(Clone, Debug)]
pub struct MatchArm {
    pub pattern: Pattern,
    pub guard: Option<Expr>,
    pub body: Expr,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct FieldInit {
    pub name: Ident,
    /// `None` is the shorthand form: `Point { x, y }`.
    pub value: Option<Expr>,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct LambdaParam {
    pub name: Ident,
    pub ty: Option<TypeExpr>,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub enum Expr {
    Int { value: u128, raw: String, span: Span },
    Float { value: f64, raw: String, span: Span },
    Str { value: String, span: Span },
    Char { value: char, span: Span },
    Bool { value: bool, span: Span },
    Template { parts: Vec<TemplatePart>, span: Span },
    Ident { name: String, span: Span },
    SelfValue { span: Span },
    Ctx { span: Span },
    /// `.Variant` — the inferred-type dot form, which requires a known
    /// expected type (SPEC 14.12).
    DotVariant { name: Ident, span: Span },
    Unit { span: Span },
    Array { elems: Vec<Expr>, span: Span },
    Tuple { elems: Vec<Expr>, span: Span },
    Block(Block),
    If { cond: Box<Expr>, then: Block, else_: Box<Expr>, span: Span },
    Match { scrutinee: Box<Expr>, arms: Vec<MatchArm>, span: Span },
    ContextExpr { body: ContextBody, span: Span },
    Lambda { params: Vec<LambdaParam>, ret: Option<TypeExpr>, body: Box<Expr>, span: Span },
    Crash { message: Box<Expr>, span: Span },
    Unary { op: UnOp, operand: Box<Expr>, span: Span },
    Binary { op: BinOp, lhs: Box<Expr>, rhs: Box<Expr>, op_span: Span, span: Span },
    /// `base.name` — field, module member, or method, decided after parsing.
    Field { base: Box<Expr>, name: Ident, span: Span },
    /// `base.0`
    TupleIndex { base: Box<Expr>, index: u32, index_span: Span, span: Span },
    Call { callee: Box<Expr>, args: Vec<Expr>, span: Span },
    /// `base[i]`, which yields `Option<T>`.
    Index { base: Box<Expr>, index: Box<Expr>, span: Span },
    /// Postfix `?`.
    Try { base: Box<Expr>, span: Span },
    /// `base::<T, U>`
    TurboFish { base: Box<Expr>, args: Vec<TypeExpr>, span: Span },
    StructLit {
        head: Box<Expr>,
        spread: Option<Box<Expr>>,
        fields: Vec<FieldInit>,
        span: Span,
    },
}

impl Expr {
    pub fn span(&self) -> Span {
        match self {
            Expr::Int { span, .. }
            | Expr::Float { span, .. }
            | Expr::Str { span, .. }
            | Expr::Char { span, .. }
            | Expr::Bool { span, .. }
            | Expr::Template { span, .. }
            | Expr::Ident { span, .. }
            | Expr::SelfValue { span }
            | Expr::Ctx { span }
            | Expr::DotVariant { span, .. }
            | Expr::Unit { span }
            | Expr::Array { span, .. }
            | Expr::Tuple { span, .. }
            | Expr::If { span, .. }
            | Expr::Match { span, .. }
            | Expr::ContextExpr { span, .. }
            | Expr::Lambda { span, .. }
            | Expr::Crash { span, .. }
            | Expr::Unary { span, .. }
            | Expr::Binary { span, .. }
            | Expr::Field { span, .. }
            | Expr::TupleIndex { span, .. }
            | Expr::Call { span, .. }
            | Expr::Index { span, .. }
            | Expr::Try { span, .. }
            | Expr::TurboFish { span, .. }
            | Expr::StructLit { span, .. } => *span,
            Expr::Block(b) => b.span,
        }
    }

    /// Brace-terminated and self-delimiting: may be an operand of a binary
    /// operator, but may not head a postfix chain (SPEC 12.11, 12.13).
    pub fn is_block_like(&self) -> bool {
        matches!(
            self,
            Expr::Block(_) | Expr::If { .. } | Expr::Match { .. } | Expr::ContextExpr { .. }
        )
    }

    /// Whether this expression is a legal head for a struct literal — a type
    /// path, optionally with a turbofish, or the dot form (SPEC 14.1).
    pub fn is_type_path(&self) -> bool {
        match self {
            Expr::Ident { .. } | Expr::DotVariant { .. } => true,
            Expr::TurboFish { base, .. } => base.is_type_path(),
            Expr::Field { base, .. } => base.is_type_path(),
            _ => false,
        }
    }
}

// ---------------------------------------------------------------------------
// Patterns
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub enum PatPayload {
    Tuple(Vec<Pattern>),
    /// `..` at the end ignores remaining fields; without it a struct pattern
    /// must mention every field.
    Record { fields: Vec<FieldPat>, rest: bool },
}

#[derive(Clone, Debug)]
pub struct FieldPat {
    pub name: Ident,
    /// `None` is the shorthand form: `User { id, name }`.
    pub pattern: Option<Pattern>,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub enum Pattern {
    Wild {
        span: Span,
    },
    /// A bare identifier is ALWAYS a binding. `None` as a pattern binds a
    /// variable named `None` (SPEC 7.2).
    Bind {
        name: Ident,
        sub: Option<Box<Pattern>>,
        span: Span,
    },
    LitInt {
        value: u128,
        negative: bool,
        raw: String,
        span: Span,
    },
    LitFloat {
        value: f64,
        negative: bool,
        raw: String,
        span: Span,
    },
    LitStr {
        value: String,
        span: Span,
    },
    LitChar {
        value: char,
        span: Span,
    },
    LitBool {
        value: bool,
        span: Span,
    },
    /// A struct pattern (`User { .. }`, `Meters(m)`), a qualified variant
    /// (`Option.Some(x)`), or the dot form (`.Some(x)`).
    Path {
        path: Vec<Ident>,
        dotted: bool,
        payload: Option<PatPayload>,
        span: Span,
    },
    Unit {
        span: Span,
    },
    Tuple {
        elems: Vec<Pattern>,
        span: Span,
    },
    /// Rest patterns bind only at the end: `[first, ..rest]` is legal,
    /// `[..init, last]` is not.
    Array {
        elems: Vec<Pattern>,
        rest: Option<Option<Ident>>,
        span: Span,
    },
    Or {
        alts: Vec<Pattern>,
        span: Span,
    },
}

impl Pattern {
    pub fn span(&self) -> Span {
        match self {
            Pattern::Wild { span }
            | Pattern::Bind { span, .. }
            | Pattern::LitInt { span, .. }
            | Pattern::LitFloat { span, .. }
            | Pattern::LitStr { span, .. }
            | Pattern::LitChar { span, .. }
            | Pattern::LitBool { span, .. }
            | Pattern::Path { span, .. }
            | Pattern::Unit { span }
            | Pattern::Tuple { span, .. }
            | Pattern::Array { span, .. }
            | Pattern::Or { span, .. } => *span,
        }
    }

    /// Collects every name this pattern binds, in source order.
    pub fn bindings(&self, out: &mut Vec<Ident>) {
        match self {
            Pattern::Bind { name, sub, .. } => {
                out.push(name.clone());
                if let Some(s) = sub {
                    s.bindings(out);
                }
            }
            Pattern::Tuple { elems, .. } => elems.iter().for_each(|p| p.bindings(out)),
            Pattern::Array { elems, rest, .. } => {
                elems.iter().for_each(|p| p.bindings(out));
                if let Some(Some(name)) = rest {
                    out.push(name.clone());
                }
            }
            Pattern::Or { alts, .. } => {
                // Alternatives must bind identical names, so the first one is
                // representative. The check itself is in `check`.
                if let Some(first) = alts.first() {
                    first.bindings(out);
                }
            }
            Pattern::Path { payload, .. } => match payload {
                Some(PatPayload::Tuple(ps)) => ps.iter().for_each(|p| p.bindings(out)),
                Some(PatPayload::Record { fields, .. }) => {
                    for f in fields {
                        match &f.pattern {
                            Some(p) => p.bindings(out),
                            None => out.push(f.name.clone()),
                        }
                    }
                }
                None => {}
            },
            _ => {}
        }
    }

    /// An irrefutable pattern matches every value of its type — required of a
    /// `let` (SPEC 14.2).
    pub fn is_irrefutable(&self) -> bool {
        match self {
            Pattern::Wild { .. } | Pattern::Bind { .. } | Pattern::Unit { .. } => true,
            Pattern::Tuple { elems, .. } => elems.iter().all(|p| p.is_irrefutable()),
            // A single-variant enum or a struct pattern can be irrefutable;
            // deciding that needs types, so it is settled in `check`.
            _ => false,
        }
    }
}

// ---------------------------------------------------------------------------
// Blocks and statements
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct Block {
    pub stmts: Vec<Stmt>,
    /// The result expression. A block whose last item is a `let` has type `()`.
    pub tail: Option<Box<Expr>>,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub enum Stmt {
    Let {
        pattern: Pattern,
        ty: Option<TypeExpr>,
        value: Expr,
        /// The `let ctx = ...` form. `ctx` is a keyword, so this is a separate
        /// production, and it is legal only where a context may be built.
        is_ctx: bool,
        span: Span,
    },
    /// Legal only in a test source, and only when its type is `()`
    /// (SPEC 11.2, 14.38).
    Expr { expr: Expr, span: Span },
}

impl Stmt {
    pub fn span(&self) -> Span {
        match self {
            Stmt::Let { span, .. } | Stmt::Expr { span, .. } => *span,
        }
    }
}
