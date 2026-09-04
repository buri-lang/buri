//! The lint catalog.
//!
//! `buri lint` reports what type checking does not: a rule about style, about
//! shape, or about a thing that is legal and still a mistake. Each of those
//! findings has a page here, for the same reason every compile error has one —
//! a finding a reader cannot look up is one they can only silence.
//!
//! The shape is `errors.rs`'s, deliberately: one markdown file per code under
//! `src/docs/reference/lints/`, its wording in the frontmatter at the top, its body the
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

macro_rules! l {
    ($code:literal, $title:literal) => {
        l!($code, $title, &[])
    };
    ($code:literal, $title:literal, $see:expr) => {
        LintDoc {
            code: $code,
            listed_title: $title,
            text: include_str!(concat!("../docs/reference/lints/", $code, ".md")),
            see_also: $see,
        }
    };
}

/// Every `buri lint` finding, in the order the index lists them.
pub const LINTS: &[LintDoc] = &[
    l!("ctx-rebinding", "`ctx` names the context a function was handed"),
    l!("dead-code", "Every declaration is reached from a `lib.buri` or a `main.buri`", &[
        "build/libraries"
    ]),
    l!("deep-nesting", "Branches nest shallowly"),
    l!("dep-cycle", "Two targets may not depend on each other", &["build/build-files"]),
    l!("discarded-result", "Every deliberately dropped `Result` is reported", &[
        "language/expressions"
    ]),
    l!("duplicate-import", "A module is imported once"),
    l!("duplicate-source", "A source file is listed by one rule", &["build/build-files"]),
    l!("empty-test-suite", "A `test` block declares the sources it tests"),
    l!("hex-digit-table", "The hexadecimal digits are not yours to keep"),
    l!("missing-dep", "Every library a package uses is in its dependencies", &["build/build-files"]),
    l!("oversized-function", "A function is one responsibility"),
    l!("test-title-newline", "A test title is one line"),
    l!("test-without-assertion", "A test asserts something"),
    l!("time-unit-conversion", "A length of time is a `Duration`"),
    l!("too-many-parameters", "A function takes few parameters"),
    l!("unsatisfiable-target", "A target admits at least one platform", &["build/tags"]),
    l!("unused-context", "A function that takes `ctx` uses it"),
    l!("unused-context-bound", "A context asks for the effects it uses"),
    l!("unused-field", "Every field is read"),
    l!("unused-import", "Every import is used"),
    l!("unused-library", "Every source file belongs to a library or a binary", &[
        "build/build-files"
    ]),
    l!("unused-type", "Every declared type is used", &["build/libraries"]),
    l!("unused-variable", "Every `let` names something the code below it reads"),
    l!("unused-variant", "Every variant is constructed or matched"),
    l!("warning-comment", "A marker comment is work that was left behind"),
];

/// The findings that are about something the program does not need, rather
/// than about something it does wrongly.
///
/// The list is here rather than at either front end because it is a fact about
/// the code and not about a surface: `buri lint` renders one as the warning it
/// already was, and the language server turns it into the LSP's
/// `DiagnosticTag.Unnecessary`, which is what makes an editor grey the span out
/// rather than underline it. One list, so the two cannot drift.
const UNNECESSARY: &[&str] = &[
    "dead-code",
    "unused-context",
    "unused-context-bound",
    "unused-field",
    "unused-import",
    "unused-type",
    "unused-variable",
    "unused-variant",
];

/// Whether a finding says "this is not needed", as opposed to "this is wrong".
pub fn is_unnecessary(code: &str) -> bool {
    UNNECESSARY.contains(&code)
}

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

    /// A page that points somewhere is a page that had prose deleted in favour
    /// of the pointer, so a broken one loses the explanation rather than merely
    /// a link.
    #[test]
    fn every_see_also_names_a_topic() {
        for l in LINTS {
            for other in l.see_also {
                assert!(
                    crate::documentation::topics::find(other).is_some(),
                    "`{}` points at `{other}`, which is not a topic",
                    l.code
                );
            }
        }
    }

    #[test]
    fn every_migrated_page_is_titled_and_worded() {
        for p in catalog().pages() {
            assert!(!p.front.title.trim().is_empty(), "`{}` has an empty title", p.code);
            assert!(!p.front.message.trim().is_empty(), "`{}` has an empty message", p.code);
        }
    }

    #[test]
    fn every_page_parses() {
        let failures = catalog().failures();
        assert!(failures.is_empty(), "these pages do not parse:\n  {}", failures.join("\n  "));
    }

    /// The same guard `errors.rs` carries: a page that lost its `---` block
    /// leaves its finding with no message, and nothing else notices.
    #[test]
    fn every_page_carries_its_wording() {
        let missing: Vec<&str> =
            LINTS.iter().map(|l| l.code).filter(|code| page(code).is_none()).collect();
        assert!(
            missing.is_empty(),
            "these pages carry no `---` frontmatter block, so their findings have no message \
             to print:\n  {}",
            missing.join("\n  ")
        );
    }
}
