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
//! The last two are about what an *input* may be, and are here rather than in
//! `repositories/generators/` because neither fixture is text a corpus could
//! hold: a schema that is not UTF-8, and one a megabyte long — past any pipe
//! buffer, which is what the request's own thread is for.
//!
//! ```text
//! cargo test -p buri --test build generators::
//! ```

use crate::harness::{ci, tests_dir, Run, Scratch};

/// The platform a binary here declares.
///
/// Named rather than defaulted, for `heap.rs`'s reason: a binary with no
/// outputs builds for JavaScript, and a JavaScript artifact would take the
/// generated module nowhere near a backend or a linker.
fn host_platform() -> &'static str {
    if cfg!(target_os = "macos") {
        "MACOS"
    } else {
        "LINUX"
    }
}

const LIBRARY: &str = "library {\n    generators: [\n        { tool: \"std/codegen/proto\", inputs: [\"address.proto\", \"demo.proto\"] },\n    ]\n\n    visibility: [\"//visibility:public\"]\n}\n";

const SURFACE: &str = "from \"//lib/proto/demo.proto\" export {\n    decodeEverything, defaultEverything, encodeEverything, Everything, Shade,\n};\n";

/// A program whose whole answer comes out of the generated module: the codec
/// encodes, the bytes are the wire format's, and the decoder reads them back
/// into the value they came from. A program that merely named a generated type
/// would link the same way and prove less.
const PROGRAM: &str = r#"from "core/bytes" import * as bytes;
from "core/effect" import { Alloc, Stdout };
from "core/host" import * as host;
from "core/io" import * as io;
from "//lib/proto" import {
    decodeEverything, defaultEverything, encodeEverything, Everything, Shade,
};

export fn main(): Result<(), Str> {
    let ctx = context {
        Alloc: host.alloc,
        Stdout: host.stdout,
    };
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
    !run.all().contains("native-artifact-not-available")
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
        ci::skipped(
            "build::generators",
            &format!(
                "this toolchain builds no native artifact for {}, so nothing linked:\n{}",
                host_platform(),
                run.all()
            ),
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
    run.says("native-artifact-not-available");
}

/// The generator the toolchain ships is compiled **once per repository**.
///
/// `std/codegen/proto` is a Buri program, and the build compiles it to an
/// `.mjs` under `.buri/out/toolchain/` the first time anything needs a schema
/// read. The file's name is its action key, so the claim this row holds is
/// two-sided: after two builds, of two targets, across three platforms, that
/// directory holds exactly one file — and the second build did not write it
/// again.
///
/// What it costs is the whole reason to care. A compile of the generator is a
/// compile of `core/codegen`, `core/buri/ast` and both halves of the schema
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
        "binary {\n    dependencies: [\"//lib/proto\"]\n\n    outputs: [{ platform: JS }, { platform: WEB }]\n}\n",
    );
    scratch.write("cmd/twice/main.buri", PROGRAM);

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
         generators: [{ tool: \"std/codegen/proto\", inputs: [\"point.proto\"] }]\n}\n",
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

/// **A request larger than a pipe holds crosses it whole.**
///
/// The build writes the request on one thread and drains both of the tool's
/// streams on two more, because a pipe holds a page or two: a request bigger
/// than that blocks the write, and a build waiting for an exit that the block
/// prevents is two processes waiting on each other. A megabyte is far past any
/// platform's buffer, so this is the row that fails if the feeding thread is
/// ever folded back into the wait.
///
/// The generator answers with the input's own length, so a request that arrived
/// truncated is a wrong number rather than a hang.
#[test]
fn an_input_larger_than_a_pipe_crosses_it_whole() {
    let scratch = Scratch::repo("generators-large-input");
    scratch.write(
        "lib/wire/BUILD.buri",
        "library {\n    generators: [{ tool: \"//cmd/gen\", inputs: [\"big.txt\"] }]\n\n    \
         visibility: [\"//visibility:public\"]\n}\n",
    );
    // One megabyte, which no pipe buffer on either platform holds.
    const SIZE: usize = 1_000_000;
    scratch.write("lib/wire/big.txt", &"x".repeat(SIZE));
    scratch.write("lib/wire/lib.buri", "from \"//lib/wire/units\" export { size };\n");
    scratch.write("cmd/gen/BUILD.buri", "binary {\n    outputs: [{ platform: JS }]\n}\n");
    scratch.write("cmd/gen/main.buri", MEASURING_GENERATOR);
    scratch.write(
        "cmd/app/BUILD.buri",
        "binary {\n    dependencies: [\"//lib/wire\"]\n\n    outputs: [{ platform: JS }]\n}\n",
    );
    scratch.write(
        "cmd/app/main.buri",
        "from \"core/effect\" import { Alloc, Stdout };\n\
         from \"core/host\" import * as host;\n\
         from \"core/io\" import * as io;\n\
         from \"//lib/wire\" import { size };\n\n\
         export fn main(): Result<(), Str> {\n  \
         let ctx = context { Alloc: host.alloc, Stdout: host.stdout };\n  \
         let _ = io.println(ctx, \"size=${size}\").ignore();\n  \
         .Ok(())\n\
         }\n",
    );

    scratch.run(&["run", "//cmd/app"]).ok().says(&format!("size={SIZE}"));
}

/// A generator that answers with the number of bytes it was handed.
const MEASURING_GENERATOR: &str = r#"from "core/effect" import { Alloc, Stdin, Stdout };
from "core/host" import * as host;
from "core/io" import * as io;
from "core/json" import * as json;
from "core/json" import { Json };
from "core/list" import * as list;
from "core/str" import * as str;

export fn main(): Result<(), Str> {
  let ctx = context { Alloc: host.alloc, Stdin: host.stdin, Stdout: host.stdout };
  let line = io.readLine(ctx).okOr("no request")?;
  let request = json.parse(ctx, line).mapErr(fn(_e) => "the request is not JSON")?;
  let text = firstInput(request).withDefault("");
  let source = str.format(ctx, "export let size: Int = ${text.len()};\n");
  let unit: Json = .Object([
    ("name", .Str("units")),
    ("text", .Str(source)),
    ("anchors", .Array(list.empty())),
  ]);
  let response: Json = .Object([
    ("modules", .Array([unit])),
    ("diagnostics", .Array(list.empty())),
  ]);
  let _ = io.println(ctx, "${json.stringify(ctx, response)}").ignore();
  .Ok(())
}

fn firstInput(request: Json): Option<Str> {
  let inputs = match (request) {
    .Object(fields) => fields.find(fn(f) => f.0 == "inputs").map(fn(f) => f.1),
    _ => .None,
  };
  let items = match (inputs.withDefault(.Null)) {
    .Array(xs) => xs,
    _ => list.empty(),
  };
  let pair = match (items.get(0).withDefault(.Null)) {
    .Array(xs) => xs,
    _ => list.empty(),
  };
  match (pair.get(1).withDefault(.Null)) {
    .Str(s) => .Some(s),
    _ => .None,
  }
}
"#;

/// Every `.mjs` under `.buri/out/toolchain/`, sorted.
fn toolchain_artifacts(scratch: &Scratch) -> Vec<std::path::PathBuf> {
    let dir = scratch.path(".buri/out/toolchain");
    let Ok(entries) = std::fs::read_dir(&dir) else { return Vec::new() };
    let mut out: Vec<std::path::PathBuf> = entries
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "mjs"))
        .collect();
    out.sort();
    out
}
