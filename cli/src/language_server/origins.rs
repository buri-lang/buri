//! Where a span in generated code came from.
//!
//! A generated module has no file on disk, so inside the server it looks
//! exactly like the embedded standard library: a `SourceFile` whose `abs_path`
//! is empty. That one test used to be the end of every answer — definition said
//! `null`, a diagnostic in generated text was dropped, rename blamed the
//! standard library.
//!
//! What tells the two apart is the store the build filled. A generated module's
//! `SourceFile::name` **is** its module path, so
//! [`Store::module`](crate::build::generators::Store::module) answers with the
//! module — and with the anchors the generator sent, which say which region of
//! the generated text came from which span of which input.
//!
//! So every behaviour below is one lookup: the innermost anchor covering the
//! offset, then the input file it names. An offset no anchor covers has no
//! origin, and no origin means the answer the server gave before any of this
//! existed.

use crate::build::session::Session;
use crate::diagnostics::Span;
use std::path::PathBuf;
use std::sync::Arc;

/// A place in a generator's input: the file, and a byte range in it.
///
/// The text is not in it, because the file it names is not always one anything
/// has read and every caller but one already knows whether it needs the bytes.
/// [`Located::text`] is the read.
pub struct Located {
    pub path: PathBuf,
    pub start: u32,
    pub end: u32,
}

impl Located {
    pub fn uri(&self) -> String {
        super::convert::uri_of(&self.path)
    }

    /// A span standing for this range. Only the offsets mean anything: they are
    /// read against [`Self::text`], never against the file the id names.
    pub fn span_in(&self, borrowed: Span) -> Span {
        Span { file: borrowed.file, start: self.start, end: self.end }
    }

    /// The input's bytes, which is what turns a byte offset into a line and a
    /// UTF-16 character.
    ///
    /// The open buffer first. `build::sources` seeds every overlaid file into
    /// the session's map under its repository-relative name, so an input being
    /// edited is already here — and it is the text the generator was handed,
    /// because running the generators is what that same overlay does first. One
    /// nothing has opened is read from disk.
    pub fn text(&self, session: &Session) -> Option<String> {
        let rel = session.workspace.rel_of(&self.path);
        match session.map.find(&rel) {
            Some(id) => Some(session.map.get(id).text.clone()),
            None => std::fs::read_to_string(&self.path).ok(),
        }
    }
}

/// The module a span is in, when a generator produced it.
///
/// `None` for a span in a file on disk and for the embedded standard library —
/// both have a name the store has never heard of.
pub fn module_of(
    session: &Session,
    span: Span,
) -> Option<Arc<crate::build::generators::GeneratedModule>> {
    if span.is_none() {
        return None;
    }
    let file = session.map.get(span.file);
    if !file.abs_path.as_os_str().is_empty() {
        return None;
    }
    session.workspace.generated.module(&file.name)
}

/// Where a span in generated text was generated *from*.
///
/// `None` when the span is not in generated code, and when it is but no anchor
/// covers it — a generator anchors the nodes it chooses to, and text it made up
/// out of nothing has nowhere to send a reader.
pub fn of_span(session: &Session, span: Span) -> Option<Located> {
    let module = module_of(session, span)?;
    let anchor = module.anchor_at(span.start as usize)?;
    // Not clamped to the input's length here, because that would be a read of
    // the file for every caller. Every consumer of the range walks the text
    // through `convert`, which clamps an offset past the end of what it is
    // given — a generator that anchored past its own input widens the answer
    // to the end of the file rather than producing one.
    Some(Located {
        path: session.workspace.root.join(&anchor.file),
        start: anchor.span.0 as u32,
        end: anchor.span.1.max(anchor.span.0) as u32,
    })
}

/// The `generators` entry that produced the module a span is in.
///
/// Where a diagnostic goes when generated code is wrong and nothing says which
/// input it came from: the rule that ran the tool is then the whole of what is
/// known, and its entry is a line a person can read and edit.
///
/// The rule's **first** entry, because the store records what a rule produced
/// rather than which of its entries produced each module. A rule with one entry
/// — which is every rule anybody has written — is answered exactly.
pub fn entry_of(session: &Session, span: Span) -> Option<Span> {
    module_of(session, span)?;
    let file = session.map.get(span.file);
    let target = session.workspace.generated.owner(&file.name)?;
    let entry = crate::build::generators::declared(&session.workspace, target).first()?;
    (!entry.span.is_none()).then_some(entry.span)
}

/// The doc comment written above a declaration in a generator's input.
///
/// The run of comment lines immediately above the line the origin starts on,
/// with the marker and one space taken off — the same shape a `///` gives Buri
/// and a `//` gives a `.proto`. A blank line ends the run, because a comment
/// two lines up is about something else.
///
/// `//` and `#` are the two markers, which between them are how every input
/// format anybody generates from writes a line comment. A file that means
/// neither has no comment lines above a declaration to find.
pub fn doc_comment(text: &str, at: &Located) -> Vec<String> {
    let before = text.get(..at.start as usize).unwrap_or("");
    // Everything above the line the declaration starts on, which is why this
    // cuts at the last newline rather than taking whole lines: a declaration
    // that starts mid-line still reads the lines above it.
    let above = match before.rfind('\n') {
        Some(nl) => text.get(..nl).unwrap_or(""),
        None => return Vec::new(),
    };
    let mut docs: Vec<String> = Vec::new();
    // `split` and not `lines`: a blank line above the declaration is the empty
    // string after the last `\n`, and `lines` drops exactly that — which read
    // the comment above the blank line as though the blank were not there.
    for line in above.split('\n').rev() {
        let trimmed = line.trim();
        let Some(rest) = trimmed.strip_prefix("//").or_else(|| trimmed.strip_prefix('#')) else {
            break;
        };
        docs.push(rest.strip_prefix(' ').unwrap_or(rest).to_string());
    }
    docs.reverse();
    docs
}

/// The sentence a rename refuses a generated name with: the tool that wrote it,
/// and the input it was written from.
///
/// Not "the standard library", which is what the refusal used to say about
/// anything with no file. Renaming generated text would be renaming a file the
/// next build overwrites; the edit that lasts is the one in the input.
pub fn refusal_sentence(session: &Session, span: Span) -> Option<String> {
    let file = session.map.get(span.file);
    module_of(session, span)?;
    let target = session.workspace.generated.owner(&file.name)?;
    let tool = crate::build::generators::declared(&session.workspace, target)
        .first()?
        .tool
        .value
        .clone();
    let input = match of_span(session, span) {
        Some(at) => session.workspace.rel_of(&at.path),
        None => crate::build::generators::inputs(&session.workspace, target).join(", "),
    };
    Some(format!(
        "that is written by `{tool}`, so renaming it would last until the next build — \
         edit `{input}` instead"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn docs_above(text: &str, needle: &str) -> Vec<String> {
        let start = text.find(needle).expect("the needle");
        let at = Located {
            path: PathBuf::from("units.txt"),
            start: start as u32,
            end: start.saturating_add(needle.len()) as u32,
        };
        doc_comment(text, &at)
    }

    #[test]
    fn a_doc_comment_is_the_run_of_comment_lines_above_it() {
        let text = "# how wide\n# in metres\nwidth 3\n";
        assert_eq!(docs_above(text, "width 3"), ["how wide", "in metres"]);
    }

    #[test]
    fn a_blank_line_ends_the_run() {
        let text = "# about something else\n\nwidth 3\n";
        assert!(docs_above(text, "width 3").is_empty());
    }

    #[test]
    fn both_markers_read_the_same_way() {
        assert_eq!(docs_above("// how wide\nwidth 3\n", "width 3"), ["how wide"]);
    }

    #[test]
    fn a_declaration_on_the_first_line_has_nothing_above_it() {
        assert!(docs_above("width 3\n", "width 3").is_empty());
    }
}
