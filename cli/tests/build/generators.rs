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
//! LLVM instead of quietly passing.
//!
//! The third row is about the tool rather than the module. The generator this
//! toolchain ships is a Buri program compiled to an `.mjs` the first time a
//! build needs one; the claim is that a repository pays for that **once**, not
//! once per target, per platform, or per build.
//!
//! ```text
//! cargo test -p buri --test build generators::
//! ```

use crate::harness::{ci, Run, Scratch};

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

const SCHEMA: &str = "edition = \"2026\";\n\npackage wire.v1;\n\nmessage Point {\n  int32 x = 1;\n  int32 y = 2;\n}\n";

const LIBRARY: &str = "library {\n    generators: [\n        { tool: \"std/codegen/proto\", inputs: [\"point.proto\"] },\n    ]\n\n    visibility: [\"//visibility:public\"]\n}\n";

const SURFACE: &str =
    "from \"//lib/wire/point.proto\" export { defaultPoint, encodePoint, Point };\n";

/// A program whose whole answer comes out of the generated module: the codec
/// encodes, and the bytes are the wire format's. A program that merely named a
/// generated type would link the same way and prove less.
const PROGRAM: &str = r#"from "core/bytes" import * as bytes;
from "core/effect" import { Alloc, Stdout };
from "core/host" import * as host;
from "core/io" import * as io;
from "//lib/wire" import { defaultPoint, encodePoint, Point };

export fn main(): Result<(), Str> {
    let ctx = context {
        Alloc: host.alloc,
        Stdout: host.stdout,
    };
    let p = Point { ..defaultPoint(), x: .Some(1), y: .Some(300) };
    io
        .println(ctx, bytes.toHex(ctx, encodePoint(ctx, p)))
        .mapErr(fn(_e) => "could not write to standard output")
}
"#;

/// The repository every row here runs in.
fn repository(name: &str) -> Scratch {
    let scratch = Scratch::repo(name);
    scratch.write("lib/wire/BUILD.buri", LIBRARY);
    scratch.write("lib/wire/point.proto", SCHEMA);
    scratch.write("lib/wire/lib.buri", SURFACE);
    scratch.write(
        "cmd/point/BUILD.buri",
        &format!(
            "binary {{\n    dependencies: [\"//lib/wire\"]\n\n    outputs: [{{ platform: {} }}]\n}}\n",
            host_platform()
        ),
    );
    scratch.write("cmd/point/main.buri", PROGRAM);
    scratch
}

/// Field 1 varint 1, field 2 varint 300 — what the schema says the two fields
/// encode to, and a number no part of this test could produce by accident.
const ENCODED: &str = "080110ac02";

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
/// two-sided: after building two targets, on two platforms, twice over, that
/// directory holds exactly one file — and the second build does not write it
/// again.
///
/// What it costs is the whole reason to care. A compile of the generator is a
/// compile of `core/codegen`, `core/buri/ast` and both halves of the schema
/// reader, and it happens before the first schema is read. Paying it per target
/// would put it in front of every package in a repository.
#[test]
fn the_toolchain_generator_is_compiled_once_per_repository() {
    let scratch = repository("generators-toolchain");
    // A second package reading the same library, so the build has two targets
    // whose closure needs the generator.
    scratch.write(
        "cmd/twice/BUILD.buri",
        "binary {\n    dependencies: [\"//lib/wire\"]\n\n    outputs: [{ platform: JS }, { platform: WEB }]\n}\n",
    );
    scratch.write("cmd/twice/main.buri", PROGRAM);

    scratch.run(&["build", "//cmd/twice"]).ok();
    let after_first = toolchain_artifacts(&scratch);
    assert_eq!(
        after_first.len(),
        1,
        "the toolchain generator was compiled {} times for one repository: {after_first:?}",
        after_first.len()
    );

    // A second build, of a second target, on a third platform. Same file, same
    // bytes, no second compile.
    let before = std::fs::metadata(&after_first[0]).expect("the generator's module").modified().ok();
    scratch.run(&["build", "//..."]).ok();
    let after_second = toolchain_artifacts(&scratch);
    assert_eq!(
        after_second, after_first,
        "a second build compiled the toolchain generator again"
    );
    let after = std::fs::metadata(&after_second[0]).expect("the generator's module").modified().ok();
    assert_eq!(before, after, "the generator's module was rewritten by a build that had one");
}

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
