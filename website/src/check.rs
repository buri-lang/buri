//! `--check`: every link the site wrote, followed.
//!
//! The links checked are the ones that were *written*, not the ones that were
//! meant: an href is resolved back to a route the way a browser would resolve
//! it, and the file that route names has to be on disk. So a relative path
//! that climbs one directory too few fails here rather than in somebody's
//! browser.
//!
//! The "edit on GitHub" paths are checked too. They are the one kind of link
//! whose target is not in the output, and a page that offers to edit a file
//! that was renamed is worse than a page that offers nothing.

use crate::links::{href_from, route_of_href, Target};
use crate::pages::Site;
use crate::render::PageLinks;
use std::path::Path;

/// Everything wrong with a built site, one line each. An empty list is a site
/// whose every internal link resolves.
pub fn problems(site: &Site, built: &[PageLinks], out: &Path) -> Vec<String> {
    let mut found = Vec::new();
    if !out.join("assets/site.css").is_file() {
        found.push("the stylesheet was not written to assets/site.css".to_string());
    }
    for page in built {
        let where_ =
            if page.route.is_empty() { "the front page" } else { page.route.as_str() };
        check_source(site, page, where_, &mut found);
        for target in &page.targets {
            check_target(site, built, out, page, target, where_, &mut found);
        }
    }
    found
}

/// The file a page offers to edit has to be the file it was generated from,
/// and that file has to exist.
fn check_source(site: &Site, page: &PageLinks, where_: &str, found: &mut Vec<String>) {
    let path = site.root.join(&page.source.path);
    let exists = if page.source.directory { path.is_dir() } else { path.is_file() };
    if !exists {
        let kind = if page.source.directory { "directory" } else { "file" };
        found.push(format!(
            "{where_}: its \"edit on GitHub\" link names {}, which is not a {kind} in the tree",
            page.source.path
        ));
    }
}

fn check_target(
    site: &Site,
    built: &[PageLinks],
    out: &Path,
    page: &PageLinks,
    target: &Target,
    where_: &str,
    found: &mut Vec<String>,
) {
    match target {
        Target::External { .. } => {}
        Target::Nowhere => {
            found.push(format!("{where_}: a link points nowhere — `[text]()`"));
        }
        Target::SameDocument { anchor } => {
            if !page.anchors.contains(anchor) {
                found.push(format!("{where_}: `#{anchor}` is not a heading on this page"));
            }
        }
        Target::Repository { path, directory } => {
            let full = site.root.join(path);
            let exists = if *directory { full.is_dir() } else { full.exists() };
            if !exists {
                found.push(format!("{where_}: links to `{path}`, which is not in the tree"));
            }
        }
        Target::Page { route, anchor } => {
            // Resolved through the href rather than through the route, so a
            // wrong number of `..` is a failure rather than a near miss.
            let Some(href) = href_from(&page.route, target) else { return };
            let Some(resolved) = route_of_href(&page.route, &href) else {
                found.push(format!("{where_}: `{href}` climbs out of the site"));
                return;
            };
            if &resolved != route {
                found.push(format!(
                    "{where_}: `{href}` resolves to `{resolved}` rather than to `{route}`"
                ));
                return;
            }
            let file = out.join(&resolved).join("index.html");
            if !file.is_file() {
                found.push(format!("{where_}: `{href}` names a page that was not written"));
                return;
            }
            let Some(anchor) = anchor else { return };
            let Some(other) = built.iter().find(|other| &other.route == route) else { return };
            if !other.anchors.contains(anchor) {
                found.push(format!("{where_}: `#{anchor}` is not a heading on `{route}`"));
            }
        }
    }
}
