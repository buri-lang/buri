//! Diagnostics: source map, spans, and the renderer.
//!
//! The output shape is fixed by the examples throughout `SPEC.md` and
//! `cli/src/docs/`:
//!
//! ```text
//! error: cmd/server/routes.buri imports //lib/money, which is not in deps
//!   --> cmd/server/routes.buri:3:6
//!    |
//!  3 | from "//lib/money" import { Cents, format };
//!    |      ^^^^^^^^^^^^^
//!    |
//!    = expected: a module path this target may see
//!    = actual: //lib/money, which is not among cmd/server's dependencies
//!    = fix: add "//lib/money" to deps in cmd/server/BUILD.buri
//! ```
//!
//! Four things every diagnostic answers, in a fixed order, so that neither a
//! person nor a program has to infer any of them:
//!
//! * **where** — the span, rendered as a caret under the source line
//! * **expected** — what the language required at that location
//! * **actual** — what the source says instead
//! * **fix** — the concrete edit that resolves it
//!
//! `expected` and `actual` are omitted where the error is not a mismatch (a
//! duplicate declaration has no "expected"), but `fix` is not: if a diagnostic
//! cannot say what to do about it, it is not finished. The reject corpus
//! asserts exactly that, case by case.
//!
//! [`Diagnostic::to_json`] renders the same content as one JSON object per
//! line, for `buri <cmd> --error-format=json`.

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
    /// What the language required at this location. `None` where the error is
    /// not a mismatch and inventing one would be noise.
    pub expected: Option<String>,
    /// What the source says instead.
    pub actual: Option<String>,
    /// The `= ...` lines beneath the snippet: the background a reader needs,
    /// as opposed to the edit, which is `fix`.
    pub notes: Vec<String>,
    /// The concrete edit that resolves this. Every diagnostic has one.
    pub fix: Option<String>,
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
            expected: None,
            actual: None,
            notes: Vec::new(),
            fix: None,
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

    pub fn with_fix(mut self, fix: impl Into<String>) -> Diagnostic {
        self.fix = Some(fix.into());
        self
    }

    /// The two halves of a mismatch, which are almost always set together.
    pub fn with_mismatch(
        mut self,
        expected: impl Into<String>,
        actual: impl Into<String>,
    ) -> Diagnostic {
        self.expected = Some(expected.into());
        self.actual = Some(actual.into());
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

    /// A stable name for the rule this diagnostic enforces.
    ///
    /// The code is what `buri docs error <code>` explains, and what a reader
    /// greps for. It is kebab-case rather than a number because a number is
    /// unsearchable in a repository whose whole aesthetic is that the message
    /// says the thing.
    pub fn code(&mut self, code: impl Into<String>) -> &mut Diagnostic {
        self.code = Some(code.into());
        self
    }

    pub fn fix(&mut self, fix: impl Into<String>) -> &mut Diagnostic {
        self.fix = Some(fix.into());
        self
    }

    pub fn mismatch(
        &mut self,
        expected: impl Into<String>,
        actual: impl Into<String>,
    ) -> &mut Diagnostic {
        self.expected = Some(expected.into());
        self.actual = Some(actual.into());
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
            // The `=` sits in the gutter column, directly under the `|`s above
            // it, so the left edge of the block is a straight line.
            let gutter = self.gutter_width(d);
            for line in Self::trailer(d) {
                let mut lines = line.lines();
                if let Some(first) = lines.next() {
                    let _ = writeln!(out, "{:w$}{c_blue}={c_reset} {first}", "", w = gutter);
                }
                for cont in lines {
                    let _ = writeln!(out, "{:w$}  {cont}", "", w = gutter);
                }
            }
        } else {
            for line in Self::trailer(d) {
                let _ = writeln!(out, "  = {line}");
            }
        }
        out
    }

    /// The `= ...` block, in the order a reader needs it: what was required,
    /// what is there, why, and then what to do — the last line being the one
    /// to act on.
    fn trailer(d: &Diagnostic) -> Vec<String> {
        let mut out = Vec::new();
        if let Some(e) = &d.expected {
            out.push(format!("expected: {e}"));
        }
        if let Some(a) = &d.actual {
            out.push(format!("actual: {a}"));
        }
        out.extend(d.notes.iter().cloned());
        if let Some(f) = &d.fix {
            out.push(format!("fix: {f}"));
        }
        out
    }

    /// One JSON object per diagnostic, for `--error-format=json`.
    ///
    /// The shape is flat and the field names are the questions they answer, so
    /// a consumer needs no schema to use it: `severity`, `message`,
    /// `location`, `expected`, `actual`, `notes`, `fix`, and `related`. The
    /// content is the same as the human form; the rendering is not repeated
    /// here, because a consumer of this wants the fields, not a picture of
    /// them.
    pub fn to_json(&self, d: &Diagnostic) -> String {
        let mut out = String::from("{");
        let _ = write!(out, "\"severity\":{}", json_str(d.severity.label()));
        let _ = write!(out, ",\"message\":{}", json_str(&d.message));
        if let Some(code) = &d.code {
            let _ = write!(out, ",\"code\":{}", json_str(code));
        }
        out.push_str(",\"location\":");
        out.push_str(&self.json_location(d.span, d.label.as_deref()));
        for (key, value) in [("expected", &d.expected), ("actual", &d.actual), ("fix", &d.fix)] {
            if let Some(v) = value {
                let _ = write!(out, ",\"{key}\":{}", json_str(v));
            }
        }
        out.push_str(",\"notes\":[");
        for (i, n) in d.notes.iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            out.push_str(&json_str(n));
        }
        out.push_str("],\"related\":[");
        for (i, sub) in d.subs.iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            out.push_str(&self.json_location(sub.span, Some(&sub.label)));
        }
        out.push(']');
        out.push('}');
        out
    }

    fn json_location(&self, span: Span, label: Option<&str>) -> String {
        if span.is_none() {
            return "null".into();
        }
        let f = self.get(span.file);
        let (line, column) = f.line_col(span.start);
        let (end_line, end_column) = f.line_col(span.end);
        let mut out = String::from("{");
        let _ = write!(out, "\"file\":{}", json_str(&f.name));
        let _ = write!(out, ",\"line\":{line},\"column\":{column}");
        let _ = write!(out, ",\"endLine\":{end_line},\"endColumn\":{end_column}");
        let _ = write!(out, ",\"text\":{}", json_str(f.line_text(line)));
        if let Some(l) = label {
            let _ = write!(out, ",\"label\":{}", json_str(l));
        }
        out.push('}');
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

/// A JSON string literal. There is no serde here, on purpose: the toolchain
/// has no dependencies, and this is the whole of what it needs.
///
/// Public because `buri docs --format=json` emits the same shape of output and
/// must escape it the same way; one escaper means one set of edge cases.
pub fn json_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Joins names for a message, each in backticks: ``a``, ``b`` and ``c``. Every
/// other identifier in a diagnostic is quoted, and a list of them should not be
/// the exception.
pub fn names(items: &[String]) -> String {
    let quoted: Vec<String> = items.iter().map(|n| format!("`{n}`")).collect();
    match quoted.split_last() {
        None => String::new(),
        Some((last, [])) => last.clone(),
        Some((last, rest)) => format!("{} and {last}", rest.join(", ")),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn one_file() -> (SourceMap, FileId) {
        let mut map = SourceMap::new();
        let id = map.add(
            "cmd/x/main.buri",
            PathBuf::from("/tmp/main.buri"),
            "fn f(): Int {\n  a + b\n}\n".to_string(),
        );
        (map, id)
    }

    #[test]
    fn the_trailer_reads_expected_actual_notes_then_fix() {
        let (map, id) = one_file();
        let d = Diagnostic::error(Span::new(id, 16, 21), "expected `I32`, found `I64`")
            .with_mismatch("`I32`", "`I64`")
            .with_note("there is no implicit promotion of any kind")
            .with_fix("convert explicitly with `.toI32()?`");
        let text = map.render(&d, false);
        let trailer: Vec<&str> =
            text.lines().filter_map(|l| l.trim_start().strip_prefix("= ")).collect();
        assert_eq!(
            trailer,
            vec![
                "expected: `I32`",
                "actual: `I64`",
                "there is no implicit promotion of any kind",
                "fix: convert explicitly with `.toI32()?`",
            ],
            "the fix is last, because it is the line to act on"
        );
    }

    #[test]
    fn the_gutter_is_a_straight_line() {
        let (map, id) = one_file();
        let d = Diagnostic::error(Span::new(id, 16, 21), "no")
            .with_fix("do something else");
        // Every `|` and every `=` sits in the same column.
        let columns: Vec<usize> = map
            .render(&d, false)
            .lines()
            .filter(|l| l.contains('|') || l.trim_start().starts_with('='))
            .map(|l| l.find(['|', '=']).unwrap())
            .collect();
        assert!(columns.windows(2).all(|w| w[0] == w[1]), "ragged gutter: {columns:?}");
    }

    #[test]
    fn json_omits_what_does_not_apply() {
        let (map, id) = one_file();
        let d = Diagnostic::error(Span::new(id, 16, 17), "nope").with_fix("do it differently");
        let json = map.to_json(&d);
        assert!(json.contains(r#""severity":"error""#));
        assert!(json.contains(r#""fix":"do it differently""#));
        assert!(json.contains(r#""line":2,"column":3"#));
        assert!(json.contains(r#""text":"  a + b""#));
        // Absent means "not applicable", so the keys are not emitted at all.
        assert!(!json.contains("expected"), "{json}");
        assert!(!json.contains("actual"), "{json}");
        assert!(!json.contains("rendered"), "{json}");
        assert!(json.ends_with('}') && json.starts_with('{'));
        assert!(!json.contains('\n'), "one diagnostic is one line");
    }

    #[test]
    fn json_escapes_what_would_break_the_line() {
        let mut map = SourceMap::new();
        let id = map.add("a.buri", PathBuf::new(), "let s = \"x\";\n".to_string());
        let d = Diagnostic::error(Span::new(id, 8, 11), "a \"quoted\" thing\twith a tab")
            .with_fix("mind the \\ backslash");
        let json = map.to_json(&d);
        assert!(json.contains(r#"a \"quoted\" thing\twith a tab"#), "{json}");
        assert!(json.contains(r#"mind the \\ backslash"#), "{json}");
        assert!(!json.contains('\t'));
    }
}
