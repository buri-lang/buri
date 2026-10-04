//! The layout engine a formatter for any language hands its document to.
//!
//! A formatter builds a [`Doc`] and [`render`] lays it out at the margin and
//! indent every `.buri` file gets, so no formatter chooses its own width. The
//! variants are the ones `core/format` declares for a tool written in Buri, and
//! the algorithm is Wadler's *A prettier printer* as Prettier generalized it —
//! the same one `crate::formatting` runs over its own, richer document.
//!
//! ```text
//! Group([Text("["), Indent([SoftLine, Text("1,"), Line, Text("2")]), SoftLine, Text("]")])
//!   fits:        [1, 2]
//!   does not:    [
//!                    1,
//!                    2
//!                ]
//! ```

use std::collections::HashSet;

/// The margin, shared with `.buri` sources.
pub const WIDTH: usize = crate::formatting::WIDTH;

/// One level of indentation, shared with `.buri` sources.
pub const INDENT: usize = crate::formatting::INDENT;

#[derive(Clone, Debug, PartialEq)]
pub enum Doc {
    /// Printed as written. A newline inside it breaks every enclosing group.
    Text(String),
    Concat(Vec<Doc>),
    /// A space, or a break.
    Line,
    /// Nothing, or a break.
    SoftLine,
    /// Always a break.
    HardLine,
    /// Breaks all its lines, or none.
    Group(Vec<Doc>),
    /// Breaks inside it indent one level.
    Indent(Vec<Doc>),
    /// The first if the enclosing group breaks, the second if it does not.
    IfBreak(Box<Doc>, Box<Doc>),
    /// Printed at the end of the current line, before its break.
    LineSuffix(String),
    /// Forces every enclosing group to break.
    BreakParent,
}

impl Doc {
    pub fn text(s: impl Into<String>) -> Doc {
        Doc::Text(s.into())
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Flat,
    Break,
}

type Frame<'d> = (usize, Mode, &'d Doc);

/// Lays `doc` out at [`WIDTH`]. Trailing spaces never survive a line break.
pub fn render(doc: &Doc) -> String {
    let mut broken = HashSet::new();
    propagate(doc, &mut broken);
    let mut out = String::new();
    let mut pos = 0usize;
    let mut suffixes: Vec<&str> = Vec::new();
    let mut stack: Vec<Frame> = vec![(0, Mode::Break, doc)];
    while let Some((ind, mode, d)) = stack.pop() {
        match d {
            Doc::BreakParent => {}
            Doc::Text(s) => {
                out.push_str(s);
                pos = column_after(pos, s);
            }
            Doc::LineSuffix(s) => suffixes.push(s),
            Doc::Concat(xs) => push_all(&mut stack, ind, mode, xs),
            Doc::Indent(xs) => push_all(&mut stack, ind.saturating_add(1), mode, xs),
            Doc::Group(xs) => {
                let forced = broken.contains(&(d as *const Doc));
                let flat = !forced
                    && (mode == Mode::Flat
                        || fits(&stack, xs, ind, WIDTH.saturating_sub(pos), &broken));
                push_all(&mut stack, ind, if flat { Mode::Flat } else { Mode::Break }, xs);
            }
            Doc::IfBreak(b, f) => {
                stack.push((ind, mode, if mode == Mode::Break { b } else { f }));
            }
            Doc::Line | Doc::SoftLine if mode == Mode::Flat => {
                if matches!(d, Doc::Line) {
                    out.push(' ');
                    pos = pos.saturating_add(1);
                }
            }
            Doc::Line | Doc::SoftLine | Doc::HardLine => {
                if !suffixes.is_empty() {
                    // The suffixes go out first, then this break again.
                    stack.push((ind, mode, d));
                    for s in suffixes.drain(..) {
                        out.push_str(s);
                    }
                    continue;
                }
                while out.ends_with(' ') {
                    out.pop();
                }
                out.push('\n');
                let spaces = ind.saturating_mul(INDENT);
                out.extend(std::iter::repeat_n(' ', spaces));
                pos = spaces;
            }
        }
    }
    for s in suffixes {
        out.push_str(s);
    }
    out
}

fn push_all<'d>(stack: &mut Vec<Frame<'d>>, ind: usize, mode: Mode, xs: &'d [Doc]) {
    for x in xs.iter().rev() {
        stack.push((ind, mode, x));
    }
}

fn column_after(pos: usize, s: &str) -> usize {
    match s.rfind('\n') {
        Some(at) => s.len().saturating_sub(at).saturating_sub(1),
        None => pos.saturating_add(s.chars().count()),
    }
}

/// Records every group that must break, and answers whether `doc` forces the
/// groups around it to. An `IfBreak` forces nothing: both branches are
/// conditional on the answer being computed.
fn propagate(doc: &Doc, broken: &mut HashSet<*const Doc>) -> bool {
    match doc {
        Doc::HardLine | Doc::BreakParent => true,
        Doc::Text(s) => s.contains('\n'),
        Doc::Line | Doc::SoftLine | Doc::LineSuffix(_) | Doc::IfBreak(..) => false,
        Doc::Concat(xs) | Doc::Indent(xs) => propagate_all(xs, broken),
        Doc::Group(xs) => {
            let forced = propagate_all(xs, broken);
            if forced {
                broken.insert(doc as *const Doc);
            }
            forced
        }
    }
}

/// `propagate` over every one of `xs`, without stopping at the first that
/// forces a break: each still has its own groups to record.
fn propagate_all(xs: &[Doc], broken: &mut HashSet<*const Doc>) -> bool {
    let mut any = false;
    for x in xs {
        any |= propagate(x, broken);
    }
    any
}

/// Whether the group's contents, and what follows them up to the next break,
/// fit in `width` columns when the group is flat.
fn fits(
    rest: &[Frame<'_>],
    group: &[Doc],
    ind: usize,
    width: usize,
    broken: &HashSet<*const Doc>,
) -> bool {
    let mut left = width;
    let mut local: Vec<Frame> = Vec::new();
    push_all(&mut local, ind, Mode::Flat, group);
    let mut behind = rest.iter().rev();
    loop {
        let (ind, mode, d) = match local.pop() {
            Some(f) => f,
            None => match behind.next() {
                Some(f) => *f,
                None => return true,
            },
        };
        match d {
            Doc::BreakParent | Doc::LineSuffix(_) => {}
            Doc::Text(s) => {
                if s.contains('\n') {
                    return false;
                }
                match left.checked_sub(s.chars().count()) {
                    Some(l) => left = l,
                    None => return false,
                }
            }
            Doc::Concat(xs) | Doc::Indent(xs) => push_all(&mut local, ind, mode, xs),
            Doc::Group(xs) => {
                let m = if broken.contains(&(d as *const Doc)) { Mode::Break } else { mode };
                push_all(&mut local, ind, m, xs);
            }
            Doc::IfBreak(b, f) => local.push((ind, mode, if mode == Mode::Break { b } else { f })),
            Doc::Line => {
                if mode == Mode::Break {
                    return true;
                }
                match left.checked_sub(1) {
                    Some(l) => left = l,
                    None => return false,
                }
            }
            Doc::SoftLine => {
                if mode == Mode::Break {
                    return true;
                }
            }
            Doc::HardLine => return true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list(items: &[&str]) -> Doc {
        let mut inner = vec![Doc::SoftLine];
        for (i, item) in items.iter().enumerate() {
            if i > 0 {
                inner.push(Doc::text(","));
                inner.push(Doc::Line);
            }
            inner.push(Doc::text(*item));
        }
        inner.push(Doc::IfBreak(Box::new(Doc::text(",")), Box::new(Doc::text(""))));
        Doc::Group(vec![Doc::text("["), Doc::Indent(inner), Doc::SoftLine, Doc::text("]")])
    }

    #[test]
    fn a_group_that_fits_stays_flat() {
        assert_eq!(render(&list(&["1", "2"])), "[1, 2]");
    }

    #[test]
    fn a_group_that_does_not_fit_breaks_every_line() {
        let long = "x".repeat(50);
        let out = render(&list(&[&long, &long]));
        assert_eq!(out, format!("[\n    {long},\n    {long},\n]"));
    }

    #[test]
    fn a_line_suffix_waits_for_the_break_and_forces_it() {
        let doc = Doc::Group(vec![
            Doc::text("["),
            Doc::Indent(vec![
                Doc::SoftLine,
                Doc::text("1"),
                Doc::LineSuffix(" // one".into()),
                Doc::BreakParent,
                Doc::text(","),
                Doc::Line,
                Doc::text("2"),
            ]),
            Doc::SoftLine,
            Doc::text("]"),
        ]);
        assert_eq!(render(&doc), "[\n    1, // one\n    2\n]");
    }

    #[test]
    fn an_inner_group_stays_flat_when_the_outer_one_breaks() {
        let long = "y".repeat(80);
        let doc = Doc::Group(vec![
            Doc::text("{"),
            Doc::Indent(vec![Doc::Line, list(&["1", "2"]), Doc::text(","), Doc::Line, Doc::text(long.clone())]),
            Doc::Line,
            Doc::text("}"),
        ]);
        assert_eq!(render(&doc), format!("{{\n    [1, 2],\n    {long}\n}}"));
    }

    #[test]
    fn a_hard_line_breaks_its_group_and_leaves_no_trailing_space() {
        let doc = Doc::Group(vec![Doc::text("a"), Doc::Line, Doc::HardLine, Doc::text("b")]);
        assert_eq!(render(&doc), "a\n\nb");
    }
}
