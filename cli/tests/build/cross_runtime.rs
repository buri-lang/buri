//! When `buri build` builds the runtime for another machine: as soon as it
//! knows an output links for one, beside checking and compiling, and only
//! then. Whether the runtime was cached or built along the way, the build
//! reports the same and writes the same artifacts.
//!
//! macOS only: on Linux x86_64 the cross output is the host's own.
#![cfg(target_os = "macos")]
use crate::build_driver::STAND_IN_CARGO;
use crate::harness::*;
use crate::programs_at_once::files;

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

const BOTH: &str = "        { platform: \"native\" },\n        { platform: \"native\", variant: \"linux-x86_64\" },\n";

/// A repository with `cmd/app` built for `outputs` from `main`, and a host
/// program and a library beside it.
fn repository(name: &str, outputs: &str, main: &str) -> Scratch {
    let scratch = Scratch::repo(name);
    scratch.write(
        "cmd/app/BUILD.buri",
        &format!("binary {{\n    outputs: [\n{outputs}    ]\n}}\n"),
    );
    scratch.write("cmd/app/main.buri", main);
    scratch.write(
        "cmd/tool/BUILD.buri",
        "binary {\n    outputs: [{ platform: \"native\" }]\n}\n",
    );
    scratch.write("cmd/tool/main.buri", NATIVE_MAIN);
    scratch.write("lib/one/BUILD.buri", "library {}\n");
    scratch.write("lib/one/lib.buri", "export fn one(): Int {\n    1\n}\n");
    scratch
}

/// A cargo that writes the warm cross runtime's archive and logs each call,
/// and the variables that point `buri` at it and at an empty home under
/// `scratch`.
struct Cold {
    env: Vec<(&'static str, String)>,
}

impl Cold {
    fn new(scratch: &Scratch) -> Cold {
        crate::harness::sweep::kept::warm_cross_runtime();
        let shared = crate::harness::sweep::kept::shared_cross_home().join("cross");
        let archive = std::fs::read_dir(&shared)
            .unwrap()
            .flatten()
            .map(|e| e.path().join("libburi_rt.a"))
            .find(|p| p.is_file())
            .expect("no warm cross runtime");
        let cargo = scratch.write_executable("fake/cargo", STAND_IN_CARGO);
        let path = |p: std::path::PathBuf| p.display().to_string();
        Cold {
            env: vec![
                ("BURI_HOME", path(scratch.path("home"))),
                ("CARGO", path(cargo)),
                ("STAND_IN_LOG", path(scratch.path("cargo.log"))),
                ("STAND_IN_ARCHIVE", path(archive)),
            ],
        }
    }

    /// Without the archive, so the runtime's build fails as cargo does
    /// offline.
    fn failing(mut self) -> Cold {
        self.env.retain(|(name, _)| *name != "STAND_IN_ARCHIVE");
        self
    }

    fn run(&self, scratch: &Scratch, args: &[&str], more: &[(&str, &str)]) -> Run {
        let mut env: Vec<(&str, &str)> = self
            .env
            .iter()
            .map(|(name, value)| (*name, value.as_str()))
            .collect();
        env.extend_from_slice(more);
        scratch.run_with_env(args, &env)
    }
}

/// How many times the runtime's cargo built, from its log.
fn cargo_builds(scratch: &Scratch) -> usize {
    std::fs::read_to_string(scratch.path("cargo.log"))
        .unwrap_or_default()
        .lines()
        .filter(|l| *l == "rustc")
        .count()
}

/// What a cross runtime home holds: its finished entries, and anything else.
fn home_entries(scratch: &Scratch) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(scratch.path("home/cross"))
        .map(|d| {
            d.flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

/// The runtime for another machine starts building before the build has
/// loaded a single module, so it runs beside every program's check and
/// compile rather than in front of the link.
#[test]
fn a_cross_runtime_starts_before_any_program_is_loaded() {
    let scratch = repository("cross-early", BOTH, NATIVE_MAIN);
    let cold = Cold::new(&scratch);
    cold.run(&scratch, &["build", "//..."], &[("BURI_PROFILE", "1")])
        .ok()
        .says("cross runtime for x86_64-unknown-linux-musl started after 0 modules loaded");
    assert_eq!(cargo_builds(&scratch), 1);
}

/// A build whose cross runtime is built along the way says what one that
/// found it cached says, line for line, and writes the same artifacts.
#[test]
fn a_cross_build_reports_alike_whether_its_runtime_was_cached() {
    let built = repository("cross-alike-built", BOTH, NATIVE_MAIN);
    let cold = Cold::new(&built);
    let first = cold.run(&built, &["build", "--explain", "//..."], &[]);
    first.ok();
    assert_eq!(cargo_builds(&built), 1);
    let cached = repository("cross-alike-cached", BOTH, NATIVE_MAIN);
    let again = cold.run(&cached, &["build", "--explain", "//..."], &[]);
    assert_eq!(
        cargo_builds(&built),
        1,
        "the cached runtime was built again"
    );
    assert_eq!(again.code, first.code);
    assert_eq!(again.stdout, first.stdout);
    assert_eq!(again.stderr, first.stderr);
    assert_eq!(
        files(&cached.path(".buri/out")),
        files(&built.path(".buri/out"))
    );
}

/// A cross program that doesn't type check reports its errors as it does
/// with the runtime cached, and the runtime's build still finishes: the home
/// holds the one entry and no half-built directory.
#[test]
fn a_cross_build_that_fails_before_linking_finishes_its_runtime() {
    let broken = "export fn main(): Int {\n    \"one\"\n}\n";
    let built = repository("cross-broken-built", BOTH, broken);
    let cold = Cold::new(&built);
    let first = cold.run(&built, &["build", "//..."], &[]);
    first.exits(1).says("cmd/app/main.buri");
    assert_eq!(cargo_builds(&built), 1);
    let entries = home_entries(&built);
    assert_eq!(entries.len(), 1, "{entries:?}");
    assert!(!entries[0].starts_with('.'), "{entries:?}");

    let cached = repository("cross-broken-cached", BOTH, broken);
    let again = cold.run(&cached, &["build", "//..."], &[]);
    assert_eq!(
        cargo_builds(&built),
        1,
        "the cached runtime was built again"
    );
    assert_eq!(again.code, first.code);
    assert_eq!(again.stdout, first.stdout);
    assert_eq!(again.stderr, first.stderr);
}

/// When the runtime can't be built, a cross program is refused for that
/// alone, whatever else is wrong with it, and the host's programs still
/// build.
#[test]
fn a_cross_runtime_that_cannot_be_built_is_the_whole_answer() {
    let broken = "export fn main(): Int {\n    \"one\"\n}\n";
    let scratch = repository(
        "cross-refused",
        "        { platform: \"native\", variant: \"linux-x86_64\" },\n",
        broken,
    );
    let cold = Cold::new(&scratch).failing();
    let run = cold.run(&scratch, &["build", "//..."], &[]);
    run.exits(1)
        .says("error: could not cross-build the runtime for x86_64-unknown-linux-musl")
        .silent_about("cmd/app/main.buri")
        .says("cmd/tool/tool (");
    assert_eq!(home_entries(&scratch), Vec::<String>::new());
}

/// A build with no output for another machine never asks for its runtime.
#[test]
fn a_build_for_this_machine_alone_builds_no_cross_runtime() {
    let scratch = repository(
        "cross-none",
        "        { platform: \"native\" },\n",
        NATIVE_MAIN,
    );
    let cold = Cold::new(&scratch);
    cold.run(&scratch, &["build", "//..."], &[("BURI_PROFILE", "1")])
        .ok()
        .silent_about("cross runtime");
    assert_eq!(cargo_builds(&scratch), 0);
    assert!(!scratch.path("home/cross").exists());
}

/// Two programs for the same other machine share one build of its runtime,
/// whether it is built along the way or found cached.
#[test]
fn two_cross_programs_share_one_runtime() {
    let tool = "binary {\n    outputs: [\n        { platform: \"native\", variant: \"linux-x86_64\" },\n    ]\n}\n";
    let built = repository("cross-shared-built", BOTH, NATIVE_MAIN);
    built.write("cmd/tool/BUILD.buri", tool);
    let cold = Cold::new(&built);
    cold.run(&built, &["build", "//..."], &[]).ok();
    assert_eq!(cargo_builds(&built), 1);
    let cached = repository("cross-shared-cached", BOTH, NATIVE_MAIN);
    cached.write("cmd/tool/BUILD.buri", tool);
    cold.run(&cached, &["build", "//..."], &[]).ok();
    assert_eq!(
        cargo_builds(&built),
        1,
        "the cached runtime was built again"
    );
    for scratch in [&built, &cached] {
        for program in ["cmd/app/app", "cmd/tool/tool"] {
            let path = scratch.path(&format!(".buri/out/native/linux-x86_64/{program}"));
            assert!(path.is_file(), "{} was not built", path.display());
        }
    }
}

/// A program for another machine that its policy refuses is refused for
/// that alone, and no runtime is built for it.
#[test]
fn a_cross_program_its_policy_refuses_builds_no_runtime() {
    let scratch = repository(
        "cross-private",
        "        { platform: \"native\", variant: \"linux-x86_64\" },\n",
        NATIVE_MAIN,
    );
    scratch.write(
        "cmd/app/BUILD.buri",
        "binary {\n    dependencies: [\"//lib/one\"]\n    outputs: [\n        { platform: \"native\", variant: \"linux-x86_64\" },\n    ]\n}\n",
    );
    let cold = Cold::new(&scratch);
    cold.run(&scratch, &["build", "//..."], &[])
        .exits(1)
        .says("[visibility-violation]");
    assert_eq!(cargo_builds(&scratch), 0);
    assert!(!scratch.path("home/cross").exists());
}
