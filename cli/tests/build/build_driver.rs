//! What `buri build` does around the compiler: checking a native artifact
//! is reproducible, refusing an output this machine can't build, writing an
//! artifact where something is in the way, the C driver and linker a link
//! runs through, and the program store macOS runs test runners from.
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
    scratch.write("cmd/app/BUILD.buri", &format!("binary {{\n    outputs: [\n{outputs}    ]\n}}\n"));
    scratch.write("cmd/app/main.buri", main);
    scratch.write("lib/one/BUILD.buri", "library {}\n");
    scratch.write("lib/one/lib.buri", "export fn one(): Int {\n    1\n}\n");
    scratch
}

/// The only directory under `dir`: the host's variant under `.buri/out/native`.
fn only_entry(dir: &std::path::Path) -> String {
    let names: Vec<String> =
        std::fs::read_dir(dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().to_string()).collect();
    let [name] = names.as_slice() else { panic!("{} holds {names:?}", dir.display()) };
    name.clone()
}

/// A native artifact built twice compares its objects and its executable, and
/// says so under `--verbose`; a library has nothing to compare.
#[test]
fn a_native_artifact_is_checked_for_reproducibility() {
    let scratch = repository("        { platform: \"native\" },\n", NATIVE_MAIN);
    let verbose = scratch.run(&["build", "--check-reproducible", "--verbose", "//cmd/app", "//lib/one"]);
    verbose.ok().says("//cmd/app is reproducible (").says(" objects)");
    let quiet = scratch.run(&["build", "--check-reproducible", "//cmd/app"]);
    quiet.ok();
    assert_eq!(quiet.all(), "", "a reproducible artifact is checked quietly");
    scratch.run(&["build", "--check-reproducible", "--verbose", "//lib/one"]).ok().says("nothing to build");
}

/// A macOS variant that isn't this machine's has no native artifact here, and
/// `--check-reproducible` refuses it before building anything.
#[test]
fn check_reproducible_refuses_another_machine_s_macos_output() {
    let other = if cfg!(target_os = "macos") && cfg!(target_arch = "aarch64") { "x86_64" } else { "arm64" };
    let variant = format!("macos-{other}");
    let scratch =
        repository(&format!("        {{ platform: \"native\", variant: \"{variant}\" }},\n"), NATIVE_MAIN);
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
    scratch.run(&["build", "//cmd/app"]).exits(1).says("cannot write ").says("app.mjs");
    scratch.run(&["build", "--force", "//cmd/app"]).exits(1).says("cannot write ").says("app.mjs");
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
    scratch.run(&["build", "//cmd/app"]).exits(1).says("cannot write ").says("/cmd/app/app");
    scratch.run(&["build", "--force", "//cmd/app"]).exits(1).says("cannot write ").says("/cmd/app/app");
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
    std::fs::File::options().write(true).open(&lock).unwrap().set_modified(an_hour_ago).unwrap();
    scratch.run(&["build", "--explain", "//cmd/app"]).ok().says("run    link //cmd/app node");
    // A build that waited the lock out goes on without taking it.
    assert!(!lock.exists(), "the stale lock is still there");
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
            .says(&format!("error: no C compiler on PATH to drive the link (looked for `{missing}`)"));
    }
    let cc = std::process::Command::new("sh").args(["-c", "command -v cc"]).output().unwrap();
    let cc = String::from_utf8_lossy(&cc.stdout).trim().to_string();
    assert!(cc.starts_with('/'), "no `cc` on PATH: {cc:?}");
    scratch.run_with_env(&["build", "--force", "//cmd/app"], &[("CC", &cc)]).ok().says("/cmd/app/app (");
    // A relative path is from where `buri` runs.
    scratch.write_executable("tools/cc", &format!("#!/bin/sh\nexec {cc} \"$@\"\n"));
    scratch.run_with_env(&["build", "--force", "//cmd/app"], &[("CC", "tools/cc")]).ok().says("/cmd/app/app (");
    // A file that isn't a program is there, and can't be run.
    let not_a_program = scratch.write("tools/notes", "not a program\n").display().to_string();
    scratch
        .run_with_env(&["build", "--force", "//cmd/app"], &[("CC", &not_a_program)])
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
    for args in [&["build", "//cmd/app"][..], &["build", "--check-reproducible", "//cmd/app"]] {
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
    let lld = if cfg!(target_os = "macos") { "ld64.lld" } else { "ld.lld" };
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
        let out = std::process::Command::new(scratch.path(&format!(".buri/out/native/{variant}/cmd/app/app")))
            .output()
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout), "native\n", "BURI_LINKER={linker}");
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
        (out.status.code(), format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)))
    };
    let host = format!("native/{}-{}", std::env::consts::OS, if cfg!(target_arch = "aarch64") { "arm64" } else { "x86_64" });
    let (code, said) = homeless(&["build", "--force", &format!("--output={host}"), "//cmd/app"]);
    assert_eq!(code, Some(0), "{said}");
    assert!(said.contains("/cmd/app/app ("), "{said}");
    if cfg!(target_os = "macos") {
        let (code, said) = homeless(&["build", "--force", "--output=native/linux-x86_64", "//cmd/app"]);
        assert_eq!(code, Some(1), "{said}");
        assert!(said.contains("error: no home directory to cache the cross runtime under"), "{said}");
        assert!(said.contains("fix: set `HOME`, or `BURI_HOME`, to a writable directory"), "{said}");
    }
}

/// macOS checks each new executable file once, so a test runner runs as the
/// file in `$BURI_HOME/programs` that first held its bytes. An entry someone
/// wrote to isn't handed out, an entry nothing ran for hours is swept and
/// made again, and a home that can't hold the store leaves the runner as the
/// linker wrote it. The tests pass every time.
#[cfg(target_os = "macos")]
#[test]
fn a_test_runner_runs_from_the_program_store() {
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
        scratch.run_with_env(&["test", "//lib/one"], &[("BURI_HOME", home)]).ok().says("1 passed");
    };
    let clean = |home: &str| {
        scratch.run_with_env(&["clean"], &[("BURI_HOME", home)]).ok();
    };
    let variant = || only_entry(&scratch.path(".buri/out/native"));
    let runner = || scratch.path(&format!(".buri/out/native/{}/test-runner", variant()));
    let kept = || std::fs::read_link(runner()).ok();

    test(&home);
    let program = kept().expect("the runner is not a link into the store");
    assert!(program.starts_with(&home), "{} is outside the store", program.display());
    test(&home);
    assert_eq!(kept().as_ref(), Some(&program), "a second run moved the runner");

    std::fs::write(&program, "changed").unwrap();
    clean(&home);
    test(&home);
    assert_eq!(kept(), None, "an entry someone wrote to was handed out");

    let hours_ago = std::time::SystemTime::now() - std::time::Duration::from_secs(3 * 60 * 60);
    for old in [program.parent().unwrap().to_path_buf(), scratch.path("home/programs/.swept")] {
        std::fs::File::open(&old).unwrap().set_modified(hours_ago).unwrap();
    }
    clean(&home);
    test(&home);
    assert_eq!(kept().as_ref(), Some(&program), "the stale entry wasn't made again");
    assert_eq!(std::fs::read(&program).unwrap(), std::fs::read(runner()).unwrap());

    let file = scratch.write("home-is-a-file", "").display().to_string();
    clean(&file);
    test(&file);
    assert_eq!(kept(), None, "a store under a file was used");
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
        scratch.run(&["test", "//lib/one"]).ok().says("1 passed");
    };
    suite(1);
    let lock = scratch.path(&format!(".buri/out/native/{}/.test-runner.lock", only_entry(&scratch.path(".buri/out/native"))));
    std::fs::write(&lock, "").unwrap();
    let a_day_ago = std::time::SystemTime::now() - std::time::Duration::from_secs(24 * 60 * 60);
    std::fs::File::options().write(true).open(&lock).unwrap().set_modified(a_day_ago).unwrap();
    suite(2);
    assert!(!lock.exists(), "the crashed run's claim is still there");
    std::fs::write(&lock, "").unwrap();
    suite(3);
    assert!(lock.exists(), "a live run's claim was taken");
}

/// A tag's `doc` is how a policy says why, so a tag without one breaks the
/// policy with no reason under it.
#[test]
fn a_tag_without_a_doc_gives_no_reason() {
    for (case, target, code) in [
        ("platform_violation", "//cmd/web", "[platform-violation]"),
        ("tag_violation", "//cmd/web", "[tag-conflict]"),
    ] {
        let scratch = Scratch::copy_of("build-driver-tags", &tests_dir().join(format!("repositories/tags/{case}/repo")));
        let repo = scratch.read("REPO.buri");
        let undocumented: String =
            repo.lines().filter(|l| !l.trim_start().starts_with("doc:")).map(|l| format!("{l}\n")).collect();
        assert_ne!(repo, undocumented, "{case} documents no tag");
        scratch.write("REPO.buri", &undocumented);
        scratch.run(&["lint", target]).exits(1).says(code).silent_about("\": ");
    }
}

/// A binary at the repository's root is named after the root's directory.
#[test]
fn a_binary_at_the_root_is_named_after_its_directory() {
    let scratch = Scratch::repo("build-driver-root");
    scratch.write("BUILD.buri", "binary {\n    outputs: [\n        { platform: \"node\" },\n    ]\n}\n");
    scratch.write("main.buri", NODE_MAIN);
    let name = scratch.root.file_name().unwrap().to_string_lossy().to_string();
    scratch.run(&["build"]).ok().says(&format!(".buri/out/node/{name}.mjs ("));
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
  esac
  exit 0
fi
exec "$REAL_CC" "$@"
"#;

/// The link replays the linker command a driver's dry run printed, and goes
/// through the driver whenever that command can't be replayed: an error, two
/// commands, a linker named by a relative path or a script, a command that
/// doesn't name the objects, or one that fails when it runs. Each still links
/// a program that runs, from scratch and again.
///
/// The driver forwards to the `CC` this suite was given. On Linux a `cc` that
/// is a gcc can't target musl, so the link would skip the driver for `musl-gcc`.
#[test]
fn a_dry_run_the_link_cannot_replay_goes_through_the_driver() {
    let scratch = repository("        { platform: \"native\" },\n", NATIVE_MAIN);
    let driver = scratch.write_executable("fake/cc", DRY_RUN_DRIVER).display().to_string();
    let real_cc = std::env::var("CC").unwrap_or_else(|_| String::from("cc"));
    let real =
        std::process::Command::new("sh").args(["-c", &format!("command -v '{real_cc}'")]).output().unwrap();
    let real = String::from_utf8_lossy(&real.stdout).trim().to_string();
    for mode in ["error", "two", "relative", "script", "no-objects", "failing"] {
        let home = scratch.path(&format!("home-{mode}")).display().to_string();
        let env = [("CC", driver.as_str()), ("REAL_CC", real.as_str()), ("FAKE_DRY", mode), ("BURI_HOME", home.as_str())];
        for args in [&["build", "//cmd/app"][..], &["build", "--force", "//cmd/app"]] {
            scratch.run_with_env(args, &env).ok();
            let variant = only_entry(&scratch.path(".buri/out/native"));
            let out = std::process::Command::new(scratch.path(&format!(".buri/out/native/{variant}/cmd/app/app")))
                .output()
                .unwrap();
            assert_eq!(String::from_utf8_lossy(&out.stdout), "native\n", "FAKE_DRY={mode}");
        }
    }
}

/// A repository platform's `assets` land beside each artifact at their own
/// paths, and an asset the platform doesn't have is refused by name.
#[test]
fn a_platform_s_assets_are_copied_and_a_missing_one_is_refused() {
    let scratch =
        Scratch::copy_of("build-driver-assets", &tests_dir().join("repositories/custom-platforms/platform_rules/repo"));
    scratch.edit("platform/pair/BUILD.buri", "platform {\n", "platform {\n    assets: [\"style/site.css\"]\n");
    scratch
        .run(&["build", "//cmd/app"])
        .exits(1)
        .says("error: style/site.css does not exist [unknown-source]")
        .says("copying it failed");
    scratch.write("platform/pair/style/site.css", "body {}\n");
    scratch.run(&["build", "//cmd/app"]).ok();
    assert_eq!(scratch.read(".buri/out/platform/pair/cmd/app/style/site.css"), "body {}\n");
}
