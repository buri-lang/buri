//! **Every example is laid out the way `buri format` lays out source.**
//!
//! `examples.rs` asks whether a fence does what the document says. This asks
//! the smaller question next to it: whether what a reader copies out of the
//! page is what the formatter would have written. A repository that formats its
//! sources and not its documentation ends up with two house styles, and the one
//! a newcomer copies is the one in the prose.
//!
//! There is no separate style here, and there is nothing to configure: a fence
//! goes through `crate::formatting`, the same printer `buri format` is. A
//! fragment is wrapped first — `wrap=body` is statements, so they are laid out
//! inside a function and the one level of indent the wrapper added is taken
//! back off — and a `sig` block is read as the standard-library module it is.
//!
//! # The hidden lines
//!
//! A fence may carry lines a reader never sees — an import the example needs
//! and the prose does not want to talk about, marked `# `. They are laid out
//! with everything else, because they are part of the module the harness
//! compiles and because the leading import run is *sorted*: a hidden import and
//! a visible one written beside it belong in one order, not two. What comes
//! back is marked hidden again, matched by its text, so a line that was hidden
//! stays hidden wherever the layout moved it to.
//!
//! # What is not asked
//!
//! Three kinds of fence have no canonical form, and each is a silence with a
//! reason rather than an omission ([`Verdict::Silent`]):
//!
//!   * a block the formatter cannot read whole. A page about a syntax error
//!     shows the syntax error, and `buri format` says the same thing about the
//!     same file: what it could not read it does not vouch for.
//!   * an empty one.
//!   * `wrap=expr`, whose layout is the layout of the expression it is bound
//!     into rather than anything the fence owns.

use crate::documentation::examples::{Block, Claim, Failure, Wrap};
use crate::documentation::markdown;
use crate::formatting::{self, Dialect};

/// The function a statement fragment is laid out inside.
///
/// It is never shown. `wrap=body` says the fence is statements, and statements
/// are laid out relative to the block they are in, so there has to *be* a
/// block; the one level of indent it adds comes back off below.
const WRAPPER: &str = "fn __fence(): () {";

/// What the formatter says about one fence.
#[derive(Clone, Debug)]
pub enum Verdict {
    /// The fence is already what the formatter would write.
    Clean,
    /// It is not, and this is what the formatter would write.
    Drifted(String),
    /// There is nothing to say about it, for this reason.
    Silent(&'static str),
}

/// The canonical layout of one block, hidden lines and all.
///
/// The text asked about is the module the harness compiles — the fence with its
/// `# ` markers taken off — and the answer comes back in the same form.
/// [`hide_again`] is what puts the markers back on the way to the page.
pub fn verdict(block: &Block) -> Verdict {
    let written = unmarked(&block.body);
    if written.trim().is_empty() {
        return Verdict::Silent("the fence shows nothing");
    }
    // An expression fragment is laid out by whatever binds it, and the fence
    // does not say what that is.
    if block.wrap == Wrap::Expr {
        return Verdict::Silent("an expression fragment has no layout of its own");
    }
    match laid_out(&written, block) {
        None => Verdict::Silent("the formatter cannot read this block whole"),
        Some(text) if text == written => Verdict::Clean,
        Some(text) => Verdict::Drifted(text),
    }
}

/// The fence as the harness compiles it: `# ` off, `## ` down to `# `, and
/// every line — hidden or not — in the place it was written.
fn unmarked(body: &str) -> String {
    let mut out = String::with_capacity(body.len());
    for line in body.lines() {
        out.push_str(&unmark(line));
        out.push('\n');
    }
    out
}

fn unmark(line: &str) -> String {
    match line.strip_prefix("##") {
        Some(rest) => format!("#{rest}"),
        None => line.strip_prefix("# ").unwrap_or(if line == "#" { "" } else { line }).to_string(),
    }
}

/// Which parser reads the block.
///
/// A `sig` block is a list of signatures, which is a standard-library module —
/// the same thing the harness compiles it as, and the one dialect where a `fn`
/// may be declared without a body.
fn dialect(block: &Block) -> Dialect {
    if matches!(block.claim, Claim::Sig) {
        Dialect::Std
    } else {
        Dialect::Source
    }
}

/// One fence's text, laid out the way the block says it is written: a module,
/// or the statements of one.
fn laid_out(text: &str, block: &Block) -> Option<String> {
    match block.wrap {
        Wrap::Module => whole(text, dialect(block)),
        Wrap::Body => statements(text),
        Wrap::Expr => None,
    }
}

/// A whole module, laid out.
///
/// `None` when the formatter refuses it, and also when it comes back with a
/// region in it: a region is a declaration printed exactly as it was written,
/// which is the formatter saying it has no opinion about that text.
fn whole(text: &str, dialect: Dialect) -> Option<String> {
    let out = formatting::formatted(text, dialect)?;
    out.regions.is_empty().then_some(out.text)
}

/// Statements, laid out inside a function that is then taken away again.
fn statements(text: &str) -> Option<String> {
    let inner = whole(&format!("{WRAPPER}\n{text}}}\n"), Dialect::Source)?;
    let body = inner.strip_prefix(&format!("{WRAPPER}\n"))?.strip_suffix("}\n")?;
    undent(body)
}

/// One level of indentation off every line, or `None` if some line has not got
/// one — which would mean the wrapper above did not print the shape this
/// expects, and a fence is not the place to find that out.
fn undent(text: &str) -> Option<String> {
    let mut out = String::with_capacity(text.len());
    for line in text.lines() {
        if !line.is_empty() {
            out.push_str(line.strip_prefix("    ")?);
        }
        out.push('\n');
    }
    Some(out)
}

/// The fence's drift, as the failure the documentation suite reports.
pub fn check(block: &Block) -> Option<Failure> {
    let Verdict::Drifted(canonical) = verdict(block) else {
        return None;
    };
    Some(Failure {
        origin: block.origin.clone(),
        what: "this example is not laid out the way `buri format` lays out source".into(),
        detail: format!(
            "what a reader copies out of a page is the house style, so it is the house \
             style that is printed there.\nRun `buri format` over the \
             documentation, which rewrites the fence and nothing else around \
             it.\nexpected:\n{}\n  actual:\n{}",
            indent(&canonical),
            indent(&unmarked(&block.body))
        ),
    })
}

fn indent(text: &str) -> String {
    text.lines().map(|l| format!("  | {l}")).collect::<Vec<_>>().join("\n")
}

// ---------------------------------------------------------------------------
// Writing it back
// ---------------------------------------------------------------------------

/// One document with every drifted fence rewritten, or `None` when nothing
/// moved.
///
/// Only the fence bodies change. The prose is the author's.
pub fn format_document(file: &str, text: &str) -> Option<String> {
    let blocks = crate::documentation::examples::extract(file, text).blocks;
    let mut lines: Vec<String> = text.lines().map(String::from).collect();
    let mut changed = false;
    // Back to front: a rewrite that changes a fence's line count moves every
    // fence under it.
    for block in blocks.iter().rev() {
        let Verdict::Drifted(canonical) = verdict(block) else {
            continue;
        };
        let Some(body) = hide_again(block, &canonical) else {
            continue;
        };
        let first = block.origin.line.saturating_sub(1);
        let last = first.saturating_add(block.body.lines().count());
        if last > lines.len() {
            continue;
        }
        let pad = " ".repeat(block.indent);
        let replacement: Vec<String> = body
            .lines()
            .map(|l| if l.is_empty() { String::new() } else { format!("{pad}{l}") })
            .collect();
        lines.splice(first..last, replacement);
        changed = true;
    }
    if !changed {
        return None;
    }
    let mut out = lines.join("\n");
    if text.ends_with('\n') {
        out.push('\n');
    }
    Some(out)
}

/// Whether a line of a fence is one the reader never sees.
fn is_hidden(line: &str) -> bool {
    line == "#" || (line.starts_with("# ") && !line.starts_with("## "))
}

/// The laid-out text with the hidden lines hidden again.
///
/// Which of the output's lines are the hidden ones is not a guess and not a
/// string search: the fence is laid out a **second** time with its hidden lines
/// left out, and the lines the first output has that the second does not are
/// exactly them. That survives everything the layout is allowed to do to a
/// hidden line — sorting it into the import run, breaking it over four lines,
/// giving it a trailing comma — because both runs do the same thing to
/// everything else.
///
/// `None` when the two do not line up: the visible half on its own does not
/// parse, or a line went missing between the two runs. There is nothing honest
/// to write back then, and the failure names the fence to fix by hand.
fn hide_again(block: &Block, canonical: &str) -> Option<String> {
    let hidden: Vec<&str> = block.body.lines().filter(|l| is_hidden(l)).collect();
    if hidden.is_empty() {
        return Some(canonical.to_string());
    }
    let visible: String = block
        .body
        .lines()
        .filter(|l| !is_hidden(l))
        .map(|l| format!("{}\n", unmark(l)))
        .collect();
    // The visible half on its own may be no module at all — a fence whose
    // hidden lines are the function its visible ones are the body of, say. Then
    // there is nothing to compare against, and counting is what is left.
    let Some(shown) = laid_out(&visible, block) else {
        return by_position(&block.body, canonical);
    };
    let mut left = shown.lines().filter(|l| !l.trim().is_empty()).peekable();

    let mut out = String::with_capacity(canonical.len());
    let mut marked = 0usize;
    for line in canonical.lines() {
        // A blank line is the layout's, never a hidden one: nothing hides an
        // empty line.
        if line.trim().is_empty() {
            out.push('\n');
            continue;
        }
        if left.next_if(|v| v.trim() == line.trim()).is_some() {
            // `#` cannot begin a line of Buri, so a visible line that does is
            // an escaped marker and goes back escaped.
            if line.starts_with('#') {
                out.push('#');
            }
            out.push_str(line);
        } else {
            marked = marked.saturating_add(1);
            out.push_str("# ");
            out.push_str(line);
        }
        out.push('\n');
    }
    // Every visible line accounted for, and something hidden: anything else is
    // the two runs having disagreed about more than the hidden lines, and
    // counting is the second opinion.
    if left.next().is_some() || marked == 0 {
        return by_position(&block.body, canonical);
    }
    Some(out)
}

/// The same, by counting rather than by comparing.
///
/// The fence the comparison cannot answer for is the one whose hidden lines
/// *enclose* its visible ones — a hidden `fn demo(): Meters {`, three lines of
/// body, a hidden `}` — because its visible half is not a module and cannot be
/// laid out on its own. There the hidden lines are a run at the top and a run at
/// the bottom with nothing in between, so the same number of lines at each end
/// of the output are the hidden ones.
///
/// Anything else is refused. A hidden line in the middle of a fence the
/// comparison could not account for is not something to guess at, and the
/// failure names the fence for a person to lay out by hand.
fn by_position(body: &str, canonical: &str) -> Option<String> {
    let written: Vec<&str> = body.lines().collect();
    let above = written.iter().take_while(|l| is_hidden(l)).count();
    let below = written.iter().rev().take_while(|l| is_hidden(l)).count();
    let hidden = written.iter().filter(|l| is_hidden(l)).count();
    if hidden == 0 || above.saturating_add(below) != hidden {
        return None;
    }
    // Blank lines are the layout's and are never hidden, so the runs are
    // counted over the lines that hold something.
    let held: Vec<usize> = canonical
        .lines()
        .enumerate()
        .filter(|(_, l)| !l.trim().is_empty())
        .map(|(i, _)| i)
        .collect();
    if above.saturating_add(below) > held.len() {
        return None;
    }
    let top = held.get(..above)?.to_vec();
    let bottom = held.get(held.len().saturating_sub(below)..)?.to_vec();
    let mut out = String::with_capacity(canonical.len());
    for (i, line) in canonical.lines().enumerate() {
        if top.contains(&i) || bottom.contains(&i) {
            out.push_str("# ");
        } else if line.starts_with('#') {
            out.push('#');
        }
        out.push_str(line);
        out.push('\n');
    }
    Some(out)
}

/// A source file whose documentation comments hold examples, with every drifted
/// fence in them rewritten.
///
/// A documentation comment is documentation, and an example in one is copied
/// the same way an example in a page is. `doc_comments` turns the file into a
/// document with one line per source line, so a fence's lines here are that
/// fence's lines there — which is what makes writing the answer back a splice
/// rather than a search.
pub fn format_doc_comments(source: &str) -> Option<String> {
    if !crate::documentation::examples::has_examples(source) {
        return None;
    }
    let document = crate::documentation::examples::doc_comments(source);
    let blocks = crate::documentation::examples::extract("", &document).blocks;
    let mut lines: Vec<String> = source.lines().map(String::from).collect();
    let mut changed = false;
    for block in blocks.iter().rev() {
        let Verdict::Drifted(canonical) = verdict(block) else {
            continue;
        };
        let Some(body) = hide_again(block, &canonical) else {
            continue;
        };
        let first = block.origin.line.saturating_sub(1);
        let last = first.saturating_add(block.body.lines().count());
        if last > lines.len() {
            continue;
        }
        // The marker every line of this comment carries — `///` or `//!`, at
        // whatever column the comment is written in.
        let Some(marker) = lines.get(first).and_then(|l| comment_marker(l)) else {
            continue;
        };
        let replacement: Vec<String> = body
            .lines()
            .map(|l| if l.is_empty() { marker.clone() } else { format!("{marker} {l}") })
            .collect();
        lines.splice(first..last, replacement);
        changed = true;
    }
    if !changed {
        return None;
    }
    let mut out = lines.join("\n");
    if source.ends_with('\n') {
        out.push('\n');
    }
    Some(out)
}

/// The `///` or `//!` that opens a documentation line, with its indentation.
fn comment_marker(line: &str) -> Option<String> {
    let trimmed = line.trim_start();
    let marker = ["//!", "///"].into_iter().find(|m| trimmed.starts_with(m))?;
    let indent = line.len().checked_sub(trimmed.len())?;
    Some(format!("{}{marker}", " ".repeat(indent)))
}

/// Every markdown document under `dir`, for the command that formats them.
///
/// The same walk `buri format` does over sources, and the same two exclusions:
/// nothing hidden, nothing built.
pub fn documents_under(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    if dir.is_file() {
        out.push(dir.to_path_buf());
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mut entries: Vec<_> = entries.filter_map(Result::ok).collect();
    entries.sort_by_key(std::fs::DirEntry::path);
    for e in entries {
        let p = e.path();
        let name = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        if name.starts_with('.') || name == "target" || name == "node_modules" {
            continue;
        }
        if p.is_dir() {
            documents_under(&p, out);
        } else if p.extension().is_some_and(|x| x == "md") {
            out.push(p);
        }
    }
}

/// Whether a document has anything for this to do, without parsing it.
pub fn has_examples(text: &str) -> bool {
    markdown::fences(text).iter().any(|f| f.lang == "buri")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::documentation::examples::extract;

    fn only(doc: &str) -> Verdict {
        let blocks = extract("test.md", doc).blocks;
        assert_eq!(blocks.len(), 1, "the document should hold one block");
        verdict(&blocks[0])
    }

    #[test]
    fn a_module_indented_by_two_is_reported_and_rewritten() {
        let doc = "```buri\nexport fn double(n: Int): Int {\n  n * 2\n}\n```\n";
        let Verdict::Drifted(text) = only(doc) else { panic!("two spaces is drift") };
        assert_eq!(text, "export fn double(n: Int): Int {\n    n * 2\n}\n");
        let out = format_document("test.md", doc).expect("the document is rewritten");
        assert_eq!(out, "```buri\nexport fn double(n: Int): Int {\n    n * 2\n}\n```\n");
        assert!(format_document("test.md", &out).is_none(), "and then it is a fixed point");
    }

    /// The wrapper is what makes a fragment laid out at all, and it must not
    /// leave a trace: what comes back is the statements, at the column they
    /// were written in.
    #[test]
    fn a_statement_fragment_keeps_its_own_column() {
        let doc = "```buri wrap=body\nlet a = {\n  let b = 1;\n  b + 1\n};\n```\n";
        let Verdict::Drifted(text) = only(doc) else { panic!("two spaces is drift") };
        assert_eq!(text, "let a = {\n    let b = 1;\n    b + 1\n};\n");
    }

    #[test]
    fn an_example_already_laid_out_is_clean() {
        let doc = "```buri\nexport fn double(n: Int): Int {\n    n * 2\n}\n```\n";
        assert!(matches!(only(doc), Verdict::Clean));
        assert!(format_document("test.md", doc).is_none());
    }

    /// A page about a syntax error shows the syntax error. `buri format` says
    /// the same thing about the same file: what it cannot read whole it does
    /// not vouch for.
    #[test]
    fn a_block_the_formatter_cannot_read_is_a_silence_with_a_reason() {
        let doc = "```buri fail code=unclosed-delimiter\nexport fn f(: Int {\n}\n```\n\
                   ```error\nexpected\n```\n";
        assert!(matches!(only(doc), Verdict::Silent(_)), "{:?}", only(doc));
    }

    /// A hidden line is laid out with everything else and comes back hidden.
    #[test]
    fn a_hidden_line_stays_hidden() {
        let doc = "```buri wrap=body\n#   let small: I32 = 5;\n\
                   let a = {\n  let b = 1;\n  b + 1\n};\n```\n";
        let out = format_document("test.md", doc).expect("rewritten");
        assert!(out.contains("# let small: I32 = 5;\n"), "{out}");
        assert!(out.contains("    let b = 1;\n"), "{out}");
        assert!(format_document("test.md", &out).is_none(), "and then it is a fixed point");
    }

    /// A hidden import written among the visible ones is sorted with them, and
    /// is still hidden wherever it lands.
    #[test]
    fn a_hidden_import_is_sorted_with_the_visible_ones() {
        let doc = "```buri\nfrom \"core/str\" import * as str;\n\
                   # from \"core/io\" import * as io;\n\
                   export fn f(): Int {\n  1\n}\n```\n";
        let out = format_document("test.md", doc).expect("rewritten");
        assert!(out.contains("# from \"core/io\" import * as io;\n"), "{out}");
        // Sorted: `core/io` before `core/str`, hidden or not.
        let io = out.find("core/io").expect("the hidden import");
        let s = out.find("core/str").expect("the visible one");
        assert!(io < s, "the run is sorted as one:\n{out}");
    }

    /// The comment's own marker and column come back, and a blank documentation
    /// line does not grow a trailing space.
    #[test]
    fn an_example_in_a_documentation_comment_is_rewritten_in_place() {
        let source = "//! ```buri\n//! export fn f(): Int {\n//!   1\n//! }\n//! ```\n\
                      export fn f(): Int {\n    1\n}\n";
        let out = format_doc_comments(source).expect("rewritten");
        assert!(out.contains("//!     1\n"), "{out}");
        assert!(out.starts_with("//! ```buri\n"), "{out}");
        assert!(format_doc_comments(&out).is_none(), "and then it is a fixed point");
    }
}
