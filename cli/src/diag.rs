//! Diagnostics: source map, spans, and the renderer.
//!
//! The output shape is fixed by the examples throughout `SPEC.md` and
//! `build-system/`:
//!
//! ```text
//! error: cmd/server/routes.buri imports //lib/money, which is not in deps
//!   --> cmd/server/routes.buri:3:6
//!    |
//!  3 | from "//lib/money" import { Cents, format };
//!    |      ^^^^^^^^^^^^^
//!    |
//!    = add "//lib/money" to deps in cmd/server/BUILD.buri
//! ```

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

/// Index into the [`SourceMap`].
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct FileId(pub u32);

impl FileId {
    /// A span with no file, for diagnostics that are about the invocation
    /// rather than about a location in a file.
    pub const NONE: FileId = FileId(u32::MAX);
}

/// A byte range within one file.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Span {
    pub file: FileId,
    pub start: u32,
    pub end: u32,
}

impl Span {
    pub fn new(file: FileId, start: usize, end: usize) -> Span {
        Span { file, start: start as u32, end: end as u32 }
    }

    /// A zero-width span, used where a construct is missing rather than wrong.
    pub fn point(file: FileId, at: usize) -> Span {
        Span::new(file, at, at)
    }

    pub const NONE: Span = Span { file: FileId::NONE, start: 0, end: 0 };

    pub fn is_none(&self) -> bool {
        self.file == FileId::NONE
    }

    pub fn start_point(&self) -> Span {
        Span { file: self.file, start: self.start, end: self.start }
    }

    /// The span covering both, assuming they are in the same file.
    pub fn to(self, other: Span) -> Span {
        if self.is_none() {
            return other;
        }
        if other.is_none() {
            return self;
        }
        Span { file: self.file, start: self.start.min(other.start), end: self.end.max(other.end) }
    }
}

impl Default for Span {
    fn default() -> Span {
        Span::NONE
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Severity {
    Error,
    Warning,
    Note,
}

impl Severity {
    fn label(self) -> &'static str {
        match self {
            Severity::Error => "error",
            Severity::Warning => "warning",
            Severity::Note => "note",
        }
    }
}

/// A secondary span, rendered after the primary one with its own caret line.
#[derive(Clone, Debug)]
pub struct SubSpan {
    pub span: Span,
    pub label: String,
}

#[derive(Clone, Debug)]
pub struct Diagnostic {
    pub severity: Severity,
    pub message: String,
    pub span: Span,
    /// Text printed beside the carets.
    pub label: Option<String>,
    /// The `= ...` lines beneath the snippet. These carry the "what to do
    /// about it" half of every diagnostic in the spec.
    pub notes: Vec<String>,
    pub subs: Vec<SubSpan>,
    /// Lint name, for `buri lint` output.
    pub code: Option<String>,
}

impl Diagnostic {
    pub fn error(span: Span, message: impl Into<String>) -> Diagnostic {
        Diagnostic {
            severity: Severity::Error,
            message: message.into(),
            span,
            label: None,
            notes: Vec::new(),
            subs: Vec::new(),
            code: None,
        }
    }

    pub fn warning(span: Span, message: impl Into<String>) -> Diagnostic {
        Diagnostic { severity: Severity::Warning, ..Diagnostic::error(span, message) }
    }

    pub fn with_label(mut self, label: impl Into<String>) -> Diagnostic {
        self.label = Some(label.into());
        self
    }

    pub fn with_note(mut self, note: impl Into<String>) -> Diagnostic {
        self.notes.push(note.into());
        self
    }

    pub fn with_sub(mut self, span: Span, label: impl Into<String>) -> Diagnostic {
        self.subs.push(SubSpan { span, label: label.into() });
        self
    }

    pub fn with_code(mut self, code: impl Into<String>) -> Diagnostic {
        self.code = Some(code.into());
        self
    }

    /// The in-place forms, for use on the `&mut Diagnostic` a sink hands back.
    pub fn note(&mut self, note: impl Into<String>) -> &mut Diagnostic {
        self.notes.push(note.into());
        self
    }

    pub fn sub(&mut self, span: Span, label: impl Into<String>) -> &mut Diagnostic {
        self.subs.push(SubSpan { span, label: label.into() });
        self
    }

    pub fn label(&mut self, label: impl Into<String>) -> &mut Diagnostic {
        self.label = Some(label.into());
        self
    }

    pub fn is_error(&self) -> bool {
        self.severity == Severity::Error
    }
}

pub struct SourceFile {
    /// Repository-relative where possible, so that two checkouts in different
    /// directories produce identical diagnostics and identical cache keys.
    pub name: String,
    pub abs_path: PathBuf,
    pub text: String,
    /// Byte offset of the start of each line.
    line_starts: Vec<u32>,
}

impl SourceFile {
    fn new(name: String, abs_path: PathBuf, text: String) -> SourceFile {
        let mut line_starts = vec![0u32];
        for (i, b) in text.bytes().enumerate() {
            if b == b'\n' {
                line_starts.push(i as u32 + 1);
            }
        }
        SourceFile { name, abs_path, text, line_starts }
    }

    /// 1-based line and column (column counted in characters, not bytes).
    pub fn line_col(&self, offset: u32) -> (usize, usize) {
        let offset = offset.min(self.text.len() as u32);
        let line = match self.line_starts.binary_search(&offset) {
            Ok(i) => i,
            Err(i) => i - 1,
        };
        let start = self.line_starts[line] as usize;
        let col = self.text[start..offset as usize].chars().count() + 1;
        (line + 1, col)
    }

    pub fn line_text(&self, line: usize) -> &str {
        let start = self.line_starts[line - 1] as usize;
        let end = self
            .line_starts
            .get(line)
            .map(|&e| e as usize)
            .unwrap_or(self.text.len());
        self.text[start..end].trim_end_matches(['\n', '\r'])
    }

    pub fn line_count(&self) -> usize {
        self.line_starts.len()
    }
}

#[derive(Default)]
pub struct SourceMap {
    files: Vec<SourceFile>,
}

impl SourceMap {
    pub fn new() -> SourceMap {
        SourceMap::default()
    }

    pub fn add(&mut self, name: impl Into<String>, abs_path: PathBuf, text: String) -> FileId {
        let id = FileId(self.files.len() as u32);
        self.files.push(SourceFile::new(name.into(), abs_path, text));
        id
    }

    /// Load a file, reusing the entry if it is already present.
    pub fn load(&mut self, name: &str, abs_path: &Path) -> std::io::Result<FileId> {
        if let Some(id) = self.find(name) {
            return Ok(id);
        }
        let text = std::fs::read_to_string(abs_path)?;
        Ok(self.add(name, abs_path.to_path_buf(), text))
    }

    pub fn find(&self, name: &str) -> Option<FileId> {
        self.files.iter().position(|f| f.name == name).map(|i| FileId(i as u32))
    }

    pub fn get(&self, id: FileId) -> &SourceFile {
        &self.files[id.0 as usize]
    }

    pub fn name(&self, id: FileId) -> &str {
        if id == FileId::NONE {
            "<none>"
        } else {
            &self.get(id).name
        }
    }

    pub fn text(&self, id: FileId) -> &str {
        &self.get(id).text
    }

    /// The source text a span covers.
    pub fn snippet(&self, span: Span) -> &str {
        if span.is_none() {
            return "";
        }
        let f = self.get(span.file);
        &f.text[span.start as usize..(span.end as usize).min(f.text.len())]
    }

    pub fn render(&self, d: &Diagnostic, color: bool) -> String {
        let mut out = String::new();
        let (c_bold, c_red, c_yellow, c_blue, c_reset) = if color {
            ("\x1b[1m", "\x1b[1;31m", "\x1b[1;33m", "\x1b[1;34m", "\x1b[0m")
        } else {
            ("", "", "", "", "")
        };
        let sev_color = match d.severity {
            Severity::Error => c_red,
            Severity::Warning => c_yellow,
            Severity::Note => c_blue,
        };

        let head = match &d.code {
            Some(code) => format!("{}: {} [{}]", d.severity.label(), d.message, code),
            None => format!("{}: {}", d.severity.label(), d.message),
        };
        let (sev, rest) = head.split_at(d.severity.label().len());
        let _ = writeln!(out, "{sev_color}{sev}{c_reset}{c_bold}{rest}{c_reset}");

        if !d.span.is_none() {
            self.render_snippet(&mut out, d.span, d.label.as_deref(), sev_color, c_blue, c_reset);
            for sub in &d.subs {
                self.render_snippet(&mut out, sub.span, Some(&sub.label), c_blue, c_blue, c_reset);
            }
            let gutter = self.gutter_width(d);
            for note in &d.notes {
                let mut lines = note.lines();
                if let Some(first) = lines.next() {
                    let _ = writeln!(out, "{:w$} {c_blue}={c_reset} {first}", "", w = gutter);
                }
                for cont in lines {
                    let _ = writeln!(out, "{:w$}   {cont}", "", w = gutter);
                }
            }
        } else {
            for note in &d.notes {
                let _ = writeln!(out, "  = {note}");
            }
        }
        out
    }

    fn gutter_width(&self, d: &Diagnostic) -> usize {
        let mut w = 1;
        for span in std::iter::once(d.span).chain(d.subs.iter().map(|s| s.span)) {
            if span.is_none() {
                continue;
            }
            let (line, _) = self.get(span.file).line_col(span.start);
            w = w.max(line.to_string().len());
        }
        w + 1
    }

    fn render_snippet(
        &self,
        out: &mut String,
        span: Span,
        label: Option<&str>,
        caret_color: &str,
        c_blue: &str,
        c_reset: &str,
    ) {
        if span.is_none() {
            return;
        }
        let f = self.get(span.file);
        let (line, col) = f.line_col(span.start);
        let (end_line, end_col) = f.line_col(span.end);
        let gw = line.to_string().len().max(1) + 1;

        let _ = writeln!(out, "{:w$}{c_blue}-->{c_reset} {}:{}:{}", "", f.name, line, col, w = gw - 1);
        let _ = writeln!(out, "{:w$}{c_blue}|{c_reset}", "", w = gw);

        let text = f.line_text(line);
        let _ = writeln!(out, "{c_blue}{:>w$} |{c_reset} {}", line, text, w = gw - 1);

        // Carets under the span. A span crossing a line break is underlined to
        // the end of its first line, which is what the reader needs to see.
        let span_chars = if end_line == line {
            (end_col.saturating_sub(col)).max(1)
        } else {
            text.chars().count().saturating_sub(col - 1).max(1)
        };
        let pad: String = std::iter::repeat(' ').take(col - 1).collect();
        let carets: String = std::iter::repeat('^').take(span_chars).collect();
        match label {
            Some(l) => {
                let _ = writeln!(
                    out,
                    "{:w$}{c_blue}|{c_reset} {pad}{caret_color}{carets} {l}{c_reset}",
                    "",
                    w = gw
                );
            }
            None => {
                let _ = writeln!(
                    out,
                    "{:w$}{c_blue}|{c_reset} {pad}{caret_color}{carets}{c_reset}",
                    "",
                    w = gw
                );
            }
        }
        let _ = writeln!(out, "{:w$}{c_blue}|{c_reset}", "", w = gw);
    }
}

/// A diagnostic sink that keeps errors in source order and deduplicates.
#[derive(Default)]
pub struct Diagnostics {
    pub items: Vec<Diagnostic>,
}

impl Diagnostics {
    pub fn new() -> Diagnostics {
        Diagnostics::default()
    }

    pub fn push(&mut self, d: Diagnostic) {
        // The checker can reach the same bad construct through more than one
        // path; reporting it twice helps nobody.
        if self.items.iter().any(|e| e.message == d.message && e.span == d.span) {
            return;
        }
        self.items.push(d);
    }

    pub fn extend(&mut self, other: impl IntoIterator<Item = Diagnostic>) {
        for d in other {
            self.push(d);
        }
    }

    pub fn has_errors(&self) -> bool {
        self.items.iter().any(|d| d.is_error())
    }

    pub fn error_count(&self) -> usize {
        self.items.iter().filter(|d| d.is_error()).count()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Source order, errors before warnings at the same location.
    pub fn sort(&mut self, map: &SourceMap) {
        self.items.sort_by_key(|d| {
            let name = if d.span.is_none() { String::new() } else { map.name(d.span.file).to_string() };
            (name, d.span.start, d.severity == Severity::Warning)
        });
    }
}
