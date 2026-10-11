//! What a file is made of, token by token and comment by comment: what the
//! suites compare to say formatting lost or invented nothing.

use buri::diagnostics::FileId;
use buri::parsing::lexer::{lex, TokenKind};

/// One element of a file, as far as the formatter is concerned.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Shape {
    Token(String),
    /// `//` and `/* */`, with each line trimmed: re-indenting a block comment
    /// is a layout change, not a loss.
    Comment(String),
    /// `///`
    Doc(String),
    /// `//!`
    ModuleDoc(String),
}

/// The tokens and the comments of a file, in source order.
pub fn token_shape(text: &str) -> Vec<Shape> {
    shapes(text, true)
}

/// The comments alone, in source order: the half of the shape formatting must
/// keep exactly.
pub fn comment_shape(text: &str) -> Vec<Shape> {
    shapes(text, false)
}

fn shapes(text: &str, tokens: bool) -> Vec<Shape> {
    let lexed = lex(text, FileId(0));
    let slice = |start: u32, end: u32| text.get(start as usize..end as usize).unwrap_or("");
    let mut out: Vec<Shape> = Vec::new();
    for (at, _) in &lexed.module_docs {
        out.push(Shape::ModuleDoc(slice(at.start, at.end).trim().to_string()));
    }
    let mut trivia = lexed.trivia.iter().peekable();
    for i in 0..lexed.tokens.len() {
        if let Some((_, tv)) = trivia.next_if(|(at, _)| *at as usize == i) {
            for c in &tv.comments {
                let lines: Vec<&str> = c.text(text).lines().map(str::trim).collect();
                out.push(Shape::Comment(lines.join("\n")));
            }
            let (from, len) = (tv.docs.start as usize, tv.docs.len as usize);
            for at in lexed.docs.get(from..from + len).unwrap_or(&[]) {
                out.push(Shape::Doc(slice(at.start, at.end).trim().to_string()));
            }
        }
        if tokens && lexed.tokens.kind(i) != TokenKind::Eof {
            out.push(Shape::Token(lexed.tokens.describe(i)));
        }
    }
    out
}
