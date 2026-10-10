//! A built-in type's name always means the built-in type, so a declaration
//! that reuses one is refused rather than left unreachable from every type
//! annotation.
use crate::harness::*;

/// Every name a type annotation reads as built in: each primitive, and the
/// four aliases.
const BUILT_IN: &[&str] = &[
    "Bool", "I8", "I16", "I32", "I64", "I128", "U8", "U16", "U32", "U64", "U128", "F32", "F64",
    "Char", "Str", "Template", "Int", "Float", "Uint", "Byte",
];

/// `//app`, a JavaScript binary whose `main.buri` declares `decl` beside an
/// empty `main`.
fn app(decl: &str) -> Scratch {
    let scratch = Scratch::repo("built-in-names");
    scratch.binary_package(
        "app",
        &format!(
            "from \"node\" import {{ NodeHost }};\n\n{decl}\n\n\
             export fn main(host: NodeHost): Result<(), Str> {{\n    .Ok(())\n}}\n"
        ),
    );
    scratch
}

/// A `struct` named after any built-in type is refused at its name.
#[test]
fn a_struct_named_after_a_built_in_type_is_refused() {
    for name in BUILT_IN {
        let run = app(&format!("struct {name} {{\n    x: U8,\n}}")).run(&["build", "//app"]);
        run.exits(1);
        run.says(&format!("`{name}` is a built-in type and may not be declared"))
            .says("[built-in-type-name]")
            .says("app/main.buri:3:8");
    }
}

/// So is an `enum`, and a `type` alias.
#[test]
fn an_enum_or_an_alias_named_after_a_built_in_type_is_refused() {
    for decl in ["enum Byte {\n    One,\n}", "type Int = U8;"] {
        let run = app(decl).run(&["build", "//app"]);
        run.exits(1);
        run.says("[built-in-type-name]");
    }
}

/// The issue's shape: `struct Byte` used in a type was silently `U8` there and
/// the struct in a literal. Now it is refused before either.
#[test]
fn a_struct_byte_is_refused_where_it_was_silently_u8() {
    let run = app(
        "struct Byte {\n    b: U8,\n}\n\nfn make(): Byte {\n    Byte { b: 1 }\n}",
    )
    .run(&["build", "//app"]);
    run.exits(1);
    run.says("[built-in-type-name]");
}

/// A name that only resembles a built-in one is an ordinary type.
#[test]
fn a_name_near_a_built_in_one_is_declared() {
    let run = app("struct Bytes {\n    b: U8,\n}\n\nstruct Integer {\n    n: Int,\n}")
        .run(&["build", "//app"]);
    run.ok();
}
