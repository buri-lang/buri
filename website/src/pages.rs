//! What the site is made of.
//!
//! The inventory is the toolchain's own: `documentation::topics` says which
//! prose pages exist and in what order, `commands::COMMANDS` says which
//! commands have a page, and `documentation::errors` and `lints` say which
//! codes do. A page the site is missing is therefore a page `buri docs` is
//! missing too, which is the only way the two can be held together without a
//! second list to keep in step.
//!
//! The *text* is read from `cli/src/docs/**` on disk rather than from the
//! copies compiled into `buri`, so that editing a page and re-running the
//! generator shows the edit — and so that `--watch` has something to watch.

use buri::commands::add::skills::SKILLS;
use buri::commands::COMMANDS;
use buri::documentation::frontmatter;
use buri::documentation::markdown;
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
        slug: "guide",
        title: "Start here",
        blurb: "Prose that introduces rather than specifies: what Buri is, what is in it, \
                and the three ideas the rest follows from.",
        directory: "cli/src/docs/guide",
    },
    Section {
        slug: "language",
        title: "The language",
        blurb: "The reference, section by section, and the whole specification on one page.",
        directory: "cli/src/docs/lang",
    },
    Section {
        slug: "build",
        title: "The build system",
        blurb: "The monorepo, the build files, the tags, and what the cache promises.",
        directory: "cli/src/docs/build",
    },
    Section {
        slug: "cli",
        title: "The CLI",
        blurb: "One page per `buri` subcommand.",
        directory: "cli/src/docs/cli",
    },
    Section {
        slug: "errors",
        title: "Diagnostics",
        blurb: "Every code the compiler emits, what the rule is, and what to do about it.",
        directory: "cli/src/docs/errors",
    },
    Section {
        slug: "lints",
        title: "Lints",
        blurb: "What `buri lint` reports that type checking does not.",
        directory: "cli/src/docs/lints",
    },
    Section {
        slug: "skills",
        title: "Agent skills",
        blurb: "The skills `buri add skills` writes, for a coding agent working in a \
                Buri repository.",
        directory: "cli/src/docs/skills",
    },
    Section {
        slug: "reference",
        title: "Normative artifacts",
        blurb: "The grammar and the build-file schemas, hand-written and held to the \
                implementation by a test.",
        directory: "cli/src/docs/schema",
    },
];

const GUIDE: usize = 0;
const LANGUAGE: usize = 1;
const BUILD: usize = 2;
const CLI: usize = 3;
const ERRORS: usize = 4;
const LINTS: usize = 5;
const SKILLS_SECTION: usize = 6;
const REFERENCE: usize = 7;

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
}

pub struct Page {
    /// The site path. `""` is the front page; every other page is written to
    /// `<route>/index.html`.
    pub route: String,
    /// The heading, and what the navigation calls it.
    pub title: String,
    /// What a reader types at `buri docs`: `lang/lexical`, `circular-import`.
    /// Empty where there is nothing to type.
    pub label: String,
    /// The first sentence, for a listing and for the page description.
    pub summary: String,
    pub source: Source,
    pub section: Option<usize>,
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

    /// The route of the section whose pages live in a repository directory.
    pub fn route_of_directory(&self, path: &str) -> Option<&str> {
        let path = path.trim_end_matches('/');
        SECTIONS.iter().find(|s| s.directory == path).map(|s| s.slug)
    }

    pub fn page(&self, route: &str) -> Option<&Page> {
        self.pages.iter().find(|p| p.route == route)
    }

    /// The pages of one section, in the order the navigation lists them.
    pub fn in_section(&self, section: usize) -> impl Iterator<Item = &Page> {
        self.pages.iter().filter(move |p| p.section == Some(section) && !p.is_index())
    }

    /// Every file the site is generated from, for `--watch` to poll.
    pub fn sources(&self) -> Vec<PathBuf> {
        let mut out: Vec<PathBuf> = self
            .pages
            .iter()
            .filter(|p| !p.source.directory)
            .map(|p| self.root.join(&p.source.path))
            .collect();
        out.sort();
        out.dedup();
        out
    }
}

impl Page {
    pub fn is_index(&self) -> bool {
        matches!(self.content, Content::Listing(_))
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
pub fn route_of_topic(id: &str) -> Option<String> {
    let (kind, name) = id.split_once('/')?;
    let section = match kind {
        "guide" => "guide",
        "lang" => "language",
        "build" => "build",
        _ => return None,
    };
    Some(format!("{section}/{name}"))
}

/// Reads every page. The only failure is a missing file, which means the
/// registry and the tree disagree — a broken checkout, not a broken page.
pub fn read(root: &Path) -> Result<Site, String> {
    let mut pages = Vec::new();
    pages.push(front_page(root)?);
    for section in 0..SECTIONS.len() {
        pages.push(listing(section));
    }
    read_topics(root, &mut pages)?;
    pages.push(specification(root)?);
    read_commands(root, &mut pages)?;
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
        content: Content::Listing(section),
        see_also: Vec::new(),
        facts: Vec::new(),
        adapted_from: None,
    }
}

fn read_topics(root: &Path, pages: &mut Vec<Page>) -> Result<(), String> {
    for topic in topics::TOPICS {
        let (kind, name) = topic.id.split_once('/').unwrap_or((topic.id, topic.id));
        let section = match topic.kind {
            Kind::Guide => GUIDE,
            Kind::Lang => LANGUAGE,
            Kind::Build => BUILD,
        };
        let path = format!("cli/src/docs/{kind}/{name}.md");
        let text = slurp(root, &path)?;
        let summary = markdown::summary(&text);
        pages.push(Page {
            route: format!("{}/{name}", section_slug(section)),
            title: topic.title.to_string(),
            label: topic.id.to_string(),
            summary,
            source: Source { path, directory: false },
            section: Some(section),
            content: Content::Prose(text),
            see_also: topic.see_also.iter().filter_map(|id| route_of_topic(id)).collect(),
            facts: Vec::new(),
            adapted_from: None,
        });
    }
    Ok(())
}

/// The assembled specification. It is the `lang/` topics end to end, and it is
/// a page of its own because twenty-nine doc links point at `../SPEC.md` and
/// because a reader who wants the whole language wants one document.
fn specification(root: &Path) -> Result<Page, String> {
    let path = "cli/src/docs/SPEC.md";
    let text = slurp(root, path)?;
    Ok(Page {
        route: "language/specification".to_string(),
        title: "The specification, on one page".to_string(),
        label: "SPEC.md".to_string(),
        summary: "Every `lang/` section end to end, assembled by `buri docs assemble`."
            .to_string(),
        source: Source { path: path.to_string(), directory: false },
        section: Some(LANGUAGE),
        content: Content::Prose(text),
        see_also: Vec::new(),
        facts: Vec::new(),
        adapted_from: None,
    })
}

fn read_commands(root: &Path, pages: &mut Vec<Page>) -> Result<(), String> {
    for command in COMMANDS {
        let path = format!("cli/src/docs/cli/{}.md", command.name);
        let text = slurp(root, &path)?;
        pages.push(Page {
            route: format!("cli/{}", command.name),
            title: format!("buri {}", command.name),
            label: format!("cli/{}", command.name),
            summary: command.blurb.to_string(),
            source: Source { path, directory: false },
            section: Some(CLI),
            content: Content::Prose(text),
            see_also: Vec::new(),
            facts: Vec::new(),
            adapted_from: None,
        });
    }
    Ok(())
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
    for (section, directory, code, listed_title, see_also) in errors.chain(lints) {
        let path = format!("cli/src/docs/{directory}/{code}.md");
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
            route: format!("{directory}/{code}"),
            title,
            label: code.to_string(),
            summary,
            source: Source { path, directory: false },
            section: Some(section),
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
        let path = format!("cli/src/docs/skills/{}.md", skill.name);
        let text = slurp(root, &path)?;
        let (front, body) = split_front_block(&text);
        let description = front.and_then(|block| field(block, "description"));
        let title = markdown::headings(body)
            .first()
            .map(|heading| heading.title.to_string())
            .unwrap_or_else(|| skill.name.to_string());
        pages.push(Page {
            route: format!("skills/{}", skill.name),
            title,
            label: skill.name.to_string(),
            summary: description.clone().unwrap_or_else(|| markdown::summary(body)),
            source: Source { path, directory: false },
            section: Some(SKILLS_SECTION),
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
            "cli/src/docs/schema/build.proto",
            "proto",
        ),
        (
            "reference/repo-schema",
            "The REPO.buri schema",
            "cli/src/docs/schema/repo.proto",
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
            content: Content::Prose(format!("# {title}\n\n```{language}\n{text}\n```\n")),
            see_also: Vec::new(),
            facts: Vec::new(),
            adapted_from: None,
        });
    }
    Ok(())
}

fn section_slug(section: usize) -> &'static str {
    SECTIONS.get(section).map(|s| s.slug).unwrap_or("")
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

    #[test]
    fn every_section_slug_is_the_first_segment_of_its_routes() {
        for (index, section) in SECTIONS.iter().enumerate() {
            assert_eq!(section_slug(index), section.slug);
        }
    }

    #[test]
    fn a_topic_id_names_the_route_its_page_is_written_to() {
        assert_eq!(route_of_topic("lang/lexical").as_deref(), Some("language/lexical"));
        assert_eq!(route_of_topic("guide/goals").as_deref(), Some("guide/goals"));
        assert_eq!(route_of_topic("build/tags").as_deref(), Some("build/tags"));
        assert_eq!(route_of_topic("nonsense"), None);
    }

    #[test]
    fn a_front_block_is_split_from_the_body_it_heads() {
        let (front, body) = split_front_block("---\nname: buri-cli\n---\n# Title\n");
        assert_eq!(front, Some("name: buri-cli\n"));
        assert_eq!(body, "# Title\n");
        assert_eq!(field("name: buri-cli\n", "name").as_deref(), Some("buri-cli"));
    }
}
