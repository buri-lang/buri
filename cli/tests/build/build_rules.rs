//! The build graph's rarer shapes: where a rule may live, the module paths
//! nothing resolves, the directories the package walk skips, a diamond in the
//! dependency graph, outputs that collide, and what `buri gen` does with a
//! package it can't or needn't rewrite.
use crate::harness::*;

const PLATFORM: &str = "platform {\n    entry {\n        name: \"e\"\n        backend: JS\n    }\n}\n";

/// `platform/effect/` holds effect packages, so a `platform` rule there is
/// misplaced, and the fix names where it belongs: `//platform/<its name>`, or
/// a placeholder for the package that is `platform/effect` itself.
#[test]
fn a_platform_rule_inside_platform_effect_is_misplaced() {
    let scratch = Scratch::repo("platform-in-effect");
    scratch.write("platform/effect/BUILD.buri", PLATFORM);
    scratch.write("platform/effect/p/BUILD.buri", PLATFORM);
    scratch
        .run(&["build", "//..."])
        .exits(2)
        .says("//platform/effect declares a `platform` rule inside //platform/effect/, which holds effect packages")
        .says("fix: move this package to //platform/<name>")
        .says("//platform/effect/p declares a `platform` rule inside //platform/effect/")
        .says("fix: move this package to //platform/p,");
}

/// Each import path that names no module gets its own reason, and the
/// directories that hold build output or other tools' packages are never
/// walked for packages, however broken the build files in them.
#[test]
fn import_paths_that_name_no_module_say_why() {
    let scratch = Scratch::repo("unresolved-paths");
    scratch.write("lib/a/BUILD.buri", "library {\n    visibility: [\"//...\"]\n}\n");
    scratch.write("lib/a/lib.buri", "export fn one(): Int {\n    1\n}\n");
    scratch.write("lib/c/BUILD.buri", "library {\n}\n");
    scratch.write(
        "lib/c/lib.buri",
        "from \"core\" import { x };\nfrom \"ui\" import { y };\nfrom \"///lib/a\" import { z };\n\
         from \"//lib/a/platform.buri\" import { w };\nfrom \"//lib/a/s.proto\" import { v };\n",
    );
    scratch.write("target/x/BUILD.buri", "not a build file\n");
    scratch.write("node_modules/y/BUILD.buri", "not a build file\n");
    scratch
        .run(&["build", "//lib/c"])
        .exits(1)
        .says("\"core\" is not a module; name one, as in \"core/list\"")
        .says("\"ui\" is not a module; name one, as in \"ui/signal\"")
        .says("\"///lib/a\" is in no package of this repository")
        .says("\"//lib/a/platform.buri\" names no file (lib/a/platform.buri)")
        .says("\"//lib/a/s.proto\" names no file (lib/a/s.proto)")
        .silent_about("target/x")
        .silent_about("node_modules/y");
}

/// Two routes from `//lib/a` to `//lib/d`, through `b` and through `c`:
/// `path` answers the first, each step with the line that declared it.
#[test]
fn a_path_through_a_diamond_takes_the_first_route() {
    let scratch = Scratch::repo("diamond");
    scratch.write("lib/a/BUILD.buri", "library {\n    dependencies: [\"//lib/b\", \"//lib/c\"]\n}\n");
    scratch.write("lib/b/BUILD.buri", "library {\n    dependencies: [\"//lib/d\"]\n}\n");
    scratch.write("lib/c/BUILD.buri", "library {\n    dependencies: [\"//lib/d\"]\n}\n");
    scratch.write("lib/d/BUILD.buri", "library {\n    visibility: [\"//...\"]\n}\n");
    for p in ["a", "b", "c", "d"] {
        scratch.write(&format!("lib/{p}/lib.buri"), "export fn f(): Int {\n    1\n}\n");
    }
    let run = scratch.run(&["query", "path(//lib/a, //lib/d)"]);
    run.ok();
    assert_eq!(
        run.stdout,
        "//lib/a\n  -> //lib/b                lib/a/BUILD.buri:2\n  -> //lib/d                lib/b/BUILD.buri:2\n"
    );
}

/// Three outputs writing one file, and an output naming a label that is no
/// platform: each is refused, and the build does nothing.
#[test]
fn outputs_that_collide_or_name_no_platform_are_refused() {
    let scratch = Scratch::repo("colliding-outputs");
    scratch.write("lib/a/BUILD.buri", "library {\n    visibility: [\"//...\"]\n}\n");
    scratch.write("lib/a/lib.buri", "export fn f(): Int {\n    1\n}\n");
    scratch.write(
        "cmd/x/BUILD.buri",
        "binary {\n    outputs: [\n        { platform: \"node\" },\n        { platform: \"node\" },\n        \
         { platform: \"node\" },\n        { platform: \"//lib/a\" },\n    ]\n    test {\n        \
         dependencies: [\"//cmd/x\"]\n    }\n}\n",
    );
    scratch.write("cmd/x/main.buri", "export fn main() {}\n");
    scratch
        .run(&["build", "//cmd/x"])
        .exits(2)
        .says("two artifacts of //cmd/x write `.buri/out/node/cmd/x/x.mjs` [duplicate-artifact-path]")
        .says("`//lib/a` is not a platform [unknown-platform]");
}

/// `buri gen` keeps a `test {}` block someone wrote, even with no suite in
/// it yet, and lists a library's `testing` sources even when the library
/// doesn't compile, so it can't say what they depend on.
#[test]
fn gen_keeps_an_empty_test_block_and_a_broken_library_s_testing_sources() {
    let scratch = Scratch::repo("gen-edges");
    scratch.write("lib/c/BUILD.buri", "library {\n    testing {\n    }\n}\n");
    scratch.write("lib/c/lib.buri", "export fn two(): Int {\n    nope\n}\n");
    scratch.write("lib/c/testing/lib.buri", "export fn fake(): Int {\n    2\n}\n");
    scratch.write("lib/c/testing/more.buri", "export fn more(): Int {\n    3\n}\n");
    scratch.write("cmd/d/BUILD.buri", "binary {\n    test {\n    }\n}\n");
    scratch.write("cmd/d/main.buri", "export fn main() {}\n");
    scratch.run(&["gen", "//lib/c", "//cmd/d"]).ok().says("+ testing.sources: testing/more.buri");
    assert_eq!(scratch.read("cmd/d/BUILD.buri"), "binary {\n    test {}\n}\n");
    assert!(scratch.read("lib/c/BUILD.buri").contains("sources: [\"testing/more.buri\"]"));
}

/// A language's `check` must name a tool. A binary isn't one, and only a
/// generator gets the sentence about the difference.
#[test]
fn a_language_checked_by_a_binary_names_no_tool() {
    let scratch = Scratch::empty("language-binary");
    scratch.write(
        "REPO.buri",
        "language {\n    name: \"lines\"\n    extensions: [\".lines\"]\n    check: \"//cmd/b\"\n}\n",
    );
    scratch.write("cmd/b/BUILD.buri", "binary {\n}\n");
    scratch.write("cmd/b/main.buri", "export fn main() {}\n");
    scratch.run(&["build", "//cmd/b"]).exits(2).says("`//cmd/b` names no tool [unknown-tool]");
}

/// Frames one JSON-RPC message per line, `<scratch>` being the repository.
fn session(scratch: &Scratch, lines: &[&str]) -> Vec<u8> {
    let mut out = Vec::new();
    for line in lines {
        let line = line.replace("<scratch>", &scratch.root.display().to_string());
        out.extend_from_slice(format!("Content-Length: {}\r\n\r\n{line}", line.len()).as_bytes());
    }
    out
}

const INITIALIZE: &str = r#"{"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"rootUri": "file://<scratch>"}}"#;
const INITIALIZED: &str = r#"{"jsonrpc": "2.0", "method": "initialized", "params": {}}"#;
const SHUTDOWN: &str = r#"{"jsonrpc": "2.0", "id": 9, "method": "shutdown"}"#;
const EXIT: &str = r#"{"jsonrpc": "2.0", "method": "exit"}"#;

/// `buri gen` leaves a tool's package as written, so the command says its
/// build file is already what `gen` would write.
#[test]
fn regenerating_a_tool_s_build_file_changes_nothing() {
    let scratch = Scratch::repo("regenerate-tool");
    scratch.write("tools/t/BUILD.buri", "tool {\n}\n");
    scratch.write("tools/t/tool.buri", "export fn main() {}\n");
    let run = scratch.run_with_stdin(
        &["lsp"],
        &session(
            &scratch,
            &[
                INITIALIZE,
                INITIALIZED,
                r#"{"jsonrpc": "2.0", "id": 2, "method": "workspace/executeCommand", "params": {"command": "buri.regenerateBuildFile", "arguments": ["file://<scratch>/tools/t/tool.buri", "//tools/t"]}}"#,
                SHUTDOWN,
                EXIT,
            ],
        ),
    );
    run.ok().says("//tools/t: BUILD.buri is already what buri gen would write").silent_about("workspace/applyEdit");
}

/// The command regenerates the build file the editor holds, unsaved edits
/// and all, since the edit it sends replaces that buffer whole: a comment
/// typed but not saved survives it.
#[test]
fn regenerating_keeps_what_the_editor_has_not_saved() {
    let scratch = Scratch::copy_of("regenerate-unsaved", &tests_dir().join("repositories/lsp/a_command_regenerates_a_build_file/repo"));
    let run = scratch.run_with_stdin(
        &["lsp"],
        &session(
            &scratch,
            &[
                INITIALIZE,
                INITIALIZED,
                r##"{"jsonrpc": "2.0", "method": "textDocument/didOpen", "params": {"textDocument": {"uri": "file://<scratch>/cmd/app/BUILD.buri", "languageId": "buri", "version": 1, "text": "# not saved yet\nbinary {\n    outputs: [\n        { platform: \"node\" },\n    ]\n}\n"}}}"##,
                r#"{"jsonrpc": "2.0", "id": 2, "method": "workspace/executeCommand", "params": {"command": "buri.regenerateBuildFile", "arguments": ["file://<scratch>/cmd/app/main.buri", "//cmd/app"]}}"#,
                r#"{"jsonrpc": "2.0", "id": "buri/applyEdit/1", "result": {"applied": true}}"#,
                SHUTDOWN,
                EXIT,
            ],
        ),
    );
    run.ok()
        .says(r##""newText":"# not saved yet\nbinary {\n    dependencies: [\"//lib/money\"]"##)
        .says("+ dependencies: //lib/money");
}

/// An unsaved build file that doesn't parse is refused, with the reason,
/// rather than replaced by a regeneration of the copy on disk.
#[test]
fn regenerating_an_unsaved_build_file_that_does_not_parse_is_refused() {
    let scratch = Scratch::copy_of("regenerate-broken", &tests_dir().join("repositories/lsp/a_command_regenerates_a_build_file/repo"));
    let run = scratch.run_with_stdin(
        &["lsp"],
        &session(
            &scratch,
            &[
                INITIALIZE,
                INITIALIZED,
                r#"{"jsonrpc": "2.0", "method": "textDocument/didOpen", "params": {"textDocument": {"uri": "file://<scratch>/cmd/app/BUILD.buri", "languageId": "buri", "version": 1, "text": "binary {\n    outputs: [[\n        { platform: \"node\" },\n    ]\n}\n"}}}"#,
                r#"{"jsonrpc": "2.0", "id": 2, "method": "workspace/executeCommand", "params": {"command": "buri.regenerateBuildFile", "arguments": ["file://<scratch>/cmd/app/main.buri", "//cmd/app"]}}"#,
                SHUTDOWN,
                EXIT,
            ],
        ),
    );
    run.ok().says(r#""id":2,"jsonrpc":"2.0""#).says("unterminated").silent_about("workspace/applyEdit");
}
