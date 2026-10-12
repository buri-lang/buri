//! Debug suites on macOS arm64 load into the test host rather than linking a
//! runner (`design/PERFORMANCE.md` §6.85). Where the host can't be had, they
//! link runners as before and pass the same, and `--verbose` and `--explain`
//! say why. An image the host can't load is a runner that didn't start.
//!
//! The host is linked by the C driver `CC` names, so a driver that spoils it
//! after linking stands for each way a host can be unusable.
#![cfg(all(target_os = "macos", target_arch = "aarch64"))]

use crate::harness::*;
use crate::held::Held;

/// The line `--verbose` and `--explain` print when suites linked runners.
const LINKED_LINE: &str = "test runners were linked rather than loaded into the test host: ";

/// `//lib/a`, whose two tests pass.
fn suite(name: &str) -> Scratch {
    let scratch = Scratch::repo(name);
    scratch.write("lib/a/BUILD.buri", "library {\n  test { sources: [\"test/a.buri\"] }\n}\n");
    scratch.write("lib/a/lib.buri", "export fn one(): Int { 1 }\n");
    scratch.write(
        "lib/a/test/a.buri",
        "from \"//lib/a\" import { one };\n\
         from \"core/testing/assert\" import * as assert;\n\
         \ntest \"first\" { assert.equal(one(), 1); }\n\
         test \"second\" { assert.equal(one() + one(), 2); }\n",
    );
    scratch
}

/// A C driver that links as the real one does, then runs `spoil` in the
/// directory it linked the host in.
fn spoiling_driver(scratch: &Scratch, spoil: &str) -> String {
    let real_cc = std::env::var("CC").unwrap_or_else(|_| String::from("cc"));
    let driver = scratch.write_executable(
        "spoiling-cc",
        &format!(
            "#!/bin/sh\n\
             case \" $* \" in *\" -o test-host \"*) host=1 ;; *) host=0 ;; esac\n\
             '{real_cc}' \"$@\" || exit $?\n\
             if [ $host = 1 ]; then {spoil}; fi\n"
        ),
    );
    driver.display().to_string()
}

/// `buri test //lib/a` in `scratch` under the home `home`, passing both tests.
fn passes(scratch: &Scratch, args: &[&str], env: &[(&str, &str)]) -> Run {
    let args: Vec<&str> = ["test", "//lib/a"].iter().chain(args).copied().collect();
    let run = scratch.run_with_env(&args, env);
    run.ok();
    assert_eq!(run.tests_passed(), 2, "{}", indent(&run.all()));
    run
}

/// Each way a host can be unusable: the suite links a runner and passes, the
/// run says why only when asked, and a second run in the same home does too.
#[test]
fn a_test_host_that_cannot_be_used_leaves_suites_linking_runners() {
    let cases = [
        (
            "didn't link",
            "echo 'ld: warning: an object built for a newer macOS' >&2; echo 'ld: no test host today' >&2; exit 1",
            "the test host didn't link: ld: no test host today",
        ),
        ("isn't a program", "printf 'not a program' > test-host", "not a 64-bit Mach-O file"),
        ("can't be started", "chmod -x test-host", "the test host can't be started: "),
        ("is unsigned", "/usr/bin/codesign --remove-signature test-host", "the test host's probe was killed by SIGKILL"),
        (
            "can't write code",
            "/usr/bin/codesign -f -s - -o runtime test-host 2>/dev/null",
            "buri test host: cannot map code memory",
        ),
    ];
    for (name, spoil, why) in cases {
        let scratch = suite(&format!("test-host-{}", name.replace(['\'', ' '], "-")));
        let cc = spoiling_driver(&scratch, spoil);
        let home = scratch.path("home").display().to_string();
        let env = [("CC", cc.as_str()), ("BURI_HOME", home.as_str())];
        for flag in ["--verbose", "--explain"] {
            let _ = std::fs::remove_dir_all(scratch.path(".buri"));
            let said = passes(&scratch, &[flag], &env).stdout;
            let line = said.lines().find(|l| l.starts_with(LINKED_LINE));
            assert!(
                line.is_some_and(|l| l.contains(why)),
                "a host that {name} wasn't reported as {why:?} under {flag}:\n{}",
                indent(&said)
            );
        }
        let _ = std::fs::remove_dir_all(scratch.path(".buri"));
        passes(&scratch, &[], &env).silent_about(LINKED_LINE);
    }
}

/// With no home to keep the host in, suites link runners.
#[test]
fn a_run_with_no_home_links_runners() {
    let scratch = suite("test-host-homeless");
    let out = buri_command()
        .current_dir(&scratch.root)
        .args(["test", "--color=never", "--verbose", "//lib/a"])
        .env_remove("HOME")
        .env_remove("BURI_HOME")
        .output()
        .unwrap();
    let said = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{}{}", said, String::from_utf8_lossy(&out.stderr));
    assert!(said.contains("2 passed"), "{said}");
    assert!(
        said.contains(&format!("{LINKED_LINE}there is no home directory to keep it in")),
        "a homeless run didn't say why it linked:\n{said}"
    );
}

/// A host someone deleted from the home leaves suites linking runners.
#[test]
fn a_test_host_deleted_from_its_home_leaves_suites_linking_runners() {
    let scratch = suite("test-host-deleted");
    let home = scratch.path("home");
    let env = [("BURI_HOME", home.to_str().unwrap())];
    passes(&scratch, &["--verbose"], &env).silent_about(LINKED_LINE);
    for host in std::fs::read_dir(home.join("test-host")).unwrap().flatten() {
        std::fs::remove_file(host.path().join("test-host")).unwrap();
    }
    std::fs::remove_dir_all(scratch.path(".buri")).unwrap();
    passes(&scratch, &["--verbose"], &env).says(LINKED_LINE).says("is incomplete");
}

/// Two runs that link the host at once both load into one of them.
#[test]
fn two_runs_linking_the_test_host_at_once_both_load() {
    let first = suite("test-host-first");
    let second = suite("test-host-second");
    let home = first.path("home").display().to_string();
    let held = Held::new("test-host-placing");
    held.hold("test-host", 1);
    let mut one = held.start_with_env(&first, &["test", "//lib/a", "--verbose"], &[("BURI_HOME", &home)]);
    let placing = held.reached("test-host", 1);
    passes(&second, &["--verbose"], &[("BURI_HOME", &home)]).silent_about(LINKED_LINE);
    placing.go();
    assert!(one.wait().unwrap().success(), "{}", held.printed());
    let said = held.printed();
    assert!(said.contains("2 passed") && !said.contains(LINKED_LINE), "{said}");
}

/// An image the host can't load is reported once, as a runner that didn't
/// start, and no test in it passes.
#[test]
fn an_image_the_test_host_cannot_load_is_reported_once() {
    let scratch = suite("test-host-unreadable-image");
    passes(&scratch, &[], &[]);
    // Every image in the cache made one of a format no host reads, at the
    // same length, so the cache still hands it out.
    let mut spoiled = 0;
    for entry in walk(&scratch.path(".buri/cache")) {
        let mut bytes = std::fs::read(&entry).unwrap();
        if bytes.starts_with(b"BURIIMG\0") {
            bytes[8..12].copy_from_slice(&u32::MAX.to_le_bytes());
            std::fs::write(&entry, &bytes).unwrap();
            spoiled += 1;
        }
    }
    assert_eq!(spoiled, 1, "the cache held {spoiled} images");
    // A golden is in the verdict's key and not in the build's, so the suite
    // runs again from the image it has.
    scratch.write("lib/a/test/__snapshots__/unused.png", "not a picture\n");
    let run = scratch.run(&["test", "//lib/a"]);
    run.exits(1).says("could not start").says("buri test host: the image is format 4294967295");
    assert_eq!(run.all().matches("buri test host:").count(), 1, "{}", indent(&run.all()));
    assert_eq!(run.tests_passed(), 0, "{}", indent(&run.all()));
}

/// Every file under `dir`.
fn walk(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.is_dir() {
            out.extend(walk(&path));
        } else {
            out.push(path);
        }
    }
    out
}
