//! JSON, JSONC and JSON5, read into one tree that keeps its comments.
//!
//! |                 | JSON  | JSONC | JSON5 |
//! | --------------- | ----- | ----- | ----- |
//! | Comments        | error | yes   | yes   |
//! | Trailing commas | error | yes   | yes   |
//! | JSON5 syntax    | error | error | yes   |
//!
//! JSON5 syntax is single-quoted strings and their escapes, unquoted keys,
//! hexadecimal numbers, a leading `+` or `.`, a trailing `.`, `Infinity` and
//! `NaN`. A key written twice in one object is an error in all three.

use super::number::{Decimal, Number};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Dialect {
    Json,
    Jsonc,
    Json5,
}

impl Dialect {
    fn comments(self) -> bool {
        self != Dialect::Json
    }
}

/// Byte offsets into the text.
pub type Range = (usize, usize);

#[derive(Clone, Debug)]
pub struct Comment {
    /// As written, delimiters included.
    pub text: String,
    /// Line breaks between the comment and whatever came before it.
    pub newlines_before: usize,
}

#[derive(Clone, Debug)]
pub struct Node {
    pub value: Value,
    pub span: Range,
    /// A scalar's text as written. Empty for an array or an object.
    pub raw: String,
    /// Comments on the lines above it.
    pub leading: Vec<Comment>,
    /// Comments after it on its own line.
    pub trailing: Vec<Comment>,
    /// An empty line sits above it, or above its first leading comment.
    pub blank_before: bool,
}

#[derive(Clone, Debug)]
pub enum Value {
    Null,
    Bool(bool),
    Number(Number),
    Str(String),
    /// The items, and the comments after the last one.
    Array(Vec<Node>, Vec<Comment>),
    /// The members, and the comments after the last one.
    Object(Vec<Member>, Vec<Comment>),
}

#[derive(Clone, Debug)]
pub struct Member {
    pub key: String,
    pub key_raw: String,
    pub key_span: Range,
    /// The comments of the member live on its value.
    pub value: Node,
}

impl Node {
    pub fn get(&self, key: &str) -> Option<&Node> {
        match &self.value {
            Value::Object(members, _) => members.iter().find(|m| m.key == key).map(|m| &m.value),
            _ => None,
        }
    }

    pub fn member(&self, key: &str) -> Option<&Member> {
        match &self.value {
            Value::Object(members, _) => members.iter().find(|m| m.key == key),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match &self.value {
            Value::Str(s) => Some(s),
            _ => None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Document {
    pub root: Node,
    /// Comments after the root, on lines of their own.
    pub trailing: Vec<Comment>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SyntaxError {
    pub span: Range,
    pub problem: String,
    pub remedy: String,
}

/// Deeper than this is refused rather than recursed into.
const MAX_DEPTH: usize = 256;

pub fn parse(text: &str, dialect: Dialect) -> Result<Document, SyntaxError> {
    let mut p = Parser { text, at: 0, dialect, depth: 0 };
    let mut first = p.token()?;
    if first.kind == Kind::Eof {
        return Err(p.error(first.span, "the file holds no value", "write a JSON value, such as `{}`"));
    }
    let leading = std::mem::take(&mut first.comments);
    let mut root = p.value(first)?;
    root.leading = leading;
    root.blank_before = false;
    let mut end = p.token()?;
    if end.kind != Kind::Eof {
        return Err(p.error(
            end.span,
            "a JSON file holds one value, and this is a second",
            "wrap the values in an array, or delete this one",
        ));
    }
    let (same_line, rest) = split_same_line(std::mem::take(&mut end.comments));
    root.trailing.extend(same_line);
    Ok(Document { root, trailing: rest })
}

#[derive(Clone, Debug, PartialEq)]
enum Kind {
    Open(char),
    Close(char),
    Colon,
    Comma,
    Str(String),
    Number(Number),
    Word(String),
    Eof,
}

struct Token {
    kind: Kind,
    span: Range,
    comments: Vec<Comment>,
    newlines_before: usize,
}

impl Token {
    /// An empty line before the token, or before its first comment.
    fn blank_before(&self) -> bool {
        match self.comments.first() {
            Some(c) => c.newlines_before >= 2,
            None => self.newlines_before >= 2,
        }
    }
}

/// The comments on the line a value ended on, and the rest.
fn split_same_line(comments: Vec<Comment>) -> (Vec<Comment>, Vec<Comment>) {
    let n = comments.iter().take_while(|c| c.newlines_before == 0).count();
    let mut same_line = comments;
    let rest = same_line.split_off(n);
    (same_line, rest)
}

struct Parser<'t> {
    text: &'t str,
    at: usize,
    dialect: Dialect,
    depth: usize,
}

fn describe(c: char) -> String {
    match c {
        '\n' => "a line break".to_string(),
        c if c.is_control() => format!("the control character U+{:04X}", c as u32),
        c => format!("`{c}`"),
    }
}

fn is_json_space(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\r')
}

fn is_json5_space(c: char) -> bool {
    is_json_space(c)
        || matches!(c, '\u{0b}' | '\u{0c}' | '\u{a0}' | '\u{2028}' | '\u{2029}' | '\u{feff}')
        || (c.is_whitespace() && !c.is_control())
}

fn is_identifier_start(c: char) -> bool {
    c.is_alphabetic() || c == '$' || c == '_'
}

fn is_identifier_part(c: char) -> bool {
    is_identifier_start(c) || c.is_alphanumeric() || c == '\u{200c}' || c == '\u{200d}'
}

impl<'t> Parser<'t> {
    fn error(&self, span: Range, problem: &str, remedy: &str) -> SyntaxError {
        SyntaxError { span, problem: problem.to_string(), remedy: remedy.to_string() }
    }

    fn peek(&self) -> Option<char> {
        self.text.get(self.at..).and_then(|s| s.chars().next())
    }

    fn peek2(&self) -> Option<char> {
        self.text.get(self.at..).and_then(|s| s.chars().nth(1))
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.peek()?;
        self.at = self.at.saturating_add(c.len_utf8());
        Some(c)
    }

    fn here(&self, len: usize) -> Range {
        (self.at, self.at.saturating_add(len).min(self.text.len()))
    }

    fn slice(&self, span: Range) -> &'t str {
        self.text.get(span.0..span.1).unwrap_or_default()
    }

    fn no_json5(&self, span: Range, what: &str) -> SyntaxError {
        self.error(
            span,
            &format!("{what} is JSON5, not JSON"),
            "write it the JSON way, or rename the file to `.json5`",
        )
    }

    /// Whitespace and comments, then one token.
    fn token(&mut self) -> Result<Token, SyntaxError> {
        let mut comments = Vec::new();
        let mut newlines = 0usize;
        while let Some(c) = self.peek() {
            let space = match self.dialect {
                Dialect::Json5 => is_json5_space(c),
                _ => is_json_space(c),
            };
            if space {
                if c == '\n' || c == '\u{2028}' || c == '\u{2029}' {
                    newlines = newlines.saturating_add(1);
                }
                self.bump();
                continue;
            }
            if c == '/' && matches!(self.peek2(), Some('/' | '*')) {
                let start = self.at;
                let block = self.peek2() == Some('*');
                if !self.dialect.comments() {
                    return Err(self.error(
                        self.here(2),
                        "JSON has no comments",
                        "delete the comment, or rename the file to `.jsonc` or `.json5`",
                    ));
                }
                self.bump();
                self.bump();
                if block {
                    loop {
                        match self.bump() {
                            None => {
                                return Err(self.error(
                                    (start, start.saturating_add(2)),
                                    "this comment is never closed",
                                    "close it with `*/`",
                                ))
                            }
                            Some('*') if self.peek() == Some('/') => {
                                self.bump();
                                break;
                            }
                            Some(_) => {}
                        }
                    }
                } else {
                    while let Some(c) = self.peek() {
                        if c == '\n' || c == '\r' || c == '\u{2028}' || c == '\u{2029}' {
                            break;
                        }
                        self.bump();
                    }
                }
                let text = self.slice((start, self.at)).trim_end().to_string();
                comments.push(Comment { text, newlines_before: newlines });
                newlines = 0;
                continue;
            }
            break;
        }
        let start = self.at;
        let kind = match self.peek() {
            None => Kind::Eof,
            Some(c @ ('{' | '[')) => {
                self.bump();
                Kind::Open(c)
            }
            Some(c @ ('}' | ']')) => {
                self.bump();
                Kind::Close(c)
            }
            Some(':') => {
                self.bump();
                Kind::Colon
            }
            Some(',') => {
                self.bump();
                Kind::Comma
            }
            Some(q @ ('"' | '\'')) => {
                if q == '\'' && self.dialect != Dialect::Json5 {
                    return Err(self.no_json5(self.here(1), "a single-quoted string"));
                }
                Kind::Str(self.string(q)?)
            }
            Some(c) if c.is_ascii_digit() || matches!(c, '-' | '+' | '.') => self.number()?,
            Some(c) if is_identifier_start(c) || c == '\\' => {
                if c == '\\' {
                    return Err(self.error(
                        self.here(1),
                        "an escape in a key is not supported",
                        "quote the key",
                    ));
                }
                while self.peek().is_some_and(is_identifier_part) {
                    self.bump();
                }
                Kind::Word(self.slice((start, self.at)).to_string())
            }
            Some(c) => {
                return Err(self.error(
                    self.here(c.len_utf8()),
                    &format!("{} cannot start a JSON value", describe(c)),
                    "check for a missing quote or a stray character",
                ))
            }
        };
        Ok(Token { kind, span: (start, self.at), comments, newlines_before: newlines })
    }

    fn string(&mut self, quote: char) -> Result<String, SyntaxError> {
        let start = self.at;
        self.bump();
        let mut out = String::new();
        loop {
            let at = self.at;
            let Some(c) = self.bump() else {
                return Err(self.error((start, start.saturating_add(1)), "this string is never closed", "close it with a quote"));
            };
            match c {
                c if c == quote => return Ok(out),
                '\\' => self.escape(at, &mut out)?,
                '\n' | '\r' => {
                    return Err(self.error(
                        (start, start.saturating_add(1)),
                        "this string is never closed",
                        "close it with a quote on the same line",
                    ))
                }
                c if (c as u32) < 0x20 => {
                    return Err(self.error(
                        (at, self.at),
                        &format!("{} must be escaped in a string", describe(c)),
                        "write it as a `\\u` escape",
                    ))
                }
                c => out.push(c),
            }
        }
    }

    fn escape(&mut self, at: usize, out: &mut String) -> Result<(), SyntaxError> {
        let json5 = self.dialect == Dialect::Json5;
        let Some(c) = self.bump() else {
            return Err(self.error((at, self.at), "this string is never closed", "close it with a quote"));
        };
        let span = (at, self.at);
        match c {
            '"' => out.push('"'),
            '\\' => out.push('\\'),
            '/' => out.push('/'),
            'b' => out.push('\u{8}'),
            'f' => out.push('\u{c}'),
            'n' => out.push('\n'),
            'r' => out.push('\r'),
            't' => out.push('\t'),
            'u' => {
                let high = self.hex(4, at)?;
                let scalar = if (0xd800..0xdc00).contains(&high)
                    && self.peek() == Some('\\')
                    && self.peek2() == Some('u')
                {
                    let back = self.at;
                    self.bump();
                    self.bump();
                    let low = self.hex(4, at)?;
                    if (0xdc00..0xe000).contains(&low) {
                        0x10000u32
                            .saturating_add(high.saturating_sub(0xd800) << 10)
                            .saturating_add(low.saturating_sub(0xdc00))
                    } else {
                        self.at = back;
                        high
                    }
                } else {
                    high
                };
                // A lone surrogate has no character to stand for; JSON allows
                // it and a Rust string cannot hold it.
                out.push(char::from_u32(scalar).unwrap_or('\u{fffd}'));
            }
            '\'' if json5 => out.push('\''),
            'v' if json5 => out.push('\u{b}'),
            '0' if json5 && !self.peek().is_some_and(|c| c.is_ascii_digit()) => out.push('\0'),
            'x' if json5 => {
                let v = self.hex(2, at)?;
                out.push(char::from_u32(v).unwrap_or('\u{fffd}'));
            }
            '\r' if json5 => {
                if self.peek() == Some('\n') {
                    self.bump();
                }
            }
            '\n' | '\u{2028}' | '\u{2029}' if json5 => {}
            c if json5 && !c.is_ascii_digit() => out.push(c),
            _ => {
                return Err(self.error(
                    span,
                    &format!("`\\{c}` is not an escape"),
                    "write `\\\\` for a backslash",
                ))
            }
        }
        Ok(())
    }

    fn hex(&mut self, n: usize, at: usize) -> Result<u32, SyntaxError> {
        let mut v = 0u32;
        for _ in 0..n {
            let Some(d) = self.peek().and_then(|c| c.to_digit(16)) else {
                return Err(self.error(
                    (at, self.at),
                    &format!("an escape needs {n} hexadecimal digits here"),
                    "complete the escape",
                ));
            };
            self.bump();
            v = v.saturating_mul(16).saturating_add(d);
        }
        Ok(v)
    }

    fn digits(&mut self) -> &'t str {
        let start = self.at;
        while self.peek().is_some_and(|c| c.is_ascii_digit()) {
            self.bump();
        }
        self.slice((start, self.at))
    }

    fn number(&mut self) -> Result<Kind, SyntaxError> {
        let start = self.at;
        let json5 = self.dialect == Dialect::Json5;
        let negative = match self.peek() {
            Some('-') => {
                self.bump();
                true
            }
            Some('+') => {
                if !json5 {
                    return Err(self.no_json5(self.here(1), "a leading `+`"));
                }
                self.bump();
                false
            }
            _ => false,
        };
        // `Infinity` and `NaN`, after an optional sign.
        if self.peek().is_some_and(is_identifier_start) {
            let word_start = self.at;
            while self.peek().is_some_and(is_identifier_part) {
                self.bump();
            }
            let word = self.slice((word_start, self.at));
            let span = (start, self.at);
            return match word {
                "Infinity" | "NaN" if !json5 => Err(self.no_json5(span, &format!("`{word}`"))),
                "Infinity" => Ok(Kind::Number(Number::Infinity(negative))),
                "NaN" => Ok(Kind::Number(Number::NaN)),
                _ => Err(self.error(span, "this is not a number", "write digits after the sign")),
            };
        }
        if self.peek() == Some('0') && matches!(self.peek2(), Some('x' | 'X')) {
            if !json5 {
                return Err(self.no_json5(self.here(2), "a hexadecimal number"));
            }
            self.bump();
            self.bump();
            let hex_start = self.at;
            while self.peek().is_some_and(|c| c.is_ascii_hexdigit()) {
                self.bump();
            }
            let hex = self.slice((hex_start, self.at));
            if hex.is_empty() {
                return Err(self.error((start, self.at), "`0x` needs digits after it", "write the digits"));
            }
            return Ok(Kind::Number(Number::Finite(Decimal::from_hex(negative, hex))));
        }
        let whole = self.digits();
        if whole.len() > 1 && whole.starts_with('0') {
            return Err(self.error(
                (start, self.at),
                "a number does not start with `0`",
                "delete the leading zeros",
            ));
        }
        let mut fraction = "";
        if self.peek() == Some('.') {
            self.bump();
            fraction = self.digits();
            if fraction.is_empty() && !json5 {
                return Err(self.no_json5((start, self.at), "a number ending in `.`"));
            }
        }
        if whole.is_empty() {
            if fraction.is_empty() {
                return Err(self.error((start, self.at.max(start.saturating_add(1))), "this is not a number", "write digits"));
            }
            if !json5 {
                return Err(self.no_json5((start, self.at), "a number starting with `.`"));
            }
        }
        let mut exp = 0i64;
        if matches!(self.peek(), Some('e' | 'E')) {
            self.bump();
            let sign: i64 = match self.peek() {
                Some('-') => {
                    self.bump();
                    -1
                }
                Some('+') => {
                    self.bump();
                    1
                }
                _ => 1,
            };
            let digits = self.digits();
            if digits.is_empty() {
                return Err(self.error((start, self.at), "an exponent needs digits", "write the digits after `e`"));
            }
            exp = digits
                .bytes()
                .fold(0i64, |n, b| n.saturating_mul(10).saturating_add(i64::from(b.saturating_sub(b'0'))))
                .saturating_mul(sign);
        }
        Ok(Kind::Number(Number::Finite(Decimal::from_parts(negative, whole, fraction, exp))))
    }

    fn scalar(&self, token: Token, value: Value) -> Node {
        Node {
            value,
            raw: self.slice(token.span).to_string(),
            span: token.span,
            leading: Vec::new(),
            trailing: Vec::new(),
            blank_before: false,
        }
    }

    /// The value `first` starts. Its comments are the caller's.
    fn value(&mut self, first: Token) -> Result<Node, SyntaxError> {
        match &first.kind {
            Kind::Str(s) => {
                let value = Value::Str(s.clone());
                Ok(self.scalar(first, value))
            }
            Kind::Number(n) => {
                let value = Value::Number(n.clone());
                Ok(self.scalar(first, value))
            }
            Kind::Word(w) => {
                let value = match w.as_str() {
                    "true" => Value::Bool(true),
                    "false" => Value::Bool(false),
                    "null" => Value::Null,
                    "Infinity" | "NaN" if self.dialect == Dialect::Json5 => {
                        Value::Number(if w == "NaN" { Number::NaN } else { Number::Infinity(false) })
                    }
                    "Infinity" | "NaN" => return Err(self.no_json5(first.span, &format!("`{w}`"))),
                    _ => {
                        return Err(self.error(
                            first.span,
                            &format!("`{w}` is not a JSON value"),
                            "quote it if it is a string",
                        ))
                    }
                };
                Ok(self.scalar(first, value))
            }
            Kind::Open(open) => {
                let open = *open;
                self.depth = self.depth.saturating_add(1);
                if self.depth > MAX_DEPTH {
                    return Err(self.error(
                        first.span,
                        &format!("values nest more than {MAX_DEPTH} deep here"),
                        "flatten the structure",
                    ));
                }
                let node = self.container(first.span.0, open)?;
                self.depth = self.depth.saturating_sub(1);
                Ok(node)
            }
            Kind::Close(c) => Err(self.error(first.span, &format!("`{c}` closes nothing here"), "delete it, or open what it closes")),
            Kind::Colon | Kind::Comma => Err(self.error(
                first.span,
                &format!("expected a value, found `{}`", self.slice(first.span)),
                "write a value here",
            )),
            Kind::Eof => Err(self.error(first.span, "the file ends where a value should be", "write the value")),
        }
    }

    /// An array or an object, after its opening bracket.
    fn container(&mut self, start: usize, open: char) -> Result<Node, SyntaxError> {
        let close = if open == '{' { '}' } else { ']' };
        let mut items: Vec<Node> = Vec::new();
        let mut members: Vec<Member> = Vec::new();
        let mut next = self.token()?;
        loop {
            // What is still on the previous element's line belongs to it.
            let (same_line, rest) = split_same_line(std::mem::take(&mut next.comments));
            let last = if open == '{' { members.last_mut().map(|m| &mut m.value) } else { items.last_mut() };
            match last {
                Some(last) => last.trailing.extend(same_line),
                None => next.comments.extend(same_line),
            }
            next.comments.extend(rest);
            if next.kind == Kind::Close(close) {
                let span = (start, next.span.1);
                let value = if open == '{' {
                    Value::Object(members, next.comments)
                } else {
                    Value::Array(items, next.comments)
                };
                return Ok(Node { value, span, raw: String::new(), leading: Vec::new(), trailing: Vec::new(), blank_before: false });
            }
            let blank_before = next.blank_before();
            let leading = std::mem::take(&mut next.comments);
            let mut node = if open == '{' {
                let (key, key_raw) = match &next.kind {
                    Kind::Str(s) => (s.clone(), self.slice(next.span).to_string()),
                    Kind::Word(w) if self.dialect == Dialect::Json5 => (w.clone(), w.clone()),
                    Kind::Word(_) => return Err(self.no_json5(next.span, "an unquoted key")),
                    Kind::Eof => {
                        return Err(self.error((start, start.saturating_add(1)), "this object is never closed", &format!("close it with `{close}`")))
                    }
                    _ => {
                        return Err(self.error(
                            next.span,
                            &format!("expected a key, found `{}`", self.slice(next.span)),
                            "write a quoted key",
                        ))
                    }
                };
                if members.iter().any(|m| m.key == key) {
                    return Err(self.error(
                        next.span,
                        &format!("the key {key_raw} appears twice in this object"),
                        "delete one of them",
                    ));
                }
                let key_span = next.span;
                let colon = self.token()?;
                if colon.kind != Kind::Colon {
                    return Err(self.error(colon.span, "expected `:` after the key", "write `:` and the value"));
                }
                let mut first = self.token()?;
                let mut inner = colon.comments;
                inner.append(&mut first.comments);
                let mut value = self.value(first)?;
                // Comments between the key and its value move above the member.
                let mut all = leading;
                all.extend(inner.into_iter().map(|mut c| {
                    c.newlines_before = c.newlines_before.max(1);
                    c
                }));
                value.leading = all;
                value.blank_before = blank_before;
                members.push(Member { key, key_raw, key_span, value });
                None
            } else {
                if next.kind == Kind::Eof {
                    return Err(self.error((start, start.saturating_add(1)), "this array is never closed", "close it with `]`"));
                }
                let mut value = self.value(next)?;
                value.leading = leading;
                value.blank_before = blank_before;
                Some(value)
            };
            if let Some(item) = node.take() {
                items.push(item);
            }
            let mut after = self.token()?;
            let last = if open == '{' { members.last_mut().map(|m| &mut m.value) } else { items.last_mut() };
            // Anything between the element and its comma is the element's.
            if let Some(last) = last {
                if after.kind == Kind::Comma || after.kind == Kind::Close(close) {
                    let (same_line, rest) = split_same_line(std::mem::take(&mut after.comments));
                    last.trailing.extend(same_line);
                    after.comments = rest;
                }
            }
            match after.kind {
                Kind::Comma => {
                    let comma = after.span;
                    let pending = after.comments;
                    next = self.token()?;
                    let mut carried = pending;
                    carried.append(&mut next.comments);
                    next.comments = carried;
                    if next.kind == Kind::Close(close) && self.dialect == Dialect::Json {
                        return Err(self.error(
                            comma,
                            "JSON has no trailing commas",
                            "delete the comma, or rename the file to `.jsonc` or `.json5`",
                        ));
                    }
                    if next.kind == Kind::Comma {
                        return Err(self.error(next.span, "two commas with no value between them", "delete one"));
                    }
                }
                Kind::Close(c) if c == close => next = after,
                Kind::Eof => {
                    return Err(self.error(
                        (start, start.saturating_add(1)),
                        &format!("this {} is never closed", if open == '{' { "object" } else { "array" }),
                        &format!("close it with `{close}`"),
                    ))
                }
                _ => {
                    return Err(self.error(
                        after.span,
                        &format!("expected `,` or `{close}`, found `{}`", self.slice(after.span)),
                        "separate the values with a comma",
                    ))
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_refuses_what_its_supersets_allow() {
        assert!(parse("{\"a\": 1,}", Dialect::Json).is_err());
        assert!(parse("{\"a\": 1,}", Dialect::Jsonc).is_ok());
        assert!(parse("// c\n{}", Dialect::Json).is_err());
        assert!(parse("// c\n{}", Dialect::Jsonc).is_ok());
        assert!(parse("{a: 'x'}", Dialect::Jsonc).is_err());
        assert!(parse("{a: 'x', b: 0x1F, c: +.5, d: 5., e: -Infinity, f: NaN,}", Dialect::Json5).is_ok());
    }

    #[test]
    fn a_duplicate_key_is_an_error() {
        let e = parse("{\"a\": 1, \"a\": 2}", Dialect::Json).unwrap_err();
        assert_eq!(e.span, (9, 12));
    }

    #[test]
    fn comments_land_on_the_value_they_describe() {
        let doc = parse("{\n  // above\n  \"a\": 1, // beside\n  \"b\": 2\n  // below\n}\n", Dialect::Jsonc).unwrap();
        let Value::Object(members, dangling) = &doc.root.value else { panic!() };
        assert_eq!(members[0].value.leading[0].text, "// above");
        assert_eq!(members[0].value.trailing[0].text, "// beside");
        assert!(members[1].value.leading.is_empty());
        assert_eq!(dangling[0].text, "// below");
    }

    #[test]
    fn strings_decode_their_escapes() {
        let doc = parse(r#"["aé😀\n"]"#, Dialect::Json).unwrap();
        let Value::Array(items, _) = &doc.root.value else { panic!() };
        assert_eq!(items[0].as_str(), Some("aé😀\n"));
    }

    #[test]
    fn deep_nesting_is_refused_rather_than_recursed() {
        let text = "[".repeat(10_000);
        assert!(parse(&text, Dialect::Json).is_err());
    }
}
