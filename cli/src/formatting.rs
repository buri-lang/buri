//! The source formatter.
//!
//! No options and no configuration file: a formatter with options is a
//! formatter whose output is a repository decision. It re-prints the parsed
//! tree, keeping doc comments and ordinary comments with the declaration
//! beneath them.
//!
//! # The document algebra
//!
//! Layout is not decided while printing. The tree is first converted to a
//! `Doc` — Wadler's *A prettier printer*, in the form Prettier generalized it
//! — and a second pass lays that out. Nothing measures by rendering a string
//! and looking at it, and nothing writes output it may have to take back.
//!
//! Wadler's algebra, with his names in brackets:
//!
//! | `Doc`                | paper            | meaning |
//! |----------------------|------------------|---------|
//! | `Text`               | `text`           | a string, never containing a newline |
//! | `Concat`             | `<>`             | one after another |
//! | `Line`               | `line`           | a space when flat, a newline when broken |
//! | `SoftLine`           | —                | nothing when flat, a newline when broken |
//! | `HardLine`           | —                | always a newline |
//! | `Nest`               | `nest`           | one more level of indent, `INDENT` spaces |
//! | `Group`              | `group`          | flat if it fits on what is left of the line, else broken |
//!
//! Wadler's `group` is `flatten x <|> x`, and his `best`/`fits` choose between
//! them with one line of lookahead. That is what `render` and `fits` below are.
//! `SoftLine` and `HardLine` are the two `line` variants every implementation
//! adds; `HardLine` is what makes a group *unconditionally* broken, which the
//! paper gets from never putting a `line` in a group at all.
//!
//! Four extensions, all of them Prettier's, because the shapes this formatter
//! already committed to cannot be written in the core algebra:
//!
//! | `Doc`            | why |
//! |------------------|-----|
//! | `Fill`           | a list that reads *across*: break only where the next item will not fit. The clause of an import and the list of a `derive` are filled, because a name in one is not an element of anything; every other list is one item to a line. |
//! | `IfBreak`        | text that exists only in one of the two modes — the trailing comma a broken list gets and a flat one must not have. |
//! | `BreakParent`    | "whatever encloses this cannot be flat". A comment carries one, which is how a comment anywhere inside a construct stops it collapsing onto a line that has nowhere to put the comment. |
//! | `Blank`          | a line break with an empty line above it — at most one, and never directly under an opening brace. The paragraph breaks somebody typed are the one part of the layout that is *not* a function of the width, and this is where they live. |
//! | `Alt`            | Prettier's `conditionalGroup`: candidate layouts in order of preference. The first — "all of it on one line" — is measured strictly, so a forced break anywhere inside rules it out; every later one has already chosen where it breaks, and is measured by its *first line*. Two shapes need it, and neither is expressible with `group` alone, because both want one part of a construct to break while a part enclosing it stays flat. |
//!
//! `Alt` earns its place twice. A call whose last argument is a lambda or a
//! literal hugs — `b.mapCtx(ctx, fn(c, x) => {` … `}).join(ctx, "")` — which
//! asks the argument list to stay flat *while the argument inside it breaks*.
//! And a chain of method calls breaks at the dots with the first call left on
//! the base line, which is a different shape from the same doc broken, not a
//! flatter one. Both are Prettier's own uses of `conditionalGroup`.
//!
//! Break propagation is Prettier's `propagateBreaks`, computed on the way up:
//! `group` records whether its contents force a break, so `fits` never has to
//! look inside a group twice. An `Alt` propagates its *first* candidate's
//! breaks and no other's — if even the one-line candidate must break, so must
//! everything containing it, and which of the rest wins is not known until the
//! layout pass. `IfBreak` propagates neither branch, because both of them are
//! conditional on the answer the propagation is trying to compute.

use crate::diagnostics::{FileId, Span};
use crate::parsing::lexer::{lex, Comment, Tok};
use crate::parsing::tree::*;
use std::fmt::Write as _;

const WIDTH: usize = 88;

/// One level of indentation. Every indent in the output is a multiple of it,
/// and nothing else in this file knows a number of spaces.
const INDENT: usize = 4;

/// The formatter without its safety check, for the toolchain's own tests.
pub fn source_unchecked(text: &str) -> String {
    let parsed = crate::parsing::parser::parse(text, FileId(0));
    let mut tv = Comments::read(text);
    render(&Build { tv: &mut tv }.module(&parsed.module))
}

/// Returns `None` when the file does not parse, in which case it is left
/// exactly as it is.
pub fn source(text: &str) -> Option<String> {
    let parsed = crate::parsing::parser::parse(text, FileId(0));
    if !parsed.errors.is_empty() {
        return None;
    }
    let mut tv = Comments::read(text);
    let out = render(&Build { tv: &mut tv }.module(&parsed.module));

    // A formatter that produces something that does not parse is worse than no
    // formatter, so the output is checked before it is offered.
    let check = crate::parsing::parser::parse(&out, FileId(0));
    if !check.errors.is_empty() {
        return None;
    }
    // A formatter that drops a comment is worse still: the loss is invisible
    // in the output, which is the one place somebody might have looked. The
    // set is compared rather than the sequence, because the leading import run
    // is sorted and a comment travels with the import it sits above.
    let (mut before, mut after) = (comment_shape(text), comment_shape(&out));
    before.sort();
    after.sort();
    if before != after {
        return None;
    }
    Some(out)
}

// -- the document ----------------------------------------------------------

#[derive(Clone, Debug)]
enum Doc {
    Nil,
    /// Never contains a newline: a comment written over several lines is
    /// several `Text`s with `HardLine` between them.
    Text(String),
    Concat(Vec<Doc>),
    /// A space when flat, a line break when broken.
    Line,
    /// Nothing when flat, a line break when broken.
    SoftLine,
    /// A line break in either mode, and a break every enclosing group inherits.
    HardLine,
    /// A line break with an empty line above it — at most one, and never
    /// directly under an opening brace. That is the whole of the blank-line
    /// rule, and it lives here rather than in the printers because every
    /// printer wants the same answer.
    Blank,
    Nest(Box<Doc>),
    Group {
        doc: Box<Doc>,
        /// Whether the contents force a break, computed when the group is
        /// built. Prettier calls the pass that does this `propagateBreaks`.
        breaks: bool,
    },
    /// Items and the separators between them, alternating, broken only where
    /// the next item would not fit.
    Fill(Vec<Doc>),
    IfBreak(Box<Doc>, Box<Doc>),
    /// Nothing is printed; the enclosing group cannot be flat.
    BreakParent,
    /// Candidate layouts, most preferred first. The last is used when none of
    /// the others has a first line that fits.
    Alt(Vec<Doc>),
}

fn text(s: impl Into<String>) -> Doc {
    Doc::Text(s.into())
}

fn cat(v: Vec<Doc>) -> Doc {
    Doc::Concat(v)
}

fn nest(d: Doc) -> Doc {
    Doc::Nest(Box::new(d))
}

fn if_break(broken: Doc, flat: Doc) -> Doc {
    Doc::IfBreak(Box::new(broken), Box::new(flat))
}

/// Whether a document contains something that forces a line break.
///
/// `Alt` and `IfBreak` are opaque: which of their branches is printed is not
/// decided until the layout pass, so neither can commit an enclosing group.
fn breaks(d: &Doc) -> bool {
    match d {
        Doc::HardLine | Doc::Blank | Doc::BreakParent => true,
        Doc::Concat(xs) | Doc::Fill(xs) => xs.iter().any(breaks),
        Doc::Nest(x) => breaks(x),
        Doc::Group { breaks, .. } => *breaks,
        // The candidate a flat parent would print. If even that must break,
        // everything containing it must too; which of the *other* candidates
        // wins is not known until the layout pass, so they say nothing here.
        Doc::Alt(states) => states.first().is_some_and(breaks),
        _ => false,
    }
}

fn group(d: Doc) -> Doc {
    let breaks = breaks(&d);
    Doc::Group { doc: Box::new(d), breaks }
}

/// The same document with its outermost group broken.
///
/// This is what a hugged argument is: the call around it stays on one line
/// *because* the argument does not, so the argument may not be measured and
/// found to fit. Prettier writes this as a `group` built with
/// `shouldBreak: true`.
fn force(d: Doc) -> Doc {
    match d {
        Doc::Group { doc, .. } => Doc::Group { doc, breaks: true },
        // Forcing a set of candidates rules out the one that is all on a line.
        Doc::Alt(mut states) if states.len() > 1 => {
            states.remove(0);
            Doc::Alt(states)
        }
        Doc::Concat(mut xs) => {
            if let Some(last) = xs.pop() {
                xs.push(force(last));
            }
            Doc::Concat(xs)
        }
        Doc::Nest(x) => Doc::Nest(Box::new(force(*x))),
        other => other,
    }
}

fn join(sep: Doc, items: Vec<Doc>) -> Doc {
    let mut out = Vec::with_capacity(items.len() * 2);
    for (i, it) in items.into_iter().enumerate() {
        if i > 0 {
            out.push(sep.clone());
        }
        out.push(it);
    }
    cat(out)
}

/// A comma-separated list that breaks all at once: `(a, b)`, or one item to a
/// line with the trailing comma a broken list gets.
fn bracketed(open: &str, items: Vec<Doc>, close: &str) -> Doc {
    if items.is_empty() {
        return text(format!("{open}{close}"));
    }
    group(cat(vec![
        text(open),
        nest(cat(vec![
            Doc::SoftLine,
            join(cat(vec![text(","), Doc::Line]), items),
            if_break(text(","), Doc::Nil),
        ])),
        Doc::SoftLine,
        text(close),
    ]))
}

/// A list that reads across the page. Each item but the last carries its own
/// comma, so the width a break is measured against is the item *and* its
/// separator — which is what makes the filled shape the same one the widest
/// import in the suite was hand-wrapped into.
fn filled(items: Vec<Doc>) -> Doc {
    let last = items.len().saturating_sub(1);
    let mut parts = Vec::new();
    for (i, it) in items.into_iter().enumerate() {
        if i > 0 {
            parts.push(Doc::Line);
        }
        parts.push(if i == last { it } else { cat(vec![it, text(",")]) });
    }
    Doc::Fill(parts)
}

// -- the layout pass -------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Mode {
    Flat,
    Break,
}

/// One entry of the work stack: a document, or the tail of a `Fill` being
/// consumed two items at a time.
#[derive(Clone, Copy)]
enum Cmd<'a> {
    One(&'a Doc),
    Fill(&'a [Doc]),
}

type Frame<'a> = (usize, Mode, Cmd<'a>);

/// Wadler's `fits`: whether what comes next reaches the end of its line inside
/// `width` columns.
///
/// It is linear because it stops at the first line break it can prove will be
/// taken — a `HardLine`, or any line inside a group already known to break —
/// and otherwise gives up as soon as it has spent the width. `rest` is the
/// printer's own stack, so a group is measured together with the `;` or `,`
/// that will follow it, which is why no printer has to be told what trails it.
///
/// `must_be_flat` is Prettier's flag of the same name: with it, a break that
/// *will* be taken is a failure rather than the end of the measurement. It is
/// what asks "does all of this fit on one line", as against "does the first
/// line of this fit".
fn fits(
    rest: &[Frame<'_>],
    seed: Vec<Frame<'_>>,
    width: usize,
    must_be_flat: bool,
) -> bool {
    let mut w = width as isize;
    let mut local = seed;
    let mut ri = rest.len();
    // `must_be_flat` is a question about the *candidate*, not about the line it
    // is being fitted into. Once the walk leaves the seed and starts on what
    // the printer already has stacked behind it, a break there is the end of
    // the line — which is exactly what "it fits" means.
    let mut strict = must_be_flat;
    loop {
        if w < 0 {
            return false;
        }
        let (ind, mode, cmd) = match local.pop() {
            Some(f) => f,
            None => {
                if ri == 0 {
                    return true;
                }
                ri -= 1;
                strict = false;
                rest[ri]
            }
        };
        let d = match cmd {
            Cmd::Fill(parts) => {
                for p in parts.iter().rev() {
                    local.push((ind, mode, Cmd::One(p)));
                }
                continue;
            }
            Cmd::One(d) => d,
        };
        match d {
            Doc::Nil => {}
            Doc::BreakParent => {
                if strict {
                    return false;
                }
            }
            Doc::Text(s) => w -= s.chars().count() as isize,
            Doc::Concat(xs) => {
                for x in xs.iter().rev() {
                    local.push((ind, mode, Cmd::One(x)));
                }
            }
            Doc::Fill(xs) => {
                for x in xs.iter().rev() {
                    local.push((ind, mode, Cmd::One(x)));
                }
            }
            Doc::Nest(x) => local.push((ind + 1, mode, Cmd::One(x))),
            Doc::Line => {
                if mode == Mode::Break {
                    return true;
                }
                w -= 1;
            }
            Doc::SoftLine => {
                if mode == Mode::Break {
                    return true;
                }
            }
            Doc::HardLine | Doc::Blank => return !strict,
            Doc::IfBreak(b, f) => {
                local.push((ind, mode, Cmd::One(if mode == Mode::Break { b } else { f })))
            }
            Doc::Group { doc, breaks } => {
                if *breaks && strict {
                    return false;
                }
                let m = if *breaks { Mode::Break } else { mode };
                local.push((ind, m, Cmd::One(doc)));
            }
            // The candidate a flat parent would print.
            Doc::Alt(states) => local.push((ind, mode, Cmd::One(&states[0]))),
        }
    }
}

fn indent_to(out: &mut String, ind: usize) {
    for _ in 0..ind * INDENT {
        out.push(' ');
    }
}

/// A line break. Whatever the line ended with in spaces goes with it: a line
/// that ends in a space is a diff nobody asked for.
///
/// A break asked for on a line nothing has been written to yet is the line it
/// asked for, so it makes no second one. That is what keeps a paragraph break
/// and the line break the construct around it also wants from stacking into
/// two empty lines.
fn newline(out: &mut String, pos: &mut usize, ind: usize) {
    let fresh = out.trim_end_matches(' ').ends_with('\n');
    while out.ends_with(' ') {
        out.pop();
    }
    if !fresh && !out.is_empty() {
        out.push('\n');
    }
    indent_to(out, ind);
    *pos = ind * INDENT;
}

/// A line break with an empty line above it — unless there is one already, or
/// a brace has just opened, because a comment on the first line of a block
/// wants no gap above it.
fn blank(out: &mut String, pos: &mut usize, ind: usize) {
    while out.ends_with(' ') {
        out.pop();
    }
    if out.is_empty() {
        return;
    }
    if !out.ends_with('\n') {
        out.push('\n');
    }
    if !out.ends_with("\n\n") && !out.ends_with("{\n") {
        out.push('\n');
    }
    indent_to(out, ind);
    *pos = ind * INDENT;
}

/// Wadler's `best`: lay the document out at the width, one pass, no backtracking.
fn render(doc: &Doc) -> String {
    let mut out = String::new();
    let mut pos = 0usize;
    let mut stack: Vec<Frame> = vec![(0, Mode::Break, Cmd::One(doc))];
    while let Some((ind, mode, cmd)) = stack.pop() {
        let d = match cmd {
            Cmd::Fill(parts) => {
                fill_step(&mut stack, ind, mode, parts, WIDTH.saturating_sub(pos));
                continue;
            }
            Cmd::One(d) => d,
        };
        match d {
            Doc::Nil | Doc::BreakParent => {}
            Doc::Text(s) => {
                out.push_str(s);
                pos += s.chars().count();
            }
            Doc::Concat(xs) => {
                for x in xs.iter().rev() {
                    stack.push((ind, mode, Cmd::One(x)));
                }
            }
            Doc::Nest(x) => stack.push((ind + 1, mode, Cmd::One(x))),
            Doc::Line => match mode {
                Mode::Flat => {
                    out.push(' ');
                    pos += 1;
                }
                Mode::Break => newline(&mut out, &mut pos, ind),
            },
            Doc::SoftLine => {
                if mode == Mode::Break {
                    newline(&mut out, &mut pos, ind);
                }
            }
            Doc::HardLine => newline(&mut out, &mut pos, ind),
            Doc::Blank => blank(&mut out, &mut pos, ind),
            Doc::IfBreak(b, f) => {
                stack.push((ind, mode, Cmd::One(if mode == Mode::Break { b } else { f })))
            }
            Doc::Group { doc, breaks } => {
                let m = if *breaks {
                    Mode::Break
                } else if mode == Mode::Flat {
                    // A flat parent has already been measured with this group
                    // inside it; measuring again would only cost time.
                    Mode::Flat
                } else if fits(
                    &stack,
                    vec![(ind, Mode::Flat, Cmd::One(doc))],
                    WIDTH.saturating_sub(pos),
                    false,
                ) {
                    Mode::Flat
                } else {
                    Mode::Break
                };
                stack.push((ind, m, Cmd::One(doc)));
            }
            Doc::Alt(states) => {
                if mode == Mode::Flat {
                    stack.push((ind, Mode::Flat, Cmd::One(&states[0])));
                    continue;
                }
                // The first candidate is "all of it on one line", so it is
                // measured strictly: a forced break anywhere inside it rules it
                // out. Every later candidate has already chosen where it
                // breaks, so what is asked of it is only that its first line
                // fits. Whichever wins is then laid out normally, so the groups
                // inside it still answer for themselves.
                let mut chosen = None;
                for (i, s) in states[..states.len() - 1].iter().enumerate() {
                    if fits(
                        &stack,
                        vec![(ind, Mode::Flat, Cmd::One(s))],
                        WIDTH.saturating_sub(pos),
                        i == 0,
                    ) {
                        chosen = Some(s);
                        break;
                    }
                }
                let s = chosen.unwrap_or_else(|| states.last().expect("non-empty"));
                stack.push((ind, Mode::Break, Cmd::One(s)));
            }
            Doc::Fill(parts) => {
                fill_step(&mut stack, ind, mode, parts, WIDTH.saturating_sub(pos))
            }
        }
    }
    out
}

/// One step of `Fill`: place the first item, then decide the separator after
/// it by whether the item *after* that would still fit.
fn fill_step<'a>(
    stack: &mut Vec<Frame<'a>>,
    ind: usize,
    mode: Mode,
    parts: &'a [Doc],
    rem: usize,
) {
    let Some(content) = parts.first() else { return };
    let content_fits = fits(&[], vec![(ind, Mode::Flat, Cmd::One(content))], rem, false);
    if parts.len() == 1 {
        stack.push((ind, if content_fits { Mode::Flat } else { Mode::Break }, Cmd::One(content)));
        return;
    }
    let ws = &parts[1];
    if parts.len() == 2 {
        let m = if content_fits { Mode::Flat } else { Mode::Break };
        stack.push((ind, m, Cmd::One(ws)));
        stack.push((ind, m, Cmd::One(content)));
        return;
    }
    let pair = fits(
        &[],
        vec![
            (ind, Mode::Flat, Cmd::One(&parts[2])),
            (ind, Mode::Flat, Cmd::One(ws)),
            (ind, Mode::Flat, Cmd::One(content)),
        ],
        rem,
        false,
    );
    stack.push((ind, mode, Cmd::Fill(&parts[2..])));
    if pair {
        stack.push((ind, Mode::Flat, Cmd::One(ws)));
        stack.push((ind, Mode::Flat, Cmd::One(content)));
    } else if content_fits {
        stack.push((ind, Mode::Break, Cmd::One(ws)));
        stack.push((ind, Mode::Flat, Cmd::One(content)));
    } else {
        stack.push((ind, Mode::Break, Cmd::One(ws)));
        stack.push((ind, Mode::Break, Cmd::One(content)));
    }
}

// -- comments --------------------------------------------------------------

/// A run of comments, doc-comment lines, and blank lines written above one
/// token, with that token's byte offset.
#[derive(Clone)]
struct Trivia {
    at: u32,
    comments: Vec<Comment>,
    docs: Vec<String>,
    /// A blank line sits between the comments and the doc lines under them.
    docs_blank: bool,
    /// A blank line sits above the run.
    blank: bool,
    /// A blank line sits between the last comment line and the token. At the
    /// top of a file that is what tells a header apart from a comment about
    /// the first import; anywhere else it is a paragraph break somebody typed,
    /// and it comes back.
    detached: bool,
}

impl Trivia {
    fn is_empty(&self) -> bool {
        self.comments.is_empty() && self.docs.is_empty()
    }
}

/// Every comment in the file, in source order, and which of them have been put
/// back.
///
/// The formatter re-prints a *tree*, and a comment belongs to no node of it —
/// it belongs to a byte offset. So the printers claim comments as they go, by
/// the offset of the token they were written above, and each construct sweeps
/// the range it is responsible for before it closes. Anything still unclaimed
/// when the module ends comes back at the end of the file: a comment in an
/// unusual place is worth moving, and is never worth deleting.
///
/// Each comment becomes `Text` lines with `HardLine` between them, so the
/// group it lands in cannot be flat. That is the whole of comment handling in
/// the layout: there is no construct that has to ask whether it contains one.
struct Comments {
    entries: Vec<Trivia>,
    used: Vec<bool>,
}

/// Whether nothing but whitespace, spanning a blank line, sits between `at`
/// and whatever was written before it.
fn blank_line_above(text: &str, at: u32) -> bool {
    let mut newlines = 0;
    for c in text[..at as usize].chars().rev() {
        match c {
            '\n' => newlines += 1,
            c if c.is_whitespace() => {}
            _ => break,
        }
        if newlines >= 2 {
            return true;
        }
    }
    false
}

impl Comments {
    fn read(text: &str) -> Comments {
        let lexed = lex(text, FileId(0));
        let entries: Vec<Trivia> = lexed
            .tokens
            .iter()
            .filter(|t| !t.comments.is_empty() || !t.docs.is_empty() || t.blank_before)
            .map(|t| Trivia {
                at: t.span.start,
                comments: t.comments.clone(),
                docs: t.docs.clone(),
                docs_blank: t.docs_blank,
                blank: t.blank_before,
                detached: blank_line_above(text, t.span.start),
            })
            .collect();
        let used = vec![false; entries.len()];
        Comments { entries, used }
    }

    /// Whether the comment above the token at `at` is separated from it by a
    /// blank line.
    fn is_detached(&self, at: u32) -> bool {
        self.entries.iter().any(|e| e.at == at && !e.comments.is_empty() && e.detached)
    }

    /// The trivia written above the token at `at`, marked as put back.
    fn take(&mut self, at: u32) -> Option<Trivia> {
        let i = self.entries.iter().position(|e| e.at == at)?;
        if self.used[i] {
            return None;
        }
        self.used[i] = true;
        Some(self.entries[i].clone())
    }

    /// Every comment not yet put back whose token lies in `lo ..= hi`, in
    /// source order. Blank-line-only trivia inside a construct is consumed and
    /// dropped: paragraph breaks are the printer's decision everywhere except
    /// above a comment, where the comment keeps the break it was given.
    fn drain(&mut self, lo: u32, hi: u32) -> Vec<Trivia> {
        let mut out = Vec::new();
        for i in 0..self.entries.len() {
            if self.used[i] || self.entries[i].at < lo || self.entries[i].at > hi {
                continue;
            }
            self.used[i] = true;
            if !self.entries[i].is_empty() {
                out.push(self.entries[i].clone());
            }
        }
        out
    }

    /// Whether a blank line was written above the token at `at`.
    ///
    /// Peeked rather than claimed: the sweep that follows consumes the record
    /// of it either way, and what the caller wants to know is whether somebody
    /// left a gap there, not whether anybody has asked yet.
    fn blank_at(&self, at: u32) -> bool {
        self.entries.iter().zip(&self.used).any(|(e, used)| !used && e.at == at && e.blank)
    }

    /// Whether a comment is waiting inside `lo ..= hi`. Only the constructs
    /// with an *empty* form ask — `trait T {}` has no line for a comment to go
    /// on, so it has to be printed as the other shape. Everywhere else the
    /// comment's own `HardLine` settles it.
    fn any_in(&self, lo: u32, hi: u32) -> bool {
        self.entries.iter().zip(&self.used).any(|(e, used)| {
            !used && e.at >= lo && e.at <= hi && !e.is_empty()
        })
    }
}

struct Build<'t> {
    tv: &'t mut Comments,
}

/// Which half of the run an import belongs to. The standard library comes
/// first because it is the part every module can assume; the repository's own
/// libraries are what distinguish this module from any other.
fn import_group(path: &str) -> u8 {
    if path.starts_with("//") {
        1
    } else {
        0
    }
}

/// Total order over the leading import run: group, then path, then the clause.
///
/// The clause breaks the tie between two imports of the same module — which is
/// legal, and is how a module takes both a namespace and a name from one path.
/// `*` sorts before `{`, so the namespace form leads.
fn import_key(item: &&Item) -> (u8, String, String) {
    let Item::Import(i) = item else { unreachable!("callers pass imports") };
    let clause = match &i.clause {
        ImportClause::Namespace(n) => format!("* as {}", n.name),
        ImportClause::Named(specs) => format!("{{ {} }}", spec_list(specs).join(", ")),
    };
    (import_group(&i.path), i.path.clone(), clause)
}

/// One specifier each: the name it exports, and the name it binds when they
/// differ — sorted, because a clause is a set exactly as the run of imports is.
///
/// Case-insensitively, so `Zed` and `apply` sort as a reader reads them rather
/// than as ASCII orders them, with the case-sensitive comparison breaking the
/// tie so the order is total.
fn spec_list(specs: &[ImportSpec]) -> Vec<String> {
    let mut out: Vec<String> = specs
        .iter()
        .map(|s| match &s.alias {
            Some(a) => format!("{} as {}", s.name.name, a.name),
            None => s.name.name.clone(),
        })
        .collect();
    out.sort_by(|a, b| a.to_lowercase().cmp(&b.to_lowercase()).then_with(|| a.cmp(b)));
    out
}

impl Build<'_> {
    // -- comments ----------------------------------------------------------

    /// Comment and doc lines, as lines of a document.
    ///
    /// The blank line *above* the run is the caller's, because who owns it
    /// differs: a declaration keeps the paragraph break it was written with, a
    /// sorted import takes the group's. The blanks *inside* the run are the
    /// comments' own — a section heading and the sentence under it are two
    /// paragraphs — and so is the one below it, which is what separates a
    /// heading from the declaration it introduces.
    fn trivia_doc(&mut self, ts: &[Trivia]) -> Doc {
        // Whatever encloses a comment cannot be flat: a line that holds a
        // whole body has nowhere to put one. Every construct used to ask this
        // question for itself; now the comment answers it, once, for all of
        // them.
        let mut lines: Vec<Doc> = vec![Doc::BreakParent];
        for t in ts {
            for (i, c) in t.comments.iter().enumerate() {
                if c.blank_before && i > 0 {
                    lines.push(Doc::Blank);
                }
                // A block comment written over several lines is one comment,
                // and the shape inside it is the author's. Its lines are moved
                // *together*: the first goes where the printer says and each of
                // the rest keeps the distance from it that it was written with.
                //
                // Re-indenting them to the printer's own level would move them
                // two columns further right on every run; leaving them at the
                // columns they were typed at would leave them behind the moment
                // anything around them is re-indented. The offset is the only
                // thing that is stable under both.
                for (j, l) in c.text.lines().enumerate() {
                    if j > 0 {
                        let indent = l.len() - l.trim_start().len();
                        let rel = indent.saturating_sub(c.column as usize);
                        lines.push(Doc::HardLine);
                        lines.push(text(format!("{}{}", " ".repeat(rel), l.trim())));
                    } else {
                        lines.push(text(l.trim_end().to_string()));
                    }
                }
                lines.push(Doc::HardLine);
            }
            if t.docs_blank && !t.comments.is_empty() {
                lines.push(Doc::Blank);
            }
            for d in &t.docs {
                let d = format!("/// {d}");
                lines.push(text(d.trim_end().to_string()));
                lines.push(Doc::HardLine);
            }
            if t.detached && !t.is_empty() {
                lines.push(Doc::Blank);
            }
        }
        // Every line above ends with its own break; the caller joins this with
        // what follows, so the last one is dropped.
        if matches!(lines.last(), Some(Doc::HardLine)) {
            lines.pop();
        }
        cat(lines)
    }

    /// Every comment still unclaimed inside `lo ..= hi`, as one line unit, or
    /// nothing when there is none.
    fn flush(&mut self, lo: u32, hi: u32) -> Option<Doc> {
        let ts = self.tv.drain(lo, hi);
        if ts.is_empty() {
            return None;
        }
        let mut parts = Vec::new();
        for t in ts {
            if t.blank {
                parts.push(Doc::Blank);
            }
            let d = self.trivia_doc(std::slice::from_ref(&t));
            parts.push(d);
            parts.push(Doc::HardLine);
        }
        parts.pop();
        Some(cat(parts))
    }

    /// An import's own comments, without the blank line `decl_trivia` would put
    /// back — inside a sorted run the blanks belong to the grouping, not to
    /// where the line used to sit.
    fn import_trivia(&mut self, at: u32, first: bool) -> Doc {
        let Some(t) = self.tv.take(at) else { return Doc::Nil };
        if t.is_empty() {
            return Doc::Nil;
        }
        let mut parts = Vec::new();
        if !first {
            parts.push(Doc::Blank);
        }
        parts.push(self.trivia_doc(std::slice::from_ref(&t)));
        parts.push(Doc::HardLine);
        cat(parts)
    }

    /// Comments and blank lines above a declaration come back where they were.
    fn decl_trivia(&mut self, at: u32, first: bool) -> Doc {
        let Some(t) = self.tv.take(at) else {
            return if first { Doc::Nil } else { Doc::Blank };
        };
        let mut parts = Vec::new();
        if (t.blank || !t.is_empty()) && !first {
            parts.push(Doc::Blank);
        }
        if !t.is_empty() {
            parts.push(self.trivia_doc(std::slice::from_ref(&t)));
            parts.push(Doc::HardLine);
        }
        cat(parts)
    }

    // -- the module --------------------------------------------------------

    fn module(&mut self, m: &Module) -> Doc {
        let mut parts: Vec<Doc> = Vec::new();
        // The module's own documentation comes back first, and is separated
        // from the first declaration by a blank line.
        for line in &m.docs {
            parts.push(text(if line.is_empty() {
                "//!".to_string()
            } else {
                format!("//! {line}")
            }));
            parts.push(Doc::HardLine);
        }
        if !m.docs.is_empty() {
            parts.push(Doc::Blank);
        }
        // The leading import run is sorted rather than preserved. Import order
        // carries no meaning — a module's imports are a set — so leaving it to
        // the author makes every diff that adds one a choice somebody has to
        // make and somebody else has to review. Sorting here rather than
        // reporting it as a lint is the same argument the rest of this file
        // makes: a canonical layout is not a finding.
        let run = m.items.iter().take_while(|i| matches!(i, Item::Import(_))).count();

        // A comment block at the very top of the file, with a blank line
        // between it and the first import, is about the file. It stays at the
        // top: sorting the run would otherwise carry the header down to
        // wherever the import it happened to sit above ended up.
        let mut header = false;
        if run > 0 {
            let at = m.items[0].span().start;
            if self.tv.is_detached(at) {
                if let Some(t) = self.tv.take(at) {
                    let d = self.trivia_doc(std::slice::from_ref(&t));
                    parts.push(d);
                    parts.push(Doc::HardLine);
                    header = true;
                }
            }
        }

        let mut head: Vec<&Item> = m.items[..run].iter().collect();
        head.sort_by_key(|i| import_key(i));

        for (i, item) in head.iter().enumerate() {
            let first = i == 0 && m.docs.is_empty() && !header;
            // The run is one block. The standard library still sorts before
            // the repository, because that order says something — what every
            // module may assume, and then what this one is — but a blank line
            // between the two says it a second time, in whitespace.
            let t = self.import_trivia(item.span().start, first);
            parts.push(t);
            let d = self.item(item);
            parts.push(d);
            parts.push(Doc::HardLine);
        }

        // A `derive` for a type declared in this file is moved to sit on it.
        // Where it was written carries no meaning — `register_derive` runs in
        // a pass of its own, after every type in every module is known — and
        // where it *reads* is against the declaration it is about, the way an
        // attribute would.
        let rest: Vec<&Item> = m.items[run..].iter().collect();
        for (i, group) in derive_groups(&rest).into_iter().enumerate() {
            // The derives lead and the declaration's own documentation stays
            // directly on the declaration. It cannot be the other way round: a
            // `///` run attaches to whatever token follows it, so documentation
            // printed above a `derive` is documentation *of* the derive the
            // next time the file is read, and formatting would not be a fixed
            // point.
            let (decl, derives) = group.split_last().expect("a group holds its declaration");
            let first = i == 0 && m.docs.is_empty() && run == 0;
            if derives.is_empty() {
                let t = self.decl_trivia(decl.span().start, first);
                parts.push(t);
            } else {
                if !first {
                    parts.push(Doc::Blank);
                }
                for d in derives {
                    // Nothing between a derive and the declaration it is
                    // about, not even the paragraph break it was typed with.
                    let t = self.import_trivia(d.span().start, true);
                    parts.push(t);
                    let doc = self.item(d);
                    parts.push(doc);
                    parts.push(Doc::HardLine);
                }
                let t = self.import_trivia(decl.span().start, true);
                parts.push(t);
            }
            let d = self.item(decl);
            parts.push(d);
            parts.push(Doc::HardLine);
            // A comment written somewhere inside the declaration that no
            // printer reaches — between a parameter list and its body, say —
            // comes back under it rather than not at all.
            if let Some(c) = self.flush(decl.span().start, decl.span().end) {
                parts.push(c);
                parts.push(Doc::HardLine);
            }
        }

        // A comment below the last declaration is attached to end-of-file and
        // belongs to nothing; it is still a comment somebody wrote.
        if let Some(c) = self.flush(0, u32::MAX) {
            parts.push(c);
            parts.push(Doc::HardLine);
        }
        cat(parts)
    }

    // -- declarations ------------------------------------------------------

    fn item(&mut self, item: &Item) -> Doc {
        match item {
            Item::Import(i) => match &i.clause {
                // A namespace import is one name, so there is nothing a width
                // could do about it.
                ImportClause::Namespace(n) => {
                    text(format!("from \"{}\" import * as {};", i.path, n.name))
                }
                ImportClause::Named(specs) => {
                    self.name_list(&format!("from \"{}\" import", i.path), specs)
                }
            },
            Item::ReExport(r) => {
                self.name_list(&format!("from \"{}\" export", r.path), &r.specs)
            }
            Item::Fn(d) => self.fn_decl(d, d.exported),
            Item::Struct(d) => self.struct_decl(d),
            Item::Enum(d) => self.enum_decl(d),
            Item::TypeAlias(d) => {
                let ex = if d.exported { "export " } else { "" };
                let g = generics(&d.generics);
                text(format!("{ex}type {}{g} = {};", d.name.name, ty(&d.ty)))
            }
            Item::Const(d) => {
                let ex = if d.exported { "export " } else { "" };
                self.assign(
                    &format!("{ex}const {}: {} = ", d.name.name, ty(&d.ty)),
                    &d.value,
                    ";",
                )
            }
            Item::Trait(d) => {
                let ex = if d.exported { "export " } else { "" };
                let kw = if d.is_effect { "effect" } else { "trait" };
                let g = generics(&d.generics);
                let head = format!("{ex}{kw} {}{g}", d.name.name);
                if d.methods.is_empty() && !self.tv.any_in(d.span.start, d.span.end) {
                    return text(format!("{head} {{}}"));
                }
                let mut lines = Vec::new();
                for m in &d.methods {
                    // A trait method's own documentation is the trait's
                    // documentation of it, and is the reason `buri docs` has
                    // anything to say about a method that has no body.
                    if let Some(c) = self.flush(d.span.start, m.span.start) {
                        lines.push(c);
                    }
                    lines.push(self.signature_doc(m, "", ";"));
                }
                if let Some(c) = self.flush(d.span.start, d.span.end.saturating_sub(1)) {
                    lines.push(c);
                }
                braced(&format!("{head} {{"), join(Doc::HardLine, lines))
            }
            Item::Impl(d) => {
                let g = generics(&d.generics);
                let head = match &d.trait_ty {
                    Some(t) => format!("impl{g} {} for {}", ty(t), ty(&d.self_ty)),
                    None => format!("impl{g} {}", ty(&d.self_ty)),
                };
                if d.methods.is_empty() && !self.tv.any_in(d.span.start, d.span.end) {
                    return text(format!("{head} {{}}"));
                }
                let mut lines = Vec::new();
                for (i, m) in d.methods.iter().enumerate() {
                    if i > 0 {
                        lines.push(Doc::Blank);
                    }
                    if let Some(c) = self.flush(d.span.start, m.span.start) {
                        lines.push(c);
                    }
                    lines.push(self.fn_decl(m, m.exported));
                }
                if let Some(c) = self.flush(d.span.start, d.span.end.saturating_sub(1)) {
                    lines.push(c);
                }
                braced(&format!("{head} {{"), join(Doc::HardLine, lines))
            }
            Item::Derive(d) => {
                let traits: Vec<Doc> = d.traits.iter().map(|t| text(ty(t))).collect();
                group(cat(vec![
                    text("derive"),
                    nest(cat(vec![
                        Doc::Line,
                        filled(traits),
                        if_break(text(","), Doc::Nil),
                    ])),
                    Doc::Line,
                    text(format!("for {};", ty(&d.self_ty))),
                ]))
            }
            Item::Context(d) => {
                let ex = if d.exported { "export " } else { "" };
                let body = self.context_body(&d.body);
                braced(&format!("{ex}context {} {{", d.name.name), body)
            }
            Item::Test(d) => {
                let body = self.block_lines(&d.body);
                braced(&format!("test {} {{", quote(&d.name)), body)
            }
        }
    }

    /// `<head> { a, b, c };`, filled when it does not fit the width.
    ///
    /// The wrapped shape is the one the widest import in the conformance suite
    /// was written in by hand: a brace on the head line, the names filled
    /// across as many lines as they need, and a trailing comma so that adding
    /// a name is a one-line diff.
    fn name_list(&mut self, head: &str, specs: &[ImportSpec]) -> Doc {
        let names = spec_list(specs);
        if names.is_empty() {
            return text(format!("{head} {{  }};"));
        }
        group(cat(vec![
            text(format!("{head} {{")),
            nest(cat(vec![
                Doc::Line,
                filled(names.into_iter().map(text).collect()),
                if_break(text(","), Doc::Nil),
            ])),
            Doc::Line,
            text("};"),
        ]))
    }

    fn context_body(&mut self, body: &ContextBody) -> Doc {
        let lo = body.span.start;
        let mut lines = Vec::new();
        if let Some(s) = &body.spread {
            if let Some(c) = self.flush(lo, s.span().start) {
                lines.push(c);
            }
            lines.push(self.assign("..", s, ","));
        }
        // No alignment. A column of padding is a column that moves every time
        // the longest name in the block changes, so one binding renamed is a
        // diff on every line around it.
        for b in &body.bindings {
            if let Some(c) = self.flush(lo, b.span.start) {
                lines.push(c);
            }
            lines.push(self.assign(&format!("{}: ", ty(&b.effect)), &b.value, ","));
        }
        if let Some(c) = self.flush(lo, body.span.end.saturating_sub(1)) {
            lines.push(c);
        }
        join(Doc::HardLine, lines)
    }

    /// A signature, one parameter to a line when the whole of it will not fit —
    /// the shape the widest function in the conformance suite was written in by
    /// hand. `tail` is whatever follows the return type: `;` for a declaration
    /// without a body, ` {` for one with.
    fn signature_doc(&mut self, d: &FnDecl, lead: &str, tail: &str) -> Doc {
        // A comment written above a parameter is about that parameter, and a
        // broken signature has a line for it to sit on. Its `BreakParent` is
        // what breaks the signature: a comment cannot share a line with the
        // list it is annotating, so a flat signature is not on offer.
        let params: Vec<Doc> = d
            .params
            .iter()
            .map(|p| {
                let c = self.flush(d.span.start, p.span.start);
                with_comment(c, text(format!("{}: {}", p.name.name, ty(&p.ty))))
            })
            .collect();
        let close = format!("): {}{tail}", ty(&d.ret));
        if params.is_empty() {
            return text(format!("{lead}fn {}{}({close}", d.name.name, generics(&d.generics)));
        }
        group(cat(vec![
            text(format!("{lead}fn {}{}(", d.name.name, generics(&d.generics))),
            nest(cat(vec![
                Doc::SoftLine,
                join(cat(vec![text(","), Doc::Line]), params),
                if_break(text(","), Doc::Nil),
            ])),
            Doc::SoftLine,
            text(close),
        ]))
    }

    /// A function, whose body is always on lines of its own.
    ///
    /// `fn f(): Int { 1 }` is a shape a body grows out of the first time
    /// somebody adds a line to it, and the diff that does it then touches
    /// three lines to add one. A body that is *empty* has no line to put
    /// anywhere, so it closes up: `fn f(): () {}`.
    fn fn_decl(&mut self, d: &FnDecl, exported: bool) -> Doc {
        // A comment inside the signature — above a parameter, say — has no
        // line of its own once the signature is re-printed on one, so it comes
        // back above the declaration.
        // Built before the sweep, not after: the signature claims the comments
        // written among its parameters, and only what is left over — a comment
        // above the return type, say, which has no line of its own — comes
        // back above the declaration.
        let sig_end = d.body.as_ref().map(|b| b.span.start).unwrap_or(d.span.end);
        let ex = if exported { "export " } else { "" };
        let decl = match &d.body {
            None => self.signature_doc(d, ex, ";"),
            Some(b) => {
                // The block is read first only to learn whether it has
                // anything in it; the signature is built once, either way,
                // because building it twice would claim its comments once.
                let inner = self.block_lines(b);
                if matches!(inner, Doc::Concat(ref v) if v.is_empty()) {
                    self.signature_doc(d, ex, " {}")
                } else {
                    cat(vec![
                        self.signature_doc(d, ex, " {"),
                        nest(cat(vec![Doc::HardLine, inner])),
                        Doc::HardLine,
                        text("}"),
                    ])
                }
            }
        };
        let above = self.flush(d.span.start, sig_end.saturating_sub(1));
        with_above(above, decl)
    }

    fn struct_decl(&mut self, d: &StructDecl) -> Doc {
        let ex = if d.exported { "export " } else { "" };
        let g = generics(&d.generics);
        match &d.body {
            StructBody::Tuple(fields) => {
                let inner = fields
                    .iter()
                    .map(|f| {
                        format!("{}{}", if f.exported { "export " } else { "" }, ty(&f.ty))
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                text(format!("{ex}struct {}{g}({inner});", d.name.name))
            }
            StructBody::Record(fields) => {
                let head = format!("{ex}struct {}{g}", d.name.name);
                if fields.is_empty() && !self.tv.any_in(d.span.start + 1, d.span.end) {
                    return text(format!("{head} {{}}"));
                }
                let mut items = Vec::new();
                for f in fields {
                    // A field's documentation is trivia like any other, and
                    // comes back through the same path as a comment above it.
                    let c = self.flush(d.span.start + 1, f.span.start);
                    items.push(with_comment(c, text(field_decl(f))));
                }
                let trailing = self.flush(d.span.start + 1, d.span.end.saturating_sub(1));
                record(&head, items, trailing)
            }
        }
    }

    fn enum_decl(&mut self, d: &EnumDecl) -> Doc {
        let ex = if d.exported { "export " } else { "" };
        let g = generics(&d.generics);
        let head = format!("{ex}enum {}{g}", d.name.name);
        if d.variants.is_empty() && !self.tv.any_in(d.span.start + 1, d.span.end) {
            return text(format!("{head} {{}}"));
        }
        let mut items = Vec::new();
        for v in &d.variants {
            let c = self.flush(d.span.start + 1, v.span.start);
            items.push(with_comment(c, text(variant(v))));
        }
        let trailing = self.flush(d.span.start + 1, d.span.end.saturating_sub(1));
        record(&head, items, trailing)
    }

    // -- statements --------------------------------------------------------

    /// The lines of a block: its statements, its tail, and every comment
    /// written among them. Joined with `HardLine`, and with no break of its
    /// own at either end, so the block that holds it decides its braces.
    fn block_lines(&mut self, b: &Block) -> Doc {
        let lo = b.span.start;
        let mut lines: Vec<Doc> = Vec::new();
        for s in &b.stmts {
            // A blank line between two statements is a paragraph break inside
            // a function, and grouping the steps of a long one is what it is
            // for. One of them, however many were typed.
            let gap = self.tv.blank_at(s.span().start);
            // Everything written above this statement, including anything
            // stranded inside the statement before it.
            match self.flush(lo, s.span().start) {
                Some(c) => lines.push(c),
                None if gap => lines.push(Doc::Blank),
                None => {}
            }
            lines.push(match s {
                Stmt::Let { pattern, ty: t, value, is_ctx, .. } => {
                    let name = if *is_ctx { "ctx".to_string() } else { pattern_str(pattern) };
                    let ann = t.as_ref().map(|x| format!(": {}", ty(x))).unwrap_or_default();
                    self.assign(&format!("let {name}{ann} = "), value, ";")
                }
                Stmt::Expr { expr: e, .. } => self.assign("", e, ";"),
            });
        }
        if let Some(t) = &b.tail {
            let gap = self.tv.blank_at(t.span().start);
            match self.flush(lo, t.span().start) {
                Some(c) => lines.push(c),
                None if gap => lines.push(Doc::Blank),
                None => {}
            }
            lines.push(self.expr(t));
        }
        // The last thing in a block may be a comment, written above the `}`.
        if let Some(c) = self.flush(lo, b.span.end.saturating_sub(1)) {
            lines.push(c);
        }
        join(Doc::HardLine, lines)
    }

    /// `<prefix><expression><suffix>` on one line, with the expression wrapped
    /// however it needs to be.
    ///
    /// An expression with nothing inside it that could break is better on a
    /// line of its own than over the margin, and the line below has two more
    /// columns of room. That is a `group` around the space before it, and it is
    /// the only place the printer looks at the *shape* of an expression rather
    /// than at its width.
    fn assign(&mut self, prefix: &str, e: &Expr, suffix: &str) -> Doc {
        let d = self.expr(e);
        match prefix.strip_suffix(' ') {
            Some(head) if !breakable(e) => group(cat(vec![
                text(head.to_string()),
                nest(cat(vec![Doc::Line, d])),
                text(suffix.to_string()),
            ])),
            _ => cat(vec![text(prefix.to_string()), d, text(suffix.to_string())]),
        }
    }

    // -- expressions -------------------------------------------------------

    fn expr(&mut self, e: &Expr) -> Doc {
        match e {
            Expr::Int { raw, .. } | Expr::Float { raw, .. } => text(raw.clone()),
            Expr::Str { value, .. } => text(quote(value)),
            Expr::Char { value, .. } => text(quote_char(*value)),
            Expr::Bool { value, .. } => text(value.to_string()),
            Expr::Unit { .. } => text("()"),
            Expr::Ident { name, .. } => text(name.clone()),
            Expr::SelfValue { .. } => text("self"),
            Expr::Ctx { .. } => text("ctx"),
            Expr::DotVariant { name, .. } => text(format!(".{}", name.name)),
            Expr::Template { parts, .. } => {
                let mut v = vec![text("\"")];
                for p in parts {
                    match p {
                        TemplatePart::Text(t) => v.push(text(template_text(t))),
                        TemplatePart::Hole(h) => {
                            v.push(text("${"));
                            let d = self.expr(h);
                            v.push(d);
                            v.push(text("}"));
                        }
                    }
                }
                v.push(text("\""));
                cat(v)
            }
            Expr::Array { elems, .. } => {
                if elems.is_empty() {
                    return text("[]");
                }
                // One line, or one element to a line. A filled array packs
                // more onto the page and makes every diff that adds an element
                // reflow the ones after it; a list that reads down does not.
                // The clause of an import is the exception, and it is one
                // because a name there is not an element of anything.
                let items: Vec<Doc> = elems.iter().map(|x| self.expr(x)).collect();
                bracketed("[", items, "]")
            }
            Expr::Tuple { elems, .. } => {
                let items: Vec<Doc> = elems.iter().map(|x| self.expr(x)).collect();
                bracketed("(", items, ")")
            }
            Expr::Block(b) => {
                let inner = self.block_lines(b);
                block_doc(inner)
            }
            Expr::If { .. } => self.if_chain(e),
            Expr::Match { scrutinee, arms, span } => self.match_expr(scrutinee, arms, *span),
            Expr::ContextExpr { body, .. } => {
                let body = self.context_body(body);
                braced("context {", body)
            }
            Expr::Lambda { params, ret, body, .. } => {
                // Braces, unless the body fits beside the parameter list. A
                // body hanging under a bare `=>` reads as a continuation of
                // nothing; the braced form is the block body the grammar
                // already has (`fn(x) => { … }`), so this costs the parser
                // nothing and the reader a delimiter they can find.
                let head = lambda_head(params, ret);
                let d = self.expr(body);
                if matches!(&**body, Expr::Block(_)) {
                    return cat(vec![text(format!("{head} ")), d]);
                }
                Doc::Alt(vec![
                    cat(vec![text(format!("{head} ")), d.clone()]),
                    cat(vec![
                        text(format!("{head} {{")),
                        nest(cat(vec![Doc::HardLine, d])),
                        Doc::HardLine,
                        text("}"),
                    ]),
                ])
            }
            Expr::Unary { op, operand, .. } => {
                let d = self.at(operand, 10);
                cat(vec![text(op.text()), d])
            }
            Expr::Binary { op, .. } => {
                // The whole run of one precedence breaks together: a chain of
                // `&&` reads as a list of conditions, and a list reads down.
                let p = binop_prec(*op);
                let mut parts = Vec::new();
                spine(e, p, &mut parts);
                let first = self.at(parts[0].0, p);
                let mut rest = Vec::new();
                for (operand, op) in &parts[1..] {
                    let d = self.at(operand, p + 1);
                    rest.push(cat(vec![
                        Doc::Line,
                        text(format!("{} ", op.expect("only the head has none").text())),
                        d,
                    ]));
                }
                group(cat(vec![first, nest(cat(rest))]))
            }
            Expr::Field { .. }
            | Expr::Call { .. }
            | Expr::Index { .. }
            | Expr::Try { .. }
            | Expr::TurboFish { .. } => self.chain_expr(e),
            Expr::TupleIndex { base, index, .. } => {
                // `t.0.1` lexes as `t` `.` `0.1`, so a nested tuple index keeps
                // its parentheses: `(t.0).1`. A known lexical wart, accepted
                // because it is what lets `pair.0` lex at all (grammar.ebnf).
                let b = if matches!(&**base, Expr::TupleIndex { .. }) {
                    let d = self.expr(base);
                    cat(vec![text("("), d, text(")")])
                } else {
                    self.operand(base)
                };
                cat(vec![b, text(format!(".{index}"))])
            }
            Expr::StructLit { head, spread, fields, .. } => {
                let h = self.operand(head);
                if spread.is_none() && fields.is_empty() {
                    return cat(vec![h, text(" { }")]);
                }
                let mut items = Vec::new();
                if let Some(s) = spread {
                    let d = self.expr(s);
                    items.push(cat(vec![text(".."), d]));
                }
                for f in fields {
                    items.push(match &f.value {
                        Some(v) => {
                            let d = self.expr(v);
                            cat(vec![text(format!("{}: ", f.name.name)), d])
                        }
                        None => text(f.name.name.clone()),
                    });
                }
                group(cat(vec![
                    h,
                    text(" {"),
                    nest(cat(vec![
                        Doc::Line,
                        join(cat(vec![text(","), Doc::Line]), items),
                        if_break(text(","), Doc::Nil),
                    ])),
                    Doc::Line,
                    text("}"),
                ]))
            }
        }
    }

    /// Parenthesizes only where precedence requires it.
    fn at(&mut self, e: &Expr, parent: u8) -> Doc {
        let d = self.expr(e);
        if expr_prec(e) < parent {
            cat(vec![text("("), d, text(")")])
        } else {
            d
        }
    }

    /// The head of a postfix chain.
    fn operand(&mut self, e: &Expr) -> Doc {
        let d = self.expr(e);
        if needs_parens(e) {
            cat(vec![text("("), d, text(")")])
        } else {
            d
        }
    }

    /// A whole `if` / `else if` / `else` chain, in one group, so that the
    /// moment any of it breaks all of it does. Keeping `{ a }` beside the
    /// condition and breaking only the tail gives a shape that depends on
    /// which branch happened to be longest.
    fn if_chain(&mut self, e: &Expr) -> Doc {
        let mut v = Vec::new();
        let mut node = e;
        let mut lead = "if (";
        loop {
            match node {
                Expr::If { cond, then, else_, .. } => {
                    let c = self.expr(cond);
                    // The condition is its own group: a wide `if` need not mean
                    // a condition that did not fit.
                    v.push(group(cat(vec![
                        text(lead),
                        nest(cat(vec![Doc::SoftLine, c])),
                        Doc::SoftLine,
                        text(") {"),
                    ])));
                    let body = self.block_lines(then);
                    v.push(nest(cat(vec![Doc::Line, body])));
                    v.push(Doc::Line);
                    lead = "} else if (";
                    node = else_;
                }
                Expr::Block(b) => {
                    let body = self.block_lines(b);
                    v.push(text("} else {"));
                    v.push(nest(cat(vec![Doc::Line, body])));
                    v.push(Doc::Line);
                    v.push(text("}"));
                    break;
                }
                other => {
                    let d = self.expr(other);
                    v.push(text("} else "));
                    v.push(d);
                    break;
                }
            }
        }
        group(cat(v))
    }

    fn match_expr(&mut self, scrutinee: &Expr, arms: &[MatchArm], span: Span) -> Doc {
        let s = self.expr(scrutinee);
        let head = group(cat(vec![
            text("match ("),
            nest(cat(vec![Doc::SoftLine, s])),
            Doc::SoftLine,
            text(") {"),
        ]));
        let mut lines = Vec::new();
        for a in arms {
            // A comment above an arm is about that arm, and a match is often
            // where the reasoning in a function lives.
            if let Some(c) = self.flush(span.start, a.span.start) {
                lines.push(c);
            }
            let mut v = vec![text(pattern_str(&a.pattern))];
            if let Some(g) = &a.guard {
                let d = self.expr(g);
                v.push(text(" if "));
                v.push(d);
            }
            // Braces, unless the body fits beside the arrow — the same rule
            // a lambda body gets, and for the same reason. An arm body is an
            // expression, so the braced form is a block expression, which is
            // what the arm would have held anyway.
            let body = self.expr(&a.body);
            if matches!(&a.body, Expr::Block(_)) {
                v.push(text(" => "));
                v.push(body);
            } else {
                v.push(Doc::Alt(vec![
                    cat(vec![text(" => "), body.clone()]),
                    cat(vec![
                        text(" => {"),
                        nest(cat(vec![Doc::HardLine, body])),
                        Doc::HardLine,
                        text("}"),
                    ]),
                ]));
            }
            v.push(text(","));
            lines.push(cat(v));
        }
        if let Some(c) = self.flush(span.start, span.end.saturating_sub(1)) {
            lines.push(c);
        }
        cat(vec![
            head,
            nest(cat(vec![Doc::HardLine, join(Doc::HardLine, lines)])),
            Doc::HardLine,
            text("}"),
        ])
    }

    /// A postfix chain: `base`, then `.name`, `(args)`, `[i]`, `?` and `::<T>`
    /// in the order they were written.
    ///
    /// Three candidate layouts, in order of preference, as one `Alt`:
    ///
    /// 1. all of it on one line;
    /// 2. **at the dots**, when there are two or more of them — and then at
    ///    *every* one of them, the first included. A chain is a pipeline, and
    ///    a pipeline whose first stage is on the line above the rest reads as
    ///    though that stage were something else. Two or more, because one dot
    ///    is not a chain, it is a call;
    /// 3. the same document laid out normally: the last argument of a call
    ///    **hugs** when it is a lambda, a block or a literal, so that the
    ///    call keeps its head on one line and that argument spills; failing
    ///    that each argument list breaks on its own.
    ///
    /// A dot is a dot: a field access breaks like a method call, and a
    /// turbofish between the name and the call does not stop `.parse::<T>()`
    /// being one. What makes a link a break point is that it starts with `.`,
    /// which is also what makes it readable at the start of a line.
    fn chain_expr(&mut self, e: &Expr) -> Doc {
        let mut links = Vec::new();
        let base = chain(e, &mut links);
        let base_doc = self.operand(base);

        // Each link is built once and cloned into every candidate: building it
        // twice would claim its comments twice, and the second copy would get
        // none of them.
        let mut assembled: Vec<Doc> = Vec::new();
        for l in &links {
            match l {
                Link::Field(name) => assembled.push(text(format!(".{name}"))),
                Link::Call(xs) => {
                    let ds: Vec<Doc> = xs.iter().map(|x| self.expr(x)).collect();
                    assembled.push(args_doc(ds, xs.last().is_some_and(huggable)));
                }
                Link::Index(x) => {
                    let d = self.expr(x);
                    assembled.push(cat(vec![text("["), d, text("]")]));
                }
                Link::Try => assembled.push(text("?")),
                Link::Turbo(ts) => {
                    let inner = ts.iter().map(ty).collect::<Vec<_>>().join(", ");
                    assembled.push(text(format!("::<{inner}>")));
                }
            }
        }

        let natural = {
            let mut v = vec![base_doc.clone()];
            v.extend(assembled.iter().cloned());
            cat(v)
        };

        let dots = links.iter().filter(|l| matches!(l, Link::Field(_))).count();
        if dots < 2 {
            // Not a chain: one call, whose arguments answer for themselves.
            return group(natural);
        }
        // Every link on its own line, the first included: all of it or none of
        // it. What a link does *within* its line is still the link's own
        // business, so a call in a chain hugs its trailing lambda exactly as
        // the same call would anywhere else.
        let mut tail = Vec::new();
        for (i, d) in assembled.iter().enumerate() {
            if matches!(links[i], Link::Field(_)) {
                tail.push(Doc::HardLine);
            }
            tail.push(d.clone());
        }
        Doc::Alt(vec![
            natural.clone(),
            cat(vec![base_doc, nest(cat(tail))]),
            natural,
        ])
    }
}

/// An argument list. Three candidates when the last argument has a shape of
/// its own to spill into — all on one line, **hugging** so that the head of the
/// call stays on its line while that argument breaks, or one argument to a
/// line — and the plain bracketed form otherwise.
fn args_doc(ds: Vec<Doc>, hug: bool) -> Doc {
    let plain = bracketed("(", ds.clone(), ")");
    // The earlier arguments have to fit on the head line for a hug to mean
    // anything, so one that breaks rules it out.
    if !hug || ds[..ds.len() - 1].iter().any(breaks) {
        return plain;
    }
    let mut v = vec![text("(")];
    for d in &ds[..ds.len() - 1] {
        v.push(d.clone());
        v.push(text(", "));
    }
    v.push(force(ds[ds.len() - 1].clone()));
    v.push(text(")"));
    Doc::Alt(vec![plain.clone(), cat(v), plain])
}

/// `<head>` and a body on lines of its own, always — the shape every
/// declaration with members is printed in.
fn braced(head: &str, body: Doc) -> Doc {
    cat(vec![
        text(head.to_string()),
        nest(cat(vec![Doc::HardLine, body])),
        Doc::HardLine,
        text("}"),
    ])
}

/// `<head> { a, b }`, or one member to a line with a trailing comma.
fn record(head: &str, items: Vec<Doc>, trailing: Option<Doc>) -> Doc {
    // A body of nothing but a comment has no list to put a trailing comma on.
    if items.is_empty() {
        return braced(&format!("{head} {{"), trailing.unwrap_or(Doc::Nil));
    }
    let mut inner = vec![
        Doc::Line,
        join(cat(vec![text(","), Doc::Line]), items),
        if_break(text(","), Doc::Nil),
    ];
    if let Some(c) = trailing {
        inner.push(Doc::HardLine);
        inner.push(c);
    }
    group(cat(vec![
        text(format!("{head} {{")),
        nest(cat(inner)),
        Doc::Line,
        text("}"),
    ]))
}

/// The declarations, with each `derive` moved to sit directly on the type it
/// is about — its documentation above it, then the derives, then the type —
/// when that type is declared in this file. A `derive` for anything else is
/// left exactly where it was, because moving it would be a guess.
///
/// Each returned group is emitted with no blank line inside it and the usual
/// paragraph break above it.
fn derive_groups<'a>(items: &[&'a Item]) -> Vec<Vec<&'a Item>> {
    fn declares(item: &Item) -> Option<&str> {
        match item {
            Item::Struct(d) => Some(&d.name.name),
            Item::Enum(d) => Some(&d.name.name),
            Item::TypeAlias(d) => Some(&d.name.name),
            _ => None,
        }
    }
    // A `derive` may only name a type declared in the same module, so a
    // one-segment path is the whole of what can be matched here.
    fn derives_for(item: &Item) -> Option<&str> {
        let Item::Derive(d) = item else { return None };
        let TypeExpr::Named { path, args, .. } = &d.self_ty else { return None };
        (path.len() == 1 && args.is_empty()).then(|| path[0].name.as_str())
    }

    // Which declaration each `derive` belongs above, if any here does. It may
    // have been written above it or below it; either way it ends up on it.
    let attach: Vec<Option<usize>> = items
        .iter()
        .map(|item| {
            let name = derives_for(item)?;
            items.iter().position(|t| declares(t) == Some(name))
        })
        .collect();

    let mut out: Vec<Vec<&Item>> = Vec::new();
    for (i, item) in items.iter().enumerate() {
        if attach[i].is_some() {
            continue;
        }
        let mut group: Vec<&Item> = attach
            .iter()
            .enumerate()
            .filter(|(_, a)| **a == Some(i))
            .map(|(d, _)| items[d])
            .collect();
        group.push(item);
        out.push(group);
    }
    out
}

/// A declaration with whatever was stranded above it.
fn with_above(c: Option<Doc>, d: Doc) -> Doc {
    match c {
        Some(c) => cat(vec![c, Doc::HardLine, d]),
        None => d,
    }
}

/// A member with whatever was written above it.
fn with_comment(c: Option<Doc>, d: Doc) -> Doc {
    match c {
        Some(c) => cat(vec![c, Doc::HardLine, d]),
        None => d,
    }
}

/// `{ tail }`, or a block on lines of its own.
fn block_doc(inner: Doc) -> Doc {
    if matches!(inner, Doc::Concat(ref v) if v.is_empty()) {
        return cat(vec![text("{"), Doc::HardLine, text("}")]);
    }
    group(cat(vec![
        text("{"),
        nest(cat(vec![Doc::Line, inner])),
        Doc::Line,
        text("}"),
    ]))
}

// -- the shape of an expression --------------------------------------------

/// One postfix operator applied to whatever is to its left.
enum Link<'a> {
    Field(&'a str),
    Call(&'a [Expr]),
    Index(&'a Expr),
    Try,
    Turbo(&'a [TypeExpr]),
}

/// Splits a postfix chain into the thing it starts from and the operators
/// applied to it, in source order.
///
/// `TupleIndex` is not one of them: `(t.0).1` needs parentheses that a chain
/// printed left to right has no way to put back, so a tuple index ends the
/// chain and is printed as the base.
fn chain<'a>(e: &'a Expr, links: &mut Vec<Link<'a>>) -> &'a Expr {
    match e {
        Expr::Field { base, name, .. } => {
            let b = chain(base, links);
            links.push(Link::Field(&name.name));
            b
        }
        Expr::Call { callee, args, .. } => {
            let b = chain(callee, links);
            links.push(Link::Call(args));
            b
        }
        Expr::Index { base, index, .. } => {
            let b = chain(base, links);
            links.push(Link::Index(index));
            b
        }
        Expr::Try { base, .. } => {
            let b = chain(base, links);
            links.push(Link::Try);
            b
        }
        Expr::TurboFish { base, args, .. } => {
            let b = chain(base, links);
            links.push(Link::Turbo(args));
            b
        }
        _ => e,
    }
}


/// Whether a trailing argument has a shape of its own that a wide call can
/// spill into, rather than one that would have to be indented as a unit.
fn huggable(e: &Expr) -> bool {
    match e {
        Expr::Lambda { .. } | Expr::Array { .. } | Expr::StructLit { .. } => true,
        e => e.is_block_like(),
    }
}

/// Whether the broken form of an expression is narrower than its one-line
/// form — that is, whether there is anything inside it to break.
fn breakable(e: &Expr) -> bool {
    match e {
        Expr::Array { .. }
        | Expr::Tuple { .. }
        | Expr::Call { .. }
        | Expr::StructLit { .. }
        | Expr::Binary { .. }
        | Expr::Lambda { .. } => true,
        Expr::Field { .. } | Expr::Index { .. } | Expr::Try { .. } | Expr::TurboFish { .. } => {
            let mut links = Vec::new();
            let base = chain(e, &mut links);
            links.iter().filter(|l| matches!(l, Link::Field(_))).count() >= 2
                || links.iter().any(|l| matches!(l, Link::Call(a) if !a.is_empty()))
                || breakable(base)
        }
        Expr::TupleIndex { base, .. } | Expr::Unary { operand: base, .. } => breakable(base),
        e => e.is_block_like(),
    }
}


/// A block-like expression may not head a postfix chain (SPEC 12.13), so it
/// gets parentheses; so does anything that binds looser than a postfix
/// operator.
fn needs_parens(e: &Expr) -> bool {
    expr_prec(e) < 11 || (e.is_block_like() && !matches!(e, Expr::Block(_)))
}

/// The operands of one precedence level, left to right. The first carries no
/// operator; each of the rest carries the one written before it.
fn spine<'a>(e: &'a Expr, p: u8, out: &mut Vec<(&'a Expr, Option<BinOp>)>) {
    if let Expr::Binary { op, lhs, rhs, .. } = e {
        if binop_prec(*op) == p {
            spine(lhs, p, out);
            out.push((rhs, Some(*op)));
            return;
        }
    }
    out.push((e, None));
}

fn lambda_head(params: &[LambdaParam], ret: &Option<TypeExpr>) -> String {
    let mut out = String::from("fn(");
    for (i, p) in params.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        out.push_str(&p.name.name);
        if let Some(t) = &p.ty {
            let _ = write!(out, ": {}", ty(t));
        }
    }
    out.push(')');
    if let Some(r) = ret {
        let _ = write!(out, ": {}", ty(r));
    }
    out.push_str(" =>");
    out
}

fn template_text(t: &str) -> String {
    let mut out = String::new();
    for c in t.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '$' => out.push_str("\\$"),
            c => out.push(c),
        }
    }
    out
}

/// The precedence ladder of SPEC 6.1, lowest to highest. Parentheses are
/// printed only where they change the parse — the source's own are not in the
/// tree, so re-adding all of them would grow the file every time it is
/// formatted.
fn binop_prec(op: BinOp) -> u8 {
    match op {
        BinOp::Or => 1,
        BinOp::Coalesce => 2,
        BinOp::And => 3,
        BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => 4,
        BinOp::BitOr => 5,
        BinOp::BitXor => 6,
        BinOp::BitAnd => 7,
        BinOp::Add | BinOp::Sub => 8,
        BinOp::Mul | BinOp::Div | BinOp::Rem => 9,
    }
}

fn expr_prec(e: &Expr) -> u8 {
    match e {
        // A lambda's body extends maximally to the right, so it is never a
        // bare operand (SPEC 12.11).
        Expr::Lambda { .. } => 0,
        Expr::Binary { op, .. } => binop_prec(*op),
        Expr::Unary { .. } => 10,
        _ => 11,
    }
}

// -- the pieces that are always one line -----------------------------------

pub fn field_decl(f: &FieldDecl) -> String {
    format!(
        "{}{}: {}",
        if f.exported { "export " } else { "" },
        f.name.name,
        ty(&f.ty)
    )
}

pub fn variant(v: &Variant) -> String {
    let ex = if v.exported { "export " } else { "" };
    match &v.payload {
        VariantPayload::None => format!("{ex}{}", v.name.name),
        VariantPayload::Tuple(ts) => format!(
            "{ex}{}({})",
            v.name.name,
            ts.iter().map(ty).collect::<Vec<_>>().join(", ")
        ),
        VariantPayload::Record(fs) => format!(
            "{ex}{} {{ {} }}",
            v.name.name,
            fs.iter().map(field_decl).collect::<Vec<_>>().join(", ")
        ),
    }
}

pub fn signature(d: &FnDecl) -> String {
    let params = d
        .params
        .iter()
        .map(|p| format!("{}: {}", p.name.name, ty(&p.ty)))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "fn {}{}({params}): {}",
        d.name.name,
        generics(&d.generics),
        ty(&d.ret)
    )
}

pub fn generics(g: &[GenericParam]) -> String {
    if g.is_empty() {
        return String::new();
    }
    let inner = g
        .iter()
        .map(|p| {
            if p.bounds.is_empty() {
                p.name.name.clone()
            } else {
                format!(
                    "{}: {}",
                    p.name.name,
                    p.bounds.iter().map(ty).collect::<Vec<_>>().join(" + ")
                )
            }
        })
        .collect::<Vec<_>>()
        .join(", ");
    format!("<{inner}>")
}

pub fn ty(t: &TypeExpr) -> String {
    match t {
        TypeExpr::Named { path, args, .. } => {
            let base = path.iter().map(|p| p.name.clone()).collect::<Vec<_>>().join(".");
            if args.is_empty() {
                base
            } else {
                format!("{base}<{}>", args.iter().map(ty).collect::<Vec<_>>().join(", "))
            }
        }
        TypeExpr::SelfType { .. } => "Self".into(),
        TypeExpr::Unit { .. } => "()".into(),
        TypeExpr::Tuple { elems, .. } => {
            format!("({})", elems.iter().map(ty).collect::<Vec<_>>().join(", "))
        }
        TypeExpr::Array { elem, .. } => format!("[{}]", ty(elem)),
        TypeExpr::Fn { params, ret, .. } => format!(
            "fn({}) => {}",
            params.iter().map(ty).collect::<Vec<_>>().join(", "),
            ty(ret)
        ),
    }
}

fn quote_char(c: char) -> String {
    match c {
        '\'' => "'\\''".into(),
        '\\' => "'\\\\'".into(),
        '\n' => "'\\n'".into(),
        '\r' => "'\\r'".into(),
        '\t' => "'\\t'".into(),
        '\0' => "'\\0'".into(),
        c if (c as u32) < 0x20 => format!("'\\u{{{:x}}}'", c as u32),
        c => format!("'{c}'"),
    }
}

fn quote(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

pub fn pattern_str(p: &Pattern) -> String {
    match p {
        Pattern::Wild { .. } => "_".into(),
        Pattern::Bind { name, sub, .. } => match sub {
            Some(s) => format!("{} @ {}", name.name, pattern_str(s)),
            None => name.name.clone(),
        },
        Pattern::LitInt { raw, negative, .. } => {
            format!("{}{raw}", if *negative { "-" } else { "" })
        }
        Pattern::LitFloat { raw, negative, .. } => {
            format!("{}{raw}", if *negative { "-" } else { "" })
        }
        Pattern::LitStr { value, .. } => quote(value),
        Pattern::LitChar { value, .. } => quote_char(*value),
        Pattern::LitBool { value, .. } => value.to_string(),
        Pattern::Unit { .. } => "()".into(),
        Pattern::Tuple { elems, .. } => {
            format!("({})", elems.iter().map(pattern_str).collect::<Vec<_>>().join(", "))
        }
        Pattern::Array { elems, rest, .. } => {
            let mut parts: Vec<String> = elems.iter().map(pattern_str).collect();
            if let Some(r) = rest {
                parts.push(match r {
                    Some(n) => format!("..{}", n.name),
                    None => "..".into(),
                });
            }
            format!("[{}]", parts.join(", "))
        }
        Pattern::Or { alts, .. } => {
            alts.iter().map(pattern_str).collect::<Vec<_>>().join(" | ")
        }
        Pattern::Path { path, dotted, payload, .. } => {
            let base = if *dotted {
                format!(".{}", path[0].name)
            } else {
                path.iter().map(|p| p.name.clone()).collect::<Vec<_>>().join(".")
            };
            match payload {
                None => base,
                Some(PatPayload::Tuple(ps)) => format!(
                    "{base}({})",
                    ps.iter().map(pattern_str).collect::<Vec<_>>().join(", ")
                ),
                Some(PatPayload::Record { fields, rest }) => {
                    let mut parts: Vec<String> = fields
                        .iter()
                        .map(|f| match &f.pattern {
                            Some(p) => format!("{}: {}", f.name.name, pattern_str(p)),
                            None => f.name.name.clone(),
                        })
                        .collect();
                    if *rest {
                        parts.push("..".into());
                    }
                    format!("{base} {{ {} }}", parts.join(", "))
                }
            }
        }
    }
}

// -- what formatting must not change ---------------------------------------

/// One element of what a file is made of, as far as the formatter is
/// concerned.
///
/// A comment is a element in its own right rather than a property of the token
/// it precedes, because that is exactly the confusion that let the formatter
/// delete one: trivia keyed by a *declaration's* offset is trivia that has
/// nowhere to go when the offset is a statement inside a body.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Shape {
    Token(String),
    /// `//` and `/* */`, with each line trimmed — re-indenting a block comment
    /// is a layout change, not a loss.
    Comment(String),
    /// `///`
    Doc(String),
    /// `//!`
    ModuleDoc(String),
}

/// Reads the tokens *and the comments* of a file, so the formatter can tell
/// whether it changed anything meaningful.
pub fn token_shape(text: &str) -> Vec<Shape> {
    let lexed = lex(text, FileId(0));
    let mut out: Vec<Shape> = Vec::new();
    for (line, _) in &lexed.module_docs {
        out.push(Shape::ModuleDoc(line.trim().to_string()));
    }
    for t in &lexed.tokens {
        for c in &t.comments {
            out.push(Shape::Comment(trim_lines(&c.text)));
        }
        for d in &t.docs {
            out.push(Shape::Doc(d.trim().to_string()));
        }
        if !matches!(t.tok, Tok::Eof) {
            out.push(Shape::Token(t.tok.to_string()));
        }
    }
    out
}

/// The comments alone, in source order.
///
/// This is the half of the shape formatting must preserve *exactly*. The token
/// half may legally change: the formatter drops a redundant parenthesis and an
/// optional trailing comma, and adds a required one.
pub fn comment_shape(text: &str) -> Vec<Shape> {
    token_shape(text).into_iter().filter(|s| !matches!(s, Shape::Token(_))).collect()
}

fn trim_lines(s: &str) -> String {
    s.lines().map(str::trim).collect::<Vec<_>>().join("\n")
}

/// The properties, rather than the shapes.
///
/// What the formatter *prints* is pinned by `cli/tests/formatting/`, a paired
/// `input.buri`/`expected.buri` for each decision it makes; a shape checked in
/// two places is a shape that has to be re-blessed in two places. What is left
/// here is the three claims no single case can make: that a comment survives
/// wherever it was written, that `source` refuses output it cannot vouch for,
/// and that a file with one of everything in it comes out inside the margin.
#[cfg(test)]
mod tests {
    use super::*;

    /// Every line of `out` is inside the margin.
    #[track_caller]
    fn within_width(out: &str) {
        for line in out.lines() {
            assert!(
                line.chars().count() <= WIDTH,
                "a line is {} columns:\n{line}\nin:\n{out}",
                line.chars().count()
            );
        }
    }

    /// Formatting is a fixed point and keeps every comment.
    #[track_caller]
    fn stable(src: &str) -> String {
        let out = source_unchecked(src);
        assert_eq!(source_unchecked(&out), out, "not a fixed point:\n{out}");
        let (mut want, mut got) = (comment_shape(src), comment_shape(&out));
        want.sort();
        got.sort();
        assert_eq!(want, got, "a comment was lost formatting:\n{src}\ninto:\n{out}");
        assert!(
            crate::parsing::parser::parse(&out, FileId(0)).errors.is_empty(),
            "the output does not parse:\n{out}"
        );
        out
    }










    /// Every other place a comment can be written, held to the one property
    /// that matters: none of them is a place it can be deleted.
    #[test]
    fn no_comment_is_ever_dropped() {
        for src in [
            "// above a declaration\nexport fn f(): Int { 1 }\n",
            "export fn f(): Int { 1 }\n// below the last declaration\n",
            "export struct S {\n  // about a field\n  export a: Int,\n  // and the last one\n}\n",
            "export enum E {\n  // about a variant\n  A,\n  // and the last one\n}\n",
            "export trait T {\n  /// what this does\n  fn f(self: Self): Int;\n}\n",
            "impl S {\n  /// what this does\n  fn f(self: Self): Int { 1 }\n}\n",
            "export context C {\n  // why this one\n  Clock: Fixed(),\n}\n",
            "test \"a test\" {\n  // about an assertion\n  assert.eq(1, 1);\n}\n",
            "export fn f(): Int {\n  let x = {\n    // inside a nested block\n    1\n  };\n  x\n}\n",
            "export fn f(): Int {\n  let x = if (true) {\n    // inside a branch\n    1\n  } else { 2 };\n  x\n}\n",
            "export fn f(): Int {\n  /* a block comment */\n  1\n}\n",
            "//! the module\n\n// and a declaration\nexport fn f(): Int { 1 }\n",
        ] {
            stable(src);
        }
    }

    /// `source` refuses output it cannot vouch for, and a lost comment is
    /// something it cannot vouch for. This is the last line of defence: a
    /// construct nobody thought of leaves the file alone rather than damaging
    /// it.
    #[test]
    fn a_file_whose_comments_would_be_lost_is_left_alone() {
        let src = "export fn f(): Int {\n  // kept\n  1\n}\n";
        assert!(source(src).is_some());
        assert!(comment_shape(src).len() == 1);
        assert!(comment_shape(&source(src).unwrap()) == comment_shape(src));
    }




















    /// The whole grammar at once: every construct, wide enough to have to
    /// wrap, with a comment in every place one can be written. This is the
    /// evidence that the corpus can be reformatted — under the width, a fixed
    /// point, and not one comment or paragraph break lost.
    #[test]
    fn a_file_of_every_construct_wraps_inside_the_width() {
        let out = stable(EVERY_CONSTRUCT);
        within_width(&out);
        // The three breaks the lexer used to forget: between two comment
        // paragraphs above one declaration, between a heading and the doc
        // comment under it, and between two fields of a struct.
        assert!(
            out.contains("---\n\n// A section heading, and then a sentence"),
            "the blank under a section heading was lost:\n{out}"
        );
        assert!(
            out.contains("---\n\n/// A signature with more parameters"),
            "the blank between a heading and a doc comment was lost:\n{out}"
        );
        assert!(
            out.contains("export b: U32,\n\n    // And the last one"),
            "the blank inside a struct body was lost:\n{out}"
        );
        assert!(
            out.contains(".None => 0,\n\n        // And a paragraph break"),
            "the blank inside a match was lost:\n{out}"
        );
        // Every wrapped shape this file exists to pin down.
        for want in [
            // one argument to a line, with a trailing comma
            "    assert.eq(\n        showAll(ctx, mark, value, fallback),\n",
            // every link of a chain on its own line, the first included, and
            // a trailing lambda hugging inside the link it belongs to
            "    let hexed = b\n        .mapCtx(ctx, fn(c, x) => {\n",
            "        })\n        .join(ctx, \"\");\n",
            "    let chained = list\n        .range(ctx, 0, 40)\n        .mapEveryOne(ctx)\n",
            // an operator run breaks together, operator first
            "        let heavy = self.a > 0\n            && self.b > 0\n",
            // a signature is a list of parameters
            "export fn noteResult<T, E, C: Alloc + Stdout>(\n    ctx: C,\n",
            // a list of anything goes one item to a line
            "const SEED: [U32] = [\n    1779033703,\n    3144134277,",
            "    let structured = Working {\n        a: t.0.wrapToU32(),\n",
            // once an `if` breaks, every branch is on lines of its own
            "    } else if (structured.a > 0) {\n        \"one\"\n    } else {\n",
        ] {
            assert!(out.contains(want), "missing:\n{want}\nin:\n{out}");
        }
    }

    /// One file with one of everything in it, written the way somebody types
    /// it — which is to say, over the margin nearly everywhere.
    ///
    /// It only has to *parse*: the formatter reads a tree and never asks what
    /// it means. What it is for is coverage of the printer, so every construct
    /// appears at least once at a width that forces it to wrap, and a comment
    /// appears in every position one can be written in.
    const EVERY_CONSTRUCT: &str = r##"//! Every construct the formatter knows, written wide enough that each of them
//! has to wrap.

// What this file is for: it is the formatter's own corpus, and it is here so
// that the rules below are checked against something with one of everything in
// it rather than against twelve snippets.

from "//lib/semantics" import { identity, constant, swap, triple, twice, largest, smallest, between, showAll, sortedShow, Pair, pair, flip, Boxed, Slot, slotOr };
from "core/str" import * as str;
from "core/list" import * as list;

// ---- types ------------------------------------------------------------------

// A section heading, and then a sentence about the declaration under it.
export type Pairing<T> = (T, T);

/// A table of constants, one to a line like every other list.
const SEED: [U32] = [1779033703, 3144134277, 1013904242, 2773480762, 1359893119, 2600822924, 528734635, 1541459225, 1116352408, 1899447441, 3049323471, 3921009573];

export struct Working {
  // The first word of the state.
  export a: U32,
  export b: U32,

  // And the last one, after a paragraph break.
  export h: U32,
}

export struct Meters(export Float);

export enum Shape {
  Circle(Float),
  Rect { width: Float, height: Float },
  // A variant with nothing in it.
  Empty,
}

export trait Weigh {
  /// What one of these weighs, given somewhere to allocate and a scale to
  /// read.
  fn weighWithEverything<C: Alloc + Stdout>(self: Self, ctx: C, scale: Str, precision: Int): Float;
}

impl Weigh for Working {
  fn weighWithEverything<C: Alloc + Stdout>(self: Self, ctx: C, scale: Str, precision: Int): Float {
    // A chain of one operator breaks as a list.
    let heavy = self.a > 0 && self.b > 0 && self.h > 0 && precision > 0 && scale.len() > 0 && self.a != self.b;
    if (heavy) { 1.0 } else { 0.0 }
  }
}

derive Eq, Ord, Show for Meters;

export context Everything {
  Clock: Fixed(0),
  Alloc: System(),
}

// ---- functions --------------------------------------------------------------

/// A signature with more parameters than fit on one line.
export fn noteResult<T, E, C: Alloc + Stdout>(ctx: C, mark: Str, value: Result<T, E>, fallback: Result<T, E>): Result<T, E> {
  // A call with more arguments than fit.
  assert.eq(showAll(ctx, mark, value, fallback), "the shape of a wide call, one argument to a line");
  value
}

export fn everything<C: Alloc>(ctx: C, b: [U8], t: (Int, Int), o: Option<Int>): Str {
  // A trailing lambda hugs the call it is the last argument of.
  let hexed = b.mapCtx(ctx, fn(c, x) => {
    let n = x.toI64();
    str.fromChars(c, [HEX.charAt(n / 16).withDefault('0'), HEX.charAt(n % 16).withDefault('0')])
  }).join(ctx, "");
  // A chain of method calls breaks at the dots.
  let chained = list.range(ctx, 0, 40).mapEveryOne(ctx).filterTheRestOfThem(ctx).joinItAllUp(ctx, ", ");
  let structured = Working { a: t.0.wrapToU32(), b: t.1.wrapToU32(), h: b[0].withDefault(0).toI64().wrapToU32() };
  let templated = "first ${structured.a}, second ${structured.b}, and last ${structured.h}";
  let indexed = b[3];
  let turbo = list.empty::<U8>();
  let tried = parseIt(ctx, hexed)?;
  let nested = {
    // A block with a comment in it never collapses.
    let inner = 1;
    inner
  };
  let branch = if (structured.a > 0 && structured.b > 0) { "both" } else if (structured.a > 0) { "one" } else { "neither" };
  let matched = match (o) {
    // Nothing is nothing.
    .None => 0,

    // And a paragraph break above the rest.
    .Some(n) if n > 100 => someRatherLongFunctionName(ctx, n, "with a string long enough to wrap"),
    .Some(n) => n,
  };
  let ctxd = context {
    ..ctx,
    Clock: Fixed(matched),
  };
  str.format(ctx, "${hexed} ${chained} ${templated} ${indexed} ${turbo} ${tried}", nested, branch, matched, ctxd)
  // The last word in the block.
}

test "every construct, and the width they all have to fit inside" {
  // An assertion wide enough to wrap.
  assert.eq(everything(Hermetic(), [1, 2, 3], (4, 5), .Some(6)), "a string that is long enough that the call around it cannot stay on one line");
  assert.isTrue([1, 0, 3].foldResultCtx(Hermetic(), fn(c, acc: [Int], x) => if (x == 0) { .Err("zero") } else { .Ok(acc.push(c, 100 / x)) }, list.empty::<Int>()).isOk());
  // The last word in the test.
}

// A comment written below everything, which belongs to nothing.
"##;
}
