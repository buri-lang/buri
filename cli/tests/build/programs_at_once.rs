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

/// Three JavaScript programs of the same size.
fn three_alike(name: &str) -> Scratch {
    let scratch = Scratch::repo(name);
    for name in ["a", "b", "c"] {
        binary(&scratch, name, "node", NODE_MAIN);
    }
    scratch
}

/// What `BURI_PROFILE` says of the programs held at once: the most there
/// were, the most they were expected to hold, and the budget.
fn expected(printed: &str) -> (usize, u64, u64) {
    let line = printed
        .lines()
        .map(str::trim)
        .find_map(|l| l.strip_prefix("programs held at once "))
        .unwrap_or_else(|| panic!("no profile line in\n{printed}"));
    let words: Vec<&str> = line.split([' ', ',']).filter(|w| !w.is_empty()).collect();
    match words.as_slice() {
        [programs, "expected", bytes, "of", budget, "bytes"] => (
            programs.parse().unwrap(),
            bytes.parse().unwrap(),
            budget.parse().unwrap(),
        ),
        _ => panic!("unexpected profile line `{line}`"),
    }
}

/// What one of [`three_alike`]'s programs is expected to hold in a release
/// build, from a build of them one at a time.
fn one_program() -> u64 {
    let scratch = three_alike("programs-at-once-one");
    let run = scratch.run_with_env(
        &["build", "--release", "//..."],
        &[("BURI_THREADS", "1"), ("BURI_PROFILE", "1")],
    );
    run.ok();
    let (programs, bytes, _) = expected(&run.stderr);
    assert_eq!(programs, 1);
    bytes
}

/// Release programs that fit the memory between them build at once: two
/// held in their front ends together, on a machine with room for two.
#[test]
fn programs_that_fit_the_memory_build_at_once() {
    let one = one_program();
    let scratch = three_alike("programs-at-once-room");
    let held = Held::new("programs-at-once-room");
    // The budget is half the memory.
    held.set("memory", 1, &(4 * one).to_string());
    held.hold("building", 1);
    held.hold("building", 2);
    let mut child = held.start_with_env(
        &scratch,
        &["build", "--release", "//..."],
        &[("BURI_THREADS", "3"), ("BURI_PROFILE", "1")],
    );
    let first = held.reached("building", 1);
    let second = held.reached("building", 2);
    first.go();
    second.go();
    assert!(child.wait().unwrap().success(), "{}", held.printed());
    assert_eq!(expected(&held.printed()), (2, 2 * one, 2 * one));
}

/// A release program that would not fit beside the one being built waits
/// for it, however many workers are free.
#[test]
fn a_program_that_does_not_fit_waits() {
    let one = one_program();
    let scratch = three_alike("programs-at-once-full");
    let held = Held::new("programs-at-once-full");
    held.set("memory", 1, &(2 * one + 1).to_string());
    let mut child = held.start_with_env(
        &scratch,
        &["build", "--release", "//..."],
        &[("BURI_THREADS", "3"), ("BURI_PROFILE", "1")],
    );
    assert!(child.wait().unwrap().success(), "{}", held.printed());
    assert_eq!(expected(&held.printed()), (1, one, one));
}

/// Modules in each of [`many_units`]'s programs: a codegen unit each.
const MODULES: usize = 12;

/// Four native programs of [`MODULES`] modules each, every module a function
/// `main` adds up.
fn many_units(name: &str) -> Scratch {
    let scratch = Scratch::repo(name);
    for program in ["a", "b", "c", "d"] {
        let mut main = String::from(
            "from \"core/io\" import * as io;\nfrom \"native\" import { NativeHost };\n\
             from \"platform/effect\" import { Allocator, Stdout };\n",
        );
        let mut sources = Vec::new();
        for i in 0..MODULES {
            scratch.write(
                &format!("cmd/{program}/m{i}.buri"),
                &format!(
                    "export fn run{i}(n: Int): Int {{\n    if (n <= 0) {{ {i} }} else {{ run{i}(n - 1) }}\n}}\n"
                ),
            );
            main.push_str(&format!(
                "from \"//cmd/{program}/m{i}.buri\" import {{ run{i} }};\n"
            ));
            sources.push(format!("\"m{i}.buri\""));
        }
        let total: Vec<String> = (0..MODULES).map(|i| format!("run{i}(2)")).collect();
        main.push_str(&format!(
            "\nexport fn main(host: NativeHost): Result<(), Str> {{\n    \
             let ctx = context {{ Allocator: host.alloc, Stdout: host.stdout }};\n    \
             io.println(ctx, \"${{{}}}\").mapErr(fn(_e) => \"no stdout\")\n}}\n",
            total.join(" + ")
        ));
        scratch.write(&format!("cmd/{program}/main.buri"), &main);
        scratch.write(
            &format!("cmd/{program}/BUILD.buri"),
            &format!(
                "binary {{\n    sources: [{}]\n    outputs: [{{ platform: \"native\" }}]\n}}\n",
                sources.join(", ")
            ),
        );
    }
    scratch
}

/// Release programs built side by side optimize no more units at once than
/// there are cores between them, so a build of many programs holds a core's
/// worth of LLVM modules rather than a core's worth for each program. All
/// four programs start together, each with more units than cores, and each
/// still builds and runs. A toolchain without the optimizing backend refuses
/// them.
#[test]
fn programs_built_side_by_side_optimize_a_unit_per_core() {
    let scratch = many_units("programs-at-once-units");
    let first = scratch.run(&["build", "--release", "//cmd/a"]);
    if first.stderr.contains("built without `backend-llvm`") {
        first.exits(1);
        return;
    }
    first.ok();
    std::fs::remove_dir_all(scratch.path(".buri")).unwrap();
    let held = Held::new("programs-at-once-units");
    for n in 1..=4 {
        held.hold("building", n);
    }
    let mut child = held.start_with_env(
        &scratch,
        &["build", "--release", "//..."],
        &[("BURI_THREADS", "4"), ("BURI_PROFILE", "1")],
    );
    let holding: Vec<_> = (1..=4).map(|n| held.reached("building", n)).collect();
    for h in holding {
        h.go();
    }
    let status = child.wait().unwrap();
    let printed = held.printed();
    assert!(status.success(), "{printed}");
    let units: usize = printed
        .lines()
        .map(str::trim)
        .find_map(|l| l.strip_prefix("units optimized at once "))
        .unwrap_or_else(|| panic!("no profile line in\n{printed}"))
        .parse()
        .unwrap();
    assert!(
        (1..=4).contains(&units),
        "{units} units at once:\n{printed}"
    );
    let want = format!("{}\n", (0..MODULES).sum::<usize>());
    for program in ["a", "b", "c", "d"] {
        let artifact = std::fs::read_dir(scratch.path(".buri/out/native"))
            .unwrap()
            .map(|variant| {
                variant
                    .unwrap()
                    .path()
                    .join(format!("cmd/{program}/{program}"))
            })
            .find(|path| path.exists())
            .unwrap_or_else(|| panic!("no artifact for {program}:\n{printed}"));
        let ran = std::process::Command::new(&artifact).output().unwrap();
        assert_eq!(String::from_utf8_lossy(&ran.stdout), want, "{program}");
    }
}
