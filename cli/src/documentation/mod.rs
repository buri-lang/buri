//! `buri docs` — the CLI is the documentation.
//!
//! Every reference an agent or a person needs is served by the binary that
//! implements the language, so what you read is what this toolchain does.
//! There is no website to fall behind, no copy to forget to update, and no
//! version skew: the prose ships inside the executable, and `cargo test`
//! compiles every example in it.
//!
//! Two properties are deliberate:
//!
//!   * **It works outside a repository.** Topics, the grammar, and the schemas
//!     are compiled in, so `buri docs language/effects` answers in an empty
//!     directory. Only a `//...` argument needs a workspace.
//!   * **Every kind of documentation is one `DocSource`.** Prose today; the
//!     CLI reference, the standard library, and the error catalog next. Adding
//!     one is a line in `sources()`, and the index, search, and `--format=json`
//!     pick it up without being told.
#![allow(
    clippy::print_stdout,
    clippy::print_stderr,
    reason = "this file is `buri docs` itself, and a page, an index, and a `there is no such \
              topic` are the command's own output; the compiler's diagnostics still go through \
              the Session"
)]
#![allow(
    clippy::arithmetic_side_effects,
    reason = "the arithmetic here is column counting and search scores measured off text already \
              in memory, both bounded by that text's length"
)]

pub mod assemble;
pub mod errors;
pub mod examples;
pub mod frontmatter;
pub mod grammar;
pub mod harness;
pub mod layout;
pub mod lints;
pub mod markdown;
pub mod reference;
pub mod topics;

use crate::commands::arguments;
use crate::commands::arguments::Format;
use crate::diagnostics::Invariant as _;
use crate::documentation::topics::{Kind, Topic};
use std::fmt::Write as _;

/// How a page is rendered.
///
/// Colour lives inside `Human`, for the reason `build::session::Rendering`
/// gives: an escape sequence in a JSON stream corrupts it, so "colour, in
/// JSON" should be a thing that cannot be written down rather than a
/// correlation `command_docs` has to re-establish on every construction — which
/// it did, and which the test helpers went around.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Render {
    /// Wrapped and coloured, for a terminal.
    Human { color: bool },
    /// The markdown source, for piping somewhere that renders it.
    Markdown,
    /// One JSON object, for a tool.
    Json,
}

impl Render {
    pub fn color(self) -> bool {
        matches!(self, Render::Human { color: true })
    }
}

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

/// How much of a page to print.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Density {
    #[default]
    Full,
    /// Headings, examples, and the first sentence of each paragraph.
    Dense,
}

pub struct Presentation {
    pub width: Width,
    pub render: Render,
    pub density: Density,
}

/// One page, ready to print.
pub struct Page {
    pub id: String,
    pub title: String,
    pub kind: &'static str,
    /// Markdown. Rendering to a terminal happens once, in `emit`.
    pub body: String,
    pub see_also: Vec<String>,
}

/// One line of a listing: what `buri docs <id>` will fetch, and what it is.
///
/// Three positional `String`s used to say this, built by hand in six `entries`
/// implementations. Transposing `title` and `summary` in any of them would have
/// compiled and passed every test.
pub struct Entry {
    /// What `resolve` takes back. `every_registered_page_resolves` is what
    /// holds the two ends together.
    pub id: String,
    pub title: String,
    pub summary: String,
    /// Everything else on the page worth searching: a topic's prose, a
    /// module's `//!` text, a `///` comment's body below its first line.
    ///
    /// Search used to read this out of `topics::find`, which only prose has —
    /// so a query was matched against the *names* of nine hundred API pages
    /// and against the bodies of thirty-nine topics. `compare ints` therefore
    /// found `core/str.compare` and never `core/order`. It is a field rather
    /// than a lookup because only the source knows where a page's prose
    /// lives, and it hangs off `Entry` rather than off `resolve` so that
    /// indexing every page costs one pass and no rendering.
    pub text: String,
}

/// A kind of documentation the CLI can serve.
///
/// This is the extensibility seam. Implement it, add one line to `sources()`,
/// and the new kind appears in the index, in search, and in the manifest.
pub trait DocSource {
    fn kind(&self) -> &'static str;
    /// The page for an id, if this source owns it.
    fn resolve(&self, id: &str) -> Option<Page>;
    /// Everything this source serves.
    fn entries(&self) -> Vec<Entry>;
}

/// Prose topics: getting started, the guides, the language reference, and the
/// reference sections.
pub struct Prose;

impl DocSource for Prose {
    fn kind(&self) -> &'static str {
        "topic"
    }

    fn resolve(&self, id: &str) -> Option<Page> {
        let t = topics::find(id)?;
        Some(page_of(t))
    }

    fn entries(&self) -> Vec<Entry> {
        topics::TOPICS
            .iter()
            .map(|t| Entry {
                id: t.id.to_string(),
                title: t.title.to_string(),
                summary: t.summary(),
                text: t.text.to_string(),
            })
            .collect()
    }
}

fn page_of(t: &'static Topic) -> Page {
    Page {
        id: t.id.to_string(),
        title: t.title.to_string(),
        kind: t.kind.label(),
        body: t.text.to_string(),
        see_also: t.see_also.iter().map(|s| s.to_string()).collect(),
    }
}

/// The grammar and the build-file schemas: normative artifacts that are not
/// prose and not generated.
pub struct Normative;

const NORMATIVE: &[(&str, &str, &str)] = &[
    ("grammar", "The normative grammar", "ebnf"),
    ("schema/build", "The BUILD.buri schema", "proto"),
    ("schema/repo", "The REPO.buri schema", "proto"),
];

impl DocSource for Normative {
    fn kind(&self) -> &'static str {
        "normative"
    }

    fn resolve(&self, id: &str) -> Option<Page> {
        // The title and the language come from the same row the index lists, so
        // a page cannot be headed one thing and listed as another.
        let (_, title, lang) = NORMATIVE.iter().find(|(name, _, _)| *name == id)?;
        let text = normative_text(id)?;
        Some(Page {
            id: id.to_string(),
            title: title.to_string(),
            kind: "normative",
            body: format!("# {title}\n\n```{lang}\n{text}\n```\n"),
            see_also: Vec::new(),
        })
    }

    fn entries(&self) -> Vec<Entry> {
        NORMATIVE
            .iter()
            .map(|(id, title, lang)| Entry {
                id: (*id).to_string(),
                title: (*title).to_string(),
                summary: format!("hand-written {lang}, held to the implementation by a test"),
                text: normative_text(id).unwrap_or_default().to_string(),
            })
            .collect()
    }
}

/// The artifact behind a normative page. One lookup, so a page cannot be
/// served one text and indexed under another.
fn normative_text(id: &str) -> Option<&'static str> {
    match id {
        "grammar" => Some(topics::GRAMMAR),
        "schema/build" => Some(topics::BUILD_PROTO),
        "schema/repo" => Some(topics::REPO_PROTO),
        _ => None,
    }
}

/// The CLI reference, generated from the same table that dispatches.
///
/// The synopsis and the flag list are produced from `commands::COMMANDS`, so a
/// page cannot describe a flag the binary does not accept, nor omit one it
/// does. The prose beside them is the only hand-written part, and it is not
/// allowed to mention a flag — a test enforces that.
pub struct Cli;

impl DocSource for Cli {
    fn kind(&self) -> &'static str {
        "command"
    }

    fn resolve(&self, id: &str) -> Option<Page> {
        let name = id.strip_prefix("cli/")?;
        let c = crate::commands::find(name)?;
        Some(Page {
            id: format!("cli/{}", c.name),
            title: format!("buri {}", c.name),
            kind: "command",
            body: crate::commands::reference(c),
            see_also: vec!["build/repo-config".to_string()],
        })
    }

    fn entries(&self) -> Vec<Entry> {
        crate::commands::COMMANDS
            .iter()
            .map(|c| Entry {
                id: format!("cli/{}", c.name),
                title: format!("buri {}", c.name),
                summary: c.blurb.to_string(),
                text: crate::commands::reference(c),
            })
            .collect()
    }
}

/// The standard library reference, rendered from the modules the compiler
/// just checked.
///
/// Built on first use and kept, because loading and checking the whole
/// standard library costs a few milliseconds and `buri docs` may ask for
/// several pages in one run.
pub struct Std {
    modules: Vec<reference::ApiModule>,
}

impl Std {
    pub fn load() -> Std {
        let mut map = crate::diagnostics::SourceMap::new();
        let analysis = crate::compiler::driver::analyze_stdlib(&mut map);
        Std { modules: reference::from_loaded(&analysis.loaded, &reference::std_filter) }
    }
}

impl DocSource for Std {
    fn kind(&self) -> &'static str {
        "api"
    }

    fn resolve(&self, id: &str) -> Option<Page> {
        module_page(&self.modules, id, "standard library")
    }

    fn entries(&self) -> Vec<Entry> {
        module_entries(&self.modules)
    }
}

/// A module reference's page: the module itself, or one item in it.
///
/// The other half of what `Entry` bought — `Std` and `Workspace` resolve
/// identically and differ only in the one word a page says it came from.
fn module_page(modules: &[reference::ApiModule], id: &str, kind: &'static str) -> Option<Page> {
    if let Some(m) = modules.iter().find(|m| m.path == id) {
        return Some(Page {
            id: m.path.clone(),
            title: m.path.clone(),
            kind,
            body: reference::render(m),
            see_also: Vec::new(),
        });
    }
    let (m, item) = reference::find_item(modules, id)?;
    Some(Page {
        id: format!("{}.{}", m.path, item.name),
        title: format!("{}.{}", m.path, item.name),
        kind,
        body: reference::render_item(m, item),
        see_also: vec![m.path.clone()],
    })
}

/// A module reference's listing: the module itself, then each item in it.
///
/// One function rather than the two near-identical copies `Std` and
/// `Workspace` used to carry, which is the other thing an `Entry` buys — the
/// duplication was only tolerable while the rows were anonymous triples.
fn module_entries(modules: &[reference::ApiModule]) -> Vec<Entry> {
    let mut out = Vec::new();
    for m in modules {
        out.push(Entry {
            id: m.path.clone(),
            title: m.path.clone(),
            summary: m.docs.first().cloned().unwrap_or_default(),
            text: m.docs.join("\n"),
        });
        for item in &m.items {
            out.push(Entry {
                id: item.path(&m.path),
                title: format!("{} {}", item.kind().label(), item.name),
                summary: item.docs.first().cloned().unwrap_or_default(),
                text: item_text(item),
            });
        }
    }
    out
}

/// Everything written about one item: its own `///` comment, and the comments
/// on the fields, variants, or methods listed under it — which is what the
/// page shows, so it is what a search of the page has to see.
fn item_text(item: &reference::ApiItem) -> String {
    let mut text = item.docs.join("\n");
    for member in item.api.members() {
        let _ = write!(text, "\n{}\n{}", member.name, member.docs.join("\n"));
    }
    text
}

/// The packages of the repository you are standing in, rendered by the same
/// code that renders the standard library.
///
/// This is what makes the feature belong to the ecosystem rather than to this
/// toolchain: a third-party repository gets an API reference for its own
/// libraries, from its own `///` comments, with nothing to set up.
pub struct Workspace {
    modules: Vec<reference::ApiModule>,
}

impl Workspace {
    /// `None` outside a repository, which is not an error — most of the
    /// documentation does not need one.
    pub fn load() -> Option<Workspace> {
        let cwd = std::env::current_dir().ok()?;
        let root = crate::build::workspace::find_root(&cwd)?;
        let mut map = crate::diagnostics::SourceMap::new();
        let mut diagnostics = crate::diagnostics::Diagnostics::new();
        let workspace =
            crate::build::workspace::Workspace::load(&root, &mut map, &mut diagnostics).ok()?;

        // Every library in the repository, checked together, so a page shows
        // what an importer would actually see.
        let mut modules = Vec::new();
        let mut cache = crate::parsing::parser::Cache::new();
        for target in workspace.targets() {
            let unit = crate::compiler::modules::Unit {
                target: Some(target),
                // A reference page is not an output. See `Unit::platform`.
                platform: None,
                with_tests: false,
            };
            let analysis =
                crate::compiler::driver::analyze(Some(&workspace), &mut map, &mut cache, &unit);
            let package = target.package;
            let owned = |m: &crate::compiler::modules::ModuleData| m.pkg == Some(package);
            for m in reference::from_loaded(&analysis.loaded, &owned) {
                if !modules.iter().any(|e: &reference::ApiModule| e.path == m.path) {
                    modules.push(m);
                }
            }
        }
        modules.sort_by(|a, b| a.path.cmp(&b.path));
        Some(Workspace { modules })
    }
}

impl DocSource for Workspace {
    fn kind(&self) -> &'static str {
        "workspace"
    }

    fn resolve(&self, id: &str) -> Option<Page> {
        module_page(&self.modules, id, "this repository")
    }

    fn entries(&self) -> Vec<Entry> {
        module_entries(&self.modules)
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
    for (index, line) in page.body.lines().enumerate() {
        let number = index.saturating_add(1);
        if dropped.iter().any(|(first, last)| number >= *first && number <= *last) {
            continue;
        }
        let trimmed = line.trim_start();
        let delimiter = trimmed.starts_with("```");
        if delimiter {
            in_fence = !in_fence;
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
            let reproduction =
                f.lang == "buri" && f.info.as_ref().is_ok_and(|info| info.get("code").is_some());
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

/// The error catalog: one page per diagnostic code.
pub struct Errors;

impl DocSource for Errors {
    fn kind(&self) -> &'static str {
        "error"
    }

    fn resolve(&self, id: &str) -> Option<Page> {
        // Both `buri docs error/result-discarded` and, via `command_docs`, the
        // two-word `buri docs error result-discarded`.
        let code = id.strip_prefix("error/").unwrap_or(id);
        let e = crate::documentation::errors::find(code)?;
        let page = errors::page(code);
        // The guarantee every page in this catalog carries, written once here
        // rather than sixty times in the pages. It is the last thing a reader
        // sees, after the program it is about, in whichever way the page is
        // rendered — the alternative was a paragraph that had to be copied,
        // kept in step, and got the code wrong the moment one was renamed.
        let promise = if page.is_some_and(|p| !p.front.reproducible) {
            String::new()
        } else {
            format!(
                "\nThe program above is compiled by the test suite, which checks that it still \
                 produces `{code}` — so this page cannot describe an error the compiler has \
                 stopped emitting.\n"
            )
        };
        // The promise stays directly under the program it is about; the
        // wording comes last, as reference for whoever edits the page.
        let body = match page {
            Some(p) => {
                format!("{}\n{promise}{}{}", titled(p), wording(&p.front), attribution(&p.front))
            }
            None => format!("{}{promise}", e.text),
        };
        Some(Page {
            id: format!("error/{}", e.code),
            title: e.title().to_string(),
            kind: "error",
            body,
            see_also: e.see_also.iter().map(|s| (*s).to_string()).collect(),
        })
    }

    fn entries(&self) -> Vec<Entry> {
        crate::documentation::errors::ERRORS
            .iter()
            .map(|e| Entry {
                id: format!("error/{}", e.code),
                title: e.title().to_string(),
                summary: format!("`{}`", e.code),
                text: catalog_text(errors::page(e.code), e.text),
            })
            .collect()
    }
}

/// The page's body under its title.
///
/// Every page whose explanation is written opens with the title as an H1, and
/// the renderer takes the heading a reader sees from there rather than from
/// `Page::title`. A page that is frontmatter and a reproduction has no such
/// line, so it borrows the title the frontmatter already carries — otherwise
/// half the catalog opens on a bare wording table with nothing naming it.
fn titled(page: &frontmatter::Page) -> String {
    let text = page.body.trim();
    if text.starts_with("# ") {
        return text.to_string();
    }
    format!("# {}\n\n{text}", page.front.title).trim_end().to_string()
}

/// What a diagnostic built from this page prints, as the templates they are.
///
/// The frontmatter is never shown as prose — it is not prose, it is the
/// wording — so `buri docs error <code>` renders it as a table with the
/// placeholders left standing, which is what somebody editing the page needs
/// to see.
fn wording(front: &frontmatter::Frontmatter) -> String {
    let mut out = String::from("\n## The wording\n\n");
    let _ = writeln!(
        out,
        "What the diagnostic prints, from the page's frontmatter. A `{{name}}` is filled in \
         from the code the compiler was given.\n"
    );
    out.push_str("| Part | Template |\n|---|---|\n");
    let rows = [
        ("severity", Some(front.severity.label().to_string())),
        ("message", Some(front.message.clone())),
        ("label", front.label.clone()),
        ("note", front.note.clone()),
        ("fix", front.fix.clone()),
    ];
    for (name, value) in rows {
        if let Some(value) = value {
            // A `|` in a template would otherwise end the cell it is in.
            let _ = writeln!(out, "| {name} | {} |", value.replace('|', "\\|"));
        }
    }
    out
}

/// Who a borrowed body is borrowed from, as the last line of the page.
///
/// It is deliberately not part of the body: the body prints under a
/// diagnostic, and a reader looking at their own compile error is owed the
/// explanation and not this repository's paperwork. So the credit is a
/// frontmatter field, and it surfaces here, where somebody is reading the
/// page itself.
///
/// Plain prose rather than emphasised: the terminal renderer marks up code
/// spans and headings, and `*` around a paragraph would reach the reader as
/// two asterisks.
fn attribution(front: &frontmatter::Frontmatter) -> String {
    match &front.adapted_from {
        Some(source) => format!("\nAdapted from {source}.\n"),
        None => String::new(),
    }
}

/// The lint catalog: one page per `buri lint` finding.
pub struct Lints;

impl DocSource for Lints {
    fn kind(&self) -> &'static str {
        "lint"
    }

    fn resolve(&self, id: &str) -> Option<Page> {
        let code = id.strip_prefix("lint/").unwrap_or(id);
        let l = crate::documentation::lints::find(code)?;
        let body = match lints::page(code) {
            Some(p) => format!("{}\n{}{}", titled(p), wording(&p.front), attribution(&p.front)),
            None => l.text.to_string(),
        };
        Some(Page {
            id: format!("lint/{}", l.code),
            title: l.title().to_string(),
            kind: "lint",
            body,
            see_also: l.see_also.iter().map(|s| (*s).to_string()).collect(),
        })
    }

    fn entries(&self) -> Vec<Entry> {
        crate::documentation::lints::LINTS
            .iter()
            .map(|l| Entry {
                id: format!("lint/{}", l.code),
                title: l.title().to_string(),
                summary: format!("`{}`", l.code),
                text: catalog_text(lints::page(l.code), l.text),
            })
            .collect()
    }
}

/// What a catalog page says, for the index: the written explanation where
/// there is one, and the built-in wording where the page is frontmatter and a
/// reproduction. Not the wording table or the promise `resolve` adds — those
/// are the same sentences on all two hundred pages, and a sentence that is on
/// every page ranks nothing.
fn catalog_text(page: Option<&'static frontmatter::Page>, fallback: &str) -> String {
    match page {
        Some(p) => format!("{}\n{}", p.front.message, p.body),
        None => fallback.to_string(),
    }
}

/// Every kind of documentation, in the order the index lists them.
///
/// This is the seam: one line here and a new kind appears in the index, in
/// search, in the manifest, and under `--format=json`.
pub fn sources() -> Vec<Box<dyn DocSource>> {
    let mut out: Vec<Box<dyn DocSource>> = vec![
        Box::new(Prose),
        Box::new(Cli),
        Box::new(Std::load()),
        Box::new(Errors),
        Box::new(Lints),
    ];
    if let Some(workspace) = Workspace::load() {
        out.push(Box::new(workspace));
    }
    out.push(Box::new(Normative));
    out
}

// ---------------------------------------------------------------------------
// The command
// ---------------------------------------------------------------------------

pub fn command_docs(args: &arguments::Args) -> i32 {
    let presentation = Presentation {
        width: Width::new(terminal_width()),
        // `--format` decides whether there is a human to colour for at all, so
        // the colour flag is only asked about when there is one.
        render: match args.flags.format {
            Format::Json => Render::Json,
            Format::Markdown => Render::Markdown,
            Format::Human => Render::Human {
                color: args
                    .flags
                    .color
                    .unwrap_or_else(|| std::env::var("NO_COLOR").is_err()),
            },
        },
        density: if args.flags.dense { Density::Dense } else { Density::Full },
    };

    let mut rest = args.targets.iter().map(String::as_str);
    let Some(first) = rest.next() else {
        arguments::out(&index(&presentation));
        return 0;
    };

    match first {
        "search" => {
            let query: Vec<&str> = rest.collect();
            if query.is_empty() {
                eprintln!("error: `buri docs search` takes something to search for");
                return 2;
            }
            search(&query.join(" "), &presentation)
        }
        "assemble" => command_assemble(args.flags.check),
        "test" => doctest_command(&rest.collect::<Vec<_>>(), &presentation),
        "manifest" => {
            arguments::out(&manifest());
            0
        }
        "list" => {
            arguments::out(&index(&presentation));
            0
        }
        // `buri docs cli build` reads better than `buri docs cli/build`, and
        // it is what somebody types. Both work.
        "error" => match rest.next() {
            Some(code) => show(&format!("error/{code}"), &presentation),
            None => {
                arguments::out(&error_index(&presentation));
                0
            }
        },
        "lint" => match rest.next() {
            Some(code) => show(&format!("lint/{code}"), &presentation),
            None => {
                arguments::out(&lint_index(&presentation));
                0
            }
        },
        "cli" => match rest.next() {
            Some(name) => show(&format!("cli/{name}"), &presentation),
            None => {
                arguments::out(&command_index(&presentation));
                0
            }
        },
        id => show(id, &presentation),
    }
}

/// Every lint code, for `buri docs lint` with no argument.
fn lint_index(presentation: &Presentation) -> String {
    let mut out = String::new();
    let (bold, dim, reset) = markdown::emphasis(presentation.render.color());
    let _ = write!(out, "{bold}buri docs lint <code>{reset} — one lint in full\n\n");
    for l in crate::documentation::lints::LINTS {
        let _ = writeln!(out, "  {:<28} {}", l.code, l.title());
    }
    let _ = write!(
        out,
        "\n{dim}A lint is what `buri lint` reports: a rule about a program that type checks\n\
         and is still a mistake. Every finding names its code, and every code has a page.\
         {reset}\n"
    );
    out
}

/// Every diagnostic code, for `buri docs error` with no argument.
fn error_index(presentation: &Presentation) -> String {
    let mut out = String::new();
    let (bold, dim, reset) = markdown::emphasis(presentation.render.color());
    let _ = write!(out, "{bold}buri docs error <code>{reset} — one diagnostic in full\n\n");
    for e in crate::documentation::errors::ERRORS {
        let _ = writeln!(out, "  {:<28} {}", e.code, e.title());
    }
    let _ = write!(
        out,
        "\n{dim}Every code is printed in the diagnostic itself, in brackets after the\n\
         message. Where one file can provoke a code, its page carries that program,\n\
         and the test suite checks that it still does.{reset}\n"
    );
    out
}

/// Every command, for `buri docs cli` with no argument.
fn command_index(presentation: &Presentation) -> String {
    let mut out = String::new();
    let (bold, dim, reset) = markdown::emphasis(presentation.render.color());
    let _ = write!(out, "{bold}buri docs cli <command>{reset} — one command in full\n\n");
    for c in crate::commands::COMMANDS {
        let _ = writeln!(out, "  cli/{:<10} {}", c.name, c.blurb);
    }
    let _ = write!(out, "\n{dim}The synopsis and flag table on each page are generated from the\n\
                          same table that dispatches, so they cannot drift.{reset}\n");
    out
}

fn show(id: &str, presentation: &Presentation) -> i32 {
    // One set of sources, not two: `sources()` loads and analyses the standard
    // library and the repository, and the "did you mean" below was doing the
    // whole of that a second time on the one path where the answer is already
    // known to be a miss.
    let sources = sources();
    for source in &sources {
        if let Some(page) = source.resolve(id) {
            arguments::out(&emit(&page, presentation));
            return 0;
        }
    }
    eprintln!("error: there is no documentation topic `{id}`");
    let all: Vec<Entry> = sources.iter().flat_map(|s| s.entries()).collect();
    let ids: Vec<&str> = all.iter().map(|e| e.id.as_str()).collect();
    if let Some(near) = crate::build::buildfile::nearest(id, &ids) {
        eprintln!("  = did you mean `{near}`?");
    }
    eprintln!("  = `buri docs` lists every topic; `buri docs search <words>` looks inside them");
    2
}

fn emit(page: &Page, presentation: &Presentation) -> String {
    let body = match presentation.density {
        Density::Full => page.body.clone(),
        Density::Dense => markdown::dense(&page.body),
    };
    match presentation.render {
        Render::Markdown => body,
        Render::Json => {
            let mut out = String::new();
            let s = crate::diagnostics::json_str;
            let _ = write!(
                out,
                "{{\"id\":{},\"title\":{},\"kind\":{},\"body\":{}",
                s(&page.id),
                s(&page.title),
                s(page.kind),
                s(&body)
            );
            if !page.see_also.is_empty() {
                let list: Vec<String> = page.see_also.iter().map(|x| s(x)).collect();
                let _ = write!(out, ",\"seeAlso\":[{}]", list.join(","));
            }
            out.push_str("}\n");
            out
        }
        Render::Human { color } => {
            let mut out = markdown::to_terminal(&body, presentation.width, color);
            if !page.see_also.is_empty() {
                let _ = write!(out, "\nSee also: {}\n", page.see_also.join(", "));
            }
            if let Some(doc) = topics::find(&page.id).and_then(assemble::document_of) {
                let _ = write!(out, "\nPart of {}.\n", doc.path);
            }
            out
        }
    }
}

/// The front page: what kinds exist, what is in each, and how to search.
fn index(presentation: &Presentation) -> String {
    if presentation.render == Render::Json {
        let s = crate::diagnostics::json_str;
        let mut rows = Vec::new();
        for source in sources() {
            for e in source.entries() {
                rows.push(format!(
                    "{{\"kind\":{},\"id\":{},\"title\":{},\"summary\":{}}}",
                    s(source.kind()),
                    s(&e.id),
                    s(&e.title),
                    s(&e.summary)
                ));
            }
        }
        return format!("{{\"topics\":[{}]}}\n", rows.join(","));
    }

    let mut out = String::new();
    let (bold, dim, reset) = markdown::emphasis(presentation.render.color());
    let _ = writeln!(out, "{bold}buri docs{reset} — the language, the build system, and this CLI\n");

    // Diátaxis, in the order a reader needs it: learn, then do, then the
    // normative text, then the material nobody has to read front to back.
    for (kinds, heading) in [
        (&[Kind::GettingStarted][..], "Getting started"),
        (&[Kind::Guide][..], "Guides"),
        (&[Kind::Language][..], "The language"),
        (&[Kind::Reference, Kind::Build][..], "Reference"),
    ] {
        let _ = writeln!(out, "{bold}{heading}{reset}");
        for t in topics::TOPICS.iter().filter(|t| kinds.contains(&t.kind)) {
            let _ = writeln!(out, "  {:<30} {}", t.id, t.title);
        }
        out.push('\n');
    }

    let _ = writeln!(out, "{bold}This CLI{reset}");
    for c in crate::commands::COMMANDS.iter().filter(|c| !c.hidden) {
        let _ = writeln!(out, "  cli/{:<26} {}", c.name, c.blurb);
    }
    out.push('\n');

    let _ = writeln!(
        out,
        "{bold}Diagnostics{reset}\n  {} codes — `buri docs error <code>`, or `buri docs error` to list them",
        crate::documentation::errors::ERRORS.len()
    );
    let lints = crate::documentation::lints::LINTS.len();
    if lints > 0 {
        let _ = writeln!(
            out,
            "  {lints} lints — `buri docs lint <code>`, or `buri docs lint` to list them"
        );
    }
    out.push('\n');

    let _ = writeln!(out, "{bold}The standard library{reset}");
    let mut line = String::from("  ");
    for m in crate::compiler::standard_library::MODULES {
        let path = m.path;
        if line.chars().count() + path.len() + 2 > presentation.width.get() {
            let _ = writeln!(out, "{line}");
            line = String::from("  ");
        }
        let _ = write!(line, "{path}  ");
    }
    let _ = writeln!(out, "{}", line.trim_end());
    let _ = writeln!(
        out,
        "  {dim}…and every item in them: `buri docs core/list.map`{reset}\n"
    );

    let _ = writeln!(out, "{bold}Normative artifacts{reset}");
    for (id, title, _) in NORMATIVE {
        let _ = writeln!(out, "  {id:<30} {title}");
    }

    let _ = writeln!(
        out,
        "\n{dim}buri docs <topic>              read one\n\
         buri docs search <words>       by name or by intent; each hit is a command\n\
         buri docs <topic> --format=json    structured, for tools\n\
         buri docs <topic> --dense          headings and examples only\n\
         buri docs manifest             every id and shape, for an agent{reset}"
    );
    out
}

/// Words that carry no signal in a query like "how do I read a file". Without
/// this, a natural-language question ranks by its filler.
const STOPWORDS: &[&str] = &[
    "a", "an", "and", "are", "as", "at", "be", "by", "can", "do", "does", "for", "from", "how",
    "i", "if", "in", "is", "it", "of", "on", "or", "that", "the", "to", "use", "what", "when",
    "where", "which", "why", "with", "you",
];

/// What somebody calls a thing, against the page that answers them.
///
/// Search is otherwise name-shaped, and a name is what a reader who already
/// knows the answer would type. `compare ints` returned `core/str.compare` and
/// `core/proto.packedVarints` and never `core/order`, because the page that
/// answers it does not contain the word "compare" — the function is called
/// `int`, and its comment says "comparator". Every row here is a question
/// somebody asked in a transcript, or the same question one synonym over.
///
/// It is deliberately small, and it is a nudge and not a redirect: a row lifts
/// a module above the noise, it does not outrank a page that matches by name.
/// A word belongs here only when it is what a reader arrives with *and* is not
/// already in the page's title or prose — everything else is the index's job,
/// and a thesaurus would need maintaining forever.
const CONCEPTS: &[(&[&str], &[&str])] = &[
    (
        &["compare", "comparator", "comparison", "sort", "sorting", "ordering", "tiebreak"],
        &["core/order", "core/list"],
    ),
    (&["pad", "padding", "align", "alignment", "justify", "column"], &["core/str"]),
    (&["hex", "hexadecimal", "base16", "nibble"], &["core/bytes", "core/char"]),
    (&["random", "seed", "seeded", "rng", "shuffle"], &["core/random"]),
    (&["discard", "ignore", "unused", "unhandled"], &["core/result"]),
    (&["fixture", "fake", "mock", "stub", "double", "harness"], &["core/host/testing"]),
    (&["assert", "assertion", "expect"], &["core/testing/assert"]),
    (&["file", "filesystem", "directory", "path"], &["core/fs"]),
    (&["print", "println", "log", "stdout", "output"], &["core/io"]),
    (&["dictionary", "hashmap", "lookup", "keyed"], &["core/map", "core/ordmap"]),
    (&["clock", "timestamp", "duration", "elapsed"], &["core/time", "core/date"]),
    (&["encode", "decode", "serialize", "parse"], &["core/json", "core/proto"]),
    (&["boolean", "predicate", "truthy"], &["core/bool"]),
    (&["concurrency", "parallel", "thread", "spawn"], &["core/tasks", "core/actor"]),
];

/// One query word and the other spellings that mean it.
///
/// A term scores as the best of its spellings rather than as the sum, so
/// "ints" is one word matching `core/order.int` and not two.
struct Term {
    spellings: Vec<String>,
}

/// A query, read once: what was typed, the words that carry signal, and the
/// pages the concepts behind those words point at.
struct Query {
    /// The whole query, lowercased. A multi-word query is usually a phrase.
    phrase: String,
    terms: Vec<Term>,
    /// Page ids `CONCEPTS` matched, deduplicated. Empty for most queries.
    concepts: Vec<&'static str>,
}

impl Query {
    fn new(query: &str) -> Query {
        let phrase = query.to_lowercase();
        let all: Vec<&str> = phrase.split_whitespace().collect();
        let kept: Vec<&str> = all.iter().copied().filter(|w| !STOPWORDS.contains(w)).collect();
        // If the query is nothing but filler, search it as written rather than
        // searching nothing.
        let words: Vec<&str> = if kept.is_empty() { all } else { kept };

        let mut concepts: Vec<&'static str> = Vec::new();
        let terms: Vec<Term> = words
            .iter()
            .map(|w| {
                for (names, pages) in CONCEPTS {
                    if names.contains(w) {
                        for page in *pages {
                            if !concepts.contains(page) {
                                concepts.push(page);
                            }
                        }
                    }
                }
                Term { spellings: spellings_of(w) }
            })
            .collect();
        Query { phrase, terms, concepts }
    }
}

/// A word and the singular behind it.
///
/// The whole of the stemming, and as much as this deserves: somebody types
/// "compare ints" and the function is `order.int`, somebody types "bytes" and
/// the module is `core/bytes`. A four-letter floor keeps "as"/"is" and the
/// short type names out of it, and a doubled `s` ("address") is left alone.
fn spellings_of(word: &str) -> Vec<String> {
    let mut out = vec![word.to_string()];
    if let Some(stem) = word.strip_suffix('s') {
        if stem.len() >= 3 && !stem.ends_with('s') {
            out.push(stem.to_string());
        }
    }
    out
}

/// One page as the index sees it.
struct Doc<'a> {
    id: &'a str,
    title: &'a str,
    tags: &'a [&'a str],
    /// The page's prose: `Entry::text`.
    text: &'a str,
}

/// The words in a name: `core/order.int` is `core`, `order`, `int`, and
/// `padStart` is `pad`, `start`.
///
/// A name is not prose — it is separators and camel case — and matching it
/// with `contains` is how `core/proto.packedVarints` came back for "compare
/// ints". A word in a query means a word in a name.
fn name_words(name: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut word = String::new();
    for c in name.chars() {
        if !c.is_alphanumeric() {
            // A separator ends a word.
        } else if c.is_uppercase() && word.chars().next_back().is_some_and(char::is_lowercase) {
            // …and so does the hump in the middle of `padStart`.
        } else {
            word.push(c.to_ascii_lowercase());
            continue;
        }
        if !word.is_empty() {
            out.push(std::mem::take(&mut word));
        }
        if c.is_alphanumeric() {
            word.push(c.to_ascii_lowercase());
        }
    }
    if !word.is_empty() {
        out.push(word);
    }
    out
}

/// How many words of `text` begin with `needle`.
///
/// At a word boundary, so "int" is `integer` and not `varint`, `print` or
/// `lint` — the substring count this replaced gave a varint encoder eight
/// points for a query about integers, which is most of why the answer to
/// "compare ints" was a page about protobuf. A prefix rather than a whole
/// word, because "align" is written `alignment` and "compare" `comparator`,
/// and a reader is owed those.
fn word_hits(text: &str, needle: &str) -> usize {
    text.split(|c: char| !c.is_alphanumeric()).filter(|word| word.starts_with(needle)).count()
}

/// One page's score against one query.
///
/// `search` and the test that asserts what a query finds both go through this.
/// The test used to carry its own copy of the weights, which made it a scorer
/// test that scored nothing: changing 60 to 6 here could not fail it.
fn score(query: &Query, doc: &Doc) -> i64 {
    let id = doc.id.to_lowercase();
    // The last segment of an id is what the thing is called: `int` in
    // `core/order.int`, `effects` in `language/effects`. Somebody searching for
    // a name types the name, not the path it is under.
    let name = id.rsplit(['.', '/']).next().unwrap_or(&id).to_string();
    let id_words = name_words(doc.id);
    let title_words = name_words(doc.title);
    let title = doc.title.to_lowercase();
    let text = doc.text.to_lowercase();
    let mut score = 0i64;
    // A multi-word query is usually a phrase — "tail call" means the section
    // about tail calls, not every page that says "call". Score the phrase
    // first and heavily, or `language/expressions` wins every query containing a
    // common word.
    if query.terms.len() > 1 {
        if title.contains(&query.phrase) {
            score += 60;
        }
        score += (text.matches(query.phrase.as_str()).count() as i64 * 12).min(60);
    }
    for term in &query.terms {
        let mut best = 0i64;
        for w in &term.spellings {
            let w = w.as_str();
            let mut this = 0i64;
            if id == w || title == w || name == w {
                this += 100;
            }
            // A query that spells a path is matched against the path — that is
            // somebody pasting `core/list` back — and a query that spells a
            // word against the words of the name.
            let (named, titled) = if w.contains(['/', '.']) {
                (id.contains(w), title.contains(w))
            } else {
                (id_words.iter().any(|x| x == w), title_words.iter().any(|x| x == w))
            };
            if named {
                this += 20;
            }
            if titled {
                this += 15;
            }
            if doc.tags.iter().any(|x| x.to_lowercase() == w) {
                this += 12;
            }
            this += (word_hits(&text, w) as i64).min(8);
            best = best.max(this);
        }
        score += best;
    }
    // The intent half. A concept lifts the module it names above the pages
    // that merely say the word — that is the whole point of the row, since the
    // module does not say it — but never above a page that matched by name.
    // Inside that module it lifts only what already matched: a boost given to
    // every item of `core/str` for "pad" buried nine other pages under the
    // ones that were not `padStart`.
    for target in &query.concepts {
        if id == *target {
            score += 40;
        } else if score > 0 && id.starts_with(&format!("{target}.")) {
            score += 10;
        }
    }
    score
}

/// One result: the page, and the command that fetches it.
struct Hit {
    score: i64,
    id: String,
    title: String,
    summary: String,
}

impl Hit {
    /// What to run to read the page. Every id in the index is a `buri docs`
    /// argument — `every_manifest_id_is_fetchable` is what says so — which is
    /// why a result can promise one.
    fn command(&self) -> String {
        format!("buri docs {}", self.id)
    }
}

/// Search across every registered page — by name, by the prose inside it, and
/// by the concept a word stands for — ranked by where the match landed.
/// Deliberately simple and deliberately deterministic: ties break on the id,
/// never on hash order.
fn ranked(query: &Query) -> Vec<Hit> {
    let mut hits: Vec<Hit> = Vec::new();
    for source in sources() {
        for Entry { id, title, summary, text } in source.entries() {
            let tags: Vec<&str> = topics::find(&id).map(|t| t.tags.to_vec()).unwrap_or_default();
            let score = score(query, &Doc { id: &id, title: &title, tags: &tags, text: &text });
            if score > 0 {
                hits.push(Hit { score, id, title, summary });
            }
        }
    }
    // One line per page. An id is not unique in the index — `failsOnCall` is a
    // method of three test doubles and `read` of four handles, each its own
    // entry under the one id — and a result is a command to run, so the same
    // command three times is two lines that answer nothing.
    hits.sort_by(|a, b| a.id.cmp(&b.id).then_with(|| b.score.cmp(&a.score)));
    hits.dedup_by(|a, b| a.id == b.id);
    // Highest score first, then by id, so two runs agree.
    hits.sort_by(|a, b| b.score.cmp(&a.score).then_with(|| a.id.cmp(&b.id)));
    // A question is answered by a handful of pages, not by a screenful.
    hits.truncate(12);
    hits
}

/// The command: rank, then print one line per page — the command that reads
/// it, and what it is.
fn search(query: &str, presentation: &Presentation) -> i32 {
    let query = Query::new(query.trim());
    let hits = ranked(&query);

    if presentation.render == Render::Json {
        let s = crate::diagnostics::json_str;
        let rows: Vec<String> = hits
            .iter()
            .map(|hit| {
                format!(
                    "{{\"id\":{},\"title\":{},\"summary\":{},\"command\":{},\"score\":{}}}",
                    s(&hit.id),
                    s(&hit.title),
                    s(&hit.summary),
                    s(&hit.command()),
                    hit.score
                )
            })
            .collect();
        arguments::out(&format!(
            "{{\"query\":{},\"hits\":[{}]}}\n",
            s(&query.phrase),
            rows.join(",")
        ));
        return if hits.is_empty() { 1 } else { 0 };
    }

    if hits.is_empty() {
        eprintln!("error: nothing matches `{}`", query.phrase);
        eprintln!("  = `buri docs` lists every topic");
        return 1;
    }
    let (bold, dim, reset) = markdown::emphasis(presentation.render.color());
    let mut listing = String::new();
    // The command rather than the id, because a result is only useful once it
    // has been read, and an id is one transcription away from being a command.
    // A question is rarely answered by one page, so several are offered and
    // each one says how to open it.
    for hit in &hits {
        let _ = writeln!(listing, "{bold}{}{reset}  {}", hit.command(), hit.title);
        let line = one_line(&hit.summary, presentation.width.get().saturating_sub(4));
        if !line.is_empty() {
            let _ = writeln!(listing, "  {dim}{line}{reset}");
        }
    }
    let _ = writeln!(listing, "\n{dim}Run any line above to read that page.{reset}");
    arguments::out(&listing);
    0
}

/// A summary as one terminal line: inline markup rendered away, whitespace
/// collapsed, truncated on a word boundary.
fn one_line(text: &str, width: usize) -> String {
    // Wrapped and then unwrapped: the collapse below turns the line breaks
    // back into spaces, and what is wanted is the inline markup rendered away.
    let flat = markdown::to_terminal(text, Width::widest(), false);
    let flat = flat.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= width {
        return flat;
    }
    let mut out = String::new();
    for word in flat.split(' ') {
        if out.chars().count() + word.chars().count() + 1 > width.saturating_sub(1) {
            break;
        }
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(word);
    }
    out.push('…');
    out
}

/// Everything an agent needs to drive `buri docs` without guessing: the ids it
/// can fetch, the shapes it will get back, and what the exit codes mean.
///
/// Generated wholly from the registries, so it cannot describe a page that
/// does not exist — and `every_manifest_id_is_fetchable` checks the converse.
fn manifest() -> String {
    let s = crate::diagnostics::json_str;
    let mut out = String::new();
    let _ = write!(
        out,
        "{{\"tool\":\"buri\",\"version\":{},\"docsVersion\":1",
        s(crate::commands::arguments::VERSION)
    );

    let mut kinds: Vec<String> = Vec::new();
    let mut pages: Vec<String> = Vec::new();
    for source in sources() {
        kinds.push(s(source.kind()));
        for Entry { id, title, summary, .. } in source.entries() {
            let tags = topics::find(&id)
                .map(|t| t.tags.iter().map(|x| s(x)).collect::<Vec<_>>())
                .unwrap_or_default();
            let _ = write!(
                pages_push(&mut pages),
                "{{\"kind\":{},\"id\":{},\"title\":{},\"summary\":{},\"tags\":[{}]}}",
                s(source.kind()),
                s(&id),
                s(&title),
                s(&summary),
                tags.join(",")
            );
        }
    }
    let _ = write!(out, ",\"docKinds\":[{}]", kinds.join(","));
    let _ = write!(out, ",\"pages\":[{}]", pages.join(","));
    let _ = write!(
        out,
        ",\"howTo\":{{\"read\":\"buri docs <id> --format=json\",\
         \"search\":\"buri docs search <words> --format=json\",\
         \"list\":\"buri docs --format=json\",\
         \"compact\":\"add --dense to drop prose but keep every example\"}}"
    );
    let _ = write!(
        out,
        ",\"exitCodes\":{{\"0\":\"success\",\
         \"1\":\"the thing you asked about is wrong\",\
         \"2\":\"the thing you asked with is wrong\"}}"
    );
    out.push_str("}\n");
    out
}

/// Appends a slot and hands back a writer for it, so the loop above can use
/// `write!` without building an intermediate `String` per page.
fn pages_push(pages: &mut Vec<String>) -> &mut String {
    pages.push(String::new());
    pages.last_mut().or_ice("a vector has a last element on the line after a push")
}

/// Compiles every fenced example in the named markdown files — or, with no
/// argument, in every markdown file of the repository you are standing in.
///
/// This is the whole documentation-testing apparatus, pointed at somebody
/// else's documentation. A repository gets `wrap=body`, `use=`, `// ERROR:`,
/// `run` blocks with pinned output, and the `repo=` key that compiles an
/// example against its own packages — with nothing to configure.
fn doctest_command(paths: &[&str], presentation: &Presentation) -> i32 {
    let cwd = match std::env::current_dir() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };
    let root = crate::build::workspace::find_root(&cwd)
        .or_else(|| repo_root_of(&cwd))
        .unwrap_or(cwd.clone());

    let files: Vec<std::path::PathBuf> = if paths.is_empty() {
        let mut found = Vec::new();
        markdown_under(&root, &mut found);
        found
    } else {
        paths.iter().map(|p| root.join(p)).collect()
    };
    if files.is_empty() {
        eprintln!("error: no markdown files to check");
        eprintln!("  = name some, or run this where the repository's documentation is");
        return 2;
    }

    let mut failures = Vec::new();
    let mut blocks = 0usize;
    let mut checked = 0usize;
    for file in &files {
        let Ok(raw) = std::fs::read_to_string(file) else { continue };
        let rel = file.strip_prefix(&root).unwrap_or(file).display().to_string();
        // A source file is read through its documentation comments, which come
        // back as a document with the source's own line numbers — so a failure
        // points at the `.buri` line the example is written on.
        let source = rel.ends_with(".buri");
        if source && !crate::documentation::examples::has_examples(&raw) {
            continue;
        }
        let text = if source { crate::documentation::examples::doc_comments(&raw) } else { raw };
        let found = crate::documentation::examples::extract(&rel, &text)
            .blocks
            .iter()
            .filter(|b| !b.claim.is_ignored())
            .count();
        if source && found == 0 {
            continue;
        }
        checked += 1;
        blocks += found;
        failures.extend(crate::documentation::examples::run_file_in_repo(&root, &rel, &text));
    }
    let files_checked = checked;

    if failures.is_empty() {
        let (_, dim, reset) = markdown::emphasis(presentation.render.color());
        arguments::out(&format!(
            "{blocks} example(s) in {files_checked} document(s) compile{dim} — and the ones \
             that print something were run{reset}\n"
        ));
        return 0;
    }
    eprint!("{}", crate::documentation::examples::report(&failures));
    eprintln!("{} example(s) do not do what the documentation says", failures.len());
    1
}

/// Every `.md` in the tree, skipping the build directory and anything hidden.
fn markdown_under(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mut entries: Vec<_> = entries.filter_map(Result::ok).collect();
    entries.sort_by_key(|e| e.path());
    for e in entries {
        let p = e.path();
        let name = p.file_name().unwrap_or_default().to_string_lossy().to_string();
        if name.starts_with('.') || name == "target" || name == "node_modules" {
            continue;
        }
        if p.is_dir() {
            markdown_under(&p, out);
        } else if p.extension().is_some_and(|x| x == "md" || x == "buri") {
            // `.buri` too: a documentation comment is documentation, and an
            // example in one has the same claim on being true as an example in
            // a prose page. `BUILD.buri` and `REPO.buri` wear the extension but
            // are textproto, and have no doc comments to find.
            if p.file_name().is_some_and(|n| n == "BUILD.buri" || n == "REPO.buri") {
                continue;
            }
            out.push(p);
        }
    }
}

fn command_assemble(check_only: bool) -> i32 {
    let cwd = match std::env::current_dir() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };
    let Some(root) = crate::build::workspace::find_root(&cwd).or_else(|| repo_root_of(&cwd)) else {
        eprintln!("error: `buri docs assemble` must run inside the Buri repository");
        return 2;
    };

    let drifted = assemble::drifted(&root);
    if check_only {
        if drifted.is_empty() {
            println!("no drift");
            return 0;
        }
        for (path, _) in &drifted {
            eprintln!("error: {path} is not what its topics assemble to");
        }
        eprintln!("  = run `buri docs assemble` to regenerate it");
        eprintln!("  = edit the topics under cli/src/docs, never the assembled file");
        return 1;
    }
    match assemble::write(&root) {
        Ok(changed) if changed.is_empty() => {
            println!("no drift");
            0
        }
        Ok(changed) => {
            for path in changed {
                println!("wrote {path}");
            }
            0
        }
        Err(e) => {
            eprintln!("error: {e}");
            2
        }
    }
}

/// The toolchain's own checkout, found by walking up to the directory holding
/// `cli/src/docs/`. `find_root` looks for `REPO.buri`, which this repository
/// does not have at its root.
fn repo_root_of(from: &std::path::Path) -> Option<std::path::PathBuf> {
    let mut dir = Some(from);
    while let Some(d) = dir {
        if d.join("cli/src/docs/SPEC.md").is_file() && d.join("cli/src/docs/language").is_dir() {
            return Some(d.to_path_buf());
        }
        dir = d.parent();
    }
    None
}

/// `$COLUMNS`, or eighty. Public because a diagnostic's explanation is wrapped
/// to the same width as a documentation page, being the same text.
pub fn terminal_width() -> usize {
    std::env::var("COLUMNS").ok().and_then(|c| c.parse().ok()).unwrap_or(80)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn presentation() -> Presentation {
        Presentation {
            width: Width::default(),
            render: Render::Human { color: false },
            density: Density::Full,
        }
    }

    #[test]
    fn every_registered_page_resolves() {
        for source in sources() {
            for Entry { id, .. } in source.entries() {
                assert!(
                    source.resolve(&id).is_some(),
                    "`{id}` is listed by `{}` but does not resolve",
                    source.kind()
                );
            }
        }
    }

    #[test]
    fn the_index_lists_every_topic() {
        let text = index(&presentation());
        for t in topics::TOPICS {
            assert!(text.contains(t.id), "the index omits `{}`", t.id);
        }
    }

    #[test]
    fn a_page_renders_in_every_style() {
        let page = Prose.resolve("language/effects").expect("language/effects exists");
        for render in [Render::Human { color: false }, Render::Markdown, Render::Json] {
            let out = emit(&page, &Presentation { render, ..presentation() });
            assert!(!out.trim().is_empty(), "{render:?} rendered nothing");
        }
        // JSON is one line, so a tool can read it a page at a time.
        let json = emit(&page, &Presentation { render: Render::Json, ..presentation() });
        assert_eq!(json.lines().count(), 1, "JSON output must be one line");
    }

    #[test]
    fn dense_is_shorter_but_keeps_the_examples() {
        let page = Prose.resolve("language/effects").unwrap();
        let full = emit(&page, &Presentation { render: Render::Markdown, ..presentation() });
        let dense = emit(
            &page,
            &Presentation { render: Render::Markdown, density: Density::Dense, ..presentation() },
        );
        assert!(dense.len() < full.len());
        for f in markdown::fences(&page.body) {
            if f.body.trim().is_empty() {
                continue;
            }
            assert!(dense.contains(f.body.trim()), "dense dropped an example");
        }
    }

    #[test]
    fn search_finds_the_obvious_things() {
        for (query, want) in [
            ("effects", "language/effects"),
            ("ctx", "language/effects"),
            ("exhaustive", "language/patterns"),
            ("tags", "build/tags"),
            ("cache", "build/hermeticity"),
            ("tail call", "language/evaluation"),
            ("public surface", "build/libraries"),
        ] {
            let q = Query::new(query);
            let mut best: Option<(i64, String)> = None;
            // `super::score` rather than a second copy of the weights: this
            // asserts what the shipped scorer ranks, so a changed weight fails
            // here rather than passing quietly.
            for t in topics::TOPICS {
                let doc = Doc { id: t.id, title: t.title, tags: t.tags, text: t.text };
                let score = score(&q, &doc);
                if score > 0 && best.as_ref().is_none_or(|(b, _)| score > *b) {
                    best = Some((score, t.id.to_string()));
                }
            }
            assert_eq!(
                best.map(|(_, id)| id).as_deref(),
                Some(want),
                "searching `{query}` should find `{want}`"
            );
        }
    }

    /// The queries the intent half was built for, each one an incident: an
    /// agent hand-wrote a comparator, copied a clock fixture, and aligned
    /// columns with tabs, with every one of these pages already shipping.
    ///
    /// A search answers with pages, plural, so what is asserted is that the
    /// page is *among* what came back — not that it is first. Ranking one
    /// page over another is a judgement; leaving the answer out is a bug.
    #[test]
    fn search_is_intent_shaped_as_well_as_name_shaped() {
        for (query, want) in [
            ("compare ints", "core/order"),
            ("compare ints", "core/order.int"),
            ("sort a list", "core/order"),
            ("fixture", "core/host/testing"),
            ("pad", "core/str.padStart"),
            ("hex", "core/bytes.toHex"),
            ("ignore a result", "core/result"),
            ("seeded random", "core/random"),
        ] {
            let found: Vec<String> =
                ranked(&Query::new(query)).into_iter().map(|hit| hit.id).collect();
            assert!(
                found.iter().any(|id| id == want),
                "searching `{query}` should return `{want}`, got {found:?}"
            );
        }
    }

    /// Every result is something to run, and every alias names something that
    /// is there. A concept pointing at a page that has been renamed would
    /// otherwise be silent — it would boost nothing, and search would quietly
    /// go back to being name-shaped.
    #[test]
    fn every_result_is_a_command_and_every_concept_a_page() {
        let sources = sources();
        let ids: Vec<String> =
            sources.iter().flat_map(|s| s.entries()).map(|e| e.id).collect();
        for (words, pages) in CONCEPTS {
            for page in *pages {
                assert!(
                    ids.iter().any(|id| id == page),
                    "the concept {words:?} points at `{page}`, which is not a page"
                );
            }
        }
        for hit in ranked(&Query::new("compare ints")) {
            assert_eq!(hit.command(), format!("buri docs {}", hit.id));
            assert!(
                sources.iter().any(|s| s.resolve(&hit.id).is_some()),
                "`{}` is offered by search and does not resolve",
                hit.command()
            );
        }
    }
}
