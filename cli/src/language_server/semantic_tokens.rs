//! `textDocument/semanticTokens` — colour, in two layers.
//!
//! **Layer one is the lexer.** Every token's kind and span, with no analysis of
//! any kind behind it: a keyword is a keyword, a literal is a literal, and a
//! comment is a comment in a file that does not parse, does not typecheck and
//! names a module that is not in the build graph. That is the layer that has to
//! survive, because the file being edited is the file most often broken.
//!
//! **Layer two is the resolver.** An identifier is the one token the lexer
//! cannot classify — `Cents` and `cents` are the same shape to it — so each
//! one is handed to [`symbols::at`], and the `Symbol` it names decides the
//! type: a trait is an `interface`, a variant is an `enumMember`, a field is a
//! `property`, a method is not a function. This is the whole reason for a
//! second answer beside the editor's grammar: a grammar cannot tell a type name
//! from a value name, and this can.
//!
//! Nothing here is a fallback for the other. Layer one runs always and layer
//! two upgrades what it can, so the worst case is a file coloured like a
//! grammar would colour it rather than a file with no colour at all.

use super::convert::{self, Position};
use super::state::Analyzed;
use super::symbols::{self, Symbol};
use crate::diagnostics::{FileId, Span};
use crate::json::Value;
use crate::parsing::lexer::TokenKind;
use std::path::Path;

/// The token types this server emits, in the order the legend declares them.
///
/// A client reads a token's type as an index into this array, so the order is
/// protocol rather than presentation: reordering it re-colours every file in
/// every editor that has already read the legend.
pub const TYPES: [&str; 13] = [
    "namespace",
    "type",
    "interface",
    "enumMember",
    "property",
    "function",
    "method",
    "variable",
    "keyword",
    "comment",
    "string",
    "number",
    "operator",
];

/// The modifiers, read as a bitset in the same way.
///
/// Both are set together and never apart: Buri writes a declaration and its
/// definition in one place, so a name that is one is the other. They are
/// declared separately because a client's theme may style either.
pub const MODIFIERS: [&str; 2] = ["declaration", "definition"];

const NAMESPACE: u32 = 0;
const TYPE: u32 = 1;
const INTERFACE: u32 = 2;
const ENUM_MEMBER: u32 = 3;
const PROPERTY: u32 = 4;
const FUNCTION: u32 = 5;
const METHOD: u32 = 6;
const VARIABLE: u32 = 7;
const KEYWORD: u32 = 8;
const COMMENT: u32 = 9;
const STRING: u32 = 10;
const NUMBER: u32 = 11;
const OPERATOR: u32 = 12;

/// `declaration` and `definition` together — see [`MODIFIERS`].
const DECLARES: u32 = 0b11;

/// One coloured run of text, before the protocol's relative encoding.
struct Piece {
    start: u32,
    end: u32,
    kind: u32,
    modifiers: u32,
}

/// Every token in the file, encoded.
pub fn encoded(analyzed: Option<&Analyzed>, path: &Path, text: &str) -> Vec<u32> {
    encode(text, &pieces(analyzed, path, text))
}

/// The same computation, filtered to what the range touches.
///
/// Intersecting rather than contained: a client asks about the lines on screen,
/// and a block comment that starts above the fold is still colour on it.
pub fn encoded_range(
    analyzed: Option<&Analyzed>,
    path: &Path,
    text: &str,
    from: u32,
    to: u32,
) -> Vec<u32> {
    let all = pieces(analyzed, path, text);
    let kept: Vec<Piece> =
        all.into_iter().filter(|p| p.start < to && from < p.end).collect();
    encode(text, &kept)
}

/// The encoded array as the protocol's `uinteger[]`.
pub fn numbers(data: &[u32]) -> Value {
    Value::Array(data.iter().map(|n| Value::number(*n)).collect())
}

/// The edits that turn one encoded result into another.
///
/// A common prefix and a common suffix, snapped to the five-number record so
/// that an edit never lands inside a token. That is enough because the encoding
/// is relative: an edit in the middle of a file moves the numbers of the token
/// that follows it and of nothing after that, so the untouched tail matches
/// itself number for number.
pub fn edits(previous: &[u32], current: &[u32]) -> Value {
    let mut head = 0usize;
    while head < previous.len()
        && head < current.len()
        && previous.get(head) == current.get(head)
    {
        head = head.saturating_add(1);
    }
    head = head.saturating_sub(head % 5);

    let previous_tail = previous.len().saturating_sub(head);
    let current_tail = current.len().saturating_sub(head);
    let mut tail = 0usize;
    while tail < previous_tail
        && tail < current_tail
        && previous.get(previous.len().saturating_sub(tail).saturating_sub(1))
            == current.get(current.len().saturating_sub(tail).saturating_sub(1))
    {
        tail = tail.saturating_add(1);
    }
    tail = tail.saturating_sub(tail % 5);

    let delete = previous.len().saturating_sub(head).saturating_sub(tail);
    let replacement = current.get(head..current.len().saturating_sub(tail)).unwrap_or(&[]);
    // Nothing to say is said as nothing, not as an empty splice.
    if delete == 0 && replacement.is_empty() {
        return Value::Array(Vec::new());
    }
    Value::Array(vec![Value::object(vec![
        ("start", Value::number(head as i64)),
        ("deleteCount", Value::number(delete as i64)),
        ("data", numbers(replacement)),
    ])])
}

// ---------------------------------------------------------------------------
// Layer one
// ---------------------------------------------------------------------------

fn pieces(analyzed: Option<&Analyzed>, path: &Path, text: &str) -> Vec<Piece> {
    // One resolver for the whole file rather than one per identifier: the
    // literal fence and the module's written names are the same answer at
    // every offset in the buffer.
    let resolver = analyzed.and_then(|a| symbols::Resolver::of(a, path, text));
    let lexed = crate::parsing::lexer::lex(text, FileId(0));
    let mut out = Vec::new();
    let mut cursor = 0u32;
    for i in 0..lexed.tokens.len() {
        let span = lexed.tokens.span(i);
        comments(text, cursor, span.start, &mut out);
        cursor = span.end;
        let kind = lexed.tokens.kind(i);
        if matches!(kind, TokenKind::Eof) {
            continue;
        }
        let Some((token, modifiers)) = classify(analyzed, resolver.as_ref(), kind, span) else {
            continue;
        };
        split(text, span.start, span.end, token, modifiers, &mut out);
    }
    comments(text, cursor, text.len() as u32, &mut out);
    out
}

/// What a token is, before any analysis.
///
/// An identifier is the only kind that needs one, and gets none when there is
/// no analysis to give it — which is what leaves a broken file coloured.
fn classify(
    analyzed: Option<&Analyzed>,
    resolver: Option<&symbols::Resolver>,
    kind: TokenKind,
    span: Span,
) -> Option<(u32, u32)> {
    match kind {
        TokenKind::Ident => resolved(analyzed?, resolver?, span),
        TokenKind::Int | TokenKind::Float => Some((NUMBER, 0)),
        TokenKind::Str
        | TokenKind::Char
        | TokenKind::TemplateHead
        | TokenKind::TemplateSpan
        | TokenKind::TemplateTail => Some((STRING, 0)),
        _ if kind.as_keyword().is_some() => Some((KEYWORD, 0)),
        _ if is_operator(kind) => Some((OPERATOR, 0)),
        _ => None,
    }
}

/// The punctuators that are operators, which is not all of them.
///
/// A brace, a comma and a colon are structure rather than arithmetic, and a
/// theme that colours `operator` is asking about `+` and `==`. Colouring the
/// separators too would paint every file's punctuation one colour, which is
/// what an editor's grammar already declines to do.
fn is_operator(kind: TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::Eq
            | TokenKind::FatArrow
            | TokenKind::EqEq
            | TokenKind::BangEq
            | TokenKind::Lt
            | TokenKind::LtEq
            | TokenKind::Gt
            | TokenKind::GtEq
            | TokenKind::Plus
            | TokenKind::Minus
            | TokenKind::Star
            | TokenKind::Slash
            | TokenKind::Percent
            | TokenKind::AndAnd
            | TokenKind::OrOr
            | TokenKind::Bang
            | TokenKind::QuestionQuestion
            | TokenKind::And
            | TokenKind::Or
            | TokenKind::Caret
            | TokenKind::Tilde
            | TokenKind::Question
            | TokenKind::DotDot
    )
}

/// The comments written between two tokens.
///
/// The gap between one token's end and the next one's start holds whitespace
/// and comments and nothing else — a string is a token — so scanning it for
/// `//` and `/*` cannot be fooled by a slash inside a literal. The nesting rule
/// is the lexer's: block comments nest.
///
/// Comments are read from the gaps rather than from `Lexed::trivia`, because
/// trivia keeps a comment's text and its column and not its span, and a span is
/// the whole of what a colour needs.
fn comments(text: &str, from: u32, to: u32, out: &mut Vec<Piece>) {
    let bytes = text.as_bytes();
    let stop = (to as usize).min(text.len());
    let mut at = from as usize;
    while at < stop {
        let two = (bytes.get(at), bytes.get(at.saturating_add(1)));
        match two {
            (Some(b'/'), Some(b'/')) => {
                let mut end = at;
                while end < stop && bytes.get(end) != Some(&b'\n') {
                    end = end.saturating_add(1);
                }
                split(text, at as u32, end as u32, COMMENT, 0, out);
                at = end;
            }
            (Some(b'/'), Some(b'*')) => {
                let mut end = at.saturating_add(2);
                let mut depth = 1usize;
                while end < stop && depth > 0 {
                    if (bytes.get(end), bytes.get(end.saturating_add(1)))
                        == (Some(&b'/'), Some(&b'*'))
                    {
                        depth = depth.saturating_add(1);
                        end = end.saturating_add(2);
                    } else if (bytes.get(end), bytes.get(end.saturating_add(1)))
                        == (Some(&b'*'), Some(&b'/'))
                    {
                        depth = depth.saturating_sub(1);
                        end = end.saturating_add(2);
                    } else {
                        end = end.saturating_add(1);
                    }
                }
                let end = end.min(stop);
                split(text, at as u32, end as u32, COMMENT, 0, out);
                at = end;
            }
            _ => at = at.saturating_add(1),
        }
    }
}

/// One run, cut at every newline it crosses.
///
/// The protocol has no way to spell a token that spans lines — the encoding
/// carries a length and no end position — so a block comment and a multi-line
/// literal are one piece per line. A trailing `\r` is dropped so a file with
/// Windows endings does not colour an invisible character.
fn split(text: &str, start: u32, end: u32, kind: u32, modifiers: u32, out: &mut Vec<Piece>) {
    let end = end.min(text.len() as u32);
    let mut at = start;
    while at < end {
        let Some(rest) = text.get(at as usize..end as usize) else { return };
        let line_end = match rest.find('\n') {
            Some(i) => at.saturating_add(i as u32),
            None => end,
        };
        let mut cut = line_end;
        if cut > at && text.as_bytes().get((cut as usize).saturating_sub(1)) == Some(&b'\r') {
            cut = cut.saturating_sub(1);
        }
        if cut > at {
            out.push(Piece { start: at, end: cut, kind, modifiers });
        }
        at = line_end.saturating_add(1);
    }
}

// ---------------------------------------------------------------------------
// Layer two
// ---------------------------------------------------------------------------

/// What an identifier names, asked of the one resolver every other request
/// asks — so a name cannot be one thing to hover and another to colour.
///
/// The cost is honest and worth naming: this is one resolution per identifier
/// in the file. What the batch takes off it is the part that does not depend on
/// the offset — the lex behind the literal fence and the scan of the names the
/// module writes, both now paid once for the request. What is left is the walk
/// of the declarations and of the typed bodies, which a different offset really
/// does answer differently.
fn resolved(
    analyzed: &Analyzed,
    resolver: &symbols::Resolver,
    span: Span,
) -> Option<(u32, u32)> {
    let found = resolver.at(span.start)?;
    let tables = &analyzed.analysis.checked.tables;
    let kind = match &found.symbol {
        // A method is reached through a value and a function is not, which is
        // the distinction a grammar has no way to see.
        Symbol::Function(id) => {
            if tables.fn_info(*id).self_ty.is_some() {
                METHOD
            } else {
                FUNCTION
            }
        }
        // A context declaration names a set of capabilities and is written like
        // a type; the outline already reports it as one.
        Symbol::Type(_) | Symbol::Context(_) => TYPE,
        Symbol::Trait(_) => INTERFACE,
        Symbol::TraitMethod { .. } => METHOD,
        Symbol::Const(_) | Symbol::Local { .. } => VARIABLE,
        Symbol::Field { .. } => PROPERTY,
        Symbol::Variant { .. } => ENUM_MEMBER,
        Symbol::Module(_) => NAMESPACE,
    };
    // The declaration modifiers, decided by position rather than by kind: this
    // token is the declaration exactly when it is the name the declaration
    // wrote. Every other mention of the same symbol is a use.
    //
    // The comparison is against the *token*, not against `Found::span`, which
    // for a trait method is the whole signature the cursor landed in. The file
    // still comes from `Found`, because the lexer here numbers every buffer
    // `FileId(0)` and the tables number them per session.
    let declared = symbols::declaration_name(analyzed, &found.symbol);
    let modifiers = if declared.file == found.span.file
        && declared.start == span.start
        && declared.end == span.end
    {
        DECLARES
    } else {
        0
    };
    Some((kind, modifiers))
}

// ---------------------------------------------------------------------------
// Encoding
// ---------------------------------------------------------------------------

/// The protocol's relative encoding: five numbers a token, each line and
/// character counted from the token before it.
///
/// The UTF-16 arithmetic is `convert`'s, in one pass — see
/// [`convert::positions_of`], which exists because asking [`convert::position_of`]
/// once per token walks the file once per token.
fn encode(text: &str, pieces: &[Piece]) -> Vec<u32> {
    let offsets: Vec<u32> = pieces.iter().map(|p| p.start).collect();
    let positions = convert::positions_of(text, &offsets);
    let mut data = Vec::with_capacity(pieces.len().saturating_mul(5));
    let mut previous = Position { line: 0, character: 0 };
    for (piece, at) in pieces.iter().zip(positions) {
        let length: u32 = text
            .get(piece.start as usize..piece.end as usize)
            .map(|s| s.chars().map(|c| c.len_utf16() as u32).sum())
            .unwrap_or(0);
        let line = at.line.saturating_sub(previous.line);
        let character =
            if line == 0 { at.character.saturating_sub(previous.character) } else { at.character };
        data.extend([line, character, length, piece.kind, piece.modifiers]);
        previous = at;
    }
    data
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Layer one alone, on a file no analysis was handed for.
    fn lexed(text: &str) -> Vec<u32> {
        encoded(None, Path::new("/x.buri"), text)
    }

    /// The five numbers of the `n`th token.
    fn record(data: &[u32], n: usize) -> &[u32] {
        data.get(n.saturating_mul(5)..n.saturating_mul(5).saturating_add(5)).unwrap_or(&[])
    }

    #[test]
    fn a_file_that_does_not_parse_is_still_coloured() {
        // A `fn` with no body and a dangling brace: nothing here typechecks.
        let data = lexed("// hi\nfn broken(: 42 {\n");
        assert_eq!(record(&data, 0), [0, 0, 5, COMMENT, 0]);
        assert_eq!(record(&data, 1), [1, 0, 2, KEYWORD, 0]);
        // `broken` is an identifier and there is no analysis, so it has no
        // colour — and the number after it still does.
        assert_eq!(record(&data, 2), [0, 12, 2, NUMBER, 0]);
    }

    #[test]
    fn a_block_comment_is_one_piece_a_line() {
        let data = lexed("/* one\n   two */\nlet\n");
        assert_eq!(record(&data, 0), [0, 0, 6, COMMENT, 0]);
        assert_eq!(record(&data, 1), [1, 0, 9, COMMENT, 0]);
        assert_eq!(record(&data, 2), [1, 0, 3, KEYWORD, 0]);
    }

    #[test]
    fn a_slash_inside_a_string_is_not_a_comment() {
        let data = lexed("let x = \"// not a comment\";\n");
        // keyword, identifier (uncoloured), `=`, string — and no comment.
        assert_eq!(record(&data, 0), [0, 0, 3, KEYWORD, 0]);
        assert_eq!(record(&data, 1), [0, 6, 1, OPERATOR, 0]);
        assert_eq!(record(&data, 2), [0, 2, 18, STRING, 0]);
        assert_eq!(data.len(), 15);
    }

    /// The protocol counts UTF-16 units, and a length is a length in them.
    #[test]
    fn a_length_and_a_character_are_counted_in_utf16_units() {
        let data = lexed("// 😀\n1\n");
        // Four units: two slashes, a space, and the surrogate pair.
        assert_eq!(record(&data, 0), [0, 0, 5, COMMENT, 0]);
        assert_eq!(record(&data, 1), [1, 0, 1, NUMBER, 0]);
    }

    #[test]
    fn a_range_keeps_what_it_touches() {
        let text = "let\n42\nlet\n";
        let all = lexed(text);
        let middle = encoded_range(None, Path::new("/x.buri"), text, 4, 6);
        assert_eq!(all.len(), 15);
        assert_eq!(middle, [1, 0, 2, NUMBER, 0]);
    }

    #[test]
    fn an_unchanged_result_has_no_edits() {
        let data = [0, 0, 3, KEYWORD, 0, 1, 0, 2, NUMBER, 0];
        assert_eq!(edits(&data, &data), Value::Array(Vec::new()));
    }

    /// One token replaced in the middle: the prefix and the suffix are kept and
    /// the splice is on a record boundary.
    #[test]
    fn an_edit_splices_whole_tokens() {
        let previous = [0, 0, 3, KEYWORD, 0, 1, 0, 2, NUMBER, 0, 1, 0, 3, KEYWORD, 0];
        let current = [0, 0, 3, KEYWORD, 0, 1, 0, 6, STRING, 0, 1, 0, 3, KEYWORD, 0];
        let out = edits(&previous, &current);
        let Value::Array(items) = &out else { panic!("{}", out.to_string()) };
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].get("start").and_then(|v| v.as_u32()), Some(5));
        assert_eq!(items[0].get("deleteCount").and_then(|v| v.as_u32()), Some(5));
        assert_eq!(
            items[0].get("data").map(|d| d.to_string()),
            Some("[1,0,6,10,0]".to_string())
        );
    }

    #[test]
    fn a_token_appended_at_the_end_is_an_insertion() {
        let previous = [0, 0, 3, KEYWORD, 0];
        let current = [0, 0, 3, KEYWORD, 0, 1, 0, 2, NUMBER, 0];
        let out = edits(&previous, &current);
        let Value::Array(items) = &out else { panic!("{}", out.to_string()) };
        assert_eq!(items[0].get("start").and_then(|v| v.as_u32()), Some(5));
        assert_eq!(items[0].get("deleteCount").and_then(|v| v.as_u32()), Some(0));
    }

    /// The legend is what a client indexes into, so every constant has to name
    /// the entry it stands for.
    #[test]
    fn the_legend_and_the_indices_agree() {
        for (index, name) in [
            (NAMESPACE, "namespace"),
            (TYPE, "type"),
            (INTERFACE, "interface"),
            (ENUM_MEMBER, "enumMember"),
            (PROPERTY, "property"),
            (FUNCTION, "function"),
            (METHOD, "method"),
            (VARIABLE, "variable"),
            (KEYWORD, "keyword"),
            (COMMENT, "comment"),
            (STRING, "string"),
            (NUMBER, "number"),
            (OPERATOR, "operator"),
        ] {
            assert_eq!(TYPES.get(index as usize), Some(&name));
        }
        assert_eq!(MODIFIERS.len(), 2);
        assert_eq!(DECLARES, 0b11);
    }
}
