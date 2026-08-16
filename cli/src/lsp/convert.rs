//! Between the compiler's world and the protocol's.
//!
//! Two mismatches, and both are the kind that produce an off-by-one nobody
//! notices until a caret lands in the wrong place:
//!
//!   * The compiler counts **bytes** from the start of the file. The protocol
//!     counts **lines**, and within a line **UTF-16 code units** — so `é` is
//!     two bytes and one unit, and `😀` is four bytes and *two* units.
//!   * The compiler's lines are 1-based, because that is what a person reading
//!     an error expects. The protocol's are 0-based.

use crate::diag::{Diagnostic, Severity, Span};
use crate::json::Value;
use std::path::{Path, PathBuf};

/// A protocol position: 0-based line, 0-based UTF-16 offset within it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Position {
    pub line: u32,
    pub character: u32,
}

impl Position {
    pub fn to_json(self) -> Value {
        Value::obj(vec![
            ("line", Value::num(self.line)),
            ("character", Value::num(self.character)),
        ])
    }

    pub fn from_json(v: &Value) -> Option<Position> {
        Some(Position {
            line: v.get("line")?.as_u32()?,
            character: v.get("character")?.as_u32()?,
        })
    }
}

/// Byte offset -> protocol position.
pub fn position_of(text: &str, offset: u32) -> Position {
    let offset = (offset as usize).min(text.len());
    let mut line = 0u32;
    let mut line_start = 0usize;
    for (i, b) in text.bytes().enumerate().take(offset) {
        if b == b'\n' {
            line += 1;
            line_start = i + 1;
        }
    }
    let character = text[line_start..offset].chars().map(|c| c.len_utf16() as u32).sum();
    Position { line, character }
}

/// Protocol position -> byte offset. A position past the end of a line clamps
/// to the end of that line, which is what a client sends when the buffer it is
/// describing is one edit ahead of the one the server has.
pub fn offset_of(text: &str, p: Position) -> u32 {
    let mut start = 0usize;
    for _ in 0..p.line {
        match text[start..].find('\n') {
            Some(i) => start += i + 1,
            None => return text.len() as u32,
        }
    }
    let line_end = text[start..].find('\n').map(|i| start + i).unwrap_or(text.len());
    let mut units = 0u32;
    for (i, c) in text[start..line_end].char_indices() {
        if units >= p.character {
            return (start + i) as u32;
        }
        units += c.len_utf16() as u32;
    }
    line_end as u32
}

pub fn range(text: &str, span: Span) -> Value {
    Value::obj(vec![
        ("start", position_of(text, span.start).to_json()),
        ("end", position_of(text, span.end).to_json()),
    ])
}

/// `file:///a/b%20c.buri`. Percent-encoding is applied to everything outside
/// the unreserved set, which is narrower than it needs to be and never wrong.
pub fn uri_of(path: &Path) -> String {
    let mut out = String::from("file://");
    for b in path.to_string_lossy().as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => {
                out.push(*b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

pub fn path_of(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    // A `file://host/path` authority is not something a local editor sends,
    // and guessing at one would produce a path that silently is not the file.
    let rest = rest.strip_prefix('/').map(|r| format!("/{r}")).unwrap_or_else(|| rest.to_string());
    let b = rest.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            let hex = std::str::from_utf8(&b[i + 1..i + 3]).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    Some(PathBuf::from(String::from_utf8(out).ok()?))
}

fn severity(s: Severity) -> f64 {
    match s {
        Severity::Error => 1.0,
        Severity::Warning => 2.0,
        Severity::Note => 3.0,
    }
}

/// One diagnostic, for a file whose text is `text`.
///
/// The `fix` becomes the first piece of `relatedInformation` rather than being
/// appended to the message, because every diagnostic this compiler emits has
/// one and a message with the fix glued on is a message that scrolls. Notes
/// follow it.
pub fn diagnostic(text: &str, d: &Diagnostic, uri: &str) -> Value {
    let mut related = Vec::new();
    let mut add = |span: Span, message: String| {
        related.push(Value::obj(vec![
            (
                "location",
                Value::obj(vec![
                    ("uri", Value::str(uri)),
                    ("range", range(text, span)),
                ]),
            ),
            ("message", Value::str(message)),
        ]));
    };
    if let Some(fix) = &d.fix {
        add(d.span, format!("fix: {fix}"));
    }
    for n in &d.notes {
        add(d.span, n.clone());
    }
    for s in &d.subs {
        // A sub-span may be in another file; the client resolves the URI, and
        // a wrong one is worse than none, so only same-file spans are related.
        if s.span.file == d.span.file {
            add(s.span, s.label.clone());
        }
    }

    let mut fields = vec![
        ("range", range(text, d.span)),
        ("severity", Value::num(severity(d.severity))),
        ("message", Value::str(&d.message)),
        ("source", Value::str("buri")),
    ];
    if let Some(code) = &d.code {
        fields.push(("code", Value::str(code)));
    }
    if !related.is_empty() {
        fields.push(("relatedInformation", Value::Arr(related)));
    }
    Value::obj(fields)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_position_counts_utf16_units_not_bytes() {
        // `é` is two bytes and one unit; `😀` is four bytes and two units.
        let text = "aé😀b";
        assert_eq!(position_of(text, 0), Position { line: 0, character: 0 });
        assert_eq!(position_of(text, 1), Position { line: 0, character: 1 });
        assert_eq!(position_of(text, 3), Position { line: 0, character: 2 });
        assert_eq!(position_of(text, 7), Position { line: 0, character: 4 });
    }

    #[test]
    fn positions_and_offsets_are_inverses() {
        let text = "let x = 1;\nlet é = 2;\n😀\n";
        for offset in 0..text.len() as u32 {
            if !text.is_char_boundary(offset as usize) {
                continue;
            }
            let p = position_of(text, offset);
            assert_eq!(offset_of(text, p), offset, "at {offset} ({p:?})");
        }
    }

    #[test]
    fn a_position_past_the_end_of_a_line_clamps_to_it() {
        let text = "ab\ncd\n";
        assert_eq!(offset_of(text, Position { line: 0, character: 99 }), 2);
        assert_eq!(offset_of(text, Position { line: 99, character: 0 }), text.len() as u32);
    }

    #[test]
    fn uris_round_trip_including_the_characters_that_need_encoding() {
        for p in ["/a/b.buri", "/a b/c.buri", "/a/é.buri", "/a/%.buri"] {
            let uri = uri_of(Path::new(p));
            assert_eq!(path_of(&uri).as_deref(), Some(Path::new(p)), "{uri}");
        }
    }

    #[test]
    fn a_uri_without_the_file_scheme_is_not_a_path() {
        assert_eq!(path_of("http://example.com/x.buri"), None);
    }
}
