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

use crate::diagnostics::{Diagnostic, FileId, Span};

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
        p.errors.push(Diagnostic::error(span, "expected a field").with_fix(
            "a build file is a list of `name: value` and `name { ... }` fields",
        ));
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

/// How deep `{ ... }` and `[ ... ]` may nest before the reader gives up.
///
/// Both forms are read by recursion, so without a bound a file that is nothing
/// but open brackets exhausts the stack — a crash with no diagnostic, which is
/// the one thing a build file must never be able to do.
const MAX_NESTING: u32 = 32;

impl<'a> Parser<'a> {
    fn peek(&self) -> u8 {
        *self.src.get(self.pos).unwrap_or(&0)
    }

    /// The text from the cursor on, or `""` if the cursor has run past the end.
    fn rest(&self) -> &'a str {
        self.text.get(self.pos..).unwrap_or("")
    }

    fn slice(&self, start: usize, end: usize) -> &'a str {
        self.text.get(start..end).unwrap_or("")
    }

    /// Steps over one whole character.
    ///
    /// Every recovery path uses this rather than `pos += 1`, because a cursor
    /// left *inside* a multi-byte character turns every later slice of the text
    /// into a panic — and a build file is free to contain one anywhere.
    fn bump_char(&mut self) {
        let step = self.rest().chars().next().map_or(1, char::len_utf8);
        self.pos = self.pos.saturating_add(step);
    }

    /// Every textproto error carries the edit that resolves it, the same way
    /// every source diagnostic does.
    fn err(&mut self, span: Span, msg: impl Into<String>, fix: impl Into<String>) {
        if self.errors.len() < 32 {
            self.errors.push(Diagnostic::error(span, msg).with_fix(fix));
        }
    }

    fn skip_trivia(&mut self) {
        let mut newlines: u32 = 0;
        loop {
            match self.peek() {
                b' ' | b'\t' | b'\r' => self.pos = self.pos.saturating_add(1),
                b'\n' => {
                    newlines = newlines.saturating_add(1);
                    if newlines >= 2 {
                        self.blank = true;
                    }
                    self.pos = self.pos.saturating_add(1);
                }
                b'#' => {
                    let start = self.pos;
                    // A comment runs to the newline. Stepping by whole
                    // characters keeps the cursor on a boundary even when the
                    // comment holds text that is not ASCII.
                    while self.pos < self.src.len() && self.peek() != b'\n' {
                        self.bump_char();
                    }
                    self.comments.push(self.slice(start, self.pos).trim_end().to_string());
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
                        self.bump_char();
                    }
                    if close.is_none() && self.pos >= self.src.len() {
                        return out;
                    }
                }
            }
            // Fields may be separated by a comma or by nothing at all.
            self.skip_trivia();
            if self.peek() == b',' {
                self.pos = self.pos.saturating_add(1);
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
            self.pos = self.pos.saturating_add(1);
            self.skip_trivia();
            self.value()?
        } else if self.peek() == b'{' {
            // A message field takes no colon.
            self.msg_value()?
        } else {
            let span = Span::new(self.file, self.pos, self.pos.saturating_add(1));
            self.err(
            span,
            format!("expected `:` or `{{` after `{name}`"),
            "write `: value` for a scalar or a list, or `{{ ... }}` for a block",
        );
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
            self.pos = self.pos.saturating_add(1);
        }
        if self.pos == start {
            let span = Span::new(self.file, start, start.saturating_add(1));
            let found = self.rest().chars().next().unwrap_or(' ').to_string();
            self.err(
                span,
                format!("expected a field name, found `{found}`"),
                "name a field the schema declares",
            );
            return None;
        }
        Some(self.slice(start, self.pos).to_string())
    }

    fn msg_value(&mut self) -> Option<Value> {
        let start = self.pos;
        self.pos = self.pos.saturating_add(1); // `{`
        self.depth = self.depth.saturating_add(1);
        if self.depth > MAX_NESTING {
            let span = Span::new(self.file, start, self.pos);
            self.err(
                span,
                "message nests too deeply",
                "flatten it; the limit exists so a pathological file cannot exhaust the reader's stack",
            );
            self.depth = self.depth.saturating_sub(1);
            return None;
        }
        let fields = self.fields(Some(b'}'));
        let trailing = std::mem::take(&mut self.comments);
        self.depth = self.depth.saturating_sub(1);
        if self.peek() != b'}' {
            let span = Span::new(self.file, start, self.pos);
            self.err(span, "unterminated message", "close it with `}`");
            return None;
        }
        self.pos = self.pos.saturating_add(1);
        Some(Value::Msg(Msg { fields, trailing }, Span::new(self.file, start, self.pos)))
    }

    fn value(&mut self) -> Option<Value> {
        let start = self.pos;
        match self.peek() {
            b'"' => {
                self.pos = self.pos.saturating_add(1);
                let mut s = String::new();
                loop {
                    if self.pos >= self.src.len() || self.peek() == b'\n' {
                        let span = Span::new(self.file, start, self.pos);
                        self.err(span, "unterminated string", "close it with a quote");
                        return None;
                    }
                    match self.peek() {
                        b'"' => {
                            self.pos = self.pos.saturating_add(1);
                            break;
                        }
                        b'\\' => {
                            self.pos = self.pos.saturating_add(1);
                            // An escape names a character, not a byte: `\é` has
                            // to step over the whole `é`, or the cursor lands
                            // inside it. A backslash at the end of the input
                            // leaves the cursor there and the loop head reports
                            // the unterminated string.
                            if let Some(c) = self.rest().chars().next() {
                                self.pos = self.pos.saturating_add(c.len_utf8());
                                s.push(match c {
                                    'n' => '\n',
                                    't' => '\t',
                                    'r' => '\r',
                                    other => other,
                                });
                            }
                        }
                        _ => match self.rest().chars().next() {
                            Some(ch) => {
                                self.pos = self.pos.saturating_add(ch.len_utf8());
                                s.push(ch);
                            }
                            // The loop head established there is input left, so
                            // this says the text is not valid UTF-8 from here;
                            // step off the byte rather than spin on it.
                            None => self.pos = self.pos.saturating_add(1),
                        },
                    }
                }
                Some(Value::Str(s, Span::new(self.file, start, self.pos)))
            }
            b'{' => self.msg_value(),
            b'[' => {
                // A list is read by recursion just as a message is, so it takes
                // the same bound; `[[[[…` is as easy to write as `{{{{…`.
                self.depth = self.depth.saturating_add(1);
                if self.depth > MAX_NESTING {
                    let span = Span::new(self.file, start, self.pos.saturating_add(1));
                    self.err(
                        span,
                        "list nests too deeply",
                        "flatten it; the limit exists so a pathological file cannot exhaust the reader's stack",
                    );
                    self.depth = self.depth.saturating_sub(1);
                    return None;
                }
                let v = self.list_value(start);
                self.depth = self.depth.saturating_sub(1);
                v
            }
            b'-' | b'0'..=b'9' => {
                if self.peek() == b'-' {
                    self.pos = self.pos.saturating_add(1);
                }
                while self.peek().is_ascii_digit() {
                    self.pos = self.pos.saturating_add(1);
                }
                let raw = self.slice(start, self.pos);
                match raw.parse::<i64>() {
                    Ok(v) => Some(Value::Int(v, Span::new(self.file, start, self.pos))),
                    Err(_) => {
                        let span = Span::new(self.file, start, self.pos);
                        let raw = raw.to_string();
                        self.err(span, format!("`{raw}` is not a number"), "write a decimal integer, or quote it if it is meant to be text");
                        None
                    }
                }
            }
            c if c.is_ascii_alphabetic() || c == b'_' => {
                let id = self.ident()?;
                Some(Value::Ident(id, Span::new(self.file, start, self.pos)))
            }
            _ => {
                let span = Span::new(self.file, self.pos, self.pos.saturating_add(1));
                let found = self.rest().chars().next().unwrap_or(' ').to_string();
                self.err(
                span,
                format!("expected a value, found `{found}`"),
                "write a string, a number, a bare word, a `[list]`, or a `{{ block }}`",
            );
                None
            }
        }
    }

    /// The body of a `[ ... ]`, with the cursor still on the `[`. Split out of
    /// [`Parser::value`] so that the nesting counter is raised and lowered in
    /// one place rather than on each of the ways out of the loop.
    fn list_value(&mut self, start: usize) -> Option<Value> {
        self.pos = self.pos.saturating_add(1);
        let mut items = Vec::new();
        loop {
            self.skip_trivia();
            if self.pos >= self.src.len() {
                let span = Span::new(self.file, start, self.pos);
                self.err(span, "unterminated list", "close it with `]`");
                return None;
            }
            if self.peek() == b']' {
                self.pos = self.pos.saturating_add(1);
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
                        self.bump_char();
                    }
                }
            }
            self.skip_trivia();
            if self.peek() == b',' {
                self.pos = self.pos.saturating_add(1);
            }
        }
        Some(Value::List(items, Span::new(self.file, start, self.pos)))
    }
}

// ---------------------------------------------------------------------------
// Printing
// ---------------------------------------------------------------------------

/// One level of indentation, the same as `crate::formatting`'s. A build file
/// and the source beside it are read by the same person on the same screen.
const INDENT: usize = 4;

/// The order the schema declares a message's fields in, keyed by the name of
/// the field that holds it — `""` for the top level of a file.
///
/// This is the canonical order, and it is the schema's rather than anybody's
/// taste: a build file is data, the order of its fields carries no meaning, and
/// the one order nobody has to argue about is the one the schema was written
/// in. `library` before `binary` falls out of it (CLI.md), and so does
/// `sources` before `dependencies` before `test`.
///
/// It is also the list `buildfile.rs` reads a message against, which is what
/// decides whether a field is a field at all — one table, so that "the
/// formatter's order" and "the fields a rule has" cannot be two answers. The
/// top level is the exception and says why below.
///
/// A name missing from here is not an error at format time: an unknown field
/// keeps its place at the end, because a formatter that dropped or reordered
/// something it did not recognise would be worse than one that left it alone.
pub fn schema_order(msg: &str) -> &'static [&'static str] {
    match msg {
        // A BUILD.buri holds `library`/`binary`; a REPO.buri holds `tag`. One
        // table here because no file has both and the formatter does not know
        // which kind it is looking at — which is why the top level is the one
        // place `buildfile.rs` keeps its own lists, and a test below holds the
        // two halves to this union.
        "" => &["library", "binary", "tag"],
        "library" => &[
            "sources",
            "proto_sources",
            "dependencies",
            "tags",
            "platforms",
            "visibility",
            "test",
            "testing",
        ],
        "binary" => &["sources", "proto_sources", "dependencies", "tags", "outputs", "test"],
        "test" => &["sources", "dependencies", "data", "timeout_seconds", "platforms"],
        "testing" => &["sources", "dependencies"],
        "outputs" => &["platform", "arch", "artifact_name", "js"],
        "js" => &["module"],
        "tag" => &["name", "doc", "forbids", "requires"],
        "forbids" => &["tags"],
        "requires" => &["platforms"],
        _ => &[],
    }
}

/// The fields of one message in canonical order.
///
/// Stable, so two fields of the same name — two `tag` blocks, two `outputs`
/// entries — keep the order they were written in. That order is the only thing
/// about a repeated field that could carry meaning, so it is the one thing not
/// touched.
fn ordered<'a>(fields: &'a [Field], msg: &str) -> Vec<&'a Field> {
    let order = schema_order(msg);
    let rank = |f: &Field| order.iter().position(|n| *n == f.name).unwrap_or(order.len());
    let mut out: Vec<&Field> = fields.iter().collect();
    out.sort_by_key(|f| rank(f));
    out
}

/// Renders a document the way `buri format` does: the schema's field order,
/// one field per line, a four-space indent, trailing commas, comments kept
/// with the field beneath them.
pub fn print(doc: &Doc) -> String {
    let mut out = String::new();
    print_fields(&mut out, &ordered(&doc.fields, ""), 0, true);
    for c in &doc.trailing {
        out.push_str(c);
        out.push('\n');
    }
    out
}

fn indent(out: &mut String, level: usize) {
    for _ in 0..level.saturating_mul(INDENT) {
        out.push(' ');
    }
}

fn print_fields(out: &mut String, fields: &[&Field], level: usize, top: bool) {
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
                    let inner = ordered(&m.fields, &f.name);
                    print_fields(out, &inner, level.saturating_add(1), false);
                    for c in &m.trailing {
                        indent(out, level.saturating_add(1));
                        out.push_str(c);
                        out.push('\n');
                    }
                    indent(out, level);
                    out.push_str("}\n");
                }
            }
            v => {
                out.push_str(": ");
                print_value(out, v, level, &f.name);
                out.push('\n');
            }
        }
    }
}

/// `within` is the name of the field this value belongs to, which is how a
/// message inside a list finds the schema order of its own fields: an element
/// of `outputs` has no name of its own, so it takes the list's.
fn print_value(out: &mut String, v: &Value, level: usize, within: &str) {
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
            // is what makes an `outputs` entry readable. Its fields take the
            // schema's order like any other message's; `within` names the list
            // that holds it, because an element of a list has no name of its
            // own to look up.
            out.push_str("{ ");
            let mut first = true;
            for f in ordered(&m.fields, within) {
                if !first {
                    out.push_str(", ");
                }
                first = false;
                out.push_str(&f.name);
                match &f.value {
                    Value::Msg(..) => {
                        out.push(' ');
                        print_value(out, &f.value, level, &f.name);
                    }
                    other => {
                        out.push_str(": ");
                        print_value(out, other, level, &f.name);
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
            //
            // The width test needs each item rendered, and the one-line form
            // needs the same rendering at the same level — so it is done once
            // and kept. Rendering twice cost twice as much at every level of
            // nesting, which made formatting `[[[[…]]]]` exponential in its
            // depth: a build file nobody could format rather than one that
            // formats oddly.
            let flat: Vec<String> = items
                .iter()
                .map(|i| {
                    let mut s = String::new();
                    print_value(&mut s, i, level, within);
                    s
                })
                .collect();
            let inline = items.len() == 1
                && items.iter().all(|i| matches!(i, Value::Str(..) | Value::Ident(..) | Value::Int(..)));
            let width: usize = flat.iter().map(|s| s.len().saturating_add(2)).sum();
            if inline || (width <= 60 && items.iter().all(|i| !matches!(i, Value::Msg(..)))) {
                out.push('[');
                for (i, item) in flat.iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    out.push_str(item);
                }
                out.push(']');
            } else {
                out.push_str("[\n");
                for item in items {
                    indent(out, level.saturating_add(1));
                    print_value(out, item, level.saturating_add(1), within);
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

    /// A build file is user input, so no build file may crash the reader. Each
    /// of these once did or nearly did: the recovery paths stepped by a byte,
    /// which left the cursor inside a multi-byte character and made the next
    /// slice of the text a panic.
    #[test]
    fn no_build_file_can_crash_the_reader() {
        let refused = [
            "é",                          // recovery inside a character, at the top level
            "a: [é]",                     // and inside a list
            "a: \"\\",                    // a backslash at the end of the input
            "a: \"unclosed",              // an unterminated string
            "a: [1, 2",                   // an unterminated list
            "a {",                        // an unterminated message
            "a:",                         // a field with no value
            "{",                          // a value where a name belongs
            "a: 99999999999999999999999", // a number no `i64` holds
        ];
        for src in refused {
            let r = parse(src, FileId(0));
            assert!(!r.errors.is_empty(), "{src:?} was accepted");
            assert!(r.errors.iter().all(|e| e.fix.is_some()), "{src:?}: an error with no fix");
        }
        // An escape names a character rather than a byte, so `\é` is an `é`.
        assert_eq!(p("a: \"\\é\"\n").fields.len(), 1);
        assert_eq!(p("# comment é\na: 1\n").fields[0].comments, vec!["# comment é"]);
    }

    /// Nesting is read by recursion, so it is bounded: without the bound a file
    /// that is nothing but open brackets exhausts the stack, which is a crash
    /// with no diagnostic.
    #[test]
    fn pathological_nesting_is_a_diagnostic_rather_than_a_stack_overflow() {
        for deep in [format!("a: {}", "[".repeat(200_000)), "a{".repeat(200_000)] {
            let r = parse(&deep, FileId(0));
            assert!(
                r.errors.iter().any(|e| e.message.contains("nests too deeply")),
                "{:#?}",
                r.errors.first()
            );
        }
        // The bound is above anything a hand-written file reaches.
        let ok = format!("a: {}1{}", "[".repeat(30), "]".repeat(30));
        assert!(parse(&ok, FileId(0)).errors.is_empty());
    }

    /// Printing is linear in the nesting rather than exponential in it. It was
    /// the latter while each item was rendered once to measure and once to
    /// emit: that doubles the cost at every level, and formatting a list the
    /// reader happily accepts took minutes. This test simply would not finish.
    #[test]
    fn formatting_a_deeply_nested_list_costs_what_its_size_says() {
        let src = format!("a: {}1{}\n", "[".repeat(32), "]".repeat(32));
        let once = print(&p(&src));
        assert_eq!(once, print(&p(&once)));
    }

    /// The canonical order is the schema's, which is what makes `library`
    /// before `binary` (CLI.md) a consequence of a rule rather than a special
    /// case — and gives `REPO.buri` the same treatment from the same table.
    #[test]
    fn fields_come_back_in_the_schemas_order() {
        let out = print(&p("binary {\n  test {}\n  outputs: []\n  sources: []\n}\nlibrary {}\n"));
        assert_eq!(
            out,
            "library {}\n\nbinary {\n    sources: []\n    outputs: []\n    test {}\n}\n"
        );
    }

    /// A field the schema has never heard of is not the formatter's to move or
    /// to drop. It keeps its place at the end, where an unknown field's own
    /// diagnostic will find it.
    #[test]
    fn an_unknown_field_keeps_its_place() {
        let out = print(&p("library {\n  whatIsThis: 1\n  sources: []\n  andThis: 2\n}\n"));
        assert_eq!(
            out,
            "library {\n    sources: []\n    whatIsThis: 1\n    andThis: 2\n}\n"
        );
    }

    /// The order of two fields of the same name is the only thing about them
    /// that could carry meaning, so it is the one thing the sort does not touch.
    #[test]
    fn repeated_blocks_keep_the_order_they_were_written_in() {
        let out = print(&p("tag { name: \"z\" }\ntag { name: \"a\" }\n"));
        assert!(out.find("\"z\"") < out.find("\"a\""), "{out}");
    }

    /// Four, the same as the source formatter's. A build file and the code
    /// beside it are read by the same person on the same screen.
    #[test]
    fn the_indent_is_four_spaces() {
        assert_eq!(INDENT, 4);
        let out = print(&p("library { sources: [] }\n"));
        assert_eq!(out, "library {\n    sources: []\n}\n");
    }

    #[test]
    fn empty_message_stays_on_one_line() {
        let out = print(&p("library {}\n"));
        assert_eq!(out, "library {}\n");
    }
}
