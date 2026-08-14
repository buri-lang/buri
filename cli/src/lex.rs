//! The lexer.
//!
//! Follows the LEXICAL GRAMMAR section of `grammar.ebnf`. Two things about it
//! are worth knowing:
//!
//! * There is no `<<`, `>>`, or `?.` token. Their absence is what keeps
//!   `Wrapper<Wrapper<Int>>` from mis-lexing and `x?.field` from needing a
//!   token splitter (SPEC 12.6).
//! * Template interpolation is the only mode-dependent part. The lexer keeps a
//!   stack of open interpolations so a `}` closing a block inside a hole is
//!   told apart from the `}` that resumes template text.

use crate::diag::{Diagnostic, FileId, Span};
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

#[derive(Clone, Debug)]
pub struct Token {
    pub tok: Tok,
    pub span: Span,
    /// Doc comment lines (`///`) immediately preceding this token.
    pub docs: Vec<String>,
    /// Ordinary comments immediately preceding, kept so the formatter can put
    /// them back where they were.
    pub comments: Vec<String>,
    /// Whether a blank line separated this token from the previous one. The
    /// formatter preserves paragraph breaks between declarations.
    pub blank_before: bool,
}

pub struct Lexer<'a> {
    src: &'a [u8],
    text: &'a str,
    pos: usize,
    file: FileId,
    tokens: Vec<Token>,
    errors: Vec<Diagnostic>,
    /// Brace depth at the point each open interpolation began.
    interp: Vec<u32>,
    brace_depth: u32,
    pending_docs: Vec<String>,
    pending_comments: Vec<String>,
    blank_before: bool,
}

pub struct Lexed {
    pub tokens: Vec<Token>,
    pub errors: Vec<Diagnostic>,
}

pub fn lex(text: &str, file: FileId) -> Lexed {
    let mut l = Lexer {
        src: text.as_bytes(),
        text,
        pos: 0,
        file,
        tokens: Vec::new(),
        errors: Vec::new(),
        interp: Vec::new(),
        brace_depth: 0,
        pending_docs: Vec::new(),
        pending_comments: Vec::new(),
        blank_before: false,
    };
    l.run();
    Lexed { tokens: l.tokens, errors: l.errors }
}

impl<'a> Lexer<'a> {
    fn peek(&self) -> u8 {
        *self.src.get(self.pos).unwrap_or(&0)
    }

    fn peek_at(&self, n: usize) -> u8 {
        *self.src.get(self.pos + n).unwrap_or(&0)
    }

    fn bump(&mut self) -> u8 {
        let c = self.peek();
        self.pos += 1;
        c
    }

    fn span(&self, start: usize) -> Span {
        Span::new(self.file, start, self.pos)
    }

    /// Every lexical error carries the edit that resolves it. `fix` is a
    /// parameter rather than something a caller may add later, so a new error
    /// site cannot forget one.
    fn err(&mut self, span: Span, msg: impl Into<String>, fix: impl Into<String>) {
        self.errors.push(Diagnostic::error(span, msg).with_fix(fix));
    }

    fn push(&mut self, tok: Tok, start: usize) {
        let span = self.span(start);
        let docs = std::mem::take(&mut self.pending_docs);
        let comments = std::mem::take(&mut self.pending_comments);
        let blank = std::mem::take(&mut self.blank_before);
        self.tokens.push(Token { tok, span, docs, comments, blank_before: blank });
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

    fn skip_trivia(&mut self) {
        let mut newlines = 0;
        loop {
            match self.peek() {
                b' ' | b'\t' | b'\r' => {
                    self.pos += 1;
                }
                b'\n' => {
                    newlines += 1;
                    // Two newlines with nothing between them is a blank line.
                    if newlines >= 2 {
                        self.blank_before = true;
                    }
                    self.pos += 1;
                }
                b'/' if self.peek_at(1) == b'/' => {
                    let start = self.pos;
                    let is_doc = self.peek_at(2) == b'/' && self.peek_at(3) != b'/';
                    while self.pos < self.src.len() && self.peek() != b'\n' {
                        self.pos += 1;
                    }
                    let raw = &self.text[start..self.pos];
                    if is_doc {
                        self.pending_docs.push(raw[3..].trim().to_string());
                    } else {
                        self.pending_comments.push(raw.trim_end().to_string());
                    }
                    newlines = 0;
                }
                b'/' if self.peek_at(1) == b'*' => {
                    let start = self.pos;
                    self.pos += 2;
                    // Block comments nest.
                    let mut depth = 1usize;
                    while self.pos < self.src.len() && depth > 0 {
                        if self.peek() == b'/' && self.peek_at(1) == b'*' {
                            depth += 1;
                            self.pos += 2;
                        } else if self.peek() == b'*' && self.peek_at(1) == b'/' {
                            depth -= 1;
                            self.pos += 2;
                        } else {
                            self.pos += 1;
                        }
                    }
                    if depth > 0 {
                        let span = self.span(start);
                        self.err(span, "unterminated block comment", "close it with `*/`; block comments nest, so each `/*` needs one");
                    }
                    let raw = self.text[start..self.pos].to_string();
                    self.pending_comments.push(raw);
                    newlines = 0;
                }
                _ => return,
            }
        }
    }

    fn ident(&mut self, start: usize) {
        while is_ident_continue(self.peek()) {
            self.pos += 1;
        }
        let s = &self.text[start..self.pos];
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
            );
            self.errors.last_mut().unwrap().notes.push(
                "reserved for a future version of Buri; see grammar.ebnf, ReservedWord".into(),
            );
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
            self.pos += 2;
            let digits_start = self.pos;
            while self.peek().is_ascii_alphanumeric() || self.peek() == b'_' {
                self.pos += 1;
            }
            let raw = self.text[start..self.pos].to_string();
            let digits: String =
                self.text[digits_start..self.pos].chars().filter(|c| *c != '_').collect();
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
            self.pos += 1;
        }

        // A FLOAT must begin with a digit, and `pair.0` must lex as three
        // tokens, so a `.` only continues the number when a digit follows it.
        let mut is_float = false;
        if self.peek() == b'.' && self.peek_at(1).is_ascii_digit() {
            is_float = true;
            self.pos += 1;
            while self.peek().is_ascii_digit() || self.peek() == b'_' {
                self.pos += 1;
            }
        }
        if matches!(self.peek(), b'e' | b'E') {
            let save = self.pos;
            self.pos += 1;
            if matches!(self.peek(), b'+' | b'-') {
                self.pos += 1;
            }
            if self.peek().is_ascii_digit() {
                is_float = true;
                while self.peek().is_ascii_digit() || self.peek() == b'_' {
                    self.pos += 1;
                }
            } else {
                self.pos = save;
            }
        }

        let raw = self.text[start..self.pos].to_string();
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
                    self.pos += 1;
                    return (out, false);
                }
                b'$' if self.peek_at(1) == b'{' => {
                    self.pos += 2;
                    return (out, true);
                }
                b'\\' => {
                    let start = self.pos;
                    self.pos += 1;
                    match self.escape(start) {
                        Some(c) => out.push(c),
                        None => {}
                    }
                }
                b'\n' => {
                    let span = Span::new(self.file, self.pos, self.pos + 1);
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
        let ch = self.text[self.pos..].chars().next().unwrap_or('\0');
        self.pos += ch.len_utf8();
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
                self.pos += 1;
                let ds = self.pos;
                while self.peek().is_ascii_hexdigit() {
                    self.pos += 1;
                }
                let digits = self.text[ds..self.pos].to_string();
                if self.peek() != b'}' {
                    let span = self.span(start);
                    self.err(span, "unterminated `\\u{...}` escape", "close it with `}`");
                    return None;
                }
                self.pos += 1;
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
                self.errors.last_mut().unwrap().notes.push(
                    "the escapes are \\n \\r \\t \\0 \\\\ \\\" \\' \\$ and \\u{...}".into(),
                );
                return None;
            }
        })
    }

    fn string_or_template(&mut self, start: usize) {
        self.pos += 1; // the opening quote
        let (body, hole) = self.scan_str_body();
        if hole {
            self.push(Tok::TemplateHead(body), start);
            self.interp.push(self.brace_depth);
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
            self.interp.pop();
        }
    }

    fn char_literal(&mut self, start: usize) {
        self.pos += 1;
        let c = if self.peek() == b'\\' {
            let s = self.pos;
            self.pos += 1;
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
                self.pos += 1;
            }
        }
        if self.peek() == b'\'' {
            self.pos += 1;
        }
        self.push(Tok::Char(c), start);
    }

    fn punct(&mut self, start: usize) {
        use Punct::*;
        let c = self.bump();
        let two = self.peek();
        let p = match (c, two) {
            (b'{', _) => {
                self.brace_depth += 1;
                LBrace
            }
            (b'}', _) => {
                // A `}` at the brace depth an interpolation started at is the
                // one that resumes template text, not a block terminator.
                if self.interp.last() == Some(&self.brace_depth) {
                    self.resume_template(start);
                    return;
                }
                self.brace_depth = self.brace_depth.saturating_sub(1);
                RBrace
            }
            (b'(', _) => LParen,
            (b')', _) => RParen,
            (b'[', _) => LBracket,
            (b']', _) => RBracket,
            (b',', _) => Comma,
            (b';', _) => Semi,
            (b':', b':') => {
                self.pos += 1;
                ColonColon
            }
            (b':', _) => Colon,
            (b'.', b'.') => {
                self.pos += 1;
                DotDot
            }
            (b'.', _) => Dot,
            (b'@', _) => At,
            (b'=', b'>') => {
                self.pos += 1;
                FatArrow
            }
            (b'=', b'=') => {
                self.pos += 1;
                EqEq
            }
            (b'=', _) => Eq,
            (b'!', b'=') => {
                self.pos += 1;
                BangEq
            }
            (b'!', _) => Bang,
            (b'<', b'=') => {
                self.pos += 1;
                LtEq
            }
            (b'<', _) => Lt,
            (b'>', b'=') => {
                self.pos += 1;
                GtEq
            }
            // No `>>` token: `Wrapper<Wrapper<Int>>` closes with two `>`.
            (b'>', _) => Gt,
            (b'+', _) => Plus,
            (b'-', _) => Minus,
            (b'*', _) => Star,
            (b'/', _) => Slash,
            (b'%', _) => Percent,
            (b'&', b'&') => {
                self.pos += 1;
                AndAnd
            }
            (b'&', _) => And,
            (b'|', b'|') => {
                self.pos += 1;
                OrOr
            }
            (b'|', _) => Or,
            (b'^', _) => Caret,
            (b'~', _) => Tilde,
            (b'?', b'?') => {
                self.pos += 1;
                QuestionQuestion
            }
            // No `?.` token, so `x?.field` is `x` `?` `.` `field`.
            (b'?', _) => Question,
            _ => {
                let span = self.span(start);
                let shown = self.text[start..self.pos].to_string();
                self.err(span, format!("unexpected character `{shown}`"), "delete it; no token in the language starts with it");
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
mod tests {
    use super::*;

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
