//! What `buri build` does around the compiler: checking a native artifact
//! is reproducible, refusing an output this machine can't build, writing an
//! artifact where something is in the way, a cache that rotted, the C driver
//! and linker a link runs through, and the program store macOS runs test
//! runners from.
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

/// A repository with `//cmd/app` built for `outputs` and a library beside it.
fn repository(outputs: &str, main: &str) -> Scratch {
    let scratch = Scratch::repo("build-driver");
    scratch.write(
        "cmd/app/BUILD.buri",
        &format!("binary {{\n    outputs: [\n{outputs}    ]\n}}\n"),
    );
    scratch.write("cmd/app/main.buri", main);
    scratch.write("lib/one/BUILD.buri", "library {}\n");
    scratch.write("lib/one/lib.buri", "export fn one(): Int {\n    1\n}\n");
    scratch
}

/// The only directory under `dir`: the host's variant under `.buri/out/native`.
fn only_entry(dir: &std::path::Path) -> String {
    let names: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
        .collect();
    let [name] = names.as_slice() else {
        panic!("{} holds {names:?}", dir.display())
    };
    name.clone()
}

/// A native artifact built twice compares its objects and its executable, and
/// says so under `--verbose`; a library has nothing to compare.
#[test]
fn a_native_artifact_is_checked_for_reproducibility() {
    let scratch = repository("        { platform: \"native\" },\n", NATIVE_MAIN);
    let verbose = scratch.run(&[
        "build",
        "--check-reproducible",
        "--verbose",
        "//cmd/app",
        "//lib/one",
    ]);
    verbose
        .ok()
        .says("//cmd/app is reproducible (")
        .says(" objects)");
    let quiet = scratch.run(&["build", "--check-reproducible", "//cmd/app"]);
    quiet.ok();
    assert_eq!(
        quiet.all(),
        "",
        "a reproducible artifact is checked quietly"
    );
    scratch
        .run(&["build", "--check-reproducible", "--verbose", "//lib/one"])
        .ok()
        .says("nothing to build");
}

/// A macOS variant that isn't this machine's has no native artifact here, and
/// `--check-reproducible` refuses it before building anything.
#[test]
fn check_reproducible_refuses_another_machine_s_macos_output() {
    let other = if cfg!(target_os = "macos") && cfg!(target_arch = "aarch64") {
        "x86_64"
    } else {
        "arm64"
    };
    let variant = format!("macos-{other}");
    let scratch = repository(
        &format!("        {{ platform: \"native\", variant: \"{variant}\" }},\n"),
        NATIVE_MAIN,
    );
    scratch
        .run(&["build", "--check-reproducible", "//cmd/app"])
        .exits(1)
        .says(&format!("error: no native artifact for {variant}"));
}

/// Something at the artifact's directory that isn't a directory: the build
/// says it can't write there, from the cache and when it compiles afresh.
#[test]
fn an_artifact_that_cannot_be_written_is_an_error() {
    let scratch = repository("        { platform: \"node\" },\n", NODE_MAIN);
    scratch.run(&["build", "//cmd/app"]).ok();
    std::fs::remove_dir_all(scratch.path(".buri/out/node/cmd/app")).unwrap();
    scratch.write(".buri/out/node/cmd/app", "in the way\n");
    scratch
        .run(&["build", "//cmd/app"])
        .exits(1)
        .says("cannot write ")
        .says("app.mjs");
    scratch
        .run(&["build", "--force", "//cmd/app"])
        .exits(1)
        .says("cannot write ")
        .says("app.mjs");
}

/// The same for a native executable.
#[test]
fn a_native_artifact_that_cannot_be_written_is_an_error() {
    let scratch = repository("        { platform: \"native\" },\n", NATIVE_MAIN);
    scratch.run(&["build", "//cmd/app"]).ok();
    let variant = only_entry(&scratch.path(".buri/out/native"));
    let dir = format!(".buri/out/native/{variant}/cmd/app");
    std::fs::remove_dir_all(scratch.path(&dir)).unwrap();
    scratch.write(&dir, "in the way\n");
    scratch
        .run(&["build", "//cmd/app"])
        .exits(1)
        .says("cannot write ")
        .says("/cmd/app/app");
    scratch
        .run(&["build", "--force", "//cmd/app"])
        .exits(1)
        .says("cannot write ")
        .says("/cmd/app/app");
}

/// A cache lock older than any live process's, as a build killed mid-wipe
/// leaves, is taken: another toolchain's cache is still emptied, and the build
/// compiles afresh rather than waiting on a process that is gone.
#[test]
fn a_cache_lock_a_crashed_build_left_is_taken() {
    let scratch = repository("        { platform: \"node\" },\n", NODE_MAIN);
    scratch.run(&["build", "//cmd/app"]).ok();
    scratch.write(".buri/cache/.toolchain", "another toolchain");
    let lock = scratch.write(".buri/cache/.lock", "");
    let an_hour_ago = std::time::SystemTime::now() - std::time::Duration::from_secs(3600);
    std::fs::File::options()
        .write(true)
        .open(&lock)
        .unwrap()
        .set_modified(an_hour_ago)
        .unwrap();
    scratch
        .run(&["build", "--explain", "//cmd/app"])
        .ok()
        .says("run    link //cmd/app node");
    // A build that waited the lock out goes on without taking it.
    assert!(!lock.exists(), "the stale lock is still there");
}

/// A cache lock some live build holds is waited on for ten seconds, then the
/// build goes on without it: its own entries still serve it, and another
/// toolchain's cache and the lock are left as they were.
#[test]
fn a_build_gives_up_on_a_held_cache_lock_and_goes_on() {
    let scratch = repository("        { platform: \"node\" },\n", NODE_MAIN);
    scratch.run(&["build", "//cmd/app"]).ok();
    scratch.write(".buri/cache/.toolchain", "another toolchain");
    let stray = scratch.write(".buri/cache/stray", "another toolchain's entry\n");
    let lock = scratch.write(".buri/cache/.lock", "");
    // Dated ahead, so however slowly this machine runs, the lock is never old
    // enough to be taken for a crashed build's.
    let an_hour_ahead = std::time::SystemTime::now() + std::time::Duration::from_secs(3600);
    std::fs::File::options()
        .write(true)
        .open(&lock)
        .unwrap()
        .set_modified(an_hour_ahead)
        .unwrap();
    scratch
        .run(&["build", "--explain", "//cmd/app"])
        .ok()
        .says("cached link //cmd/app node");
    assert!(lock.exists(), "the build took a lock another build holds");
    assert!(
        stray.exists(),
        "the build emptied the cache without the lock"
    );
}

/// Every entry in the cache cut to half its length, as a crash or a full disk
/// can leave it: the next build rebuilds what it needs, and the program runs.
#[test]
fn a_truncated_cache_is_rebuilt_without_a_clean() {
    let scratch = repository("        { platform: \"native\" },\n", NATIVE_MAIN);
    scratch.run(&["build", "//cmd/app"]).ok();
    let entries = cache_entries(&scratch);
    assert!(!entries.is_empty(), "the build cached nothing");
    for entry in &entries {
        let file = std::fs::File::options().write(true).open(entry).unwrap();
        file.set_len(file.metadata().unwrap().len() / 2).unwrap();
    }
    scratch.run(&["build", "//cmd/app"]).ok();
    scratch.run(&["run", "//cmd/app"]).ok().says("native");
}

/// Every object file in the cache overwritten with zeros, its length kept:
/// a build after an edit compiles those units again rather than linking the
/// zeros.
#[test]
fn a_cached_object_overwritten_in_place_is_compiled_again() {
    let scratch = repository("        { platform: \"native\" },\n", NATIVE_MAIN);
    scratch.run(&["build", "//cmd/app"]).ok();
    let mut zeroed = 0;
    for entry in cache_entries(&scratch) {
        let bytes = std::fs::read(&entry).unwrap();
        if is_relocatable_object(&bytes) {
            std::fs::write(&entry, vec![0; bytes.len()]).unwrap();
            zeroed += 1;
        }
    }
    assert!(zeroed > 0, "the cache holds no object files");
    scratch.edit("cmd/app/main.buri", "ctx, \"native\"", "ctx, \"edited\"");
    scratch.run(&["build", "//cmd/app"]).ok();
    scratch.run(&["run", "//cmd/app"]).ok().says("edited");
}

/// The files under `.buri/cache`'s two-character directories, which is where
/// its entries are.
fn cache_entries(scratch: &Scratch) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    for dir in std::fs::read_dir(scratch.path(".buri/cache"))
        .unwrap()
        .flatten()
    {
        let Ok(entries) = std::fs::read_dir(dir.path()) else {
            continue;
        };
        out.extend(entries.flatten().map(|e| e.path()).filter(|p| p.is_file()));
    }
    out
}

/// A Mach-O or ELF object file, not yet linked.
fn is_relocatable_object(bytes: &[u8]) -> bool {
    let at = |range: std::ops::Range<usize>| bytes.get(range).unwrap_or_default();
    (at(0..4) == [0xcf, 0xfa, 0xed, 0xfe] && at(12..16) == [1, 0, 0, 0])
        || (at(0..4) == *b"\x7fELF" && at(16..18) == [1, 0])
}

/// `CC` names the C driver the link runs through, by name on `PATH` or by
/// path, and one that isn't there is refused by the name it was given. On
/// Ubuntu `cc` is a gcc, so the link goes through the machine's `musl-gcc`.
#[test]
fn the_link_runs_through_the_c_driver_cc_names() {
    let scratch = repository("        { platform: \"native\" },\n", NATIVE_MAIN);
    for missing in ["no-such-cc", "/nowhere/cc"] {
        scratch
            .run_with_env(&["build", "--force", "//cmd/app"], &[("CC", missing)])
            .exits(1)
            .says(&format!(
                "error: no C compiler on PATH to drive the link (looked for `{missing}`)"
            ));
    }
    let cc = std::process::Command::new("sh")
        .args(["-c", "command -v cc"])
        .output()
        .unwrap();
    let cc = String::from_utf8_lossy(&cc.stdout).trim().to_string();
    assert!(cc.starts_with('/'), "no `cc` on PATH: {cc:?}");
    scratch
        .run_with_env(&["build", "--force", "//cmd/app"], &[("CC", &cc)])
        .ok()
        .says("/cmd/app/app (");
    // A relative path is from where `buri` runs.
    scratch.write_executable("tools/cc", &format!("#!/bin/sh\nexec {cc} \"$@\"\n"));
    scratch
        .run_with_env(&["build", "--force", "//cmd/app"], &[("CC", "tools/cc")])
        .ok()
        .says("/cmd/app/app (");
    // A file that isn't a program is there, and can't be run.
    let not_a_program = scratch
        .write("tools/notes", "not a program\n")
        .display()
        .to_string();
    scratch
        .run_with_env(
            &["build", "--force", "//cmd/app"],
            &[("CC", &not_a_program)],
        )
        .exits(1)
        .says(&format!("error: cannot run {not_a_program}: "))
        .says("fix: install a C toolchain, or set `CC` to one");
}

/// A C driver whose link fails: the build and `--check-reproducible` both
/// say the link failed, with the linker's own words and the command to rerun.
/// The driver compiles, so on Linux it passes for a clang that can target musl
/// rather than giving way to the machine's `musl-gcc`.
#[test]
fn a_failing_link_reports_the_linker_s_output() {
    let scratch = repository("        { platform: \"native\" },\n", NATIVE_MAIN);
    let cc = scratch.write_executable(
        "fake/cc",
        "#!/bin/sh\ncase \"$*\" in\n  *--version*) echo \"fake cc 1.0\"; exit 0 ;;\n  \
         *\" -c \"*) cat >/dev/null; exit 0 ;;\nesac\n\
         echo \"fake-ld: error: this linker links nothing\" >&2\nexit 1\n",
    );
    let cc = cc.display().to_string();
    for args in [
        &["build", "//cmd/app"][..],
        &["build", "--check-reproducible", "//cmd/app"],
    ] {
        scratch
            .run_with_env(args, &[("CC", &cc)])
            .exits(1)
            .says("error: the link failed (")
            .says("fake-ld: error: this linker links nothing")
            .says(&format!(" && {cc} "));
    }
}

/// `BURI_LINKER` picks the linker the driver runs, `cc` being the driver's own
/// default, and each links a program that runs. A linker the machine doesn't
/// have fails the link by name, with the way back to the driver's own.
#[test]
fn buri_linker_picks_the_linker() {
    let scratch = repository("        { platform: \"native\" },\n", NATIVE_MAIN);
    let lld = if cfg!(target_os = "macos") {
        "ld64.lld"
    } else {
        "ld.lld"
    };
    let has_lld = std::process::Command::new("sh")
        .args(["-c", &format!("command -v {lld}")])
        .output()
        .unwrap()
        .status
        .success();
    for linker in ["cc", "system", "lld", "anything else"] {
        let run = scratch.run_with_env(&["build", "//cmd/app"], &[("BURI_LINKER", linker)]);
        if linker == "lld" && !has_lld {
            run.exits(1)
                .says("error: the link failed (cc+lld)")
                .says("fix: if the C compiler does not understand the chosen linker, set `BURI_LINKER=cc`");
            continue;
        }
        run.ok();
        let variant = only_entry(&scratch.path(".buri/out/native"));
        let out = std::process::Command::new(
            scratch.path(&format!(".buri/out/native/{variant}/cmd/app/app")),
        )
        .output()
        .unwrap();
        assert_eq!(
            String::from_utf8_lossy(&out.stdout),
            "native\n",
            "BURI_LINKER={linker}"
        );
    }
}

/// With neither `HOME` nor `BURI_HOME` there is nowhere to remember the linker
/// between runs: a host build links anyway, and a cross build, whose runtime
/// is cached under the home, is refused with what to set.
#[test]
fn a_build_with_no_home_directory() {
    // On Linux x86_64 the cross variant would be the host's own path.
    let outputs = if cfg!(target_os = "macos") {
        "        { platform: \"native\" },\n        { platform: \"native\", variant: \"linux-x86_64\" },\n"
    } else {
        "        { platform: \"native\" },\n"
    };
    let scratch = repository(outputs, NATIVE_MAIN);
    let homeless = |args: &[&str]| {
        let out = buri_command()
            .current_dir(&scratch.root)
            .args(args)
            .arg("--color=never")
            .env_remove("HOME")
            .env_remove("BURI_HOME")
            .output()
            .unwrap();
        (
            out.status.code(),
            format!(
                "{}{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            ),
        )
    };
    let host = format!(
        "native/{}-{}",
        std::env::consts::OS,
        if cfg!(target_arch = "aarch64") {
            "arm64"
        } else {
            "x86_64"
        }
    );
    let (code, said) = homeless(&["build", "--force", &format!("--output={host}"), "//cmd/app"]);
    assert_eq!(code, Some(0), "{said}");
    assert!(said.contains("/cmd/app/app ("), "{said}");
    if cfg!(target_os = "macos") {
        let (code, said) = homeless(&[
            "build",
            "--force",
            "--output=native/linux-x86_64",
            "//cmd/app",
        ]);
        assert_eq!(code, Some(1), "{said}");
        assert!(
            said.contains("error: no home directory to cache the cross runtime under"),
            "{said}"
        );
        assert!(
            said.contains("fix: set `HOME`, or `BURI_HOME`, to a writable directory"),
            "{said}"
        );
    }
}

/// macOS checks each new executable file once, so a test runner runs as the
/// file in `$BURI_HOME/programs` that first held its bytes. An entry someone
/// wrote to isn't handed out, an entry nothing ran for hours is swept and
/// made again while one in use stays, a store that can't be listed still
/// works, and a home that can't hold the store leaves the runner as the linker
/// wrote it. The tests pass every time.
#[cfg(target_os = "macos")]
#[test]
fn a_test_runner_runs_from_the_program_store() {
    use std::os::unix::fs::PermissionsExt;
    let scratch = Scratch::repo("build-driver-store");
    scratch.write(
        "lib/one/BUILD.buri",
        "library {\n    test {\n        sources: [\"test/one_test.buri\"]\n        backends: [NATIVE]\n    }\n}\n",
    );
    scratch.write("lib/one/lib.buri", "export fn one(): Int {\n    1\n}\n");
    scratch.write(
        "lib/one/test/one_test.buri",
        "from \"core/testing/assert\" import * as assert;\nfrom \"//lib/one\" import { one };\n\n\
         test \"one is one\" {\n    assert.equal(one(), 1);\n}\n",
    );
    let home = scratch.path("home").display().to_string();
    let test = |home: &str| {
        scratch
            .run_with_env(&["test", "//lib/one"], &[("BURI_HOME", home), LINKED])
            .ok()
            .says("1 passed");
    };
    let clean = |home: &str| {
        scratch
            .run_with_env(&["clean"], &[("BURI_HOME", home)])
            .ok();
    };
    let variant = || only_entry(&scratch.path(".buri/out/native"));
    let runner = || scratch.path(&format!(".buri/out/native/{}/test-runner", variant()));
    let kept = || std::fs::read_link(runner()).ok();

    test(&home);
    let program = kept().expect("the runner is not a link into the store");
    assert!(
        program.starts_with(&home),
        "{} is outside the store",
        program.display()
    );
    test(&home);
    assert_eq!(
        kept().as_ref(),
        Some(&program),
        "a second run moved the runner"
    );

    std::fs::write(&program, "changed").unwrap();
    clean(&home);
    test(&home);
    assert_eq!(kept(), None, "an entry someone wrote to was handed out");

    let hours_ago = std::time::SystemTime::now() - std::time::Duration::from_secs(3 * 60 * 60);
    for old in [
        program.parent().unwrap().to_path_buf(),
        scratch.path("home/programs/.swept"),
    ] {
        std::fs::File::open(&old)
            .unwrap()
            .set_modified(hours_ago)
            .unwrap();
    }
    clean(&home);
    test(&home);
    assert_eq!(
        kept().as_ref(),
        Some(&program),
        "the stale entry wasn't made again"
    );
    assert_eq!(
        std::fs::read(&program).unwrap(),
        std::fs::read(runner()).unwrap()
    );

    // A sweep leaves an entry something ran lately, and a stale one that
    // something is starting, under its lock.
    use std::os::unix::fs::MetadataExt;
    let inode = || std::fs::metadata(&program).unwrap().ino();
    let made = inode();
    std::fs::File::open(scratch.path("home/programs/.swept"))
        .unwrap()
        .set_modified(hours_ago)
        .unwrap();
    clean(&home);
    test(&home);
    assert_eq!(
        kept().as_ref(),
        Some(&program),
        "a sweep took an entry in use"
    );
    for old in [
        program.parent().unwrap().to_path_buf(),
        scratch.path("home/programs/.swept"),
    ] {
        std::fs::File::open(&old)
            .unwrap()
            .set_modified(hours_ago)
            .unwrap();
    }
    let starting = std::fs::File::open(program.parent().unwrap().join("lock")).unwrap();
    starting.lock_shared().unwrap();
    clean(&home);
    test(&home);
    drop(starting);
    assert_eq!(inode(), made, "a sweep took an entry under its lock");
    // One whose time can't be kept is still handed out.
    let entry = program.parent().unwrap();
    std::fs::set_permissions(entry, std::fs::Permissions::from_mode(0o300)).unwrap();
    clean(&home);
    test(&home);
    std::fs::set_permissions(entry, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(
        kept().as_ref(),
        Some(&program),
        "an entry that can't be touched wasn't handed out"
    );

    // A store that can't be listed still hands out entries; one that can't be
    // made leaves the runner as the linker wrote it.
    let unlisted = scratch.path("unlisted-home");
    std::fs::create_dir_all(unlisted.join("programs")).unwrap();
    std::fs::set_permissions(
        unlisted.join("programs"),
        std::fs::Permissions::from_mode(0o300),
    )
    .unwrap();
    clean(&unlisted.display().to_string());
    test(&unlisted.display().to_string());
    let listed = kept();
    std::fs::set_permissions(
        unlisted.join("programs"),
        std::fs::Permissions::from_mode(0o700),
    )
    .unwrap();
    assert!(
        listed.is_some_and(|p| p.starts_with(&unlisted)),
        "an unlistable store handed out nothing"
    );
    let blocked = scratch.path("blocked-home");
    std::fs::create_dir_all(&blocked).unwrap();
    std::fs::write(blocked.join("programs"), "").unwrap();
    clean(&blocked.display().to_string());
    test(&blocked.display().to_string());
    assert_eq!(kept(), None, "a store under a file was used");
    let read_only = scratch.path("read-only-home");
    std::fs::create_dir_all(read_only.join("programs")).unwrap();
    std::fs::set_permissions(
        read_only.join("programs"),
        std::fs::Permissions::from_mode(0o500),
    )
    .unwrap();
    clean(&read_only.display().to_string());
    test(&read_only.display().to_string());
    std::fs::set_permissions(
        read_only.join("programs"),
        std::fs::Permissions::from_mode(0o700),
    )
    .unwrap();
    assert_eq!(kept(), None, "a read-only store was used");

    let file = scratch.write("home-is-a-file", "").display().to_string();
    clean(&file);
    test(&file);
    assert_eq!(kept(), None, "a store under a file was used");

    clean(&home);
    let homeless = buri_command()
        .current_dir(&scratch.root)
        .args(["test", "--color=never", "//lib/one"])
        .env_remove("HOME")
        .env_remove("BURI_HOME")
        .env(LINKED.0, LINKED.1)
        .output()
        .unwrap();
    assert!(
        String::from_utf8_lossy(&homeless.stdout).contains("1 passed"),
        "{homeless:?}"
    );
    assert_eq!(kept(), None, "a run with no home used a store");
}

/// The shared test runner's claim, left by a run that crashed a day ago, is
/// taken and given back; one a live run holds is left alone, and this run
/// links its runner beside it. The tests pass either way.
#[test]
fn a_test_runner_claim_a_crashed_run_left_is_taken() {
    let scratch = Scratch::repo("build-driver-claim");
    scratch.write(
        "lib/one/BUILD.buri",
        "library {\n    test {\n        sources: [\"test/one_test.buri\"]\n        backends: [NATIVE]\n    }\n}\n",
    );
    scratch.write("lib/one/lib.buri", "export fn one(): Int {\n    1\n}\n");
    let suite = |n: u32| {
        scratch.write(
            "lib/one/test/one_test.buri",
            &format!(
                "from \"core/testing/assert\" import * as assert;\nfrom \"//lib/one\" import {{ one }};\n\n\
                 test \"one is one\" {{\n    assert.equal(one() * {n}, {n});\n}}\n"
            ),
        );
        scratch
            .run_with_env(&["test", "//lib/one"], &[LINKED])
            .ok()
            .says("1 passed");
    };
    suite(1);
    let lock = scratch.path(&format!(
        ".buri/out/native/{}/.test-runner.lock",
        only_entry(&scratch.path(".buri/out/native"))
    ));
    std::fs::write(&lock, "").unwrap();
    let a_day_ago = std::time::SystemTime::now() - std::time::Duration::from_secs(24 * 60 * 60);
    std::fs::File::options()
        .write(true)
        .open(&lock)
        .unwrap()
        .set_modified(a_day_ago)
        .unwrap();
    suite(2);
    assert!(!lock.exists(), "the crashed run's claim is still there");
    std::fs::write(&lock, "").unwrap();
    suite(3);
    assert!(lock.exists(), "a live run's claim was taken");
}

/// A suite linked again beside a failing suite's kept binary runs its own
/// binary, and the failing suite still runs its own. Another run holds the
/// shared runner throughout, so the two suites' batch binary is linked under
/// the first suite's name, and the edit makes that suite link again.
#[test]
fn a_suite_linked_again_leaves_a_kept_binary_under_its_name_alone() {
    let scratch = Scratch::repo("build-driver-kept-name");
    for (name, answer) in [("a", 1), ("b", 2)] {
        scratch.write(
            &format!("lib/{name}/BUILD.buri"),
            "library {\n    test {\n        sources: [\"test/t.buri\"]\n    }\n}\n",
        );
        scratch.write(
            &format!("lib/{name}/lib.buri"),
            "export fn one(): Int {\n    1\n}\n",
        );
        scratch.write(
            &format!("lib/{name}/test/t.buri"),
            &format!(
                "from \"core/testing/assert\" import * as assert;\nfrom \"//lib/{name}\" import {{ one }};\n\n\
                 test \"{name} is {answer}\" {{\n    assert.equal(one(), {answer});\n}}\n"
            ),
        );
    }
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
    scratch.write(
        &format!(".buri/out/native/{os}-{arch}/.test-runner.lock"),
        "",
    );

    scratch
        .run(&["test", "//..."])
        .exits(1)
        .says("1 passed, 1 failed")
        .says("b is 2");
    scratch.write(
        "lib/a/test/t.buri",
        "from \"core/testing/assert\" import * as assert;\nfrom \"//lib/a\" import { one };\n\n\
         test \"a is 1\" {\n    assert.equal(one(), 1);\n}\n\ntest \"a is still 1\" {\n    assert.equal(one(), 1);\n}\n",
    );
    scratch
        .run(&["test", "//..."])
        .exits(1)
        .says("2 passed, 1 failed")
        .says("b is 2");
}

/// A tag's `doc` is how a policy says why, so a tag without one breaks the
/// policy with no reason under it.
#[test]
fn a_tag_without_a_doc_gives_no_reason() {
    for (case, target, code) in [
        ("platform_violation", "//cmd/web", "[platform-violation]"),
        ("tag_violation", "//cmd/web", "[tag-conflict]"),
    ] {
        let scratch = Scratch::copy_of(
            "build-driver-tags",
            &tests_dir().join(format!("repositories/tags/{case}/repo")),
        );
        let repo = scratch.read("REPO.buri");
        let undocumented: String = repo
            .lines()
            .filter(|l| !l.trim_start().starts_with("doc:"))
            .map(|l| format!("{l}\n"))
            .collect();
        assert_ne!(repo, undocumented, "{case} documents no tag");
        scratch.write("REPO.buri", &undocumented);
        scratch
            .run(&["lint", target])
            .exits(1)
            .says(code)
            .silent_about("\": ");
    }
}

/// A binary at the repository's root is named after the root's directory.
#[test]
fn a_binary_at_the_root_is_named_after_its_directory() {
    let scratch = Scratch::repo("build-driver-root");
    scratch.write(
        "BUILD.buri",
        "binary {\n    outputs: [\n        { platform: \"node\" },\n    ]\n}\n",
    );
    scratch.write("main.buri", NODE_MAIN);
    let name = scratch
        .root
        .file_name()
        .unwrap()
        .to_string_lossy()
        .to_string();
    scratch
        .run(&["build"])
        .ok()
        .says(&format!(".buri/out/node/{name}.mjs ("));
}

/// A C driver that links through the real one and answers a dry run (`-###`)
/// the way `FAKE_DRY` says.
const DRY_RUN_DRIVER: &str = r#"#!/bin/sh
dry=
for a in "$@"; do [ "$a" = "-###" ] && dry=1; done
if [ -n "$dry" ]; then
  q=""
  for a in "$@"; do q="$q \"$a\""; done
  case "$FAKE_DRY" in
    error) echo 'clang: error: no dry run here' >&2 ;;
    two) echo " \"/usr/bin/false\"$q" >&2; echo " \"/usr/bin/false\"$q" >&2 ;;
    relative) echo " \"ld\"$q" >&2 ;;
    script) echo " \"$0\"$q" >&2 ;;
    no-objects) echo ' "/usr/bin/false" "-o" "artifact"' >&2 ;;
    failing) echo " \"/usr/bin/false\"$q" >&2 ;;
    missing) echo " \"/nowhere/ld\"$q" >&2 ;;
  esac
  exit 0
fi
exec "$REAL_CC" "$@"
"#;

/// The link replays the linker command a driver's dry run printed, and goes
/// through the driver whenever that command can't be replayed: an error, two
/// commands, a linker named by a relative path or a script, a command that
/// doesn't name the objects, one that fails when it runs, or a linker that
/// isn't there. Each still links a program that runs, from scratch and again.
///
/// The driver forwards to the `CC` this suite was given. On Linux a `cc` that
/// is a gcc can't target musl, so the link would skip the driver for `musl-gcc`.
#[test]
fn a_dry_run_the_link_cannot_replay_goes_through_the_driver() {
    let scratch = repository("        { platform: \"native\" },\n", NATIVE_MAIN);
    let driver = scratch
        .write_executable("fake/cc", DRY_RUN_DRIVER)
        .display()
        .to_string();
    let real_cc = std::env::var("CC").unwrap_or_else(|_| String::from("cc"));
    let real = std::process::Command::new("sh")
        .args(["-c", &format!("command -v '{real_cc}'")])
        .output()
        .unwrap();
    let real = String::from_utf8_lossy(&real.stdout).trim().to_string();
    for mode in [
        "error",
        "two",
        "relative",
        "script",
        "no-objects",
        "failing",
        "missing",
    ] {
        let home = scratch.path(&format!("home-{mode}")).display().to_string();
        let env = [
            ("CC", driver.as_str()),
            ("REAL_CC", real.as_str()),
            ("FAKE_DRY", mode),
            ("BURI_HOME", home.as_str()),
        ];
        for args in [
            &["build", "//cmd/app"][..],
            &["build", "--force", "//cmd/app"],
        ] {
            scratch.run_with_env(args, &env).ok();
            let variant = only_entry(&scratch.path(".buri/out/native"));
            let out = std::process::Command::new(
                scratch.path(&format!(".buri/out/native/{variant}/cmd/app/app")),
            )
            .output()
            .unwrap();
            assert_eq!(
                String::from_utf8_lossy(&out.stdout),
                "native\n",
                "FAKE_DRY={mode}"
            );
        }
    }
}

/// A repository platform's `assets` land beside each artifact at their own
/// paths, and an asset the platform doesn't have is refused by name.
#[test]
fn a_platform_s_assets_are_copied_and_a_missing_one_is_refused() {
    let scratch = Scratch::copy_of(
        "build-driver-assets",
        &tests_dir().join("repositories/custom-platforms/platform_rules/repo"),
    );
    scratch.edit(
        "platform/pair/BUILD.buri",
        "platform {\n",
        "platform {\n    assets: [\"style/site.css\"]\n",
    );
    scratch
        .run(&["build", "//cmd/app"])
        .exits(1)
        .says("error: style/site.css does not exist [unknown-source]")
        .says("copying it failed");
    scratch.write("platform/pair/style/site.css", "body {}\n");
    scratch.run(&["build", "//cmd/app"]).ok();
    assert_eq!(
        scratch.read(".buri/out/platform/pair/cmd/app/style/site.css"),
        "body {}\n"
    );
}

/// Sets the permission bits of `path` and everything under it.
fn chmod_tree(path: &std::path::Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    if path.is_dir() {
        for entry in std::fs::read_dir(path).unwrap().flatten() {
            chmod_tree(&entry.path(), mode);
        }
    }
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
}

/// A cache nothing can be written into: the build after an edit compiles,
/// links and writes the program anyway, without keeping any of it.
#[test]
fn a_cache_that_cannot_be_written_still_builds() {
    let scratch = repository("        { platform: \"native\" },\n", NATIVE_MAIN);
    scratch.run(&["build", "//cmd/app"]).ok();
    scratch.edit("cmd/app/main.buri", "ctx, \"native\"", "ctx, \"edited\"");
    let cache = scratch.path(".buri/cache");
    chmod_tree(&cache, 0o555);
    let built = scratch.run(&["build", "//cmd/app"]);
    chmod_tree(&cache, 0o755);
    built.ok();
    scratch.run(&["run", "//cmd/app"]).ok().says("edited");
}

/// Another toolchain's cache entries go even when there is nowhere to move
/// them aside, and a cache whose directory can't be listed keeps its entries.
#[test]
fn another_toolchain_s_cache_is_emptied_without_a_trash_directory() {
    let scratch = repository("        { platform: \"node\" },\n", NODE_MAIN);
    scratch.run(&["build", "//cmd/app"]).ok();
    scratch.write(".buri/cache/.toolchain", "another toolchain");
    scratch.write(".buri/cache/stray", "another toolchain's entry\n");
    scratch.write(".buri/cache/zz/old", "another toolchain's entry\n");
    scratch.write(".buri/trash", "in the way\n");
    scratch.run(&["build", "//cmd/app"]).ok();
    assert!(
        !scratch.path(".buri/cache/stray").exists(),
        "another toolchain's file entry is still there"
    );
    assert!(
        !scratch.path(".buri/cache/zz").exists(),
        "another toolchain's directory is still there"
    );

    scratch.write(".buri/cache/.toolchain", "another toolchain");
    let cache = scratch.path(".buri/cache");
    std::fs::set_permissions(&cache, std::os::unix::fs::PermissionsExt::from_mode(0o300)).unwrap();
    let built = scratch.run(&["build", "//cmd/app"]);
    std::fs::set_permissions(&cache, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    built.ok().says(".buri/out/node/cmd/app/app.mjs (");
}

/// The linker's identity is remembered under `BURI_HOME`. A record cut short,
/// one that isn't a digest, and a directory where the record goes are each
/// asked again, and the edited program still builds and runs.
#[test]
fn a_damaged_linker_record_is_asked_again() {
    let scratch = repository("        { platform: \"native\" },\n", NATIVE_MAIN);
    let home = scratch.path("home").display().to_string();
    let env = [("BURI_HOME", home.as_str())];
    scratch.run_with_env(&["build", "//cmd/app"], &env).ok();
    let records = || -> Vec<std::path::PathBuf> {
        std::fs::read_dir(scratch.path("home/linker-identity"))
            .unwrap()
            .flatten()
            .map(|e| e.path())
            .collect()
    };
    assert!(!records().is_empty(), "the build remembered no linker");
    let mut said = "native";
    for how in ["short", "not hex", "a directory"] {
        for record in records() {
            match how {
                "short" => std::fs::write(&record, "short").unwrap(),
                "not hex" => std::fs::write(&record, "z".repeat(64)).unwrap(),
                _ => {
                    std::fs::remove_file(&record).unwrap();
                    std::fs::create_dir_all(record.join("in-the-way")).unwrap();
                }
            }
        }
        scratch.edit(
            "cmd/app/main.buri",
            &format!("ctx, \"{said}\""),
            &format!("ctx, \"{how}\""),
        );
        said = how;
        scratch.run_with_env(&["build", "//cmd/app"], &env).ok();
        scratch
            .run_with_env(&["run", "//cmd/app"], &env)
            .ok()
            .says(how);
    }
}

/// A cross link drives the C driver with `--target=`, so a driver that can't
/// compile for the target, or can't be run at all, is refused before anything
/// is built.
#[test]
fn a_cross_link_needs_a_c_driver_that_takes_a_target() {
    let (variant, triple) = if cfg!(target_os = "linux") && cfg!(target_arch = "x86_64") {
        ("linux-arm64", "aarch64-unknown-linux-musl")
    } else {
        ("linux-x86_64", "x86_64-unknown-linux-musl")
    };
    let scratch = repository(
        &format!("        {{ platform: \"native\", variant: \"{variant}\" }},\n"),
        NATIVE_MAIN,
    );
    let refuses = scratch
        .write_executable("fake/cc", "#!/bin/sh\nexit 1\n")
        .display()
        .to_string();
    let not_a_program = scratch
        .write("fake/notes", "not a program\n")
        .display()
        .to_string();
    for cc in [refuses, not_a_program] {
        scratch
            .run_with_env(&["build", "//cmd/app"], &[("CC", &cc)])
            .exits(1)
            .says(&format!(
                "error: the C driver cannot be pointed at {triple}, so it cannot cross-link"
            ))
            .says("fix: install a clang that understands `--target=`, or set `CC` to one");
    }
}

/// `BURI_MUSL` is a question about a Linux link. Elsewhere it's ignored. On
/// Linux a value that isn't a libc is refused, `off` links against glibc
/// through the dynamic loader, and `baked` and `system` each link with musl
/// or say why they can't.
#[test]
fn buri_musl_forces_the_libc_of_a_linux_link() {
    let scratch = repository("        { platform: \"native\" },\n", NATIVE_MAIN);
    let build = |value: &str| {
        scratch.run_with_env(&["build", "--force", "//cmd/app"], &[("BURI_MUSL", value)])
    };
    if !cfg!(target_os = "linux") {
        build("bogus").ok();
        return;
    }
    build("bogus")
        .exits(1)
        .says("`BURI_MUSL=bogus` is not a libc this toolchain can be forced to")
        .says("set `BURI_MUSL` to `baked`, `system` or `off`");
    let program = || {
        let variant = only_entry(&scratch.path(".buri/out/native"));
        std::fs::read(scratch.path(&format!(".buri/out/native/{variant}/cmd/app/app"))).unwrap()
    };
    let interpreted = |bytes: &[u8]| bytes.windows(b"/ld-linux".len()).any(|w| w == b"/ld-linux");
    // A glibc link of a musl runtime archive fails in the linker, as `off` asks.
    for (value, glibc, refusal) in [
        ("off", true, "error: the link failed ("),
        (
            "baked",
            false,
            "its C driver does not understand `--target=`",
        ),
        (
            "system",
            false,
            "`BURI_MUSL=system` was set and no musl driver is on PATH",
        ),
    ] {
        let run = build(value);
        if run.code == 0 {
            assert_eq!(
                interpreted(&program()),
                glibc,
                "BURI_MUSL={value} linked the wrong libc"
            );
            scratch
                .run_with_env(&["run", "//cmd/app"], &[("BURI_MUSL", value)])
                .ok()
                .says("native");
        } else {
            run.exits(1).says(refusal);
        }
    }
}

/// Under `BURI_MUSL=system`, a C driver that targets musl itself links the
/// program through the machine's musl driver.
#[cfg(target_os = "linux")]
#[test]
fn a_c_driver_that_is_musl_s_own_links_the_program() {
    let musl = std::process::Command::new("sh")
        .args(["-c", "command -v musl-gcc || command -v musl-clang"])
        .output()
        .unwrap();
    let musl = String::from_utf8_lossy(&musl.stdout).trim().to_string();
    let scratch = repository("        { platform: \"native\" },\n", NATIVE_MAIN);
    // A driver that says it targets musl, as Alpine's `cc` does, and can't be
    // pointed at another target.
    let cc = scratch
        .write_executable(
            "fake/cc",
            &format!(
                "#!/bin/sh\ncase \"$*\" in\n  -dumpmachine) echo x86_64-alpine-linux-musl ;;\n  \
                 *--target=*) exit 1 ;;\n  *) exec {musl} \"$@\" ;;\nesac\n"
            ),
        )
        .display()
        .to_string();
    let built = scratch.run_with_env(
        &["build", "//cmd/app"],
        &[("CC", &cc), ("BURI_MUSL", "system")],
    );
    if musl.is_empty() {
        built.exits(1).says("error: the link failed (");
        return;
    }
    built.ok();
    scratch
        .run_with_env(
            &["run", "//cmd/app"],
            &[("CC", &cc), ("BURI_MUSL", "system")],
        )
        .ok()
        .says("native");
}

/// A C driver some process is still writing can't be started. The build tries
/// for about half a second, then says why.
#[cfg(target_os = "linux")]
#[test]
fn a_c_driver_still_being_written_is_reported() {
    let scratch = repository("        { platform: \"native\" },\n", NATIVE_MAIN);
    let cc = scratch.write_executable("fake/cc", "#!/bin/sh\nexit 0\n");
    // Open for writing for as long as the build runs, so every start is `ETXTBSY`.
    let _writing = std::fs::File::options().append(true).open(&cc).unwrap();
    scratch
        .run_with_env(
            &["build", "//cmd/app"],
            &[("CC", &cc.display().to_string())],
        )
        .exits(1)
        .says("Text file busy");
}

/// A glibc C driver that can't be pointed at musl, with no musl driver on
/// `PATH`, links nothing. Unforced, the build says no musl driver was found
/// either; `BURI_MUSL=baked` names the driver as the reason.
#[cfg(target_os = "linux")]
#[test]
fn a_c_driver_that_cant_target_musl_links_nothing() {
    let scratch = repository("        { platform: \"native\" },\n", NATIVE_MAIN);
    let cc = scratch
        .write_executable("fake/cc", "#!/bin/sh\ncase \"$*\" in\n  -dumpmachine) echo x86_64-linux-gnu ;;\n  *) exit 1 ;;\nesac\n")
        .display()
        .to_string();
    let path = scratch.path("fake").display().to_string();
    for (musl, says) in [
        ("", "no musl driver is on PATH either"),
        ("baked", "its C driver does not understand `--target=`"),
    ] {
        scratch
            .run_with_env(
                &["build", "//cmd/app"],
                &[("CC", &cc), ("PATH", &path), ("BURI_MUSL", musl)],
            )
            .exits(1)
            .says(says);
    }
}

/// A `buri` whose linker wrote no build id knows itself by the hash of its
/// bytes, remembered under `XDG_CACHE_HOME`. Its builds are cached like any
/// other's, whether the remembered hash is damaged or can't be written.
#[cfg(target_os = "linux")]
#[test]
fn a_buri_without_a_build_id_still_caches() {
    let scratch = repository("        { platform: \"node\" },\n", NODE_MAIN);
    let mut bytes = std::fs::read(buri()).unwrap();
    let note = bytes
        .windows(16)
        .position(|w| w[..4] == [4, 0, 0, 0] && w[8..12] == [3, 0, 0, 0] && &w[12..] == b"GNU\0")
        .expect("buri carries no GNU build id");
    // The build id's kind under another owner's name, `GNX`: the loader doesn't
    // read it, and `buri` no longer finds its id.
    bytes[note + 14] = b'X';
    let copy = scratch.path("bin/buri");
    write_executable(&copy, bytes);
    // Under the coverage gate, the copy's profiles lack the build id the
    // pooled ones carry, so the two can't share a pooled file.
    let profile = std::env::var_os("LLVM_PROFILE_FILE")
        .map(|pool| std::path::Path::new(&pool).with_file_name("%m_no_build_id_%p.profraw"));
    // `None` is no home at all, so nowhere to remember the hash.
    let build = |xdg: Option<&str>| {
        let mut command = std::process::Command::new(&copy);
        if let Some(profile) = &profile {
            command.env("LLVM_PROFILE_FILE", profile);
        }
        match xdg {
            Some(xdg) => command.env("XDG_CACHE_HOME", scratch.path(xdg)),
            None => command.env_remove("XDG_CACHE_HOME").env_remove("HOME"),
        };
        let out = heap_checked(&mut command)
            .current_dir(&scratch.root)
            .args(["build", "--color=never", "//cmd/app"])
            .env("BURI_HOME", scratch.path("home"))
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).to_string()
    };
    assert!(
        !build(Some("xdg")).contains("cached"),
        "the first build was served from the cache"
    );
    assert!(
        build(Some("xdg")).contains("cached"),
        "the second build wasn't served from the cache"
    );
    for damage in [
        "sha256:short".to_string(),
        format!("sha256:{}", "z".repeat(64)),
    ] {
        for record in std::fs::read_dir(scratch.path("xdg/buri/exe-identity"))
            .unwrap()
            .flatten()
        {
            std::fs::write(record.path(), &damage).unwrap();
        }
        assert!(
            build(Some("xdg")).contains("cached"),
            "a record holding {damage} moved the toolchain's identity"
        );
    }
    scratch.write("blocked", "a file where the cache directory goes\n");
    assert!(
        build(Some("blocked")).contains("cached"),
        "an unwritable record moved the toolchain's identity"
    );
    assert!(
        build(None).contains("cached"),
        "a buri with no home has another identity"
    );
}

/// A cargo that logs each call to `$STAND_IN_LOG` and writes
/// `$STAND_IN_ARCHIVE` where `cargo rustc --release` puts the runtime. Without
/// an archive it fails as cargo does offline. With `$STAND_IN_THEIRS`, a build
/// first copies that cross runtime to `$STAND_IN_PUBLISH`, as another build
/// finishing first would.
#[cfg(target_os = "macos")]
const STAND_IN_CARGO: &str = r#"#!/bin/sh
echo "$1" >> "$STAND_IN_LOG"
command=$1
while [ $# -gt 0 ]; do
  case "$1" in
    --target) target=$2; shift ;;
    --target-dir) dir=$2; shift ;;
  esac
  shift
done
if [ -z "$STAND_IN_ARCHIVE" ]; then echo "error: no matching package named \`libc\` found" >&2; exit 101; fi
if [ "$command" = rustc ] && [ -n "$STAND_IN_THEIRS" ]; then cp -R "$STAND_IN_THEIRS" "$STAND_IN_PUBLISH"; fi
mkdir -p "$dir/$target/release/deps" && cp "$STAND_IN_ARCHIVE" "$dir/$target/release/deps/libburi_rt.a"
"#;

/// `CARGO` builds a cross runtime into `BURI_HOME` once and later builds reuse
/// it, until its feature list or a sysroot member is damaged. A cargo that
/// fails is reported with its own words, and leaves nothing in the home.
#[cfg(target_os = "macos")]
#[test]
fn a_cross_runtime_is_built_once_with_cargo() {
    crate::harness::sweep::kept::warm_cross_runtime();
    let shared = crate::harness::sweep::kept::shared_cross_home().join("cross");
    let archive = std::fs::read_dir(&shared)
        .unwrap()
        .flatten()
        .map(|e| e.path().join("libburi_rt.a"))
        .find(|p| p.is_file())
        .expect("no warm cross runtime");
    let scratch = repository(
        "        { platform: \"native\", variant: \"linux-x86_64\" },\n",
        NATIVE_MAIN,
    );
    let cargo = scratch
        .write_executable("fake/cargo", STAND_IN_CARGO)
        .display()
        .to_string();
    let home = scratch.path("home").display().to_string();
    let log = scratch.path("cargo.log").display().to_string();
    let archive = archive.display().to_string();
    let env = [
        ("BURI_HOME", home.as_str()),
        ("CARGO", cargo.as_str()),
        ("STAND_IN_LOG", log.as_str()),
    ];
    scratch
        .run_with_env(&["build", "//cmd/app"], &env)
        .exits(1)
        .says("error: could not cross-build the runtime for x86_64-unknown-linux-musl")
        .says("error: no matching package named `libc` found");
    let left: Vec<_> = std::fs::read_dir(scratch.path("home/cross"))
        .map(|d| d.flatten().collect())
        .unwrap_or_default();
    assert!(left.is_empty(), "a failed cross build left {left:?}");

    // `CARGO_HOME` is the one `CARGO_*` variable the runtime's cargo keeps.
    let cargo_home = scratch.path("cargo-home").display().to_string();
    let env = [
        env[0],
        env[1],
        env[2],
        ("STAND_IN_ARCHIVE", archive.as_str()),
        ("CARGO_HOME", cargo_home.as_str()),
    ];
    let builds = || {
        scratch
            .read("cargo.log")
            .lines()
            .filter(|l| *l == "rustc")
            .count()
    };
    let build = || {
        scratch
            .run_with_env(&["build", "--force", "//cmd/app"], &env)
            .ok();
        let program =
            std::fs::read(scratch.path(".buri/out/native/linux-x86_64/cmd/app/app")).unwrap();
        assert_eq!(
            program.get(..4),
            Some(&b"\x7fELF"[..]),
            "the cross build wrote no ELF program"
        );
    };
    let failed = builds();
    build();
    assert_eq!(
        builds(),
        failed + 1,
        "the first cross build didn't run cargo once"
    );
    build();
    assert_eq!(
        builds(),
        failed + 1,
        "the second cross build ran cargo again"
    );
    let entry = || {
        std::fs::read_dir(scratch.path("home/cross"))
            .unwrap()
            .flatten()
            .map(|e| e.path())
            .find(|p| p.is_dir())
            .unwrap()
    };
    std::fs::write(entry().join("features"), "another feature\n").unwrap();
    build();
    assert_eq!(builds(), failed + 2, "a damaged feature list was reused");
    std::fs::remove_file(entry().join("musl/lib/crt1.o")).unwrap();
    build();
    assert_eq!(
        builds(),
        failed + 3,
        "a sysroot missing a member was reused"
    );
    // Both rounds of a reproducibility check link against the one runtime.
    scratch
        .run_with_env(&["build", "--check-reproducible", "//cmd/app"], &env)
        .ok();
    assert_eq!(
        builds(),
        failed + 3,
        "a reproducibility check built the cross runtime again"
    );
    // Another build that publishes the runtime while this one builds it wins,
    // and this one links against theirs.
    let ours = entry();
    let theirs = scratch.path("theirs").display().to_string();
    std::fs::rename(&ours, &theirs).unwrap();
    let ours = ours.display().to_string();
    let racing = [
        env[0],
        env[1],
        env[2],
        env[3],
        env[4],
        ("STAND_IN_THEIRS", theirs.as_str()),
        ("STAND_IN_PUBLISH", ours.as_str()),
    ];
    scratch
        .run_with_env(&["build", "--force", "//cmd/app"], &racing)
        .ok();
    assert_eq!(builds(), failed + 4, "the racing build didn't run cargo");
    assert!(
        std::path::Path::new(&ours).join("libburi_rt.a").is_file(),
        "the published runtime is gone"
    );
}

/// A failing native suite runs again from the runner it was linked into. One
/// someone deleted is put back, and the suite still fails the same way.
#[test]
fn a_failing_suite_whose_runner_was_deleted_runs_again() {
    let scratch = Scratch::repo("build-driver-runner");
    scratch.write(
        "lib/one/BUILD.buri",
        "library {\n    test {\n        sources: [\"test/one_test.buri\"]\n        backends: [NATIVE]\n    }\n}\n",
    );
    scratch.write("lib/one/lib.buri", "export fn one(): Int {\n    1\n}\n");
    scratch.write(
        "lib/one/test/one_test.buri",
        "from \"core/testing/assert\" import * as assert;\nfrom \"//lib/one\" import { one };\n\n\
         test \"one is two\" {\n    assert.equal(one(), 2);\n}\n",
    );
    scratch
        .run_with_env(&["test", "//lib/one"], &[LINKED])
        .exits(1)
        .says("0 passed, 1 failed");
    let variant = only_entry(&scratch.path(".buri/out/native"));
    std::fs::remove_file(scratch.path(&format!(".buri/out/native/{variant}/test-runner"))).unwrap();
    scratch
        .run_with_env(&["test", "//lib/one"], &[LINKED])
        .exits(1)
        .says("0 passed, 1 failed");
}

/// A link line the driver printed once that no longer links is dropped, and
/// the driver links instead. Here the line names a linker that always fails,
/// and both rounds of a reproducibility check still link.
#[test]
fn a_stale_link_line_falls_back_to_the_driver() {
    let scratch = repository("        { platform: \"native\" },\n", NATIVE_MAIN);
    let found = std::process::Command::new("sh")
        .args(["-c", "command -v \"${CC:-cc}\""])
        .output()
        .unwrap();
    let real = String::from_utf8_lossy(&found.stdout).trim().to_string();
    let cc = scratch
        .write_executable(
            "fake/cc",
            &format!(
                "#!/bin/sh\nif [ \"$1\" = \"-###\" ]; then\n  \"{real}\" \"$@\" 2>&1 >/dev/null | \
                 sed 's|^ \"[^\"]*\"| \"/usr/bin/false\"|' >&2\n  exit 0\nfi\nexec \"{real}\" \"$@\"\n"
            ),
        )
        .display()
        .to_string();
    scratch
        .run_with_env(
            &["build", "--check-reproducible", "--verbose", "//cmd/app"],
            &[("CC", &cc)],
        )
        .ok()
        .says("//cmd/app is reproducible (");
}

/// The link directory of `//cmd/app`'s native output, under `.buri/link`.
fn link_dir(scratch: &Scratch) -> std::path::PathBuf {
    scratch.path(&format!(
        ".buri/link/{}",
        only_entry(&scratch.path(".buri/link"))
    ))
}

/// Something in the way of the link directory fails the build with the path
/// it couldn't make. Once it's gone the edited program builds and runs.
#[test]
fn a_link_directory_that_cannot_be_written_is_an_error() {
    let scratch = repository("        { platform: \"native\" },\n", NATIVE_MAIN);
    scratch.run(&["build", "//cmd/app"]).ok();
    let dir = link_dir(&scratch);
    std::fs::rename(scratch.path(".buri/link"), scratch.path("link-aside")).unwrap();
    scratch.write(".buri/link", "in the way\n");
    scratch.edit("cmd/app/main.buri", "ctx, \"native\"", "ctx, \"edited\"");
    scratch
        .run(&["build", "//cmd/app"])
        .exits(1)
        .says("error: cannot create ")
        .says(&format!("{}: ", dir.file_name().unwrap().to_string_lossy()));
    std::fs::remove_file(scratch.path(".buri/link")).unwrap();
    std::fs::rename(scratch.path("link-aside"), scratch.path(".buri/link")).unwrap();

    scratch.run(&["build", "//cmd/app"]).ok();
    scratch.run(&["run", "//cmd/app"]).ok().says("edited");
}

/// A forced link into a directory it can read but not write to fails with a
/// path inside that directory, rather than linking what the last build left
/// there. Once it's writable again the program builds and runs.
#[test]
fn a_read_only_link_directory_is_an_error() {
    let scratch = repository("        { platform: \"native\" },\n", NATIVE_MAIN);
    scratch.run(&["build", "//cmd/app"]).ok();
    let dir = link_dir(&scratch);
    let mode = |m| std::fs::set_permissions(&dir, std::os::unix::fs::PermissionsExt::from_mode(m));
    mode(0o555).unwrap();
    let forced = scratch.run(&["build", "--force", "//cmd/app"]);
    mode(0o755).unwrap();
    forced
        .exits(1)
        .says("error: cannot write ")
        .says(&dir.display().to_string());

    scratch.run(&["build", "--force", "//cmd/app"]).ok();
    scratch.run(&["run", "//cmd/app"]).ok().says("native");
}

/// A C driver that says it linked but wrote no program is an error naming the
/// file it should have written.
#[test]
fn a_link_that_writes_no_program_is_an_error() {
    let scratch = repository("        { platform: \"native\" },\n", NATIVE_MAIN);
    let cc = scratch
        .write_executable(
            "fake/cc",
            "#!/bin/sh\ncase \"$*\" in *--version*) echo \"fake cc 1.0\" ;; esac\nexit 0\n",
        )
        .display()
        .to_string();
    scratch
        .run_with_env(&["build", "//cmd/app"], &[("CC", &cc)])
        .exits(1)
        .says("error: the link produced no ")
        .says("/artifact: ");
}

/// A cache on another filesystem than the repository: the linked program is
/// copied into it rather than moved, and the next build is served from it.
#[cfg(target_os = "linux")]
#[test]
fn a_cache_on_another_filesystem_keeps_the_program() {
    let scratch = repository("        { platform: \"native\" },\n", NATIVE_MAIN);
    let elsewhere =
        std::path::Path::new("/dev/shm").join(format!("buri-cache-{}", std::process::id()));
    std::fs::create_dir_all(&elsewhere).unwrap();
    std::fs::create_dir_all(scratch.path(".buri")).unwrap();
    std::os::unix::fs::symlink(&elsewhere, scratch.path(".buri/cache")).unwrap();
    let first = scratch.run(&["build", "//cmd/app"]);
    let _ = std::fs::remove_dir_all(scratch.path(".buri/out"));
    let second = scratch.run(&["build", "//cmd/app"]);
    let ran = scratch.run(&["run", "//cmd/app"]);
    let _ = std::fs::remove_dir_all(&elsewhere);
    first.ok();
    second.ok().says("cached");
    ran.ok().says("native");
}

/// With no lld on `PATH` the link goes through the system's own linker, and
/// the program runs.
#[cfg(target_os = "macos")]
#[test]
fn a_mac_without_lld_links_with_the_system_linker() {
    let scratch = repository("        { platform: \"native\" },\n", NATIVE_MAIN);
    scratch
        .run_with_env(&["build", "//cmd/app"], &[("PATH", "/usr/bin:/bin")])
        .ok();
    scratch
        .run_with_env(&["run", "//cmd/app"], &[("PATH", "/usr/bin:/bin")])
        .ok()
        .says("native");
}

/// A built program someone made unexecutable is made executable again by the
/// next build, which has nothing else to do.
#[test]
fn a_program_that_lost_its_execute_bit_gets_it_back() {
    use std::os::unix::fs::PermissionsExt;
    let scratch = repository("        { platform: \"native\" },\n", NATIVE_MAIN);
    scratch.run(&["build", "//cmd/app"]).ok();
    let variant = only_entry(&scratch.path(".buri/out/native"));
    let program = scratch.path(&format!(".buri/out/native/{variant}/cmd/app/app"));
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o644)).unwrap();
    scratch.run(&["build", "//cmd/app"]).ok().says("cached");
    assert_eq!(
        std::fs::metadata(&program).unwrap().permissions().mode() & 0o777,
        0o755
    );
}
