//! The build's rarer paths, driven as a user reaches them: files no rule
//! names open in the editor, a test dependency on a platform, a dependency
//! cycle, a suite naming a backend twice, a source that stops being UTF-8
//! between two analyses, a build file that is not UTF-8, a `buri run` with no
//! JavaScript runtime, a damaged record of a link's unlisted reads, a runner
//! kept under a fresh `BURI_HOME`, a platform asset read as a schema, a
//! `platform` rule outside `//platform/`, `format` on a `testing` surface, and
//! the generator paths a contract, a missing schema or a tool nobody declared
//! takes; and the language server's sweep of a deeply nested file, and a sweep
//! that lands while the server sits idle.
use crate::harness::*;

const PLATFORM: &str =
    "platform {\n    entry {\n        name: \"e\"\n        backend: JS\n    }\n}\n";

const NODE_MAIN: &str =
    "from \"node\" import { NodeHost };\n\nexport fn main(_host: NodeHost): Result<(), Str> {\n    .Ok(())\n}\n";

const ONE: &str = "export fn one(): Int {\n    1\n}\n";

const SUITE: &str =
    "from \"//lib/a\" import { one };\nfrom \"core/testing/assert\" import * as assert;\n\n\
                     test \"one\" {\n    assert.equal(one(), 1);\n}\n";

const INITIALIZE: &str = r#"{"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"rootUri": "file://<scratch>"}}"#;
const INITIALIZED: &str = r#"{"jsonrpc": "2.0", "method": "initialized", "params": {}}"#;
const SHUTDOWN: &str = r#"{"jsonrpc": "2.0", "id": 9, "method": "shutdown"}"#;
const EXIT: &str = r#"{"jsonrpc": "2.0", "method": "exit"}"#;

/// One JSON-RPC message with its `Content-Length` framing, `<scratch>` being
/// the repository.
fn frame(scratch: &Scratch, line: &str) -> Vec<u8> {
    let line = line.replace("<scratch>", &scratch.root.display().to_string());
    format!("Content-Length: {}\r\n\r\n{line}", line.len()).into_bytes()
}

/// A whole session, written at once.
fn session(scratch: &Scratch, lines: &[String]) -> Vec<u8> {
    lines.iter().flat_map(|line| frame(scratch, line)).collect()
}

/// `text` as a JSON string, quotes included.
fn quoted(text: &str) -> String {
    format!(
        "\"{}\"",
        text.replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('\n', "\\n")
    )
}

fn did_open(scratch: &Scratch, rel: &str) -> String {
    format!(
        r#"{{"jsonrpc": "2.0", "method": "textDocument/didOpen", "params": {{"textDocument": {{"uri": "file://<scratch>/{rel}", "languageId": "buri", "version": 1, "text": {}}}}}}}"#,
        quoted(&scratch.read(rel))
    )
}

fn did_change(scratch: &Scratch, rel: &str) -> String {
    format!(
        r#"{{"jsonrpc": "2.0", "method": "textDocument/didChange", "params": {{"textDocument": {{"uri": "file://<scratch>/{rel}", "version": 2}}, "contentChanges": [{{"text": {}}}]}}}}"#,
        quoted(&scratch.read(rel))
    )
}

/// What the server publishes for a file it has nothing to say about.
fn nothing_for(scratch: &Scratch, rel: &str) -> String {
    format!(
        r#"{{"diagnostics":[],"uri":"file://{}/{rel}"}}"#,
        scratch.root.display()
    )
}

/// The editor asks which rule a file is in by its name first. A `lib.buri`,
/// a `testing/lib.buri`, a `main.buri` or a `tool.buri` whose package has no
/// such rule is in none, so nothing checks it and the server reports nothing
/// in it, wrong as it is.
#[test]
fn an_entry_point_s_name_in_a_package_without_its_rule_is_not_checked() {
    let scratch = Scratch::repo("rare-unnamed-entry-points");
    scratch.write("cmd/b/BUILD.buri", JS_BINARY);
    scratch.write("cmd/b/main.buri", NODE_MAIN);
    scratch.write("cmd/t/BUILD.buri", JS_BINARY);
    scratch.write("cmd/t/main.buri", NODE_MAIN);
    scratch.write("lib/a/BUILD.buri", "library {\n}\n");
    scratch.write("lib/a/lib.buri", ONE);
    let wrong = [
        "cmd/b/lib.buri",
        "cmd/b/tool.buri",
        "cmd/t/testing/lib.buri",
        "lib/a/main.buri",
    ];
    for rel in wrong {
        scratch.write(rel, "export fn wrong(): Int {\n    \"not an Int\"\n}\n");
    }
    let mut lines = vec![INITIALIZE.to_string(), INITIALIZED.to_string()];
    lines.extend(wrong.iter().map(|rel| did_open(&scratch, rel)));
    lines.extend([SHUTDOWN.to_string(), EXIT.to_string()]);
    let run = scratch.run_with_stdin(&["lsp"], &session(&scratch, &lines));
    run.ok()
        .says("//cmd/t has a testing/ directory and no `testing` block");
    for rel in wrong {
        run.says(&nothing_for(&scratch, rel));
    }
}

/// A platform is a known dependency label but no library, so a suite that
/// names one in `test.dependencies` reaches nothing more and still runs. A
/// `testing` label whose package isn't there says so.
#[test]
fn a_test_dependency_on_a_platform_reaches_no_library() {
    let scratch = Scratch::repo("rare-platform-test-dependency");
    scratch.write("platform/p/BUILD.buri", PLATFORM);
    scratch.write(
        "lib/a/BUILD.buri",
        "library {\n    test {\n        sources: [\"test/a.buri\"]\n        dependencies: [\"//platform/p\"]\n        \
         backends: [JS]\n    }\n}\n",
    );
    scratch.write("lib/a/lib.buri", ONE);
    scratch.write("lib/a/test/a.buri", SUITE);
    scratch
        .run(&["test", "//lib/a"])
        .ok()
        .says("1 passed, 0 failed");
    scratch.edit(
        "lib/a/BUILD.buri",
        "library {\n",
        "library {\n    dependencies: [\"//lib/zz/testing\"]\n",
    );
    scratch
        .run(&["build", "//lib/a"])
        .exits(2)
        .says("`//lib/zz/testing` names no library [unknown-dependency]")
        .says("no package is at `lib/zz/testing`");
}

/// Two libraries depending on each other build, and `lint` names the cycle.
#[test]
fn a_dependency_cycle_builds_and_lint_names_it() {
    let scratch = Scratch::repo("rare-cycle");
    for (me, other) in [("c1", "c2"), ("c2", "c1")] {
        scratch.write(
            &format!("lib/{me}/BUILD.buri"),
            &format!("library {{\n    dependencies: [\"//lib/{other}\"]\n    visibility: [\"//...\"]\n}}\n"),
        );
        scratch.write(&format!("lib/{me}/lib.buri"), ONE);
    }
    scratch.run(&["build", "//lib/c1"]).ok();
    scratch
        .run(&["lint", "//lib/c1"])
        .exits(1)
        .says("//lib/c1 and //lib/c2 depend on each other [circular-dependency]");
}

/// A suite naming one backend twice runs once on it.
#[test]
fn a_backend_named_twice_runs_the_suite_once() {
    let scratch = Scratch::repo("rare-backend-twice");
    scratch.write("lib/a/BUILD.buri", "library {\n    test {\n        sources: [\"test/a.buri\"]\n        backends: [JS, JS]\n    }\n}\n");
    scratch.write("lib/a/lib.buri", ONE);
    scratch.write("lib/a/test/a.buri", SUITE);
    scratch
        .run(&["test", "//lib/a"])
        .ok()
        .says("1 passed, 0 failed, 0 skipped");
}

/// A JSON file a generator lists is checked as it's typed, even when the
/// generator names a tool nobody declared, which the server reports on the
/// build file instead.
#[test]
fn a_file_listed_for_an_unknown_tool_is_still_checked_as_it_is_typed() {
    let scratch = Scratch::repo("rare-unknown-tool-input");
    scratch.write("lib/x/BUILD.buri", "library {\n    generators: [\n        { tool: \"//nope\", inputs: [\"data.json\"] },\n    ]\n}\n");
    scratch.write("lib/x/data.json", "{ \"side\": 2 }\n");
    scratch.write("lib/x/lib.buri", ONE);
    let lines = [
        INITIALIZE.to_string(),
        INITIALIZED.to_string(),
        did_open(&scratch, "lib/x/data.json"),
        did_change(&scratch, "lib/x/data.json"),
        SHUTDOWN.to_string(),
        EXIT.to_string(),
    ];
    scratch
        .run_with_stdin(&["lsp"], &session(&scratch, &lines))
        .ok()
        .says("`//nope` names no tool")
        .says(r#""code":"json-missing-schema""#);
}

/// A repository language can't be generated by `json`. The editor says so on
/// `REPO.buri`, even while a tool's contract still asks for it.
#[test]
fn the_editor_names_a_language_generated_by_json() {
    let scratch = Scratch::copy_of(
        "rare-json-generate",
        &tests_dir().join("repositories/tools/contract_errors/repo"),
    );
    scratch.write(
        "REPO.buri",
        "language {\n    name: \"conf\"\n    extensions: [\".conf\"]\n    check: \"json\"\n    generate: \"json\"\n}\n",
    );
    scratch.write(
        "tools/b/BUILD.buri",
        "tool {\n    generate {\n        accepts: [\n            { language: \"conf\", type_schema: \"b.schema.json\" },\n        ]\n    }\n}\n",
    );
    let lines = [
        INITIALIZE.to_string(),
        INITIALIZED.to_string(),
        did_open(&scratch, "tools/b/tool.buri"),
        SHUTDOWN.to_string(),
        EXIT.to_string(),
    ];
    scratch
        .run_with_stdin(&["lsp"], &session(&scratch, &lines))
        .ok()
        .says("REPO.buri: `json` makes types for the JSON languages alone, so it can't be `conf`'s `generate`");
}

/// A file the `json` tool reads first, with no contract, and a contract tool
/// reads after is checked against its own `$schema`, which has to be there.
#[test]
fn a_file_read_by_json_and_by_a_contract_needs_its_own_schema() {
    let scratch = Scratch::copy_of(
        "rare-json-then-contract",
        &tests_dir().join("repositories/tools/contract_errors/repo"),
    );
    scratch.write(
        "lib/x/BUILD.buri",
        "library {\n    generators: [\n        { tool: \"json\", inputs: [\"config.json\"] },\n        \
         { tool: \"//tools/a\", inputs: [\"config.json\"] },\n    ]\n}\n",
    );
    scratch
        .run(&["build", "//lib/x"])
        .exits(1)
        .says("this file has no `\"$schema\"` [json-missing-schema]");
    scratch.write(
        "lib/x/config.json",
        "{ \"$schema\": \"//schemas/names.schema.json\", \"names\": [\"a\", \"b\"] }\n",
    );
    scratch.run(&["build", "//lib/x"]).ok();
}

/// `gen` in a package with both rules places a new file by which entry point
/// imports it. An import of the other entry point, of the file itself or of a
/// file that isn't there places nothing, and an entry point missing from disk
/// reaches nothing.
#[test]
fn gen_places_a_file_past_imports_that_reach_nothing() {
    let scratch = Scratch::repo("rare-gen-reach");
    scratch.write("app/p1/BUILD.buri", "library {\n}\n\nbinary {\n}\n");
    scratch.write(
        "app/p1/main.buri",
        "from \"//app/p1/lib.buri\" import { one };\nfrom \"//app/p1/main.buri\" import { main };\n\
         from \"//app/p1/gone.buri\" import { two };\nfrom \"//app/p1/helper.buri\" import { three };\n\n\
         export fn main() {}\n",
    );
    scratch.write("app/p1/lib.buri", ONE);
    scratch.write("app/p1/helper.buri", "export fn three(): Int {\n    3\n}\n");
    scratch.write("app/p2/BUILD.buri", "library {\n}\n\nbinary {\n}\n");
    scratch.write("app/p2/main.buri", "export fn main() {}\n");
    scratch.write("app/p2/h.buri", "export fn h(): Int {\n    3\n}\n");
    scratch
        .run(&["gen", "//app/..."])
        .exits(1)
        .says("+ binary.sources: helper.buri")
        .says("h.buri is reachable from neither `lib.buri` nor `main.buri`");
    assert_eq!(
        scratch.read("app/p1/BUILD.buri"),
        "library {}\n\nbinary {\n    sources: [\"helper.buri\"]\n}\n"
    );
}

/// A language server the test talks to one message at a time, so a change
/// on disk lands between two answers rather than wherever the scheduler puts
/// it.
struct Server {
    child: std::process::Child,
    input: Option<std::process::ChildStdin>,
    output: std::process::ChildStdout,
    read: String,
}

impl Server {
    fn start(scratch: &Scratch) -> Server {
        let mut child = buri_command()
            .args(["lsp", "--color=never"])
            .env("BURI_LSP_ANALYSIS", "synchronous")
            .current_dir(&scratch.root)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("the buri binary runs");
        let input = child.stdin.take();
        let output = child.stdout.take().expect("stdout is piped");
        Server {
            child,
            input,
            output,
            read: String::new(),
        }
    }

    fn send(&mut self, scratch: &Scratch, line: &str) {
        use std::io::Write as _;
        self.input
            .as_mut()
            .unwrap()
            .write_all(&frame(scratch, line))
            .unwrap();
    }

    /// Reads up to the response to request `id`.
    fn answer(&mut self, id: u32) {
        self.read_until(&format!("\"id\":{id},"));
    }

    /// Reads up to the first message holding `want`.
    fn read_until(&mut self, want: &str) {
        use std::io::Read as _;
        loop {
            let mut header = Vec::new();
            let mut byte = [0u8; 1];
            while !header.ends_with(b"\r\n\r\n") {
                assert_eq!(
                    self.output.read(&mut byte).unwrap(),
                    1,
                    "the server stopped before saying {want}"
                );
                header.push(byte[0]);
            }
            let header = String::from_utf8(header).unwrap();
            let length: usize = header
                .trim()
                .trim_start_matches("Content-Length:")
                .trim()
                .parse()
                .unwrap();
            let mut body = vec![0u8; length];
            self.output.read_exact(&mut body).unwrap();
            let body = String::from_utf8(body).unwrap();
            self.read.push_str(&body);
            if body.contains(want) {
                return;
            }
        }
    }

    /// Closes the session and answers everything the server said.
    fn finish(mut self) -> Run {
        use std::io::Read as _;
        drop(self.input.take());
        let mut rest = String::new();
        self.output.read_to_string(&mut rest).unwrap();
        let status = self.child.wait().unwrap();
        Run {
            code: status.code().unwrap_or(-1),
            stdout: format!("{}{rest}", self.read),
            stderr: String::new(),
            what: String::from("buri lsp"),
        }
    }
}

/// A source the last analysis read that stops being UTF-8 before the next is
/// read again from scratch, and the next analysis says it can't be read.
#[test]
fn a_source_that_stops_being_utf_8_between_analyses_is_reported() {
    let scratch = Scratch::repo("rare-not-utf-8");
    scratch.write(
        "lib/a/BUILD.buri",
        "library {\n    sources: [\"b.buri\"]\n}\n",
    );
    scratch.write(
        "lib/a/lib.buri",
        "from \"//lib/a/b.buri\" import { two };\n\nexport fn one(): Int {\n    two()\n}\n",
    );
    scratch.write("lib/a/b.buri", "export fn two(): Int {\n    2\n}\n");
    let save = r#"{"jsonrpc": "2.0", "method": "textDocument/didSave", "params": {"textDocument": {"uri": "file://<scratch>/lib/a/lib.buri"}}}"#;
    let symbols = |id: u32| {
        format!(
            r#"{{"jsonrpc": "2.0", "id": {id}, "method": "textDocument/documentSymbol", "params": {{"textDocument": {{"uri": "file://<scratch>/lib/a/lib.buri"}}}}}}"#
        )
    };
    let mut server = Server::start(&scratch);
    for line in [
        INITIALIZE.to_string(),
        INITIALIZED.to_string(),
        did_open(&scratch, "lib/a/lib.buri"),
        symbols(2),
    ] {
        server.send(&scratch, &line);
    }
    server.answer(2);
    std::fs::write(
        scratch.path("lib/a/b.buri"),
        b"export fn two(): Int {\n    \xff\n}\n",
    )
    .unwrap();
    for line in [save.to_string(), symbols(3)] {
        server.send(&scratch, &line);
    }
    server.answer(3);
    server.send(&scratch, SHUTDOWN);
    server.send(&scratch, EXIT);
    server
        .finish()
        .ok()
        .says("cannot read lib/a/b.buri: stream did not contain valid UTF-8");
}

/// A link's record keeps the digest of each module no rule lists. A record
/// damaged there is a miss: the link runs again and the program still runs.
#[test]
fn a_damaged_record_of_unlisted_reads_links_again() {
    let scratch = Scratch::repo("rare-damaged-reads");
    scratch.write("cmd/b/BUILD.buri", JS_BINARY);
    scratch.write(
        "cmd/b/main.buri",
        "from \"node\" import { NodeHost };\nfrom \"//cmd/b/lib.buri\" import { one };\n\n\
         export fn main(_host: NodeHost): Result<(), Str> {\n    let _ = one();\n    .Ok(())\n}\n",
    );
    scratch.write("cmd/b/lib.buri", ONE);
    scratch.run(&["build", "//cmd/b"]).ok();
    scratch
        .run(&["build", "--explain", "//cmd/b"])
        .ok()
        .says("cached link //cmd/b node");
    let mut damaged = 0;
    for dir in std::fs::read_dir(scratch.path(".buri/cache"))
        .unwrap()
        .flatten()
    {
        let Ok(entries) = std::fs::read_dir(dir.path()) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(bytes) = std::fs::read(entry.path()) else {
                continue;
            };
            let Some(at) = bytes.windows(15).position(|w| w == b"cmd/b/lib.buri\t") else {
                continue;
            };
            let mut bytes = bytes;
            bytes[at + 14] = b' ';
            std::fs::write(entry.path(), bytes).unwrap();
            damaged += 1;
        }
    }
    assert_eq!(
        damaged, 1,
        "no record of the unlisted read was found to damage"
    );
    scratch
        .run(&["build", "--explain", "//cmd/b"])
        .ok()
        .says("run    link //cmd/b node");
    scratch.exec_js("cmd/b").ok();
}

/// A suite whose verdict can't be reused but whose runner can starts the
/// runner already on disk, under a `BURI_HOME` that has never kept it: no new
/// executable is launched.
#[test]
fn a_runner_restored_under_a_fresh_home_is_no_new_executable() {
    if let Some(why) = ci::native_host_gap() {
        ci::skipped("build::rare_paths", &why);
        return;
    }
    let scratch = Scratch::repo("rare-fresh-home");
    scratch.write(
        "lib/a/BUILD.buri",
        "library {\n    test {\n        sources: [\"test/a.buri\"]\n    }\n}\n",
    );
    scratch.write("lib/a/lib.buri", ONE);
    scratch.write("lib/a/test/a.buri", SUITE);
    let first = scratch.path(".home-first").display().to_string();
    let second = scratch.path(".home-second").display().to_string();
    scratch
        .run_with_env(
            &["test", "//lib/a"],
            &[("BURI_PROFILE", "1"), ("BURI_HOME", first.as_str()), LINKED],
        )
        .ok()
        .says("new executables launched 1");
    // A golden is in the verdict's key and not in the build's.
    scratch.write("lib/a/test/__snapshots__/unused.png", "not a picture\n");
    scratch
        .run_with_env(
            &["test", "//lib/a"],
            &[
                ("BURI_PROFILE", "1"),
                ("BURI_HOME", second.as_str()),
                LINKED,
            ],
        )
        .ok()
        .says("1 passed, 0 failed")
        .says("suites restored 1")
        .says("new executables launched 0");
}

/// A binary's generator input can name a schema its platform ships as an
/// asset. The asset is copied beside the artifacts, and an edit to it reaches
/// the next build.
#[test]
fn a_generator_input_s_schema_can_be_its_platform_s_asset() {
    let scratch = Scratch::copy_of(
        "rare-schema-asset",
        &tests_dir().join("repositories/custom-platforms/platform_rules/repo"),
    );
    scratch.edit(
        "platform/pair/BUILD.buri",
        "platform {\n",
        "platform {\n    assets: [\"s.schema.json\"]\n",
    );
    scratch.write(
        "platform/pair/s.schema.json",
        "{\n    \"$schema\": \"https://json-schema.org/draft/2020-12/schema\",\n    \"title\": \"S\",\n    \
         \"type\": \"object\",\n    \"properties\": { \"$schema\": { \"type\": \"string\" }, \"side\": { \"type\": \"integer\" } }\n}\n",
    );
    scratch.edit(
        "cmd/app/BUILD.buri",
        "binary {\n",
        "binary {\n    generators: [\n        { tool: \"json\", inputs: [\"d.json\"] },\n    ]\n",
    );
    scratch.write(
        "cmd/app/d.json",
        "{ \"$schema\": \"//platform/pair/s.schema.json\", \"side\": 2 }\n",
    );
    let copied = ".buri/out/platform/pair/cmd/app/s.schema.json";
    scratch.run(&["build", "//cmd/app"]).ok();
    assert!(scratch.read(copied).contains("\"title\": \"S\""));
    scratch.edit(
        "platform/pair/s.schema.json",
        "\"title\": \"S\"",
        "\"title\": \"Side\"",
    );
    scratch.run(&["build", "//cmd/app"]).ok();
    assert!(scratch.read(copied).contains("\"title\": \"Side\""));
}

/// A `platform` rule outside `//platform/` is misplaced, and an output naming
/// it names no platform.
#[test]
fn a_platform_rule_outside_platform_is_misplaced() {
    let scratch = Scratch::repo("rare-platform-outside");
    scratch.write("lib/p/BUILD.buri", PLATFORM);
    scratch.write(
        "cmd/x/BUILD.buri",
        "binary {\n    outputs: [{ platform: \"//lib/p\" }]\n}\n",
    );
    scratch.write("cmd/x/main.buri", NODE_MAIN);
    scratch
        .run(&["build", "//..."])
        .exits(2)
        .says("//lib/p declares a `platform` rule outside //platform/ [misplaced-rule]")
        .says("`//lib/p` is not a platform [unknown-platform]");
}

/// `buri format` on a label formats the library's `testing` surface too.
#[test]
fn format_on_a_label_reaches_the_testing_surface() {
    let scratch = Scratch::repo("rare-format-testing");
    scratch.write("lib/c/BUILD.buri", "library {\n    testing {\n    }\n}\n");
    scratch.write("lib/c/lib.buri", ONE);
    scratch.write("lib/c/testing/lib.buri", "export fn fake(): Int {   2 }\n");
    scratch
        .run(&["format", "//lib/c"])
        .ok()
        .says("formatted lib/c/testing/lib.buri");
    assert_eq!(
        scratch.read("lib/c/testing/lib.buri"),
        "export fn fake(): Int {\n    2\n}\n"
    );
}

/// A schema a generator lists and nobody wrote is missing twice over: as the
/// generator's input, and as the module an import names.
#[test]
fn a_listed_schema_that_is_not_there_is_reported_where_it_is_imported() {
    let scratch = Scratch::repo("rare-missing-proto");
    scratch.write(
        "lib/g/BUILD.buri",
        "library {\n    generators: [\n        { tool: \"proto\", inputs: [\"s.proto\"] },\n    ]\n}\n",
    );
    scratch.write(
        "lib/g/lib.buri",
        "from \"//lib/g/s.proto\" import { S };\n\nexport fn f(): Int {\n    1\n}\n",
    );
    scratch
        .run(&["build", "//lib/g"])
        .exits(1)
        .says("s.proto does not exist [unknown-source]")
        .says("\"//lib/g/s.proto\" names no file (lib/g/s.proto) [unknown-module]");
}

/// **A build file or a `REPO.buri` that can't be read is refused by name**,
/// rather than with the operating system's sentence and no file in it.
#[test]
fn a_build_file_that_cannot_be_read_is_named() {
    let scratch = Scratch::repo("rare-unreadable-build-file");
    scratch.write("lib/a/BUILD.buri", "library {}\n");
    scratch.write("lib/a/lib.buri", "export fn one(): Int { 1 }\n");
    scratch.write("lib/bad/lib.buri", "export fn two(): Int { 2 }\n");
    std::fs::write(scratch.path("lib/bad/BUILD.buri"), b"library {}\n\xff\n").unwrap();
    scratch
        .run(&["build", "//lib/a"])
        .exits(2)
        .says("cannot read lib/bad/BUILD.buri: stream did not contain valid UTF-8");

    std::fs::remove_dir_all(scratch.path("lib/bad")).unwrap();
    std::fs::write(scratch.path("REPO.buri"), b"\xff\n").unwrap();
    scratch
        .run(&["build", "//lib/a"])
        .exits(2)
        .says("cannot read REPO.buri: stream did not contain valid UTF-8");
}

/// `buri run` of a JavaScript output where no runtime can be started says it
/// could not start the artifact and what it needs to.
#[test]
fn a_run_with_no_javascript_runtime_says_what_it_needs() {
    let scratch = Scratch::copy_of(
        "rare-run-no-runtime",
        &tests_dir().join("repositories/cli/run_passthrough/repo"),
    );
    scratch
        .run_with_env(&["run", "//cmd/echo"], &[("BURI_JS", "/nonexistent/bun")])
        .exits(2)
        .says("error: cannot execute the artifact")
        .says("`buri run` needs a JavaScript runtime; install bun, or set BURI_JS");
}

/// A suite that lists a source that isn't there, or an entry point, is told
/// so, whichever rule it is the suite of, and the sources it did list
/// still load.
#[test]
fn a_suite_listing_a_source_that_is_not_there_is_refused_in_every_rule() {
    let scratch = Scratch::repo("rare-suite-sources");
    scratch.write(
        "lib/l/BUILD.buri",
        "library {\n    test {\n        sources: [\"missing.buri\"]\n    }\n}\n",
    );
    scratch.write("lib/l/lib.buri", ONE);
    scratch.write(
        "cmd/b/BUILD.buri",
        "binary {\n    outputs: [{ platform: \"node\" }]\n\n    test {\n        sources: [\"missing.buri\"]\n    }\n}\n",
    );
    scratch.write("cmd/b/main.buri", NODE_MAIN);
    scratch.write(
        "tools/t/BUILD.buri",
        "tool {\n    generate {}\n\n    test {\n        sources: [\"missing.buri\"]\n    }\n}\n",
    );
    scratch.write(
        "tools/t/tool.buri",
        "from \"core/tool\" import { Generated, GenerateRequest };\nfrom \"platform/effect\" import { Allocator };\n\nexport fn generate<C: Allocator>(ctx: C, request: GenerateRequest<Str>): Generated {\n    Generated { modules: [], diagnostics: [], needs: [] }\n}\n",
    );
    for package in ["lib/l", "cmd/b", "tools/t"] {
        scratch
            .run(&["test", &format!("//{package}")])
            .exits(1)
            .says("missing.buri does not exist [unknown-source]");
    }
}

/// A `testing/` directory with a build file of its own is a package of its
/// own, and the library beside it is not told to declare it.
#[test]
fn a_testing_directory_that_is_a_package_is_not_the_librarys_surface() {
    let scratch = Scratch::repo("rare-testing-package");
    scratch.write("lib/a/BUILD.buri", "library {}\n");
    scratch.write("lib/a/lib.buri", ONE);
    scratch.write("lib/a/testing/BUILD.buri", "library {}\n");
    scratch.write("lib/a/testing/lib.buri", ONE);
    scratch.run(&["build", "//lib/a"]).ok();
}

/// A file nested as deep as the parser allows is swept like any other.
///
/// The sweep compiles every target on a thread of its own, and every stage
/// after parsing walks the tree by recursion. That thread had the platform's
/// default stack rather than the one `buri lint` runs on, so a chain of `else
/// if`s the command line lints overflowed it and took the server down.
#[test]
fn a_deeply_nested_file_is_swept_like_the_command_line_lints_it() {
    let scratch = Scratch::repo("rare-deep-sweep");
    let chain: String = (0..2_000)
        .map(|i| format!("if (x == {i}) {{ {i} }} else "))
        .collect();
    scratch.write("lib/deep/BUILD.buri", "library {\n}\n");
    scratch.write(
        "lib/deep/lib.buri",
        &format!("export fn pick(x: I32): I32 {{\n    {chain}{{ -1 }}\n}}\n"),
    );
    scratch.run(&["lint", "//lib/deep"]).ok();
    let pull = r#"{"jsonrpc": "2.0", "id": 2, "method": "workspace/diagnostic", "params": {"previousResultIds": []}}"#;
    scratch
        .run_with_stdin(
            &["lsp"],
            &session(
                &scratch,
                &[INITIALIZE.to_string(), INITIALIZED.to_string(), pull.to_string(), SHUTDOWN.to_string(), EXIT.to_string()],
            ),
        )
        .ok()
        .says(&format!(
            r#"{{"items":[],"kind":"full","resultId":"2","uri":"file://{}/lib/deep/lib.buri","version":null}}"#,
            scratch.root.display()
        ));
}

/// In an editor's schedule a sweep publishes when it lands, with no request
/// waiting for it: the server is sitting idle at its input, and the worker
/// wakes it.
///
/// Deterministic without a clock: the only thing that publishes the error is
/// the landed sweep, and the test reads until it arrives.
#[test]
fn a_sweep_publishes_when_it_lands_on_an_idle_server() {
    let scratch = Scratch::repo("rare-idle-sweep");
    scratch.write("lib/a/BUILD.buri", "library {\n}\n");
    scratch.write("lib/a/lib.buri", ONE);
    scratch.write(
        "lib/b/BUILD.buri",
        "library {\n    dependencies: [\"//lib/a\"]\n}\n",
    );
    scratch.write(
        "lib/b/lib.buri",
        "from \"//lib/a\" import { one };\n\nexport fn two(): Int {\n    one() + 1\n}\n",
    );
    let initialize = r#"{"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"rootUri": "file://<scratch>", "initializationOptions": {"analysis": "asynchronous"}}}"#;
    let pull = |id: u32| {
        format!(
            r#"{{"jsonrpc": "2.0", "id": {id}, "method": "workspace/diagnostic", "params": {{"previousResultIds": []}}}}"#
        )
    };
    let mut server = Server::start(&scratch);
    for line in [
        initialize.to_string(),
        INITIALIZED.to_string(),
        did_open(&scratch, "lib/a/lib.buri"),
        pull(2),
    ] {
        server.send(&scratch, &line);
    }
    server.answer(2);
    // `one` renamed: `lib/b/lib.buri`, which nobody has open, no longer
    // compiles, and only a sweep reaches it.
    scratch.write("lib/a/lib.buri", "export fn won(): Int {\n    1\n}\n");
    for line in [did_change(&scratch, "lib/a/lib.buri"), pull(3)] {
        server.send(&scratch, &line);
    }
    server.answer(3);
    server.read_until("`one`");
    server.send(&scratch, SHUTDOWN);
    server.send(&scratch, EXIT);
    server.finish().ok();
}
