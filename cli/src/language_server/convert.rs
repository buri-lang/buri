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

use crate::diagnostics::{Diagnostic, Severity, Span};
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
///
/// Walking characters rather than slicing at `offset` means an offset that
/// lands inside a multi-byte character counts that character rather than
/// splitting it. Nothing in the compiler produces such an offset, but the
/// alternative to counting one is a panic.
pub fn position_of(text: &str, offset: u32) -> Position {
    let offset = (offset as usize).min(text.len());
    let mut line = 0u32;
    let mut character = 0u32;
    for (i, c) in text.char_indices() {
        if i >= offset {
            break;
        }
        if c == '\n' {
            line = line.saturating_add(1);
            character = 0;
        } else {
            character = character.saturating_add(c.len_utf16() as u32);
        }
    }
    Position { line, character }
}

/// Protocol position -> byte offset. A position past the end of a line clamps
/// to the end of that line, which is what a client sends when the buffer it is
/// describing is one edit ahead of the one the server has. A line past the end
/// of the file clamps to the end of the file, for the same reason.
///
/// The result is always a character boundary in `text`, which is what lets the
/// callers slice with it.
pub fn offset_of(text: &str, p: Position) -> u32 {
    let mut line_start = 0usize;
    for (n, line) in text.split_inclusive('\n').enumerate() {
        if n != p.line as usize {
            line_start = line_start.saturating_add(line.len());
            continue;
        }
        let line = line.strip_suffix('\n').unwrap_or(line);
        let mut units = 0u32;
        for (i, c) in line.char_indices() {
            if units >= p.character {
                return line_start.saturating_add(i) as u32;
            }
            units = units.saturating_add(c.len_utf16() as u32);
        }
        return line_start.saturating_add(line.len()) as u32;
    }
    text.len() as u32
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
    let mut out: Vec<u8> = Vec::with_capacity(rest.len());
    let mut bytes: &[u8] = rest.as_bytes();
    while let Some((&b, tail)) = bytes.split_first() {
        // A `%` without two bytes behind it is not an escape. `uri_of` never
        // writes one, but a client's encoder is not this one.
        match if b == b'%' { tail.split_at_checked(2) } else { None } {
            Some((hex, after)) => {
                out.push(u8::from_str_radix(std::str::from_utf8(hex).ok()?, 16).ok()?);
                bytes = after;
            }
            None => {
                out.push(b);
                bytes = tail;
            }
        }
    }
    Some(PathBuf::from(String::from_utf8(out).ok()?))
}

fn severity(s: Severity) -> i64 {
    match s {
        Severity::Error => 1,
        Severity::Warning => 2,
        Severity::Note => 3,
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
    for secondary in &d.secondary_spans {
        // A secondary span may be in another file; the client resolves the URI,
        // a wrong one is worse than none, so only same-file spans are related.
        if secondary.span.file == d.span.file {
            add(secondary.span, secondary.label.clone());
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

    /// A position is whatever the client sent. Every one of these used to be a
    /// slice or a subtraction; none of them may be a panic, and each has to
    /// land on a character boundary so that the callers can slice with it.
    #[test]
    fn a_position_the_client_made_up_still_lands_somewhere() {
        let text = "aé😀\nx\n";
        let hostile = [
            Position { line: 0, character: u32::MAX },
            Position { line: u32::MAX, character: u32::MAX },
            Position { line: u32::MAX, character: 0 },
            // Inside `é`, and inside `😀`: a UTF-16 offset the text has no
            // boundary for.
            Position { line: 0, character: 1 },
            Position { line: 0, character: 3 },
            Position { line: 1, character: 500 },
            Position { line: 2, character: 0 },
            Position { line: 3, character: 7 },
        ];
        for p in hostile {
            let o = offset_of(text, p) as usize;
            assert!(o <= text.len(), "{p:?} left the file at {o}");
            assert!(text.is_char_boundary(o), "{p:?} landed inside a character at {o}");
        }
        // And the empty file, where there is no line to clamp to.
        for p in hostile {
            assert_eq!(offset_of("", p), 0, "{p:?}");
        }
    }

    /// A byte offset past the end, or inside a character, is the compiler's
    /// side of the same question.
    #[test]
    fn a_byte_offset_that_is_not_a_boundary_still_has_a_position() {
        let text = "aé😀\nx";
        assert_eq!(position_of(text, u32::MAX), Position { line: 1, character: 1 });
        // Inside the `😀`: the character it is inside counts whole.
        assert_eq!(position_of(text, 5), Position { line: 0, character: 4 });
        assert_eq!(position_of("", 9), Position { line: 0, character: 0 });
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
