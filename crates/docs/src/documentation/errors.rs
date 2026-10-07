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
            text: include_str!(concat!("../docs/reference/errors/", $code, ".md")),
            see_also: $see,
        }
    };
}

pub const ERRORS: &[ErrorDoc] = &[
    e!("accepts-duplicate-language", "A contract lists each language once", &["build/tools"]),
    e!("accepts-missing-field", "An `accepts` entry names a language and a schema", &["build/tools"]),
    e!("accepts-missing-generate", "A contract language needs a `generate`", &["build/tools"]),
    e!("accepts-unknown-language", "A contract names a declared language", &["build/tools"]),
    e!("alpha-out-of-range", "A colour's alpha is a fraction from 0 to 1", &["guides/user-interfaces"]),
    e!("ambiguous-free-function", "A method called as a free function names one type"),
    e!("ambiguous-source", "A source two rules reach is listed explicitly", &["build/build-files"]),
    e!("ambiguous-trait-method", "A method two bounds share needs qualifying"),
    e!("argument-count", "A call passes the arguments its callee declares"),
    e!("array-impl-outside-core-list", "The defining module of `[T]` is `core/list`"),
    e!("binary-entry-import", "A binary's entry point is imported only by its own tests", &["build/libraries"]),
    e!("binary-field-not-allowed", "Some library fields don't apply to a binary", &["build/build-files"]),
    e!("binary-internal-import", "A binary imports its library through the surface", &["build/build-files"]),
    e!("binary-source-import", "A library never imports its binary", &["build/build-files"]),
    e!("bitwise-non-integer", "The bitwise operators are defined on integers"),
    e!("bound-not-trait", "A bound names a trait or an effect"),
    e!("build-field-mismatch", "A build-file field holds one kind of value", &["build/build-files"]),
    e!("build-file-syntax", "A build file is a well-formed textproto", &["build/build-files"]),
    e!("build-quoted-word", "An enumerated build-file value is a bare word", &["build/build-files"]),
    e!("build-unknown-field", "A build file uses only declared fields", &["build/build-files"]),
    e!("build-unknown-word", "A build-file field takes one of a closed set of words", &["build/build-files"]),
    e!("built-in-language-tool", "A built-in language keeps its own tools", &["build/repo-config"]),
    e!("chain-too-long", "A chain has a bounded length"),
    e!("chained-comparison", "Comparison operators do not chain"),
    e!("character-literal-length", "A character literal holds one scalar value"),
    e!("circular-generator", "A generator's tool never builds from its output", &["build/generators"]),
    e!("circular-import", "Modules form a graph with no cycles"),
    e!("circular-type-alias", "A type alias never expands to itself", &["language/types"]),
    e!("const-declaration", "A module-level binding is written with `let`", &["language/lexical"]),
    e!("context-binding-not-effect", "A context binding names an effect"),
    e!("context-export", "A `context` is exported only from a test-only module"),
    e!("context-mismatch", "Two contexts are one type when their bindings match"),
    e!("context-not-called", "A context is built by calling it"),
    e!("context-parameters", "A context declaration takes no parameters"),
    e!("context-spread-operand", "A context spread takes another context"),
    e!("cross-app-dependency", "An app reaches only its own packages and shared ones", &["build/build-files"]),
    e!("cryptography-unavailable", "Secure randomness needs a cryptography-enabled toolchain"),
    e!("ctx-not-first", "`ctx` comes first, or immediately after `self`"),
    e!("custom-effect-outside-js", "Only a `JS` entry offers an effect its platform implements", &["build/platforms"]),
    e!("derive-missing-traits", "A `derive` clause names at least one trait"),
    e!("derive-not-trait", "A `derive` names a declared trait"),
    e!("derive-only-trait", "Some traits are derived, never implemented", &["reference/standard-library"]),
    e!("derive-operator-not-newtype", "A derived operator needs a single-field struct"),
    e!("derive-operator-not-numeric", "A derived operator needs a wrapped number"),
    e!("derive-target-not-type", "A `derive` names a declared type"),
    e!("double-colon", "A module's members are reached with `.`"),
    e!("duplicate-artifact-path", "Each artifact has its own path", &["build/build-files"]),
    e!("duplicate-context-binding", "A context binds each effect once"),
    e!("duplicate-ctx", "A function takes one context"),
    e!("duplicate-declaration", "A name is declared once"),
    e!("duplicate-entry", "An output fills each entry once", &["build/build-files"]),
    e!("duplicate-extension", "An extension belongs to one language", &["build/repo-config"]),
    e!("duplicate-field", "A field name is used once"),
    e!("duplicate-field-value", "A struct literal gives each field once"),
    e!("duplicate-impl", "A type implements a trait once"),
    e!("duplicate-language", "A language is declared once", &["build/repo-config"]),
    e!("duplicate-method", "A type has one method of each name"),
    e!("duplicate-pattern-binding", "A pattern binds each name once"),
    e!("duplicate-platform-entry", "A platform names each entry once", &["build/platforms"]),
    e!("duplicate-rest-pattern", "An array pattern has one rest pattern"),
    e!("duplicate-tag", "A tag is declared once", &["build/tags"]),
    e!("duplicate-test", "A test name is used once per file"),
    e!("effect-and-trait", "No type implements both an effect and a trait"),
    e!("effect-carrying-bound", "A type that carries an effect satisfies no trait bound"),
    e!("effect-method-call", "An effect is performed through a function, not a method", &["language/effects"]),
    e!("effect-missing-test-implementation", "An effect has a test implementation beside it", &["build/platforms"]),
    e!("effect-not-imported", "A context binding names an effect this file imports"),
    e!("effect-not-on-backend", "A host offers only what its backend implements", &["build/platforms"]),
    e!("effect-outside-effect-directory", "An effect lives in an effect package", &["build/platforms"]),
    e!("effect-parameter-not-ctx", "An effect-carrying parameter is `self` or `ctx`"),
    e!("entry-declaration-imported", "A platform's entry is filled, not called", &["language/effects"]),
    e!("entry-host-mismatch", "An entry takes the host of the platform it is built for", &["build/build-files"]),
    e!("entry-missing-field", "An output's entry names the entry and its function", &["build/build-files"]),
    e!("entry-missing-host", "An entry takes its platform's host", &["language/effects"]),
    e!("entry-not-runnable", "`buri run` runs an entry that starts itself", &["build/platforms"]),
    e!("entry-point-listed", "An entry point is named by its rule, never listed", &["build/build-files"]),
    e!("entry-signature-mismatch", "An entry has the signature its platform declares", &["build/build-files"]),
    e!("enum-not-value", "An enum value names a variant"),
    e!("example-missing-main", "A runnable example exports `main`"),
    e!("expression-too-deep", "An expression nests to a bounded depth"),
    e!("field-not-callable", "A field holding a value is not a method"),
    e!("float-tuple-index", "Two tuple indices in a row lex as a float"),
    e!("generator-duplicate-module", "A generated module's name is unique", &["build/generators"]),
    e!("generator-missing-tool", "A generator names its tool", &["build/generators"]),
    e!("generator-not-tool", "A generator is a tool rule", &["build/generators", "build/tools"]),
    e!("generic-effect-unsupported", "A trait or an effect takes no type parameters of its own"),
    e!("host-field-not-production", "A host's fields are production structs", &["build/platforms"]),
    e!("host-file-missing-method", "A `js` file implements every method its structs declare", &["build/platforms"]),
    e!("host-import-outside-platform", "Only a platform names the backends' production values", &["language/effects"]),
    e!("icon-not-drawable", "An icon's artwork is written out, and holds only shapes", &["guides/user-interfaces"]),
    e!("impl-item-not-method", "An `impl` body holds methods"),
    e!("impl-method-export", "An `impl` method is not separately exported"),
    e!("impl-missing-method", "An `impl` supplies every method of its trait"),
    e!("impl-missing-self", "Everything in an `impl` takes `self`"),
    e!("impl-outside-type-module", "An `impl` or `derive` lives in its type's module"),
    e!("impl-signature-mismatch", "An `impl` supplies the signature its trait declares"),
    e!("impl-target-not-declared-type", "Only a declared type has methods"),
    e!("impl-target-not-type", "An `impl` names a declared type"),
    e!("impl-unknown-method", "An `impl` supplies only its trait's methods"),
    e!("import-missing-extension", "A non-surface module is imported by file name", &["language/modules"]),
    e!("input-not-accepted", "A tool with a contract reads the languages it lists", &["build/tools"]),
    e!("integer-missing-digits", "A base prefix is followed by digits"),
    e!("integer-too-wide", "An integer literal fits in 128 bits"),
    e!("internal-import", "A library is imported through its surface", &["build/libraries"]),
    e!("invalid-entry-function", "An output's `entries` name functions", &["build/build-files"]),
    e!("invalid-extension", "An extension starts with a dot", &["build/repo-config"]),
    e!("invalid-float", "A float literal has a digit on each side of the point"),
    e!("invalid-integer", "An integer's digits match its base prefix"),
    e!("invalid-platform-variant", "A native variant names an operating system and an architecture", &["build/platforms"]),
    e!("invalid-tuple-index", "A tuple index is a plain decimal number"),
    e!("invalid-unicode-escape", "A Unicode escape names a scalar value"),
    e!("js-outside-js-entry", "Only a `JS` entry has a `js` file", &["build/platforms"]),
    e!("json-invalid-schema", "A schema is one this toolchain can read", &["guides/json"]),
    e!("json-missing-schema", "A JSON file names its schema", &["guides/json"]),
    e!("json-schema-draft-unsupported", "A schema is JSON Schema 2020-12", &["guides/json"]),
    e!("json-schema-violation", "A JSON file follows its schema", &["guides/json"]),
    e!("json-syntax", "A JSON file parses", &["guides/json"]),
    e!("json-untyped-keyword", "A schema that generates types maps to Buri types", &["guides/json"]),
    e!("lambda-captures-effect", "A lambda may not capture an effect"),
    e!("lambda-captures-generic", "A lambda may not capture a value that could be a context"),
    e!("language-missing-name", "A `language` block has a `name`", &["build/repo-config"]),
    e!("leading-export", "A re-export is written without a leading `export`"),
    e!("literal-out-of-range", "A literal fits its type"),
    e!("load-in-reactive-builder", "A reactive builder is synchronous, so it may not `load`", &["guides/websites", "guides/compile-to-js"]),
    e!("load-not-function", "`load` takes a function name"),
    e!("match-not-exhaustive", "A `match` covers every case"),
    e!("method-not-value", "A method is not a value"),
    e!("method-outside-impl", "A method is declared inside an `impl`"),
    e!("misplaced-artifact-name", "A page keeps its entry's file name", &["build/build-files"]),
    e!("misplaced-context", "A context is built only in an entry or a test"),
    e!("misplaced-context-declaration", "A `context` is declared only in an entry or test code"),
    e!("misplaced-rule", "A tool or platform rule lives in its own directory", &["build/tools", "build/build-files"]),
    e!("missing-arrow", "A match arm is `pattern => expression`"),
    e!("missing-body", "A declaration outside a trait or effect has a body"),
    e!("missing-comma", "A list separates its elements with `,`"),
    e!("missing-else", "`if` is an expression, so it needs an `else`"),
    e!("missing-field-pattern", "A struct pattern mentions every field"),
    e!("missing-field-value", "A literal gives every required field a value"),
    e!("missing-host-file", "An entry with bodiless methods has a `js` file", &["build/platforms"]),
    e!("missing-impl", "A type implements the traits its uses need"),
    e!("missing-main", "A binary exports the entry its output fills", &["build/build-files"]),
    e!("missing-payload-pattern", "A variant with a payload is matched with one"),
    e!("missing-platform-variant", "An output names a variant its platform requires", &["build/build-files"]),
    e!("missing-semicolon", "A declaration ends with `;`"),
    e!("module-doc-not-first", "`//!` documents the module, so it comes first"),
    e!("module-outside-repository", "A `//` path needs a repository"),
    e!("native-artifact-unavailable", "A native artifact needs a toolchain that builds one", &["build/tags"]),
    e!("networking-unavailable", "The network needs a networking-enabled toolchain"),
    e!("not-callable", "A call names a function or a lambda"),
    e!("not-indexable", "Indexing is defined on arrays"),
    e!("not-interpolatable", "A string hole holds a renderable value"),
    e!("not-on-surface", "A method from another library is on its surface"),
    e!("not-tuple", "A numeric field access indexes a tuple"),
    e!("or-pattern-bindings", "Or-pattern alternatives bind the same names"),
    e!("output-missing-platform", "An output is the artifact for one platform", &["build/build-files"]),
    e!("output-unavailable", "A build that selects no output skips the ones this host cannot build", &["build/tags"]),
    e!("package-missing-rule", "A build file declares a rule", &["build/build-files"]),
    e!("pattern-not-array", "An array pattern matches an array"),
    e!("pattern-not-tuple", "A tuple pattern matches a tuple of its length"),
    e!("pattern-type-mismatch", "A pattern matches the matched value's shape"),
    e!("payload-count", "A constructor is given the values it holds"),
    e!("payload-pattern-count", "A payload pattern matches the values the variant holds"),
    e!("platform-entry-missing-field", "A platform's entry has a name and a backend", &["build/platforms"]),
    e!("platform-missing-entry", "A platform has an entry", &["build/platforms"]),
    e!("platform-testing-only-import", "Only an effect's testing surface keeps state", &["build/libraries"]),
    e!("platform-violation", "A target is built only for a platform its closure admits", &["build/tags"]),
    e!("postfix-on-block", "A block expression is parenthesised before `.`, `(` or `[`"),
    e!("private-to-module", "A private declaration is private to its module", &["build/libraries"]),
    e!("proto-ambiguous-type", "A type name says which schema it means", &["build/proto"]),
    e!("proto-contract-unsupported", "A `.proto` file is taken as text", &["build/proto", "build/tools"]),
    e!("proto-duplicate-field", "A message field's number and name are unique", &["build/proto"]),
    e!("proto-duplicate-type", "A fully-qualified name names one type", &["build/proto"]),
    e!("proto-missing-edition", "A schema declares its edition", &["build/proto"]),
    e!("proto-syntax", "A `.proto` file is a well-formed schema", &["build/proto"]),
    e!("proto-unknown-feature", "Every `features` value is supported", &["build/proto"]),
    e!("proto-unknown-import", "A schema's import is written from the repository root", &["build/proto"]),
    e!("proto-unknown-type", "A field's type names a message or an enum", &["build/proto"]),
    e!("proto-unsupported", "The schema reader refuses what it cannot express", &["build/proto"]),
    e!("proto-unsupported-edition", "A schema declares the edition this reader supports", &["build/proto"]),
    e!("reassignment", "A binding is given its value once"),
    e!("refutable-pattern", "A `let` pattern must match every value"),
    e!("relative-import", "Every module path is absolute"),
    e!("reserved-word", "Reserved words are not identifiers"),
    e!("rest-pattern-not-last", "A rest pattern comes last"),
    e!("result-discarded", "A `Result` may not be discarded", &["language/expressions"]),
    e!("schema-mismatch", "A file under a contract has the contract's schema", &["build/tools"]),
    e!("schema-outside-repository", "A schema is checked into the repository", &["guides/json"]),
    e!("self-not-first", "`self` is the first parameter or nothing"),
    e!("self-outside-method", "`self` is legal only in a method body"),
    e!("self-type-outside-impl", "`Self` names the implementing type"),
    e!("shared-depends-on-app", "Shared code reaches no app", &["build/build-files"]),
    e!("statement-not-unit", "A statement's value is used or bound"),
    e!("statement-outside-test", "An expression statement is legal only in a test"),
    e!("struct-literal-head", "A struct literal starts with a type or variant"),
    e!("style-not-static", "A conditional style is known at compile time", &["guides/user-interfaces"]),
    e!("tag-conflict", "A target never mixes tags that forbid each other", &["build/tags"]),
    e!("tag-duplicate-platform", "A tag lists each platform once", &["build/tags"]),
    e!("tag-missing-name", "A `tag` block has a `name`", &["build/tags"]),
    e!("tag-name-not-string", "A tag is named by a quoted string", &["build/tags"]),
    e!("tag-not-block", "A `tag` is a block in REPO.buri", &["build/tags"]),
    e!("tag-platform-conflict", "A tag never requires and forbids the same platform", &["build/tags"]),
    e!("tags-under-requires", "`requires` holds no `tags`", &["build/tags"]),
    e!("test-internal-import", "A test imports its library through the surface", &["build/libraries"]),
    e!("test-only-import", "A `testing` module is reachable only from a test", &["build/libraries"]),
    e!("test-outside-test-source", "A `test` lives in a test source"),
    e!("test-run-unavailable", "A test runs only where this toolchain can build it", &["build/tags"]),
    e!("test-source-export", "A test source exports nothing"),
    e!("test-source-import", "Nothing imports a test source", &["build/libraries"]),
    e!("test-timeout", "A suite finishes inside its `timeout_seconds`"),
    e!("textproto-duplicate-field", "A single-value field is set once", &["guides/textproto"]),
    e!("textproto-invalid-value", "A value is one its field holds", &["guides/textproto"]),
    e!("textproto-missing-header", "A text format file names its message", &["guides/textproto"]),
    e!("textproto-syntax", "A text format file parses", &["guides/textproto"]),
    e!("textproto-unknown-field", "A field is one its message declares", &["guides/textproto"]),
    e!("textproto-unknown-message", "A text format file names a message its schema declares", &["guides/textproto"]),
    e!("textproto-unsupported", "The text format reader refuses what it cannot read", &["guides/textproto"]),
    e!("tool-diagnostic", "A tool reports under a code the catalogue has", &["build/tools"]),
    e!("tool-effect-unavailable", "A tool's `ctx` holds only an allocator", &["build/tools"]),
    e!("tool-entry-point-not-exported", "A tool exports the entry point each block declares", &["build/tools"]),
    e!("tool-failed", "A tool answers", &["build/tools"]),
    e!("tool-missing-block", "A tool declares every entry point it exports", &["build/tools"]),
    e!("tool-missing-entry-point", "A tool has the entry point it is asked for", &["build/tools"]),
    e!("tool-request-mismatch", "A tool with a contract takes its root type", &["build/tools"]),
    e!("tool-source-import", "Nothing imports a tool's modules", &["build/tools"]),
    e!("trait-not-derivable", "Only some traits can be derived"),
    e!("trait-not-type", "A trait is a bound, not a type"),
    e!("try-error-mismatch", "`?` does not convert the error type"),
    e!("try-operand", "`?` takes a `Result` or an `Option`"),
    e!("try-return-mismatch", "`?` needs a matching return type"),
    e!("tuple-element-count", "A tuple has two elements or more"),
    e!("tuple-struct-not-called", "A tuple struct's name constructs one"),
    e!("turbofish", "Type arguments are written without `::`"),
    e!("type-argument-count", "A type takes the arguments it declares"),
    e!("type-arguments-on-value", "Type arguments qualify a function, not a value"),
    e!("type-mismatch", "A value has the type its position expects"),
    e!("type-not-crossable", "A value crosses to a `js` file by the crossing table", &["build/platforms"]),
    e!("type-not-value", "A type's name is not a value"),
    e!("typed-self", "`self` is written without a type", &["language/expressions"]),
    e!("unbraced-unicode-escape", "A Unicode escape braces its code point"),
    e!("unclosed-delimiter", "Every delimiter a construct opens is closed"),
    e!("undeclared-testing-surface", "A `testing/` directory is declared by a `testing` block", &["build/build-files"]),
    e!("underivable-field", "A derived trait holds for every field"),
    e!("undetermined-type", "A call resolves to a concrete type"),
    e!("unexpected-character", "Every byte of a source file starts a token"),
    e!("unexpected-token", "The grammar expected something else here"),
    e!("uninhabited", "Every type has a finite value"),
    e!("unknown-effect", "A context binding names a declared effect"),
    e!("unknown-entry", "An output fills the entries its platform offers", &["build/build-files"]),
    e!("unknown-entry-function", "An output enters through a function its binary exports", &["build/build-files"]),
    e!("unknown-escape", "A backslash escape is one of a closed set"),
    e!("unknown-export", "An import names something its module exports"),
    e!("unknown-field", "A field is one its type declares"),
    e!("unknown-method", "A method is one its type declares"),
    e!("unknown-module", "A module path names exactly one file"),
    e!("unknown-name", "Every name resolves to a declaration"),
    e!("unknown-platform", "A platform is bundled or a repository's `platform` rule", &["build/build-files"]),
    e!("unknown-platform-variant", "A variant is one its platform declares", &["build/build-files"]),
    e!("unknown-positional-field", "A tuple struct's fields are numbered from zero"),
    e!("unknown-schema", "A schema path names a file", &["guides/json"]),
    e!("unknown-source", "Every source a rule lists exists", &["build/build-files"]),
    e!("unknown-tag", "Every tag is declared in REPO.buri", &["build/tags"]),
    e!("unknown-tool", "A tool name names a declared tool", &["build/tools"]),
    e!("unknown-tuple-element", "A tuple's elements are numbered from zero"),
    e!("unknown-type", "Every type name resolves to a declaration"),
    e!("unknown-variant", "A variant is one its enum declares"),
    e!("unknown-visibility", "A visibility entry is one of five forms", &["build/build-files"]),
    e!("unnamed-namespace-import", "A namespace import must be named"),
    e!("unreachable-alternative", "Every alternative of an or-pattern must be reachable"),
    e!("unreachable-arm", "Every arm must be reachable"),
    e!("unterminated-character", "A character literal closes its quote"),
    e!("unterminated-comment", "A block comment is closed"),
    e!("unterminated-string", "A string literal closes on the line it opens"),
    e!("unterminated-unicode-escape", "A Unicode escape closes its brace"),
    e!("untyped-struct-literal", "An untyped literal needs an expected type", &["language/types"]),
    e!("untyped-variant", "A `.Variant` needs a known expected type"),
    e!("variant-export", "An exported enum exports every variant", &["language/types"]),
    e!("visibility-violation", "A dependency is visible to the package that names it", &["build/build-files"]),
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

    /// Every code is migrated, so a page with no `---` block is one that lost
    /// it. Without this the loss is silent: `every_page_parses` only sees the
    /// pages that opened a block, and the emission site does not panic until
    /// something provokes the code.
    #[test]
    fn every_page_carries_its_wording() {
        let missing: Vec<&str> =
            ERRORS.iter().map(|e| e.code).filter(|code| page(code).is_none()).collect();
        assert!(
            missing.is_empty(),
            "these pages carry no `---` frontmatter block, so their diagnostics have no \
             message to print:\n  {}",
            missing.join("\n  ")
        );
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
