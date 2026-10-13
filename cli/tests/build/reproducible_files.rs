//! `--check-reproducible` over rounds that write different sets of files: more
//! lazy chunks in one round, a stylesheet in one and a chunk in the other, and
//! codegen units under other names around the same code.
//!
//! As in `check_reproducible`, the generator's tool runs under `BURI_JS`, and a
//! stand-in runtime made of shell builtins answers each request differently.
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

const MAIN: &str = r#"from "HOST_MODULE" import { HOST };
from "platform/effect" import { Allocator, StdoutEFFECTS };
from "//lib/text" import { run };

export fn main(host: HOST): Result<(), Str> {
    let ctx = context {
        Allocator: host.alloc,
        Stdout: host.stdout,
        CONTEXT
    };
    let _ = run(ctx);
    .Ok(())
}
"#;

/// Request `n` lazily loads `n` functions, so each round writes more chunks.
const GROWING_CHUNKS: &str = r#"#!/bin/sh
while read -r line; do
  n=0
  [ -f COUNT ] && read -r n < COUNT
  n=$((n + 1))
  echo $n > COUNT
  fns=""
  calls=""
  i=1
  while [ $i -le $n ]; do
    fns="${fns}fn say$i<C: Allocator + Stdout>(ctx: C, n: Int): () { io.println(ctx, \\\"$i\\\").ignore() }\\n"
    calls="${calls}let f$i = lazy.load(say$i); let _ = f$i(ctx, 1); "
    i=$((i + 1))
  done
  printf '%s\n' "{\"modules\":[{\"name\":\"text\",\"text\":\"from \\\"core/io\\\" import * as io;\\nfrom \\\"core/lazy\\\" import * as lazy;\\nfrom \\\"platform/effect\\\" import { Allocator, Stdout };\\n${fns}export fn run<C: Allocator + Stdout>(ctx: C): () { ${calls}() }\\n\",\"anchors\":[]}],\"diagnostics\":[],\"needs\":[]}"
done
"#;

/// The first four requests, which the first round's are among, answer a styled
/// page; the rest answer one that lazily loads a function.
const STYLE_THEN_CHUNK: &str = r#"#!/bin/sh
while read -r line; do
  n=0
  [ -f COUNT ] && read -r n < COUNT
  n=$((n + 1))
  echo $n > COUNT
  if [ $n -le 4 ]; then
    printf '%s\n' '{"modules":[{"name":"text","text":"from \"ui/node\" import * as ui;\nfrom \"platform/effect\" import { Allocator, Stdout, Ui, Watch };\nexport fn run<C: Allocator + Stdout + Ui + Watch>(ctx: C): () { ui.mount(ctx, ui.stack({ styles: [.Gap(.Rem(0.5))], children: [] }), []).ignore() }\n","anchors":[]}],"diagnostics":[],"needs":[]}'
  else
    printf '%s\n' '{"modules":[{"name":"text","text":"from \"core/io\" import * as io;\nfrom \"core/lazy\" import * as lazy;\nfrom \"platform/effect\" import { Allocator, Stdout, Ui, Watch };\nfn say<C: Allocator + Stdout + Ui + Watch>(ctx: C, n: Int): () { io.println(ctx, \"a\").ignore() }\nexport fn run<C: Allocator + Stdout + Ui + Watch>(ctx: C): () { let f = lazy.load(say); f(ctx, 1) }\n","anchors":[]}],"diagnostics":[],"needs":[]}'
  fi
done
"#;

/// Request `n` answers the same code from a module named `m<n>`.
const RENAMED_MODULE: &str = r#"#!/bin/sh
while read -r line; do
  n=0
  [ -f COUNT ] && read -r n < COUNT
  n=$((n + 1))
  echo $n > COUNT
  printf '%s\n' "{\"modules\":[{\"name\":\"text\",\"text\":\"from \\\"//lib/text/m$n\\\" export { run };\\n\",\"anchors\":[]},{\"name\":\"m$n\",\"text\":\"from \\\"core/io\\\" import * as io;\\nfrom \\\"platform/effect\\\" import { Allocator, Stdout };\\nexport fn run<C: Allocator + Stdout>(ctx: C): () { io.println(ctx, \\\"a\\\").ignore() }\\n\",\"anchors\":[]}],\"diagnostics\":[],\"needs\":[]}"
done
"#;

/// A repository whose `//cmd/app` runs what `//tools/gen` generated into
/// `//lib/text`, for `output`, with the runtime `script` answering the tool.
/// Answers the repository and the path to point `BURI_JS` at.
fn repository(name: &str, output: &str, script: &str) -> (Scratch, String) {
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
        "from \"//lib/text/text\" export { run };\n",
    );
    let (module, host, effects, context) = match output {
        "web" => (
            "web",
            "WebHost",
            ", Ui, Watch",
            "Ui: host.ui,\n        Watch: host.watch,",
        ),
        "node" => ("node", "NodeHost", "", ""),
        _ => ("native", "NativeHost", "", ""),
    };
    let declared = match output {
        "native" => format!(
            "{{ platform: \"native\", variant: \"{}\" }}",
            host_variant()
        ),
        other => format!("{{ platform: \"{other}\" }}"),
    };
    scratch.write(
        "cmd/app/BUILD.buri",
        &format!("binary {{\n    dependencies: [\"//lib/text\"]\n    outputs: [{declared}]\n}}\n"),
    );
    scratch.write(
        "cmd/app/main.buri",
        &MAIN
            .replace("HOST_MODULE", module)
            .replace("HOST", host)
            .replace("EFFECTS", effects)
            .replace("CONTEXT", context),
    );
    let count = scratch.path("count").display().to_string();
    let js = scratch.write_executable("fake/js", &script.replace("COUNT", &count));
    (scratch, js.display().to_string())
}

/// A round that writes more files than the other is reported by the count.
#[cfg(unix)]
#[test]
fn a_round_with_more_chunks_is_reported_by_its_file_count() {
    let (scratch, js) = repository("reproducible-more-chunks", "node", GROWING_CHUNKS);
    scratch
        .run_with_env(
            &["build", "--check-reproducible", "//cmd/app"],
            &[("BURI_JS", &js)],
        )
        .exits(1)
        .says("error: //cmd/app is not reproducible")
        .says("two builds of the same tree wrote ")
        .says(" files and ");
}

/// A round that writes a stylesheet where the other writes a chunk is
/// reported by the two names.
#[cfg(unix)]
#[test]
fn a_stylesheet_in_one_round_and_a_chunk_in_the_other_are_named() {
    let (scratch, js) = repository("reproducible-style-or-chunk", "web", STYLE_THEN_CHUNK);
    scratch
        .run_with_env(
            &["build", "--check-reproducible", "//cmd/app"],
            &[("BURI_JS", &js)],
        )
        .exits(1)
        .says("error: //cmd/app is not reproducible")
        .says("two builds of the same tree wrote `main.css` and `main.0.mjs`");
}

/// The same code under a renamed module changes a codegen unit's name, and
/// the check blames the toolchain rather than the linker.
#[cfg(unix)]
#[test]
fn a_renamed_unit_is_reported_and_blamed_on_the_toolchain() {
    let (scratch, js) = repository("reproducible-renamed-unit", "native", RENAMED_MODULE);
    scratch
        .run_with_env(
            &["build", "--check-reproducible", "//cmd/app"],
            &[("BURI_JS", &js)],
        )
        .exits(1)
        .says("error: //cmd/app is not reproducible")
        .says("two builds of the same tree named unit `__lib_text_m")
        .says("this is a toolchain bug, not a problem with your repository");
}
