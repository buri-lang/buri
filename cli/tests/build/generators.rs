//! **A generated module on the host's own backend, and the tool that writes
//! it.**
//!
//! `repositories/proto/every_platform` asks whether a `.proto` module compiles
//! for every platform an output can name. It asks with `lint`, because a native
//! artifact links only on a host of its own platform and that case has to mean
//! the same thing on both CI hosts. What it therefore cannot ask is the half
//! that matters most: **does a generated module survive code generation and the
//! linker** — the stencil backend on a default toolchain, LLVM under
//! `--release` — and does the linked program print the right answer.
//!
//! So that is what is here, in the shape `heap.rs` uses for the same reason: a
//! scratch repository, the host's platform named rather than defaulted, and two
//! arms on the release row so the test means something on a toolchain without
//! LLVM instead of quietly passing. The schema is the conformance corpus's own,
//! which is one message of every field kind editions has, so what the linker
//! gets is the whole of what the `.proto` generator can write.
//!
//! The third row is about the tool rather than the module. The generator this
//! toolchain ships is a Buri program compiled to an `.mjs` the first time a
//! build needs one; the claim is that a repository pays for that **once**, not
//! once per target, per platform, or per build.
//!
//! The last three are here rather than in `repositories/generators/` because
//! none of their fixtures is something a corpus could hold: a schema that is
//! not UTF-8; a megabyte through both of the tool's pipes, past any
//! platform's buffer; and the printer's own text compared byte for byte with
//! the file beside it.
//!
//! ```text
//! cargo test -p buri --test build generators::
//! ```

use crate::harness::{ci, indent, tests_dir, Run, Scratch};

/// The platform a binary here declares.
///
/// Named rather than defaulted, for `heap.rs`'s reason: a binary with no
/// outputs builds for JavaScript, and a JavaScript artifact would take the
/// generated module nowhere near a backend or a linker.
fn host_platform() -> String {
    let os = if cfg!(target_os = "macos") { "macos" } else { "linux" };
    let arch = if cfg!(target_arch = "aarch64") { "arm64" } else { "x86_64" };
    format!("\"native\", variant: \"{os}-{arch}\"")
}

const LIBRARY: &str = "library {\n    generators: [\n        { tool: \"proto\", inputs: [\"address.proto\", \"demo.proto\"] },\n    ]\n\n    visibility: [\"//visibility:public\"]\n}\n";

const SURFACE: &str = "from \"//lib/proto/demo.proto\" export {\n    decodeEverything, defaultEverything, encodeEverything, Everything, Shade,\n};\n";

/// A program whose whole answer comes out of the generated module: the codec
/// encodes, the bytes are the wire format's, and the decoder reads them back
/// into the value they came from. A program that merely named a generated type
/// would link the same way and prove less.
///
/// Its entry is `native`'s; [`TWICE`] is the same program entered from node
/// and from a page, each through a function of its own.
const PROGRAM: &str = r#"from "core/bytes" import * as bytes;
from "core/io" import * as io;
from "native" import { NativeHost };
from "platform/effect" import { Allocator, Stdout };
from "//lib/proto" import {
    decodeEverything, defaultEverything, encodeEverything, Everything, Shade,
};

export fn main(host: NativeHost): Result<(), Str> {
    run(context { Allocator: host.alloc, Stdout: host.stdout })
}

fn run<C: Allocator + Stdout>(ctx: C): Result<(), Str> {
    let v = Everything {
        ..defaultEverything(),
        count: .Some(300),
        shade: .Some(Shade.DARK),
        scores: [1, 2, 3],
    };
    let wire = encodeEverything(ctx, v);
    let back = decodeEverything(ctx, wire) == .Ok(v);
    io
        .println(ctx, "${bytes.toHex(ctx, wire)} ${back}")
        .mapErr(fn(_e) => "could not write to standard output")
}
"#;

/// [`PROGRAM`] for a node output entering at `mainForNode` and a page entering
/// at `mainForWeb`.
fn twice() -> String {
    PROGRAM
        .replace(
            "from \"native\" import { NativeHost };",
            "from \"node\" import { NodeHost };\nfrom \"web\" import { WebHost };",
        )
        .replace(
            "export fn main(host: NativeHost): Result<(), Str> {\n    run(context { Allocator: host.alloc, Stdout: host.stdout })\n}",
            "export fn mainForNode(host: NodeHost): Result<(), Str> {\n    run(context { Allocator: host.alloc, Stdout: host.stdout })\n}\n\n\
             export fn mainForWeb(host: WebHost): Result<(), Str> {\n    run(context { Allocator: host.alloc, Stdout: host.stdout })\n}",
        )
}

/// The repository every row here runs in.
///
/// **The schemas are `cli/tests/conformance/lib/proto`'s own**, copied rather
/// than written again. That corpus is one message of every field kind editions
/// has — nested messages, an enum, a oneof, repeated packed and expanded, both
/// presences, a field per scalar type, a name that is a Buri keyword, and a
/// second schema it imports across a file boundary — and `language::conformance`
/// and `native::conformance` both assert what its codecs compute. What is left
/// over is exactly what this file is for: does all of that survive a *linker*.
fn repository(name: &str) -> Scratch {
    let scratch = Scratch::repo(name);
    let corpus = tests_dir().join("conformance/lib/proto");
    scratch.write("lib/proto/BUILD.buri", LIBRARY);
    for schema in ["address.proto", "demo.proto"] {
        let text = std::fs::read_to_string(corpus.join(schema)).expect("the conformance schema");
        scratch.write(&format!("lib/proto/{schema}"), &text);
    }
    scratch.write("lib/proto/lib.buri", SURFACE);
    scratch.write(
        "cmd/point/BUILD.buri",
        &format!(
            "binary {{\n    dependencies: [\"//lib/proto\"]\n\n    outputs: [{{ platform: {} }}]\n}}\n",
            host_platform()
        ),
    );
    scratch.write("cmd/point/main.buri", PROGRAM);
    scratch
}

/// Field 2 varint 300 (`10 ac 02`), field 16 varint 2 (`80 01 02`), and field
/// 30 packed with three varints (`f2 01 03 01 02 03`) — checked by hand against
/// the protobuf encoding rules, and `true` for the decode that read them back.
///
/// Three fields rather than twenty-five because the rest of the message is
/// unset, and a field holding no value writes no bytes: what the other twenty-
/// two are here for is the code the backend has to compile, not the bytes.
const ENCODED: &str = "10ac02800102f20103010203 true";

fn ran_natively(run: &Run) -> bool {
    !run.all().contains("native-artifact-unavailable")
}

/// A generated module goes through the host's native backend and its linker,
/// and the program that comes out prints what the schema says.
///
/// **The row `every_platform` cannot write.** That case's `LINUX` and `MACOS`
/// outputs are linted, never linked, because only one of the two is the host on
/// any given run. This one names whichever platform the host is, so the module
/// a tool wrote reaches a code generator and a linker on every machine the
/// suite runs on.
#[test]
fn a_generated_module_links_into_the_hosts_native_artifact() {
    let scratch = repository("generators-native");
    let run = scratch.run(&["run", "//cmd/point"]);
    if !ran_natively(&run) {
        ci::native_program_failed(
            "build::generators",
            &format!("the native build for {} was refused", host_platform()),
            &run.all(),
        );
        return;
    }
    run.ok().says(ENCODED);
}

/// The same through the optimizing pipeline, or a refusal that says why.
///
/// **Two arms and no skip**, for `heap::a_release_artifact_is_asked_for_its_*`'s
/// reason. `--release` routes to LLVM (`backend::select`), which a default
/// toolchain is built without — so this row cannot assert the linked answer
/// unconditionally, and it must not quietly pass on the toolchain that has no
/// LLVM either. Under `--features backend-llvm` it is the proposal's claim in
/// full: a generated module is a module the release backend compiles and the
/// linker links, and the program it produced ran.
#[test]
fn a_generated_module_links_into_a_release_artifact_or_is_refused_by_name() {
    let scratch = repository("generators-release");
    let run = scratch.run(&["run", "//cmd/point", "--release"]);
    if ran_natively(&run) {
        run.ok().says(ENCODED);
        return;
    }
    assert_ne!(
        run.code, 0,
        "a release artifact ran and printed nothing the schema decided:\n{}",
        run.all()
    );
    run.says("native-artifact-unavailable");
}

/// The generator the toolchain ships is compiled **once per repository**.
///
/// The `proto` tool is a Buri program, and the build compiles it to an
/// `.mjs` under `.buri/out/tools/` the first time anything needs a schema
/// read. The file's name is its action key, so the claim this row holds is
/// two-sided: after two builds, of two targets, across three platforms, that
/// directory holds exactly one file — and the second build did not write it
/// again.
///
/// What it costs is the whole reason to care. A compile of the generator is a
/// compile of `core/tool`, `core/buri/ast` and both halves of the schema
/// reader, and it happens before the first schema is read. Paying it per target
/// would put it in front of every package in a repository.
#[test]
fn the_toolchain_generator_is_compiled_once_per_repository() {
    let scratch = repository("generators-toolchain");
    // A second package reading the same library, on two platforms of its own.
    // Nothing native here: whether this host links is a different question, and
    // this row has to mean the same thing on every machine.
    scratch.write(
        "cmd/twice/BUILD.buri",
        "binary {\n    dependencies: [\"//lib/proto\"]\n\n    outputs: [\n        { platform: \"node\", entries: [{ name: \"main\", function: \"mainForNode\" }] },\n        { platform: \"web\", entries: [{ name: \"main\", function: \"mainForWeb\" }] },\n    ]\n}\n",
    );
    scratch.write("cmd/twice/main.buri", &twice());

    scratch.run(&["build", "//lib/proto"]).ok();
    let after_first = toolchain_artifacts(&scratch);
    assert_eq!(
        after_first.len(),
        1,
        "the toolchain generator was compiled {} times for one repository: {after_first:?}",
        after_first.len()
    );

    // A second build, of a second target, on two more platforms. Same file,
    // same bytes, no second compile.
    let before = std::fs::metadata(&after_first[0]).expect("the generator's module").modified().ok();
    scratch.run(&["build", "//cmd/twice"]).ok();
    let after_second = toolchain_artifacts(&scratch);
    assert_eq!(after_second, after_first, "a second build compiled the toolchain generator again");
    let after = std::fs::metadata(&after_second[0]).expect("the generator's module").modified().ok();
    assert_eq!(before, after, "the generator's module was rewritten by a build that had one");
}

// ---------------------------------------------------------------------------
// What an input may be
// ---------------------------------------------------------------------------

/// **A file that is there is never reported as absent**, and the sentence is
/// the one a `sources` entry gets about the same bytes.
///
/// A schema saved in UTF-16 is the shape somebody actually meets. Reading it
/// used to answer `None` the way a missing file does, so the report said
/// `gone.proto does not exist` and offered "create the file" about a file the
/// author could see in the directory the caret named.
///
/// Rust rather than a repository case because the fixture is bytes that are not
/// text, and nothing else in `cli/tests/repositories/` is.
#[test]
fn an_input_that_is_not_text_is_reported_as_unreadable_rather_than_absent() {
    let scratch = Scratch::repo("generators-not-utf8");
    scratch.write(
        "lib/wire/BUILD.buri",
        "library {\n    sources: [\"beside.buri\"]\n\n    \
         generators: [{ tool: \"proto\", inputs: [\"point.proto\"] }]\n}\n",
    );
    scratch.write("lib/wire/lib.buri", "export fn here(): Int { 1 }\n");
    scratch.write("lib/wire/beside.buri", "export fn beside(): Int { 2 }\n");
    std::fs::write(scratch.path("lib/wire/point.proto"), b"edition = \"2026\";\n\xff\xfe\n")
        .expect("a schema that is not UTF-8");

    let run = scratch.run(&["build", "//lib/wire"]);
    run.exits(1)
        .says("cannot read lib/wire/point.proto")
        .says("check the file exists and is readable");
    assert!(
        !run.all().contains("does not exist"),
        "a file that is there was reported as absent:\n{}",
        run.all()
    );

    // The same bytes under `sources`, which is the wording this one now
    // matches. A generator's input and a rule's source are the same question
    // about the same file, and two answers to it would be two bugs to fix.
    std::fs::write(scratch.path("lib/wire/beside.buri"), b"export fn beside(): Int { \xff\xfe }\n")
        .expect("a source that is not UTF-8");
    scratch.write(
        "lib/wire/BUILD.buri",
        "library {\n    sources: [\"beside.buri\"]\n}\n",
    );
    scratch
        .run(&["build", "//lib/wire"])
        .exits(1)
        .says("cannot read lib/wire/beside.buri")
        .says("check the file exists and is readable");
}

/// **Both pipes carry more than a pipe holds.**
///
/// The build writes the request on one thread and drains the tool's streams on
/// others, because a pipe holds a page or two: a stream bigger than that
/// blocks whoever is writing it, and a build waiting for an exit that the block
/// prevents is two processes waiting on each other with nothing to end it. So
/// the tool here is handed a megabyte and answers with a module holding all of
/// it — far past any platform's buffer, both ways.
///
/// The generator answers with the input's own length and with the input itself,
/// so a stream that arrived truncated is a wrong number rather than a hang.
#[test]
fn an_input_larger_than_a_pipe_crosses_it_whole() {
    let scratch = Scratch::repo("generators-large-input");
    scratch.write(
        "lib/wire/BUILD.buri",
        "library {\n    generators: [{ tool: \"//tool/gen\", inputs: [\"big.txt\"] }]\n\n    \
         visibility: [\"//visibility:public\"]\n}\n",
    );
    // One megabyte, which no pipe buffer on either platform holds.
    const SIZE: usize = 1_000_000;
    scratch.write("lib/wire/big.txt", &"x".repeat(SIZE));
    scratch.write(
        "lib/wire/lib.buri",
        "from \"//lib/wire/units\" export { echoed, size };\n",
    );
    scratch.write("tool/gen/BUILD.buri", "tool {\n    generate {}\n}\n");
    scratch.write("tool/gen/tool.buri", MEASURING_GENERATOR);
    scratch.write(
        "cmd/app/BUILD.buri",
        "binary {\n    dependencies: [\"//lib/wire\"]\n\n    outputs: [{ platform: \"node\" }]\n}\n",
    );
    scratch.write(
        "cmd/app/main.buri",
        "from \"platform/effect\" import { Allocator, Stdout };\n\
         from \"node\" import { NodeHost };\n\
         from \"core/io\" import * as io;\n\
         from \"//lib/wire\" import { echoed, size };\n\n\
         export fn main(host: NodeHost): Result<(), Str> {\n  \
         let ctx = context { Allocator: host.alloc, Stdout: host.stdout };\n  \
         let _ = io.println(ctx, \"size=${size} echoed=${echoed.length()}\").ignore();\n  \
         .Ok(())\n\
         }\n",
    );

    scratch.run(&["run", "//cmd/app"]).ok().says(&format!("size={SIZE} echoed={SIZE}"));
}

/// A generator that answers with the bytes it was handed and how many there
/// were.
const MEASURING_GENERATOR: &str = r#"from "core/buri/ast" import * as ast;
from "platform/effect" import { Allocator };
from "core/str" import * as str;
from "core/tool" import { Generated, GenerateRequest };

export fn generate<C: Allocator>(ctx: C, request: GenerateRequest<Str>): Generated {
  let text = request.inputs.get(0).map(fn(i) => i.value).withDefault("");
  let source = str.format(
    ctx,
    "export let size: Int = ${text.length()};\nexport let echoed: Str = \"${text}\";\n",
  );
  let modules = match (ast.parse(ctx, "", source)) {
    .Ok(parsed) => [("units", parsed)],
    .Err(_) => [],
  };
  Generated { modules: modules, diagnostics: [], needs: [] }
}
"#;

// ---------------------------------------------------------------------------
// What the printer wrote
// ---------------------------------------------------------------------------

/// **The printer wrote exactly the file a person would have written.**
///
/// `repositories/generators/the_printer_round_trips` runs the same function
/// twice — printed out of a `core/buri/ast` tree, and hand-written beside it —
/// and asserts they compute the same answers. That is the claim that matters,
/// and it is blind to layout: a printer that emitted every declaration on one
/// line would still pass it.
///
/// This is the other half, and it is one comparison. `lib/wire/twin.buri` is a
/// source of this repository, so `language::corpus::…_is_formatted` holds it to
/// what `buri format` writes; asserting the tool's module text equals it byte
/// for byte therefore says **`print` writes source the formatter leaves
/// alone**, which is `core/buri/ast`'s own promise and had nothing behind it.
///
/// The fixture is where it is because a repository case cannot ask this: the
/// generated text is never a file, so there is nothing for a `file` step to
/// name.
#[test]
fn the_printers_text_is_the_file_beside_it_byte_for_byte() {
    let fixture = tests_dir().join("repositories/generators/the_printer_round_trips/repo");
    let scratch = Scratch::copy_of("generators-printed-text", &fixture);
    scratch.run(&["build", "//lib/wire"]).ok();

    let tool = toolchain_artifacts(&scratch);
    let [tool] = tool.as_slice() else { panic!("one tool was compiled: {tool:?}") };
    let line = buri::build::generators::run_artifact(tool, r#"{"entry":"generate","inputs":[],"files":[]}"#)
        .expect("the generator answers");
    let response = buri::build::generators::Response::decode(&line).expect("an answer");
    let printed = &response.modules.first().expect("one module").text;
    let twin = std::fs::read_to_string(fixture.join("lib/wire/twin.buri")).expect("the twin");
    assert_eq!(
        printed, &twin,
        "`print` and `buri format` disagree about the same module; the first \
         difference is at byte {:?}",
        printed.bytes().zip(twin.bytes()).position(|(a, b)| a != b)
    );
}

/// Every `.mjs` under `.buri/out/tools/`, sorted.
fn toolchain_artifacts(scratch: &Scratch) -> Vec<std::path::PathBuf> {
    let dir = scratch.path(".buri/out/tools");
    let Ok(entries) = std::fs::read_dir(&dir) else { return Vec::new() };
    let mut out: Vec<std::path::PathBuf> = entries
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "mjs"))
        .collect();
    out.sort();
    out
}

// ---------------------------------------------------------------------------
// How many processes a pass starts
// ---------------------------------------------------------------------------

/// A generator that writes one module per input: `name value` becomes the
/// module `name`, exporting `name` as a hundred divided by `value`. A value of
/// 0 stops the tool with a division by zero.
const DIVIDING_TOOL: &str = r#"from "core/buri/ast" import * as ast;
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

const SUMMING_PROGRAM: &str = r#"from "core/io" import * as io;
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

/// A repository whose one rule runs one tool twice, once per `generators`
/// entry, and a program printing what the two modules hold between them.
fn two_entries(name: &str) -> Scratch {
    let scratch = Scratch::repo(name);
    scratch.write("tool/gen/BUILD.buri", "tool {\n    generate {}\n}\n");
    scratch.write("tool/gen/tool.buri", DIVIDING_TOOL);
    scratch.write(
        "lib/wire/BUILD.buri",
        "library {\n    generators: [\n        { tool: \"//tool/gen\", inputs: [\"alpha.txt\"] },\n        { tool: \"//tool/gen\", inputs: [\"beta.txt\"] },\n    ]\n\n    visibility: [\"//visibility:public\"]\n}\n",
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
    scratch.write("cmd/app/main.buri", SUMMING_PROGRAM);
    scratch
}

/// The JavaScript runtime's absolute path. A stand-in runs with an empty
/// environment, so it can't look the runtime up itself.
fn js_path() -> String {
    let found = std::process::Command::new("/bin/sh")
        .arg("-c")
        .arg(format!("command -v {}", crate::harness::js_runtime()))
        .output()
        .expect("sh runs");
    String::from_utf8_lossy(&found.stdout).trim().to_string()
}

/// A script standing in for the JavaScript runtime that notes each tool it is
/// started on, one line per start, in `starts.txt`, and answers its path for
/// `BURI_JS`. It runs with an empty environment, so every path is absolute.
fn counting_runtime(scratch: &Scratch) -> String {
    let js = js_path();
    let log = scratch.path("starts.txt");
    let path = scratch.write(
        "runtime.sh",
        &format!(
            "#!/bin/sh\ncase \"$1\" in\n*/.buri/out/tools/*) echo \"$1\" >> '{}' ;;\nesac\nexec '{js}' \"$@\"\n",
            log.display()
        ),
    );
    std::process::Command::new("/bin/chmod").arg("+x").arg(&path).status().expect("chmod runs");
    path.display().to_string()
}

/// How many times a tool process was started.
fn tool_starts(scratch: &Scratch) -> usize {
    std::fs::read_to_string(scratch.path("starts.txt")).map(|t| t.lines().count()).unwrap_or(0)
}

/// **One tool process answers every request a pass makes of it in turn.**
///
/// Starting the JavaScript runtime and loading a tool costs more than most
/// requests do, and a schema's checks and its rule's `generate` are one
/// request after another to the same program. Both entries here run in the
/// same pass, one after the other, so the second is asked of the process that
/// answered the first.
#[test]
fn one_tool_process_answers_both_entries_of_a_rule() {
    let scratch = two_entries("generators-one-process");
    let runtime = counting_runtime(&scratch);
    let run = scratch.run_with_env(&["run", "//cmd/app"], &[("BURI_JS", &runtime)]);
    run.ok();
    assert_eq!(run.stdout.trim(), "150", "the two generated modules did not hold 100 and 50:\n{}", indent(&run.all()));
    assert_eq!(tool_starts(&scratch), 1, "two requests of one tool started more than one process");
}

/// **A request that stops a kept process is reported as if it had a process of
/// its own.** The second entry divides by zero after the first was answered by
/// the same process: the note is the tool's exit and what it said on standard
/// error, and the first entry's module is still there to import.
#[test]
fn a_tool_that_stops_partway_through_a_pass_is_reported_in_its_own_words() {
    let scratch = two_entries("generators-kept-process-stops");
    let runtime = counting_runtime(&scratch);
    scratch.write("lib/wire/beta.txt", "beta 0\n");
    let run = scratch.run_with_env(&["build", "//cmd/app"], &[("BURI_JS", &runtime)]);
    run.exits(1);
    let want = "error: `//tool/gen` did not answer [tool-failed]\n --> lib/wire/BUILD.buri:4:9\n  |\n4 |         { tool: \"//tool/gen\", inputs: [\"beta.txt\"] },\n  |         ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^\n  |\n  = the tool exited with 1\n    division by zero\n";
    assert!(run.stderr.contains(want), "the stopped tool was not reported in its own words:\n{}", indent(&run.all()));
    assert!(
        !run.stderr.contains("\"//lib/wire/alpha\" names no file"),
        "the entry answered before the stop lost its module:\n{}",
        indent(&run.all())
    );
}

/// A repository of `rules` libraries, each with one rule asking `//tool/gen`
/// for one module. Each rule is in a package of its own, so they all run in
/// the same round, side by side, and most start a tool process of their own.
fn many_rules(name: &str, rules: usize) -> Scratch {
    let scratch = Scratch::repo(name);
    scratch.write("tool/gen/BUILD.buri", "tool {\n    generate {}\n}\n");
    scratch.write("tool/gen/tool.buri", DIVIDING_TOOL);
    for i in 0..rules {
        scratch.write(
            &format!("lib/w{i}/BUILD.buri"),
            "library {\n    generators: [\n        { tool: \"//tool/gen\", inputs: [\"alpha.txt\"] },\n    ]\n}\n",
        );
        scratch.write(&format!("lib/w{i}/alpha.txt"), &format!("alpha {}\n", i + 1));
        scratch.write(&format!("lib/w{i}/lib.buri"), &format!("from \"//lib/w{i}/alpha\" export {{ alpha }};\n"));
    }
    scratch
}

/// A script standing in for the JavaScript runtime that notes, for each tool
/// it's started on, the descriptors it was started with, one line per start in
/// `descriptors.txt`. Started with `--descriptors`, it prints the line instead
/// and runs nothing.
fn descriptor_noting_runtime(scratch: &Scratch) -> String {
    let js = js_path();
    let log = scratch.path("descriptors.txt");
    let path = scratch.write(
        "runtime.sh",
        &format!(
            "#!/bin/sh\ncase \"$1\" in\n--descriptors) echo $(ls /dev/fd/) ; exit 0 ;;\n*/.buri/out/tools/*) echo $(ls /dev/fd/) >> '{}' ;;\nesac\nexec '{js}' \"$@\"\n",
            log.display()
        ),
    );
    std::process::Command::new("/bin/chmod").arg("+x").arg(&path).status().expect("chmod runs");
    path.display().to_string()
}

/// **A tool's process holds its own three streams and nothing of another's.**
///
/// A tool process answers requests until its input ends, and the build keeps
/// one for a pass's next request. A tool started while another's pipes were
/// being made could inherit them. Holding the write end of a kept process's
/// input, it kept that process from ever reading the end, and the build
/// waited on it forever: a cold `buri test` hung in its generators for 21
/// minutes. Many rules side by side start many tools at once, so their starts
/// overlap, and each build here runs every one again. What each was started
/// with is compared with what the stand-in sees when it's started alone.
#[test]
fn a_tool_process_inherits_no_other_process_pipe() {
    const RULES: usize = 96;
    const BUILDS: usize = 5;
    let scratch = many_rules("generators-inherit-nothing", RULES);
    let runtime = descriptor_noting_runtime(&scratch);
    let alone = std::process::Command::new(&runtime)
        .arg("--descriptors")
        .env_clear()
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .output()
        .expect("the stand-in runs");
    let alone: Vec<String> = String::from_utf8_lossy(&alone.stdout).split_whitespace().map(str::to_string).collect();
    for _ in 0..BUILDS {
        scratch.run_with_env(&["build", "--force", "//..."], &[("BURI_JS", &runtime)]).ok();
    }
    let noted = std::fs::read_to_string(scratch.path("descriptors.txt")).unwrap_or_default();
    // A process a rule let go of may answer a rule that starts after it, so
    // a build starts at most one per rule, and at least one.
    let starts = noted.lines().count();
    assert!((BUILDS..=RULES * BUILDS).contains(&starts), "{starts} tools were started in {BUILDS} builds");
    let extra: Vec<&str> =
        noted.lines().filter(|line| line.split_whitespace().any(|fd| !alone.iter().any(|a| a == fd))).collect();
    assert!(extra.is_empty(), "a tool was started holding more than {alone:?}:\n{}", indent(&extra.join("\n")));
}
