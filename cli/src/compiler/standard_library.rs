//! The embedded standard library: `buri-stdlib`'s table, and the questions
//! about it that need the checker's types or the build-file reader.

pub use buri_stdlib::compiler::standard_library::*;

use crate::build::buildfile::PlatformName;
use crate::compiler::semantics::types::Prim;

/// A bundled platform's name and the host type its `platform.buri` declares:
/// `("native", "NativeHost")`.
pub fn host_type_of(platform: &str) -> Option<(&'static str, &'static str)> {
    PlatformName::bundled(platform).map(|p| (p.name(), p.host()))
}

/// Whether `name` is one of the entries a bundled platform's `platform.buri`
/// declares without a body, for a program to fill.
pub fn is_entry_declaration(module: &str, name: &str) -> bool {
    let canonical = module.strip_suffix("/lib.buri").unwrap_or(module);
    PlatformName::bundled(canonical).is_some_and(|p| p.entries().contains(&name))
}

/// The same, for what gets built.
pub fn host_type(platform: crate::build::buildfile::Platform) -> Option<(&'static str, &'static str)> {
    host_type_of(platform.slug())
}

/// The defining module of each built-in type (SPEC 6.7.3). A type's operations
/// travel with it, so this is where `xs.map(...)` and `s.trim()` resolve.
///
/// Total over `Prim` rather than a `&str` match with a catch-all: a new
/// primitive is now a compile error here instead of silently landing in
/// `core/number`.
pub fn defining_module(p: Prim) -> &'static str {
    match p {
        Prim::Str => "core/str",
        Prim::Char => "core/character",
        Prim::Bool => "core/bool",
        // A template is a `Str` with holes, and its operations are the
        // numeric-rendering ones, so it shares `core/number`'s module the way
        // every numeric type does.
        Prim::Template => "core/number",
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
        | Prim::F64 => "core/number",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every type a primitive can be must have a module that exists.
    #[test]
    fn every_primitive_has_a_defining_module() {
        for p in Prim::all() {
            let path = defining_module(*p);
            assert!(find(path).is_some(), "`{}` names a module that does not exist", p.name());
        }
    }

    /// `buri-stdlib` keeps the bundled platforms' names as strings, because it
    /// sits below the enum.
    #[test]
    fn the_bundled_platforms_are_one_list() {
        let names: Vec<&str> = PlatformName::BUNDLED.iter().map(|p| p.name()).collect();
        assert_eq!(names, BUNDLED_PLATFORMS);
    }

    /// The mailbox bound is one number, written twice, and the two spellings
    /// must agree.
    ///
    /// `cli/runtime/rt.rs` refuses to take a message past the bound, and
    /// `core/actor` is where the number a reader of the module is told about
    /// lives. A number quoted in the documentation that could drift from the
    /// one the runtime enforces is a claim nobody can check.
    ///
    /// Read out of the two sources rather than shared as a constant, because
    /// they are two crates that never link against each other — the archive is
    /// `include_bytes!`d — which is the same reason `BURI_OK` is transcribed in
    /// `backend/runtime_table.rs` rather than imported.
    #[test]
    fn the_default_mailbox_is_the_one_core_actor_names() {
        const RUNTIME: &str = include_str!("../../runtime/rt.rs");
        let buri = find("core/actor").expect("`core/actor` is in the table").source;
        // `split` rather than `find` and a range, because a byte range into a
        // `&str` is `clippy::string_slice` and this needs no offset — what
        // follows the needle is what the second piece begins with.
        let named = |text: &str, needle: &str| -> String {
            text.split(needle)
                .nth(1)
                .unwrap_or_else(|| panic!("no `{needle}`"))
                .chars()
                .take_while(char::is_ascii_digit)
                .collect::<String>()
        };
        let module = named(buri, "let MAILBOX: Int = ");
        let runtime = named(RUNTIME, "pub const MAILBOX: i64 = ");
        assert!(!module.is_empty(), "`core/actor` names no default mailbox");
        assert_eq!(
            module, runtime,
            "`core/actor`'s MAILBOX is {module} and `cli/runtime/rt.rs`'s is {runtime}"
        );
    }
}
