//! What the site is made of.
//!
//! The inventory is the toolchain's own: `documentation::topics` says which
//! prose pages exist and in what order, `commands::COMMANDS` says which
//! commands the CLI page has a block for, and `documentation::errors` and
//! `lints` say which codes have a page. A page the site is missing is therefore
//! a page `buri docs` is missing too, which is the only way the two can be held
//! together without a second list to keep in step.
//!
//! The *text* is read from `cli/src/docs/**` on disk rather than from the
//! copies compiled into `buri`, so that editing a page and re-running the
//! generator shows the edit — and so that `--watch` has something to watch.

use buri::commands::add::skills::SKILLS;
use buri::commands::COMMANDS;
use buri::documentation::frontmatter;
use buri::documentation::markdown;
use buri::documentation::reference::{self, Api, ApiItem, ApiModule, ItemKind};
use buri::documentation::topics::{self, Kind};
use std::path::{Path, PathBuf};

/// One grouping in the navigation, mirroring the headings `buri docs` prints
/// over its own index.
pub struct Section {
    /// The first segment of every route in it.
    pub slug: &'static str,
    pub title: &'static str,
    pub blurb: &'static str,
    /// Where its pages are edited. A doc link naming the directory rather than
    /// a file lands on the section's index.
    pub directory: &'static str,
}

pub const SECTIONS: &[Section] = &[
    Section {
        slug: "getting-started",
        title: "Getting started",
        blurb: "Why the language is shaped the way it is, how to install it, and the \
                first program — read in that order, once.",
        directory: "cli/src/docs/getting-started",
    },
    Section {
        slug: "guides",
        title: "Guides",
        blurb: "How to do one thing, and the ideas you have to hold to do it: the build \
                system, testing, effects, numbers, the browser.",
        directory: "cli/src/docs/guides",
    },
    Section {
        slug: "language",
        title: "The language",
        blurb: "The reference, section by section: what the language is, exactly, and what \
                a conforming implementation has to do.",
        directory: "cli/src/docs/language",
    },
    Section {
        slug: "reference",
        title: "Reference",
        blurb: "Lookup, not reading: the standard library, the build files, the CLI, every \
                diagnostic and lint code, and the normative grammar and schemas.",
        directory: "cli/src/docs/reference",
    },
];

const GETTING_STARTED: usize = 0;
const GUIDES: usize = 1;
const LANGUAGE: usize = 2;
const REFERENCE: usize = 3;

/// One heading inside the reference section, in the sidebar and on the
/// reference index.
///
/// The reference is the one section a flat list cannot serve: it holds two
/// hundred and twenty-one error pages beside eight build pages, and a sidebar
/// that listed them all would list nothing a reader could find. So the section
/// navigates by group, and a group too long to list — errors, lints — is
/// represented by the one page that lists it.
pub struct Group {
    pub title: &'static str,
    pub blurb: &'static str,
    /// The page that lists everything in this group, where the group is too
    /// long to list in the navigation. `None` where the group's own pages are
    /// short enough to be the listing.
    pub index: Option<Catalogue>,
}

/// The one page a group too long to navigate is represented by.
pub struct Catalogue {
    pub route: &'static str,
    /// What the navigation calls it, and its own heading. Not the group's own
    /// name: a group called "Errors" holding one link called "Errors" says the
    /// word twice and the second one tells a reader nothing.
    pub title: &'static str,
    /// Where its pages are edited.
    pub directory: &'static str,
}

pub const GROUPS: &[Group] = &[
    Group {
        title: "Standard library",
        blurb: "What `core/` and `ui/` give you, module by module, and what they deliberately \
                do not.",
        index: None,
    },
    Group {
        title: "Build",
        blurb: "The monorepo, the build files, the tags, and what the cache promises.",
        index: None,
    },
    Group {
        title: "The CLI",
        blurb: "Every command on one page, generated from the table that dispatches them.",
        index: None,
    },
    Group {
        title: "Errors",
        blurb: "Every code the compiler emits, what the rule is, and what to do about it.",
        index: Some(Catalogue {
            route: "reference/errors",
            title: "Every error code",
            directory: "cli/src/docs/reference/errors",
        }),
    },
    Group {
        title: "Lints",
        blurb: "What `buri lint` reports that type checking does not.",
        index: Some(Catalogue {
            route: "reference/lints",
            title: "Every lint code",
            directory: "cli/src/docs/reference/lints",
        }),
    },
    Group {
        title: "Agent skills",
        blurb: "The skills `buri add skills` writes, for a coding agent working in a \
                Buri repository.",
        index: None,
    },
    Group {
        title: "Normative",
        blurb: "The grammar and the build-file schemas, hand-written and held to the \
                implementation by a test.",
        index: None,
    },
];

const STANDARD_LIBRARY: usize = 0;
const BUILD: usize = 1;
const CLI: usize = 2;
const ERRORS: usize = 3;
const LINTS: usize = 4;
const SKILLS_GROUP: usize = 5;
const NORMATIVE_GROUP: usize = 6;

/// The topic the generated CLI page opens with: the rules every command
/// shares. It is a registered topic, so `buri docs reference/cli` serves the
/// same prose, and its file is found through the registry rather than named
/// twice.
const CLI_TOPIC: &str = "reference/cli";

/// Where the CLI page's prose is edited. One page, a directory of sources —
/// the intro beside it and one file per command.
const CLI_DIRECTORY: &str = "cli/src/docs/reference/cli";

/// Where the standard library is written. Its pages are generated from the
/// API the compiler reads out of these files, and this is where a reader who
/// wants to change one goes.
const STD_SOURCES: &str = "cli/src/compiler/standard_library/sources";

/// The page that lists every standard library module.
///
/// Forty-odd modules are a listing rather than a navigation, so the sidebar
/// names this page and the module pages themselves are unlisted — the same
/// arrangement the error and lint catalogues use, and for the same reason.
/// The group's face is still `reference/standard-library`, the prose map over
/// the top of the library, which is what a reader who does not yet know which
/// module they want needs first.
const STD_INDEX: Catalogue =
    Catalogue { route: "reference/std", title: "Every module", directory: STD_SOURCES };

/// Where a page came from, and whether that is a file or a directory. The
/// "edit on GitHub" link is a `blob` for one and a `tree` for the other.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Source {
    pub path: String,
    pub directory: bool,
}

/// A row a diagnostic page's frontmatter contributes, above its prose.
pub struct Fact {
    pub term: &'static str,
    pub value: String,
}

pub enum Content {
    /// Markdown, rendered.
    Prose(String),
    /// A section's own listing, generated from the pages in it.
    Listing(usize),
    /// The reference index: every group, and what the navigation shows under it.
    Groups,
    /// One group's own listing, generated from every page in it. This is where
    /// the two hundred and twenty-one error codes are written down.
    GroupListing(usize),
}

pub struct Page {
    /// The site path. `""` is the front page; every other page is written to
    /// `<route>/index.html`.
    pub route: String,
    /// The heading, and what the navigation calls it.
    pub title: String,
    /// What a reader types at `buri docs`: `language/lexical`,
    /// `circular-import`. Empty where there is nothing to type.
    pub label: String,
    /// The first sentence, for a listing and for the page description.
    pub summary: String,
    pub source: Source,
    pub section: Option<usize>,
    /// Which group of the reference section this page belongs to. `None`
    /// everywhere else — no other section groups.
    pub group: Option<usize>,
    /// Whether the navigation names this page under its group. A code page is
    /// not named: its group's index page lists it, and a sidebar holding all of
    /// them would hold nothing anybody could find.
    pub listed: bool,
    pub content: Content,
    /// Routes this page sends a reader to next.
    pub see_also: Vec<String>,
    pub facts: Vec<Fact>,
    /// Whose writing a page's body was adapted from, licence and all. It
    /// belongs to the page rather than to the diagnostic, so `buri docs` puts
    /// it under the page and so does this.
    pub adapted_from: Option<String>,
}

pub struct Site {
    pub root: PathBuf,
    pub pages: Vec<Page>,
}

impl Site {
    /// The route of the page generated from a repository path.
    pub fn route_of_source(&self, path: &str) -> Option<&str> {
        self.pages
            .iter()
            .find(|p| !p.source.directory && p.source.path == path)
            .map(|p| p.route.as_str())
    }

    /// The route of the page a repository *directory* is published as: a
    /// section's index, a group's index, or the one page a directory of prose
    /// files is assembled into.
    pub fn route_of_directory(&self, path: &str) -> Option<&str> {
        let path = path.trim_end_matches('/');
        self.pages
            .iter()
            .find(|p| p.source.directory && p.source.path == path)
            .map(|p| p.route.as_str())
    }

    pub fn page(&self, route: &str) -> Option<&Page> {
        self.pages.iter().find(|p| p.route == route)
    }

    /// The pages of one section, in the order the navigation lists them.
    pub fn in_section(&self, section: usize) -> impl Iterator<Item = &Page> {
        self.pages.iter().filter(move |p| p.section == Some(section) && !p.is_index())
    }

    /// What the navigation shows under one group of the reference: its pages,
    /// or — where the group is a catalogue — the one page that lists it.
    pub fn navigation_of(&self, group: usize) -> impl Iterator<Item = &Page> {
        self.pages.iter().filter(move |p| p.group == Some(group) && p.listed)
    }

    /// Everything in one group that the navigation does not name. This is what
    /// a group's own listing page is made of: the two hundred and twenty-one
    /// error codes, the forty standard library modules. A page the sidebar
    /// already names is not repeated in the catalogue that stands in for the
    /// ones it cannot.
    pub fn in_group(&self, group: usize) -> impl Iterator<Item = &Page> {
        self.pages.iter().filter(move |p| p.group == Some(group) && !p.listed && !p.is_index())
    }

    /// Every file the site is generated from, for `--watch` to poll.
    ///
    /// The standard library's sources are not among them, although its pages
    /// name one each. Those pages are built from the API the compiler read out
    /// of the copies `include_str!` baked into `buri`, so editing
    /// `sources/list.buri` changes nothing this generator can see until the
    /// binary is rebuilt — and a poll that rebuilt the site into the same bytes
    /// would be claiming otherwise.
    pub fn sources(&self) -> Vec<PathBuf> {
        let mut out: Vec<PathBuf> = self
            .pages
            .iter()
            .filter(|p| !p.source.directory && !p.source.path.starts_with(STD_SOURCES))
            .map(|p| self.root.join(&p.source.path))
            .collect();
        out.sort();
        out.dedup();
        out
    }
}

impl Page {
    pub fn is_index(&self) -> bool {
        matches!(
            self.content,
            Content::Listing(_) | Content::Groups | Content::GroupListing(_)
        )
    }

    /// Where this page is edited, on GitHub.
    pub fn edit_url(&self) -> String {
        crate::links::repository_url(&self.source.path, self.source.directory)
    }
}

/// The directory the repository is checked out in, found by walking up from
/// `start` until the documentation and the README are both under it.
pub fn find_root(start: &Path) -> Option<PathBuf> {
    let mut here = Some(start);
    while let Some(directory) = here {
        if directory.join("cli/src/docs").is_dir() && directory.join("README.md").is_file() {
            return Some(directory.to_path_buf());
        }
        here = directory.parent();
    }
    None
}

/// The route a topic id names, so that a `see_also` and a cross-reference land
/// on the same page.
///
/// The id is not the route: `build/tags` is served at `reference/build/tags`,
/// because the id has been published for a while and the shelf it belongs on
/// has changed. `Kind::directory` is the one place that mapping lives, and the
/// site reads it from there rather than keeping a second copy.
pub fn route_of_topic(id: &str) -> Option<String> {
    topics::find(id).map(route_of)
}

fn route_of(topic: &topics::Topic) -> String {
    let name = topic.id.rsplit('/').next().unwrap_or(topic.id);
    format!("{}/{name}", topic.kind.directory())
}

/// Reads every page. The only failure is a missing file, which means the
/// registry and the tree disagree — a broken checkout, not a broken page.
pub fn read(root: &Path) -> Result<Site, String> {
    let mut pages = Vec::new();
    pages.push(front_page(root)?);
    for section in 0..SECTIONS.len() {
        pages.push(listing(section));
    }
    for group in 0..GROUPS.len() {
        if let Some(page) = group_listing(group) {
            pages.push(page);
        }
    }
    read_topics(root, &mut pages)?;
    pages.push(cli_page(root)?);
    read_standard_library(root, &mut pages);
    read_catalog(root, &mut pages)?;
    read_skills(root, &mut pages)?;
    read_normative(root, &mut pages)?;
    Ok(Site { root: root.to_path_buf(), pages })
}

fn slurp(root: &Path, path: &str) -> Result<String, String> {
    std::fs::read_to_string(root.join(path)).map_err(|why| format!("{path}: {why}"))
}

fn front_page(root: &Path) -> Result<Page, String> {
    let text = slurp(root, "README.md")?;
    let summary = markdown::summary(&text);
    Ok(Page {
        route: String::new(),
        title: "Buri".to_string(),
        label: String::new(),
        summary,
        source: Source { path: "README.md".to_string(), directory: false },
        section: None,
        group: None,
        listed: false,
        content: Content::Prose(text),
        see_also: Vec::new(),
        facts: Vec::new(),
        adapted_from: None,
    })
}

fn listing(section: usize) -> Page {
    let entry = SECTIONS.get(section);
    let (slug, title, blurb, directory) = match entry {
        Some(s) => (s.slug, s.title, s.blurb, s.directory),
        None => ("", "", "", ""),
    };
    Page {
        route: slug.to_string(),
        title: title.to_string(),
        label: String::new(),
        summary: blurb.to_string(),
        source: Source { path: directory.to_string(), directory: true },
        section: Some(section),
        group: None,
        listed: false,
        content: if section == REFERENCE { Content::Groups } else { Content::Listing(section) },
        see_also: Vec::new(),
        facts: Vec::new(),
        adapted_from: None,
    }
}

/// The page that lists one group, for a group long enough to need one.
fn group_listing(group: usize) -> Option<Page> {
    let entry = GROUPS.get(group)?;
    let catalogue = entry.index.as_ref()?;
    Some(Page {
        route: catalogue.route.to_string(),
        title: catalogue.title.to_string(),
        label: String::new(),
        summary: entry.blurb.to_string(),
        source: Source { path: catalogue.directory.to_string(), directory: true },
        section: Some(REFERENCE),
        group: Some(group),
        listed: true,
        content: Content::GroupListing(group),
        see_also: Vec::new(),
        facts: Vec::new(),
        adapted_from: None,
    })
}

fn read_topics(root: &Path, pages: &mut Vec<Page>) -> Result<(), String> {
    for topic in topics::TOPICS {
        // The CLI's prose is served as a topic and read here as the head of the
        // generated CLI page, which is written to the route this topic names.
        // Reading it twice would write two pages to one route.
        if topic.id == CLI_TOPIC {
            continue;
        }
        let (section, group) = match topic.kind {
            Kind::GettingStarted => (GETTING_STARTED, None),
            Kind::Guide => (GUIDES, None),
            Kind::Language => (LANGUAGE, None),
            Kind::Build => (REFERENCE, Some(BUILD)),
            // The reference topics that are not the build system are the
            // standard library's prose map and the CLI's shared rules; the one
            // that is left belongs with the generated module pages.
            Kind::Reference => (REFERENCE, Some(STANDARD_LIBRARY)),
        };
        let path = topic.path();
        let text = slurp(root, &path)?;
        let summary = markdown::summary(&text);
        pages.push(Page {
            route: route_of(topic),
            title: topic.title.to_string(),
            label: topic.id.to_string(),
            summary,
            source: Source { path, directory: false },
            section: Some(section),
            group,
            listed: true,
            content: Content::Prose(text),
            see_also: topic.see_also.iter().filter_map(|id| route_of_topic(id)).collect(),
            facts: Vec::new(),
            adapted_from: None,
        });
    }
    Ok(())
}

/// Every command on one page.
///
/// The synopsis, the subcommands and the flag table of each are generated from
/// `commands::COMMANDS` — the same table that dispatches — so the page cannot
/// describe a flag the binary does not accept nor omit one it does. Thirteen
/// pages of chrome around a paragraph each was thirteen navigations to answer
/// one question; a reader looking a command up is looking the *set* of them up,
/// and the browser's own find is the index.
fn cli_page(root: &Path) -> Result<Page, String> {
    let topic =
        topics::find(CLI_TOPIC).ok_or_else(|| format!("`{CLI_TOPIC}` is not a topic"))?;
    let intro = slurp(root, &topic.path())?;
    let summary = markdown::summary(&intro);
    let mut text = from_one_directory_up(&intro);
    for command in COMMANDS.iter().filter(|c| !c.hidden) {
        text.push('\n');
        text.push_str(&command_block(&buri::commands::reference(command)));
    }
    Ok(Page {
        route: route_of(topic),
        title: topic.title.to_string(),
        label: topic.id.to_string(),
        summary,
        source: Source { path: CLI_DIRECTORY.to_string(), directory: true },
        section: Some(REFERENCE),
        group: Some(CLI),
        listed: true,
        content: Content::Prose(text),
        see_also: topic.see_also.iter().filter_map(|id| route_of_topic(id)).collect(),
        facts: Vec::new(),
        adapted_from: None,
    })
}

/// The intro's relative links, rewritten as though the file sat one directory
/// further down.
///
/// The intro is `reference/cli.md` and the command prose is
/// `reference/cli/<name>.md`, and a page resolves its links against one base:
/// the directory, because that is where twelve of the thirteen sources are. A
/// destination the intro wrote is correct where the file is read — on GitHub,
/// and by `buri docs reference/cli` — so it is the one that has to climb a
/// level here rather than the other twelve.
fn from_one_directory_up(text: &str) -> String {
    let mut out = String::with_capacity(text.len().saturating_add(32));
    let mut rest = text;
    while let Some(at) = rest.find("](") {
        let (before, after) = rest.split_at(at.saturating_add(2));
        out.push_str(before);
        let end = after.find(')').unwrap_or(0);
        let destination = after.get(..end).unwrap_or("");
        let elsewhere = destination.is_empty()
            || destination.starts_with('#')
            || destination.contains("://")
            || destination.starts_with("mailto:");
        if !elsewhere {
            out.push_str("../");
        }
        rest = after;
    }
    out.push_str(rest);
    out
}

/// One command's reference as a block of the CLI page.
///
/// Every heading goes down a level, so that `# buri build` is a section rather
/// than a second title on the page — headings inside a fence are not headings,
/// because a `textproto` comment opens with `#` too. And the exit codes come
/// out: `buri docs cli build` is one command alone and needs them, but they are
/// the same three for every command and the page states them once above.
fn command_block(text: &str) -> String {
    let mut out = String::with_capacity(text.len().saturating_add(64));
    let mut fenced = false;
    let mut after_exit_codes = false;
    for line in text.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") {
            fenced = !fenced;
        }
        if !fenced {
            if trimmed.starts_with("Exit codes:") {
                after_exit_codes = true;
                continue;
            }
            if std::mem::take(&mut after_exit_codes) && trimmed.is_empty() {
                continue;
            }
            if trimmed.starts_with('#') {
                out.push('#');
            }
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// Every module of the standard library, one page each, and the listing that
/// names them all.
///
/// Nothing here is hand-written. `documentation::reference` builds the same
/// `ApiModule` values `buri docs core/list` prints — signatures printed by the
/// formatter, documentation lifted off the declarations the compiler checked —
/// and this writes them out as pages. The markdown is assembled here rather
/// than taken from `reference::render` because a web page needs two things a
/// terminal does not: an item's heading has to slug to the item's own name, so
/// that `reference/std/core/list#map` lands on `map`, and the prose a module
/// carries has to sit *under* the page's own headings rather than beside them.
fn read_standard_library(root: &Path, pages: &mut Vec<Page>) {
    let mut map = buri::diagnostics::SourceMap::new();
    let analysis = buri::compiler::driver::analyze_stdlib(&mut map);
    let modules = reference::from_loaded(&analysis.loaded, &reference::std_filter);
    let files = std_sources(root);

    pages.push(Page {
        route: STD_INDEX.route.to_string(),
        title: STD_INDEX.title.to_string(),
        label: String::new(),
        summary: "Every module the standard library ships, and what each one is for.".to_string(),
        source: Source { path: STD_INDEX.directory.to_string(), directory: true },
        section: Some(REFERENCE),
        group: Some(STANDARD_LIBRARY),
        listed: true,
        content: Content::GroupListing(STANDARD_LIBRARY),
        see_also: Vec::new(),
        facts: Vec::new(),
        adapted_from: None,
    });

    for module in &modules {
        let text = module_markdown(module);
        // A module's first line names itself — "core/list — the defining
        // module of `[T]`" — and a listing that has just written the name
        // does not need it again.
        let summary = markdown::summary(&text);
        let summary =
            summary.strip_prefix(&format!("{} — ", module.path)).unwrap_or(&summary).to_string();
        pages.push(Page {
            route: format!("{}/{}", STD_INDEX.route, module.path),
            // The path is the title: it is what the module is called, what
            // `buri docs` answers to, and what an import writes.
            title: module.path.clone(),
            label: String::new(),
            summary,
            source: std_source_of(&files, &module.path),
            section: Some(REFERENCE),
            group: Some(STANDARD_LIBRARY),
            listed: false,
            content: Content::Prose(text),
            see_also: route_of_topic("reference/standard-library").into_iter().collect(),
            facts: Vec::new(),
            adapted_from: None,
        });
    }
}

/// Every `.buri` file the standard library is written in, paired with its text.
///
/// The table in `compiler::standard_library` holds a module's path and the
/// text `include_str!` embedded, and not the name of the file that text came
/// from: `core/host/testing` is `host_testing.buri` and `core/net/http` is
/// `http.buri`, so the file cannot be derived from the path. Matching on the
/// bytes is exact, because the embedded copy *is* the file.
fn std_sources(root: &Path) -> Vec<(String, String)> {
    let Ok(entries) = std::fs::read_dir(root.join(STD_SOURCES)) else { return Vec::new() };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|extension| extension.to_str()) != Some("buri") {
            continue;
        }
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else { continue };
        let Ok(text) = std::fs::read_to_string(&path) else { continue };
        out.push((text, format!("{STD_SOURCES}/{name}")));
    }
    out.sort_by(|a, b| a.1.cmp(&b.1));
    out
}

/// Where one module is edited: its own file, or — in a checkout whose text has
/// moved on from the copy compiled into `buri`, which is the only way the
/// match fails — the directory they all live in, which is a link that still
/// lands somewhere true.
fn std_source_of(files: &[(String, String)], module: &str) -> Source {
    let embedded = buri::compiler::standard_library::source(module);
    match embedded.and_then(|text| files.iter().find(|(body, _)| body == text)) {
        Some((_, path)) => Source { path: path.clone(), directory: false },
        None => Source { path: STD_SOURCES.to_string(), directory: true },
    }
}

/// One module as a page: what the module says about itself, then its items in
/// the reference's own order — what a type *is* before what it can do.
fn module_markdown(m: &ApiModule) -> String {
    let mut out = format!("# {}\n\n", m.path);
    if !m.docs.is_empty() {
        out.push_str(&demoted(&m.docs.join("\n"), 1));
        out.push('\n');
    }
    if m.items.is_empty() {
        out.push_str("This module exports nothing.\n");
        return out;
    }
    let mut last: Option<ItemKind> = None;
    for item in &m.items {
        if last != Some(item.kind()) {
            out.push_str(&format!("## {}\n\n", item.kind().heading()));
            if item.api.is_callable() {
                out.push_str(reference::PURITY);
                out.push_str("\n\n");
            }
            last = Some(item.kind());
        }
        out.push_str(&item_markdown(item));
    }
    out
}

/// One item: its name as a heading a link can name, its signature, what it is
/// allowed to do to the world, and its own documentation.
///
/// The heading is the bare name rather than `[A].map`, so that the anchor is
/// the name a reader would write down. What a method hangs off is the first
/// thing said under it instead, which is where the effects line already was.
fn item_markdown(item: &ApiItem) -> String {
    let mut out = format!("### {}\n\n```buri sig\n{}\n```\n\n", item.name, item.signature);
    let mut said = String::new();
    if let Api::Method { owner, via_trait, .. } = &item.api {
        said.push_str(&format!("A method on `{owner}`"));
        if let Some(via) = via_trait {
            said.push_str(&format!(", via `{via}`"));
        }
        said.push_str(". ");
    }
    let effects = item.api.effects();
    if !effects.is_empty() {
        said.push_str(&format!("Effects: `{}`.", effects.join("` · `")));
    } else if item.api.is_callable() {
        said.push_str("Pure.");
    }
    if !said.trim().is_empty() {
        out.push_str(said.trim_end());
        out.push_str("\n\n");
    }
    if !item.docs.is_empty() {
        out.push_str(&demoted(&item.docs.join("\n"), 2));
        out.push('\n');
    }
    if !item.api.members().is_empty() {
        for member in item.api.members() {
            let docs = if member.docs.is_empty() {
                String::new()
            } else {
                format!(" — {}", member.docs.join(" "))
            };
            out.push_str(&format!("- `{}`{docs}\n", member.signature));
        }
        out.push('\n');
    }
    out
}

/// Prose that documents a module or an item, one or two heading levels further
/// down, so that it sits under the page's headings rather than beside them —
/// `core/crypto` opens its `//!` with a `#`, which would otherwise be a second
/// title on the page.
///
/// A `#` inside a fence is a comment or a hidden line in an example rather than
/// a heading, which is the distinction the CLI page's `command_block` draws for
/// the same reason.
fn demoted(text: &str, levels: usize) -> String {
    let mut out = String::with_capacity(text.len().saturating_add(64));
    let mut fenced = false;
    for line in text.lines() {
        if line.trim_start().starts_with("```") {
            fenced = !fenced;
        }
        if !fenced && line.starts_with('#') {
            let hashes = line.chars().take_while(|c| *c == '#').count();
            // Six is as deep as a heading goes, and one that is already there
            // stays where it is.
            if line.get(hashes..).is_some_and(|rest| rest.starts_with(' ')) {
                for _ in hashes..hashes.saturating_add(levels).min(6) {
                    out.push('#');
                }
            }
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// Every diagnostic and every lint. The two catalogs have one shape — a
/// frontmatter block carrying the wording, and a body explaining it — so they
/// are read by one function.
fn read_catalog(root: &Path, pages: &mut Vec<Page>) -> Result<(), String> {
    let errors = buri::documentation::errors::ERRORS
        .iter()
        .map(|e| (ERRORS, "errors", e.code, e.listed_title, e.see_also));
    let lints = buri::documentation::lints::LINTS
        .iter()
        .map(|l| (LINTS, "lints", l.code, l.listed_title, l.see_also));
    for (group, directory, code, listed_title, see_also) in errors.chain(lints) {
        let path = format!("cli/src/docs/reference/{directory}/{code}.md");
        let text = slurp(root, &path)?;
        let parsed = frontmatter::parse(&text).map_err(|why| format!("{path}: {why}"))?;
        let (title, facts, adapted_from, body) = match parsed {
            Some((front, body)) => {
                let mut facts = vec![
                    Fact { term: "Severity", value: front.severity.label().to_string() },
                    Fact { term: "Message", value: front.message.clone() },
                ];
                if let Some(label) = front.label.clone() {
                    facts.push(Fact { term: "Label", value: label });
                }
                if let Some(note) = front.note.clone() {
                    facts.push(Fact { term: "Note", value: note });
                }
                if let Some(fix) = front.fix.clone() {
                    facts.push(Fact { term: "Fix", value: fix });
                }
                (front.title.clone(), facts, front.adapted_from.clone(), body.to_string())
            }
            None => (listed_title.to_string(), Vec::new(), None, text.clone()),
        };
        let summary = match markdown::summary(&body) {
            sentence if sentence.trim().is_empty() => {
                facts.iter().find(|f| f.term == "Message").map(|f| f.value.clone()).unwrap_or_default()
            }
            sentence => sentence,
        };
        pages.push(Page {
            route: format!("reference/{directory}/{code}"),
            title,
            label: code.to_string(),
            summary,
            source: Source { path, directory: false },
            section: Some(REFERENCE),
            group: Some(group),
            listed: false,
            content: Content::Prose(body),
            see_also: see_also.iter().filter_map(|id| route_of_topic(id)).collect(),
            facts,
            adapted_from,
        });
    }
    Ok(())
}

fn read_skills(root: &Path, pages: &mut Vec<Page>) -> Result<(), String> {
    for skill in SKILLS {
        let path = format!("cli/src/docs/reference/skills/{}.md", skill.name);
        let text = slurp(root, &path)?;
        let (front, body) = split_front_block(&text);
        let description = front.and_then(|block| field(block, "description"));
        let title = markdown::headings(body)
            .first()
            .map(|heading| heading.title.to_string())
            .unwrap_or_else(|| skill.name.to_string());
        pages.push(Page {
            route: format!("reference/skills/{}", skill.name),
            title,
            label: skill.name.to_string(),
            summary: description.clone().unwrap_or_else(|| markdown::summary(body)),
            source: Source { path, directory: false },
            section: Some(REFERENCE),
            group: Some(SKILLS_GROUP),
            listed: true,
            content: Content::Prose(body.to_string()),
            see_also: Vec::new(),
            facts: description
                .into_iter()
                .map(|value| Fact { term: "Use when", value })
                .collect(),
            adapted_from: None,
        });
    }
    Ok(())
}

/// The grammar and the two schemas: hand-written, normative, and shown
/// verbatim in one block, exactly as `buri docs grammar` shows them.
fn read_normative(root: &Path, pages: &mut Vec<Page>) -> Result<(), String> {
    const NORMATIVE: &[(&str, &str, &str, &str)] = &[
        ("reference/grammar", "The normative grammar", "cli/src/docs/grammar.ebnf", "ebnf"),
        (
            "reference/build-schema",
            "The BUILD.buri schema",
            "cli/src/docs/reference/schema/build.proto",
            "proto",
        ),
        (
            "reference/repo-schema",
            "The REPO.buri schema",
            "cli/src/docs/reference/schema/repo.proto",
            "proto",
        ),
    ];
    for (route, title, path, language) in NORMATIVE {
        let text = slurp(root, path)?;
        pages.push(Page {
            route: (*route).to_string(),
            title: (*title).to_string(),
            label: path.rsplit('/').next().unwrap_or(path).to_string(),
            summary: format!("Hand-written {language}, held to the implementation by a test."),
            source: Source { path: (*path).to_string(), directory: false },
            section: Some(REFERENCE),
            group: Some(NORMATIVE_GROUP),
            listed: true,
            content: Content::Prose(format!("# {title}\n\n```{language}\n{text}\n```\n")),
            see_also: Vec::new(),
            facts: Vec::new(),
            adapted_from: None,
        });
    }
    Ok(())
}

/// The `---` block a skill page opens with, and the markdown under it. The
/// catalogs' frontmatter is read by `documentation::frontmatter`, which knows
/// the keys a diagnostic page may carry; a skill declares `name` and
/// `description`, which are not among them.
fn split_front_block(text: &str) -> (Option<&str>, &str) {
    let Some(rest) = text.strip_prefix("---\n") else { return (None, text) };
    let mut offset = 0usize;
    for line in rest.split_inclusive('\n') {
        if line.trim_end() == "---" {
            let block = rest.get(..offset).unwrap_or("");
            let body = rest.get(offset.saturating_add(line.len())..).unwrap_or("");
            return (Some(block), body);
        }
        offset = offset.saturating_add(line.len());
    }
    (None, text)
}

fn field(block: &str, key: &str) -> Option<String> {
    for line in block.lines() {
        if let Some((found, value)) = line.split_once(':') {
            if found.trim() == key {
                return Some(value.trim().to_string());
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The id is the reader's, the route is the site's, and the two differ
    /// wherever a topic's shelf moved without its name moving.
    #[test]
    fn a_topic_id_names_the_route_its_page_is_written_to() {
        assert_eq!(route_of_topic("language/lexical").as_deref(), Some("language/lexical"));
        assert_eq!(
            route_of_topic("getting-started/installing").as_deref(),
            Some("getting-started/installing")
        );
        assert_eq!(route_of_topic("guides/testing").as_deref(), Some("guides/testing"));
        assert_eq!(route_of_topic("build/tags").as_deref(), Some("reference/build/tags"));
        assert_eq!(
            route_of_topic("reference/standard-library").as_deref(),
            Some("reference/standard-library")
        );
        assert_eq!(route_of_topic("nonsense"), None);
    }

    /// Every route the site writes has to sit under the section that navigates
    /// to it, or the masthead highlights one section while the reader is in
    /// another.
    #[test]
    fn every_topic_route_is_under_its_section() {
        for topic in topics::TOPICS {
            let route = route_of(topic);
            let section = match topic.kind {
                Kind::GettingStarted => GETTING_STARTED,
                Kind::Guide => GUIDES,
                Kind::Language => LANGUAGE,
                Kind::Build | Kind::Reference => REFERENCE,
            };
            let slug = SECTIONS.get(section).map_or("", |s| s.slug);
            assert!(
                route.starts_with(&format!("{slug}/")),
                "`{}` is served at `{route}`, which is not under `{slug}`",
                topic.id
            );
        }
    }

    /// A group that says it has an index page has one, and it is in the group.
    #[test]
    fn every_indexed_group_has_its_listing_page() {
        for (index, group) in GROUPS.iter().enumerate() {
            let Some(catalogue) = group.index.as_ref() else {
                assert!(group_listing(index).is_none());
                continue;
            };
            let page = group_listing(index).expect("an indexed group has a listing page");
            assert_eq!(page.route, catalogue.route);
            assert_ne!(
                page.title, group.title,
                "the navigation would name `{}` twice over",
                group.title
            );
            assert_eq!(page.group, Some(index));
            assert!(page.listed, "the navigation would show nothing for `{}`", group.title);
        }
    }

    #[test]
    fn a_command_reference_is_demoted_but_a_fence_is_left_alone() {
        let block = command_block("# buri build\n\n```textproto\n# a comment\n```\n\n## Caching\n");
        assert_eq!(block, "## buri build\n\n```textproto\n# a comment\n```\n\n### Caching\n");
    }

    /// The intro is read where it sits and shown where the commands sit, and
    /// only a link that goes somewhere in the tree moves with it.
    #[test]
    fn the_intros_links_climb_the_level_the_command_prose_already_has() {
        let text = "see [it](./build/repo-config.md) and [there](https://example.com) and \
                    [here](#exit-codes)";
        assert_eq!(
            from_one_directory_up(text),
            "see [it](.././build/repo-config.md) and [there](https://example.com) and \
             [here](#exit-codes)"
        );
    }

    /// The three exit codes are the same for every command, so the page says
    /// them once above rather than thirteen times down it.
    #[test]
    fn a_command_block_leaves_the_shared_exit_codes_to_the_intro() {
        let generated = buri::commands::reference(&COMMANDS[0]);
        assert!(generated.contains("Exit codes:"), "the generated reference no longer states them");
        assert!(!command_block(&generated).contains("Exit codes:"));
    }

    /// The anchor a reader writes down is the item's name, so the heading has
    /// to be the name and nothing else — the receiver is said under it.
    #[test]
    fn an_item_is_headed_by_the_name_its_anchor_is_made_from() {
        let item = ApiItem {
            api: Api::Method {
                owner: "[A]".to_string(),
                via_trait: None,
                effects: vec!["Alloc".to_string()],
            },
            name: "map".to_string(),
            signature: "fn map<B, C: Alloc>(self, ctx: C, f: fn(A) => B): [B]".to_string(),
            docs: vec!["Applies `f` to every element.".to_string()],
        };
        let page = item_markdown(&item);
        assert!(page.starts_with("### map\n"), "{page}");
        assert_eq!(buri::documentation::markdown::slug("map"), "map");
        assert!(page.contains("```buri sig\nfn map<B, C: Alloc>"), "{page}");
        assert!(page.contains("A method on `[A]`. Effects: `Alloc`."), "{page}");
    }

    /// A pure function says so, and a struct — which cannot be called — says
    /// neither thing.
    #[test]
    fn purity_is_stated_only_where_it_is_a_statement_about_the_item() {
        let pure = ApiItem {
            api: Api::Function { effects: Vec::new() },
            name: "len".to_string(),
            signature: "fn len(self): Int".to_string(),
            docs: Vec::new(),
        };
        assert!(item_markdown(&pure).contains("\nPure.\n"));

        let structure = ApiItem {
            api: Api::Struct { fields: Vec::new() },
            name: "Stats".to_string(),
            signature: "struct Stats".to_string(),
            docs: Vec::new(),
        };
        let page = item_markdown(&structure);
        assert!(!page.contains("Pure."), "{page}");
        assert!(!page.contains("Effects:"), "{page}");
    }

    /// `core/crypto` opens its module documentation with a `#`, and a page
    /// with two titles on it is a page whose outline is wrong.
    #[test]
    fn prose_a_module_wrote_about_itself_sits_under_the_pages_own_headings() {
        let text = demoted("# What is not here\n\n```buri\n# hidden\n```\n\n## Vectors\n", 1);
        assert_eq!(text, "## What is not here\n\n```buri\n# hidden\n```\n\n### Vectors\n");
        assert_eq!(demoted("###### Deep\n", 2), "###### Deep\n");
        assert_eq!(demoted("#nothing\n", 1), "#nothing\n");
    }

    #[test]
    fn a_front_block_is_split_from_the_body_it_heads() {
        let (front, body) = split_front_block("---\nname: buri-cli\n---\n# Title\n");
        assert_eq!(front, Some("name: buri-cli\n"));
        assert_eq!(body, "# Title\n");
        assert_eq!(field("name: buri-cli\n", "name").as_deref(), Some("buri-cli"));
    }
}
