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

pub struct ErrorDoc {
    pub code: &'static str,
    pub title: &'static str,
    pub text: &'static str,
}

macro_rules! e {
    ($code:literal, $title:literal) => {
        ErrorDoc { code: $code, title: $title, text: include_str!(concat!("../docs/errors/", $code, ".md")) }
    };
}

pub const ERRORS: &[ErrorDoc] = &[
    e!("chained-comparison", "Comparison operators do not chain"),
    e!("context-not-allowed", "A context may only be built where authority enters"),
    e!("ctx-not-first", "`ctx` comes first, or immediately after `self`"),
    e!("derive-only-trait", "Some traits are derived, never implemented"),
    e!("derive-without-traits", "A `derive` clause names at least one trait"),
    e!("duplicate-bound", "A bound is named once"),
    e!("duplicate-declaration", "A name is declared once"),
    e!("duplicate-field", "A field name is used once"),
    e!("duplicate-test-name", "A test name is used once per file"),
    e!("effect-and-trait", "No type implements both an effect and a trait"),
    e!("effect-outside-platform", "Only a platform module declares an effect"),
    e!("effect-param-not-ctx", "An effect-carrying parameter is `self` or `ctx`"),
    e!("expression-statement", "An expression statement is legal only in a test"),
    e!("host-not-granted", "A platform grants the effects its host exports"),
    e!("if-without-else", "`if` is an expression, so it needs an `else`"),
    e!("impl-fn-without-self", "Everything in an `impl` takes `self`"),
    e!("impl-method-export", "An `impl` method is not separately exported"),
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
    e!("private-to-module", "A private declaration is private to its module"),
    e!("question-mark-mismatch", "`?` propagates into a matching return type"),
    e!("refutable-pattern", "A `let` pattern must match every value"),
    e!("relative-import", "Every module path is absolute"),
    e!("reserved-word", "Reserved words are not identifiers"),
    e!("rest-pattern-not-last", "A rest pattern comes last"),
    e!("result-discarded", "A `Result` may not be discarded"),
    e!("self-not-first", "`self` is the first parameter or nothing"),
    e!("self-type-outside-impl", "`Self` names the implementing type"),
    e!("struct-literal-head", "A struct literal is headed by a type"),
    e!("style-not-static", "A conditional style is known at compile time"),
    e!("test-only-import", "A `testing` module is reachable only from a test"),
    e!("test-outside-test-source", "A `test` lives in a test source"),
    e!("turbofish", "Type arguments are written without `::`"),
    e!("type-args-on-a-value", "Type arguments qualify a function, not a value"),
    e!("type-mismatch", "There are no implicit conversions"),
    e!("unannotated-variant", "A `.Variant` needs a known expected type"),
    e!("underivable", "A derive is a fold over the type's components"),
    e!("unexpected-token", "The grammar expected something else here"),
    e!("uninhabited", "A type with no finite value cannot be constructed"),
    e!("unnamed-namespace-import", "A namespace import must be named"),
    e!("unreachable-arm", "Every arm must be reachable"),
    e!("unresolved-name", "Every name resolves to a declaration"),
    e!("unresolved-type", "Every type name resolves to a declaration"),
    e!("unterminated-comment", "A block comment is closed"),
];

pub fn find(code: &str) -> Option<&'static ErrorDoc> {
    ERRORS.iter().find(|e| e.code == code)
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
            assert!(
                e.text.contains(&format!("code={}", e.code)),
                "`{}`'s page has no reproduction tagged with its own code",
                e.code
            );
        }
    }
}
