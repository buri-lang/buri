//! Every prose topic the toolchain ships.
//!
//! The documentation lives here, inside the crate, and is compiled into the
//! binary. That is what makes `buri docs` work in a directory that is not a
//! Buri repository, on a machine with no checkout — and it is what stops the
//! documentation and the toolchain from being separately versioned, which is
//! how a doc goes stale without anybody noticing.
//!
//! `cli/src/docs/SPEC.md` and the repository's `README.md` are *generated*
//! from the `lang/` and `guide/` topics by `buri docs assemble`, so the files a
//! reader meets on GitHub and the pages `buri docs` serves are the same bytes.
//! The specification is every `lang/` topic; the README is three `guide/` ones,
//! and the rest of the guide is read here or through `buri docs`.
//!
//! Adding a topic is one line in `TOPICS`, plus — if it belongs in an
//! assembled document — one line in `assemble::DOCUMENTS`.

use crate::documentation::markdown;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    /// The language reference: `cli/src/docs/SPEC.md`.
    Lang,
    /// The build system, the monorepo, and the CLI.
    Build,
    /// Prose that introduces rather than specifies. Three of these assemble
    /// into `README.md`; the rest are pages in their own right.
    Guide,
}

impl Kind {
    pub fn label(self) -> &'static str {
        match self {
            Kind::Lang => "language",
            Kind::Build => "build system",
            Kind::Guide => "guide",
        }
    }
}

pub struct Topic {
    /// What `buri docs <id>` takes, and the topic's stable identity. Renaming
    /// one breaks a link somebody wrote down, so treat it as public.
    pub id: &'static str,
    pub title: &'static str,
    pub kind: Kind,
    pub text: &'static str,
    /// Words a reader might search for that the prose does not itself contain.
    /// One literal per topic, and the cheapest lever there is on whether a
    /// natural-language query finds the right page.
    pub tags: &'static [&'static str],
    pub see_also: &'static [&'static str],
}

const fn t(id: &'static str, title: &'static str, kind: Kind, text: &'static str) -> Topic {
    Topic { id, title, kind, text, tags: &[], see_also: &[] }
}

const fn tagged(
    id: &'static str,
    title: &'static str,
    kind: Kind,
    text: &'static str,
    tags: &'static [&'static str],
    see_also: &'static [&'static str],
) -> Topic {
    Topic { id, title, kind, text, tags, see_also }
}

pub const TOPICS: &[Topic] = &[
    // -- The language reference, in specification order --------------------
    t("lang/introduction", "Introduction", Kind::Lang, include_str!("../docs/lang/introduction.md")),
    t("lang/notation", "Notation and conformance", Kind::Lang, include_str!("../docs/lang/notation.md")),
    tagged(
        "lang/lexical",
        "Source text and lexical structure",
        Kind::Lang,
        include_str!("../docs/lang/lexical.md"),
        &["comment", "identifier", "keyword", "literal", "string", "interpolation", "utf-8"],
        &["lang/grammar-rationale"],
    ),
    tagged(
        "lang/modules",
        "Modules",
        Kind::Lang,
        include_str!("../docs/lang/modules.md"),
        &["import", "export", "re-export", "module path", "visibility"],
        &["build/libraries"],
    ),
    tagged(
        "lang/types",
        "Types",
        Kind::Lang,
        include_str!("../docs/lang/types.md"),
        &["struct", "enum", "tuple", "array", "generic", "trait", "derive", "Int", "Float", "number"],
        &["lang/expressions"],
    ),
    tagged(
        "lang/expressions",
        "Expressions",
        Kind::Lang,
        include_str!("../docs/lang/expressions.md"),
        &["if", "match", "method", "call", "lambda", "operator", "precedence", "?", "??", "abort"],
        &["lang/patterns"],
    ),
    tagged(
        "lang/patterns",
        "Patterns",
        Kind::Lang,
        include_str!("../docs/lang/patterns.md"),
        &["match", "exhaustive", "destructure", "wildcard"],
        &["lang/expressions"],
    ),
    tagged(
        "lang/evaluation",
        "Evaluation",
        Kind::Lang,
        include_str!("../docs/lang/evaluation.md"),
        &["strict", "order", "immutability", "tail call", "recursion", "closure"],
        &[],
    ),
    t("lang/functions", "Functions", Kind::Lang, include_str!("../docs/lang/functions.md")),
    tagged(
        "lang/effects",
        "Effects and purity",
        Kind::Lang,
        include_str!("../docs/lang/effects.md"),
        &["ctx", "context", "capability", "pure", "io", "allocation", "Alloc", "side effect"],
        &["lang/programs", "build/testing"],
    ),
    tagged(
        "lang/programs",
        "Programs",
        Kind::Lang,
        include_str!("../docs/lang/programs.md"),
        &["main", "entry point", "platform", "host", "test"],
        &["lang/effects"],
    ),
    tagged(
        "lang/grammar-rationale",
        "Why the grammar is context-free and unambiguous",
        Kind::Lang,
        include_str!("../docs/lang/grammar-rationale.md"),
        &["type arguments", "parser", "ambiguity", "LR(1)"],
        &["lang/lexical"],
    ),
    t("lang/invariants", "Compilation invariants", Kind::Lang, include_str!("../docs/lang/invariants.md")),
    t(
        "lang/static-rules",
        "Static rules not expressed in the grammar",
        Kind::Lang,
        include_str!("../docs/lang/static-rules.md"),
    ),
    t(
        "lang/open-questions",
        "Non-goals and open questions",
        Kind::Lang,
        include_str!("../docs/lang/open-questions.md"),
    ),
    // -- The build system --------------------------------------------------
    tagged(
        "build/overview",
        "The Buri build system",
        Kind::Build,
        include_str!("../docs/build/overview.md"),
        &["monorepo", "target", "package", "label", "dependency"],
        &["build/build-files"],
    ),
    tagged(
        "build/build-files",
        "BUILD.buri",
        Kind::Build,
        include_str!("../docs/build/build-files.md"),
        &["sources", "dependencies", "binary", "library", "textproto", "visibility"],
        &["build/repo-config"],
    ),
    tagged(
        "build/libraries",
        "Libraries: `lib.buri` and the public surface",
        Kind::Build,
        include_str!("../docs/build/libraries.md"),
        &["lib.buri", "re-export", "internal", "api"],
        &["lang/modules"],
    ),
    tagged(
        "build/tags",
        "Tags, platforms, and policy",
        Kind::Build,
        include_str!("../docs/build/tags.md"),
        &["forbids", "policy", "platform", "js", "linux", "macos"],
        &[],
    ),
    tagged(
        "build/testing",
        "Testing",
        Kind::Build,
        include_str!("../docs/build/testing.md"),
        &["test", "assert", "golden", "hermetic", "double", "fake"],
        &["lang/effects"],
    ),
    tagged(
        "build/repo-config",
        "REPO.buri",
        Kind::Build,
        include_str!("../docs/build/repo-config.md"),
        &["tag", "root", "repository", "policy"],
        &["build/build-files"],
    ),
    tagged(
        "build/cli",
        "The buri CLI",
        Kind::Build,
        include_str!("../docs/build/cli.md"),
        &["command", "flag", "exit code", "build", "run", "query"],
        &[],
    ),
    tagged(
        "build/proto",
        "Importing a `.proto` schema",
        Kind::Build,
        include_str!("../docs/build/proto.md"),
        &["proto", "protobuf", "schema", "wire format", "varint", "oneof", "serialization", "json"],
        &["build/build-files"],
    ),
    tagged(
        "build/hermeticity",
        "Hermeticity, actions, and the cache",
        Kind::Build,
        include_str!("../docs/build/hermeticity.md"),
        &["cache", "reproducible", "incremental", "action", "sandbox"],
        &[],
    ),
    // -- The guide ---------------------------------------------------------
    // The first two and `guide/installing` are what `README.md` assembles to;
    // the others are pages, reached by `buri docs` or read in
    // `cli/src/docs/guide/`.
    t(
        "guide/readme-intro",
        "What Buri is",
        Kind::Guide,
        include_str!("../docs/guide/readme-intro.md"),
    ),
    tagged(
        "guide/readme-links",
        "Where the documentation is",
        Kind::Guide,
        include_str!("../docs/guide/readme-links.md"),
        &["readme", "index", "manual", "where", "find"],
        &["guide/goals"],
    ),
    t("guide/goals", "Goals", Kind::Guide, include_str!("../docs/guide/goals.md")),
    t("guide/three-ideas", "Three ideas", Kind::Guide, include_str!("../docs/guide/three-ideas.md")),
    t("guide/numbers", "Numbers: two names, one set of types", Kind::Guide, include_str!("../docs/guide/numbers.md")),
    t(
        "guide/methods-and-traits",
        "Methods, and traits as interfaces",
        Kind::Guide,
        include_str!("../docs/guide/methods-and-traits.md"),
    ),
    t(
        "guide/restricting-effects",
        "Restricting what propagates",
        Kind::Guide,
        include_str!("../docs/guide/restricting-effects.md"),
    ),
    t("guide/errors", "Errors are not ignorable", Kind::Guide, include_str!("../docs/guide/errors.md")),
    t("guide/imports", "Imports name the module first", Kind::Guide, include_str!("../docs/guide/imports.md")),
    t("guide/whats-in", "What's in v0.2", Kind::Guide, include_str!("../docs/guide/whats-in.md")),
    t("guide/installing", "Installing", Kind::Guide, include_str!("../docs/guide/installing.md")),
    t("guide/status", "Status and open questions", Kind::Guide, include_str!("../docs/guide/status.md")),
    t("guide/naming", "Naming", Kind::Guide, include_str!("../docs/guide/naming.md")),
    tagged(
        "guide/standard-library",
        "The standard library",
        Kind::Guide,
        include_str!("../docs/guide/standard-library.md"),
        &["core", "stdlib", "std", "list", "map", "json", "crypto", "alloc", "allocator", "simd"],
        &["lang/effects"],
    ),
];

/// The front matter each assembled document opens with: a title, and whatever
/// precedes its first section.
pub const LANG_FRONT: &str = include_str!("../docs/lang/_front.md");
pub const GUIDE_FRONT: &str = include_str!("../docs/guide/_front.md");

/// The normative grammar, and the source the tree-sitter grammar is generated
/// from (`documentation::grammar`). It is hand-written because it is the
/// declaration and `parsing/parser.rs` is the implementation, but it is not
/// inert: `every_grammar_keyword_is_a_keyword` holds it against `lexer::Kw`,
/// `language/corpus.rs::the_tree_sitter_grammar_is_generated_from_the_ebnf` regenerates
/// the editor grammar from it, and `editors/tree-sitter-buri/check.sh` holds
/// the result to what the parser accepts and rejects.
pub const GRAMMAR: &str = include_str!("../docs/grammar.ebnf");

/// The build-file schemas, likewise normative and hand-written.
pub const BUILD_PROTO: &str = include_str!("../docs/schema/build.proto");
pub const REPO_PROTO: &str = include_str!("../docs/schema/repo.proto");

pub fn find(id: &str) -> Option<&'static Topic> {
    TOPICS.iter().find(|t| t.id == id)
}

impl Topic {
    /// The first sentence, for an index listing or a search result.
    pub fn summary(&self) -> String {
        markdown::summary(self.text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn every_topic_is_reachable_and_unique() {
        let mut seen = HashSet::new();
        for t in TOPICS {
            assert!(seen.insert(t.id), "`{}` is registered twice", t.id);
            assert!(!t.text.trim().is_empty(), "`{}` is empty", t.id);
            assert!(
                t.id.contains('/'),
                "`{}` needs a `kind/name` id so the index can group it",
                t.id
            );
        }
    }

    #[test]
    fn every_see_also_names_a_topic() {
        for t in TOPICS {
            for other in t.see_also {
                assert!(find(other).is_some(), "`{}` points at `{other}`, which does not exist", t.id);
            }
        }
    }

    /// A topic's registered title must be the heading a reader actually sees,
    /// modulo the section number the specification carries. Otherwise the
    /// index and the page disagree about what the page is called.
    #[test]
    fn every_title_matches_its_first_heading() {
        for t in TOPICS {
            let headings = markdown::headings(t.text);
            let Some(h) = headings.first() else {
                panic!("`{}` has no heading", t.id);
            };
            let written = h.title.trim_start_matches(|c: char| c.is_ascii_digit() || c == '.').trim();
            let normalize = |s: &str| s.replace(['`', ':'], "").to_lowercase();
            assert!(
                normalize(written).starts_with(&normalize(t.title))
                    || normalize(t.title).starts_with(&normalize(written)),
                "`{}` is titled `{}` but its heading reads `{written}`",
                t.id,
                t.title
            );
        }
    }
}
