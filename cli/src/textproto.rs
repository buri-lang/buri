//! A textproto reader and writer for `BUILD.buri` and `REPO.buri`.
//!
//! Build files are data: no expression language, no glob, no macro, no import.
//! So this is a small parser, and the only thing it has to be careful about is
//! keeping enough structure that `buri gen` can rewrite the fields it manages
//! and leave everything else — comments included — saying exactly what it said.
//!
//! The dialect is the one the worked example uses:
//!
//! ```textproto
//! library {
//!   sources: ["cents.buri", "parse.buri"]
//!   test { sources: ["test/cents.buri"] }
//! }
//! outputs: [
//!   { platform: LINUX, arch: X86_64 },
//!   { platform: JS, js { module: ESM } },
//! ]
//! ```
//!
//! A message field takes no colon; a scalar does. Fields inside a message may
//! be separated by whitespace or by commas. `#` starts a comment.

use crate::diag::{Diagnostic, FileId, Span};

#[derive(Clone, Debug)]
pub struct Doc {
    pub fields: Vec<Field>,
    /// Comments after the last field, kept so the formatter can put them back.
    pub trailing: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct Field {
    pub name: String,
    pub name_span: Span,
    pub value: Value,
    /// Comments immediately above this field. A comment stays with the field
    /// beneath it (CLI.md, `format`).
    pub comments: Vec<String>,
    pub blank_before: bool,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub enum Value {
    Str(String, Span),
    Int(i64, Span),
    /// An enum value or a bool, both spelled as a bare identifier.
    Ident(String, Span),
    Msg(Msg, Span),
    List(Vec<Value>, Span),
}

impl Value {
    pub fn span(&self) -> Span {
        match self {
            Value::Str(_, s)
            | Value::Int(_, s)
            | Value::Ident(_, s)
            | Value::Msg(_, s)
            | Value::List(_, s) => *s,
        }
    }

    pub fn kind(&self) -> &'static str {
        match self {
            Value::Str(..) => "a string",
            Value::Int(..) => "a number",
            Value::Ident(..) => "an identifier",
            Value::Msg(..) => "a message",
            Value::List(..) => "a list",
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Msg {
    pub fields: Vec<Field>,
    pub trailing: Vec<String>,
}

impl Msg {
    pub fn get(&self, name: &str) -> Option<&Field> {
        self.fields.iter().find(|f| f.name == name)
    }

    pub fn all<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Field> + 'a {
        self.fields.iter().filter(move |f| f.name == name)
    }
}

impl Doc {
    pub fn get(&self, name: &str) -> Option<&Field> {
        self.fields.iter().find(|f| f.name == name)
    }

    pub fn all<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Field> + 'a {
        self.fields.iter().filter(move |f| f.name == name)
    }

    pub fn as_msg(&self) -> Msg {
        Msg { fields: self.fields.clone(), trailing: self.trailing.clone() }
    }
}

pub struct ParsedProto {
    pub doc: Doc,
    pub errors: Vec<Diagnostic>,
}

pub fn parse(text: &str, file: FileId) -> ParsedProto {
    let mut p = Parser {
        src: text.as_bytes(),
        text,
        pos: 0,
        file,
        errors: Vec::new(),
        comments: Vec::new(),
        blank: false,
        depth: 0,
    };
    let fields = p.fields(None);
    let trailing = std::mem::take(&mut p.comments);
    if p.pos < p.src.len() {
        let span = Span::new(file, p.pos, p.src.len());
        p.errors.push(Diagnostic::error(span, "expected a field"));
    }
    ParsedProto { doc: Doc { fields, trailing }, errors: p.errors }
}

struct Parser<'a> {
    src: &'a [u8],
    text: &'a str,
    pos: usize,
    file: FileId,
    errors: Vec<Diagnostic>,
    comments: Vec<String>,
    blank: bool,
    depth: u32,
}

impl<'a> Parser<'a> {
    fn peek(&self) -> u8 {
        *self.src.get(self.pos).unwrap_or(&0)
    }

    fn err(&mut self, span: Span, msg: impl Into<String>) {
        if self.errors.len() < 32 {
            self.errors.push(Diagnostic::error(span, msg));
        }
    }

    fn skip_trivia(&mut self) {
        let mut newlines = 0;
        loop {
            match self.peek() {
                b' ' | b'\t' | b'\r' => self.pos += 1,
                b'\n' => {
                    newlines += 1;
                    if newlines >= 2 {
                        self.blank = true;
                    }
                    self.pos += 1;
                }
                b'#' => {
                    let start = self.pos;
                    while self.pos < self.src.len() && self.peek() != b'\n' {
                        self.pos += 1;
                    }
                    self.comments.push(self.text[start..self.pos].trim_end().to_string());
                    newlines = 0;
                }
                _ => return,
            }
        }
    }

    /// Parses fields until `close`, or to end of input when `close` is `None`.
    fn fields(&mut self, close: Option<u8>) -> Vec<Field> {
        let mut out = Vec::new();
        loop {
            self.skip_trivia();
            if self.pos >= self.src.len() {
                return out;
            }
            if let Some(c) = close {
                if self.peek() == c {
                    return out;
                }
            }
            let before = self.pos;
            match self.field() {
                Some(f) => out.push(f),
                None => {
                    // Recover: skip a token and try again.
                    if self.pos == before {
                        self.pos += 1;
                    }
                    if close.is_none() && self.pos >= self.src.len() {
                        return out;
                    }
                }
            }
            // Fields may be separated by a comma or by nothing at all.
            self.skip_trivia();
            if self.peek() == b',' {
                self.pos += 1;
            }
        }
    }

    fn field(&mut self) -> Option<Field> {
        let comments = std::mem::take(&mut self.comments);
        let blank_before = std::mem::take(&mut self.blank);
        let start = self.pos;
        let name = self.ident()?;
        let name_span = Span::new(self.file, start, self.pos);
        self.skip_trivia();

        let value = if self.peek() == b':' {
            self.pos += 1;
            self.skip_trivia();
            self.value()?
        } else if self.peek() == b'{' {
            // A message field takes no colon.
            self.msg_value()?
        } else {
            let span = Span::new(self.file, self.pos, self.pos + 1);
            self.err(span, format!("expected `:` or `{{` after `{name}`"));
            return None;
        };

        Some(Field {
            name,
            name_span,
            span: Span::new(self.file, start, self.pos),
            value,
            comments,
            blank_before,
        })
    }

    fn ident(&mut self) -> Option<String> {
        let start = self.pos;
        while self.peek().is_ascii_alphanumeric() || self.peek() == b'_' {
            self.pos += 1;
        }
        if self.pos == start {
            let span = Span::new(self.file, start, start + 1);
            let found = self.text[start..].chars().next().unwrap_or(' ').to_string();
            self.err(span, format!("expected a field name, found `{found}`"));
            return None;
        }
        Some(self.text[start..self.pos].to_string())
    }

    fn msg_value(&mut self) -> Option<Value> {
        let start = self.pos;
        self.pos += 1; // `{`
        self.depth += 1;
        if self.depth > 32 {
            let span = Span::new(self.file, start, self.pos);
            self.err(span, "message nests too deeply");
            self.depth -= 1;
            return None;
        }
        let fields = self.fields(Some(b'}'));
        let trailing = std::mem::take(&mut self.comments);
        self.depth -= 1;
        if self.peek() != b'}' {
            let span = Span::new(self.file, start, self.pos);
            self.err(span, "unterminated message");
            return None;
        }
        self.pos += 1;
        Some(Value::Msg(Msg { fields, trailing }, Span::new(self.file, start, self.pos)))
    }

    fn value(&mut self) -> Option<Value> {
        let start = self.pos;
        match self.peek() {
            b'"' => {
                self.pos += 1;
                let mut s = String::new();
                loop {
                    if self.pos >= self.src.len() || self.peek() == b'\n' {
                        let span = Span::new(self.file, start, self.pos);
                        self.err(span, "unterminated string");
                        return None;
                    }
                    match self.peek() {
                        b'"' => {
                            self.pos += 1;
                            break;
                        }
                        b'\\' => {
                            self.pos += 1;
                            let c = self.peek();
                            self.pos += 1;
                            s.push(match c {
                                b'n' => '\n',
                                b't' => '\t',
                                b'r' => '\r',
                                other => other as char,
                            });
                        }
                        _ => {
                            let ch = self.text[self.pos..].chars().next().unwrap();
                            self.pos += ch.len_utf8();
                            s.push(ch);
                        }
                    }
                }
                Some(Value::Str(s, Span::new(self.file, start, self.pos)))
            }
            b'{' => self.msg_value(),
            b'[' => {
                self.pos += 1;
                let mut items = Vec::new();
                loop {
                    self.skip_trivia();
                    if self.pos >= self.src.len() {
                        let span = Span::new(self.file, start, self.pos);
                        self.err(span, "unterminated list");
                        return None;
                    }
                    if self.peek() == b']' {
                        self.pos += 1;
                        break;
                    }
                    // Comments inside a list are dropped; `gen` rewrites list
                    // contents wholesale, so there is nothing to attach them to.
                    self.comments.clear();
                    let before = self.pos;
                    match self.value() {
                        Some(v) => items.push(v),
                        None => {
                            if self.pos == before {
                                self.pos += 1;
                            }
                        }
                    }
                    self.skip_trivia();
                    if self.peek() == b',' {
                        self.pos += 1;
                    }
                }
                Some(Value::List(items, Span::new(self.file, start, self.pos)))
            }
            b'-' | b'0'..=b'9' => {
                if self.peek() == b'-' {
                    self.pos += 1;
                }
                while self.peek().is_ascii_digit() {
                    self.pos += 1;
                }
                let raw = &self.text[start..self.pos];
                match raw.parse::<i64>() {
                    Ok(v) => Some(Value::Int(v, Span::new(self.file, start, self.pos))),
                    Err(_) => {
                        let span = Span::new(self.file, start, self.pos);
                        let raw = raw.to_string();
                        self.err(span, format!("`{raw}` is not a number"));
                        None
                    }
                }
            }
            c if c.is_ascii_alphabetic() || c == b'_' => {
                let id = self.ident()?;
                Some(Value::Ident(id, Span::new(self.file, start, self.pos)))
            }
            _ => {
                let span = Span::new(self.file, self.pos, self.pos + 1);
                let found = self.text[self.pos..].chars().next().unwrap_or(' ').to_string();
                self.err(span, format!("expected a value, found `{found}`"));
                None
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Printing
// ---------------------------------------------------------------------------

/// Renders a document the way `buri format` does: one field per line, two-space
/// indent, trailing commas, comments kept with the field beneath them.
pub fn print(doc: &Doc) -> String {
    let mut out = String::new();
    print_fields(&mut out, &doc.fields, 0, true);
    for c in &doc.trailing {
        out.push_str(c);
        out.push('\n');
    }
    out
}

fn indent(out: &mut String, level: usize) {
    for _ in 0..level {
        out.push_str("  ");
    }
}

fn print_fields(out: &mut String, fields: &[Field], level: usize, top: bool) {
    for (i, f) in fields.iter().enumerate() {
        // A blank line before a field is preserved, so paragraph structure in a
        // hand-written file survives. At the top level, message fields are
        // always separated.
        let want_blank = f.blank_before
            || (top && i > 0 && matches!(f.value, Value::Msg(..)))
            || (!f.comments.is_empty() && i > 0);
        if want_blank && !out.is_empty() && !out.ends_with("\n\n") {
            out.push('\n');
        }
        for c in &f.comments {
            indent(out, level);
            out.push_str(c);
            out.push('\n');
        }
        indent(out, level);
        out.push_str(&f.name);
        match &f.value {
            Value::Msg(m, _) => {
                out.push_str(" {");
                if m.fields.is_empty() && m.trailing.is_empty() {
                    out.push('}');
                    out.push('\n');
                } else {
                    out.push('\n');
                    print_fields(out, &m.fields, level + 1, false);
                    for c in &m.trailing {
                        indent(out, level + 1);
                        out.push_str(c);
                        out.push('\n');
                    }
                    indent(out, level);
                    out.push_str("}\n");
                }
            }
            v => {
                out.push_str(": ");
                print_value(out, v, level);
                out.push('\n');
            }
        }
    }
}

fn print_value(out: &mut String, v: &Value, level: usize) {
    match v {
        Value::Str(s, _) => {
            out.push('"');
            for c in s.chars() {
                match c {
                    '"' => out.push_str("\\\""),
                    '\\' => out.push_str("\\\\"),
                    '\n' => out.push_str("\\n"),
                    '\t' => out.push_str("\\t"),
                    _ => out.push(c),
                }
            }
            out.push('"');
        }
        Value::Int(n, _) => out.push_str(&n.to_string()),
        Value::Ident(s, _) => out.push_str(s),
        Value::Msg(m, _) => {
            // A message inside a list stays on one line when it is small, which
            // is what makes an `outputs` entry readable.
            out.push_str("{ ");
            let mut first = true;
            for f in &m.fields {
                if !first {
                    out.push_str(", ");
                }
                first = false;
                out.push_str(&f.name);
                match &f.value {
                    Value::Msg(..) => {
                        out.push(' ');
                        print_value(out, &f.value, level);
                    }
                    other => {
                        out.push_str(": ");
                        print_value(out, other, level);
                    }
                }
            }
            out.push_str(" }");
        }
        Value::List(items, _) => {
            if items.is_empty() {
                out.push_str("[]");
                return;
            }
            // A list of one short scalar stays on its line; anything longer
            // goes one element per line with a trailing comma.
            let inline = items.len() == 1
                && items.iter().all(|i| matches!(i, Value::Str(..) | Value::Ident(..) | Value::Int(..)));
            let width: usize = items
                .iter()
                .map(|i| {
                    let mut s = String::new();
                    print_value(&mut s, i, level);
                    s.len() + 2
                })
                .sum();
            if inline || (width <= 60 && items.iter().all(|i| !matches!(i, Value::Msg(..)))) {
                out.push('[');
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    print_value(out, item, level);
                }
                out.push(']');
            } else {
                out.push_str("[\n");
                for item in items {
                    indent(out, level + 1);
                    print_value(out, item, level + 1);
                    out.push_str(",\n");
                }
                indent(out, level);
                out.push(']');
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(src: &str) -> Doc {
        let r = parse(src, FileId(0));
        assert!(r.errors.is_empty(), "{:#?}", r.errors);
        r.doc
    }

    #[test]
    fn message_fields_take_no_colon() {
        let d = p("library {\n  sources: [\"a.buri\"]\n}\n");
        assert_eq!(d.fields.len(), 1);
        assert!(matches!(d.fields[0].value, Value::Msg(..)));
    }

    #[test]
    fn list_of_messages_with_commas_between_fields() {
        let d = p("outputs: [\n  { platform: LINUX, arch: X86_64 },\n  { platform: JS, js { module: ESM } },\n]\n");
        let Value::List(items, _) = &d.fields[0].value else { panic!() };
        assert_eq!(items.len(), 2);
        let Value::Msg(m, _) = &items[1] else { panic!() };
        assert_eq!(m.fields.len(), 2);
        assert!(matches!(m.fields[1].value, Value::Msg(..)));
    }

    #[test]
    fn comments_attach_to_the_field_beneath() {
        let d = p("# why\nlibrary {\n  # inner\n  sources: []\n}\n");
        assert_eq!(d.fields[0].comments, vec!["# why"]);
        let Value::Msg(m, _) = &d.fields[0].value else { panic!() };
        assert_eq!(m.fields[0].comments, vec!["# inner"]);
    }

    #[test]
    fn repeated_top_level_messages() {
        let d = p("tag { name: \"a\" }\ntag { name: \"b\" }\n");
        assert_eq!(d.all("tag").count(), 2);
    }

    #[test]
    fn printing_is_idempotent() {
        let src = "library {\n  sources: [\n    \"a.buri\",\n    \"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb.buri\",\n  ]\n\n  test {}\n}\n";
        let once = print(&p(src));
        let twice = print(&p(&once));
        assert_eq!(once, twice, "left:\n{once}\nright:\n{twice}");
    }

    #[test]
    fn empty_message_stays_on_one_line() {
        let out = print(&p("library {}\n"));
        assert_eq!(out, "library {}\n");
    }
}
