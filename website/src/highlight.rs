//! Syntax highlighting, at build time, by the toolchain's own front end.
//!
//! A `buri` block is lexed by `parsing::lexer` and a `textproto` block is
//! parsed by `build::textproto` — the same code the compiler and `buri gen`
//! run. So a page cannot colour a word as a keyword that the lexer does not
//! think is one, and a keyword added to the language is highlighted here the
//! day it is added. Nothing ships to the browser: the classes are in the HTML.
//!
//! Every other language is left alone. A shell transcript or a diagnostic
//! listing has no lexer here, and guessing at one is how a highlighter starts
//! being wrong in public.
#![allow(
    clippy::arithmetic_side_effects,
    reason = "every offset here is a byte position inside the snippet being \
              highlighted, and each is compared against its length before use"
)]

use buri::build::textproto;
use buri::diagnostics::{FileId, Span};
use buri::parsing::lexer::{self, TokenKind};

/// The classes a snippet can carry. They are the lexer's distinctions, not a
/// theme's: a theme decides what `keyword` looks like, and this decides what
/// is one.
pub const CLASSES: &[&str] =
    &["keyword", "string", "number", "comment", "operator", "identifier", "type"];

/// One highlighted block's inner HTML — what goes inside `<code>`.
pub fn block(language: &str, body: &str) -> String {
    match language {
        "buri" => buri_source(body),
        "textproto" => build_file(body),
        _ => {
            let mut out = String::with_capacity(body.len());
            escape(body, &mut out);
            out
        }
    }
}

/// Whether a fence's language is one the site highlights.
pub fn highlights(language: &str) -> bool {
    matches!(language, "buri" | "textproto")
}

/// A Buri snippet, token by token.
///
/// Comments are not tokens — the lexer hands them to the formatter as trivia —
/// so they are found in the gaps between tokens, which is also where the
/// whitespace is. That makes the walk total: every byte of the snippet is
/// either inside a token's span or inside a gap, and both are emitted.
fn buri_source(text: &str) -> String {
    let lexed = lexer::lex(text, FileId::NONE);
    let tokens = &lexed.tokens;
    let mut out = String::with_capacity(text.len().saturating_mul(2));
    let mut at = 0usize;
    for index in 0..tokens.len() {
        let kind = tokens.kind(index);
        if kind == TokenKind::Eof {
            break;
        }
        let Span { start, end, .. } = tokens.span(index);
        let (start, end) = (start as usize, end as usize);
        if start < at || end > text.len() || end < start {
            continue;
        }
        gap(text.get(at..start).unwrap_or(""), &mut out);
        let written = text.get(start..end).unwrap_or("");
        paint(class_of(kind, written), written, &mut out);
        at = end;
    }
    gap(text.get(at..).unwrap_or(""), &mut out);
    out
}

/// What is between two tokens: whitespace, and comments.
fn gap(text: &str, out: &mut String) {
    let bytes = text.as_bytes();
    let mut at = 0usize;
    let mut plain = 0usize;
    while at < bytes.len() {
        let rest = text.get(at..).unwrap_or("");
        let comment = if rest.starts_with("//") {
            Some(rest.find('\n').unwrap_or(rest.len()))
        } else if rest.starts_with("/*") {
            Some(rest.find("*/").map(|end| end + 2).unwrap_or(rest.len()))
        } else {
            None
        };
        match comment {
            Some(length) => {
                escape(text.get(plain..at).unwrap_or(""), out);
                paint(Some("comment"), rest.get(..length).unwrap_or(""), out);
                at += length;
                plain = at;
            }
            None => at += next_character(bytes, at),
        }
    }
    escape(text.get(plain..).unwrap_or(""), out);
}

/// The width of the character starting at `at`, so the walk always lands on a
/// character boundary in a snippet that is not all ASCII.
fn next_character(bytes: &[u8], at: usize) -> usize {
    let first = bytes.get(at).copied().unwrap_or(0);
    match first {
        0x00..=0x7f => 1,
        0xc0..=0xdf => 2,
        0xe0..=0xef => 3,
        _ => 4,
    }
}

/// A token's class. The kinds are the lexer's own, so this is a translation
/// rather than a judgement — except for the last line, which is the one
/// convention the lexer has no opinion about: a capitalized identifier is a
/// type in every Buri program, because that is what the style rules say.
fn class_of(kind: TokenKind, written: &str) -> Option<&'static str> {
    if kind.as_keyword().is_some() {
        return Some("keyword");
    }
    match kind {
        TokenKind::Str
        | TokenKind::Char
        | TokenKind::TemplateHead
        | TokenKind::TemplateSpan
        | TokenKind::TemplateTail => Some("string"),
        TokenKind::Int | TokenKind::Float => Some("number"),
        TokenKind::Ident => {
            if written.chars().next().is_some_and(char::is_uppercase) {
                Some("type")
            } else {
                Some("identifier")
            }
        }
        _ if is_delimiter(kind) => None,
        _ if kind.as_punctuation().is_some() => Some("operator"),
        _ => None,
    }
}

/// The punctuation that holds a program's shape rather than computing
/// anything. Colouring a brace like a `+` makes a block hard to see.
fn is_delimiter(kind: TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::LBrace
            | TokenKind::RBrace
            | TokenKind::LParen
            | TokenKind::RParen
            | TokenKind::LBracket
            | TokenKind::RBracket
            | TokenKind::Comma
            | TokenKind::Semi
            | TokenKind::Colon
            | TokenKind::Dot
    )
}

/// A `BUILD.buri` or `REPO.buri` snippet, from the build-file parser's own
/// tree.
///
/// Only the leaves are painted. A message's span covers everything inside its
/// braces, so painting it would paint the fields it contains twice; the walk
/// descends into it instead and paints the names and scalars it finds.
fn build_file(text: &str) -> String {
    let parsed = textproto::parse(text, FileId::NONE);
    let mut spans: Vec<(usize, usize, &'static str)> = Vec::new();
    collect_fields(&parsed.document.fields, &mut spans);
    spans.sort_by_key(|(start, end, _)| (*start, *end));

    let mut out = String::with_capacity(text.len().saturating_mul(2));
    let mut at = 0usize;
    for (start, end, class) in spans {
        if start < at || end > text.len() || end < start {
            continue;
        }
        build_file_gap(text.get(at..start).unwrap_or(""), &mut out);
        paint(Some(class), text.get(start..end).unwrap_or(""), &mut out);
        at = end;
    }
    build_file_gap(text.get(at..).unwrap_or(""), &mut out);
    out
}

fn collect_fields(fields: &[textproto::Field], spans: &mut Vec<(usize, usize, &'static str)>) {
    for field in fields {
        push_span(field.name_span, "keyword", spans);
        collect_value(&field.value, spans);
    }
}

fn collect_value(value: &textproto::Value, spans: &mut Vec<(usize, usize, &'static str)>) {
    match value {
        textproto::Value::Str(_, span) => push_span(*span, "string", spans),
        textproto::Value::Int(_, span) => push_span(*span, "number", spans),
        textproto::Value::Ident(written, span) => {
            let class = if written == "true" || written == "false" { "keyword" } else { "type" };
            push_span(*span, class, spans);
        }
        textproto::Value::Message(message, _) => collect_fields(&message.fields, spans),
        textproto::Value::List(values, _) => {
            for value in values {
                collect_value(value, spans);
            }
        }
    }
}

fn push_span(span: Span, class: &'static str, spans: &mut Vec<(usize, usize, &'static str)>) {
    spans.push((span.start as usize, span.end as usize, class));
}

/// Between two painted spans in a build file: whitespace, punctuation, and
/// `#` comments.
fn build_file_gap(text: &str, out: &mut String) {
    let bytes = text.as_bytes();
    let mut at = 0usize;
    let mut plain = 0usize;
    while at < bytes.len() {
        let rest = text.get(at..).unwrap_or("");
        if rest.starts_with('#') {
            let length = rest.find('\n').unwrap_or(rest.len());
            escape(text.get(plain..at).unwrap_or(""), out);
            paint(Some("comment"), rest.get(..length).unwrap_or(""), out);
            at += length;
            plain = at;
            continue;
        }
        at += next_character(bytes, at);
    }
    escape(text.get(plain..).unwrap_or(""), out);
}

fn paint(class: Option<&str>, text: &str, out: &mut String) {
    if text.is_empty() {
        return;
    }
    match class {
        Some(class) => {
            out.push_str("<span class=\"");
            out.push_str(class);
            out.push_str("\">");
            escape(text, out);
            out.push_str("</span>");
        }
        None => escape(text, out),
    }
}

/// The four characters that are markup in an HTML body or attribute.
pub fn escape(text: &str, out: &mut String) {
    for character in text.chars() {
        match character {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            other => out.push(other),
        }
    }
}

pub fn escaped(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    escape(text, &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The classes one snippet produces, in order, so a test can say what it
    /// expects without writing HTML out longhand.
    fn classes(html: &str) -> Vec<(String, String)> {
        let mut found = Vec::new();
        let mut rest = html;
        while let Some(at) = rest.find("<span class=\"") {
            rest = rest.get(at + 13..).unwrap_or("");
            let Some((class, tail)) = rest.split_once("\">") else { break };
            let Some((text, tail)) = tail.split_once("</span>") else { break };
            found.push((class.to_string(), text.to_string()));
            rest = tail;
        }
        found
    }

    #[test]
    fn a_buri_snippet_is_classified_by_the_lexer() {
        let html = block(
            "buri",
            "// a comment\nexport fn shortfall(self): Int {\n  let n = 12 + 1;\n  \"text\"\n}\n",
        );
        assert_eq!(
            classes(&html),
            vec![
                ("comment".to_string(), "// a comment".to_string()),
                ("keyword".to_string(), "export".to_string()),
                ("keyword".to_string(), "fn".to_string()),
                ("identifier".to_string(), "shortfall".to_string()),
                ("keyword".to_string(), "self".to_string()),
                ("type".to_string(), "Int".to_string()),
                ("keyword".to_string(), "let".to_string()),
                ("identifier".to_string(), "n".to_string()),
                ("operator".to_string(), "=".to_string()),
                ("number".to_string(), "12".to_string()),
                ("operator".to_string(), "+".to_string()),
                ("number".to_string(), "1".to_string()),
                ("string".to_string(), "&quot;text&quot;".to_string()),
            ]
        );
    }

    #[test]
    fn a_snippet_that_does_not_lex_still_renders_its_own_text() {
        let html = block("buri", "let x = \"unterminated\n");
        assert!(html.contains("let"), "{html}");
    }

    #[test]
    fn markup_in_a_snippet_is_escaped_rather_than_emitted() {
        let html = block("buri", "let a = b < c && d > e;\n");
        assert!(html.contains("&lt;"), "{html}");
        assert!(html.contains("&amp;&amp;"), "{html}");
        assert!(!html.contains("< c"), "{html}");
    }

    #[test]
    fn a_build_file_is_classified_by_the_build_file_parser() {
        let html = block(
            "textproto",
            "# a comment\nlibrary {\n  sources: [\"cents.buri\"]\n  count: 2\n}\n",
        );
        assert_eq!(
            classes(&html),
            vec![
                ("comment".to_string(), "# a comment".to_string()),
                ("keyword".to_string(), "library".to_string()),
                ("keyword".to_string(), "sources".to_string()),
                ("string".to_string(), "&quot;cents.buri&quot;".to_string()),
                ("keyword".to_string(), "count".to_string()),
                ("number".to_string(), "2".to_string()),
            ]
        );
    }

    #[test]
    fn an_enum_value_in_a_build_file_reads_as_a_type() {
        let html = block("textproto", "outputs: [{ platform: LINUX }]\n");
        assert!(classes(&html).contains(&("type".to_string(), "LINUX".to_string())), "{html}");
    }

    #[test]
    fn an_unknown_language_is_left_unstyled() {
        let html = block("sh", "buri build //...\n");
        assert_eq!(html, "buri build //...\n");
        assert!(classes(&html).is_empty());
    }
}
