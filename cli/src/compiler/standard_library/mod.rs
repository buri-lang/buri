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
//!
//! Everything the rest of the compiler asks about a standard library module is
//! answered from the one table below. It used to be five hand-maintained lists
//! of the same strings — a `source()` match, a `MODULES` array, an
//! `EAGER_MODULES` array, a `PRELUDE_MODULES` array and a `PRELUDE` array of
//! pairs — so a path in one and not another was representable: a module in
//! `MODULES` with no `source()` arm made the diagnostics suggest a near miss
//! that then failed to load, and a name in `PRELUDE` whose module was not
//! loaded eagerly was silently not in scope.

use crate::compiler::semantics::types::Prim;

/// One module of the embedded standard library.
pub struct StdModule {
    pub path: &'static str,
    /// The source text, embedded at build time. Present by construction, so
    /// there is no module the toolchain names and cannot load.
    pub source: &'static str,
    /// Loaded whether or not anything imports it. See `EAGER` below.
    pub eager: bool,
    /// Only platform modules may declare effects, so the set of things a Buri
    /// program can do to the world is fixed by its platform rather than
    /// open-ended (SPEC 10.1).
    pub platform: bool,
    /// Names this module puts into every module's scope without an import.
    /// `Option`, `Result` and `Order` are the prelude of SPEC 5.7; the operator
    /// and comparison traits are here because `derive Eq for Point;` appears in
    /// programs that import nothing from `core/order`, and because `a + b`
    /// means `Add.add` whether or not anyone wrote the name down.
    ///
    /// A module with a non-empty prelude must be eager — its names are in
    /// scope everywhere, so it is part of every compilation. `MODULES_AGREE`
    /// below checks that rather than leaving it to review.
    pub prelude: &'static [&'static str],
}

const fn m(path: &'static str, source: &'static str) -> StdModule {
    StdModule { path, source, eager: false, platform: false, prelude: &[] }
}

/// Every module the standard library provides. The single source of truth for
/// what exists, what its text is, when it loads, and what it publishes into
/// every scope.
pub const MODULES: &[StdModule] = &[
    StdModule {
        prelude: &["Option"],
        eager: true,
        ..m("core/option", include_str!("sources/option.buri"))
    },
    StdModule {
        prelude: &["Result"],
        eager: true,
        ..m("core/result", include_str!("sources/result.buri"))
    },
    StdModule {
        prelude: &["Order", "Eq", "Ord", "Show", "Hash"],
        eager: true,
        ..m("core/order", include_str!("sources/order.buri"))
    },
    StdModule {
        prelude: &[
            "Add",
            "Sub",
            "Mul",
            "Div",
            "Rem",
            "Neg",
            "Bounded",
            "Checked",
            "Wrapping",
            "Saturating",
            "RangeError",
        ],
        eager: true,
        ..m("core/num", include_str!("sources/num.buri"))
    },
    // `[T]`, `Str`, `Char` and `Bool` need their defining modules present in a
    // program that never names them, because a method needs no import
    // (SPEC 6.7.3): `xs.map(...)` resolves in `core/list` and `s.trim()` in
    // `core/str`. Everything below declares methods only on its *own* types,
    // which cannot exist in a program that did not import it — so it loads on
    // import, and a repository does not pay to parse `core/crypto` to compile
    // a program that has never heard of it.
    StdModule { eager: true, ..m("core/list", include_str!("sources/list.buri")) },
    StdModule { eager: true, ..m("core/str", include_str!("sources/str.buri")) },
    StdModule { eager: true, ..m("core/char", include_str!("sources/char.buri")) },
    StdModule { eager: true, ..m("core/bool", include_str!("sources/bool.buri")) },
    m("core/queue", include_str!("sources/queue.buri")),
    m("core/bitset", include_str!("sources/bitset.buri")),
    m("core/json", include_str!("sources/json.buri")),
    m("core/proto", include_str!("sources/proto.buri")),
    m("core/map", include_str!("sources/map.buri")),
    m("core/set", include_str!("sources/set.buri")),
    m("core/bytes", include_str!("sources/bytes.buri")),
    m("core/crypto", include_str!("sources/crypto.buri")),
    m("core/math", include_str!("sources/math.buri")),
    m("core/simd", include_str!("sources/simd.buri")),
    m("core/bits", include_str!("sources/bits.buri")),
    StdModule { platform: true, ..m("core/cap", include_str!("sources/cap.buri")) },
    StdModule { platform: true, ..m("core/host", include_str!("sources/host.buri")) },
    // Not a platform module, deliberately. It *implements* `Alloc` rather than
    // declaring it, and `Alloc` is the one effect whose implementation carries
    // no authority — a `Region` is a number, so a library that builds its own
    // allocator has been granted nothing (SPEC 10.5). That is why this is
    // importable anywhere and `core/host` is not.
    m("core/alloc", include_str!("sources/alloc.buri")),
    m("core/io", include_str!("sources/io.buri")),
    m("core/fs", include_str!("sources/fs.buri")),
    m("core/env", include_str!("sources/env.buri")),
    m("core/time", include_str!("sources/time.buri")),
    m("core/date", include_str!("sources/date.buri")),
    m("core/random", include_str!("sources/random.buri")),
    m("core/net/http", include_str!("sources/http.buri")),
    StdModule {
        platform: true,
        ..m("core/testing/assert", include_str!("sources/assert.buri"))
    },
    StdModule {
        platform: true,
        ..m("core/testing/context", include_str!("sources/testing_context.buri"))
    },
];

pub fn find(path: &str) -> Option<&'static StdModule> {
    MODULES.iter().find(|m| m.path == path)
}

/// Module path -> source text.
pub fn source(path: &str) -> Option<&'static str> {
    find(path).map(|m| m.source)
}

pub fn is_platform_module(path: &str) -> bool {
    find(path).is_some_and(|m| m.platform)
}

/// The modules a compilation always needs: the prelude, and the defining
/// module of every built-in type.
///
/// Adding a module here is safe. *Removing* one is not, unless nothing in it
/// declares a method on a built-in type — which `semantics::resolve` enforces
/// rather than leaving to review.
pub fn eager_modules() -> impl Iterator<Item = &'static str> {
    MODULES.iter().filter(|m| m.eager).map(|m| m.path)
}

/// The modules whose names are in scope in every module without an import.
pub fn prelude_modules() -> impl Iterator<Item = &'static str> {
    MODULES.iter().filter(|m| !m.prelude.is_empty()).map(|m| m.path)
}

/// `(module, exported name)` pairs injected into every module's scope, at
/// lower priority than the module's own declarations and its imports — so a
/// module may shadow any of them, and importing one explicitly is harmless.
pub fn prelude() -> impl Iterator<Item = (&'static str, &'static str)> {
    MODULES.iter().flat_map(|m| m.prelude.iter().map(move |n| (m.path, *n)))
}

/// The defining module of each built-in type (SPEC 6.7.3). A type's operations
/// travel with it, so this is where `xs.map(...)` and `s.trim()` resolve.
///
/// Total over `Prim` rather than a `&str` match with a catch-all: a new
/// primitive is now a compile error here instead of silently landing in
/// `core/num`.
pub fn defining_module(p: Prim) -> &'static str {
    match p {
        Prim::Str => "core/str",
        Prim::Char => "core/char",
        Prim::Bool => "core/bool",
        // A template is a `Str` with holes, and its operations are the
        // numeric-rendering ones, so it shares `core/num`'s module the way
        // every numeric type does.
        Prim::Template => "core/num",
        Prim::I8
        | Prim::I16
        | Prim::I32
        | Prim::I64
        | Prim::I128
        | Prim::U8
        | Prim::U16
        | Prim::U32
        | Prim::U64
        | Prim::U128
        | Prim::F32
        | Prim::F64 => "core/num",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A prelude name is in scope in every module, so its module has to be in
    /// every compilation.
    #[test]
    fn every_prelude_module_is_eager() {
        for m in MODULES {
            assert!(
                m.prelude.is_empty() || m.eager,
                "`{}` publishes a prelude name but does not load eagerly",
                m.path
            );
        }
    }

    #[test]
    fn every_module_path_is_distinct() {
        let mut seen: Vec<&str> = MODULES.iter().map(|m| m.path).collect();
        let before = seen.len();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(before, seen.len(), "two entries share a path");
    }

    /// Every type a primitive can be must have a module that exists.
    #[test]
    fn every_primitive_has_a_defining_module() {
        for p in Prim::all() {
            let path = defining_module(*p);
            assert!(find(path).is_some(), "`{}` names a module that does not exist", p.name());
        }
    }
}
