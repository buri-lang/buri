//! The prose the toolchain ships, as data: every page under `src/docs/`, the
//! error and lint catalogs read out of them, and the markdown renderer that
//! prints a page to a terminal.
//!
//! It sits below the diagnostics because a templated diagnostic takes its
//! wording from its page. `buri docs`, which serves the pages, is in `buri`.

pub mod embedded;
pub mod errors;
pub mod frontmatter;
pub mod harness;
pub mod lints;
pub mod markdown;
pub mod topics;

use std::fmt::Write as _;

/// The column text is wrapped at, clamped once — here, on the way in.
///
/// It used to be a bare `usize` clamped inside `markdown::to_terminal`, so
/// `index` wrapped its listing against the raw `COLUMNS` while the page beneath
/// it wrapped against the clamped one: two widths on one screen.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Width(usize);

impl Width {
    /// Narrower than this and a signature will not fit; wider and prose stops
    /// being readable.
    const NARROWEST: usize = 40;
    const WIDEST: usize = 100;

    pub fn new(columns: usize) -> Width {
        Width(columns.clamp(Width::NARROWEST, Width::WIDEST))
    }

    /// As wide as text is ever wrapped, for a caller that collapses the
    /// whitespace afterwards and so does not want wrapping at all.
    pub fn widest() -> Width {
        Width(Width::WIDEST)
    }

    pub fn get(self) -> usize {
        self.0
    }
}

impl Default for Width {
    fn default() -> Width {
        Width::new(80)
    }
}

/// The page a diagnostic code takes its wording from, whichever catalog it is
/// in.
///
/// The two catalogs are separate because a compile error and a lint finding are
/// documented differently, and a diagnostic does not care which one it came
/// from: it has a code, and the code has a page.
pub fn page_of_code(code: &str) -> Option<&'static frontmatter::Page> {
    errors::page(code).or_else(|| lints::page(code))
}

/// The part of a page that is printed under the diagnostic itself.
///
/// Three things are dropped, each because the diagnostic it is printed under
/// has already said it. The title heading, which is the docs index's line. The
/// specimen — the ```` ```text ```` block showing what the diagnostic looks
/// like — which the reader is looking at. And the reproduction, because a
/// reader meeting a diagnostic has a program that provokes it in front of them
/// and does not need ours.
///
/// What is left is the freeform explanation, which is empty for a page carrying
/// only frontmatter and a reproduction, and so prints nothing at all.
pub fn explanation_of(page: &frontmatter::Page) -> String {
    let dropped = quoted_lines(page);
    let mut out = String::new();
    // A section heading is held back until something is printed under it, so a
    // section that was only a reproduction does not leave its title behind.
    let mut pending: Option<&str> = None;
    let mut in_fence = false;
    // A Buri fence prints as a reader sees it: no hidden `# ` lines, and no
    // blank line they leave at its top.
    let mut in_buri = false;
    let mut fence_empty = false;
    for (index, line) in page.body.lines().enumerate() {
        let number = index.saturating_add(1);
        if dropped.iter().any(|(first, last)| number >= *first && number <= *last) {
            continue;
        }
        let trimmed = line.trim_start();
        let delimiter = trimmed.starts_with("```");
        if delimiter {
            in_fence = !in_fence;
            in_buri = in_fence && trimmed.starts_with("```buri");
            fence_empty = true;
        } else if in_buri {
            if trimmed == "#" || trimmed.starts_with("# ") || (fence_empty && trimmed.is_empty()) {
                continue;
            }
            fence_empty = false;
        }
        if !in_fence && !delimiter && trimmed.starts_with('#') {
            pending = if trimmed.starts_with("# ") { None } else { Some(line) };
            continue;
        }
        if trimmed.is_empty() && !in_fence {
            if pending.is_none() {
                out.push('\n');
            }
            continue;
        }
        if let Some(heading) = pending.take() {
            let _ = writeln!(out, "{heading}\n");
        }
        let _ = writeln!(out, "{line}");
    }
    out.trim().to_string()
}

/// The line range of every block a diagnostic must not print back at its
/// reader: the `buri fail code=…` reproduction, and the transcript of the
/// diagnostic itself. Inclusive of both fences.
fn quoted_lines(page: &frontmatter::Page) -> Vec<(usize, usize)> {
    let bracketed = format!("[{}]", page.code);
    markdown::fences(page.body)
        .iter()
        .filter(|f| {
            // A lint page's example is the thing it explains, so it prints.
            let reproduction = f.lang == "buri"
                && f.info.as_ref().is_ok_and(|info| {
                    info.get("code").is_some() && info.mode.as_deref() == Some("fail")
                });
            let specimen = f.lang == "text"
                && f.body
                    .lines()
                    .next()
                    .is_some_and(|l| l.contains(&bracketed) && l.contains(": "));
            reproduction || specimen
        })
        .map(|f| (f.line, f.body_line.saturating_add(f.body.lines().count())))
        .collect()
}

/// `$COLUMNS`, or eighty. Public because a diagnostic's explanation is wrapped
/// to the same width as a documentation page, being the same text.
pub fn terminal_width() -> usize {
    std::env::var("COLUMNS").ok().and_then(|c| c.parse().ok()).unwrap_or(80)
}
