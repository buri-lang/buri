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

use crate::ast::*;
use crate::diag::{Diagnostic, FileId, Span};
use crate::lex::{lex, Kw, Punct, Tok, Token};

pub struct Parsed {
    pub module: Module,
    pub errors: Vec<Diagnostic>,
}

/// Parse one source file.
pub fn parse(text: &str, file: FileId) -> Parsed {
    parse_with(text, file, false)
}

/// Parse an embedded standard library module, where a `fn` may be declared
/// without a body: the operations the backend supplies are declared for their
/// signatures and implemented in the runtime (see `stdlib.rs`).
pub fn parse_stdlib(text: &str, file: FileId) -> Parsed {
    parse_with(text, file, true)
}

fn parse_with(text: &str, file: FileId, allow_bodyless: bool) -> Parsed {
    let lexed = lex(text, file);
    let first_item = lexed.tokens.first().map(|t| t.span.start).unwrap_or(0);
    let mut p = Parser {
        toks: lexed.tokens,
        pos: 0,
        errors: lexed.errors,
        allow_bodyless,
        depth: 0,
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

/// Bail-out for error recovery: unwinds to the nearest item or statement.
struct Bail;
type PResult<T> = Result<T, Bail>;

struct Parser {
    toks: Vec<Token>,
    pos: usize,
    errors: Vec<Diagnostic>,
    allow_bodyless: bool,
    depth: u32,
}

const MAX_DEPTH: u32 = 256;

impl Parser {
    // -- token access -------------------------------------------------------

    fn peek(&self) -> &Tok {
        &self.toks[self.pos.min(self.toks.len() - 1)].tok
    }

    fn span(&self) -> Span {
        self.toks[self.pos.min(self.toks.len() - 1)].span
    }

    fn prev_span(&self) -> Span {
        self.toks[self.pos.saturating_sub(1).min(self.toks.len() - 1)].span
    }

    fn docs(&self) -> Vec<String> {
        self.toks[self.pos.min(self.toks.len() - 1)].docs.clone()
    }

    fn at_eof(&self) -> bool {
        matches!(self.peek(), Tok::Eof)
    }

    fn bump(&mut self) -> Token {
        let t = self.toks[self.pos.min(self.toks.len() - 1)].clone();
        if self.pos < self.toks.len() - 1 {
            self.pos += 1;
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
        if self.errors.iter().any(|e| e.span == span) {
            return None;
        }
        self.errors.push(Diagnostic::error(span, msg).with_fix(fix));
        self.errors.last_mut()
    }

    /// A syntax error is always a mismatch: the grammar admits one thing here
    /// and the source has another.
    fn expected(
        &mut self,
        span: Span,
        want: impl std::fmt::Display,
        found: impl std::fmt::Display,
        fix: impl Into<String>,
    ) {
        if self.errors.iter().any(|e| e.span == span) {
            return;
        }
        self.errors.push(
            Diagnostic::error(span, format!("expected {want}, found {found}")).with_code("unexpected-token")
                .with_mismatch(want.to_string(), found.to_string())
                .with_fix(fix),
        );
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
        self.depth += 1;
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
        self.depth -= 1;
    }

    // -- recovery -----------------------------------------------------------

    /// Skip to the start of something that could begin a new item.
    fn sync_item(&mut self) {
        let mut depth = 0i32;
        loop {
            match self.peek() {
                Tok::Eof => return,
                Tok::Punct(Punct::LBrace) => {
                    depth += 1;
                    self.bump();
                }
                Tok::Punct(Punct::RBrace) => {
                    depth -= 1;
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
                    depth += 1;
                    self.bump();
                }
                Tok::Punct(Punct::RBrace) if depth <= 0 => return,
                Tok::Punct(Punct::RBrace | Punct::RParen | Punct::RBracket) => {
                    depth -= 1;
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
                ).map(|d| d.code("unnamed-namespace-import"));
                self.errors.last_mut().unwrap().notes.push(
                    "write `import * as name`; bare `import *` is not derivable from the grammar, \
                     so that no identifier enters a module's scope without appearing in that \
                     module's own source"
                        .into(),
                );
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
                self.errors
                    .last_mut()
                    .unwrap()
                    .notes
                    .push("a function declaration outside a trait or effect needs a block".into());
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
                ).map(|d| d.code("impl-method-export"));
                    self.errors.last_mut().unwrap().notes.push(
                        "conformance is a property of the type, visible wherever the type is"
                            .into(),
                    );
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
                );
                self.errors
                    .last_mut()
                    .unwrap()
                    .notes
                    .push("`(T)` is a parenthesized type and `()` is unit".into());
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
        while self.is(Punct::OrOr) {
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
            let rhs = self.coalesce_expr()?;
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
        while self.is(Punct::AndAnd) {
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
            ).map(|d| d.code("chained-comparison"));
            self.errors
                .last_mut()
                .unwrap()
                .notes
                .push("write `a < b && b < c` rather than `a < b < c`".into());
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
        while self.is(Punct::Or) {
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
        while self.is(Punct::Caret) {
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
        while self.is(Punct::And) {
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
        loop {
            let op = match self.peek() {
                Tok::Punct(Punct::Plus) => BinOp::Add,
                Tok::Punct(Punct::Minus) => BinOp::Sub,
                _ => break,
            };
            let op_span = self.bump().span;
            let rhs = self.mul_expr()?;
            let span = lhs.span().to(rhs.span());
            lhs = Expr::Binary { op, lhs: Box::new(lhs), rhs: Box::new(rhs), op_span, span };
        }
        Ok(lhs)
    }

    fn mul_expr(&mut self) -> PResult<Expr> {
        let mut lhs = self.unary_expr()?;
        loop {
            let op = match self.peek() {
                Tok::Punct(Punct::Star) => BinOp::Mul,
                Tok::Punct(Punct::Slash) => BinOp::Div,
                Tok::Punct(Punct::Percent) => BinOp::Rem,
                _ => break,
            };
            let op_span = self.bump().span;
            let rhs = self.unary_expr()?;
            let span = lhs.span().to(rhs.span());
            lhs = Expr::Binary { op, lhs: Box::new(lhs), rhs: Box::new(rhs), op_span, span };
        }
        Ok(lhs)
    }

    fn unary_expr(&mut self) -> PResult<Expr> {
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
            .map(|d| d.code("if-without-else"));
            self.errors.last_mut().unwrap().notes.push(
                "`if` is an expression, so both branches must produce a value of the same type"
                    .into(),
            );
            return Err(Bail);
        }
        self.bump();
        let else_ = if self.is_kw(Kw::If) {
            Box::new(self.if_expr()?)
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
                    depth += 1;
                    self.bump();
                }
                Tok::Punct(Punct::RBrace | Punct::RParen | Punct::RBracket) => {
                    depth -= 1;
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
                    );
                    self.errors
                        .last_mut()
                        .unwrap()
                        .notes
                        .push("`(e)` is grouping and `()` is the unit value".into());
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
        loop {
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
                Tok::Punct(Punct::ColonColon) => {
                    self.bump();
                    if !self.is(Punct::Lt) {
                        let span = self.span();
                        self.expected(
                            span,
                            "`<` after `::`",
                            "something else",
                            "write the type arguments, as in `::<Str, C>`",
                        );
                        self.errors.last_mut().unwrap().notes.push(
                            "explicit type arguments in an expression use the turbofish, \
                             `f::<Int>(x)`"
                                .into(),
                        );
                        return Err(Bail);
                    }
                    let args = self.type_args()?;
                    base = Expr::TurboFish {
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
        let mut alts = vec![first];
        while self.eat(Punct::Or) {
            alts.push(self.pattern_primary()?);
        }
        let span = alts[0].span().to(alts.last().unwrap().span());
        Ok(Pattern::Or { alts, span })
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
                    ).map(|d| d.code("rest-pattern-not-last"));
                    self.errors
                        .last_mut()
                        .unwrap()
                        .notes
                        .push("`[first, ..rest]` is legal; `[..init, last]` is not".into());
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
    fn turbofish() {
        ok("fn f(): Int { identity::<Int>(7) }");
        ok("fn f(): [Int] { list.empty::<Int>() }");
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
}
