//! The abstract syntax tree.
//!
//! One node per production in `grammar.ebnf`, with the deliberate exception
//! named in SPEC 12.16: a method call has no node of its own. `sq.area()` is a
//! `Call` whose callee is a `Field`, and which of the four meanings that `.`
//! carries — field, tuple index, module member, method — is settled during
//! name resolution rather than during parsing.

use crate::diagnostics::Span;

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
    /// `//!` lines at the top of the file. These document the module itself,
    /// which is what `buri docs <module>` prints above the item list.
    pub docs: Vec<String>,
    /// Everything below the declaration level, flattened — see
    /// [`flat`](crate::parsing::flat). A declaration holds the id of its body;
    /// there is no other representation of one.
    pub tree: crate::parsing::flat::Tree,
}

/// A declaration.
///
/// Every variant is boxed, which makes an `Item` sixteen bytes rather than as
/// wide as the widest declaration there is. The parser builds one through
/// `fn_decl` → `Item::Fn` → `Some` → `Ok` → `items.push`, so the unboxed form
/// was copied four or five times per declaration — and a file of small
/// functions is nothing but declarations. Boxing uniformly rather than only
/// where it pays is what keeps the width from being a property of whichever
/// declaration happens to have the most fields.
#[derive(Clone, Debug)]
pub enum Item {
    Import(Box<Import>),
    ReExport(Box<ReExport>),
    Fn(Box<FnDecl>),
    Struct(Box<StructDecl>),
    Enum(Box<EnumDecl>),
    TypeAlias(Box<TypeAliasDecl>),
    Const(Box<ConstDecl>),
    Trait(Box<TraitDecl>),
    Impl(Box<ImplDecl>),
    Derive(Box<DeriveDecl>),
    Context(Box<ContextDecl>),
    Test(Box<TestDecl>),
}

const _: () = assert!(std::mem::size_of::<Item>() == 16);

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
    ///
    /// The body lives in [`Module::tree`]; this names it.
    pub body: Option<crate::parsing::flat::BlockId>,
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
    pub value: crate::parsing::flat::ExprId,
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
    pub docs: Vec<String>,
    pub generics: Vec<GenericParam>,
    /// `None` for an inherent `impl Type { ... }`, which declares the type's
    /// own methods. `Some` for `impl Trait for Type`, which declares
    /// conformance and supplies the trait's methods.
    pub trait_ty: Option<TypeExpr>,
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
    pub body: crate::parsing::flat::CtxBodyId,
    pub exported: bool,
    pub span: Span,
    pub docs: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct TestDecl {
    pub name: String,
    pub name_span: Span,
    pub body: crate::parsing::flat::BlockId,
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
