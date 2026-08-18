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
//!     are compiled in, so `buri docs lang/effects` answers in an empty
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
pub mod grammar;
pub mod harness;
pub mod markdown;
pub mod reference;
pub mod topics;

use crate::commands::arguments;
use crate::diagnostics::Invariant as _;
use crate::documentation::topics::{Kind, Topic};
use std::fmt::Write as _;

/// How a page is rendered.
///
/// Colour lives inside `Human`, for the reason `build::session::Rendering`
/// gives: an escape sequence in a JSON stream corrupts it, so "colour, in
/// JSON" should be a thing that cannot be written down rather than a
/// correlation `cmd_docs` has to re-establish on every construction — which it
/// did, and which the test helpers went around.
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
    pub const NARROWEST: usize = 40;
    pub const WIDEST: usize = 100;

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

pub struct DocCtx {
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

/// Prose topics: the language reference, the build system, the guide.
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
        let text = match id {
            "grammar" => topics::GRAMMAR,
            "schema/build" => topics::BUILD_PROTO,
            "schema/repo" => topics::REPO_PROTO,
            _ => return None,
        };
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
                id: id.to_string(),
                title: title.to_string(),
                summary: format!("hand-written {lang}, held to the implementation by a test"),
            })
            .collect()
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
            see_also: vec!["build/cli".to_string()],
        })
    }

    fn entries(&self) -> Vec<Entry> {
        crate::commands::COMMANDS
            .iter()
            .map(|c| Entry {
                id: format!("cli/{}", c.name),
                title: format!("buri {}", c.name),
                summary: c.blurb.to_string(),
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
        if let Some(m) = self.modules.iter().find(|m| m.path == id) {
            return Some(Page {
                id: m.path.clone(),
                title: m.path.clone(),
                kind: "standard library",
                body: reference::render(m),
                see_also: Vec::new(),
            });
        }
        let (m, item) = reference::find_item(&self.modules, id)?;
        Some(Page {
            id: format!("{}.{}", m.path, item.name),
            title: format!("{}.{}", m.path, item.name),
            kind: "standard library",
            body: reference::render_item(m, item),
            see_also: vec![m.path.clone()],
        })
    }

    fn entries(&self) -> Vec<Entry> {
        module_entries(&self.modules)
    }
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
        });
        for item in &m.items {
            out.push(Entry {
                id: item.path(&m.path),
                title: format!("{} {}", item.kind().label(), item.name),
                summary: item.docs.first().cloned().unwrap_or_default(),
            });
        }
    }
    out
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
        let mut diags = crate::diagnostics::Diagnostics::new();
        let ws = crate::build::workspace::Workspace::load(&root, &mut map, &mut diags).ok()?;

        // Every library in the repository, checked together, so a page shows
        // what an importer would actually see.
        let mut modules = Vec::new();
        let mut cache = crate::parsing::parser::Cache::new();
        for target in ws.targets() {
            let unit = crate::compiler::modules::Unit {
                target: Some(target),
                platform: crate::compiler::driver::host_platform(),
                with_tests: false,
            };
            let analysis =
                crate::compiler::driver::analyze(Some(&ws), &mut map, &mut cache, &unit);
            let pkg = target.pkg;
            let owned = |m: &crate::compiler::modules::ModuleData| m.pkg == Some(pkg);
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
        if let Some(m) = self.modules.iter().find(|m| m.path == id) {
            return Some(Page {
                id: m.path.clone(),
                title: m.path.clone(),
                kind: "this repository",
                body: reference::render(m),
                see_also: Vec::new(),
            });
        }
        let (m, item) = reference::find_item(&self.modules, id)?;
        Some(Page {
            id: format!("{}.{}", m.path, item.name),
            title: format!("{}.{}", m.path, item.name),
            kind: "this repository",
            body: reference::render_item(m, item),
            see_also: vec![m.path.clone()],
        })
    }

    fn entries(&self) -> Vec<Entry> {
        module_entries(&self.modules)
    }
}

/// The error catalog: one page per diagnostic code.
pub struct Errors;

impl DocSource for Errors {
    fn kind(&self) -> &'static str {
        "error"
    }

    fn resolve(&self, id: &str) -> Option<Page> {
        // Both `buri docs error/result-discarded` and, via `cmd_docs`, the
        // two-word `buri docs error result-discarded`.
        let code = id.strip_prefix("error/").unwrap_or(id);
        let e = crate::documentation::errors::find(code)?;
        Some(Page {
            id: format!("error/{}", e.code),
            title: e.title.to_string(),
            kind: "error",
            body: e.text.to_string(),
            see_also: Vec::new(),
        })
    }

    fn entries(&self) -> Vec<Entry> {
        crate::documentation::errors::ERRORS
            .iter()
            .map(|e| Entry {
                id: format!("error/{}", e.code),
                title: e.title.to_string(),
                summary: format!("`{}`", e.code),
            })
            .collect()
    }
}

/// Every kind of documentation, in the order the index lists them.
///
/// This is the seam: one line here and a new kind appears in the index, in
/// search, in the manifest, and under `--format=json`.
pub fn sources() -> Vec<Box<dyn DocSource>> {
    let mut out: Vec<Box<dyn DocSource>> =
        vec![Box::new(Prose), Box::new(Cli), Box::new(Std::load()), Box::new(Errors)];
    if let Some(ws) = Workspace::load() {
        out.push(Box::new(ws));
    }
    out.push(Box::new(Normative));
    out
}

// ---------------------------------------------------------------------------
// The command
// ---------------------------------------------------------------------------

pub fn cmd_docs(args: &arguments::Args) -> i32 {
    let ctx = DocCtx {
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
        arguments::out(&index(&ctx));
        return 0;
    };

    match first {
        "search" => {
            let query: Vec<&str> = rest.collect();
            if query.is_empty() {
                eprintln!("error: `buri docs search` takes something to search for");
                return 2;
            }
            search(&query.join(" "), &ctx)
        }
        "assemble" => cmd_assemble(args.flags.check),
        "test" => doctest_command(&rest.collect::<Vec<_>>(), &ctx),
        "manifest" => {
            arguments::out(&manifest());
            0
        }
        "list" => {
            arguments::out(&index(&ctx));
            0
        }
        // `buri docs cli build` reads better than `buri docs cli/build`, and
        // it is what somebody types. Both work.
        "error" => match rest.next() {
            Some(code) => show(&format!("error/{code}"), &ctx),
            None => {
                arguments::out(&error_index(&ctx));
                0
            }
        },
        "cli" => match rest.next() {
            Some(name) => show(&format!("cli/{name}"), &ctx),
            None => {
                arguments::out(&command_index(&ctx));
                0
            }
        },
        id => show(id, &ctx),
    }
}

/// Every diagnostic code, for `buri docs error` with no argument.
fn error_index(ctx: &DocCtx) -> String {
    let mut out = String::new();
    let (bold, dim, reset) =
        if ctx.render.color() { ("\x1b[1m", "\x1b[2m", "\x1b[0m") } else { ("", "", "") };
    let _ = write!(out, "{bold}buri docs error <code>{reset} — one diagnostic in full\n\n");
    for e in crate::documentation::errors::ERRORS {
        let _ = writeln!(out, "  {:<28} {}", e.code, e.title);
    }
    let _ = write!(
        out,
        "\n{dim}Every code is printed in the diagnostic itself, in brackets after the\n\
         message. Every page carries a program that provokes it, and the test suite\n\
         checks that it still does.{reset}\n"
    );
    out
}

/// Every command, for `buri docs cli` with no argument.
fn command_index(ctx: &DocCtx) -> String {
    let mut out = String::new();
    let (bold, dim, reset) =
        if ctx.render.color() { ("\x1b[1m", "\x1b[2m", "\x1b[0m") } else { ("", "", "") };
    let _ = write!(out, "{bold}buri docs cli <command>{reset} — one command in full\n\n");
    for c in crate::commands::COMMANDS {
        let _ = writeln!(out, "  cli/{:<10} {}", c.name, c.blurb);
    }
    let _ = write!(out, "\n{dim}The synopsis and flag table on each page are generated from the\n\
                          same table that dispatches, so they cannot drift.{reset}\n");
    out
}

fn show(id: &str, ctx: &DocCtx) -> i32 {
    for source in sources() {
        if let Some(page) = source.resolve(id) {
            arguments::out(&emit(&page, ctx));
            return 0;
        }
    }
    eprintln!("error: there is no documentation topic `{id}`");
    let all: Vec<Entry> = sources().iter().flat_map(|s| s.entries()).collect();
    let ids: Vec<&str> = all.iter().map(|e| e.id.as_str()).collect();
    if let Some(near) = crate::build::buildfile::nearest(id, &ids) {
        eprintln!("  = did you mean `{near}`?");
    }
    eprintln!("  = `buri docs` lists every topic; `buri docs search <words>` looks inside them");
    2
}

fn emit(page: &Page, ctx: &DocCtx) -> String {
    let body = match ctx.density {
        Density::Full => page.body.clone(),
        Density::Dense => markdown::dense(&page.body),
    };
    match ctx.render {
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
            let mut out = markdown::to_terminal(&body, ctx.width, color);
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
fn index(ctx: &DocCtx) -> String {
    if ctx.render == Render::Json {
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
    let (bold, dim, reset) =
        if ctx.render.color() { ("\x1b[1m", "\x1b[2m", "\x1b[0m") } else { ("", "", "") };
    let _ = writeln!(out, "{bold}buri docs{reset} — the language, the build system, and this CLI\n");

    for (kind, heading) in [
        (Kind::Guide, "Start here"),
        (Kind::Lang, "The language"),
        (Kind::Build, "The build system and the CLI"),
    ] {
        let _ = writeln!(out, "{bold}{heading}{reset}");
        for t in topics::TOPICS.iter().filter(|t| t.kind == kind) {
            let _ = writeln!(out, "  {:<26} {}", t.id, t.title);
        }
        out.push('\n');
    }

    let _ = writeln!(out, "{bold}This CLI{reset}");
    for c in crate::commands::COMMANDS.iter().filter(|c| !c.hidden) {
        let _ = writeln!(out, "  cli/{:<22} {}", c.name, c.blurb);
    }
    out.push('\n');

    let _ = writeln!(
        out,
        "{bold}Diagnostics{reset}\n  {} codes — `buri docs error <code>`, or `buri docs error` to list them\n",
        crate::documentation::errors::ERRORS.len()
    );

    let _ = writeln!(out, "{bold}The standard library{reset}");
    let mut line = String::from("  ");
    for m in crate::compiler::standard_library::MODULES {
        let path = m.path;
        if line.chars().count() + path.len() + 2 > ctx.width.get() {
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
        let _ = writeln!(out, "  {id:<26} {title}");
    }

    let _ = writeln!(
        out,
        "\n{dim}buri docs <topic>              read one\n\
         buri docs search <words>       search all of them\n\
         buri docs <topic> --format=json    structured, for tools\n\
         buri docs <topic> --dense          headings and examples only\n\
         buri docs manifest             every id and shape, for an agent{reset}"
    );
    out
}

/// Substring search across every registered page, ranked by where the match
/// landed. Deliberately simple and deliberately deterministic: ties break on
/// the id, never on hash order.
/// Words that carry no signal in a query like "how do I read a file". Without
/// this, a natural-language question ranks by its filler.
const STOPWORDS: &[&str] = &[
    "a", "an", "and", "are", "as", "at", "be", "by", "can", "do", "does", "for", "from", "how",
    "i", "if", "in", "is", "it", "of", "on", "or", "that", "the", "to", "use", "what", "when",
    "where", "which", "why", "with", "you",
];

fn search(query: &str, ctx: &DocCtx) -> i32 {
    let needle = query.to_lowercase();
    let all: Vec<&str> = needle.split_whitespace().collect();
    let kept: Vec<&str> = all.iter().copied().filter(|w| !STOPWORDS.contains(w)).collect();
    // If the query is nothing but filler, search it as written rather than
    // searching nothing.
    let words: Vec<&str> = if kept.is_empty() { all } else { kept };
    let mut hits: Vec<(i64, String, String, String)> = Vec::new();

    for source in sources() {
        for Entry { id, title, summary } in source.entries() {
            let topic = topics::find(&id);
            let body = topic.map(|t| t.text).unwrap_or("").to_lowercase();
            let tags: Vec<&str> = topic.map(|t| t.tags.to_vec()).unwrap_or_default();
            let id_l = id.to_lowercase();
            let title_l = title.to_lowercase();

            let mut score = 0i64;
            // A multi-word query is usually a phrase — "tail call" means the
            // section about tail calls, not every page that says "call". Score
            // the phrase first and heavily, or `lang/expressions` wins every
            // query containing a common word.
            if words.len() > 1 {
                if title_l.contains(&needle) {
                    score += 60;
                }
                score += (body.matches(needle.as_str()).count() as i64 * 12).min(60);
            }
            for w in &words {
                if id_l == *w || title_l == *w {
                    score += 100;
                }
                if id_l.contains(w) {
                    score += 20;
                }
                if title_l.contains(w) {
                    score += 15;
                }
                if tags.iter().any(|t| t.to_lowercase() == *w) {
                    score += 12;
                }
                let n = body.matches(w).count() as i64;
                score += n.min(8);
            }
            if score > 0 {
                hits.push((score, id, title, summary));
            }
        }
    }
    // Highest score first, then by id, so two runs agree.
    hits.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    hits.truncate(12);

    if ctx.render == Render::Json {
        let s = crate::diagnostics::json_str;
        let rows: Vec<String> = hits
            .iter()
            .map(|(score, id, title, summary)| {
                format!(
                    "{{\"id\":{},\"title\":{},\"summary\":{},\"score\":{score}}}",
                    s(id),
                    s(title),
                    s(summary)
                )
            })
            .collect();
        arguments::out(&format!("{{\"query\":{},\"hits\":[{}]}}\n", s(query), rows.join(",")));
        return if hits.is_empty() { 1 } else { 0 };
    }

    if hits.is_empty() {
        eprintln!("error: nothing matches `{query}`");
        eprintln!("  = `buri docs` lists every topic");
        return 1;
    }
    let (bold, dim, reset) =
        if ctx.render.color() { ("\x1b[1m", "\x1b[2m", "\x1b[0m") } else { ("", "", "") };
    let mut listing = String::new();
    for (_, id, title, summary) in &hits {
        let _ = writeln!(listing, "{bold}{id}{reset}  {title}");
        let line = one_line(summary, ctx.width.get().saturating_sub(4));
        if !line.is_empty() {
            let _ = writeln!(listing, "  {dim}{line}{reset}");
        }
    }
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
        for Entry { id, title, summary } in source.entries() {
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
fn doctest_command(paths: &[&str], ctx: &DocCtx) -> i32 {
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
        let (dim, reset) = if ctx.render.color() { ("\x1b[2m", "\x1b[0m") } else { ("", "") };
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

fn cmd_assemble(check_only: bool) -> i32 {
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
        if d.join("cli/src/docs/SPEC.md").is_file() && d.join("cli/src/docs/lang").is_dir() {
            return Some(d.to_path_buf());
        }
        dir = d.parent();
    }
    None
}

fn terminal_width() -> usize {
    std::env::var("COLUMNS").ok().and_then(|c| c.parse().ok()).unwrap_or(80)
}

/// Where `--format` came from, so `cmd_docs` can ask for it without importing
/// the whole flag module.
pub use crate::commands::arguments::Format;

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> DocCtx {
        DocCtx {
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
        let text = index(&ctx());
        for t in topics::TOPICS {
            assert!(text.contains(t.id), "the index omits `{}`", t.id);
        }
    }

    #[test]
    fn a_page_renders_in_every_style() {
        let page = Prose.resolve("lang/effects").expect("lang/effects exists");
        for render in [Render::Human { color: false }, Render::Markdown, Render::Json] {
            let out = emit(&page, &DocCtx { render, ..ctx() });
            assert!(!out.trim().is_empty(), "{render:?} rendered nothing");
        }
        // JSON is one line, so a tool can read it a page at a time.
        let json = emit(&page, &DocCtx { render: Render::Json, ..ctx() });
        assert_eq!(json.lines().count(), 1, "JSON output must be one line");
    }

    #[test]
    fn dense_is_shorter_but_keeps_the_examples() {
        let page = Prose.resolve("lang/effects").unwrap();
        let full = emit(&page, &DocCtx { render: Render::Markdown, ..ctx() });
        let dense = emit(
            &page,
            &DocCtx { render: Render::Markdown, density: Density::Dense, ..ctx() },
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
            ("effects", "lang/effects"),
            ("ctx", "lang/effects"),
            ("exhaustive", "lang/patterns"),
            ("tags", "build/tags"),
            ("cache", "build/hermeticity"),
            ("tail call", "lang/evaluation"),
            ("public surface", "build/libraries"),
        ] {
            let needle = query.to_lowercase();
            let words: Vec<&str> = needle.split_whitespace().collect();
            let mut best: Option<(i64, String)> = None;
            for t in topics::TOPICS {
                let body = t.text.to_lowercase();
                let mut score = 0i64;
                if words.len() > 1 {
                    if t.title.to_lowercase().contains(&needle) {
                        score += 60;
                    }
                    score += (body.matches(needle.as_str()).count() as i64 * 12).min(60);
                }
                for w in &words {
                    if t.id.to_lowercase() == *w || t.title.to_lowercase() == *w {
                        score += 100;
                    }
                    if t.id.to_lowercase().contains(w) {
                        score += 20;
                    }
                    if t.title.to_lowercase().contains(w) {
                        score += 15;
                    }
                    if t.tags.iter().any(|x| x.to_lowercase() == *w) {
                        score += 12;
                    }
                    score += (body.matches(w).count() as i64).min(8);
                }
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
}
