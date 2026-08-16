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

use crate::cli;
use crate::doc_assemble;
use crate::doc_md;
use crate::doc_topics::{self, Kind, Topic};
use std::fmt::Write as _;

/// How a page is rendered.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Style {
    /// Wrapped, coloured, for a terminal.
    #[default]
    Human,
    /// The markdown source, for piping somewhere that renders it.
    Markdown,
    /// One JSON object, for a tool.
    Json,
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
    pub width: usize,
    pub color: bool,
    pub style: Style,
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

/// A kind of documentation the CLI can serve.
///
/// This is the extensibility seam. Implement it, add one line to `sources()`,
/// and the new kind appears in the index, in search, and in the manifest.
pub trait DocSource {
    fn kind(&self) -> &'static str;
    /// The page for an id, if this source owns it.
    fn resolve(&self, id: &str) -> Option<Page>;
    /// `(id, title, summary)` for everything this source serves.
    fn entries(&self) -> Vec<(String, String, String)>;
}

/// Prose topics: the language reference, the build system, the guide.
pub struct Prose;

impl DocSource for Prose {
    fn kind(&self) -> &'static str {
        "topic"
    }

    fn resolve(&self, id: &str) -> Option<Page> {
        let t = doc_topics::find(id)?;
        Some(page_of(t))
    }

    fn entries(&self) -> Vec<(String, String, String)> {
        doc_topics::TOPICS
            .iter()
            .map(|t| (t.id.to_string(), t.title.to_string(), t.summary()))
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
        let (title, lang, text) = match id {
            "grammar" => (NORMATIVE[0].1, "ebnf", doc_topics::GRAMMAR),
            "schema/build" => (NORMATIVE[1].1, "proto", doc_topics::BUILD_PROTO),
            "schema/repo" => (NORMATIVE[2].1, "proto", doc_topics::REPO_PROTO),
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

    fn entries(&self) -> Vec<(String, String, String)> {
        NORMATIVE
            .iter()
            .map(|(id, title, lang)| {
                (id.to_string(), title.to_string(), format!("hand-written {lang}, held to the implementation by a test"))
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

    fn entries(&self) -> Vec<(String, String, String)> {
        crate::commands::COMMANDS
            .iter()
            .map(|c| (format!("cli/{}", c.name), format!("buri {}", c.name), c.blurb.to_string()))
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
    modules: Vec<crate::doc_api::ApiModule>,
}

impl Std {
    pub fn load() -> Std {
        let mut map = crate::diag::SourceMap::new();
        let analysis = crate::driver::analyze_stdlib(&mut map);
        Std { modules: crate::doc_api::from_loaded(&analysis.loaded, &crate::doc_api::std_filter) }
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
                body: crate::doc_api::render(m),
                see_also: Vec::new(),
            });
        }
        let (m, item) = crate::doc_api::find_item(&self.modules, id)?;
        Some(Page {
            id: format!("{}.{}", m.path, item.name),
            title: format!("{}.{}", m.path, item.name),
            kind: "standard library",
            body: crate::doc_api::render_item(m, item),
            see_also: vec![m.path.clone()],
        })
    }

    fn entries(&self) -> Vec<(String, String, String)> {
        let mut out = Vec::new();
        for m in &self.modules {
            out.push((
                m.path.clone(),
                m.path.clone(),
                m.docs.first().cloned().unwrap_or_default(),
            ));
            for item in &m.items {
                out.push((
                    item.path(&m.path),
                    format!("{} {}", item.kind.label(), item.name),
                    item.docs.first().cloned().unwrap_or_default(),
                ));
            }
        }
        out
    }
}

/// The packages of the repository you are standing in, rendered by the same
/// code that renders the standard library.
///
/// This is what makes the feature belong to the ecosystem rather than to this
/// toolchain: a third-party repository gets an API reference for its own
/// libraries, from its own `///` comments, with nothing to set up.
pub struct Workspace {
    modules: Vec<crate::doc_api::ApiModule>,
}

impl Workspace {
    /// `None` outside a repository, which is not an error — most of the
    /// documentation does not need one.
    pub fn load() -> Option<Workspace> {
        let cwd = std::env::current_dir().ok()?;
        let root = crate::workspace::find_root(&cwd)?;
        let mut map = crate::diag::SourceMap::new();
        let mut diags = crate::diag::Diagnostics::new();
        let ws = crate::workspace::Workspace::load(&root, &mut map, &mut diags).ok()?;

        // Every library in the repository, checked together, so a page shows
        // what an importer would actually see.
        let mut modules = Vec::new();
        for target in ws.targets() {
            let unit = crate::compile::Unit {
                target: Some(target),
                platform: crate::driver::host_platform(),
                with_tests: false,
            };
            let analysis = crate::driver::analyze(Some(&ws), &mut map, &unit);
            let pkg = target.pkg;
            let owned = |m: &crate::compile::ModuleData| m.pkg == Some(pkg);
            for m in crate::doc_api::from_loaded(&analysis.loaded, &owned) {
                if !modules.iter().any(|e: &crate::doc_api::ApiModule| e.path == m.path) {
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
                body: crate::doc_api::render(m),
                see_also: Vec::new(),
            });
        }
        let (m, item) = crate::doc_api::find_item(&self.modules, id)?;
        Some(Page {
            id: format!("{}.{}", m.path, item.name),
            title: format!("{}.{}", m.path, item.name),
            kind: "this repository",
            body: crate::doc_api::render_item(m, item),
            see_also: vec![m.path.clone()],
        })
    }

    fn entries(&self) -> Vec<(String, String, String)> {
        let mut out = Vec::new();
        for m in &self.modules {
            out.push((m.path.clone(), m.path.clone(), m.docs.first().cloned().unwrap_or_default()));
            for item in &m.items {
                out.push((
                    item.path(&m.path),
                    format!("{} {}", item.kind.label(), item.name),
                    item.docs.first().cloned().unwrap_or_default(),
                ));
            }
        }
        out
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
        let e = crate::doc_errors::find(code)?;
        Some(Page {
            id: format!("error/{}", e.code),
            title: e.title.to_string(),
            kind: "error",
            body: e.text.to_string(),
            see_also: Vec::new(),
        })
    }

    fn entries(&self) -> Vec<(String, String, String)> {
        crate::doc_errors::ERRORS
            .iter()
            .map(|e| {
                (format!("error/{}", e.code), e.title.to_string(), format!("`{}`", e.code))
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

pub fn cmd_docs(args: &cli::Args) -> i32 {
    let ctx = DocCtx {
        width: terminal_width(),
        color: args.flags.color.unwrap_or_else(|| std::env::var("NO_COLOR").is_err())
            && args.flags.format != Some(Format::Json),
        style: match args.flags.format {
            Some(Format::Json) => Style::Json,
            Some(Format::Markdown) => Style::Markdown,
            None => Style::Human,
        },
        density: if args.flags.dense { Density::Dense } else { Density::Full },
    };

    let mut rest = args.targets.iter().map(String::as_str);
    let Some(first) = rest.next() else {
        cli::out(&index(&ctx));
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
        "assemble" => assemble(args.flags.check),
        "test" => doctest_command(&rest.collect::<Vec<_>>(), &ctx),
        "manifest" => {
            cli::out(&manifest());
            0
        }
        "list" => {
            cli::out(&index(&ctx));
            0
        }
        // `buri docs cli build` reads better than `buri docs cli/build`, and
        // it is what somebody types. Both work.
        "error" => match rest.next() {
            Some(code) => show(&format!("error/{code}"), &ctx),
            None => {
                cli::out(&error_index(&ctx));
                0
            }
        },
        "cli" => match rest.next() {
            Some(name) => show(&format!("cli/{name}"), &ctx),
            None => {
                cli::out(&command_index(&ctx));
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
        if ctx.color { ("\x1b[1m", "\x1b[2m", "\x1b[0m") } else { ("", "", "") };
    let _ = write!(out, "{bold}buri docs error <code>{reset} — one diagnostic in full\n\n");
    for e in crate::doc_errors::ERRORS {
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
        if ctx.color { ("\x1b[1m", "\x1b[2m", "\x1b[0m") } else { ("", "", "") };
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
            cli::out(&emit(&page, ctx));
            return 0;
        }
    }
    eprintln!("error: there is no documentation topic `{id}`");
    let all: Vec<(String, String, String)> =
        sources().iter().flat_map(|s| s.entries()).collect();
    let ids: Vec<&str> = all.iter().map(|(i, _, _)| i.as_str()).collect();
    if let Some(near) = crate::buildfile::nearest(id, &ids) {
        eprintln!("  = did you mean `{near}`?");
    }
    eprintln!("  = `buri docs` lists every topic; `buri docs search <words>` looks inside them");
    2
}

fn emit(page: &Page, ctx: &DocCtx) -> String {
    let body = match ctx.density {
        Density::Full => page.body.clone(),
        Density::Dense => doc_md::dense(&page.body),
    };
    match ctx.style {
        Style::Markdown => body,
        Style::Json => {
            let mut out = String::new();
            let s = crate::diag::json_str;
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
        Style::Human => {
            let mut out = doc_md::to_terminal(&body, ctx.width, ctx.color);
            if !page.see_also.is_empty() {
                let _ = write!(out, "\nSee also: {}\n", page.see_also.join(", "));
            }
            if let Some(doc) = doc_topics::find(&page.id).and_then(doc_assemble::document_of) {
                let _ = write!(out, "\nPart of {}.\n", doc.path);
            }
            out
        }
    }
}

/// The front page: what kinds exist, what is in each, and how to search.
fn index(ctx: &DocCtx) -> String {
    if ctx.style == Style::Json {
        let s = crate::diag::json_str;
        let mut rows = Vec::new();
        for source in sources() {
            for (id, title, summary) in source.entries() {
                rows.push(format!(
                    "{{\"kind\":{},\"id\":{},\"title\":{},\"summary\":{}}}",
                    s(source.kind()),
                    s(&id),
                    s(&title),
                    s(&summary)
                ));
            }
        }
        return format!("{{\"topics\":[{}]}}\n", rows.join(","));
    }

    let mut out = String::new();
    let (bold, dim, reset) =
        if ctx.color { ("\x1b[1m", "\x1b[2m", "\x1b[0m") } else { ("", "", "") };
    let _ = writeln!(out, "{bold}buri docs{reset} — the language, the build system, and this CLI\n");

    for (kind, heading) in [
        (Kind::Guide, "Start here"),
        (Kind::Lang, "The language"),
        (Kind::Build, "The build system and the CLI"),
    ] {
        let _ = writeln!(out, "{bold}{heading}{reset}");
        for t in doc_topics::TOPICS.iter().filter(|t| t.kind == kind) {
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
        crate::doc_errors::ERRORS.len()
    );

    let _ = writeln!(out, "{bold}The standard library{reset}");
    let mut line = String::from("  ");
    for path in crate::stdlib::MODULES {
        if line.chars().count() + path.len() + 2 > ctx.width {
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
        for (id, title, summary) in source.entries() {
            let topic = doc_topics::find(&id);
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

    if ctx.style == Style::Json {
        let s = crate::diag::json_str;
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
        cli::out(&format!("{{\"query\":{},\"hits\":[{}]}}\n", s(query), rows.join(",")));
        return if hits.is_empty() { 1 } else { 0 };
    }

    if hits.is_empty() {
        eprintln!("error: nothing matches `{query}`");
        eprintln!("  = `buri docs` lists every topic");
        return 1;
    }
    let (bold, dim, reset) =
        if ctx.color { ("\x1b[1m", "\x1b[2m", "\x1b[0m") } else { ("", "", "") };
    let mut listing = String::new();
    for (_, id, title, summary) in &hits {
        let _ = writeln!(listing, "{bold}{id}{reset}  {title}");
        let line = one_line(summary, ctx.width.saturating_sub(4));
        if !line.is_empty() {
            let _ = writeln!(listing, "  {dim}{line}{reset}");
        }
    }
    cli::out(&listing);
    0
}

/// A summary as one terminal line: inline markup rendered away, whitespace
/// collapsed, truncated on a word boundary.
fn one_line(text: &str, width: usize) -> String {
    let flat = doc_md::to_terminal(text, 10_000, false);
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
    let s = crate::diag::json_str;
    let mut out = String::new();
    let _ = write!(
        out,
        "{{\"tool\":\"buri\",\"version\":{},\"docsVersion\":1",
        s(crate::cli::VERSION)
    );

    let mut kinds: Vec<String> = Vec::new();
    let mut pages: Vec<String> = Vec::new();
    for source in sources() {
        kinds.push(s(source.kind()));
        for (id, title, summary) in source.entries() {
            let tags = doc_topics::find(&id)
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
    pages.last_mut().unwrap()
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
    let root = crate::workspace::find_root(&cwd)
        .or_else(|| repo_root_of(&cwd))
        .unwrap_or(cwd.clone());

    let files: Vec<std::path::PathBuf> = if paths.is_empty() {
        let mut found = Vec::new();
        markdown_under(&root, &root, &mut found);
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
        if source && !crate::doctest::has_examples(&raw) {
            continue;
        }
        let text = if source { crate::doctest::doc_comments(&raw) } else { raw };
        let found = crate::doctest::extract(&rel, &text)
            .blocks
            .iter()
            .filter(|b| b.mode != crate::doctest::Mode::Ignore)
            .count();
        if source && found == 0 {
            continue;
        }
        checked += 1;
        blocks += found;
        failures.extend(crate::doctest::run_file_in_repo(&root, &rel, &text));
    }
    let files_checked = checked;

    if failures.is_empty() {
        let (dim, reset) = if ctx.color { ("\x1b[2m", "\x1b[0m") } else { ("", "") };
        cli::out(&format!(
            "{blocks} example(s) in {files_checked} document(s) compile{dim} — and the ones \
             that print something were run{reset}\n"
        ));
        return 0;
    }
    eprint!("{}", crate::doctest::report(&failures));
    eprintln!("{} example(s) do not do what the documentation says", failures.len());
    1
}

/// Every `.md` in the tree, skipping the build directory and anything hidden.
fn markdown_under(root: &std::path::Path, dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
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
            markdown_under(root, &p, out);
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

fn assemble(check_only: bool) -> i32 {
    let cwd = match std::env::current_dir() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };
    let Some(root) = crate::workspace::find_root(&cwd).or_else(|| repo_root_of(&cwd)) else {
        eprintln!("error: `buri docs assemble` must run inside the Buri repository");
        return 2;
    };

    let drifted = doc_assemble::drifted(&root);
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
    match doc_assemble::write(&root) {
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
/// `SPEC.md` and `cli/`. `find_root` looks for `REPO.buri`, which this
/// repository does not have at its root.
fn repo_root_of(from: &std::path::Path) -> Option<std::path::PathBuf> {
    let mut dir = Some(from);
    while let Some(d) = dir {
        if d.join("SPEC.md").is_file() && d.join("cli").is_dir() {
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
pub use crate::cli::Format;

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> DocCtx {
        DocCtx { width: 80, color: false, style: Style::Human, density: Density::Full }
    }

    #[test]
    fn every_registered_page_resolves() {
        for source in sources() {
            for (id, _, _) in source.entries() {
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
        for t in doc_topics::TOPICS {
            assert!(text.contains(t.id), "the index omits `{}`", t.id);
        }
    }

    #[test]
    fn a_page_renders_in_every_style() {
        let page = Prose.resolve("lang/effects").expect("lang/effects exists");
        for style in [Style::Human, Style::Markdown, Style::Json] {
            let out = emit(&page, &DocCtx { style, ..ctx() });
            assert!(!out.trim().is_empty(), "{style:?} rendered nothing");
        }
        // JSON is one line, so a tool can read it a page at a time.
        let json = emit(&page, &DocCtx { style: Style::Json, ..ctx() });
        assert_eq!(json.lines().count(), 1, "JSON output must be one line");
    }

    #[test]
    fn dense_is_shorter_but_keeps_the_examples() {
        let page = Prose.resolve("lang/effects").unwrap();
        let full = emit(&page, &DocCtx { style: Style::Markdown, ..ctx() });
        let dense = emit(
            &page,
            &DocCtx { style: Style::Markdown, density: Density::Dense, ..ctx() },
        );
        assert!(dense.len() < full.len());
        for f in doc_md::fences(&page.body) {
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
            for t in doc_topics::TOPICS {
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
