//! The built-in `proto` language: `std/proto` checks it, and [`format`] lays
//! it out.
//!
//! ```text
//! message Point{int32 x=1;int32 y=2 [deprecated=true];}
//!   ->
//! message Point {
//!     int32 x = 1;
//!     int32 y = 2 [deprecated = true];
//! }
//! ```
//!
//! The formatter reads tokens and statements, not the protobuf grammar: which
//! words make a valid field is the check's business. So it moves whitespace and
//! comments and nothing else, and the tokens of its output are the tokens of its
//! input, in order. That is what makes it safe: a schema's meaning is its
//! tokens.
//!
//! - one statement per line, and one level of indent per block;
//! - an empty line between two statements survives, and two become one;
//! - a field's `[...]` options break one per line when they do not fit;
//! - a comment on a line of its own stays there, and one at the end of a line
//!   stays at the end of that statement's line.
//!
//! A string, a comment or a bracket that does not close leaves the file as it
//! is.

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
    LineComment,
    BlockComment,
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

    fn is_comment(&self) -> bool {
        matches!(self.kind, Kind::LineComment | Kind::BlockComment)
    }
}

/// Splits `text` into tokens, comments included. `None` for a string or a
/// block comment that does not close.
fn lex(text: &str) -> Option<Vec<Token>> {
    let cs: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    let mut newlines = 0;
    let at = |k: usize| cs.get(k).copied().unwrap_or('\0');
    let word_start = |c: char| c.is_ascii_alphabetic() || c == '_';
    let word_part = |c: char| c.is_ascii_alphanumeric() || c == '_' || c == '.';
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
        } else if c == '/' && at(i + 1) == '/' {
            while i < cs.len() && at(i) != '\n' {
                i += 1;
            }
            Kind::LineComment
        } else if c == '/' && at(i + 1) == '*' {
            i += 2;
            while !(at(i) == '*' && at(i + 1) == '/') {
                if i >= cs.len() {
                    return None;
                }
                i += 1;
            }
            i += 2;
            Kind::BlockComment
        } else if word_start(c) || (c == '.' && word_start(at(i + 1))) {
            i += 1;
            while word_part(at(i)) {
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
        if kind == Kind::LineComment {
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
    line: bool,
}

impl Comment {
    fn of(t: &Token) -> Comment {
        Comment { text: t.text.clone(), newlines: t.newlines, line: t.kind == Kind::LineComment }
    }
}

/// A token, and the comments between it and the next.
#[derive(Debug)]
struct Piece {
    token: Token,
    after: Vec<Comment>,
}

#[derive(Debug)]
enum Item {
    /// A comment on a line of its own.
    Comment(Comment),
    Statement(Statement),
}

impl Item {
    fn newlines(&self) -> usize {
        match self {
            Item::Comment(c) => c.newlines,
            Item::Statement(s) => s.newlines,
        }
    }
}

/// `...;`, or `... { ... }`.
#[derive(Debug)]
struct Statement {
    newlines: usize,
    /// Up to and including a `;`; for a block, up to its `{`.
    pieces: Vec<Piece>,
    block: Option<Block>,
    /// At the end of its last line.
    trailing: Vec<Comment>,
}

#[derive(Debug)]
struct Block {
    /// On the line the block opens.
    opening: Vec<Comment>,
    items: Vec<Item>,
    /// `};`, as some schemas write a block.
    semicolon: bool,
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

    /// Comments that start on the line the reader is on.
    fn same_line(&mut self) -> Vec<Comment> {
        let mut out = Vec::new();
        while let Some(t) = self.peek().filter(|t| t.is_comment() && t.newlines == 0) {
            out.push(Comment::of(t));
            self.at += 1;
        }
        out
    }

    /// Items up to the end of the file, or up to the `}` of a block.
    fn items(&mut self, nested: bool) -> Option<Vec<Item>> {
        let mut out = Vec::new();
        loop {
            let Some(t) = self.peek() else { return (!nested).then_some(out) };
            if t.is_comment() {
                out.push(Item::Comment(Comment::of(t)));
                self.at += 1;
            } else if t.is("}") {
                return nested.then_some(out);
            } else {
                out.push(Item::Statement(self.statement()?));
            }
        }
    }

    fn statement(&mut self) -> Option<Statement> {
        let newlines = self.peek()?.newlines;
        let mut pieces: Vec<Piece> = Vec::new();
        let mut depth = 0usize;
        loop {
            let t = self.next()?;
            if t.is_comment() {
                pieces.last_mut()?.after.push(Comment::of(&t));
                continue;
            }
            if depth == 0 && t.is(";") {
                pieces.push(Piece { token: t, after: Vec::new() });
                let trailing = self.same_line();
                return Some(Statement { newlines, pieces, block: None, trailing });
            }
            // A `{` after `=` or `:` is an option's value; any other opens a
            // block of statements.
            let value = pieces.last().is_some_and(|p| p.token.is("=") || p.token.is(":"));
            if depth == 0 && t.is("{") && !value {
                let opening = self.same_line();
                let items = self.items(true)?;
                self.next()?;
                let semicolon = self.peek().is_some_and(|t| t.is(";") && t.newlines == 0);
                if semicolon {
                    self.at += 1;
                }
                let trailing = self.same_line();
                let block = Some(Block { opening, items, semicolon });
                return Some(Statement { newlines, pieces, block, trailing });
            }
            if t.kind == Kind::Symbol && matches!(t.text.as_str(), "(" | "[" | "<" | "{") {
                depth += 1;
            }
            if t.kind == Kind::Symbol && matches!(t.text.as_str(), ")" | "]" | ">" | "}") {
                depth = depth.checked_sub(1)?;
            }
            pieces.push(Piece { token: t, after: Vec::new() });
        }
    }
}

/// The canonical text of a `.proto` file, or `None` when a string, a comment
/// or a bracket in it does not close.
pub fn format(text: &str) -> Option<String> {
    let mut reader = Reader { tokens: lex(text)?, at: 0 };
    let items = reader.items(false)?;
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
            Item::Comment(c) => out.push(own_line(c)),
            Item::Statement(s) => statement(s, out),
        }
    }
}

fn statement(s: &Statement, out: &mut Vec<Doc>) {
    out.push(pieces(&s.pieces));
    if let Some(block) = &s.block {
        out.push(Doc::text(" {"));
        out.extend(block.opening.iter().flat_map(suffix));
        if !block.items.is_empty() {
            let mut inner = vec![Doc::HardLine];
            lay_out(&block.items, &mut inner);
            out.push(Doc::Indent(inner));
        }
        if !block.items.is_empty() || !block.opening.is_empty() {
            out.push(Doc::HardLine);
        }
        out.push(Doc::text(if block.semicolon { "};" } else { "}" }));
    }
    out.extend(s.trailing.iter().flat_map(suffix));
}

/// A comment on a line of its own. A block comment whose lines all start with
/// `*` is re-indented; any other is written as it stands.
fn own_line(c: &Comment) -> Doc {
    let mut lines = c.text.lines();
    let first = lines.next().unwrap_or_default().to_string();
    let rest: Vec<&str> = lines.collect();
    if rest.is_empty() || !rest.iter().all(|l| l.trim_start().starts_with('*')) {
        return Doc::Text(c.text.clone());
    }
    let mut parts = vec![Doc::Text(first)];
    for l in rest {
        parts.push(Doc::HardLine);
        parts.push(Doc::Text(format!(" {}", l.trim_start())));
    }
    Doc::Concat(parts)
}

/// A comment at the end of the line it follows.
fn suffix(c: &Comment) -> [Doc; 2] {
    [Doc::LineSuffix(format!(" {}", c.text)), Doc::BreakParent]
}

/// One token and the comments after it.
fn piece(p: &Piece) -> Doc {
    let mut parts = vec![Doc::Text(p.token.text.clone())];
    for c in &p.after {
        match c.line {
            true => parts.extend(suffix(c)),
            false => parts.push(Doc::Text(format!(" {}", c.text))),
        }
    }
    Doc::Concat(parts)
}

/// Whether a line comment ends `p`, so what follows it starts a new line.
fn ends_line(p: &Piece) -> bool {
    p.after.last().is_some_and(|c| c.line)
}

/// Tokens on one line, except that a field's `[...]` options break one per
/// line when they do not fit.
fn pieces(ps: &[Piece]) -> Doc {
    let mut out = Vec::new();
    let mut k = 0;
    let mut prev: Option<&Piece> = None;
    while let Some(p) = ps.get(k) {
        if let Some(prev) = prev {
            if ends_line(prev) {
                out.push(Doc::Indent(vec![Doc::HardLine]));
            } else if spaced(&prev.token, &p.token) {
                out.push(Doc::text(" "));
            }
        }
        let options = p.token.is("[") && prev.is_some_and(|q| matches!(q.token.kind, Kind::Number | Kind::Word));
        let group = options.then(|| matching(ps, k)).flatten().and_then(|close| {
            let doc = options_group(p, ps.get(k + 1..close)?, ps.get(close)?);
            Some((doc, close))
        });
        match group {
            Some((doc, close)) => {
                out.push(doc);
                prev = ps.get(close);
                k = close + 1;
            }
            None => {
                out.push(piece(p));
                prev = Some(p);
                k += 1;
            }
        }
    }
    Doc::Concat(out)
}

/// The index of the `]` that closes the `[` at `open`.
fn matching(ps: &[Piece], open: usize) -> Option<usize> {
    let mut depth = 0usize;
    for (i, p) in ps.iter().enumerate().skip(open) {
        if p.token.kind != Kind::Symbol {
            continue;
        }
        match p.token.text.as_str() {
            "(" | "[" | "<" | "{" => depth += 1,
            ")" | "]" | ">" | "}" => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return (p.token.text == "]").then_some(i);
                }
            }
            _ => {}
        }
    }
    None
}

/// `[a = 1, b = 2]`, or one option per line.
fn options_group(open: &Piece, inside: &[Piece], close: &Piece) -> Doc {
    let mut inner = vec![Doc::SoftLine];
    let mut depth = 0usize;
    let mut start = 0;
    for (i, p) in inside.iter().enumerate() {
        let t = &p.token;
        if t.kind == Kind::Symbol && matches!(t.text.as_str(), "(" | "[" | "<" | "{") {
            depth += 1;
        }
        if t.kind == Kind::Symbol && matches!(t.text.as_str(), ")" | "]" | ">" | "}") {
            depth = depth.saturating_sub(1);
        }
        if depth > 0 || !t.is(",") {
            continue;
        }
        let option = inside.get(start..i).unwrap_or_default();
        inner.push(pieces(option));
        if option.last().is_some_and(ends_line) {
            inner.push(Doc::Indent(vec![Doc::HardLine]));
        }
        inner.push(piece(p));
        inner.push(Doc::Line);
        start = i + 1;
    }
    let last = inside.get(start..).unwrap_or_default();
    inner.push(pieces(last));
    if last.last().is_some_and(ends_line) {
        inner.push(Doc::HardLine);
    }
    Doc::Concat(vec![
        Doc::Group(vec![piece(open), Doc::Indent(inner), Doc::SoftLine, Doc::text("]")]),
        Doc::Concat(close.after.iter().flat_map(suffix).collect()),
    ])
}

/// Whether a space separates two tokens on one line.
fn spaced(a: &Token, b: &Token) -> bool {
    let sym = |t: &Token, set: &[&str]| t.kind == Kind::Symbol && set.contains(&t.text.as_str());
    if sym(b, &[";", ",", ")", "]", ">", ":", "<"]) || sym(a, &["(", "[", "<", "-", "+"]) {
        return false;
    }
    if b.kind == Kind::Word && b.text.starts_with('.') && a.is(")") {
        return false;
    }
    if b.is("(") && a.kind == Kind::Word && !matches!(a.text.as_str(), "option" | "returns") {
        return false;
    }
    !(a.is("{") && b.is("}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fmt(text: &str) -> String {
        format(text).unwrap_or_else(|| panic!("does not format:\n{text}"))
    }

    #[test]
    fn statements_go_one_per_line_and_blocks_indent() {
        let text = "edition=\"2026\";package a.v1;\nmessage Point{int32 x=1;int32 y=2 [deprecated=true];message Empty{}}";
        let want = "edition = \"2026\";\npackage a.v1;\nmessage Point {\n    int32 x = 1;\n    int32 y = 2 [deprecated = true];\n    message Empty {}\n}\n";
        assert_eq!(fmt(text), want);
    }

    #[test]
    fn blank_lines_collapse_to_one_and_leave_block_edges() {
        let text = "\n\nedition = \"2026\";\n\n\n\nmessage A {\n\n  int32 a = 1;\n\n\n  int32 b = 2;\n\n}\n\n";
        assert_eq!(fmt(text), "edition = \"2026\";\n\nmessage A {\n    int32 a = 1;\n\n    int32 b = 2;\n}\n");
    }

    #[test]
    fn every_comment_survives() {
        let text = "// head\n\n/* block\n   * star\n   */\nmessage A { // opens\n  // above\n  int32 a = 1; // beside\n  int32 b /* inside */ = 2;\n  // last\n} // closes\n// end\n";
        let want = "// head\n\n/* block\n * star\n */\nmessage A { // opens\n    // above\n    int32 a = 1; // beside\n    int32 b /* inside */ = 2;\n    // last\n} // closes\n// end\n";
        assert_eq!(fmt(text), want);
    }

    #[test]
    fn long_options_break_one_per_line() {
        let long = "x".repeat(60);
        let text = format!("message A {{ string s = 1 [json_name = \"{long}\", deprecated = true]; }}");
        let want = format!("message A {{\n    string s = 1 [\n        json_name = \"{long}\",\n        deprecated = true\n    ];\n}}\n");
        assert_eq!(fmt(&text), want);
    }

    #[test]
    fn values_options_and_names_keep_their_punctuation() {
        let text = "option (my.ext).field = { a: 1 b: [1, 2] };\nenum E { A = 0; B = -1 [(x) = 2]; }\nmessage M { map<string, int32> m = 1; .pkg.T t = 2; }\nservice S { rpc Get(stream Req) returns (Resp); }\nmessage N {};\n";
        let want = "option (my.ext).field = { a: 1 b: [1, 2] };\nenum E {\n    A = 0;\n    B = -1 [(x) = 2];\n}\nmessage M {\n    map<string, int32> m = 1;\n    .pkg.T t = 2;\n}\nservice S {\n    rpc Get(stream Req) returns (Resp);\n}\nmessage N {};\n";
        assert_eq!(fmt(text), want);
    }

    #[test]
    fn what_does_not_close_is_refused() {
        for broken in ["message A {", "message A { int32 a = 1; }}", "string s = \"open;", "/* open", "a = (1;"] {
            assert_eq!(format(broken), None, "{broken}");
        }
    }

    /// Non-comment tokens, and comments with their whitespace squeezed.
    fn meaning(text: &str) -> (Vec<String>, Vec<String>) {
        let tokens = lex(text).expect("lexes");
        let (comments, rest): (Vec<Token>, Vec<Token>) = tokens.into_iter().partition(Token::is_comment);
        let squeeze = |t: Token| t.text.split_whitespace().collect::<Vec<_>>().join(" ");
        (rest.into_iter().map(|t| t.text).collect(), comments.into_iter().map(squeeze).collect())
    }

    /// Every `.proto` file under `cli/tests`, the vendored conformance schemas
    /// among them: a fixed point, the same tokens, and every comment kept.
    #[test]
    fn every_schema_in_the_tests_formats_to_a_fixed_point_with_its_meaning() {
        let mut files = Vec::new();
        let mut stack = vec![std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests")];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).expect("reads").flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else if path.extension().is_some_and(|e| e == "proto") {
                    files.push(path);
                }
            }
        }
        assert!(files.len() >= 20, "found {} schemas", files.len());
        let mut refused = Vec::new();
        for path in files {
            let text = std::fs::read_to_string(&path).expect("reads");
            let Some(once) = format(&text) else {
                refused.push(path.display().to_string());
                continue;
            };
            assert_eq!(format(&once).as_deref(), Some(once.as_str()), "{} is not a fixed point", path.display());
            assert_eq!(meaning(&once), meaning(&text), "{} changed meaning", path.display());
        }
        // Only the fixtures broken on purpose: a message or a string that
        // never closes.
        assert!(refused.iter().all(|p| p.ends_with("/broken.proto")), "{refused:?}");
    }
}
