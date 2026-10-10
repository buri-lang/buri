//! What the lexer reads at the edges its fast paths stop at: a word against
//! its first eight bytes and the end of the file, an integer against the
//! nineteen digits a `u64` holds, and a comment against the line's end.
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
    for word in ["abcdefgh", "abcdefghi", "lets", "selfish", "Selfie", "fnord", "unreachables"] {
        let scratch = app(&format!(
            "from \"node\" import {{ NodeHost }};\n\n\
             export fn main(host: NodeHost): Result<(), Str> {{\n    .Ok(())\n}}\n{word}"
        ));
        let run = scratch.run(&["build", "//app"]);
        run.exits(1);
        run.says(&format!("found `{word}`")).says("app/main.buri:6:1");
    }
    // A reserved word longer than eight bytes, last in the file.
    let scratch = app(
        "from \"node\" import { NodeHost };\n\n\
         export fn main(host: NodeHost): Result<(), Str> {\n    .Ok(())\n}\nunreachable",
    );
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
