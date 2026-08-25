//! The frontmatter on a diagnostic's page, and the templates in it.
//!
//! A diagnostic's wording lives at the top of the page that explains it, in a
//! `---` fenced block, so that changing what a user reads is an edit to a
//! document rather than to a `format!` in the middle of the type checker:
//!
//! ```text
//! ---
//! title: Type arguments qualify a function, not a value
//! message: explicit type arguments qualify a function or a call
//! fix: attach the type arguments to the call, as in `{function}<Str>(x)`
//! ---
//! ```
//!
//! It is YAML the way the rest of this repository is JSON: hand-written,
//! because what is needed is a handful of scalars and a crate would be a
//! dependency for a hundred lines. Every value is a string, there are no lists
//! and no maps, and an unknown key is an error rather than something quietly
//! ignored — a misspelled `mesage` must fail the build, not print nothing.
//!
//! `{placeholder}` is the one piece of templating. The call site binds whole
//! phrases; there are no filters, no conditionals, and no pluralization,
//! because a wording that varies by more than an interpolated name is a second
//! diagnostic and should be a second code. `{{` and `}}` are the literal
//! braces.

use crate::diagnostics::Severity;

/// The fields a page may declare. Every one is a template except `title`,
/// which is the docs index's line and is never printed in a diagnostic.
#[derive(Clone, Debug)]
pub struct Frontmatter {
    pub title: String,
    pub severity: Severity,
    pub message: String,
    /// Printed beside the carets.
    pub label: Option<String>,
    /// One `= note:` line of background.
    pub note: Option<String>,
    /// The concrete edit. Optional here, required of every compiler
    /// diagnostic by the reject corpus.
    pub fix: Option<String>,
    /// False when the page says `reproduction: none` — the code cannot be
    /// provoked by a single-file program, so the page carries no `buri fail`
    /// block for the doctest suite to compile.
    pub reproducible: bool,
}

/// One page of a catalog, after its frontmatter has been read.
#[derive(Clone, Debug)]
pub struct Page {
    pub code: &'static str,
    pub front: Frontmatter,
    /// The markdown below the frontmatter, byte for byte as the file has it.
    pub body: &'static str,
}

/// Every page of one catalog, parsed once, with the pages that would not parse
/// kept beside them rather than dropped.
///
/// A page that is malformed is a bug in the repository, and it fails the tests
/// that walk `failures`. It must not fail the *compiler*: a user meeting a
/// broken page mid-diagnostic gets the diagnostic without the wording it could
/// not read, which is the old behaviour of a page with no frontmatter at all.
pub struct Catalog {
    pages: Vec<Page>,
    failures: Vec<String>,
}

impl Catalog {
    pub fn build(entries: &[(&'static str, &'static str)]) -> Catalog {
        let mut pages = Vec::new();
        let mut failures = Vec::new();
        for (code, text) in entries {
            match parse(text) {
                Ok(None) => {}
                Ok(Some((front, body))) => pages.push(Page { code, front, body }),
                Err(why) => failures.push(format!("{code}: {why}")),
            }
        }
        Catalog { pages, failures }
    }

    pub fn page(&self, code: &str) -> Option<&Page> {
        self.pages.iter().find(|p| p.code == code)
    }

    pub fn pages(&self) -> &[Page] {
        &self.pages
    }

    /// Why a page did not parse, one line each. Empty in a healthy tree.
    pub fn failures(&self) -> &[String] {
        &self.failures
    }
}

/// Splits a page into its frontmatter block and its body.
///
/// `Ok(None)` for a page that has none, which is how a page not yet migrated
/// keeps working; `Err` for one that opens a block and never closes it, which
/// is a typo rather than a decision.
pub fn parse(text: &str) -> Result<Option<(Frontmatter, &str)>, String> {
    let Some(rest) = text.strip_prefix("---\n") else { return Ok(None) };
    let mut offset = 0usize;
    let mut end = None;
    for line in rest.split_inclusive('\n') {
        if line.trim_end() == "---" {
            end = Some((offset, offset.saturating_add(line.len())));
            break;
        }
        offset = offset.saturating_add(line.len());
    }
    let Some((block_end, body_start)) = end else {
        return Err("the frontmatter block is opened by `---` and never closed".to_string());
    };
    let block = rest.get(..block_end).unwrap_or("");
    let body = rest.get(body_start..).unwrap_or("");
    Ok(Some((fields(block)?, body)))
}

/// The keys a page may carry. Anything else is a misspelling, and is reported
/// as one.
const KEYS: &[&str] = &["title", "severity", "message", "label", "note", "fix", "reproduction"];

fn fields(block: &str) -> Result<Frontmatter, String> {
    let mut found: Vec<(&str, String)> = Vec::new();
    for (number, line) in block.lines().enumerate() {
        let line = line.trim_end();
        let trimmed = line.trim_start();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let at = number.saturating_add(1);
        if trimmed.len() != line.len() {
            return Err(format!("line {at}: a value spans one line, so nothing is indented"));
        }
        let Some((key, raw)) = line.split_once(':') else {
            return Err(format!("line {at}: expected `key: value`, found `{line}`"));
        };
        if !KEYS.contains(&key) {
            return Err(format!("line {at}: `{key}` is not a frontmatter key"));
        }
        if found.iter().any(|(k, _)| *k == key) {
            return Err(format!("line {at}: `{key}` is set twice"));
        }
        found.push((key, scalar(raw).map_err(|why| format!("line {at}: {why}"))?));
    }

    let get = |key: &str| found.iter().find(|(k, _)| *k == key).map(|(_, v)| v.clone());
    let title = get("title").ok_or("a page needs a `title`")?;
    let message = get("message").ok_or("a page needs a `message`")?;
    let severity = match get("severity").as_deref() {
        None | Some("error") => Severity::Error,
        Some("warning") => Severity::Warning,
        Some(other) => return Err(format!("`severity: {other}`; expected `error` or `warning`")),
    };
    let reproducible = match get("reproduction").as_deref() {
        None => true,
        Some("none") => false,
        Some(other) => {
            return Err(format!("`reproduction: {other}`; the only value is `none`"));
        }
    };
    Ok(Frontmatter {
        title,
        severity,
        message,
        label: get("label"),
        note: get("note"),
        fix: get("fix"),
        reproducible,
    })
}

/// One scalar: bare, or quoted when it would otherwise begin or end with
/// whitespace. Backticks and colons are ordinary characters in a bare value,
/// because nearly every message has both.
fn scalar(raw: &str) -> Result<String, String> {
    let text = raw.trim();
    if text.is_empty() {
        return Err("the value is empty".to_string());
    }
    let quoted = |quote: char| {
        text.strip_prefix(quote)
            .and_then(|r| r.strip_suffix(quote))
            .filter(|_| text.chars().count() >= 2)
    };
    if let Some(inner) = quoted('\'') {
        return Ok(inner.to_string());
    }
    let Some(inner) = quoted('"') else { return Ok(text.to_string()) };
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => match chars.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some('"') => out.push('"'),
                Some('\\') => out.push('\\'),
                Some(other) => return Err(format!("`\\{other}` is not an escape")),
                None => return Err("the value ends in a backslash".to_string()),
            },
            '"' => return Err("a `\"` inside a quoted value is written `\\\"`".to_string()),
            c => out.push(c),
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Templates
// ---------------------------------------------------------------------------

/// A template is text with holes in it. `{{` and `}}` are the literal braces.
enum Piece<'a> {
    Text(&'a str),
    /// What stands between the braces, which the tests hold to snake_case.
    Hole(&'a str),
}

fn pieces(template: &str) -> Vec<Piece<'_>> {
    let mut out = Vec::new();
    let bytes = template.as_bytes();
    let mut at = 0usize;
    let mut text_from = 0usize;
    // Byte indices only ever land on `{` and `}`, which are ASCII, so every
    // slice below is on a character boundary.
    while at < bytes.len() {
        let here = bytes.get(at).copied().unwrap_or(0);
        let next = bytes.get(at.saturating_add(1)).copied();
        if (here == b'{' || here == b'}') && next == Some(here) {
            // A doubled brace: keep one of the pair and start again after both.
            out.push(Piece::Text(template.get(text_from..at.saturating_add(1)).unwrap_or("")));
            at = at.saturating_add(2);
            text_from = at;
            continue;
        }
        if here == b'{' {
            if let Some(close) = template
                .get(at..)
                .and_then(|r| r.find('}'))
                .map(|i| at.saturating_add(i))
            {
                let name = template.get(at.saturating_add(1)..close).unwrap_or("");
                if !name.is_empty() && !name.contains('{') {
                    out.push(Piece::Text(template.get(text_from..at).unwrap_or("")));
                    out.push(Piece::Hole(name));
                    at = close.saturating_add(1);
                    text_from = at;
                    continue;
                }
            }
        }
        at = at.saturating_add(1);
    }
    out.push(Piece::Text(template.get(text_from..).unwrap_or("")));
    out
}

/// Every hole in a template, in the order they appear.
pub fn placeholders(template: &str) -> Vec<&str> {
    pieces(template)
        .into_iter()
        .filter_map(|p| match p {
            Piece::Hole(name) => Some(name),
            Piece::Text(_) => None,
        })
        .collect()
}

/// The template with its bound holes filled in.
///
/// An unbound hole is left as it is written rather than dropped: this is the
/// release build's behaviour for a binding the call site forgot, and a visible
/// `{name}` is a bug report where a silently empty sentence is not. A debug
/// build never gets here — [`crate::diagnostics::Diagnostic`] checks the
/// bindings first and panics.
pub fn render(template: &str, bindings: &[(String, String)]) -> String {
    let mut out = String::with_capacity(template.len());
    for piece in pieces(template) {
        match piece {
            Piece::Text(text) => out.push_str(text),
            Piece::Hole(name) => match bindings.iter().find(|(n, _)| n == name) {
                Some((_, value)) => out.push_str(value),
                None => {
                    out.push('{');
                    out.push_str(name);
                    out.push('}');
                }
            },
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_page_without_frontmatter_is_not_an_error() {
        assert!(parse("# A page\n\nprose\n").expect("no frontmatter is fine").is_none());
    }

    #[test]
    fn the_fields_are_read_and_the_body_is_untouched() {
        let text = "---\ntitle: A title\nseverity: warning\nmessage: a message\n\
                    fix: do the other thing\n---\n# A page\n\nprose\n";
        let (front, body) = parse(text).expect("this parses").expect("there is frontmatter");
        assert_eq!(front.title, "A title");
        assert_eq!(front.severity, Severity::Warning);
        assert_eq!(front.message, "a message");
        assert_eq!(front.fix.as_deref(), Some("do the other thing"));
        assert!(front.reproducible);
        assert_eq!(body, "# A page\n\nprose\n");
    }

    /// Every one of these is a typo somebody will make, and each has to name
    /// itself rather than parse to something almost right.
    #[test]
    fn a_malformed_page_says_what_is_wrong_with_it() {
        for (text, expect) in [
            ("---\ntitle: t\n", "never closed"),
            ("---\ntitle: t\n---\n", "needs a `message`"),
            ("---\nmessage: m\n---\n", "needs a `title`"),
            ("---\ntitle: t\nmesage: m\n---\n", "not a frontmatter key"),
            ("---\ntitle: t\nmessage: m\nmessage: n\n---\n", "set twice"),
            ("---\ntitle: t\nmessage:\n---\n", "value is empty"),
            ("---\ntitle: t\nmessage: m\nseverity: loud\n---\n", "expected `error`"),
            ("---\ntitle: t\nmessage: m\nreproduction: maybe\n---\n", "the only value is `none`"),
            ("---\ntitle: t\nmessage: m\nnonsense\n---\n", "expected `key: value`"),
        ] {
            let why = parse(text).expect_err("this must not parse");
            assert!(why.contains(expect), "expected {expect:?} in {why:?}");
        }
    }

    #[test]
    fn a_quoted_value_keeps_its_edges() {
        let text = "---\ntitle: t\nmessage: \"  spaced  \"\nnote: 'plain'\n---\n";
        let (front, _) = parse(text).expect("this parses").expect("there is frontmatter");
        assert_eq!(front.message, "  spaced  ");
        assert_eq!(front.note.as_deref(), Some("plain"));
    }

    #[test]
    fn templates_fill_their_holes_and_keep_their_braces() {
        let bind = |n: &str, v: &str| (n.to_string(), v.to_string());
        assert_eq!(render("a {x} b", &[bind("x", "1")]), "a 1 b");
        assert_eq!(render("{{x}}", &[bind("x", "1")]), "{x}");
        assert_eq!(render("a {x} b", &[]), "a {x} b");
        assert_eq!(placeholders("as in `{function}<Str>(x)`"), vec!["function"]);
        assert_eq!(placeholders("{{not_a_hole}}"), Vec::<&str>::new());
        assert_eq!(placeholders("{a} and {b}"), vec!["a", "b"]);
    }
}
