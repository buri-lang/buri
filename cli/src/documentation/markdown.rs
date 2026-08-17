//! Just enough Markdown to serve the documentation from the binary.
//!
//! This is not a CommonMark implementation and does not want to be. The corpus
//! it reads is one we write, so it needs exactly four things and needs them to
//! be exact: where the fenced blocks are (the doctest harness compiles them),
//! where the headings are (assembly numbers them and search ranks on them),
//! where the links point (a test resolves every one), and a terminal rendering
//! a reader will accept.
//!
//! The rules it deliberately does not implement — setext headings, reference
//! links, HTML blocks, `~~~` fences — are rejected or ignored rather than
//! half-supported, because a document that renders differently here than on
//! GitHub is a second source of truth, which is the thing this whole feature
//! exists to prevent.
//!
//! Nothing here may panic on the text it is handed. Most of the corpus ships
//! inside the binary, but `buri docs test` reads any markdown file in the
//! repository you are standing in, and the API reference renders `///`
//! comments out of somebody else's source — so a document with an emoji at an
//! awkward offset or a fence nobody closed is ordinary input, not an anomaly.
#![allow(
    clippy::arithmetic_side_effects,
    reason = "every operand is a byte offset into, or the length of, a `&str` already in memory, \
              so a sum is bounded by that string's length and cannot overflow; the subtractions, \
              which can underflow, are guarded or saturating at each site"
)]

use std::fmt::Write as _;

// ---------------------------------------------------------------------------
// Fences
// ---------------------------------------------------------------------------

/// Tracks whether a line walk is inside a fenced block.
///
/// Every scanner in this file shares it. They must agree on where the fences
/// are or they disagree about the document: if `headings` closed a block that
/// `fences` left open, a heading inside a quoted example would be numbered.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Where {
    /// Ordinary prose.
    Outside,
    /// The ``` line that opens or closes a block.
    Delimiter,
    /// Inside a fenced block.
    Inside,
}

#[derive(Default)]
struct FenceState {
    open: bool,
}

impl FenceState {
    fn feed(&mut self, line: &str) -> Where {
        let trimmed = line.trim_start();
        let Some(rest) = trimmed.strip_prefix("```") else {
            return if self.open { Where::Inside } else { Where::Outside };
        };
        if self.open {
            // Only a bare ``` closes, per CommonMark. That is what lets a
            // document quote a fenced block inside a ```text block.
            if rest.trim().is_empty() {
                self.open = false;
                return Where::Delimiter;
            }
            return Where::Inside;
        }
        self.open = true;
        Where::Delimiter
    }
}

/// A parsed fence info string: ```` ```buri run wrap=body ctx=alloc ````.
///
/// The first word is the language, and lives on the [`Fence`] rather than
/// here: it is readable from a fence whose info string does not parse, and a
/// fence with no language at all is a bare ```` ``` ````. At most one further
/// bare word is the mode. Everything else is `key=value`, with `"` around a
/// value that has spaces.
///
/// There is deliberately no `Default`: an `Info` only ever comes from
/// [`parse_info`], so there is no such thing as an info string that failed to
/// parse and yet produced one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Info {
    pub mode: Option<String>,
    pub keys: Vec<(String, String)>,
}

impl Info {
    pub fn get(&self, key: &str) -> Option<&str> {
        self.keys.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }

    /// A `key=a,b,c` list, empty when the key is absent.
    pub fn list(&self, key: &str) -> Vec<String> {
        match self.get(key) {
            None => Vec::new(),
            Some(v) => v.split(',').map(str::trim).filter(|s| !s.is_empty()).map(String::from).collect(),
        }
    }
}

/// Splits an info string into words, keeping a double-quoted value in one
/// piece so `why="the grammar, not a program"` survives.
fn info_words(raw: &str) -> Result<Vec<String>, String> {
    let mut words = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    let mut any = false;
    for c in raw.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                any = true;
            }
            c if c.is_whitespace() && !quoted => {
                if any {
                    words.push(std::mem::take(&mut cur));
                    any = false;
                }
            }
            c => {
                cur.push(c);
                any = true;
            }
        }
    }
    if quoted {
        return Err(format!("unterminated `\"` in the fence info string `{}`", raw.trim()));
    }
    if any {
        words.push(cur);
    }
    Ok(words)
}

/// The language word of an info string: everything up to the first space.
///
/// Readable whether or not the rest of the info string parses, which is what
/// lets a malformed ```` ```buri ```` fence still be routed to the code that
/// knows how to complain about it.
pub fn info_lang(raw: &str) -> &str {
    raw.split_whitespace().next().unwrap_or("")
}

pub fn parse_info(raw: &str) -> Result<Info, String> {
    let words = info_words(raw)?;
    let mut info = Info { mode: None, keys: Vec::new() };
    for (i, w) in words.iter().enumerate() {
        if i == 0 {
            continue;
        }
        match w.split_once('=') {
            Some((k, v)) => {
                if k.is_empty() {
                    return Err(format!("`{w}` has no key before its `=`"));
                }
                if info.keys.iter().any(|(existing, _)| existing == k) {
                    return Err(format!("`{k}` is given twice"));
                }
                info.keys.push((k.to_string(), v.to_string()));
            }
            None => {
                if let Some(first) = &info.mode {
                    return Err(format!(
                        "`{first}` and `{w}` are both modes; a fence takes at most one"
                    ));
                }
                info.mode = Some(w.clone());
            }
        }
    }
    Ok(info)
}

#[derive(Clone, Debug)]
pub struct Fence<'a> {
    /// The fence's language, or `""` for a bare ```` ``` ````.
    pub lang: &'a str,
    /// The rest of the info string, or the reason it does not parse.
    ///
    /// A `Result` rather than a best effort: a fence whose info string is
    /// malformed used to fall back to an empty `Info`, which read as "a fence
    /// in no language" and so was neither compiled nor reported.
    pub info: Result<Info, String>,
    /// The info string exactly as written, for error messages.
    pub raw_info: &'a str,
    /// The block's contents, with the opener's indentation removed and a
    /// trailing newline if non-empty.
    pub body: String,
    /// 1-based line of the ``` that opens the block.
    pub line: usize,
    /// 1-based line of the block's first content line.
    pub body_line: usize,
    pub indent: usize,
    /// Byte range of the whole block, opener through closer.
    pub start: usize,
    pub end: usize,
}

/// Every fenced block, in order.
///
/// A ``` inside a block is content, not an opener — which is why this is a
/// state machine over lines rather than a search. Blocks indented inside a
/// list item are found, and their indentation is stripped, so an example does
/// not have to be flush-left to be compiled.
pub fn fences(text: &str) -> Vec<Fence<'_>> {
    let mut out = Vec::new();
    let mut state = FenceState::default();
    let mut open: Option<(usize, usize, usize, &str, usize)> = None; // start, line, indent, info, body_start
    for (line_no, offset, line) in lines(text) {
        let trimmed = line.trim_start();
        let indent = line.len().saturating_sub(trimmed.len());
        if state.feed(line) != Where::Delimiter {
            continue;
        }
        match open.take() {
            None => {
                let rest = trimmed.strip_prefix("```").unwrap_or("");
                open = Some((offset, line_no, indent, rest, offset + line.len() + 1));
            }
            Some((start, open_line, open_indent, raw_info, body_start)) => {
                let raw = text.get(body_start..offset.max(body_start)).unwrap_or("");
                out.push(Fence {
                    lang: info_lang(raw_info),
                    info: parse_info(raw_info),
                    raw_info,
                    body: strip_indent(raw, open_indent),
                    line: open_line,
                    body_line: open_line + 1,
                    indent: open_indent,
                    start,
                    end: offset + line.len(),
                });
            }
        }
    }
    // An unterminated fence runs to the end of the document. Reporting it is
    // the caller's job; losing its contents would be worse.
    if let Some((start, line, indent, raw_info, body_start)) = open {
        // A fence opened on the last line of a document that does not end in a
        // newline has a body that starts one byte past the end of the text.
        let raw = text.get(body_start..).unwrap_or("");
        out.push(Fence {
            lang: info_lang(raw_info),
            info: parse_info(raw_info),
            raw_info,
            body: strip_indent(raw, indent),
            line,
            body_line: line + 1,
            indent,
            start,
            end: text.len(),
        });
    }
    out
}

/// The line of a ``` with no partner, if the document has one.
pub fn unterminated_fence(text: &str) -> Option<usize> {
    let mut state = FenceState::default();
    let mut opened_at = None;
    for (line_no, _, line) in lines(text) {
        if state.feed(line) == Where::Delimiter {
            opened_at = if state.open { Some(line_no) } else { None };
        }
    }
    opened_at
}

fn strip_indent(raw: &str, indent: usize) -> String {
    if indent == 0 {
        return raw.to_string();
    }
    let mut out = String::with_capacity(raw.len());
    for line in raw.lines() {
        // A space is one byte, so a count of leading spaces is also the byte
        // offset just past them.
        let keep = line.bytes().take(indent).take_while(|b| *b == b' ').count();
        out.push_str(line.get(keep..).unwrap_or(line));
        out.push('\n');
    }
    out
}

// ---------------------------------------------------------------------------
// Headings
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct Heading<'a> {
    /// 1 for `#`, 2 for `##`, and so on.
    pub level: usize,
    pub title: &'a str,
    pub line: usize,
    pub start: usize,
}

/// An ATX heading's level and title, or `None` if the line is not one.
///
/// One definition because five scanners ask the question, and a document whose
/// renderer and whose numbering disagree about what a heading is would be
/// numbered in one place and not the other. Takes a line already trimmed of its
/// indentation.
fn atx(trimmed: &str) -> Option<(usize, &str)> {
    let hashes = trimmed.bytes().take_while(|b| *b == b'#').count();
    if hashes == 0 || hashes > 6 {
        return None;
    }
    // `#hashtag` is not a heading; ATX requires a space.
    let rest = trimmed.get(hashes..)?.strip_prefix(' ')?;
    Some((hashes, rest.trim()))
}

/// Every ATX heading outside a fenced block.
///
/// Skipping fenced blocks is not an optimization: `SPEC.md` contains shell
/// transcripts whose lines begin with `#`, and numbering them would corrupt
/// the document.
pub fn headings(text: &str) -> Vec<Heading<'_>> {
    let mut out = Vec::new();
    let mut state = FenceState::default();
    for (line_no, offset, line) in lines(text) {
        if state.feed(line) != Where::Outside {
            continue;
        }
        let Some((level, title)) = atx(line.trim_start()) else { continue };
        out.push(Heading { level, title, line: line_no, start: offset });
    }
    out
}

/// GitHub's anchor algorithm: lowercase, drop everything that is not
/// alphanumeric, space, hyphen, or underscore, then spaces become hyphens.
///
/// Reproduced exactly rather than approximated, because `every_link_resolves`
/// checks our `#anchor`s against it and a near-miss would either pass links
/// that break on GitHub or fail links that work.
pub fn slug(title: &str) -> String {
    let mut out = String::with_capacity(title.len());
    for c in title.chars() {
        if c.is_alphanumeric() || c == '-' || c == '_' {
            out.extend(c.to_lowercase());
        } else if c == ' ' {
            out.push('-');
        }
    }
    out
}

/// Adds `by` to every heading level outside a fenced block.
pub fn shift_headings(text: &str, by: isize) -> String {
    let mut out = String::with_capacity(text.len());
    let mut state = FenceState::default();
    for line in text.lines() {
        let trimmed = line.trim_start();
        let outside = state.feed(line) == Where::Outside;
        let Some((hashes, title)) = outside.then(|| atx(trimmed)).flatten() else {
            out.push_str(line);
            out.push('\n');
            continue;
        };
        let level = (hashes as isize).saturating_add(by).clamp(1, 6) as usize;
        let indent = line.strip_suffix(trimmed).unwrap_or("");
        let _ = writeln!(out, "{indent}{} {title}", "#".repeat(level));
    }
    out
}

/// Prefixes each heading with its section number: the shallowest heading gets
/// `prefix`, the next level down `prefix.1`, `prefix.2`, and so on.
///
/// Numbering is positional *within one topic*, which is safe — a topic is a
/// unit somebody edits as a whole. Numbering across topics is not, which is
/// why `doc_assemble` pins the top-level number by hand.
pub fn number_headings(text: &str, prefix: &str) -> String {
    let base = headings(text).iter().map(|h| h.level).min().unwrap_or(1);
    let mut counters: Vec<usize> = Vec::new();
    let mut out = String::with_capacity(text.len());
    let mut state = FenceState::default();
    for line in text.lines() {
        let trimmed = line.trim_start();
        let outside = state.feed(line) == Where::Outside;
        let Some((hashes, title)) = outside.then(|| atx(trimmed)).flatten() else {
            out.push_str(line);
            out.push('\n');
            continue;
        };
        let depth = hashes.saturating_sub(base);
        // The truncate and the pushes together leave `depth` as the last slot,
        // whichever side of it the previous heading's depth was on.
        counters.truncate(depth + 1);
        while counters.len() <= depth {
            counters.push(0);
        }
        if let Some(n) = counters.last_mut() {
            *n += 1;
        }
        let number = if depth == 0 {
            prefix.to_string()
        } else {
            let tail: Vec<String> =
                counters.get(1..=depth).unwrap_or(&[]).iter().map(usize::to_string).collect();
            format!("{prefix}.{}", tail.join("."))
        };
        let _ = writeln!(out, "{} {number} {title}", "#".repeat(hashes));
    }
    out
}

/// A bulleted table of contents linking to every heading between level 2 and
/// `max_level` inclusive.
pub fn toc(text: &str, max_level: usize) -> String {
    let mut out = String::new();
    for h in headings(text) {
        if h.level < 2 || h.level > max_level {
            continue;
        }
        let indent = "  ".repeat(h.level.saturating_sub(2));
        let _ = writeln!(out, "{indent}- [{}](#{})", h.title, slug(h.title));
    }
    out
}

// ---------------------------------------------------------------------------
// Links
// ---------------------------------------------------------------------------

/// Where a link points.
///
/// This used to be a `target` and an `anchor` with an empty string standing for
/// "absent" in either position, which made `("", "")` — a link pointing nowhere
/// — indistinguishable from a link to a heading called nothing, and made every
/// consumer re-derive the case from two `is_empty()` tests.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Dest {
    /// `[text](#anchor)` — a heading in this document.
    SameDoc { anchor: String },
    /// `[text](path)` or `[text](path#anchor)`.
    File { path: String, anchor: Option<String> },
    /// `http://` or `https://`. Outside the corpus, so nothing resolves it.
    External { url: String },
    /// `[text]()`. No document wants one; it is reported rather than dropped so
    /// that the link checker is the thing that says so.
    Nowhere,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Link {
    pub text: String,
    pub dest: Dest,
    pub line: usize,
}

/// One link destination, as written.
fn dest_of(dest: &str) -> Dest {
    if dest.starts_with("http://") || dest.starts_with("https://") {
        return Dest::External { url: dest.to_string() };
    }
    if let Some(anchor) = dest.strip_prefix('#') {
        return Dest::SameDoc { anchor: anchor.to_string() };
    }
    if dest.is_empty() {
        return Dest::Nowhere;
    }
    match dest.split_once('#') {
        Some((path, anchor)) => {
            Dest::File { path: path.to_string(), anchor: Some(anchor.to_string()) }
        }
        None => Dest::File { path: dest.to_string(), anchor: None },
    }
}

/// Every inline `[text](target)` outside a fenced block or a code span.
///
/// Reference links (`[text][id]`) and autolinks are not produced; the corpus
/// uses neither, and a test asserts the corpus keeps not using them.
pub fn links(text: &str) -> Vec<Link> {
    let mut out = Vec::new();
    let mut state = FenceState::default();
    for (line_no, _, line) in lines(text) {
        if state.feed(line) != Where::Outside {
            continue;
        }
        let b = line.as_bytes();
        let mut i = 0;
        let mut in_code = false;
        // Every byte this walk stops on is ASCII, and an ASCII byte in UTF-8 is
        // always a whole character, so each offset it hands to a slice is on a
        // character boundary.
        while let Some(&c) = b.get(i) {
            match c {
                b'`' => {
                    in_code = !in_code;
                    i += 1;
                }
                b'\\' => i += 2,
                b'[' if !in_code => {
                    let Some(close) = find_balanced(b, i, b'[', b']') else {
                        i += 1;
                        continue;
                    };
                    if b.get(close + 1) != Some(&b'(') {
                        i = close + 1;
                        continue;
                    }
                    let Some(paren) = find_balanced(b, close + 1, b'(', b')') else {
                        i = close + 1;
                        continue;
                    };
                    let dest = line.get(close + 2..paren).unwrap_or("");
                    // A title after the destination, as in `(url "title")`.
                    let dest = dest.split_whitespace().next().unwrap_or("");
                    out.push(Link {
                        text: line.get(i + 1..close).unwrap_or("").to_string(),
                        dest: dest_of(dest),
                        line: line_no,
                    });
                    i = paren + 1;
                }
                _ => i += 1,
            }
        }
    }
    out
}

/// The offset of the `close` that matches the `open` at `from`, if there is
/// one. A `\` escapes whatever follows it.
fn find_balanced(b: &[u8], from: usize, open: u8, close: u8) -> Option<usize> {
    let mut depth = 0usize;
    let mut escaped = false;
    for (i, c) in b.iter().enumerate().skip(from) {
        if escaped {
            escaped = false;
            continue;
        }
        match *c {
            b'\\' => escaped = true,
            c if c == open => depth += 1,
            c if c == close => {
                // Saturating because a caller that pointed `from` at something
                // other than an `open` would otherwise underflow; the answer it
                // gets, "the close is right here", is the same one the balanced
                // count would give.
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Summaries and density
// ---------------------------------------------------------------------------

/// The first sentence of the first paragraph, for index listings and search
/// results. Headings, fences, lists, and block quotes are skipped, so the
/// summary is prose or nothing.
pub fn summary(text: &str) -> String {
    let mut state = FenceState::default();
    for line in text.lines() {
        if state.feed(line) != Where::Outside {
            continue;
        }
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        if t.starts_with('#') || t.starts_with('>') || t.starts_with('-') || t.starts_with('|')
            || t.starts_with("<!--")
        {
            continue;
        }
        return first_sentence(t);
    }
    String::new()
}

/// Splits on the first `. ` that is not inside a code span and not part of an
/// abbreviation we use (`e.g.`, `i.e.`, `SPEC 5.1.`).
fn first_sentence(line: &str) -> String {
    let b = line.as_bytes();
    let mut in_code = false;
    // `wrapping_sub` rather than a bounds test: at the start of the line the
    // index wraps to something no slice contains, and "there is no byte before
    // this one" is exactly what each of these questions wants to hear.
    for (i, c) in b.iter().enumerate() {
        match *c {
            b'`' => in_code = !in_code,
            b'.' if !in_code => {
                let next = b.get(i + 1);
                if next.is_some() && next != Some(&b' ') {
                    continue;
                }
                // A digit on either side means a version or a section number.
                let prev_digit = b.get(i.wrapping_sub(1)).is_some_and(u8::is_ascii_digit);
                let next_digit = b.get(i + 2).is_some_and(u8::is_ascii_digit);
                if prev_digit && next_digit {
                    continue;
                }
                // A single letter before the dot is an initial or `e.g`.
                if b.get(i.wrapping_sub(1)).is_some_and(u8::is_ascii_alphabetic)
                    && b.get(i.wrapping_sub(2)).is_some_and(|c| !c.is_ascii_alphanumeric())
                {
                    continue;
                }
                return line.get(..=i).unwrap_or(line).to_string();
            }
            _ => {}
        }
    }
    line.to_string()
}

/// Sections whose prose is background rather than reference. Dense mode keeps
/// their headings so the reader knows they exist and can ask for them, and
/// drops their bodies.
const BACKGROUND_HEADINGS: &[&str] =
    &["why ", "rationale", "considered and cut", "background", "history", "non-goals"];

fn is_background(title: &str) -> bool {
    let t = title.trim().to_lowercase();
    let t = t.trim_start_matches(|c: char| c.is_ascii_digit() || c == '.' || c == ' ');
    BACKGROUND_HEADINGS.iter().any(|h| t.starts_with(h))
}

/// The token-frugal rendering, for agents.
///
/// Keeps every heading, **every fenced block verbatim**, every table, and the
/// first sentence of each paragraph. Drops block quotes and the bodies of
/// background sections. Code is what a caller most needs and is never
/// abridged — a dense doc that dropped its examples would be worse than no
/// doc, because it would look complete.
pub fn dense(text: &str) -> String {
    let mut out = String::with_capacity(text.len() / 2);
    let mut state = FenceState::default();
    let mut skipping_at: Option<usize> = None;
    let mut paragraph = String::new();

    let flush = |out: &mut String, paragraph: &mut String| {
        if !paragraph.is_empty() {
            let _ = writeln!(out, "{}\n", first_sentence(paragraph.trim()));
            paragraph.clear();
        }
    };

    for line in text.lines() {
        let t = line.trim();

        // A fenced block is reproduced whole, delimiters and all, even inside
        // a section dense would otherwise skip.
        match state.feed(line) {
            Where::Delimiter => {
                flush(&mut out, &mut paragraph);
                out.push_str(line);
                out.push('\n');
                continue;
            }
            Where::Inside => {
                out.push_str(line);
                out.push('\n');
                continue;
            }
            Where::Outside => {}
        }

        if let Some((hashes, title)) = atx(t) {
            flush(&mut out, &mut paragraph);
            skipping_at = match skipping_at {
                // A heading at or above the level that started the skip ends it.
                Some(level) if hashes <= level => {
                    if is_background(title) {
                        Some(hashes)
                    } else {
                        None
                    }
                }
                Some(level) => Some(level),
                None if is_background(title) => Some(hashes),
                None => None,
            };
            let _ = writeln!(out, "{line}\n");
            continue;
        }

        if skipping_at.is_some() {
            continue;
        }
        if t.is_empty() {
            flush(&mut out, &mut paragraph);
            continue;
        }
        if t.starts_with('>') {
            continue;
        }
        // Tables and lists are already dense; pass them through whole.
        if t.starts_with('|') || t.starts_with("- ") || t.starts_with("* ")
            || t.chars().next().is_some_and(|c| c.is_ascii_digit()) && t.contains(". ")
        {
            flush(&mut out, &mut paragraph);
            out.push_str(line);
            out.push('\n');
            continue;
        }
        if !paragraph.is_empty() {
            paragraph.push(' ');
        }
        paragraph.push_str(t);
    }
    flush(&mut out, &mut paragraph);
    collapse_blank_runs(&out)
}

fn collapse_blank_runs(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut blank = 0;
    for line in text.lines() {
        if line.trim().is_empty() {
            blank += 1;
            if blank > 1 {
                continue;
            }
        } else {
            blank = 0;
        }
        out.push_str(line);
        out.push('\n');
    }
    while out.ends_with("\n\n") {
        out.pop();
    }
    out
}

// ---------------------------------------------------------------------------
// Terminal rendering
// ---------------------------------------------------------------------------

const BOLD: &str = "\x1b[1m";
const DIM: &str = "\x1b[2m";
const BLUE: &str = "\x1b[1;34m";
const RESET: &str = "\x1b[0m";

/// Markdown as a terminal reader wants it: headings bold, code indented and
/// dim, paragraphs wrapped, tables and lists left alone.
///
/// Wrapping stops at `width` columns but never inside a fenced block — a
/// wrapped program is not a program, and somebody will copy it.
pub fn to_terminal(text: &str, width: super::Width, color: bool) -> String {
    let (bold, dim, blue, reset) =
        if color { (BOLD, DIM, BLUE, RESET) } else { ("", "", "", "") };
    // Already clamped: `Width` has no other constructor.
    let width = width.get();
    let mut out = String::with_capacity(text.len());
    let mut state = FenceState::default();
    let mut paragraph = String::new();
    let mut table: Vec<String> = Vec::new();

    let flush = |out: &mut String, paragraph: &mut String| {
        if !paragraph.is_empty() {
            wrap_into(out, paragraph.trim(), width, "");
            out.push('\n');
            paragraph.clear();
        }
    };

    for line in text.lines() {
        let t = line.trim_end();
        let trimmed = t.trim_start();

        // The delimiters are dropped and the body is indented instead, which
        // is how a terminal shows a block. The body is never wrapped: a
        // wrapped program is not a program, and somebody will copy it.
        match state.feed(line) {
            Where::Delimiter => {
                flush(&mut out, &mut paragraph);
                continue;
            }
            Where::Inside => {
                let _ = writeln!(out, "  {dim}{t}{reset}");
                continue;
            }
            Where::Outside => {}
        }
        if t.trim().is_empty() {
            flush(&mut out, &mut paragraph);
            out.push('\n');
            continue;
        }
        if t.starts_with("<!--") {
            continue;
        }

        if let Some((hashes, heading)) = atx(trimmed) {
            flush(&mut out, &mut paragraph);
            let title = inline(heading, color);
            if hashes <= 2 {
                let _ = writeln!(out, "{bold}{title}{reset}");
                let _ = writeln!(out, "{blue}{}{reset}", "─".repeat(title.chars().count().min(width)));
            } else {
                let _ = writeln!(out, "{bold}{title}{reset}");
            }
            continue;
        }
        // A table is gathered whole so its columns can be aligned; a raw
        // `|---|---|` row is noise a reader has to look past.
        if trimmed.starts_with('|') {
            flush(&mut out, &mut paragraph);
            table.push(trimmed.to_string());
            continue;
        }
        if !table.is_empty() {
            render_table(&mut out, &table, width, color);
            table.clear();
        }
        // Lists and block quotes keep their shape; only their inline markup is
        // rendered.
        if trimmed.starts_with("- ") || trimmed.starts_with("* ") || trimmed.starts_with('>') {
            flush(&mut out, &mut paragraph);
            let indent = t.strip_suffix(trimmed).unwrap_or("");
            let _ = writeln!(out, "{indent}{}", inline(trimmed, color));
            continue;
        }
        if !paragraph.is_empty() {
            paragraph.push(' ');
        }
        paragraph.push_str(trimmed);
    }
    flush(&mut out, &mut paragraph);
    if !table.is_empty() {
        render_table(&mut out, &table, width, color);
    }
    collapse_blank_runs(&out)
}

/// A pipe table as aligned columns, with the alignment row dropped and the
/// header underlined. The last column absorbs the remaining width and wraps
/// inside it, because that is nearly always the description.
fn render_table(out: &mut String, rows: &[String], width: usize, color: bool) {
    let (bold, reset) = if color { (BOLD, RESET) } else { ("", "") };
    let cells: Vec<Vec<String>> = rows
        .iter()
        .filter(|r| !is_alignment_row(r))
        .map(|r| {
            r.trim().trim_matches('|').split('|').map(|c| inline(c.trim(), false)).collect()
        })
        .collect();
    if cells.is_empty() {
        return;
    }
    let columns = cells.iter().map(|r| r.len()).max().unwrap_or(0);
    // Every column but the last is sized to its widest cell; the last takes
    // what is left.
    let mut widths = vec![0usize; columns];
    for row in &cells {
        // `widths` is as long as the widest row, so the zip covers every cell.
        for (w, c) in widths.iter_mut().zip(row) {
            *w = (*w).max(c.chars().count());
        }
    }
    // `split_last` rather than `columns - 1`: it is the same "all but the last
    // column", and it says what a table with no columns at all does.
    let Some((_, leading)) = widths.split_last() else { return };
    let fixed: usize = leading.iter().map(|w| w + 2).sum();
    let last = width.saturating_sub(fixed + 2).max(20);

    for (n, row) in cells.iter().enumerate() {
        let mut lead = String::from("  ");
        for (i, w) in leading.iter().enumerate() {
            let cell = row.get(i).map(String::as_str).unwrap_or("");
            let _ = write!(lead, "{cell:<w$}  ", w = *w);
        }
        let tail = row.get(leading.len()).map(String::as_str).unwrap_or("");
        let mut wrapped = String::new();
        wrap_into(&mut wrapped, tail, last, "");
        for (k, line) in wrapped.lines().enumerate() {
            if k == 0 {
                if n == 0 {
                    let _ = writeln!(out, "{bold}{lead}{line}{reset}");
                } else {
                    let _ = writeln!(out, "{lead}{line}");
                }
            } else {
                let _ = writeln!(out, "{:w$}{line}", "", w = lead.chars().count());
            }
        }
    }
    out.push('\n');
}

/// The `|---|:--:|` row a table puts under its header.
fn is_alignment_row(row: &str) -> bool {
    row.trim().trim_matches('|').split('|').all(|c| {
        let c = c.trim();
        !c.is_empty() && c.chars().all(|ch| ch == '-' || ch == ':')
    })
}

/// Inline markup: `code` dim, **bold** bold, and `[text](target)` shown as the
/// text followed by the target, since a terminal cannot be clicked.
fn inline(s: &str, color: bool) -> String {
    let (bold, dim, reset) = if color { (BOLD, DIM, RESET) } else { ("", "", "") };
    let mut out = String::with_capacity(s.len());
    let b = s.as_bytes();
    let mut i = 0;
    // Every jump below lands just past an ASCII delimiter, and the fall-through
    // advances by one whole character, so `i` is always on a character
    // boundary — but each slice is taken with `get` anyway, because the cost of
    // being wrong about that is a panic in the middle of somebody's `///`
    // comment.
    while let Some(&c) = b.get(i) {
        if c == b'`' {
            if let Some(end) = s.get(i + 1..).and_then(|rest| rest.find('`')) {
                let _ = write!(out, "{dim}{}{reset}", s.get(i + 1..i + 1 + end).unwrap_or(""));
                i += end + 2;
                continue;
            }
        }
        if c == b'*' && b.get(i + 1) == Some(&b'*') {
            if let Some(end) = s.get(i + 2..).and_then(|rest| rest.find("**")) {
                let _ = write!(out, "{bold}{}{reset}", s.get(i + 2..i + 2 + end).unwrap_or(""));
                i += end + 4;
                continue;
            }
        }
        if c == b'[' {
            if let Some(close) = find_balanced(b, i, b'[', b']') {
                if b.get(close + 1) == Some(&b'(') {
                    if let Some(paren) = find_balanced(b, close + 1, b'(', b')') {
                        let label = s.get(i + 1..close).unwrap_or("");
                        let dest = s.get(close + 2..paren).unwrap_or("");
                        let _ = write!(out, "{label} {dim}({dest}){reset}");
                        i = paren + 1;
                        continue;
                    }
                }
            }
        }
        let Some(ch) = s.get(i..).and_then(|rest| rest.chars().next()) else { break };
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

fn wrap_into(out: &mut String, text: &str, width: usize, indent: &str) {
    let rendered = inline(text, false);
    let mut col = indent.len();
    out.push_str(indent);
    for word in rendered.split_whitespace() {
        let w = word.chars().count();
        if col > indent.len() && col + 1 + w > width {
            out.push('\n');
            out.push_str(indent);
            col = indent.len();
        } else if col > indent.len() {
            out.push(' ');
            col += 1;
        }
        out.push_str(word);
        col += w;
    }
    out.push('\n');
}

// ---------------------------------------------------------------------------
// Shared line walk
// ---------------------------------------------------------------------------

/// `(1-based line number, byte offset of the line, the line without its
/// newline)` for every line, including a trailing empty one only if the text
/// does not end in a newline.
fn lines(text: &str) -> impl Iterator<Item = (usize, usize, &str)> {
    let mut offset = 0;
    text.lines().enumerate().map(move |(i, line)| {
        let at = offset;
        offset += line.len() + 1;
        (i + 1, at, line)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SPEC: &str = include_str!("../../../SPEC.md");

    #[test]
    fn slugs_match_github() {
        // Both of these are live anchors in the checked-in documents.
        assert_eq!(
            slug("12. Why the grammar is context-free and unambiguous"),
            "12-why-the-grammar-is-context-free-and-unambiguous"
        );
        assert_eq!(slug("Why `forbids` has no platforms"), "why-forbids-has-no-platforms");
        assert_eq!(slug("`Result` is must-use"), "result-is-must-use");
        assert_eq!(slug("6.8 `?` — error propagation"), "68---error-propagation");
    }

    #[test]
    fn fences_are_found_and_dedented() {
        let text = "intro\n\n```buri run wrap=body\nlet a = 1;\n```\n\nafter\n";
        let f = fences(text);
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].lang, "buri");
        let info = f[0].info.as_ref().unwrap();
        assert_eq!(info.mode.as_deref(), Some("run"));
        assert_eq!(info.get("wrap"), Some("body"));
        assert_eq!(f[0].body, "let a = 1;\n");
        assert_eq!(f[0].line, 3);
        assert_eq!(f[0].body_line, 4);
    }

    #[test]
    fn a_fence_indented_in_a_list_is_found() {
        let text = "- a step:\n\n  ```buri\n  let a = 1;\n  ```\n";
        let f = fences(text);
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].indent, 2);
        assert_eq!(f[0].body, "let a = 1;\n");
    }

    #[test]
    fn backticks_inside_a_fence_are_content() {
        let text = "```text\n```buri\n```\n";
        let f = fences(text);
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].lang, "text");
        assert_eq!(f[0].body, "```buri\n");
    }

    /// A document is text somebody else wrote. These are the shapes that used
    /// to index past the end of one, or into the middle of a character.
    #[test]
    fn a_malformed_document_is_read_rather_than_indexed_past() {
        // A fence opened on the last line, with no newline after it: the body
        // begins one byte past the end of the document.
        assert_eq!(fences("```buri").len(), 1);
        assert_eq!(fences("```buri")[0].body, "");
        assert_eq!(unterminated_fence("```buri"), Some(1));

        // Multi-byte characters wherever a byte walk might stop between them.
        for text in ["# 🎈\n\n[é](ü.md#ñ)\n\n| á | é |\n|---|---|\n| ü | ñ |\n", "`é", "**é", "[é"] {
            let _ = fences(text);
            let _ = headings(text);
            let _ = links(text);
            let _ = summary(text);
            let _ = dense(text);
            let _ = number_headings(text, "1");
            let _ = shift_headings(text, 1);
            let _ = to_terminal(text, super::super::Width::default(), true);
        }
    }

    #[test]
    fn info_strings_parse() {
        let i = parse_info("buri ignore why=\"the precedence table, not a program\"").unwrap();
        assert_eq!(i.mode.as_deref(), Some("ignore"));
        assert_eq!(i.get("why"), Some("the precedence table, not a program"));

        let i = parse_info("buri check ctx=alloc,stdout").unwrap();
        assert_eq!(i.list("ctx"), vec!["alloc", "stdout"]);

        assert!(parse_info("buri run check").is_err(), "two modes is an error");
        assert!(parse_info("buri why=\"unclosed").is_err());
    }

    #[test]
    fn headings_skip_fenced_blocks() {
        let text = "# Title\n\n```sh\n# not a heading\n```\n\n## Real\n";
        let h = headings(text);
        assert_eq!(h.len(), 2);
        assert_eq!(h[0].title, "Title");
        assert_eq!(h[1].title, "Real");
        assert_eq!(h[1].line, 7);
    }

    #[test]
    fn shifting_headings_is_invertible() {
        let shifted = shift_headings(SPEC, 1);
        let back = shift_headings(&shifted, -1);
        // `#` cannot shift below 1, so compare the levels we did not clamp.
        let original: Vec<usize> = headings(SPEC).iter().map(|h| h.level).collect();
        let round: Vec<usize> = headings(&back).iter().map(|h| h.level).collect();
        assert_eq!(original, round);
    }

    #[test]
    fn headings_are_numbered_within_a_topic() {
        let text = "# Effects\n\n## The model\n\n### Detail\n\n## The rule\n";
        let numbered = number_headings(text, "10");
        let got: Vec<&str> = headings(&numbered).iter().map(|h| h.title).collect();
        assert_eq!(got, vec!["10 Effects", "10.1 The model", "10.1.1 Detail", "10.2 The rule"]);
    }

    #[test]
    fn links_are_extracted_with_their_anchors() {
        let text = "see [tags](cli/src/docs/build/tags.md#why-forbids) and `[not a link](x)`\n";
        let l = links(text);
        assert_eq!(l.len(), 1);
        assert_eq!(l[0].text, "tags");
        assert_eq!(
            l[0].dest,
            Dest::File {
                path: "cli/src/docs/build/tags.md".into(),
                anchor: Some("why-forbids".into()),
            }
        );

        // The three other shapes a destination comes in, and the fourth that
        // used to be spelled with two empty strings.
        let l = links("[a](#here) [b](other.md) [c](https://x.test) [d]()\n");
        let dests: Vec<&Dest> = l.iter().map(|l| &l.dest).collect();
        assert_eq!(
            dests,
            vec![
                &Dest::SameDoc { anchor: "here".into() },
                &Dest::File { path: "other.md".into(), anchor: None },
                &Dest::External { url: "https://x.test".into() },
                &Dest::Nowhere,
            ]
        );
    }

    #[test]
    fn the_spec_is_scannable() {
        // A guard on the scanner, not on the document: if these collapse to
        // zero the scanner broke, and every downstream test would pass
        // vacuously.
        let f = fences(SPEC);
        assert!(f.len() > 50, "found only {} fences in SPEC.md", f.len());
        assert!(f.iter().any(|f| f.lang == "buri"));
        assert!(headings(SPEC).len() > 50);
        assert_eq!(unterminated_fence(SPEC), None);
    }

    #[test]
    fn summaries_are_one_sentence() {
        let text = "# T\n\nEffects travel as a `ctx` parameter. The rest follows.\n";
        assert_eq!(summary(text), "Effects travel as a `ctx` parameter.");
    }

    #[test]
    fn dense_keeps_every_example() {
        let text = "# T\n\nA long paragraph. With a second sentence that dense drops.\n\n\
                    ```buri\nlet a = 1;\n```\n\n## Why this is so\n\nBackground prose.\n";
        let d = dense(text);
        assert!(d.contains("let a = 1;"), "dense dropped a fence:\n{d}");
        assert!(!d.contains("second sentence"));
        assert!(d.contains("## Why this is so"), "dense should keep the heading");
        assert!(!d.contains("Background prose"), "dense should drop a background body");
    }

    #[test]
    fn dense_is_shorter_than_the_original() {
        // A quarter is the floor, not the target — the observed saving on
        // SPEC.md is over 40%. The floor is set well below it so the test
        // fails when dense stops working, not when a document gains prose.
        let d = dense(SPEC);
        assert!(d.len() * 4 < SPEC.len() * 3, "dense SPEC is {} of {}", d.len(), SPEC.len());
        // Every fence body survives verbatim.
        for f in fences(SPEC) {
            if f.body.trim().is_empty() {
                continue;
            }
            assert!(d.contains(f.body.trim()), "dense dropped the fence at SPEC.md:{}", f.line);
        }
    }
}
