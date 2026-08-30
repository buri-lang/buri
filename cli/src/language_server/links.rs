//! The underlines in a file: `textDocument/documentLink`.
//!
//! A link is a different affordance from go-to-definition, not a second
//! spelling of it. An editor draws every one at once, without a cursor, and
//! follows it on a click — so the answer covers things `definition` cannot
//! reach at all: the address written in a doc comment is not a name, and no
//! position request will ever have anything to say about it.
//!
//! Two producers live here and the third is in `build_files`, because a
//! `BUILD.buri` is textproto and the strings in it are the build graph's
//! rather than the language's.

use crate::diagnostics::FileId;
use crate::json::Value;
use crate::parsing::tree::Item;
use std::path::Path;
use super::convert;
use super::state::Analyzed;

/// Every link a Buri source file has: its import paths, and the addresses its
/// comments write.
///
/// The analysis is optional, and the two halves are why: an import path is
/// resolved by the workspace, which a file in no open repository has none of,
/// while an address in a comment is in the text and needs nothing at all. A
/// file the server cannot analyse still gets its URLs.
pub fn document_links(analyzed: Option<&Analyzed>, path: &Path, text: &str) -> Value {
    let mut found: Vec<(u32, u32, String)> = Vec::new();
    if let Some(analyzed) = analyzed {
        imports(analyzed, path, &mut found);
    }
    urls(text, &mut found);
    render(text, found)
}

/// A `DocumentLink[]`, in a fixed order and with no two links over one range.
pub(super) fn render(text: &str, mut found: Vec<(u32, u32, String)>) -> Value {
    found.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));
    found.dedup_by(|a, b| a.0 == b.0 && a.1 == b.1);
    Value::Array(
        found
            .into_iter()
            .map(|(start, end, target)| {
                Value::object(vec![
                    (
                        "range",
                        Value::object(vec![
                            ("start", convert::position_of(text, start).to_json()),
                            ("end", convert::position_of(text, end).to_json()),
                        ]),
                    ),
                    ("target", Value::str(target)),
                ])
            })
            .collect(),
    )
}

/// The path string of every `import` and `export` line, resolved to the file
/// it names.
///
/// The same resolution `definition` performs on the one path under the cursor,
/// run over all of them. A `core/...` path is a module compiled into the
/// binary and has no file, so it gets no underline — the same silence
/// `definition` answers with there.
fn imports(analyzed: &Analyzed, path: &Path, out: &mut Vec<(u32, u32, String)>) {
    let rel = analyzed.session.workspace.rel_of(path);
    let Some(file) = analyzed.session.map.find(&rel) else { return };
    let Some(module) = analyzed.analysis.loaded.modules.iter().find(|m| m.file == file) else {
        return;
    };
    for item in &module.ast.items {
        let (written, span) = match item {
            Item::Import(i) => (&i.path, i.path_span),
            Item::ReExport(r) => (&r.path, r.path_span),
            _ => continue,
        };
        let Ok(resolved) = analyzed.session.workspace.resolve_module(written) else { continue };
        let Some(in_package) = resolved.in_package() else { continue };
        let (start, end) = inside_quotes(&analyzed.session.map.get(file).text, span);
        out.push((start, end, convert::uri_of(&in_package.file)));
    }
}

/// The path without the quotes around it.
///
/// An import's span is the string literal, quotes included, because that is
/// what every import diagnostic is anchored on — and a textproto entry's is
/// the same. An underline belongs under the address rather than under the
/// punctuation holding it.
pub(super) fn inside_quotes(text: &str, span: crate::diagnostics::Span) -> (u32, u32) {
    let inner = span.inside_quotes(text);
    (inner.start, inner.end)
}

/// Every `http://` or `https://` run written in a comment.
///
/// **Which text is a comment is asked of the lexer, not guessed at.** Every
/// token's span is code, so an offset inside none of them is whitespace or a
/// comment — and that is exactly the rule that keeps the address inside
/// `"https://example.org/shop"` out of the answer, where a scan for `//` would
/// have found the path of an import instead.
///
/// The run ends at the first byte that cannot continue an address: whitespace,
/// or a delimiter something wrote *around* it. Trailing sentence punctuation
/// is dropped, because a full stop after a link is a full stop.
fn urls(text: &str, out: &mut Vec<(u32, u32, String)>) {
    let lexed = crate::parsing::lexer::lex(text, FileId(0));
    let code: Vec<(u32, u32)> =
        (0..lexed.tokens.len()).map(|i| (lexed.tokens.span(i).start, lexed.tokens.span(i).end)).collect();
    let bytes = text.as_bytes();
    let mut at = 0usize;
    while at < text.len() {
        let Some(hit) = text.get(at..).and_then(scheme_at) else { break };
        let start = at.saturating_add(hit);
        at = start.saturating_add(1);
        // A scheme glued to the end of a word is part of that word.
        let attached = start
            .checked_sub(1)
            .is_some_and(|b| bytes.get(b).is_some_and(|c| c.is_ascii_alphanumeric()));
        let in_code = code.iter().any(|(s, e)| *s <= start as u32 && (start as u32) < *e);
        if attached || in_code {
            continue;
        }
        let end = run_end(bytes, start);
        // A scheme with nothing after it names nothing.
        if let Some(target) = text.get(start..end).filter(|u| u.len() > scheme_len(u)) {
            out.push((start as u32, end as u32, target.to_string()));
        }
    }
}

/// Where the next `http://` or `https://` begins in the slice.
fn scheme_at(slice: &str) -> Option<usize> {
    let http = slice.find("http://");
    let https = slice.find("https://");
    match (http, https) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    }
}

fn scheme_len(url: &str) -> usize {
    match url.starts_with("https://") {
        true => "https://".len(),
        false => "http://".len(),
    }
}

/// The end of an address that started at `start`.
fn run_end(bytes: &[u8], start: usize) -> usize {
    let mut end = start;
    while let Some(b) = bytes.get(end) {
        // `*/` closes a block comment; it is not part of the address inside it.
        let closes_comment = *b == b'*' && bytes.get(end.saturating_add(1)) == Some(&b'/');
        let delimiter = matches!(b, b'<' | b'>' | b'"' | b'\'' | b'(' | b')' | b'[' | b']'
            | b'{' | b'}' | b'|' | b'\\' | b'^' | b'`');
        if b.is_ascii_whitespace() || delimiter || closes_comment {
            break;
        }
        end = end.saturating_add(1);
    }
    // Prose runs on after a link: `see https://example.org/pricing, which …`.
    while end > start && matches!(bytes.get(end.saturating_sub(1)), Some(b'.' | b',' | b';' | b':' | b'!' | b'?')) {
        end = end.saturating_sub(1);
    }
    end
}

#[cfg(test)]
mod tests {
    use super::run_end;

    fn run(text: &str) -> &str {
        let end = run_end(text.as_bytes(), 0);
        text.get(..end).unwrap_or("")
    }

    #[test]
    fn an_address_ends_where_the_prose_resumes() {
        assert_eq!(run("https://example.org/a b"), "https://example.org/a");
        assert_eq!(run("https://example.org/a, and"), "https://example.org/a");
        assert_eq!(run("https://example.org/a."), "https://example.org/a");
        assert_eq!(run("https://example.org/a>"), "https://example.org/a");
        assert_eq!(run("https://example.org/a*/"), "https://example.org/a");
        assert_eq!(run("https://example.org/a#b?c=d"), "https://example.org/a#b?c=d");
    }
}
