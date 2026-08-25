//! The error catalog.
//!
//! Every diagnostic the compiler emits carries a stable code, and every code
//! has a page here explaining the rule, what to do about it, and — the part
//! that matters — a program that provokes it.
//!
//! That reproduction is a `buri fail code=<code>` block, so the doctest suite
//! compiles it and checks that it still produces *that* code. A page cannot
//! describe an error the compiler has stopped emitting, and a code cannot be
//! renamed without the page failing. `every_emitted_code_has_a_page` closes
//! the loop from the other side.
//!
//! Codes are kebab-case rather than numbered. `E0308` is unsearchable; a
//! reader who sees `[result-discarded]` already knows most of the answer, and
//! can grep for it.
//!
//! A page also holds the *wording*, in the `---` block at its top: the message,
//! the label, the note and the fix, as templates the emission site binds values
//! into (`documentation::frontmatter`). A page that carries none is a page not
//! yet migrated, and keeps working the old way — its title comes from the list
//! below and its emission site builds the sentence itself.

use crate::documentation::frontmatter::{Catalog, Page};

pub struct ErrorDoc {
    pub code: &'static str,
    /// The docs-index title, for a page whose frontmatter does not give one.
    /// Read through [`ErrorDoc::title`], never directly.
    pub listed_title: &'static str,
    pub text: &'static str,
    /// Where the rule this page states is set out in full. A page explains one
    /// diagnostic; the chapter that owns the rule explains the rule, and a
    /// page that repeated it would be the second copy that goes stale.
    pub see_also: &'static [&'static str],
}

impl ErrorDoc {
    /// The page's own title where it has one, the registered title otherwise.
    pub fn title(&self) -> &str {
        match page(self.code) {
            Some(p) => &p.front.title,
            None => self.listed_title,
        }
    }
}

macro_rules! e {
    ($code:literal, $title:literal) => {
        e!($code, $title, &[])
    };
    ($code:literal, $title:literal, $see:expr) => {
        ErrorDoc {
            code: $code,
            listed_title: $title,
            text: include_str!(concat!("../docs/errors/", $code, ".md")),
            see_also: $see,
        }
    };
}

pub const ERRORS: &[ErrorDoc] = &[
    e!("chained-comparison", "Comparison operators do not chain"),
    e!("context-decl-not-allowed", "A `context` declaration lives where a context may be built"),
    e!("context-not-allowed", "A context may only be built where authority enters"),
    e!("ctx-not-first", "`ctx` comes first, or immediately after `self`"),
    e!("derive-only-trait", "Some traits are derived, never implemented", &["guide/standard-library"]),
    e!("derive-without-traits", "A `derive` clause names at least one trait"),
    e!("duplicate-bound", "A bound is named once"),
    e!("duplicate-declaration", "A name is declared once"),
    e!("duplicate-field", "A field name is used once"),
    e!("duplicate-pattern-binding", "A pattern binds each name once"),
    e!("duplicate-test-name", "A test name is used once per file"),
    e!("effect-and-trait", "No type implements both an effect and a trait"),
    e!("effect-carrying-bound", "A type that carries an effect satisfies no trait bound"),
    e!("effect-outside-platform", "Only a platform module declares an effect"),
    e!("effect-param-not-ctx", "An effect-carrying parameter is `self` or `ctx`"),
    e!("error-type-mismatch", "`?` does not convert the error type"),
    e!("expression-statement", "An expression statement is legal only in a test"),
    e!("host-not-granted", "A platform grants the effects its host exports", &["build/build-files"]),
    e!("if-without-else", "`if` is an expression, so it needs an `else`"),
    e!("impl-fn-without-self", "Everything in an `impl` takes `self`"),
    e!("impl-method-export", "An `impl` method is not separately exported"),
    e!("impl-outside-its-module", "An `impl` or a `derive` lives in its type's own module"),
    e!("incomplete-impl", "An `impl` supplies every method of its trait"),
    e!("lambda-captures-effect", "A lambda may not capture an effect"),
    e!("lambda-captures-generic", "A lambda may not capture a value that could be a context"),
    e!("literal-out-of-range", "A literal must fit the type it is pinned to"),
    e!("main-signature", "`main` has one shape"),
    e!("match-not-exhaustive", "A `match` covers every case"),
    e!("method-declared-free", "A method is declared inside an `impl`"),
    e!("method-not-a-value", "A method is not a value"),
    e!("missing-conformance", "Conformance is declared, never inferred"),
    e!("missing-field-pattern", "A struct pattern mentions every field"),
    e!("module-doc-not-first", "`//!` documents the module, so it comes first"),
    e!("module-not-found", "A module path names exactly one file"),
    e!("no-such-export", "A module exports what it says it exports"),
    e!("no-such-field", "A field is named by the type that declares it"),
    e!("no-such-method", "A method is looked up in its type's defining module"),
    e!("no-such-module", "A module path names a module that exists"),
    e!("not-a-trait", "A bound names a trait or an effect"),
    e!("not-an-effect", "A context binds effects"),
    e!("or-pattern-bindings", "Or-pattern alternatives bind the same names"),
    e!("private-to-module", "A private declaration is private to its module", &["build/libraries"]),
    e!("question-mark-mismatch", "`?` propagates into a matching return type"),
    e!("refutable-pattern", "A `let` pattern must match every value"),
    e!("relative-import", "Every module path is absolute"),
    e!("reserved-word", "Reserved words are not identifiers"),
    e!("rest-pattern-not-last", "A rest pattern comes last"),
    e!("result-discarded", "A `Result` may not be discarded", &["build/cli"]),
    e!("self-not-first", "`self` is the first parameter or nothing"),
    e!("self-type-outside-impl", "`Self` names the implementing type"),
    e!("struct-literal-head", "A struct literal is headed by a type"),
    e!("style-not-static", "A conditional style is known at compile time", &["guide/user-interfaces"]),
    e!("test-only-import", "A `testing` module is reachable only from a test", &["build/libraries"]),
    e!("test-outside-test-source", "A `test` lives in a test source"),
    e!("turbofish", "Type arguments are written without `::`"),
    e!("type-args-on-a-value", "Type arguments qualify a function, not a value"),
    e!("type-has-no-methods", "Only a declared type has methods"),
    e!("type-mismatch", "There are no implicit conversions"),
    e!("unannotated-variant", "A `.Variant` needs a known expected type"),
    e!("underivable", "A derive is a fold over the type's components"),
    e!("unexpected-token", "The grammar expected something else here"),
    e!("uninhabited", "A type with no finite value cannot be constructed"),
    e!("unnamed-namespace-import", "A namespace import must be named"),
    e!("unreachable-arm", "Every arm must be reachable"),
    e!("unresolved-name", "Every name resolves to a declaration"),
    e!("unresolved-type", "Every type name resolves to a declaration"),
    e!("unresolved-type-in-pattern", "A pattern's path names a type or a variant"),
    e!("unsatisfied-bound", "A bound is satisfied by a declaration"),
    e!("unterminated-comment", "A block comment is closed"),
];

pub fn find(code: &str) -> Option<&'static ErrorDoc> {
    ERRORS.iter().find(|e| e.code == code)
}

/// Every page's frontmatter, parsed on first use and kept.
///
/// Once per process rather than once per diagnostic: a build that reports four
/// hundred errors reads the catalog once, and a `&'static Page` is what a
/// [`crate::diagnostics::Diagnostic`] can hold without copying its templates.
pub fn catalog() -> &'static Catalog {
    static CATALOG: std::sync::OnceLock<Catalog> = std::sync::OnceLock::new();
    CATALOG.get_or_init(|| {
        let entries: Vec<(&'static str, &'static str)> =
            ERRORS.iter().map(|e| (e.code, e.text)).collect();
        Catalog::build(&entries)
    })
}

/// The parsed page for a code, or `None` for a code with no page and for a page
/// that has not been given frontmatter yet.
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
        for e in ERRORS {
            assert!(seen.insert(e.code), "`{}` is registered twice", e.code);
            assert!(!e.text.trim().is_empty(), "`{}` has an empty page", e.code);
            // A page says `reproduction: none` when no single file can provoke
            // the code; everything else carries the program that does.
            if page(e.code).is_none_or(|p| p.front.reproducible) {
                assert!(
                    e.text.contains(&format!("code={}", e.code)),
                    "`{}`'s page has no reproduction tagged with its own code, and does not say \
                     `reproduction: none`",
                    e.code
                );
            }
        }
    }

    /// A page whose frontmatter does not parse is a page whose diagnostic
    /// prints without its wording, which is a failure a user should never be
    /// the one to find.
    #[test]
    fn every_page_parses() {
        let failures = catalog().failures();
        assert!(failures.is_empty(), "these pages do not parse:\n  {}", failures.join("\n  "));
    }

    #[test]
    fn every_migrated_page_is_titled_and_worded() {
        for p in catalog().pages() {
            assert!(!p.front.title.trim().is_empty(), "`{}` has an empty title", p.code);
            assert!(!p.front.message.trim().is_empty(), "`{}` has an empty message", p.code);
        }
    }

    /// `{function}`, never `{fn}` or `{fnName}`: the project spells names out,
    /// and a template is read by whoever edits the page.
    #[test]
    fn every_placeholder_is_snake_case() {
        for p in catalog().pages() {
            let templates = [
                Some(&p.front.message),
                p.front.label.as_ref(),
                p.front.note.as_ref(),
                p.front.fix.as_ref(),
            ];
            for template in templates.into_iter().flatten() {
                for name in crate::documentation::frontmatter::placeholders(template) {
                    assert!(
                        !name.is_empty()
                            && name.chars().all(|c| c.is_ascii_lowercase() || c == '_')
                            && !name.starts_with('_')
                            && !name.ends_with('_'),
                        "`{}` has the placeholder `{{{name}}}`, which is not snake_case",
                        p.code
                    );
                }
            }
        }
    }

    /// A page that points somewhere is a page that had prose deleted in favour
    /// of the pointer, so a broken one loses the explanation rather than merely
    /// a link.
    #[test]
    fn every_see_also_names_a_topic() {
        for e in ERRORS {
            for other in e.see_also {
                assert!(
                    crate::documentation::topics::find(other).is_some(),
                    "`{}` points at `{other}`, which is not a topic",
                    e.code
                );
            }
        }
    }
}
