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

use crate::diagnostics::{Diagnostic, FileId, Invariant as _, Span};
use crate::parsing::lexer::{lex, Kw, Punct, Tok, Token};
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
    let first_item = lexed.tokens.first().map(|t| t.span.start).unwrap_or(0);
    let mut p = Parser {
        toks: lexed.tokens,
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
fn starts_expr(t: &Tok) -> bool {
    match t {
        Tok::Ident(_)
        | Tok::Int(..)
        | Tok::Float(..)
        | Tok::Str(_)
        | Tok::Char(_)
        | Tok::TemplateHead(_) => true,
        Tok::Kw(k) => matches!(
            k,
            Kw::True
                | Kw::False
                | Kw::SelfValue
                | Kw::Ctx
                | Kw::If
                | Kw::Match
                | Kw::Context
                | Kw::Fn
        ),
        Tok::Punct(p) => matches!(
            p,
            // `.Variant`, an array, a tuple or grouping, a block, and the
            // three prefix operators.
            Punct::Dot
                | Punct::LBracket
                | Punct::LParen
                | Punct::LBrace
                | Punct::Minus
                | Punct::Bang
                | Punct::Tilde
        ),
        Tok::TemplateSpan(_) | Tok::TemplateTail(_) | Tok::Eof => false,
    }
}

/// Bail-out for error recovery: unwinds to the nearest item or statement.
struct Bail;
type PResult<T> = Result<T, Bail>;

struct Parser {
    toks: Vec<Token>,
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
/// Each level costs a dozen frames on the way down through the precedence
/// ladder, and nesting this deep is a thing a person wrote rather than a thing
/// a generator emitted, so the budget is small.
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

impl Parser {
    // -- token access -------------------------------------------------------

    /// The token at `i`, or the last one.
    ///
    /// Clamping rather than returning an `Option` is what lets every `peek` in
    /// the parser below be unconditional: the stream always ends with `Eof`,
    /// which every production already has a case for, so running off the end
    /// reads as end-of-file rather than as a missing token. `lex` finishes by
    /// pushing that `Eof` on every path, which is what makes the stream
    /// non-empty and this the only place that has to know it.
    fn at(&self, i: usize) -> &Token {
        let last = self.toks.len().saturating_sub(1);
        self.toks
            .get(i.min(last))
            .or_ice("`lex` ends every token stream with `Eof`, so there is always a last token")
    }

    fn peek(&self) -> &Tok {
        &self.at(self.pos).tok
    }

    fn span(&self) -> Span {
        self.at(self.pos).span
    }

    fn prev_span(&self) -> Span {
        self.at(self.pos.saturating_sub(1)).span
    }

    fn docs(&self) -> Vec<String> {
        self.at(self.pos).docs.clone()
    }

    fn at_eof(&self) -> bool {
        matches!(self.peek(), Tok::Eof)
    }

    fn bump(&mut self) -> Token {
        let t = self.at(self.pos).clone();
        if self.pos < self.toks.len().saturating_sub(1) {
            self.pos = self.pos.saturating_add(1);
        }
        t
    }

    fn is(&self, p: Punct) -> bool {
        matches!(self.peek(), Tok::Punct(q) if *q == p)
    }

    fn is_kw(&self, k: Kw) -> bool {
        matches!(self.peek(), Tok::Kw(q) if *q == k)
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
            Ok(self.bump().span)
        } else {
            let found = self.peek().clone();
            let span = self.span();
            let want = format!("`{}`", p.text());
            self.expected(span, &want, &found, format!("write {want} here"));
            Err(Bail)
        }
    }

    fn expect_kw(&mut self, k: Kw) -> PResult<Span> {
        if self.is_kw(k) {
            Ok(self.bump().span)
        } else {
            let found = self.peek().clone();
            let span = self.span();
            let want = format!("`{}`", k.text());
            self.expected(span, &want, &found, format!("write {want} here"));
            Err(Bail)
        }
    }

    fn expect_ident(&mut self) -> PResult<Ident> {
        match self.peek().clone() {
            Tok::Ident(name) => {
                let span = self.bump().span;
                Ok(Ident::new(name, span))
            }
            other => {
                let span = self.span();
                self.expected(
                    span,
                    "an identifier",
                    &other,
                    "name it: a lowerCamelCase binding, or an UpperCamelCase type",
                );
                Err(Bail)
            }
        }
    }

    fn expect_string(&mut self) -> PResult<(String, Span)> {
        match self.peek().clone() {
            Tok::Str(s) => {
                let span = self.bump().span;
                Ok((s, span))
            }
            other => {
                let span = self.span();
                self.expected(span, "a string literal", &other, "quote it, as in `\"core/list\"`");
                Err(Bail)
            }
        }
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
                Tok::Eof => return,
                Tok::Punct(Punct::LBrace) => {
                    depth = depth.saturating_add(1);
                    self.bump();
                }
                Tok::Punct(Punct::RBrace) => {
                    depth = depth.saturating_sub(1);
                    self.bump();
                    if depth <= 0 {
                        return;
                    }
                }
                Tok::Punct(Punct::Semi) if depth <= 0 => {
                    self.bump();
                    return;
                }
                Tok::Kw(
                    Kw::From
                    | Kw::Export
                    | Kw::Fn
                    | Kw::Struct
                    | Kw::Enum
                    | Kw::Type
                    | Kw::Const
                    | Kw::Trait
                    | Kw::Effect
                    | Kw::Impl
                    | Kw::Derive
                    | Kw::Test,
                ) if depth <= 0 => return,
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
                Tok::Eof => return,
                Tok::Punct(Punct::Semi) if depth <= 0 => {
                    self.bump();
                    return;
                }
                Tok::Punct(Punct::LBrace | Punct::LParen | Punct::LBracket) => {
                    depth = depth.saturating_add(1);
                    self.bump();
                }
                Tok::Punct(Punct::RBrace) if depth <= 0 => return,
                Tok::Punct(Punct::RBrace | Punct::RParen | Punct::RBracket) => {
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
            match self.item() {
                Ok(Some(item)) => items.push(item),
                Ok(None) => {}
                Err(Bail) => self.sync_item(),
            }
            // Guarantee progress even if a sub-parser consumed nothing.
            if self.pos == before {
                self.bump();
            }
        }
        Module { items, docs: Vec::new() }
    }

    fn item(&mut self) -> PResult<Option<Item>> {
        let docs = self.docs();
        let start = self.span();

        if self.is_kw(Kw::From) {
            return Ok(Some(self.import_or_reexport()?));
        }
        if self.is_kw(Kw::Impl) {
            return Ok(Some(Item::Impl(self.impl_decl(docs)?)));
        }
        if self.is_kw(Kw::Derive) {
            return Ok(Some(Item::Derive(self.derive_decl()?)));
        }
        if self.is_kw(Kw::Test) {
            return Ok(Some(Item::Test(self.test_decl(docs)?)));
        }

        let exported = self.eat_kw(Kw::Export);
        let item = match self.peek().clone() {
            Tok::Kw(Kw::Fn) => Item::Fn(self.fn_decl(exported, docs, start)?),
            Tok::Kw(Kw::Struct) => Item::Struct(self.struct_decl(exported, docs, start)?),
            Tok::Kw(Kw::Enum) => Item::Enum(self.enum_decl(exported, docs, start)?),
            Tok::Kw(Kw::Type) => Item::TypeAlias(self.type_alias(exported, docs, start)?),
            Tok::Kw(Kw::Const) => Item::Const(self.const_decl(exported, docs, start)?),
            Tok::Kw(Kw::Trait) => Item::Trait(self.trait_decl(exported, docs, start, false)?),
            Tok::Kw(Kw::Effect) => Item::Trait(self.trait_decl(exported, docs, start, true)?),
            Tok::Kw(Kw::Context) => Item::Context(self.context_decl(exported, docs, start)?),
            // `from "..." export { ... }` after a stray `export`.
            Tok::Kw(Kw::From) if exported => {
                let span = self.span();
                self.error(
                    span,
                    "write `from \"...\" export { ... }`, without a leading `export`",
                    "drop the leading `export`: the `export` after the path is the one that \
                     re-exports",
                );
                return Err(Bail);
            }
            other => {
                let span = self.span();
                self.expected(
                    span,
                    "a declaration",
                    &other,
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
            return Ok(Item::ReExport(ReExport { path, path_span, specs, span: start.to(end) }));
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
        Ok(Item::Import(Import { path, path_span, clause, span: start.to(end) }))
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
            let mut bounds = Vec::new();
            if self.eat(Punct::Colon) {
                loop {
                    bounds.push(self.named_type()?);
                    if !self.eat(Punct::Plus) {
                        break;
                    }
                }
            }
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
                let span = self.bump().span;
                (ParamKind::SelfParam, Ident::new("self", span))
            } else if self.is_kw(Kw::Ctx) {
                let span = self.bump().span;
                (ParamKind::CtxParam, Ident::new("ctx", span))
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
            let span = start.to(ty.span());
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
                let n = name.name.clone();
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
            let found = self.peek().clone();
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
                let mut tys = Vec::new();
                while !self.is(Punct::RParen) && !self.at_eof() {
                    tys.push(self.ty()?);
                    if !self.eat(Punct::Comma) {
                        break;
                    }
                }
                self.expect(Punct::RParen)?;
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
            match self.method_sig() {
                Ok(m) => methods.push(m),
                Err(Bail) => {
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
        let mut traits = vec![self.named_type()?];
        while self.eat(Punct::Comma) {
            traits.push(self.named_type()?);
        }
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

    fn context_body(&mut self) -> PResult<ContextBody> {
        let start = self.expect(Punct::LBrace)?;
        let spread = if self.is(Punct::DotDot) {
            self.bump();
            let e = self.expr()?;
            self.eat(Punct::Comma);
            Some(Box::new(e))
        } else {
            None
        };
        let mut bindings = Vec::new();
        while !self.is(Punct::RBrace) && !self.at_eof() {
            let bstart = self.span();
            let effect = self.named_type()?;
            self.expect(Punct::Colon)?;
            let value = self.expr()?;
            bindings.push(ContextBinding { effect, value, span: bstart.to(self.prev_span()) });
            if !self.eat(Punct::Comma) {
                break;
            }
        }
        let end = self.expect(Punct::RBrace)?;
        Ok(ContextBody { spread, bindings, span: start.to(end) })
    }

    fn test_decl(&mut self, docs: Vec<String>) -> PResult<TestDecl> {
        let start = self.expect_kw(Kw::Test)?;
        let (name, name_span) = self.expect_string()?;
        let body = self.block()?;
        Ok(TestDecl { name, name_span, body, span: start.to(self.prev_span()), docs })
    }

    // -- types --------------------------------------------------------------

    fn named_type(&mut self) -> PResult<TypeExpr> {
        let start = self.span();
        // `Self` is a legal bound position spelling in an impl's type list.
        if self.is_kw(Kw::SelfType) {
            let span = self.bump().span;
            return Ok(TypeExpr::SelfType { span });
        }
        let mut path = vec![self.expect_ident()?];
        while self.is(Punct::Dot) {
            self.bump();
            path.push(self.expect_ident()?);
        }
        let args = self.type_args()?;
        Ok(TypeExpr::Named { path, args, span: start.to(self.prev_span()) })
    }

    fn type_args(&mut self) -> PResult<Vec<TypeExpr>> {
        if !self.is(Punct::Lt) {
            return Ok(Vec::new());
        }
        self.bump();
        let mut args = Vec::new();
        while !self.is(Punct::Gt) && !self.at_eof() {
            args.push(self.ty()?);
            if !self.eat(Punct::Comma) {
                break;
            }
        }
        self.expect(Punct::Gt)?;
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
    fn type_args_in_expr(&mut self) -> Option<Vec<TypeExpr>> {
        if !self.scan_for_type_args() {
            return None;
        }
        let pos = self.pos;
        let depth = self.depth;
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
            Tok::Punct(Punct::LParen | Punct::LBrace) => true,
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
            match &self.at(self.pos.saturating_add(i)).tok {
                Tok::Punct(Punct::Lt) => depth = depth.saturating_add(1),
                Tok::Punct(Punct::Gt) => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        return true;
                    }
                }
                Tok::Ident(_)
                | Tok::Kw(Kw::SelfType | Kw::Fn)
                | Tok::Punct(
                    Punct::Dot
                    | Punct::Comma
                    | Punct::LParen
                    | Punct::RParen
                    | Punct::LBracket
                    | Punct::RBracket
                    | Punct::FatArrow,
                ) => {}
                _ => return false,
            }
        }
        false
    }

    fn ty(&mut self) -> PResult<TypeExpr> {
        self.enter()?;
        let r = self.ty_inner();
        self.leave();
        r
    }

    fn ty_inner(&mut self) -> PResult<TypeExpr> {
        let start = self.span();
        // Function types are written with `fn` for the same reason lambdas are:
        // it makes `(A, B)` unambiguously a tuple everywhere.
        if self.is_kw(Kw::Fn) {
            self.bump();
            self.expect(Punct::LParen)?;
            let mut params = Vec::new();
            while !self.is(Punct::RParen) && !self.at_eof() {
                params.push(self.ty()?);
                if !self.eat(Punct::Comma) {
                    break;
                }
            }
            self.expect(Punct::RParen)?;
            self.expect(Punct::FatArrow)?;
            let ret = Box::new(self.ty()?);
            return Ok(TypeExpr::Fn { params, ret, span: start.to(self.prev_span()) });
        }

        if self.is_kw(Kw::SelfType) {
            let span = self.bump().span;
            return Ok(TypeExpr::SelfType { span });
        }

        if self.is(Punct::LBracket) {
            self.bump();
            let elem = Box::new(self.ty()?);
            self.expect(Punct::RBracket)?;
            return Ok(TypeExpr::Array { elem, span: start.to(self.prev_span()) });
        }

        if self.is(Punct::LParen) {
            self.bump();
            // `()` is unit, `(T)` is grouping, `(T, U)` is a tuple.
            if self.is(Punct::RParen) {
                let end = self.bump().span;
                return Ok(TypeExpr::Unit { span: start.to(end) });
            }
            let first = self.ty()?;
            if self.is(Punct::RParen) {
                self.bump();
                return Ok(first);
            }
            let mut elems = vec![first];
            while self.eat(Punct::Comma) {
                if self.is(Punct::RParen) {
                    break;
                }
                elems.push(self.ty()?);
            }
            self.expect(Punct::RParen)?;
            if elems.len() < 2 {
                let span = start.to(self.prev_span());
                self.error(
                    span,
                    "a tuple type has arity 2 or more",
                    "write the element type on its own for one, or `()` for none",
                )
                .map(|d| d.note("`(T)` is a parenthesized type and `()` is unit"));
            }
            return Ok(TypeExpr::Tuple { elems, span: start.to(self.prev_span()) });
        }

        match self.peek() {
            Tok::Ident(_) => self.named_type(),
            other => {
                let other = other.clone();
                let span = self.span();
                self.expected(span, "a type", &other, "name a type here, as in `Int` or `[Str]`");
                Err(Bail)
            }
        }
    }

    // -- blocks and statements ---------------------------------------------

    fn block(&mut self) -> PResult<Block> {
        self.enter()?;
        let r = self.block_inner();
        self.leave();
        r
    }

    fn block_inner(&mut self) -> PResult<Block> {
        let start = self.expect(Punct::LBrace)?;
        let mut stmts = Vec::new();
        let mut tail = None;

        while !self.is(Punct::RBrace) && !self.at_eof() {
            let before = self.pos;
            if self.is_kw(Kw::Let) {
                match self.let_stmt() {
                    Ok(s) => stmts.push(s),
                    Err(Bail) => self.sync_stmt(),
                }
            } else {
                let estart = self.span();
                match self.expr() {
                    Ok(e) => {
                        if self.is(Punct::Semi) {
                            let end = self.bump().span;
                            // An expression statement is legal only in a test
                            // source and only when its type is `()`; both are
                            // static rules, so the grammar admits it here.
                            stmts.push(Stmt::Expr { expr: e, span: estart.to(end) });
                        } else {
                            tail = Some(Box::new(e));
                            break;
                        }
                    }
                    Err(Bail) => self.sync_stmt(),
                }
            }
            if self.pos == before {
                self.bump();
            }
        }

        let end = self.expect(Punct::RBrace)?;
        Ok(Block { stmts, tail, span: start.to(end) })
    }

    fn let_stmt(&mut self) -> PResult<Stmt> {
        let start = self.expect_kw(Kw::Let)?;
        // After `let`, one token of lookahead decides which form this is: the
        // `ctx` keyword takes no pattern and no annotation, because a context's
        // type is generated and never written.
        if self.is_kw(Kw::Ctx) {
            let name_span = self.bump().span;
            self.expect(Punct::Eq)?;
            let value = self.expr()?;
            let end = self.expect(Punct::Semi)?;
            return Ok(Stmt::Let {
                pattern: Pattern::Bind { name: Ident::new("ctx", name_span), sub: None, span: name_span },
                ty: None,
                value,
                is_ctx: true,
                span: start.to(end),
            });
        }
        let pattern = self.pattern()?;
        let ty = if self.eat(Punct::Colon) { Some(self.ty()?) } else { None };
        self.expect(Punct::Eq)?;
        let value = self.expr()?;
        let end = self.expect(Punct::Semi)?;
        Ok(Stmt::Let { pattern, ty, value, is_ctx: false, span: start.to(end) })
    }

    // -- expressions --------------------------------------------------------

    fn expr(&mut self) -> PResult<Expr> {
        self.enter()?;
        let r = self.expr_inner();
        self.leave();
        r
    }

    fn expr_inner(&mut self) -> PResult<Expr> {
        // A lambda is top-level-only: its body extends as far right as
        // possible, so allowing it as an operand would make
        // `2 * fn(x) => x + 1` ambiguous (SPEC 12.11).
        if self.is_kw(Kw::Fn) {
            return self.lambda();
        }
        self.or_expr()
    }

    fn lambda(&mut self) -> PResult<Expr> {
        let start = self.expect_kw(Kw::Fn)?;
        self.expect(Punct::LParen)?;
        let mut params = Vec::new();
        while !self.is(Punct::RParen) && !self.at_eof() {
            let name = self.expect_ident()?;
            let ty = if self.eat(Punct::Colon) { Some(self.ty()?) } else { None };
            let span = name.span.to(self.prev_span());
            params.push(LambdaParam { name, ty, span });
            if !self.eat(Punct::Comma) {
                break;
            }
        }
        self.expect(Punct::RParen)?;
        let ret = if self.eat(Punct::Colon) { Some(self.ty()?) } else { None };
        self.expect(Punct::FatArrow)?;
        let body = Box::new(self.expr()?);
        let span = start.to(body.span());
        Ok(Expr::Lambda { params, ret, body, span })
    }

    fn or_expr(&mut self) -> PResult<Expr> {
        let mut lhs = self.coalesce_expr()?;
        let mut links = 0u32;
        while self.is(Punct::OrOr) {
            links = links.saturating_add(1);
            self.link(links)?;
            let op_span = self.bump().span;
            let rhs = self.coalesce_expr()?;
            let span = lhs.span().to(rhs.span());
            lhs = Expr::Binary { op: BinOp::Or, lhs: Box::new(lhs), rhs: Box::new(rhs), op_span, span };
        }
        Ok(lhs)
    }

    /// `??` is right-associative, so `a ?? b ?? c` works.
    fn coalesce_expr(&mut self) -> PResult<Expr> {
        let lhs = self.and_expr()?;
        if self.is(Punct::QuestionQuestion) {
            let op_span = self.bump().span;
            self.chain_in()?;
            let rhs = self.coalesce_expr();
            self.chain_out();
            let rhs = rhs?;
            let span = lhs.span().to(rhs.span());
            return Ok(Expr::Binary {
                op: BinOp::Coalesce,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
                op_span,
                span,
            });
        }
        Ok(lhs)
    }

    fn and_expr(&mut self) -> PResult<Expr> {
        let mut lhs = self.cmp_expr()?;
        let mut links = 0u32;
        while self.is(Punct::AndAnd) {
            links = links.saturating_add(1);
            self.link(links)?;
            let op_span = self.bump().span;
            let rhs = self.cmp_expr()?;
            let span = lhs.span().to(rhs.span());
            lhs =
                Expr::Binary { op: BinOp::And, lhs: Box::new(lhs), rhs: Box::new(rhs), op_span, span };
        }
        Ok(lhs)
    }

    /// Comparison is non-associative: `a < b < c` is a parse error, not a bug
    /// waiting to happen (SPEC 6.1).
    fn cmp_expr(&mut self) -> PResult<Expr> {
        let lhs = self.bitor_expr()?;
        let op = match self.peek() {
            Tok::Punct(Punct::EqEq) => BinOp::Eq,
            Tok::Punct(Punct::BangEq) => BinOp::Ne,
            Tok::Punct(Punct::Lt) => BinOp::Lt,
            Tok::Punct(Punct::LtEq) => BinOp::Le,
            Tok::Punct(Punct::Gt) => BinOp::Gt,
            Tok::Punct(Punct::GtEq) => BinOp::Ge,
            _ => return Ok(lhs),
        };
        let op_span = self.bump().span;
        let rhs = self.bitor_expr()?;
        let span = lhs.span().to(rhs.span());
        let result = Expr::Binary { op, lhs: Box::new(lhs), rhs: Box::new(rhs), op_span, span };

        if matches!(
            self.peek(),
            Tok::Punct(
                Punct::EqEq | Punct::BangEq | Punct::Lt | Punct::LtEq | Punct::Gt | Punct::GtEq
            )
        ) {
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
            // Consume the rest of the chain and hand back what was parsed, so
            // this one diagnostic is not followed by a cascade of type errors
            // about the recovered shape.
            while matches!(
                self.peek(),
                Tok::Punct(
                    Punct::EqEq | Punct::BangEq | Punct::Lt | Punct::LtEq | Punct::Gt | Punct::GtEq
                )
            ) {
                self.bump();
                let _ = self.bitor_expr()?;
            }
            return Ok(result);
        }
        Ok(result)
    }

    fn bitor_expr(&mut self) -> PResult<Expr> {
        let mut lhs = self.bitxor_expr()?;
        let mut links = 0u32;
        while self.is(Punct::Or) {
            links = links.saturating_add(1);
            self.link(links)?;
            let op_span = self.bump().span;
            let rhs = self.bitxor_expr()?;
            let span = lhs.span().to(rhs.span());
            lhs = Expr::Binary {
                op: BinOp::BitOr,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
                op_span,
                span,
            };
        }
        Ok(lhs)
    }

    fn bitxor_expr(&mut self) -> PResult<Expr> {
        let mut lhs = self.bitand_expr()?;
        let mut links = 0u32;
        while self.is(Punct::Caret) {
            links = links.saturating_add(1);
            self.link(links)?;
            let op_span = self.bump().span;
            let rhs = self.bitand_expr()?;
            let span = lhs.span().to(rhs.span());
            lhs = Expr::Binary {
                op: BinOp::BitXor,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
                op_span,
                span,
            };
        }
        Ok(lhs)
    }

    fn bitand_expr(&mut self) -> PResult<Expr> {
        let mut lhs = self.add_expr()?;
        let mut links = 0u32;
        while self.is(Punct::And) {
            links = links.saturating_add(1);
            self.link(links)?;
            let op_span = self.bump().span;
            let rhs = self.add_expr()?;
            let span = lhs.span().to(rhs.span());
            lhs = Expr::Binary {
                op: BinOp::BitAnd,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
                op_span,
                span,
            };
        }
        Ok(lhs)
    }

    fn add_expr(&mut self) -> PResult<Expr> {
        let mut lhs = self.mul_expr()?;
        let mut links = 0u32;
        loop {
            let op = match self.peek() {
                Tok::Punct(Punct::Plus) => BinOp::Add,
                Tok::Punct(Punct::Minus) => BinOp::Sub,
                _ => break,
            };
            links = links.saturating_add(1);
            self.link(links)?;
            let op_span = self.bump().span;
            let rhs = self.mul_expr()?;
            let span = lhs.span().to(rhs.span());
            lhs = Expr::Binary { op, lhs: Box::new(lhs), rhs: Box::new(rhs), op_span, span };
        }
        Ok(lhs)
    }

    fn mul_expr(&mut self) -> PResult<Expr> {
        let mut lhs = self.unary_expr()?;
        let mut links = 0u32;
        loop {
            let op = match self.peek() {
                Tok::Punct(Punct::Star) => BinOp::Mul,
                Tok::Punct(Punct::Slash) => BinOp::Div,
                Tok::Punct(Punct::Percent) => BinOp::Rem,
                _ => break,
            };
            links = links.saturating_add(1);
            self.link(links)?;
            let op_span = self.bump().span;
            let rhs = self.unary_expr()?;
            let span = lhs.span().to(rhs.span());
            lhs = Expr::Binary { op, lhs: Box::new(lhs), rhs: Box::new(rhs), op_span, span };
        }
        Ok(lhs)
    }

    /// A prefix operator's operand is another unary expression, so this is the
    /// second place in the grammar that recurses without passing through
    /// [`Parser::expr`] — `!!!!…true` and `----…1` are one production deep in
    /// the reader's eyes and a hundred thousand frames deep in the parser's.
    /// It costs one unit of the same budget the other four spend.
    fn unary_expr(&mut self) -> PResult<Expr> {
        self.enter()?;
        let r = self.unary_inner();
        self.leave();
        r
    }

    fn unary_inner(&mut self) -> PResult<Expr> {
        let op = match self.peek() {
            Tok::Punct(Punct::Minus) => Some(UnOp::Neg),
            Tok::Punct(Punct::Bang) => Some(UnOp::Not),
            Tok::Punct(Punct::Tilde) => Some(UnOp::BitNot),
            _ => None,
        };
        if let Some(op) = op {
            let start = self.bump().span;
            let operand = Box::new(self.unary_expr()?);
            let span = start.to(operand.span());
            return Ok(Expr::Unary { op, operand, span });
        }

        // Block-like expressions are operands but never postfix-chain heads,
        // which is what stops `if (c) { a } else { b } { x: 1 }` from having
        // two parses (SPEC 12.13). They are returned without entering
        // `postfix_ops`.
        if self.is(Punct::LBrace) {
            return Ok(Expr::Block(self.block()?));
        }
        if self.is_kw(Kw::If) {
            return self.if_expr();
        }
        if self.is_kw(Kw::Match) {
            return self.match_expr();
        }
        if self.is_kw(Kw::Context) {
            let start = self.bump().span;
            let body = self.context_body()?;
            let span = start.to(body.span);
            return Ok(Expr::ContextExpr { body, span });
        }

        let primary = self.primary_expr()?;
        self.postfix_ops(primary)
    }

    fn if_expr(&mut self) -> PResult<Expr> {
        let start = self.expect_kw(Kw::If)?;
        // The condition is parenthesized, so the `{` that follows is always a
        // block (SPEC 12.1).
        self.expect(Punct::LParen)?;
        let cond = Box::new(self.expr()?);
        self.expect(Punct::RParen)?;
        let then = self.block()?;
        // `else` is mandatory. There is nothing sensible for a missing branch
        // to produce in a language where `if` is an expression.
        if !self.is_kw(Kw::Else) {
            let span = then.span;
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
            Box::new(nested?)
        } else {
            Box::new(Expr::Block(self.block()?))
        };
        let span = start.to(else_.span());
        Ok(Expr::If { cond, then, else_, span })
    }

    fn match_expr(&mut self) -> PResult<Expr> {
        let start = self.expect_kw(Kw::Match)?;
        self.expect(Punct::LParen)?;
        let scrutinee = Box::new(self.expr()?);
        self.expect(Punct::RParen)?;
        self.expect(Punct::LBrace)?;
        let mut arms = Vec::new();
        while !self.is(Punct::RBrace) && !self.at_eof() {
            let before = self.pos;
            let astart = self.span();
            let arm = (|| -> PResult<MatchArm> {
                let pattern = self.pattern()?;
                let guard = if self.eat_kw(Kw::If) { Some(self.expr()?) } else { None };
                self.expect(Punct::FatArrow)?;
                let body = self.expr()?;
                Ok(MatchArm { pattern, guard, body, span: astart.to(self.prev_span()) })
            })();
            match arm {
                Ok(a) => arms.push(a),
                Err(Bail) => {
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
        Ok(Expr::Match { scrutinee, arms, span: start.to(end) })
    }

    fn sync_match_arm(&mut self) {
        let mut depth = 0i32;
        loop {
            match self.peek() {
                Tok::Eof => return,
                Tok::Punct(Punct::Comma) if depth <= 0 => return,
                Tok::Punct(Punct::RBrace) if depth <= 0 => return,
                Tok::Punct(Punct::LBrace | Punct::LParen | Punct::LBracket) => {
                    depth = depth.saturating_add(1);
                    self.bump();
                }
                Tok::Punct(Punct::RBrace | Punct::RParen | Punct::RBracket) => {
                    depth = depth.saturating_sub(1);
                    self.bump();
                }
                _ => {
                    self.bump();
                }
            }
        }
    }

    fn primary_expr(&mut self) -> PResult<Expr> {
        let start = self.span();
        match self.peek().clone() {
            Tok::Int(value, raw) => {
                let span = self.bump().span;
                Ok(Expr::Int { value, raw, span })
            }
            Tok::Float(value, raw) => {
                let span = self.bump().span;
                Ok(Expr::Float { value, raw, span })
            }
            Tok::Str(value) => {
                let span = self.bump().span;
                Ok(Expr::Str { value, span })
            }
            Tok::Char(value) => {
                let span = self.bump().span;
                Ok(Expr::Char { value, span })
            }
            Tok::Kw(Kw::True) => {
                let span = self.bump().span;
                Ok(Expr::Bool { value: true, span })
            }
            Tok::Kw(Kw::False) => {
                let span = self.bump().span;
                Ok(Expr::Bool { value: false, span })
            }
            Tok::TemplateHead(head) => self.template(head, start),
            Tok::Ident(name) => {
                let span = self.bump().span;
                Ok(Expr::Ident { name, span })
            }
            Tok::Kw(Kw::SelfValue) => {
                let span = self.bump().span;
                Ok(Expr::SelfValue { span })
            }
            Tok::Kw(Kw::Ctx) => {
                let span = self.bump().span;
                Ok(Expr::Ctx { span })
            }
            // `.Variant` — the inferred-type dot form.
            Tok::Punct(Punct::Dot) => {
                self.bump();
                let name = self.expect_ident()?;
                let span = start.to(name.span);
                Ok(Expr::DotVariant { name, span })
            }
            Tok::Punct(Punct::LBracket) => {
                self.bump();
                let mut elems = Vec::new();
                while !self.is(Punct::RBracket) && !self.at_eof() {
                    elems.push(self.expr()?);
                    if !self.eat(Punct::Comma) {
                        break;
                    }
                }
                let end = self.expect(Punct::RBracket)?;
                Ok(Expr::Array { elems, span: start.to(end) })
            }
            Tok::Punct(Punct::LParen) => {
                self.bump();
                if self.is(Punct::RParen) {
                    let end = self.bump().span;
                    return Ok(Expr::Unit { span: start.to(end) });
                }
                let first = self.expr()?;
                if self.is(Punct::RParen) {
                    self.bump();
                    return Ok(first);
                }
                let mut elems = vec![first];
                while self.eat(Punct::Comma) {
                    if self.is(Punct::RParen) {
                        break;
                    }
                    elems.push(self.expr()?);
                }
                let end = self.expect(Punct::RParen)?;
                if elems.len() < 2 {
                    self.error(
                        start.to(end),
                        "a tuple has arity 2 or more",
                        "drop the parentheses for one element, or write `()` for none",
                    )
                    .map(|d| d.note("`(e)` is grouping and `()` is the unit value"));
                }
                Ok(Expr::Tuple { elems, span: start.to(end) })
            }
            other => {
                let span = self.span();
                self.expected(span, "an expression", &other, "write a value here");
                Err(Bail)
            }
        }
    }

    fn template(&mut self, head: String, start: Span) -> PResult<Expr> {
        self.bump();
        let mut parts = Vec::new();
        if !head.is_empty() {
            parts.push(TemplatePart::Text(head));
        }
        loop {
            let hole = self.expr()?;
            parts.push(TemplatePart::Hole(hole));
            match self.peek().clone() {
                Tok::TemplateSpan(text) => {
                    self.bump();
                    if !text.is_empty() {
                        parts.push(TemplatePart::Text(text));
                    }
                }
                Tok::TemplateTail(text) => {
                    let end = self.bump().span;
                    if !text.is_empty() {
                        parts.push(TemplatePart::Text(text));
                    }
                    return Ok(Expr::Template { parts, span: start.to(end) });
                }
                other => {
                    let span = self.span();
                    self.expected(
                        span,
                        "the rest of the string",
                        &other,
                        "close the template: every `${` needs a `}` and the string needs a \
                         closing quote",
                    );
                    return Err(Bail);
                }
            }
        }
    }

    fn postfix_ops(&mut self, mut base: Expr) -> PResult<Expr> {
        let mut links = 0u32;
        loop {
            links = links.saturating_add(1);
            self.link(links)?;
            let start = base.span();
            match self.peek().clone() {
                Tok::Punct(Punct::Dot) => {
                    self.bump();
                    match self.peek().clone() {
                        // Tuple element access. `t.0.1` lexes as `t` `.` `0.1`,
                        // a known wart; write `(t.0).1`.
                        Tok::Int(value, raw) => {
                            let index_span = self.bump().span;
                            if value > u32::MAX as u128 {
                                self.error(
                                index_span,
                                format!("`{raw}` is not a tuple index"),
                                "a tuple index is a plain decimal number, as in `pair.0`",
                            );
                            }
                            base = Expr::TupleIndex {
                                base: Box::new(base),
                                index: value as u32,
                                index_span,
                                span: start.to(index_span),
                            };
                        }
                        Tok::Float(_, raw) => {
                            let span = self.span();
                            self.error(
                                span,
                                format!("`.{raw}` lexes as a float, not two tuple indices"),
                                "parenthesize the first index: `(t.0).1`",
                            );
                            return Err(Bail);
                        }
                        _ => {
                            let name = self.expect_ident()?;
                            let span = start.to(name.span);
                            base = Expr::Field { base: Box::new(base), name, span };
                        }
                    }
                }
                Tok::Punct(Punct::LParen) => {
                    self.bump();
                    let mut args = Vec::new();
                    while !self.is(Punct::RParen) && !self.at_eof() {
                        args.push(self.expr()?);
                        if !self.eat(Punct::Comma) {
                            break;
                        }
                    }
                    let end = self.expect(Punct::RParen)?;
                    base = Expr::Call { callee: Box::new(base), args, span: start.to(end) };
                }
                Tok::Punct(Punct::LBracket) => {
                    self.bump();
                    let index = Box::new(self.expr()?);
                    let end = self.expect(Punct::RBracket)?;
                    base = Expr::Index { base: Box::new(base), index, span: start.to(end) };
                }
                Tok::Punct(Punct::Question) => {
                    let end = self.bump().span;
                    base = Expr::Try { base: Box::new(base), span: start.to(end) };
                }
                // Type arguments, or the comparison operator that is spelled
                // the same. `type_args_in_expr` decides and rewinds if it is
                // the latter, leaving the `<` for the binary parser above.
                Tok::Punct(Punct::Lt) => match self.type_args_in_expr() {
                    Some(args) => {
                        base = Expr::Generic {
                            base: Box::new(base),
                            args,
                            span: start.to(self.prev_span()),
                        };
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
                Tok::Punct(Punct::ColonColon) => {
                    let colons = self.bump().span;
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
                    base = Expr::Generic {
                        base: Box::new(base),
                        args,
                        span: start.to(self.prev_span()),
                    };
                }
                // With records gone, a `{` following a path is always a struct
                // literal. Nothing competes, so field shorthand is unambiguous.
                Tok::Punct(Punct::LBrace) => {
                    self.bump();
                    let spread = if self.is(Punct::DotDot) {
                        self.bump();
                        let e = self.expr()?;
                        self.eat(Punct::Comma);
                        Some(Box::new(e))
                    } else {
                        None
                    };
                    let mut fields = Vec::new();
                    while !self.is(Punct::RBrace) && !self.at_eof() {
                        let fname = self.expect_ident()?;
                        let value = if self.eat(Punct::Colon) { Some(self.expr()?) } else { None };
                        let fspan = fname.span.to(self.prev_span());
                        fields.push(FieldInit { name: fname, value, span: fspan });
                        if !self.eat(Punct::Comma) {
                            break;
                        }
                    }
                    let end = self.expect(Punct::RBrace)?;
                    base = Expr::StructLit {
                        head: Box::new(base),
                        spread,
                        fields,
                        span: start.to(end),
                    };
                }
                _ => return Ok(base),
            }
        }
    }

    // -- patterns -----------------------------------------------------------

    fn pattern(&mut self) -> PResult<Pattern> {
        self.enter()?;
        let r = self.pattern_or();
        self.leave();
        r
    }

    fn pattern_or(&mut self) -> PResult<Pattern> {
        let first = self.pattern_primary()?;
        if !self.is(Punct::Or) {
            return Ok(first);
        }
        let start = first.span();
        let mut alts = vec![first];
        while self.eat(Punct::Or) {
            alts.push(self.pattern_primary()?);
        }
        let end = alts.last().map_or(start, |p| p.span());
        Ok(Pattern::Or { alts, span: start.to(end) })
    }

    fn pattern_primary(&mut self) -> PResult<Pattern> {
        let start = self.span();
        match self.peek().clone() {
            Tok::Punct(Punct::Underscore) => {
                let span = self.bump().span;
                Ok(Pattern::Wild { span })
            }
            Tok::Punct(Punct::Minus) => {
                self.bump();
                match self.peek().clone() {
                    Tok::Int(value, raw) => {
                        let end = self.bump().span;
                        Ok(Pattern::LitInt { value, negative: true, raw, span: start.to(end) })
                    }
                    Tok::Float(value, raw) => {
                        let end = self.bump().span;
                        Ok(Pattern::LitFloat { value, negative: true, raw, span: start.to(end) })
                    }
                    other => {
                        let span = self.span();
                        self.expected(
                            span,
                            "a number after `-`",
                            &other,
                            "negation applies to a numeric literal here",
                        );
                        Err(Bail)
                    }
                }
            }
            Tok::Int(value, raw) => {
                let span = self.bump().span;
                Ok(Pattern::LitInt { value, negative: false, raw, span })
            }
            Tok::Float(value, raw) => {
                let span = self.bump().span;
                Ok(Pattern::LitFloat { value, negative: false, raw, span })
            }
            Tok::Str(value) => {
                let span = self.bump().span;
                Ok(Pattern::LitStr { value, span })
            }
            Tok::Char(value) => {
                let span = self.bump().span;
                Ok(Pattern::LitChar { value, span })
            }
            Tok::Kw(Kw::True) => {
                let span = self.bump().span;
                Ok(Pattern::LitBool { value: true, span })
            }
            Tok::Kw(Kw::False) => {
                let span = self.bump().span;
                Ok(Pattern::LitBool { value: false, span })
            }
            // `.Variant`, with or without a payload.
            Tok::Punct(Punct::Dot) => {
                self.bump();
                let name = self.expect_ident()?;
                let payload = self.pattern_payload()?;
                Ok(Pattern::Path {
                    path: vec![name],
                    dotted: true,
                    payload,
                    span: start.to(self.prev_span()),
                })
            }
            Tok::Punct(Punct::LBracket) => self.array_pattern(),
            Tok::Punct(Punct::LParen) => {
                self.bump();
                if self.is(Punct::RParen) {
                    let end = self.bump().span;
                    return Ok(Pattern::Unit { span: start.to(end) });
                }
                let first = self.pattern()?;
                if self.is(Punct::RParen) {
                    self.bump();
                    return Ok(first);
                }
                let mut elems = vec![first];
                while self.eat(Punct::Comma) {
                    if self.is(Punct::RParen) {
                        break;
                    }
                    elems.push(self.pattern()?);
                }
                let end = self.expect(Punct::RParen)?;
                Ok(Pattern::Tuple { elems, span: start.to(end) })
            }
            Tok::Ident(_) => {
                let first = self.expect_ident()?;
                // The token *after* the identifier decides what this is, never
                // what the identifier means (SPEC 12.7).
                if self.is(Punct::Dot) {
                    let mut path = vec![first];
                    while self.eat(Punct::Dot) {
                        path.push(self.expect_ident()?);
                    }
                    let payload = self.pattern_payload()?;
                    return Ok(Pattern::Path {
                        path,
                        dotted: false,
                        payload,
                        span: start.to(self.prev_span()),
                    });
                }
                if self.is(Punct::LParen) || self.is(Punct::LBrace) {
                    let payload = self.pattern_payload()?;
                    return Ok(Pattern::Path {
                        path: vec![first],
                        dotted: false,
                        payload,
                        span: start.to(self.prev_span()),
                    });
                }
                // A bare identifier is ALWAYS a binding.
                let sub = if self.eat(Punct::At) {
                    Some(Box::new(self.pattern_primary()?))
                } else {
                    None
                };
                let span = start.to(self.prev_span());
                Ok(Pattern::Bind { name: first, sub, span })
            }
            other => {
                let span = self.span();
                self.expected(
                    span,
                    "a pattern",
                    &other,
                    "write a pattern: a binding, a literal, `.Variant`, or `_`",
                );
                Err(Bail)
            }
        }
    }

    fn pattern_payload(&mut self) -> PResult<Option<PatPayload>> {
        if self.eat(Punct::LParen) {
            let mut ps = Vec::new();
            while !self.is(Punct::RParen) && !self.at_eof() {
                ps.push(self.pattern()?);
                if !self.eat(Punct::Comma) {
                    break;
                }
            }
            self.expect(Punct::RParen)?;
            return Ok(Some(PatPayload::Tuple(ps)));
        }
        if self.eat(Punct::LBrace) {
            let mut fields = Vec::new();
            let mut rest = false;
            while !self.is(Punct::RBrace) && !self.at_eof() {
                if self.is(Punct::DotDot) {
                    self.bump();
                    rest = true;
                    self.eat(Punct::Comma);
                    break;
                }
                let name = self.expect_ident()?;
                let pattern = if self.eat(Punct::Colon) { Some(self.pattern()?) } else { None };
                let span = name.span.to(self.prev_span());
                fields.push(FieldPat { name, pattern, span });
                if !self.eat(Punct::Comma) {
                    break;
                }
            }
            self.expect(Punct::RBrace)?;
            return Ok(Some(PatPayload::Record { fields, rest }));
        }
        Ok(None)
    }

    fn array_pattern(&mut self) -> PResult<Pattern> {
        let start = self.expect(Punct::LBracket)?;
        let mut elems = Vec::new();
        let mut rest = None;
        while !self.is(Punct::RBracket) && !self.at_eof() {
            if self.is(Punct::DotDot) {
                let dd = self.bump().span;
                let name = if matches!(self.peek(), Tok::Ident(_)) {
                    Some(self.expect_ident()?)
                } else {
                    None
                };
                if rest.is_some() {
                    self.error(
                        dd,
                        "an array pattern may have at most one rest pattern",
                        "keep one `..` and match the other elements by position",
                    );
                }
                rest = Some(name);
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
            elems.push(self.pattern()?);
            if !self.eat(Punct::Comma) {
                break;
            }
        }
        let end = self.expect(Punct::RBracket)?;
        Ok(Pattern::Array { elems, rest, span: start.to(end) })
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
        assert!(p.module.items.iter().any(|i| matches!(i, Item::Fn(f) if f.name.name == "b")));
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
            p.module.items.iter().any(|i| matches!(i, Item::Fn(f) if f.name.name == "after")),
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
