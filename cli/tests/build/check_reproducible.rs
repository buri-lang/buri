//! `--check-reproducible` over a generator whose answers change from one
//! request to the next, a target edited between the two rounds, and a target
//! the build itself refuses.
//!
//! The generator's tool runs under `BURI_JS`, so a stand-in runtime that
//! answers each request with something new is what makes two rounds disagree.
//! It uses shell builtins alone, because a tool's process gets an empty
//! environment and no `PATH`.
use crate::harness::*;

/// The platform a native binary here declares.
fn host_variant() -> String {
    let os = if cfg!(target_os = "macos") {
        "macos"
    } else {
        "linux"
    };
    let arch = if cfg!(target_arch = "aarch64") {
        "arm64"
    } else {
        "x86_64"
    };
    format!("{os}-{arch}")
}

const TOOL: &str = r#"from "core/tool" import { Generated, GenerateRequest };
from "platform/effect" import { Allocator };

export fn generate<C: Allocator>(ctx: C, request: GenerateRequest<Str>): Generated {
    Generated { modules: [], diagnostics: [], needs: [] }
}
"#;

const MAIN: &str = r#"from "core/io" import * as io;
from "HOST_MODULE" import { HOST };
from "platform/effect" import { Allocator, Stdout };
from "//lib/text" import { text };

export fn main(host: HOST): Result<(), Str> {
    let ctx = context {
        Allocator: host.alloc,
        Stdout: host.stdout,
    };
    io.println(ctx, text()).mapErr(fn(_e) => "could not write to standard output")
}
"#;

/// Answers request `n` with a module `text` reaching `n` generated modules,
/// each calling the one before it, the first returning `"n"`. So each round
/// asks for, and compiles, more modules than the round before.
const GROWING_RUNTIME: &str = r#"#!/bin/sh
while read -r line; do
  n=0
  [ -f COUNT ] && read -r n < COUNT
  n=$((n + 1))
  echo $n > COUNT
  modules="{\"name\":\"m1\",\"text\":\"export fn m1(): Str { \\\"$n\\\" }\\n\",\"anchors\":[]}"
  i=2
  while [ $i -le $n ]; do
    j=$((i - 1))
    modules="${modules},{\"name\":\"m$i\",\"text\":\"from \\\"//lib/text/m$j\\\" import { m$j };\\nexport fn m$i(): Str { m$j() }\\n\",\"anchors\":[]}"
    i=$((i + 1))
  done
  printf '%s\n' "{\"modules\":[{\"name\":\"text\",\"text\":\"from \\\"//lib/text/m$n\\\" import { m$n };\\nexport fn text(): Str { m$n() }\\n\",\"anchors\":[]},${modules}],\"diagnostics\":[],\"needs\":[]}"
done
"#;

/// A repository whose `//cmd/app` prints what `//tools/gen` generated into
/// `//lib/text`, for `platform` (`node` or `native`).
fn repository(name: &str, platform: &str) -> Scratch {
    let scratch = Scratch::repo(name);
    scratch.write("tools/gen/BUILD.buri", "tool {\n    generate {}\n}\n");
    scratch.write("tools/gen/tool.buri", TOOL);
    scratch.write(
        "lib/text/BUILD.buri",
        "library {\n    generators: [\n        { tool: \"//tools/gen\", inputs: [\"a.txt\"] },\n    ]\n    \
         visibility: [\"//visibility:public\"]\n}\n",
    );
    scratch.write("lib/text/a.txt", "a\n");
    scratch.write(
        "lib/text/lib.buri",
        "from \"//lib/text/text\" export { text };\n",
    );
    let (output, module, host) = match platform {
        "native" => (
            format!(
                "{{ platform: \"native\", variant: \"{}\" }}",
                host_variant()
            ),
            "native",
            "NativeHost",
        ),
        _ => (String::from("{ platform: \"node\" }"), "node", "NodeHost"),
    };
    scratch.write(
        "cmd/app/BUILD.buri",
        &format!("binary {{\n    dependencies: [\"//lib/text\"]\n    outputs: [{output}]\n}}\n"),
    );
    scratch.write(
        "cmd/app/main.buri",
        &MAIN.replace("HOST_MODULE", module).replace("HOST", host),
    );
    scratch
}

/// The same repository with `//lib/text` written by hand, so it builds under
/// any JavaScript runtime.
fn written(name: &str, platform: &str) -> Scratch {
    let scratch = repository(name, platform);
    scratch.write(
        "lib/text/BUILD.buri",
        "library {\n    sources: [\"text.buri\"]\n    visibility: [\"//visibility:public\"]\n}\n",
    );
    scratch.write("lib/text/text.buri", "export fn text(): Str {\n    \"a\"\n}\n");
    scratch.write("lib/text/lib.buri", "from \"//lib/text/text.buri\" export { text };\n");
    scratch
}

/// Writes `script` as the JavaScript runtime, with `COUNT` naming a file in
/// the repository, and answers the path to point `BURI_JS` at.
fn runtime(scratch: &Scratch, script: &str) -> String {
    let count = scratch.path("count").display().to_string();
    scratch
        .write_executable("fake/js", &script.replace("COUNT", &count))
        .display()
        .to_string()
}

/// A JavaScript artifact that differs between the rounds names the file.
#[cfg(unix)]
#[test]
fn a_generated_module_that_differs_between_rounds_makes_the_script_differ() {
    let scratch = repository("check-reproducible-growing-js", "node");
    let js = runtime(&scratch, GROWING_RUNTIME);
    scratch
        .run_with_env(&["build", "--check-reproducible", "//cmd/app"], &[("BURI_JS", &js)])
        .exits(1)
        .says("error: //cmd/app is not reproducible")
        .says(".buri/out/node/cmd/app/app.mjs differs between two builds of the same tree, first at byte ");
}

/// A native round compiles more units than the round before, under other
/// names, and each difference is reported.
#[cfg(unix)]
#[test]
fn a_generated_module_that_differs_between_rounds_changes_the_codegen_units() {
    let scratch = repository("check-reproducible-growing-native", "native");
    let js = runtime(&scratch, GROWING_RUNTIME);
    scratch
        .run_with_env(
            &["build", "--check-reproducible", "//cmd/app"],
            &[("BURI_JS", &js)],
        )
        .exits(1)
        .says("error: //cmd/app is not reproducible")
        .says("codegen units")
        .says("two builds of the same tree named unit `__lib_text_m");
}

/// A native artifact that is reproducible is checked in silence without
/// `--verbose`.
#[test]
fn a_reproducible_native_artifact_is_quiet() {
    let scratch = written("check-reproducible-quiet-native", "native");
    let run = scratch.run(&["build", "--check-reproducible", "//cmd/app"]);
    run.ok();
    assert_eq!(run.all().trim(), "", "a passing check printed something");
}

/// A target that stops being a binary between the rounds is reported, not
/// built.
#[cfg(unix)]
#[test]
fn a_target_edited_between_rounds_is_reported() {
    let scratch = repository("check-reproducible-vanishing", "node");
    let build = scratch.path("cmd/app/BUILD.buri").display().to_string();
    let script = r#"#!/bin/sh
while read -r line; do
  printf 'library {\n    dependencies: ["//lib/text"]\n}\n' > BUILD
  printf '%s\n' '{"modules":[{"name":"text","text":"export fn text(): Str { \"a\" }\n","anchors":[]}],"diagnostics":[],"needs":[]}'
done
"#;
    let js = runtime(&scratch, &script.replace("BUILD", &build));
    scratch
        .run_with_env(
            &["build", "--check-reproducible", "//cmd/app"],
            &[("BURI_JS", &js)],
        )
        .exits(2)
        .says("error: //cmd/app disappeared between two builds of one tree");
}

/// A target the build refuses for its policy is refused by the check too,
/// rather than called reproducible.
#[test]
fn a_target_the_build_refuses_is_not_reproducible() {
    let scratch = written("check-reproducible-private", "node");
    scratch.edit(
        "lib/text/BUILD.buri",
        "visibility: [\"//visibility:public\"]",
        "",
    );
    scratch
        .run(&["build", "//cmd/app"])
        .exits(1)
        .says("[visibility-violation]");
    scratch
        .run(&["build", "--check-reproducible", "--verbose", "//cmd/app"])
        .exits(1)
        .says("[visibility-violation]")
        .silent_about("is reproducible");
}
