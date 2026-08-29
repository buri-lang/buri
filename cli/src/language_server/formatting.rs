//! The formatter, as text edits.
//!
//! `buri format` prints a whole file and has no partial mode, and the protocol
//! asks three questions of it: the whole file, a range of it, and the moment a
//! `}` or a `;` was typed. All three are answered from the **same** canonical
//! output — a range answer is the whole-file diff filtered to the hunks that
//! range touches, never a second formatter that could disagree with the first.
//! That is what makes serving a range honest here: nothing partial is computed,
//! something whole is withheld.
//!
//! The diff is over lines. A formatter's output differs from its input in whole
//! lines almost by construction — it re-indents, re-wraps and re-orders lines —
//! so a finer diff would spend more and land in the same places.

use crate::json::Value;
use super::convert::{self, Position};

/// The whole file as one edit, or an empty array when there is nothing to do.
///
/// This is what `textDocument/formatting` answers and what
/// `textDocument/willSaveWaitUntil` answers, and they are one function because
/// format-on-save and format-the-file are the same act at two moments.
pub fn whole_file(name: &str, text: &str) -> Value {
    let Some(formatted) = formatted(name, text) else { return Value::Array(Vec::new()) };
    Value::Array(vec![edit(
        text,
        0,
        u32::try_from(text.len()).unwrap_or(u32::MAX),
        &formatted,
    )])
}

/// Only the edits a byte range touches.
///
/// A hunk that merely abuts the range is kept: a pure insertion has no width,
/// and one at the caret is exactly the edit a reader who selected up to there
/// asked for.
pub fn ranged(name: &str, text: &str, from: u32, to: u32) -> Value {
    let Some(formatted) = formatted(name, text) else { return Value::Array(Vec::new()) };
    let (low, high) = if from <= to { (from, to) } else { (to, from) };
    Value::Array(
        hunks(text, &formatted)
            .into_iter()
            .filter(|h| h.start <= high && h.end >= low)
            .map(|h| edit(text, h.start, h.end, &h.new_text))
            .collect(),
    )
}

/// The byte extent of the top-level item an offset is inside.
///
/// What `onTypeFormatting` scopes to. A `}` closes something, and the something
/// it closes is inside exactly one declaration — so re-printing that
/// declaration is the most a typed brace can honestly ask for.
pub fn enclosing_item(text: &str, offset: u32) -> Option<(u32, u32)> {
    let parsed = crate::parsing::parser::parse(text, crate::diagnostics::FileId(0));
    parsed
        .module
        .items
        .iter()
        .map(crate::parsing::tree::Item::span)
        .find(|span| span.start <= offset && offset <= span.end)
        .map(|span| (span.start, span.end))
}

/// What the formatter would write, or nothing — a file that does not parse is
/// left alone rather than mangled, and one already formatted has no edit.
fn formatted(name: &str, text: &str) -> Option<String> {
    let formatted = crate::commands::format::file(name, text)?;
    (formatted != text).then_some(formatted)
}

fn edit(text: &str, start: u32, end: u32, new_text: &str) -> Value {
    Value::object(vec![
        (
            "range",
            Value::object(vec![
                ("start", position(text, start).to_json()),
                ("end", position(text, end).to_json()),
            ]),
        ),
        ("newText", Value::str(new_text)),
    ])
}

/// The start of the file is the one position `position_of` need not be asked
/// for, and asking it would walk the file to answer `0, 0`.
fn position(text: &str, offset: u32) -> Position {
    if offset == 0 {
        return Position { line: 0, character: 0 };
    }
    convert::position_of(text, offset)
}

/// One replacement: a byte range of the original, and what goes there.
struct Hunk {
    start: u32,
    end: u32,
    new_text: String,
}

/// The line-level differences between a file and its formatted form.
fn hunks(text: &str, formatted: &str) -> Vec<Hunk> {
    let before: Vec<&str> = text.split_inclusive('\n').collect();
    let after: Vec<&str> = formatted.split_inclusive('\n').collect();
    let starts = line_starts(text, &before);

    // The matching top and bottom are not differences and are not searched.
    let mut head = 0usize;
    while before.get(head).is_some() && before.get(head) == after.get(head) {
        head = head.saturating_add(1);
    }
    let mut tail = 0usize;
    while before.len().saturating_sub(head) > tail
        && after.len().saturating_sub(head) > tail
        && before.get(before.len().saturating_sub(tail).saturating_sub(1))
            == after.get(after.len().saturating_sub(tail).saturating_sub(1))
    {
        tail = tail.saturating_add(1);
    }
    let (Some(a), Some(b)) = (
        before.get(head..before.len().saturating_sub(tail)),
        after.get(head..after.len().saturating_sub(tail)),
    ) else {
        return Vec::new();
    };

    let mut out = Vec::new();
    let (mut i, mut j) = (0usize, 0usize);
    for (ai, bj) in aligned(a, b) {
        cut(&mut out, &starts, head, b, (i, ai), (j, bj));
        i = ai.saturating_add(1);
        j = bj.saturating_add(1);
    }
    cut(&mut out, &starts, head, b, (i, a.len()), (j, b.len()));
    out
}

/// One run of removed lines replaced by one run of added lines. An empty run on
/// both sides is not a hunk.
fn cut(
    out: &mut Vec<Hunk>,
    starts: &[u32],
    head: usize,
    added: &[&str],
    removed_lines: (usize, usize),
    added_lines: (usize, usize),
) {
    let (removed_from, removed_to) = removed_lines;
    let (added_from, added_to) = added_lines;
    if removed_from == removed_to && added_from == added_to {
        return;
    }
    let start = starts.get(head.saturating_add(removed_from)).copied().unwrap_or(0);
    let end = starts.get(head.saturating_add(removed_to)).copied().unwrap_or(start);
    out.push(Hunk {
        start,
        end,
        new_text: added.get(added_from..added_to).unwrap_or_default().concat(),
    });
}

/// The byte offset each line begins at, plus the end of the file.
fn line_starts(text: &str, lines: &[&str]) -> Vec<u32> {
    let mut out = Vec::with_capacity(lines.len().saturating_add(1));
    let mut at = 0usize;
    for line in lines {
        out.push(u32::try_from(at).unwrap_or(u32::MAX));
        at = at.saturating_add(line.len());
    }
    out.push(u32::try_from(text.len()).unwrap_or(u32::MAX));
    out
}

/// The lines both sides keep, paired by index — a longest common subsequence.
///
/// What is between two consecutive pairs is one hunk, which is why the answer
/// is the pairs rather than the length: a diff that reported only *how much*
/// matched could not say *where*.
#[expect(
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    reason = "every index is a loop bound below `a.len()` or `b.len()`, and the table is \
              allocated at exactly `(a.len() + 1) * (b.len() + 1)` — the product is checked \
              against the bound above before anything is allocated, so neither the \
              multiplication nor the addition can overflow, and `i * width + j` is inside the \
              table for every `i <= a.len()` and `j <= b.len()`"
)]
fn aligned(a: &[&str], b: &[&str]) -> Vec<(usize, usize)> {
    // Both sides have had their common top and bottom removed, so what is left
    // is the difference itself and is small for anything a formatter does.
    // Past the bound the table is not worth building, and no pairs means one
    // hunk covering the whole difference — still a correct answer, and the one
    // a whole-file format gives.
    if a.len().saturating_mul(b.len()) > 250_000 {
        return Vec::new();
    }
    let width = b.len() + 1;
    let mut table = vec![0u32; (a.len() + 1) * width];
    for i in (0..a.len()).rev() {
        for j in (0..b.len()).rev() {
            table[i * width + j] = if a[i] == b[j] {
                table[(i + 1) * width + j + 1] + 1
            } else {
                table[(i + 1) * width + j].max(table[i * width + j + 1])
            };
        }
    }
    let mut out = Vec::new();
    let (mut i, mut j) = (0usize, 0usize);
    while i < a.len() && j < b.len() {
        if a[i] == b[j] {
            out.push((i, j));
            i += 1;
            j += 1;
        } else if table[(i + 1) * width + j] >= table[i * width + j + 1] {
            i += 1;
        } else {
            j += 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hunk_text(text: &str, formatted: &str) -> Vec<(u32, u32, String)> {
        hunks(text, formatted).into_iter().map(|h| (h.start, h.end, h.new_text)).collect()
    }

    #[test]
    fn an_unchanged_file_has_no_hunks() {
        assert!(hunk_text("a\nb\n", "a\nb\n").is_empty());
    }

    /// Two edits far apart stay two edits. A diff that collapsed them into one
    /// would make every range answer the whole file.
    #[test]
    fn two_distant_changes_are_two_hunks() {
        let before = "a\nb\nc\nd\ne\n";
        let after = "A\nb\nc\nd\nE\n";
        let found = hunk_text(before, after);
        assert_eq!(found.len(), 2, "{found:?}");
        assert_eq!(found.first().map(|h| (h.0, h.1)), Some((0, 2)));
        assert_eq!(found.get(1).map(|h| (h.0, h.1)), Some((8, 10)));
    }

    /// A line added between two kept ones is a zero-width hunk at the seam.
    #[test]
    fn an_insertion_has_no_width() {
        let found = hunk_text("a\nc\n", "a\nb\nc\n");
        assert_eq!(found, vec![(2, 2, "b\n".to_string())]);
    }

    #[test]
    fn a_deletion_replaces_its_lines_with_nothing() {
        let found = hunk_text("a\nb\nc\n", "a\nc\n");
        assert_eq!(found, vec![(2, 4, String::new())]);
    }

    /// The last line of a file need not end in a newline, and a hunk that
    /// reaches it must stop at the end of the text rather than past it.
    #[test]
    fn a_change_to_an_unterminated_last_line_stops_at_the_end() {
        let found = hunk_text("a\nb", "a\nB");
        assert_eq!(found, vec![(2, 3, "B".to_string())]);
    }
}
