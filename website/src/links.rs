//! Where a link in a documentation page points once the page is a web page.
//!
//! A doc link is written relative to the file it is in — `./tags.md#lint`,
//! `../SPEC.md`, `./design/` — because that is what resolves on GitHub, and
//! `cli/tests/docs` holds the corpus to it. The site keeps every one of them
//! working: a destination that names a page becomes a link to that page, and
//! everything else becomes a link to the file or directory on GitHub.

use crate::pages::{Page, Site};

/// The repository the "edit on GitHub" links and the fallbacks point at.
pub const REPOSITORY: &str = "https://github.com/buri-lang/buri";

/// The branch those links name.
pub const BRANCH: &str = "main";

/// Where one destination resolves to.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Target {
    /// A heading in the page the link was written in.
    SameDocument { anchor: String },
    /// Another page of the site.
    Page { route: String, anchor: Option<String> },
    /// A file or a directory in the repository, which the site does not
    /// publish. It is still a real path, and `--check` says so if it is not.
    Repository { path: String, directory: bool },
    /// Somewhere else entirely. Nothing here resolves it.
    External { url: String },
    /// `[text]()`. Rendered as its own text, with no link around it.
    Nowhere,
}

/// One page's view of the site, for the renderer to resolve against.
///
/// The two halves are separate because they can come from different pages: a
/// section's listing shows the first sentence of every page under it, and a
/// link in that sentence was written where the sentence was, while the `href`
/// has to be written where it is being shown.
pub struct Resolver<'a> {
    pub site: &'a Site,
    /// The file destinations were written relative to.
    pub base: String,
    /// The route the `href` is written on.
    pub route: String,
}

impl<'a> Resolver<'a> {
    /// Resolving a page's own prose.
    pub fn for_page(site: &'a Site, page: &Page) -> Resolver<'a> {
        Resolver { site, base: base_of(page), route: page.route.clone() }
    }

    /// Resolving prose lifted from `written` onto the page at `shown`.
    pub fn for_quotation(site: &'a Site, shown: &Page, written: &Page) -> Resolver<'a> {
        Resolver { site, base: base_of(written), route: shown.route.clone() }
    }

    /// Where a destination written in this page points.
    pub fn classify(&self, destination: &str) -> Target {
        if destination.starts_with("http://")
            || destination.starts_with("https://")
            || destination.starts_with("mailto:")
        {
            return Target::External { url: destination.to_string() };
        }
        if let Some(anchor) = destination.strip_prefix('#') {
            return Target::SameDocument { anchor: anchor.to_string() };
        }
        if destination.is_empty() {
            return Target::Nowhere;
        }
        let (path, anchor) = match destination.split_once('#') {
            Some((path, anchor)) => (path, Some(anchor.to_string())),
            None => (destination, None),
        };
        let directory = path.ends_with('/');
        let resolved = resolve(&self.base, path);
        if let Some(route) = self.site.route_of_source(&resolved) {
            return Target::Page { route: route.to_string(), anchor };
        }
        if let Some(route) = self.site.route_of_directory(&resolved) {
            return Target::Page { route: route.to_string(), anchor };
        }
        Target::Repository { path: resolved, directory }
    }

    /// The `href` a target is written as, from this page.
    pub fn href(&self, target: &Target) -> Option<String> {
        href_from(&self.route, target)
    }
}

/// The file a page's links are written relative to.
///
/// `cli/src/docs/SPEC.md` and the `lang/` topics it is assembled from write
/// theirs from the repository root, and everything else writes them from
/// where the file sits. That is not this crate's rule — it is the corpus's,
/// stated as `ROOT_RELATIVE` in `cli/tests/docs/documents.rs`, and read here
/// from the other side so that the two agree about what resolves.
fn base_of(page: &Page) -> String {
    if is_root_relative(&page.source.path) {
        return String::new();
    }
    if page.source.directory {
        // `resolve` reads its base as a file and drops the last segment, so a
        // directory needs one to drop.
        return format!("{}/index.md", page.source.path);
    }
    page.source.path.clone()
}

fn is_root_relative(source: &str) -> bool {
    source == "cli/src/docs/SPEC.md" || source.starts_with("cli/src/docs/lang/")
}

/// The `href` a target is written as, from a page at `route`.
pub fn href_from(route: &str, target: &Target) -> Option<String> {
    match target {
        Target::SameDocument { anchor } => Some(format!("#{anchor}")),
        Target::Page { route: to, anchor } => {
            let mut href = relative(route, to);
            if let Some(anchor) = anchor {
                href.push('#');
                href.push_str(anchor);
            }
            Some(href)
        }
        Target::Repository { path, directory } => Some(repository_url(path, *directory)),
        Target::External { url } => Some(url.clone()),
        Target::Nowhere => None,
    }
}

/// The GitHub URL for a path in the tree. A directory is browsed, a file is
/// read; naming the wrong one gives a page that loads and says nothing.
pub fn repository_url(path: &str, directory: bool) -> String {
    let kind = if directory { "tree" } else { "blob" };
    let path = path.trim_end_matches('/');
    format!("{REPOSITORY}/{kind}/{BRANCH}/{path}")
}

/// A path written relative to `from`, normalized against the repository root.
///
/// `from` is a file, so the path is resolved against the directory holding it.
fn resolve(from: &str, path: &str) -> String {
    let mut segments: Vec<&str> = match from.rsplit_once('/') {
        Some((directory, _)) => directory.split('/').collect(),
        None => Vec::new(),
    };
    for segment in path.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                segments.pop();
            }
            other => segments.push(other),
        }
    }
    segments.join("/")
}

/// The href from one route to another. Every page is written to
/// `<route>/index.html`, so a route of two segments sits two directories down
/// and reaches the root with two `..`.
///
/// Relative rather than absolute so that the site works unchanged at the root
/// of a domain, under a project path, and opened straight off the disk.
pub fn relative(from: &str, to: &str) -> String {
    let depth = if from.is_empty() { 0 } else { from.split('/').count() };
    let mut href = String::new();
    for _ in 0..depth {
        href.push_str("../");
    }
    if !to.is_empty() {
        href.push_str(to);
        href.push('/');
    }
    if href.is_empty() {
        return "./".to_string();
    }
    href
}

/// The route an href written on `from` names, or `None` if it leaves the site.
///
/// This is [`relative`] read backwards, and it is what `--check` walks: the
/// link it verifies is the one that was written, not the one that was meant.
pub fn route_of_href(from: &str, href: &str) -> Option<String> {
    let href = href.split('#').next().unwrap_or(href);
    let mut segments: Vec<&str> = if from.is_empty() {
        Vec::new()
    } else {
        from.split('/').collect()
    };
    for segment in href.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                segments.pop()?;
            }
            other => segments.push(other),
        }
    }
    Some(segments.join("/"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_relative_path_resolves_against_the_file_it_was_written_in() {
        assert_eq!(resolve("cli/src/docs/build/tags.md", "./testing.md"), "cli/src/docs/build/testing.md");
        assert_eq!(resolve("cli/src/docs/lang/types.md", "../SPEC.md"), "cli/src/docs/SPEC.md");
        assert_eq!(resolve("README.md", "./cli/src/docs/grammar.ebnf"), "cli/src/docs/grammar.ebnf");
        assert_eq!(resolve("cli/src/docs/build/tags.md", "../../../tests/example/"), "cli/tests/example");
    }

    #[test]
    fn an_href_climbs_out_of_the_directory_the_page_is_written_into() {
        assert_eq!(relative("", "guide/installing"), "guide/installing/");
        assert_eq!(relative("errors/circular-import", "language/lexical"), "../../language/lexical/");
        assert_eq!(relative("guide/installing", ""), "../../");
        assert_eq!(relative("", ""), "./");
    }

    /// Every href the site writes has to be readable by the link checker, so
    /// the two directions are one another's inverse.
    #[test]
    fn every_href_reads_back_as_the_route_it_names() {
        let routes = ["", "guide", "guide/installing", "errors/circular-import", "reference/grammar"];
        for from in routes {
            for to in routes {
                let href = relative(from, to);
                assert_eq!(
                    route_of_href(from, &href).as_deref(),
                    Some(to),
                    "`{href}` written on `{from}` should name `{to}`"
                );
            }
        }
    }

    #[test]
    fn a_repository_path_is_browsed_or_read_by_what_it_is() {
        assert_eq!(
            repository_url("design/", true),
            "https://github.com/buri-lang/buri/tree/main/design"
        );
        assert_eq!(
            repository_url("README.md", false),
            "https://github.com/buri-lang/buri/blob/main/README.md"
        );
    }
}
