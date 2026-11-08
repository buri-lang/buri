//! What a build says when a tool goes wrong: a tool that asks for files
//! outside the repository; tools built from each other's generated code;
//! contracts the loader refuses, read by the language server anyway; and a
//! tool's request types. A tool process that answers nothing, dies, or says
//! too much is a shell script standing in for a tool, in
//! `build::generators`'s unit tests.
//!
//! ```text
//! cargo test -p buri --test build generator_failures::
//! ```

use crate::harness::{indent, Run, Scratch};

/// Each input's first word becomes an exported `Int`, a hundred over its
/// second.
const TOOL: &str = r#"from "core/buri/ast" import * as ast;
from "core/str" import * as str;
from "core/tool" import { Generated, GenerateRequest };
from "platform/effect" import { Allocator };

export fn generate<C: Allocator>(ctx: C, request: GenerateRequest<Str>): Generated {
    let text = request.inputs.get(0).map(fn(i) => i.value).withDefault("").trim();
    let split = text.splitOnce(" ").withDefault(("none", "1"));
    let value = split.1.toInt().withDefault(1);
    let source = str.format(ctx, "export let ${split.0}: Int = ${100 / value};\n");
    let modules = match (ast.parse(ctx, "", source)) {
        .Ok(parsed) => [(split.0, parsed)],
        .Err(_) => [],
    };
    Generated { modules: modules, diagnostics: [], needs: [] }
}
"#;

const SUMMING: &str = r#"from "core/io" import * as io;
from "node" import { NodeHost };
from "platform/effect" import { Allocator, Stdout };
from "//lib/wire" import { alpha, beta };

export fn main(host: NodeHost): Result<(), Str> {
    let ctx = context {
        Allocator: host.alloc,
        Stdout: host.stdout,
    };
    io
        .println(ctx, "${alpha + beta}")
        .mapErr(fn(_e) => "could not write to standard output")
}
"#;

/// A schema whose root type is `Names`.
const NAMES_SCHEMA: &str = r#"{
    "$schema": "https://json-schema.org/draft/2020-12/schema",
    "title": "Names",
    "type": "object",
    "properties": {
        "names": { "type": "array", "items": { "type": "string" } }
    },
    "required": ["names"]
}
"#;

/// A tool reading `Names` through its `json` contract, answering with no module.
const NAMES_TOOL: &str = r#"from "core/tool" import { Generated, GenerateRequest };
from "platform/effect" import { Allocator };
from "//tools/b/json" import * as a;

export fn generate<C: Allocator>(ctx: C, request: GenerateRequest<a.Names>): Generated {
    Generated { modules: [], diagnostics: [], needs: [] }
}
"#;

/// A tool reading `Names` through `//tools/ta`'s contract, answering with one
/// module, `b`, that counts them.
const COUNTING_TOOL: &str = r#"from "core/buri/ast" import * as ast;
from "core/tool" import { Generated, GenerateRequest };
from "platform/effect" import { Allocator };
from "//tools/ta/json" import * as a;

export fn generate<C: Allocator>(ctx: C, request: GenerateRequest<a.Names>): Generated {
    let total = request
        .inputs
        .fold(fn(n: Int, input) => n + input.value.names.length(), 0);
    let int = ast.Name { text: "Int", origin: ast.nowhere() };
    let count = ast.Item {
        kind: .Let(ast.LetDecl {
            name: ast.Name { text: "count", origin: ast.nowhere() },
            ty: ast.Type { kind: .Named(int, []), origin: ast.nowhere() },
            value: ast.Expr { kind: .Int(total), origin: ast.nowhere() },
            exported: true,
            docs: [],
        }),
        origin: ast.nowhere(),
    };
    Generated {
        modules: [("b", ast.Module { items: [count], docs: [] })],
        diagnostics: [],
        needs: [],
    }
}
"#;

/// A repository whose `//lib/wire` runs `//tools/gen` twice, and a program
/// printing the sum of what the two modules hold: 150.
fn two_entries(name: &str) -> Scratch {
    let scratch = Scratch::repo(name);
    scratch.write("tools/gen/BUILD.buri", "tool {\n    generate {}\n}\n");
    scratch.write("tools/gen/tool.buri", TOOL);
    scratch.write(
        "lib/wire/BUILD.buri",
        "library {\n    generators: [\n        { tool: \"//tools/gen\", inputs: [\"alpha.txt\"] },\n        { tool: \"//tools/gen\", inputs: [\"beta.txt\"] },\n    ]\n\n    visibility: [\"//visibility:public\"]\n}\n",
    );
    scratch.write("lib/wire/alpha.txt", "alpha 1\n");
    scratch.write("lib/wire/beta.txt", "beta 2\n");
    scratch.write(
        "lib/wire/lib.buri",
        "from \"//lib/wire/alpha\" export { alpha };\nfrom \"//lib/wire/beta\" export { beta };\n",
    );
    scratch.write(
        "cmd/app/BUILD.buri",
        "binary {\n    dependencies: [\"//lib/wire\"]\n\n    outputs: [\n        { platform: \"node\" },\n    ]\n}\n",
    );
    scratch.write("cmd/app/main.buri", SUMMING);
    scratch
}

/// A tool that needs files: asked with none, it names them, inside the
/// repository and out; asked with some, it says which it was handed.
const NEEDING_TOOL: &str = r#"from "core/str" import * as str;
from "core/tool" import { Diagnostic, Generated, GenerateRequest };
from "platform/effect" import { Allocator };

export fn generate<C: Allocator>(ctx: C, request: GenerateRequest<Str>): Generated {
    if (request.files.isEmpty()) {
        Generated {
            modules: [],
            diagnostics: [],
            needs: [
                "//lib/wire/extra.txt",
                "lib/wire/extra.txt",
                "//lib/wire/extra.txt",
                "//",
                "/etc/hosts",
                "https://example.org/a",
                "a\\b",
                "../../up",
            ],
        }
    } else {
        let handed = request.files.mapCtx(ctx, fn(c, f) => str.format(c, "${f.0}=${f.1.trim()}")).join(ctx, ", ");
        Generated {
            modules: [],
            diagnostics: [Diagnostic { code: "handed", message: handed, note: .None, fix: .None, origin: .None }],
            needs: [],
        }
    }
}
"#;

/// A tool's `needs` are read and handed back once each, and one outside the
/// repository is refused by name.
#[test]
fn a_tool_s_needs_outside_the_repository_are_refused() {
    let scratch = two_entries("tool-needs");
    scratch.write("tools/gen/tool.buri", NEEDING_TOOL);
    scratch.write("lib/wire/extra.txt", "extra\n");
    let run = scratch.run(&["build", "//lib/wire"]);
    run.exits(1);
    for outside in [
        "//",
        "/etc/hosts",
        "https://example.org/a",
        "a\\b",
        "../../up",
    ] {
        run.says(&format!(
            "error: `{outside}` is not in this repository [needs-outside-repository]"
        ));
    }
    run.says("= fix: check the file in, and have the tool ask for it by its repository path")
        .says("//lib/wire/extra.txt=extra, lib/wire/extra.txt=extra")
        .silent_about("$schema")
        .silent_about("[schema-outside-repository]")
        .silent_about("`//lib/wire/extra.txt` is not");
}

/// A language's `check` and the `generate` a contract asks for types say a
/// need outside the repository in the same words a generator's does.
#[test]
fn a_check_s_and_a_contract_s_needs_outside_the_repository_are_refused() {
    let scratch = Scratch::repo("language-needs");
    scratch.write(
        "REPO.buri",
        "language {\n    name: \"lines\"\n    extensions: [\".lines\"]\n    check: \"//tools/types\"\n    generate: \"//tools/types\"\n}\n",
    );
    scratch.write(
        "tools/types/BUILD.buri",
        "tool {\n    check {}\n\n    generate {}\n}\n",
    );
    scratch.write(
        "tools/types/tool.buri",
        "from \"core/tool\" import { Checked, CheckRequest, Generated, GenerateRequest };\n\
         from \"platform/effect\" import { Allocator };\n\n\
         export fn check<C: Allocator>(ctx: C, request: CheckRequest<Str>): Checked {\n    \
         Checked { diagnostics: [], needs: [\"https://example.org/a\"] }\n}\n\n\
         export fn generate<C: Allocator>(ctx: C, request: GenerateRequest<Str>): Generated {\n    \
         Generated { modules: [], diagnostics: [], needs: [\"https://example.org/a\"] }\n}\n",
    );
    scratch.write(
        "tools/lines/BUILD.buri",
        "tool {\n    generate {\n        accepts: [\n            { language: \"lines\", type_schema: \"words.lines\" },\n        ]\n    }\n}\n",
    );
    scratch.write("tools/lines/words.lines", "one\n");
    scratch.write("tools/lines/tool.buri", TOOL);
    scratch.write("tools/gen/BUILD.buri", "tool {\n    generate {}\n}\n");
    scratch.write("tools/gen/tool.buri", TOOL);
    scratch.write("lib/x/BUILD.buri", "library {\n    generators: [\n        { tool: \"//tools/gen\", inputs: [\"x.lines\"] },\n    ]\n}\n");
    scratch.write("lib/x/x.lines", "x 1\n");
    scratch.write("lib/x/lib.buri", "export let one: Int = 1;\n");
    for (target, at) in [
        ("//tools/lines", "tools/lines/BUILD.buri:4:47"),
        ("//lib/x", "lib/x/x.lines"),
    ] {
        scratch
            .run(&["build", target])
            .exits(1)
            .says(&format!("error: `https://example.org/a` is not in this repository [needs-outside-repository]\n --> {at}"))
            .says("= fix: check the file in, and have the tool ask for it by its repository path")
            .silent_about("[schema-outside-repository]");
    }
}

/// Two libraries, each generated by a tool built from the other, are
/// generated one at a time. A third tool built from the library that runs it
/// is a cycle, and said to be.
#[test]
fn tools_built_from_each_other_s_libraries_run_one_at_a_time() {
    let scratch = Scratch::repo("tools-from-each-other");
    scratch.write(
        "tools/ta/BUILD.buri",
        "tool {\n    dependencies: [\"//lib/b\"]\n\n    generate {\n        accepts: [\n            { language: \"json\", type_schema: \"b.schema.json\" },\n        ]\n    }\n}\n",
    );
    scratch.write("tools/ta/b.schema.json", NAMES_SCHEMA);
    scratch.write("tools/ta/tool.buri", COUNTING_TOOL);
    scratch.write(
        "tools/tb/BUILD.buri",
        "tool {\n    dependencies: [\"//lib/a\"]\n\n    generate {}\n}\n",
    );
    scratch.write("tools/tb/tool.buri", TOOL);
    scratch.write(
        "lib/a/BUILD.buri",
        "library {\n    generators: [\n        { tool: \"//tools/ta\", inputs: [\"alpha.json\"] },\n    ]\n\n    visibility: [\"//visibility:public\"]\n}\n",
    );
    scratch.write("lib/a/alpha.json", "{\"names\": [\"a\", \"b\"]}\n");
    scratch.write("lib/a/lib.buri", "from \"//lib/a/b\" export { count };\n");
    scratch.write(
        "lib/b/BUILD.buri",
        "library {\n    generators: [\n        { tool: \"//tools/tb\", inputs: [\"beta.txt\"] },\n    ]\n\n    visibility: [\"//visibility:public\"]\n}\n",
    );
    scratch.write("lib/b/beta.txt", "beta 2\n");
    scratch.write("lib/b/lib.buri", "from \"//lib/b/beta\" export { beta };\n");
    scratch.run(&["build", "//lib/a", "//lib/b"]).ok();

    scratch.write(
        "tools/tc/BUILD.buri",
        "tool {\n    dependencies: [\"//lib/a\"]\n\n    generate {}\n}\n",
    );
    scratch.write("tools/tc/tool.buri", TOOL);
    scratch.write(
        "lib/a/BUILD.buri",
        "library {\n    generators: [\n        { tool: \"//tools/ta\", inputs: [\"alpha.json\"] },\n        { tool: \"//tools/tc\", inputs: [\"gamma.txt\"] },\n    ]\n\n    visibility: [\"//visibility:public\"]\n}\n",
    );
    scratch.write("lib/a/gamma.txt", "gamma 1\n");
    scratch
        .run(&["build", "//lib/a"])
        .exits(1)
        .says("error: `//tools/tc` is built from //lib/a, which is the target that runs it [circular-generator]");
}

/// Frames one JSON-RPC message per line, `<scratch>` being the repository,
/// between an `initialize` and a `shutdown`.
fn session(scratch: &Scratch, lines: &[&str]) -> Vec<u8> {
    let mut all = vec![
        r#"{"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"rootUri": "file://<scratch>"}}"#,
        r#"{"jsonrpc": "2.0", "method": "initialized", "params": {}}"#,
    ];
    all.extend(lines);
    all.extend([
        r#"{"jsonrpc": "2.0", "id": 99, "method": "shutdown"}"#,
        r#"{"jsonrpc": "2.0", "method": "exit"}"#,
    ]);
    let mut out = Vec::new();
    for line in all {
        let line = line.replace("<scratch>", &scratch.root.display().to_string());
        out.extend_from_slice(format!("Content-Length: {}\r\n\r\n{line}", line.len()).as_bytes());
    }
    out
}

/// The editor's answer to request `id`.
fn reply(run: &Run, id: u32) -> String {
    let head = format!("{{\"id\":{id},");
    let at = run
        .stdout
        .find(&head)
        .unwrap_or_else(|| panic!("no reply {id}:\n{}", indent(&run.all())));
    let rest = &run.stdout[at..];
    rest[..rest.find("Content-Length").unwrap_or(rest.len())].to_string()
}

/// The `accepts` entries the loader refuses — a language twice, one the
/// repository doesn't have, one with no `generate` — leave the editor the
/// types of the one it can generate, so the tool's own source reads clean.
#[test]
fn an_editor_gets_the_contract_types_a_refused_build_file_still_names() {
    let scratch = Scratch::repo("contracts-in-the-editor");
    scratch.write(
        "REPO.buri",
        "language {\n    name: \"lines\"\n    extensions: [\".lines\"]\n    check: \"//tools/lines\"\n}\n",
    );
    scratch.write("tools/lines/BUILD.buri", "tool {\n    check {}\n}\n");
    scratch.write(
        "tools/lines/tool.buri",
        "from \"core/tool\" import { Checked, CheckRequest };\nfrom \"platform/effect\" import { Allocator };\n\nexport fn check<C: Allocator>(ctx: C, request: CheckRequest<Str>): Checked {\n    Checked { diagnostics: [], needs: [] }\n}\n",
    );
    scratch.write(
        "tools/b/BUILD.buri",
        "tool {\n    generate {\n        accepts: [\n            { language: \"json\", type_schema: \"b.schema.json\" },\n            { language: \"json\", type_schema: \"b.schema.json\" },\n            { language: \"yaml\", type_schema: \"b.schema.json\" },\n            { language: \"lines\", type_schema: \"b.schema.json\" },\n        ]\n    }\n}\n",
    );
    scratch.write("tools/b/b.schema.json", NAMES_SCHEMA);
    scratch.write("tools/b/tool.buri", NAMES_TOOL);
    scratch
        .run(&["build", "//tools/b"])
        .exits(2)
        .says("[accepts-unknown-language]")
        .says("[accepts-missing-generate]");
    let run = scratch.run_with_stdin(
        &["lsp"],
        &session(
            &scratch,
            &[r#"{"jsonrpc": "2.0", "id": 2, "method": "textDocument/diagnostic", "params": {"textDocument": {"uri": "file://<scratch>/tools/b/tool.buri"}}}"#],
        ),
    );
    run.ok();
    assert_eq!(
        reply(&run, 2),
        r#"{"id":2,"jsonrpc":"2.0","result":{"items":[],"kind":"full","resultId":"1"}}"#
    );
}

/// The contract an input is checked under, in the editor, is the first entry's
/// whose tool has one: past an entry whose tool isn't there, and one with no
/// contract. One file under two contracts is refused where the second names it.
#[test]
fn an_editor_checks_an_input_under_the_contract_that_reads_it() {
    let scratch = Scratch::repo("contract-of-an-input");
    scratch.write("tools/gen/BUILD.buri", "tool {\n    generate {}\n}\n");
    scratch.write("tools/gen/tool.buri", TOOL);
    for (tool, root) in [("b", "Names"), ("c", "Other")] {
        scratch.write(
            &format!("tools/{tool}/BUILD.buri"),
            &format!("tool {{\n    generate {{\n        accepts: [\n            {{ language: \"json\", type_schema: \"{tool}.schema.json\" }},\n        ]\n    }}\n}}\n"),
        );
        scratch.write(
            &format!("tools/{tool}/{tool}.schema.json"),
            &NAMES_SCHEMA.replace("Names", root),
        );
        scratch.write(
            &format!("tools/{tool}/tool.buri"),
            &NAMES_TOOL
                .replace("//tools/b/json", &format!("//tools/{tool}/json"))
                .replace("a.Names", &format!("a.{root}")),
        );
    }
    scratch.write(
        "lib/x/BUILD.buri",
        "library {\n    generators: [\n        { tool: \"//tools/missing\", inputs: [\"other.txt\"] },\n        { tool: \"//tools/gen\", inputs: [\"other.txt\"] },\n        { tool: \"//tools/b\", inputs: [\"config.json\"] },\n        { tool: \"//tools/b\", inputs: [\"config.json\"] },\n        { tool: \"//tools/gen\", inputs: [\"config.json\"] },\n    ]\n}\n",
    );
    scratch.write("lib/x/config.json", "{\"names\": [\"a\"]}\n");
    scratch.write("lib/x/other.txt", "other 1\n");
    scratch.write("lib/x/lib.buri", "export let one: Int = 1;\n");
    scratch.write(
        "lib/y/BUILD.buri",
        "library {\n    generators: [\n        { tool: \"//tools/b\", inputs: [\"data.json\"] },\n        { tool: \"//tools/c\", inputs: [\"data.json\"] },\n        { tool: \"json\", inputs: [\"other.txt\"] },\n    ]\n}\n",
    );
    scratch.write("lib/y/data.json", "{\"names\": [\"a\"]}\n");
    scratch.write("lib/y/other.txt", "other 1\n");
    scratch.write("lib/y/lib.buri", "export let one: Int = 1;\n");
    let run = scratch.run_with_stdin(
        &["lsp"],
        &session(
            &scratch,
            &[
                r#"{"jsonrpc": "2.0", "method": "textDocument/didChange", "params": {"textDocument": {"uri": "file://<scratch>/lib/x/config.json", "version": 2}, "contentChanges": [{"text": "{\"names\": [2]}\n"}]}}"#,
                r#"{"jsonrpc": "2.0", "id": 2, "method": "textDocument/diagnostic", "params": {"textDocument": {"uri": "file://<scratch>/lib/y/BUILD.buri"}}}"#,
            ],
        ),
    );
    run.ok()
        .says("`//tools/missing` names no tool")
        .says(r#""message":"expected a string, found an integer""#)
        .says("the schema says so at `tools/b/b.schema.json#/properties/names/items/type`");
    assert!(
        reply(&run, 2).contains(
            r#""code":"schema-mismatch","message":"`data.json` is read under two contracts, `//tools/b/b.schema.json` and `//tools/c/c.schema.json`""#
        ),
        "the second contract was not refused:\n{}",
        indent(&run.all())
    );
}

/// An entry point's request must name its contract's root type through the
/// contract module's import; one that names anything else is refused, and one
/// with no request or no generic context isn't checked for effects.
#[test]
fn a_tool_s_request_type_names_its_contract_s_root() {
    let scratch = Scratch::repo("request-types");
    let head = "from \"core/tool\" import { Generated, GenerateRequest };\nfrom \"platform/effect\" import { Allocator };\nfrom \"//tools/TOOL/json\" import * as a;\n\n";
    let body = "{\n    Generated { modules: [], diagnostics: [], needs: [] }\n}\n";
    let tools = [
        ("none", "export fn generate(): Generated "),
        ("int", "export fn generate(ctx: Int, request: GenerateRequest<a.Names>): Generated "),
        ("tuple", "export fn generate<C: Allocator>(ctx: (Int, Int), request: GenerateRequest<a.Names>): Generated "),
        ("member", "export fn generate<C: Allocator>(ctx: C, request: GenerateRequest<a.Other>): Generated "),
        ("namespace", "export fn generate<C: Allocator>(ctx: C, request: GenerateRequest<b.Names>): Generated "),
        ("head", "export fn generate<C: Allocator>(ctx: C, request: Other<a.Names>): Generated "),
    ];
    for (tool, signature) in tools {
        scratch.write(
            &format!("tools/{tool}/BUILD.buri"),
            "tool {\n    generate {\n        accepts: [\n            { language: \"json\", type_schema: \"b.schema.json\" },\n        ]\n    }\n}\n",
        );
        scratch.write(&format!("tools/{tool}/b.schema.json"), NAMES_SCHEMA);
        scratch.write(
            &format!("tools/{tool}/tool.buri"),
            &format!("{}{signature}{body}", head.replace("TOOL", tool)),
        );
    }
    let run = scratch.run(&["build", "//tools/..."]);
    run.exits(1);
    for tool in ["member", "namespace", "head"] {
        run.says(&format!(
            "error: `generate` takes `GenerateRequest<Names>` under its `json` contract [tool-request-mismatch]\n --> tools/{tool}/tool.buri:5:"
        ));
    }
    for tool in ["none", "int", "tuple"] {
        run.silent_about(&format!("tools/{tool}/"));
    }
}

/// A repository language whose `generate` answers a contract's `typesOf`
/// with a module that has no `decode` of the usual shape still gives the tool
/// its types; and a file whose `format` tool answers with no layout is left as
/// it is.
#[test]
fn a_repository_language_s_tools_answer_however_they_answer() {
    let scratch = Scratch::repo("language-answers");
    scratch.write(
        "REPO.buri",
        "language {\n    name: \"lines\"\n    extensions: [\".lines\"]\n    generate: \"//tools/types\"\n    format: \"//tools/types\"\n}\n",
    );
    scratch.write(
        "tools/lines/BUILD.buri",
        "tool {\n    generate {\n        accepts: [\n            { language: \"lines\", type_schema: \"words.lines\" },\n        ]\n    }\n}\n",
    );
    scratch.write("tools/lines/words.lines", "one\ntwo\n");
    scratch.write("tools/lines/tool.buri", TOOL);
    scratch.write(
        "tools/types/BUILD.buri",
        "tool {\n    generate {}\n\n    format {}\n}\n",
    );
    scratch.write("tools/types/tool.buri", TYPES_TOOL);
    scratch.write("lib/x/BUILD.buri", "library {\n    generators: [\n        { tool: \"//tools/lines\", inputs: [\"x.lines\"] },\n    ]\n}\n");
    scratch.write("lib/x/x.lines", "three\n");
    scratch.write("lib/x/lib.buri", "export let one: Int = 1;\n");
    scratch.run(&["build", "//tools/lines"]).ok();
    scratch
        .run(&["format", "--check", "lib/x/x.lines"])
        .exits(1)
        .says("lib/x/x.lines: does not parse, and was left exactly as it is");
}

/// A language's tool whose `generate` answers every request with one module,
/// `lines`, holding no `decode` of the usual shape, and whose `format` answers
/// with no layout.
const TYPES_TOOL: &str = r#"from "core/buri/ast" import * as ast;
from "core/format" import { Doc };
from "core/tool" import { Diagnostic, FormatRequest, Generated, GenerateRequest };
from "platform/effect" import { Allocator };

export fn generate<C: Allocator>(ctx: C, request: GenerateRequest<Str>): Generated {
    let source = "fn hidden(): Int {\n    1\n}\n\nexport fn other(): Int {\n    2\n}\n\nexport fn decode(): Int {\n    3\n}\n";
    let modules = match (ast.parse(ctx, "", source)) {
        .Ok(parsed) => [("lines", parsed)],
        .Err(_) => [],
    };
    Generated { modules: modules, diagnostics: [], needs: [] }
}

export fn format<C: Allocator>(ctx: C, request: FormatRequest): Result<Doc, [Diagnostic]> {
    .Err([])
}
"#;

/// One language with two contracts on one tool is refused, across `check`
/// and `generate` too.
#[test]
fn check_and_generate_share_a_language_s_contract() {
    let scratch = Scratch::repo("check-and-generate");
    scratch.write(
        "tools/b/BUILD.buri",
        "tool {\n    check {\n        accepts: [\n            { language: \"json\", type_schema: \"a.schema.json\" },\n        ]\n    }\n\n    generate {\n        accepts: [\n            { language: \"json\", type_schema: \"b.schema.json\" },\n        ]\n    }\n}\n",
    );
    scratch.write("tools/b/a.schema.json", NAMES_SCHEMA);
    scratch.write("tools/b/b.schema.json", NAMES_SCHEMA);
    scratch.write("tools/b/tool.buri", NAMES_TOOL);
    scratch
        .run(&["build", "//tools/b"])
        .exits(2)
        .says("error: `json` has two contracts on this tool [accepts-duplicate-language]\n  --> tools/b/BUILD.buri:10:25");
}

/// A repository language can't name the in-tree `json` tool as its
/// `generate`: it makes types for the JSON languages alone. It's refused where
/// it's written, and its `check` and `format` still work without it.
#[test]
fn a_language_generated_by_the_json_tool_is_refused_where_it_is_written() {
    let scratch = Scratch::repo("json-generates-a-language");
    scratch.write(
        "REPO.buri",
        "language {\n    name: \"conf\"\n    extensions: [\".conf\"]\n    check: \"json\"\n    format: \"json\"\n    generate: \"json\"\n}\n",
    );
    scratch.write(
        "tools/b/BUILD.buri",
        "tool {\n    generate {\n        accepts: [\n            { language: \"conf\", type_schema: \"b.schema.json\" },\n        ]\n    }\n}\n",
    );
    scratch.write("tools/b/b.schema.json", NAMES_SCHEMA);
    scratch.write("tools/b/tool.buri", TOOL);
    scratch.write("lib/x/BUILD.buri", "library {\n    generators: [\n        { tool: \"//tools/b\", inputs: [\"x.conf\"] },\n    ]\n}\n");
    scratch.write("lib/x/x.conf", "{\"names\":   [\"a\"]}\n");
    scratch.write("lib/x/lib.buri", "export let one: Int = 1;\n");
    scratch
        .run(&["build", "//tools/b"])
        .exits(2)
        .says("error: `json` makes types for the JSON languages alone, so it can't be `conf`'s `generate` [json-generate-unsupported]\n --> REPO.buri:6:15")
        .says("= fix: give `.conf` to `json`, `jsonc` or `json5` instead, or name a tool of your own whose `generate` answers `typesOf`")
        .silent_about("has no program");
    scratch.write(
        "REPO.buri",
        "language {\n    name: \"conf\"\n    extensions: [\".conf\"]\n    check: \"json\"\n    format: \"json\"\n}\n",
    );
    scratch.write("tools/b/BUILD.buri", "tool {\n    generate {}\n}\n");
    scratch
        .run(&["format", "--check"])
        .exits(1)
        .says("lib/x/x.conf");
}
