//! One page, as a whole HTML document.
//!
//! Every page carries the same chrome — the section bar, the theme picker, the
//! sidebar for the section it is in — and the same two inline scripts. There
//! is no external JavaScript and no highlighter in the browser, and the
//! stylesheet is written into the document rather than linked: a page is one
//! file, and fetches nothing at all to render itself.

use crate::highlight::escaped;
use crate::links::{href_from, Resolver, Target};
use crate::markdown;
use crate::pages::{Content, Page, Site, Source, GROUPS, SECTIONS};
use crate::themes;

/// What one rendered page pointed at, so that `--check` can walk it without
/// parsing the HTML back out again.
pub struct PageLinks {
    pub route: String,
    pub source: Source,
    pub anchors: Vec<String>,
    pub targets: Vec<Target>,
}

/// The `localStorage` key the picker writes and the boot script reads.
const THEME_KEY: &str = "buri-theme";

/// Runs in `<head>`, before anything is painted, so a reader who chose a
/// scheme never sees the default one first.
fn boot_script() -> String {
    format!(
        "(function(){{try{{var t=localStorage.getItem(\"{THEME_KEY}\");\
         if(t)document.documentElement.setAttribute(\"data-theme\",t)}}catch(e){{}}}})()"
    )
}

/// One page's document, and the links it turned out to contain.
pub fn document(site: &Site, page: &Page) -> (String, PageLinks) {
    let resolver = Resolver::for_page(site, page);
    let mut targets: Vec<Target> = Vec::new();
    let mut anchors: Vec<String> = vec!["content".to_string()];

    let mut heading = String::new();
    let body = match &page.content {
        Content::Prose(text) => {
            let rendered = markdown::render(text, &resolver);
            anchors.extend(rendered.anchors);
            targets.extend(rendered.links);
            if let Some((anchor, html)) = own_heading(text, &page.title) {
                anchors.push(anchor);
                heading = html;
            }
            rendered.html
        }
        Content::Listing(section) => {
            let entry = SECTIONS.get(*section);
            let title = entry.map_or("", |s| s.title);
            let blurb = entry.map_or("", |s| s.blurb);
            let mut out = titled(title, blurb, &resolver, &mut targets, &mut anchors);
            out.push_str(&entries(site, page, site.in_section(*section), &mut targets));
            out
        }
        Content::Groups => {
            let entry = page.section.and_then(|index| SECTIONS.get(index));
            let title = entry.map_or("", |s| s.title);
            let blurb = entry.map_or("", |s| s.blurb);
            let mut out = titled(title, blurb, &resolver, &mut targets, &mut anchors);
            for (index, group) in GROUPS.iter().enumerate() {
                let anchor = buri::documentation::markdown::slug(group.title);
                // A group whose whole navigation is its own index page is that
                // link: listing it under a heading of the same name would say
                // the name twice and the sentence twice.
                let heading = match &group.index {
                    Some(catalogue) => {
                        link(page, catalogue.route, group.title, false, &mut targets)
                    }
                    None => markdown::title(group.title),
                };
                out.push_str(&format!("<h2 id=\"{}\">{heading}</h2>\n", escaped(&anchor)));
                anchors.push(anchor);
                let blurb = markdown::render_inline(group.blurb, &resolver);
                targets.extend(blurb.links);
                out.push_str(&format!("<p>{}</p>\n", blurb.html));
                if group.index.is_none() {
                    out.push_str(&entries(site, page, site.navigation_of(index), &mut targets));
                }
            }
            out
        }
        Content::GroupListing(group) => {
            let blurb = GROUPS.get(*group).map_or("", |g| g.blurb);
            let mut out = titled(&page.title, blurb, &resolver, &mut targets, &mut anchors);
            out.push_str(&entries(site, page, site.in_group(*group), &mut targets));
            out
        }
    };

    let mut facts = String::new();
    if !page.facts.is_empty() {
        facts.push_str("<dl class=\"facts\">\n");
        for fact in &page.facts {
            let rendered = markdown::render_inline(&fact.value, &resolver);
            targets.extend(rendered.links);
            facts.push_str(&format!(
                "<dt>{}</dt><dd>{}</dd>\n",
                escaped(fact.term),
                rendered.html
            ));
        }
        facts.push_str("</dl>\n");
    }

    let stylesheet = themes::inline_stylesheet();
    let mut out = String::with_capacity(
        body.len().saturating_add(stylesheet.len()).saturating_add(16 * 1024),
    );
    out.push_str("<!doctype html>\n<html lang=\"en\" data-theme=\"");
    out.push_str(themes::DEFAULT);
    out.push_str("\">\n<head>\n<meta charset=\"utf-8\">\n");
    out.push_str("<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n");
    out.push_str(&format!("<title>{}</title>\n", escaped(&title_of(page))));
    out.push_str(&format!(
        "<meta name=\"description\" content=\"{}\">\n",
        escaped(&one_line(&page.summary))
    ));
    out.push_str(&format!("<script>{}</script>\n", boot_script()));
    out.push_str(&format!("<style>{stylesheet}</style>\n"));
    out.push_str("</head>\n<body>\n");
    out.push_str("<a class=\"skip\" href=\"#content\">Skip to content</a>\n");
    out.push_str(&masthead(page, &mut targets));
    out.push_str("<div class=\"frame\">\n");
    out.push_str(&sidebar(site, page, &mut targets));
    out.push_str("<main id=\"content\">\n<article>\n");
    out.push_str(&crumbs(page, &mut targets));
    out.push_str(&heading);
    out.push_str(&facts);
    out.push_str(&body);
    if let Some(source) = &page.adapted_from {
        out.push_str(&format!("<p class=\"adapted\">Adapted from {}.</p>\n", escaped(source)));
    }
    out.push_str(&also(site, page, &mut targets));
    out.push_str("</article>\n");
    out.push_str(&colophon(page, &mut targets));
    out.push_str("</main>\n</div>\n");
    out.push_str(&format!("<script>{NAVIGATION_SCRIPT}</script>\n"));
    out.push_str("</body>\n</html>\n");

    (out, PageLinks { route: page.route.clone(), source: page.source.clone(), anchors, targets })
}

/// The heading a page needs and its own text does not give it.
///
/// A `lang/` topic opens with its own numbered heading, and a diagnostic page
/// whose body is still to be written opens with nothing at all. The first is
/// already titled; the second, and every command page — which opens at "What
/// it does" — is not, and a page whose title appears only in the browser tab
/// is a page a reader has to guess at.
///
/// The comparison is `topics.rs`'s own `every_title_matches_its_first_heading`,
/// which is what makes "already titled" mean the same thing in both places.
fn own_heading(text: &str, title: &str) -> Option<(String, String)> {
    let normalize = |text: &str| text.replace(['`', ':'], "").to_lowercase();
    let first = buri::documentation::markdown::headings(text).first().map(|heading| {
        heading.title.trim_start_matches(|c: char| c.is_ascii_digit() || c == '.').trim().to_string()
    });
    if let Some(written) = first {
        let (written, wanted) = (normalize(&written), normalize(title));
        if written.starts_with(&wanted) || wanted.starts_with(&written) {
            return None;
        }
    }
    let anchor = buri::documentation::markdown::slug(title);
    let html = format!(
        "<h1 id=\"{}\">{}<a class=\"anchor\" href=\"#{}\" aria-label=\"Link to this section\">#</a></h1>\n",
        escaped(&anchor),
        markdown::title(title),
        escaped(&anchor),
    );
    Some((anchor, html))
}

fn title_of(page: &Page) -> String {
    if page.route.is_empty() {
        "Buri — a strict, purely functional language".to_string()
    } else {
        format!("{} — Buri", one_line(&page.title))
    }
}

/// A description meta tag is one line, and a summary lifted from prose is
/// whatever the prose was.
fn one_line(text: &str) -> String {
    let flattened: String =
        text.chars().map(|c| if c.is_whitespace() { ' ' } else { c }).collect();
    let mut squeezed = String::with_capacity(flattened.len());
    let mut space = false;
    for character in flattened.chars() {
        if character == ' ' {
            space = true;
            continue;
        }
        if space && !squeezed.is_empty() {
            squeezed.push(' ');
        }
        space = false;
        squeezed.push(character);
    }
    squeezed.replace('`', "")
}

fn link(page: &Page, route: &str, label: &str, current: bool, targets: &mut Vec<Target>) -> String {
    let target = Target::Page { route: route.to_string(), anchor: None };
    let href = href_from(&page.route, &target).unwrap_or_default();
    targets.push(target);
    let mark = if current { " aria-current=\"page\"" } else { "" };
    format!("<a href=\"{}\"{mark}>{}</a>", escaped(&href), markdown::title(label))
}

fn masthead(page: &Page, targets: &mut Vec<Target>) -> String {
    let mut out = String::from("<header class=\"masthead\">\n");
    let home = Target::Page { route: String::new(), anchor: None };
    let href = href_from(&page.route, &home).unwrap_or_default();
    targets.push(home);
    out.push_str(&format!("<a class=\"wordmark\" href=\"{}\">Buri</a>\n", escaped(&href)));
    out.push_str("<nav aria-label=\"Sections\">\n");
    for (index, section) in SECTIONS.iter().enumerate() {
        let current = page.section == Some(index);
        let target = Target::Page { route: section.slug.to_string(), anchor: None };
        let href = href_from(&page.route, &target).unwrap_or_default();
        targets.push(target);
        let mark = if current { " aria-current=\"true\"" } else { "" };
        out.push_str(&format!(
            "<a href=\"{}\"{mark}>{}</a>\n",
            escaped(&href),
            escaped(section.title)
        ));
    }
    out.push_str("</nav>\n");
    out.push_str(&picker());
    out.push_str("</header>\n");
    out
}

/// The scheme picker: a labelled `<select>`, which is keyboard-operable
/// everywhere without a line of script to make it so.
fn picker() -> String {
    let mut out = String::from(
        "<div class=\"theme\">\n<label for=\"theme-picker\">Theme</label>\n\
         <select id=\"theme-picker\" data-theme-picker>\n",
    );
    for theme in themes::THEMES {
        out.push_str(&format!(
            "<option value=\"{}\">{}</option>\n",
            escaped(theme.id),
            escaped(theme.name)
        ));
    }
    out.push_str("</select>\n</div>\n");
    out
}

/// The section the reader is in, listed page by page. On the front page there
/// is no section to list, so the sections themselves are.
///
/// The reference is listed by group instead. It is the one section whose pages
/// do not fit a list — a sidebar holding every error code holds nothing a
/// reader can find, and the code pages themselves want the same sidebar as the
/// rest of the reference rather than a list of their two hundred and twenty
/// siblings.
fn sidebar(site: &Site, page: &Page, targets: &mut Vec<Target>) -> String {
    let mut out = String::from("<nav class=\"sidebar\" aria-label=\"Documentation\">\n");
    match page.section.and_then(|index| SECTIONS.get(index).map(|s| (index, s))) {
        Some((index, section)) => {
            out.push_str(&format!("<div>\n<h2>{}</h2>\n<ul>\n", escaped(section.title)));
            out.push_str(&format!(
                "<li>{}</li>\n",
                link(page, section.slug, "Overview", page.route == section.slug, targets)
            ));
            if !grouped(index) {
                for other in site.in_section(index) {
                    out.push_str(&format!(
                        "<li>{}</li>\n",
                        link(page, &other.route, &other.title, other.route == page.route, targets)
                    ));
                }
            }
            out.push_str("</ul>\n</div>\n");
            if grouped(index) {
                for (group, entry) in GROUPS.iter().enumerate() {
                    out.push_str(&format!("<div>\n<h2>{}</h2>\n<ul>\n", escaped(entry.title)));
                    for other in site.navigation_of(group) {
                        let here = other.route == page.route
                            || (page.group == Some(group) && !page.listed && other.is_index());
                        out.push_str(&format!(
                            "<li>{}</li>\n",
                            link(page, &other.route, &other.title, here, targets)
                        ));
                    }
                    out.push_str("</ul>\n</div>\n");
                }
            }
        }
        None => {
            out.push_str("<div>\n<h2>Sections</h2>\n<ul>\n");
            for section in SECTIONS {
                out.push_str(&format!(
                    "<li>{}</li>\n",
                    link(page, section.slug, section.title, false, targets)
                ));
            }
            out.push_str("</ul>\n</div>\n");
        }
    }
    out.push_str("</nav>\n");
    out
}

/// Whether a section navigates by group. Read from the pages themselves, so
/// the answer is "some page in it is in a group" rather than a second list of
/// which section that is.
fn grouped(section: usize) -> bool {
    SECTIONS.get(section).is_some_and(|s| s.slug == "reference")
}

fn crumbs(page: &Page, targets: &mut Vec<Target>) -> String {
    let Some(section) = page.section.and_then(|index| SECTIONS.get(index)) else {
        return String::new();
    };
    if page.route == section.slug {
        return String::new();
    }
    let mut out =
        format!("<p class=\"crumbs\">{}", link(page, section.slug, section.title, false, targets));
    if let Some(group) = page.group.and_then(|index| GROUPS.get(index)) {
        match &group.index {
            Some(catalogue) if catalogue.route != page.route => {
                out.push_str(&format!(
                    " · {}",
                    link(page, catalogue.route, group.title, false, targets)
                ));
            }
            Some(_) => {}
            None => out.push_str(&format!(" · {}", escaped(group.title))),
        }
    }
    if !page.label.is_empty() {
        out.push_str(&format!(" · <code>{}</code>", escaped(&page.label)));
    }
    out.push_str("</p>\n");
    out
}

fn also(site: &Site, page: &Page, targets: &mut Vec<Target>) -> String {
    if page.see_also.is_empty() {
        return String::new();
    }
    let mut out = String::from("<nav class=\"also\">\n<h2>See also</h2>\n<ul>\n");
    for route in &page.see_also {
        let Some(other) = site.page(route) else { continue };
        out.push_str(&format!("<li>{}</li>\n", link(page, route, &other.title, false, targets)));
    }
    out.push_str("</ul>\n</nav>\n");
    out
}

fn colophon(page: &Page, targets: &mut Vec<Target>) -> String {
    let target =
        Target::Repository { path: page.source.path.clone(), directory: page.source.directory };
    let href = href_from(&page.route, &target).unwrap_or_default();
    targets.push(target);
    let verb = if page.source.directory { "Browse this section" } else { "Edit this page" };
    format!(
        "<footer class=\"colophon\">\n\
         <a href=\"{}\" rel=\"noreferrer\">{verb} on GitHub</a>\n\
         <span><code>{}</code></span>\n\
         </footer>\n",
        escaped(&href),
        escaped(&page.source.path)
    )
}

/// The heading and the blurb an index page opens with.
fn titled(
    title: &str,
    blurb: &str,
    resolver: &Resolver<'_>,
    targets: &mut Vec<Target>,
    anchors: &mut Vec<String>,
) -> String {
    let anchor = buri::documentation::markdown::slug(title);
    let mut out = format!(
        "<h1 id=\"{}\">{}</h1>\n",
        escaped(&anchor),
        markdown::title(title)
    );
    anchors.push(anchor);
    let blurb = markdown::render_inline(blurb, resolver);
    targets.extend(blurb.links);
    out.push_str(&format!("<p>{}</p>\n", blurb.html));
    out
}

/// A list of pages, each with what it is: the body of every index page the
/// site writes, whether it is listing a section, a group, or a catalogue.
fn entries<'a>(
    site: &Site,
    page: &Page,
    pages: impl Iterator<Item = &'a Page>,
    targets: &mut Vec<Target>,
) -> String {
    let mut out = String::from("<ul class=\"listing\">\n");
    for other in pages {
        out.push_str("<li>\n");
        out.push_str(&link(page, &other.route, &other.title, false, targets));
        if !other.label.is_empty() {
            out.push_str(&format!("<code>{}</code>", escaped(&other.label)));
        }
        if !other.summary.trim().is_empty() {
            // The sentence was written on the other page, so a link in it
            // resolves from there and is written from here.
            let quoted = Resolver::for_quotation(site, page, other);
            let summary = markdown::render_inline(&other.summary, &quoted);
            targets.extend(summary.links);
            out.push_str(&format!("\n<p>{}</p>", summary.html));
        }
        out.push_str("\n</li>\n");
    }
    out.push_str("</ul>\n");
    out
}

/// Instant page transitions, the pages fetched ahead on hover, and the theme
/// the reader chose.
///
/// It hardens the obvious sketch. A click is intercepted only when the browser
/// would otherwise do a plain same-origin navigation: a modified click, a
/// middle click, a download, a new tab, an anchor on the page itself and
/// anything off-site are all left alone. `popstate` runs the same swap, so
/// back and forward behave like the forward direction. A fetch that fails for
/// any reason — offline, a 404, a page opened over `file://`, where `fetch`
/// cannot read a sibling at all — falls through to a real navigation, so the
/// worst case is the site without the enhancement rather than a dead link.
///
/// Pointing at a link for a moment fetches the page it names into a small,
/// capped map that the click then reads, so the common navigation costs no
/// round trip. The map holds the promise rather than the text, which is what
/// makes a hover and the click after it one request; a reader who asked the
/// browser to save data is never preloaded for.
const NAVIGATION_SCRIPT: &str = r#"(function () {
  var KEY = "buri-theme";
  var PRELOAD_LIMIT = 48;
  var HOVER_DELAY = 70;

  function saved() { try { return localStorage.getItem(KEY); } catch (e) { return null; } }
  function apply() {
    var chosen = saved();
    if (chosen) { document.documentElement.setAttribute("data-theme", chosen); }
    var picker = document.querySelector("[data-theme-picker]");
    if (picker) { picker.value = document.documentElement.getAttribute("data-theme"); }
  }
  document.addEventListener("change", function (event) {
    var picker = event.target && event.target.closest && event.target.closest("[data-theme-picker]");
    if (!picker) { return; }
    document.documentElement.setAttribute("data-theme", picker.value);
    try { localStorage.setItem(KEY, picker.value); } catch (e) {}
  });

  // The link a plain same-origin navigation would follow, or null for one the
  // browser should keep: a download, another tab, off-site, an anchor here.
  function navigable(anchor) {
    if (!anchor || !anchor.getAttribute || !anchor.getAttribute("href")) { return null; }
    if (anchor.hasAttribute("download")) { return null; }
    if (anchor.target && anchor.target !== "" && anchor.target !== "_self") { return null; }
    var url;
    try { url = new URL(anchor.href, location.href); } catch (e) { return null; }
    if (url.origin !== location.origin) { return null; }
    if (url.pathname === location.pathname && url.search === location.search) { return null; }
    if (!/(\/|\.html)$/.test(url.pathname)) { return null; }
    return url;
  }

  // Pages asked for this session, keyed by path. The value is the promise, so
  // a hover and the click after it are one request rather than two.
  var pages = new Map();
  function request(url) {
    var key = url.pathname + url.search;
    var waiting = pages.get(key);
    if (waiting) { return waiting; }
    waiting = fetch(url.href, { credentials: "same-origin" }).then(function (response) {
      if (!response.ok) { throw new Error(String(response.status)); }
      return response.text();
    });
    // A failure leaves nothing behind, so the click retries rather than
    // inheriting the miss, and the rejection is never unhandled.
    waiting.catch(function () { pages.delete(key); });
    pages.set(key, waiting);
    while (pages.size > PRELOAD_LIMIT) { pages.delete(pages.keys().next().value); }
    return waiting;
  }

  function scroll(hash) {
    var target = hash ? document.getElementById(hash.slice(1)) : null;
    if (target) { target.scrollIntoView(); } else { window.scrollTo(0, 0); }
  }
  function go(url, push) {
    var address;
    try { address = new URL(url, location.href); } catch (e) { location.href = url; return; }
    request(address).then(function (html) {
      var parsed = new DOMParser().parseFromString(html, "text/html");
      if (!parsed || !parsed.body) { throw new Error("no body"); }
      document.title = parsed.title || document.title;
      document.body.innerHTML = parsed.body.innerHTML;
      if (push) { history.pushState({}, "", address.href); }
      apply();
      scroll(address.hash);
    }).catch(function () { location.href = address.href; });
  }

  // A reader on a metered connection asked for less traffic, not more.
  function savingData() {
    var connection = navigator.connection;
    return !!(connection && connection.saveData);
  }

  // The delay is the difference between meaning to read a link and sweeping
  // the pointer across the navigation on the way somewhere else.
  var hoverTimer = null;
  function preloadOnHover(event) {
    if (savingData()) { return; }
    var anchor = event.target && event.target.closest && event.target.closest("a");
    var url = navigable(anchor);
    if (!url) { return; }
    clearTimeout(hoverTimer);
    hoverTimer = setTimeout(function () { request(url); }, HOVER_DELAY);
  }
  document.addEventListener("mouseover", preloadOnHover);
  document.addEventListener("mouseout", function () { clearTimeout(hoverTimer); });
  document.addEventListener("touchstart", function (event) {
    if (savingData()) { return; }
    var anchor = event.target && event.target.closest && event.target.closest("a");
    var url = navigable(anchor);
    if (url) { request(url); }
  }, { passive: true });

  document.addEventListener("click", function (event) {
    if (event.defaultPrevented || event.button !== 0) { return; }
    if (event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) { return; }
    var anchor = event.target && event.target.closest && event.target.closest("a");
    var url = navigable(anchor);
    if (!url) { return; }
    event.preventDefault();
    go(url.href, true);
  });
  window.addEventListener("popstate", function () { go(location.href, false); });

  if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", apply);
  } else {
    apply();
  }
})()"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_summary_becomes_one_line_of_meta_description() {
        assert_eq!(one_line("A `Str`\n  is  text.\n"), "A Str is text.");
    }

    /// One page, with nothing around it, for the tests that only want the
    /// chrome a page carries.
    fn a_page() -> (Site, Page) {
        let page = Page {
            route: "getting-started/installing".to_string(),
            title: "Installing".to_string(),
            label: "getting-started/installing".to_string(),
            summary: "How to install.".to_string(),
            source: Source {
                path: "cli/src/docs/getting-started/installing.md".to_string(),
                directory: false,
            },
            section: None,
            group: None,
            listed: true,
            content: Content::Prose("Some prose.\n".to_string()),
            see_also: Vec::new(),
            facts: Vec::new(),
            adapted_from: None,
        };
        let site = Site {
            root: std::path::PathBuf::from("/nowhere"),
            pages: vec![Page {
                route: page.route.clone(),
                title: page.title.clone(),
                label: page.label.clone(),
                summary: page.summary.clone(),
                source: page.source.clone(),
                section: None,
                group: None,
                listed: true,
                content: Content::Prose(String::new()),
                see_also: Vec::new(),
                facts: Vec::new(),
                adapted_from: None,
            }],
        };
        (site, page)
    }

    /// The browser runs this, not the test suite, so what is pinned here is
    /// that the handler reaches the page at all.
    #[test]
    fn a_rendered_page_carries_the_hover_preload() {
        let (site, page) = a_page();
        let (html, _) = document(&site, &page);
        for marker in ["preloadOnHover", "\"mouseover\"", "\"touchstart\"", "saveData", "passive"] {
            assert!(html.contains(marker), "the rendered page is missing `{marker}`");
        }
        assert!(!html.contains("<script src="), "the page loads an external script");
    }

    /// The stylesheet is written into the document rather than linked, so a
    /// reader with a cold cache paints from the first response and the site
    /// has no assets directory to lose.
    #[test]
    fn a_rendered_page_carries_its_stylesheet_inline() {
        let (site, page) = a_page();
        let (html, _) = document(&site, &page);
        let (head, _) = html.split_once("</head>").expect("a head");
        let (_, rest) = head.split_once("<style>").expect("a style element in the head");
        let (css, _) = rest.split_once("</style>").expect("the style element ends");
        assert!(css.contains("pre .keyword"), "the style element is not the stylesheet");
        assert!(
            css.contains(&format!("[data-theme=\"{}\"]", themes::DEFAULT)),
            "the inlined sheet has no palette for the default scheme"
        );
        assert!(!html.contains("<link rel=\"stylesheet\""), "the page still links a stylesheet");
        assert!(!html.contains("site.css"), "the page still names a stylesheet file");
    }

    /// A hover and the click after it must be one request, the map must not
    /// grow without bound, and the swap must still do what it did before.
    #[test]
    fn the_preload_shares_one_request_and_bounds_its_map() {
        assert!(
            NAVIGATION_SCRIPT.contains("var pages = new Map();"),
            "the preloaded pages are no longer held in a map"
        );
        assert!(
            NAVIGATION_SCRIPT.contains("pages.set(key, waiting);"),
            "the map holds the text rather than the promise, so a hover and a click are two fetches"
        );
        assert!(
            NAVIGATION_SCRIPT.contains("pages.delete(pages.keys().next().value);"),
            "the map no longer evicts its oldest entry"
        );
        for kept in ["history.pushState", "document.title = parsed.title", "scroll(address.hash)"] {
            assert!(NAVIGATION_SCRIPT.contains(kept), "the swap no longer does `{kept}`");
        }
        assert!(
            NAVIGATION_SCRIPT.contains("window.addEventListener(\"popstate\""),
            "back and forward no longer run the swap"
        );
    }
}
