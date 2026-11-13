//! What a type's name means where a signature or a `let` writes it: each
//! built-in name its own type, an alias what it stands for, and the same
//! type variables and parameters in every body, however many a body mints.
use crate::harness::*;

/// `//app`, a JavaScript binary whose `main.buri` is `source`.
fn app(source: &str) -> Scratch {
    let scratch = Scratch::repo("named-types");
    scratch.binary_package("app", source);
    scratch
}

const HEADER: &str = "from \"node\" import { NodeHost };\n\
from \"platform/effect\" import { Allocator, Stdout };\n\
from \"core/io\" import * as io;\n\n";

/// `main` printing `shown`, with `items` above it.
fn printing(items: &str, shown: &str) -> String {
    format!(
        "{HEADER}{items}\n\
         export fn main(host: NodeHost): Result<(), Str> {{\n    \
         let ctx = context {{ Allocator: host.alloc, Stdout: host.stdout }};\n    \
         let _ = io.println(ctx, {shown}).ignore();\n    \
         .Ok(())\n}}\n"
    )
}

/// **Each built-in name annotates as its own type**, each alias as the type
/// it stands for: a value at its type's edge passes through a parameter of
/// that type unchanged.
#[test]
fn each_built_in_name_annotates_as_its_own_type() {
    let names = [
        ("I8", "-128"),
        ("I16", "-32768"),
        ("I32", "-2147483648"),
        ("I64", "9"),
        ("Int", "-1"),
        ("U8", "255"),
        ("U16", "65535"),
        ("U32", "4294967295"),
        ("U64", "7"),
        ("Uint", "3"),
        ("Byte", "1"),
        ("F64", "1.5"),
        ("Float", "2.5"),
        ("Bool", "true"),
        ("Char", "'z'"),
        ("Str", "\"s\""),
    ];
    let items: String = names
        .iter()
        .map(|(name, _)| format!("fn through{name}(x: {name}): {name} {{\n    x\n}}\n\n"))
        .collect();
    let holes: Vec<String> = names
        .iter()
        .map(|(name, value)| format!("${{through{name}({value})}}"))
        .collect();
    let shown = format!("\"{}\"", holes.join(" "));
    let scratch = app(&printing(&items, &shown));
    scratch.run(&["build", "//app"]).ok();
    scratch
        .exec_js("app")
        .ok()
        .says("-128 -32768 -2147483648 9 -1 255 65535 4294967295 7 3 1 1.5 2.5 true z s");
}

/// A value of one built-in type where a signature names another is refused,
/// and the message names the type the signature wrote.
#[test]
fn a_value_of_another_type_is_refused_at_each_built_in_name() {
    for name in [
        "I8", "I16", "I32", "I64", "I128", "U8", "U16", "U32", "U64", "U128", "F32", "F64", "Char",
        "Str",
    ] {
        let items = format!("fn wrong(): {name} {{\n    true\n}}\n");
        let run = app(&printing(&items, "\"x\"")).run(&["build", "//app"]);
        run.exits(1);
        run.says(&format!("expected `{name}`, found `Bool`"))
            .says("app/main.buri:5:");
    }
}

/// **An alias means what it stands for**, generic or not, as a parameter, a
/// result and a `let`'s annotation alike.
#[test]
fn an_alias_means_what_it_stands_for() {
    let items = "type Twin<T> = (T, T);\n\
                 type Label = Str;\n\n\
                 fn twin<T>(x: T): Twin<T> {\n    (x, x)\n}\n\n\
                 fn swap<A, B>(p: (A, B)): (B, A) {\n    (p.1, p.0)\n}\n\n\
                 fn label(n: Int): Label {\n    if (n > 0) { \"up\" } else { \"down\" }\n}\n";
    let shown = "{\n        let pair: Twin<Label> = twin(label(4));\n        \
                 let flipped: (Bool, Int) = swap((7, false));\n        \
                 \"${pair.0} ${pair.1} ${flipped.0} ${flipped.1}\"\n    }";
    let scratch = app(&printing(items, shown));
    scratch.run(&["build", "//app"]).ok();
    scratch.exec_js("app").ok().says("up up false 7");
}

/// **A body with thousands of literals checks and runs**: each literal is a
/// type variable of its own, past the ids the checker keeps closest to hand,
/// and every one of them still settles on `Int`.
#[test]
fn a_body_minting_thousands_of_type_variables_checks() {
    let literals: Vec<String> = (0..3000).map(|n| n.to_string()).collect();
    let items = format!(
        "fn total(): Int {{\n    let xs = [{}];\n    xs.fold(fn(a, b) => a + b, 0)\n}}\n",
        literals.join(", ")
    );
    let scratch = app(&printing(&items, "\"${total()}\""));
    scratch.run(&["build", "//app"]).ok();
    scratch.exec_js("app").ok().says("4498500");
}
