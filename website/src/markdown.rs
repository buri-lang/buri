//! Markdown to HTML.
//!
//! Not a general renderer. The corpus it serves is `cli/src/docs/**` and the
//! README, and `cli/tests/docs` already holds that corpus to a small dialect:
//! ATX headings, fenced blocks, lists, tables, block quotes, and inline
//! `code`, `**strong**`, `*emphasis*` and `[links](targets)`. Reference links,
//! autolinks, images and raw HTML are not in it.
//!
//! Anchors come from `documentation::markdown::slug`, which is GitHub's
//! algorithm, so a `#section` link written for the file on GitHub lands on the
//! same heading here.
#![allow(
    clippy::arithmetic_side_effects,
    reason = "every counter here is a line index or a byte offset into the \
              document being walked, and each is bounded by its length"
)]

use crate::highlight;
use crate::links::{Resolver, Target};
use buri::documentation::examples;
use buri::documentation::markdown::slug;

pub struct Rendered {
    pub html: String,
    /// Every heading's anchor, in order, so the link checker can answer
    /// whether `#foo` is on this page.
    pub anchors: Vec<String>,
    /// Every destination the page pointed at, for the same reason.
    pub links: Vec<Target>,
}

/// One page's markdown, rendered.
pub fn render(text: &str, resolver: &Resolver<'_>) -> Rendered {
    let mut renderer = Renderer::new(resolver);
    let lines: Vec<&str> = text.lines().collect();
    renderer.blocks(&lines);
    renderer.finish()
}

/// A registered title, as HTML.
///
/// A title in `documentation::topics` or in a page's frontmatter is prose with
/// backticks in it — "Libraries: `lib.buri` and the public surface" — and a
/// navigation entry that spelled the backticks would be the only place on the
/// site that did. There is nothing else in one, so this needs no resolver.
pub fn title(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let bytes = text.as_bytes();
    let mut at = 0usize;
    while at < bytes.len() {
        let rest = text.get(at..).unwrap_or("");
        if bytes.get(at) == Some(&b'`') {
            if let Some((body, length)) = code_span(rest) {
                out.push_str("<code>");
                highlight::escape(body, &mut out);
                out.push_str("</code>");
                at += length;
                continue;
            }
        }
        let character = rest.chars().next().unwrap_or(' ');
        highlight::escape(&character.to_string(), &mut out);
        at += character.len_utf8();
    }
    out
}

/// One line of markdown with no block structure — a frontmatter message, a
/// summary in a listing — rendered as the inline run it is.
pub fn render_inline(text: &str, resolver: &Resolver<'_>) -> Rendered {
    let mut renderer = Renderer::new(resolver);
    let html = renderer.inline(text);
    renderer.out = html;
    renderer.finish()
}

struct Renderer<'a> {
    resolver: &'a Resolver<'a>,
    out: String,
    anchors: Vec<String>,
    links: Vec<Target>,
}

impl<'a> Renderer<'a> {
    fn new(resolver: &'a Resolver<'a>) -> Renderer<'a> {
        Renderer { resolver, out: String::new(), anchors: Vec::new(), links: Vec::new() }
    }

    fn finish(self) -> Rendered {
        Rendered { html: self.out, anchors: self.anchors, links: self.links }
    }

    // -- blocks -------------------------------------------------------------

    fn blocks(&mut self, lines: &[&str]) {
        let mut at = 0usize;
        while at < lines.len() {
            let line = lines.get(at).copied().unwrap_or("");
            let trimmed = line.trim_start();
            if trimmed.is_empty() {
                at += 1;
                continue;
            }
            if trimmed.starts_with("<!--") {
                at = skip_comment(lines, at);
                continue;
            }
            if let Some((indent, info)) = fence_open(line) {
                at = self.fence(lines, at, indent, info);
                continue;
            }
            if let Some((level, title)) = atx(trimmed) {
                self.heading(level, title);
                at += 1;
                continue;
            }
            if is_rule(trimmed) {
                self.out.push_str("<hr>\n");
                at += 1;
                continue;
            }
            if trimmed.starts_with('>') {
                at = self.quote(lines, at);
                continue;
            }
            let next = lines.get(at + 1).copied().unwrap_or("");
            if trimmed.starts_with('|') && is_table_delimiter(next) {
                at = self.table(lines, at);
                continue;
            }
            if list_marker(line).is_some() {
                at = self.list(lines, at);
                continue;
            }
            at = self.paragraph(lines, at);
        }
    }

    fn heading(&mut self, level: usize, title: &str) {
        let anchor = slug(title);
        let level = level.clamp(1, 6);
        let body = self.inline(title);
        self.out.push_str(&format!(
            "<h{level} id=\"{anchor}\">{body}\
             <a class=\"anchor\" href=\"#{anchor}\" aria-label=\"Link to this section\">#</a>\
             </h{level}>\n",
            anchor = highlight::escaped(&anchor),
        ));
        self.links.push(Target::SameDocument { anchor: anchor.clone() });
        self.anchors.push(anchor);
    }

    /// A fenced block, from its opener to the first closer at or below its own
    /// indentation. An unterminated fence runs to the end of the document,
    /// which is what `documentation::markdown` does with one too.
    fn fence(&mut self, lines: &[&str], at: usize, indent: usize, info: &str) -> usize {
        let language = info.split_whitespace().next().unwrap_or("");
        let mut body = String::new();
        let mut cursor = at + 1;
        while cursor < lines.len() {
            let line = lines.get(cursor).copied().unwrap_or("");
            if line.trim_start().starts_with("```") && fence_open(line).is_some_and(|(_, rest)| rest.trim().is_empty())
            {
                cursor += 1;
                break;
            }
            let stripped = strip_indent(line, indent);
            body.push_str(stripped);
            body.push('\n');
            cursor += 1;
        }
        // A `# ` line in a Buri example is a hidden import the doctest suite
        // compiles and the reader is not shown. `buri docs` drops them, and a
        // page that showed them would be showing a different program.
        let body = if language == "buri" { examples::rendered(&body) } else { body };
        let inner = highlight::block(language, &body);
        let class = if highlight::highlights(language) {
            format!(" class=\"language-{}\"", highlight::escaped(language))
        } else if language.is_empty() {
            String::new()
        } else {
            format!(" class=\"plain-{}\"", highlight::escaped(language))
        };
        self.out.push_str(&format!("<pre><code{class}>{inner}</code></pre>\n"));
        cursor
    }

    fn quote(&mut self, lines: &[&str], at: usize) -> usize {
        let mut inner: Vec<&str> = Vec::new();
        let mut cursor = at;
        while cursor < lines.len() {
            let line = lines.get(cursor).copied().unwrap_or("");
            let trimmed = line.trim_start();
            if !trimmed.starts_with('>') {
                break;
            }
            let rest = trimmed.get(1..).unwrap_or("");
            inner.push(rest.strip_prefix(' ').unwrap_or(rest));
            cursor += 1;
        }
        self.out.push_str("<blockquote>\n");
        self.blocks(&inner);
        self.out.push_str("</blockquote>\n");
        cursor
    }

    fn table(&mut self, lines: &[&str], at: usize) -> usize {
        let header = cells(lines.get(at).copied().unwrap_or(""));
        let alignments = alignments(lines.get(at + 1).copied().unwrap_or(""));
        self.out.push_str("<div class=\"table-scroll\">\n<table>\n<thead>\n<tr>");
        for (column, cell) in header.iter().enumerate() {
            let body = self.inline(cell);
            self.out.push_str(&format!("<th{}>{body}</th>", align(&alignments, column)));
        }
        self.out.push_str("</tr>\n</thead>\n<tbody>\n");
        let mut cursor = at + 2;
        while cursor < lines.len() {
            let line = lines.get(cursor).copied().unwrap_or("");
            if !line.trim_start().starts_with('|') {
                break;
            }
            self.out.push_str("<tr>");
            for (column, cell) in cells(line).iter().enumerate() {
                let body = self.inline(cell);
                self.out.push_str(&format!("<td{}>{body}</td>", align(&alignments, column)));
            }
            self.out.push_str("</tr>\n");
            cursor += 1;
        }
        self.out.push_str("</tbody>\n</table>\n</div>\n");
        cursor
    }

    /// A list, and everything indented under it.
    ///
    /// An item's first run of lines is rendered inline, so a one-paragraph item
    /// is `<li>text</li>` rather than `<li><p>text</p></li>`; anything after it
    /// — a nested list, a code block, a second paragraph — goes back through
    /// the block loop.
    fn list(&mut self, lines: &[&str], at: usize) -> usize {
        let Some(first) = list_marker(lines.get(at).copied().unwrap_or("")) else { return at + 1 };
        let tag = if first.ordered { "ol" } else { "ul" };
        let start = if first.ordered && first.number != 1 {
            format!(" start=\"{}\"", first.number)
        } else {
            String::new()
        };
        self.out.push_str(&format!("<{tag}{start}>\n"));

        let mut cursor = at;
        while cursor < lines.len() {
            let line = lines.get(cursor).copied().unwrap_or("");
            let Some(marker) = list_marker(line) else { break };
            if marker.indent > first.indent || marker.ordered != first.ordered {
                break;
            }
            let content = marker.content;
            // The marker line is cut past the marker itself, not merely
            // dedented: leaving the `*` on would make the item a list again,
            // and the item a list again, and so on.
            let mut item: Vec<String> = vec![line.get(content..).unwrap_or("").to_string()];
            cursor += 1;
            while cursor < lines.len() {
                let next = lines.get(cursor).copied().unwrap_or("");
                if next.trim().is_empty() {
                    // A blank line ends the item unless the list continues
                    // under it.
                    let continues = lines
                        .iter()
                        .skip(cursor + 1)
                        .find(|line| !line.trim().is_empty())
                        .is_some_and(|line| {
                            indent_of(line) >= content
                                || list_marker(line).is_some_and(|m| m.indent == first.indent)
                        });
                    if !continues {
                        break;
                    }
                    item.push(String::new());
                    cursor += 1;
                    continue;
                }
                if list_marker(next).is_some_and(|m| m.indent <= first.indent) {
                    break;
                }
                if indent_of(next) < content && list_marker(next).is_none() && item
                    .last()
                    .is_some_and(|line| line.trim().is_empty())
                {
                    break;
                }
                item.push(strip_indent(next, content).to_string());
                cursor += 1;
            }
            self.item(&item);
        }
        self.out.push_str(&format!("</{tag}>\n"));
        cursor
    }

    fn item(&mut self, lines: &[String]) {
        let borrowed: Vec<&str> = lines.iter().map(String::as_str).collect();
        let lead = borrowed
            .iter()
            .position(|line| line.trim().is_empty() || starts_block(line))
            .unwrap_or(borrowed.len());
        self.out.push_str("<li>");
        if lead > 0 {
            let text = borrowed.get(..lead).unwrap_or(&[]).join("\n");
            let body = self.inline(&text);
            self.out.push_str(&body);
        }
        let rest = borrowed.get(lead..).unwrap_or(&[]);
        if rest.iter().any(|line| !line.trim().is_empty()) {
            self.out.push('\n');
            self.blocks(rest);
        }
        self.out.push_str("</li>\n");
    }

    fn paragraph(&mut self, lines: &[&str], at: usize) -> usize {
        let mut cursor = at;
        let mut collected: Vec<&str> = Vec::new();
        while cursor < lines.len() {
            let line = lines.get(cursor).copied().unwrap_or("");
            if line.trim().is_empty() {
                break;
            }
            if cursor > at && starts_block(line) {
                break;
            }
            collected.push(line.trim_end());
            cursor += 1;
        }
        let text = collected.join("\n");
        let body = self.inline(&text);
        self.out.push_str(&format!("<p>{body}</p>\n"));
        cursor.max(at + 1)
    }

    // -- inline -------------------------------------------------------------

    fn inline(&mut self, text: &str) -> String {
        let mut out = String::with_capacity(text.len());
        let bytes = text.as_bytes();
        let mut at = 0usize;
        while at < bytes.len() {
            let rest = text.get(at..).unwrap_or("");
            let byte = bytes.get(at).copied().unwrap_or(0);
            match byte {
                b'\\' => {
                    let next = text.get(at + 1..).and_then(|rest| rest.chars().next());
                    match next.filter(|c| c.is_ascii_punctuation()) {
                        Some(character) => {
                            highlight::escape(&character.to_string(), &mut out);
                            at += 1 + character.len_utf8();
                        }
                        None => {
                            out.push('\\');
                            at += 1;
                        }
                    }
                }
                b'`' => match code_span(rest) {
                    Some((body, length)) => {
                        out.push_str("<code>");
                        highlight::escape(body, &mut out);
                        out.push_str("</code>");
                        at += length;
                    }
                    None => {
                        out.push('`');
                        at += 1;
                    }
                },
                b'*' if rest.starts_with("**") => match emphasis(rest, "**") {
                    Some((body, length)) => {
                        let inner = self.inline(body);
                        out.push_str(&format!("<strong>{inner}</strong>"));
                        at += length;
                    }
                    None => {
                        out.push('*');
                        at += 1;
                    }
                },
                b'*' => match emphasis(rest, "*") {
                    Some((body, length)) => {
                        let inner = self.inline(body);
                        out.push_str(&format!("<em>{inner}</em>"));
                        at += length;
                    }
                    None => {
                        out.push('*');
                        at += 1;
                    }
                },
                b'[' => match link(rest) {
                    Some((label, destination, length)) => {
                        let inner = self.inline(label);
                        out.push_str(&self.anchor(&inner, destination));
                        at += length;
                    }
                    None => {
                        out.push('[');
                        at += 1;
                    }
                },
                _ => {
                    let character = rest.chars().next().unwrap_or(' ');
                    highlight::escape(&character.to_string(), &mut out);
                    at += character.len_utf8();
                }
            }
        }
        out
    }

    /// One link, resolved. A destination that points nowhere keeps its text
    /// and loses its link rather than becoming an `href` that goes nowhere.
    fn anchor(&mut self, label: &str, destination: &str) -> String {
        let target = self.resolver.classify(destination);
        let href = self.resolver.href(&target);
        self.links.push(target.clone());
        match href {
            Some(href) => {
                let external = matches!(target, Target::External { .. } | Target::Repository { .. });
                let attributes = if external { " rel=\"noreferrer\"" } else { "" };
                format!("<a href=\"{}\"{attributes}>{label}</a>", highlight::escaped(&href))
            }
            None => label.to_string(),
        }
    }
}

// ---------------------------------------------------------------------------
// Line shapes
// ---------------------------------------------------------------------------

fn indent_of(line: &str) -> usize {
    line.len().saturating_sub(line.trim_start().len())
}

fn strip_indent(line: &str, indent: usize) -> &str {
    let take = indent.min(indent_of(line));
    line.get(take..).unwrap_or("")
}

/// An ATX heading's level and title. `#hashtag` is not a heading; ATX needs
/// the space.
fn atx(trimmed: &str) -> Option<(usize, &str)> {
    let hashes = trimmed.bytes().take_while(|byte| *byte == b'#').count();
    if hashes == 0 || hashes > 6 {
        return None;
    }
    let rest = trimmed.get(hashes..)?.strip_prefix(' ')?;
    Some((hashes, rest.trim()))
}

fn fence_open(line: &str) -> Option<(usize, &str)> {
    let trimmed = line.trim_start();
    let rest = trimmed.strip_prefix("```")?;
    Some((indent_of(line), rest))
}

fn is_rule(trimmed: &str) -> bool {
    for marker in ['-', '*', '_'] {
        let count = trimmed.chars().filter(|c| *c == marker).count();
        if count >= 3 && trimmed.chars().all(|c| c == marker || c == ' ') {
            return true;
        }
    }
    false
}

fn is_table_delimiter(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed.starts_with('|')
        && trimmed.contains('-')
        && trimmed.chars().all(|c| matches!(c, '|' | '-' | ':' | ' '))
}

struct Marker {
    indent: usize,
    /// Where the item's own text begins on the line.
    content: usize,
    ordered: bool,
    number: usize,
}

fn list_marker(line: &str) -> Option<Marker> {
    let indent = indent_of(line);
    let rest = line.trim_start();
    for bullet in ["- ", "* ", "+ "] {
        if rest.starts_with(bullet) {
            return Some(Marker { indent, content: indent + 2, ordered: false, number: 1 });
        }
    }
    let digits = rest.chars().take_while(char::is_ascii_digit).count();
    if digits == 0 || digits > 9 {
        return None;
    }
    if !rest.get(digits..)?.starts_with(". ") {
        return None;
    }
    let number = rest.get(..digits)?.parse::<usize>().ok()?;
    Some(Marker { indent, content: indent + digits + 2, ordered: true, number })
}

/// Whether a line opens a block, and so ends the paragraph above it.
fn starts_block(line: &str) -> bool {
    let trimmed = line.trim_start();
    trimmed.is_empty()
        || trimmed.starts_with("```")
        || trimmed.starts_with('>')
        || trimmed.starts_with("<!--")
        || atx(trimmed).is_some()
        || is_rule(trimmed)
        || list_marker(line).is_some()
}

fn skip_comment(lines: &[&str], at: usize) -> usize {
    let mut cursor = at;
    while cursor < lines.len() {
        let line = lines.get(cursor).copied().unwrap_or("");
        cursor += 1;
        if line.contains("-->") {
            break;
        }
    }
    cursor
}

fn cells(line: &str) -> Vec<String> {
    let trimmed = line.trim();
    let inner = trimmed.strip_prefix('|').unwrap_or(trimmed);
    let inner = inner.strip_suffix('|').unwrap_or(inner);
    inner.split('|').map(|cell| cell.trim().to_string()).collect()
}

fn alignments(line: &str) -> Vec<&'static str> {
    cells(line)
        .iter()
        .map(|cell| match (cell.starts_with(':'), cell.ends_with(':')) {
            (true, true) => "center",
            (false, true) => "right",
            _ => "left",
        })
        .collect()
}

fn align(alignments: &[&'static str], column: usize) -> String {
    match alignments.get(column) {
        Some(&"left") | None => String::new(),
        Some(which) => format!(" class=\"align-{which}\""),
    }
}

/// A code span, and how many bytes of the input it consumed. The opening run
/// of backticks is closed by a run of exactly the same length, so `` `a` ``
/// holds a backtick.
fn code_span(rest: &str) -> Option<(&str, usize)> {
    let ticks = rest.bytes().take_while(|byte| *byte == b'`').count();
    let fence = rest.get(..ticks)?;
    let body_start = ticks;
    let mut search = body_start;
    while let Some(found) = rest.get(search..)?.find(fence) {
        let start = search + found;
        let run = rest.get(start..)?.bytes().take_while(|byte| *byte == b'`').count();
        if run == ticks {
            let body = rest.get(body_start..start)?;
            let body = body.strip_prefix(' ').unwrap_or(body);
            let body = body.strip_suffix(' ').unwrap_or(body);
            return Some((body, start + ticks));
        }
        search = start + run;
    }
    None
}

/// An emphasis run. The opener must be followed by something other than a
/// space and the closer preceded by one, so `a * b` is arithmetic rather than
/// the start of an italic.
fn emphasis<'a>(rest: &'a str, marker: &str) -> Option<(&'a str, usize)> {
    let after = rest.get(marker.len()..)?;
    if after.starts_with(char::is_whitespace) || after.is_empty() {
        return None;
    }
    let mut search = 0usize;
    while let Some(found) = after.get(search..)?.find(marker) {
        let end = search + found;
        if end == 0 {
            search = end + marker.len();
            continue;
        }
        let body = after.get(..end)?;
        if body.ends_with(char::is_whitespace) {
            search = end + marker.len();
            continue;
        }
        return Some((body, marker.len() + end + marker.len()));
    }
    None
}

/// `[label](destination)`, and how many bytes it consumed. Parentheses inside
/// the destination are counted, so a link to one is not cut in half.
fn link(rest: &str) -> Option<(&str, &str, usize)> {
    let mut depth = 0usize;
    let mut label_end = None;
    let bytes = rest.as_bytes();
    let mut at = 0usize;
    while at < bytes.len() {
        match bytes.get(at).copied().unwrap_or(0) {
            b'\\' => at += 1,
            b'[' => depth += 1,
            b']' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    label_end = Some(at);
                    break;
                }
            }
            _ => {}
        }
        at += 1;
    }
    let label_end = label_end?;
    if !rest.get(label_end + 1..)?.starts_with('(') {
        return None;
    }
    let destination_start = label_end + 2;
    let mut parentheses = 1usize;
    let mut cursor = destination_start;
    while cursor < bytes.len() {
        match bytes.get(cursor).copied().unwrap_or(0) {
            b'(' => parentheses += 1,
            b')' => {
                parentheses = parentheses.saturating_sub(1);
                if parentheses == 0 {
                    let label = rest.get(1..label_end)?;
                    let destination = rest.get(destination_start..cursor)?;
                    return Some((label, destination.trim(), cursor + 1));
                }
            }
            _ => {}
        }
        cursor += 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pages::{Content, Page, Site, Source};
    use std::path::PathBuf;

    fn site() -> Site {
        let page = |route: &str, source: &str| Page {
            route: route.to_string(),
            title: route.to_string(),
            label: String::new(),
            summary: String::new(),
            source: Source { path: source.to_string(), directory: false },
            section: None,
            content: Content::Prose(String::new()),
            see_also: Vec::new(),
            facts: Vec::new(),
            adapted_from: None,
        };
        Site {
            root: PathBuf::from("/nowhere"),
            pages: vec![
                page("build/tags", "cli/src/docs/build/tags.md"),
                page("build/testing", "cli/src/docs/build/testing.md"),
            ],
        }
    }

    fn rendered(text: &str) -> Rendered {
        let site = site();
        let page = site.page("build/tags").expect("the fixture has this page");
        let resolver = Resolver::for_page(&site, page);
        render(text, &resolver)
    }

    #[test]
    fn a_heading_carries_the_anchor_github_would_give_it() {
        let out = rendered("## Tags and tests\n");
        assert!(out.html.contains("id=\"tags-and-tests\""), "{}", out.html);
        assert_eq!(out.anchors, vec!["tags-and-tests".to_string()]);
    }

    #[test]
    fn a_doc_link_to_another_page_becomes_a_link_to_that_page() {
        let out = rendered("See [testing](./testing.md#golden).\n");
        assert!(out.html.contains("href=\"../../build/testing/#golden\""), "{}", out.html);
    }

    #[test]
    fn a_doc_link_to_something_the_site_does_not_publish_goes_to_github() {
        let out = rendered("See [the design notes](../../../../design/).\n");
        assert!(
            out.html.contains("https://github.com/buri-lang/buri/tree/main/design"),
            "{}",
            out.html
        );
    }

    #[test]
    fn inline_runs_are_rendered_and_escaped() {
        let out = rendered("A `Vec<T>` is **strong** and *emphatic*.\n");
        assert!(out.html.contains("<code>Vec&lt;T&gt;</code>"), "{}", out.html);
        assert!(out.html.contains("<strong>strong</strong>"), "{}", out.html);
        assert!(out.html.contains("<em>emphatic</em>"), "{}", out.html);
    }

    #[test]
    fn an_asterisk_between_spaces_is_not_an_emphasis() {
        let out = rendered("The product a * b.\n");
        assert!(out.html.contains("a * b"), "{}", out.html);
    }

    #[test]
    fn a_list_nests_and_a_table_scrolls() {
        let out = rendered("- one\n- two\n     * inner\n\n| a | b |\n|---|---|\n| 1 | 2 |\n");
        assert!(out.html.contains("<ul>"), "{}", out.html);
        assert!(out.html.matches("<ul>").count() == 2, "{}", out.html);
        assert!(out.html.contains("<table>"), "{}", out.html);
        assert!(out.html.contains("<td>1</td>"), "{}", out.html);
    }

    #[test]
    fn a_fenced_block_is_highlighted_and_an_unknown_language_is_not() {
        let out = rendered("```buri\nlet n = 1;\n```\n\n```sh\nburi build\n```\n");
        assert!(out.html.contains("class=\"language-buri\""), "{}", out.html);
        assert!(out.html.contains("<span class=\"keyword\">let</span>"), "{}", out.html);
        assert!(out.html.contains("<code class=\"plain-sh\">buri build\n</code>"), "{}", out.html);
    }

    #[test]
    fn an_html_comment_is_not_rendered() {
        let out = rendered("<!-- a note to the editor -->\n\n# Title\n");
        assert!(!out.html.contains("a note to the editor"), "{}", out.html);
    }
}
