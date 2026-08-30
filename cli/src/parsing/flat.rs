//! The flattened parse tree: everything below the declaration level, stored in
//! append-only arrays and addressed by `u32`.
//!
//! An expression used to be a 64-byte enum reached through a `Box`, with a
//! `Vec` for every child list and a `String` for every identifier; a thousand
//! lines of ordinary Buri cost about six thousand `malloc`/`free` pairs to
//! parse and the allocator was more of the profile than the lexer and the
//! parser together. Here a node is twenty-four bytes appended to a `Vec`, a
//! child list is a range into another one, and an identifier is the source
//! under its own span — so the arenas are a fixed handful of allocations per
//! file whatever the file contains.
//!
//! # Three properties the rest of the front end is entitled to rely on
//!
//! * **Post-order.** A production records `nodes.len()` before it parses its
//!   children and pushes itself after, so every child's id is smaller than its
//!   parent's and a subtree is the contiguous range `[id + 1 - subtree, id]`.
//!   `subtree` counts descendants and self.
//! * **Spans are stored, never derived.** Ninety-seven diagnostic goldens and
//!   the whole of the formatter's comment placement are byte-pinned to spans
//!   that come out of expressions like `lhs.span().to(rhs.span())` and out of
//!   the grouping case, where `(e)` has the span of `e` rather than of the
//!   parentheses. Eight bytes a node is the price of making a mistake there
//!   impossible rather than merely unlikely.
//! * **No mutation after parsing.** `parser::Cache` hands the same
//!   `Rc<Module>` to every target that imports the file. The tree grows only
//!   while the parser holds it and is read-only from the moment `parse`
//!   returns, which is why nothing here needs an interior-mutability story and
//!   why ids rather than `&'arena` references are the only workable addressing.
//!
//! # Why the payload is `[u32; 4]`
//!
//! This repository contains no `unsafe` and that is worth keeping, so a
//! `union` is out. An enum payload would re-encode the discriminant `kind`
//! already holds and cost twenty-four bytes for the widest arm. Four words is
//! sixteen bytes, covers every variant without a second indirection — `Binary`
//! needs `lhs`, `rhs` and the operator's span; `StructLit` needs a head, a
//! spread and a field range — and every "which word is which" mistake lands in
//! the one exhaustive `match` that builds a view, where the test suite finds
//! it immediately.

use std::rc::Rc;

use crate::diagnostics::{FileId, Span};
use crate::parsing::tree::{BinOp, UnOp};

/// The absent optional id.
///
/// One sentinel rather than an `Option<u32>` per field, because `u32` has no
/// niche and the `Option` would double every payload word it appeared in.
/// [`Tree::opt`] is the only place that decodes it.
pub const NONE: u32 = u32::MAX;

macro_rules! id {
    ($(#[$m:meta])* $name:ident) => {
        $(#[$m])*
        #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
        #[repr(transparent)]
        pub struct $name(pub u32);

        impl $name {
            pub fn index(self) -> usize {
                self.0 as usize
            }
        }
    };
}

id!(
    /// An expression.
    ExprId
);
id!(
    /// A pattern.
    PatId
);
id!(BlockId);
id!(StmtId);
id!(ArmId);
id!(
    /// A type expression.
    TypeId
);
id!(CtxBodyId);

/// A span with the file taken out.
///
/// One `Module` is one file, so the `FileId` in every `Span` was the same
/// thirty-two bits repeated once per node. [`Tree::span`] puts it back.
#[derive(Clone, Copy, Default, Debug)]
pub struct Location {
    pub start: u32,
    pub end: u32,
}

impl Location {
    pub fn of(s: Span) -> Location {
        Location { start: s.start, end: s.end }
    }
}

/// What an expression node is.
///
/// The operator is part of the kind rather than of the payload: `BinOp`'s
/// seventeen values and `UnOp`'s three become twenty discriminants here, which
/// costs nothing — the tag was going to be read anyway — and saves a payload
/// word on the commonest interior node there is. `BinOp` and `UnOp` survive as
/// public enums, and [`ExprView`] hands one back, so the formatter's operator
/// table is untouched.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum Kind {
    Int,
    Float,
    Str,
    Char,
    True,
    False,
    Template,
    Ident,
    SelfValue,
    Ctx,
    DotVariant,
    Unit,
    Array,
    Tuple,
    Block,
    If,
    Match,
    ContextExpr,
    Lambda,
    // -- UnOp, one kind each ------------------------------------------------
    Neg,
    Not,
    BitNot,
    // -- BinOp, one kind each, in `BinOp`'s own order -----------------------
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
    // -- postfix and the rest -----------------------------------------------
    Field,
    TupleIndex,
    Call,
    Index,
    Try,
    Generic,
    StructLit,
    /// A region that did not parse. A leaf standing for the text the parser
    /// gave up on, so that what follows is told a mistake was here rather than
    /// handed a hole it cannot tell from an empty construct.
    Error,
}

impl Kind {
    pub fn of_binop(op: BinOp) -> Kind {
        match op {
            BinOp::Or => Kind::Or,
            BinOp::Coalesce => Kind::Coalesce,
            BinOp::And => Kind::And,
            BinOp::Eq => Kind::Eq,
            BinOp::Ne => Kind::Ne,
            BinOp::Lt => Kind::Lt,
            BinOp::Le => Kind::Le,
            BinOp::Gt => Kind::Gt,
            BinOp::Ge => Kind::Ge,
            BinOp::BitOr => Kind::BitOr,
            BinOp::BitXor => Kind::BitXor,
            BinOp::BitAnd => Kind::BitAnd,
            BinOp::Add => Kind::Add,
            BinOp::Sub => Kind::Sub,
            BinOp::Mul => Kind::Mul,
            BinOp::Div => Kind::Div,
            BinOp::Rem => Kind::Rem,
        }
    }

    fn binop(self) -> Option<BinOp> {
        Some(match self {
            Kind::Or => BinOp::Or,
            Kind::Coalesce => BinOp::Coalesce,
            Kind::And => BinOp::And,
            Kind::Eq => BinOp::Eq,
            Kind::Ne => BinOp::Ne,
            Kind::Lt => BinOp::Lt,
            Kind::Le => BinOp::Le,
            Kind::Gt => BinOp::Gt,
            Kind::Ge => BinOp::Ge,
            Kind::BitOr => BinOp::BitOr,
            Kind::BitXor => BinOp::BitXor,
            Kind::BitAnd => BinOp::BitAnd,
            Kind::Add => BinOp::Add,
            Kind::Sub => BinOp::Sub,
            Kind::Mul => BinOp::Mul,
            Kind::Div => BinOp::Div,
            Kind::Rem => BinOp::Rem,
            _ => return None,
        })
    }

    pub fn of_unop(op: UnOp) -> Kind {
        match op {
            UnOp::Neg => Kind::Neg,
            UnOp::Not => Kind::Not,
            UnOp::BitNot => Kind::BitNot,
        }
    }

    fn unop(self) -> Option<UnOp> {
        Some(match self {
            Kind::Neg => UnOp::Neg,
            Kind::Not => UnOp::Not,
            Kind::BitNot => UnOp::BitNot,
            _ => return None,
        })
    }

    /// Brace-terminated and self-delimiting: an operand of a binary operator,
    /// but never the head of a postfix chain (SPEC 12.11, 12.13).
    pub fn is_block_like(self) -> bool {
        matches!(self, Kind::Block | Kind::If | Kind::Match | Kind::ContextExpr)
    }
}

/// What a pattern node is.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum PatternKind {
    Wild,
    Bind,
    LitInt,
    LitFloat,
    LitStr,
    LitChar,
    LitTrue,
    LitFalse,
    Path,
    Unit,
    Tuple,
    Array,
    Or,
}

/// What a type expression is.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum TypeKind {
    Named,
    SelfType,
    Unit,
    Tuple,
    Array,
    Fn,
}

/// A range of type ids: a bound list, a tuple variant's payload, a `derive`'s
/// trait list. A declaration holds one of these where it used to hold a
/// `Vec<TypeExpr>`, and [`Tree::type_list`] hands back the slice.
#[derive(Clone, Copy, Default, Debug)]
pub struct TypeList {
    pub start: u32,
    pub len: u32,
}

impl TypeList {
    pub fn is_empty(self) -> bool {
        self.len == 0
    }

    pub fn len(self) -> usize {
        self.len as usize
    }
}

/// One type node.
///
/// A type has no `subtree` count: nothing skips a type subtree by arithmetic,
/// and a type is reached from a declaration field or from a child range rather
/// than by scanning the arena. It carries its own span instead, because unlike
/// an expression a type is not read span-first.
#[derive(Clone, Copy, Debug)]
pub struct TypeData {
    pub kind: TypeKind,
    pub payload: [u32; 4],
    pub span: Location,
}

/// One node of either arena.
///
/// The width is measured and pinned. A variant that needs a fifth payload word
/// is a compile error here rather than eight silent bytes per node everywhere,
/// which is the same discipline `tree.rs`'s assertion on `Item` keeps over the
/// declaration level.
#[derive(Clone, Copy, Debug)]
pub struct Node {
    pub kind: Kind,
    /// Descendants plus self, within this arena. A subtree is the contiguous
    /// range `[id + 1 - subtree, id]`, so skipping one is arithmetic.
    ///
    /// Exact on well-formed input. After a syntax error it is conservative:
    /// see [`Tree::rewind`], which the four `Bail` catch sites use to make it
    /// exact there too.
    pub subtree: u32,
    pub payload: [u32; 4],
}

#[derive(Clone, Copy, Debug)]
pub struct PNode {
    pub kind: PatternKind,
    pub subtree: u32,
    pub payload: [u32; 4],
}

const _: () = assert!(std::mem::size_of::<Node>() == 24);
const _: () = assert!(std::mem::size_of::<PNode>() == 24);
const _: () = assert!(std::mem::size_of::<TypeData>() == 28);
const _: () = assert!(std::mem::size_of::<TypeList>() == 8);
const _: () = assert!(std::mem::size_of::<Location>() == 8);

// ---------------------------------------------------------------------------
// Satellite records
// ---------------------------------------------------------------------------

/// A block: a range of statements and an optional tail expression.
#[derive(Clone, Copy, Debug)]
pub struct BlockData {
    pub stmts_start: u32,
    pub stmts_len: u32,
    pub tail: u32,
    pub span: Location,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum StmtKind {
    Let,
    Expr,
}

/// A statement. The owned `Stmt` is a hundred and seventy-six bytes — the
/// widest type in the front end — because it inlines a `Pattern` and an
/// `Expr`; this is twenty-four, and it is the largest single reduction the
/// front end has taken.
#[derive(Clone, Copy, Debug)]
pub struct StmtData {
    pub kind: StmtKind,
    /// The `let ctx = ...` form, which takes no pattern and no annotation.
    pub is_ctx: bool,
    pub pattern: u32,
    pub ty: u32,
    pub value: u32,
    pub span: Location,
}

#[derive(Clone, Copy, Debug)]
pub struct ArmData {
    pub pattern: u32,
    pub guard: u32,
    pub body: u32,
    pub span: Location,
}

/// A struct-literal field. `value` is [`NONE`] for the shorthand form.
#[derive(Clone, Copy, Debug)]
pub struct InitData {
    pub name: Location,
    pub value: u32,
    pub span: Location,
}

/// A struct-pattern field. `pattern` is [`NONE`] for the shorthand form.
#[derive(Clone, Copy, Debug)]
pub struct FieldPatData {
    pub name: Location,
    pub pattern: u32,
    pub span: Location,
}

#[derive(Clone, Copy, Debug)]
pub struct LambdaParamData {
    pub name: Location,
    pub ty: u32,
    pub span: Location,
}

/// One piece of a template literal: exactly one of the two is set.
#[derive(Clone, Copy, Debug)]
pub struct PartData {
    pub text: u32,
    pub hole: u32,
}

#[derive(Clone, Copy, Debug)]
pub struct CtxBodyData {
    pub spread: u32,
    pub bind_start: u32,
    pub bind_len: u32,
    pub span: Location,
}

#[derive(Clone, Copy, Debug)]
pub struct CtxBindData {
    pub effect: u32,
    pub value: u32,
    pub span: Location,
}

/// A variant pattern's payload: `Some(x)` or `User { id, .. }`.
#[derive(Clone, Copy, Debug)]
pub struct PatPayloadData {
    /// `false` is the tuple form, and the range is into `pkids`; `true` is the
    /// record form, and it is into `fpats`.
    pub record: bool,
    /// `..` at the end. Record form only.
    pub rest: bool,
    pub start: u32,
    pub len: u32,
}

const _: () = assert!(std::mem::size_of::<BlockData>() == 20);
const _: () = assert!(std::mem::size_of::<StmtData>() == 24);
const _: () = assert!(std::mem::size_of::<ArmData>() == 20);
const _: () = assert!(std::mem::size_of::<InitData>() == 20);
const _: () = assert!(std::mem::size_of::<FieldPatData>() == 20);
const _: () = assert!(std::mem::size_of::<LambdaParamData>() == 20);
const _: () = assert!(std::mem::size_of::<PartData>() == 8);
const _: () = assert!(std::mem::size_of::<CtxBodyData>() == 20);
const _: () = assert!(std::mem::size_of::<CtxBindData>() == 16);
const _: () = assert!(std::mem::size_of::<PatPayloadData>() == 12);

// ---------------------------------------------------------------------------
// The arenas
// ---------------------------------------------------------------------------

/// Everything below the declaration level of one file, plus the type
/// expressions and the source a declaration's own fields name.
#[derive(Clone, Debug)]
pub struct Tree {
    /// One file per `Module`, hoisted out of every span.
    file: FileId,
    /// The text the spans index.
    ///
    /// The tree owns it because an identifier's text is now `src[span]` and
    /// `parse(text: &str, file)` is called with temporaries at six sites whose
    /// signature may not change — so a borrow is out and an `Rc<str>` is one
    /// allocation and one memcpy of the file against the ~2 600 `String`s per
    /// thousand lines it deletes. `Rc` rather than `Box` so that the day the
    /// `SourceMap` learns to hand its text out shared, `parse` can take one
    /// and the copy goes away without any other change.
    src: Rc<str>,

    nodes: Vec<Node>,
    /// Parallel to `nodes`. Split out rather than folded in because span-only
    /// access is the commonest read in the whole consumer surface — thirty-two
    /// sites in `expressions.rs`, about thirty-one in `formatting.rs` — and
    /// eight bytes of an otherwise thirty-two-byte record is a fourfold
    /// bandwidth difference for those passes.
    spans: Vec<Location>,
    pnodes: Vec<PNode>,
    pspans: Vec<Location>,

    /// Every variadic expression child list, end to end, in source order.
    kids: Vec<ExprId>,
    pkids: Vec<PatId>,
    tkids: Vec<TypeId>,
    /// Every dotted path — a pattern's and a named type's — end to end. A
    /// name is the source under its own span, so a path is a range of spans
    /// and the segments themselves are not stored anywhere.
    names: Vec<Location>,

    blocks: Vec<BlockData>,
    stmts: Vec<StmtData>,
    arms: Vec<ArmData>,
    inits: Vec<InitData>,
    fpats: Vec<FieldPatData>,
    lparams: Vec<LambdaParamData>,
    parts: Vec<PartData>,
    ctxb: Vec<CtxBodyData>,
    ctxbind: Vec<CtxBindData>,
    ppay: Vec<PatPayloadData>,

    types: Vec<TypeData>,

    /// Wide or owned leaves, kept out of the payload. Each is a small
    /// fraction of tokens, so a side table costs an indirection on the rare
    /// path rather than eight bytes on every node.
    ints: Vec<u128>,
    floats: Vec<f64>,
    /// Cooked string and template text: the only owned text left in the tree.
    strs: Vec<String>,
}

impl Default for Tree {
    fn default() -> Tree {
        Tree::new(FileId(0), "")
    }
}

/// Every arena length at a point in time.
///
/// Declared field for field beside the arena list, and both are kept in step
/// by [`Tree::mark`] and [`Tree::rewind`] having to name every one: adding an
/// arena without adding a field here is a compile error, which is the only
/// mechanism that keeps a rollback from silently forgetting one.
#[derive(Clone, Copy, Debug)]
pub struct Mark {
    nodes: u32,
    spans: u32,
    pnodes: u32,
    pspans: u32,
    kids: u32,
    pkids: u32,
    tkids: u32,
    names: u32,
    blocks: u32,
    stmts: u32,
    arms: u32,
    inits: u32,
    fpats: u32,
    lparams: u32,
    parts: u32,
    ctxb: u32,
    ctxbind: u32,
    ppay: u32,
    types: u32,
    ints: u32,
    floats: u32,
    strs: u32,
}

impl Tree {
    /// An empty tree over `src`.
    ///
    /// The arenas are sized from the text, as `lexer.rs` already sizes its
    /// token buffer: a thousand lines of Buri is about seven thousand tokens
    /// and about two thousand expression nodes, so a byte count divided by
    /// sixteen is within a growth step of the truth and the arenas are then a
    /// fixed handful of allocations per file rather than a logarithmic number.
    pub fn new(file: FileId, src: &str) -> Tree {
        // One expression node per sixteen bytes of source, and the five other
        // arenas that are written on nearly every line sized off the same
        // figure. Measured against the generated corpora: a hundred thousand
        // lines of `mixed` lands within one growth step on all six. The rest
        // start empty on purpose — a file with no patterns should not pay for
        // a pattern arena.
        let n = src.len() / 16;
        Tree {
            file,
            src: Rc::from(src),
            nodes: Vec::with_capacity(n),
            spans: Vec::with_capacity(n),
            pnodes: Vec::with_capacity(n / 8),
            pspans: Vec::with_capacity(n / 8),
            kids: Vec::with_capacity(n / 4),
            pkids: Vec::new(),
            tkids: Vec::new(),
            names: Vec::with_capacity(n / 4),
            blocks: Vec::new(),
            stmts: Vec::with_capacity(n / 8),
            arms: Vec::new(),
            inits: Vec::new(),
            fpats: Vec::new(),
            lparams: Vec::new(),
            parts: Vec::new(),
            ctxb: Vec::new(),
            ctxbind: Vec::new(),
            ppay: Vec::new(),
            types: Vec::new(),
            ints: Vec::new(),
            floats: Vec::new(),
            strs: Vec::new(),
        }
    }

    // -- reading ------------------------------------------------------------

    pub fn file(&self) -> FileId {
        self.file
    }

    pub fn source(&self) -> &Rc<str> {
        &self.src
    }

    /// The text under a location. Empty if it does not describe one — every
    /// offset here came from the lexer, so in a correct front end that cannot
    /// happen, and a total accessor makes the one arrangement of bytes where
    /// it does a wrong answer rather than a panic.
    pub fn text(&self, at: Location) -> &str {
        self.src.get(at.start as usize..at.end as usize).unwrap_or("")
    }

    pub fn span_of(&self, at: Location) -> Span {
        Span { file: self.file, start: at.start, end: at.end }
    }

    pub fn span(&self, id: ExprId) -> Span {
        self.span_of(self.spans.get(id.index()).copied().unwrap_or_default())
    }

    pub fn pspan(&self, id: PatId) -> Span {
        self.span_of(self.pspans.get(id.index()).copied().unwrap_or_default())
    }

    pub fn kind(&self, id: ExprId) -> Kind {
        self.nodes.get(id.index()).map_or(Kind::Unit, |n| n.kind)
    }

    pub fn pkind(&self, id: PatId) -> PatternKind {
        self.pnodes.get(id.index()).map_or(PatternKind::Wild, |n| n.kind)
    }

    /// The optional-id decoding, in one place.
    pub fn opt(&self, x: u32) -> Option<ExprId> {
        if x == NONE {
            None
        } else {
            Some(ExprId(x))
        }
    }

    pub fn opt_pat(&self, x: u32) -> Option<PatId> {
        if x == NONE {
            None
        } else {
            Some(PatId(x))
        }
    }

    pub fn opt_type(&self, x: u32) -> Option<TypeId> {
        if x == NONE {
            None
        } else {
            Some(TypeId(x))
        }
    }

    /// The one place a type payload is decoded.
    pub fn ty(&self, id: TypeId) -> TypeView<'_> {
        let Some(n) = self.types.get(id.index()) else {
            return TypeView::Unit { span: self.span_of(Location::default()) };
        };
        let span = self.span_of(n.span);
        let p = n.payload;
        match n.kind {
            TypeKind::Named => TypeView::Named {
                path: self.slice(&self.names, p[0], p[1]),
                args: self.slice(&self.tkids, p[2], p[3]),
                span,
            },
            TypeKind::SelfType => TypeView::SelfType { span },
            TypeKind::Unit => TypeView::Unit { span },
            TypeKind::Tuple => TypeView::Tuple { elems: self.slice(&self.tkids, p[0], p[1]), span },
            TypeKind::Array => TypeView::Array { elem: TypeId(p[0]), span },
            TypeKind::Fn => TypeView::Fn {
                params: self.slice(&self.tkids, p[0], p[1]),
                ret: TypeId(p[2]),
                span,
            },
        }
    }

    pub fn type_span(&self, id: TypeId) -> Span {
        self.span_of(self.types.get(id.index()).map(|t| t.span).unwrap_or_default())
    }

    /// The trailing segment of a named type's path — the name itself.
    pub fn type_head(&self, id: TypeId) -> Option<&str> {
        match self.ty(id) {
            TypeView::Named { path, .. } => path.last().map(|s| self.text(*s)),
            _ => None,
        }
    }

    /// A named type's path as it was written, dots and all. For the diagnostic
    /// that quotes a type nobody declared.
    pub fn path_text(&self, path: &[Location]) -> String {
        let mut out = String::new();
        for (i, seg) in path.iter().enumerate() {
            if i > 0 {
                out.push('.');
            }
            out.push_str(self.text(*seg));
        }
        out
    }

    pub fn type_list(&self, l: TypeList) -> &[TypeId] {
        self.slice(&self.tkids, l.start, l.len)
    }

    /// The text a declaration's name was written with.
    pub fn name(&self, n: crate::parsing::tree::Name) -> &str {
        self.text(Location { start: n.span.start, end: n.span.end })
    }

    pub fn block(&self, id: BlockId) -> BlockData {
        self.blocks.get(id.index()).copied().unwrap_or(BlockData {
            stmts_start: 0,
            stmts_len: 0,
            tail: NONE,
            span: Location::default(),
        })
    }

    pub fn block_span(&self, id: BlockId) -> Span {
        self.span_of(self.block(id).span)
    }

    pub fn ctx_body(&self, id: CtxBodyId) -> CtxBodyData {
        self.ctxb.get(id.index()).copied().unwrap_or(CtxBodyData {
            spread: NONE,
            bind_start: 0,
            bind_len: 0,
            span: Location::default(),
        })
    }

    pub fn stmts_at(&self, start: u32, len: u32) -> &[StmtData] {
        self.slice(&self.stmts, start, len)
    }

    pub fn bindings_at(&self, start: u32, len: u32) -> &[CtxBindData] {
        self.slice(&self.ctxbind, start, len)
    }

    pub fn fpats_at(&self, start: u32, len: u32) -> &[FieldPatData] {
        self.slice(&self.fpats, start, len)
    }

    pub fn pkids_at(&self, start: u32, len: u32) -> &[PatId] {
        self.slice(&self.pkids, start, len)
    }

    pub fn payload(&self, at: u32) -> Option<PatPayloadData> {
        if at == NONE {
            None
        } else {
            self.ppay.get(at as usize).copied()
        }
    }

    /// One piece of a template literal. `PartData` holds two words and exactly
    /// one of them is set, so the discrimination belongs here rather than at
    /// every reader.
    pub fn part(&self, p: PartData) -> PartView<'_> {
        if p.hole == NONE {
            PartView::Text(self.strs.get(p.text as usize).map_or("", String::as_str))
        } else {
            PartView::Hole(ExprId(p.hole))
        }
    }

    /// Whether an expression is a type path: a name, the dot form, or either
    /// of those under type arguments or a qualifier (SPEC 14.1).
    ///
    /// Iterative rather than recursive because `Generic` and `Field` chain, and
    /// the flat tree makes walking a chain a loop rather than a call.
    pub fn is_type_path(&self, id: ExprId) -> bool {
        let mut at = id;
        loop {
            match self.expr(at) {
                ExprView::Ident { .. } | ExprView::DotVariant { .. } => return true,
                ExprView::Generic { base, .. } | ExprView::Field { base, .. } => at = base,
                _ => return false,
            }
        }
    }

    /// The expression under any number of type-argument layers. A method call
    /// is a `Field` whose base is a value, and `xs.fold<Int>(...)` puts a
    /// `Generic` between the call and the field.
    pub fn strip_type_args(&self, id: ExprId) -> ExprId {
        let mut at = id;
        while let ExprView::Generic { base, .. } = self.expr(at) {
            at = base;
        }
        at
    }

    /// A bounded slice of any arena.
    ///
    /// Total rather than an index expression for the same reason [`Tree::text`]
    /// is: a range in a payload was written by the builder three lines below
    /// the read, so out of bounds is unreachable, and a wrong answer beats a
    /// panic on the day it is not.
    fn slice<'t, T>(&'t self, v: &'t [T], start: u32, len: u32) -> &'t [T] {
        let a = start as usize;
        let b = a.saturating_add(len as usize);
        v.get(a..b).unwrap_or(&[])
    }

    /// The raw arenas.
    ///
    /// For the benchmark's structural check, which is what verifies the
    /// post-order and `subtree` invariants the module comment promises — no
    /// pass reads `subtree` yet, so without a checker the field would be
    /// believed rather than known until the first pass that depends on it.
    pub fn nodes(&self) -> &[Node] {
        &self.nodes
    }

    pub fn pat_nodes(&self) -> &[PNode] {
        &self.pnodes
    }

    /// Every type this file wrote down, in one arena. The language server asks
    /// which of them a cursor is inside, and a type is reached from whichever
    /// declaration named it rather than by scanning — so without this there is
    /// no way to ask the question of a whole file at once.
    pub fn type_nodes(&self) -> &[TypeData] {
        &self.types
    }

    /// How many nodes, blocks and so on this tree holds. For the benchmark and
    /// for reporting; nothing in the compiler reads it.
    pub fn counts(&self) -> [usize; 6] {
        [
            self.nodes.len(),
            self.pnodes.len(),
            self.kids.len(),
            self.stmts.len(),
            self.types.len(),
            self.strs.len(),
        ]
    }

    // -- the view façade ----------------------------------------------------

    /// The one place a payload is decoded.
    ///
    /// Every field of every variant is named here and nowhere else, so a
    /// builder that writes `lhs` where the reader expects `rhs` is a single
    /// wrong line in a single exhaustive `match` rather than a class of
    /// mistake spread across the parser.
    pub fn expr(&self, id: ExprId) -> ExprView<'_> {
        let Some(n) = self.nodes.get(id.index()) else {
            return ExprView::Unit { span: self.span(id) };
        };
        let span = self.span(id);
        let p = n.payload;
        if let Some(op) = n.kind.binop() {
            return ExprView::Binary {
                op,
                lhs: ExprId(p[0]),
                rhs: ExprId(p[1]),
                op_span: self.span_of(Location { start: p[2], end: p[3] }),
                span,
            };
        }
        if let Some(op) = n.kind.unop() {
            return ExprView::Unary { op, operand: ExprId(p[0]), span };
        }
        match n.kind {
            Kind::Int => ExprView::Int {
                value: self.ints.get(p[0] as usize).copied().unwrap_or(0),
                raw: self.text(Location { start: p[1], end: p[2] }),
                span,
            },
            Kind::Float => ExprView::Float {
                value: self.floats.get(p[0] as usize).copied().unwrap_or(0.0),
                raw: self.text(Location { start: p[1], end: p[2] }),
                span,
            },
            Kind::Str => ExprView::Str {
                value: self.strs.get(p[0] as usize).map_or("", String::as_str),
                span,
            },
            Kind::Char => {
                ExprView::Char { value: char::from_u32(p[0]).unwrap_or('\u{0}'), span }
            }
            Kind::True => ExprView::Bool { value: true, span },
            Kind::False => ExprView::Bool { value: false, span },
            Kind::Template => {
                ExprView::Template { parts: self.slice(&self.parts, p[0], p[1]), span }
            }
            Kind::Ident => {
                let at = Location { start: span.start, end: span.end };
                ExprView::Ident { name: self.text(at), span }
            }
            Kind::SelfValue => ExprView::SelfValue { span },
            Kind::Ctx => ExprView::Ctx { span },
            Kind::DotVariant => {
                let at = Location { start: p[0], end: p[1] };
                ExprView::DotVariant { name: self.text(at), name_span: self.span_of(at), span }
            }
            Kind::Unit => ExprView::Unit { span },
            Kind::Array => ExprView::Array { elems: self.slice(&self.kids, p[0], p[1]), span },
            Kind::Tuple => ExprView::Tuple { elems: self.slice(&self.kids, p[0], p[1]), span },
            Kind::Block => ExprView::Block { block: BlockId(p[0]), span },
            Kind::If => ExprView::If {
                cond: ExprId(p[0]),
                then: BlockId(p[1]),
                else_: ExprId(p[2]),
                span,
            },
            Kind::Match => ExprView::Match {
                scrutinee: ExprId(p[0]),
                arms: self.slice(&self.arms, p[1], p[2]),
                span,
            },
            Kind::ContextExpr => ExprView::ContextExpr { body: CtxBodyId(p[0]), span },
            Kind::Lambda => ExprView::Lambda {
                params: self.slice(&self.lparams, p[0], p[1]),
                ret: self.opt_type(p[2]),
                body: ExprId(p[3]),
                span,
            },
            Kind::Field => {
                let at = Location { start: p[1], end: p[2] };
                ExprView::Field {
                    base: ExprId(p[0]),
                    name: self.text(at),
                    name_span: self.span_of(at),
                    span,
                }
            }
            Kind::TupleIndex => ExprView::TupleIndex {
                base: ExprId(p[0]),
                index: p[1],
                index_span: self.span_of(Location { start: p[2], end: p[3] }),
                span,
            },
            Kind::Call => ExprView::Call {
                callee: ExprId(p[0]),
                args: self.slice(&self.kids, p[1], p[2]),
                span,
            },
            Kind::Index => ExprView::Index { base: ExprId(p[0]), index: ExprId(p[1]), span },
            Kind::Try => ExprView::Try { base: ExprId(p[0]), span },
            Kind::Generic => ExprView::Generic {
                base: ExprId(p[0]),
                args: self.slice(&self.tkids, p[1], p[2]),
                span,
            },
            Kind::StructLit => ExprView::StructLit {
                head: self.opt(p[0]),
                spread: self.opt(p[1]),
                fields: self.slice(&self.inits, p[2], p[3]),
                span,
            },
            Kind::Error => ExprView::Error { span },
            // Every operator kind was answered above.
            Kind::Neg
            | Kind::Not
            | Kind::BitNot
            | Kind::Or
            | Kind::Coalesce
            | Kind::And
            | Kind::Eq
            | Kind::Ne
            | Kind::Lt
            | Kind::Le
            | Kind::Gt
            | Kind::Ge
            | Kind::BitOr
            | Kind::BitXor
            | Kind::BitAnd
            | Kind::Add
            | Kind::Sub
            | Kind::Mul
            | Kind::Div
            | Kind::Rem => ExprView::Unit { span },
        }
    }

    pub fn pat(&self, id: PatId) -> PatView<'_> {
        let Some(n) = self.pnodes.get(id.index()) else {
            return PatView::Wild { span: self.pspan(id) };
        };
        let span = self.pspan(id);
        let p = n.payload;
        match n.kind {
            PatternKind::Wild => PatView::Wild { span },
            PatternKind::Bind => {
                let at = Location { start: p[0], end: p[1] };
                PatView::Bind {
                    name: self.text(at),
                    name_span: self.span_of(at),
                    sub: self.opt_pat(p[2]),
                    span,
                }
            }
            PatternKind::LitInt => PatView::LitInt {
                value: self.ints.get(p[0] as usize).copied().unwrap_or(0),
                negative: p[3] != 0,
                raw: self.text(Location { start: p[1], end: p[2] }),
                span,
            },
            PatternKind::LitFloat => PatView::LitFloat {
                value: self.floats.get(p[0] as usize).copied().unwrap_or(0.0),
                negative: p[3] != 0,
                raw: self.text(Location { start: p[1], end: p[2] }),
                span,
            },
            PatternKind::LitStr => {
                PatView::LitStr { value: self.strs.get(p[0] as usize).map_or("", String::as_str), span }
            }
            PatternKind::LitChar => {
                PatView::LitChar { value: char::from_u32(p[0]).unwrap_or('\u{0}'), span }
            }
            PatternKind::LitTrue => PatView::LitBool { value: true, span },
            PatternKind::LitFalse => PatView::LitBool { value: false, span },
            PatternKind::Path => PatView::Path {
                path: self.slice(&self.names, p[0], p[1]),
                dotted: p[3] != 0,
                payload: self.payload(p[2]),
                span,
            },
            PatternKind::Unit => PatView::Unit { span },
            PatternKind::Tuple => {
                PatView::Tuple { elems: self.slice(&self.pkids, p[0], p[1]), span }
            }
            PatternKind::Array => PatView::Array {
                elems: self.slice(&self.pkids, p[0], p[1]),
                // `Option<Option<Ident>>`: absent, present and anonymous, or
                // present and named.
                rest: match p[2] {
                    0 => None,
                    1 => Some(None),
                    _ => Some(self.names.get(p[3] as usize).copied()),
                },
                span,
            },
            PatternKind::Or => PatView::Or { alts: self.slice(&self.pkids, p[0], p[1]), span },
        }
    }

    // -- building -----------------------------------------------------------
    //
    // Append-only, and used by the parser and by nothing else. A production
    // records `len()` before its children and calls one of these after them,
    // which is the whole of the flattening transformation.

    /// The id the next expression node will get, which is what a production
    /// records before it parses its children.
    pub fn next_node(&self) -> u32 {
        self.nodes.len() as u32
    }

    pub fn next_pat(&self) -> u32 {
        self.pnodes.len() as u32
    }

    /// Where `id`'s subtree begins. A postfix chain reads it to give the node
    /// it is about to append the same start its base had, which is what makes
    /// `x.f().g()` one subtree rather than three overlapping ones.
    pub fn subtree_start(&self, id: ExprId) -> u32 {
        let s = self.nodes.get(id.index()).map_or(1, |n| n.subtree);
        id.0.saturating_add(1).saturating_sub(s)
    }

    pub fn push(&mut self, kind: Kind, payload: [u32; 4], span: Span, start: u32) -> ExprId {
        let id = self.nodes.len() as u32;
        let subtree = id.saturating_sub(start).saturating_add(1);
        self.nodes.push(Node { kind, subtree, payload });
        self.spans.push(Location::of(span));
        ExprId(id)
    }

    pub fn ppush(&mut self, kind: PatternKind, payload: [u32; 4], span: Span, start: u32) -> PatId {
        let id = self.pnodes.len() as u32;
        let subtree = id.saturating_sub(start).saturating_add(1);
        self.pnodes.push(PNode { kind, subtree, payload });
        self.pspans.push(Location::of(span));
        PatId(id)
    }

    pub fn push_kids(&mut self, ids: &[ExprId]) -> (u32, u32) {
        let at = self.kids.len() as u32;
        self.kids.extend_from_slice(ids);
        (at, ids.len() as u32)
    }

    pub fn push_pkids(&mut self, ids: &[PatId]) -> (u32, u32) {
        let at = self.pkids.len() as u32;
        self.pkids.extend_from_slice(ids);
        (at, ids.len() as u32)
    }

    pub fn push_names(&mut self, path: &[Location]) -> (u32, u32) {
        let at = self.names.len() as u32;
        self.names.extend_from_slice(path);
        (at, path.len() as u32)
    }

    pub fn push_name(&mut self, at: Location) -> u32 {
        let i = self.names.len() as u32;
        self.names.push(at);
        i
    }

    pub fn push_block(&mut self, d: BlockData) -> BlockId {
        self.blocks.push(d);
        BlockId(self.blocks.len().saturating_sub(1) as u32)
    }

    pub fn push_stmts(&mut self, ds: &[StmtData]) -> (u32, u32) {
        let at = self.stmts.len() as u32;
        self.stmts.extend_from_slice(ds);
        (at, ds.len() as u32)
    }

    pub fn push_arms(&mut self, ds: &[ArmData]) -> (u32, u32) {
        let at = self.arms.len() as u32;
        self.arms.extend_from_slice(ds);
        (at, ds.len() as u32)
    }

    pub fn push_inits(&mut self, ds: &[InitData]) -> (u32, u32) {
        let at = self.inits.len() as u32;
        self.inits.extend_from_slice(ds);
        (at, ds.len() as u32)
    }

    pub fn push_fpats(&mut self, ds: &[FieldPatData]) -> (u32, u32) {
        let at = self.fpats.len() as u32;
        self.fpats.extend_from_slice(ds);
        (at, ds.len() as u32)
    }

    pub fn push_lparams(&mut self, ds: &[LambdaParamData]) -> (u32, u32) {
        let at = self.lparams.len() as u32;
        self.lparams.extend_from_slice(ds);
        (at, ds.len() as u32)
    }

    pub fn push_parts(&mut self, ds: &[PartData]) -> (u32, u32) {
        let at = self.parts.len() as u32;
        self.parts.extend_from_slice(ds);
        (at, ds.len() as u32)
    }

    pub fn push_bindings(&mut self, ds: &[CtxBindData]) -> (u32, u32) {
        let at = self.ctxbind.len() as u32;
        self.ctxbind.extend_from_slice(ds);
        (at, ds.len() as u32)
    }

    pub fn push_ctx_body(&mut self, d: CtxBodyData) -> CtxBodyId {
        self.ctxb.push(d);
        CtxBodyId(self.ctxb.len().saturating_sub(1) as u32)
    }

    pub fn push_payload(&mut self, d: PatPayloadData) -> u32 {
        self.ppay.push(d);
        self.ppay.len().saturating_sub(1) as u32
    }

    pub fn push_tkids(&mut self, ids: &[TypeId]) -> TypeList {
        let at = self.tkids.len() as u32;
        self.tkids.extend_from_slice(ids);
        TypeList { start: at, len: ids.len() as u32 }
    }

    pub fn push_type(&mut self, kind: TypeKind, payload: [u32; 4], span: Span) -> TypeId {
        self.types.push(TypeData { kind, payload, span: Location::of(span) });
        TypeId(self.types.len().saturating_sub(1) as u32)
    }

    pub fn push_int(&mut self, v: u128) -> u32 {
        self.ints.push(v);
        self.ints.len().saturating_sub(1) as u32
    }

    pub fn push_float(&mut self, v: f64) -> u32 {
        self.floats.push(v);
        self.floats.len().saturating_sub(1) as u32
    }

    pub fn push_str(&mut self, v: String) -> u32 {
        self.strs.push(v);
        self.strs.len().saturating_sub(1) as u32
    }

    /// Every arena length, for a rollback.
    pub fn mark(&self) -> Mark {
        Mark {
            nodes: self.nodes.len() as u32,
            spans: self.spans.len() as u32,
            pnodes: self.pnodes.len() as u32,
            pspans: self.pspans.len() as u32,
            kids: self.kids.len() as u32,
            pkids: self.pkids.len() as u32,
            tkids: self.tkids.len() as u32,
            names: self.names.len() as u32,
            blocks: self.blocks.len() as u32,
            stmts: self.stmts.len() as u32,
            arms: self.arms.len() as u32,
            inits: self.inits.len() as u32,
            fpats: self.fpats.len() as u32,
            lparams: self.lparams.len() as u32,
            parts: self.parts.len() as u32,
            ctxb: self.ctxb.len() as u32,
            ctxbind: self.ctxbind.len() as u32,
            ppay: self.ppay.len() as u32,
            types: self.types.len() as u32,
            ints: self.ints.len() as u32,
            floats: self.floats.len() as u32,
            strs: self.strs.len() as u32,
        }
    }

    /// Drop everything appended since `m`.
    ///
    /// The four sites that catch `Bail` and carry on — `module`, `block_inner`
    /// twice, `match_expr`, and the two declaration bodies — leave nodes in
    /// the arenas that nothing references. They are harmless for correctness,
    /// but they sit inside the `subtree` count of whichever enclosing node
    /// recorded its start before them, so rewinding is what keeps "descendants
    /// and self" exact on a file with a syntax error in it. Nothing abandoned
    /// is reachable, so this is always safe where it is called.
    pub fn rewind(&mut self, m: Mark) {
        self.nodes.truncate(m.nodes as usize);
        self.spans.truncate(m.spans as usize);
        self.pnodes.truncate(m.pnodes as usize);
        self.pspans.truncate(m.pspans as usize);
        self.kids.truncate(m.kids as usize);
        self.pkids.truncate(m.pkids as usize);
        self.tkids.truncate(m.tkids as usize);
        self.names.truncate(m.names as usize);
        self.blocks.truncate(m.blocks as usize);
        self.stmts.truncate(m.stmts as usize);
        self.arms.truncate(m.arms as usize);
        self.inits.truncate(m.inits as usize);
        self.fpats.truncate(m.fpats as usize);
        self.lparams.truncate(m.lparams as usize);
        self.parts.truncate(m.parts as usize);
        self.ctxb.truncate(m.ctxb as usize);
        self.ctxbind.truncate(m.ctxbind as usize);
        self.ppay.truncate(m.ppay as usize);
        self.types.truncate(m.types as usize);
        self.ints.truncate(m.ints as usize);
        self.floats.truncate(m.floats as usize);
        self.strs.truncate(m.strs as usize);
    }
}

// ---------------------------------------------------------------------------
// Views
// ---------------------------------------------------------------------------

/// One expression node, decoded.
///
/// Every binding is `Copy`, so a consumer's `match` arm loses the `&` and `*`
/// noise it has today rather than gaining any. Constructing one is a jump
/// table on `kind` and three or four loads the pass was going to do anyway;
/// the phase this representation exists to speed up *builds* the tree and
/// never views it.
#[derive(Clone, Copy, Debug)]
pub enum ExprView<'t> {
    Int { value: u128, raw: &'t str, span: Span },
    Float { value: f64, raw: &'t str, span: Span },
    Str { value: &'t str, span: Span },
    Char { value: char, span: Span },
    Bool { value: bool, span: Span },
    Template { parts: &'t [PartData], span: Span },
    Ident { name: &'t str, span: Span },
    SelfValue { span: Span },
    Ctx { span: Span },
    DotVariant { name: &'t str, name_span: Span, span: Span },
    Unit { span: Span },
    Array { elems: &'t [ExprId], span: Span },
    Tuple { elems: &'t [ExprId], span: Span },
    Block { block: BlockId, span: Span },
    If { cond: ExprId, then: BlockId, else_: ExprId, span: Span },
    Match { scrutinee: ExprId, arms: &'t [ArmData], span: Span },
    ContextExpr { body: CtxBodyId, span: Span },
    Lambda { params: &'t [LambdaParamData], ret: Option<TypeId>, body: ExprId, span: Span },
    Unary { op: UnOp, operand: ExprId, span: Span },
    Binary { op: BinOp, lhs: ExprId, rhs: ExprId, op_span: Span, span: Span },
    Field { base: ExprId, name: &'t str, name_span: Span, span: Span },
    TupleIndex { base: ExprId, index: u32, index_span: Span, span: Span },
    Call { callee: ExprId, args: &'t [ExprId], span: Span },
    Index { base: ExprId, index: ExprId, span: Span },
    Try { base: ExprId, span: Span },
    Generic { base: ExprId, args: &'t [TypeId], span: Span },
    /// `head` is the type path of `World { ... }`, and `None` for the
    /// anonymous `{ ... }`, whose type comes from what it is checked against.
    StructLit {
        head: Option<ExprId>,
        spread: Option<ExprId>,
        fields: &'t [InitData],
        span: Span,
    },
    Error { span: Span },
}

/// One piece of a template literal, decoded.
#[derive(Clone, Copy, Debug)]
pub enum PartView<'t> {
    Text(&'t str),
    Hole(ExprId),
}

/// One type node, decoded.
#[derive(Clone, Copy, Debug)]
pub enum TypeView<'t> {
    Named { path: &'t [Location], args: &'t [TypeId], span: Span },
    SelfType { span: Span },
    Unit { span: Span },
    Tuple { elems: &'t [TypeId], span: Span },
    Array { elem: TypeId, span: Span },
    Fn { params: &'t [TypeId], ret: TypeId, span: Span },
}

impl TypeView<'_> {
    pub fn span(&self) -> Span {
        match *self {
            TypeView::Named { span, .. }
            | TypeView::SelfType { span }
            | TypeView::Unit { span }
            | TypeView::Tuple { span, .. }
            | TypeView::Array { span, .. }
            | TypeView::Fn { span, .. } => span,
        }
    }
}

/// One pattern node, decoded.
#[derive(Clone, Copy, Debug)]
pub enum PatView<'t> {
    Wild { span: Span },
    Bind { name: &'t str, name_span: Span, sub: Option<PatId>, span: Span },
    LitInt { value: u128, negative: bool, raw: &'t str, span: Span },
    LitFloat { value: f64, negative: bool, raw: &'t str, span: Span },
    LitStr { value: &'t str, span: Span },
    LitChar { value: char, span: Span },
    LitBool { value: bool, span: Span },
    Path { path: &'t [Location], dotted: bool, payload: Option<PatPayloadData>, span: Span },
    Unit { span: Span },
    Tuple { elems: &'t [PatId], span: Span },
    Array { elems: &'t [PatId], rest: Option<Option<Location>>, span: Span },
    Or { alts: &'t [PatId], span: Span },
}
