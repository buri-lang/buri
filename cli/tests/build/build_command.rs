//! What `buri build` itself answers: `--check-reproducible` with nowhere to
//! write its rounds, beside a broken build file, on a library alone and over a
//! linker that writes something new each time, and two pages built in one run.
use crate::harness::*;

const NATIVE_MAIN: &str = r#"from "core/io" import * as io;
from "native" import { NativeHost };
from "platform/effect" import { Allocator, Stdout };

export fn main(host: NativeHost): Result<(), Str> {
    let ctx = context {
        Allocator: host.alloc,
        Stdout: host.stdout,
    };
    io.println(ctx, "native").mapErr(fn(_e) => "could not write to standard output")
}
"#;

const NODE_MAIN: &str = r#"from "node" import { NodeHost };

export fn main(host: NodeHost): Result<(), Str> {
    .Ok(())
}
"#;

const PAGE_MAIN: &str = r#"from "platform/effect" import { Allocator, Ui };
from "ui/node" import * as ui;
from "web" import { WebHost };

export fn main(host: WebHost): Result<(), Str> {
    let ctx = context { Allocator: host.alloc, Ui: host.ui };
    ui.mount(ctx, ui.text({ content: .Const("hi") }), [])
}
"#;

/// A repository with one binary at `cmd/app` for `platform`, and a library.
fn repository(name: &str, platform: &str, main: &str) -> Scratch {
    let scratch = Scratch::repo(name);
    scratch.write(
        "cmd/app/BUILD.buri",
        &format!("binary {{\n    outputs: [{{ platform: \"{platform}\" }}]\n}}\n"),
    );
    scratch.write("cmd/app/main.buri", main);
    scratch.write("lib/one/BUILD.buri", "library {}\n");
    scratch.write("lib/one/lib.buri", "export fn one(): Int {\n    1\n}\n");
    scratch
}

/// The rounds are written under the temporary directory; when that is a file,
/// a native round says it can't write its executable there.
#[test]
fn a_native_round_with_nowhere_to_link_is_an_error() {
    let scratch = repository("build-command-native-tmp", "native", NATIVE_MAIN);
    scratch.write("not-a-directory", "a file\n");
    let tmp = scratch.path("not-a-directory");
    scratch
        .run_with_env(&["build", "--check-reproducible", "//cmd/app"], &[("TMPDIR", tmp.to_str().unwrap())])
        .exits(2)
        .says("error: cannot write ")
        .says("/cmd/app/app");
}

/// A broken build file anywhere in the repository stops each round before it
/// builds, as it stops `buri build`.
#[test]
fn check_reproducible_reports_a_broken_build_file() {
    let scratch = repository("build-command-broken", "node", NODE_MAIN);
    scratch.write("lib/bad/BUILD.buri", "library {\n  nonsense: 1\n}\n");
    scratch
        .run(&["build", "--check-reproducible", "//cmd/app"])
        .exits(2)
        .says("unknown field `nonsense` in a `library` rule");
}

/// A library has no artifact to build twice: without `--verbose` the check
/// says nothing at all.
#[test]
fn check_reproducible_on_a_library_is_quiet() {
    let scratch = repository("build-command-library", "node", NODE_MAIN);
    let run = scratch.run(&["build", "--check-reproducible", "//lib/one"]);
    run.ok();
    assert_eq!(run.all(), "", "a library has nothing to compare");
}

/// Two pages build in one run, each beside the platform's page.
#[test]
fn two_pages_build_in_one_run() {
    let scratch = Scratch::repo("build-command-pages");
    for name in ["a", "b"] {
        scratch.write(&format!("cmd/{name}/BUILD.buri"), "binary {\n    outputs: [{ platform: \"web\" }]\n}\n");
        scratch.write(&format!("cmd/{name}/main.buri"), PAGE_MAIN);
    }
    scratch.run(&["build", "//cmd/a", "//cmd/b"]).ok().says(".buri/out/web/cmd/a/main.mjs").says(".buri/out/web/cmd/b/main.mjs");
    assert_eq!(scratch.read(".buri/out/web/cmd/a/index.html"), scratch.read(".buri/out/web/cmd/b/index.html"));
}

/// A C driver that links through the real one, answers no dry run, and then
/// appends how many times it has run to what it wrote.
#[cfg(target_os = "macos")]
const COUNTING_DRIVER: &str = r#"#!/bin/sh
for a in "$@"; do [ "$a" = "-###" ] && { echo 'clang: error: no dry run here' >&2; exit 0; }; done
"$REAL_CC" "$@" || exit $?
out=
prev=
for a in "$@"; do [ "$prev" = "-o" ] && out=$a; prev=$a; done
n=$(cat "$FAKE_COUNT" 2>/dev/null || echo 0)
n=$((n + 1))
echo $n > "$FAKE_COUNT"
[ -n "$out" ] && printf 'link %s' "$n" >> "$out"
exit 0
"#;

/// Two links that write different executables from the same objects are
/// reported, naming the executable and pointing at the objects above it.
///
/// macOS only: a Linux link of a musl runtime goes through a musl driver, not
/// the `CC` given.
#[cfg(target_os = "macos")]
#[test]
fn an_executable_that_differs_between_two_links_is_not_reproducible() {
    let scratch = repository("build-command-counting-linker", "native", NATIVE_MAIN);
    let driver = scratch.write_executable("fake/cc", COUNTING_DRIVER).display().to_string();
    let real_cc = std::env::var("CC").unwrap_or_else(|_| String::from("cc"));
    let real =
        std::process::Command::new("sh").args(["-c", &format!("command -v '{real_cc}'")]).output().unwrap();
    let real = String::from_utf8_lossy(&real.stdout).trim().to_string();
    let count = scratch.path("count").display().to_string();
    let home = scratch.path("home").display().to_string();
    let env = [("CC", driver.as_str()), ("REAL_CC", real.as_str()), ("FAKE_COUNT", count.as_str()), ("BURI_HOME", home.as_str())];
    scratch
        .run_with_env(&["build", "--check-reproducible", "//cmd/app"], &env)
        .exits(1)
        .says("error: //cmd/app is not reproducible")
        .says("/cmd/app/app differs between two builds of the same tree, first at byte ")
        .says("the objects above say whether this is a codegen difference or a link one");
}
