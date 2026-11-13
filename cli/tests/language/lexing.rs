//! What the lexer reads at the edges its fast paths stop at: a word against
//! its first eight bytes and the end of the file, an integer against the
//! nineteen digits a `u64` holds, and a comment against the line's end. Each
//! one-byte punctuator, read from a table, reads as itself, and a character
//! as its scalar value.
use crate::harness::*;

/// `//app`, a JavaScript binary whose `main.buri` is `source`.
fn app(source: &str) -> Scratch {
    let scratch = Scratch::repo("lexing");
    scratch.binary_package("app", source);
    scratch
}

/// **Every way of writing an integer reads as its value**, on either side of
/// nineteen digits and with whatever follows it.
#[test]
fn integer_literals_read_as_their_values() {
    let scratch = app(
        "from \"node\" import { NodeHost };\n\
         from \"platform/effect\" import { Stdout };\n\
         from \"core/io\" import * as io;\n\n\
         export fn main(host: NodeHost): Result<(), Str> {\n    \
         let ctx = context { Stdout: host.stdout };\n    \
         let pair = (10, 20);\n    \
         let small = [0, 7, 42, 007, 1_000_000, 0x1F, 0b101, 0o17, pair.0, pair.1];\n    \
         let wide = [1234567890123456789, 9223372036854775807, 9_223_372_036_854_775_807];\n    \
         let _ = io.println(ctx, \"${small} ${wide} ${1.5} ${2e3} ${1E-3} ${12.0}\").ignore();\n    \
         .Ok(())\n}\n",
    );
    scratch.run(&["build", "//app"]).ok();
    scratch.exec_js("app").ok().says(
        "[0, 7, 42, 7, 1000000, 31, 5, 15, 10, 20] \
         [1234567890123456789, 9223372036854775807, 9223372036854775807] 1.5 2000.0 0.001 12.0",
    );
}

/// **An integer past what any integer type holds is reported**, at twenty
/// digits as at forty.
#[test]
fn an_integer_wider_than_any_type_is_reported() {
    for digits in [20, 40] {
        let literal = "9".repeat(digits);
        let scratch = app(&format!(
            "from \"node\" import {{ NodeHost }};\n\n\
             export fn main(host: NodeHost): Result<(), Str> {{\n    let _ = {literal};\n    .Ok(())\n}}\n"
        ));
        let run = scratch.run(&["build", "//app"]);
        run.exits(1);
        run.says("app/main.buri:4:13");
    }
}

/// **A word is read whole**: past eight bytes, at the very end of the file,
/// and when it begins with a keyword.
#[test]
fn a_word_is_read_whole_wherever_it_ends() {
    for word in [
        "abcdefgh",
        "abcdefghi",
        "lets",
        "selfish",
        "Selfie",
        "fnord",
        "unreachables",
    ] {
        let scratch = app(&format!(
            "from \"node\" import {{ NodeHost }};\n\n\
             export fn main(host: NodeHost): Result<(), Str> {{\n    .Ok(())\n}}\n{word}"
        ));
        let run = scratch.run(&["build", "//app"]);
        run.exits(1);
        run.says(&format!("found `{word}`"))
            .says("app/main.buri:6:1");
    }
    // A reserved word longer than eight bytes, last in the file.
    let scratch = app("from \"node\" import { NodeHost };\n\n\
         export fn main(host: NodeHost): Result<(), Str> {\n    .Ok(())\n}\nunreachable");
    let run = scratch.run(&["build", "//app"]);
    run.exits(1);
    run.says("[reserved-word]").says("app/main.buri:6:1");
}

/// **A comment keeps its text and loses its trailing blanks**, whichever
/// blank it ends in and whether or not a line break follows it.
#[test]
fn a_comments_trailing_blanks_are_dropped() {
    let source = "// Comment, then spaces.   \n\
                  // Comment, then an em space.\u{2003}\n\
                  /// Doc, then spaces.   \n\
                  /// Doc, then a tab.\t\n\
                  /// Doc, then a no-break space.\u{a0}\n\
                  export fn answer(): Int {\n    42\n}\n\
                  // The last line, with no line break.  ";
    let scratch = Scratch::repo("lexing-comments");
    scratch.write("lib/BUILD.buri", "library {}\n");
    scratch.write("lib/lib.buri", source);
    scratch.run(&["format", "lib/lib.buri"]).ok();
    assert_eq!(
        scratch.read("lib/lib.buri"),
        "// Comment, then spaces.\n\
         // Comment, then an em space.\n\
         /// Doc, then spaces.\n\
         /// Doc, then a tab.\n\
         /// Doc, then a no-break space.\n\
         export fn answer(): Int {\n    42\n}\n\
         // The last line, with no line break.\n"
    );
}

/// **A file packed with tokens reads whole**: a list written as one token a
/// byte, far more than a file usually holds, with literals of every kind and
/// nested lists after it.
#[test]
fn a_file_packed_with_tokens_reads_whole() {
    let ones = vec!["1"; 4000].join(",");
    let scratch = app(&format!(
        "from \"node\" import {{ NodeHost }};\n\
         from \"platform/effect\" import {{ Stdout }};\n\
         from \"core/io\" import * as io;\n\n\
         export fn main(host: NodeHost): Result<(), Str> {{\n    \
         let ctx = context {{ Stdout: host.stdout }};\n    \
         let ones = [{ones}];\n    \
         let nested = [[[1], [2, 3]], [[4], [5, 6]]];\n    \
         let _ = io.println(ctx, \"${{ones}} ${{0x10}} ${{2.5}} ${{'b'}} ${{\"word\"}} ${{nested}}\").ignore();\n    \
         .Ok(())\n}}\n"
    ));
    scratch.run(&["build", "//app"]).ok();
    let printed = vec!["1"; 4000].join(", ");
    scratch.exec_js("app").ok().says(&format!(
        "[{printed}] 16 2.5 b word [[[1], [2, 3]], [[4], [5, 6]]]"
    ));
}

/// **Each one-byte punctuator reads as itself**: brackets, the separators,
/// the arithmetic operators, `^`, `~` and a pattern's `@`.
#[test]
fn each_one_byte_punctuator_reads_as_itself() {
    let scratch = app("from \"node\" import { NodeHost };\n\
         from \"platform/effect\" import { Stdout };\n\
         from \"core/io\" import * as io;\n\n\
         export fn main(host: NodeHost): Result<(), Str> {\n    \
         let ctx = context { Stdout: host.stdout };\n    \
         let a: I64 = 17;\n    \
         let b = (a + 3) * 2 - a % 4;\n    \
         let c = a ^ 5;\n    \
         let d = ~a;\n    \
         let e = match (a) { n @ 17 => n * 2, _ => 0 };\n    \
         let _ = io.println(ctx, \"${[a, b, c, d, e]}\").ignore();\n    \
         .Ok(())\n}\n");
    scratch.run(&["build", "//app"]).ok();
    scratch.exec_js("app").ok().says("[17, 39, 20, -18, 34]");
}

/// **A character literal reads as its scalar value**, whichever of its three
/// bytes the value fills, and each token beside it keeps its own kind.
#[test]
fn a_character_reads_as_its_scalar_value() {
    let scratch = app("from \"node\" import { NodeHost };\n\
         from \"platform/effect\" import { Stdout };\n\
         from \"core/io\" import * as io;\n\n\
         export fn main(host: NodeHost): Result<(), Str> {\n    \
         let ctx = context { Stdout: host.stdout };\n    \
         let _ = io.println(ctx, \"${'\\u{0}'.toU32()} ${'A'.toU32()} ${'\\u{ff}'.toU32()} \
         ${'\\u{100}'.toU32()} ${'\\u{ffff}'.toU32()} ${'\\u{10000}'.toU32()} \
         ${'\\u{10FFFF}'.toU32()} ${'\u{1F600}'.toU32()} ${'x' == 'x'} ${'A'}\").ignore();\n    \
         .Ok(())\n}\n");
    scratch.run(&["build", "//app"]).ok();
    scratch
        .exec_js("app")
        .ok()
        .says("0 65 255 256 65535 65536 1114111 128512 true A");
}
