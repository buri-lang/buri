//! The embedded standard library.
//!
//! `core/*` ships with the toolchain and is never listed in a `dependencies`.
//! It is available to every target, and the purity tiers in SPEC 11.1 govern
//! what any given import of it can do.
//!
//! Modules here may declare a `fn` with no body. Those are the operations the
//! backend supplies — string and array primitives, the platform's effect
//! implementations, the test runner's — and every one of them must have an
//! entry in the backend's runtime or code generation fails loudly.

/// Module path -> source text.
pub fn source(path: &str) -> Option<&'static str> {
    Some(match path {
        "core/option" => include_str!("std/option.buri"),
        "core/result" => include_str!("std/result.buri"),
        "core/order" => include_str!("std/order.buri"),
        "core/num" => include_str!("std/num.buri"),
        "core/list" => include_str!("std/list.buri"),
        "core/str" => include_str!("std/str.buri"),
        "core/char" => include_str!("std/char.buri"),
        "core/bool" => include_str!("std/bool.buri"),
        "core/math" => include_str!("std/math.buri"),
        "core/bits" => include_str!("std/bits.buri"),
        "core/cap" => include_str!("std/cap.buri"),
        "core/host" => include_str!("std/host.buri"),
        "core/io" => include_str!("std/io.buri"),
        "core/fs" => include_str!("std/fs.buri"),
        "core/env" => include_str!("std/env.buri"),
        "core/time" => include_str!("std/time.buri"),
        "core/random" => include_str!("std/random.buri"),
        "core/net/http" => include_str!("std/http.buri"),
        "core/testing/assert" => include_str!("std/assert.buri"),
        "core/testing/context" => include_str!("std/testing_context.buri"),
        _ => return None,
    })
}

/// Every module the standard library provides, for diagnostics that want to
/// suggest a near miss.
pub const MODULES: &[&str] = &[
    "core/option",
    "core/result",
    "core/order",
    "core/num",
    "core/list",
    "core/str",
    "core/char",
    "core/bool",
    "core/math",
    "core/bits",
    "core/cap",
    "core/host",
    "core/io",
    "core/fs",
    "core/env",
    "core/time",
    "core/random",
    "core/net/http",
    "core/testing/assert",
    "core/testing/context",
];

/// Only platform modules may declare effects, so the set of things a Buri
/// program can do to the world is fixed by its platform rather than
/// open-ended (SPEC 10.1).
pub fn is_platform_module(path: &str) -> bool {
    matches!(path, "core/cap" | "core/host" | "core/testing/assert" | "core/testing/context")
}

/// The modules whose names are in scope in every module without an import.
/// `Option`, `Result` and `Order` are the prelude of SPEC 5.7; the operator
/// and comparison traits are here because `derive Eq for Point;` appears in
/// programs that import nothing from `core/order`, and because `a + b` means
/// `Add.add` whether or not anyone wrote the name down.
pub const PRELUDE_MODULES: &[&str] =
    &["core/option", "core/result", "core/order", "core/num"];

/// `(module, exported name)` pairs injected into every module's scope, at
/// lower priority than the module's own declarations and its imports — so a
/// module may shadow any of them, and importing one explicitly is harmless.
pub const PRELUDE: &[(&str, &str)] = &[
    ("core/option", "Option"),
    ("core/result", "Result"),
    ("core/order", "Order"),
    ("core/order", "Eq"),
    ("core/order", "Ord"),
    ("core/order", "Show"),
    ("core/order", "Hash"),
    ("core/num", "Add"),
    ("core/num", "Sub"),
    ("core/num", "Mul"),
    ("core/num", "Div"),
    ("core/num", "Rem"),
    ("core/num", "Neg"),
    ("core/num", "Bounded"),
    ("core/num", "Checked"),
    ("core/num", "Wrapping"),
    ("core/num", "Saturating"),
    ("core/num", "RangeError"),
];

/// The defining module of each built-in type (SPEC 6.7.3). A type's operations
/// travel with it, so this is where `xs.map(...)` and `s.trim()` resolve.
pub fn defining_module(builtin: &str) -> &'static str {
    match builtin {
        "Str" => "core/str",
        "Char" => "core/char",
        "Bool" => "core/bool",
        _ => "core/num",
    }
}
