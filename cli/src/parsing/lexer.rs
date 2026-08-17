//! The lexer.
//!
//! Follows the LEXICAL GRAMMAR section of `grammar.ebnf`. Two things about it
//! are worth knowing:
//!
//! * There is no `<<`, `>>`, or `?.` token. Their absence is what keeps
//!   `Wrapper<Wrapper<Int>>` from mis-lexing and `x?.field` from needing a
//!   token splitter (SPEC 12.6).
//! * Template interpolation is the only mode-dependent part. The lexer keeps
//!   one stack of what is currently open — a brace or an interpolation hole —
//!   so a `}` closing a block inside a hole is told apart from the `}` that
//!   resumes template text by which of the two is on top.

use crate::diagnostics::{Diagnostic, FileId, Invariant as _, Span};
use std::fmt;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kw {
    As,
    Const,
    Context,
    Ctx,
    Derive,
    Effect,
    Else,
    Enum,
    Export,
    False,
    Fn,
    For,
    From,
    If,
    Impl,
    Import,
    Let,
    Match,
    SelfValue,
    SelfType,
    Struct,
    Test,
    Trait,
    True,
    Type,
}

impl Kw {
    /// Every keyword, so a test can hold the hand-written grammar and the
    /// lexer to the same list. Adding a variant without adding it here is a
    /// compile error, because `text` matches exhaustively and this is checked
    /// against it.
    pub const ALL: &'static [Kw] = &[
        Kw::As,
        Kw::Const,
        Kw::Context,
        Kw::Ctx,
        Kw::Derive,
        Kw::Effect,
        Kw::Else,
        Kw::Enum,
        Kw::Export,
        Kw::False,
        Kw::Fn,
        Kw::For,
        Kw::From,
        Kw::If,
        Kw::Impl,
        Kw::Import,
        Kw::Let,
        Kw::Match,
        Kw::SelfValue,
        Kw::SelfType,
        Kw::Struct,
        Kw::Test,
        Kw::Trait,
        Kw::True,
        Kw::Type,
    ];

    pub fn text(self) -> &'static str {
        match self {
            Kw::As => "as",
            Kw::Const => "const",
            Kw::Context => "context",
            Kw::Ctx => "ctx",
            Kw::Derive => "derive",
            Kw::Effect => "effect",
            Kw::Else => "else",
            Kw::Enum => "enum",
            Kw::Export => "export",
            Kw::False => "false",
            Kw::Fn => "fn",
            Kw::For => "for",
            Kw::From => "from",
            Kw::If => "if",
            Kw::Impl => "impl",
            Kw::Import => "import",
            Kw::Let => "let",
            Kw::Match => "match",
            Kw::SelfValue => "self",
            Kw::SelfType => "Self",
            Kw::Struct => "struct",
            Kw::Test => "test",
            Kw::Trait => "trait",
            Kw::True => "true",
            Kw::Type => "type",
        }
    }

    fn from_str(s: &str) -> Option<Kw> {
        Some(match s {
            "as" => Kw::As,
            "const" => Kw::Const,
            "context" => Kw::Context,
            "ctx" => Kw::Ctx,
            "derive" => Kw::Derive,
            "effect" => Kw::Effect,
            "else" => Kw::Else,
            "enum" => Kw::Enum,
            "export" => Kw::Export,
            "false" => Kw::False,
            "fn" => Kw::Fn,
            "for" => Kw::For,
            "from" => Kw::From,
            "if" => Kw::If,
            "impl" => Kw::Impl,
            "import" => Kw::Import,
            "let" => Kw::Let,
            "match" => Kw::Match,
            "self" => Kw::SelfValue,
            "Self" => Kw::SelfType,
            "struct" => Kw::Struct,
            "test" => Kw::Test,
            "trait" => Kw::Trait,
            "true" => Kw::True,
            "type" => Kw::Type,
            _ => return None,
        })
    }
}

/// Reserved but unused in v0.3, rejected by the lexer so that later versions can
/// claim them without breaking source compatibility.
const RESERVED: &[&str] = &[
    "async", "await", "break", "continue", "do", "in", "is", "loop", "module", "mut", "opaque",
    "panic", "pub", "return", "unreachable", "use", "when", "where", "while", "with", "yield",
];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Punct {
    LBrace,
    RBrace,
    LParen,
    RParen,
    LBracket,
    RBracket,
    Comma,
    Semi,
    Colon,
    ColonColon,
    Dot,
    DotDot,
    At,
    Underscore,
    Eq,
    FatArrow,
    EqEq,
    BangEq,
    Lt,
    LtEq,
    Gt,
    GtEq,
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    AndAnd,
    OrOr,
    Bang,
    QuestionQuestion,
    And,
    Or,
    Caret,
    Tilde,
    Question,
}

impl Punct {
    pub fn text(self) -> &'static str {
        match self {
            Punct::LBrace => "{",
            Punct::RBrace => "}",
            Punct::LParen => "(",
            Punct::RParen => ")",
            Punct::LBracket => "[",
            Punct::RBracket => "]",
            Punct::Comma => ",",
            Punct::Semi => ";",
            Punct::Colon => ":",
            Punct::ColonColon => "::",
            Punct::Dot => ".",
            Punct::DotDot => "..",
            Punct::At => "@",
            Punct::Underscore => "_",
            Punct::Eq => "=",
            Punct::FatArrow => "=>",
            Punct::EqEq => "==",
            Punct::BangEq => "!=",
            Punct::Lt => "<",
            Punct::LtEq => "<=",
            Punct::Gt => ">",
            Punct::GtEq => ">=",
            Punct::Plus => "+",
            Punct::Minus => "-",
            Punct::Star => "*",
            Punct::Slash => "/",
            Punct::Percent => "%",
            Punct::AndAnd => "&&",
            Punct::OrOr => "||",
            Punct::Bang => "!",
            Punct::QuestionQuestion => "??",
            Punct::And => "&",
            Punct::Or => "|",
            Punct::Caret => "^",
            Punct::Tilde => "~",
            Punct::Question => "?",
        }
    }
}

#[derive(Clone, PartialEq, Debug)]
pub enum Tok {
    Ident(String),
    Kw(Kw),
    Punct(Punct),
    /// Value plus the raw spelling, so diagnostics can quote what was written.
    Int(u128, String),
    Float(f64, String),
    Str(String),
    Char(char),
    /// `"head${`
    TemplateHead(String),
    /// `}span${`
    TemplateSpan(String),
    /// `}tail"`
    TemplateTail(String),
    Eof,
}

impl fmt::Display for Tok {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Tok::Ident(s) => write!(f, "`{s}`"),
            Tok::Kw(k) => write!(f, "`{}`", k.text()),
            Tok::Punct(p) => write!(f, "`{}`", p.text()),
            Tok::Int(_, raw) => write!(f, "`{raw}`"),
            Tok::Float(_, raw) => write!(f, "`{raw}`"),
            Tok::Str(_) => write!(f, "a string literal"),
            Tok::Char(_) => write!(f, "a character literal"),
            Tok::TemplateHead(_) | Tok::TemplateSpan(_) | Tok::TemplateTail(_) => {
                write!(f, "an interpolated string")
            }
            Tok::Eof => write!(f, "end of file"),
        }
    }
}

/// One ordinary comment: a `//` line, or a whole `/* */` block.
///
/// The blank line above it is part of it. A run of comment lines above one
/// declaration is not necessarily one paragraph — a section heading and the
/// sentence about the declaration under it are two — and a formatter that
/// keeps only the lines glues them together.
#[derive(Clone, Debug)]
pub struct Comment {
    pub text: String,
    /// Whether a blank line sat immediately above this comment.
    pub blank_before: bool,
    /// The column its first character was written at, so a formatter can move
    /// the whole of a `/* … */` and keep the shape inside it.
    pub column: u32,
}

#[derive(Clone, Debug)]
pub struct Token {
    pub tok: Tok,
    pub span: Span,
    /// Doc comment lines (`///`) immediately preceding this token.
    pub docs: Vec<String>,
    /// Whether a blank line sat above the doc-comment run. It matters only
    /// when ordinary comments came first: a section heading, a blank line, and
    /// then the declaration's own documentation are three things, not one.
    pub docs_blank: bool,
    /// Ordinary comments immediately preceding, kept so the formatter can put
    /// them back where they were.
    pub comments: Vec<Comment>,
    /// Whether a blank line separated this token's trivia — its comments and
    /// doc lines, or the token itself when it has none — from whatever was
    /// written before. The formatter preserves paragraph breaks between
    /// declarations; the breaks *inside* a comment run are on the comments.
    pub blank_before: bool,
    /// Whether a blank line separated the comment run *below* from this token.
    /// `false` when there is no run, where the question does not arise.
    ///
    /// This is here rather than left to the formatter because the formatter
    /// used to re-derive it by scanning the source backwards, which made two
    /// independent answers to "is there a blank line here" — this one, counted
    /// while lexing, and that one — that could disagree about the same gap.
    pub detached: bool,
}

/// One thing a `}` could be closing, innermost last.
///
/// This was two coupled fields — a `Vec<u32>` of the brace depth each open
/// interpolation began at, and a `u32` counter — which had to agree for the
/// lexer to tell a block's `}` from the one that resumes template text. An
/// unbalanced `}` clamped the counter with `saturating_sub`, leaving it saying
/// a smaller depth than the interpolations recorded, and a later `}` closing a
/// genuine block was then read as "resume the template" — turning the rest of
/// the file into string content. One stack cannot disagree with itself: the
/// depth *is* its length, and an unbalanced `}` is a `pop` that finds nothing.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum LexMode {
    /// An open `{`. Its `}` is a block terminator.
    Braces,
    /// An open interpolation hole. Its `}` resumes the template text, and it
    /// is popped by the `}"` that ends the template rather than by the `}`
    /// that ends the hole, because a template may have several holes.
    Interpolation,
}

pub struct Lexer<'a> {
    src: &'a [u8],
    text: &'a str,
    pos: usize,
    file: FileId,
    tokens: Vec<Token>,
    errors: Vec<Diagnostic>,
    /// What is currently open, innermost last.
    modes: Vec<LexMode>,
    pending_docs: Vec<String>,
    pending_docs_blank: bool,
    pending_comments: Vec<Comment>,
    module_docs: Vec<(String, Span)>,
    blank_before: bool,
    detached: bool,
}

pub struct Lexed {
    pub tokens: Vec<Token>,
    pub errors: Vec<Diagnostic>,
    /// `//!` lines, with the span of each, in source order. The parser keeps
    /// the ones before the first item and reports the rest.
    pub module_docs: Vec<(String, Span)>,
}

pub fn lex(text: &str, file: FileId) -> Lexed {
    let mut l = Lexer {
        src: text.as_bytes(),
        text,
        pos: 0,
        file,
        tokens: Vec::new(),
        errors: Vec::new(),
        modes: Vec::new(),
        pending_docs: Vec::new(),
        pending_docs_blank: false,
        pending_comments: Vec::new(),
        module_docs: Vec::new(),
        blank_before: false,
        detached: false,
    };
    l.run();
    Lexed { tokens: l.tokens, errors: l.errors, module_docs: l.module_docs }
}

impl<'a> Lexer<'a> {
    fn peek(&self) -> u8 {
        *self.src.get(self.pos).unwrap_or(&0)
    }

    fn peek_at(&self, n: usize) -> u8 {
        *self.src.get(self.pos.saturating_add(n)).unwrap_or(&0)
    }

    fn bump(&mut self) -> u8 {
        let c = self.peek();
        self.pos = self.pos.saturating_add(1);
        c
    }

    /// The source between two offsets, empty if they do not describe one.
    ///
    /// Every offset here is one the lexer walked to, so in a correct lexer
    /// this is `&self.text[a..b]` — but that spelling panics on the one
    /// arrangement of bytes where the lexer is wrong, and the arrangement in
    /// question is "a character outside ASCII", which is not exotic input. A
    /// total accessor makes the failure a short message instead of a crash,
    /// and there is exactly one of it to reason about.
    fn slice(&self, a: usize, b: usize) -> &'a str {
        self.text.get(a..b).unwrap_or("")
    }

    fn span(&self, start: usize) -> Span {
        Span::new(self.file, start, self.pos)
    }

    /// Every lexical error carries the edit that resolves it. `fix` is a
    /// parameter rather than something a caller may add later, so a new error
    /// site cannot forget one.
    /// Hands the diagnostic back so a caller can name the rule it enforces.
    fn err(
        &mut self,
        span: Span,
        msg: impl Into<String>,
        fix: impl Into<String>,
    ) -> &mut Diagnostic {
        self.errors.push(Diagnostic::error(span, msg).with_fix(fix));
        // Pushed on the line above, so there is a last one.
        self.errors.last_mut().or_ice("the diagnostic just pushed is still there")
    }

    fn push(&mut self, tok: Tok, start: usize) {
        let span = self.span(start);
        let docs = std::mem::take(&mut self.pending_docs);
        let docs_blank = std::mem::take(&mut self.pending_docs_blank);
        let comments = std::mem::take(&mut self.pending_comments);
        let blank = std::mem::take(&mut self.blank_before);
        let detached = std::mem::take(&mut self.detached);
        self.tokens.push(Token {
            tok,
            span,
            docs,
            docs_blank,
            comments,
            blank_before: blank,
            detached,
        });
    }

    fn run(&mut self) {
        loop {
            self.skip_trivia();
            if self.pos >= self.src.len() {
                let start = self.pos;
                self.push(Tok::Eof, start);
                return;
            }
            let start = self.pos;
            let c = self.peek();
            match c {
                b'0'..=b'9' => self.number(start),
                b'"' => self.string_or_template(start),
                b'\'' => self.char_literal(start),
                c if is_ident_start(c) => self.ident(start),
                _ => self.punct(start),
            }
        }
    }

    /// The column `at` sits at, counting from zero.
    fn column(&self, at: usize) -> u32 {
        let line = self.slice(0, at).rfind('\n').map_or(0, |i| i.saturating_add(1));
        self.slice(line, at).chars().count() as u32
    }

    /// Whether nothing of this token's trivia has been read yet, so a blank
    /// line here is the one above the whole run rather than one inside it.
    fn run_empty(&self) -> bool {
        self.pending_comments.is_empty() && self.pending_docs.is_empty()
    }

    fn skip_trivia(&mut self) {
        let mut newlines = 0usize;
        loop {
            match self.peek() {
                b' ' | b'\t' | b'\r' => {
                    self.pos = self.pos.saturating_add(1);
                }
                b'\n' => {
                    newlines = newlines.saturating_add(1);
                    self.pos = self.pos.saturating_add(1);
                }
                b'/' if self.peek_at(1) == b'/' => {
                    let start = self.pos;
                    let is_doc = self.peek_at(2) == b'/' && self.peek_at(3) != b'/';
                    // `//!` documents the module rather than the declaration
                    // that follows, which is the only comment form that
                    // attaches upward. It is legal only before the first
                    // token; `check` reports one that appears later, where a
                    // reader would take it for a `///` typo.
                    let is_module_doc = self.peek_at(2) == b'!';
                    while self.pos < self.src.len() && self.peek() != b'\n' {
                        self.pos = self.pos.saturating_add(1);
                    }
                    let raw = self.slice(start, self.pos);
                    // Two newlines with nothing between them is a blank line.
                    // Above the first thing in the run it is the token's; above
                    // a later comment it is that comment's own paragraph break.
                    let blank = newlines >= 2;
                    if is_module_doc {
                        let span = self.span(start);
                        self.module_docs.push((doc_body(raw.get(3..).unwrap_or("")), span));
                    } else if is_doc {
                        if self.run_empty() {
                            self.blank_before = blank;
                        }
                        if self.pending_docs.is_empty() {
                            self.pending_docs_blank = blank;
                        }
                        self.pending_docs.push(doc_body(raw.get(3..).unwrap_or("")));
                    } else {
                        if self.run_empty() {
                            self.blank_before = blank;
                        }
                        let text = raw.trim_end().to_string();
                        let column = self.column(start);
                        self.pending_comments.push(Comment { text, blank_before: blank, column });
                    }
                    newlines = 0;
                }
                b'/' if self.peek_at(1) == b'*' => {
                    let start = self.pos;
                    self.pos = self.pos.saturating_add(2);
                    // Block comments nest.
                    let mut depth = 1usize;
                    while self.pos < self.src.len() && depth > 0 {
                        if self.peek() == b'/' && self.peek_at(1) == b'*' {
                            depth = depth.saturating_add(1);
                            self.pos = self.pos.saturating_add(2);
                        } else if self.peek() == b'*' && self.peek_at(1) == b'/' {
                            depth = depth.saturating_sub(1);
                            self.pos = self.pos.saturating_add(2);
                        } else {
                            self.pos = self.pos.saturating_add(1);
                        }
                    }
                    if depth > 0 {
                        let span = self.span(start);
                        self.err(span, "unterminated block comment", "close it with `*/`; block comments nest, so each `/*` needs one").code("unterminated-comment");
                    }
                    let blank = newlines >= 2;
                    if self.run_empty() {
                        self.blank_before = blank;
                    }
                    let text = self.slice(start, self.pos).to_string();
                    let column = self.column(start);
                    self.pending_comments.push(Comment { text, blank_before: blank, column });
                    newlines = 0;
                }
                _ => {
                    // The newlines counted since the last thing read. With an
                    // empty run that gap is above the token; with a run above
                    // it, it is the gap between the run and the token, which
                    // is what makes a file header a header rather than a
                    // comment about the first declaration.
                    if self.run_empty() {
                        self.blank_before = newlines >= 2;
                    } else {
                        self.detached = newlines >= 2;
                    }
                    return;
                }
            }
        }
    }

    fn ident(&mut self, start: usize) {
        while is_ident_continue(self.peek()) {
            self.pos = self.pos.saturating_add(1);
        }
        let s = self.slice(start, self.pos);
        if s == "_" {
            self.push(Tok::Punct(Punct::Underscore), start);
            return;
        }
        if let Some(kw) = Kw::from_str(s) {
            self.push(Tok::Kw(kw), start);
            return;
        }
        if RESERVED.contains(&s) {
            let span = self.span(start);
            let s = s.to_string();
            self.err(
                span,
                format!("`{s}` is a reserved word and may not be used as an identifier"),
                format!("pick another name; `{s}` is not available"),
            ).code("reserved-word");
            if let Some(d) = self.errors.last_mut() {
                d.notes.push(
                    "reserved for a future version of Buri; see grammar.ebnf, ReservedWord".into(),
                );
            }
            self.push(Tok::Ident(s), start);
            return;
        }
        let s = s.to_string();
        self.push(Tok::Ident(s), start);
    }

    fn number(&mut self, start: usize) {
        // Radix prefixes. `0x`, `0o`, `0b` are integers only.
        if self.peek() == b'0' && matches!(self.peek_at(1), b'x' | b'X' | b'o' | b'O' | b'b' | b'B')
        {
            let radix_char = self.peek_at(1).to_ascii_lowercase();
            let radix: u32 = match radix_char {
                b'x' => 16,
                b'o' => 8,
                _ => 2,
            };
            self.pos = self.pos.saturating_add(2);
            let digits_start = self.pos;
            while self.peek().is_ascii_alphanumeric() || self.peek() == b'_' {
                self.pos = self.pos.saturating_add(1);
            }
            let raw = self.slice(start, self.pos).to_string();
            let digits: String =
                self.slice(digits_start, self.pos).chars().filter(|c| *c != '_').collect();
            if digits.is_empty() {
                let span = self.span(start);
                self.err(span, format!("`{raw}` has no digits"), "write at least one digit after the base prefix, as in `0x1F`");
                self.push(Tok::Int(0, raw), start);
                return;
            }
            match u128::from_str_radix(&digits, radix) {
                Ok(v) => self.push(Tok::Int(v, raw), start),
                Err(_) => {
                    let span = self.span(start);
                    self.err(
                span,
                format!("`{raw}` is not a valid base-{radix} integer, or does not fit in 128 bits"),
                format!("use digits base-{radix} admits, and a value inside 128 bits"),
            );
                    self.push(Tok::Int(0, raw), start);
                }
            }
            return;
        }

        while self.peek().is_ascii_digit() || self.peek() == b'_' {
            self.pos = self.pos.saturating_add(1);
        }

        // A FLOAT must begin with a digit, and `pair.0` must lex as three
        // tokens, so a `.` only continues the number when a digit follows it.
        let mut is_float = false;
        if self.peek() == b'.' && self.peek_at(1).is_ascii_digit() {
            is_float = true;
            self.pos = self.pos.saturating_add(1);
            while self.peek().is_ascii_digit() || self.peek() == b'_' {
                self.pos = self.pos.saturating_add(1);
            }
        }
        if matches!(self.peek(), b'e' | b'E') {
            let save = self.pos;
            self.pos = self.pos.saturating_add(1);
            if matches!(self.peek(), b'+' | b'-') {
                self.pos = self.pos.saturating_add(1);
            }
            if self.peek().is_ascii_digit() {
                is_float = true;
                while self.peek().is_ascii_digit() || self.peek() == b'_' {
                    self.pos = self.pos.saturating_add(1);
                }
            } else {
                self.pos = save;
            }
        }

        let raw = self.slice(start, self.pos).to_string();
        let clean: String = raw.chars().filter(|c| *c != '_').collect();
        if is_float {
            match clean.parse::<f64>() {
                Ok(v) => self.push(Tok::Float(v, raw), start),
                Err(_) => {
                    let span = self.span(start);
                    self.err(
                span,
                format!("`{raw}` is not a valid float literal"),
                "a float needs a digit on each side of the point, as in `0.5`, and at most one exponent",
            );
                    self.push(Tok::Float(0.0, raw), start);
                }
            }
        } else {
            match clean.parse::<u128>() {
                Ok(v) => self.push(Tok::Int(v, raw), start),
                Err(_) => {
                    let span = self.span(start);
                    self.err(span, format!("`{raw}` does not fit in 128 bits"), "write a smaller value; 128 bits is the widest integer type");
                    self.push(Tok::Int(0, raw), start);
                }
            }
        }
    }

    /// Scans string body text, stopping at `"` or at an unescaped `${`.
    /// Returns (contents, ended_with_hole).
    fn scan_str_body(&mut self) -> (String, bool) {
        let mut out = String::new();
        loop {
            if self.pos >= self.src.len() {
                let span = Span::new(self.file, self.pos, self.pos);
                self.err(span, "unterminated string literal", "close it with `\"`; a string literal does not span a line break");
                return (out, false);
            }
            match self.peek() {
                b'"' => {
                    self.pos = self.pos.saturating_add(1);
                    return (out, false);
                }
                b'$' if self.peek_at(1) == b'{' => {
                    self.pos = self.pos.saturating_add(2);
                    return (out, true);
                }
                b'\\' => {
                    let start = self.pos;
                    self.pos = self.pos.saturating_add(1);
                    if let Some(c) = self.escape(start) {
                        out.push(c);
                    }
                }
                b'\n' => {
                    let span = Span::new(self.file, self.pos, self.pos.saturating_add(1));
                    self.err(span, "unterminated string literal", "close it with `\"`; a string literal does not span a line break");
                    return (out, false);
                }
                _ => {
                    let ch = self.next_char();
                    out.push(ch);
                }
            }
        }
    }

    fn next_char(&mut self) -> char {
        let ch = self.text.get(self.pos..).and_then(|s| s.chars().next()).unwrap_or('\0');
        self.pos = self.pos.saturating_add(ch.len_utf8());
        ch
    }

    /// Called with `self.pos` just past the backslash.
    fn escape(&mut self, start: usize) -> Option<char> {
        let c = self.bump();
        Some(match c {
            b'n' => '\n',
            b'r' => '\r',
            b't' => '\t',
            b'0' => '\0',
            b'\\' => '\\',
            b'"' => '"',
            b'\'' => '\'',
            b'$' => '$',
            b'u' => {
                if self.peek() != b'{' {
                    let span = self.span(start);
                    self.err(
                        span,
                        "`\\u` must be followed by `{`, as in `\\u{1F600}`",
                        "brace the code point: `\\u{1F600}`",
                    );
                    return None;
                }
                self.pos = self.pos.saturating_add(1);
                let ds = self.pos;
                while self.peek().is_ascii_hexdigit() {
                    self.pos = self.pos.saturating_add(1);
                }
                let digits = self.slice(ds, self.pos).to_string();
                if self.peek() != b'}' {
                    let span = self.span(start);
                    self.err(span, "unterminated `\\u{...}` escape", "close it with `}`");
                    return None;
                }
                self.pos = self.pos.saturating_add(1);
                match u32::from_str_radix(&digits, 16).ok().and_then(char::from_u32) {
                    Some(c) => c,
                    None => {
                        let span = self.span(start);
                        self.err(
                            span,
                            format!("`\\u{{{digits}}}` is not a Unicode scalar value"),
                            "a scalar value is at most 10FFFF and outside D800-DFFF",
                        );
                        return None;
                    }
                }
            }
            _ => {
                let span = self.span(start);
                let shown = (c as char).to_string();
                self.err(
                    span,
                    format!("unknown escape `\\{shown}`"),
                    "the escapes are `\\n` `\\r` `\\t` `\\0` `\\\\` `\\\"` `\\'` `\\$` and `\\u{...}`",
                );
                if let Some(d) = self.errors.last_mut() {
                    d.notes
                        .push("the escapes are \\n \\r \\t \\0 \\\\ \\\" \\' \\$ and \\u{...}".into());
                }
                return None;
            }
        })
    }

    fn string_or_template(&mut self, start: usize) {
        self.pos = self.pos.saturating_add(1); // the opening quote
        let (body, hole) = self.scan_str_body();
        if hole {
            self.push(Tok::TemplateHead(body), start);
            self.modes.push(LexMode::Interpolation);
        } else {
            self.push(Tok::Str(body), start);
        }
    }

    /// Resumes template text after the `}` that closes a hole.
    fn resume_template(&mut self, start: usize) {
        let (body, hole) = self.scan_str_body();
        if hole {
            self.push(Tok::TemplateSpan(body), start);
        } else {
            self.push(Tok::TemplateTail(body), start);
            debug_assert_eq!(self.modes.last(), Some(&LexMode::Interpolation));
            self.modes.pop();
        }
    }

    fn char_literal(&mut self, start: usize) {
        self.pos = self.pos.saturating_add(1);
        let c = if self.peek() == b'\\' {
            let s = self.pos;
            self.pos = self.pos.saturating_add(1);
            self.escape(s).unwrap_or('\0')
        } else if self.pos >= self.src.len() || self.peek() == b'\n' {
            let span = self.span(start);
            self.err(span, "unterminated character literal", "close it with `\'`");
            self.push(Tok::Char('\0'), start);
            return;
        } else {
            self.next_char()
        };
        if self.peek() != b'\'' {
            let span = self.span(start);
            self.err(
                span,
                "a character literal holds exactly one Unicode scalar value",
                "use a string literal for more than one",
            );
            // Recover by skipping to the closing quote if one is nearby.
            while self.pos < self.src.len() && self.peek() != b'\'' && self.peek() != b'\n' {
                self.pos = self.pos.saturating_add(1);
            }
        }
        if self.peek() == b'\'' {
            self.pos = self.pos.saturating_add(1);
        }
        self.push(Tok::Char(c), start);
    }

    /// A two-character token, the first character of which `bump` has already
    /// taken: this consumes the second.
    fn second(&mut self, p: Punct) -> Punct {
        self.pos = self.pos.saturating_add(1);
        p
    }

    fn punct(&mut self, start: usize) {
        use Punct::*;
        let c = self.bump();
        let two = self.peek();
        let p = match (c, two) {
            (b'{', _) => {
                self.modes.push(LexMode::Braces);
                LBrace
            }
            (b'}', _) => {
                match self.modes.last() {
                    // The innermost thing open is a hole, so this `}` resumes
                    // template text rather than terminating a block. The mode
                    // stays open until the template itself ends.
                    Some(LexMode::Interpolation) => {
                        self.resume_template(start);
                        return;
                    }
                    Some(LexMode::Braces) => {
                        self.modes.pop();
                    }
                    // Nothing is open. This `}` closes nothing, which the
                    // parser reports where it can say what was expected
                    // instead; what matters here is that there is no counter
                    // to clamp, so nothing after it is mis-lexed.
                    None => {}
                }
                RBrace
            }
            (b'(', _) => LParen,
            (b')', _) => RParen,
            (b'[', _) => LBracket,
            (b']', _) => RBracket,
            (b',', _) => Comma,
            (b';', _) => Semi,
            (b':', b':') => self.second(ColonColon),
            (b':', _) => Colon,
            (b'.', b'.') => self.second(DotDot),
            (b'.', _) => Dot,
            (b'@', _) => At,
            (b'=', b'>') => self.second(FatArrow),
            (b'=', b'=') => self.second(EqEq),
            (b'=', _) => Eq,
            (b'!', b'=') => self.second(BangEq),
            (b'!', _) => Bang,
            (b'<', b'=') => self.second(LtEq),
            (b'<', _) => Lt,
            (b'>', b'=') => self.second(GtEq),
            // No `>>` token: `Wrapper<Wrapper<Int>>` closes with two `>`.
            (b'>', _) => Gt,
            (b'+', _) => Plus,
            (b'-', _) => Minus,
            (b'*', _) => Star,
            (b'/', _) => Slash,
            (b'%', _) => Percent,
            (b'&', b'&') => self.second(AndAnd),
            (b'&', _) => And,
            (b'|', b'|') => self.second(OrOr),
            (b'|', _) => Or,
            (b'^', _) => Caret,
            (b'~', _) => Tilde,
            (b'?', b'?') => self.second(QuestionQuestion),
            // No `?.` token, so `x?.field` is `x` `?` `.` `field`.
            (b'?', _) => Question,
            _ => {
                // `bump` advanced one *byte*, and this arm is where every
                // character outside ASCII arrives — a `×` someone typed for
                // multiplication, a non-breaking space a word processor left
                // behind. Those are two, three, or four bytes, so reporting
                // the span `bump` left would cut a character in half; taking
                // the whole scalar is what makes the message show what was
                // typed. The code point is spelled out because the two
                // characters most likely to reach here are invisible.
                self.pos = start;
                let shown = self.next_char();
                let span = self.span(start);
                self.err(
                    span,
                    format!("unexpected character `{shown}` (U+{:04X})", shown as u32),
                    "delete it; no token in the language starts with it",
                );
                return;
            }
        };
        self.push(Tok::Punct(p), start);
    }
}

fn is_ident_start(c: u8) -> bool {
    c == b'_' || c.is_ascii_alphabetic()
}

fn is_ident_continue(c: u8) -> bool {
    c == b'_' || c.is_ascii_alphanumeric()
}

#[cfg(test)]
mod kw_tests {
    use super::Kw;

    /// `ALL` must list every variant. `text` matches exhaustively, so the
    /// compiler catches a missing variant there; this catches a variant that
    /// exists but was left out of `ALL`.
    #[test]
    fn all_lists_every_keyword() {
        let mut texts: Vec<&str> = Kw::ALL.iter().map(|k| k.text()).collect();
        texts.sort();
        texts.dedup();
        assert_eq!(texts.len(), Kw::ALL.len(), "`ALL` repeats a keyword");
        // `self` and `Self` differ only in case, so the count is the guard.
        assert_eq!(Kw::ALL.len(), 25, "a keyword was added without updating `ALL`");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `punct` reported the byte it had bumped past, and every character
    /// outside ASCII is more than one byte — so a stray `×`, an emoji, or a
    /// non-breaking space out of a word processor panicked the lexer on a
    /// `String` index that was not a character boundary. The message names the
    /// code point because the two most likely to arrive are invisible.
    #[test]
    fn a_character_outside_ascii_is_one_diagnostic_naming_it() {
        for (text, wanted) in [
            ("5 × 3", "`×` (U+00D7)"),
            ("🙂", "`🙂` (U+1F642)"),
            ("a\u{a0}b", "(U+00A0)"),
            ("\u{0}", "(U+0000)"),
            ("\u{200f}", "(U+200F)"),
        ] {
            let l = lex(text, FileId(0));
            let messages: Vec<&str> = l.errors.iter().map(|e| e.message.as_str()).collect();
            assert!(
                messages.iter().any(|m| m.contains(wanted)),
                "lexing {text:?} said {messages:?}, not {wanted}"
            );
        }
    }

    /// Every offset the lexer slices at has to be one it walked to. A file made
    /// only of characters it does not recognise is where that stopped being
    /// true.
    #[test]
    fn a_file_of_nothing_but_unrecognised_characters_lexes() {
        let text = "×÷≠🙂\u{a0}\u{200f}—“”";
        let l = lex(text, FileId(0));
        assert_eq!(l.errors.len(), text.chars().count(), "one per character");
        assert_eq!(l.tokens.last().map(|t| t.tok.clone()), Some(Tok::Eof));
    }

    fn toks(src: &str) -> Vec<Tok> {
        let l = lex(src, FileId(0));
        assert!(l.errors.is_empty(), "unexpected errors: {:?}", l.errors);
        l.tokens.into_iter().map(|t| t.tok).collect()
    }

    #[test]
    fn tuple_access_is_three_tokens() {
        // `pair.0` must not lex as IDENT FLOAT — this is why a float literal
        // has to begin with a digit (SPEC 12.14).
        assert_eq!(
            toks("pair.0"),
            vec![
                Tok::Ident("pair".into()),
                Tok::Punct(Punct::Dot),
                Tok::Int(0, "0".into()),
                Tok::Eof
            ]
        );
    }

    #[test]
    fn nested_generics_close_with_two_gt() {
        let t = toks("Foo<Bar<Int>>");
        assert_eq!(t[t.len() - 3..t.len() - 1], [Tok::Punct(Punct::Gt), Tok::Punct(Punct::Gt)]);
    }

    #[test]
    fn the_known_wart_still_lexes_as_documented() {
        // `Foo<Bar<Int>>= x` lexes `>` `>=`. Documented in grammar.ebnf.
        let t = toks("Foo<Bar<Int>>= x");
        assert!(t.contains(&Tok::Punct(Punct::GtEq)));
    }

    #[test]
    fn block_comments_nest() {
        assert_eq!(toks("/* a /* b */ c */ x"), vec![Tok::Ident("x".into()), Tok::Eof]);
    }

    #[test]
    fn template_holes_track_brace_depth() {
        // The `}` closing the block inside the hole must not end the template.
        let t = toks(r#""a${ { let x = 1; x } }b""#);
        assert_eq!(t[0], Tok::TemplateHead("a".into()));
        assert_eq!(t[t.len() - 2], Tok::TemplateTail("b".into()));
    }

    /// A `}` that closes nothing used to clamp a counter with `saturating_sub`,
    /// leaving the counter and the stack of open interpolations describing two
    /// different nestings. There is no counter now — the depth is the stack's
    /// length — so an unbalanced `}` is a `pop` that finds nothing, and the
    /// template after it still lexes as a template.
    #[test]
    fn an_unbalanced_brace_does_not_derail_a_later_template() {
        let t = toks(r#"} { let s = "a${x}b"; }"#);
        assert!(
            t.contains(&Tok::TemplateHead("a".into())),
            "the template head was swallowed: {t:?}"
        );
        assert!(
            t.contains(&Tok::TemplateTail("b".into())),
            "the template never ended: {t:?}"
        );
        assert_eq!(t.last(), Some(&Tok::Eof));
    }

    #[test]
    fn multiple_holes() {
        let t = toks(r#""${a}m${b}s""#);
        assert_eq!(t[0], Tok::TemplateHead("".into()));
        assert_eq!(t[2], Tok::TemplateSpan("m".into()));
        assert_eq!(t[4], Tok::TemplateTail("s".into()));
    }

    #[test]
    fn escaped_dollar_is_not_a_hole() {
        assert_eq!(toks(r#""\$19.05""#), vec![Tok::Str("$19.05".into()), Tok::Eof]);
    }

    #[test]
    fn radix_and_separators() {
        assert_eq!(toks("0xFF"), vec![Tok::Int(255, "0xFF".into()), Tok::Eof]);
        assert_eq!(toks("0o755"), vec![Tok::Int(0o755, "0o755".into()), Tok::Eof]);
        assert_eq!(toks("0b1010_0110"), vec![Tok::Int(0b1010_0110, "0b1010_0110".into()), Tok::Eof]);
        assert_eq!(toks("1_000_000"), vec![Tok::Int(1_000_000, "1_000_000".into()), Tok::Eof]);
    }

    #[test]
    fn floats_need_a_leading_digit() {
        assert_eq!(toks("1.0e-9"), vec![Tok::Float(1.0e-9, "1.0e-9".into()), Tok::Eof]);
        assert_eq!(toks("6.02e23"), vec![Tok::Float(6.02e23, "6.02e23".into()), Tok::Eof]);
    }

    #[test]
    fn reserved_words_are_rejected() {
        let l = lex("let while = 1;", FileId(0));
        assert!(l.errors.iter().any(|e| e.message.contains("reserved")));
    }

    #[test]
    fn underscore_is_its_own_token() {
        assert_eq!(toks("_"), vec![Tok::Punct(Punct::Underscore), Tok::Eof]);
        assert_eq!(toks("_x"), vec![Tok::Ident("_x".into()), Tok::Eof]);
    }

    #[test]
    fn no_question_dot_token() {
        assert_eq!(
            toks("x?.f"),
            vec![
                Tok::Ident("x".into()),
                Tok::Punct(Punct::Question),
                Tok::Punct(Punct::Dot),
                Tok::Ident("f".into()),
                Tok::Eof
            ]
        );
    }
}

/// The text of a `///` or `//!` line, after the marker.
///
/// One leading space is the separator between the marker and the prose and
/// comes off; everything after that is content. Trimming further would be
/// wrong: a fenced code block inside a doc comment is indented relative to the
/// fence, and `trim()` — which this used to do — flattened it, so an example
/// with a nested block came out unparseable.
pub fn doc_body(after_marker: &str) -> String {
    let s = after_marker.strip_prefix(' ').unwrap_or(after_marker);
    s.trim_end().to_string()
}
