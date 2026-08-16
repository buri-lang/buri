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
        "core/option" => include_str!("sources/option.buri"),
        "core/result" => include_str!("sources/result.buri"),
        "core/order" => include_str!("sources/order.buri"),
        "core/num" => include_str!("sources/num.buri"),
        "core/list" => include_str!("sources/list.buri"),
        "core/queue" => include_str!("sources/queue.buri"),
        "core/bitset" => include_str!("sources/bitset.buri"),
        "core/json" => include_str!("sources/json.buri"),
        "core/proto" => include_str!("sources/proto.buri"),
        "core/map" => include_str!("sources/map.buri"),
        "core/set" => include_str!("sources/set.buri"),
        "core/str" => include_str!("sources/str.buri"),
        "core/bytes" => include_str!("sources/bytes.buri"),
        "core/crypto" => include_str!("sources/crypto.buri"),
        "core/char" => include_str!("sources/char.buri"),
        "core/bool" => include_str!("sources/bool.buri"),
        "core/math" => include_str!("sources/math.buri"),
        "core/simd" => include_str!("sources/simd.buri"),
        "core/bits" => include_str!("sources/bits.buri"),
        "core/cap" => include_str!("sources/cap.buri"),
        "core/host" => include_str!("sources/host.buri"),
        "core/io" => include_str!("sources/io.buri"),
        "core/fs" => include_str!("sources/fs.buri"),
        "core/env" => include_str!("sources/env.buri"),
        "core/time" => include_str!("sources/time.buri"),
        "core/date" => include_str!("sources/date.buri"),
        "core/random" => include_str!("sources/random.buri"),
        "core/net/http" => include_str!("sources/http.buri"),
        "core/testing/assert" => include_str!("sources/assert.buri"),
        "core/testing/context" => include_str!("sources/testing_context.buri"),
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
    "core/queue",
    "core/bitset",
    "core/json",
    "core/proto",
    "core/map",
    "core/set",
    "core/str",
    "core/bytes",
    "core/crypto",
    "core/char",
    "core/bool",
    "core/math",
    "core/simd",
    "core/bits",
    "core/cap",
    "core/host",
    "core/io",
    "core/fs",
    "core/env",
    "core/time",
    "core/date",
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

/// The standard library modules every compilation loads whether or not
/// anything imports them.
///
/// Not an optimisation boundary — a correctness one. A method needs no import
/// (SPEC 6.7.3): `xs.map(...)` resolves in `[T]`'s defining module, which is
/// `core/list`, and `s.trim()` in `core/str`. Those modules have to be present
/// for a program that never names them, so they load eagerly. Everything else
/// declares methods only on its *own* types, which cannot exist in a program
/// that did not import the module that declares them — so it loads on import,
/// and a repository does not pay to parse `core/crypto` to compile a program
/// that has never heard of it.
///
/// Adding a module here is safe. *Removing* one is not, unless nothing in it
/// declares a method on a built-in type — which `semantics::resolve` enforces rather
/// than leaving to review.
pub const EAGER_MODULES: &[&str] = &[
    "core/option",
    "core/result",
    "core/order",
    "core/num",
    "core/list",
    "core/str",
    "core/char",
    "core/bool",
];

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
