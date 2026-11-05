//! `buri build` builds a repository's programs side by side, and reports as if
//! it had built them one at a time: the same lines, in target order, and the
//! same artifacts.
use crate::harness::*;
use crate::held::Held;
use std::collections::BTreeMap;
use std::path::Path;

const NODE_MAIN: &str = r#"from "node" import { NodeHost };

export fn main(host: NodeHost): Result<(), Str> {
    .Ok(())
}
"#;

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

/// A binary at `cmd/<name>` for `platform`.
fn binary(scratch: &Scratch, name: &str, platform: &str, main: &str) {
    scratch.write(
        &format!("cmd/{name}/BUILD.buri"),
        &format!("binary {{\n    outputs: [{{ platform: \"{platform}\" }}]\n}}\n"),
    );
    scratch.write(&format!("cmd/{name}/main.buri"), main);
}

/// Binaries and libraries in label order: one binary that doesn't type check
/// and one library that doesn't, a native binary, and JavaScript ones around
/// them.
fn repository(name: &str) -> Scratch {
    let scratch = Scratch::repo(name);
    binary(&scratch, "a", "node", NODE_MAIN);
    binary(
        &scratch,
        "b",
        "node",
        "from \"node\" import { NodeHost };\n\nexport fn main(host: NodeHost): Result<(), Str> {\n    let n: Int = \"two\";\n    .Ok(())\n}\n",
    );
    binary(&scratch, "c", "native", NATIVE_MAIN);
    binary(&scratch, "d", "node", NODE_MAIN);
    scratch.write("lib/broken/BUILD.buri", "library {}\n");
    scratch.write(
        "lib/broken/lib.buri",
        "export fn one(): Int {\n    \"one\"\n}\n",
    );
    scratch.write("lib/fine/BUILD.buri", "library {}\n");
    scratch.write("lib/fine/lib.buri", "export fn one(): Int {\n    1\n}\n");
    binary(&scratch, "z", "node", NODE_MAIN);
    scratch
}

/// Every file under `dir`, by its path below it.
pub(crate) fn files(dir: &Path) -> BTreeMap<String, Vec<u8>> {
    let mut found = BTreeMap::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(at) = stack.pop() {
        for entry in std::fs::read_dir(&at).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                stack.push(path);
            } else {
                let rel = path.strip_prefix(dir).unwrap().display().to_string();
                found.insert(rel, std::fs::read(&path).unwrap());
            }
        }
    }
    found
}

/// Built with one worker and with four, a repository prints the same report,
/// exits the same way and leaves the same artifacts, with `out` pointing at
/// the last output that built.
#[test]
fn programs_built_side_by_side_report_as_one_at_a_time() {
    let build = |name: &str, threads: &str| {
        let scratch = repository(name);
        let run = scratch.run_with_env(
            &["build", "--explain", "//..."],
            &[("BURI_THREADS", threads)],
        );
        (scratch, run)
    };
    let (one, alone) = build("programs-at-once-one", "1");
    let (four, together) = build("programs-at-once-four", "4");
    assert_eq!(alone.code, 1, "{}", indent(&alone.all()));
    assert_eq!(together.code, alone.code);
    assert_eq!(together.stdout, alone.stdout);
    assert_eq!(together.stderr, alone.stderr);

    // Target order, whichever finished first.
    let artifacts: Vec<&str> = alone
        .stdout
        .lines()
        .filter(|l| l.ends_with(" bytes)"))
        .collect();
    assert_eq!(artifacts.len(), 4, "{}", indent(&alone.stdout));
    for (line, dir) in artifacts
        .iter()
        .zip(["cmd/a/", "cmd/c/", "cmd/d/", "cmd/z/"])
    {
        assert!(line.contains(dir), "{line} is not {dir}'s");
    }
    let mismatch = alone.stderr.find("mismatch").unwrap_or(usize::MAX);
    let broken = alone.stderr.find("lib/broken").unwrap_or(0);
    assert!(
        alone.stderr.contains("cmd/b/main.buri") && mismatch < broken,
        "the binary's error comes before the library's:\n{}",
        indent(&alone.stderr)
    );

    assert_eq!(
        files(&four.path(".buri/out")),
        files(&one.path(".buri/out"))
    );
    let link = |s: &Scratch| std::fs::read_link(s.path("out")).unwrap();
    assert_eq!(link(&four), link(&one));
    assert_eq!(link(&one), Path::new(".buri/out/node"));
}

/// Two programs are in their front ends at once: the first is held there
/// while the second reaches it.
#[test]
fn two_programs_build_at_once() {
    let scratch = Scratch::repo("programs-at-once-held");
    binary(&scratch, "a", "node", NODE_MAIN);
    binary(&scratch, "b", "node", NODE_MAIN);
    let held = Held::new("programs-at-once");
    held.hold("building", 1);
    held.hold("building", 2);
    let mut child = held.start_with_env(&scratch, &["build", "//..."], &[("BURI_THREADS", "2")]);
    let first = held.reached("building", 1);
    // Only a build that runs both programs at once gets here with the first
    // still held.
    let second = held.reached("building", 2);
    first.go();
    second.go();
    let status = child.wait().unwrap();
    assert!(status.success(), "{}", held.printed());
    let printed = held.printed();
    let a = printed.find("cmd/a/").unwrap_or(usize::MAX);
    let b = printed.find("cmd/b/").unwrap_or(0);
    assert!(a < b, "{printed}");
}
