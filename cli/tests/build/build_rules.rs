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

const NODE_MAIN: &str = "from \"node\" import { NodeHost };\n\nexport fn main(host: NodeHost): Result<(), Str> {\n    .Ok(())\n}\n";
const ONE: &str = "export fn one(): Int {\n    1\n}\n";

/// A program that loads half of itself lazily writes that half as `app.0.mjs`.
const LAZY_MAIN: &str = r#"from "core/io" import * as io;
from "core/lazy" import * as lazy;
from "node" import { NodeHost };
from "platform/effect" import { Allocator, Stdout };

fn later<C: Stdout>(ctx: C, _n: Int): () {
    io.println(ctx, "later").ignore()
}

export fn main(host: NodeHost): Result<(), Str> {
    let ctx = context {
        Allocator: host.alloc,
        Stdout: host.stdout,
    };
    let open = lazy.load(later);
    let _ = open(ctx, 1);
    .Ok(())
}
"#;

/// A chunk a build with more of them left behind is removed, and a chunk
/// that can't be written fails the build, from the cache and afresh.
#[test]
fn an_artifact_s_chunks_replace_the_last_build_s() {
    let scratch = Scratch::repo("lazy-chunks");
    scratch.write("cmd/app/BUILD.buri", JS_BINARY);
    scratch.write("cmd/app/main.buri", LAZY_MAIN);
    scratch.run(&["build", "//cmd/app"]).ok();
    assert!(scratch.path(".buri/out/node/cmd/app/app.0.mjs").is_file());
    scratch.write(".buri/out/node/cmd/app/app.1.mjs", "stale\n");
    scratch.write(".buri/out/node/cmd/app/app.2.mjs", "stale\n");
    scratch.run(&["build", "//cmd/app"]).ok().says("cached");
    assert!(!scratch.path(".buri/out/node/cmd/app/app.1.mjs").exists(), "a stale chunk stays");
    assert!(!scratch.path(".buri/out/node/cmd/app/app.2.mjs").exists(), "a stale chunk stays");

    std::fs::remove_file(scratch.path(".buri/out/node/cmd/app/app.0.mjs")).unwrap();
    std::fs::create_dir(scratch.path(".buri/out/node/cmd/app/app.0.mjs")).unwrap();
    for args in [&["build", "//cmd/app"][..], &["build", "--force", "//cmd/app"]] {
        scratch.run(args).exits(1).says("cannot write ").says("app.0.mjs");
    }
}

/// Gives a path its permissions back when dropped, so the scratch tree can go.
struct Reopen(std::path::PathBuf, u32);

impl Drop for Reopen {
    fn drop(&mut self) {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&self.0, std::fs::Permissions::from_mode(self.1));
    }
}

fn close(path: std::path::PathBuf, reopen: u32) -> Reopen {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).unwrap();
    Reopen(path, reopen)
}

/// `gen` lists a package's own sources: not a dotfile, not a nested
/// package's, nothing in a directory it can't read, and not a listed source
/// that's gone. A source it can't read is still one of the package's.
#[test]
fn gen_lists_only_the_sources_a_package_owns() {
    let scratch = Scratch::repo("gen-walk");
    scratch.write("lib/a/BUILD.buri", "library {\n    visibility: [\"//...\"]\n    sources: [\"gone.buri\"]\n}\n");
    scratch.write("lib/a/lib.buri", ONE);
    scratch.write("lib/a/extra.buri", ONE);
    scratch.write("lib/a/.hidden.buri", ONE);
    scratch.write("lib/a/nested/BUILD.buri", "library {}\n");
    scratch.write("lib/a/nested/lib.buri", ONE);
    scratch.write("lib/a/locked/x.buri", ONE);
    scratch.write("lib/a/sealed.buri", ONE);
    let _dir = close(scratch.path("lib/a/locked"), 0o755);
    let _file = close(scratch.path("lib/a/sealed.buri"), 0o644);
    scratch.run(&["gen", "//lib/a"]).ok().says("+ sources: extra.buri, sealed.buri").says("- sources: gone.buri");
    assert_eq!(
        scratch.read("lib/a/BUILD.buri"),
        "library {\n    sources: [\"extra.buri\", \"sealed.buri\"]\n    visibility: [\"//...\"]\n}\n"
    );
}

/// In a package with only a binary, every suite is the binary's, whatever
/// it imports.
#[test]
fn gen_gives_a_binary_only_package_s_suites_to_the_binary() {
    let scratch = Scratch::repo("gen-binary-suite");
    scratch.write("cmd/b/BUILD.buri", JS_BINARY);
    scratch.write("cmd/b/main.buri", NODE_MAIN);
    scratch.write("cmd/b/test/t.buri", "test \"t\" {\n}\n");
    scratch.run(&["gen", "//cmd/b"]).ok().says("+ test.sources: test/t.buri");
    assert!(scratch.read("cmd/b/BUILD.buri").contains("    test {\n        sources: [\"test/t.buri\"]\n    }\n"));
}

/// The editor's command leaves a platform's package as written, and rewrites
/// an unsaved build file whose lists hold numbers from the package's files.
#[test]
fn regenerating_a_platform_or_an_unsaved_mistyped_build_file() {
    let scratch = Scratch::repo("regenerate-shapes");
    scratch.write("platform/kit/BUILD.buri", PLATFORM);
    scratch.write("platform/kit/platform.buri", "export struct KitHost {}\nexport fn e(host: KitHost): Result<(), Str>;\n");
    scratch.write("lib/a/BUILD.buri", "library {\n    visibility: [\"//...\"]\n}\n");
    scratch.write("lib/a/lib.buri", ONE);
    scratch.write("lib/a/extra.buri", ONE);
    let command = |file: &str, label: &str| {
        format!(
            r#"{{"jsonrpc": "2.0", "id": 2, "method": "workspace/executeCommand", "params": {{"command": "buri.regenerateBuildFile", "arguments": ["file://<scratch>/{file}", "{label}"]}}}}"#
        )
    };
    let open = |text: &str| {
        let text = text.replace('"', "\\\"").replace('\n', "\\n");
        format!(
            r#"{{"jsonrpc": "2.0", "method": "textDocument/didOpen", "params": {{"textDocument": {{"uri": "file://<scratch>/lib/a/BUILD.buri", "languageId": "buri", "version": 1, "text": "{text}"}}}}}}"#
        )
    };
    const APPLIED: &str = r#"{"jsonrpc": "2.0", "id": "buri/applyEdit/1", "result": {"applied": true}}"#;
    let platform = command("platform/kit/platform.buri", "//platform/kit");
    scratch
        .run_with_stdin(&["lsp"], &session(&scratch, &[INITIALIZE, INITIALIZED, &platform, SHUTDOWN, EXIT]))
        .ok()
        .says("//platform/kit: BUILD.buri is already what buri gen would write");

    let numbers = open("library {\n    visibility: [\"//...\"]\n    sources: [1]\n    test {\n        sources: [2]\n    }\n}\n");
    let library = command("lib/a/lib.buri", "//lib/a");
    scratch
        .run_with_stdin(&["lsp"], &session(&scratch, &[INITIALIZE, INITIALIZED, &numbers, &library, APPLIED, SHUTDOWN, EXIT]))
        .ok()
        .says(r#""newText":"library {\n    sources: [\"extra.buri\"]\n    visibility: [\"//...\"]\n    test {\n        sources: []\n    }\n}\n""#);

    let not_a_block = open("library: 1\n");
    scratch
        .run_with_stdin(&["lsp"], &session(&scratch, &[INITIALIZE, INITIALIZED, &not_a_block, &library, SHUTDOWN, EXIT]))
        .ok()
        .says("//lib/a: BUILD.buri is already what buri gen would write")
        .silent_about("workspace/applyEdit");
}

/// Each way two outputs land on one path names its own cause, and a page
/// platform refuses an `artifact_name`.
#[test]
fn outputs_that_share_a_path_say_how() {
    let scratch = Scratch::repo("shared-paths");
    scratch.write(
        "platform/many/BUILD.buri",
        "platform {\n    entry {\n        name: \"one\"\n        backend: JS\n    }\n    entry {\n        name: \"two\"\n        \
         backend: JS\n    }\n    entry {\n        name: \"three\"\n        backend: JS\n    }\n}\n",
    );
    scratch.write(
        "platform/many/platform.buri",
        "export struct ManyHost {}\nexport fn one(host: ManyHost): Result<(), Str>;\n\
         export fn two(host: ManyHost): Result<(), Str>;\nexport fn three(host: ManyHost): Result<(), Str>;\n",
    );
    scratch.write(
        "platform/page/BUILD.buri",
        "platform {\n    entry {\n        name: \"main\"\n        backend: JS\n    }\n    assets: [\"index.html\"]\n}\n",
    );
    scratch.write("platform/page/index.html", "<html></html>\n");
    scratch.write("platform/page/platform.buri", "export struct PageHost {}\nexport fn main(host: PageHost): Result<(), Str>;\n");
    scratch.write("cmd/a/BUILD.buri", "binary {\n    outputs: [{ platform: \"//platform/many\", artifact_name: \"same\" }]\n}\n");
    scratch.write(
        "cmd/a/main.buri",
        "from \"//platform/many\" import { ManyHost };\nexport fn one(host: ManyHost): Result<(), Str> {\n    .Ok(())\n}\n\
         export fn two(host: ManyHost): Result<(), Str> {\n    .Ok(())\n}\nexport fn three(host: ManyHost): Result<(), Str> {\n    .Ok(())\n}\n",
    );
    scratch.write("cmd/b/BUILD.buri", "binary {\n    outputs: [{ platform: \"//platform/page\", artifact_name: \"x\" }]\n}\n");
    scratch.write(
        "cmd/b/main.buri",
        "from \"//platform/page\" import { PageHost };\nexport fn main(host: PageHost): Result<(), Str> {\n    .Ok(())\n}\n",
    );
    scratch.write("cmd/c/BUILD.buri", "binary {\n    outputs: [{ platform: \"web\" }, { platform: \"web\" }]\n}\n");
    scratch.write("cmd/c/main.buri", "from \"web\" import { WebHost };\nexport fn main(host: WebHost): Result<(), Str> {\n    .Ok(())\n}\n");
    scratch.write(
        "cmd/d/BUILD.buri",
        "binary {\n    outputs: [{ platform: \"node\", artifact_name: \"d\" }, { platform: \"node\" }, { platform: \"node\", artifact_name: \"d\" }]\n}\n",
    );
    scratch.write("cmd/d/main.buri", NODE_MAIN);
    let run = scratch.run(&["build", "//cmd/a", "//cmd/b", "//cmd/c", "//cmd/d"]);
    run.exits(2)
        .says("two artifacts of //cmd/a write `.buri/out/platform/many/cmd/a/same.mjs` [duplicate-artifact-path]")
        .says("`//platform/many` has several entries, and `artifact_name` names each of them `same`")
        .says("`//platform/page` names its own artifacts [misplaced-artifact-name]")
        .says("both outputs are `web` pages, and a page's file is named after its entry")
        .says("both outputs are `node`, in one directory, under one name");
    assert_eq!(run.all().matches("two artifacts of //cmd/a write").count(), 1, "one written output is told about once");
    assert_eq!(run.all().matches("two artifacts of //cmd/d write").count(), 2);
}

/// A tag that forbids one sorted after it conflicts as one sorted before
/// does, and a binary carrying a tag nobody declared, or beside its package's
/// library, is held to what its dependencies admit.
#[test]
fn policy_reads_tags_both_ways_and_a_binary_beside_a_library() {
    let scratch = Scratch::empty("policy-shapes");
    scratch.write(
        "REPO.buri",
        "tag {\n    name: \"alpha\"\n    doc: \"first\"\n\n    forbids {\n        tags: [\"beta\"]\n    }\n}\n\n\
         tag {\n    name: \"beta\"\n    doc: \"second\"\n}\n",
    );
    scratch.write("lib/a/BUILD.buri", "library {\n    visibility: [\"//...\"]\n    tags: [\"alpha\"]\n}\n");
    scratch.write("lib/a/lib.buri", ONE);
    scratch.write("lib/b/BUILD.buri", "library {\n    visibility: [\"//...\"]\n    tags: [\"beta\"]\n}\n");
    scratch.write("lib/b/lib.buri", ONE);
    scratch.write("lib/web/BUILD.buri", "library {\n    visibility: [\"//...\"]\n    platforms: [\"web\"]\n}\n");
    scratch.write("lib/web/lib.buri", ONE);
    scratch.write(
        "cmd/x/BUILD.buri",
        "binary {\n    dependencies: [\"//lib/a\", \"//lib/b\"]\n    outputs: [{ platform: \"node\" }]\n}\n",
    );
    scratch.write("cmd/x/main.buri", &format!("from \"//lib/a\" import {{ one }};\nfrom \"//lib/b\" import {{ one as two }};\n{NODE_MAIN}"));
    scratch.write(
        "cmd/z/BUILD.buri",
        "binary {\n    dependencies: [\"//lib/web\"]\n    outputs: [{ platform: \"node\" }]\n}\nlibrary {\n    visibility: [\"//...\"]\n}\n",
    );
    scratch.write("cmd/z/main.buri", &format!("from \"//lib/web\" import {{ one }};\n{NODE_MAIN}"));
    scratch.write("cmd/z/lib.buri", ONE);
    scratch.write(
        "cmd/y/BUILD.buri",
        "binary {\n    tags: [\"mystery\"]\n    dependencies: [\"//lib/web\"]\n    outputs: [{ platform: \"node\" }]\n}\n",
    );
    scratch.write("cmd/y/main.buri", &format!("from \"//lib/web\" import {{ one }};\n{NODE_MAIN}"));
    scratch
        .run(&["build", "//cmd/x", "//cmd/y", "//cmd/z"])
        .exits(1)
        .says("//cmd/x cannot contain both \"alpha\" and \"beta\" code [tag-conflict]")
        .says("unknown tag \"mystery\" [unknown-tag]")
        .says("//cmd/y cannot be built for node [platform-violation]")
        .says("//cmd/z cannot be built for node [platform-violation]")
        .says("//lib/web declares platforms web");
}

/// Every `dependencies` list names a library, a library's `testing` surface,
/// or a repository platform. Anything else is refused where it's written,
/// with the near miss when there is one.
#[test]
fn a_dependency_that_names_no_library_is_refused() {
    let scratch = Scratch::repo("unknown-dependencies");
    scratch.write("lib/money/BUILD.buri", "library {\n    visibility: [\"//...\"]\n}\n");
    scratch.write("lib/money/lib.buri", ONE);
    scratch.write(
        "lib/a/BUILD.buri",
        "library {\n    dependencies: [\"//lib/mony\"]\n    test {\n        dependencies: [\"//cmd/x\"]\n    }\n    \
         testing {\n        dependencies: [\"//lib/money/testing\"]\n    }\n}\n",
    );
    scratch.write("lib/a/lib.buri", ONE);
    scratch.write("lib/a/testing/lib.buri", ONE);
    scratch.write("cmd/x/BUILD.buri", "binary {\n    dependencies: [\"lib/money\"]\n    test {\n        dependencies: [\"//nowhere\"]\n    }\n}\n");
    scratch.write("cmd/x/main.buri", NODE_MAIN);
    scratch.write("tools/t/BUILD.buri", "tool {\n    dependencies: [\"//tools/t\"]\n}\n");
    scratch.write("tools/t/tool.buri", "export fn main() {}\n");
    scratch.write("platform/kit/BUILD.buri", "platform {\n    dependencies: [\"//lib/gone\"]\n    entry {\n        name: \"e\"\n        backend: JS\n    }\n}\n");
    scratch.write("platform/kit/platform.buri", "export struct KitHost {}\nexport fn e(host: KitHost): Result<(), Str>;\n");
    let run = scratch.run(&["build", "//lib/money"]);
    run.exits(2)
        .says("error: `//lib/mony` names no library [unknown-dependency]")
        .says("--> lib/a/BUILD.buri:2:20")
        .says("fix: if you meant `//lib/money`, use that")
        .says("error: `//cmd/x` names no library [unknown-dependency]")
        .says("//cmd/x declares no `library` rule, and only a library can be depended on")
        .says("error: `//lib/money/testing` names no library [unknown-dependency]")
        .says("//lib/money declares no `testing` surface")
        .says("error: `lib/money` names no library [unknown-dependency]")
        .says("a dependency is a `//` label")
        .says("error: `//nowhere` names no library [unknown-dependency]")
        .says("no package is at `nowhere`")
        .says("error: `//tools/t` names no library [unknown-dependency]")
        .says("error: `//lib/gone` names no library [unknown-dependency]");

    // A repository platform is a dependency too: a binary imports its host from it.
    let ok = Scratch::copy_of("platform-dependency", &tests_dir().join("repositories/custom-platforms/platform_dependencies/repo"));
    ok.write("cmd/app/BUILD.buri", "binary {\n    tags: [\"client\"]\n    dependencies: [\"//platform/kit\"]\n    outputs: [\n        { platform: \"//platform/kit\" },\n    ]\n}\n");
    ok.write("lib/words/BUILD.buri", "library {\n    visibility: [\"//...\"]\n}\n");
    ok.run(&["build", "//cmd/app"]).ok();
}

/// A library of more sources than are worth reading one at a time builds as
/// one of a few does.
#[test]
fn a_library_of_forty_sources_builds() {
    let scratch = Scratch::repo("forty-sources");
    let names: Vec<String> = (1..=40).map(|i| format!("s{i}.buri")).collect();
    for (i, name) in names.iter().enumerate() {
        scratch.write(&format!("lib/many/{name}"), &format!("export fn f{i}(): Int {{\n    {i}\n}}\n"));
    }
    let listed: Vec<String> = names.iter().map(|n| format!("\"{n}\"")).collect();
    scratch.write(
        "lib/many/BUILD.buri",
        &format!("library {{\n    visibility: [\"//...\"]\n    sources: [{}]\n}}\n", listed.join(", ")),
    );
    scratch.write("lib/many/lib.buri", ONE);
    scratch.write("cmd/app/BUILD.buri", "binary {\n    dependencies: [\"//lib/many\"]\n    outputs: [{ platform: \"node\" }]\n}\n");
    scratch.write(
        "cmd/app/main.buri",
        "from \"node\" import { NodeHost };\nfrom \"//lib/many\" import { one };\n\n\
         export fn main(host: NodeHost): Result<(), Str> {\n    let _ = one();\n    .Ok(())\n}\n",
    );
    scratch.run(&["build", "//cmd/app"]).ok().says(".buri/out/node/cmd/app/app.mjs");
}

/// A build file that's a symbolic link makes its directory a package.
#[test]
fn a_symlinked_build_file_makes_a_package() {
    let scratch = Scratch::repo("symlinked-build-file");
    scratch.write("shared/library.txt", "library {\n    visibility: [\"//...\"]\n}\n");
    scratch.write("lib/link/lib.buri", ONE);
    std::os::unix::fs::symlink("../../shared/library.txt", scratch.path("lib/link/BUILD.buri")).unwrap();
    scratch.run(&["query", "sources(//lib/link)"]).ok().says("lib/link/lib.buri");
}

/// A repository platform's dependency brings its own dependencies into the
/// artifact.
#[test]
fn a_platform_s_dependency_brings_its_own() {
    let scratch =
        Scratch::copy_of("platform-deep", &tests_dir().join("repositories/custom-platforms/platform_dependencies/repo"));
    scratch.write("lib/words/BUILD.buri", "library {\n    visibility: [\"//...\"]\n    dependencies: [\"//lib/deep\"]\n}\n");
    scratch.write("lib/words/lib.buri", "from \"//lib/deep\" import { word };\n\nexport fn greeting(): Str {\n    word()\n}\n");
    scratch.write("lib/deep/BUILD.buri", "library {\n    visibility: [\"//lib/words\"]\n}\n");
    scratch.write("lib/deep/lib.buri", "export fn word(): Str {\n    \"hello from deep down\"\n}\n");
    scratch.run(&["build", "//cmd/app"]).ok();
    assert!(scratch.read(".buri/out/platform/kit/cmd/app/main.mjs").contains("hello from deep down"));
}
