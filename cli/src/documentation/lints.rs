//! The lint catalog.
//!
//! `buri lint` reports what type checking does not: a rule about style, about
//! shape, or about a thing that is legal and still a mistake. Each of those
//! findings has a page here, for the same reason every compile error has one —
//! a finding a reader cannot look up is one they can only silence.
//!
//! The shape is `errors.rs`'s, deliberately: one markdown file per code under
//! `src/docs/lints/`, its wording in the frontmatter at the top, its body the
//! explanation. What differs is the severity — a lint finding is usually a
//! warning, and says so in its page rather than at its emission site.

use crate::documentation::frontmatter::{Catalog, Page};

pub struct LintDoc {
    pub code: &'static str,
    /// The docs-index title, for a page whose frontmatter does not give one.
    /// Read through [`LintDoc::title`], never directly.
    pub listed_title: &'static str,
    pub text: &'static str,
    /// Where the rule this page states is set out in full.
    pub see_also: &'static [&'static str],
}

impl LintDoc {
    pub fn title(&self) -> &str {
        match page(self.code) {
            Some(p) => &p.front.title,
            None => self.listed_title,
        }
    }
}

#[allow(
    unused_macros,
    reason = "the catalog is registered a page at a time, and the first page is not written yet"
)]
macro_rules! l {
    ($code:literal, $title:literal) => {
        l!($code, $title, &[])
    };
    ($code:literal, $title:literal, $see:expr) => {
        LintDoc {
            code: $code,
            listed_title: $title,
            text: include_str!(concat!("../docs/lints/", $code, ".md")),
            see_also: $see,
        }
    };
}

/// Every `buri lint` finding, in the order the index lists them.
pub const LINTS: &[LintDoc] = &[];

pub fn find(code: &str) -> Option<&'static LintDoc> {
    LINTS.iter().find(|l| l.code == code)
}

/// Every page's frontmatter, parsed on first use and kept. See
/// [`super::errors::catalog`].
pub fn catalog() -> &'static Catalog {
    static CATALOG: std::sync::OnceLock<Catalog> = std::sync::OnceLock::new();
    CATALOG.get_or_init(|| {
        let entries: Vec<(&'static str, &'static str)> =
            LINTS.iter().map(|l| (l.code, l.text)).collect();
        Catalog::build(&entries)
    })
}

pub fn page(code: &str) -> Option<&'static Page> {
    catalog().page(code)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn every_code_is_unique_and_documented() {
        let mut seen = HashSet::new();
        for l in LINTS {
            assert!(seen.insert(l.code), "`{}` is registered twice", l.code);
            assert!(!l.text.trim().is_empty(), "`{}` has an empty page", l.code);
        }
    }

    #[test]
    fn every_page_parses() {
        let failures = catalog().failures();
        assert!(failures.is_empty(), "these pages do not parse:\n  {}", failures.join("\n  "));
    }
}
