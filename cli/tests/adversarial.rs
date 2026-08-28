//! Hostile input, and the promise that none of it crashes the toolchain.
//!
//! The rule the lint set in `Cargo.toml` pins is that **no input may panic the
//! compiler**. A malformed source file, build file, schema, flag, or
//! language-server message must produce a diagnostic and a clean exit — never a
//! Rust panic, never a stack overflow, never a process killed by a signal.
//!
//! Every case here crashed the toolchain when it was written, or is one step
//! away from a case that did. They go through the *binary*, not the library,
//! because the binary is what a user runs and because the stack the toolchain
//! runs on is the binary's (`STACK` in `main.rs`) rather than a test thread's.
//!
//! The assertion is deliberately weak about *what* the toolchain says and
//! strong about *how* it stops: an exit status it chose, no `panicked at`, no
//! stack overflow. What a particular diagnostic reads like belongs in the
//! reject corpus, where it can be checked exactly.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::string_slice,
    clippy::arithmetic_side_effects,
    clippy::print_stdout,
    clippy::print_stderr,
    reason = "test code. The lint set in `Cargo.toml` pins a promise about the \
              toolchain — that no input panics it — and a harness that drives \
              the toolchain is not the toolchain. A test that unwraps fails on \
              the line that broke, which is what a test is for, and threading \
              `?` through an assertion buys nothing. `clippy.toml` exempts \
              `#[test]` functions already; this covers the helpers around them."
)]
mod harness;
use harness::*;

/// Asserts the toolchain stopped the way a program stops, rather than the way
/// one dies.
///
/// `Run::code` is `-1` when the process was killed by a signal, which is what a
/// stack overflow and an allocation failure look like from outside.
fn survived(run: &Run, what: &str) {
    let all = run.all();
    assert!(
        run.code >= 0,
        "{what}: the toolchain was killed by a signal rather than exiting:\n{}",
        indent(&all)
    );
    assert!(
        !all.contains("panicked at"),
        "{what}: the toolchain panicked:\n{}",
        indent(&all)
    );
    assert!(
        !all.contains("overflowed its stack") && !all.contains("stack overflow"),
        "{what}: the toolchain overflowed its stack:\n{}",
        indent(&all)
    );
    assert!(
        !all.contains("internal compiler error"),
        "{what}: an invariant the toolchain claims cannot be broken by input was broken by \
         input, which makes it a bug in the claim:\n{}",
        indent(&all)
    );
}

/// A repository with one JavaScript binary at `//app`, whose `main.buri` the
/// caller is about to replace with something hostile.
fn app(name: &str) -> Scratch {
    let s = Scratch::repo(name);
    s.binary_package("app", "export fn main(): Result<(), Str> {\n  .Ok(())\n}\n");
    s
}

// ---------------------------------------------------------------------------
// Source text
// ---------------------------------------------------------------------------

/// The lexer read the *byte* after a character it did not recognise, and every
/// character outside ASCII is more than one byte — so `let x = 5 × 3;`, or a
/// non-breaking space a word processor left behind, panicked with "byte index
/// is not a char boundary". A stray `×` is not exotic input.
#[test]
fn a_character_outside_ascii_is_a_diagnostic_rather_than_a_panic() {
    let s = app("adversarial-non-ascii");
    for (what, text) in [
        ("a multiplication sign", "export fn main(): Result<(), Str> {\n  let x = 5 × 3;\n  .Ok(())\n}\n"),
        ("an emoji", "export fn main(): Result<(), Str> {\n  🙂\n  .Ok(())\n}\n"),
        ("a non-breaking space", "export fn main(): Result<(), Str> {\n\u{a0} .Ok(())\n}\n"),
        ("a NUL byte", "export fn main(): Result<(), Str> {\n\u{0}  .Ok(())\n}\n"),
        ("a right-to-left mark", "export fn main(): Result<(), Str> {\n  \u{200f}.Ok(())\n}\n"),
    ] {
        s.write("app/main.buri", text);
        let run = s.run(&["build", "//app"]);
        survived(&run, what);
        run.exits(1).says("unexpected character");
    }
}

/// The code point is spelled out because the two characters most likely to
/// reach that diagnostic — a non-breaking space and a zero-width space — are
/// invisible, and "unexpected character ` `" is not a message anyone can act
/// on.
#[test]
fn an_invisible_character_is_named_by_its_code_point() {
    let s = app("adversarial-invisible");
    s.write("app/main.buri", "export fn main(): Result<(), Str> {\n\u{a0} .Ok(())\n}\n");
    s.run(&["build", "//app"]).exits(1).says("U+00A0");
}

/// A file that is not UTF-8 at all is refused by name, rather than by
/// `read_to_string` unwrapping somewhere.
#[test]
fn a_source_file_that_is_not_utf8_is_refused() {
    let s = app("adversarial-not-utf8");
    std::fs::write(s.path("app/main.buri"), b"export fn main(): Result<(), Str> {\n\xff\xfe\n}\n")
        .unwrap();
    let run = s.run(&["build", "//app"]);
    survived(&run, "invalid UTF-8 in a source file");
    run.exits(1).says("app/main.buri");
}

#[test]
fn an_empty_source_file_is_a_diagnostic() {
    let s = app("adversarial-empty");
    s.write("app/main.buri", "");
    let run = s.run(&["build", "//app"]);
    survived(&run, "a zero-length source file");
    run.exits(1);
}

/// Every way of leaving a construct open at the end of the file, in one file,
/// because each one is a place where a scanner runs off the end.
#[test]
fn everything_left_unterminated_is_a_diagnostic() {
    let s = app("adversarial-unterminated");
    for (what, text) in [
        ("a string", "export fn main(): Result<(), Str> {\n  let s = \"abc\n"),
        ("a character", "export fn main(): Result<(), Str> {\n  let c = 'a\n"),
        ("a block comment", "export fn main(): Result<(), Str> {\n  /* open\n"),
        ("a template hole", "export fn main(): Result<(), Str> {\n  let s = \"a${\n"),
        ("a unicode escape", "export fn main(): Result<(), Str> {\n  let s = \"\\u{1F60\n"),
        ("a block", "export fn main(): Result<(), Str> {\n  let x = 1;\n"),
    ] {
        s.write("app/main.buri", text);
        let run = s.run(&["build", "//app"]);
        survived(&run, what);
        run.exits(1);
    }
}

// ---------------------------------------------------------------------------
// Depth: the shapes that are a tree deeper than they look
// ---------------------------------------------------------------------------

/// The parser bounds how deep the grammar may nest a production inside itself,
/// because every stage after it walks the tree by recursion. Ten thousand of
/// anything is a diagnostic rather than a stack overflow.
#[test]
fn nesting_deeper_than_the_limit_is_a_diagnostic() {
    let s = app("adversarial-nesting");
    let n = 10_000;
    for (what, text) in [
        ("parentheses", format!("export fn main(): Result<(), Str> {{\n  let x = {}1{};\n  .Ok(())\n}}\n", "(".repeat(n), ")".repeat(n))),
        ("array literals", format!("export fn main(): Result<(), Str> {{\n  let x = {}1{};\n  .Ok(())\n}}\n", "[".repeat(n), "]".repeat(n))),
        ("blocks", format!("export fn main(): Result<(), Str> {{\n  let x = {}1{};\n  .Ok(())\n}}\n", "{".repeat(n), "}".repeat(n))),
        ("type arguments", format!("export fn main(): Result<(), Str> {{\n  let x: {}I32{} = list.empty();\n  .Ok(())\n}}\n", "List<".repeat(n), ">".repeat(n))),
        ("template holes", format!("export fn main(): Result<(), Str> {{\n  let x = {}1{};\n  .Ok(())\n}}\n", "\"a${".repeat(n), "}b\"".repeat(n))),
        ("array patterns", format!("export fn main(): Result<(), Str> {{\n  let x = match ([1]) {{ {}a{} => 1, _ => 0 }};\n  .Ok(())\n}}\n", "[".repeat(n), "]".repeat(n))),
        ("lambdas", format!("export fn main(): Result<(), Str> {{\n  let x = {}1;\n  .Ok(())\n}}\n", "fn(a) => ".repeat(n))),
    ] {
        s.write("app/main.buri", &text);
        let run = s.run(&["build", "//app"]);
        survived(&run, what);
        run.exits(1).says("nests too deeply");
    }
}

/// A `<` in expression position is either type arguments or a comparison, and
/// telling them apart means looking ahead for the `>` that would close the
/// list. The look is bounded (`MAX_TYPE_ARG_LOOKAHEAD`), which is what keeps a
/// file that is nothing but `<` linear rather than quadratic — and what makes
/// nesting past the bound a comparison, and so a diagnostic, rather than a
/// type-argument list deeper than the parser's stack.
#[test]
fn type_arguments_in_an_expression_do_not_look_ahead_forever() {
    let s = app("adversarial-type-args");
    for (what, text) in [
        // No `>` anywhere: every one of these is a comparison, and the scan
        // for a `>` must not walk to the end of the file at each `<`.
        (
            "unclosed type arguments",
            format!(
                "export fn main(): Result<(), Str> {{\n  let x = {}b;\n  .Ok(())\n}}\n",
                "a<".repeat(20_000)
            ),
        ),
        // Closed, and far deeper than the look: read as a comparison, which
        // is a chain, which is a sentence rather than a stack overflow.
        (
            "type arguments nested past the look",
            format!(
                "export fn main(): Result<(), Str> {{\n  let x = {}b{}(c);\n  .Ok(())\n}}\n",
                "a<".repeat(5_000),
                ">".repeat(5_000)
            ),
        ),
    ] {
        s.write("app/main.buri", &text);
        let run = s.run(&["build", "//app"]);
        survived(&run, what);
        run.exits(1);
    }
}

/// A prefix operator's operand is another unary expression, which is the one
/// place the grammar recursed without spending the nesting budget: a hundred
/// thousand `!`s overflowed the stack while parsing.
#[test]
fn a_prefix_operator_chain_is_bounded() {
    let s = app("adversarial-prefix");
    for op in ["!", "-", "~"] {
        let text = format!(
            "export fn main(): Result<(), Str> {{\n  let x = {}1;\n  .Ok(())\n}}\n",
            op.repeat(100_000)
        );
        s.write("app/main.buri", &text);
        let run = s.run(&["build", "//app"]);
        survived(&run, &format!("a chain of `{op}`"));
        run.exits(1);
    }
}

/// `a + b + c`, `x.f().g()` and `if … else if …` are flat to a reader and one
/// node deep per link in the tree. The parser builds them with a loop, so its
/// own stack was never the problem; the stack of every pass that walks what it
/// built was. A hundred thousand links is a diagnostic.
#[test]
fn a_chain_longer_than_the_limit_is_a_diagnostic() {
    let s = app("adversarial-chains");
    let n = 100_000;
    for (what, text) in [
        ("binary operators", format!("export fn main(): Result<(), Str> {{\n  let x = 1{};\n  .Ok(())\n}}\n", " + 1".repeat(n))),
        ("logical operators", format!("export fn main(): Result<(), Str> {{\n  let x = true{};\n  .Ok(())\n}}\n", " && true".repeat(n))),
        ("method calls", format!("export fn main(): Result<(), Str> {{\n  let x = 1{};\n  .Ok(())\n}}\n", ".abs()".repeat(n))),
        ("null coalescing", format!("export fn main(): Result<(), Str> {{\n  let x = 1{};\n  .Ok(())\n}}\n", " ?? 1".repeat(n))),
    ] {
        s.write("app/main.buri", &text);
        let run = s.run(&["build", "//app"]);
        survived(&run, what);
        run.exits(1).says("this chain is too long");
    }
}

/// An `else if` chain is the shape a *generated* decoder has — one per protobuf
/// field — so the budget it spends has to be large enough for a schema nobody
/// would call pathological. A thousand of them compiles; a hundred thousand is
/// a diagnostic. Both of those used to be a stack overflow.
#[test]
fn an_else_if_chain_compiles_at_the_size_generated_code_reaches() {
    let s = app("adversarial-else-if");
    let ok: String = (0..1_000).map(|i| format!("if (x == {i}) {{ {i} }} else ")).collect();
    s.write(
        "app/main.buri",
        &format!(
            "fn pick(x: I32): I32 {{\n  {ok}{{ -1 }}\n}}\n\
             export fn main(): Result<(), Str> {{\n  let _ = pick(1);\n  .Ok(())\n}}\n"
        ),
    );
    let run = s.run(&["build", "//app"]);
    survived(&run, "a thousand `else if`s");
    run.ok();
}

/// A protobuf message with a field per line generates one `else if` per field,
/// so a large schema is a deep tree in a file nobody wrote. Five hundred fields
/// overflowed the stack; two thousand now build.
#[test]
fn a_protobuf_schema_with_many_fields_builds() {
    let s = Scratch::repo("adversarial-proto");
    s.write("lib/big/BUILD.buri", "library {\n  proto_sources: [\"big.proto\"]\n}\n");
    s.write("lib/big/lib.buri", "from \"//lib/big/big.proto\" export { M };\n");
    let fields: String =
        (0..2_000).map(|i| format!("  int32 f{i} = {};\n", i + 1)).collect();
    s.write(
        "lib/big/big.proto",
        &format!("edition = \"2026\";\n\npackage big.v1;\n\nmessage M {{\n{fields}}}\n"),
    );
    let run = s.run(&["build", "//lib/big"]);
    survived(&run, "a schema with two thousand fields");
    run.ok();
}

// ---------------------------------------------------------------------------
// Volume
// ---------------------------------------------------------------------------

/// A diagnostic used to print the whole line it points at. A generated or
/// minified file is *one* line, so forty thousand errors in a two-megabyte line
/// wrote seventy gigabytes to the terminal — a failure that takes the machine
/// with it rather than only the build. A long line is now a window around the
/// caret.
#[test]
fn diagnostics_in_one_enormous_line_do_not_fill_the_disk() {
    let s = app("adversarial-one-line");
    let chain: String = (0..20_000).map(|i| format!("if (x == {i}) {{ {i} }} else ")).collect();
    s.write(
        "app/main.buri",
        &format!(
            "fn pick(x: I32): I32 {{ {chain}{{ -1 }} }}\n\
             export fn main(): Result<(), Str> {{\n  let _ = pick(1);\n  .Ok(())\n}}\n"
        ),
    );
    let run = s.run(&["build", "//app"]);
    survived(&run, "many errors in one very long line");
    let printed = run.all().len();
    // The source is about half a megabyte on one line. Printing it once per
    // diagnostic would be gigabytes; a window is a few hundred bytes each.
    assert!(
        printed < 64 * 1024 * 1024,
        "the diagnostics for one long line came to {printed} bytes"
    );
}

/// The lexer and the parser both hold literals whose text is unbounded, and the
/// integer paths parse into fixed-width types.
#[test]
fn absurd_literals_are_diagnostics() {
    let s = app("adversarial-literals");
    for (what, value) in [
        ("four hundred digits", "9".repeat(400)),
        ("a hexadecimal of the same size", format!("0x{}", "f".repeat(400))),
        ("a binary of the same size", format!("0b{}", "1".repeat(4000))),
        ("an exponent that overflows", "1e999999".to_string()),
        ("a huge tuple index", format!("(1, 2).{}", "9".repeat(30))),
    ] {
        s.write(
            "app/main.buri",
            &format!("export fn main(): Result<(), Str> {{\n  let x = {value};\n  .Ok(())\n}}\n"),
        );
        let run = s.run(&["build", "//app"]);
        survived(&run, what);
    }
}

// ---------------------------------------------------------------------------
// Build files and schemas
// ---------------------------------------------------------------------------

#[test]
fn a_hostile_build_file_is_a_diagnostic() {
    let s = app("adversarial-build-file");
    for (what, text) in [
        ("nested braces", format!("{}{}", "binary {".repeat(10_000), "}".repeat(10_000))),
        ("nested lists", format!("binary {{ outputs: {}{} }}", "[".repeat(10_000), "]".repeat(10_000))),
        ("an unterminated string", "binary {\n  name: \"open\n".to_string()),
        ("a number nothing holds", "binary {\n  timeout_seconds: 999999999999999999999999\n}\n".to_string()),
        ("a character outside ASCII", "binary {\n  × \n}\n".to_string()),
        ("nothing at all", String::new()),
    ] {
        s.write("app/BUILD.buri", &text);
        let run = s.run(&["build", "//app"]);
        survived(&run, what);
    }
}

#[test]
fn a_build_file_that_is_a_directory_is_a_diagnostic() {
    let s = app("adversarial-build-dir");
    std::fs::remove_file(s.path("app/BUILD.buri")).unwrap();
    std::fs::create_dir(s.path("app/BUILD.buri")).unwrap();
    let run = s.run(&["build", "//app"]);
    survived(&run, "a BUILD.buri that is a directory");
    assert!(run.code != 0, "a directory named BUILD.buri built successfully");
}

#[test]
fn a_repo_file_that_is_a_directory_is_a_diagnostic() {
    let s = app("adversarial-repo-dir");
    std::fs::remove_file(s.path("REPO.buri")).unwrap();
    std::fs::create_dir(s.path("REPO.buri")).unwrap();
    let run = s.run(&["build", "//app"]);
    survived(&run, "a REPO.buri that is a directory");
    assert!(run.code != 0, "a directory named REPO.buri built successfully");
}

#[test]
fn a_source_file_that_is_a_directory_is_a_diagnostic() {
    let s = app("adversarial-source-dir");
    std::fs::remove_file(s.path("app/main.buri")).unwrap();
    std::fs::create_dir(s.path("app/main.buri")).unwrap();
    let run = s.run(&["build", "//app"]);
    survived(&run, "a main.buri that is a directory");
    assert!(run.code != 0, "a directory named main.buri built successfully");
}

#[test]
fn a_hostile_schema_is_a_diagnostic() {
    let s = Scratch::repo("adversarial-schema");
    s.write("lib/bad/BUILD.buri", "library {\n  proto_sources: [\"bad.proto\"]\n}\n");
    s.write("lib/bad/lib.buri", "from \"//lib/bad/bad.proto\" export { M };\n");
    for (what, text) in [
        ("nested messages", format!("edition = \"2026\";\npackage p;\n{}{}", "message M {".repeat(10_000), "}".repeat(10_000))),
        ("a field number nothing holds", "edition = \"2026\";\npackage p;\nmessage M { int32 a = 999999999999999999999; }\n".to_string()),
        ("a negative field number", "edition = \"2026\";\npackage p;\nmessage M { int32 a = -1; }\n".to_string()),
        ("an escape of a character outside ASCII", "edition = \"2026\";\npackage p;\nmessage M { string s = 1 [default = \"\\é\"]; }\n".to_string()),
        ("a message that contains itself", "edition = \"2026\";\npackage p;\nmessage M { M m = 1; }\n".to_string()),
        ("a file that imports itself", "edition = \"2026\";\nimport \"lib/bad/bad.proto\";\npackage p;\nmessage M { int32 a = 1; }\n".to_string()),
        ("nothing at all", String::new()),
    ] {
        s.write("lib/bad/bad.proto", &text);
        let run = s.run(&["build", "//lib/bad"]);
        survived(&run, what);
    }
}

// ---------------------------------------------------------------------------
// The command line
// ---------------------------------------------------------------------------

/// Argument handling is the first thing a user's bytes reach, and the one place
/// where "there is no next argument" is a thing to check rather than to assume.
#[test]
fn a_hostile_command_line_is_a_usage_error() {
    let s = app("adversarial-argv");
    let long = "/".repeat(100_000);
    let cases: Vec<Vec<&str>> = vec![
        vec![""],
        vec!["build", ""],
        vec!["build", "//"],
        vec!["build", "///"],
        vec!["build", "...."],
        vec!["build", "//..."],
        vec!["build", &long],
        vec!["build", "--error-format"],
        vec!["build", "--error-format="],
        vec!["build", "--error-format=nonsense"],
        vec!["build", "--"],
        vec!["build", "--nope"],
        vec!["build", "-"],
        vec!["query"],
        vec!["query", "deps"],
        vec!["query", "deps", ""],
        vec!["query", "path"],
        vec!["query", "path", "//app"],
        vec!["docs", ""],
        vec!["docs", "error"],
        vec!["docs", "error", ""],
        vec!["run"],
        vec!["test", "--filter"],
        vec!["gen", ""],
        vec!["lint", "--fix", ""],
        vec!["format", ""],
    ];
    for argv in cases {
        let run = s.run(&argv);
        survived(&run, &format!("`buri {}`", argv.join(" ")));
    }
}

// ---------------------------------------------------------------------------
// The language server
// ---------------------------------------------------------------------------

/// The framing read the declared length into a buffer it allocated first, so
/// `Content-Length: 18446744073709551615` aborted the process on the allocation
/// before a byte of the body arrived. A header is a number someone else wrote.
#[test]
fn an_absurd_content_length_is_refused_rather_than_allocated() {
    let s = app("adversarial-lsp-framing");
    for header in [
        "18446744073709551615",
        "999999999999999999999999",
        "1099511627776",
        "-1",
        "abc",
        "",
    ] {
        let message = format!("Content-Length: {header}\r\n\r\n{{}}");
        let run = s.run_with_stdin(&["lsp"], message.as_bytes());
        survived(&run, &format!("Content-Length: {header}"));
    }
}

/// A position is two numbers a client sent. Past the end of the file, past the
/// end of the line, inside a multi-byte character, and the largest number the
/// protocol can spell are all things an editor can ask about.
#[test]
fn a_position_the_client_made_up_is_answered_rather_than_indexed() {
    let s = app("adversarial-lsp-positions");
    let root = s.root.display().to_string();
    let uri = format!("file://{root}/app/main.buri");
    // `café 🙂` puts a two-byte and a four-byte character on line 1.
    let text = "export fn main(): Result<(), Str> {\\n  let s = \\\"café 🙂\\\";\\n  .Ok(())\\n}\\n";
    let mut session = String::new();
    let mut push = |line: &str| {
        session.push_str(&format!("Content-Length: {}\r\n\r\n{line}", line.len()));
    };
    push(&format!(
        r#"{{"jsonrpc":"2.0","id":1,"method":"initialize","params":{{"rootUri":"file://{root}"}}}}"#
    ));
    push(r#"{"jsonrpc":"2.0","method":"initialized","params":{}}"#);
    push(&format!(
        r#"{{"jsonrpc":"2.0","method":"textDocument/didOpen","params":{{"textDocument":{{"uri":"{uri}","languageId":"buri","version":1,"text":"{text}"}}}}}}"#
    ));
    for (id, method, line, character) in [
        (2, "textDocument/hover", "4294967295", "4294967295"),
        (3, "textDocument/hover", "1", "13"),
        (4, "textDocument/hover", "-5", "-9"),
        (5, "textDocument/definition", "999999", "0"),
        (6, "textDocument/completion", "1", "4294967295"),
        (7, "textDocument/hover", "0", "0"),
        // Every other request that takes a position, on the character that
        // splits into a surrogate pair and on a line the file does not have.
        (8, "textDocument/typeDefinition", "1", "4294967295"),
        (9, "textDocument/declaration", "1", "13"),
        (10, "textDocument/documentHighlight", "1", "13"),
        (11, "textDocument/references", "999999", "999999"),
        (12, "textDocument/signatureHelp", "1", "13"),
        (13, "textDocument/signatureHelp", "4294967295", "0"),
        (14, "textDocument/prepareRename", "1", "13"),
    ] {
        push(&format!(
            r#"{{"jsonrpc":"2.0","id":{id},"method":"{method}","params":{{"textDocument":{{"uri":"{uri}"}},"position":{{"line":{line},"character":{character}}}}}}}"#
        ));
    }
    // A document nobody opened, a URI that is not one, and params of the wrong
    // shape entirely.
    push(r#"{"jsonrpc":"2.0","id":20,"method":"textDocument/hover","params":{"textDocument":{"uri":"file:///nowhere/x.buri"},"position":{"line":0,"character":0}}}"#);
    push(r#"{"jsonrpc":"2.0","id":21,"method":"textDocument/hover","params":{"textDocument":{"uri":"file://%zz%"},"position":{"line":0,"character":0}}}"#);
    push(r#"{"jsonrpc":"2.0","id":22,"method":"textDocument/hover","params":{}}"#);
    push(r#"{"jsonrpc":"2.0","id":23,"method":"textDocument/hover"}"#);
    push(r#"{"jsonrpc":"2.0","id":24,"method":"textDocument/hover","params":{"textDocument":{"uri":42},"position":"nope"}}"#);
    push(r#"{"jsonrpc":"2.0","id":25,"method":"nonsense/method","params":{}}"#);
    // The requests whose params are not only a position: a rename with no new
    // name and one with a new name that is not a name, a selection range whose
    // positions are not positions, and a query that is not a string.
    push(&format!(
        r#"{{"jsonrpc":"2.0","id":30,"method":"textDocument/rename","params":{{"textDocument":{{"uri":"{uri}"}},"position":{{"line":0,"character":10}}}}}}"#
    ));
    push(&format!(
        r#"{{"jsonrpc":"2.0","id":31,"method":"textDocument/rename","params":{{"textDocument":{{"uri":"{uri}"}},"position":{{"line":0,"character":10}},"newName":"  "}}}}"#
    ));
    push(&format!(
        r#"{{"jsonrpc":"2.0","id":32,"method":"textDocument/selectionRange","params":{{"textDocument":{{"uri":"{uri}"}},"positions":[1,"x",{{}}]}}}}"#
    ));
    push(&format!(
        r#"{{"jsonrpc":"2.0","id":33,"method":"textDocument/foldingRange","params":{{"textDocument":{{"uri":"{uri}"}}}}}}"#
    ));
    push(r#"{"jsonrpc":"2.0","id":34,"method":"workspace/symbol","params":{"query":7}}"#);
    push(r#"{"jsonrpc":"2.0","id":35,"method":"workspace/symbol","params":{}}"#);
    push(r#"{"jsonrpc":"2.0","id":26,"method":"shutdown"}"#);
    push(r#"{"jsonrpc":"2.0","method":"exit"}"#);

    let run = s.run_with_stdin(&["lsp"], session.as_bytes());
    survived(&run, "a session of made-up positions");
    run.exits(0);
}

/// The reader is recursive and the text arrives on a socket, so nesting is
/// stack: `[[[[[…` a hundred thousand deep is four bytes of typing.
#[test]
fn a_deeply_nested_message_is_refused_rather_than_recursed() {
    let s = app("adversarial-lsp-nesting");
    for body in [
        format!("{}1{}", "[".repeat(200_000), "]".repeat(200_000)),
        format!("{}1{}", "{\"a\":".repeat(200_000), "}".repeat(200_000)),
        "[".repeat(200_000),
    ] {
        let message = format!("Content-Length: {}\r\n\r\n{body}", body.len());
        let run = s.run_with_stdin(&["lsp"], message.as_bytes());
        survived(&run, "a deeply nested message");
    }
}

/// Bytes that are not a message at all, in the shapes a broken client sends.
#[test]
fn a_malformed_message_does_not_stop_the_server_badly() {
    let s = app("adversarial-lsp-garbage");
    for bytes in [
        b"not a header at all\r\n\r\n{}".to_vec(),
        b"Content-Length: 2\r\n\r\n\xff\xfe".to_vec(),
        b"Content-Length: 5\r\n\r\n{".to_vec(),
        b"\r\n\r\n".to_vec(),
        Vec::new(),
    ] {
        let run = s.run_with_stdin(&["lsp"], &bytes);
        survived(&run, "malformed protocol bytes");
    }
}
