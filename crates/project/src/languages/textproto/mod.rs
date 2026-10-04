//! The built-in `textproto` language: the `textproto` tool checks it, and
//! [`format`] lays it out.
//!
//! ```text
//! name:"api";ports:[80,443] limits<cpu:0.5>
//!   ->
//! name: "api"
//! ports: [80, 443]
//! limits {
//!     cpu: 0.5
//! }
//! ```
//!
//! Like the `.proto` formatter, this one reads tokens and nesting, not a
//! message: whether a field exists is the check's business. It changes only
//! what carries no value:
//!
//! - one field per line, and one level of indent per message;
//! - `name: value` for a scalar or a list, `name { ... }` for a message, and
//!   `< ... >` written as `{ ... }`;
//! - the `;` or `,` after a field goes;
//! - a list stays on one line when it fits, and breaks one value per line when
//!   it does not; a message in a list sits on one line too, unless it holds a
//!   comment, a list or a message;
//! - an empty line between two fields survives, and two become one;
//! - a comment on a line of its own stays there, and one at the end of a line
//!   stays at the end of that field's line.
//!
//! Strings, numbers and words are written exactly as they were. A string, a
//! bracket or a message that does not close leaves the file as it is.

#![allow(
    clippy::arithmetic_side_effects,
    reason = "every operand is an index into, or a count of, the characters or tokens of a file \
              already in memory, so a sum is bounded by that length; `i - 1` follows a step past \
              the first character"
)]

use crate::layout::Doc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Word,
    Number,
    Str,
    Symbol,
    Comment,
}

#[derive(Clone, Debug)]
struct Token {
    kind: Kind,
    text: String,
    /// Line breaks between the previous token and this one.
    newlines: usize,
}

impl Token {
    fn is(&self, symbol: &str) -> bool {
        self.kind == Kind::Symbol && self.text == symbol
    }
}

/// Splits `text` into tokens, comments included. `None` for a string that does
/// not close on its line.
fn lex(text: &str) -> Option<Vec<Token>> {
    let cs: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    let mut newlines = 0;
    let at = |k: usize| cs.get(k).copied().unwrap_or('\0');
    let word_start = |c: char| c.is_ascii_alphabetic() || c == '_';
    while i < cs.len() {
        let c = at(i);
        let start = i;
        let kind = if c == '\n' {
            newlines += 1;
            i += 1;
            continue;
        } else if c.is_whitespace() || c == '\u{feff}' {
            i += 1;
            continue;
        } else if c == '#' {
            while i < cs.len() && at(i) != '\n' {
                i += 1;
            }
            Kind::Comment
        } else if word_start(c) {
            while at(i).is_ascii_alphanumeric() || at(i) == '_' {
                i += 1;
            }
            Kind::Word
        } else if c.is_ascii_digit() || (c == '.' && at(i + 1).is_ascii_digit()) {
            let hex = c == '0' && matches!(at(i + 1), 'x' | 'X');
            i += 1;
            loop {
                let d = at(i);
                let exponent = matches!(d, '+' | '-') && !hex && matches!(at(i - 1), 'e' | 'E');
                if !(d.is_ascii_alphanumeric() || d == '_' || d == '.' || exponent) {
                    break;
                }
                i += 1;
            }
            Kind::Number
        } else if c == '"' || c == '\'' {
            i += 1;
            loop {
                if i >= cs.len() || at(i) == '\n' {
                    return None;
                }
                match at(i) {
                    '\\' => i += 2,
                    d if d == c => break,
                    _ => i += 1,
                }
            }
            i += 1;
            Kind::Str
        } else {
            i += 1;
            Kind::Symbol
        };
        let mut text: String = cs.get(start..i.min(cs.len()))?.iter().collect();
        if kind == Kind::Comment {
            text.truncate(text.trim_end().len());
        }
        out.push(Token { kind, text, newlines });
        newlines = 0;
    }
    Some(out)
}

#[derive(Debug)]
struct Comment {
    text: String,
    newlines: usize,
}

#[derive(Debug)]
enum Item {
    /// A comment on a line of its own.
    Comment(Comment),
    Field(Field),
}

impl Item {
    fn newlines(&self) -> usize {
        match self {
            Item::Comment(c) => c.newlines,
            Item::Field(f) => f.newlines,
        }
    }
}

#[derive(Debug)]
struct Field {
    newlines: usize,
    name: String,
    value: Value,
    /// Comments inside the field, and at the end of its line: each goes at the
    /// end of the line it lands on.
    after: Vec<Comment>,
}

#[derive(Debug)]
enum Value {
    /// Adjacent strings, or one number or word with its sign.
    Scalar(Vec<String>),
    Message(Block),
    List(List),
}

#[derive(Debug)]
struct Block {
    /// On the line the message opens.
    opening: Vec<Comment>,
    items: Vec<Item>,
}

#[derive(Debug)]
struct List {
    opening: Vec<Comment>,
    elements: Vec<Element>,
    /// Comments on lines of their own before the `]`.
    closing: Vec<Comment>,
}

#[derive(Debug)]
struct Element {
    /// Comments on lines of their own before it.
    before: Vec<Comment>,
    value: Value,
    after: Vec<Comment>,
}

struct Reader {
    tokens: Vec<Token>,
    at: usize,
}

impl Reader {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.at)
    }

    fn next(&mut self) -> Option<Token> {
        let t = self.tokens.get(self.at)?.clone();
        self.at += 1;
        Some(t)
    }

    fn eat(&mut self, symbol: &str) -> bool {
        let found = self.peek().is_some_and(|t| t.is(symbol));
        if found {
            self.at += 1;
        }
        found
    }

    /// Every comment from here, wherever it sits.
    fn comments(&mut self) -> Vec<Comment> {
        let mut out = Vec::new();
        while let Some(t) = self.peek().filter(|t| t.kind == Kind::Comment) {
            out.push(Comment { text: t.text.clone(), newlines: t.newlines });
            self.at += 1;
        }
        out
    }

    /// Comments that start on the line the reader is on.
    fn same_line(&mut self) -> Vec<Comment> {
        let mut out = Vec::new();
        while let Some(t) = self.peek().filter(|t| t.kind == Kind::Comment && t.newlines == 0) {
            out.push(Comment { text: t.text.clone(), newlines: 0 });
            self.at += 1;
        }
        out
    }

    /// Items up to the end of the file, or up to `close`.
    fn items(&mut self, close: Option<&str>) -> Option<Vec<Item>> {
        let mut out = Vec::new();
        loop {
            let Some(t) = self.peek() else { return close.is_none().then_some(out) };
            if t.kind == Kind::Comment {
                out.push(Item::Comment(Comment { text: t.text.clone(), newlines: t.newlines }));
                self.at += 1;
            } else if close.is_some_and(|c| t.is(c)) {
                return Some(out);
            } else {
                out.push(Item::Field(self.field()?));
            }
        }
    }

    fn field(&mut self) -> Option<Field> {
        let first = self.next()?;
        let newlines = first.newlines;
        let name = match first.kind {
            Kind::Word => first.text,
            // `[pkg.ext]` or `[type.googleapis.com/pkg.Msg]`, as one name.
            Kind::Symbol if first.text == "[" => {
                let mut name = String::from("[");
                loop {
                    let t = self.next()?;
                    if t.kind == Kind::Comment {
                        return None;
                    }
                    name.push_str(&t.text);
                    if t.is("]") {
                        break name;
                    }
                }
            }
            _ => return None,
        };
        let mut after = self.comments();
        self.eat(":");
        after.extend(self.comments());
        let value = self.value()?;
        // A comment on a line of its own after the value belongs to what
        // follows.
        after.extend(self.same_line());
        if self.eat(";") || self.eat(",") {
            after.extend(self.same_line());
        }
        Some(Field { newlines, name, value, after })
    }

    fn value(&mut self) -> Option<Value> {
        let t = self.next()?;
        match t.kind {
            Kind::Symbol if t.text == "{" || t.text == "<" => {
                let close = if t.text == "{" { "}" } else { ">" };
                let opening = self.same_line();
                let items = self.items(Some(close))?;
                self.next()?;
                Some(Value::Message(Block { opening, items }))
            }
            Kind::Symbol if t.text == "[" => self.list(),
            Kind::Symbol if t.text == "-" => {
                let n = self.next().filter(|n| matches!(n.kind, Kind::Number | Kind::Word))?;
                Some(Value::Scalar(vec![format!("-{}", n.text)]))
            }
            Kind::Number | Kind::Word => Some(Value::Scalar(vec![t.text])),
            Kind::Str => {
                let mut parts = vec![t.text];
                while let Some(s) = self.peek().filter(|s| s.kind == Kind::Str) {
                    parts.push(s.text.clone());
                    self.at += 1;
                }
                Some(Value::Scalar(parts))
            }
            _ => None,
        }
    }

    fn list(&mut self) -> Option<Value> {
        let opening = self.same_line();
        let mut elements = Vec::new();
        loop {
            let before = self.comments();
            if self.eat("]") {
                return Some(Value::List(List { opening, elements, closing: before }));
            }
            let value = self.value()?;
            let mut after = self.same_line();
            let comma = self.eat(",");
            after.extend(self.same_line());
            elements.push(Element { before, value, after });
            if !comma {
                let closing = self.comments();
                if !self.eat("]") {
                    return None;
                }
                return Some(Value::List(List { opening, elements, closing }));
            }
        }
    }
}

/// The canonical text of a text format file, or `None` when a string, a
/// message or a list in it does not close.
pub fn format(text: &str) -> Option<String> {
    let mut reader = Reader { tokens: lex(text)?, at: 0 };
    let items = reader.items(None)?;
    if items.is_empty() {
        return Some(String::new());
    }
    let mut out = Vec::new();
    lay_out(&items, &mut out);
    out.push(Doc::HardLine);
    Some(crate::layout::render(&Doc::Concat(out)))
}

fn lay_out(items: &[Item], out: &mut Vec<Doc>) {
    for (i, item) in items.iter().enumerate() {
        if i > 0 {
            out.push(Doc::HardLine);
            if item.newlines() >= 2 {
                out.push(Doc::HardLine);
            }
        }
        match item {
            Item::Comment(c) => out.push(Doc::Text(c.text.clone())),
            Item::Field(f) => field(f, out),
        }
    }
}

fn field(f: &Field, out: &mut Vec<Doc>) {
    out.push(Doc::Text(f.name.clone()));
    match &f.value {
        Value::Message(_) => out.push(Doc::text(" ")),
        _ => out.push(Doc::text(": ")),
    }
    out.push(value(&f.value));
    out.extend(f.after.iter().flat_map(suffix));
}

/// A comment at the end of the line it lands on.
fn suffix(c: &Comment) -> [Doc; 2] {
    [Doc::LineSuffix(format!(" {}", c.text)), Doc::BreakParent]
}

fn value(v: &Value) -> Doc {
    match v {
        Value::Scalar(parts) => match parts.as_slice() {
            [one] => Doc::Text(one.clone()),
            [first, rest @ ..] => {
                let mut inner = Vec::new();
                for p in rest {
                    inner.push(Doc::Line);
                    inner.push(Doc::Text(p.clone()));
                }
                Doc::Group(vec![Doc::Text(first.clone()), Doc::Indent(inner)])
            }
            [] => Doc::Concat(Vec::new()),
        },
        Value::Message(block) => {
            let mut out = vec![Doc::text("{")];
            out.extend(block.opening.iter().flat_map(suffix));
            if !block.items.is_empty() {
                let mut inner = vec![Doc::HardLine];
                lay_out(&block.items, &mut inner);
                out.push(Doc::Indent(inner));
            }
            if !block.items.is_empty() || !block.opening.is_empty() {
                out.push(Doc::HardLine);
            }
            out.push(Doc::text("}"));
            Doc::Concat(out)
        }
        Value::List(list) => {
            if list.elements.is_empty() && list.opening.is_empty() && list.closing.is_empty() {
                return Doc::text("[]");
            }
            let mut inner = vec![Doc::SoftLine];
            for (i, e) in list.elements.iter().enumerate() {
                if i > 0 {
                    inner.push(Doc::Line);
                }
                for c in &e.before {
                    inner.push(Doc::Text(c.text.clone()));
                    inner.push(Doc::HardLine);
                }
                inner.push(match &e.value {
                    Value::Message(block) if plain(block) => inline(block),
                    other => value(other),
                });
                if i + 1 < list.elements.len() {
                    inner.push(Doc::text(","));
                }
                inner.extend(e.after.iter().flat_map(suffix));
            }
            for c in &list.closing {
                inner.push(Doc::HardLine);
                inner.push(Doc::Text(c.text.clone()));
            }
            let mut out = vec![Doc::text("[")];
            out.extend(list.opening.iter().flat_map(suffix));
            out.push(Doc::Indent(inner));
            out.push(Doc::SoftLine);
            out.push(Doc::text("]"));
            Doc::Group(out)
        }
    }
}

/// Whether a message in a list may sit on one line: scalar fields only, and no
/// comment anywhere in it.
fn plain(block: &Block) -> bool {
    block.opening.is_empty()
        && block.items.iter().all(|item| match item {
            Item::Comment(_) => false,
            Item::Field(f) => f.after.is_empty() && matches!(f.value, Value::Scalar(_)),
        })
}

/// `{ a: 1 b: 2 }` when it fits, and one field per line when it does not.
fn inline(block: &Block) -> Doc {
    if block.items.is_empty() {
        return Doc::text("{}");
    }
    let mut inner = Vec::new();
    for item in &block.items {
        inner.push(Doc::Line);
        if let Item::Field(f) = item {
            field(f, &mut inner);
        }
    }
    Doc::Group(vec![Doc::text("{"), Doc::Indent(inner), Doc::Line, Doc::text("}")])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fmt(text: &str) -> String {
        format(text).unwrap_or_else(|| panic!("does not format:\n{text}"))
    }

    #[test]
    fn fields_go_one_per_line_and_messages_indent() {
        let text = "name:\"api\";ports:[80,443] limits<cpu:0.5>empty{}";
        let want = "name: \"api\"\nports: [80, 443]\nlimits {\n    cpu: 0.5\n}\nempty {}\n";
        assert_eq!(fmt(text), want);
    }

    #[test]
    fn blank_lines_collapse_to_one_and_leave_message_edges() {
        let text = "\n\na: 1\n\n\n\nb {\n\n  c: 2\n\n\n  d: 3\n\n}\n\n";
        assert_eq!(fmt(text), "a: 1\n\nb {\n    c: 2\n\n    d: 3\n}\n");
    }

    #[test]
    fn every_comment_survives() {
        let text = "# proto-file: a.proto\n# proto-message: A\n\nb { # opens\n  # above\n  c: 1 # beside\n  d # odd\n  : 2\n  # last\n} # closes\nl: [ # list\n  1, # one\n  # two next\n  2\n  # end\n]\n# end\n";
        let want = "# proto-file: a.proto\n# proto-message: A\n\nb { # opens\n    # above\n    c: 1 # beside\n    d: 2 # odd\n    # last\n} # closes\nl: [ # list\n    1, # one\n    # two next\n    2\n    # end\n]\n# end\n";
        assert_eq!(fmt(text), want);
        assert_eq!(fmt(want), want);
    }

    #[test]
    fn a_long_list_breaks_one_value_per_line() {
        let long = "x".repeat(40);
        let text = format!("names: [\"{long}\", \"{long}\"]");
        let want = format!("names: [\n    \"{long}\",\n    \"{long}\"\n]\n");
        assert_eq!(fmt(&text), want);
        assert_eq!(fmt(&want), want);
    }

    #[test]
    fn values_keep_their_spelling() {
        let text = "a: -0x1F b: - 1.5f c: 'it''s' \"x\" d: -inf e: [{x: 1}, <y: 2>] [pkg.ext]: 3";
        let want = "a: -0x1F\nb: -1.5f\nc: 'it' 's' \"x\"\nd: -inf\ne: [{ x: 1 }, { y: 2 }]\n[pkg.ext]: 3\n";
        assert_eq!(fmt(text), want);
        assert_eq!(fmt(want), want);
    }

    /// The tokens that carry the value: every string, number and word in
    /// order, a sign joined to what follows it, and `<` `>` read as braces.
    fn meaning(text: &str) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        let mut sign = false;
        for t in lex(text).expect("lexes") {
            match t.kind {
                Kind::Comment => {}
                Kind::Symbol if t.text == "-" => sign = true,
                Kind::Symbol if matches!(t.text.as_str(), ":" | ";" | ",") => {}
                Kind::Symbol if t.text == "<" => out.push("{".to_string()),
                Kind::Symbol if t.text == ">" => out.push("}".to_string()),
                _ if sign => {
                    out.push(format!("-{}", t.text));
                    sign = false;
                }
                _ => out.push(t.text),
            }
        }
        out
    }

    fn comments(text: &str) -> Vec<String> {
        lex(text).expect("lexes").into_iter().filter(|t| t.kind == Kind::Comment).map(|t| t.text).collect()
    }

    /// Every text format file under `cli/tests`: a fixed point, the same
    /// value, and every comment kept.
    #[test]
    fn every_text_format_file_in_the_tests_formats_to_a_fixed_point_with_its_meaning() {
        let mut files = Vec::new();
        let mut stack = vec![std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../cli/tests")];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).expect("reads").flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else if path.extension().is_some_and(|e| e == "txtpb" || e == "textproto") {
                    files.push(path);
                }
            }
        }
        assert!(files.len() >= 20, "found {} files", files.len());
        let mut refused = Vec::new();
        for path in files {
            let text = std::fs::read_to_string(&path).expect("reads");
            let Some(once) = format(&text) else {
                refused.push(path.display().to_string());
                continue;
            };
            assert_eq!(format(&once).as_deref(), Some(once.as_str()), "{} is not a fixed point", path.display());
            assert_eq!(meaning(&once), meaning(&text), "{} changed meaning", path.display());
            assert_eq!(comments(&once), comments(&text), "{} lost a comment", path.display());
        }
        // The repository-case manifests are text format files too. Only the
        // fixtures broken on purpose are refused: a string that never closes,
        // and a list missing its comma.
        assert!(
            refused.iter().all(|p| p.ends_with("/broken.txtpb") || p.ends_with("/syntax.txtpb")),
            "{refused:?}"
        );
    }

    #[test]
    fn what_does_not_close_is_refused() {
        for broken in ["a {", "a: \"open", "a: [1, 2", "a: }", "a: [1 2]", "a {}}", ": 1"] {
            assert_eq!(format(broken), None, "{broken}");
        }
    }
}
