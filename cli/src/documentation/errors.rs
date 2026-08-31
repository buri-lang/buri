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
    e!("ambiguous-free-function", "A method called as a free function names one type"),
    e!("ambiguous-trait-method", "Two bounds declaring one method name need disambiguating"),
    e!("argument-count-mismatch", "A call passes the arguments the value's type declares"),
    e!("array-impl-outside-core-list", "The defining module of `[T]` is `core/list`"),
    e!("binary-entry-import", "A binary's entry point is imported only by its own tests", &["build/libraries"]),
    e!("binary-field-not-allowed", "A binary has no platforms of its own, and nothing depends on it", &["build/build-files"]),
    e!("binary-internal-import", "A binary reaches the library beside it through its surface", &["build/build-files"]),
    e!("binary-source-import", "A library does not reach the binary beside it", &["build/build-files"]),
    e!("bitwise-on-a-non-integer", "The bitwise operators are defined on integers"),
    e!("build-file-syntax", "A build file is a well-formed textproto", &["build/build-files"]),
    e!("chain-too-long", "A chain has a bounded length"),
    e!("chained-comparison", "Comparison operators do not chain"),
    e!("character-literal-length", "A character literal holds one scalar value"),
    e!("circular-import", "Modules form a graph with no cycles"),
    e!(
        "circular-type-alias",
        "A type alias expands to a type, not back to itself",
        &["lang/types"]
    ),
    e!("coalesce-operand", "`??` supplies a default for an absent or failed value"),
    e!("colon-colon-not-an-operator", "A module's members are reached with `.`"),
    e!("const-declaration", "A module-level binding is written with `let`", &["lang/lexical"]),
    e!("context-binding-not-an-effect", "A context binding names an effect"),
    e!("context-call-with-arguments", "A context declaration takes no parameters"),
    e!("context-declaration-not-allowed", "A `context` declaration lives where a context may be built"),
    e!("context-export", "A `context` is exported only from a test-only module"),
    e!("context-not-a-value", "A context declaration is called, not named"),
    e!("context-not-allowed", "A context may only be built where authority enters"),
    e!("context-not-called", "A context is constructed by calling it"),
    e!("context-spread-operand", "A context spread takes another context"),
    e!("ctx-not-first", "`ctx` comes first, or immediately after `self`"),
    e!("declaration-without-a-body", "A declaration outside a trait or effect has a body"),
    e!("derive-not-a-trait", "A `derive` names a declared trait"),
    e!("derive-only-trait", "Some traits are derived, never implemented", &["guide/standard-library"]),
    e!("derive-operator-not-a-newtype", "A derived arithmetic operator wraps exactly one value"),
    e!("derive-operator-not-numeric", "A derived arithmetic operator is the operation on the wrapped value"),
    e!("derive-target-not-a-type", "A `derive` names a declared type"),
    e!("derive-without-traits", "A `derive` clause names at least one trait"),
    e!("duplicate-bound", "A bound is named once"),
    e!("duplicate-ctx-parameter", "A function takes one context"),
    e!("duplicate-declaration", "A name is declared once"),
    e!("duplicate-field", "A field name is used once"),
    e!("duplicate-field-initializer", "A struct literal gives each field once"),
    e!("duplicate-implementation", "There is one implementation per trait and type"),
    e!("duplicate-method", "A type has one method of each name"),
    e!("duplicate-module-declaration", "A name has one meaning in a module"),
    e!("duplicate-pattern-binding", "A pattern binds each name once"),
    e!("duplicate-rest-pattern", "An array pattern has one rest pattern"),
    e!("duplicate-tag", "A tag is declared once", &["build/tags"]),
    e!("duplicate-test-name", "A test name is used once per file"),
    e!("effect-and-trait", "No type implements both an effect and a trait"),
    e!("effect-carrying-bound", "A type that carries an effect satisfies no trait bound"),
    e!("effect-method-call", "An effect is performed through a function, not a method", &["lang/effects"]),
    e!("effect-outside-platform", "Only a platform module declares an effect"),
    e!("effect-param-not-ctx", "An effect-carrying parameter is `self` or `ctx`"),
    e!("entry-point-listed", "An entry point is named by its rule, never listed", &["build/build-files"]),
    e!("enum-without-a-variant", "An enum is named through one of its variants"),
    e!("error-type-mismatch", "`?` does not convert the error type"),
    e!("example-without-main", "A documented example that runs exports `main`"),
    e!("expression-statement", "An expression statement is legal only in a test"),
    e!("expression-too-deep", "An expression nests to a bounded depth"),
    e!("field-not-callable", "A field holding a value is not a method"),
    e!("field-wrong-kind", "A build-file field holds one kind of value", &["build/build-files"]),
    e!("float-as-a-tuple-index", "Two tuple indices in a row lex as a float"),
    e!("generic-effect-unsupported", "A trait or an effect takes no type parameters of its own"),
    e!("host-import", "`core/host` is imported by the module that exports `main`", &["build/hermeticity"]),
    e!("host-not-granted", "A platform grants the effects its host exports", &["build/build-files"]),
    e!("if-without-else", "`if` is an expression, so it needs an `else`"),
    e!("impl-body-not-a-method", "An `impl` body holds methods"),
    e!("impl-fn-without-self", "Everything in an `impl` takes `self`"),
    e!("impl-head-not-a-type", "An `impl` names a declared type"),
    e!("impl-method-export", "An `impl` method is not separately exported"),
    e!("impl-outside-its-module", "An `impl` or a `derive` lives in its type's own module"),
    e!("import-path-without-a-file", "A module that is not a surface is named by its file", &["lang/modules"]),
    e!("incomplete-impl", "An `impl` supplies every method of its trait"),
    e!("integer-not-in-base", "An integer literal is written in the base its prefix names"),
    e!("integer-too-wide", "An integer literal fits in 128 bits"),
    e!("integer-without-digits", "A base prefix is followed by digits"),
    e!("internal-import", "A library is reached through its surface", &["build/libraries"]),
    e!("lambda-captures-effect", "A lambda may not capture an effect"),
    e!("lambda-captures-generic", "A lambda may not capture a value that could be a context"),
    e!("literal-out-of-range", "A literal must fit the type it is pinned to"),
    e!("main-signature", "`main` has one shape"),
    e!("match-not-exhaustive", "A `match` covers every case"),
    e!("method-declared-free", "A method is declared inside an `impl`"),
    e!("method-not-a-value", "A method is not a value"),
    e!("method-supplied-twice", "An `impl` supplies each method once"),
    e!("missing-arrow", "A match arm is `pattern => expression`"),
    e!("missing-conformance", "Conformance is declared, never inferred"),
    e!("missing-field-pattern", "A struct pattern mentions every field"),
    e!("missing-field-value", "A literal gives every required field a value"),
    e!("missing-payload-pattern", "A variant with a payload is matched with one"),
    e!("missing-separator", "A list separates its elements with `,`"),
    e!("missing-terminator", "A declaration ends with `;`"),
    e!("module-doc-not-first", "`//!` documents the module, so it comes first"),
    e!("module-not-found", "A module path names exactly one file"),
    e!("module-outside-repository", "A `//` path needs a repository to be relative to"),
    e!(
        "networking-not-available",
        "A program that uses the network needs a toolchain built with networking"
    ),
    e!("no-assignment", "A binding is given its value once"),
    e!("no-main", "A binary is entered through `main`", &["build/build-files"]),
    e!("no-structural-derive", "Only some traits have a structural derivation"),
    e!("no-such-export", "A module exports what it says it exports"),
    e!("no-such-field", "A field is named by the type that declares it"),
    e!("no-such-member", "A namespace member is named by the module that exports it"),
    e!("no-such-method", "A method is looked up in its type's defining module"),
    e!("no-such-module", "A module path names a module that exists"),
    e!("no-such-positional-field", "A tuple struct's fields are numbered from zero"),
    e!("no-such-source", "Every source a rule lists exists", &["build/build-files"]),
    e!("no-such-tuple-element", "A tuple's elements are numbered from zero"),
    e!("no-such-variant", "A variant is named by the enum that declares it"),
    e!("no-type-arguments", "A built-in type takes no type arguments"),
    e!("not-a-bare-word", "An enumerated build-file value is a bare word", &["build/build-files"]),
    e!("not-a-float-literal", "A float literal has a digit on each side of the point"),
    e!("not-a-scalar-value", "A Unicode escape names a scalar value"),
    e!("not-a-struct-literal-head", "A field literal is headed by a struct or a variant"),
    e!("not-a-trait", "A bound names a trait or an effect"),
    e!("not-a-trait-method", "An `impl` supplies the methods its trait declares"),
    e!("not-a-tuple", "A numeric field access indexes a tuple"),
    e!("not-a-tuple-index", "A tuple index is a plain decimal number"),
    e!("not-an-effect", "A context binds effects"),
    e!("not-an-enum", "A `.Variant` form names an enum"),
    e!("not-callable", "A call names a function or a lambda"),
    e!("not-indexable", "Indexing is defined on arrays"),
    e!("not-interpolatable", "A hole in a string holds a primitive"),
    e!("not-on-the-surface", "A library is reached through its surface"),
    e!("or-pattern-bindings", "Or-pattern alternatives bind the same names"),
    e!("output-with-an-architecture", "Only a native output names an architecture", &["build/build-files"]),
    e!("output-without-a-platform", "An output is the artifact for one platform", &["build/build-files"]),
    e!("package-without-a-rule", "A build file declares a library or a binary", &["build/build-files"]),
    e!("pattern-not-a-tuple", "A tuple pattern matches a tuple of that arity"),
    e!("pattern-not-an-array", "An array pattern matches an array"),
    e!("pattern-type-mismatch", "A pattern matches the shape of the scrutinee"),
    e!("platform-not-implemented", "A test runs only on a platform this toolchain can build", &["build/tags"]),
    e!("platform-violation", "A target is built only for a platform its closure admits", &["build/tags"]),
    e!("platforms-under-forbids", "A platform restriction is a whitelist under `requires`", &["build/tags"]),
    e!("postfix-on-a-block", "A block-like expression is not the head of a postfix chain"),
    e!("private-to-module", "A private declaration is private to its module", &["build/libraries"]),
    e!("proto-ambiguous-type", "A type name says which schema it means", &["build/proto"]),
    e!("proto-circular-import", "Schemas form a graph with no cycles", &["build/proto"]),
    e!("proto-duplicate-type", "A fully-qualified name names one type", &["build/proto"]),
    e!("proto-edition", "A schema declares the one edition this reader implements", &["build/proto"]),
    e!("proto-edition-missing", "A schema declares its edition", &["build/proto"]),
    e!("proto-import-not-found", "A schema's import is written from the repository root", &["build/proto"]),
    e!("proto-schema", "A `.proto` file is a well-formed schema", &["build/proto"]),
    e!("proto-source-not-a-schema", "`proto_sources` holds schemas", &["build/proto"]),
    e!("proto-syntax-declaration", "A schema declares an edition, not a syntax", &["build/proto"]),
    e!("proto-unknown-feature", "Every `features` value is one the reader models", &["build/proto"]),
    e!("proto-unknown-type", "A field's type names a message or an enum", &["build/proto"]),
    e!("proto-unsupported", "The schema reader refuses what it cannot express", &["build/proto"]),
    e!("question-mark-mismatch", "`?` propagates into a matching return type"),
    e!("re-export-with-a-leading-export", "A re-export is written without a leading `export`"),
    e!("refutable-pattern", "A `let` pattern must match every value"),
    e!("relative-import", "Every module path is absolute"),
    e!("reserved-word", "Reserved words are not identifiers"),
    e!("retired-test-data", "A suite's filesystem is written in the suite", &["build/build-files", "build/testing"]),
    e!("rest-pattern-not-last", "A rest pattern comes last"),
    e!("result-discarded", "A `Result` may not be discarded", &["build/cli"]),
    e!("self-not-first", "`self` is the first parameter or nothing"),
    e!("self-outside-a-method", "`self` is legal only in a method body"),
    e!("self-type-outside-impl", "`Self` names the implementing type"),
    e!("self-with-a-type", "`self` is written without a type", &["lang/expressions"]),
    e!("signature-mismatch", "An `impl` supplies the signature its trait declares"),
    e!("statement-not-unit", "A statement's value is used or bound"),
    e!("struct-literal-head", "A struct literal is headed by a type"),
    e!(
        "struct-literal-type",
        "An anonymous literal takes its type from its surroundings",
        &["lang/types"]
    ),
    e!("style-not-static", "A conditional style is known at compile time", &["guide/user-interfaces"]),
    e!("tag-name-not-a-string", "A tag is named by a quoted string", &["build/tags"]),
    e!("tag-not-a-block", "A `tag` is a block in REPO.buri", &["build/tags"]),
    e!("tag-violation", "Two tags that forbid each other stay out of one closure", &["build/tags"]),
    e!("tag-without-a-name", "A tag is identified by its name", &["build/tags"]),
    e!("tags-under-requires", "A required tag would be forced onto every library", &["build/tags"]),
    e!("test-internal-import", "A test reaches its library the way a dependent does", &["build/libraries"]),
    e!("test-only-import", "A `testing` module is reachable only from a test", &["build/libraries"]),
    e!("test-outside-test-source", "A `test` lives in a test source"),
    e!("test-source-export", "A test source exports nothing"),
    e!("test-source-import", "A test source is not a module anybody can name", &["build/libraries"]),
    e!("test-timeout", "A suite finishes inside its `timeout_seconds`"),
    e!("trait-not-an-effect", "A context binds effects, not traits"),
    e!("trait-not-derivable", "A trait is derivable or it is written by hand"),
    e!("trait-used-as-a-type", "A trait is a bound, not a type"),
    e!("try-operand", "`?` propagates a failure"),
    e!("tuple-arity", "A tuple has two elements or more"),
    e!("tuple-struct-not-called", "A tuple struct's name constructs one"),
    e!("tuple-type-arity", "A tuple type has two elements or more"),
    e!("turbofish", "Type arguments are written without `::`"),
    e!("type-args-on-a-value", "Type arguments qualify a function, not a value"),
    e!("type-argument-arity", "A type's arguments are named in full or not at all"),
    e!("type-argument-count", "A type is written with the arguments its declaration takes"),
    e!("type-argument-mismatch", "A call supplies the type arguments its function declares"),
    e!("type-arguments-required", "A trait method call resolves to a concrete type"),
    e!("type-has-no-methods", "Only a declared type has methods"),
    e!("type-mismatch", "There are no implicit conversions"),
    e!("type-not-a-value", "A type's name is not a value"),
    e!("type-parameter-with-arguments", "A type parameter stands for one type"),
    e!("unannotated-variant", "A `.Variant` needs a known expected type"),
    e!("unbound-effect", "A context binds the effects the code it is passed to needs"),
    e!("unbraced-unicode-escape", "A Unicode escape braces its code point"),
    e!("unclosed-delimiter", "Every delimiter a construct opens is closed"),
    e!("undeclared-testing-surface", "A `testing/` directory is declared by a `testing` block", &["build/build-files"]),
    e!("underivable", "A derive is a fold over the type's components"),
    e!("unexpected-character", "Every byte of a source file starts a token"),
    e!("unexpected-token", "The grammar expected something else here"),
    e!("uninhabited", "A type with no finite value cannot be constructed"),
    e!("unknown-bare-word", "A build-file field takes one of a closed set of words", &["build/build-files"]),
    e!("unknown-escape", "A backslash escape is one of a closed set"),
    e!("unknown-field", "A build file names only the fields its schema declares", &["build/build-files"]),
    e!("unknown-tag", "Every tag is declared in REPO.buri", &["build/tags"]),
    e!("unknown-visibility", "A visibility entry is one of five forms", &["build/build-files"]),
    e!("unnamed-namespace-import", "A namespace import must be named"),
    e!("unplaceable-source", "A generated rule places a source where the imports already put it", &["build/build-files"]),
    e!("unreachable-alternative", "Every alternative of an or-pattern must be reachable"),
    e!("unreachable-arm", "Every arm must be reachable"),
    e!("unresolved-name", "Every name resolves to a declaration"),
    e!("unresolved-type", "Every type name resolves to a declaration"),
    e!("unresolved-type-in-pattern", "A pattern's path names a type or a variant"),
    e!("unsatisfied-bound", "A bound is satisfied by a declaration"),
    e!("unterminated-character", "A character literal closes its quote"),
    e!("unterminated-comment", "A block comment is closed"),
    e!("unterminated-string", "A string literal closes on the line it opens"),
    e!("unterminated-unicode-escape", "A Unicode escape closes its brace"),
    e!("variant-export", "An exported enum exports every variant", &["lang/types"]),
    e!("visibility-violation", "A dependency is visible to the package that names it", &["build/build-files"]),
    e!("web-output-with-a-js-block", "A page is always an ES module", &["build/build-files"]),
    e!("wrong-argument-count", "A call passes exactly the arguments the function declares"),
    e!("wrong-matched-value-count", "A payload pattern matches the values the variant holds"),
    e!("wrong-value-count", "A constructor is given the values it holds"),
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
