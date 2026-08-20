//! The parser.
//!
//! Recursive descent over the LR(1) grammar in `grammar.ebnf`. No production
//! consults name resolution or types, so this is one pass with no feedback and
//! files parse independently (SPEC 13.1).
//!
//! The three places the grammar earns its unambiguity, and where that shows up
//! here:
//!
//! * `unary` handles block-like expressions separately from postfix chains, so
//!   `match (x) { ... }.field` does not parse (SPEC 12.13).
//! * A `{` following a postfix expression is always a struct literal, and a
//!   bare `{` is always a block. Nothing competes, because there are no
//!   records (SPEC 12.3).
//! * `pattern_primary` decides binding-versus-variant on the token *after* an
//!   identifier, never on what the identifier means (SPEC 12.7).

use crate::diagnostics::{Diagnostic, FileId, Span};
use crate::parsing::flat::{
    ArmData, BlockData, BlockId, CtxBindData, CtxBodyData, CtxBodyId, ExprId, FieldPatData,
    InitData, Kind, LambdaParamData, Loc, PKind, PartData, PatId, PatPayloadData, StmtData,
    StmtKind, TKind, Tree, TypeId, TypeList, NONE,
};
use crate::parsing::lexer::{lex, Kw, Punct, TokKind, Tokens, Trivia};
use crate::parsing::tree::*;

pub struct Parsed {
    pub module: Module,
    pub errors: Vec<Diagnostic>,
}

/// Every file parsed so far in this process, by the [`FileId`] it was read as.
///
/// One command analyses one *target* at a time, and every target pulls in the
/// standard library and its own transitive imports — so a repository with a
/// hundred targets used to lex and parse `core/list` a hundred times, and
/// `buri lint //...` spent more than a quarter of its life re-reading files it
/// had already read. Parsing is a function of the text and nothing else, so
/// the second answer is the first one.
///
/// This is keyed on `FileId` rather than on a path or a hash of the contents
/// because [`SourceMap`](crate::diagnostics::SourceMap) already decides when a
/// file's text is re-read: it caches text under the same assumption, that
/// within one process a file is read once. Deriving the parse cache from the
/// same identity means the two cannot disagree about which revision of a file
/// is in play. A `SourceMap` that learns to reload a file has to hand out a
/// new `FileId` for the new text, and this follows for free.
///
/// The syntax tree is shared rather than copied — nothing mutates a tree after
/// parsing — and the diagnostics travel with it, because a cache that dropped
/// them would make a file's syntax errors appear only for whichever target
/// happened to reach it first.
#[derive(Default)]
pub struct Cache {
    entries: crate::hash::Map<FileId, (std::rc::Rc<Module>, std::rc::Rc<Vec<Diagnostic>>)>,
}

impl Cache {
    pub fn new() -> Cache {
        Cache::default()
    }

    /// The parse of `file`, computed once. `allow_bodyless` is a property of
    /// the file's role, which does not change between calls for one file.
    pub fn parse(
        &mut self,
        text: &str,
        file: FileId,
        allow_bodyless: bool,
    ) -> (std::rc::Rc<Module>, std::rc::Rc<Vec<Diagnostic>>) {
        if let Some(hit) = self.entries.get(&file) {
            return hit.clone();
        }
        let parsed = parse_with(text, file, allow_bodyless);
        let entry = (std::rc::Rc::new(parsed.module), std::rc::Rc::new(parsed.errors));
        self.entries.insert(file, entry.clone());
        entry
    }
}

/// Parse one source file.
pub fn parse(text: &str, file: FileId) -> Parsed {
    parse_with(text, file, false)
}

/// Parse an embedded standard library module, where a `fn` may be declared
/// without a body: the operations the backend supplies are declared for their
/// signatures and implemented in the runtime (see `compiler::standard_library`).
pub fn parse_stdlib(text: &str, file: FileId) -> Parsed {
    parse_with(text, file, true)
}

fn parse_with(text: &str, file: FileId, allow_bodyless: bool) -> Parsed {
    let lexed = lex(text, file);
    let first_item = lexed.tokens.span(0).start;
    let mut p = Parser {
        src: text,
        tree: Tree::new(file, text),
        scratch: Scratch::default(),
        last: lexed.tokens.len().saturating_sub(1),
        toks: lexed.tokens,
        trivia: lexed.trivia,
        pos: 0,
        reported: lexed.errors.iter().map(|e| (e.span.start, e.span.end)).collect(),
        errors: lexed.errors,
        allow_bodyless,
        depth: 0,
        trial: 0,
        chain: 0,
    };
    let mut module = p.module();
    // `//!` documents the module, so it belongs above everything. One that
    // appears later is almost always a mistyped `///`, and silently attaching
    // it to the module would publish a comment somebody wrote about a
    // declaration.
    for (line, span) in lexed.module_docs {
        if span.start <= first_item {
            module.docs.push(line);
        } else {
            p.errors.push(
                Diagnostic::error(span, "`//!` documents the module, so it must come first").with_code("module-doc-not-first")
                    .with_fix("move it above the first declaration, or write `///` to document the declaration below it"),
            );
        }
    }
    Parsed { module, errors: p.errors }
}

/// Whether a token can begin an expression.
///
/// The one caller is [`Parser::commits_to_type_args`], and what it needs to
/// know is whether the comparison reading of `… > HERE` has a right operand at
/// all. That is a question about the *first* token of `Expr`, so this is the
/// first set of `primary_expr` and the prefixes that reach it — kept beside
/// them rather than derived, because a new way to begin an expression is a new
/// arm there and a new line here, and the parser's own tests cover the pair.
fn starts_expr(t: TokKind) -> bool {
    match t {
        TokKind::Ident
        | TokKind::Int
        | TokKind::Float
        | TokKind::Str
        | TokKind::Char
        | TokKind::TemplateHead
        | TokKind::KwTrue
        | TokKind::KwFalse
        | TokKind::KwSelfValue
        | TokKind::KwCtx
        | TokKind::KwIf
        | TokKind::KwMatch
        | TokKind::KwContext
        | TokKind::KwFn
        // `.Variant`, an array, a tuple or grouping, a block, and the three
        // prefix operators.
        | TokKind::Dot
        | TokKind::LBracket
        | TokKind::LParen
        | TokKind::LBrace
        | TokKind::Minus
        | TokKind::Bang
        | TokKind::Tilde => true,
        TokKind::TemplateSpan
        | TokKind::TemplateTail
        | TokKind::Eof
        | TokKind::KwAs
        | TokKind::KwConst
        | TokKind::KwDerive
        | TokKind::KwEffect
        | TokKind::KwElse
        | TokKind::KwEnum
        | TokKind::KwExport
        | TokKind::KwFor
        | TokKind::KwFrom
        | TokKind::KwImpl
        | TokKind::KwImport
        | TokKind::KwLet
        | TokKind::KwSelfType
        | TokKind::KwStruct
        | TokKind::KwTest
        | TokKind::KwTrait
        | TokKind::KwType
        | TokKind::RBrace
        | TokKind::RParen
        | TokKind::RBracket
        | TokKind::Comma
        | TokKind::Semi
        | TokKind::Colon
        | TokKind::ColonColon
        | TokKind::DotDot
        | TokKind::At
        | TokKind::Underscore
        | TokKind::Eq
        | TokKind::FatArrow
        | TokKind::EqEq
        | TokKind::BangEq
        | TokKind::Lt
        | TokKind::LtEq
        | TokKind::Gt
        | TokKind::GtEq
        | TokKind::Plus
        | TokKind::Star
        | TokKind::Slash
        | TokKind::Percent
        | TokKind::AndAnd
        | TokKind::OrOr
        | TokKind::QuestionQuestion
        | TokKind::And
        | TokKind::Or
        | TokKind::Caret
        | TokKind::Question => false,
    }
}

/// The rung comparison sits on, which is the one rung that is neither left-
/// nor right-associative and so cannot be expressed as a binding power alone.
const CMP_LEVEL: usize = 4;

/// The rung `??` sits on, which is the one rung that builds its chain by
/// recursing and so spends [`Parser::chain_in`] rather than a loop counter.
const COALESCE_LEVEL: usize = 2;

/// Where a binary operator sits in the grammar's precedence order, as the
/// rung it is on and the `(left, right)` binding powers a precedence-climbing
/// loop compares against.
///
/// A rung is two binding powers apart from its neighbours, so associativity is
/// which of the pair is the larger: a left-associative rung is `(2n, 2n+1)`,
/// which stops its own operator from being re-consumed by the right-hand side,
/// and the one right-associative rung — `??` — is `(2n+1, 2n)`, which is what
/// makes `a ?? b ?? c` group to the right. Comparison is neither, and
/// [`Parser::binary_expr`] rejects a second one rather than grouping it
/// (SPEC 6.1).
///
/// This is the whole of the operator table. `starts_expr` and the postfix
/// chain are the other two places a new operator would have to be named.
fn binding_power(p: Punct) -> Option<(BinOp, u8, u8, usize)> {
    let (op, level) = match p {
        Punct::OrOr => (BinOp::Or, 1),
        Punct::QuestionQuestion => (BinOp::Coalesce, COALESCE_LEVEL),
        Punct::AndAnd => (BinOp::And, 3),
        Punct::EqEq => (BinOp::Eq, CMP_LEVEL),
        Punct::BangEq => (BinOp::Ne, CMP_LEVEL),
        Punct::Lt => (BinOp::Lt, CMP_LEVEL),
        Punct::LtEq => (BinOp::Le, CMP_LEVEL),
        Punct::Gt => (BinOp::Gt, CMP_LEVEL),
        Punct::GtEq => (BinOp::Ge, CMP_LEVEL),
        Punct::Or => (BinOp::BitOr, 5),
        Punct::Caret => (BinOp::BitXor, 6),
        Punct::And => (BinOp::BitAnd, 7),
        Punct::Plus => (BinOp::Add, 8),
        Punct::Minus => (BinOp::Sub, 8),
        Punct::Star => (BinOp::Mul, 9),
        Punct::Slash => (BinOp::Div, 9),
        Punct::Percent => (BinOp::Rem, 9),
        _ => return None,
    };
    let base = (level as u8).saturating_mul(2);
    let (lbp, rbp) = if level == COALESCE_LEVEL {
        (base.saturating_add(1), base)
    } else {
        (base, base.saturating_add(1))
    };
    Some((op, lbp, rbp, level))
}

/// Bail-out for error recovery: unwinds to the nearest item or statement.
struct Bail;
type PResult<T> = Result<T, Bail>;

/// Where a child list is accumulated before it is copied into the tree.
///
/// A variadic list — a call's arguments, a block's statements, a match's arms
/// — is contiguous in the tree, but its elements are parsed one at a time and
/// each of them may itself contain a list, so the elements cannot be appended
/// to the arena as they arrive. They go on a stack instead, and the enclosing
/// production copies its own tail of it into the arena and truncates.
///
/// One stack per kind, owned by the parser and reused for the whole file, so a
/// nested list costs a `push` rather than the `Vec` per child list the owned
/// tree allocated — about a thousand allocations per thousand lines.
#[derive(Default)]
struct Scratch {
    exprs: Vec<ExprId>,
    pats: Vec<PatId>,
    stmts: Vec<StmtData>,
    arms: Vec<ArmData>,
    inits: Vec<InitData>,
    fpats: Vec<FieldPatData>,
    lparams: Vec<LambdaParamData>,
    parts: Vec<PartData>,
    binds: Vec<CtxBindData>,
    names: Vec<Loc>,
    tys: Vec<TypeId>,
}

/// What a production pushed onto a scratch stack: everything from `base` on.
///
/// A total accessor rather than `&v[base..]`, because `base` is a length this
/// file recorded and the slice is therefore always in range — and because the
/// panic-free lint set in the workspace manifest is a promise about the
/// toolchain rather than a style rule, so "in range by construction" has to be
/// spelled rather than argued.
fn since<T>(v: &[T], base: usize) -> &[T] {
    v.get(base..).unwrap_or(&[])
}

/// Every arena length and every scratch depth at one point in time.
///
/// A production that fails leaves both partly written, and the six sites that
/// catch [`Bail`] and carry on — `module`, `block_inner` twice, `match_expr`,
/// `trait_decl` and `impl_decl` — restore both rather than stepping over the
/// wreckage. Nothing abandoned is reachable, so a rollback is always safe;
/// what it buys is that `subtree` stays exact on a file with a syntax error in
/// it, and that a half-built argument list cannot be adopted by whichever list
/// is still open outside it.
#[derive(Clone, Copy)]
struct Save {
    tree: crate::parsing::flat::Mark,
    exprs: usize,
    pats: usize,
    stmts: usize,
    arms: usize,
    inits: usize,
    fpats: usize,
    lparams: usize,
    parts: usize,
    binds: usize,
    names: usize,
    tys: usize,
}

struct Parser<'a> {
    /// The source the tokens were read from. A token records where it is, not
    /// what was written there, so this is where a literal's spelling and the
    /// text a message quotes come from.
    src: &'a str,
    /// The flat tree being built. Everything below the declaration level goes
    /// in here as it is parsed; a declaration holds the id of its body — see
    /// [`Parser::block`] and [`crate::parsing::flat`].
    tree: Tree,
    scratch: Scratch,
    toks: Tokens<'a>,
    /// What was written above a token, by token index, ascending — see
    /// [`Trivia`]. Only declarations read it, so it is searched rather than
    /// indexed.
    trivia: Vec<(u32, Trivia)>,
    /// The index of the `Eof` every stream ends with, so that clamping a read
    /// to the end of the stream is one comparison rather than a length
    /// lookup and a subtraction on every token access.
    last: usize,
    pos: usize,
    errors: Vec<Diagnostic>,
    /// The spans already reported, so that "one syntax error per location" is
    /// a lookup rather than a scan of everything reported so far. Scanning
    /// made a file that produces `n` errors cost `n²`: forty thousand of them
    /// — one bad token repeated, which is what a generated or truncated file
    /// looks like — took half a minute, almost all of it comparing spans.
    reported: crate::hash::Set<(u32, u32)>,
    allow_bodyless: bool,
    depth: u32,
    /// Non-zero while a speculative parse is running — see
    /// [`Parser::type_args_in_expr`]. A trial that fails must leave no trace,
    /// so while this is set nothing is reported and nothing is recorded as
    /// reported: the tokens it walked will be walked again, by whichever
    /// reading wins, and that reading is the one entitled to complain.
    trial: u32,
    /// Links in the chain currently being built. Only the constructs that
    /// recurse to build one — `else if`, and `??`, which is
    /// right-associative — keep a count here; a loop passes its own count to
    /// [`Parser::link`] instead, so there is nothing for it to leak.
    chain: u32,
}

/// How deep the grammar may nest a production inside itself: an expression
/// inside parentheses, a block inside a block, a type argument inside a type.
///
/// Each level costs several frames on the way down through the expression
/// grammar, and nesting this deep is a thing a person wrote rather than a
/// thing a generator emitted, so the budget is small.
const MAX_DEPTH: u32 = 256;

/// How long a chain the parser may build while looping.
///
/// `a + b + c`, `x.f().g()`, and `if … else if … else if …` are flat to a
/// reader and *nested* in the tree: one node per link. The parser builds them
/// without recursing, but every stage after it walks the tree by recursion, so
/// the length of the chain is the depth of a later stage's stack.
///
/// The budget is much larger than [`MAX_DEPTH`] because the code that reaches
/// it is generated rather than written: a protobuf decoder is one `else if`
/// per field, and a schema with a thousand fields is nobody's mistake. It is
/// still an order of magnitude under what the toolchain's stack — see `STACK`
/// in `main.rs` — has been measured to survive.
const MAX_CHAIN: u32 = 2048;

/// How many tokens past a `<` in expression position the parser will look for
/// the `>` that would close a type-argument list.
///
/// `f<A>(x)` and `(f < A) > (x)` are the same tokens, and the only way to tell
/// them apart is to look for the `>`. The bound is what keeps that from being
/// quadratic: a file of nothing but `<` costs this many token reads per `<`
/// rather than a scan to the end of the file, and a type-argument list longer
/// than this is not one anybody wrote — [`MAX_DEPTH`] already refuses the
/// nesting long before the length becomes reachable.
const MAX_TYPE_ARG_LOOKAHEAD: usize = 256;

impl<'a> Parser<'a> {
    // -- token access -------------------------------------------------------

    /// The index of the token at `i`, or of the last one.
    ///
    /// Clamping rather than returning an `Option` is what lets every `peek` in
    /// the parser below be unconditional: the stream always ends with `Eof`,
    /// which every production already has a case for, so running off the end
    /// reads as end-of-file rather than as a missing token. `lex` finishes by
    /// pushing that `Eof` on every path, which is what makes the stream
    /// non-empty and this the only place that has to know it.
    fn at(&self, i: usize) -> usize {
        i.min(self.last)
    }

    /// What the parser is standing on, as the byte the kind column holds.
    ///
    /// One load and one comparison per question asked of it, against a
    /// forty-eight-byte record's discriminant and payload — which is the
    /// reason the token buffer is columns. See [`crate::parsing::lexer::Tokens`].
    fn peek(&self) -> TokKind {
        self.toks.kind(self.at(self.pos))
    }

    fn kind_at(&self, i: usize) -> TokKind {
        self.toks.kind(self.at(i))
    }

    fn span(&self) -> Span {
        self.toks.span(self.at(self.pos))
    }

    fn prev_span(&self) -> Span {
        self.toks.span(self.at(self.pos.saturating_sub(1)))
    }

    /// The doc lines attached to the token about to be read, moved out of the
    /// trivia table.
    ///
    /// Moving rather than copying is safe because a token's documentation is
    /// read once: the production that reads it is the one that owns the
    /// declaration, and a speculative parse — see [`Parser::type_args_in_expr`]
    /// — never reaches a declaration.
    ///
    /// A binary search rather than an index because the table holds only the
    /// tokens that have something above them, which is a small fraction of the
    /// file — and this is called once per declaration, not once per token.
    fn docs(&mut self) -> Vec<String> {
        let at = self.pos as u32;
        let Ok(i) = self.trivia.binary_search_by_key(&at, |(a, _)| *a) else {
            return Vec::new();
        };
        match self.trivia.get_mut(i) {
            Some((_, t)) => std::mem::take(&mut t.docs),
            None => Vec::new(),
        }
    }

    /// The source under a span, empty if it does not describe one.
    ///
    /// Every offset here came from the lexer, so in a correct front end this
    /// is `&self.src[..]` — but that spelling panics on the one arrangement of
    /// bytes where it is wrong, and a total accessor makes that a short
    /// message rather than a crash.
    fn slice(&self, span: Span) -> &'a str {
        self.src.get(span.start as usize..span.end as usize).unwrap_or("")
    }

    /// What was written where the parser is standing.
    ///
    /// This is where a numeric literal's spelling comes from: `0xFF` and `255`
    /// are one value and two literals, and the token carries only the value.
    fn raw(&self) -> &'a str {
        self.toks.text(self.at(self.pos))
    }

    /// The same, as a message names it — see
    /// [`crate::parsing::lexer::Tok::describe`]. Every "expected …, found …"
    /// goes through this.
    fn found(&self) -> String {
        self.toks.describe(self.at(self.pos))
    }

    fn at_eof(&self) -> bool {
        self.peek() == TokKind::Eof
    }

    /// The value of the literal the parser is standing on.
    ///
    /// A literal's value is in a side table beside the token columns rather
    /// than in the token, because fewer than one token in ten has one and a
    /// column wide enough for a `u128` would have been paid for by all of
    /// them. Each of these is reached only from the arm that has already read
    /// the kind, so a token that is not a literal of that kind cannot get here.
    fn int_value(&self) -> u128 {
        self.toks.int(self.at(self.pos))
    }

    fn float_value(&self) -> f64 {
        self.toks.float(self.at(self.pos))
    }

    fn char_value(&self) -> u32 {
        self.toks.ch(self.at(self.pos)) as u32
    }

    /// Consume the current token and hand back its span.
    ///
    /// A `Span` rather than the `Token`, because a `Token` carries a `String`
    /// and two `Vec`s and every caller wants the span: returning the token
    /// deep-copied a hundred and twenty-eight bytes, and allocated, for every
    /// token the parser read.
    fn bump(&mut self) -> Span {
        let span = self.span();
        if self.pos < self.last {
            self.pos = self.pos.saturating_add(1);
        }
        span
    }

    /// The text a token carries — a string's contents, a template segment's —
    /// moved out of it when that is safe.
    ///
    /// An identifier is not one of these: its text is the source under its
    /// span, so it is borrowed rather than owned and there is nothing to move.
    /// See [`Parser::expect_name`].
    ///
    /// A failed trial rewinds `pos` and the tokens it walked are read again by
    /// whichever reading wins, so while `trial` is set the payload is copied
    /// instead. Outside a trial the parser owns the stream outright and no
    /// consumed token's text is read twice, so the copy the lexer already made
    /// is the one the tree keeps.
    fn take_text(&mut self) -> String {
        let pos = self.at(self.pos);
        if !matches!(
            self.toks.kind(pos),
            TokKind::Str | TokKind::TemplateHead | TokKind::TemplateSpan | TokKind::TemplateTail
        ) {
            return String::new();
        }
        if self.trial > 0 {
            self.toks.str_at(pos).to_string()
        } else {
            self.toks.take_str(pos)
        }
    }

    fn is(&self, p: Punct) -> bool {
        self.peek() == TokKind::of_punct(p)
    }

    fn is_kw(&self, k: Kw) -> bool {
        self.peek() == TokKind::of_kw(k)
    }

    fn eat(&mut self, p: Punct) -> bool {
        if self.is(p) {
            self.bump();
            true
        } else {
            false
        }
    }

    fn eat_kw(&mut self, k: Kw) -> bool {
        if self.is_kw(k) {
            self.bump();
            true
        } else {
            false
        }
    }

    /// Every parser error carries the edit that resolves it. `fix` is the
    /// third argument rather than something a caller may add later, so a new
    /// error site cannot forget one.
    /// Hands the diagnostic back so the caller can name the rule it enforces.
    /// Returns `None` when the error was suppressed as a duplicate.
    fn error(
        &mut self,
        span: Span,
        msg: impl Into<String>,
        fix: impl Into<String>,
    ) -> Option<&mut Diagnostic> {
        // One syntax error usually causes several; the first is the useful one.
        if self.trial > 0 || !self.reported.insert((span.start, span.end)) {
            return None;
        }
        self.errors.push(Diagnostic::error(span, msg).with_fix(fix));
        self.errors.last_mut()
    }

    /// A syntax error is always a mismatch: the grammar admits one thing here
    /// and the source has another.
    /// Returns `None` when the error was suppressed as a duplicate, so that a
    /// caller adding a note adds it to the diagnostic it just made or to
    /// nothing at all. Reaching for `errors.last_mut()` instead attached the
    /// note to whatever error happened to be last, which for a suppressed
    /// duplicate is a different error entirely.
    fn expected(
        &mut self,
        span: Span,
        want: impl std::fmt::Display,
        found: impl std::fmt::Display,
        fix: impl Into<String>,
    ) -> Option<&mut Diagnostic> {
        if self.trial > 0 || !self.reported.insert((span.start, span.end)) {
            return None;
        }
        self.errors.push(
            Diagnostic::error(span, format!("expected {want}, found {found}")).with_code("unexpected-token")
                .with_mismatch(want.to_string(), found.to_string())
                .with_fix(fix),
        );
        self.errors.last_mut()
    }

    fn expect(&mut self, p: Punct) -> PResult<Span> {
        if self.is(p) {
            Ok(self.bump())
        } else {
            let found = self.found();
            let span = self.span();
            let want = format!("`{}`", p.text());
            self.expected(span, &want, &found, format!("write {want} here"));
            Err(Bail)
        }
    }

    fn expect_kw(&mut self, k: Kw) -> PResult<Span> {
        if self.is_kw(k) {
            Ok(self.bump())
        } else {
            let found = self.found();
            let span = self.span();
            let want = format!("`{}`", k.text());
            self.expected(span, &want, &found, format!("write {want} here"));
            Err(Bail)
        }
    }

    /// An identifier, as the span it was written at.
    ///
    /// Inside the flat tree a name *is* its span: the text is `src[span]` for
    /// every identifier the parser has ever built, so storing it costs
    /// nothing and reading it allocates nothing. This is what deletes the
    /// largest single line of the allocation budget — one `String` per
    /// identifier token, about thirty-five percent of all tokens — without
    /// interning and without hashing.
    fn expect_name(&mut self) -> PResult<Span> {
        if self.peek() == TokKind::Ident {
            return Ok(self.bump());
        }
        let found = self.found();
        let span = self.span();
        self.expected(
            span,
            "an identifier",
            &found,
            "name it: a lowerCamelCase binding, or an UpperCamelCase type",
        );
        Err(Bail)
    }

    /// The same, as the [`Name`] a declaration holds.
    fn expect_ident(&mut self) -> PResult<Name> {
        Ok(Name::new(self.expect_name()?))
    }

    /// Every arena length and scratch depth, for a rollback.
    fn save(&self) -> Save {
        Save {
            tree: self.tree.mark(),
            exprs: self.scratch.exprs.len(),
            pats: self.scratch.pats.len(),
            stmts: self.scratch.stmts.len(),
            arms: self.scratch.arms.len(),
            inits: self.scratch.inits.len(),
            fpats: self.scratch.fpats.len(),
            lparams: self.scratch.lparams.len(),
            parts: self.scratch.parts.len(),
            binds: self.scratch.binds.len(),
            names: self.scratch.names.len(),
            tys: self.scratch.tys.len(),
        }
    }

    fn restore(&mut self, s: Save) {
        self.tree.rewind(s.tree);
        self.scratch.exprs.truncate(s.exprs);
        self.scratch.pats.truncate(s.pats);
        self.scratch.stmts.truncate(s.stmts);
        self.scratch.arms.truncate(s.arms);
        self.scratch.inits.truncate(s.inits);
        self.scratch.fpats.truncate(s.fpats);
        self.scratch.lparams.truncate(s.lparams);
        self.scratch.parts.truncate(s.parts);
        self.scratch.binds.truncate(s.binds);
        self.scratch.tys.truncate(s.tys);
        self.scratch.names.truncate(s.names);
    }

    fn expect_string(&mut self) -> PResult<(String, Span)> {
        if matches!(self.peek(), TokKind::Str) {
            let s = self.take_text();
            let span = self.bump();
            return Ok((s, span));
        }
        let found = self.found();
        let span = self.span();
        self.expected(span, "a string literal", &found, "quote it, as in `\"core/list\"`");
        Err(Bail)
    }

    fn enter(&mut self) -> PResult<()> {
        self.depth = self.depth.saturating_add(1);
        if self.depth > MAX_DEPTH {
            let span = self.span();
            self.error(
                span,
                "expression nests too deeply",
                "split it with `let` bindings; the limit exists so a pathological \
                 input cannot exhaust the parser's stack",
            );
            return Err(Bail);
        }
        Ok(())
    }

    fn leave(&mut self) {
        self.depth = self.depth.saturating_sub(1);
    }

    /// One more link in a chain being built without recursion, `links` being
    /// how many this loop has taken so far. See [`MAX_CHAIN`].
    fn link(&mut self, links: u32) -> PResult<()> {
        if self.chain.saturating_add(links) > MAX_CHAIN {
            let span = self.span();
            self.error(
                span,
                "this chain is too long",
                "split it with `let` bindings; the limit exists so that a pathological input \
                 cannot exhaust the stack of the passes that walk what this builds",
            );
            return Err(Bail);
        }
        Ok(())
    }

    /// The same budget, for the two chains that are built by recursing.
    fn chain_in(&mut self) -> PResult<()> {
        self.chain = self.chain.saturating_add(1);
        self.link(0)
    }

    fn chain_out(&mut self) {
        self.chain = self.chain.saturating_sub(1);
    }

    // -- recovery -----------------------------------------------------------

    /// Skip to the start of something that could begin a new item.
    fn sync_item(&mut self) {
        let mut depth = 0i32;
        loop {
            match self.peek() {
                TokKind::Eof => return,
                TokKind::LBrace => {
                    depth = depth.saturating_add(1);
                    self.bump();
                }
                TokKind::RBrace => {
                    depth = depth.saturating_sub(1);
                    self.bump();
                    if depth <= 0 {
                        return;
                    }
                }
                TokKind::Semi if depth <= 0 => {
                    self.bump();
                    return;
                }
                TokKind::KwFrom | TokKind::KwExport | TokKind::KwFn | TokKind::KwStruct | TokKind::KwEnum | TokKind::KwType | TokKind::KwConst | TokKind::KwTrait | TokKind::KwEffect | TokKind::KwImpl | TokKind::KwDerive | TokKind::KwTest if depth <= 0 => return,
                _ => {
                    self.bump();
                }
            }
        }
    }

    /// Skip to the end of the current statement.
    fn sync_stmt(&mut self) {
        let mut depth = 0i32;
        loop {
            match self.peek() {
                TokKind::Eof => return,
                TokKind::Semi if depth <= 0 => {
                    self.bump();
                    return;
                }
                TokKind::LBrace | TokKind::LParen | TokKind::LBracket => {
                    depth = depth.saturating_add(1);
                    self.bump();
                }
                TokKind::RBrace if depth <= 0 => return,
                TokKind::RBrace | TokKind::RParen | TokKind::RBracket => {
                    depth = depth.saturating_sub(1);
                    self.bump();
                }
                _ => {
                    self.bump();
                }
            }
        }
    }

    // -- module -------------------------------------------------------------

    fn module(&mut self) -> Module {
        let mut items = Vec::new();
        while !self.at_eof() {
            let before = self.pos;
            let save = self.save();
            match self.item() {
                Ok(Some(item)) => items.push(item),
                Ok(None) => {}
                Err(Bail) => {
                    self.restore(save);
                    self.sync_item();
                }
            }
            // Guarantee progress even if a sub-parser consumed nothing.
            if self.pos == before {
                self.bump();
            }
        }
        Module { items, docs: Vec::new(), tree: std::mem::take(&mut self.tree) }
    }

    fn item(&mut self) -> PResult<Option<Item>> {
        let docs = self.docs();
        let start = self.span();

        if self.is_kw(Kw::From) {
            return Ok(Some(self.import_or_reexport()?));
        }
        if self.is_kw(Kw::Impl) {
            return Ok(Some(Item::Impl(Box::new(self.impl_decl(docs)?))));
        }
        if self.is_kw(Kw::Derive) {
            return Ok(Some(Item::Derive(Box::new(self.derive_decl()?))));
        }
        if self.is_kw(Kw::Test) {
            return Ok(Some(Item::Test(Box::new(self.test_decl(docs)?))));
        }

        let exported = self.eat_kw(Kw::Export);
        let kw = self.peek().as_kw();
        let item = match kw {
            Some(Kw::Fn) => Item::Fn(Box::new(self.fn_decl(exported, docs, start)?)),
            Some(Kw::Struct) => Item::Struct(Box::new(self.struct_decl(exported, docs, start)?)),
            Some(Kw::Enum) => Item::Enum(Box::new(self.enum_decl(exported, docs, start)?)),
            Some(Kw::Type) => Item::TypeAlias(Box::new(self.type_alias(exported, docs, start)?)),
            Some(Kw::Const) => Item::Const(Box::new(self.const_decl(exported, docs, start)?)),
            Some(Kw::Trait) => Item::Trait(Box::new(self.trait_decl(exported, docs, start, false)?)),
            Some(Kw::Effect) => Item::Trait(Box::new(self.trait_decl(exported, docs, start, true)?)),
            Some(Kw::Context) => Item::Context(Box::new(self.context_decl(exported, docs, start)?)),
            // `from "..." export { ... }` after a stray `export`.
            Some(Kw::From) if exported => {
                let span = self.span();
                self.error(
                    span,
                    "write `from \"...\" export { ... }`, without a leading `export`",
                    "drop the leading `export`: the `export` after the path is the one that \
                     re-exports",
                );
                return Err(Bail);
            }
            _ => {
                let found = self.found();
                let span = self.span();
                self.expected(
                    span,
                    "a declaration",
                    &found,
                    "start it with one of `from` `export` `fn` `struct` `enum` `type` `const` \
                     `trait` `effect` `impl` `derive` `context` `test` — a module is a list of \
                     declarations, with no statements between them",
                );
                return Err(Bail);
            }
        };
        Ok(Some(item))
    }

    fn import_or_reexport(&mut self) -> PResult<Item> {
        let start = self.expect_kw(Kw::From)?;
        let (path, path_span) = self.expect_string()?;

        if self.eat_kw(Kw::Export) {
            self.expect(Punct::LBrace)?;
            let specs = self.import_specs(Punct::RBrace)?;
            self.expect(Punct::RBrace)?;
            let end = self.expect(Punct::Semi)?;
            return Ok(Item::ReExport(Box::new(ReExport { path, path_span, specs, span: start.to(end) })));
        }

        self.expect_kw(Kw::Import)?;
        let clause = if self.eat(Punct::Star) {
            // A namespace import must be named. Bare `import *` is not
            // derivable from the grammar, and the diagnostic says so.
            if !self.is_kw(Kw::As) {
                let span = self.prev_span();
                self.error(
                    span,
                    "a namespace import must be named",
                    "write `import * as list`, so every name it brings in is reached through \
                     one prefix",
                ).map(|d| {
                    d.code("unnamed-namespace-import").note(
                        "write `import * as name`; bare `import *` is not derivable from the \
                         grammar, so that no identifier enters a module's scope without \
                         appearing in that module's own source",
                    )
                });
                return Err(Bail);
            }
            self.bump();
            ImportClause::Namespace(self.expect_ident()?)
        } else {
            self.expect(Punct::LBrace)?;
            let specs = self.import_specs(Punct::RBrace)?;
            self.expect(Punct::RBrace)?;
            ImportClause::Named(specs)
        };
        let end = self.expect(Punct::Semi)?;
        Ok(Item::Import(Box::new(Import { path, path_span, clause, span: start.to(end) })))
    }

    fn import_specs(&mut self, close: Punct) -> PResult<Vec<ImportSpec>> {
        let mut specs = Vec::new();
        while !self.is(close) && !self.at_eof() {
            let name = self.expect_ident()?;
            let alias = if self.eat_kw(Kw::As) { Some(self.expect_ident()?) } else { None };
            let span = name.span.to(alias.as_ref().map(|a| a.span).unwrap_or(name.span));
            specs.push(ImportSpec { name, alias, span });
            if !self.eat(Punct::Comma) {
                break;
            }
        }
        Ok(specs)
    }

    // -- declarations -------------------------------------------------------

    fn generic_params(&mut self) -> PResult<Vec<GenericParam>> {
        if !self.is(Punct::Lt) {
            return Ok(Vec::new());
        }
        self.bump();
        let mut params = Vec::new();
        while !self.is(Punct::Gt) && !self.at_eof() {
            let name = self.expect_ident()?;
            let base = self.scratch.tys.len();
            if self.eat(Punct::Colon) {
                loop {
                    let b = self.named_type()?;
                    self.scratch.tys.push(b);
                    if !self.eat(Punct::Plus) {
                        break;
                    }
                }
            }
            let bounds = self.tree.push_tkids(since(&self.scratch.tys, base));
            self.scratch.tys.truncate(base);
            let span = name.span.to(self.prev_span());
            params.push(GenericParam { name, bounds, span });
            if !self.eat(Punct::Comma) {
                break;
            }
        }
        self.expect(Punct::Gt)?;
        Ok(params)
    }

    fn params(&mut self) -> PResult<Vec<Param>> {
        let mut params = Vec::new();
        let mut first = true;
        while !self.is(Punct::RParen) && !self.at_eof() {
            let start = self.span();
            let (kind, name) = if self.is_kw(Kw::SelfValue) {
                let span = self.bump();
                (ParamKind::SelfParam, Name::new(span))
            } else if self.is_kw(Kw::Ctx) {
                let span = self.bump();
                (ParamKind::CtxParam, Name::new(span))
            } else {
                (ParamKind::Normal, self.expect_ident()?)
            };
            if kind == ParamKind::SelfParam && !first {
                self.error(
                    name.span,
                    "`self` may appear only as a function's first parameter",
                    "move it to the front, or rename it if this parameter is not the receiver",
                ).map(|d| d.code("self-not-first"));
            }
            self.expect(Punct::Colon)?;
            let ty = self.ty()?;
            let span = start.to(self.tree.type_span(ty));
            params.push(Param { kind, name, ty, span });
            first = false;
            if !self.eat(Punct::Comma) {
                break;
            }
        }
        Ok(params)
    }

    fn fn_decl(&mut self, exported: bool, docs: Vec<String>, start: Span) -> PResult<FnDecl> {
        self.expect_kw(Kw::Fn)?;
        let name = self.expect_ident()?;
        let generics = self.generic_params()?;
        self.expect(Punct::LParen)?;
        let params = self.params()?;
        self.expect(Punct::RParen)?;
        // The return type annotation is required on every top-level `fn`
        // (SPEC 9), which is what keeps inference local to a body.
        self.expect(Punct::Colon)?;
        let ret = self.ty()?;

        let body = if self.is(Punct::LBrace) {
            Some(self.block()?)
        } else if self.is(Punct::Semi) {
            if !self.allow_bodyless {
                let span = self.span();
                let n = self.slice(name.span).to_string();
                self.error(
                    span,
                    format!("`{n}` has no body"),
                    "give it one: `{ ... }`. Only the bundled standard library declares a \
                     signature with no body, for an operation the runtime supplies",
                );
                if let Some(d) = self.errors.last_mut() {
                    d.note("a function declaration outside a trait or effect needs a block");
                }
            }
            self.bump();
            None
        } else {
            let found = self.found();
            let span = self.span();
            self.expected(span, "a function body", &found, "write `{ ... }` after the return type");
            return Err(Bail);
        };
        let span = start.to(self.prev_span());
        Ok(FnDecl { name, generics, params, ret, body, exported, span, docs })
    }

    /// `MethodSig` inside a trait or effect: no body, terminated with `;`.
    fn method_sig(&mut self) -> PResult<FnDecl> {
        let docs = self.docs();
        let start = self.span();
        self.expect_kw(Kw::Fn)?;
        let name = self.expect_ident()?;
        let generics = self.generic_params()?;
        self.expect(Punct::LParen)?;
        let params = self.params()?;
        self.expect(Punct::RParen)?;
        self.expect(Punct::Colon)?;
        let ret = self.ty()?;
        let end = self.expect(Punct::Semi)?;
        Ok(FnDecl {
            name,
            generics,
            params,
            ret,
            body: None,
            exported: false,
            span: start.to(end),
            docs,
        })
    }

    fn struct_decl(&mut self, exported: bool, docs: Vec<String>, start: Span) -> PResult<StructDecl> {
        self.expect_kw(Kw::Struct)?;
        let name = self.expect_ident()?;
        let generics = self.generic_params()?;

        let body = if self.eat(Punct::LParen) {
            // Tuple struct. Fields carry the same `export` marker.
            let mut fields = Vec::new();
            while !self.is(Punct::RParen) && !self.at_eof() {
                let fstart = self.span();
                let fexported = self.eat_kw(Kw::Export);
                let ty = self.ty()?;
                fields.push(TupleField { exported: fexported, ty, span: fstart.to(self.prev_span()) });
                if !self.eat(Punct::Comma) {
                    break;
                }
            }
            self.expect(Punct::RParen)?;
            // Tuple-struct declarations are terminated with `;`; record-struct
            // declarations are not.
            self.expect(Punct::Semi)?;
            StructBody::Tuple(fields)
        } else {
            self.expect(Punct::LBrace)?;
            let fields = self.field_decls(Punct::RBrace)?;
            self.expect(Punct::RBrace)?;
            StructBody::Record(fields)
        };
        let span = start.to(self.prev_span());
        Ok(StructDecl { name, generics, body, exported, span, docs })
    }

    fn field_decls(&mut self, close: Punct) -> PResult<Vec<FieldDecl>> {
        let mut fields = Vec::new();
        while !self.is(close) && !self.at_eof() {
            let docs = self.docs();
            let start = self.span();
            let exported = self.eat_kw(Kw::Export);
            let name = self.expect_ident()?;
            self.expect(Punct::Colon)?;
            let ty = self.ty()?;
            fields.push(FieldDecl { exported, name, ty, span: start.to(self.prev_span()), docs });
            if !self.eat(Punct::Comma) {
                break;
            }
        }
        Ok(fields)
    }

    fn enum_decl(&mut self, exported: bool, docs: Vec<String>, start: Span) -> PResult<EnumDecl> {
        self.expect_kw(Kw::Enum)?;
        let name = self.expect_ident()?;
        let generics = self.generic_params()?;
        self.expect(Punct::LBrace)?;
        let mut variants = Vec::new();
        while !self.is(Punct::RBrace) && !self.at_eof() {
            let vdocs = self.docs();
            let vstart = self.span();
            let vexported = self.eat_kw(Kw::Export);
            let vname = self.expect_ident()?;
            let payload = if self.eat(Punct::LParen) {
                let base = self.scratch.tys.len();
                while !self.is(Punct::RParen) && !self.at_eof() {
                    let t = self.ty()?;
                    self.scratch.tys.push(t);
                    if !self.eat(Punct::Comma) {
                        break;
                    }
                }
                self.expect(Punct::RParen)?;
                let tys = self.tree.push_tkids(since(&self.scratch.tys, base));
                self.scratch.tys.truncate(base);
                VariantPayload::Tuple(tys)
            } else if self.eat(Punct::LBrace) {
                let fields = self.field_decls(Punct::RBrace)?;
                self.expect(Punct::RBrace)?;
                VariantPayload::Record(fields)
            } else {
                VariantPayload::None
            };
            variants.push(Variant {
                exported: vexported,
                name: vname,
                payload,
                span: vstart.to(self.prev_span()),
                docs: vdocs,
            });
            if !self.eat(Punct::Comma) {
                break;
            }
        }
        self.expect(Punct::RBrace)?;
        let span = start.to(self.prev_span());
        Ok(EnumDecl { name, generics, variants, exported, span, docs })
    }

    fn type_alias(&mut self, exported: bool, docs: Vec<String>, start: Span) -> PResult<TypeAliasDecl> {
        self.expect_kw(Kw::Type)?;
        let name = self.expect_ident()?;
        let generics = self.generic_params()?;
        self.expect(Punct::Eq)?;
        let ty = self.ty()?;
        let end = self.expect(Punct::Semi)?;
        Ok(TypeAliasDecl { name, generics, ty, exported, span: start.to(end), docs })
    }

    fn const_decl(&mut self, exported: bool, docs: Vec<String>, start: Span) -> PResult<ConstDecl> {
        self.expect_kw(Kw::Const)?;
        let name = self.expect_ident()?;
        self.expect(Punct::Colon)?;
        let ty = self.ty()?;
        self.expect(Punct::Eq)?;
        let value = self.expr()?;
        let end = self.expect(Punct::Semi)?;
        Ok(ConstDecl { name, ty, value, exported, span: start.to(end), docs })
    }

    fn trait_decl(
        &mut self,
        exported: bool,
        docs: Vec<String>,
        start: Span,
        is_effect: bool,
    ) -> PResult<TraitDecl> {
        self.bump(); // `trait` or `effect`
        let name = self.expect_ident()?;
        let generics = self.generic_params()?;
        self.expect(Punct::LBrace)?;
        let mut methods = Vec::new();
        while !self.is(Punct::RBrace) && !self.at_eof() {
            let before = self.pos;
            let save = self.save();
            match self.method_sig() {
                Ok(m) => methods.push(m),
                Err(Bail) => {
                    self.restore(save);
                    self.sync_stmt();
                    if self.is(Punct::RBrace) || self.pos == before {
                        break;
                    }
                }
            }
        }
        self.expect(Punct::RBrace)?;
        let span = start.to(self.prev_span());
        Ok(TraitDecl { name, generics, methods, is_effect, exported, span, docs })
    }

    fn impl_decl(&mut self, docs: Vec<String>) -> PResult<ImplDecl> {
        let start = self.expect_kw(Kw::Impl)?;
        let generics = self.generic_params()?;
        // A full type either side: `[T]` has methods of its own, so the self
        // position is not restricted to a named type. The trait position is,
        // but the check that names the rule belongs with the other trait
        // resolution rather than here.
        let first = self.ty()?;
        // One token of lookahead: `for` makes this a conformance declaration,
        // and its absence makes it the type's own methods.
        let (trait_ty, self_ty) = if self.eat_kw(Kw::For) {
            (Some(first), self.ty()?)
        } else {
            (None, first)
        };
        self.expect(Punct::LBrace)?;
        let mut methods = Vec::new();
        let mut escaped = false;
        while !self.is(Punct::RBrace) && !self.at_eof() {
            let before = self.pos;
            let save = self.save();
            let docs = self.docs();
            let mstart = self.span();
            // A method of the type's own is exported on its own terms. A
            // method that satisfies a trait is not: conformance belongs to
            // the type, so it is visible wherever the type is.
            let exported = if self.is_kw(Kw::Export) {
                if trait_ty.is_some() {
                    let span = self.span();
                    self.error(
                    span,
                    "an `impl` method is not separately exported",
                    "drop the `export`",
                ).map(|d| {
                        d.code("impl-method-export").note(
                            "conformance is a property of the type, visible wherever the type is",
                        )
                    });
                }
                self.bump();
                trait_ty.is_none()
            } else {
                false
            };
            match self.fn_decl(exported, docs, mstart) {
                Ok(f) => methods.push(f),
                Err(Bail) => {
                    // `sync_stmt`, not `sync_item`: it stops *at* the closing
                    // brace without consuming it, which keeps recovery inside
                    // the body it was recovering in. `sync_item` consumed the
                    // brace, so whatever followed the `impl` was swallowed as
                    // a method — and when that was an item keyword it could
                    // not be, so the loop resynchronized to the same token
                    // forever. `impl V { ... }` followed by any declaration
                    // used to hang the compiler outright.
                    self.restore(save);
                    self.sync_stmt();
                    if self.is(Punct::RBrace) {
                        break;
                    }
                    if self.pos == before {
                        escaped = true;
                        break;
                    }
                }
            }
        }
        if escaped {
            // The body is already behind us, so consuming a `}` that is not
            // there would eat the next declaration. Report and hand the item
            // back: the `fn` after a mangled `impl` still deserves to be
            // parsed and type-checked.
            let span = self.span();
            self.error(
                span,
                "this `impl` body holds something that is not a method",
                "an `impl` holds `fn` declarations and nothing else",
            );
        } else {
            self.expect(Punct::RBrace)?;
        }
        Ok(ImplDecl { docs, generics, trait_ty, self_ty, methods, span: start.to(self.prev_span()) })
    }

    fn derive_decl(&mut self) -> PResult<DeriveDecl> {
        let start = self.expect_kw(Kw::Derive)?;
        // `derive for Meters;` reaches the type-name parser at `for`, which
        // reports "expected an identifier" and offers to name a binding. The
        // grammar is not what is confusing here: the clause is empty, and a
        // clause naming no traits would generate nothing.
        if self.is_kw(Kw::For) {
            let span = self.span();
            self.error(
                span,
                "a `derive` clause names no traits",
                "name the traits between `derive` and `for`, as in `derive Eq, Show for Meters;`",
            )
            .map(|d| {
                d.code("derive-without-traits").note(
                    "`derive` generates one implementation per trait it names, so a clause \
                     naming none would generate nothing; delete it, or name what the type \
                     should derive",
                )
            });
            return Err(Bail);
        }
        let base = self.scratch.tys.len();
        let first = self.named_type()?;
        self.scratch.tys.push(first);
        while self.eat(Punct::Comma) {
            let t = self.named_type()?;
            self.scratch.tys.push(t);
        }
        let traits = self.tree.push_tkids(since(&self.scratch.tys, base));
        self.scratch.tys.truncate(base);
        self.expect_kw(Kw::For)?;
        let self_ty = self.named_type()?;
        let end = self.expect(Punct::Semi)?;
        Ok(DeriveDecl { traits, self_ty, span: start.to(end) })
    }

    fn context_decl(&mut self, exported: bool, docs: Vec<String>, start: Span) -> PResult<ContextDecl> {
        self.expect_kw(Kw::Context)?;
        let name = self.expect_ident()?;
        let body = self.context_body()?;
        let span = start.to(self.prev_span());
        Ok(ContextDecl { name, body, exported, span, docs })
    }

    fn context_body(&mut self) -> PResult<CtxBodyId> {
        let start = self.expect(Punct::LBrace)?;
        let spread = if self.is(Punct::DotDot) {
            self.bump();
            let e = self.expr()?;
            self.eat(Punct::Comma);
            e.0
        } else {
            NONE
        };
        let base = self.scratch.binds.len();
        while !self.is(Punct::RBrace) && !self.at_eof() {
            let bstart = self.span();
            let effect = self.named_type()?;
            self.expect(Punct::Colon)?;
            let value = self.expr()?;
            let span = Loc::of(bstart.to(self.prev_span()));
            self.scratch.binds.push(CtxBindData { effect: effect.0, value: value.0, span });
            if !self.eat(Punct::Comma) {
                break;
            }
        }
        let end = self.expect(Punct::RBrace)?;
        let (bs, bl) = self.tree.push_bindings(since(&self.scratch.binds, base));
        self.scratch.binds.truncate(base);
        Ok(self.tree.push_ctx_body(CtxBodyData {
            spread,
            bind_start: bs,
            bind_len: bl,
            span: Loc::of(start.to(end)),
        }))
    }

    fn test_decl(&mut self, docs: Vec<String>) -> PResult<TestDecl> {
        let start = self.expect_kw(Kw::Test)?;
        let (name, name_span) = self.expect_string()?;
        let body = self.block()?;
        Ok(TestDecl { name, name_span, body, span: start.to(self.prev_span()), docs })
    }

    // -- types --------------------------------------------------------------

    fn named_type(&mut self) -> PResult<TypeId> {
        let start = self.span();
        // `Self` is a legal bound position spelling in an impl's type list.
        if self.is_kw(Kw::SelfType) {
            let span = self.bump();
            return Ok(self.tree.push_type(TKind::SelfType, [0; 4], span));
        }
        let nbase = self.scratch.names.len();
        let first = self.expect_name()?;
        self.scratch.names.push(Loc::of(first));
        while self.is(Punct::Dot) {
            self.bump();
            let seg = self.expect_name()?;
            self.scratch.names.push(Loc::of(seg));
        }
        let (ps, pl) = self.tree.push_names(since(&self.scratch.names, nbase));
        self.scratch.names.truncate(nbase);
        let args = self.type_args()?;
        let span = start.to(self.prev_span());
        Ok(self.tree.push_type(TKind::Named, [ps, pl, args.start, args.len], span))
    }

    /// A type-argument list, appended to the tree. No `<` is the empty list,
    /// which occupies nothing.
    fn type_args(&mut self) -> PResult<TypeList> {
        if !self.is(Punct::Lt) {
            return Ok(TypeList::default());
        }
        self.bump();
        let base = self.scratch.tys.len();
        while !self.is(Punct::Gt) && !self.at_eof() {
            let t = self.ty()?;
            self.scratch.tys.push(t);
            if !self.eat(Punct::Comma) {
                break;
            }
        }
        self.expect(Punct::Gt)?;
        let args = self.tree.push_tkids(since(&self.scratch.tys, base));
        self.scratch.tys.truncate(base);
        Ok(args)
    }

    /// Type arguments in *expression* position — `list.empty<Int>()` — or
    /// `None` when this `<` is the comparison operator it also spells.
    ///
    /// Buri rejects chained comparisons outright (`chained-comparison`), so
    /// `a < b > c` has no legal reading as comparison whatever this decides.
    /// That is what makes a decision possible at all: there is no program the
    /// two readings both accept and disagree about the meaning of. What is
    /// left is choosing the reading that produces the better error, in three
    /// steps, cheapest first:
    ///
    /// 1. a token scan for the `>` that would close the list, over the tokens
    ///    a type may be built from and no others. `a < b` has no `>` at all
    ///    and `x < y && y > z` has a `&&` in the way, so neither reaches the
    ///    parser below. The scan is bounded — see [`MAX_TYPE_ARG_LOOKAHEAD`] —
    ///    which is what keeps a `<`-per-token file linear;
    /// 2. the real type-argument parser, speculatively, with reporting off.
    ///    A scan is approximate and this is exact: `f() < g() > h()` gets past
    ///    the scan and fails here, at the `(` that no type may contain;
    /// 3. the token *after* the closing `>`, which is what settles the cases
    ///    both readings parse. See [`Parser::commits_to_type_args`].
    ///
    /// A trial that fails costs the tokens it walked and nothing else: the
    /// position is restored, and `trial` kept every diagnostic it would have
    /// reported from being reported.
    fn type_args_in_expr(&mut self) -> Option<TypeList> {
        if !self.scan_for_type_args() {
            return None;
        }
        let pos = self.pos;
        let depth = self.depth;
        // A trial appends type nodes. The reading that loses must leave none
        // of them behind, or every `a < b` in the file costs an arena entry.
        let save = self.save();
        self.trial = self.trial.saturating_add(1);
        let parsed = self.type_args();
        self.trial = self.trial.saturating_sub(1);
        // `ty` unwinds through `leave` on every path, so this is belt and
        // braces — and it is what lets a future production bail out of a trial
        // without leaving the budget spent.
        self.depth = depth;
        match parsed {
            Ok(args) if !args.is_empty() && self.commits_to_type_args() => Some(args),
            _ => {
                self.restore(save);
                self.pos = pos;
                None
            }
        }
    }

    /// Whether a `>` that closes a plausible type-argument list is followed by
    /// something that settles the reading.
    ///
    /// Both readings parse `a < b > (c)` and `a < b > c`. Neither is a legal
    /// program — the comparison reading of each is a chained comparison — so
    /// the choice is which diagnostic the writer of the mistake should get:
    ///
    /// * `(` and `{` are what type arguments are *for*: a call and a struct
    ///   literal. `a<b>(c)` is the new spelling of `a::<b>(c)` and reads as a
    ///   call to everyone who writes one, so it is one;
    /// * anything that cannot begin an expression — `;`, `,`, `)`, `+`, end of
    ///   file — leaves the comparison reading with no right operand, so type
    ///   arguments are the only reading that is a program at all. This is what
    ///   carries `let f = identity<Int>;`, the generic function reference;
    /// * anything else — `a < b > c` — keeps the comparison reading, so that
    ///   what comes back is `chained-comparison`, which names the mistake,
    ///   rather than a type error about `a` not being generic.
    fn commits_to_type_args(&self) -> bool {
        match self.peek() {
            TokKind::LParen | TokKind::LBrace => true,
            other => !starts_expr(other),
        }
    }

    /// A bounded look for the `>` that would close a type-argument list.
    ///
    /// Approximate on purpose: it tracks angle-bracket depth and refuses any
    /// token a type cannot be built from, which is enough to answer "is it
    /// worth parsing this as types" without parsing it. Parentheses and
    /// brackets are counted as *tokens*, not nesting, because `fn(A) => B` and
    /// `[T]` put them inside a type and a scan that balanced them would be the
    /// type parser again.
    fn scan_for_type_args(&self) -> bool {
        let mut depth = 0u32;
        for i in 0..MAX_TYPE_ARG_LOOKAHEAD {
            match self.kind_at(self.pos.saturating_add(i)) {
                TokKind::Lt => depth = depth.saturating_add(1),
                TokKind::Gt => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        return true;
                    }
                }
                TokKind::Ident
                | TokKind::KwSelfType
                | TokKind::KwFn
                | TokKind::Dot
                | TokKind::Comma
                | TokKind::LParen
                | TokKind::RParen
                | TokKind::LBracket
                | TokKind::RBracket
                | TokKind::FatArrow => {}
                _ => return false,
            }
        }
        false
    }

    fn ty(&mut self) -> PResult<TypeId> {
        self.enter()?;
        let r = self.ty_inner();
        self.leave();
        r
    }

    fn ty_inner(&mut self) -> PResult<TypeId> {
        let start = self.span();
        // Function types are written with `fn` for the same reason lambdas are:
        // it makes `(A, B)` unambiguously a tuple everywhere.
        if self.is_kw(Kw::Fn) {
            self.bump();
            self.expect(Punct::LParen)?;
            let base = self.scratch.tys.len();
            while !self.is(Punct::RParen) && !self.at_eof() {
                let t = self.ty()?;
                self.scratch.tys.push(t);
                if !self.eat(Punct::Comma) {
                    break;
                }
            }
            let params = self.tree.push_tkids(since(&self.scratch.tys, base));
            self.scratch.tys.truncate(base);
            self.expect(Punct::RParen)?;
            self.expect(Punct::FatArrow)?;
            let ret = self.ty()?;
            let span = start.to(self.prev_span());
            return Ok(self.tree.push_type(TKind::Fn, [params.start, params.len, ret.0, 0], span));
        }

        if self.is_kw(Kw::SelfType) {
            let span = self.bump();
            return Ok(self.tree.push_type(TKind::SelfType, [0; 4], span));
        }

        if self.is(Punct::LBracket) {
            self.bump();
            let elem = self.ty()?;
            self.expect(Punct::RBracket)?;
            let span = start.to(self.prev_span());
            return Ok(self.tree.push_type(TKind::Array, [elem.0, 0, 0, 0], span));
        }

        if self.is(Punct::LParen) {
            self.bump();
            // `()` is unit, `(T)` is grouping, `(T, U)` is a tuple.
            if self.is(Punct::RParen) {
                let end = self.bump();
                return Ok(self.tree.push_type(TKind::Unit, [0; 4], start.to(end)));
            }
            let first = self.ty()?;
            if self.is(Punct::RParen) {
                self.bump();
                return Ok(first);
            }
            let base = self.scratch.tys.len();
            self.scratch.tys.push(first);
            while self.eat(Punct::Comma) {
                if self.is(Punct::RParen) {
                    break;
                }
                let t = self.ty()?;
                self.scratch.tys.push(t);
            }
            self.expect(Punct::RParen)?;
            let n = self.scratch.tys.len().saturating_sub(base);
            let elems = self.tree.push_tkids(since(&self.scratch.tys, base));
            self.scratch.tys.truncate(base);
            if n < 2 {
                let span = start.to(self.prev_span());
                self.error(
                    span,
                    "a tuple type has arity 2 or more",
                    "write the element type on its own for one, or `()` for none",
                )
                .map(|d| d.note("`(T)` is a parenthesized type and `()` is unit"));
            }
            let span = start.to(self.prev_span());
            return Ok(self.tree.push_type(TKind::Tuple, [elems.start, elems.len, 0, 0], span));
        }

        match self.peek() {
            TokKind::Ident => self.named_type(),
            _ => {
                let found = self.found();
                let span = self.span();
                self.expected(span, "a type", &found, "name a type here, as in `Int` or `[Str]`");
                Err(Bail)
            }
        }
    }

    // -- blocks and statements ---------------------------------------------

    fn block(&mut self) -> PResult<BlockId> {
        self.enter()?;
        let r = self.block_inner();
        self.leave();
        r
    }

    fn block_inner(&mut self) -> PResult<BlockId> {
        let start = self.expect(Punct::LBrace)?;
        let base = self.scratch.stmts.len();
        let mut tail = NONE;

        while !self.is(Punct::RBrace) && !self.at_eof() {
            let before = self.pos;
            let save = self.save();
            if self.is_kw(Kw::Let) {
                match self.let_stmt() {
                    Ok(s) => self.scratch.stmts.push(s),
                    Err(Bail) => {
                        self.restore(save);
                        self.sync_stmt();
                    }
                }
            } else {
                let estart = self.span();
                match self.expr() {
                    Ok(e) => {
                        if self.is(Punct::Semi) {
                            let end = self.bump();
                            // An expression statement is legal only in a test
                            // source and only when its type is `()`; both are
                            // static rules, so the grammar admits it here.
                            self.scratch.stmts.push(StmtData {
                                kind: StmtKind::Expr,
                                is_ctx: false,
                                pattern: NONE,
                                ty: NONE,
                                value: e.0,
                                span: Loc::of(estart.to(end)),
                            });
                        } else {
                            tail = e.0;
                            break;
                        }
                    }
                    Err(Bail) => {
                        self.restore(save);
                        self.sync_stmt();
                    }
                }
            }
            if self.pos == before {
                self.bump();
            }
        }

        let end = self.expect(Punct::RBrace)?;
        let (ss, sl) = self.tree.push_stmts(since(&self.scratch.stmts, base));
        self.scratch.stmts.truncate(base);
        Ok(self.tree.push_block(BlockData {
            stmts_start: ss,
            stmts_len: sl,
            tail,
            span: Loc::of(start.to(end)),
        }))
    }

    fn let_stmt(&mut self) -> PResult<StmtData> {
        let start = self.expect_kw(Kw::Let)?;
        // After `let`, one token of lookahead decides which form this is: the
        // `ctx` keyword takes no pattern and no annotation, because a context's
        // type is generated and never written.
        if self.is_kw(Kw::Ctx) {
            let name_span = self.bump();
            self.expect(Punct::Eq)?;
            let value = self.expr()?;
            let end = self.expect(Punct::Semi)?;
            // The binding is spelled `ctx` at the keyword's own span, so the
            // source under the span *is* the name — as it is for every other
            // name in the flat tree.
            let at = self.tree.next_pat();
            let pattern =
                self.tree.ppush(PKind::Bind, [name_span.start, name_span.end, NONE, 0], name_span, at);
            return Ok(StmtData {
                kind: StmtKind::Let,
                is_ctx: true,
                pattern: pattern.0,
                ty: NONE,
                value: value.0,
                span: Loc::of(start.to(end)),
            });
        }
        let pattern = self.pattern()?;
        let ty = if self.eat(Punct::Colon) {
            self.ty()?.0
        } else {
            NONE
        };
        self.expect(Punct::Eq)?;
        let value = self.expr()?;
        let end = self.expect(Punct::Semi)?;
        Ok(StmtData {
            kind: StmtKind::Let,
            is_ctx: false,
            pattern: pattern.0,
            ty,
            value: value.0,
            span: Loc::of(start.to(end)),
        })
    }

    // -- expressions --------------------------------------------------------

    fn expr(&mut self) -> PResult<ExprId> {
        self.enter()?;
        let r = self.expr_inner();
        self.leave();
        r
    }

    fn expr_inner(&mut self) -> PResult<ExprId> {
        // A lambda is top-level-only: its body extends as far right as
        // possible, so allowing it as an operand would make
        // `2 * fn(x) => x + 1` ambiguous (SPEC 12.11).
        if self.is_kw(Kw::Fn) {
            return self.lambda();
        }
        self.binary_expr(0)
    }

    fn lambda(&mut self) -> PResult<ExprId> {
        let at = self.tree.next_node();
        let start = self.expect_kw(Kw::Fn)?;
        self.expect(Punct::LParen)?;
        let base = self.scratch.lparams.len();
        while !self.is(Punct::RParen) && !self.at_eof() {
            let name = self.expect_name()?;
            let ty = if self.eat(Punct::Colon) { self.ty()?.0 } else { NONE };
            let span = name.to(self.prev_span());
            self.scratch.lparams.push(LambdaParamData {
                name: Loc::of(name),
                ty,
                span: Loc::of(span),
            });
            if !self.eat(Punct::Comma) {
                break;
            }
        }
        self.expect(Punct::RParen)?;
        let ret = if self.eat(Punct::Colon) {
            self.ty()?.0
        } else {
            NONE
        };
        self.expect(Punct::FatArrow)?;
        let body = self.expr()?;
        let span = start.to(self.tree.span(body));
        let (ps, pl) = self.tree.push_lparams(since(&self.scratch.lparams, base));
        self.scratch.lparams.truncate(base);
        Ok(self.tree.push(Kind::Lambda, [ps, pl, ret, body.0], span, at))
    }

    /// Precedence climbing over the binary operators.
    ///
    /// `min_bp` is the loosest operator this call may consume; anything looser
    /// belongs to the caller's loop. See [`binding_power`] for the rungs and
    /// what the two halves of a binding power mean.
    ///
    /// One loop rather than the eight functions the ladder used to be. The
    /// functions held nothing but the order of the rungs, and charged every
    /// expression — `1` included — eight calls down and eight moves of a
    /// hundred-and-twenty-eight-byte `Expr` back up for it.
    fn binary_expr(&mut self, min_bp: u8) -> PResult<ExprId> {
        // Recorded before the left operand, so every node this call appends —
        // the whole left-leaning chain — has the same subtree start.
        let at = self.tree.next_node();
        let mut lhs = self.unary_expr()?;
        // The chain budget is counted per rung, as it was when each rung was
        // its own function: a run of `+` and a run of `||` in one expression
        // are two chains. Tracking the current rung is enough to do that with
        // one counter, because each right-hand side consumes everything that
        // binds tighter than the operator above it — so within one call the
        // rungs only ever loosen, and a rung never comes back.
        let mut links = 0u32;
        let mut rung = usize::MAX;
        loop {
            let Some(p) = self.peek().as_punct() else { return Ok(lhs) };
            let Some((op, lbp, rbp, level)) = binding_power(p) else { return Ok(lhs) };
            if lbp < min_bp {
                return Ok(lhs);
            }

            if level == CMP_LEVEL {
                let op_span = self.bump();
                let rhs = self.binary_expr(rbp)?;
                let span = self.tree.span(lhs).to(self.tree.span(rhs));
                lhs = self.tree.push(
                    Kind::of_binop(op),
                    [lhs.0, rhs.0, op_span.start, op_span.end],
                    span,
                    at,
                );
                // Comparison is non-associative: `a < b < c` is a parse error,
                // not a bug waiting to happen (SPEC 6.1).
                if self.at_cmp_op() {
                    let span = self.span();
                    self.error(
                        span,
                        "comparison operators are non-associative",
                        "write `a < b && b < c` rather than `a < b < c`",
                    )
                    .map(|d| {
                        d.code("chained-comparison")
                            .note("write `a < b && b < c` rather than `a < b < c`")
                    });
                    // Consume the rest of the chain and hand back what was
                    // parsed, so this one diagnostic is not followed by a
                    // cascade of type errors about the recovered shape.
                    // What the recovery parses is thrown away, so the arena is
                    // wound back over it: an abandoned operand nothing
                    // references would otherwise sit inside the `subtree`
                    // count of every node still open above it.
                    while self.at_cmp_op() {
                        self.bump();
                        let mark = self.tree.mark();
                        let _ = self.binary_expr(rbp)?;
                        self.tree.rewind(mark);
                    }
                }
                continue;
            }

            if level == COALESCE_LEVEL {
                // `??` is right-associative, so `a ?? b ?? c` works — and it
                // builds its chain by recursing, so it spends the budget
                // through `chain_in` rather than through a loop counter.
                let op_span = self.bump();
                self.chain_in()?;
                let rhs = self.binary_expr(rbp);
                self.chain_out();
                let rhs = rhs?;
                let span = self.tree.span(lhs).to(self.tree.span(rhs));
                lhs = self.tree.push(
                    Kind::of_binop(op),
                    [lhs.0, rhs.0, op_span.start, op_span.end],
                    span,
                    at,
                );
                continue;
            }

            if rung != level {
                rung = level;
                links = 0;
            }
            links = links.saturating_add(1);
            self.link(links)?;
            let op_span = self.bump();
            let rhs = self.binary_expr(rbp)?;
            let span = self.tree.span(lhs).to(self.tree.span(rhs));
            lhs = self.tree.push(
                Kind::of_binop(op),
                [lhs.0, rhs.0, op_span.start, op_span.end],
                span,
                at,
            );
        }
    }

    fn at_cmp_op(&self) -> bool {
        matches!(
            self.peek(),
            TokKind::EqEq | TokKind::BangEq | TokKind::Lt | TokKind::LtEq | TokKind::Gt | TokKind::GtEq
        )
    }

    /// A prefix operator's operand is another unary expression, so this is the
    /// second place in the grammar that recurses without passing through
    /// [`Parser::expr`] — `!!!!…true` and `----…1` are one production deep in
    /// the reader's eyes and a hundred thousand frames deep in the parser's.
    /// It costs one unit of the same budget the other four spend.
    fn unary_expr(&mut self) -> PResult<ExprId> {
        self.enter()?;
        let r = self.unary_inner();
        self.leave();
        r
    }

    fn unary_inner(&mut self) -> PResult<ExprId> {
        let at = self.tree.next_node();
        let op = match self.peek() {
            TokKind::Minus => Some(UnOp::Neg),
            TokKind::Bang => Some(UnOp::Not),
            TokKind::Tilde => Some(UnOp::BitNot),
            _ => None,
        };
        if let Some(op) = op {
            let start = self.bump();
            let operand = self.unary_expr()?;
            let span = start.to(self.tree.span(operand));
            return Ok(self.tree.push(Kind::of_unop(op), [operand.0, 0, 0, 0], span, at));
        }

        // Block-like expressions are operands but never postfix-chain heads,
        // which is what stops `if (c) { a } else { b } { x: 1 }` from having
        // two parses (SPEC 12.13). They are returned without entering
        // `postfix_ops`.
        if self.is(Punct::LBrace) {
            let b = self.block()?;
            let span = self.tree.span_of(self.tree.block(b).span);
            return Ok(self.tree.push(Kind::Block, [b.0, 0, 0, 0], span, at));
        }
        if self.is_kw(Kw::If) {
            return self.if_expr();
        }
        if self.is_kw(Kw::Match) {
            return self.match_expr();
        }
        if self.is_kw(Kw::Context) {
            let start = self.bump();
            let body = self.context_body()?;
            let span = start.to(self.tree.span_of(self.tree.ctx_body(body).span));
            return Ok(self.tree.push(Kind::ContextExpr, [body.0, 0, 0, 0], span, at));
        }

        let primary = self.primary_expr()?;
        self.postfix_ops(primary)
    }

    fn if_expr(&mut self) -> PResult<ExprId> {
        let at = self.tree.next_node();
        let start = self.expect_kw(Kw::If)?;
        // The condition is parenthesized, so the `{` that follows is always a
        // block (SPEC 12.1).
        self.expect(Punct::LParen)?;
        let cond = self.expr()?;
        self.expect(Punct::RParen)?;
        let then = self.block()?;
        // `else` is mandatory. There is nothing sensible for a missing branch
        // to produce in a language where `if` is an expression.
        if !self.is_kw(Kw::Else) {
            let span = self.tree.span_of(self.tree.block(then).span);
            self.error(
                span,
                "`if` requires an `else` branch",
                "add `else { ... }`; an `if` is an expression, so it has a value either way",
            )
            .map(|d| {
                d.code("if-without-else").note(
                    "`if` is an expression, so both branches must produce a value of the same type",
                )
            });
            return Err(Bail);
        }
        self.bump();
        let else_ = if self.is_kw(Kw::If) {
            // `else if` builds the chain by recursing, so it is the chain
            // budget it spends rather than the nesting one: a generated
            // decoder is one of these per field.
            self.chain_in()?;
            let nested = self.if_expr();
            self.chain_out();
            nested?
        } else {
            let bat = self.tree.next_node();
            let b = self.block()?;
            let span = self.tree.span_of(self.tree.block(b).span);
            self.tree.push(Kind::Block, [b.0, 0, 0, 0], span, bat)
        };
        let span = start.to(self.tree.span(else_));
        Ok(self.tree.push(Kind::If, [cond.0, then.0, else_.0, 0], span, at))
    }

    fn match_expr(&mut self) -> PResult<ExprId> {
        let at = self.tree.next_node();
        let start = self.expect_kw(Kw::Match)?;
        self.expect(Punct::LParen)?;
        let scrutinee = self.expr()?;
        self.expect(Punct::RParen)?;
        self.expect(Punct::LBrace)?;
        let base = self.scratch.arms.len();
        while !self.is(Punct::RBrace) && !self.at_eof() {
            let before = self.pos;
            let astart = self.span();
            let save = self.save();
            match self.match_arm(astart) {
                Ok(a) => self.scratch.arms.push(a),
                Err(Bail) => {
                    self.restore(save);
                    self.sync_match_arm();
                    if self.is(Punct::RBrace) {
                        break;
                    }
                }
            }
            // Arms are comma-separated, always — the comma is required even
            // after a brace-terminated body, because without it `A => x`
            // followed by `-1 =>` would greedily parse as `x - 1` (SPEC 12.12).
            if !self.eat(Punct::Comma) {
                break;
            }
            if self.pos == before {
                self.bump();
            }
        }
        let end = self.expect(Punct::RBrace)?;
        let (as_, al) = self.tree.push_arms(since(&self.scratch.arms, base));
        self.scratch.arms.truncate(base);
        Ok(self.tree.push(Kind::Match, [scrutinee.0, as_, al, 0], start.to(end), at))
    }

    /// One arm. A method rather than the closure it used to be, because the
    /// closure would have to borrow the parser mutably while the loop around
    /// it holds the arm stack.
    fn match_arm(&mut self, astart: Span) -> PResult<ArmData> {
        let pattern = self.pattern()?;
        let guard = if self.eat_kw(Kw::If) { self.expr()?.0 } else { NONE };
        self.expect(Punct::FatArrow)?;
        let body = self.expr()?;
        Ok(ArmData {
            pattern: pattern.0,
            guard,
            body: body.0,
            span: Loc::of(astart.to(self.prev_span())),
        })
    }

    fn sync_match_arm(&mut self) {
        let mut depth = 0i32;
        loop {
            match self.peek() {
                TokKind::Eof => return,
                TokKind::Comma if depth <= 0 => return,
                TokKind::RBrace if depth <= 0 => return,
                TokKind::LBrace | TokKind::LParen | TokKind::LBracket => {
                    depth = depth.saturating_add(1);
                    self.bump();
                }
                TokKind::RBrace | TokKind::RParen | TokKind::RBracket => {
                    depth = depth.saturating_sub(1);
                    self.bump();
                }
                _ => {
                    self.bump();
                }
            }
        }
    }

    fn primary_expr(&mut self) -> PResult<ExprId> {
        let at = self.tree.next_node();
        let start = self.span();
        match self.peek() {
            // A numeric literal's spelling is the source under the literal
            // token, which is *not* always the source under the node's span —
            // see `pattern_primary`, where `-1` spans the `-` as well. The two
            // are carried separately for that reason, here as well as there,
            // so that there is one rule rather than two.
            TokKind::Int => {
                let value = self.int_value();
                let span = self.bump();
                let ix = self.tree.push_int(value);
                Ok(self.tree.push(Kind::Int, [ix, span.start, span.end, 0], span, at))
            }
            TokKind::Float => {
                let value = self.float_value();
                let span = self.bump();
                let ix = self.tree.push_float(value);
                Ok(self.tree.push(Kind::Float, [ix, span.start, span.end, 0], span, at))
            }
            TokKind::Str => {
                let value = self.take_text();
                let span = self.bump();
                let ix = self.tree.push_str(value);
                Ok(self.tree.push(Kind::Str, [ix, 0, 0, 0], span, at))
            }
            TokKind::Char => {
                let value = self.char_value();
                let span = self.bump();
                Ok(self.tree.push(Kind::Char, [value, 0, 0, 0], span, at))
            }
            TokKind::KwTrue => {
                let span = self.bump();
                Ok(self.tree.push(Kind::True, [0; 4], span, at))
            }
            TokKind::KwFalse => {
                let span = self.bump();
                Ok(self.tree.push(Kind::False, [0; 4], span, at))
            }
            TokKind::TemplateHead => {
                let head = self.take_text();
                self.template(head, start)
            }
            TokKind::Ident => {
                let span = self.bump();
                Ok(self.tree.push(Kind::Ident, [0; 4], span, at))
            }
            TokKind::KwSelfValue => {
                let span = self.bump();
                Ok(self.tree.push(Kind::SelfValue, [0; 4], span, at))
            }
            TokKind::KwCtx => {
                let span = self.bump();
                Ok(self.tree.push(Kind::Ctx, [0; 4], span, at))
            }
            // `.Variant` — the inferred-type dot form.
            TokKind::Dot => {
                self.bump();
                let name = self.expect_name()?;
                let span = start.to(name);
                Ok(self.tree.push(Kind::DotVariant, [name.start, name.end, 0, 0], span, at))
            }
            TokKind::LBracket => {
                self.bump();
                let base = self.scratch.exprs.len();
                while !self.is(Punct::RBracket) && !self.at_eof() {
                    let e = self.expr()?;
                    self.scratch.exprs.push(e);
                    if !self.eat(Punct::Comma) {
                        break;
                    }
                }
                let end = self.expect(Punct::RBracket)?;
                let (ks, kl) = self.tree.push_kids(since(&self.scratch.exprs, base));
                self.scratch.exprs.truncate(base);
                Ok(self.tree.push(Kind::Array, [ks, kl, 0, 0], start.to(end), at))
            }
            TokKind::LParen => {
                self.bump();
                if self.is(Punct::RParen) {
                    let end = self.bump();
                    return Ok(self.tree.push(Kind::Unit, [0; 4], start.to(end), at));
                }
                let first = self.expr()?;
                // Grouping hands back the inner expression, so `(e)` keeps
                // `e`'s span and not the parentheses'. Several golden
                // diagnostics are pinned to that.
                if self.is(Punct::RParen) {
                    self.bump();
                    return Ok(first);
                }
                let base = self.scratch.exprs.len();
                self.scratch.exprs.push(first);
                while self.eat(Punct::Comma) {
                    if self.is(Punct::RParen) {
                        break;
                    }
                    let e = self.expr()?;
                    self.scratch.exprs.push(e);
                }
                let end = self.expect(Punct::RParen)?;
                if self.scratch.exprs.len().saturating_sub(base) < 2 {
                    self.error(
                        start.to(end),
                        "a tuple has arity 2 or more",
                        "drop the parentheses for one element, or write `()` for none",
                    )
                    .map(|d| d.note("`(e)` is grouping and `()` is the unit value"));
                }
                let (ks, kl) = self.tree.push_kids(since(&self.scratch.exprs, base));
                self.scratch.exprs.truncate(base);
                Ok(self.tree.push(Kind::Tuple, [ks, kl, 0, 0], start.to(end), at))
            }
            _ => {
                let found = self.found();
                let span = self.span();
                self.expected(span, "an expression", &found, "write a value here");
                Err(Bail)
            }
        }
    }

    fn template(&mut self, head: String, start: Span) -> PResult<ExprId> {
        let at = self.tree.next_node();
        self.bump();
        let base = self.scratch.parts.len();
        if !head.is_empty() {
            let ix = self.tree.push_str(head);
            self.scratch.parts.push(PartData { text: ix, hole: NONE });
        }
        loop {
            let hole = self.expr()?;
            self.scratch.parts.push(PartData { text: NONE, hole: hole.0 });
            match self.peek() {
                TokKind::TemplateSpan => {
                    let text = self.take_text();
                    self.bump();
                    if !text.is_empty() {
                        let ix = self.tree.push_str(text);
                        self.scratch.parts.push(PartData { text: ix, hole: NONE });
                    }
                }
                TokKind::TemplateTail => {
                    let text = self.take_text();
                    let end = self.bump();
                    if !text.is_empty() {
                        let ix = self.tree.push_str(text);
                        self.scratch.parts.push(PartData { text: ix, hole: NONE });
                    }
                    let (ps, pl) = self.tree.push_parts(since(&self.scratch.parts, base));
                    self.scratch.parts.truncate(base);
                    return Ok(self.tree.push(
                        Kind::Template,
                        [ps, pl, 0, 0],
                        start.to(end),
                        at,
                    ));
                }
                _ => {
                    let found = self.found();
                    let span = self.span();
                    self.expected(
                        span,
                        "the rest of the string",
                        &found,
                        "close the template: every `${` needs a `}` and the string needs a \
                         closing quote",
                    );
                    return Err(Bail);
                }
            }
        }
    }

    fn postfix_ops(&mut self, mut base: ExprId) -> PResult<ExprId> {
        // Every link of the chain replaces `base` with a node whose subtree
        // covers everything from here, so the start is read once from the head
        // rather than threaded down from `unary_inner`.
        let at = self.tree.subtree_start(base);
        let mut links = 0u32;
        loop {
            links = links.saturating_add(1);
            self.link(links)?;
            let start = self.tree.span(base);
            match self.peek() {
                TokKind::Dot => {
                    self.bump();
                    match self.peek() {
                        // Tuple element access. `t.0.1` lexes as `t` `.` `0.1`,
                        // a known wart; write `(t.0).1`.
                        TokKind::Int => {
                            let value = self.int_value();
                            if value > u32::MAX as u128 {
                                let raw = self.raw();
                                let span = self.span();
                                self.error(
                                span,
                                format!("`{raw}` is not a tuple index"),
                                "a tuple index is a plain decimal number, as in `pair.0`",
                            );
                            }
                            let index_span = self.bump();
                            base = self.tree.push(
                                Kind::TupleIndex,
                                [base.0, value as u32, index_span.start, index_span.end],
                                start.to(index_span),
                                at,
                            );
                        }
                        TokKind::Float => {
                            let raw = self.raw();
                            let span = self.span();
                            self.error(
                                span,
                                format!("`.{raw}` lexes as a float, not two tuple indices"),
                                "parenthesize the first index: `(t.0).1`",
                            );
                            return Err(Bail);
                        }
                        _ => {
                            let name = self.expect_name()?;
                            let span = start.to(name);
                            base = self.tree.push(
                                Kind::Field,
                                [base.0, name.start, name.end, 0],
                                span,
                                at,
                            );
                        }
                    }
                }
                TokKind::LParen => {
                    self.bump();
                    let abase = self.scratch.exprs.len();
                    while !self.is(Punct::RParen) && !self.at_eof() {
                        let e = self.expr()?;
                        self.scratch.exprs.push(e);
                        if !self.eat(Punct::Comma) {
                            break;
                        }
                    }
                    let end = self.expect(Punct::RParen)?;
                    let (ks, kl) = self.tree.push_kids(since(&self.scratch.exprs, abase));
                    self.scratch.exprs.truncate(abase);
                    base =
                        self.tree.push(Kind::Call, [base.0, ks, kl, 0], start.to(end), at);
                }
                TokKind::LBracket => {
                    self.bump();
                    let index = self.expr()?;
                    let end = self.expect(Punct::RBracket)?;
                    base = self.tree.push(
                        Kind::Index,
                        [base.0, index.0, 0, 0],
                        start.to(end),
                        at,
                    );
                }
                TokKind::Question => {
                    let end = self.bump();
                    base = self.tree.push(Kind::Try, [base.0, 0, 0, 0], start.to(end), at);
                }
                // Type arguments, or the comparison operator that is spelled
                // the same. `type_args_in_expr` decides and rewinds if it is
                // the latter, leaving the `<` for the binary parser above.
                TokKind::Lt => match self.type_args_in_expr() {
                    Some(args) => {
                        base = self.tree.push(
                            Kind::Generic,
                            [base.0, args.start, args.len, 0],
                            start.to(self.prev_span()),
                            at,
                        );
                    }
                    None => return Ok(base),
                },
                // The turbofish, which this language had until it did not.
                // Reported rather than accepted, because two spellings of one
                // thing is the thing itself plus a decision nobody wants to
                // make — and reported *here*, with the `::` under the caret
                // and an edit that deletes it, so that `lint --fix` and an
                // editor's quick fix migrate a file that still has it.
                //
                // Parsing continues as though the `::` had not been written:
                // one error names the whole mistake, and everything after it
                // in the file is read for what it says rather than through a
                // recovery.
                TokKind::ColonColon => {
                    let colons = self.bump();
                    if !self.is(Punct::Lt) {
                        // `::` is not an operator at all any more.
                        self.error(
                            colons,
                            "`::` is not an operator",
                            "a module's members are reached with `.`, as in `list.empty`",
                        );
                        return Err(Bail);
                    }
                    self.error(
                        colons,
                        "type arguments in an expression are written without `::`",
                        "remove the `::`, as in `list.empty<Int>()`",
                    )
                    .map(|d| {
                        d.code("turbofish").edit(colons, "").note(
                            "`::` was needed when `<` in expression position was always a \
                             comparison; it no longer is",
                        )
                    });
                    let args = self.type_args()?;
                    base = self.tree.push(
                        Kind::Generic,
                        [base.0, args.start, args.len, 0],
                        start.to(self.prev_span()),
                        at,
                    );
                }
                // With records gone, a `{` following a path is always a struct
                // literal. Nothing competes, so field shorthand is unambiguous.
                TokKind::LBrace => {
                    self.bump();
                    let spread = if self.is(Punct::DotDot) {
                        self.bump();
                        let e = self.expr()?;
                        self.eat(Punct::Comma);
                        e.0
                    } else {
                        NONE
                    };
                    let fbase = self.scratch.inits.len();
                    while !self.is(Punct::RBrace) && !self.at_eof() {
                        let fname = self.expect_name()?;
                        let value =
                            if self.eat(Punct::Colon) { self.expr()?.0 } else { NONE };
                        let fspan = fname.to(self.prev_span());
                        self.scratch.inits.push(InitData {
                            name: Loc::of(fname),
                            value,
                            span: Loc::of(fspan),
                        });
                        if !self.eat(Punct::Comma) {
                            break;
                        }
                    }
                    let end = self.expect(Punct::RBrace)?;
                    let (is, il) = self.tree.push_inits(since(&self.scratch.inits, fbase));
                    self.scratch.inits.truncate(fbase);
                    base = self.tree.push(
                        Kind::StructLit,
                        [base.0, spread, is, il],
                        start.to(end),
                        at,
                    );
                }
                _ => return Ok(base),
            }
        }
    }

    // -- patterns -----------------------------------------------------------

    fn pattern(&mut self) -> PResult<PatId> {
        self.enter()?;
        let r = self.pattern_or();
        self.leave();
        r
    }

    fn pattern_or(&mut self) -> PResult<PatId> {
        let at = self.tree.next_pat();
        let first = self.pattern_primary()?;
        if !self.is(Punct::Or) {
            return Ok(first);
        }
        let start = self.tree.pspan(first);
        let base = self.scratch.pats.len();
        self.scratch.pats.push(first);
        while self.eat(Punct::Or) {
            let p = self.pattern_primary()?;
            self.scratch.pats.push(p);
        }
        let last = self.scratch.pats.last().copied();
        let end = last.map_or(start, |p| self.tree.pspan(p));
        let (ks, kl) = self.tree.push_pkids(since(&self.scratch.pats, base));
        self.scratch.pats.truncate(base);
        Ok(self.tree.ppush(PKind::Or, [ks, kl, 0, 0], start.to(end), at))
    }

    fn pattern_primary(&mut self) -> PResult<PatId> {
        let at = self.tree.next_pat();
        let start = self.span();
        match self.peek() {
            TokKind::Underscore => {
                let span = self.bump();
                Ok(self.tree.ppush(PKind::Wild, [0; 4], span, at))
            }
            // The trap this whole layout exists for: a negative literal's
            // *span* starts at the `-` and its *spelling* does not. `-1` has
            // `raw == "1"`, and a formatter that derived the spelling from the
            // span would print `--1`. So the literal's own extent is carried
            // in the payload, separately from the node's span.
            TokKind::Minus => {
                self.bump();
                match self.peek() {
                    TokKind::Int => {
                        let value = self.int_value();
                        let end = self.bump();
                        let ix = self.tree.push_int(value);
                        Ok(self.tree.ppush(
                            PKind::LitInt,
                            [ix, end.start, end.end, 1],
                            start.to(end),
                            at,
                        ))
                    }
                    TokKind::Float => {
                        let value = self.float_value();
                        let end = self.bump();
                        let ix = self.tree.push_float(value);
                        Ok(self.tree.ppush(
                            PKind::LitFloat,
                            [ix, end.start, end.end, 1],
                            start.to(end),
                            at,
                        ))
                    }
                    _ => {
                        let found = self.found();
                        let span = self.span();
                        self.expected(
                            span,
                            "a number after `-`",
                            &found,
                            "negation applies to a numeric literal here",
                        );
                        Err(Bail)
                    }
                }
            }
            TokKind::Int => {
                let value = self.int_value();
                let span = self.bump();
                let ix = self.tree.push_int(value);
                Ok(self.tree.ppush(PKind::LitInt, [ix, span.start, span.end, 0], span, at))
            }
            TokKind::Float => {
                let value = self.float_value();
                let span = self.bump();
                let ix = self.tree.push_float(value);
                Ok(self.tree.ppush(PKind::LitFloat, [ix, span.start, span.end, 0], span, at))
            }
            TokKind::Str => {
                let value = self.take_text();
                let span = self.bump();
                let ix = self.tree.push_str(value);
                Ok(self.tree.ppush(PKind::LitStr, [ix, 0, 0, 0], span, at))
            }
            TokKind::Char => {
                let value = self.char_value();
                let span = self.bump();
                Ok(self.tree.ppush(PKind::LitChar, [value, 0, 0, 0], span, at))
            }
            TokKind::KwTrue => {
                let span = self.bump();
                Ok(self.tree.ppush(PKind::LitTrue, [0; 4], span, at))
            }
            TokKind::KwFalse => {
                let span = self.bump();
                Ok(self.tree.ppush(PKind::LitFalse, [0; 4], span, at))
            }
            // `.Variant`, with or without a payload.
            TokKind::Dot => {
                self.bump();
                let name = self.expect_name()?;
                let payload = self.pattern_payload()?;
                let (ns, nl) = self.tree.push_names(&[Loc::of(name)]);
                Ok(self.tree.ppush(
                    PKind::Path,
                    [ns, nl, payload, 1],
                    start.to(self.prev_span()),
                    at,
                ))
            }
            TokKind::LBracket => self.array_pattern(),
            TokKind::LParen => {
                self.bump();
                if self.is(Punct::RParen) {
                    let end = self.bump();
                    return Ok(self.tree.ppush(PKind::Unit, [0; 4], start.to(end), at));
                }
                let first = self.pattern()?;
                if self.is(Punct::RParen) {
                    self.bump();
                    return Ok(first);
                }
                let base = self.scratch.pats.len();
                self.scratch.pats.push(first);
                while self.eat(Punct::Comma) {
                    if self.is(Punct::RParen) {
                        break;
                    }
                    let p = self.pattern()?;
                    self.scratch.pats.push(p);
                }
                let end = self.expect(Punct::RParen)?;
                let (ks, kl) = self.tree.push_pkids(since(&self.scratch.pats, base));
                self.scratch.pats.truncate(base);
                Ok(self.tree.ppush(PKind::Tuple, [ks, kl, 0, 0], start.to(end), at))
            }
            TokKind::Ident => {
                let first = self.expect_name()?;
                // The token *after* the identifier decides what this is, never
                // what the identifier means (SPEC 12.7).
                if self.is(Punct::Dot) {
                    let nbase = self.scratch.names.len();
                    self.scratch.names.push(Loc::of(first));
                    while self.eat(Punct::Dot) {
                        let n = self.expect_name()?;
                        self.scratch.names.push(Loc::of(n));
                    }
                    let payload = self.pattern_payload()?;
                    let (ns, nl) = self.tree.push_names(since(&self.scratch.names, nbase));
                    self.scratch.names.truncate(nbase);
                    return Ok(self.tree.ppush(
                        PKind::Path,
                        [ns, nl, payload, 0],
                        start.to(self.prev_span()),
                        at,
                    ));
                }
                if self.is(Punct::LParen) || self.is(Punct::LBrace) {
                    let payload = self.pattern_payload()?;
                    let (ns, nl) = self.tree.push_names(&[Loc::of(first)]);
                    return Ok(self.tree.ppush(
                        PKind::Path,
                        [ns, nl, payload, 0],
                        start.to(self.prev_span()),
                        at,
                    ));
                }
                // A bare identifier is ALWAYS a binding.
                let sub = if self.eat(Punct::At) { self.pattern_primary()?.0 } else { NONE };
                let span = start.to(self.prev_span());
                Ok(self.tree.ppush(PKind::Bind, [first.start, first.end, sub, 0], span, at))
            }
            _ => {
                let found = self.found();
                let span = self.span();
                self.expected(
                    span,
                    "a pattern",
                    &found,
                    "write a pattern: a binding, a literal, `.Variant`, or `_`",
                );
                Err(Bail)
            }
        }
    }

    /// A variant pattern's payload, as an index into the payload table or
    /// [`NONE`].
    fn pattern_payload(&mut self) -> PResult<u32> {
        if self.eat(Punct::LParen) {
            let base = self.scratch.pats.len();
            while !self.is(Punct::RParen) && !self.at_eof() {
                let p = self.pattern()?;
                self.scratch.pats.push(p);
                if !self.eat(Punct::Comma) {
                    break;
                }
            }
            self.expect(Punct::RParen)?;
            let (s, l) = self.tree.push_pkids(since(&self.scratch.pats, base));
            self.scratch.pats.truncate(base);
            return Ok(self.tree.push_payload(PatPayloadData {
                record: false,
                rest: false,
                start: s,
                len: l,
            }));
        }
        if self.eat(Punct::LBrace) {
            let base = self.scratch.fpats.len();
            let mut rest = false;
            while !self.is(Punct::RBrace) && !self.at_eof() {
                if self.is(Punct::DotDot) {
                    self.bump();
                    rest = true;
                    self.eat(Punct::Comma);
                    break;
                }
                let name = self.expect_name()?;
                let pattern = if self.eat(Punct::Colon) { self.pattern()?.0 } else { NONE };
                let span = name.to(self.prev_span());
                self.scratch.fpats.push(FieldPatData {
                    name: Loc::of(name),
                    pattern,
                    span: Loc::of(span),
                });
                if !self.eat(Punct::Comma) {
                    break;
                }
            }
            self.expect(Punct::RBrace)?;
            let (s, l) = self.tree.push_fpats(since(&self.scratch.fpats, base));
            self.scratch.fpats.truncate(base);
            return Ok(self.tree.push_payload(PatPayloadData {
                record: true,
                rest,
                start: s,
                len: l,
            }));
        }
        Ok(NONE)
    }

    fn array_pattern(&mut self) -> PResult<PatId> {
        let at = self.tree.next_pat();
        let start = self.expect(Punct::LBracket)?;
        let base = self.scratch.pats.len();
        // `Option<Option<Ident>>` in three states: absent, present and
        // anonymous, present and named.
        let mut rest_kind = 0u32;
        let mut rest_name = 0u32;
        while !self.is(Punct::RBracket) && !self.at_eof() {
            if self.is(Punct::DotDot) {
                let dd = self.bump();
                let name = if matches!(self.peek(), TokKind::Ident) {
                    Some(self.expect_name()?)
                } else {
                    None
                };
                if rest_kind != 0 {
                    self.error(
                        dd,
                        "an array pattern may have at most one rest pattern",
                        "keep one `..` and match the other elements by position",
                    );
                }
                match name {
                    Some(n) => {
                        rest_kind = 2;
                        rest_name = self.tree.push_name(Loc::of(n));
                    }
                    None => rest_kind = 1,
                }
                self.eat(Punct::Comma);
                // Rest patterns bind only at the end: `[first, ..rest]` is
                // legal, `[..init, last]` is not.
                if !self.is(Punct::RBracket) {
                    let span = self.span();
                    self.error(
                        span,
                        "a rest pattern must come last",
                        "move `..` to the end, as in `[first, ..rest]`; matching a prefix is \
                         what an array pattern does",
                    )
                    .map(|d| {
                        d.code("rest-pattern-not-last")
                            .note("`[first, ..rest]` is legal; `[..init, last]` is not")
                    });
                    return Err(Bail);
                }
                break;
            }
            let p = self.pattern()?;
            self.scratch.pats.push(p);
            if !self.eat(Punct::Comma) {
                break;
            }
        }
        let end = self.expect(Punct::RBracket)?;
        let (ks, kl) = self.tree.push_pkids(since(&self.scratch.pats, base));
        self.scratch.pats.truncate(base);
        Ok(self.tree.ppush(PKind::Array, [ks, kl, rest_kind, rest_name], start.to(end), at))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(src: &str) -> Module {
        let p = parse(src, FileId(0));
        assert!(p.errors.is_empty(), "unexpected errors in {src:?}: {:#?}", p.errors);
        p.module
    }

    fn bad(src: &str) -> Vec<Diagnostic> {
        let p = parse(src, FileId(0));
        assert!(!p.errors.is_empty(), "expected an error in {src:?}");
        p.errors
    }

    /// Runs `f` with the stack the toolchain gives itself.
    ///
    /// Parsing to [`MAX_DEPTH`] costs about a megabyte and a half of frames,
    /// which the `buri` binary has room for — it reserves `STACK` in `main.rs`
    /// precisely so that anything the parser accepts is something every later
    /// stage can walk — and a test thread, at two megabytes, does not. The
    /// limits below are what the *product* promises, so they are checked
    /// against the product's stack rather than the harness's.
    fn on_the_toolchain_stack(f: impl FnOnce() + Send + 'static) {
        std::thread::Builder::new()
            .stack_size(64 * 1024 * 1024)
            .spawn(f)
            .expect("a thread")
            .join()
            .expect("the case does not panic");
    }

    #[test]
    fn imports() {
        ok(r#"from "core/list" import { map, filter };"#);
        ok(r#"from "core/list" import { map as listMap };"#);
        ok(r#"from "core/list" import * as list;"#);
        ok(r#"from "//lib/money/cents" export { Cents, fromCents };"#);
    }

    #[test]
    fn bare_namespace_import_is_not_derivable() {
        let e = bad(r#"from "core/list" import *;"#);
        assert!(e[0].message.contains("must be named"));
    }

    #[test]
    fn declarations() {
        ok("export fn area(self: Square): Int { self.height * self.width }");
        ok("struct Meters(export F64);");
        ok("struct User { export id: UserId, name: Str }");
        ok("enum Shape { Empty, Circle(Float), Rect { width: Float, height: Float } }");
        ok("type Handler<T> = fn(T) => Result<(), Str>;");
        ok("const MAX: Int = 5;");
        ok("trait Ord { fn compare(self: Self, other: Self): Order; }");
        ok("effect Fs { fn readFile(self: Self, path: Str): Result<Str, IoError>; }");
        ok("impl Ord for Version { fn compare(self: Version, other: Version): Order { .Equal } }");
        ok("derive Eq, Ord, Show for Playlist;");
        ok("context Hermetic { Alloc: alloc(), Fs: data() }");
        ok(r#"test "pads the cents place" { let x = 1; }"#);
    }

    #[test]
    fn comparison_is_non_associative() {
        let e = bad("fn f(): Bool { a < b < c }");
        assert!(e[0].message.contains("non-associative"));
    }

    #[test]
    fn else_is_mandatory() {
        let e = bad("fn f(): Int { if (c) { 1 } }");
        assert!(e[0].message.contains("`else`"));
    }

    #[test]
    fn block_like_cannot_head_a_postfix_chain() {
        // `match (x) { ... }.field` must not parse: the `.field` is left over.
        let e = bad("fn f(): Int { match (x) { _ => 1 }.field }");
        assert!(!e.is_empty());
    }

    #[test]
    fn struct_literal_after_a_path() {
        ok("fn f(): Point { Point { x: 1, y: 2 } }");
        // Shorthand works, because nothing competes with it.
        ok("fn f(): Point { Point { x, y } }");
        ok("fn f(): Point { Point { ..p, x: 1 } }");
    }

    #[test]
    fn a_bare_brace_is_a_block() {
        let m = ok("fn f(): Int { { let n = 1; n } }");
        assert_eq!(m.items.len(), 1);
    }

    #[test]
    fn lambda_is_not_a_bare_operand() {
        bad("fn f(): Int { 2 * fn(x) => x }");
        ok("fn f(): Int { 2 * (fn(x: Int) => x)(3) }");
    }

    #[test]
    fn type_arguments_in_an_expression() {
        ok("fn f(): Int { identity<Int>(7) }");
        ok("fn f(): [Int] { list.empty<Int>() }");
        ok("fn f(): Int { map.new<Str, Int>() }");
        ok("fn f(): Int { list.empty<[Int]>() }");
        ok("fn f(): Int { list.empty<fn(Int) => Int>() }");
        ok("fn f(): Int { list.empty<(Str, Int)>() }");
        // A generic function used as a value, with no call to pin it.
        ok("fn f(): Int { let g = identity<Int>; g(7) }");
        // Not the head of the chain: type arguments in the middle of one.
        ok("fn f(): Int { s.parse<Int>().unwrap<Int>(0) }");
        ok("fn f(): Int { list.empty<Int>()? }");
    }

    /// The other reading of the same tokens. None of these is a call.
    #[test]
    fn a_less_than_is_still_a_comparison() {
        ok("fn f(): Bool { a < b }");
        ok("fn f(): Bool { a<b }");
        ok("fn f(): Bool { f() < g() }");
        ok("fn f(): Bool { x < y && y > z }");
        ok("fn f(): Bool { h(a < b, c) }");
        ok("fn f(): Bool { [a < b, c > d] }");
        ok("fn f(): Bool { match (x) { n if n < 1 => true, _ => false } }");
        ok("fn f(): Int { if (a < b) { 1 } else { 2 } }");
        ok("fn f(): Bool { P { flag: a < b } }");
        // `>` closes a plausible list, but what follows can begin an operand,
        // so the comparison reading stands — and it is a chained one.
        let e = bad("fn f(): Bool { a < b > c }");
        assert!(e[0].message.contains("non-associative"), "{:?}", e[0].message);
        // The same, one argument list deeper: `f(a < b, c > d)` is two
        // comparisons, not `f<a, c>` applied to `d`.
        ok("fn f(): Bool { h(a < b, c > d) }");
    }

    /// `a < b > (c)` is the one shape both readings parse. The comparison
    /// reading of it is a chained comparison, which is not a program, so the
    /// call reading wins — that is the whole point of the new spelling.
    #[test]
    fn type_arguments_win_when_a_call_follows() {
        let m = ok("fn f(): Bool { a < b > (c) }");
        assert_eq!(m.items.len(), 1);
    }

    #[test]
    fn the_turbofish_is_gone() {
        let e = bad("fn f(): [Int] { list.empty::<Int>() }");
        assert_eq!(e.len(), 1, "one error names the whole mistake: {e:#?}");
        assert!(e[0].message.contains("without `::`"), "{:?}", e[0].message);
        assert_eq!(e[0].code.as_deref(), Some("turbofish"));
        // The fix is mechanical, so it travels as bytes: delete the `::`.
        assert_eq!(e[0].edits.len(), 1);
        assert_eq!(e[0].edits[0].replacement, "");
        let at = e[0].edits[0].at;
        let src = "fn f(): [Int] { list.empty::<Int>() }";
        assert_eq!(src.get(at.start as usize..at.end as usize), Some("::"));
    }

    #[test]
    fn colon_colon_is_not_an_operator() {
        let e = bad("fn f(): Int { list::empty() }");
        assert!(e[0].message.contains("`::` is not an operator"), "{:?}", e[0].message);
    }

    #[test]
    fn patterns() {
        ok("fn f(): Int { match (x) { .Some(n) if n > 0 => 1, .None => 2, _ => 3 } }");
        ok("fn f(): Int { match (x) { Point { x: 0, y } => y, anything => anything.x } }");
        ok("fn f(): Int { match (xs) { [] => 0, [x, ..rest] => x } }");
        ok("fn f(): Int { match (x) { whole @ .Circle(r) => 1, _ => 0 } }");
        ok("fn f(): Int { match (x) { .Circle(_) | .Empty => 1, _ => 0 } }");
        ok("fn f(): Int { match (u) { User { id, .. } => 1 } }");
    }

    #[test]
    fn rest_pattern_must_come_last() {
        let e = bad("fn f(): Int { match (xs) { [..init, last] => 1 } }");
        assert!(e.iter().any(|d| d.message.contains("must come last")));
    }

    #[test]
    fn templates_nest_blocks() {
        ok(r#"fn f(): Int { let s = "n = ${ { let x = 1; x } }"; 0 }"#);
    }

    #[test]
    fn tuple_index_chain_needs_parens() {
        let e = bad("fn f(): Int { t.0.1 }");
        assert!(e.iter().any(|d| d.message.contains("lexes as a float")));
        ok("fn f(): Int { (t.0).1 }");
    }

    #[test]
    fn let_ctx_takes_no_annotation() {
        ok("fn f(): Int { let ctx = context { Alloc: host.alloc }; 0 }");
    }

    #[test]
    fn expression_statements_parse_and_are_checked_later() {
        // The grammar admits `Expr ";"`; restricting it to test sources and to
        // type `()` is a static rule, not a grammar one (SPEC 12.2).
        let m = ok(r#"test "t" { assert.eq(a, b); }"#);
        assert_eq!(m.items.len(), 1);
    }

    #[test]
    fn recovery_reports_more_than_one_error() {
        let p = parse("fn a(: Int { } \n fn b(): Int { 1 }", FileId(0));
        assert!(!p.errors.is_empty());
        // The good declaration after the bad one is still parsed.
        assert!(p
            .module
            .items
            .iter()
            .any(|i| matches!(i, Item::Fn(f) if p.module.tree.name(f.name) == "b")));
    }

    /// An `impl` body that holds something other than a method, followed by
    /// another item, used to hang: `fn_decl` bailed without consuming and
    /// `sync_item` stopped at the next keyword without consuming, so the
    /// method loop spun. `{ ... }` is how documentation elides a body, which
    /// is how this was found.
    #[test]
    fn a_malformed_impl_body_does_not_hang() {
        for src in [
            "struct V(Int);\nimpl V { ... }\nderive Eq for V;",
            "impl Ord for V { ... }\nfn after(): Int { 1 }",
            "trait T { ... }\nstruct W(Int);",
            "impl V { ... }",
        ] {
            let p = parse(src, FileId(0));
            assert!(!p.errors.is_empty(), "expected a diagnostic for `{src}`");
        }
        // Recovery still reaches the item after the malformed body.
        let p = parse("impl V { ... }\nfn after(): Int { 1 }", FileId(0));
        assert!(
            p.module
                .items
                .iter()
                .any(|i| matches!(i, Item::Fn(f) if p.module.tree.name(f.name) == "after")),
            "the declaration after a malformed `impl` should still parse"
        );
    }

    /// Everything the parser walks is walked again by every stage after it, so
    /// the depth of what it hands on is the depth of a later stage's stack.
    /// Each of these overflowed one of those stacks.
    #[test]
    fn nesting_past_the_limit_is_a_diagnostic() {
        on_the_toolchain_stack(|| {
            let n = 10_000;
            for (what, src) in [
                ("parentheses", format!("fn f(): I32 {{ {}1{} }}", "(".repeat(n), ")".repeat(n))),
                ("arrays", format!("fn f(): I32 {{ {}1{} }}", "[".repeat(n), "]".repeat(n))),
                ("blocks", format!("fn f(): I32 {{ {}1{} }}", "{".repeat(n), "}".repeat(n))),
                ("types", format!("fn f(): {}I32{} {{ 1 }}", "List<".repeat(n), ">".repeat(n))),
                ("patterns", format!("fn f(): I32 {{ match (1) {{ {}a{} => 1, _ => 0 }} }}", "[".repeat(n), "]".repeat(n))),
            ] {
                let errors = bad(&src);
                assert!(
                    errors.iter().any(|e| e.message.contains("nests too deeply")),
                    "{what}: {:?}",
                    errors.iter().map(|e| &e.message).collect::<Vec<_>>()
                );
            }
        });
    }

    /// A prefix operator recurses into itself, and used to do it without
    /// spending the budget: a hundred thousand `!`s overflowed the stack while
    /// parsing rather than producing a diagnostic.
    #[test]
    fn a_prefix_operator_chain_spends_the_nesting_budget() {
        on_the_toolchain_stack(|| {
            for op in ["!", "-", "~"] {
                let errors = bad(&format!("fn f(): I32 {{ {}1 }}", op.repeat(100_000)));
                assert!(
                    errors.iter().any(|e| e.message.contains("nests too deeply")),
                    "a chain of `{op}` was accepted"
                );
            }
        });
    }

    /// `a + b + c`, `x.f().g()` and `if … else if …` are built by looping, so
    /// the parser's own stack was never the problem — but each link is a node,
    /// and the tree is as deep as the chain is long.
    #[test]
    fn a_chain_past_the_limit_is_a_diagnostic() {
        on_the_toolchain_stack(|| {
            let n = MAX_CHAIN as usize + 10;
            for (what, src) in [
                ("addition", format!("fn f(): I32 {{ 1{} }}", " + 1".repeat(n))),
                ("conjunction", format!("fn f(): Bool {{ true{} }}", " && true".repeat(n))),
                ("method calls", format!("fn f(): I32 {{ 1{} }}", ".abs()".repeat(n))),
                ("coalescing", format!("fn f(): I32 {{ 1{} }}", " ?? 1".repeat(n))),
                (
                    "else-if",
                    format!(
                        "fn f(x: I32): I32 {{ {}{{ -1 }} }}",
                        "if (x == 1) { 1 } else ".repeat(n)
                    ),
                ),
            ] {
                let errors = bad(&src);
                assert!(
                    errors.iter().any(|e| e.message.contains("chain is too long")),
                    "{what}: {:?}",
                    errors.iter().map(|e| &e.message).collect::<Vec<_>>()
                );
            }
        });
    }

    /// The budget is large because the code that reaches it is generated: a
    /// protobuf decoder is one `else if` per field. A schema with a thousand
    /// fields is nobody's mistake, so a thousand links parse.
    #[test]
    fn a_chain_the_length_generated_code_reaches_parses() {
        on_the_toolchain_stack(|| {
            let chain: String =
                (0..1_000).map(|i| format!("if (x == {i}) {{ {i} }} else ")).collect();
            ok(&format!("fn f(x: I32): I32 {{ {chain}{{ -1 }} }}"));
            ok(&format!("fn g(): I32 {{ 1{} }}", " + 1".repeat(1_000)));
        });
    }

    /// One syntax error per location used to be a scan of every error reported
    /// so far, which is quadratic in a file that produces a lot of them — and a
    /// truncated or generated file produces a lot of them.
    #[test]
    fn a_file_full_of_errors_is_read_in_time_proportional_to_its_size() {
        let src: String = (0..40_000).map(|_| "fn ;\n").collect();
        let started = std::time::Instant::now();
        let p = parse(&src, FileId(0));
        assert!(!p.errors.is_empty());
        // Generous by two orders of magnitude: the point is that it is not
        // n², which took half a minute at this size.
        assert!(
            started.elapsed() < std::time::Duration::from_secs(10),
            "parsing 40,000 errors took {:?}",
            started.elapsed()
        );
    }
}
