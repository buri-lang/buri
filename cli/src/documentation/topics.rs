//! Every prose topic the toolchain ships.
//!
//! The documentation lives here, inside the crate, and is compiled into the
//! binary. That is what makes `buri docs` work in a directory that is not a
//! Buri repository, on a machine with no checkout — and it is what stops the
//! documentation and the toolchain from being separately versioned, which is
//! how a doc goes stale without anybody noticing.
//!
//! `cli/src/docs/SPEC.md` is *generated* from the `language/` topics by
//! `buri docs assemble`, so the file a reader meets on GitHub and the pages
//! `buri docs` serves are the same bytes. The specification is every
//! `language/` topic; every other topic is a page in its own right, read here
//! or through `buri docs`.
//!
//! Adding a topic is one line in `TOPICS`, plus — if it belongs in an
//! assembled document — one line in `assemble::DOCUMENTS`.

use crate::documentation::markdown;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    /// A tutorial: what somebody meeting the language reads first.
    GettingStarted,
    /// A task, or a concept you have to know to perform one.
    Guide,
    /// The language reference: `cli/src/docs/SPEC.md`.
    Language,
    /// The build system and the monorepo — reference, not required reading.
    Build,
    /// Reference prose that is not about the build system. Lookup material:
    /// nothing here has to be read before writing a program.
    Reference,
}

impl Kind {
    pub fn label(self) -> &'static str {
        match self {
            Kind::GettingStarted => "getting started",
            Kind::Guide => "guide",
            Kind::Language => "language",
            Kind::Build => "build system",
            Kind::Reference => "reference",
        }
    }

    /// Where this kind's files live under `cli/src/docs/`.
    ///
    /// Deliberately not the id's prefix. A `build/*` topic keeps its short,
    /// long-published id and lives under `reference/build/`, so anything that
    /// wants the file — `include_str!`'s sibling in the tests, the website's
    /// edit links — has to ask here rather than split the id on `/`.
    pub fn directory(self) -> &'static str {
        match self {
            Kind::GettingStarted => "getting-started",
            Kind::Guide => "guides",
            Kind::Language => "language",
            Kind::Build => "reference/build",
            Kind::Reference => "reference",
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
    // -- Getting started ----------------------------------------------------
    tagged(
        "getting-started/why-buri",
        "Why Buri",
        Kind::GettingStarted,
        include_str!("../docs/getting-started/why-buri.md"),
        &["goals", "safe", "fast", "friendly", "immutable", "effects", "grammar", "name"],
        &["getting-started/installing", "getting-started/first-program", "language/introduction"],
    ),
    tagged(
        "getting-started/installing",
        "Installing",
        Kind::GettingStarted,
        include_str!("../docs/getting-started/installing.md"),
        &["install", "nix", "homebrew", "cargo", "init", "scaffold", "skills", "agent"],
        &["getting-started/first-program", "guides/editor-setup"],
    ),
    t(
        "getting-started/first-program",
        "Your first program",
        Kind::GettingStarted,
        include_str!("../docs/getting-started/first-program.md"),
    ),
    t(
        "getting-started/tutorial",
        "Tutorial: a small program, end to end",
        Kind::GettingStarted,
        include_str!("../docs/getting-started/tutorial.md"),
    ),
    // -- The language reference, in specification order --------------------
    t(
        "language/introduction",
        "Introduction",
        Kind::Language,
        include_str!("../docs/language/introduction.md"),
    ),
    t(
        "language/notation",
        "Notation and conformance",
        Kind::Language,
        include_str!("../docs/language/notation.md"),
    ),
    tagged(
        "language/lexical",
        "Source text and lexical structure",
        Kind::Language,
        include_str!("../docs/language/lexical.md"),
        &["comment", "identifier", "keyword", "literal", "string", "interpolation", "utf-8"],
        &["language/notation"],
    ),
    tagged(
        "language/modules",
        "Modules",
        Kind::Language,
        include_str!("../docs/language/modules.md"),
        &["import", "export", "re-export", "module path", "visibility"],
        &["build/libraries"],
    ),
    tagged(
        "language/types",
        "Types",
        Kind::Language,
        include_str!("../docs/language/types.md"),
        &["struct", "enum", "tuple", "array", "generic", "trait", "derive", "Int", "Float", "number"],
        &["language/expressions"],
    ),
    tagged(
        "language/expressions",
        "Expressions",
        Kind::Language,
        include_str!("../docs/language/expressions.md"),
        &["if", "match", "method", "call", "lambda", "operator", "precedence", "?", "??", "abort"],
        &["language/patterns"],
    ),
    tagged(
        "language/patterns",
        "Patterns",
        Kind::Language,
        include_str!("../docs/language/patterns.md"),
        &["match", "exhaustive", "destructure", "wildcard"],
        &["language/expressions"],
    ),
    tagged(
        "language/evaluation",
        "Evaluation",
        Kind::Language,
        include_str!("../docs/language/evaluation.md"),
        &["strict", "order", "immutability", "tail call", "recursion", "closure"],
        &[],
    ),
    t(
        "language/functions",
        "Functions",
        Kind::Language,
        include_str!("../docs/language/functions.md"),
    ),
    tagged(
        "language/effects",
        "Effects and purity",
        Kind::Language,
        include_str!("../docs/language/effects.md"),
        &["ctx", "context", "capability", "pure", "io", "allocation", "Alloc", "side effect"],
        &["language/programs", "build/testing"],
    ),
    tagged(
        "language/programs",
        "Programs",
        Kind::Language,
        include_str!("../docs/language/programs.md"),
        &["main", "entry point", "platform", "host", "test"],
        &["language/effects"],
    ),
    // -- Reference -----------------------------------------------------------
    // What every command shares — how a target is named, the two global flags,
    // the exit codes, and the shape of a diagnostic — which no single command's
    // page can carry. The file sits beside the directory of per-command prose
    // it heads: `reference/cli.md` is this topic and `reference/cli/build.md`
    // is `buri docs cli/build`.
    tagged(
        "reference/cli",
        "The CLI",
        Kind::Reference,
        include_str!("../docs/reference/cli.md"),
        &[
            "target pattern",
            "label",
            "global flag",
            "color",
            "error-format",
            "json",
            "exit code",
            "diagnostic",
            "agent",
        ],
        &["build/repo-config"],
    ),
    tagged(
        "reference/standard-library",
        "The standard library",
        Kind::Reference,
        include_str!("../docs/reference/standard-library.md"),
        &["core", "stdlib", "std", "list", "map", "json", "crypto", "alloc", "allocator", "simd"],
        &["language/effects", "guides/user-interfaces"],
    ),
    // -- The build system --------------------------------------------------
    tagged(
        "build/overview",
        "The build model",
        Kind::Build,
        include_str!("../docs/reference/build/overview.md"),
        &["monorepo", "target", "package", "label", "dependency"],
        &["guides/build-system", "build/build-files"],
    ),
    tagged(
        "build/build-files",
        "BUILD.buri",
        Kind::Build,
        include_str!("../docs/reference/build/build-files.md"),
        &["sources", "dependencies", "binary", "library", "textproto", "visibility"],
        &["build/repo-config"],
    ),
    tagged(
        "build/libraries",
        "Libraries: `lib.buri` and the public surface",
        Kind::Build,
        include_str!("../docs/reference/build/libraries.md"),
        &["lib.buri", "re-export", "internal", "api"],
        &["language/modules"],
    ),
    tagged(
        "build/tags",
        "Tags, platforms, and policy",
        Kind::Build,
        include_str!("../docs/reference/build/tags.md"),
        &["forbids", "policy", "platform", "js", "linux", "macos"],
        &["guides/tags-policy"],
    ),
    tagged(
        "build/testing",
        "Test targets and the testing host",
        Kind::Build,
        include_str!("../docs/reference/build/testing.md"),
        &["test", "assert", "golden", "hermetic", "double", "fake"],
        &["guides/testing", "language/effects"],
    ),
    tagged(
        "build/repo-config",
        "REPO.buri",
        Kind::Build,
        include_str!("../docs/reference/build/repo-config.md"),
        &["tag", "root", "repository", "policy"],
        &["build/build-files"],
    ),
    tagged(
        "build/proto",
        "Importing a `.proto` schema",
        Kind::Build,
        include_str!("../docs/reference/build/proto.md"),
        &["proto", "protobuf", "schema", "wire format", "varint", "oneof", "serialization", "json"],
        &["build/build-files", "guides/proto"],
    ),
    tagged(
        "build/hermeticity",
        "Hermeticity, actions, and the cache",
        Kind::Build,
        include_str!("../docs/reference/build/hermeticity.md"),
        &["cache", "reproducible", "incremental", "action", "sandbox"],
        &["guides/reproducibility"],
    ),
    // -- The guides ---------------------------------------------------------
    // Pages, reached by `buri docs` or read in `cli/src/docs/guides/`. None of
    // them is assembled into a document; the root `README.md` is hand-written
    // and keeps its own copy of what it needs.
    tagged(
        "guides/build-system",
        "Using the build system",
        Kind::Guide,
        include_str!("../docs/guides/build-system.md"),
        &["BUILD.buri", "package", "label", "visibility", "dependency", "gen", "monorepo"],
        &["guides/testing", "build/overview"],
    ),
    tagged(
        "guides/testing",
        "Testing your code",
        Kind::Guide,
        include_str!("../docs/guides/testing.md"),
        &["test", "assert", "double", "fake", "fixture", "golden", "filter", "watch"],
        &["build/testing", "guides/effects"],
    ),
    tagged(
        "guides/editor-setup",
        "Set up your editor",
        Kind::Guide,
        include_str!("../docs/guides/editor-setup.md"),
        &["lsp", "language server", "editor", "ide", "zed", "highlighting", "tree-sitter"],
        &[],
    ),
    tagged(
        "guides/effects",
        "Effects and capabilities",
        Kind::Guide,
        include_str!("../docs/guides/effects.md"),
        &["ctx", "context", "capability", "host", "attenuation", "pure", "purity", "mock", "alloc"],
        &["language/effects", "build/testing"],
    ),
    t(
        "guides/numbers",
        "Numbers: two names, one set of types",
        Kind::Guide,
        include_str!("../docs/guides/numbers.md"),
    ),
    t(
        "guides/methods-and-traits",
        "Methods, and traits as interfaces",
        Kind::Guide,
        include_str!("../docs/guides/methods-and-traits.md"),
    ),
    tagged(
        "guides/compile-speed",
        "How Buri compiles fast",
        Kind::Guide,
        include_str!("../docs/guides/compile-speed.md"),
        &[
            "compile speed",
            "fast",
            "incremental",
            "parallel",
            "inference",
            "signature",
            "monomorphization",
            "invariant",
        ],
        &["language/functions", "build/hermeticity"],
    ),
    tagged(
        "guides/user-interfaces",
        "User interfaces",
        Kind::Guide,
        include_str!("../docs/guides/user-interfaces.md"),
        &["ui", "signal", "reactive", "node", "style", "theme", "dom", "browser", "web"],
        &["reference/standard-library"],
    ),
    tagged(
        "guides/concurrency",
        "Tasks and actors",
        Kind::Guide,
        include_str!("../docs/guides/concurrency.md"),
        &[
            "concurrency",
            "parallel",
            "task",
            "actor",
            "mailbox",
            "message",
            "state",
            "ask",
            "send",
        ],
        &["guides/web-server", "language/effects"],
    ),
    tagged(
        "guides/compile-to-js",
        "Compile to JavaScript",
        Kind::Guide,
        include_str!("../docs/guides/compile-to-js.md"),
        &["javascript", "js", "node", "bun", "browser", "web", "esm", "mjs"],
        &["build/build-files", "build/tags"],
    ),
    tagged(
        "guides/web-server",
        "Build a web server",
        Kind::Guide,
        include_str!("../docs/guides/web-server.md"),
        &[
            "server",
            "http",
            "listen",
            "socket",
            "websocket",
            "route",
            "handler",
            "port",
            "tls",
        ],
        &["guides/concurrency", "reference/standard-library"],
    ),
    tagged(
        "guides/proto",
        "Import a .proto schema",
        Kind::Guide,
        include_str!("../docs/guides/proto.md"),
        &["proto", "protobuf", "schema", "codegen", "serialization"],
        &["build/proto"],
    ),
    tagged(
        "guides/tags-policy",
        "Enforce policy with tags",
        Kind::Guide,
        include_str!("../docs/guides/tags-policy.md"),
        &["policy", "forbids", "boundary", "layering", "deployment"],
        &["build/tags"],
    ),
    tagged(
        "guides/reproducibility",
        "Reproducible builds",
        Kind::Guide,
        include_str!("../docs/guides/reproducibility.md"),
        &["cache", "incremental", "explain", "deterministic", "hermetic"],
        &["build/hermeticity"],
    ),
];

/// The front matter an assembled document opens with, under the generated-file
/// notice `assemble` writes: a title, and whatever precedes its first section.
pub const LANG_FRONT: &str = include_str!("../docs/language/_front.md");

/// The normative grammar, and the source the tree-sitter grammar is generated
/// from (`documentation::grammar`). It is hand-written because it is the
/// declaration and `parsing/parser.rs` is the implementation, but it is not
/// inert: `every_grammar_keyword_is_a_keyword` holds it against
/// `lexer::Keyword`,
/// `language/corpus.rs::the_tree_sitter_grammar_is_generated_from_the_ebnf` regenerates
/// the editor grammar from it, and `editors/tree-sitter-buri/check.sh` holds
/// the result to what the parser accepts and rejects.
pub const GRAMMAR: &str = include_str!("../docs/grammar.ebnf");

/// The build-file schemas, likewise normative and hand-written.
pub const BUILD_PROTO: &str = include_str!("../docs/reference/schema/build.proto");
pub const REPO_PROTO: &str = include_str!("../docs/reference/schema/repo.proto");

pub fn find(id: &str) -> Option<&'static Topic> {
    TOPICS.iter().find(|t| t.id == id)
}

impl Topic {
    /// The first sentence, for an index listing or a search result.
    pub fn summary(&self) -> String {
        markdown::summary(self.text)
    }

    /// The file this topic's text is `include_str!`d from, relative to the
    /// repository root. The one place a topic's path is derived, so a moved
    /// directory is one edit in `Kind::directory` rather than a search for
    /// every caller that concatenated an id.
    pub fn path(&self) -> String {
        let name = self.id.rsplit('/').next().unwrap_or(self.id);
        format!("cli/src/docs/{}/{name}.md", self.kind.directory())
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
