//! The abstract syntax tree.
//!
//! One node per production in `grammar.ebnf`, with the deliberate exception
//! named in design/grammar-rationale.md 12.16: a method call has no node of its
//! own. `sq.area()` is a `Call` whose callee is a `Field`, and which of the four
//! meanings that `.` carries — field, tuple index, module member, method — is
//! settled during name resolution rather than during parsing.

use crate::diagnostics::Span;
use crate::parsing::flat::{Docs, List, NONE, TypeId, TypeList};

/// A name a declaration introduces, or one segment of a written path.
///
/// The text is the source under the span — `Tree::name` is the only way to
/// read it — so a name costs nothing to store and nothing to build. Every
/// identifier the parser ever built held exactly `src[span]`, including the
/// three synthetic ones: `self` and `ctx` are spelled at the keyword's own
/// span.
#[derive(Clone, Copy, Debug)]
pub struct Name {
    pub span: Span,
}

impl Name {
    pub fn new(span: Span) -> Name {
        Name { span }
    }
}

// A name is its span and nothing else. The `String` that used to sit beside it
// was one allocation per declared name — about four hundred and fifty per
// thousand lines — and a name that grows storage again is a compile error here
// rather than a number in a later report.
const _: () = assert!(std::mem::size_of::<Name>() == 12);
const _: () = assert!(std::mem::size_of::<Param>() == 32);

// ---------------------------------------------------------------------------
// Compilation unit
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Default)]
pub struct Module {
    pub items: Vec<Item>,
    /// `//!` lines at the top of the file. These document the module itself,
    /// which is what `buri docs <module>` prints above the item list.
    pub docs: Docs,
    /// Everything below the declaration level, flattened — see
    /// [`flat`](crate::parsing::flat). A declaration holds the id of its body,
    /// of each type it names, and the span of each name it introduces; there
    /// is no other representation of any of them, and reading one back needs
    /// this.
    pub tree: crate::parsing::flat::Tree,
}

/// A declaration.
///
/// Held inline rather than boxed. A declaration's lists and doc comments are
/// ranges into [`Module::tree`]'s arenas, which keeps the widest variant at a
/// few dozen bytes, so `items` is itself the arena: one allocation for every
/// declaration in the file, where a `Box` each used to be one apiece.
#[derive(Clone, Debug)]
pub enum Item {
    Import(Import),
    ReExport(ReExport),
    Fn(FnDecl),
    Struct(StructDecl),
    Enum(EnumDecl),
    TypeAlias(TypeAliasDecl),
    Let(LetDecl),
    Trait(TraitDecl),
    Impl(ImplDecl),
    Derive(DeriveDecl),
    Context(ContextDecl),
    Test(TestDecl),
    /// A declaration that did not parse, as the extent recovery skipped over.
    Error(Span),
}

// The width is pinned for the same reason a node's is: a declaration that
// grows a field grows every item in every file.
const _: () = assert!(std::mem::size_of::<Item>() == 72);

impl Module {
    /// Moves this module to `to`: what parsing it as file `to` would have
    /// given, from a parse as file `from`. A loader parses a file before it
    /// knows the id the file will have, and this is the one place that has
    /// to know every span a module holds.
    pub fn refile(&mut self, from: crate::diagnostics::FileId, to: crate::diagnostics::FileId) {
        use crate::parsing::flat::refile;
        let at = |s: &mut Span| refile(s, from, to);
        let name = |n: &mut Name| refile(&mut n.span, from, to);
        for item in &mut self.items {
            match item {
                Item::Import(i) => {
                    at(&mut i.path_span);
                    if let ImportClause::Namespace(n) = &mut i.clause {
                        name(n);
                    }
                    at(&mut i.span);
                }
                Item::ReExport(r) => {
                    at(&mut r.path_span);
                    at(&mut r.span);
                }
                Item::Fn(d) => {
                    name(&mut d.name);
                    at(&mut d.span);
                }
                Item::Struct(d) => {
                    name(&mut d.name);
                    at(&mut d.span);
                }
                Item::Enum(d) => {
                    name(&mut d.name);
                    at(&mut d.span);
                }
                Item::TypeAlias(d) => {
                    name(&mut d.name);
                    at(&mut d.span);
                }
                Item::Let(d) => {
                    name(&mut d.name);
                    at(&mut d.span);
                }
                Item::Trait(d) => {
                    name(&mut d.name);
                    at(&mut d.span);
                }
                Item::Impl(d) => at(&mut d.span),
                Item::Derive(d) => at(&mut d.span),
                Item::Context(d) => {
                    name(&mut d.name);
                    at(&mut d.span);
                }
                Item::Test(d) => {
                    at(&mut d.name_span);
                    at(&mut d.span);
                }
                Item::Error(span) => at(span),
            }
        }
        self.tree.refile(from, to);
    }
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
            Item::Let(i) => i.span,
            Item::Trait(i) => i.span,
            Item::Impl(i) => i.span,
            Item::Derive(i) => i.span,
            Item::Context(i) => i.span,
            Item::Test(i) => i.span,
            Item::Error(at) => *at,
        }
    }


    pub fn is_exported(&self) -> bool {
        match self {
            Item::Fn(d) => d.exported,
            Item::Struct(d) => d.exported,
            Item::Enum(d) => d.exported,
            Item::TypeAlias(d) => d.exported,
            Item::Let(d) => d.exported,
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
    Named(List<ImportSpec>),
    /// `import * as list`. A namespace import must be named; bare `import *`
    /// is not derivable from the grammar.
    Namespace(Name),
}

#[derive(Clone, Debug)]
pub struct ImportSpec {
    pub name: Name,
    pub alias: Option<Name>,
    pub span: Span,
}

impl ImportSpec {
    /// The name this specifier binds locally.
    pub fn local(&self) -> Name {
        self.alias.unwrap_or(self.name)
    }
}

#[derive(Clone, Debug)]
pub struct ReExport {
    pub path: String,
    pub path_span: Span,
    pub specs: List<ImportSpec>,
    pub span: Span,
}

// ---------------------------------------------------------------------------
// Declarations
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct GenericParam {
    pub name: Name,
    pub bounds: TypeList,
    pub span: Span,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ParamKind {
    /// Written literally `self`, with no type. A function is a method if and
    /// only if its first parameter is this (SPEC 6.7.1).
    SelfParam,
    /// Written literally `ctx`, first or immediately after `self`.
    CtxParam,
    Normal,
}

#[derive(Clone, Debug)]
pub struct Param {
    pub kind: ParamKind,
    pub name: Name,
    /// The written type, or [`flat::NONE`] where none is written. Read through
    /// [`Param::written_type`]; the sentinel rather than an `Option` because
    /// `TypeId` has no niche and the `Option` would grow every parameter.
    pub ty: u32,
    pub span: Span,
}

impl Param {
    /// `None` for `self`, which writes no type: it is the `impl` head's type,
    /// or the implementing type of the trait the signature is declared in.
    pub fn written_type(&self) -> Option<TypeId> {
        (self.ty != NONE).then_some(TypeId(self.ty))
    }
}

#[derive(Clone, Debug)]
pub struct FnDecl {
    pub name: Name,
    pub generics: List<GenericParam>,
    pub params: List<Param>,
    pub ret: TypeId,
    /// `None` for a trait or effect method signature, and for the
    /// signature-only declarations the embedded standard library uses for
    /// operations the backend supplies.
    ///
    /// The body lives in [`Module::tree`]; this names it.
    pub body: Option<crate::parsing::flat::BlockId>,
    pub exported: bool,
    pub span: Span,
    pub docs: Docs,
}

#[derive(Clone, Debug)]
pub struct StructDecl {
    pub name: Name,
    pub generics: List<GenericParam>,
    pub body: StructBody,
    pub exported: bool,
    pub span: Span,
    pub docs: Docs,
}

#[derive(Clone, Debug)]
pub enum StructBody {
    Record(List<FieldDecl>),
    Tuple(List<TupleField>),
}

#[derive(Clone, Debug)]
pub struct FieldDecl {
    pub exported: bool,
    pub name: Name,
    pub ty: TypeId,
    pub span: Span,
    pub docs: Docs,
}

#[derive(Clone, Debug)]
pub struct TupleField {
    pub exported: bool,
    pub ty: TypeId,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct EnumDecl {
    pub name: Name,
    pub generics: List<GenericParam>,
    pub variants: List<Variant>,
    pub exported: bool,
    pub span: Span,
    pub docs: Docs,
}

/// A variant carries no visibility of its own: it is exported exactly when the
/// enum that declares it is, and so are the fields of its payload.
#[derive(Clone, Debug)]
pub struct Variant {
    pub name: Name,
    pub payload: VariantPayload,
    pub span: Span,
    pub docs: Docs,
}

#[derive(Clone, Debug)]
pub enum VariantPayload {
    None,
    Tuple(TypeList),
    Record(List<FieldDecl>),
}

#[derive(Clone, Debug)]
pub struct TypeAliasDecl {
    pub name: Name,
    pub generics: List<GenericParam>,
    pub ty: TypeId,
    pub exported: bool,
    pub span: Span,
    pub docs: Docs,
}

/// A module-level `let`. The block-level one is a `Stmt`, and differs in that
/// it binds a pattern and may leave the type to inference.
#[derive(Clone, Debug)]
pub struct LetDecl {
    pub name: Name,
    pub ty: TypeId,
    pub value: crate::parsing::flat::ExprId,
    pub exported: bool,
    pub span: Span,
    pub docs: Docs,
}

#[derive(Clone, Debug)]
pub struct TraitDecl {
    pub name: Name,
    pub generics: List<GenericParam>,
    pub methods: List<FnDecl>,
    /// Declared with `effect` rather than `trait`. The only difference is that
    /// implementors are effect-carrying (SPEC 10.1).
    pub is_effect: bool,
    pub exported: bool,
    pub span: Span,
    pub docs: Docs,
}

#[derive(Clone, Debug)]
pub struct ImplDecl {
    pub docs: Docs,
    pub generics: List<GenericParam>,
    /// `None` for an inherent `impl Type { ... }`, which declares the type's
    /// own methods. `Some` for `impl Trait for Type`, which declares
    /// conformance and supplies the trait's methods.
    pub trait_ty: Option<TypeId>,
    pub self_ty: TypeId,
    pub methods: List<FnDecl>,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct DeriveDecl {
    pub traits: TypeList,
    pub self_ty: TypeId,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct ContextDecl {
    pub name: Name,
    pub body: crate::parsing::flat::CtxBodyId,
    pub exported: bool,
    pub span: Span,
    pub docs: Docs,
}

#[derive(Clone, Debug)]
pub struct TestDecl {
    pub name: String,
    pub name_span: Span,
    pub body: crate::parsing::flat::BlockId,
    pub span: Span,
    pub docs: Docs,
}

// ---------------------------------------------------------------------------
// Expressions
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BinOp {
    Or,
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
            BinOp::Sub => ("Subtract", "subtract"),
            BinOp::Mul => ("Multiply", "multiply"),
            BinOp::Div => ("Divide", "divide"),
            BinOp::Rem => ("Remainder", "remainder"),
            BinOp::Eq | BinOp::Ne => ("Equal", "equal"),
            BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => ("Ordered", "compare"),
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
